//! `Keylock`: the keylock path. Plays a transport [`Segment`] through a time-stretcher so
//! the rate changes speed but not pitch.
//!
//! The stretcher is the pure-Rust Bungee port in [`engine_stretch::bungee`], driven through
//! its grain API. It plays any non-zero velocity up to [`MAX_STRETCH_RATE`] in either
//! direction, so reverse keeps the pitch too; rate 0 and anything faster go to the resampler
//! ([`crate::Scaler`]). The audio that comes out at a given moment is the audio at
//! [`crate::Transport::position`], so waveforms, cues and later sync need no keylock offset.
//!
//! # Grains placed where the playhead will be
//!
//! Bungee is pull-based: each grain asks for an input position and returns one synthesis
//! hop of output, two grains later, tagged with the positions it covers. The path keeps an
//! output clock and places each grain at the position the transport will have when that
//! grain's output plays, assuming the current velocity holds. Input comes straight from the
//! resident [`Source`]. Output goes through a preallocated FIFO with a position per frame.
//!
//! - **Jumps** (seek, loop wrap, slip snap-back, first block): the segment does not continue
//!   the last one, so the path re-primes: one pre-roll grain before the new position, run
//!   until the first chunk at it, earlier output dropped. The next frame is exactly the new
//!   position. On a jump the deck does not declick itself (a loop wrap), the old audio
//!   still in the FIFO is crossfaded out over [`JUMP_FADE_FRAMES`]; commanded jumps get only
//!   the deck's declick, so the two fades never stack.
//! - **Rate changes** need no correction: grains already requested keep the old velocity
//!   (about 30 ms of output). The next grain is placed on the new trajectory, and Bungee works
//!   the hop out from the position difference, so the audio rejoins the playhead within one
//!   grain.

mod fifo;
mod grain_path;

use crate::source::Source;
use crate::transport::Segment;
use grain_path::GrainPath;

/// Fastest rate the stretcher plays; above it the deck uses the resampler.
const MAX_STRETCH_RATE: f64 = 4.0;

/// Crossfade from the old audio when the path re-primes at a jump the deck does not declick
/// itself (loop wraps). It carries over into the next block if the jump is near the end of
/// one.
const JUMP_FADE_FRAMES: usize = 64;

/// Playheads closer than this (frames) count as the same position.
const ALIGN_EPSILON: f64 = 1e-6;

/// If the audio at the FIFO head is further than this (frames) from the playhead, something
/// went wrong (it should never be more than a few thousand after a rate change): re-prime.
const MAX_DRIFT: f64 = 65_536.0;

/// The keylock path of one deck. See the [module docs](self).
pub(crate) struct Keylock {
    path: Box<GrainPath>,
}

impl std::fmt::Debug for Keylock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = &self.path;
        f.debug_struct("Keylock")
            .field("cursor", &g.cursor)
            .field("clock", &g.clock)
            .field("emit", &g.emit)
            .field("fifo", &g.fifo.len)
            .finish_non_exhaustive()
    }
}

