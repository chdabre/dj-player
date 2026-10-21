// SPDX-License-Identifier: AGPL-3.0-or-later
//
// A port of Essentia's KeyExtractor (FrameCutter → Windowing → Spectrum → SpectralPeaks →
// SpectralWhitening → HPCP → Key), Copyright (C) 2006-2021 Music Technology Group, Universitat
// Pompeu Fabra, AGPL-3.0. It follows Essentia's code closely, constants and quirks included,
// so that its keys match Essentia's on the same audio; this file is therefore AGPL-3.0 too.

//! `hpcpkey`: key detection, ported from Essentia's `KeyExtractor` with its defaults.
//!
//! Per 4096-sample frame (hop 4096, first frame centred on sample 0) at 44.1 kHz: Hann window
//! normalised to area 2, magnitude spectrum, the 60 strongest spectral peaks between 25 Hz and
//! 3.5 kHz (parabolic interpolation, threshold 1e-4), spectral whitening, and a 12-bin HPCP
//! (5 harmonics, cosine weighting over one semitone, A = bin 0). The frames' HPCPs are
//! averaged, normalised to their peak, gated at 0.2 and correlated with the 24 rotations of a
//! major and a minor key profile (EDM by default).

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::analyzer::{AnalysisError, Analyzer, CancelFlag};
use crate::model::{
    Analysed, Analysis, AnalyzerId, Feature, FeatureSet, Key, Provenance, TrackAudio,
};

const RATE: u32 = 44_100;
const FRAME: usize = 4096;
const HOP: usize = 4096;
/// Essentia's defaults for the analysed frequency range.
const MIN_HZ: f32 = 25.0;
const MAX_HZ: f32 = 3500.0;
const PEAK_THRESHOLD: f32 = 0.0001;
const MAX_PEAKS: usize = 60;
const HARMONICS: usize = 4;
const REFERENCE_HZ: f32 = 440.0;
/// Essentia's default gate on the averaged, peak-normalised PCP.
const PCP_THRESHOLD: f32 = 0.2;

/// What can be changed; the defaults are Essentia's `KeyExtractor` defaults (with the EDM
/// profile), which reproduce its keys exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HpcpKeyConfig {
    pub profile: Profile,
    /// Spectral peaks below this are ignored (Essentia: 25 Hz).
    pub min_hz: f32,
    /// ... and above this (Essentia: 3500 Hz).
    pub max_hz: f32,
    /// PCP bins below this fraction of the strongest are zeroed (Essentia: 0.2).
    pub pcp_threshold: f32,
}

impl Default for HpcpKeyConfig {
    fn default() -> Self {
        Self {
            profile: Profile::Edma,
            min_hz: MIN_HZ,
            max_hz: MAX_HZ,
            pcp_threshold: PCP_THRESHOLD,
        }
    }
}
const PCP: usize = 12;

/// Major and minor key profiles, tonic first (Essentia's tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Faraldo et al. 2017, automatic profiles from electronic dance music (Essentia's
    /// `KeyExtractor` default in our bridge).
    Edma,
    /// Faraldo's "bgate" profiles for EDM.
    Bgate,
    Krumhansl,
    Temperley,
}

impl Profile {
    fn tables(self) -> ([f32; 12], [f32; 12]) {
        match self {
            Profile::Edma => (
                [
                    1.00, 0.29, 0.50, 0.40, 0.60, 0.56, 0.32, 0.80, 0.31, 0.45, 0.42, 0.39,
                ],
                [
                    1.00, 0.31, 0.44, 0.58, 0.33, 0.49, 0.29, 0.78, 0.43, 0.29, 0.53, 0.32,
                ],
            ),
            Profile::Bgate => (
                [
                    1.00, 0.00, 0.42, 0.00, 0.53, 0.37, 0.00, 0.77, 0.00, 0.38, 0.21, 0.30,
                ],
                [
                    1.00, 0.00, 0.36, 0.39, 0.00, 0.38, 0.00, 0.74, 0.27, 0.00, 0.42, 0.23,
                ],
            ),
            Profile::Krumhansl => (
                [
                    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
                ],
                [
                    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
                ],
            ),
            Profile::Temperley => (
                [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0],
                [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0],
            ),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Profile::Edma => "edma",
            Profile::Bgate => "bgate",
            Profile::Krumhansl => "krumhansl",
            Profile::Temperley => "temperley",
        }
    }
}

