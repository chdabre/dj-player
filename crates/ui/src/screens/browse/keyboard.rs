//! The on-screen keyboard: its keys, the shift, and drawing it over the lower bands.

use super::*;

/// Where the keyboard goes: over the status band and the bottom strip.
pub(super) const KEYBOARD: Rect = Rect::new(106.0, 395.0, 915.0, 202.0);

/// The on-screen keyboard's special keys, as characters.
pub const BACKSPACE: char = '\u{8}';

pub const CLEAR: char = '\u{18}';

pub const DONE: char = '\n';

pub const SHIFT: char = '\u{f}';

/// The on-screen keyboard's shift: off, for the next letter, or locked (tap again to cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shift {
    #[default]
    Off,
    Once,
    Lock,
}

impl Shift {
    /// The next state on a tap of SHIFT.
    pub fn tap(self) -> Shift {
        match self {
            Shift::Off => Shift::Once,
            Shift::Once => Shift::Lock,
            Shift::Lock => Shift::Off,
        }
    }

    /// `c` as typed with this shift, and the shift after it (a letter uses up `Once`).
    pub fn apply(self, c: char) -> (char, Shift) {
        if self == Shift::Off || !c.is_alphabetic() {
            return (c, self);
        }
        let up = c.to_uppercase().next().unwrap_or(c);
        (
            up,
            if self == Shift::Once {
                Shift::Off
            } else {
                self
            },
        )
    }
}

/// No key: a gap in a row (the stagger of a real keyboard).
pub(super) const GAP: char = '\0';

/// The on-screen keyboard, laid out as a computer's: rows of (label, character, width in
/// key units), 11 units a row. Digits with backspace on top, the letter rows staggered,
/// SHIFT bottom left, and a bottom row of CLEAR, the space bar and DONE (as on a phone).
pub const KEYS: [&[(&str, char, f32)]; 5] = [
    &[
        ("1", '1', 1.0),
        ("2", '2', 1.0),
        ("3", '3', 1.0),
        ("4", '4', 1.0),
        ("5", '5', 1.0),
        ("6", '6', 1.0),
        ("7", '7', 1.0),
        ("8", '8', 1.0),
        ("9", '9', 1.0),
        ("0", '0', 1.0),
        ("⌫", BACKSPACE, 1.0),
    ],
    &[
        ("", GAP, 0.5),
        ("Q", 'q', 1.0),
        ("W", 'w', 1.0),
        ("E", 'e', 1.0),
        ("R", 'r', 1.0),
        ("T", 't', 1.0),
        ("Y", 'y', 1.0),
        ("U", 'u', 1.0),
        ("I", 'i', 1.0),
        ("O", 'o', 1.0),
        ("P", 'p', 1.0),
        ("", GAP, 0.5),
    ],
    &[
        ("", GAP, 1.0),
        ("A", 'a', 1.0),
        ("S", 's', 1.0),
        ("D", 'd', 1.0),
        ("F", 'f', 1.0),
        ("G", 'g', 1.0),
        ("H", 'h', 1.0),
        ("J", 'j', 1.0),
        ("K", 'k', 1.0),
        ("L", 'l', 1.0),
        ("", GAP, 1.0),
    ],
    &[
        ("SHIFT", SHIFT, 1.0),
        ("Z", 'z', 1.0),
        ("X", 'x', 1.0),
        ("C", 'c', 1.0),
        ("V", 'v', 1.0),
        ("B", 'b', 1.0),
        ("N", 'n', 1.0),
        ("M", 'm', 1.0),
        ("-", '-', 1.0),
        (".", '.', 1.0),
        ("'", '\'', 1.0),
    ],
    &[
        ("CLEAR", CLEAR, 2.0),
        ("&", '&', 1.0),
        ("SPACE", ' ', 6.0),
        ("DONE", DONE, 2.0),
    ],
];

/// Every key, in order, without the gaps.
pub(super) fn keys() -> impl Iterator<Item = &'static (&'static str, char, f32)> {
    KEYS.iter().flat_map(|r| r.iter()).filter(|k| k.1 != GAP)
}

/// How many keys the keyboard has.
#[cfg(test)]
pub fn key_count() -> usize {
    keys().count()
}

/// The character of keyboard key `i` (counted across the rows).
pub fn key_char(i: u8) -> Option<char> {
    keys().nth(usize::from(i)).map(|k| k.1)
}

/// The search keyboard, over the status band and the bottom strip.
pub(super) fn keyboard(ui: &mut Ui, shift: Shift) {
    let t = ui.t;
    let Rect { x: x0, y: y0, w, h } = KEYBOARD;
    ui.section(Rect::new(x0, y0, w, h));
    let gap = 3.0;
    let (inner_x, inner_w) = (x0 + 7.0, w - 14.0);
    // A key of `u` units is `u * unit - gap` wide, so a row of 11 units fills the width.
    let unit = (inner_w + gap) / 11.0;
    let rows = KEYS.len() as f32;
    let row_h = (h - 2.0 * gap - (rows - 1.0) * gap) / rows;
    let mut index = 0u8;
    for (r, keys) in KEYS.iter().enumerate() {
        let y = y0 + gap + r as f32 * (row_h + gap);
        let mut u = 0.0;
        for &(label, ch, units) in keys.iter() {
            let x = inner_x + u * unit;
            u += units;
            if ch == GAP {
                continue;
            }
            let kw = units * unit - gap;
            let on = ch == DONE || (ch == SHIFT && shift != Shift::Off);
            // Letters show the case they type.
            let shown = match (ch, shift) {
                (SHIFT, Shift::Lock) => "CAPS".to_string(),
                (c, Shift::Off) if c.is_alphabetic() => label.to_lowercase(),
                _ => label.to_string(),
            };
            let style = if label.chars().count() > 1 {
                t.text.small.semibold().color(t.color.text)
            } else {
                t.text.button
            };
            ui.button_styled(
                Target::Browse(BrowseTarget::Key(index)),
                Rect::new(x, y, kw, row_h),
                &shown,
                None,
                on,
                style,
            );
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_rows_are_eleven_units_and_keys_map_to_characters() {
        for row in KEYS {
            let units: f32 = row.iter().map(|k| k.2).sum();
            assert!((units - 11.0).abs() < 1e-6, "{units}");
        }
        assert_eq!(key_char(0), Some('1'));
        assert_eq!(key_char(10), Some(BACKSPACE));
        assert_eq!(key_char(11), Some('q'), "the row's gap is no key");
        assert_eq!(key_char(20), Some('p'));
        assert_eq!(key_char(30), Some(SHIFT));
        let n = key_count();
        assert_eq!(key_char(n as u8 - 1), Some(DONE), "DONE bottom right");
        assert_eq!(key_char(n as u8), None);
        assert!(keys().any(|k| k.1 == ' ' && k.2 >= 5.0), "a wide space bar");
    }

    #[test]
    fn shift_capitalises_one_letter_or_locks() {
        let s = Shift::Off.tap();
        assert_eq!(s, Shift::Once);
        let (c, s) = s.apply('p');
        assert_eq!((c, s), ('P', Shift::Off), "once: one letter");
        assert_eq!(
            Shift::Once.apply('1'),
            ('1', Shift::Once),
            "digits don't use it up"
        );
        let lock = Shift::Once.tap();
        assert_eq!(lock.apply('e'), ('E', Shift::Lock));
        assert_eq!(lock.tap(), Shift::Off);
        assert_eq!(Shift::Off.apply('k'), ('k', Shift::Off));
    }
}
