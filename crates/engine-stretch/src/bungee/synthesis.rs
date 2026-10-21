// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Synthesis.cpp and Stretch.h / Stretch.cpp: the phase rotation each
// bin gets so the grain continues the previous one at the new hop. Integer phase arithmetic
// wraps on purpose (Bungee relies on two's-complement overflow).

use super::fourier::bin_count;
use super::grain::Grain;
use super::phase::Phase;

/// Phase rotation of every bin, relative to its peak, for a frequency-domain stretch.
fn frequency(speed: f32, n: usize, rotation: &mut [Phase], phase: &[Phase]) {
    const SHIFT: u32 = 8;
    let speed = speed.abs() + 1e-20;
    let multiplier = ((1 << SHIFT) as f32 / -speed)
        .round()
        .max(f32::from(i16::MIN)) as i32;
    debug_assert!(multiplier <= 0);

    rotation[0] = 0;
    for m in 1..n {
        let delta = phase[m - 1].wrapping_sub(phase[m]);
        let x = (i32::from(delta).wrapping_mul(multiplier)) >> SHIFT;
        rotation[m] = rotation[m - 1].wrapping_add(x as Phase).wrapping_add(delta);
    }
}

/// Bungee's `Stretch::Time`: the phase advance of a peak over one synthesis hop given the
/// analysis hop it was measured over.
struct Time {
    reverse: bool,
    reverse_previous: bool,
    a: i32,
    multiplier: i32,
}

impl Time {
    const LOG2_SYNTHESIS_HOP_REVOLUTION: i32 = -3;

    fn new(log2_synthesis_hop: u32, analysis_hop: i32, analysis_hop_previous: i32) -> Self {
        let log2_transform_length = log2_synthesis_hop as i32 - Self::LOG2_SYNTHESIS_HOP_REVOLUTION;
        let a = analysis_hop.wrapping_shl((32 - log2_transform_length) as u32);

        let dividend = (1i32 << log2_synthesis_hop) << 16;
        let divisor = analysis_hop.wrapping_shl(1);
        let multiplier = if divisor != 0 {
            (dividend + divisor.wrapping_abs() / 2) / divisor
        } else {
            0
        };
        Self {
            reverse: analysis_hop < 0,
            reverse_previous: analysis_hop_previous < 0,
            a,
            multiplier,
        }
    }

    fn offset(&self, phase: Phase, previous: Phase) -> Phase {
        let p = if self.reverse {
            -i32::from(phase)
        } else {
            i32::from(phase)
        };
        let q = if self.reverse_previous {
            -i32::from(previous)
        } else {
            i32::from(previous)
        };
        (p - q) as Phase
    }

    fn delta32(&self, phase: i32, previous: i32, m: i32) -> i32 {
        const LOG_S: u32 = (32 + Time::LOG2_SYNTHESIS_HOP_REVOLUTION) as u32;
        let da = phase
            .wrapping_sub(previous)
            .wrapping_sub(m.wrapping_mul(self.a));
        m.wrapping_shl(LOG_S)
            .wrapping_add((da >> 15).wrapping_mul(self.multiplier))
    }

    fn delta(&self, phase: Phase, previous: Phase, m: i32) -> Phase {
        (self.delta32(i32::from(phase) << 16, i32::from(previous) << 16, m) >> 16) as Phase
    }
}

pub fn synthesise(log2_synthesis_hop: u32, grain: &mut Grain, previous: &Grain) {
    let n = grain.valid_bin_count;
    frequency(
        grain.analysis.speed as f32,
        n,
        &mut grain.rotation,
        &grain.phase,
    );

    let partials = &grain.partials[..grain.partial_count];
    if grain.continuous {
        let time = Time::new(
            log2_synthesis_hop,
            grain.analysis.hop,
            previous.analysis.hop,
        );
        for (i, partial) in partials.iter().enumerate() {
            let peak = partial.peak;
            let offset = time.offset(grain.phase[peak], previous.phase[peak]);
            let stretched = time.delta(grain.phase[peak], previous.phase[peak], peak as i32);
            grain.delta[i] = previous.rotation[peak]
                .wrapping_sub(offset)
                .wrapping_add(stretched)
                .wrapping_sub(grain.rotation[peak]);
        }
    } else {
        for (i, partial) in partials.iter().enumerate() {
            grain.delta[i] = grain.rotation[partial.peak].wrapping_neg();
        }
    }

    // Bungee's do-while: every partial touches at least one bin.
    let mut m = 0;
    for (i, partial) in partials.iter().enumerate() {
        loop {
            if let Some(r) = grain.rotation.get_mut(m) {
                *r = r.wrapping_add(grain.delta[i]);
            }
            m += 1;
            if m >= partial.end {
                break;
            }
        }
    }

    let nyquist = bin_count(grain.log2_transform_length) - 1;
    grain.rotation[nyquist] = grain.rotation[nyquist - 1];
}
