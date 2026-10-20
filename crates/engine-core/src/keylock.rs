//! `Keylock`: the keylock path. Plays a transport [`Segment`] through a [`Stretcher`] so
//! the rate changes speed but not pitch.
//!
//! # Latency compensation
//!
//! A stretcher's output lags its input by `input_latency + output_latency * rate` input
//! frames (the *lookahead*). Because the whole track is resident, the keylock path simply
//! reads that far ahead of the transport. The audio that comes out at a given moment is then
//! the audio at [`crate::Transport::position`], so waveforms, cues and later sync need no
//! keylock offset.
//!
//! - **Jumps** (seek, slip snap-back, first block): [`Keylock::render`] primes the stretcher
//!   at the new position. It seeks with the audio before it, then runs and discards one
//!   output latency of audio, so the very next output frame is the new position.
//! - **Rate changes**: the lookahead depends on the rate. It is slewed towards the new value
//!   by at most [`LOOKAHEAD_SLEW`] of each block, so the audio catches up with the playhead
//!   over a fraction of a second instead of jumping.
//!
//! The path only runs forward at rates in `(0, MAX_STRETCH_RATE]`. Reverse, rate 0 and
//! anything faster fall back to the resampler ([`crate::Scaler`]), as the research doc says
//! for Signalsmith.

use engine_stretch::Stretcher;

use crate::deck::MAX_BLOCK_FRAMES;
use crate::source::{CHANNELS, FrameSource, Source};
use crate::transport::Segment;

/// Fastest rate the stretcher plays; above it the deck uses the resampler.
pub const MAX_STRETCH_RATE: f64 = 4.0;

/// Largest lookahead correction per block, as a fraction of the block length (1 % speed).
pub const LOOKAHEAD_SLEW: f64 = 0.01;

/// Playheads closer than this (frames) count as the same position.
const ALIGN_EPSILON: f64 = 1e-6;

pub struct Keylock {
    stretcher: Box<dyn Stretcher>,
    /// Transport position the stretcher's next output frame corresponds to, if primed.
    cursor: Option<f64>,
    /// One past the last source frame fed to the stretcher.
    fed_end: i64,
    /// `fed_end - cursor`: how far ahead of the playhead the input runs.
    lookahead: f64,
    input: Box<[f32]>,
    discard: Box<[f32]>,
}

impl std::fmt::Debug for Keylock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keylock")
            .field("cursor", &self.cursor)
            .field("fed_end", &self.fed_end)
            .field("lookahead", &self.lookahead)
            .finish_non_exhaustive()
    }
}

impl Keylock {
    /// Wrap a stereo stretcher and allocate the scratch space. Call off the audio thread.
    pub fn new(stretcher: Box<dyn Stretcher>) -> Self {
        assert_eq!(
            stretcher.channels(),
            CHANNELS,
            "keylock needs a stereo stretcher"
        );
        let per_block = (MAX_BLOCK_FRAMES as f64 * MAX_STRETCH_RATE * (1.0 + LOOKAHEAD_SLEW)).ceil()
            as usize
            + 2;
        let frames = stretcher.preroll_frames().max(per_block);
        Self {
            stretcher,
            cursor: None,
            fed_end: 0,
            lookahead: 0.0,
            input: vec![0.0; frames * CHANNELS].into_boxed_slice(),
            discard: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
        }
    }

    /// True if the keylock path plays a segment with this step (frames per output frame).
    pub fn handles(step: f64) -> bool {
        step > 0.0 && step <= MAX_STRETCH_RATE
    }

    /// True if the next output continues seamlessly from `position`, with no priming.
    pub fn is_aligned(&self, position: f64) -> bool {
        self.cursor
            .is_some_and(|c| (c - position).abs() < ALIGN_EPSILON)
    }

    /// Forget the stretcher's position (new track); the next render primes.
    pub fn invalidate(&mut self) {
        self.cursor = None;
    }

    /// Lookahead the stretcher needs at `rate` for its output to match the playhead.
    pub fn latency_at(&self, rate: f64) -> f64 {
        self.stretcher.input_latency() as f64 + self.stretcher.output_latency() as f64 * rate
    }

    /// Render `out.len() / 2` frames of `segment` (whose step must satisfy
    /// [`Keylock::handles`]), overwriting `out`. Primes first if the stretcher is not at
    /// `segment.start`.
    ///
    /// Real-time safe. Returns `false`, with `out` untouched and the stretcher unchanged, if a
    /// frame it needs is not loaded yet.
    pub fn render(&mut self, source: &Source, segment: Segment, out: &mut [f32]) -> bool {
        let frames = out.len() / CHANNELS;
        let rate = segment.step;
        debug_assert!(Self::handles(rate), "keylock cannot play step {rate}");
        if !self.is_aligned(segment.start) && !self.prime(source, segment.start, rate) {
            return false;
        }
        let slew = frames as f64 * LOOKAHEAD_SLEW;
        let lookahead =
            self.lookahead + (self.latency_at(rate) - self.lookahead).clamp(-slew, slew);
        let end = segment.start + rate * frames as f64;
        let capacity = (self.input.len() / CHANNELS) as i64;
        let n = ((end + lookahead).round() as i64 - self.fed_end).clamp(0, capacity);
        if n > 0 && !source.range_ready(self.fed_end, self.fed_end + n - 1) {
            return false;
        }
        let input = fill(&mut self.input, source, self.fed_end, n as usize);
        self.stretcher.process(input, out);
        self.fed_end += n;
        self.lookahead = self.fed_end as f64 - end;
        self.cursor = Some(end);
        true
    }

