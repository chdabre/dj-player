//! The player's analysis: which analyzers run on a loaded track, in what order, and how their
//! results combine.

use crate::analyzer::Analyzer;
use crate::gridfit::{GridFit, GridFitConfig};
use crate::model::{Analysis, Settings};

/// Analyzers in the order they run, each refining the results before it.
pub type Chain = Vec<Box<dyn Analyzer>>;

/// The player's analyzers in order: TempoCNN (if `TEMPOCNN_MODEL` is set), gridfit with its
/// tempo as the hint, and the key. Also the name of the hint, if there is one. Fails if the
/// TempoCNN model is set but can't be loaded.
#[allow(unused_mut)] // Without the tempocnn feature nothing changes the hint.
pub fn player_chain(settings: Settings) -> Result<(Chain, Option<&'static str>), String> {
    let mut analyzers: Chain = Vec::new();
    let mut hint = None;
    #[cfg(feature = "tempocnn")]
    if let Some(t) = crate::tempocnn::TempoCnn::from_env() {
        analyzers.push(Box::new(t?.with_range(settings.bpm_range)));
        hint = Some("tempocnn");
    }
    analyzers.push(Box::new(GridFit::new(GridFitConfig {
        tempo_hint: hint,
        bpm_range: settings.bpm_range,
        ..GridFitConfig::default()
    })));
    #[cfg(feature = "hpcpkey")]
    analyzers.push(Box::new(crate::hpcpkey::HpcpKey::default()));
    Ok((analyzers, hint))
}

/// One result from the chain's, feature by feature: later analyzers refine earlier ones
/// (gridfit's tempo over TempoCNN's).
pub fn merge(results: Vec<Analysis>) -> Analysis {
    let mut merged = Analysis::default();
    for r in results {
        merged.tempo = r.tempo.or(merged.tempo);
        merged.beatgrid = r.beatgrid.or(merged.beatgrid);
        merged.key = r.key.or(merged.key);
    }
    merged
}
