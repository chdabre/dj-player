//! The hardware stand-in: a debug shell that draws the player's physical controls around the
//! device screen, so the prototype can be played on a laptop before the control surface exists.
//! Layout from the canvas artboard "8 · Hardware stand-in" (1512×950, after the reference
//! player's control layout).
//!
//! It is a separate layer (`gfx::ShellFrame`): the device screen is rendered on its own and
//! placed into [`DISPLAY`]. Everything here is behind the `standin` cargo feature; a device
//! build leaves it out, and the RP2040 control surface sends the same deck commands instead.
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
//! The shell has its own colours: it is a debug tool, not part of the device theme.

use engine_core::{DeckCommand, HOT_CUES};

use crate::browse::Nav;
use crate::scene::{Align, Rgba, Scene, Text, Weight};

/// The shell's design size.
pub const SIZE: (f32, f32) = (1512.0, 950.0);
/// Where the 1024×600 device screen goes, in the shell's design pixels.
pub const DISPLAY: (f32, f32, f32, f32) = (242.0, 12.0, 1024.0, 600.0);

/// Tempo fader ranges (fraction of the rate).
const RANGES: [(f64, &str); 4] = [(0.06, "±6"), (0.10, "±10"), (0.16, "±16"), (1.0, "WIDE")];
/// Seconds of audio per jog turn (a record at 33⅓ rpm).
const SECONDS_PER_TURN: f64 = 1.8;
/// SEARCH speed while held.
const SEARCH_RATE: f64 = 4.0;
/// Pitch bend per jog turn per second, and its limit.
const BEND_PER_TURN_PER_S: f64 = 0.08;
const MAX_BEND: f64 = 0.25;
/// How fast a bend relaxes when the jog stops moving (s).
const BEND_RELAX_S: f64 = 0.08;

const JOG_CENTER: (f32, f32) = (737.0, 781.0);
const JOG_RADIUS: f32 = 153.0;
const PLATE_RADIUS: f32 = 125.0;
/// The fader's travel: the cap's centre at rate 0 and the travel either way.
const FADER_CENTER_Y: f32 = 685.0;
const FADER_TRAVEL: f32 = 211.0;
const FADER: (f32, f32, f32, f32) = (1352.0, 440.0, 76.0, 490.0);

// Colours of the stand-in (not the device theme).
const BODY: Rgba = Rgba::hex(0x141619);
const PANEL: Rgba = Rgba::hex(0x0b0c0e);
const PANEL_EDGE: Rgba = Rgba::hex(0x26292e);
const EDGE: Rgba = Rgba::hex(0x3a3d42);
const EDGE_LIGHT: Rgba = Rgba::hex(0x5f6369);
const BUTTON_TOP: Rgba = Rgba::hex(0x2a2d32);
const BUTTON_BOTTOM: Rgba = Rgba::hex(0x1d1f23);
const BUTTON_DOWN: Rgba = Rgba::hex(0x3a3e45);
const TEXT: Rgba = Rgba::hex(0xe6e6e6);
const LABEL: Rgba = Rgba::hex(0xc9ccd1);
const SUB: Rgba = Rgba::hex(0x8f939a);
const KEY_FILL: Rgba = Rgba::hex(0x1a1c20);
const KEY_TEXT: Rgba = Rgba::hex(0x7cb4ff);
const AMBER: Rgba = Rgba::hex(0xe0a33a);
const AMBER_TEXT: Rgba = Rgba::hex(0xf2c06a);
const AMBER_DIM: Rgba = Rgba::hex(0x5a4420);
const GREEN: Rgba = Rgba::hex(0x3fbf6a);
const GREEN_TEXT: Rgba = Rgba::hex(0x7ee0a0);
const GREEN_DIM: Rgba = Rgba::hex(0x1d4a2c);
const RED_EDGE: Rgba = Rgba::hex(0x8a3040);
const RED_FILL: Rgba = Rgba::hex(0x3a141b);
const RED_LIT: Rgba = Rgba::hex(0x7a2232);
const RED_TEXT: Rgba = Rgba::hex(0xffb3bf);
const BLUE: Rgba = Rgba::hex(0x2b7fff);
const BLUE_FILL: Rgba = Rgba::hex(0x0b2350);
const BLUE_TEXT: Rgba = Rgba::hex(0x7cb4ff);
const KNOB: Rgba = Rgba::hex(0xb9bcc1);
const JOG_RING: Rgba = Rgba::hex(0x24272b);
const JOG_PLATE: Rgba = Rgba::hex(0x121316);
const BLACK: Rgba = Rgba::hex(0x000000);
const PAD_COLORS: [Rgba; 8] = [
    Rgba::hex(0xe8667a),
    Rgba::hex(0xe0a33a),
    Rgba::hex(0x5ab8ec),
    Rgba::hex(0x6cc08b),
    Rgba::hex(0xa78bfa),
    Rgba::hex(0xf0762a),
    Rgba::hex(0x3fb8ae),
    Rgba::hex(0xe878c8),
];

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
}

