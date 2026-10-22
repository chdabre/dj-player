//! The library's side of the browser: its levels built from the listing and the
//! playlists, and what it knows of a file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use library::{EntryId, PlaylistId, TrackId};

use super::{Browser, Entry, Kind, Level, LevelKind, Source, Tags, walk};
use crate::store::{Listing, Playlists, stored_path};

/// The library menu's entries.
pub(super) const MENU_PLAYLISTS: &str = "PLAYLISTS";
pub(super) const MENU_FOLDERS: &str = "FOLDERS";
pub(super) const MENU_TRACKS: &str = "TRACKS";

impl Browser {
    pub(super) fn library_level(&self) -> Level {
        let kind = LevelKind::LibraryMenu;
        let all = self.library_entries(&kind);
        Level::new(kind, "LIBRARY".into(), self.root.clone(), all, None)
    }

    /// A library track entry for stored path `p`.
    fn library_track(&self, p: &str) -> Entry {
        let path = self.root.join(p);
        let name = path
            .file_name()
            .map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned());
        Entry {
            name,
            path,
            kind: Kind::Track,
            entry: None,
        }
    }

    /// The entries of a library level, from the listing.
    pub(super) fn library_entries(&self, kind: &LevelKind) -> Vec<Entry> {
        let keys = || {
            let mut k: Vec<&String> = self.listing.iter().flat_map(|l| l.keys()).collect();
            k.sort();
            k
        };
        match kind {
            LevelKind::LibraryMenu => [MENU_PLAYLISTS, MENU_FOLDERS, MENU_TRACKS]
                .into_iter()
                .map(|name| Entry {
                    name: name.into(),
                    path: PathBuf::from(format!("library:{name}")),
                    kind: Kind::Folder,
                    entry: None,
                })
                .collect(),
            LevelKind::Playlists { parent } => self
                .playlists
                .iter()
                .flat_map(|p| &p.lists)
                .filter(|p| p.parent == *parent)
                .map(|p| Entry {
                    name: p.name.clone(),
                    path: PathBuf::from(format!("playlist:{}", p.id)),
                    kind: Kind::Playlist(p.id),
                    entry: None,
                })
                .collect(),
            LevelKind::Playlist { id } => {
                let files = self.files_by_track();
                self.playlists
                    .iter()
                    .flat_map(|p| p.entries.get(id))
                    .flatten()
                    .map(|e| match files.get(&e.track) {
                        Some(p) => Entry {
                            entry: Some(e.id),
                            ..self.library_track(p)
                        },
                        None => Entry {
                            name: "Missing track".into(),
                            path: PathBuf::new(),
                            kind: Kind::Track,
                            entry: Some(e.id),
                        },
                    })
                    .collect()
            }
            LevelKind::LibraryTracks => keys().into_iter().map(|p| self.library_track(p)).collect(),
            LevelKind::LibraryFolder { prefix } => {
                let mut folders = std::collections::BTreeSet::new();
                let mut out = Vec::new();
                for p in keys() {
                    let Some(rest) = p.strip_prefix(prefix.as_str()) else {
                        continue;
                    };
                    match rest.split_once('/') {
                        Some((folder, _)) => {
                            folders.insert(folder.to_string());
                        }
                        None => out.push(self.library_track(p)),
                    }
                }
                let mut v: Vec<Entry> = folders
                    .into_iter()
                    .map(|name| Entry {
                        path: self.root.join(format!("{prefix}{name}")),
                        name,
                        kind: Kind::Folder,
                        entry: None,
                    })
                    .collect();
                v.extend(out);
                v
            }
            _ => Vec::new(),
        }
    }

    /// The tracks a search from `level` looks through.
    pub(super) fn search_scope(&self, level: &Level) -> Vec<Entry> {
        match &level.kind {
            LevelKind::Folder => walk(&level.dir)
                .into_iter()
                .map(|path| Entry {
                    name: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path,
                    kind: Kind::Track,
                    entry: None,
                })
                .collect(),
            LevelKind::Playlist { .. } => self.library_entries(&level.kind),
            LevelKind::LibraryFolder { prefix } => {
                let mut k: Vec<&String> = self
                    .listing
                    .iter()
                    .flat_map(|l| l.keys())
                    .filter(|p| p.starts_with(prefix.as_str()))
                    .collect();
                k.sort();
                k.into_iter().map(|p| self.library_track(p)).collect()
            }
            LevelKind::LibraryMenu | LevelKind::LibraryTracks | LevelKind::Playlists { .. } => {
                self.library_entries(&LevelKind::LibraryTracks)
            }
            LevelKind::Search { .. } => level.all.clone(),
        }
    }

    /// What the library knows, as it last said: its tags win over the files' BPM and key,
    /// and the library view lists its tracks.
    pub fn set_listing(&mut self, listing: Arc<Listing>) {
        if self
            .listing
            .as_ref()
            .is_some_and(|l| Arc::ptr_eq(l, &listing))
        {
            return;
        }
        self.listing = Some(listing);
        self.rebuild();
        self.ordered = None;
    }

    /// What the library's playlists are, as it last said.
    pub fn set_playlists(&mut self, playlists: Arc<Playlists>) {
        if self
            .playlists
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p, &playlists))
        {
            return;
        }
        self.tagged = playlists.tagged().collect();
        self.playlists = Some(playlists);
        self.rebuild();
        self.ordered = None;
    }

    /// The library's files by track (a track's first location).
    fn files_by_track(&self) -> HashMap<TrackId, &str> {
        let mut out: HashMap<TrackId, &str> = HashMap::new();
        for l in self.listing.iter().flat_map(|l| l.values()) {
            let e = out.entry(l.track).or_insert(&l.path);
            if l.path.as_str() < *e {
                *e = &l.path;
            }
        }
        out
    }

    /// Rebuild the library's levels from the listing and the playlists.
    fn rebuild(&mut self) {
        if self.source != Source::Folders {
            // Every library level (and a search over one) takes the new listing.
            for i in 0..self.levels.len() {
                let all = match &self.levels[i].kind {
                    LevelKind::Search { .. } if i > 0 => self.search_scope(&self.levels[i - 1]),
                    LevelKind::Folder | LevelKind::Search { .. } => continue,
                    kind => self.library_entries(kind),
                };
                self.levels[i].all = all;
            }
            let all = self.level().all.clone();
            self.tags.want(&all);
        }
    }

    /// The library's track for `path`, if it has one.
    pub fn track_of(&self, path: &Path) -> Option<TrackId> {
        self.listed(path).map(|l| l.track)
    }

    /// `path`'s track is on the tag list.
    pub fn is_tagged(&self, path: &Path) -> bool {
        self.track_of(path)
            .is_some_and(|t| self.tagged.contains(&t))
    }

    /// The tag list's entries of `path`'s track (to remove it).
    pub fn tag_entries(&self, path: &Path) -> Vec<EntryId> {
        let Some(t) = self.track_of(path) else {
            return Vec::new();
        };
        self.playlists
            .iter()
            .flat_map(|p| p.entries.get(&PlaylistId::TAG_LIST))
            .flatten()
            .filter(|e| e.track == t)
            .map(|e| e.id)
            .collect()
    }

    /// The playlists, as the library last said.
    pub fn playlists(&self) -> Option<&Arc<Playlists>> {
        self.playlists.as_ref()
    }

    /// The playlist shown (a list or the tag list), if the level is one.
    pub fn shown_playlist(&self) -> Option<PlaylistId> {
        match self.level().kind {
            LevelKind::Playlist { id } => Some(id),
            _ => None,
        }
    }

    /// The playlist folder shown (`Some(None)`: the top of PLAYLISTS), if the level is one.
    pub fn shown_playlist_folder(&self) -> Option<Option<PlaylistId>> {
        match self.level().kind {
            LevelKind::Playlists { parent } => Some(parent),
            _ => None,
        }
    }

    /// What is known of `path`: the library's tag cache and the file's own tags, the
    /// library's BPM and key first (they are ours once analysed). `None` until either is
    /// known.
    pub fn info(&self, path: &Path) -> Option<Tags> {
        let file = self.tags.get(path).and_then(|r| r.tags);
        let listed = self.listed(path);
        let lib = listed.map(|l| &l.tags);
        match (file, lib) {
            (None, None) => None,
            (f, l) => {
                let mut t = f.unwrap_or_default();
                if let Some(l) = l {
                    t.title = t.title.or_else(|| l.title.clone());
                    t.artist = t.artist.or_else(|| l.artist.clone());
                    t.album = t.album.or_else(|| l.album.clone());
                    t.genre = t.genre.or_else(|| l.genre.clone());
                    t.duration = t.duration.or(l.duration);
                    t.bpm = l.bpm.or(t.bpm);
                    t.key = l.key.clone().or(t.key);
                }
                if let Some(r) = listed.and_then(|l| l.rating) {
                    t.rating = Some(r);
                }
                Some(t)
            }
        }
    }

    /// The library's record of `path`, if it has one.
    pub(crate) fn listed(&self, path: &Path) -> Option<&library::Listed> {
        self.listing
            .as_ref()?
            .get(&stored_path(path, self.root_canon.as_deref()))
    }
}
