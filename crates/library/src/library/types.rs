//! What opening, reading and writing a library take and give back.

use std::io;

use crate::db::{self, DbError};
use crate::ids::{DeviceId, EntryId, PlaylistId, TrackId};
use crate::time::system_ms;

/// How to open a library.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// This device: written next to each edit, for a later sync layer.
    pub device: DeviceId,
    /// Never write to the folder: the file is read into RAM, and edits stay there, shown
    /// as unsaved (a write-protected stick).
    pub read_only: bool,
    /// Wall-clock milliseconds since the Unix epoch; 0 if the device has no clock.
    pub now: fn() -> u64,
}

impl MountOptions {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            read_only: false,
            now: system_ms,
        }
    }
}

#[derive(Debug)]
pub enum MountError {
    Io(io::Error),
    /// The file can't be opened or read (locked by another process, unreadable).
    Db(String),
    /// Written by a newer version that this one can't read.
    Newer(i64),
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::Io(e) => write!(f, "library: {e}"),
            MountError::Db(e) => write!(f, "library: {e}"),
            MountError::Newer(v) => write!(
                f,
                "library needs player version {v} or later (this is {})",
                db::SCHEMA_VERSION
            ),
        }
    }
}

impl std::error::Error for MountError {}

impl From<io::Error> for MountError {
    fn from(e: io::Error) -> Self {
        MountError::Io(e)
    }
}

impl From<DbError> for MountError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::Newer { min_reader } => MountError::Newer(min_reader),
            e => MountError::Db(e.to_string()),
        }
    }
}

/// A read that failed (the stick is gone or damaged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError(pub String);

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "can't read the library: {}", self.0)
    }
}

impl std::error::Error for ReadError {}

impl From<rusqlite::Error> for ReadError {
    fn from(e: rusqlite::Error) -> Self {
        ReadError(e.to_string())
    }
}

/// An edit that can't be made.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteError {
    NoSuchTrack(TrackId),
    BadHotCueSlot(usize),
    BadAnalyzerName(String),
    NoSuchCue(String),
    NoSuchPlaylist(PlaylistId),
    NoSuchEntry(EntryId),
    /// A playlist edit that can't be made (why).
    BadPlaylistEdit(&'static str),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::NoSuchTrack(t) => write!(f, "no track {t}"),
            WriteError::BadHotCueSlot(s) => write!(f, "hot cue slot {s} out of range"),
            WriteError::BadAnalyzerName(n) => write!(f, "bad analyzer name {n:?}"),
            WriteError::NoSuchCue(c) => write!(f, "no cue {c}"),
            WriteError::NoSuchPlaylist(p) => write!(f, "no playlist {p}"),
            WriteError::NoSuchEntry(e) => write!(f, "no playlist entry {e}"),
            WriteError::BadPlaylistEdit(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for WriteError {}

/// Whether edits reach the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persistence {
    /// Everything is committed.
    Saved,
    /// Edits are waiting for the next commit.
    Pending,
    /// Edits are kept in RAM only (no library, a read-only or failing stick) and lost at
    /// power-off.
    Unsaved { edits: usize, reason: String },
}

/// What opening found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountStats {
    pub tracks: usize,
    /// There was no library file: an empty one was made.
    pub created: bool,
    /// Edits left unsaved by an earlier mount of this library, written now.
    pub replayed: usize,
    /// The last session did not close the file cleanly: [`Library::check`] should run.
    ///
    /// [`Library::check`]: super::Library::check
    pub needs_check: bool,
}

/// What [`Library::find_or_create`] did.
///
/// [`Library::find_or_create`]: super::Library::find_or_create
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub track: TrackId,
    /// No record existed: a new one was made.
    pub created: bool,
    /// The record existed, but not at this path: a new location (an exact duplicate or a
    /// moved file) was added.
    pub new_location: bool,
}
