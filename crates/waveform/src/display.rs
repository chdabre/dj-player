//! Display modes: from one entry's band values to what a column draws.
//!
//! Every mode draws from the same stored data. A column is a list of mirrored segments,
//! measured from the centre line outwards (0 = centre, 1 = the view's half-height), drawn
//! in order, later ones on top, each at its band's opacity over what is below:
//!
//! - [`Mode::Single`]: one segment, the combined level ([`Entry::combined`]), one colour.
//! - [`Mode::ThreeBand`]: one segment per band in the theme's band colours and opacities,
//!   in one of two layouts, which either view can use:
//!   - [`Layout::Overlaid`] (the zoom's default): each band starts at the centre, painted
//!     low → mid → high, so the wide low body is behind and the highs are an inner core.
//!     With a mid opacity below 1, mids blend with the lows where they overlap (as the
//!     CDJ-3000's 3-band detail does).
//!   - [`Layout::Stacked`] (the overview's default): the bands stack outwards, low, then
//!     mid on it, then high.
//! - [`Mode::Rgb`]: one segment at the combined level, coloured from the band ratios.
//!
//! Heights are `track gain · band gain · value / 255`: the stored square-root curve is the
//! display curve. The gains (per band, and per track from [`auto_gain`]) are display
//! settings, uniforms in the shader, not stored data. All colours come from a [`Theme`].

use crate::compute::Entry;
pub use crate::gain::{
    AUTO_GAIN_PERCENTILE, AUTO_GAIN_RANGE, AUTO_GAIN_TARGET, TrackGain, auto_gain,
};
pub use crate::theme::{Rgb, Theme};

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

/// A theme plus the display settings next to it. None of it touches stored data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub theme: Theme,
    /// Display gain per band (low, mid, high); heights clamp at 1.
    pub gains: [f32; 3],
    /// Display gain on the combined level (single colour and RGB).
    pub combined_gain: f32,
    /// Per-track gain on every height (bands and combined), so loud and quiet masters both
    /// use the view; one uniform. Set it from [`TrackGain::resolve`]; 1 = off.
    pub track_gain: f32,
    /// In [`Layout::Stacked`], each band's height is scaled by this before stacking, so a
    /// full-scale column of all three bands does not simply clip.
    pub stack_scale: f32,
    /// Brightness of the played part (0..=1).
    pub played: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            theme: Theme::DEFAULT,
            gains: [1.0, 0.85, 0.7],
            combined_gain: 1.0,
            track_gain: 1.0,
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
    /// Opacity over what is below (0..=1).
    pub alpha: f32,
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

    /// The colour at distance `d` from the centre (0..=1) over `under` (the background
    /// or grid there): every segment covering `d` blended on in paint order. `None` if no
    /// segment covers it.
    pub fn color_at(&self, d: f32, under: Rgb) -> Option<Rgb> {
        let mut covered = false;
        let c = self
            .segments()
            .iter()
            .filter(|s| d >= s.from && d < s.to)
            .fold(under, |below, s| {
                covered = true;
                s.color.over(below, s.alpha)
            });
        covered.then_some(c)
    }
}

fn height(value: u8, gain: f32, style: &Style) -> f32 {
    (style.track_gain * gain * f32::from(value) / 255.0).clamp(0.0, 1.0)
}

/// The column for `e` in `mode`.
pub fn column(e: Entry, mode: Mode, layout: Layout, style: &Style) -> Column {
    let mut c = Column::default();
    let combined = || height(e.combined(), style.combined_gain, style);
    match mode {
        Mode::Single => c.push(Segment {
            from: 0.0,
            to: combined(),
            color: style.theme.single,
            alpha: 1.0,
        }),
        Mode::Rgb => c.push(Segment {
            from: 0.0,
            to: combined(),
            color: rgb_color(e, &style.theme),
            alpha: 1.0,
        }),
        Mode::ThreeBand => {
            let mut base = 0.0;
            for (i, v) in [e.low, e.mid, e.high].into_iter().enumerate() {
                let h = height(v, style.gains[i], style);
                let (from, to) = match layout {
                    Layout::Overlaid => (0.0, h),
                    Layout::Stacked => (base, (base + h * style.stack_scale).min(1.0)),
                };
                base = to;
                if to > from {
                    c.push(Segment {
                        from,
                        to,
                        color: style.theme.bands[i],
                        alpha: style.theme.band_alpha[i],
                    });
                }
            }
        }
    }
    c
}

