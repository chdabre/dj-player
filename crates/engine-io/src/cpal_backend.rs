//! cpal implementation of [`AudioBackend`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig,
};

use crate::{AudioBackend, AudioStream, BackendError, RenderCallback, StreamFormat, StreamRequest};

/// Largest device buffer converted in one pass for non-f32 devices; longer buffers are split.
const CONVERT_FRAMES: usize = 4096;

/// cpal on the platform's default host (CoreAudio on macOS, ALSA on Linux).
pub struct CpalBackend {
    host: cpal::Host,
}

impl std::fmt::Debug for CpalBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalBackend")
            .field("host", &self.host.id())
            .finish()
    }
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalBackend {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// Name of the cpal host in use (e.g. "CoreAudio", "ALSA").
    pub fn host_name(&self) -> String {
        format!("{:?}", self.host.id())
    }

    /// "<id>: <name>" of every output device on the default host.
    pub fn output_device_names(&self) -> Result<Vec<String>, BackendError> {
        let devices = self.host.output_devices().map_err(err)?;
        Ok(devices.map(|d| device_name(&d)).collect())
    }

    fn device(&self, request: &StreamRequest) -> Result<Device, BackendError> {
        let host = &self.host;
        match &request.device {
            None => host
                .default_output_device()
                .ok_or_else(|| BackendError::new("no default output device")),
            Some(wanted) => {
                let wanted_lc = wanted.to_lowercase();
                host.output_devices()
                    .map_err(err)?
                    .find(|d| device_name(d).to_lowercase().contains(&wanted_lc))
                    .ok_or_else(|| {
                        BackendError::new(format!("no output device matches {wanted:?}"))
                    })
            }
        }
    }

    fn config(
        &self,
        request: &StreamRequest,
    ) -> Result<(Device, SupportedStreamConfig, StreamConfig), BackendError> {
        let device = self.device(request)?;
        let supported = device.default_output_config().map_err(err)?;
        let mut config = supported.config();
        if let Some(frames) = request.buffer_frames {
            let ok = match supported.buffer_size() {
                SupportedBufferSize::Range { min, max } => (*min..=*max).contains(&frames),
                SupportedBufferSize::Unknown => true,
            };
            if !ok {
                return Err(BackendError::new(format!(
                    "buffer size {frames} not supported by the device ({:?})",
                    supported.buffer_size()
                )));
            }
            config.buffer_size = BufferSize::Fixed(frames);
        }
        Ok((device, supported, config))
    }
}

fn err(e: impl std::fmt::Display) -> BackendError {
    BackendError::new(e.to_string())
}

/// "<id>: <description>", e.g. "alsa:hw:CARD=Device,DEV=0: USB Audio" or the CoreAudio
/// device UID and name. `--device` matches against this whole string.
fn device_name(d: &Device) -> String {
    let id = d.id().map(|id| id.to_string()).unwrap_or_default();
    let name = d
        .description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    if id.is_empty() {
        name
    } else {
        format!("{id}: {name}")
    }
}

impl AudioBackend for CpalBackend {
    type Stream = CpalStream;

    fn format(&self, request: &StreamRequest) -> Result<StreamFormat, BackendError> {
        let (_, _, config) = self.config(request)?;
        Ok(StreamFormat {
            sample_rate: config.sample_rate,
            channels: usize::from(config.channels),
        })
    }

    fn start(
        &self,
        request: &StreamRequest,
        callback: Box<dyn RenderCallback>,
    ) -> Result<CpalStream, BackendError> {
        let (device, supported, config) = self.config(request)?;
        let format = StreamFormat {
            sample_rate: config.sample_rate,
            channels: usize::from(config.channels),
        };
        let (err_tx, err_rx) = sync_channel(64);
        let dropped = Arc::new(AtomicU64::new(0));
        let errors = ErrorSink {
            tx: err_tx,
            dropped: Arc::clone(&dropped),
        };
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_f32(&device, &config, callback, errors),
            SampleFormat::I16 => build_converted::<i16>(&device, &config, callback, errors),
            SampleFormat::I32 => build_converted::<i32>(&device, &config, callback, errors),
            SampleFormat::U16 => build_converted::<u16>(&device, &config, callback, errors),
            SampleFormat::F64 => build_converted::<f64>(&device, &config, callback, errors),
            other => Err(BackendError::new(format!(
                "unsupported device sample format {other}"
            ))),
        }?;
        stream.play().map_err(err)?;
        let buffer_frames = match config.buffer_size {
            BufferSize::Fixed(n) => Some(n),
            BufferSize::Default => None,
        };
        Ok(CpalStream {
            _stream: stream,
            format,
            buffer_frames,
            errors: err_rx,
            dropped,
            device: device_name(&device),
        })
    }
}

/// Forwards stream errors out of cpal's error callback without blocking.
struct ErrorSink {
    tx: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl ErrorSink {
    fn report(&self, e: cpal::Error) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(e.to_string()) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn build_f32(
    device: &Device,
    config: &StreamConfig,
    mut callback: Box<dyn RenderCallback>,
    errors: ErrorSink,
) -> Result<cpal::Stream, BackendError> {
    let channels = usize::from(config.channels);
    device
        .build_output_stream::<f32, _, _>(
            *config,
            move |data: &mut [f32], _| {
                let frames = data.len() / channels;
                callback.render(data, frames);
            },
            move |e| errors.report(e),
            None,
        )
        .map_err(err)
}

fn build_converted<T>(
    device: &Device,
    config: &StreamConfig,
    mut callback: Box<dyn RenderCallback>,
    errors: ErrorSink,
) -> Result<cpal::Stream, BackendError>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(config.channels);
    // Allocated here, before the stream starts, and moved into the callback.
    let mut scratch = vec![0.0f32; CONVERT_FRAMES * channels];
    device
        .build_output_stream::<T, _, _>(
            *config,
            move |data: &mut [T], _| {
                for piece in data.chunks_mut(CONVERT_FRAMES * channels) {
                    let frames = piece.len() / channels;
                    let buf = &mut scratch[..frames * channels];
                    callback.render(buf, frames);
                    for (d, &s) in piece.iter_mut().zip(buf.iter()) {
                        *d = T::from_sample(s);
                    }
                }
            },
            move |e| errors.report(e),
            None,
        )
        .map_err(err)
}

/// A running cpal output stream.
pub struct CpalStream {
    _stream: cpal::Stream,
    format: StreamFormat,
    buffer_frames: Option<u32>,
    errors: Receiver<String>,
    dropped: Arc<AtomicU64>,
    device: String,
}

impl std::fmt::Debug for CpalStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalStream")
            .field("device", &self.device)
            .field("format", &self.format)
            .field("buffer_frames", &self.buffer_frames)
            .finish()
    }
}

impl CpalStream {
    pub fn device_name(&self) -> &str {
        &self.device
    }
}

impl AudioStream for CpalStream {
    fn format(&self) -> StreamFormat {
        self.format
    }

    fn buffer_frames(&self) -> Option<u32> {
        self.buffer_frames
    }

    fn take_errors(&self) -> Vec<String> {
        let mut out: Vec<String> = self.errors.try_iter().collect();
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            out.push(format!("{dropped} more stream errors not shown"));
        }
        out
    }
}
