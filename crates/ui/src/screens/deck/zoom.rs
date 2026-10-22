//! The zoom waveform: the waveform around the playhead with its beat grid, loop, cue, hot
//! cues, slip shadow and playhead.

use super::{DeckTarget, hot_cue_color, hot_cue_letter, panel};
use crate::kit::Rect;
use crate::screens::{Target, Ui, View};
use crate::waves::WaveDraw;

/// Where track positions fall on the zoom view: its area, the frame at its left edge, and the
/// frames one design pixel covers.
#[derive(Clone, Copy)]
struct Timeline {
    area: Rect,
    left: f64,
    fpp: f64,
}

impl Timeline {
    fn x(self, pos: f64) -> f32 {
        self.area.x + ((pos - self.left) / self.fpp) as f32
    }

    fn visible(self, x: f32) -> bool {
        x >= self.area.x && x <= self.area.x + self.area.w
    }
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
    let tl = Timeline {
        area,
        left: v.position - span * 0.5,
        fpp,
    };

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
    loop_span(ui, v, tl);
    grid(ui, v, tl, span);
    hot_cues(ui, v, tl);
    markers(ui, v, tl);
}

/// The loop (active or stored): a fill with an edge at each end.
fn loop_span(ui: &mut Ui, v: &View, tl: Timeline) {
    let wf = &ui.t.waveform;
    let Rect { x: x0, y: y0, w, h } = tl.area;
    if !(v.deck.looping || v.deck.loop_end > v.deck.loop_start) {
        return;
    }
    let (a, b) = (
        tl.x(v.deck.loop_start).max(x0),
        tl.x(v.deck.loop_end).min(x0 + w),
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
        for e in [tl.x(v.deck.loop_start), tl.x(v.deck.loop_end)] {
            if tl.visible(e) {
                ui.scene.fill(e - 1.0, y0, 2.0, h, edge);
            }
        }
    }
}

/// Beat grid: ticks along the top and bottom edges, over the waveform.
fn grid(ui: &mut Ui, v: &View, tl: Timeline, span: f64) {
    let wf = &ui.t.waveform;
    let Rect { y: y0, h, .. } = tl.area;
    let Some(g) = v.deck.grid() else {
        return;
    };
    let first = g.beat_at(tl.left).ceil() as i64;
    let last = g.beat_at(tl.left + span).floor() as i64;
    for b in first..=last.min(first + 256) {
        let pos = g.position_of(b as f64);
        let x = tl.x(pos);
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

/// Hot cues: a line in the pad's colour with its letter in a tag at the bottom.
fn hot_cues(ui: &mut Ui, v: &View, tl: Timeline) {
    let t = ui.t;
    let Rect { y: y0, h, .. } = tl.area;
    for (i, &at) in v.deck.hot_cue_positions.iter().enumerate() {
        let x = tl.x(at);
        if !at.is_finite() || !tl.visible(x) {
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
}

/// The cue, the slip shadow, the grid's state and the playhead, centred.
fn markers(ui: &mut Ui, v: &View, tl: Timeline) {
    let t = ui.t;
    let wf = &t.waveform;
    let Rect { x: x0, y: y0, w, h } = tl.area;
    let cue = tl.x(v.deck.cue);
    if tl.visible(cue) {
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
        let s = tl.x(v.deck.shadow_position);
        if tl.visible(s) {
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
