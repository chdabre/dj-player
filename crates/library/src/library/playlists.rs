//! Playlists and folders: the playlist edits, the position lookups they need, and reading
//! the tree and a list's entries.

use rusqlite::{OptionalExtension, Params, Row, params};

use crate::ids::{EntryId, PlaylistId, TrackId};
use crate::model::{Playlist, PlaylistEntry, PlaylistKind};
use crate::position;

use super::read::{playlist_of, track_of};
use super::{Library, Op, ReadError, WriteError};

impl Library {
    // Playlists: editing

    pub(super) fn create_playlist(
        &mut self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        kind: PlaylistKind,
        name: String,
    ) -> Result<(), WriteError> {
        if kind == PlaylistKind::Tag {
            return Err(WriteError::BadPlaylistEdit("there is one tag list"));
        }
        self.check_folder(parent)?;
        let position = position::after(self.last_child(parent).as_deref());
        self.write(Op::PutPlaylist {
            id,
            parent,
            kind,
            name,
            position,
        });
        Ok(())
    }

    pub(super) fn rename_playlist(
        &mut self,
        id: PlaylistId,
        name: String,
    ) -> Result<(), WriteError> {
        self.playlist_kind(id)?;
        self.write(Op::RenamePlaylist(id, name));
        Ok(())
    }

    pub(super) fn move_playlist(
        &mut self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        before: Option<PlaylistId>,
    ) -> Result<(), WriteError> {
        if self.playlist_kind(id)? == PlaylistKind::Tag {
            return Err(WriteError::BadPlaylistEdit("the tag list stays at the top"));
        }
        self.check_folder(parent)?;
        // Not into itself or a folder inside it.
        let mut up = parent;
        while let Some(p) = up {
            if p == id {
                return Err(WriteError::BadPlaylistEdit(
                    "a folder can't go inside itself",
                ));
            }
            up = self.playlist(p).ok().flatten().and_then(|p| p.parent);
        }
        let position = match before {
            Some(b) => {
                let at = self
                    .playlist(b)
                    .ok()
                    .flatten()
                    .filter(|p| p.parent == parent)
                    .ok_or(WriteError::NoSuchPlaylist(b))?;
                let prev = self.child_before(parent, &at.position, id);
                position::between(prev.as_deref(), Some(&at.position))
            }
            None => position::after(self.last_child_except(parent, id).as_deref()),
        };
        self.write(Op::MovePlaylist {
            id,
            parent,
            position,
        });
        Ok(())
    }

    pub(super) fn delete_playlist(&mut self, id: PlaylistId) -> Result<(), WriteError> {
        if self.playlist_kind(id)? == PlaylistKind::Tag {
            return Err(WriteError::BadPlaylistEdit(
                "the tag list can be cleared, not deleted",
            ));
        }
        self.write(Op::DeletePlaylist(id));
        Ok(())
    }

    pub(super) fn add_entries(
        &mut self,
        playlist: PlaylistId,
        tracks: Vec<TrackId>,
        before: Option<EntryId>,
    ) -> Result<(), WriteError> {
        if self.playlist_kind(playlist)? == PlaylistKind::Folder {
            return Err(WriteError::BadPlaylistEdit("a folder holds playlists"));
        }
        let (prev, at) = self.entry_gap(playlist, before, None)?;
        let positions = match at {
            Some(at) => position::between_n(prev.as_deref(), Some(&at), tracks.len()),
            None => position::after_n(prev.as_deref(), tracks.len()),
        };
        for (track, position) in tracks.into_iter().zip(positions) {
            self.write(Op::PutEntry {
                id: EntryId::random(),
                playlist,
                track,
                position,
            });
        }
        Ok(())
    }

    pub(super) fn move_entry(
        &mut self,
        entry: EntryId,
        before: Option<EntryId>,
    ) -> Result<(), WriteError> {
        let (playlist, _) = self.entry(entry)?.ok_or(WriteError::NoSuchEntry(entry))?;
        let (prev, at) = self.entry_gap(playlist, before, Some(entry))?;
        let position = position::between(prev.as_deref(), at.as_deref());
        self.write(Op::MoveEntry(entry, position));
        Ok(())
    }

