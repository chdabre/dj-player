//! Damaged, hostile and concurrent stores: a bad line, a cut-short snapshot, a segment that
//! syncs late, compacted segments, two mounts as one device, unreadable files and a corrupt
//! blob.

use std::sync::Arc;

use library::codec::{self, Line, SegmentHeader};
use library::{
    Cue, DeviceId, Edit, FORMAT_VERSION, FileHash, FileIdentity, FolderStore, Hlc, HotCue, Library,
    MemStore, MountOptions, Op, Stamp, Store,
};

fn dev(n: u8) -> DeviceId {
    DeviceId::from_bytes([n; 16])
}

fn mount(store: &Arc<MemStore>, d: u8) -> Library {
    Library::mount(Arc::clone(store), MountOptions::new(dev(d))).unwrap()
}

fn file(path: &str, bytes: &[u8]) -> FileIdentity {
    FileIdentity::new(path, bytes.len() as u64, FileHash::of(bytes))
}

fn segment_path(d: u8, seg: u32) -> String {
    format!("journal/{}/{seg:08}.log", dev(d))
}

/// A whole segment of device `d` holding `ops`, as its writer would frame it.
fn segment(d: u8, seg: u32, ops: &[Op]) -> Vec<u8> {
    let header = SegmentHeader {
        segment: seg,
        device: dev(d),
        format: FORMAT_VERSION,
        opened: Hlc::ZERO,
        extra: Default::default(),
    };
    let mut text = codec::frame(&header.to_json());
    for op in ops {
        text += &codec::frame(&op.to_json());
    }
    text.into_bytes()
}

#[test]
fn one_line_stamped_at_the_top_of_the_clock_is_dropped() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.flush();
    drop(lib);
    // Someone else's stick: one CRC-valid line from the year 10889.
    let top = Stamp {
        hlc: Hlc(u64::MAX),
        device: dev(9),
    };
    let bad = Op::set(
        top,
        format!("track/{t}/hotcue/0"),
        serde_json::json!({"at": 9.0}),
    );
    s.put(&segment_path(9, 1), segment(9, 1, &[bad]));

    let mut lib = mount(&s, 1);
    assert_eq!(lib.stats().skipped_lines, 1);
    assert!(
        lib.hlc().is_plausible(),
        "the clock did not jump to {}",
        lib.hlc()
    );
    lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(1.0))))
        .unwrap();
    let a = lib.apply(Edit::AddCue(t, Cue::memory(2.0))).unwrap();
    let b = lib.apply(Edit::AddCue(t, Cue::memory(3.0))).unwrap();
    assert_ne!(a, b, "two edits get two element ids");
    lib.flush();
    drop(lib);
    let mut lib = mount(&s, 2);
    let rec = lib.track(t).unwrap();
    assert_eq!(
        rec.hot_cues[0].as_ref().unwrap().at,
        1.0,
        "the real edit wins"
    );
    assert_eq!(rec.cues.len(), 2);
}

/// Three mounts with an edit each (segments 1–3 of device 1) and a snapshot after them.
fn three_sessions_and_a_snapshot() -> (Arc<MemStore>, library::TrackId, String) {
    let s = Arc::new(MemStore::new());
    let mut t = None;
    for n in 0..3 {
        let mut lib = mount(&s, 1);
        let id = lib.find_or_create(file("x.mp3", b"x")).track;
        lib.apply(Edit::AddCue(id, Cue::memory(f64::from(n))))
            .unwrap();
        lib.flush();
        t = Some(id);
    }
    let snap = mount(&s, 1).snapshot().unwrap();
    (s, t.unwrap(), snap)
}

#[test]
fn a_whole_snapshot_covers_its_segments() {
    let (s, t, _) = three_sessions_and_a_snapshot();
    let mut lib = mount(&s, 2);
    assert_eq!(
        lib.stats().segments_covered,
        2,
        "1 and 2 skipped, 3 read again"
    );
    assert_eq!(lib.track(t).unwrap().cues.len(), 3);
}

#[test]
fn a_cut_short_snapshot_covers_nothing() {
    let (s, t, snap) = three_sessions_and_a_snapshot();
    let bytes = s.read(&snap).unwrap();
    // Cut exactly at a line boundary, after the header and one line: every remaining line
    // is valid, so only the missing end line tells.
    let cut: usize = bytes
        .split_inclusive(|&b| b == b'\n')
        .take(2)
        .map(<[u8]>::len)
        .sum();
    s.put(&snap, bytes[..cut].to_vec());
    let mut lib = mount(&s, 2);
    assert_eq!(lib.stats().skipped_lines, 0);
    assert_eq!(
        lib.stats().segments_covered,
        0,
        "nothing is skipped as covered"
    );
    assert_eq!(lib.track(t).unwrap().cues.len(), 3);
    assert!(lib.take_warnings().iter().any(|w| w.contains("incomplete")));
    // A snapshot written now holds what was read, so compacting against it loses nothing.
    lib.snapshot().unwrap();
    s.remove(&snap).unwrap();
    for seg in 1..=2 {
        s.remove(&segment_path(1, seg)).unwrap();
    }
    assert_eq!(mount(&s, 3).track(t).unwrap().cues.len(), 3);
}

