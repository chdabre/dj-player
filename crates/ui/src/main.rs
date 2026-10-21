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
//! See docs/spikes/ui-shell.md for what this measures and what it found.

mod clock;
mod gfx;
mod perform;
mod scene;
mod screen;
mod stats;
mod theme;
mod waves;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use engine_core::DeckCommand;
use engine_io::{CpalBackend, StreamRequest};
use engined::{AutoCue, LibraryChoice, Options, Persistence, Player};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::clock::PlayheadClock;
use crate::gfx::Gfx;
use crate::scene::Scene;
use crate::screen::{HitBox, OVERVIEW, PerfState, SaveState, ScreenKind, Target, View};
use crate::stats::FrameStats;
use crate::theme::Theme;
use crate::waves::{OverviewCache, WaveSettings};
use waveform::compute::Entry;
use waveform::display::{Mode, TrackGain};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const USAGE: &str = "usage: dj-ui <file> [--device <name>] [--buffer <frames>] [--fullscreen] \
                     [--theme <name>] [--wave-theme <name>] [--wave-mode 3band|rgb|single] [--perform] [--pad-mode hotcue|beatjump|delete] \
                     [--hot-cues <s,s,…>] [--at <seconds>] [--play] [--run-for <seconds>] \
                     [--screenshot <out.png>] [--library <dir> | --no-library] [--auto-cue off|<dB>]";
const SEEK_SECONDS: f64 = 5.0;
const RATE_STEP: f64 = 0.01;
const ZOOM_STEPS: [f64; 4] = [8.0, 16.0, 32.0, 64.0];

