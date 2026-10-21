//! Display modes: from one entry's band values to what a column draws.
//!
//! Every mode draws from the same stored data. A column is a list of mirrored segments,
//! measured from the centre line outwards (0 = centre, 1 = the view's half-height), drawn
//! in order, later ones on top:
//!
//! - [`Mode::Single`]: one segment, the combined level ([`Entry::combined`]), one colour.
//! - [`Mode::ThreeBand`]: one segment per band in the findings.md palette. In the zoom
//!   ([`Layout::Overlaid`]) each band starts at the centre, painted low → mid → high, so
//!   the wide indigo body is behind and the cream highs are an inner core. In the overview
//!   ([`Layout::Stacked`]) the bands stack outwards: low, then mid on it, then high.
//! - [`Mode::Rgb`]: one segment at the combined level, coloured from the band ratios.
//!
//! Heights are `gain · value / 255`: the stored square-root curve is the display curve, and
//! the per-band gains are display settings (uniforms in the shader), not stored data.

use crate::compute::Entry;

/// An sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);

    /// From `0xRRGGBB`.
    pub const fn hex(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Each channel times `f` (0..=1), in sRGB values; good enough for dimming.
    pub fn scale(self, f: f32) -> Rgb {
        let s = |c: u8| (f32::from(c) * f).round().clamp(0.0, 255.0) as u8;
        Rgb(s(self.0), s(self.1), s(self.2))
    }

    /// `self` over `under` at opacity `alpha`.
    pub fn over(self, under: Rgb, alpha: f32) -> Rgb {
        let m = |a: u8, b: u8| {
            (f32::from(a) * alpha + f32::from(b) * (1.0 - alpha))
                .round()
                .clamp(0.0, 255.0) as u8
        };
        Rgb(m(self.0, under.0), m(self.1, under.1), m(self.2, under.2))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Single,
    ThreeBand,
    Rgb,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Single, Mode::ThreeBand, Mode::Rgb];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Single => "single",
            Mode::ThreeBand => "3band",
            Mode::Rgb => "rgb",
        }
    }
}

/// How a 3-band column is built: bands overlaid from the centre (zoom) or stacked
/// (overview). Single colour and RGB draw one segment either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Overlaid,
    Stacked,
}

/// Colours and gains. All of it is display settings; changing it never touches stored data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Single colour mode.
    pub single: Rgb,
    /// 3-band mode: low, mid, high (findings.md).
    pub bands: [Rgb; 3],
    /// RGB mode: the colour each band contributes (red low, green mid, blue high).
    pub rgb: [Rgb; 3],
    /// Display gain per band (low, mid, high); heights clamp at 1.
    pub gains: [f32; 3],
    /// Display gain on the combined level (single colour and RGB).
    pub combined_gain: f32,
    /// In [`Layout::Stacked`], each band's height is scaled by this before stacking, so a
    /// full-scale column of all three bands does not simply clip.
    pub stack_scale: f32,
    /// Brightness of the played part (0..=1).
    pub played: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            single: Rgb::hex(0x7CB4FF),
            bands: [Rgb::hex(0x4B4FC2), Rgb::hex(0x3FB8AE), Rgb::hex(0xF4EAD2)],
            rgb: [Rgb::hex(0xFF0000), Rgb::hex(0x00FF00), Rgb::hex(0x0000FF)],
            gains: [1.0, 0.85, 0.7],
            combined_gain: 1.0,
            stack_scale: 1.0 / 3f32.sqrt(),
            played: 0.4,
        }
    }
}

/// One mirrored segment: from `from` to `to` (0 = centre, 1 = edge) on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Segment {
    pub from: f32,
    pub to: f32,
    pub color: Rgb,
}

/// What one column draws: up to three segments, in paint order.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Column {
    segments: [Segment; 3],
    len: usize,
}

impl Column {
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.len]
    }

    fn push(&mut self, s: Segment) {
        self.segments[self.len] = s;
        self.len += 1;
    }

    /// The same column, every colour scaled by `f` (played-part dimming).
    pub fn dimmed(mut self, f: f32) -> Column {
        for s in &mut self.segments[..self.len] {
            s.color = s.color.scale(f);
        }
        self
    }

    /// The colour at distance `d` from the centre (0..=1), if any segment covers it:
    /// the last one painted wins.
    pub fn color_at(&self, d: f32) -> Option<Rgb> {
        self.segments()
            .iter()
            .rev()
            .find(|s| d >= s.from && d < s.to)
            .map(|s| s.color)
    }
}

fn height(value: u8, gain: f32) -> f32 {
    (gain * f32::from(value) / 255.0).clamp(0.0, 1.0)
}

/// The column for `e` in `mode`.
pub fn column(e: Entry, mode: Mode, layout: Layout, style: &Style) -> Column {
    let mut c = Column::default();
    let combined = || height(e.combined(), style.combined_gain);
    match mode {
        Mode::Single => c.push(Segment {
            from: 0.0,
            to: combined(),
            color: style.single,
        }),
        Mode::Rgb => c.push(Segment {
            from: 0.0,
            to: combined(),
            color: rgb_color(e, style),
        }),
        Mode::ThreeBand => {
            let mut base = 0.0;
            for (i, v) in [e.low, e.mid, e.high].into_iter().enumerate() {
                let h = height(v, style.gains[i]);
                let (from, to) = match layout {
                    Layout::Overlaid => (0.0, h),
                    Layout::Stacked => (base, (base + h * style.stack_scale).min(1.0)),
                };
                base = to;
                if to > from {
                    c.push(Segment {
                        from,
                        to,
                        color: style.bands[i],
                    });
                }
            }
        }
    }
    c
}

