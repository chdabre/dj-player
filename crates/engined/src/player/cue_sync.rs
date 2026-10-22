//! Cues between a deck and the library, as bookkeeping only: what to hand the deck from a
//! record, whether the DJ has changed the deck's hot cues since, the load-time cue point, and
//! which hot cue edits to write back. Nothing here talks to the engine or the library: the
//! player sends what this decides (`cues.rs`), so it is tested without an audio device.

use engine_core::{HOT_CUES, TrackCues};
use library::{CueKind, HotCue, Track};

use super::AutoCue;

/// One deck's cues against its track's record. Positions are frames at the engine rate.
#[derive(Debug, Default)]
pub(super) struct CueSync {
    /// Serial of the last stored cues sent to the deck (0 = none yet): the deck's applied
    /// serial is compared against it.
    pub(super) serial: u64,
    /// Hot cues as the library gave them, and as last seen on the deck.
    restored: [Option<f64>; HOT_CUES],
    known: [Option<f64>; HOT_CUES],
    /// The library's hot cues per slot, so a deck edit (a new position) keeps the rest:
    /// end, colour, comment and fields this version doesn't know.
    records: [Option<HotCue>; HOT_CUES],
    /// Memory cues in the record, once the library answered (0 if there is no record).
    pub(super) memory_cues: Option<usize>,
    /// The load-time cue point is decided (a memory cue, the first sound, or none).
    cue_decided: bool,
}

impl CueSync {
    /// What the deck should get from `record`: its hot cues, the first memory cue as the cue
    /// point, and that cue's loop to store for RELOOP. `fill_only` keeps what the deck has.
    pub(super) fn cues_of(record: &Track, fill_only: bool, sample_rate: f64) -> TrackCues {
        let sr = sample_rate;
        let mut hot = [None; HOT_CUES];
        for (slot, h) in hot.iter_mut().zip(&record.hot_cues) {
            *slot = h.as_ref().map(|h| h.at * sr);
        }
        let first = record.cues.first().map(|(_, c)| c);
        TrackCues {
            cue: first.map(|c| c.at * sr),
            hot,
            stored_loop: first.and_then(|c| match (c.kind, c.end) {
                (CueKind::Loop, Some(end)) => Some((c.at * sr, end * sr)),
                _ => None,
            }),
            fill_only,
        }
    }

    /// Cues were sent to the deck under `serial`.
    pub(super) fn sent(&mut self, serial: u64) {
        self.serial = serial;
    }

    /// `record`'s cues, as [`CueSync::cues_of`] made them (`cues`), are on their way to the
    /// deck: they are what the library gave and, until the deck says otherwise, what it has.
    pub(super) fn restored(&mut self, record: &Track, cues: &TrackCues) {
        self.restored = cues.hot;
        self.known = cues.hot;
        self.records = std::array::from_fn(|i| record.hot_cues.get(i).cloned().flatten());
    }

    /// The deck's hot cues are still the ones the library gave: a merged record may replace
    /// them.
    pub(super) fn untouched(&self) -> bool {
        self.known == self.restored
    }

    /// The load-time cue point when the track has no memory cue: the first sound, with auto
    /// cue on. `first_sound` asks the decoder (`None` while still looking). Some
    /// once, when it is decided that the deck gets that cue point; `None` otherwise.
    pub(super) fn auto_cue(
        &mut self,
        auto_cue: AutoCue,
        first_sound: impl FnOnce() -> Option<Option<f64>>,
    ) -> Option<f64> {
        if self.cue_decided {
            return None;
        }
        // Still waiting for the record: a stored memory cue wins.
        let memory = self.memory_cues?;
        if memory > 0 || auto_cue == AutoCue::Off {
            self.cue_decided = true;
            return None;
        }
        let first = first_sound()?;
        self.cue_decided = true;
        // Silent all through: no cue point.
        first
    }

