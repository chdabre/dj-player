//! The player loading one track after another at run time, through the real engine and
//! library: hot cues set on a track come back when it is loaded again. Needs an audio output
//! device, so it is ignored by default: `cargo test -p engined --test player -- --ignored`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use engine_core::DeckCommand;
use engine_io::{CpalBackend, StreamRequest};
use engined::{AutoCue, LibraryChoice, Options, Player};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("engined-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Three seconds of a 16-bit stereo tone.
fn write_wav(path: &Path, freq: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..3 * 44_100 {
        let x = (std::f32::consts::TAU * freq * i as f32 / 44_100.0).sin();
        let s = (x * 12_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

/// Poll until `done` holds (or fail after 10 s).
fn poll_until(p: &mut Player, what: &str, mut done: impl FnMut(&Player) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !done(p) {
        for line in p.poll() {
            eprintln!("{line}");
        }
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "needs an audio output device"]
fn hot_cues_follow_their_track_across_loads() {
    let dir = temp_dir("player");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, 440.0);
    write_wav(&b, 660.0);
    let options = Options {
        library: LibraryChoice::Folder(dir.join("DJPLAYER")),
        auto_cue: AutoCue::Off,
        ..Options::default()
    };
    let mut p = Player::start(&CpalBackend::new(), &StreamRequest::default(), options).unwrap();
    let sr = f64::from(p.format.sample_rate);
    let hot0 = |p: &Player| p.state().hot_cue_positions[0].load();

    p.load(&a).unwrap();
    poll_until(&mut p, "a's record", |p| p.track().is_some());
    let track_a = p.track().unwrap();
    p.send(DeckCommand::Seek(1.0 * sr));
    p.send(DeckCommand::HotCue(0));
    poll_until(&mut p, "hot cue A on a", |p| !hot0(p).is_nan());

    p.load(&b).unwrap();
    poll_until(&mut p, "b's record", |p| {
        p.track().is_some_and(|t| t != track_a) && hot0(p).is_nan()
    });
    assert_eq!(p.loaded().unwrap().file(), b.as_path());

    p.load(&a).unwrap();
    poll_until(&mut p, "a again, with its hot cue", |p| {
        p.track() == Some(track_a) && !hot0(p).is_nan()
    });
    assert!(
        (hot0(&p) / sr - 1.0).abs() < 0.01,
        "hot cue A at {}",
        hot0(&p) / sr
    );
    assert!(!p.state().playing.load(Ordering::Relaxed));
}

#[test]
#[ignore = "needs an audio output device"]
fn a_track_plays_and_analyses_while_it_arrives_and_is_recorded_once_complete() {
    use std::io::Write;
    let dir = temp_dir("arriving");
    let src = dir.join("src.wav");
    write_wav(&src, 440.0);
    let bytes = std::fs::read(&src).unwrap();
    let downloads = dir.join("SoulSeek Downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let (file, part) = (downloads.join("a.wav"), downloads.join("a.wav.part"));
    let mut out = std::fs::File::create(&part).unwrap();
    out.write_all(&bytes[..256 * 1024]).unwrap();
    out.flush().unwrap();

    let options = Options {
        library: LibraryChoice::Folder(dir.join("DJPLAYER")),
        auto_cue: AutoCue::Off,
        ..Options::default()
    };
    let mut p = Player::start(&CpalBackend::new(), &StreamRequest::default(), options).unwrap();
    // Not enough to open: the load says so and nothing waits.
    let early = dir.join("early.wav");
    std::fs::write(dir.join("early.wav.part"), &bytes[..8]).unwrap();
    assert!(
        p.load_arriving(&early, &dir.join("early.wav.part"), bytes.len() as u64)
            .is_err()
    );

    p.load_arriving(&file, &part, bytes.len() as u64).unwrap();
    assert_eq!(p.loaded().unwrap().file(), file.as_path());
    // Playable from the first chunk; the rest follows as it arrives.
    poll_until(&mut p, "the first chunk", |p| {
        p.loaded().unwrap().source.chunks_ready() > 0
    });
    assert!(
        !p.loaded().unwrap().source.is_complete(),
        "only what arrived"
    );
    out.write_all(&bytes[256 * 1024..]).unwrap();
    out.flush().unwrap();
    poll_until(&mut p, "the analysis, before the download ended", |p| {
        p.loaded().unwrap().analysed.is_some()
    });
    assert!(p.track().is_none(), "no record while it arrives");

    std::fs::rename(&part, &file).unwrap();
    assert_eq!(p.arrived(&file, true), None);
    poll_until(&mut p, "the record, with the analysis stored", |p| {
        p.track().is_some()
            && p.listing()
                .and_then(|l| l.get("SoulSeek Downloads/a.wav"))
                .is_some_and(|l| l.analysed)
    });
}
