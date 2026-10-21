//! Playlists (schema version 2): the tree, entries in order, the tag list, merges, and the
//! migration of a version 1 file.

use std::path::{Path, PathBuf};

use library::{
    AudioHash, DeviceId, Edit, EntryId, FileHash, FileIdentity, Library, MountOptions, PlaylistId,
    PlaylistKind, TrackId, WriteError,
};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("playlists-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("DJPLAYER")
}

fn mount(dir: &Path) -> Library {
    Library::mount(dir, MountOptions::new(DeviceId::from_bytes([1; 16]))).unwrap()
}

fn track(lib: &mut Library, name: &str) -> TrackId {
    let f = FileIdentity::new(name, name.len() as u64, FileHash::of(name.as_bytes()));
    lib.find_or_create(f).unwrap().track
}

fn list(lib: &mut Library, parent: Option<PlaylistId>, name: &str) -> PlaylistId {
    let id = PlaylistId::random();
    lib.apply(Edit::CreatePlaylist {
        id,
        parent,
        kind: PlaylistKind::List,
        name: name.into(),
    })
    .unwrap();
    id
}

fn tracks_of(lib: &Library, p: PlaylistId) -> Vec<TrackId> {
    lib.entries(p)
        .unwrap()
        .into_iter()
        .map(|e| e.track)
        .collect()
}

fn ids_of(lib: &Library, p: PlaylistId) -> Vec<EntryId> {
    lib.entries(p).unwrap().into_iter().map(|e| e.id).collect()
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_new_library_has_only_the_tag_list() {
    let dir = temp("new");
    let lib = mount(&dir);
    let all = lib.playlists().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, PlaylistId::TAG_LIST);
    assert_eq!(
        (all[0].kind, all[0].name.as_str()),
        (PlaylistKind::Tag, "TAG LIST")
    );
    drop(lib);
    cleanup(&dir);
}

#[test]
fn playlists_keep_their_tree_and_their_order_across_mounts() {
    let dir = temp("tree");
    let (a, b, c);
    let (crate_, warmup, peak);
    {
        let mut lib = mount(&dir);
        (a, b, c) = (
            track(&mut lib, "a.mp3"),
            track(&mut lib, "b.mp3"),
            track(&mut lib, "c.mp3"),
        );
        crate_ = PlaylistId::random();
        lib.apply(Edit::CreatePlaylist {
            id: crate_,
            parent: None,
            kind: PlaylistKind::Folder,
            name: "Saturday".into(),
        })
        .unwrap();
        warmup = list(&mut lib, Some(crate_), "Warm-up");
        peak = list(&mut lib, Some(crate_), "Peak");
        lib.apply(Edit::AddEntries {
            playlist: warmup,
            tracks: vec![a, b, a],
            before: None,
        })
        .unwrap();
        // c at the front.
        let first = ids_of(&lib, warmup)[0];
        lib.apply(Edit::AddEntries {
            playlist: warmup,
            tracks: vec![c],
            before: Some(first),
        })
        .unwrap();
        lib.flush();
    }
    let mut lib = mount(&dir);
    let names: Vec<(String, Option<PlaylistId>)> = lib
        .playlists()
        .unwrap()
        .into_iter()
        .map(|p| (p.name, p.parent))
        .collect();
    assert_eq!(
        names,
        [
            ("TAG LIST".to_string(), None),
            ("Saturday".to_string(), None),
            ("Warm-up".to_string(), Some(crate_)),
            ("Peak".to_string(), Some(crate_)),
        ]
    );
    assert_eq!(
        tracks_of(&lib, warmup),
        [c, a, b, a],
        "the same track twice is fine"
    );
    assert_eq!(lib.playlist(warmup).unwrap().unwrap().entries, 4);

    // Moves: the last a to the front, then the front c to the end.
    let ids = ids_of(&lib, warmup);
    lib.apply(Edit::MoveEntry {
        entry: ids[3],
        before: Some(ids[0]),
    })
    .unwrap();
    lib.apply(Edit::MoveEntry {
        entry: ids[0],
        before: None,
    })
    .unwrap();
    assert_eq!(tracks_of(&lib, warmup), [a, a, b, c]);

    // Peak before Warm-up; then out of the folder to the top.
    lib.apply(Edit::MovePlaylist {
        id: peak,
        parent: Some(crate_),
        before: Some(warmup),
    })
    .unwrap();
    let order: Vec<String> = lib
        .playlists()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(order, ["TAG LIST", "Saturday", "Peak", "Warm-up"]);
    lib.apply(Edit::MovePlaylist {
        id: peak,
        parent: None,
        before: None,
    })
    .unwrap();
    let order: Vec<String> = lib
        .playlists()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(order, ["TAG LIST", "Saturday", "Warm-up", "Peak"]);

    // A folder can't go into itself; a list holds no playlists; the tag list stays.
    assert!(matches!(
        lib.apply(Edit::MovePlaylist {
            id: crate_,
            parent: Some(crate_),
            before: None
        }),
        Err(WriteError::BadPlaylistEdit(_))
    ));
    assert!(matches!(
        lib.apply(Edit::CreatePlaylist {
            id: PlaylistId::random(),
            parent: Some(peak),
            kind: PlaylistKind::List,
            name: "x".into()
        }),
        Err(WriteError::BadPlaylistEdit(_))
    ));
    assert!(
        lib.apply(Edit::DeletePlaylist(PlaylistId::TAG_LIST))
            .is_err()
    );

    // Remove one entry; rename; deleting the folder takes Warm-up and its entries.
    let first = ids_of(&lib, warmup)[0];
    lib.apply(Edit::RemoveEntries(vec![first])).unwrap();
    assert_eq!(tracks_of(&lib, warmup), [a, b, c]);
    lib.apply(Edit::RenamePlaylist(peak, "Peak time".into()))
        .unwrap();
    lib.apply(Edit::DeletePlaylist(crate_)).unwrap();
    let left: Vec<String> = lib
        .playlists()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(left, ["TAG LIST", "Peak time"]);
    assert!(lib.entries(warmup).unwrap().is_empty());
    assert_eq!(
        lib.tracks().unwrap().len(),
        3,
        "tracks are never deleted with a list"
    );
    drop(lib);
    cleanup(&dir);
}

