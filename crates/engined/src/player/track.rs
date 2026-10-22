//! The track on the deck, and what the player knows of its library record.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use engine_core::{HOT_CUES, Source};
use library::{HotCue, TrackRef};

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
    /// Serial of the last stored cues sent to the deck (0 = none yet).
    pub(super) serial: u64,
    /// Hot cues (frames) as the library gave them, and as last seen on the deck.
    pub(super) restored: [Option<f64>; HOT_CUES],
    pub(super) known: [Option<f64>; HOT_CUES],
    /// The library's hot cues per slot, so a deck edit (a new position) keeps the rest:
    /// end, colour, comment and fields this version doesn't know.
    pub(super) records: [Option<HotCue>; HOT_CUES],
    /// The waveform is stored, or was sent to be stored.
    pub(super) waveform_stored: bool,
    /// Memory cues in the record, once the library answered (0 if there is no record).
    pub(super) memory_cues: Option<usize>,
    /// The load-time cue point is decided (a memory cue, the first sound, or none).
    pub(super) cue_decided: bool,
}
