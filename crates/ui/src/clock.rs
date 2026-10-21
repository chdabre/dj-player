//! Playhead interpolation: a smooth position for every display frame from the engine's
//! position, which only moves once per audio callback (every 5–20 ms, not in step with
//! vsync).
//!
//! Each time the published position changes, we note when we saw it. The engine's position
//! "now" is estimated as that value plus the time since, at the playing speed. The shown
//! position runs at the playing speed too and is pulled towards the estimate with a short time
//! constant, so callback jitter never shows as a stutter. Jumps (seek, cue, loop wrap, slip
//! snap-back) and pausing snap straight to the engine.

use std::time::Instant;

/// How quickly the shown playhead converges on the engine's (seconds).
const TIME_CONSTANT: f64 = 0.05;
/// Errors beyond this (seconds of audio) are a jump, not drift: snap.
const SNAP_SECONDS: f64 = 0.05;
/// Never extrapolate further than this past the last engine update (seconds): if the engine
/// stalls, the playhead stops too.
const MAX_EXTRAPOLATION: f64 = 0.1;

#[derive(Debug, Clone, Copy)]
pub struct PlayheadClock {
    sample_rate: f64,
    /// Last position the engine published, and when we first saw it.
    raw: f64,
    raw_seen: Option<Instant>,
    shown: f64,
    last_frame: Option<Instant>,
}

impl PlayheadClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            raw: 0.0,
            raw_seen: None,
            shown: 0.0,
            last_frame: None,
        }
    }

    /// The position to draw at `now`, given the engine's published `position` (frames) and
    /// its playing speed in frames of track per frame of output (signed; 0 when paused).
    pub fn update(&mut self, now: Instant, position: f64, speed: f64) -> f64 {
        if self.raw_seen.is_none() || position != self.raw {
            self.raw = position;
            self.raw_seen = Some(now);
        }
        let dt = self
            .last_frame
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f64());
        self.last_frame = Some(now);
        if speed == 0.0 {
            self.shown = position;
            return position;
        }
        let since = self
            .raw_seen
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f64());
        let fps = speed * self.sample_rate;
        let estimate = self.raw + since.min(MAX_EXTRAPOLATION) * fps;
        // A stalled engine (no new position for a while): stop running and settle on it.
        let stalled = since >= MAX_EXTRAPOLATION;
        let advanced = if stalled {
            self.shown
        } else {
            self.shown + dt * fps
        };
        let error = estimate - advanced;
        self.shown = if error.abs() > SNAP_SECONDS * self.sample_rate {
            estimate
        } else {
            advanced + error * (1.0 - (-dt / TIME_CONSTANT).exp())
        };
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SR: u32 = 48_000;

    /// The engine publishes every `block` frames at 48 kHz; the display runs at `hz`.
    fn run(block: usize, hz: f64, seconds: f64) -> Vec<(f64, f64)> {
        let start = Instant::now();
        let mut clock = PlayheadClock::new(SR);
        let mut out = Vec::new();
        let frames = (seconds * hz) as usize;
        for k in 0..frames {
            let t = k as f64 / hz;
            let published = (t * f64::from(SR) / block as f64).floor() * block as f64;
            let shown = clock.update(start + Duration::from_secs_f64(t), published, 1.0);
            out.push((t * f64::from(SR), shown));
        }
        out
    }

    #[test]
    fn tracks_the_true_position_without_steps() {
        for (block, hz) in [(256, 60.0), (480, 60.0), (1024, 60.0), (512, 120.0)] {
            let frames = run(block, hz, 3.0);
            // After settling: within one callback of the truth, and every frame advances by
            // close to the nominal step (no stutter at callback boundaries).
            let step = f64::from(SR) / hz;
            for w in frames[30..].windows(2) {
                let (truth, shown) = w[1];
                assert!(
                    (truth - shown).abs() <= block as f64,
                    "block {block} @ {hz} Hz: shown {shown} vs true {truth}"
                );
                let d = w[1].1 - w[0].1;
                assert!(
                    (d - step).abs() < 0.35 * step,
                    "block {block} @ {hz} Hz: step {d} vs {step}"
                );
            }
        }
    }

    #[test]
    fn jumps_snap_and_pause_holds() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut c = PlayheadClock::new(SR);
        c.update(at(0), 0.0, 1.0);
        c.update(at(16), 768.0, 1.0);
        let shown = c.update(at(33), 500_000.0, 1.0);
        assert_eq!(shown, 500_000.0, "a seek snaps");
        assert_eq!(
            c.update(at(50), 500_000.0, 0.0),
            500_000.0,
            "paused: exactly the engine"
        );
    }

    #[test]
    fn a_stalled_engine_stops_the_playhead() {
        let start = Instant::now();
        let mut c = PlayheadClock::new(SR);
        let mut shown = 0.0;
        for k in 0..60 {
            shown = c.update(start + Duration::from_millis(k * 16), 1_000.0, 1.0);
        }
        assert!(shown <= 1_000.0 + MAX_EXTRAPOLATION * f64::from(SR) + 1.0);
    }
}
