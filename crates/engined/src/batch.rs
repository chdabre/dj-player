//! ANALYZE ALL: analyse a list of files one after another on a background thread, for the
//! library: tempo, grid and key (as a loaded track gets them), the waveform and the audio
//! hash, each file decoded once.
//!
//! The deck comes first: between files the job waits while `busy` is set (the player sets it
//! while its own track decodes or analyses), and [`Batch::stop`] cancels the file under way.
//! Results go to the player as [`BatchEvent`]s; it hands them to the library worker
//! ([`crate::persist::LibraryWorker::store`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;

use waveform::compute::{Builder, Origin, Params, Timeline};

use crate::persist::BatchResult;

/// What the batch job reports.
#[derive(Debug)]
pub enum BatchEvent {
    /// Working on this file (index among the total).
    Started(usize, PathBuf),
    Done(Box<BatchResult>),
    Failed(PathBuf, String),
    /// Every file done, or stopped.
    Finished {
        stopped: bool,
    },
}

/// A running batch job.
pub struct Batch {
    rx: Receiver<BatchEvent>,
    stop: engine_media::Cancel,
    thread: Option<JoinHandle<()>>,
    pub total: usize,
}

/// How often a waiting job looks whether the deck is free again.
const WAIT: Duration = Duration::from_millis(100);

impl Batch {
    /// Analyse `files` in order. `engine_rate` only scales the grid the analysis also makes
    /// (the stored results are in seconds).
    pub fn start(
        files: Vec<PathBuf>,
        settings: engine_analysis::Settings,
        engine_rate: u32,
        busy: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = engine_media::Cancel::new();
        let total = files.len();
        let job_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("batch analysis".into())
            .spawn(move || {
                for (i, file) in files.into_iter().enumerate() {
                    while busy.load(Ordering::Relaxed) && !job_stop.is_cancelled() {
                        std::thread::sleep(WAIT);
                    }
                    if job_stop.is_cancelled() {
                        break;
                    }
                    let _ = tx.send(BatchEvent::Started(i, file.clone()));
                    let event = match analyse_file(&file, &job_stop, settings, engine_rate) {
                        Ok(r) => BatchEvent::Done(Box::new(r)),
                        Err(_) if job_stop.is_cancelled() => break,
                        Err(e) => BatchEvent::Failed(file, e),
                    };
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                let _ = tx.send(BatchEvent::Finished {
                    stopped: job_stop.is_cancelled(),
                });
            })?;
        Ok(Self {
            rx,
            stop,
            thread: Some(thread),
            total,
        })
    }

    pub fn try_recv(&self) -> Option<BatchEvent> {
        self.rx.try_recv().ok()
    }

    /// Stop after (cancelling) the file under way.
    pub fn stop(&self) {
        self.stop.cancel();
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Decode `file` once and compute everything the library stores for it.
fn analyse_file(
    file: &std::path::Path,
    stop: &engine_media::Cancel,
    settings: engine_analysis::Settings,
    engine_rate: u32,
) -> Result<BatchResult, String> {
    let info = engine_media::probe(file).map_err(|e| e.to_string())?;
    let mut audio = Some(engine_analysis::load_until(file, stop).map_err(|e| e.to_string())?);
    let analysed = crate::analyze_with(
        file,
        &mut audio,
        stop,
        engine_rate,
        settings,
        &BTreeMap::new(),
        true,
    )?;
    let audio = audio.expect("decoded above");
    // The waveform from the same decode, on the gapless-trimmed timeline the deck uses.
    let mut builder = Builder::new(audio.sample_rate, &Params::default());
    for chunk in audio.stereo.chunks(8192) {
        builder.push(chunk);
    }
    let mut waveform = builder.finish();
    waveform.timeline = Timeline {
        origin: Origin::Trimmed,
        start_trim: info.start_trim,
        file_rate: audio.sample_rate,
    };
    Ok(BatchResult {
        file: file.to_path_buf(),
        waveform,
        fresh: analysed.fresh,
        audio_hash: analysed.audio_hash,
        bpm: analysed.bpm,
        key: analysed.key,
    })
}
