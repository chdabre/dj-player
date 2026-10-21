//! Binary encoding of a waveform blob (detail or overview), version 1. Little-endian.
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 4 | magic `DJWF` |
//! | 4 | 2 | format version (1) |
//! | 6 | 2 | header length in bytes, signal table included; the data starts here |
//! | 8 | 1 | layout: 0 = detail (timed entries), 1 = overview (columns spread over the track) |
//! | 9 | 1 | value curve: 1 = `round(255·√peak)` |
//! | 10 | 1 | filter: 1 = Linkwitz-Riley 4th order, per channel, peak of max(L, R) |
//! | 11 | 1 | provenance: 0 own, 1 Rekordbox PWV7, 2 PWV6, 3 PWV5, 4 PWV3 |
//! | 12 | 4 | entry rate in millihertz (150 /s = 150 000); 0 for an overview |
//! | 16 | 4 | entry count |
//! | 20 | 8 | track duration, seconds (f64) |
//! | 28 | 4 | low-band smoothing half-width, seconds (f32) |
//! | 32 | 1 | signal count N |
//! | 33 | 3 | reserved, 0 |
//! | 36 | 12·N | signal table: kind (u8), 3 reserved bytes, lower and upper edge in Hz (f32; 0 = open) |
//! | header length | count·N | data, entry-major: N bytes per entry, in signal-table order |
//!
//! Kinds: 1 low, 2 mid, 3 high; 16 + i for stem i (later). A reader takes the signals it
//! knows and skips the rest, and starts the data at the header length, so later versions can
//! add signals and header fields without breaking older readers.

use crate::compute::{ENTRIES_PER_SECOND, Entry, Overview, Params, Provenance, Waveform};

pub const MAGIC: [u8; 4] = *b"DJWF";
pub const VERSION: u16 = 1;

const FIXED_HEADER: usize = 36;
const SIGNAL_DESC: usize = 12;

const LAYOUT_DETAIL: u8 = 0;
const LAYOUT_OVERVIEW: u8 = 1;
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

/// The bands of a waveform this crate computes, in storage order.
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
    provenance: Provenance,
    rate_mhz: u32,
    duration: f64,
    low_smoothing: f32,
    signals: Vec<Signal>,
}

fn encode(h: &Header, entries: &[Entry]) -> Vec<u8> {
    let header_len = FIXED_HEADER + SIGNAL_DESC * h.signals.len();
    let mut out = Vec::with_capacity(header_len + 3 * entries.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(header_len as u16).to_le_bytes());
    out.extend_from_slice(&[
        h.layout,
        CURVE_SQRT,
        FILTER_LR4_PEAK,
        provenance_code(h.provenance),
    ]);
    out.extend_from_slice(&h.rate_mhz.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&h.duration.to_le_bytes());
    out.extend_from_slice(&h.low_smoothing.to_le_bytes());
    out.extend_from_slice(&[h.signals.len() as u8, 0, 0, 0]);
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
    let header_len = usize::from(u16_at(6)?);
    let [layout, curve, filter, provenance] = bytes(8, 4)?.try_into().unwrap();
    if curve != CURVE_SQRT {
        return Err(DecodeError::Unsupported("value curve"));
    }
    if filter != FILTER_LR4_PEAK {
        return Err(DecodeError::Unsupported("filter"));
    }
    let rate_mhz = u32_at(12)?;
    let count = u32_at(16)? as usize;
    let duration = f64::from_le_bytes(bytes(20, 8)?.try_into().unwrap());
    let low_smoothing = f32_at(28)?;
    let n = usize::from(bytes(32, 1)?[0]);
    if header_len < FIXED_HEADER + SIGNAL_DESC * n {
        return Err(DecodeError::Truncated);
    }
    let signals = (0..n)
        .map(|i| {
            let at = FIXED_HEADER + SIGNAL_DESC * i;
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
    let data = bytes(header_len, count * n)?;
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
        provenance: provenance_from_code(provenance)?,
        rate_mhz,
        duration,
        low_smoothing,
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
    }
}

fn provenance_from_code(c: u8) -> Result<Provenance, DecodeError> {
    Ok(match c {
        0 => Provenance::Own,
        1 => Provenance::RekordboxPwv7,
        2 => Provenance::RekordboxPwv6,
        3 => Provenance::RekordboxPwv5,
        4 => Provenance::RekordboxPwv3,
        _ => return Err(DecodeError::Unsupported("provenance")),
    })
}

impl Waveform {
    /// Encode as a detail blob.
    pub fn to_blob(&self) -> Vec<u8> {
        let header = Header {
            layout: LAYOUT_DETAIL,
            provenance: self.provenance,
            rate_mhz: self.params.entries_per_second * 1000,
            duration: self.duration,
            low_smoothing: self.params.low_smoothing,
            signals: bands(&self.params).to_vec(),
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
            duration: h.duration,
            entries,
        })
    }
}

impl Overview {
    /// Encode as an overview blob; `params` are those of the detail it was reduced from.
    pub fn to_blob(&self, params: &Params, provenance: Provenance) -> Vec<u8> {
        let header = Header {
            layout: LAYOUT_OVERVIEW,
            provenance,
            rate_mhz: 0,
            duration: self.duration,
            low_smoothing: params.low_smoothing,
            signals: bands(params).to_vec(),
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
        assert_eq!(blob[12..16], 150_000u32.to_le_bytes());
        assert_eq!(Waveform::from_blob(&blob).unwrap(), wf);
        for p in [Provenance::RekordboxPwv7, Provenance::RekordboxPwv3] {
            let wf = Waveform {
                provenance: p,
                ..sample()
            };
            assert_eq!(Waveform::from_blob(&wf.to_blob()).unwrap(), wf);
        }
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

    #[test]
    fn extra_signals_and_header_fields_are_skipped() {
        // A later writer: a stem signal between the bands and 8 bytes of new header fields.
        let wf = sample();
        let mut h = Header {
            layout: LAYOUT_DETAIL,
            provenance: Provenance::Own,
            rate_mhz: 150_000,
            duration: wf.duration,
            low_smoothing: wf.params.low_smoothing,
            signals: bands(&wf.params).to_vec(),
        };
        h.signals.insert(
            1,
            Signal {
                kind: SignalKind::Stem(0),
                low_hz: 0.0,
                high_hz: 0.0,
            },
        );
        let three = encode(&h, &wf.entries);
        // Re-lay the data with a fourth byte per entry and grow the header.
        let n = h.signals.len();
        let header_len = FIXED_HEADER + SIGNAL_DESC * n;
        let mut blob = three[..header_len].to_vec();
        blob[6..8].copy_from_slice(&((header_len + 8) as u16).to_le_bytes());
        blob.extend_from_slice(&[0xAA; 8]);
        for e in &wf.entries {
            blob.extend_from_slice(&[e.low, 99, e.mid, e.high]);
        }
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
        for cut in [5, 20, 40, blob.len() - 1] {
            assert_eq!(
                Waveform::from_blob(&blob[..cut]).unwrap_err(),
                DecodeError::Truncated,
                "{cut}"
            );
        }
        let mut v2 = blob.clone();
        v2[4] = 2;
        assert_eq!(
            Waveform::from_blob(&v2).unwrap_err(),
            DecodeError::UnsupportedVersion(2)
        );
        let mut no_high = blob.clone();
        no_high[FIXED_HEADER + 2 * SIGNAL_DESC] = 99;
        assert_eq!(
            Waveform::from_blob(&no_high).unwrap_err(),
            DecodeError::MissingBand(SignalKind::High)
        );
    }
}
