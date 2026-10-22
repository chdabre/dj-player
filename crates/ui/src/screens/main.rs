//! The main playback screen ("1 · Normal playback"): the title bar, phase meters and zoom
//! waveform over the status band and overview.

use super::deck::{cue_column, phase, zoom};
use super::{Target, Ui, View, chrome, deck};
use crate::kit::Rect;
use crate::render::scene::Rgba;
use crate::waves::OverviewCache;

const ZOOM: Rect = Rect::new(113.0, 172.0, 703.0, 211.0);

/// Lay out the main screen.
pub(crate) fn build(ui: &mut Ui, view: &View, overview: &mut OverviewCache) {
    let t = ui.t;
    // Frame: rail, tab row, info, status band, bottom strip, column dividers.
    chrome::rail_panel(ui);
    ui.section(Rect::new(106.0, 2.0, 915.0, 64.0));
    ui.section(Rect::new(106.0, 66.0, 915.0, 325.0));
    chrome::lower(ui);
    ui.scene
        .fill(822.0, 117.0, 1.0, 269.0, t.color.section_edge);

    chrome::rail(ui, view, Target::Usb);
    chrome::tabs(ui, None);
    info(ui, view);
    deck::status(ui, view);
    deck::bottom(ui, view, overview);
}

fn info(ui: &mut Ui, v: &View) {
    let t = ui.t;
    // Title bar with the track colour on the left.
    ui.scene.rounded(
        113.0,
        69.0,
        903.0,
        43.0,
        t.shape.button_radius,
        t.color.title_bar,
        t.shape.border,
        t.color.button_edge,
    );
    ui.scene.rounded(
        113.0,
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
    ui.chip(
        Rect::new(828.0, 344.0, 88.0, 36.0),
        &zoom_label,
        t.text.chip,
        t.color.chip_edge,
        t.color.chip_selected,
    );
    ui.hit(Target::Zoom, Rect::new(828.0, 344.0, 88.0, 36.0));
    ui.chip(
        Rect::new(923.0, 344.0, 93.0, 36.0),
        "GRID",
        t.text.chip,
        t.color.chip_edge,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::Grid, Rect::new(923.0, 344.0, 93.0, 36.0));
}
