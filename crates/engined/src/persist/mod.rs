//! The library on a worker thread: mount, find or create the loaded track's record, write
//! edits and blobs back. Nothing here touches the audio thread; the deck gets the stored
//! cues through [`engine_core::DeckCommand::SetCues`] from [`crate::Player::poll`].

mod locate;
mod records;
mod worker;

pub use crate::config::device_id;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use library::{
    AnalysisRef, AudioHash, DeviceId, Edit, Listed, Persistence, Playlist, PlaylistEntry,
    PlaylistId, Track, TrackRef,
};
use waveform::compute::Waveform;

pub use library::{LIBRARY_DIR, stored_path};

/// True if the stick (or folder) at `root` already has our library.
pub fn has_library(root: &Path) -> bool {
    library::has_library(&root.join(LIBRARY_DIR))
}

/// Which library the player mounts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LibraryChoice {
    /// `DJPLAYER/` at the root of the track's volume if that is removable (a USB stick or SD
    /// card) and can be written (or the folder exists, then read-only); otherwise RAM only.
    /// Never internal storage: an internal-storage library is given as a
    /// [`LibraryChoice::Folder`].
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
    /// The [`LibraryWorker::open`] this answers.
    pub serial: u64,
    pub track: TrackRef,
    pub record: Track,
    /// The stored waveform, if there is one that decodes.
    pub waveform: Option<Waveform>,
    /// Stored analysis results by analyzer name, with their blobs.
    pub analysis: BTreeMap<String, (AnalysisRef, Vec<u8>)>,
}

/// The library's tracks by path (relative to the stick's root, as stored).
pub type Listing = HashMap<String, Listed>;

/// The playlists of a library, as the browser shows them.
#[derive(Debug, Clone, Default)]
pub struct Playlists {
    /// The tag list first, then the tree depth first (see [`library::Library::playlists`]).
    pub lists: Vec<Playlist>,
    /// Each list's entries, in order.
    pub entries: HashMap<PlaylistId, Vec<PlaylistEntry>>,
}

impl Playlists {
    pub fn get(&self, id: PlaylistId) -> Option<&Playlist> {
        self.lists.iter().find(|p| p.id == id)
    }

    /// The tracks on the tag list.
    pub fn tagged(&self) -> impl Iterator<Item = library::TrackId> + '_ {
        self.entries
            .get(&PlaylistId::TAG_LIST)
            .into_iter()
            .flatten()
            .map(|e| e.track)
    }
}

/// What the batch job computed for one file, to store with its record (made if missing).
#[derive(Debug, Clone)]
pub struct BatchResult {
    pub file: PathBuf,
    pub waveform: Waveform,
    pub fresh: Vec<FreshAnalysis>,
    pub audio_hash: Option<AudioHash>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
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
    Open {
        serial: u64,
        file: PathBuf,
    },
    Store(Box<BatchResult>),
    /// Files into a playlist, in order, records made where missing.
    AddFiles {
        playlist: PlaylistId,
        files: Vec<PathBuf>,
    },
    /// A new list made of files (MAKE PLAYLIST on a folder).
    NewPlaylist {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    },
    Scan,
    Analysed {
        bpm: Option<f64>,
        key: Option<String>,
    },
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
    /// The opened file got no record (it can't be read); its edits are not stored.
    Unrecorded { serial: u64, reason: String },
    /// Whether edits reach the store, whenever it changes.
    Status(Persistence),
    /// Every track the library knows, by path relative to the stick's root: after the mount,
    /// and again when tracks or their tags change.
    Listing(Arc<Listing>),
    /// A scan's progress, files done of `total`.
    Scan { done: usize, total: usize },
    /// The playlists and their entries: after the mount and after every playlist change.
    Playlists(Arc<Playlists>),
}

/// The library worker: one mounted library, and the track the deck has loaded in it.
pub struct LibraryWorker {
    tx: Option<Sender<Request>>,
    rx: Receiver<Reply>,
    thread: Option<JoinHandle<()>>,
}

impl LibraryWorker {
    /// Start a worker for the chosen library; it mounts on the first [`LibraryWorker::open`].
    pub fn start(device: Option<DeviceId>, choice: LibraryChoice) -> std::io::Result<Self> {
        let (tx, requests) = std::sync::mpsc::channel();
        let (replies, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("library".into())
            .spawn(move || worker::run(device, choice, &requests, &replies))?;
        Ok(Self {
            tx: Some(tx),
            rx,
            thread: Some(thread),
        })
    }

    /// Mount the chosen library and look up `file` (serial 0) on a new thread, as `device`
    /// (`None`: this device's stored id).
    pub fn spawn_as(
        device: Option<DeviceId>,
        choice: LibraryChoice,
        file: PathBuf,
    ) -> std::io::Result<Self> {
        let w = Self::start(device, choice)?;
        w.open(0, file);
        Ok(w)
    }

    /// The deck loaded `file`: find or make its record, answered by a [`Reply::Loaded`] (or
    /// [`Reply::Unrecorded`]) with this `serial`. Audio hash, waveform and analysis requests
    /// from now on are for this track.
    pub fn open(&self, serial: u64, file: PathBuf) {
        self.send(Request::Open { serial, file });
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

    /// Scan every audio file under the stick's root: records and tags, a file at a time
    /// between other requests, reported as [`Reply::Scan`].
    pub fn scan(&self) {
        self.send(Request::Scan);
    }

    /// Add `files` at the end of `playlist`, in order, between other requests (a folder of
    /// hundreds of files doesn't hold up a load); files without a record get one. Playlist
    /// edits go through [`LibraryWorker::edit`]; the playlists are published after either.
    pub fn add_files(&self, playlist: PlaylistId, files: Vec<PathBuf>) {
        self.send(Request::AddFiles { playlist, files });
    }

    /// MAKE PLAYLIST: a new list `id` at the end of folder `parent`, of `files` in order.
    pub fn new_playlist(
        &self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    ) {
        self.send(Request::NewPlaylist {
            id,
            parent,
            name,
            files,
        });
    }

    /// Store a batch result with its file's record; the loaded track stays the one requests
    /// without a file go to.
    pub fn store(&self, result: BatchResult) {
        self.send(Request::Store(Box::new(result)));
    }

    /// The loaded track's analysed BPM and key, for the tag cache.
    pub fn analysed(&self, bpm: Option<f64>, key: Option<String>) {
        self.send(Request::Analysed { bpm, key });
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
