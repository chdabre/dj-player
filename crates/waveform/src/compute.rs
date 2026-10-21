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
//!   frame, so the data does not depend on the file's or the device's sample rate. The
//!   [`Timeline`] records whether that frame is the gapless-trimmed start (ours) or the
//!   untrimmed one (Rekordbox's), and the trim.
//! - **Incremental:** [`Builder`] takes the audio chunk by chunk during decode; [`compute`]
//!   is the same for a whole track in memory.

use std::collections::VecDeque;
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
    /// Computed by [`Builder`] / [`compute`].
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
    /// The first frame after gapless trimming: the engine's timeline, what [`Builder`] sees
    /// from our decoder.
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

/// Compute the detail waveform of a whole track at once: interleaved stereo (L, R, L, R,
/// ...; a trailing half frame is ignored) at `sample_rate`. A thin wrapper over [`Builder`].
pub fn compute(stereo: &[f32], sample_rate: u32, params: &Params) -> Waveform {
    let mut b = Builder::new(sample_rate, params);
    b.push(stereo);
    b.finish()
}

/// Computes a waveform incrementally, chunk by chunk, as a decoder or loader produces audio:
/// [`Builder::push`] each chunk of interleaved stereo, then [`Builder::finish`]. The result is
/// the same as [`compute`] on the whole track, byte for byte, however the audio is chunked.
///
/// The rate is that of the audio pushed (the device rate in the loader); entries are timed in
/// seconds, so it does not matter which. Memory stays constant apart from the entries: the
/// centred low-band max holds back the last `r` entries (±12 ms) until their right
/// neighbours exist.
pub struct Builder {
    params: Params,
    sample_rate: u32,
    split: [Splitter; 2],
    /// Frames pushed so far.
    frames: u64,
    /// Index of the entry being accumulated, and the frame the next one starts at.
    current: u64,
    next_start: u64,
    peak: [f32; 3],
    /// A left sample whose right sample is in the next chunk.
    half: Option<f32>,
    /// Raw peaks of the entries not yet final: indices `first..first + raw.len()`.
    raw: VecDeque<[f32; 3]>,
    first: usize,
    entries: Vec<Entry>,
}

impl Builder {
    pub fn new(sample_rate: u32, params: &Params) -> Self {
        let mut b = Builder {
            params: *params,
            sample_rate,
            split: [
                Splitter::new(params, sample_rate),
                Splitter::new(params, sample_rate),
            ],
            frames: 0,
            current: 0,
            next_start: 0,
            peak: [0.0; 3],
            half: None,
            raw: VecDeque::new(),
            first: 0,
            entries: Vec::new(),
        };
        b.next_start = b.start(1);
        b
    }

    /// First frame of entry `k`: `floor(k·rate/entries_per_second)`.
    fn start(&self, k: u64) -> u64 {
        k * u64::from(self.sample_rate) / u64::from(self.params.entries_per_second)
    }

    /// Add the next chunk of interleaved stereo. Chunks may split a frame.
    pub fn push(&mut self, stereo: &[f32]) {
        let mut rest = stereo;
        if let Some(l) = self.half.take() {
            let Some((&r, tail)) = rest.split_first() else {
                self.half = Some(l);
                return;
            };
            self.frame(l, r);
            rest = tail;
        }
        let mut frames = rest.chunks_exact(2);
        for f in &mut frames {
            self.frame(f[0], f[1]);
        }
        self.half = frames.remainder().first().copied();
    }

    fn frame(&mut self, l: f32, r: f32) {
        for (ch, x) in [l, r].into_iter().enumerate() {
            let bands = self.split[ch].process(f64::from(x));
            for (p, b) in self.peak.iter_mut().zip(bands) {
                *p = p.max(b.abs() as f32);
            }
        }
        self.frames += 1;
        if self.frames == self.next_start {
            self.end_entry();
        }
    }

    /// The current entry is complete.
    fn end_entry(&mut self) {
        self.raw.push_back(std::mem::take(&mut self.peak));
        self.current += 1;
        self.next_start = self.start(self.current + 1);
        self.split.iter_mut().for_each(Splitter::flush);
        // Entry k is final once entry k + r exists.
        let r = self.params.low_smoothing_entries();
        let done = self.first + self.raw.len();
        if done > r {
            self.finalize(done - 1 - r, done);
        }
    }

    /// Encode entry `k`, whose low band takes the max over `[k - r, k + r]` of the entries
    /// that exist (`< done`), and drop raw peaks no later entry needs.
    fn finalize(&mut self, k: usize, done: usize) {
        let r = self.params.low_smoothing_entries();
        let raw = |i: usize| self.raw[i - self.first];
        let low = (k.saturating_sub(r).max(self.first)..(k + r + 1).min(done))
            .map(|i| raw(i)[0])
            .fold(0.0f32, f32::max);
        let p = raw(k);
        self.entries.push(Entry {
            low: encode_level(low),
            mid: encode_level(p[1]),
            high: encode_level(p[2]),
        });
        while self.first + r < k + 1 {
            self.raw.pop_front();
            self.first += 1;
        }
    }

    /// Finish the track: a partly filled last entry counts, and the held-back entries are
    /// encoded. A trailing half frame is dropped.
    pub fn finish(mut self) -> Waveform {
        if self.frames > self.start(self.current) {
            self.end_entry();
        }
        let done = self.first + self.raw.len();
        for k in self.entries.len()..done {
            self.finalize(k, done);
        }
        Waveform {
            params: self.params,
            provenance: Provenance::Own,
            timeline: Timeline::default(),
            duration: self.frames as f64 / f64::from(self.sample_rate),
            entries: self.entries,
        }
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
    fn chunked_equals_batch() {
        // Tone, silence and a burst, so every band and the smoothing see edges.
        let mut audio = tone(44_100, 45.0, 0.6, 0.0, 1.0);
        audio.extend(vec![0.0; 2 * 4410]);
        audio.extend(tone(44_100, 3000.0, 0.3, 0.0, 0.7));
        audio.push(0.25); // a trailing half frame, ignored by both
        for params in [
            Params::default(),
            Params {
                low_smoothing: 0.0,
                ..Params::default()
            },
            Params {
                low_smoothing: 0.05,
                ..Params::default()
            },
        ] {
            let batch = compute(&audio, 44_100, &params);
            assert_eq!(batch.entries.len(), 270); // 1.8 s
            // 1 and 3 floats split frames; 97 and 255 are under one entry (294 frames).
            for chunk in [1, 3, 97, 255, 588, 1001, 44_100, audio.len()] {
                let mut b = Builder::new(44_100, &params);
                for c in audio.chunks(chunk) {
                    b.push(c);
                }
                b.push(&[]);
                assert_eq!(b.finish(), batch, "chunks of {chunk}");
            }
        }
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
