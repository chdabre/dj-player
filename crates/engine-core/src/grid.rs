//! `DeckGrid`: a track's beatgrid in engine frames, as the deck uses it for quantize, beat
//! loops and beat jumps.
//!
//! Built off the audio thread from an analysis result (or a Rekordbox grid) and handed to the
//! deck through the command ring; the callback only reads it. Positions are in frames at the
//! engine rate, like the transport's. Before the first and after the last beat the grid is
//! extended at the track tempo, so beat jumps and loops work right up to the track ends.

/// The tempo used for beat lengths when a track has no grid yet (the reference player's
/// fallback for auto beat loops).
pub const FALLBACK_BPM: f64 = 130.0;

#[derive(Debug, Clone, PartialEq)]
pub struct DeckGrid {
    /// Beat positions in engine frames, strictly increasing.
    beats: Box<[f64]>,
    /// Position of each beat in its bar, 1 = downbeat; 0 = unknown.
    bar_pos: Box<[u8]>,
    bpm: f64,
    /// Frames per beat at the track tempo (for extrapolation).
    period: f64,
}

impl DeckGrid {
    /// A grid from beat positions in seconds, for an engine at `sample_rate`. `bpm` is the
    /// track tempo. `None` if there are fewer than two beats or they are not increasing.
    /// Allocates; call off the audio thread.
    pub fn from_seconds(
        beats: impl IntoIterator<Item = (f64, u8)>,
        bpm: f64,
        sample_rate: u32,
    ) -> Option<Self> {
        let sr = f64::from(sample_rate);
        let (beats, bar_pos): (Vec<f64>, Vec<u8>) =
            beats.into_iter().map(|(t, b)| (t * sr, b)).unzip();
        if beats.len() < 2 || beats.windows(2).any(|w| w[1] <= w[0]) || bpm <= 0.0 {
            return None;
        }
        Some(Self {
            period: 60.0 / bpm * sr,
            beats: beats.into_boxed_slice(),
            bar_pos: bar_pos.into_boxed_slice(),
            bpm,
        })
    }

    /// A constant-tempo grid: a downbeat at `first` frames, `bpm`, 4/4, covering `length`.
    pub fn constant(first: f64, bpm: f64, length: f64, sample_rate: u32) -> Option<Self> {
        let sr = f64::from(sample_rate);
        let period = 60.0 / bpm * sr;
        let n = ((length - first) / period).ceil().max(2.0) as usize;
        Self::from_seconds(
            (0..n).map(|k| ((first + k as f64 * period) / sr, (k % 4) as u8 + 1)),
            bpm,
            sample_rate,
        )
    }

    /// Track tempo.
    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Frames per beat at the track tempo.
    pub fn period(&self) -> f64 {
        self.period
    }

    /// Fractional beat number at `position`: 0.0 on the first grid beat, 1.0 on the second,
    /// interpolated in between and extrapolated outside.
    pub fn beat_at(&self, position: f64) -> f64 {
        let b = &self.beats;
        let last = b.len() - 1;
        if position < b[0] {
            return (position - b[0]) / self.period;
        }
        if position >= b[last] {
            return last as f64 + (position - b[last]) / self.period;
        }
        // b[i] <= position < b[i + 1]
        let i = b.partition_point(|&x| x <= position) - 1;
        i as f64 + (position - b[i]) / (b[i + 1] - b[i])
    }

    /// Position (frames) of fractional beat number `beat`; the inverse of [`DeckGrid::beat_at`].
    pub fn position_of(&self, beat: f64) -> f64 {
        let b = &self.beats;
        let last = b.len() - 1;
        if beat < 0.0 {
            return b[0] + beat * self.period;
        }
        if beat >= last as f64 {
            return b[last] + (beat - last as f64) * self.period;
        }
        let i = beat.floor() as usize;
        b[i] + (beat - i as f64) * (b[i + 1] - b[i])
    }

    /// The nearest grid point to `position` at `resolution` beats (1.0, 0.5, 0.25, 0.125).
    pub fn quantize(&self, position: f64, resolution: f64) -> f64 {
        let beat = self.beat_at(position);
        self.position_of((beat / resolution).round() * resolution)
    }

    /// The position `beats` beats after `position` (negative: before), following the grid.
    pub fn offset(&self, position: f64, beats: f64) -> f64 {
        self.position_of(self.beat_at(position) + beats)
    }

    /// Position in the bar (1 = downbeat) of the beat at or before `position`; 0 if unknown.
    pub fn bar_position(&self, position: f64) -> u8 {
        let beat = self.beat_at(position).floor();
        if beat < 0.0 {
            return 0;
        }
        self.bar_pos.get(beat as usize).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn grid() -> DeckGrid {
        // 120 BPM: a beat every 24000 frames, first downbeat at 1000.
        DeckGrid::constant(1000.0, 120.0, 10.0 * 48_000.0, SR).unwrap()
    }

    #[test]
    fn beat_numbers_and_positions_round_trip() {
        let g = grid();
        assert_eq!(g.beat_at(1000.0), 0.0);
        assert_eq!(g.beat_at(25_000.0), 1.0);
        assert_eq!(g.beat_at(13_000.0), 0.5);
        assert_eq!(g.beat_at(-23_000.0), -1.0, "extrapolated before the grid");
        for beat in [-2.0, -0.25, 0.0, 3.3, 19.0, 25.5] {
            assert!(
                (g.beat_at(g.position_of(beat)) - beat).abs() < 1e-9,
                "{beat}"
            );
        }
        assert!(
            g.position_of(100.0) > 10.0 * 48_000.0,
            "extrapolated after the grid"
        );
    }

    #[test]
    fn quantize_snaps_to_the_resolution() {
        let g = grid();
        assert_eq!(g.quantize(25_000.0 + 11_000.0, 1.0), 25_000.0);
        assert_eq!(g.quantize(25_000.0 + 13_000.0, 1.0), 49_000.0);
        assert_eq!(g.quantize(25_000.0 + 7_000.0, 0.25), 25_000.0 + 6_000.0);
        assert_eq!(g.quantize(25_000.0 + 2_000.0, 0.125), 25_000.0 + 3_000.0);
    }

    #[test]
    fn follows_tempo_changes_between_beats() {
        // Beats speeding up: 1 s, then 0.5 s apart.
        let g =
            DeckGrid::from_seconds([(0.0, 1), (1.0, 2), (1.5, 3), (2.0, 4)], 120.0, SR).unwrap();
        assert_eq!(g.beat_at(1.25 * 48_000.0), 1.5);
        assert_eq!(g.offset(0.0, 2.0), 1.5 * 48_000.0);
        assert_eq!(g.bar_position(1.6 * 48_000.0), 3);
        assert!(DeckGrid::from_seconds([(1.0, 1), (0.5, 2)], 120.0, SR).is_none());
    }
}
