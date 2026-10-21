//! Identity, edits, durability and the folder layout.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use library::{
    AnalysisRef, AudioHash, Cue, DeviceId, Edit, FileHash, FileIdentity, FolderStore, HotCue,
    Libraries, Library, MemStore, MountOptions, Op, Persistence, SEGMENT_ROTATE_BYTES, Stamp,
    Store, WaveformRef, track_id,
};
use serde_json::json;

fn dev(n: u8) -> DeviceId {
    DeviceId::from_bytes([n; 16])
}

fn mount(store: &Arc<MemStore>, d: u8) -> Library {
    Library::mount(Arc::clone(store), MountOptions::new(dev(d))).unwrap()
}

/// A store holding a copy of another's files (a stick moved to another player).
fn copy(store: &MemStore) -> Arc<MemStore> {
    let c = Arc::new(MemStore::new());
    for (k, v) in store.dump() {
        c.put(&k, v);
    }
    c
}

fn file(path: &str, bytes: &[u8]) -> FileIdentity {
    FileIdentity::new(path, bytes.len() as u64, FileHash::of(bytes))
}

fn audio(n: u8) -> AudioHash {
    AudioHash {
        v: 1,
        hash: [n; 32],
    }
}

#[test]
fn two_devices_discovering_the_same_file_get_the_same_id() {
    let a = Arc::new(MemStore::new());
    let b = Arc::new(MemStore::new());
    let ta = mount(&a, 1).find_or_create(file("Music/x.mp3", b"same bytes"));
    let tb = mount(&b, 2).find_or_create(file("Other/x.mp3", b"same bytes"));
    assert!(ta.created && tb.created);
    assert_eq!(ta.track, tb.track);
    assert_eq!(ta.track, track_id(&FileHash::of(b"same bytes")));
}

#[test]
fn an_exact_duplicate_is_one_track_with_two_locations() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let first = lib.find_or_create(file("a/x.mp3", b"bytes"));
    let again = lib.find_or_create(file("a/x.mp3", b"bytes"));
    assert_eq!(again.track, first.track);
    assert!(!again.created && !again.new_location);
    let dup = lib.find_or_create(file("b/copy of x.mp3", b"bytes"));
    assert_eq!(dup.track, first.track);
    assert!(!dup.created && dup.new_location);
    let t = lib.track(first.track).unwrap();
    assert_eq!(t.locations.len(), 2);
    assert!(t.has_duplicates());
}

#[test]
fn a_tag_edited_copy_merges_by_audio_hash_into_the_older_track() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let old = lib
        .find_or_create(file("x.mp3", b"original tags + audio"))
        .track;
    lib.set_audio_hash(old, FileHash::of(b"original tags + audio"), audio(7));
    lib.apply(Edit::SetHotCue(old, 0, Some(HotCue::at(12.5))))
        .unwrap();

    // The same audio with edited tags: a new file hash, so first a new record...
    let edited_hash = FileHash::of(b"edited tags + audio");
    let new = lib.find_or_create(file("x (edited).mp3", b"edited tags + audio"));
    assert!(new.created);
    assert_ne!(new.track, old);
    lib.apply(Edit::AddCue(new.track, Cue::memory(30.0)))
        .unwrap();
    // ...until analysis finds the same audio: it aliases to the older track.
    let survivor = lib.set_audio_hash(new.track, edited_hash, audio(7));
    assert_eq!(survivor, old);
    assert_eq!(lib.resolve(new.track), old);

    let t = lib.track(new.track).unwrap();
    assert_eq!(t.id, old);
    assert_eq!(t.merged, [new.track]);
    assert_eq!(t.locations.len(), 2);
    assert_eq!(t.hot_cues[0].as_ref().map(|h| h.at), Some(12.5));
    assert_eq!(t.cues.len(), 1, "fields of both tracks merge");
    // Loading the edited file again finds the survivor, and new edits go there.
    assert_eq!(
        lib.find_or_create(file("x (edited).mp3", b"edited tags + audio"))
            .track,
        old
    );
    lib.apply(Edit::SetRating(new.track, Some(5))).unwrap();
    assert!(
        lib.state()
            .register(&format!("track/{old}/rating"))
            .is_some()
    );

    // Survives a remount on another device.
    let mut other = mount(&copy(&s), 2);
    assert_eq!(other.resolve(new.track), old);
    assert_eq!(other.track(old).unwrap().rating, Some(5));
}

