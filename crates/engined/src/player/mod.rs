//! The [`Player`]: the output stream and engine, the deck's track, and the library worker
//! and background jobs that feed it.

mod controls;
mod cues;
mod options;
mod track;

pub use options::{AutoCue, Options};
pub use track::LoadedTrack;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use engine_core::{DeckCommand, DeckState, Engine, EngineConfig, EngineHandle, LoadMeter};
use engine_io::{AudioBackend, AudioStream, CpalBackend, CpalStream, StreamFormat, StreamRequest};
use library::{AnalysisRef, Edit, Persistence, TrackRef};
use waveform::compute::{Origin, Timeline};

use crate::analysis::analyze_in_background;
use crate::batch::{self, BatchProgress};
use crate::store::{self, LibraryWorker, Reply};
use crate::waveform_live::{LiveWaveform, WaveTap};
use track::LibState;

/// The deck everything here drives.
pub(crate) const DECK: usize = 0;

pub struct Player {
    pub format: StreamFormat,
    handle: EngineHandle,
    /// `None` only while dropping: the stream goes first, so the engine and its sources are
    /// dropped off the audio thread before the retire ring is drained.
    stream: Option<CpalStream>,
    settings: engine_analysis::Settings,
    library: Option<LibraryWorker>,
    auto_cue: AutoCue,
    persistence: Option<Persistence>,
    /// The track on the deck, if one was loaded.
    loaded: Option<LoadedTrack>,
    /// Serial of the last load; library answers for earlier loads are stale.
    loads: u64,
    /// Serial of the last [`DeckCommand::SetCues`] sent, across tracks: the deck's applied
    /// serial is compared against it.
    cue_serial: u64,
    /// The library's tracks, as last published.
    listing: Option<Arc<store::Listing>>,
    /// The library's playlists, as last published.
    playlists: Option<Arc<store::Playlists>>,
    /// The last scan's progress (done, total).
    scan: Option<(usize, usize)>,
    /// ANALYZE ALL under way, and how far it is.
    batch: Option<batch::Batch>,
    batch_progress: Option<BatchProgress>,
    /// Set while the deck's own track decodes or analyses: the batch job waits.
    deck_busy: Arc<AtomicBool>,
    /// What the player's controls do ([`Player::control`]).
    controls: crate::controls::Surface,
}

impl Player {
    /// [`Player::start`], then [`Player::load`] `file`.
    pub fn open_with(
        backend: &CpalBackend,
        request: &StreamRequest,
        file: &Path,
        options: Options,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut player = Self::start(backend, request, options)?;
        player.load(file)?;
        Ok(player)
    }

