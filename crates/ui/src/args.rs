//! The command line.

use std::path::PathBuf;

use browser::{Sort, Source};
use engined::StreamRequest;
use engined::{AutoCue, LibraryChoice};

use crate::screens::perform::{self, PerfState};
use crate::theme::Theme;
use waveform::display::Mode;

pub const USAGE: &str = "usage: dj-ui <file> [--device <name>] [--buffer <frames>] [--fullscreen] [--no-standin] \
                     [--theme <name>] [--wave-theme <name>] [--wave-mode 3band|rgb|single] [--perform] [--pad-mode hotcue|beatjump|delete] \
                     [--hot-cues <s,s,…>] [--at <seconds>] [--play] [--run-for <seconds>] \
                     [--screenshot <out.png>] [--library <dir> | --no-library] [--auto-cue off|<dB>] \
                     [--browse] [--source usb|library] [--sort name|title|artist|bpm|key|time] \
                     [--search <query>] [--scan] [--info] [--info-screen] [--analyze-all] \
                     [--source usb|library|taglist] [--open <a/b>] [--menu]\n\
                     LIBRARY_DIR=<dir> browses <dir> as a USB stick's root; the file is then optional.";

pub struct Args {
    pub file: Option<PathBuf>,
    pub request: StreamRequest,
    pub fullscreen: bool,
    /// Draw the hardware stand-in around the device screen (feature `standin`).
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    pub standin: bool,
    pub screenshot: Option<PathBuf>,
    pub at: Option<f64>,
    pub play: bool,
    pub run_for: Option<f64>,
    pub theme: Theme,
    pub wave_mode: Mode,
    /// `--library` / `--no-library`, if given.
    pub library: Option<LibraryChoice>,
    pub auto_cue: AutoCue,
    pub perform: Option<PerfState>,
    /// Start on the browse screen.
    pub browse: bool,
    /// Browse screen set-up, for screenshots.
    pub source: Option<Source>,
    pub sort: Option<Sort>,
    pub search: Option<String>,
    /// Answer "scan this stick?" with yes (or scan anyway).
    pub scan: bool,
    /// Start with the browser's info panel up.
    pub info: bool,
    /// Start on the INFO screen.
    pub info_screen: bool,
    /// Start ANALYZE ALL once the library has said what it has.
    pub analyze_all: bool,
    /// Open these levels by name (`PLAYLISTS/Peak`), then the MENU if asked.
    pub open: Option<String>,
    pub menu: bool,
    /// Hot cues to set at start (seconds), for screenshots and demos.
    pub hot_cues: Vec<f64>,
}

/// The arguments still to read.
type Rest = std::iter::Skip<std::env::Args>;

pub fn parse() -> Result<Args, String> {
    let mut a = Args::defaults();
    let mut wave_theme: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--wave-theme" {
            wave_theme = Some(args.next().ok_or("--wave-theme needs a name")?);
            continue;
        }
        let handled = output(&mut a, &arg, &mut args)?
            || look(&mut a, &arg, &mut args)?
            || deck(&mut a, &arg, &mut args)?
            || browse(&mut a, &arg, &mut args)?;
        if handled {
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ if a.file.is_none() => a.file = Some(PathBuf::from(arg)),
            _ => return Err("only one file, please".into()),
        }
    }
    if let Some(name) = wave_theme {
        let names: Vec<_> = waveform::display::Theme::ALL
            .iter()
            .map(|t| t.name)
            .collect();
        a.theme = a.theme.with_waveform_theme(&name).ok_or_else(|| {
            format!(
                "unknown waveform theme {name:?} (themes: {})",
                names.join(", ")
            )
        })?;
    }
    Ok(a)
}

impl Args {
    /// Everything as it is without options.
    fn defaults() -> Self {
        Args {
            file: None,
            request: StreamRequest::default(),
            fullscreen: false,
            standin: cfg!(feature = "standin"),
            screenshot: None,
            at: None,
            play: false,
            run_for: None,
            theme: Theme::default(),
            wave_mode: Mode::ThreeBand,
            library: None,
            auto_cue: AutoCue::default(),
            perform: None,
            browse: false,
            source: None,
            sort: None,
            search: None,
            scan: false,
            info: false,
            info_screen: false,
            analyze_all: false,
            open: None,
            menu: false,
            hot_cues: Vec::new(),
        }
    }
}

/// A time option's seconds.
fn seconds(s: &str) -> Result<f64, String> {
    s.parse().map_err(|_| format!("bad time {s:?}"))
}

