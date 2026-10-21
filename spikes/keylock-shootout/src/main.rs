//! Keylock shootout (spike 2 in `docs/research/engine.md`).
//!
//! ```text
//! keylock-shootout bench [--track FILE] [--seconds N]
//! keylock-shootout render --out DIR FILE...
//! keylock-shootout compare
//! ```
//!
//! `bench` plays each candidate in 128-frame blocks at 0.92x, 1.0x, 1.08x, the WIDE extremes
//! (0.1x, 2x) and reverse (where supported), and reports per-block CPU against the 1.3 ms
//! half-budget, start (seek) cost, allocations in the audio path (macOS), alignment with the
//! playhead, how fast a rate change is heard, and how far ahead each one reads.
//!
//! `render` writes blind-labelled 20 s excerpts of each track at 0.92x and 1.08x for an A/B
//! listen, with the answer key in a separate `key.csv`.

mod allocs;
mod bungee;
mod candidate;
mod compare;
mod rubberband;
mod track;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use candidate::{Candidate, Signalsmith};
use rubberband::{Engine, RubberBand};
use track::{CHANNELS, Track};

pub const SR: u32 = 48_000;
const BLOCK: usize = 128;
/// Half of a 128-frame callback at 48 kHz, the deck + mixer budget from the research doc.
const BUDGET_US: f64 = 1333.0;

