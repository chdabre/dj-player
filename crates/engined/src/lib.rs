//! One deck playing one file through the real engine: device, decode, analysis, library and
//! stream, shared by the `engined` terminal CLI and the UI spike.
//!
//! [`Player::open_with`] picks the device format, starts decoding the file at that rate,
//! starts the stream and mounts the library on a worker thread ([`store`]). The waveform is
//! computed during the decode, from exactly the frames the deck plays, and grows in the
//! loaded track's [`LiveWaveform`]. The owner calls [`Player::poll`] regularly: it restores
//! the track's stored cues into the deck, uses the stored waveform and analysis when the
//! library has them (analysing otherwise, on a background thread), hands the grid to the
//! deck, writes new waveforms, analysis results and cue edits back, collects retired objects
//! off the audio thread and reports loader and stream errors.

mod analysis;
pub mod batch;
mod config;
pub mod controls;
mod player;
pub mod store;
mod waveform_live;

pub use analysis::Analysed;
pub use batch::BatchProgress;
pub use engine_io::{CpalBackend, StreamRequest};
pub use library::Persistence;
pub use player::{AutoCue, LoadedTrack, Options, Player};
pub use player_api::Port;
pub use store::LibraryChoice;
pub use waveform_live::LiveWaveform;
