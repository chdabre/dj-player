//! Cues between the deck and the library: the stored cues restored at load, the auto cue
//! point, hot cue edits written back, and MEMORY.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use engine_core::{DeckCommand, HOT_CUES, TrackCues};
use library::{Cue, Edit, Track};

use super::Player;
use super::cue_sync::CueSync;
use crate::store::Loaded;

impl Player {
    /// MEMORY: store the active loop, else the cue point, as a memory cue in the library.
    /// Returns a line to show.
    pub fn memory(&mut self) -> String {
        let (Some(t), Some(lib)) = (self.track(), &self.library) else {
            return "no library record yet: memory not stored".into();
        };
        let st = self.handle.deck_state(self.deck.index);
        let sr = f64::from(self.format.sample_rate);
        let cue = if st.looping.load(Ordering::Relaxed) {
            Cue::memory_loop(st.loop_start.load() / sr, st.loop_end.load() / sr)
        } else {
            Cue::memory(st.cue.load() / sr)
        };
        let line = match cue.end {
            Some(end) => format!("memory loop {:.3}-{:.3} s stored", cue.at, end),
            None => format!("memory cue {:.3} s stored", cue.at),
        };
        lib.edit(Edit::AddCue(t.track, cue));
        line
    }

    /// Send `cues` for the loaded track to the deck under the next serial. True if sent:
    /// the serial is then the one the deck's applied serial is compared against.
    fn send_cues(&mut self, cues: TrackCues) -> bool {
        let Some(d) = self.deck.loaded.as_mut() else {
            return false;
        };
        let serial = self.deck.cue_serial + 1;
        let sent = self
            .handle
            .send_deck(
                self.deck.index,
                DeckCommand::SetCues {
                    source_id: d.source.id(),
                    serial,
                    cues: Arc::new(cues),
                },
            )
            .is_ok();
        if sent {
            self.deck.cue_serial = serial;
            d.lib.cues.sent(serial);
        }
        sent
    }

    /// Hand the record's cues to the deck. `fill_only` keeps whatever the deck already has
    /// (cues the DJ set before the library answered); the next [`Player::poll`] then writes
    /// those to the library, since they differ from what it gave.
    fn restore_cues(&mut self, record: &Track, fill_only: bool) -> bool {
        let cues = CueSync::cues_of(record, fill_only, f64::from(self.format.sample_rate));
        if !self.send_cues(cues) {
            return false;
        }
        if let Some(d) = self.deck.loaded.as_mut() {
            d.lib.cues.restored(record, &cues);
        }
        true
    }

    pub(super) fn on_loaded(&mut self, l: Loaded, lines: &mut Vec<String>) {
        let Some(d) = self.deck.loaded.as_mut().filter(|d| d.serial == l.serial) else {
            return; // for a track no longer on the deck
        };
        let first = d.lib.track.is_none();
        d.lib.track = Some(l.track);
        if first {
            d.lib.cues.memory_cues = Some(l.record.cues.len());
        }
        let hot = l.record.hot_cues.iter().flatten().count();
        let cues = l.record.cues.len();
        // The first restore only fills what the DJ hasn't set while the library was being
        // read (and, with nothing stored, gives the deck a serial to compare against). On a
        // merge, the merged record's cues replace the deck's only if the DJ has not edited
        // them since they were restored.
        let untouched = d.lib.cues.untouched();
        if first || untouched {
            if !self.restore_cues(&l.record, first) {
                lines.push("command ring full, stored cues dropped".into());
            } else if hot + cues > 0 {
                lines.push(format!(
                    "library: {hot} hot cues and {cues} memory cues restored"
                ));
            }
        }
        if first {
            let Some(d) = self.deck.loaded.as_mut() else {
                return;
            };
            if let Some(w) = l.waveform {
                if d.waveform.set_stored(w) {
                    lines.push("library: waveform from the library".into());
                }
                d.lib.waveform_stored = true;
            }
            let need_hash = !l.record.audio_hash.is_some_and(|a| a.is_current());
            self.start_analysis(l.analysis, need_hash);
        }
    }

    /// The load-time cue point when the track has no memory cue: the first sound, with auto
    /// cue on. Waits for the library's answer (a stored memory cue wins). Only fills: a cue
    /// point the DJ set meanwhile stays.
    pub(super) fn apply_auto_cue(&mut self, lines: &mut Vec<String>) {
        let Some(d) = self.deck.loaded.as_mut() else {
            return;
        };
        let waveform = &d.waveform;
        let Some(at) = d
            .lib
            .cues
            .auto_cue(self.auto_cue, || waveform.first_sound())
        else {
            return;
        };
        let sent = self.send_cues(TrackCues {
            cue: Some(at),
            hot: [None; HOT_CUES],
            stored_loop: None,
            fill_only: true,
        });
        if sent {
            lines.push(format!(
                "auto cue: first sound at {:.3} s",
                at / f64::from(self.format.sample_rate)
            ));
        } else {
            lines.push("command ring full, auto cue dropped".into());
        }
    }

    /// Write hot cue changes made on the deck to the library.
    pub(super) fn write_back_cues(&mut self) {
        let (Some(d), Some(lib)) = (self.deck.loaded.as_mut(), &self.library) else {
            return;
        };
        let Some(t) = d.lib.track else {
            return;
        };
        let st = self.handle.deck_state(self.deck.index);
        let positions = std::array::from_fn(|i| st.hot_cue_positions[i].load());
        let applied = st.cues_serial.load(Ordering::Relaxed);
        let sr = f64::from(self.format.sample_rate);
        for (slot, cue) in d.lib.cues.write_back(applied, positions, sr) {
            lib.edit(Edit::SetHotCue(t.track, slot, cue));
        }
    }
}
