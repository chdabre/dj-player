//! An RGB canvas and the 3×5 pixel font its labels use.

use crate::display::Rgb;

/// An RGB image, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgb>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: Rgb) -> Self {
        Canvas {
            width,
            height,
            pixels: vec![fill; width * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Rgb {
        self.pixels[y * self.width + x]
    }

    /// Blend `c` at opacity `alpha` into one pixel; outside the canvas is ignored.
    pub fn put(&mut self, x: i64, y: i64, c: Rgb, alpha: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let p = &mut self.pixels[y as usize * self.width + x as usize];
        *p = if alpha >= 1.0 { c } else { c.over(*p, alpha) };
    }

    /// Fill `[x0, x1) × [y0, y1)`.
    pub fn fill(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgb, alpha: f32) {
        for y in y0.max(0)..y1.min(self.height as i64) {
            for x in x0.max(0)..x1.min(self.width as i64) {
                self.put(x, y, c, alpha);
            }
        }
    }

    /// A vertical line over `[y0, y1)`; `dash` = (on, off) pixels.
    pub fn vline(&mut self, x: i64, y0: i64, y1: i64, c: Rgb, dash: Option<(i64, i64)>) {
        for y in y0..y1 {
            if dash.is_none_or(|(on, off)| (y - y0) % (on + off) < on) {
                self.put(x, y, c, 1.0);
            }
        }
    }

    /// A filled triangle pointing down (`down`) or up, apex at `(x, y)`, `size` px tall.
    pub fn triangle(&mut self, x: i64, y: i64, size: i64, down: bool, c: Rgb) {
        for i in 0..size {
            let row = if down { y - i } else { y + i };
            self.fill(x - i, row, x + i + 1, row + 1, c, 1.0);
        }
    }

    /// Text in the 3×5 pixel font, top-left at `(x, y)`, each font pixel `scale` px.
    pub fn text(&mut self, x: i64, y: i64, s: &str, scale: i64, c: Rgb) {
        for (i, ch) in s.chars().enumerate() {
            let rows = glyph(ch);
            let gx = x + i as i64 * 4 * scale;
            for (r, bits) in rows.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) != 0 {
                        let px = gx + col * scale;
                        let py = y + r as i64 * scale;
                        self.fill(px, py, px + scale, py + scale, c, 1.0);
                    }
                }
            }
        }
    }

    /// Copy `other` with its top-left corner at `(x, y)`.
    pub fn blit(&mut self, other: &Canvas, x: usize, y: usize) {
        for row in 0..other.height.min(self.height.saturating_sub(y)) {
            let w = other.width.min(self.width.saturating_sub(x));
            let dst = (y + row) * self.width + x;
            self.pixels[dst..dst + w]
                .copy_from_slice(&other.pixels[row * other.width..row * other.width + w]);
        }
    }

    /// Packed 8-bit RGB, for PNG encoders.
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| [p.0, p.1, p.2]).collect()
    }
}

/// 3×5 glyphs for digits and the hot cue letters A–H; anything else is blank.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        _ => [0; 5],
    }
}
