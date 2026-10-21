//! Track analysis: tempo, beatgrid and key, behind pluggable backends.
//!
//! - [`model`]: what analysis produces ([`Tempo`], [`Beatgrid`], [`Key`]), each wrapped in
//!   [`Analysed`] with a confidence and a [`Provenance`]: the DJ, Rekordbox or one of our
//!   [`Analyzer`]s. Times are seconds from the start of the decoded track.
//! - [`Analyzer`]: the backend interface. One backend may provide several features from one
//!   pass. [`GridFit`] is our own constant-tempo grid; [`Stratum`] wraps stratum-dsp.
//! - With the `essentia` feature, [`essentia::Essentia`]: Essentia's classic algorithms and
//!   its TempoCNN model, in a Python worker process.
//! - [`Policy`]: picks one result per feature from several sources (manual, then Rekordbox,
//!   then analyzers in preference order, skipping unsure analyzer results).
//! - [`rekordbox`]: reads a Rekordbox XML export (tempo, key, grid) as ground truth or as a
//!   Rekordbox-sourced result.
//! - [`score`]: compares results against a reference, so backends can be ranked on a
//!   ground-truth set (`docs/research/engine.md`, "On-device analysis").
//!
//! Analysis runs on worker threads and never touches the audio callback.

pub mod analyzer;
#[cfg(feature = "essentia")]
pub mod essentia;
pub mod gridfit;
#[cfg(feature = "hpcpkey")]
pub mod hpcpkey;
pub mod model;
pub mod rekordbox;
pub mod score;
pub mod select;
#[cfg(feature = "stratum")]
pub mod stratum;
#[cfg(feature = "tempocnn")]
pub mod tempocnn;

pub use analyzer::{AnalysisError, Analyzer, CancelFlag, RunResults, run_all};
pub use gridfit::{GridFit, GridFitConfig, PhaseSource};
pub use model::{
    Analysed, Analysis, AnalyzerId, Beat, Beatgrid, BpmRange, Feature, FeatureSet, Key, Mode,
    PitchClass, Provenance, Settings, Tempo, TrackAudio,
};
pub use select::{Policy, Source};
#[cfg(feature = "stratum")]
pub use stratum::Stratum;

/// Decode a file for analysis, at its own sample rate.
pub fn load(path: impl AsRef<std::path::Path>) -> Result<TrackAudio, engine_media::LoadError> {
    load_until(path, &engine_media::Cancel::new())
}

/// [`load`], stopped once `cancel` is set.
pub fn load_until(
    path: impl AsRef<std::path::Path>,
    cancel: &engine_media::Cancel,
) -> Result<TrackAudio, engine_media::LoadError> {
    let (stereo, sample_rate) = engine_media::decode_native_until(path, cancel)?;
    Ok(TrackAudio {
        sample_rate,
        stereo,
    })
}

/// Every analyzer compiled into this build, in default preference order, with default
/// settings.
pub fn analyzers() -> Vec<Box<dyn Analyzer>> {
    analyzers_with(&Settings::default())
}

/// Every analyzer compiled into this build, configured with `settings`.
// One push per backend, each behind its own feature.
#[allow(unused_mut, clippy::vec_init_then_push)]
pub fn analyzers_with(settings: &Settings) -> Vec<Box<dyn Analyzer>> {
    let gridfit = |hint: Option<&'static str>| {
        GridFit::new(GridFitConfig {
            tempo_hint: hint,
            bpm_range: settings.bpm_range,
            ..GridFitConfig::default()
        })
    };
    let mut v: Vec<Box<dyn Analyzer>> = vec![Box::new(gridfit(None))];
    #[cfg(feature = "hpcpkey")]
    v.push(Box::new(hpcpkey::HpcpKey::default()));
    #[cfg(feature = "stratum")]
    v.push(Box::new(Stratum::new()));
    #[cfg(feature = "essentia")]
    for variant in [essentia::Variant::Dsp, essentia::Variant::TempoCnn] {
        match essentia::Essentia::from_env(variant) {
            Some(Ok(e)) => v.push(Box::new(e)),
            Some(Err(e)) => eprintln!("essentia ({variant:?}) unavailable: {e}"),
            None => {}
        }
    }
    #[cfg(feature = "tempocnn")]
    match tempocnn::TempoCnn::from_env() {
        Some(Ok(t)) => v.push(Box::new(t.with_range(settings.bpm_range))),
        Some(Err(e)) => eprintln!("tempocnn unavailable: {e}"),
        None => {}
    }
    #[cfg(feature = "tempocnn")]
    v.push(Box::new(gridfit(Some("tempocnn"))));
    // Last, so TempoCNN has run: gridfit refines its tempo.
    #[cfg(feature = "essentia")]
    v.push(Box::new(gridfit(Some("essentia-tempocnn"))));
    v
}
