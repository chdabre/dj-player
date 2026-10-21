//! Cue point, hot cues and quantize: the CUE button and pads A–H, as the reference player
//! behaves (XDJ reference spec, "Playback & performance").
//!
//! Pure logic over a [`Transport`] and an optional [`DeckGrid`]; the deck owns the state and
//! calls these on commands. All positions are frames at the engine rate.
//!
//! - **CUE while playing:** back cue: jump to the cue point and pause.
//! - **CUE while paused away from the cue:** set the cue here (quantized) and move there.
//! - **CUE held while paused at the cue:** preview from it; releasing returns to the cue and
//!   pauses; PLAY while holding keeps playing.
//! - **Hot cue on an empty pad:** store the current position (quantized). **On a set pad:**
//!   jump there, keeping the play state. While playing with quantize on, the jump keeps the
//!   beat phase (lands as far from the beat as the playhead was), so it stays on beat.

use crate::grid::DeckGrid;
use crate::transport::Transport;

/// Number of hot cues (A–H).
pub const HOT_CUES: usize = 8;

/// Quantize resolutions the reference offers, in beats.
pub const QUANTIZE_RESOLUTIONS: [f64; 4] = [0.125, 0.25, 0.5, 1.0];

/// Positions closer than this (frames) count as "at the cue".
const AT_CUE: f64 = 1.0;

/// A track's stored cues, handed to the deck when the library has them (see
/// [`crate::DeckCommand::SetCues`]). Frames at the engine rate. Plain data, so the command
/// carries it without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackCues {
    /// The cue point (the earliest memory cue, as the reference's auto cue at the MEMORY
    /// level does); `None` keeps the track start.
    pub cue: Option<f64>,
    pub hot: [Option<f64>; HOT_CUES],
    /// A memory loop to store for RELOOP, not entered.
    pub stored_loop: Option<(f64, f64)>,
    /// Only fill in what the deck doesn't have yet: empty hot cue slots, the cue point if it
    /// is still at the track start, the stored loop if there is none. For the first restore
    /// after a load, when the DJ may have set cues before the library answered; those stay.
    pub fill_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CueState {
    /// The cue point; 0 (track start) until set.
    pub cue: f64,
    /// CUE is held for a preview that returns to the cue on release.
    pub previewing: bool,
    pub hot: [Option<f64>; HOT_CUES],
    /// Quantize resolution in beats, or `None` when quantize is off.
    pub quantize: Option<f64>,
}

impl Default for CueState {
    fn default() -> Self {
        Self {
            cue: 0.0,
            previewing: false,
            hot: [None; HOT_CUES],
            // The reference ships with quantize on at one beat.
            quantize: Some(1.0),
        }
    }
}

impl CueState {
    /// `position` snapped to the grid at the quantize resolution; unchanged without a grid or
    /// with quantize off.
    pub fn snap(&self, grid: Option<&DeckGrid>, position: f64) -> f64 {
        match (grid, self.quantize) {
            (Some(g), Some(res)) => g.quantize(position, res),
            _ => position,
        }
    }

    pub fn cue_press(&mut self, t: &mut Transport, grid: Option<&DeckGrid>) {
        if t.is_playing() {
            t.pause();
            t.seek(self.cue);
            self.previewing = false;
        } else if (t.position() - self.cue).abs() <= AT_CUE {
            self.previewing = true;
            t.play();
        } else {
            self.cue = self.snap(grid, t.position()).clamp(0.0, t.length());
            t.seek(self.cue);
        }
    }

    pub fn cue_release(&mut self, t: &mut Transport) {
        if self.previewing {
            self.previewing = false;
            t.pause();
            t.seek(self.cue);
        }
    }

    /// PLAY pressed: while previewing, keep playing (release no longer returns).
    pub fn play_pressed(&mut self) {
        self.previewing = false;
    }

    /// Real-time cue: set the cue at the current position without moving the playhead.
    pub fn set_cue_here(&mut self, t: &Transport, grid: Option<&DeckGrid>) {
        self.cue = self.snap(grid, t.position()).clamp(0.0, t.length());
    }

