//! `Transport`: playhead, rate, direction and the slip shadow playhead.
//!
//! Pure logic, no audio. Positions are in frames at the engine sample rate, as `f64` so that
//! fractional positions from non-unity rates accumulate without drift over hours.
//!
//! # Slip
//!
//! While slip is enabled, some actions are *slip-able*: they move the audible playhead, while
//! a shadow playhead keeps running underneath at the normal forward rate. When the last held
//! slip-able action ends, the audible playhead snaps to the shadow. The slip-able actions
//! ([`SlipAction`]) are vinyl-style pause, reverse, an active loop and scratch. Hot-cue hold
//! joins later, as do the auto-end rules (reverse after 8 beats, hot cue after 4 beats).
//!
//! A seek is not slip-able: it moves both playheads and ends any active slip.
//!
//! # Loops
//!
//! An active loop wraps the playhead from its end back to its start (and from its start to
//! its end in reverse) on the exact frame, so one block may read several pieces:
//! [`Transport::advance`] returns them as [`Segments`]. A loop is slip-able: with slip on, the
//! shadow keeps running straight and exiting the loop snaps to it. The loop is remembered
//! after exit for RELOOP. Loops shorter than [`MIN_LOOP_FRAMES`] are refused, which bounds
//! the number of pieces per block.
//!
//! # Keylock
//!
//! Keylock is a flag here and changes nothing about how the playheads move:
//! the rate is the speed either way. The deck reads it to choose between the resampler and the
//! time-stretcher.

mod loops;
mod scratch;
mod segments;
mod slip;

pub use loops::Loop;
pub(crate) use loops::MIN_LOOP_FRAMES;
pub(crate) use segments::{Segment, Segments};
pub(crate) use slip::SlipAction;

use scratch::Scratch;

/// Fastest playback, in either direction, by rate or by hand. The loop logic assumes at most
/// this: see [`MIN_LOOP_FRAMES`].
pub(crate) const MAX_RATE: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    position: f64,
    rate: f64,
    reverse: bool,
    playing: bool,
    length: f64,
    slip_enabled: bool,
    shadow: f64,
    slip_held: u8,
    /// The current or last loop; `looping` says whether it is active.
    loop_: Option<Loop>,
    looping: bool,
    keylock: bool,
    scratch: Option<Scratch>,
}

