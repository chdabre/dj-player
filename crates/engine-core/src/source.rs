//! `Source`: the resident sample store of one track.
//!
//! The whole track lives in RAM as interleaved stereo `f32` at the engine sample rate. It is
//! allocated once, up front, at its final (or estimated) size and filled chunk by chunk by a
//! loader thread through the single [`SourceWriter`]. Every chunk has a ready flag; the audio
//! callback checks the flags covering the range it is about to read and never waits.
//!
//! Samples are stored as `AtomicU32` bit patterns so the writer and the callback can share the
//! buffer without locks or `unsafe`. Relaxed atomic loads and stores of 32-bit values compile
//! to plain loads and stores on x86_64 and aarch64; the ready flag (release on write, acquire
//! on read) is what orders the data.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Source ids are unique for the life of the process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Number of interleaved channels in a `Source`. The loader maps every file to stereo.
pub const CHANNELS: usize = 2;

/// Default chunk length in frames (~0.34 s at 48 kHz).
const DEFAULT_CHUNK_FRAMES: usize = 16_384;

/// One stereo frame.
pub type Frame = [f32; CHANNELS];

/// Anything the scaler can read frames from by integer index.
///
/// Indices outside the stored range must return silence.
pub trait FrameSource {
    fn frame(&self, index: i64) -> Frame;
}

/// The resident, chunked sample store of one track.
pub struct Source {
    id: u64,
    sample_rate: u32,
    chunk_frames: usize,
    capacity_frames: usize,
    samples: Box<[AtomicU32]>,
    ready: Box<[AtomicBool]>,
    ready_count: AtomicUsize,
    len_frames: AtomicUsize,
    complete: AtomicBool,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("sample_rate", &self.sample_rate)
            .field("capacity_frames", &self.capacity_frames)
            .field("len_frames", &self.len_frames())
            .field("chunks_ready", &self.chunks_ready())
            .field("chunk_count", &self.chunk_count())
            .field("complete", &self.is_complete())
            .finish()
    }
}

impl Source {
    /// Allocate an empty source with room for `capacity_frames` stereo frames and return it
    /// together with its only writer. Call this off the audio thread: it allocates.
    pub fn new(sample_rate: u32, capacity_frames: usize) -> (Arc<Source>, SourceWriter) {
        Self::with_chunk_frames(sample_rate, capacity_frames, DEFAULT_CHUNK_FRAMES)
    }

    /// Like [`Source::new`] with an explicit chunk length (frames).
    pub fn with_chunk_frames(
        sample_rate: u32,
        capacity_frames: usize,
        chunk_frames: usize,
    ) -> (Arc<Source>, SourceWriter) {
        assert!(chunk_frames > 0, "chunk_frames must be > 0");
        let chunks = capacity_frames.div_ceil(chunk_frames);
        let samples = (0..capacity_frames * CHANNELS)
            .map(|_| AtomicU32::new(0))
            .collect();
        let ready = (0..chunks).map(|_| AtomicBool::new(false)).collect();
        let source = Arc::new(Source {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            sample_rate,
            chunk_frames,
            capacity_frames,
            samples,
            ready,
            ready_count: AtomicUsize::new(0),
            len_frames: AtomicUsize::new(capacity_frames),
            complete: AtomicBool::new(false),
        });
        let writer = SourceWriter {
            source: Arc::clone(&source),
            written: 0,
        };
        (source, writer)
    }

    /// Build a fully loaded source from interleaved stereo samples (tests, generated signals).
    pub fn from_interleaved(sample_rate: u32, interleaved: &[f32]) -> Arc<Source> {
        let frames = interleaved.len() / CHANNELS;
        let (source, mut writer) = Source::new(sample_rate, frames);
        writer.push(interleaved);
        writer.finish();
        source
    }

