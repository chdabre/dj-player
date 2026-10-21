//! The main playback screen, laid out after the 1024×600 wireframe ("1 · Normal playback",
//! canvas version 28). Pure function of a [`View`]: it fills a [`Scene`] and returns the touch
//! targets, so the layout can be tested and screenshotted without a device.
//!
//! The waveforms are stand-ins until the waveform work lands: the zoom view draws the beat
//! grid, loop, cue and playhead; the overview draws the played part, loop, cue, loading
//! progress and 16-bar ticks.

use engine_core::DeckGrid;

use crate::scene::{Align, Rgba, Scene, Text, Weight};

// Design tokens (docs/research/findings.md, "Reference UI wireframes — design tokens").
const BG: Rgba = Rgba::hex(0x000000);
const SECTION: Rgba = Rgba::hex(0x34373d);
const RAIL_EDGE: Rgba = Rgba::hex(0x3a3d42);
const BUTTON: Rgba = Rgba::hex(0x222428);
const BUTTON_PRESSED: Rgba = Rgba::hex(0x33373d);
const BUTTON_EDGE: Rgba = Rgba::hex(0x5f6369);
const CHIP_EDGE: Rgba = Rgba::hex(0x7a7e84);
const TEXT: Rgba = Rgba::hex(0xe6e6e6);
const LABEL: Rgba = Rgba::hex(0xa6a9ae);
const DIM: Rgba = Rgba::hex(0x6b6f75);
const ACCENT: Rgba = Rgba::hex(0x2b7fff);
const ACCENT_TEXT: Rgba = Rgba::hex(0x7cb4ff);
const ACCENT_FILL: Rgba = Rgba::hex(0x0b2350);
const ACCENT_DIM: Rgba = Rgba::hex(0x22457a);
const AMBER: Rgba = Rgba::hex(0xe0a33a);
const AMBER_TEXT: Rgba = Rgba::hex(0xf2c06a);
const AMBER_DIM: Rgba = Rgba::hex(0x7a6a4a);
const KEY: Rgba = Rgba::hex(0x6cc4f0);
const TITLE_BAR: Rgba = Rgba::hex(0x1e2024);
const TITLE_ACCENT: Rgba = Rgba::hex(0x8b7be0);
const BEAT_LINE: Rgba = Rgba::hex(0x262a30);
const BAR_LINE: Rgba = Rgba::hex(0x48505a);
const HOT_CUE: Rgba = Rgba::hex(0xe8667a);

/// Things on screen that can be touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Library,
    Link,
    Usb,
    Slip,
    Browse,
    TagList,
    Info,
    Menu,
    Perform,
    Delete,
    Memory,
    CallPrev,
    CallNext,
    Zoom,
    Grid,
    Sync,
    Master,
    TempoRange,
    JumpSize,
    /// The overview: touching it seeks (needle search).
    Overview,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitBox {
    pub target: Target,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl HitBox {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// The overview's bounds, for turning a touch into a track position.
pub const OVERVIEW: (f32, f32, f32, f32) = (113.0, 502.0, 703.0, 67.0);
const ZOOM: (f32, f32, f32, f32) = (113.0, 172.0, 703.0, 211.0);

/// Everything the screen shows, read from the deck once per frame.
pub struct View<'a> {
    pub title: &'a str,
    pub key: Option<&'a str>,
    pub sample_rate: f64,
    /// Interpolated playhead (frames).
    pub position: f64,
    pub shadow_position: f64,
    pub length: f64,
    pub loaded_fraction: f64,
    pub rate: f64,
    pub slip_enabled: bool,
    pub slip_active: bool,
    pub keylock: bool,
    /// Quantize resolution in beats; 0 = off.
    pub quantize: f64,
    pub cue: f64,
    pub hot_cues: u8,
    pub looping: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub grid: Option<&'a DeckGrid>,
    pub analysing: bool,
    /// Beats across the zoom view.
    pub zoom_beats: f64,
    pub pressed: Option<Target>,
    /// Frame timing line, if the overlay is on.
    pub debug: Option<&'a str>,
}

struct Ui<'s> {
    scene: &'s mut Scene,
    hits: Vec<HitBox>,
    pressed: Option<Target>,
}

