//! The performance screen ("2 · Performance" and its variants on the 1024×600 canvas): four
//! pads that play hot cues or beat jumps, a row of beat loops, a small zoom waveform, and the
//! main screen's status band and overview.
//!
//! - **HOT CUE:** a pad stores the current position if empty, else jumps there (the deck's
//!   `HotCue` command). BANK pages through the hot cues four at a time.
//! - **BEAT JUMP:** the pads jump ∓4 / ∓1 beats; BANK switches them to ∓32 / ∓16.
//! - **DELETE:** arms deletion: tapping a set pad deletes that hot cue, until DELETE is
//!   tapped again or the mode changes. CALL (stored hot cues to call up) comes with the library.
//! - **BEAT LOOP:** a 1/2 to 16 beat loop from the playhead; tapping the active length exits.

use engine_core::{BEAT_LOOPS, DeckCommand, HOT_CUES};

use super::chrome::{lower, rail_bottom, rail_panel};
use super::deck::{bottom, cue_column, hot_cue_color, hot_cue_letter, phase, status, zoom};
use super::{ChromeTarget, Target, Ui, View};
use crate::kit::Rect;
use crate::render::scene::Rgba;
/// The performance screen's touch targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerfTarget {
    HotCueMode,
    BeatJumpMode,
    DeleteCall,
    Bank,
    /// One of the four pads (0..4, left to right).
    Pad(u8),
    /// One of the beat loop lengths ([`BEAT_LOOPS`] index).
    BeatLoop(u8),
}

/// Pads per bank.
pub const PADS: usize = 4;

const ZOOM: Rect = Rect::new(113.0, 95.0, 703.0, 72.0);

const PAD_ROW: Rect = Rect::new(113.0, 180.0, 703.0, 88.0);

const LOOP_ROW: Rect = Rect::new(113.0, 299.0, 703.0, 67.0);

/// Inner padding of a pad row, and the gap between pads.
const ROW_PAD: f32 = 6.0;

/// Beats each pad jumps in beat jump mode: the first bank, and the second.
const JUMPS: [[f64; PADS]; 2] = [[-4.0, -1.0, 1.0, 4.0], [-32.0, -16.0, 16.0, 32.0]];

/// Hot cue banks: all hot cues, four at a time.
pub const BANKS: usize = HOT_CUES.div_ceil(PADS);

/// What a touch on the performance screen does: updates `perf` and returns the deck command
/// to send, if any. `hot_cues` is the deck's set-pads mask, `loop_beats` the active loop's
/// length in beats (None when not looping).
pub fn press(
    perf: &mut PerfState,
    target: PerfTarget,
    hot_cues: u8,
    loop_beats: Option<f64>,
) -> Option<DeckCommand> {
    match target {
        PerfTarget::HotCueMode => {
            perf.mode = PadMode::HotCue;
            perf.delete_armed = false;
            None
        }
        PerfTarget::BeatJumpMode => {
            perf.mode = PadMode::BeatJump;
            perf.delete_armed = false;
            None
        }
        PerfTarget::DeleteCall => {
            if perf.mode == PadMode::HotCue {
                perf.delete_armed = !perf.delete_armed;
            } else {
                perf.mode = PadMode::HotCue;
                perf.delete_armed = true;
            }
            None
        }
        PerfTarget::Bank => {
            match perf.mode {
                PadMode::HotCue => perf.bank = (perf.bank + 1) % BANKS,
                PadMode::BeatJump => perf.jump_bank = !perf.jump_bank,
            }
            None
        }
        PerfTarget::Pad(i) => {
            let i = usize::from(i);
            match perf.mode {
                PadMode::HotCue => {
                    let cue = perf.bank * PADS + i;
                    if cue >= HOT_CUES {
                        None
                    } else if perf.delete_armed {
                        (hot_cues & (1 << cue) != 0).then_some(DeckCommand::DeleteHotCue(cue))
                    } else {
                        Some(DeckCommand::HotCue(cue))
                    }
                }
                PadMode::BeatJump => Some(DeckCommand::BeatJump(
                    JUMPS[usize::from(perf.jump_bank)][i.min(PADS - 1)],
                )),
            }
        }
        PerfTarget::BeatLoop(k) => {
            let beats = BEAT_LOOPS[usize::from(k).min(BEAT_LOOPS.len() - 1)];
            if loop_beats.is_some_and(|b| (b - beats).abs() < 1e-3) {
                Some(DeckCommand::ReloopExit)
            } else {
                Some(DeckCommand::BeatLoop(beats))
            }
        }
    }
}

