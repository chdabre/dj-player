//! Binary encoding of a waveform blob (detail or overview), format version 1. Little-endian.
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 4 | magic `DJWF` |
//! | 4 | 2 | format version (1) |
//! | 6 | 2 | data offset: header length in bytes, signal table included |
//! | 8 | 2 | signal table offset: length of the fixed fields (48 in version 1) |
//! | 10 | 1 | signal descriptor size (12 in version 1) |
//! | 11 | 1 | signal count N |
//! | 12 | 1 | layout: 0 = detail (timed entries), 1 = overview (columns spread over the track) |
//! | 13 | 1 | value curve: 0 = imported / unknown, 1 = `round(255·√peak)` |
//! | 14 | 1 | filter: 0 = imported / unknown, 1 = Linkwitz-Riley 4th order per channel, peak of max(L, R) |
//! | 15 | 1 | provenance: 0 own, 1 Rekordbox PWV7, 2 PWV6, 3 PWV5, 4 PWV3, 255 unknown |
//! | 16 | 4 | entry rate in millihertz (150 /s = 150 000); 0 for an overview |
//! | 20 | 4 | entry count |
//! | 24 | 8 | track duration, seconds (f64) |
//! | 32 | 4 | low-band smoothing half-width, seconds (f32) |
//! | 36 | 1 | timeline origin of entry 0: 0 unknown, 1 gapless-trimmed (ours), 2 untrimmed (Rekordbox) |
//! | 37 | 3 | reserved, 0 |
//! | 40 | 4 | gapless start trim, frames at the file's rate |
//! | 44 | 4 | the file's sample rate, Hz (0 = unknown) |
//! | 48 | 12·N | signal table: kind (u8), 3 reserved bytes, lower and upper edge in Hz (f32; 0 = open) |
//! | data offset | count·N | data, entry-major: N bytes per entry, in signal-table order |
//!
//! Signal kinds: 1 low, 2 mid, 3 high; 16 + i for stem i (later).
//!
//! A blob that is not our own computation (provenance other than own) is written with value
//! curve and filter 0: an import must not claim our curve or crossovers.
//!
//! **Versioning.** The version changes only on a breaking change, one an older reader would
//! misread; a reader rejects any version it does not know. Additive changes keep the version:
//!
//! - a new header field goes after the existing fixed fields (the signal table offset grows),
//! - a new per-signal field goes at the end of each descriptor (the descriptor size grows),
//! - a new signal kind is a new row; readers skip kinds they do not know,
//! - a new provenance or timeline-origin code; readers take codes they do not know as
//!   unknown,
//!
//! and readers locate the signal table, each descriptor and the data through the recorded
//! offsets and sizes, never through constants, so they read what they know and skip the
//! rest.

use crate::compute::{
    ENTRIES_PER_SECOND, Entry, Origin, Overview, Params, Provenance, Timeline, Waveform,
};

pub const MAGIC: [u8; 4] = *b"DJWF";
pub const VERSION: u16 = 1;

/// Fixed fields written by this version; the signal table starts here.
const FIXED_HEADER: usize = 48;
const SIGNAL_DESC: usize = 12;

const LAYOUT_DETAIL: u8 = 0;
const LAYOUT_OVERVIEW: u8 = 1;
const UNKNOWN: u8 = 0;
const CURVE_SQRT: u8 = 1;
const FILTER_LR4_PEAK: u8 = 1;

/// What one signal of the blob holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Low,
    Mid,
    High,
    /// A stem (drums, bass, ...), later.
    Stem(u8),
    /// A kind this version does not know; skipped on read.
    Unknown(u8),
}

impl SignalKind {
    fn code(self) -> u8 {
        match self {
            SignalKind::Low => 1,
            SignalKind::Mid => 2,
            SignalKind::High => 3,
            SignalKind::Stem(i) => 16 + i,
            SignalKind::Unknown(c) => c,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            1 => SignalKind::Low,
            2 => SignalKind::Mid,
            3 => SignalKind::High,
            16..=31 => SignalKind::Stem(c - 16),
            _ => SignalKind::Unknown(c),
        }
    }
}