pub struct HpcpKey {
    config: HpcpKeyConfig,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    /// (semitone offset, strength) per distinct harmonic, as Essentia's HPCP table.
    harmonics: Vec<(f32, f32)>,
}

impl Default for HpcpKey {
    fn default() -> Self {
        Self::with_config(HpcpKeyConfig::default())
    }
}

impl HpcpKey {
    pub fn new(profile: Profile) -> Self {
        Self::with_config(HpcpKeyConfig {
            profile,
            ..HpcpKeyConfig::default()
        })
    }

    pub fn with_config(config: HpcpKeyConfig) -> Self {
        // Symmetric Hann, then Essentia's normalisation: area 2.
        let mut window: Vec<f32> = (0..FRAME)
            .map(|i| {
                (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (FRAME as f64 - 1.0)).cos()) as f32
            })
            .collect();
        let sum: f32 = window.iter().map(|w| w.abs()).sum();
        for w in &mut window {
            *w *= 2.0 / sum;
        }
        Self {
            config,
            window,
            fft: FftPlanner::new().plan_fft_forward(FRAME),
            harmonics: harmonic_table(),
        }
    }

    /// The averaged, gated 12-bin HPCP of a mono signal at 44.1 kHz (bin 0 = A).
    pub fn average_hpcp(
        &self,
        mono: &[f32],
        cancel: &CancelFlag,
    ) -> Result<[f32; PCP], AnalysisError> {
        let mut buf = vec![Complex32::default(); FRAME];
        let mut scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        let mut spectrum = vec![0.0f32; FRAME / 2 + 1];
        let mut sum = [0.0f64; PCP];
        let mut frames = 0usize;
        let mut start = -((FRAME / 2) as isize);
        while start < mono.len() as isize {
            if frames.is_multiple_of(64) {
                cancel.check()?;
            }
            for (i, c) in buf.iter_mut().enumerate() {
                let n = start + i as isize;
                let x = if n >= 0 && (n as usize) < mono.len() {
                    mono[n as usize]
                } else {
                    0.0
                };
                *c = Complex32::new(x * self.window[i], 0.0);
            }
            self.fft.process_with_scratch(&mut buf, &mut scratch);
            for (s, c) in spectrum.iter_mut().zip(&buf) {
                *s = c.norm();
            }
            let (freqs, mags) = spectral_peaks(&spectrum, self.config.min_hz, self.config.max_hz);
            let white = whiten(&spectrum, &freqs, &mags, self.config.max_hz);
            let hpcp = self.hpcp(&freqs, &white);
            for (s, h) in sum.iter_mut().zip(hpcp) {
                *s += f64::from(h);
            }
            frames += 1;
            start += HOP as isize;
        }
        let mut avg = [0.0f32; PCP];
        for (a, s) in avg.iter_mut().zip(sum) {
            *a = (s / frames.max(1) as f64) as f32;
        }
        // normalizePcpPeak, pcpGate, then shiftPcp (only a renormalisation at 12 bins).
        normalize_max(&mut avg);
        for a in &mut avg {
            if *a < self.config.pcp_threshold {
                *a = 0.0;
            }
        }
        normalize_max(&mut avg);
        Ok(avg)
    }

    fn hpcp(&self, freqs: &[f32], mags: &[f32]) -> [f32; PCP] {
        let mut hpcp = [0.0f32; PCP];
        for (&freq, &mag) in freqs.iter().zip(mags) {
            if !(self.config.min_hz..=self.config.max_hz).contains(&freq) {
                continue;
            }
            for &(semitone, strength) in &self.harmonics {
                let f = freq * 2f32.powf(-semitone / 12.0);
                // Cosine weighting over one semitone (resolution 1 bin per semitone).
                let bin = (f / REFERENCE_HZ).log2() * PCP as f32;
                let left = (bin - 0.5).ceil() as i32;
                let right = (bin + 0.5).floor() as i32;
                for i in left..=right {
                    let distance = (bin - i as f32).abs();
                    let weight = (std::f32::consts::PI * distance).cos();
                    let w = i.rem_euclid(PCP as i32) as usize;
                    hpcp[w] += weight * (mag * mag) * strength * strength;
                }
            }
        }
        hpcp
    }

    /// Essentia's `Key::compute` on a 12-bin PCP: (tonic pitch class from A, major?, strength).
    fn key(&self, pcp: &[f32; PCP]) -> Option<(usize, bool, f32)> {
        let (major, minor) = self.config.profile.tables();
        let stats = |v: &[f32; PCP]| {
            let mean = v.iter().sum::<f32>() / PCP as f32;
            let std = v
                .iter()
                .map(|x| (x - mean) * (x - mean))
                .sum::<f32>()
                .sqrt();
            (mean, std)
        };
        let (pm, ps) = stats(pcp);
        let corr = |profile: &[f32; PCP], shift: usize| -> f32 {
            let (m, s) = stats(profile);
            if ps == 0.0 || s == 0.0 {
                return 0.0;
            }
            let r: f32 = (0..PCP)
                .map(|i| (pcp[i] - pm) * (profile[(i + PCP - shift) % PCP] - m))
                .sum();
            r / (ps * s)
        };
        let best = |profile: &[f32; PCP]| {
            let mut max = -1.0f32;
            let mut index = None;
            for shift in 0..PCP {
                let c = corr(profile, shift);
                if c > max {
                    max = c;
                    index = Some(shift);
                }
            }
            index.map(|i| (i, max))
        };
        let (maj_i, maj) = best(&major)?;
        let (min_i, min) = best(&minor)?;
        Some(if maj > min {
            (maj_i, true, maj)
        } else {
            (min_i, false, min)
        })
    }
}

