//! stratum-dsp on a synthetic track with a known grid, end to end through the scorer.

#![cfg(feature = "stratum")]

use engine_analysis::score::{TempoVerdict, score_beats, score_tempo};
use engine_analysis::{Analyzer, Beatgrid, CancelFlag, FeatureSet, Stratum, TrackAudio};

const SR: u32 = 44_100;

/// Kick on every beat, hat on the off-beats, a quiet pad; silence before `intro` seconds.
fn track(bpm: f64, intro: f64, seconds: f64) -> TrackAudio {
    let sr = f64::from(SR);
    let beat = 60.0 / bpm;
    let mut noise = 0x2545_f491u32;
    let stereo = (0..(seconds * sr) as usize)
        .flat_map(|i| {
            let t = i as f64 / sr;
            let x = if t < intro {
                0.0
            } else {
                let tb = (t - intro) % beat;
                let kick = (-tb * 25.0).exp() * (std::f64::consts::TAU * 50.0 * tb).sin();
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let to = (t - intro + beat / 2.0) % beat;
                let hat = (-to * 150.0).exp() * (f64::from(noise) / f64::from(u32::MAX) - 0.5);
                let pad = 0.1 * (std::f64::consts::TAU * 220.0 * t).sin();
                0.6 * kick + 0.25 * hat + pad
            } as f32;
            [x, x]
        })
        .collect();
    TrackAudio {
        sample_rate: SR,
        stereo,
    }
}

#[test]
fn finds_tempo_and_grid_of_a_track_with_a_silent_intro() {
    let (bpm, intro, seconds) = (124.0, 3.0, 40.0);
    let audio = track(bpm, intro, seconds);
    let a = Stratum::new()
        .analyze(&audio, FeatureSet::ALL, &[], &CancelFlag::new())
        .expect("analysis");

    let tempo = a.tempo.expect("tempo").value.bpm;
    assert_eq!(score_tempo(tempo, bpm), TempoVerdict::Exact, "{tempo} BPM");

    let grid = a.beatgrid.expect("grid").value;
    let reference = Beatgrid::constant(intro, bpm, seconds);
    let s = score_beats(&grid, &reference);
    eprintln!(
        "beat F {:.3}, median offset {:?} ms",
        s.f_measure, s.median_offset_ms
    );
    assert!(s.f_measure > 0.9, "beat F {}", s.f_measure);
    // With stratum-dsp's own silence trimming the grid would be 3 s early. What is left is
    // stratum-dsp 1.0.0's own bias: on this signal every beat is about 32 ms early (also at
    // 140 and 174 BPM, and at 48/96 kHz). Not corrected until real tracks with Rekordbox
    // grids show whether it is constant; see docs/plan/analysis.md.
    let offset = s.median_offset_ms.unwrap();
    assert!((-40.0..=0.0).contains(&offset), "offset {offset} ms");
    assert!(
        grid.beats[0].time > intro - 0.1,
        "no beats in the silent intro"
    );
}

#[test]
fn cancelled_jobs_stop() {
    let flag = CancelFlag::new();
    flag.cancel();
    let audio = track(120.0, 0.0, 5.0);
    assert!(matches!(
        Stratum::new().analyze(&audio, FeatureSet::ALL, &[], &flag),
        Err(engine_analysis::AnalysisError::Cancelled)
    ));
}
