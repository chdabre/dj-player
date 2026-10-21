//! Where a library's files live. The library does no I/O except through [`Store`], so it
//! runs the same on a stick folder, in RAM (tests, a RAM-only session) and later on a cloud
//! folder.
//!
//! Paths are relative to the library folder (`DJPLAYER/`), with `/` separators.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// Held while a library journals as one device; dropping it releases the lock.
pub type WriterLock = Box<dyn std::any::Any + Send>;

pub trait Store: Send {
    /// Names of the entries directly in `dir` (files and folders), sorted. A missing folder
    /// is empty.
    fn list(&self, dir: &str) -> io::Result<Vec<String>>;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// [`Store::read`], refusing a file over `limit` bytes with [`io::ErrorKind::InvalidData`]
    /// (a junk file on someone else's stick must not fill RAM).
    fn read_limited(&self, path: &str, limit: u64) -> io::Result<Vec<u8>> {
        let bytes = self.read(path)?;
        if bytes.len() as u64 > limit {
            return Err(too_big(path, limit));
        }
        Ok(bytes)
    }
    /// Append to a file, creating it (and its folders) if needed. Only for the device's own
    /// journal segments.
    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Make a new file with `bytes` that is then appended to, failing with
    /// [`io::ErrorKind::AlreadyExists`] if it exists: a check and create in one step, so
    /// two writers can never open the same segment.
    fn append_new(&self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Write a whole file once: written aside and renamed into place, so a reader never sees
    /// it half-written. Fails with [`io::ErrorKind::AlreadyExists`] if it exists.
    fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()>;
    fn remove(&self, path: &str) -> io::Result<()>;
    /// Make what was appended to `path` durable.
    fn flush(&self, path: &str) -> io::Result<()>;
    /// Remove what crashed writes left in `tmp/` (only files no live writer can still be
    /// using). Returns how many were removed.
    fn sweep_tmp(&self) -> io::Result<usize> {
        Ok(0)
    }
    /// Take the writer lock at `path` (a file in the device's journal folder), so two
    /// programs can't journal as one device into one library at once. `Ok(None)` if another
    /// program holds it. Libraries in one process share it (they share their stamps). The
    /// system releases it if the program dies, so it never goes stale.
    fn lock_writer(&self, _path: &str) -> io::Result<Option<WriterLock>> {
        Ok(Some(Box::new(())))
    }
}

impl<S: Store + Sync + ?Sized> Store for std::sync::Arc<S> {
    fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        (**self).list(dir)
    }
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        (**self).read(path)
    }
    fn read_limited(&self, path: &str, limit: u64) -> io::Result<Vec<u8>> {
        (**self).read_limited(path, limit)
    }
    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        (**self).append(path, bytes)
    }
    fn append_new(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        (**self).append_new(path, bytes)
    }
    fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        (**self).create(path, bytes)
    }
    fn remove(&self, path: &str) -> io::Result<()> {
        (**self).remove(path)
    }
    fn flush(&self, path: &str) -> io::Result<()> {
        (**self).flush(path)
    }
    fn sweep_tmp(&self) -> io::Result<usize> {
        (**self).sweep_tmp()
    }
    fn lock_writer(&self, path: &str) -> io::Result<Option<WriterLock>> {
        (**self).lock_writer(path)
    }
}

fn too_big(path: &str, limit: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{path} is over {limit} bytes"),
    )
}

/// A store in RAM. Writes can be made to fail, to test a full or yanked stick.
#[derive(Debug, Default)]
pub struct MemStore {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    fail_writes: AtomicBool,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn files(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Make every later write fail (true) or work again (false).
    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::Relaxed);
    }

    fn check_write(&self) -> io::Result<()> {
        if self.fail_writes.load(Ordering::Relaxed) {
            Err(io::Error::other("store is failing writes"))
        } else {
            Ok(())
        }
    }

    /// Every file and its bytes, for tests that copy or damage a library.
    pub fn dump(&self) -> BTreeMap<String, Vec<u8>> {
        self.files().clone()
    }

    /// Replace a file's bytes outright (tests: truncate or corrupt a file).
    pub fn put(&self, path: &str, bytes: Vec<u8>) {
        self.files().insert(path.to_owned(), bytes);
    }
}

impl Store for MemStore {
    fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        let files = self.files();
        let mut names: Vec<String> = files
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .filter_map(|(k, _)| k[prefix.len()..].split('/').next().map(str::to_owned))
            .collect();
        names.dedup();
        Ok(names)
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.files()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_owned()))
    }

    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        self.check_write()?;
        self.files()
            .entry(path.to_owned())
            .or_default()
            .extend_from_slice(bytes);
        Ok(())
    }

    fn append_new(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        self.create(path, bytes)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        self.check_write()?;
        let mut files = self.files();
        if files.contains_key(path) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                path.to_owned(),
            ));
        }
        files.insert(path.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.check_write()?;
        self.files()
            .remove(path)
            .map(drop)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_owned()))
    }

    fn flush(&self, _path: &str) -> io::Result<()> {
        self.check_write()
    }
}

/// A library folder on a file system (a stick's `DJPLAYER/`).
#[derive(Debug)]
pub struct FolderStore {
    root: PathBuf,
    tmp_seq: AtomicU64,
}

