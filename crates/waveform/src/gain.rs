//! The per-track display gain: automatic from the entries, off, or fixed.

use crate::model::Entry;

/// The per-track gain setting: automatic, off, or a fixed value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackGain {
    Auto,
    Off,
    Fixed(f32),
}

impl TrackGain {
    /// The gain for a track, from its detail entries (see [`auto_gain`]).
    pub fn resolve(self, entries: &[Entry]) -> f32 {
        match self {
            TrackGain::Auto => auto_gain(entries),
            TrackGain::Off => 1.0,
            TrackGain::Fixed(g) => g,
        }
    }
}

/// Where [`auto_gain`] puts the track's loud level, as a fraction of the half-height.
pub const AUTO_GAIN_TARGET: f32 = 0.8;
/// Percentile of the combined level that counts as the track's loud level.
pub const AUTO_GAIN_PERCENTILE: f32 = 0.95;
/// [`auto_gain`] stays in this range, so near-silent tracks and noise floors don't blow up.
pub const AUTO_GAIN_RANGE: (f32, f32) = (0.5, 4.0);

/// The track gain that puts the 95th percentile of the combined level of the non-silent
/// entries at 0.8 of the half-height, clamped to 0.5–4. Silence gives 1.
///
/// Pass the detail entries, so zoom and overview use the same gain. Overview columns work
/// too (a track list without the detail), but each column is a max over many entries, so
/// they give a somewhat lower gain.
pub fn auto_gain(entries: &[Entry]) -> f32 {
    let mut histogram = [0usize; 256];
    for e in entries {
        histogram[usize::from(e.combined())] += 1;
    }
    let loud: usize = histogram[1..].iter().sum();
    if loud == 0 {
        return 1.0;
    }
    // The smallest value with at least 95 % of the non-silent entries at or below it.
    let rank = (AUTO_GAIN_PERCENTILE * loud as f32).ceil() as usize;
    let mut seen = 0;
    let value = (1..256)
        .find(|&v| {
            seen += histogram[v];
            seen >= rank
        })
        .unwrap_or(255);
    let (lo, hi) = AUTO_GAIN_RANGE;
    (AUTO_GAIN_TARGET * 255.0 / value as f32).clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Layout, Mode, Style, column};

    /// Entries of a synthetic track at `scale` (linear amplitude), on the stored curve.
    fn track(scale: f32) -> Vec<Entry> {
        (0..3000u32)
            .map(|i| {
                let beat = (i % 70) as f32 / 70.0;
                let amp = |a: f32| crate::compute::encode_level(scale * a);
                Entry {
                    low: amp(0.9 * (1.0 - beat).powi(3) + 0.05),
                    mid: amp(0.3 + 0.1 * ((i % 13) as f32 / 13.0)),
                    high: amp(0.15 * (1.0 - (i % 35) as f32 / 35.0)),
                }
            })
            .collect()
    }

    #[test]
    fn auto_gain_evens_out_levels() {
        let loud = track(1.0);
        let quiet = track(0.25); // −12 dB
        let (gl, gq) = (auto_gain(&loud), auto_gain(&quiet));
        assert!((gq / gl - 2.0).abs() < 0.05, "{gl} {gq}"); // √ curve: −12 dB halves values
        for mode in Mode::ALL {
            for (a, b) in loud.iter().zip(&quiet) {
                let h = |e: Entry, g: f32| {
                    let s = Style {
                        track_gain: g,
                        ..Style::default()
                    };
                    column(e, mode, Layout::Overlaid, &s)
                        .segments()
                        .iter()
                        .map(|s| s.to)
                        .collect::<Vec<_>>()
                };
                let (ha, hb) = (h(*a, gl), h(*b, gq));
                assert_eq!(ha.len(), hb.len());
                for (x, y) in ha.iter().zip(&hb) {
                    assert!((x - y).abs() < 0.03, "{mode:?}: {x} vs {y}");
                }
            }
        }
        // The loud level lands at the target.
        let s = Style {
            track_gain: gl,
            ..Style::default()
        };
        let mut combined: Vec<f32> = loud
            .iter()
            .map(|&e| column(e, Mode::Single, Layout::Overlaid, &s).segments()[0].to)
            .collect();
        combined.sort_by(f32::total_cmp);
        let p95 = combined[combined.len() * 95 / 100];
        assert!((p95 - AUTO_GAIN_TARGET).abs() < 0.02, "{p95}");
    }

    #[test]
    fn auto_gain_is_clamped_and_safe() {
        assert_eq!(auto_gain(&[]), 1.0);
        assert_eq!(auto_gain(&[Entry::SILENT; 500]), 1.0);
        assert_eq!(auto_gain(&track(0.001)), AUTO_GAIN_RANGE.1); // −60 dB
        assert_eq!(
            auto_gain(
                &[Entry {
                    low: 255,
                    mid: 255,
                    high: 255
                }; 10]
            ),
            0.8
        );
        assert_eq!(TrackGain::Off.resolve(&track(0.01)), 1.0);
        assert_eq!(TrackGain::Fixed(2.5).resolve(&track(1.0)), 2.5);
        assert_eq!(TrackGain::Auto.resolve(&track(1.0)), auto_gain(&track(1.0)));
    }
}
