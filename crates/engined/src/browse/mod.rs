//! The browser of a stick: its folder tree, or the tracks its library knows; sorted, and
//! searched; with a cursor per level, and the files' tags and artwork read on a background
//! thread.
//!
//! A stick without our library offers the folder tree (docs/plan/library.md): nothing is
//! scanned unless the DJ asks, and a file gets its library record when it is loaded. Listing
//! a folder reads only the directory; the tags of its files follow, current folder first,
//! and are kept for the session. What the library knows (its tag cache, with our analysed
//! BPM and key) comes in as a [`Listing`] and wins over the files' own BPM and key.

mod files;
mod level;
mod order;
mod tags;
#[cfg(test)]
mod tests;
mod view;

pub use crate::stick::walk;
pub(crate) use level::LevelKind;
pub use level::{Entered, Entry, Kind, Level, Sort, Source};
pub use tags::{LARGE_SIZE, Tags, Thumb, read_tags, thumbnail};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use library::{PlaylistId, PlaylistKind, TrackId};

use crate::store::{Listing, Playlists};
use order::{matches, order};
use tags::TagReader;
use view::{MENU_FOLDERS, MENU_PLAYLISTS};

/// The browser of one stick root.
pub struct Browser {
    root: PathBuf,
    root_canon: Option<PathBuf>,
    source: Source,
    /// The source's top level first; the last is the one shown.
    levels: Vec<Level>,
    tags: TagReader,
    sort: Sort,
    /// Rows shown at once: the cursor is kept among them.
    rows: usize,
    listing: Option<Arc<Listing>>,
    playlists: Option<Arc<Playlists>>,
    /// The tracks on the tag list.
    tagged: std::collections::HashSet<TrackId>,
    /// The tag generation the shown level was last ordered with, if it is up to date.
    ordered: Option<u64>,
}

