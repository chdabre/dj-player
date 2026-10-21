//! [stratum-dsp](https://docs.rs/stratum-dsp) backend: tempo, beatgrid and key in one pass.
//! Pure Rust, MIT OR Apache-2.0, single-threaded.

use crate::analyzer::{AnalysisError, Analyzer, CancelFlag};
use crate::model::{
    Analysed, Analysis, AnalyzerId, Beat, Beatgrid, Feature, FeatureSet, Key, Provenance, Tempo,
    TrackAudio,
};

/// The stratum-dsp version this backend is built against (part of the cache key).
pub const STRATUM_VERSION: &str = "1.0.0";

/// stratum-dsp's frame and hop sizes are in samples and tuned for 44.1 kHz: at 48 and 96 kHz
/// its tempo and grid get much worse. Audio at any other rate is resampled to this first.
pub const ANALYSIS_RATE: u32 = 44_100;

/// stratum-dsp with its default configuration, except that it does not trim silence.
///
/// stratum-dsp trims leading silence before analysing but reports beat times relative to the
/// trimmed audio, so every beat of a track with a silent intro would be early by the length
/// of that silence. With trimming off the times refer to the whole decoded track.
pub struct Stratum {
    config: stratum_dsp::AnalysisConfig,
}

impl Default for Stratum {
    fn default() -> Self {
        Self::new()
    }
}

impl Stratum {
    pub fn new() -> Self {
        Self::with_config(stratum_dsp::AnalysisConfig {
            enable_silence_trimming: false,
            ..stratum_dsp::AnalysisConfig::default()
        })
    }

    /// Any stratum-dsp configuration (experiments, tuning). Note that silence trimming makes
    /// beat times relative to the trimmed audio; see the type docs.
    pub fn with_config(config: stratum_dsp::AnalysisConfig) -> Self {
        Self { config }
    }
}

/// FNV-1a over the configuration's `Debug` text: stable across builds and Rust versions, and
/// different for any change of any parameter.
fn fingerprint(config: &stratum_dsp::AnalysisConfig) -> String {
    let text = format!("{config:?}");
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{ANALYSIS_RATE}Hz,cfg={hash:016x}")
}

impl Analyzer for Stratum {
    fn id(&self) -> AnalyzerId {
        AnalyzerId {
            name: "stratum-dsp".into(),
            version: STRATUM_VERSION.into(),
            params: fingerprint(&self.config),
        }
    }

    fn features(&self) -> FeatureSet {
        FeatureSet::ALL
    }

    fn analyze(
        &mut self,
        audio: &TrackAudio,
        want: FeatureSet,
        _context: &[Analysis],
        cancel: &CancelFlag,
    ) -> Result<Analysis, AnalysisError> {
        cancel.check()?;
        if audio.len_frames() == 0 {
            return Err(AnalysisError::Input("no audio".into()));
        }
        let audio = audio
            .at_rate(ANALYSIS_RATE)
            .map_err(AnalysisError::Backend)?;
        let mono = audio.mono();
        let r = stratum_dsp::analyze_audio(&mono, audio.sample_rate, self.config.clone()).map_err(
            |e| match e {
                stratum_dsp::AnalysisError::InvalidInput(m) => AnalysisError::Input(m),
                other => AnalysisError::Backend(other.to_string()),
            },
        )?;
        cancel.check()?;

        let provenance = Provenance::Analyzer(self.id());
        let mut out = Analysis::default();
        if want.contains(Feature::Tempo) && r.bpm > 0.0 {
            out.tempo = Some(Analysed {
                value: Tempo {
                    bpm: f64::from(r.bpm),
                },
                confidence: Some(r.bpm_confidence),
                provenance: provenance.clone(),
            });
        }
        if want.contains(Feature::Beatgrid) && !r.beat_grid.beats.is_empty() {
            out.beatgrid = Some(Analysed {
                value: grid(&r.beat_grid.beats, &r.beat_grid.downbeats),
                confidence: Some(r.grid_stability),
                provenance: provenance.clone(),
            });
        }
        if want.contains(Feature::Key) && r.key_confidence > 0.0 {
            let key = match r.key {
                stratum_dsp::Key::Major(i) => Key::major(i as i32),
                stratum_dsp::Key::Minor(i) => Key::minor(i as i32),
            };
            out.key = Some(Analysed {
                value: key,
                confidence: Some(r.key_confidence),
                provenance,
            });
        }
        Ok(out)
    }
}

/// Beats with bar positions: 1 on each reported downbeat, counting up from there (4/4).
/// Beats before the first downbeat count back from it; with no downbeats the first beat is 1.
fn grid(beats: &[f32], downbeats: &[f32]) -> Beatgrid {
    const SAME_BEAT: f64 = 0.010;
    let is_downbeat = |t: f64| {
        downbeats
            .iter()
            .any(|&d| (f64::from(d) - t).abs() < SAME_BEAT)
    };
    let times: Vec<f64> = beats.iter().map(|&b| f64::from(b)).collect();
    let first_down = times.iter().position(|&t| is_downbeat(t)).unwrap_or(0);
    let mut pos = (4 - (first_down % 4)) % 4; // so that `first_down` lands on 0
    let beats = times
        .iter()
        .enumerate()
        .map(|(i, &time)| {
            if i >= first_down && is_downbeat(time) {
                pos = 0;
            }
            let beat = Beat {
                time,
                bar_pos: (pos % 4) as u8 + 1,
            };
            pos += 1;
            beat
        })
        .collect();
    Beatgrid { beats }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_positions_follow_the_downbeats() {
        let beats = [0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];
        let g = grid(&beats, &[1.0, 3.0]);
        let pos: Vec<u8> = g.beats.iter().map(|b| b.bar_pos).collect();
        // 1.0 and 3.0 are downbeats; the two beats before 1.0 are 3 and 4 of a bar.
        assert_eq!(pos, [3, 4, 1, 2, 3, 4, 1, 2, 3]);
        let g = grid(&beats, &[]);
        assert_eq!(g.beats[0].bar_pos, 1);
        assert_eq!(g.beats[4].bar_pos, 1);
    }
}
