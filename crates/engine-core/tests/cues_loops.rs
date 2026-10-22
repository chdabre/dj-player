//! Cues, loops and beat jumps through the real engine, offline and sample-accurate.
//!
//! The source's left channel is a ramp `frame / 2^21`, so the output tells exactly which
//! source position each output frame read. In debug builds every callback runs inside
//! `assert_no_alloc`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use engine_core::render::index_ramp;
use engine_core::{
    CHANNELS, DeckCommand, DeckGrid, Engine, EngineConfig, EngineHandle, HOT_CUES, Source,
    TrackCues,
};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SR: u32 = 48_000;
const BLOCK: usize = 128;
const SCALE: f32 = (1u32 << 21) as f32;
/// A non-round tempo, so beats fall between frames: 23328.4 frames per beat.
const BPM: f64 = 123.456;
const FIRST_BEAT: f64 = 1_000.0;

struct Rig {
    engine: Engine,
    handle: EngineHandle,
    grid: Arc<DeckGrid>,
}

fn rig(declick: bool) -> Rig {
    let (mut engine, mut handle) = Engine::new(EngineConfig {
        sample_rate: SR,
        channels: 2,
        decks: 1,
    });
    engine.deck_mut(0).set_declick(declick);
    let frames = 60 * SR as usize;
    let source = Source::from_interleaved(SR, &index_ramp(frames, SCALE, 440.0, SR));
    let grid = Arc::new(DeckGrid::constant(FIRST_BEAT, BPM, frames as f64, SR).unwrap());
    send(&mut handle, DeckCommand::Load(Arc::clone(&source)));
    send(
        &mut handle,
        DeckCommand::SetGrid {
            source_id: source.id(),
            grid: Arc::clone(&grid),
        },
    );
    let mut rig = Rig {
        engine,
        handle,
        grid,
    };
    rig.render(BLOCK);
    rig
}

fn send(h: &mut EngineHandle, cmd: DeckCommand) {
    h.send_deck(0, cmd).expect("command ring full");
}

impl Rig {
    fn send(&mut self, cmd: DeckCommand) {
        send(&mut self.handle, cmd);
    }

    /// Render `frames` frames in blocks; returns the source position read per output frame.
    fn render(&mut self, frames: usize) -> Vec<f64> {
        let mut out = vec![0.0f32; frames * CHANNELS];
        let mut done = 0;
        while done < frames {
            let n = BLOCK.min(frames - done);
            let block = &mut out[done * CHANNELS..(done + n) * CHANNELS];
            let e = &mut self.engine;
            assert_no_alloc::assert_no_alloc(|| e.process(block, n));
            self.handle.collect_retired();
            done += n;
        }
        out.as_chunks::<CHANNELS>()
            .0
            .iter()
            .map(|f| f64::from(f[0]) * f64::from(SCALE))
            .collect()
    }

    fn position(&self) -> f64 {
        self.handle.deck_state(0).position.load()
    }
}

#[test]
fn beat_loops_wrap_on_the_exact_frame() {
    for beats in [0.5, 1.0, 2.0, 4.0, 8.0, 16.0] {
        let mut r = rig(false);
        r.send(DeckCommand::Seek(r.grid.position_of(10.0) + 5_000.0));
        r.send(DeckCommand::Play);
        r.render(BLOCK);
        r.send(DeckCommand::BeatLoop(beats));
        // Quantized to the beat at or nearest the playhead: beat 10.
        let start = r.grid.position_of(10.0);
        let end = r.grid.position_of(10.0 + beats);
        let len = end - start;
        let p0 = r.position();
        let frames = (len * 2.5) as usize;
        let got = r.render(frames);
        for (k, &pos) in got.iter().enumerate() {
            let expected = start + (p0 + k as f64 - start).rem_euclid(len);
            assert!(
                (pos - expected).abs() < 0.05,
                "{beats} beats, frame {k}: read {pos}, expected {expected}"
            );
        }
        assert!(r.handle.deck_state(0).looping.load(Ordering::Relaxed));
    }
}

