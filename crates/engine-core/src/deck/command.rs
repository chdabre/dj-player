//! The commands a deck takes and what it reports per block.

use std::sync::Arc;

use crate::cue::TrackCues;
use crate::grid::DeckGrid;
use crate::source::Source;

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
    /// to the cue. `serial` is published in [`crate::DeckState::cues_serial`] once applied, so the
    /// owner knows which deck cues are the library's. Ignored if another track is loaded.
    SetCues {
        source_id: u64,
        serial: u64,
        /// Retired after it is read, like a grid, so the callback never frees it.
        cues: Arc<TrackCues>,
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
    /// The hand takes the track (touch on the waveform, later the jog): the playhead holds,
    /// then follows [`DeckCommand::ScratchBy`]. Plays through the resampler, so the pitch
    /// follows the hand like vinyl. With slip on and the track playing, the shadow runs on.
    ScratchBegin,
    /// Move the hand by this many frames (positive = forward in the track).
    ScratchBy(f64),
    /// The hand lets go: play state and rate as before; with slip, back to the shadow.
    ScratchEnd,
    /// The CUE button: press and release.
    CuePress,
    CueRelease,
    /// Hot cue pad A–H (0–7): set if empty, else jump.
    HotCue(usize),
    DeleteHotCue(usize),
    /// Quantize resolution in beats, `None` = off. Kept across track loads.
    SetQuantize(Option<f64>),
    /// Loops and beat jump.
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
pub(crate) enum BlockOutcome {
    /// No track loaded: silence.
    Empty,
    /// Rendered audio (or silence because the transport is paused).
    Rendered,
    /// The block needed a chunk that is not loaded yet: silence, transport held.
    Starved,
}
