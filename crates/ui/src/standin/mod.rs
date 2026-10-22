//! The hardware stand-in: a debug shell that draws the player's physical controls around the
//! device screen, so the prototype can be played on a laptop before the control surface exists.
//! Layout from the canvas artboard "8 · Hardware stand-in" (1512×950, after the reference
//! player's control layout).
//!
//! It is a separate layer (`render::ShellFrame`): the device screen is rendered on its own and
//! placed into [`DISPLAY`]. Everything here is behind the `standin` cargo feature; a device
//! build leaves it out, and the RP2040 control surface sends the same control events instead.
//!
//! The shell only knows where its controls are: a pointer on them becomes [`ControlEvent`]s,
//! and what those do is `app/surface.rs`, shared with the dev keyboard and the hardware.
//!
//! - **Buttons:** CUE (hold to preview), PLAY/PAUSE, REV, QUANTIZE, KEYLOCK, loop IN / OUT /
//!   RELOOP, hot cue pads A–H (shift or right click deletes), track search |◀◀ (back to the
//!   start).
//! - **SEARCH ◀◀ ▶▶:** while held, the track is scrubbed at 4x.
//! - **Jog:** dragging the top plate scratches (one turn = 1.8 s, as a record at 33⅓ rpm);
//!   dragging the ring bends the pitch by its speed. With JOG MODE off (CDJ mode) the top plate
//!   bends too.
//! - **Tempo fader:** drag; up is slower, as on the reference. TEMPO cycles ±6 / ±10 / ±16 / WIDE.
//!
//! The shell has its own colours (`theme.shell`) and type sizes: it is a debug tool, not part
//! of the device's look. This file turns the pointer into control events; `layout.rs` places
//! the controls and `draw.rs` draws them.

mod draw;
mod layout;

use crate::input::{Control, ControlEvent, Jog};
use crate::kit::{Hits, Rect};

/// The shell's design size.
pub const SIZE: (f32, f32) = (1512.0, 950.0);
/// Where the 1024×600 device screen goes, in the shell's design pixels.
pub const DISPLAY: Rect = Rect::new(242.0, 12.0, 1024.0, 600.0);

const JOG_CENTER: (f32, f32) = (737.0, 781.0);
const JOG_RADIUS: f32 = 153.0;
const PLATE_RADIUS: f32 = 125.0;
/// The fader's travel: the cap's centre at rate 0 and the travel either way.
const FADER_CENTER_Y: f32 = 685.0;
const FADER_TRAVEL: f32 = 211.0;
const FADER: Rect = Rect::new(1352.0, 440.0, 76.0, 490.0);

/// What the shell shows of the deck.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellView {
    pub playing: bool,
    pub reverse: bool,
    pub keylock: bool,
    pub quantize: bool,
    pub looping: bool,
    pub hot_cues: u8,
    pub rate: f64,
    /// Playhead (seconds), for the jog's position marker.
    pub position: f64,
    /// The tempo fader's range (fraction either way) and its label.
    pub range: (f64, &'static str),
    /// Jog mode VINYL (else CDJ), and the hand on the jog, if any.
    pub vinyl: bool,
    pub jog: Option<Jog>,
}

/// A touchable part of the shell: a control, or the knob's ▲ ▼, the fader or the jog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    TimeMode,
    Quantize,
    Rev,
    TrackBack,
    TrackNext,
    SearchBack,
    SearchFwd,
    Cue,
    Play,
    Back,
    TagTrack,
    /// The browse knob: push loads, turning (a vertical drag, or the wheel) scrolls.
    Browse,
    /// The knob's ▲ and ▼: one step.
    BrowseUp,
    BrowseDown,
    F1,
    F2,
    Fader,
    TempoRange,
    Keylock,
    JogMode,
    LoopIn,
    LoopOut,
    Reloop,
    Pad(u8),
    Jog,
}

/// What the pointer holds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Held {
    Button(Part),
    Fader,
    /// The jog, last at `angle`.
    Jog {
        angle: f32,
    },
    /// The browse knob, grabbed at `y`; `turned` once a drag stepped it.
    Knob {
        y: f32,
        turned: bool,
    },
}

/// Vertical drag on the knob per step.
const KNOB_STEP: f32 = 18.0;

/// The stand-in's state: where its controls were drawn, and what the pointer holds.
pub struct Standin {
    held: Option<Held>,
    hits: Hits<Part>,
}

fn angle_of(x: f32, y: f32) -> f32 {
    (y - JOG_CENTER.1).atan2(x - JOG_CENTER.0)
}

