//! Blobs: content-addressed files under `blobs/<aa>/<hash>`, or in RAM without a writable
//! folder.

use std::io::{self, Write};
use std::path::Path;

use crate::ids::BlobId;

use super::{Library, Mode};

impl Library {
    /// Store a blob under its BLAKE3 name, `blobs/<aa>/<hash>`, written once (tmp, sync,
    /// rename) before any row names it. Without a writable folder it stays in RAM.
    pub fn put_blob(&mut self, bytes: &[u8]) -> BlobId {
        let id = BlobId::of(bytes);
        self.store_blob(id, bytes.to_vec());
        id
    }

    pub(super) fn store_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
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

/// Write a blob unless it is there already: tmp file, sync, rename, sync the folder.
pub(super) fn write_blob(dir: &Path, id: BlobId, bytes: &[u8]) -> io::Result<()> {
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

pub(super) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
