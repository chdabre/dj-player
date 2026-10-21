// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee Basic 2.x (https://github.com/bungee-audio-stretch/bungee): the
// grain API of Bungee.h, Stretcher.h / Stretcher.cpp, Timing.h / Timing.cpp and
// Grains.h / Grains.cpp. Eigen arrays became planar slices, kissfft became rustfft.

//! [Bungee](https://github.com/bungee-audio-stretch/bungee) Basic, ported to Rust.
//!
//! A pull-based, granular time-stretcher and pitch-shifter. The caller drives it one grain
//! at a time:
//!
//! 1. [`Stretcher::specify_grain`] with a [`Request`] (input position and speed); it answers
//!    with the [`InputChunk`] it needs.
//! 2. [`Stretcher::analyse_grain`] with that chunk's audio, planar.
//! 3. [`Stretcher::synthesise_grain`] returns an [`OutputChunk`]: one synthesis hop of
//!    output, tagged with the requests of the grains either side of it.
//! 4. [`Stretcher::next`] advances the request by one hop at its speed.
//!
//! Positions may go backwards, so reverse playback keeps the pitch. A request with a NaN
//! position is an invalid grain: it produces silence and flushes the pipeline.
//!
//! Only Bungee's default resampling mode (`autoOut`) is ported. Everything is allocated by
//! [`Stretcher::new`]; nothing else allocates.

mod fourier;
mod grain;
mod input;
mod output;
mod partials;
mod phase;
mod resample;
mod synthesis;
mod window;

use rustfft::num_complex::Complex32;

use fourier::{RealFft, bin_count, frequency_rows};
use grain::Grain;
use input::Input;
use output::{ChunkSource, Output, Segment};

/// One grain's request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    /// Input frame at the centre of the grain. NaN marks an invalid grain, which produces no
    /// audio and can be used for flushing.
    pub position: f64,
    /// Output speed; 1 is unchanged. Only used when it cannot be worked out from the
    /// previous grain's position.
    pub speed: f64,
    /// Frequency multiplier; 1 is no pitch change.
    pub pitch: f64,
    /// Forget all previous grains and restart on this one.
    pub reset: bool,
}

/// The input frames a grain needs, relative to the buffer start given to
/// [`Stretcher::specify_grain`]. Consecutive grains' chunks usually overlap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputChunk {
    pub begin: i64,
    pub end: i64,
}

impl InputChunk {
    pub fn len(&self) -> usize {
        (self.end - self.begin).max(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One chunk of output. Chunks do not overlap; appended, they play seamlessly.
#[derive(Debug)]
pub struct OutputChunk<'a> {
    /// Planar audio: channel `c` is `data[c * channel_stride..][..frame_count]`.
    pub data: &'a [f32],
    pub frame_count: usize,
    pub channel_stride: usize,
    /// `request[0]` belongs to the first frame of `data`, `request[1]` to the frame after
    /// the last.
    pub request: [Request; 2],
}

impl OutputChunk<'_> {
    pub fn channel(&self, c: usize) -> &[f32] {
        &self.data[c * self.channel_stride..][..self.frame_count]
    }
}

/// Input and output sample rates in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleRates {
    pub input: u32,
    pub output: u32,
}

const MAX_PITCH_OCTAVES: u32 = 2;

/// Bungee's `Timing`: the synthesis hop and how requests advance.
#[derive(Debug, Clone, Copy)]
struct Timing {
    log2_synthesis_hop: u32,
    sample_rates: SampleRates,
}

impl Timing {
    fn new(sample_rates: SampleRates, log2_synthesis_hop_adjust: i32) -> Self {
        let log2_floor = 31 - sample_rates.input.leading_zeros() as i32;
        let log2_synthesis_hop = log2_floor - 6 + log2_synthesis_hop_adjust;
        assert!(log2_synthesis_hop >= 2, "sample rate too low");
        Self {
            log2_synthesis_hop: log2_synthesis_hop as u32,
            sample_rates,
        }
    }

    fn max_input_frame_count(&self) -> usize {
        let shift = MAX_PITCH_OCTAVES + self.log2_synthesis_hop + 3;
        ((i64::from(self.sample_rates.input) << shift) / i64::from(self.sample_rates.output))
            as usize
            + 1
    }

    fn max_output_frame_count(&self) -> usize {
        let shift = MAX_PITCH_OCTAVES + self.log2_synthesis_hop;
        ((i64::from(self.sample_rates.output) << shift) / i64::from(self.sample_rates.input))
            as usize
            + 1
    }

    fn calculate_input_hop(&self, request: &Request) -> f64 {
        let unit_hop = f64::from(1u32 << self.log2_synthesis_hop)
            * resample::Operations::default().setup(self.sample_rates, request.pitch);
        unit_hop * request.speed
    }
}

