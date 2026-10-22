//! The dev keyboard: which key stands for which control, plus shortcuts for what the hardware
//! has no button for (seek, rate nudge, debug toggles). Two tables: [`typed`] for when a text
//! field has the keyboard (search, naming), and [`shortcut`] for everything else. The app
//! decides which applies (focus); this module only knows the keys.

use player_api::DeckCommand;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

use crate::input::{Control, ControlEvent, Nav};
use crate::screens::browse::{BACKSPACE, DONE};

/// Seconds ← / → seek.
const SEEK_SECONDS: f64 = 5.0;
/// Rate step of + / -.
const RATE_STEP: f64 = 0.01;

/// A key event as the keymap reads it.
#[derive(Debug, Clone)]
pub struct KeyPress {
    pub key: Key,
    pub code: PhysicalKey,
    pub pressed: bool,
    pub repeat: bool,
}

impl From<&KeyEvent> for KeyPress {
    fn from(e: &KeyEvent) -> Self {
        Self {
            key: e.logical_key.clone(),
            code: e.physical_key,
            pressed: e.state == ElementState::Pressed,
            repeat: e.repeat,
        }
    }
}

/// Where the keyboard is used: the app's focus, as far as the keys care.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyContext {
    /// The browse screen is up (arrows, Enter, `/` and `i` browse).
    pub browsing: bool,
    /// The INFO screen is up (`i` closes it).
    pub info: bool,
    pub shift: bool,
}

/// What a key does outside text fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Shortcut {
    /// A control of the player, as the hardware would report it.
    Control(ControlEvent),
    /// A deck command with no control of its own (dev keys). Frames need the rate, so seeks
    /// are in seconds here.
    Deck(DevDeck),
    /// A browse request with no control of its own (shift+x: untag).
    Nav(Nav),
    /// The screens: PERFORM, BROWSE (Tab), SEARCH (`/`), INFO (`i`).
    Perform,
    ToggleBrowse,
    Search,
    Info,
    /// Dev toggles.
    Debug,
    Memory,
    WaveMode,
    Fullscreen,
    Quit,
}

/// Deck keys without a control on the player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DevDeck {
    SeekSeconds(f64),
    NudgeRate(f64),
    ResetRate,
    ToggleSlip,
    BeatLoop(f64),
    LoopResize(f64),
    BeatJump(f64),
}

impl DevDeck {
    /// The command, at `sample_rate`.
    pub fn command(self, sample_rate: f64) -> DeckCommand {
        match self {
            DevDeck::SeekSeconds(s) => DeckCommand::SeekRelative(s * sample_rate),
            DevDeck::NudgeRate(d) => DeckCommand::NudgeRate(d),
            DevDeck::ResetRate => DeckCommand::SetRate(1.0),
            DevDeck::ToggleSlip => DeckCommand::ToggleSlip,
            DevDeck::BeatLoop(b) => DeckCommand::BeatLoop(b),
            DevDeck::LoopResize(f) => DeckCommand::LoopResize(f),
            DevDeck::BeatJump(b) => DeckCommand::BeatJump(b),
        }
    }
}

/// What `key` types into a text field, if anything: characters, space, backspace, and Enter
/// as DONE. Esc is not typing (it leaves the field). Releases give the same characters, so a
/// field can swallow them too; only presses type.
pub fn typed(key: &KeyPress) -> Vec<char> {
    match &key.key {
        Key::Character(c) => c.chars().collect(),
        Key::Named(NamedKey::Space) => vec![' '],
        Key::Named(NamedKey::Backspace) => vec![BACKSPACE],
        Key::Named(NamedKey::Enter) => vec![DONE],
        _ => Vec::new(),
    }
}

/// What `key` does outside text fields. Releases matter only for controls (CUE previews while
/// held, TAG TRACK counts its long press); key repeats are ignored.
pub fn shortcut(key: &KeyPress, ctx: KeyContext) -> Option<Shortcut> {
    if key.repeat {
        return None;
    }
    let down = key.pressed;
    let control = |c: Control| {
        Some(Shortcut::Control(if down {
            ControlEvent::Down {
                control: c,
                alt: ctx.shift,
            }
        } else {
            ControlEvent::Up(c)
        }))
    };
    if let Some(i) = hot_cue_key(key) {
        return control(Control::Pad(i));
    }
    let Key::Character(c) = &key.key else {
        return match &key.key {
            Key::Named(NamedKey::Space) => control(Control::Play),
            Key::Named(NamedKey::Escape | NamedKey::Backspace) => control(Control::Back),
            Key::Named(NamedKey::Enter) if ctx.browsing => control(Control::Browse),
            Key::Named(NamedKey::ArrowUp) if ctx.browsing && down => {
                Some(Shortcut::Control(ControlEvent::Turn(-1)))
            }
            Key::Named(NamedKey::ArrowDown) if ctx.browsing && down => {
                Some(Shortcut::Control(ControlEvent::Turn(1)))
            }
            _ if !down => None,
            Key::Named(NamedKey::Tab) => Some(Shortcut::ToggleBrowse),
            Key::Named(NamedKey::ArrowLeft) => {
                Some(Shortcut::Deck(DevDeck::SeekSeconds(-SEEK_SECONDS)))
            }
            Key::Named(NamedKey::ArrowRight) => {
                Some(Shortcut::Deck(DevDeck::SeekSeconds(SEEK_SECONDS)))
            }
            _ => None,
        };
    };
    let c = c.to_lowercase();
    match c.as_str() {
        "c" => return control(Control::Cue),
        "k" => return control(Control::Keylock),
        "r" => return control(Control::Rev),
        "z" => return control(Control::Quantize),
        "x" if ctx.shift => return down.then_some(Shortcut::Nav(Nav::Untag)),
        "x" => return control(Control::TagTrack),
        "i" if !ctx.browsing && !ctx.info => return control(Control::LoopIn),
        "o" => return control(Control::LoopOut),
        "l" => return control(Control::Reloop),
        _ => {}
    }
    if !down {
        return None;
    }
    let deck = |d| Some(Shortcut::Deck(d));
    match c.as_str() {
        "i" => Some(Shortcut::Info),
        "/" if ctx.browsing => Some(Shortcut::Search),
        "q" => Some(Shortcut::Quit),
        "d" => Some(Shortcut::Debug),
        "m" => Some(Shortcut::Memory),
        "p" => Some(Shortcut::Perform),
        "w" => Some(Shortcut::WaveMode),
        "f" => Some(Shortcut::Fullscreen),
        "+" | "=" => deck(DevDeck::NudgeRate(RATE_STEP)),
        "-" | "_" => deck(DevDeck::NudgeRate(-RATE_STEP)),
        "0" => deck(DevDeck::ResetRate),
        "s" => deck(DevDeck::ToggleSlip),
        "b" => deck(DevDeck::BeatLoop(4.0)),
        "[" => deck(DevDeck::LoopResize(0.5)),
        "]" => deck(DevDeck::LoopResize(2.0)),
        "," => deck(DevDeck::BeatJump(-4.0)),
        "." => deck(DevDeck::BeatJump(4.0)),
        _ => None,
    }
}

