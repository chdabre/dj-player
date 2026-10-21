//! What one frame draws, in layout pixels (the 1024×600 design canvas): rounded rectangles
//! and waveforms in paint order, then text on top. Colours are sRGB, as in the design tokens.

use crate::waves::WaveDraw;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    /// `0xRRGGBB`, opaque.
    pub const fn hex(rgb: u32) -> Self {
        Self([
            ((rgb >> 16) & 0xff) as f32 / 255.0,
            ((rgb >> 8) & 0xff) as f32 / 255.0,
            (rgb & 0xff) as f32 / 255.0,
            1.0,
        ])
    }

    pub const fn alpha(self, a: f32) -> Self {
        let [r, g, b, _] = self.0;
        Self([r, g, b, a])
    }

    pub const TRANSPARENT: Self = Self([0.0; 4]);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    /// Border width; 0 = none. The border is drawn inside the rectangle.
    pub border: f32,
    pub fill: Rgba,
    pub stroke: Rgba,
    /// A vertical gradient from `fill` to this colour, if any.
    pub gradient: Option<(Rgba, Gradient)>,
}

/// How a gradient runs: from the top down, or from both edges to the centre line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gradient {
    Down,
    Mirrored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    SemiBold,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    /// Anchor: the left edge, centre or right edge, per `align`.
    pub x: f32,
    /// Top of the line box.
    pub y: f32,
    pub size: f32,
    /// Line box height as a multiple of `size` (CSS `line-height`).
    pub line_height: f32,
    pub weight: Weight,
    pub color: Rgba,
    pub align: Align,
    /// Longest the line may be (design pixels); longer text ends in an ellipsis.
    pub max_width: Option<f32>,
}

#[derive(Debug, Default)]
pub struct Scene {
    /// What the frame is cleared to before drawing.
    pub background: Rgba,
    pub rects: Vec<Rect>,
    /// Waveforms, each drawn after the first `.0` rectangles and before the rest, so a
    /// waveform sits on its panel and grid and under its overlays.
    pub waves: Vec<(usize, WaveDraw)>,
    pub texts: Vec<Text>,
    /// Pictures, drawn over every rectangle and waveform and under the text.
    pub images: Vec<Image>,
}

/// A picture (artwork) in a rounded rectangle, design pixels.
#[derive(Debug, Clone)]
pub struct Image {
    pub picture: std::sync::Arc<engined::browse::Thumb>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
}

impl Scene {
    pub fn clear(&mut self) {
        self.rects.clear();
        self.waves.clear();
        self.texts.clear();
        self.images.clear();
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        self.rects.push(Rect {
            x,
            y,
            w,
            h,
            radius: 0.0,
            border: 0.0,
            fill: color,
            stroke: Rgba::TRANSPARENT,
            gradient: None,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rounded(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill: Rgba,
        border: f32,
        stroke: Rgba,
    ) {
        self.rects.push(Rect {
            x,
            y,
            w,
            h,
            radius,
            border,
            fill,
            stroke,
            gradient: None,
        });
    }

    /// A rectangle filled with a vertical gradient from `from` (top, or edges) to `to`
    /// (bottom, or centre line).
    #[allow(clippy::too_many_arguments)]
    pub fn gradient(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        from: Rgba,
        to: Rgba,
        kind: Gradient,
    ) {
        self.rects.push(Rect {
            x,
            y,
            w,
            h,
            radius,
            border: 0.0,
            fill: from,
            stroke: Rgba::TRANSPARENT,
            gradient: Some((to, kind)),
        });
    }

    /// Draw `wave` on top of everything so far.
    pub fn waveform(&mut self, wave: WaveDraw) {
        self.waves.push((self.rects.len(), wave));
    }

    pub fn text(&mut self, t: Text) {
        self.texts.push(t);
    }
}
