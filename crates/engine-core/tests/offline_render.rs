//! Offline render: drive `Engine::process` block by block with no audio device, write the
//! result to a WAV file and check seek, rate, reverse and slip sample-accurately.
//!
//! The test signal's left channel is a ramp `frame_index / 2^21`, so reading the output's left
//! channel back (times 2^21) gives the exact source position the scaler read. Dividing by a
//! power of two keeps every integer index exact in f32.
//!
//! In debug builds every `process` call runs inside `assert_no_alloc`, so an allocation or
//! free in the callback path aborts the test.

use std::sync::Arc;

use engine_core::render::index_ramp;
use engine_core::{CHANNELS, DeckCommand, Engine, EngineConfig, EngineHandle, Source};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SR: u32 = 48_000;
const BLOCK: usize = 128;
const SCALE: f32 = (1u32 << 21) as f32;

fn engine(channels: usize) -> (Engine, EngineHandle) {
    Engine::new(EngineConfig {
        sample_rate: SR,
        channels,
        decks: 1,
    })
}

/// Render `out` block by block; `schedule(frame, handle)` runs before each block.
fn render(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    out: &mut [f32],
    mut schedule: impl FnMut(usize, &mut EngineHandle),
) {
    let ch = engine.config().channels;
    let frames = out.len() / ch;
    let mut done = 0;
    while done < frames {
        schedule(done, handle);
        let n = BLOCK.min(frames - done);
        let block = &mut out[done * ch..(done + n) * ch];
        assert_no_alloc::assert_no_alloc(|| engine.process(block, n));
        handle.collect_retired();
        done += n;
    }
}

fn send(h: &mut EngineHandle, cmd: DeckCommand) {
    h.send_deck(0, cmd).expect("command ring full");
}

fn read_position(frame: &[f32]) -> f64 {
    frame[0] as f64 * SCALE as f64
}

/// Expected source position (or `None` for silence) per output frame of the scripted run.
#[derive(Clone, Copy)]
struct Piece {
    from: usize,
    /// Source position at `from`, frames per output frame; `None` = silence.
    pos: Option<(f64, f64)>,
}