/// RGB mode's colour: each band's colour weighted by its gained value, then scaled so the
/// brightest channel is full (the colour carries the band ratios, the height the level).
/// Silence is black.
pub fn rgb_color(e: Entry, style: &Style) -> Rgb {
    let mut sum = [0.0f32; 3];
    for ((v, g), c) in [e.low, e.mid, e.high]
        .into_iter()
        .zip(style.gains)
        .zip(style.rgb)
    {
        for (s, ch) in sum.iter_mut().zip([c.0, c.1, c.2]) {
            *s += f32::from(v) * g * f32::from(ch);
        }
    }
    let peak = sum.iter().cloned().fold(0.0, f32::max);
    if peak <= 0.0 {
        return Rgb::BLACK;
    }
    let ch = |s: f32| (255.0 * s / peak).round() as u8;
    Rgb(ch(sum[0]), ch(sum[1]), ch(sum[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn e(low: u8, mid: u8, high: u8) -> Entry {
        Entry { low, mid, high }
    }

    fn flat() -> Style {
        Style {
            gains: [1.0; 3],
            ..Style::default()
        }
    }

    #[test]
    fn single_colour_draws_the_combined_level() {
        let s = flat();
        let c = column(e(0, 255, 0), Mode::Single, Layout::Overlaid, &s);
        assert_eq!(
            c.segments(),
            [Segment {
                from: 0.0,
                to: 1.0,
                color: s.single
            }]
        );
        let c = column(e(128, 128, 0), Mode::Single, Layout::Stacked, &s);
        assert_eq!(c.segments()[0].to, 152.0 / 255.0);
    }

    #[test]
    fn three_band_overlays_in_paint_order() {
        let s = flat();
        let c = column(e(255, 102, 51), Mode::ThreeBand, Layout::Overlaid, &s);
        let seg = c.segments();
        assert_eq!(seg.len(), 3);
        assert_eq!(
            seg.iter().map(|s| s.color).collect::<Vec<_>>(),
            [Rgb::hex(0x4B4FC2), Rgb::hex(0x3FB8AE), Rgb::hex(0xF4EAD2)]
        );
        assert!(seg.iter().all(|s| s.from == 0.0));
        assert_eq!(
            seg.iter().map(|s| s.to).collect::<Vec<_>>(),
            [1.0, 0.4, 0.2]
        );
        // Highs are the inner core, mids around them, lows outside.
        assert_eq!(c.color_at(0.1), Some(s.bands[2]));
        assert_eq!(c.color_at(0.3), Some(s.bands[1]));
        assert_eq!(c.color_at(0.9), Some(s.bands[0]));
        assert_eq!(c.color_at(1.0), None);
        // A silent band draws nothing.
        assert_eq!(
            column(e(0, 0, 9), Mode::ThreeBand, Layout::Overlaid, &s)
                .segments()
                .len(),
            1
        );
    }

    #[test]
    fn three_band_stacks_in_the_overview() {
        let s = Style {
            stack_scale: 0.5,
            ..flat()
        };
        let c = column(e(102, 51, 204), Mode::ThreeBand, Layout::Stacked, &s);
        let seg = c.segments();
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(seg[0].from, 0.0) && close(seg[0].to, 0.2));
        assert!(close(seg[1].from, 0.2) && close(seg[1].to, 0.3));
        assert!(close(seg[2].from, 0.3) && close(seg[2].to, 0.7));
        // Full scale clips at the edge.
        let c = column(e(255, 255, 255), Mode::ThreeBand, Layout::Stacked, &s);
        assert_eq!(c.segments().last().unwrap().to, 1.0);
    }

    #[test]
    fn gains_scale_and_clamp() {
        let s = Style {
            gains: [2.0, 0.5, 1.0],
            ..Style::default()
        };
        let c = column(e(200, 200, 0), Mode::ThreeBand, Layout::Overlaid, &s);
        assert_eq!(c.segments()[0].to, 1.0);
        assert_eq!(c.segments()[1].to, 100.0 / 255.0);
    }

    #[test]
    fn rgb_colour_follows_the_band_ratios() {
        let s = flat();
        assert_eq!(rgb_color(e(200, 0, 0), &s), Rgb(255, 0, 0));
        assert_eq!(rgb_color(e(0, 50, 0), &s), Rgb(0, 255, 0));
        assert_eq!(rgb_color(e(0, 0, 255), &s), Rgb(0, 0, 255));
        assert_eq!(rgb_color(e(100, 50, 0), &s), Rgb(255, 128, 0));
        assert_eq!(rgb_color(e(90, 90, 90), &s), Rgb(255, 255, 255));
        assert_eq!(rgb_color(e(0, 0, 0), &s), Rgb::BLACK);
        // The height is the combined level, the colour only the ratio.
        let c = column(e(100, 50, 0), Mode::Rgb, Layout::Overlaid, &s);
        assert_eq!(
            c.segments()[0].to,
            f32::from(e(100, 50, 0).combined()) / 255.0
        );
        assert_eq!(c.segments()[0].color, Rgb(255, 128, 0));
    }

    #[test]
    fn played_part_is_dimmed() {
        let s = Style::default();
        let c = column(e(255, 255, 255), Mode::ThreeBand, Layout::Overlaid, &s).dimmed(0.5);
        assert_eq!(c.segments()[2].color, Rgb(122, 117, 105));
        assert_eq!(c.segments()[0].to, 1.0);
        assert_eq!(Rgb(200, 100, 0).over(Rgb(0, 0, 100), 0.25), Rgb(50, 25, 75));
    }
}