/// Bungee Basic's stretcher. See the [module docs](self).
pub struct Stretcher {
    timing: Timing,
    channels: usize,
    fft: RealFft,
    input: Input,
    /// Bungee's `Grains`: a ring of four; see [`Stretcher::index`].
    grains: [Grain; 4],
    rotation: usize,
    output: Output,
    temporary: Box<[Complex32]>,
}

impl std::fmt::Debug for Stretcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stretcher")
            .field("sample_rates", &self.timing.sample_rates)
            .field("channels", &self.channels)
            .field("log2_synthesis_hop", &self.timing.log2_synthesis_hop)
            .finish_non_exhaustive()
    }
}

impl Stretcher {
    /// A stretcher with Bungee's default granularity. Allocates everything it will use.
    pub fn new(sample_rates: SampleRates, channels: usize) -> Self {
        Self::with_hop_adjust(sample_rates, channels, 0)
    }

    /// `log2_synthesis_hop_adjust` -1 halves the hop (lower latency, maybe better weak
    /// transients); +1 doubles it (maybe better dense tones). 0 is the default.
    pub fn with_hop_adjust(
        sample_rates: SampleRates,
        channels: usize,
        log2_synthesis_hop_adjust: i32,
    ) -> Self {
        assert!(channels > 0, "need at least one channel");
        let timing = Timing::new(sample_rates, log2_synthesis_hop_adjust);
        let hop = timing.log2_synthesis_hop;
        let max_output = timing.max_output_frame_count();
        Self {
            timing,
            channels,
            fft: RealFft::new(hop + 3),
            input: Input::new(hop, channels),
            grains: std::array::from_fn(|_| Grain::new(hop, channels)),
            rotation: 0,
            output: Output::new(hop, channels, max_output, 0.25, &[1.0, 0.5]),
            temporary: vec![Complex32::default(); frequency_rows(hop + 3)].into_boxed_slice(),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rates(&self) -> SampleRates {
        self.timing.sample_rates
    }

    /// Frames per synthesis hop: the output chunk length at pitch 1.
    pub fn synthesis_hop(&self) -> usize {
        1 << self.timing.log2_synthesis_hop
    }

    /// The most input frames [`Stretcher::specify_grain`] may ask for.
    pub fn max_input_frame_count(&self) -> usize {
        self.timing.max_input_frame_count()
    }

    /// The most frames one [`OutputChunk`] may hold.
    pub fn max_output_frame_count(&self) -> usize {
        self.timing.max_output_frame_count()
    }

    /// Move `request` back by one hop and set `reset`, so the stretcher runs in for a grain
    /// before reaching the requested position.
    pub fn preroll(&self, request: &mut Request) {
        request.position -= self.timing.calculate_input_hop(request);
        request.reset = true;
    }

    /// Advance `request` by one hop at its speed, for the next grain.
    pub fn next(&self, request: &mut Request) {
        if !request.speed.is_nan() && !request.position.is_nan() {
            request.position += self.timing.calculate_input_hop(request);
            request.reset = false;
        }
    }

    /// True if every grain in the pipeline is invalid (NaN position).
    pub fn is_flushed(&self) -> bool {
        self.grains.iter().all(|g| !g.valid())
    }

    /// The input [`Stretcher::specify_grain`] would ask for, without starting a grain. Not in
    /// Bungee's C++ API: it lets a real-time caller check that the audio is there first.
    pub fn input_chunk(&self, request: &Request, buffer_start_position: f64) -> InputChunk {
        if request.position.is_nan() {
            return InputChunk::default();
        }
        // Without input resampling (autoOut) the chunk is one transform long, centred.
        let half = (4i64) << self.timing.log2_synthesis_hop;
        let offset = (request.position - buffer_start_position).round() as i64;
        InputChunk {
            begin: offset - half,
            end: offset + half,
        }
    }

    /// Storage index of Bungee's `grains[i]` (0 is the newest).
    fn index(&self, i: usize) -> usize {
        (3 - i + self.rotation) % 4
    }

    /// Start a grain; returns the input it needs, relative to `buffer_start_position`.
    pub fn specify_grain(&mut self, request: &Request, buffer_start_position: f64) -> InputChunk {
        self.rotation = (self.rotation + 1) % 4;
        let (i0, i1) = (self.index(0), self.index(1));
        let [grain, previous] = self
            .grains
            .get_disjoint_mut([i0, i1])
            .expect("distinct grains");
        let timing = self.timing;
        grain.specify(
            request,
            previous,
            timing.sample_rates,
            timing.log2_synthesis_hop,
            buffer_start_position,
        )
    }

    /// Analyse the input of the grain just specified: `data` holds its chunk planar, channels
    /// `channel_stride` apart. Muted frames at the head or tail are not read and count as
    /// zero; an empty `data` mutes everything.
    pub fn analyse_grain(
        &mut self,
        data: &[f32],
        channel_stride: usize,
        mute_head: usize,
        mute_tail: usize,
    ) {
        let (i0, i1) = (self.index(0), self.index(1));
        let [grain, previous] = self
            .grains
            .get_disjoint_mut([i0, i1])
            .expect("distinct grains");

        let frame_count = grain.input_chunk.end - grain.input_chunk.begin;
        let (mut head, mut tail) = (mute_head as i64, mute_tail as i64);
        if data.is_empty() {
            head = frame_count;
            tail = 0;
        }
        head = head.clamp(0, frame_count);
        tail = tail.clamp(0, frame_count);
        grain.mute_frame_count_head = head;
        grain.mute_frame_count_tail = tail;

        grain.valid_bin_count = 0;
        if !grain.valid() {
            return;
        }

        let log2_transform_length = self.input.apply_analysis_window(
            data,
            channel_stride,
            frame_count as usize,
            head,
            tail,
        );
        let length = self.input.window.len();
        let rows = grain.rows;
        for c in 0..self.channels {
            self.fft.forward(
                &self.input.windowed_input[c * length..][..length],
                &mut grain.transformed[c * rows..][..length / 2 + 1],
            );
        }

        let n = bin_count(grain.log2_transform_length) - 1;
        let valid = ((n as f32 / grain.resample.output.ratio).ceil() as usize).min(n) + 1;
        grain.valid_bin_count = valid;
        for c in 0..self.channels {
            grain.transformed[c * rows + valid..c * rows + n + 1].fill(Complex32::default());
        }
        grain.log2_transform_length = log2_transform_length;

        for i in 0..valid {
            let mut x = grain.transformed[i];
            for c in 1..self.channels {
                x += grain.transformed[c * rows + i];
            }
            grain.energy[i] = x.re * x.re + x.im * x.im;
            grain.phase[i] = phase::from_radians(x.im.atan2(x.re));
        }

        grain.partial_count = partials::enumerate(&mut grain.partials, valid, &mut grain.energy);

        if grain.continuous {
            partials::suppress_transient_partials(
                &mut grain.partials[..grain.partial_count],
                &grain.energy,
                &previous.energy,
            );
        }
    }

    /// Finish the grain and return the next chunk of output. It is one grain behind: it is
    /// tagged with the requests of the previous two grains.
    pub fn synthesise_grain(&mut self) -> OutputChunk<'_> {
        let hop = self.timing.log2_synthesis_hop;
        let (i0, i1, i2, i3) = (self.index(0), self.index(1), self.index(2), self.index(3));

        {
            let [grain, previous] = self
                .grains
                .get_disjoint_mut([i0, i1])
                .expect("distinct grains");
            if grain.valid() {
                synthesis::synthesise(hop, grain, previous);

                let valid = grain.valid_bin_count;
                let k = std::f32::consts::PI / 32768.0;
                for (t, &r) in self.temporary[..valid]
                    .iter_mut()
                    .zip(&grain.rotation[..valid])
                {
                    let (sin, cos) = (f32::from(r) * k).sin_cos();
                    *t = Complex32::new(cos, sin);
                }

                let rows = grain.rows;
                let reverse = grain.reverse();
                let length = self.output.transform_length;
                for c in 0..self.channels {
                    let bins = &mut grain.transformed[c * rows..][..valid];
                    for (x, t) in bins.iter_mut().zip(&self.temporary[..valid]) {
                        *x = if reverse { x.conj() * t } else { *x * t };
                    }
                    self.fft.inverse(
                        &grain.transformed[c * rows..][..length / 2 + 1],
                        &mut self.output.inverse_transformed[c * length..][..length],
                    );
                }
            }
        }

        {
            let [g0, g1, g2, g3] = self
                .grains
                .get_disjoint_mut([i0, i1, i2, i3])
                .expect("distinct grains");
            self.output.apply_synthesis_window(hop, [g0, g1, g2, g3]);
        }

        let [g1, g2, g3] = self
            .grains
            .get_disjoint_mut([i1, i2, i3])
            .expect("distinct grains");
        Segment::lap_padding(&mut g3.segment, &mut g2.segment);

        let (source, frame_count) = output::resample_segment(
            &mut g3.segment,
            &mut self.output,
            g2.resample.output,
            g1.resample.output,
        );
        let request = [g2.request, g1.request];
        let (data, channel_stride) = match source {
            ChunkSource::Lapped => {
                let lapped = &g3.segment.buffer_lapped;
                (&lapped.array[resample::PADDING..], lapped.rows())
            }
            ChunkSource::Resampled => (
                &self.output.buffer_resampled[..],
                self.output.resampled_rows,
            ),
        };
        OutputChunk {
            data,
            frame_count,
            channel_stride,
            request,
        }
    }
}

#[cfg(test)]
mod tests;
