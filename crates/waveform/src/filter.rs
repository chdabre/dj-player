//! The band split: one 3-way Linkwitz-Riley crossover per channel, built from Butterworth
//! biquads.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use crate::model::Params;

/// One channel's 3-way Linkwitz-Riley crossover.
pub(crate) struct Splitter {
    low: [Biquad; 2],
    mid_hp: [Biquad; 2],
    mid_lp: [Biquad; 2],
    high: [Biquad; 2],
}

impl Splitter {
    pub(crate) fn new(p: &Params, sample_rate: u32) -> Self {
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

    pub(crate) fn process(&mut self, x: f64) -> [f64; 3] {
        let run = |[a, b]: &mut [Biquad; 2], x| b.process(a.process(x));
        let low = run(&mut self.low, x);
        let mid = run(&mut self.mid_lp, run(&mut self.mid_hp, x));
        let high = run(&mut self.high, x);
        [low, mid, high]
    }

    pub(crate) fn flush(&mut self) {
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