/// How a pad looks.
#[derive(Clone, Copy)]
enum PadLook {
    Empty,
    /// A set hot cue, in its colour.
    Set(Rgba),
    /// A beat jump.
    Jump,
    /// Armed for deletion.
    Armed,
    /// Selected (the active beat loop).
    Selected,
    /// Empty while deletion is armed.
    Dim,
}

/// Draw a pad with `label` and an optional second line.
fn pad(
    ui: &mut Ui,
    target: Target,
    Rect { x, y, w, h }: Rect,
    label: &str,
    sub: Option<&str>,
    look: PadLook,
) {
    let t = ui.t;
    let pressed = ui.pressed == Some(target);
    let (fill, edge, border) = match look {
        PadLook::Selected => (t.color.button_on, t.color.button_on_edge, t.shape.border),
        PadLook::Armed => (t.color.danger_fill, t.color.danger, t.shape.border_active),
        _ => (t.color.button, t.color.button_edge, t.shape.border),
    };
    let fill = if pressed {
        t.color.button_pressed
    } else {
        fill
    };
    ui.scene
        .rounded(x, y, w, h, t.shape.button_radius, fill, border, edge);
    let strip = 4.0;
    match look {
        PadLook::Set(c) => {
            ui.scene.rounded(
                x,
                y,
                w,
                h,
                t.shape.button_radius,
                c.alpha(t.color.pad_tint),
                0.0,
                c,
            );
            ui.scene
                .rounded(x, y, w, strip, t.shape.button_radius, c, 0.0, c);
        }
        PadLook::Jump => {
            let c = t.color.accent_text;
            ui.scene
                .rounded(x, y, w, strip, t.shape.button_radius, c, 0.0, c);
        }
        _ => {}
    }
    let (label_color, sub_color) = match look {
        PadLook::Dim => (t.color.dim, t.color.dim),
        PadLook::Armed => (t.color.text, t.color.danger_text),
        _ => (t.color.text, t.color.label),
    };
    let lh = t.font.line_height;
    let block = t.font.pad * lh + sub.map_or(0.0, |_| t.font.small * lh);
    let top = y + (h - block) * 0.5;
    ui.text(
        label,
        x + w * 0.5,
        top,
        t.text.pad.color(label_color).center(),
    );
    if let Some(sub) = sub {
        ui.text(
            sub,
            x + w * 0.5,
            top + t.font.pad * lh,
            t.text.small.color(sub_color).center(),
        );
    }
    ui.hit(target, Rect::new(x, y, w, h));
}

/// The rectangles of `n` pads in a row area.
fn row_cells(row: Rect, n: usize) -> impl Iterator<Item = Rect> {
    row.inset(ROW_PAD).columns(n, ROW_PAD)
}

fn row_frame(ui: &mut Ui, Rect { x, y, w, h }: Rect) {
    let t = ui.t;
    ui.scene.rounded(
        x,
        y,
        w,
        h,
        t.shape.button_radius,
        Rgba::TRANSPARENT,
        t.shape.border,
        t.color.button_edge,
    );
}

