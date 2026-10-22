//! [`Deck::publish`]: the fast state out of the callback, once per block.

use std::sync::atomic::Ordering;

use super::{BlockOutcome, Deck};

impl Deck {
    /// Publish the fast state. Real-time safe.
    pub(crate) fn publish(&self, outcome: BlockOutcome) {
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
        s.scratching.store(t.is_scratching(), Ordering::Relaxed);
        s.position.store(t.position());
        s.shadow_position.store(t.shadow_position());
        s.rate.store(t.rate());
        s.cue.store(self.cues.cue);
        s.hot_cues.store(self.cues.hot_mask(), Ordering::Relaxed);
        for (slot, hot) in s.hot_cue_positions.iter().zip(self.cues.hot) {
            slot.store(hot.unwrap_or(f64::NAN));
        }
        s.cues_serial.store(self.cues_serial, Ordering::Relaxed);
        s.quantize.store(self.cues.quantize.unwrap_or(0.0));
        let (looping, l) = match (t.active_loop(), t.stored_loop()) {
            (Some(l), _) => (true, Some(l)),
            (None, l) => (false, l),
        };
        s.looping.store(looping, Ordering::Relaxed);
        s.loop_start.store(l.map_or(f64::NAN, |l| l.start));
        s.loop_end.store(l.map_or(f64::NAN, |l| l.end));
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
