//! Mount and replay time for a large synthetic library.
//!
//! `cargo run --release -p library --example scale [tracks] [dir]`: builds a library of
//! `tracks` (default 10 000) tracks in `dir` (default a temporary folder), each with a file
//! identity and its audio hash, 8 hot cues, 3 memory cues, a rating, 3 analysis results and
//! a waveform, written by 3 devices over many sessions. Then times mounting it from the
//! journals, writing a snapshot, and mounting it from the snapshot.

use std::path::PathBuf;
use std::time::Instant;

use library::{
    AnalysisRef, AudioHash, Cue, DeviceId, Edit, FileHash, FileIdentity, FolderStore, HotCue,
    Library, MountOptions, Store, WaveformRef,
};

fn dir_size(p: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let tracks: usize = args.next().map_or(Ok(10_000), |s| s.parse())?;
    let dir = args.next().map_or_else(
        || std::env::temp_dir().join(format!("library-scale-{}", std::process::id())),
        PathBuf::from,
    );
    let _ = std::fs::remove_dir_all(&dir);
    let root = dir.join("DJPLAYER");
    let devices: Vec<DeviceId> = (0..3).map(|_| DeviceId::random()).collect();

    let started = Instant::now();
    // 50 sessions, round-robin over the devices: each mounts, edits its share, unmounts.
    let sessions = 50;
    let fake_blob = |n: usize, what: &str| library::BlobId::of(format!("{what}{n}").as_bytes());
    for s in 0..sessions {
        let device = devices[s % devices.len()];
        let mut lib = Library::mount(FolderStore::new(&root), MountOptions::new(device))?;
        for i in (s..tracks).step_by(sessions) {
            let bytes = format!("file {i}");
            let fh = FileHash::of(bytes.as_bytes());
            let id = lib
                .find_or_create(FileIdentity::new(
                    format!("Music/Artist {}/Track {i}.mp3", i % 300),
                    8_000_000 + i as u64,
                    fh,
                ))
                .track;
            let mut ah = [0u8; 32];
            ah[..8].copy_from_slice(&(i as u64).to_le_bytes());
            lib.set_audio_hash(id, fh, AudioHash { v: 1, hash: ah });
            for slot in 0..8 {
                lib.apply(Edit::SetHotCue(
                    id,
                    slot,
                    Some(HotCue::at(slot as f64 * 15.0 + 0.123)),
                ))?;
            }
            for k in 0..3 {
                lib.apply(Edit::AddCue(id, Cue::memory(k as f64 * 32.0 + 0.5)))?;
            }
            lib.apply(Edit::SetRating(id, Some((i % 6) as u8)))?;
            for name in ["tempocnn", "gridfit", "hpcpkey"] {
                let a = AnalysisRef {
                    version: "0.1.0".into(),
                    params: "default".into(),
                    blob: fake_blob(i, name),
                    created: library::rfc3339(library::system_ms()),
                };
                lib.apply(Edit::SetAnalysis(id, name.into(), a))?;
            }
            let w = WaveformRef {
                blob: fake_blob(i, "detail"),
                overview_blob: fake_blob(i, "overview"),
            };
            lib.apply(Edit::SetWaveform(id, w))?;
        }
        lib.flush();
    }
    let built = started.elapsed();
    let journal_bytes = dir_size(&root.join("journal"));
    let segments: usize = FolderStore::new(&root)
        .list("journal")?
        .iter()
        .map(|d| {
            FolderStore::new(&root)
                .list(&format!("journal/{d}"))
                .map_or(0, |v| v.len())
        })
        .sum();

    let t = Instant::now();
    let mut lib = Library::mount(FolderStore::new(&root), MountOptions::new(devices[0]))?;
    let mount_journal = t.elapsed();
    let ops = lib.stats().ops;
    assert_eq!(lib.tracks().len(), tracks);

    let t = Instant::now();
    let probe = lib.tracks()[tracks / 2];
    let view = lib.track(probe).expect("track");
    let first_lookup = t.elapsed();
    assert_eq!(view.hot_cues.iter().flatten().count(), 8);

    let t = Instant::now();
    let snap = lib.snapshot()?;
    let snapshot_write = t.elapsed();
    let snap_bytes = std::fs::metadata(root.join(&snap))?.len();
    drop(lib);

    // Remove the covered segments, as compaction will: mount reads the snapshot alone.
    let store = FolderStore::new(&root);
    for d in store.list("journal")? {
        for s in store.list(&format!("journal/{d}"))? {
            store.remove(&format!("journal/{d}/{s}"))?;
        }
    }
    let t = Instant::now();
    let lib = Library::mount(FolderStore::new(&root), MountOptions::new(devices[1]))?;
    let mount_snapshot = t.elapsed();
    assert_eq!(lib.tracks().len(), tracks);

    println!(
        "{tracks} tracks, {ops} ops in {segments} segments ({:.1} MB of journal), built in {:.1} s",
        journal_bytes as f64 / 1e6,
        built.as_secs_f64()
    );
    println!(
        "mount + replay from journals: {:.0} ms",
        mount_journal.as_secs_f64() * 1e3
    );
    println!(
        "first track lookup (builds the index): {:.1} ms",
        first_lookup.as_secs_f64() * 1e3
    );
    println!(
        "snapshot write: {:.0} ms ({:.1} MB)",
        snapshot_write.as_secs_f64() * 1e3,
        snap_bytes as f64 / 1e6
    );
    println!(
        "mount from the snapshot alone: {:.0} ms",
        mount_snapshot.as_secs_f64() * 1e3
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
