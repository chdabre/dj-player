//! `Deck`: one player, tying a [`Source`], a [`Transport`] and its two scalers together.
//!
//! Each block plays through one of two paths: the [`Scaler`] (resampler: pitch follows rate,
//! any direction) or, with keylock on and a forward rate it can play, the [`Keylock`]
//! time-stretcher. A change of path crossfades like a jump does.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::grid::DeckGrid;
use crate::keylock::Keylock;
use crate::scaler::Scaler;
use crate::source::{CHANNELS, Source};
use crate::state::DeckState;
use crate::transport::Transport;

/// Largest block a deck renders in one go. [`crate::Engine::process`] splits longer device
/// buffers into blocks of at most this many frames, so the deck's scratch space is fixed.
pub const MAX_BLOCK_FRAMES: usize = 1024;

/// Length of the crossfade applied when the playhead jumps (seek, play/pause, slip snap-back)
/// or the deck switches between resampler and stretcher. Shorter blocks fade over the whole
/// block.
pub const DECLICK_FRAMES: usize = 128;

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
    transport: Transport,
    scaler: Scaler,
    /// `None` if no stretcher backend is compiled in; keylock then has no effect.
    keylock: Option<Keylock>,
    /// The last rendered block went through the stretcher.
    stretching: bool,
    state: Arc<DeckState>,
    /// Transport as it was before the first command of the current block, if any.
    fade_from: Option<Transport>,
    declick: bool,
    scratch: Box<[f32]>,
}

impl Deck {
    /// Create an empty deck for an engine running at `sample_rate`, with the default
    /// stretcher. Allocates its scratch space and stretcher; call off the audio thread.
    pub fn new(state: Arc<DeckState>, sample_rate: u32) -> Self {
        let stretcher = engine_stretch::default_stretcher(CHANNELS, sample_rate);
        Self::with_keylock(state, stretcher.map(Keylock::new))
    }

    /// Create an empty deck with an explicit keylock path (or none).
    pub fn with_keylock(state: Arc<DeckState>, keylock: Option<Keylock>) -> Self {
        Self {
            source: None,
            grid: None,
            transport: Transport::default(),
            scaler: Scaler::new(),
            keylock,
            stretching: false,
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

    /// Replace the loaded source and return the previous one and its grid, which the caller
    /// must not drop on the audio thread. The new track starts paused at frame 0, gridless.
    pub fn swap_source(
        &mut self,
        source: Option<Arc<Source>>,
    ) -> (Option<Arc<Source>>, Option<Arc<DeckGrid>>) {
        let length = source.as_ref().map_or(0.0, |s| s.len_frames() as f64);
        let keylock = self.transport.is_keylock_enabled();
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
        match *cmd {
            DeckCommand::Load(_) | DeckCommand::Unload | DeckCommand::SetGrid { .. } => {}
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
        let segment = next.advance(frames);
        let audible = self.transport.is_playing();
        let stretch = self.uses_keylock(&self.transport);
        let fade = fade_from.filter(|old| {
            self.declick
                && (old.position() != segment.start
                    || old.is_playing() != audible
                    || self.uses_keylock(old) != stretch)
        });

        // The outgoing audio first: if it came from the stretcher, it has to be rendered
        // before the stretcher is primed at the new position.
        let fade_len = frames.min(DECLICK_FRAMES);
        if let Some(mut old) = fade {
            let old_stretch = self.uses_keylock(&old);
            let old_audible = old.is_playing();
            let old_segment = old.advance(fade_len);
            let scratch = &mut self.scratch[..fade_len * CHANNELS];
            let (first, last) = Scaler::span(old_segment, fade_len);
            let rendered = match &mut self.keylock {
                Some(k) if old_stretch && k.is_aligned(old_segment.start) => {
                    k.render(source, old_segment, scratch)
                }
                _ if old_audible && source.range_ready(first, last) => {
                    self.scaler.render(source, old_segment, scratch);
                    true
                }
                _ => false,
            };
            if !rendered {
                scratch.fill(0.0);
            }
        }

        if audible {
            let ready = match &mut self.keylock {
                Some(k) if stretch => k.render(source, segment, out),
                _ => {
                    let (first, last) = Scaler::span(segment, frames);
                    let ready = source.range_ready(first, last);
                    if ready {
                        self.scaler.render(source, segment, out);
                    }
                    ready
                }
            };
            if !ready {
                out.fill(0.0);
                return BlockOutcome::Starved;
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
