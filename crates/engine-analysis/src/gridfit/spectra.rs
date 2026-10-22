//! Per-frame spectra: spectral flux in three bands and log energy in log-spaced bands.

use rustfft::num_complex::Complex32;

use super::{BANDS, FRAME, GridFit, HIGH_BIN, HOP, LOW_BINS};

pub(super) struct Spectra {
    /// Spectral flux per frame, all bins.
    pub(super) flux_full: Vec<f32>,
    /// Spectral flux per frame, low bins only.
    pub(super) flux_low: Vec<f32>,
    /// Spectral flux per frame, bins above ~2.5 kHz (clicks, hats).
    pub(super) flux_high: Vec<f32>,
    /// Log energy in [`BANDS`] log-spaced bands per frame.
    pub(super) bands: Vec<[f32; BANDS]>,
}

impl GridFit {
    pub(super) fn spectra(&self, mono: &[f32]) -> Spectra {
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
