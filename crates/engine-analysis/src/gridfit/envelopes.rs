//! Onset envelopes from the spectral flux, and the time each envelope frame stands for.

use super::{BANDS, FRAME, LOW_WEIGHT, ONSET_LATENCY, RATE};

/// Time (seconds) of the onset that envelope frame `k` responds to most: half a hop after
/// the centre of frame `k`. This was measured, not derived: on synthetic kicks with exact
/// onset times it puts the grid within ±2 ms (see the gridfit test).
pub(super) fn frame_time(k: f64, fps: f64) -> f64 {
    (k + 0.5) / fps + (FRAME as f64 / 2.0) / f64::from(RATE) + ONSET_LATENCY
}

/// Normalised flux (low band weighted up) minus a ~0.4 s moving average, floored at 0.
pub(super) fn onset_envelope(full: &[f32], low: &[f32], fps: f64) -> Vec<f32> {
    let norm = |v: &[f32]| {
        let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
        let m = if mean > 0.0 { mean } else { 1.0 };
        v.iter().map(|x| x / m).collect::<Vec<f32>>()
    };
    let (full, low) = (norm(full), norm(low));
    let raw: Vec<f32> = full
        .iter()
        .zip(&low)
        .map(|(f, l)| f + LOW_WEIGHT * l)
        .collect();
    let half = (0.2 * fps) as usize;
    let mut prefix = vec![0.0f64; raw.len() + 1];
    for (i, x) in raw.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*x);
    }
    (0..raw.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(raw.len()));
            let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
            (raw[i] - mean as f32).max(0.0)
        })
        .collect()
}
/// Bands `0..max_band` that set the phase (edges up to ~4.5 kHz at 44.1 kHz).
pub(super) const PHASE_BANDS: usize = 18;

/// Equal-weight sum of the per-band onset envelopes of bands `0..max_band`.
pub(super) fn band_envelope(bands: &[[f32; BANDS]], max_band: usize, fps: f64) -> Vec<f32> {
    let mut sum = vec![0.0f32; bands.len()];
    for b in 0..max_band {
        let flux: Vec<f32> = (0..bands.len())
            .map(|k| {
                if k == 0 {
                    0.0
                } else {
                    (bands[k][b] - bands[k - 1][b]).max(0.0)
                }
            })
            .collect();
        for (s, e) in sum.iter_mut().zip(onset_envelope(&flux, &flux, fps)) {
            *s += e;
        }
    }
    sum
}

/// Kick-like onsets: the geometric mean of the low-band and high-band envelopes, so only
/// moments with both a body and a click stand out.
pub(super) fn kick_envelope(low: &[f32], high: &[f32], fps: f64) -> Vec<f32> {
    let l = onset_envelope(low, low, fps);
    let h = onset_envelope(high, high, fps);
    l.iter().zip(&h).map(|(a, b)| (a * b).sqrt()).collect()
}