/// A control on the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
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

#[derive(Debug, Clone, Copy)]
struct Hit {
    control: Control,
    rect: (f32, f32, f32, f32),
}

/// What is held down.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Held {
    Button(Control),
    Search(f64),
    Fader,
    /// The jog: scratching (top plate in vinyl mode) or bending, and the last angle.
    Scratch {
        angle: f32,
    },
    Bend {
        angle: f32,
        base: f64,
    },
    /// TAG TRACK, held this long (seconds): a long press removes.
    Tag {
        held: f64,
    },
    /// The browse knob, grabbed at `y`; `turned` once a drag stepped it.
    Knob {
        y: f32,
        turned: bool,
    },
}

/// TAG TRACK held this long removes the track from the tag list.
const TAG_HOLD_S: f64 = 0.6;

/// Vertical drag on the knob per step.
const KNOB_STEP: f32 = 18.0;

/// The stand-in's state: fader range, jog mode, what is held.
pub struct Standin {
    sample_rate: f64,
    range: usize,
    vinyl: bool,
    held: Option<Held>,
    hits: Vec<Hit>,
    /// The current pitch bend (fraction) and the rate it bends from.
    bend: f64,
    bend_base: f64,
    /// Jog angle travelled since the last tick, for the bend speed.
    bend_angle: f32,
    /// Browse requests since the last [`Standin::take_nav`].
    nav: Vec<Nav>,
}

fn angle_of(x: f32, y: f32) -> f32 {
    (y - JOG_CENTER.1).atan2(x - JOG_CENTER.0)
}