/// One row of the signal table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signal {
    pub kind: SignalKind,
    /// Band edges in Hz; 0 = open (no high-pass / no low-pass).
    pub low_hz: f32,
    pub high_hz: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    NotAWaveform,
    UnsupportedVersion(u16),
    Truncated,
    WrongLayout,
    Unsupported(&'static str),
    MissingBand(SignalKind),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotAWaveform => write!(f, "not a waveform blob"),
            DecodeError::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            DecodeError::Truncated => write!(f, "truncated"),
            DecodeError::WrongLayout => write!(f, "detail and overview mixed up"),
            DecodeError::Unsupported(what) => write!(f, "unsupported {what}"),
            DecodeError::MissingBand(k) => write!(f, "no {k:?} band"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// The bands of a waveform, in storage order.
fn bands(p: &Params) -> [Signal; 3] {
    [
        Signal {
            kind: SignalKind::Low,
            low_hz: 0.0,
            high_hz: p.low_mid_hz,
        },
        Signal {
            kind: SignalKind::Mid,
            low_hz: p.low_mid_hz,
            high_hz: p.mid_high_hz,
        },
        Signal {
            kind: SignalKind::High,
            low_hz: p.mid_high_hz,
            high_hz: 0.0,
        },
    ]
}

struct Header {
    layout: u8,
    curve: u8,
    filter: u8,
    provenance: Provenance,
    rate_mhz: u32,
    duration: f64,
    low_smoothing: f32,
    timeline: Timeline,
    signals: Vec<Signal>,
}

impl Header {
    fn new(layout: u8, params: &Params, provenance: Provenance, timeline: Timeline) -> Self {
        let own = provenance == Provenance::Own;
        let code = |c| if own { c } else { UNKNOWN };
        Header {
            layout,
            curve: code(CURVE_SQRT),
            filter: code(FILTER_LR4_PEAK),
            provenance,
            rate_mhz: 0,
            duration: 0.0,
            low_smoothing: params.low_smoothing,
            timeline,
            signals: bands(params).to_vec(),
        }
    }
}

fn encode(h: &Header, entries: &[Entry]) -> Vec<u8> {
    let header_len = FIXED_HEADER + SIGNAL_DESC * h.signals.len();
    let mut out = Vec::with_capacity(header_len + 3 * entries.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(header_len as u16).to_le_bytes());
    out.extend_from_slice(&(FIXED_HEADER as u16).to_le_bytes());
    out.extend_from_slice(&[SIGNAL_DESC as u8, h.signals.len() as u8]);
    out.extend_from_slice(&[h.layout, h.curve, h.filter, provenance_code(h.provenance)]);
    out.extend_from_slice(&h.rate_mhz.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&h.duration.to_le_bytes());
    out.extend_from_slice(&h.low_smoothing.to_le_bytes());
    out.extend_from_slice(&[origin_code(h.timeline.origin), 0, 0, 0]);
    out.extend_from_slice(&h.timeline.start_trim.to_le_bytes());
    out.extend_from_slice(&h.timeline.file_rate.to_le_bytes());
    debug_assert_eq!(out.len(), FIXED_HEADER);
    for s in &h.signals {
        out.extend_from_slice(&[s.kind.code(), 0, 0, 0]);
        out.extend_from_slice(&s.low_hz.to_le_bytes());
        out.extend_from_slice(&s.high_hz.to_le_bytes());
    }
    for e in entries {
        out.extend_from_slice(&[e.low, e.mid, e.high]);
    }
    out
}

fn decode(blob: &[u8]) -> Result<(Header, Vec<Entry>), DecodeError> {
    let bytes = |at: usize, n: usize| blob.get(at..at + n).ok_or(DecodeError::Truncated);
    let u16_at = |at| Ok(u16::from_le_bytes(bytes(at, 2)?.try_into().unwrap()));
    let u32_at = |at| Ok(u32::from_le_bytes(bytes(at, 4)?.try_into().unwrap()));
    let f32_at = |at| Ok(f32::from_le_bytes(bytes(at, 4)?.try_into().unwrap()));
    if blob.get(..4) != Some(&MAGIC[..]) {
        return Err(DecodeError::NotAWaveform);
    }
    let version = u16_at(4)?;
    if version != VERSION {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    let data_at = usize::from(u16_at(6)?);
    let table_at = usize::from(u16_at(8)?);
    let [desc_len, n] = bytes(10, 2)?.try_into().unwrap();
    let (desc_len, n) = (usize::from(desc_len), usize::from(n));
    // Every version-1 field must be there; later additions may make both longer.
    if table_at < FIXED_HEADER || desc_len < SIGNAL_DESC || data_at < table_at + desc_len * n {
        return Err(DecodeError::Truncated);
    }
    let [layout, curve, filter, provenance] = bytes(12, 4)?.try_into().unwrap();
    if curve > CURVE_SQRT {
        return Err(DecodeError::Unsupported("value curve"));
    }
    if filter > FILTER_LR4_PEAK {
        return Err(DecodeError::Unsupported("filter"));
    }
    let rate_mhz = u32_at(16)?;
    let count = u32_at(20)? as usize;
    let duration = f64::from_le_bytes(bytes(24, 8)?.try_into().unwrap());
    let low_smoothing = f32_at(32)?;
    let timeline = Timeline {
        origin: origin_from_code(bytes(36, 1)?[0]),
        start_trim: u32_at(40)?,
        file_rate: u32_at(44)?,
    };
    let signals = (0..n)
        .map(|i| {
            let at = table_at + desc_len * i;
            Ok(Signal {
                kind: SignalKind::from_code(bytes(at, 1)?[0]),
                low_hz: f32_at(at + 4)?,
                high_hz: f32_at(at + 8)?,
            })
        })
        .collect::<Result<Vec<_>, DecodeError>>()?;
    let column = |kind| {
        signals
            .iter()
            .position(|s| s.kind == kind)
            .ok_or(DecodeError::MissingBand(kind))
    };
    let (low, mid, high) = (
        column(SignalKind::Low)?,
        column(SignalKind::Mid)?,
        column(SignalKind::High)?,
    );
    let data = bytes(data_at, count * n)?;
    let entries = data
        .chunks_exact(n)
        .map(|e| Entry {
            low: e[low],
            mid: e[mid],
            high: e[high],
        })
        .collect();
    let header = Header {
        layout,
        curve,
        filter,
        provenance: provenance_from_code(provenance),
        rate_mhz,
        duration,
        low_smoothing,
        timeline,
        signals,
    };
    Ok((header, entries))
}

fn provenance_code(p: Provenance) -> u8 {
    match p {
        Provenance::Own => 0,
        Provenance::RekordboxPwv7 => 1,
        Provenance::RekordboxPwv6 => 2,
        Provenance::RekordboxPwv5 => 3,
        Provenance::RekordboxPwv3 => 4,
        Provenance::Unknown => 255,
    }
}

/// Codes this version does not know read as unknown: a later writer may add sources.
fn provenance_from_code(c: u8) -> Provenance {
    match c {
        0 => Provenance::Own,
        1 => Provenance::RekordboxPwv7,
        2 => Provenance::RekordboxPwv6,
        3 => Provenance::RekordboxPwv5,
        4 => Provenance::RekordboxPwv3,
        _ => Provenance::Unknown,
    }
}

fn origin_code(o: Origin) -> u8 {
    match o {
        Origin::Unknown => 0,
        Origin::Trimmed => 1,
        Origin::Untrimmed => 2,
    }
}

/// Codes this version does not know read as unknown, like provenance.
fn origin_from_code(c: u8) -> Origin {
    match c {
        1 => Origin::Trimmed,
        2 => Origin::Untrimmed,
        _ => Origin::Unknown,
    }
}

impl Waveform {
    /// Encode as a detail blob.
    pub fn to_blob(&self) -> Vec<u8> {
        let header = Header {
            rate_mhz: self.params.entries_per_second * 1000,
            duration: self.duration,
            ..Header::new(LAYOUT_DETAIL, &self.params, self.provenance, self.timeline)
        };
        encode(&header, &self.entries)
    }

    /// Decode a detail blob.
    pub fn from_blob(blob: &[u8]) -> Result<Waveform, DecodeError> {
        let (h, entries) = decode(blob)?;
        if h.layout != LAYOUT_DETAIL {
            return Err(DecodeError::WrongLayout);
        }
        if h.rate_mhz % 1000 != 0 || h.rate_mhz == 0 {
            return Err(DecodeError::Unsupported("entry rate"));
        }
        let edge = |kind, upper: bool| {
            h.signals
                .iter()
                .find(|s| s.kind == kind)
                .map(|s| if upper { s.high_hz } else { s.low_hz })
                .unwrap_or_default()
        };
        Ok(Waveform {
            params: Params {
                entries_per_second: h.rate_mhz / 1000,
                low_mid_hz: edge(SignalKind::Low, true),
                mid_high_hz: edge(SignalKind::High, false),
                low_smoothing: h.low_smoothing,
            },
            provenance: h.provenance,
            timeline: h.timeline,
            duration: h.duration,
            entries,
        })
    }
}

impl Overview {
    /// Encode as an overview blob; `params` are those of the detail it was reduced from.
    pub fn to_blob(&self, params: &Params, provenance: Provenance) -> Vec<u8> {
        let header = Header {
            duration: self.duration,
            ..Header::new(LAYOUT_OVERVIEW, params, provenance, self.timeline)
        };
        encode(&header, &self.columns)
    }

    /// Decode an overview blob.
    pub fn from_blob(blob: &[u8]) -> Result<Overview, DecodeError> {
        let (h, columns) = decode(blob)?;
        if h.layout != LAYOUT_OVERVIEW {
            return Err(DecodeError::WrongLayout);
        }
        Ok(Overview {
            duration: h.duration,
            timeline: h.timeline,
            columns,
        })
    }
}

/// Size in bytes of a detail blob for `seconds` of audio at the default rate.
pub fn detail_size(seconds: f64) -> usize {
    FIXED_HEADER + 3 * SIGNAL_DESC + 3 * (seconds * f64::from(ENTRIES_PER_SECOND)).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Waveform {
        Waveform {
            params: Params::default(),
            provenance: Provenance::Own,
            timeline: Timeline {
                origin: Origin::Trimmed,
                start_trim: 1105,
                file_rate: 44_100,
            },
            duration: 2.5,
            entries: (0..375u32)
                .map(|i| Entry {
                    low: i as u8,
                    mid: (i * 7) as u8,
                    high: (255 - i % 256) as u8,
                })
                .collect(),
        }
    }

    #[test]
    fn detail_round_trips() {
        let wf = sample();
        let blob = wf.to_blob();
        assert_eq!(blob.len(), detail_size(2.5));
        assert_eq!(&blob[..4], b"DJWF");
        assert_eq!(blob[4..6], [1, 0]);
        assert_eq!(blob[13..15], [CURVE_SQRT, FILTER_LR4_PEAK]);
        assert_eq!(blob[16..20], 150_000u32.to_le_bytes());
        assert_eq!(Waveform::from_blob(&blob).unwrap(), wf);
    }

    #[test]
    fn timeline_round_trips() {
        for timeline in [
            Timeline::default(),
            Timeline {
                origin: Origin::Untrimmed,
                start_trim: 1105,
                file_rate: 48_000,
            },
            Timeline {
                origin: Origin::Unknown,
                start_trim: 0,
                file_rate: 0,
            },
        ] {
            let wf = Waveform {
                timeline,
                ..sample()
            };
            assert_eq!(
                Waveform::from_blob(&wf.to_blob()).unwrap().timeline,
                timeline
            );
            let ov = wf.overview();
            let blob = ov.to_blob(&wf.params, wf.provenance);
            assert_eq!(Overview::from_blob(&blob).unwrap().timeline, timeline);
        }
        assert_eq!(sample().timeline.trim_seconds(), Some(1105.0 / 44_100.0));
        assert_eq!(Timeline::default().trim_seconds(), None);
    }

    #[test]
    fn imports_do_not_claim_our_curve_or_filter() {
        for p in [
            Provenance::RekordboxPwv7,
            Provenance::RekordboxPwv6,
            Provenance::RekordboxPwv5,
            Provenance::RekordboxPwv3,
            Provenance::Unknown,
        ] {
            let wf = Waveform {
                provenance: p,
                ..sample()
            };
            let blob = wf.to_blob();
            assert_eq!(blob[13..15], [UNKNOWN, UNKNOWN], "{p:?}");
            assert_eq!(Waveform::from_blob(&blob).unwrap(), wf);
            let blob = wf.overview().to_blob(&wf.params, p);
            assert_eq!(blob[13..15], [UNKNOWN, UNKNOWN], "{p:?}");
        }
        // Later codes for provenance and origin read as unknown.
        let mut blob = sample().to_blob();
        blob[15] = 77;
        blob[36] = 9;
        let wf = Waveform::from_blob(&blob).unwrap();
        assert_eq!(wf.provenance, Provenance::Unknown);
        assert_eq!(wf.timeline.origin, Origin::Unknown);
        let mut blob = sample().to_blob();
        blob[13] = 2;
        assert_eq!(
            Waveform::from_blob(&blob).unwrap_err(),
            DecodeError::Unsupported("value curve")
        );
    }

    #[test]
    fn overview_round_trips() {
        let wf = sample();
        let ov = wf.overview();
        let blob = ov.to_blob(&wf.params, wf.provenance);
        assert_eq!(blob.len(), FIXED_HEADER + 3 * SIGNAL_DESC + 3 * 375);
        assert_eq!(Overview::from_blob(&blob).unwrap(), ov);
        assert_eq!(
            Waveform::from_blob(&blob).unwrap_err(),
            DecodeError::WrongLayout
        );
        assert_eq!(
            Overview::from_blob(&wf.to_blob()).unwrap_err(),
            DecodeError::WrongLayout
        );
    }

    #[test]
    fn crossovers_are_recorded() {
        let wf = Waveform {
            params: Params {
                low_mid_hz: 600.0,
                mid_high_hz: 4000.0,
                low_smoothing: 0.0,
                ..Params::default()
            },
            ..sample()
        };
        assert_eq!(
            Waveform::from_blob(&wf.to_blob()).unwrap().params,
            wf.params
        );
    }

    /// A blob from a later writer of the same version: 8 bytes of new header fields, 4 new
    /// bytes per signal descriptor, a stem signal between the bands and 8 bytes of padding
    /// before the data. This version's reader skips all of it.
    #[test]
    fn additive_changes_keep_the_version() {
        let wf = sample();
        let ours = wf.to_blob();
        let extra_fields = 8;
        let desc = SIGNAL_DESC + 4;
        let signals = [
            (1u8, 0.0f32, 250.0f32),
            (16, 0.0, 0.0),
            (2, 250.0, 2500.0),
            (3, 2500.0, 0.0),
        ];
        let table_at = FIXED_HEADER + extra_fields;
        let data_at = table_at + desc * signals.len() + 8;
        let mut blob = ours[..FIXED_HEADER].to_vec();
        blob[6..8].copy_from_slice(&(data_at as u16).to_le_bytes());
        blob[8..10].copy_from_slice(&(table_at as u16).to_le_bytes());
        blob[10] = desc as u8;
        blob[11] = signals.len() as u8;
        blob.extend_from_slice(&[0xAA; 8]);
        for (kind, lo, hi) in signals {
            blob.extend_from_slice(&[kind, 0, 0, 0]);
            blob.extend_from_slice(&lo.to_le_bytes());
            blob.extend_from_slice(&hi.to_le_bytes());
            blob.extend_from_slice(&[0xBB; 4]);
        }
        blob.extend_from_slice(&[0xCC; 8]);
        for e in &wf.entries {
            blob.extend_from_slice(&[e.low, 99, e.mid, e.high]);
        }
        assert_eq!(&blob[4..6], &[1, 0]);
        assert_eq!(Waveform::from_blob(&blob).unwrap(), wf);
    }

    #[test]
    fn rejects_bad_blobs() {
        let blob = sample().to_blob();
        assert_eq!(
            Waveform::from_blob(b"RIFF....").unwrap_err(),
            DecodeError::NotAWaveform
        );
        assert_eq!(
            Waveform::from_blob(&[]).unwrap_err(),
            DecodeError::NotAWaveform
        );
        for cut in [5, 20, 40, 60, blob.len() - 1] {
            assert_eq!(
                Waveform::from_blob(&blob[..cut]).unwrap_err(),
                DecodeError::Truncated,
                "{cut}"
            );
        }
        // A breaking change bumps the version, and this reader refuses it.
        let mut v2 = blob.clone();
        v2[4] = 2;
        assert_eq!(
            Waveform::from_blob(&v2).unwrap_err(),
            DecodeError::UnsupportedVersion(2)
        );
        // A table offset inside the version-1 fields means fields are missing.
        let mut short = blob.clone();
        short[8..10].copy_from_slice(&40u16.to_le_bytes());
        assert_eq!(
            Waveform::from_blob(&short).unwrap_err(),
            DecodeError::Truncated
        );
        let mut no_high = blob.clone();
        no_high[FIXED_HEADER + 2 * SIGNAL_DESC] = 99;
        assert_eq!(
            Waveform::from_blob(&no_high).unwrap_err(),
            DecodeError::MissingBand(SignalKind::High)
        );
    }
}
