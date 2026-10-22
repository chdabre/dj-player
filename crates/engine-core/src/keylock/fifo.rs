//! The keylock path's output FIFO: stereo frames, each tagged with the source position it plays.

use crate::source::CHANNELS;

/// Fixed-capacity FIFO of interleaved stereo frames, each with the source position it plays.
pub(super) struct Fifo {
    audio: Box<[f32]>,
    pub(super) position: Box<[f64]>,
    pub(super) head: usize,
    pub(super) len: usize,
}

impl Fifo {
    pub(super) fn new(frames: usize) -> Self {
        Self {
            audio: vec![0.0; frames * CHANNELS].into_boxed_slice(),
            position: vec![0.0; frames].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.position.len()
    }

    pub(super) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Position of the next frame out, if any.
    pub(super) fn head_position(&self) -> Option<f64> {
        (self.len > 0).then(|| self.position[self.head])
    }

    /// Append a frame. The FIFO is sized so it never fills; if it did, the frame would be
    /// dropped rather than panic on the audio thread.
    pub(super) fn push(&mut self, frame: [f32; CHANNELS], position: f64) {
        debug_assert!(self.len < self.capacity(), "keylock fifo overflow");
        if self.len == self.capacity() {
            return;
        }
        let at = (self.head + self.len) % self.capacity();
        self.audio[at * CHANNELS..(at + 1) * CHANNELS].copy_from_slice(&frame);
        self.position[at] = position;
        self.len += 1;
    }

    /// Copy the first `out.len() / 2` frames into `out` without removing them; frames the
    /// FIFO does not hold are silence.
    pub(super) fn peek(&self, out: &mut [f32]) {
        let frames = (out.len() / CHANNELS).min(self.len);
        for (k, dst) in out.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            if k < frames {
                let at = (self.head + k) % self.capacity();
                dst.copy_from_slice(&self.audio[at * CHANNELS..(at + 1) * CHANNELS]);
            } else {
                dst.fill(0.0);
            }
        }
    }

    /// Fill `out` from the front and remove those frames. The path always queues enough
    /// first; a shortfall would be silence rather than a panic on the audio thread.
    pub(super) fn pop(&mut self, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        debug_assert!(frames <= self.len, "keylock fifo underrun");
        self.peek(out);
        let frames = frames.min(self.len);
        self.head = (self.head + frames) % self.capacity();
        self.len -= frames;
    }
}
