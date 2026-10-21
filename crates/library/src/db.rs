//! The SQLite file: schema, pragmas and versions.
//!
//! Every table that holds an edit carries `updated_at` (wall-clock ms, 0 without a clock) and
//! `device` (16-byte id), written and not yet read: they are there for a later sync layer.
//! Writes name their columns (`INSERT … ON CONFLICT DO UPDATE SET` on the columns this
//! version knows, never `INSERT OR REPLACE`), so columns added by a newer version survive.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

/// The schema this build writes, in `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 2;

/// The file inside `DJPLAYER/`.
pub const DB_FILE: &str = "library.db";

/// The copy made before the first write of each mount (`VACUUM INTO`).
pub const BACKUP_FILE: &str = "library.db.bak";

/// The WAL grows to at most this much before a checkpoint truncates it.
const JOURNAL_SIZE_LIMIT: i64 = 4 << 20;

pub const SCHEMA: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value) WITHOUT ROWID;
CREATE TABLE tracks(
  id BLOB PRIMARY KEY,
  title TEXT, artist TEXT, album TEXT, genre TEXT, bpm REAL, key TEXT, duration REAL,
  rating INTEGER, color TEXT, comment TEXT,
  audio_hash BLOB, audio_hash_v INTEGER,
  waveform BLOB, waveform_overview BLOB,
  updated_at INTEGER NOT NULL, device BLOB NOT NULL);
CREATE INDEX tracks_audio_hash ON tracks(audio_hash);
CREATE TABLE locations(
  track BLOB NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  path TEXT NOT NULL, size INTEGER NOT NULL, file_hash BLOB NOT NULL,
  updated_at INTEGER NOT NULL, device BLOB NOT NULL,
  PRIMARY KEY(track, path)) WITHOUT ROWID;
CREATE INDEX locations_file_hash ON locations(file_hash);
CREATE TABLE hot_cues(
  track BLOB NOT NULL REFERENCES tracks(id) ON DELETE CASCADE, slot INTEGER NOT NULL,
  at REAL NOT NULL, end_at REAL, color TEXT, comment TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL, device BLOB NOT NULL,
  PRIMARY KEY(track, slot)) WITHOUT ROWID;
CREATE TABLE cues(
  id BLOB PRIMARY KEY,
  track BLOB NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  kind TEXT NOT NULL, at REAL NOT NULL, end_at REAL, color TEXT,
  comment TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL, device BLOB NOT NULL) WITHOUT ROWID;
CREATE INDEX cues_track ON cues(track);
CREATE TABLE analysis(
  track BLOB NOT NULL REFERENCES tracks(id) ON DELETE CASCADE, analyzer TEXT NOT NULL,
  version TEXT NOT NULL, params TEXT NOT NULL, blob BLOB NOT NULL, created TEXT NOT NULL,
  updated_at INTEGER NOT NULL, device BLOB NOT NULL,
  PRIMARY KEY(track, analyzer)) WITHOUT ROWID;
CREATE TABLE aliases(
  old BLOB PRIMARY KEY, new BLOB NOT NULL,
  updated_at INTEGER NOT NULL, device BLOB NOT NULL) WITHOUT ROWID;
CREATE INDEX aliases_new ON aliases(new);
"#;

/// Version 2: playlists. Entries name their track without a foreign key: a merge moves
/// them to the survivor, and a reader resolves an older id through `aliases`.
pub const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS playlists(
  id BLOB PRIMARY KEY,
  parent BLOB REFERENCES playlists(id) ON DELETE CASCADE,
  kind TEXT NOT NULL, name TEXT NOT NULL, position TEXT NOT NULL, created INTEGER NOT NULL,
  updated_at INTEGER NOT NULL, device BLOB NOT NULL) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS playlists_parent ON playlists(parent, position);
CREATE TABLE IF NOT EXISTS playlist_entries(
  id BLOB PRIMARY KEY,
  playlist BLOB NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
  track BLOB NOT NULL, position TEXT NOT NULL, added INTEGER NOT NULL,
  updated_at INTEGER NOT NULL, device BLOB NOT NULL) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS playlist_entries_order ON playlist_entries(playlist, position);
CREATE INDEX IF NOT EXISTS playlist_entries_track ON playlist_entries(track);
"#;

/// Why a file can't be used.
#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    /// Written by a newer version that says this one can't read it.
    Newer {
        min_reader: i64,
    },
    /// Not one of our libraries (no `library_id`).
    NotOurs,
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "{e}"),
            DbError::Newer { min_reader } => write!(
                f,
                "written by a newer player (needs version {min_reader}, this is {SCHEMA_VERSION})"
            ),
            DbError::NotOurs => f.write_str("not a library of this player"),
        }
    }
}

impl DbError {
    /// The file itself is damaged (as opposed to locked, missing or newer).
    pub fn is_damage(&self) -> bool {
        match self {
            DbError::Sqlite(e) => is_damage(e),
            DbError::NotOurs => true,
            DbError::Newer { .. } => false,
        }
    }
}

