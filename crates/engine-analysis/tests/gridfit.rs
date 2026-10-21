//! gridfit on synthetic tracks with a known grid.

use engine_analysis::score::{TempoVerdict, score_beats, score_downbeats, score_tempo};
use engine_analysis::{
    Analyzer, Beatgrid, CancelFlag, FeatureSet, GridFit, GridFitConfig, TrackAudio,
};

/// Kick on every beat, hat on the off-beats, and a bass note that changes every bar (so
/// the bars are audible), with `intro` seconds of silence first. The hat is high-passed
/// noise (second difference, +12 dB/octave) like a real hat, whose energy sits mostly above
/// ~6 kHz; plain white noise would put as much energy in the kick's bands.
fn track(sr: u32, bpm: f64, intro: f64, seconds: f64) -> TrackAudio {
    let srf = f64::from(sr);
    let beat = 60.0 / bpm;
    let mut noise = 0x2545_f491u32;
    let (mut n1, mut n2) = (0.0f64, 0.0f64);
    let notes = [55.0, 73.4, 65.4, 49.0];
    let stereo = (0..(seconds * srf) as usize)
        .flat_map(|i| {
            let t = i as f64 / srf;
            let x = if t < intro {
                0.0
            } else {
                let tt = t - intro;
                let tb = tt % beat;
                let kick = (-tb * 25.0).exp() * (std::f64::consts::TAU * 50.0 * tb).sin();
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let to = (tt + beat / 2.0) % beat;
                let white = f64::from(noise) / f64::from(u32::MAX) - 0.5;
                let bright = 0.25 * (white - 2.0 * n1 + n2);
                (n2, n1) = (n1, white);
                let hat = (-to * 150.0).exp() * bright;
                let bar = (tt / (4.0 * beat)) as usize;
                let bass = 0.2 * (std::f64::consts::TAU * notes[bar % 4] * 2.0 * tt).sin();
                0.6 * kick + 0.25 * hat + bass
            } as f32;
            [x, x]
        })
        .collect();
    TrackAudio {
        sample_rate: sr,
        stereo,
    }
}

#[test]
fn fits_tempo_phase_and_bars_across_tempos_and_rates() {
    let seconds = 60.0;
    let default = GridFitConfig::default();
    // With a kick on every beat, 165 and 174 fit half that tempo just as well, and gridfit
    // alone picks the half. Like Rekordbox, a BPM range setting decides here; in the
    // analysis package the tempo hint (TempoCNN) will. Those cases run with a fast range.
    let fast = GridFitConfig {
        min_bpm: 100.0,
        ..default
    };
    for (sr, bpm, intro, config) in [
        (44_100, 124.0, 3.0, default),
        (44_100, 128.0, 0.0, default),
        (48_000, 100.0, 1.37, default),
        (48_000, 140.0, 0.5, default),
        (44_100, 90.0, 0.2, default),
        (44_100, 165.0, 1.0, fast),
        (96_000, 174.0, 2.0, fast),
        (44_100, 123.456, 0.731, default),
    ] {
        let a = GridFit::new(config)
            .analyze(
                &track(sr, bpm, intro, seconds),
                FeatureSet::ALL,
                &[],
                &CancelFlag::new(),
            )
            .expect("analysis");
        let tempo = a.tempo.expect("tempo").value.bpm;
        let grid = a.beatgrid.expect("grid").value;
        let reference = Beatgrid::constant(intro, bpm, seconds);
        let beats = score_beats(&grid, &reference);
        let downs = score_downbeats(&grid, &reference);
        eprintln!(
            "{sr} Hz {bpm} BPM: {tempo:.3} BPM, beat F {:.3}, offset {:+.2} ms, downbeat F {:.3}",
            beats.f_measure,
            beats.median_offset_ms.unwrap_or(f64::NAN),
            downs.f_measure
        );
        assert_eq!(
            score_tempo(tempo, bpm),
            TempoVerdict::Exact,
            "{sr} {bpm}: {tempo}"
        );
        assert!((tempo - bpm).abs() < 0.02, "{sr} {bpm}: {tempo}");
        assert!(beats.f_measure > 0.97, "{sr} {bpm}: F {}", beats.f_measure);
        assert!(beats.median_offset_ms.unwrap().abs() < 5.0, "{sr} {bpm}");
        assert!(
            downs.f_measure > 0.9,
            "{sr} {bpm}: downbeat F {}",
            downs.f_measure
        );
    }
}

#[test]
fn a_tempo_hint_decides_the_octave() {
    use engine_analysis::{Analysed, Analysis, AnalyzerId, Provenance, Tempo};
    // 165 with a kick on every beat: alone, gridfit picks 82.5 (see above).
    let audio = track(44_100, 165.0, 1.0, 60.0);
    let hint = |name: &str, provenance: Provenance| Analysis {
        tempo: Some(Analysed {
            value: Tempo { bpm: 165.0 },
            confidence: None,
            provenance: if name.is_empty() {
                provenance
            } else {
                Provenance::Analyzer(AnalyzerId {
                    name: name.into(),
                    version: "test".into(),
                    params: String::new(),
                })
            },
        }),
        ..Analysis::default()
    };
    let mut g = GridFit::hinted("tempocnn");
    let a = g
        .analyze(
            &audio,
            FeatureSet::ALL,
            &[hint("tempocnn", Provenance::Manual)],
            &CancelFlag::new(),
        )
        .unwrap();
    let tempo = a.tempo.unwrap();
    assert!(
        (tempo.value.bpm - 165.0).abs() < 0.02,
        "{}",
        tempo.value.bpm
    );
    let Provenance::Analyzer(id) = tempo.provenance else {
        panic!()
    };
    assert!(id.params.contains("hint=tempocnn test"), "{}", id.params);
    // Another analyzer's tempo, or reference data, is not a hint.
    for other in [
        hint("someone-else", Provenance::Manual),
        hint("", Provenance::Rekordbox),
    ] {
        let a = g
            .analyze(&audio, FeatureSet::ALL, &[other], &CancelFlag::new())
            .unwrap();
        assert!((a.tempo.unwrap().value.bpm - 82.5).abs() < 0.02);
    }
}