fn candidates() -> Vec<Box<dyn Candidate>> {
    vec![
        Box::new(Signalsmith::default_preset(SR)),
        Box::new(Signalsmith::cheaper_preset(SR)),
        Box::new(bungee::Bungee::cpp(SR)),
        Box::new(bungee::Bungee::rust(SR)),
        Box::new(RubberBand::new(Engine::R2, SR)),
        Box::new(RubberBand::new(Engine::R3, SR)),
        Box::new(RubberBand::new(Engine::R3Short, SR)),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench") => bench(&args[1..]),
        Some("render") => render(&args[1..]),
        Some("compare") => compare::compare(&args[1..]),
        _ => Err(
            "usage: keylock-shootout bench [--track FILE] [--seconds N]\n       \
                  keylock-shootout render --out DIR FILE...\n       \
                  keylock-shootout compare"
                .into(),
        ),
    };
    if let Err(e) = result {
        eprintln!("keylock-shootout: {e}");
        std::process::exit(2);
    }
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------------------------
// bench

fn bench(args: &[String]) -> Result<()> {
    let mut track_path = None;
    let mut seconds = 20.0;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--track" => track_path = Some(PathBuf::from(it.next().ok_or("--track FILE")?)),
            "--seconds" => seconds = it.next().ok_or("--seconds N")?.parse()?,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let counting = allocs::install();
    let music = match &track_path {
        Some(p) => Track::load(p, SR)?,
        None => Track::from_samples("generated", generated_music(120.0)),
    };
    let bursts = Track::from_samples("bursts", burst_train(60.0));

    let mut report = String::new();
    writeln!(
        report,
        "# Keylock shootout\n\n{} {}, {} Hz, {BLOCK}-frame blocks, {seconds} s per row, CPU \
         track: {}.\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        SR,
        music.name
    )?;
    writeln!(
        report,
        "## CPU per block\n\nBudget: {BUDGET_US:.0} µs per block (half a callback). \
         Allocations count every malloc/free in `render` after warm-up{}.\n",
        if counting {
            ""
        } else {
            " (not available on this OS)"
        }
    )?;
    writeln!(
        report,
        "| candidate | rate | mean µs | p99 µs | max µs | mean % of budget | start µs | \
         allocs start / run | read-ahead ms | underruns |\n| --- | --- | --- | --- | --- | --- \
         | --- | --- | --- | --- |"
    )?;
    let rates = [0.92, 1.0, 1.08, 0.1, 2.0, -1.0];
    let blocks = (seconds * f64::from(SR)) as usize / BLOCK;
    let mut times = Vec::with_capacity(blocks);
    let mut out = vec![0.0f32; BLOCK * CHANNELS];
    for mut c in candidates() {
        for &rate in &rates {
            if rate < 0.0 && !c.reverse() {
                writeln!(
                    report,
                    "| {} | {rate} | — | — | — | — | — | — | — | resampler fallback |",
                    c.name()
                )?;
                continue;
            }
            // Reverse needs room behind the start; the slow rate needs little ahead.
            let start = (music.len_frames() as f64 * 0.5).min(60.0 * f64::from(SR));
            let t0 = Instant::now();
            let ((), start_allocs) = allocs::count(counting, || c.start(&music, start, rate));
            let start_us = t0.elapsed().as_secs_f64() * 1e6;
            for _ in 0..(SR as usize / BLOCK) {
                c.render(&music, rate, &mut out);
            }
            let under0 = c.underruns();
            times.clear();
            let ((), run_allocs) = allocs::count(counting, || {
                for _ in 0..blocks {
                    let t = Instant::now();
                    c.render(&music, rate, &mut out);
                    times.push(t.elapsed().as_nanos() as u64);
                    std::hint::black_box(&out);
                }
            });
            let s = Stats::of(&mut times);
            writeln!(
                report,
                "| {} | {rate} | {:.1} | {:.1} | {:.1} | {:.1} % | {:.0} | {} / {} | {:.1} | {} |",
                c.name(),
                s.mean_us,
                s.p99_us,
                s.max_us,
                s.mean_us / BUDGET_US * 100.0,
                start_us,
                opt(start_allocs),
                opt(run_allocs),
                c.read_ahead() * 1000.0 / f64::from(SR),
                c.underruns() - under0,
            )?;
            eprintln!("{} at {rate}: {:.1} µs mean", c.name(), s.mean_us);
        }
    }

    writeln!(
        report,
        "\n## Timing\n\nBurst train (Hann-windowed 1 kHz bursts every 500 ms). Alignment = \
         where the burst is heard minus where the playhead says it is (mean, worst). Rate \
         response = delay before a 1.0 → 1.25 change is heard.\n"
    )?;
    writeln!(
        report,
        "| candidate | align 0.92 ms | align 1.08 ms | align reverse ms | rate response ms |\n\
         | --- | --- | --- | --- | --- |"
    )?;
    for mut c in candidates() {
        let a092 = alignment(c.as_mut(), &bursts, 0.92);
        let a108 = alignment(c.as_mut(), &bursts, 1.08);
        let arev = c.reverse().then(|| alignment(c.as_mut(), &bursts, -1.0));
        let response = rate_response(c.as_mut(), &bursts);
        writeln!(
            report,
            "| {} | {} | {} | {} | {:.1} |",
            c.name(),
            a092,
            a108,
            arev.map_or("—".into(), |a| a.to_string()),
            response
        )?;
    }
    println!("{report}");
    Ok(())
}

fn opt(n: Option<u64>) -> String {
    n.map_or("n/a".into(), |n| n.to_string())
}

struct Stats {
    mean_us: f64,
    p99_us: f64,
    max_us: f64,
}

impl Stats {
    fn of(ns: &mut [u64]) -> Self {
        ns.sort_unstable();
        let mean = ns.iter().sum::<u64>() as f64 / ns.len() as f64;
        let p99 = ns[(ns.len() as f64 * 0.99) as usize];
        Stats {
            mean_us: mean / 1e3,
            p99_us: p99 as f64 / 1e3,
            max_us: *ns.last().unwrap() as f64 / 1e3,
        }
    }
}

const BURST_EVERY: f64 = 0.5;
const BURST_LEN: f64 = 0.04;

/// Hann-windowed 1 kHz bursts, centred at `(k + 0.5) * BURST_EVERY` seconds.
fn burst_train(seconds: f64) -> Vec<f32> {
    let sr = f64::from(SR);
    let frames = (seconds * sr) as usize;
    let len = (BURST_LEN * sr) as usize;
    let mut s = vec![0.0f32; frames * CHANNELS];
    let mut k = 0;
    loop {
        let centre = ((k as f64 + 0.5) * BURST_EVERY * sr) as usize;
        if centre + len >= frames {
            break;
        }
        for i in 0..len {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos();
            let x = (0.5 * w * (std::f64::consts::TAU * 1000.0 * i as f64 / sr).sin()) as f32;
            let at = (centre - len / 2 + i) * CHANNELS;
            s[at] = x;
            s[at + 1] = x;
        }
        k += 1;
    }
    s
}

fn burst_centres(track: &Track) -> impl Iterator<Item = f64> {
    let sr = f64::from(SR);
    let n = (track.len_frames() as f64 / (BURST_EVERY * sr)) as usize;
    (0..n).map(move |k| (k as f64 + 0.5) * BURST_EVERY * sr)
}

/// Energy centroid (output frame) of the left channel within `±window` of `around`.
fn centroid(out: &[f32], around: f64, window: f64) -> Option<f64> {
    let lo = (around - window).max(0.0) as usize;
    let hi = ((around + window) as usize).min(out.len() / CHANNELS);
    let (mut sum, mut weighted) = (0.0, 0.0);
    for k in lo..hi {
        let e = f64::from(out[k * CHANNELS]).powi(2);
        sum += e;
        weighted += e * k as f64;
    }
    (sum > 1e-6).then(|| weighted / sum)
}

fn play(c: &mut dyn Candidate, track: &Track, start: f64, rates: &[(usize, f64)]) -> Vec<f32> {
    let total: usize = rates.iter().map(|r| r.0).sum();
    let mut out = vec![0.0; total * CHANNELS];
    c.start(track, start, rates[0].1);
    let mut done = 0;
    for &(frames, rate) in rates {
        let end = done + frames;
        while done < end {
            let n = BLOCK.min(end - done);
            c.render(
                track,
                rate,
                &mut out[done * CHANNELS..(done + n) * CHANNELS],
            );
            done += n;
        }
    }
    out
}

struct Alignment {
    mean_ms: f64,
    worst_ms: f64,
}

impl std::fmt::Display for Alignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:+.1} ({:+.1})", self.mean_ms, self.worst_ms)
    }
}

