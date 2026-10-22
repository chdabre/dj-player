//! Decoded audio as analysis sees it, and the loader that produces it.

/// Decoded audio as analysis sees it: interleaved stereo at the file's own rate.
#[derive(Debug, Clone)]
pub struct TrackAudio {
    pub sample_rate: u32,
    pub stereo: Vec<f32>,
}

impl TrackAudio {
    pub fn len_frames(&self) -> usize {
        self.stereo.len() / 2
    }

    pub fn duration(&self) -> f64 {
        self.len_frames() as f64 / f64::from(self.sample_rate)
    }

    /// This audio at `rate`, resampled if needed (borrowed if it already is at `rate`).
    pub fn at_rate(&self, rate: u32) -> Result<std::borrow::Cow<'_, TrackAudio>, String> {
        if self.sample_rate == rate {
            return Ok(std::borrow::Cow::Borrowed(self));
        }
        let stereo = engine_media::resample(&self.stereo, self.sample_rate, rate)
            .map_err(|e| format!("resample: {e}"))?;
        Ok(std::borrow::Cow::Owned(TrackAudio {
            sample_rate: rate,
            stereo,
        }))
    }

    /// Mono mixdown (L+R)/2.
    pub fn mono(&self) -> Vec<f32> {
        self.stereo
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| 0.5 * (f[0] + f[1]))
            .collect()
    }
}

/// Decode a file for analysis, at its own sample rate.
pub fn load(path: impl AsRef<std::path::Path>) -> Result<TrackAudio, engine_media::LoadError> {
    load_until(path, &engine_media::Cancel::new())
}

/// [`load`], stopped once `cancel` is set.
pub fn load_until(
    path: impl AsRef<std::path::Path>,
    cancel: &engine_media::Cancel,
) -> Result<TrackAudio, engine_media::LoadError> {
    let (stereo, sample_rate) = engine_media::decode_native_until(path, cancel)?;
    Ok(TrackAudio {
        sample_rate,
        stereo,
    })
}
