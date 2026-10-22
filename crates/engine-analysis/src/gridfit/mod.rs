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

mod envelopes;
mod fit;
mod spectra;

use std::sync::Arc;

use rustfft::{Fft, FftPlanner};

use crate::analyzer::{AnalysisError, Analyzer, CancelFlag};
use crate::model::{
    Analysed, Analysis, AnalyzerId, Beat, Beatgrid, BpmRange, Feature, FeatureSet, Provenance,
    Tempo, TrackAudio,
};
use envelopes::{PHASE_BANDS, band_envelope, kick_envelope, onset_envelope};
use fit::{Fit, coarse_period, downbeat_phase, fine_fit, grid_times, sharpness};
use spectra::Spectra;

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
    /// The user's BPM range: without a hint it is the search range; a hint is folded into
    /// it.
    pub bpm_range: Option<BpmRange>,
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
            bpm_range: None,
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
                "{RATE}Hz,frame={FRAME},hop={HOP},range={}-{},prior={}/{},phase={:?},band_tempo={},bpm_range={}",
                c.min_bpm,
                c.max_bpm,
                c.prior_bpm,
                c.prior_octaves,
                c.phase,
                c.band_tempo,
                c.bpm_range.map_or("auto".into(), |r| r.to_string())
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
        let hint = self.hint(context);
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
        let search = self.search(hint.as_ref().map(|(bpm, _)| *bpm));
        let Some(coarse) = coarse_period(&[&env, &low, &linear], fps, &search) else {
            return Ok(Analysis::default());
        };
        cancel.check()?;
        // With a hint, the fine search must not wander off it either.
        let span = if hint.is_some() { HINT_SPAN } else { 0.02 };
        let mut fit = fine_fit(&env, fps, coarse, span);
        self.place_phase(&mut fit, &frames, &low, bands, fps);
        let duration = audio.duration();
        let mut beats = grid_times(fit.phase, fit.period, duration);
        let bar = downbeat_phase(&beats, &frames.bands, fps);
        for (k, b) in beats.iter_mut().enumerate() {
            b.bar_pos = ((k + 4 - bar) % 4) as u8 + 1;
        }
        Ok(self.result(&fit, beats, hint.map(|(_, from)| from), want))
    }
}

impl GridFit {
    /// The named analyzer's tempo (folded into the BPM range) and its id; never reference
    /// data (Rekordbox, manual edits).
    fn hint(&self, context: &[Analysis]) -> Option<(f64, AnalyzerId)> {
        self.config.tempo_hint.and_then(|from| {
            context.iter().find_map(|a| {
                let t = a.tempo.as_ref()?;
                match &t.provenance {
                    Provenance::Analyzer(id) if id.name == from => Some((
                        self.config
                            .bpm_range
                            .map_or(t.value.bpm, |r| r.fold(t.value.bpm)),
                        id.clone(),
                    )),
                    _ => None,
                }
            })
        })
    }

    /// Where the coarse stage searches: around the hint, else the user's BPM range, else
    /// the configured range.
    fn search(&self, hint_bpm: Option<f64>) -> GridFitConfig {
        match hint_bpm {
            Some(bpm) => GridFitConfig {
                min_bpm: bpm * (1.0 - HINT_SPAN),
                max_bpm: bpm * (1.0 + HINT_SPAN),
                ..self.config
            },
            None => match self.config.bpm_range {
                Some(r) => GridFitConfig {
                    min_bpm: r.min,
                    max_bpm: r.max,
                    ..self.config
                },
                None => self.config,
            },
        }
    }

    /// The kick defines where the beat is; hats and claps on the off-beats can outweigh it
    /// in the full band. Take the phase from the chosen source when it has a clear pulse.
    fn place_phase(&self, fit: &mut Fit, frames: &Spectra, low: &[f32], bands: Vec<f32>, fps: f64) {
        let source = match self.config.phase {
            PhaseSource::Full => None,
            PhaseSource::Low => Some(low.to_vec()),
            PhaseSource::Kick => Some(kick_envelope(&frames.flux_low, &frames.flux_high, fps)),
            PhaseSource::Bands => Some(bands),
        };
        if let Some(env) = source {
            let (score, phase) = sharpness(&env, fps, fit.period);
            if score > MIN_LOW_SHARPNESS {
                fit.phase = phase;
            }
        }
    }

    /// The fitted tempo and grid, as far as `want` asks for them.
    fn result(
        &self,
        fit: &Fit,
        beats: Vec<Beat>,
        hint: Option<AnalyzerId>,
        want: FeatureSet,
    ) -> Analysis {
        let mut id = self.id();
        if let Some(from) = &hint {
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
        out
    }
}
