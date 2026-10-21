//! TempoCNN in Rust: Hendrik Schreiber's tempo network (`deeptemp-k16-3`, as shipped with
//! Essentia, AGPL-3.0), converted to ONNX and run with tract, with Essentia's input step
//! (`TensorflowInputTempoCNN` and `TensorflowPredictTempoCNN`) reimplemented exactly:
//!
//! - audio at 11025 Hz, frames of 1024 samples every 512, the first centred on sample 0;
//! - symmetric Hann window, magnitude spectrum, 40 triangular mel bands (Slaney scale,
//!   20–5000 Hz, linear weighting, unit-area triangles), no log;
//! - patches of 256 frames every 128, each standardised over all its values; the last
//!   partial patch is dropped;
//! - per patch a softmax over 30..=285 BPM; the track tempo is the majority vote.
//!
//! The model is not bundled: [`TempoCnn::load`] takes the `.onnx` path (converted with
//! tf2onnx from `deeptemp-k16-3.pb`; see docs/plan/analysis.md).

use std::path::Path;
use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use tract_onnx::prelude::*;

use crate::analyzer::{AnalysisError, Analyzer, CancelFlag};
use crate::model::{
    Analysed, Analysis, AnalyzerId, Feature, FeatureSet, Provenance, Tempo, TrackAudio,
};

const RATE: u32 = 11_025;
const FRAME: usize = 1024;
const HOP: usize = 512;
const BANDS: usize = 40;
const PATCH: usize = 256;
const PATCH_HOP: usize = 128;
const BPM_OFFSET: usize = 30;
const LOW_HZ: f64 = 20.0;
const HIGH_HZ: f64 = 5000.0;

type Model = Arc<TypedRunnableModel>;

pub struct TempoCnn {
    model: Model,
    /// Per band: first spectrum bin and its filter coefficients.
    filters: Vec<(usize, Vec<f32>)>,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
}

impl TempoCnn {
    /// Load the ONNX model. Slow (model optimisation); do it once, off the audio thread.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let model = tract_onnx::onnx()
            .model_for_path(path.as_ref())
            .and_then(|m| m.with_input_fact(0, f32::fact([1, BANDS, PATCH, 1]).into()))
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("TempoCNN model {}: {e}", path.as_ref().display()))?;
        let window = (0..FRAME)
            .map(|i| {
                (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (FRAME as f64 - 1.0)).cos()) as f32
            })
            .collect();
        Ok(Self {
            model,
            filters: mel_filters(),
            window,
            fft: FftPlanner::new().plan_fft_forward(FRAME),
        })
    }

    /// `TEMPOCNN_MODEL` from the environment; `None` if not set.
    pub fn from_env() -> Option<Result<Self, String>> {
        let path = std::env::var_os("TEMPOCNN_MODEL")?;
        Some(Self::load(path))
    }

    /// Mel bands per frame, as Essentia's `TensorflowInputTempoCNN` computes them.
    fn features(&self, mono: &[f32]) -> Vec<[f32; BANDS]> {
        let mut out = Vec::new();
        let mut buf = vec![Complex32::default(); FRAME];
        let mut scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        let mut start = -((FRAME / 2) as isize);
        while start < mono.len() as isize {
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
            let mut bands = [0.0f32; BANDS];
            for (b, (first, coef)) in self.filters.iter().enumerate() {
                bands[b] = coef
                    .iter()
                    .enumerate()
                    .map(|(k, w)| w * buf[first + k].norm())
                    .sum();
            }
            out.push(bands);
            start += HOP as isize;
        }
        out
    }

    /// Softmax over 256 tempo classes for one standardised patch.
    fn predict(&self, patch: &[[f32; BANDS]]) -> Result<Vec<f32>, String> {
        let n = (PATCH * BANDS) as f32;
        let mean = patch.iter().flatten().sum::<f32>() / n;
        let var = patch
            .iter()
            .flatten()
            .map(|x| (x - mean).powi(2))
            .sum::<f32>()
            / n;
        let std = if var > 0.0 { var.sqrt() } else { 1.0 };
        // Model layout: [batch, mels, time, channel].
        let mut data = vec![0.0f32; BANDS * PATCH];
        for (t, frame) in patch.iter().enumerate() {
            for (m, x) in frame.iter().enumerate() {
                data[m * PATCH + t] = (x - mean) / std;
            }
        }
        let input: Tensor = tract_ndarray::Array4::from_shape_vec((1, BANDS, PATCH, 1), data)
            .map_err(|e| e.to_string())?
            .into();
        let out = self
            .model
            .run(tvec!(input.into()))
            .map_err(|e| format!("TempoCNN: {e}"))?;
        let probs = out[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| e.to_string())?;
        Ok(probs.iter().copied().collect())
    }

    /// Per-patch tempos (BPM) of a mono signal at 11025 Hz.
    pub fn local_tempi(
        &self,
        mono: &[f32],
        cancel: &CancelFlag,
    ) -> Result<Vec<u32>, AnalysisError> {
        let feats = self.features(mono);
        let mut tempi = Vec::new();
        let mut p = 0;
        while p + PATCH <= feats.len() {
            cancel.check()?;
            let probs = self
                .predict(&feats[p..p + PATCH])
                .map_err(AnalysisError::Backend)?;
            let best = probs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(0, |(i, _)| i);
            tempi.push((best + BPM_OFFSET) as u32);
            p += PATCH_HOP;
        }
        Ok(tempi)
    }
}

