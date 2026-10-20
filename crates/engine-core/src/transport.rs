//! `Transport`: playhead, rate, direction and the slip shadow playhead.
//!
//! Pure logic, no audio. Positions are in frames at the engine sample rate, as `f64` so that
//! fractional positions from non-unity rates accumulate without drift over hours.
//!
//! # Slip
//!
//! While slip is enabled, some actions are *slip-able*: they move the audible playhead, while
//! a shadow playhead keeps running underneath at the normal forward rate. When the last held
//! slip-able action ends, the audible playhead snaps to the shadow. In step 1 the slip-able
//! actions are vinyl-style pause and reverse ([`SlipAction`]). Loops, scratch and hot-cue hold
//! join later, with their auto-end rules (reverse after 8 beats, hot cue after 4 beats), once
//! the beatgrid exists.
//!
//! A seek is not slip-able: it moves both playheads and ends any active slip.

/// Actions that the shadow playhead slides underneath while slip is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SlipAction {
    /// Pausing while playing (vinyl-mode pause).
    Pause = 1 << 0,
    /// Playing in reverse.
    Reverse = 1 << 1,
}

/// One block of playhead movement: frame `k` of the block reads the source at
/// `start + k as f64 * step`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub step: f64,
}

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
}

impl Default for Transport {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl Transport {
    /// A stopped transport at frame 0, rate 1.0, for a track of `length` frames.
    pub fn new(length: f64) -> Self {
        Self {
            position: 0.0,
            rate: 1.0,
            reverse: false,
            playing: false,
            length: length.max(0.0),
            slip_enabled: false,
            shadow: 0.0,
            slip_held: 0,
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    /// The shadow playhead. Equals [`Transport::position`] unless a slip is active.
    pub fn shadow_position(&self) -> f64 {
        self.shadow
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

    pub fn is_slip_enabled(&self) -> bool {
        self.slip_enabled
    }

    /// True while at least one slip-able action is held and the shadow runs separately.
    pub fn is_slip_active(&self) -> bool {
        self.slip_held != 0
    }

    /// Playhead movement in frames per output frame: signed, 0 when paused.
    pub fn velocity(&self) -> f64 {
        if !self.playing {
            return 0.0;
        }
        if self.reverse { -self.rate } else { self.rate }
    }

    /// Set the track length (frames). Used while a track is still loading.
    pub fn set_length(&mut self, length: f64) {
        self.length = length.max(0.0);
        self.position = self.position.min(self.length);
        self.shadow = self.shadow.min(self.length);
    }

    pub fn play(&mut self) {
        self.playing = true;
        self.end_slip(SlipAction::Pause);
    }

    pub fn pause(&mut self) {
        if self.playing {
            self.begin_slip(SlipAction::Pause);
        }
        self.playing = false;
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Jump to `frame` (clamped to the track). Ends any active slip.
    pub fn seek(&mut self, frame: f64) {
        self.position = frame.clamp(0.0, self.length);
        self.shadow = self.position;
        self.slip_held = 0;
    }

    pub fn seek_relative(&mut self, frames: f64) {
        self.seek(self.position + frames);
    }

    /// Set the playback rate (1.0 = normal). Any finite value is allowed, including 0 and
    /// negative values; NaN and infinities are ignored.
    pub fn set_rate(&mut self, rate: f64) {
        if rate.is_finite() {
            self.rate = rate;
        }
    }

    pub fn set_reverse(&mut self, reverse: bool) {
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

    /// Enable or disable slip. Disabling it while a slip is active drops the shadow and keeps
    /// playing from the audible position.
    pub fn set_slip(&mut self, enabled: bool) {
        self.slip_enabled = enabled;
        if !enabled {
            self.slip_held = 0;
            self.shadow = self.position;
        }
    }

    fn begin_slip(&mut self, action: SlipAction) {
        if !self.slip_enabled {
            return;
        }
        if self.slip_held == 0 {
            self.shadow = self.position;
        }
        self.slip_held |= action as u8;
    }

    fn end_slip(&mut self, action: SlipAction) {
        if self.slip_held & action as u8 == 0 {
            return;
        }
        self.slip_held &= !(action as u8);
        if self.slip_held == 0 {
            self.position = self.shadow;
        }
    }

    /// Advance by `frames` output frames and return the segment the block should read.
    ///
    /// Reaching the end while moving forward stops playback at the end; reaching the start
    /// in reverse holds at frame 0.
    pub fn advance(&mut self, frames: usize) -> Segment {
        let step = self.velocity();
        let segment = Segment {
            start: self.position,
            step,
        };
        let n = frames as f64;
        self.position += step * n;
        if self.position >= self.length && step > 0.0 {
            self.position = self.length;
            self.playing = false;
        }
        self.position = self.position.clamp(0.0, self.length);
        if self.is_slip_active() {
            self.shadow = (self.shadow + self.rate * n).clamp(0.0, self.length);
        } else {
            self.shadow = self.position;
        }
        segment
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: f64 = 100_000_000.0;

    fn playing() -> Transport {
        let mut t = Transport::new(LEN);
        t.play();
        t
    }

    #[test]
    fn starts_stopped_at_zero() {
        let mut t = Transport::new(LEN);
        assert_eq!(t.velocity(), 0.0);
        let s = t.advance(128);
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
        let s = t.advance(128);
        assert_eq!(
            s,
            Segment {
                start: 0.0,
                step: 1.0
            }
        );
        assert_eq!(t.position(), 128.0);
        assert_eq!(t.advance(64).start, 128.0);
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
        let s = t.advance(100);
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
        let s = t.advance(512);
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
    fn non_finite_rate_is_ignored() {
        let mut t = playing();
        t.set_rate(f64::NAN);
        t.set_rate(f64::INFINITY);
        assert_eq!(t.rate(), 1.0);
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
    fn without_slip_shadow_follows_playhead() {
        let mut t = playing();
        t.set_reverse(true);
        t.seek(1000.0);
        t.advance(100);
        assert!(!t.is_slip_active());
        assert_eq!(t.shadow_position(), t.position());
        t.set_reverse(false);
        assert_eq!(t.position(), 900.0);
    }

    #[test]
    fn slip_reverse_snaps_back_to_shadow() {
        let mut t = playing();
        t.set_slip(true);
        t.seek(10_000.0);
        t.set_rate(1.5);
        t.set_reverse(true);
        assert!(t.is_slip_active());
        t.advance(1000);
        assert_eq!(t.position(), 10_000.0 - 1500.0);
        assert_eq!(t.shadow_position(), 10_000.0 + 1500.0);
        t.set_reverse(false);
        assert!(!t.is_slip_active());
        assert_eq!(t.position(), 11_500.0);
        assert_eq!(t.advance(10).start, 11_500.0);
    }

    #[test]
    fn slip_pause_keeps_shadow_running() {
        let mut t = playing();
        t.set_slip(true);
        t.advance(100);
        t.pause();
        assert!(t.is_slip_active());
        t.advance(400);
        assert_eq!(t.position(), 100.0);
        assert_eq!(t.shadow_position(), 500.0);
        t.play();
        assert_eq!(t.position(), 500.0);
    }

    #[test]
    fn overlapping_slip_actions_snap_after_the_last_one() {
        let mut t = playing();
        t.set_slip(true);
        t.seek(1000.0);
        t.set_reverse(true);
        t.advance(100); // audible 900, shadow 1100
        t.pause();
        t.advance(100); // audible 900, shadow 1200
        t.set_reverse(false);
        assert!(t.is_slip_active(), "pause still held");
        assert_eq!(t.position(), 900.0);
        t.play();
        assert_eq!(t.position(), 1200.0);
    }

    #[test]
    fn pause_while_stopped_is_not_a_slip() {
        let mut t = Transport::new(LEN);
        t.set_slip(true);
        t.pause();
        assert!(!t.is_slip_active());
    }

    #[test]
    fn seek_ends_slip_and_moves_both_playheads() {
        let mut t = playing();
        t.set_slip(true);
        t.set_reverse(true);
        t.seek(5000.0);
        assert!(!t.is_slip_active());
        assert_eq!(t.shadow_position(), 5000.0);
        t.set_reverse(false);
        assert_eq!(t.position(), 5000.0);
    }

    #[test]
    fn disabling_slip_drops_the_shadow() {
        let mut t = playing();
        t.set_slip(true);
        t.seek(1000.0);
        t.set_reverse(true);
        t.advance(100);
        t.set_slip(false);
        assert!(!t.is_slip_active());
        t.set_reverse(false);
        assert_eq!(t.position(), 900.0);
    }
}