#[test]
fn reverse_wraps_from_the_start_to_the_end() {
    let mut r = rig(false);
    r.send(DeckCommand::Seek(r.grid.position_of(20.0)));
    r.send(DeckCommand::Play);
    r.send(DeckCommand::BeatLoop(1.0));
    r.render(BLOCK);
    r.send(DeckCommand::SetReverse(true));
    let start = r.grid.position_of(20.0);
    let len = r.grid.position_of(21.0) - start;
    let p0 = r.position();
    let got = r.render(3 * len as usize);
    for (k, &pos) in got.iter().enumerate() {
        let expected = start + (p0 - k as f64 - start).rem_euclid(len);
        assert!(
            (pos - expected).abs() < 0.05,
            "frame {k}: {pos} vs {expected}"
        );
    }
}

#[test]
fn exiting_a_slip_loop_snaps_to_the_shadow() {
    let mut r = rig(false);
    r.send(DeckCommand::SetSlip(true));
    r.send(DeckCommand::Seek(r.grid.position_of(4.0)));
    r.send(DeckCommand::Play);
    r.send(DeckCommand::BeatLoop(1.0));
    let entered = r.grid.position_of(4.0);
    let frames = 40 * BLOCK;
    r.render(frames);
    r.send(DeckCommand::ReloopExit);
    let got = r.render(BLOCK);
    // The shadow ran straight on from where the loop started.
    let shadow = entered + frames as f64;
    assert!((got[0] - shadow).abs() < 0.05, "{} vs {shadow}", got[0]);
}

#[test]
fn cue_and_hot_cues_land_on_grid_lines() {
    let mut r = rig(false);
    let near = r.grid.position_of(7.0) + 3_000.0;
    r.send(DeckCommand::Seek(near));
    r.send(DeckCommand::CuePress);
    r.render(BLOCK);
    let cue = r.handle.deck_state(0).cue.load();
    assert!((cue - r.grid.position_of(7.0)).abs() < 1e-6, "{cue}");
    r.send(DeckCommand::HotCue(2));
    r.render(BLOCK);
    assert_eq!(
        r.handle.deck_state(0).hot_cues.load(Ordering::Relaxed),
        0b100
    );
    let hot = &r.handle.deck_state(0).hot_cue_positions;
    assert!((hot[2].load() - r.grid.position_of(7.0)).abs() < 1e-6);
    assert!(hot[0].load().is_nan() && hot[1].load().is_nan());
    // Elsewhere, CUE while playing: back to the cue, paused.
    r.send(DeckCommand::Seek(r.grid.position_of(30.0)));
    r.send(DeckCommand::Play);
    r.render(BLOCK);
    r.send(DeckCommand::CuePress);
    r.render(BLOCK);
    assert!((r.position() - cue).abs() < 1e-6);
    assert!(!r.handle.deck_state(0).playing.load(Ordering::Relaxed));
}

#[test]
fn beat_jump_moves_whole_beats() {
    let mut r = rig(false);
    let from = r.grid.position_of(12.25);
    r.send(DeckCommand::Seek(from));
    r.send(DeckCommand::BeatJump(4.0));
    r.render(BLOCK);
    assert!((r.position() - r.grid.position_of(16.25)).abs() < 1e-6);
}

#[test]
fn loop_wraps_without_a_click() {
    // A 100 Hz sine; the wrap lands at an arbitrary phase.
    let (mut engine, mut handle) = Engine::new(EngineConfig {
        sample_rate: SR,
        channels: 2,
        decks: 1,
    });
    let w = std::f32::consts::TAU * 100.0 / SR as f32;
    let frames = 20 * SR as usize;
    let signal: Vec<f32> = (0..frames)
        .flat_map(|i| {
            let x = 0.5 * (w * i as f32).sin();
            [x, x]
        })
        .collect();
    let source = Source::from_interleaved(SR, &signal);
    send(&mut handle, DeckCommand::Load(source));
    send(&mut handle, DeckCommand::SetQuantize(None));
    send(&mut handle, DeckCommand::Seek(12_345.0));
    send(&mut handle, DeckCommand::Play);
    send(&mut handle, DeckCommand::BeatLoop(0.5));
    let mut out = vec![0.0f32; 2 * SR as usize * CHANNELS];
    for block in out.chunks_mut(BLOCK * CHANNELS) {
        let n = block.len() / CHANNELS;
        assert_no_alloc::assert_no_alloc(|| engine.process(block, n));
        handle.collect_retired();
    }
    // Natural steps of this sine are up to ~0.0065; a raw wrap could jump by up to 1.0.
    let max_step = out
        .as_chunks::<CHANNELS>()
        .0
        .iter()
        .zip(out.as_chunks::<CHANNELS>().0.iter().skip(1))
        .skip(BLOCK)
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.03, "max step {max_step}");
}

