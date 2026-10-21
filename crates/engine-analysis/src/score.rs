//! Scoring analysis results against a reference ("ground truth").
//!
//! - **Tempo:** exact (±0.5 BPM, what a DJ needs), close (±2 BPM, the figure stratum-dsp
//!   reports), half/double tempo, 3:2 errors, or wrong.
//! - **Beats:** F-measure with a ±70 ms window (the usual beat-tracking tolerance), plus the
//!   median signed offset of matched beats. A grid that is consistently 20 ms late still
//!   scores F = 1, but it is unusable for beatmatching; the offset shows it.
//! - **Downbeats:** the same F-measure on bar position 1 only, for sources that know the bars.
//! - **Key:** the MIREX weighting: correct 1.0, perfect fifth 0.5, relative 0.3, parallel 0.2.

use std::fmt::Write as _;

use crate::model::{Analysis, Beatgrid, Key, Mode};

/// Reference values for one track. Missing values are not scored.
#[derive(Debug, Clone, Default)]
pub struct Truth {
    pub tempo: Option<f64>,
    pub beatgrid: Option<Beatgrid>,
    pub key: Option<Key>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TempoVerdict {
    Exact,
    Close,
    /// Half or double the reference.
    Octave,
    /// 2/3 or 3/2 of the reference.
    Triplet,
    Wrong,
}

pub fn score_tempo(estimate: f64, reference: f64) -> TempoVerdict {
    let d = (estimate - reference).abs();
    if d <= 0.5 {
        return TempoVerdict::Exact;
    }
    if d <= 2.0 {
        return TempoVerdict::Close;
    }
    let near = |factor: f64| (estimate - reference * factor).abs() <= reference * factor * 0.02;
    if near(2.0) || near(0.5) {
        TempoVerdict::Octave
    } else if near(1.5) || near(2.0 / 3.0) {
        TempoVerdict::Triplet
    } else {
        TempoVerdict::Wrong
    }
}

/// Beat-tracking match against a reference grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatScore {
    pub precision: f64,
    pub recall: f64,
    pub f_measure: f64,
    /// Median of (estimate - reference) over matched beats, in milliseconds.
    pub median_offset_ms: Option<f64>,
}

pub const BEAT_WINDOW: f64 = 0.070;

/// Match beat times one-to-one within `window` seconds (both lists sorted).
pub fn score_beat_times(estimate: &[f64], reference: &[f64], window: f64) -> BeatScore {
    if estimate.is_empty() || reference.is_empty() {
        return BeatScore {
            precision: 0.0,
            recall: 0.0,
            f_measure: 0.0,
            median_offset_ms: None,
        };
    }
    let mut used = vec![false; estimate.len()];
    let mut offsets = Vec::new();
    let mut start = 0;
    for &r in reference {
        while start < estimate.len() && estimate[start] < r - window {
            start += 1;
        }
        let best = (start..estimate.len())
            .take_while(|&i| estimate[i] <= r + window)
            .filter(|&i| !used[i])
            .min_by(|&a, &b| (estimate[a] - r).abs().total_cmp(&(estimate[b] - r).abs()));
        if let Some(i) = best {
            used[i] = true;
            offsets.push(estimate[i] - r);
        }
    }
    let hits = offsets.len() as f64;
    let precision = hits / estimate.len() as f64;
    let recall = hits / reference.len() as f64;
    let f_measure = if hits > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    offsets.sort_by(f64::total_cmp);
    BeatScore {
        precision,
        recall,
        f_measure,
        median_offset_ms: offsets.get(offsets.len() / 2).map(|o| o * 1000.0),
    }
}

pub fn score_beats(estimate: &Beatgrid, reference: &Beatgrid) -> BeatScore {
    let times = |g: &Beatgrid| g.beats.iter().map(|b| b.time).collect::<Vec<_>>();
    score_beat_times(&times(estimate), &times(reference), BEAT_WINDOW)
}

