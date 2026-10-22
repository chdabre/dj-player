//! The deck as the UI sees it: one [`DeckSnapshot`] read at the start of a frame, and what is
//! known about the track on it ([`OnDeck`]).

use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

use engine_core::{DeckGrid, FALLBACK_BPM, HOT_CUES, beat_period};
use waveform::compute::Entry;

/// The loaded track's waveform as it grows (or the stored one, once the library has said).
pub trait WaveformFeed: Debug + Send + Sync {
    /// Append the entries `have` does not have yet. Returns true once the waveform is
    /// complete (and `have` holds all of it).
    fn update(&self, have: &mut Vec<Entry>) -> bool;
}

/// The track on the deck.
#[derive(Debug, Clone)]
pub struct OnDeck {
    pub file: PathBuf,
    pub waveform: Arc<dyn WaveformFeed>,
    /// Tempo and key from the analysis, once it is done (`analysed`).
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub analysed: bool,
    /// The whole file is decoded.
    pub decoded: bool,
}

/// The deck at the start of a frame. Positions are frames at the engine rate.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckSnapshot {
    pub sample_rate: f64,
    /// The engine's playhead (a screen shows it smoothed).
    pub position: f64,
    pub shadow_position: f64,
    pub length: f64,
    pub playing: bool,
    pub reverse: bool,
    pub rate: f64,
    pub slip_enabled: bool,
    pub slip_active: bool,
    pub keylock: bool,
    /// Quantize resolution in beats; 0 = off.
    pub quantize: f64,
    pub cue: f64,
    /// Hot cues in use (bit 0 = A).
    pub hot_cues: u8,
    /// Hot cue positions, NaN where empty.
    pub hot_cue_positions: [f64; HOT_CUES],
    pub looping: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    /// The loaded track's grid and key, once analysed.
    pub grid: Option<Arc<DeckGrid>>,
    pub key: Option<String>,
    /// A track is loaded and its analysis is still running.
    pub analysing: bool,
    /// The share of the loaded track decoded so far (0..=1).
    pub loaded_fraction: f64,
}

impl DeckSnapshot {
    pub fn grid(&self) -> Option<&DeckGrid> {
        self.grid.as_deref()
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// The playhead's speed in track frames per second of real time over the engine rate:
    /// the rate, negative in reverse, 0 when paused.
    pub fn speed(&self) -> f64 {
        match (self.playing, self.reverse) {
            (false, _) => 0.0,
            (true, false) => self.rate,
            (true, true) => -self.rate,
        }
    }

    /// The active loop's length in beats ([`FALLBACK_BPM`] until there is a grid), if a loop is
    /// active.
    pub fn loop_beats(&self) -> Option<f64> {
        let (a, b) = (self.loop_start, self.loop_end);
        if !self.looping || b <= a {
            return None;
        }
        Some(match self.grid() {
            Some(g) => g.beat_at(b) - g.beat_at(a),
            None => (b - a) / beat_period(FALLBACK_BPM, self.sample_rate),
        })
    }

    /// Frames across a zoom view of `zoom_beats` beats ([`FALLBACK_BPM`] until there is a
    /// grid, as the engine's beat loops and jumps).
    pub fn zoom_span(&self, zoom_beats: f64) -> f64 {
        match self.grid() {
            Some(g) => zoom_beats * g.period(),
            None => zoom_beats * beat_period(FALLBACK_BPM, self.sample_rate),
        }
    }

    /// A deck with nothing loaded, at `sample_rate`: for tests and layouts.
    pub fn empty(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            position: 0.0,
            shadow_position: 0.0,
            length: 0.0,
            playing: false,
            reverse: false,
            rate: 1.0,
            slip_enabled: false,
            slip_active: false,
            keylock: false,
            quantize: 0.0,
            cue: 0.0,
            hot_cues: 0,
            hot_cue_positions: [f64::NAN; HOT_CUES],
            looping: false,
            loop_start: f64::NAN,
            loop_end: f64::NAN,
            grid: None,
            key: None,
            analysing: false,
            loaded_fraction: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    #[test]
    fn speed_is_signed_and_zero_when_paused() {
        let mut d = DeckSnapshot {
            rate: 1.08,
            ..DeckSnapshot::empty(SR)
        };
        assert_eq!(d.speed(), 0.0);
        d.playing = true;
        assert_eq!(d.speed(), 1.08);
        d.reverse = true;
        assert_eq!(d.speed(), -1.08);
    }

    #[test]
    fn loop_beats_from_the_grid_else_120_bpm() {
        let mut d = DeckSnapshot {
            looping: true,
            loop_start: SR,
            loop_end: 3.0 * SR,
            ..DeckSnapshot::empty(SR)
        };
        // Two seconds at the fallback tempo.
        assert_eq!(d.loop_beats(), Some(2.0 * FALLBACK_BPM / 60.0));
        // 125 BPM: 0.48 s a beat.
        d.grid = DeckGrid::constant(0.0, 125.0, 300.0 * SR, SR as u32).map(Arc::new);
        assert!((d.loop_beats().unwrap() - 2.0 / 0.48).abs() < 1e-6);
        d.looping = false;
        assert_eq!(d.loop_beats(), None);
    }

    #[test]
    fn zoom_span_is_beats_of_the_grid() {
        let mut d = DeckSnapshot::empty(SR);
        // 16 beats at the fallback tempo.
        assert_eq!(d.zoom_span(16.0), 16.0 * 60.0 / FALLBACK_BPM * SR);
        d.grid = DeckGrid::constant(0.0, 125.0, 300.0 * SR, SR as u32).map(Arc::new);
        assert!((d.zoom_span(16.0) - 16.0 * 0.48 * SR).abs() < 1e-6);
    }
}
