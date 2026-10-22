//! Playlists on the browse screen: the MENU of actions on the selection, the playlist
//! picker (ADD TO PLAYLIST), naming with the search keyboard, the confirmation before a
//! delete, and moving an entry with the knob. [`Editing`] holds the state and turns choices
//! into [`Effect`]s; the app carries them out on the player.

use std::path::PathBuf;

use engined::browse::{Browser, Kind, Sort, walk};
use library::{Edit, EntryId, PlaylistId, PlaylistKind};

use crate::app::effects::Effect;
use crate::screens::browse::{BACKSPACE, CLEAR, DONE};

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

/// Editing playlists on the browse screen: the modal over the list, if any, and the entry the
/// knob is moving, if any.
#[derive(Debug, Default)]
pub struct Editing {
    pub modal: Option<Modal>,
    pub moving: Option<EntryId>,
}

impl Editing {
    /// MENU: the actions on the selection, as a modal; false if there are none.
    pub fn open_menu(&mut self, b: &Browser) -> bool {
        let actions = menu(b);
        if actions.is_empty() {
            return false;
        }
        self.modal = Some(Modal::Menu { actions, cursor: 0 });
        true
    }

    /// Carry out a MENU action (or TAG TRACK's).
    pub fn act(&mut self, a: Action, browser: Option<&mut Browser>, fx: &mut Vec<Effect>) {
        let name = |naming, text: &str| {
            Some(Modal::Name {
                naming,
                text: text.to_string(),
            })
        };
        match a {
            Action::Tag(files) => fx.push(Effect::AddFiles(PlaylistId::TAG_LIST, files)),
            Action::Untag(entries) => fx.push(Effect::Library(Edit::RemoveEntries(entries))),
            Action::Remove(entry) => fx.push(Effect::Library(Edit::RemoveEntries(vec![entry]))),
            Action::AddTo(files) => {
                let lists = browser.map(|b| pick_lists(b)).unwrap_or_default();
                self.modal = Some(Modal::Pick {
                    files,
                    lists,
                    cursor: 0,
                });
            }
            Action::Move(entry) => {
                // Moves follow the list's own order.
                if let Some(b) = browser {
                    b.set_sort(Sort::Order);
                }
                self.moving = Some(entry);
            }
            Action::MakePlaylist { files, name: n } => {
                let naming = Naming::NewPlaylist {
                    parent: None,
                    files,
                };
                self.modal = name(naming, &n);
            }
            Action::NewPlaylist(parent) => {
                let naming = Naming::NewPlaylist {
                    parent,
                    files: Vec::new(),
                };
                self.modal = name(naming, "");
            }
            Action::NewFolder(parent) => self.modal = name(Naming::NewFolder(parent), ""),
            Action::Rename(id, n) => self.modal = name(Naming::Rename(id), &n),
            Action::ConvertTagList => {
                self.modal = name(Naming::ConvertTagList, "Tagged tracks");
            }
            a @ (Action::Delete(..) | Action::ClearTagList) => {
                let question = match &a {
                    Action::Delete(_, n) => format!("DELETE {n}?"),
                    _ => "REMOVE ALL FROM THE TAG LIST?".into(),
                };
                self.modal = Some(Modal::Confirm {
                    question,
                    action: a,
                    cursor: 1,
                });
            }
        }
    }

