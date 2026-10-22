//! `dj-ui <file>`: the UI shell spike. The main playback screen at the panel's 1024×600,
//! drawn with raw wgpu and glyphon, driving one deck of the real engine in-process.
//!
//! Touch or click: SLIP, CALL ◀ ▶ (beat jump ∓4, or loop ½X / 2X in a loop), ZOOM, and the
//! overview to seek. Keys as in `engined`, plus a real CUE hold (the window sees key
//! releases): space play/pause, c cue, 1–8 hot cues (shift: delete), i/o/l loop,
//! b 4-beat loop, [ ] loop ½X / 2X, , . beat jump, ←/→ seek ±5 s, +/- rate, 0 reset,
//! k keylock, r reverse, s slip, m memory cue, z quantize, w waveform mode (3-band / RGB /
//! single colour), p performance screen (hot cue / beat jump pads, beat loops), d timing
//! overlay, f fullscreen, q quit.
//!
//! Browsing: `LIBRARY_DIR=<dir>` makes a folder act as the root of a USB stick: the browse
//! screen lists its folders and tracks, or its library's tracks (`screens/browse/`), and its
//! library is `<dir>/DJPLAYER` unless `--library`/`--no-library` say otherwise. A stick with
//! no library asks once whether to scan it. Without a file the app starts on the browse
//! screen. Tab (or the BROWSE tab, or the stand-in's knob) opens it; ↑/↓ or the wheel move,
//! Enter loads or opens, Esc / Backspace goes up, `/` searches (type the query; Esc ends it),
//! `i` (or the INFO tab) shows the selected track's details.
//!
//! With the `standin` feature (on by default) the window also shows the hardware stand-in:
//! the player's physical controls drawn around the device screen (`standin.rs`), as a
//! separate layer. `--no-standin` shows the device screen alone, as on the Pi.
//!
//! See docs/spikes/ui-shell.md for what this measures and what it found.

mod app;
mod clock;
// Without the stand-in only the dev keyboard reports controls, until the RP2040 reader exists.
#[cfg_attr(not(feature = "standin"), allow(dead_code))]
mod input;
mod kit;
mod playlists;
mod render;
mod screens;
#[cfg(feature = "standin")]
mod standin;
mod stats;
mod theme;
mod waves;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_core::DeckCommand;
use engine_io::{CpalBackend, StreamRequest};
use engined::browse::{Browser, Entered, Kind, Sort, Source};
use engined::{AutoCue, LibraryChoice, Options, Persistence, Player};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, KeyEvent, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::app::effects::{Effect, Effects};
use crate::app::engine::DeckSnapshot;
use crate::app::keymap::{self, KeyContext, Shortcut};
use crate::app::surface::Surface;
use crate::app::track::{Track, track_title};
use crate::input::{ControlEvent, Nav};
use crate::kit::Rect;
use crate::playlists::{Action, Editing, Modal};
use crate::render::Gfx;
use crate::render::scene::Scene;
use crate::screens::browse::{self, BrowseView, InfoView, ModalView, ROW_H, ROWS, Row, RowKind};
use crate::screens::deck::OVERVIEW;
use crate::screens::{HitBox, PerfState, SaveState, ScreenKind, Target, View, perform};
use crate::stats::FrameStats;
use crate::theme::Theme;
use crate::waves::WaveSettings;
use library::PlaylistKind;
use waveform::display::Mode;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const USAGE: &str = "usage: dj-ui <file> [--device <name>] [--buffer <frames>] [--fullscreen] [--no-standin] \
                     [--theme <name>] [--wave-theme <name>] [--wave-mode 3band|rgb|single] [--perform] [--pad-mode hotcue|beatjump|delete] \
                     [--hot-cues <s,s,…>] [--at <seconds>] [--play] [--run-for <seconds>] \
                     [--screenshot <out.png>] [--library <dir> | --no-library] [--auto-cue off|<dB>] \
                     [--browse] [--source usb|library] [--sort name|title|artist|bpm|key|time] \
                     [--search <query>] [--scan] [--info] [--info-screen] [--analyze-all] \
                     [--source usb|library|taglist] [--open <a/b>] [--menu]\n\
                     LIBRARY_DIR=<dir> browses <dir> as a USB stick's root; the file is then optional.";
const ZOOM_STEPS: [f64; 4] = [8.0, 16.0, 32.0, 64.0];

