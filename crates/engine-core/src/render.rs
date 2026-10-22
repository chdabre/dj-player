//! Test and bench helpers (behind the `testing` feature): generated test signals.
//!
//! Used by the offline tests and the criterion bench, which drive [`crate::Engine::process`]
//! exactly like an audio backend would, block by block.

/// A stereo test signal whose left channel encodes the frame index (`index / scale`) and whose
/// right channel is a sine. Reading the left channel back tells you exactly which source frame
/// (or fractional position, since the ramp is linear) the scaler read.
pub fn index_ramp(frames: usize, scale: f32, sine_hz: f32, sample_rate: u32) -> Vec<f32> {
    let w = std::f32::consts::TAU * sine_hz / sample_rate as f32;
    (0..frames)
        .flat_map(|i| [i as f32 / scale, 0.5 * (w * i as f32).sin()])
        .collect()
}