/// Essentia's HPCP harmonic table for `harmonics = 4` (harmonics 1..=5).
fn harmonic_table() -> Vec<(f32, f32)> {
    const PRECISION: f32 = 0.00001;
    let mut table: Vec<(f32, f32)> = Vec::new();
    for i in 0..=HARMONICS {
        let mut semitone = 12.0 * ((i + 1) as f32).log2();
        let octweight = 1.0f32.max(semitone / 12.0 * 0.5);
        while semitone >= 12.0 - PRECISION {
            semitone -= 12.0;
        }
        match table
            .iter_mut()
            .find(|(s, _)| *s > semitone - PRECISION && *s < semitone + PRECISION)
        {
            Some((_, strength)) => *strength += 1.0 / octweight,
            None => table.push((semitone, 1.0 / octweight)),
        }
    }
    table
}

fn normalize_max(v: &mut [f32]) {
    let max = v.iter().copied().fold(f32::MIN, f32::max);
    if max > 0.0 {
        for x in v.iter_mut() {
            *x /= max;
        }
    }
}

/// Essentia's `SpectralPeaks` (`PeakDetection` with interpolation, ordered by magnitude).
/// Returns (frequencies, magnitudes).
fn spectral_peaks(spectrum: &[f32], min_hz: f32, max_hz: f32) -> (Vec<f32>, Vec<f32>) {
    let size = spectrum.len();
    let a = spectrum;
    let scale = (RATE as f32 / 2.0) / (size - 1) as f32;
    let mut peaks: Vec<(f32, f32)> = Vec::new(); // (position, magnitude)
    let interpolate = |l: f32, m: f32, r: f32, bin: usize| {
        let dx = 0.5 * ((l - r) / (l - 2.0 * m + r));
        (bin as f32 + dx, m - 0.25 * (l - r) * dx)
    };
    let mut i = ((min_hz / scale).ceil() as usize).min(size - 1);
    if i + 1 < size && a[i] > a[i + 1] && a[i] > PEAK_THRESHOLD {
        peaks.push((i as f32 * scale, a[i]));
    }
    loop {
        while i + 1 < size - 1 && a[i] >= a[i + 1] {
            i += 1;
        }
        while i + 1 < size - 1 && a[i] < a[i + 1] {
            i += 1;
        }
        let mut j = i;
        while j + 1 < size - 1 && a[j] == a[j + 1] {
            j += 1;
        }
        if j + 1 < size - 1 && a[j + 1] < a[j] && a[j] > PEAK_THRESHOLD {
            let (bin, val) = if j != i {
                ((i + j) as f32 * 0.5, a[i])
            } else {
                interpolate(a[j - 1], a[j], a[j + 1], j)
            };
            let pos = bin * scale;
            if pos > max_hz {
                break;
            }
            peaks.push((pos, val));
        }
        i = j;
        if i + 1 >= size - 1 {
            if i == size - 2 && a[i - 1] < a[i] && a[i + 1] < a[i] && a[i] > PEAK_THRESHOLD {
                let (bin, val) = interpolate(a[i - 1], a[i], a[i + 1], j);
                peaks.push((bin * scale, val));
            }
            break;
        }
    }
    let pos = max_hz / scale;
    if ((size - 2) as f32) < pos
        && pos <= (size - 1) as f32
        && a[size - 1] > a[size - 2]
        && a[size - 1] > PEAK_THRESHOLD
    {
        peaks.push(((size - 1) as f32 * scale, a[size - 1]));
    }
    // Magnitude descending, then position ascending.
    peaks.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.total_cmp(&y.0)));
    peaks.truncate(MAX_PEAKS);
    peaks.into_iter().unzip()
}

