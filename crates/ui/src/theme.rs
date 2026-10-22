//! Design tokens: every colour, type size and shape value the screens use, in one [`Theme`].
//!
//! Screens never write a colour or a font size of their own; they ask the theme by role
//! ("button edge", "readout", "cue"), so a theme switch (daylight, high contrast, a user's
//! colours) is a new `Theme` value and no layout change. The default is the wireframe's
//! tokens (`docs/research/findings.md`, "Reference UI wireframes — design tokens", and the
//! 1024×600 canvas).
//!
//! The waveform itself is drawn with the waveform crate's [`waveform::display::Style`] (band
//! palettes, RGB weights, per-band gains, played-part dimming), held in [`WaveformTokens`]
//! next to the colours of what the UI draws around it (grid, cue, loop, playhead), so one
//! theme drives the screen and the waveform shader.

use crate::render::scene::{Align, Rgba, Weight};

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub color: Colors,
    pub font: TypeScale,
    /// Text roles built from `font` and `color`: what screens draw text with.
    pub text: TextStyles,
    pub shape: Shape,
    pub waveform: WaveformTokens,
    /// The hardware stand-in's colours (a debug shell around the device screen, not part of
    /// the device's look).
    pub shell: ShellColors,
}

/// Colours by role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    /// Screen background, and the fill of sections and meters.
    pub background: Rgba,
    /// Section outlines and column dividers.
    pub section_edge: Rgba,
    /// The rail's edge and the rules beside a rule label.
    pub rule: Rgba,

    pub button: Rgba,
    pub button_edge: Rgba,
    pub button_pressed: Rgba,
    pub button_pressed_edge: Rgba,
    /// A button that is on (USB, MASTER, SLIP while enabled).
    pub button_on: Rgba,
    pub button_on_edge: Rgba,

    pub chip_edge: Rgba,
    /// A chip that is selected (ZOOM).
    pub chip_selected: Rgba,

    pub text: Rgba,
    pub label: Rgba,
    /// Disabled or secondary text.
    pub dim: Rgba,
    /// Text on a coloured badge.
    pub on_badge: Rgba,

    /// The accent: active state, slip shadow, BPM, beat meter.
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub accent_dim: Rgba,

    /// Cue, loop, master and "on" indicators (the reference's amber).
    pub cue: Rgba,
    pub cue_text: Rgba,
    pub cue_dim: Rgba,

    pub key: Rgba,
    pub title_bar: Rgba,
    /// The track colour strip on the title bar, until tracks carry their own.
    pub track_color: Rgba,
    /// A pad's colour until the library gives hot cues their own.
    pub hot_cue: Rgba,
    /// Hot cue colours by pad (A, B, …) until the library gives each cue its own.
    pub hot_cue_colors: [Rgba; 8],
    /// How strongly a set pad is tinted with its colour (0..=1).
    pub pad_tint: f32,
    /// Destructive actions armed (delete): edge, fill, text.
    pub danger: Rgba,
    pub danger_fill: Rgba,
    pub danger_text: Rgba,
    /// Behind the timing overlay.
    pub overlay: Rgba,
    /// The browse list: the cursor's row, the rule under each row, folder icons.
    pub row_selected: Rgba,
    pub row_rule: Rgba,
    pub folder: Rgba,
}

/// Font sizes by role, in design pixels (Barlow Condensed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeScale {
    /// Line box height as a multiple of the size, for running text (CSS `normal`).
    pub line_height: f32,
    /// Section labels: TRACK, REMAIN, TEMPO, rule labels.
    pub label: f32,
    /// Second lines, units, hints.
    pub small: f32,
    /// The smallest text (the MASTER PLAYER badge label).
    pub caption: f32,
    /// Performance pads, and the rail's mode buttons.
    pub pad: f32,
    pub rail_mode: f32,
    pub button: f32,
    pub chip: f32,
    pub chip_large: f32,
    /// Track title.
    pub title: f32,
    /// Text beside the title (key).
    pub title_aside: f32,
    /// Large readouts: track number, remaining time, tempo.
    pub readout: f32,
    /// The milliseconds after the remaining time.
    pub readout_fraction: f32,
    /// The current BPM.
    pub bpm: f32,
    /// The player number on the rail.
    pub player: f32,
    /// Bar counters beside the phase meters.
    pub counter: f32,
    /// The number in the master badge.
    pub badge: f32,
    /// A unit after a readout (%).
    pub unit: f32,
    /// Browse list rows and their header.
    pub row: f32,
}

