//! The worker's event loop: requests first, then (between them) files being added to
//! playlists, the scan, and the commit once edits are due.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use library::{AudioHash, DeviceId, Edit, Library, Persistence, PlaylistId, TrackId};
use waveform::compute::Waveform;

use super::locate::{Located, mount};
use super::records::{
    is_playlist_edit, loaded, open, publish, publish_playlists, refresh_tags, scan_one,
    store_analysis, store_bpm_key, store_waveform,
};
use super::{BatchResult, FreshAnalysis, LibraryChoice, Reply, Request};

/// Edits are committed this long after the first uncommitted one.
const FLUSH_AFTER: Duration = Duration::from_secs(1);

/// How often a running scan reports progress and publishes the listing.
const SCAN_REPORT: Duration = Duration::from_millis(500);

/// A track a deck has loaded: requests name it by its load serial.
struct Open {
    track: TrackId,
    serial: u64,
}

/// A scan of the stick under way: the files still to do.
struct Scan {
    files: Vec<PathBuf>,
    total: usize,
    /// When progress was last reported.
    reported: Instant,
}

/// The last persistence sent to the player.
struct Status {
    replies: Sender<Reply>,
    last: Option<Persistence>,
}

pub(super) fn run(
    device: Option<DeviceId>,
    choice: LibraryChoice,
    requests: &Receiver<Request>,
    replies: &Sender<Reply>,
) {
    let Some(mut w) = Worker::new(device, choice, replies) else {
        return;
    };
    loop {
        let req = match w.wait() {
            Some(wait) => match requests.recv_timeout(wait) {
                Ok(r) => Some(r),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match requests.recv() {
                Ok(r) => Some(r),
                Err(_) => break,
            },
        };
        match req {
            None => w.idle(),
            Some(Request::Open { serial, file }) => {
                if !w.open_file(serial, &file) {
                    return;
                }
            }
            Some(req) => w.request(req),
        }
    }
    w.finish();
}

/// Says a line to the player.
fn sayer(replies: &Sender<Reply>) -> impl Fn(String) + '_ {
    move |m: String| {
        let _ = replies.send(Reply::Message(m));
    }
}

/// The worker's state between requests.
struct Worker<'a> {
    choice: LibraryChoice,
    device: DeviceId,
    replies: &'a Sender<Reply>,
    status: Status,
    /// Mounted now for a chosen folder (or RAM), so the browser can list it before anything
    /// loads; on the first open for the automatic choice: it needs a file to find its stick.
    loc: Option<Located>,
    /// The tracks on the decks, until each is closed.
    open: Vec<Open>,
    scan: Option<Scan>,
    /// Files still to add to playlists, in order, and when the playlists were last sent.
    adding: VecDeque<(PlaylistId, PathBuf)>,
    adding_sent: Instant,
    /// The listing changed since it was last published.
    listing_stale: bool,
    /// When the edits so far are committed.
    deadline: Option<Instant>,
}

impl<'a> Worker<'a> {
    /// The worker, with a chosen folder (or RAM) mounted; `None` if that failed (and the
    /// player was told).
    fn new(
        device: Option<DeviceId>,
        choice: LibraryChoice,
        replies: &'a Sender<Reply>,
    ) -> Option<Self> {
        let say = sayer(replies);
        let device = device.unwrap_or_else(|| {
            let (device, warning) = crate::config::device_id();
            if let Some(w) = warning {
                say(w);
            }
            device
        });
        let mut w = Self {
            choice,
            device,
            replies,
            status: Status {
                replies: replies.clone(),
                last: None,
            },
            loc: None,
            open: Vec::new(),
            scan: None,
            adding: VecDeque::new(),
            adding_sent: Instant::now(),
            listing_stale: false,
            deadline: None,
        };
        if w.choice != LibraryChoice::Auto {
            match mount(&w.choice, Path::new(""), device, &say) {
                Ok(mut l) => {
                    report(&mut l.lib, &say, &mut w.status);
                    publish(&l.lib, replies);
                    w.loc = Some(l);
                }
                Err(e) => {
                    let _ = replies.send(Reply::Failed(e));
                    return None;
                }
            }
        }
        Some(w)
    }