/// The shortest signed difference between two angles.
fn wrap(d: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (d + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

impl Part {
    /// The player's control this part is, if it is one.
    fn control(self) -> Option<Control> {
        Some(match self {
            Part::TimeMode => Control::TimeMode,
            Part::Quantize => Control::Quantize,
            Part::Rev => Control::Rev,
            Part::TrackBack => Control::TrackBack,
            Part::TrackNext => Control::TrackNext,
            Part::SearchBack => Control::SearchBack,
            Part::SearchFwd => Control::SearchFwd,
            Part::Cue => Control::Cue,
            Part::Play => Control::Play,
            Part::Back => Control::Back,
            Part::TagTrack => Control::TagTrack,
            Part::F1 => Control::F1,
            Part::F2 => Control::F2,
            Part::TempoRange => Control::TempoRange,
            Part::Keylock => Control::Keylock,
            Part::JogMode => Control::JogMode,
            Part::LoopIn => Control::LoopIn,
            Part::LoopOut => Control::LoopOut,
            Part::Reloop => Control::Reloop,
            Part::Pad(i) => Control::Pad(i),
            Part::Browse | Part::BrowseUp | Part::BrowseDown | Part::Fader | Part::Jog => {
                return None;
            }
        })
    }
}

/// The fader's position (-1 top .. 1 bottom) at design y `y`.
fn fader_at(y: f32) -> f64 {
    f64::from(((y - FADER_CENTER_Y) / FADER_TRAVEL).clamp(-1.0, 1.0))
}

impl Standin {
    pub fn new() -> Self {
        Self {
            held: None,
            hits: Hits::default(),
        }
    }

    /// The wheel turned by `steps` (positive = down) over (`x`, `y`): over the browse knob,
    /// it turns the knob.
    pub fn wheel(&mut self, x: f32, y: f32, steps: i32) -> Option<ControlEvent> {
        matches!(
            self.part_at(x, y),
            Some(Part::Browse | Part::BrowseUp | Part::BrowseDown)
        )
        .then_some(ControlEvent::Turn(steps))
    }

    fn part_at(&self, x: f32, y: f32) -> Option<Part> {
        let (dx, dy) = (x - JOG_CENTER.0, y - JOG_CENTER.1);
        if (dx * dx + dy * dy).sqrt() <= JOG_RADIUS {
            return Some(Part::Jog);
        }
        self.hits.find(x, y).map(|h| h.target)
    }

    /// A press at shell design pixels (`x`, `y`); `alt` = shift or the right button.
    pub fn press(&mut self, x: f32, y: f32, alt: bool) -> Vec<ControlEvent> {
        let Some(part) = self.part_at(x, y) else {
            return Vec::new();
        };
        let (held, events) = match part {
            Part::Fader => (Held::Fader, vec![ControlEvent::Fader(fader_at(y))]),
            Part::Jog => {
                let (dx, dy) = (x - JOG_CENTER.0, y - JOG_CENTER.1);
                let plate = (dx * dx + dy * dy).sqrt() <= PLATE_RADIUS;
                let angle = angle_of(x, y);
                (Held::Jog { angle }, vec![ControlEvent::JogTouch { plate }])
            }
            Part::Browse => (Held::Knob { y, turned: false }, Vec::new()),
            Part::BrowseUp => (Held::Button(part), vec![ControlEvent::Turn(-1)]),
            Part::BrowseDown => (Held::Button(part), vec![ControlEvent::Turn(1)]),
            _ => (
                Held::Button(part),
                part.control()
                    .map(|control| ControlEvent::Down { control, alt })
                    .into_iter()
                    .collect(),
            ),
        };
        self.held = Some(held);
        events
    }

    /// The pointer moved to (`x`, `y`) while held.
    pub fn drag(&mut self, x: f32, y: f32) -> Vec<ControlEvent> {
        match self.held {
            Some(Held::Fader) => vec![ControlEvent::Fader(fader_at(y))],
            Some(Held::Jog { angle }) => {
                let a = angle_of(x, y);
                self.held = Some(Held::Jog { angle: a });
                vec![ControlEvent::JogTurn(wrap(a - angle))]
            }
            Some(Held::Knob { y: from, .. }) => {
                let steps = ((y - from) / KNOB_STEP).trunc();
                if steps == 0.0 {
                    return Vec::new();
                }
                self.held = Some(Held::Knob {
                    y: from + steps * KNOB_STEP,
                    turned: true,
                });
                vec![ControlEvent::Turn(steps as i32)]
            }
            _ => Vec::new(),
        }
    }

    /// The pointer came up.
    pub fn release(&mut self) -> Vec<ControlEvent> {
        match self.held.take() {
            Some(Held::Button(part)) => part.control().map(ControlEvent::Up).into_iter().collect(),
            Some(Held::Jog { .. }) => vec![ControlEvent::JogRelease],
            // A push without turning.
            Some(Held::Knob { turned: false, .. }) => vec![
                ControlEvent::down(Control::Browse),
                ControlEvent::Up(Control::Browse),
            ],
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    fn is_down(&self, part: Part) -> bool {
        is_down(self.held, part)
    }
}

/// `part` is held down, when the pointer holds `held`.
fn is_down(held: Option<Held>, part: Part) -> bool {
    match held {
        Some(Held::Button(p)) => p == part,
        Some(Held::Fader) => part == Part::Fader,
        Some(Held::Jog { .. }) => part == Part::Jog,
        Some(Held::Knob { .. }) => part == Part::Browse,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::scene::Scene;
    use crate::theme::Theme;

    fn shell() -> Standin {
        let mut s = Standin::new();
        s.build(
            &ShellView::default(),
            &Theme::default(),
            &mut Scene::default(),
        );
        s
    }

    fn tap(s: &mut Standin, x: f32, y: f32) -> Vec<ControlEvent> {
        let mut e = s.press(x, y, false);
        e.extend(s.release());
        e
    }

    #[test]
    fn the_browse_knob_pushes_turns_and_back_goes_up() {
        let mut s = shell();
        // A push without turning pushes.
        assert_eq!(
            tap(&mut s, 1388.0, 202.0),
            [
                ControlEvent::down(Control::Browse),
                ControlEvent::Up(Control::Browse)
            ]
        );
        // A turn (vertical drag) steps and does not push.
        assert!(s.press(1388.0, 202.0, false).is_empty());
        assert_eq!(
            s.drag(1388.0, 202.0 + 2.5 * KNOB_STEP),
            [ControlEvent::Turn(2)]
        );
        assert_eq!(
            s.drag(1388.0, 202.0 + 0.9 * KNOB_STEP),
            [ControlEvent::Turn(-1)]
        );
        assert!(s.release().is_empty());
        // ▲ ▼, the wheel over the knob (not elsewhere), BACK.
        assert_eq!(tap(&mut s, 1388.0, 120.0), [ControlEvent::Turn(-1)]);
        assert_eq!(tap(&mut s, 1388.0, 280.0), [ControlEvent::Turn(1)]);
        assert_eq!(s.wheel(1388.0, 202.0, -2), Some(ControlEvent::Turn(-2)));
        assert_eq!(s.wheel(700.0, 300.0, 3), None);
        assert_eq!(
            tap(&mut s, 1340.0, 60.0),
            [
                ControlEvent::down(Control::Back),
                ControlEvent::Up(Control::Back)
            ]
        );
    }

    #[test]
    fn buttons_go_down_and_up_and_alt_rides_along() {
        let mut s = shell();
        assert_eq!(
            tap(&mut s, 121.0, 618.0),
            [
                ControlEvent::down(Control::Cue),
                ControlEvent::Up(Control::Cue)
            ]
        );
        assert_eq!(
            s.press(364.0, 887.0, true),
            [ControlEvent::Down {
                control: Control::Pad(5),
                alt: true
            }]
        );
        assert!(s.is_down(Part::Pad(5)));
        s.release();
        assert!(!s.is_down(Part::Pad(5)));
    }

    #[test]
    fn the_fader_reports_its_travel_top_is_minus_one() {
        let mut s = shell();
        assert_eq!(
            s.press(1390.0, FADER_CENTER_Y - FADER_TRAVEL, false),
            [ControlEvent::Fader(-1.0)]
        );
        assert_eq!(
            s.drag(1390.0, FADER_CENTER_Y + FADER_TRAVEL * 0.5),
            [ControlEvent::Fader(0.5)]
        );
        assert!(s.release().is_empty());
    }

    #[test]
    fn the_jog_tells_plate_from_ring_and_turns_clockwise_positive() {
        let mut s = shell();
        let (cx, cy) = JOG_CENTER;
        assert_eq!(
            s.press(cx + 80.0, cy, false),
            [ControlEvent::JogTouch { plate: true }]
        );
        // From 3 o'clock to 6 o'clock (screen y down): a quarter turn clockwise.
        match s.drag(cx, cy + 80.0).as_slice() {
            [ControlEvent::JogTurn(a)] => {
                assert!((a - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "{a}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(s.release(), [ControlEvent::JogRelease]);
        assert_eq!(
            s.press(cx + 140.0, cy, false),
            [ControlEvent::JogTouch { plate: false }]
        );
    }

    #[test]
    fn the_display_slot_is_not_a_control_and_fits_the_shell() {
        let s = shell();
        let Rect { x, y, w, h } = DISPLAY;
        assert!(s.part_at(x + w * 0.5, y + h * 0.5).is_none());
        assert!(x + w <= SIZE.0 && y + h <= SIZE.1);
    }
}
