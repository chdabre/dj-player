//! Open time, size and edit cost of a large synthetic library.
//!
//! `cargo run --release -p library --example scale [tracks] [dir]`: builds a library of
//! `tracks` (default 10 000) tracks in `dir` (default a temporary folder), each with a file
//! identity and its audio hash, 8 hot cues, 3 memory cues, a rating, 3 analysis results and
//! a waveform, written in sessions of 500 tracks. Then times opening it, reading one track
//! and committing a batch of 20 edits.

use std::path::PathBuf;
use std::time::Instant;

use library::{
    AnalysisRef, AudioHash, Cue, DeviceId, Edit, FileHash, FileIdentity, HotCue, Library,
    MountOptions, WaveformRef,
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
    let opts = MountOptions::new(DeviceId::random());

    let t0 = Instant::now();
    let mut lib = Library::mount(&root, opts.clone())?;
    for i in 0..tracks {
        let mut fh = [0u8; 32];
        fh[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let id = lib
            .find_or_create(FileIdentity::new(
                format!("Music/Artist {}/Track {i}.mp3", i % 300),
                8_000_000 + i as u64,
                FileHash(fh),
            ))?
            .track;
        let mut ah = fh;
        ah[31] = 0xa1;
        lib.set_audio_hash(id, AudioHash { v: 1, hash: ah })?;
        for slot in 0..8 {
            lib.apply(Edit::SetHotCue(
                id,
                slot,
                Some(HotCue::at(slot as f64 * 15.0)),
            ))?;
        }
        for k in 0..3 {
            lib.apply(Edit::AddCue(id, Cue::memory(k as f64 * 32.0 + 0.5)))?;
        }
        lib.apply(Edit::SetRating(id, Some((i % 6) as u8)))?;
        for name in ["tempocnn", "gridfit", "hpcpkey"] {
            let blob = lib.put_blob(format!("{name} {i}").as_bytes());
            let a = AnalysisRef {
                version: "0.1.0".into(),
                params: "default".into(),
                blob,
                created: "2026-10-21T12:00:00Z".into(),
            };
            lib.apply(Edit::SetAnalysis(id, name.into(), a))?;
        }
        let wf = lib.put_blob(format!("waveform {i}").as_bytes());
        let ov = lib.put_blob(format!("overview {i}").as_bytes());
        lib.apply(Edit::SetWaveform(id, WaveformRef::new(wf, ov)))?;
        if i % 500 == 499 {
            // A session ends: the next mount opens the file again.
            drop(lib);
            lib = Library::mount(&root, opts.clone())?;
        }
    }
    drop(lib);
    let built = t0.elapsed();
    let db = std::fs::metadata(root.join("library.db"))?.len();
    println!(
        "built {tracks} tracks in {:.1} s; library.db {:.1} MB, folder {:.1} MB",
        built.as_secs_f64(),
        db as f64 / 1e6,
        dir_size(&root) as f64 / 1e6
    );

    let t = Instant::now();
    let mut lib = Library::mount(&root, opts)?;
    let open = t.elapsed();
    let ids = lib.tracks()?;
    assert_eq!(ids.len(), tracks);
    let probe = ids[tracks / 2];
    let t = Instant::now();
    let view = lib.track(probe)?.expect("a track");
    let read = t.elapsed();
    assert_eq!(view.hot_cues.iter().flatten().count(), 8);
    let t = Instant::now();
    for k in 0..20 {
        lib.apply(Edit::SetHotCue(
            probe,
            8 + k % 8,
            Some(HotCue::at(k as f64)),
        ))?;
    }
    lib.flush();
    let batch = t.elapsed();
    // The first write of a mount also makes the backup: that is in this time.
    println!(
        "open {:.1} ms, one track {:.2} ms, 20 edits and the backup committed {:.1} ms",
        open.as_secs_f64() * 1e3,
        read.as_secs_f64() * 1e3,
        batch.as_secs_f64() * 1e3,
    );
    let t = Instant::now();
    for k in 0..20 {
        lib.apply(Edit::SetHotCue(probe, k % 8, Some(HotCue::at(k as f64))))?;
    }
    lib.flush();
    println!(
        "20 more edits committed {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    drop(lib);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