    /// Open the output with an empty deck and start the library worker (it mounts on the
    /// first load).
    pub fn start(
        backend: &CpalBackend,
        request: &StreamRequest,
        options: Options,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // The device decides the engine rate; tracks are resampled to it at load.
        let format = backend.format(request)?;
        let (mut engine, handle) = Engine::new(EngineConfig {
            sample_rate: format.sample_rate,
            channels: format.channels,
            decks: 1,
        });
        let library = LibraryWorker::start(None, options.library).ok();

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
            handle,
            stream: Some(stream),
            settings: options.settings,
            library,
            auto_cue: options.auto_cue,
            persistence: None,
            loaded: None,
            loads: 0,
            cue_serial: 0,
            listing: None,
            playlists: None,
            scan: None,
            batch: None,
            batch_progress: None,
            deck_busy: Arc::new(AtomicBool::new(false)),
            controls: crate::controls::Surface::new(format.sample_rate),
        })
    }

    /// Load `file` onto the deck, replacing what is there (paused at the cue point once it
    /// is known). The previous track's last cue edits go to the library first.
    pub fn load(&mut self, file: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let waveform = Arc::new(LiveWaveform::default());
        let tap = WaveTap::new(self.format.sample_rate, Arc::clone(&waveform))
            .with_first_sound(self.auto_cue.threshold());
        let loading =
            engine_media::load_file_tapped(file, self.format.sample_rate, Some(tap.boxed()))?;
        waveform.set_timeline(Timeline {
            // Our decoder trims gapless padding: entry 0 is the deck's frame 0.
            origin: Origin::Trimmed,
            start_trim: loading.info.start_trim,
            file_rate: loading.info.file_sample_rate,
        });
        self.write_back_cues();
        self.handle
            .send_deck(DECK, DeckCommand::Load(Arc::clone(&loading.source)))?;
        self.loads += 1;
        if let Some(lib) = &self.library {
            lib.open(self.loads, file.to_path_buf());
        }
        // Dropping the previous track stops its decode and analysis where they are.
        self.loaded = Some(LoadedTrack {
            info: loading.info,
            source: loading.source,
            loader: Some(loading.thread),
            cancel: loading.cancel,
            file: file.to_path_buf(),
            serial: self.loads,
            analysis: None,
            analysed: None,
            waveform,
            lib: LibState::default(),
        });
        if self.library.is_none() {
            self.no_record();
        }
        Ok(())
    }

    /// The library's tracks by stored path ([`store::stored_path`]), once it has said.
    pub fn listing(&self) -> Option<&Arc<store::Listing>> {
        self.listing.as_ref()
    }

    /// The library's playlists, once it has said.
    pub fn playlists(&self) -> Option<&Arc<store::Playlists>> {
        self.playlists.as_ref()
    }

    /// A playlist edit (or any library edit). False without a library.
    pub fn library_edit(&mut self, edit: Edit) -> bool {
        match &self.library {
            Some(lib) => {
                lib.edit(edit);
                true
            }
            None => false,
        }
    }

    /// Add `files` at the end of `playlist` (records made where missing), in the background.
    pub fn add_files(&mut self, playlist: library::PlaylistId, files: Vec<PathBuf>) -> bool {
        match &self.library {
            Some(lib) if !files.is_empty() => {
                lib.add_files(playlist, files);
                true
            }
            _ => false,
        }
    }

    /// MAKE PLAYLIST from `files`: a new list at the end of folder `parent`.
    pub fn new_playlist(
        &mut self,
        id: library::PlaylistId,
        parent: Option<library::PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    ) -> bool {
        match &self.library {
            Some(lib) => {
                lib.new_playlist(id, parent, name, files);
                true
            }
            None => false,
        }
    }

    /// Scan the stick: give every audio file under its root a record, with its tags.
    pub fn scan(&mut self) {
        if let Some(lib) = &self.library {
            lib.scan();
            self.scan.get_or_insert((0, 0));
        }
    }

    /// ANALYZE ALL: analyse `files` for the library in a background job, one at a time, the
    /// deck's own track first. False (and nothing started) without a library or while a
    /// job runs.
    pub fn analyze_all(&mut self, files: Vec<PathBuf>) -> bool {
        if self.library.is_none() || self.batch.is_some() || files.is_empty() {
            return false;
        }
        match batch::Batch::start(
            files,
            self.settings,
            self.format.sample_rate,
            Arc::clone(&self.deck_busy),
        ) {
            Ok(b) => {
                self.batch_progress = Some(BatchProgress {
                    total: b.total,
                    ..BatchProgress::default()
                });
                self.batch = Some(b);
                true
            }
            Err(_) => false,
        }
    }

    /// Stop ANALYZE ALL (the file under way is cancelled; what is done stays stored).
    pub fn stop_analyze_all(&mut self) {
        if let Some(b) = &self.batch {
            b.stop();
            if let Some(p) = self.batch_progress.as_mut() {
                p.stopping = true;
            }
        }
    }

    /// How far ANALYZE ALL is, while it runs.
    pub fn batch_progress(&self) -> Option<&BatchProgress> {
        self.batch_progress.as_ref()
    }

    /// The scan's progress (files done, total) once one was asked for.
    pub fn scan_progress(&self) -> Option<(usize, usize)> {
        self.scan
    }

    /// The track on the deck, if any.
    pub fn loaded(&self) -> Option<&LoadedTrack> {
        self.loaded.as_ref()
    }

    /// Whether the library is saving this deck's edits; `None` until it has said.
    pub fn persistence(&self) -> Option<&Persistence> {
        self.persistence.as_ref()
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

    /// The loaded track in its library, once the library found or made its record.
    pub fn track(&self) -> Option<TrackRef> {
        self.loaded.as_ref().and_then(LoadedTrack::track)
    }

    /// Send a command to the deck; false if the command ring is full.
    pub fn send(&mut self, cmd: DeckCommand) -> bool {
        self.handle.send_deck(DECK, cmd).is_ok()
    }

    /// The loaded track has no library record (no library, or the file could not be read):
    /// analyse from scratch, and nothing is stored.
    fn no_record(&mut self) {
        if let Some(d) = self.loaded.as_mut() {
            d.lib.memory_cues.get_or_insert(0);
        }
        self.start_analysis(BTreeMap::new(), false);
    }

    fn start_analysis(
        &mut self,
        cached: BTreeMap<String, (AnalysisRef, Vec<u8>)>,
        audio_hash: bool,
    ) {
        let Some(d) = self.loaded.as_mut() else {
            return;
        };
        if d.analysis.is_none() {
            d.analysis = Some(analyze_in_background(
                d.file.clone(),
                d.cancel.clone(),
                self.format.sample_rate,
                self.settings,
                cached,
                audio_hash,
            ));
        }
    }

    /// Housekeeping; returns lines worth showing the user (library, analysis, errors).
    pub fn poll(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        self.poll_library(&mut lines);
        self.poll_analysis(&mut lines);
        self.store_waveform();
        self.poll_batch(&mut lines);
        self.apply_auto_cue(&mut lines);
        self.write_back_cues();
        self.handle.collect_retired();
        self.poll_loader(&mut lines);
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

    /// Take in the library worker's replies.
    fn poll_library(&mut self, lines: &mut Vec<String>) {
        while let Some(reply) = self.library.as_ref().and_then(LibraryWorker::try_recv) {
            match reply {
                Reply::Message(m) => lines.push(m),
                Reply::Loaded(l) => self.on_loaded(*l, lines),
                Reply::Unrecorded { serial, reason } => {
                    if self.loaded.as_ref().is_some_and(|d| d.serial == serial) {
                        lines.push(format!(
                            "library: {reason}; this track's edits are not stored"
                        ));
                        self.no_record();
                    }
                }
                Reply::Failed(e) => {
                    lines.push(format!("library: {e}; nothing is stored"));
                    self.library = None;
                    self.persistence = Some(Persistence::Unsaved {
                        edits: 0,
                        reason: e,
                    });
                    self.no_record();
                }
                Reply::Status(p) => self.persistence = Some(p),
                Reply::Listing(l) => self.listing = Some(l),
                Reply::Playlists(p) => self.playlists = Some(p),
                Reply::Scan { done, total } => self.scan = Some((done, total)),
            }
        }
    }

    /// The loaded track's analysis, once it arrives: the grid to the deck, the results to
    /// the library.
    fn poll_analysis(&mut self, lines: &mut Vec<String>) {
        let Some(d) = self.loaded.as_mut() else {
            return;
        };
        let Some(Ok(result)) = d.analysis.as_ref().map(Receiver::try_recv) else {
            return;
        };
        match result {
            Ok(mut done) => {
                lines.push(done.summary.clone());
                if let Some(grid) = &done.grid {
                    let cmd = DeckCommand::SetGrid {
                        source_id: d.source.id(),
                        grid: Arc::clone(grid),
                    };
                    if self.handle.send_deck(DECK, cmd).is_err() {
                        lines.push("command ring full, grid dropped".into());
                    }
                }
                if let Some(lib) = &self.library
                    && d.lib.track.is_some()
                {
                    lib.analysis(std::mem::take(&mut done.fresh));
                    lib.analysed(done.bpm, done.key.clone());
                    if let Some(h) = done.audio_hash {
                        lib.audio_hash(h);
                    }
                }
                d.analysed = Some(done);
            }
            Err(e) => lines.push(format!("analysis: {e}")),
        }
    }

    /// Store the computed waveform once the decode finished it (once per track).
    fn store_waveform(&mut self) {
        if let Some(d) = self.loaded.as_mut()
            && !d.lib.waveform_stored
            && d.lib.track.is_some()
            && let Some(lib) = &self.library
            && let Some(w) = d.waveform.finished()
        {
            lib.waveform(w);
            d.lib.waveform_stored = true;
        }
    }

    /// Pass the batch job's results to the library, and hold it while the deck is busy.
    fn poll_batch(&mut self, lines: &mut Vec<String>) {
        let busy = self
            .loaded
            .as_ref()
            .is_some_and(|d| d.loader.is_some() || (d.analysis.is_some() && d.analysed.is_none()));
        self.deck_busy.store(busy, Ordering::Relaxed);
        while let Some(event) = self.batch.as_ref().and_then(batch::Batch::try_recv) {
            let Some(progress) = self.batch_progress.as_mut() else {
                break;
            };
            let finished = matches!(event, batch::BatchEvent::Finished { .. });
            if let Some(r) = progress.note(event, lines)
                && let Some(lib) = &self.library
            {
                lib.store(*r);
            }
            if finished {
                self.batch = None;
                self.batch_progress = None;
            }
        }
    }

    /// Report a decode that ended in an error.
    fn poll_loader(&mut self, lines: &mut Vec<String>) {
        if let Some(d) = self.loaded.as_mut()
            && d.loader.as_ref().is_some_and(|t| t.is_finished())
            && let Some(Err(e)) = d.loader.take().and_then(|t| t.join().ok())
        {
            lines.push(format!("loader: {e} (playing what was decoded)"));
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Last deck edits first; dropping the worker then waits for its final flush.
        self.write_back_cues();
        drop(self.library.take());
        drop(self.stream.take());
        self.handle.collect_retired();
    }
}
