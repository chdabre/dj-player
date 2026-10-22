//! The grain scheduler behind [`Keylock`](super::Keylock): places each Bungee grain where
//! the playhead will be when its output plays (see the [module docs](super)).

use engine_stretch::bungee::{self, Request, SampleRates};

use super::fifo::Fifo;
use super::{ALIGN_EPSILON, JUMP_FADE_FRAMES, MAX_DRIFT};
use crate::deck::MAX_BLOCK_FRAMES;
use crate::fade::crossfade;
use crate::source::{CHANNELS, FrameSource, Source};
use crate::transport::Segment;

pub(super) struct GrainPath {
    stretcher: bungee::Stretcher,
    hop: i64,
    /// Planar grain input, one channel per `stride` floats (one grain's chunk).
    input: Box<[f32]>,
    stride: usize,
    pub(super) fifo: Fifo,
    /// Output clock (frames) at the start of the next block.
    pub(super) clock: i64,
    /// Output clock at which the next synthesised chunk begins.
    pub(super) emit: i64,
    /// Transport position the next output frame corresponds to, if primed.
    pub(super) cursor: Option<f64>,
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
    pub(super) fn new(sample_rate: u32) -> Self {
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

    pub(super) fn ready(
        &self,
        source: &Source,
        parts: impl Iterator<Item = (usize, Segment)>,
    ) -> bool {
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

    pub(super) fn render(
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
            let old = &self.fade[self.fade_done * CHANNELS..self.fade_len * CHANNELS];
            crossfade(out, old, self.fade_done, self.fade_len);
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
