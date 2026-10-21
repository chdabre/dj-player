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

/// Actions that the shadow playhead slides underneath while slip is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SlipAction {
    /// Pausing while playing (vinyl-mode pause).
    Pause = 1 << 0,
    /// Playing in reverse.
    Reverse = 1 << 1,
    /// Inside an active loop.
    Loop = 1 << 2,
    /// Moving the track by hand (waveform scrub, later the jog) while it was playing.
    Scratch = 1 << 3,
}

/// The playhead follows a target set by hand (touch on the waveform, later the jog wheel).
///
/// The playhead is pulled to the target by a critically damped spring with time constant
/// [`SCRATCH_TIME_S`], solved exactly per block. So jittery touch updates play as smooth
/// record movement, the track stops softly when the hand stops, and it never overshoots and
/// swings back.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Scratch {
    target: f64,
    /// The spring's velocity at the end of the last block (frames per output frame).
    spring: f64,
    /// The current block's playback step: the spring's average speed over it.
    step: f64,
    /// [`SCRATCH_TIME_S`] in frames at the engine rate.
    tau: f64,
}

/// Time constant of the scratch follower: the playhead is about 2τ behind a moving hand.
pub const SCRATCH_TIME_S: f64 = 0.01;
/// Fastest scratch, in either direction (the loop logic assumes at most 4x).
pub const MAX_SCRATCH_RATE: f64 = 4.0;

/// Shortest loop, in frames. With blocks of at most 1024 frames and rates up to 4x, a block
/// then wraps at most 8 times.
pub const MIN_LOOP_FRAMES: f64 = 512.0;

/// Most pieces one block can be split into.
pub const MAX_SEGMENTS: usize = 16;

/// A loop between two positions (frames), `start < end`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    pub start: f64,
    pub end: f64,
}

impl Loop {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    pub fn contains(&self, position: f64) -> bool {
        position >= self.start && position < self.end
    }
}

/// The pieces of one block: piece `i` starts at output frame `offset` and reads its
/// [`Segment`] until the next piece (or the end of the block).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segments {
    parts: [(usize, Segment); MAX_SEGMENTS],
    len: usize,
}

impl Segments {
    fn new() -> Self {
        Self {
            parts: [(
                0,
                Segment {
                    start: 0.0,
                    step: 0.0,
                },
            ); MAX_SEGMENTS],
            len: 0,
        }
    }

    fn push(&mut self, offset: usize, segment: Segment) {
        if self.len < MAX_SEGMENTS {
            self.parts[self.len] = (offset, segment);
            self.len += 1;
        }
    }

    /// The first piece (the whole block when there is no loop wrap).
    pub fn first(&self) -> Segment {
        self.parts[0].1
    }

