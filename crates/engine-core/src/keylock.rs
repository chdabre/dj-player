//! `Keylock`: the keylock path. Plays a transport [`Segment`] through a time-stretcher so
//! the rate changes speed but not pitch.
//!
//! Two backends:
//!
//! - **Bungee** (default, [`Keylock::bungee`]): the pure-Rust port in
//!   [`engine_stretch::bungee`], driven through its grain API. Plays any non-zero velocity up
//!   to [`MAX_STRETCH_RATE`] in either direction, so reverse keeps the pitch too.
//! - **Push** ([`Keylock::new`]): any [`Stretcher`] (Signalsmith), fed input frames ahead of
//!   the playhead. Forward rates only.
//!
//! Either way the audio that comes out at a given moment is the audio at
//! [`crate::Transport::position`], so waveforms, cues and later sync need no keylock offset.
//!
//! # Bungee: grains placed where the playhead will be
//!
//! Bungee is pull-based: each grain asks for an input position and returns one synthesis
//! hop of output, two grains later, tagged with the positions it covers. The path keeps an
//! output clock and places each grain at the position the transport will have when that
//! grain's output plays, assuming the current velocity holds. Input comes straight from the
//! resident [`Source`]. Output goes through a preallocated FIFO with a position per frame.
//!
//! - **Jumps** (seek, loop wrap, slip snap-back, first block): the segment does not continue
//!   the last one, so the path re-primes: one pre-roll grain before the new position, run
//!   until the first chunk at it, earlier output dropped. The next frame is exactly the new
//!   position. The old audio still in the FIFO is crossfaded out over [`JUMP_FADE_FRAMES`].
//! - **Rate changes** need no correction: grains already requested keep the old velocity
//!   (about 30 ms of output). The next grain is placed on the new trajectory, and Bungee works
//!   the hop out from the position difference, so the audio rejoins the playhead within one
//!   grain.
//!
//! # Push: latency compensation
//!
//! A push stretcher's output lags its input by `input_latency + output_latency * rate` input
//! frames (the *lookahead*). Because the whole track is resident, the path simply reads that
//! far ahead of the transport.
//!
//! - **Jumps**: [`Keylock::render`] primes the stretcher at the new position. It seeks with the
//!   audio before it, then runs and discards one output latency of audio, so the very next
//!   output frame is the new position.
//! - **Rate changes**: the lookahead depends on the rate. It is slewed towards the new value
//!   by at most [`LOOKAHEAD_SLEW`] of each block, so the audio catches up with the playhead
//!   over a fraction of a second instead of jumping.
//!
//! The push path only runs forward at rates in `(0, MAX_STRETCH_RATE]`. Reverse, rate 0 and
//! anything faster fall back to the resampler ([`crate::Scaler`]).

use engine_stretch::Stretcher;
use engine_stretch::bungee::{self, Request, SampleRates};

use crate::deck::MAX_BLOCK_FRAMES;
use crate::source::{CHANNELS, FrameSource, Source};
use crate::transport::Segment;

/// Fastest rate the stretcher plays; above it the deck uses the resampler.
pub const MAX_STRETCH_RATE: f64 = 4.0;

/// Largest lookahead correction per block of the push path, as a fraction of the block
/// length (1 % speed).
pub const LOOKAHEAD_SLEW: f64 = 0.01;

/// Crossfade from the old audio when the Bungee path re-primes at a jump. The deck's own
/// declick covers jumps from commands; this covers loop wraps.
pub const JUMP_FADE_FRAMES: usize = 64;

/// Playheads closer than this (frames) count as the same position.
const ALIGN_EPSILON: f64 = 1e-6;

/// If the audio at the FIFO head is further than this (frames) from the playhead, something
/// went wrong (it should never be more than a few thousand after a rate change): re-prime.
const MAX_DRIFT: f64 = 65_536.0;

pub struct Keylock {
    path: Path,
}

enum Path {
    Bungee(Box<GrainPath>),
    Push(PushPath),
}

impl std::fmt::Debug for Keylock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.path {
            Path::Bungee(g) => f
                .debug_struct("Keylock::Bungee")
                .field("cursor", &g.cursor)
                .field("clock", &g.clock)
                .field("emit", &g.emit)
                .field("fifo", &g.fifo.len)
                .finish_non_exhaustive(),
            Path::Push(p) => f
                .debug_struct("Keylock::Push")
                .field("cursor", &p.cursor)
                .field("fed_end", &p.fed_end)
                .field("lookahead", &p.lookahead)
                .finish_non_exhaustive(),
        }
    }
}