/// Essentia's majority vote: the tempo with the most patches; on a tie, the first seen.
fn majority(tempi: &[u32]) -> Option<(u32, usize)> {
    let mut best: Option<(u32, usize)> = None;
    let mut seen = Vec::new();
    for &t in tempi {
        if seen.contains(&t) {
            continue;
        }
        seen.push(t);
        let votes = tempi.iter().filter(|&&x| x == t).count();
        if best.is_none_or(|(_, v)| votes > v) {
            best = Some((t, votes));
        }
    }
    best
}

fn hz_to_slaney(hz: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const LIN: f64 = 3.0 / 200.0;
    if hz < MIN_LOG_HZ {
        hz * LIN
    } else {
        MIN_LOG_HZ * LIN + (hz / MIN_LOG_HZ).ln() / (6.4f64.ln() / 27.0)
    }
}

fn slaney_to_hz(mel: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const LIN: f64 = 3.0 / 200.0;
    let min_log_mel = MIN_LOG_HZ * LIN;
    if mel < min_log_mel {
        mel / LIN
    } else {
        MIN_LOG_HZ * ((mel - min_log_mel) * (6.4f64.ln() / 27.0)).exp()
    }
}

/// Essentia's `MelBands` + `TriangularBands` with the TempoCNN settings.
fn mel_filters() -> Vec<(usize, Vec<f32>)> {
    let bins = FRAME / 2 + 1;
    let (lo, hi) = (hz_to_slaney(LOW_HZ), hz_to_slaney(HIGH_HZ));
    let step = (hi - lo) / (BANDS + 1) as f64;
    let edges: Vec<f64> = (0..BANDS + 2)
        .map(|i| slaney_to_hz(lo + i as f64 * step))
        .collect();
    let scale = (f64::from(RATE) / 2.0) / (bins - 1) as f64;
    (0..BANDS)
        .map(|i| {
            let (f0, f1, f2) = (edges[i], edges[i + 1], edges[i + 2]);
            let (s1, s2) = (f1 - f0, f2 - f1);
            let first = (f0 / scale).ceil() as usize;
            let last = (f2 / scale).floor() as usize;
            // unit_tri: divide by the triangle's theoretical area, not the sum of weights.
            let area = (s1 + s2) / 2.0;
            let coef = (first..=last)
                .map(|j| {
                    let f = j as f64 * scale;
                    let w = if f < f1 { (f - f0) / s1 } else { (f2 - f) / s2 };
                    (w / area) as f32
                })
                .collect();
            (first, coef)
        })
        .collect()
}

impl Analyzer for TempoCnn {
    fn id(&self) -> AnalyzerId {
        AnalyzerId {
            name: "tempocnn".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            params: "deeptemp-k16-3,onnx,tract".into(),
        }
    }

    fn features(&self) -> FeatureSet {
        Feature::Tempo.into()
    }

    fn analyze(
        &mut self,
        audio: &TrackAudio,
        want: FeatureSet,
        _context: &[Analysis],
        cancel: &CancelFlag,
    ) -> Result<Analysis, AnalysisError> {
        cancel.check()?;
        if !want.contains(Feature::Tempo) {
            return Ok(Analysis::default());
        }
        let audio = audio.at_rate(RATE).map_err(AnalysisError::Backend)?;
        let tempi = self.local_tempi(&audio.mono(), cancel)?;
        let Some((bpm, votes)) = majority(&tempi) else {
            return Ok(Analysis::default());
        };
        Ok(Analysis {
            tempo: Some(Analysed {
                value: Tempo {
                    bpm: f64::from(bpm),
                },
                confidence: Some(votes as f32 / tempi.len() as f32),
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
    fn majority_vote_matches_essentia() {
        assert_eq!(
            majority(&[120, 124, 124, 120]),
            Some((120, 2)),
            "tie: first seen"
        );
        assert_eq!(majority(&[60, 124, 124]), Some((124, 2)));
        assert_eq!(majority(&[]), None);
    }

    #[test]
    fn mel_filters_cover_20_to_5000_hz_with_unit_area() {
        let f = mel_filters();
        assert_eq!(f.len(), BANDS);
        let scale = (f64::from(RATE) / 2.0) / (FRAME / 2) as f64;
        assert!(f[0].0 as f64 * scale >= LOW_HZ);
        let (first, coef) = f.last().unwrap();
        assert!((first + coef.len() - 1) as f64 * scale <= HIGH_HZ);
        // Weights times bin spacing approximate the unit area.
        for (_, c) in &f[10..] {
            let area: f64 = c.iter().map(|w| f64::from(*w) * scale).sum();
            assert!((area - 1.0).abs() < 0.1, "{area}");
        }
        assert!((slaney_to_hz(hz_to_slaney(3000.0)) - 3000.0).abs() < 1e-9);
    }
}
