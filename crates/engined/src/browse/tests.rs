//! The browser's navigation, sorting, search and library views.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use library::{EntryId, PlaylistId, PlaylistKind, TrackId};

use super::*;
use crate::persist::{Listing, Playlists};

pub(super) fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("browse-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(super) fn touch(p: &Path) {
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
    assert_eq!(Sort::Length.next(), Sort::Order);
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
    assert_eq!(names(&b), ["PLAYLISTS", "FOLDERS", "TRACKS"]);

    // FOLDERS: the stick's structure, from the stored paths.
    b.select(1);
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
    b.select(2);
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
fn playlists_list_in_their_own_order_and_the_tag_list_marks_tracks() {
    use library::{Playlist, PlaylistEntry};
    let d = temp_dir("lists");
    for f in ["a.mp3", "b.mp3", "c.mp3"] {
        touch(&d.join(f));
    }
    let mut b = Browser::new(d.clone(), 7);
    let listing = listing(&[
        ("a.mp3", "A", "x", 1.0),
        ("b.mp3", "B", "y", 2.0),
        ("c.mp3", "C", "z", 3.0),
    ]);
    // Each listed track needs its own id here.
    let mut l = (*listing).clone();
    for (i, v) in l.values_mut().enumerate() {
        v.track = TrackId([i as u8 + 1; 16]);
    }
    let id_of = |name: &str| l[name].track;
    let (ta, tb, tc) = (id_of("a.mp3"), id_of("b.mp3"), id_of("c.mp3"));
    b.set_listing(Arc::new(l.clone()));
    let crate_ = PlaylistId::random();
    let set = PlaylistId::random();
    let pl = |id, parent, kind, name: &str, position: &str, entries| Playlist {
        id,
        parent,
        kind,
        name: name.into(),
        position: position.into(),
        entries,
    };
    let entry = |t: TrackId, p: &str| PlaylistEntry {
        id: EntryId::random(),
        track: t,
        position: p.into(),
    };
    let gone = TrackId([99; 16]);
    b.set_playlists(Arc::new(Playlists {
        lists: vec![
            pl(
                PlaylistId::TAG_LIST,
                None,
                PlaylistKind::Tag,
                "TAG LIST",
                "0i",
                1,
            ),
            pl(crate_, None, PlaylistKind::Folder, "Saturday", "i", 0),
            pl(set, Some(crate_), PlaylistKind::List, "Peak", "i", 4),
        ],
        entries: [
            (PlaylistId::TAG_LIST, vec![entry(tb, "i")]),
            (
                set,
                vec![
                    entry(tc, "i"),
                    entry(ta, "j"),
                    entry(gone, "k"),
                    entry(tc, "l"),
                ],
            ),
        ]
        .into_iter()
        .collect(),
    }));
    b.set_source(Source::Library);
    assert_eq!(b.enter(), Entered::Folder, "PLAYLISTS");
    assert_eq!(b.shown_playlist_folder(), Some(None));
    assert_eq!(names(&b), ["TAG LIST", "Saturday"]);
    b.select(1);
    b.enter();
    assert_eq!(names(&b), ["Peak"]);
    b.enter();
    assert_eq!(b.shown_playlist(), Some(set));
    assert_eq!(b.sort_label(), "#");
    assert_eq!(
        names(&b),
        ["c.mp3", "a.mp3", "Missing track", "c.mp3"],
        "the playlist's order, a missing track kept, a track twice"
    );
    assert!(b.level().entries.iter().all(|e| e.entry.is_some()));
    b.select(2);
    assert_eq!(b.enter(), Entered::Nothing, "a missing track can't load");
    b.set_sort(Sort::Name);
    assert_eq!(names(&b)[0], "a.mp3");
    b.set_sort(Sort::Order);
    assert!(b.is_tagged(&d.join("b.mp3")) && !b.is_tagged(&d.join("a.mp3")));
    assert_eq!(b.tag_entries(&d.join("b.mp3")).len(), 1);
    b.set_source(Source::TagList);
    assert_eq!(b.shown_playlist(), Some(PlaylistId::TAG_LIST));
    assert_eq!(names(&b), ["b.mp3"]);
}
