//! What analysis produces, independent of who produced it.
//!
//! Times are seconds from the first decoded frame of the track (after the decoder's gapless
//! trimming), as `f64`. Seconds work for every source: Rekordbox stores milliseconds, files
//! come at 44.1 or 48 kHz, and the deck runs at the device rate. Each consumer converts to its
//! own frames when it loads the result.

use std::fmt;

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

/// Main tempo in beats per minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f64,
}

/// One beat of a [`Beatgrid`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    /// Seconds from the start of the track.
    pub time: f64,
    /// Position in the bar, 1 = downbeat (1..=4 in 4/4); 0 = unknown (the source found the
    /// beats but not the bars).
    pub bar_pos: u8,
}

/// Every beat of a track, in time order. A list rather than (offset, BPM), so tempo changes
/// and drifting live recordings work.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Beatgrid {
    pub beats: Vec<Beat>,
}

impl Beatgrid {
    /// A constant-tempo grid: first downbeat at `first`, `bpm`, 4/4, until `until` seconds.
    pub fn constant(first: f64, bpm: f64, until: f64) -> Self {
        let period = 60.0 / bpm;
        let beats = (0..)
            .map(|k| Beat {
                time: first + k as f64 * period,
                bar_pos: (k % 4) as u8 + 1,
            })
            .take_while(|b| b.time < until)
            .collect();
        Beatgrid { beats }
    }

    pub fn is_empty(&self) -> bool {
        self.beats.is_empty()
    }

    /// True if the grid knows where the bars start.
    pub fn has_bars(&self) -> bool {
        self.beats.iter().any(|b| b.bar_pos != 0)
    }

    /// Beats at the given times with unknown bar positions.
    pub fn from_times(times: impl IntoIterator<Item = f64>) -> Self {
        Beatgrid {
            beats: times
                .into_iter()
                .map(|time| Beat { time, bar_pos: 0 })
                .collect(),
        }
    }

    /// Median tempo from the beat intervals, if there are at least two beats.
    pub fn median_bpm(&self) -> Option<f64> {
        let mut periods: Vec<f64> = self
            .beats
            .windows(2)
            .map(|w| w[1].time - w[0].time)
            .filter(|p| *p > 0.0)
            .collect();
        if periods.is_empty() {
            return None;
        }
        periods.sort_by(f64::total_cmp);
        Some(60.0 / periods[periods.len() / 2])
    }
}

/// Pitch class, 0 = C ... 11 = B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PitchClass(u8);

impl PitchClass {
    pub fn new(semitones_from_c: i32) -> Self {
        PitchClass(semitones_from_c.rem_euclid(12) as u8)
    }

    pub fn index(self) -> u8 {
        self.0
    }

    /// The pitch class `semitones` above this one.
    pub fn up(self, semitones: i32) -> Self {
        PitchClass::new(i32::from(self.0) + semitones)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Major,
    Minor,
}

/// A musical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub tonic: PitchClass,
    pub mode: Mode,
}

impl Key {
    pub fn major(tonic: i32) -> Self {
        Key {
            tonic: PitchClass::new(tonic),
            mode: Mode::Major,
        }
    }

    pub fn minor(tonic: i32) -> Self {
        Key {
            tonic: PitchClass::new(tonic),
            mode: Mode::Minor,
        }
    }

    /// Parse "C", "F#", "Bb", "Am", "C#m", "Ebmin", "A minor", "D major".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut chars = s.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let base = match letter {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        let rest = chars.as_str();
        let (accidental, rest) = match rest.chars().next() {
            Some(c @ ('#' | '♯')) => (1, &rest[c.len_utf8()..]),
            Some(c @ ('b' | '♭')) => (-1, &rest[c.len_utf8()..]),
            _ => (0, rest),
        };
        let mode = match rest.trim().to_ascii_lowercase().as_str() {
            "" | "maj" | "major" => Mode::Major,
            "m" | "min" | "minor" => Mode::Minor,
            _ => return None,
        };
        Some(Key {
            tonic: PitchClass::new(base + accidental),
            mode,
        })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
        ];
        let m = if self.mode == Mode::Minor { "m" } else { "" };
        write!(f, "{}{m}", NAMES[usize::from(self.tonic.0)])
    }
}

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

/// Decoded audio as analysis sees it: interleaved stereo at the file's own rate.
#[derive(Debug, Clone)]
pub struct TrackAudio {
    pub sample_rate: u32,
    pub stereo: Vec<f32>,
}

impl TrackAudio {
    pub fn len_frames(&self) -> usize {
        self.stereo.len() / 2
    }

    pub fn duration(&self) -> f64 {
        self.len_frames() as f64 / f64::from(self.sample_rate)
    }

    /// This audio at `rate`, resampled if needed (borrowed if it already is at `rate`).
    pub fn at_rate(&self, rate: u32) -> Result<std::borrow::Cow<'_, TrackAudio>, String> {
        if self.sample_rate == rate {
            return Ok(std::borrow::Cow::Borrowed(self));
        }
        let stereo = engine_media::resample(&self.stereo, self.sample_rate, rate)
            .map_err(|e| format!("resample: {e}"))?;
        Ok(std::borrow::Cow::Owned(TrackAudio {
            sample_rate: rate,
            stereo,
        }))
    }

    /// Mono mixdown (L+R)/2.
    pub fn mono(&self) -> Vec<f32> {
        self.stereo
            .chunks_exact(2)
            .map(|f| 0.5 * (f[0] + f[1]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_keys() {
        for (s, k) in [
            ("C", Key::major(0)),
            ("Am", Key::minor(9)),
            ("F#m", Key::minor(6)),
            ("Bb", Key::major(10)),
            ("Ebmin", Key::minor(3)),
            ("A minor", Key::minor(9)),
            ("D major", Key::major(2)),
            ("C#", Key::major(1)),
        ] {
            assert_eq!(Key::parse(s), Some(k), "{s}");
        }
        assert_eq!(Key::parse("H"), None);
        assert_eq!(Key::parse("C dorian"), None);
        assert_eq!(Key::minor(9).to_string(), "Am");
        assert_eq!(Key::major(3).to_string(), "Eb");
    }

    #[test]
    fn constant_grid_counts_bars_and_has_its_tempo() {
        let g = Beatgrid::constant(0.5, 120.0, 10.0);
        assert_eq!(g.beats.len(), 19);
        assert_eq!(g.beats[0].bar_pos, 1);
        assert_eq!(g.beats[4].bar_pos, 1);
        assert_eq!(g.beats[5].bar_pos, 2);
        assert!((g.median_bpm().unwrap() - 120.0).abs() < 1e-9);
    }

    #[test]
    fn feature_sets() {
        let s = FeatureSet::from([Feature::Tempo, Feature::Key]);
        assert!(s.contains(Feature::Tempo) && !s.contains(Feature::Beatgrid));
        assert_eq!(s.iter().count(), 2);
        assert!(s.intersect(Feature::Beatgrid.into()).is_empty());
    }
}
