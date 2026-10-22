//! [`Op`]: one row-level write, what the queue holds and what a later sync layer would
//! replicate, and its SQL.

use rusqlite::{Connection, params};

use crate::hash::AudioHash;
use crate::ids::{EntryId, PlaylistId, TrackId};
use crate::model::{AnalysisRef, Cue, FileIdentity, HotCue, PlaylistKind, TrackTags, WaveformRef};

use super::identity::{merge, resolve};

/// One row-level write: what the queue holds. Each is idempotent, so replaying a queue that
/// was partly written already is harmless.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Op {
    Locate(TrackId, FileIdentity),
    AudioHash(TrackId, AudioHash),
    /// Move everything of `from` to `into` and make `from` an alias of it.
    Merge {
        from: TrackId,
        into: TrackId,
    },
    Rating(TrackId, Option<u8>),
    PutCue(TrackId, [u8; 16], Cue),
    RemoveCue([u8; 16]),
    HotCue(TrackId, usize, Option<HotCue>),
    Analysis(TrackId, String, AnalysisRef),
    Waveform(TrackId, WaveformRef),
    Tags(TrackId, TrackTags),
    PutPlaylist {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        kind: PlaylistKind,
        name: String,
        position: String,
    },
    RenamePlaylist(PlaylistId, String),
    MovePlaylist {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        position: String,
    },
    DeletePlaylist(PlaylistId),
    PutEntry {
        id: EntryId,
        playlist: PlaylistId,
        track: TrackId,
        position: String,
    },
    MoveEntry(EntryId, String),
    RemoveEntry(EntryId),
    ClearPlaylist(PlaylistId),
    /// Every entry of `from` into `to`, keeping their positions.
    MoveEntries {
        from: PlaylistId,
        to: PlaylistId,
    },
}

impl Op {
    /// Run this op's SQL. `now` and `device` stamp the rows it writes.
    pub(super) fn exec(&self, c: &Connection, device: [u8; 16], now: i64) -> rusqlite::Result<()> {
        let dev = &device[..];
        match self {
            Op::Locate(t, f) => locate(c, *t, f, dev, now),
            Op::AudioHash(t, a) => set_audio_hash(c, *t, a, dev, now),
            Op::Merge { from, into } => merge(c, *from, *into, dev, now),
            Op::Rating(t, r) => set_rating(c, *t, *r, dev, now),
            Op::PutCue(t, id, cue) => put_cue(c, *t, id, cue, dev, now),
            Op::RemoveCue(id) => {
                c.prepare_cached("DELETE FROM cues WHERE id = ?1")?
                    .execute([&id[..]])?;
                Ok(())
            }
            Op::HotCue(t, slot, h) => set_hot_cue(c, *t, *slot, h.as_ref(), dev, now),
            Op::Analysis(t, name, a) => set_analysis(c, *t, name, a, dev, now),
            Op::Waveform(t, w) => set_waveform(c, *t, w, dev, now),
            Op::Tags(t, g) => set_tags(c, *t, g, dev, now),
            Op::PutPlaylist {
                id,
                parent,
                kind,
                name,
                position,
            } => put_playlist(c, *id, *parent, *kind, name, position, dev, now),
            Op::RenamePlaylist(id, name) => {
                c.prepare_cached(
                    "UPDATE playlists SET name = ?2, updated_at = ?3, device = ?4 WHERE id = ?1",
                )?
                .execute(params![&id.bytes()[..], name, now, dev])?;
                Ok(())
            }
            Op::MovePlaylist {
                id,
                parent,
                position,
            } => move_playlist(c, *id, *parent, position, dev, now),
            Op::DeletePlaylist(id) => {
                c.prepare_cached("DELETE FROM playlists WHERE id = ?1 AND kind != 'tag'")?
                    .execute([&id.bytes()[..]])?;
                Ok(())
            }
            Op::PutEntry {
                id,
                playlist,
                track,
                position,
            } => put_entry(c, *id, *playlist, *track, position, dev, now),
            Op::MoveEntry(id, position) => {
                c.prepare_cached(
                    "UPDATE playlist_entries SET position = ?2, updated_at = ?3, device = ?4 \
                     WHERE id = ?1",
                )?
                .execute(params![&id.bytes()[..], position, now, dev])?;
                Ok(())
            }
            Op::RemoveEntry(id) => {
                c.prepare_cached("DELETE FROM playlist_entries WHERE id = ?1")?
                    .execute([&id.bytes()[..]])?;
                Ok(())
            }
            Op::ClearPlaylist(id) => {
                c.prepare_cached("DELETE FROM playlist_entries WHERE playlist = ?1")?
                    .execute([&id.bytes()[..]])?;
                Ok(())
            }
            Op::MoveEntries { from, to } => {
                c.prepare_cached(
                    "UPDATE playlist_entries SET playlist = ?2, updated_at = ?3, device = ?4 \
                     WHERE playlist = ?1",
                )?
                .execute(params![&from.bytes()[..], &to.bytes()[..], now, dev])?;
                Ok(())
            }
        }
    }
}

