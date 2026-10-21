// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0

use super::*;

const SR: u32 = 48_000;

fn sine(frames: usize, hz: f64) -> Vec<f32> {
    let w = std::f64::consts::TAU * hz / f64::from(SR);
    (0..frames)
        .map(|i| (0.5 * (w * i as f64).sin()) as f32)
        .collect()
}

/// Play `input` (mono) from `start` at `speed`; return the output and the first request
/// position of every chunk.
fn play(input: &[f32], start: f64, speed: f64, chunks: usize) -> (Vec<f32>, Vec<f64>) {
    let mut s = Stretcher::new(
        SampleRates {
            input: SR,
            output: SR,
        },
        1,
    );
    let mut buf = vec![0.0; s.max_input_frame_count()];
    let mut request = Request {
        position: start,
        speed,
        pitch: 1.0,
        reset: true,
    };
    s.preroll(&mut request);
    let (mut out, mut tags) = (Vec::new(), Vec::new());
    for _ in 0..chunks {
        let predicted = s.input_chunk(&request, 0.0);
        let chunk = s.specify_grain(&request, 0.0);
        assert_eq!(predicted, chunk);
        let n = chunk.len();
        let (mut head, mut tail) = (0, 0);
        for (k, x) in buf[..n].iter_mut().enumerate() {
            let i = chunk.begin + k as i64;
            match usize::try_from(i).ok().and_then(|i| input.get(i)) {
                Some(&v) => *x = v,
                None if i < 0 => head = k + 1,
                None => {
                    tail = n - k;
                    break;
                }
            }
        }
        s.analyse_grain(&buf, buf.len(), head, tail);
        let o = s.synthesise_grain();
        if !o.request[0].position.is_nan() {
            out.extend_from_slice(o.channel(0));
            tags.push(o.request[0].position);
        }
        s.next(&mut request);
    }
    (out, tags)
}

/// Frequency from rising zero crossings.
fn frequency(x: &[f32]) -> f64 {
    let rising: Vec<usize> = (1..x.len())
        .filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0)
        .collect();
    let (first, last) = (rising[0], rising[rising.len() - 1]);
    (rising.len() - 1) as f64 * f64::from(SR) / (last - first) as f64
}

#[test]
fn unity_speed_reproduces_the_input() {
    let input = sine(SR as usize * 2, 440.0);
    let (out, tags) = play(&input, 24_000.0, 1.0, 40);
    // Chunk k covers the hop that starts at its first request position.
    let hop = 512;
    let k = 10;
    let p = tags[k] as usize;
    let rms_err: f64 = (0..hop)
        .map(|i| f64::from(out[k * hop + i] - input[p + i]).powi(2))
        .sum::<f64>()
        / hop as f64;
    assert!(rms_err.sqrt() < 1e-3, "rms error {}", rms_err.sqrt());
}

#[test]
fn keeps_pitch_at_any_speed_and_in_reverse() {
    let input = sine(SR as usize * 6, 1000.0);
    for speed in [0.5, 0.92, 1.08, 2.0, -1.0] {
        let (out, tags) = play(&input, 3.0 * f64::from(SR), speed, 60);
        let f = frequency(&out[2048..]);
        assert!((f - 1000.0).abs() < 2.0, "speed {speed}: {f} Hz");
        let advance = (tags[20] - tags[10]) / 10.0;
        assert!(
            (advance - 512.0 * speed).abs() < 1e-6,
            "speed {speed}: hop {advance}"
        );
    }
}

#[test]
fn pitch_shifts_through_the_output_resampler() {
    let input = sine(SR as usize * 4, 500.0);
    let mut s = Stretcher::new(
        SampleRates {
            input: SR,
            output: SR,
        },
        1,
    );
    let mut buf = vec![0.0; s.max_input_frame_count()];
    let mut request = Request {
        position: f64::from(SR),
        speed: 1.0,
        pitch: 2.0,
        reset: true,
    };
    s.preroll(&mut request);
    let mut out = Vec::new();
    for _ in 0..80 {
        let chunk = s.specify_grain(&request, 0.0);
        let n = chunk.len();
        let at = chunk.begin as usize;
        buf[..n].copy_from_slice(&input[at..at + n]);
        s.analyse_grain(&buf, buf.len(), 0, 0);
        let o = s.synthesise_grain();
        out.extend_from_slice(o.channel(0));
        s.next(&mut request);
    }
    let f = frequency(&out[4096..]);
    assert!((f - 1000.0).abs() < 3.0, "{f} Hz");
}

#[test]
fn nan_grains_flush_the_pipeline() {
    let mut s = Stretcher::new(
        SampleRates {
            input: SR,
            output: SR,
        },
        2,
    );
    assert!(s.is_flushed());
    let buf = vec![0.1; s.max_input_frame_count() * 2];
    let stride = s.max_input_frame_count();
    let mut request = Request {
        position: 10_000.0,
        speed: 1.0,
        pitch: 1.0,
        reset: true,
    };
    for _ in 0..4 {
        s.specify_grain(&request, 0.0);
        s.analyse_grain(&buf, stride, 0, 0);
        s.synthesise_grain();
        s.next(&mut request);
    }
    assert!(!s.is_flushed());
    request.position = f64::NAN;
    for _ in 0..4 {
        s.specify_grain(&request, 0.0);
        s.analyse_grain(&buf, stride, 0, 0);
        s.synthesise_grain();
    }
    assert!(s.is_flushed());
}