/// The digit keys 1–8 (by position, so shift and layout do not matter) are hot cues A–H.
fn hot_cue_key(key: &KeyPress) -> Option<u8> {
    const DIGITS: [KeyCode; 8] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
    ];
    match key.code {
        PhysicalKey::Code(code) => DIGITS.iter().position(|&d| d == code).map(|i| i as u8),
        PhysicalKey::Unidentified(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn press(key: Key, code: KeyCode) -> KeyPress {
        KeyPress {
            key,
            code: PhysicalKey::Code(code),
            pressed: true,
            repeat: false,
        }
    }

    fn ch(c: &str, code: KeyCode) -> KeyPress {
        press(Key::Character(SmolStr::new(c)), code)
    }

    fn up(mut k: KeyPress) -> KeyPress {
        k.pressed = false;
        k
    }

    const PLAIN: KeyContext = KeyContext {
        browsing: false,
        info: false,
        shift: false,
    };

    #[test]
    fn cue_and_tag_track_report_press_and_release_other_keys_only_press() {
        let c = ch("c", KeyCode::KeyC);
        assert_eq!(
            shortcut(&c, PLAIN),
            Some(Shortcut::Control(ControlEvent::down(Control::Cue)))
        );
        assert_eq!(
            shortcut(&up(c), PLAIN),
            Some(Shortcut::Control(ControlEvent::Up(Control::Cue)))
        );
        assert_eq!(shortcut(&up(ch("q", KeyCode::KeyQ)), PLAIN), None);
        let mut held = ch("c", KeyCode::KeyC);
        held.repeat = true;
        assert_eq!(shortcut(&held, PLAIN), None);
    }

    #[test]
    fn digits_are_pads_by_position_and_shift_deletes() {
        // Shift+1 is "!" on most layouts: the position counts.
        let k = ch("!", KeyCode::Digit1);
        let shifted = KeyContext {
            shift: true,
            ..PLAIN
        };
        assert_eq!(
            shortcut(&k, shifted),
            Some(Shortcut::Control(ControlEvent::Down {
                control: Control::Pad(0),
                alt: true
            }))
        );
    }

    #[test]
    fn browse_keys_only_browse_on_the_browse_screen() {
        let browsing = KeyContext {
            browsing: true,
            ..PLAIN
        };
        let arrow = press(Key::Named(NamedKey::ArrowDown), KeyCode::ArrowDown);
        assert_eq!(
            shortcut(&arrow, browsing),
            Some(Shortcut::Control(ControlEvent::Turn(1)))
        );
        assert_eq!(shortcut(&arrow, PLAIN), None);
        let i = ch("i", KeyCode::KeyI);
        assert_eq!(shortcut(&i, browsing), Some(Shortcut::Info));
        assert_eq!(
            shortcut(&i, PLAIN),
            Some(Shortcut::Control(ControlEvent::down(Control::LoopIn)))
        );
        assert_eq!(shortcut(&ch("/", KeyCode::Slash), PLAIN), None);
        // Esc is BACK everywhere.
        assert_eq!(
            shortcut(&press(Key::Named(NamedKey::Escape), KeyCode::Escape), PLAIN),
            Some(Shortcut::Control(ControlEvent::down(Control::Back)))
        );
    }

    #[test]
    fn typing_takes_characters_space_backspace_and_enter() {
        assert_eq!(typed(&ch("aB", KeyCode::KeyA)), ['a', 'B']);
        assert_eq!(
            typed(&press(Key::Named(NamedKey::Space), KeyCode::Space)),
            [' ']
        );
        assert_eq!(
            typed(&press(Key::Named(NamedKey::Backspace), KeyCode::Backspace)),
            [BACKSPACE]
        );
        assert_eq!(
            typed(&press(Key::Named(NamedKey::Enter), KeyCode::Enter)),
            [DONE]
        );
        assert!(typed(&press(Key::Named(NamedKey::Escape), KeyCode::Escape)).is_empty());
    }
}
