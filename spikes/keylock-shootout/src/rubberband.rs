//! Rubber Band Library 4.0 (GPL-2.0-or-later) in real-time mode, through the raw C API.
//!
//! The `rubberband` crate's safe wrapper collects a `Vec` of channel pointers on every
//! `process` and `retrieve`, so it allocates in the audio path; this calls `rubberband-sys`
//! directly with preallocated planes instead.
//!
//! Rubber Band produces output in chunks of its own size, so a FIFO sits between it and the
//! fixed 128-frame blocks. Alignment follows the library's documented recipe: feed
//! `preferred_start_pad` frames before the start position (the real audio there, not silence)
//! and drop `start_delay` frames of output.
#![allow(unsafe_code)]

use std::ptr::NonNull;

use rubberband_sys as rb;

use crate::candidate::Candidate;
use crate::track::{CHANNELS, Fifo, Track};

/// Largest input chunk passed to one `process` call (also set as the max process size).
const MAX_PROCESS: usize = 8192;
const FIFO_FRAMES: usize = 4 * MAX_PROCESS;

#[derive(Clone, Copy)]
pub enum Engine {
    /// R2, `OptionEngineFaster`.
    R2,
    /// R3, `OptionEngineFiner`.
    R3,
    /// R3 with `OptionWindowShort`: less CPU and latency.
    R3Short,
}

pub struct RubberBand {
    name: &'static str,
    state: NonNull<rb::RubberBandState_>,
    fifo: Fifo,
    input: Vec<f32>,
    output: Vec<f32>,
    cursor: i64,
    playhead: f64,
    rate: f64,
    discard: usize,
    underruns: u64,
}

// SAFETY: the state is only ever used through `&mut self`, from one thread at a time.
unsafe impl Send for RubberBand {}

impl RubberBand {
    pub fn new(engine: Engine, sample_rate: u32) -> Self {
        let (name, flags) = match engine {
            Engine::R2 => (
                "rubberband-r2",
                rb::RubberBandOption_RubberBandOptionEngineFaster,
            ),
            Engine::R3 => (
                "rubberband-r3",
                rb::RubberBandOption_RubberBandOptionEngineFiner,
            ),
            Engine::R3Short => (
                "rubberband-r3-short",
                rb::RubberBandOption_RubberBandOptionEngineFiner
                    | rb::RubberBandOption_RubberBandOptionWindowShort,
            ),
        };
        let options = rb::RubberBandOption_RubberBandOptionProcessRealTime
            | rb::RubberBandOption_RubberBandOptionThreadingNever
            | flags;
        // SAFETY: plain constructor call; null is checked below.
        let raw =
            unsafe { rb::rubberband_new(sample_rate, CHANNELS as u32, options as i32, 1.0, 1.0) };
        let state = NonNull::new(raw).expect("rubberband_new failed");
        // SAFETY: `state` is valid until drop.
        unsafe { rb::rubberband_set_max_process_size(state.as_ptr(), MAX_PROCESS as u32) };
        Self {
            name,
            state,
            fifo: Fifo::new(FIFO_FRAMES),
            input: vec![0.0; MAX_PROCESS * CHANNELS],
            output: vec![0.0; MAX_PROCESS * CHANNELS],
            cursor: 0,
            playhead: 0.0,
            rate: 1.0,
            discard: 0,
            underruns: 0,
        }
    }

    fn ptr(&self) -> rb::RubberBandState {
        self.state.as_ptr()
    }

    /// Feed `frames` (at most [`MAX_PROCESS`]) from `first`, then collect what came out.
    fn feed(&mut self, track: &Track, first: i64, frames: usize) {
        track.copy_planar(first, frames, &mut self.input, MAX_PROCESS);
        let planes = [self.input.as_ptr(), self.input[MAX_PROCESS..].as_ptr()];
        // SAFETY: both planes hold at least `frames` samples; the state is valid.
        unsafe { rb::rubberband_process(self.ptr(), planes.as_ptr(), frames as u32, 0) };
        self.collect();
    }

    fn collect(&mut self) {
        loop {
            // SAFETY: valid state.
            let available = unsafe { rb::rubberband_available(self.ptr()) };
            let n = (available.max(0) as usize)
                .min(MAX_PROCESS)
                .min(self.fifo.room() + self.discard);
            if n == 0 {
                return;
            }
            let planes = [
                self.output.as_mut_ptr(),
                self.output[MAX_PROCESS..].as_mut_ptr(),
            ];
            // SAFETY: both planes have room for `n <= MAX_PROCESS` samples.
            let got =
                unsafe { rb::rubberband_retrieve(self.ptr(), planes.as_ptr(), n as u32) } as usize;
            let skip = got.min(self.discard);
            self.discard -= skip;
            self.fifo
                .push_planar(&self.output[skip..], got - skip, MAX_PROCESS);
        }
    }
}

impl Drop for RubberBand {
    fn drop(&mut self) {
        // SAFETY: created by rubberband_new, deleted once.
        unsafe { rb::rubberband_delete(self.ptr()) };
    }
}

impl Candidate for RubberBand {
    fn name(&self) -> &'static str {
        self.name
    }

    fn start(&mut self, track: &Track, position: f64, rate: f64) {
        let p = position.round() as i64;
        // SAFETY: valid state for all calls below.
        let (pad, delay) = unsafe {
            rb::rubberband_reset(self.ptr());
            rb::rubberband_set_time_ratio(self.ptr(), 1.0 / rate);
            (
                rb::rubberband_get_preferred_start_pad(self.ptr()) as i64,
                rb::rubberband_get_start_delay(self.ptr()) as usize,
            )
        };
        self.fifo.clear();
        self.discard = delay;
        self.rate = rate;
        let mut at = p - pad;
        while at < p {
            let n = ((p - at) as usize).min(MAX_PROCESS);
            self.feed(track, at, n);
            at += n as i64;
        }
        self.cursor = p;
        self.playhead = position;
    }

    fn render(&mut self, track: &Track, rate: f64, out: &mut [f32]) {
        let n_out = out.len() / CHANNELS;
        if rate != self.rate {
            // SAFETY: valid state.
            unsafe { rb::rubberband_set_time_ratio(self.ptr(), 1.0 / rate) };
            self.rate = rate;
        }
        // Bounded: each pass feeds what the library asks for; give up after a few passes.
        for _ in 0..64 {
            if self.fifo.len() >= n_out {
                break;
            }
            // SAFETY: valid state.
            let required = unsafe { rb::rubberband_get_samples_required(self.ptr()) } as usize;
            let n = if required == 0 {
                256
            } else {
                required.min(MAX_PROCESS)
            };
            self.feed(track, self.cursor, n);
            self.cursor += n as i64;
        }
        self.underruns += self.fifo.pop(out) as u64;
        self.playhead += rate * n_out as f64;
    }

    fn read_ahead(&self) -> f64 {
        self.cursor as f64 - self.playhead
    }

    fn underruns(&self) -> u64 {
        self.underruns
    }
}
