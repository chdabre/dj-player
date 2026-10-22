//! The Markdown report over a set of scored tracks.

use std::fmt::Write as _;

use super::{BeatScore, KeyVerdict, TempoVerdict, TrackScore};

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
    summary(&mut s, scores);
    s
}

/// The summary under the rows: tempo verdict shares, mean beat and downbeat F, key score.
fn summary(s: &mut String, scores: &[TrackScore]) {
    let dash = || "—".to_string();
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
}
