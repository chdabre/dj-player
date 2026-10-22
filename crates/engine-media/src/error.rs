use std::fmt;

use symphonia::core::errors::Error as SymphoniaError;

/// Why a track could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Decode(SymphoniaError),
    NoAudioTrack,
    UnknownSampleRate,
    Resample(String),
    /// The file opened and its stream looked valid, but not one frame decoded.
    NoAudio,
    /// Stopped by its [`Cancel`](crate::Cancel) (the track was replaced before it finished).
    Cancelled,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "cannot open file: {e}"),
            LoadError::Decode(e) => write!(f, "cannot decode file: {e}"),
            LoadError::NoAudioTrack => f.write_str("file has no decodable audio track"),
            LoadError::UnknownSampleRate => f.write_str("file does not state its sample rate"),
            LoadError::Resample(e) => write!(f, "resampler error: {e}"),
            LoadError::NoAudio => f.write_str("no audio could be decoded from the file"),
            LoadError::Cancelled => f.write_str("cancelled"),
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
