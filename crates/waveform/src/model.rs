//! The waveform data types: entries and their level curve, the parameters that made them,
//! where they came from and where entry 0 sits, the detail and the overview.

/// Detail entries per second (Rekordbox's PWV7 rate, so its 3-band tags import 1:1).
pub const ENTRIES_PER_SECOND: u32 = 150;

/// Overview columns for a whole track, whatever its length.
pub const OVERVIEW_COLUMNS: usize = 1200;

/// One entry: the encoded peak of each band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Entry {
    pub low: u8,
    pub mid: u8,
    pub high: u8,
}

impl Entry {
    pub const SILENT: Entry = Entry {
        low: 0,
        mid: 0,
        high: 0,
    };

    /// Band-wise maximum.
    pub fn max(self, other: Entry) -> Entry {
        Entry {
            low: self.low.max(other.low),
            mid: self.mid.max(other.mid),
            high: self.high.max(other.high),
        }
    }

    /// Combined level of the three bands, encoded like a band: the power sum of the linear
    /// peaks, `√(low² + mid² + high²)`, as if the bands were uncorrelated. Single colour and
    /// RGB modes draw this height.
    pub fn combined(self) -> u8 {
        let sum: f32 = [self.low, self.mid, self.high]
            .iter()
            .map(|&v| level(v).powi(2))
            .sum();
        encode_level(sum.sqrt())
    }
}

/// Encode a linear peak (1.0 = full scale) on the square-root curve: −40 dB still gets 25
/// steps instead of 2–3.
pub fn encode_level(peak: f32) -> u8 {
    (255.0 * peak.clamp(0.0, 1.0).sqrt()).round() as u8
}

/// The linear peak an encoded value stands for (inverse of [`encode_level`]).
pub fn level(value: u8) -> f32 {
    (f32::from(value) / 255.0).powi(2)
}

/// Parameters that change the computed data; all of them go into the blob header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub entries_per_second: u32,
    /// Crossover between low and mid, Hz.
    pub low_mid_hz: f32,
    /// Crossover between mid and high, Hz.
    pub mid_high_hz: f32,
    /// Half-width of the centred max applied to the low band, seconds (0 = off).
    pub low_smoothing: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            entries_per_second: ENTRIES_PER_SECOND,
            low_mid_hz: 250.0,
            mid_high_hz: 2500.0,
            low_smoothing: 0.012,
        }
    }
}

impl Params {
    /// Low-band smoothing radius in entries.
    pub fn low_smoothing_entries(&self) -> usize {
        (f64::from(self.low_smoothing) * f64::from(self.entries_per_second)).round() as usize
    }
}

/// Where a waveform came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provenance {
    /// Computed by [`Builder`](crate::Builder) / [`compute`](crate::compute()).
    #[default]
    Own,
    /// Imported from Rekordbox's 3-band detail tag (PWV7, `.2EX`); placeholder only.
    RekordboxPwv7,
    /// Imported from Rekordbox's 3-band overview tag (PWV6, `.2EX`); placeholder only.
    RekordboxPwv6,
    /// Approximated from Rekordbox's RGB detail tag (PWV5, `.EXT`); placeholder only.
    RekordboxPwv5,
    /// Rekordbox's mono blue detail (PWV3, `.EXT`), copied into all bands; placeholder only.
    RekordboxPwv3,
    /// A source this version does not know (a later writer's code); treated as an import.
    Unknown,
}

/// Where entry 0 of a waveform sits, so waveforms from different decoders line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    /// Not known (e.g. an import whose convention is unverified).
    Unknown,
    /// The first frame after gapless trimming: the engine's timeline, what
    /// [`Builder`](crate::Builder) sees from our decoder.
    #[default]
    Trimmed,
    /// The decoder's first frame without gapless trimming (Rekordbox's convention, which
    /// starts an MP3 `start_trim` frames earlier).
    Untrimmed,
}