// Tracks

fn locate(
    c: &Connection,
    t: TrackId,
    f: &FileIdentity,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    let t = resolve(c, t)?;
    c.prepare_cached(
        "INSERT INTO tracks(id, updated_at, device) VALUES (?1, ?2, ?3) \
         ON CONFLICT(id) DO NOTHING",
    )?
    .execute(params![&t.0[..], now, dev])?;
    c.prepare_cached(
        "INSERT INTO locations(track, path, size, file_hash, updated_at, device) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(track, path) DO UPDATE SET \
         size = excluded.size, file_hash = excluded.file_hash, \
         updated_at = excluded.updated_at, device = excluded.device",
    )?
    .execute(params![
        &t.0[..],
        f.path,
        f.size as i64,
        &f.file_hash.0[..],
        now,
        dev
    ])?;
    Ok(())
}

fn set_audio_hash(
    c: &Connection,
    t: TrackId,
    a: &AudioHash,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE tracks SET audio_hash = ?2, audio_hash_v = ?3, updated_at = ?4, \
         device = ?5 WHERE id = ?1",
    )?
    .execute(params![&resolve(c, t)?.0[..], &a.hash[..], a.v, now, dev])?;
    Ok(())
}

fn set_rating(
    c: &Connection,
    t: TrackId,
    r: Option<u8>,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE tracks SET rating = ?2, updated_at = ?3, device = ?4 WHERE id = ?1")?
        .execute(params![&resolve(c, t)?.0[..], r, now, dev])?;
    Ok(())
}

fn set_tags(
    c: &Connection,
    t: TrackId,
    g: &TrackTags,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE tracks SET title = ?2, artist = ?3, album = ?4, genre = ?5, \
         bpm = ?6, key = ?7, duration = ?8, updated_at = ?9, device = ?10 \
         WHERE id = ?1",
    )?
    .execute(params![
        &resolve(c, t)?.0[..],
        g.title,
        g.artist,
        g.album,
        g.genre,
        g.bpm,
        g.key,
        g.duration,
        now,
        dev
    ])?;
    Ok(())
}

fn set_waveform(
    c: &Connection,
    t: TrackId,
    w: &WaveformRef,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE tracks SET waveform = ?2, waveform_overview = ?3, updated_at = ?4, \
         device = ?5 WHERE id = ?1",
    )?
    .execute(params![
        &resolve(c, t)?.0[..],
        &w.blob.0[..],
        &w.overview_blob.0[..],
        now,
        dev
    ])?;
    Ok(())
}

