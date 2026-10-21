//! Waveform data: three bands of peak amplitude per entry, computed from decoded audio.
//!
//! - **Bands:** low < 250 Hz, mid 250 Hz–2.5 kHz, high > 2.5 kHz, split with 4th-order
//!   Linkwitz-Riley filters (two cascaded Butterworth biquads), run per channel.
//! - **Value:** per entry and band, the peak |x| over both channels (mono). The low band is
//!   smoothed with a centred max over ±12 ms, so a 30–50 Hz tone, whose period is longer than
//!   an entry, does not ripple into a comb.
//! - **Encoding:** one `u8` per band, `round(255·√min(peak, 1))`; absolute levels, never
//!   normalised.
//! - **Time base:** seconds. Entry `k` covers `[k, k + 1) / 150` s from the first decoded
//!   frame, so the data does not depend on the file's or the device's sample rate.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

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
    /// Computed by [`compute`].
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
}

/// A track's detail waveform.
#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    pub params: Params,
    pub provenance: Provenance,
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

/// Compute the detail waveform of interleaved stereo at the file's own `sample_rate`.
pub fn compute(stereo: &[f32], sample_rate: u32, params: &Params) -> Waveform {
    let frames = stereo.len() / 2;
    let sr = u64::from(sample_rate);
    let eps = u64::from(params.entries_per_second);
    // Entry k starts at frame floor(k·sr/eps); there are as many entries as start inside
    // the track.
    let count = (frames as u64 * eps).div_ceil(sr) as usize;
    let start = |k: usize| (k as u64 * sr / eps) as usize;

    let mut split = [
        Splitter::new(params, sample_rate),
        Splitter::new(params, sample_rate),
    ];
    let mut peaks = Vec::with_capacity(count);
    for k in 0..count {
        let mut peak = [0.0f32; 3];
        for frame in stereo[2 * start(k)..2 * start(k + 1).min(frames)].chunks_exact(2) {
            for (ch, &x) in frame.iter().enumerate() {
                let bands = split[ch].process(f64::from(x));
                for (p, b) in peak.iter_mut().zip(bands) {
                    *p = p.max(b.abs() as f32);
                }
            }
        }
        peaks.push(peak);
        split.iter_mut().for_each(Splitter::flush);
    }

    let r = params.low_smoothing_entries();
    let entries = (0..count)
        .map(|k| {
            let low = peaks[k.saturating_sub(r)..(k + r + 1).min(count)]
                .iter()
                .fold(0.0f32, |a, p| a.max(p[0]));
            Entry {
                low: encode_level(low),
                mid: encode_level(peaks[k][1]),
                high: encode_level(peaks[k][2]),
            }
        })
        .collect();

    Waveform {
        params: *params,
        provenance: Provenance::Own,
        duration: frames as f64 / f64::from(sample_rate),
        entries,
    }
}

/// One channel's 3-way Linkwitz-Riley crossover.
struct Splitter {
    low: [Biquad; 2],
    mid_hp: [Biquad; 2],
    mid_lp: [Biquad; 2],
    high: [Biquad; 2],
}

impl Splitter {
    fn new(p: &Params, sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate);
        let lp = |fc: f32| [Biquad::lowpass(fc.into(), sr); 2];
        let hp = |fc: f32| [Biquad::highpass(fc.into(), sr); 2];
        Splitter {
            low: lp(p.low_mid_hz),
            mid_hp: hp(p.low_mid_hz),
            mid_lp: lp(p.mid_high_hz),
            high: hp(p.mid_high_hz),
        }
    }

    fn process(&mut self, x: f64) -> [f64; 3] {
        let run = |[a, b]: &mut [Biquad; 2], x| b.process(a.process(x));
        let low = run(&mut self.low, x);
        let mid = run(&mut self.mid_lp, run(&mut self.mid_hp, x));
        let high = run(&mut self.high, x);
        [low, mid, high]
    }

    fn flush(&mut self) {
        for b in [
            &mut self.low,
            &mut self.mid_hp,
            &mut self.mid_lp,
            &mut self.high,
        ] {
            b.iter_mut().for_each(Biquad::flush);
        }
    }
}

