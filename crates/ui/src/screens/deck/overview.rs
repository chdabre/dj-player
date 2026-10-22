//! The bottom strip: the overview of the whole track (needle search), and beside it the tempo
//! range, the beat jump size and the BPM.

use super::{DeckTarget, hot_cue_color, panel};
use crate::kit::Rect;
use crate::render::scene::Rgba;
use crate::screens::chrome::{COLUMN_X, CONTENT_W, CONTENT_X};
use crate::screens::{Target, Ui, View};
use crate::waves::{OverviewCache, WaveDraw};

/// The overview's bounds, for turning a touch into a track position.
pub const OVERVIEW: Rect = Rect::new(CONTENT_X, 502.0, CONTENT_W, 67.0);

/// The bottom strip: the overview and the tempo column beside it.
pub(crate) fn bottom(ui: &mut Ui, v: &View, overview: &mut OverviewCache) {
    track_overview(ui, v, overview);
    tempo_column(ui, v);
}

/// The overview: the whole track's waveform (or the played part shaded), the loaded part,
/// 16-bar ticks, the loop, hot cues, cue and playhead.
fn track_overview(ui: &mut Ui, v: &View, overview: &mut OverviewCache) {
    let t = ui.t;
    let Rect {
        x: x0, y: y0, w, ..
    } = OVERVIEW;
    panel(ui, OVERVIEW, v.wave.map(|w| w.settings.overview_align));
    if v.deck.length > 0.0 {
        markers(ui, v, overview);
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
}

/// What the overview shows of a track with a length.
fn markers(ui: &mut Ui, v: &View, overview: &mut OverviewCache) {
    let t = ui.t;
    let wf = &t.waveform;
    let Rect { x: x0, y: y0, w, h } = OVERVIEW;
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

/// The tempo range and beat jump size chips, and the BPM.
fn tempo_column(ui: &mut Ui, v: &View) {
    let t = ui.t;
    let range = Rect::new(COLUMN_X, 502.0, 62.0, 39.0);
    ui.chip(
        range,
        "±10",
        t.text.chip_large.color(t.color.cue_text),
        t.color.cue,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::Deck(DeckTarget::TempoRange), range);
    let jump = Rect::new(COLUMN_X, 545.0, 62.0, 43.0);
    ui.chip(
        jump,
        "[4]",
        t.text.chip_large,
        t.color.chip_edge,
        Rgba::TRANSPARENT,
    );
    ui.hit(Target::Deck(DeckTarget::JumpSize), jump);
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