#[allow(clippy::too_many_arguments)]
impl Ui<'_> {
    fn text(
        &mut self,
        s: impl Into<String>,
        x: f32,
        y: f32,
        size: f32,
        weight: Weight,
        color: Rgba,
        align: Align,
    ) {
        self.scene.text(Text {
            text: s.into(),
            x,
            y,
            size,
            line_height: 1.2,
            weight,
            color,
            align,
        });
    }

    /// A number readout: CSS `line-height: 1`.
    fn digits(
        &mut self,
        s: impl Into<String>,
        x: f32,
        y: f32,
        size: f32,
        color: Rgba,
        align: Align,
    ) {
        self.scene.text(Text {
            text: s.into(),
            x,
            y,
            size,
            line_height: 1.0,
            weight: Weight::SemiBold,
            color,
            align,
        });
    }

    fn label(&mut self, s: &str, x: f32, y: f32, color: Rgba) {
        self.text(s, x, y, 15.0, Weight::Regular, color, Align::Left);
    }

    fn section(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.scene.rounded(x, y, w, h, 6.0, BG, 1.0, SECTION);
    }

    /// `.b`: a touch button with an optional second line.
    fn button(
        &mut self,
        target: Target,
        (x, y, w, h): (f32, f32, f32, f32),
        label: &str,
        sub: Option<&str>,
        on: bool,
        size: f32,
    ) {
        let pressed = self.pressed == Some(target);
        let (fill, edge) = match (on, pressed) {
            (_, true) => (BUTTON_PRESSED, TEXT),
            (true, false) => (ACCENT_FILL, ACCENT),
            (false, false) => (BUTTON, BUTTON_EDGE),
        };
        self.scene.rounded(x, y, w, h, 4.0, fill, 1.0, edge);
        let block = size * 1.2 + sub.map_or(0.0, |_| 14.0 * 1.2);
        let top = y + (h - block) * 0.5;
        self.text(
            label,
            x + w * 0.5,
            top,
            size,
            Weight::SemiBold,
            TEXT,
            Align::Center,
        );
        if let Some(sub) = sub {
            self.text(
                sub,
                x + w * 0.5,
                top + size * 1.2,
                14.0,
                Weight::Regular,
                LABEL,
                Align::Center,
            );
        }
        self.hits.push(HitBox { target, x, y, w, h });
    }

    /// `.f`: an outlined chip (indicator, sometimes touchable).
    fn chip(
        &mut self,
        (x, y, w, h): (f32, f32, f32, f32),
        label: &str,
        size: f32,
        edge: Rgba,
        color: Rgba,
        fill: Rgba,
    ) {
        self.scene.rounded(x, y, w, h, 3.0, fill, 1.0, edge);
        self.text(
            label,
            x + w * 0.5,
            y + (h - size * 1.2) * 0.5,
            size,
            Weight::SemiBold,
            color,
            Align::Center,
        );
    }

    fn hit(&mut self, target: Target, (x, y, w, h): (f32, f32, f32, f32)) {
        self.hits.push(HitBox { target, x, y, w, h });
    }

    /// A label centred in a horizontal rule (`CUE/LOOP`, `CALL`).
    fn rule_label(&mut self, label: &str, x: f32, y: f32, w: f32) {
        // Barlow Condensed caps at 15 px are ~7 px wide; close enough to leave a gap.
        let text_w = label.len() as f32 * 7.0 + 12.0;
        let side = (w - text_w) * 0.5;
        let mid = y + 8.0;
        self.scene.fill(x, mid, side, 1.0, RAIL_EDGE);
        self.scene.fill(x + w - side, mid, side, 1.0, RAIL_EDGE);
        self.text(
            label,
            x + w * 0.5,
            y - 1.0,
            15.0,
            Weight::Regular,
            LABEL,
            Align::Center,
        );
    }
}

