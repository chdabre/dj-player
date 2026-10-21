// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Resample.h, limited to its default mode (`autoOut`): input is never
// resampled; output is resampled with bilinear interpolation only when the pitch (or a
// sample-rate change) needs it.

use super::SampleRates;

/// Rows of padding either side of a [`Padded`] buffer.
pub const PADDING: usize = 8;

/// Planar audio (each channel `rows()` apart) with [`PADDING`] spare rows before and after,
/// so interpolation can read across the edges.
#[derive(Debug)]
pub struct Padded {
    pub array: Box<[f32]>,
    pub frame_count: usize,
    pub all_zeros: bool,
    rows: usize,
}

impl Padded {
    pub fn new(max_frame_count: usize, channels: usize) -> Self {
        let rows = PADDING + max_frame_count + PADDING;
        Self {
            array: vec![0.0; rows * channels].into_boxed_slice(),
            frame_count: 0,
            all_zeros: true,
            rows,
        }
    }

    /// Distance between channels in `array`.
    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn channels(&self) -> usize {
        self.array.len() / self.rows
    }

    /// Unpadded rows `begin..begin + len` of channel `c`.
    pub fn unpadded_mut(&mut self, c: usize, begin: usize, len: usize) -> &mut [f32] {
        let at = c * self.rows + PADDING + begin;
        &mut self.array[at..at + len]
    }
}

/// One direction of resampling: whether it runs, and its ratio.
#[derive(Debug, Clone, Copy, Default)]
pub struct Operation {
    pub active: bool,
    pub ratio: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Operations {
    pub input: Operation,
    pub output: Operation,
}

impl Operations {
    /// Configure for `pitch`; returns the input-to-output frame ratio of one unit hop.
    pub fn setup(&mut self, sample_rates: SampleRates, pitch: f64) -> f64 {
        let resample_ratio = pitch * f64::from(sample_rates.input) / f64::from(sample_rates.output);
        self.input.ratio = (1.0 / resample_ratio) as f32;
        self.output.ratio = resample_ratio as f32;

        // autoOut: only output resampling, and only when needed.
        self.input.active = false;
        self.output.active = resample_ratio != 1.0;

        if !self.input.active {
            self.input.ratio = 1.0;
        }
        if !self.output.active {
            self.output.ratio = 1.0;
        }

        (f64::from(sample_rates.input) / f64::from(sample_rates.output))
            / f64::from(self.output.ratio)
    }
}

/// Resample `fixed` (at a fixed rate) into `variable` (a planar buffer with `variable_rows`
/// rows per channel), with the ratio moving linearly from `ratio_begin` to `ratio_end`.
/// `fixed_offset` carries the fractional read position from one call to the next. With
/// `interpolate` false nothing is written, but the frame count and offset advance the same
/// way (Bungee's `Resample::None`). Returns the number of frames written.
#[allow(clippy::too_many_arguments)]
pub fn fixed_to_variable(
    fixed: &Padded,
    fixed_offset: &mut f32,
    variable: &mut [f32],
    variable_rows: usize,
    ratio_begin: f32,
    mut ratio_end: f32,
    align_end: bool,
    interpolate: bool,
) -> usize {
    let fixed_frames = fixed.frame_count as f32;
    let mut count = (2.0 * (fixed_frames + ratio_end - *fixed_offset) / (ratio_begin + ratio_end)
        - 1.0)
        .round() as i64;

    let truncate = count > variable_rows as i64;
    if truncate {
        count = variable_rows as i64;
    }
    let count = count.max(0) as usize;

    if align_end {
        let dividend = 2.0 * (fixed_frames - *fixed_offset) - (count + 1) as f32 * ratio_begin;
        let divisor = count as f32 - 1.0;
        ratio_end = dividend / divisor;
    }

    let channels = fixed.channels();
    let rows = fixed.rows();
    let mut step = |x: f32, i: usize| {
        if !interpolate {
            return;
        }
        let xi = x as usize;
        let k = x - xi as f32;
        if xi + 1 >= rows {
            return;
        }
        for c in 0..channels {
            let f = &fixed.array[c * rows..];
            let v = &mut variable[c * variable_rows + i];
            *v = f[xi + 1] * k;
            *v += f[xi] * (1.0 - k);
        }
    };

    let offset = PADDING as f32 + *fixed_offset;
    let mut ratio = ratio_begin;
    if ratio_begin != ratio_end {
        let gradient = (ratio_end - ratio_begin) / count as f32;
        for i in 0..count {
            let x = offset + i as f32 * (ratio_begin + ratio) * 0.5;
            ratio = ratio_begin + gradient * i as f32;
            step(x, i);
        }
    } else {
        for i in 0..count {
            step(offset + i as f32 * ratio, i);
        }
    }
    *fixed_offset += count as f32 * (ratio_begin + ratio) * 0.5;
    *fixed_offset -= fixed_frames;

    let tolerance = if align_end {
        1e-2
    } else {
        (ratio_begin + ratio_end) * 0.3
    };
    if !(truncate || fixed_offset.abs() < tolerance) {
        // "resample landed badly": Bungee asserts in debug builds and resets.
        *fixed_offset = 0.0;
    }

    count
}
