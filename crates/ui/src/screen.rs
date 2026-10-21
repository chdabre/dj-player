//! The main playback screen, laid out after the 1024×600 wireframe ("1 · Normal playback",
//! canvas version 28). Pure function of a [`View`]: it fills a [`Scene`] and returns the touch
//! targets, so the layout can be tested and screenshotted without a device.
//!
//! The waveforms come from the deck's computed waveform (`waves.rs`), drawn between their
//! panel and grid and the overlays (loop, cue, slip shadow, playhead), as the waveform crate's
//! reference renderer orders them. Until the first entries exist, the views show the grid and
//! markers alone.

use engine_core::DeckGrid;

use crate::kit::{self, Rect};
use crate::scene::{Gradient, Rgba, Scene};
use crate::theme::Theme;
use crate::waves::{OverviewCache, WaveDraw, WaveSettings};

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
    /// The zoom waveform: touching it takes the track, dragging scrubs it.
    Waveform,
    // Performance screen.
    HotCueMode,
    BeatJumpMode,
    DeleteCall,
    Bank,
    /// One of the four pads (0..4, left to right).
    Pad(u8),
    /// One of the beat loop lengths ([`crate::perform::BEAT_LOOPS`] index).
    BeatLoop(u8),
    // Browse screen.
    /// A shown row of the list (0 = top).
    Row(u8),
    /// The list header: up a level.
    BrowseBack,
    /// Header chips: the sort order, search, scan the stick.
    Sort,
    Search,
    Scan,
    /// ANALYZE ALL (or STOP while it runs).
    AnalyzeAll,
    /// The "scan this stick?" question's answers.
    ScanYes,
    ScanNo,
    /// A key of the on-screen keyboard ([`crate::browse::KEYS`] index).
    Key(u8),
    /// A row of a modal over the browse list (its index among all the modal's rows).
    ModalRow(u16),
}

/// Which screen is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenKind {
    #[default]
    Main,
    Performance,
    Browse,
    /// INFO: the loaded track's details.
    Info,
}

/// What the performance pads do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadMode {
    #[default]
    HotCue,
    BeatJump,
}

/// The performance screen's own state (the UI's, not the deck's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfState {
    pub mode: PadMode,
    /// Hot cue bank: pads show cues `4 * bank ..`.
    pub bank: usize,
    /// Beat jump bank: false = 1 and 4 beats, true = 16 and 32.
    pub jump_bank: bool,
    /// DELETE armed: the next pad taps delete hot cues.
    pub delete_armed: bool,
}

/// A touch target on the device screen.
pub type HitBox = kit::HitBox<Target>;

/// The overview's bounds, for turning a touch into a track position.
pub const OVERVIEW: Rect = Rect::new(113.0, 502.0, 703.0, 67.0);
const ZOOM: Rect = Rect::new(113.0, 172.0, 703.0, 211.0);

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
    /// The track's waveform so far, if any.
    pub wave: Option<Wave<'a>>,
    /// Hot cue positions (frames), NaN where empty.
    pub hot_cue_positions: [f64; engine_core::HOT_CUES],
    pub screen: ScreenKind,
    pub perf: PerfState,
    /// Whether the library is saving edits; `None` until it says.
    pub save: Option<SaveState>,
    /// The browse screen's list, when there is something to browse.
    pub browse: Option<&'a crate::browse::BrowseView>,
}

/// What the status band says about the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    /// Every edit is on the stick.
    Saved,
    /// Written, flushed within a second or two.
    Saving,
    /// Edits are in RAM only and lost at power-off (no library, a locked or failing stick).
    Unsaved,
}

impl SaveState {
    fn label(self) -> &'static str {
        match self {
            SaveState::Saved => "SAVED",
            SaveState::Saving => "SAVING",
            SaveState::Unsaved => "UNSAVED",
        }
    }
}

/// A deck's waveform as the screen draws it.
#[derive(Clone, Copy)]
pub struct Wave<'a> {
    pub entries: &'a [waveform::compute::Entry],
    pub entries_per_second: f64,
    pub settings: WaveSettings,
    /// The theme's waveform style with this track's gain set.
    pub style: waveform::display::Style,
}

/// The toolkit, drawing for the device screen's targets.
pub(crate) type Ui<'s> = kit::Ui<'s, Target>;