struct Args {
    file: Option<PathBuf>,
    request: StreamRequest,
    fullscreen: bool,
    /// Draw the hardware stand-in around the device screen (feature `standin`).
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    standin: bool,
    screenshot: Option<PathBuf>,
    at: Option<f64>,
    play: bool,
    run_for: Option<f64>,
    theme: Theme,
    wave_mode: Mode,
    /// `--library` / `--no-library`, if given.
    library: Option<LibraryChoice>,
    auto_cue: AutoCue,
    perform: Option<PerfState>,
    /// Start on the browse screen.
    browse: bool,
    /// Browse screen set-up, for screenshots.
    source: Option<Source>,
    sort: Option<Sort>,
    search: Option<String>,
    /// Answer "scan this stick?" with yes (or scan anyway).
    scan: bool,
    /// Start with the browser's info panel up.
    info: bool,
    /// Start on the INFO screen.
    info_screen: bool,
    /// Start ANALYZE ALL once the library has said what it has.
    analyze_all: bool,
    /// Open these levels by name (`PLAYLISTS/Peak`), then the MENU if asked.
    open: Option<String>,
    menu: bool,
    /// Hot cues to set at start (seconds), for screenshots and demos.
    hot_cues: Vec<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut file = None;
    let mut request = StreamRequest::default();
    let (mut fullscreen, mut screenshot, mut at) = (false, None, None);
    let mut standin = cfg!(feature = "standin");
    let (mut play, mut run_for) = (false, None);
    let mut theme = Theme::default();
    let mut wave_theme: Option<String> = None;
    let mut wave_mode = Mode::ThreeBand;
    let mut library = None;
    let mut browse = false;
    let (mut source, mut sort, mut search, mut scan) = (None, None, None, false);
    let (mut info, mut info_screen, mut analyze_all) = (false, false, false);
    let (mut open, mut menu) = (None, false);
    let mut auto_cue = AutoCue::default();
    let mut perform: Option<PerfState> = None;
    let mut hot_cues: Vec<f64> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--device" => request.device = Some(args.next().ok_or("--device needs a name")?),
            "--buffer" => {
                let n = args.next().ok_or("--buffer needs a frame count")?;
                request.buffer_frames = Some(n.parse().map_err(|_| format!("bad buffer {n:?}"))?);
            }
            "--fullscreen" => fullscreen = true,
            "--no-standin" => standin = false,
            "--screenshot" => {
                screenshot = Some(PathBuf::from(
                    args.next().ok_or("--screenshot needs a path")?,
                ))
            }
            "--at" => {
                let s = args.next().ok_or("--at needs seconds")?;
                at = Some(s.parse().map_err(|_| format!("bad time {s:?}"))?);
            }
            "--wave-theme" => {
                let name = args.next().ok_or("--wave-theme needs a name")?;
                wave_theme = Some(name);
            }
            "--wave-mode" => {
                let name = args
                    .next()
                    .ok_or("--wave-mode needs 3band, rgb or single")?;
                wave_mode = Mode::ALL
                    .into_iter()
                    .find(|m| m.name() == name)
                    .ok_or(format!(
                        "unknown waveform mode {name:?} (3band, rgb, single)"
                    ))?;
            }
            "--perform" => {
                perform.get_or_insert_with(PerfState::default);
            }
            "--pad-mode" => {
                let p = perform.get_or_insert_with(PerfState::default);
                match args.next().as_deref() {
                    Some("hotcue") => {}
                    Some("beatjump") => p.mode = perform::PadMode::BeatJump,
                    Some("delete") => p.delete_armed = true,
                    other => {
                        return Err(format!("bad pad mode {other:?} (hotcue, beatjump, delete)"));
                    }
                }
            }
            "--hot-cues" => {
                let list = args
                    .next()
                    .ok_or("--hot-cues needs seconds, comma-separated")?;
                hot_cues = list
                    .split(',')
                    .map(|x| {
                        x.trim()
                            .parse::<f64>()
                            .map_err(|_| format!("bad time {x:?}"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--play" => play = true,
            "--theme" => {
                let name = args.next().ok_or("--theme needs a name")?;
                theme = Theme::named(&name).ok_or_else(|| {
                    let names: Vec<_> = Theme::all().iter().map(|t| t.name).collect();
                    format!("unknown theme {name:?} (themes: {})", names.join(", "))
                })?;
            }
            "--library" => {
                library = Some(LibraryChoice::Folder(PathBuf::from(
                    args.next().ok_or("--library needs a folder")?,
                )))
            }
            "--no-library" => library = Some(LibraryChoice::Ram),
            "--browse" => browse = true,
            "--source" => {
                source = Some(match args.next().as_deref() {
                    Some("usb") => Source::Folders,
                    Some("library") => Source::Library,
                    Some("taglist") => Source::TagList,
                    other => return Err(format!("bad source {other:?} (usb, library)")),
                });
                browse = true;
            }
            "--sort" => {
                let name = args.next().unwrap_or_default();
                sort = Some(
                    Sort::ALL
                        .into_iter()
                        .find(|s| s.label().eq_ignore_ascii_case(&name))
                        .ok_or(format!("bad sort {name:?}"))?,
                );
                browse = true;
            }
            "--info" => {
                info = true;
                browse = true;
            }
            "--info-screen" => info_screen = true,
            "--open" => {
                open = Some(args.next().ok_or("--open needs names, /-separated")?);
                browse = true;
            }
            "--menu" => {
                menu = true;
                open.get_or_insert_with(String::new);
                browse = true;
            }
            "--analyze-all" => {
                analyze_all = true;
                source = Some(Source::Library);
                browse = true;
            }
            "--scan" => {
                scan = true;
                browse = true;
            }
            "--search" => {
                search = Some(args.next().ok_or("--search needs a query")?);
                browse = true;
            }
            "--auto-cue" => {
                let v = args.next().ok_or("--auto-cue needs off or a level in dB")?;
                auto_cue = AutoCue::parse(&v).ok_or(format!("bad auto cue {v:?}"))?;
            }
            "--run-for" => {
                let s = args.next().ok_or("--run-for needs seconds")?;
                run_for = Some(s.parse().map_err(|_| format!("bad time {s:?}"))?);
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => return Err("only one file, please".into()),
        }
    }
    if let Some(name) = wave_theme {
        let names: Vec<_> = waveform::display::Theme::ALL
            .iter()
            .map(|t| t.name)
            .collect();
        theme = theme.with_waveform_theme(&name).ok_or_else(|| {
            format!(
                "unknown waveform theme {name:?} (themes: {})",
                names.join(", ")
            )
        })?;
    }
    Ok(Args {
        file,
        request,
        fullscreen,
        standin,
        screenshot,
        at,
        play,
        run_for,
        theme,
        wave_mode,
        library,
        auto_cue,
        perform,
        browse,
        source,
        sort,
        search,
        scan,
        info,
        info_screen,
        analyze_all,
        open,
        menu,
        hot_cues,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("dj-ui: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    // On the dev host a folder stands in for the stick: its root, with our library in it.
    let root = std::env::var_os("LIBRARY_DIR").map(PathBuf::from);
    if let Some(r) = &root
        && !r.is_dir()
    {
        eprintln!("dj-ui: LIBRARY_DIR {} is not a folder", r.display());
        return ExitCode::from(2);
    }
    if args.file.is_none() && root.is_none() {
        eprintln!("dj-ui: no file given, and LIBRARY_DIR is not set");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let library = match (args.library.clone(), &root) {
        (Some(choice), _) => choice,
        (None, Some(r)) => LibraryChoice::Folder(r.join(engined::persist::LIBRARY_DIR)),
        (None, None) => LibraryChoice::Auto,
    };
    // A stick with no library is asked about once, before mounting makes one.
    let ask_scan = args.library.is_none()
        && root.as_ref().is_some_and(|r| {
            !r.join(engined::persist::LIBRARY_DIR)
                .join("library.db")
                .exists()
        });
    let backend = CpalBackend::new();
    let options = Options {
        library,
        auto_cue: args.auto_cue,
        ..Options::default()
    };
    let mut player = match Player::start(&backend, &args.request, options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dj-ui: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "{} Hz, {} ch, buffer {}, {}",
        player.format.sample_rate,
        player.format.channels,
        player
            .buffer_frames()
            .map_or("default".into(), |n| format!("{n} frames")),
        player.device_name()
    );
    if args.scan {
        player.scan();
    }
    if let Some(file) = &args.file {
        if let Err(e) = player.load(file) {
            eprintln!("dj-ui: {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
        let sr = f64::from(player.format.sample_rate);
        for (i, &at) in args.hot_cues.iter().enumerate().take(engine_core::HOT_CUES) {
            player.send(DeckCommand::Seek(at * sr));
            player.send(DeckCommand::HotCue(i));
        }
        if let Some(at) = args.at {
            player.send(DeckCommand::Seek(at * sr));
        }
        if args.play {
            player.send(DeckCommand::Play);
        }
    }
    let sr = player.format.sample_rate;
    let track = match args.file.as_deref() {
        Some(f) => Track::new(f, None, sr),
        None => Track::none(sr),
    };
    let deck = DeckSnapshot::read(&player);
    let mut browser = root.map(|r| Browser::new(r, ROWS));
    if let Some(b) = browser.as_mut() {
        if let Some(s) = args.sort {
            b.set_sort(s);
        }
        if let Some(s) = args.source {
            b.set_source(s);
        }
    }
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dj-ui: {e}");
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        pointer_on: PointerOn::Device,
        #[cfg(feature = "standin")]
        standin: args.standin.then(standin::Standin::new),
        #[cfg(feature = "standin")]
        shell_scene: Scene::default(),
        last_frame: None,
        scrub: None,
        deck,
        track,
        player,
        browser,
        // Applied once the library's listing is in (a library search needs it).
        pending_search: args.search.clone(),
        keyboard: args.search.is_some(),
        side: args.info,
        editing: Editing::default(),
        effects: Vec::new(),
        surface: Surface::new(sr),
        shift: browse::Shift::Off,
        pending_analyze_all: args.analyze_all,
        pending_open: args.open.clone(),
        pending_menu: args.menu,
        unanalysed_cache: std::cell::RefCell::new(None),
        ask_scan: ask_scan && !args.scan,
        list_drag: None,
        wheel: 0.0,
        fullscreen: args.fullscreen,
        screenshot: args.screenshot,
        run_for: args.run_for.map(Duration::from_secs_f64),
        started: Instant::now(),
        gfx: None,
        theme: args.theme,
        scene: Scene::default(),
        screen: if args.perform.is_some() {
            ScreenKind::Performance
        } else if args.info_screen {
            ScreenKind::Info
        } else if args.browse || args.file.is_none() {
            ScreenKind::Browse
        } else {
            ScreenKind::Main
        },
        perf: args.perform.unwrap_or_default(),
        wave_settings: WaveSettings {
            mode: args.wave_mode,
            ..WaveSettings::default()
        },
        hits: Vec::new(),
        pressed: None,
        cursor: (0.0, 0.0),
        modifiers: ModifiersState::empty(),
        zoom: 1,
        debug: true,
        stats: FrameStats::default(),
        overlay: String::new(),
        error: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("dj-ui: {e}");
        return ExitCode::FAILURE;
    }
    match app.error {
        Some(e) => {
            eprintln!("dj-ui: {e}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}

#[derive(Debug, Clone, Copy)]
struct Scrub {
    last_x: f32,
    frames_per_px: f64,
    shown: f64,
}

/// Who got the pointer when it went down: the device screen or the stand-in shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerOn {
    Device,
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    Shell,
}

/// A finger on the browse list: where it went down (design y, row), how far the list has
/// scrolled under it, and whether it moved enough to be a drag rather than a tap.
#[derive(Debug, Clone, Copy)]
struct ListDrag {
    y: f32,
    row: usize,
    scrolled: isize,
    moved: bool,
}

/// Finger travel (design pixels) before a touch on the list counts as a drag.
const DRAG_SLOP: f32 = 8.0;

struct App {
    player: Player,
    /// The deck as of the start of the last frame: what the screen shows and input acts on.
    deck: DeckSnapshot,
    /// The loaded track, as the UI holds it.
    track: Track,
    /// The stick's folder tree, when there is a stick (`LIBRARY_DIR`).
    browser: Option<Browser>,
    /// A `--search` query still to start.
    pending_search: Option<String>,
    /// The search keyboard is up.
    keyboard: bool,
    /// The browser's info panel is up beside the list.
    side: bool,
    /// Playlist editing on the browse screen: a modal over the list (MENU, ADD TO PLAYLIST,
    /// naming, a confirmation), or an entry the knob is moving.
    editing: Editing,
    /// What input asked the engine and the library to do, carried out after each event.
    effects: Effects,
    /// What the player's controls do (fader range, jog, held buttons), whoever presses them.
    surface: Surface,
    /// The on-screen keyboard's shift.
    shift: browse::Shift,
    /// `--analyze-all` still to start.
    pending_analyze_all: bool,
    /// `--open` levels still to open, and `--menu` still to show.
    pending_open: Option<String>,
    pending_menu: bool,
    /// The unanalysed track count, and the listing it was counted from.
    unanalysed_cache: std::cell::RefCell<Option<(Arc<engined::persist::Listing>, usize)>>,
    /// Ask whether to scan this stick (it had no library).
    ask_scan: bool,
    list_drag: Option<ListDrag>,
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
    screen: ScreenKind,
    perf: PerfState,
    wave_settings: WaveSettings,
    hits: Vec<HitBox>,
    /// Touched target, held until release; the overview tracks the drag.
    pressed: Option<Target>,
    cursor: (f64, f64),
    modifiers: ModifiersState,
    zoom: usize,
    debug: bool,
    /// The hand on the zoom waveform: where the finger was (design x), the frames one design
    /// pixel covers, and the playhead the finger has moved the track to.
    scrub: Option<Scrub>,
    stats: FrameStats,
    overlay: String,
    error: Option<String>,
}

impl App {
    fn send(&mut self, cmd: DeckCommand) {
        self.effects.push(Effect::Deck(cmd));
    }

    /// A control of the player did something (stand-in, keyboard, or the hardware).
    fn control(&mut self, e: ControlEvent) {
        let mut navs = Vec::new();
        self.surface
            .event(e, &self.deck, &mut self.effects, &mut navs);
        for n in navs {
            self.nav(n);
        }
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
            }
        }
    }

    /// The pointer went down at window pixels (`x`, `y`); `alt` = right button or shift.
    fn pointer_down(&mut self, x: f64, y: f64, alt: bool) {
        #[cfg(feature = "standin")]
        if self.standin.is_some()
            && let Some(gfx) = &self.gfx
            && !gfx.on_device(x, y)
            && let Some((sx, sy)) = gfx.to_shell(x, y)
        {
            self.pointer_on = PointerOn::Shell;
            let alt = alt || self.modifiers.shift_key();
            let events = self
                .standin
                .as_mut()
                .map(|s| s.press(sx, sy, alt))
                .unwrap_or_default();
            for e in events {
                self.control(e);
            }
            return;
        }
        let _ = alt;
        self.pointer_on = PointerOn::Device;
        self.press(x, y);
    }

    fn pointer_move(&mut self, x: f64, y: f64) {
        #[cfg(feature = "standin")]
        if self.pointer_on == PointerOn::Shell {
            let point = self.gfx.as_ref().and_then(|g| g.to_shell(x, y));
            let events = match (point, self.standin.as_mut()) {
                (Some((sx, sy)), Some(s)) => s.drag(sx, sy),
                _ => Vec::new(),
            };
            for e in events {
                self.control(e);
            }
            return;
        }
        self.drag(x, y);
    }

    fn pointer_up(&mut self) {
        #[cfg(feature = "standin")]
        if self.pointer_on == PointerOn::Shell {
            let events = self
                .standin
                .as_mut()
                .map(|s| s.release())
                .unwrap_or_default();
            for e in events {
                self.control(e);
            }
            self.pointer_on = PointerOn::Device;
            return;
        }
        self.release();
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

    fn press(&mut self, x: f64, y: f64) {
        let Some(gfx) = &self.gfx else { return };
        let (dx, dy) = gfx.to_design(x, y);
        let Some(hit) = self.hits.iter().find(|h| h.contains(dx, dy)).copied() else {
            return;
        };
        self.pressed = Some(hit.target);
        let looping = self.deck.looping;
        match hit.target {
            Target::Perform => {
                self.screen = match self.screen {
                    ScreenKind::Performance => ScreenKind::Main,
                    _ => ScreenKind::Performance,
                };
            }
            Target::Browse if self.browser.is_some() => {
                self.screen = match self.screen {
                    ScreenKind::Browse => ScreenKind::Main,
                    _ => ScreenKind::Browse,
                };
            }
            Target::BrowseBack => self.nav(Nav::Back),
            Target::Library | Target::Usb if self.browser.is_some() => {
                let source = if hit.target == Target::Library {
                    Source::Library
                } else {
                    Source::Folders
                };
                if let Some(b) = self.browser.as_mut() {
                    b.set_source(source);
                }
                self.keyboard = false;
                self.screen = ScreenKind::Browse;
            }
            Target::Sort => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_sort(b.sort().next());
                }
            }
            Target::Search => self.search(),
            Target::TagList if self.browser.is_some() => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_source(Source::TagList);
                }
                self.keyboard = false;
                self.editing.modal = None;
                self.screen = ScreenKind::Browse;
            }
            Target::Menu if self.screen == ScreenKind::Browse => self.open_menu(),
            Target::ModalRow(i) => {
                if let Some(m) = self.editing.modal.as_mut() {
                    m.select(usize::from(i));
                }
                self.modal_push();
                self.naming_started();
            }
            Target::Info => self.toggle_info(),
            Target::Scan => self.effects.push(Effect::Scan),
            Target::AnalyzeAll => self.analyze_all(),
            Target::ScanYes => self.answer_scan(true),
            Target::ScanNo => self.answer_scan(false),
            Target::Key(i) => {
                if let Some(c) = browse::key_char(i) {
                    if c == browse::SHIFT {
                        self.shift = self.shift.tap();
                        return;
                    }
                    let (c, shift) = self.shift.apply(c);
                    self.shift = shift;
                    if self.editing.naming() {
                        self.type_name(c);
                    } else {
                        self.type_char(c);
                    }
                }
            }
            Target::Row(row) => {
                self.list_drag = Some(ListDrag {
                    y: dy,
                    row: usize::from(row),
                    scrolled: 0,
                    moved: false,
                });
            }
            Target::HotCueMode
            | Target::BeatJumpMode
            | Target::DeleteCall
            | Target::Bank
            | Target::Pad(_)
            | Target::BeatLoop(_) => {
                let (hot, beats) = (self.deck.hot_cues, self.deck.loop_beats());
                if let Some(cmd) = perform::press(&mut self.perf, hit.target, hot, beats) {
                    self.send(cmd);
                }
            }
            Target::Slip => self.send(DeckCommand::ToggleSlip),
            Target::CallPrev if looping => self.send(DeckCommand::LoopResize(0.5)),
            Target::CallNext if looping => self.send(DeckCommand::LoopResize(2.0)),
            Target::CallPrev => self.send(DeckCommand::BeatJump(-4.0)),
            Target::CallNext => self.send(DeckCommand::BeatJump(4.0)),
            Target::Zoom => self.zoom = (self.zoom + 1) % ZOOM_STEPS.len(),
            Target::Overview => self.seek_overview(dx),
            Target::Waveform => {
                let span = self.deck.zoom_span(ZOOM_STEPS[self.zoom]);
                let shown = self.track.last_shown.map_or(self.deck.position, |(_, p)| p);
                self.scrub = Some(Scrub {
                    last_x: dx,
                    frames_per_px: span / f64::from(hit.rect.w),
                    shown,
                });
                self.send(DeckCommand::ScratchBegin);
            }
            _ => {}
        }
    }

    fn drag(&mut self, x: f64, y: f64) {
        if let (Some(mut scrub), Some(gfx)) = (self.scrub, &self.gfx) {
            let (dx, _) = gfx.to_design(x, y);
            // The waveform moves with the finger: dragging left plays forward.
            let frames = -f64::from(dx - scrub.last_x) * scrub.frames_per_px;
            let length = self.deck.length;
            scrub.shown = (scrub.shown + frames).clamp(0.0, length);
            scrub.last_x = dx;
            self.scrub = Some(scrub);
            if frames != 0.0 {
                self.send(DeckCommand::ScratchBy(frames));
            }
            return;
        }
        if let (Some(mut d), Some(gfx)) = (self.list_drag, &self.gfx) {
            let (_, dy) = gfx.to_design(x, y);
            let travel = dy - d.y;
            d.moved |= travel.abs() > DRAG_SLOP;
            if d.moved {
                // The list moves with the finger: dragging up shows later rows.
                let want = (-travel / ROW_H).round() as isize;
                if want != d.scrolled
                    && let Some(b) = self.browser.as_mut()
                {
                    b.scroll_by(want - d.scrolled);
                    d.scrolled = want;
                }
                self.pressed = None;
            }
            self.list_drag = Some(d);
            return;
        }
        if self.pressed == Some(Target::Overview)
            && let Some(gfx) = &self.gfx
        {
            let (dx, _) = gfx.to_design(x, y);
            self.seek_overview(dx);
        }
    }

    /// The finger or button came up.
    fn release(&mut self) {
        self.pressed = None;
        if self.scrub.take().is_some() {
            self.send(DeckCommand::ScratchEnd);
        }
        if let Some(d) = self.list_drag.take()
            && !d.moved
        {
            self.tap_row(d.row);
        }
    }

    /// A tap on shown row `row`: a folder opens; a track is selected, and loads if it was
    /// selected already.
    fn tap_row(&mut self, row: usize) {
        let Some(b) = self.browser.as_mut() else {
            return;
        };
        let i = b.level().top + row;
        let Some(entry) = b.level().entries.get(i) else {
            return;
        };
        let open = entry.kind != Kind::Track || b.level().cursor == i;
        b.select(i);
        if open {
            self.enter();
        }
    }

    /// Open the selected folder, or load the selected track.
    fn enter(&mut self) {
        let Some(b) = self.browser.as_mut() else {
            return;
        };
        if let Entered::Track(path) = b.enter() {
            self.effects.push(Effect::Load(path));
        }
    }

    /// A browse control: knob turn / push, BACK.
    fn nav(&mut self, n: Nav) {
        if self.screen == ScreenKind::Info && n == Nav::Back {
            self.screen = ScreenKind::Main;
            return;
        }
        if n == Nav::Tag || n == Nav::Untag {
            return self.tag_track(n == Nav::Untag);
        }
        if self.browser.is_none() {
            return;
        }
        if self.screen != ScreenKind::Browse {
            // Pushing the knob (or BROWSE) elsewhere opens the browser.
            if n == Nav::Push {
                self.screen = ScreenKind::Browse;
            }
            return;
        }
        if let Some(m) = self.editing.modal.as_mut() {
            match n {
                Nav::Up => m.move_by(-1),
                Nav::Down => m.move_by(1),
                Nav::Push => {
                    self.modal_push();
                    self.naming_started();
                }
                Nav::Back => self.editing.modal = None,
                Nav::Tag | Nav::Untag => {}
            }
            return;
        }
        if self.editing.moving.is_some() {
            match n {
                Nav::Up => self.move_entry(-1),
                Nav::Down => self.move_entry(1),
                _ => self.editing.moving = None,
            }
            return;
        }
        let Some(b) = self.browser.as_mut() else {
            return;
        };
        match n {
            Nav::Tag | Nav::Untag => {}
            Nav::Up => b.move_by(-1),
            Nav::Down => b.move_by(1),
            Nav::Push => self.enter(),
            Nav::Back if self.side => self.side = false,
            Nav::Back => {
                if !b.back() {
                    self.screen = ScreenKind::Main;
                }
                if b.search_query().is_none() {
                    self.keyboard = false;
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
        self.track = Track::new(path, tags.as_ref(), self.player.format.sample_rate);
        self.keyboard = false;
        self.scrub = None;
        self.screen = ScreenKind::Main;
        eprintln!("loaded {}", path.display());
    }

    /// The wheel turned `rows` (positive = down) at window pixels (`x`, `y`).
    fn wheel(&mut self, x: f64, y: f64, rows: i32) {
        #[cfg(feature = "standin")]
        if self.standin.is_some()
            && let Some(gfx) = &self.gfx
            && !gfx.on_device(x, y)
        {
            let turn = match (gfx.to_shell(x, y), self.standin.as_mut()) {
                (Some((sx, sy)), Some(s)) => s.wheel(sx, sy, rows),
                _ => None,
            };
            if let Some(e) = turn {
                self.control(e);
            }
            return;
        }
        let _ = (x, y);
        if self.screen == ScreenKind::Browse
            && let Some(b) = self.browser.as_mut()
        {
            b.scroll_by(rows as isize);
        }
    }

    /// ANALYZE ALL: analyse every library track without our analysis, in a batch; or stop.
    fn analyze_all(&mut self) {
        let effect = if self.player.batch_progress().is_some() {
            Effect::StopAnalyzeAll
        } else {
            Effect::AnalyzeAll(self.unanalysed())
        };
        self.effects.push(effect);
    }

    /// The library's tracks without our analysis, one file each, in path order.
    fn unanalysed(&self) -> Vec<PathBuf> {
        let (Some(b), Some(listing)) = (&self.browser, self.player.listing()) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let mut rows: Vec<_> = listing.values().filter(|l| !l.analysed).collect();
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        rows.into_iter()
            .filter(|l| seen.insert(l.track))
            .map(|l| b.root().join(&l.path))
            .collect()
    }

    /// How many library tracks lack our analysis, counted once per listing.
    fn unanalysed_count(&self) -> usize {
        let Some(listing) = self.player.listing() else {
            return 0;
        };
        if let Some((l, n)) = &*self.unanalysed_cache.borrow()
            && Arc::ptr_eq(l, listing)
        {
            return *n;
        }
        let n = self.unanalysed().len();
        *self.unanalysed_cache.borrow_mut() = Some((Arc::clone(listing), n));
        n
    }

    /// The answer to "scan this stick?": a scan shows the library filling in.
    fn answer_scan(&mut self, scan: bool) {
        self.ask_scan = false;
        if scan {
            self.effects.push(Effect::Scan);
            if let Some(b) = self.browser.as_mut() {
                b.set_source(Source::Library);
            }
        }
    }

    /// INFO: in the browser, the selected track's details beside the list (or not); on
    /// any other screen, the INFO screen of the loaded track (or back to the main screen).
    fn toggle_info(&mut self) {
        match self.screen {
            ScreenKind::Browse => {
                self.side = !self.side;
                if self.side {
                    self.keyboard = false;
                }
            }
            ScreenKind::Info => self.screen = ScreenKind::Main,
            _ => self.screen = ScreenKind::Info,
        }
    }

    /// SEARCH: start a search with the keyboard up, or show / hide the keyboard of the one
    /// under way.
    fn search(&mut self) {
        let Some(b) = self.browser.as_mut() else {
            return;
        };
        self.screen = ScreenKind::Browse;
        self.side = false;
        if b.search_query().is_some() {
            self.keyboard = !self.keyboard;
        } else {
            b.search_begin();
            self.keyboard = true;
        }
    }

    /// TAG TRACK: the selected track (on the browse screen) or the loaded one onto the tag
    /// list; `remove` (a long press) takes it off.
    fn tag_track(&mut self, remove: bool) {
        let loaded = self.player.loaded().map(|d| d.file().to_path_buf());
        let browsing = self.screen == ScreenKind::Browse;
        let Some(path) = playlists::tag_target(self.browser.as_ref(), browsing, loaded) else {
            return;
        };
        let tagged = self
            .browser
            .as_ref()
            .map(|b| b.tag_entries(&path))
            .unwrap_or_default();
        match (remove, tagged.is_empty()) {
            (true, false) => self.act(Action::Untag(tagged)),
            (false, true) => self.act(Action::Tag(vec![path])),
            // Already tagged, or nothing to remove.
            _ => {}
        }
    }

    /// MENU: the actions on the selection, as a modal.
    fn open_menu(&mut self) {
        let Some(b) = self.browser.as_ref() else {
            return;
        };
        if !self.editing.open_menu(b) {
            eprintln!("menu: nothing to do here");
            return;
        }
        self.keyboard = false;
    }

    /// The modal's push (or a touch on one of its rows).
    fn modal_push(&mut self) {
        self.editing.push(self.browser.as_mut(), &mut self.effects);
    }

    /// Carry out a MENU action (or TAG TRACK's).
    fn act(&mut self, a: Action) {
        self.editing
            .act(a, self.browser.as_mut(), &mut self.effects);
    }

    /// A name to type: the keyboard starts with a capital for an empty one.
    fn naming_started(&mut self) {
        self.shift = if self.editing.naming_empty() {
            browse::Shift::Once
        } else {
            browse::Shift::Off
        };
    }

    /// A key typed into a name (on-screen keyboard or a real one); DONE names it.
    fn type_name(&mut self, c: char) {
        self.editing.type_name(c, &mut self.effects);
    }

    /// MOVE: the moving entry one place up (`-1`) or down, the cursor with it.
    fn move_entry(&mut self, delta: isize) {
        if let Some(b) = self.browser.as_mut() {
            self.editing.move_entry(delta, b, &mut self.effects);
        }
    }

    /// A key typed into the search (on-screen keyboard or a real one).
    fn type_char(&mut self, c: char) {
        let Some(b) = self.browser.as_mut() else {
            return;
        };
        let Some(q) = b.search_query() else {
            return;
        };
        let mut q = q.to_string();
        match c {
            browse::BACKSPACE => {
                q.pop();
            }
            browse::CLEAR => q.clear(),
            browse::DONE => self.keyboard = false,
            c if !c.is_control() => q.push(c),
            _ => {}
        }
        b.set_query(&q);
    }

    /// What the browse screen shows of the browser.
    fn browse_view(&self) -> Option<BrowseView> {
        let b = self.browser.as_ref()?;
        let l = b.level();
        let loaded = self.player.loaded().map(|d| d.file());
        let rows = l
            .entries
            .iter()
            .skip(l.top)
            .take(ROWS)
            .map(|e| match e.kind {
                Kind::Folder | Kind::Playlist(_) => Row {
                    kind: match e.kind {
                        Kind::Playlist(id) => match b.playlists().and_then(|p| p.get(id)) {
                            Some(p) if p.kind == PlaylistKind::Folder => RowKind::Folder,
                            _ => RowKind::List,
                        },
                        _ => RowKind::Folder,
                    },
                    title: e.name.clone(),
                    artist: String::new(),
                    detail: String::new(),
                    loaded: false,
                    art: None,
                    tagged: false,
                    missing: false,
                },
                Kind::Track => {
                    let tags = b.info(&e.path);
                    let tags = tags.as_ref();
                    let detail = [
                        tags.and_then(|t| t.bpm).map(|b| format!("{b:.1}")),
                        tags.and_then(|t| t.key.clone()),
                        tags.and_then(|t| t.duration).map(browse::length),
                    ];
                    Row {
                        kind: RowKind::Track,
                        title: track_title(&e.path, tags),
                        artist: tags.and_then(|t| t.artist.clone()).unwrap_or_default(),
                        detail: detail.into_iter().flatten().collect::<Vec<_>>().join("   "),
                        loaded: loaded == Some(e.path.as_path()),
                        art: b.art(&e.path),
                        tagged: b.is_tagged(&e.path),
                        missing: e.path.as_os_str().is_empty(),
                    }
                }
            })
            .collect();
        let library = b.source() == Source::Library;
        let top = if library {
            "LIBRARY".to_string()
        } else if b.source() == Source::TagList {
            "TAG LIST".to_string()
        } else {
            b.root()
                .file_name()
                .map_or("USB".into(), |n| n.to_string_lossy().into_owned())
        };
        let mut header = std::iter::once(top.as_str())
            .chain(b.trail())
            .collect::<Vec<_>>()
            .join("  /  ");
        let query = b.search_query();
        if let Some(q) = query {
            header = format!("{header}:  {q}{}", if self.keyboard { "▏" } else { "" });
        }
        let empty = match (&l.error, l.entries.is_empty()) {
            (Some(e), _) => Some(format!("Can't read this folder: {e}")),
            (None, true) if query.is_some() => Some("Nothing matches".into()),
            (None, true) if library => Some(
                "No tracks in the library yet: scan the USB, or load tracks from its folders"
                    .into(),
            ),
            (None, true) => Some("No folders or tracks here".into()),
            _ => None,
        };
        let scan = self
            .player
            .scan_progress()
            .filter(|&(done, total)| done < total || total == 0);
        let side = self.side.then(|| self.info_view(b, library)).flatten();
        Some(BrowseView {
            side,
            info: None,
            header,
            at_root: b.at_root(),
            library,
            sort: b.sort_label(),
            searching: query.is_some(),
            keyboard: (self.keyboard && query.is_some()) || self.editing.naming(),
            shift: self.shift,
            tag_list: b.source() == Source::TagList,
            modal: self.editing.modal.as_ref().and_then(|m| {
                m.rows().map(|(title, rows, selected)| ModalView {
                    title,
                    rows,
                    selected,
                    danger_first: matches!(m, Modal::Confirm { .. }),
                })
            }),
            header_override: match (&self.editing.modal, self.editing.moving) {
                (Some(Modal::Name { naming, text }), _) => {
                    Some(format!("{}:  {text}▏", naming.prompt()))
                }
                (_, Some(_)) => Some("MOVING  ·  TURN TO MOVE, PUSH TO DROP".into()),
                _ => None,
            },
            scan,
            can_scan: self.player.listing().is_some(),
            unanalysed: if library { self.unanalysed_count() } else { 0 },
            analyzing: self
                .player
                .batch_progress()
                .map(|p| (p.done, p.total, p.stopping)),
            ask_scan: self.ask_scan,
            rows,
            selected: l.cursor.checked_sub(l.top).filter(|&r| r < ROWS),
            first: l.top,
            total: l.entries.len(),
            empty,
            has_tracks: l.entries.iter().any(|e| e.kind == Kind::Track),
        })
    }

    /// The info panel's view of the selected entry.
    fn info_view(&self, b: &Browser, library: bool) -> Option<InfoView> {
        let l = b.level();
        let e = l.selected()?;
        let source = if library { "LIBRARY" } else { "USB" };
        let position = format!("TRACK {:03}/{:03}", l.cursor + 1, l.entries.len());
        if e.kind != Kind::Track {
            return Some(InfoView {
                source,
                position,
                folder: true,
                fields: vec![("FOLDER", e.name.clone())],
                ..InfoView::default()
            });
        }
        let tags = b.info(&e.path).unwrap_or_default();
        Some(InfoView {
            source,
            position,
            folder: false,
            fields: info_fields(&e.path, &tags),
            rating: tags.rating,
            // The list's thumbnail stands in until the large picture is read.
            art: b.large_art(&e.path).unwrap_or_else(|| b.art(&e.path)),
            comment: tags.comment.clone(),
            loaded: self.player.loaded().map(|d| d.file()) == Some(e.path.as_path()),
        })
    }

    /// The INFO screen's view: the loaded track, from its own tags, the library's listing
    /// and the analysis (ours win).
    fn info_screen_view(&self) -> BrowseView {
        let loaded = self.player.loaded();
        let info = match (loaded, &self.track.tags) {
            (Some(d), Some(lt)) => {
                let path = d.file();
                let mut tags = self
                    .browser
                    .as_ref()
                    .and_then(|b| b.info(path))
                    .unwrap_or_else(|| lt.tags.clone());
                if let Some(a) = &d.analysed {
                    tags.bpm = a.bpm.or(tags.bpm);
                    tags.key = a.key.clone().or(tags.key);
                }
                tags.duration = tags.duration.or(lt.tags.duration);
                tags.comment = tags.comment.or_else(|| lt.tags.comment.clone());
                InfoView {
                    source: "USB",
                    position: "ON DECK".into(),
                    folder: false,
                    fields: info_fields(path, &tags),
                    rating: tags.rating,
                    art: lt.art.clone(),
                    comment: tags.comment.clone(),
                    loaded: true,
                }
            }
            _ => InfoView {
                source: "USB",
                position: "NO TRACK".into(),
                fields: vec![("TITLE", "No track loaded".into())],
                ..InfoView::default()
            },
        };
        BrowseView {
            info: Some(info),
            ..BrowseView::default()
        }
    }

    /// The shown rows' tags have all been read, and a scan asked for is done (for
    /// screenshots).
    fn browse_settled(&self) -> bool {
        let Some(b) = &self.browser else {
            return true;
        };
        if self.pending_search.is_some()
            || (b.source() != Source::Folders
                && (self.player.listing().is_none() || self.player.playlists().is_none()))
            || self.pending_open.is_some()
            || self
                .player
                .scan_progress()
                .is_some_and(|(done, total)| done < total || total == 0)
        {
            return false;
        }
        let l = b.level();
        let info_ready = !self.side
            || l.selected()
                .is_none_or(|e| e.kind == Kind::Folder || b.large_art(&e.path).is_some());
        info_ready
            && l.entries
                .iter()
                .skip(l.top)
                .take(ROWS)
                .all(|e| e.kind == Kind::Folder || b.is_read(&e.path))
    }

    fn seek_overview(&mut self, x: f32) {
        let Rect { x: x0, w, .. } = OVERVIEW;
        let f = f64::from(((x - x0) / w).clamp(0.0, 1.0));
        self.send(DeckCommand::Seek(f * self.deck.length));
    }

    /// A key of the dev keyboard. Text fields (naming, the search) take the keys first; the
    /// rest is the keymap's.
    fn key(&mut self, event_loop: &ActiveEventLoop, key: &KeyEvent) {
        if key.repeat {
            return;
        }
        let key = keymap::KeyPress::from(key);
        let pressed = key.pressed;
        let browsing = self.screen == ScreenKind::Browse;
        if browsing && self.ask_scan {
            if pressed {
                match &key.key {
                    Key::Named(NamedKey::Enter) => self.answer_scan(true),
                    Key::Named(NamedKey::Escape) => self.answer_scan(false),
                    _ => {}
                }
            }
            return;
        }
        // While naming, the keyboard types the name: Enter names it, Esc gives up.
        if browsing && self.editing.naming() {
            if !pressed {
                return;
            }
            if key.key == Key::Named(NamedKey::Escape) {
                self.editing.modal = None;
            }
            for c in keymap::typed(&key) {
                self.type_name(c);
            }
            return;
        }
        // While the search keyboard is up, the keys type; Enter and Esc still browse.
        if browsing && self.keyboard {
            let chars = keymap::typed(&key);
            if !chars.is_empty() && chars != [browse::DONE] {
                if pressed {
                    for c in chars {
                        self.type_char(c);
                    }
                }
                return;
            }
        }
        let ctx = KeyContext {
            browsing,
            info: self.screen == ScreenKind::Info,
            shift: self.modifiers.shift_key(),
        };
        let Some(shortcut) = keymap::shortcut(&key, ctx) else {
            return;
        };
        match shortcut {
            Shortcut::Control(e) => self.control(e),
            Shortcut::Deck(d) => self.send(d.command(self.deck.sample_rate)),
            Shortcut::Nav(n) => self.nav(n),
            Shortcut::Perform => {
                self.screen = match self.screen {
                    ScreenKind::Performance => ScreenKind::Main,
                    _ => ScreenKind::Performance,
                };
            }
            Shortcut::ToggleBrowse => {
                if self.browser.is_some() {
                    self.screen = if browsing {
                        ScreenKind::Main
                    } else {
                        ScreenKind::Browse
                    };
                }
            }
            Shortcut::Search => self.search(),
            Shortcut::Info => self.toggle_info(),
            Shortcut::Debug => self.debug = !self.debug,
            Shortcut::Memory => eprintln!("{}", self.player.memory()),
            Shortcut::WaveMode => {
                self.wave_settings.mode = match self.wave_settings.mode {
                    Mode::ThreeBand => Mode::Rgb,
                    Mode::Rgb => Mode::Single,
                    Mode::Single => Mode::ThreeBand,
                };
            }
            Shortcut::Fullscreen => {
                if let Some(g) = &self.gfx {
                    let w = g.window();
                    w.set_fullscreen(match w.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(None)),
                    });
                }
            }
            Shortcut::Quit => event_loop.exit(),
        }
    }

    /// Draw one frame; true when the app should exit (a screenshot was taken).
    fn frame(&mut self) -> bool {
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
        if let Some(scrub) = self.scrub {
            // Under the finger the picture follows the hand at once; the audio trails it by the
            // scratch spring's few milliseconds.
            position = scrub.shown;
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
        if pending_analyze {
            self.analyze_all();
        }
        if pending_menu {
            self.open_menu();
        }
        // Before the screen is built, so it shows what they started (a scan, ANALYZE ALL).
        self.apply_effects();
        // Judged before the scene is built, so a screenshot shows everything it waited for.
        let ready = self.player.loaded().is_none_or(|d| {
            d.analysed.is_some() && d.source.is_complete() && self.track.wave_complete
        }) && (self.screen != ScreenKind::Browse || self.browse_settled());
        let browse = match self.screen {
            ScreenKind::Browse => self.browse_view(),
            ScreenKind::Info => Some(self.info_screen_view()),
            _ => None,
        };
        let view = View {
            deck: &self.deck,
            title: &self.track.title,
            position,
            zoom_beats: ZOOM_STEPS[self.zoom],
            pressed: self.pressed,
            debug: self.debug.then_some(self.overlay.as_str()),
            screen: self.screen,
            perf: self.perf,
            save: self.player.persistence().map(|p| match p {
                Persistence::Saved => SaveState::Saved,
                Persistence::Pending => SaveState::Saving,
                Persistence::Unsaved { .. } => SaveState::Unsaved,
            }),
            browse: browse.as_ref(),
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
            s.build(&shell_view, &mut self.shell_scene);
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
}

/// The info panels' fields of a track.
fn info_fields(path: &Path, tags: &engined::browse::Tags) -> Vec<(&'static str, String)> {
    let or_dash = |v: Option<String>| v.unwrap_or_else(|| "–".into());
    vec![
        ("TITLE", track_title(path, Some(tags))),
        ("ARTIST", or_dash(tags.artist.clone())),
        ("ALBUM", or_dash(tags.album.clone())),
        ("TIME", or_dash(tags.duration.map(browse::length))),
        ("BPM", or_dash(tags.bpm.map(|b| format!("{b:.1}")))),
        ("KEY", or_dash(tags.key.clone())),
        ("GENRE", or_dash(tags.genre.clone())),
    ]
}

fn write_png(
    path: &std::path::Path,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

impl App {
    /// The window's logical size: the device screen, or the stand-in around it.
    fn window_size(&self) -> LogicalSize<f32> {
        #[cfg(feature = "standin")]
        if self.standin.is_some() {
            return LogicalSize::new(standin::SIZE.0, standin::SIZE.1);
        }
        LogicalSize::new(render::DESIGN_W, render::DESIGN_H)
    }
}

impl App {
    /// One window event: input, a redraw, a resize.
    fn handle(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gfx {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if self.frame() {
                    return event_loop.exit();
                }
                if let Some(g) = &self.gfx {
                    g.window().request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::MouseWheel { delta, .. } => {
                // Rows: a notch is one; a trackpad scrolls a row per row height of travel.
                self.wheel -= match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => p.y / f64::from(ROW_H),
                };
                let rows = self.wheel.trunc();
                if rows != 0.0 {
                    self.wheel -= rows;
                    self.wheel(self.cursor.0, self.cursor.1, rows as i32);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(event_loop, &event),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.pointer_move(position.x, position.y);
            }
            WindowEvent::MouseInput {
                state,
                button: button @ (MouseButton::Left | MouseButton::Right),
                ..
            } => match state {
                ElementState::Pressed => {
                    let alt = button == MouseButton::Right;
                    self.pointer_down(self.cursor.0, self.cursor.1, alt)
                }
                ElementState::Released => self.pointer_up(),
            },
            WindowEvent::Touch(Touch {
                phase, location, ..
            }) => match phase {
                TouchPhase::Started => self.pointer_down(location.x, location.y, false),
                TouchPhase::Moved => self.pointer_move(location.x, location.y),
                TouchPhase::Ended | TouchPhase::Cancelled => self.pointer_up(),
            },
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("dj-ui")
            .with_inner_size(self.window_size())
            .with_resizable(true);
        if self.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("window: {e}"));
                return event_loop.exit();
            }
        };
        match Gfx::new(window, event_loop) {
            Ok(g) => {
                eprintln!(
                    "{} ({:?}), {:?}, frame latency 1",
                    g.adapter_info.name, g.adapter_info.backend, g.present_mode
                );
                g.window().request_redraw();
                self.gfx = Some(g);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.handle(event_loop, event);
        self.apply_effects();
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        for line in self.player.poll() {
            eprintln!("{line}");
        }
    }
}
