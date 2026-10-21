//! `compare`: the pure-Rust Bungee port against the C++ library on the same input.
//!
//! Both are driven grain by grain with the same requests (a pre-roll, then `next` at a fixed
//! speed) and fed the same input chunks from synthetic signals. Float operation order differs
//! (rustfft vs kissfft, Eigen's vectorised maths), so the output is close rather than
//! bit-identical. Reported per signal and speed: whether every chunk's request tags and frame
//! count match, the error relative to the C++ output (SNR), the largest sample difference and
//! the short-time log-spectral difference.

use std::fmt::Write as _;
use std::sync::Arc;

use engine_stretch::bungee::Request;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::bungee::{Cpp, GrainApi, Rust};
use crate::track::{CHANNELS, Track};
use crate::{Result, SR};

/// Gain of the control run: the C++ against itself on input scaled by this. Bungee's
/// decisions are scale-invariant, so only float rounding changes; how far that moves the
/// output is the floor for any port.
const CONTROL_GAIN: f32 = 1.0 + 1.0 / 1_048_576.0;

/// Grains per case: about 6.8 s of output.
const GRAINS: usize = 640;
/// Short-time spectrum frame and hop for the spectral comparison.
const FRAME: usize = 2048;
const FRAME_HOP: usize = 1024;

struct Run {
    /// Planar output, one Vec per channel.
    out: [Vec<f32>; CHANNELS],
    tags: Vec<[Option<(f64, f64)>; 2]>,
    frames: Vec<usize>,
}

fn run(s: &mut impl GrainApi, track: &Track, position: f64, speed: f64, pitch: f64) -> Run {
    let stride = s.max_input_frame_count();
    let mut input = vec![0.0; stride * CHANNELS];
    let mut request = Request {
        position,
        speed,
        pitch,
        reset: true,
    };
    s.preroll(&mut request);
    let mut run = Run {
        out: [Vec::new(), Vec::new()],
        tags: Vec::new(),
        frames: Vec::new(),
    };
    for _ in 0..GRAINS {
        let (begin, end) = s.specify_grain(&request);
        let frames = ((end - begin).max(0) as usize).min(stride);
        track.copy_planar(begin, frames, &mut input, stride);
        s.analyse_grain(&mut input, stride);
        s.synthesise_grain(|data, n, channel_stride, tags| {
            for (c, out) in run.out.iter_mut().enumerate() {
                out.extend_from_slice(&data[c * channel_stride..][..n]);
            }
            run.tags
                .push(tags.map(|t| t.map(|r| (r.position, r.speed))));
            run.frames.push(n);
        });
        s.next(&mut request);
    }
    run
}

/// Log-magnitude short-time spectra of one channel, Hann-windowed.
fn spectra(fft: &Arc<dyn Fft<f32>>, x: &[f32]) -> Vec<Vec<f32>> {
    let window: Vec<f32> = (0..FRAME)
        .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FRAME as f64).cos()) as f32)
        .collect();
    let mut buf = vec![Complex32::default(); FRAME];
    let mut frames = Vec::new();
    let mut at = 0;
    while at + FRAME <= x.len() {
        for (b, (s, w)) in buf.iter_mut().zip(x[at..].iter().zip(&window)) {
            *b = Complex32::new(s * w, 0.0);
        }
        fft.process(&mut buf);
        frames.push(buf[..=FRAME / 2].iter().map(|c| c.norm()).collect());
        at += FRAME_HOP;
    }
    frames
}

struct Diff {
    tags_match: bool,
    frames_match: bool,
    snr_db: f64,
    max_abs: f32,
    spectral_mean_db: f64,
    spectral_p99_db: f64,
}

