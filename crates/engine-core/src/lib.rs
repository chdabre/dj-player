//! Real-time core of the DJ player engine.
//!
//! No I/O and no threads: everything here can run in a unit test, and the offline render in
//! `tests/` drives it exactly the way an audio device would. See `docs/research/engine.md`
//! ("Deck pipeline", "Real-time rules") for the design.
//!
//! - [`Source`]: resident sample store with per-chunk ready flags.
//! - [`Transport`]: playhead, rate, direction and the slip shadow playhead.
//! - [`Scaler`]: cubic-Hermite reader at fractional positions, any rate.
//! - [`Keylock`]: the keylock path through a time-stretcher (Bungee), aligned with
//!   the playhead.
//! - [`Deck`]: the three together.
//! - [`Engine`]: owns the decks; [`Engine::process`] is the audio callback body.

pub mod cue;
pub mod deck;
pub mod engine;
pub mod grid;
pub mod keylock;
pub mod looping;
pub mod render;
pub mod scaler;
pub mod source;
pub mod state;
pub mod transport;

pub use cue::{CueState, HOT_CUES, QUANTIZE_RESOLUTIONS};
pub use deck::{BlockOutcome, DECLICK_FRAMES, Deck, DeckCommand, MAX_BLOCK_FRAMES};
pub use engine::{Command, CommandRingFull, Engine, EngineConfig, EngineHandle, Retired};
pub use grid::{DeckGrid, FALLBACK_BPM};
pub use keylock::{Keylock, MAX_STRETCH_RATE};
pub use looping::{BEAT_JUMPS, BEAT_LOOPS, LoopControl};
pub use scaler::Scaler;
pub use source::{CHANNELS, Frame, FrameSource, SliceSource, Source, SourceWriter};
pub use state::{AtomicF64, DeckState, LoadMeter, LoadReport};
pub use transport::{
    Loop, MAX_SEGMENTS, MIN_LOOP_FRAMES, Segment, Segments, SlipAction, Transport,
};
