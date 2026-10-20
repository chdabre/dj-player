//! Offline rendering helpers: generated test signals and a device-less render loop.
//!
//! Used by the offline test and the criterion bench so both drive [`Engine::process`] exactly
//! like an audio backend would, block by block.

use crate::engine::{Engine, EngineHandle};
use crate::source::CHANNELS;

/// A stereo test signal whose left channel encodes the frame index (`index / scale`) and whose
/// right channel is a sine. Reading the left channel back tells you exactly which source frame
/// (or fractional position, since the ramp is linear) the scaler read.
pub fn index_ramp(frames: usize, scale: f32, sine_hz: f32, sample_rate: u32) -> Vec<f32> {
    let w = std::f32::consts::TAU * sine_hz / sample_rate as f32;
    (0..frames)
        .flat_map(|i| [i as f32 / scale, 0.5 * (w * i as f32).sin()])
        .collect()
}

/// Render `frames` frames in blocks of `block` frames into `out` (interleaved, with the
/// engine's channel count), the way an audio device would. `schedule` runs before each block
/// with that block's first output frame, and may send commands through the handle; they take
/// effect at that block boundary.
pub fn render_offline(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    out: &mut [f32],
    block: usize,
    mut schedule: impl FnMut(usize, &mut EngineHandle),
) {
    assert!(block > 0, "block must be > 0");
    let ch = engine.config().channels;
    let frames = out.len() / ch;
    let mut done = 0;
    while done < frames {
        schedule(done, handle);
        let n = block.min(frames - done);
        engine.process(&mut out[done * ch..(done + n) * ch], n);
        handle.collect_retired();
        done += n;
    }
}

/// Left channel of an interleaved buffer with `channels` channels.
pub fn left(buf: &[f32], channels: usize) -> impl Iterator<Item = f32> + '_ {
    buf.chunks_exact(channels).map(|f| f[0])
}

/// Silence check for one stereo frame.
pub fn is_silent(frame: &[f32]) -> bool {
    frame[..CHANNELS.min(frame.len())].iter().all(|&s| s == 0.0)
}
