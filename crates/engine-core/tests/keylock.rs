//! Keylock through the real engine, offline: pitch is kept, the audio lines up with the
//! reported playhead (latency compensation), reverse keeps the pitch (Bungee, the default),
//! and switching paths or wrapping a loop does not click. The alignment test also runs on the
//! Signalsmith push path.
//!
//! In debug builds every `process` call runs inside `assert_no_alloc`. Bungee is pure Rust,
//! so that covers it completely; Signalsmith's C++ `malloc` calls are not visible to it.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use engine_core::{CHANNELS, DeckCommand, Engine, EngineConfig, EngineHandle, Keylock, Source};
use engine_stretch::SignalsmithStretcher;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SR: u32 = 48_000;
const S: usize = SR as usize;
const BLOCK: usize = 128;

fn engine() -> (Engine, EngineHandle) {
    Engine::new(EngineConfig {
        sample_rate: SR,
        channels: 2,
        decks: 1,
    })
}

/// An engine whose deck uses the Signalsmith push path instead of Bungee.
fn signalsmith_engine() -> (Engine, EngineHandle) {
    let (mut eng, h) = engine();
    let push = Keylock::new(Box::new(SignalsmithStretcher::new(CHANNELS, SR)));
    drop(eng.deck_mut(0).replace_keylock(Some(push)));
    (eng, h)
}

fn render(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    out: &mut [f32],
    mut schedule: impl FnMut(usize, &mut EngineHandle),
) {
    let frames = out.len() / CHANNELS;
    let mut done = 0;
    while done < frames {
        schedule(done, handle);
        let n = BLOCK.min(frames - done);
        let block = &mut out[done * CHANNELS..(done + n) * CHANNELS];
        assert_no_alloc::assert_no_alloc(|| engine.process(block, n));
        handle.collect_retired();
        done += n;
    }
}

fn send(h: &mut EngineHandle, cmd: DeckCommand) {
    h.send_deck(0, cmd).expect("command ring full");
}

fn sine(frames: usize, hz: f64, amp: f64) -> Vec<f32> {
    let w = std::f64::consts::TAU * hz / f64::from(SR);
    (0..frames)
        .flat_map(|i| {
            let x = (amp * (w * i as f64).sin()) as f32;
            [x, x]
        })
        .collect()
}

/// Frequency of the left channel from rising zero crossings.
fn frequency(interleaved: &[f32]) -> f64 {
    let left: Vec<f32> = interleaved.iter().step_by(CHANNELS).copied().collect();
    let rising: Vec<usize> = (1..left.len())
        .filter(|&i| left[i - 1] < 0.0 && left[i] >= 0.0)
        .collect();
    let (first, last) = (rising[0], rising[rising.len() - 1]);
    (rising.len() - 1) as f64 * f64::from(SR) / (last - first) as f64
}

fn play_sine(keylock: bool, rate: f64) -> (Vec<f32>, f64, bool) {
    let source = Source::from_interleaved(SR, &sine(10 * S, 1000.0, 0.5));
    let (mut eng, mut h) = engine();
    let mut out = vec![0.0; 2 * S * CHANNELS];
    render(&mut eng, &mut h, &mut out, |f, h| {
        if f == 0 {
            send(h, DeckCommand::Load(Arc::clone(&source)));
            send(h, DeckCommand::SetKeylock(keylock));
            send(h, DeckCommand::SetRate(rate));
            send(h, DeckCommand::Play);
        }
    });
    let state = h.deck_state(0);
    (
        out,
        state.position.load(),
        state.stretching.load(Ordering::Relaxed),
    )
}

#[test]
fn keylock_keeps_pitch_and_still_changes_speed() {
    for rate in [0.92, 1.08, 1.5] {
        let (out, position, stretching) = play_sine(true, rate);
        let f = frequency(&out[S / 4 * CHANNELS..]);
        assert!((f - 1000.0).abs() < 5.0, "rate {rate}: {f} Hz with keylock");
        assert!(stretching);
        assert!((position - rate * 2.0 * S as f64).abs() < 1e-6);
    }
    let (out, _, stretching) = play_sine(false, 1.08);
    let f = frequency(&out[S / 4 * CHANNELS..]);
    assert!((f - 1080.0).abs() < 5.0, "{f} Hz without keylock");
    assert!(!stretching);
}

/// A 1 kHz burst at each of `onsets` (source frames), silence elsewhere.
fn bursts(frames: usize, onsets: &[usize]) -> Vec<f32> {
    let tone = sine(S / 10, 1000.0, 0.5);
    let mut signal = vec![0.0; frames * CHANNELS];
    for &at in onsets {
        signal[at * CHANNELS..at * CHANNELS + tone.len()].copy_from_slice(&tone);
    }
    signal
}

