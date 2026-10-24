//! The client's thread: it takes searches and downloads from the UI, logs in when first
//! needed, gathers a search's hits while its window is open (ranked, and published as they
//! come), follows each download to its end or cancels it, and changes account when asked.
//! A search stops gathering when its window closes, when the DJ leaves it, or when a
//! download starts. On start it removes the partial files a previous run left behind. The network is behind [`Network`], so all of
//! this runs in tests without one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Credentials, Hit, HitKey, Queued, Request, Snapshot, Status, Transfer, best_first};

/// How long a search gathers hits. Peers answer when they get round to it, and the late
/// ones are often the better sources.
pub const SEARCH_WINDOW: Duration = Duration::from_secs(20);

/// How often the worker looks for new hits and download progress.
const POLL: Duration = Duration::from_millis(250);

/// What the worker needs of a SoulSeek network. Calls block at most for a login.
pub trait Network: Send {
    /// An account is set.
    fn has_account(&self) -> bool;
    /// Use `account` from now on, ending the session under way.
    fn set_account(&mut self, account: Option<Credentials>);
    /// Log in, unless a session is up.
    fn connect(&mut self) -> Result<(), String>;
    /// Send a search for `query`; the hits arrive later ([`Network::hits`]).
    fn search(&mut self, query: &str) -> Result<(), String>;
    /// Every audio file found for `query` so far, or `None` if nothing came in since the last
    /// call.
    fn hits(&mut self, query: &str) -> Option<Vec<Hit>>;
    /// Stop gathering hits for `query`.
    fn forget(&mut self, query: &str);
    /// Ask `hit`'s peer for the file, to be written to `dir` under [`Hit::name`].
    fn download(&mut self, hit: &Hit, dir: &Path) -> Result<Receiver<Progress>, String>;
    /// Stop the download of `key`, removing its partial file.
    fn cancel(&mut self, key: &HitKey);
}

/// A download's progress, as the network reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Queued,
    Receiving { done: u64, total: u64 },
    Done,
    Failed(String),
}

pub(super) struct Worker {
    net: Box<dyn Network>,
    dir: PathBuf,
    shared: Arc<Mutex<Snapshot>>,
    /// The search gathering hits, and when it was sent.
    search: Option<(String, Instant)>,
    /// The downloads under way.
    downloads: HashMap<HitKey, Receiver<Progress>>,
}

impl Worker {
    pub(super) fn new(net: Box<dyn Network>, dir: PathBuf, shared: Arc<Mutex<Snapshot>>) -> Self {
        if let Ok(mut s) = shared.lock() {
            s.account = net.has_account();
        }
        Self {
            net,
            dir,
            shared,
            search: None,
            downloads: HashMap::new(),
        }
    }

