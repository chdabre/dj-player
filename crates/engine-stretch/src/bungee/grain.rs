// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Grain.h / Grain.cpp.

use rustfft::num_complex::Complex32;

use super::fourier::{frequency_rows, transform_length};
use super::output::Segment;
use super::partials::Partial;
use super::phase::Phase;
use super::resample::Operations;
use super::{InputChunk, Request, SampleRates};

#[derive(Debug, Clone, Copy, Default)]
pub struct Analysis {
    pub position_error: f64,
    pub hop_ideal: f64,
    pub speed: f64,
    /// Rounded analysis hop, in input frames; negative in reverse.
    pub hop: i32,
}

pub struct Grain {
    pub log2_transform_length: u32,
    pub request: Request,

    pub request_hop: f64,
    pub continuous: bool,
    pub passthrough: i32,
    pub valid_bin_count: usize,
    pub mute_frame_count_head: i64,
    pub mute_frame_count_tail: i64,

    pub resample: Operations,

    pub input_chunk: InputChunk,
    pub analysis: Analysis,

    /// One spectrum per channel, `rows` apart.
    pub transformed: Box<[Complex32]>,
    pub rows: usize,
    pub phase: Box<[Phase]>,
    pub energy: Box<[f32]>,
    pub rotation: Box<[Phase]>,
    pub delta: Box<[Phase]>,
    pub partials: Box<[Partial]>,
    pub partial_count: usize,

    pub segment: Segment,
}

impl Grain {
    pub fn new(log2_synthesis_hop: u32, channels: usize) -> Self {
        let log2_transform_length = log2_synthesis_hop + 3;
        let rows = frequency_rows(log2_transform_length);
        Self {
            log2_transform_length,
            request: Request {
                position: f64::NAN,
                speed: f64::NAN,
                pitch: 1.0,
                reset: false,
            },
            request_hop: 0.0,
            continuous: false,
            passthrough: 0,
            valid_bin_count: 0,
            mute_frame_count_head: 0,
            mute_frame_count_tail: 0,
            resample: Operations::default(),
            input_chunk: InputChunk::default(),
            analysis: Analysis::default(),
            transformed: vec![Complex32::default(); rows * channels].into_boxed_slice(),
            rows,
            phase: vec![0; rows].into_boxed_slice(),
            energy: vec![0.0; rows].into_boxed_slice(),
            rotation: vec![0; rows].into_boxed_slice(),
            delta: vec![0; rows].into_boxed_slice(),
            partials: vec![Partial::default(); transform_length(log2_transform_length)]
                .into_boxed_slice(),
            partial_count: 0,
            segment: Segment::new(log2_synthesis_hop, channels),
        }
    }

    pub fn reverse(&self) -> bool {
        self.analysis.hop < 0
    }

    pub fn valid(&self) -> bool {
        !self.request.position.is_nan()
    }

    /// Take `request` as this grain, following `previous`; return the input it needs.
    pub fn specify(
        &mut self,
        request: &Request,
        previous: &Grain,
        sample_rates: SampleRates,
        log2_synthesis_hop: u32,
        buffer_start_position: f64,
    ) -> InputChunk {
        self.request = *request;
        debug_assert!(self.request.pitch > 0.0);

        let synthesis_hop = f64::from(1u32 << log2_synthesis_hop);
        let unit_hop = synthesis_hop * self.resample.setup(sample_rates, self.request.pitch);

        self.request_hop = self.request.position - previous.request.position;
        if self.request_hop.is_nan() || self.request.reset {
            self.request_hop = self.request.speed * unit_hop;
        }

        self.analysis.hop_ideal = self.request_hop * f64::from(self.resample.input.ratio);

        self.continuous = !self.request.reset && !previous.request.position.is_nan();
        if self.continuous {
            self.analysis.position_error =
                previous.analysis.position_error - self.analysis.hop_ideal;
            self.analysis.hop = (-self.analysis.position_error).round() as i32;
            self.analysis.position_error += f64::from(self.analysis.hop);
        } else {
            self.analysis.hop = self.analysis.hop_ideal.round() as i32;
            self.analysis.position_error = self.request.position.round() - self.request.position;
        }

        self.analysis.speed = self.analysis.hop_ideal / synthesis_hop;

        self.passthrough = if self.analysis.speed.abs() == 1.0 {
            self.analysis.speed as i32
        } else {
            0
        };
        if self.continuous && self.passthrough != previous.passthrough {
            self.passthrough = 0;
        }

        self.log2_transform_length = log2_synthesis_hop + 3;

        // Without input resampling (autoOut) the chunk is exactly one transform long.
        let half = (transform_length(self.log2_transform_length) / 2) as i64;
        self.input_chunk = InputChunk {
            begin: -half,
            end: half,
        };

        if self.request.position.is_nan() {
            return InputChunk::default();
        }

        let offset = (self.request.position - buffer_start_position).round() as i64;
        self.input_chunk.begin += offset;
        self.input_chunk.end += offset;
        self.input_chunk
    }
}
