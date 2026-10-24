//! Track loading for the DJ player engine.
//!
//! [`load_file`] probes a file with Symphonia, allocates a resident [`Source`] at the device
//! sample rate, and starts a loader thread that decodes, maps to stereo, resamples once with
//! rubato (only if the file rate differs from the device rate) and fills the source chunk by
//! chunk. Playback can start as soon as the first chunk is ready; the callback never waits.
//!
//! A file can be loaded while it is still arriving (a download): see [`arriving`].
//!
//! [`Source`]: engine_core::Source

mod arriving;
mod artwork;
mod cancel;
mod decode;
mod error;
mod id3;
mod load;
mod resample;
mod tags;
#[cfg(test)]
mod testutil;

pub use arriving::{arrived, arriving, is_arriving};
pub use artwork::Artwork;
pub use cancel::Cancel;
pub use decode::{TrackInfo, decode_native, decode_native_until, decode_to_vec, probe};
pub use error::LoadError;
pub use load::{Loading, Tap, load_file, load_file_tapped};
pub use resample::resample;
pub use tags::{Tags, tags};