/// Lay out the performance screen.
pub(crate) fn build(ui: &mut Ui, v: &View, overview: &mut crate::waves::OverviewCache) {
    let t = ui.t;
    let perf = v.perf;

    // Frame: rail, top row, pads section, status band, bottom strip, column dividers.
    rail_panel(ui);
    ui.section(Rect::new(106.0, 2.0, 915.0, 85.0));
    ui.section(Rect::new(106.0, 90.0, 915.0, 301.0));
    lower(ui);
    for (y, h) in [(7.0, 75.0), (95.0, 292.0)] {
        ui.scene.fill(822.0, y, 1.0, h, t.color.section_edge);
    }

    rail(ui, perf);
    rail_bottom(ui, v);

    // Top row: phase meters and the way back to the main screen.
    phase(ui, v, 233.0, 367.0, 12.0);
    ui.button(
        Target::Chrome(ChromeTarget::Perform),
        Rect::new(828.0, 7.0, 188.0, 77.0),
        "PERFORM",
        None,
        true,
    );

    zoom(ui, v, ZOOM);
    pads(ui, v, perf);

    ui.rule_label("BEAT LOOP", LOOP_ROW.x, 279.0, LOOP_ROW.w);
    row_frame(ui, LOOP_ROW);
    let active = v.deck.loop_beats();
    for (k, (cell, beats)) in row_cells(LOOP_ROW, BEAT_LOOPS.len())
        .zip(BEAT_LOOPS)
        .enumerate()
    {
        let label = if beats < 1.0 {
            format!("1/{}", (1.0 / beats).round())
        } else {
            format!("{beats}")
        };
        let look = if active.is_some_and(|b| (b - beats).abs() < 1e-3) {
            PadLook::Selected
        } else {
            PadLook::Empty
        };
        pad(
            ui,
            Target::Perform(PerfTarget::BeatLoop(k as u8)),
            cell,
            &label,
            None,
            look,
        );
    }

    cue_column(ui, v, 183.0, 60.0, 60.0);
    status(ui, v);
    bottom(ui, v, overview);
}

fn rail(ui: &mut Ui, perf: PerfState) {
    let t = ui.t;
    let style = t.text.rail_mode;
    ui.button_styled(
        Target::Perform(PerfTarget::HotCueMode),
        Rect::new(6.0, 6.0, 88.0, 52.0),
        "HOT CUE",
        None,
        perf.mode == PadMode::HotCue,
        style,
    );
    ui.button_styled(
        Target::Perform(PerfTarget::BeatJumpMode),
        Rect::new(6.0, 62.0, 88.0, 52.0),
        "BEAT JUMP",
        None,
        perf.mode == PadMode::BeatJump,
        style,
    );
    let delete = Rect::new(6.0, 126.0, 88.0, 62.0);
    if perf.delete_armed {
        let Rect { x, y, w, h } = delete;
        ui.scene.rounded(
            x,
            y,
            w,
            h,
            t.shape.button_radius,
            t.color.danger_fill,
            t.shape.border,
            t.color.danger,
        );
        let lh = t.font.line_height;
        let top = y + (h - t.font.button * lh - t.font.small * lh) * 0.5;
        ui.text("DELETE", x + w * 0.5, top, t.text.button.center());
        ui.text(
            "ARMED",
            x + w * 0.5,
            top + t.font.button * lh,
            t.text.small.color(t.color.danger_text).center(),
        );
        ui.hit(Target::Perform(PerfTarget::DeleteCall), delete);
    } else {
        ui.button(
            Target::Perform(PerfTarget::DeleteCall),
            delete,
            "DELETE",
            Some("– CALL"),
            false,
        );
    }
    let bank = match perf.mode {
        PadMode::HotCue => {
            let first = perf.bank * PADS;
            let last = (first + PADS).min(HOT_CUES) - 1;
            format!("{}–{}", hot_cue_letter(first), hot_cue_letter(last))
        }
        PadMode::BeatJump if perf.jump_bank => "16 · 32".to_string(),
        PadMode::BeatJump => "1 · 4".to_string(),
    };
    ui.button(
        Target::Perform(PerfTarget::Bank),
        Rect::new(6.0, 196.0, 88.0, 62.0),
        "BANK",
        Some(&bank),
        false,
    );
}