#[test]
fn a_segment_that_syncs_late_is_still_read() {
    // Device 2 writes segments 1–3; segment 2 has not reached this copy yet.
    let b = Arc::new(MemStore::new());
    let mut t = None;
    for n in 0..3 {
        let mut lib = mount(&b, 2);
        let id = lib.find_or_create(file("x.mp3", b"x")).track;
        lib.apply(Edit::SetHotCue(id, n, Some(HotCue::at(n as f64))))
            .unwrap();
        lib.flush();
        t = Some(id);
    }
    let t = t.unwrap();
    let a = Arc::new(MemStore::new());
    for (k, v) in b.dump() {
        if k != segment_path(2, 2) {
            a.put(&k, v);
        }
    }
    mount(&a, 1).snapshot().unwrap();
    // Segment 2 arrives after the snapshot was written.
    a.put(&segment_path(2, 2), b.read(&segment_path(2, 2)).unwrap());
    let mut lib = mount(&a, 1);
    assert!(
        lib.track(t).unwrap().hot_cues[1].is_some(),
        "segment 2 was not skipped as covered"
    );
}

#[test]
fn a_device_never_reuses_a_segment_a_snapshot_holds() {
    let (s, t, _) = three_sessions_and_a_snapshot();
    // Compaction: device 1's segments are deleted, the snapshot holds them.
    for seg in 1..=3 {
        s.remove(&segment_path(1, seg)).unwrap();
    }
    let mut lib = mount(&s, 1);
    lib.apply(Edit::SetRating(t, Some(4))).unwrap();
    lib.flush();
    drop(lib);
    assert!(
        s.read(&segment_path(1, 4)).is_ok(),
        "the next number after the vector's"
    );
    assert_eq!(mount(&s, 1).track(t).unwrap().rating, Some(4));
}

#[test]
fn two_mounts_as_one_device_never_share_a_segment_or_an_id() {
    let s = Arc::new(MemStore::new());
    let mut first = mount(&s, 1);
    let t = first.find_or_create(file("x.mp3", b"x")).track;
    first.flush();
    drop(first);
    // Two decks on one stick, both mounted before either writes.
    let mut a = mount(&s, 1);
    let mut b = mount(&s, 1);
    let ia = a.apply(Edit::AddCue(t, Cue::memory(1.0))).unwrap();
    let ib = b.apply(Edit::AddCue(t, Cue::memory(2.0))).unwrap();
    a.flush();
    b.flush();
    assert_ne!(ia, ib);
    let journal = format!("journal/{}", dev(1));
    assert_eq!(
        s.list(&journal).unwrap().len(),
        3,
        "each mount wrote its own segment"
    );
    for name in s.list(&journal).unwrap() {
        let d = codec::decode(&s.read(&format!("{journal}/{name}")).unwrap());
        assert_eq!(d.skipped, 0);
        assert!(matches!(d.lines.first(), Some(Line::Segment(_))));
    }
    assert_eq!(mount(&s, 2).track(t).unwrap().cues.len(), 2);
}

#[test]
fn an_unreadable_file_skips_only_that_file() {
    let dir = std::env::temp_dir().join(format!("library-damaged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut lib = Library::mount(FolderStore::new(&dir), MountOptions::new(dev(1))).unwrap();
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(1.5))))
        .unwrap();
    lib.flush();
    drop(lib);
    // Folders where files should be (as a bad sector or a stray tool might leave).
    std::fs::create_dir_all(dir.join(segment_path(1, 9))).unwrap();
    std::fs::create_dir_all(dir.join("snapshots/x.snap")).unwrap();
    let mut lib = Library::mount(FolderStore::new(&dir), MountOptions::new(dev(1))).unwrap();
    assert_eq!(lib.stats().skipped_files, 2);
    assert_eq!(lib.track(t).unwrap().hot_cues[0].as_ref().unwrap().at, 1.5);
    // Segment 9 is taken (even unreadable): the next edit goes to 10.
    lib.apply(Edit::SetRating(t, Some(2))).unwrap();
    lib.flush();
    assert!(dir.join(segment_path(1, 10)).is_file());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_damaged_blob_is_rewritten() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let id = lib.put_blob(b"waveform bytes");
    s.put(&id.path(), b"cut sh".to_vec());
    assert!(lib.read_blob(id).is_err());
    assert_eq!(lib.put_blob(b"waveform bytes"), id);
    assert_eq!(lib.read_blob(id).unwrap(), b"waveform bytes");
    assert!(lib.take_warnings().iter().any(|w| w.contains("rewritten")));
}

