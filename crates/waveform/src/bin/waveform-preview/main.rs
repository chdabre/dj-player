//! `waveform-preview [OPTIONS] FILE`: compute a file's waveform, fit a beatgrid (gridfit)
//! and write one PNG sheet per display mode: the overview on top, then a 20-beat zoom at each
//! position. Prints the compute time and blob sizes.
//!
//! `waveform-preview [OPTIONS] --synth track|tones|sweep`: the same for a synthetic signal
//! (the previews in `docs/waveforms/` come from these; no real music in the repo).
//!
//! Options: `--out DIR` (default `.`), `--at F,F,...` playhead positions as fractions of the
//! track (default 0.15,0.42,0.65), `--bpm N` a constant grid from 0 s instead of gridfit,
//! `--rate HZ` the synthetic signal's sample rate (default 44100), `--no-markers` to leave
//! out the demo cue, hot cue and loop overlays.

mod synth;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use engine_analysis::{Analyzer, CancelFlag, Feature, FeatureSet, GridFit, TrackAudio, load};
use waveform::compute::{Params, compute};
use waveform::render::{
    Canvas, GridBeat, HotCue, Loop, Markers, OverviewView, ZoomView, render_overview, render_zoom,
    theme,
};
use waveform::{Mode, Style};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Options {
    input: Input,
    out: PathBuf,
    at: Vec<f64>,
    bpm: Option<f64>,
    rate: u32,
    markers: bool,
}

enum Input {
    File(PathBuf),
    Synth(String),
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        input: Input::Synth(String::new()),
        out: PathBuf::from("."),
        at: vec![0.15, 0.42, 0.65],
        bpm: None,
        rate: 44_100,
        markers: true,
    };
    let mut input = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => o.out = PathBuf::from(value()?),
            "--at" => {
                o.at = value()?
                    .split(',')
                    .map(str::parse)
                    .collect::<std::result::Result<_, _>>()?
            }
            "--bpm" => o.bpm = Some(value()?.parse()?),
            "--rate" => o.rate = value()?.parse()?,
            "--synth" => input = Some(Input::Synth(value()?.clone())),
            "--no-markers" => o.markers = false,
            f if !f.starts_with("--") => input = Some(Input::File(PathBuf::from(f))),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    o.input = input.ok_or("no FILE or --synth")?;
    Ok(o)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: waveform-preview [--out DIR] [--at F,F,...] [--bpm N] [--no-markers] FILE\n       \
             waveform-preview [--out DIR] [--rate HZ] [...] --synth track|tones|sweep"
        );
        return ExitCode::from(2);
    }
    match parse(&args).and_then(|o| run(&o)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("waveform-preview: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(o: &Options) -> Result<()> {
    let started = Instant::now();
    let (audio, name, default_bpm) = match &o.input {
        Input::File(p) => (load(p)?, stem(p), None),
        Input::Synth(kind) => {
            let (stereo, bpm) = match kind.as_str() {
                "track" => (synth::track(o.rate), None),
                "tones" => (synth::tones(o.rate), Some(120.0)),
                "sweep" => (synth::sweep(o.rate), Some(120.0)),
                _ => return Err(format!("unknown synthetic signal {kind}").into()),
            };
            let audio = TrackAudio {
                sample_rate: o.rate,
                stereo,
            };
            (audio, format!("synth-{kind}"), bpm)
        }
    };
    let decode_s = started.elapsed().as_secs_f64();

    let started = Instant::now();
    let wf = compute(&audio.stereo, audio.sample_rate, &Params::default());
    let compute_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let ov = wf.overview();
    let overview_s = started.elapsed().as_secs_f64();
    let detail_blob = wf.to_blob();
    let overview_blob = ov.to_blob(&wf.params, wf.provenance);

    let started = Instant::now();
    let grid = match o.bpm.or(default_bpm) {
        Some(bpm) => constant_grid(bpm, wf.duration),
        None => fitted_grid(&audio)?,
    };
    let grid_s = started.elapsed().as_secs_f64();

    let minutes = wf.duration / 60.0;
    println!(
        "{name}: {:.1} s at {} Hz, {} entries, {} grid beats",
        wf.duration,
        audio.sample_rate,
        wf.entries.len(),
        grid.len()
    );
    println!("  decode/synth {decode_s:.3} s, gridfit {grid_s:.3} s");
    println!(
        "  waveform {:.1} ms ({:.1} ms per minute of audio), overview {:.3} ms",
        compute_s * 1e3,
        compute_s * 1e3 / minutes,
        overview_s * 1e3
    );
    println!(
        "  blobs: detail {} B, overview {} B ({:.0} B per 5 minutes of detail)",
        detail_blob.len(),
        overview_blob.len(),
        detail_blob.len() as f64 / minutes * 5.0
    );

    let positions: Vec<f64> = o.at.iter().map(|f| f * wf.duration).collect();
    let markers = if o.markers {
        demo_markers(&grid, &positions)
    } else {
        Markers::default()
    };
    let style = Style::default();
    let zoom = ZoomView::default();
    let overview = OverviewView::default();
    std::fs::create_dir_all(&o.out)?;
    for mode in Mode::ALL {
        let gap = 8;
        let height = overview.height + positions.len() * (zoom.height + gap);
        let mut sheet = Canvas::new(zoom.width, height, theme::BORDER);
        let playhead = positions.first().copied().unwrap_or(0.0);
        let canvas = render_overview(&ov, &grid, playhead, &markers, mode, &style, &overview);
        sheet.blit(&canvas, 0, 0);
        for (i, &t) in positions.iter().enumerate() {
            let canvas = render_zoom(&wf, &grid, t, &markers, mode, &style, &zoom);
            sheet.blit(&canvas, 0, overview.height + gap + i * (zoom.height + gap));
        }
        let path = o.out.join(format!("{name}-{}.png", mode.name()));
        write_png(&path, &sheet)?;
        println!("  wrote {}", path.display());
    }
    Ok(())
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "track".into())
}

