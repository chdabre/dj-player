//! The widget toolkit: what every screen draws with. It knows the theme, the scene and touch
//! targets, and nothing of decks, the browser or the library, so screens, overlays and the
//! stand-in can share it.
//!
//! Drawing is immediate: a screen calls widgets each frame, and each widget adds its shapes to
//! the [`Scene`] and its touch target to [`Ui::hits`]. `T` is the caller's target type.
//!
//! **Layers.** What is drawn in [`Ui::on`]`(Layer::Overlay, …)` covers the base layer, text
//! included, and its touch targets come first. A [`Ui::scrim`] (or [`Ui::block`]) stops touches
//! from reaching the layers under it, so a modal needs no help from what it covers.

mod rect;

pub use rect::Rect;

use std::ops::{Deref, DerefMut};

use crate::render::scene::{Layer, Rgba, Scene, Text};
use crate::theme::{TextStyle, Theme};

/// A touch target, where it is, and the layer it was drawn on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitBox<T> {
    pub target: T,
    pub rect: Rect,
    pub layer: Layer,
}

impl<T> HitBox<T> {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }
}

/// One frame's touch targets, in drawing order, and the areas that keep touches from the
/// layers under them.
#[derive(Debug, Clone)]
pub struct Hits<T> {
    boxes: Vec<HitBox<T>>,
    blocks: Vec<(Rect, Layer)>,
}

impl<T> Default for Hits<T> {
    fn default() -> Self {
        Self {
            boxes: Vec::new(),
            blocks: Vec::new(),
        }
    }
}

impl<T: Copy> Hits<T> {
    /// What a touch at (`x`, `y`) lands on: the target on the highest layer there that no
    /// block above it covers (the first drawn, among one layer's).
    pub fn find(&self, x: f32, y: f32) -> Option<HitBox<T>> {
        self.boxes
            .iter()
            .filter(|h| h.contains(x, y) && !self.blocked(h))
            .fold(None, |best: Option<&HitBox<T>>, h| match best {
                Some(b) if b.layer >= h.layer => Some(b),
                _ => Some(h),
            })
            .copied()
    }

    /// `h` sits under a block of a higher layer (everywhere a touch would reach it, it is
    /// covered: blocks are at least as big as what they cover, in practice).
    pub fn blocked(&self, h: &HitBox<T>) -> bool {
        self.blocks
            .iter()
            .any(|(r, layer)| *layer > h.layer && r.overlaps(h.rect))
    }
}

impl<T> Deref for Hits<T> {
    type Target = Vec<HitBox<T>>;
    fn deref(&self) -> &Self::Target {
        &self.boxes
    }
}

impl<T> DerefMut for Hits<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.boxes
    }
}

/// One frame's drawing: the theme to draw with, the scene to fill, the touch targets so far,
/// and the target the finger is on (drawn pressed).
pub struct Ui<'s, T> {
    pub t: &'s Theme,
    pub scene: &'s mut Scene,
    pub hits: Hits<T>,
    pub pressed: Option<T>,
    layer: Layer,
}

impl<'s, T> Ui<'s, T> {
    pub fn new(t: &'s Theme, scene: &'s mut Scene, pressed: Option<T>) -> Self {
        Self {
            t,
            scene,
            hits: Hits::default(),
            pressed,
            layer: Layer::Base,
        }
    }

    /// The touch targets drawn.
    pub fn into_hits(self) -> Hits<T> {
        self.hits
    }
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
        let layer = self.layer;
        self.hits.push(HitBox {
            target,
            rect: r,
            layer,
        });
    }

    /// Draw `f` on `layer`: over the base layer, text included, and touched first.
    pub fn on(&mut self, layer: Layer, f: impl FnOnce(&mut Self)) {
        let (scene_was, was) = (self.scene.set_layer(layer), self.layer);
        self.layer = layer;
        f(self);
        self.layer = was;
        self.scene.set_layer(scene_was);
    }

    /// Touches in `r` stop at this layer: nothing under it there can be touched.
    pub fn block(&mut self, r: Rect) {
        self.hits.blocks.push((r, self.layer));
    }

    /// Dim `r` with the theme's overlay colour and [`Ui::block`] it: what is under a modal.
    pub fn scrim(&mut self, r: Rect) {
        let t = self.t;
        self.scene.fill(r.x, r.y, r.w, r.h, t.color.overlay);
        self.block(r);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touches_find_the_top_layer_and_scrims_block_what_is_under_them() {
        let theme = Theme::default();
        let mut scene = Scene::default();
        let mut ui = Ui::new(&theme, &mut scene, None);
        ui.hit('a', Rect::new(0.0, 0.0, 100.0, 100.0));
        ui.hit('b', Rect::new(0.0, 200.0, 100.0, 100.0));
        ui.on(Layer::Overlay, |ui| {
            ui.scrim(Rect::new(0.0, 0.0, 100.0, 150.0));
            ui.hit('m', Rect::new(10.0, 10.0, 20.0, 20.0));
        });
        let hits = ui.into_hits();
        let at = |x, y| hits.find(x, y).map(|h| h.target);
        assert_eq!(at(15.0, 15.0), Some('m'), "the overlay's target first");
        assert_eq!(at(80.0, 80.0), None, "under the scrim, nothing");
        assert_eq!(
            at(50.0, 250.0),
            Some('b'),
            "beside the scrim, the base layer"
        );
        assert!(hits.blocked(&hits[0]) && !hits.blocked(&hits[1]));
        // The scrim and the overlay's drawing are on the overlay plane.
        assert_eq!(scene.planes[Layer::Overlay as usize].rects.len(), 1);
        assert!(scene.planes[Layer::Base as usize].rects.is_empty());
    }
}
