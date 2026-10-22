//! Which features analysis provides, and sets of them.

/// One feature analysis can provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Feature {
    /// The track's main tempo (what a player shows as BPM).
    Tempo = 1 << 0,
    /// Every beat, with bar positions.
    Beatgrid = 1 << 1,
    Key = 1 << 2,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Tempo, Feature::Beatgrid, Feature::Key];
}

/// A set of [`Feature`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FeatureSet(u8);

impl FeatureSet {
    pub const EMPTY: FeatureSet = FeatureSet(0);
    pub const ALL: FeatureSet = FeatureSet(0b111);

    pub fn contains(self, f: Feature) -> bool {
        self.0 & f as u8 != 0
    }

    pub fn with(self, f: Feature) -> Self {
        FeatureSet(self.0 | f as u8)
    }

    pub fn intersect(self, other: FeatureSet) -> Self {
        FeatureSet(self.0 & other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |f| self.contains(*f))
    }
}

impl From<Feature> for FeatureSet {
    fn from(f: Feature) -> Self {
        FeatureSet(f as u8)
    }
}

impl<const N: usize> From<[Feature; N]> for FeatureSet {
    fn from(fs: [Feature; N]) -> Self {
        fs.into_iter().fold(FeatureSet::EMPTY, FeatureSet::with)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_sets() {
        let s = FeatureSet::from([Feature::Tempo, Feature::Key]);
        assert!(s.contains(Feature::Tempo) && !s.contains(Feature::Beatgrid));
        assert_eq!(s.iter().count(), 2);
        assert!(s.intersect(Feature::Beatgrid.into()).is_empty());
    }
}