impl Keylock {
    /// The default path: Bungee, stereo at `sample_rate`. Allocates; call off the audio
    /// thread.
    pub fn bungee(sample_rate: u32) -> Self {
        Self {
            path: Path::Bungee(Box::new(GrainPath::new(sample_rate))),
        }
    }

    /// The push path around a stereo stretcher (e.g. Signalsmith). Allocates the scratch
    /// space; call off the audio thread.
    pub fn new(stretcher: Box<dyn Stretcher>) -> Self {
        Self {
            path: Path::Push(PushPath::new(stretcher)),
        }
    }

    /// True if this path plays a segment with this step (frames per output frame).
    pub fn handles(&self, step: f64) -> bool {
        match self.path {
            Path::Bungee(_) => step != 0.0 && step.abs() <= MAX_STRETCH_RATE,
            Path::Push(_) => step > 0.0 && step <= MAX_STRETCH_RATE,
        }
    }

    /// True if the next output continues seamlessly from `position`, with no priming.
    pub fn is_aligned(&self, position: f64) -> bool {
        let cursor = match &self.path {
            Path::Bungee(g) => g.cursor,
            Path::Push(p) => p.cursor,
        };
        cursor.is_some_and(|c| (c - position).abs() < ALIGN_EPSILON)
    }

    /// Forget the stretcher's position (new track); the next render primes.
    pub fn invalidate(&mut self) {
        match &mut self.path {
            Path::Bungee(g) => g.cursor = None,
            Path::Push(p) => p.cursor = None,
        }
    }

