use std::fmt;

use engine_core::CHANNELS;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::LoadError;

/// Frames per resampler call.
const RESAMPLE_CHUNK: usize = 1024;

/// Resample interleaved stereo from `from` Hz to `to` Hz in one go, with the same resampler
/// (and delay trimming, so the output lines up with the input) as track loading.
pub fn resample(stereo: &[f32], from: u32, to: u32) -> Result<Vec<f32>, LoadError> {
    if from == to {
        return Ok(stereo.to_vec());
    }
    resample_all(stereo, from, to)
}

pub(crate) fn resample_all(stereo: &[f32], from: u32, to: u32) -> Result<Vec<f32>, LoadError> {
    let mut out = Vec::with_capacity(
        (stereo.len() as u64 * u64::from(to)).div_ceil(u64::from(from)) as usize,
    );
    let mut resampler = StreamResampler::new(from, to)?;
    resampler.push(stereo, &mut out)?;
    resampler.flush(&mut out)?;
    Ok(out)
}

/// Where resampled stereo goes: the source being loaded, or a plain vector.
pub(crate) trait FrameOut {
    fn put(&mut self, stereo: &[f32]);
}

impl FrameOut for Vec<f32> {
    fn put(&mut self, stereo: &[f32]) {
        self.extend_from_slice(stereo);
    }
}

/// Chunked load-time resampler with the startup delay trimmed, matching what rubato's
/// `process_all` does for a whole clip, but streaming so the source fills progressively.
pub(crate) struct StreamResampler {
    inner: Fft<f32>,
    pending: Vec<f32>,
    out: Vec<f32>,
    to_trim: usize,
    input_frames: usize,
    output_frames: usize,
    from: u64,
    to: u64,
}

impl StreamResampler {
    pub(crate) fn new(from: u32, to: u32) -> Result<Self, LoadError> {
        let inner = Fft::<f32>::new(
            from as usize,
            to as usize,
            RESAMPLE_CHUNK,
            CHANNELS,
            FixedSync::Input,
        )
        .map_err(|e| LoadError::Resample(e.to_string()))?;
        let out = vec![0.0; inner.output_frames_max() * CHANNELS];
        let to_trim = inner.output_delay();
        Ok(Self {
            inner,
            pending: Vec::new(),
            out,
            to_trim,
            input_frames: 0,
            output_frames: 0,
            from: u64::from(from),
            to: u64::from(to),
        })
    }

    /// Output length for everything pushed so far: ceil(input * to / from), exact.
    fn expected_output(&self) -> usize {
        (self.input_frames as u64 * self.to).div_ceil(self.from) as usize
    }

    pub(crate) fn push(
        &mut self,
        stereo: &[f32],
        writer: &mut impl FrameOut,
    ) -> Result<(), LoadError> {
        self.pending.extend_from_slice(stereo);
        self.input_frames += stereo.len() / CHANNELS;
        let need = self.inner.input_frames_next();
        let pending = std::mem::take(&mut self.pending);
        let mut offset = 0;
        let mut result = Ok(());
        while result.is_ok() && pending.len() - offset >= need * CHANNELS {
            let input = &pending[offset..offset + need * CHANNELS];
            result = self.process(input, need, None, writer);
            offset += need * CHANNELS;
        }
        self.pending = pending;
        self.pending.drain(..offset);
        result
    }

    pub(crate) fn flush(&mut self, writer: &mut impl FrameOut) -> Result<(), LoadError> {
        let expected = self.expected_output();
        let need = self.inner.input_frames_next();
        let mut last = std::mem::take(&mut self.pending);
        let partial = last.len() / CHANNELS;
        last.resize(need * CHANNELS, 0.0);
        self.process(&last, need, Some(partial), writer)?;
        let zeros = vec![0.0; need * CHANNELS];
        while self.output_frames < expected {
            let before = self.output_frames + self.to_trim;
            self.process(&zeros, need, Some(0), writer)?;
            if self.output_frames + self.to_trim == before {
                break;
            }
        }
        Ok(())
    }

    fn process(
        &mut self,
        input: &[f32],
        frames: usize,
        partial: Option<usize>,
        writer: &mut impl FrameOut,
    ) -> Result<(), LoadError> {
        let err = |e: &dyn fmt::Display| LoadError::Resample(e.to_string());
        let in_adapter = InterleavedSlice::new(input, CHANNELS, frames).map_err(|e| err(&e))?;
        let cap = self.out.len() / CHANNELS;
        let mut out_adapter =
            InterleavedSlice::new_mut(&mut self.out, CHANNELS, cap).map_err(|e| err(&e))?;
        let mut indexing = Indexing::new();
        if let Some(p) = partial {
            indexing = indexing.partial_len(p);
        }
        let (_, written) = self
            .inner
            .process_into_buffer(&in_adapter, &mut out_adapter, Some(&indexing))
            .map_err(|e| err(&e))?;
        let skip = self.to_trim.min(written);
        self.to_trim -= skip;
        let keep = written - skip;
        // Never write past the expected length on the final flush.
        let expected = self.expected_output();
        let keep = if partial.is_some() {
            keep.min(expected.saturating_sub(self.output_frames))
        } else {
            keep
        };
        writer.put(&self.out[skip * CHANNELS..(skip + keep) * CHANNELS]);
        self.output_frames += keep;
        Ok(())
    }
}
