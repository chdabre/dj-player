//! Where a result came from, and one source's results for a track.

use std::fmt;

use super::{Beatgrid, Feature, FeatureSet, Key, Tempo};

/// Identifies an analyzer and its configuration; part of the cache key, so results from an
/// older version or other parameters are never reused by mistake.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnalyzerId {
    /// Stable backend name, e.g. `stratum-dsp`.
    pub name: String,
    /// Backend version, e.g. the crate version.
    pub version: String,
    /// Fingerprint of the parameters that change results.
    pub params: String,
}

impl fmt::Display for AnalyzerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} ({})", self.name, self.version, self.params)
    }
}

/// Where a result came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// Edited by the DJ (tap tempo, grid shift, key override).
    Manual,
    /// Imported from a Rekordbox library or ANLZ file (read-only source).
    Rekordbox,
    /// Computed by one of our analyzers.
    Analyzer(AnalyzerId),
}

/// A value together with how sure its source is and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysed<T> {
    pub value: T,
    /// 0.0..=1.0 as reported by the source; `None` if it does not report one.
    pub confidence: Option<f32>,
    pub provenance: Provenance,
}

/// Everything one source produced for one track. Features it did not provide are `None`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Analysis {
    pub tempo: Option<Analysed<Tempo>>,
    pub beatgrid: Option<Analysed<Beatgrid>>,
    pub key: Option<Analysed<Key>>,
}

impl Analysis {
    /// The features this analysis actually holds.
    pub fn features(&self) -> FeatureSet {
        let mut set = FeatureSet::EMPTY;
        if self.tempo.is_some() {
            set = set.with(Feature::Tempo);
        }
        if self.beatgrid.is_some() {
            set = set.with(Feature::Beatgrid);
        }
        if self.key.is_some() {
            set = set.with(Feature::Key);
        }
        set
    }
}
