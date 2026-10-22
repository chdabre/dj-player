//! The queue and the write transaction: every op runs at once inside a transaction that
//! [`Library::flush`] commits; a failed write rolls it back and keeps the queue in RAM.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Mutex;

use crate::db::{self, BACKUP_FILE};
use crate::ids::{BlobId, LibraryId};

use super::blobs::{sync_dir, write_blob};
use super::{Library, Mode, Op};

/// Edits and blobs a dropped library could not write, by library.
#[derive(Default)]
pub(super) struct Parked {
    pub(super) ops: Vec<Op>,
    pub(super) blobs: HashMap<BlobId, Vec<u8>>,
}

static PARKED: Mutex<BTreeMap<LibraryId, Parked>> = Mutex::new(BTreeMap::new());

pub(super) fn lock_parked() -> std::sync::MutexGuard<'static, BTreeMap<LibraryId, Parked>> {
    PARKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Library {
    pub(super) fn write(&mut self, op: Op) {
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
        self.begin_tx()
    }

    /// `BEGIN IMMEDIATE`, then mark the file in use (`clean_unmount = 0`). `in_tx` is set
    /// once the transaction is open, so a failure after that rolls it back.
    fn begin_tx(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        self.in_tx = true;
        db::set_clean(&self.conn, false)
    }

    /// Roll back the write transaction, if one is open.
    pub(super) fn rollback(&mut self) {
        if self.in_tx {
            let _ = self.conn.execute_batch("ROLLBACK");
            self.in_tx = false;
        }
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

    pub(super) fn fail(&mut self, e: &dyn std::fmt::Display) {
        self.rollback();
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
        let done = self.begin_tx().and_then(|()| {
            self.queue
                .iter()
                .try_for_each(|op| op.exec(&self.conn, self.device, now))
        });
        match done.and_then(|()| self.conn.execute_batch("COMMIT")) {
            Ok(()) => {
                self.in_tx = false;
                Ok(())
            }
            Err(e) => {
                self.rollback();
                Err(e.to_string())
            }
        }
    }
}