/// Lay out the screen into `scene`; returns the touch targets.
pub fn build(
    view: &View,
    theme: &Theme,
    overview: &mut OverviewCache,
    scene: &mut Scene,
) -> Vec<HitBox> {
    scene.clear();
    scene.background = theme.color.background;
    let mut ui = Ui {
        t: theme,
        scene,
        hits: Vec::new(),
        pressed: view.pressed,
    };

    let t = theme;
    if view.screen == ScreenKind::Performance {
        crate::perform::build(&mut ui, view, overview);
        debug_line(&mut ui, view);
        return ui.hits;
    }
    if matches!(view.screen, ScreenKind::Browse | ScreenKind::Info)
        && let Some(b) = view.browse
    {
        // No timing overlay here: it would cover the list.
        crate::browse::build(&mut ui, view, b, overview);
        return ui.hits;
    }
    // Frame: rail, tab row, info, status band, bottom strip, column dividers.
    ui.scene.fill(0.0, 0.0, 101.0, 600.0, t.color.background);
    ui.scene.fill(100.0, 0.0, 1.0, 600.0, t.color.rule);
    ui.section(Rect::new(106.0, 2.0, 915.0, 64.0));
    ui.section(Rect::new(106.0, 66.0, 915.0, 325.0));
    ui.section(Rect::new(106.0, 395.0, 915.0, 99.0));
    ui.section(Rect::new(106.0, 498.0, 915.0, 99.0));
    ui.scene
        .fill(822.0, 117.0, 1.0, 269.0, t.color.section_edge);
    ui.scene.fill(822.0, 400.0, 1.0, 89.0, t.color.section_edge);
    ui.scene.fill(822.0, 502.0, 1.0, 90.0, t.color.section_edge);

    rail(&mut ui, view, Target::Usb);
    tabs(&mut ui, None);
    info(&mut ui, view);
    status(&mut ui, view);
    bottom(&mut ui, view, overview);
    debug_line(&mut ui, view);
    ui.hits
}

/// The colour of hot cue `i` (A = 0), until the library gives cues their own.
pub(crate) fn hot_cue_color(t: &Theme, i: usize) -> Rgba {
    t.color.hot_cue_colors[i % t.color.hot_cue_colors.len()]
}

/// Frames across the zoom view: `zoom_beats` beats of the grid (120 BPM until there is one).
pub fn zoom_span(grid: Option<&DeckGrid>, zoom_beats: f64, sample_rate: f64) -> f64 {
    match grid {
        Some(g) => zoom_beats * g.period(),
        None => zoom_beats * 0.5 * sample_rate,
    }
}

/// Hot cue `i`'s pad letter: A, B, …
pub(crate) fn hot_cue_letter(i: usize) -> char {
    (b'A' + (i % 26) as u8) as char
}

/// The frame timing overlay, if on.
fn debug_line(ui: &mut Ui, view: &View) {
    let t = ui.t;
    if let Some(line) = view.debug.filter(|l| !l.is_empty()) {
        ui.scene.rounded(
            113.0,
            176.0,
            420.0,
            22.0,
            t.shape.chip_radius,
            t.color.overlay,
            0.0,
            t.color.background,
        );
        ui.text(line, 119.0, 178.0, t.text.label.color(t.color.cue_text));
    }
}

/// The rail with the source `lit` (LIBRARY or USB) on.
pub(crate) fn rail(ui: &mut Ui, v: &View, lit: Target) {
    ui.button(
        Target::Library,
        Rect::new(6.0, 16.0, 88.0, 117.0),
        "LIBRARY",
        None,
        lit == Target::Library,
    );
    ui.button(
        Target::Link,
        Rect::new(6.0, 138.0, 88.0, 124.0),
        "LINK",
        None,
        false,
    );
    ui.button(
        Target::Usb,
        Rect::new(6.0, 267.0, 88.0, 117.0),
        "USB",
        None,
        lit == Target::Usb,
    );
    rail_bottom(ui, v);
}

/// The rail's player number and SLIP, the same on every screen.
pub(crate) fn rail_bottom(ui: &mut Ui, v: &View) {
    let t = ui.t;
    ui.scene.rounded(
        6.0,
        410.0,
        88.0,
        83.0,
        t.shape.button_radius,
        t.color.button,
        t.shape.border,
        t.color.button_edge,
    );
    ui.text("PLAYER", 50.0, 418.0, t.text.small.center());
    ui.text("1", 50.0, 438.0, t.text.player.center());
    ui.button(
        Target::Slip,
        Rect::new(6.0, 505.0, 88.0, 76.0),
        "SLIP",
        None,
        v.slip_enabled,
    );
    if v.slip_active {
        // Slipping: the shadow is running; amber edge like the reference's flashing button.
        ui.scene.rounded(
            6.0,
            505.0,
            88.0,
            76.0,
            t.shape.button_radius,
            Rgba::TRANSPARENT,
            t.shape.border_active,
            t.color.cue,
        );
    }
}

/// The tab row, with `active` (a tab's target) lit.
pub(crate) fn tabs(ui: &mut Ui, active: Option<Target>) {
    let y = 7.0;
    let h = 54.0;
    ui.button(
        Target::Browse,
        Rect::new(113.0, y, 171.0, h),
        "BROWSE",
        Some("– SEARCH"),
        active == Some(Target::Browse),
    );
    ui.button(
        Target::TagList,
        Rect::new(289.0, y, 171.0, h),
        "TAG LIST",
        None,
        active == Some(Target::TagList),
    );
    ui.button(
        Target::Info,
        Rect::new(464.0, y, 171.0, h),
        "INFO",
        Some("– LINK INFO"),
        active == Some(Target::Info),
    );
    ui.button(
        Target::Menu,
        Rect::new(640.0, y, 171.0, h),
        "MENU",
        Some("– UTILITY"),
        active == Some(Target::Menu),
    );
    ui.button(
        Target::Perform,
        Rect::new(841.0, y, 175.0, h),
        "PERFORM",
        None,
        active == Some(Target::Perform),
    );
}

