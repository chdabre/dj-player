//! Damage: the check after an unclean session, and setting a damaged file aside for the
//! backup.

use std::io;
use std::path::Path;

use rusqlite::Connection;

use crate::db::{self, BACKUP_FILE, DB_FILE};
use crate::time::{rfc3339, system_ms};

use super::blobs::sync_dir;
use super::{Library, Mode, MountError};

impl Library {
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
        self.rollback();
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
}

/// Move a damaged `library.db` (and its WAL) aside as `library.db.corrupt-<date>` and put
/// the backup in its place, if there is one. Returns a line for the user.
pub(super) fn set_aside(dir: &Path, why: &str) -> io::Result<String> {
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
