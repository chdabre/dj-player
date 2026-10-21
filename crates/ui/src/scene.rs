//! What one frame draws, in layout pixels (the 1024×600 design canvas): rounded rectangles,
//! then text on top. Colours are sRGB, as in the design tokens.

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
}

#[derive(Debug, Default)]
pub struct Scene {
    /// What the frame is cleared to before drawing.
    pub background: Rgba,
    pub rects: Vec<Rect>,
    pub texts: Vec<Text>,
}

impl Scene {
    pub fn clear(&mut self) {
        self.rects.clear();
        self.texts.clear();
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
        });
    }

    pub fn text(&mut self, t: Text) {
        self.texts.push(t);
    }
}
