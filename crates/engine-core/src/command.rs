//! What goes through the engine's two rings: commands in, retired values out.

use std::sync::Arc;

use crate::cue::TrackCues;
use crate::deck::DeckCommand;
use crate::grid::DeckGrid;
use crate::source::Source;

/// Capacity of the command ring.
pub(crate) const COMMAND_CAPACITY: usize = 256;
/// Most commands applied per callback, so command handling has a fixed upper bound.
pub(crate) const MAX_COMMANDS_PER_CALLBACK: usize = 64;
/// Capacity of the ring carrying replaced sources out of the callback.
pub(crate) const RETIRE_CAPACITY: usize = 16;

/// Something the callback replaced and hands back to be freed elsewhere.
#[derive(Debug)]
// The values are never read, only held until the handle drops them off the audio thread.
#[allow(dead_code)]
pub(crate) enum Retired {
    Source(Arc<Source>),
    Grid(Arc<DeckGrid>),
    Cues(Arc<TrackCues>),
}

/// A command into the engine.
#[derive(Debug, Clone)]
pub enum Command {
    Deck { deck: usize, cmd: DeckCommand },
}

impl Command {
    pub fn deck(deck: usize, cmd: DeckCommand) -> Self {
        Command::Deck { deck, cmd }
    }
}

/// The command ring was full; the command is handed back.
#[derive(Debug)]
pub struct CommandRingFull(pub Command);

impl std::fmt::Display for CommandRingFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("engine command ring is full")
    }
}

impl std::error::Error for CommandRingFull {}