/// How a piece of text looks: its role's size, weight, colour and line box, plus the
/// alignment and width limit a call site sets. Take one from [`TextStyles`] and adjust it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: Weight,
    pub color: Rgba,
    /// Line box height as a multiple of `size` (CSS `line-height`).
    pub line_height: f32,
    /// Which edge (or the centre) the anchor x is.
    pub align: Align,
    /// Longest the line may be (design pixels); longer text ends in an ellipsis.
    pub max_width: Option<f32>,
}

impl TextStyle {
    const fn new(size: f32, weight: Weight, color: Rgba, line_height: f32) -> Self {
        Self {
            size,
            weight,
            color,
            line_height,
            align: Align::Left,
            max_width: None,
        }
    }

    pub const fn color(self, color: Rgba) -> Self {
        Self { color, ..self }
    }

    pub const fn weight(self, weight: Weight) -> Self {
        Self { weight, ..self }
    }

    pub const fn semibold(self) -> Self {
        self.weight(Weight::SemiBold)
    }

    pub const fn align(self, align: Align) -> Self {
        Self { align, ..self }
    }

    pub const fn center(self) -> Self {
        self.align(Align::Center)
    }

    pub const fn right(self) -> Self {
        self.align(Align::Right)
    }

    pub const fn max_width(self, w: f32) -> Self {
        Self {
            max_width: Some(w),
            ..self
        }
    }

    /// The height of one line box.
    pub fn line_box(self) -> f32 {
        self.size * self.line_height
    }
}

/// Text by role. Running text uses the type scale's line height; number readouts use a
/// line box of exactly their size (CSS `line-height: 1`), as the wireframe sets them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyles {
    pub label: TextStyle,
    pub small: TextStyle,
    pub caption: TextStyle,
    pub pad: TextStyle,
    pub rail_mode: TextStyle,
    pub button: TextStyle,
    pub chip: TextStyle,
    pub chip_large: TextStyle,
    pub title: TextStyle,
    pub title_aside: TextStyle,
    pub row: TextStyle,
    pub readout: TextStyle,
    pub readout_fraction: TextStyle,
    pub bpm: TextStyle,
    pub player: TextStyle,
    pub counter: TextStyle,
    pub badge: TextStyle,
    pub unit: TextStyle,
}

impl TextStyles {
    pub fn new(f: &TypeScale, c: &Colors) -> Self {
        let text = |size, weight, color| TextStyle::new(size, weight, color, f.line_height);
        let digits = |size| TextStyle::new(size, Weight::SemiBold, c.text, 1.0);
        Self {
            label: text(f.label, Weight::Regular, c.label),
            small: text(f.small, Weight::Regular, c.label),
            caption: text(f.caption, Weight::Regular, c.label),
            pad: text(f.pad, Weight::SemiBold, c.text),
            rail_mode: text(f.rail_mode, Weight::SemiBold, c.text),
            button: text(f.button, Weight::SemiBold, c.text),
            chip: text(f.chip, Weight::SemiBold, c.text),
            chip_large: text(f.chip_large, Weight::SemiBold, c.text),
            title: text(f.title, Weight::SemiBold, c.text),
            title_aside: text(f.title_aside, Weight::SemiBold, c.key),
            row: text(f.row, Weight::Regular, c.text),
            readout: digits(f.readout),
            readout_fraction: digits(f.readout_fraction),
            bpm: digits(f.bpm),
            player: digits(f.player),
            counter: digits(f.counter),
            badge: digits(f.badge),
            unit: digits(f.unit),
        }
    }
}

