//! [`Deck::render`]: one block through the resampler or the stretcher, with the declick of a
//! jump and the crossfade across a loop wrap.

use super::{BlockOutcome, DECLICK_FRAMES, Deck, MAX_BLOCK_FRAMES, WRAP_FADE_FRAMES};
use crate::fade::crossfade;
use crate::scaler::Scaler;
use crate::source::{CHANNELS, Source};
use crate::transport::{Segment, Segments, Transport};

impl Deck {
    /// Render `out.len() / 2` interleaved stereo frames (at most [`MAX_BLOCK_FRAMES`]),
    /// overwriting `out`.
    ///
    /// Real-time safe. If the block would read a chunk that is not loaded yet, the block is
    /// silent, the transport does not move (so no audio is skipped) and the outcome is
    /// [`BlockOutcome::Starved`]. If a command made the playhead jump or switched between
    /// resampler and stretcher, the first [`DECLICK_FRAMES`] frames crossfade from what the
    /// old state would have played.
    pub(crate) fn render(&mut self, out: &mut [f32]) -> BlockOutcome {
        let frames = out.len() / CHANNELS;
        debug_assert!(frames <= MAX_BLOCK_FRAMES, "block too long");
        let fade_from = self.fade_from.take();
        // The source is moved out for the block so the helpers can borrow the rest of the
        // deck; moving the `Arc` neither allocates nor touches its count.
        let Some(source) = self.source.take() else {
            out.fill(0.0);
            self.stretching = false;
            return BlockOutcome::Empty;
        };
        let outcome = self.render_source(&source, fade_from, out);
        self.source = Some(source);
        outcome
    }

    fn render_source(
        &mut self,
        source: &Source,
        fade_from: Option<Transport>,
        out: &mut [f32],
    ) -> BlockOutcome {
        let frames = out.len() / CHANNELS;
        let length = source.len_frames() as f64;
        if length != self.transport.length() {
            self.transport.set_length(length);
        }

        let mut next = self.transport;
        let parts = next.advance(frames);
        let first = parts.first();
        let audible = self.transport.is_audible();
        let stretch = self.uses_keylock(&self.transport);
        let fade = fade_from.filter(|old| {
            self.declick
                && (old.position() != first.start
                    || old.is_audible() != audible
                    || self.uses_keylock(old) != stretch)
        });

        // The outgoing audio first: if it came from the stretcher, it has to be rendered
        // before the stretcher is primed at the new position.
        let fade_len = frames.min(DECLICK_FRAMES);
        if let Some(old) = fade {
            self.render_outgoing(source, old, fade_len);
        }

        if audible {
            // Every piece must be readable before anything is rendered.
            if !self.parts_ready(source, &parts, frames, stretch)
                || !self.render_parts(source, &parts, stretch, fade.is_some(), out)
            {
                out.fill(0.0);
                return BlockOutcome::Starved;
            }
        } else {
            out.fill(0.0);
        }
        self.transport = next;
        self.stretching = audible && stretch;

        if fade.is_some() {
            crossfade(out, &self.scratch[..fade_len * CHANNELS], 0, fade_len);
        }
        BlockOutcome::Rendered
    }

    /// Render the first `fade_len` frames of what `old` would have played into the scratch
    /// buffer: the audio the declick fades out of. Silence if any of it cannot be read.
    fn render_outgoing(&mut self, source: &Source, mut old: Transport, fade_len: usize) {
        let old_stretch = self.uses_keylock(&old);
        let old_audible = old.is_audible();
        let old_parts = old.advance(fade_len);
        let scratch = &mut self.scratch[..fade_len * CHANNELS];
        let mut rendered = old_audible;
        for (off, n, seg) in old_parts.iter(fade_len) {
            let dst = &mut scratch[off * CHANNELS..(off + n) * CHANNELS];
            let (lo, hi) = Scaler::span(seg, n);
            rendered &= match &mut self.keylock {
                Some(k) if old_stretch && k.is_aligned(seg.start) => {
                    k.render(source, seg, dst, false)
                }
                _ if source.range_ready(lo, hi) => {
                    self.scaler.render(source, seg, dst);
                    true
                }
                _ => false,
            };
        }
        if !rendered {
            scratch.fill(0.0);
        }
    }

    /// True if every piece of the block can be read, on the path the block takes.
    fn parts_ready(&self, source: &Source, parts: &Segments, frames: usize, stretch: bool) -> bool {
        match &self.keylock {
            Some(k) if stretch => k.ready(source, parts.iter(frames).map(|(_, n, s)| (n, s))),
            _ => parts.iter(frames).all(|(_, n, seg)| {
                let (lo, hi) = Scaler::span(seg, n);
                source.range_ready(lo, hi)
            }),
        }
    }

    /// Render every piece of the block into `out`, crossfading across loop wraps. `declicked`:
    /// the deck crossfades the start of the block itself. False if the stretcher starved.
    fn render_parts(
        &mut self,
        source: &Source,
        parts: &Segments,
        stretch: bool,
        declicked: bool,
        out: &mut [f32],
    ) -> bool {
        let frames = out.len() / CHANNELS;
        let mut prev: Option<(usize, Segment)> = None;
        for (off, n, seg) in parts.iter(frames) {
            let dst = &mut out[off * CHANNELS..(off + n) * CHANNELS];
            match &mut self.keylock {
                Some(k) if stretch => {
                    // The deck declicks a commanded jump itself (first piece); the keylock
                    // fades the rest (loop wraps).
                    let fade_jump = self.declick && !(off == 0 && declicked);
                    if !k.render(source, seg, dst, fade_jump) {
                        return false;
                    }
                }
                _ => self.scaler.render(source, seg, dst),
            }
            if let Some((prev_off, prev_seg)) = prev
                && !stretch
                && self.declick
            {
                self.wrap_fade(source, prev_off, prev_seg, off, out);
            }
            prev = Some((off, seg));
        }
        true
    }

    /// Loop wrap at output frame `off` (resampler path): crossfade from the audio after the
    /// loop end, where the previous piece (from `prev_off`) would have gone on.
    fn wrap_fade(
        &mut self,
        source: &Source,
        prev_off: usize,
        prev_seg: Segment,
        off: usize,
        out: &mut [f32],
    ) {
        let frames = out.len() / CHANNELS;
        let len = WRAP_FADE_FRAMES.min(frames - off);
        let tail = Segment {
            start: prev_seg.start + (off - prev_off) as f64 * prev_seg.step,
            step: prev_seg.step,
        };
        let (lo, hi) = Scaler::span(tail, len);
        if source.range_ready(lo, hi) {
            let ws = &mut self.wrap_scratch[..len * CHANNELS];
            self.scaler.render(source, tail, ws);
            crossfade(&mut out[off * CHANNELS..(off + len) * CHANNELS], ws, 0, len);
        }
    }
}
