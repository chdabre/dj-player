// Copyright (C) 2020-2025 Parabola Research Limited
// SPDX-License-Identifier: MPL-2.0
//
// Rust port of Bungee's Phase.h: angles as `i16`, the interval [-pi, pi) mapped onto the
// whole integer range so that arithmetic wraps the way phases do.

pub type Phase = i16;

pub fn from_radians(radians: f32) -> Phase {
    const K: f32 = (65536.0 / std::f64::consts::TAU) as f32;
    // pi itself lands on 32768 and wraps to -32768, like the C++ on common targets.
    (K * radians) as i32 as Phase
}
