//! The player's controls: events from the UI's stand-in, its dev keyboard or the hardware go
//! through the [`Surface`] to the deck.

use surface::{ControlEvent, Nav};

use super::{DECK, Player};
use crate::controls::{DeckNow, Surface};

impl Player {
    /// A control did something: its deck commands go to the deck; what the browse controls
    /// ask of the screen comes back.
    pub fn control(&mut self, e: ControlEvent) -> Vec<Nav> {
        let st = self.handle.deck_state(DECK);
        let deck = DeckNow {
            rate: st.rate.load(),
            quantize_on: st.quantize.load() > 0.0,
        };
        let (mut commands, mut nav) = (Vec::new(), Vec::new());
        self.controls.event(e, &deck, &mut commands, &mut nav);
        self.send_all(commands);
        nav
    }

    /// Held controls act (SEARCH scrubs, a jog bend follows the jog, TAG TRACK counts towards
    /// its long press), `dt` seconds after the last call. Call it regularly, e.g. per frame.
    pub fn tick_controls(&mut self, dt: f64) {
        let mut commands = Vec::new();
        self.controls.tick(dt, &mut commands);
        self.send_all(commands);
    }

    /// The controls' own state: the tempo range, jog mode and what the jog is doing.
    pub fn controls(&self) -> &Surface {
        &self.controls
    }

    fn send_all(&mut self, commands: Vec<engine_core::DeckCommand>) {
        for c in commands {
            if !self.send(c) {
                eprintln!("command ring full, dropped");
            }
        }
    }
}
