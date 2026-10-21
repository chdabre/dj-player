//! One deck playing one file through the real engine: device, decode, analysis, library and
//! stream, shared by the `engined` terminal CLI and the UI spike.
//!
//! [`Player::open`] picks the device format, starts decoding the file at that rate, starts
//! the stream and mounts the library on a worker thread ([`persist`]). The waveform is
//! computed during the decode, from exactly the frames the deck plays, and grows in
//! [`Player::waveform`]. The owner calls [`Player::poll`] regularly: it restores the track's
//! stored cues into the deck, uses the stored waveform and analysis when the library has
//! them (analysing otherwise, on a background thread), hands the grid to the deck, writes
//! new waveforms, analysis results and cue edits back, collects retired objects off the
//! audio thread and reports loader and stream errors.

pub mod batch;
pub mod browse;
pub mod persist;
mod stored;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Instant;

use engine_core::{
    DeckCommand, DeckGrid, DeckState, Engine, EngineConfig, EngineHandle, HOT_CUES, LoadMeter,
    Source, TrackCues,
};
use engine_io::{AudioBackend, AudioStream, CpalBackend, CpalStream, StreamFormat, StreamRequest};
use library::{AnalysisRef, AudioHash, Cue, CueKind, Edit, HotCue, Track, TrackRef};
use waveform::compute::{Builder, Entry, Origin, Params, Timeline, Waveform};

pub use library::Persistence;
pub use persist::LibraryChoice;
use persist::{FreshAnalysis, LibraryWorker, Loaded, Reply};

/// The deck everything here drives.
pub const DECK: usize = 0;

/// How to open a [`Player`].
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub settings: engine_analysis::Settings,
    pub library: LibraryChoice,
    pub auto_cue: AutoCue,
}

/// Where the cue point goes when a track loads: the first stored memory cue if there is
/// one; else, with auto cue on, the first sound at or above the level; else no cue point,
/// the playhead at the track start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoCue {
    Off,
    /// Peak level in dBFS (the reference player offers −36 to −78 dB).
    Level(f32),
}

impl Default for AutoCue {
    fn default() -> Self {
        AutoCue::Level(-36.0)
    }
}

impl AutoCue {
    /// `off`, or a level in dB (`-36`, `-48`, …).
    pub fn parse(s: &str) -> Option<Self> {
        if s == "off" {
            return Some(AutoCue::Off);
        }
        let db: f32 = s.trim_end_matches("dB").parse().ok()?;
        (db.is_finite() && db <= 0.0).then_some(AutoCue::Level(db))
    }

    /// The linear peak that counts as sound, if auto cue is on.
    fn threshold(self) -> Option<f32> {
        match self {
            AutoCue::Off => None,
            AutoCue::Level(db) => Some(10f32.powf(db / 20.0)),
        }
    }
}

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
    listing: Option<Arc<persist::Listing>>,
    /// The last scan's progress (done, total).
    scan: Option<(usize, usize)>,
    /// ANALYZE ALL under way, and how far it is.
    batch: Option<batch::Batch>,
    batch_progress: Option<BatchProgress>,
    /// Set while the deck's own track decodes or analyses: the batch job waits.
    deck_busy: Arc<AtomicBool>,
}

/// How far ANALYZE ALL is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchProgress {
    /// Files finished (analysed or failed) of `total`.
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    /// The file under way.
    pub current: Option<String>,
    /// Asked to stop; finishing the file under way.
    pub stopping: bool,
}

/// The track on the deck: its source and decode, waveform, analysis and library record.
pub struct LoadedTrack {
    pub info: engine_media::TrackInfo,
    pub source: Arc<Source>,
    loader: Option<JoinHandle<Result<(), engine_media::LoadError>>>,
    /// Stops the decode and the analysis when the track is replaced.
    cancel: engine_media::Cancel,
    file: PathBuf,
    /// The load serial the library answers with.
    serial: u64,
    /// Started once the library said what it has for the track.
    analysis: Option<Receiver<Result<Analysed, String>>>,
    /// The analysis result, once it arrived.
    pub analysed: Option<Analysed>,
    waveform: Arc<LiveWaveform>,
    lib: LibState,
}