struct Args {
    file: PathBuf,
    request: StreamRequest,
    fullscreen: bool,
    screenshot: Option<PathBuf>,
    at: Option<f64>,
    play: bool,
    run_for: Option<f64>,
    theme: Theme,
    wave_mode: Mode,
    library: LibraryChoice,
    auto_cue: AutoCue,
    perform: Option<PerfState>,
    /// Hot cues to set at start (seconds), for screenshots and demos.
    hot_cues: Vec<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut file = None;
    let mut request = StreamRequest::default();
    let (mut fullscreen, mut screenshot, mut at) = (false, None, None);
    let (mut play, mut run_for) = (false, None);
    let mut theme = Theme::default();
    let mut wave_theme: Option<String> = None;
    let mut wave_mode = Mode::ThreeBand;
    let mut library = LibraryChoice::Auto;
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
                    Some("beatjump") => p.mode = screen::PadMode::BeatJump,
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
                library = LibraryChoice::Folder(PathBuf::from(
                    args.next().ok_or("--library needs a folder")?,
                ))
            }
            "--no-library" => library = LibraryChoice::Ram,
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
        file: file.ok_or("no file given")?,
        request,
        fullscreen,
        screenshot,
        at,
        play,
        run_for,
        theme,
        wave_mode,
        library,
        auto_cue,
        perform,
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
    let backend = CpalBackend::new();
    let options = Options {
        library: args.library.clone(),
        auto_cue: args.auto_cue,
        ..Options::default()
    };
    let mut player = match Player::open_with(&backend, &args.request, &args.file, options) {
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
    let sr = f64::from(player.format.sample_rate);
    for (i, &at) in args.hot_cues.iter().enumerate().take(engine_core::HOT_CUES) {
        player.send(DeckCommand::Seek(at * sr));
        player.send(DeckCommand::HotCue(i));
    }
    if let Some(at) = args.at {
        player.send(DeckCommand::Seek(at * f64::from(player.format.sample_rate)));
    }
    if args.play {
        player.send(DeckCommand::Play);
    }
    let title = args
        .file
        .file_stem()
        .map_or("[TRACK]".into(), |s| s.to_string_lossy().into_owned());
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dj-ui: {e}");
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        clock: PlayheadClock::new(player.format.sample_rate),
        last_shown: None,
        scrub: None,
        player,
        title,
        fullscreen: args.fullscreen,
        screenshot: args.screenshot,
        run_for: args.run_for.map(Duration::from_secs_f64),
        started: Instant::now(),
        gfx: None,
        theme: args.theme,
        scene: Scene::default(),
        screen: if args.perform.is_some() {
            ScreenKind::Performance
        } else {
            ScreenKind::Main
        },
        perf: args.perform.unwrap_or_default(),
        wave: Vec::new(),
        wave_complete: false,
        wave_settings: WaveSettings {
            mode: args.wave_mode,
            ..WaveSettings::default()
        },
        track_gain: (1.0, 0),
        overview: OverviewCache::default(),
        hits: Vec::new(),
        pressed: None,
        cursor: (0.0, 0.0),
        modifiers: ModifiersState::empty(),
        rate_steps: 0,
        quantize: true,
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

struct App {
    player: Player,
    title: String,
    fullscreen: bool,
    screenshot: Option<PathBuf>,
    run_for: Option<Duration>,
    started: Instant,
    gfx: Option<Gfx>,
    theme: Theme,
    scene: Scene,
    screen: ScreenKind,
    perf: PerfState,
    wave: Vec<Entry>,
    wave_complete: bool,
    wave_settings: WaveSettings,
    /// The track gain, and the entry count it was computed from.
    track_gain: (f32, usize),
    overview: OverviewCache,
    hits: Vec<HitBox>,
    /// Touched target, held until release; the overview tracks the drag.
    pressed: Option<Target>,
    cursor: (f64, f64),
    modifiers: ModifiersState,
    rate_steps: i32,
    quantize: bool,
    zoom: usize,
    debug: bool,
    clock: PlayheadClock,
    /// The hand on the zoom waveform: where the finger was (design x), the frames one design
    /// pixel covers, and the playhead the finger has moved the track to.
    scrub: Option<Scrub>,
    last_shown: Option<(Instant, f64)>,
    stats: FrameStats,
    overlay: String,
    error: Option<String>,
}

impl App {
    fn send(&mut self, cmd: DeckCommand) {
        if !self.player.send(cmd) {
            eprintln!("command ring full, dropped");
        }
    }

    fn press(&mut self, x: f64, y: f64) {
        let Some(gfx) = &self.gfx else { return };
        let (dx, dy) = gfx.to_design(x, y);
        let Some(hit) = self.hits.iter().find(|h| h.contains(dx, dy)).copied() else {
            return;
        };
        self.pressed = Some(hit.target);
        let looping = self.player.state().looping.load(Ordering::Relaxed);
        match hit.target {
            Target::Perform => {
                self.screen = match self.screen {
                    ScreenKind::Main => ScreenKind::Performance,
                    ScreenKind::Performance => ScreenKind::Main,
                };
            }
            Target::HotCueMode
            | Target::BeatJumpMode
            | Target::DeleteCall
            | Target::Bank
            | Target::Pad(_)
            | Target::BeatLoop(_) => {
                let hot = self.player.state().hot_cues.load(Ordering::Relaxed);
                let beats = self.loop_beats();
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
                let st = self.player.state();
                let sr = f64::from(self.player.format.sample_rate);
                let grid = self
                    .player
                    .analysed
                    .as_ref()
                    .and_then(|a| a.grid.as_deref());
                let span = screen::zoom_span(grid, ZOOM_STEPS[self.zoom], sr);
                let shown = self.last_shown.map_or(st.position.load(), |(_, p)| p);
                self.scrub = Some(Scrub {
                    last_x: dx,
                    frames_per_px: span / f64::from(hit.w),
                    shown,
                });
                self.send(DeckCommand::ScratchBegin);
            }
            _ => {}
        }
    }

    /// The active loop's length in beats, as the performance screen shows it.
    fn loop_beats(&self) -> Option<f64> {
        let st = self.player.state();
        let (a, b) = (st.loop_start.load(), st.loop_end.load());
        if !st.looping.load(Ordering::Relaxed) || b <= a {
            return None;
        }
        let grid = self
            .player
            .analysed
            .as_ref()
            .and_then(|x| x.grid.as_deref());
        Some(match grid {
            Some(g) => g.beat_at(b) - g.beat_at(a),
            None => {
                (b - a) / f64::from(self.player.format.sample_rate) * engine_core::FALLBACK_BPM
                    / 60.0
            }
        })
    }

    fn drag(&mut self, x: f64, y: f64) {
        if let (Some(mut scrub), Some(gfx)) = (self.scrub, &self.gfx) {
            let (dx, _) = gfx.to_design(x, y);
            // The waveform moves with the finger: dragging left plays forward.
            let frames = -f64::from(dx - scrub.last_x) * scrub.frames_per_px;
            let length = self.player.state().length.load();
            scrub.shown = (scrub.shown + frames).clamp(0.0, length);
            scrub.last_x = dx;
            self.scrub = Some(scrub);
            if frames != 0.0 {
                self.send(DeckCommand::ScratchBy(frames));
            }
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
    }

    fn seek_overview(&mut self, x: f32) {
        let (x0, _, w, _) = OVERVIEW;
        let f = f64::from(((x - x0) / w).clamp(0.0, 1.0));
        let length = self.player.state().length.load();
        self.send(DeckCommand::Seek(f * length));
    }

    fn key(&mut self, event_loop: &ActiveEventLoop, key: &KeyEvent) {
        let down = key.state == ElementState::Pressed;
        // CUE is the one key with a release action: holding at the cue previews.
        if let Key::Character(c) = &key.logical_key
            && c.eq_ignore_ascii_case("c")
        {
            if !key.repeat {
                self.send(if down {
                    DeckCommand::CuePress
                } else {
                    DeckCommand::CueRelease
                });
            }
            return;
        }
        if !down || key.repeat {
            return;
        }
        let sr = f64::from(self.player.format.sample_rate);
        let cmd = match &key.logical_key {
            Key::Named(NamedKey::Space) => DeckCommand::TogglePlay,
            Key::Named(NamedKey::ArrowLeft) => DeckCommand::SeekRelative(-SEEK_SECONDS * sr),
            Key::Named(NamedKey::ArrowRight) => DeckCommand::SeekRelative(SEEK_SECONDS * sr),
            Key::Named(NamedKey::Escape) => return event_loop.exit(),
            Key::Character(c) => {
                let c = c.as_str();
                match c {
                    "q" => return event_loop.exit(),
                    "d" => {
                        self.debug = !self.debug;
                        return;
                    }
                    "m" => {
                        eprintln!("{}", self.player.memory());
                        return;
                    }
                    "p" => {
                        self.screen = match self.screen {
                            ScreenKind::Main => ScreenKind::Performance,
                            ScreenKind::Performance => ScreenKind::Main,
                        };
                        return;
                    }
                    "w" => {
                        self.wave_settings.mode = match self.wave_settings.mode {
                            Mode::ThreeBand => Mode::Rgb,
                            Mode::Rgb => Mode::Single,
                            Mode::Single => Mode::ThreeBand,
                        };
                        return;
                    }
                    "f" => {
                        if let Some(g) = &self.gfx {
                            let w = g.window();
                            w.set_fullscreen(match w.fullscreen() {
                                Some(_) => None,
                                None => Some(Fullscreen::Borderless(None)),
                            });
                        }
                        return;
                    }
                    "+" | "=" => {
                        self.rate_steps += 1;
                        DeckCommand::SetRate(1.0 + f64::from(self.rate_steps) * RATE_STEP)
                    }
                    "-" | "_" => {
                        self.rate_steps -= 1;
                        DeckCommand::SetRate(1.0 + f64::from(self.rate_steps) * RATE_STEP)
                    }
                    "0" => {
                        self.rate_steps = 0;
                        DeckCommand::SetRate(1.0)
                    }
                    "k" => DeckCommand::ToggleKeylock,
                    "r" => DeckCommand::ToggleReverse,
                    "s" => DeckCommand::ToggleSlip,
                    "z" => {
                        self.quantize = !self.quantize;
                        DeckCommand::SetQuantize(self.quantize.then_some(1.0))
                    }
                    "i" => DeckCommand::LoopIn,
                    "o" => DeckCommand::LoopOut,
                    "l" => DeckCommand::ReloopExit,
                    "b" => DeckCommand::BeatLoop(4.0),
                    "[" => DeckCommand::LoopResize(0.5),
                    "]" => DeckCommand::LoopResize(2.0),
                    "," => DeckCommand::BeatJump(-4.0),
                    "." => DeckCommand::BeatJump(4.0),
                    _ => match hot_cue_key(key) {
                        Some(i) if self.modifiers.shift_key() => DeckCommand::DeleteHotCue(i),
                        Some(i) => DeckCommand::HotCue(i),
                        None => return,
                    },
                }
            }
            _ => return,
        };
        self.send(cmd);
    }

    /// Draw one frame; true when the app should exit (a screenshot was taken).
    fn frame(&mut self) -> bool {
        let now = Instant::now();
        let st = Arc::clone(self.player.state());
        let sr = f64::from(self.player.format.sample_rate);
        let playing = st.playing.load(Ordering::Relaxed);
        let reverse = st.reverse.load(Ordering::Relaxed);
        let rate = st.rate.load();
        let speed = if playing {
            rate * if reverse { -1.0 } else { 1.0 }
        } else {
            0.0
        };
        let mut position = self.clock.update(now, st.position.load(), speed);
        if let Some(scrub) = self.scrub {
            // Under the finger the picture follows the hand at once; the audio trails it by the
            // scratch spring's few milliseconds.
            position = scrub.shown;
        }
        if let Some((t, p)) = self.last_shown
            && speed != 0.0
        {
            let expected = (now - t).as_secs_f64() * speed * sr;
            self.stats
                .record_playhead((position - p - expected) / sr * 1e3);
        }
        self.last_shown = Some((now, position));
        if !self.wave_complete {
            self.wave_complete = self.player.waveform().update(&mut self.wave);
        }
        // Auto gain over what has decoded so far; it settles as the track fills in.
        let n = self.wave.len();
        if n != self.track_gain.1 && (self.wave_complete || n >= self.track_gain.1 + 1500) {
            self.track_gain = (TrackGain::Auto.resolve(&self.wave), n);
        }
        let mut wave_style = self.theme.waveform.style;
        wave_style.track_gain = self.track_gain.0;
        let analysed = self.player.analysed.as_ref();
        let source = &self.player.source;
        let view = View {
            title: &self.title,
            key: analysed.and_then(|a| a.key.as_deref()),
            sample_rate: sr,
            position,
            shadow_position: st.shadow_position.load(),
            length: st.length.load(),
            loaded_fraction: source.chunks_ready() as f64 / source.chunk_count().max(1) as f64,
            rate,
            slip_enabled: st.slip_enabled.load(Ordering::Relaxed),
            slip_active: st.slip_active.load(Ordering::Relaxed),
            keylock: st.keylock_enabled.load(Ordering::Relaxed),
            quantize: st.quantize.load(),
            cue: st.cue.load(),
            hot_cues: st.hot_cues.load(Ordering::Relaxed),
            looping: st.looping.load(Ordering::Relaxed),
            loop_start: st.loop_start.load(),
            loop_end: st.loop_end.load(),
            grid: analysed.and_then(|a| a.grid.as_deref()),
            analysing: analysed.is_none(),
            zoom_beats: ZOOM_STEPS[self.zoom],
            pressed: self.pressed,
            debug: self.debug.then_some(self.overlay.as_str()),
            hot_cue_positions: std::array::from_fn(|i| st.hot_cue_positions[i].load()),
            screen: self.screen,
            perf: self.perf,
            save: self.player.persistence().map(|p| match p {
                Persistence::Saved => SaveState::Saved,
                Persistence::Pending => SaveState::Saving,
                Persistence::Unsaved { .. } => SaveState::Unsaved,
            }),
            wave: (!self.wave.is_empty()).then_some(screen::Wave {
                entries: &self.wave,
                entries_per_second: f64::from(waveform::compute::ENTRIES_PER_SECOND),
                settings: self.wave_settings,
                style: wave_style,
            }),
        };
        let build_start = Instant::now();
        self.hits = screen::build(&view, &self.theme, &mut self.overview, &mut self.scene);
        let build = build_start.elapsed();

        let Some(gfx) = &mut self.gfx else {
            return false;
        };
        if let Some(path) = &self.screenshot {
            let ready = self.player.analysed.is_some()
                && self.player.source.is_complete()
                && self.wave_complete;
            if ready || self.started.elapsed() > Duration::from_secs(30) {
                let pixels = gfx.screenshot(&self.scene);
                match write_png(path, &pixels) {
                    Ok(()) => eprintln!("wrote {}", path.display()),
                    Err(e) => self.error = Some(format!("screenshot: {e}")),
                }
                return true;
            }
        }
        if let Some(timing) = gfx.render(&self.scene) {
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

/// The digit keys 1–8 (by position, so shift and layout do not matter) are hot cues A–H.
fn hot_cue_key(key: &KeyEvent) -> Option<usize> {
    use winit::keyboard::{KeyCode, PhysicalKey};
    const DIGITS: [KeyCode; 8] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
    ];
    match key.physical_key {
        PhysicalKey::Code(code) => DIGITS.iter().position(|&d| d == code),
        PhysicalKey::Unidentified(_) => None,
    }
}

fn write_png(path: &std::path::Path, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, gfx::DESIGN_W as u32, gfx::DESIGN_H as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("dj-ui")
            .with_inner_size(LogicalSize::new(gfx::DESIGN_W, gfx::DESIGN_H))
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
            WindowEvent::KeyboardInput { event, .. } => self.key(event_loop, &event),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.drag(position.x, position.y);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.press(self.cursor.0, self.cursor.1),
                ElementState::Released => self.release(),
            },
            WindowEvent::Touch(Touch {
                phase, location, ..
            }) => match phase {
                TouchPhase::Started => self.press(location.x, location.y),
                TouchPhase::Moved => self.drag(location.x, location.y),
                TouchPhase::Ended | TouchPhase::Cancelled => self.release(),
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        for line in self.player.poll() {
            eprintln!("{line}");
        }
    }
}
