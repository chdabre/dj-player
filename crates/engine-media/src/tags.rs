use std::fs::File;
use std::path::Path;

use symphonia::core::formats::FormatReader;
use symphonia::core::meta::StandardTag as S;

use crate::LoadError;
use crate::artwork::{self, Artwork};
use crate::decode::{Decoding, probe_format};
use crate::id3::AfterId3;

/// What a file's tags say, for the browser. Every field is optional: plenty of files carry
/// no tags at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    /// Seconds, from the container's frame count.
    pub duration: Option<f64>,
    pub comment: Option<String>,
    /// Stars, 0–5 (from the tag's rating, if it has one).
    pub rating: Option<u8>,
    /// The embedded picture: the front cover if there is one, else the first.
    pub artwork: Option<Artwork>,
}

/// Read `path`'s tags and length without decoding audio. Tags come from a probe of the whole
/// file (leading ID3v2 included); the length from the probe the loader makes, which skips the
/// ID3v2 tag. A file without a leading tag is probed once, since both see the same bytes.
pub fn tags(path: impl AsRef<Path>) -> Result<Tags, LoadError> {
    let path = path.as_ref();
    let file = AfterId3::open(path)?;
    let skipped = file.skipped();
    let mut decoding = Decoding::from_after_id3(path, file)?;
    let mut tags = Tags {
        duration: decoding
            .frames
            .map(|f| f as f64 / f64::from(decoding.sample_rate)),
        ..Tags::default()
    };
    if !skipped {
        read_into(&mut *decoding.format, &mut tags);
        return Ok(tags);
    }
    // A file whose tag trips the probe still has a length.
    if let Ok(mut format) = probe_format(path, Box::new(File::open(path)?)) {
        read_into(&mut *format, &mut tags);
    }
    Ok(tags)
}

/// Fill `tags` from the latest metadata `format` holds; the first non-empty value of each
/// field wins.
fn read_into(format: &mut dyn FormatReader, tags: &mut Tags) {
    let mut meta = format.metadata();
    let Some(rev) = meta.skip_to_latest() else {
        return;
    };
    let all = rev
        .media
        .tags
        .iter()
        .chain(rev.per_track.iter().flat_map(|t| &t.metadata.tags));
    let text = |s: &str| {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    };
    tags.artwork = artwork::choose(rev);
    for tag in all {
        match &tag.std {
            Some(S::TrackTitle(s)) => tags.title = tags.title.take().or_else(|| text(s)),
            Some(S::Artist(s)) => tags.artist = tags.artist.take().or_else(|| text(s)),
            Some(S::Album(s)) => tags.album = tags.album.take().or_else(|| text(s)),
            Some(S::Genre(s)) => tags.genre = tags.genre.take().or_else(|| text(s)),
            Some(S::InitialKey(s)) => tags.key = tags.key.take().or_else(|| text(s)),
            Some(S::Bpm(b)) if *b > 0 => tags.bpm = tags.bpm.or(Some(*b as f64)),
            Some(S::Comment(s)) => tags.comment = tags.comment.take().or_else(|| text(s)),
            // Parts per million of the scale.
            Some(S::Rating(ppm)) => {
                let stars = (f64::from(*ppm) / 1e6 * 5.0).round().clamp(0.0, 5.0) as u8;
                tags.rating = tags.rating.or(Some(stars));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{id3_with, tagged_wav, text_frame, tmp, write_wav};

    #[test]
    fn tags_are_read_from_a_leading_id3_tag() {
        let path = tagged_wav(
            "tags.wav",
            &id3_with(&[
                text_frame(b"TIT2", "Title"),
                text_frame(b"TPE1", " Artist "),
                text_frame(b"TALB", "Album"),
                text_frame(b"TCON", "House"),
                text_frame(b"TBPM", "124"),
                text_frame(b"TKEY", "Am"),
                text_frame(b"TXXX", ""),
            ]),
        );
        let t = tags(&path).unwrap();
        assert_eq!(t.title.as_deref(), Some("Title"));
        assert_eq!(t.artist.as_deref(), Some("Artist"));
        assert_eq!(t.album.as_deref(), Some("Album"));
        assert_eq!(t.genre.as_deref(), Some("House"));
        assert_eq!(t.bpm, Some(124.0));
        assert_eq!(t.key.as_deref(), Some("Am"));
        assert_eq!(t.duration, Some(0.5));
        assert_eq!(t.artwork, None);
    }

    #[test]
    fn an_untagged_file_has_only_a_length() {
        let wav = tmp("untagged.wav");
        write_wav(&wav, 48_000, 2, &[0.25; 2 * 12_000]);
        let t = tags(&wav).unwrap();
        assert_eq!(
            t,
            Tags {
                duration: Some(0.25),
                ..Tags::default()
            }
        );
        assert!(matches!(
            tags("/definitely/not/here.wav"),
            Err(LoadError::Io(_))
        ));
    }
}