/// Lay out the screen into `scene`; returns the touch targets.
pub fn build(view: &View, scene: &mut Scene) -> Vec<HitBox> {
    scene.clear();
    let mut ui = Ui {
        scene,
        hits: Vec::new(),
        pressed: view.pressed,
    };

    // Frame: rail, tab row, info, status band, bottom strip, column dividers.
    ui.scene.fill(0.0, 0.0, 101.0, 600.0, BG);
    ui.scene.fill(100.0, 0.0, 1.0, 600.0, RAIL_EDGE);
    ui.section(106.0, 2.0, 915.0, 64.0);
    ui.section(106.0, 66.0, 915.0, 325.0);
    ui.section(106.0, 395.0, 915.0, 99.0);
    ui.section(106.0, 498.0, 915.0, 99.0);
    ui.scene.fill(822.0, 117.0, 1.0, 269.0, SECTION);
    ui.scene.fill(822.0, 400.0, 1.0, 89.0, SECTION);
    ui.scene.fill(822.0, 502.0, 1.0, 90.0, SECTION);

    rail(&mut ui, view);
    tabs(&mut ui);
    info(&mut ui, view);
    status(&mut ui, view);
    bottom(&mut ui, view);

    if let Some(line) = view.debug.filter(|l| !l.is_empty()) {
        ui.scene.rounded(
            113.0,
            176.0,
            420.0,
            22.0,
            3.0,
            Rgba::hex(0x000000).alpha(0.75),
            0.0,
            BG,
        );
        ui.text(
            line,
            119.0,
            178.0,
            15.0,
            Weight::Regular,
            AMBER_TEXT,
            Align::Left,
        );
    }
    ui.hits
}

fn rail(ui: &mut Ui, v: &View) {
    ui.button(
        Target::Library,
        (6.0, 16.0, 88.0, 117.0),
        "LIBRARY",
        None,
        false,
        20.0,
    );
    ui.button(
        Target::Link,
        (6.0, 138.0, 88.0, 124.0),
        "LINK",
        None,
        false,
        20.0,
    );
    ui.button(
        Target::Usb,
        (6.0, 267.0, 88.0, 117.0),
        "USB",
        None,
        true,
        20.0,
    );
    ui.scene
        .rounded(6.0, 410.0, 88.0, 83.0, 4.0, BUTTON, 1.0, BUTTON_EDGE);
    ui.text(
        "PLAYER",
        50.0,
        418.0,
        14.0,
        Weight::Regular,
        LABEL,
        Align::Center,
    );
    ui.digits("1", 50.0, 438.0, 39.0, TEXT, Align::Center);
    ui.button(
        Target::Slip,
        (6.0, 505.0, 88.0, 76.0),
        "SLIP",
        None,
        v.slip_enabled,
        20.0,
    );
    if v.slip_active {
        // Slipping: the shadow is running; amber edge like the reference's flashing button.
        ui.scene
            .rounded(6.0, 505.0, 88.0, 76.0, 4.0, Rgba::TRANSPARENT, 2.0, AMBER);
    }
}

fn tabs(ui: &mut Ui) {
    let y = 7.0;
    let h = 54.0;
    ui.button(
        Target::Browse,
        (113.0, y, 171.0, h),
        "BROWSE",
        Some("– SEARCH"),
        false,
        20.0,
    );
    ui.button(
        Target::TagList,
        (289.0, y, 171.0, h),
        "TAG LIST",
        None,
        false,
        20.0,
    );
    ui.button(
        Target::Info,
        (464.0, y, 171.0, h),
        "INFO",
        Some("– LINK INFO"),
        false,
        20.0,
    );
    ui.button(
        Target::Menu,
        (640.0, y, 171.0, h),
        "MENU",
        Some("– UTILITY"),
        false,
        20.0,
    );
    ui.button(
        Target::Perform,
        (841.0, y, 175.0, h),
        "PERFORM",
        None,
        false,
        20.0,
    );
}