/// RGB mode's colour: each band's colour weighted by its value times the theme's weight,
/// then scaled so the brightest channel is full (the colour carries the band ratios, the
/// height the level). Silence is black.
pub fn rgb_color(e: Entry, theme: &Theme) -> Rgb {
    let mut sum = [0.0f32; 3];
    for ((v, g), c) in [e.low, e.mid, e.high]
        .into_iter()
        .zip(theme.rgb_weights)
        .zip(theme.rgb)
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
            theme: Theme {
                rgb_weights: [1.0; 3],
                ..Theme::DEFAULT
            },
            gains: [1.0; 3],
            ..Style::default()
        }
    }

    fn rgb_color_flat(e: Entry, s: &Style) -> Rgb {
        rgb_color(e, &s.theme)
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
                color: s.theme.single,
                alpha: 1.0,
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
            [Rgb::hex(0x2F6FE8), Rgb::hex(0xF0762A), Rgb::hex(0xF5F5F2)]
        );
        assert!(seg.iter().all(|s| s.from == 0.0));
        assert_eq!(
            seg.iter().map(|s| s.to).collect::<Vec<_>>(),
            [1.0, 0.4, 0.2]
        );
        // Highs are the inner core, mids around them, lows outside.
        let bg = s.theme.background;
        assert_eq!(c.color_at(0.1, bg), Some(s.theme.bands[2]));
        assert_eq!(c.color_at(0.3, bg), Some(s.theme.bands[1]));
        assert_eq!(c.color_at(0.9, bg), Some(s.theme.bands[0]));
        assert_eq!(c.color_at(1.0, bg), None);
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
        assert_eq!(rgb_color_flat(e(200, 0, 0), &s), Rgb(255, 0, 0));
        assert_eq!(rgb_color_flat(e(0, 50, 0), &s), Rgb(0, 255, 0));
        assert_eq!(rgb_color_flat(e(0, 0, 255), &s), Rgb(0, 0, 255));
        assert_eq!(rgb_color_flat(e(100, 50, 0), &s), Rgb(255, 128, 0));
        assert_eq!(rgb_color_flat(e(90, 90, 90), &s), Rgb(255, 255, 255));
        assert_eq!(rgb_color_flat(e(0, 0, 0), &s), Rgb::BLACK);
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
        assert_eq!(c.segments()[2].color, Rgb(123, 123, 121));
        assert_eq!(c.segments()[0].to, 1.0);
        assert_eq!(Rgb(200, 100, 0).over(Rgb(0, 0, 100), 0.25), Rgb(50, 25, 75));
    }

    #[test]
    fn themes_change_colours_not_heights() {
        let e = e(200, 120, 90);
        for mode in Mode::ALL {
            let heights = |t: Theme| {
                let s = Style {
                    theme: t,
                    ..Style::default()
                };
                let c = column(e, mode, Layout::Overlaid, &s);
                c.segments().iter().map(|s| s.to).collect::<Vec<_>>()
            };
            assert_eq!(heights(Theme::DEFAULT), heights(Theme::SPECTRUM));
        }
        // Spectrum weights pull a bass-heavy entry away from pure red.
        let bassy = Entry {
            low: 220,
            mid: 120,
            high: 80,
        };
        let d = rgb_color(bassy, &Theme::DEFAULT);
        let sp = rgb_color(bassy, &Theme::SPECTRUM);
        assert!(sp.1 > d.1 && sp.2 > d.2, "{d:?} vs {sp:?}");
    }

    #[test]
    fn band_opacity_blends_with_what_is_below() {
        let s = Style {
            theme: Theme {
                bands: [Rgb(0, 0, 200), Rgb(200, 100, 0), Rgb(250, 250, 250)],
                band_alpha: [1.0, 0.5, 1.0],
                ..Theme::DEFAULT
            },
            ..flat()
        };
        let bg = Rgb(0, 0, 0);
        let c = column(e(255, 153, 51), Mode::ThreeBand, Layout::Overlaid, &s);
        // Highs opaque on top; mids half over the lows; lows alone outside.
        assert_eq!(c.color_at(0.1, bg), Some(Rgb(250, 250, 250)));
        assert_eq!(c.color_at(0.5, bg), Some(Rgb(100, 50, 100)));
        assert_eq!(c.color_at(0.9, bg), Some(Rgb(0, 0, 200)));
        // Mids taller than the lows: half over the background.
        let c = column(e(51, 204, 0), Mode::ThreeBand, Layout::Overlaid, &s);
        assert_eq!(c.color_at(0.5, bg), Some(Rgb(100, 50, 0)));
        // Opaque bands (the default theme) give exactly the topmost colour.
        let d = Style::default();
        let c = column(e(255, 153, 51), Mode::ThreeBand, Layout::Overlaid, &d);
        assert_eq!(c.color_at(0.3, Rgb(9, 9, 9)), Some(d.theme.bands[1]));
        assert_eq!(
            Theme::by_name("blended").unwrap().band_alpha,
            [1.0, 0.6, 1.0]
        );
    }
}
