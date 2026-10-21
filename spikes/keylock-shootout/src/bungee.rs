//! Bungee Basic (MPL-2.0) through its native, pull-based grain API: the C++ library through
//! `bungee-rs`, and the pure-Rust port in `engine_stretch::bungee`, driven identically.
//!
//! Each grain asks for a position and speed; Bungee says which input range it needs, we hand
//! it that range straight from the resident track, and it returns a chunk of output tagged
//! with the positions it covers. Positions may move backwards, so reverse keeps the pitch too.

use engine_stretch::bungee::{self as port, Request};

use crate::candidate::Candidate;
use crate::track::{CHANNELS, Fifo, Track};

const FIFO_FRAMES: usize = 16_384;

/// The grain API both implementations share.
pub trait GrainApi {
    fn max_input_frame_count(&self) -> usize;
    fn preroll(&mut self, request: &mut Request);
    fn next(&mut self, request: &mut Request);
    fn is_flushed(&self) -> bool;
    /// Input range `[begin, end)` the grain needs.
    fn specify_grain(&mut self, request: &Request) -> (i64, i64);
    fn analyse_grain(&mut self, data: &mut [f32], stride: usize);
    /// Hand the output chunk to `f`: planar data, frame count, channel stride and the two
    /// request tags (`None` where the C++ API gives a null pointer).
    fn synthesise_grain<R>(
        &mut self,
        f: impl FnOnce(&[f32], usize, usize, [Option<Request>; 2]) -> R,
    ) -> R;
}

/// The C++ library through `bungee-rs`.
pub struct Cpp(bungee_rs::Stretcher);

impl Cpp {
    pub fn new(sample_rate: u32) -> Self {
        Self(bungee_rs::Stretcher::new(sample_rate as usize, CHANNELS).expect("bungee stretcher"))
    }
}

fn to_cpp(r: &Request) -> bungee_rs::Request {
    bungee_rs::Request {
        position: r.position,
        speed: r.speed,
        pitch: r.pitch,
        reset: r.reset,
    }
}

fn from_cpp(r: &bungee_rs::Request) -> Request {
    Request {
        position: r.position,
        speed: r.speed,
        pitch: r.pitch,
        reset: r.reset,
    }
}

impl GrainApi for Cpp {
    fn max_input_frame_count(&self) -> usize {
        self.0.max_input_frame_count()
    }

    fn preroll(&mut self, request: &mut Request) {
        let mut r = to_cpp(request);
        self.0.preroll(&mut r);
        *request = from_cpp(&r);
    }

    fn next(&mut self, request: &mut Request) {
        let mut r = to_cpp(request);
        self.0.next(&mut r);
        *request = from_cpp(&r);
    }

    fn is_flushed(&self) -> bool {
        self.0.is_flushed()
    }

    fn specify_grain(&mut self, request: &Request) -> (i64, i64) {
        let chunk = self.0.specify_grain(&to_cpp(request));
        (chunk.begin as i64, chunk.end as i64)
    }

    fn analyse_grain(&mut self, data: &mut [f32], stride: usize) {
        self.0.analyse_grain(data, stride);
    }

    fn synthesise_grain<R>(
        &mut self,
        f: impl FnOnce(&[f32], usize, usize, [Option<Request>; 2]) -> R,
    ) -> R {
        let mut empty: [f32; 0] = [];
        let mut out = bungee_rs::OutputChunk::new(&mut empty, 0);
        self.0.synthesise_grain(&mut out);
        let request = out.request.map(|r| r.as_ref().map(from_cpp));
        f(out.data, out.frame_count, out.channel_stride, request)
    }
}

/// The pure-Rust port.
pub struct Rust(port::Stretcher);

impl Rust {
    pub fn new(sample_rate: u32) -> Self {
        let rates = port::SampleRates {
            input: sample_rate,
            output: sample_rate,
        };
        Self(port::Stretcher::new(rates, CHANNELS))
    }
}

impl GrainApi for Rust {
    fn max_input_frame_count(&self) -> usize {
        self.0.max_input_frame_count()
    }

    fn preroll(&mut self, request: &mut Request) {
        self.0.preroll(request);
    }

    fn next(&mut self, request: &mut Request) {
        self.0.next(request);
    }

    fn is_flushed(&self) -> bool {
        self.0.is_flushed()
    }

    fn specify_grain(&mut self, request: &Request) -> (i64, i64) {
        let chunk = self.0.specify_grain(request, 0.0);
        (chunk.begin, chunk.end)
    }

    fn analyse_grain(&mut self, data: &mut [f32], stride: usize) {
        self.0.analyse_grain(data, stride, 0, 0);
    }