fn info(ui: &mut Ui, v: &View) {
    // Title bar with the track colour on the left.
    ui.scene
        .rounded(113.0, 69.0, 903.0, 43.0, 4.0, TITLE_BAR, 1.0, BUTTON_EDGE);
    ui.scene
        .rounded(113.0, 69.0, 6.0, 43.0, 2.0, TITLE_ACCENT, 0.0, BG);
    ui.text(
        v.title,
        132.0,
        76.0,
        23.0,
        Weight::SemiBold,
        TEXT,
        Align::Left,
    );
    let key = v.key.map_or_else(
        || {
            if v.analysing {
                "KEY …".to_string()
            } else {
                "KEY –".to_string()
            }
        },
        |k| format!("KEY {k}"),
    );
    ui.text(key, 1003.0, 77.0, 22.0, Weight::SemiBold, KEY, Align::Right);

    // Master badge and phase meters: bar in the 4-bar phrase, beat in the bar.
    ui.text(
        "MASTER",
        262.0,
        117.0,
        13.0,
        Weight::Regular,
        AMBER_TEXT,
        Align::Right,
    );
    ui.text(
        "PLAYER",
        262.0,
        131.0,
        13.0,
        Weight::Regular,
        AMBER_TEXT,
        Align::Right,
    );
    ui.scene
        .rounded(267.0, 122.0, 17.0, 23.0, 3.0, AMBER, 0.0, BG);
    ui.digits("1", 275.5, 124.0, 19.0, BG, Align::Center);
    let (beat_in_bar, bar_in_phrase, bar, bars_left) = match v.grid {
        Some(g) => {
            let beat = g.beat_at(v.position).floor();
            let in_bar = g.bar_position(v.position).max(1);
            let bar_start = beat - f64::from(in_bar - 1);
            let bar = (bar_start / 4.0).floor();
            let left = ((g.beat_at(v.length) - g.beat_at(v.position)) / 4.0).max(0.0);
            (
                Some(in_bar),
                Some(bar.rem_euclid(4.0) as u8 + 1),
                Some((bar as i64 + 1, in_bar)),
                Some(left),
            )
        }
        None => (None, None, None, None),
    };
    meter(ui, 289.0, 123.0, bar_in_phrase, false);
    meter(ui, 289.0, 148.0, beat_in_bar, true);
    let (amber, blue) = match (bar, bars_left) {
        (Some((b, beat)), Some(left)) => (
            format!("{b:02}.{beat}"),
            format!(
                "{:02}.{}",
                left.floor() as i64,
                ((left.fract() * 4.0).floor() as i64) + 1
            ),
        ),
        _ => ("--.-".into(), "--.-".into()),
    };
    ui.digits(amber, 640.0, 118.0, 27.0, AMBER_TEXT, Align::Left);
    ui.text(
        "BARS",
        690.0,
        126.0,
        14.0,
        Weight::Regular,
        AMBER_TEXT,
        Align::Left,
    );
    ui.digits(blue, 640.0, 144.0, 27.0, ACCENT_TEXT, Align::Left);
    ui.text(
        "BARS LEFT",
        690.0,
        152.0,
        14.0,
        Weight::Regular,
        ACCENT_TEXT,
        Align::Left,
    );

    zoom(ui, v);

    // Right column: cue/loop memory and call.
    ui.rule_label("CUE/LOOP", 828.0, 159.0, 188.0);
    ui.button(
        Target::Delete,
        (828.0, 180.0, 88.0, 58.0),
        "DELETE",
        None,
        false,
        20.0,
    );
    ui.button(
        Target::Memory,
        (923.0, 180.0, 93.0, 58.0),
        "MEMORY",
        None,
        false,
        20.0,
    );
    ui.rule_label("CALL", 828.0, 250.0, 188.0);
    let (prev, next) = if v.looping {
        ("◀ ½X", "2X ▶")
    } else {
        ("◀", "▶")
    };
    ui.button(
        Target::CallPrev,
        (828.0, 272.0, 88.0, 62.0),
        prev,
        None,
        false,
        20.0,
    );
    ui.button(
        Target::CallNext,
        (923.0, 272.0, 93.0, 62.0),
        next,
        None,
        false,
        20.0,
    );
    let zoom_label = format!("ZOOM {}", v.zoom_beats);
    ui.chip(
        (828.0, 344.0, 88.0, 36.0),
        &zoom_label,
        16.0,
        CHIP_EDGE,
        TEXT,
        Rgba::hex(0x33363b),
    );
    ui.hit(Target::Zoom, (828.0, 344.0, 88.0, 36.0));
    ui.chip(
        (923.0, 344.0, 93.0, 36.0),
        "GRID",
        16.0,
        CHIP_EDGE,
        TEXT,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::Grid, (923.0, 344.0, 93.0, 36.0));
}

