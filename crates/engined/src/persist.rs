//! The library on a worker thread: mount, find or create the loaded track's record, write
//! edits and blobs back. Nothing here touches the audio thread; the deck gets the stored
//! cues through [`engine_core::DeckCommand::SetCues`] from [`crate::Player::poll`].

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use library::{
    AnalysisRef, AudioHash, ContentHasher, DeviceId, Edit, FileHash, FileIdentity, Library, Listed,
    MountOptions, Persistence, Playlist, PlaylistEntry, PlaylistId, Track, TrackRef, TrackTags,
    WaveformRef,
};
use waveform::compute::Waveform;

/// Edits are committed this long after the first uncommitted one.
const FLUSH_AFTER: Duration = Duration::from_secs(1);

/// The name of our library folder at the root of a stick.
pub const LIBRARY_DIR: &str = "DJPLAYER";

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
    pub file_hash: FileHash,
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
            .spawn(move || run(device, choice, &requests, &replies))?;
        Ok(Self {
            tx: Some(tx),
            rx,
            thread: Some(thread),
        })
    }

    /// Mount the chosen library and look up `file` (serial 0) on a new thread.
    pub fn spawn(choice: LibraryChoice, file: PathBuf) -> std::io::Result<Self> {
        Self::spawn_as(None, choice, file)
    }

    /// As [`LibraryWorker::spawn`], as `device` instead of this device's stored id.
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

/// Where the library is and how paths in it are written.
struct Located {
    lib: Library,
    /// Paths of files are stored relative to this (the folder holding `DJPLAYER/`).
    root: Option<PathBuf>,
    what: String,
}

/// The track requests apply to: the last one opened.
struct Current {
    track: library::TrackId,
    identity: FileHash,
    serial: u64,
}

/// A scan of the stick under way: the files still to do.
struct Scan {
    files: Vec<PathBuf>,
    total: usize,
    /// When progress was last reported.
    reported: Instant,
}

/// How often a running scan reports progress and publishes the listing.
const SCAN_REPORT: Duration = Duration::from_millis(500);

