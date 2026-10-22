//! Colours: [`Rgb`] and the [`Theme`] palettes every waveform view draws with.

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

/// The default 3-band colours: low blue, mid red-leaning orange, high white.
const BLUE_ORANGE_WHITE: [Rgb; 3] = [Rgb::hex(0x2F6FE8), Rgb::hex(0xF0762A), Rgb::hex(0xF5F5F2)];

/// Every colour the waveform views draw, as plain values (shader uniforms): the waveform
/// palettes and the UI around them. The renderer hard-codes none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// Single colour mode.
    pub single: Rgb,
    /// 3-band mode: low, mid, high.
    pub bands: [Rgb; 3],
    /// 3-band mode: each band's opacity (0..=1) over what is painted below it.
    pub band_alpha: [f32; 3],
    /// RGB mode: the colour each band contributes (low, mid, high) ...
    pub rgb: [Rgb; 3],
    /// ... and how much: each band's value is weighted by this before mixing.
    pub rgb_weights: [f32; 3],
    pub background: Rgb,
    /// Panel borders and the overview's scale line.
    pub border: Rgb,
    pub beat_line: Rgb,
    pub bar_line: Rgb,
    /// Bar numbers and 16-bar ticks.
    pub label: Rgb,
    pub cue: Rgb,
    pub memory_cue: Rgb,
    pub loop_color: Rgb,
    /// Opacity of the loop region's shading in the zoom and in the overview.
    pub loop_shade: [f32; 2],
    pub playhead: Rgb,
    /// Text on a hot cue tag.
    pub tag_text: Rgb,
    /// Hot cue pad colours (rose, amber, sky); a hot cue carries its own colour, these are
    /// the theme's suggestions.
    pub pads: [Rgb; 3],
}

impl Theme {
    /// Blue / orange / white bands (owner decision 2026-10-21, replacing findings.md's
    /// indigo / teal / cream); the orange leans to red so it stays apart from the amber cue
    /// and loop overlays. Black panels, #34373D borders, amber cues (findings.md); RGB mode
    /// red / green / blue weighted like the 3-band gains.
    pub const DEFAULT: Theme = Theme {
        name: "default",
        single: Rgb::hex(0x7CB4FF),
        bands: BLUE_ORANGE_WHITE,
        band_alpha: [1.0; 3],
        rgb: [Rgb::hex(0xFF0000), Rgb::hex(0x00FF00), Rgb::hex(0x0000FF)],
        rgb_weights: [1.0, 0.85, 0.7],
        background: Rgb::hex(0x000000),
        border: Rgb::hex(0x34373D),
        beat_line: Rgb::hex(0x26282D),
        bar_line: Rgb::hex(0x5A5E66),
        label: Rgb::hex(0x8A8F98),
        cue: Rgb::hex(0xE0A33A),
        memory_cue: Rgb::hex(0x8A8F98),
        loop_color: Rgb::hex(0xF2C06A),
        loop_shade: [0.18, 0.35],
        playhead: Rgb::hex(0xFFFFFF),
        tag_text: Rgb::hex(0x000000),
        pads: [Rgb::hex(0xE8667A), Rgb::hex(0xE0A33A), Rgb::hex(0x5AB8EC)],
    };

    /// The findings.md palette: indigo / teal / cream bands, otherwise the default.
    pub const FINDINGS: Theme = Theme {
        name: "findings",
        bands: [Rgb::hex(0x4B4FC2), Rgb::hex(0x3FB8AE), Rgb::hex(0xF4EAD2)],
        ..Theme::DEFAULT
    };

    /// RGB weighted towards the highs (low 0.45, mid 1.0, high 2.2), since on real music the
    /// low band dominates and equal weights come out red almost everywhere; a white single
    /// colour. Bands as the default.
    pub const SPECTRUM: Theme = Theme {
        name: "spectrum",
        single: Rgb::hex(0xE6E6E6),
        rgb: [Rgb::hex(0xFF2A2A), Rgb::hex(0x2AFF5A), Rgb::hex(0x3A6BFF)],
        rgb_weights: [0.45, 1.0, 2.2],
        ..Theme::DEFAULT
    };

    /// The default colours with the mids at 60 % opacity, so they blend with the lows
    /// where both are drawn and show darker over the background, as on the CDJ-3000.
    pub const BLENDED: Theme = Theme {
        name: "blended",
        band_alpha: [1.0, 0.6, 1.0],
        ..Theme::DEFAULT
    };

    pub const ALL: [Theme; 4] = [
        Theme::DEFAULT,
        Theme::FINDINGS,
        Theme::SPECTRUM,
        Theme::BLENDED,
    ];

    pub fn by_name(name: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.name == name)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_are_found_by_name() {
        assert_eq!(Theme::by_name("default"), Some(Theme::DEFAULT));
        assert_eq!(
            Theme::by_name("findings").unwrap().bands[0],
            Rgb::hex(0x4B4FC2)
        );
        assert_eq!(
            Theme::by_name("spectrum").unwrap().rgb_weights,
            [0.45, 1.0, 2.2]
        );
        assert_eq!(Theme::by_name("nope"), None);
    }
}