fn info(ui: &mut Ui, v: &View) {
    let t = ui.t;
    // Title bar with the track colour on the left.
    ui.scene.rounded(
        113.0,
        69.0,
        903.0,
        43.0,
        t.shape.button_radius,
        t.color.title_bar,
        t.shape.border,
        t.color.button_edge,
    );
    ui.scene.rounded(
        113.0,
        69.0,
        6.0,
        43.0,
        t.shape.panel_radius,
        t.color.track_color,
        0.0,
        t.color.background,
    );
    ui.text(v.title, 132.0, 76.0, t.text.title);
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
    ui.text(key, 1003.0, 77.0, t.text.title_aside.right());

    phase(ui, v, 289.0, 338.0, 123.0);

    zoom(ui, v, ZOOM);

    // Right column: cue/loop memory and call.
    cue_column(ui, v, 159.0, 58.0, 62.0);
    let zoom_label = format!("ZOOM {}", v.zoom_beats);
    ui.chip(
        Rect::new(828.0, 344.0, 88.0, 36.0),
        &zoom_label,
        t.text.chip,
        t.color.chip_edge,
        t.color.chip_selected,
    );
    ui.hit(Target::Zoom, Rect::new(828.0, 344.0, 88.0, 36.0));
    ui.chip(
        Rect::new(923.0, 344.0, 93.0, 36.0),
        "GRID",
        t.text.chip,
        t.color.chip_edge,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::Grid, Rect::new(923.0, 344.0, 93.0, 36.0));
}

/// The master badge, the two phase meters (bar in the 4-bar phrase, beat in the bar) and the
/// bar counters, with the meters at `mx` (width `mw`) and the first meter's top at `y`.
pub(crate) fn phase(ui: &mut Ui, v: &View, mx: f32, mw: f32, y: f32) {
    let t = ui.t;
    let (lx, bx, cx) = (mx - 27.0, mx - 22.0, mx + mw + 13.0);
    ui.text(
        "MASTER",
        lx,
        y - 6.0,
        t.text.caption.color(t.color.cue_text).right(),
    );
    ui.text(
        "PLAYER",
        lx,
        y + 8.0,
        t.text.caption.color(t.color.cue_text).right(),
    );
    ui.scene.rounded(
        bx,
        y - 1.0,
        17.0,
        23.0,
        t.shape.chip_radius,
        t.color.cue,
        0.0,
        t.color.background,
    );
    ui.text(
        "1",
        bx + 8.5,
        y + 1.0,
        t.text.badge.color(t.color.on_badge).center(),
    );
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
    meter(ui, mx, y, mw, bar_in_phrase, false);
    meter(ui, mx, y + 25.0, mw, beat_in_bar, true);
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
    ui.text(amber, cx, y - 5.0, t.text.counter.color(t.color.cue_text));
    ui.text(
        "BARS",
        cx + 50.0,
        y + 3.0,
        t.text.small.color(t.color.cue_text),
    );
    ui.text(
        blue,
        cx,
        y + 21.0,
        t.text.counter.color(t.color.accent_text),
    );
    ui.text(
        "BARS LEFT",
        cx + 50.0,
        y + 29.0,
        t.text.small.color(t.color.accent_text),
    );
}

/// The right column's CUE/LOOP (DELETE, MEMORY) and CALL (◀ ▶) groups: rule labels at `rule`
/// and above the call buttons, buttons `button_h` and `call_h` high.
pub(crate) fn cue_column(ui: &mut Ui, v: &View, rule: f32, button_h: f32, call_h: f32) {
    let button_y = rule + 21.0;
    let call_y = rule + 113.0 + (button_h - 58.0);
    ui.rule_label("CUE/LOOP", 828.0, rule, 188.0);
    ui.button(
        Target::Delete,
        Rect::new(828.0, button_y, 88.0, button_h),
        "DELETE",
        None,
        false,
    );
    ui.button(
        Target::Memory,
        Rect::new(923.0, button_y, 93.0, button_h),
        "MEMORY",
        None,
        false,
    );
    ui.rule_label("CALL", 828.0, rule + 91.0 + button_h - 58.0, 188.0);
    let (prev, next) = if v.looping {
        ("◀ ½X", "2X ▶")
    } else {
        ("◀", "▶")
    };
    ui.button(
        Target::CallPrev,
        Rect::new(828.0, call_y, 88.0, call_h),
        prev,
        None,
        false,
    );
    ui.button(
        Target::CallNext,
        Rect::new(923.0, call_y, 93.0, call_h),
        next,
        None,
        false,
    );
}

