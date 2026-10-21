//! Synthetic test signals for the previews (no real music in the repo).

use std::f64::consts::TAU;

/// xorshift32 white noise in −0.5..0.5.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        f64::from(self.0) / f64::from(u32::MAX) - 0.5
    }
}

pub const TRACK_BPM: f64 = 128.0;

/// A 96-bar, 128 BPM house-style arrangement, mono in both channels:
///
/// | Bars | Parts |
/// | --- | --- |
/// | 1–8 | kick |
/// | 9–16 | kick, off-beat hats |
/// | 17–32 | + off-beat bass |
/// | 33–48 | breakdown: pad chords, hats, a noise riser over the last 8 bars |
/// | 49–80 | drop: kick, bass, clap on 2 and 4, 16th hats, pad |
/// | 81–96 | outro: kick and hats, fading |
pub fn track(sr: u32) -> Vec<f32> {
    let srf = f64::from(sr);
    let beat = 60.0 / TRACK_BPM;
    let bars = 96.0;
    let mut noise = Noise(0x2545_f491);
    let (mut n1, mut n2) = (0.0, 0.0);
    let bass_notes = [55.0, 55.0, 65.41, 49.0];
    let chords = [
        [220.0, 277.18, 329.63],
        [196.0, 246.94, 293.66],
        [174.61, 220.0, 261.63],
        [196.0, 246.94, 293.66],
    ];
    (0..(bars * 4.0 * beat * srf) as usize)
        .flat_map(|i| {
            let t = i as f64 / srf;
            let b = t / beat;
            let bar = (b / 4.0) as usize; // 0-based
            let in_beat = b.fract() * beat; // seconds since the beat
            let off = (b + 0.5).fract() * beat; // seconds since the off-beat
            let sixteenth = (b * 4.0).fract() * beat / 4.0;
            let white = noise.next();
            let bright = white - 2.0 * n1 + n2; // +12 dB/octave: hat-like
            (n2, n1) = (n1, white);

            let kick_on = bar < 32 || (48..96).contains(&bar);
            let hats_on = bar >= 8;
            let bass_on = (16..32).contains(&bar) || (48..80).contains(&bar);
            let pad_on = (32..80).contains(&bar);
            let drop = (48..80).contains(&bar);
            let fade = if bar >= 80 {
                1.0 - (b - 320.0) / 64.0
            } else {
                1.0
            };

            let mut x = 0.0;
            if kick_on {
                // Pitch falls from 120 to 50 Hz; the phase is its integral.
                let phase = TAU * (50.0 * in_beat + 70.0 / 30.0 * (1.0 - (-in_beat * 30.0).exp()));
                x += 0.75 * (-in_beat * 9.0).exp() * phase.sin();
            }
            if bass_on {
                let f = bass_notes[bar % 4];
                let env = (off * 40.0).min(1.0) * (-off * 6.0).exp();
                x += 0.35 * env * ((TAU * f * t).sin() + 0.3 * (TAU * 2.0 * f * t).sin());
            }
            if hats_on {
                x += 0.22 * (-off * 60.0).exp() * bright;
                if drop {
                    x += 0.08 * (-sixteenth * 120.0).exp() * bright;
                }
            }
            if drop && matches!((b as usize) % 4, 1 | 3) {
                let env = (-in_beat * 18.0).exp();
                x += 0.3 * env * (white + 0.6 * (TAU * 190.0 * in_beat).sin());
            }
            if pad_on {
                let level = if drop { 0.05 } else { 0.11 };
                let swell = ((b % 16.0) / 2.0).min(1.0);
                let chord = chords[(bar / 2) % 4];
                x += level * swell * chord.iter().map(|f| (TAU * f * t).sin()).sum::<f64>();
            }
            if (40..48).contains(&bar) {
                let rise = (b - 160.0) / 32.0;
                x += 0.25 * rise * rise * bright;
            }
            let y = (0.85 * fade * x).clamp(-1.0, 1.0) as f32;
            [y, y]
        })
        .collect()
}

/// Two-beat tone bursts at 120 BPM, cycling 60 Hz, 800 Hz, 6 kHz and all three together,
/// each at −6 dBFS, with a beat of silence between; 32 s.
pub fn tones(sr: u32) -> Vec<f32> {
    let srf = f64::from(sr);
    (0..(32.0 * srf) as usize)
        .flat_map(|i| {
            let t = i as f64 / srf;
            let slot = (t / 1.5) as usize; // 2 beats on, 1 beat off at 0.5 s per beat
            let on = t % 1.5 < 1.0;
            let tone = |f: f64| (TAU * f * t).sin();
            let x = if on {
                match slot % 4 {
                    0 => 0.5 * tone(60.0),
                    1 => 0.5 * tone(800.0),
                    2 => 0.5 * tone(6000.0),
                    _ => 0.5 / 3.0 * (tone(60.0) + tone(800.0) + tone(6000.0)),
                }
            } else {
                0.0
            } as f32;
            [x, x]
        })
        .collect()
}

/// A logarithmic sweep 20 Hz → 20 kHz at −6 dBFS over 40 s.
pub fn sweep(sr: u32) -> Vec<f32> {
    let srf = f64::from(sr);
    let (f0, f1, len) = (20.0f64, 20_000.0f64, 40.0);
    let k = (f1 / f0).ln() / len;
    (0..(len * srf) as usize)
        .flat_map(|i| {
            let t = i as f64 / srf;
            // Phase of an exponential sweep: 2π·f0·(e^{kt} − 1)/k.
            let x = (0.5 * (TAU * f0 * ((k * t).exp() - 1.0) / k).sin()) as f32;
            [x, x]
        })
        .collect()
}
