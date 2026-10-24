//! Files still arriving: a download being written while the deck already decodes and the
//! analysis already reads it.
//!
//! [`arriving`] says that a file will be at its path once complete, is being written to
//! another (`part`) meanwhile, and how long it will be. Opening it then gives a reader that
//! knows the final length: it reads what is there, and on a thread that may wait (the loader,
//! the analysis) waits for the rest, until [`arrived`] says the download ended. The decode
//! and the analysis so run alongside the download, and finish with it. On a thread that must
//! not wait ([`without_waiting`]: the caller of a load, probing the file) a read past what has
//! arrived fails at once, so a load that comes too early fails rather than blocking; only
//! the file's last kilobyte, where a probe looks for trailing tags, reads as zeros instead.
//!
//! The reader keeps the part file open: renaming it to the final path on completion does not
//! disturb it.

use std::cell::Cell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use symphonia::core::io::MediaSource;

/// How long a waiting read sleeps before it looks again.
const WAIT: Duration = Duration::from_millis(25);

/// The end of a file that a probe may read for trailing tags before it has arrived.
const TAIL: u64 = 1024;

/// The arrival is under way, complete, or stopped (failed, cancelled, or no longer wanted).
const ARRIVING: u8 = 0;
const COMPLETE: u8 = 1;
const STOPPED: u8 = 2;

/// A file on its way.
#[derive(Debug)]
struct Arrival {
    part: PathBuf,
    len: u64,
    state: AtomicU8,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<Arrival>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Arrival>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn lookup(file: &Path) -> Option<Arc<Arrival>> {
    registry().lock().ok()?.get(file).cloned()
}

/// `file` is arriving: it is written to `part` until complete, `len` bytes in all, and then
/// is at `file`.
pub fn arriving(file: &Path, part: &Path, len: u64) {
    if let Ok(mut r) = registry().lock() {
        let arrival = Arrival {
            part: part.to_path_buf(),
            len,
            state: AtomicU8::new(ARRIVING),
        };
        if let Some(old) = r.insert(file.to_path_buf(), Arc::new(arrival)) {
            old.state.store(STOPPED, Ordering::Relaxed);
        }
    }
}

/// `file`'s arrival ended: it is complete at its path, or it stopped. Readers waiting for
/// the rest end (with what there is, or an error).
pub fn arrived(file: &Path, complete: bool) {
    let Some(a) = registry().lock().ok().and_then(|mut r| r.remove(file)) else {
        return;
    };
    a.state
        .store(if complete { COMPLETE } else { STOPPED }, Ordering::Relaxed);
}

/// Whether `file` is still arriving.
pub fn is_arriving(file: &Path) -> bool {
    lookup(file).is_some()
}

thread_local! {
    static NO_WAIT: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with reads of arriving files failing ([`io::ErrorKind::WouldBlock`]) instead of
/// waiting for bytes that have not arrived.
pub(crate) fn without_waiting<T>(f: impl FnOnce() -> T) -> T {
    let before = NO_WAIT.with(|w| w.replace(true));
    let out = f();
    NO_WAIT.with(|w| w.set(before));
    out
}

/// A file as decoding reads it: on disk, or arriving.
pub(crate) enum MediaFile {
    Disk(File),
    Arriving(ArrivingFile),
}

impl MediaFile {
    /// Open `path`, as an arriving file if it is one (and its part file is still there).
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        if let Some(arrival) = lookup(path)
            && let Ok(file) = File::open(&arrival.part)
        {
            return Ok(MediaFile::Arriving(ArrivingFile {
                file,
                arrival,
                pos: 0,
            }));
        }
        Ok(MediaFile::Disk(File::open(path)?))
    }

    /// The file's length: once complete, for an arriving one.
    pub(crate) fn len(&self) -> io::Result<u64> {
        match self {
            MediaFile::Disk(f) => Ok(f.metadata()?.len()),
            MediaFile::Arriving(a) => Ok(a.arrival.len),
        }
    }
}

impl Read for MediaFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MediaFile::Disk(f) => f.read(buf),
            MediaFile::Arriving(a) => a.read(buf),
        }
    }
}

impl Seek for MediaFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            MediaFile::Disk(f) => f.seek(pos),
            MediaFile::Arriving(a) => a.seek(pos),
        }
    }
}

impl MediaSource for MediaFile {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.len().ok()
    }
}

/// An arriving file's reader: what has been written to its part file, and the rest as it
/// comes.
pub(crate) struct ArrivingFile {
    file: File,
    arrival: Arc<Arrival>,
    pos: u64,
}

