//! Musical keys.

use std::fmt;

/// Pitch class, 0 = C ... 11 = B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PitchClass(u8);

impl PitchClass {
    pub fn new(semitones_from_c: i32) -> Self {
        PitchClass(semitones_from_c.rem_euclid(12) as u8)
    }

    pub fn index(self) -> u8 {
        self.0
    }

    /// The pitch class `semitones` above this one.
    pub fn up(self, semitones: i32) -> Self {
        PitchClass::new(i32::from(self.0) + semitones)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Major,
    Minor,
}

/// A musical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub tonic: PitchClass,
    pub mode: Mode,
}

impl Key {
    pub fn major(tonic: i32) -> Self {
        Key {
            tonic: PitchClass::new(tonic),
            mode: Mode::Major,
        }
    }

    pub fn minor(tonic: i32) -> Self {
        Key {
            tonic: PitchClass::new(tonic),
            mode: Mode::Minor,
        }
    }

    /// Parse "C", "F#", "Bb", "Am", "C#m", "Ebmin", "A minor", "D major".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut chars = s.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let base = match letter {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        let rest = chars.as_str();
        let (accidental, rest) = match rest.chars().next() {
            Some(c @ ('#' | '♯')) => (1, &rest[c.len_utf8()..]),
            Some(c @ ('b' | '♭')) => (-1, &rest[c.len_utf8()..]),
            _ => (0, rest),
        };
        let mode = match rest.trim().to_ascii_lowercase().as_str() {
            "" | "maj" | "major" => Mode::Major,
            "m" | "min" | "minor" => Mode::Minor,
            _ => return None,
        };
        Some(Key {
            tonic: PitchClass::new(base + accidental),
            mode,
        })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
        ];
        let m = if self.mode == Mode::Minor { "m" } else { "" };
        write!(f, "{}{m}", NAMES[usize::from(self.tonic.0)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_keys() {
        for (s, k) in [
            ("C", Key::major(0)),
            ("Am", Key::minor(9)),
            ("F#m", Key::minor(6)),
            ("Bb", Key::major(10)),
            ("Ebmin", Key::minor(3)),
            ("A minor", Key::minor(9)),
            ("D major", Key::major(2)),
            ("C#", Key::major(1)),
        ] {
            assert_eq!(Key::parse(s), Some(k), "{s}");
        }
        assert_eq!(Key::parse("H"), None);
        assert_eq!(Key::parse("C dorian"), None);
        assert_eq!(Key::minor(9).to_string(), "Am");
        assert_eq!(Key::major(3).to_string(), "Eb");
    }
}
