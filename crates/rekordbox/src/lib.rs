//! Reading Rekordbox's exports, read-only. Nothing here depends on the engine: callers turn
//! these values into their own types.
//!
//! - [`xml`]: the XML collection export (File → Export Collection in xml format): tempo, key
//!   and beatgrid per track, and playlists. The grid is a list of `TEMPO` anchors: from
//!   `Inizio` (seconds) the beats run at `Bpm`, and the anchor itself is beat `Battito` of a
//!   `Metro` bar. Each anchor holds until the next one.
//! - The USB export (`PIONEER/`: `export.pdb` and the ANLZ files) comes with the overlay
//!   (`docs/plan/library.md`).
//!
//! # Timeline
//!
//! Rekordbox decodes MP3s without gapless trimming; the player trims the encoder and decoder
//! delay. Rekordbox's times are therefore later than ours by [`timeline_offset`]: on 141
//! hand-checked MP3s the measured offset matched it to within 0.4 ms at both 44.1 and 48 kHz
//! (25.1 and 23.0 ms for the usual 1105-frame delay). Apply it with [`XmlTrack::shift`]
//! before comparing Rekordbox times with ours.

mod grid;
pub mod xml;

pub use grid::Beat;
pub use xml::{TempoMarker, XmlTrack, read_collection, read_playlist};

/// Seconds to subtract from Rekordbox times of a file to land on our timeline: its start
/// trim (frames of encoder and decoder delay that we skip) at its own sample rate.
pub fn timeline_offset(start_trim: u32, file_sample_rate: u32) -> f64 {
    f64::from(start_trim) / f64::from(file_sample_rate)
}

#[cfg(any(test, feature = "testing"))]
/// A small export: a track without a grid, one with three anchors and one in 3/4, and two
/// playlists.
pub const TEST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
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
