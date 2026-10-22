//! `EngineHandle`: the control side of the engine, off the audio thread.

use std::sync::Arc;

use rtrb::{Consumer, Producer};

use crate::command::{Command, CommandRingFull, Retired};
use crate::deck::DeckCommand;
use crate::engine::EngineConfig;
use crate::state::{DeckState, LoadMeter};

/// The control side: send commands, read state, collect retired sources.
pub struct EngineHandle {
    config: EngineConfig,
    commands: Producer<Command>,
    retired: Consumer<Retired>,
    decks: Box<[Arc<DeckState>]>,
    load: Arc<LoadMeter>,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EngineHandle {
    /// The handle for an engine built by [`crate::Engine::new`], on the other ends of its rings.
    pub(crate) fn new(
        config: EngineConfig,
        commands: Producer<Command>,
        retired: Consumer<Retired>,
        decks: Box<[Arc<DeckState>]>,
    ) -> Self {
        Self {
            config,
            commands,
            retired,
            decks,
            load: Arc::new(LoadMeter::default()),
        }
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Queue a command for the next callback.
    // A full ring hands the command back by value; boxing it would allocate for nothing.
    #[allow(clippy::result_large_err)]
    pub fn send(&mut self, cmd: Command) -> Result<(), CommandRingFull> {
        self.commands
            .push(cmd)
            .map_err(|rtrb::PushError::Full(c)| CommandRingFull(c))
    }

    #[allow(clippy::result_large_err)]
    pub fn send_deck(&mut self, deck: usize, cmd: DeckCommand) -> Result<(), CommandRingFull> {
        self.send(Command::deck(deck, cmd))
    }

    pub fn deck_state(&self, deck: usize) -> &Arc<DeckState> {
        &self.decks[deck]
    }

    /// The load meter. The backend's callback wrapper records into it.
    pub fn load_meter(&self) -> &Arc<LoadMeter> {
        &self.load
    }

    /// Drop sources and grids the callback has retired. Call regularly from a non-real-time thread.
    /// Returns how many were freed.
    pub fn collect_retired(&mut self) -> usize {
        let mut n = 0;
        while let Ok(src) = self.retired.pop() {
            drop(src);
            n += 1;
        }
        n
    }
}
