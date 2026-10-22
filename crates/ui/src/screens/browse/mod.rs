//! The browse screen ("Browse" on the 1024×600 canvas): the stick's folder tree (USB) or
//! every track its library knows (LIBRARY) as a list of seven rows (jacket, title, artist,
//! BPM, key, length), with the main screen's rail, tabs, status band and overview around it.
//!
//! Tapping a folder opens it; tapping a track selects it and tapping the selected track
//! loads it. The header goes up a level when not at the top; its chips change the sort
//! order, search (with an on-screen keyboard in place of the status band) and, in LIBRARY,
//! scan the stick. A drag on the list scrolls it. The browse knob (stand-in) and the arrow
//! keys move the cursor, push / Enter loads, BACK / Esc goes up. A stick with no library
//! asks once whether to scan it.

use std::sync::Arc;

use engined::browse::Thumb;

mod keyboard;
mod list;
mod modal;
mod panels;
mod state;
mod view;

#[cfg(test)]
pub use keyboard::key_count;
pub use keyboard::{BACKSPACE, CLEAR, DONE, SHIFT, Shift, key_char};
pub use state::BrowseState;
#[cfg(test)]
pub use state::Focus;
pub use view::{OnDeck, info_screen};
// The parts' layout constants and drawing, for each other (`use super::*`).
use keyboard::*;
use list::*;
use modal::*;
use panels::*;

use super::chrome::{lower, rail, rail_panel, tabs};
use super::deck::{bottom, status};
use super::{ChromeTarget, Target, Ui, View};
use crate::kit::Rect;
use crate::render::scene::{Image, Layer, Rgba, Weight};
use crate::theme::TextStyle;
use crate::waves::OverviewCache;
/// The browse screen's touch targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseTarget {
    /// A shown row of the list (0 = top).
    Row(u8),
    /// The list header: up a level.
    Back,
    /// Header chips: the sort order, search, scan the stick.
    Sort,
    Search,
    Scan,
    /// ANALYZE ALL (or STOP while it runs).
    AnalyzeAll,
    /// The "scan this stick?" question's answers.
    ScanYes,
    ScanNo,
    /// A key of the on-screen keyboard ([`KEYS`] index).
    Key(u8),
    /// A row of a modal over the browse list (its index among all the modal's rows).
    ModalRow(u16),
}

/// Rows shown at once.
pub const ROWS: usize = 7;

/// Row height, design pixels.
pub const ROW_H: f32 = 40.0;

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Folder,
    Track,
    /// A playlist (or the tag list).
    List,
}

/// One shown row.
#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    pub title: String,
    pub artist: String,
    /// BPM, key and length, right-aligned in the artist column.
    pub detail: String,
    /// The track on the deck.
    pub loaded: bool,
    pub art: Option<Arc<Thumb>>,
    /// On the tag list.
    pub tagged: bool,
    /// A playlist entry whose file isn't known: shown dimmed, can't load.
    pub missing: bool,
}

/// A list modal over the browse list (MENU, ADD TO PLAYLIST, a confirmation).
#[derive(Debug, Clone, Default)]
pub struct ModalView {
    pub title: String,
    pub rows: Vec<String>,
    pub selected: usize,
    /// The first row: a confirmation, its YES drawn as a destructive action.
    pub danger_first: bool,
}

/// Rows a modal shows at once.
pub const MODAL_ROWS: usize = 5;

