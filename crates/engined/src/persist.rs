//! The library on a worker thread: mount, find or create the loaded track's record, write
//! edits and blobs back. Nothing here touches the audio thread; the deck gets the stored
//! cues through [`engine_core::DeckCommand::SetCues`] from [`crate::Player::poll`].

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use library::{
    AnalysisRef, AudioHash, ContentHasher, DeviceId, Edit, FileHash, FileIdentity, FolderStore,
    Library, MemStore, MountOptions, Persistence, Track, TrackRef, WaveformRef,
};
use waveform::compute::Waveform;

/// Small edits are flushed this long after the first unflushed one (the design allows 2 s).
const FLUSH_AFTER: Duration = Duration::from_secs(1);

/// The name of our library folder at the root of a stick.
pub const LIBRARY_DIR: &str = "DJPLAYER";

/// Which library the player mounts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LibraryChoice {
    /// `DJPLAYER/` at the root of the track's volume, if it can be written (or exists, then
    /// read-only); otherwise RAM only.
    #[default]
    Auto,
    /// This library folder (the `DJPLAYER/` folder itself), made if missing.
    Folder(PathBuf),
    /// No library: edits live in RAM for the session.
    Ram,
}

/// What a load found in the library.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub track: TrackRef,
    pub file_hash: FileHash,
    pub record: Track,
    /// The stored waveform, if there is one that decodes.
    pub waveform: Option<Waveform>,
    /// Stored analysis results by analyzer name, with their blobs.
    pub analysis: BTreeMap<String, (AnalysisRef, Vec<u8>)>,
}

/// A fresh analysis result to store.
#[derive(Debug, Clone)]
pub struct FreshAnalysis {
    pub name: String,
    pub version: String,
    pub params: String,
    pub blob: Vec<u8>,
}

enum Request {
    Edit(Edit),
    AudioHash(AudioHash),
    Waveform(Box<Waveform>),
    Analysis(Vec<FreshAnalysis>),
}

pub enum Reply {
    /// A line worth showing.
    Message(String),
    /// The track's record (first after mount; again when an audio hash merged it into
    /// another track).
    Loaded(Box<Loaded>),
    /// The library could not be mounted; nothing will be stored or restored.
    Failed(String),
}

/// The library worker of one loaded track.
pub struct LibraryWorker {
    tx: Option<Sender<Request>>,
    rx: Receiver<Reply>,
    thread: Option<JoinHandle<()>>,
}

impl LibraryWorker {
    /// Mount the chosen library and look up `file` on a new thread.
    pub fn spawn(choice: LibraryChoice, file: PathBuf) -> std::io::Result<Self> {
        Self::spawn_as(None, choice, file)
    }

