//! `Scaler`: turns a transport [`Segment`] into output samples by reading the source at
//! fractional positions.
//!
//! This is the resampler path from the research doc, written by hand because it needs random
//! access in both directions: any rate works, including 0 (hold one position) and negative
//! (reverse). Interpolation is 4-point cubic Hermite (Catmull-Rom), which reproduces the
//! source exactly at integer positions and reproduces linear signals exactly in between. A
//! short windowed-sinc kernel for normal play above 1.0x is a later step.

use crate::source::{CHANNELS, Frame, FrameSource};
use crate::transport::Segment;

#[derive(Debug, Default, Clone)]
pub struct Scaler;

impl Scaler {
    pub fn new() -> Self {
        Self
    }

    /// Inclusive range of source frame indices that rendering `frames` output frames of
    /// `segment` will read, including the interpolation neighbours.
    pub fn span(segment: Segment, frames: usize) -> (i64, i64) {
        if frames == 0 {
            let i = segment.start.floor() as i64;
            return (i, i);
        }
        let end = segment.start + (frames - 1) as f64 * segment.step;
        let (lo, hi) = if end < segment.start {
            (end, segment.start)
        } else {
            (segment.start, end)
        };
        (lo.floor() as i64 - 1, hi.floor() as i64 + 2)
    }

    /// Render `out.len() / 2` interleaved stereo frames of `segment` from `source`, overwriting
    /// `out`. Real-time safe: no allocation, no locks.
    pub fn render<S: FrameSource + ?Sized>(
        &mut self,
        source: &S,
        segment: Segment,
        out: &mut [f32],
    ) {
        for (k, frame) in out.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            let pos = segment.start + k as f64 * segment.step;
            frame.copy_from_slice(&Self::sample_at(source, pos));
        }
    }

    /// The interpolated frame at fractional source position `pos`.
    #[inline]
    pub fn sample_at<S: FrameSource + ?Sized>(source: &S, pos: f64) -> Frame {
        let floor = pos.floor();
        let i = floor as i64;
        let t = (pos - floor) as f32;
        if t == 0.0 {
            return source.frame(i);
        }
        let ym1 = source.frame(i - 1);
        let y0 = source.frame(i);
        let y1 = source.frame(i + 1);
        let y2 = source.frame(i + 2);
        let mut out = [0.0; CHANNELS];
        for c in 0..CHANNELS {
            out[c] = hermite(t, ym1[c], y0[c], y1[c], y2[c]);
        }
        out
    }
}

/// 4-point, 3rd-order Hermite (Catmull-Rom) interpolation between `y0` and `y1`, `t` in [0, 1).
#[inline]
pub fn hermite(t: f32, ym1: f32, y0: f32, y1: f32, y2: f32) -> f32 {
    let c1 = 0.5 * (y1 - ym1);
    let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
    let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
    ((c3 * t + c2) * t + c1) * t + y0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SliceSource;

    /// Stereo ramp: left = index, right = -index.
    fn ramp(frames: usize) -> Vec<f32> {
        (0..frames).flat_map(|i| [i as f32, -(i as f32)]).collect()
    }

    fn render(src: &[f32], start: f64, step: f64, frames: usize) -> Vec<f32> {
        let mut out = vec![f32::NAN; frames * CHANNELS];
        Scaler::new().render(&SliceSource(src), Segment { start, step }, &mut out);
        out
    }

    #[test]
    fn unity_rate_at_integer_start_is_an_exact_copy() {
        let src: Vec<f32> = (0..64).map(|i| ((i * 7919) % 101) as f32 / 101.0).collect();
        let out = render(&src, 3.0, 1.0, 20);
        assert_eq!(&out[..], &src[6..46]);
    }

    #[test]
    fn interpolates_linear_signals_exactly() {
        let src = ramp(100);
        let out = render(&src, 10.25, 0.5, 8);
        for k in 0..8 {
            let expected = 10.25 + 0.5 * k as f32;
            assert!(
                (out[2 * k] - expected).abs() < 1e-4,
                "k={k}: {}",
                out[2 * k]
            );
            assert!((out[2 * k + 1] + expected).abs() < 1e-4);
        }
    }

    #[test]
    fn reverse_reads_backwards() {
        let src = ramp(100);
        let out = render(&src, 50.0, -1.0, 10);
        for k in 0..10 {
            assert_eq!(out[2 * k], 50.0 - k as f32);
        }
        let out = render(&src, 50.5, -1.5, 10);
        for k in 0..10 {
            let expected = 50.5 - 1.5 * k as f32;
            assert!((out[2 * k] - expected).abs() < 1e-4);
        }
    }

    #[test]
    fn rate_zero_holds_one_value() {
        let src = ramp(100);
        let out = render(&src, 33.75, 0.0, 16);
        for k in 0..16 {
            assert!((out[2 * k] - 33.75).abs() < 1e-4);
            assert_eq!(out[2 * k], out[0]);
        }
    }

    #[test]
    fn reads_outside_the_source_are_silent() {
        let src = vec![1.0; 2 * 10];
        let out = render(&src, -5.0, 1.0, 3);
        assert!(out.iter().all(|&s| s == 0.0));
        let out = render(&src, 20.0, 1.0, 3);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn hermite_hits_the_knots() {
        assert_eq!(hermite(0.0, 3.0, 5.0, -2.0, 8.0), 5.0);
        assert!((hermite(0.999_999, 3.0, 5.0, -2.0, 8.0) + 2.0).abs() < 1e-4);
    }

    #[test]
    fn span_covers_neighbours_in_both_directions() {
        let fwd = Scaler::span(
            Segment {
                start: 10.5,
                step: 1.0,
            },
            4,
        );
        assert_eq!(fwd, (9, 15));
        let rev = Scaler::span(
            Segment {
                start: 10.0,
                step: -1.0,
            },
            4,
        );
        assert_eq!(rev, (6, 12));
        let hold = Scaler::span(
            Segment {
                start: 10.0,
                step: 0.0,
            },
            4,
        );
        assert_eq!(hold, (9, 12));
    }
}