/// Four segments; `active` is 1-based. `filled`: the beat meter fills, the bar meter outlines.
pub(crate) fn meter(ui: &mut Ui, x: f32, y: f32, w: f32, active: Option<u8>, filled: bool) {
    let t = ui.t;
    let (h, gap) = (19.0, 5.0);
    let seg = (w - 3.0 * gap) / 4.0;
    for i in 0..4u8 {
        let sx = x + f32::from(i) * (seg + gap);
        let on = active == Some(i + 1);
        let (fill, border, edge) = match (filled, on) {
            (true, true) => (t.color.accent, t.shape.border, t.color.accent),
            (true, false) => (t.color.background, t.shape.border, t.color.accent_dim),
            (false, true) => (t.color.background, t.shape.border_active, t.color.cue),
            (false, false) => (t.color.background, t.shape.border, t.color.cue_dim),
        };
        ui.scene
            .rounded(sx, y, seg, h, t.shape.chip_radius, fill, border, edge);
        if i < 3 {
            let line = if filled {
                t.color.accent_dim
            } else {
                t.color.cue_dim
            };
            ui.scene.fill(sx + seg, y + h * 0.5 - 1.0, gap, 2.0, line);
        }
    }
}

/// The panel behind a waveform: a gradient towards the waveform's baseline (the centre line
/// for a mirrored view, the bottom for a bottom-aligned one).
pub(crate) fn panel(ui: &mut Ui, Rect { x, y, w, h }: Rect, align: Option<crate::waves::Align>) {
    let t = ui.t;
    let wf = &t.waveform;
    let kind = match align {
        Some(crate::waves::Align::Bottom) => Gradient::Down,
        _ => Gradient::Mirrored,
    };
    ui.scene.gradient(
        x,
        y,
        w,
        h,
        t.shape.panel_radius,
        wf.background,
        wf.background_glow,
        kind,
    );
}

