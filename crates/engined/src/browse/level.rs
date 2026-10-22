//! What the browser lists: entries, the levels they make up, and how they are ordered.

use std::path::PathBuf;

use library::{EntryId, PlaylistId};

use super::files::list;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Track,
    /// A playlist or a folder of playlists.
    Playlist(PlaylistId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The file (empty for a playlist entry whose track has no known file).
    pub path: PathBuf,
    pub kind: Kind,
    /// In a playlist: the entry, for removing and moving it.
    pub entry: Option<EntryId>,
}

/// How tracks are ordered. Folders always come first, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// As the level lists them: a playlist in its own order, a folder by name.
    #[default]
    Order,
    /// By file name.
    Name,
    Title,
    Artist,
    Bpm,
    Key,
    Length,
}

impl Sort {
    pub const ALL: [Sort; 7] = [
        Sort::Order,
        Sort::Name,
        Sort::Title,
        Sort::Artist,
        Sort::Bpm,
        Sort::Key,
        Sort::Length,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Order => "#",
            Sort::Name => "NAME",
            Sort::Title => "TITLE",
            Sort::Artist => "ARTIST",
            Sort::Bpm => "BPM",
            Sort::Key => "KEY",
            Sort::Length => "TIME",
        }
    }

    /// The next order, round the list.
    pub fn next(self) -> Sort {
        let i = Sort::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Sort::ALL[(i + 1) % Sort::ALL.len()]
    }
}

/// Where the list comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The stick's folders and files.
    Folders,
    /// Every track the library knows, in one list.
    Library,
    /// The tag list on its own (the TAG LIST tab).
    TagList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LevelKind {
    /// A folder on the stick.
    Folder,
    /// The library's top: FOLDERS and TRACKS.
    LibraryMenu,
    /// Every track the library knows, in one list.
    LibraryTracks,
    /// The library's tracks under the folder `prefix` of their stored paths (`""` for the
    /// stick's root, else ending in `/`): its subfolders, then its tracks.
    LibraryFolder { prefix: String },
    /// The playlists and folders in playlist folder `parent` (`None`: the top).
    Playlists { parent: Option<PlaylistId> },
    /// A playlist's (or the tag list's) tracks, in its order.
    Playlist { id: PlaylistId },
    /// The tracks under the level below whose name or tags hold every word of `query`.
    Search { query: String },
}

/// One level of the list: its entries in order, the cursor and the first row shown.
#[derive(Debug, Clone)]
pub struct Level {
    pub(crate) kind: LevelKind,
    /// The level's name in the header.
    pub(crate) name: String,
    pub(crate) dir: PathBuf,
    /// Everything in the level, before sorting and searching.
    pub(super) all: Vec<Entry>,
    /// What is shown, sorted (and filtered, when searching).
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub top: usize,
    /// Why the folder could not be read, if it couldn't.
    pub error: Option<String>,
    /// The DJ moved the cursor or scrolled here; until then a re-sort keeps the top in view
    /// rather than following an entry nobody picked.
    pub(super) touched: bool,
}

impl Level {
    pub(super) fn folder(dir: PathBuf, root: bool) -> Self {
        let (all, error) = match list(&dir, root) {
            Ok(e) => (e, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let name = dir.file_name().map_or_else(
            || dir.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        Self::new(LevelKind::Folder, name, dir, all, error)
    }

    pub(super) fn new(
        kind: LevelKind,
        name: String,
        dir: PathBuf,
        all: Vec<Entry>,
        error: Option<String>,
    ) -> Self {
        Self {
            kind,
            name,
            dir,
            entries: all.clone(),
            all,
            cursor: 0,
            top: 0,
            error,
            touched: false,
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }
}

/// What [`Browser::enter`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entered {
    /// Opened a folder.
    Folder,
    /// A track to load.
    Track(PathBuf),
    /// Nothing selected.
    Nothing,
}