/// Equal, counting NaN as equal to NaN (invalid grains and unset speeds are NaN).
fn same(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

fn diff(cpp: &Run, rust: &Run, fft: &Arc<dyn Fft<f32>>) -> Diff {
    let tags_match = cpp.tags.len() == rust.tags.len()
        && cpp.tags.iter().zip(&rust.tags).all(|(a, b)| {
            a.iter().zip(b).all(|(x, y)| match (x, y) {
                (Some(x), Some(y)) => same(x.0, y.0) && same(x.1, y.1),
                (None, None) => true,
                _ => false,
            })
        });
    let frames_match = cpp.frames == rust.frames;
    let (mut signal, mut noise, mut max_abs) = (0.0f64, 0.0f64, 0.0f32);
    for c in 0..CHANNELS {
        for (a, b) in cpp.out[c].iter().zip(&rust.out[c]) {
            signal += f64::from(*a).powi(2);
            noise += f64::from(a - b).powi(2);
            max_abs = max_abs.max((a - b).abs());
        }
    }
    // Spectral difference in dB, over bins within 60 dB of the frame's loudest.
    let mut d = Vec::new();
    for c in 0..CHANNELS {
        for (a, b) in spectra(fft, &cpp.out[c])
            .iter()
            .zip(spectra(fft, &rust.out[c]))
        {
            let peak = a.iter().fold(0.0f32, |m, &v| m.max(v));
            if peak <= 1e-6 {
                continue;
            }
            for (x, y) in a.iter().zip(&b) {
                if *x > peak * 1e-3 {
                    d.push((20.0 * f64::from(x.max(1e-12) / y.max(1e-12)).log10()).abs());
                }
            }
        }
    }
    d.sort_by(f64::total_cmp);
    let mean = d.iter().sum::<f64>() / d.len().max(1) as f64;
    let p99 = d.get(d.len() * 99 / 100).copied().unwrap_or(0.0);
    Diff {
        tags_match,
        frames_match,
        snr_db: 10.0 * (signal / noise.max(1e-30)).log10(),
        max_abs,
        spectral_mean_db: mean,
        spectral_p99_db: p99,
    }
}

pub fn compare(args: &[String]) -> Result<()> {
    if let Some(a) = args.first() {
        return Err(format!("unknown argument {a}").into());
    }
    let signals = [
        Track::from_samples("mix", mix(20.0)),
        Track::from_samples("sweep", sweep(20.0)),
        Track::from_samples("chord", chord(20.0)),
    ];
    let scaled = |t: &Track| {
        Track::from_samples(
            &t.name,
            t.samples.iter().map(|s| s * CONTROL_GAIN).collect(),
        )
    };
    // (speed, pitch); the pitch rows exercise the output resampler.
    let cases = [
        (0.92, 1.0),
        (1.0, 1.0),
        (1.08, 1.0),
        (0.5, 1.0),
        (2.0, 1.0),
        (-1.0, 1.0),
        (1.0, 1.5),
        (1.0, 0.75),
    ];
    let fft = FftPlanner::new().plan_fft_forward(FRAME);
    let mut report = String::new();
    writeln!(
        report,
        "Bungee port vs C++ ({} {}, {SR} Hz, stereo, {GRAINS} grains per row from 10 s in).\n\n\
         Rust vs C++, then in brackets the control: C++ vs C++ on input × (1 + 2⁻²⁰).\n\n\
         | signal | speed | pitch | tags equal | frame counts equal | SNR dB | max abs diff | \
         spectral diff mean dB | spectral diff p99 dB |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    for track in &signals {
        for &(speed, pitch) in &cases {
            let position = 10.0 * f64::from(SR);
            let cpp = run(&mut Cpp::new(SR), track, position, speed, pitch);
            let rust = run(&mut Rust::new(SR), track, position, speed, pitch);
            let mut control = run(&mut Cpp::new(SR), &scaled(track), position, speed, pitch);
            for out in &mut control.out {
                out.iter_mut().for_each(|s| *s /= CONTROL_GAIN);
            }
            let d = diff(&cpp, &rust, &fft);
            let c = diff(&cpp, &control, &fft);
            writeln!(
                report,
                "| {} | {speed} | {pitch} | {} | {} | {:.1} ({:.1}) | {:.1e} ({:.1e}) | {:.3} ({:.3}) \
                 | {:.3} ({:.3}) |",
                track.name,
                if d.tags_match { "yes" } else { "NO" },
                if d.frames_match { "yes" } else { "NO" },
                d.snr_db,
                c.snr_db,
                d.max_abs,
                c.max_abs,
                d.spectral_mean_db,
                c.spectral_mean_db,
                d.spectral_p99_db,
                c.spectral_p99_db,
            )?;
        }
    }
    print!("{report}");
    Ok(())
}

// Synthetic test signals, stereo interleaved, peak around 0.8.

fn mix(seconds: f64) -> Vec<f32> {
    let sr = f64::from(SR);
    let beat = 60.0 / 124.0 * sr;
    let mut noise = 0x9e37_79b9u32;
    (0..(seconds * sr) as usize)
        .flat_map(|i| {
            let t = i as f64 / sr;
            let in_beat = (i as f64 % beat) / sr;
            let kick_f = 50.0 + 120.0 * (-in_beat * 40.0).exp();
            let kick = (-in_beat * 12.0).exp() * (std::f64::consts::TAU * kick_f * in_beat).sin();
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let white = f64::from(noise) / f64::from(u32::MAX) - 0.5;
            let off = ((i as f64 + beat / 2.0) % beat) / sr;
            let hat = (-off * 150.0).exp() * white;
            let bass_f = [55.0, 55.0, 65.41, 49.0][(i as f64 / (2.0 * beat)) as usize % 4];
            let bass = (std::f64::consts::TAU * bass_f * t).sin().signum() * 0.5
                + (std::f64::consts::TAU * bass_f * t).sin() * 0.5;
            let pad = [261.63, 329.63, 392.0, 493.88]
                .iter()
                .enumerate()
                .map(|(k, f)| (std::f64::consts::TAU * f * t * (1.0 + 0.002 * k as f64)).sin())
                .sum::<f64>()
                / 4.0;
            let l = 0.45 * kick + 0.15 * hat + 0.2 * bass + 0.12 * pad;
            let r = 0.45 * kick + 0.08 * hat + 0.2 * bass + 0.18 * pad;
            [l as f32, r as f32]
        })
        .collect()
}

/// Logarithmic sine sweep 40 Hz to 16 kHz over the whole signal, right channel an octave up
/// (capped).
fn sweep(seconds: f64) -> Vec<f32> {
    let sr = f64::from(SR);
    let (f0, f1) = (40.0f64, 16_000.0f64);
    let k = (f1 / f0).ln() / seconds;
    (0..(seconds * sr) as usize)
        .flat_map(|i| {
            let t = i as f64 / sr;
            let phase = std::f64::consts::TAU * f0 * ((k * t).exp() - 1.0) / k;
            let l = 0.5 * phase.sin();
            let r = 0.5
                * (2.0 * phase).sin()
                * if f0 * (k * t).exp() < 10_000.0 {
                    1.0
                } else {
                    0.0
                };
            [l as f32, r as f32]
        })
        .collect()
}

/// A sustained chord with harmonics, changing every two seconds.
fn chord(seconds: f64) -> Vec<f32> {
    let sr = f64::from(SR);
    let chords: [[f64; 3]; 3] = [
        [220.0, 277.18, 329.63],
        [196.0, 246.94, 293.66],
        [174.61, 220.0, 261.63],
    ];
    (0..(seconds * sr) as usize)
        .flat_map(|i| {
            let t = i as f64 / sr;
            let notes = chords[(t / 2.0) as usize % chords.len()];
            let mut l = 0.0;
            let mut r = 0.0;
            for (n, f) in notes.iter().enumerate() {
                for h in 1..=4 {
                    let a = 0.6 / (3.0 * f64::from(h * h));
                    let x = a * (std::f64::consts::TAU * f * f64::from(h) * t).sin();
                    if n % 2 == 0 {
                        l += x;
                        r += 0.6 * x;
                    } else {
                        l += 0.6 * x;
                        r += x;
                    }
                }
            }
            [l as f32, r as f32]
        })
        .collect()
}
