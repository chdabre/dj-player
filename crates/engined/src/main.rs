//! `engined <file>`: play one track on one deck through the real engine.
//!
//! Keys: space play/pause, ←/→ seek ±5 s, +/- rate ±1 %, 0 reset rate, k keylock,
//! r reverse, s slip, q quit. Prints the deck state and the callback load meter once per second.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use engine_core::{DeckCommand, DeckGrid, DeckState, Engine, EngineConfig, LoadMeter, Source};
use engine_io::{AudioBackend, AudioStream, CpalBackend, StreamRequest};

/// Debug builds abort on any allocation inside `assert_no_alloc` scopes (the audio callback).
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const DECK: usize = 0;
const SEEK_SECONDS: f64 = 5.0;
const RATE_STEP: f64 = 0.01;

const USAGE: &str = "\
usage: engined <file> [--device <name>] [--buffer <frames>] [--bpm-range MIN-MAX] [--list-devices]

Set TEMPOCNN_MODEL to the TempoCNN .onnx model for model-based tempo (see docs/plan/analysis.md).

keys:  space play/pause   <-/-> seek -/+5 s   +/- rate +/-1 %   0 reset rate
       k keylock   r reverse   s slip   q quit";

struct Args {
    file: Option<PathBuf>,
    request: StreamRequest,
    list_devices: bool,
    settings: engine_analysis::Settings,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        file: None,
        request: StreamRequest::default(),
        list_devices: false,
        settings: engine_analysis::Settings::default(),
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
                out.settings.bpm_range = Some(
                    engine_analysis::BpmRange::parse(&r).ok_or(format!("bad BPM range {r:?}"))?,
                );
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

    // The device decides the engine rate; tracks are resampled to it at load.
    let format = backend.format(&args.request)?;
    println!(
        "{} output: {} Hz, {} channels, buffer {}",
        backend.host_name(),
        format.sample_rate,
        format.channels,
        args.request
            .buffer_frames
            .map_or("default".to_string(), |n| format!("{n} frames")),
    );

    let loading = engine_media::load_file(&file, format.sample_rate)?;
    println!(
        "loading {} ({} Hz, {} ch{})",
        file.display(),
        loading.info.file_sample_rate,
        loading.info.file_channels,
        if loading.info.file_sample_rate == format.sample_rate {
            ""
        } else {
            ", resampling"
        },
    );
    let source = Arc::clone(&loading.source);
    let mut loader = Some(loading.thread);

    let (mut engine, mut handle) = Engine::new(EngineConfig {
        sample_rate: format.sample_rate,
        channels: format.channels,
        decks: 1,
    });
    handle.send_deck(DECK, DeckCommand::Load(Arc::clone(&source)))?;
    let analysis = analyze_in_background(file.clone(), format.sample_rate, args.settings);

    let meter = Arc::clone(handle.load_meter());
    let ns_per_frame = 1e9 / f64::from(format.sample_rate);
    let callback = move |out: &mut [f32], frames: usize| {
        let start = Instant::now();
        assert_no_alloc::assert_no_alloc(|| engine.process(out, frames));
        let busy = start.elapsed().as_nanos() as u64;
        meter.record(busy, (frames as f64 * ns_per_frame) as u64);
    };
    let stream = backend.start(&args.request, Box::new(callback))?;
    println!("{}", stream.device_name());

    let interactive = std::io::stdin().is_terminal() && terminal::enable_raw_mode().is_ok();
    let _raw = interactive.then_some(RawModeGuard);
    if interactive {
        out(
            "space play/pause  <-/-> seek 5 s  +/- rate 1 %  0 reset  k keylock  r reverse  \
             s slip  q quit",
        );
    } else {
        out("stdin is not a terminal: playing to the end without key control");
        handle.send_deck(DECK, DeckCommand::Play)?;
    }