    pub fn hot_cue(&mut self, index: usize, t: &mut Transport, grid: Option<&DeckGrid>) {
        let Some(&slot) = self.hot.get(index) else {
            return;
        };
        match slot {
            None => {
                let here = self.snap(grid, t.position()).clamp(0.0, t.length());
                self.hot[index] = Some(here);
            }
            Some(target) => {
                let to = match (grid, self.quantize, t.is_playing()) {
                    // Keep the beat phase: land as far past the target as we are past the
                    // nearest grid point.
                    (Some(g), Some(res), true) => {
                        target + (t.position() - g.quantize(t.position(), res))
                    }
                    _ => target,
                };
                t.seek(to);
                self.previewing = false;
            }
        }
    }

    pub fn delete_hot_cue(&mut self, index: usize) {
        if let Some(slot) = self.hot.get_mut(index) {
            *slot = None;
        }
    }

    /// Hot cues in use, as a bit mask (bit 0 = A).
    pub fn hot_mask(&self) -> u8 {
        self.hot
            .iter()
            .enumerate()
            .filter(|(_, h)| h.is_some())
            .fold(0, |m, (i, _)| m | (1 << i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const LEN: f64 = 48_000.0 * 300.0;

    /// 120 BPM, first beat at 1000: beats every 24000 frames.
    fn grid() -> DeckGrid {
        DeckGrid::constant(1000.0, 120.0, LEN, SR).unwrap()
    }

    fn at(position: f64, playing: bool) -> Transport {
        let mut t = Transport::new(LEN);
        t.seek(position);
        if playing {
            t.play();
        }
        t
    }

    #[test]
    fn cue_sets_when_paused_and_backcues_when_playing() {
        let g = grid();
        let mut c = CueState::default();
        let mut t = at(100_000.0, false);
        c.cue_press(&mut t, Some(&g));
        // 100000 snaps to the nearest beat: 97000 (beat 4).
        assert_eq!(c.cue, 97_000.0);
        assert_eq!(t.position(), 97_000.0);
        assert!(!t.is_playing());

        let mut t = at(300_000.0, true);
        c.cue_press(&mut t, Some(&g));
        assert_eq!(t.position(), 97_000.0);
        assert!(!t.is_playing());
    }

    #[test]
    fn holding_cue_previews_and_release_returns() {
        let mut c = CueState {
            cue: 50_000.0,
            ..CueState::default()
        };
        let mut t = at(50_000.0, false);
        c.cue_press(&mut t, None);
        assert!(t.is_playing() && c.previewing);
        t.advance(10_000);
        c.cue_release(&mut t);
        assert!(!t.is_playing());
        assert_eq!(t.position(), 50_000.0);

        // PLAY while holding: keeps playing after release.
        c.cue_press(&mut t, None);
        c.play_pressed();
        t.advance(10_000);
        c.cue_release(&mut t);
        assert!(t.is_playing());
        assert_eq!(t.position(), 60_000.0);
    }

    #[test]
    fn quantize_off_or_no_grid_keeps_the_exact_position() {
        let mut c = CueState {
            quantize: None,
            ..CueState::default()
        };
        let mut t = at(100_000.0, false);
        c.cue_press(&mut t, Some(&grid()));
        assert_eq!(c.cue, 100_000.0);
        let mut c = CueState::default();
        let mut t = at(100_123.0, false);
        c.cue_press(&mut t, None);
        assert_eq!(c.cue, 100_123.0);
    }

    #[test]
    fn hot_cues_set_on_empty_pads_and_jump_keeping_the_beat_phase() {
        let g = grid();
        let mut c = CueState::default();
        let mut t = at(49_200.0, true); // 200 frames after beat 2 (49000)
        c.hot_cue(0, &mut t, Some(&g));
        assert_eq!(c.hot[0], Some(49_000.0));
        assert_eq!(t.position(), 49_200.0, "setting does not move the playhead");
        assert_eq!(c.hot_mask(), 0b1);

        // Later, 300 frames after beat 10 (241000): the jump lands 300 after the pad.
        let mut t = at(241_300.0, true);
        c.hot_cue(0, &mut t, Some(&g));
        assert_eq!(t.position(), 49_300.0);
        assert!(t.is_playing());
        // Paused: exactly on the pad.
        let mut t = at(241_300.0, false);
        c.hot_cue(0, &mut t, Some(&g));
        assert_eq!(t.position(), 49_000.0);
        c.delete_hot_cue(0);
        assert_eq!(c.hot_mask(), 0);
        c.hot_cue(99, &mut t, Some(&g)); // out of range: ignored
    }
}
