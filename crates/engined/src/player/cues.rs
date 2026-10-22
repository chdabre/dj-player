//! Cues between the deck and the library: the stored cues restored at load, the auto cue
//! point, hot cue edits written back, and MEMORY.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use engine_core::{DeckCommand, HOT_CUES, TrackCues};
use library::{Cue, CueKind, Edit, HotCue, Track};

use super::{AutoCue, DECK, Player};
use crate::store::Loaded;

impl Player {
    /// MEMORY: store the active loop, else the cue point, as a memory cue in the library.
    /// Returns a line to show.
    pub fn memory(&mut self) -> String {
        let (Some(t), Some(lib)) = (self.track(), &self.library) else {
            return "no library record yet: memory not stored".into();
        };
        let st = self.handle.deck_state(DECK);
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
        let Some(d) = self.loaded.as_mut() else {
            return false;
        };
        let serial = self.cue_serial + 1;
        let sent = self
            .handle
            .send_deck(
                DECK,
                DeckCommand::SetCues {
                    source_id: d.source.id(),
                    serial,
                    cues: Arc::new(cues),
                },
            )
            .is_ok();
        if sent {
            self.cue_serial = serial;
            d.lib.serial = serial;
        }
        sent
    }

    /// Hand the record's cues to the deck. `fill_only` keeps whatever the deck already has
    /// (cues the DJ set before the library answered); the next [`Player::poll`] then writes
    /// those to the library, since they differ from what it gave.
    fn restore_cues(&mut self, record: &Track, fill_only: bool) -> bool {
        let sr = f64::from(self.format.sample_rate);
        let mut hot = [None; HOT_CUES];
        for (slot, h) in hot.iter_mut().zip(&record.hot_cues) {
            *slot = h.as_ref().map(|h| h.at * sr);
        }
        let first = record.cues.first().map(|(_, c)| c);
        let cues = TrackCues {
            cue: first.map(|c| c.at * sr),
            hot,
            stored_loop: first.and_then(|c| match (c.kind, c.end) {
                (CueKind::Loop, Some(end)) => Some((c.at * sr, end * sr)),
                _ => None,
            }),
            fill_only,
        };
        if !self.send_cues(cues) {
            return false;
        }
        if let Some(d) = self.loaded.as_mut() {
            d.lib.restored = hot;
            d.lib.known = hot;
            d.lib.records = std::array::from_fn(|i| record.hot_cues.get(i).cloned().flatten());
        }
        true
    }

    pub(super) fn on_loaded(&mut self, l: Loaded, lines: &mut Vec<String>) {
        let Some(d) = self.loaded.as_mut().filter(|d| d.serial == l.serial) else {
            return; // for a track no longer on the deck
        };
        let first = d.lib.track.is_none();
        d.lib.track = Some(l.track);
        if first {
            d.lib.memory_cues = Some(l.record.cues.len());
        }
        let hot = l.record.hot_cues.iter().flatten().count();
        let cues = l.record.cues.len();
        // The first restore only fills what the DJ hasn't set while the library was being
        // read (and, with nothing stored, gives the deck a serial to compare against). On a
        // merge, the merged record's cues replace the deck's only if the DJ has not edited
        // them since they were restored.
        let untouched = d.lib.known == d.lib.restored;
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
            let Some(d) = self.loaded.as_mut() else {
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
        let Some(d) = self.loaded.as_mut() else {
            return;
        };
        if d.lib.cue_decided {
            return;
        }
        let Some(memory) = d.lib.memory_cues else {
            return; // still waiting for the record
        };
        if memory > 0 || self.auto_cue == AutoCue::Off {
            d.lib.cue_decided = true;
            return;
        }
        let Some(first) = d.waveform.first_sound() else {
            return;
        };
        d.lib.cue_decided = true;
        let Some(at) = first else {
            return; // silent all through: no cue point
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
        let (Some(d), Some(lib)) = (self.loaded.as_mut(), &self.library) else {
            return;
        };
        let Some(t) = d.lib.track else {
            return;
        };
        let st = self.handle.deck_state(DECK);
        if st.cues_serial.load(Ordering::Relaxed) != d.lib.serial {
            // The deck has not applied the stored cues yet.
            return;
        }
        let sr = f64::from(self.format.sample_rate);
        for (slot, pos) in st.hot_cue_positions.iter().enumerate() {
            let p = pos.load();
            let now = (!p.is_nan()).then_some(p);
            if now != d.lib.known[slot] {
                d.lib.known[slot] = now;
                let cue = now.map(|f| {
                    let at = f / sr;
                    match d.lib.records[slot].take() {
                        // Moved: the same cue at its new place. A loop end that would now
                        // come before the start no longer applies.
                        Some(old) => HotCue {
                            at,
                            end: old.end.filter(|&e| e > at),
                            ..old
                        },
                        None => HotCue::at(at),
                    }
                });
                d.lib.records[slot] = cue.clone();
                lib.edit(Edit::SetHotCue(t.track, slot, cue));
            }
        }
    }
}
