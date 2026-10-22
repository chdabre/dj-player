//! The UI's view of the engine. Everything the UI knows about the deck comes from one
//! [`DeckSnapshot`], read once per frame; nothing else in the crate reads `DeckState`. When the
//! UI moves to its own process, the snapshot is filled from the shared-memory block instead
//! and nothing above this module changes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use engine_core::{DeckGrid, FALLBACK_BPM, HOT_CUES, beat_period};
use engined::store::{Listing, Playlists};
use engined::{BatchProgress, Player};

/// The library as input handlers may look at it: what it lists and what it is doing. Changes
/// go through `Effect`s.
#[derive(Clone, Copy, Default)]
pub struct LibraryView<'a> {
    /// The library's tracks, once it has said.
    pub listing: Option<&'a Arc<Listing>>,
    pub playlists: Option<&'a Arc<Playlists>>,
    /// ANALYZE ALL under way.
    pub batch: Option<&'a BatchProgress>,
    /// A scan under way: files done, total.
    pub scan: Option<(usize, usize)>,
}

impl<'a> LibraryView<'a> {
    pub fn read(player: &'a Player) -> Self {
        Self {
            listing: player.listing(),
            playlists: player.playlists(),
            batch: player.batch_progress(),
            scan: player.scan_progress(),
        }
    }

    /// A scan is running (or asked for and not started).
    pub fn scanning(&self) -> bool {
        self.scan
            .is_some_and(|(done, total)| done < total || total == 0)
    }
}

/// The deck at the start of a frame. Positions are frames at the engine rate.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckSnapshot {
    pub sample_rate: f64,
    /// The engine's playhead (the screen shows it smoothed: [`crate::clock`]).
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
    /// Read the deck now.
    pub fn read(player: &Player) -> Self {
        let st = player.state();
        let loaded = player.loaded();
        let analysed = loaded.and_then(|d| d.analysed.as_ref());
        Self {
            sample_rate: f64::from(player.format.sample_rate),
            position: st.position.load(),
            shadow_position: st.shadow_position.load(),
            length: st.length.load(),
            playing: st.playing.load(Ordering::Relaxed),
            reverse: st.reverse.load(Ordering::Relaxed),
            rate: st.rate.load(),
            slip_enabled: st.slip_enabled.load(Ordering::Relaxed),
            slip_active: st.slip_active.load(Ordering::Relaxed),
            keylock: st.keylock_enabled.load(Ordering::Relaxed),
            quantize: st.quantize.load(),
            cue: st.cue.load(),
            hot_cues: st.hot_cues.load(Ordering::Relaxed),
            hot_cue_positions: std::array::from_fn(|i| st.hot_cue_positions[i].load()),
            looping: st.looping.load(Ordering::Relaxed),
            loop_start: st.loop_start.load(),
            loop_end: st.loop_end.load(),
            grid: analysed.and_then(|a| a.grid.clone()),
            key: analysed.and_then(|a| a.key.clone()),
            analysing: loaded.is_some() && analysed.is_none(),
            loaded_fraction: loaded.map_or(0.0, |d| {
                d.source.chunks_ready() as f64 / d.source.chunk_count().max(1) as f64
            }),
        }
    }

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
    #[cfg(test)]
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