impl Default for Transport {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl Transport {
    /// A stopped transport at frame 0, rate 1.0, for a track of `length` frames.
    pub(crate) fn new(length: f64) -> Self {
        Self {
            position: 0.0,
            rate: 1.0,
            reverse: false,
            playing: false,
            length: length.max(0.0),
            slip_enabled: false,
            shadow: 0.0,
            slip_held: 0,
            loop_: None,
            looping: false,
            keylock: false,
            scratch: None,
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn is_reverse(&self) -> bool {
        self.reverse
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn length(&self) -> f64 {
        self.length
    }

    /// Keylock: rate changes speed but not pitch.
    pub fn is_keylock_enabled(&self) -> bool {
        self.keylock
    }

    pub(crate) fn set_keylock(&mut self, enabled: bool) {
        self.keylock = enabled;
    }

    /// Playhead movement in frames per output frame: signed, 0 when paused. While
    /// scratching, the hand's velocity.
    pub fn velocity(&self) -> f64 {
        if let Some(s) = self.scratch {
            return s.step;
        }
        if !self.playing {
            return 0.0;
        }
        if self.reverse { -self.rate } else { self.rate }
    }

    /// True if the deck makes sound: playing, or moved by hand.
    pub fn is_audible(&self) -> bool {
        self.playing || self.scratch.is_some()
    }

    /// Set the track length (frames). Used while a track is still loading.
    pub(crate) fn set_length(&mut self, length: f64) {
        self.length = length.max(0.0);
        self.position = self.position.min(self.length);
        self.shadow = self.shadow.min(self.length);
    }

    pub(crate) fn play(&mut self) {
        self.playing = true;
        self.end_slip(SlipAction::Pause);
    }

    pub(crate) fn pause(&mut self) {
        if self.playing {
            self.begin_slip(SlipAction::Pause);
        }
        self.playing = false;
    }

    /// Jump to `frame` (clamped to the track). Ends any active slip; leaves an active loop if
    /// `frame` is outside it.
    pub(crate) fn seek(&mut self, frame: f64) {
        self.position = frame.clamp(0.0, self.length);
        self.shadow = self.position;
        self.slip_held = 0;
        if let Some(l) = self.active_loop()
            && !l.contains(self.position)
        {
            self.looping = false;
        }
    }

    pub(crate) fn seek_relative(&mut self, frames: f64) {
        self.seek(self.position + frames);
    }

    /// Set the playback rate (1.0 = normal), clamped to ±[`MAX_RATE`]. 0 and negative values
    /// are allowed; NaN is ignored.
    pub(crate) fn set_rate(&mut self, rate: f64) {
        if !rate.is_nan() {
            self.rate = rate.clamp(-MAX_RATE, MAX_RATE);
        }
    }

    pub(crate) fn set_reverse(&mut self, reverse: bool) {
        if reverse == self.reverse {
            return;
        }
        self.reverse = reverse;
        if reverse {
            if self.playing {
                self.begin_slip(SlipAction::Reverse);
            }
        } else {
            self.end_slip(SlipAction::Reverse);
        }
    }

    /// Advance by `frames` output frames and return the pieces the block should read.
    ///
    /// Reaching the end while moving forward stops playback at the end; reaching the start
    /// in reverse holds at frame 0. An active loop wraps on the exact frame.
    pub(crate) fn advance(&mut self, frames: usize) -> Segments {
        self.steer(frames);
        let step = self.velocity();
        let n = frames as f64;
        let mut parts = Segments::new();
        match self.active_loop() {
            Some(l) if step != 0.0 && frames > 0 => {
                self.advance_in_loop(l, step, frames, &mut parts)
            }
            _ => {
                parts.push(
                    0,
                    Segment {
                        start: self.position,
                        step,
                    },
                );
                self.position += step * n;
                if self.position >= self.length && step > 0.0 {
                    self.position = self.length;
                    if self.scratch.is_none() {
                        self.playing = false;
                    }
                }
            }
        }
        self.position = self.position.clamp(0.0, self.length);
        if self.is_slip_active() {
            self.shadow = (self.shadow + self.rate * n).clamp(0.0, self.length);
        } else {
            self.shadow = self.position;
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) const LEN: f64 = 100_000_000.0;

    pub(super) fn playing() -> Transport {
        let mut t = Transport::new(LEN);
        t.play();
        t
    }

    /// Advance in 128-frame blocks for `seconds` at 48 kHz; returns the per-block steps.
    pub(super) fn run(t: &mut Transport, seconds: f64) -> Vec<f64> {
        (0..(seconds * 48_000.0 / 128.0) as usize)
            .map(|_| t.advance(128).first().step)
            .collect()
    }

    #[test]
    fn starts_stopped_at_zero() {
        let mut t = Transport::new(LEN);
        assert_eq!(t.velocity(), 0.0);
        let s = t.advance(128).first();
        assert_eq!(
            s,
            Segment {
                start: 0.0,
                step: 0.0
            }
        );
        assert_eq!(t.position(), 0.0);
    }

    #[test]
    fn plays_at_unity_rate() {
        let mut t = playing();
        let s = t.advance(128).first();
        assert_eq!(
            s,
            Segment {
                start: 0.0,
                step: 1.0
            }
        );
        assert_eq!(t.position(), 128.0);
        assert_eq!(t.advance(64).first().start, 128.0);
    }

    #[test]
    fn fractional_rate_accumulates_exactly() {
        let mut t = playing();
        t.set_rate(1.01);
        for _ in 0..10_000 {
            t.advance(128);
        }
        let expected = 1.01 * 128.0 * 10_000.0;
        assert!((t.position() - expected).abs() < 1e-6, "{}", t.position());
    }

    #[test]
    fn reverse_moves_backwards_and_holds_at_zero() {
        let mut t = playing();
        t.seek(1000.0);
        t.set_reverse(true);
        let s = t.advance(100).first();
        assert_eq!(
            s,
            Segment {
                start: 1000.0,
                step: -1.0
            }
        );
        assert_eq!(t.position(), 900.0);
        t.advance(2000);
        assert_eq!(t.position(), 0.0);
        assert!(t.is_playing());
    }

    #[test]
    fn negative_rate_is_reverse_too() {
        let mut t = playing();
        t.seek(500.0);
        t.set_rate(-0.5);
        assert_eq!(t.velocity(), -0.5);
        t.advance(100);
        assert_eq!(t.position(), 450.0);
        // Reverse of a negative rate plays forward.
        t.set_reverse(true);
        assert_eq!(t.velocity(), 0.5);
    }

    #[test]
    fn rate_zero_holds_position_while_playing() {
        let mut t = playing();
        t.seek(42.5);
        t.set_rate(0.0);
        let s = t.advance(512).first();
        assert_eq!(
            s,
            Segment {
                start: 42.5,
                step: 0.0
            }
        );
        assert_eq!(t.position(), 42.5);
        assert!(t.is_playing());
    }

    #[test]
    fn rate_is_clamped_and_nan_ignored() {
        let mut t = playing();
        t.set_rate(f64::NAN);
        assert_eq!(t.rate(), 1.0);
        t.set_rate(f64::INFINITY);
        assert_eq!(t.rate(), MAX_RATE);
        t.set_rate(-100.0);
        assert_eq!(t.rate(), -MAX_RATE);
    }

    #[test]
    fn fastest_block_on_shortest_loop_stays_inside() {
        let mut t = playing();
        t.set_rate(1000.0);
        assert!(t.set_loop(Loop {
            start: 1000.0,
            end: 1000.0 + MIN_LOOP_FRAMES,
        }));
        t.seek(1000.0);
        let frames = crate::deck::MAX_BLOCK_FRAMES;
        let parts = t.advance(frames);
        assert!(parts.count() < segments::MAX_SEGMENTS);
        for (i, (_, len, seg)) in parts.iter(frames).enumerate() {
            let last = seg.start + seg.step * (len - 1) as f64;
            assert!(
                seg.start >= 1000.0 && last < 1000.0 + MIN_LOOP_FRAMES,
                "piece {i}"
            );
        }
    }

    #[test]
    fn stops_at_end() {
        let mut t = Transport::new(1000.0);
        t.play();
        t.seek(900.0);
        t.advance(200);
        assert_eq!(t.position(), 1000.0);
        assert!(!t.is_playing());
    }

    #[test]
    fn seek_clamps() {
        let mut t = Transport::new(1000.0);
        t.seek(-5.0);
        assert_eq!(t.position(), 0.0);
        t.seek(5000.0);
        assert_eq!(t.position(), 1000.0);
        t.seek(10.0);
        t.seek_relative(-20.0);
        assert_eq!(t.position(), 0.0);
    }

    #[test]
    fn keylock_does_not_change_movement() {
        let mut t = playing();
        t.set_rate(1.08);
        let mut k = t;
        k.set_keylock(true);
        assert!(k.is_keylock_enabled() && !t.is_keylock_enabled());
        assert_eq!(t.advance(1000), k.advance(1000));
        assert_eq!(t.position(), k.position());
    }
}