    /// The hot cue edits to write for the deck's hot cues now (`positions`, NaN where empty),
    /// once the deck has applied the stored cues (`applied` is its serial). Each slot that
    /// changed comes back once, as the cue to store there (`None`: deleted).
    pub(super) fn write_back(
        &mut self,
        applied: u64,
        positions: [f64; HOT_CUES],
        sample_rate: f64,
    ) -> Vec<(usize, Option<HotCue>)> {
        if applied != self.serial {
            // The deck has not applied the stored cues yet.
            return Vec::new();
        }
        let mut edits = Vec::new();
        for (slot, p) in positions.into_iter().enumerate() {
            let now = (!p.is_nan()).then_some(p);
            if now == self.known[slot] {
                continue;
            }
            self.known[slot] = now;
            let cue = now.map(|f| {
                let at = f / sample_rate;
                match self.records[slot].take() {
                    // Moved: the same cue at its new place. A loop end that would now come
                    // before the start no longer applies.
                    Some(old) => HotCue {
                        at,
                        end: old.end.filter(|&e| e > at),
                        ..old
                    },
                    None => HotCue::at(at),
                }
            });
            self.records[slot] = cue.clone();
            edits.push((slot, cue));
        }
        edits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::Cue;

    const SR: f64 = 48_000.0;

    fn empty() -> [f64; HOT_CUES] {
        [f64::NAN; HOT_CUES]
    }

    /// A record with a coloured hot cue A at 2 s ending at 6 s, and a memory loop at 1-3 s.
    fn record() -> Track {
        let mut t = Track::empty(library::TrackId([7; 16]));
        t.hot_cues[0] = Some(HotCue {
            end: Some(6.0),
            color: Some("#ff0000".into()),
            ..HotCue::at(2.0)
        });
        t.cues = vec![("c1".into(), Cue::memory_loop(1.0, 3.0))];
        t
    }

    #[test]
    fn a_record_gives_hot_cues_the_cue_point_and_the_stored_loop() {
        let c = CueSync::cues_of(&record(), true, SR);
        assert_eq!(c.hot[0], Some(2.0 * SR));
        assert!(c.hot[1..].iter().all(Option::is_none));
        assert_eq!(c.cue, Some(SR));
        assert_eq!(c.stored_loop, Some((SR, 3.0 * SR)));
        assert!(c.fill_only);
    }

    #[test]
    fn nothing_is_written_back_until_the_deck_applied_the_restore() {
        let mut s = CueSync::default();
        let r = record();
        let cues = CueSync::cues_of(&r, false, SR);
        s.restored(&r, &cues);
        s.sent(3);
        let mut deck = empty();
        deck[0] = 2.0 * SR;
        deck[1] = 10.0 * SR;
        assert!(s.write_back(2, deck, SR).is_empty(), "an older serial");
        assert_eq!(s.write_back(3, deck, SR), [(1, Some(HotCue::at(10.0)))]);
        assert!(s.write_back(3, deck, SR).is_empty(), "each change once");
    }

    #[test]
    fn a_moved_hot_cue_keeps_its_colour_and_drops_an_end_before_it() {
        let mut s = CueSync::default();
        let r = record();
        s.restored(&r, &CueSync::cues_of(&r, false, SR));
        let mut deck = empty();
        deck[0] = 4.0 * SR;
        let edits = s.write_back(0, deck, SR);
        let cue = edits[0].1.as_ref().unwrap();
        assert_eq!((cue.at, cue.end), (4.0, Some(6.0)));
        assert_eq!(cue.color.as_deref(), Some("#ff0000"));
        deck[0] = 7.0 * SR;
        let cue = s.write_back(0, deck, SR)[0].1.clone().unwrap();
        assert_eq!((cue.at, cue.end), (7.0, None), "the end would come first");
        assert_eq!(cue.color.as_deref(), Some("#ff0000"));
        deck[0] = f64::NAN;
        assert_eq!(s.write_back(0, deck, SR), [(0, None)]);
    }

    #[test]
    fn a_deck_edit_makes_the_cues_touched() {
        let mut s = CueSync::default();
        let r = record();
        s.restored(&r, &CueSync::cues_of(&r, false, SR));
        assert!(s.untouched());
        let mut deck = empty();
        deck[0] = 2.0 * SR;
        deck[5] = SR;
        s.write_back(0, deck, SR);
        assert!(!s.untouched());
    }

    #[test]
    fn auto_cue_waits_for_the_record_and_a_memory_cue_wins() {
        let mut s = CueSync::default();
        assert_eq!(s.auto_cue(AutoCue::default(), || Some(Some(100.0))), None);
        s.memory_cues = Some(1);
        assert_eq!(s.auto_cue(AutoCue::default(), || Some(Some(100.0))), None);
        // Decided: a later first sound changes nothing.
        s.memory_cues = Some(0);
        assert_eq!(s.auto_cue(AutoCue::default(), || Some(Some(100.0))), None);
    }

    #[test]
    fn auto_cue_takes_the_first_sound_once() {
        let mut s = CueSync {
            memory_cues: Some(0),
            ..CueSync::default()
        };
        assert_eq!(
            s.auto_cue(AutoCue::default(), || None),
            None,
            "still looking"
        );
        assert_eq!(
            s.auto_cue(AutoCue::default(), || Some(Some(100.0))),
            Some(100.0)
        );
        assert_eq!(s.auto_cue(AutoCue::default(), || Some(Some(100.0))), None);
        let mut silent = CueSync {
            memory_cues: Some(0),
            ..CueSync::default()
        };
        assert_eq!(silent.auto_cue(AutoCue::default(), || Some(None)), None);
        assert_eq!(silent.auto_cue(AutoCue::Off, || Some(Some(5.0))), None);
    }
}
