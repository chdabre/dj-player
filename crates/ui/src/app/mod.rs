//! The application around the screens: the window and its events (`platform.rs`), the
//! frame (`frame.rs`), screenshots (`capture.rs`), the player's port (`engine.rs`), what the UI
//! holds about the loaded track (`track.rs`), effects (`effects.rs`) and the dev keyboard
//! (`keymap.rs`); what the controls do is the player's (`engined::controls`). [`App`] owns
//! all of it.

mod capture;
pub mod effects;
pub mod engine;
mod frame;
pub mod keymap;
mod platform;
mod start;
pub mod track;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use browser::Browser;
use player_api::{DeckCommand, OnDeck, Port};
use soulseek::{Credentials, DOWNLOADS_DIR, Snapshot, Soulseek};
use winit::keyboard::ModifiersState;

use crate::args::{self, Args};
use crate::input::{ControlEvent, Nav};
use crate::render::Gfx;
use crate::render::scene::Scene;
use crate::screens::browse::ROWS;
use crate::screens::{Cx, Hits, ScreenKind, Screens};
#[cfg(feature = "standin")]
use crate::standin;
use crate::stats::FrameStats;
use crate::theme::Theme;
use crate::waves::WaveSettings;
use effects::{Effect, Effects};
use engine::DeckSnapshot;
use start::start_player;
use track::Track;

/// Who got the pointer when it went down: the device screen or the stand-in shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerOn {
    Device,
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    Shell,
}

pub struct App {
    /// The player, through its port: everything the UI asks of it and reads from it.
    player: Box<dyn Port>,
    /// The deck as of the start of the last frame: what the screen shows and input acts on.
    deck: DeckSnapshot,
    /// The track on the deck, as of the same read (and since the last load).
    on_deck: Option<OnDeck>,
    /// The loaded track, as the UI holds it.
    track: Track,
    /// The stick's folder tree, when there is a stick (`LIBRARY_DIR`).
    browser: Option<Browser>,
    /// The SoulSeek client, with a stick and an account (`SOULSEEK_USERNAME`,
    /// `SOULSEEK_PASSWORD`); it downloads into the stick's [`DOWNLOADS_DIR`].
    soulseek: Option<Soulseek>,
    /// What it had at the start of the frame: what the screen shows and input acts on.
    soulseek_seen: Option<Snapshot>,
    /// A `--search` query still to start.
    pending_search: Option<String>,
    /// Which screen is up, each screen's state, and the touch in progress.
    screens: Screens,
    /// What input asked the engine and the library to do, carried out after each event.
    effects: Effects,
    /// `--analyze-all` still to start.
    pending_analyze_all: bool,
    /// `--open` levels still to open, and `--menu` still to show.
    pending_open: Option<String>,
    pending_menu: bool,
    /// Wheel travel not yet turned into whole rows.
    wheel: f64,
    pointer_on: PointerOn,
    #[cfg(feature = "standin")]
    standin: Option<standin::Standin>,
    #[cfg(feature = "standin")]
    shell_scene: Scene,
    last_frame: Option<Instant>,
    fullscreen: bool,
    screenshot: Option<PathBuf>,
    run_for: Option<Duration>,
    started: Instant,
    gfx: Option<Gfx>,
    theme: Theme,
    scene: Scene,
    wave_settings: WaveSettings,
    hits: Hits,
    cursor: (f64, f64),
    modifiers: ModifiersState,
    debug: bool,
    stats: FrameStats,
    overlay: String,
    error: Option<String>,
}

