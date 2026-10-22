//! Expanding grid anchors into beats, and moving them between timelines.

use crate::XmlTrack;

/// One beat of a Rekordbox grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    /// Seconds from the start of the track, on Rekordbox's timeline.
    pub time: f64,
    /// Position in the bar, 1 = downbeat.
    pub bar_pos: u8,
}

impl XmlTrack {
    /// Every beat up to `until` seconds (the track length), if Rekordbox analysed the grid.
    pub fn beats(&self, until: f64) -> Option<Vec<Beat>> {
        if self.tempo.is_empty() {
            return None;
        }
        let mut beats = Vec::new();
        for (i, m) in self.tempo.iter().enumerate() {
            let end = self.tempo.get(i + 1).map_or(until, |n| n.start);
            if m.bpm <= 0.0 {
                continue;
            }
            let period = 60.0 / m.bpm;
            let per_bar = u32::from(m.beats_per_bar.max(1));
            for k in 0u32.. {
                let time = m.start + f64::from(k) * period;
                // Anchors are written in whole milliseconds, so the previous run can end a
                // millisecond or two short of the next anchor; that beat is the anchor's.
                if time >= end - 0.010 {
                    break;
                }
                let pos = (u32::from(m.beat.max(1)) - 1 + k) % per_bar + 1;
                beats.push(Beat {
                    time,
                    bar_pos: pos as u8,
                });
            }
        }
        Some(beats)
    }

    /// Move every time (grid anchors) by `seconds`; negative moves earlier. Use
    /// `-`[`crate::timeline_offset`] to bring Rekordbox times onto our timeline.
    pub fn shift(&mut self, seconds: f64) {
        for m in &mut self.tempo {
            m.start += seconds;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{TEST_XML as XML, read_collection};

    #[test]
    fn expands_anchors_into_beats() {
        let t = read_collection(XML).unwrap();
        let g = t[1].beats(50.0).unwrap();
        // 0.025 .. 48.026 at 0.5 s: 96 beats; then 48.026 (1), 48.525 (2), then on to 50 s.
        assert_eq!(g[0].time, 0.025);
        assert_eq!(g[95].time, 0.025 + 95.0 * 0.5);
        assert_eq!(g[96].time, 48.026);
        assert_eq!(g[96].bar_pos, 1);
        assert_eq!(g[97].time, 48.525);
        assert_eq!(g[97].bar_pos, 2);
        assert_eq!(g[98].bar_pos, 3);
        assert!(g.last().unwrap().time < 50.0);
        assert!(g.windows(2).all(|w| w[1].time > w[0].time));

        let waltz = t[2].beats(5.0).unwrap();
        let pos: Vec<u8> = waltz.iter().map(|b| b.bar_pos).collect();
        assert_eq!(pos, [2, 3, 1, 2, 3, 1]);
    }
}
