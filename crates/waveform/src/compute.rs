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

use crate::filter::Splitter;
pub use crate::model::{
    ENTRIES_PER_SECOND, Entry, OVERVIEW_COLUMNS, Origin, Overview, Params, Provenance, Timeline,
    Waveform, encode_level, level, overview,
};

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
        let (frames, remainder) = rest.as_chunks::<2>();
        for f in frames {
            self.frame(f[0], f[1]);
        }
        self.half = remainder.first().copied();
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

    /// The entries that are final so far, for a progressive display during decode. The
    /// last few (the low-band smoothing radius) and a partly filled entry follow later.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
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

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

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
}