fn alignment(c: &mut dyn Candidate, bursts: &Track, rate: f64) -> Alignment {
    let sr = f64::from(SR);
    let start = 30.25 * sr;
    let frames = (8.0 * sr) as usize;
    let out = play(c, bursts, start, &[(frames, rate)]);
    let errors: Vec<f64> = burst_centres(bursts)
        .map(|b| (b - start) / rate)
        .filter(|&t| t > 0.3 * sr && t < frames as f64 - 0.3 * sr)
        .filter_map(|t| centroid(&out, t, 0.15 * sr).map(|m| (m - t) * 1000.0 / sr))
        .collect();
    let mean = errors.iter().sum::<f64>() / errors.len().max(1) as f64;
    let worst = errors
        .iter()
        .copied()
        .fold(0.0f64, |w, e| if e.abs() > w.abs() { e } else { w });
    Alignment {
        mean_ms: mean,
        worst_ms: worst,
    }
}

/// Delay (ms) before a rate change from 1.0 to 1.25 is heard.
///
/// If the new rate takes effect `D` late, the content runs `0.25 * D` behind the ideal
/// playhead from then on, so every later burst arrives `0.25 * D / 1.25` late.
fn rate_response(c: &mut dyn Candidate, bursts: &Track) -> f64 {
    let sr = f64::from(SR);
    let start = 10.25 * sr;
    let before = 2 * SR as usize;
    let after = 4 * SR as usize;
    let out = play(c, bursts, start, &[(before, 1.0), (after, 1.25)]);
    let t_change = before as f64;
    let p_change = start + t_change;
    let mut base = Vec::new();
    let mut late = Vec::new();
    for b in burst_centres(bursts) {
        if b < p_change {
            let t = b - start;
            if t > 0.3 * sr
                && t < t_change - 0.3 * sr
                && let Some(m) = centroid(&out, t, 0.15 * sr)
            {
                base.push(m - t);
            }
        } else {
            let t = t_change + (b - p_change) / 1.25;
            if t > t_change + 1.0 * sr
                && t < (before + after) as f64 - 0.3 * sr
                && let Some(m) = centroid(&out, t, 0.15 * sr)
            {
                late.push(m - t);
            }
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    (mean(&late) - mean(&base)) * 1.25 / 0.25 * 1000.0 / sr
}

/// Something music-like for the CPU rows when no track is given: a kick every beat at
/// `bpm`, a hat on the off-beat and a chord pad, so the stretchers never see silence.
fn generated_music(bpm: f64) -> Vec<f32> {
    let sr = f64::from(SR);
    let frames = (120.0 * sr) as usize;
    let beat = 60.0 / bpm * sr;
    let mut noise = 0x1234_5678u32;
    (0..frames)
        .flat_map(|i| {
            let t = i as f64 / sr;
            let in_beat = (i as f64 % beat) / sr;
            let kick = (-in_beat * 30.0).exp() * (std::f64::consts::TAU * 55.0 * in_beat).sin();
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let off = ((i as f64 + beat / 2.0) % beat) / sr;
            let hat = (-off * 200.0).exp() * (f64::from(noise) / f64::from(u32::MAX) - 0.5);
            let pad: f64 = [220.0, 277.18, 329.63]
                .iter()
                .map(|f| (std::f64::consts::TAU * f * t).sin())
                .sum::<f64>()
                / 3.0;
            let x = (0.5 * kick + 0.2 * hat + 0.15 * pad) as f32;
            [x, x]
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// render

const EXCERPT_SECONDS: f64 = 20.0;
const LISTEN_RATES: [f64; 2] = [0.92, 1.08];

fn render(args: &[String]) -> Result<()> {
    let mut out_dir = None;
    let mut files = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = Some(PathBuf::from(it.next().ok_or("--out DIR")?)),
            _ => files.push(PathBuf::from(a)),
        }
    }
    let out_dir = out_dir.ok_or("--out DIR is required")?;
    if files.is_empty() {
        return Err("no tracks given".into());
    }
    std::fs::create_dir_all(&out_dir)?;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos() as u64
        | 1;
    let mut rng = XorShift(seed);
    let mut key = format!("# seed {seed}\ntrack,file,rate,label,candidate\n");
    let mut sheet = String::from("track,rate,label,score (1-5),notes\n");
    let frames = (EXCERPT_SECONDS * f64::from(SR)) as usize;
    for (n, path) in files.iter().enumerate() {
        let no = n + 1;
        let track = Track::load(path, SR)?;
        let start = (60.0 * f64::from(SR))
            .min(track.len_frames() as f64 / 3.0)
            .round();
        let mut reference = vec![0.0; frames * CHANNELS];
        track.copy_interleaved(start as i64, &mut reference);
        write_wav(&out_dir.join(format!("{no:02}-ref.wav")), &reference)?;
        for &rate in &LISTEN_RATES {
            let mut cands = candidates();
            rng.shuffle(&mut cands);
            for (i, mut c) in cands.into_iter().enumerate() {
                let label = (b'A' + i as u8) as char;
                let audio = play(c.as_mut(), &track, start, &[(frames, rate)]);
                write_wav(&out_dir.join(format!("{no:02}-{rate}-{label}.wav")), &audio)?;
                writeln!(key, "{no},{},{rate},{label},{}", csv(&track.name), c.name())?;
                writeln!(sheet, "{no},{rate},{label},,")?;
            }
        }
        eprintln!("{no:02} {}", track.name);
    }
    std::fs::write(out_dir.join("key.csv"), key)?;
    std::fs::write(out_dir.join("listening-sheet.csv"), sheet)?;
    println!(
        "wrote {} tracks to {}. Score listening-sheet.csv before opening key.csv.",
        files.len(),
        out_dir.display()
    );
    Ok(())
}

fn csv(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn write_wav(path: &Path, interleaved: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: CHANNELS as u16,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &x in interleaved {
        w.write_sample((x.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
    }
    w.finalize()?;
    Ok(())
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}
