//! The main playback screen ("1 · Normal playback"): the title bar, phase meters and zoom
//! waveform over the status band and overview.

use super::chrome::{self, COLUMN_X, CONTENT_W, CONTENT_X, Chrome, DIVIDER_X, Top};
use super::deck::{cue_column, phase, zoom};
use super::{ChromeTarget, DeckTarget, Target, Ui, View, deck};
use crate::kit::Rect;
use crate::render::scene::Rgba;
use crate::waves::OverviewCache;

pub(crate) const ZOOM: Rect = Rect::new(CONTENT_X, 172.0, CONTENT_W, 211.0);

/// Lay out the main screen.
pub(crate) fn build(ui: &mut Ui, view: &View, overview: &mut OverviewCache) {
    let t = ui.t;
    chrome::frame(
        ui,
        view,
        Chrome {
            top: Top::Tabs {
                lit: Target::Chrome(ChromeTarget::Usb),
                tab: None,
            },
            lower: true,
        },
    );
    // The right column's divider, beside the zoom.
    ui.scene
        .fill(DIVIDER_X, 117.0, 1.0, 269.0, t.color.section_edge);
    info(ui, view);
    deck::status(ui, view);
    deck::bottom(ui, view, overview);
}

fn info(ui: &mut Ui, v: &View) {
    let t = ui.t;
    // Title bar with the track colour on the left.
    ui.scene.rounded(
        CONTENT_X,
        69.0,
        903.0,
        43.0,
        t.shape.button_radius,
        t.color.title_bar,
        t.shape.border,
        t.color.button_edge,
    );
    ui.scene.rounded(
        CONTENT_X,
        69.0,
        6.0,
        43.0,
        t.shape.panel_radius,
        t.color.track_color,
        0.0,
        t.color.background,
    );
    ui.text(v.title, 132.0, 76.0, t.text.title);
    let key = v.deck.key().map_or_else(
        || {
            if v.deck.analysing {
                "KEY …".to_string()
            } else {
                "KEY –".to_string()
            }
        },
        |k| format!("KEY {k}"),
    );
    ui.text(key, 1003.0, 77.0, t.text.title_aside.right());

    phase(ui, v, 289.0, 338.0, 123.0);

    zoom(ui, v, ZOOM);

    // Right column: cue/loop memory and call.
    cue_column(ui, v, 159.0, 58.0, 62.0);
    let zoom_label = format!("ZOOM {}", v.zoom_beats);
    let zoom_chip = Rect::new(COLUMN_X, 344.0, 88.0, 36.0);
    ui.chip(
        zoom_chip,
        &zoom_label,
        t.text.chip,
        t.color.chip_edge,
        t.color.chip_selected,
    );
    ui.hit(Target::Deck(DeckTarget::Zoom), zoom_chip);
    let grid_chip = Rect::new(923.0, 344.0, 93.0, 36.0);
    ui.chip(
        grid_chip,
        "GRID",
        t.text.chip,
        t.color.chip_edge,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::Deck(DeckTarget::Grid), grid_chip);
}
