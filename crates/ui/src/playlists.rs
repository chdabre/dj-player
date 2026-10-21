//! Playlists on the browse screen: the MENU of actions on the selection, the playlist
//! picker (ADD TO PLAYLIST), naming with the search keyboard, the confirmation before a
//! delete, and moving an entry with the knob. Pure state and choices here; `main.rs` carries
//! them out on the player.

use std::path::PathBuf;

use engined::browse::{Browser, Kind, walk};
use library::{EntryId, PlaylistId, PlaylistKind};

/// Something the MENU (or TAG TRACK) does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Add these files to the tag list.
    Tag(Vec<PathBuf>),
    /// Remove these tag list entries.
    Untag(Vec<EntryId>),
    /// Pick a playlist to add these files to.
    AddTo(Vec<PathBuf>),
    /// Remove an entry from the shown list.
    Remove(EntryId),
    /// Move an entry with the knob.
    Move(EntryId),
    /// A new list of these files, named after `name` (asked first).
    MakePlaylist {
        files: Vec<PathBuf>,
        name: String,
    },
    NewPlaylist(Option<PlaylistId>),
    NewFolder(Option<PlaylistId>),
    Rename(PlaylistId, String),
    /// Ask, then delete.
    Delete(PlaylistId, String),
    /// The tag list into a new playlist (named first), emptying it.
    ConvertTagList,
    /// Ask, then empty the tag list.
    ClearTagList,
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Action::Tag(f) if f.len() > 1 => format!("ADD {} TRACKS TO TAG LIST", f.len()),
            Action::Tag(_) => "ADD TO TAG LIST".into(),
            Action::Untag(_) => "REMOVE FROM TAG LIST".into(),
            Action::AddTo(f) if f.len() > 1 => format!("ADD {} TRACKS TO PLAYLIST…", f.len()),
            Action::AddTo(_) => "ADD TO PLAYLIST…".into(),
            Action::Remove(_) => "REMOVE FROM THIS LIST".into(),
            Action::Move(_) => "MOVE".into(),
            Action::MakePlaylist { .. } => "MAKE PLAYLIST".into(),
            Action::NewPlaylist(_) => "NEW PLAYLIST".into(),
            Action::NewFolder(_) => "NEW FOLDER".into(),
            Action::Rename(..) => "RENAME".into(),
            Action::Delete(..) => "DELETE".into(),
            Action::ConvertTagList => "MAKE PLAYLIST FROM TAG LIST".into(),
            Action::ClearTagList => "REMOVE ALL".into(),
        }
    }
}

/// What a name typed on the keyboard is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    NewPlaylist {
        parent: Option<PlaylistId>,
        files: Vec<PathBuf>,
    },
    NewFolder(Option<PlaylistId>),
    Rename(PlaylistId),
    ConvertTagList,
}

impl Naming {
    pub fn prompt(&self) -> &'static str {
        match self {
            Naming::NewPlaylist { .. } | Naming::ConvertTagList => "NEW PLAYLIST",
            Naming::NewFolder(_) => "NEW FOLDER",
            Naming::Rename(_) => "RENAME",
        }
    }
}

/// A modal over the browse list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Menu {
        actions: Vec<Action>,
        cursor: usize,
    },
    /// ADD TO PLAYLIST: `None` is NEW PLAYLIST.
    Pick {
        files: Vec<PathBuf>,
        lists: Vec<(String, Option<PlaylistId>)>,
        cursor: usize,
    },
    /// Typing a name on the keyboard.
    Name {
        naming: Naming,
        text: String,
    },
    /// Delete or empty, once confirmed.
    Confirm {
        question: String,
        action: Action,
        cursor: usize,
    },
}

impl Modal {
    /// Rows a list modal shows: its title and its rows (none for naming).
    pub fn rows(&self) -> Option<(String, Vec<String>, usize)> {
        match self {
            Modal::Menu { actions, cursor } => Some((
                "MENU".into(),
                actions.iter().map(Action::label).collect(),
                *cursor,
            )),
            Modal::Pick { lists, cursor, .. } => Some((
                "ADD TO PLAYLIST".into(),
                lists.iter().map(|l| l.0.clone()).collect(),
                *cursor,
            )),
            Modal::Confirm {
                question, cursor, ..
            } => Some((
                question.clone(),
                vec!["YES".into(), "CANCEL".into()],
                *cursor,
            )),
            Modal::Name { .. } => None,
        }
    }

    fn len(&self) -> usize {
        self.rows().map_or(0, |r| r.1.len())
    }

    /// Move a list modal's cursor (clamped).
    pub fn move_by(&mut self, delta: isize) {
        let last = self.len().saturating_sub(1);
        if let Modal::Menu { cursor, .. }
        | Modal::Pick { cursor, .. }
        | Modal::Confirm { cursor, .. } = self
        {
            *cursor = cursor.saturating_add_signed(delta).min(last);
        }
    }