    /// Serve requests until the client is dropped.
    pub(super) fn run(mut self, requests: Receiver<Request>) {
        clean(&self.dir);
        loop {
            match requests.recv_timeout(POLL) {
                Ok(r) => self.handle(r, Instant::now()),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.poll(Instant::now());
        }
    }

    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut s) = self.shared.lock() {
            f(&mut s);
        }
    }

    fn handle(&mut self, request: Request, now: Instant) {
        match request {
            Request::Search(query) => self.start_search(query.trim(), now),
            Request::Download(hit) => self.start_download(hit),
            Request::Cancel(key) => self.cancel(&key),
            Request::StopSearch => self.stop_search(),
            Request::Account(account) => {
                let set = account.is_some();
                self.net.set_account(account);
                self.update(|s| {
                    s.account = set;
                    // A failed login says nothing about the new account.
                    if matches!(s.status, Status::Failed(_)) {
                        s.status = Status::Idle;
                    }
                });
            }
        }
    }

    /// Stop `key`'s download if it is under way, and take it off the queue.
    fn cancel(&mut self, key: &HitKey) {
        if self.downloads.remove(key).is_some() {
            self.net.cancel(key);
        }
        self.update(|s| {
            s.transfers
                .retain(|q| q.hit.user != key.user || q.hit.path != key.path)
        });
    }

    fn start_search(&mut self, query: &str, now: Instant) {
        if query.is_empty() {
            return;
        }
        if let Some((old, _)) = self.search.take() {
            self.net.forget(&old);
        }
        self.update(|s| {
            s.query = query.to_string();
            s.hits = Arc::default();
            s.status = Status::Connecting;
        });
        let sent = self.net.connect().and_then(|()| self.net.search(query));
        let status = match sent {
            Ok(()) => {
                self.search = Some((query.to_string(), now));
                Status::Searching
            }
            Err(e) => Status::Failed(e),
        };
        self.update(|s| s.status = status);
    }

    fn start_download(&mut self, hit: Hit) {
        let key = hit.key();
        if self.downloads.contains_key(&key) {
            return;
        }
        // The DJ has found what they wanted; the search's traffic would slow the download.
        self.stop_search();
        let started = self
            .net
            .connect()
            .and_then(|()| self.net.download(&hit, &self.dir));
        let transfer = match started {
            Ok(progress) => {
                self.downloads.insert(key.clone(), progress);
                Transfer::Queued
            }
            Err(e) => Transfer::Failed(e),
        };
        let path = self.dir.join(hit.name());
        self.update(|s| set(s, &hit, transfer, path));
    }

    /// Stop gathering hits, keeping those in: the search is done.
    fn stop_search(&mut self) {
        let Some((query, _)) = self.search.take() else {
            return;
        };
        self.publish_hits(&query);
        self.net.forget(&query);
        self.update(|s| s.status = Status::Done);
    }

    /// Publish `query`'s hits, ranked, if new ones came in.
    fn publish_hits(&mut self, query: &str) {
        if let Some(mut hits) = self.net.hits(query) {
            hits.sort_by(best_first);
            self.update(|s| s.hits = Arc::new(hits));
        }
    }

    /// Publish new hits, close the search's window, and follow the downloads.
    fn poll(&mut self, now: Instant) {
        if let Some((query, sent)) = self.search.clone() {
            self.publish_hits(&query);
            if now.duration_since(sent) >= SEARCH_WINDOW {
                self.stop_search();
            }
        }
        let mut changed = Vec::new();
        self.downloads.retain(|key, progress| {
            let mut last = None;
            let open = loop {
                match progress.try_recv() {
                    Ok(p) => last = Some(p),
                    Err(TryRecvError::Empty) => break true,
                    Err(TryRecvError::Disconnected) => break false,
                }
            };
            let finished = matches!(last, Some(Progress::Done | Progress::Failed(_)));
            match last {
                Some(p) => changed.push((key.clone(), p)),
                None if !open => {
                    changed.push((key.clone(), Progress::Failed("the transfer stopped".into())))
                }
                None => {}
            }
            open && !finished
        });
        if changed.is_empty() {
            return;
        }
        let dir = self.dir.clone();
        self.update(|s| {
            for (key, p) in changed {
                let transfer = match p {
                    Progress::Queued => Transfer::Queued,
                    Progress::Receiving { done, total } => Transfer::Receiving { done, total },
                    Progress::Done => Transfer::Done(dir.join(file_name(&key.path))),
                    Progress::Failed(e) => Transfer::Failed(e),
                };
                if let Some(q) = s
                    .transfers
                    .iter_mut()
                    .find(|q| q.hit.user == key.user && q.hit.path == key.path)
                {
                    q.transfer = transfer;
                }
            }
        });
    }
}

/// `hit`'s place in the queue says `transfer` (at the end, if it has none).
fn set(s: &mut Snapshot, hit: &Hit, transfer: Transfer, path: PathBuf) {
    match s.transfers.iter_mut().find(|q| q.hit.key() == hit.key()) {
        Some(q) => q.transfer = transfer,
        None => s.transfers.push(Queued {
            hit: hit.clone(),
            transfer,
            path,
        }),
    }
}

/// The partial files (`.part`) in the downloads folder `dir`: what a run that ended with
/// downloads under way left behind. Nothing else is touched. Returns how many went.
fn clean(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_string_lossy().ends_with(PART_SUFFIX))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// What a download writes to until it is complete (`soulseek-rs-lib`'s suffix).
pub(crate) const PART_SUFFIX: &str = ".part";

