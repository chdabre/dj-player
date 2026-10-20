//! Track loading for the DJ player engine.
//!
//! [`load_file`] probes a file with Symphonia, allocates a resident [`Source`] at the device
//! sample rate, and starts a loader thread that decodes, maps to stereo, resamples once with
//! rubato (only if the file rate differs from the device rate) and fills the source chunk by
//! chunk. Playback can start as soon as the first chunk is ready; the callback never waits.

use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use engine_core::{CHANNELS, Source, SourceWriter};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Frames per resampler call.
const RESAMPLE_CHUNK: usize = 1024;
/// Extra capacity on top of the estimated length, for rounding in the resampler and for
/// containers whose frame count is slightly off.
const CAPACITY_MARGIN_FRAMES: usize = 4096;

/// Why a track could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Decode(SymphoniaError),
    NoAudioTrack,
    UnknownSampleRate,
    Resample(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "cannot open file: {e}"),
            LoadError::Decode(e) => write!(f, "cannot decode file: {e}"),
            LoadError::NoAudioTrack => f.write_str("file has no decodable audio track"),
            LoadError::UnknownSampleRate => f.write_str("file does not state its sample rate"),
            LoadError::Resample(e) => write!(f, "resampler error: {e}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io(e) => Some(e),
            LoadError::Decode(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        LoadError::Io(e)
    }
}

impl From<SymphoniaError> for LoadError {
    fn from(e: SymphoniaError) -> Self {
        LoadError::Decode(e)
    }
}

/// Facts about a file, known after probing.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub file_sample_rate: u32,
    pub file_channels: usize,
    /// Frames in the file at its own rate, if the container says.
    pub file_frames: Option<u64>,
    /// Engine (device) sample rate the source is stored at.
    pub sample_rate: u32,
}

/// A track being loaded: the source is usable right away; `thread` finishes the decode.
#[derive(Debug)]
pub struct Loading {
    pub info: TrackInfo,
    pub source: Arc<Source>,
    pub thread: JoinHandle<Result<(), LoadError>>,
}

/// Probe `path` and start decoding it into a new [`Source`] at `sample_rate` on a loader
/// thread.
///
/// If the container does not state the track length, the whole file is decoded on the calling
/// thread first (the source is then complete when this returns).
pub fn load_file(path: impl AsRef<Path>, sample_rate: u32) -> Result<Loading, LoadError> {
    let path = path.as_ref().to_path_buf();
    let mut decoding = Decoding::open(&path)?;
    let info = TrackInfo {
        path: path.clone(),
        file_sample_rate: decoding.sample_rate,
        file_channels: decoding.channels,
        file_frames: decoding.frames,
        sample_rate,
    };
    let ratio = sample_rate as f64 / decoding.sample_rate as f64;

    match decoding.frames {
        Some(frames) => {
            let capacity = (frames as f64 * ratio).ceil() as usize + CAPACITY_MARGIN_FRAMES;
            let (source, writer) = Source::new(sample_rate, capacity);
            let thread = std::thread::Builder::new()
                .name("loader".into())
                .spawn(move || {
                    let mut sink = Sink::new(writer, decoding.sample_rate, sample_rate)?;
                    let result = decoding.run(&mut sink);
                    sink.finish()?;
                    result
                })?;
            Ok(Loading {
                info,
                source,
                thread,
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
            let source = Source::from_interleaved(sample_rate, &resampled);
            let thread = std::thread::Builder::new()
                .name("loader".into())
                .spawn(|| Ok(()))?;
            Ok(Loading {
                info,
                source,
                thread,
            })
        }
    }
}

/// Decode `path` completely and return interleaved stereo at `sample_rate` (tests, tools).
pub fn decode_to_vec(path: impl AsRef<Path>, sample_rate: u32) -> Result<Vec<f32>, LoadError> {
    let mut decoding = Decoding::open(path.as_ref())?;
    let mut collected = Vec::new();
    decoding.run(&mut |s: &[f32]| collected.extend_from_slice(s))?;
    if decoding.sample_rate == sample_rate {
        Ok(collected)
    } else {
        resample_all(&collected, decoding.sample_rate, sample_rate)
    }
}

fn resample_all(stereo: &[f32], from: u32, to: u32) -> Result<Vec<f32>, LoadError> {
    let mut out = Vec::with_capacity(
        (stereo.len() as u64 * u64::from(to)).div_ceil(u64::from(from)) as usize,
    );
    let mut resampler = StreamResampler::new(from, to)?;
    resampler.push(stereo, &mut out)?;
    resampler.flush(&mut out)?;
    Ok(out)
}

/// Where resampled stereo goes: the source being loaded, or a plain vector.
trait FrameOut {
    fn put(&mut self, stereo: &[f32]);
}

impl FrameOut for SourceWriter {
    fn put(&mut self, stereo: &[f32]) {
        self.push(stereo);
    }
}

impl FrameOut for Vec<f32> {
    fn put(&mut self, stereo: &[f32]) {
        self.extend_from_slice(stereo);
    }
}

/// Receives decoded interleaved stereo at the file rate.
trait StereoSink {
    fn push(&mut self, stereo: &[f32]) -> Result<(), LoadError>;
}

impl<F: FnMut(&[f32])> StereoSink for &mut F {
    fn push(&mut self, stereo: &[f32]) -> Result<(), LoadError> {
        self(stereo);
        Ok(())
    }
}

/// An opened file with its decoder.
struct Decoding {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    frames: Option<u64>,
}

impl Decoding {
    fn open(path: &Path) -> Result<Self, LoadError> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(LoadError::NoAudioTrack)?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(LoadError::NoAudioTrack)?;
        let sample_rate = params.sample_rate.ok_or(LoadError::UnknownSampleRate)?;
        let channels = params.channels.as_ref().map_or(0, |c| c.count());
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default().gapless(true))?;
        let track_id = track.id;
        let frames = track.num_frames;
        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            frames,
        })
    }

    /// Decode every packet and push stereo frames into `sink`.
    fn run(&mut self, mut sink: impl StereoSink) -> Result<(), LoadError> {
        let mut interleaved: Vec<f32> = Vec::new();
        let mut stereo: Vec<f32> = Vec::new();
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(SymphoniaError::ResetRequired) => break,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                // Skip corrupt packets, like every player does.
                Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            let channels = buf.spec().channels().count().max(1);
            interleaved.resize(buf.samples_interleaved(), 0.0);
            buf.copy_to_slice_interleaved(&mut interleaved);
            to_stereo(&interleaved, channels, &mut stereo);
            sink.push(&stereo)?;
        }
        Ok(())
    }
}

