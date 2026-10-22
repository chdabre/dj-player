//! What one frame draws, in layout pixels (the 1024×600 design canvas), on two planes (base,
//! overlay): per plane, rounded rectangles and waveforms in paint order, then pictures, then
//! text. Colours are sRGB, as in the design tokens.

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
pub struct Quad {
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

/// A plane of the scene. The renderer draws the base plane, then the overlay plane over it,
/// text included, so a modal covers what is under it whatever was drawn first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Layer {
    #[default]
    Base,
    Overlay,
}

pub const LAYERS: usize = 2;

/// What one plane draws, in paint order: rectangles with the waveforms between them, then
/// pictures, then text.
#[derive(Debug, Default)]
pub struct Plane {
    pub rects: Vec<Quad>,
    /// Waveforms, each drawn after the first `.0` rectangles and before the rest, so a
    /// waveform sits on its panel and grid and under its overlays. Base plane only.
    pub waves: Vec<(usize, WaveDraw)>,
    pub texts: Vec<Text>,
    /// Pictures, drawn over every rectangle and waveform and under the text.
    pub images: Vec<Image>,
}

impl Plane {
    fn clear(&mut self) {
        self.rects.clear();
        self.waves.clear();
        self.texts.clear();
        self.images.clear();
    }
}

#[derive(Debug, Default)]
pub struct Scene {
    /// What the frame is cleared to before drawing.
    pub background: Rgba,
    pub planes: [Plane; LAYERS],
    /// The plane drawing goes to.
    layer: Layer,
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
        for p in &mut self.planes {
            p.clear();
        }
        self.layer = Layer::Base;
    }

    /// Draw on `layer` from now on; returns the layer drawn on before.
    pub fn set_layer(&mut self, layer: Layer) -> Layer {
        std::mem::replace(&mut self.layer, layer)
    }

    fn plane(&mut self) -> &mut Plane {
        &mut self.planes[self.layer as usize]
    }

    /// Every rectangle, base plane first.
    #[cfg(test)]
    pub fn rects(&self) -> impl Iterator<Item = &Quad> {
        self.planes.iter().flat_map(|p| &p.rects)
    }

    /// Every text, base plane first.
    #[cfg(test)]
    pub fn texts(&self) -> impl Iterator<Item = &Text> {
        self.planes.iter().flat_map(|p| &p.texts)
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        self.plane().rects.push(Quad {
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
        self.plane().rects.push(Quad {
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
        self.plane().rects.push(Quad {
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

    /// Draw `wave` on top of everything so far (on the base plane).
    pub fn waveform(&mut self, wave: WaveDraw) {
        debug_assert_eq!(
            self.layer,
            Layer::Base,
            "waveforms are drawn on the base plane"
        );
        let p = &mut self.planes[Layer::Base as usize];
        p.waves.push((p.rects.len(), wave));
    }

    pub fn text(&mut self, t: Text) {
        self.plane().texts.push(t);
    }

    pub fn image(&mut self, image: Image) {
        self.plane().images.push(image);
    }
}
