// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Fourier.h / Fourier.cpp. Bungee uses kissfft's real transforms; this
// uses rustfft with the usual half-length complex packing. Both are unnormalised.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub const fn transform_length(log2_transform_length: u32) -> usize {
    1 << log2_transform_length
}

pub const fn bin_count(log2_transform_length: u32) -> usize {
    transform_length(log2_transform_length - 1) + 1
}

/// Rows of a frequency-domain array: all bins plus padding (Bungee's `Fourier::resize<true>`),
/// so `Partials::enumerate` can place its two sentinels past the last valid bin.
pub const fn frequency_rows(log2_transform_length: u32) -> usize {
    bin_count(log2_transform_length) - 1 + 8
}

/// Real FFT of one length, like a kissfft `kiss_fftr` forward and inverse pair. All buffers
/// are allocated here; [`RealFft::forward`] and [`RealFft::inverse`] do not allocate.
pub struct RealFft {
    n: usize,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    /// `e^(-2 pi i k / n)` for `k < n / 2`.
    twiddles: Box<[Complex32]>,
    buf: Box<[Complex32]>,
    scratch: Box<[Complex32]>,
}

impl RealFft {
    pub fn new(log2_transform_length: u32) -> Self {
        let n = transform_length(log2_transform_length);
        assert!(n >= 4, "transform too short");
        let m = n / 2;
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(m);
        let inverse = planner.plan_fft_inverse(m);
        let scratch = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let twiddles = (0..m)
            .map(|k| {
                let a = -std::f64::consts::TAU * k as f64 / n as f64;
                Complex32::new(a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Self {
            n,
            forward,
            inverse,
            twiddles,
            buf: vec![Complex32::default(); m].into_boxed_slice(),
            scratch: vec![Complex32::default(); scratch].into_boxed_slice(),
        }
    }

    /// `n` real samples of `t` to bins `0..=n/2` of `f`.
    pub fn forward(&mut self, t: &[f32], f: &mut [Complex32]) {
        let m = self.n / 2;
        for (k, z) in self.buf.iter_mut().enumerate() {
            *z = Complex32::new(t[2 * k], t[2 * k + 1]);
        }
        self.forward
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        let z0 = self.buf[0];
        f[0] = Complex32::new(z0.re + z0.im, 0.0);
        f[m] = Complex32::new(z0.re - z0.im, 0.0);
        let minus_half_i = Complex32::new(0.0, -0.5);
        for (k, bin) in f[1..m].iter_mut().enumerate().map(|(k, b)| (k + 1, b)) {
            let a = self.buf[k];
            let b = self.buf[m - k].conj();
            let even = (a + b) * 0.5;
            let odd = (a - b) * minus_half_i;
            *bin = even + self.twiddles[k] * odd;
        }
    }

    /// Bins `0..=n/2` of `f` to `n` real samples of `t`, unnormalised. Like kissfft, the
    /// imaginary parts of the DC and Nyquist bins are ignored.
    pub fn inverse(&mut self, f: &[Complex32], t: &mut [f32]) {
        let m = self.n / 2;
        let i = Complex32::new(0.0, 1.0);
        for k in 0..m {
            let (a, b) = if k == 0 {
                (Complex32::new(f[0].re, 0.0), Complex32::new(f[m].re, 0.0))
            } else {
                (f[k], f[m - k].conj())
            };
            self.buf[k] = (a + b) + i * self.twiddles[k].conj() * (a - b);
        }
        self.inverse
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        for (k, z) in self.buf.iter().enumerate() {
            t[2 * k] = z.re;
            t[2 * k + 1] = z.im;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dft(x: &[f32]) -> Vec<(f64, f64)> {
        let n = x.len();
        (0..=n / 2)
            .map(|k| {
                x.iter().enumerate().fold((0.0, 0.0), |(re, im), (j, &v)| {
                    let a = -std::f64::consts::TAU * (k * j) as f64 / n as f64;
                    (re + f64::from(v) * a.cos(), im + f64::from(v) * a.sin())
                })
            })
            .collect()
    }

    #[test]
    fn forward_matches_a_dft_and_inverse_undoes_it() {
        let n = 64;
        let x: Vec<f32> = (0..n)
            .map(|i| ((i * 7919) % 101) as f32 / 50.0 - 1.0)
            .collect();
        let mut fft = RealFft::new(6);
        let mut f = vec![Complex32::default(); n / 2 + 1];
        fft.forward(&x, &mut f);
        for (got, want) in f.iter().zip(dft(&x)) {
            assert!((f64::from(got.re) - want.0).abs() < 1e-3, "{got} {want:?}");
            assert!((f64::from(got.im) - want.1).abs() < 1e-3, "{got} {want:?}");
        }
        let mut y = vec![0.0; n];
        fft.inverse(&f, &mut y);
        for (a, b) in x.iter().zip(&y) {
            assert!((a * n as f32 - b).abs() < 1e-3, "{a} {b}");
        }
    }
}
