//! One open library: the SQLite file, the queue of edits not yet committed, and the blobs.
//!
//! The library worker is the only writer. An edit runs against the file at once, inside a
//! write transaction that [`Library::flush`] commits (the player calls it about once a
//! second), and stays in the queue until that commit. If a write fails (the stick was
//! pulled, is full or went read-only) the transaction is rolled back, the queue is kept and
//! the library says [`Persistence::Unsaved`]. When the library is dropped with edits still
//! queued, they are kept in this process and replayed when the same library (by its id) is
//! opened again.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::{self, BACKUP_FILE, DB_FILE, DbError};
use crate::hash::{AudioHash, FileHash, track_id};
use crate::ids::{BlobId, DeviceId, LibraryId, TrackId, TrackRef, hex, unhex};
use crate::model::{
    AnalysisRef, Cue, CueKind, FileIdentity, HOT_CUES, HotCue, Listed, Track, TrackTags,
    WaveformRef, is_component,
};

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

/// Milliseconds since the Unix epoch from the system clock (0 if it is before 1970).
pub fn system_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::NoSuchTrack(t) => write!(f, "no track {t}"),
            WriteError::BadHotCueSlot(s) => write!(f, "hot cue slot {s} out of range"),
            WriteError::BadAnalyzerName(n) => write!(f, "bad analyzer name {n:?}"),
            WriteError::NoSuchCue(c) => write!(f, "no cue {c}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// An edit to one track of this library.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    SetRating(TrackId, Option<u8>),
    /// A new memory cue or loop.
    AddCue(TrackId, Cue),
    /// Change the cue with this id; it keeps its id.
    ReplaceCue(TrackId, String, Cue),
    RemoveCue(TrackId, String),
    /// Hot cue slot 0–15 (two banks of 8); `None` deletes it.
    SetHotCue(TrackId, usize, Option<HotCue>),
    SetAnalysis(TrackId, String, AnalysisRef),
    SetWaveform(TrackId, WaveformRef),
    /// The tag cache: every field as given.
    SetTags(TrackId, TrackTags),
}

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
    pub needs_check: bool,
}

/// What [`Library::find_or_create`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub track: TrackId,
    /// No record existed: a new one was made.
    pub created: bool,
    /// The record existed, but not at this path: a new location (an exact duplicate or a
    /// moved file) was added.
    pub new_location: bool,
}

/// One row-level write: what the queue holds. Each is idempotent, so replaying a queue that
/// was partly written already is harmless.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    Locate(TrackId, FileIdentity),
    AudioHash(TrackId, AudioHash),
    /// Move everything of `from` to `into` and make `from` an alias of it.
    Merge {
        from: TrackId,
        into: TrackId,
    },
    Rating(TrackId, Option<u8>),
    PutCue(TrackId, [u8; 16], Cue),
    RemoveCue([u8; 16]),
    HotCue(TrackId, usize, Option<HotCue>),
    Analysis(TrackId, String, AnalysisRef),
    Waveform(TrackId, WaveformRef),
    Tags(TrackId, TrackTags),
}

/// Edits and blobs a dropped library could not write, by library.
#[derive(Default)]
struct Parked {
    ops: Vec<Op>,
    blobs: HashMap<BlobId, Vec<u8>>,
}

static PARKED: Mutex<BTreeMap<LibraryId, Parked>> = Mutex::new(BTreeMap::new());

/// The library id of [`Library::ram`]: edits made without a library carry over between
/// loads in one process.
const RAM_LIBRARY: LibraryId = LibraryId::nil();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The file on the stick, written through a transaction.
    Folder,
    /// A write to the folder failed; the queue is in RAM only.
    Failed,
    /// An in-memory database (no library, or a read-only stick's copy).
    Ram,
}

pub struct Library {
    conn: Connection,
    /// The `DJPLAYER/` folder, unless this library lives in RAM only.
    dir: Option<PathBuf>,
    id: LibraryId,
    device: [u8; 16],
    now: fn() -> u64,
    mode: Mode,
    /// Why edits are not saved; `None` while they reach the folder.
    unsaved: Option<String>,
    /// Ops not yet committed (or never, in RAM), in order.
    queue: Vec<Op>,
    /// Every op this mount wrote, to replay onto a backup if the check finds damage.
    session: Vec<Op>,
    /// A write transaction is open on `conn`.
    in_tx: bool,
    /// The file is marked in use (`clean_unmount = 0`) and backed up.
    started: bool,
    /// The last session didn't close the file cleanly.
    unclean: bool,
    ram_blobs: HashMap<BlobId, Vec<u8>>,
    warnings: Vec<String>,
    stats: MountStats,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("id", &self.id)
            .field("dir", &self.dir)
            .field("mode", &self.mode)
            .field("queue", &self.queue.len())
            .finish_non_exhaustive()
    }
}