impl Drop for LoadedTrack {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl LoadedTrack {
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The track's waveform, growing while the track decodes (or the stored one).
    pub fn waveform(&self) -> &Arc<LiveWaveform> {
        &self.waveform
    }

    /// The track in its library, once the library found or made its record.
    pub fn track(&self) -> Option<TrackRef> {
        self.lib.track
    }
}

/// What the player knows about the track's library record.
#[derive(Debug, Default)]
struct LibState {
    track: Option<TrackRef>,
    /// Serial of the last stored cues sent to the deck (0 = none yet).
    serial: u64,
    /// Hot cues (frames) as the library gave them, and as last seen on the deck.
    restored: [Option<f64>; HOT_CUES],
    known: [Option<f64>; HOT_CUES],
    /// The library's hot cues per slot, so a deck edit (a new position) keeps the rest:
    /// end, colour, comment and fields this version doesn't know.
    records: [Option<HotCue>; HOT_CUES],
    /// The waveform is stored, or was sent to be stored.
    waveform_stored: bool,
    /// Memory cues in the record, once the library answered (0 if there is no record).
    memory_cues: Option<usize>,
    /// The load-time cue point is decided (a memory cue, the first sound, or none).
    cue_decided: bool,
}

impl Player {
    /// Open the output, start loading `file` and start playing paused, with the library at
    /// its default place (see [`LibraryChoice::Auto`]).
    pub fn open(
        backend: &CpalBackend,
        request: &StreamRequest,
        file: &Path,
        settings: engine_analysis::Settings,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let options = Options {
            settings,
            ..Options::default()
        };
        Self::open_with(backend, request, file, options)
    }

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
            scan: None,
            batch: None,
            batch_progress: None,
            deck_busy: Arc::new(AtomicBool::new(false)),
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

