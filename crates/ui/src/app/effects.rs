//! What input asks the engine and the library to do. Handlers push [`Effect`]s instead of
//! calling the player, so they can be tested without audio, a window or a stick; the app
//! carries the effects out after each event, in order.

use std::path::PathBuf;

use player_api::DeckCommand;
use player_api::{Edit, PlaylistId};
use soulseek::Hit;

#[derive(Debug, Clone)]
pub enum Effect {
    /// A command to the deck.
    Deck(DeckCommand),
    /// Load this file onto the deck (and show the main screen).
    Load(PathBuf),
    /// An edit to the library.
    Library(Edit),
    /// Add these files to a playlist (records are made for files without one).
    AddFiles(PlaylistId, Vec<PathBuf>),
    /// A new playlist of these files.
    NewPlaylist {
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    },
    /// Scan the stick into the library.
    Scan,
    /// Analyse these files in a batch.
    AnalyzeAll(Vec<PathBuf>),
    StopAnalyzeAll,
    /// Search SoulSeek for this.
    SoulseekSearch(String),
    /// Download this SoulSeek hit into the stick's downloads folder.
    Download(Hit),
}

/// Effects in the order they were asked for.
pub type Effects = Vec<Effect>;