/// SQLite says the file is damaged or not a database.
pub fn is_damage(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, DatabaseCorrupt | NotADatabase))
}

/// Open (or make) the library file for writing: WAL with exclusive locking (no `-shm` file,
/// and the lock keeps a second writer out), `synchronous=FULL`.
pub fn open_rw(path: &Path) -> Result<Connection, DbError> {
    let c = Connection::open(path)?;
    // A library another connection holds (the previous load's worker finishing): wait.
    c.busy_timeout(Duration::from_secs(5))?;
    c.pragma_update(None, "locking_mode", "exclusive")?;
    c.pragma_update(None, "journal_mode", "wal")?;
    c.pragma_update(None, "synchronous", "full")?;
    // macOS's fsync doesn't reach the medium; F_FULLFSYNC does.
    c.pragma_update(None, "fullfsync", true)?;
    c.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
    pragmas(&c)?;
    Ok(c)
}

/// The library file read once into RAM (a write-protected stick): nothing is written to it.
pub fn open_copy(path: &Path) -> Result<Connection, DbError> {
    let uri = format!(
        "file:{}?immutable=1",
        path.to_string_lossy()
            .replace('%', "%25")
            .replace('?', "%3f")
            .replace('#', "%23")
    );
    let file = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let mut c = Connection::open_in_memory()?;
    rusqlite::backup::Backup::new(&file, &mut c)?.run_to_completion(1024, Duration::ZERO, None)?;
    pragmas(&c)?;
    Ok(c)
}

/// An empty library in RAM.
pub fn open_memory() -> Result<Connection, DbError> {
    let c = Connection::open_in_memory()?;
    pragmas(&c)?;
    Ok(c)
}

fn pragmas(c: &Connection) -> rusqlite::Result<()> {
    c.pragma_update(None, "foreign_keys", true)?;
    c.pragma_update(None, "cache_size", -8000)
}

/// Make the schema in an empty file, or check the version of an existing one. Returns the
/// library id and whether the last session ended cleanly (no integrity check needed).
pub fn prepare(c: &Connection, new_id: [u8; 16]) -> Result<([u8; 16], bool), DbError> {
    let version: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version == 0 && tables == 0 {
        c.execute_batch(&format!(
            "BEGIN; {SCHEMA} {SCHEMA_V2} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
        ))?;
        tag_list(c)?;
        c.execute(
            "INSERT INTO meta(key, value) VALUES ('library_id', ?1), ('min_reader_version', 1), \
             ('clean_unmount', 1)",
            params![&new_id[..]],
        )?;
    }
    let meta = |k: &str| -> rusqlite::Result<Option<rusqlite::types::Value>> {
        c.query_row("SELECT value FROM meta WHERE key = ?1", [k], |r| r.get(0))
            .optional()
    };
    let min_reader = match meta("min_reader_version").map_err(|_| DbError::NotOurs)? {
        Some(rusqlite::types::Value::Integer(v)) => v,
        _ => return Err(DbError::NotOurs),
    };
    if min_reader > SCHEMA_VERSION {
        return Err(DbError::Newer { min_reader });
    }
    let id = match meta("library_id")? {
        Some(rusqlite::types::Value::Blob(b)) => b.try_into().map_err(|_| DbError::NotOurs)?,
        _ => return Err(DbError::NotOurs),
    };
    let clean = !matches!(
        meta("clean_unmount")?,
        Some(rusqlite::types::Value::Integer(0))
    );
    // Migrations from older versions, one step per version. A newer file this build may
    // still write keeps its version.
    let version: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 2 {
        c.execute_batch(&format!(
            "BEGIN; {SCHEMA_V2} PRAGMA user_version = 2; COMMIT;"
        ))?;
    }
    tag_list(c)?;
    Ok((id, clean))
}

/// Make the tag list if the file has none (a new file, a migrated one, or one an older
/// version wrote to after a newer one made the tables).
fn tag_list(c: &Connection) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO playlists(id, parent, kind, name, position, created, updated_at, device) \
         VALUES (?1, NULL, 'tag', 'TAG LIST', '0i', 0, 0, ?2) ON CONFLICT(id) DO NOTHING",
        params![&crate::PlaylistId::TAG_LIST.bytes()[..], &[0u8; 16][..]],
    )
    .map(drop)
}

/// Mark the file in use (`clean = false`, in the first write transaction) or cleanly closed.
pub fn set_clean(c: &Connection, clean: bool) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO meta(key, value) VALUES ('clean_unmount', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [i64::from(clean)],
    )
    .map(drop)
}

/// `PRAGMA quick_check`: `None` if the file is sound, else what it found.
pub fn quick_check(c: &Connection) -> rusqlite::Result<Option<String>> {
    let mut st = c.prepare("PRAGMA quick_check")?;
    let lines: Vec<String> = st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok((lines != ["ok"]).then(|| lines.join("; ")))
}
