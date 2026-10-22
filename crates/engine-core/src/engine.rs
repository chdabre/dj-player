//! `Engine`: owns every deck and runs the body of the real-time audio callback.
//!
//! Commands come in through a bounded SPSC ring ([`rtrb`]); state goes out through the atomics
//! in [`DeckState`]. Sources replaced by a load are pushed onto a second ring and dropped by
//! whoever drains [`EngineHandle::collect_retired`], so the callback never frees memory.

use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::command::{
    COMMAND_CAPACITY, Command, MAX_COMMANDS_PER_CALLBACK, RETIRE_CAPACITY, Retired,
};
use crate::deck::{BlockOutcome, Deck, DeckCommand, MAX_BLOCK_FRAMES};
use crate::handle::EngineHandle;
use crate::source::CHANNELS;
use crate::state::DeckState;

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
    retire: Producer<Retired>,
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
        let decks = states
            .iter()
            .map(|s| Deck::new(Arc::clone(s), config.sample_rate))
            .collect();
        let engine = Engine {
            config,
            decks,
            commands: cmd_rx,
            retire: retire_tx,
            mix: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
            deck_buf: vec![0.0; MAX_BLOCK_FRAMES * CHANNELS].into_boxed_slice(),
        };
        let handle = EngineHandle::new(config, cmd_tx, retire_rx, states);
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
            // A load hands the old source and grid to the retire ring, a new grid the old
            // grid, stored cues themselves once read; wait for room rather than freeing
            // anything here.
            let needs = match self.commands.peek() {
                Ok(Command::Deck { cmd, .. }) => match cmd {
                    DeckCommand::Load(_) | DeckCommand::Unload => 2,
                    DeckCommand::SetGrid { .. } | DeckCommand::SetCues { .. } => 1,
                    _ => 0,
                },
                Err(_) => return,
            };
            if self.retire.slots() < needs {
                return;
            }
            let Ok(Command::Deck { deck, cmd }) = self.commands.pop() else {
                return;
            };
            let Some(d) = self.decks.get_mut(deck) else {
                // Unknown deck: the command may own a source, grid or cues; retire, never drop.
                match cmd {
                    DeckCommand::Load(src) => self.retire_now(Retired::Source(src)),
                    DeckCommand::SetGrid { grid, .. } => self.retire_now(Retired::Grid(grid)),
                    DeckCommand::SetCues { cues, .. } => self.retire_now(Retired::Cues(cues)),
                    _ => {}
                }
                continue;
            };
            let (old_source, old_grid, cues) = match cmd {
                DeckCommand::Load(src) => {
                    let (s, g) = d.swap_source(Some(src));
                    (s, g, None)
                }
                DeckCommand::Unload => {
                    let (s, g) = d.swap_source(None);
                    (s, g, None)
                }
                DeckCommand::SetGrid { source_id, grid } => {
                    (None, d.set_grid(source_id, grid), None)
                }
                DeckCommand::SetCues {
                    source_id,
                    serial,
                    cues,
                } => {
                    d.set_cues(source_id, serial, &cues);
                    (None, None, Some(cues))
                }
                ref other => {
                    d.apply(other);
                    (None, None, None)
                }
            };
            if let Some(s) = old_source {
                self.retire_now(Retired::Source(s));
            }
            if let Some(g) = old_grid {
                self.retire_now(Retired::Grid(g));
            }
            if let Some(c) = cues {
                self.retire_now(Retired::Cues(c));
            }
        }
    }

    /// Hand `item` to the retire ring. [`Engine::drain_commands`] checks for room before it
    /// takes a command, so this never fails; if it did, the item would be freed here, on the
    /// audio thread.
    fn retire_now(&mut self, item: Retired) {
        let pushed = self.retire.push(item);
        debug_assert!(
            pushed.is_ok(),
            "retire ring full: room is checked before each command"
        );
    }
}

/// Copy the stereo master into the device layout: channels 0/1 get L/R, a mono device gets
/// the average, any extra channels are silent (the cue bus goes to 2/3 later).
fn write_master(mix: &[f32], out: &mut [f32], channels: usize) {
    for (frame, lr) in out
        .chunks_exact_mut(channels)
        .zip(mix.as_chunks::<CHANNELS>().0.iter())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::TrackCues;
    use crate::grid::DeckGrid;
    use crate::source::Source;

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
        for f in out.as_chunks::<4>().0.iter() {
            assert_eq!(*f, [0.25, 0.25, 0.0, 0.0]);
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
    fn grids_attach_to_their_own_track_only() {
        let (mut e, mut h) = engine(2);
        let first = constant(48_000, 0.0);
        let second = constant(48_000, 0.0);
        let grid = |bpm| Arc::new(DeckGrid::constant(0.0, bpm, 48_000.0, 48_000).unwrap());
        let mut out = vec![0.0; 256];
        h.send_deck(0, DeckCommand::Load(Arc::clone(&first)))
            .unwrap();
        h.send_deck(
            0,
            DeckCommand::SetGrid {
                source_id: first.id(),
                grid: grid(120.0),
            },
        )
        .unwrap();
        e.process(&mut out, 128);
        assert_eq!(e.deck(0).grid().unwrap().bpm(), 120.0);
        assert_eq!(h.deck_state(0).track_bpm.load(), 120.0);
        // A new track drops the grid; a late grid for the old track is retired, not applied.
        h.send_deck(0, DeckCommand::Load(Arc::clone(&second)))
            .unwrap();
        h.send_deck(
            0,
            DeckCommand::SetGrid {
                source_id: first.id(),
                grid: grid(125.0),
            },
        )
        .unwrap();
        e.process(&mut out, 128);
        assert!(e.deck(0).grid().is_none());
        assert_eq!(h.deck_state(0).track_bpm.load(), 0.0);
        assert!(h.deck_state(0).beat.load().is_nan());
        assert_eq!(h.collect_retired(), 3, "old source, old grid, stale grid");
    }

    #[test]
    fn stored_cues_are_retired_not_dropped() {
        let (mut e, mut h) = engine(2);
        let track = constant(48_000, 0.0);
        let mut out = vec![0.0; 256];
        h.send_deck(0, DeckCommand::Load(Arc::clone(&track)))
            .unwrap();
        e.process(&mut out, 128);
        h.collect_retired();
        let applied = Arc::new(TrackCues {
            cue: Some(100.0),
            ..TrackCues::default()
        });
        let stale = Arc::new(TrackCues::default());
        let (weak_applied, weak_stale) = (Arc::downgrade(&applied), Arc::downgrade(&stale));
        for (source_id, cues) in [(track.id(), applied), (track.id() + 1, stale)] {
            let cmd = DeckCommand::SetCues {
                source_id,
                serial: 1,
                cues,
            };
            h.send_deck(0, cmd).unwrap();
        }
        e.process(&mut out, 128);
        assert_eq!(e.deck(0).cues().cue, 100.0);
        assert!(weak_applied.upgrade().is_some() && weak_stale.upgrade().is_some());
        assert_eq!(h.collect_retired(), 2, "applied and stale cues");
        assert!(weak_applied.upgrade().is_none() && weak_stale.upgrade().is_none());
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
        // A load reserves two slots (source and grid). The first load retires nothing, the
        // next RETIRE_CAPACITY - 1 one source each; then a load waits for room.
        assert_eq!(h.collect_retired(), RETIRE_CAPACITY - 1);
        e.process(&mut out, 128);
        assert_eq!(h.collect_retired(), 3);
    }
}
