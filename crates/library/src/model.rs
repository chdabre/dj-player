//! The values the library stores for a track. Positions are f64 seconds on the engine's
//! gapless-trimmed timeline.

use std::collections::BTreeMap;

use crate::hash::{AudioHash, FileHash};
use crate::ids::{BlobId, TrackId};

/// Hot cues per track: two banks of 8 (A–H).
pub const HOT_CUES: usize = 16;

/// A name an analyzer can have: ASCII letters, digits, `-`, `_`, `.`, `+`.
pub fn is_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
}

/// Where a file is and what it is: one location of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct FileIdentity {
    /// Relative to the library's root (the folder holding `DJPLAYER/`), `/`-separated and
    /// NFC-normalised (see [`crate::normalize_path`]). A hint for finding the file, never
    /// its identity.
    pub path: String,
    pub size: u64,
    pub file_hash: FileHash,
}

impl FileIdentity {
    pub fn new(path: impl Into<String>, size: u64, file_hash: FileHash) -> Self {
        Self {
            path: path.into(),
            size,
            file_hash,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    Memory,
    Loop,
}

impl CueKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CueKind::Memory => "memory",
            CueKind::Loop => "loop",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        if s == "loop" {
            CueKind::Loop
        } else {
            CueKind::Memory
        }
    }
}

/// A memory cue (`end: None`) or memory loop.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub kind: CueKind,
    /// Seconds.
    pub at: f64,
    /// Loop end, seconds.
    pub end: Option<f64>,
    pub color: Option<String>,
    pub comment: String,
}

impl Cue {
    pub fn memory(at: f64) -> Self {
        Self {
            kind: CueKind::Memory,
            at,
            end: None,
            color: None,
            comment: String::new(),
        }
    }

    pub fn memory_loop(at: f64, end: f64) -> Self {
        Self {
            kind: CueKind::Loop,
            end: Some(end),
            ..Self::memory(at)
        }
    }
}

/// A hot cue (or hot loop with `end`).
#[derive(Debug, Clone, PartialEq)]
pub struct HotCue {
    pub at: f64,
    pub end: Option<f64>,
    pub color: Option<String>,
    pub comment: String,
}

impl HotCue {
    pub fn at(at: f64) -> Self {
        Self {
            at,
            end: None,
            color: None,
            comment: String::new(),
        }
    }
}

/// The latest result of one analyzer for a track.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisRef {
    /// The analyzer's version and parameter fingerprint: a result is reused only if both
    /// match the analyzer that would run now.
    pub version: String,
    pub params: String,
    /// The result itself; its format is the analyzer's (versioned, small).
    pub blob: BlobId,
    /// RFC 3339 UTC.
    pub created: String,
}

/// A track's waveform: DJWF blobs, whose headers carry their parameters and timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformRef {
    pub blob: BlobId,
    pub overview_blob: BlobId,
}

impl WaveformRef {
    pub fn new(blob: BlobId, overview_blob: BlobId) -> Self {
        Self {
            blob,
            overview_blob,
        }
    }
}

/// A track's record, with what it took over from the tracks merged into it.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: TrackId,
    /// Every known location, oldest first. More than one file hash means tag-edited copies;
    /// several paths with one hash are exact duplicates.
    pub locations: Vec<FileIdentity>,
    /// The decoded audio's hash, once analysis has made one.
    pub audio_hash: Option<AudioHash>,
    pub rating: Option<u8>,
    /// Memory cues and loops in time order, with their ids.
    pub cues: Vec<(String, Cue)>,
    pub hot_cues: [Option<HotCue>; HOT_CUES],
    /// Latest result per analyzer name.
    pub analysis: BTreeMap<String, AnalysisRef>,
    pub waveform: Option<WaveformRef>,
    /// Tracks merged into this one (their ids resolve to it).
    pub merged: Vec<TrackId>,
}

impl Track {
    /// A track with nothing stored.
    pub fn empty(id: TrackId) -> Self {
        Self {
            id,
            locations: Vec::new(),
            audio_hash: None,
            rating: None,
            cues: Vec::new(),
            hot_cues: Default::default(),
            analysis: BTreeMap::new(),
            waveform: None,
            merged: Vec::new(),
        }
    }

    /// True if more than one location holds the same file (exact duplicates).
    pub fn has_duplicates(&self) -> bool {
        let mut hashes: Vec<FileHash> = self.locations.iter().map(|l| l.file_hash).collect();
        hashes.sort();
        hashes.windows(2).any(|w| w[0] == w[1])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn analyzer_names_are_plain() {
        assert!(super::is_component("tempocnn"));
        assert!(!super::is_component("a/b") && !super::is_component(".."));
    }
}
