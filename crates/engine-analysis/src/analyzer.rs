//! The interface every analysis backend implements.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::model::{Analysis, AnalyzerId, FeatureSet, TrackAudio};

/// A backend that computes some features from decoded audio.
///
/// One analyzer may provide several features from a single pass (stratum-dsp gives tempo,
/// beatgrid and key), so the interface is per backend, and [`Analyzer::features`] says which
/// it can do. Analysis runs on worker threads, never on the audio thread.
pub trait Analyzer: Send {
    /// Name, version and parameters: the cache key for everything it produces.
    fn id(&self) -> AnalyzerId;

    /// The features this analyzer can provide.
    fn features(&self) -> FeatureSet;

    /// Compute the features in `want` that this analyzer provides. The result holds only
    /// those; each carries [`crate::Provenance::Analyzer`] with [`Analyzer::id`].
    ///
    /// Backends check `cancel` where they can; one that runs a library call to completion
    /// checks it before and after.
    fn analyze(
        &mut self,
        audio: &TrackAudio,
        want: FeatureSet,
        cancel: &CancelFlag,
    ) -> Result<Analysis, AnalysisError>;
}

/// Shared cancellation flag for one analysis job.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` if cancelled.
    pub fn check(&self) -> Result<(), AnalysisError> {
        if self.is_cancelled() {
            Err(AnalysisError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
pub enum AnalysisError {
    Cancelled,
    /// Too short, silent, or otherwise unusable input.
    Input(String),
    /// The backend failed.
    Backend(String),
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisError::Cancelled => f.write_str("analysis cancelled"),
            AnalysisError::Input(e) => write!(f, "unusable input: {e}"),
            AnalysisError::Backend(e) => write!(f, "analyzer failed: {e}"),
        }
    }
}

impl std::error::Error for AnalysisError {}

/// What [`run_all`] returns: each analyzer that ran, with its result or its error.
pub type RunResults = Vec<(AnalyzerId, Result<Analysis, AnalysisError>)>;

/// Run every analyzer that provides something in `want`, in order, and collect what each one
/// produced. A failing analyzer is reported and skipped; cancellation stops the run.
pub fn run_all(
    analyzers: &mut [Box<dyn Analyzer>],
    audio: &TrackAudio,
    want: FeatureSet,
    cancel: &CancelFlag,
) -> Result<RunResults, AnalysisError> {
    let mut out = Vec::new();
    for a in analyzers.iter_mut() {
        let wanted = want.intersect(a.features());
        if wanted.is_empty() {
            continue;
        }
        cancel.check()?;
        let result = a.analyze(audio, wanted, cancel);
        if matches!(result, Err(AnalysisError::Cancelled)) {
            return Err(AnalysisError::Cancelled);
        }
        out.push((a.id(), result));
    }
    Ok(out)
}