#[test]
fn stored_cues_restore_into_the_deck() {
    let mut r = rig(false);
    let id = r.engine.deck(0).source().unwrap().id();
    let mut hot = [None; HOT_CUES];
    hot[0] = Some(48_000.0);
    hot[5] = Some(96_000.5);
    let cues = TrackCues {
        cue: Some(24_000.0),
        hot,
        stored_loop: Some((200_000.0, 248_000.0)),
        fill_only: false,
    };
    // Cues of another track are ignored.
    r.send(DeckCommand::SetCues {
        source_id: id + 1,
        serial: 1,
        cues: Arc::new(cues),
    });
    r.render(BLOCK);
    let st = r.handle.deck_state(0);
    assert_eq!(st.cues_serial.load(Ordering::Relaxed), 0);
    assert_eq!(st.hot_cues.load(Ordering::Relaxed), 0);

    r.send(DeckCommand::SetCues {
        source_id: id,
        serial: 2,
        cues: Arc::new(cues),
    });
    r.render(BLOCK);
    let st = r.handle.deck_state(0);
    assert_eq!(st.cues_serial.load(Ordering::Relaxed), 2);
    assert_eq!(st.cue.load(), 24_000.0);
    assert_eq!(
        r.position(),
        24_000.0,
        "a paused deck at the start moves to the cue"
    );
    assert_eq!(st.hot_cues.load(Ordering::Relaxed), 0b10_0001);
    assert_eq!(st.hot_cue_positions[5].load(), 96_000.5);
    assert!(st.hot_cue_positions[1].load().is_nan());
    assert!(
        !st.looping.load(Ordering::Relaxed),
        "the loop is stored, not entered"
    );
    assert_eq!(st.loop_start.load(), 200_000.0);

    // The restored hot cue plays like one set on the deck; RELOOP enters the stored loop.
    r.send(DeckCommand::HotCue(5));
    r.render(BLOCK);
    assert_eq!(r.position(), 96_000.5);
    r.send(DeckCommand::ReloopExit);
    r.render(BLOCK);
    assert!(r.handle.deck_state(0).looping.load(Ordering::Relaxed));
    assert_eq!(r.position(), 200_000.0);
}

#[test]
fn a_first_restore_keeps_what_the_dj_set_meanwhile() {
    let mut r = rig(false);
    let id = r.engine.deck(0).source().unwrap().id();
    // The DJ sets hot cue B before the library has answered.
    r.send(DeckCommand::Seek(30_000.0));
    r.render(BLOCK);
    r.send(DeckCommand::HotCue(1));
    r.render(BLOCK);
    let st = r.handle.deck_state(0);
    let dj = st.hot_cue_positions[1].load();
    assert!(!dj.is_nan());
    let mut hot = [None; HOT_CUES];
    hot[0] = Some(48_000.0);
    hot[1] = Some(96_000.0);
    let stored = TrackCues {
        cue: Some(24_000.0),
        hot,
        stored_loop: None,
        fill_only: true,
    };
    r.send(DeckCommand::SetCues {
        source_id: id,
        serial: 1,
        cues: Arc::new(stored),
    });
    r.render(BLOCK);
    let st = r.handle.deck_state(0);
    assert_eq!(st.cues_serial.load(Ordering::Relaxed), 1);
    assert_eq!(
        st.hot_cue_positions[0].load(),
        48_000.0,
        "an empty slot is filled"
    );
    assert_eq!(st.hot_cue_positions[1].load(), dj, "the DJ's hot cue stays");
    // A full restore (the record after a merge) replaces the slots.
    r.send(DeckCommand::SetCues {
        source_id: id,
        serial: 2,
        cues: Arc::new(TrackCues {
            fill_only: false,
            ..stored
        }),
    });
    r.render(BLOCK);
    assert_eq!(r.handle.deck_state(0).hot_cue_positions[1].load(), 96_000.0);
}

