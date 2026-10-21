//! The interface every keylock candidate implements, and the Signalsmith candidates.
//!
//! Candidates play a resident [`Track`] the way a deck would: start at a position (aligned, so
//! the first output frame *is* that position), then render 128-frame blocks at a rate that may
//! change between blocks. Each one feeds its stretcher in its own natural way.

use engine_stretch::{SignalsmithStretcher, Stretcher};

use crate::track::{CHANNELS, Track};

/// Largest block the harness renders.
pub const MAX_BLOCK: usize = 1024;

pub trait Candidate {
    fn name(&self) -> &'static str;

    /// True if negative rates play backwards with the pitch kept.
    fn reverse(&self) -> bool {
        false
    }

    /// Prepare to play from `position` (source frames) at `rate`, so that the next output frame
    /// is `position`.
    fn start(&mut self, track: &Track, position: f64, rate: f64);

    /// Render `out.len() / 2` frames at `rate`.
    fn render(&mut self, track: &Track, rate: f64, out: &mut [f32]);

    /// Input frames read beyond the ideal playhead (how far ahead the stretcher has to read).
    fn read_ahead(&self) -> f64;

    /// Output frames the candidate could not supply in time (rendered as silence).
    fn underruns(&self) -> u64 {
        0
    }
}

/// Signalsmith Stretch, fed the way `engine_core::Keylock` feeds it: primed with `seek` plus
/// one output latency run off, then exactly `rate` input frames per output frame (no lookahead
/// slew here, so the rate-response measurement sees the stretcher alone).
pub struct Signalsmith {
    name: &'static str,
    stretcher: SignalsmithStretcher,
    playhead: f64,
    feed: f64,
    fed_end: i64,
    input: Vec<f32>,
    discard: Vec<f32>,
}

impl Signalsmith {
    pub fn default_preset(sample_rate: u32) -> Self {
        Self::with(
            "signalsmith-default",
            SignalsmithStretcher::new(CHANNELS, sample_rate),
        )
    }

    pub fn cheaper_preset(sample_rate: u32) -> Self {
        Self::with(
            "signalsmith-cheaper",
            SignalsmithStretcher::cheaper(CHANNELS, sample_rate),
        )
    }

    fn with(name: &'static str, stretcher: SignalsmithStretcher) -> Self {
        // Room for the pre-roll or for one block at the fastest rate tested (4x).
        let frames = stretcher.preroll_frames().max(MAX_BLOCK * 4 + 2);
        Self {
            name,
            stretcher,
            playhead: 0.0,
            feed: 0.0,
            fed_end: 0,
            input: vec![0.0; frames * CHANNELS],
            discard: vec![0.0; MAX_BLOCK * CHANNELS],
        }
    }

    fn process(&mut self, track: &Track, frames_in: usize, out: &mut [f32]) {
        let input = &mut self.input[..frames_in * CHANNELS];
        track.copy_interleaved(self.fed_end, input);
        self.stretcher.process(input, out);
        self.fed_end += frames_in as i64;
    }
}

impl Candidate for Signalsmith {
    fn name(&self) -> &'static str {
        self.name
    }

    fn start(&mut self, track: &Track, position: f64, rate: f64) {
        let preroll = self.stretcher.preroll_frames();
        let out_latency = self.stretcher.output_latency();
        let start = position.round() as i64 + self.stretcher.input_latency() as i64;
        self.stretcher.reset();
        let input = &mut self.input[..preroll * CHANNELS];
        track.copy_interleaved(start - preroll as i64, input);
        self.stretcher.seek(input, rate);
        self.fed_end = start;
        let mut done = 0;
        while done < out_latency {
            let n_out = (out_latency - done).min(MAX_BLOCK);
            let target = start + ((done + n_out) as f64 * rate).round() as i64;
            let n_in = (target - self.fed_end) as usize;
            let mut discard = std::mem::take(&mut self.discard);
            self.process(track, n_in, &mut discard[..n_out * CHANNELS]);
            self.discard = discard;
            done += n_out;
        }
        self.playhead = position;
        self.feed = self.fed_end as f64;
    }

    fn render(&mut self, track: &Track, rate: f64, out: &mut [f32]) {
        let n_out = out.len() / CHANNELS;
        self.playhead += rate * n_out as f64;
        self.feed += rate * n_out as f64;
        let n_in = (self.feed.round() as i64 - self.fed_end).max(0) as usize;
        self.process(track, n_in, out);
    }

    fn read_ahead(&self) -> f64 {
        self.fed_end as f64 - self.playhead
    }
}
