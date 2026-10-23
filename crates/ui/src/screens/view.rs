//! What the screens draw from: the deck as of this frame, the loaded track's waveform, the
//! library's save state and the UI's own state.

use super::browse::{BrowseView, InfoView};
use super::settings::SettingsView;
use super::{PerfState, ScreenKind, Target};
use crate::app::engine::DeckSnapshot;
use crate::waves::WaveSettings;

/// Everything the screen shows: the deck as of this frame, and the UI's own state.
pub struct View<'a> {
    pub deck: &'a DeckSnapshot,
    pub title: &'a str,
    /// The playhead the screen shows (frames): smoothed between engine updates, or where the
    /// finger has moved the track.
    pub position: f64,
    /// Beats across the zoom view.
    pub zoom_beats: f64,
    pub pressed: Option<Target>,
    /// Frame timing line, if the overlay is on.
    pub debug: Option<&'a str>,
    /// The track's waveform so far, if any.
    pub wave: Option<Wave<'a>>,
    pub screen: ScreenKind,
    pub perf: PerfState,
    /// Whether the library is saving edits; `None` until it says.
    pub save: Option<SaveState>,
    /// The browse screen's list, when there is something to browse.
    pub browse: Option<&'a BrowseView>,
    /// The INFO screen's track.
    pub info: Option<&'a InfoView>,
    /// The settings screen.
    pub settings: Option<&'a SettingsView>,
}

/// What the status band says about the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    /// Every edit is on the stick.
    Saved,
    /// Written, flushed within a second or two.
    Saving,
    /// Edits are in RAM only and lost at power-off (no library, a locked or failing stick).
    Unsaved,
}

impl SaveState {
    pub(super) fn label(self) -> &'static str {
        match self {
            SaveState::Saved => "SAVED",
            SaveState::Saving => "SAVING",
            SaveState::Unsaved => "UNSAVED",
        }
    }
}

/// A deck's waveform as the screen draws it.
#[derive(Clone, Copy)]
pub struct Wave<'a> {
    pub entries: &'a [waveform::compute::Entry],
    pub entries_per_second: f64,
    pub settings: WaveSettings,
    /// The theme's waveform style with this track's gain set.
    pub style: waveform::display::Style,
}
