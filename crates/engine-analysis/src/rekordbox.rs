//! Rekordbox XML collection export (File → Export Collection in xml format): tempo, key and
//! beatgrid per track, read-only.
//!
//! Used as ground truth for scoring and as a [`Provenance::Rekordbox`] source. The grid is a
//! list of `TEMPO` anchors: from `Inizio` (seconds) the beats run at `Bpm`, and the anchor
//! itself is beat `Battito` of a `Metro` bar. Each anchor holds until the next one.
//!
//! Reading the USB export database and ANLZ files (rbox) belongs to the library crate later;
//! this only reads the XML.

use std::path::PathBuf;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::model::{Analysed, Analysis, Beat, Beatgrid, Key, Provenance, Tempo};
use crate::score::Truth;

/// One grid anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoMarker {
    /// Seconds from the start of the track.
    pub start: f64,
    pub bpm: f64,
    /// Beats per bar (the numerator of `Metro`).
    pub beats_per_bar: u8,
    /// Position of this anchor in its bar, 1 = downbeat.
    pub beat: u8,
}

/// One track of the collection.
#[derive(Debug, Clone, PartialEq)]
pub struct XmlTrack {
    pub name: String,
    pub artist: String,
    /// "MP3 File", "WAV File", ...
    pub kind: String,
    pub location: PathBuf,
    pub average_bpm: Option<f64>,
    pub key: Option<Key>,
    /// The key text as Rekordbox wrote it, when it did not parse.
    pub unparsed_key: Option<String>,
    pub tempo: Vec<TempoMarker>,
}

impl XmlTrack {
    /// The beatgrid up to `until` seconds (the track length), if Rekordbox analysed it.
    pub fn beatgrid(&self, until: f64) -> Option<Beatgrid> {
        if self.tempo.is_empty() {
            return None;
        }
        let mut beats = Vec::new();
        for (i, m) in self.tempo.iter().enumerate() {
            let end = self.tempo.get(i + 1).map_or(until, |n| n.start);
            if m.bpm <= 0.0 {
                continue;
            }
            let period = 60.0 / m.bpm;
            let per_bar = u32::from(m.beats_per_bar.max(1));
            for k in 0u32.. {
                let time = m.start + f64::from(k) * period;
                // Anchors are written in whole milliseconds, so the previous run can end a
                // millisecond or two short of the next anchor; that beat is the anchor's.
                if time >= end - 0.010 {
                    break;
                }
                let pos = (u32::from(m.beat.max(1)) - 1 + k) % per_bar + 1;
                beats.push(Beat {
                    time,
                    bar_pos: pos as u8,
                });
            }
        }
        Some(Beatgrid { beats })
    }

