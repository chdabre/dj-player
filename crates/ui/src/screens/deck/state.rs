//! The deck displays' own state (the zoom step and the hand on the waveform) and what touches
//! on them do.

use player_api::DeckCommand;

use super::{DeckTarget, OVERVIEW};
use crate::app::effects::Effect;
use crate::kit::Rect;
use crate::screens::Cx;

/// Beats across the zoom view, by ZOOM step.
const ZOOM_STEPS: [f64; 4] = [8.0, 16.0, 32.0, 64.0];

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
pub struct DeckDisplay {
    /// Index into [`ZOOM_STEPS`].
    zoom: usize,
    scrub: Option<Scrub>,
}

impl Default for DeckDisplay {
    fn default() -> Self {
        Self {
            zoom: 1,
            scrub: None,
        }
    }
}

impl DeckDisplay {
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
            // Drawn, not wired yet: placeholders for memory cues, the grid editor, sync, the
            // tempo range and the beat jump size.
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
