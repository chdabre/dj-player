//! `gridfit`: our own constant-tempo beatgrid. Pure Rust.
//!
//! Club tracks almost always run at one fixed tempo (all 100 Rekordbox grids in the first
//! ground-truth set do), so instead of tracking beats one by one, this fits a single grid,
//! `phase + k * period`, to the whole track:
//!
//! 1. **Onset envelope:** spectral flux of log magnitudes, with the low band (kick, bass)
//!    weighted up, at a hop of 256 frames (5.8 ms at 44.1 kHz), minus its local mean.
//! 2. **Coarse tempo:** autocorrelation of the envelope, with its 2x and 4x lags added in so
//!    the beat level wins over the bar level, and a mild preference for ~124 BPM inside the
//!    DJ range.
//! 3. **Fine tempo and phase:** for each candidate period, fold the envelope of the whole
//!    track modulo the period. The right period piles every onset onto one sharp peak; a
//!    period off by 0.01 BPM smears it over a 5-minute track. The peak position is the
//!    phase. Searched coarse to fine, down to about 0.001 BPM.
//!    The phase then comes from the low band alone where it has a clear pulse, so the grid
//!    sits on the kick rather than on off-beat hats.
//! 4. **Downbeat:** the beat (mod 4) where the spectrum changes most between the four
//!    beats before and the four after: new parts usually enter on the one.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::analyzer::{AnalysisError, Analyzer, CancelFlag};
use crate::model::{
    Analysed, Analysis, AnalyzerId, Beat, Beatgrid, Feature, FeatureSet, Provenance, Tempo,
    TrackAudio,
};

const RATE: u32 = 44_100;
const FRAME: usize = 1024;
const HOP: usize = 256;
/// Bins below ~170 Hz count as the low band (kick, bass).
const LOW_BINS: usize = 4;
const LOW_WEIGHT: f32 = 2.0;
/// Bins from ~2.5 kHz up count as the high band (beater click, hats).
const HIGH_BIN: usize = 58;
const BANDS: usize = 24;
/// Fold sharpness above which the low band alone sets the beat phase.
const MIN_LOW_SHARPNESS: f64 = 2.0;
/// Extra onset-time correction (seconds) on top of the frame geometry; calibrated on
/// synthetic kicks.
const ONSET_LATENCY: f64 = 0.0;

/// Which onsets decide where the beat is, once the tempo is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseSource {
    /// Spectral flux over all bins.
    Full,
    /// Low band only (kick and bass).
    Low,
    /// Low band and high band together: a kick has both its body and its beater click; an
    /// off-beat bass note has no click and a hat has no body.
    Kick,
    /// The log-spaced bands below ~4.5 kHz, each weighted equally. On 70 Rekordbox-checked
    /// tracks every one of these bands was louder on the beat than on the off-beat in 53–66
    /// of them, while the hat range above was not (or the other way round). Flux summed
    /// over linear bins is dominated by that high range, which is what put grids on
    /// off-beat hats.
    Bands,
}

/// Search range and the tempo the coarse stage leans towards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridFitConfig {
    pub min_bpm: f64,
    pub max_bpm: f64,
    pub prior_bpm: f64,
    /// Width of the preference in octaves (larger = weaker).
    pub prior_octaves: f64,
    pub phase: PhaseSource,
    /// Use the band envelope (see [`PhaseSource::Bands`]) instead of the linear-bin flux
    /// for the tempo as well.
    pub band_tempo: bool,
    /// Refine the tempo of this analyzer (by [`AnalyzerId::name`]), if it ran earlier on
    /// the same track, instead of choosing the tempo itself: the search then stays within
    /// [`HINT_SPAN`] of it, so the hint decides octave and 3:2 questions.
    pub tempo_hint: Option<&'static str>,
}

/// How far (relative) gridfit searches around a hinted tempo. TempoCNN reports whole BPM,
/// so ±1 % covers its rounding at any club tempo.
pub const HINT_SPAN: f64 = 0.01;

impl Default for GridFitConfig {
    fn default() -> Self {
        Self {
            min_bpm: 70.0,
            max_bpm: 180.0,
            prior_bpm: 124.0,
            // Tuned on the first Rekordbox set (100 tracks): 0.8–1.5 score alike; 3.0 (almost
            // no preference) drops tempo accuracy from 89 % to 70 %.
            prior_octaves: 1.0,
            phase: PhaseSource::Bands,
            band_tempo: true,
            tempo_hint: None,
        }
    }
}

pub struct GridFit {
    config: GridFitConfig,
    fft: Arc<dyn Fft<f32>>,
}

impl Default for GridFit {
    fn default() -> Self {
        Self::new(GridFitConfig::default())
    }
}

impl GridFit {
    /// gridfit that refines the tempo of `analyzer`, run before it (`gridfit+<analyzer>`).
    pub fn hinted(analyzer: &'static str) -> Self {
        Self::new(GridFitConfig {
            tempo_hint: Some(analyzer),
            ..GridFitConfig::default()
        })
    }