/// The hardware stand-in: a dark body with grey caps, and the reference player's amber
/// (CUE, loops), green (PLAY), red (REV) and blue (displays, keylock).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellColors {
    pub body: Rgba,
    pub panel: Rgba,
    pub panel_edge: Rgba,
    pub edge: Rgba,
    pub edge_light: Rgba,
    pub button_top: Rgba,
    pub button_bottom: Rgba,
    pub button_down: Rgba,
    pub text: Rgba,
    pub label: Rgba,
    pub sub: Rgba,
    /// Key caps showing the dev keyboard's shortcuts.
    pub key_fill: Rgba,
    pub key_text: Rgba,
    pub amber: Rgba,
    pub amber_text: Rgba,
    pub amber_dim: Rgba,
    pub amber_edge: Rgba,
    /// Text on an amber button.
    pub on_amber: Rgba,
    pub green: Rgba,
    pub green_text: Rgba,
    pub green_dim: Rgba,
    pub red_edge: Rgba,
    pub red_fill: Rgba,
    pub red_lit: Rgba,
    pub red_text: Rgba,
    pub blue: Rgba,
    pub blue_fill: Rgba,
    pub blue_text: Rgba,
    pub knob: Rgba,
    pub fader_cap: Rgba,
    pub fader_cap_edge: Rgba,
    /// The hot cue pads' panel, a set pad, and each pad's colour.
    pub pads_panel: Rgba,
    pub pads_panel_edge: Rgba,
    pub pad_set: Rgba,
    pub pads: [Rgba; 8],
    pub jog_ring: Rgba,
    pub jog_ring_active: Rgba,
    pub jog_plate: Rgba,
    pub jog_plate_active: Rgba,
    /// The mark on the jog that turns with the track.
    pub jog_marker: Rgba,
    pub black: Rgba,
}

/// Corner radii and line widths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub section_radius: f32,
    pub button_radius: f32,
    pub chip_radius: f32,
    /// Waveform panels and small markers.
    pub panel_radius: f32,
    pub border: f32,
    /// The highlighted outline of an active segment or slipping button.
    pub border_active: f32,
}

/// The waveform areas: the waveform's own style, and what the UI draws around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveformTokens {
    /// Palettes, gains and dimming of the waveform (the waveform crate's theme). The track
    /// gain inside it is set per track at run time.
    pub style: waveform::display::Style,
    /// The panel behind the waveform: a vertical gradient from `background` at the edges
    /// (top, for a bottom-aligned view) to `background_glow` at the baseline.
    pub background: Rgba,
    pub background_glow: Rgba,
    /// Opacity of the drawn waveform over its panel and grid (0..=1).
    pub opacity: f32,
    pub center_line: Rgba,
    /// Beat ticks along the zoom's top and bottom edges.
    pub beat_line: Rgba,
    /// Downbeat (first beat of a bar) ticks.
    pub downbeat_line: Rgba,
    /// Length of a beat tick from the edge, design pixels.
    pub tick_length: f32,
    /// Width of a beat tick and of a downbeat tick.
    pub tick_width: f32,
    pub downbeat_tick_width: f32,
    /// The overview's 16-bar ticks.
    pub bar_line: Rgba,
    pub playhead: Rgba,
    pub cue: Rgba,
    pub loop_color: Rgba,
    /// Loop fill opacity: active and stored loop, zoom and overview.
    pub loop_fill: f32,
    pub loop_fill_stored: f32,
    pub overview_loop_fill: f32,
    pub overview_loop_fill_stored: f32,
    pub loop_edge_stored: Rgba,
    /// The overview's played part.
    pub played: Rgba,
    /// The overview's not-yet-loaded part.
    pub loading: Rgba,
    pub slip_shadow: Rgba,
}

impl Default for Theme {
    fn default() -> Self {
        Self::reference()
    }
}