    /// (offset, frames, segment) per piece, for a block of `frames` frames.
    pub fn iter(&self, frames: usize) -> impl Iterator<Item = (usize, usize, Segment)> + '_ {
        (0..self.len).map(move |i| {
            let (offset, seg) = self.parts[i];
            let end = if i + 1 < self.len {
                self.parts[i + 1].0
            } else {
                frames
            };
            (offset, end - offset, seg)
        })
    }

    /// Number of pieces: 1 without a loop wrap in the block.
    pub fn count(&self) -> usize {
        self.len
    }
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
            loop_: None,
            looping: false,
            keylock: false,
            scratch: None,
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

    /// Keylock: rate changes speed but not pitch.
    pub fn is_keylock_enabled(&self) -> bool {
        self.keylock
    }

    pub fn set_keylock(&mut self, enabled: bool) {
        self.keylock = enabled;
    }

    /// The active loop, if any.
    pub fn active_loop(&self) -> Option<Loop> {
        self.loop_.filter(|_| self.looping)
    }

    /// The current or last loop (for RELOOP), active or not.
    pub fn stored_loop(&self) -> Option<Loop> {
        self.loop_
    }

    /// Set and enter a loop. Refused (returns false) if shorter than [`MIN_LOOP_FRAMES`] or
    /// not inside the track. The playhead is not moved: it wraps when it reaches the end.
    pub fn set_loop(&mut self, l: Loop) -> bool {
        if l.len() < MIN_LOOP_FRAMES || l.start < 0.0 || l.end > self.length.max(l.start) {
            return false;
        }
        self.loop_ = Some(l);
        if !self.looping {
            self.looping = true;
            self.begin_slip(SlipAction::Loop);
        }
        true
    }

    /// Store a loop for RELOOP without entering it (a memory loop restored at load). Ignored
    /// while a loop is active; refused like [`Transport::set_loop`].
    pub fn store_loop(&mut self, l: Loop) -> bool {
        if self.looping
            || l.len() < MIN_LOOP_FRAMES
            || l.start < 0.0
            || l.end > self.length.max(l.start)
        {
            return false;
        }
        self.loop_ = Some(l);
        true
    }

    /// Leave the active loop; with slip, snap to the shadow.
    pub fn exit_loop(&mut self) {
        if self.looping {
            self.looping = false;
            self.end_slip(SlipAction::Loop);
        }
    }

    /// RELOOP: re-enter the stored loop from its start.
    pub fn reloop(&mut self) {
        if let Some(l) = self.loop_
            && !self.looping
        {
            self.looping = true;
            self.begin_slip(SlipAction::Loop);
            self.position = l.start;
        }
    }

    /// Loop move: shift the loop and the playhead by `frames`, keeping any slip.
    pub fn shift_loop(&mut self, frames: f64) {
        if let Some(l) = self.loop_ {
            let moved = Loop {
                start: l.start + frames,
                end: l.end + frames,
            };
            if moved.start >= 0.0 && moved.end <= self.length {
                self.loop_ = Some(moved);
                if self.looping {
                    self.position = (self.position + frames).clamp(0.0, self.length);
                }
            }
        }
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

    /// True while the playhead is moved by hand.
    pub fn is_scratching(&self) -> bool {
        self.scratch.is_some()
    }

    /// True if the deck makes sound: playing, or moved by hand.
    pub fn is_audible(&self) -> bool {
        self.playing || self.scratch.is_some()
    }

    /// The hand takes the track (`sample_rate` sets the follow and smoothing times). The
    /// playhead holds where it is until [`Transport::scratch_to`] moves the target; play and
    /// rate resume on [`Transport::scratch_end`]. With slip on and the track playing, the
    /// shadow keeps running.
    pub fn scratch_begin(&mut self, sample_rate: u32) {
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
    pub fn scratch_to(&mut self, frame: f64) {
        if let Some(s) = &mut self.scratch
            && frame.is_finite()
        {
            s.target = frame.clamp(0.0, self.length);
        }
    }

    /// Move the scratch target by `frames` (the hand's movement since the last update). No
    /// effect unless scratching.
    pub fn scratch_by(&mut self, frames: f64) {
        if let Some(s) = self.scratch
            && frames.is_finite()
        {
            self.scratch_to(s.target + frames);
        }
    }

    /// The hand lets go: play state and rate as before; with slip, snap to the shadow. A
    /// paused track stays exactly where the hand put it (the target), even if the spring had
    /// not quite caught up.
    pub fn scratch_end(&mut self) {
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
    fn steer(&mut self, frames: usize) {
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
            s.step = (-e0 / t).clamp(-MAX_SCRATCH_RATE, MAX_SCRATCH_RATE);
            s.spring = 0.0;
        } else {
            s.step = ((e1 - e0) / t).clamp(-MAX_SCRATCH_RATE, MAX_SCRATCH_RATE);
            s.spring = v1.clamp(-MAX_SCRATCH_RATE, MAX_SCRATCH_RATE);
        }
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

    /// Jump to `frame` (clamped to the track). Ends any active slip; leaves an active loop if
    /// `frame` is outside it.
    pub fn seek(&mut self, frame: f64) {
        self.position = frame.clamp(0.0, self.length);
        self.shadow = self.position;
        self.slip_held = 0;
        if let Some(l) = self.active_loop()
            && !l.contains(self.position)
        {
            self.looping = false;
        }
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

    /// Advance by `frames` output frames and return the pieces the block should read.
    ///
    /// Reaching the end while moving forward stops playback at the end; reaching the start
    /// in reverse holds at frame 0. An active loop wraps on the exact frame.
    pub fn advance(&mut self, frames: usize) -> Segments {
        self.steer(frames);
        let step = self.velocity();
        let n = frames as f64;
        let mut parts = Segments::new();
        match self.active_loop() {
            Some(l) if step != 0.0 && frames > 0 => {
                let len = l.len();
                // A playhead outside the loop (e.g. after LOOP 1/2X) continues inside it.
                if !l.contains(self.position) {
                    self.position = l.start + (self.position - l.start).rem_euclid(len);
                }
                let mut p = self.position;
                let mut done = 0usize;
                while done < frames && parts.count() < MAX_SEGMENTS {
                    // Output frames until a read leaves [start, end).
                    let until = if step > 0.0 {
                        ((l.end - p) / step).ceil()
                    } else {
                        ((p - l.start) / -step).floor() + 1.0
                    };
                    let take = (until.max(1.0) as usize).min(frames - done);
                    parts.push(done, Segment { start: p, step });
                    p += step * take as f64;
                    done += take;
                    if step > 0.0 && p >= l.end {
                        p -= len;
                    } else if step < 0.0 && p < l.start {
                        p += len;
                    }
                }
                self.position = p;
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

    const LEN: f64 = 100_000_000.0;

    fn playing() -> Transport {
        let mut t = Transport::new(LEN);
        t.play();
        t
    }

    /// Advance in 128-frame blocks for `seconds` at 48 kHz; returns the per-block steps.
    fn run(t: &mut Transport, seconds: f64) -> Vec<f64> {
        (0..(seconds * 48_000.0 / 128.0) as usize)
            .map(|_| t.advance(128).first().step)
            .collect()
    }

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
        let bound = MAX_SCRATCH_RATE * 128.0 / (SCRATCH_TIME_S * 48_000.0);
        assert!(
            max_change <= bound,
            "speed jumps by {max_change} (bound {bound})"
        );
        assert!(steps.iter().all(|s| s.abs() <= MAX_SCRATCH_RATE));
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
    fn keylock_does_not_change_movement() {
        let mut t = playing();
        t.set_rate(1.08);
        let mut k = t;
        k.set_keylock(true);
        assert!(k.is_keylock_enabled() && !t.is_keylock_enabled());
        assert_eq!(t.advance(1000), k.advance(1000));
        assert_eq!(t.position(), k.position());
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