impl Browser {
    /// Browse `root` (the stick's root folder) `rows` rows at a time, starting with its
    /// folders.
    pub fn new(root: PathBuf, rows: usize) -> Self {
        let mut b = Self {
            root_canon: root.canonicalize().ok(),
            root,
            source: Source::Folders,
            levels: Vec::new(),
            tags: TagReader::start(),
            sort: Sort::Order,
            rows: rows.max(1),
            listing: None,
            playlists: None,
            tagged: Default::default(),
            ordered: None,
        };
        b.set_source(Source::Folders);
        b
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Show the folder tree or the library's tracks, from their top.
    pub fn set_source(&mut self, source: Source) {
        self.source = source;
        let level = match source {
            Source::Folders => Level::folder(self.root.clone(), true),
            Source::Library => self.library_level(),
            Source::TagList => {
                let kind = LevelKind::Playlist {
                    id: PlaylistId::TAG_LIST,
                };
                let all = self.library_entries(&kind);
                Level::new(kind, "TAG LIST".into(), self.root.clone(), all, None)
            }
        };
        self.tags.want(&level.all);
        self.levels = vec![level];
        self.ordered = None;
        self.refresh();
    }

    /// Re-sort (and re-filter) the shown level if tags arrived since; call once per frame.
    /// The cursor stays on its entry.
    pub fn refresh(&mut self) {
        let generation = self.tags.generation();
        if self.ordered == Some(generation) {
            return;
        }
        self.ordered = Some(generation);
        let sort = self.sort;
        let rows = self.rows;
        let Some(level) = self.levels.last() else {
            return;
        };
        let words: Vec<String> = match &level.kind {
            LevelKind::Search { query } => {
                query.split_whitespace().map(str::to_lowercase).collect()
            }
            _ => Vec::new(),
        };
        let mut keyed: Vec<(Entry, Option<Tags>)> = level
            .all
            .iter()
            .map(|e| (e.clone(), self.info(&e.path)))
            .filter(|(e, info)| words.is_empty() || matches(&words, e, info.as_ref()))
            .collect();
        // Playlists (and their folders) keep their own order under `#`, and the library menu
        // always does;
        // anything else lists by name.
        let fixed = match level.kind {
            LevelKind::LibraryMenu => true,
            LevelKind::Playlist { .. } | LevelKind::Playlists { .. } => sort == Sort::Order,
            _ => false,
        };
        if !fixed {
            keyed.sort_by(|(a, ia), (b, ib)| order(sort, a, ia.as_ref(), b, ib.as_ref()));
        }
        let level = self.levels.last_mut().expect("checked above");
        // By file and playlist entry: a list may hold the same track twice.
        let keep = level
            .selected()
            .filter(|_| level.touched)
            .map(|e| (e.path.clone(), e.entry));
        level.entries = keyed.into_iter().map(|(e, _)| e).collect();
        level.cursor = keep
            .and_then(|(p, entry)| {
                level
                    .entries
                    .iter()
                    .position(|e| e.path == p && e.entry == entry)
            })
            .unwrap_or(0)
            .min(level.entries.len().saturating_sub(1));
        level.top = level.top.min(level.entries.len().saturating_sub(rows));
        Self::keep_visible(level, rows);
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// The sort order's name for the shown level: `#` means the playlist's own order, and
    /// reads as NAME where the level has none.
    pub fn sort_label(&self) -> &'static str {
        match (self.sort, &self.level().kind) {
            (Sort::Order, LevelKind::Playlist { .. } | LevelKind::Playlists { .. }) => {
                Sort::Order.label()
            }
            (Sort::Order, _) => Sort::Name.label(),
            (s, _) => s.label(),
        }
    }

    pub fn set_sort(&mut self, sort: Sort) {
        if sort != self.sort {
            self.sort = sort;
            self.ordered = None;
            self.refresh();
        }
    }

    /// The level shown.
    pub fn level(&self) -> &Level {
        self.levels.last().expect("the top level is never popped")
    }

    fn level_mut(&mut self) -> &mut Level {
        self.levels
            .last_mut()
            .expect("the top level is never popped")
    }

    /// The level names below the top one, down to the shown level (empty at the top).
    pub fn trail(&self) -> Vec<&str> {
        self.levels[1..].iter().map(|l| l.name.as_str()).collect()
    }

    pub fn at_root(&self) -> bool {
        self.levels.len() == 1
    }

    /// Move the cursor by `delta` rows (clamped), keeping it among the rows shown.
    pub fn move_by(&mut self, delta: isize) {
        let rows = self.rows;
        let l = self.level_mut();
        if l.entries.is_empty() {
            return;
        }
        let last = l.entries.len() - 1;
        l.touched = true;
        l.cursor = l.cursor.saturating_add_signed(delta).min(last);
        Self::keep_visible(l, rows);
    }

    /// Put the cursor on entry `i` (a touch on its row).
    pub fn select(&mut self, i: usize) {
        let rows = self.rows;
        let l = self.level_mut();
        if i < l.entries.len() {
            l.touched = true;
            l.cursor = i;
            Self::keep_visible(l, rows);
        }
    }

    /// Scroll the list by `delta` rows without moving past either end; the cursor stays on
    /// its entry if that is still shown, else moves to the nearest shown row.
    pub fn scroll_by(&mut self, delta: isize) {
        let rows = self.rows;
        let l = self.level_mut();
        let max_top = l.entries.len().saturating_sub(rows);
        l.touched = true;
        l.top = l.top.saturating_add_signed(delta).min(max_top);
        if !l.entries.is_empty() {
            let bottom = (l.top + rows).min(l.entries.len()) - 1;
            l.cursor = l.cursor.clamp(l.top, bottom.max(l.top));
        }
    }

    fn keep_visible(l: &mut Level, rows: usize) {
        let rows = rows.max(1);
        if l.cursor < l.top {
            l.top = l.cursor;
        } else if l.cursor >= l.top + rows {
            l.top = l.cursor + 1 - rows;
        }
    }

    /// Open the selected folder, or hand back the selected track to load.
    pub fn enter(&mut self) -> Entered {
        let Some(e) = self.level().selected().cloned() else {
            return Entered::Nothing;
        };
        match e.kind {
            Kind::Track if e.path.as_os_str().is_empty() => Entered::Nothing,
            Kind::Track => Entered::Track(e.path),
            Kind::Playlist(id) => {
                let folder = self
                    .playlists
                    .as_ref()
                    .and_then(|p| p.get(id))
                    .is_some_and(|p| p.kind == PlaylistKind::Folder);
                let kind = if folder {
                    LevelKind::Playlists { parent: Some(id) }
                } else {
                    LevelKind::Playlist { id }
                };
                let all = self.library_entries(&kind);
                self.tags.want(&all);
                self.levels.push(Level::new(
                    kind,
                    e.name.clone(),
                    self.root.clone(),
                    all,
                    None,
                ));
                self.ordered = None;
                self.refresh();
                Entered::Folder
            }
            Kind::Folder => {
                let kind = match &self.level().kind {
                    LevelKind::LibraryMenu if e.name == MENU_PLAYLISTS => {
                        Some(LevelKind::Playlists { parent: None })
                    }
                    LevelKind::LibraryMenu if e.name == MENU_FOLDERS => {
                        Some(LevelKind::LibraryFolder {
                            prefix: String::new(),
                        })
                    }
                    LevelKind::LibraryMenu => Some(LevelKind::LibraryTracks),
                    LevelKind::LibraryFolder { prefix } => Some(LevelKind::LibraryFolder {
                        prefix: format!("{prefix}{}/", e.name),
                    }),
                    _ => None,
                };
                let level = match kind {
                    Some(kind) => {
                        let all = self.library_entries(&kind);
                        Level::new(kind, e.name.clone(), self.root.clone(), all, None)
                    }
                    None => Level::folder(e.path, false),
                };
                self.tags.want(&level.all);
                self.levels.push(level);
                self.ordered = None;
                self.refresh();
                Entered::Folder
            }
        }
    }

    /// Up one level (out of a search first); false at the top.
    pub fn back(&mut self) -> bool {
        if self.at_root() {
            return false;
        }
        self.levels.pop();
        let all = self.level().all.clone();
        self.tags.want(&all);
        self.ordered = None;
        self.refresh();
        true
    }

    /// Search the tracks under the shown folder (or the whole library), starting with an
    /// empty query that matches them all.
    pub fn search_begin(&mut self) {
        if self.search_query().is_some() {
            return;
        }
        let all = self.search_scope(self.level());
        self.tags.want(&all);
        let dir = self.level().dir.clone();
        self.levels.push(Level::new(
            LevelKind::Search {
                query: String::new(),
            },
            "SEARCH".into(),
            dir,
            all,
            None,
        ));
        self.ordered = None;
        self.refresh();
    }

    /// The query, while searching.
    pub fn search_query(&self) -> Option<&str> {
        match &self.level().kind {
            LevelKind::Search { query } => Some(query),
            _ => None,
        }
    }

    pub fn set_query(&mut self, q: &str) {
        if let LevelKind::Search { query } = &mut self.level_mut().kind
            && query != q
        {
            *query = q.to_string();
            let l = self.level_mut();
            l.cursor = 0;
            l.top = 0;
            self.ordered = None;
            self.refresh();
        }
    }

    /// Leave the search, back to where it started.
    pub fn search_end(&mut self) {
        if self.search_query().is_some() {
            self.back();
        }
    }

    /// `path`'s artwork at [`LARGE_SIZE`], for the info panel: read on the tag thread, one
    /// file at a time. `None` until it is read (asking for it), and `Some(None)` if the file
    /// has none.
    pub fn large_art(&self, path: &Path) -> Option<Option<Arc<Thumb>>> {
        self.tags.large(path)
    }

    /// True once `path`'s tags were read (or the library knows them).
    pub fn is_read(&self, path: &Path) -> bool {
        self.tags.get(path).is_some()
    }

    /// The file's artwork, once read.
    pub fn art(&self, path: &Path) -> Option<Arc<Thumb>> {
        self.tags.get(path).and_then(|r| r.art)
    }
}