    fn synthesise_grain<R>(
        &mut self,
        f: impl FnOnce(&[f32], usize, usize, [Option<Request>; 2]) -> R,
    ) -> R {
        let out = self.0.synthesise_grain();
        f(
            out.data,
            out.frame_count,
            out.channel_stride,
            out.request.map(Some),
        )
    }
}

pub struct Bungee<S: GrainApi> {
    name: &'static str,
    stretcher: S,
    request: Request,
    input: Vec<f32>,
    stride: usize,
    fifo: Fifo,
    /// Output before this position (in the playing direction) is pre-roll and is dropped.
    start: f64,
    started: bool,
    playhead: f64,
    read_ahead: f64,
    underruns: u64,
}

impl Bungee<Cpp> {
    pub fn cpp(sample_rate: u32) -> Self {
        Self::with("bungee", Cpp::new(sample_rate))
    }
}

impl Bungee<Rust> {
    pub fn rust(sample_rate: u32) -> Self {
        Self::with("bungee-rust", Rust::new(sample_rate))
    }
}

impl<S: GrainApi> Bungee<S> {
    fn with(name: &'static str, stretcher: S) -> Self {
        let stride = stretcher.max_input_frame_count();
        Self {
            name,
            stretcher,
            request: Request {
                position: f64::NAN,
                speed: 1.0,
                pitch: 1.0,
                reset: true,
            },
            input: vec![0.0; stride * CHANNELS],
            stride,
            fifo: Fifo::new(FIFO_FRAMES),
            start: 0.0,
            started: false,
            playhead: 0.0,
            read_ahead: 0.0,
            underruns: 0,
        }
    }

    fn grain(&mut self, track: &Track) {
        let (begin, end) = self.stretcher.specify_grain(&self.request);
        let frames = ((end - begin).max(0) as usize).min(self.stride);
        track.copy_planar(begin, frames, &mut self.input, self.stride);
        self.stretcher.analyse_grain(&mut self.input, self.stride);
        let (fifo, started, start, read_ahead) = (
            &mut self.fifo,
            &mut self.started,
            self.start,
            &mut self.read_ahead,
        );
        self.stretcher
            .synthesise_grain(|data, frame_count, channel_stride, request| {
                let [Some(r0), Some(r1)] = request else {
                    return;
                };
                if r0.position.is_nan() || r1.position.is_nan() || frame_count == 0 {
                    return;
                }
                let n = frame_count;
                let forward = r1.position >= r0.position;
                let mut skip = 0;
                if !*started {
                    let before = if forward {
                        start - r0.position
                    } else {
                        r0.position - start
                    };
                    let span = (r1.position - r0.position).abs();
                    if before >= span {
                        skip = n;
                    } else {
                        *started = true;
                        if before > 0.0 && span > 0.0 {
                            skip = ((before / span) * n as f64).round() as usize;
                        }
                    }
                }
                if skip < n {
                    let take = (n - skip).min(fifo.room());
                    fifo.push_planar(&data[skip..], take, channel_stride);
                }
                let ahead = if forward {
                    end as f64 - r0.position
                } else {
                    r0.position - begin as f64
                };
                *read_ahead = read_ahead.max(ahead);
            });
        self.stretcher.next(&mut self.request);
    }
}

impl<S: GrainApi> Candidate for Bungee<S> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn reverse(&self) -> bool {
        true
    }

    fn start(&mut self, track: &Track, position: f64, rate: f64) {
        // Grains still in flight from the previous run come out tagged with its positions;
        // flush them with invalid (NaN) grains first.
        self.request = Request {
            position: f64::NAN,
            speed: rate,
            pitch: 1.0,
            reset: true,
        };
        for _ in 0..16 {
            if self.stretcher.is_flushed() {
                break;
            }
            self.grain(track);
        }
        self.request = Request {
            position,
            speed: rate,
            pitch: 1.0,
            reset: true,
        };
        self.stretcher.preroll(&mut self.request);
        self.fifo.clear();
        self.start = position;
        self.started = false;
        self.read_ahead = 0.0;
        // Run through the pre-roll grains now, so `render` only does steady-state work.
        for _ in 0..64 {
            if self.started {
                break;
            }
            self.grain(track);
        }
        self.playhead = position;
    }

    fn render(&mut self, track: &Track, rate: f64, out: &mut [f32]) {
        let n_out = out.len() / CHANNELS;
        self.request.speed = rate;
        for _ in 0..64 {
            if self.fifo.len() >= n_out {
                break;
            }
            self.grain(track);
        }
        self.underruns += self.fifo.pop(out) as u64;
        self.playhead += rate * n_out as f64;
    }

    fn read_ahead(&self) -> f64 {
        self.read_ahead
    }

    fn underruns(&self) -> u64 {
        self.underruns
    }
}
