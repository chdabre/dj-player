//! Track waveforms, from decoded audio to pixels (`docs/spikes/waveforms.md`).
//!
//! - [`compute`]: three bands of peak amplitude (low < 250 Hz, mid, high > 2.5 kHz), 150
//!   entries per second on a seconds time base, one `u8` per band on a square-root curve,
//!   plus an overview reduced from that detail.
//! - [`display`]: the three display modes (single colour, 3-band, RGB) as pure functions
//!   from band values to drawn segments and colours, plus played-part dimming.
//! - [`blob`]: the versioned little-endian encoding of a detail or overview blob.
//! - [`render`]: the CPU reference renderer for the 20-beat zoom and the overview, the
//!   golden reference for the GPU shader. `waveform-preview` (feature `cli`) writes PNGs.
//!
//! The crate has no dependencies: the UI and the GPU renderer can use it without pulling in
//! decoding or analysis.

pub mod blob;
pub mod compute;
pub mod display;
pub mod render;

pub use blob::DecodeError;
pub use compute::{
    Builder, Entry, OVERVIEW_COLUMNS, Origin, Overview, Params, Provenance, Timeline, Waveform,
    compute, encode_level, level, overview,
};
pub use display::{Column, Layout, Mode, Rgb, Segment, Style, Theme, TrackGain, auto_gain};
