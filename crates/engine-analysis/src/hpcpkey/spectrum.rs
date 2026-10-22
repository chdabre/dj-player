// SPDX-License-Identifier: AGPL-3.0-or-later
//
// A port of Essentia's KeyExtractor (FrameCutter → Windowing → Spectrum → SpectralPeaks →
// SpectralWhitening → HPCP → Key), Copyright (C) 2006-2021 Music Technology Group, Universitat
// Pompeu Fabra, AGPL-3.0. It follows Essentia's code closely, constants and quirks included,
// so that its keys match Essentia's on the same audio; this file is therefore AGPL-3.0 too.

//! Spectral peaks and spectral whitening, as Essentia's `SpectralPeaks` and
//! `SpectralWhitening`.

use super::{MAX_PEAKS, PEAK_THRESHOLD, RATE};

/// Essentia's `SpectralPeaks` (`PeakDetection` with interpolation, ordered by magnitude).
/// Returns (frequencies, magnitudes).
pub(super) fn spectral_peaks(spectrum: &[f32], min_hz: f32, max_hz: f32) -> (Vec<f32>, Vec<f32>) {
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

/// Spacing (Hz) of the whitening envelope's points.
const BPF_RESOLUTION: f32 = 100.0;

/// Essentia's `SpectralWhitening` with `maxFrequency` 3500 Hz.
pub(super) fn whiten(spectrum: &[f32], freqs: &[f32], mags: &[f32], max_hz: f32) -> Vec<f32> {
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

    let (xs, ys) = spectral_envelope(spectrum, max_freq, range);
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

/// The whitening's spectral envelope: weighted band energies every [`BPF_RESOLUTION`] Hz up
/// to `max_freq`, in dB, as the points (Hz, dB) of a break-point function.
fn spectral_envelope(spectrum: &[f32], max_freq: f32, range: f32) -> (Vec<f32>, Vec<f32>) {
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
    (xs, ys)
}
