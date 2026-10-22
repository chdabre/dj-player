//! The deck displays the playback screens share:
//!
//! - `state.rs`: their own state (zoom step, the hand on the waveform) and touch handlers;
//! - `zoom.rs`: the zoom waveform and its panel;
//! - `overview.rs`: the bottom strip (overview, tempo range, jump size, BPM);
//! - `status.rs`: the status band (track, remaining time, tempo) and the phase meters;
//! - `cues.rs`: the CUE/LOOP and CALL column.

mod cues;
mod overview;
mod state;
mod status;
mod zoom;

pub(crate) use cues::cue_column;
pub use overview::OVERVIEW;
pub(crate) use overview::bottom;
pub use state::DeckDisplay;
pub(crate) use status::{phase, status};
pub(crate) use zoom::zoom;

use crate::kit::Rect;
use crate::render::scene::{Gradient, Rgba};
use crate::screens::Ui;
use crate::theme::Theme;

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

/// The colour of hot cue `i` (A = 0), until the library gives cues their own.
pub(crate) fn hot_cue_color(t: &Theme, i: usize) -> Rgba {
    t.color.hot_cue_colors[i % t.color.hot_cue_colors.len()]
}

/// Hot cue `i`'s pad letter: A, B, …
pub(crate) fn hot_cue_letter(i: usize) -> char {
    (b'A' + (i % 26) as u8) as char
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