    /// How long to wait for a request: not at all while files are being added or scanned (a
    /// request always comes first), else until the commit is due; `None`: until one comes.
    fn wait(&self) -> Option<Duration> {
        if !self.adding.is_empty() || self.scan.is_some() {
            return Some(Duration::ZERO);
        }
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// An edit to commit: within [`FLUSH_AFTER`] of the first uncommitted one.
    fn mark_dirty(&mut self) {
        self.deadline
            .get_or_insert_with(|| Instant::now() + FLUSH_AFTER);
    }

    fn report(&mut self) {
        if let Some(loc) = self.loc.as_mut() {
            report(&mut loc.lib, &sayer(self.replies), &mut self.status);
        }
    }

    /// No request came: add a file to a playlist, or scan one, and commit when due.
    fn idle(&mut self) {
        if self.loc.is_none() {
            self.deadline = None;
            return;
        }
        if self.add_next() {
            return;
        }
        self.scan_next();
        self.flush_if_due();
    }

    /// Add the next queued file to its playlist; false if none is queued.
    fn add_next(&mut self) -> bool {
        let say = sayer(self.replies);
        let Some(l) = self.loc.as_mut() else {
            return false;
        };
        let Some((playlist, file)) = self.adding.pop_front() else {
            return false;
        };
        match open(l, &file, &|_| {}) {
            Ok((track, _)) => {
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
        self.listing_stale = true;
        if self.adding.is_empty() || self.adding_sent.elapsed() >= SCAN_REPORT {
            self.adding_sent = Instant::now();
            publish_playlists(&l.lib, self.replies);
        }
        self.mark_dirty();
        true
    }

    /// Scan the next file, if a scan is under way, and report its progress now and then.
    fn scan_next(&mut self) {
        let say = sayer(self.replies);
        let (Some(l), Some(sc)) = (self.loc.as_mut(), self.scan.as_mut()) else {
            return;
        };
        scan_one(l, &mut sc.files, &say);
        self.listing_stale = true;
        let finished = sc.files.is_empty();
        if finished || sc.reported.elapsed() >= SCAN_REPORT {
            sc.reported = Instant::now();
            let _ = self.replies.send(Reply::Scan {
                done: sc.total - sc.files.len(),
                total: sc.total,
            });
            publish(&l.lib, self.replies);
            self.listing_stale = false;
        }
        if finished {
            say(format!("library: scan done, {} files", sc.total));
            self.scan = None;
            l.lib.flush();
            report(&mut l.lib, &say, &mut self.status);
        }
        self.mark_dirty();
    }

    /// Commit once the deadline has passed, and publish the listing if it changed.
    fn flush_if_due(&mut self) {
        let say = sayer(self.replies);
        let Some(l) = self.loc.as_mut() else {
            return;
        };
        if !self.deadline.is_some_and(|d| Instant::now() >= d) {
            return;
        }
        l.lib.flush();
        check(&mut l.lib, &say);
        report(&mut l.lib, &say, &mut self.status);
        self.deadline = None;
        if self.listing_stale {
            publish(&l.lib, self.replies);
            self.listing_stale = false;
        }
    }

    /// The deck loaded `file`: mount if this is the first, then find or make its record and
    /// answer with it. False if the library could not be mounted (and the worker ends).
    fn open_file(&mut self, serial: u64, file: &Path) -> bool {
        let say = sayer(self.replies);
        if self.loc.is_none() {
            match mount(&self.choice, file, self.device, &say) {
                Ok(l) => {
                    publish(&l.lib, self.replies);
                    self.loc = Some(l);
                }
                Err(e) => {
                    let _ = self.replies.send(Reply::Failed(e));
                    return false;
                }
            }
        }
        let loc = self.loc.as_mut().expect("just mounted");
        match open(loc, file, &say) {
            Ok((track, new_location)) => {
                let tags_changed = refresh_tags(&mut loc.lib, track, file, &say);
                let _ =
                    self.replies
                        .send(Reply::Loaded(Box::new(loaded(&mut loc.lib, track, serial))));
                self.open.retain(|o| o.serial != serial);
                self.open.push(Open { track, serial });
                report(&mut loc.lib, &say, &mut self.status);
                // After an unclean close: the record is out, so the check doesn't hold up
                // the load.
                check(&mut loc.lib, &say);
                // A new record or location is committed like an edit.
                if new_location || tags_changed {
                    self.listing_stale = true;
                    self.mark_dirty();
                }
            }
            Err(reason) => {
                let _ = self.replies.send(Reply::Unrecorded { serial, reason });
            }
        }
        true
    }

    /// Any request but an open; nothing without a library.
    fn request(&mut self, req: Request) {
        if self.loc.is_none() {
            return;
        }
        let handled = match req {
            Request::Open { .. } => unreachable!("handled by open_file"),
            Request::Scan => {
                self.start_scan();
                true
            }
            Request::Store(r) => {
                self.store(*r);
                true
            }
            Request::Edit(e) => {
                self.edit(e);
                true
            }
            Request::AddFiles { playlist, files } => {
                self.adding.extend(files.into_iter().map(|f| (playlist, f)));
                self.adding_sent = Instant::now();
                true
            }
            Request::NewPlaylist {
                id,
                parent,
                name,
                files,
            } => {
                self.new_playlist(id, parent, name, files);
                true
            }
            Request::Close(serial) => {
                self.open.retain(|o| o.serial != serial);
                false
            }
            Request::Analysed { serial, bpm, key } => self.analysed(serial, bpm, key),
            Request::AudioHash(serial, h) => self.audio_hash(serial, h),
            Request::Waveform(serial, w) => self.waveform(serial, &w),
            Request::Analysis(serial, fresh) => self.analysis(serial, fresh),
        };
        // A request for a track that is not open is dropped unreported.
        if handled {
            self.report();
        }
    }

    fn start_scan(&mut self) {
        let Some(loc) = self.loc.as_ref() else {
            return;
        };
        if self.scan.is_some() {
            return;
        }
        let files = loc
            .root
            .as_deref()
            .map(library::stick::walk)
            .unwrap_or_default();
        sayer(self.replies)(format!("library: scanning {} files", files.len()));
        let _ = self.replies.send(Reply::Scan {
            done: 0,
            total: files.len(),
        });
        self.scan = Some(Scan {
            total: files.len(),
            // Taken from the end: reverse, so the scan goes in path order.
            files: files.into_iter().rev().collect(),
            reported: Instant::now(),
        });
    }

    fn store(&mut self, r: BatchResult) {
        let say = sayer(self.replies);
        let Some(loc) = self.loc.as_mut() else {
            return;
        };
        match open(loc, &r.file, &say) {
            Ok((mut track, _)) => {
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
                self.listing_stale = true;
                // Publishes the listing within the second.
                self.mark_dirty();
            }
            Err(e) => say(format!("library: {e}")),
        }
    }

    fn edit(&mut self, e: Edit) {
        let Some(loc) = self.loc.as_mut() else {
            return;
        };
        let playlist = is_playlist_edit(&e);
        if let Err(e) = loc.lib.apply(e) {
            sayer(self.replies)(format!("library: {e}"));
        }
        if playlist {
            publish_playlists(&loc.lib, self.replies);
        }
        self.mark_dirty();
    }

    fn new_playlist(
        &mut self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    ) {
        let Some(loc) = self.loc.as_mut() else {
            return;
        };
        let make = Edit::CreatePlaylist {
            id,
            parent,
            kind: library::PlaylistKind::List,
            name,
        };
        match loc.lib.apply(make) {
            Ok(_) => {
                publish_playlists(&loc.lib, self.replies);
                self.adding.extend(files.into_iter().map(|f| (id, f)));
                self.adding_sent = Instant::now();
            }
            Err(e) => sayer(self.replies)(format!("library: {e}")),
        }
        self.mark_dirty();
    }

    /// The track open under `serial`, and the library; `None` if either is missing.
    fn track(&mut self, serial: u64) -> Option<(&mut Located, &mut Open)> {
        let loc = self.loc.as_mut()?;
        let open = self.open.iter_mut().find(|o| o.serial == serial)?;
        Some((loc, open))
    }

    /// An open track's analysed BPM and key, for the tag cache; false if it is not open.
    fn analysed(&mut self, serial: u64, bpm: Option<f64>, key: Option<String>) -> bool {
        let replies = self.replies;
        let Some((loc, cur)) = self.track(serial) else {
            return false;
        };
        if store_bpm_key(&mut loc.lib, cur.track, bpm, key, &sayer(replies)) {
            self.listing_stale = true;
            self.mark_dirty();
        }
        true
    }

    /// An open track's audio hash; a track with the same audio takes its place.
    fn audio_hash(&mut self, serial: u64, h: AudioHash) -> bool {
        let replies = self.replies;
        let say = sayer(replies);
        let Some((loc, cur)) = self.track(serial) else {
            return false;
        };
        let survivor = match loc.lib.set_audio_hash(cur.track, h) {
            Ok(s) => s,
            Err(e) => {
                say(format!("library: {e}"));
                cur.track
            }
        };
        if survivor != cur.track {
            say(format!(
                "library: same audio as track {survivor}: merged into it"
            ));
            cur.track = survivor;
            let _ = replies.send(Reply::Loaded(Box::new(loaded(
                &mut loc.lib,
                cur.track,
                cur.serial,
            ))));
            self.listing_stale = true;
        }
        self.mark_dirty();
        true
    }

    fn waveform(&mut self, serial: u64, w: &Waveform) -> bool {
        let replies = self.replies;
        let Some((loc, cur)) = self.track(serial) else {
            return false;
        };
        store_waveform(&mut loc.lib, cur.track, w, &sayer(replies));
        loc.lib.flush();
        true
    }

    fn analysis(&mut self, serial: u64, fresh: Vec<FreshAnalysis>) -> bool {
        let replies = self.replies;
        let Some((loc, cur)) = self.track(serial) else {
            return false;
        };
        store_analysis(&mut loc.lib, cur.track, fresh, &sayer(replies));
        loc.lib.flush();
        true
    }

    /// The player hung up: the final commit.
    fn finish(mut self) {
        if let Some(loc) = self.loc.as_mut() {
            loc.lib.flush();
        }
        self.report();
    }
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