/// The info panel: what is known of the selected track, in place of the list.
#[derive(Debug, Clone, Default)]
pub struct InfoView {
    /// Where the track is: `USB`, `LIBRARY`.
    pub source: &'static str,
    /// The selection's place in the list: `TRACK 002/030`.
    pub position: String,
    pub folder: bool,
    /// (label, value) rows: title, artist, album, time, BPM, key.
    pub fields: Vec<(&'static str, String)>,
    /// Stars, 0–5, if rated.
    pub rating: Option<u8>,
    /// The artwork (large once read, the list's thumbnail until then).
    pub art: Option<Arc<Thumb>>,
    pub comment: Option<String>,
    /// The track on the deck.
    pub loaded: bool,
}

/// What the browse screen shows, prepared from the browser once per frame.
#[derive(Debug, Clone, Default)]
pub struct BrowseView {
    /// Where the list is: the source, then the folders down to it (or the search).
    pub header: String,
    pub at_root: bool,
    /// Showing the library's tracks (LIBRARY) rather than the folders (USB).
    pub library: bool,
    /// The sort order's name.
    pub sort: &'static str,
    /// Searching; the keyboard is up.
    pub searching: bool,
    pub keyboard: bool,
    /// The keyboard's shift: letters show (and type) in capitals while it is on.
    pub shift: Shift,
    /// A scan under way: files done, total.
    pub scan: Option<(usize, usize)>,
    /// The library can be scanned (offered in LIBRARY).
    pub can_scan: bool,
    /// Tracks in the library without our analysis (ANALYZE ALL is offered while > 0).
    pub unanalysed: usize,
    /// ANALYZE ALL under way: files done, total, and whether it is stopping.
    pub analyzing: Option<(usize, usize, bool)>,
    /// Ask whether to scan this stick, which has no library.
    pub ask_scan: bool,
    /// The rows from the first one shown, at most [`ROWS`].
    pub rows: Vec<Row>,
    /// The cursor's row among `rows`.
    pub selected: Option<usize>,
    /// Index of the first row shown, and the entry count, for the scroll bar.
    pub first: usize,
    pub total: usize,
    /// Shown instead of rows when there are none.
    pub empty: Option<String>,
    /// The level holds tracks: the list has its title and artist columns. Without (folders
    /// only, the library menu) it is one column, with no divider.
    pub has_tracks: bool,
    /// The INFO screen: a track's details instead of the list.
    pub info: Option<InfoView>,
    /// INFO in the browser: the selected track's details beside the list.
    pub side: Option<InfoView>,
    /// The tag list is shown (the TAG LIST tab lit).
    pub tag_list: bool,
    /// A modal over the list.
    pub modal: Option<ModalView>,
    /// Shown in the header instead of `header` (naming, moving).
    pub header_override: Option<String>,
}

pub(crate) fn build(ui: &mut Ui, v: &View, b: &BrowseView, overview: &mut OverviewCache) {
    rail_panel(ui);
    ui.section(Rect::new(106.0, 2.0, 915.0, 64.0));
    ui.section(Rect::new(106.0, 66.0, 915.0, 325.0));
    // The tag list belongs to no source on the rail.
    let lit = if b.library {
        Target::Chrome(ChromeTarget::Library)
    } else if b.tag_list {
        Target::Chrome(ChromeTarget::TagList)
    } else {
        Target::Chrome(ChromeTarget::Usb)
    };
    rail(ui, v, lit);
    match &b.info {
        Some(info) => {
            tabs(ui, Some(Target::Chrome(ChromeTarget::Info)));
            info_panel(ui, info);
        }
        None => {
            tabs(
                ui,
                Some(if b.tag_list {
                    Target::Chrome(ChromeTarget::TagList)
                } else {
                    Target::Chrome(ChromeTarget::Browse)
                }),
            );
            header(ui, b);
            list(ui, b);
            if let Some(side) = &b.side {
                side_panel(ui, side);
            }
        }
    }
    if b.keyboard {
        keyboard(ui, b.shift);
    } else {
        lower(ui);
        status(ui, v);
        bottom(ui, v, overview);
    }
    // A modal or the scan question goes over the list, which dims and stops taking touches.
    let modal_up = b.info.is_none() && b.modal.is_some();
    if modal_up || b.ask_scan {
        ui.on(Layer::Overlay, |ui| {
            ui.scrim(LIST_AREA);
            if let Some(m) = b.modal.as_ref().filter(|_| modal_up) {
                modal(ui, m);
            }
            if b.ask_scan {
                ask_scan(ui);
            }
        });
    }
}

/// `m:ss` for a track length.
pub fn length(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_as_minutes_and_seconds() {
        assert_eq!(length(312.4), "5:12");
        assert_eq!(length(59.6), "1:00");
    }
}
