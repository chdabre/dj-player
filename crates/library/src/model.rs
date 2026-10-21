//! The data this version reads and writes, on top of the generic paths.
//!
//! | Path | Kind | Value |
//! | --- | --- | --- |
//! | `track/<id>/file` | set | [`FileIdentity`]: where the file is and what it is |
//! | `track/<id>/rating` | register | 0–5 |
//! | `track/<id>/cue` | set | [`Cue`]: memory cues and loops |
//! | `track/<id>/hotcue/<0..15>` | register | [`HotCue`] or null; two banks of 8 |
//! | `track/<id>/analysis/<analyzer>` | register | [`AnalysisRef`]: latest result, as a blob |
//! | `track/<id>/waveform` | register | [`WaveformRef`]: DJWF detail + overview blobs |
//! | `alias/<id>` | register | the track id this one was merged into |
//!
//! Positions are f64 seconds on the engine's gapless-trimmed timeline. Every value keeps
//! the fields this version does not know (`extra`), so editing a value written by a newer
//! device does not drop its additions.

use serde_json::{Map, Value, json};

use crate::hash::{AudioHash, FileHash};
use crate::ids::{BlobId, TrackId, unhex};

/// Hot cues per track: two banks of 8 (A–H).
pub const HOT_CUES: usize = 16;

pub fn track_prefix(id: TrackId) -> String {
    format!("track/{id}/")
}

pub fn file_path(id: TrackId) -> String {
    format!("track/{id}/file")
}

pub fn rating_path(id: TrackId) -> String {
    format!("track/{id}/rating")
}

pub fn cue_path(id: TrackId) -> String {
    format!("track/{id}/cue")
}

pub fn hot_cue_path(id: TrackId, slot: usize) -> String {
    format!("track/{id}/hotcue/{slot}")
}

pub fn analysis_path(id: TrackId, analyzer: &str) -> String {
    format!("track/{id}/analysis/{analyzer}")
}

pub fn waveform_path(id: TrackId) -> String {
    format!("track/{id}/waveform")
}

pub fn alias_path(id: TrackId) -> String {
    format!("alias/{id}")
}

/// A track id from the second component of a `track/<id>/…` or `alias/<id>` path.
pub fn track_of_path(path: &str) -> Option<TrackId> {
    let mut parts = path.split('/');
    match parts.next()? {
        "track" | "alias" => parts.next()?.parse().ok(),
        _ => None,
    }
}

/// A path component an analyzer name can be: ASCII letters, digits, `-`, `_`, `.`.
pub fn is_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn take_f64(m: &mut Map<String, Value>, k: &str) -> Option<f64> {
    m.remove(k)?.as_f64()
}

fn take_opt_f64(m: &mut Map<String, Value>, k: &str) -> Option<Option<f64>> {
    match m.remove(k) {
        None | Some(Value::Null) => Some(None),
        Some(v) => v.as_f64().map(Some),
    }
}

fn take_opt_string(m: &mut Map<String, Value>, k: &str) -> Option<String> {
    match m.remove(k)? {
        Value::String(s) => Some(s),
        _ => None,
    }
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
    pub audio_hash: Option<AudioHash>,
    pub extra: Map<String, Value>,
}