    let state = Arc::clone(handle.deck_state(DECK));
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
                    Action::Send(cmd) => {
                        if handle.send_deck(DECK, cmd).is_err() {
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
                report(&state, handle.load_meter(), &source, format.sample_rate);
                break;
            }
        }
        if let Ok(result) = analysis.try_recv() {
            match result {
                Ok(done) => {
                    out(&done.summary);
                    if let Some(grid) = done.grid {
                        let cmd = DeckCommand::SetGrid {
                            source_id: source.id(),
                            grid,
                        };
                        if handle.send_deck(DECK, cmd).is_err() {
                            out("command ring full, grid dropped");
                        }
                    }
                }
                Err(e) => out(&format!("analysis: {e}")),
            }
        }
        handle.collect_retired();
        if loader.as_ref().is_some_and(|t| t.is_finished())
            && let Some(Err(e)) = loader.take().and_then(|t| t.join().ok())
        {
            out(&format!("loader: {e} (playing what was decoded)"));
        }
        for e in stream.take_errors() {
            out(&format!("stream: {e}"));
        }
        if Instant::now() >= next_report {
            next_report += Duration::from_secs(1);
            report(&state, handle.load_meter(), &source, format.sample_rate);
        }
    }
    // Stop the stream first: that drops the engine (and its sources) off the audio thread.
    drop(stream);
    handle.collect_retired();
    Ok(())
}

/// What the analysis thread hands back: the deck grid (engine frames) and a line to print.
struct Analysed {
    grid: Option<Arc<DeckGrid>>,
    summary: String,
}

/// Analyse `path` on its own thread: tempo (TempoCNN, if `TEMPOCNN_MODEL` is set) refined by
/// gridfit, the grid, and the key. The result arrives on the returned channel.
fn analyze_in_background(
    path: PathBuf,
    engine_rate: u32,
    settings: engine_analysis::Settings,
) -> std::sync::mpsc::Receiver<Result<Analysed, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || {
            let _ = tx.send(analyze(&path, engine_rate, settings));
        });
    if let Err(e) = spawned {
        out(&format!("analysis thread: {e}"));
    }
    rx
}

fn analyze(
    path: &std::path::Path,
    engine_rate: u32,
    settings: engine_analysis::Settings,
) -> Result<Analysed, String> {
    use engine_analysis::{Analysis, Analyzer, CancelFlag, FeatureSet, GridFit, GridFitConfig};
    let started = Instant::now();
    let audio = engine_analysis::load(path).map_err(|e| e.to_string())?;
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
    let results =
        engine_analysis::run_all(&mut analyzers, &audio, FeatureSet::ALL, &CancelFlag::new())
            .map_err(|e| e.to_string())?;
    // Later analyzers refine earlier ones (gridfit's tempo over TempoCNN's).
    let mut merged = Analysis::default();
    for (_, r) in results {
        let r = r.map_err(|e| e.to_string())?;
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
    let summary = format!(
        "analysed in {:.1} s: {}, key {}, first downbeat {}{}",
        started.elapsed().as_secs_f64(),
        bpm.map_or("no tempo".into(), |b| format!("{b:.2} BPM")),
        merged.key.map_or("?".into(), |k| k.value.to_string()),
        first_down.map_or("?".into(), |t| format!("{t:.3} s")),
        if hint.is_none() {
            " (TEMPOCNN_MODEL not set: gridfit alone)"
        } else {
            ""
        },
    );
    Ok(Analysed { grid, summary })
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
    let mut line = format!(
        "{} / {}  rate {:+.2} %{tempo}  {} {} {} {}  load avg {:4.1} % peak {:4.1} % ({} cb)",
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
    Send(DeckCommand),
}

/// Maps keys to deck commands. Rate is kept here in whole 1 % steps and sent as an absolute
/// value, so repeated presses do not accumulate floating-point error.
struct Controls {
    sample_rate: f64,
    rate_steps: i32,
}

impl Controls {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            rate_steps: 0,
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
