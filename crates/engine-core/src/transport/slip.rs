//! Slip: the shadow playhead that runs on underneath slip-able actions (see the
//! [module docs](super)).

use super::Transport;

/// Actions that the shadow playhead slides underneath while slip is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SlipAction {
    /// Pausing while playing (vinyl-mode pause).
    Pause = 1 << 0,
    /// Playing in reverse.
    Reverse = 1 << 1,
    /// Inside an active loop.
    Loop = 1 << 2,
    /// Moving the track by hand (waveform scrub, later the jog) while it was playing.
    Scratch = 1 << 3,
}

impl Transport {
    /// The shadow playhead. Equals [`Transport::position`] unless a slip is active.
    pub fn shadow_position(&self) -> f64 {
        self.shadow
    }

    pub fn is_slip_enabled(&self) -> bool {
        self.slip_enabled
    }

    /// True while at least one slip-able action is held and the shadow runs separately.
    pub fn is_slip_active(&self) -> bool {
        self.slip_held != 0
    }

    /// Enable or disable slip. Disabling it while a slip is active drops the shadow and keeps
    /// playing from the audible position.
    pub(crate) fn set_slip(&mut self, enabled: bool) {
        self.slip_enabled = enabled;
        if !enabled {
            self.slip_held = 0;
            self.shadow = self.position;
        }
    }

    pub(super) fn begin_slip(&mut self, action: SlipAction) {
        if !self.slip_enabled {
            return;
        }
        if self.slip_held == 0 {
            self.shadow = self.position;
        }
        self.slip_held |= action as u8;
    }

    pub(super) fn end_slip(&mut self, action: SlipAction) {
        if self.slip_held & action as u8 == 0 {
            return;
        }
        self.slip_held &= !(action as u8);
        if self.slip_held == 0 {
            self.position = self.shadow;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{LEN, playing};
    use super::*;

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
        assert_eq!(t.advance(10).first().start, 11_500.0);
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