    /// Reference values for scoring.
    pub fn truth(&self, until: f64) -> Truth {
        Truth {
            tempo: self.average_bpm,
            beatgrid: self.beatgrid(until),
            key: self.key,
        }
    }

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

/// Corrections to Rekordbox's analysis, checked by ear: a ground truth is only as good as
/// the grids in it.
///
/// One rule per line, `#` comments allowed: `<match>,<action>[,<value>]`, where `<match>`
/// is a case-insensitive substring of "Artist – Name" (or of the file path), and the action
/// is `exclude` (drop the track, e.g. it drifts), `bpm,<value>` (replace the tempo), or
/// `shift_beats,<value>` (move the grid by that many beats, e.g. `0.5` when Rekordbox is half a
/// beat early, `-0.5` when it is half a beat late).
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    rules: Vec<(String, Override)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Override {
    Exclude,
    Bpm(f64),
    ShiftBeats(f64),
}

impl Overrides {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rules = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.rsplitn(3, ',').collect();
            let bad = || format!("overrides line {}: {line:?}", n + 1);
            let rule = match parts.as_slice() {
                [action, m] if action.trim() == "exclude" => (m.trim(), Override::Exclude),
                [value, action, m] => {
                    let v: f64 = value.trim().parse().map_err(|_| bad())?;
                    match action.trim() {
                        "bpm" => (m.trim(), Override::Bpm(v)),
                        "shift_beats" => (m.trim(), Override::ShiftBeats(v)),
                        _ => return Err(bad()),
                    }
                }
                _ => return Err(bad()),
            };
            rules.push((rule.0.to_lowercase(), rule.1));
        }
        Ok(Self { rules })
    }

    /// Apply the rules to `tracks`: excluded tracks are removed, the rest corrected.
    /// Returns how many tracks each rule matched, in rule order.
    pub fn apply(&self, tracks: &mut Vec<XmlTrack>) -> Vec<(String, usize)> {
        let mut hits = vec![0usize; self.rules.len()];
        tracks.retain_mut(|t| {
            let title = format!("{} – {}", t.artist, t.name).to_lowercase();
            let path = t.location.to_string_lossy().to_lowercase();
            let mut keep = true;
            for (i, (m, rule)) in self.rules.iter().enumerate() {
                if !title.contains(m.as_str()) && !path.contains(m.as_str()) {
                    continue;
                }
                hits[i] += 1;
                match *rule {
                    Override::Exclude => keep = false,
                    Override::Bpm(bpm) => t.average_bpm = Some(bpm),
                    Override::ShiftBeats(beats) => {
                        for marker in &mut t.tempo {
                            marker.start += beats * 60.0 / marker.bpm;
                        }
                    }
                }
            }
            keep
        });
        self.rules
            .iter()
            .zip(hits)
            .map(|((m, _), n)| (m.clone(), n))
            .collect()
    }
}

/// Read the `COLLECTION` of a Rekordbox XML export. Playlists are ignored.
pub fn read_collection(xml: &str) -> Result<Vec<XmlTrack>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut tracks = Vec::new();
    let mut in_collection = false;
    let mut current: Option<XmlTrack> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("XML error at byte {}: {e}", reader.buffer_position()))?;
        match event {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == "COLLECTION" => {
                in_collection = true;
            }
            Event::End(e) if e.name().as_ref() == "COLLECTION" => in_collection = false,
            Event::Start(e) if in_collection && e.name().as_ref() == "TRACK" => {
                current = Some(track(&e)?);
            }
            Event::Empty(e) if in_collection && e.name().as_ref() == "TRACK" => {
                tracks.push(track(&e)?);
            }
            Event::End(e) if in_collection && e.name().as_ref() == "TRACK" => {
                tracks.extend(current.take());
            }
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == "TEMPO" => {
                if let Some(t) = &mut current {
                    t.tempo.push(marker(&e)?);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(tracks)
}

fn attrs(e: &BytesStart) -> Result<Vec<(String, String)>, String> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(|e| e.to_string())?;
            let key = AsRef::<str>::as_ref(&a.key).to_owned();
            let value = a
                .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                .map_err(|e| e.to_string())?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn get<'a>(attrs: &'a [(String, String)], name: &str) -> &'a str {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map_or("", |(_, v)| v.as_str())
}

fn track(e: &BytesStart) -> Result<XmlTrack, String> {
    let a = attrs(e)?;
    let average_bpm = get(&a, "AverageBpm")
        .parse::<f64>()
        .ok()
        .filter(|b| *b > 0.0);
    let tonality = get(&a, "Tonality").trim();
    let key = Key::parse(tonality);
    Ok(XmlTrack {
        name: get(&a, "Name").into(),
        artist: get(&a, "Artist").into(),
        kind: get(&a, "Kind").into(),
        location: location_path(get(&a, "Location")),
        average_bpm,
        key,
        unparsed_key: (key.is_none() && !tonality.is_empty()).then(|| tonality.into()),
        tempo: Vec::new(),
    })
}

