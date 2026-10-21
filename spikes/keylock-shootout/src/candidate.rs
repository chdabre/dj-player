//! The interface every keylock candidate implements.
//!
//! Candidates play a resident [`Track`] the way a deck would: start at a position (aligned, so
//! the first output frame *is* that position), then render 128-frame blocks at a rate that may
//! change between blocks. Each one feeds its stretcher in its own natural way.

use crate::track::Track;

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
