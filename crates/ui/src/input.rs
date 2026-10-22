//! The input vocabulary: what the player's physical controls report, whoever stands in for
//! them. On the device the RP2040 control surface sends these; on a laptop the stand-in
//! (`standin.rs`) and the dev keyboard (`app/keymap.rs`) do. What a control *does* is decided
//! in one place, `app/surface.rs`, so the stand-in, the keyboard and the hardware behave alike.

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