    pub fn new(config: GridFitConfig) -> Self {
        Self {
            config,
            fft: FftPlanner::new().plan_fft_forward(FRAME),
        }
    }
}

impl Analyzer for GridFit {
    fn id(&self) -> AnalyzerId {
        let c = &self.config;
        AnalyzerId {
            name: match c.tempo_hint {
                Some(from) => format!("gridfit+{from}"),
                None => "gridfit".into(),
            },
            version: env!("CARGO_PKG_VERSION").into(),
            params: format!(
                "{RATE}Hz,frame={FRAME},hop={HOP},range={}-{},prior={}/{},phase={:?},band_tempo={}",
                c.min_bpm, c.max_bpm, c.prior_bpm, c.prior_octaves, c.phase, c.band_tempo
            ),
        }
    }

    fn features(&self) -> FeatureSet {
        FeatureSet::from([Feature::Tempo, Feature::Beatgrid])
    }

    fn analyze(
        &mut self,
        audio: &TrackAudio,
        want: FeatureSet,
        context: &[Analysis],
        cancel: &CancelFlag,
    ) -> Result<Analysis, AnalysisError> {
        cancel.check()?;
        // The named analyzer's tempo; never reference data (Rekordbox, manual edits).
        let hint = self.config.tempo_hint.and_then(|from| {
            context.iter().find_map(|a| {
                let t = a.tempo.as_ref()?;
                match &t.provenance {
                    Provenance::Analyzer(id) if id.name == from => Some((t.value.bpm, id.clone())),
                    _ => None,
                }
            })
        });
        let audio = audio.at_rate(RATE).map_err(AnalysisError::Backend)?;
        let mono = audio.mono();
        if mono.len() < FRAME * 64 {
            return Err(AnalysisError::Input("too short for a grid".into()));
        }
        let frames = self.spectra(&mono);
        cancel.check()?;
        let fps = f64::from(RATE) / HOP as f64;
        let bands = band_envelope(&frames.bands, PHASE_BANDS, fps);
        let env = if self.config.band_tempo {
            bands.clone()
        } else {
            onset_envelope(&frames.flux_full, &frames.flux_low, fps)
        };
        let low = onset_envelope(&frames.flux_low, &frames.flux_low, fps);
        let linear = onset_envelope(&frames.flux_full, &frames.flux_low, fps);
        let search = match &hint {
            Some((bpm, _)) => GridFitConfig {
                min_bpm: bpm * (1.0 - HINT_SPAN),
                max_bpm: bpm * (1.0 + HINT_SPAN),
                ..self.config
            },
            None => self.config,
        };
        let Some(coarse) = coarse_period(&[&env, &low, &linear], fps, &search) else {
            return Ok(Analysis::default());
        };
        cancel.check()?;
        // With a hint, the fine search must not wander off it either.
        let span = if hint.is_some() { HINT_SPAN } else { 0.02 };
        let mut fit = fine_fit(&env, fps, coarse, span);
        // The kick defines where the beat is; hats and claps on the off-beats can outweigh it
        // in the full band. Take the phase from the chosen source when it has a clear pulse.
        let source = match self.config.phase {
            PhaseSource::Full => None,
            PhaseSource::Low => Some(low.clone()),
            PhaseSource::Kick => Some(kick_envelope(&frames.flux_low, &frames.flux_high, fps)),
            PhaseSource::Bands => Some(bands),
        };
        if let Some(env) = source {
            let (score, phase) = sharpness(&env, fps, fit.period);
            if score > MIN_LOW_SHARPNESS {
                fit.phase = phase;
            }
        }
        let duration = audio.duration();
        let mut beats = grid_times(fit.phase, fit.period, duration);
        let bar = downbeat_phase(&beats, &frames.bands, fps);
        for (k, b) in beats.iter_mut().enumerate() {
            b.bar_pos = ((k + 4 - bar) % 4) as u8 + 1;
        }

        let mut id = self.id();
        if let Some((_, from)) = &hint {
            // The result depends on the hint's source: part of the cache key.
            id.params += &format!(",hint={} {}", from.name, from.version);
        }
        let provenance = Provenance::Analyzer(id);
        let mut out = Analysis::default();
        if want.contains(Feature::Tempo) {
            out.tempo = Some(Analysed {
                value: Tempo {
                    bpm: 60.0 / fit.period,
                },
                confidence: Some(fit.confidence),
                provenance: provenance.clone(),
            });
        }
        if want.contains(Feature::Beatgrid) {
            out.beatgrid = Some(Analysed {
                value: Beatgrid { beats },
                confidence: Some(fit.confidence),
                provenance,
            });
        }
        Ok(out)
    }
}

struct Spectra {
    /// Spectral flux per frame, all bins.
    flux_full: Vec<f32>,
    /// Spectral flux per frame, low bins only.
    flux_low: Vec<f32>,
    /// Spectral flux per frame, bins above ~2.5 kHz (clicks, hats).
    flux_high: Vec<f32>,
    /// Log energy in [`BANDS`] log-spaced bands per frame.
    bands: Vec<[f32; BANDS]>,
}

