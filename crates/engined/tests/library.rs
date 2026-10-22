//! The library worker against a real folder and a real (synthetic) audio file: a loaded
//! track gets a record, and cues, the waveform and analysis written back come back on the
//! next load, on another device too. No audio device needed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use engined::store::{FreshAnalysis, LibraryChoice, LibraryWorker, Loaded, Reply};
use library::{DeviceId, Edit, HotCue, Persistence};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("engined-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Two seconds of a 16-bit stereo tone.
fn write_wav(path: &Path, freq: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..88_200 {
        let x = (std::f32::consts::TAU * freq * i as f32 / 44_100.0).sin();
        let s = (x * 12_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn wait_loaded(w: &LibraryWorker) -> Loaded {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        match w.try_recv() {
            Some(Reply::Loaded(l)) => return *l,
            Some(Reply::Failed(e) | Reply::Unrecorded { reason: e, .. }) => panic!("{e}"),
            Some(Reply::Message(m)) => eprintln!("{m}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("no record");
}

#[test]
fn edits_waveform_and_analysis_come_back_on_the_next_load() {
    let dir = temp_dir("worker");
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    let track = music.join("tone.wav");
    write_wav(&track, 440.0);
    let lib_dir = dir.join("DJPLAYER");
    let player = DeviceId::random();

    let w = LibraryWorker::spawn_as(
        Some(player),
        LibraryChoice::Folder(lib_dir.clone()),
        track.clone(),
    )
    .unwrap();
    let first = wait_loaded(&w);
    assert_eq!(first.record.locations.len(), 1);
    assert_eq!(first.record.locations[0].path, "Music/tone.wav");
    assert!(first.waveform.is_none() && first.analysis.is_empty());
    let t = first.track.track;

    w.edit(Edit::SetHotCue(t, 2, Some(HotCue::at(1.25))));
    // Committed within 2 s without the worker stopping.
    let until = Instant::now() + Duration::from_secs(2);
    let mut saved = false;
    while !saved && Instant::now() < until {
        match w.try_recv() {
            Some(Reply::Status(Persistence::Saved)) => saved = true,
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert!(saved, "the edit was committed");

    let wf = waveform::compute::compute(&vec![0.25; 2 * 44_100], 44_100, &Default::default());
    // spawn_as opens the file under serial 0.
    w.waveform(0, wf.clone());
    w.analysis(
        0,
        vec![FreshAnalysis {
            name: "gridfit".into(),
            version: "1".into(),
            params: "p".into(),
            blob: b"{}".to_vec(),
        }],
    );
    w.audio_hash(0, library::AudioHash::of_pcm(&[0.0, 0.5]));
    drop(w); // waits for the final flush

    // The same file loaded again, on another player.
    let w = LibraryWorker::spawn_as(
        Some(DeviceId::random()),
        LibraryChoice::Folder(lib_dir.clone()),
        track.clone(),
    )
    .unwrap();
    let again = wait_loaded(&w);
    assert_eq!(again.track, first.track, "same library, same track");
    assert_eq!(again.record.hot_cues[2].as_ref().unwrap().at, 1.25);
    assert_eq!(again.waveform.as_ref().unwrap().entries, wf.entries);
    let (stored, blob) = &again.analysis["gridfit"];
    assert_eq!(
        (stored.version.as_str(), blob.as_slice()),
        ("1", &b"{}"[..])
    );
    assert!(again.record.audio_hash.is_some());
    drop(w);

    // An exact copy elsewhere on the stick is the same track at a second location.
    let copy = dir.join("copy.wav");
    std::fs::copy(&track, &copy).unwrap();
    let w = LibraryWorker::spawn_as(Some(player), LibraryChoice::Folder(lib_dir), copy).unwrap();
    let dup = wait_loaded(&w);
    assert_eq!(dup.track.track, t);
    assert_eq!(dup.record.locations.len(), 2);
    assert!(dup.record.has_duplicates());
    drop(w);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_a_library_edits_live_in_ram() {
    let dir = temp_dir("ram");
    let track = dir.join("tone.wav");
    write_wav(&track, 220.0);
    let w = LibraryWorker::spawn_as(Some(DeviceId::random()), LibraryChoice::Ram, track).unwrap();
    let l = wait_loaded(&w);
    w.edit(Edit::SetHotCue(l.track.track, 0, Some(HotCue::at(0.5))));
    drop(w);
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "nothing but the track"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The next [`Reply::Loaded`] or [`Reply::Unrecorded`], with its serial.
fn wait_answer(w: &LibraryWorker) -> Result<Loaded, (u64, String)> {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        match w.try_recv() {
            Some(Reply::Loaded(l)) => return Ok(*l),
            Some(Reply::Unrecorded { serial, reason }) => return Err((serial, reason)),
            Some(Reply::Failed(e)) => panic!("{e}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("no answer");
}

#[test]
fn one_worker_loads_track_after_track() {
    let dir = temp_dir("reload");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, 440.0);
    write_wav(&b, 660.0);
    let w = LibraryWorker::start(
        Some(DeviceId::random()),
        LibraryChoice::Folder(dir.join("DJPLAYER")),
    )
    .unwrap();

    w.open(1, a.clone());
    let la = wait_answer(&w).unwrap();
    assert_eq!(la.serial, 1);
    // The load filled the tag cache from the file (a bare WAV: its length only).
    assert!(
        la.record
            .tags
            .duration
            .is_some_and(|d| (d - 2.0).abs() < 0.01)
    );
    w.edit(Edit::SetHotCue(la.track.track, 0, Some(HotCue::at(0.5))));

    w.open(2, b.clone());
    let lb = wait_answer(&w).unwrap();
    assert_eq!(lb.serial, 2);
    assert_ne!(lb.track, la.track);
    assert!(lb.record.hot_cues.iter().all(Option::is_none));
    // Requests name the load they are for.
    let wf = waveform::compute::compute(&vec![0.25; 2 * 44_100], 44_100, &Default::default());
    w.waveform(2, wf);

    w.open(3, a.clone());
    let again = wait_answer(&w).unwrap();
    assert_eq!((again.serial, again.track), (3, la.track));
    assert_eq!(again.record.hot_cues[0].as_ref().map(|h| h.at), Some(0.5));
    assert!(again.waveform.is_none(), "the waveform went to b");

    w.open(4, b);
    assert!(wait_answer(&w).unwrap().waveform.is_some());

    // A file that can't be read gets no record, and the worker carries on.
    w.open(5, dir.join("missing.wav"));
    assert_eq!(wait_answer(&w).unwrap_err().0, 5);
    w.open(6, a);
    assert_eq!(wait_answer(&w).unwrap().track, la.track);
}

#[test]
fn two_open_tracks_each_get_their_own_requests_until_closed() {
    let dir = temp_dir("two-open");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, 440.0);
    write_wav(&b, 660.0);
    let w = LibraryWorker::start(
        Some(DeviceId::random()),
        LibraryChoice::Folder(dir.join("DJPLAYER")),
    )
    .unwrap();
    // Two decks: a under serial 1, b under serial 2, both open at once.
    w.open(1, a.clone());
    let la = wait_answer(&w).unwrap();
    w.open(2, b.clone());
    let lb = wait_answer(&w).unwrap();
    let wf = waveform::compute::compute(&vec![0.25; 2 * 44_100], 44_100, &Default::default());
    // The request for a, sent after b was opened, still goes to a.
    w.waveform(1, wf.clone());
    // b is let go: its request is dropped.
    w.close(2);
    w.waveform(2, wf);
    w.open(3, a);
    let again = wait_answer(&w).unwrap();
    assert_eq!(again.track, la.track);
    assert!(again.waveform.is_some(), "the waveform went to a");
    w.open(4, b);
    let b_again = wait_answer(&w).unwrap();
    assert_eq!(b_again.track, lb.track);
    assert!(
        b_again.waveform.is_none(),
        "b was closed before its waveform came"
    );
}

#[test]
fn a_scan_records_every_track_with_its_tags_and_publishes_the_listing() {
    let dir = temp_dir("scan");
    std::fs::create_dir_all(dir.join("UKG/deep")).unwrap();
    for (p, f) in [
        ("a.wav", 440.0),
        ("UKG/b.wav", 550.0),
        ("UKG/deep/c.wav", 660.0),
    ] {
        write_wav(&dir.join(p), f);
    }
    std::fs::write(dir.join("notes.txt"), b"not audio").unwrap();
    let w = LibraryWorker::start(
        Some(DeviceId::random()),
        LibraryChoice::Folder(dir.join("DJPLAYER")),
    )
    .unwrap();
    w.scan();
    let until = Instant::now() + Duration::from_secs(10);
    let mut listing = None;
    let mut finished = false;
    while !(finished
        && listing
            .as_ref()
            .is_some_and(|l: &std::sync::Arc<engined::store::Listing>| l.len() == 3))
    {
        assert!(Instant::now() < until, "scan did not finish");
        match w.try_recv() {
            Some(Reply::Scan { done, total }) => {
                assert_eq!(total, 3);
                finished = done == total;
            }
            Some(Reply::Listing(l)) => listing = Some(l),
            Some(Reply::Failed(e)) => panic!("{e}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let listing = listing.unwrap();
    let mut paths: Vec<&str> = listing.keys().map(String::as_str).collect();
    paths.sort();
    assert_eq!(paths, ["UKG/b.wav", "UKG/deep/c.wav", "a.wav"]);
    let d = listing["a.wav"].tags.duration.unwrap();
    assert!((d - 2.0).abs() < 0.01, "{d}");
}

#[test]
fn analyze_all_stores_waveform_analysis_and_tags_for_every_file() {
    use engined::batch::{Batch, BatchEvent};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = temp_dir("batch");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, 440.0);
    write_wav(&b, 660.0);
    let w = LibraryWorker::start(
        Some(DeviceId::random()),
        LibraryChoice::Folder(dir.join("DJPLAYER")),
    )
    .unwrap();
    // Held while "the deck is busy", then let go.
    let busy = Arc::new(AtomicBool::new(true));
    let job = Batch::start(
        vec![a.clone(), b.clone(), dir.join("missing.wav")],
        Default::default(),
        48_000,
        Arc::clone(&busy),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        job.try_recv().is_none(),
        "nothing starts while the deck is busy"
    );
    busy.store(false, Ordering::Relaxed);
    let until = Instant::now() + Duration::from_secs(60);
    let (mut done, mut failed) = (0, 0);
    loop {
        assert!(Instant::now() < until, "batch did not finish");
        match job.try_recv() {
            Some(BatchEvent::Done(r)) => {
                assert!(!r.waveform.entries.is_empty() && !r.fresh.is_empty());
                assert!(r.audio_hash.is_some());
                w.store(*r);
                done += 1;
            }
            Some(BatchEvent::Failed(..)) => failed += 1,
            Some(BatchEvent::Finished { stopped }) => {
                assert!(!stopped);
                break;
            }
            Some(BatchEvent::Started(..)) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert_eq!((done, failed), (2, 1));
    // The listing comes back with both marked analysed.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < until, "listing never showed them analysed");
        match w.try_recv() {
            Some(Reply::Listing(l)) if l.len() == 2 && l.values().all(|x| x.analysed) => break,
            Some(Reply::Failed(e)) => panic!("{e}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[test]
fn files_go_into_playlists_and_the_playlists_come_back() {
    use library::PlaylistId;
    let dir = temp_dir("playlists");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, 440.0);
    write_wav(&b, 660.0);
    let w = LibraryWorker::start(
        Some(DeviceId::random()),
        LibraryChoice::Folder(dir.join("DJPLAYER")),
    )
    .unwrap();
    // Neither file has a record yet: adding makes them.
    w.add_files(PlaylistId::TAG_LIST, vec![b.clone(), a.clone()]);
    let set = PlaylistId::random();
    w.new_playlist(set, None, "Set".into(), vec![a.clone(), a.clone()]);
    let until = Instant::now() + Duration::from_secs(10);
    let lists = loop {
        assert!(Instant::now() < until, "playlists never came back");
        match w.try_recv() {
            Some(Reply::Playlists(p))
                if p.entries.get(&PlaylistId::TAG_LIST).map(Vec::len) == Some(2)
                    && p.entries.get(&set).map(Vec::len) == Some(2) =>
            {
                break p;
            }
            Some(Reply::Failed(e)) => panic!("{e}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let names: Vec<&str> = lists.lists.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["TAG LIST", "Set"]);
    let tag = &lists.entries[&PlaylistId::TAG_LIST];
    assert_ne!(tag[0].track, tag[1].track, "b then a");
    let s = &lists.entries[&set];
    assert_eq!(s[0].track, s[1].track, "a twice");
    assert_eq!(s[0].track, tag[1].track);
    assert_eq!(lists.tagged().count(), 2);

    // Edits go through as library edits; the playlists come back after each.
    w.edit(Edit::RemoveEntries(vec![tag[0].id]));
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < until);
        match w.try_recv() {
            Some(Reply::Playlists(p)) if p.entries[&PlaylistId::TAG_LIST].len() == 1 => break,
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}