fn lin2db(x: f32) -> f32 {
    if x < 1e-10 { -100.0 } else { 10.0 * x.log10() }
}

fn db2lin(x: f32) -> f32 {
    10f32.powf(x / 10.0)
}

/// Essentia's `SpectralWhitening` with `maxFrequency` 3500 Hz.
fn whiten(spectrum: &[f32], freqs: &[f32], mags: &[f32], max_hz: f32) -> Vec<f32> {
    const BPF_RESOLUTION: f32 = 100.0;
    let n = mags.len();
    if n == 0 {
        return Vec::new();
    }
    let max_freq = max_hz * 1.2;
    let range = RATE as f32 / 2.0;
    let mags_db: Vec<f32> = mags.iter().map(|&m| 2.0 * lin2db(m)).collect();
    let max_amp = freqs
        .iter()
        .zip(&mags_db)
        .filter(|(f, _)| **f <= max_freq)
        .map(|(_, a)| *a)
        .fold(-f32::MAX, f32::max);

    let spec_size = spectrum.len();
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let mut freq = 0.0f32;
    while freq <= max_freq && freq <= range {
        let bf = freq - 50.0f32.max(freq * 0.34);
        let ef = freq + 50.0f32.max(freq * 0.58);
        let mut b = (bf / range * (spec_size as f32 - 1.0) + 0.5) as i32;
        let mut e = (ef / range * (spec_size as f32 - 1.0) + 0.5) as i32;
        b = b.max(0).min(spec_size as i32 - 1);
        e = e.max(b + 1).min(spec_size as i32);
        let c = b as f32 / 2.0 + e as f32 / 2.0;
        let half = e as f32 - c;
        let (mut wsum, mut wavg) = (0.0f32, 0.0f32);
        for i in b..e {
            let mut w = 1.0 - (i as f32 - c).abs() / half;
            w *= w;
            w *= w;
            let energy = spectrum[i as usize] * spectrum[i as usize];
            w *= energy;
            wavg += energy * w;
            wsum += w;
        }
        if wsum != 0.0 {
            wavg /= wsum;
        }
        xs.push(freq);
        ys.push(wavg);
        freq += BPF_RESOLUTION;
    }
    let last = ys.len() - 1;
    ys[last] = ys[last - 1];
    for y in &mut ys {
        *y = 2.0 * lin2db(y.sqrt());
    }
    let bpf = |x: f32| {
        let mut j = 0;
        while j + 1 < xs.len() - 1 && x > xs[j + 1] {
            j += 1;
        }
        let slope = (ys[j + 1] - ys[j]) / (xs[j + 1] - xs[j]);
        (x - xs[j]) * slope + ys[j]
    };

    let mut white = vec![0.0f32; n];
    for i in 0..n {
        let f = freqs[i];
        let amp = mags_db[i];
        if f > max_freq - BPF_RESOLUTION {
            white[i] = amp;
            continue;
        }
        let env = bpf(f);
        // (Essentia first sets a value for amp < max_amp - 40 dB, which the branch below
        // always overwrites; kept out, with the same result.)
        let _ = max_amp;
        white[i] = if amp > env {
            0.0
        } else if amp > env - 30.0 {
            amp - env
        } else {
            -200.0
        };
        white[i] -= 20.0 * f / 4000.0;
    }
    for w in &mut white {
        *w = db2lin(*w / 2.0);
    }
    white
}

