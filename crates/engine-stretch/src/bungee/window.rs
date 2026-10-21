// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Window.h / Window.cpp.

use rustfft::num_complex::Complex32;

use super::fourier::{RealFft, bin_count, transform_length};

/// A zero-phase window (centred on sample 0) built from a few cosine coefficients, through an
/// inverse transform of size `2^log2_size`. Allocates.
pub fn from_frequency_domain_coefficients(
    log2_size: u32,
    gain: f32,
    coefficients: &[f32],
) -> Box<[f32]> {
    let mut frequency = vec![Complex32::default(); bin_count(log2_size)];
    for (bin, &c) in frequency.iter_mut().zip(coefficients) {
        *bin = Complex32::new(c * gain, 0.0);
    }
    let mut window = vec![0.0; transform_length(log2_size)];
    RealFft::new(log2_size).inverse(&frequency, &mut window);
    window.into_boxed_slice()
}

/// `output = input * window`, or `output += input * window` if `add`.
pub fn apply(add: bool, window: &[f32], input: &[f32], output: &mut [f32]) {
    let rows = window.iter().zip(input).zip(output.iter_mut());
    if add {
        rows.for_each(|((w, x), y)| *y += x * w);
    } else {
        rows.for_each(|((w, x), y)| *y = x * w);
    }
}