#[test]
fn an_operation_with_a_header_like_field_stays_an_operation() {
    let stamp = Stamp {
        hlc: Hlc::new(1000, 0),
        device: dev(1),
    };
    let mut op = Op::set(stamp, "track/x/rating", 3.into());
    op.extra.insert("segment".into(), 7.into());
    op.extra.insert("snapshot".into(), true.into());
    let d = codec::decode(codec::frame(&op.to_json()).as_bytes());
    assert_eq!(d.lines, vec![Line::Op(op)]);
}

#[test]
fn a_trusted_clock_does_not_follow_a_fast_one() {
    const DAY: u64 = 24 * 60 * 60 * 1000;
    const NOW: u64 = 1_800_000_000_000; // 2027
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.flush();
    drop(lib);
    // A device whose clock ran two days fast left an edit.
    let fast = Stamp {
        hlc: Hlc::new(NOW + 2 * DAY, 0),
        device: dev(9),
    };
    let op = Op::set(fast, format!("track/{t}/rating"), 5.into());
    s.put(&segment_path(9, 1), segment(9, 1, &[op]));
    let opts = |trusted| MountOptions {
        now: || NOW,
        trusted_clock: trusted,
        ..MountOptions::new(dev(2))
    };
    let mut trusted = Library::mount(Arc::clone(&s), opts(true)).unwrap();
    assert!(
        trusted.hlc() < fast.hlc,
        "the trusted clock stays near its own time"
    );
    assert!(trusted.take_warnings().iter().any(|w| w.contains("ahead")));
    // The merge is the same for everyone: the fast edit is still there.
    assert_eq!(trusted.track(t).unwrap().rating, Some(5));
    let untrusted = Library::mount(Arc::clone(&s), opts(false)).unwrap();
    assert!(
        untrusted.hlc() >= fast.hlc,
        "without a trusted clock it follows the library"
    );
}

#[test]
fn a_ram_only_library_says_its_edits_are_unsaved() {
    let s = Arc::new(MemStore::new());
    let mut lib = mount(&s, 1);
    lib.keep_in_ram("no library: --no-library");
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.apply(Edit::SetRating(t, Some(3))).unwrap();
    assert!(matches!(
        lib.persistence(),
        library::Persistence::Unsaved { edits: 2, .. }
    ));
    assert!(s.list(&format!("journal/{}", dev(1))).unwrap().is_empty());
}

#[test]
fn another_program_writing_as_this_device_keeps_edits_in_ram() {
    let dir = std::env::temp_dir().join(format!("library-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut lib = Library::mount(FolderStore::new(&dir), MountOptions::new(dev(1))).unwrap();
    let t = lib.find_or_create(file("x.mp3", b"x")).track;
    lib.flush();
    // Two libraries in one process share the lock (and their stamps).
    let mut same = Library::mount(FolderStore::new(&dir), MountOptions::new(dev(1))).unwrap();
    same.apply(Edit::SetRating(t, Some(1))).unwrap();
    assert!(same.is_writable());
    drop((lib, same));
    // Another program holds it: its own open file, not this process's shared one.
    let held = std::fs::File::open(dir.join(format!("journal/{}/writer.lock", dev(1)))).unwrap();
    held.try_lock().unwrap();
    let mut lib = Library::mount(FolderStore::new(&dir), MountOptions::new(dev(1))).unwrap();
    lib.apply(Edit::SetRating(t, Some(2))).unwrap();
    match lib.persistence() {
        library::Persistence::Unsaved { reason, .. } => assert!(reason.contains("another program")),
        p => panic!("{p:?}"),
    }
    assert_eq!(lib.track(t).unwrap().rating, Some(2), "kept in RAM");
    drop(held);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn leftovers_of_interrupted_writes_are_swept() {
    let dir = std::env::temp_dir().join(format!("library-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tmp")).unwrap();
    let old = dir.join("tmp/1234-0.tmp");
    std::fs::write(&old, b"half a snapshot").unwrap();
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();
    let fresh = dir.join("tmp/5678-0.tmp");
    std::fs::write(&fresh, b"being written now").unwrap();
    let mut lib = Library::mount(FolderStore::new(&dir), MountOptions::new(dev(1))).unwrap();
    assert!(!old.exists(), "an old leftover is removed");
    assert!(
        fresh.exists(),
        "a file a live writer may still be using stays"
    );
    assert!(lib.take_warnings().iter().any(|w| w.contains("leftover")));
    std::fs::remove_dir_all(&dir).unwrap();
}
