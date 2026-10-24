//! The port between the player and its UI: everything the UI may ask of the player and read
//! from it, behind one trait ([`Port`]).
//!
//! Today the player runs in the UI's process and `engined::Player` implements the port
//! directly. The design runs it as its own process (`docs/research/engine.md`, "Process and
//! threads"): the deck's fast state through a seqlock'd shared-memory block, commands and slow
//! state over a socket. Then a client implements [`Port`] and nothing in the UI changes.
//!
//! The types that cross are here, or re-exported here from the crates that define them, so
//! the UI needs no other engine crate.

mod deck;
mod library;

use std::path::{Path, PathBuf};

pub use deck::{DeckSnapshot, OnDeck, WaveformFeed};
pub use library::{BatchProgress, LibraryState, Listing, Playlists};

pub use ::library::{
    Edit, EntryId, Listed, Persistence, Playlist, PlaylistEntry, PlaylistId, PlaylistKind, TrackId,
    TrackTags,
};
pub use engine_core::{
    BEAT_LOOPS, DeckCommand, DeckGrid, FALLBACK_BPM, HOT_CUES, LoadReport, beat_period,
};
pub use surface::{Control, ControlEvent, Jog, Nav, SECONDS_PER_TURN};

/// The audio output the player opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub sample_rate: u32,
    pub channels: usize,
    pub device: String,
}

/// The controls' own state, for showing them: the tempo fader's range (the fraction either
/// way, and its label), the jog mode, and what a hand on the jog is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlsView {
    pub range: (f64, &'static str),
    pub vinyl: bool,
    pub jog: Option<Jog>,
}

/// What the UI may ask of the player and read from it.
pub trait Port {
    fn output(&self) -> Output;

    /// Send a command to the deck; false if it could not be queued.
    fn send(&mut self, cmd: DeckCommand) -> bool;
    /// A control did something: the player acts on the deck; what the browse controls ask
    /// of the screen comes back.
    fn control(&mut self, e: ControlEvent) -> Vec<Nav>;
    /// Held controls act, `dt` seconds after the last call (once per frame).
    fn tick_controls(&mut self, dt: f64);
    fn controls(&self) -> ControlsView;

    /// The deck now.
    fn deck(&self) -> DeckSnapshot;
    /// The track on the deck, if any.
    fn on_deck(&self) -> Option<OnDeck>;
    /// Load `file` onto the deck, replacing what is there.
    fn load(&mut self, file: &Path) -> Result<(), String>;
    /// Load `file` while it is still arriving at `part` (a download), `len` bytes in all:
    /// decode and analysis wait for the rest as they go, so playback starts as soon as the
    /// first chunk is decoded. Fails if not enough has arrived to open it yet.
    fn load_arriving(&mut self, file: &Path, part: &Path, len: u64) -> Result<(), String>;
    /// `file`'s download ended, complete or not (it gets its library record if complete and
    /// on the deck). A line worth showing may come back.
    fn arrived(&mut self, file: &Path, complete: bool) -> Option<String>;
    /// Take in what the player's workers finished; lines worth showing come back.
    fn poll(&mut self) -> Vec<String>;
    /// The audio callback's load since the last call.
    fn load_meter(&self) -> LoadReport;
    /// A line on memory use (the decoded track, caches).
    fn memory(&mut self) -> String;

    fn library(&self) -> LibraryState<'_>;
    /// A library edit; false without a library.
    fn library_edit(&mut self, edit: Edit) -> bool;
    /// Add `files` at the end of `playlist` (records made where missing).
    fn add_files(&mut self, playlist: PlaylistId, files: Vec<PathBuf>) -> bool;
    /// Give `files` a library record each (with their tags) where they have none: files that
    /// arrived on the stick, such as downloads. False without a library.
    fn import(&mut self, files: Vec<PathBuf>) -> bool;
    /// A new playlist of `files` at the end of folder `parent`.
    fn new_playlist(
        &mut self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    ) -> bool;
    /// Scan the stick: every audio file under its root gets a record.
    fn scan(&mut self);
    /// ANALYZE ALL over `files`; false if it could not start.
    fn analyze_all(&mut self, files: Vec<PathBuf>) -> bool;
    fn stop_analyze_all(&mut self);
}