#[test]
fn a_different_encode_is_a_separate_track() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let mp3 = lib.find_or_create(file("x.mp3", b"mp3 bytes")).track;
    let flac = lib.find_or_create(file("x.flac", b"flac bytes")).track;
    assert_eq!(
        lib.set_audio_hash(mp3, FileHash::of(b"mp3 bytes"), audio(1)),
        mp3
    );
    assert_eq!(
        lib.set_audio_hash(flac, FileHash::of(b"flac bytes"), audio(2)),
        flac
    );
    assert_eq!(lib.tracks().len(), 2);
}

#[test]
fn a_stale_audio_hash_is_replaced() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    let old = AudioHash {
        v: 0,
        hash: [1; 32],
    };
    lib.set_audio_hash(t, FileHash::of(b"x"), old);
    assert_eq!(
        lib.track(t).unwrap().audio_hash(),
        None,
        "v0 is not current"
    );
    lib.set_audio_hash(t, FileHash::of(b"x"), audio(3));
    let track = lib.track(t).unwrap();
    assert_eq!(track.audio_hash(), Some(audio(3)));
    assert_eq!(track.locations.len(), 1);
}

#[test]
fn a_missing_file_keeps_its_record() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("gone.mp3", b"gone")).track;
    lib.apply(Edit::SetHotCue(t, 3, Some(HotCue::at(1.0))))
        .unwrap();
    lib.flush();
    drop(lib);
    // Nothing about the file is checked on mount: its record and cues stay.
    let mut lib = mount(&s, 1);
    let track = lib.track(t).unwrap();
    assert_eq!(track.locations[0].1.path, "gone.mp3");
    assert!(track.hot_cues[3].is_some());
}

#[test]
fn cues_hot_cues_rating_analysis_and_waveform_persist() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    let cue = lib
        .apply(Edit::AddCue(t, Cue::memory(61.284)))
        .unwrap()
        .unwrap();
    lib.apply(Edit::AddCue(t, Cue::memory_loop(10.0, 12.0)))
        .unwrap();
    let edited = lib
        .apply(Edit::ReplaceCue(t, cue.clone(), Cue::memory(61.3)))
        .unwrap()
        .unwrap();
    assert_ne!(edited, cue);
    assert!(
        lib.apply(Edit::RemoveCue(t, cue)).is_err(),
        "already replaced"
    );
    for slot in [0, 7, 8, 15] {
        lib.apply(Edit::SetHotCue(t, slot, Some(HotCue::at(slot as f64))))
            .unwrap();
    }
    lib.apply(Edit::SetHotCue(t, 7, None)).unwrap();
    assert!(lib.apply(Edit::SetHotCue(t, 16, None)).is_err());
    lib.apply(Edit::SetRating(t, Some(4))).unwrap();
    let blob = lib.put_blob(b"analysis result");
    let a = AnalysisRef {
        version: "1".into(),
        params: "p".into(),
        blob,
        created: library::rfc3339(0),
    };
    lib.apply(Edit::SetAnalysis(t, "gridfit".into(), a.clone()))
        .unwrap();
    assert!(
        lib.apply(Edit::SetAnalysis(t, "../x".into(), a.clone()))
            .is_err()
    );
    let w = WaveformRef {
        blob: lib.put_blob(b"detail"),
        overview_blob: lib.put_blob(b"overview"),
    };
    lib.apply(Edit::SetWaveform(t, w)).unwrap();
    assert_eq!(lib.persistence(), Persistence::Pending);
    lib.flush();
    assert_eq!(lib.persistence(), Persistence::Saved);
    let before = lib.track(t).unwrap();

    let mut again = mount(&copy(&s), 2);
    let after = again.track(t).unwrap();
    assert_eq!(after, before);
    assert_eq!(after.cues.len(), 2);
    assert_eq!(after.cues[0].1.at, 10.0, "cues come in time order");
    let slots: Vec<usize> = (0..16).filter(|&i| after.hot_cues[i].is_some()).collect();
    assert_eq!(slots, [0, 8, 15]);
    assert_eq!(after.rating, Some(4));
    assert_eq!(after.analysis["gridfit"], a);
    assert_eq!(again.read_blob(blob).unwrap(), b"analysis result");
    assert_eq!(after.waveform, Some(w));
}