impl GridFit {
    fn spectra(&self, mono: &[f32]) -> Spectra {
        let n = (mono.len() - FRAME) / HOP + 1;
        let window: Vec<f32> = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FRAME as f32).cos())
            .collect();
        let half = FRAME / 2;
        // Log-spaced band edges from ~40 Hz to Nyquist.
        let edges: Vec<usize> = (0..=BANDS)
            .map(|b| {
                let lo = 1.0f64;
                let hi = half as f64;
                (lo * (hi / lo).powf(b as f64 / BANDS as f64)).round() as usize
            })
            .collect();
        let mut buf = vec![Complex32::default(); FRAME];
        let mut scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        let mut prev = vec![0.0f32; half];
        let mut cur = vec![0.0f32; half];
        let mut out = Spectra {
            flux_full: Vec::with_capacity(n),
            flux_low: Vec::with_capacity(n),
            flux_high: Vec::with_capacity(n),
            bands: Vec::with_capacity(n),
        };
        for k in 0..n {
            let start = k * HOP;
            for (i, c) in buf.iter_mut().enumerate() {
                *c = Complex32::new(mono[start + i] * window[i], 0.0);
            }
            self.fft.process_with_scratch(&mut buf, &mut scratch);
            for (i, m) in cur.iter_mut().enumerate() {
                *m = (1.0 + 100.0 * buf[i].norm()).ln();
            }
            let (mut full, mut low, mut high) = (0.0f32, 0.0f32, 0.0f32);
            for i in 1..half {
                let d = (cur[i] - prev[i]).max(0.0);
                full += d;
                if i <= LOW_BINS {
                    low += d;
                } else if i >= HIGH_BIN {
                    high += d;
                }
            }
            let mut bands = [0.0f32; BANDS];
            for (b, e) in bands.iter_mut().enumerate() {
                let (lo, hi) = (edges[b], edges[b + 1].max(edges[b] + 1).min(half));
                *e = cur[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
            }
            out.flux_full.push(if k == 0 { 0.0 } else { full });
            out.flux_low.push(if k == 0 { 0.0 } else { low });
            out.flux_high.push(if k == 0 { 0.0 } else { high });
            out.bands.push(bands);
            std::mem::swap(&mut prev, &mut cur);
        }
        out
    }
}

/// Time (seconds) of the onset that envelope frame `k` responds to most: half a hop after
/// the centre of frame `k`. This was measured, not derived: on synthetic kicks with exact
/// onset times it puts the grid within ±2 ms (see the gridfit test).
fn frame_time(k: f64, fps: f64) -> f64 {
    (k + 0.5) / fps + (FRAME as f64 / 2.0) / f64::from(RATE) + ONSET_LATENCY
}

/// Normalised flux (low band weighted up) minus a ~0.4 s moving average, floored at 0.
fn onset_envelope(full: &[f32], low: &[f32], fps: f64) -> Vec<f32> {
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

/// Bands `0..max_band` that set the phase (edges up to ~4.5 kHz at 44.1 kHz).
const PHASE_BANDS: usize = 18;

/// Equal-weight sum of the per-band onset envelopes of bands `0..max_band`.
fn band_envelope(bands: &[[f32; BANDS]], max_band: usize, fps: f64) -> Vec<f32> {
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
fn kick_envelope(low: &[f32], high: &[f32], fps: f64) -> Vec<f32> {
    let l = onset_envelope(low, low, fps);
    let h = onset_envelope(high, high, fps);
    l.iter().zip(&h).map(|(a, b)| (a * b).sqrt()).collect()
}

/// Beat period (seconds) from the summed beat salience of several envelopes' autocorrelations,
/// or `None` for a track with no pulse. Each sees something the others miss: the low band
/// (kicks) keeps 3:2 periods out, since kicks only line up at whole beats; the linear-bin
/// flux hears the off-beat hats, which tell 165 BPM from 82.5 when every beat has a kick.
fn coarse_period(envs: &[&[f32]], fps: f64, c: &GridFitConfig) -> Option<f64> {
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

struct Fit {
    period: f64,
    phase: f64,
    confidence: f32,
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
fn sharpness(env: &[f32], fps: f64, period: f64) -> (f64, f64) {
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
fn fine_fit(env: &[f32], fps: f64, coarse: f64, relative_span: f64) -> Fit {
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
fn grid_times(phase: f64, period: f64, duration: f64) -> Vec<Beat> {
    let first = phase.rem_euclid(period);
    (0..)
        .map(|k| first + k as f64 * period)
        .take_while(|&t| t < duration)
        .map(|time| Beat { time, bar_pos: 0 })
        .collect()
}

/// Which beat (mod 4) starts the bars: where the four beats after differ most from the four
/// before, summed over the track.
fn downbeat_phase(beats: &[Beat], bands: &[[f32; BANDS]], fps: f64) -> usize {
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