/// Four segments; `active` is 1-based. `filled`: the beat meter fills, the bar meter outlines.
fn meter(ui: &mut Ui, x: f32, y: f32, active: Option<u8>, filled: bool) {
    let (w, h, gap) = (338.0, 19.0, 5.0);
    let seg = (w - 3.0 * gap) / 4.0;
    for i in 0..4u8 {
        let sx = x + f32::from(i) * (seg + gap);
        let on = active == Some(i + 1);
        match (filled, on) {
            (true, true) => ui.scene.rounded(sx, y, seg, h, 3.0, ACCENT, 1.0, ACCENT),
            (true, false) => ui.scene.rounded(sx, y, seg, h, 3.0, BG, 1.0, ACCENT_DIM),
            (false, true) => ui.scene.rounded(sx, y, seg, h, 3.0, BG, 2.0, AMBER),
            (false, false) => ui.scene.rounded(sx, y, seg, h, 3.0, BG, 1.0, AMBER_DIM),
        }
        if i < 3 {
            let line = if filled { ACCENT_DIM } else { AMBER_DIM };
            ui.scene.fill(sx + seg, y + h * 0.5 - 1.0, gap, 2.0, line);
        }
    }
}

/// Zoomed view around the playhead: beat grid, loop, cue, hot cue strip, playhead.
fn zoom(ui: &mut Ui, v: &View) {
    let (x0, y0, w, h) = ZOOM;
    ui.scene
        .rounded(x0, y0, w, h, 2.0, Rgba::hex(0x07080a), 0.0, BG);
    ui.scene.fill(x0, y0 + h * 0.5, w, 1.0, Rgba::hex(0x15171b));
    // Frames per design pixel.
    let span = match v.grid {
        Some(g) => v.zoom_beats * g.period(),
        None => v.zoom_beats * 0.5 * v.sample_rate, // 120 BPM until the grid arrives
    };
    let fpp = span / f64::from(w);
    let left = v.position - span * 0.5;
    let to_x = |pos: f64| x0 + ((pos - left) / fpp) as f32;
    let visible = |x: f32| x >= x0 && x <= x0 + w;

    if v.looping || v.loop_end > v.loop_start {
        let (a, b) = (to_x(v.loop_start).max(x0), to_x(v.loop_end).min(x0 + w));
        if b > a {
            let (fill, edge) = if v.looping {
                (AMBER.alpha(0.16), AMBER)
            } else {
                (AMBER.alpha(0.05), AMBER_DIM)
            };
            ui.scene.fill(a, y0, b - a, h, fill);
            for e in [to_x(v.loop_start), to_x(v.loop_end)] {
                if visible(e) {
                    ui.scene.fill(e - 1.0, y0, 2.0, h, edge);
                }
            }
        }
    }
    if let Some(g) = v.grid {
        let first = g.beat_at(left).ceil() as i64;
        let last = g.beat_at(left + span).floor() as i64;
        for b in first..=last.min(first + 256) {
            let pos = g.position_of(b as f64);
            let x = to_x(pos);
            if g.bar_position(pos + 0.5) == 1 {
                ui.scene.fill(x - 0.75, y0, 1.5, h, BAR_LINE);
            } else {
                ui.scene.fill(x - 0.5, y0 + 18.0, 1.0, h - 36.0, BEAT_LINE);
            }
        }
    }
    let cue = to_x(v.cue);
    if visible(cue) {
        ui.scene.fill(cue - 1.0, y0, 2.0, h, AMBER);
        ui.scene
            .rounded(cue - 6.0, y0, 12.0, 8.0, 2.0, AMBER, 0.0, BG);
    }
    if v.slip_active {
        let s = to_x(v.shadow_position);
        if visible(s) {
            ui.scene.fill(s - 1.0, y0, 2.0, h, ACCENT.alpha(0.8));
        }
    }
    let label = match (v.grid, v.analysing) {
        (Some(_), _) => "BEAT GRID · WAVEFORM PENDING",
        (None, true) => "ANALYSING…",
        (None, false) => "NO GRID",
    };
    ui.text(
        label,
        x0 + 8.0,
        y0 + h - 24.0,
        14.0,
        Weight::Regular,
        DIM,
        Align::Left,
    );
    // Playhead, centred.
    let px = x0 + w * 0.5;
    ui.scene.fill(px - 1.0, y0, 2.0, h, TEXT);
    ui.scene
        .rounded(px - 5.0, y0, 10.0, 6.0, 1.0, TEXT, 0.0, BG);
}

