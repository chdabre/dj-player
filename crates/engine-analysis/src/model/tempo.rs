//! Tempo, the user's BPM range, and the settings built on it.

use std::fmt;

/// Main tempo in beats per minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f64,
}

/// The range a DJ wants tempos in, like Rekordbox's BPM range setting: a track that fits
/// half or double its tempo equally well (87 or 174) is shown in the range. Tempos are
/// folded by octaves (doubling or halving), never otherwise changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BpmRange {
    pub min: f64,
    pub max: f64,
}

impl BpmRange {
    /// `None` unless `0 < min < max`.
    pub fn new(min: f64, max: f64) -> Option<Self> {
        (min > 0.0 && min < max).then_some(Self { min, max })
    }

    /// Parse "88-176".
    pub fn parse(s: &str) -> Option<Self> {
        let (a, b) = s.split_once('-')?;
        Self::new(a.trim().parse().ok()?, b.trim().parse().ok()?)
    }

    /// `bpm` doubled or halved into `[min, max)`. In a range narrower than an octave, the
    /// octave closest to it (in log terms) is used.
    pub fn fold(&self, bpm: f64) -> f64 {
        if bpm <= 0.0 || !bpm.is_finite() {
            return bpm;
        }
        let mut b = bpm;
        while b >= self.max {
            b /= 2.0;
        }
        while b < self.min {
            b *= 2.0;
        }
        if b >= self.max {
            // Narrower than an octave: b is above, b / 2 below; take the closer one.
            let below = b / 2.0;
            if (self.min / below).ln() < (b / self.max).ln() {
                b = below;
            }
        }
        b
    }
}

impl fmt::Display for BpmRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.min, self.max)
    }
}

/// Analysis settings a user can change.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Settings {
    /// Tempos are shown in this range; `None` keeps each analyzer's own choice.
    pub bpm_range: Option<BpmRange>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_range_folds_by_octaves() {
        let r = BpmRange::parse("88-176").unwrap();
        assert_eq!(r.fold(87.0), 174.0);
        assert_eq!(r.fold(174.0), 174.0);
        assert_eq!(r.fold(180.0), 90.0);
        assert_eq!(r.fold(44.0), 88.0);
        assert_eq!(r.fold(124.0), 124.0);
        let low = BpmRange::new(70.0, 140.0).unwrap();
        assert_eq!(low.fold(174.0), 87.0);
        // Narrower than an octave: the nearer octave.
        let narrow = BpmRange::new(120.0, 130.0).unwrap();
        assert_eq!(narrow.fold(62.0), 124.0);
        // 100: one octave up (200) is further from 120–130 than 100 is, so 100 stays.
        assert_eq!(narrow.fold(100.0), 100.0);
        assert!(BpmRange::new(100.0, 90.0).is_none() && BpmRange::parse("x").is_none());
    }
}