impl App {
    /// Start the engine and the library as `args` ask, load the file if one was given, and
    /// set up the screens; on failure, the exit code (the reason is printed).
    pub fn start(args: Args) -> Result<Self, ExitCode> {
        let root = stick_root(&args)?;
        let (mut player, ask_scan) = start_player(&args, root.as_deref())?;
        load_startup(player.as_mut(), &args)?;
        let sr = player.output().sample_rate;
        let track = match args.file.as_deref() {
            Some(f) => Track::new(f, None, sr),
            None => Track::none(sr),
        };
        let deck = player.deck();
        let on_deck = player.on_deck();
        let soulseek = root
            .as_ref()
            .zip(Credentials::from_env())
            .map(|(r, c)| Soulseek::start(c, r.join(DOWNLOADS_DIR)));
        let browser = root.map(|r| {
            let mut b = Browser::new(r, ROWS);
            if let Some(s) = args.sort {
                b.set_sort(s);
            }
            if let Some(s) = args.source {
                b.set_source(s);
            }
            b
        });
        let screens = startup_screens(&args, ask_scan);
        Ok(App {
            pointer_on: PointerOn::Device,
            #[cfg(feature = "standin")]
            standin: args.standin.then(standin::Standin::new),
            #[cfg(feature = "standin")]
            shell_scene: Scene::default(),
            last_frame: None,
            deck,
            on_deck,
            track,
            player,
            browser,
            soulseek,
            soulseek_seen: None,
            // Applied once the library's listing is in (a library search needs it).
            pending_search: args.search.clone(),
            screens,
            effects: Vec::new(),
            pending_analyze_all: args.analyze_all,
            pending_open: args.open.clone(),
            pending_menu: args.menu,
            wheel: 0.0,
            fullscreen: args.fullscreen,
            screenshot: args.screenshot,
            run_for: args.run_for.map(Duration::from_secs_f64),
            started: Instant::now(),
            gfx: None,
            theme: args.theme,
            scene: Scene::default(),
            wave_settings: WaveSettings {
                mode: args.wave_mode,
                ..WaveSettings::default()
            },
            hits: Hits::default(),
            cursor: (0.0, 0.0),
            modifiers: ModifiersState::empty(),
            debug: true,
            stats: FrameStats::default(),
            overlay: String::new(),
            error: None,
        })
    }

    /// The exit code once the event loop has ended.
    pub fn finish(self) -> ExitCode {
        match self.error {
            Some(e) => {
                eprintln!("dj-ui: {e}");
                ExitCode::FAILURE
            }
            None => ExitCode::SUCCESS,
        }
    }

    fn send(&mut self, cmd: DeckCommand) {
        self.effects.push(Effect::Deck(cmd));
    }

    /// The screens, and what their handlers may use.
    fn split(&mut self) -> (&mut Screens, Cx<'_>) {
        let cx = Cx {
            deck: &self.deck,
            browser: self.browser.as_mut(),
            library: self.player.library(),
            soulseek: self.soulseek_seen.as_ref(),
            loaded: self.on_deck.as_ref().map(|d| d.file.as_path()),
            shown: self.track.last_shown.map(|(_, p)| p),
            effects: &mut self.effects,
        };
        (&mut self.screens, cx)
    }

    /// A control of the player did something (stand-in, keyboard, or the hardware).
    fn control(&mut self, e: ControlEvent) {
        for n in self.player.control(e) {
            self.nav(n);
        }
    }

    /// A browse control: the knob, BACK, TAG TRACK.
    fn nav(&mut self, n: Nav) {
        let (screens, mut cx) = self.split();
        screens.nav(n, &mut cx);
    }

