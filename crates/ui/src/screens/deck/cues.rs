//! The right column's cue groups: CUE/LOOP memory (DELETE, MEMORY) and CALL (◀ ▶).

use super::DeckTarget;
use crate::kit::Rect;
use crate::screens::chrome::{COLUMN_W, COLUMN_X};
use crate::screens::{Target, Ui, View};

/// The right column's CUE/LOOP (DELETE, MEMORY) and CALL (◀ ▶) groups: rule labels at `rule`
/// and above the call buttons, buttons `button_h` and `call_h` high.
pub(crate) fn cue_column(ui: &mut Ui, v: &View, rule: f32, button_h: f32, call_h: f32) {
    let button_y = rule + 21.0;
    let call_y = rule + 113.0 + (button_h - 58.0);
    ui.rule_label("CUE/LOOP", COLUMN_X, rule, COLUMN_W);
    ui.button(
        Target::Deck(DeckTarget::Delete),
        Rect::new(COLUMN_X, button_y, 88.0, button_h),
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
    ui.rule_label("CALL", COLUMN_X, rule + 91.0 + button_h - 58.0, COLUMN_W);
    let (prev, next) = if v.deck.looping {
        ("◀ ½X", "2X ▶")
    } else {
        ("◀", "▶")
    };
    ui.button(
        Target::Deck(DeckTarget::CallPrev),
        Rect::new(COLUMN_X, call_y, 88.0, call_h),
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
