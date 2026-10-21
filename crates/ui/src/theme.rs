//! Design tokens: every colour, type size and shape value the screens use, in one [`Theme`].
//!
//! Screens never write a colour or a font size of their own; they ask the theme by role
//! ("button edge", "readout", "cue"), so a theme switch (daylight, high contrast, a user's
//! colours) is a new `Theme` value and no layout change. The default is the wireframe's
//! tokens (`docs/research/findings.md`, "Reference UI wireframes — design tokens", and the
//! 1024×600 canvas).
//!
//! The waveform colours here cover the stand-in drawing (grid, cue, loop, playhead). When the
//! waveform renderer lands, its theme (band palettes, RGB weights) joins [`WaveformTokens`],
//! so one theme drives both the screen and the waveform shader.

use crate::scene::Rgba;

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub color: Colors,
    pub font: TypeScale,
    pub shape: Shape,
    pub waveform: WaveformTokens,
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
    /// Behind the timing overlay.
    pub overlay: Rgba,
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

/// Colours of the waveform areas (stand-in drawing for now).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveformTokens {
    pub background: Rgba,
    pub center_line: Rgba,
    pub beat_line: Rgba,
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
        Theme {
            name: "reference",
            color: Colors {
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
                overlay: Rgba::hex(0x000000).alpha(0.75),
            },
            font: TypeScale {
                line_height: 1.2,
                label: 15.0,
                small: 14.0,
                caption: 13.0,
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
            },
            shape: Shape {
                section_radius: 6.0,
                button_radius: 4.0,
                chip_radius: 3.0,
                panel_radius: 2.0,
                border: 1.0,
                border_active: 2.0,
            },
            waveform: WaveformTokens {
                background: Rgba::hex(0x07080a),
                center_line: Rgba::hex(0x15171b),
                beat_line: Rgba::hex(0x262a30),
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
        }
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
            ("screen.rs", include_str!("screen.rs")),
            ("main.rs", include_str!("main.rs")),
            ("gfx.rs", include_str!("gfx.rs")),
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