/// Entry 0's origin and the file's gapless trim, so a consumer can move between the trimmed
/// and the untrimmed timeline: the untrimmed one starts `start_trim / file_rate` seconds
/// earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timeline {
    pub origin: Origin,
    /// Frames the decoder drops from the start for gapless playback, at the file's rate
    /// (`engine_media::TrackInfo::start_trim`); 0 if none or unknown.
    pub start_trim: u32,
    /// The file's own sample rate; 0 if unknown.
    pub file_rate: u32,
}

impl Timeline {
    /// Seconds from the untrimmed start to the trimmed one, if known.
    pub fn trim_seconds(&self) -> Option<f64> {
        (self.file_rate > 0).then(|| f64::from(self.start_trim) / f64::from(self.file_rate))
    }
}

/// A track's detail waveform.
#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    pub params: Params,
    pub provenance: Provenance,
    /// Where entry 0 sits on the file's timeline.
    pub timeline: Timeline,
    /// Track length in seconds (frames ÷ the file's rate).
    pub duration: f64,
    /// Entry `k` covers `[k, k + 1) / entries_per_second` seconds.
    pub entries: Vec<Entry>,
}

impl Waveform {
    /// The whole-track overview, reduced from this detail.
    pub fn overview(&self) -> Overview {
        Overview {
            duration: self.duration,
            timeline: self.timeline,
            columns: overview(&self.entries, OVERVIEW_COLUMNS),
        }
    }

    /// Index of the entry that covers `seconds` (may be past the end).
    pub fn entry_at(&self, seconds: f64) -> isize {
        (seconds * f64::from(self.params.entries_per_second)).floor() as isize
    }
}

/// A whole-track overview: `columns` spread evenly over `duration`.
#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub duration: f64,
    pub timeline: Timeline,
    pub columns: Vec<Entry>,
}

/// Reduce `detail` to `columns` columns (fewer if the detail is shorter), each the band-wise
/// max of the entries it covers. Column `c` covers entries `[c·n/columns, (c+1)·n/columns)`.
pub fn overview(detail: &[Entry], columns: usize) -> Vec<Entry> {
    let n = detail.len();
    let columns = columns.min(n);
    (0..columns)
        .map(|c| {
            detail[c * n / columns..(c + 1) * n / columns]
                .iter()
                .fold(Entry::SILENT, |a, &e| a.max(e))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_is_a_max_reduction_of_the_detail() {
        let mut seed = 0x9e37_79b9u32;
        let mut byte = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        };
        for n in [0, 1, 7, 1199, 1200, 1201, 45_000, 45_123] {
            let detail: Vec<Entry> = (0..n)
                .map(|_| Entry {
                    low: byte(),
                    mid: byte(),
                    high: byte(),
                })
                .collect();
            let ov = overview(&detail, OVERVIEW_COLUMNS);
            assert_eq!(ov.len(), n.min(OVERVIEW_COLUMNS));
            // Every entry falls in exactly one column, and each column is its entries' max.
            let mut covered = 0;
            for (c, col) in ov.iter().enumerate() {
                let from = c * n / ov.len();
                let to = (c + 1) * n / ov.len();
                assert_eq!(from, covered);
                assert!(to > from);
                covered = to;
                let want = Entry {
                    low: detail[from..to].iter().map(|e| e.low).max().unwrap(),
                    mid: detail[from..to].iter().map(|e| e.mid).max().unwrap(),
                    high: detail[from..to].iter().map(|e| e.high).max().unwrap(),
                };
                assert_eq!(*col, want);
            }
            assert_eq!(covered, n);
        }
    }

    #[test]
    fn combined_level() {
        let e = |low, mid, high| Entry { low, mid, high };
        assert_eq!(e(0, 0, 0).combined(), 0);
        assert_eq!(e(200, 0, 0).combined(), 200);
        assert_eq!(e(0, 0, 200).combined(), 200);
        // Two equal bands: +3 dB in power, 2^(1/4) in encoded value.
        assert_eq!(e(128, 128, 0).combined(), 152);
        assert_eq!(e(255, 255, 255).combined(), 255);
    }
}
