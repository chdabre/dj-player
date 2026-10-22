use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use engine_core::{Source, SourceWriter};

use crate::decode::{Decoding, StereoSink};
use crate::resample::{FrameOut, StreamResampler, resample_all};
use crate::{Cancel, LoadError, TrackInfo};

/// Extra capacity on top of the estimated length, for rounding in the resampler and for
/// containers whose frame count is slightly off.
const CAPACITY_MARGIN_FRAMES: usize = 4096;

/// A track being loaded: the source is usable right away; `thread` finishes the decode, and
/// `cancel` stops it early.
#[derive(Debug)]
pub struct Loading {
    pub info: TrackInfo,
    pub source: Arc<Source>,
    pub thread: JoinHandle<Result<(), LoadError>>,
    pub cancel: Cancel,
}

/// Sees every chunk of interleaved stereo the loader writes into the source, at the engine
/// rate and in order, on the loader thread: what the deck will play, frame for frame. Used to
/// compute the waveform during decode.
pub type Tap = Box<dyn FnMut(&[f32]) + Send>;

/// Probe `path` and start decoding it into a new [`Source`] at `sample_rate` on a loader
/// thread.
///
/// If the container does not state the track length, the whole file is decoded on the calling
/// thread first (the source is then complete when this returns).
pub fn load_file(path: impl AsRef<Path>, sample_rate: u32) -> Result<Loading, LoadError> {
    load_file_tapped(path, sample_rate, None)
}

/// [`load_file`], with `tap` seeing the audio as it is written into the source.
pub fn load_file_tapped(
    path: impl AsRef<Path>,
    sample_rate: u32,
    mut tap: Option<Tap>,
) -> Result<Loading, LoadError> {
    let path = path.as_ref().to_path_buf();
    let cancel = Cancel::new();
    let mut decoding = Decoding::open(&path)?;
    decoding.cancel = Some(cancel.clone());
    let info = TrackInfo {
        path: path.clone(),
        file_sample_rate: decoding.sample_rate,
        file_channels: decoding.channels,
        file_frames: decoding.frames,
        sample_rate,
        start_trim: decoding.start_trim,
    };
    let ratio = sample_rate as f64 / decoding.sample_rate as f64;

    match decoding.frames {
        Some(frames) => {
            let capacity = (frames as f64 * ratio).ceil() as usize + CAPACITY_MARGIN_FRAMES;
            let (source, writer) = Source::new(sample_rate, capacity);
            let thread = std::thread::Builder::new()
                .name("loader".into())
                .spawn(move || {
                    let mut sink = Sink::new(writer, tap, decoding.sample_rate, sample_rate)?;
                    let result = decoding.run(&mut sink);
                    sink.finish()?;
                    result
                })?;
            Ok(Loading {
                info,
                source,
                thread,
                cancel,
            })
        }
        None => {
            let mut collected = Vec::new();
            decoding.run(&mut |s: &[f32]| collected.extend_from_slice(s))?;
            let resampled = if decoding.sample_rate == sample_rate {
                collected
            } else {
                resample_all(&collected, decoding.sample_rate, sample_rate)?
            };
            if let Some(tap) = tap.as_mut() {
                tap(&resampled);
            }
            let source = Source::from_interleaved(sample_rate, &resampled);
            let thread = std::thread::Builder::new()
                .name("loader".into())
                .spawn(|| Ok(()))?;
            Ok(Loading {
                info,
                source,
                thread,
                cancel,
            })
        }
    }
}

/// Streams decoded stereo into a [`SourceWriter`], resampling on the way if needed.
struct Sink {
    writer: Option<Tapped>,
    resampler: Option<StreamResampler>,
}

/// The source being loaded, plus whoever watches what goes into it.
struct Tapped {
    writer: SourceWriter,
    tap: Option<Tap>,
}

impl FrameOut for Tapped {
    fn put(&mut self, stereo: &[f32]) {
        self.writer.push(stereo);
        if let Some(tap) = self.tap.as_mut() {
            tap(stereo);
        }
    }
}

impl Sink {
    fn new(writer: SourceWriter, tap: Option<Tap>, from: u32, to: u32) -> Result<Self, LoadError> {
        let resampler = if from == to {
            None
        } else {
            Some(StreamResampler::new(from, to)?)
        };
        Ok(Self {
            writer: Some(Tapped { writer, tap }),
            resampler,
        })
    }

    fn finish(&mut self) -> Result<(), LoadError> {
        let Some(mut writer) = self.writer.take() else {
            return Ok(());
        };
        let result = match self.resampler.as_mut() {
            Some(r) => r.flush(&mut writer),
            None => Ok(()),
        };
        writer.writer.finish();
        result
    }
}