#[test]
fn the_tag_list_becomes_a_playlist_and_empties() {
    let dir = temp("tag");
    let mut lib = mount(&dir);
    let (a, b) = (track(&mut lib, "a.mp3"), track(&mut lib, "b.mp3"));
    lib.apply(Edit::AddEntries {
        playlist: PlaylistId::TAG_LIST,
        tracks: vec![a, b],
        before: None,
    })
    .unwrap();
    let made = PlaylistId::random();
    lib.apply(Edit::ConvertTagList {
        id: made,
        parent: None,
        name: "Tagged 2026-10-21".into(),
    })
    .unwrap();
    assert_eq!(tracks_of(&lib, made), [a, b]);
    assert!(lib.entries(PlaylistId::TAG_LIST).unwrap().is_empty());
    lib.apply(Edit::AddEntries {
        playlist: PlaylistId::TAG_LIST,
        tracks: vec![b],
        before: None,
    })
    .unwrap();
    lib.apply(Edit::ClearPlaylist(PlaylistId::TAG_LIST))
        .unwrap();
    assert!(lib.entries(PlaylistId::TAG_LIST).unwrap().is_empty());
    drop(lib);
    cleanup(&dir);
}

#[test]
fn entries_follow_a_merge_to_the_surviving_track() {
    let dir = temp("merge");
    let mut lib = mount(&dir);
    let (old, copy) = (track(&mut lib, "a.mp3"), track(&mut lib, "a copy.mp3"));
    let p = list(&mut lib, None, "Set");
    lib.apply(Edit::AddEntries {
        playlist: p,
        tracks: vec![copy, old],
        before: None,
    })
    .unwrap();
    let same = AudioHash {
        v: 1,
        hash: [9; 32],
    };
    lib.set_audio_hash(old, same).unwrap();
    assert_eq!(lib.set_audio_hash(copy, same).unwrap(), old);
    assert_eq!(tracks_of(&lib, p), [old, old]);
    drop(lib);
    cleanup(&dir);
}

#[test]
fn a_version_1_file_gets_the_playlist_tables() {
    let dir = temp("v1");
    let t = {
        let mut lib = mount(&dir);
        let t = track(&mut lib, "a.mp3");
        lib.flush();
        t
    };
    // Back to what version 1 wrote.
    {
        let c = rusqlite::Connection::open(dir.join("library.db")).unwrap();
        c.execute_batch(
            "DROP TABLE playlist_entries; DROP TABLE playlists; PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let mut lib = mount(&dir);
    assert!(lib.track(t).unwrap().is_some(), "the tracks are kept");
    assert_eq!(lib.playlists().unwrap().len(), 1, "the tag list is made");
    let p = list(&mut lib, None, "New");
    lib.apply(Edit::AddEntries {
        playlist: p,
        tracks: vec![t],
        before: None,
    })
    .unwrap();
    lib.flush();
    drop(lib);
    let c = rusqlite::Connection::open(dir.join("library.db")).unwrap();
    let v: i64 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 2);
    drop(c);
    cleanup(&dir);
}

#[test]
fn a_ram_library_has_playlists_too() {
    let mut lib = Library::ram(MountOptions::new(DeviceId::random()), "test");
    let t = track(&mut lib, "a.mp3");
    lib.apply(Edit::AddEntries {
        playlist: PlaylistId::TAG_LIST,
        tracks: vec![t],
        before: None,
    })
    .unwrap();
    assert_eq!(tracks_of(&lib, PlaylistId::TAG_LIST), [t]);
}
