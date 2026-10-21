//! Choosing one result per feature from several sources.
//!
//! The default order follows `docs/research/engine.md`: the DJ's own edits, then Rekordbox's
//! analysis from the stick, then our analyzers in preference order. An analyzer result below
//! the confidence threshold is passed over for the next source; if nothing passes, the
//! highest-ranked result is used anyway, since some grid beats none.

use crate::model::{Analysed, Analysis, Provenance};

/// A source in the preference order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Manual,
    Rekordbox,
    /// An analyzer by [`crate::AnalyzerId::name`] (any version).
    Analyzer(String),
}

impl Source {
    fn matches(&self, p: &Provenance) -> bool {
        match (self, p) {
            (Source::Manual, Provenance::Manual) => true,
            (Source::Rekordbox, Provenance::Rekordbox) => true,
            (Source::Analyzer(name), Provenance::Analyzer(id)) => *name == id.name,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub order: Vec<Source>,
    /// Analyzer results below this confidence lose to the next source. Manual edits and
    /// Rekordbox data are always trusted.
    pub min_confidence: f32,
}

impl Policy {
    /// Manual, Rekordbox, then the given analyzers in order.
    pub fn with_analyzers<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut order = vec![Source::Manual, Source::Rekordbox];
        order.extend(names.into_iter().map(|n| Source::Analyzer(n.into())));
        Policy {
            order,
            min_confidence: 0.3,
        }
    }

    fn rank(&self, p: &Provenance) -> Option<usize> {
        self.order.iter().position(|s| s.matches(p))
    }

    fn trusted<T>(&self, a: &Analysed<T>) -> bool {
        match a.provenance {
            Provenance::Analyzer(_) => a.confidence.is_none_or(|c| c >= self.min_confidence),
            _ => true,
        }
    }

    fn pick<'a, T>(&self, found: impl Iterator<Item = &'a Analysed<T>>) -> Option<&'a Analysed<T>>
    where
        T: 'a,
    {
        let mut ranked: Vec<(usize, &Analysed<T>)> = found
            .filter_map(|a| self.rank(&a.provenance).map(|r| (r, a)))
            .collect();
        ranked.sort_by_key(|(r, _)| *r);
        ranked
            .iter()
            .find(|(_, a)| self.trusted(a))
            .or(ranked.first())
            .map(|(_, a)| *a)
    }

    /// One result per feature from `candidates`. Sources not in the order are ignored.
    pub fn select(&self, candidates: &[Analysis]) -> Analysis {
        Analysis {
            tempo: self
                .pick(candidates.iter().filter_map(|c| c.tempo.as_ref()))
                .cloned(),
            beatgrid: self
                .pick(candidates.iter().filter_map(|c| c.beatgrid.as_ref()))
                .cloned(),
            key: self
                .pick(candidates.iter().filter_map(|c| c.key.as_ref()))
                .cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AnalyzerId, Key, Tempo};

    fn analyzer(name: &str) -> Provenance {
        Provenance::Analyzer(AnalyzerId {
            name: name.into(),
            version: "1".into(),
            params: String::new(),
        })
    }

    fn tempo(bpm: f64, confidence: Option<f32>, provenance: Provenance) -> Analysis {
        Analysis {
            tempo: Some(Analysed {
                value: Tempo { bpm },
                confidence,
                provenance,
            }),
            ..Analysis::default()
        }
    }

    #[test]
    fn follows_the_order_and_skips_unsure_analyzers() {
        let p = Policy::with_analyzers(["stratum-dsp", "essentia"]);
        let ours = tempo(124.0, Some(0.9), analyzer("stratum-dsp"));
        let rb = tempo(125.0, None, Provenance::Rekordbox);
        assert_eq!(
            p.select(&[ours.clone(), rb.clone()])
                .tempo
                .unwrap()
                .value
                .bpm,
            125.0
        );

        let unsure = tempo(62.0, Some(0.1), analyzer("stratum-dsp"));
        let fallback = tempo(124.0, Some(0.8), analyzer("essentia"));
        assert_eq!(
            p.select(&[unsure.clone(), fallback])
                .tempo
                .unwrap()
                .value
                .bpm,
            124.0
        );
        // Nothing passes: the best-ranked result still wins.
        assert_eq!(p.select(&[unsure]).tempo.unwrap().value.bpm, 62.0);
    }

    #[test]
    fn features_are_chosen_independently() {
        let p = Policy::with_analyzers(["stratum-dsp"]);
        let rb = tempo(125.0, None, Provenance::Rekordbox);
        let ours = Analysis {
            key: Some(Analysed {
                value: Key::minor(9),
                confidence: Some(0.7),
                provenance: analyzer("stratum-dsp"),
            }),
            ..tempo(124.0, Some(0.9), analyzer("stratum-dsp"))
        };
        let pick = p.select(&[ours, rb]);
        assert_eq!(pick.tempo.unwrap().provenance, Provenance::Rekordbox);
        assert_eq!(pick.key.unwrap().value, Key::minor(9));
    }

    #[test]
    fn unknown_sources_are_ignored() {
        let p = Policy::with_analyzers(["stratum-dsp"]);
        assert!(
            p.select(&[tempo(90.0, Some(1.0), analyzer("other"))])
                .tempo
                .is_none()
        );
    }
}