impl Keylock {
    /// A stereo keylock path at `sample_rate`. Allocates; call off the audio thread.
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            path: Box::new(GrainPath::new(sample_rate)),
        }
    }

    /// True if the path plays a segment with this step (frames per output frame).
    pub(crate) fn handles(step: f64) -> bool {
        step != 0.0 && step.abs() <= MAX_STRETCH_RATE
    }

    /// True if the next output continues seamlessly from `position`, with no priming.
    pub(crate) fn is_aligned(&self, position: f64) -> bool {
        self.path
            .cursor
            .is_some_and(|c| (c - position).abs() < ALIGN_EPSILON)
    }

    /// Forget the stretcher's position (new track); the next render primes.
    pub(crate) fn invalidate(&mut self) {
        self.path.cursor = None;
    }

    /// True if every frame needed to render `parts` (frames, segment) in order is loaded,
    /// so a block split at loop wraps either renders whole or not at all. Real-time safe.
    pub(crate) fn ready(
        &self,
        source: &Source,
        parts: impl Iterator<Item = (usize, Segment)>,
    ) -> bool {
        self.path.ready(source, parts)
    }

    /// Render `out.len() / 2` frames of `segment` (whose step must satisfy
    /// [`Keylock::handles`]), overwriting `out`. Primes first if the stretcher is not at
    /// `segment.start`; then, if `fade_jump`, crossfades from the old audio over
    /// [`JUMP_FADE_FRAMES`] (pass `false` when the caller declicks the jump itself).
    ///
    /// Real-time safe. Returns `false`, with `out` untouched and the stretcher unchanged, if a
    /// frame it needs is not loaded yet.
    pub(crate) fn render(
        &mut self,
        source: &Source,
        segment: Segment,
        out: &mut [f32],
        fade_jump: bool,
    ) -> bool {
        debug_assert!(
            Self::handles(segment.step),
            "keylock cannot play step {}",
            segment.step
        );
        self.path.render(source, segment, out, fade_jump)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::CHANNELS;

    const SR: u32 = 48_000;

    fn sine_source(seconds: usize, hz: f64) -> std::sync::Arc<Source> {
        let w = std::f64::consts::TAU * hz / f64::from(SR);
        let frames: Vec<f32> = (0..seconds * SR as usize)
            .flat_map(|i| {
                let x = (0.5 * (w * i as f64).sin()) as f32;
                [x, x]
            })
            .collect();
        Source::from_interleaved(SR, &frames)
    }

    fn max_step(out: &[f32]) -> f32 {
        out.as_chunks::<CHANNELS>()
            .0
            .iter()
            .zip(out.as_chunks::<CHANNELS>().0.iter().skip(1))
            .map(|(a, b)| (b[0] - a[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_wrap_near_the_end_of_a_block_still_fades_over_the_whole_fade() {
        // 100 Hz at 0.5: natural steps up to ~0.0065. Jump to an arbitrary phase 4 frames
        // before the end of a block; the fade must continue into the next block.
        let src = sine_source(10, 100.0);
        let mut k = Keylock::new(SR);
        let mut out = vec![0.0; 40 * 128 * CHANNELS];
        let mut pos = 48_000.0;
        for (b, block) in out.chunks_mut(128 * CHANNELS).enumerate() {
            if b == 20 {
                let (head, tail) = block.split_at_mut(124 * CHANNELS);
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, head, true));
                // Half a cycle out of phase with the audio it replaces.
                pos = 50_684.0 + 48_000.0 + 240.0;
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, tail, true));
                pos += 4.0;
            } else {
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, block, true));
                pos += 128.0;
            }
        }
        let step = max_step(&out[10 * 128 * CHANNELS..]);
        assert!(step < 0.03, "max step {step}");
    }

    #[test]
    fn a_jump_without_fade_plays_the_new_position_exactly() {
        // When the caller declicks, the keylock must not mix in old audio of its own: its
        // output after the jump is what a freshly primed keylock plays there.
        let src = sine_source(10, 440.0);
        let mut played = Keylock::new(SR);
        let mut fresh = Keylock::new(SR);
        let mut out = vec![0.0; 128 * CHANNELS];
        for b in 0..30 {
            let seg = Segment {
                start: 48_000.0 + 128.0 * b as f64,
                step: 1.08,
            };
            assert!(played.render(&src, seg, &mut out, true));
        }
        let seg = Segment {
            start: 200_000.0,
            step: 1.08,
        };
        let mut a = vec![0.0; 128 * CHANNELS];
        let mut b = vec![0.0; 128 * CHANNELS];
        assert!(played.render(&src, seg, &mut a, false));
        assert!(fresh.render(&src, seg, &mut b, false));
        let diff = a
            .iter()
            .zip(&b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(diff < 1e-6, "differs from a fresh prime by {diff}");
    }

    #[test]
    fn ready_covers_every_part_of_a_split_block() {
        // Only the first second is loaded: a block whose second part wraps into unloaded
        // audio is not ready as a whole, and checking changes nothing.
        let (src, mut writer) = Source::with_chunk_frames(SR, 10 * SR as usize, 4096);
        writer.push(&vec![0.1; SR as usize * CHANNELS]);
        let k = Keylock::new(SR);
        let near = Segment {
            start: 20_000.0,
            step: 1.0,
        };
        let far = Segment {
            start: 300_000.0,
            step: 1.0,
        };
        assert!(k.ready(&src, [(64, near)].into_iter()));
        assert!(!k.ready(&src, [(64, near), (64, far)].into_iter()));
        assert!(!k.is_aligned(20_000.0));
    }

    #[test]
    fn handles_both_directions_up_to_the_limit() {
        assert!(Keylock::handles(1.0));
        assert!(Keylock::handles(0.01));
        assert!(Keylock::handles(-1.0));
        assert!(Keylock::handles(-MAX_STRETCH_RATE));
        assert!(Keylock::handles(MAX_STRETCH_RATE));
        assert!(!Keylock::handles(0.0));
        assert!(!Keylock::handles(MAX_STRETCH_RATE + 0.1));
        assert!(!Keylock::handles(-MAX_STRETCH_RATE - 0.1));
    }

    #[test]
    fn primes_once_and_its_output_is_the_playhead() {
        let src = Source::from_interleaved(SR, &vec![0.1; 10 * SR as usize * CHANNELS]);
        let mut k = Keylock::new(SR);
        let mut out = vec![0.0; 128 * CHANNELS];
        let mut pos = 20_000.0;
        for rate in [1.08, 1.08, -0.5, 2.0] {
            for _ in 0..50 {
                let seg = Segment {
                    start: pos,
                    step: rate,
                };
                assert!(k.render(&src, seg, &mut out, true));
                pos += rate * 128.0;
                assert!(k.is_aligned(pos));
            }
            // After the grains requested at the old rate are out, the next frame plays the
            // playhead exactly.
            let head = k.path.fifo.head_position().unwrap();
            assert!((head - pos).abs() < 1e-6, "rate {rate}: {head} vs {pos}");
        }
    }

    #[test]
    fn a_corrupted_fifo_head_re_primes_onto_the_playhead() {
        // The safety net: if the audio queued at the FIFO head is implausibly far from the
        // playhead (or NaN), the next block re-primes instead of playing it, and plays what a
        // fresh keylock plays there.
        let src = sine_source(10, 440.0);
        for bad in [f64::NAN, 1.0e9] {
            let mut k = Keylock::new(SR);
            let mut fresh = Keylock::new(SR);
            let mut out = vec![0.0; 128 * CHANNELS];
            let mut pos = 48_000.0;
            for _ in 0..20 {
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, &mut out, false));
                pos += 128.0;
            }
            assert!(k.is_aligned(pos));
            let head = k.path.fifo.head;
            k.path.fifo.position[head] = bad;
            let seg = Segment {
                start: pos,
                step: 1.0,
            };
            let (mut a, mut b) = (vec![0.0; 128 * CHANNELS], vec![0.0; 128 * CHANNELS]);
            assert!(k.render(&src, seg, &mut a, false));
            assert!(fresh.render(&src, seg, &mut b, false));
            let diff = a
                .iter()
                .zip(&b)
                .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
            assert!(
                diff < 1e-6,
                "head {bad}: differs from a fresh prime by {diff}"
            );
            let head = k.path.fifo.head_position().unwrap();
            assert!((head - (pos + 128.0)).abs() < 1e-6, "head {bad}: {head}");
        }
    }

    #[test]
    fn unloaded_frames_starve_without_touching_state() {
        let (src, _writer) = Source::new(SR, SR as usize);
        let mut k = Keylock::new(SR);
        let mut out = vec![7.0; 128 * CHANNELS];
        let seg = Segment {
            start: 0.0,
            step: 1.0,
        };
        assert!(!k.render(&src, seg, &mut out, true));
        assert!(out.iter().all(|&s| s == 7.0));
        assert!(!k.is_aligned(0.0));
    }
}