fn clock(seconds: f64) -> (String, String) {
    let s = seconds.max(0.0);
    let m = (s / 60.0).floor();
    let rest = s - m * 60.0;
    (
        format!("{:02}:{:02}", m as u64, rest.floor() as u64),
        format!(".{:03}", ((rest.fract()) * 1000.0).floor() as u64),
    )
}

fn status(ui: &mut Ui, v: &View) {
    ui.label("TRACK", 113.0, 398.0, LABEL);
    ui.digits("01", 113.0, 422.0, 56.0, TEXT, Align::Left);
    let hot = v.hot_cues;
    let pads: String = (0..8u8)
        .map(|i| {
            if hot & (1 << i) != 0 {
                (b'A' + i) as char
            } else {
                '·'
            }
        })
        .collect();
    ui.chip(
        (226.0, 422.0, 113.0, 25.0),
        &pads,
        16.0,
        if hot != 0 { HOT_CUE } else { CHIP_EDGE },
        TEXT,
        Rgba::TRANSPARENT,
    );
    ui.chip(
        (226.0, 453.0, 113.0, 25.0),
        "A.CUE",
        16.0,
        AMBER,
        AMBER_TEXT,
        Rgba::TRANSPARENT,
    );

    ui.label("REMAIN", 351.0, 398.0, LABEL);
    let quantize = match v.quantize {
        q if q <= 0.0 => "QUANTIZE : OFF".to_string(),
        q if q >= 1.0 => format!("QUANTIZE : {q}"),
        q => format!("QUANTIZE : 1/{}", (1.0 / q).round()),
    };
    ui.label(
        &quantize,
        424.0,
        398.0,
        if v.quantize > 0.0 { AMBER_TEXT } else { DIM },
    );
    let (mmss, ms) = clock((v.length - v.position) / v.sample_rate);
    // Tabular digits: "00:00" at 56 px ends ~118 px in.
    ui.digits(mmss, 469.0, 422.0, 56.0, TEXT, Align::Right);
    ui.digits(ms, 470.0, 437.0, 37.0, TEXT, Align::Left);

    ui.label("TEMPO", 653.0, 398.0, LABEL);
    let (edge, color) = if v.keylock {
        (AMBER, AMBER_TEXT)
    } else {
        (CHIP_EDGE, DIM)
    };
    ui.chip(
        (705.0, 397.0, 64.0, 23.0),
        "KEYLOCK",
        15.0,
        edge,
        color,
        Rgba::TRANSPARENT,
    );
    let pct = (v.rate - 1.0) * 100.0;
    let tempo = if pct.abs() < 0.005 {
        "0.00".to_string()
    } else if pct > 0.0 {
        format!("+{pct:.2}")
    } else {
        format!("−{:.2}", -pct)
    };
    ui.digits(tempo, 790.0, 422.0, 56.0, TEXT, Align::Right);
    ui.digits("%", 792.0, 448.0, 22.0, TEXT, Align::Left);

    ui.button(
        Target::Sync,
        (828.0, 408.0, 88.0, 78.0),
        "SYNC",
        Some("– INST.D."),
        false,
        20.0,
    );
    ui.button(
        Target::Master,
        (923.0, 408.0, 93.0, 78.0),
        "MASTER",
        None,
        true,
        20.0,
    );
}

