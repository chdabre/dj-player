//! `Deck`: one player, tying a [`Source`], a [`Transport`] and its two scalers together.
//!
//! Each block plays through one of two paths: the [`Scaler`] (resampler: pitch follows rate,
//! any direction) or, with keylock on and a velocity it can play, the [`Keylock`]
//! time-stretcher (Bungee: either direction up to 4x). A change of path crossfades like a
//! jump does.

mod command;
mod publish;
mod render;

use std::sync::Arc;

pub(crate) use command::BlockOutcome;
pub use command::DeckCommand;

use crate::cue::{CueState, TrackCues};
use crate::grid::DeckGrid;
use crate::keylock::Keylock;
use crate::looping::LoopControl;
use crate::scaler::Scaler;
use crate::source::{CHANNELS, Source};
use crate::state::DeckState;
use crate::transport::{Loop, Transport};

/// Largest block a deck renders in one go. [`crate::Engine::process`] splits longer device
/// buffers into blocks of at most this many frames, so the deck's scratch space is fixed.
pub const MAX_BLOCK_FRAMES: usize = 1024;

/// Length of the crossfade applied when the playhead jumps (seek, play/pause, slip snap-back)
/// or the deck switches between resampler and stretcher. Shorter blocks fade over the whole
/// block.
pub const DECLICK_FRAMES: usize = 128;

/// Length of the crossfade across a loop wrap (resampler path).
const WRAP_FADE_FRAMES: usize = 64;

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
    pub(crate) fn new(state: Arc<DeckState>, sample_rate: u32) -> Self {
        Self::with_keylock(state, sample_rate, Some(Keylock::new(sample_rate)))
    }

    /// Create an empty deck with an explicit keylock path (or none).
    pub(crate) fn with_keylock(
        state: Arc<DeckState>,
        sample_rate: u32,
        keylock: Option<Keylock>,
    ) -> Self {
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
    pub(crate) fn set_grid(
        &mut self,
        source_id: u64,
        grid: Arc<DeckGrid>,
    ) -> Option<Arc<DeckGrid>> {
        if self.source.as_ref().map(|s| s.id()) == Some(source_id) {
            self.grid.replace(grid)
        } else {
            Some(grid)
        }
    }

    /// Apply the stored cues of source `source_id` (see [`DeckCommand::SetCues`]). Returns
    /// false, changing nothing, if another track is loaded.
    pub(crate) fn set_cues(&mut self, source_id: u64, serial: u64, cues: &TrackCues) -> bool {
        if self.source.as_ref().map(|s| s.id()) != Some(source_id) {
            return false;
        }
        if self.fade_from.is_none() {
            self.fade_from = Some(self.transport);
        }
        let t = &mut self.transport;
        let fill = cues.fill_only;
        if let Some(cue) = cues.cue
            && (!fill || self.cues.cue == 0.0)
        {
            self.cues.cue = cue.max(0.0);
            if !t.is_playing() && t.position() == 0.0 {
                t.seek(self.cues.cue);
            }
        }
        for (have, stored) in self.cues.hot.iter_mut().zip(cues.hot) {
            if !fill || have.is_none() {
                *have = stored;
            }
        }
        if let Some((start, end)) = cues.stored_loop
            && (!fill || t.stored_loop().is_none())
        {
            t.store_loop(Loop { start, end });
        }
        self.cues_serial = serial;
        true
    }

    /// Replace the loaded source and return the previous one and its grid, which the caller
    /// must not drop on the audio thread. The new track starts paused at frame 0, gridless.
    pub(crate) fn swap_source(
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
    pub(crate) fn apply(&mut self, cmd: &DeckCommand) {
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
            DeckCommand::ScratchBegin => t.scratch_begin(sr),
            DeckCommand::ScratchBy(frames) => t.scratch_by(frames),
            DeckCommand::ScratchEnd => t.scratch_end(),
        }
    }

    /// True if a block of `transport` goes through the stretcher.
    fn uses_keylock(&self, transport: &Transport) -> bool {
        self.keylock.is_some()
            && transport.is_keylock_enabled()
            && !transport.is_scratching()
            && Keylock::handles(transport.velocity())
    }
}
