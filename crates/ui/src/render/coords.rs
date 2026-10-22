//! Where the device screen and the shell are in the window: the design → window transform,
//! and window pixels back to design pixels for touches.

use super::{DESIGN_H, DESIGN_W, Gfx};

/// A design of `design` pixels → physical pixels on a `w`×`h` target: (scale, offset x,
/// offset y). The design is scaled uniformly and centred.
pub(super) fn transform(w: u32, h: u32, (dw, dh): (f32, f32)) -> (f32, f32, f32) {
    let (w, h) = (w as f32, h as f32);
    let s = (w / dw).min(h / dh);
    (s, (w - dw * s) * 0.5, (h - dh * s) * 0.5)
}

impl Gfx {
    /// Physical pixel position in the window → the device screen's design pixels, as the
    /// last frame placed it. Outside the device screen (on a shell) it is out of range.
    pub fn to_design(&self, x: f64, y: f64) -> (f32, f32) {
        let (s, ox, oy) = self.device_xform;
        ((x as f32 - ox) / s, (y as f32 - oy) / s)
    }

    /// True if a window pixel falls on the device screen.
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    pub fn on_device(&self, x: f64, y: f64) -> bool {
        let (dx, dy) = self.to_design(x, y);
        (0.0..DESIGN_W).contains(&dx) && (0.0..DESIGN_H).contains(&dy)
    }

    /// Window pixel → the shell's design pixels, if a shell was drawn.
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    pub fn to_shell(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        self.shell_xform
            .map(|(s, ox, oy)| ((x as f32 - ox) / s, (y as f32 - oy) / s))
    }
}
