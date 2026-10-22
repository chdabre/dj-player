//! The loaded track's analysis: tempo, grid and key on a background thread, reusing stored
//! results whose analyzer version and parameters still match.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use engine_analysis::{Analysis, Analyzer, CancelFlag, FeatureSet};
use engine_core::DeckGrid;
use library::{AnalysisRef, AudioHash};

use crate::store::FreshAnalysis;

/// What the analysis thread hands back: the deck grid (engine frames), the key, a line to
/// print, and what to store in the library.
pub struct Analysed {
    pub grid: Option<Arc<DeckGrid>>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub summary: String,
    /// Results computed now (not from the library), to be stored.
    pub fresh: Vec<FreshAnalysis>,
    /// The audio hash, if it was asked for.
    pub audio_hash: Option<AudioHash>,
}

/// Analyse `path` on its own thread: tempo (TempoCNN, if `TEMPOCNN_MODEL` is set) refined by
/// gridfit, the grid, and the key, reusing `cached` results whose analyzer version and
/// parameters still match. The result arrives on the returned channel.
pub(crate) fn analyze_in_background(
    path: PathBuf,
    cancel: engine_media::Cancel,
    engine_rate: u32,
    settings: engine_analysis::Settings,
    cached: BTreeMap<String, (AnalysisRef, Vec<u8>)>,
    audio_hash: bool,
) -> Receiver<Result<Analysed, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || {
            let _ = tx.send(analyze(
                &path,
                &cancel,
                engine_rate,
                settings,
                &cached,
                audio_hash,
            ));
        });
    if let Err(e) = spawned {
        // The sender is gone with the closure: the receiver just never yields.
        eprintln!("analysis thread: {e}");
    }
    rx
}

fn analyze(
    path: &Path,
    stop: &engine_media::Cancel,
    engine_rate: u32,
    settings: engine_analysis::Settings,
    cached: &BTreeMap<String, (AnalysisRef, Vec<u8>)>,
    want_audio_hash: bool,
) -> Result<Analysed, String> {
    analyze_with(
        path,
        &mut None,
        stop,
        engine_rate,
        settings,
        cached,
        want_audio_hash,
    )
}

/// [`analyze`], decoding into `audio` only if something has to be computed (or using what
/// is there): the batch job passes it in and computes the waveform from it afterwards.
pub(crate) fn analyze_with(
    path: &Path,
    audio: &mut Option<engine_analysis::TrackAudio>,
    stop: &engine_media::Cancel,
    engine_rate: u32,
    settings: engine_analysis::Settings,
    cached: &BTreeMap<String, (AnalysisRef, Vec<u8>)>,
    want_audio_hash: bool,
) -> Result<Analysed, String> {
    let started = Instant::now();
    let (mut analyzers, hint) = engine_analysis::player_chain(settings)?;
    let mut source = Decode { path, stop, audio };
    let (context, fresh) = run_chain(&mut analyzers, &mut source, cached)?;
    let audio_hash = if want_audio_hash {
        Some(AudioHash::of_pcm(&source.get()?.stereo))
    } else {
        None
    };
    let merged = engine_analysis::merge(context);
    let bpm = merged.tempo.as_ref().map(|t| t.value.bpm);
    let grid = match (&merged.beatgrid, bpm) {
        (Some(g), Some(bpm)) => DeckGrid::from_seconds(
            g.value.beats.iter().map(|b| (b.time, b.bar_pos)),
            bpm,
            engine_rate,
        )
        .map(Arc::new),
        _ => None,
    };
    let first_down = merged
        .beatgrid
        .as_ref()
        .and_then(|g| g.value.beats.iter().find(|b| b.bar_pos == 1))
        .map(|b| b.time);
    let key = merged.key.map(|k| k.value.to_string());
    let summary = format!(
        "{} in {:.1} s: {}, key {}, first downbeat {}{}",
        if fresh.is_empty() {
            "analysis from the library"
        } else {
            "analysed"
        },
        started.elapsed().as_secs_f64(),
        bpm.map_or("no tempo".into(), |b| format!("{b:.2} BPM")),
        key.as_deref().unwrap_or("?"),
        first_down.map_or("?".into(), |t| format!("{t:.3} s")),
        if hint.is_none() {
            " (TEMPOCNN_MODEL not set: gridfit alone)"
        } else {
            ""
        },
    );
    Ok(Analysed {
        grid,
        bpm,
        key,
        summary,
        fresh,
        audio_hash,
    })
}

