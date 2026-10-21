//! The browser of a stick: its folder tree, or the tracks its library knows; sorted, and
//! searched; with a cursor per level, and the files' tags and artwork read on a background
//! thread.
//!
//! A stick without our library offers the folder tree (docs/plan/library.md): nothing is
//! scanned unless the DJ asks, and a file gets its library record when it is loaded. Listing
//! a folder reads only the directory; the tags of its files follow, current folder first,
//! and are kept for the session. What the library knows (its tag cache, with our analysed
//! BPM and key) comes in as a [`Listing`] and wins over the files' own BPM and key.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

pub use engine_media::{Tags, tags as read_tags};

use crate::persist::{LIBRARY_DIR, Listing, stored_path};

/// File extensions the browser lists as tracks (lower case).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "aif", "aiff", "flac", "m4a", "mp4", "aac", "alac", "ogg", "oga",
];

/// Folders at a stick's root that hold no tracks: ours, and Rekordbox's database.
const HIDDEN_AT_ROOT: &[&str] = &[LIBRARY_DIR, "PIONEER", "System Volume Information"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Track,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

/// The folders and tracks in `dir`: folders first, then tracks, each sorted by name without
/// regard to case. Hidden files are left out, and at the root (`root`) the folders that hold
/// no tracks.
pub fn list(dir: &Path, root: bool) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || (root && HIDDEN_AT_ROOT.contains(&name.as_str())) {
            continue;
        }
        let path = e.path();
        // Follows symlinks: a linked folder browses like a folder.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let kind = if meta.is_dir() {
            Kind::Folder
        } else if meta.is_file() && is_audio(&path) {
            Kind::Track
        } else {
            continue;
        };
        out.push(Entry { name, path, kind });
    }
    out.sort_by(|a, b| {
        (a.kind == Kind::Track)
            .cmp(&(b.kind == Kind::Track))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// Every audio file under `root`, in path order, skipping what [`list`] hides. Links to
/// folders are not followed, so a link loop can't make it run forever.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), true)];
    while let Some((dir, at_root)) = dirs.pop() {
        let Ok(entries) = list(&dir, at_root) else {
            continue;
        };
        // Folders come first in a listing; pushed in reverse they are walked in order, after
        // this folder's tracks.
        let mut sub = Vec::new();
        for e in entries {
            match e.kind {
                Kind::Track => out.push(e.path),
                Kind::Folder => {
                    let is_link = std::fs::symlink_metadata(&e.path)
                        .is_ok_and(|m| m.file_type().is_symlink());
                    if !is_link {
                        sub.push((e.path, false));
                    }
                }
            }
        }
        dirs.extend(sub.into_iter().rev());
    }
    out.sort();
    out
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// How tracks are ordered. Folders always come first, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// By file name.
    #[default]
    Name,
    Title,
    Artist,
    Bpm,
    Key,
    Length,
}

impl Sort {
    pub const ALL: [Sort; 6] = [
        Sort::Name,
        Sort::Title,
        Sort::Artist,
        Sort::Bpm,
        Sort::Key,
        Sort::Length,
    ];

    pub fn label(self) -> &'static str {
        match self {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LevelKind {
    /// A folder on the stick.
    Folder,
    /// The library's top: FOLDERS and TRACKS.
    LibraryMenu,
    /// Every track the library knows, in one list.
    LibraryTracks,
    /// The library's tracks under the folder `prefix` of their stored paths (`""` for the
    /// stick's root, else ending in `/`): its subfolders, then its tracks.
    LibraryFolder { prefix: String },
    /// The tracks under the level below whose name or tags hold every word of `query`.
    Search { query: String },
}

/// The library menu's entries.
const MENU_FOLDERS: &str = "FOLDERS";
const MENU_TRACKS: &str = "TRACKS";

/// One level of the list: its entries in order, the cursor and the first row shown.
#[derive(Debug, Clone)]
pub struct Level {
    pub kind: LevelKind,
    /// The level's name in the header.
    pub name: String,
    pub dir: PathBuf,
    /// Everything in the level, before sorting and searching.
    all: Vec<Entry>,
    /// What is shown, sorted (and filtered, when searching).
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub top: usize,
    /// Why the folder could not be read, if it couldn't.
    pub error: Option<String>,
    /// The DJ moved the cursor or scrolled here; until then a re-sort keeps the top in view
    /// rather than following an entry nobody picked.
    touched: bool,
}

impl Level {
    fn folder(dir: PathBuf, root: bool) -> Self {
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

    fn new(
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

/// Artwork scaled down for the jacket column: square RGBA8.
#[derive(Debug)]
pub struct Thumb {
    /// Unique per picture, for the renderer's texture cache.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Side of a [`Thumb`] for the list, pixels.
pub const THUMB_SIZE: u32 = 64;
/// Side of the selected track's artwork in the info panel, pixels.
pub const LARGE_SIZE: u32 = 288;

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
            sort: Sort::Name,
            rows: rows.max(1),
            listing: None,
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
        };
        self.tags.want(&level.all);
        self.levels = vec![level];
        self.ordered = None;
        self.refresh();
    }

    fn library_level(&self) -> Level {
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
        }
    }

    /// The entries of a library level, from the listing.
    fn library_entries(&self, kind: &LevelKind) -> Vec<Entry> {
        let keys = || {
            let mut k: Vec<&String> = self.listing.iter().flat_map(|l| l.keys()).collect();
            k.sort();
            k
        };
        match kind {
            LevelKind::LibraryMenu => [MENU_FOLDERS, MENU_TRACKS]
                .into_iter()
                .map(|name| Entry {
                    name: name.into(),
                    path: PathBuf::from(format!("library:{name}")),
                    kind: Kind::Folder,
                })
                .collect(),
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
                    })
                    .collect();
                v.extend(out);
                v
            }
            _ => Vec::new(),
        }
    }

    /// The tracks a search from `level` looks through.
    fn search_scope(&self, level: &Level) -> Vec<Entry> {
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
                })
                .collect(),
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
            LevelKind::LibraryMenu | LevelKind::LibraryTracks => {
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
        if self.source == Source::Library {
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
        self.ordered = None;
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
        keyed.sort_by(|(a, ia), (b, ib)| order(sort, a, ia.as_ref(), b, ib.as_ref()));
        let level = self.levels.last_mut().expect("checked above");
        let keep = level
            .selected()
            .filter(|_| level.touched)
            .map(|e| e.path.clone());
        level.entries = keyed.into_iter().map(|(e, _)| e).collect();
        level.cursor = keep
            .and_then(|p| level.entries.iter().position(|e| e.path == p))
            .unwrap_or(0)
            .min(level.entries.len().saturating_sub(1));
        level.top = level.top.min(level.entries.len().saturating_sub(rows));
        Self::keep_visible(level, rows);
    }

    pub fn sort(&self) -> Sort {
        self.sort
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
            Kind::Track => Entered::Track(e.path),
            Kind::Folder => {
                let kind = match &self.level().kind {
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
    pub fn listed(&self, path: &Path) -> Option<&library::Listed> {
        self.listing
            .as_ref()?
            .get(&stored_path(path, self.root_canon.as_deref()))
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

/// Every word appears in the entry's name or tags (lower case).
fn matches(words: &[String], e: &Entry, info: Option<&Tags>) -> bool {
    let mut hay = e.name.to_lowercase();
    if let Some(t) = info {
        for s in [&t.title, &t.artist, &t.album, &t.genre]
            .into_iter()
            .flatten()
        {
            hay.push(' ');
            hay.push_str(&s.to_lowercase());
        }
    }
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// Folders first by name; tracks by `sort`, unknown values last, then by name.
fn order(
    sort: Sort,
    a: &Entry,
    ia: Option<&Tags>,
    b: &Entry,
    ib: Option<&Tags>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let by_name = || {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    };
    let folders = (a.kind == Kind::Track).cmp(&(b.kind == Kind::Track));
    if folders != Ordering::Equal || a.kind == Kind::Folder {
        return folders.then_with(by_name);
    }
    // Some before None, so the unknown come last.
    fn known<T>(a: Option<T>, b: Option<T>, cmp: impl Fn(T, T) -> Ordering) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => cmp(a, b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    let text = |t: Option<&Tags>, f: fn(&Tags) -> &Option<String>| {
        t.and_then(|t| f(t).as_ref()).map(|s| s.to_lowercase())
    };
    let title = |t: Option<&Tags>, e: &Entry| {
        text(t, |t| &t.title).unwrap_or_else(|| e.name.to_lowercase())
    };
    let num = |t: Option<&Tags>, f: fn(&Tags) -> Option<f64>| t.and_then(f);
    let ord = match sort {
        Sort::Name => Ordering::Equal,
        Sort::Title => title(ia, a).cmp(&title(ib, b)),
        Sort::Artist => known(text(ia, |t| &t.artist), text(ib, |t| &t.artist), |x, y| {
            x.cmp(&y)
        })
        .then_with(|| title(ia, a).cmp(&title(ib, b))),
        Sort::Bpm => known(num(ia, |t| t.bpm), num(ib, |t| t.bpm), |x, y| {
            x.total_cmp(&y)
        }),
        Sort::Key => known(text(ia, |t| &t.key), text(ib, |t| &t.key), |x, y| x.cmp(&y)),
        Sort::Length => known(num(ia, |t| t.duration), num(ib, |t| t.duration), |x, y| {
            x.total_cmp(&y)
        }),
    };
    ord.then_with(by_name)
}

/// A file's tags (without the picture) and its artwork thumbnail; `tags` is `None` if the
/// file could not be read.
#[derive(Debug, Clone)]
struct Read {
    tags: Option<Tags>,
    art: Option<Arc<Thumb>>,
}

/// Reads tags on its own thread: the wanted files in order, the newest wish first.
struct TagReader {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<TagState>,
    wake: Condvar,
    /// Bumped whenever a file's tags arrive.
    generation: AtomicU64,
}

#[derive(Default)]
struct TagState {
    queue: VecDeque<PathBuf>,
    done: HashMap<PathBuf, Read>,
    /// The file whose large artwork is wanted, and the last one read.
    large_wanted: Option<PathBuf>,
    large: Option<(PathBuf, Option<Arc<Thumb>>)>,
    stop: bool,
}

impl TagReader {
    fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let s = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("tags".into())
            .spawn(move || tag_thread(&s))
            .ok();
        Self { shared, thread }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TagState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the tracks among `entries` next, before anything wanted earlier.
    fn want(&self, entries: &[Entry]) {
        let mut s = self.lock();
        let fresh: Vec<PathBuf> = entries
            .iter()
            .filter(|e| e.kind == Kind::Track && !s.done.contains_key(&e.path))
            .map(|e| e.path.clone())
            .collect();
        let set: std::collections::HashSet<&PathBuf> = fresh.iter().collect();
        s.queue.retain(|p| !set.contains(p));
        for p in fresh.iter().rev() {
            s.queue.push_front(p.clone());
        }
        self.shared.wake.notify_one();
    }

    fn get(&self, path: &Path) -> Option<Read> {
        self.lock().done.get(path).cloned()
    }

    fn large(&self, path: &Path) -> Option<Option<Arc<Thumb>>> {
        let mut s = self.lock();
        if let Some((p, art)) = &s.large
            && p == path
        {
            return Some(art.clone());
        }
        if s.large_wanted.as_deref() != Some(path) {
            s.large_wanted = Some(path.to_path_buf());
            self.shared.wake.notify_one();
        }
        None
    }

    fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
}

impl Drop for TagReader {
    fn drop(&mut self) {
        self.lock().stop = true;
        self.shared.wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn tag_thread(shared: &Shared) {
    let lock = || shared.state.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        // The info panel's picture first: someone is looking at it.
        let (path, large) = {
            let mut s = lock();
            loop {
                if s.stop {
                    return;
                }
                if let Some(p) = s.large_wanted.take() {
                    break (p, true);
                }
                if let Some(p) = s.queue.pop_front() {
                    break (p, false);
                }
                s = shared.wake.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let mut tags = engine_media::tags(&path).ok();
        let artwork = tags.as_mut().and_then(|t| t.artwork.take());
        if large {
            let art = artwork
                .and_then(|a| thumbnail(&a.data, LARGE_SIZE))
                .map(Arc::new);
            lock().large = Some((path, art));
            shared.generation.fetch_add(1, Ordering::Release);
            continue;
        }
        let art = artwork
            .and_then(|a| thumbnail(&a.data, THUMB_SIZE))
            .map(Arc::new);
        lock().done.insert(path, Read { tags, art });
        shared.generation.fetch_add(1, Ordering::Release);
    }
}

/// Decode a picture and scale it to fill a `size` square (centre-cropped).
pub fn thumbnail(data: &[u8], size: u32) -> Option<Thumb> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let img = image::load_from_memory(data).ok()?;
    let small = img
        .resize_to_fill(size, size, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Some(Thumb {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        width: small.width(),
        height: small.height(),
        rgba: small.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("browse-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path) {
        std::fs::write(p, b"").unwrap();
    }

    fn names(b: &Browser) -> Vec<&str> {
        b.level().entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// Wait until every file under the shown level has been read.
    fn settle(b: &mut Browser) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !b
            .level()
            .all
            .iter()
            .all(|e| e.kind == Kind::Folder || b.is_read(&e.path))
        {
            assert!(std::time::Instant::now() < until, "tags never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        b.refresh();
    }

    #[test]
    fn lists_folders_then_tracks_and_hides_what_holds_no_tracks() {
        let d = temp_dir("list");
        for f in ["b.mp3", "A.WAV", "notes.txt", ".hidden.mp3"] {
            touch(&d.join(f));
        }
        for f in ["Zed", "alpha", LIBRARY_DIR, "PIONEER", ".Trashes"] {
            std::fs::create_dir(d.join(f)).unwrap();
        }
        let names = |root| -> Vec<String> {
            list(&d, root)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect()
        };
        assert_eq!(names(true), ["alpha", "Zed", "A.WAV", "b.mp3"]);
        assert_eq!(
            names(false),
            ["alpha", LIBRARY_DIR, "PIONEER", "Zed", "A.WAV", "b.mp3"]
        );
    }

    #[test]
    fn walks_every_track_below_a_folder() {
        let d = temp_dir("walk");
        std::fs::create_dir_all(d.join("x/y")).unwrap();
        std::fs::create_dir_all(d.join(LIBRARY_DIR)).unwrap();
        for f in [
            "a.mp3",
            "x/b.mp3",
            "x/y/c.wav",
            "x/notes.txt",
            "DJPLAYER/d.mp3",
        ] {
            touch(&d.join(f));
        }
        let rel: Vec<PathBuf> = walk(&d)
            .into_iter()
            .map(|p| p.strip_prefix(&d).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            rel,
            [
                PathBuf::from("a.mp3"),
                PathBuf::from("x/b.mp3"),
                PathBuf::from("x/y/c.wav")
            ]
        );
    }

    #[test]
    fn navigates_down_and_back_keeping_the_cursor() {
        let d = temp_dir("nav");
        std::fs::create_dir(d.join("crate")).unwrap();
        for i in 0..10 {
            touch(&d.join(format!("t{i:02}.mp3")));
        }
        touch(&d.join("crate").join("inner.mp3"));
        let mut b = Browser::new(d.clone(), 4);
        assert_eq!(b.level().entries.len(), 11);
        b.move_by(8);
        assert_eq!(b.level().cursor, 8);
        assert_eq!(b.level().top, 5, "the cursor stays among the 4 rows shown");
        assert_eq!(
            b.enter(),
            Entered::Track(d.join("t07.mp3")),
            "folders first: row 8 is the 8th track"
        );
        b.move_by(-100);
        assert_eq!((b.level().cursor, b.level().top), (0, 0));
        assert_eq!(b.enter(), Entered::Folder);
        assert_eq!(b.trail(), ["crate"]);
        assert_eq!(b.level().entries[0].name, "inner.mp3");
        assert!(b.back());
        assert!(!b.back(), "the root has no parent");
        assert_eq!(b.level().selected().unwrap().name, "crate");
    }

    #[test]
    fn scrolling_clamps_and_drags_the_cursor_along() {
        let d = temp_dir("scroll");
        for i in 0..10 {
            touch(&d.join(format!("t{i}.mp3")));
        }
        let mut b = Browser::new(d, 4);
        b.scroll_by(3);
        assert_eq!((b.level().top, b.level().cursor), (3, 3));
        b.scroll_by(100);
        assert_eq!((b.level().top, b.level().cursor), (6, 6));
        b.select(9);
        b.scroll_by(-100);
        assert_eq!((b.level().top, b.level().cursor), (0, 3));
    }

    #[test]
    fn unreadable_tags_arrive_as_read() {
        let d = temp_dir("tags");
        // Not audio: read, but unreadable.
        touch(&d.join("broken.mp3"));
        let mut b = Browser::new(d.clone(), 7);
        settle(&mut b);
        assert!(b.is_read(&d.join("broken.mp3")));
        assert_eq!(b.info(&d.join("broken.mp3")), None);
    }

    /// A library listing for `d` with these (path, title, artist, bpm) rows.
    fn listing(rows: &[(&str, &str, &str, f64)]) -> Arc<Listing> {
        Arc::new(
            rows.iter()
                .map(|&(p, title, artist, bpm)| {
                    (
                        p.to_string(),
                        library::Listed {
                            track: library::TrackId([0; 16]),
                            path: p.to_string(),
                            tags: library::TrackTags {
                                title: Some(title.into()),
                                artist: Some(artist.into()),
                                bpm: Some(bpm),
                                ..Default::default()
                            },
                            rating: Some(3),
                            analysed: false,
                        },
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn sorts_by_the_library_tags_and_keeps_the_cursor_on_its_track() {
        let d = temp_dir("sort");
        std::fs::create_dir(d.join("crate")).unwrap();
        for f in ["a.mp3", "b.mp3", "c.mp3"] {
            touch(&d.join(f));
        }
        let mut b = Browser::new(d.clone(), 7);
        b.set_listing(listing(&[
            ("a.mp3", "Zulu", "Mid", 128.0),
            ("b.mp3", "Alpha", "Zed", 90.0),
            ("c.mp3", "Mike", "Able", 174.0),
        ]));
        settle(&mut b);
        b.select(3); // c.mp3
        b.set_sort(Sort::Title);
        assert_eq!(names(&b), ["crate", "b.mp3", "c.mp3", "a.mp3"]);
        assert_eq!(b.level().selected().unwrap().name, "c.mp3");
        b.set_sort(Sort::Artist);
        assert_eq!(names(&b), ["crate", "c.mp3", "a.mp3", "b.mp3"]);
        b.set_sort(Sort::Bpm);
        assert_eq!(names(&b), ["crate", "b.mp3", "a.mp3", "c.mp3"]);
        assert_eq!(b.info(&d.join("a.mp3")).unwrap().bpm, Some(128.0));
        assert_eq!(
            b.info(&d.join("a.mp3")).unwrap().rating,
            Some(3),
            "the library's rating"
        );
        // A file with no picture: the large artwork comes back as none.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while b.large_art(&d.join("a.mp3")).is_none() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(b.large_art(&d.join("a.mp3")).unwrap().is_none());
        assert_eq!(Sort::Length.next(), Sort::Name);
    }

    #[test]
    fn searches_below_the_folder_by_name_and_tags() {
        let d = temp_dir("search");
        std::fs::create_dir_all(d.join("UKG/deep")).unwrap();
        for f in ["flowers.mp3", "UKG/red flag.mp3", "UKG/deep/true.mp3"] {
            touch(&d.join(f));
        }
        let mut b = Browser::new(d.clone(), 7);
        b.set_listing(listing(&[("UKG/deep/true.mp3", "True", "Jaimeson", 130.0)]));
        b.search_begin();
        settle(&mut b);
        assert_eq!(
            b.level().entries.len(),
            3,
            "an empty query matches everything"
        );
        b.set_query("RED");
        assert_eq!(names(&b), ["red flag.mp3"]);
        b.set_query("jaim tru");
        assert_eq!(names(&b), ["true.mp3"]);
        b.set_query("nothing here");
        assert!(b.level().entries.is_empty());
        b.search_end();
        assert!(b.search_query().is_none() && b.at_root());
    }

    #[test]
    fn the_library_source_offers_its_folders_and_every_track() {
        let d = temp_dir("libsrc");
        let mut b = Browser::new(d.clone(), 7);
        b.set_listing(listing(&[
            ("b.mp3", "B", "x", 1.0),
            ("UKG/a.mp3", "A", "y", 2.0),
            ("UKG/deep/c.mp3", "C", "z", 3.0),
        ]));
        b.set_source(Source::Library);
        assert_eq!(b.level().kind, LevelKind::LibraryMenu);
        assert_eq!(names(&b), ["FOLDERS", "TRACKS"]);

        // FOLDERS: the stick's structure, from the stored paths.
        assert_eq!(b.enter(), Entered::Folder);
        assert_eq!(names(&b), ["UKG", "b.mp3"]);
        assert_eq!(b.trail(), ["FOLDERS"]);
        assert_eq!(b.enter(), Entered::Folder);
        assert_eq!(names(&b), ["deep", "a.mp3"]);
        assert_eq!(b.trail(), ["FOLDERS", "UKG"]);
        b.select(1);
        assert_eq!(b.enter(), Entered::Track(d.join("UKG/a.mp3")));
        // A search here looks under UKG only.
        b.search_begin();
        assert_eq!(b.level().entries.len(), 2);
        b.search_end();
        assert!(b.back() && b.back());

        // TRACKS: one flat list.
        b.select(1);
        assert_eq!(b.enter(), Entered::Folder);
        assert_eq!(b.level().kind, LevelKind::LibraryTracks);
        assert_eq!(names(&b), ["a.mp3", "b.mp3", "c.mp3"]);

        // Tracks arriving (a scan): an untouched list stays at its top; a picked track
        // keeps the cursor.
        b.set_listing(listing(&[
            ("b.mp3", "B", "x", 1.0),
            ("UKG/a.mp3", "A", "y", 2.0),
            ("UKG/deep/c.mp3", "C", "z", 3.0),
            ("0.mp3", "0", "z", 3.0),
        ]));
        b.refresh();
        assert_eq!((b.level().cursor, names(&b)[0]), (0, "0.mp3"));
        b.select(2);
        b.set_listing(listing(&[
            ("b.mp3", "B", "x", 1.0),
            ("UKG/a.mp3", "A", "y", 2.0),
            ("UKG/deep/c.mp3", "C", "z", 3.0),
            ("0.mp3", "0", "z", 3.0),
            ("00.mp3", "00", "z", 3.0),
        ]));
        b.refresh();
        assert_eq!(b.level().selected().unwrap().name, "b.mp3");
        b.set_source(Source::Folders);
        assert_eq!(b.level().kind, LevelKind::Folder);
    }

    #[test]
    fn artwork_becomes_a_square_thumbnail() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 30, 30, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let t = thumbnail(&png, THUMB_SIZE).unwrap();
        assert_eq!((t.width, t.height), (THUMB_SIZE, THUMB_SIZE));
        assert_eq!(&t.rgba[..4], &[200, 30, 30, 255]);
        assert!(thumbnail(b"not a picture", THUMB_SIZE).is_none());
    }
}
