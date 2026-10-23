//! One frame: the deck and the track read, the browser kept up with the library, the
//! screen built and rendered (or captured), and the timing reported.

use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Browser;
use player_api::Persistence;
use soulseek::Soulseek;

use super::App;
use super::capture::write_png;
use crate::render;
use crate::screens::browse::{BrowseView, InfoView};
use crate::screens::info::{self, OnDeck};
use crate::screens::{self, SaveState, ScreenKind, View};
#[cfg(feature = "standin")]
use crate::standin;

/// The CLI's startup requests that wait for the library: ANALYZE ALL, and the MENU.
#[derive(Default)]
struct Startup {
    analyze: bool,
    menu: bool,
}

impl App {
    /// Draw one frame; true when the app should exit (a screenshot was taken).
    pub(super) fn frame(&mut self) -> bool {
        let now = Instant::now();
        let position = self.advance(now);
        let startup = self.sync_library();
        self.start_requests(startup);
        // Judged before the scene is built, so a screenshot shows everything it waited for.
        let ready = self.ready();
        let build = self.build_scene(position);
        self.present(now, build, ready)
    }

    /// The controls held since the last frame act, the deck is read, and the playhead to show
    /// is worked out (smoothed, or where the finger has the track); the track's waveform is
    /// kept up with the deck's.
    fn advance(&mut self, now: Instant) -> f64 {
        let dt = self
            .last_frame
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f64());
        self.last_frame = Some(now);
        self.player.tick_controls(dt);
        self.soulseek_seen = self.soulseek.as_ref().map(Soulseek::snapshot);
        // Held controls act before the deck is read, as a press would.
        self.apply_effects();
        self.deck = self.player.deck();
        self.on_deck = self.player.on_deck();
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
        if let Some(d) = &self.on_deck {
            self.track.update_wave(d.waveform.as_ref());
        }
        position
    }

    /// The browser takes the library's latest playlists and listing; once the library has
    /// said what it has, the CLI's `--open`, `--search` and `--analyze-all` start.
    fn sync_library(&mut self) -> Startup {
        let mut startup = Startup::default();
        let Some(b) = self.browser.as_mut() else {
            return startup;
        };
        let library = self.player.library();
        if let Some(p) = library.playlists {
            b.set_playlists(Arc::clone(p));
        }
        if let Some(l) = library.listing {
            b.set_listing(Arc::clone(l));
            // `--open` once the library has said what it has (playlists included).
            if library.playlists.is_some()
                && let Some(path) = self.pending_open.take()
            {
                open_levels(b, &path);
                startup.menu = std::mem::take(&mut self.pending_menu);
            }
            if std::mem::take(&mut self.pending_analyze_all) {
                startup.analyze = true;
            }
            // `--search` starts once the library has said what it has.
            if let Some(q) = self.pending_search.take() {
                b.search_begin();
                b.set_query(&q);
            }
        }
        b.refresh();
        startup
    }

    /// Start what the CLI asked for once the library was in, and carry out what is pending.
    fn start_requests(&mut self, startup: Startup) {
        let (screens, mut cx) = self.split();
        if startup.analyze {
            screens.browse.analyze_all(&mut cx);
        }
        if startup.menu {
            screens.browse.open_menu(&mut cx);
        }
        // Before the screen is built, so it shows what they started (a scan, ANALYZE ALL).
        self.apply_effects();
    }

    /// A screenshot can be taken: the track is decoded, analysed and its waveform complete,
    /// and the browse screen shows everything it waits for.
    fn ready(&self) -> bool {
        self.on_deck
            .as_ref()
            .is_none_or(|d| d.analysed && d.decoded && self.track.wave_complete)
            && (self.screens.screen != ScreenKind::Browse || self.browse_settled())
    }

    /// Build the device screen (and the stand-in) into their scenes; returns how long it took.
    fn build_scene(&mut self, position: f64) -> Duration {
        let mut wave_style = self.theme.waveform.style;
        wave_style.track_gain = self.track.gain();
        let browse = self.browse_view();
        let info = self.info_view();
        let view = View {
            deck: &self.deck,
            title: &self.track.title,
            position,
            zoom_beats: self.screens.deck.zoom_beats(),
            pressed: self.screens.pressed,
            debug: self.debug.then_some(self.overlay.as_str()),
            screen: self.screens.screen,
            perf: self.screens.perform,
            save: self.save_state(),
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
        build_start.elapsed()
    }

    /// The browse screen's view, when it is up.
    fn browse_view(&self) -> Option<BrowseView> {
        if self.screens.screen != ScreenKind::Browse {
            return None;
        }
        let library = self.player.library();
        let loaded = self.on_deck.as_ref().map(|d| d.file.as_path());
        self.browser.as_ref().map(|b| {
            self.screens
                .browse
                .view(b, &library, self.soulseek_seen.as_ref(), loaded)
        })
    }

    /// The INFO screen's view, when it is up.
    fn info_view(&self) -> Option<InfoView> {
        let loaded = self.on_deck.as_ref();
        (self.screens.screen == ScreenKind::Info).then(|| {
            info::view(
                self.browser.as_ref(),
                loaded
                    .zip(self.track.tags.as_ref())
                    .map(|(d, tags)| OnDeck {
                        path: &d.file,
                        bpm: d.bpm,
                        key: d.key.clone(),
                        tags,
                    }),
            )
        })
    }

    /// What the status band says about the library.
    fn save_state(&self) -> Option<SaveState> {
        self.player.library().persistence.map(|p| match p {
            Persistence::Saved => SaveState::Saved,
            Persistence::Pending => SaveState::Saving,
            Persistence::Unsaved { .. } => SaveState::Unsaved,
        })
    }

    /// Render the scenes (or capture them, once `ready` or after 30 s, and exit) and report
    /// the timing; true when the app should exit.
    fn present(&mut self, now: Instant, build: Duration, ready: bool) -> bool {
        #[cfg(feature = "standin")]
        let shell = self.standin.as_ref().map(|_| render::ShellFrame {
            scene: &self.shell_scene,
            size: standin::SIZE,
            display: {
                let d = standin::DISPLAY;
                (d.x, d.y, d.w, d.h)
            },
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
            let load = self.player.load_meter();
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
        let controls = self.player.controls();
        standin::ShellView {
            playing: d.playing,
            reverse: d.reverse,
            keylock: d.keylock,
            quantize: d.quantize > 0.0,
            looping: d.looping,
            hot_cues: d.hot_cues,
            rate: d.rate,
            position: d.position / d.sample_rate,
            range: controls.range,
            vinyl: controls.vinyl,
            jog: controls.jog,
        }
    }
}

/// `--open`: open the levels named in `path` (`PLAYLISTS/Peak`), one after the other.
fn open_levels(b: &mut Browser, path: &str) {
    for name in path.split('/').filter(|n| !n.is_empty()) {
        match b.level().entries.iter().position(|e| e.name == name) {
            Some(i) => {
                b.select(i);
                b.enter();
            }
            None => eprintln!("--open: no {name:?} here"),
        }
    }
}