impl Library {
    /// Open the library in `dir` (the `DJPLAYER/` folder), making the folder and an empty
    /// library if there is none. A damaged file is set aside as `library.db.corrupt-<date>`
    /// and the backup used instead (or an empty library, if there is no backup).
    pub fn mount(dir: &Path, opts: MountOptions) -> Result<Self, MountError> {
        let path = dir.join(DB_FILE);
        let mut warnings = Vec::new();
        if opts.read_only {
            let conn = if path.exists() {
                db::open_copy(&path)?
            } else {
                db::open_memory()?
            };
            let created = !path.exists();
            // Blobs are still read from the folder.
            let mut lib = Self::with(conn, Some(dir.to_path_buf()), opts, Mode::Ram, warnings)?;
            lib.stats.created = created;
            lib.unsaved = Some(format!("{} is read-only", dir.display()));
            return Ok(lib);
        }
        std::fs::create_dir_all(dir.join("blobs"))?;
        let existed = path.exists();
        let open = |path: &Path| -> Result<Connection, DbError> {
            let c = db::open_rw(path)?;
            db::prepare(&c, LibraryId::random().bytes())?;
            Ok(c)
        };
        let conn = match open(&path) {
            Ok(c) => c,
            Err(e) if e.is_damage() => {
                warnings.push(set_aside(dir, &e.to_string())?);
                open(&path)?
            }
            Err(e) => return Err(e.into()),
        };
        let mut lib = Self::with(conn, Some(dir.to_path_buf()), opts, Mode::Folder, warnings)?;
        lib.stats.created = !existed;
        Ok(lib)
    }

    /// A library in RAM only: nothing is saved. Edits made in it carry over to the next
    /// [`Library::ram`] in this process.
    pub fn ram(opts: MountOptions, reason: impl Into<String>) -> Self {
        let conn = db::open_memory().expect("an in-memory database opens");
        db::prepare(&conn, RAM_LIBRARY.bytes()).expect("an empty database takes the schema");
        let mut lib = Self::with(conn, None, opts, Mode::Ram, Vec::new())
            .expect("an in-memory library opens");
        lib.unsaved = Some(reason.into());
        lib.stats.created = true;
        lib
    }