impl FileIdentity {
    pub fn new(path: impl Into<String>, size: u64, file_hash: FileHash) -> Self {
        Self {
            path: path.into(),
            size,
            file_hash,
            audio_hash: None,
            extra: Map::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("path".into(), self.path.clone().into());
        m.insert("size".into(), self.size.into());
        m.insert("file_hash".into(), self.file_hash.to_string().into());
        if let Some(a) = &self.audio_hash {
            m.insert(
                "audio_hash".into(),
                json!({ "v": a.v, "hash": a.hash_hex() }),
            );
        }
        Value::Object(m)
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let mut m = v.as_object()?.clone();
        let path = take_opt_string(&mut m, "path")?;
        let size = m.remove("size")?.as_u64()?;
        let file_hash = FileHash::parse(m.remove("file_hash")?.as_str()?)?;
        let audio_hash = match m.remove("audio_hash") {
            None | Some(Value::Null) => None,
            Some(a) => Some(AudioHash {
                v: u32::try_from(a.get("v")?.as_u64()?).ok()?,
                hash: unhex(a.get("hash")?.as_str()?)?,
            }),
        };
        Some(Self {
            path,
            size,
            file_hash,
            audio_hash,
            extra: m,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    Memory,
    Loop,
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
    pub extra: Map<String, Value>,
}

impl Cue {
    pub fn memory(at: f64) -> Self {
        Self {
            kind: CueKind::Memory,
            at,
            end: None,
            color: None,
            comment: String::new(),
            extra: Map::new(),
        }
    }

    pub fn memory_loop(at: f64, end: f64) -> Self {
        Self {
            kind: CueKind::Loop,
            end: Some(end),
            ..Self::memory(at)
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        let kind = match self.kind {
            CueKind::Memory => "memory",
            CueKind::Loop => "loop",
        };
        m.insert("kind".into(), kind.into());
        m.insert("at".into(), self.at.into());
        if let Some(e) = self.end {
            m.insert("end".into(), e.into());
        }
        m.insert("color".into(), self.color.clone().into());
        m.insert("comment".into(), self.comment.clone().into());
        Value::Object(m)
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let mut m = v.as_object()?.clone();
        let kind = match m.remove("kind")?.as_str()? {
            "memory" => CueKind::Memory,
            "loop" => CueKind::Loop,
            _ => return None,
        };
        let at = take_f64(&mut m, "at")?;
        let end = take_opt_f64(&mut m, "end")?;
        let color = take_opt_string(&mut m, "color");
        let comment = take_opt_string(&mut m, "comment").unwrap_or_default();
        Some(Self {
            kind,
            at,
            end,
            color,
            comment,
            extra: m,
        })
    }
}

/// A hot cue (or hot loop with `end`).
#[derive(Debug, Clone, PartialEq)]
pub struct HotCue {
    pub at: f64,
    pub end: Option<f64>,
    pub color: Option<String>,
    pub comment: String,
    pub extra: Map<String, Value>,
}

impl HotCue {
    pub fn at(at: f64) -> Self {
        Self {
            at,
            end: None,
            color: None,
            comment: String::new(),
            extra: Map::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("at".into(), self.at.into());
        if let Some(e) = self.end {
            m.insert("end".into(), e.into());
        }
        m.insert("color".into(), self.color.clone().into());
        m.insert("comment".into(), self.comment.clone().into());
        Value::Object(m)
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let mut m = v.as_object()?.clone();
        let at = take_f64(&mut m, "at")?;
        let end = take_opt_f64(&mut m, "end")?;
        let color = take_opt_string(&mut m, "color");
        let comment = take_opt_string(&mut m, "comment").unwrap_or_default();
        Some(Self {
            at,
            end,
            color,
            comment,
            extra: m,
        })
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

impl AnalysisRef {
    pub fn to_json(&self) -> Value {
        json!({
            "version": self.version,
            "params": self.params,
            "blob": self.blob.to_string(),
            "created": self.created,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            version: v.get("version")?.as_str()?.to_owned(),
            params: v.get("params")?.as_str()?.to_owned(),
            blob: v.get("blob")?.as_str()?.parse().ok()?,
            created: v.get("created")?.as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// A track's waveform: DJWF blobs, whose headers carry their parameters and timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveformRef {
    pub blob: BlobId,
    pub overview_blob: BlobId,
}

impl WaveformRef {
    pub fn to_json(&self) -> Value {
        json!({ "blob": self.blob.to_string(), "overview_blob": self.overview_blob.to_string() })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            blob: v.get("blob")?.as_str()?.parse().ok()?,
            overview_blob: v.get("overview_blob")?.as_str()?.parse().ok()?,
        })
    }
}

/// A track's merged record: its own fields and those of every track aliased into it.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: TrackId,
    /// Every known location, with the set element id of each. More than one file hash
    /// means tag-edited copies; several paths with one hash are exact duplicates.
    pub locations: Vec<(String, FileIdentity)>,
    pub rating: Option<u8>,
    /// Memory cues and loops, in time order, with their element ids.
    pub cues: Vec<(String, Cue)>,
    pub hot_cues: [Option<HotCue>; HOT_CUES],
    /// Latest result per analyzer name.
    pub analysis: std::collections::BTreeMap<String, AnalysisRef>,
    pub waveform: Option<WaveformRef>,
    /// Tracks merged into this one (their ids alias to it).
    pub merged: Vec<TrackId>,
}

impl Track {
    /// The current audio hash, if any location has one.
    pub fn audio_hash(&self) -> Option<AudioHash> {
        self.locations
            .iter()
            .filter_map(|(_, l)| l.audio_hash)
            .find(AudioHash::is_current)
    }

    /// True if more than one location holds the same file (exact duplicates).
    pub fn has_duplicates(&self) -> bool {
        let mut hashes: Vec<FileHash> = self.locations.iter().map(|(_, l)| l.file_hash).collect();
        hashes.sort();
        hashes.windows(2).any(|w| w[0] == w[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_keep_unknown_fields() {
        let mut c = Cue::memory_loop(1.5, 3.25);
        c.color = Some("rose".into());
        c.extra.insert("future".into(), json!([1, 2]));
        assert_eq!(Cue::from_json(&c.to_json()), Some(c));

        let mut h = HotCue::at(0.1 + 0.2);
        h.comment = "drop".into();
        assert_eq!(HotCue::from_json(&h.to_json()), Some(h));

        let mut f = FileIdentity::new("Music/a.mp3", 123, FileHash([7; 32]));
        f.audio_hash = Some(AudioHash {
            v: 1,
            hash: [9; 32],
        });
        assert_eq!(FileIdentity::from_json(&f.to_json()), Some(f));

        let w = WaveformRef {
            blob: BlobId([1; 32]),
            overview_blob: BlobId([2; 32]),
        };
        assert_eq!(WaveformRef::from_json(&w.to_json()), Some(w));
    }

    #[test]
    fn paths_name_their_track() {
        let id = TrackId([0x9c; 16]);
        assert_eq!(track_of_path(&hot_cue_path(id, 3)), Some(id));
        assert_eq!(track_of_path(&alias_path(id)), Some(id));
        assert_eq!(track_of_path("history"), None);
        assert!(is_component("tempocnn") && !is_component("a/b") && !is_component(".."));
    }
}
