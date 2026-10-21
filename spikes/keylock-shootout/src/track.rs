//! A resident stereo track and a small output FIFO, shared by every candidate.

use std::path::Path;

pub const CHANNELS: usize = 2;

/// Interleaved stereo at the engine rate. Reads outside the track are silence.
pub struct Track {
    pub name: String,
    pub samples: Vec<f32>,
}

impl Track {
    pub fn load(path: &Path, sample_rate: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let samples = engine_media::decode_to_vec(path, sample_rate)?;
        let name = path
            .file_stem()
            .map_or_else(|| "track".into(), |s| s.to_string_lossy().into_owned());
        Ok(Self { name, samples })
    }

    pub fn from_samples(name: &str, samples: Vec<f32>) -> Self {
        Self {
            name: name.into(),
            samples,
        }
    }

    pub fn len_frames(&self) -> usize {
        self.samples.len() / CHANNELS
    }

    #[inline]
    pub fn frame(&self, index: i64) -> [f32; CHANNELS] {
        if index < 0 || index as usize >= self.len_frames() {
            return [0.0; CHANNELS];
        }
        let i = index as usize * CHANNELS;
        [self.samples[i], self.samples[i + 1]]
    }

    /// Copy `dst.len() / 2` frames starting at `first`, interleaved.
    pub fn copy_interleaved(&self, first: i64, dst: &mut [f32]) {
        for (k, f) in dst.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            f.copy_from_slice(&self.frame(first + k as i64));
        }
    }

    /// Copy `frames` frames starting at `first` into one plane per channel, `stride` apart.
    pub fn copy_planar(&self, first: i64, frames: usize, dst: &mut [f32], stride: usize) {
        for k in 0..frames {
            let f = self.frame(first + k as i64);
            for (c, s) in f.iter().enumerate() {
                dst[c * stride + k] = *s;
            }
        }
    }
}

/// Fixed-capacity interleaved FIFO. Never allocates after construction.
pub struct Fifo {
    buf: Box<[f32]>,
    len: usize,
}

impl Fifo {
    pub fn new(frames: usize) -> Self {
        Self {
            buf: vec![0.0; frames * CHANNELS].into_boxed_slice(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn room(&self) -> usize {
        self.buf.len() / CHANNELS - self.len
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Append frames from per-channel planes (`stride` apart). Panics if there is no room.
    pub fn push_planar(&mut self, data: &[f32], frames: usize, stride: usize) {
        assert!(frames <= self.room(), "fifo overflow");
        let base = self.len * CHANNELS;
        for k in 0..frames {
            for c in 0..CHANNELS {
                self.buf[base + k * CHANNELS + c] = data[c * stride + k];
            }
        }
        self.len += frames;
    }

    /// Fill `out` from the front; frames the FIFO cannot supply are zero. Returns the number of
    /// frames that were missing.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        let want = out.len() / CHANNELS;
        let n = want.min(self.len);
        out[..n * CHANNELS].copy_from_slice(&self.buf[..n * CHANNELS]);
        out[n * CHANNELS..].fill(0.0);
        self.buf.copy_within(n * CHANNELS..self.len * CHANNELS, 0);
        self.len -= n;
        want - n
    }
}
