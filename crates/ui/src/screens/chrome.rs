//! The frame every screen shares: the rail (sources, player number, SLIP), the tab row, and
//! the lower bands' panels.

use engine_core::DeckCommand;
use engined::browse::Source;

use super::{Cx, ScreenKind, Screens, Target, Ui, View};
use crate::app::effects::Effect;
use crate::kit::Rect;
use crate::render::scene::Rgba;

/// The rail's and the tab row's touch targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeTarget {
    Library,
    Link,
    Usb,
    Slip,
    Browse,
    TagList,
    Info,
    Menu,
    Perform,
}

/// A touch on the rail or a tab.
pub(crate) fn tap(s: &mut Screens, t: ChromeTarget, cx: &mut Cx) {
    match t {
        ChromeTarget::Perform => s.toggle_perform(),
        ChromeTarget::Browse => s.toggle_browse(cx),
        ChromeTarget::Library => s.browse_source(Source::Library, cx),
        ChromeTarget::Usb => s.browse_source(Source::Folders, cx),
        ChromeTarget::TagList => {
            if cx.browser.is_some() {
                s.browse.editing.modal = None;
            }
            s.browse_source(Source::TagList, cx);
        }
        ChromeTarget::Menu if s.screen == ScreenKind::Browse => s.browse.open_menu(cx),
        ChromeTarget::Info => s.toggle_info(),
        ChromeTarget::Slip => cx.effects.push(Effect::Deck(DeckCommand::ToggleSlip)),
        ChromeTarget::Menu | ChromeTarget::Link => {}
    }
}

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
        Target::Chrome(ChromeTarget::Library),
        Rect::new(6.0, 16.0, 88.0, 117.0),
        "LIBRARY",
        None,
        lit == Target::Chrome(ChromeTarget::Library),
    );
    ui.button(
        Target::Chrome(ChromeTarget::Link),
        Rect::new(6.0, 138.0, 88.0, 124.0),
        "LINK",
        None,
        false,
    );
    ui.button(
        Target::Chrome(ChromeTarget::Usb),
        Rect::new(6.0, 267.0, 88.0, 117.0),
        "USB",
        None,
        lit == Target::Chrome(ChromeTarget::Usb),
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
        Target::Chrome(ChromeTarget::Slip),
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
        Target::Chrome(ChromeTarget::Browse),
        Rect::new(113.0, y, 171.0, h),
        "BROWSE",
        Some("– SEARCH"),
        active == Some(Target::Chrome(ChromeTarget::Browse)),
    );
    ui.button(
        Target::Chrome(ChromeTarget::TagList),
        Rect::new(289.0, y, 171.0, h),
        "TAG LIST",
        None,
        active == Some(Target::Chrome(ChromeTarget::TagList)),
    );
    ui.button(
        Target::Chrome(ChromeTarget::Info),
        Rect::new(464.0, y, 171.0, h),
        "INFO",
        Some("– LINK INFO"),
        active == Some(Target::Chrome(ChromeTarget::Info)),
    );
    ui.button(
        Target::Chrome(ChromeTarget::Menu),
        Rect::new(640.0, y, 171.0, h),
        "MENU",
        Some("– UTILITY"),
        active == Some(Target::Chrome(ChromeTarget::Menu)),
    );
    ui.button(
        Target::Chrome(ChromeTarget::Perform),
        Rect::new(841.0, y, 175.0, h),
        "PERFORM",
        None,
        active == Some(Target::Chrome(ChromeTarget::Perform)),
    );
}
