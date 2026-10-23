//! What the browse and INFO screens show, prepared once per frame from the browser, the
//! library and the browse state.

use std::path::Path;

use browser::{Browser, Entry, Kind, Source, Tags};
use player_api::PlaylistKind;
use soulseek::Snapshot;

use super::editing::Modal;
use super::state::BrowseState;
use super::{BrowseView, InfoView, ModalView, ROWS, Row, RowKind, SearchSource, length};
use crate::app::engine::LibraryView;
use crate::app::track::track_title;

impl BrowseState {
    /// What the browse screen shows of the browser (or, searching there, of SoulSeek);
    /// `loaded` is the file on the deck.
    pub fn view(
        &self,
        b: &Browser,
        lib: &LibraryView,
        soulseek: Option<&Snapshot>,
        loaded: Option<&Path>,
    ) -> BrowseView {
        let l = b.level();
        let rows = l
            .entries
            .iter()
            .skip(l.top)
            .take(ROWS)
            .map(|e| row(b, e, loaded))
            .collect();
        let library = b.source() == Source::Library;
        let query = b.search_query();
        let header = header(b, query, self.keyboard);
        let empty = empty(b, query, library);
        let scan = lib.scan.filter(|&(done, total)| done < total || total == 0);
        let side = self.side.then(|| info_view(b, library, loaded)).flatten();
        let mut v = BrowseView {
            side,
            header,
            at_root: b.at_root(),
            library,
            sort: b.sort_label(),
            searching: query.is_some(),
            search_source: self.source,
            keyboard: (self.keyboard && query.is_some()) || self.editing.naming(),
            shift: self.shift,
            tag_list: b.source() == Source::TagList,
            modal: self.editing.modal.as_ref().and_then(|m| {
                m.rows().map(|(title, rows, selected)| ModalView {
                    title,
                    rows,
                    selected,
                    danger_first: matches!(m, Modal::Confirm { .. }),
                })
            }),
            header_override: match (&self.editing.modal, self.editing.moving) {
                (Some(Modal::Name { naming, text }), _) => {
                    Some(format!("{}:  {text}▏", naming.prompt()))
                }
                (_, Some(_)) => Some("MOVING  ·  TURN TO MOVE, PUSH TO DROP".into()),
                _ => None,
            },
            scan,
            can_scan: lib.listing.is_some(),
            unanalysed: if library {
                self.unanalysed_count(b, lib.listing)
            } else {
                0
            },
            analyzing: lib.batch.map(|p| (p.done, p.total, p.stopping)),
            ask_scan: self.ask_scan,
            rows,
            selected: l.cursor.checked_sub(l.top).filter(|&r| r < ROWS),
            first: l.top,
            total: l.entries.len(),
            empty,
            has_tracks: l.entries.iter().any(|e| e.kind == Kind::Track),
        };
        if query.is_some() && self.source == SearchSource::Soulseek {
            self.soulseek_view(&mut v, soulseek, loaded);
        }
        v
    }

    /// The shown rows' tags have all been read, and the library has said what it has (for
    /// screenshots).
    pub fn settled(&self, b: &Browser, lib: &LibraryView) -> bool {
        if b.search_query().is_some() && self.source == SearchSource::Soulseek {
            return true;
        }
        if (b.source() != Source::Folders && (lib.listing.is_none() || lib.playlists.is_none()))
            || lib.scanning()
        {
            return false;
        }
        let l = b.level();
        let info_ready = !self.side
            || l.selected()
                .is_none_or(|e| e.kind == Kind::Folder || b.large_art(&e.path).is_some());
        info_ready
            && l.entries
                .iter()
                .skip(l.top)
                .take(ROWS)
                .all(|e| e.kind == Kind::Folder || b.is_read(&e.path))
    }
}

