//! Loop and beat-jump buttons, as the reference player behaves (XDJ reference spec, "Loop"
//! and "Jump"). Pure logic over a [`Transport`], an optional [`DeckGrid`] and the cue state.
//!
//! - **IN:** remember the loop in point (quantized); while playing it also sets the cue
//!   (real-time cue). **OUT:** close the loop there (quantized) and enter it.
//! - **RELOOP/EXIT:** leave the active loop, or re-enter the stored one from its start.
//! - **Beat loop** of 1/2 … 16 beats from the playhead (quantized).
//! - **LOOP 1/2X / 2X:** halve or double the loop, keeping its in point; lengths follow the
//!   grid in beats.
//! - **Beat jump** ±1 … 32 beats; with a loop active it moves the loop instead.
//!
//! Without a grid, beat lengths come from [`FALLBACK_BPM`], and nothing is quantized.

use crate::cue::CueState;
use crate::grid::{DeckGrid, FALLBACK_BPM};
use crate::transport::{Loop, MIN_LOOP_FRAMES, Transport};

/// Auto beat loop lengths the reference offers, in beats.
pub const BEAT_LOOPS: [f64; 6] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

/// Beat jump distances the reference offers, in beats.
pub const BEAT_JUMPS: [f64; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoopControl {
    /// IN pressed, OUT not yet.
    pub pending_in: Option<f64>,
}

/// Where `beats` beats from `position` lands, on the grid or at the fallback tempo.
fn beats_from(grid: Option<&DeckGrid>, sample_rate: u32, position: f64, beats: f64) -> f64 {
    match grid {
        Some(g) => g.offset(position, beats),
        None => position + beats * 60.0 / FALLBACK_BPM * f64::from(sample_rate),
    }
}

/// Length of `l` in beats.
fn length_beats(grid: Option<&DeckGrid>, sample_rate: u32, l: Loop) -> f64 {
    match grid {
        Some(g) => g.beat_at(l.end) - g.beat_at(l.start),
        None => l.len() / (60.0 / FALLBACK_BPM * f64::from(sample_rate)),
    }
}

impl LoopControl {
    pub fn loop_in(&mut self, t: &Transport, grid: Option<&DeckGrid>, cues: &mut CueState) {
        if t.active_loop().is_some() {
            return;
        }
        let here = cues.snap(grid, t.position());
        self.pending_in = Some(here);
        if t.is_playing() {
            cues.set_cue_here(t, grid);
        }
    }

    pub fn loop_out(&mut self, t: &mut Transport, grid: Option<&DeckGrid>, cues: &CueState) {
        let Some(start) = self.pending_in else {
            return;
        };
        let end = cues.snap(grid, t.position());
        if end - start >= MIN_LOOP_FRAMES && t.set_loop(Loop { start, end }) {
            self.pending_in = None;
        }
    }

    pub fn reloop_exit(&mut self, t: &mut Transport) {
        if t.active_loop().is_some() {
            t.exit_loop();
        } else {
            t.reloop();
        }
    }

    pub fn beat_loop(
        &mut self,
        beats: f64,
        t: &mut Transport,
        grid: Option<&DeckGrid>,
        cues: &CueState,
        sample_rate: u32,
    ) {
        let start = cues.snap(grid, t.position());
        let end = beats_from(grid, sample_rate, start, beats);
        if t.set_loop(Loop { start, end }) {
            self.pending_in = None;
        }
    }

    /// LOOP 1/2X (`factor` 0.5) or LOOP 2X (`factor` 2.0) on the active or stored loop.
    pub fn resize(
        &mut self,
        factor: f64,
        t: &mut Transport,
        grid: Option<&DeckGrid>,
        sample_rate: u32,
    ) {
        let Some(l) = t.stored_loop() else {
            return;
        };
        let beats = length_beats(grid, sample_rate, l) * factor;
        let end = beats_from(grid, sample_rate, l.start, beats);
        let resized = Loop {
            start: l.start,
            end,
        };
        if t.active_loop().is_some() {
            t.set_loop(resized);
        } else if resized.len() >= MIN_LOOP_FRAMES {
            // Stored loop: resize for the next RELOOP without entering it.
            t.set_loop(resized);
            t.exit_loop();
        }
    }

    pub fn beat_jump(
        &mut self,
        beats: f64,
        t: &mut Transport,
        grid: Option<&DeckGrid>,
        sample_rate: u32,
    ) {
        match t.active_loop() {
            Some(l) => {
                let to = beats_from(grid, sample_rate, l.start, beats);
                t.shift_loop(to - l.start);
            }
            None => {
                let to = beats_from(grid, sample_rate, t.position(), beats);
                t.seek(to);
            }
        }
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

    fn playing_at(position: f64) -> Transport {
        let mut t = Transport::new(LEN);
        t.seek(position);
        t.play();
        t
    }

    #[test]
    fn in_out_makes_a_quantized_loop_and_sets_the_cue() {
        let g = grid();
        let mut cues = CueState::default();
        let mut lc = LoopControl::default();
        let mut t = playing_at(49_300.0);
        lc.loop_in(&t, Some(&g), &mut cues);
        assert_eq!(lc.pending_in, Some(49_000.0));
        assert_eq!(cues.cue, 49_000.0, "real-time cue");
        t.seek(144_800.0);
        lc.loop_out(&mut t, Some(&g), &cues);
        assert_eq!(
            t.active_loop(),
            Some(Loop {
                start: 49_000.0,
                end: 145_000.0
            })
        );
        lc.reloop_exit(&mut t);
        assert!(t.active_loop().is_none() && t.stored_loop().is_some());
        lc.reloop_exit(&mut t);
        assert_eq!(t.position(), 49_000.0);
        assert!(t.active_loop().is_some());
    }

    #[test]
    fn beat_loops_follow_the_grid_and_resize_in_beats() {
        let g = grid();
        let cues = CueState::default();
        let mut lc = LoopControl::default();
        let mut t = playing_at(97_100.0); // just after beat 4 (97000)
        lc.beat_loop(4.0, &mut t, Some(&g), &cues, SR);
        assert_eq!(
            t.active_loop().unwrap(),
            Loop {
                start: 97_000.0,
                end: 193_000.0
            }
        );
        lc.resize(0.5, &mut t, Some(&g), SR);
        assert_eq!(t.active_loop().unwrap().end, 145_000.0);
        lc.resize(2.0, &mut t, Some(&g), SR);
        lc.resize(2.0, &mut t, Some(&g), SR);
        assert_eq!(t.active_loop().unwrap().end, 97_000.0 + 8.0 * 24_000.0);
    }

    #[test]
    fn without_a_grid_beats_are_130_bpm_and_unquantized() {
        let cues = CueState::default();
        let mut lc = LoopControl::default();
        let mut t = playing_at(10_123.0);
        lc.beat_loop(1.0, &mut t, None, &cues, SR);
        let l = t.active_loop().unwrap();
        assert_eq!(l.start, 10_123.0);
        assert!((l.len() - 60.0 / 130.0 * 48_000.0).abs() < 1e-9);
    }

    #[test]
    fn beat_jump_moves_the_playhead_or_the_loop() {
        let g = grid();
        let cues = CueState::default();
        let mut lc = LoopControl::default();
        let mut t = playing_at(49_500.0);
        lc.beat_jump(4.0, &mut t, Some(&g), SR);
        assert_eq!(t.position(), 49_500.0 + 4.0 * 24_000.0);
        lc.beat_jump(-8.0, &mut t, Some(&g), SR);
        assert_eq!(
            t.position(),
            0.0,
            "8 beats back from 145500 is before the start: clamped"
        );
        // In a loop: the loop and the playhead move together, the loop stays active.
        let mut t = playing_at(97_100.0);
        lc.beat_loop(2.0, &mut t, Some(&g), &cues, SR);
        lc.beat_jump(4.0, &mut t, Some(&g), SR);
        assert_eq!(
            t.active_loop().unwrap(),
            Loop {
                start: 193_000.0,
                end: 241_000.0
            }
        );
        assert_eq!(t.position(), 97_100.0 + 96_000.0);
    }
}
