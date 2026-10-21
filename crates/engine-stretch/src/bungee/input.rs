// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Input.h / Input.cpp.

use super::window;

pub struct Input {
    pub window: Box<[f32]>,
    /// One windowed transform input per channel, `window.len()` apart.
    pub windowed_input: Box<[f32]>,
}

impl Input {
    pub fn new(log2_synthesis_hop: u32, channels: usize) -> Self {
        let pi = std::f32::consts::PI;
        let gain = (3.0 * pi) / (3.0 * pi + 8.0);
        let length = 8 << log2_synthesis_hop;
        Self {
            window: window::from_frequency_domain_coefficients(
                log2_synthesis_hop + 3,
                gain / length as f32,
                &[1.0, 0.5],
            ),
            windowed_input: vec![0.0; length * channels].into_boxed_slice(),
        }
    }

    /// Window `rows` frames of planar `input` (channels `stride` apart) into
    /// `windowed_input`, rotated so the grain centre lands on sample 0. Muted frames at either
    /// end are not read and count as zeros. Returns the log2 transform length.
    pub fn apply_analysis_window(
        &mut self,
        input: &[f32],
        stride: usize,
        rows: usize,
        mut mute_head: i64,
        mut mute_tail: i64,
    ) -> u32 {
        let length = self.window.len();
        let half = length / 2;
        let h = half as i64;
        debug_assert!(rows.is_multiple_of(2));
        let unused = (rows / 2).saturating_sub(half) as i64;
        mute_head -= unused;
        mute_tail -= unused;
        let channels = self.windowed_input.len() / length;
        let middle = rows / 2;

        let span = |head: i64, tail: i64| {
            let head = head.clamp(0, h) as usize;
            let tail = tail.clamp(0, h) as usize;
            let unmuted = half.saturating_sub(head + tail);
            (head, half - head - unmuted, unmuted)
        };

        // Top half of the window, bottom half of the input -> top half of the output.
        let (head, tail, unmuted) = span(mute_head - h, mute_tail);
        for c in 0..channels {
            let out = &mut self.windowed_input[c * length..][..half];
            out[..head].fill(0.0);
            if unmuted > 0 {
                let src = &input[c * stride + middle + head..][..unmuted];
                let window = &self.window[head..head + unmuted];
                window::apply(false, window, src, &mut out[head..head + unmuted]);
            }
            out[half - tail..].fill(0.0);
        }

        // Bottom half of the window, top half of the input -> bottom half of the output.
        let (head, tail, unmuted) = span(mute_head, mute_tail - h);
        for c in 0..channels {
            let out = &mut self.windowed_input[c * length + half..][..half];
            out[..head].fill(0.0);
            if unmuted > 0 {
                let src = &input[c * stride + middle - half + head..][..unmuted];
                let w = length - tail - unmuted;
                let window = &self.window[w..w + unmuted];
                window::apply(false, window, src, &mut out[head..head + unmuted]);
            }
            out[half - tail..].fill(0.0);
        }

        length.trailing_zeros()
    }
}