fn bottom(ui: &mut Ui, v: &View) {
    let (x0, y0, w, h) = OVERVIEW;
    ui.scene
        .rounded(x0, y0, w, h, 2.0, Rgba::hex(0x07080a), 0.0, BG);
    if v.length > 0.0 {
        let to_x = |pos: f64| x0 + (pos / v.length).clamp(0.0, 1.0) as f32 * w;
        if v.loaded_fraction < 1.0 {
            let lx = to_x(v.loaded_fraction * v.length);
            ui.scene
                .fill(lx, y0 + h - 3.0, x0 + w - lx, 3.0, DIM.alpha(0.5));
        }
        // 16-bar ticks.
        if let Some(g) = v.grid {
            let beats = g.beat_at(v.length).floor() as i64;
            let mut b = 0;
            while b <= beats {
                let x = to_x(g.position_of(b as f64));
                ui.scene.fill(x - 0.5, y0 + h - 10.0, 1.0, 10.0, BAR_LINE);
                b += 64;
            }
        }
        let px = to_x(v.position);
        ui.scene
            .fill(x0, y0 + 8.0, px - x0, h - 16.0, ACCENT.alpha(0.18));
        if v.loop_end > v.loop_start {
            let (a, b) = (to_x(v.loop_start), to_x(v.loop_end));
            ui.scene.fill(
                a,
                y0,
                (b - a).max(2.0),
                h,
                if v.looping {
                    AMBER.alpha(0.45)
                } else {
                    AMBER.alpha(0.2)
                },
            );
        }
        let cx = to_x(v.cue);
        ui.scene.fill(cx - 1.0, y0, 2.0, h, AMBER);
        ui.scene.fill(px - 1.0, y0, 2.0, h, TEXT);
    }
    ui.text(
        "OVERVIEW STAND-IN · TOUCH TO SEEK",
        x0 + 8.0,
        y0 + 4.0,
        14.0,
        Weight::Regular,
        DIM,
        Align::Left,
    );
    ui.hit(Target::Overview, OVERVIEW);
    ui.scene.fill(x0, 572.0, w, 1.0, BUTTON_EDGE);
    ui.text(
        "NEEDLE SEARCH",
        x0 + w * 0.5,
        574.0,
        14.0,
        Weight::Regular,
        LABEL,
        Align::Center,
    );

    ui.chip(
        (828.0, 502.0, 62.0, 39.0),
        "±10",
        22.0,
        AMBER,
        AMBER_TEXT,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::TempoRange, (828.0, 502.0, 62.0, 39.0));
    ui.chip(
        (828.0, 545.0, 62.0, 43.0),
        "[4]",
        22.0,
        CHIP_EDGE,
        TEXT,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::JumpSize, (828.0, 545.0, 62.0, 43.0));
    ui.scene
        .rounded(898.0, 502.0, 118.0, 86.0, 4.0, BUTTON, 1.0, ACCENT);
    let bpm = v
        .grid
        .map_or("--.-".to_string(), |g| format!("{:.1}", g.bpm() * v.rate));
    ui.digits(bpm, 1006.0, 514.0, 41.0, ACCENT_TEXT, Align::Right);
    ui.text(
        "BPM",
        1006.0,
        558.0,
        14.0,
        Weight::Regular,
        LABEL,
        Align::Right,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(grid: Option<&DeckGrid>) -> View<'_> {
        View {
            title: "Test",
            key: Some("Em"),
            sample_rate: 48_000.0,
            position: 1_000_000.0,
            shadow_position: 0.0,
            length: 48_000.0 * 300.0,
            loaded_fraction: 1.0,
            rate: 1.0,
            slip_enabled: false,
            slip_active: false,
            keylock: true,
            quantize: 1.0,
            cue: 900_000.0,
            hot_cues: 0b101,
            looping: true,
            loop_start: 990_000.0,
            loop_end: 1_090_000.0,
            grid,
            analysing: false,
            zoom_beats: 16.0,
            pressed: None,
            debug: None,
        }
    }

    #[test]
    fn everything_stays_on_the_panel_and_targets_do_not_overlap() {
        let grid = DeckGrid::constant(1_000.0, 123.0, 48_000.0 * 300.0, 48_000).unwrap();
        let mut scene = Scene::default();
        let hits = build(&view(Some(&grid)), &mut scene);
        for r in &scene.rects {
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
                "{r:?}"
            );
        }
        for (i, a) in hits.iter().enumerate() {
            for b in &hits[i + 1..] {
                let overlap =
                    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                assert!(!overlap, "{:?} overlaps {:?}", a.target, b.target);
            }
        }
        // Touch targets the design pass sized for a 7" panel: at least 36 px on each side.
        for h in hits.iter().filter(|h| h.target != Target::Overview) {
            assert!(
                h.w >= 36.0 && h.h >= 36.0,
                "{:?} is {}×{}",
                h.target,
                h.w,
                h.h
            );
        }
    }

    #[test]
    fn builds_without_a_grid() {
        let mut scene = Scene::default();
        build(&view(None), &mut scene);
        assert!(scene.texts.iter().any(|t| t.text == "--.-"));
    }
}
