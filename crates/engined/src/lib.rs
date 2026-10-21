//! One deck playing one file through the real engine: device, decode, analysis and stream,
//! shared by the `engined` terminal CLI and the UI spike.
//!
//! [`Player::open`] picks the device format, starts decoding the file at that rate, starts
//! the stream and analyses the file on a background thread. The owner calls [`Player::poll`]
//! regularly: it hands the analysed grid to the deck, collects retired objects off the audio
//! thread and reports loader and stream errors.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Instant;

use engine_core::{
    DeckCommand, DeckGrid, DeckState, Engine, EngineConfig, EngineHandle, LoadMeter, Source,
};
use engine_io::{AudioBackend, AudioStream, CpalBackend, CpalStream, StreamFormat, StreamRequest};

/// The deck everything here drives.
pub const DECK: usize = 0;

pub struct Player {
    pub format: StreamFormat,
    pub info: engine_media::TrackInfo,
    pub source: Arc<Source>,
    handle: EngineHandle,
    /// `None` only while dropping: the stream goes first, so the engine and its sources are
    /// dropped off the audio thread before the retire ring is drained.
    stream: Option<CpalStream>,
    loader: Option<JoinHandle<Result<(), engine_media::LoadError>>>,
    analysis: Receiver<Result<Analysed, String>>,
    /// The analysis result, once it arrived.
    pub analysed: Option<Analysed>,
}

impl Player {
    /// Open the output, start loading `file` and start playing paused.
    pub fn open(
        backend: &CpalBackend,
        request: &StreamRequest,
        file: &Path,
        settings: engine_analysis::Settings,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // The device decides the engine rate; tracks are resampled to it at load.
        let format = backend.format(request)?;
        let loading = engine_media::load_file(file, format.sample_rate)?;
        let source = Arc::clone(&loading.source);
        let (mut engine, mut handle) = Engine::new(EngineConfig {
            sample_rate: format.sample_rate,
            channels: format.channels,
            decks: 1,
        });
        handle.send_deck(DECK, DeckCommand::Load(Arc::clone(&source)))?;
        let analysis = analyze_in_background(file.to_path_buf(), format.sample_rate, settings);

        let meter = Arc::clone(handle.load_meter());
        let ns_per_frame = 1e9 / f64::from(format.sample_rate);
        let callback = move |out: &mut [f32], frames: usize| {
            let start = Instant::now();
            assert_no_alloc::assert_no_alloc(|| engine.process(out, frames));
            let busy = start.elapsed().as_nanos() as u64;
            meter.record(busy, (frames as f64 * ns_per_frame) as u64);
        };
        let stream = backend.start(request, Box::new(callback))?;
        Ok(Self {
            format,
            info: loading.info,
            source,
            handle,
            stream: Some(stream),
            loader: Some(loading.thread),
            analysis,
            analysed: None,
        })
    }

    pub fn device_name(&self) -> &str {
        self.stream.as_ref().map_or("", |s| s.device_name())
    }

    pub fn buffer_frames(&self) -> Option<u32> {
        self.stream.as_ref().and_then(|s| s.buffer_frames())
    }

    pub fn state(&self) -> &Arc<DeckState> {
        self.handle.deck_state(DECK)
    }

    pub fn load_meter(&self) -> &Arc<LoadMeter> {
        self.handle.load_meter()
    }

    /// Send a command to the deck; false if the command ring is full.
    pub fn send(&mut self, cmd: DeckCommand) -> bool {
        self.handle.send_deck(DECK, cmd).is_ok()
    }

    /// Housekeeping; returns lines worth showing the user (analysis result, errors).
    pub fn poll(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Ok(result) = self.analysis.try_recv() {
            match result {
                Ok(done) => {
                    lines.push(done.summary.clone());
                    if let Some(grid) = &done.grid {
                        let cmd = DeckCommand::SetGrid {
                            source_id: self.source.id(),
                            grid: Arc::clone(grid),
                        };
                        if !self.send(cmd) {
                            lines.push("command ring full, grid dropped".into());
                        }
                    }
                    self.analysed = Some(done);
                }
                Err(e) => lines.push(format!("analysis: {e}")),
            }
        }
        self.handle.collect_retired();
        if self.loader.as_ref().is_some_and(|t| t.is_finished())
            && let Some(Err(e)) = self.loader.take().and_then(|t| t.join().ok())
        {
            lines.push(format!("loader: {e} (playing what was decoded)"));
        }
        if let Some(stream) = &self.stream {
            lines.extend(
                stream
                    .take_errors()
                    .into_iter()
                    .map(|e| format!("stream: {e}")),
            );
        }
        lines
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        drop(self.stream.take());
        self.handle.collect_retired();
    }
}

/// What the analysis thread hands back: the deck grid (engine frames), the key and a line to
/// print.
pub struct Analysed {
    pub grid: Option<Arc<DeckGrid>>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub summary: String,
}

/// Analyse `path` on its own thread: tempo (TempoCNN, if `TEMPOCNN_MODEL` is set) refined by
/// gridfit, the grid, and the key. The result arrives on the returned channel.
fn analyze_in_background(
    path: PathBuf,
    engine_rate: u32,
    settings: engine_analysis::Settings,
) -> Receiver<Result<Analysed, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || {
            let _ = tx.send(analyze(&path, engine_rate, settings));
        });
    if let Err(e) = spawned {
        // The sender is gone with the closure: the receiver just never yields.
        eprintln!("analysis thread: {e}");
    }
    rx
}

fn analyze(
    path: &Path,
    engine_rate: u32,
    settings: engine_analysis::Settings,
) -> Result<Analysed, String> {
    use engine_analysis::{Analysis, Analyzer, CancelFlag, FeatureSet, GridFit, GridFitConfig};
    let started = Instant::now();
    let audio = engine_analysis::load(path).map_err(|e| e.to_string())?;
    let mut analyzers: Vec<Box<dyn Analyzer>> = Vec::new();
    let mut hint = None;
    if let Some(t) = engine_analysis::tempocnn::TempoCnn::from_env() {
        analyzers.push(Box::new(t?.with_range(settings.bpm_range)));
        hint = Some("tempocnn");
    }
    analyzers.push(Box::new(GridFit::new(GridFitConfig {
        tempo_hint: hint,
        bpm_range: settings.bpm_range,
        ..GridFitConfig::default()
    })));
    analyzers.push(Box::new(engine_analysis::hpcpkey::HpcpKey::default()));
    let results =
        engine_analysis::run_all(&mut analyzers, &audio, FeatureSet::ALL, &CancelFlag::new())
            .map_err(|e| e.to_string())?;
    // Later analyzers refine earlier ones (gridfit's tempo over TempoCNN's).
    let mut merged = Analysis::default();
    for (_, r) in results {
        let r = r.map_err(|e| e.to_string())?;
        merged.tempo = r.tempo.or(merged.tempo);
        merged.beatgrid = r.beatgrid.or(merged.beatgrid);
        merged.key = r.key.or(merged.key);
    }
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
        "analysed in {:.1} s: {}, key {}, first downbeat {}{}",
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
    })
}
