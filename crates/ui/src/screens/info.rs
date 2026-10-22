//! The INFO screen: the loaded track's details (artwork, names, BPM, key, time, rating,
//! comment) in place of the list, with the rail, tabs and lower bands around it. The panel is
//! the browser's (`browse::info_panel`), which also draws the side panel beside the list.

use std::path::Path;

use engined::browse::Browser;

use super::browse::{InfoView, info_fields, info_panel};
use super::chrome::{self, Chrome, Top};
use super::deck::{bottom, status};
use super::{ChromeTarget, Target, Ui, View};
use crate::app::track::LoadedTags;
use crate::waves::OverviewCache;

/// Lay out the INFO screen.
pub(crate) fn build(ui: &mut Ui, v: &View, info: &InfoView, overview: &mut OverviewCache) {
    chrome::frame(
        ui,
        v,
        Chrome {
            top: Top::Tabs {
                lit: Target::Chrome(ChromeTarget::Usb),
                tab: Some(Target::Chrome(ChromeTarget::Info)),
            },
            lower: true,
        },
    );
    info_panel(ui, info);
    status(ui, v);
    bottom(ui, v, overview);
}

/// The loaded track as the INFO screen shows it: its file, its analysis (BPM, key) and its own
/// tags as read when it loaded.
pub struct OnDeck<'a> {
    pub path: &'a Path,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub tags: &'a LoadedTags,
}

/// The INFO screen's view: the loaded track, from its own tags, the library's listing and
/// the analysis (ours win).
pub fn view(b: Option<&Browser>, on_deck: Option<OnDeck>) -> InfoView {
    match on_deck {
        Some(d) => {
            let lt = d.tags;
            let mut tags = b
                .and_then(|b| b.info(d.path))
                .unwrap_or_else(|| lt.tags.clone());
            tags.bpm = d.bpm.or(tags.bpm);
            tags.key = d.key.or(tags.key);
            tags.duration = tags.duration.or(lt.tags.duration);
            tags.comment = tags.comment.or_else(|| lt.tags.comment.clone());
            InfoView {
                source: "USB",
                position: "ON DECK".into(),
                folder: false,
                fields: info_fields(d.path, &tags),
                rating: tags.rating,
                art: lt.art.clone(),
                comment: tags.comment.clone(),
                loaded: true,
            }
        }
        None => InfoView {
            source: "USB",
            position: "NO TRACK".into(),
            fields: vec![("TITLE", "No track loaded".into())],
            ..InfoView::default()
        },
    }
}
