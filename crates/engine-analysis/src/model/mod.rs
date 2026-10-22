//! What analysis produces, independent of who produced it.
//!
//! Times are seconds from the first decoded frame of the track (after the decoder's gapless
//! trimming), as `f64`. Seconds work for every source: Rekordbox stores milliseconds, files
//! come at 44.1 or 48 kHz, and the deck runs at the device rate. Each consumer converts to its
//! own frames when it loads the result.

mod feature;
mod grid;
mod key;
mod provenance;
mod tempo;

pub use feature::{Feature, FeatureSet};
pub use grid::{Beat, Beatgrid};
pub use key::{Key, Mode, PitchClass};
pub use provenance::{Analysed, Analysis, AnalyzerId, Provenance};
pub use tempo::{BpmRange, Settings, Tempo};

// `TrackAudio` lives with its loader in `audio`; still reachable as `model::TrackAudio`.
pub use crate::audio::TrackAudio;
