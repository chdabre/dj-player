//! Track waveforms, from decoded audio to pixels (`docs/spikes/waveforms.md`).
//!
//! - [`compute`]: three bands of peak amplitude (low < 250 Hz, mid, high > 2.5 kHz), 150
//!   entries per second on a seconds time base, one `u8` per band on a square-root curve,
//!   plus an overview reduced from that detail.
//!
//! The crate has no dependencies: the UI and the GPU renderer can use it without pulling in
//! decoding or analysis.

pub mod compute;

pub use compute::{
    Entry, OVERVIEW_COLUMNS, Overview, Params, Provenance, Waveform, compute, encode_level, level,
    overview,
};