fn to_grid(beats: &[engine_analysis::Beat]) -> Vec<GridBeat> {
    let bars = beats.iter().any(|b| b.bar_pos != 0);
    beats
        .iter()
        .enumerate()
        .map(|(k, b)| GridBeat {
            time: b.time,
            downbeat: if bars { b.bar_pos == 1 } else { k % 4 == 0 },
        })
        .collect()
}

fn fitted_grid(audio: &TrackAudio) -> Result<Vec<GridBeat>> {
    let analysis = GridFit::default().analyze(
        audio,
        FeatureSet::EMPTY.with(Feature::Beatgrid),
        &[],
        &CancelFlag::new(),
    )?;
    let grid = analysis.beatgrid.ok_or("gridfit found no grid")?;
    Ok(to_grid(&grid.value.beats))
}

fn constant_grid(bpm: f64, duration: f64) -> Vec<GridBeat> {
    to_grid(&engine_analysis::Beatgrid::constant(0.0, bpm, duration).beats)
}

/// Overlays that show every marker style: a cue just behind the first position, a memory
/// cue ahead of it; a hot cue behind the second and a 4-beat loop ahead of it; two hot cues
/// ahead of the third.
fn demo_markers(grid: &[GridBeat], positions: &[f64]) -> Markers {
    // The downbeat at or after `t`, `beats` beats on (negative: back).
    let beat_from = |t: f64, beats: isize| {
        let i = grid.partition_point(|b| b.time < t);
        let i = (i..grid.len()).find(|&i| grid[i].downbeat).unwrap_or(i) as isize + beats;
        grid.get(i.clamp(0, grid.len() as isize - 1) as usize)
            .map_or(t, |b| b.time)
    };
    let mut m = Markers::default();
    if let Some(&p) = positions.first() {
        m.cue = Some(beat_from(p, -4));
        m.memory_cues.push(beat_from(p, 4));
    }
    if let Some(&p) = positions.get(1) {
        m.hot_cues.push(HotCue {
            time: beat_from(p, -8),
            label: 'A',
            color: theme::PADS[0],
        });
        m.active_loop = Some(Loop {
            start: beat_from(p, 0),
            end: beat_from(p, 4),
            beats: 4,
        });
    }
    if let Some(&p) = positions.get(2) {
        for (i, (beats, label)) in [(0, 'B'), (4, 'C')].into_iter().enumerate() {
            m.hot_cues.push(HotCue {
                time: beat_from(p, beats),
                label,
                color: theme::PADS[1 + i],
            });
        }
    }
    m
}

fn write_png(path: &Path, canvas: &Canvas) -> Result<()> {
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        canvas.width as u32,
        canvas.height as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&canvas.to_rgb8())?;
    writer.finish()?;
    Ok(())
}