/// Zoomed view around the playhead: beat grid, loop, cue, hot cue strip, playhead.
pub(crate) fn zoom(ui: &mut Ui, v: &View, area: Rect) {
    let t = ui.t;
    let wf = &t.waveform;
    let Rect { x: x0, y: y0, w, h } = area;
    panel(ui, area, v.wave.map(|w| w.settings.zoom_align));
    ui.scene.fill(x0, y0 + h * 0.5, w, 1.0, wf.center_line);
    ui.hit(Target::Waveform, area);
    // Frames per design pixel.
    let span = zoom_span(v.grid, v.zoom_beats, v.sample_rate);
    let fpp = span / f64::from(w);
    let left = v.position - span * 0.5;
    let to_x = |pos: f64| x0 + ((pos - left) / fpp) as f32;
    let visible = |x: f32| x >= x0 && x <= x0 + w;

    if let Some(wave) = &v.wave {
        let draw = crate::waves::zoom(
            wave.entries,
            wave.entries_per_second,
            v.position / v.sample_rate,
            fpp / v.sample_rate,
            area,
            &wave.settings,
            &wave.style,
        );
        ui.scene.waveform(WaveDraw {
            opacity: wf.opacity,
            ..draw
        });
    }
    if v.looping || v.loop_end > v.loop_start {
        let (a, b) = (to_x(v.loop_start).max(x0), to_x(v.loop_end).min(x0 + w));
        if b > a {
            let (fill, edge) = if v.looping {
                (wf.loop_color.alpha(wf.loop_fill), wf.loop_color)
            } else {
                (
                    wf.loop_color.alpha(wf.loop_fill_stored),
                    wf.loop_edge_stored,
                )
            };
            ui.scene.fill(a, y0, b - a, h, fill);
            for e in [to_x(v.loop_start), to_x(v.loop_end)] {
                if visible(e) {
                    ui.scene.fill(e - 1.0, y0, 2.0, h, edge);
                }
            }
        }
    }
    // Beat grid: ticks along the top and bottom edges, over the waveform.
    if let Some(g) = v.grid {
        let first = g.beat_at(left).ceil() as i64;
        let last = g.beat_at(left + span).floor() as i64;
        for b in first..=last.min(first + 256) {
            let pos = g.position_of(b as f64);
            let x = to_x(pos);
            let (color, width) = if g.bar_position(pos + 0.5) == 1 {
                (wf.downbeat_line, wf.downbeat_tick_width)
            } else {
                (wf.beat_line, wf.tick_width)
            };
            let len = wf.tick_length;
            ui.scene.fill(x - width * 0.5, y0, width, len, color);
            ui.scene
                .fill(x - width * 0.5, y0 + h - len, width, len, color);
        }
    }
    // Hot cues: a line in the pad's colour with its letter in a tag at the bottom.
    for (i, &at) in v.hot_cue_positions.iter().enumerate() {
        let x = to_x(at);
        if !at.is_finite() || !visible(x) {
            continue;
        }
        let c = hot_cue_color(t, i);
        ui.scene.fill(x - 1.0, y0, 2.0, h, c);
        let tag = t.font.caption + 4.0;
        ui.scene.rounded(
            x - 1.0,
            y0 + h - tag,
            tag,
            tag,
            t.shape.panel_radius,
            c,
            0.0,
            c,
        );
        ui.text(
            hot_cue_letter(i).to_string(),
            x - 1.0 + tag * 0.5,
            y0 + h - tag + 1.0,
            t.text.caption.semibold().color(t.color.on_badge).center(),
        );
    }
    let cue = to_x(v.cue);
    if visible(cue) {
        ui.scene.fill(cue - 1.0, y0, 2.0, h, wf.cue);
        ui.scene.rounded(
            cue - 6.0,
            y0,
            12.0,
            8.0,
            t.shape.panel_radius,
            wf.cue,
            0.0,
            wf.cue,
        );
    }
    if v.slip_active {
        let s = to_x(v.shadow_position);
        if visible(s) {
            ui.scene.fill(s - 1.0, y0, 2.0, h, wf.slip_shadow);
        }
    }
    let label = match (v.grid, v.analysing) {
        (Some(_), _) => "",
        (None, true) => "ANALYSING…",
        (None, false) => "NO GRID",
    };
    ui.text(
        label,
        x0 + 8.0,
        y0 + h - 24.0,
        t.text.small.color(t.color.dim),
    );
    // Playhead, centred.
    let px = x0 + w * 0.5;
    ui.scene.fill(px - 1.0, y0, 2.0, h, wf.playhead);
    ui.scene
        .rounded(px - 5.0, y0, 10.0, 6.0, 1.0, wf.playhead, 0.0, wf.playhead);
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

pub(crate) fn status(ui: &mut Ui, v: &View) {
    let t = ui.t;
    ui.text("TRACK", 113.0, 398.0, t.text.label);
    if let Some(save) = v.save {
        let color = match save {
            SaveState::Saved => t.color.dim,
            SaveState::Saving => t.color.label,
            SaveState::Unsaved => t.color.danger_text,
        };
        ui.text(save.label(), 160.0, 398.0, t.text.label.color(color));
    }
    ui.text("01", 113.0, 422.0, t.text.readout);
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
        Rect::new(226.0, 422.0, 113.0, 25.0),
        &pads,
        t.text.chip,
        if hot != 0 {
            t.color.hot_cue
        } else {
            t.color.chip_edge
        },
        Rgba::TRANSPARENT,
    );
    ui.chip(
        Rect::new(226.0, 453.0, 113.0, 25.0),
        "A.CUE",
        t.text.chip.color(t.color.cue_text),
        t.color.cue,
        Rgba::TRANSPARENT,
    );

    ui.text("REMAIN", 351.0, 398.0, t.text.label);
    let quantize = match v.quantize {
        q if q <= 0.0 => "QUANTIZE : OFF".to_string(),
        q if q >= 1.0 => format!("QUANTIZE : {q}"),
        q => format!("QUANTIZE : 1/{}", (1.0 / q).round()),
    };
    ui.text(
        &quantize,
        424.0,
        398.0,
        t.text.label.color(if v.quantize > 0.0 {
            t.color.cue_text
        } else {
            t.color.dim
        }),
    );
    let (mmss, ms) = clock((v.length - v.position) / v.sample_rate);
    // Tabular digits: "00:00" at 56 px ends ~118 px in.
    ui.text(mmss, 469.0, 422.0, t.text.readout.right());
    ui.text(ms, 470.0, 437.0, t.text.readout_fraction);

    ui.text("TEMPO", 653.0, 398.0, t.text.label);
    let (edge, color) = if v.keylock {
        (t.color.cue, t.color.cue_text)
    } else {
        (t.color.chip_edge, t.color.dim)
    };
    ui.chip(
        Rect::new(705.0, 397.0, 64.0, 23.0),
        "KEYLOCK",
        t.text.label.semibold().color(color),
        edge,
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
    ui.text(tempo, 790.0, 422.0, t.text.readout.right());
    ui.text("%", 792.0, 448.0, t.text.unit);

    ui.button(
        Target::Sync,
        Rect::new(828.0, 408.0, 88.0, 78.0),
        "SYNC",
        Some("– INST.D."),
        false,
    );
    ui.button(
        Target::Master,
        Rect::new(923.0, 408.0, 93.0, 78.0),
        "MASTER",
        None,
        true,
    );
}

pub(crate) fn bottom(ui: &mut Ui, v: &View, overview: &mut OverviewCache) {
    let t = ui.t;
    let wf = &t.waveform;
    let Rect { x: x0, y: y0, w, h } = OVERVIEW;
    panel(ui, OVERVIEW, v.wave.map(|w| w.settings.overview_align));
    if v.length > 0.0 {
        let to_x = |pos: f64| x0 + (pos / v.length).clamp(0.0, 1.0) as f32 * w;
        if let Some(wave) = &v.wave {
            let draw = overview.draw(
                wave.entries,
                wave.entries_per_second,
                v.length / v.sample_rate,
                v.position / v.sample_rate,
                OVERVIEW,
                &wave.settings,
                &wave.style,
            );
            ui.scene.waveform(WaveDraw {
                opacity: wf.opacity,
                ..draw
            });
        }
        if v.loaded_fraction < 1.0 {
            let lx = to_x(v.loaded_fraction * v.length);
            ui.scene
                .fill(lx, y0 + h - 3.0, x0 + w - lx, 3.0, wf.loading);
        }
        // 16-bar ticks.
        if let Some(g) = v.grid {
            let beats = g.beat_at(v.length).floor() as i64;
            let mut b = 0;
            while b <= beats {
                let x = to_x(g.position_of(b as f64));
                ui.scene
                    .fill(x - 0.5, y0 + h - 10.0, 1.0, 10.0, wf.bar_line);
                b += 64;
            }
        }
        let px = to_x(v.position);
        if v.wave.is_none() {
            // Without a waveform the played part is shaded; with one it is dimmed.
            ui.scene.fill(x0, y0 + 8.0, px - x0, h - 16.0, wf.played);
        }
        if v.loop_end > v.loop_start {
            let (a, b) = (to_x(v.loop_start), to_x(v.loop_end));
            ui.scene.fill(
                a,
                y0,
                (b - a).max(2.0),
                h,
                wf.loop_color.alpha(if v.looping {
                    wf.overview_loop_fill
                } else {
                    wf.overview_loop_fill_stored
                }),
            );
        }
        for (i, &at) in v.hot_cue_positions.iter().enumerate() {
            if at.is_finite() {
                let c = hot_cue_color(t, i);
                ui.scene
                    .rounded(to_x(at) - 2.5, y0, 5.0, 7.0, 1.0, c, 0.0, c);
            }
        }
        let cx = to_x(v.cue);
        ui.scene.fill(cx - 1.0, y0, 2.0, h, wf.cue);
        ui.scene.fill(px - 1.0, y0, 2.0, h, wf.playhead);
    }
    ui.text(
        if v.wave.is_some() {
            ""
        } else {
            "TOUCH TO SEEK"
        },
        x0 + 8.0,
        y0 + 4.0,
        t.text.small.color(t.color.dim),
    );
    ui.hit(Target::Overview, OVERVIEW);
    ui.scene.fill(x0, 572.0, w, 1.0, t.color.button_edge);
    ui.text("NEEDLE SEARCH", x0 + w * 0.5, 574.0, t.text.small.center());

    ui.chip(
        Rect::new(828.0, 502.0, 62.0, 39.0),
        "±10",
        t.text.chip_large.color(t.color.cue_text),
        t.color.cue,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::TempoRange, Rect::new(828.0, 502.0, 62.0, 39.0));
    ui.chip(
        Rect::new(828.0, 545.0, 62.0, 43.0),
        "[4]",
        t.text.chip_large,
        t.color.chip_edge,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::JumpSize, Rect::new(828.0, 545.0, 62.0, 43.0));
    ui.scene.rounded(
        898.0,
        502.0,
        118.0,
        86.0,
        t.shape.button_radius,
        t.color.button,
        t.shape.border,
        t.color.accent,
    );
    let bpm = v
        .grid
        .map_or("--.-".to_string(), |g| format!("{:.1}", g.bpm() * v.rate));
    ui.text(
        bpm,
        1006.0,
        514.0,
        t.text.bpm.color(t.color.accent_text).right(),
    );
    ui.text("BPM", 1006.0, 558.0, t.text.small.right());
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
            wave: None,
            hot_cue_positions: [f64::NAN; engine_core::HOT_CUES],
            screen: ScreenKind::Main,
            perf: PerfState::default(),
            save: Some(SaveState::Unsaved),
            browse: None,
        }
    }

    #[test]
    fn browse_screen_fits_and_targets_do_not_overlap() {
        use crate::browse::{BrowseView, InfoView, MODAL_ROWS, ModalView, ROWS, Row, RowKind};
        let rows: Vec<Row> = (0..ROWS)
            .map(|i| Row {
                kind: match i {
                    0 => RowKind::Folder,
                    1 => RowKind::List,
                    _ => RowKind::Track,
                },
                tagged: i == 4,
                missing: i == 5,
                title: "A title far too long for its column ".repeat(4),
                artist: "Artist".into(),
                detail: "128.0   8A   5:12".into(),
                loaded: i == 3,
                art: None,
            })
            .collect();
        let base = BrowseView {
            header: "DJ  /  UKG".into(),
            at_root: false,
            sort: "TITLE",
            rows,
            selected: Some(3),
            first: 10,
            total: 40,
            has_tracks: true,
            ..BrowseView::default()
        };
        let variants = [
            base.clone(),
            BrowseView {
                library: true,
                can_scan: true,
                ..base.clone()
            },
            BrowseView {
                library: true,
                can_scan: true,
                scan: Some((120, 586)),
                ..base.clone()
            },
            BrowseView {
                library: true,
                can_scan: true,
                unanalysed: 12,
                ..base.clone()
            },
            BrowseView {
                library: true,
                can_scan: true,
                unanalysed: 12,
                analyzing: Some((3, 12, false)),
                ..base.clone()
            },
            BrowseView {
                searching: true,
                keyboard: true,
                ..base.clone()
            },
            BrowseView {
                has_tracks: false,
                ..base.clone()
            },
            BrowseView {
                tag_list: true,
                modal: Some(ModalView {
                    title: "MENU".into(),
                    rows: (0..7).map(|i| format!("ACTION {i}")).collect(),
                    selected: 6,
                    danger_first: false,
                }),
                ..base.clone()
            },
            BrowseView {
                modal: Some(ModalView {
                    title: "DELETE PEAK?".into(),
                    rows: vec!["YES".into(), "CANCEL".into()],
                    selected: 1,
                    danger_first: true,
                }),
                ..base.clone()
            },
            BrowseView {
                keyboard: true,
                header_override: Some("NEW PLAYLIST:  PEAK▏".into()),
                ..base.clone()
            },
            BrowseView {
                ask_scan: true,
                at_root: true,
                ..base.clone()
            },
            BrowseView {
                side: Some(InfoView {
                    source: "USB",
                    fields: vec![
                        ("TITLE", "A title far too long for its column ".repeat(4)),
                        ("ARTIST", "Artist".into()),
                        ("GENRE", "uk garage".into()),
                    ],
                    rating: Some(3),
                    comment: Some("word ".repeat(80)),
                    ..InfoView::default()
                }),
                ..base.clone()
            },
            BrowseView {
                library: true,
                info: Some(InfoView {
                    source: "USB",
                    position: "TRACK 004/040".into(),
                    fields: vec![
                        ("TITLE", "A title far too long for its column ".repeat(4)),
                        ("ARTIST", "Artist".into()),
                        ("ALBUM", "Album".into()),
                        ("TIME", "5:12".into()),
                        ("BPM", "128.0".into()),
                        ("KEY", "8A".into()),
                    ],
                    rating: Some(3),
                    comment: Some("word ".repeat(80)),
                    ..InfoView::default()
                }),
                ..base.clone()
            },
        ];
        for b in &variants {
            let mut v = view(None);
            v.screen = ScreenKind::Browse;
            v.browse = Some(b);
            let mut scene = Scene::default();
            let hits = build(
                &v,
                &Theme::default(),
                &mut OverviewCache::default(),
                &mut scene,
            );
            for r in &scene.rects {
                assert!(
                    r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
                    "{r:?}"
                );
            }
            for (i, a) in hits.iter().enumerate() {
                for c in &hits[i + 1..] {
                    let overlap = a.rect.overlaps(c.rect);
                    assert!(!overlap, "{:?} overlaps {:?}", a.target, c.target);
                }
            }
            for h in hits.iter().filter(|h| h.target != Target::Overview) {
                assert!(
                    h.rect.w >= 36.0 && h.rect.h >= 36.0,
                    "{:?} is {}×{}",
                    h.target,
                    h.rect.w,
                    h.rect.h
                );
            }
            let has = |t: Target| hits.iter().any(|h| h.target == t);
            let rows = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Row(_)))
                .count();
            let listing = b.info.is_none();
            let covered = b.ask_scan || b.modal.is_some() || !listing;
            assert_eq!(rows, if covered { 0 } else { ROWS });
            if let Some(m) = &b.modal {
                // Inside the list area, clear of the status band.
                for h in hits
                    .iter()
                    .filter(|h| matches!(h.target, Target::ModalRow(_)))
                {
                    assert!(h.rect.bottom() <= 386.0, "{h:?}");
                }
                let shown = hits
                    .iter()
                    .filter(|h| matches!(h.target, Target::ModalRow(_)));
                assert_eq!(shown.count(), m.rows.len().min(MODAL_ROWS));
                // Scrolled to keep the cursor in view.
                let sel = Target::ModalRow(m.selected as u16);
                assert!(hits.iter().any(|h| h.target == sel));
            }
            if let Some(h) = &b.header_override {
                assert!(scene.texts.iter().any(|t| &t.text == h));
            }
            assert_eq!(has(Target::BrowseBack), !b.at_root && listing);
            assert_eq!(has(Target::Scan), b.library && b.scan.is_none() && listing);
            assert_eq!(has(Target::ScanYes), b.ask_scan);
            let analyze = listing
                && b.library
                && b.scan.is_none()
                && (b.unanalysed > 0 || b.analyzing.is_some());
            assert_eq!(has(Target::AnalyzeAll), analyze);
            if b.analyzing.is_some() {
                assert!(scene.texts.iter().any(|t| t.text == "ANALYZING 3 / 12"));
            }
            let keys = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Key(_)))
                .count();
            let all_keys = crate::browse::key_count();
            assert_eq!(keys, if b.keyboard { all_keys } else { 0 });
            assert_eq!(
                has(Target::Overview),
                !b.keyboard,
                "the keyboard covers the overview"
            );
            let tab = if b.tag_list {
                Target::TagList
            } else {
                Target::Browse
            };
            assert!(hits.iter().any(|h| h.target == tab));
            // The title column's divider: only with tracks (or beside the info panel).
            let divider = scene
                .rects
                .iter()
                .any(|r| r.x == 559.0 && r.y == 103.0 && r.w == 1.0);
            if b.info.is_none() && !b.ask_scan {
                assert_eq!(divider, b.has_tracks || b.side.is_some());
            }
            if b.side.is_some() {
                // The list keeps its jacket and title columns; the panel is beside it.
                for h in hits.iter().filter(|h| matches!(h.target, Target::Row(_))) {
                    assert!(h.rect.right() <= 560.0, "{h:?}");
                }
                assert!(!scene.texts.iter().any(|t| t.text == "128.0   8A   5:12"));
                assert!(scene.texts.iter().any(|t| t.text == "★★★☆☆"));
                assert!(scene.texts.iter().any(|t| t.text == "uk garage"));
            }
            if let Some(info) = &b.info {
                let stars = scene.texts.iter().any(|t| t.text == "★★★☆☆");
                assert!(stars, "the rating shows as stars");
                let title = &info.fields[0].1;
                assert!(
                    scene
                        .texts
                        .iter()
                        .any(|t| &t.text == title && t.max_width.is_some())
                );
            }
            // Row text is ellipsized to its column.
            assert!(
                scene
                    .texts
                    .iter()
                    .filter(|t| t.text.starts_with("A title"))
                    .all(|t| t.max_width.is_some())
            );
        }
    }

    #[test]
    fn everything_stays_on_the_panel_and_targets_do_not_overlap() {
        let grid = DeckGrid::constant(1_000.0, 123.0, 48_000.0 * 300.0, 48_000).unwrap();
        let mut scene = Scene::default();
        let hits = build(
            &view(Some(&grid)),
            &Theme::default(),
            &mut OverviewCache::default(),
            &mut scene,
        );
        for r in &scene.rects {
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
                "{r:?}"
            );
        }
        for (i, a) in hits.iter().enumerate() {
            for b in &hits[i + 1..] {
                let overlap = a.rect.overlaps(b.rect);
                assert!(!overlap, "{:?} overlaps {:?}", a.target, b.target);
            }
        }
        // Touch targets the design pass sized for a 7" panel: at least 36 px on each side.
        for h in hits.iter().filter(|h| h.target != Target::Overview) {
            assert!(
                h.rect.w >= 36.0 && h.rect.h >= 36.0,
                "{:?} is {}×{}",
                h.target,
                h.rect.w,
                h.rect.h
            );
        }
    }

    #[test]
    fn performance_screen_fits_and_targets_do_not_overlap() {
        let grid = DeckGrid::constant(1_000.0, 123.0, 48_000.0 * 300.0, 48_000).unwrap();
        let variants = [
            PerfState::default(),
            PerfState {
                mode: PadMode::BeatJump,
                jump_bank: true,
                ..PerfState::default()
            },
            PerfState {
                delete_armed: true,
                bank: 1,
                ..PerfState::default()
            },
        ];
        for perf in variants {
            let mut v = view(Some(&grid));
            v.screen = ScreenKind::Performance;
            v.perf = perf;
            v.hot_cue_positions[0] = 1_010_000.0;
            let mut scene = Scene::default();
            let hits = build(
                &v,
                &Theme::default(),
                &mut OverviewCache::default(),
                &mut scene,
            );
            for r in &scene.rects {
                assert!(
                    r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.5 && r.y + r.h <= 600.0,
                    "{perf:?}: {r:?}"
                );
            }
            for (i, a) in hits.iter().enumerate() {
                for b in &hits[i + 1..] {
                    let overlap = a.rect.overlaps(b.rect);
                    assert!(!overlap, "{perf:?}: {:?} overlaps {:?}", a.target, b.target);
                }
            }
            for h in hits.iter().filter(|h| h.target != Target::Overview) {
                assert!(
                    h.rect.w >= 36.0 && h.rect.h >= 36.0,
                    "{:?} is {}×{}",
                    h.target,
                    h.rect.w,
                    h.rect.h
                );
            }
            let pads = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Pad(_)))
                .count();
            assert_eq!(pads, crate::perform::PADS, "{perf:?}");
            let loops = hits
                .iter()
                .filter(|h| matches!(h.target, Target::BeatLoop(_)))
                .count();
            assert_eq!(loops, engine_core::BEAT_LOOPS.len());
        }
    }

    #[test]
    fn builds_without_a_grid() {
        let mut scene = Scene::default();
        build(
            &view(None),
            &Theme::default(),
            &mut OverviewCache::default(),
            &mut scene,
        );
        assert!(scene.texts.iter().any(|t| t.text == "--.-"));
    }
}
