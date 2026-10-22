//! Loops on the transport: set, store, exit, reloop, move, and the exact-frame wrap.

use super::Transport;
use super::segments::{MAX_SEGMENTS, Segment, Segments};
use super::slip::SlipAction;

/// Shortest loop, in frames. With blocks of at most 1024 frames and rates up to 4x, a block
/// then wraps at most 8 times.
pub(crate) const MIN_LOOP_FRAMES: f64 = 512.0;

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

impl Transport {
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
    pub(crate) fn set_loop(&mut self, l: Loop) -> bool {
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
    pub(crate) fn store_loop(&mut self, l: Loop) -> bool {
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
    pub(crate) fn exit_loop(&mut self) {
        if self.looping {
            self.looping = false;
            self.end_slip(SlipAction::Loop);
        }
    }

    /// RELOOP: re-enter the stored loop from its start.
    pub(crate) fn reloop(&mut self) {
        if let Some(l) = self.loop_
            && !self.looping
        {
            self.looping = true;
            self.begin_slip(SlipAction::Loop);
            self.position = l.start;
        }
    }

    /// Loop move: shift the loop and the playhead by `frames`, keeping any slip.
    pub(crate) fn shift_loop(&mut self, frames: f64) {
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

    /// Move `frames` output frames at `step` (non-zero) through the active loop `l`, wrapping
    /// on the exact frame, and push the pieces read onto `parts`.
    pub(super) fn advance_in_loop(
        &mut self,
        l: Loop,
        step: f64,
        frames: usize,
        parts: &mut Segments,
    ) {
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
}
