//! One open library: the SQLite file, the queue of edits not yet committed, and the blobs.
//!
//! The library worker is the only writer. An edit runs against the file at once, inside a
//! write transaction that [`Library::flush`] commits (the player calls it about once a
//! second), and stays in the queue until that commit. If a write fails (the stick was
//! pulled, is full or went read-only) the transaction is rolled back, the queue is kept and
//! the library says [`Persistence::Unsaved`]. When the library is dropped with edits still
//! queued, they are kept in this process and replayed when the same library (by its id) is
//! opened again.
//!
//! - `txn`: the queue, the write transaction and what a failed write does.
//! - `recover`: the check after an unclean session, and setting a damaged file aside.
//! - `ops`: [`Op`], one row-level write and its SQL.
//! - `edits`, `playlists`: turning an [`Edit`] into ops.
//! - `identity`: finding, creating and merging tracks.
//! - `read`: tracks and the listing. `blobs`: the content-addressed files.

mod blobs;
mod edits;
mod identity;
mod ops;
mod playlists;
mod read;
mod recover;
mod txn;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db::{self, DB_FILE, DbError};
use crate::ids::{BlobId, LibraryId, TrackId, TrackRef};

pub use edits::Edit;
use ops::Op;
use recover::set_aside;
use txn::lock_parked;
pub use types::{Found, MountError, MountOptions, MountStats, Persistence, ReadError, WriteError};

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