fn pads(ui: &mut Ui, v: &View, perf: PerfState) {
    row_frame(ui, PAD_ROW);
    for (i, cell) in row_cells(PAD_ROW, PADS).enumerate() {
        let target = Target::Perform(PerfTarget::Pad(i as u8));
        match perf.mode {
            PadMode::HotCue => {
                let cue = perf.bank * PADS + i;
                if cue >= HOT_CUES {
                    continue;
                }
                let set = v.deck.hot_cues & (1 << cue) != 0;
                let letter = hot_cue_letter(cue);
                match (perf.delete_armed, set) {
                    (true, true) => pad(
                        ui,
                        target,
                        cell,
                        &format!("✕ {letter}"),
                        Some("TAP TO DELETE"),
                        PadLook::Armed,
                    ),
                    (true, false) => pad(
                        ui,
                        target,
                        cell,
                        &letter.to_string(),
                        Some("EMPTY"),
                        PadLook::Dim,
                    ),
                    (false, true) => {
                        let c = hot_cue_color(ui.t, cue);
                        pad(ui, target, cell, &letter.to_string(), None, PadLook::Set(c));
                    }
                    (false, false) => {
                        pad(ui, target, cell, &letter.to_string(), None, PadLook::Empty)
                    }
                }
            }
            PadMode::BeatJump => {
                let beats = JUMPS[usize::from(perf.jump_bank)][i];
                let n = beats.abs();
                let label = if beats < 0.0 {
                    format!("◀ {n}")
                } else {
                    format!("{n} ▶")
                };
                let unit = if n == 1.0 { "BEAT" } else { "BEATS" };
                pad(ui, target, cell, &label, Some(unit), PadLook::Jump);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Commands compare by their debug form (`DeckCommand` holds sources, so no `PartialEq`).
    fn cmd(c: Option<DeckCommand>) -> String {
        format!("{c:?}")
    }

    #[test]
    fn hot_cue_pads_set_call_and_delete_by_bank() {
        let mut p = PerfState::default();
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(2), 0, None)),
            cmd(Some(DeckCommand::HotCue(2)))
        );
        press(&mut p, PerfTarget::Bank, 0, None);
        assert_eq!(p.bank, 1);
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(0), 0, None)),
            cmd(Some(DeckCommand::HotCue(4)))
        );
        // Banks wrap.
        for _ in 1..BANKS {
            press(&mut p, PerfTarget::Bank, 0, None);
        }
        assert_eq!(p.bank, 0);
        // Delete: only set pads, and it stays armed until DELETE again.
        press(&mut p, PerfTarget::DeleteCall, 0b0100, None);
        assert!(p.delete_armed);
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(0), 0b0100, None)),
            cmd(None)
        );
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(2), 0b0100, None)),
            cmd(Some(DeckCommand::DeleteHotCue(2)))
        );
        assert!(p.delete_armed);
        press(&mut p, PerfTarget::DeleteCall, 0, None);
        assert!(!p.delete_armed);
    }

    #[test]
    fn beat_jump_pads_and_their_bank() {
        let mut p = PerfState::default();
        press(&mut p, PerfTarget::BeatJumpMode, 0, None);
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(0), 0, None)),
            cmd(Some(DeckCommand::BeatJump(-4.0)))
        );
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(3), 0, None)),
            cmd(Some(DeckCommand::BeatJump(4.0)))
        );
        press(&mut p, PerfTarget::Bank, 0, None);
        assert_eq!(
            cmd(press(&mut p, PerfTarget::Pad(1), 0, None)),
            cmd(Some(DeckCommand::BeatJump(-16.0)))
        );
        // DELETE from beat jump mode goes back to hot cues, armed.
        press(&mut p, PerfTarget::DeleteCall, 0, None);
        assert_eq!(p.mode, PadMode::HotCue);
        assert!(p.delete_armed);
    }

    #[test]
    fn beat_loop_pads_start_a_loop_or_exit_the_active_one() {
        let mut p = PerfState::default();
        assert_eq!(
            cmd(press(&mut p, PerfTarget::BeatLoop(3), 0, None)),
            cmd(Some(DeckCommand::BeatLoop(4.0)))
        );
        assert_eq!(
            cmd(press(&mut p, PerfTarget::BeatLoop(3), 0, Some(4.0))),
            cmd(Some(DeckCommand::ReloopExit))
        );
        assert_eq!(
            cmd(press(&mut p, PerfTarget::BeatLoop(0), 0, Some(4.0))),
            cmd(Some(DeckCommand::BeatLoop(0.5)))
        );
    }
}
