//! `Keylock`: the keylock path. Plays a transport [`Segment`] through a time-stretcher so
//! the rate changes speed but not pitch.
//!
//! The stretcher is the pure-Rust Bungee port in [`engine_stretch::bungee`], driven through
//! its grain API. It plays any non-zero velocity up to [`MAX_STRETCH_RATE`] in either
//! direction, so reverse keeps the pitch too; rate 0 and anything faster go to the resampler
//! ([`crate::Scaler`]). The audio that comes out at a given moment is the audio at
//! [`crate::Transport::position`], so waveforms, cues and later sync need no keylock offset.
//!
//! # Grains placed where the playhead will be
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
//!   position. On a jump the deck does not declick itself (a loop wrap), the old audio
//!   still in the FIFO is crossfaded out over [`JUMP_FADE_FRAMES`]; commanded jumps get only
//!   the deck's declick, so the two fades never stack.
//! - **Rate changes** need no correction: grains already requested keep the old velocity
//!   (about 30 ms of output). The next grain is placed on the new trajectory, and Bungee works
//!   the hop out from the position difference, so the audio rejoins the playhead within one
//!   grain.

use engine_stretch::bungee::{self, Request, SampleRates};

use crate::deck::MAX_BLOCK_FRAMES;
use crate::source::{CHANNELS, FrameSource, Source};
use crate::transport::Segment;

/// Fastest rate the stretcher plays; above it the deck uses the resampler.
pub const MAX_STRETCH_RATE: f64 = 4.0;

/// Crossfade from the old audio when the path re-primes at a jump the deck does not declick
/// itself (loop wraps). It carries over into the next block if the jump is near the end of
/// one.
pub const JUMP_FADE_FRAMES: usize = 64;

/// Playheads closer than this (frames) count as the same position.
const ALIGN_EPSILON: f64 = 1e-6;

/// If the audio at the FIFO head is further than this (frames) from the playhead, something
/// went wrong (it should never be more than a few thousand after a rate change): re-prime.
const MAX_DRIFT: f64 = 65_536.0;

/// The keylock path of one deck. See the [module docs](self).
pub struct Keylock {
    path: Box<GrainPath>,
}

impl std::fmt::Debug for Keylock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = &self.path;
        f.debug_struct("Keylock")
            .field("cursor", &g.cursor)
            .field("clock", &g.clock)
            .field("emit", &g.emit)
            .field("fifo", &g.fifo.len)
            .finish_non_exhaustive()
    }
}

impl Keylock {
    /// A stereo keylock path at `sample_rate`. Allocates; call off the audio thread.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            path: Box::new(GrainPath::new(sample_rate)),
        }
    }

    /// True if the path plays a segment with this step (frames per output frame).
    pub fn handles(step: f64) -> bool {
        step != 0.0 && step.abs() <= MAX_STRETCH_RATE
    }

    /// True if the next output continues seamlessly from `position`, with no priming.
    pub fn is_aligned(&self, position: f64) -> bool {
        self.path
            .cursor
            .is_some_and(|c| (c - position).abs() < ALIGN_EPSILON)
    }

    /// Forget the stretcher's position (new track); the next render primes.
    pub fn invalidate(&mut self) {
        self.path.cursor = None;
    }

    /// True if every frame needed to render `parts` (frames, segment) in order is loaded,
    /// so a block split at loop wraps either renders whole or not at all. Real-time safe.
    pub fn ready(&self, source: &Source, parts: impl Iterator<Item = (usize, Segment)>) -> bool {
        self.path.ready(source, parts)
    }

    /// Render `out.len() / 2` frames of `segment` (whose step must satisfy
    /// [`Keylock::handles`]), overwriting `out`. Primes first if the stretcher is not at
    /// `segment.start`; then, if `fade_jump`, crossfades from the old audio over
    /// [`JUMP_FADE_FRAMES`] (pass `false` when the caller declicks the jump itself).
    ///
    /// Real-time safe. Returns `false`, with `out` untouched and the stretcher unchanged, if a
    /// frame it needs is not loaded yet.
    pub fn render(
        &mut self,
        source: &Source,
        segment: Segment,
        out: &mut [f32],
        fade_jump: bool,
    ) -> bool {
        debug_assert!(
            Self::handles(segment.step),
            "keylock cannot play step {}",
            segment.step
        );
        self.path.render(source, segment, out, fade_jump)
    }
}

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

    /// Append a frame. The FIFO is sized so it never fills; if it did, the frame would be
    /// dropped rather than panic on the audio thread.
    fn push(&mut self, frame: [f32; CHANNELS], position: f64) {
        debug_assert!(self.len < self.capacity(), "keylock fifo overflow");
        if self.len == self.capacity() {
            return;
        }
        let at = (self.head + self.len) % self.capacity();
        self.audio[at * CHANNELS..(at + 1) * CHANNELS].copy_from_slice(&frame);
        self.position[at] = position;
        self.len += 1;
    }

    /// Copy the first `out.len() / 2` frames into `out` without removing them; frames the
    /// FIFO does not hold are silence.
    fn peek(&self, out: &mut [f32]) {
        let frames = (out.len() / CHANNELS).min(self.len);
        for (k, dst) in out.chunks_exact_mut(CHANNELS).enumerate() {
            if k < frames {
                let at = (self.head + k) % self.capacity();
                dst.copy_from_slice(&self.audio[at * CHANNELS..(at + 1) * CHANNELS]);
            } else {
                dst.fill(0.0);
            }
        }
    }

    /// Fill `out` from the front and remove those frames. The path always queues enough
    /// first; a shortfall would be silence rather than a panic on the audio thread.
    fn pop(&mut self, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        debug_assert!(frames <= self.len, "keylock fifo underrun");
        self.peek(out);
        let frames = frames.min(self.len);
        self.head = (self.head + frames) % self.capacity();
        self.len -= frames;
    }
}

