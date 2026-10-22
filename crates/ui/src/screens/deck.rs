//! The deck displays the playback screens share: the zoom waveform and its panel, the
//! overview, the status band (track, remaining time, tempo), the phase meters and the
//! CUE/LOOP and CALL column.

use engine_core::DeckCommand;

use super::{Cx, SaveState, Target, Ui, View};
use crate::app::effects::Effect;
use crate::kit::Rect;
use crate::render::scene::{Gradient, Rgba};
use crate::theme::Theme;
use crate::waves::{OverviewCache, WaveDraw};

/// Beats across the zoom view, by ZOOM step.
pub const ZOOM_STEPS: [f64; 4] = [8.0, 16.0, 32.0, 64.0];

/// The hand on the zoom waveform: where the finger was (design x), the frames one design
/// pixel covers, and the playhead the finger has moved the track to.
#[derive(Debug, Clone, Copy)]
struct Scrub {
    last_x: f32,
    frames_per_px: f64,
    shown: f64,
}

/// The deck displays' own state, shared by the main and performance screens.
#[derive(Debug)]
pub struct DeckState {
    /// Index into [`ZOOM_STEPS`].
    zoom: usize,
    scrub: Option<Scrub>,
}

impl Default for DeckState {
    fn default() -> Self {
        Self {
            zoom: 1,
            scrub: None,
        }
    }
}

impl DeckState {
    /// Beats across the zoom view.
    pub fn zoom_beats(&self) -> f64 {
        ZOOM_STEPS[self.zoom]
    }

    /// The playhead the finger has moved the track to, while it scrubs: the picture follows
    /// the hand at once, the audio trails it by the scratch spring's few milliseconds.
    pub fn scrubbed(&self) -> Option<f64> {
        self.scrub.map(|s| s.shown)
    }

    /// A touch on `t` (drawn at `r`) at design x `x`.
    pub fn tap(&mut self, t: DeckTarget, r: Rect, x: f32, cx: &mut Cx) {
        let looping = cx.deck.looping;
        let mut send = |c| cx.effects.push(Effect::Deck(c));
        match t {
            DeckTarget::CallPrev if looping => send(DeckCommand::LoopResize(0.5)),
            DeckTarget::CallNext if looping => send(DeckCommand::LoopResize(2.0)),
            DeckTarget::CallPrev => send(DeckCommand::BeatJump(-4.0)),
            DeckTarget::CallNext => send(DeckCommand::BeatJump(4.0)),
            DeckTarget::Zoom => self.zoom = (self.zoom + 1) % ZOOM_STEPS.len(),
            DeckTarget::Overview => seek_overview(x, cx),
            DeckTarget::Waveform => {
                let span = cx.deck.zoom_span(self.zoom_beats());
                self.scrub = Some(Scrub {
                    last_x: x,
                    frames_per_px: span / f64::from(r.w),
                    shown: cx.shown.unwrap_or(cx.deck.position),
                });
                send(DeckCommand::ScratchBegin);
            }
            // Not wired yet.
            DeckTarget::Delete
            | DeckTarget::Memory
            | DeckTarget::Grid
            | DeckTarget::Sync
            | DeckTarget::Master
            | DeckTarget::TempoRange
            | DeckTarget::JumpSize => {}
        }
    }

    /// The finger moved to design x `x` while on `pressed`; true if the deck displays took it.
    pub fn drag(&mut self, pressed: Option<DeckTarget>, x: f32, cx: &mut Cx) -> bool {
        if let Some(mut scrub) = self.scrub {
            // The waveform moves with the finger: dragging left plays forward.
            let frames = -f64::from(x - scrub.last_x) * scrub.frames_per_px;
            scrub.shown = (scrub.shown + frames).clamp(0.0, cx.deck.length);
            scrub.last_x = x;
            self.scrub = Some(scrub);
            if frames != 0.0 {
                cx.effects
                    .push(Effect::Deck(DeckCommand::ScratchBy(frames)));
            }
            return true;
        }
        if pressed == Some(DeckTarget::Overview) {
            seek_overview(x, cx);
            return true;
        }
        false
    }

    /// The finger came up.
    pub fn release(&mut self, cx: &mut Cx) {
        if self.scrub.take().is_some() {
            cx.effects.push(Effect::Deck(DeckCommand::ScratchEnd));
        }
    }

    /// A track loaded: the hand is off it.
    pub fn loaded(&mut self) {
        self.scrub = None;
    }
}