    /// The modal's push (or a touch on one of its rows).
    pub fn push(&mut self, browser: Option<&mut Browser>, fx: &mut Vec<Effect>) {
        match self.modal.take() {
            Some(Modal::Menu { actions, cursor }) => {
                if let Some(a) = actions.get(cursor).cloned() {
                    self.act(a, browser, fx);
                }
            }
            Some(Modal::Pick {
                files,
                lists,
                cursor,
            }) => match lists.get(cursor).map(|l| l.1) {
                Some(Some(id)) => fx.push(Effect::AddFiles(id, files)),
                Some(None) => {
                    self.modal = Some(Modal::Name {
                        naming: Naming::NewPlaylist {
                            parent: None,
                            files,
                        },
                        text: String::new(),
                    });
                }
                None => {}
            },
            Some(Modal::Confirm { action, cursor, .. }) => match action {
                _ if cursor != 0 => {}
                Action::Delete(id, _) => fx.push(Effect::Library(Edit::DeletePlaylist(id))),
                Action::ClearTagList => {
                    fx.push(Effect::Library(Edit::ClearPlaylist(PlaylistId::TAG_LIST)));
                }
                _ => {}
            },
            Some(name @ Modal::Name { .. }) => self.modal = Some(name),
            None => {}
        }
    }

    /// A name is being typed, and it is empty so far (the keyboard starts with a capital).
    pub fn naming_empty(&self) -> bool {
        matches!(&self.modal, Some(Modal::Name { text, .. }) if text.is_empty())
    }

    pub fn naming(&self) -> bool {
        matches!(self.modal, Some(Modal::Name { .. }))
    }

    /// A key typed into a name (on-screen keyboard or a real one); DONE names it.
    pub fn type_name(&mut self, c: char, fx: &mut Vec<Effect>) {
        let Some(Modal::Name { naming, text }) = self.modal.as_mut() else {
            return;
        };
        match c {
            BACKSPACE => {
                text.pop();
            }
            CLEAR => text.clear(),
            DONE => {
                let (naming, name) = (naming.clone(), text.trim().to_string());
                self.modal = None;
                if !name.is_empty() {
                    fx.push(named(naming, name));
                }
            }
            c if !c.is_control() => text.push(c),
            _ => {}
        }
    }

    /// MOVE: the moving entry one place up (`-1`) or down, the cursor with it.
    pub fn move_entry(&mut self, delta: isize, b: &mut Browser, fx: &mut Vec<Effect>) {
        let Some(entry) = self.moving else {
            return;
        };
        let l = b.level();
        let Some(i) = l.entries.iter().position(|e| e.entry == Some(entry)) else {
            self.moving = None;
            return;
        };
        let before = match delta {
            -1 if i > 0 => Some(l.entries[i - 1].entry),
            1 if i + 1 < l.entries.len() => Some(l.entries.get(i + 2).and_then(|e| e.entry)),
            _ => None,
        };
        let Some(before) = before else {
            return;
        };
        fx.push(Effect::Library(Edit::MoveEntry { entry, before }));
        b.move_by(delta);
    }
}

