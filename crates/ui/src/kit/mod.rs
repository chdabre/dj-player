//! The widget toolkit: what every screen draws with. It knows the theme, the scene and touch
//! targets, and nothing of decks, the browser or the library, so screens, overlays and the
//! stand-in can share it.
//!
//! Drawing is immediate: a screen calls widgets each frame, and each widget adds its shapes to
//! the [`Scene`] and its touch target to [`Ui::hits`]. `T` is the caller's target type.

mod rect;

pub use rect::Rect;

use crate::scene::{Rgba, Scene, Text};
use crate::theme::{TextStyle, Theme};

/// A touch target and where it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitBox<T> {
    pub target: T,
    pub rect: Rect,
}

impl<T> HitBox<T> {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }
}

/// One frame's drawing: the theme to draw with, the scene to fill, the touch targets so far,
/// and the target the finger is on (drawn pressed).
pub struct Ui<'s, T> {
    pub t: &'s Theme,
    pub scene: &'s mut Scene,
    pub hits: Vec<HitBox<T>>,
    pub pressed: Option<T>,
}

impl<T: Copy + PartialEq> Ui<'_, T> {
    /// A line of text anchored at (`x`, `y`): the top of its line box, and the edge (or
    /// centre) its style's alignment says.
    pub fn text(&mut self, s: impl Into<String>, x: f32, y: f32, style: TextStyle) {
        self.scene.text(Text {
            text: s.into(),
            x,
            y,
            size: style.size,
            line_height: style.line_height,
            weight: style.weight,
            color: style.color,
            align: style.align,
            max_width: style.max_width,
        });
    }

    /// `style`'s line centred vertically in a box from `y`, `h` high.
    pub fn text_in(&mut self, s: impl Into<String>, x: f32, y: f32, h: f32, style: TextStyle) {
        self.text(s, x, y + (h - style.line_box()) * 0.5, style);
    }

    /// A section: the outlined panel the screen is divided into.
    pub fn section(&mut self, r: Rect) {
        let t = self.t;
        self.scene.rounded(
            r.x,
            r.y,
            r.w,
            r.h,
            t.shape.section_radius,
            t.color.background,
            t.shape.border,
            t.color.section_edge,
        );
    }

    /// `target` can be touched in `r`.
    pub fn hit(&mut self, target: T, r: Rect) {
        self.hits.push(HitBox { target, rect: r });
    }

    /// `.b`: a touch button with an optional second line.
    pub fn button(&mut self, target: T, r: Rect, label: &str, sub: Option<&str>, on: bool) {
        self.button_styled(target, r, label, sub, on, self.t.text.button);
    }

    /// [`Ui::button`] with its label in `style`.
    pub fn button_styled(
        &mut self,
        target: T,
        r: Rect,
        label: &str,
        sub: Option<&str>,
        on: bool,
        style: TextStyle,
    ) {
        let t = self.t;
        let pressed = self.pressed == Some(target);
        let (fill, edge) = match (on, pressed) {
            (_, true) => (t.color.button_pressed, t.color.button_pressed_edge),
            (true, false) => (t.color.button_on, t.color.button_on_edge),
            (false, false) => (t.color.button, t.color.button_edge),
        };
        self.scene.rounded(
            r.x,
            r.y,
            r.w,
            r.h,
            t.shape.button_radius,
            fill,
            t.shape.border,
            edge,
        );
        let sub_style = t.text.small.center();
        let block = style.line_box() + sub.map_or(0.0, |_| sub_style.line_box());
        let top = r.y + (r.h - block) * 0.5;
        self.text(label, r.center_x(), top, style.center());
        if let Some(sub) = sub {
            self.text(sub, r.center_x(), top + style.line_box(), sub_style);
        }
        self.hit(target, r);
    }

    /// `.f`: an outlined chip (an indicator; [`Ui::hit`] makes it touchable).
    pub fn chip(&mut self, r: Rect, label: &str, style: TextStyle, edge: Rgba, fill: Rgba) {
        let t = self.t;
        self.scene.rounded(
            r.x,
            r.y,
            r.w,
            r.h,
            t.shape.chip_radius,
            fill,
            t.shape.border,
            edge,
        );
        self.text_in(label, r.center_x(), r.y, r.h, style.semibold().center());
    }

    /// A label centred in a horizontal rule (`CUE/LOOP`, `CALL`).
    pub fn rule_label(&mut self, label: &str, x: f32, y: f32, w: f32) {
        let t = self.t;
        // Barlow Condensed caps at the label size are ~7/15 em wide; close enough to leave a gap.
        let text_w = label.len() as f32 * t.font.label * (7.0 / 15.0) + 12.0;
        let side = (w - text_w) * 0.5;
        let mid = y + 8.0;
        self.scene.fill(x, mid, side, 1.0, t.color.rule);
        self.scene.fill(x + w - side, mid, side, 1.0, t.color.rule);
        self.text(label, x + w * 0.5, y - 1.0, t.text.label.center());
    }
}