#[test]
fn scratching_moves_a_paused_track_by_hand_and_letting_go_leaves_it_there() {
    let mut r = rig(true);
    r.send(DeckCommand::SetKeylock(true));
    r.send(DeckCommand::Seek(100_000.0));
    r.render(BLOCK);
    r.send(DeckCommand::ScratchBegin);
    // The hand moves the track 0.5 s forward in 50 updates, one per 4 blocks (~10 ms).
    let mut reads = Vec::new();
    for _ in 0..50 {
        r.send(DeckCommand::ScratchBy(24_000.0 / 50.0));
        reads.extend(r.render(4 * BLOCK));
    }
    let st = Arc::clone(r.handle.deck_state(0));
    assert!(st.scratching.load(Ordering::Relaxed));
    assert!(
        !st.stretching.load(Ordering::Relaxed),
        "scratch plays through the resampler, not the keylock"
    );
    reads.extend(r.render(100 * BLOCK));
    assert!((r.position() - 124_000.0).abs() < 0.5, "{}", r.position());
    // The audio is the track moving forward: the frames read never go backwards (after the
    // declick at the start).
    let backwards = reads[engine_core::DECLICK_FRAMES..]
        .windows(2)
        .filter(|w| w[1] < w[0] - 0.05)
        .count();
    assert_eq!(backwards, 0);
    r.send(DeckCommand::ScratchEnd);
    r.render(BLOCK);
    assert!(!st.playing.load(Ordering::Relaxed) && !st.scratching.load(Ordering::Relaxed));
    assert!((r.position() - 124_000.0).abs() < 0.5);
}

#[test]
fn letting_go_of_a_playing_track_plays_on_at_its_rate() {
    let mut r = rig(true);
    r.send(DeckCommand::Seek(100_000.0));
    r.send(DeckCommand::Play);
    r.render(10 * BLOCK);
    r.send(DeckCommand::ScratchBegin);
    r.send(DeckCommand::ScratchBy(-12_000.0));
    r.render(40 * BLOCK);
    r.send(DeckCommand::ScratchEnd);
    let before = r.position();
    r.render(10 * BLOCK);
    assert!((r.position() - before - 10.0 * BLOCK as f64).abs() < 1e-6);
    assert!(r.handle.deck_state(0).playing.load(Ordering::Relaxed));
}

#[test]
fn taking_and_releasing_the_track_does_not_click() {
    // A 100 Hz sine, paused: taking it, moving it and letting go must stay smooth.
    let (mut engine, mut handle) = Engine::new(EngineConfig {
        sample_rate: SR,
        channels: 2,
        decks: 1,
    });
    let w = std::f32::consts::TAU * 100.0 / SR as f32;
    let signal: Vec<f32> = (0..10 * SR as usize)
        .flat_map(|i| {
            let x = 0.5 * (w * i as f32).sin();
            [x, x]
        })
        .collect();
    send(
        &mut handle,
        DeckCommand::Load(Source::from_interleaved(SR, &signal)),
    );
    send(&mut handle, DeckCommand::Seek(48_123.0));
    let mut out = vec![0.0f32; 60 * BLOCK * CHANNELS];
    for (b, block) in out.chunks_mut(BLOCK * CHANNELS).enumerate() {
        match b {
            5 => send(&mut handle, DeckCommand::ScratchBegin),
            6..=30 => send(&mut handle, DeckCommand::ScratchBy(60.0)),
            45 => send(&mut handle, DeckCommand::ScratchEnd),
            _ => {}
        }
        let n = block.len() / CHANNELS;
        assert_no_alloc::assert_no_alloc(|| engine.process(block, n));
        handle.collect_retired();
    }
    let max_step = out
        .as_chunks::<CHANNELS>()
        .0
        .iter()
        .zip(out.as_chunks::<CHANNELS>().0.iter().skip(1))
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0, f32::max);
    // The sine's own steps at up to 4x are ~0.026; a hard start or stop could step by 0.5.
    assert!(max_step < 0.05, "max step {max_step}");
}
