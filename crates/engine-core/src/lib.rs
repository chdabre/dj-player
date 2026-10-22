//! Real-time core of the DJ player engine.
//!
//! No I/O and no threads: everything here can run in a unit test, and the offline render in
//! `tests/` drives it exactly the way an audio device would. See `docs/research/engine.md`
//! ("Deck pipeline", "Real-time rules") for the design.
//!
//! - [`Source`]: resident sample store with per-chunk ready flags.
//! - [`Transport`]: playhead, rate, direction and the slip shadow playhead.
//! - The scaler: cubic-Hermite reader at fractional positions, any rate.
//! - The keylock: the keylock path through a time-stretcher (Bungee), aligned with the
//!   playhead.
//! - [`Deck`]: the three together.
//! - [`Engine`]: owns the decks; [`Engine::process`] is the audio callback body.

mod command;
mod cue;
mod deck;
mod engine;
mod fade;
mod grid;
mod handle;
mod keylock;
mod looping;
#[cfg(feature = "testing")]
pub mod render;
mod scaler;
mod source;
mod state;
mod transport;

pub use command::{Command, CommandRingFull};
pub use cue::{CueState, HOT_CUES, QUANTIZE_RESOLUTIONS, TrackCues};
pub use deck::{DECLICK_FRAMES, Deck, DeckCommand, MAX_BLOCK_FRAMES};
pub use engine::{Engine, EngineConfig};
pub use grid::{DeckGrid, FALLBACK_BPM};
pub use handle::EngineHandle;
pub use looping::{BEAT_JUMPS, BEAT_LOOPS};
pub use source::{CHANNELS, Frame, FrameSource, Source, SourceWriter};
pub use state::{AtomicF64, DeckState, LoadMeter, LoadReport};
pub use transport::{Loop, Transport};
