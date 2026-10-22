//! A rectangle in design pixels (the 1024×600 canvas), and the splits the layouts make.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center_x(self) -> f32 {
        self.x + self.w * 0.5
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The two share some area (touching edges don't count).
    pub fn overlaps(self, o: Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// Smaller by `d` on every side.
    pub fn inset(self, d: f32) -> Self {
        Self::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    /// `n` equal columns with `gap` between them, left to right.
    pub fn columns(self, n: usize, gap: f32) -> impl Iterator<Item = Rect> {
        let w = (self.w - gap * n.saturating_sub(1) as f32) / n.max(1) as f32;
        (0..n).map(move |i| Self::new(self.x + i as f32 * (w + gap), self.y, w, self.h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fill_the_width_with_gaps() {
        let cols: Vec<Rect> = Rect::new(10.0, 0.0, 100.0, 20.0).columns(4, 4.0).collect();
        assert_eq!(cols.len(), 4);
        assert_eq!(cols[0].x, 10.0);
        assert_eq!(cols[0].w, 22.0);
        assert_eq!(cols[3].right(), 110.0);
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 9.99));
        assert!(!r.contains(10.0, 5.0));
    }
}
