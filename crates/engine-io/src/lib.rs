//! Audio output for the DJ player engine.
//!
//! [`AudioBackend`] is the thin seam between the engine and the OS audio stack: it reports the
//! device format (so tracks can be resampled to it at load time) and runs a real-time render
//! callback. All device, host, thread-priority and core-pinning specifics stay behind it.
//! [`CpalBackend`] implements it with cpal: CoreAudio on macOS, ALSA on Linux. A hand-written
//! ALSA mmap backend can replace it later without touching the engine.
//!
//! The callback always sees interleaved `f32`; if the device wants another sample format the
//! backend converts through a preallocated scratch buffer, so the callback path never
//! allocates.

use std::fmt;

/// The device format a stream runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: usize,
}

/// What to open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamRequest {
    /// Case-insensitive substring of the output device id or name; `None` = default device.
    pub device: Option<String>,
    /// Requested buffer size in frames; `None` = the device default.
    pub buffer_frames: Option<u32>,
}

/// Error reported by a backend.
#[derive(Debug)]
pub struct BackendError(String);

impl BackendError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

/// The real-time render function: fill `out` with `frames` interleaved frames of
/// [`StreamFormat::channels`] channels. Must not allocate, lock, log or block.
pub trait RenderCallback: Send + 'static {
    fn render(&mut self, out: &mut [f32], frames: usize);
}

impl<F: FnMut(&mut [f32], usize) + Send + 'static> RenderCallback for F {
    fn render(&mut self, out: &mut [f32], frames: usize) {
        self(out, frames)
    }
}

/// An audio output backend.
pub trait AudioBackend {
    type Stream: AudioStream;

    /// The format the stream for `request` will run at. Call before loading tracks, so they are
    /// resampled to the device rate.
    fn format(&self, request: &StreamRequest) -> Result<StreamFormat, BackendError>;

    /// Open and start an output stream that calls `callback` from the audio thread.
    fn start(
        &self,
        request: &StreamRequest,
        callback: Box<dyn RenderCallback>,
    ) -> Result<Self::Stream, BackendError>;
}

/// A running stream. Dropping it stops playback.
pub trait AudioStream {
    fn format(&self) -> StreamFormat;
    /// Buffer size in frames, if the backend knows it.
    fn buffer_frames(&self) -> Option<u32>;
    /// Device or stream errors since the last call (xruns, device lost), oldest first.
    fn take_errors(&self) -> Vec<String>;
}

mod cpal_backend;
pub use cpal_backend::{CpalBackend, CpalStream};