    /// Put the stretcher at `position` so its next output frame is that position.
    fn prime(&mut self, source: &Source, position: f64, rate: f64) -> bool {
        let preroll = self.stretcher.preroll_frames();
        let out_latency = self.stretcher.output_latency();
        let start = position.round() as i64 + self.stretcher.input_latency() as i64;
        let discard_in = (out_latency as f64 * rate).round() as i64;
        if !source.range_ready(start - preroll as i64, start + discard_in) {
            return false;
        }
        self.stretcher.reset();
        let input = fill(&mut self.input, source, start - preroll as i64, preroll);
        self.stretcher.seek(input, rate);

        // Run off the output latency, so what comes next is `position`.
        let mut fed = start;
        let mut done = 0;
        while done < out_latency {
            let n_out = (out_latency - done).min(MAX_BLOCK_FRAMES);
            let target = start + ((done + n_out) as f64 * rate).round() as i64;
            let input = fill(&mut self.input, source, fed, (target - fed) as usize);
            self.stretcher
                .process(input, &mut self.discard[..n_out * CHANNELS]);
            fed = target;
            done += n_out;
        }
        self.fed_end = fed;
        self.lookahead = fed as f64 - position;
        self.cursor = Some(position);
        true
    }
}

/// Copy `frames` source frames starting at `first` into `buf`; return the filled part.
fn fill<'a>(buf: &'a mut [f32], source: &Source, first: i64, frames: usize) -> &'a [f32] {
    let dst = &mut buf[..frames * CHANNELS];
    for (k, frame) in dst.chunks_exact_mut(CHANNELS).enumerate() {
        frame.copy_from_slice(&source.frame(first + k as i64));
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_stretch::SignalsmithStretcher;

    const SR: u32 = 48_000;

    fn keylock() -> Keylock {
        Keylock::new(Box::new(SignalsmithStretcher::new(CHANNELS, SR)))
    }

    #[test]
    fn handles_forward_rates_only() {
        assert!(Keylock::handles(1.0));
        assert!(Keylock::handles(0.01));
        assert!(Keylock::handles(MAX_STRETCH_RATE));
        assert!(!Keylock::handles(0.0));
        assert!(!Keylock::handles(-1.0));
        assert!(!Keylock::handles(MAX_STRETCH_RATE + 0.1));
    }

    #[test]
    fn primes_once_then_stays_aligned() {
        let src = Source::from_interleaved(SR, &vec![0.1; 10 * SR as usize * CHANNELS]);
        let mut k = keylock();
        let mut out = vec![0.0; 128 * CHANNELS];
        let seg = Segment {
            start: 1000.0,
            step: 1.08,
        };
        assert!(k.render(&src, seg, &mut out));
        assert!(k.is_aligned(1000.0 + 1.08 * 128.0));
        assert!((k.lookahead - k.latency_at(1.08)).abs() <= 1.0);
    }

    #[test]
    fn lookahead_follows_rate_changes_gradually() {
        let src = Source::from_interleaved(SR, &vec![0.1; 30 * SR as usize * CHANNELS]);
        let mut k = keylock();
        let mut out = vec![0.0; 128 * CHANNELS];
        let mut pos = 0.0;
        let mut play = |k: &mut Keylock, rate: f64| {
            assert!(k.render(
                &src,
                Segment {
                    start: pos,
                    step: rate
                },
                &mut out
            ));
            pos += rate * 128.0;
        };
        play(&mut k, 1.0);
        play(&mut k, 1.08);
        let jump = k.lookahead - k.latency_at(1.0);
        assert!(jump <= 128.0 * LOOKAHEAD_SLEW + 1.0, "{jump}");
        for _ in 0..400 {
            play(&mut k, 1.08);
        }
        assert!((k.lookahead - k.latency_at(1.08)).abs() <= 1.0);
    }

    #[test]
    fn unloaded_frames_starve_without_touching_state() {
        let (src, _writer) = Source::new(SR, SR as usize);
        let mut k = keylock();
        let mut out = vec![7.0; 128 * CHANNELS];
        let seg = Segment {
            start: 0.0,
            step: 1.0,
        };
        assert!(!k.render(&src, seg, &mut out));
        assert!(out.iter().all(|&s| s == 7.0));
        assert!(!k.is_aligned(0.0));
    }
}
