//! The frame every screen shares: the rail (sources, player number, SLIP), the tab row, and
//! the lower bands' panels.

use super::{Target, Ui, View};
use crate::kit::Rect;
use crate::render::scene::Rgba;

/// The rail's panel and the rule beside it.
pub(crate) fn rail_panel(ui: &mut Ui) {
    let t = ui.t;
    ui.scene.fill(0.0, 0.0, 101.0, 600.0, t.color.background);
    ui.scene.fill(100.0, 0.0, 1.0, 600.0, t.color.rule);
}

/// The status band's and bottom strip's panels, with their right column's dividers.
pub(crate) fn lower(ui: &mut Ui) {
    let t = ui.t;
    ui.section(Rect::new(106.0, 395.0, 915.0, 99.0));
    ui.section(Rect::new(106.0, 498.0, 915.0, 99.0));
    ui.scene.fill(822.0, 400.0, 1.0, 89.0, t.color.section_edge);
    ui.scene.fill(822.0, 502.0, 1.0, 90.0, t.color.section_edge);
}

/// The rail with the source `lit` (LIBRARY or USB) on.
pub(crate) fn rail(ui: &mut Ui, v: &View, lit: Target) {
    ui.button(
        Target::Library,
        Rect::new(6.0, 16.0, 88.0, 117.0),
        "LIBRARY",
        None,
        lit == Target::Library,
    );
    ui.button(
        Target::Link,
        Rect::new(6.0, 138.0, 88.0, 124.0),
        "LINK",
        None,
        false,
    );
    ui.button(
        Target::Usb,
        Rect::new(6.0, 267.0, 88.0, 117.0),
        "USB",
        None,
        lit == Target::Usb,
    );
    rail_bottom(ui, v);
}

/// The rail's player number and SLIP, the same on every screen.
pub(crate) fn rail_bottom(ui: &mut Ui, v: &View) {
    let t = ui.t;
    ui.scene.rounded(
        6.0,
        410.0,
        88.0,
        83.0,
        t.shape.button_radius,
        t.color.button,
        t.shape.border,
        t.color.button_edge,
    );
    ui.text("PLAYER", 50.0, 418.0, t.text.small.center());
    ui.text("1", 50.0, 438.0, t.text.player.center());
    ui.button(
        Target::Slip,
        Rect::new(6.0, 505.0, 88.0, 76.0),
        "SLIP",
        None,
        v.deck.slip_enabled,
    );
    if v.deck.slip_active {
        // Slipping: the shadow is running; amber edge like the reference's flashing button.
        ui.scene.rounded(
            6.0,
            505.0,
            88.0,
            76.0,
            t.shape.button_radius,
            Rgba::TRANSPARENT,
            t.shape.border_active,
            t.color.cue,
        );
    }
}

/// The tab row, with `active` (a tab's target) lit.
pub(crate) fn tabs(ui: &mut Ui, active: Option<Target>) {
    let y = 7.0;
    let h = 54.0;
    ui.button(
        Target::Browse,
        Rect::new(113.0, y, 171.0, h),
        "BROWSE",
        Some("– SEARCH"),
        active == Some(Target::Browse),
    );
    ui.button(
        Target::TagList,
        Rect::new(289.0, y, 171.0, h),
        "TAG LIST",
        None,
        active == Some(Target::TagList),
    );
    ui.button(
        Target::Info,
        Rect::new(464.0, y, 171.0, h),
        "INFO",
        Some("– LINK INFO"),
        active == Some(Target::Info),
    );
    ui.button(
        Target::Menu,
        Rect::new(640.0, y, 171.0, h),
        "MENU",
        Some("– UTILITY"),
        active == Some(Target::Menu),
    );
    ui.button(
        Target::Perform,
        Rect::new(841.0, y, 175.0, h),
        "PERFORM",
        None,
        active == Some(Target::Perform),
    );
}
