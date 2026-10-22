//! The linear crossfade used for every declick: a jump, a loop wrap, a change of path.

use crate::source::CHANNELS;

/// Crossfade `out` in from `old` (both interleaved stereo), over as many frames as both hold.
///
/// The fade is `total` frames long and `done` of them were applied before (by an earlier
/// block): frame `k` of `out` gets the new audio's gain `(done + k + 1) / total`, so the last
/// frame of the fade is all new audio. Real-time safe.
pub(crate) fn crossfade(out: &mut [f32], old: &[f32], done: usize, total: usize) {
    let inv = 1.0 / total as f32;
    for (k, (o, s)) in out
        .as_chunks_mut::<CHANNELS>()
        .0
        .iter_mut()
        .zip(old.as_chunks::<CHANNELS>().0.iter())
        .enumerate()
    {
        let g = (done + k + 1) as f32 * inv;
        for c in 0..CHANNELS {
            o[c] = s[c] + (o[c] - s[c]) * g;
        }
    }
}
