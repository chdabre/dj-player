// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Partials.h / Partials.cpp.

use std::cmp::Ordering;

/// A spectral peak and the bin one past the end of the region it owns.
#[derive(Debug, Clone, Copy, Default)]
pub struct Partial {
    pub peak: usize,
    pub end: usize,
}

/// Split bins `0..n` into partials, one per energy peak. `energy` needs two spare rows past
/// `n`, used as sentinels and restored afterwards. Returns the number of partials.
pub fn enumerate(partials: &mut [Partial], n: usize, energy: &mut [f32]) -> usize {
    let undo = [energy[n], energy[n + 1]];
    energy[n] = -1.0;
    energy[n + 1] = 0.0;

    let mut count = 0;
    let mut m = 1;
    loop {
        while energy[m] < energy[m + 1] {
            m += 1;
        }
        partials[count].peak = m;
        m += 1;

        // Not `>=`: a NaN ends neither loop early, as in the C++.
        while energy[m].partial_cmp(&energy[m + 1]) != Some(Ordering::Less) {
            m += 1;
        }
        partials[count].end = m;
        count += 1;
        m += 1;

        if m > n || count == partials.len() {
            break;
        }
    }

    energy[n] = undo[0];
    energy[n + 1] = undo[1];
    count
}

fn suppress_partial(partials: &mut [Partial], i: usize, energy: &[f32]) {
    if energy[partials[i - 1].end] > energy[partials[i].end] {
        partials[i - 1].end = partials[i].end;
    } else {
        partials[i].end = partials[i - 1].end;
    }
}

/// Merge partials whose energy jumped since the previous grain (transients) into a neighbour,
/// so they are not phase-locked as tones.
pub fn suppress_transient_partials(
    partials: &mut [Partial],
    energy: &[f32],
    previous_energy: &[f32],
) {
    let mut strongest = 0;
    for i in 1..partials.len() {
        if energy[partials[i].peak] > energy[partials[strongest].peak] {
            strongest = i;
        }
    }

    // Fudge: a lower constant helps transients, a higher one helps tones.
    const K: f32 = 1.5;
    for i in 1..partials.len().saturating_sub(1) {
        if i != strongest && energy[partials[i].peak] > K * previous_energy[partials[i].peak] {
            suppress_partial(partials, i, energy);
        }
    }
}
