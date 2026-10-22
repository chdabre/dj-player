//! The MENU: what can be done with the browser's selection, the lists files can go into, and
//! TAG TRACK's track.

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
