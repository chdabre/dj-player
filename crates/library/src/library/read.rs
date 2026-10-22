//! Reading tracks: one record with everything the player keeps for it, the browser's
//! listing, and the row helpers the reads share.

use rusqlite::{Connection, OptionalExtension, Row};

use crate::hash::{AudioHash, FileHash};
use crate::ids::{BlobId, PlaylistId, TrackId, hex};
use crate::model::{
    AnalysisRef, Cue, CueKind, FileIdentity, HotCue, Listed, Track, TrackTags, WaveformRef,
};

use super::identity::resolve;
use super::{Library, ReadError};

impl Library {
    /// The record of a track (following merges), if the library has one.
    pub fn track(&self, id: TrackId) -> Result<Option<Track>, ReadError> {
        let c = &self.conn;
        let id = resolve(c, id)?;
        let key = &id.0[..];
        let Some(mut t) = track_row(c, id)? else {
            return Ok(None);
        };
        t.locations = locations(c, key)?;
        t.cues = cues(c, key)?;
        read_hot_cues(c, key, &mut t)?;
        read_analysis(c, key, &mut t)?;
        let mut st = c.prepare_cached("SELECT old FROM aliases WHERE new = ?1 ORDER BY old")?;
        t.merged = st
            .query_map([key], |r| Ok(track_of(&r.get::<_, Vec<u8>>(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(t))
    }

    /// Every location of every track with the track's tags, by path: what the browser lists
    /// and searches.
    pub fn listing(&self) -> Result<Vec<Listed>, ReadError> {
        let mut st = self.conn.prepare_cached(
            "SELECT l.track, l.path, t.title, t.artist, t.album, t.genre, t.bpm, t.key, \
             t.duration, t.rating, \
             (t.waveform IS NOT NULL AND EXISTS (SELECT 1 FROM analysis a WHERE a.track = t.id)) \
             FROM locations l JOIN tracks t ON t.id = l.track ORDER BY l.path",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Listed {
                    track: track_of(&r.get::<_, Vec<u8>>(0)?),
                    path: r.get(1)?,
                    tags: tags_at(r, 2)?,
                    rating: r.get(9)?,
                    analysed: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Every track id (merged tracks resolve to their survivor and are not listed), in the
    /// order the library first saw them.
    pub fn tracks(&self) -> Result<Vec<TrackId>, ReadError> {
        let mut st = self
            .conn
            .prepare_cached("SELECT id FROM tracks ORDER BY rowid")?;
        let ids = st
            .query_map([], |r| Ok(track_of(&r.get::<_, Vec<u8>>(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }
}

/// The `tracks` row of `id` (already resolved): rating, audio hash, waveform and tags.
fn track_row(c: &Connection, id: TrackId) -> rusqlite::Result<Option<Track>> {
    c.query_row(
        "SELECT rating, audio_hash, audio_hash_v, waveform, waveform_overview, \
         title, artist, album, genre, bpm, key, duration FROM tracks WHERE id = ?1",
        [&id.0[..]],
        |r| {
            let mut t = Track::empty(id);
            t.rating = r.get(0)?;
            let hash: Option<Vec<u8>> = r.get(1)?;
            t.audio_hash = hash.and_then(|h| {
                Some(AudioHash {
                    v: r.get(2).ok()?,
                    hash: h.try_into().ok()?,
                })
            });
            t.waveform = match (blob_col(r, 3)?, blob_col(r, 4)?) {
                (Some(blob), Some(overview_blob)) => Some(WaveformRef {
                    blob,
                    overview_blob,
                }),
                _ => None,
            };
            t.tags = tags_at(r, 5)?;
            Ok(t)
        },
    )
    .optional()
}

fn locations(c: &Connection, key: &[u8]) -> rusqlite::Result<Vec<FileIdentity>> {
    let mut st = c.prepare_cached(
        "SELECT path, size, file_hash FROM locations WHERE track = ?1 \
         ORDER BY updated_at, path",
    )?;
    st.query_map([key], |r| {
        let hash: Vec<u8> = r.get(2)?;
        Ok(FileIdentity {
            path: r.get(0)?,
            size: r.get::<_, i64>(1)? as u64,
            file_hash: FileHash(hash.try_into().unwrap_or([0; 32])),
        })
    })?
    .collect()
}

/// Memory cues and loops with their ids, in time order.
fn cues(c: &Connection, key: &[u8]) -> rusqlite::Result<Vec<(String, Cue)>> {
    let mut st = c.prepare_cached(
        "SELECT id, kind, at, end_at, color, comment FROM cues WHERE track = ?1 \
         ORDER BY at, id",
    )?;
    st.query_map([key], |r| {
        let id: Vec<u8> = r.get(0)?;
        let kind: String = r.get(1)?;
        Ok((
            hex(&id),
            Cue {
                kind: CueKind::parse(&kind),
                at: r.get(2)?,
                end: r.get(3)?,
                color: r.get(4)?,
                comment: r.get(5)?,
            },
        ))
    })?
    .collect()
}

/// Hot cues into their slots of `t` (a slot out of range is skipped).
fn read_hot_cues(c: &Connection, key: &[u8], t: &mut Track) -> rusqlite::Result<()> {
    let mut st =
        c.prepare_cached("SELECT slot, at, end_at, color, comment FROM hot_cues WHERE track = ?1")?;
    let hot = st.query_map([key], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            HotCue {
                at: r.get(1)?,
                end: r.get(2)?,
                color: r.get(3)?,
                comment: r.get(4)?,
            },
        ))
    })?;
    for h in hot {
        let (slot, cue) = h?;
        if let Some(s) = usize::try_from(slot)
            .ok()
            .and_then(|s| t.hot_cues.get_mut(s))
        {
            *s = Some(cue);
        }
    }
    Ok(())
}

/// Analysis results into `t`, by analyzer (a row without a blob is skipped).
fn read_analysis(c: &Connection, key: &[u8], t: &mut Track) -> rusqlite::Result<()> {
    let mut st = c.prepare_cached(
        "SELECT analyzer, version, params, blob, created FROM analysis WHERE track = ?1",
    )?;
    let rows = st.query_map([key], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            blob_col(r, 3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        if let (name, version, params, Some(blob), created) = row? {
            t.analysis.insert(
                name,
                AnalysisRef {
                    version,
                    params,
                    blob,
                    created,
                },
            );
        }
    }
    Ok(())
}

/// The seven tag cache columns starting at column `i`.
fn tags_at(r: &Row, i: usize) -> rusqlite::Result<TrackTags> {
    Ok(TrackTags {
        title: r.get(i)?,
        artist: r.get(i + 1)?,
        album: r.get(i + 2)?,
        genre: r.get(i + 3)?,
        bpm: r.get(i + 4)?,
        key: r.get(i + 5)?,
        duration: r.get(i + 6)?,
    })
}

pub(super) fn playlist_of(b: &[u8]) -> PlaylistId {
    PlaylistId::from_bytes(b.try_into().unwrap_or([0; 16]))
}

pub(super) fn track_of(b: &[u8]) -> TrackId {
    TrackId(b.try_into().unwrap_or([0; 16]))
}

fn blob_col(r: &Row, i: usize) -> rusqlite::Result<Option<BlobId>> {
    let b: Option<Vec<u8>> = r.get(i)?;
    Ok(b.and_then(|b| b.try_into().ok()).map(BlobId))
}