/// The shortest signed difference between two angles.
fn wrap(d: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (d + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

impl Standin {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            range: 1,
            vinyl: true,
            held: None,
            hits: Vec::new(),
            bend: 0.0,
            bend_base: 1.0,
            bend_angle: 0.0,
            nav: Vec::new(),
        }
    }

    /// The browse requests since the last call.
    pub fn take_nav(&mut self) -> Vec<Nav> {
        std::mem::take(&mut self.nav)
    }

    /// The wheel turned by `steps` (positive = down) over (`x`, `y`): over the browse knob,
    /// it turns the knob.
    pub fn wheel(&mut self, x: f32, y: f32, steps: i32) {
        if matches!(
            self.control_at(x, y),
            Some(Control::Browse | Control::BrowseUp | Control::BrowseDown)
        ) {
            let step = if steps > 0 { Nav::Down } else { Nav::Up };
            self.nav
                .extend(std::iter::repeat_n(step, steps.unsigned_abs() as usize));
        }
    }

    fn range(&self) -> f64 {
        RANGES[self.range].0
    }

    fn control_at(&self, x: f32, y: f32) -> Option<Control> {
        let (dx, dy) = (x - JOG_CENTER.0, y - JOG_CENTER.1);
        if (dx * dx + dy * dy).sqrt() <= JOG_RADIUS {
            return Some(Control::Jog);
        }
        self.hits
            .iter()
            .find(|h| {
                let (hx, hy, hw, hh) = h.rect;
                x >= hx && x < hx + hw && y >= hy && y < hy + hh
            })
            .map(|h| h.control)
    }

    /// The rate the fader stands for at design y `y`.
    fn fader_rate(&self, y: f32) -> f64 {
        let v = f64::from(((y - FADER_CENTER_Y) / FADER_TRAVEL).clamp(-1.0, 1.0));
        1.0 + v * self.range()
    }

    /// A press at shell design pixels (`x`, `y`); `alt` = shift or the right button.
    pub fn press(&mut self, x: f32, y: f32, alt: bool, view: &ShellView) -> Vec<DeckCommand> {
        let Some(control) = self.control_at(x, y) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let held = match control {
            Control::Cue => {
                out.push(DeckCommand::CuePress);
                Held::Button(control)
            }
            Control::Play => {
                out.push(DeckCommand::TogglePlay);
                Held::Button(control)
            }
            Control::Rev => {
                out.push(DeckCommand::ToggleReverse);
                Held::Button(control)
            }
            Control::Quantize => {
                out.push(DeckCommand::SetQuantize((!view.quantize).then_some(1.0)));
                Held::Button(control)
            }
            Control::Keylock => {
                out.push(DeckCommand::ToggleKeylock);
                Held::Button(control)
            }
            Control::LoopIn => {
                out.push(DeckCommand::LoopIn);
                Held::Button(control)
            }
            Control::LoopOut => {
                out.push(DeckCommand::LoopOut);
                Held::Button(control)
            }
            Control::Reloop => {
                out.push(DeckCommand::ReloopExit);
                Held::Button(control)
            }
            Control::TrackBack => {
                out.push(DeckCommand::Seek(0.0));
                Held::Button(control)
            }
            Control::Pad(i) => {
                let i = usize::from(i);
                if i < HOT_CUES {
                    out.push(if alt {
                        DeckCommand::DeleteHotCue(i)
                    } else {
                        DeckCommand::HotCue(i)
                    });
                }
                Held::Button(control)
            }
            Control::SearchBack | Control::SearchFwd => {
                out.push(DeckCommand::ScratchBegin);
                let dir = if control == Control::SearchBack {
                    -1.0
                } else {
                    1.0
                };
                Held::Search(dir)
            }
            Control::TempoRange => {
                self.range = (self.range + 1) % RANGES.len();
                Held::Button(control)
            }
            Control::JogMode => {
                self.vinyl = !self.vinyl;
                Held::Button(control)
            }
            Control::Fader => {
                out.push(DeckCommand::SetRate(self.fader_rate(y)));
                Held::Fader
            }
            Control::Jog => {
                let (dx, dy) = (x - JOG_CENTER.0, y - JOG_CENTER.1);
                let on_plate = (dx * dx + dy * dy).sqrt() <= PLATE_RADIUS;
                let angle = angle_of(x, y);
                if on_plate && self.vinyl {
                    out.push(DeckCommand::ScratchBegin);
                    Held::Scratch { angle }
                } else {
                    self.bend_base = view.rate;
                    Held::Bend {
                        angle,
                        base: view.rate,
                    }
                }
            }
            Control::Back => {
                self.nav.push(Nav::Back);
                Held::Button(control)
            }
            Control::BrowseUp => {
                self.nav.push(Nav::Up);
                Held::Button(control)
            }
            Control::BrowseDown => {
                self.nav.push(Nav::Down);
                Held::Button(control)
            }
            Control::Browse => Held::Knob { y, turned: false },
            // Present but not wired yet: no time mode, tags or shortcuts.
            Control::TagTrack => Held::Tag { held: 0.0 },
            Control::TimeMode | Control::TrackNext | Control::F1 | Control::F2 => {
                Held::Button(control)
            }
        };
        self.held = Some(held);
        out
    }

    /// The pointer moved to (`x`, `y`) while held.
    pub fn drag(&mut self, x: f32, y: f32) -> Vec<DeckCommand> {
        match self.held {
            Some(Held::Fader) => vec![DeckCommand::SetRate(self.fader_rate(y))],
            Some(Held::Scratch { angle }) => {
                let a = angle_of(x, y);
                let d = wrap(a - angle);
                self.held = Some(Held::Scratch { angle: a });
                let frames =
                    f64::from(d) / std::f64::consts::TAU * SECONDS_PER_TURN * self.sample_rate;
                vec![DeckCommand::ScratchBy(frames)]
            }
            Some(Held::Bend { angle, base }) => {
                let a = angle_of(x, y);
                self.bend_angle += wrap(a - angle);
                self.held = Some(Held::Bend { angle: a, base });
                Vec::new()
            }
            Some(Held::Knob { y: from, .. }) => {
                let steps = ((y - from) / KNOB_STEP).trunc();
                if steps != 0.0 {
                    let step = if steps > 0.0 { Nav::Down } else { Nav::Up };
                    self.nav
                        .extend(std::iter::repeat_n(step, steps.abs() as usize));
                    self.held = Some(Held::Knob {
                        y: from + steps * KNOB_STEP,
                        turned: true,
                    });
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The pointer came up.
    pub fn release(&mut self) -> Vec<DeckCommand> {
        match self.held.take() {
            Some(Held::Button(Control::Cue)) => vec![DeckCommand::CueRelease],
            Some(Held::Search(_)) | Some(Held::Scratch { .. }) => vec![DeckCommand::ScratchEnd],
            Some(Held::Bend { base, .. }) => {
                self.bend = 0.0;
                vec![DeckCommand::SetRate(base)]
            }
            Some(Held::Knob { turned: false, .. }) => {
                self.nav.push(Nav::Push);
                Vec::new()
            }
            Some(Held::Tag { held }) => {
                self.nav.push(if held >= TAG_HOLD_S {
                    Nav::Untag
                } else {
                    Nav::Tag
                });
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Once per frame, `dt` seconds after the last: held SEARCH scrubs, and a jog bend follows
    /// the jog's speed and relaxes when it stops.
    pub fn tick(&mut self, dt: f64) -> Vec<DeckCommand> {
        let dt = dt.clamp(0.0, 0.1);
        if let Some(Held::Tag { held }) = &mut self.held {
            *held += dt;
        }
        match self.held {
            Some(Held::Search(dir)) => {
                vec![DeckCommand::ScratchBy(
                    dir * SEARCH_RATE * dt * self.sample_rate,
                )]
            }
            Some(Held::Bend { base, .. }) if dt > 0.0 => {
                let turns_per_s = f64::from(self.bend_angle) / std::f64::consts::TAU / dt;
                self.bend_angle = 0.0;
                let target = (turns_per_s * BEND_PER_TURN_PER_S).clamp(-MAX_BEND, MAX_BEND);
                let k = 1.0 - (-dt / BEND_RELAX_S).exp();
                let bend = self.bend + (target - self.bend) * k;
                if (bend - self.bend).abs() > 1e-5 {
                    self.bend = bend;
                    return vec![DeckCommand::SetRate(base * (1.0 + bend))];
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn is_down(&self, control: Control) -> bool {
        match self.held {
            Some(Held::Button(c)) => c == control,
            Some(Held::Search(d)) => {
                (d < 0.0 && control == Control::SearchBack)
                    || (d > 0.0 && control == Control::SearchFwd)
            }
            Some(Held::Fader) => control == Control::Fader,
            Some(Held::Scratch { .. } | Held::Bend { .. }) => control == Control::Jog,
            Some(Held::Knob { .. }) => control == Control::Browse,
            Some(Held::Tag { .. }) => control == Control::TagTrack,
            None => false,
        }
    }

    /// Lay out the shell into `scene`.
    pub fn build(&mut self, v: &ShellView, scene: &mut Scene) {
        scene.clear();
        scene.background = BODY;
        let mut d = Draw {
            scene,
            hits: Vec::new(),
        };

        // Panels: left column, display, bottom band, right column.
        for (x, y, w, h) in [
            (6.0, 6.0, 226.0, 938.0),
            (235.0, 6.0, 1041.0, 612.0),
            (235.0, 622.0, 1041.0, 322.0),
            (1280.0, 6.0, 226.0, 938.0),
        ] {
            d.scene.rounded(x, y, w, h, 10.0, PANEL, 1.0, PANEL_EDGE);
        }
        // The display's bezel; the device screen is copied over the inside.
        d.scene
            .rounded(236.0, 8.0, 1036.0, 608.0, 6.0, BLACK, 1.0, EDGE);

        // ----- Left column (centred on x = 121) -----
        d.label("TIME MODE", 121.0, 24.0, LABEL, 15.0, Align::Center);
        d.label("– AUTO CUE", 121.0, 42.0, SUB, 12.0, Align::Center);
        d.round(
            Control::TimeMode,
            105.0,
            62.0,
            32.0,
            self.is_down(Control::TimeMode),
        );
        d.key("t", 145.0, 69.0);
        d.label("QUANTIZE", 121.0, 114.0, LABEL, 15.0, Align::Center);
        d.round(
            Control::Quantize,
            105.0,
            136.0,
            32.0,
            self.is_down(Control::Quantize),
        );
        d.led(116.0, 147.0, if v.quantize { AMBER } else { EDGE });
        d.key("z", 145.0, 143.0);
        d.rule(31.0, 192.0, 180.0);
        d.label("DIRECTION", 121.0, 210.0, LABEL, 15.0, Align::Center);
        let rev_fill = if v.reverse { RED_LIT } else { RED_FILL };
        d.rect_button(
            Control::Rev,
            (71.0, 232.0, 100.0, 40.0),
            "REV",
            rev_fill,
            RED_EDGE,
            RED_TEXT,
            self.is_down(Control::Rev),
        );
        d.key("r", 179.0, 242.0);
        d.label("TRACK SEARCH", 121.0, 296.0, LABEL, 15.0, Align::Center);
        d.round_label(
            Control::TrackBack,
            66.0,
            318.0,
            50.0,
            "|◀◀",
            self.is_down(Control::TrackBack),
        );
        d.round_label(
            Control::TrackNext,
            126.0,
            318.0,
            50.0,
            "▶▶|",
            self.is_down(Control::TrackNext),
        );
        d.label("SEARCH", 121.0, 392.0, LABEL, 15.0, Align::Center);
        d.round_label(
            Control::SearchBack,
            66.0,
            414.0,
            50.0,
            "◀◀",
            self.is_down(Control::SearchBack),
        );
        d.round_label(
            Control::SearchFwd,
            126.0,
            414.0,
            50.0,
            "▶▶",
            self.is_down(Control::SearchFwd),
        );
        d.key("← →", 106.0, 474.0);
        d.rule(31.0, 520.0, 180.0);
        // CUE and PLAY, with their rings lit by state.
        let cue_ring = if v.playing { AMBER_DIM } else { AMBER };
        d.big(
            Control::Cue,
            69.0,
            566.0,
            "CUE",
            cue_ring,
            AMBER_TEXT,
            self.is_down(Control::Cue),
        );
        d.key("c", 184.0, 608.0);
        d.label("PLAY / PAUSE", 121.0, 706.0, LABEL, 15.0, Align::Center);
        let play_ring = if v.playing { GREEN } else { GREEN_DIM };
        d.big(
            Control::Play,
            69.0,
            734.0,
            "▶ ‖",
            play_ring,
            GREEN_TEXT,
            self.is_down(Control::Play),
        );
        d.key("space", 184.0, 776.0);

        // ----- Right column -----
        d.rect_button(
            Control::Back,
            (1290.0, 40.0, 100.0, 42.0),
            "BACK",
            BUTTON_TOP,
            EDGE,
            TEXT,
            self.is_down(Control::Back),
        );
        d.rect_button(
            Control::TagTrack,
            (1396.0, 40.0, 100.0, 42.0),
            "TAG TRACK",
            BUTTON_TOP,
            EDGE,
            TEXT,
            self.is_down(Control::TagTrack),
        );
        d.key("esc", 1290.0, 88.0);
        d.key("x  ⇧x", 1396.0, 88.0);
        d.scene
            .rounded(1312.0, 110.0, 152.0, 186.0, 14.0, BUTTON_BOTTOM, 1.0, EDGE);
        d.label("▲", 1388.0, 116.0, SUB, 12.0, Align::Center);
        let knob_fill = if self.is_down(Control::Browse) {
            BUTTON_DOWN
        } else {
            BUTTON_TOP
        };
        d.scene.rounded(
            1332.0, 146.0, 112.0, 112.0, 56.0, knob_fill, 2.0, EDGE_LIGHT,
        );
        d.scene
            .rounded(1358.0, 172.0, 60.0, 60.0, 30.0, KNOB, 0.0, KNOB);
        d.hit(Control::Browse, (1332.0, 146.0, 112.0, 112.0));
        d.hit(Control::BrowseUp, (1312.0, 110.0, 152.0, 36.0));
        d.hit(Control::BrowseDown, (1312.0, 258.0, 152.0, 38.0));
        d.label("▼", 1388.0, 268.0, SUB, 12.0, Align::Center);
        d.label(
            "BROWSE · TURN: SCROLL · PUSH: LOAD",
            1388.0,
            302.0,
            SUB,
            12.0,
            Align::Center,
        );
        d.key("↑↓ ⏎", 1334.0, 320.0);
        d.key("scroll", 1390.0, 320.0);
        d.rect_button(
            Control::F1,
            (1290.0, 350.0, 100.0, 40.0),
            "F1",
            BUTTON_TOP,
            EDGE,
            TEXT,
            self.is_down(Control::F1),
        );
        d.rect_button(
            Control::F2,
            (1396.0, 350.0, 100.0, 40.0),
            "F2",
            BUTTON_TOP,
            EDGE,
            TEXT,
            self.is_down(Control::F2),
        );
        d.rule(1290.0, 408.0, 206.0);
        d.label("TEMPO", 1390.0, 418.0, LABEL, 15.0, Align::Center);
        let (fx, fy, fw, fh) = FADER;
        d.scene.rounded(fx, fy, fw, fh, 6.0, PANEL, 1.0, EDGE);
        d.scene.fill(1388.0, 456.0, 4.0, 458.0, BLACK);
        d.label("–", 1336.0, 452.0, SUB, 12.0, Align::Left);
        d.label("0", 1334.0, 678.0, SUB, 12.0, Align::Left);
        d.scene.fill(1352.0, 685.0, 12.0, 2.0, TEXT);
        d.label("+", 1335.0, 902.0, SUB, 12.0, Align::Left);
        // The cap follows the deck's rate on this range (pitch bends move it too).
        let v_rate = ((v.rate - 1.0) / self.range()).clamp(-1.0, 1.0) as f32;
        let cap_y = FADER_CENTER_Y + v_rate * FADER_TRAVEL - 18.0;
        let cap = if self.is_down(Control::Fader) {
            BUTTON_DOWN
        } else {
            Rgba::hex(0x4a4e55)
        };
        d.scene.rounded(
            1362.0,
            cap_y,
            56.0,
            36.0,
            4.0,
            cap,
            1.0,
            Rgba::hex(0x7a7e84),
        );
        d.scene.fill(1362.0, cap_y + 17.0, 56.0, 2.0, TEXT);
        d.hit(Control::Fader, FADER);
        d.key("+ / −", 1438.0, 666.0);
        d.key("0", 1438.0, 690.0);
        d.label(
            &format!("{:+.2} %", (v.rate - 1.0) * 100.0),
            1390.0,
            932.0,
            SUB,
            12.0,
            Align::Center,
        );

        // ----- Bottom band: loop, pads, jog, jog mode / tempo / keylock -----
        d.label("IN / 4BEAT", 282.0, 632.0, SUB, 12.0, Align::Center);
        d.label("OUT", 352.0, 632.0, SUB, 12.0, Align::Center);
        d.label("RELOOP / EXIT", 440.0, 632.0, SUB, 12.0, Align::Center);
        d.scene.fill(310.0, 680.0, 14.0, 2.0, EDGE_LIGHT);
        d.scene.fill(380.0, 680.0, 40.0, 2.0, EDGE_LIGHT);
        let loop_fill = if v.looping { AMBER_TEXT } else { AMBER };
        d.amber(
            Control::LoopIn,
            254.0,
            652.0,
            56.0,
            "IN",
            loop_fill,
            self.is_down(Control::LoopIn),
        );
        d.amber(
            Control::LoopOut,
            324.0,
            652.0,
            56.0,
            "OUT",
            loop_fill,
            self.is_down(Control::LoopOut),
        );
        d.round(
            Control::Reloop,
            420.0,
            660.0,
            40.0,
            self.is_down(Control::Reloop),
        );
        d.key("i", 272.0, 716.0);
        d.key("o", 342.0, 716.0);
        d.key("l", 430.0, 716.0);
        d.key("b · [ ]", 478.0, 670.0);
        d.scene.rounded(
            242.0,
            748.0,
            322.0,
            184.0,
            8.0,
            Rgba::hex(0x0e1420),
            1.0,
            Rgba::hex(0x22457a),
        );
        d.label(
            "HOT CUE A–H · OUR PADS",
            254.0,
            756.0,
            SUB,
            12.0,
            Align::Left,
        );
        d.key("1–8 · ⇧ del", 478.0, 754.0);
        for i in 0..8u8 {
            let (col, row) = (f32::from(i % 4), f32::from(i / 4));
            let (x, y) = (254.0 + col * 76.0, 780.0 + row * 74.0);
            let set = usize::from(i) < HOT_CUES && v.hot_cues & (1 << i) != 0;
            let c = PAD_COLORS[usize::from(i)];
            let fill = if self.is_down(Control::Pad(i)) {
                BUTTON_DOWN
            } else if set {
                Rgba::hex(0x1f1d22)
            } else {
                KEY_FILL
            };
            d.scene.rounded(x, y, 68.0, 66.0, 6.0, fill, 1.0, EDGE);
            if set {
                d.scene
                    .rounded(x, y, 68.0, 66.0, 6.0, c.alpha(0.05), 0.0, c);
                d.scene.rounded(x, y, 68.0, 4.0, 2.0, c, 0.0, c);
            }
            let letter = ((b'A' + i) as char).to_string();
            d.label(&letter, x + 34.0, y + 21.0, TEXT, 20.0, Align::Center);
            d.hit(Control::Pad(i), (x, y, 68.0, 66.0));
        }

        // Jog: ring, top plate, centre display, and a marker that turns with the track.
        let (cx, cy) = JOG_CENTER;
        let ring = if matches!(self.held, Some(Held::Bend { .. })) {
            Rgba::hex(0x30343a)
        } else {
            JOG_RING
        };
        d.circle(cx, cy, JOG_RADIUS, ring, 2.0, EDGE);
        let plate = if matches!(self.held, Some(Held::Scratch { .. })) {
            Rgba::hex(0x1a1c20)
        } else {
            JOG_PLATE
        };
        d.circle(cx, cy, PLATE_RADIUS, plate, 2.0, EDGE_LIGHT);
        d.circle(cx, cy, 46.0, BLACK, 3.0, BLUE);
        let vinyl = if self.vinyl { "VINYL" } else { "CDJ" };
        d.label(vinyl, cx, cy - 9.0, TEXT, 13.0, Align::Center);
        let turn = (v.position / SECONDS_PER_TURN).fract() as f32 * std::f32::consts::TAU;
        let (mx, my) = (cx + 104.0 * turn.sin(), cy - 104.0 * turn.cos());
        d.circle(mx, my, 6.0, Rgba::hex(0xe8667a), 0.0, BLACK);
        d.label("– REV", 574.0, 918.0, SUB, 12.0, Align::Left);
        d.label("FWD +", 862.0, 918.0, SUB, 12.0, Align::Left);

        d.label("JOG MODE", 924.0, 658.0, LABEL, 15.0, Align::Left);
        let (jf, je, jt) = if self.vinyl {
            (BLUE_FILL, BLUE, BLUE_TEXT)
        } else {
            (BUTTON_TOP, EDGE, SUB)
        };
        d.rect_button(
            Control::JogMode,
            (1160.0, 646.0, 90.0, 44.0),
            "VINYL",
            jf,
            je,
            jt,
            self.is_down(Control::JogMode),
        );
        d.key("v", 1126.0, 658.0);
        d.label(
            &format!("TEMPO  {}", RANGES[self.range].1),
            924.0,
            732.0,
            LABEL,
            15.0,
            Align::Left,
        );
        d.label(
            "±6 / ±10 / ±16 / WIDE",
            924.0,
            750.0,
            SUB,
            12.0,
            Align::Left,
        );
        d.round(
            Control::TempoRange,
            1210.0,
            728.0,
            36.0,
            self.is_down(Control::TempoRange),
        );
        d.key("g", 1166.0, 737.0);
        d.label("KEYLOCK", 924.0, 812.0, LABEL, 15.0, Align::Left);
        d.round(
            Control::Keylock,
            1212.0,
            804.0,
            32.0,
            self.is_down(Control::Keylock),
        );
        d.led(1223.0, 815.0, if v.keylock { AMBER } else { EDGE });
        d.key("k", 1166.0, 810.0);
        d.rule(924.0, 880.0, 326.0);
        d.label("JOG", 924.0, 890.0, LABEL, 15.0, Align::Left);
        d.label(
            "TOP: SCRATCH · RING: PITCH BEND",
            964.0,
            892.0,
            SUB,
            12.0,
            Align::Left,
        );
        d.key("drag top", 924.0, 912.0);
        d.key("drag ring", 994.0, 912.0);

        self.hits = d.hits;
    }
}

/// Drawing helpers for the shell.
struct Draw<'s> {
    scene: &'s mut Scene,
    hits: Vec<Hit>,
}

#[allow(clippy::too_many_arguments)]
impl Draw<'_> {
    fn hit(&mut self, control: Control, rect: (f32, f32, f32, f32)) {
        self.hits.push(Hit { control, rect });
    }

    fn label(&mut self, s: &str, x: f32, y: f32, color: Rgba, size: f32, align: Align) {
        self.scene.text(Text {
            text: s.to_string(),
            x,
            y,
            size,
            line_height: 1.2,
            weight: if size >= 15.0 {
                Weight::SemiBold
            } else {
                Weight::Regular
            },
            color,
            align,
            max_width: None,
        });
    }

    /// A key cap showing the keyboard shortcut.
    fn key(&mut self, s: &str, x: f32, y: f32) {
        let w = 12.0 + 6.5 * s.chars().count() as f32;
        self.scene
            .rounded(x, y, w, 19.0, 4.0, KEY_FILL, 1.0, EDGE_LIGHT);
        self.label(s, x + w * 0.5, y + 1.5, KEY_TEXT, 12.0, Align::Center);
    }

    fn led(&mut self, x: f32, y: f32, color: Rgba) {
        self.scene.rounded(x, y, 9.0, 9.0, 4.5, color, 0.0, color);
    }

    fn rule(&mut self, x: f32, y: f32, w: f32) {
        self.scene.fill(x, y, w, 1.0, PANEL_EDGE);
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Rgba, border: f32, edge: Rgba) {
        self.scene
            .rounded(cx - r, cy - r, 2.0 * r, 2.0 * r, r, fill, border, edge);
    }

    fn round(&mut self, control: Control, x: f32, y: f32, d: f32, down: bool) {
        let fill = if down { BUTTON_DOWN } else { BUTTON_TOP };
        self.scene.rounded(x, y, d, d, d * 0.5, fill, 1.0, EDGE);
        self.hit(control, (x, y, d, d));
    }

    fn round_label(&mut self, control: Control, x: f32, y: f32, d: f32, s: &str, down: bool) {
        self.round(control, x, y, d, down);
        self.label(s, x + d * 0.5, y + d * 0.5 - 8.0, TEXT, 13.0, Align::Center);
    }

    fn amber(&mut self, control: Control, x: f32, y: f32, d: f32, s: &str, fill: Rgba, down: bool) {
        let fill = if down { AMBER_TEXT } else { fill };
        self.scene
            .rounded(x, y, d, d, d * 0.5, fill, 1.0, Rgba::hex(0x7a5a22));
        self.label(
            s,
            x + d * 0.5,
            y + d * 0.5 - 8.0,
            Rgba::hex(0x1a1205),
            13.0,
            Align::Center,
        );
        self.hit(control, (x, y, d, d));
    }

    /// CUE or PLAY: a big round button with a coloured ring.
    fn big(
        &mut self,
        control: Control,
        x: f32,
        y: f32,
        s: &str,
        ring: Rgba,
        text: Rgba,
        down: bool,
    ) {
        let d = 104.0;
        let fill = if down { BUTTON_DOWN } else { BUTTON_TOP };
        self.scene.rounded(x, y, d, d, d * 0.5, fill, 4.0, ring);
        self.label(
            s,
            x + d * 0.5,
            y + d * 0.5 - 14.0,
            text,
            23.0,
            Align::Center,
        );
        self.hit(control, (x, y, d, d));
    }

    fn rect_button(
        &mut self,
        control: Control,
        (x, y, w, h): (f32, f32, f32, f32),
        s: &str,
        fill: Rgba,
        edge: Rgba,
        text: Rgba,
        down: bool,
    ) {
        let fill = if down { BUTTON_DOWN } else { fill };
        self.scene.rounded(x, y, w, h, 6.0, fill, 1.0, edge);
        self.label(
            s,
            x + w * 0.5,
            y + h * 0.5 - 10.0,
            text,
            16.0,
            Align::Center,
        );
        self.hit(control, (x, y, w, h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(c: Vec<DeckCommand>) -> String {
        format!("{c:?}")
    }

    fn shell() -> Standin {
        let mut s = Standin::new(48_000);
        s.build(&ShellView::default(), &mut Scene::default());
        s
    }

    #[test]
    fn the_browse_knob_pushes_turns_and_back_goes_up() {
        let mut s = shell();
        let v = ShellView::default();
        // A push without turning loads.
        s.press(1388.0, 202.0, false, &v);
        s.release();
        assert_eq!(s.take_nav(), [Nav::Push]);
        // A turn (vertical drag) steps and does not push.
        s.press(1388.0, 202.0, false, &v);
        s.drag(1388.0, 202.0 + 2.5 * KNOB_STEP);
        s.drag(1388.0, 202.0 + 0.9 * KNOB_STEP);
        s.release();
        assert_eq!(s.take_nav(), [Nav::Down, Nav::Down, Nav::Up]);
        s.press(1388.0, 120.0, false, &v);
        s.release();
        s.press(1388.0, 280.0, false, &v);
        s.release();
        s.wheel(1388.0, 202.0, -2);
        s.wheel(700.0, 300.0, 3); // not over the knob
        s.press(1340.0, 60.0, false, &v);
        s.release();
        assert_eq!(
            s.take_nav(),
            [Nav::Up, Nav::Down, Nav::Up, Nav::Up, Nav::Back]
        );
    }

    #[test]
    fn tag_track_tags_and_a_long_press_removes() {
        let mut s = shell();
        let v = ShellView::default();
        s.press(1440.0, 60.0, false, &v);
        s.tick(0.1);
        s.release();
        s.press(1440.0, 60.0, false, &v);
        for _ in 0..8 {
            s.tick(0.1);
        }
        s.release();
        assert_eq!(s.take_nav(), [Nav::Tag, Nav::Untag]);
    }

    #[test]
    fn cue_holds_and_releases() {
        let mut s = shell();
        let v = ShellView::default();
        assert_eq!(cmds(s.press(121.0, 618.0, false, &v)), "[CuePress]");
        assert_eq!(cmds(s.release()), "[CueRelease]");
    }

    #[test]
    fn pads_set_and_delete_hot_cues() {
        let mut s = shell();
        let v = ShellView::default();
        assert_eq!(cmds(s.press(288.0, 813.0, false, &v)), "[HotCue(0)]");
        s.release();
        assert_eq!(cmds(s.press(364.0, 887.0, true, &v)), "[DeleteHotCue(5)]");
    }

    #[test]
    fn the_fader_sets_the_rate_up_is_slower() {
        let mut s = shell();
        let v = ShellView::default();
        // Range ±10 by default: the top of the travel is −10 %.
        let c = s.press(1390.0, FADER_CENTER_Y - FADER_TRAVEL, false, &v);
        assert_eq!(cmds(c), format!("{:?}", vec![DeckCommand::SetRate(0.9)]));
        let c = s.drag(1390.0, FADER_CENTER_Y + FADER_TRAVEL * 0.5);
        assert_eq!(cmds(c), format!("{:?}", vec![DeckCommand::SetRate(1.05)]));
        assert!(s.release().is_empty());
    }

    #[test]
    fn a_quarter_turn_of_the_plate_scratches_a_quarter_of_1_8_s() {
        let mut s = shell();
        let v = ShellView::default();
        let (cx, cy) = JOG_CENTER;
        assert_eq!(cmds(s.press(cx + 80.0, cy, false, &v)), "[ScratchBegin]");
        // Clockwise (screen y down) from 3 o'clock to 6 o'clock: forward.
        let c = s.drag(cx, cy + 80.0);
        let frames = 0.25 * SECONDS_PER_TURN * 48_000.0;
        match c.as_slice() {
            [DeckCommand::ScratchBy(f)] => assert!((f - frames).abs() < 1.0, "{f}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(cmds(s.release()), "[ScratchEnd]");
    }

    #[test]
    fn the_ring_bends_and_letting_go_restores_the_rate() {
        let mut s = shell();
        let v = ShellView {
            rate: 1.02,
            ..ShellView::default()
        };
        let (cx, cy) = JOG_CENTER;
        assert!(s.press(cx + 140.0, cy, false, &v).is_empty());
        s.drag(cx, cy + 140.0); // a quarter turn forward
        let c = s.tick(0.05);
        match c.as_slice() {
            [DeckCommand::SetRate(r)] => assert!(*r > 1.02, "{r}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            cmds(s.release()),
            format!("{:?}", vec![DeckCommand::SetRate(1.02)])
        );
    }

    #[test]
    fn search_scrubs_while_held() {
        let mut s = shell();
        let v = ShellView::default();
        assert_eq!(cmds(s.press(151.0, 439.0, false, &v)), "[ScratchBegin]");
        let c = s.tick(0.01);
        match c.as_slice() {
            [DeckCommand::ScratchBy(f)] => assert!((f - 4.0 * 0.01 * 48_000.0).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
        assert_eq!(cmds(s.release()), "[ScratchEnd]");
    }

    #[test]
    fn the_display_slot_is_not_a_control_and_fits_the_shell() {
        let s = shell();
        let (x, y, w, h) = DISPLAY;
        assert!(s.control_at(x + w * 0.5, y + h * 0.5).is_none());
        assert!(x + w <= SIZE.0 && y + h <= SIZE.1);
    }
}