/// What a typed name was for.
fn named(naming: Naming, name: String) -> Effect {
    match naming {
        Naming::NewPlaylist { parent, files } if files.is_empty() => {
            Effect::Library(Edit::CreatePlaylist {
                id: PlaylistId::random(),
                parent,
                kind: PlaylistKind::List,
                name,
            })
        }
        Naming::NewPlaylist { parent, files } => Effect::NewPlaylist {
            id: PlaylistId::random(),
            parent,
            name,
            files,
        },
        Naming::NewFolder(parent) => Effect::Library(Edit::CreatePlaylist {
            id: PlaylistId::random(),
            parent,
            kind: PlaylistKind::Folder,
            name,
        }),
        Naming::Rename(id) => Effect::Library(Edit::RenamePlaylist(id, name)),
        Naming::ConvertTagList => Effect::Library(Edit::ConvertTagList {
            id: PlaylistId::random(),
            parent: None,
            name,
        }),
    }
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
        // One folder per call: tests run in parallel.
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("ui-playlists-{}-{n}", std::process::id()));
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

    /// The effects as text, for comparing (`DeckCommand` has no `PartialEq`).
    fn said(fx: &[Effect]) -> Vec<String> {
        fx.iter().map(|e| format!("{e:?}")).collect()
    }

    #[test]
    fn a_delete_happens_only_on_yes() {
        let id = PlaylistId::random();
        let mut e = Editing::default();
        let mut fx = Vec::new();
        e.act(Action::Delete(id, "Peak".into()), None, &mut fx);
        // The question comes up on CANCEL.
        assert_eq!(e.modal.as_ref().and_then(Modal::rows).unwrap().2, 1);
        e.push(None, &mut fx);
        assert!(fx.is_empty() && e.modal.is_none());

        e.act(Action::Delete(id, "Peak".into()), None, &mut fx);
        e.modal.as_mut().unwrap().move_by(-1);
        e.push(None, &mut fx);
        assert_eq!(
            said(&fx),
            [format!("{:?}", Effect::Library(Edit::DeletePlaylist(id)))]
        );
    }

    #[test]
    fn a_typed_name_makes_the_playlist_and_an_empty_one_nothing() {
        let mut e = Editing::default();
        let mut fx = Vec::new();
        e.act(Action::NewFolder(None), None, &mut fx);
        assert!(e.naming() && e.naming_empty());
        for c in ['x', BACKSPACE, 'S', 'e', 't', 's', ' '] {
            e.type_name(c, &mut fx);
        }
        assert!(!e.naming_empty());
        e.type_name(DONE, &mut fx);
        assert!(e.modal.is_none());
        assert!(
            matches!(&fx[..], [Effect::Library(Edit::CreatePlaylist { parent: None, kind: PlaylistKind::Folder, name, .. })] if name == "Sets"),
            "{fx:?}"
        );

        fx.clear();
        e.act(
            Action::Rename(PlaylistId::TAG_LIST, "Old".into()),
            None,
            &mut fx,
        );
        e.type_name(CLEAR, &mut fx);
        e.type_name(' ', &mut fx);
        e.type_name(DONE, &mut fx);
        assert!(
            fx.is_empty() && e.modal.is_none(),
            "a blank name names nothing"
        );
    }

    #[test]
    fn add_to_playlist_picks_a_list_or_names_a_new_one() {
        let (mut b, d, peak) = browser();
        let files = vec![d.join("crate/a.mp3")];
        let mut e = Editing::default();
        let mut fx = Vec::new();
        e.act(Action::AddTo(files.clone()), Some(&mut b), &mut fx);
        e.modal.as_mut().unwrap().select(1);
        e.push(Some(&mut b), &mut fx);
        assert_eq!(
            said(&fx),
            [format!("{:?}", Effect::AddFiles(peak, files.clone()))]
        );

        fx.clear();
        e.act(Action::AddTo(files.clone()), Some(&mut b), &mut fx);
        // NEW PLAYLIST: the files wait for a name.
        e.push(Some(&mut b), &mut fx);
        assert!(fx.is_empty() && e.naming());
        for c in "Warmup".chars() {
            e.type_name(c, &mut fx);
        }
        e.type_name(DONE, &mut fx);
        assert!(
            matches!(&fx[..], [Effect::NewPlaylist { parent: None, name, files: f, .. }] if name == "Warmup" && *f == files),
            "{fx:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn move_drops_the_entry_before_the_one_below_and_follows_it() {
        use engined::browse::Source;
        let (mut b, d, peak) = browser();
        b.set_source(Source::Library);
        b.enter();
        b.select(1);
        b.enter();
        b.enter();
        assert_eq!(b.shown_playlist(), Some(peak));
        let first = b.level().entries[0].entry.unwrap();
        let mut e = Editing::default();
        let mut fx = Vec::new();
        e.act(Action::Move(first), Some(&mut b), &mut fx);
        // At the top: nothing above.
        e.move_entry(-1, &mut b, &mut fx);
        assert!(fx.is_empty());
        // Down past the last entry: to the end.
        e.move_entry(1, &mut b, &mut fx);
        assert_eq!(
            said(&fx),
            [format!(
                "{:?}",
                Effect::Library(Edit::MoveEntry {
                    entry: first,
                    before: None
                })
            )]
        );
        assert_eq!(b.level().cursor, 1, "the cursor goes with it");
        let _ = std::fs::remove_dir_all(&d);
    }
}