/// The track's audio, decoded on first use only.
struct Decode<'a> {
    path: &'a Path,
    stop: &'a engine_media::Cancel,
    audio: &'a mut Option<engine_analysis::TrackAudio>,
}

impl Decode<'_> {
    fn get(&mut self) -> Result<&engine_analysis::TrackAudio, String> {
        if self.audio.is_none() {
            *self.audio =
                Some(engine_analysis::load_until(self.path, self.stop).map_err(|e| e.to_string())?);
        }
        Ok(self.audio.as_ref().expect("just loaded"))
    }
}

/// Every analyzer's result in order, each from `cached` where it still applies, else
/// computed (decoding the audio the first time one is); and the computed ones, to store.
fn run_chain(
    analyzers: &mut [Box<dyn Analyzer>],
    audio: &mut Decode,
    cached: &BTreeMap<String, (AnalysisRef, Vec<u8>)>,
) -> Result<(Vec<Analysis>, Vec<FreshAnalysis>), String> {
    let cancel = CancelFlag::from_shared(audio.stop.flag());
    let mut context: Vec<Analysis> = Vec::new();
    let mut fresh = Vec::new();
    for a in analyzers {
        let want = FeatureSet::ALL.intersect(a.features());
        if want.is_empty() {
            continue;
        }
        let id = a.id();
        // A result is reused only while everything before it was reused too: analyzers
        // refine their predecessors' results.
        let reuse = cached
            .get(&id.name)
            .filter(|(r, _)| fresh.is_empty() && r.version == id.version && r.params == id.params)
            .and_then(|(_, blob)| engine_analysis::stored::decode(blob, &id));
        let result = match reuse {
            Some(r) => r,
            None => {
                let r = a
                    .analyze(audio.get()?, want, &context, &cancel)
                    .map_err(|e| e.to_string())?;
                fresh.push(FreshAnalysis {
                    name: id.name.clone(),
                    version: id.version.clone(),
                    params: id.params.clone(),
                    blob: engine_analysis::stored::encode(&r),
                });
                r
            }
        };
        context.push(result);
    }
    Ok((context, fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 20 s of clicks at 120 BPM, stereo, 16-bit.
    fn clicks(path: &Path) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..44_100 * 20 {
            let since = i % 22_050;
            let x = if since < 400 {
                (since as f32 * 0.7).sin() * (1.0 - since as f32 / 400.0)
            } else {
                0.0
            };
            let s = (x * 20_000.0) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn stored_analysis_is_reused_without_decoding() {
        let dir = std::env::temp_dir().join(format!("engined-analysis-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clicks.wav");
        clicks(&path);
        let settings = engine_analysis::Settings::default();
        let first = analyze(
            &path,
            &engine_media::Cancel::new(),
            48_000,
            settings,
            &BTreeMap::new(),
            true,
        )
        .unwrap();
        assert!(!first.fresh.is_empty());
        assert!(first.audio_hash.is_some());
        let bpm = first.bpm.unwrap();
        assert!((bpm - 120.0).abs() < 0.5, "{bpm}");

        let cached: BTreeMap<String, (AnalysisRef, Vec<u8>)> = first
            .fresh
            .iter()
            .map(|f| {
                let r = AnalysisRef {
                    version: f.version.clone(),
                    params: f.params.clone(),
                    blob: library::BlobId::of(&f.blob),
                    created: String::new(),
                };
                (f.name.clone(), (r, f.blob.clone()))
            })
            .collect();
        // With every result stored and no audio hash wanted, the file is not even read.
        std::fs::remove_file(&path).unwrap();
        let again = analyze(
            &path,
            &engine_media::Cancel::new(),
            48_000,
            settings,
            &cached,
            false,
        )
        .unwrap();
        assert!(again.fresh.is_empty(), "every result from the library");
        assert_eq!(again.bpm, first.bpm);
        assert_eq!(again.key, first.key);
        assert_eq!(
            again.grid.as_ref().map(|g| g.bpm()),
            first.grid.as_ref().map(|g| g.bpm())
        );

        // A result from another analyzer version is recomputed (and the file is needed).
        let mut stale = cached;
        for (r, _) in stale.values_mut() {
            r.version = "0.0.0-old".into();
        }
        assert!(
            analyze(
                &path,
                &engine_media::Cancel::new(),
                48_000,
                settings,
                &stale,
                false
            )
            .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
