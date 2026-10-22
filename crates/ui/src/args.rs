//! The command line.

use std::path::PathBuf;

use engine_io::StreamRequest;
use engined::browse::{Sort, Source};
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

pub fn parse() -> Result<Args, String> {
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
