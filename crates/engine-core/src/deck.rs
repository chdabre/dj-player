//! `Deck`: one player, tying a [`Source`], a [`Transport`] and a [`Scaler`] together.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::scaler::Scaler;
use crate::source::{CHANNELS, Source};
use crate::state::DeckState;
use crate::transport::Transport;

/// Largest block a deck renders in one go. [`crate::Engine::process`] splits longer device
/// buffers into blocks of at most this many frames, so the deck's scratch space is fixed.
pub const MAX_BLOCK_FRAMES: usize = 1024;

/// Length of the crossfade applied when the playhead jumps (seek, play/pause, slip snap-back).
/// Shorter blocks fade over the whole block.
pub const DECLICK_FRAMES: usize = 128;

/// Operations on one deck, sent into the callback as part of a [`crate::Command`].
#[derive(Debug, Clone)]
pub enum DeckCommand {
    /// Swap in a (possibly still loading) track. The previous source, if any, is handed back
    /// through the engine's retire ring so it is never freed on the audio thread.
    Load(Arc<Source>),
    Unload,
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
    transport: Transport,
    scaler: Scaler,
    state: Arc<DeckState>,
    /// Transport as it was before the first command of the current block, if any.
    fade_from: Option<Transport>,
    declick: bool,
    scratch: Box<[f32]>,
}

impl Deck {
    /// Create an empty deck. Allocates its scratch space; call off the audio thread.
    pub fn new(state: Arc<DeckState>) -> Self {
        Self {
            source: None,
            transport: Transport::default(),
            scaler: Scaler::new(),
            state,
            fade_from: None,
            declick: true,
            scratch: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
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

    /// Replace the loaded source and return the previous one, which the caller must not drop
    /// on the audio thread. The new track starts paused at frame 0.
    pub fn swap_source(&mut self, source: Option<Arc<Source>>) -> Option<Arc<Source>> {
        let length = source.as_ref().map_or(0.0, |s| s.len_frames() as f64);
        self.transport = Transport::new(length);
        self.fade_from = None;
        std::mem::replace(&mut self.source, source)
    }

    /// Apply a command that does not change the source. `Load` and `Unload` are handled by
    /// [`crate::Engine`], which owns the retire ring.
    pub fn apply(&mut self, cmd: &DeckCommand) {
        if self.fade_from.is_none() {
            self.fade_from = Some(self.transport);
        }
        let t = &mut self.transport;
        match *cmd {
            DeckCommand::Load(_) | DeckCommand::Unload => {}
            DeckCommand::Play => t.play(),
            DeckCommand::Pause => t.pause(),
            DeckCommand::TogglePlay => t.toggle_play(),
            DeckCommand::Seek(frame) => t.seek(frame),
            DeckCommand::SeekRelative(frames) => t.seek_relative(frames),
            DeckCommand::SetRate(rate) => t.set_rate(rate),
            DeckCommand::NudgeRate(delta) => t.set_rate(t.rate() + delta),
            DeckCommand::SetReverse(on) => t.set_reverse(on),
            DeckCommand::ToggleReverse => t.set_reverse(!t.is_reverse()),
            DeckCommand::SetSlip(on) => t.set_slip(on),
            DeckCommand::ToggleSlip => t.set_slip(!t.is_slip_enabled()),
        }
    }

    /// Render `out.len() / 2` interleaved stereo frames (at most [`MAX_BLOCK_FRAMES`]),
    /// overwriting `out`.
    ///
    /// Real-time safe. If the block would read a chunk that is not loaded yet, the block is
    /// silent, the transport does not move (so no audio is skipped) and the outcome is
    /// [`BlockOutcome::Starved`]. If a command made the playhead jump, the first
    /// [`DECLICK_FRAMES`] frames crossfade from where the old playhead would have gone.
    pub fn render(&mut self, out: &mut [f32]) -> BlockOutcome {
        let frames = out.len() / CHANNELS;
        assert!(frames <= MAX_BLOCK_FRAMES, "block too long");
        let fade_from = self.fade_from.take();
        let Some(source) = self.source.as_deref() else {
            out.fill(0.0);
            return BlockOutcome::Empty;
        };
        let length = source.len_frames() as f64;
        if length != self.transport.length() {
            self.transport.set_length(length);
        }

        let mut next = self.transport;
        let segment = next.advance(frames);
        let audible = self.transport.is_playing();
        if audible {
            let (first, last) = Scaler::span(segment, frames);
            if !source.range_ready(first, last) {
                out.fill(0.0);
                return BlockOutcome::Starved;
            }
            self.scaler.render(source, segment, out);
        } else {
            out.fill(0.0);
        }
        self.transport = next;

        if let Some(mut old) = fade_from
            && self.declick
            && (old.position() != segment.start || old.is_playing() != audible)
        {
            let fade = frames.min(DECLICK_FRAMES);
            let scratch = &mut self.scratch[..fade * CHANNELS];
            let old_audible = old.is_playing();
            let old_segment = old.advance(fade);
            let (first, last) = Scaler::span(old_segment, fade);
            if old_audible && source.range_ready(first, last) {
                self.scaler.render(source, old_segment, scratch);
            } else {
                scratch.fill(0.0);
            }
            let inv = 1.0 / fade as f32;
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
        s.position.store(t.position());
        s.shadow_position.store(t.shadow_position());
        s.rate.store(t.rate());
        s.length.store(t.length());
        let starved = outcome == BlockOutcome::Starved;
        s.starved.store(starved, Ordering::Relaxed);
        if starved {
            s.starved_blocks.fetch_add(1, Ordering::Relaxed);
        }
    }
}
