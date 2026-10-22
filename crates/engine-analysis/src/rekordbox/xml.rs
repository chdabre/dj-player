//! Reading the XML: the collection's tracks with their grid anchors, and playlists.

use std::path::PathBuf;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::model::Key;

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
    /// Rekordbox's `TrackID`, which playlists refer to.
    pub id: String,
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

/// The `TrackID`s of the playlist called `name` (anywhere in the playlist tree), in order;
/// `None` if there is no such playlist.
pub fn read_playlist(xml: &str, name: &str) -> Result<Option<Vec<String>>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_playlists = false;
    let mut depth_in_list: Option<usize> = None;
    let mut depth = 0usize;
    let mut ids = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("XML error at byte {}: {e}", reader.buffer_position()))?;
        match event {
            Event::Start(e) if e.name().as_ref() == "PLAYLISTS" => in_playlists = true,
            Event::End(e) if e.name().as_ref() == "PLAYLISTS" => in_playlists = false,
            Event::Start(e) if in_playlists && e.name().as_ref() == "NODE" => {
                depth += 1;
                let a = attrs(&e)?;
                if depth_in_list.is_none() && get(&a, "Type") == "1" && get(&a, "Name") == name {
                    depth_in_list = Some(depth);
                }
            }
            Event::End(e) if in_playlists && e.name().as_ref() == "NODE" => {
                if depth_in_list == Some(depth) {
                    return Ok(Some(ids));
                }
                depth -= 1;
            }
            Event::Empty(e) | Event::Start(e)
                if depth_in_list.is_some() && e.name().as_ref() == "TRACK" =>
            {
                ids.push(get(&attrs(&e)?, "Key").to_string());
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(depth_in_list.map(|_| ids))
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
        id: get(&a, "TrackID").into(),
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
    use crate::rekordbox::TEST_XML as XML;

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
    fn reads_playlists() {
        assert_eq!(
            read_playlist(XML, "Validation").unwrap(),
            Some(vec!["3".into(), "2".into()])
        );
        assert_eq!(read_playlist(XML, "Empty").unwrap(), Some(vec![]));
        assert_eq!(read_playlist(XML, "Nope").unwrap(), None);
    }
}