/// Needle search: seek to where design x `x` falls on the overview.
fn seek_overview(x: f32, cx: &mut Cx) {
    let Rect { x: x0, w, .. } = OVERVIEW;
    let f = f64::from(((x - x0) / w).clamp(0.0, 1.0));
    cx.effects
        .push(Effect::Deck(DeckCommand::Seek(f * cx.deck.length)));
}

/// The deck displays' touch targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckTarget {
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
}

/// The overview's bounds, for turning a touch into a track position.
pub const OVERVIEW: Rect = Rect::new(113.0, 502.0, 703.0, 67.0);

/// The colour of hot cue `i` (A = 0), until the library gives cues their own.
pub(crate) fn hot_cue_color(t: &Theme, i: usize) -> Rgba {
    t.color.hot_cue_colors[i % t.color.hot_cue_colors.len()]
}

/// Hot cue `i`'s pad letter: A, B, …
pub(crate) fn hot_cue_letter(i: usize) -> char {
    (b'A' + (i % 26) as u8) as char
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
    let (beat_in_bar, bar_in_phrase, bar, bars_left) = match v.deck.grid() {
        Some(g) => {
            let beat = g.beat_at(v.position).floor();
            let in_bar = g.bar_position(v.position).max(1);
            let bar_start = beat - f64::from(in_bar - 1);
            let bar = (bar_start / 4.0).floor();
            let left = ((g.beat_at(v.deck.length) - g.beat_at(v.position)) / 4.0).max(0.0);
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
        Target::Deck(DeckTarget::Delete),
        Rect::new(828.0, button_y, 88.0, button_h),
        "DELETE",
        None,
        false,
    );
    ui.button(
        Target::Deck(DeckTarget::Memory),
        Rect::new(923.0, button_y, 93.0, button_h),
        "MEMORY",
        None,
        false,
    );
    ui.rule_label("CALL", 828.0, rule + 91.0 + button_h - 58.0, 188.0);
    let (prev, next) = if v.deck.looping {
        ("◀ ½X", "2X ▶")
    } else {
        ("◀", "▶")
    };
    ui.button(
        Target::Deck(DeckTarget::CallPrev),
        Rect::new(828.0, call_y, 88.0, call_h),
        prev,
        None,
        false,
    );
    ui.button(
        Target::Deck(DeckTarget::CallNext),
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
    ui.hit(Target::Deck(DeckTarget::Waveform), area);
    // Frames per design pixel.
    let span = v.deck.zoom_span(v.zoom_beats);
    let fpp = span / f64::from(w);
    let left = v.position - span * 0.5;
    let to_x = |pos: f64| x0 + ((pos - left) / fpp) as f32;
    let visible = |x: f32| x >= x0 && x <= x0 + w;

    if let Some(wave) = &v.wave {
        let draw = crate::waves::zoom(
            wave.entries,
            wave.entries_per_second,
            v.position / v.deck.sample_rate,
            fpp / v.deck.sample_rate,
            area,
            &wave.settings,
            &wave.style,
        );
        ui.scene.waveform(WaveDraw {
            opacity: wf.opacity,
            ..draw
        });
    }
    if v.deck.looping || v.deck.loop_end > v.deck.loop_start {
        let (a, b) = (
            to_x(v.deck.loop_start).max(x0),
            to_x(v.deck.loop_end).min(x0 + w),
        );
        if b > a {
            let (fill, edge) = if v.deck.looping {
                (wf.loop_color.alpha(wf.loop_fill), wf.loop_color)
            } else {
                (
                    wf.loop_color.alpha(wf.loop_fill_stored),
                    wf.loop_edge_stored,
                )
            };
            ui.scene.fill(a, y0, b - a, h, fill);
            for e in [to_x(v.deck.loop_start), to_x(v.deck.loop_end)] {
                if visible(e) {
                    ui.scene.fill(e - 1.0, y0, 2.0, h, edge);
                }
            }
        }
    }
    // Beat grid: ticks along the top and bottom edges, over the waveform.
    if let Some(g) = v.deck.grid() {
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
    for (i, &at) in v.deck.hot_cue_positions.iter().enumerate() {
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
    let cue = to_x(v.deck.cue);
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
    if v.deck.slip_active {
        let s = to_x(v.deck.shadow_position);
        if visible(s) {
            ui.scene.fill(s - 1.0, y0, 2.0, h, wf.slip_shadow);
        }
    }
    let label = match (v.deck.grid(), v.deck.analysing) {
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

pub(crate) fn clock(seconds: f64) -> (String, String) {
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
    let hot = v.deck.hot_cues;
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
    let quantize = match v.deck.quantize {
        q if q <= 0.0 => "QUANTIZE : OFF".to_string(),
        q if q >= 1.0 => format!("QUANTIZE : {q}"),
        q => format!("QUANTIZE : 1/{}", (1.0 / q).round()),
    };
    ui.text(
        &quantize,
        424.0,
        398.0,
        t.text.label.color(if v.deck.quantize > 0.0 {
            t.color.cue_text
        } else {
            t.color.dim
        }),
    );
    let (mmss, ms) = clock((v.deck.length - v.position) / v.deck.sample_rate);
    // Tabular digits: "00:00" at 56 px ends ~118 px in.
    ui.text(mmss, 469.0, 422.0, t.text.readout.right());
    ui.text(ms, 470.0, 437.0, t.text.readout_fraction);

    ui.text("TEMPO", 653.0, 398.0, t.text.label);
    let (edge, color) = if v.deck.keylock {
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
    let pct = (v.deck.rate - 1.0) * 100.0;
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
        Target::Deck(DeckTarget::Sync),
        Rect::new(828.0, 408.0, 88.0, 78.0),
        "SYNC",
        Some("– INST.D."),
        false,
    );
    ui.button(
        Target::Deck(DeckTarget::Master),
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
    if v.deck.length > 0.0 {
        let to_x = |pos: f64| x0 + (pos / v.deck.length).clamp(0.0, 1.0) as f32 * w;
        if let Some(wave) = &v.wave {
            let draw = overview.draw(
                wave.entries,
                wave.entries_per_second,
                v.deck.length / v.deck.sample_rate,
                v.position / v.deck.sample_rate,
                OVERVIEW,
                &wave.settings,
                &wave.style,
            );
            ui.scene.waveform(WaveDraw {
                opacity: wf.opacity,
                ..draw
            });
        }
        if v.deck.loaded_fraction < 1.0 {
            let lx = to_x(v.deck.loaded_fraction * v.deck.length);
            ui.scene
                .fill(lx, y0 + h - 3.0, x0 + w - lx, 3.0, wf.loading);
        }
        // 16-bar ticks.
        if let Some(g) = v.deck.grid() {
            let beats = g.beat_at(v.deck.length).floor() as i64;
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
        if v.deck.loop_end > v.deck.loop_start {
            let (a, b) = (to_x(v.deck.loop_start), to_x(v.deck.loop_end));
            ui.scene.fill(
                a,
                y0,
                (b - a).max(2.0),
                h,
                wf.loop_color.alpha(if v.deck.looping {
                    wf.overview_loop_fill
                } else {
                    wf.overview_loop_fill_stored
                }),
            );
        }
        for (i, &at) in v.deck.hot_cue_positions.iter().enumerate() {
            if at.is_finite() {
                let c = hot_cue_color(t, i);
                ui.scene
                    .rounded(to_x(at) - 2.5, y0, 5.0, 7.0, 1.0, c, 0.0, c);
            }
        }
        let cx = to_x(v.deck.cue);
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
    ui.hit(Target::Deck(DeckTarget::Overview), OVERVIEW);
    ui.scene.fill(x0, 572.0, w, 1.0, t.color.button_edge);
    ui.text("NEEDLE SEARCH", x0 + w * 0.5, 574.0, t.text.small.center());

    ui.chip(
        Rect::new(828.0, 502.0, 62.0, 39.0),
        "±10",
        t.text.chip_large.color(t.color.cue_text),
        t.color.cue,
        Rgba::TRANSPARENT,
    );
    ui.hit(
        Target::Deck(DeckTarget::TempoRange),
        Rect::new(828.0, 502.0, 62.0, 39.0),
    );
    ui.chip(
        Rect::new(828.0, 545.0, 62.0, 43.0),
        "[4]",
        t.text.chip_large,
        t.color.chip_edge,
        Rgba::TRANSPARENT,
    );
    ui.hit(
        Target::Deck(DeckTarget::JumpSize),
        Rect::new(828.0, 545.0, 62.0, 43.0),
    );
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
    let bpm = v.deck.grid().map_or("--.-".to_string(), |g| {
        format!("{:.1}", g.bpm() * v.deck.rate)
    });
    ui.text(
        bpm,
        1006.0,
        514.0,
        t.text.bpm.color(t.color.accent_text).right(),
    );
    ui.text("BPM", 1006.0, 558.0, t.text.small.right());
}
