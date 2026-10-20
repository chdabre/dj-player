//! Real-time core of the DJ player engine.
//!
//! No I/O and no threads: everything here can run in a unit test, and the offline render in
//! `tests/` drives it exactly the way an audio device would. See `docs/research/engine.md`
//! ("Deck pipeline", "Real-time rules") for the design.
//!
//! - [`Source`]: resident sample store with per-chunk ready flags.
//! - [`Transport`]: playhead, rate, direction and the slip shadow playhead.
//! - [`Scaler`]: cubic-Hermite reader at fractional positions, any rate.
//! - [`Deck`]: the three together.
//! - [`Engine`]: owns the decks; [`Engine::process`] is the audio callback body.

pub mod deck;
pub mod engine;
pub mod render;
pub mod scaler;
pub mod source;
pub mod state;
pub mod transport;

pub use deck::{BlockOutcome, DECLICK_FRAMES, Deck, DeckCommand, MAX_BLOCK_FRAMES};
pub use engine::{Command, CommandRingFull, Engine, EngineConfig, EngineHandle};
pub use scaler::Scaler;
pub use source::{CHANNELS, Frame, FrameSource, SliceSource, Source, SourceWriter};
pub use state::{AtomicF64, DeckState, LoadMeter, LoadReport};
pub use transport::{Segment, SlipAction, Transport};