impl Analyzer for HpcpKey {
    fn id(&self) -> AnalyzerId {
        AnalyzerId {
            name: "hpcpkey".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            params: {
                let c = &self.config;
                format!(
                    "keyextractor,profile={},range={}-{},gate={}",
                    c.profile.name(),
                    c.min_hz,
                    c.max_hz,
                    c.pcp_threshold
                )
            },
        }
    }

    fn features(&self) -> FeatureSet {
        Feature::Key.into()
    }

    fn analyze(
        &mut self,
        audio: &TrackAudio,
        want: FeatureSet,
        _context: &[Analysis],
        cancel: &CancelFlag,
    ) -> Result<Analysis, AnalysisError> {
        if !want.contains(Feature::Key) {
            return Ok(Analysis::default());
        }
        let audio = audio.at_rate(RATE).map_err(AnalysisError::Backend)?;
        let pcp = self.average_hpcp(&audio.mono(), cancel)?;
        let Some((shift, major, strength)) = self.key(&pcp) else {
            return Ok(Analysis::default());
        };
        // Bin 0 is A (440 Hz); our pitch classes count from C.
        let tonic = (shift as i32 + 9) % 12;
        let key = if major {
            Key::major(tonic)
        } else {
            Key::minor(tonic)
        };
        Ok(Analysis {
            key: Some(Analysed {
                value: key,
                confidence: Some(strength),
                provenance: Provenance::Analyzer(self.id()),
            }),
            ..Analysis::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonic_table_matches_essentia() {
        // Harmonics 1, 2, 4 fold onto the root; 3 is a fifth (7.02); 5 a major third (3.86).
        let t = harmonic_table();
        assert_eq!(t.len(), 3);
        assert!(t[0].0.abs() < 1e-4 && (t[0].1 - (1.0 + 1.0 + 1.0)).abs() < 1e-4);
        assert!((t[1].0 - 7.019_55).abs() < 1e-3);
        assert!((t[2].0 - 3.863_14).abs() < 1e-3);
    }

    fn chord(notes_hz: &[f32], seconds: f32) -> Vec<f32> {
        let n = (seconds * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                notes_hz
                    .iter()
                    .map(|f| (std::f32::consts::TAU * f * t).sin())
                    .sum::<f32>()
                    * 0.2
            })
            .collect()
    }

    #[test]
    fn finds_a_minor_and_c_major_triads() {
        let k = HpcpKey::default();
        let c = CancelFlag::new();
        // A minor: A C E (and the octave).
        let am = k
            .average_hpcp(&chord(&[220.0, 261.63, 329.63, 440.0], 6.0), &c)
            .unwrap();
        let (shift, major, _) = k.key(&am).unwrap();
        assert_eq!(((shift + 9) % 12, major), (9, false));
        // C major: C E G.
        let cmaj = k
            .average_hpcp(&chord(&[261.63, 329.63, 392.0, 523.25], 6.0), &c)
            .unwrap();
        let (shift, major, _) = k.key(&cmaj).unwrap();
        assert_eq!(((shift + 9) % 12, major), (0, true));
    }
}
