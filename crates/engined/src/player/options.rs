//! How a [`crate::Player`] is opened: analysis settings, the library and auto cue.

use crate::store::LibraryChoice;

/// How to open a [`crate::Player`].
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub settings: engine_analysis::Settings,
    pub library: LibraryChoice,
    pub auto_cue: AutoCue,
}

/// Where the cue point goes when a track loads: the first stored memory cue if there is
/// one; else, with auto cue on, the first sound at or above the level; else no cue point,
/// the playhead at the track start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoCue {
    Off,
    /// Peak level in dBFS (the reference player offers −36 to −78 dB).
    Level(f32),
}

impl Default for AutoCue {
    fn default() -> Self {
        AutoCue::Level(-36.0)
    }
}

impl AutoCue {
    /// `off`, or a level in dB (`-36`, `-48`, …).
    pub fn parse(s: &str) -> Option<Self> {
        if s == "off" {
            return Some(AutoCue::Off);
        }
        let db: f32 = s.trim_end_matches("dB").parse().ok()?;
        (db.is_finite() && db <= 0.0).then_some(AutoCue::Level(db))
    }

    /// The linear peak that counts as sound, if auto cue is on.
    pub(crate) fn threshold(self) -> Option<f32> {
        match self {
            AutoCue::Off => None,
            AutoCue::Level(db) => Some(10f32.powf(db / 20.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_cue_levels_parse() {
        assert_eq!(AutoCue::parse("off"), Some(AutoCue::Off));
        assert_eq!(AutoCue::parse("-48"), Some(AutoCue::Level(-48.0)));
        assert_eq!(AutoCue::parse("-60dB"), Some(AutoCue::Level(-60.0)));
        assert_eq!(AutoCue::parse("6"), None);
        assert_eq!(AutoCue::parse("loud"), None);
        assert_eq!(AutoCue::default(), AutoCue::Level(-36.0));
        let t = AutoCue::Level(-20.0).threshold().unwrap();
        assert!((t - 0.1).abs() < 1e-6);
        assert_eq!(AutoCue::Off.threshold(), None);
    }
}