/// First output frame at or after `from` whose level exceeds a quarter of the burst.
fn onset(out: &[f32], from: usize) -> usize {
    out.chunks_exact(CHANNELS)
        .enumerate()
        .skip(from)
        .find(|(_, f)| f[0].abs() > 0.125)
        .map(|(k, _)| k)
        .expect("no onset")
}

#[test]
fn audio_lines_up_with_the_reported_playhead() {
    lines_up(engine());
}

#[test]
fn signalsmith_audio_lines_up_with_the_reported_playhead() {
    lines_up(signalsmith_engine());
}

fn lines_up((mut eng, mut h): (Engine, EngineHandle)) {
    // Bursts at 2, 4, 8 and 12 s of the track.
    let marks = [2 * S, 4 * S, 8 * S, 12 * S];
    let source = Source::from_interleaved(SR, &bursts(20 * S, &marks));
    eng.deck_mut(0).set_declick(false);
    let mut out = vec![0.0; 10 * S * CHANNELS];
    let state = Arc::clone(h.deck_state(0));
    // Track position at the start of every block, as the UI would read it.
    let mut positions = Vec::new();
    render(&mut eng, &mut h, &mut out, |f, h| {
        positions.push((f, state.position.load()));
        match f {
            0 => {
                send(h, DeckCommand::Load(Arc::clone(&source)));
                send(h, DeckCommand::SetKeylock(true));
                send(h, DeckCommand::SetRate(1.25));
                send(h, DeckCommand::Play);
            }
            // 2.5 s: jump to 0.5 s before the 4 s burst.
            f if f == 2 * S + S / 2 => send(h, DeckCommand::Seek((4 * S - S / 2) as f64)),
            // 3.5 s: tempo change on the fly, 2.5 s before the 8 s burst.
            f if f == 3 * S + S / 2 => send(h, DeckCommand::SetRate(0.92)),
            _ => {}
        }
    });

    // Output frame at which the reported playhead passes `mark`.
    let when = |mark: usize| -> f64 {
        let w = positions
            .windows(2)
            .find(|w| w[1].1 >= mark as f64)
            .unwrap();
        let ((f0, p0), (f1, p1)) = (w[0], w[1]);
        f0 as f64 + (mark as f64 - p0) / (p1 - p0) * (f1 - f0) as f64
    };
    let mut from = 0;
    for &mark in &marks[..3] {
        let expected = when(mark);
        let got = onset(&out, from);
        let err_ms = (got as f64 - expected) * 1000.0 / f64::from(SR);
        eprintln!(
            "burst at {mark}: heard at {got}, playhead there at {expected:.0} ({err_ms:+.1} ms)"
        );
        assert!(err_ms.abs() < 5.0, "burst {mark} off by {err_ms:.1} ms");
        from = got + S / 5;
    }
}

#[test]
fn reverse_keeps_the_pitch() {
    let source = Source::from_interleaved(SR, &sine(10 * S, 1000.0, 0.5));
    let (mut eng, mut h) = engine();
    let mut out = vec![0.0; 2 * S * CHANNELS];
    let reverse_at = 200 * BLOCK;
    render(&mut eng, &mut h, &mut out, |f, h| {
        if f == 0 {
            send(h, DeckCommand::Load(Arc::clone(&source)));
            send(h, DeckCommand::SetKeylock(true));
            send(h, DeckCommand::SetRate(1.08));
            send(h, DeckCommand::Seek(5.0 * S as f64));
            send(h, DeckCommand::Play);
        } else if f == reverse_at {
            send(h, DeckCommand::SetReverse(true));
        }
    });
    let state = h.deck_state(0);
    assert!(
        state.stretching.load(Ordering::Relaxed),
        "reverse went to the resampler"
    );
    let f = frequency(&out[(reverse_at + S / 4) * CHANNELS..]);
    assert!((f - 1000.0).abs() < 5.0, "{f} Hz in reverse");
    let expected = 5.0 * S as f64 + 1.08 * (2 * reverse_at as i64 - 2 * S as i64) as f64;
    assert!((state.position.load() - expected).abs() < 1e-6);
}

