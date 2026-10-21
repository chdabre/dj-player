//! Bungee Basic (MPL-2.0) through its native, pull-based grain API.
//!
//! Each grain asks for a position and speed; Bungee says which input range it needs, we hand
//! it that range straight from the resident track, and it returns a chunk of output tagged
//! with the positions it covers. Positions may move backwards, so reverse keeps the pitch too.

use bungee_rs::{OutputChunk, Request, Stretcher};

use crate::candidate::Candidate;
use crate::track::{CHANNELS, Fifo, Track};

const FIFO_FRAMES: usize = 16_384;

pub struct Bungee {
    stretcher: Stretcher,
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

impl Bungee {
    pub fn new(sample_rate: u32) -> Self {
        let stretcher = Stretcher::new(sample_rate as usize, CHANNELS).expect("bungee stretcher");
        let stride = stretcher.max_input_frame_count();
        Self {
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
        let chunk = self.stretcher.specify_grain(&self.request);
        let frames = chunk.len().min(self.stride);
        track.copy_planar(chunk.begin as i64, frames, &mut self.input, self.stride);
        self.stretcher.analyse_grain(&mut self.input, self.stride);
        let mut empty: [f32; 0] = [];
        let mut out = OutputChunk::new(&mut empty, 0);
        self.stretcher.synthesise_grain(&mut out);

        if let [Some(r0), Some(r1)] = out.request
            && !r0.position.is_nan()
            && !r1.position.is_nan()
            && out.frame_count > 0
        {
            let n = out.frame_count;
            let forward = r1.position >= r0.position;
            let mut skip = 0;
            if !self.started {
                let before = if forward {
                    self.start - r0.position
                } else {
                    r0.position - self.start
                };
                let span = (r1.position - r0.position).abs();
                if before >= span {
                    skip = n;
                } else {
                    self.started = true;
                    if before > 0.0 && span > 0.0 {
                        skip = ((before / span) * n as f64).round() as usize;
                    }
                }
            }
            if skip < n {
                let take = (n - skip).min(self.fifo.room());
                self.fifo
                    .push_planar(&out.data[skip..], take, out.channel_stride);
            }
            let ahead = if forward {
                chunk.end as f64 - r0.position
            } else {
                r0.position - chunk.begin as f64
            };
            self.read_ahead = self.read_ahead.max(ahead);
        }
        self.stretcher.next(&mut self.request);
    }
}

impl Candidate for Bungee {
    fn name(&self) -> &'static str {
        "bungee"
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