/// A peer's file name without its folders.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, Quality};
    use std::sync::mpsc::{Sender, channel};

    /// A network that answers from a script, and records what it was asked.
    #[derive(Default)]
    struct Script {
        logins: Arc<Mutex<Vec<bool>>>,
        /// Fail the next login with this.
        refuse: Option<String>,
        hits: Vec<Hit>,
        /// Hits already handed out.
        given: usize,
        senders: Arc<Mutex<Vec<Sender<Progress>>>>,
        forgotten: Arc<Mutex<Vec<String>>>,
        accounts: Arc<Mutex<Vec<Option<String>>>>,
        cancelled: Arc<Mutex<Vec<String>>>,
    }

    impl Network for Script {
        fn has_account(&self) -> bool {
            true
        }

        fn set_account(&mut self, account: Option<Credentials>) {
            self.accounts.lock().unwrap().push(account.map(|a| a.user));
        }

        fn cancel(&mut self, key: &HitKey) {
            self.cancelled.lock().unwrap().push(key.path.clone());
        }

        fn connect(&mut self) -> Result<(), String> {
            let refused = self.refuse.take();
            self.logins.lock().unwrap().push(refused.is_none());
            refused.map_or(Ok(()), Err)
        }

        fn search(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }

        fn hits(&mut self, _: &str) -> Option<Vec<Hit>> {
            (self.given != self.hits.len()).then(|| {
                self.given = self.hits.len();
                self.hits.clone()
            })
        }

        fn forget(&mut self, query: &str) {
            self.forgotten.lock().unwrap().push(query.into());
        }

        fn download(&mut self, _: &Hit, _: &Path) -> Result<Receiver<Progress>, String> {
            let (tx, rx) = channel();
            self.senders.lock().unwrap().push(tx);
            Ok(rx)
        }
    }

    fn hit(name: &str, kbps: u32) -> Hit {
        Hit {
            user: "peer".into(),
            path: format!("@@share\\Artist\\{name}"),
            size: 1000,
            quality: Quality::new(Format::of(name).unwrap(), &[(0, kbps)].into()),
            free_slot: true,
            speed: 1,
        }
    }

    fn worker(net: Script) -> (Worker, Arc<Mutex<Snapshot>>) {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let w = Worker::new(
            Box::new(net),
            PathBuf::from("/stick/SoulSeek Downloads"),
            Arc::clone(&shared),
        );
        (w, shared)
    }

    fn snap(s: &Arc<Mutex<Snapshot>>) -> Snapshot {
        s.lock().unwrap().clone()
    }

    #[test]
    fn a_search_publishes_its_hits_best_first_until_its_window_closes() {
        let net = Script {
            hits: vec![
                hit("low.mp3", 128),
                hit("high.flac", 0),
                hit("mid.mp3", 320),
            ],
            ..Script::default()
        };
        let forgotten = Arc::clone(&net.forgotten);
        let (mut w, shared) = worker(net);
        let t0 = Instant::now();
        w.handle(Request::Search("  artist track ".into()), t0);
        assert_eq!(snap(&shared).status, Status::Searching);
        assert_eq!(snap(&shared).query, "artist track", "trimmed");
        w.poll(t0);
        let names: Vec<_> = snap(&shared)
            .hits
            .iter()
            .map(|h| h.name().to_string())
            .collect();
        assert_eq!(names, ["high.flac", "mid.mp3", "low.mp3"]);
        w.poll(t0 + SEARCH_WINDOW);
        assert_eq!(snap(&shared).status, Status::Done);
        assert_eq!(snap(&shared).hits.len(), 3, "the hits stay once it is done");
        assert_eq!(*forgotten.lock().unwrap(), ["artist track"]);
    }

    #[test]
    fn a_failed_login_is_shown_and_the_next_request_tries_again() {
        let net = Script {
            refuse: Some("Authentication failed".into()),
            ..Script::default()
        };
        let logins = Arc::clone(&net.logins);
        let (mut w, shared) = worker(net);
        w.handle(Request::Search("x".into()), Instant::now());
        assert_eq!(
            snap(&shared).status,
            Status::Failed("Authentication failed".into())
        );
        w.handle(Request::Search("x".into()), Instant::now());
        assert_eq!(snap(&shared).status, Status::Searching);
        assert_eq!(*logins.lock().unwrap(), [false, true]);
        // An empty query sends nothing.
        w.handle(Request::Search("   ".into()), Instant::now());
        assert_eq!(logins.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_download_is_followed_to_the_file_in_the_downloads_folder() {
        let net = Script::default();
        let senders = Arc::clone(&net.senders);
        let (mut w, shared) = worker(net);
        let h = hit("Track.flac", 0);
        w.handle(Request::Download(h.clone()), Instant::now());
        assert_eq!(snap(&shared).transfer(&h), Some(&Transfer::Queued));
        // Asked again while under way: nothing new.
        w.handle(Request::Download(h.clone()), Instant::now());
        assert_eq!(senders.lock().unwrap().len(), 1);

        let tx = senders.lock().unwrap()[0].clone();
        tx.send(Progress::Receiving {
            done: 10,
            total: 1000,
        })
        .unwrap();
        tx.send(Progress::Receiving {
            done: 500,
            total: 1000,
        })
        .unwrap();
        w.poll(Instant::now());
        assert_eq!(
            snap(&shared).transfer(&h),
            Some(&Transfer::Receiving {
                done: 500,
                total: 1000
            }),
            "the latest progress"
        );
        tx.send(Progress::Done).unwrap();
        w.poll(Instant::now());
        assert_eq!(
            snap(&shared).transfer(&h),
            Some(&Transfer::Done(PathBuf::from(
                "/stick/SoulSeek Downloads/Track.flac"
            )))
        );
        // Done: asking again downloads it again (the DJ asked).
        w.handle(Request::Download(h.clone()), Instant::now());
        assert_eq!(senders.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_transfer_that_goes_quiet_fails() {
        let net = Script::default();
        let senders = Arc::clone(&net.senders);
        let (mut w, shared) = worker(net);
        let h = hit("a.mp3", 320);
        w.handle(Request::Download(h.clone()), Instant::now());
        senders.lock().unwrap().clear();
        w.poll(Instant::now());
        assert!(matches!(
            snap(&shared).transfer(&h),
            Some(Transfer::Failed(_))
        ));
    }

    #[test]
    fn cancelling_stops_a_download_and_takes_finished_ones_off_the_queue() {
        let net = Script::default();
        let (senders, cancelled) = (Arc::clone(&net.senders), Arc::clone(&net.cancelled));
        let (mut w, shared) = worker(net);
        let (a, b) = (hit("a.mp3", 320), hit("b.flac", 0));
        w.handle(Request::Download(a.clone()), Instant::now());
        w.handle(Request::Download(b.clone()), Instant::now());
        let order: Vec<_> = snap(&shared)
            .transfers
            .iter()
            .map(|q| q.hit.name().to_string())
            .collect();
        assert_eq!(order, ["a.mp3", "b.flac"], "in the order asked for");
        assert_eq!(snap(&shared).progress(), (2, 0));
        senders.lock().unwrap()[1].send(Progress::Done).unwrap();
        w.poll(Instant::now());
        assert_eq!(snap(&shared).progress(), (1, 0), "one under way");

        w.handle(Request::Cancel(a.key()), Instant::now());
        assert_eq!(
            cancelled.lock().unwrap().as_slice(),
            std::slice::from_ref(&a.path)
        );
        w.handle(Request::Cancel(b.key()), Instant::now());
        assert_eq!(
            cancelled.lock().unwrap().len(),
            1,
            "a finished one is only taken off"
        );
        assert!(snap(&shared).transfers.is_empty());
    }

    #[test]
    fn a_new_account_clears_a_failed_login() {
        let net = Script {
            refuse: Some("Authentication failed".into()),
            ..Script::default()
        };
        let accounts = Arc::clone(&net.accounts);
        let (mut w, shared) = worker(net);
        assert!(snap(&shared).account);
        w.handle(Request::Search("x".into()), Instant::now());
        w.handle(
            Request::Account(Some(Credentials {
                user: "new".into(),
                password: "p".into(),
            })),
            Instant::now(),
        );
        assert_eq!(snap(&shared).status, Status::Idle);
        w.handle(Request::Account(None), Instant::now());
        assert!(!snap(&shared).account);
        assert_eq!(*accounts.lock().unwrap(), [Some("new".into()), None]);
    }

    #[test]
    fn leaving_the_search_or_starting_a_download_stops_it_and_keeps_the_hits() {
        let net = Script {
            hits: vec![hit("a.mp3", 320)],
            ..Script::default()
        };
        let forgotten = Arc::clone(&net.forgotten);
        let (mut w, shared) = worker(net);
        w.handle(Request::Search("q".into()), Instant::now());
        w.handle(Request::StopSearch, Instant::now());
        assert_eq!(snap(&shared).status, Status::Done);
        assert_eq!(snap(&shared).hits.len(), 1, "the hits in by then are kept");
        assert_eq!(*forgotten.lock().unwrap(), ["q"]);
        w.handle(Request::StopSearch, Instant::now());
        assert_eq!(forgotten.lock().unwrap().len(), 1, "once");

        w.handle(Request::Search("r".into()), Instant::now());
        w.handle(Request::Download(hit("a.mp3", 320)), Instant::now());
        assert_eq!(snap(&shared).status, Status::Done);
        assert_eq!(*forgotten.lock().unwrap(), ["q", "r"]);
    }

    #[test]
    fn partial_files_left_behind_are_removed_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("soulseek-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub.part")).unwrap();
        for f in ["a.flac.part", "b.mp3", "c.part.mp3", "sub.part/d.mp3.part"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        assert_eq!(clean(&dir), 1);
        assert!(!dir.join("a.flac.part").exists());
        for kept in ["b.mp3", "c.part.mp3", "sub.part/d.mp3.part"] {
            assert!(dir.join(kept).exists(), "{kept}");
        }
        assert_eq!(clean(&dir.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