#[test]
fn the_clock_starts_above_everything_in_the_library() {
    static WALL: AtomicU64 = AtomicU64::new(0);
    let s = Arc::new(MemStore::new());
    let mut a = Library::mount(Arc::clone(&s), MountOptions::new(dev(1))).unwrap();
    let t = a.find_or_create(file("x.mp3", b"x")).track;
    a.apply(Edit::SetRating(t, Some(1))).unwrap();
    a.flush();
    let ahead = a.hlc();

    // A player with no clock source (wall clock 0) mounts the stick later.
    let opts = MountOptions {
        now: || WALL.load(Ordering::Relaxed),
        ..MountOptions::new(dev(2))
    };
    let mut b = Library::mount(copy(&s), opts).unwrap();
    assert!(b.hlc() >= ahead);
    b.apply(Edit::SetRating(t, Some(2))).unwrap();
    assert!(b.hlc() > ahead);
    assert_eq!(b.track(t).unwrap().rating, Some(2), "its edit wins");
}

#[test]
fn each_mount_opens_a_new_segment_and_big_ones_rotate() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.flush();
    drop(lib);
    let mut lib = mount(&s, 1);
    // A mount with no edits writes nothing.
    let journal = format!("journal/{}", dev(1));
    assert_eq!(s.list(&journal).unwrap(), ["00000001.log"]);
    let mut n = 0;
    while s.list(&journal).unwrap().len() < 4 {
        lib.apply(Edit::AddCue(t, Cue::memory(n as f64))).unwrap();
        n += 1;
    }
    lib.flush();
    let names = s.list(&journal).unwrap();
    assert_eq!(
        names,
        [
            "00000001.log",
            "00000002.log",
            "00000003.log",
            "00000004.log"
        ]
    );
    for full in &names[1..3] {
        let len = s.read(&format!("{journal}/{full}")).unwrap().len();
        assert!(
            (SEGMENT_ROTATE_BYTES..SEGMENT_ROTATE_BYTES + 1024).contains(&len),
            "{len}"
        );
    }
    assert_eq!(mount(&copy(&s), 2).track(t).unwrap().cues.len(), n);
    assert!(s.list("devices").unwrap()[0].ends_with(".json"));
}

#[test]
fn a_yanked_stick_keeps_every_flushed_line() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(1.0))))
        .unwrap();
    lib.flush();
    lib.apply(Edit::SetHotCue(t, 1, Some(HotCue::at(2.0))))
        .unwrap();
    // Yanked while the last line was being written.
    let seg = format!("journal/{}/00000001.log", dev(1));
    let mut bytes = s.read(&seg).unwrap();
    bytes.truncate(bytes.len() - 10);
    let stick = copy(&s);
    stick.put(&seg, bytes);
    let mut lib = mount(&stick, 1);
    let track = lib.track(t).unwrap();
    assert!(track.hot_cues[0].is_some() && track.hot_cues[1].is_none());
    assert_eq!(lib.stats().skipped_lines, 1);
    // The next session appends to a new segment, never after the torn line.
    lib.apply(Edit::SetHotCue(t, 2, Some(HotCue::at(3.0))))
        .unwrap();
    assert_eq!(stick.list(&format!("journal/{}", dev(1))).unwrap().len(), 2);
}

#[test]
fn a_read_only_or_failing_store_keeps_edits_in_ram() {
    let s = Arc::new(MemStore::new());
    mount(&s, 1).find_or_create(file("x.mp3", b"x"));
    let opts = MountOptions {
        read_only: true,
        ..MountOptions::new(dev(2))
    };
    let before = s.dump();
    let mut ro = Library::mount(Arc::clone(&s), opts).unwrap();
    let t = ro.find_or_create(file("y.mp3", b"y")).track;
    ro.apply(Edit::SetRating(t, Some(3))).unwrap();
    let blob = ro.put_blob(b"waveform");
    assert_eq!(ro.read_blob(blob).unwrap(), b"waveform");
    assert_eq!(ro.track(t).unwrap().rating, Some(3));
    assert!(matches!(
        ro.persistence(),
        Persistence::Unsaved { edits: 2, .. }
    ));
    assert_eq!(s.dump(), before, "nothing written");

    // A stick that fills up mid-session: the library says so and keeps going in RAM.
    let mut lib = mount(&s, 1);
    let x = lib.find_or_create(file("x.mp3", b"x")).track;
    s.set_fail_writes(true);
    lib.apply(Edit::SetRating(x, Some(1))).unwrap();
    assert!(matches!(lib.persistence(), Persistence::Unsaved { .. }));
    assert_eq!(lib.track(x).unwrap().rating, Some(1));
    assert!(lib.take_warnings().iter().any(|w| w.contains("RAM")));
}