fn set_analysis(
    c: &Connection,
    t: TrackId,
    name: &str,
    a: &AnalysisRef,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO analysis(track, analyzer, version, params, blob, created, \
         updated_at, device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(track, analyzer) DO UPDATE SET version = excluded.version, \
         params = excluded.params, blob = excluded.blob, \
         created = excluded.created, updated_at = excluded.updated_at, \
         device = excluded.device",
    )?
    .execute(params![
        &resolve(c, t)?.0[..],
        name,
        a.version,
        a.params,
        &a.blob.0[..],
        a.created,
        now,
        dev
    ])?;
    Ok(())
}

// Cues

fn put_cue(
    c: &Connection,
    t: TrackId,
    id: &[u8; 16],
    cue: &Cue,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO cues(id, track, kind, at, end_at, color, comment, updated_at, \
         device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(id) DO UPDATE SET track = excluded.track, kind = excluded.kind, \
         at = excluded.at, end_at = excluded.end_at, color = excluded.color, \
         comment = excluded.comment, updated_at = excluded.updated_at, \
         device = excluded.device",
    )?
    .execute(params![
        &id[..],
        &resolve(c, t)?.0[..],
        cue.kind.as_str(),
        cue.at,
        cue.end,
        cue.color,
        cue.comment,
        now,
        dev
    ])?;
    Ok(())
}

/// Hot cue `slot` of `t`; `None` deletes it.
fn set_hot_cue(
    c: &Connection,
    t: TrackId,
    slot: usize,
    h: Option<&HotCue>,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    let Some(h) = h else {
        c.prepare_cached("DELETE FROM hot_cues WHERE track = ?1 AND slot = ?2")?
            .execute(params![&resolve(c, t)?.0[..], slot as i64])?;
        return Ok(());
    };
    c.prepare_cached(
        "INSERT INTO hot_cues(track, slot, at, end_at, color, comment, updated_at, \
         device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(track, slot) DO UPDATE SET at = excluded.at, \
         end_at = excluded.end_at, color = excluded.color, \
         comment = excluded.comment, updated_at = excluded.updated_at, \
         device = excluded.device",
    )?
    .execute(params![
        &resolve(c, t)?.0[..],
        slot as i64,
        h.at,
        h.end,
        h.color,
        h.comment,
        now,
        dev
    ])?;
    Ok(())
}

// Playlists

#[allow(clippy::too_many_arguments)]
fn put_playlist(
    c: &Connection,
    id: PlaylistId,
    parent: Option<PlaylistId>,
    kind: PlaylistKind,
    name: &str,
    position: &str,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO playlists(id, parent, kind, name, position, created, updated_at, \
         device) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7) ON CONFLICT(id) DO UPDATE SET \
         parent = excluded.parent, kind = excluded.kind, name = excluded.name, \
         position = excluded.position, updated_at = excluded.updated_at, \
         device = excluded.device",
    )?
    .execute(params![
        &id.bytes()[..],
        parent.map(|p| p.bytes().to_vec()),
        kind.as_str(),
        name,
        position,
        now,
        dev
    ])?;
    Ok(())
}

fn move_playlist(
    c: &Connection,
    id: PlaylistId,
    parent: Option<PlaylistId>,
    position: &str,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE playlists SET parent = ?2, position = ?3, updated_at = ?4, \
         device = ?5 WHERE id = ?1",
    )?
    .execute(params![
        &id.bytes()[..],
        parent.map(|p| p.bytes().to_vec()),
        position,
        now,
        dev
    ])?;
    Ok(())
}

fn put_entry(
    c: &Connection,
    id: EntryId,
    playlist: PlaylistId,
    track: TrackId,
    position: &str,
    dev: &[u8],
    now: i64,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO playlist_entries(id, playlist, track, position, added, \
         updated_at, device) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6) \
         ON CONFLICT(id) DO UPDATE SET playlist = excluded.playlist, \
         track = excluded.track, position = excluded.position, \
         updated_at = excluded.updated_at, device = excluded.device",
    )?
    .execute(params![
        &id.bytes()[..],
        &playlist.bytes()[..],
        &resolve(c, track)?.0[..],
        position,
        now,
        dev
    ])?;
    Ok(())
}