/// Second-order Butterworth section (RBJ cookbook, Q = 1/√2), transposed direct form II.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(fc: f64, sr: f64, high: bool) -> Self {
        // Keep the corner below Nyquist for very low file rates.
        let w0 = TAU * fc.min(0.45 * sr) / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = if high {
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0]
        } else {
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0]
        };
        Biquad {
            b: b.map(|v| v / a0),
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    fn lowpass(fc: f64, sr: f64) -> Self {
        Self::new(fc, sr, false)
    }

    fn highpass(fc: f64, sr: f64) -> Self {
        Self::new(fc, sr, true)
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    /// Zero a state that has decayed to nothing. A tail left to decay in silence turns
    /// subnormal, which is many times slower on x86. Called once per entry; within one
    /// entry (≤ 7 ms) a state at 1e-30 cannot reach the subnormal range.
    fn flush(&mut self) {
        for z in &mut self.z {
            if z.abs() < 1e-30 {
                *z = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` of a sine at `hz` and `amp` from `from` seconds on, stereo.
    fn tone(sr: u32, hz: f64, amp: f64, from: f64, seconds: f64) -> Vec<f32> {
        let srf = f64::from(sr);
        (0..(seconds * srf) as usize)
            .flat_map(|i| {
                let t = i as f64 / srf;
                let x = if t >= from {
                    (amp * (TAU * hz * t).sin()) as f32
                } else {
                    0.0
                };
                [x, x]
            })
            .collect()
    }

    fn middle(wf: &Waveform) -> Entry {
        wf.entries[wf.entries.len() / 2]
    }

    #[test]
    fn a_tone_in_each_band_lands_in_that_band() {
        let target = encode_level(0.5);
        for sr in [44_100, 48_000] {
            for (hz, band) in [(50.0, 0), (60.0, 0), (800.0, 1), (7100.0, 2)] {
                let wf = compute(&tone(sr, hz, 0.5, 0.0, 2.0), sr, &Params::default());
                // Past the filters' settling, every entry looks the same.
                for e in &wf.entries[30..wf.entries.len() - 30] {
                    let v = [e.low, e.mid, e.high];
                    let got = i32::from(v[band]);
                    assert!(
                        (got - i32::from(target)).abs() <= 3,
                        "{hz} Hz at {sr}: band {band} = {got}, want {target} ({v:?})"
                    );
                    for (other, &x) in v.iter().enumerate().filter(|&(b, _)| b != band) {
                        assert!(
                            x < target / 4,
                            "{hz} Hz at {sr}: band {other} = {x} next to {target}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn low_band_smoothing_removes_the_comb() {
        // 40 Hz has a 25 ms period: without smoothing some 6.7 ms entries miss the crest.
        let audio = tone(48_000, 40.0, 0.5, 0.0, 2.0);
        let spread = |p: &Params| {
            let wf = compute(&audio, 48_000, p);
            let lows: Vec<u8> = wf.entries[30..270].iter().map(|e| e.low).collect();
            lows.iter().max().unwrap() - lows.iter().min().unwrap()
        };
        let raw = Params {
            low_smoothing: 0.0,
            ..Params::default()
        };
        assert!(spread(&raw) > 10, "unsmoothed {}", spread(&raw));
        assert!(spread(&Params::default()) <= 2);
    }

    #[test]
    fn level_curve_is_square_root() {
        assert_eq!(encode_level(0.0), 0);
        assert_eq!(encode_level(1.0), 255);
        assert_eq!(encode_level(4.0), 255);
        assert_eq!(encode_level(-1.0), 0);
        assert_eq!(encode_level(0.25), 128); // −12 dB → half height
        assert_eq!(encode_level(0.01), 26); // −40 dB
        assert_eq!(encode_level(0.001), 8); // −60 dB
        for v in 0..=255u8 {
            assert_eq!(encode_level(level(v)), v);
        }
        // A full-scale sine computes to full scale, a quarter-scale one to half; within the
        // mid band's passband loss (two LR4 skirts, −0.2 dB at 1 kHz).
        for (amp, want) in [(1.0, 255), (0.25, 128)] {
            let wf = compute(
                &tone(48_000, 1000.0, amp, 0.0, 1.0),
                48_000,
                &Params::default(),
            );
            assert!(
                middle(&wf).mid.abs_diff(want) <= 4,
                "{amp}: {:?}",
                middle(&wf)
            );
        }
    }

    #[test]
    fn entries_are_timed_in_seconds_whatever_the_rate() {
        for sr in [22_050, 44_100, 48_000, 96_000] {
            let wf = compute(&tone(sr, 1000.0, 0.5, 1.0, 10.0), sr, &Params::default());
            assert_eq!(wf.entries.len(), 1500, "{sr}");
            assert_eq!(wf.duration, 10.0);
            let onset = wf.entries.iter().position(|e| e.mid > 64).unwrap();
            assert_eq!(onset, 150, "{sr}");
            assert_eq!(wf.entry_at(1.0), 150);
        }
        // A partial last entry still counts.
        let wf = compute(&vec![0.0; 2 * 44_101], 44_100, &Params::default());
        assert_eq!(wf.entries.len(), 151);
    }

    #[test]
    fn silence_is_zero() {
        let wf = compute(&vec![0.0; 2 * 48_000], 48_000, &Params::default());
        assert!(wf.entries.iter().all(|&e| e == Entry::SILENT));
        assert!(compute(&[], 48_000, &Params::default()).entries.is_empty());
    }

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