    /// Carry out the effects input asked for, in order.
    fn apply_effects(&mut self) {
        for effect in std::mem::take(&mut self.effects) {
            match effect {
                Effect::Deck(cmd) => {
                    if !self.player.send(cmd) {
                        eprintln!("command ring full, dropped");
                    }
                }
                Effect::Load(path) => self.load(&path),
                Effect::Library(edit) => {
                    self.player.library_edit(edit);
                }
                Effect::AddFiles(id, files) => {
                    self.player.add_files(id, files);
                }
                Effect::NewPlaylist {
                    id,
                    parent,
                    name,
                    files,
                } => {
                    self.player.new_playlist(id, parent, name, files);
                }
                Effect::Scan => self.player.scan(),
                Effect::AnalyzeAll(files) => {
                    if !self.player.analyze_all(files) {
                        eprintln!("analyze all: nothing to analyse (or no library)");
                    }
                }
                Effect::StopAnalyzeAll => self.player.stop_analyze_all(),
                Effect::SoulseekSearch(query) => {
                    if let Some(s) = &self.soulseek {
                        s.search(&query);
                    }
                }
                Effect::Download(hit) => {
                    if let Some(s) = &self.soulseek {
                        s.download(&hit);
                    }
                }
            }
        }
    }

    /// Load `path` onto the deck and show the main screen.
    fn load(&mut self, path: &Path) {
        if let Err(e) = self.player.load(path) {
            eprintln!("can't load {}: {e}", path.display());
            return;
        }
        let tags = self.browser.as_ref().and_then(|b| b.info(path));
        self.track = Track::new(path, tags.as_ref(), self.player.output().sample_rate);
        self.on_deck = self.player.on_deck();
        self.screens.loaded();
        eprintln!("loaded {}", path.display());
    }

    /// The browse screen shows everything it waits for, and the CLI's own requests are done
    /// (for screenshots).
    fn browse_settled(&self) -> bool {
        let Some(b) = &self.browser else {
            return true;
        };
        self.pending_search.is_none()
            && self.pending_open.is_none()
            && self.screens.browse.settled(b, &self.player.library())
    }
}

/// The folder that stands in for the stick on the dev host (`LIBRARY_DIR`), if any: its root,
/// with our library in it. Without it a file is needed.
fn stick_root(args: &Args) -> Result<Option<PathBuf>, ExitCode> {
    let root = std::env::var_os("LIBRARY_DIR").map(PathBuf::from);
    if let Some(r) = &root
        && !r.is_dir()
    {
        eprintln!("dj-ui: LIBRARY_DIR {} is not a folder", r.display());
        return Err(ExitCode::from(2));
    }
    if args.file.is_none() && root.is_none() {
        eprintln!("dj-ui: no file given, and LIBRARY_DIR is not set");
        eprintln!("{}", args::USAGE);
        return Err(ExitCode::from(2));
    }
    Ok(root)
}

/// `--scan`, and the file with `--hot-cues`, `--at` and `--play`.
fn load_startup(player: &mut dyn Port, args: &Args) -> Result<(), ExitCode> {
    if args.scan {
        player.scan();
    }
    let Some(file) = &args.file else {
        return Ok(());
    };
    if let Err(e) = player.load(file) {
        eprintln!("dj-ui: {}: {e}", file.display());
        return Err(ExitCode::FAILURE);
    }
    let sr = f64::from(player.output().sample_rate);
    for (i, &at) in args.hot_cues.iter().enumerate().take(player_api::HOT_CUES) {
        player.send(DeckCommand::Seek(at * sr));
        player.send(DeckCommand::HotCue(i));
    }
    if let Some(at) = args.at {
        player.send(DeckCommand::Seek(at * sr));
    }
    if args.play {
        player.send(DeckCommand::Play);
    }
    Ok(())
}

/// The screen to start on, and the screens' state as the command line sets it up.
fn startup_screens(args: &Args, ask_scan: bool) -> Screens {
    let mut screens = Screens {
        screen: if args.perform.is_some() {
            ScreenKind::Performance
        } else if args.info_screen {
            ScreenKind::Info
        } else if args.browse || args.file.is_none() {
            ScreenKind::Browse
        } else {
            ScreenKind::Main
        },
        perform: args.perform.unwrap_or_default(),
        ..Screens::default()
    };
    screens.browse.keyboard = args.search.is_some();
    screens.browse.side = args.info;
    screens.browse.ask_scan = ask_scan && !args.scan;
    screens
}