/// A shown row of the list: a folder or playlist, or a track with its tags.
fn row(b: &Browser, e: &Entry, loaded: Option<&Path>) -> Row {
    match e.kind {
        Kind::Folder | Kind::Playlist(_) => Row {
            kind: match e.kind {
                Kind::Playlist(id) => match b.playlists().and_then(|p| p.get(id)) {
                    Some(p) if p.kind == PlaylistKind::Folder => RowKind::Folder,
                    _ => RowKind::List,
                },
                _ => RowKind::Folder,
            },
            title: e.name.clone(),
            artist: String::new(),
            detail: String::new(),
            loaded: false,
            art: None,
            tagged: false,
            missing: false,
        },
        Kind::Track => {
            let tags = b.info(&e.path);
            let tags = tags.as_ref();
            let detail = [
                tags.and_then(|t| t.bpm).map(|b| format!("{b:.1}")),
                tags.and_then(|t| t.key.clone()),
                tags.and_then(|t| t.duration).map(length),
            ];
            Row {
                kind: RowKind::Track,
                title: track_title(&e.path, tags),
                artist: tags.and_then(|t| t.artist.clone()).unwrap_or_default(),
                detail: detail.into_iter().flatten().collect::<Vec<_>>().join("   "),
                loaded: loaded == Some(e.path.as_path()),
                art: b.art(&e.path),
                tagged: b.is_tagged(&e.path),
                missing: e.path.as_os_str().is_empty(),
            }
        }
    }
}

/// Where the list is: the source, then the folders down to it, and the search with its
/// cursor while the keyboard is up.
fn header(b: &Browser, query: Option<&str>, keyboard: bool) -> String {
    let top = if b.source() == Source::Library {
        "LIBRARY".to_string()
    } else if b.source() == Source::TagList {
        "TAG LIST".to_string()
    } else {
        b.root()
            .file_name()
            .map_or("USB".into(), |n| n.to_string_lossy().into_owned())
    };
    let mut header = std::iter::once(top.as_str())
        .chain(b.trail())
        .collect::<Vec<_>>()
        .join("  /  ");
    if let Some(q) = query {
        header = format!("{header}:  {q}{}", if keyboard { "▏" } else { "" });
    }
    header
}

/// What the list says when it has no rows.
fn empty(b: &Browser, query: Option<&str>, library: bool) -> Option<String> {
    let l = b.level();
    match (&l.error, l.entries.is_empty()) {
        (Some(e), _) => Some(format!("Can't read this folder: {e}")),
        (None, true) if query.is_some() => Some("Nothing matches".into()),
        (None, true) if library => Some(
            "No tracks in the library yet: scan the USB, or load tracks from its folders".into(),
        ),
        (None, true) => Some("No folders or tracks here".into()),
        _ => None,
    }
}

/// The side panel's view of the selected entry.
fn info_view(b: &Browser, library: bool, loaded: Option<&Path>) -> Option<InfoView> {
    let l = b.level();
    let e = l.selected()?;
    let source = if library { "LIBRARY" } else { "USB" };
    let position = format!("TRACK {:03}/{:03}", l.cursor + 1, l.entries.len());
    if e.kind != Kind::Track {
        return Some(InfoView {
            source,
            position,
            folder: true,
            fields: vec![("FOLDER", e.name.clone())],
            ..InfoView::default()
        });
    }
    let tags = b.info(&e.path).unwrap_or_default();
    Some(InfoView {
        source,
        position,
        folder: false,
        fields: info_fields(&e.path, &tags),
        rating: tags.rating,
        // The list's thumbnail stands in until the large picture is read.
        art: b.large_art(&e.path).unwrap_or_else(|| b.art(&e.path)),
        comment: tags.comment.clone(),
        loaded: loaded == Some(e.path.as_path()),
    })
}

/// The info panels' fields of a track.
pub(crate) fn info_fields(path: &Path, tags: &Tags) -> Vec<(&'static str, String)> {
    let or_dash = |v: Option<String>| v.unwrap_or_else(|| "–".into());
    vec![
        ("TITLE", track_title(path, Some(tags))),
        ("ARTIST", or_dash(tags.artist.clone())),
        ("ALBUM", or_dash(tags.album.clone())),
        ("TIME", or_dash(tags.duration.map(length))),
        ("BPM", or_dash(tags.bpm.map(|b| format!("{b:.1}")))),
        ("KEY", or_dash(tags.key.clone())),
        ("GENRE", or_dash(tags.genre.clone())),
    ]
}