pub fn score_downbeats(estimate: &Beatgrid, reference: &Beatgrid) -> BeatScore {
    let downs = |g: &Beatgrid| {
        g.beats
            .iter()
            .filter(|b| b.bar_pos == 1)
            .map(|b| b.time)
            .collect::<Vec<_>>()
    };
    score_beat_times(&downs(estimate), &downs(reference), BEAT_WINDOW)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyVerdict {
    Correct,
    Fifth,
    Relative,
    Parallel,
    Other,
}

impl KeyVerdict {
    /// MIREX key-detection weight.
    pub fn weight(self) -> f64 {
        match self {
            KeyVerdict::Correct => 1.0,
            KeyVerdict::Fifth => 0.5,
            KeyVerdict::Relative => 0.3,
            KeyVerdict::Parallel => 0.2,
            KeyVerdict::Other => 0.0,
        }
    }
}

pub fn score_key(estimate: Key, reference: Key) -> KeyVerdict {
    if estimate == reference {
        return KeyVerdict::Correct;
    }
    let same_mode = estimate.mode == reference.mode;
    if same_mode
        && (estimate.tonic == reference.tonic.up(7) || estimate.tonic == reference.tonic.up(5))
    {
        return KeyVerdict::Fifth;
    }
    let relative = match (reference.mode, estimate.mode) {
        (Mode::Major, Mode::Minor) => estimate.tonic == reference.tonic.up(9),
        (Mode::Minor, Mode::Major) => estimate.tonic == reference.tonic.up(3),
        _ => false,
    };
    if relative {
        KeyVerdict::Relative
    } else if !same_mode && estimate.tonic == reference.tonic {
        KeyVerdict::Parallel
    } else {
        KeyVerdict::Other
    }
}

/// Scores for one track; `None` where either side is missing.
#[derive(Debug, Clone)]
pub struct TrackScore {
    pub name: String,
    pub tempo: Option<(f64, f64, TempoVerdict)>,
    pub beats: Option<BeatScore>,
    pub downbeats: Option<BeatScore>,
    pub key: Option<(Key, Key, KeyVerdict)>,
}

pub fn score_track(name: &str, analysis: &Analysis, truth: &Truth) -> TrackScore {
    let tempo = analysis
        .tempo
        .as_ref()
        .zip(truth.tempo)
        .map(|(e, r)| (e.value.bpm, r, score_tempo(e.value.bpm, r)));
    let grids = analysis
        .beatgrid
        .as_ref()
        .zip(truth.beatgrid.as_ref())
        .map(|(e, r)| (&e.value, r));
    let key = analysis
        .key
        .as_ref()
        .zip(truth.key)
        .map(|(e, r)| (e.value, r, score_key(e.value, r)));
    TrackScore {
        name: name.into(),
        tempo,
        beats: grids.map(|(e, r)| score_beats(e, r)),
        downbeats: grids
            .filter(|(e, r)| e.has_bars() && r.has_bars())
            .map(|(e, r)| score_downbeats(e, r)),
        key,
    }
}

/// A Markdown report: one row per track, then a summary.
pub fn report(analyzer: &str, scores: &[TrackScore]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Analysis score: {analyzer}\n");
    let _ = writeln!(
        s,
        "| track | BPM (ref) | tempo | beat F | offset ms | downbeat F | key (ref) | key |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    let dash = || "—".to_string();
    for t in scores {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            t.name,
            t.tempo
                .map_or_else(dash, |(e, r, _)| format!("{e:.2} ({r:.2})")),
            t.tempo.map_or_else(dash, |(_, _, v)| format!("{v:?}")),
            t.beats.map_or_else(dash, |b| format!("{:.3}", b.f_measure)),
            t.beats
                .and_then(|b| b.median_offset_ms)
                .map_or_else(dash, |o| format!("{o:+.1}")),
            t.downbeats
                .map_or_else(dash, |b| format!("{:.3}", b.f_measure)),
            t.key.map_or_else(dash, |(e, r, _)| format!("{e} ({r})")),
            t.key.map_or_else(dash, |(_, _, v)| format!("{v:?}")),
        );
    }

    let tempos: Vec<TempoVerdict> = scores.iter().filter_map(|t| t.tempo.map(|x| x.2)).collect();
    let pct = |n: usize, of: usize| {
        if of == 0 {
            "—".to_string()
        } else {
            format!("{:.1} % ({n}/{of})", 100.0 * n as f64 / of as f64)
        }
    };
    let count = |v: TempoVerdict| tempos.iter().filter(|&&t| t == v).count();
    let _ = writeln!(s, "\n## Summary\n");
    let _ = writeln!(
        s,
        "- Tempo within ±0.5 BPM: {}",
        pct(count(TempoVerdict::Exact), tempos.len())
    );
    let _ = writeln!(
        s,
        "- Tempo within ±2 BPM: {}",
        pct(
            count(TempoVerdict::Exact) + count(TempoVerdict::Close),
            tempos.len()
        )
    );
    let _ = writeln!(
        s,
        "- Half/double tempo: {}; 3:2: {}; wrong: {}",
        pct(count(TempoVerdict::Octave), tempos.len()),
        pct(count(TempoVerdict::Triplet), tempos.len()),
        pct(count(TempoVerdict::Wrong), tempos.len())
    );
    let beats: Vec<BeatScore> = scores.iter().filter_map(|t| t.beats).collect();
    if !beats.is_empty() {
        let mean_f = beats.iter().map(|b| b.f_measure).sum::<f64>() / beats.len() as f64;
        let mut offsets: Vec<f64> = beats.iter().filter_map(|b| b.median_offset_ms).collect();
        offsets.sort_by(f64::total_cmp);
        let _ = writeln!(
            s,
            "- Beat F-measure (±70 ms): mean {mean_f:.3} over {} tracks; median offset {}",
            beats.len(),
            offsets
                .get(offsets.len() / 2)
                .map_or_else(dash, |o| format!("{o:+.1} ms"))
        );
    }
    let downs: Vec<BeatScore> = scores.iter().filter_map(|t| t.downbeats).collect();
    if !downs.is_empty() {
        let mean_f = downs.iter().map(|b| b.f_measure).sum::<f64>() / downs.len() as f64;
        let _ = writeln!(s, "- Downbeat F-measure: mean {mean_f:.3}");
    }
    let keys: Vec<KeyVerdict> = scores.iter().filter_map(|t| t.key.map(|k| k.2)).collect();
    if !keys.is_empty() {
        let weighted = keys.iter().map(|k| k.weight()).sum::<f64>() / keys.len() as f64;
        let correct = keys.iter().filter(|&&k| k == KeyVerdict::Correct).count();
        let _ = writeln!(
            s,
            "- Key: correct {}; MIREX weighted score {weighted:.3}",
            pct(correct, keys.len())
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempo_verdicts() {
        assert_eq!(score_tempo(124.3, 124.0), TempoVerdict::Exact);
        assert_eq!(score_tempo(125.5, 124.0), TempoVerdict::Close);
        assert_eq!(score_tempo(62.0, 124.0), TempoVerdict::Octave);
        assert_eq!(score_tempo(174.0, 87.0), TempoVerdict::Octave);
        assert_eq!(score_tempo(186.0, 124.0), TempoVerdict::Triplet);
        assert_eq!(score_tempo(140.0, 124.0), TempoVerdict::Wrong);
    }

    #[test]
    fn beat_f_measure_and_offset() {
        let reference = Beatgrid::constant(1.0, 120.0, 61.0);
        let late = Beatgrid {
            beats: reference
                .beats
                .iter()
                .map(|b| crate::Beat {
                    time: b.time + 0.020,
                    ..*b
                })
                .collect(),
        };
        let s = score_beats(&late, &reference);
        assert_eq!(s.f_measure, 1.0);
        assert!((s.median_offset_ms.unwrap() - 20.0).abs() < 1e-6);

        // Half tempo: every other beat matches, so precision 1, recall 0.5.
        let half = Beatgrid::constant(1.0, 60.0, 61.0);
        let s = score_beats(&half, &reference);
        assert!((s.precision - 1.0).abs() < 1e-9 && (s.recall - 0.5).abs() < 0.01);

        // Off by half a beat: nothing within 70 ms.
        let off = Beatgrid::constant(1.25, 120.0, 61.0);
        assert_eq!(score_beats(&off, &reference).f_measure, 0.0);
        assert_eq!(score_beats(&Beatgrid::default(), &reference).f_measure, 0.0);
    }

    #[test]
    fn downbeats_score_bar_positions() {
        let reference = Beatgrid::constant(0.0, 120.0, 32.0);
        let mut shifted = reference.clone();
        for b in &mut shifted.beats {
            b.bar_pos = b.bar_pos % 4 + 1; // bar starts one beat late
        }
        assert_eq!(score_beats(&shifted, &reference).f_measure, 1.0);
        assert_eq!(score_downbeats(&shifted, &reference).f_measure, 0.0);
    }

    #[test]
    fn key_verdicts() {
        let c = Key::major(0);
        assert_eq!(score_key(c, c), KeyVerdict::Correct);
        assert_eq!(score_key(Key::major(7), c), KeyVerdict::Fifth);
        assert_eq!(score_key(Key::major(5), c), KeyVerdict::Fifth);
        assert_eq!(score_key(Key::minor(9), c), KeyVerdict::Relative);
        assert_eq!(
            score_key(Key::major(0), Key::minor(9)),
            KeyVerdict::Relative
        );
        assert_eq!(score_key(Key::minor(0), c), KeyVerdict::Parallel);
        assert_eq!(score_key(Key::major(1), c), KeyVerdict::Other);
    }
}
