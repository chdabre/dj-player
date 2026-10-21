//! `engine-analyze FILE...`: analyse tracks and print tempo, key and grid.
//! `engine-analyze score TRUTH.csv`: analyse the tracks a ground-truth CSV lists and score
//! them. The CSV has a header and the columns `file,bpm,key` (paths relative to the CSV;
//! empty cells are not scored).
//! `engine-analyze rekordbox EXPORT.xml [--min-seconds N] [--limit N] [--only A,B]
//! [--overrides FILE] [--playlist NAME]`: score
//! against the
//! tempo, grid and key of every track Rekordbox analysed whose file is present.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use engine_analysis::score::{TrackScore, Truth, report, score_track};
use engine_analysis::{Analysis, CancelFlag, FeatureSet, Key, TrackAudio, analyzers, load};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            eprintln!(
                "usage: engine-analyze FILE...\n       engine-analyze score TRUTH.csv\n       \
                 engine-analyze rekordbox EXPORT.xml [--min-seconds N] [--limit N] [--only A,B] \
                 [--overrides FILE] [--playlist NAME]"
            );
            return ExitCode::from(2);
        }
        Some("score") => match args.get(1) {
            Some(csv) => score(Path::new(csv)),
            None => Err("score needs a CSV file".into()),
        },
        Some("rekordbox") => rekordbox(&args[1..]),
        Some(_) => analyze(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("engine-analyze: {e}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Run every compiled-in analyzer on one file; return (name, analysis) per analyzer.
fn run(path: &Path) -> Result<Vec<(String, Analysis)>> {
    Ok(run_audio(&load(path)?, &[])?
        .into_iter()
        .map(|(n, a, _)| (n, a))
        .collect())
}

/// Like [`run`], with each analyzer's wall time in seconds.
fn run_audio(audio: &TrackAudio, only: &[String]) -> Result<Vec<(String, Analysis, f64)>> {
    let mut out = Vec::new();
    // What earlier analyzers found, as a hint for later ones (never the reference data).
    let mut context: Vec<Analysis> = Vec::new();
    for mut a in selected(only) {
        let started = Instant::now();
        // One analyzer failing on one track is reported, not fatal for a long run.
        let analysis = match a.analyze(audio, FeatureSet::ALL, &context, &CancelFlag::new()) {
            Ok(analysis) => analysis,
            Err(e) => {
                eprintln!("  {}: {e}", a.id().name);
                continue;
            }
        };
        context.push(analysis.clone());
        let secs = started.elapsed().as_secs_f64();
        eprintln!(
            "  {}: {:.1} s for {:.0} s of audio ({:.0}x real time)",
            a.id().name,
            secs,
            audio.duration(),
            audio.duration() / secs
        );
        out.push((a.id().name, analysis, secs));
    }
    Ok(out)
}

/// The compiled-in analyzers, or only those named in `only` (when not empty).
fn selected(only: &[String]) -> Vec<Box<dyn engine_analysis::Analyzer>> {
    analyzers()
        .into_iter()
        .filter(|a| only.is_empty() || only.contains(&a.id().name))
        .collect()
}

fn analyze(files: &[String]) -> Result<()> {
    for f in files {
        println!("{f}");
        for (name, a) in run(Path::new(f))? {
            let conf = |c: Option<f32>| c.map_or(String::new(), |c| format!(" (conf {c:.2})"));
            if let Some(t) = &a.tempo {
                println!(
                    "  {name} tempo {:.2} BPM{}",
                    t.value.bpm,
                    conf(t.confidence)
                );
            }
            if let Some(k) = &a.key {
                println!("  {name} key {}{}", k.value, conf(k.confidence));
            }
            if let Some(g) = &a.beatgrid {
                let first_down = g.value.beats.iter().find(|b| b.bar_pos == 1);
                println!(
                    "  {name} grid {} beats, median {:.2} BPM, first downbeat at {}{}",
                    g.value.beats.len(),
                    g.value.median_bpm().unwrap_or(0.0),
                    first_down.map_or("—".into(), |b| format!("{:.3} s", b.time)),
                    conf(g.confidence)
                );
            }
        }
    }
    Ok(())
}

fn score(csv: &Path) -> Result<()> {
    let dir = csv.parent().unwrap_or(Path::new("."));
    let text = std::fs::read_to_string(csv)?;
    let mut rows = text.lines().filter(|l| !l.trim().is_empty());
    rows.next().ok_or("empty CSV")?;
    let mut tracks: Vec<(PathBuf, Truth)> = Vec::new();
    for (n, line) in rows.enumerate() {
        let cells = split_csv(line);
        let cell = |i: usize| cells.get(i).map(|s| s.trim()).filter(|s| !s.is_empty());
        let file = cell(0).ok_or_else(|| format!("row {}: no file", n + 2))?;
        let tempo = cell(1).map(str::parse::<f64>).transpose()?;
        let key = match cell(2) {
            Some(k) => Some(Key::parse(k).ok_or_else(|| format!("row {}: bad key {k:?}", n + 2))?),
            None => None,
        };
        tracks.push((
            dir.join(file),
            Truth {
                tempo,
                beatgrid: None,
                key,
            },
        ));
    }
    let mut by_analyzer: Vec<(String, Vec<_>)> = Vec::new();
    for (path, truth) in &tracks {
        eprintln!("{}", path.display());
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        for (analyzer, a) in run(path)? {
            let s = score_track(&name, &a, truth);
            match by_analyzer.iter_mut().find(|(n, _)| *n == analyzer) {
                Some((_, v)) => v.push(s),
                None => by_analyzer.push((analyzer, vec![s])),
            }
        }
    }
    for (analyzer, scores) in &by_analyzer {
        println!("{}", report(analyzer, scores));
    }
    Ok(())
}

fn rekordbox(args: &[String]) -> Result<()> {
    let mut xml = None;
    let mut min_seconds = 30.0;
    let mut limit = usize::MAX;
    let mut only: Vec<String> = Vec::new();
    let mut overrides = None;
    let mut playlist: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--min-seconds" => min_seconds = it.next().ok_or("--min-seconds N")?.parse()?,
            "--limit" => limit = it.next().ok_or("--limit N")?.parse()?,
            "--playlist" => playlist = Some(it.next().ok_or("--playlist NAME")?.clone()),
            "--overrides" => overrides = Some(PathBuf::from(it.next().ok_or("--overrides FILE")?)),
            "--only" => {
                only = it
                    .next()
                    .ok_or("--only NAME[,NAME...]")?
                    .split(',')
                    .map(str::to_string)
                    .collect()
            }
            _ => xml = Some(PathBuf::from(a)),
        }
    }
    let xml = xml.ok_or("rekordbox needs the XML export")?;
    let text = std::fs::read_to_string(&xml)?;
    let mut tracks = engine_analysis::rekordbox::read_collection(&text)?;
    if let Some(name) = &playlist {
        let ids = engine_analysis::rekordbox::read_playlist(&text, name)?
            .ok_or_else(|| format!("no playlist {name:?}"))?;
        tracks.retain(|t| ids.contains(&t.id));
    }
    if let Some(path) = &overrides {
        let o = engine_analysis::rekordbox::Overrides::parse(&std::fs::read_to_string(path)?)?;
        for (rule, hits) in o.apply(&mut tracks) {
            if hits != 1 {
                eprintln!("override {rule:?} matched {hits} tracks");
            }
        }
    }
    let usable: Vec<_> = tracks
        .iter()
        .filter(|t| !t.tempo.is_empty() && t.location.exists())
        .take(limit)
        .collect();
    eprintln!(
        "{} tracks in the collection, {} with a Rekordbox grid and a file present",
        tracks.len(),
        usable.len()
    );
    let mut by_analyzer: Vec<(String, Vec<TrackScore>)> = Vec::new();
    let mut timing: Vec<(String, f64)> = Vec::new();
    let mut audio_seconds = 0.0;
    let mut formats: Vec<String> = Vec::new();
    let mut skipped = 0;
    for (n, t) in usable.iter().enumerate() {
        let audio = load(&t.location)?;
        if audio.duration() < min_seconds {
            skipped += 1;
            continue;
        }
        let name = if t.artist.is_empty() {
            t.name.clone()
        } else {
            format!("{} – {}", t.artist, t.name)
        };
        eprintln!("[{}/{}] {name}", n + 1, usable.len());
        let truth = t.truth(audio.duration());
        audio_seconds += audio.duration();
        for (analyzer, a, secs) in run_audio(&audio, &only)? {
            match timing.iter_mut().find(|(n, _)| *n == analyzer) {
                Some((_, t)) => *t += secs,
                None => timing.push((analyzer.clone(), secs)),
            }
            let s = score_track(&name, &a, &truth);
            match by_analyzer.iter_mut().find(|(n, _)| *n == analyzer) {
                Some((_, v)) => v.push(s),
                None => by_analyzer.push((analyzer, vec![s])),
            }
        }
        formats.push(
            t.location
                .extension()
                .map_or_else(|| "?".into(), |e| e.to_string_lossy().to_lowercase()),
        );
    }
    eprintln!("skipped {skipped} shorter than {min_seconds} s");
    println!(
        "# Speed\n\n{:.0} s of audio in {} tracks.\n",
        audio_seconds,
        formats.len()
    );
    println!("| analyzer | total s | x real time | s per 5-min track |\n| --- | --- | --- | --- |");
    for (analyzer, secs) in &timing {
        println!(
            "| {analyzer} | {secs:.1} | {:.0} | {:.2} |",
            audio_seconds / secs,
            300.0 * secs / audio_seconds
        );
    }
    println!();
    for (analyzer, scores) in &by_analyzer {
        println!("{}", report(analyzer, scores));
        println!("## Median beat offset by format\n");
        let mut kinds: Vec<&String> = formats.iter().collect();
        kinds.sort();
        kinds.dedup();
        for kind in kinds {
            let mut offsets: Vec<f64> = scores
                .iter()
                .zip(&formats)
                .filter(|(_, f)| *f == kind)
                .filter_map(|(s, _)| s.beats.and_then(|b| b.median_offset_ms))
                .collect();
            offsets.sort_by(f64::total_cmp);
            match offsets.get(offsets.len() / 2) {
                Some(m) => println!("- {kind}: {m:+.1} ms over {} tracks", offsets.len()),
                None => println!("- {kind}: no matched beats"),
            }
        }
        println!();
    }
    Ok(())
}

/// Split one CSV line, honouring double quotes ("" inside quotes is a quote).
fn split_csv(line: &str) -> Vec<String> {
    let mut cells = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                cells.last_mut().unwrap().push('"');
            }
            ('"', _) => quoted = !quoted,
            (',', false) => cells.push(String::new()),
            _ => cells.last_mut().unwrap().push(c),
        }
    }
    cells
}
