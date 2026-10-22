//! Rekordbox XML collection export (File → Export Collection in xml format): tempo, key and
//! beatgrid per track, read-only.
//!
//! Used as ground truth for scoring and as a [`Provenance::Rekordbox`] source. The grid is a
//! list of `TEMPO` anchors: from `Inizio` (seconds) the beats run at `Bpm`, and the anchor
//! itself is beat `Battito` of a `Metro` bar. Each anchor holds until the next one.
//!
//! Reading the USB export database and ANLZ files (rbox) belongs to the library crate later;
//! this only reads the XML.
//!
//! # Timeline
//!
//! Rekordbox decodes MP3s without gapless trimming; we trim the encoder and decoder delay
//! (see `engine_media::TrackInfo::start_trim`). Its times are therefore later than ours by
//! [`timeline_offset`]: on 141 hand-checked MP3s the measured offset matched it to within
//! 0.4 ms at both 44.1 and 48 kHz (25.1 and 23.0 ms for the usual 1105-frame delay). Apply it
//! with [`XmlTrack::shift`] before comparing Rekordbox times with ours.

mod grid;
mod xml;

use crate::model::{Analysed, Analysis, Provenance, Tempo};

pub use xml::{TempoMarker, XmlTrack, read_collection, read_playlist};
// The ground-truth corrections live with the scorer; still reachable here.
pub use crate::score::Overrides;

/// Seconds to subtract from Rekordbox times of this file to land on our timeline.
pub fn timeline_offset(info: &engine_media::TrackInfo) -> f64 {
    f64::from(info.start_trim) / f64::from(info.file_sample_rate)
}

impl XmlTrack {
    /// The same values as a Rekordbox-sourced [`Analysis`], for the selection policy.
    pub fn analysis(&self, until: f64) -> Analysis {
        let wrap = |value| Analysed {
            value,
            confidence: None,
            provenance: Provenance::Rekordbox,
        };
        Analysis {
            tempo: self.average_bpm.map(|bpm| wrap(Tempo { bpm })),
            beatgrid: self.beatgrid(until).map(|g| Analysed {
                value: g,
                confidence: None,
                provenance: Provenance::Rekordbox,
            }),
            key: self.key.map(|k| Analysed {
                value: k,
                confidence: None,
                provenance: Provenance::Rekordbox,
            }),
        }
    }
}

/// A small export: a track without a grid, one with three anchors and one in 3/4, and two
/// playlists.
#[cfg(test)]
pub(crate) const TEST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DJ_PLAYLISTS Version="1.0.0">
  <PRODUCT Name="rekordbox" Version="7.2.19" Company="AlphaTheta"/>
  <COLLECTION Entries="3">
    <TRACK TrackID="1" Name="Loop" Artist="" Kind="WAV File" AverageBpm="0.00" Tonality=""
           Location="file://localhost/Users/x/NOISE.wav"/>
    <TRACK TrackID="2" Name="Rock &amp; Roll" Artist="A" Kind="MP3 File" AverageBpm="120.00"
           Tonality="Abm" Location="file://localhost/Users/x/Demo%20Track%202%20%C3%A9.mp3">
      <TEMPO Inizio="0.025" Bpm="120.00" Metro="4/4" Battito="1"/>
      <TEMPO Inizio="48.026" Bpm="120.00" Metro="4/4" Battito="1"/>
      <TEMPO Inizio="48.525" Bpm="120.00" Metro="4/4" Battito="2"/>
    </TRACK>
    <TRACK TrackID="3" Name="Waltz" Kind="MP3 File" AverageBpm="90.00" Tonality="X"
           Location="file://localhost/w.mp3">
      <TEMPO Inizio="1.0" Bpm="90.00" Metro="3/4" Battito="2"/>
    </TRACK>
  </COLLECTION>
  <PLAYLISTS><NODE Type="0" Name="ROOT"><NODE Type="1" Name="Validation" Entries="2"><TRACK Key="3"/><TRACK Key="2"/></NODE><NODE Type="1" Name="Empty" Entries="0"></NODE></NODE></PLAYLISTS>
</DJ_PLAYLISTS>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Key;

    const XML: &str = TEST_XML;

    #[test]
    fn rekordbox_values_carry_their_provenance() {
        let t = read_collection(XML).unwrap();
        let a = t[1].analysis(50.0);
        assert_eq!(a.tempo.unwrap().provenance, Provenance::Rekordbox);
        assert_eq!(a.key.unwrap().value, Key::minor(8));
    }
}
