//! [`Edit`]: what the player asks for, checked and turned into ops. Playlist edits are in
//! `playlists`; the track half is checked here.

use rusqlite::OptionalExtension;

use crate::ids::{EntryId, LibraryId, PlaylistId, TrackId, hex, unhex};
use crate::model::{
    AnalysisRef, Cue, HOT_CUES, HotCue, PlaylistKind, TrackTags, WaveformRef, is_component,
};

use super::identity::resolve;
use super::{Library, Op, WriteError};

/// An edit to one track or one playlist of this library.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// A new playlist or folder (`kind` [`PlaylistKind::List`] or [`PlaylistKind::Folder`])
    /// at the end of folder `parent` (`None`: the top).
    CreatePlaylist {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        kind: PlaylistKind,
        name: String,
    },
    RenamePlaylist(PlaylistId, String),
    /// Into folder `parent` (`None`: the top), before its child `before` or at the end.
    MovePlaylist {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        before: Option<PlaylistId>,
    },
    /// A playlist with its entries, or a folder with everything in it. Never the tag list.
    DeletePlaylist(PlaylistId),
    /// Tracks into a list, in order, before entry `before` or at the end.
    AddEntries {
        playlist: PlaylistId,
        tracks: Vec<TrackId>,
        before: Option<EntryId>,
    },
    /// An entry to before `before` (in its own list) or to the end.
    MoveEntry {
        entry: EntryId,
        before: Option<EntryId>,
    },
    RemoveEntries(Vec<EntryId>),
    /// Every entry of a list.
    ClearPlaylist(PlaylistId),
    /// MAKE PLAYLIST on the tag list: its entries become the new list `id`, at the end of
    /// `parent`; the tag list is left empty.
    ConvertTagList {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
    },
    SetRating(TrackId, Option<u8>),
    /// A new memory cue or loop.
    AddCue(TrackId, Cue),
    /// Change the cue with this id; it keeps its id.
    ReplaceCue(TrackId, String, Cue),
    RemoveCue(TrackId, String),
    /// Hot cue slot 0–15 (two banks of 8); `None` deletes it.
    SetHotCue(TrackId, usize, Option<HotCue>),
    SetAnalysis(TrackId, String, AnalysisRef),
    SetWaveform(TrackId, WaveformRef),
    /// The tag cache: every field as given.
    SetTags(TrackId, TrackTags),
}

/// The track half of [`Edit`], without the track.
enum TrackEdit {
    Rating(Option<u8>),
    AddCue(Cue),
    ReplaceCue(String, Cue),
    RemoveCue(String),
    HotCue(usize, Option<HotCue>),
    Analysis(String, AnalysisRef),
    Waveform(WaveformRef),
    Tags(TrackTags),
}

impl Library {
    /// Make an edit. Edits to a track that was merged into another go to the survivor.
    /// Returns the cue's id for [`Edit::AddCue`] and [`Edit::ReplaceCue`].
    pub fn apply(&mut self, edit: Edit) -> Result<Option<String>, WriteError> {
        let (track, edit) = match edit {
            Edit::CreatePlaylist {
                id,
                parent,
                kind,
                name,
            } => return self.create_playlist(id, parent, kind, name).map(|()| None),
            Edit::RenamePlaylist(id, name) => {
                return self.rename_playlist(id, name).map(|()| None);
            }
            Edit::MovePlaylist { id, parent, before } => {
                return self.move_playlist(id, parent, before).map(|()| None);
            }
            Edit::DeletePlaylist(id) => return self.delete_playlist(id).map(|()| None),
            Edit::AddEntries {
                playlist,
                tracks,
                before,
            } => return self.add_entries(playlist, tracks, before).map(|()| None),
            Edit::MoveEntry { entry, before } => {
                return self.move_entry(entry, before).map(|()| None);
            }
            Edit::RemoveEntries(entries) => {
                self.remove_entries(entries);
                return Ok(None);
            }
            Edit::ClearPlaylist(id) => return self.clear_playlist(id).map(|()| None),
            Edit::ConvertTagList { id, parent, name } => {
                return self.convert_tag_list(id, parent, name).map(|()| None);
            }
            Edit::SetRating(t, r) => (t, TrackEdit::Rating(r)),
            Edit::AddCue(t, cue) => (t, TrackEdit::AddCue(cue)),
            Edit::ReplaceCue(t, old, cue) => (t, TrackEdit::ReplaceCue(old, cue)),
            Edit::RemoveCue(t, old) => (t, TrackEdit::RemoveCue(old)),
            Edit::SetHotCue(t, slot, cue) => (t, TrackEdit::HotCue(slot, cue)),
            Edit::SetAnalysis(t, name, a) => (t, TrackEdit::Analysis(name, a)),
            Edit::SetWaveform(t, w) => (t, TrackEdit::Waveform(w)),
            Edit::SetTags(t, tags) => (t, TrackEdit::Tags(tags)),
        };
        self.apply_track(track, edit)
    }

    fn apply_track(
        &mut self,
        track: TrackId,
        edit: TrackEdit,
    ) -> Result<Option<String>, WriteError> {
        // Without a readable file (a pulled stick) the edit is queued unchecked.
        let t = resolve(&self.conn, track).unwrap_or(track);
        if self.has_track(t) == Some(false) {
            return Err(WriteError::NoSuchTrack(track));
        }
        let (op, id) = match edit {
            TrackEdit::Rating(r) => (Op::Rating(t, r), None),
            TrackEdit::AddCue(cue) => {
                let id = LibraryId::random().bytes();
                (Op::PutCue(t, id, cue), Some(hex(&id)))
            }
            TrackEdit::ReplaceCue(old, cue) => {
                let id = self.cue_of(t, &old)?;
                (Op::PutCue(t, id, cue), Some(old))
            }
            TrackEdit::RemoveCue(old) => (Op::RemoveCue(self.cue_of(t, &old)?), None),
            TrackEdit::HotCue(slot, cue) => {
                if slot >= HOT_CUES {
                    return Err(WriteError::BadHotCueSlot(slot));
                }
                (Op::HotCue(t, slot, cue), None)
            }
            TrackEdit::Analysis(name, a) => {
                if !is_component(&name) {
                    return Err(WriteError::BadAnalyzerName(name));
                }
                (Op::Analysis(t, name, a), None)
            }
            TrackEdit::Waveform(w) => (Op::Waveform(t, w), None),
            TrackEdit::Tags(tags) => (Op::Tags(t, tags), None),
        };
        self.write(op);
        Ok(id)
    }

    /// The cue `id` of track `t`; any cue id passes when the file can't be read.
    fn cue_of(&self, t: TrackId, id: &str) -> Result<[u8; 16], WriteError> {
        let bytes = unhex::<16>(id).ok_or_else(|| WriteError::NoSuchCue(id.into()))?;
        let owner: rusqlite::Result<Option<Vec<u8>>> = self
            .conn
            .query_row("SELECT track FROM cues WHERE id = ?1", [&bytes[..]], |r| {
                r.get(0)
            })
            .optional();
        match owner {
            Ok(Some(o)) if o == t.0 => Ok(bytes),
            Ok(_) => Err(WriteError::NoSuchCue(id.into())),
            Err(_) => Ok(bytes),
        }
    }
}