    pub(super) fn remove_entries(&mut self, entries: Vec<EntryId>) {
        for e in entries {
            self.write(Op::RemoveEntry(e));
        }
    }

    pub(super) fn clear_playlist(&mut self, id: PlaylistId) -> Result<(), WriteError> {
        self.playlist_kind(id)?;
        self.write(Op::ClearPlaylist(id));
        Ok(())
    }

    pub(super) fn convert_tag_list(
        &mut self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
    ) -> Result<(), WriteError> {
        self.check_folder(parent)?;
        let position = position::after(self.last_child(parent).as_deref());
        self.write(Op::PutPlaylist {
            id,
            parent,
            kind: PlaylistKind::List,
            name,
            position,
        });
        self.write(Op::MoveEntries {
            from: PlaylistId::TAG_LIST,
            to: id,
        });
        Ok(())
    }

    /// `parent` is the top or a folder.
    fn check_folder(&self, parent: Option<PlaylistId>) -> Result<(), WriteError> {
        match parent {
            None => Ok(()),
            Some(p) => match self.playlist_kind(p)? {
                PlaylistKind::Folder => Ok(()),
                _ => Err(WriteError::BadPlaylistEdit("only a folder holds playlists")),
            },
        }
    }

    pub(super) fn playlist_kind(&self, id: PlaylistId) -> Result<PlaylistKind, WriteError> {
        self.playlist(id)
            .ok()
            .flatten()
            .map(|p| p.kind)
            .ok_or(WriteError::NoSuchPlaylist(id))
    }

    // Playlists: positions

    /// The neighbours new entries of `playlist` go between: before entry `before` (which must
    /// be in it) or at the end, leaving `except` out. `None` after is the end.
    fn entry_gap(
        &self,
        playlist: PlaylistId,
        before: Option<EntryId>,
        except: Option<EntryId>,
    ) -> Result<(Option<String>, Option<String>), WriteError> {
        match before {
            Some(b) => {
                let at = self
                    .entry(b)?
                    .filter(|(p, _)| *p == playlist)
                    .ok_or(WriteError::NoSuchEntry(b))?
                    .1;
                Ok((self.entry_before(playlist, &at, except), Some(at)))
            }
            None => Ok((self.last_entry(playlist, except), None)),
        }
    }

    /// `max(position)` from a query; `None` for no rows or an unreadable file.
    fn max_position(&self, sql: &str, params: impl Params) -> Option<String> {
        self.conn
            .query_row(sql, params, |r| r.get(0))
            .ok()
            .flatten()
    }

    /// The last position among `parent`'s children.
    fn last_child(&self, parent: Option<PlaylistId>) -> Option<String> {
        self.last_child_except(parent, PlaylistId::nil())
    }

    fn last_child_except(&self, parent: Option<PlaylistId>, except: PlaylistId) -> Option<String> {
        let parent = parent.map(|p| p.bytes().to_vec());
        self.max_position(
            "SELECT max(position) FROM playlists WHERE parent IS ?1 AND id != ?2",
            params![parent, &except.bytes()[..]],
        )
    }

    /// The position just before `at` among `parent`'s children, leaving `except` out.
    fn child_before(
        &self,
        parent: Option<PlaylistId>,
        at: &str,
        except: PlaylistId,
    ) -> Option<String> {
        let parent = parent.map(|p| p.bytes().to_vec());
        self.max_position(
            "SELECT max(position) FROM playlists WHERE parent IS ?1 AND position < ?2 \
             AND id != ?3",
            params![parent, at, &except.bytes()[..]],
        )
    }

    fn last_entry(&self, playlist: PlaylistId, except: Option<EntryId>) -> Option<String> {
        let except = except.unwrap_or(EntryId::nil());
        self.max_position(
            "SELECT max(position) FROM playlist_entries WHERE playlist = ?1 AND id != ?2",
            params![&playlist.bytes()[..], &except.bytes()[..]],
        )
    }

