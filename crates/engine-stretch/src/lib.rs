//! Time-stretchers for keylock: change speed without changing pitch.
//!
//! Two kinds of backend. [`bungee`] is a pure-Rust port of Bungee Basic with its own pull
//! (grain) API; it is the decks' default keylock (see `docs/spikes/keylock-shootout.md`).
//! Push backends sit behind [`Stretcher`]: so far Signalsmith Stretch through its C++
//! wrapper. See `docs/research/engine.md` ("Rate, scratching and keylock").
//!
//! Construct stretchers off the audio thread: construction allocates. After that, Bungee's
//! grain calls and [`Stretcher::seek`], [`Stretcher::process`] and [`Stretcher::reset`] must
//! be real-time safe. Note that `assert_no_alloc` only sees Rust's allocator; allocations
//! inside a C++ backend go straight to `malloc` and are invisible to it.

pub mod bungee;

#[cfg(feature = "signalsmith")]
mod signalsmith;

#[cfg(feature = "signalsmith")]
pub use signalsmith::SignalsmithStretcher;

/// A streaming time-stretcher over interleaved frames with a fixed channel count.
///
/// The playback rate is implied by each [`Stretcher::process`] call: `input` frames in,
/// `output` frames out. Pitch is unchanged.
///
/// # Latency
///
/// The audio leaving `process` lags the input fed so far by
/// `input_latency() + output_latency() * rate` input frames. A caller that wants the output
/// to line up with a playhead feeds input that far ahead of it.
pub trait Stretcher: Send {
    fn channels(&self) -> usize;

    /// Input frames the stretcher reads ahead of its processing time.
    fn input_latency(&self) -> usize;

    /// Output frames between the processing time and the audio `process` returns.
    fn output_latency(&self) -> usize;

    /// How many input frames [`Stretcher::seek`] wants to fully prime the analysis.
    fn preroll_frames(&self) -> usize;

    /// Clear all internal state.
    fn reset(&mut self);

    /// Jump to a new place in the input: `input` is the audio just *before* the new input
    /// position (ideally [`Stretcher::preroll_frames`] long). After this the processing time
    /// is `input_latency()` frames before the end of `input`; `rate` is a hint for the first
    /// output.
    fn seek(&mut self, input: &[f32], rate: f64);

    /// Consume `input` and fill `output` (both interleaved, whole frames).
    fn process(&mut self, input: &[f32], output: &mut [f32]);
}

/// The push-API stretcher for `engine_core::Keylock::new` (Signalsmith), or `None` if no push
/// backend is compiled in. Decks default to the [`bungee`] port, which is always present.
pub fn default_stretcher(channels: usize, sample_rate: u32) -> Option<Box<dyn Stretcher>> {
    #[cfg(feature = "signalsmith")]
    {
        Some(Box::new(SignalsmithStretcher::new(channels, sample_rate)))
    }
    #[cfg(not(feature = "signalsmith"))]
    {
        let _ = (channels, sample_rate);
        None
    }
}
