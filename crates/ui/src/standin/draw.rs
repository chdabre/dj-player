//! The shell's drawing helpers: its own buttons, key caps, LEDs and labels, on `kit`'s scene
//! and touch targets, in the shell's colours and type sizes.

use super::{Held, Part, is_down};
use crate::kit::{Rect, Ui};
use crate::render::scene::{Align, Rgba, Weight};
use crate::theme::{ShellColors, TextStyle};

/// Drawing for the shell: the toolkit (scene and touch targets), its colours, and what the
/// pointer holds (drawn pressed).
pub(super) struct Draw<'s> {
    pub ui: Ui<'s, Part>,
    pub c: &'s ShellColors,
    pub held: Option<Held>,
}

/// The shell's label text: its own sizes, semibold from 15 px up.
fn style(size: f32, color: Rgba, align: Align) -> TextStyle {
    TextStyle {
        size,
        weight: if size >= 15.0 {
            Weight::SemiBold
        } else {
            Weight::Regular
        },
        color,
        line_height: 1.2,
        align,
        max_width: None,
    }
}

#[allow(clippy::too_many_arguments)]
impl Draw<'_> {
    /// `part` is held down.
    pub fn down(&self, part: Part) -> bool {
        is_down(self.held, part)
    }

    pub fn label(&mut self, s: &str, x: f32, y: f32, color: Rgba, size: f32, align: Align) {
        self.ui.text(s, x, y, style(size, color, align));
    }

    /// A key cap showing the keyboard shortcut.
    pub fn key(&mut self, s: &str, x: f32, y: f32) {
        let w = 12.0 + 6.5 * s.chars().count() as f32;
        self.ui
            .scene
            .rounded(x, y, w, 19.0, 4.0, self.c.key_fill, 1.0, self.c.edge_light);
        self.label(
            s,
            x + w * 0.5,
            y + 1.5,
            self.c.key_text,
            12.0,
            Align::Center,
        );
    }

    pub fn led(&mut self, x: f32, y: f32, color: Rgba) {
        self.ui
            .scene
            .rounded(x, y, 9.0, 9.0, 4.5, color, 0.0, color);
    }

    /// A rule between groups (the shell's own: thinner panel edge than the device's rules).
    pub fn rule(&mut self, x: f32, y: f32, w: f32) {
        self.ui.scene.fill(x, y, w, 1.0, self.c.panel_edge);
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Rgba, border: f32, edge: Rgba) {
        self.ui
            .scene
            .rounded(cx - r, cy - r, 2.0 * r, 2.0 * r, r, fill, border, edge);
    }

    /// A round button `d` across, with its top-left at (`x`, `y`).
    pub fn round(&mut self, part: Part, x: f32, y: f32, d: f32) {
        let fill = if self.down(part) {
            self.c.button_down
        } else {
            self.c.button_top
        };
        self.ui
            .scene
            .rounded(x, y, d, d, d * 0.5, fill, 1.0, self.c.edge);
        self.ui.hit(part, Rect::new(x, y, d, d));
    }

    pub fn round_label(&mut self, part: Part, x: f32, y: f32, d: f32, s: &str) {
        self.round(part, x, y, d);
        self.label(
            s,
            x + d * 0.5,
            y + d * 0.5 - 8.0,
            self.c.text,
            13.0,
            Align::Center,
        );
    }

    /// A loop button: round, amber, `fill` unless held.
    pub fn amber(&mut self, part: Part, x: f32, y: f32, d: f32, s: &str, fill: Rgba) {
        let fill = if self.down(part) {
            self.c.amber_text
        } else {
            fill
        };
        self.ui
            .scene
            .rounded(x, y, d, d, d * 0.5, fill, 1.0, self.c.amber_edge);
        self.label(
            s,
            x + d * 0.5,
            y + d * 0.5 - 8.0,
            self.c.on_amber,
            13.0,
            Align::Center,
        );
        self.ui.hit(part, Rect::new(x, y, d, d));
    }

    /// CUE or PLAY: a big round button with a coloured ring.
    pub fn big(&mut self, part: Part, x: f32, y: f32, s: &str, ring: Rgba, text: Rgba) {
        let d = 104.0;
        let fill = if self.down(part) {
            self.c.button_down
        } else {
            self.c.button_top
        };
        self.ui.scene.rounded(x, y, d, d, d * 0.5, fill, 4.0, ring);
        self.label(
            s,
            x + d * 0.5,
            y + d * 0.5 - 14.0,
            text,
            23.0,
            Align::Center,
        );
        self.ui.hit(part, Rect::new(x, y, d, d));
    }

    pub fn rect_button(
        &mut self,
        part: Part,
        r: Rect,
        s: &str,
        fill: Rgba,
        edge: Rgba,
        text: Rgba,
    ) {
        let Rect { x, y, w, h } = r;
        let fill = if self.down(part) {
            self.c.button_down
        } else {
            fill
        };
        self.ui.scene.rounded(x, y, w, h, 6.0, fill, 1.0, edge);
        self.label(
            s,
            x + w * 0.5,
            y + h * 0.5 - 10.0,
            text,
            16.0,
            Align::Center,
        );
        self.ui.hit(part, r);
    }
}
