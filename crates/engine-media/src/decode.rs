use std::path::{Path, PathBuf};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

use crate::id3::AfterId3;
use crate::resample::resample_all;
use crate::{Cancel, LoadError};

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
    /// Frames (at the file's rate) the decoder drops from the start for gapless playback:
    /// for MP3 the LAME header's encoder delay plus the decoder's 529 frames; 0 if the file
    /// states none. Software that decodes without gapless trimming has its sample 0 this
    /// many frames earlier, so times taken from it (e.g. Rekordbox grids and cues) are this
    /// much later than ours.
    pub start_trim: u32,
}

/// Probe `path` without decoding it. `sample_rate` in the result is the file's own rate.
pub fn probe(path: impl AsRef<Path>) -> Result<TrackInfo, LoadError> {
    let path = path.as_ref();
    let d = Decoding::open(path)?;
    Ok(TrackInfo {
        path: path.to_path_buf(),
        file_sample_rate: d.sample_rate,
        file_channels: d.channels,
        file_frames: d.frames,
        sample_rate: d.sample_rate,
        start_trim: d.start_trim,
    })
}

/// Decode `path` completely at the file's own sample rate. Returns interleaved stereo and that
/// rate. Analysis uses this, so its results do not depend on the device the engine runs on.
pub fn decode_native(path: impl AsRef<Path>) -> Result<(Vec<f32>, u32), LoadError> {
    decode_native_until(path, &Cancel::new())
}

/// [`decode_native`], stopped with [`LoadError::Cancelled`] once `cancel` is set.
pub fn decode_native_until(
    path: impl AsRef<Path>,
    cancel: &Cancel,
) -> Result<(Vec<f32>, u32), LoadError> {
    let mut decoding = Decoding::open(path.as_ref())?;
    decoding.cancel = Some(cancel.clone());
    let mut collected = Vec::new();
    decoding.run(&mut |s: &[f32]| collected.extend_from_slice(s))?;
    Ok((collected, decoding.sample_rate))
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

/// Probe `source` for its container, with `path`'s extension as the hint. Decoding and tag
/// reading both open files through this, so they see a file the same way.
pub(crate) fn probe_format(
    path: &Path,
    source: Box<dyn MediaSource>,
) -> Result<Box<dyn FormatReader>, SymphoniaError> {
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )
}

/// Receives decoded interleaved stereo at the file rate.
pub(crate) trait StereoSink {
    fn push(&mut self, stereo: &[f32]) -> Result<(), LoadError>;
}

impl<F: FnMut(&[f32])> StereoSink for &mut F {
    fn push(&mut self, stereo: &[f32]) -> Result<(), LoadError> {
        self(stereo);
        Ok(())
    }
}

/// An opened file with its decoder.
pub(crate) struct Decoding {
    pub(crate) format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub(crate) sample_rate: u32,
    pub(crate) channels: usize,
    pub(crate) frames: Option<u64>,
    pub(crate) start_trim: u32,
    pub(crate) cancel: Option<Cancel>,
}

impl Decoding {
    /// Open `path` with any leading ID3v2 tags skipped (see [`AfterId3`]).
    pub(crate) fn open(path: &Path) -> Result<Self, LoadError> {
        Self::from_after_id3(path, AfterId3::open(path)?)
    }

    /// [`Decoding::open`] on a file already opened past its tags.
    pub(crate) fn from_after_id3(path: &Path, file: AfterId3) -> Result<Self, LoadError> {
        let format = probe_format(path, Box::new(file))?;
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
        let start_trim = track.delay.unwrap_or(0);
        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            frames,
            start_trim,
            cancel: None,
        })
    }

    /// Decode every packet and push stereo frames into `sink`.
    pub(crate) fn run(&mut self, mut sink: impl StereoSink) -> Result<(), LoadError> {
        let mut interleaved: Vec<f32> = Vec::new();
        let mut stereo: Vec<f32> = Vec::new();
        let mut decoded = 0usize;
        loop {
            if self.cancel.as_ref().is_some_and(Cancel::is_cancelled) {
                return Err(LoadError::Cancelled);
            }
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
            decoded += stereo.len();
            sink.push(&stereo)?;
        }
        // Skipping corrupt packets must not turn a file we cannot read into silence.
        if decoded == 0 {
            return Err(LoadError::NoAudio);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{id3_tag, tmp};

    #[test]
    fn a_file_that_decodes_to_nothing_is_an_error() {
        // A tag and then nothing decodable.
        let path = tmp("empty.mp3");
        let mut bytes = id3_tag(1000);
        bytes.extend(std::iter::repeat_n(0u8, 4000));
        std::fs::write(&path, bytes).unwrap();
        assert!(decode_native(&path).is_err());
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