impl StereoSink for &mut Sink {
    fn push(&mut self, stereo: &[f32]) -> Result<(), LoadError> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        match self.resampler.as_mut() {
            Some(r) => r.push(stereo, writer),
            None => {
                writer.put(stereo);
                Ok(())
            }
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        // Mark the source complete even if decoding failed half-way, so the callback plays
        // what exists instead of reporting it as still loading.
        if let Some(w) = self.writer.take() {
            w.writer.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_to_vec;
    use crate::testutil::{sine, tmp, write_wav};

    #[test]
    fn same_rate_stereo_load_is_exact() {
        let path = tmp("same.wav");
        let frames = 100_000;
        let samples: Vec<f32> = (0..frames * 2)
            .map(|i| (i % 1000) as f32 / 1000.0)
            .collect();
        write_wav(&path, 48_000, 2, &samples);
        let loading = load_file(&path, 48_000).unwrap();
        loading.thread.join().unwrap().unwrap();
        let src = loading.source;
        assert!(src.is_complete());
        assert_eq!(src.len_frames(), frames);
        use engine_core::FrameSource;
        for i in [0usize, 1, 999, 50_000, frames - 1] {
            assert_eq!(src.frame(i as i64), [samples[2 * i], samples[2 * i + 1]]);
        }
    }

    #[test]
    fn the_tap_sees_exactly_what_the_source_gets() {
        use engine_core::FrameSource;
        use std::sync::{Arc, Mutex};
        let path = tmp("tap44.wav");
        let frames = 44_100 * 3 / 2;
        let samples: Vec<f32> = sine(44_100, 440.0, frames)
            .into_iter()
            .flat_map(|x| [x, -x])
            .collect();
        write_wav(&path, 44_100, 2, &samples);
        for rate in [44_100, 48_000] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&seen);
            let tap: Tap = Box::new(move |s: &[f32]| sink.lock().unwrap().extend_from_slice(s));
            let loading = load_file_tapped(&path, rate, Some(tap)).unwrap();
            loading.thread.join().unwrap().unwrap();
            let src = loading.source;
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2 * src.len_frames(), "{rate}");
            for i in [0, 1, 12_345, src.len_frames() - 1] {
                let f = src.frame(i as i64);
                assert_eq!([seen[2 * i], seen[2 * i + 1]], f, "{rate} frame {i}");
            }
        }
    }

    #[test]
    fn mono_is_duplicated_and_resampled_to_device_rate() {
        let path = tmp("mono44.wav");
        let frames = 44_100 * 2;
        write_wav(&path, 44_100, 1, &sine(44_100, 1000.0, frames));
        let loading = load_file(&path, 48_000).unwrap();
        assert_eq!(loading.info.file_sample_rate, 44_100);
        loading.thread.join().unwrap().unwrap();
        let src = loading.source;
        assert_eq!(src.len_frames(), 96_000);
        use engine_core::FrameSource;
        // Count positive-going zero crossings in the middle second: ~1000 Hz at the new rate.
        let mut crossings = 0;
        let mut prev = src.frame(24_000)[0];
        for i in 24_001..72_000 {
            let f = src.frame(i);
            assert_eq!(f[0], f[1]);
            if prev < 0.0 && f[0] >= 0.0 {
                crossings += 1;
            }
            prev = f[0];
        }
        assert!((999..=1001).contains(&crossings), "{crossings}");
        // The delay is trimmed: the sine starts at 0 and rises, as in the file.
        assert!(src.frame(0)[0].abs() < 0.02);
        assert!(src.frame(3)[0] > 0.0);
        // Matches the whole-clip resample.
        let whole = decode_to_vec(&path, 48_000).unwrap();
        assert_eq!(whole.len(), 96_000 * 2);
        let ideal =
            |j: usize| 0.5 * (std::f64::consts::TAU * 1000.0 * j as f64 / 48_000.0).sin() as f32;
        let max_err = (100..95_900)
            .map(|j| (src.frame(j as i64)[0] - ideal(j)).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-3, "streamed load vs ideal sine: {max_err}");
        let max_diff = (0..96_000)
            .map(|i| (whole[2 * i] - src.frame(i as i64)[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-3, "{max_diff}");
    }

    #[test]
    fn missing_file_is_an_io_error() {
        assert!(matches!(
            load_file("/definitely/not/here.wav", 48_000),
            Err(LoadError::Io(_))
        ));
    }
}
