//! What the UI holds about the loaded track: its title and tags, the waveform as far as it
//! has decoded, the gain the waveform is drawn with, the overview's cache and the smoothed
//! playhead. Loading a track replaces the whole [`Track`], so nothing carries over from the
//! last one.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use waveform::compute::Entry;
use waveform::display::TrackGain;

use crate::clock::PlayheadClock;
use crate::waves::OverviewCache;

/// Entries the waveform grows by before its auto gain is worked out again while decoding.
const GAIN_STEP: usize = 1500;

pub struct Track {
    pub title: String,
    /// The file's own tags and large artwork, for the INFO screen; `None` with no track.
    pub tags: Option<LoadedTags>,
    /// The waveform so far, and whether it is complete.
    pub wave: Vec<Entry>,
    pub wave_complete: bool,
    /// The waveform's auto gain, and the entry count it was worked out from.
    gain: (f32, usize),
    pub overview: OverviewCache,
    pub clock: PlayheadClock,
    /// When the last frame was drawn and the playhead it showed.
    pub last_shown: Option<(Instant, f64)>,
}

impl Track {
    /// No track loaded.
    pub fn none(sample_rate: u32) -> Self {
        Self {
            title: String::new(),
            tags: None,
            wave: Vec::new(),
            wave_complete: false,
            gain: (1.0, 0),
            overview: OverviewCache::default(),
            clock: PlayheadClock::new(sample_rate),
            last_shown: None,
        }
    }

    /// `path` just loaded; `listed` are its tags as the browser has them, if it does.
    pub fn new(path: &Path, listed: Option<&engined::browse::Tags>, sample_rate: u32) -> Self {
        Self {
            title: track_title(path, listed),
            tags: Some(LoadedTags::read(path)),
            ..Self::none(sample_rate)
        }
    }

    /// Take the waveform entries decoded since the last frame from `live`.
    pub fn update_wave(&mut self, live: &engined::LiveWaveform) {
        if !self.wave_complete {
            self.wave_complete = live.update(&mut self.wave);
        }
        // Auto gain over what has decoded so far; it settles as the track fills in.
        let n = self.wave.len();
        if n != self.gain.1 && (self.wave_complete || n >= self.gain.1 + GAIN_STEP) {
            self.gain = (TrackGain::Auto.resolve(&self.wave), n);
        }
    }

    /// The gain the waveform is drawn with.
    pub fn gain(&self) -> f32 {
        self.gain.0
    }
}

/// The loaded track's tags and artwork, read once when it loads (the INFO screen).
pub struct LoadedTags {
    pub tags: engined::browse::Tags,
    pub art: Option<Arc<engined::browse::Thumb>>,
}

impl LoadedTags {
    fn read(path: &Path) -> Self {
        let mut tags = engined::browse::read_tags(path).unwrap_or_default();
        let art = tags
            .artwork
            .take()
            .and_then(|a| engined::browse::thumbnail(&a.data, engined::browse::LARGE_SIZE))
            .map(Arc::new);
        Self { tags, art }
    }
}

/// A track's title: its tag, else the file name without extension.
pub fn track_title(path: &Path, tags: Option<&engined::browse::Tags>) -> String {
    tags.and_then(|t| t.title.clone()).unwrap_or_else(|| {
        path.file_stem()
            .map_or("[TRACK]".into(), |s| s.to_string_lossy().into_owned())
    })
}