#[test]
fn reverse_audio_lines_up_with_the_reported_playhead() {
    // Bursts starting at 2, 4 and 6 s; played backwards, each is heard from its end.
    let marks = [2 * S, 4 * S, 6 * S];
    let len = S / 10;
    let source = Source::from_interleaved(SR, &bursts(20 * S, &marks));
    let (mut eng, mut h) = engine();
    eng.deck_mut(0).set_declick(false);
    let mut out = vec![0.0; 6 * S * CHANNELS];
    let state = Arc::clone(h.deck_state(0));
    let mut positions = Vec::new();
    render(&mut eng, &mut h, &mut out, |f, h| {
        positions.push((f, state.position.load()));
        match f {
            0 => {
                send(h, DeckCommand::Load(Arc::clone(&source)));
                send(h, DeckCommand::SetKeylock(true));
                send(h, DeckCommand::SetRate(0.92));
                send(h, DeckCommand::Seek(7.0 * S as f64));
                send(h, DeckCommand::SetReverse(true));
                send(h, DeckCommand::Play);
            }
            // Tempo change on the fly, between the 6 s and 4 s bursts.
            f if f == 2 * S => send(h, DeckCommand::SetRate(1.08)),
            _ => {}
        }
    });
    let when = |mark: usize| -> f64 {
        let w = positions
            .windows(2)
            .find(|w| w[1].1 <= mark as f64)
            .unwrap();
        let ((f0, p0), (f1, p1)) = (w[0], w[1]);
        f0 as f64 + (mark as f64 - p0) / (p1 - p0) * (f1 - f0) as f64
    };
    let mut from = 0;
    for &mark in marks.iter().rev() {
        let expected = when(mark + len);
        let got = onset(&out, from);
        let err_ms = (got as f64 - expected) * 1000.0 / f64::from(SR);
        eprintln!(
            "burst ending at {}: heard at {got}, playhead there at {expected:.0} ({err_ms:+.1} ms)",
            mark + len
        );
        assert!(err_ms.abs() < 5.0, "burst {mark} off by {err_ms:.1} ms");
        from = got + S / 5;
    }
}

#[test]
fn keylock_loop_wraps_without_a_click() {
    // A 100 Hz sine; the wrap lands at an arbitrary phase.
    let source = Source::from_interleaved(SR, &sine(20 * S, 100.0, 0.5));
    let (mut eng, mut h) = engine();
    let mut out = vec![0.0; 2 * S * CHANNELS];
    render(&mut eng, &mut h, &mut out, |f, h| {
        if f == 0 {
            send(h, DeckCommand::Load(Arc::clone(&source)));
            send(h, DeckCommand::SetKeylock(true));
            send(h, DeckCommand::SetRate(1.04));
            send(h, DeckCommand::SetQuantize(None));
            send(h, DeckCommand::Seek(12_345.0));
            send(h, DeckCommand::Play);
            send(h, DeckCommand::BeatLoop(0.5));
        }
    });
    assert!(h.deck_state(0).stretching.load(Ordering::Relaxed));
    assert!(h.deck_state(0).looping.load(Ordering::Relaxed));
    // Natural steps of this sine are up to ~0.0065; a raw wrap could jump by up to 1.0.
    let max_step = out
        .chunks_exact(CHANNELS)
        .zip(out.chunks_exact(CHANNELS).skip(1))
        .skip(BLOCK)
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.03, "max step {max_step}");
}

#[test]
fn toggling_keylock_while_playing_does_not_click() {
    // A 100 Hz sine: the largest natural step between frames is ~0.007 at this level.
    let source = Source::from_interleaved(SR, &sine(10 * S, 100.0, 0.5));
    let (mut eng, mut h) = engine();
    let mut out = vec![0.0; 2 * S * CHANNELS];
    render(&mut eng, &mut h, &mut out, |f, h| match f {
        0 => {
            send(h, DeckCommand::Load(Arc::clone(&source)));
            send(h, DeckCommand::SetRate(1.06));
            send(h, DeckCommand::Play);
        }
        f if f == S / 2 => send(h, DeckCommand::ToggleKeylock),
        f if f == S + S / 2 => send(h, DeckCommand::ToggleKeylock),
        _ => {}
    });
    let max_step = out
        .chunks_exact(CHANNELS)
        .zip(out.chunks_exact(CHANNELS).skip(1))
        .skip(1)
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.03, "max step {max_step}");
}

#[test]
fn keylock_survives_a_track_load() {
    let source = Source::from_interleaved(SR, &sine(S, 1000.0, 0.5));
    let (mut eng, mut h) = engine();
    let mut out = vec![0.0; BLOCK * CHANNELS];
    render(&mut eng, &mut h, &mut out, |_, h| {
        send(h, DeckCommand::SetKeylock(true));
        send(h, DeckCommand::Load(Arc::clone(&source)));
    });
    assert!(h.deck_state(0).keylock_enabled.load(Ordering::Relaxed));
    assert!(eng.deck(0).transport().is_keylock_enabled());
}
