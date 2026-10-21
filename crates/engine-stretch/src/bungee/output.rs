// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Output.h / Output.cpp.

use super::grain::Grain;
use super::resample::{self, Operation, PADDING, Padded};
use super::window;

pub struct Output {
    pub synthesis_window: Box<[f32]>,
    /// One inverse transform per channel, `transform_length` apart.
    pub inverse_transformed: Box<[f32]>,
    pub transform_length: usize,
    pub buffer_resampled: Box<[f32]>,
    pub resampled_rows: usize,
    pub resample_offset: f32,
}

impl Output {
    pub fn new(
        log2_synthesis_hop: u32,
        channels: usize,
        max_output_chunk_size: usize,
        window_gain: f32,
        window_coefficients: &[f32],
    ) -> Self {
        let transform_length = 8 << log2_synthesis_hop;
        Self {
            synthesis_window: window::from_frequency_domain_coefficients(
                log2_synthesis_hop + 2,
                window_gain,
                window_coefficients,
            ),
            inverse_transformed: vec![0.0; transform_length * channels].into_boxed_slice(),
            transform_length,
            buffer_resampled: vec![0.0; max_output_chunk_size * channels].into_boxed_slice(),
            resampled_rows: max_output_chunk_size,
            resample_offset: 0.0,
        }
    }

    /// Overlap-add the newest grain's inverse transform into the four grains' output
    /// segments. `grains` is ordered newest first, as Bungee's `grains[0..4]`.
    pub fn apply_synthesis_window(&self, log2_synthesis_hop: u32, grains: [&mut Grain; 4]) {
        let [g0, g1, g2, g3] = grains;
        let quadrant_size = self.synthesis_window.len() / 4;
        let hops_per_transform = 1 << (g0.log2_transform_length - log2_synthesis_hop);
        let valid = g0.valid();
        let needs_resample = g1.resample.output.active || g0.resample.output.active;

        g0.segment.buffer_lapped.frame_count = 0;
        g0.segment.buffer_lapped.all_zeros = true;

        let mut quadrants = [g3, g2, g1, g0];
        for (i, grain) in quadrants.iter_mut().enumerate() {
            let quadrant = &mut grain.segment.buffer_lapped;
            if valid {
                let w = quadrant_size * (i ^ 2);
                let window = &self.synthesis_window[w..w + quadrant_size];
                let j = (i + hops_per_transform - 2) % hops_per_transform;
                let add = quadrant.frame_count != 0;
                for c in 0..quadrant.channels() {
                    let at = c * self.transform_length + quadrant_size * j;
                    let input = &self.inverse_transformed[at..at + quadrant_size];
                    window::apply(
                        add,
                        window,
                        input,
                        quadrant.unpadded_mut(c, 0, quadrant_size),
                    );
                }
                quadrant.all_zeros = false;
            } else if quadrant.frame_count == 0 {
                for c in 0..quadrant.channels() {
                    quadrant.unpadded_mut(c, 0, quadrant_size).fill(0.0);
                }
            }
            quadrant.frame_count = quadrant_size;
        }

        quadrants[1].segment.needs_resample = needs_resample;
    }
}

#[derive(Debug)]
pub struct Segment {
    pub buffer_lapped: Padded,
    pub needs_resample: bool,
}

impl Segment {
    pub fn new(log2_frame_count: u32, channels: usize) -> Self {
        Self {
            buffer_lapped: Padded::new(1 << log2_frame_count, channels),
            needs_resample: false,
        }
    }

    /// Share the edge frames of two consecutive segments through their padding, so the
    /// output resampler interpolates across the boundary.
    pub fn lap_padding(current: &mut Segment, next: &mut Segment) {
        const N: usize = PADDING;
        let rows = current.buffer_lapped.rows();
        let channels = current.buffer_lapped.channels();
        let frames = current.buffer_lapped.frame_count;

        if current.needs_resample {
            for c in 0..channels {
                let dst = &mut current.buffer_lapped.array[c * rows + N + frames..][..N];
                if next.buffer_lapped.all_zeros {
                    dst.fill(0.0);
                } else {
                    dst.copy_from_slice(&next.buffer_lapped.array[c * rows + N..][..N]);
                }
            }
        }

        if current.needs_resample || next.needs_resample {
            for c in 0..channels {
                let dst = &mut next.buffer_lapped.array[c * rows..][..N];
                if current.buffer_lapped.all_zeros {
                    dst.fill(0.0);
                } else {
                    dst.copy_from_slice(&current.buffer_lapped.array[c * rows + frames..][..N]);
                }
            }
        }
    }
}

/// Where a finished output chunk lives: the segment itself or the resampling buffer.
pub enum ChunkSource {
    Lapped,
    Resampled,
}

/// Resample the segment into `output.buffer_resampled` if either end needs it. Returns where
/// the chunk is and its frame count; an all-zero segment produces zeros.
pub fn resample_segment(
    segment: &mut Segment,
    output: &mut Output,
    mut begin: Operation,
    mut end: Operation,
) -> (ChunkSource, usize) {
    if !begin.active {
        begin.ratio = 1.0;
    }
    if !end.active {
        end.ratio = 1.0;
        end.active = begin.active;
    }

    let lapped = &mut segment.buffer_lapped;
    if end.active {
        let align_end = end.ratio == 1.0;
        let frames = resample::fixed_to_variable(
            lapped,
            &mut output.resample_offset,
            &mut output.buffer_resampled,
            output.resampled_rows,
            begin.ratio,
            end.ratio,
            align_end,
            !lapped.all_zeros,
        );
        if lapped.all_zeros {
            for c in 0..lapped.channels() {
                let at = c * output.resampled_rows;
                output.buffer_resampled[at..at + frames].fill(0.0);
            }
        }
        (ChunkSource::Resampled, frames)
    } else {
        let frames = lapped.frame_count;
        if lapped.all_zeros {
            for c in 0..lapped.channels() {
                lapped.unpadded_mut(c, 0, frames).fill(0.0);
            }
        }
        (ChunkSource::Lapped, frames)
    }
}
