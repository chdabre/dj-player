//! `Engine`: owns every deck and runs the body of the real-time audio callback.
//!
//! Commands come in through a bounded SPSC ring ([`rtrb`]); state goes out through the atomics
//! in [`DeckState`]. Sources replaced by a load are pushed onto a second ring and dropped by
//! whoever drains [`EngineHandle::collect_retired`], so the callback never frees memory.

use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::deck::{BlockOutcome, Deck, DeckCommand, MAX_BLOCK_FRAMES};
use crate::source::{CHANNELS, Source};
use crate::state::{DeckState, LoadMeter};

/// Capacity of the command ring.
pub const COMMAND_CAPACITY: usize = 256;
/// Most commands applied per callback, so command handling has a fixed upper bound.
pub const MAX_COMMANDS_PER_CALLBACK: usize = 64;
/// Capacity of the ring carrying replaced sources out of the callback.
pub const RETIRE_CAPACITY: usize = 16;

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

/// Engine configuration, fixed for the lifetime of an engine instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// Device sample rate. Every source must already be at this rate.
    pub sample_rate: u32,
    /// Interleaved output channels of the device. Master goes to channels 0 and 1.
    pub channels: usize,
    pub decks: usize,
}

/// The real-time side. Move it into the audio callback and call [`Engine::process`].
pub struct Engine {
    config: EngineConfig,
    decks: Box<[Deck]>,
    commands: Consumer<Command>,
    retire: Producer<Arc<Source>>,
    mix: Box<[f32]>,
    deck_buf: Box<[f32]>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("config", &self.config)
            .field("decks", &self.decks)
            .finish_non_exhaustive()
    }
}

/// The control side: send commands, read state, collect retired sources.
pub struct EngineHandle {
    config: EngineConfig,
    commands: Producer<Command>,
    retired: Consumer<Arc<Source>>,
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

/// The command ring was full; the command is handed back.
#[derive(Debug)]
pub struct CommandRingFull(pub Command);

impl std::fmt::Display for CommandRingFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("engine command ring is full")
    }
}

impl std::error::Error for CommandRingFull {}

impl Engine {
    /// Build an engine and its handle. Allocates everything the callback will ever need.
    pub fn new(config: EngineConfig) -> (Engine, EngineHandle) {
        assert!(config.channels > 0, "need at least one output channel");
        assert!(config.decks > 0, "need at least one deck");
        let (cmd_tx, cmd_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (retire_tx, retire_rx) = RingBuffer::new(RETIRE_CAPACITY);
        let states: Box<[Arc<DeckState>]> = (0..config.decks)
            .map(|_| Arc::new(DeckState::default()))
            .collect();
        let decks = states.iter().map(|s| Deck::new(Arc::clone(s))).collect();
        let engine = Engine {
            config,
            decks,
            commands: cmd_rx,
            retire: retire_tx,
            mix: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
            deck_buf: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
        };
        let handle = EngineHandle {
            config,
            commands: cmd_tx,
            retired: retire_rx,
            decks: states,
            load: Arc::new(LoadMeter::default()),
        };
        (engine, handle)
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }

    pub fn deck(&self, index: usize) -> &Deck {
        &self.decks[index]
    }

    pub fn deck_mut(&mut self, index: usize) -> &mut Deck {
        &mut self.decks[index]
    }

    /// The callback body: drain commands, render every deck, mix, write `frames` interleaved
    /// frames of `config.channels` channels into `out`.
    ///
    /// Real-time safe: no allocation, no locks, no I/O. Device buffers longer than
    /// [`MAX_BLOCK_FRAMES`] are processed in several blocks.
    pub fn process(&mut self, out: &mut [f32], frames: usize) {
        let ch = self.config.channels;
        let frames = frames.min(out.len() / ch);
        self.drain_commands();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK_FRAMES);
            self.render_block(n);
            let dst = &mut out[done * ch..(done + n) * ch];
            write_master(&self.mix[..n * CHANNELS], dst, ch);
            done += n;
        }
        out[frames * ch..].fill(0.0);
    }

    fn render_block(&mut self, frames: usize) {
        let mix = &mut self.mix[..frames * CHANNELS];
        mix.fill(0.0);
        let buf = &mut self.deck_buf[..frames * CHANNELS];
        for deck in self.decks.iter_mut() {
            let outcome = deck.render(buf);
            if outcome == BlockOutcome::Rendered {
                for (m, s) in mix.iter_mut().zip(buf.iter()) {
                    *m += *s;
                }
            }
            deck.publish(outcome);
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..MAX_COMMANDS_PER_CALLBACK {
            // A load hands the old source to the retire ring; wait for room rather than
            // freeing it here.
            let swaps_source = match self.commands.peek() {
                Ok(Command::Deck { cmd, .. }) => {
                    matches!(cmd, DeckCommand::Load(_) | DeckCommand::Unload)
                }
                Err(_) => return,
            };
            if swaps_source && self.retire.slots() == 0 {
                return;
            }
            let Ok(Command::Deck { deck, cmd }) = self.commands.pop() else {
                return;
            };
            let Some(d) = self.decks.get_mut(deck) else {
                // Unknown deck: the command may own a source; retire it, never drop it here.
                if let DeckCommand::Load(src) = cmd {
                    let _ = self.retire.push(src);
                }
                continue;
            };
            let old = match cmd {
                DeckCommand::Load(src) => d.swap_source(Some(src)),
                DeckCommand::Unload => d.swap_source(None),
                ref other => {
                    d.apply(other);
                    None
                }
            };
            if let Some(old) = old {
                // Room was checked above; the push cannot fail.
                let _ = self.retire.push(old);
            }
        }
    }
}