    fn entry_before(
        &self,
        playlist: PlaylistId,
        at: &str,
        except: Option<EntryId>,
    ) -> Option<String> {
        let except = except.unwrap_or(EntryId::nil());
        self.max_position(
            "SELECT max(position) FROM playlist_entries WHERE playlist = ?1 \
             AND position < ?2 AND id != ?3",
            params![&playlist.bytes()[..], at, &except.bytes()[..]],
        )
    }

    /// An entry's playlist and position.
    fn entry(&self, id: EntryId) -> Result<Option<(PlaylistId, String)>, WriteError> {
        Ok(self
            .conn
            .query_row(
                "SELECT playlist, position FROM playlist_entries WHERE id = ?1",
                [&id.bytes()[..]],
                |r| Ok((playlist_of(&r.get::<_, Vec<u8>>(0)?), r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten())
    }

    // Playlists: reading

    /// Every playlist and folder, the tag list first, then the tree depth first in order.
    pub fn playlists(&self) -> Result<Vec<Playlist>, ReadError> {
        let mut st = self.conn.prepare_cached(
            "SELECT p.id, p.parent, p.kind, p.name, p.position, \
             (SELECT count(*) FROM playlist_entries e WHERE e.playlist = p.id) \
             FROM playlists p ORDER BY p.position",
        )?;
        let all: Vec<Playlist> = st
            .query_map([], |r| {
                playlist_at(r, playlist_of(&r.get::<_, Vec<u8>>(0)?), 1)
            })?
            .collect::<rusqlite::Result<_>>()?;
        // Depth first from the top, each level in position order (`all` is sorted).
        let mut out = Vec::with_capacity(all.len());
        fn visit(all: &[Playlist], parent: Option<PlaylistId>, out: &mut Vec<Playlist>) {
            let mut kids: Vec<&Playlist> = all.iter().filter(|p| p.parent == parent).collect();
            // The tag list leads the top level.
            kids.sort_by_key(|p| (p.kind != PlaylistKind::Tag, p.position.clone()));
            for k in kids {
                out.push(k.clone());
                if k.kind == PlaylistKind::Folder {
                    visit(all, Some(k.id), out);
                }
            }
        }
        visit(&all, None, &mut out);
        Ok(out)
    }

    /// One playlist or folder.
    pub fn playlist(&self, id: PlaylistId) -> Result<Option<Playlist>, ReadError> {
        Ok(self
            .conn
            .query_row(
                "SELECT p.parent, p.kind, p.name, p.position, \
                 (SELECT count(*) FROM playlist_entries e WHERE e.playlist = p.id) \
                 FROM playlists p WHERE p.id = ?1",
                [&id.bytes()[..]],
                |r| playlist_at(r, id, 0),
            )
            .optional()?)
    }

    /// A list's entries in order, each track resolved to the survivor of any merge.
    pub fn entries(&self, playlist: PlaylistId) -> Result<Vec<PlaylistEntry>, ReadError> {
        let mut st = self.conn.prepare_cached(
            "SELECT e.id, coalesce(a.new, e.track), e.position FROM playlist_entries e \
             LEFT JOIN aliases a ON a.old = e.track WHERE e.playlist = ?1 \
             ORDER BY e.position, e.id",
        )?;
        let rows = st
            .query_map([&playlist.bytes()[..]], |r| {
                Ok(PlaylistEntry {
                    id: EntryId::from_bytes(r.get::<_, Vec<u8>>(0)?.try_into().unwrap_or([0; 16])),
                    track: track_of(&r.get::<_, Vec<u8>>(1)?),
                    position: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}

/// Playlist `id` from the parent, kind, name, position and entry count columns starting at
/// column `i`.
fn playlist_at(r: &Row, id: PlaylistId, i: usize) -> rusqlite::Result<Playlist> {
    let parent: Option<Vec<u8>> = r.get(i)?;
    Ok(Playlist {
        id,
        parent: parent.map(|p| playlist_of(&p)),
        kind: PlaylistKind::parse(&r.get::<_, String>(i + 1)?),
        name: r.get(i + 2)?,
        position: r.get(i + 3)?,
        entries: r.get::<_, i64>(i + 4)? as usize,
    })
}
