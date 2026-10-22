//! Fitting the grid: coarse tempo from autocorrelation, fine tempo and phase from folding,
//! then the downbeat.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;

use super::envelopes::frame_time;
use super::{BANDS, GridFitConfig};
use crate::model::Beat;

/// Autocorrelation of `env`, normalised so lag 0 is 1; `None` if `env` is all zero.
fn autocorrelation(env: &[f32]) -> Option<Vec<f64>> {
    let n = env.len();
    let size = (2 * n).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let (fwd, inv) = (
        planner.plan_fft_forward(size),
        planner.plan_fft_inverse(size),
    );
    let mut buf: Vec<Complex32> = env
        .iter()
        .map(|&x| Complex32::new(x, 0.0))
        .chain(std::iter::repeat(Complex32::default()))
        .take(size)
        .collect();
    fwd.process(&mut buf);
    for x in &mut buf {
        *x = Complex32::new(x.norm_sqr(), 0.0);
    }
    inv.process(&mut buf);
    let zero = f64::from(buf[0].re);
    (zero > 0.0).then(|| buf[..n].iter().map(|x| f64::from(x.re) / zero).collect())
}

/// Beat period (seconds) from the summed beat salience of several envelopes' autocorrelations,
/// or `None` for a track with no pulse. Each sees something the others miss: the low band
/// (kicks) keeps 3:2 periods out, since kicks only line up at whole beats; the linear-bin
/// flux hears the off-beat hats, which tell 165 BPM from 82.5 when every beat has a kick.
pub(super) fn coarse_period(envs: &[&[f32]], fps: f64, c: &GridFitConfig) -> Option<f64> {
    let acs: Vec<Vec<f64>> = envs.iter().filter_map(|e| autocorrelation(e)).collect();
    if acs.is_empty() {
        return None;
    }
    let mut best: Option<(f64, f64)> = None;
    let mut bpm = c.min_bpm;
    while bpm < c.max_bpm {
        let lag = 60.0 / bpm * fps;
        let salience: f64 = acs.iter().map(|ac| beat_salience(ac, lag)).sum();
        let octaves = (bpm / c.prior_bpm).log2() / c.prior_octaves;
        let score = salience * (-0.5 * octaves * octaves).exp();
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, 60.0 / bpm));
        }
        bpm += 0.05;
    }
    best.map(|(_, p)| p)
}

/// How strongly `ac` supports a beat at `lag` frames.
fn beat_salience(ac: &[f64], lag: f64) -> f64 {
    // Linear interpolation of the autocorrelation at a fractional lag.
    let at = |lag: f64| -> f64 {
        let i = lag.floor() as usize;
        if i + 1 >= ac.len() {
            return 0.0;
        }
        let t = lag - i as f64;
        ac[i] * (1.0 - t) + ac[i + 1] * t
    };
    // The beat lag plus its multiples: a true beat period also repeats at 2 and 4 beats,
    // and usually has something on the off-beat.
    at(lag) + 0.5 * at(2.0 * lag) + 0.25 * at(4.0 * lag) + 0.5 * at(0.5 * lag)
}

pub(super) struct Fit {
    pub(super) period: f64,
    pub(super) phase: f64,
    pub(super) confidence: f32,
}

/// Fold the envelope at `period` into `bins` phase bins. Returns the circularly smoothed
/// histogram.
fn fold(env: &[f32], fps: f64, period: f64, bins: usize) -> Vec<f64> {
    let mut h = vec![0.0f64; bins];
    for (k, &x) in env.iter().enumerate() {
        if x <= 0.0 {
            continue;
        }
        let t = frame_time(k as f64, fps);
        let pos = (t / period).rem_euclid(1.0) * bins as f64;
        let i = pos.floor() as usize % bins;
        let f = pos - pos.floor();
        h[i] += f64::from(x) * (1.0 - f);
        h[(i + 1) % bins] += f64::from(x) * f;
    }
    // Smooth over ±2 bins (about ±2 ms): onsets are not sample-exact.
    let k = [1.0, 2.0, 3.0, 2.0, 1.0];
    (0..bins)
        .map(|i| {
            (0..5)
                .map(|j| k[j] * h[(i + bins + j - 2) % bins])
                .sum::<f64>()
                / 9.0
        })
        .collect()
}

