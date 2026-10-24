//! The track on the deck, and what the player knows of its library record.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use engine_core::Source;
use library::TrackRef;

use super::cue_sync::CueSync;
use crate::analysis::Analysed;
use crate::waveform_live::LiveWaveform;

/// The track on the deck: its source and decode, waveform, analysis and library record.
pub struct LoadedTrack {
    pub info: engine_media::TrackInfo,
    pub source: Arc<Source>,
    pub(super) loader: Option<JoinHandle<Result<(), engine_media::LoadError>>>,
    /// Stops the decode and the analysis when the track is replaced.
    pub(super) cancel: engine_media::Cancel,
    pub(super) file: PathBuf,
    /// The load serial the library answers with.
    pub(super) serial: u64,
    /// Started once the library said what it has for the track.
    pub(super) analysis: Option<Receiver<Result<Analysed, String>>>,
    /// The analysis result, once it arrived.
    pub analysed: Option<Analysed>,
    pub(super) waveform: Arc<LiveWaveform>,
    pub(super) lib: LibState,
    /// The file is still arriving (a download): the library opens its record once it is
    /// complete, since its identity is its content.
    pub(super) arriving: bool,
}

impl Drop for LoadedTrack {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl LoadedTrack {
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The track's waveform, growing while the track decodes (or the stored one).
    pub fn waveform(&self) -> &Arc<LiveWaveform> {
        &self.waveform
    }

    /// The track in its library, once the library found or made its record.
    pub fn track(&self) -> Option<TrackRef> {
        self.lib.track
    }
}

/// What the player knows about the track's library record.
#[derive(Debug, Default)]
pub(super) struct LibState {
    pub(super) track: Option<TrackRef>,
    /// The deck's cues against the record.
    pub(super) cues: CueSync,
    /// The waveform is stored, or was sent to be stored.
    pub(super) waveform_stored: bool,
}
