//! [`Player`] as the UI's [`Port`]: the player runs in the UI's process, so every call is a
//! direct one, and the deck is read straight from its atomics.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use player_api::{
    ControlEvent, ControlsView, DeckCommand, DeckSnapshot, Edit, LibraryState, LoadReport, Nav,
    OnDeck, Output, PlaylistId, Port,
};

use super::Player;

impl Port for Player {
    fn output(&self) -> Output {
        Output {
            sample_rate: self.format.sample_rate,
            channels: self.format.channels,
            device: self.device_name().to_owned(),
        }
    }

    fn send(&mut self, cmd: DeckCommand) -> bool {
        Player::send(self, cmd)
    }

    fn control(&mut self, e: ControlEvent) -> Vec<Nav> {
        Player::control(self, e)
    }

    fn tick_controls(&mut self, dt: f64) {
        Player::tick_controls(self, dt);
    }

    fn controls(&self) -> ControlsView {
        let c = Player::controls(self);
        ControlsView {
            range: c.range(),
            vinyl: c.vinyl(),
            jog: c.jog(),
        }
    }

    fn deck(&self) -> DeckSnapshot {
        let st = self.state();
        let loaded = self.loaded();
        let analysed = loaded.and_then(|d| d.analysed.as_ref());
        DeckSnapshot {
            sample_rate: f64::from(self.format.sample_rate),
            position: st.position.load(),
            shadow_position: st.shadow_position.load(),
            length: st.length.load(),
            playing: st.playing.load(Ordering::Relaxed),
            reverse: st.reverse.load(Ordering::Relaxed),
            rate: st.rate.load(),
            slip_enabled: st.slip_enabled.load(Ordering::Relaxed),
            slip_active: st.slip_active.load(Ordering::Relaxed),
            keylock: st.keylock_enabled.load(Ordering::Relaxed),
            quantize: st.quantize.load(),
            cue: st.cue.load(),
            hot_cues: st.hot_cues.load(Ordering::Relaxed),
            hot_cue_positions: std::array::from_fn(|i| st.hot_cue_positions[i].load()),
            looping: st.looping.load(Ordering::Relaxed),
            loop_start: st.loop_start.load(),
            loop_end: st.loop_end.load(),
            grid: analysed.and_then(|a| a.grid.clone()),
            key: analysed.and_then(|a| a.key.clone()),
            analysing: loaded.is_some() && analysed.is_none(),
            loaded_fraction: loaded.map_or(0.0, |d| {
                d.source.chunks_ready() as f64 / d.source.chunk_count().max(1) as f64
            }),
        }
    }

    fn on_deck(&self) -> Option<OnDeck> {
        self.loaded().map(|d| OnDeck {
            file: d.file().to_path_buf(),
            waveform: Arc::clone(d.waveform()) as _,
            bpm: d.analysed.as_ref().and_then(|a| a.bpm),
            key: d.analysed.as_ref().and_then(|a| a.key.clone()),
            analysed: d.analysed.is_some(),
            decoded: d.source.is_complete(),
        })
    }

    fn load(&mut self, file: &Path) -> Result<(), String> {
        Player::load(self, file).map_err(|e| e.to_string())
    }

    fn load_arriving(&mut self, file: &Path, part: &Path, len: u64) -> Result<(), String> {
        Player::load_arriving(self, file, part, len).map_err(|e| e.to_string())
    }

    fn arrived(&mut self, file: &Path, complete: bool) -> Option<String> {
        Player::arrived(self, file, complete)
    }

    fn poll(&mut self) -> Vec<String> {
        Player::poll(self)
    }

    fn load_meter(&self) -> LoadReport {
        Player::load_meter(self).take()
    }

    fn memory(&mut self) -> String {
        Player::memory(self)
    }

    fn library(&self) -> LibraryState<'_> {
        LibraryState {
            listing: self.listing(),
            playlists: self.playlists(),
            batch: self.batch_progress(),
            scan: self.scan_progress(),
            persistence: self.persistence(),
        }
    }

    fn library_edit(&mut self, edit: Edit) -> bool {
        Player::library_edit(self, edit)
    }

    fn add_files(&mut self, playlist: PlaylistId, files: Vec<PathBuf>) -> bool {
        Player::add_files(self, playlist, files)
    }

    fn import(&mut self, files: Vec<PathBuf>) -> bool {
        Player::import(self, files)
    }

    fn new_playlist(
        &mut self,
        id: PlaylistId,
        parent: Option<PlaylistId>,
        name: String,
        files: Vec<PathBuf>,
    ) -> bool {
        Player::new_playlist(self, id, parent, name, files)
    }

    fn scan(&mut self) {
        Player::scan(self);
    }

    fn analyze_all(&mut self, files: Vec<PathBuf>) -> bool {
        Player::analyze_all(self, files)
    }

    fn stop_analyze_all(&mut self) {
        Player::stop_analyze_all(self);
    }
}
