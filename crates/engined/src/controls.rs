//! What the player's physical controls do: the deck buttons, the tempo fader and its range,
//! the jog (scratch on the plate in vinyl mode, pitch bend on the ring), SEARCH scrubbing while
//! held, the browse knob, BACK, and TAG TRACK with its long press. One [`Surface`] takes the
//! [`ControlEvent`]s of the UI's stand-in, its dev keyboard or the hardware alike, and turns
//! them into deck commands and browse [`Nav`]s for the screen.
//!
//! It lives with the engine, not the UI, so the jog's latency is bounded by the engine
//! rather than the UI's frames (`docs/research/engine.md`, "Controller path").

use engine_core::{DeckCommand, HOT_CUES};
pub use surface::Jog;
use surface::{Control, ControlEvent, Nav, SECONDS_PER_TURN};

/// What the surface reads of the deck when a control acts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckNow {
    pub rate: f64,
    pub quantize_on: bool,
}

/// Tempo fader ranges (fraction of the rate either way) and their labels; WIDE is ±100 %.
pub const RANGES: [(f64, &str); 4] = [(0.06, "±6"), (0.10, "±10"), (0.16, "±16"), (1.0, "WIDE")];
/// SEARCH held: track seconds per second.
const SEARCH_RATE: f64 = 4.0;
/// Pitch bend per jog turn per second, and its limit (fractions of the rate).
const BEND_PER_TURN_PER_S: f64 = 0.08;
const MAX_BEND: f64 = 0.25;
/// How fast a bend follows the jog's speed and relaxes when it stops (time constant, s).
const BEND_RELAX_S: f64 = 0.08;
/// TAG TRACK held this long removes the track from the tag list.
const TAG_HOLD_S: f64 = 0.6;

/// The controls' own state: settings on the surface and what is held.
pub struct Surface {
    sample_rate: f64,
    /// Index into [`RANGES`].
    range: usize,
    /// Jog mode: VINYL scratches with the plate, CDJ bends with it.
    vinyl: bool,
    /// SEARCH held: -1 back, 1 forward.
    search: Option<f64>,
    jog: Option<Jog>,
    /// The current bend (fraction of the base rate), and jog angle turned since the last tick.
    bend: f64,
    bend_angle: f32,
    /// TAG TRACK held this long (seconds).
    tag_held: Option<f64>,
}

