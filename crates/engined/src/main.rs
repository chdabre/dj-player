//! `engined <file>`: play one track on one deck through the real engine.
//!
//! Keys: space play/pause, ←/→ seek ±5 s, +/- rate ±1 %, 0 reset rate, k keylock,
//! r reverse, s slip, m memory, q quit. Prints the deck state and the callback load meter once
//! per second. Cues, hot cues, the waveform and analysis persist in the library (`--library`).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use engine_core::{DeckCommand, DeckState, LoadMeter, Source};
use engine_io::{CpalBackend, StreamRequest};
use engined::{AutoCue, LibraryChoice, Options, Player};

/// Debug builds abort on any allocation inside `assert_no_alloc` scopes (the audio callback).
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SEEK_SECONDS: f64 = 5.0;
const RATE_STEP: f64 = 0.01;

const USAGE: &str = "\
usage: engined <file> [--device <name>] [--buffer <frames>] [--bpm-range MIN-MAX]
               [--library <dir> | --no-library] [--auto-cue off|<dB>] [--list-devices]

--library <dir>: the library folder (made if missing). Default: DJPLAYER/ at the root of the
track's removable volume (a USB stick or SD card) if it can be written, else RAM only.
--no-library: RAM only.
--auto-cue: with no stored memory cue, the load cue point is the first sound at or above this
level (default -36 dB); off: no cue point, the playhead at the start.

Set TEMPOCNN_MODEL to the TempoCNN .onnx model for model-based tempo (see docs/plan/analysis.md).