    /// Render `out.len() / 2` frames of `segment` (whose step must satisfy
    /// [`Keylock::handles`]), overwriting `out`. Primes first if the stretcher is not at
    /// `segment.start`.
    ///
    /// Real-time safe. Returns `false`, with `out` untouched and the stretcher unchanged, if a
    /// frame it needs is not loaded yet.
    pub fn render(&mut self, source: &Source, segment: Segment, out: &mut [f32]) -> bool {
        debug_assert!(
            self.handles(segment.step),
            "keylock cannot play step {}",
            segment.step
        );
        match &mut self.path {
            Path::Bungee(g) => g.render(source, segment, out),
            Path::Push(p) => p.render(source, segment, out),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Bungee

/// Fixed-capacity FIFO of interleaved stereo frames, each with the source position it plays.
struct Fifo {
    audio: Box<[f32]>,
    position: Box<[f64]>,
    head: usize,
    len: usize,
}

impl Fifo {
    fn new(frames: usize) -> Self {
        Self {
            audio: vec![0.0; frames * CHANNELS].into_boxed_slice(),
            position: vec![0.0; frames].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.position.len()
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Position of the next frame out, if any.
    fn head_position(&self) -> Option<f64> {
        (self.len > 0).then(|| self.position[self.head])
    }

    fn push(&mut self, frame: [f32; CHANNELS], position: f64) {
        assert!(self.len < self.capacity(), "keylock fifo overflow");
        let at = (self.head + self.len) % self.capacity();
        self.audio[at * CHANNELS..(at + 1) * CHANNELS].copy_from_slice(&frame);
        self.position[at] = position;
        self.len += 1;
    }

    /// Copy the first `out.len() / 2` frames into `out` without removing them.
    fn peek(&self, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        debug_assert!(frames <= self.len);
        for (k, dst) in out.chunks_exact_mut(CHANNELS).enumerate().take(frames) {
            let at = (self.head + k) % self.capacity();
            dst.copy_from_slice(&self.audio[at * CHANNELS..(at + 1) * CHANNELS]);
        }
    }

    /// Fill `out` from the front and remove those frames.
    fn pop(&mut self, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        assert!(frames <= self.len, "keylock fifo underrun");
        self.peek(out);
        self.head = (self.head + frames) % self.capacity();
        self.len -= frames;
    }
}

struct GrainPath {
    stretcher: bungee::Stretcher,
    hop: i64,
    /// Planar grain input, one channel per `stride` floats.
    input: Box<[f32]>,
    stride: usize,
    fifo: Fifo,
    /// Output clock (frames) at the start of the next block.
    clock: i64,
    /// Output clock at which the next synthesised chunk begins.
    emit: i64,
    /// Transport position the next output frame corresponds to, if primed.
    cursor: Option<f64>,
    /// Old audio to crossfade from after a jump.
    fade: Box<[f32]>,
}

impl GrainPath {
    fn new(sample_rate: u32) -> Self {
        let rates = SampleRates {
            input: sample_rate,
            output: sample_rate,
        };
        let stretcher = bungee::Stretcher::new(rates, CHANNELS);
        let hop = stretcher.synthesis_hop();
        let stride = stretcher.max_input_frame_count();
        // The FIFO holds at most one block, the fade lead and one chunk beyond.
        let fifo = MAX_BLOCK_FRAMES + JUMP_FADE_FRAMES + 2 * stretcher.max_output_frame_count();
        Self {
            stretcher,
            hop: hop as i64,
            input: vec![0.0; stride * CHANNELS].into_boxed_slice(),
            stride,
            fifo: Fifo::new(fifo),
            clock: 0,
            emit: 0,
            cursor: None,
            fade: vec![0.0; JUMP_FADE_FRAMES * CHANNELS].into_boxed_slice(),
        }
    }

    /// Position for the grain specified while the chunk at `emit` comes out: its own output
    /// starts two chunks later. `segment` holds from the block start at `clock`.
    fn grain_position(&self, segment: Segment, emit: i64) -> f64 {
        segment.start + segment.step * (emit + 2 * self.hop - self.clock) as f64
    }

    fn render(&mut self, source: &Source, segment: Segment, out: &mut [f32]) -> bool {
        let frames = out.len() / CHANNELS;
        let aligned = self
            .cursor
            .is_some_and(|c| (c - segment.start).abs() < ALIGN_EPSILON);
        // A NaN drift counts as drifted too.
        let drifted = self
            .fifo
            .head_position()
            .map(|p| (p - segment.start).abs())
            .is_some_and(|d| d.is_nan() || d > MAX_DRIFT);
        let prime = !aligned || drifted;

        // Plan the grains: from the next chunk (or a pre-roll grain three chunks before the
        // block, on a prime) until the FIFO covers the block plus the fade lead.
        let first_emit = if prime {
            self.clock - 3 * self.hop
        } else {
            self.emit
        };
        let need = self.clock + (frames + JUMP_FADE_FRAMES) as i64;
        let grains = ((need - first_emit).max(0) as u64).div_ceil(self.hop as u64) as i64;
        if grains > 0 {
            let request = |emit: i64| Request {
                position: self.grain_position(segment, emit),
                speed: segment.step,
                pitch: 1.0,
                reset: false,
            };
            let a = self.stretcher.input_chunk(&request(first_emit), 0.0);
            let last = first_emit + (grains - 1) * self.hop;
            let b = self.stretcher.input_chunk(&request(last), 0.0);
            let (lo, hi) = (a.begin.min(b.begin), a.end.max(b.end) - 1);
            if !source.range_ready(lo, hi) {
                return false;
            }
        }

        let mut fade_len = 0;
        if prime {
            if self.cursor.is_some() {
                fade_len = self.fifo.len.min(JUMP_FADE_FRAMES).min(frames);
                self.fifo.peek(&mut self.fade[..fade_len * CHANNELS]);
            }
            self.fifo.clear();
            self.emit = first_emit;
        }

        for g in 0..grains {
            let request = Request {
                position: self.grain_position(segment, self.emit),
                speed: segment.step,
                pitch: 1.0,
                reset: prime && g == 0,
            };
            self.grain(source, &request);
        }

        self.fifo.pop(out);
        self.clock += frames as i64;
        self.cursor = Some(segment.start + segment.step * frames as f64);

        if fade_len > 0 {
            let inv = 1.0 / fade_len as f32;
            for (k, (o, s)) in out
                .chunks_exact_mut(CHANNELS)
                .zip(self.fade.chunks_exact(CHANNELS))
                .take(fade_len)
                .enumerate()
            {
                let g = (k + 1) as f32 * inv;
                for c in 0..CHANNELS {
                    o[c] = s[c] + (o[c] - s[c]) * g;
                }
            }
        }
        true
    }

    /// Run one grain and queue the part of its output chunk at or after the clock.
    fn grain(&mut self, source: &Source, request: &Request) {
        let chunk = self.stretcher.specify_grain(request, 0.0);
        let n = chunk.len().min(self.stride);
        for k in 0..n {
            let frame = source.frame(chunk.begin + k as i64);
            for (c, s) in frame.iter().enumerate() {
                self.input[c * self.stride + k] = *s;
            }
        }
        self.stretcher.analyse_grain(&self.input, self.stride, 0, 0);
        let output = self.stretcher.synthesise_grain();
        let [r0, r1] = output.request;
        let count = output.frame_count;
        let step = (r1.position - r0.position) / count as f64;
        for k in 0..count {
            let at = self.emit + k as i64;
            if at < self.clock {
                continue;
            }
            let frame = std::array::from_fn(|c| output.data[c * output.channel_stride + k]);
            self.fifo.push(frame, r0.position + step * k as f64);
        }
        self.emit += count as i64;
    }
}

// ---------------------------------------------------------------------------------------------
// Push (Signalsmith)

struct PushPath {
    stretcher: Box<dyn Stretcher>,
    /// Transport position the stretcher's next output frame corresponds to, if primed.
    cursor: Option<f64>,
    /// One past the last source frame fed to the stretcher.
    fed_end: i64,
    /// `fed_end - cursor`: how far ahead of the playhead the input runs.
    lookahead: f64,
    input: Box<[f32]>,
    discard: Box<[f32]>,
}

impl PushPath {
    fn new(stretcher: Box<dyn Stretcher>) -> Self {
        assert_eq!(
            stretcher.channels(),
            CHANNELS,
            "keylock needs a stereo stretcher"
        );
        let per_block = (MAX_BLOCK_FRAMES as f64 * MAX_STRETCH_RATE * (1.0 + LOOKAHEAD_SLEW)).ceil()
            as usize
            + 2;
        let frames = stretcher.preroll_frames().max(per_block);
        Self {
            stretcher,
            cursor: None,
            fed_end: 0,
            lookahead: 0.0,
            input: vec![0.0; frames * CHANNELS].into_boxed_slice(),
            discard: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
        }
    }

    /// Lookahead the stretcher needs at `rate` for its output to match the playhead.
    fn latency_at(&self, rate: f64) -> f64 {
        self.stretcher.input_latency() as f64 + self.stretcher.output_latency() as f64 * rate
    }

    fn is_aligned(&self, position: f64) -> bool {
        self.cursor
            .is_some_and(|c| (c - position).abs() < ALIGN_EPSILON)
    }

    fn render(&mut self, source: &Source, segment: Segment, out: &mut [f32]) -> bool {
        let frames = out.len() / CHANNELS;
        let rate = segment.step;
        if !self.is_aligned(segment.start) && !self.prime(source, segment.start, rate) {
            return false;
        }
        let slew = frames as f64 * LOOKAHEAD_SLEW;
        let lookahead =
            self.lookahead + (self.latency_at(rate) - self.lookahead).clamp(-slew, slew);
        let end = segment.start + rate * frames as f64;
        let capacity = (self.input.len() / CHANNELS) as i64;
        let n = ((end + lookahead).round() as i64 - self.fed_end).clamp(0, capacity);
        if n > 0 && !source.range_ready(self.fed_end, self.fed_end + n - 1) {
            return false;
        }
        let input = fill(&mut self.input, source, self.fed_end, n as usize);
        self.stretcher.process(input, out);
        self.fed_end += n;
        self.lookahead = self.fed_end as f64 - end;
        self.cursor = Some(end);
        true
    }

    /// Put the stretcher at `position` so its next output frame is that position.
    fn prime(&mut self, source: &Source, position: f64, rate: f64) -> bool {
        let preroll = self.stretcher.preroll_frames();
        let out_latency = self.stretcher.output_latency();
        let start = position.round() as i64 + self.stretcher.input_latency() as i64;
        let discard_in = (out_latency as f64 * rate).round() as i64;
        if !source.range_ready(start - preroll as i64, start + discard_in) {
            return false;
        }
        self.stretcher.reset();
        let input = fill(&mut self.input, source, start - preroll as i64, preroll);
        self.stretcher.seek(input, rate);

        // Run off the output latency, so what comes next is `position`.
        let mut fed = start;
        let mut done = 0;
        while done < out_latency {
            let n_out = (out_latency - done).min(MAX_BLOCK_FRAMES);
            let target = start + ((done + n_out) as f64 * rate).round() as i64;
            let input = fill(&mut self.input, source, fed, (target - fed) as usize);
            self.stretcher
                .process(input, &mut self.discard[..n_out * CHANNELS]);
            fed = target;
            done += n_out;
        }
        self.fed_end = fed;
        self.lookahead = fed as f64 - position;
        self.cursor = Some(position);
        true
    }
}

/// Copy `frames` source frames starting at `first` into `buf`; return the filled part.
fn fill<'a>(buf: &'a mut [f32], source: &Source, first: i64, frames: usize) -> &'a [f32] {
    let dst = &mut buf[..frames * CHANNELS];
    for (k, frame) in dst.chunks_exact_mut(CHANNELS).enumerate() {
        frame.copy_from_slice(&source.frame(first + k as i64));
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_stretch::SignalsmithStretcher;

    const SR: u32 = 48_000;

    fn push_keylock() -> Keylock {
        Keylock::new(Box::new(SignalsmithStretcher::new(CHANNELS, SR)))
    }

    fn push(k: &Keylock) -> &PushPath {
        match &k.path {
            Path::Push(p) => p,
            Path::Bungee(_) => panic!("not the push path"),
        }
    }

    fn grains(k: &Keylock) -> &GrainPath {
        match &k.path {
            Path::Bungee(g) => g,
            Path::Push(_) => panic!("not the Bungee path"),
        }
    }

    #[test]
    fn bungee_handles_both_directions() {
        let k = Keylock::bungee(SR);
        assert!(k.handles(1.0));
        assert!(k.handles(0.01));
        assert!(k.handles(-1.0));
        assert!(k.handles(-MAX_STRETCH_RATE));
        assert!(k.handles(MAX_STRETCH_RATE));
        assert!(!k.handles(0.0));
        assert!(!k.handles(MAX_STRETCH_RATE + 0.1));
        assert!(!k.handles(-MAX_STRETCH_RATE - 0.1));
    }

    #[test]
    fn push_handles_forward_rates_only() {
        let k = push_keylock();
        assert!(k.handles(1.0));
        assert!(k.handles(0.01));
        assert!(k.handles(MAX_STRETCH_RATE));
        assert!(!k.handles(0.0));
        assert!(!k.handles(-1.0));
        assert!(!k.handles(MAX_STRETCH_RATE + 0.1));
    }

    #[test]
    fn bungee_primes_once_and_its_output_is_the_playhead() {
        let src = Source::from_interleaved(SR, &vec![0.1; 10 * SR as usize * CHANNELS]);
        let mut k = Keylock::bungee(SR);
        let mut out = vec![0.0; 128 * CHANNELS];
        let mut pos = 20_000.0;
        for rate in [1.08, 1.08, -0.5, 2.0] {
            for _ in 0..50 {
                let seg = Segment {
                    start: pos,
                    step: rate,
                };
                assert!(k.render(&src, seg, &mut out));
                pos += rate * 128.0;
                assert!(k.is_aligned(pos));
            }
            // After the grains requested at the old rate are out, the next frame plays the
            // playhead exactly.
            let head = grains(&k).fifo.head_position().unwrap();
            assert!((head - pos).abs() < 1e-6, "rate {rate}: {head} vs {pos}");
        }
    }

    #[test]
    fn push_primes_once_then_stays_aligned() {
        let src = Source::from_interleaved(SR, &vec![0.1; 10 * SR as usize * CHANNELS]);
        let mut k = push_keylock();
        let mut out = vec![0.0; 128 * CHANNELS];
        let seg = Segment {
            start: 1000.0,
            step: 1.08,
        };
        assert!(k.render(&src, seg, &mut out));
        assert!(k.is_aligned(1000.0 + 1.08 * 128.0));
        let p = push(&k);
        assert!((p.lookahead - p.latency_at(1.08)).abs() <= 1.0);
    }

    #[test]
    fn push_lookahead_follows_rate_changes_gradually() {
        let src = Source::from_interleaved(SR, &vec![0.1; 30 * SR as usize * CHANNELS]);
        let mut k = push_keylock();
        let mut out = vec![0.0; 128 * CHANNELS];
        let mut pos = 0.0;
        let mut play = |k: &mut Keylock, rate: f64| {
            assert!(k.render(
                &src,
                Segment {
                    start: pos,
                    step: rate
                },
                &mut out
            ));
            pos += rate * 128.0;
        };
        play(&mut k, 1.0);
        play(&mut k, 1.08);
        let jump = push(&k).lookahead - push(&k).latency_at(1.0);
        assert!(jump <= 128.0 * LOOKAHEAD_SLEW + 1.0, "{jump}");
        for _ in 0..400 {
            play(&mut k, 1.08);
        }
        let p = push(&k);
        assert!((p.lookahead - p.latency_at(1.08)).abs() <= 1.0);
    }

    #[test]
    fn unloaded_frames_starve_without_touching_state() {
        for mut k in [Keylock::bungee(SR), push_keylock()] {
            let (src, _writer) = Source::new(SR, SR as usize);
            let mut out = vec![7.0; 128 * CHANNELS];
            let seg = Segment {
                start: 0.0,
                step: 1.0,
            };
            assert!(!k.render(&src, seg, &mut out));
            assert!(out.iter().all(|&s| s == 7.0));
            assert!(!k.is_aligned(0.0));
        }
    }
}
