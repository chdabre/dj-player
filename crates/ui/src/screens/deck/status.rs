//! The status band (track, remaining time, tempo, SYNC and MASTER) and the phase meters with
//! their bar counters.

use super::DeckTarget;
use crate::kit::Rect;
use crate::render::scene::Rgba;
use crate::screens::chrome::{COLUMN_X, CONTENT_X};
use crate::screens::{SaveState, Target, Ui, View};

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

/// Four segments; `active` is 1-based. `filled`: the beat meter fills, the bar meter outlines.
fn meter(ui: &mut Ui, x: f32, y: f32, w: f32, active: Option<u8>, filled: bool) {
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

/// `mm:ss` and `.mmm` of a time.
fn clock(seconds: f64) -> (String, String) {
    let s = seconds.max(0.0);
    let m = (s / 60.0).floor();
    let rest = s - m * 60.0;
    (
        format!("{:02}:{:02}", m as u64, rest.floor() as u64),
        format!(".{:03}", ((rest.fract()) * 1000.0).floor() as u64),
    )
}

/// The status band: the track and its save state, the remaining time, the tempo, SYNC and
/// MASTER.
pub(crate) fn status(ui: &mut Ui, v: &View) {
    track(ui, v);
    remain(ui, v);
    tempo(ui, v);
    ui.button(
        Target::Deck(DeckTarget::Sync),
        Rect::new(COLUMN_X, 408.0, 88.0, 78.0),
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

/// TRACK: the save state, the track number, the set hot cues and A.CUE.
fn track(ui: &mut Ui, v: &View) {
    let t = ui.t;
    ui.text("TRACK", CONTENT_X, 398.0, t.text.label);
    if let Some(save) = v.save {
        let color = match save {
            SaveState::Saved => t.color.dim,
            SaveState::Saving => t.color.label,
            SaveState::Unsaved => t.color.danger_text,
        };
        ui.text(save.label(), 160.0, 398.0, t.text.label.color(color));
    }
    ui.text("01", CONTENT_X, 422.0, t.text.readout);
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
}

/// REMAIN: the quantize setting and the time left.
fn remain(ui: &mut Ui, v: &View) {
    let t = ui.t;
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
}

/// TEMPO: KEYLOCK and the tempo in percent.
fn tempo(ui: &mut Ui, v: &View) {
    let t = ui.t;
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
}
