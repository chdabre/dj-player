//! A search hit's sound quality, from its file name and the attributes the peer sent, and
//! the order hits are shown in: best first.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::Hit;

/// The audio formats a hit can be; anything else is left out of the results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Flac,
    Wav,
    Aiff,
    Alac,
    Mp3,
    /// AAC, or ALAC in an `.m4a` (the name can't tell).
    M4a,
    Ogg,
    Opus,
}

impl Format {
    /// The format of a file, by its extension.
    pub fn of(name: &str) -> Option<Format> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match ext.as_str() {
            "flac" => Format::Flac,
            "wav" => Format::Wav,
            "aif" | "aiff" => Format::Aiff,
            "alac" => Format::Alac,
            "mp3" => Format::Mp3,
            "m4a" | "aac" => Format::M4a,
            "ogg" => Format::Ogg,
            "opus" => Format::Opus,
            _ => return None,
        })
    }

    pub fn lossless(self) -> bool {
        matches!(
            self,
            Format::Flac | Format::Wav | Format::Aiff | Format::Alac
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Flac => "FLAC",
            Format::Wav => "WAV",
            Format::Aiff => "AIFF",
            Format::Alac => "ALAC",
            Format::Mp3 => "MP3",
            Format::M4a => "M4A",
            Format::Ogg => "OGG",
            Format::Opus => "OPUS",
        }
    }
}

/// The attributes of a search result's file (the protocol's numbering).
const BITRATE: u32 = 0;
const DURATION: u32 = 1;
const VBR: u32 = 2;
const SAMPLE_RATE: u32 = 4;
const BIT_DEPTH: u32 = 5;

/// What a file sounds like, as far as its peer says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    pub format: Format,
    /// kbps; for lossless files the peer's figure is the compressed rate, so it is not used.
    pub bitrate: Option<u32>,
    pub vbr: bool,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    /// Seconds.
    pub duration: Option<u32>,
}

impl Quality {
    /// The quality of a `format` file with these attributes.
    pub fn new(format: Format, attributes: &HashMap<u32, u32>) -> Self {
        let get = |k| attributes.get(&k).copied().filter(|&v| v > 0);
        Self {
            format,
            bitrate: get(BITRATE),
            vbr: get(VBR) == Some(1),
            sample_rate: get(SAMPLE_RATE),
            bit_depth: get(BIT_DEPTH),
            duration: get(DURATION),
        }
    }

    /// What the file carries, in kbps: a lossless file's PCM rate (CD quality where the peer
    /// doesn't say), a lossy file's bitrate. `None`: a lossy file of unknown bitrate.
    pub fn kbps(&self) -> Option<u32> {
        if self.format.lossless() {
            let rate = self.sample_rate.unwrap_or(44_100);
            let depth = self.bit_depth.unwrap_or(16);
            Some(rate * depth * 2 / 1000)
        } else {
            self.bitrate
        }
    }

    /// `FLAC 24/96`, `FLAC`, `MP3 320`, `MP3 245 VBR`, `OGG`.
    pub fn label(&self) -> String {
        let f = self.format.label();
        if self.format.lossless() {
            return match (self.bit_depth, self.sample_rate) {
                (Some(d), Some(r)) => format!("{f} {d}/{}", khz(r)),
                _ => f.to_string(),
            };
        }
        match (self.bitrate, self.vbr) {
            (Some(b), true) => format!("{f} {b} VBR"),
            (Some(b), false) => format!("{f} {b}"),
            (None, _) => f.to_string(),
        }
    }
}

/// `44.1`, `48`, `96`.
fn khz(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        (rate / 1000).to_string()
    } else {
        format!("{:.1}", f64::from(rate) / 1000.0)
    }
}

/// The order hits are shown in: lossless first, then by rate; among equals, peers with a free
/// slot (they start sending at once), then faster peers.
pub fn best_first(a: &Hit, b: &Hit) -> Ordering {
    let key = |h: &Hit| {
        (
            h.quality.format.lossless(),
            h.quality.kbps().unwrap_or(0),
            h.free_slot,
            h.speed,
        )
    };
    key(b)
        .cmp(&key(a))
        .then_with(|| a.name().cmp(b.name()))
        .then_with(|| a.user.cmp(&b.user))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(u32, u32)]) -> HashMap<u32, u32> {
        pairs.iter().copied().collect()
    }

    fn hit(name: &str, pairs: &[(u32, u32)], free_slot: bool, speed: u32) -> Hit {
        let format = Format::of(name).unwrap();
        Hit {
            user: "peer".into(),
            path: format!("@@music\\{name}"),
            size: 1,
            quality: Quality::new(format, &attrs(pairs)),
            free_slot,
            speed,
        }
    }

    #[test]
    fn formats_come_from_the_extension() {
        assert_eq!(Format::of("a\\b\\Track.FLAC"), Some(Format::Flac));
        assert_eq!(Format::of("x.aif"), Some(Format::Aiff));
        assert_eq!(Format::of("cover.jpg"), None);
        assert_eq!(Format::of("no extension"), None);
        assert!(Format::Wav.lossless() && !Format::Mp3.lossless());
    }

    #[test]
    fn labels_say_depth_and_rate_or_bitrate() {
        let q = |name, pairs: &[(u32, u32)]| {
            Quality::new(Format::of(name).unwrap(), &attrs(pairs)).label()
        };
        assert_eq!(q("a.flac", &[(5, 24), (4, 96_000)]), "FLAC 24/96");
        assert_eq!(q("a.flac", &[(5, 16), (4, 44_100)]), "FLAC 16/44.1");
        assert_eq!(q("a.flac", &[(0, 900)]), "FLAC");
        assert_eq!(q("a.mp3", &[(0, 320)]), "MP3 320");
        assert_eq!(q("a.mp3", &[(0, 245), (2, 1)]), "MP3 245 VBR");
        assert_eq!(q("a.ogg", &[]), "OGG");
    }

    #[test]
    fn lossless_counts_its_pcm_rate_not_the_compressed_one() {
        let flac = Quality::new(Format::Flac, &attrs(&[(0, 700)]));
        assert_eq!(
            flac.kbps(),
            Some(1411),
            "CD quality where the peer doesn't say"
        );
        let hi = Quality::new(Format::Flac, &attrs(&[(5, 24), (4, 96_000)]));
        assert_eq!(hi.kbps(), Some(4608));
        assert_eq!(Quality::new(Format::Mp3, &attrs(&[])).kbps(), None);
    }

    #[test]
    fn hits_rank_lossless_then_bitrate_then_free_slot_and_speed() {
        let mut hits = [
            hit("a.mp3", &[(0, 128)], true, 9_000),
            hit("b.mp3", &[(0, 320)], false, 100),
            hit("c.flac", &[], false, 10),
            hit("d.mp3", &[(0, 320)], true, 50),
            hit("e.mp3", &[(0, 320)], true, 500),
            hit("f.wav", &[(5, 24), (4, 48_000)], false, 10),
            hit("g.mp3", &[], true, 1_000_000),
        ];
        hits.sort_by(best_first);
        let names: Vec<_> = hits.iter().map(Hit::name).collect();
        assert_eq!(
            names,
            [
                "f.wav", "c.flac", "e.mp3", "d.mp3", "b.mp3", "a.mp3", "g.mp3"
            ]
        );
    }
}