/// The audio device, the window and what to do with the frames: true if `arg` was one.
fn output(a: &mut Args, arg: &str, args: &mut Rest) -> Result<bool, String> {
    match arg {
        "--device" => a.request.device = Some(args.next().ok_or("--device needs a name")?),
        "--buffer" => {
            let n = args.next().ok_or("--buffer needs a frame count")?;
            a.request.buffer_frames = Some(n.parse().map_err(|_| format!("bad buffer {n:?}"))?);
        }
        "--fullscreen" => a.fullscreen = true,
        "--no-standin" => a.standin = false,
        "--screenshot" => {
            a.screenshot = Some(PathBuf::from(
                args.next().ok_or("--screenshot needs a path")?,
            ))
        }
        "--run-for" => a.run_for = Some(seconds(&args.next().ok_or("--run-for needs seconds")?)?),
        _ => return Ok(false),
    }
    Ok(true)
}

/// The theme and the waveform mode: true if `arg` was one.
fn look(a: &mut Args, arg: &str, args: &mut Rest) -> Result<bool, String> {
    match arg {
        "--theme" => {
            let name = args.next().ok_or("--theme needs a name")?;
            a.theme = Theme::named(&name).ok_or_else(|| {
                let names: Vec<_> = Theme::all().iter().map(|t| t.name).collect();
                format!("unknown theme {name:?} (themes: {})", names.join(", "))
            })?;
        }
        "--wave-mode" => {
            let name = args
                .next()
                .ok_or("--wave-mode needs 3band, rgb or single")?;
            a.wave_mode = Mode::ALL
                .into_iter()
                .find(|m| m.name() == name)
                .ok_or(format!(
                    "unknown waveform mode {name:?} (3band, rgb, single)"
                ))?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// The deck and the performance screen at start: true if `arg` was one.
fn deck(a: &mut Args, arg: &str, args: &mut Rest) -> Result<bool, String> {
    match arg {
        "--at" => a.at = Some(seconds(&args.next().ok_or("--at needs seconds")?)?),
        "--play" => a.play = true,
        "--perform" => {
            a.perform.get_or_insert_with(PerfState::default);
        }
        "--pad-mode" => {
            let p = a.perform.get_or_insert_with(PerfState::default);
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
            a.hot_cues = list
                .split(',')
                .map(|x| {
                    x.trim()
                        .parse::<f64>()
                        .map_err(|_| format!("bad time {x:?}"))
                })
                .collect::<Result<_, _>>()?;
        }
        "--auto-cue" => {
            let v = args.next().ok_or("--auto-cue needs off or a level in dB")?;
            a.auto_cue = AutoCue::parse(&v).ok_or(format!("bad auto cue {v:?}"))?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// The library and the browse and INFO screens at start: true if `arg` was one. Most of
/// these start on the browse screen.
fn browse(a: &mut Args, arg: &str, args: &mut Rest) -> Result<bool, String> {
    match arg {
        "--library" => {
            a.library = Some(LibraryChoice::Folder(PathBuf::from(
                args.next().ok_or("--library needs a folder")?,
            )))
        }
        "--no-library" => a.library = Some(LibraryChoice::Ram),
        "--info-screen" => a.info_screen = true,
        "--browse" => a.browse = true,
        "--source" => {
            a.source = Some(match args.next().as_deref() {
                Some("usb") => Source::Folders,
                Some("library") => Source::Library,
                Some("taglist") => Source::TagList,
                other => return Err(format!("bad source {other:?} (usb, library)")),
            });
            a.browse = true;
        }
        "--sort" => {
            let name = args.next().unwrap_or_default();
            a.sort = Some(
                Sort::ALL
                    .into_iter()
                    .find(|s| s.label().eq_ignore_ascii_case(&name))
                    .ok_or(format!("bad sort {name:?}"))?,
            );
            a.browse = true;
        }
        "--info" => {
            a.info = true;
            a.browse = true;
        }
        "--open" => {
            a.open = Some(args.next().ok_or("--open needs names, /-separated")?);
            a.browse = true;
        }
        "--menu" => {
            a.menu = true;
            a.open.get_or_insert_with(String::new);
            a.browse = true;
        }
        "--analyze-all" => {
            a.analyze_all = true;
            a.source = Some(Source::Library);
            a.browse = true;
        }
        "--scan" => {
            a.scan = true;
            a.browse = true;
        }
        "--search" => {
            a.search = Some(args.next().ok_or("--search needs a query")?);
            a.browse = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