/// Map `channels`-channel interleaved audio to stereo: mono is duplicated, more than two
/// channels keep the first two (front left/right).
fn to_stereo(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    match channels {
        1 => out.extend(interleaved.iter().flat_map(|&s| [s, s])),
        2 => out.extend_from_slice(interleaved),
        n => out.extend(interleaved.chunks_exact(n).flat_map(|f| [f[0], f[1]])),
    }
}

/// Streams decoded stereo into a [`SourceWriter`], resampling on the way if needed.
struct Sink {
    writer: Option<SourceWriter>,
    resampler: Option<StreamResampler>,
}

impl Sink {
    fn new(writer: SourceWriter, from: u32, to: u32) -> Result<Self, LoadError> {
        let resampler = if from == to {
            None
        } else {
            Some(StreamResampler::new(from, to)?)
        };
        Ok(Self {
            writer: Some(writer),
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
        writer.finish();
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
                writer.push(stereo);
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
            w.finish();
        }
    }
}

/// Chunked load-time resampler with the startup delay trimmed, matching what rubato's
/// `process_all` does for a whole clip, but streaming so the source fills progressively.
struct StreamResampler {
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
    fn new(from: u32, to: u32) -> Result<Self, LoadError> {
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

    fn push(&mut self, stereo: &[f32], writer: &mut impl FrameOut) -> Result<(), LoadError> {
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

    fn flush(&mut self, writer: &mut impl FrameOut) -> Result<(), LoadError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("engine-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn sine(rate: u32, hz: f32, frames: usize) -> Vec<f32> {
        let w = std::f32::consts::TAU * hz / rate as f32;
        (0..frames).map(|i| 0.5 * (w * i as f32).sin()).collect()
    }

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

    #[test]
    fn to_stereo_maps_channels() {
        let mut out = Vec::new();
        to_stereo(&[1.0, 2.0], 1, &mut out);
        assert_eq!(out, [1.0, 1.0, 2.0, 2.0]);
        to_stereo(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 3, &mut out);
        assert_eq!(out, [1.0, 2.0, 4.0, 5.0]);
    }
}