impl Theme {
    /// The wireframe's look: black panels, grey chrome, electric-blue accent, amber cues.
    pub fn reference() -> Self {
        let accent = Rgba::hex(0x2b7fff);
        let amber = Rgba::hex(0xe0a33a);
        let dim = Rgba::hex(0x6b6f75);
        let color = Colors {
            background: Rgba::hex(0x000000),
            section_edge: Rgba::hex(0x34373d),
            rule: Rgba::hex(0x3a3d42),
            button: Rgba::hex(0x222428),
            button_edge: Rgba::hex(0x5f6369),
            button_pressed: Rgba::hex(0x33373d),
            button_pressed_edge: Rgba::hex(0xe6e6e6),
            button_on: Rgba::hex(0x0b2350),
            button_on_edge: accent,
            chip_edge: Rgba::hex(0x7a7e84),
            chip_selected: Rgba::hex(0x33363b),
            text: Rgba::hex(0xe6e6e6),
            label: Rgba::hex(0xa6a9ae),
            dim,
            on_badge: Rgba::hex(0x000000),
            accent,
            accent_text: Rgba::hex(0x7cb4ff),
            accent_dim: Rgba::hex(0x22457a),
            cue: amber,
            cue_text: Rgba::hex(0xf2c06a),
            cue_dim: Rgba::hex(0x7a6a4a),
            key: Rgba::hex(0x6cc4f0),
            title_bar: Rgba::hex(0x1e2024),
            track_color: Rgba::hex(0x8b7be0),
            hot_cue: Rgba::hex(0xe8667a),
            hot_cue_colors: [
                Rgba::hex(0xe8667a), // rose
                Rgba::hex(0xe0a33a), // amber
                Rgba::hex(0x5ab8ec), // sky
                Rgba::hex(0x6cc08b), // green
                Rgba::hex(0xa78bfa), // violet
                Rgba::hex(0xf0762a), // orange
                Rgba::hex(0x3fb8ae), // teal
                Rgba::hex(0xe878c8), // pink
            ],
            pad_tint: 0.04,
            danger: Rgba::hex(0xe8667a),
            danger_fill: Rgba::hex(0x3a1a20),
            danger_text: Rgba::hex(0xf0a0ab),
            overlay: Rgba::hex(0x000000).alpha(0.75),
            row_selected: accent.alpha(0.09),
            row_rule: Rgba::hex(0x2c2e33),
            folder: Rgba::hex(0x7a7e84),
        };
        let font = TypeScale {
            line_height: 1.2,
            label: 15.0,
            small: 14.0,
            caption: 13.0,
            pad: 25.0,
            rail_mode: 18.0,
            button: 20.0,
            chip: 16.0,
            chip_large: 22.0,
            title: 23.0,
            title_aside: 22.0,
            readout: 56.0,
            readout_fraction: 37.0,
            bpm: 41.0,
            player: 39.0,
            counter: 27.0,
            badge: 19.0,
            unit: 22.0,
            row: 20.0,
        };
        Theme {
            name: "reference",
            text: TextStyles::new(&font, &color),
            color,
            font,
            shape: Shape {
                section_radius: 6.0,
                button_radius: 4.0,
                chip_radius: 3.0,
                panel_radius: 2.0,
                border: 1.0,
                border_active: 2.0,
            },
            waveform: WaveformTokens {
                style: waveform::display::Style::default(),
                background: Rgba::hex(0x07080a),
                background_glow: Rgba::hex(0x1f2430),
                opacity: 0.7,
                center_line: Rgba::hex(0x15171b),
                beat_line: Rgba::hex(0xffffff),
                downbeat_line: Rgba::hex(0xd93a3a),
                tick_length: 12.0,
                tick_width: 1.5,
                downbeat_tick_width: 2.5,
                bar_line: Rgba::hex(0x48505a),
                playhead: Rgba::hex(0xe6e6e6),
                cue: amber,
                loop_color: amber,
                loop_fill: 0.16,
                loop_fill_stored: 0.05,
                overview_loop_fill: 0.45,
                overview_loop_fill_stored: 0.2,
                loop_edge_stored: Rgba::hex(0x7a6a4a),
                played: accent.alpha(0.18),
                loading: dim.alpha(0.5),
                slip_shadow: accent.alpha(0.8),
            },
            shell: ShellColors {
                body: Rgba::hex(0x141619),
                panel: Rgba::hex(0x0b0c0e),
                panel_edge: Rgba::hex(0x26292e),
                edge: Rgba::hex(0x3a3d42),
                edge_light: Rgba::hex(0x5f6369),
                button_top: Rgba::hex(0x2a2d32),
                button_bottom: Rgba::hex(0x1d1f23),
                button_down: Rgba::hex(0x3a3e45),
                text: Rgba::hex(0xe6e6e6),
                label: Rgba::hex(0xc9ccd1),
                sub: Rgba::hex(0x8f939a),
                key_fill: Rgba::hex(0x1a1c20),
                key_text: Rgba::hex(0x7cb4ff),
                amber: Rgba::hex(0xe0a33a),
                amber_text: Rgba::hex(0xf2c06a),
                amber_dim: Rgba::hex(0x5a4420),
                amber_edge: Rgba::hex(0x7a5a22),
                on_amber: Rgba::hex(0x1a1205),
                green: Rgba::hex(0x3fbf6a),
                green_text: Rgba::hex(0x7ee0a0),
                green_dim: Rgba::hex(0x1d4a2c),
                red_edge: Rgba::hex(0x8a3040),
                red_fill: Rgba::hex(0x3a141b),
                red_lit: Rgba::hex(0x7a2232),
                red_text: Rgba::hex(0xffb3bf),
                blue: Rgba::hex(0x2b7fff),
                blue_fill: Rgba::hex(0x0b2350),
                blue_text: Rgba::hex(0x7cb4ff),
                knob: Rgba::hex(0xb9bcc1),
                fader_cap: Rgba::hex(0x4a4e55),
                fader_cap_edge: Rgba::hex(0x7a7e84),
                pads_panel: Rgba::hex(0x0e1420),
                pads_panel_edge: Rgba::hex(0x22457a),
                pad_set: Rgba::hex(0x1f1d22),
                pads: [
                    Rgba::hex(0xe8667a),
                    Rgba::hex(0xe0a33a),
                    Rgba::hex(0x5ab8ec),
                    Rgba::hex(0x6cc08b),
                    Rgba::hex(0xa78bfa),
                    Rgba::hex(0xf0762a),
                    Rgba::hex(0x3fb8ae),
                    Rgba::hex(0xe878c8),
                ],
                jog_ring: Rgba::hex(0x24272b),
                jog_ring_active: Rgba::hex(0x30343a),
                jog_plate: Rgba::hex(0x121316),
                jog_plate_active: Rgba::hex(0x1a1c20),
                jog_marker: Rgba::hex(0xe8667a),
                black: Rgba::hex(0x000000),
            },
        }
    }

