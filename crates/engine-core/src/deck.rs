//! `Deck`: one player, tying a [`Source`], a [`Transport`] and its two scalers together.
//!
//! Each block plays through one of two paths: the [`Scaler`] (resampler: pitch follows rate,
//! any direction) or, with keylock on and a velocity it can play, the [`Keylock`]
//! time-stretcher (Bungee: either direction up to 4x). A change of path crossfades like a
//! jump does.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::cue::{CueState, TrackCues};
use crate::grid::DeckGrid;
use crate::keylock::Keylock;
use crate::looping::LoopControl;
use crate::scaler::Scaler;
use crate::source::{CHANNELS, Source};
use crate::state::DeckState;
use crate::transport::{Loop, Segment, Transport};

/// Largest block a deck renders in one go. [`crate::Engine::process`] splits longer device
/// buffers into blocks of at most this many frames, so the deck's scratch space is fixed.
pub const MAX_BLOCK_FRAMES: usize = 1024;

/// Length of the crossfade applied when the playhead jumps (seek, play/pause, slip snap-back)
/// or the deck switches between resampler and stretcher. Shorter blocks fade over the whole
/// block.
pub const DECLICK_FRAMES: usize = 128;

/// Length of the crossfade across a loop wrap (resampler path).
pub const WRAP_FADE_FRAMES: usize = 64;

/// Operations on one deck, sent into the callback as part of a [`crate::Command`].
#[derive(Debug, Clone)]
pub enum DeckCommand {
    /// Swap in a (possibly still loading) track. The previous source, if any, is handed back
    /// through the engine's retire ring so it is never freed on the audio thread.
    Load(Arc<Source>),
    Unload,
    /// The beatgrid of the source with this id (see [`Source::id`]). Ignored, and retired,
    /// if another track is loaded by the time it arrives.
    SetGrid {
        source_id: u64,
        grid: Arc<DeckGrid>,
    },
    /// The stored cues of the source with this id, from the library: replaces the cue
    /// point, the hot cues and the stored loop. A paused deck still at the track start moves
    /// to the cue. `serial` is published in [`DeckState::cues_serial`] once applied, so the
    /// owner knows which deck cues are the library's. Ignored if another track is loaded.
    SetCues {
        source_id: u64,
        serial: u64,
        cues: TrackCues,
    },
    Play,
    Pause,
    TogglePlay,
    /// Absolute seek in frames at the engine rate.
    Seek(f64),
    /// Relative seek in frames at the engine rate.
    SeekRelative(f64),
    SetRate(f64),
    /// Add to the current rate.
    NudgeRate(f64),
    SetReverse(bool),
    ToggleReverse,
    SetSlip(bool),
    ToggleSlip,
    /// Keylock: rate changes speed, not pitch. Kept across track loads.
    SetKeylock(bool),
    ToggleKeylock,
    /// The CUE button (see [`crate::cue`]): press and release.
    CuePress,
    CueRelease,
    /// Hot cue pad A–H (0–7): set if empty, else jump.
    HotCue(usize),
    DeleteHotCue(usize),
    /// Quantize resolution in beats, `None` = off. Kept across track loads.
    SetQuantize(Option<f64>),
    /// Loops and beat jump (see [`crate::looping`]).
    LoopIn,
    LoopOut,
    ReloopExit,
    BeatLoop(f64),
    /// LOOP 1/2X (0.5) or LOOP 2X (2.0).
    LoopResize(f64),
    BeatJump(f64),
}

/// What the deck did with the last block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// No track loaded: silence.
    Empty,
    /// Rendered audio (or silence because the transport is paused).
    Rendered,
    /// The block needed a chunk that is not loaded yet: silence, transport held.
    Starved,
}

#[derive(Debug)]
pub struct Deck {
    source: Option<Arc<Source>>,
    grid: Option<Arc<DeckGrid>>,
    cues: CueState,
    /// Serial of the last [`DeckCommand::SetCues`] applied.
    cues_serial: u64,
    loops: LoopControl,
    sample_rate: u32,
    transport: Transport,
    scaler: Scaler,
    /// `None` for a deck without a keylock path; keylock then has no effect.
    keylock: Option<Keylock>,
    /// The last rendered block went through the stretcher.
    stretching: bool,
    state: Arc<DeckState>,
    /// Transport as it was before the first command of the current block, if any.
    fade_from: Option<Transport>,
    declick: bool,
    scratch: Box<[f32]>,
    wrap_scratch: Box<[f32]>,
}

impl Deck {
    /// Create an empty deck for an engine running at `sample_rate`, with a keylock path.
    /// Allocates its scratch space and stretcher; call off the audio thread.
    pub fn new(state: Arc<DeckState>, sample_rate: u32) -> Self {
        Self::with_keylock(state, sample_rate, Some(Keylock::new(sample_rate)))
    }

