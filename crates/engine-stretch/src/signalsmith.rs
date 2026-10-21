//! [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch) (MIT),
//! through the `signalsmith-stretch` crate's C++ wrapper.

use signalsmith_stretch::Stretch;

use crate::Stretcher;

/// Signalsmith Stretch with its default preset: 120 ms blocks, 30 ms interval. At 48 kHz that
/// is 2880 frames of input and 2880 frames of output latency.
///
/// The C++ `seek` and `process` only resize buffers that `configure` already sized, so they
/// do not allocate after construction.
pub struct SignalsmithStretcher {
    inner: Stretch,
    channels: usize,
    block: usize,
    interval: usize,
}

impl std::fmt::Debug for SignalsmithStretcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalsmithStretcher")
            .field("channels", &self.channels)
            .field("block", &self.block)
            .field("interval", &self.interval)
            .finish()
    }
}

impl SignalsmithStretcher {
    /// Same block and interval as Signalsmith's `presetDefault`. Allocates.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate);
        Self::with_block(channels, (sr * 0.12) as usize, (sr * 0.03) as usize)
    }

    /// Signalsmith's `presetCheaper`: 100 ms blocks, 40 ms interval, and split computation,
    /// which spreads the work evenly at the cost of one more interval of output latency.
    /// Allocates.
    pub fn cheaper(channels: usize, sample_rate: u32) -> Self {
        assert!(channels > 0, "need at least one channel");
        let sr = f64::from(sample_rate);
        Self {
            inner: Stretch::preset_cheaper(channels as u32, sample_rate),
            channels,
            block: (sr * 0.1) as usize,
            interval: (sr * 0.04) as usize,
        }
    }

    /// Explicit FFT block and interval, in frames. Allocates.
    pub fn with_block(channels: usize, block: usize, interval: usize) -> Self {
        assert!(channels > 0, "need at least one channel");
        assert!(block > 0 && interval > 0, "block and interval must be > 0");
        Self {
            inner: Stretch::new(channels as u32, block, interval),
            channels,
            block,
            interval,
        }
    }
}

impl Stretcher for SignalsmithStretcher {
    fn channels(&self) -> usize {
        self.channels
    }

    fn input_latency(&self) -> usize {
        self.inner.input_latency()
    }

    fn output_latency(&self) -> usize {
        self.inner.output_latency()
    }

    fn preroll_frames(&self) -> usize {
        self.block + self.interval
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn seek(&mut self, input: &[f32], rate: f64) {
        self.inner.seek(input, rate);
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.inner.process(input, output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn sine(frames: usize, hz: f64) -> Vec<f32> {
        let w = std::f64::consts::TAU * hz / f64::from(SR);
        (0..frames)
            .flat_map(|i| {
                let s = (0.5 * (w * i as f64).sin()) as f32;
                [s, s]
            })
            .collect()
    }

    /// Frequency of the left channel from rising zero crossings.
    fn frequency(interleaved: &[f32]) -> f64 {
        let left: Vec<f32> = interleaved.iter().step_by(2).copied().collect();
        let rising: Vec<usize> = (1..left.len())
            .filter(|&i| left[i - 1] < 0.0 && left[i] >= 0.0)
            .collect();
        let (first, last) = (rising[0], rising[rising.len() - 1]);
        (rising.len() - 1) as f64 * f64::from(SR) / (last - first) as f64
    }

    /// Stream `input` through in 128-frame output blocks at `rate`; return the output.
    fn stretch(
        s: &mut SignalsmithStretcher,
        input: &[f32],
        rate: f64,
        out_frames: usize,
    ) -> Vec<f32> {
        let block = 128;
        let mut out = vec![0.0; out_frames * 2];
        let mut fed = 0usize;
        for (k, chunk) in out.chunks_mut(block * 2).enumerate() {
            let want = (((k + 1) * block) as f64 * rate).round() as usize;
            let n = want - fed;
            s.process(&input[fed * 2..(fed + n) * 2], chunk);
            fed += n;
        }
        out
    }

    #[test]
    fn keeps_pitch_while_changing_speed() {
        for rate in [0.92, 1.08, 1.5] {
            let mut s = SignalsmithStretcher::new(2, SR);
            let input = sine(SR as usize * 3, 1000.0);
            let out = stretch(&mut s, &input, rate, SR as usize);
            // Skip the pre-roll and the fade-in.
            let f = frequency(&out[2 * SR as usize / 4..]);
            assert!((f - 1000.0).abs() < 5.0, "rate {rate}: {f} Hz");
        }
    }

    #[test]
    fn reports_latency_of_the_default_preset() {
        let s = SignalsmithStretcher::new(2, SR);
        assert_eq!(s.input_latency(), 2880);
        assert_eq!(s.output_latency(), 2880);
        assert_eq!(s.preroll_frames(), 5760 + 1440);
        let c = SignalsmithStretcher::cheaper(2, SR);
        assert_eq!(c.preroll_frames(), 4800 + 1920);
        assert!(
            c.output_latency() > c.input_latency(),
            "split adds an interval"
        );
    }
}