    /// The library's tracks by stored path ([`persist::stored_path`]), once it has said.
    pub fn listing(&self) -> Option<&Arc<persist::Listing>> {
        self.listing.as_ref()
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

    /// MEMORY: store the active loop, else the cue point, as a memory cue in the library.
    /// Returns a line to show.
    pub fn memory(&mut self) -> String {
        let (Some(t), Some(lib)) = (self.track(), &self.library) else {
            return "no library record yet: memory not stored".into();
        };
        let st = self.handle.deck_state(DECK);
        let sr = f64::from(self.format.sample_rate);
        let cue = if st.looping.load(Ordering::Relaxed) {
            Cue::memory_loop(st.loop_start.load() / sr, st.loop_end.load() / sr)
        } else {
            Cue::memory(st.cue.load() / sr)
        };
        let line = match cue.end {
            Some(end) => format!("memory loop {:.3}-{:.3} s stored", cue.at, end),
            None => format!("memory cue {:.3} s stored", cue.at),
        };
        lib.edit(Edit::AddCue(t.track, cue));
        line
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

    /// Hand the record's cues to the deck. `fill_only` keeps whatever the deck already has
    /// (cues the DJ set before the library answered); the next [`Player::poll`] then writes
    /// those to the library, since they differ from what it gave.
    fn restore_cues(&mut self, record: &Track, fill_only: bool) -> bool {
        let Some(d) = self.loaded.as_mut() else {
            return false;
        };
        let sr = f64::from(self.format.sample_rate);
        let mut hot = [None; HOT_CUES];
        for (slot, h) in hot.iter_mut().zip(&record.hot_cues) {
            *slot = h.as_ref().map(|h| h.at * sr);
        }
        let first = record.cues.first().map(|(_, c)| c);
        let cues = TrackCues {
            cue: first.map(|c| c.at * sr),
            hot,
            stored_loop: first.and_then(|c| match (c.kind, c.end) {
                (CueKind::Loop, Some(end)) => Some((c.at * sr, end * sr)),
                _ => None,
            }),
            fill_only,
        };
        let serial = self.cue_serial + 1;
        let sent = self
            .handle
            .send_deck(
                DECK,
                DeckCommand::SetCues {
                    source_id: d.source.id(),
                    serial,
                    cues,
                },
            )
            .is_ok();
        if sent {
            self.cue_serial = serial;
            d.lib.serial = serial;
            d.lib.restored = hot;
            d.lib.known = hot;
            d.lib.records = std::array::from_fn(|i| record.hot_cues.get(i).cloned().flatten());
        }
        sent
    }

    fn on_loaded(&mut self, l: Loaded, lines: &mut Vec<String>) {
        let Some(d) = self.loaded.as_mut().filter(|d| d.serial == l.serial) else {
            return; // for a track no longer on the deck
        };
        let first = d.lib.track.is_none();
        d.lib.track = Some(l.track);
        if first {
            d.lib.memory_cues = Some(l.record.cues.len());
        }
        let hot = l.record.hot_cues.iter().flatten().count();
        let cues = l.record.cues.len();
        // The first restore only fills what the DJ hasn't set while the library was being
        // read (and, with nothing stored, gives the deck a serial to compare against). On a
        // merge, the merged record's cues replace the deck's only if the DJ has not edited
        // them since they were restored.
        let untouched = d.lib.known == d.lib.restored;
        if first || untouched {
            if !self.restore_cues(&l.record, first) {
                lines.push("command ring full, stored cues dropped".into());
            } else if hot + cues > 0 {
                lines.push(format!(
                    "library: {hot} hot cues and {cues} memory cues restored"
                ));
            }
        }
        if first {
            let Some(d) = self.loaded.as_mut() else {
                return;
            };
            if let Some(w) = l.waveform {
                if d.waveform.set_stored(w) {
                    lines.push("library: waveform from the library".into());
                }
                d.lib.waveform_stored = true;
            }
            let need_hash = !l.record.audio_hash.is_some_and(|a| a.is_current());
            self.start_analysis(l.analysis, need_hash);
        }
    }

    /// The load-time cue point when the track has no memory cue: the first sound, with auto
    /// cue on. Waits for the library's answer (a stored memory cue wins). Only fills: a cue
    /// point the DJ set meanwhile stays.
    fn apply_auto_cue(&mut self, lines: &mut Vec<String>) {
        let Some(d) = self.loaded.as_mut() else {
            return;
        };
        if d.lib.cue_decided {
            return;
        }
        let Some(memory) = d.lib.memory_cues else {
            return; // still waiting for the record
        };
        if memory > 0 || self.auto_cue == AutoCue::Off {
            d.lib.cue_decided = true;
            return;
        }
        let Some(first) = d.waveform.first_sound() else {
            return;
        };
        d.lib.cue_decided = true;
        let Some(at) = first else {
            return; // silent all through: no cue point
        };
        let serial = self.cue_serial + 1;
        let sent = self
            .handle
            .send_deck(
                DECK,
                DeckCommand::SetCues {
                    source_id: d.source.id(),
                    serial,
                    cues: TrackCues {
                        cue: Some(at),
                        hot: [None; HOT_CUES],
                        stored_loop: None,
                        fill_only: true,
                    },
                },
            )
            .is_ok();
        if sent {
            self.cue_serial = serial;
            d.lib.serial = serial;
            lines.push(format!(
                "auto cue: first sound at {:.3} s",
                at / f64::from(self.format.sample_rate)
            ));
        } else {
            lines.push("command ring full, auto cue dropped".into());
        }
    }

    /// Write hot cue changes made on the deck to the library.
    fn write_back_cues(&mut self) {
        let (Some(d), Some(lib)) = (self.loaded.as_mut(), &self.library) else {
            return;
        };
        let Some(t) = d.lib.track else {
            return;
        };
        let st = self.handle.deck_state(DECK);
        if st.cues_serial.load(Ordering::Relaxed) != d.lib.serial {
            // The deck has not applied the stored cues yet.
            return;
        }
        let sr = f64::from(self.format.sample_rate);
        for (slot, pos) in st.hot_cue_positions.iter().enumerate() {
            let p = pos.load();
            let now = (!p.is_nan()).then_some(p);
            if now != d.lib.known[slot] {
                d.lib.known[slot] = now;
                let cue = now.map(|f| {
                    let at = f / sr;
                    match d.lib.records[slot].take() {
                        // Moved: the same cue at its new place. A loop end that would now
                        // come before the start no longer applies.
                        Some(old) => HotCue {
                            at,
                            end: old.end.filter(|&e| e > at),
                            ..old
                        },
                        None => HotCue::at(at),
                    }
                });
                d.lib.records[slot] = cue.clone();
                lib.edit(Edit::SetHotCue(t.track, slot, cue));
            }
        }
    }

    /// Pass the batch job's results to the library, and hold it while the deck is busy.
    fn poll_batch(&mut self, lines: &mut Vec<String>) {
        let busy = self
            .loaded
            .as_ref()
            .is_some_and(|d| d.loader.is_some() || (d.analysis.is_some() && d.analysed.is_none()));
        self.deck_busy.store(busy, Ordering::Relaxed);
        let name = |p: &Path| {
            p.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        };
        while let Some(event) = self.batch.as_ref().and_then(batch::Batch::try_recv) {
            let Some(progress) = self.batch_progress.as_mut() else {
                break;
            };
            match event {
                batch::BatchEvent::Started(_, file) => progress.current = Some(name(&file)),
                batch::BatchEvent::Done(r) => {
                    progress.done += 1;
                    lines.push(format!(
                        "analyze all {}/{}: {}: {}, key {}",
                        progress.done,
                        progress.total,
                        name(&r.file),
                        r.bpm.map_or("no tempo".into(), |b| format!("{b:.2} BPM")),
                        r.key.as_deref().unwrap_or("?"),
                    ));
                    if let Some(lib) = &self.library {
                        lib.store(*r);
                    }
                }
                batch::BatchEvent::Failed(file, e) => {
                    progress.done += 1;
                    progress.failed += 1;
                    lines.push(format!("analyze all: {}: {e}", name(&file)));
                }
                batch::BatchEvent::Finished { stopped } => {
                    lines.push(format!(
                        "analyze all {}: {} of {} done{}",
                        if stopped { "stopped" } else { "finished" },
                        progress.done - progress.failed,
                        progress.total,
                        if progress.failed > 0 {
                            format!(", {} failed", progress.failed)
                        } else {
                            String::new()
                        }
                    ));
                    self.batch = None;
                    self.batch_progress = None;
                }
            }
        }
    }

    /// Housekeeping; returns lines worth showing the user (library, analysis, errors).
    pub fn poll(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Some(reply) = self.library.as_ref().and_then(LibraryWorker::try_recv) {
            match reply {
                Reply::Message(m) => lines.push(m),
                Reply::Loaded(l) => self.on_loaded(*l, &mut lines),
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
                Reply::Scan { done, total } => self.scan = Some((done, total)),
            }
        }
        if let Some(d) = self.loaded.as_mut()
            && let Some(Ok(result)) = d.analysis.as_ref().map(Receiver::try_recv)
        {
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
        if let Some(d) = self.loaded.as_mut()
            && !d.lib.waveform_stored
            && d.lib.track.is_some()
            && let Some(lib) = &self.library
            && let Some(w) = d.waveform.finished()
        {
            lib.waveform(w);
            d.lib.waveform_stored = true;
        }
        self.poll_batch(&mut lines);
        self.apply_auto_cue(&mut lines);
        self.write_back_cues();
        self.handle.collect_retired();
        if let Some(d) = self.loaded.as_mut()
            && d.loader.as_ref().is_some_and(|t| t.is_finished())
            && let Some(Err(e)) = d.loader.take().and_then(|t| t.join().ok())
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
        // Last deck edits first; dropping the worker then waits for its final flush.
        self.write_back_cues();
        drop(self.library.take());
        drop(self.stream.take());
        self.handle.collect_retired();
    }
}

/// A waveform being computed on the loader thread and read by the UI.
#[derive(Debug, Default)]
pub struct LiveWaveform {
    state: Mutex<LiveState>,
    /// A stored waveform replaced the computation: the tap stops.
    stored: AtomicBool,
}

#[derive(Debug, Default)]
struct LiveState {
    entries: Vec<Entry>,
    done: Option<Waveform>,
    timeline: Timeline,
    /// The first frame at or above the auto cue level: `Some(None)` once the whole track was
    /// decoded without one.
    first_sound: Option<Option<f64>>,
}

impl LiveWaveform {
    fn lock(&self) -> std::sync::MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_timeline(&self, timeline: Timeline) {
        self.lock().timeline = timeline;
    }

    /// Use a stored waveform instead of computing one. False (and ignored) if the
    /// computed one is already complete.
    fn set_stored(&self, w: Waveform) -> bool {
        let mut s = self.lock();
        if s.done.is_some() {
            return false;
        }
        s.entries.clear();
        s.timeline = w.timeline;
        s.done = Some(w);
        self.stored.store(true, Ordering::Relaxed);
        true
    }

    /// Append the entries `have` does not have yet. Returns true once the waveform is
    /// complete (and `have` holds all of it).
    pub fn update(&self, have: &mut Vec<Entry>) -> bool {
        let s = self.lock();
        let entries = s.done.as_ref().map_or(&s.entries, |w| &w.entries);
        if have.len() > entries.len() {
            have.clear();
        }
        have.extend_from_slice(&entries[have.len()..]);
        s.done.is_some()
    }

    /// The first frame (engine rate) at or above the auto cue level, once known; `Some(None)`
    /// if the track has none. `None` while still looking.
    pub fn first_sound(&self) -> Option<Option<f64>> {
        self.lock().first_sound
    }

    /// The finished waveform, with its timeline, once the decode is done.
    pub fn finished(&self) -> Option<Waveform> {
        let s = self.lock();
        s.done.clone().map(|mut w| {
            w.timeline = s.timeline;
            w
        })
    }
}

/// Feeds the decoded audio to a [`Builder`] on the loader thread and publishes the entries
/// every so often. Dropping it (the loader is done) finishes the waveform.
struct WaveTap {
    builder: Option<Builder>,
    live: Arc<LiveWaveform>,
    /// Frames since the last publish.
    pending: usize,
    publish_every: usize,
    /// The auto cue level (linear peak) while still looking for the first sound.
    first_sound: Option<f32>,
    /// Frames seen so far.
    frames: u64,
}

impl WaveTap {
    fn new(sample_rate: u32, live: Arc<LiveWaveform>) -> Self {
        Self {
            builder: Some(Builder::new(sample_rate, &Params::default())),
            live,
            pending: 0,
            // A quarter second of audio: the display grows smoothly, the lock stays rare.
            publish_every: sample_rate as usize / 4,
            first_sound: None,
            frames: 0,
        }
    }

    /// Also look for the first frame whose peak (either channel) reaches `threshold`.
    fn with_first_sound(mut self, threshold: Option<f32>) -> Self {
        self.first_sound = threshold;
        self
    }

    fn find_first_sound(&mut self, stereo: &[f32]) {
        let Some(level) = self.first_sound else {
            return;
        };
        let hit = stereo
            .as_chunks::<2>()
            .0
            .iter()
            .position(|f| f[0].abs().max(f[1].abs()) >= level);
        if let Some(i) = hit {
            self.live.lock().first_sound = Some(Some((self.frames + i as u64) as f64));
            self.first_sound = None;
        }
        self.frames += (stereo.len() / 2) as u64;
    }

    fn boxed(mut self) -> engine_media::Tap {
        Box::new(move |stereo: &[f32]| self.push(stereo))
    }

    fn push(&mut self, stereo: &[f32]) {
        self.find_first_sound(stereo);
        if self.live.stored.load(Ordering::Relaxed) {
            self.builder = None;
        }
        let Some(b) = self.builder.as_mut() else {
            return;
        };
        b.push(stereo);
        self.pending += stereo.len() / 2;
        if self.pending >= self.publish_every {
            self.pending = 0;
            let mut s = self.live.lock();
            let have = s.entries.len();
            s.entries.extend_from_slice(&b.entries()[have..]);
        }
    }
}

impl Drop for WaveTap {
    fn drop(&mut self) {
        if self.first_sound.is_some() {
            // Decoded to the end without reaching the level.
            self.live.lock().first_sound = Some(None);
        }
        if let Some(b) = self.builder.take() {
            let wf = b.finish();
            let mut s = self.live.lock();
            if s.done.is_none() {
                s.entries.clear();
                s.done = Some(wf);
            }
        }
    }
}

/// What the analysis thread hands back: the deck grid (engine frames), the key, a line to
/// print, and what to store in the library.
pub struct Analysed {
    pub grid: Option<Arc<DeckGrid>>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub summary: String,
    /// Results computed now (not from the library), to be stored.
    pub fresh: Vec<FreshAnalysis>,
    /// Analyzers whose result came from the library.
    pub from_library: usize,
    /// The audio hash, if it was asked for.
    pub audio_hash: Option<AudioHash>,
}

/// Analyse `path` on its own thread: tempo (TempoCNN, if `TEMPOCNN_MODEL` is set) refined by
/// gridfit, the grid, and the key, reusing `cached` results whose analyzer version and
/// parameters still match. The result arrives on the returned channel.
fn analyze_in_background(
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
fn analyze_with(
    path: &Path,
    audio: &mut Option<engine_analysis::TrackAudio>,
    stop: &engine_media::Cancel,
    engine_rate: u32,
    settings: engine_analysis::Settings,
    cached: &BTreeMap<String, (AnalysisRef, Vec<u8>)>,
    want_audio_hash: bool,
) -> Result<Analysed, String> {
    use engine_analysis::{Analysis, Analyzer, CancelFlag, FeatureSet, GridFit, GridFitConfig};
    let started = Instant::now();
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

    // Decoded only if something has to be computed.
    let load = || -> Result<engine_analysis::TrackAudio, String> {
        engine_analysis::load_until(path, stop).map_err(|e| e.to_string())
    };
    let cancel = CancelFlag::from_shared(stop.flag());
    let mut context: Vec<Analysis> = Vec::new();
    let mut fresh = Vec::new();
    let mut from_library = 0;
    for a in &mut analyzers {
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
            .and_then(|(_, blob)| stored::decode(blob, &id));
        let result = match reuse {
            Some(r) => {
                from_library += 1;
                r
            }
            None => {
                if audio.is_none() {
                    *audio = Some(load()?);
                }
                let audio = audio.as_ref().expect("just loaded");
                let r = a
                    .analyze(audio, want, &context, &cancel)
                    .map_err(|e| e.to_string())?;
                fresh.push(FreshAnalysis {
                    name: id.name.clone(),
                    version: id.version.clone(),
                    params: id.params.clone(),
                    blob: stored::encode(&r),
                });
                r
            }
        };
        context.push(result);
    }
    let audio_hash = if want_audio_hash {
        if audio.is_none() {
            *audio = Some(load()?);
        }
        audio.as_ref().map(|a| AudioHash::of_pcm(&a.stereo))
    } else {
        None
    };
    // Later analyzers refine earlier ones (gridfit's tempo over TempoCNN's).
    let mut merged = Analysis::default();
    for r in context {
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
        from_library,
        audio_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_waveform_grows_during_decode_and_ends_as_computed() {
        let sr = 48_000;
        let audio: Vec<f32> = (0..sr * 3)
            .flat_map(|i| {
                let x = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.5;
                [x, x]
            })
            .collect();
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(sr as u32, Arc::clone(&live)).boxed();
        let mut seen = Vec::new();
        let mut grew = 0;
        for chunk in audio.chunks(4096) {
            tap(chunk);
            let before = seen.len();
            assert!(!live.update(&mut seen), "not complete while decoding");
            grew += usize::from(seen.len() > before);
        }
        assert!(grew >= 8, "published {grew} times");
        drop(tap); // the loader is done
        assert!(live.update(&mut seen));
        let batch = waveform::compute::compute(&audio, sr as u32, &Params::default());
        assert_eq!(seen, batch.entries);
        assert_eq!(live.finished().unwrap().entries, batch.entries);
    }

    #[test]
    fn a_stored_waveform_stops_the_computation() {
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(48_000, Arc::clone(&live)).boxed();
        tap(&vec![0.5; 2 * 48_000]);
        let stored = waveform::compute::compute(&vec![0.1; 4 * 48_000], 48_000, &Params::default());
        assert!(live.set_stored(stored.clone()));
        tap(&vec![0.5; 2 * 48_000]);
        drop(tap);
        assert_eq!(live.finished().unwrap().entries, stored.entries);
        assert!(!live.set_stored(stored), "only before one is complete");
    }

    #[test]
    fn the_tap_finds_the_first_sound_at_the_auto_cue_level() {
        let at = AutoCue::Level(-36.0).threshold();
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(48_000, Arc::clone(&live))
            .with_first_sound(at)
            .boxed();
        // Silence, then a noise floor under the level, then sound in the right channel
        // only, in the second block.
        tap(&vec![0.0; 2 * 1000]);
        assert_eq!(live.first_sound(), None, "still looking");
        let mut block = vec![0.001f32; 2 * 1000];
        block[2 * 250 + 1] = 0.1;
        tap(&block);
        assert_eq!(live.first_sound(), Some(Some(1250.0)));
        drop(tap);
        assert_eq!(live.first_sound(), Some(Some(1250.0)));

        // A track that never reaches the level has no first sound.
        let live = Arc::new(LiveWaveform::default());
        let mut tap = WaveTap::new(48_000, Arc::clone(&live))
            .with_first_sound(at)
            .boxed();
        tap(&vec![0.01; 2 * 1000]);
        drop(tap);
        assert_eq!(live.first_sound(), Some(None));
    }

    #[test]
    fn auto_cue_levels_parse() {
        assert_eq!(AutoCue::parse("off"), Some(AutoCue::Off));
        assert_eq!(AutoCue::parse("-48"), Some(AutoCue::Level(-48.0)));
        assert_eq!(AutoCue::parse("-60dB"), Some(AutoCue::Level(-60.0)));
        assert_eq!(AutoCue::parse("6"), None);
        assert_eq!(AutoCue::parse("loud"), None);
        assert_eq!(AutoCue::default(), AutoCue::Level(-36.0));
        let t = AutoCue::Level(-20.0).threshold().unwrap();
        assert!((t - 0.1).abs() < 1e-6);
        assert_eq!(AutoCue::Off.threshold(), None);
    }

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
        assert_eq!(first.from_library, 0);
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
        assert_eq!(again.from_library, first.fresh.len());
        assert!(again.fresh.is_empty());
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
