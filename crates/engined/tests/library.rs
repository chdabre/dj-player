//! The library worker against a real folder and a real (synthetic) audio file: a loaded
//! track gets a record, and cues, the waveform and analysis written back come back on the
//! next load, from another device too. No audio device needed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use engined::persist::{FreshAnalysis, LibraryChoice, LibraryWorker, Loaded, Reply};
use library::{DeviceId, Edit, FolderStore, HotCue, Library, MountOptions};

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
            Some(Reply::Failed(e)) => panic!("{e}"),
            Some(Reply::Message(m)) => eprintln!("{m}"),
            Some(Reply::Status(_)) => {}
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
    assert_eq!(first.record.locations[0].1.path, "Music/tone.wav");
    assert!(first.waveform.is_none() && first.analysis.is_empty());
    let t = first.track.track;

    w.edit(Edit::SetHotCue(t, 2, Some(HotCue::at(1.25))));
    // Flushed within 2 s without the worker stopping: another device mounting the folder
    // now sees the edit.
    std::thread::sleep(Duration::from_millis(1500));
    let mut other = Library::mount(
        FolderStore::new(&lib_dir),
        MountOptions {
            read_only: true,
            ..MountOptions::new(DeviceId::random())
        },
    )
    .unwrap();
    assert_eq!(
        other.track(t).unwrap().hot_cues[2].as_ref().unwrap().at,
        1.25
    );

    let wf = waveform::compute::compute(&vec![0.25; 2 * 44_100], 44_100, &Default::default());
    w.waveform(wf.clone());
    w.analysis(vec![FreshAnalysis {
        name: "gridfit".into(),
        version: "1".into(),
        params: "p".into(),
        blob: b"{}".to_vec(),
    }]);
    w.audio_hash(library::AudioHash::of_pcm(&[0.0, 0.5]));
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
    assert!(again.record.audio_hash().is_some());
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
