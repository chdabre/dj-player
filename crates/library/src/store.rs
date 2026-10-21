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

pub trait Store: Send {
    /// Names of the entries directly in `dir` (files and folders), sorted. A missing folder
    /// is empty.
    fn list(&self, dir: &str) -> io::Result<Vec<String>>;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// Append to a file, creating it (and its folders) if needed. Only for the device's own
    /// journal segments.
    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Write a whole file once: written aside and renamed into place, so a reader never sees
    /// it half-written. Fails with [`io::ErrorKind::AlreadyExists`] if it exists.
    fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()>;
    fn remove(&self, path: &str) -> io::Result<()>;
    /// Make what was appended to `path` durable.
    fn flush(&self, path: &str) -> io::Result<()>;
}

impl<S: Store + Sync + ?Sized> Store for std::sync::Arc<S> {
    fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        (**self).list(dir)
    }
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        (**self).read(path)
    }
    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        (**self).append(path, bytes)
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

    fn make_parent(p: &Path) -> io::Result<()> {
        match p.parent() {
            Some(dir) => std::fs::create_dir_all(dir),
            None => Ok(()),
        }
    }
}

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

    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let p = self.full(path)?;
        Self::make_parent(&p)?;
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
        Self::make_parent(&p)?;
        let tmp_dir = self.root.join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let n = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let tmp = tmp_dir.join(format!("{}-{n}.tmp", std::process::id()));
        let result = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &p)
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
