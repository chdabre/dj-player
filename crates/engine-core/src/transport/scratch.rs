//! Scratch: the hand moves the playhead (touch on the waveform, later the jog wheel).

use super::slip::SlipAction;
use super::{MAX_RATE, Transport};

/// Time constant of the scratch follower: the playhead is about 2τ behind a moving hand.
const SCRATCH_TIME_S: f64 = 0.01;

/// The playhead follows a target set by hand (touch on the waveform, later the jog wheel).
///
/// The playhead is pulled to the target by a critically damped spring with time constant
/// [`SCRATCH_TIME_S`], solved exactly per block. So jittery touch updates play as smooth
/// record movement, the track stops softly when the hand stops, and it never overshoots and
/// swings back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Scratch {
    target: f64,
    /// The spring's velocity at the end of the last block (frames per output frame).
    spring: f64,
    /// The current block's playback step: the spring's average speed over it.
    pub(super) step: f64,
    /// [`SCRATCH_TIME_S`] in frames at the engine rate.
    tau: f64,
}

impl Transport {
    /// True while the playhead is moved by hand.
    pub fn is_scratching(&self) -> bool {
        self.scratch.is_some()
    }

    /// The hand takes the track (`sample_rate` sets the follow and smoothing times). The
    /// playhead holds where it is until [`Transport::scratch_to`] moves the target; play and
    /// rate resume on [`Transport::scratch_end`]. With slip on and the track playing, the
    /// shadow keeps running.
    pub(crate) fn scratch_begin(&mut self, sample_rate: u32) {
        if self.scratch.is_some() {
            return;
        }
        let velocity = self.velocity();
        let tau = (SCRATCH_TIME_S * f64::from(sample_rate)).max(1.0);
        self.scratch = Some(Scratch {
            // Catch the record as it moves: with the target one time constant ahead the
            // spring brings it to rest there without swinging back.
            target: (self.position + velocity * tau).clamp(0.0, self.length),
            spring: velocity,
            step: velocity,
            tau,
        });
        if self.playing {
            self.begin_slip(SlipAction::Scratch);
        }
    }

    /// Move the scratch target to `frame` (clamped to the track). No effect unless scratching.
    pub(crate) fn scratch_to(&mut self, frame: f64) {
        if let Some(s) = &mut self.scratch
            && frame.is_finite()
        {
            s.target = frame.clamp(0.0, self.length);
        }
    }

    /// Move the scratch target by `frames` (the hand's movement since the last update). No
    /// effect unless scratching.
    pub(crate) fn scratch_by(&mut self, frames: f64) {
        if let Some(s) = self.scratch
            && frames.is_finite()
        {
            self.scratch_to(s.target + frames);
        }
    }

    /// The hand lets go: play state and rate as before; with slip, snap to the shadow. A
    /// paused track stays exactly where the hand put it (the target), even if the spring had
    /// not quite caught up.
    pub(crate) fn scratch_end(&mut self) {
        if let Some(s) = self.scratch.take() {
            if self.playing {
                self.end_slip(SlipAction::Scratch);
            } else {
                self.position = s.target;
                self.shadow = s.target;
            }
        }
    }

    /// Run the scratch spring for a block of `frames`: the block plays at the spring's
    /// average speed over it, so it lands where the spring is at its end.
    pub(super) fn steer(&mut self, frames: usize) {
        let position = self.position;
        let Some(s) = &mut self.scratch else {
            return;
        };
        let t = frames.max(1) as f64;
        let w = 1.0 / s.tau;
        // Critically damped: e(t) = (e0 + (v0 + w e0) t) exp(-w t), with e the distance past
        // the target and v0 the spring's velocity.
        let e0 = position - s.target;
        let c = s.spring + w * e0;
        let decay = (-w * t).exp();
        let e1 = (e0 + c * t) * decay;
        let v1 = (s.spring - w * c * t) * decay;
        if e1.abs() < 0.5 && v1.abs() < 1e-3 {
            // Coming to rest: land on the target and stop.
            s.step = (-e0 / t).clamp(-MAX_RATE, MAX_RATE);
            s.spring = 0.0;
        } else {
            s.step = ((e1 - e0) / t).clamp(-MAX_RATE, MAX_RATE);
            s.spring = v1.clamp(-MAX_RATE, MAX_RATE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{LEN, playing, run};
    use super::*;

    #[test]
    fn scratch_follows_the_target_smoothly_and_settles_on_it() {
        let mut t = Transport::new(LEN);
        t.seek(100_000.0);
        t.scratch_begin(48_000);
        assert!(t.is_scratching() && t.is_audible() && t.velocity() == 0.0);
        // The finger moves the track 2 s forward over half a second, then holds.
        let mut steps = Vec::new();
        for k in 1..=48 {
            t.scratch_to(100_000.0 + 96_000.0 * f64::from(k) / 48.0);
            steps.extend(run(&mut t, 0.5 / 48.0));
        }
        steps.extend(run(&mut t, 0.3));
        assert!((t.position() - 196_000.0).abs() < 0.5, "{}", t.position());
        assert_eq!(t.velocity(), 0.0, "held still: no movement");
        // No jumps in speed from block to block: the hand's updates are smoothed.
        let max_change = steps
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        // Each block moves the speed at most the smoothing's share of the largest step.
        // A spring with time constant τ changes speed by at most the top speed times one
        // block over τ per block: no steps, whatever the touch updates do.
        let bound = MAX_RATE * 128.0 / (SCRATCH_TIME_S * 48_000.0);
        assert!(
            max_change <= bound,
            "speed jumps by {max_change} (bound {bound})"
        );
        assert!(steps.iter().all(|s| s.abs() <= MAX_RATE));
    }

    #[test]
    fn letting_go_resumes_play_and_slip_snaps_back() {
        let mut t = playing();
        t.set_slip(true);
        run(&mut t, 1.0);
        t.scratch_begin(48_000);
        assert!(t.is_slip_active());
        // Pull the record back 1 s and hold it there for a second.
        t.scratch_by(-48_000.0);
        run(&mut t, 1.0);
        let shadow = t.shadow_position();
        t.scratch_end();
        assert!(!t.is_scratching() && t.is_playing());
        assert_eq!(t.position(), shadow, "slip: back where the track would be");
        assert_eq!(t.velocity(), 1.0);

        // A paused deck: scratching moves it, letting go leaves it paused there.
        let mut t = Transport::new(LEN);
        t.scratch_begin(48_000);
        t.scratch_to(24_000.0);
        run(&mut t, 0.5);
        t.scratch_end();
        assert!(!t.is_playing() && !t.is_audible());
        assert!((t.position() - 24_000.0).abs() < 0.5);
        // Let go before the spring caught up: still exactly where the hand put it.
        t.scratch_begin(48_000);
        t.scratch_by(10_000.0);
        t.advance(128);
        t.scratch_end();
        assert_eq!(t.position(), 34_000.0);
    }

    #[test]
    fn catching_a_playing_record_starts_from_its_speed() {
        let mut t = playing();
        t.set_rate(1.08);
        run(&mut t, 0.1);
        t.scratch_begin(48_000);
        assert!((t.velocity() - 1.08).abs() < 1e-12);
        // Holding the target still brings it to a stop without reversing past it.
        let target = t.position();
        let steps = run(&mut t, 0.2);
        assert!(steps.iter().all(|&s| s >= -1e-9), "it never runs backwards");
        assert!(t.position() >= target);
    }
}
