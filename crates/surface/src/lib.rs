//! The input vocabulary: what the player's physical controls report, whoever stands in for
//! them. On the device the RP2040 control surface sends these; on a laptop the UI's stand-in
//! and dev keyboard do. What a control *does* is decided in one place, `engined`'s
//! `surface`, so the stand-in, the keyboard and the hardware behave alike.
//!
//! No dependencies and no `std`, so the firmware's side can use the same types. Events
//! travel as short frames ([`ControlEvent::encode`], [`ControlEvent::decode`]).

#![no_std]

/// A button (or the push of the browse knob) on the player.
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
    /// The browse knob pushed.
    Browse,
    F1,
    F2,
    TempoRange,
    Keylock,
    JogMode,
    LoopIn,
    LoopOut,
    Reloop,
    /// A hot cue pad (0 = A).
    Pad(u8),
}

/// Something a control did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlEvent {
    /// A button went down. `alt`: with the dev shift (the stand-in's right button or the shift
    /// key), which deletes a hot cue on a pad.
    Down {
        control: Control,
        alt: bool,
    },
    Up(Control),
    /// The browse knob turned `steps` detents (positive = down the list).
    Turn(i32),
    /// The tempo fader moved to `at`: -1 at the top of its travel (slower), 1 at the bottom.
    Fader(f64),
    /// A hand on the jog wheel: on the top plate, or on the ring.
    JogTouch {
        plate: bool,
    },
    /// The jog turned by `radians`, clockwise positive.
    JogTurn(f32),
    JogRelease,
}

impl ControlEvent {
    pub const fn down(control: Control) -> Self {
        Self::Down {
            control,
            alt: false,
        }
    }
}

/// Seconds of track one turn of the jog's plate scratches (a 33⅓ record).
pub const SECONDS_PER_TURN: f64 = 1.8;

/// A hand on the jog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Jog {
    /// Scratching with the top plate.
    Scratch,
    /// Bending with the ring from the rate `base`.
    Bend { base: f64 },
}

/// What the browse controls (knob, BACK, TAG TRACK) ask of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    /// The knob pushed: open the browser, or load / open the selection.
    Push,
    Back,
    /// TAG TRACK: onto the tag list; a long press takes it off.
    Tag,
    Untag,
}

/// Longest encoded [`ControlEvent`], in bytes.
pub const MAX_FRAME: usize = 9;

impl Control {
    /// One byte: the buttons in declaration order, then the pads from 0x40.
    fn code(self) -> u8 {
        use Control::*;
        match self {
            Pad(i) => 0x40 | (i & 0x3f),
            c => BUTTONS.iter().position(|&b| b == c).unwrap_or(0) as u8,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        if code & 0x40 != 0 {
            return Some(Control::Pad(code & 0x3f));
        }
        BUTTONS.get(usize::from(code)).copied()
    }
}

/// The buttons that are not pads, by code.
const BUTTONS: [Control; 20] = {
    use Control::*;
    [
        TimeMode, Quantize, Rev, TrackBack, TrackNext, SearchBack, SearchFwd, Cue, Play, Back,
        TagTrack, Browse, F1, F2, TempoRange, Keylock, JogMode, LoopIn, LoopOut, Reloop,
    ]
};

impl ControlEvent {
    /// The event as a frame: a tag byte, then its fields little-endian. Returns the bytes of
    /// `out` used.
    pub fn encode(&self, out: &mut [u8; MAX_FRAME]) -> usize {
        match *self {
            ControlEvent::Down { control, alt } => {
                out[0] = 1;
                out[1] = control.code();
                out[2] = u8::from(alt);
                3
            }
            ControlEvent::Up(control) => {
                out[0] = 2;
                out[1] = control.code();
                2
            }
            ControlEvent::Turn(steps) => {
                out[0] = 3;
                out[1..5].copy_from_slice(&steps.to_le_bytes());
                5
            }
            ControlEvent::Fader(at) => {
                out[0] = 4;
                out[1..9].copy_from_slice(&at.to_le_bytes());
                9
            }
            ControlEvent::JogTouch { plate } => {
                out[0] = 5;
                out[1] = u8::from(plate);
                2
            }
            ControlEvent::JogTurn(radians) => {
                out[0] = 6;
                out[1..5].copy_from_slice(&radians.to_le_bytes());
                5
            }
            ControlEvent::JogRelease => {
                out[0] = 7;
                1
            }
        }
    }

    /// The event in `frame`, if it is a whole and known one.
    pub fn decode(frame: &[u8]) -> Option<Self> {
        let byte = |i: usize| frame.get(i).copied();
        let four = || -> Option<[u8; 4]> { frame.get(1..5)?.try_into().ok() };
        Some(match (byte(0)?, frame.len()) {
            (1, 3) => ControlEvent::Down {
                control: Control::from_code(byte(1)?)?,
                alt: byte(2)? != 0,
            },
            (2, 2) => ControlEvent::Up(Control::from_code(byte(1)?)?),
            (3, 5) => ControlEvent::Turn(i32::from_le_bytes(four()?)),
            (4, 9) => ControlEvent::Fader(f64::from_le_bytes(frame.get(1..9)?.try_into().ok()?)),
            (5, 2) => ControlEvent::JogTouch {
                plate: byte(1)? != 0,
            },
            (6, 5) => ControlEvent::JogTurn(f32::from_le_bytes(four()?)),
            (7, 1) => ControlEvent::JogRelease,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_survives_a_frame() {
        let mut events = [ControlEvent::JogRelease; 64];
        let mut n = 0;
        let mut push = |e| {
            events[n] = e;
            n += 1;
        };
        for c in BUTTONS {
            push(ControlEvent::Down {
                control: c,
                alt: true,
            });
            push(ControlEvent::Up(c));
        }
        for p in [0, 7, 15] {
            push(ControlEvent::down(Control::Pad(p)));
        }
        push(ControlEvent::Turn(-3));
        push(ControlEvent::Fader(-0.25));
        push(ControlEvent::JogTouch { plate: true });
        push(ControlEvent::JogTurn(0.5));
        push(ControlEvent::JogRelease);
        for e in &events[..n] {
            let mut frame = [0; MAX_FRAME];
            let len = e.encode(&mut frame);
            assert_eq!(ControlEvent::decode(&frame[..len]), Some(*e), "{e:?}");
        }
    }

    #[test]
    fn short_or_unknown_frames_are_refused() {
        assert_eq!(ControlEvent::decode(&[]), None);
        assert_eq!(ControlEvent::decode(&[3, 1, 0]), None);
        assert_eq!(ControlEvent::decode(&[9]), None);
        assert_eq!(ControlEvent::decode(&[2, 0x3f]), None);
    }
}
