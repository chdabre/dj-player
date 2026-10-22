//! The frame every screen shares: the rail (sources, player number, SLIP), the tab row, and
//! the lower bands' panels. A screen draws it with [`frame`] and its own [`Chrome`] options,
//! and lays itself out against the geometry here.

use browser::Source;
use player_api::DeckCommand;

use super::{Cx, ScreenKind, Screens, Target, Ui, View};
use crate::app::effects::Effect;
use crate::kit::Rect;
use crate::render::scene::Rgba;

/// The sections' left edge and width: everything right of the rail.
pub const SECTION_X: f32 = 106.0;
pub const SECTION_W: f32 = 915.0;
/// The content column inside the sections: its left edge, and its width up to the right
/// column's divider.
pub const CONTENT_X: f32 = 113.0;
pub const CONTENT_W: f32 = 703.0;
/// The divider before the right column, and the column itself.
pub const DIVIDER_X: f32 = 822.0;
pub const COLUMN_X: f32 = 828.0;
pub const COLUMN_W: f32 = 188.0;

/// The tab row's section, and the body under it.
pub const TOP: Rect = Rect::new(SECTION_X, 2.0, SECTION_W, 64.0);
pub const BODY: Rect = Rect::new(SECTION_X, 66.0, SECTION_W, 325.0);
/// The performance screen's taller top row, and its pads section.
pub const PERFORM_TOP: Rect = Rect::new(SECTION_X, 2.0, SECTION_W, 85.0);
pub const PERFORM_BODY: Rect = Rect::new(SECTION_X, 90.0, SECTION_W, 301.0);
/// The status band and the bottom strip.
pub const STATUS: Rect = Rect::new(SECTION_X, 395.0, SECTION_W, 99.0);
pub const STRIP: Rect = Rect::new(SECTION_X, 498.0, SECTION_W, 99.0);
/// Both lower bands (where the browse keyboard goes).
pub const LOWER: Rect = Rect::new(SECTION_X, STATUS.y, SECTION_W, STRIP.y + STRIP.h - STATUS.y);

/// What a screen's frame has.
#[derive(Debug, Clone, Copy)]
pub struct Chrome {
    pub top: Top,
    /// The status band's and bottom strip's panels (the browse keyboard covers them).
    pub lower: bool,
}

/// The frame's top.
#[derive(Debug, Clone, Copy)]
pub enum Top {
    /// The source rail with `lit` on, and the tab row over the body with `tab` lit.
    Tabs { lit: Target, tab: Option<Target> },
    /// The performance screen: a taller top row and the pads section, with dividers for the
    /// right column. Its rail (the pad modes) is its own; the frame draws the player number
    /// and SLIP under it.
    Perform,
}

/// Draw the frame: the rail, the sections, the lower bands and the tab row.
pub(crate) fn frame(ui: &mut Ui, v: &View, c: Chrome) {
    let t = ui.t;
    rail_panel(ui);
    let (top, body) = match c.top {
        Top::Tabs { .. } => (TOP, BODY),
        Top::Perform => (PERFORM_TOP, PERFORM_BODY),
    };
    ui.section(top);
    ui.section(body);
    if c.lower {
        lower(ui);
    }
    match c.top {
        Top::Tabs { lit, tab } => {
            rail(ui, v, lit);
            tabs(ui, tab);
        }
        Top::Perform => {
            for (y, h) in [(7.0, 75.0), (95.0, 292.0)] {
                ui.scene.fill(DIVIDER_X, y, 1.0, h, t.color.section_edge);
            }
            rail_bottom(ui, v);
        }
    }
}

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
fn rail_panel(ui: &mut Ui) {
    let t = ui.t;
    ui.scene.fill(0.0, 0.0, 101.0, 600.0, t.color.background);
    ui.scene.fill(100.0, 0.0, 1.0, 600.0, t.color.rule);
}

/// The status band's and bottom strip's panels, with their right column's dividers.
fn lower(ui: &mut Ui) {
    let t = ui.t;
    ui.section(STATUS);
    ui.section(STRIP);
    ui.scene
        .fill(DIVIDER_X, 400.0, 1.0, 89.0, t.color.section_edge);
    ui.scene
        .fill(DIVIDER_X, 502.0, 1.0, 90.0, t.color.section_edge);
}

/// The rail with the source `lit` (LIBRARY or USB) on.
fn rail(ui: &mut Ui, v: &View, lit: Target) {
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
fn rail_bottom(ui: &mut Ui, v: &View) {
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
fn tabs(ui: &mut Ui, active: Option<Target>) {
    let y = 7.0;
    let h = 54.0;
    ui.button(
        Target::Chrome(ChromeTarget::Browse),
        Rect::new(CONTENT_X, y, 171.0, h),
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
