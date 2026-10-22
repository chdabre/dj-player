//! The loaded track's waveform, computed on the loader thread from exactly the frames the
//! deck plays and read by the UI while it grows; or the stored one, once the library has
//! said.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use waveform::compute::{Builder, Entry, Params, Timeline, Waveform};

/// A waveform being computed on the loader thread and read by the UI.
#[derive(Debug, Default)]
pub struct LiveWaveform {
    state: Mutex<LiveState>,
    /// A stored waveform replaced the computation: the tap stops.
    stored: AtomicBool,
}

#[derive(Debug, Default)]
struct LiveState {
    entries: Vec<Entry>,
    done: Option<Waveform>,
    timeline: Timeline,
    /// The first frame at or above the auto cue level: `Some(None)` once the whole track was
    /// decoded without one.
    first_sound: Option<Option<f64>>,
}

impl LiveWaveform {
    fn lock(&self) -> std::sync::MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_timeline(&self, timeline: Timeline) {
        self.lock().timeline = timeline;
    }

    /// Use a stored waveform instead of computing one. False (and ignored) if the
    /// computed one is already complete.
    pub(crate) fn set_stored(&self, w: Waveform) -> bool {
        let mut s = self.lock();
        if s.done.is_some() {
            return false;
        }
        s.entries.clear();
        s.timeline = w.timeline;
        s.done = Some(w);
        self.stored.store(true, Ordering::Relaxed);
        true
    }

    /// Append the entries `have` does not have yet. Returns true once the waveform is
    /// complete (and `have` holds all of it).
    pub fn update(&self, have: &mut Vec<Entry>) -> bool {
        let s = self.lock();
        let entries = s.done.as_ref().map_or(&s.entries, |w| &w.entries);
        if have.len() > entries.len() {
            have.clear();
        }
        have.extend_from_slice(&entries[have.len()..]);
        s.done.is_some()
    }

    /// The first frame (engine rate) at or above the auto cue level, once known; `Some(None)`
    /// if the track has none. `None` while still looking.
    pub fn first_sound(&self) -> Option<Option<f64>> {
        self.lock().first_sound
    }

    /// The finished waveform, with its timeline, once the decode is done.
    pub fn finished(&self) -> Option<Waveform> {
        let s = self.lock();
        s.done.clone().map(|mut w| {
            w.timeline = s.timeline;
            w
        })
    }
}

/// Feeds the decoded audio to a [`Builder`] on the loader thread and publishes the entries
/// every so often. Dropping it (the loader is done) finishes the waveform.
pub(crate) struct WaveTap {
    builder: Option<Builder>,
    live: Arc<LiveWaveform>,
    /// Frames since the last publish.
    pending: usize,
    publish_every: usize,
    /// The auto cue level (linear peak) while still looking for the first sound.
    first_sound: Option<f32>,
    /// Frames seen so far.
    frames: u64,
}

impl WaveTap {
    pub(crate) fn new(sample_rate: u32, live: Arc<LiveWaveform>) -> Self {
        Self {
            builder: Some(Builder::new(sample_rate, &Params::default())),
            live,
            pending: 0,
            // A quarter second of audio: the display grows smoothly, the lock stays rare.
            publish_every: sample_rate as usize / 4,
            first_sound: None,
            frames: 0,
        }
    }

    /// Also look for the first frame whose peak (either channel) reaches `threshold`.
    pub(crate) fn with_first_sound(mut self, threshold: Option<f32>) -> Self {
        self.first_sound = threshold;
        self
    }

    fn find_first_sound(&mut self, stereo: &[f32]) {
        let Some(level) = self.first_sound else {
            return;
        };
        let hit = stereo
            .as_chunks::<2>()
            .0
            .iter()
            .position(|f| f[0].abs().max(f[1].abs()) >= level);
        if let Some(i) = hit {
            self.live.lock().first_sound = Some(Some((self.frames + i as u64) as f64));
            self.first_sound = None;
        }
        self.frames += (stereo.len() / 2) as u64;
    }

    pub(crate) fn boxed(mut self) -> engine_media::Tap {
        Box::new(move |stereo: &[f32]| self.push(stereo))
    }

    fn push(&mut self, stereo: &[f32]) {
        self.find_first_sound(stereo);
        if self.live.stored.load(Ordering::Relaxed) {
            self.builder = None;
        }
        let Some(b) = self.builder.as_mut() else {
            return;
        };
        b.push(stereo);
        self.pending += stereo.len() / 2;
        if self.pending >= self.publish_every {
            self.pending = 0;
            let mut s = self.live.lock();
            let have = s.entries.len();
            s.entries.extend_from_slice(&b.entries()[have..]);
        }
    }
}

impl Drop for WaveTap {
    fn drop(&mut self) {
        if self.first_sound.is_some() {
            // Decoded to the end without reaching the level.
            self.live.lock().first_sound = Some(None);
        }
        if let Some(b) = self.builder.take() {
            let wf = b.finish();
            let mut s = self.live.lock();
            if s.done.is_none() {
                s.entries.clear();
                s.done = Some(wf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AutoCue;

    #[test]
    fn the_live_waveform_grows_during_decode_and_ends_as_computed() {
        let sr = 48_000;
        let audio: Vec<f32> = (0..sr * 3)
            .flat_map(|i| {
                let x = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.5;
                [x, x]
            })
            .collect();
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(sr as u32, Arc::clone(&live)).boxed();
        let mut seen = Vec::new();
        let mut grew = 0;
        for chunk in audio.chunks(4096) {
            tap(chunk);
            let before = seen.len();
            assert!(!live.update(&mut seen), "not complete while decoding");
            grew += usize::from(seen.len() > before);
        }
        assert!(grew >= 8, "published {grew} times");
        drop(tap); // the loader is done
        assert!(live.update(&mut seen));
        let batch = waveform::compute::compute(&audio, sr as u32, &Params::default());
        assert_eq!(seen, batch.entries);
        assert_eq!(live.finished().unwrap().entries, batch.entries);
    }

    #[test]
    fn a_stored_waveform_stops_the_computation() {
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(48_000, Arc::clone(&live)).boxed();
        tap(&vec![0.5; 2 * 48_000]);
        let stored = waveform::compute::compute(&vec![0.1; 4 * 48_000], 48_000, &Params::default());
        assert!(live.set_stored(stored.clone()));
        tap(&vec![0.5; 2 * 48_000]);
        drop(tap);
        assert_eq!(live.finished().unwrap().entries, stored.entries);
        assert!(!live.set_stored(stored), "only before one is complete");
    }

    #[test]
    fn the_tap_finds_the_first_sound_at_the_auto_cue_level() {
        let at = AutoCue::Level(-36.0).threshold();
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(48_000, Arc::clone(&live))
            .with_first_sound(at)
            .boxed();
        // Silence, then a noise floor under the level, then sound in the right channel
        // only, in the second block.
        tap(&vec![0.0; 2 * 1000]);
        assert_eq!(live.first_sound(), None, "still looking");
        let mut block = vec![0.001f32; 2 * 1000];
        block[2 * 250 + 1] = 0.1;
        tap(&block);
        assert_eq!(live.first_sound(), Some(Some(1250.0)));
        drop(tap);
        assert_eq!(live.first_sound(), Some(Some(1250.0)));

        // A track that never reaches the level has no first sound.
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(48_000, Arc::clone(&live))
            .with_first_sound(at)
            .boxed();
        tap(&vec![0.01; 2 * 1000]);
        drop(tap);
        assert_eq!(live.first_sound(), Some(None));
    }
}