fn run(
    device: Option<DeviceId>,
    choice: LibraryChoice,
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
    let mut status = Status {
        replies: replies.clone(),
        last: None,
    };
    // A chosen folder (or RAM) mounts now, so the browser can list it before anything loads;
    // the automatic choice waits for the first open: it needs a file to find its stick.
    let mut loc: Option<Located> = None;
    if choice != LibraryChoice::Auto {
        match mount(&choice, Path::new(""), device, &say) {
            Ok(mut l) => {
                report(&mut l.lib, &say, &mut status);
                publish(&l.lib, replies);
                loc = Some(l);
            }
            Err(e) => {
                let _ = replies.send(Reply::Failed(e));
                return;
            }
        }
    }
    let mut current: Option<Current> = None;
    let mut scan: Option<Scan> = None;
    // Files still to add to playlists, in order, and when the playlists were last sent.
    let mut adding: std::collections::VecDeque<(PlaylistId, PathBuf)> = Default::default();
    let mut adding_sent = Instant::now();
    // The listing changed since it was last published.
    let mut listing_stale = false;
    let mut deadline: Option<Instant> = None;
    loop {
        // While scanning, look for requests without waiting; a request always comes first.
        let wait = match (&scan, deadline) {
            _ if !adding.is_empty() => Some(Duration::ZERO),
            (Some(_), _) => Some(Duration::ZERO),
            (None, Some(d)) => Some(d.saturating_duration_since(Instant::now())),
            (None, None) => None,
        };
        let req = match wait {
            Some(w) => match requests.recv_timeout(w) {
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
            if let Some(l) = loc.as_mut() {
                if let Some((playlist, file)) = adding.pop_front() {
                    match open(l, &file, &|_| {}) {
                        Ok((track, _, _)) => {
                            refresh_tags(&mut l.lib, track, &file, &say);
                            let add = Edit::AddEntries {
                                playlist,
                                tracks: vec![track],
                                before: None,
                            };
                            if let Err(e) = l.lib.apply(add) {
                                say(format!("library: {e}"));
                            }
                        }
                        Err(e) => say(format!("library: {e}")),
                    }
                    listing_stale = true;
                    deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                    if adding.is_empty() || adding_sent.elapsed() >= SCAN_REPORT {
                        adding_sent = Instant::now();
                        publish_playlists(&l.lib, replies);
                    }
                    continue;
                }
                if let Some(sc) = scan.as_mut() {
                    scan_one(l, sc, &say);
                    listing_stale = true;
                    deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                    let finished = sc.files.is_empty();
                    if finished || sc.reported.elapsed() >= SCAN_REPORT {
                        sc.reported = Instant::now();
                        let _ = replies.send(Reply::Scan {
                            done: sc.total - sc.files.len(),
                            total: sc.total,
                        });
                        publish(&l.lib, replies);
                        listing_stale = false;
                    }
                    if finished {
                        say(format!("library: scan done, {} files", sc.total));
                        scan = None;
                        l.lib.flush();
                        report(&mut l.lib, &say, &mut status);
                    }
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    l.lib.flush();
                    check(&mut l.lib, &say);
                    report(&mut l.lib, &say, &mut status);
                    deadline = None;
                    if listing_stale {
                        publish(&l.lib, replies);
                        listing_stale = false;
                    }
                }
            } else {
                deadline = None;
            }
            continue;
        };
        if let Request::Open { serial, file } = req {
            if loc.is_none() {
                match mount(&choice, &file, device, &say) {
                    Ok(l) => {
                        publish(&l.lib, replies);
                        loc = Some(l);
                    }
                    Err(e) => {
                        let _ = replies.send(Reply::Failed(e));
                        return;
                    }
                }
            }
            let loc = loc.as_mut().expect("just mounted");
            current = None;
            match open(loc, &file, &say) {
                Ok((track, identity, new_location)) => {
                    let tags_changed = refresh_tags(&mut loc.lib, track, &file, &say);
                    let _ = replies.send(Reply::Loaded(Box::new(loaded(
                        &mut loc.lib,
                        track,
                        identity,
                        serial,
                    ))));
                    current = Some(Current {
                        track,
                        identity,
                        serial,
                    });
                    report(&mut loc.lib, &say, &mut status);
                    // After an unclean close: the record is out, so the check doesn't hold
                    // up the load.
                    check(&mut loc.lib, &say);
                    // A new record or location is committed like an edit.
                    if new_location || tags_changed {
                        listing_stale = true;
                        deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                    }
                }
                Err(reason) => {
                    let _ = replies.send(Reply::Unrecorded { serial, reason });
                }
            }
            continue;
        }
        let Some(loc) = loc.as_mut() else {
            continue;
        };
        match req {
            Request::Open { .. } => unreachable!("handled above"),
            Request::Scan => {
                if scan.is_none() {
                    let files = loc
                        .root
                        .as_deref()
                        .map(crate::browse::walk)
                        .unwrap_or_default();
                    say(format!("library: scanning {} files", files.len()));
                    let _ = replies.send(Reply::Scan {
                        done: 0,
                        total: files.len(),
                    });
                    scan = Some(Scan {
                        total: files.len(),
                        // Taken from the end: reverse, so the scan goes in path order.
                        files: files.into_iter().rev().collect(),
                        reported: Instant::now(),
                    });
                }
            }
            Request::Store(r) => match open(loc, &r.file, &say) {
                Ok((mut track, _, _)) => {
                    refresh_tags(&mut loc.lib, track, &r.file, &say);
                    if let Some(h) = r.audio_hash {
                        match loc.lib.set_audio_hash(track, h) {
                            Ok(survivor) => track = survivor,
                            Err(e) => say(format!("library: {e}")),
                        }
                    }
                    store_analysis(&mut loc.lib, track, r.fresh, &say);
                    store_waveform(&mut loc.lib, track, &r.waveform, &say);
                    store_bpm_key(&mut loc.lib, track, r.bpm, r.key, &say);
                    loc.lib.flush();
                    listing_stale = true;
                    // Publishes the listing within the second.
                    deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                }
                Err(e) => say(format!("library: {e}")),
            },
            Request::Edit(e) => {
                let playlist = is_playlist_edit(&e);
                if let Err(e) = loc.lib.apply(e) {
                    say(format!("library: {e}"));
                }
                if playlist {
                    publish_playlists(&loc.lib, replies);
                }
                deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
            }
            Request::AddFiles { playlist, files } => {
                adding.extend(files.into_iter().map(|f| (playlist, f)));
                adding_sent = Instant::now();
            }
            Request::NewPlaylist {
                id,
                parent,
                name,
                files,
            } => {
                let make = Edit::CreatePlaylist {
                    id,
                    parent,
                    kind: library::PlaylistKind::List,
                    name,
                };
                match loc.lib.apply(make) {
                    Ok(_) => {
                        publish_playlists(&loc.lib, replies);
                        adding.extend(files.into_iter().map(|f| (id, f)));
                        adding_sent = Instant::now();
                    }
                    Err(e) => say(format!("library: {e}")),
                }
                deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
            }
            Request::Analysed { bpm, key } => {
                let Some(cur) = &current else {
                    continue;
                };
                if store_bpm_key(&mut loc.lib, cur.track, bpm, key, &say) {
                    listing_stale = true;
                    deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                }
            }
            Request::AudioHash(h) => {
                let Some(cur) = current.as_mut() else {
                    continue;
                };
                let survivor = match loc.lib.set_audio_hash(cur.track, h) {
                    Ok(s) => s,
                    Err(e) => {
                        say(format!("library: {e}"));
                        cur.track
                    }
                };
                deadline.get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
                if survivor != cur.track {
                    say(format!(
                        "library: same audio as track {survivor}: merged into it"
                    ));
                    cur.track = survivor;
                    listing_stale = true;
                    let _ = replies.send(Reply::Loaded(Box::new(loaded(
                        &mut loc.lib,
                        cur.track,
                        cur.identity,
                        cur.serial,
                    ))));
                }
            }
            Request::Waveform(w) => {
                let Some(cur) = &current else {
                    continue;
                };
                store_waveform(&mut loc.lib, cur.track, &w, &say);
                loc.lib.flush();
            }
            Request::Analysis(fresh) => {
                let Some(cur) = &current else {
                    continue;
                };
                store_analysis(&mut loc.lib, cur.track, fresh, &say);
                loc.lib.flush();
            }
        }
        report(&mut loc.lib, &say, &mut status);
    }
    if let Some(loc) = loc.as_mut() {
        loc.lib.flush();
        report(&mut loc.lib, &say, &mut status);
    }
}

fn store_waveform(lib: &mut Library, track: library::TrackId, w: &Waveform, say: &dyn Fn(String)) {
    let detail = w.to_blob();
    let overview = w.overview().to_blob(&w.params, w.provenance);
    let r = WaveformRef::new(lib.put_blob(&detail), lib.put_blob(&overview));
    if let Err(e) = lib.apply(Edit::SetWaveform(track, r)) {
        say(format!("library: {e}"));
    }
}

fn store_analysis(
    lib: &mut Library,
    track: library::TrackId,
    fresh: Vec<FreshAnalysis>,
    say: &dyn Fn(String),
) {
    for f in fresh {
        let a = AnalysisRef {
            version: f.version,
            params: f.params,
            blob: lib.put_blob(&f.blob),
            created: library::rfc3339(library::system_ms()),
        };
        if let Err(e) = lib.apply(Edit::SetAnalysis(track, f.name, a)) {
            say(format!("library: {e}"));
        }
    }
}

/// Put our analysed BPM and key in the track's tag cache; true if it changed.
fn store_bpm_key(
    lib: &mut Library,
    track: library::TrackId,
    bpm: Option<f64>,
    key: Option<String>,
    say: &dyn Fn(String),
) -> bool {
    let Ok(Some(t)) = lib.track(track) else {
        return false;
    };
    let tags = TrackTags {
        bpm: bpm.or(t.tags.bpm),
        key: key.or(t.tags.key.clone()),
        ..t.tags.clone()
    };
    if tags == t.tags {
        return false;
    }
    if let Err(e) = lib.apply(Edit::SetTags(track, tags)) {
        say(format!("library: {e}"));
        return false;
    }
    true
}

/// Scan the next file: its record and its tags.
fn scan_one(loc: &mut Located, scan: &mut Scan, say: &dyn Fn(String)) {
    let Some(file) = scan.files.pop() else {
        return;
    };
    match open(loc, &file, &|_| {}) {
        Ok((track, _, _)) => {
            refresh_tags(&mut loc.lib, track, &file, say);
        }
        Err(e) => say(format!("library: scan: {e}")),
    }
}

/// Fill the track's tag cache from `file`'s tags. The BPM and key stay as our analysis set
/// them once the track has been analysed. True if the cache changed.
fn refresh_tags(
    lib: &mut Library,
    track: library::TrackId,
    file: &Path,
    say: &dyn Fn(String),
) -> bool {
    let Ok(Some(record)) = lib.track(track) else {
        return false;
    };
    let Ok(read) = engine_media::tags(file) else {
        return false;
    };
    let analysed = !record.analysis.is_empty();
    let tags = TrackTags {
        title: read.title,
        artist: read.artist,
        album: read.album,
        genre: read.genre,
        bpm: if analysed { record.tags.bpm } else { read.bpm },
        key: if analysed {
            record.tags.key.clone()
        } else {
            read.key
        },
        duration: read.duration,
    };
    if tags == record.tags {
        return false;
    }
    if let Err(e) = lib.apply(Edit::SetTags(track, tags)) {
        say(format!("library: {e}"));
        return false;
    }
    true
}

/// Send the library's listing to the player.
fn publish(lib: &Library, replies: &Sender<Reply>) {
    if let Ok(rows) = lib.listing() {
        let listing: Listing = rows.into_iter().map(|l| (l.path.clone(), l)).collect();
        let _ = replies.send(Reply::Listing(Arc::new(listing)));
    }
    publish_playlists(lib, replies);
}

/// Send the playlists and their entries to the player.
fn publish_playlists(lib: &Library, replies: &Sender<Reply>) {
    let Ok(lists) = lib.playlists() else {
        return;
    };
    let entries = lists
        .iter()
        .filter(|p| p.kind != library::PlaylistKind::Folder)
        .filter_map(|p| Some((p.id, lib.entries(p.id).ok()?)))
        .collect();
    let _ = replies.send(Reply::Playlists(Arc::new(Playlists { lists, entries })));
}

fn is_playlist_edit(e: &Edit) -> bool {
    matches!(
        e,
        Edit::CreatePlaylist { .. }
            | Edit::RenamePlaylist(..)
            | Edit::MovePlaylist { .. }
            | Edit::DeletePlaylist(_)
            | Edit::AddEntries { .. }
            | Edit::MoveEntry { .. }
            | Edit::RemoveEntries(_)
            | Edit::ClearPlaylist(_)
            | Edit::ConvertTagList { .. }
    )
}

/// Mount the chosen library (the automatic choice: on `file`'s stick) and say what it is.
fn mount(
    choice: &LibraryChoice,
    file: &Path,
    device: DeviceId,
    say: &dyn Fn(String),
) -> Result<Located, String> {
    let started = Instant::now();
    let mut loc = locate(choice, file, device)?;
    let stats = loc.lib.stats().clone();
    say(format!(
        "library: {} ({} tracks, opened in {:.0} ms{})",
        loc.what,
        stats.tracks,
        started.elapsed().as_secs_f64() * 1e3,
        if stats.created { ", new" } else { "" },
    ));
    if stats.replayed > 0 {
        say(format!(
            "library: {} edits kept from an earlier mount written",
            stats.replayed
        ));
    }
    for w in loc.lib.take_warnings() {
        say(format!("library: {w}"));
    }
    Ok(loc)
}

/// Find or make `file`'s record: its track, file hash, and whether the record or the
/// location is new.
fn open(
    loc: &mut Located,
    file: &Path,
    say: &dyn Fn(String),
) -> Result<(library::TrackId, FileHash, bool), String> {
    let (identity, size) =
        hash_file(file).map_err(|e| format!("can't read {}: {e}", file.display()))?;
    let path = stored_path(file, loc.root.as_deref());
    let found = loc
        .lib
        .find_or_create(FileIdentity::new(path, size, identity))
        .map_err(|e| e.to_string())?;
    let track = found.track;
    if found.created {
        say(format!("library: new track {track}"));
    } else if found.new_location {
        say(format!(
            "library: track {track} (known file at a new location)"
        ));
    }
    Ok((track, identity, found.new_location))
}

/// Run the library's integrity check if it wants one, and say what it found.
fn check(lib: &mut Library, say: &dyn Fn(String)) {
    if let Some(problem) = lib.check() {
        say(format!("library: integrity check failed: {problem}"));
    }
}

fn report(lib: &mut Library, say: &dyn Fn(String), status: &mut Status) {
    for w in lib.take_warnings() {
        say(format!("library: {w}"));
    }
    let now = lib.persistence();
    if status.last.as_ref() != Some(&now) {
        let _ = status.replies.send(Reply::Status(now.clone()));
        status.last = Some(now);
    }
}

/// The last persistence sent to the player.
struct Status {
    replies: Sender<Reply>,
    last: Option<Persistence>,
}

/// The record of `track`, with its stored waveform and analysis blobs read.
fn loaded(lib: &mut Library, track: library::TrackId, file_hash: FileHash, serial: u64) -> Loaded {
    let record = lib
        .track(track)
        .ok()
        .flatten()
        .unwrap_or_else(|| Track::empty(track));
    let waveform = record
        .waveform
        .as_ref()
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
        serial,
        track: lib.track_ref(record.id),
        file_hash,
        record,
        waveform,
        analysis,
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
pub fn stored_path(file: &Path, root: Option<&Path>) -> String {
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

/// True if `root` (a mount point) is a removable volume: a USB or SD-card device that is not
/// the disk the system runs from. On macOS, a volume under `/Volumes`.
fn is_removable(root: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        // The disk (not the partition) a path's file system lives on, from sysfs.
        let disk = |p: &Path| -> Option<PathBuf> {
            let dev = std::fs::metadata(p).ok()?.dev();
            let (major, minor) = ((dev >> 8) & 0xfff, (dev & 0xff) | ((dev >> 12) & 0xfff00));
            let sys = std::fs::canonicalize(format!("/sys/dev/block/{major}:{minor}")).ok()?;
            Some(if sys.join("partition").exists() {
                sys.parent()?.to_path_buf()
            } else {
                sys
            })
        };
        let Some(d) = disk(root) else {
            return false;
        };
        if disk(Path::new("/")).is_some_and(|r| r == d) {
            return false;
        }
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let flagged = std::fs::read_to_string(d.join("removable")).is_ok_and(|s| s.trim() == "1");
        flagged || d.to_string_lossy().contains("/usb") || name.starts_with("mmcblk")
    }
    #[cfg(target_os = "macos")]
    {
        let abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        abs.starts_with("/Volumes") && abs != Path::new("/")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = root;
        false
    }
}

fn mount_options(device: DeviceId, read_only: bool) -> MountOptions {
    MountOptions {
        read_only,
        ..MountOptions::new(device)
    }
}

fn ram(device: DeviceId, why: String) -> Located {
    Located {
        lib: Library::ram(mount_options(device, false), format!("no library: {why}")),
        root: None,
        what: format!("RAM only, edits are not saved ({why})"),
    }
}

fn locate(choice: &LibraryChoice, file: &Path, device: DeviceId) -> Result<Located, String> {
    let folder = |dir: &Path| -> Result<Located, String> {
        let _ = std::fs::create_dir_all(dir);
        let read_only = !writable(dir);
        let lib = Library::mount(dir, mount_options(device, read_only))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
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
            if !is_removable(&root) {
                return Ok(ram(
                    device,
                    format!(
                        "{} is not a removable volume; use --library <dir>",
                        root.display()
                    ),
                ));
            }
            let dir = root.join(LIBRARY_DIR);
            // Making the folder is the write test at the stick's root; nothing else is written
            // outside it.
            if dir.is_dir() || std::fs::create_dir(&dir).is_ok() {
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
    fn the_system_disk_is_not_removable() {
        assert!(!is_removable(Path::new("/")));
        let here = std::env::current_dir().unwrap();
        // A checkout on the system disk; CI and dev machines alike.
        if volume_root(&here.join("Cargo.toml")) == Path::new("/") {
            assert!(!is_removable(&volume_root(&here.join("Cargo.toml"))));
        }
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
