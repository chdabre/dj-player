use symphonia::core::meta::{MetadataRevision, StandardVisualKey};

/// An embedded picture, still encoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Artwork {
    /// MIME type, if the tag says (`image/jpeg`, `image/png`).
    pub media_type: Option<String>,
    pub data: Vec<u8>,
}

/// The picture of `rev` to show: the front cover if there is one, else the first.
pub(crate) fn choose(rev: &MetadataRevision) -> Option<Artwork> {
    let visuals = || {
        rev.media
            .visuals
            .iter()
            .chain(rev.per_track.iter().flat_map(|t| &t.metadata.visuals))
    };
    visuals()
        .find(|v| v.usage == Some(StandardVisualKey::FrontCover))
        .or_else(|| visuals().next())
        .map(|v| Artwork {
            media_type: v.media_type.clone(),
            data: v.data.to_vec(),
        })
}

#[cfg(test)]
mod tests {
    use crate::tags;
    use crate::testutil::{id3_with, picture_frame, tagged_wav};

    #[test]
    fn the_front_cover_wins_else_the_first_picture() {
        let path = tagged_wav(
            "art-front.wav",
            &id3_with(&[picture_frame(0, &[1, 2, 3]), picture_frame(3, &[4, 5, 6])]),
        );
        let art = tags(&path).unwrap().artwork.unwrap();
        assert_eq!(art.media_type.as_deref(), Some("image/png"));
        assert_eq!(art.data, [4, 5, 6]);

        let path = tagged_wav(
            "art-first.wav",
            &id3_with(&[picture_frame(0, &[1, 2, 3]), picture_frame(4, &[7])]),
        );
        assert_eq!(tags(&path).unwrap().artwork.unwrap().data, [1, 2, 3]);
    }
}
