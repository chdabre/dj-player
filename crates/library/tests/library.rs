//! Identity, edits and persistence against a library folder on disk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use library::{
    AnalysisRef, AudioHash, BlobId, Cue, DeviceId, Edit, FileHash, FileIdentity, HotCue, Library,
    MountOptions, Persistence, WaveformRef, WriteError, track_id,
};

/// A fresh `DJPLAYER/` folder path (not made yet) in a fresh temporary folder.
fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("DJPLAYER")
}

fn dev(n: u8) -> DeviceId {
    DeviceId::from_bytes([n; 16])
}

fn mount(dir: &Path) -> Library {
    Library::mount(dir, MountOptions::new(dev(1))).unwrap()
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

/// A clock that moves on by a second at every reading.
fn ticking() -> u64 {
    static NOW: AtomicU64 = AtomicU64::new(1_790_000_000_000);
    NOW.fetch_add(1000, Ordering::Relaxed)
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_new_file_gets_the_id_of_its_hash_and_keeps_it() {
    let dir = temp("id");
    let f = file("Music/a.mp3", b"aaaa");
    let id = {
        let mut lib = mount(&dir);
        assert!(lib.stats().created);
        let found = lib.find_or_create(f.clone()).unwrap();
        assert!(found.created && found.new_location);
        assert_eq!(found.track, track_id(&f.file_hash));
        assert_eq!(lib.persistence(), Persistence::Pending);
        lib.flush();
        assert_eq!(lib.persistence(), Persistence::Saved);
        found.track
    };
    let mut lib = mount(&dir);
    assert!(!lib.stats().created);
    assert_eq!(lib.stats().tracks, 1);
    let again = lib.find_or_create(f).unwrap();
    assert_eq!(
        (again.track, again.created, again.new_location),
        (id, false, false)
    );
    assert_eq!(lib.persistence(), Persistence::Saved, "nothing to write");
    drop(lib);
    cleanup(&dir);
}

#[test]
fn an_exact_duplicate_is_one_track_with_two_locations() {
    let dir = temp("dup");
    let mut lib = mount(&dir);
    let a = lib.find_or_create(file("Music/a.mp3", b"same")).unwrap();
    let b = lib.find_or_create(file("Backup/a.mp3", b"same")).unwrap();
    assert_eq!(a.track, b.track);
    assert!(!b.created && b.new_location);
    let t = lib.track(a.track).unwrap().unwrap();
    assert_eq!(t.locations.len(), 2);
    assert!(t.has_duplicates());
    drop(lib);
    cleanup(&dir);
}

#[test]
fn a_tag_edited_copy_merges_by_audio_hash_into_the_older_track() {
    let dir = temp("merge");
    let mut lib = Library::mount(
        &dir,
        MountOptions {
            now: ticking,
            ..MountOptions::new(dev(1))
        },
    )
    .unwrap();
    let old = lib
        .find_or_create(file("a.mp3", b"tags v1 + audio"))
        .unwrap()
        .track;
    lib.apply(Edit::SetHotCue(old, 0, Some(HotCue::at(1.0))))
        .unwrap();
    lib.apply(Edit::SetHotCue(old, 1, Some(HotCue::at(2.0))))
        .unwrap();
    lib.apply(Edit::AddCue(old, Cue::memory(3.0))).unwrap();
    lib.apply(Edit::SetRating(old, Some(4))).unwrap();
    assert_eq!(lib.set_audio_hash(old, audio(7)).unwrap(), old);

    let copy = lib
        .find_or_create(file("b.mp3", b"tags v2 + audio"))
        .unwrap()
        .track;
    assert_ne!(copy, old);
    lib.apply(Edit::SetHotCue(copy, 1, Some(HotCue::at(2.5))))
        .unwrap();
    lib.apply(Edit::SetHotCue(copy, 2, Some(HotCue::at(4.0))))
        .unwrap();
    lib.apply(Edit::AddCue(copy, Cue::memory(5.0))).unwrap();
    assert_eq!(
        lib.set_audio_hash(copy, audio(7)).unwrap(),
        old,
        "the older survives"
    );
    lib.flush();

    let t = lib.track(copy).unwrap().unwrap();
    assert_eq!(t.id, old);
    assert_eq!(t.merged, vec![copy]);
    assert_eq!(t.locations.len(), 2);
    assert_eq!(t.rating, Some(4));
    let hot: Vec<Option<f64>> = t.hot_cues[..3]
        .iter()
        .map(|h| h.as_ref().map(|h| h.at))
        .collect();
    // Slot 1 was set on both: the later edit, the copy's, wins.
    assert_eq!(hot, [Some(1.0), Some(2.5), Some(4.0)]);
    assert_eq!(t.cues.len(), 2);
    assert_eq!(lib.tracks().unwrap(), vec![old]);
    assert_eq!(lib.resolve(copy), old);

    // Edits to the merged id go to the survivor.
    lib.apply(Edit::SetRating(copy, Some(5))).unwrap();
    assert_eq!(lib.track(old).unwrap().unwrap().rating, Some(5));
    drop(lib);
    cleanup(&dir);
}

#[test]
fn a_different_encode_is_a_separate_track() {
    let dir = temp("encode");
    let mut lib = mount(&dir);
    let a = lib.find_or_create(file("a.mp3", b"mp3")).unwrap().track;
    let b = lib.find_or_create(file("a.flac", b"flac")).unwrap().track;
    assert_eq!(lib.set_audio_hash(a, audio(1)).unwrap(), a);
    assert_eq!(lib.set_audio_hash(b, audio(2)).unwrap(), b);
    assert_eq!(lib.tracks().unwrap().len(), 2);
    drop(lib);
    cleanup(&dir);
}

#[test]
fn edits_analysis_and_waveform_come_back() {
    let dir = temp("edits");
    let blob = BlobId::of(b"analysis");
    let (t, cue_id) = {
        let mut lib = mount(&dir);
        let t = lib.find_or_create(file("a.mp3", b"x")).unwrap().track;
        let mut h = HotCue::at(0.1 + 0.2);
        h.end = Some(1.5);
        h.color = Some("rose".into());
        h.comment = "drop".into();
        lib.apply(Edit::SetHotCue(t, 15, Some(h))).unwrap();
        lib.apply(Edit::SetHotCue(t, 3, Some(HotCue::at(9.0))))
            .unwrap();
        lib.apply(Edit::SetHotCue(t, 3, None)).unwrap();
        let id = lib
            .apply(Edit::AddCue(t, Cue::memory_loop(1.0, 2.0)))
            .unwrap()
            .unwrap();
        let gone = lib
            .apply(Edit::AddCue(t, Cue::memory(0.5)))
            .unwrap()
            .unwrap();
        lib.apply(Edit::RemoveCue(t, gone.clone())).unwrap();
        assert_eq!(
            lib.apply(Edit::RemoveCue(t, gone.clone())),
            Err(WriteError::NoSuchCue(gone))
        );
        let mut moved = Cue::memory_loop(1.25, 2.0);
        moved.comment = "moved".into();
        assert_eq!(
            lib.apply(Edit::ReplaceCue(t, id.clone(), moved)).unwrap(),
            Some(id.clone())
        );
        assert_eq!(lib.put_blob(b"analysis"), blob);
        lib.apply(Edit::SetAnalysis(
            t,
            "tempocnn".into(),
            AnalysisRef {
                version: "1".into(),
                params: "p".into(),
                blob,
                created: "2026-10-21T00:00:00Z".into(),
            },
        ))
        .unwrap();
        let wf = lib.put_blob(b"waveform");
        let ov = lib.put_blob(b"overview");
        lib.apply(Edit::SetWaveform(t, WaveformRef::new(wf, ov)))
            .unwrap();
        assert_eq!(
            lib.apply(Edit::SetHotCue(t, 16, None)),
            Err(WriteError::BadHotCueSlot(16))
        );
        assert!(matches!(
            lib.apply(Edit::SetAnalysis(
                t,
                "../x".into(),
                lib.track(t).unwrap().unwrap().analysis["tempocnn"].clone()
            )),
            Err(WriteError::BadAnalyzerName(_))
        ));
        let nobody = library::TrackId([9; 16]);
        assert_eq!(
            lib.apply(Edit::SetRating(nobody, Some(1))),
            Err(WriteError::NoSuchTrack(nobody))
        );
        (t, id)
    };
    let lib = mount(&dir);
    let r = lib.track(t).unwrap().unwrap();
    let h = r.hot_cues[15].as_ref().unwrap();
    assert_eq!(
        (h.at, h.end, h.comment.as_str()),
        (0.1 + 0.2, Some(1.5), "drop")
    );
    assert_eq!(h.color.as_deref(), Some("rose"));
    assert!(r.hot_cues[3].is_none());
    assert_eq!(r.cues.len(), 1);
    assert_eq!(r.cues[0].0, cue_id);
    assert_eq!(
        (r.cues[0].1.at, r.cues[0].1.comment.as_str()),
        (1.25, "moved")
    );
    assert_eq!(
        lib.read_blob(r.analysis["tempocnn"].blob).unwrap(),
        b"analysis"
    );
    let w = r.waveform.unwrap();
    assert_eq!(lib.read_blob(w.overview_blob).unwrap(), b"overview");
    drop(lib);
    cleanup(&dir);
}

#[test]
fn the_folder_holds_the_database_its_backup_and_blobs() {
    let dir = temp("layout");
    {
        let mut lib = mount(&dir);
        lib.find_or_create(file("a.mp3", b"x")).unwrap();
        lib.put_blob(b"blob");
    }
    let names = |d: &PathBuf| {
        let mut n: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    };
    // Closed cleanly: no WAL left; a new library has nothing to back up.
    assert_eq!(names(&dir), ["blobs", "library.db"]);
    {
        let mut lib = mount(&dir);
        assert!(!lib.stats().needs_check);
        lib.find_or_create(file("b.mp3", b"y")).unwrap();
    }
    assert_eq!(names(&dir), ["blobs", "library.db", "library.db.bak"]);
    // The backup is the library as it was before that mount wrote.
    let bak = std::env::temp_dir().join(format!("library-bak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bak);
    std::fs::create_dir_all(&bak).unwrap();
    std::fs::copy(dir.join("library.db.bak"), bak.join("library.db")).unwrap();
    assert_eq!(mount(&bak).tracks().unwrap().len(), 1);
    std::fs::remove_dir_all(&bak).unwrap();
    cleanup(&dir);
}

#[test]
fn a_read_only_stick_keeps_edits_in_ram_until_it_can_be_written() {
    let dir = temp("ro");
    let t = {
        let mut lib = mount(&dir);
        lib.find_or_create(file("a.mp3", b"x")).unwrap().track
    };
    let before = std::fs::read(dir.join("library.db")).unwrap();
    let opts = MountOptions {
        read_only: true,
        ..MountOptions::new(dev(2))
    };
    {
        let mut lib = Library::mount(&dir, opts.clone()).unwrap();
        lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(1.0))))
            .unwrap();
        lib.put_blob(b"in ram");
        lib.flush();
        assert!(matches!(
            lib.persistence(),
            Persistence::Unsaved { edits: 1, .. }
        ));
        assert_eq!(
            lib.track(t).unwrap().unwrap().hot_cues[0]
                .as_ref()
                .unwrap()
                .at,
            1.0
        );
    }
    assert_eq!(std::fs::read(dir.join("library.db")).unwrap(), before);
    {
        // Still read-only: the kept edit is there again, still unsaved.
        let lib = Library::mount(&dir, opts).unwrap();
        assert!(lib.track(t).unwrap().unwrap().hot_cues[0].is_some());
        assert_eq!(lib.read_blob(BlobId::of(b"in ram")).unwrap(), b"in ram");
    }
    {
        // Writable again: the edit and the blob are written.
        let lib = mount(&dir);
        assert_eq!(lib.stats().replayed, 1);
        assert_eq!(lib.persistence(), Persistence::Saved);
    }
    let lib = mount(&dir);
    assert_eq!(
        lib.track(t).unwrap().unwrap().hot_cues[0]
            .as_ref()
            .unwrap()
            .at,
        1.0
    );
    assert!(dir.join(BlobId::of(b"in ram").path()).exists());
    drop(lib);
    cleanup(&dir);
}

#[test]
fn a_failing_write_keeps_the_queue_and_retries() {
    let dir = temp("fail");
    let mut lib = mount(&dir);
    let t = lib.find_or_create(file("a.mp3", b"x")).unwrap().track;
    lib.flush();
    // Something in the way of the blob's folder: the write fails like a pulled stick's.
    let blob = BlobId::of(b"waveform");
    let block = dir.join(blob.path()).parent().unwrap().to_path_buf();
    std::fs::write(&block, b"in the way").unwrap();
    lib.put_blob(b"waveform");
    lib.apply(Edit::SetWaveform(t, WaveformRef::new(blob, blob)))
        .unwrap();
    lib.apply(Edit::SetRating(t, Some(3))).unwrap();
    lib.flush();
    let Persistence::Unsaved { edits, reason } = lib.persistence() else {
        panic!("{:?}", lib.persistence());
    };
    assert_eq!(edits, 2);
    assert!(reason.contains("can't write"), "{reason}");
    assert!(!lib.take_warnings().is_empty());
    // The way is clear again: the next flush writes the blob, then the queue.
    std::fs::remove_file(&block).unwrap();
    lib.flush();
    assert_eq!(lib.persistence(), Persistence::Saved);
    drop(lib);
    let lib = mount(&dir);
    let r = lib.track(t).unwrap().unwrap();
    assert_eq!(r.rating, Some(3));
    assert_eq!(
        lib.read_blob(r.waveform.unwrap().blob).unwrap(),
        b"waveform"
    );
    drop(lib);
    cleanup(&dir);
}

#[test]
fn without_a_library_edits_carry_over_between_loads() {
    let opts = MountOptions::new(dev(3));
    let t = {
        let mut lib = Library::ram(opts.clone(), "no stick");
        let t = lib.find_or_create(file("a.mp3", b"ram")).unwrap().track;
        lib.apply(Edit::SetRating(t, Some(2))).unwrap();
        assert!(matches!(lib.persistence(), Persistence::Unsaved { .. }));
        t
    };
    let lib = Library::ram(opts, "no stick");
    assert_eq!(lib.track(t).unwrap().unwrap().rating, Some(2));
}

#[test]
fn tags_are_kept_and_listed_by_path() {
    let dir = temp("tags");
    let (a, b) = (file("UKG/a.mp3", b"aaaa"), file("b.mp3", b"bbbb"));
    let tags = library::TrackTags {
        title: Some("Red Flag".into()),
        artist: Some("Sunship".into()),
        bpm: Some(132.0),
        key: Some("8A".into()),
        duration: Some(142.1),
        ..Default::default()
    };
    {
        let mut lib = mount(&dir);
        let ta = lib.find_or_create(a).unwrap().track;
        let tb = lib.find_or_create(b).unwrap().track;
        lib.apply(Edit::SetTags(ta, tags.clone())).unwrap();
        // b gets a waveform and an analysis: analysed.
        let blob = lib.put_blob(b"analysis");
        let a = AnalysisRef {
            version: "1".into(),
            params: "p".into(),
            blob,
            created: "2026-10-21T00:00:00Z".into(),
        };
        lib.apply(Edit::SetAnalysis(tb, "gridfit".into(), a))
            .unwrap();
        let wf = lib.put_blob(b"waveform");
        lib.apply(Edit::SetWaveform(tb, WaveformRef::new(wf, wf)))
            .unwrap();
        lib.flush();
    }
    let lib = mount(&dir);
    let listed = lib.listing().unwrap();
    let paths: Vec<&str> = listed.iter().map(|l| l.path.as_str()).collect();
    assert_eq!(paths, ["UKG/a.mp3", "b.mp3"]);
    assert_eq!(listed[0].tags, tags);
    assert!(!listed[0].analysed, "no waveform or analysis yet");
    assert_eq!(listed[1].tags, library::TrackTags::default());
    assert!(listed[1].analysed);
    assert_eq!(lib.track(listed[0].track).unwrap().unwrap().tags, tags);
    cleanup(&dir);
}