/// Copy the stereo master into the device layout: channels 0/1 get L/R, a mono device gets
/// the average, any extra channels are silent (the cue bus goes to 2/3 later).
fn write_master(mix: &[f32], out: &mut [f32], channels: usize) {
    for (frame, lr) in out
        .chunks_exact_mut(channels)
        .zip(mix.chunks_exact(CHANNELS))
    {
        if channels == 1 {
            frame[0] = 0.5 * (lr[0] + lr[1]);
        } else {
            frame[0] = lr[0];
            frame[1] = lr[1];
            frame[2..].fill(0.0);
        }
    }
}

impl EngineHandle {
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Queue a command for the next callback.
    pub fn send(&mut self, cmd: Command) -> Result<(), CommandRingFull> {
        self.commands
            .push(cmd)
            .map_err(|rtrb::PushError::Full(c)| CommandRingFull(c))
    }

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

    /// Drop sources the callback has retired. Call regularly from a non-real-time thread.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(channels: usize) -> (Engine, EngineHandle) {
        Engine::new(EngineConfig {
            sample_rate: 48_000,
            channels,
            decks: 1,
        })
    }

    fn constant(frames: usize, value: f32) -> Arc<Source> {
        Source::from_interleaved(48_000, &vec![value; frames * CHANNELS])
    }

    #[test]
    fn empty_engine_outputs_silence() {
        let (mut e, _h) = engine(2);
        let mut out = vec![1.0; 256];
        e.process(&mut out, 128);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn maps_master_to_device_channels() {
        let (mut e, mut h) = engine(4);
        e.deck_mut(0).set_declick(false);
        h.send_deck(0, DeckCommand::Load(constant(1000, 0.25)))
            .unwrap();
        h.send_deck(0, DeckCommand::Play).unwrap();
        let mut out = vec![9.0; 4 * 64];
        e.process(&mut out, 64);
        for f in out.chunks_exact(4) {
            assert_eq!(f, [0.25, 0.25, 0.0, 0.0]);
        }
        let (mut e, mut h) = engine(1);
        e.deck_mut(0).set_declick(false);
        h.send_deck(0, DeckCommand::Load(constant(1000, 0.5)))
            .unwrap();
        h.send_deck(0, DeckCommand::Play).unwrap();
        let mut out = vec![9.0; 64];
        e.process(&mut out, 64);
        assert!(out.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn long_device_buffers_are_split_into_blocks() {
        let (mut e, mut h) = engine(2);
        h.send_deck(0, DeckCommand::Load(constant(10_000, 0.1)))
            .unwrap();
        h.send_deck(0, DeckCommand::Play).unwrap();
        let frames = MAX_BLOCK_FRAMES * 2 + 17;
        let mut out = vec![0.0; frames * 2];
        e.process(&mut out, frames);
        assert_eq!(h.deck_state(0).position.load(), frames as f64);
    }

    #[test]
    fn replaced_sources_are_retired_not_dropped() {
        let (mut e, mut h) = engine(2);
        let first = constant(100, 0.0);
        let weak = Arc::downgrade(&first);
        h.send_deck(0, DeckCommand::Load(first)).unwrap();
        h.send_deck(0, DeckCommand::Load(constant(100, 0.0)))
            .unwrap();
        let mut out = vec![0.0; 256];
        e.process(&mut out, 128);
        assert!(weak.upgrade().is_some(), "still alive in the retire ring");
        assert_eq!(h.collect_retired(), 1);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn full_retire_ring_defers_loads() {
        let (mut e, mut h) = engine(2);
        for _ in 0..RETIRE_CAPACITY + 3 {
            h.send_deck(0, DeckCommand::Load(constant(10, 0.0)))
                .unwrap();
        }
        let mut out = vec![0.0; 256];
        e.process(&mut out, 128);
        // First load retires nothing; then RETIRE_CAPACITY retire; the rest wait.
        assert_eq!(h.collect_retired(), RETIRE_CAPACITY);
        e.process(&mut out, 128);
        assert_eq!(h.collect_retired(), 2);
    }
}
