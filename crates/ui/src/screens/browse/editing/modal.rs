//! What the editing modals hold: a list (MENU, ADD TO PLAYLIST, a confirmation) with its
//! cursor, or a name being typed and what it is for.

use std::path::PathBuf;

use player_api::PlaylistId;

use super::Action;

/// What a name typed on the keyboard is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    NewPlaylist {
        parent: Option<PlaylistId>,
        files: Vec<PathBuf>,
    },
    NewFolder(Option<PlaylistId>),
    Rename(PlaylistId),
    ConvertTagList,
}

impl Naming {
    pub fn prompt(&self) -> &'static str {
        match self {
            Naming::NewPlaylist { .. } | Naming::ConvertTagList => "NEW PLAYLIST",
            Naming::NewFolder(_) => "NEW FOLDER",
            Naming::Rename(_) => "RENAME",
        }
    }
}

/// A modal over the browse list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Menu {
        actions: Vec<Action>,
        cursor: usize,
    },
    /// ADD TO PLAYLIST: `None` is NEW PLAYLIST.
    Pick {
        files: Vec<PathBuf>,
        lists: Vec<(String, Option<PlaylistId>)>,
        cursor: usize,
    },
    /// Typing a name on the keyboard.
    Name {
        naming: Naming,
        text: String,
    },
    /// Delete or empty, once confirmed.
    Confirm {
        question: String,
        action: Action,
        cursor: usize,
    },
}

impl Modal {
    /// Rows a list modal shows: its title and its rows (none for naming).
    pub fn rows(&self) -> Option<(String, Vec<String>, usize)> {
        match self {
            Modal::Menu { actions, cursor } => Some((
                "MENU".into(),
                actions.iter().map(Action::label).collect(),
                *cursor,
            )),
            Modal::Pick { lists, cursor, .. } => Some((
                "ADD TO PLAYLIST".into(),
                lists.iter().map(|l| l.0.clone()).collect(),
                *cursor,
            )),
            Modal::Confirm {
                question, cursor, ..
            } => Some((
                question.clone(),
                vec!["YES".into(), "CANCEL".into()],
                *cursor,
            )),
            Modal::Name { .. } => None,
        }
    }

    fn len(&self) -> usize {
        self.rows().map_or(0, |r| r.1.len())
    }

    /// Move a list modal's cursor (clamped).
    pub fn move_by(&mut self, delta: isize) {
        let last = self.len().saturating_sub(1);
        if let Modal::Menu { cursor, .. }
        | Modal::Pick { cursor, .. }
        | Modal::Confirm { cursor, .. } = self
        {
            *cursor = cursor.saturating_add_signed(delta).min(last);
        }
    }

    pub fn select(&mut self, i: usize) {
        if i < self.len()
            && let Modal::Menu { cursor, .. }
            | Modal::Pick { cursor, .. }
            | Modal::Confirm { cursor, .. } = self
        {
            *cursor = i;
        }
    }
}