    /// The same theme with the waveform drawn in the waveform crate's theme `name` (its
    /// presets: default, findings, spectrum, blended).
    pub fn with_waveform_theme(mut self, name: &str) -> Option<Self> {
        self.waveform.style.theme = waveform::display::Theme::by_name(name)?;
        Some(self)
    }

    /// Every theme this build knows, by name.
    pub fn named(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|t| t.name == name)
    }

    pub fn all() -> Vec<Self> {
        vec![Self::reference()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screens take colours and sizes from the theme only: no colour literal outside this
    /// file (tests excepted).
    #[test]
    fn screens_have_no_colour_literals() {
        for (file, src) in [
            ("main.rs", include_str!("main.rs")),
            ("screens/mod.rs", include_str!("screens/mod.rs")),
            ("screens/chrome.rs", include_str!("screens/chrome.rs")),
            ("screens/deck.rs", include_str!("screens/deck.rs")),
            ("screens/main.rs", include_str!("screens/main.rs")),
            ("screens/info.rs", include_str!("screens/info.rs")),
            ("screens/perform.rs", include_str!("screens/perform.rs")),
            (
                "screens/browse/mod.rs",
                include_str!("screens/browse/mod.rs"),
            ),
            (
                "screens/browse/keyboard.rs",
                include_str!("screens/browse/keyboard.rs"),
            ),
            (
                "screens/browse/list.rs",
                include_str!("screens/browse/list.rs"),
            ),
            (
                "screens/browse/panels.rs",
                include_str!("screens/browse/panels.rs"),
            ),
            (
                "screens/browse/modal.rs",
                include_str!("screens/browse/modal.rs"),
            ),
            ("standin.rs", include_str!("standin.rs")),
            ("kit/mod.rs", include_str!("kit/mod.rs")),
            ("kit/rect.rs", include_str!("kit/rect.rs")),
            ("render/mod.rs", include_str!("render/mod.rs")),
            ("render/rects.rs", include_str!("render/rects.rs")),
            ("render/waves.rs", include_str!("render/waves.rs")),
            ("render/images.rs", include_str!("render/images.rs")),
            ("render/text.rs", include_str!("render/text.rs")),
            ("render/offscreen.rs", include_str!("render/offscreen.rs")),
        ] {
            let code = src.split("#[cfg(test)]").next().unwrap_or(src);
            for pattern in ["Rgba::hex(", "Rgba(["] {
                assert!(
                    !code.contains(pattern),
                    "{file} has a colour literal ({pattern}); add a token to theme.rs"
                );
            }
        }
    }

    #[test]
    fn themes_are_found_by_name() {
        for t in Theme::all() {
            assert_eq!(Theme::named(t.name).as_ref(), Some(&t));
        }
        assert!(Theme::named("nope").is_none());
    }
}
