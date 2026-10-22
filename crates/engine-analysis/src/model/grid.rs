//! Beats and beatgrids.

/// One beat of a [`Beatgrid`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    /// Seconds from the start of the track.
    pub time: f64,
    /// Position in the bar, 1 = downbeat (1..=4 in 4/4); 0 = unknown (the source found the
    /// beats but not the bars).
    pub bar_pos: u8,
}

/// Every beat of a track, in time order. A list rather than (offset, BPM), so tempo changes
/// and drifting live recordings work.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Beatgrid {
    pub beats: Vec<Beat>,
}

impl Beatgrid {
    /// A constant-tempo grid: first downbeat at `first`, `bpm`, 4/4, until `until` seconds.
    pub fn constant(first: f64, bpm: f64, until: f64) -> Self {
        let period = 60.0 / bpm;
        let beats = (0..)
            .map(|k| Beat {
                time: first + k as f64 * period,
                bar_pos: (k % 4) as u8 + 1,
            })
            .take_while(|b| b.time < until)
            .collect();
        Beatgrid { beats }
    }

    pub fn is_empty(&self) -> bool {
        self.beats.is_empty()
    }

    /// True if the grid knows where the bars start.
    pub fn has_bars(&self) -> bool {
        self.beats.iter().any(|b| b.bar_pos != 0)
    }

    /// Beats at the given times with unknown bar positions.
    pub fn from_times(times: impl IntoIterator<Item = f64>) -> Self {
        Beatgrid {
            beats: times
                .into_iter()
                .map(|time| Beat { time, bar_pos: 0 })
                .collect(),
        }
    }

    /// Median tempo from the beat intervals, if there are at least two beats.
    pub fn median_bpm(&self) -> Option<f64> {
        let mut periods: Vec<f64> = self
            .beats
            .windows(2)
            .map(|w| w[1].time - w[0].time)
            .filter(|p| *p > 0.0)
            .collect();
        if periods.is_empty() {
            return None;
        }
        periods.sort_by(f64::total_cmp);
        Some(60.0 / periods[periods.len() / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_grid_counts_bars_and_has_its_tempo() {
        let g = Beatgrid::constant(0.5, 120.0, 10.0);
        assert_eq!(g.beats.len(), 19);
        assert_eq!(g.beats[0].bar_pos, 1);
        assert_eq!(g.beats[4].bar_pos, 1);
        assert_eq!(g.beats[5].bar_pos, 2);
        assert!((g.median_bpm().unwrap() - 120.0).abs() < 1e-9);
    }
}