keys:  space play/pause   <-/-> seek -/+5 s   +/- rate +/-1 %   0 reset rate
       k keylock   r reverse   s slip   z quantize   c cue   1-4 hot cue A-D (!@#$ delete)
       i/o loop in/out   l reloop/exit   b 4-beat loop   [/] loop 1/2X / 2X   ,/. jump -/+4 beats
       m memory (store the cue point, or the active loop, in the library)   q quit

A terminal reports no key releases, so c is a tap: back cue and set cue work, holding to
preview does not.";

struct Args {
    file: Option<PathBuf>,
    request: StreamRequest,
    list_devices: bool,
    options: Options,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        file: None,
        request: StreamRequest::default(),
        list_devices: false,
        options: Options::default(),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--device" => out.request.device = Some(args.next().ok_or("--device needs a name")?),
            "--buffer" => {
                let n = args.next().ok_or("--buffer needs a frame count")?;
                out.request.buffer_frames =
                    Some(n.parse().map_err(|_| format!("bad buffer size {n:?}"))?);
            }
            "--list-devices" => out.list_devices = true,
            "--bpm-range" => {
                let r = args.next().ok_or("--bpm-range needs MIN-MAX")?;
                out.options.settings.bpm_range = Some(
                    engine_analysis::BpmRange::parse(&r).ok_or(format!("bad BPM range {r:?}"))?,
                );
            }
            "--library" => {
                let dir = args.next().ok_or("--library needs a folder")?;
                out.options.library = LibraryChoice::Folder(PathBuf::from(dir));
            }
            "--no-library" => out.options.library = LibraryChoice::Ram,
            "--auto-cue" => {
                let v = args.next().ok_or("--auto-cue needs off or a level in dB")?;
                out.options.auto_cue = AutoCue::parse(&v).ok_or(format!("bad auto cue {v:?}"))?;
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ if out.file.is_none() => out.file = Some(PathBuf::from(arg)),
            _ => return Err("only one file, please".into()),
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("engined: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("engined: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let backend = CpalBackend::new();
    if args.list_devices {
        for name in backend.output_device_names()? {
            println!("{name}");
        }
        return Ok(());
    }
    let Some(file) = args.file else {
        eprintln!("{USAGE}");
        return Err("no file given".into());
    };

    let mut player = Player::open_with(&backend, &args.request, &file, args.options)?;
    let format = player.format;
    let loaded = player.loaded().expect("just loaded");
    let (info, source) = (loaded.info.clone(), Arc::clone(&loaded.source));
    println!(
        "{} output: {} Hz, {} channels, buffer {}",
        backend.host_name(),
        format.sample_rate,
        format.channels,
        args.request
            .buffer_frames
            .map_or("default".to_string(), |n| format!("{n} frames")),
    );
    println!(
        "loading {} ({} Hz, {} ch{})",
        file.display(),
        info.file_sample_rate,
        info.file_channels,
        if info.file_sample_rate == format.sample_rate {
            ""
        } else {
            ", resampling"
        },
    );
    println!("{}", player.device_name());

    let interactive = std::io::stdin().is_terminal() && terminal::enable_raw_mode().is_ok();
    let _raw = interactive.then_some(RawModeGuard);
    if interactive {
        out(
            "space play  c cue  1-4 hot cues  m memory  i/o/l loop  b beat loop  [/] 1/2X 2X  \
             ,/. jump  z quantize  k keylock  r reverse  s slip  q quit (engined --help for all keys)",
        );
    } else {
        out("stdin is not a terminal: playing to the end without key control");
        player.send(DeckCommand::Play);
    }

    let state = Arc::clone(player.state());
    let mut controls = Controls::new(format.sample_rate);
    let mut next_report = Instant::now() + Duration::from_secs(1);
    loop {
        if interactive {
            if event::poll(Duration::from_millis(20))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match controls.command(key) {
                    Action::Quit => break,
                    Action::Memory => out(&player.memory()),
                    Action::Send(cmd) => {
                        if !player.send(cmd) {
                            out("command ring full, key dropped");
                        }
                    }
                    Action::Tap(press, release) => {
                        if !player.send(press) || !player.send(release) {
                            out("command ring full, key dropped");
                        }
                    }
                    Action::None => {}
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(20));
            if source.is_complete()
                && !state.playing.load(Ordering::Relaxed)
                && state.position.load() > 0.0
            {
                report(&state, player.load_meter(), &source, format.sample_rate);
                break;
            }
        }
        for line in player.poll() {
            out(&line);
        }
        if Instant::now() >= next_report {
            next_report += Duration::from_secs(1);
            report(&state, player.load_meter(), &source, format.sample_rate);
        }
    }
    Ok(())
}

/// Print a line that also works in raw mode.
fn out(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "{line}\r\n");
    let _ = stdout.flush();
}

fn report(state: &DeckState, meter: &LoadMeter, source: &Source, rate: u32) {
    let load = meter.take();
    let sr = f64::from(rate);
    let flags = [
        if state.playing.load(Ordering::Relaxed) {
            "PLAY"
        } else {
            "PAUSE"
        },
        if state.reverse.load(Ordering::Relaxed) {
            "REV"
        } else {
            "FWD"
        },
        match (
            state.slip_enabled.load(Ordering::Relaxed),
            state.slip_active.load(Ordering::Relaxed),
        ) {
            (false, _) => "slip off",
            (true, false) => "slip on",
            (true, true) => "SLIPPING",
        },
        match (
            state.keylock_enabled.load(Ordering::Relaxed),
            state.stretching.load(Ordering::Relaxed),
        ) {
            (false, _) => "keylock off",
            (true, true) => "keylock on",
            // Paused, reversing or at a rate the stretcher does not play: resampler.
            (true, false) => "keylock bypassed",
        },
    ];
    let track_bpm = state.track_bpm.load();
    let tempo = if track_bpm > 0.0 {
        let beat = state.beat.load();
        format!(
            "  {:.2} BPM  bar {:.0}.{}",
            track_bpm * state.rate.load(),
            (beat / 4.0).floor() + 1.0,
            beat.rem_euclid(4.0).floor() as u32 + 1
        )
    } else {
        String::new()
    };
    let hot = state.hot_cues.load(Ordering::Relaxed);
    let pads: String = (0..8)
        .map(|i| {
            if hot & (1 << i) != 0 {
                (b'A' + i) as char
            } else {
                '.'
            }
        })
        .take(4)
        .collect();
    let extras = format!(
        "  Q{}  {}  hot {pads}",
        match state.quantize.load() {
            q if q > 0.0 => format!(" {q}"),
            _ => " off".into(),
        },
        if state.looping.load(Ordering::Relaxed) {
            format!(
                "LOOP {}",
                clock((state.loop_end.load() - state.loop_start.load()) / sr)
            )
        } else {
            "no loop".into()
        },
    );
    let mut line = format!(
        "{} / {}  rate {:+.2} %{tempo}  {} {} {} {}{extras}  load avg {:4.1} % peak {:4.1} % ({} cb)",
        clock(state.position.load() / sr),
        clock(state.length.load() / sr),
        (state.rate.load() - 1.0) * 100.0,
        flags[0],
        flags[1],
        flags[2],
        flags[3],
        load.average * 100.0,
        load.peak * 100.0,
        load.callbacks,
    );
    if state.slip_active.load(Ordering::Relaxed) {
        line += &format!("  shadow {}", clock(state.shadow_position.load() / sr));
    }
    if !source.is_complete() {
        line += &format!(
            "  loading {}/{} chunks",
            source.chunks_ready(),
            source.chunk_count()
        );
    }
    let starved = state.starved_blocks.swap(0, Ordering::Relaxed);
    if starved > 0 {
        line += &format!("  STARVED {starved} blocks");
    }
    out(&line);
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let m = (s / 60.0).floor();
    format!("{:02}:{:06.3}", m as u64, s - m * 60.0)
}

enum Action {
    None,
    Quit,
    /// MEMORY: store a memory cue in the library.
    Memory,
    Send(DeckCommand),
    /// A button press and its release (the terminal reports no releases).
    Tap(DeckCommand, DeckCommand),
}

/// Maps keys to deck commands. Rate is kept here in whole 1 % steps and sent as an absolute
/// value, so repeated presses do not accumulate floating-point error.
struct Controls {
    sample_rate: f64,
    rate_steps: i32,
    quantize: bool,
}

impl Controls {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            rate_steps: 0,
            // The deck starts with quantize on (one beat).
            quantize: true,
        }
    }

    fn rate(&self) -> DeckCommand {
        DeckCommand::SetRate(1.0 + f64::from(self.rate_steps) * RATE_STEP)
    }

    fn command(&mut self, key: KeyEvent) -> Action {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c {
            return Action::Quit;
        }
        let seek = SEEK_SECONDS * self.sample_rate;
        Action::Send(match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char(' ') => DeckCommand::TogglePlay,
            KeyCode::Left => DeckCommand::SeekRelative(-seek),
            KeyCode::Right => DeckCommand::SeekRelative(seek),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.rate_steps += 1;
                self.rate()
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.rate_steps -= 1;
                self.rate()
            }
            KeyCode::Char('0') => {
                self.rate_steps = 0;
                self.rate()
            }
            KeyCode::Char('k') => DeckCommand::ToggleKeylock,
            KeyCode::Char('r') => DeckCommand::ToggleReverse,
            KeyCode::Char('s') => DeckCommand::ToggleSlip,
            KeyCode::Char('z') => {
                self.quantize = !self.quantize;
                DeckCommand::SetQuantize(self.quantize.then_some(1.0))
            }
            KeyCode::Char('c') => {
                return Action::Tap(DeckCommand::CuePress, DeckCommand::CueRelease);
            }
            KeyCode::Char(c @ '1'..='4') => DeckCommand::HotCue(c as usize - '1' as usize),
            KeyCode::Char(c @ ('!' | '@' | '#' | '$')) => {
                DeckCommand::DeleteHotCue("!@#$".find(c).unwrap_or(0))
            }
            KeyCode::Char('m') => return Action::Memory,
            KeyCode::Char('i') => DeckCommand::LoopIn,
            KeyCode::Char('o') => DeckCommand::LoopOut,
            KeyCode::Char('l') => DeckCommand::ReloopExit,
            KeyCode::Char('b') => DeckCommand::BeatLoop(4.0),
            KeyCode::Char('[') => DeckCommand::LoopResize(0.5),
            KeyCode::Char(']') => DeckCommand::LoopResize(2.0),
            KeyCode::Char(',') => DeckCommand::BeatJump(-4.0),
            KeyCode::Char('.') => DeckCommand::BeatJump(4.0),
            _ => return Action::None,
        })
    }
}

/// Restores the terminal on exit, including on panic unwinding.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