struct GrainPath {
    stretcher: bungee::Stretcher,
    hop: i64,
    /// Planar grain input, one channel per `stride` floats (one grain's chunk).
    input: Box<[f32]>,
    stride: usize,
    fifo: Fifo,
    /// Output clock (frames) at the start of the next block.
    clock: i64,
    /// Output clock at which the next synthesised chunk begins.
    emit: i64,
    /// Transport position the next output frame corresponds to, if primed.
    cursor: Option<f64>,
    /// Old audio to crossfade from after a jump, `fade_len` frames, `fade_done` already used.
    fade: Box<[f32]>,
    fade_len: usize,
    fade_done: usize,
}

/// The grains one render will run.
#[derive(Debug, Clone, Copy)]
struct Plan {
    prime: bool,
    /// Output clock of the chunk the first grain's call emits.
    first_emit: i64,
    grains: i64,
}

impl GrainPath {
    fn new(sample_rate: u32) -> Self {
        let rates = SampleRates {
            input: sample_rate,
            output: sample_rate,
        };
        let stretcher = bungee::Stretcher::new(rates, CHANNELS);
        let hop = stretcher.synthesis_hop();
        let stride = stretcher.grain_input_frame_count();
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
            fade_len: 0,
            fade_done: 0,
        }
    }

    /// Position for the grain specified while the chunk at `emit` comes out: its own output
    /// starts two chunks later. `segment` holds from the block start at `clock`.
    fn grain_position(&self, segment: Segment, clock: i64, emit: i64) -> f64 {
        segment.start + segment.step * (emit + 2 * self.hop - clock) as f64
    }

    fn request(&self, segment: Segment, clock: i64, emit: i64, reset: bool) -> Request {
        Request {
            position: self.grain_position(segment, clock, emit),
            speed: segment.step,
            pitch: 1.0,
            reset,
        }
    }

    /// The grains to render `frames` of `segment` from a state of `clock`, `emit` and
    /// `cursor`: from the next chunk, or on a prime from a pre-roll grain three chunks
    /// before the block, until the FIFO covers the block plus the fade lead.
    fn plan(
        &self,
        segment: Segment,
        frames: usize,
        clock: i64,
        emit: i64,
        cursor: Option<f64>,
        drifted: bool,
    ) -> Plan {
        let aligned = cursor.is_some_and(|c| (c - segment.start).abs() < ALIGN_EPSILON);
        let prime = !aligned || drifted;
        let first_emit = if prime { clock - 3 * self.hop } else { emit };
        let need = clock + (frames + JUMP_FADE_FRAMES) as i64;
        let grains = ((need - first_emit).max(0) as u64).div_ceil(self.hop as u64) as i64;
        Plan {
            prime,
            first_emit,
            grains,
        }
    }

    /// True if the audio at the FIFO head is implausibly far from `position` (or NaN).
    fn drifted(&self, position: f64) -> bool {
        self.fifo
            .head_position()
            .map(|p| (p - position).abs())
            .is_some_and(|d| d.is_nan() || d > MAX_DRIFT)
    }

    /// True if every input frame `plan` reads is loaded.
    fn plan_ready(&self, source: &Source, segment: Segment, clock: i64, plan: Plan) -> bool {
        if plan.grains == 0 {
            return true;
        }
        let last_emit = plan.first_emit + (plan.grains - 1) * self.hop;
        // Positions are linear in the output clock, so the first and last grains bound the
        // range.
        let a = self
            .stretcher
            .input_chunk(&self.request(segment, clock, plan.first_emit, false), 0.0);
        let b = self
            .stretcher
            .input_chunk(&self.request(segment, clock, last_emit, false), 0.0);
        source.range_ready(a.begin.min(b.begin), a.end.max(b.end) - 1)
    }

    fn ready(&self, source: &Source, parts: impl Iterator<Item = (usize, Segment)>) -> bool {
        let (mut clock, mut emit, mut cursor) = (self.clock, self.emit, self.cursor);
        for (k, (frames, segment)) in parts.enumerate() {
            let drifted = k == 0 && self.drifted(segment.start);
            let plan = self.plan(segment, frames, clock, emit, cursor, drifted);
            if !self.plan_ready(source, segment, clock, plan) {
                return false;
            }
            emit = plan.first_emit + plan.grains * self.hop;
            clock += frames as i64;
            cursor = Some(segment.start + segment.step * frames as f64);
        }
        true
    }

    fn render(
        &mut self,
        source: &Source,
        segment: Segment,
        out: &mut [f32],
        fade_jump: bool,
    ) -> bool {
        let frames = out.len() / CHANNELS;
        let drifted = self.drifted(segment.start);
        let plan = self.plan(segment, frames, self.clock, self.emit, self.cursor, drifted);
        if !self.plan_ready(source, segment, self.clock, plan) {
            return false;
        }

        if plan.prime {
            self.fade_len = 0;
            self.fade_done = 0;
            if fade_jump && self.cursor.is_some() {
                self.fade_len = self.fifo.len.min(JUMP_FADE_FRAMES);
                self.fifo.peek(&mut self.fade[..self.fade_len * CHANNELS]);
            }
            self.fifo.clear();
            self.emit = plan.first_emit;
        }

        for g in 0..plan.grains {
            let request = self.request(segment, self.clock, self.emit, plan.prime && g == 0);
            self.grain(source, &request);
        }

        self.fifo.pop(out);
        self.clock += frames as i64;
        self.cursor = Some(segment.start + segment.step * frames as f64);

        // Crossfade from the old audio, continuing a fade a previous block started.
        let fade = self.fade_len - self.fade_done;
        if fade > 0 {
            let inv = 1.0 / self.fade_len as f32;
            let old = &self.fade[self.fade_done * CHANNELS..self.fade_len * CHANNELS];
            for (k, (o, s)) in out
                .chunks_exact_mut(CHANNELS)
                .zip(old.chunks_exact(CHANNELS))
                .enumerate()
            {
                let g = (self.fade_done + k + 1) as f32 * inv;
                for c in 0..CHANNELS {
                    o[c] = s[c] + (o[c] - s[c]) * g;
                }
            }
            self.fade_done += fade.min(frames);
        }
        true
    }

    /// Run one grain and queue the part of its output chunk at or after the clock.
    fn grain(&mut self, source: &Source, request: &Request) {
        let chunk = self.stretcher.specify_grain(request, 0.0);
        let n = chunk.len().min(self.stride);
        debug_assert_eq!(n, chunk.len(), "grain input longer than the buffer");
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

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn sine_source(seconds: usize, hz: f64) -> std::sync::Arc<Source> {
        let w = std::f64::consts::TAU * hz / f64::from(SR);
        let frames: Vec<f32> = (0..seconds * SR as usize)
            .flat_map(|i| {
                let x = (0.5 * (w * i as f64).sin()) as f32;
                [x, x]
            })
            .collect();
        Source::from_interleaved(SR, &frames)
    }

    fn max_step(out: &[f32]) -> f32 {
        out.chunks_exact(CHANNELS)
            .zip(out.chunks_exact(CHANNELS).skip(1))
            .map(|(a, b)| (b[0] - a[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_wrap_near_the_end_of_a_block_still_fades_over_the_whole_fade() {
        // 100 Hz at 0.5: natural steps up to ~0.0065. Jump to an arbitrary phase 4 frames
        // before the end of a block; the fade must continue into the next block.
        let src = sine_source(10, 100.0);
        let mut k = Keylock::new(SR);
        let mut out = vec![0.0; 40 * 128 * CHANNELS];
        let mut pos = 48_000.0;
        for (b, block) in out.chunks_mut(128 * CHANNELS).enumerate() {
            if b == 20 {
                let (head, tail) = block.split_at_mut(124 * CHANNELS);
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, head, true));
                // Half a cycle out of phase with the audio it replaces.
                pos = 50_684.0 + 48_000.0 + 240.0;
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, tail, true));
                pos += 4.0;
            } else {
                let seg = Segment {
                    start: pos,
                    step: 1.0,
                };
                assert!(k.render(&src, seg, block, true));
                pos += 128.0;
            }
        }
        let step = max_step(&out[10 * 128 * CHANNELS..]);
        assert!(step < 0.03, "max step {step}");
    }

    #[test]
    fn a_jump_without_fade_plays_the_new_position_exactly() {
        // When the caller declicks, the keylock must not mix in old audio of its own: its
        // output after the jump is what a freshly primed keylock plays there.
        let src = sine_source(10, 440.0);
        let mut played = Keylock::new(SR);
        let mut fresh = Keylock::new(SR);
        let mut out = vec![0.0; 128 * CHANNELS];
        for b in 0..30 {
            let seg = Segment {
                start: 48_000.0 + 128.0 * b as f64,
                step: 1.08,
            };
            assert!(played.render(&src, seg, &mut out, true));
        }
        let seg = Segment {
            start: 200_000.0,
            step: 1.08,
        };
        let mut a = vec![0.0; 128 * CHANNELS];
        let mut b = vec![0.0; 128 * CHANNELS];
        assert!(played.render(&src, seg, &mut a, false));
        assert!(fresh.render(&src, seg, &mut b, false));
        let diff = a
            .iter()
            .zip(&b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(diff < 1e-6, "differs from a fresh prime by {diff}");
    }

    #[test]
    fn ready_covers_every_part_of_a_split_block() {
        // Only the first second is loaded: a block whose second part wraps into unloaded
        // audio is not ready as a whole, and checking changes nothing.
        let (src, mut writer) = Source::with_chunk_frames(SR, 10 * SR as usize, 4096);
        writer.push(&vec![0.1; SR as usize * CHANNELS]);
        let k = Keylock::new(SR);
        let near = Segment {
            start: 20_000.0,
            step: 1.0,
        };
        let far = Segment {
            start: 300_000.0,
            step: 1.0,
        };
        assert!(k.ready(&src, [(64, near)].into_iter()));
        assert!(!k.ready(&src, [(64, near), (64, far)].into_iter()));
        assert!(!k.is_aligned(20_000.0));
    }

    #[test]
    fn handles_both_directions_up_to_the_limit() {
        assert!(Keylock::handles(1.0));
        assert!(Keylock::handles(0.01));
        assert!(Keylock::handles(-1.0));
        assert!(Keylock::handles(-MAX_STRETCH_RATE));
        assert!(Keylock::handles(MAX_STRETCH_RATE));
        assert!(!Keylock::handles(0.0));
        assert!(!Keylock::handles(MAX_STRETCH_RATE + 0.1));
        assert!(!Keylock::handles(-MAX_STRETCH_RATE - 0.1));
    }

    #[test]
    fn primes_once_and_its_output_is_the_playhead() {
        let src = Source::from_interleaved(SR, &vec![0.1; 10 * SR as usize * CHANNELS]);
        let mut k = Keylock::new(SR);
        let mut out = vec![0.0; 128 * CHANNELS];
        let mut pos = 20_000.0;
        for rate in [1.08, 1.08, -0.5, 2.0] {
            for _ in 0..50 {
                let seg = Segment {
                    start: pos,
                    step: rate,
                };
                assert!(k.render(&src, seg, &mut out, true));
                pos += rate * 128.0;
                assert!(k.is_aligned(pos));
            }
            // After the grains requested at the old rate are out, the next frame plays the
            // playhead exactly.
            let head = k.path.fifo.head_position().unwrap();
            assert!((head - pos).abs() < 1e-6, "rate {rate}: {head} vs {pos}");
        }
    }

    #[test]
    fn unloaded_frames_starve_without_touching_state() {
        let (src, _writer) = Source::new(SR, SR as usize);
        let mut k = Keylock::new(SR);
        let mut out = vec![7.0; 128 * CHANNELS];
        let seg = Segment {
            start: 0.0,
            step: 1.0,
        };
        assert!(!k.render(&src, seg, &mut out, true));
        assert!(out.iter().all(|&s| s == 7.0));
        assert!(!k.is_aligned(0.0));
    }
}
