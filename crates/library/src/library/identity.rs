//! Identity: which track a file is, new records, and merging tag-edited copies by their
//! audio hash.

use rusqlite::{Connection, OptionalExtension, params};

use crate::hash::{AudioHash, track_id};
use crate::ids::TrackId;
use crate::model::FileIdentity;

use super::read::track_of;
use super::{Found, Library, Op, ReadError};

impl Library {
    /// The track `id` was merged into (itself if none).
    pub fn resolve(&self, id: TrackId) -> TrackId {
        resolve(&self.conn, id).unwrap_or(id)
    }

    /// Find the track a file belongs to by its file hash, or make its record. A known file
    /// at a new path gets that path as another location (an exact duplicate, or a moved
    /// file); a new file gets the id derived from its hash.
    pub fn find_or_create(&mut self, file: FileIdentity) -> Result<Found, ReadError> {
        let known: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT l.track FROM locations l JOIN tracks t ON t.id = l.track \
                 WHERE l.file_hash = ?1 ORDER BY t.rowid LIMIT 1",
                [&file.file_hash.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        let found = match known.map(|b| track_of(&b)) {
            Some(track) => {
                let have: bool = self.conn.query_row(
                    "SELECT count(*) > 0 FROM locations WHERE track = ?1 AND path = ?2",
                    params![&track.0[..], file.path],
                    |r| r.get(0),
                )?;
                Found {
                    track,
                    created: false,
                    new_location: !have,
                }
            }
            None => {
                let track = resolve(&self.conn, track_id(&file.file_hash))?;
                Found {
                    track,
                    created: self.has_track(track) != Some(true),
                    new_location: true,
                }
            }
        };
        if found.new_location {
            self.write(Op::Locate(found.track, file));
            if found.created {
                self.stats.tracks += 1;
            }
        }
        Ok(found)
    }

    /// Record the audio hash of `track`, then merge every other track with the same audio
    /// hash (tag-edited copies): the one this library saw first survives and the others
    /// become its aliases. Returns the surviving track.
    pub fn set_audio_hash(
        &mut self,
        track: TrackId,
        audio: AudioHash,
    ) -> Result<TrackId, ReadError> {
        let root = resolve(&self.conn, track)?;
        self.write(Op::AudioHash(root, audio));
        let mut st = self.conn.prepare_cached(
            "SELECT id FROM tracks WHERE audio_hash = ?1 AND audio_hash_v = ?2 ORDER BY rowid",
        )?;
        let same: Vec<TrackId> = st
            .query_map(params![&audio.hash[..], audio.v], |r| {
                Ok(track_of(&r.get::<_, Vec<u8>>(0)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(st);
        let survivor = same.first().copied().unwrap_or(root);
        for &from in same.iter().filter(|&&t| t != survivor) {
            self.write(Op::Merge {
                from,
                into: survivor,
            });
            self.stats.tracks = self.stats.tracks.saturating_sub(1);
        }
        Ok(survivor)
    }

    /// `Some(true)` if the track has a record, `None` if the file can't be read.
    pub(super) fn has_track(&self, t: TrackId) -> Option<bool> {
        self.conn
            .query_row("SELECT 1 FROM tracks WHERE id = ?1", [&t.0[..]], |_| Ok(()))
            .optional()
            .ok()
            .map(|r| r.is_some())
    }
}

/// The track `id` was merged into (aliases point straight at the survivor).
pub(super) fn resolve(c: &Connection, id: TrackId) -> rusqlite::Result<TrackId> {
    let new: Option<Vec<u8>> = c
        .prepare_cached("SELECT new FROM aliases WHERE old = ?1")?
        .query_row([&id.0[..]], |r| r.get(0))
        .optional()?;
    Ok(new.map_or(id, |b| track_of(&b)))
}

/// Move everything of `from` to `into`: locations and memory cues all; hot cues and
/// analysis results where `into` has none for that slot or analyzer, or an older one; track
/// fields where `into` has none. `from` is then deleted and becomes an alias of `into`, and
/// aliases of `from` point to `into`.
pub(super) fn merge(
    c: &Connection,
    from: TrackId,
    into: TrackId,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    let into = resolve(c, into)?;
    let from = resolve(c, from)?;
    if from == into {
        return Ok(());
    }
    let (f, i) = (&from.0[..], &into.0[..]);
    c.execute(
        "INSERT INTO locations(track, path, size, file_hash, updated_at, device) \
         SELECT ?2, path, size, file_hash, updated_at, device FROM locations WHERE track = ?1 \
         ON CONFLICT(track, path) DO NOTHING",
        [f, i],
    )?;
    c.execute("UPDATE cues SET track = ?2 WHERE track = ?1", [f, i])?;
    c.execute(
        "UPDATE playlist_entries SET track = ?2 WHERE track = ?1",
        [f, i],
    )?;
    c.execute(
        "INSERT INTO hot_cues(track, slot, at, end_at, color, comment, updated_at, device) \
         SELECT ?2, slot, at, end_at, color, comment, updated_at, device FROM hot_cues h \
         WHERE track = ?1 AND NOT EXISTS (SELECT 1 FROM hot_cues s WHERE s.track = ?2 \
         AND s.slot = h.slot AND s.updated_at >= h.updated_at) \
         ON CONFLICT(track, slot) DO UPDATE SET at = excluded.at, end_at = excluded.end_at, \
         color = excluded.color, comment = excluded.comment, updated_at = excluded.updated_at, \
         device = excluded.device",
        [f, i],
    )?;
    c.execute(
        "INSERT INTO analysis(track, analyzer, version, params, blob, created, updated_at, \
         device) SELECT ?2, analyzer, version, params, blob, created, updated_at, device \
         FROM analysis a WHERE track = ?1 AND NOT EXISTS (SELECT 1 FROM analysis s \
         WHERE s.track = ?2 AND s.analyzer = a.analyzer AND s.updated_at >= a.updated_at) \
         ON CONFLICT(track, analyzer) DO UPDATE SET version = excluded.version, \
         params = excluded.params, blob = excluded.blob, created = excluded.created, \
         updated_at = excluded.updated_at, device = excluded.device",
        [f, i],
    )?;
    c.execute(
        "UPDATE tracks SET \
         rating = coalesce(rating, (SELECT rating FROM tracks WHERE id = ?1)), \
         color = coalesce(color, (SELECT color FROM tracks WHERE id = ?1)), \
         comment = coalesce(comment, (SELECT comment FROM tracks WHERE id = ?1)), \
         waveform = coalesce(waveform, (SELECT waveform FROM tracks WHERE id = ?1)), \
         waveform_overview = coalesce(waveform_overview, \
           (SELECT waveform_overview FROM tracks WHERE id = ?1)) \
         WHERE id = ?2",
        [f, i],
    )?;
    c.execute("DELETE FROM tracks WHERE id = ?1", [f])?;
    c.execute(
        "UPDATE aliases SET new = ?2, updated_at = ?3, device = ?4 WHERE new = ?1",
        params![f, i, now, dev],
    )?;
    c.execute(
        "INSERT INTO aliases(old, new, updated_at, device) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(old) DO UPDATE SET new = excluded.new, updated_at = excluded.updated_at, \
         device = excluded.device",
        params![f, i, now, dev],
    )?;
    Ok(())
}
