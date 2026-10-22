//! Ground truth from a Rekordbox export: its values as a [`Truth`], and corrections to them
//! checked by ear.

use super::Truth;
use crate::rekordbox::XmlTrack;

impl XmlTrack {
    /// Reference values for scoring.
    pub fn truth(&self, until: f64) -> Truth {
        Truth {
            tempo: self.average_bpm,
            beatgrid: self.beatgrid(until),
            key: self.key,
        }
    }
}

/// Corrections to Rekordbox's analysis, checked by ear: a ground truth is only as good as
/// the grids in it.
///
/// One rule per line, `#` comments allowed: `<match>,<action>[,<value>]`, where `<match>`
/// is a case-insensitive substring of "Artist – Name" (or of the file path), and the action
/// is `exclude` (drop the track, e.g. it drifts), `bpm,<value>` (replace the tempo), or
/// `shift_beats,<value>` (move the grid by that many beats, e.g. `0.5` when Rekordbox is half a
/// beat early, `-0.5` when it is half a beat late).
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    rules: Vec<(String, Override)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Override {
    Exclude,
    Bpm(f64),
    ShiftBeats(f64),
}

impl Overrides {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rules = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.rsplitn(3, ',').collect();
            let bad = || format!("overrides line {}: {line:?}", n + 1);
            let rule = match parts.as_slice() {
                [action, m] if action.trim() == "exclude" => (m.trim(), Override::Exclude),
                [value, action, m] => {
                    let v: f64 = value.trim().parse().map_err(|_| bad())?;
                    match action.trim() {
                        "bpm" => (m.trim(), Override::Bpm(v)),
                        "shift_beats" => (m.trim(), Override::ShiftBeats(v)),
                        _ => return Err(bad()),
                    }
                }
                _ => return Err(bad()),
            };
            rules.push((rule.0.to_lowercase(), rule.1));
        }
        Ok(Self { rules })
    }

    /// Apply the rules to `tracks`: excluded tracks are removed, the rest corrected.
    /// Returns how many tracks each rule matched, in rule order.
    pub fn apply(&self, tracks: &mut Vec<XmlTrack>) -> Vec<(String, usize)> {
        let mut hits = vec![0usize; self.rules.len()];
        tracks.retain_mut(|t| {
            let title = format!("{} – {}", t.artist, t.name).to_lowercase();
            let path = t.location.to_string_lossy().to_lowercase();
            let mut keep = true;
            for (i, (m, rule)) in self.rules.iter().enumerate() {
                if !title.contains(m.as_str()) && !path.contains(m.as_str()) {
                    continue;
                }
                hits[i] += 1;
                match *rule {
                    Override::Exclude => keep = false,
                    Override::Bpm(bpm) => t.average_bpm = Some(bpm),
                    Override::ShiftBeats(beats) => {
                        for marker in &mut t.tempo {
                            marker.start += beats * 60.0 / marker.bpm;
                        }
                    }
                }
            }
            keep
        });
        self.rules
            .iter()
            .zip(hits)
            .map(|((m, _), n)| (m.clone(), n))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rekordbox::TEST_XML as XML;
    use crate::rekordbox::read_collection;

    #[test]
    fn overrides_exclude_retempo_and_shift() {
        let mut t = read_collection(XML).unwrap();
        let o = Overrides::parse(
            "# checked by ear\nwaltz,exclude\nrock & roll,shift_beats,-0.5\nNOISE.wav,bpm,100\n",
        )
        .unwrap();
        let hits = o.apply(&mut t);
        assert_eq!(hits.iter().map(|h| h.1).collect::<Vec<_>>(), [1, 1, 1]);
        assert_eq!(t.len(), 2);
        assert!((t[1].tempo[0].start - (0.025 - 0.25)).abs() < 1e-12);
        assert_eq!(t[0].average_bpm, Some(100.0));
        assert!(Overrides::parse("x,frobnicate,1").is_err());
    }
}
