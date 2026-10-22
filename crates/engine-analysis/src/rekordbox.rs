//! Rekordbox's analysis as ours: the XML export's tempo, key and beatgrid (read by the
//! [`rekordbox`] crate) as a [`Provenance::Rekordbox`] source and as ground truth for
//! scoring.
//!
//! Its times are on Rekordbox's timeline: shift them by `-`[`timeline_offset`] before
//! comparing them with ours (see the [`rekordbox`] crate).

use crate::model::{Analysed, Analysis, Beat, Beatgrid, Key, Provenance, Tempo};
use crate::score::Truth;

pub use crate::score::Overrides;
pub use rekordbox::{TempoMarker, XmlTrack, read_collection, read_playlist};

/// Seconds to subtract from Rekordbox times of this file to land on our timeline.
pub fn timeline_offset(info: &engine_media::TrackInfo) -> f64 {
    rekordbox::timeline_offset(info.start_trim, info.file_sample_rate)
}

/// An [`XmlTrack`]'s values in our types.
pub trait XmlTrackExt {
    /// The key, if Rekordbox's text for it parses.
    fn key(&self) -> Option<Key>;
    /// The beatgrid up to `until` seconds (the track length), if Rekordbox analysed it.
    fn beatgrid(&self, until: f64) -> Option<Beatgrid>;
    /// The same values as a Rekordbox-sourced [`Analysis`], for the selection policy.
    fn analysis(&self, until: f64) -> Analysis;
    /// Reference values for scoring.
    fn truth(&self, until: f64) -> Truth;
}

impl XmlTrackExt for XmlTrack {
    fn key(&self) -> Option<Key> {
        Key::parse(&self.tonality)
    }

    fn beatgrid(&self, until: f64) -> Option<Beatgrid> {
        let beats = self.beats(until)?;
        Some(Beatgrid {
            beats: beats
                .into_iter()
                .map(|b| Beat {
                    time: b.time,
                    bar_pos: b.bar_pos,
                })
                .collect(),
        })
    }

    fn analysis(&self, until: f64) -> Analysis {
        fn wrap<T>(value: T) -> Analysed<T> {
            Analysed {
                value,
                confidence: None,
                provenance: Provenance::Rekordbox,
            }
        }
        Analysis {
            tempo: self.average_bpm.map(|bpm| wrap(Tempo { bpm })),
            beatgrid: self.beatgrid(until).map(wrap),
            key: self.key().map(wrap),
        }
    }

    fn truth(&self, until: f64) -> Truth {
        Truth {
            tempo: self.average_bpm,
            beatgrid: self.beatgrid(until),
            key: self.key(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rekordbox::TEST_XML as XML;

    #[test]
    fn rekordbox_values_carry_their_provenance() {
        let t = read_collection(XML).unwrap();
        let a = t[1].analysis(50.0);
        assert_eq!(a.tempo.unwrap().provenance, Provenance::Rekordbox);
        assert_eq!(a.key.unwrap().value, Key::minor(8));
        assert_eq!(t[2].key(), None, "\"X\" is no key");
    }

    #[test]
    fn the_grid_keeps_beats_and_bar_positions() {
        let t = read_collection(XML).unwrap();
        let g = t[2].beatgrid(5.0).unwrap();
        let pos: Vec<u8> = g.beats.iter().map(|b| b.bar_pos).collect();
        assert_eq!(pos, [2, 3, 1, 2, 3, 1]);
        assert_eq!(g.beats[0].time, 1.0);
    }
}
