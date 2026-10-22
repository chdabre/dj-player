//! One frame: the deck and the track read, the browser kept up with the library, the
//! screen built and rendered (or captured), and the timing reported.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engined::Persistence;

use super::App;
use super::capture::write_png;
use super::engine::{DeckSnapshot, LibraryView};
use crate::render;
use crate::screens::info::{self, OnDeck};
use crate::screens::{self, SaveState, ScreenKind, View};
#[cfg(feature = "standin")]
use crate::standin;

impl App {
    /// Draw one frame; true when the app should exit (a screenshot was taken).
    pub(super) fn frame(&mut self) -> bool {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f64());
        self.last_frame = Some(now);
        self.surface.tick(dt, &mut self.effects);
        // Held controls act before the deck is read, as a press would.
        self.apply_effects();
        self.deck = DeckSnapshot::read(&self.player);
        let sr = self.deck.sample_rate;
        let speed = self.deck.speed();
        let mut position = self.track.clock.update(now, self.deck.position, speed);
        if let Some(shown) = self.screens.deck.scrubbed() {
            position = shown;
        }
        if let Some((t, p)) = self.track.last_shown
            && speed != 0.0
        {
            let expected = (now - t).as_secs_f64() * speed * sr;
            self.stats
                .record_playhead((position - p - expected) / sr * 1e3);
        }
        self.track.last_shown = Some((now, position));
        if let Some(d) = self.player.loaded() {
            self.track.update_wave(d.waveform());
        }
        let mut wave_style = self.theme.waveform.style;
        wave_style.track_gain = self.track.gain();
        let mut pending_analyze = false;
        let mut pending_menu = false;
        if let Some(b) = self.browser.as_mut() {
            if let Some(p) = self.player.playlists() {
                b.set_playlists(Arc::clone(p));
            }
            if let Some(l) = self.player.listing() {
                b.set_listing(Arc::clone(l));
                // `--open` once the library has said what it has (playlists included).
                if self.player.playlists().is_some()
                    && let Some(path) = self.pending_open.take()
                {
                    for name in path.split('/').filter(|n| !n.is_empty()) {
                        match b.level().entries.iter().position(|e| e.name == name) {
                            Some(i) => {
                                b.select(i);
                                b.enter();
                            }
                            None => eprintln!("--open: no {name:?} here"),
                        }
                    }
                    pending_menu = std::mem::take(&mut self.pending_menu);
                }
                if std::mem::take(&mut self.pending_analyze_all) {
                    pending_analyze = true;
                }
                // `--search` starts once the library has said what it has.
                if let Some(q) = self.pending_search.take() {
                    b.search_begin();
                    b.set_query(&q);
                }
            }
            b.refresh();
        }
        let (screens, mut cx) = self.split();
        if pending_analyze {
            screens.browse.analyze_all(&mut cx);
        }
        if pending_menu {
            screens.browse.open_menu(&mut cx);
        }
        // Before the screen is built, so it shows what they started (a scan, ANALYZE ALL).
        self.apply_effects();
        // Judged before the scene is built, so a screenshot shows everything it waited for.
        let ready = self.player.loaded().is_none_or(|d| {
            d.analysed.is_some() && d.source.is_complete() && self.track.wave_complete
        }) && (self.screens.screen != ScreenKind::Browse || self.browse_settled());
        let library = LibraryView::read(&self.player);
        let loaded = self.player.loaded();
        let browse = (self.screens.screen == ScreenKind::Browse)
            .then(|| {
                self.browser.as_ref().map(|b| {
                    self.screens
                        .browse
                        .view(b, &library, loaded.map(|d| d.file()))
                })
            })
            .flatten();
        let info = (self.screens.screen == ScreenKind::Info).then(|| {
            info::view(
                self.browser.as_ref(),
                loaded
                    .zip(self.track.tags.as_ref())
                    .map(|(d, tags)| OnDeck {
                        path: d.file(),
                        bpm: d.analysed.as_ref().and_then(|a| a.bpm),
                        key: d.analysed.as_ref().and_then(|a| a.key.clone()),
                        tags,
                    }),
            )
        });
        let view = View {
            deck: &self.deck,
            title: &self.track.title,
            position,
            zoom_beats: self.screens.deck.zoom_beats(),
            pressed: self.screens.pressed,
            debug: self.debug.then_some(self.overlay.as_str()),
            screen: self.screens.screen,
            perf: self.screens.perform,
            save: self.player.persistence().map(|p| match p {
                Persistence::Saved => SaveState::Saved,
                Persistence::Pending => SaveState::Saving,
                Persistence::Unsaved { .. } => SaveState::Unsaved,
            }),
            browse: browse.as_ref(),
            info: info.as_ref(),
            wave: (!self.track.wave.is_empty()).then_some(screens::Wave {
                entries: &self.track.wave,
                entries_per_second: f64::from(waveform::compute::ENTRIES_PER_SECOND),
                settings: self.wave_settings,
                style: wave_style,
            }),
        };
        let build_start = Instant::now();
        self.hits = screens::build(
            &view,
            &self.theme,
            &mut self.track.overview,
            &mut self.scene,
        );
        #[cfg(feature = "standin")]
        let shell_view = self.shell_view();
        #[cfg(feature = "standin")]
        if let Some(s) = self.standin.as_mut() {
            s.build(&shell_view, &self.theme, &mut self.shell_scene);
        }
        let build = build_start.elapsed();
        #[cfg(feature = "standin")]
        let shell = self.standin.as_ref().map(|_| render::ShellFrame {
            scene: &self.shell_scene,
            size: standin::SIZE,
            display: standin::DISPLAY,
        });
        #[cfg(not(feature = "standin"))]
        let shell: Option<render::ShellFrame> = None;

        let Some(gfx) = &mut self.gfx else {
            return false;
        };
        if let Some(path) = &self.screenshot
            && (ready || self.started.elapsed() > Duration::from_secs(30))
        {
            let (w, h, pixels) = gfx.screenshot(&self.scene, shell);
            match write_png(path, w, h, &pixels) {
                Ok(()) => eprintln!("wrote {}", path.display()),
                Err(e) => self.error = Some(format!("screenshot: {e}")),
            }
            return true;
        }
        if let Some(timing) = gfx.render(&self.scene, shell) {
            self.stats.record(now, build, timing);
        }
        if let Some(summary) = self.stats.take_if_due(now, Duration::from_secs(2)) {
            let load = self.player.load_meter().take();
            eprintln!(
                "{summary}  audio load avg {:.1} % peak {:.1} %",
                load.average * 100.0,
                load.peak * 100.0
            );
            self.overlay = summary;
        }
        self.run_for.is_some_and(|d| self.started.elapsed() >= d)
    }

    /// What the stand-in shows of the deck.
    #[cfg(feature = "standin")]
    fn shell_view(&self) -> standin::ShellView {
        let d = &self.deck;
        standin::ShellView {
            playing: d.playing,
            reverse: d.reverse,
            keylock: d.keylock,
            quantize: d.quantize > 0.0,
            looping: d.looping,
            hot_cues: d.hot_cues,
            rate: d.rate,
            position: d.position / d.sample_rate,
            range: self.surface.range(),
            vinyl: self.surface.vinyl(),
            jog: self.surface.jog(),
        }
    }
}