impl Read for ArrivingFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.arrival.len;
        loop {
            if self.pos >= len || buf.is_empty() {
                return Ok(0);
            }
            let have = self.file.metadata()?.len().min(len);
            if self.pos < have {
                let n = usize::try_from(have - self.pos).unwrap_or(usize::MAX);
                let n = n.min(buf.len());
                self.file.seek(SeekFrom::Start(self.pos))?;
                let read = self.file.read(&mut buf[..n])?;
                self.pos += read as u64;
                return Ok(read);
            }
            match self.arrival.state.load(Ordering::Relaxed) {
                STOPPED => return Err(io::Error::other("the download stopped")),
                // Complete yet shorter than said: what is there is all there is.
                COMPLETE => return Ok(0),
                _ => {}
            }
            if NO_WAIT.with(Cell::get) {
                // The probe looks for tags at the very end (ID3v1, APE). They have not
                // arrived: it finds none, and the decode reads the real bytes later.
                if self.pos >= len.saturating_sub(TAIL) {
                    let n = usize::try_from(len - self.pos).unwrap_or(usize::MAX);
                    let n = n.min(buf.len());
                    buf[..n].fill(0);
                    self.pos += n as u64;
                    return Ok(n);
                }
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "not downloaded that far yet",
                ));
            }
            std::thread::sleep(WAIT);
        }
    }
}

impl Seek for ArrivingFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.pos = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::End(d) => self.arrival.len.saturating_add_signed(d),
            SeekFrom::Current(d) => self.pos.saturating_add_signed(d),
        };
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{tmp, write_wav};
    use std::io::Write;

    /// A WAV of `frames` frames, written to `part` a slice at a time by a thread while `file`
    /// is arriving; the thread finishes the arrival (renaming `part` to `file`) unless
    /// `stop_at` stops it there.
    fn trickle(
        name: &str,
        frames: usize,
        stop_at: Option<usize>,
    ) -> (PathBuf, std::thread::JoinHandle<()>) {
        let src = tmp(&format!("{name}-src.wav"));
        write_wav(&src, 48_000, 2, &vec![0.25; 2 * frames]);
        let bytes = std::fs::read(&src).unwrap();
        let file = tmp(&format!("{name}.wav"));
        let part = tmp(&format!("{name}.wav.part"));
        let _ = std::fs::remove_file(&file);
        let mut out = File::create(&part).unwrap();
        // The header is there before the load, as a download's first bytes would be.
        out.write_all(&bytes[..4096]).unwrap();
        arriving(&file, &part, bytes.len() as u64);
        let (f, p) = (file.clone(), part.clone());
        let writer = std::thread::spawn(move || {
            for (i, chunk) in bytes[4096..].chunks(64 * 1024).enumerate() {
                if stop_at == Some(i) {
                    arrived(&f, false);
                    return;
                }
                out.write_all(chunk).unwrap();
                out.flush().unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
            std::fs::rename(&p, &f).unwrap();
            arrived(&f, true);
        });
        (file, writer)
    }

    #[test]
    fn a_decode_waits_for_the_rest_and_ends_with_the_download() {
        let frames = 4 * 48_000;
        let (file, writer) = trickle("arriving-whole", frames, None);
        assert!(is_arriving(&file));
        let (audio, rate) = crate::decode_native(&file).unwrap();
        writer.join().unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(
            audio.len(),
            2 * frames,
            "every frame, though most came later"
        );
        assert!(!is_arriving(&file));
    }

    #[test]
    fn a_stopped_download_ends_the_decode_with_an_error() {
        let (file, writer) = trickle("arriving-stopped", 4 * 48_000, Some(3));
        let r = crate::decode_native(&file);
        writer.join().unwrap();
        assert!(r.is_err(), "{r:?}");
    }

    #[test]
    fn without_waiting_a_read_past_what_arrived_fails_at_once() {
        let file = tmp("arriving-early.wav");
        let part = tmp("arriving-early.wav.part");
        std::fs::write(&part, b"RIFF").unwrap();
        arriving(&file, &part, 1_000_000);
        let mut f = MediaFile::open(&file).unwrap();
        assert_eq!(f.len().unwrap(), 1_000_000, "the final length");
        let mut buf = [0u8; 16];
        assert_eq!(f.read(&mut buf).unwrap(), 4, "what is there");
        let e = without_waiting(|| f.read(&mut buf)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        arrived(&file, false);
        assert!(f.read(&mut buf).is_err(), "stopped");
    }
}