impl FolderStore {
    /// The folder need not exist yet; it is made on the first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            tmp_seq: AtomicU64::new(0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn full(&self, path: &str) -> io::Result<PathBuf> {
        let bad = path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..");
        if bad {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bad library path {path:?}"),
            ));
        }
        Ok(self.root.join(path))
    }

    /// Make `p`'s folder (and any missing above it, inside the root), syncing the folder
    /// entries of the ones it made.
    fn make_parent(&self, p: &Path) -> io::Result<()> {
        let Some(dir) = p.parent() else {
            return Ok(());
        };
        if dir.is_dir() {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        for d in dir.ancestors().skip(1) {
            sync_dir(d);
            if d == self.root || !d.starts_with(&self.root) {
                break;
            }
        }
        Ok(())
    }
}

/// Make a folder's entries (a rename or a new file in it) durable. Best effort: some
/// systems can't open a folder for syncing, and the data itself is already synced.
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Writer locks this process holds, by file: a second library in the same process shares
/// the first one's instead of being refused by its own process's lock.
static HELD: Mutex<BTreeMap<PathBuf, std::sync::Weak<std::fs::File>>> = Mutex::new(BTreeMap::new());

/// A file in `tmp/` this old can't belong to a write still in progress.
const STALE_TMP: std::time::Duration = std::time::Duration::from_secs(3600);

impl Store for FolderStore {
    fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let p = if dir.is_empty() {
            self.root.clone()
        } else {
            self.full(dir)?
        };
        let rd = match std::fs::read_dir(&p) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut names = Vec::new();
        for entry in rd {
            if let Some(name) = entry?.file_name().to_str() {
                // macOS litters FAT sticks with `._name` resource forks; never ours.
                if !name.starts_with("._") {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.full(path)?)
    }

    fn read_limited(&self, path: &str, limit: u64) -> io::Result<Vec<u8>> {
        use std::io::Read;
        let f = std::fs::File::open(self.full(path)?)?;
        if f.metadata()?.len() > limit {
            return Err(too_big(path, limit));
        }
        // The file may grow between the check and the read: never take more than allowed.
        let mut bytes = Vec::new();
        f.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(too_big(path, limit));
        }
        Ok(bytes)
    }

    fn append_new(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let p = self.full(path)?;
        self.make_parent(&p)?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)?;
        f.write_all(bytes)?;
        // The new file's folder entry, so a flushed segment is found after a power cut.
        if let Some(dir) = p.parent() {
            sync_dir(dir);
        }
        Ok(())
    }

    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let p = self.full(path)?;
        self.make_parent(&p)?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)?;
        f.write_all(bytes)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let p = self.full(path)?;
        if p.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                path.to_owned(),
            ));
        }
        self.make_parent(&p)?;
        let tmp_dir = self.root.join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let n = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let tmp = tmp_dir.join(format!("{}-{n}.tmp", std::process::id()));
        let result = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &p)?;
            // The rename itself must survive a power cut too.
            if let Some(dir) = p.parent() {
                sync_dir(dir);
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        std::fs::remove_file(self.full(path)?)
    }

    fn flush(&self, path: &str) -> io::Result<()> {
        std::fs::OpenOptions::new()
            .append(true)
            .open(self.full(path)?)?
            .sync_all()
    }

    fn lock_writer(&self, path: &str) -> io::Result<Option<WriterLock>> {
        let p = self.full(path)?;
        self.make_parent(&p)?;
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(f) = held.get(&key).and_then(std::sync::Weak::upgrade) {
            return Ok(Some(Box::new(f)));
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&p)?;
        match f.try_lock() {
            Ok(()) => {
                let f = std::sync::Arc::new(f);
                held.insert(key, std::sync::Arc::downgrade(&f));
                Ok(Some(Box::new(f)))
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    fn sweep_tmp(&self) -> io::Result<usize> {
        let dir = self.root.join("tmp");
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        for entry in rd {
            let entry = entry?;
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age >= STALE_TMP));
            if stale && entry.file_type().is_ok_and(|t| t.is_file()) {
                std::fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(s: &dyn Store) {
        assert!(s.list("journal").unwrap().is_empty());
        s.append("journal/a/00000001.log", b"one\n").unwrap();
        s.append("journal/a/00000001.log", b"two\n").unwrap();
        s.flush("journal/a/00000001.log").unwrap();
        s.append("journal/b/00000001.log", b"x").unwrap();
        let taken = s.append_new("journal/b/00000001.log", b"y").unwrap_err();
        assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
        s.append_new("journal/b/00000002.log", b"new").unwrap();
        s.append("journal/b/00000002.log", b"+").unwrap();
        assert_eq!(s.read("journal/b/00000002.log").unwrap(), b"new+");
        assert_eq!(
            s.read_limited("journal/b/00000002.log", 3)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            s.read_limited("journal/b/00000002.log", 4).unwrap(),
            b"new+"
        );
        assert_eq!(s.list("journal").unwrap(), ["a", "b"]);
        assert_eq!(s.read("journal/a/00000001.log").unwrap(), b"one\ntwo\n");
        s.create("blobs/ab/abcd", b"blob").unwrap();
        let again = s.create("blobs/ab/abcd", b"blob").unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(s.read("blobs/ab/abcd").unwrap(), b"blob");
        s.remove("blobs/ab/abcd").unwrap();
        assert!(s.read("blobs/ab/abcd").is_err());
    }

    #[test]
    fn mem_store_behaves_like_a_folder() {
        exercise(&MemStore::new());
    }

    #[test]
    fn folder_store_writes_through_tmp_and_rename() {
        let dir = std::env::temp_dir().join(format!("library-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = FolderStore::new(&dir);
        exercise(&s);
        // Nothing half-written is left behind.
        assert!(s.list("tmp").unwrap().is_empty());
        assert!(s.read("../escape").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