    pub fn select(&mut self, i: usize) {
        if i < self.len()
            && let Modal::Menu { cursor, .. }
            | Modal::Pick { cursor, .. }
            | Modal::Confirm { cursor, .. } = self
        {
            *cursor = i;
        }
    }
}

/// The files of the selection: a track, or every track under a folder.
fn files_of(b: &Browser) -> Vec<PathBuf> {
    let Some(e) = b.level().selected() else {
        return Vec::new();
    };
    match e.kind {
        Kind::Track if !e.path.as_os_str().is_empty() => vec![e.path.clone()],
        Kind::Folder if e.path.is_dir() => walk(&e.path),
        _ => Vec::new(),
    }
}

/// The MENU for the browser's selection and the level shown.
pub fn menu(b: &Browser) -> Vec<Action> {
    let mut out = Vec::new();
    let level = b.level();
    let shown = b.shown_playlist();
    match level.selected() {
        Some(e) if e.kind == Kind::Track && !e.path.as_os_str().is_empty() => {
            let tagged = b.tag_entries(&e.path);
            if tagged.is_empty() {
                out.push(Action::Tag(vec![e.path.clone()]));
            } else {
                out.push(Action::Untag(tagged));
            }
            out.push(Action::AddTo(vec![e.path.clone()]));
        }
        Some(e) if e.kind == Kind::Folder && e.path.is_dir() => {
            let files = files_of(b);
            if !files.is_empty() {
                out.push(Action::MakePlaylist {
                    files: files.clone(),
                    name: e.name.clone(),
                });
                out.push(Action::Tag(files.clone()));
                out.push(Action::AddTo(files));
            }
        }
        Some(e) => {
            if let Kind::Playlist(id) = e.kind {
                let kind = b.playlists().and_then(|p| p.get(id)).map(|p| p.kind);
                match kind {
                    Some(PlaylistKind::Tag) => {
                        out.push(Action::ConvertTagList);
                        out.push(Action::ClearTagList);
                    }
                    Some(_) => {
                        out.push(Action::Rename(id, e.name.clone()));
                        out.push(Action::Delete(id, e.name.clone()));
                    }
                    None => {}
                }
            }
        }
        None => {}
    }
    // In a list: the selected entry.
    if shown.is_some()
        && let Some(entry) = level.selected().and_then(|e| e.entry)
    {
        if shown != Some(PlaylistId::TAG_LIST) {
            out.push(Action::Remove(entry));
        }
        out.push(Action::Move(entry));
    }
    if shown == Some(PlaylistId::TAG_LIST) && !level.entries.is_empty() {
        out.push(Action::ConvertTagList);
        out.push(Action::ClearTagList);
    }
    if let Some(parent) = b.shown_playlist_folder() {
        out.push(Action::NewPlaylist(parent));
        out.push(Action::NewFolder(parent));
    }
    out.dedup();
    out
}

/// The lists files can go into, by their place in the tree ("Saturday / Peak"), after NEW
/// PLAYLIST. The tag list has its own button.
pub fn pick_lists(b: &Browser) -> Vec<(String, Option<PlaylistId>)> {
    let mut out = vec![("NEW PLAYLIST".to_string(), None)];
    let Some(p) = b.playlists() else {
        return out;
    };
    let name_of = |id: PlaylistId| p.get(id).map_or_else(String::new, |l| l.name.clone());
    for l in &p.lists {
        if l.kind != PlaylistKind::List {
            continue;
        }
        let mut path = vec![l.name.clone()];
        let mut up = l.parent;
        while let Some(f) = up {
            path.push(name_of(f));
            up = p.get(f).and_then(|x| x.parent);
        }
        path.reverse();
        out.push((path.join("  /  "), Some(l.id)));
    }
    out
}