    /// As [`LibraryWorker::spawn`], as `device` instead of this device's stored id.
    pub fn spawn_as(
        device: Option<DeviceId>,
        choice: LibraryChoice,
        file: PathBuf,
    ) -> std::io::Result<Self> {
        let (tx, requests) = std::sync::mpsc::channel();
        let (replies, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("library".into())
            .spawn(move || run(device, choice, &file, &requests, &replies))?;
        Ok(Self {
            tx: Some(tx),
            rx,
            thread: Some(thread),
        })
    }

    pub fn try_recv(&self) -> Option<Reply> {
        self.rx.try_recv().ok()
    }

    fn send(&self, r: Request) {
        if let Some(tx) = &self.tx {
            // A worker that died has already said why.
            let _ = tx.send(r);
        }
    }

    pub fn edit(&self, e: Edit) {
        self.send(Request::Edit(e));
    }

    pub fn audio_hash(&self, h: AudioHash) {
        self.send(Request::AudioHash(h));
    }

    pub fn waveform(&self, w: Waveform) {
        self.send(Request::Waveform(Box::new(w)));
    }

    pub fn analysis(&self, fresh: Vec<FreshAnalysis>) {
        if !fresh.is_empty() {
            self.send(Request::Analysis(fresh));
        }
    }
}

impl Drop for LibraryWorker {
    /// Hang up and wait for the worker's final flush.
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Where the library is and how paths in it are written.
struct Located {
    lib: Library,
    /// Paths of files are stored relative to this (the folder holding `DJPLAYER/`).
    root: Option<PathBuf>,
    what: String,
}

fn run(
    device: Option<DeviceId>,
    choice: LibraryChoice,
    file: &Path,
    requests: &Receiver<Request>,
    replies: &Sender<Reply>,
) {
    let say = |m: String| {
        let _ = replies.send(Reply::Message(m));
    };
    let device = device.unwrap_or_else(|| {
        let (device, warning) = device_id();
        if let Some(w) = warning {
            say(w);
        }
        device
    });
    let started = Instant::now();
    let mut loc = match locate(&choice, file, device) {
        Ok(l) => l,
        Err(e) => {
            let _ = replies.send(Reply::Failed(e));
            return;
        }
    };
    let stats = loc.lib.stats().clone();
    say(format!(
        "library: {} ({} tracks, {} ops from {} segments and {} snapshots in {:.0} ms)",
        loc.what,
        loc.lib.tracks().len(),
        stats.ops,
        stats.segments,
        stats.snapshots,
        started.elapsed().as_secs_f64() * 1e3,
    ));
    for w in loc.lib.take_warnings() {
        say(format!("library: {w}"));
    }

    let (identity, size) = match hash_file(file) {
        Ok(h) => h,
        Err(e) => {
            let _ = replies.send(Reply::Failed(format!("can't read {}: {e}", file.display())));
            return;
        }
    };
    let path = stored_path(file, loc.root.as_deref());
    let found = loc
        .lib
        .find_or_create(FileIdentity::new(path, size, identity));
    let mut track = found.track;
    if found.created {
        say(format!("library: new track {track}"));
    } else if found.new_location {
        say(format!(
            "library: track {track} (known file at a new location)"
        ));
    }
    let _ = replies.send(Reply::Loaded(Box::new(loaded(
        &mut loc.lib,
        track,
        identity,
    ))));

    let mut deadline: Option<Instant> = None;
    loop {
        let req = match deadline {
            Some(d) => match requests.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(r) => Some(r),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match requests.recv() {
                Ok(r) => Some(r),
                Err(_) => break,
            },
        };
        let Some(req) = req else {
            loc.lib.flush();
            deadline = None;
            report(&mut loc.lib, &say);
            continue;
        };
        match req {
            Request::Edit(e) => {
                if let Err(e) = loc.lib.apply(e) {
                    say(format!("library: {e}"));
                }
                deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
            }
            Request::AudioHash(h) => {
                let survivor = loc.lib.set_audio_hash(track, identity, h);
                deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                if survivor != track {
                    say(format!(
                        "library: same audio as track {survivor}: merged into it"
                    ));
                    track = survivor;
                    let _ = replies.send(Reply::Loaded(Box::new(loaded(
                        &mut loc.lib,
                        track,
                        identity,
                    ))));
                }
            }
            Request::Waveform(w) => {
                let detail = w.to_blob();
                let overview = w.overview().to_blob(&w.params, w.provenance);
                let r = WaveformRef {
                    blob: loc.lib.put_blob(&detail),
                    overview_blob: loc.lib.put_blob(&overview),
                };
                if let Err(e) = loc.lib.apply(Edit::SetWaveform(track, r)) {
                    say(format!("library: {e}"));
                }
                loc.lib.flush();
            }
            Request::Analysis(fresh) => {
                for f in fresh {
                    let a = AnalysisRef {
                        version: f.version,
                        params: f.params,
                        blob: loc.lib.put_blob(&f.blob),
                        created: library::rfc3339(library::system_ms()),
                    };
                    if let Err(e) = loc.lib.apply(Edit::SetAnalysis(track, f.name, a)) {
                        say(format!("library: {e}"));
                    }
                }
                loc.lib.flush();
            }
        }
        report(&mut loc.lib, &say);
    }
    loc.lib.flush();
    report(&mut loc.lib, &say);
}

fn report(lib: &mut Library, say: &dyn Fn(String)) {
    for w in lib.take_warnings() {
        say(format!("library: {w}"));
    }
}

/// The record of `track`, with its stored waveform and analysis blobs read.
fn loaded(lib: &mut Library, track: library::TrackId, file_hash: FileHash) -> Loaded {
    let record = lib.track(track).unwrap_or_else(|| empty_track(track));
    let waveform = record
        .waveform
        .and_then(|w| lib.read_blob(w.blob).ok())
        .and_then(|b| Waveform::from_blob(&b).ok());
    let analysis = record
        .analysis
        .iter()
        .filter_map(|(name, a)| {
            let blob = lib.read_blob(a.blob).ok()?;
            Some((name.clone(), (a.clone(), blob)))
        })
        .collect();
    Loaded {
        track: lib.track_ref(record.id),
        file_hash,
        record,
        waveform,
        analysis,
    }
}

fn empty_track(id: library::TrackId) -> Track {
    Track {
        id,
        locations: Vec::new(),
        rating: None,
        cues: Vec::new(),
        hot_cues: Default::default(),
        analysis: BTreeMap::new(),
        waveform: None,
        merged: Vec::new(),
    }
}

/// Dropbox content hash and size of a file, read in 4 MiB blocks.
fn hash_file(path: &Path) -> std::io::Result<(FileHash, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = ContentHasher::new();
    let mut buf = vec![0u8; library::hash::BLOCK];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok((h.finish(), size))
}

/// `file` relative to `root`, `/`-separated and NFC; a file outside the root keeps its
/// absolute path (a hint only: identity is the hash).
fn stored_path(file: &Path, root: Option<&Path>) -> String {
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = root
        .and_then(|r| r.canonicalize().ok())
        .and_then(|r| abs.strip_prefix(r).ok().map(Path::to_path_buf));
    library::normalize_path(&rel.unwrap_or(abs).to_string_lossy())
}

/// True if a file can be made and removed in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".write-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The mount point holding `file`: the highest ancestor on the same device.
fn volume_root(file: &Path) -> PathBuf {
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        let mut cur = abs.parent().unwrap_or(&abs).to_path_buf();
        while let Some(parent) = cur.parent() {
            if dev(parent).is_none() || dev(parent) != dev(&cur) {
                break;
            }
            cur = parent.to_path_buf();
        }
        cur
    }
    #[cfg(not(unix))]
    {
        abs.ancestors().last().unwrap_or(&abs).to_path_buf()
    }
}

fn mount_options(device: DeviceId, read_only: bool) -> MountOptions {
    MountOptions {
        read_only,
        device_info: serde_json::json!({ "name": "engined", "model": "dev" }),
        ..MountOptions::new(device)
    }
}

fn mount_folder(dir: &Path, device: DeviceId, read_only: bool) -> Result<Library, String> {
    Library::mount(FolderStore::new(dir), mount_options(device, read_only))
        .map_err(|e| format!("{}: {e}", dir.display()))
}

fn ram(device: DeviceId, why: String) -> Located {
    let lib = Library::mount(MemStore::new(), mount_options(device, false))
        .expect("an empty RAM library always mounts");
    Located {
        lib,
        root: None,
        what: format!("RAM only, edits are not saved ({why})"),
    }
}

fn locate(choice: &LibraryChoice, file: &Path, device: DeviceId) -> Result<Located, String> {
    let folder = |dir: &Path| -> Result<Located, String> {
        let _ = std::fs::create_dir_all(dir);
        let read_only = !writable(dir);
        let lib = mount_folder(dir, device, read_only)?;
        let what = if read_only {
            format!("{} (read-only: edits stay in RAM, unsaved)", dir.display())
        } else {
            dir.display().to_string()
        };
        Ok(Located {
            lib,
            root: dir.parent().map(Path::to_path_buf),
            what,
        })
    };
    match choice {
        LibraryChoice::Ram => Ok(ram(device, "--no-library".into())),
        LibraryChoice::Folder(dir) => folder(dir),
        LibraryChoice::Auto => {
            let root = volume_root(file);
            let dir = root.join(LIBRARY_DIR);
            if dir.is_dir() || (writable(&root) && std::fs::create_dir(&dir).is_ok()) {
                folder(&dir)
            } else {
                Ok(ram(
                    device,
                    format!("{} is not writable; use --library <dir>", root.display()),
                ))
            }
        }
    }
}

/// This device's id, kept next to the player's settings on its own storage
/// (`$DJPLAYER_CONFIG`, else `$XDG_CONFIG_HOME/djplayer`, else `~/.config/djplayer`), never
/// on a stick. Made on first use. Returns a warning if it could not be kept.
pub fn device_id() -> (DeviceId, Option<String>) {
    let dir = std::env::var_os("DJPLAYER_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(|d| PathBuf::from(d).join("djplayer")))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("djplayer"))
        });
    let Some(dir) = dir else {
        return (
            DeviceId::random(),
            Some("no config folder: a new device id for this run".into()),
        );
    };
    let path = dir.join("device-id");
    if let Some(id) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
    {
        return (id, None);
    }
    let id = DeviceId::random();
    let kept =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, format!("{id}\n")));
    let warning = kept.err().map(|e| {
        format!(
            "can't keep the device id in {} ({e}): a new one for this run",
            path.display()
        )
    });
    (id, warning)
}

/// How the library stands, for a status line.
pub fn describe(p: &Persistence) -> String {
    match p {
        Persistence::Saved => "saved".into(),
        Persistence::Pending => "saving".into(),
        Persistence::Unsaved { edits, reason } => format!("{edits} unsaved edits ({reason})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_stored_relative_to_the_library_root() {
        let dir = std::env::temp_dir().join(format!("engined-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Music")).unwrap();
        let f = dir.join("Music").join("a.mp3");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(stored_path(&f, Some(&dir)), "Music/a.mp3");
        let outside = stored_path(&f, Some(&dir.join("elsewhere")));
        assert!(outside.ends_with("/Music/a.mp3") && outside.starts_with('/'));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_volume_root_is_an_ancestor() {
        let here = std::env::current_dir().unwrap();
        let root = volume_root(&here.join("Cargo.toml"));
        assert!(
            here.starts_with(&root),
            "{} {}",
            root.display(),
            here.display()
        );
    }
}
