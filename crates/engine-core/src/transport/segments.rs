//! The pieces one block reads: a single [`Segment`] normally, several when a loop wraps.

#[cfg(doc)]
use super::{MAX_RATE, MIN_LOOP_FRAMES};

/// Most pieces one block can be split into.
pub(crate) const MAX_SEGMENTS: usize = 16;

/// The pieces of one block: piece `i` starts at output frame `offset` and reads its
/// [`Segment`] until the next piece (or the end of the block).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segments {
    parts: [(usize, Segment); MAX_SEGMENTS],
    len: usize,
}

impl Segments {
    pub(super) fn new() -> Self {
        Self {
            parts: [(
                0,
                Segment {
                    start: 0.0,
                    step: 0.0,
                },
            ); MAX_SEGMENTS],
            len: 0,
        }
    }

    /// Add a piece. [`MAX_RATE`] and [`MIN_LOOP_FRAMES`] keep a block well under
    /// [`MAX_SEGMENTS`] pieces; a piece beyond that would be lost.
    pub(super) fn push(&mut self, offset: usize, segment: Segment) {
        debug_assert!(
            self.len < MAX_SEGMENTS,
            "more loop wraps in one block than allowed"
        );
        if self.len < MAX_SEGMENTS {
            self.parts[self.len] = (offset, segment);
            self.len += 1;
        }
    }

    /// The first piece (the whole block when there is no loop wrap).
    pub(crate) fn first(&self) -> Segment {
        self.parts[0].1
    }

    /// (offset, frames, segment) per piece, for a block of `frames` frames.
    pub(crate) fn iter(&self, frames: usize) -> impl Iterator<Item = (usize, usize, Segment)> + '_ {
        (0..self.len).map(move |i| {
            let (offset, seg) = self.parts[i];
            let end = if i + 1 < self.len {
                self.parts[i + 1].0
            } else {
                frames
            };
            (offset, end - offset, seg)
        })
    }

    /// Number of pieces: 1 without a loop wrap in the block.
    pub(crate) fn count(&self) -> usize {
        self.len
    }
}

/// One block of playhead movement: frame `k` of the block reads the source at
/// `start + k as f64 * step`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segment {
    pub start: f64,
    pub step: f64,
}