    /// Create an empty deck with an explicit keylock path (or none).
    pub fn with_keylock(state: Arc<DeckState>, sample_rate: u32, keylock: Option<Keylock>) -> Self {
        Self {
            source: None,
            grid: None,
            cues: CueState::default(),
            cues_serial: 0,
            loops: LoopControl::default(),
            sample_rate,
            transport: Transport::default(),
            scaler: Scaler::new(),
            keylock,
            stretching: false,
            state,
            fade_from: None,
            declick: true,
            scratch: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
            wrap_scratch: vec![0.0; WRAP_FADE_FRAMES * CHANNELS].into_boxed_slice(),
        }
    }

    /// Enable or disable the jump crossfade (on by default). Offline tests turn it off to
    /// check sample-exact output right at a seek.
    pub fn set_declick(&mut self, enabled: bool) {
        self.declick = enabled;
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub fn source(&self) -> Option<&Arc<Source>> {
        self.source.as_ref()
    }

    pub fn state(&self) -> &Arc<DeckState> {
        &self.state
    }

    pub fn cues(&self) -> &CueState {
        &self.cues
    }

    pub fn grid(&self) -> Option<&Arc<DeckGrid>> {
        self.grid.as_ref()
    }

    /// Attach the grid of source `source_id`. Returns the grid to retire: the previous one,
    /// or `grid` itself if it belongs to a track that is no longer loaded.
    pub fn set_grid(&mut self, source_id: u64, grid: Arc<DeckGrid>) -> Option<Arc<DeckGrid>> {
        if self.source.as_ref().map(|s| s.id()) == Some(source_id) {
            self.grid.replace(grid)
        } else {
            Some(grid)
        }
    }

    /// Apply the stored cues of source `source_id` (see [`DeckCommand::SetCues`]). Returns
    /// false, changing nothing, if another track is loaded.
    pub fn set_cues(&mut self, source_id: u64, serial: u64, cues: &TrackCues) -> bool {
        if self.source.as_ref().map(|s| s.id()) != Some(source_id) {
            return false;
        }
        if self.fade_from.is_none() {
            self.fade_from = Some(self.transport);
        }
        let t = &mut self.transport;
        if let Some(cue) = cues.cue {
            self.cues.cue = cue.max(0.0);
            if !t.is_playing() && t.position() == 0.0 {
                t.seek(self.cues.cue);
            }
        }
        self.cues.hot = cues.hot;
        if let Some((start, end)) = cues.stored_loop {
            t.store_loop(Loop { start, end });
        }
        self.cues_serial = serial;
        true
    }

    /// Replace the loaded source and return the previous one and its grid, which the caller
    /// must not drop on the audio thread. The new track starts paused at frame 0, gridless.
    pub fn swap_source(
        &mut self,
        source: Option<Arc<Source>>,
    ) -> (Option<Arc<Source>>, Option<Arc<DeckGrid>>) {
        let length = source.as_ref().map_or(0.0, |s| s.len_frames() as f64);
        let keylock = self.transport.is_keylock_enabled();
        // Cues and loops belong to the track; the quantize setting stays.
        self.cues = CueState {
            quantize: self.cues.quantize,
            ..CueState::default()
        };
        self.loops = LoopControl::default();
        self.transport = Transport::new(length);
        self.transport.set_keylock(keylock);
        self.fade_from = None;
        if let Some(k) = &mut self.keylock {
            k.invalidate();
        }
        (
            std::mem::replace(&mut self.source, source),
            self.grid.take(),
        )
    }

    /// Apply a command that does not change the source. `Load` and `Unload` are handled by
    /// [`crate::Engine`], which owns the retire ring.
    pub fn apply(&mut self, cmd: &DeckCommand) {
        if self.fade_from.is_none() {
            self.fade_from = Some(self.transport);
        }
        let t = &mut self.transport;
        let cues = &mut self.cues;
        let loops = &mut self.loops;
        let sr = self.sample_rate;
        let grid = self.grid.as_deref();
        match *cmd {
            DeckCommand::Load(_)
            | DeckCommand::Unload
            | DeckCommand::SetGrid { .. }
            | DeckCommand::SetCues { .. } => {}
            DeckCommand::Play => {
                cues.play_pressed();
                t.play();
            }
            DeckCommand::Pause => t.pause(),
            DeckCommand::TogglePlay => {
                if !t.is_playing() || cues.previewing {
                    cues.play_pressed();
                    t.play();
                } else {
                    t.pause();
                }
            }
            DeckCommand::CuePress => cues.cue_press(t, grid),
            DeckCommand::CueRelease => cues.cue_release(t),
            DeckCommand::HotCue(i) => cues.hot_cue(i, t, grid),
            DeckCommand::DeleteHotCue(i) => cues.delete_hot_cue(i),
            DeckCommand::SetQuantize(q) => cues.quantize = q.filter(|r| *r > 0.0),
            DeckCommand::LoopIn => loops.loop_in(t, grid, cues),
            DeckCommand::LoopOut => loops.loop_out(t, grid, cues),
            DeckCommand::ReloopExit => loops.reloop_exit(t),
            DeckCommand::BeatLoop(beats) => loops.beat_loop(beats, t, grid, cues, sr),
            DeckCommand::LoopResize(factor) => loops.resize(factor, t, grid, sr),
            DeckCommand::BeatJump(beats) => loops.beat_jump(beats, t, grid, sr),
            DeckCommand::Seek(frame) => t.seek(frame),
            DeckCommand::SeekRelative(frames) => t.seek_relative(frames),
            DeckCommand::SetRate(rate) => t.set_rate(rate),
            DeckCommand::NudgeRate(delta) => t.set_rate(t.rate() + delta),
            DeckCommand::SetReverse(on) => t.set_reverse(on),
            DeckCommand::ToggleReverse => t.set_reverse(!t.is_reverse()),
            DeckCommand::SetSlip(on) => t.set_slip(on),
            DeckCommand::ToggleSlip => t.set_slip(!t.is_slip_enabled()),
            DeckCommand::SetKeylock(on) => t.set_keylock(on),
            DeckCommand::ToggleKeylock => t.set_keylock(!t.is_keylock_enabled()),
        }
    }

    /// True if a block of `transport` goes through the stretcher.
    fn uses_keylock(&self, transport: &Transport) -> bool {
        self.keylock.is_some()
            && transport.is_keylock_enabled()
            && Keylock::handles(transport.velocity())
    }

    /// Render `out.len() / 2` interleaved stereo frames (at most [`MAX_BLOCK_FRAMES`]),
    /// overwriting `out`.
    ///
    /// Real-time safe. If the block would read a chunk that is not loaded yet, the block is
    /// silent, the transport does not move (so no audio is skipped) and the outcome is
    /// [`BlockOutcome::Starved`]. If a command made the playhead jump or switched between
    /// resampler and stretcher, the first [`DECLICK_FRAMES`] frames crossfade from what the
    /// old state would have played.
    pub fn render(&mut self, out: &mut [f32]) -> BlockOutcome {
        let frames = out.len() / CHANNELS;
        assert!(frames <= MAX_BLOCK_FRAMES, "block too long");
        let fade_from = self.fade_from.take();
        let Some(source) = self.source.as_deref() else {
            out.fill(0.0);
            self.stretching = false;
            return BlockOutcome::Empty;
        };
        let length = source.len_frames() as f64;
        if length != self.transport.length() {
            self.transport.set_length(length);
        }

        let mut next = self.transport;
        let parts = next.advance(frames);
        let first = parts.first();
        let audible = self.transport.is_playing();
        let stretch = self.uses_keylock(&self.transport);
        let fade = fade_from.filter(|old| {
            self.declick
                && (old.position() != first.start
                    || old.is_playing() != audible
                    || self.uses_keylock(old) != stretch)
        });

        // The outgoing audio first: if it came from the stretcher, it has to be rendered
        // before the stretcher is primed at the new position.
        let fade_len = frames.min(DECLICK_FRAMES);
        if let Some(mut old) = fade {
            let old_stretch = self.uses_keylock(&old);
            let old_audible = old.is_playing();
            let old_parts = old.advance(fade_len);
            let scratch = &mut self.scratch[..fade_len * CHANNELS];
            let mut rendered = old_audible;
            for (off, n, seg) in old_parts.iter(fade_len) {
                let dst = &mut scratch[off * CHANNELS..(off + n) * CHANNELS];
                let (lo, hi) = Scaler::span(seg, n);
                rendered &= match &mut self.keylock {
                    Some(k) if old_stretch && k.is_aligned(seg.start) => {
                        k.render(source, seg, dst, false)
                    }
                    _ if source.range_ready(lo, hi) => {
                        self.scaler.render(source, seg, dst);
                        true
                    }
                    _ => false,
                };
            }
            if !rendered {
                scratch.fill(0.0);
            }
        }

        if audible {
            // Every piece must be readable before anything is rendered.
            let ready = match &self.keylock {
                Some(k) if stretch => k.ready(source, parts.iter(frames).map(|(_, n, s)| (n, s))),
                _ => parts.iter(frames).all(|(_, n, seg)| {
                    let (lo, hi) = Scaler::span(seg, n);
                    source.range_ready(lo, hi)
                }),
            };
            if !ready {
                out.fill(0.0);
                return BlockOutcome::Starved;
            }
            let mut prev: Option<(usize, Segment)> = None;
            for (off, n, seg) in parts.iter(frames) {
                let dst = &mut out[off * CHANNELS..(off + n) * CHANNELS];
                match &mut self.keylock {
                    Some(k) if stretch => {
                        // The deck declicks a commanded jump itself (first piece); the keylock
                        // fades the rest (loop wraps).
                        let fade_jump = self.declick && !(off == 0 && fade.is_some());
                        if !k.render(source, seg, dst, fade_jump) {
                            out.fill(0.0);
                            return BlockOutcome::Starved;
                        }
                    }
                    _ => self.scaler.render(source, seg, dst),
                }
                // Loop wrap (resampler path): crossfade from the audio after the loop end.
                if let Some((prev_off, prev_seg)) = prev
                    && !stretch
                    && self.declick
                {
                    let len = WRAP_FADE_FRAMES.min(frames - off);
                    let tail = Segment {
                        start: prev_seg.start + (off - prev_off) as f64 * prev_seg.step,
                        step: prev_seg.step,
                    };
                    let (lo, hi) = Scaler::span(tail, len);
                    if source.range_ready(lo, hi) {
                        let ws = &mut self.wrap_scratch[..len * CHANNELS];
                        self.scaler.render(source, tail, ws);
                        let inv = 1.0 / len as f32;
                        let span = &mut out[off * CHANNELS..(off + len) * CHANNELS];
                        for (k, (o, w)) in span
                            .chunks_exact_mut(CHANNELS)
                            .zip(ws.chunks_exact(CHANNELS))
                            .enumerate()
                        {
                            let g = (k + 1) as f32 * inv;
                            for c in 0..CHANNELS {
                                o[c] = w[c] + (o[c] - w[c]) * g;
                            }
                        }
                    }
                }
                prev = Some((off, seg));
            }
        } else {
            out.fill(0.0);
        }
        self.transport = next;
        self.stretching = audible && stretch;

        if fade.is_some() {
            let scratch = &self.scratch[..fade_len * CHANNELS];
            let inv = 1.0 / fade_len as f32;
            for (k, (o, s)) in out
                .chunks_exact_mut(CHANNELS)
                .zip(scratch.chunks_exact(CHANNELS))
                .enumerate()
            {
                let g = (k + 1) as f32 * inv;
                for c in 0..CHANNELS {
                    o[c] = s[c] + (o[c] - s[c]) * g;
                }
            }
        }
        BlockOutcome::Rendered
    }

    /// Publish the fast state. Real-time safe.
    pub fn publish(&self, outcome: BlockOutcome) {
        let s = &*self.state;
        let t = &self.transport;
        s.loaded.store(self.source.is_some(), Ordering::Relaxed);
        s.playing.store(t.is_playing(), Ordering::Relaxed);
        s.reverse.store(t.is_reverse(), Ordering::Relaxed);
        s.slip_enabled.store(t.is_slip_enabled(), Ordering::Relaxed);
        s.slip_active.store(t.is_slip_active(), Ordering::Relaxed);
        s.keylock_enabled
            .store(t.is_keylock_enabled(), Ordering::Relaxed);
        s.stretching.store(self.stretching, Ordering::Relaxed);
        s.position.store(t.position());
        s.shadow_position.store(t.shadow_position());
        s.rate.store(t.rate());
        s.cue.store(self.cues.cue);
        s.hot_cues.store(self.cues.hot_mask(), Ordering::Relaxed);
        for (slot, hot) in s.hot_cue_positions.iter().zip(self.cues.hot) {
            slot.store(hot.unwrap_or(f64::NAN));
        }
        s.cues_serial.store(self.cues_serial, Ordering::Relaxed);
        s.quantize.store(self.cues.quantize.unwrap_or(0.0));
        let (looping, l) = match (t.active_loop(), t.stored_loop()) {
            (Some(l), _) => (true, Some(l)),
            (None, l) => (false, l),
        };
        s.looping.store(looping, Ordering::Relaxed);
        s.loop_start.store(l.map_or(f64::NAN, |l| l.start));
        s.loop_end.store(l.map_or(f64::NAN, |l| l.end));
        match &self.grid {
            Some(g) => {
                s.track_bpm.store(g.bpm());
                s.beat.store(g.beat_at(t.position()));
            }
            None => {
                s.track_bpm.store(0.0);
                s.beat.store(f64::NAN);
            }
        }
        s.length.store(t.length());
        let starved = outcome == BlockOutcome::Starved;
        s.starved.store(starved, Ordering::Relaxed);
        if starved {
            s.starved_blocks.fetch_add(1, Ordering::Relaxed);
        }
    }
}