#[test]
fn ten_seconds_scripted_render_is_sample_accurate() {
    let seconds = 10;
    let frames = SR as usize * seconds;
    let s = SR as usize;
    // 30 s of signal so seeks have somewhere to go.
    let signal = index_ramp(30 * s, SCALE, 440.0, SR);
    let source = Source::from_interleaved(SR, &signal);

    let (mut eng, mut h) = engine(2);
    eng.deck_mut(0).set_declick(false);
    let mut out = vec![f32::NAN; frames * 2];

    render(&mut eng, &mut h, &mut out, |f, h| match f {
        0 => {
            send(h, DeckCommand::Load(Arc::clone(&source)));
            send(h, DeckCommand::Play);
        }
        // 1 s: seek to 5 s.
        f if f == s => send(h, DeckCommand::Seek(5.0 * s as f64)),
        // 2 s: rate +1 % (fractional positions).
        f if f == 2 * s => send(h, DeckCommand::SetRate(1.01)),
        // 3 s: negative rate plays backwards.
        f if f == 3 * s => send(h, DeckCommand::SetRate(-1.0)),
        // 4 s: rate 1, slip on, reverse (slip-able).
        f if f == 4 * s => {
            send(h, DeckCommand::SetRate(1.0));
            send(h, DeckCommand::SetSlip(true));
            send(h, DeckCommand::SetReverse(true));
        }
        // 5 s: release reverse: snap to the shadow.
        f if f == 5 * s => send(h, DeckCommand::SetReverse(false)),
        // 6 s: slip pause; 7 s: play again (snap).
        f if f == 6 * s => send(h, DeckCommand::Pause),
        f if f == 7 * s => send(h, DeckCommand::Play),
        // 8 s: rate 0 holds; 9 s: back to 1.
        f if f == 8 * s => send(h, DeckCommand::SetRate(0.0)),
        f if f == 9 * s => send(h, DeckCommand::SetRate(1.0)),
        _ => {}
    });

    // Expected positions, piece by piece.
    let sf = s as f64;
    let p1 = 5.0 * sf + sf; // at 2 s
    let p2 = p1 + 1.01 * sf; // at 3 s
    let p3 = p2 - sf; // at 4 s
    let shadow_at_5 = p3 + sf; // shadow kept running forward during slip reverse
    let p6 = shadow_at_5 + sf; // at 6 s
    let p7 = p6 + sf; // shadow after one second of slip pause
    let p8 = p7 + sf; // at 8 s; rate 0 holds here
    let pieces = [
        Piece {
            from: 0,
            pos: Some((0.0, 1.0)),
        },
        Piece {
            from: s,
            pos: Some((5.0 * sf, 1.0)),
        },
        Piece {
            from: 2 * s,
            pos: Some((p1, 1.01)),
        },
        Piece {
            from: 3 * s,
            pos: Some((p2, -1.0)),
        },
        Piece {
            from: 4 * s,
            pos: Some((p3, -1.0)),
        },
        Piece {
            from: 5 * s,
            pos: Some((shadow_at_5, 1.0)),
        },
        Piece {
            from: 6 * s,
            pos: None,
        },
        Piece {
            from: 7 * s,
            pos: Some((p7, 1.0)),
        },
        Piece {
            from: 8 * s,
            pos: Some((p8, 0.0)),
        },
        Piece {
            from: 9 * s,
            pos: Some((p8, 1.0)),
        },
    ];

    let mut worst: f64 = 0.0;
    for (k, frame) in out.chunks_exact(2).enumerate() {
        let piece = pieces.iter().rev().find(|p| p.from <= k).unwrap();
        match piece.pos {
            None => assert_eq!(frame, [0.0, 0.0], "frame {k} should be silent"),
            Some((start, step)) => {
                let expected = start + (k - piece.from) as f64 * step;
                let got = read_position(frame);
                let err = (got - expected).abs();
                worst = worst.max(err);
                if expected.fract() == 0.0 {
                    assert_eq!(got, expected, "frame {k}: integer position must be exact");
                } else {
                    assert!(err < 0.05, "frame {k}: expected {expected}, got {got}");
                }
            }
        }
    }
    eprintln!("worst position error over the render: {worst:.4} frames");

    // The first frame after each command boundary is exactly where the script puts it.
    for (k, expected) in [
        (s, 5.0 * sf),
        (4 * s, p3),
        (5 * s, shadow_at_5),
        (7 * s, p7),
    ] {
        assert_eq!(read_position(&out[2 * k..2 * k + 2]), expected, "frame {k}");
    }

    let state = h.deck_state(0);
    // 1.01 is not exact in binary; 375 blocks of 128 * 1.01 accumulate ~1e-8 frames.
    assert!((state.position.load() - (p8 + sf)).abs() < 1e-6);
    assert_eq!(state.shadow_position.load(), state.position.load());
    assert!(
        state
            .slip_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(!state.slip_active.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(
        state
            .starved_blocks
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    // Write the render to a WAV file and read it back.
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("offline_render.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for &x in &out {
        w.write_sample(x).unwrap();
    }
    w.finalize().unwrap();
    let mut r = hound::WavReader::open(&path).unwrap();
    assert_eq!(r.duration() as usize, frames);
    let back: Vec<f32> = r.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(back, out);
    eprintln!("wrote {}", path.display());
}

#[test]
fn unloaded_chunk_gives_silence_and_a_flag_and_skips_nothing() {
    let chunk = 1024;
    let total = 16 * chunk;
    let signal = index_ramp(total, SCALE, 440.0, SR);
    let (source, mut writer) = Source::with_chunk_frames(SR, total, chunk);
    // Only the first 4 chunks are decoded.
    writer.push(&signal[..4 * chunk * CHANNELS]);

    let (mut eng, mut h) = engine(2);
    eng.deck_mut(0).set_declick(false);
    send(&mut h, DeckCommand::Load(Arc::clone(&source)));
    send(&mut h, DeckCommand::Play);

    let mut out = vec![f32::NAN; 64 * BLOCK * 2];
    render(&mut eng, &mut h, &mut out, |_, _| {});
    let state = Arc::clone(h.deck_state(0));
    let starved = state
        .starved_blocks
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(starved > 0);
    assert!(state.starved.load(std::sync::atomic::Ordering::Relaxed));
    // Playback stopped short of the unloaded chunk (the scaler reads up to 2 frames ahead).
    let held = state.position.load();
    assert!(held <= (4 * chunk - 2) as f64 && held > (4 * chunk - 2 - BLOCK) as f64);
    let audible: Vec<f64> = out
        .chunks_exact(2)
        .filter(|f| f != &[0.0, 0.0])
        .map(read_position)
        .collect();
    assert_eq!(audible.len(), held as usize - 1, "frame 0 reads as silence");
    for (i, p) in audible.iter().enumerate() {
        assert_eq!(*p, (i + 1) as f64);
    }

    // The loader catches up; playback resumes exactly where it stopped.
    writer.push(&signal[4 * chunk * CHANNELS..]);
    writer.finish();
    let mut out = vec![f32::NAN; 4 * BLOCK * 2];
    render(&mut eng, &mut h, &mut out, |_, _| {});
    assert!(!state.starved.load(std::sync::atomic::Ordering::Relaxed));
    for (k, f) in out.chunks_exact(2).enumerate() {
        assert_eq!(read_position(f), held + k as f64);
    }
}

#[test]
fn declick_bounds_the_jump_at_a_seek() {
    // A full-scale 100 Hz sine: the largest natural step between frames is ~0.013.
    let s = SR as usize;
    let w = std::f32::consts::TAU * 100.0 / SR as f32;
    let signal: Vec<f32> = (0..4 * s)
        .flat_map(|i| {
            let x = (w * i as f32).sin();
            [x, x]
        })
        .collect();
    let source = Source::from_interleaved(SR, &signal);
    let (mut eng, mut h) = engine(2);
    let mut out = vec![0.0; s * 2];
    // Seek a quarter period away at 0.5 s: without a fade that is a jump of up to 2.0.
    render(&mut eng, &mut h, &mut out, |f, h| {
        if f == 0 {
            send(h, DeckCommand::Load(Arc::clone(&source)));
            send(h, DeckCommand::Play);
        } else if f == s / 2 {
            send(h, DeckCommand::Seek((s / 2 + 120) as f64));
        }
    });
    let max_step = out
        .chunks_exact(2)
        .zip(out.chunks_exact(2).skip(1))
        .skip(1)
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.05, "max step {max_step}");
}