    /// Unique id of this source, so results computed for one track (e.g. its grid) are never
    /// applied to another.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    pub fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    /// Track length in frames: the estimate while loading, the exact length once complete.
    pub fn len_frames(&self) -> usize {
        self.len_frames.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len_frames() == 0
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub fn chunk_count(&self) -> usize {
        self.ready.len()
    }

    pub fn chunks_ready(&self) -> usize {
        self.ready_count.load(Ordering::Relaxed)
    }

    pub fn is_chunk_ready(&self, chunk: usize) -> bool {
        self.ready
            .get(chunk)
            .is_none_or(|flag| flag.load(Ordering::Acquire))
    }

    /// True when every frame in `[first, last]` (inclusive, may be negative or past the end)
    /// can be read. Frames outside the stored range are silence and always count as ready.
    /// Real-time safe: no allocation, no locks.
    pub fn range_ready(&self, first: i64, last: i64) -> bool {
        let (lo, hi) = if first <= last {
            (first, last)
        } else {
            (last, first)
        };
        if hi < 0 || lo >= self.capacity_frames as i64 {
            return true;
        }
        let lo = lo.max(0) as usize;
        let hi = (hi as usize).min(self.capacity_frames - 1);
        (lo / self.chunk_frames..=hi / self.chunk_frames).all(|c| self.is_chunk_ready(c))
    }
}

impl FrameSource for Source {
    #[inline]
    fn frame(&self, index: i64) -> Frame {
        if index < 0 || index as usize >= self.capacity_frames {
            return [0.0; CHANNELS];
        }
        let i = index as usize * CHANNELS;
        [
            f32::from_bits(self.samples[i].load(Ordering::Relaxed)),
            f32::from_bits(self.samples[i + 1].load(Ordering::Relaxed)),
        ]
    }
}

/// The single writer of a [`Source`], owned by the loader thread.
///
/// Frames are appended in order; each chunk is marked ready as soon as it is full.
#[derive(Debug)]
pub struct SourceWriter {
    source: Arc<Source>,
    written: usize,
}

impl SourceWriter {
    pub fn source(&self) -> &Arc<Source> {
        &self.source
    }

    /// Frames written so far.
    pub fn written_frames(&self) -> usize {
        self.written
    }

    /// Append interleaved stereo samples. Frames beyond the capacity are dropped.
    /// Returns the number of frames stored.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        let src = &*self.source;
        let frames = (interleaved.len() / CHANNELS).min(src.capacity_frames - self.written);
        let base = self.written * CHANNELS;
        for (slot, &s) in src.samples[base..base + frames * CHANNELS]
            .iter()
            .zip(interleaved)
        {
            slot.store(s.to_bits(), Ordering::Relaxed);
        }
        let first_chunk = self.written / src.chunk_frames;
        self.written += frames;
        let full_chunks = self.written / src.chunk_frames;
        for chunk in first_chunk..full_chunks {
            self.mark_ready(chunk);
        }
        frames
    }

    fn mark_ready(&self, chunk: usize) {
        let src = &*self.source;
        if let Some(flag) = src.ready.get(chunk)
            && !flag.swap(true, Ordering::AcqRel)
        {
            src.ready_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Finish loading: fix the exact length to what was written and mark every chunk ready
    /// (frames past the end stay silent).
    pub fn finish(self) -> Arc<Source> {
        let src = &*self.source;
        src.len_frames.store(self.written, Ordering::Release);
        for chunk in 0..src.ready.len() {
            self.mark_ready(chunk);
        }
        src.complete.store(true, Ordering::Release);
        self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_become_ready_as_they_fill() {
        let (src, mut w) = Source::with_chunk_frames(48_000, 10, 4);
        assert_eq!(src.chunk_count(), 3);
        assert!(!src.range_ready(0, 0));
        w.push(&[0.5; 2 * 3]);
        assert!(!src.is_chunk_ready(0));
        w.push(&[0.5; 2 * 2]);
        assert!(src.is_chunk_ready(0));
        assert!(!src.is_chunk_ready(1));
        assert!(src.range_ready(0, 3));
        assert!(!src.range_ready(3, 4));
        // Out of range is silence and therefore ready.
        assert!(src.range_ready(-10, -1));
        assert!(src.range_ready(10, 20));
        assert_eq!(src.frame(4), [0.5, 0.5]);
        assert_eq!(src.frame(-1), [0.0, 0.0]);
        let src = w.finish();
        assert!(src.is_complete());
        assert_eq!(src.len_frames(), 5);
        assert_eq!(src.chunks_ready(), 3);
        assert!(src.range_ready(0, 9));
    }

    #[test]
    fn push_beyond_capacity_is_truncated() {
        let (src, mut w) = Source::with_chunk_frames(44_100, 3, 2);
        assert_eq!(w.push(&[1.0; 2 * 5]), 3);
        let src2 = w.finish();
        assert!(Arc::ptr_eq(&src, &src2));
        assert_eq!(src.len_frames(), 3);
        assert_eq!(src.frame(3), [0.0, 0.0]);
    }
}
