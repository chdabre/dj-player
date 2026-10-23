//! SoulSeek, as the browse screen's second search source: search the network, see the hits
//! best quality first, and download the one the DJ picks into the stick's
//! [`DOWNLOADS_DIR`] folder, where the folder tree (and a scan) find it like any other file.
//!
//! The network runs on a thread of its own ([`worker`]), so nothing here blocks a frame:
//! [`Soulseek::search`] and [`Soulseek::download`] queue a request, and [`Soulseek::snapshot`]
//! reads what has come in. It logs in when first asked for something, with the account from
//! [`Credentials`], and logs in again after the session drops. Nothing is shared with the
//! network yet.

mod live;
mod quality;
mod worker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

pub use live::Live;
pub use quality::{Format, Quality, best_first};
pub use worker::{Network, Progress};

/// The folder at the stick's root that downloads go to.
pub const DOWNLOADS_DIR: &str = "SoulSeek Downloads";

/// The SoulSeek account to log in with.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    pub password: String,
}

impl Credentials {
    /// `SOULSEEK_USERNAME` and `SOULSEEK_PASSWORD`, if both are set.
    pub fn from_env() -> Option<Self> {
        let get = |k| std::env::var(k).ok().filter(|v: &String| !v.is_empty());
        Some(Self {
            user: get("SOULSEEK_USERNAME")?,
            password: get("SOULSEEK_PASSWORD")?,
        })
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// One file a peer offered for the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The peer.
    pub user: String,
    /// The file as the peer names it (folders split by `\` or `/`).
    pub path: String,
    /// Bytes.
    pub size: u64,
    pub quality: Quality,
    /// The peer has an upload slot free now.
    pub free_slot: bool,
    /// The peer's upload speed, bytes per second.
    pub speed: u32,
}

impl Hit {
    /// The file's name, without the peer's folders.
    pub fn name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }

    /// The name without its extension.
    pub fn title(&self) -> &str {
        let name = self.name();
        name.rsplit_once('.').map_or(name, |(t, _)| t)
    }

    pub fn key(&self) -> HitKey {
        HitKey {
            user: self.user.clone(),
            path: self.path.clone(),
        }
    }
}

/// Which file of which peer: what a download is known by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HitKey {
    pub user: String,
    pub path: String,
}

/// Where the search is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// Nothing asked yet.
    #[default]
    Idle,
    /// Logging in.
    Connecting,
    /// Hits are coming in.
    Searching,
    /// The search's window has closed; the hits are all there will be.
    Done,
    Failed(String),
}

/// Where a download is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transfer {
    /// Asked for; the peer has not started sending (no free slot, or not reached yet).
    Queued,
    Receiving {
        done: u64,
        total: u64,
    },
    /// In the downloads folder, at this path.
    Done(PathBuf),
    Failed(String),
}

impl Transfer {
    /// Under way: asking for it again does nothing.
    pub fn active(&self) -> bool {
        matches!(self, Transfer::Queued | Transfer::Receiving { .. })
    }
}

/// What the client has: the search, its hits best first, and the downloads.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// The query last searched for.
    pub query: String,
    pub status: Status,
    /// Best first ([`best_first`]).
    pub hits: Arc<Vec<Hit>>,
    pub transfers: HashMap<HitKey, Transfer>,
}

impl Snapshot {
    pub fn transfer(&self, hit: &Hit) -> Option<&Transfer> {
        self.transfers.get(&hit.key())
    }
}

/// A request to the worker.
#[derive(Debug)]
enum Request {
    Search(String),
    Download(Hit),
}

/// A SoulSeek client on its own thread.
pub struct Soulseek {
    requests: Sender<Request>,
    shared: Arc<Mutex<Snapshot>>,
}

impl Soulseek {
    /// A client logging in as `credentials`, downloading into `dir`.
    pub fn start(credentials: Credentials, dir: PathBuf) -> Self {
        Self::with_network(Box::new(Live::new(credentials)), dir)
    }

    /// A client on `net` (a stand-in network, for tests), downloading into `dir`.
    pub fn with_network(net: Box<dyn Network>, dir: PathBuf) -> Self {
        let (requests, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let worker = worker::Worker::new(net, dir, Arc::clone(&shared));
        std::thread::Builder::new()
            .name("soulseek".into())
            .spawn(move || worker.run(rx))
            .expect("spawn the soulseek thread");
        Self { requests, shared }
    }

    /// Search for `query`, replacing the last search and its hits.
    pub fn search(&self, query: &str) {
        let _ = self.requests.send(Request::Search(query.to_string()));
    }

    /// Download `hit` into the downloads folder (again, if it failed).
    pub fn download(&self, hit: &Hit) {
        let _ = self.requests.send(Request::Download(hit.clone()));
    }

    /// What has come in so far.
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_is_named_without_the_peers_folders() {
        let hit = Hit {
            user: "u".into(),
            path: "@@abc\\Music/Artist\\01 Track.flac".into(),
            size: 0,
            quality: Quality::new(Format::Flac, &HashMap::new()),
            free_slot: true,
            speed: 0,
        };
        assert_eq!(hit.name(), "01 Track.flac");
        assert_eq!(hit.title(), "01 Track");
    }

    #[test]
    fn credentials_never_print_the_password() {
        let c = Credentials {
            user: "dj".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{c:?}").contains("hunter2"));
    }
}