impl Surface {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            range: 1,
            vinyl: true,
            search: None,
            jog: None,
            bend: 0.0,
            bend_angle: 0.0,
            tag_held: None,
        }
    }

    /// The tempo fader's range: the fraction either way, and its label.
    pub fn range(&self) -> (f64, &'static str) {
        RANGES[self.range]
    }

    pub fn vinyl(&self) -> bool {
        self.vinyl
    }

    pub fn jog(&self) -> Option<Jog> {
        self.jog
    }

    /// Act on `e`, with the deck as `deck` shows it.
    pub fn event(
        &mut self,
        e: ControlEvent,
        deck: &DeckNow,
        out: &mut Vec<DeckCommand>,
        nav: &mut Vec<Nav>,
    ) {
        let mut send = |c| out.push(c);
        match e {
            ControlEvent::Down { control, alt } => match control {
                Control::Cue => send(DeckCommand::CuePress),
                Control::Play => send(DeckCommand::TogglePlay),
                Control::Rev => send(DeckCommand::ToggleReverse),
                Control::Quantize => {
                    send(DeckCommand::SetQuantize((!deck.quantize_on).then_some(1.0)));
                }
                Control::Keylock => send(DeckCommand::ToggleKeylock),
                Control::LoopIn => send(DeckCommand::LoopIn),
                Control::LoopOut => send(DeckCommand::LoopOut),
                Control::Reloop => send(DeckCommand::ReloopExit),
                Control::TrackBack => send(DeckCommand::Seek(0.0)),
                Control::Pad(i) => {
                    let i = usize::from(i);
                    if i < HOT_CUES {
                        send(if alt {
                            DeckCommand::DeleteHotCue(i)
                        } else {
                            DeckCommand::HotCue(i)
                        });
                    }
                }
                Control::SearchBack | Control::SearchFwd => {
                    send(DeckCommand::ScratchBegin);
                    self.search = Some(if control == Control::SearchBack {
                        -1.0
                    } else {
                        1.0
                    });
                }
                Control::TempoRange => self.range = (self.range + 1) % RANGES.len(),
                Control::JogMode => self.vinyl = !self.vinyl,
                Control::Back => nav.push(Nav::Back),
                Control::Browse => nav.push(Nav::Push),
                Control::TagTrack => self.tag_held = Some(0.0),
                // Present but not wired yet: no time mode, next track or shortcuts.
                Control::TimeMode | Control::TrackNext | Control::F1 | Control::F2 => {}
            },
            ControlEvent::Up(control) => match control {
                Control::Cue => send(DeckCommand::CueRelease),
                Control::SearchBack | Control::SearchFwd if self.search.take().is_some() => {
                    send(DeckCommand::ScratchEnd);
                }
                Control::TagTrack => {
                    if let Some(held) = self.tag_held.take() {
                        nav.push(if held >= TAG_HOLD_S {
                            Nav::Untag
                        } else {
                            Nav::Tag
                        });
                    }
                }
                _ => {}
            },
            ControlEvent::Turn(steps) => {
                let step = if steps > 0 { Nav::Down } else { Nav::Up };
                nav.extend(std::iter::repeat_n(step, steps.unsigned_abs() as usize));
            }
            ControlEvent::Fader(at) => {
                send(DeckCommand::SetRate(
                    1.0 + at.clamp(-1.0, 1.0) * self.range().0,
                ));
            }
            ControlEvent::JogTouch { plate } => {
                if plate && self.vinyl {
                    send(DeckCommand::ScratchBegin);
                    self.jog = Some(Jog::Scratch);
                } else {
                    self.jog = Some(Jog::Bend { base: deck.rate });
                }
            }
            ControlEvent::JogTurn(radians) => match self.jog {
                Some(Jog::Scratch) => {
                    let turns = f64::from(radians) / std::f64::consts::TAU;
                    send(DeckCommand::ScratchBy(
                        turns * SECONDS_PER_TURN * self.sample_rate,
                    ));
                }
                Some(Jog::Bend { .. }) => self.bend_angle += radians,
                None => {}
            },
            ControlEvent::JogRelease => match self.jog.take() {
                Some(Jog::Scratch) => send(DeckCommand::ScratchEnd),
                Some(Jog::Bend { base }) => {
                    self.bend = 0.0;
                    self.bend_angle = 0.0;
                    send(DeckCommand::SetRate(base));
                }
                None => {}
            },
        }
    }

    /// Once per frame, `dt` seconds after the last: held SEARCH scrubs, a jog bend follows the
    /// jog's speed and relaxes when it stops, and a held TAG TRACK counts towards its long press.
    pub fn tick(&mut self, dt: f64, out: &mut Vec<DeckCommand>) {
        let dt = dt.clamp(0.0, 0.1);
        if let Some(held) = &mut self.tag_held {
            *held += dt;
        }
        if let Some(dir) = self.search {
            out.push(DeckCommand::ScratchBy(
                dir * SEARCH_RATE * dt * self.sample_rate,
            ));
        }
        if let Some(Jog::Bend { base }) = self.jog
            && dt > 0.0
        {
            let turns_per_s = f64::from(self.bend_angle) / std::f64::consts::TAU / dt;
            self.bend_angle = 0.0;
            let target = (turns_per_s * BEND_PER_TURN_PER_S).clamp(-MAX_BEND, MAX_BEND);
            let k = 1.0 - (-dt / BEND_RELAX_S).exp();
            let bend = self.bend + (target - self.bend) * k;
            if (bend - self.bend).abs() > 1e-5 {
                self.bend = bend;
                out.push(DeckCommand::SetRate(base * (1.0 + bend)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn deck() -> DeckNow {
        DeckNow {
            rate: 1.0,
            quantize_on: false,
        }
    }

    /// Run `events` on `s`: the effects (as text) and the navs.
    fn run(s: &mut Surface, d: &DeckNow, events: &[ControlEvent]) -> (String, Vec<Nav>) {
        let (mut fx, mut nav) = (Vec::new(), Vec::new());
        for &e in events {
            s.event(e, d, &mut fx, &mut nav);
        }
        (format!("{fx:?}"), nav)
    }

    fn deck_cmds(c: &[DeckCommand]) -> String {
        format!("{:?}", c.to_vec())
    }

    #[test]
    fn the_browse_knob_pushes_turns_and_back_goes_up() {
        let mut s = Surface::new(SR);
        let (fx, nav) = run(
            &mut s,
            &deck(),
            &[
                ControlEvent::down(Control::Browse),
                ControlEvent::Up(Control::Browse),
                ControlEvent::Turn(2),
                ControlEvent::Turn(-1),
                ControlEvent::down(Control::Back),
            ],
        );
        assert_eq!(fx, "[]");
        assert_eq!(nav, [Nav::Push, Nav::Down, Nav::Down, Nav::Up, Nav::Back]);
    }

    #[test]
    fn tag_track_tags_and_a_long_press_removes() {
        let mut s = Surface::new(SR);
        let d = deck();
        let mut fx = Vec::new();
        let mut nav = Vec::new();
        s.event(ControlEvent::down(Control::TagTrack), &d, &mut fx, &mut nav);
        s.tick(0.1, &mut fx);
        s.event(ControlEvent::Up(Control::TagTrack), &d, &mut fx, &mut nav);
        s.event(ControlEvent::down(Control::TagTrack), &d, &mut fx, &mut nav);
        for _ in 0..8 {
            s.tick(0.1, &mut fx);
        }
        s.event(ControlEvent::Up(Control::TagTrack), &d, &mut fx, &mut nav);
        assert_eq!(nav, [Nav::Tag, Nav::Untag]);
    }

    #[test]
    fn cue_holds_and_releases() {
        let mut s = Surface::new(SR);
        let (fx, _) = run(
            &mut s,
            &deck(),
            &[
                ControlEvent::down(Control::Cue),
                ControlEvent::Up(Control::Cue),
            ],
        );
        assert_eq!(
            fx,
            deck_cmds(&[DeckCommand::CuePress, DeckCommand::CueRelease])
        );
    }

    #[test]
    fn pads_set_and_delete_hot_cues() {
        let mut s = Surface::new(SR);
        let (fx, _) = run(
            &mut s,
            &deck(),
            &[
                ControlEvent::down(Control::Pad(0)),
                ControlEvent::Down {
                    control: Control::Pad(5),
                    alt: true,
                },
            ],
        );
        assert_eq!(
            fx,
            deck_cmds(&[DeckCommand::HotCue(0), DeckCommand::DeleteHotCue(5)])
        );
    }

    #[test]
    fn the_fader_sets_the_rate_up_is_slower_within_the_range() {
        let mut s = Surface::new(SR);
        // Range ±10 by default: the top of the travel is −10 %.
        let (fx, _) = run(
            &mut s,
            &deck(),
            &[
                ControlEvent::Fader(-1.0),
                ControlEvent::Fader(0.5),
                ControlEvent::down(Control::TempoRange),
                ControlEvent::Fader(0.5),
            ],
        );
        assert_eq!(
            fx,
            deck_cmds(&[
                DeckCommand::SetRate(0.9),
                DeckCommand::SetRate(1.05),
                DeckCommand::SetRate(1.08),
            ])
        );
        assert_eq!(s.range().1, "±16");
    }

    #[test]
    fn a_quarter_turn_of_the_plate_scratches_a_quarter_of_1_8_s() {
        let mut s = Surface::new(SR);
        let d = deck();
        let (mut fx, mut nav) = (Vec::new(), Vec::new());
        s.event(
            ControlEvent::JogTouch { plate: true },
            &d,
            &mut fx,
            &mut nav,
        );
        s.event(
            ControlEvent::JogTurn(std::f32::consts::FRAC_PI_2),
            &d,
            &mut fx,
            &mut nav,
        );
        s.event(ControlEvent::JogRelease, &d, &mut fx, &mut nav);
        let frames = 0.25 * SECONDS_PER_TURN * f64::from(SR);
        match fx.as_slice() {
            [
                DeckCommand::ScratchBegin,
                DeckCommand::ScratchBy(f),
                DeckCommand::ScratchEnd,
            ] => assert!((f - frames).abs() < 1.0, "{f}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn in_cdj_mode_the_plate_bends() {
        let mut s = Surface::new(SR);
        let (fx, _) = run(
            &mut s,
            &deck(),
            &[
                ControlEvent::down(Control::JogMode),
                ControlEvent::JogTouch { plate: true },
            ],
        );
        assert_eq!(fx, "[]");
        assert_eq!(s.jog(), Some(Jog::Bend { base: 1.0 }));
    }

    #[test]
    fn the_ring_bends_and_letting_go_restores_the_rate() {
        let mut s = Surface::new(SR);
        let d = DeckNow {
            rate: 1.02,
            ..deck()
        };
        let (mut fx, mut nav) = (Vec::new(), Vec::new());
        s.event(
            ControlEvent::JogTouch { plate: false },
            &d,
            &mut fx,
            &mut nav,
        );
        s.event(
            ControlEvent::JogTurn(std::f32::consts::FRAC_PI_2),
            &d,
            &mut fx,
            &mut nav,
        );
        assert!(fx.is_empty());
        s.tick(0.05, &mut fx);
        match fx.as_slice() {
            [DeckCommand::SetRate(r)] => assert!(*r > 1.02, "{r}"),
            other => panic!("{other:?}"),
        }
        fx.clear();
        s.event(ControlEvent::JogRelease, &d, &mut fx, &mut nav);
        assert_eq!(format!("{fx:?}"), deck_cmds(&[DeckCommand::SetRate(1.02)]));
    }

    #[test]
    fn search_scrubs_while_held() {
        let mut s = Surface::new(SR);
        let d = deck();
        let (mut fx, mut nav) = (Vec::new(), Vec::new());
        s.event(
            ControlEvent::down(Control::SearchFwd),
            &d,
            &mut fx,
            &mut nav,
        );
        s.tick(0.01, &mut fx);
        s.event(ControlEvent::Up(Control::SearchFwd), &d, &mut fx, &mut nav);
        match fx.as_slice() {
            [
                DeckCommand::ScratchBegin,
                DeckCommand::ScratchBy(f),
                DeckCommand::ScratchEnd,
            ] => assert!((f - 4.0 * 0.01 * f64::from(SR)).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
    }
}