fn marker(e: &BytesStart) -> Result<TempoMarker, String> {
    let a = attrs(e)?;
    let num = |name: &str| {
        get(&a, name)
            .parse::<f64>()
            .map_err(|_| format!("TEMPO {name}={:?}", get(&a, name)))
    };
    let beats_per_bar = get(&a, "Metro")
        .split('/')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(4);
    Ok(TempoMarker {
        start: num("Inizio")?,
        bpm: num("Bpm")?,
        beats_per_bar,
        beat: get(&a, "Battito").parse().unwrap_or(1),
    })
}

/// `file://localhost/Users/me/a%20b.mp3` → `/Users/me/a b.mp3`.
fn location_path(url: &str) -> PathBuf {
    let path = url
        .strip_prefix("file://localhost")
        .or_else(|| url.strip_prefix("file://"))
        .unwrap_or(url);
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len() + 1
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|&b| hex(b)),
                bytes.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
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
  <PLAYLISTS><NODE Type="0" Name="ROOT"><TRACK Key="2"/></NODE></PLAYLISTS>
</DJ_PLAYLISTS>"#;

    #[test]
    fn reads_tracks_keys_and_locations() {
        let t = read_collection(XML).unwrap();
        assert_eq!(t.len(), 3, "playlist entries are not tracks");
        assert_eq!(t[0].average_bpm, None);
        assert!(t[0].tempo.is_empty() && t[0].beatgrid(10.0).is_none());
        assert_eq!(t[1].name, "Rock & Roll");
        assert_eq!(t[1].key, Some(Key::minor(8)));
        assert_eq!(t[1].location, PathBuf::from("/Users/x/Demo Track 2 é.mp3"));
        assert_eq!(t[2].key, None);
        assert_eq!(t[2].unparsed_key.as_deref(), Some("X"));
    }

    #[test]
    fn expands_anchors_into_beats() {
        let t = read_collection(XML).unwrap();
        let g = t[1].beatgrid(50.0).unwrap();
        // 0.025 .. 48.026 at 0.5 s: 96 beats; then 48.026 (1), 48.525 (2), then on to 50 s.
        assert_eq!(g.beats[0].time, 0.025);
        assert_eq!(g.beats[95].time, 0.025 + 95.0 * 0.5);
        assert_eq!(g.beats[96].time, 48.026);
        assert_eq!(g.beats[96].bar_pos, 1);
        assert_eq!(g.beats[97].time, 48.525);
        assert_eq!(g.beats[97].bar_pos, 2);
        assert_eq!(g.beats[98].bar_pos, 3);
        assert!(g.beats.last().unwrap().time < 50.0);
        assert!(g.beats.windows(2).all(|w| w[1].time > w[0].time));

        let waltz = t[2].beatgrid(5.0).unwrap();
        let pos: Vec<u8> = waltz.beats.iter().map(|b| b.bar_pos).collect();
        assert_eq!(pos, [2, 3, 1, 2, 3, 1]);
    }

    #[test]
    fn overrides_exclude_retempo_and_shift() {
        let mut t = read_collection(XML).unwrap();
        let o = Overrides::parse(
            "# checked by ear\nwaltz,exclude\nrock & roll,shift_beats,-0.5\nNOISE.wav,bpm,100\n",
        )
        .unwrap();
        let hits = o.apply(&mut t);
        assert_eq!(hits.iter().map(|h| h.1).collect::<Vec<_>>(), [1, 1, 1]);
        assert_eq!(t.len(), 2);
        assert!((t[1].tempo[0].start - (0.025 - 0.25)).abs() < 1e-12);
        assert_eq!(t[0].average_bpm, Some(100.0));
        assert!(Overrides::parse("x,frobnicate,1").is_err());
    }

    #[test]
    fn rekordbox_values_carry_their_provenance() {
        let t = read_collection(XML).unwrap();
        let a = t[1].analysis(50.0);
        assert_eq!(a.tempo.unwrap().provenance, Provenance::Rekordbox);
        assert_eq!(a.key.unwrap().value, Key::minor(8));
    }
}