/// Peak height over mean of the fold at `period`: how well one grid explains the track.
pub(super) fn sharpness(env: &[f32], fps: f64, period: f64) -> (f64, f64) {
    let bins = ((period * 1000.0).round() as usize).max(16); // ~1 ms bins
    let h = fold(env, fps, period, bins);
    let mean = h.iter().sum::<f64>() / bins as f64;
    let (i, peak) = h
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, p)| (i, *p))
        .unwrap_or((0, 0.0));
    // Parabolic interpolation of the peak position.
    let (l, r) = (h[(i + bins - 1) % bins], h[(i + 1) % bins]);
    let denom = l - 2.0 * peak + r;
    let delta = if denom.abs() > 1e-12 {
        (0.5 * (l - r) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let phase = (i as f64 + delta) / bins as f64 * period;
    let score = if mean > 0.0 { peak / mean } else { 0.0 };
    (score, phase)
}

/// Refine the period around `coarse` (± `relative_span`) coarse to fine and find the phase.
pub(super) fn fine_fit(env: &[f32], fps: f64, coarse: f64, relative_span: f64) -> Fit {
    let mut center = coarse;
    let mut span = relative_span * coarse;
    let mut best = (0.0, coarse, 0.0);
    for _ in 0..4 {
        let steps = 40;
        for s in 0..=steps {
            let p = center - span + 2.0 * span * s as f64 / steps as f64;
            let (score, phase) = sharpness(env, fps, p);
            if score > best.0 {
                best = (score, p, phase);
            }
        }
        center = best.1;
        span /= 10.0;
    }
    let (score, period, phase) = best;
    // A flat fold scores 1; a perfect pulse scores high. Map to 0..1.
    let confidence = (1.0 - 1.0 / score.max(1.0)) as f32;
    Fit {
        period,
        phase,
        confidence,
    }
}

/// `phase + k * period` from the first beat at or after 0 to `duration`.
pub(super) fn grid_times(phase: f64, period: f64, duration: f64) -> Vec<Beat> {
    let first = phase.rem_euclid(period);
    (0..)
        .map(|k| first + k as f64 * period)
        .take_while(|&t| t < duration)
        .map(|time| Beat { time, bar_pos: 0 })
        .collect()
}

/// Which beat (mod 4) starts the bars: where the four beats after differ most from the four
/// before, summed over the track.
pub(super) fn downbeat_phase(beats: &[Beat], bands: &[[f32; BANDS]], fps: f64) -> usize {
    let frame_of = |t: f64| ((t - frame_time(0.0, fps)) * fps).round().max(0.0) as usize;
    // Mean band energy over each beat.
    let per_beat: Vec<[f32; BANDS]> = beats
        .windows(2)
        .map(|w| {
            let (a, b) = (frame_of(w[0].time), frame_of(w[1].time).min(bands.len()));
            let mut m = [0.0f32; BANDS];
            if b > a {
                for f in &bands[a..b] {
                    for (x, y) in m.iter_mut().zip(f) {
                        *x += y;
                    }
                }
                for x in &mut m {
                    *x /= (b - a) as f32;
                }
            }
            m
        })
        .collect();
    let mut score = [0.0f64; 4];
    for b in 4..per_beat.len().saturating_sub(4) {
        let mut d = 0.0f64;
        for band in 0..BANDS {
            let before: f32 = per_beat[b - 4..b].iter().map(|x| x[band]).sum();
            let after: f32 = per_beat[b..b + 4].iter().map(|x| x[band]).sum();
            d += f64::from((after - before) / 4.0).abs();
        }
        score[b % 4] += d;
    }
    (0..4)
        .max_by(|&a, &b| score[a].total_cmp(&score[b]))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_at_the_first_beat_after_zero() {
        let g = grid_times(-0.1, 0.5, 2.0);
        let t: Vec<f64> = g.iter().map(|b| b.time).collect();
        assert_eq!(t, [0.4, 0.9, 1.4, 1.9]);
    }

    #[test]
    fn fold_finds_phase_of_a_pulse_train() {
        let fps = 200.0;
        let period = 0.5;
        // A pulse every 100 frames, starting at the frame whose time is 0.123 s.
        let k0 = ((0.123 - frame_time(0.0, fps)) * fps).round() as usize;
        let mut env = vec![0.0f32; 20_000];
        for k in (k0..env.len()).step_by(100) {
            env[k] = 1.0;
        }
        let (score, phase) = sharpness(&env, fps, period);
        assert!(score > 50.0, "{score}");
        let expected = frame_time(k0 as f64, fps).rem_euclid(period);
        assert!((phase - expected).abs() < 0.002, "{phase} vs {expected}");
    }
}