#[test]
fn unknown_paths_fields_and_newer_formats() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    // A newer device's op: an unknown path, and a hot cue with a field we don't know.
    let stamp = Stamp {
        hlc: library::Hlc::new(u64::from(u32::MAX), 0),
        device: dev(3),
    };
    let mut future = Op::set(stamp, format!("track/{t}/stems"), json!({"blob": "…"}));
    future.extra.insert("sig".into(), json!("abc"));
    let hot = Op::set(
        stamp,
        format!("track/{t}/hotcue/4"),
        json!({"at": 4.0, "color": null, "comment": "", "quantized": true}),
    );
    lib.apply_remote(&[future.clone(), hot]);
    lib.flush();
    let snap = lib.snapshot().unwrap();
    assert!(snap.starts_with("snapshots/"));

    let mut again = mount(&copy(&s), 2);
    assert_eq!(again.stats().snapshots, 1);
    let kept = again.state().register(&format!("track/{t}/stems")).unwrap();
    assert_eq!(kept.extra.get("sig"), Some(&json!("abc")));
    let h = again.track(t).unwrap().hot_cues[4].clone().unwrap();
    assert_eq!(h.extra.get("quantized"), Some(&json!(true)));
    assert!(again.hlc() >= stamp.hlc, "the clock observed the remote op");

    let newer = Arc::new(MemStore::new());
    newer.put(
        "FORMAT",
        br#"{"format":"djplayer-library","version":2,"library":"x"}"#.to_vec(),
    );
    assert!(matches!(
        Library::mount(newer, MountOptions::new(dev(1))),
        Err(library::MountError::NewerFormat(2))
    ));
}

#[test]
fn libraries_address_tracks_by_library_and_track() {
    let mut libs = Libraries::new();
    let mut a = mount(&Arc::new(MemStore::new()), 1);
    let mut b = mount(&Arc::new(MemStore::new()), 1);
    let ta = a.find_or_create(file("x.mp3", b"x")).track;
    let tb = b.find_or_create(file("x.mp3", b"x")).track;
    assert_eq!(ta, tb, "the same file, the same id");
    a.apply(Edit::SetRating(ta, Some(5))).unwrap();
    let (ra, rb) = (a.track_ref(ta), b.track_ref(tb));
    libs.insert(a);
    libs.insert(b);
    assert_eq!(libs.track(ra).unwrap().rating, Some(5));
    assert_eq!(
        libs.track(rb).unwrap().rating,
        None,
        "data stays per library"
    );
}

#[test]
fn a_folder_library_on_disk() {
    let dir = std::env::temp_dir().join(format!("library-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let root = dir.join("DJPLAYER");
    let mut lib = Library::mount(FolderStore::new(&root), MountOptions::new(dev(1))).unwrap();
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(1.5))))
        .unwrap();
    let blob = lib.put_blob(b"blob bytes");
    lib.flush();
    let id = lib.id();
    drop(lib);
    for p in ["FORMAT", "devices", "journal", &blob.path()] {
        assert!(root.join(p).exists(), "{p}");
    }
    let mut lib = Library::mount(FolderStore::new(&root), MountOptions::new(dev(2))).unwrap();
    assert_eq!(lib.id(), id);
    assert_eq!(lib.track(t).unwrap().hot_cues[0].as_ref().unwrap().at, 1.5);
    assert_eq!(lib.read_blob(blob).unwrap(), b"blob bytes");
    std::fs::write(root.join(blob.path()), b"tampered").unwrap();
    assert!(lib.read_blob(blob).is_err(), "a corrupt blob is refused");
    std::fs::remove_dir_all(&dir).unwrap();
}