    fn with(
        conn: Connection,
        dir: Option<PathBuf>,
        opts: MountOptions,
        mode: Mode,
        warnings: Vec<String>,
    ) -> Result<Self, MountError> {
        let (id, clean) = db::prepare(&conn, LibraryId::random().bytes())?;
        let id = LibraryId::from_bytes(id);
        let tracks: i64 = conn
            .query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))
            .map_err(DbError::from)?;
        let mut lib = Self {
            conn,
            dir,
            id,
            device: opts.device.bytes(),
            now: opts.now,
            mode,
            unsaved: None,
            queue: Vec::new(),
            session: Vec::new(),
            in_tx: false,
            started: false,
            unclean: !clean && mode == Mode::Folder,
            ram_blobs: HashMap::new(),
            warnings,
            stats: MountStats {
                tracks: tracks as usize,
                needs_check: !clean && mode == Mode::Folder,
                ..MountStats::default()
            },
        };
        let parked = lock_parked().remove(&id).unwrap_or_default();
        if !parked.ops.is_empty() || !parked.blobs.is_empty() {
            lib.stats.replayed = parked.ops.len();
            for (id, bytes) in parked.blobs {
                lib.store_blob(id, bytes);
            }
            for op in parked.ops {
                lib.write(op);
            }
            lib.flush();
        }
        Ok(lib)
    }

    pub fn id(&self) -> LibraryId {
        self.id
    }

    pub fn stats(&self) -> &MountStats {
        &self.stats
    }

    /// Things worth telling the user since the last call (failed writes, a damaged file).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    pub fn persistence(&self) -> Persistence {
        match &self.unsaved {
            Some(reason) => Persistence::Unsaved {
                edits: self.queue.len(),
                reason: reason.clone(),
            },
            None if self.queue.is_empty() => Persistence::Saved,
            None => Persistence::Pending,
        }
    }

    pub fn track_ref(&self, track: TrackId) -> TrackRef {
        TrackRef {
            library: self.id,
            track,
        }
    }

    // Writing

    fn write(&mut self, op: Op) {
        if self.mode == Mode::Folder
            && !self.in_tx
            && let Err(e) = self.begin()
        {
            self.fail(&e);
        }
        if self.mode != Mode::Failed
            && let Err(e) = op.exec(&self.conn, self.device, (self.now)() as i64)
        {
            self.fail_db(&e);
        }
        self.session.push(op.clone());
        self.queue.push(op);
    }

    /// Open the write transaction. The first one of a mount backs the file up and marks it
    /// in use.
    fn begin(&mut self) -> rusqlite::Result<()> {
        if !self.started {
            if !self.stats.created
                && let Err(e) = self.backup()
            {
                self.warnings.push(format!("no backup made: {e}"));
            }
            self.started = true;
        }
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        self.in_tx = true;
        db::set_clean(&self.conn, false)
    }

    /// `library.db.bak`: the file as it was before this mount wrote to it.
    fn backup(&self) -> io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let tmp = dir.join(format!("{BACKUP_FILE}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        self.conn
            .execute("VACUUM INTO ?1", [tmp.to_string_lossy()])
            .map_err(io::Error::other)?;
        std::fs::File::open(&tmp)?.sync_all()?;
        std::fs::rename(&tmp, dir.join(BACKUP_FILE))?;
        sync_dir(dir)
    }

    fn fail(&mut self, e: &dyn std::fmt::Display) {
        if self.in_tx {
            let _ = self.conn.execute_batch("ROLLBACK");
            self.in_tx = false;
        }
        if self.mode == Mode::Folder {
            self.mode = Mode::Failed;
            let reason = format!("can't write the library: {e}");
            self.warnings.push(reason.clone());
            self.unsaved = Some(reason);
        }
    }

    /// A failed statement; a damaged file also asks for [`Library::check`].
    fn fail_db(&mut self, e: &rusqlite::Error) {
        if db::is_damage(e) {
            self.stats.needs_check = true;
        }
        self.fail(e);
    }

    /// Commit the queued edits. After a failed write, try writing the whole queue again.
    pub fn flush(&mut self) {
        match self.mode {
            Mode::Ram => {}
            Mode::Folder => {
                if self.in_tx {
                    match self.conn.execute_batch("COMMIT") {
                        Ok(()) => {
                            self.in_tx = false;
                            self.queue.clear();
                        }
                        Err(e) => self.fail_db(&e),
                    }
                }
            }
            Mode::Failed => {
                if self.retry().is_ok() {
                    self.mode = Mode::Folder;
                    self.unsaved = None;
                    self.queue.clear();
                    self.warnings.push("library: writing again".into());
                }
            }
        }
    }

    /// Write the blobs kept in RAM and the queue in one transaction.
    fn retry(&mut self) -> Result<(), String> {
        let dir = self.dir.clone().expect("only a folder library fails");
        for (id, bytes) in &self.ram_blobs {
            write_blob(&dir, *id, bytes).map_err(|e| e.to_string())?;
        }
        self.ram_blobs.clear();
        let now = (self.now)() as i64;
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let done = db::set_clean(&self.conn, false).and_then(|()| {
            self.queue
                .iter()
                .try_for_each(|op| op.exec(&self.conn, self.device, now))
        });
        match done.and_then(|()| self.conn.execute_batch("COMMIT")) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e.to_string())
            }
        }
    }

    /// Check the file after a session that didn't close it cleanly, or after a write found
    /// damage (`PRAGMA quick_check`; seconds on a big library on a stick). Cheap when there
    /// is nothing to check. Damage sets the file aside, goes back to the
    /// backup and writes this mount's edits onto it. Returns what was found, if anything.
    pub fn check(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.stats.needs_check) || self.mode == Mode::Ram {
            return None;
        }
        let problem = match db::quick_check(&self.conn) {
            Ok(None) => return None,
            Ok(Some(p)) => p,
            Err(e) => e.to_string(),
        };
        let dir = self.dir.clone().expect("a folder library");
        if self.in_tx {
            let _ = self.conn.execute_batch("ROLLBACK");
            self.in_tx = false;
        }
        let reopened = (|| -> Result<Connection, MountError> {
            drop(std::mem::replace(&mut self.conn, db::open_memory()?));
            let line = set_aside(&dir, &problem)?;
            self.warnings.push(line);
            let conn = db::open_rw(&dir.join(DB_FILE))?;
            db::prepare(&conn, self.id.bytes())?;
            Ok(conn)
        })();
        match reopened {
            Ok(conn) => {
                self.conn = conn;
                self.started = true;
                self.mode = Mode::Folder;
                self.unsaved = None;
                let ops = std::mem::take(&mut self.session);
                self.queue.clear();
                for op in ops {
                    self.write(op);
                }
                self.flush();
            }
            Err(e) => {
                self.mode = Mode::Ram;
                self.unsaved = Some(format!("library damaged: {e}"));
            }
        }
        Some(problem)
    }

    /// Make an edit. Edits to a track that was merged into another go to the survivor.
    /// Returns the cue's id for [`Edit::AddCue`] and [`Edit::ReplaceCue`].
    pub fn apply(&mut self, edit: Edit) -> Result<Option<String>, WriteError> {
        let track = match &edit {
            Edit::SetRating(t, _)
            | Edit::AddCue(t, _)
            | Edit::ReplaceCue(t, _, _)
            | Edit::RemoveCue(t, _)
            | Edit::SetHotCue(t, _, _)
            | Edit::SetAnalysis(t, _, _)
            | Edit::SetWaveform(t, _)
            | Edit::SetTags(t, _) => *t,
        };
        // Without a readable file (a pulled stick) the edit is queued unchecked.
        let t = resolve(&self.conn, track).unwrap_or(track);
        if self.has_track(t) == Some(false) {
            return Err(WriteError::NoSuchTrack(track));
        }
        let cue_of = |lib: &Self, id: &str| -> Result<[u8; 16], WriteError> {
            let bytes = unhex::<16>(id).ok_or_else(|| WriteError::NoSuchCue(id.into()))?;
            let owner: rusqlite::Result<Option<Vec<u8>>> = lib
                .conn
                .query_row("SELECT track FROM cues WHERE id = ?1", [&bytes[..]], |r| {
                    r.get(0)
                })
                .optional();
            match owner {
                Ok(Some(o)) if o == t.0 => Ok(bytes),
                Ok(_) => Err(WriteError::NoSuchCue(id.into())),
                Err(_) => Ok(bytes),
            }
        };
        let (op, id) = match edit {
            Edit::SetRating(_, r) => (Op::Rating(t, r), None),
            Edit::AddCue(_, cue) => {
                let id = LibraryId::random().bytes();
                (Op::PutCue(t, id, cue), Some(hex(&id)))
            }
            Edit::ReplaceCue(_, old, cue) => {
                let id = cue_of(self, &old)?;
                (Op::PutCue(t, id, cue), Some(old))
            }
            Edit::RemoveCue(_, old) => (Op::RemoveCue(cue_of(self, &old)?), None),
            Edit::SetHotCue(_, slot, cue) => {
                if slot >= HOT_CUES {
                    return Err(WriteError::BadHotCueSlot(slot));
                }
                (Op::HotCue(t, slot, cue), None)
            }
            Edit::SetAnalysis(_, name, a) => {
                if !is_component(&name) {
                    return Err(WriteError::BadAnalyzerName(name));
                }
                (Op::Analysis(t, name, a), None)
            }
            Edit::SetWaveform(_, w) => (Op::Waveform(t, w), None),
            Edit::SetTags(_, tags) => (Op::Tags(t, tags), None),
        };
        self.write(op);
        Ok(id)
    }

    /// `Some(true)` if the track has a record, `None` if the file can't be read.
    fn has_track(&self, t: TrackId) -> Option<bool> {
        self.conn
            .query_row("SELECT 1 FROM tracks WHERE id = ?1", [&t.0[..]], |_| Ok(()))
            .optional()
            .ok()
            .map(|r| r.is_some())
    }

    // Identity

    /// The track `id` was merged into (itself if none).
    pub fn resolve(&self, id: TrackId) -> TrackId {
        resolve(&self.conn, id).unwrap_or(id)
    }

    /// Find the track a file belongs to by its file hash, or make its record. A known file
    /// at a new path gets that path as another location (an exact duplicate, or a moved
    /// file); a new file gets the id derived from its hash.
    pub fn find_or_create(&mut self, file: FileIdentity) -> Result<Found, ReadError> {
        let known: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT l.track FROM locations l JOIN tracks t ON t.id = l.track \
                 WHERE l.file_hash = ?1 ORDER BY t.rowid LIMIT 1",
                [&file.file_hash.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        let found = match known.map(|b| track_of(&b)) {
            Some(track) => {
                let have: bool = self.conn.query_row(
                    "SELECT count(*) > 0 FROM locations WHERE track = ?1 AND path = ?2",
                    params![&track.0[..], file.path],
                    |r| r.get(0),
                )?;
                Found {
                    track,
                    created: false,
                    new_location: !have,
                }
            }
            None => {
                let track = resolve(&self.conn, track_id(&file.file_hash))?;
                Found {
                    track,
                    created: self.has_track(track) != Some(true),
                    new_location: true,
                }
            }
        };
        if found.new_location {
            self.write(Op::Locate(found.track, file));
            if found.created {
                self.stats.tracks += 1;
            }
        }
        Ok(found)
    }

    /// Record the audio hash of `track`, then merge every other track with the same audio
    /// hash (tag-edited copies): the one this library saw first survives and the others
    /// become its aliases. Returns the surviving track.
    pub fn set_audio_hash(
        &mut self,
        track: TrackId,
        audio: AudioHash,
    ) -> Result<TrackId, ReadError> {
        let root = resolve(&self.conn, track)?;
        self.write(Op::AudioHash(root, audio));
        let mut st = self.conn.prepare_cached(
            "SELECT id FROM tracks WHERE audio_hash = ?1 AND audio_hash_v = ?2 ORDER BY rowid",
        )?;
        let same: Vec<TrackId> = st
            .query_map(params![&audio.hash[..], audio.v], |r| {
                Ok(track_of(&r.get::<_, Vec<u8>>(0)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(st);
        let survivor = same.first().copied().unwrap_or(root);
        for &from in same.iter().filter(|&&t| t != survivor) {
            self.write(Op::Merge {
                from,
                into: survivor,
            });
            self.stats.tracks = self.stats.tracks.saturating_sub(1);
        }
        Ok(survivor)
    }

    // Reading

    /// The record of a track (following merges), if the library has one.
    pub fn track(&self, id: TrackId) -> Result<Option<Track>, ReadError> {
        let c = &self.conn;
        let id = resolve(c, id)?;
        let key = &id.0[..];
        let Some(mut t) = c
            .query_row(
                "SELECT rating, audio_hash, audio_hash_v, waveform, waveform_overview, \
                 title, artist, album, genre, bpm, key, duration FROM tracks WHERE id = ?1",
                [key],
                |r| {
                    let mut t = Track::empty(id);
                    t.rating = r.get(0)?;
                    let hash: Option<Vec<u8>> = r.get(1)?;
                    t.audio_hash = hash.and_then(|h| {
                        Some(AudioHash {
                            v: r.get(2).ok()?,
                            hash: h.try_into().ok()?,
                        })
                    });
                    t.waveform = match (blob_col(r, 3)?, blob_col(r, 4)?) {
                        (Some(blob), Some(overview_blob)) => Some(WaveformRef {
                            blob,
                            overview_blob,
                        }),
                        _ => None,
                    };
                    t.tags = tags_at(r, 5)?;
                    Ok(t)
                },
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut st = c.prepare_cached(
            "SELECT path, size, file_hash FROM locations WHERE track = ?1 \
             ORDER BY updated_at, path",
        )?;
        t.locations = st
            .query_map([key], |r| {
                let hash: Vec<u8> = r.get(2)?;
                Ok(FileIdentity {
                    path: r.get(0)?,
                    size: r.get::<_, i64>(1)? as u64,
                    file_hash: FileHash(hash.try_into().unwrap_or([0; 32])),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut st = c.prepare_cached(
            "SELECT id, kind, at, end_at, color, comment FROM cues WHERE track = ?1 \
             ORDER BY at, id",
        )?;
        t.cues = st
            .query_map([key], |r| {
                let id: Vec<u8> = r.get(0)?;
                let kind: String = r.get(1)?;
                Ok((
                    hex(&id),
                    Cue {
                        kind: CueKind::parse(&kind),
                        at: r.get(2)?,
                        end: r.get(3)?,
                        color: r.get(4)?,
                        comment: r.get(5)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut st = c.prepare_cached(
            "SELECT slot, at, end_at, color, comment FROM hot_cues WHERE track = ?1",
        )?;
        let hot = st.query_map([key], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                HotCue {
                    at: r.get(1)?,
                    end: r.get(2)?,
                    color: r.get(3)?,
                    comment: r.get(4)?,
                },
            ))
        })?;
        for h in hot {
            let (slot, cue) = h?;
            if let Some(s) = usize::try_from(slot)
                .ok()
                .and_then(|s| t.hot_cues.get_mut(s))
            {
                *s = Some(cue);
            }
        }
        let mut st = c.prepare_cached(
            "SELECT analyzer, version, params, blob, created FROM analysis WHERE track = ?1",
        )?;
        let rows = st.query_map([key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                blob_col(r, 3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            if let (name, version, params, Some(blob), created) = row? {
                t.analysis.insert(
                    name,
                    AnalysisRef {
                        version,
                        params,
                        blob,
                        created,
                    },
                );
            }
        }
        let mut st = c.prepare_cached("SELECT old FROM aliases WHERE new = ?1 ORDER BY old")?;
        t.merged = st
            .query_map([key], |r| Ok(track_of(&r.get::<_, Vec<u8>>(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(t))
    }

    /// Every location of every track with the track's tags, by path: what the browser lists
    /// and searches.
    pub fn listing(&self) -> Result<Vec<Listed>, ReadError> {
        let mut st = self.conn.prepare_cached(
            "SELECT l.track, l.path, t.title, t.artist, t.album, t.genre, t.bpm, t.key, \
             t.duration, t.rating, \
             (t.waveform IS NOT NULL AND EXISTS (SELECT 1 FROM analysis a WHERE a.track = t.id)) \
             FROM locations l JOIN tracks t ON t.id = l.track ORDER BY l.path",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Listed {
                    track: track_of(&r.get::<_, Vec<u8>>(0)?),
                    path: r.get(1)?,
                    tags: tags_at(r, 2)?,
                    rating: r.get(9)?,
                    analysed: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Every track id (merged tracks resolve to their survivor and are not listed), in the
    /// order the library first saw them.
    pub fn tracks(&self) -> Result<Vec<TrackId>, ReadError> {
        let mut st = self
            .conn
            .prepare_cached("SELECT id FROM tracks ORDER BY rowid")?;
        let ids = st
            .query_map([], |r| Ok(track_of(&r.get::<_, Vec<u8>>(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    // Blobs

    /// Store a blob under its BLAKE3 name, `blobs/<aa>/<hash>`, written once (tmp, sync,
    /// rename) before any row names it. Without a writable folder it stays in RAM.
    pub fn put_blob(&mut self, bytes: &[u8]) -> BlobId {
        let id = BlobId::of(bytes);
        self.store_blob(id, bytes.to_vec());
        id
    }

    fn store_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
        if self.mode == Mode::Folder {
            let dir = self.dir.as_ref().expect("a folder library");
            match write_blob(dir, id, &bytes) {
                Ok(()) => return,
                // The row that names it must not be committed: fail the transaction.
                Err(e) => self.fail(&e),
            }
        }
        self.ram_blobs.insert(id, bytes);
    }

    /// A blob's bytes, checked against its name.
    pub fn read_blob(&self, id: BlobId) -> io::Result<Vec<u8>> {
        if let Some(b) = self.ram_blobs.get(&id) {
            return Ok(b.clone());
        }
        let Some(dir) = &self.dir else {
            return Err(io::ErrorKind::NotFound.into());
        };
        let bytes = std::fs::read(dir.join(id.path()))?;
        if BlobId::of(&bytes) != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("blob {id} is damaged"),
            ));
        }
        Ok(bytes)
    }
}

impl Drop for Library {
    /// Commit, mark the file cleanly closed and fold the WAL back into it. Edits that could
    /// not be written wait in this process for the next mount of this library.
    fn drop(&mut self) {
        self.flush();
        if self.mode == Mode::Folder {
            // Still clean, or checked; a check that never ran is left for the next mount.
            if (self.started || self.unclean) && !self.stats.needs_check {
                let _ = db::set_clean(&self.conn, true);
            }
            let _ = self.conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
        }
        if !self.queue.is_empty() || !self.ram_blobs.is_empty() {
            let mut parked = lock_parked();
            let p = parked.entry(self.id).or_default();
            p.ops.append(&mut self.queue);
            p.blobs.extend(self.ram_blobs.drain());
        }
    }
}

fn lock_parked() -> std::sync::MutexGuard<'static, BTreeMap<LibraryId, Parked>> {
    PARKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Op {
    /// Run this op's SQL. `now` and `device` stamp the rows it writes.
    fn exec(&self, c: &Connection, device: [u8; 16], now: i64) -> rusqlite::Result<()> {
        let dev = &device[..];
        match self {
            Op::Locate(t, f) => {
                let t = resolve(c, *t)?;
                c.prepare_cached(
                    "INSERT INTO tracks(id, updated_at, device) VALUES (?1, ?2, ?3) \
                     ON CONFLICT(id) DO NOTHING",
                )?
                .execute(params![&t.0[..], now, dev])?;
                c.prepare_cached(
                    "INSERT INTO locations(track, path, size, file_hash, updated_at, device) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(track, path) DO UPDATE SET \
                     size = excluded.size, file_hash = excluded.file_hash, \
                     updated_at = excluded.updated_at, device = excluded.device",
                )?
                .execute(params![
                    &t.0[..],
                    f.path,
                    f.size as i64,
                    &f.file_hash.0[..],
                    now,
                    dev
                ])?;
            }
            Op::AudioHash(t, a) => {
                c.prepare_cached(
                    "UPDATE tracks SET audio_hash = ?2, audio_hash_v = ?3, updated_at = ?4, \
                     device = ?5 WHERE id = ?1",
                )?
                .execute(params![
                    &resolve(c, *t)?.0[..],
                    &a.hash[..],
                    a.v,
                    now,
                    dev
                ])?;
            }
            Op::Merge { from, into } => merge(c, *from, *into, dev, now)?,
            Op::Rating(t, r) => {
                c.prepare_cached(
                    "UPDATE tracks SET rating = ?2, updated_at = ?3, device = ?4 WHERE id = ?1",
                )?
                .execute(params![&resolve(c, *t)?.0[..], r, now, dev])?;
            }
            Op::PutCue(t, id, cue) => {
                c.prepare_cached(
                    "INSERT INTO cues(id, track, kind, at, end_at, color, comment, updated_at, \
                     device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                     ON CONFLICT(id) DO UPDATE SET track = excluded.track, kind = excluded.kind, \
                     at = excluded.at, end_at = excluded.end_at, color = excluded.color, \
                     comment = excluded.comment, updated_at = excluded.updated_at, \
                     device = excluded.device",
                )?
                .execute(params![
                    &id[..],
                    &resolve(c, *t)?.0[..],
                    cue.kind.as_str(),
                    cue.at,
                    cue.end,
                    cue.color,
                    cue.comment,
                    now,
                    dev
                ])?;
            }
            Op::RemoveCue(id) => {
                c.prepare_cached("DELETE FROM cues WHERE id = ?1")?
                    .execute([&id[..]])?;
            }
            Op::HotCue(t, slot, None) => {
                c.prepare_cached("DELETE FROM hot_cues WHERE track = ?1 AND slot = ?2")?
                    .execute(params![&resolve(c, *t)?.0[..], *slot as i64])?;
            }
            Op::HotCue(t, slot, Some(h)) => {
                c.prepare_cached(
                    "INSERT INTO hot_cues(track, slot, at, end_at, color, comment, updated_at, \
                     device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(track, slot) DO UPDATE SET at = excluded.at, \
                     end_at = excluded.end_at, color = excluded.color, \
                     comment = excluded.comment, updated_at = excluded.updated_at, \
                     device = excluded.device",
                )?
                .execute(params![
                    &resolve(c, *t)?.0[..],
                    *slot as i64,
                    h.at,
                    h.end,
                    h.color,
                    h.comment,
                    now,
                    dev
                ])?;
            }
            Op::Analysis(t, name, a) => {
                c.prepare_cached(
                    "INSERT INTO analysis(track, analyzer, version, params, blob, created, \
                     updated_at, device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(track, analyzer) DO UPDATE SET version = excluded.version, \
                     params = excluded.params, blob = excluded.blob, \
                     created = excluded.created, updated_at = excluded.updated_at, \
                     device = excluded.device",
                )?
                .execute(params![
                    &resolve(c, *t)?.0[..],
                    name,
                    a.version,
                    a.params,
                    &a.blob.0[..],
                    a.created,
                    now,
                    dev
                ])?;
            }
            Op::Tags(t, g) => {
                c.prepare_cached(
                    "UPDATE tracks SET title = ?2, artist = ?3, album = ?4, genre = ?5, \
                     bpm = ?6, key = ?7, duration = ?8, updated_at = ?9, device = ?10 \
                     WHERE id = ?1",
                )?
                .execute(params![
                    &resolve(c, *t)?.0[..],
                    g.title,
                    g.artist,
                    g.album,
                    g.genre,
                    g.bpm,
                    g.key,
                    g.duration,
                    now,
                    dev
                ])?;
            }
            Op::Waveform(t, w) => {
                c.prepare_cached(
                    "UPDATE tracks SET waveform = ?2, waveform_overview = ?3, updated_at = ?4, \
                     device = ?5 WHERE id = ?1",
                )?
                .execute(params![
                    &resolve(c, *t)?.0[..],
                    &w.blob.0[..],
                    &w.overview_blob.0[..],
                    now,
                    dev
                ])?;
            }
        }
        Ok(())
    }
}

/// Move everything of `from` to `into`: locations and memory cues all; hot cues and
/// analysis results where `into` has none for that slot or analyzer, or an older one; track
/// fields where `into` has none. `from` is then deleted and becomes an alias of `into`, and
/// aliases of `from` point to `into`.
fn merge(
    c: &Connection,
    from: TrackId,
    into: TrackId,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    let into = resolve(c, into)?;
    let from = resolve(c, from)?;
    if from == into {
        return Ok(());
    }
    let (f, i) = (&from.0[..], &into.0[..]);
    c.execute(
        "INSERT INTO locations(track, path, size, file_hash, updated_at, device) \
         SELECT ?2, path, size, file_hash, updated_at, device FROM locations WHERE track = ?1 \
         ON CONFLICT(track, path) DO NOTHING",
        [f, i],
    )?;
    c.execute("UPDATE cues SET track = ?2 WHERE track = ?1", [f, i])?;
    c.execute(
        "INSERT INTO hot_cues(track, slot, at, end_at, color, comment, updated_at, device) \
         SELECT ?2, slot, at, end_at, color, comment, updated_at, device FROM hot_cues h \
         WHERE track = ?1 AND NOT EXISTS (SELECT 1 FROM hot_cues s WHERE s.track = ?2 \
         AND s.slot = h.slot AND s.updated_at >= h.updated_at) \
         ON CONFLICT(track, slot) DO UPDATE SET at = excluded.at, end_at = excluded.end_at, \
         color = excluded.color, comment = excluded.comment, updated_at = excluded.updated_at, \
         device = excluded.device",
        [f, i],
    )?;
    c.execute(
        "INSERT INTO analysis(track, analyzer, version, params, blob, created, updated_at, \
         device) SELECT ?2, analyzer, version, params, blob, created, updated_at, device \
         FROM analysis a WHERE track = ?1 AND NOT EXISTS (SELECT 1 FROM analysis s \
         WHERE s.track = ?2 AND s.analyzer = a.analyzer AND s.updated_at >= a.updated_at) \
         ON CONFLICT(track, analyzer) DO UPDATE SET version = excluded.version, \
         params = excluded.params, blob = excluded.blob, created = excluded.created, \
         updated_at = excluded.updated_at, device = excluded.device",
        [f, i],
    )?;
    c.execute(
        "UPDATE tracks SET \
         rating = coalesce(rating, (SELECT rating FROM tracks WHERE id = ?1)), \
         color = coalesce(color, (SELECT color FROM tracks WHERE id = ?1)), \
         comment = coalesce(comment, (SELECT comment FROM tracks WHERE id = ?1)), \
         waveform = coalesce(waveform, (SELECT waveform FROM tracks WHERE id = ?1)), \
         waveform_overview = coalesce(waveform_overview, \
           (SELECT waveform_overview FROM tracks WHERE id = ?1)) \
         WHERE id = ?2",
        [f, i],
    )?;
    c.execute("DELETE FROM tracks WHERE id = ?1", [f])?;
    c.execute(
        "UPDATE aliases SET new = ?2, updated_at = ?3, device = ?4 WHERE new = ?1",
        params![f, i, now, dev],
    )?;
    c.execute(
        "INSERT INTO aliases(old, new, updated_at, device) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(old) DO UPDATE SET new = excluded.new, updated_at = excluded.updated_at, \
         device = excluded.device",
        params![f, i, now, dev],
    )?;
    Ok(())
}

/// The track `id` was merged into (aliases point straight at the survivor).
fn resolve(c: &Connection, id: TrackId) -> rusqlite::Result<TrackId> {
    let new: Option<Vec<u8>> = c
        .prepare_cached("SELECT new FROM aliases WHERE old = ?1")?
        .query_row([&id.0[..]], |r| r.get(0))
        .optional()?;
    Ok(new.map_or(id, |b| track_of(&b)))
}

/// The seven tag cache columns starting at column `i`.
fn tags_at(r: &Row, i: usize) -> rusqlite::Result<TrackTags> {
    Ok(TrackTags {
        title: r.get(i)?,
        artist: r.get(i + 1)?,
        album: r.get(i + 2)?,
        genre: r.get(i + 3)?,
        bpm: r.get(i + 4)?,
        key: r.get(i + 5)?,
        duration: r.get(i + 6)?,
    })
}

fn track_of(b: &[u8]) -> TrackId {
    TrackId(b.try_into().unwrap_or([0; 16]))
}

fn blob_col(r: &Row, i: usize) -> rusqlite::Result<Option<BlobId>> {
    let b: Option<Vec<u8>> = r.get(i)?;
    Ok(b.and_then(|b| b.try_into().ok()).map(BlobId))
}

/// Write a blob unless it is there already: tmp file, sync, rename, sync the folder.
fn write_blob(dir: &Path, id: BlobId, bytes: &[u8]) -> io::Result<()> {
    let path = dir.join(id.path());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(());
    }
    let folder = path.parent().expect("blobs/<aa>/");
    std::fs::create_dir_all(folder)?;
    let tmp = folder.join(format!(".{id}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, &path)?;
    sync_dir(folder)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Move a damaged `library.db` (and its WAL) aside as `library.db.corrupt-<date>` and put
/// the backup in its place, if there is one. Returns a line for the user.
fn set_aside(dir: &Path, why: &str) -> io::Result<String> {
    let stamp = rfc3339(system_ms()).replace(':', "");
    let db = dir.join(DB_FILE);
    let kept = dir.join(format!("{DB_FILE}.corrupt-{stamp}"));
    std::fs::rename(&db, &kept)?;
    let wal = dir.join(format!("{DB_FILE}-wal"));
    if wal.exists() {
        std::fs::rename(&wal, dir.join(format!("{DB_FILE}.corrupt-{stamp}-wal")))?;
    }
    let bak = dir.join(BACKUP_FILE);
    let restored = bak.exists() && std::fs::copy(&bak, &db).is_ok();
    sync_dir(dir)?;
    let name = kept.file_name().unwrap_or_default().to_string_lossy();
    Ok(if restored {
        format!(
            "library damaged ({why}): kept as {name}, back to the backup made at the last mount"
        )
    } else {
        format!("library damaged ({why}): kept as {name}, starting an empty library")
    })
}

/// `ms` since the Unix epoch as RFC 3339 UTC, e.g. `2026-10-21T22:41:07Z`.
pub fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant), for the proleptic Gregorian calendar.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_formats_utc() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339(1_792_621_267_000), "2026-10-21T22:21:07Z");
    }
}
