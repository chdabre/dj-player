//! The library as the UI sees it: what it lists, its playlists, and what it is doing.

use std::collections::HashMap;
use std::sync::Arc;

use library::{Listed, Persistence, Playlist, PlaylistEntry, PlaylistId, TrackId};

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
    pub fn tagged(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.entries
            .get(&PlaylistId::TAG_LIST)
            .into_iter()
            .flatten()
            .map(|e| e.track)
    }
}

/// How far ANALYZE ALL is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchProgress {
    /// Files finished (analysed or failed) of `total`.
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    /// The file under way.
    pub current: Option<String>,
    /// Asked to stop; finishing the file under way.
    pub stopping: bool,
}

/// The library at a glance: what it lists and what it is doing. Changes go through the
/// [`crate::Port`]'s library calls.
#[derive(Debug, Clone, Copy, Default)]
pub struct LibraryState<'a> {
    /// The library's tracks, once it has said.
    pub listing: Option<&'a Arc<Listing>>,
    pub playlists: Option<&'a Arc<Playlists>>,
    /// ANALYZE ALL under way.
    pub batch: Option<&'a BatchProgress>,
    /// A scan under way: files done, total.
    pub scan: Option<(usize, usize)>,
    /// Whether the deck's edits are being saved; `None` until the library has said.
    pub persistence: Option<&'a Persistence>,
}

impl LibraryState<'_> {
    /// A scan is running (or asked for and not started).
    pub fn scanning(&self) -> bool {
        self.scan
            .is_some_and(|(done, total)| done < total || total == 0)
    }
}