/// TAG TRACK's track: the selection on the browse screen, else the loaded file.
pub fn tag_target(b: Option<&Browser>, browsing: bool, loaded: Option<PathBuf>) -> Option<PathBuf> {
    if browsing
        && let Some(e) = b.and_then(|b| b.level().selected())
        && e.kind == Kind::Track
        && !e.path.as_os_str().is_empty()
    {
        return Some(e.path.clone());
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_cursors_clamp() {
        let mut m = Modal::Confirm {
            question: "DELETE?".into(),
            action: Action::ClearTagList,
            cursor: 0,
        };
        m.move_by(5);
        assert_eq!(m.rows().unwrap().2, 1);
        m.move_by(-9);
        assert_eq!(m.rows().unwrap().2, 0);
        m.select(1);
        assert_eq!(m.rows().unwrap().2, 1);
        m.select(7);
        assert_eq!(m.rows().unwrap().2, 1);
        assert!(
            Modal::Name {
                naming: Naming::ConvertTagList,
                text: String::new()
            }
            .rows()
            .is_none()
        );
    }

    /// A browser over a temporary stick with tracks a, b (in folder `crate`), a list
    /// "Peak" (in folder "Saturday") holding a then b, and b on the tag list.
    fn browser() -> (Browser, std::path::PathBuf, PlaylistId) {
        use engined::persist::{Listing, Playlists};
        use library::{Listed, Playlist, PlaylistEntry, TrackId, TrackTags};
        use std::sync::Arc;
        let d = std::env::temp_dir().join(format!("ui-playlists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("crate")).unwrap();
        for f in ["crate/a.mp3", "crate/b.mp3"] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        let (ta, tb) = (TrackId([1; 16]), TrackId([2; 16]));
        let listing: Listing = [("crate/a.mp3", ta), ("crate/b.mp3", tb)]
            .into_iter()
            .map(|(p, t)| {
                let l = Listed {
                    track: t,
                    path: p.into(),
                    tags: TrackTags::default(),
                    rating: None,
                    analysed: false,
                };
                (p.to_string(), l)
            })
            .collect();
        let (folder, peak) = (PlaylistId::random(), PlaylistId::random());
        let list = |id, parent, kind, name: &str| Playlist {
            id,
            parent,
            kind,
            name: name.into(),
            position: "i".into(),
            entries: 0,
        };
        let entry = |t, p: &str| PlaylistEntry {
            id: EntryId::random(),
            track: t,
            position: p.into(),
        };
        let playlists = Playlists {
            lists: vec![
                list(PlaylistId::TAG_LIST, None, PlaylistKind::Tag, "TAG LIST"),
                list(folder, None, PlaylistKind::Folder, "Saturday"),
                list(peak, Some(folder), PlaylistKind::List, "Peak"),
            ],
            entries: [
                (PlaylistId::TAG_LIST, vec![entry(tb, "i")]),
                (peak, vec![entry(ta, "i"), entry(tb, "j")]),
            ]
            .into_iter()
            .collect(),
        };
        let mut b = Browser::new(d.clone(), 7);
        b.set_listing(Arc::new(listing));
        b.set_playlists(Arc::new(playlists));
        (b, d, peak)
    }

    fn labels(b: &Browser) -> Vec<String> {
        menu(b).iter().map(Action::label).collect()
    }

    #[test]
    fn the_menu_offers_what_the_selection_allows() {
        use engined::browse::Source;
        let (mut b, d, peak) = browser();
        // A folder on the stick.
        assert_eq!(
            labels(&b),
            [
                "MAKE PLAYLIST",
                "ADD 2 TRACKS TO TAG LIST",
                "ADD 2 TRACKS TO PLAYLIST…"
            ]
        );
        // Its tracks: a is not tagged, b is.
        b.enter();
        assert_eq!(labels(&b), ["ADD TO TAG LIST", "ADD TO PLAYLIST…"]);
        b.select(1);
        assert_eq!(labels(&b), ["REMOVE FROM TAG LIST", "ADD TO PLAYLIST…"]);
        assert_eq!(
            tag_target(Some(&b), true, None),
            Some(d.join("crate/b.mp3")),
            "on the browse screen, TAG TRACK takes the selection"
        );
        assert_eq!(tag_target(Some(&b), false, None), None);

        // PLAYLISTS: the tag list, a folder; new playlist or folder here.
        b.set_source(Source::Library);
        b.enter();
        assert_eq!(
            labels(&b),
            [
                "MAKE PLAYLIST FROM TAG LIST",
                "REMOVE ALL",
                "NEW PLAYLIST",
                "NEW FOLDER"
            ]
        );
        b.select(1);
        assert_eq!(
            labels(&b),
            ["RENAME", "DELETE", "NEW PLAYLIST", "NEW FOLDER"]
        );
        b.enter();
        b.enter();
        assert_eq!(b.shown_playlist(), Some(peak));
        assert_eq!(
            labels(&b),
            [
                "ADD TO TAG LIST",
                "ADD TO PLAYLIST…",
                "REMOVE FROM THIS LIST",
                "MOVE"
            ]
        );

        // The picker: NEW PLAYLIST, then lists by their place in the tree.
        let lists: Vec<String> = pick_lists(&b).into_iter().map(|l| l.0).collect();
        assert_eq!(lists, ["NEW PLAYLIST", "Saturday  /  Peak"]);

        // The tag list: no "remove from this list" (that is REMOVE FROM TAG LIST).
        b.set_source(Source::TagList);
        assert_eq!(
            labels(&b),
            [
                "REMOVE FROM TAG LIST",
                "ADD TO PLAYLIST…",
                "MOVE",
                "MAKE PLAYLIST FROM TAG LIST",
                "REMOVE ALL"
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn actions_read_as_the_player_says_them() {
        assert_eq!(Action::Tag(vec![PathBuf::new()]).label(), "ADD TO TAG LIST");
        assert_eq!(
            Action::Tag(vec![PathBuf::new(), PathBuf::new()]).label(),
            "ADD 2 TRACKS TO TAG LIST"
        );
        assert_eq!(
            Action::AddTo(vec![PathBuf::new()]).label(),
            "ADD TO PLAYLIST…"
        );
    }
}
