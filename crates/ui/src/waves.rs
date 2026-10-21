//! Waveform columns for the zoom and overview, built on the CPU with the waveform crate's own
//! display logic (`waveform::display::column`), so every mode, gain and theme matches the
//! reference renderer. The GPU only draws them (`gfx.rs`, the waveform pipeline).
//!
//! **Zoom:** column `j` always covers `[j, j + 1)·s` seconds of the track (`s` = seconds per
//! design pixel), as in `waveform::render::render_zoom`, so a column keeps its value while
//! the track plays. Instead of snapping to whole pixels, the columns are placed at their
//! exact (fractional) position for the interpolated playhead; the shader box-filters between
//! neighbours, so the waveform scrolls smoothly without shimmer.
//!
//! **Overview:** one column per design pixel over the whole track, each the band-wise max of
//! the entries it covers. The per-pixel maxima are cached and rebuilt only when the waveform
//! grows during decode.

use waveform::compute::Entry;
use waveform::display::{Column, Layout, Mode, Style, column};
pub use waveform::render::Align;

/// The waveform settings a deck shows, separate from the theme: what the user picks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveSettings {
    pub mode: Mode,
    pub zoom_layout: Layout,
    pub overview_layout: Layout,
    pub zoom_align: Align,
    pub overview_align: Align,
    /// Dim the played part of the zoom too. Off by default: the zoom stays bright on both
    /// sides of the playhead, and only the overview shows how far the track has played.
    pub zoom_dims_played: bool,
}

impl Default for WaveSettings {
    fn default() -> Self {
        Self {
            mode: Mode::ThreeBand,
            zoom_layout: Layout::Overlaid,
            overview_layout: Layout::Stacked,
            zoom_align: Align::Mirrored,
            // Bands stacked up from the bottom, as on the CDJ-3000's overview.
            overview_align: Align::Bottom,
            zoom_dims_played: false,
        }
    }
}

/// Columns to draw in a rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveDraw {
    /// The area, design pixels.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub align: Align,
    /// Design x of the left edge of `columns[0]`, and the width of one column.
    pub first_x: f32,
    pub column_width: f32,
    pub columns: Vec<Column>,
    /// Opacity of the whole waveform over what is below it.
    pub opacity: f32,
}

/// The band-wise max of `entries[a..b]` (at least one entry if `a` is in range).
fn span_max(entries: &[Entry], a: i64, b: i64) -> Option<Entry> {
    let n = entries.len() as i64;
    if a >= n || b <= 0 {
        return None;
    }
    let a = a.max(0) as usize;
    let b = (b.min(n) as usize).max(a + 1);
    Some(entries[a..b].iter().fold(Entry::SILENT, |m, &e| m.max(e)))
}

/// The zoom's columns around `playhead` (seconds), `seconds_per_px` per design pixel, the
/// playhead at the centre of `area`. Columns left of the playhead's column are dimmed only
/// if [`WaveSettings::zoom_dims_played`] is on.
#[allow(clippy::too_many_arguments)]
pub fn zoom(
    entries: &[Entry],
    entries_per_second: f64,
    playhead: f64,
    seconds_per_px: f64,
    (x, y, w, h): (f32, f32, f32, f32),
    settings: &WaveSettings,
    style: &Style,
) -> WaveDraw {
    let spp = seconds_per_px;
    let half = f64::from(w) * 0.5;
    let playhead_col = (playhead / spp).floor() as i64;
    // One spare column on each side, for the partial columns at the edges.
    let first = ((playhead / spp) - half).floor() as i64 - 1;
    let count = w.ceil() as i64 + 3;
    let columns = (first..first + count)
        .map(|j| {
            let (t0, t1) = (j as f64 * spp, (j + 1) as f64 * spp);
            let a = (t0 * entries_per_second).floor() as i64;
            let b = (t1 * entries_per_second).ceil() as i64;
            match span_max(entries, a, b) {
                Some(e) if t1 > 0.0 => {
                    let c = column(e, settings.mode, settings.zoom_layout, style);
                    if settings.zoom_dims_played && j < playhead_col {
                        c.dimmed(style.played)
                    } else {
                        c
                    }
                }
                _ => Column::default(),
            }
        })
        .collect();
    WaveDraw {
        x,
        y,
        w,
        h,
        align: settings.zoom_align,
        first_x: x + (half + first as f64 - playhead / spp) as f32,
        column_width: 1.0,
        columns,
        opacity: 1.0,
    }
}

/// Per-pixel maxima of the overview, cached until the waveform grows.
#[derive(Debug, Default)]
pub struct OverviewCache {
    key: (usize, u64, u32),
    maxima: Vec<Option<Entry>>,
}

impl OverviewCache {
    /// The overview of `entries` over a track of `duration` seconds in `area`, played part
    /// (left of `playhead`) dimmed.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        entries: &[Entry],
        entries_per_second: f64,
        duration: f64,
        playhead: f64,
        (x, y, w, h): (f32, f32, f32, f32),
        settings: &WaveSettings,
        style: &Style,
    ) -> WaveDraw {
        let px = w.round().max(1.0) as usize;
        let key = (entries.len(), duration.to_bits(), px as u32);
        if key != self.key {
            self.key = key;
            let per_px = duration.max(f64::MIN_POSITIVE) * entries_per_second / px as f64;
            self.maxima = (0..px)
                .map(|i| {
                    let a = (i as f64 * per_px).floor() as i64;
                    let b = ((i + 1) as f64 * per_px).floor() as i64;
                    span_max(entries, a, b.max(a + 1))
                })
                .collect();
        }
        let played = (playhead / duration.max(f64::MIN_POSITIVE) * px as f64).floor() as i64;
        let columns = self
            .maxima
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Some(e) => {
                    let c = column(*e, settings.mode, settings.overview_layout, style);
                    if (i as i64) < played {
                        c.dimmed(style.played)
                    } else {
                        c
                    }
                }
                None => Column::default(),
            })
            .collect();
        WaveDraw {
            x,
            y,
            w,
            h,
            align: settings.overview_align,
            first_x: x,
            column_width: w / px as f32,
            columns,
            opacity: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 150.0;

    fn ramp(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| {
                let v = (i % 256) as u8;
                Entry {
                    low: v,
                    mid: v / 2,
                    high: v / 3,
                }
            })
            .collect()
    }

    fn area() -> (f32, f32, f32, f32) {
        (113.0, 172.0, 703.0, 211.0)
    }

    /// Timeline column `j`: where `d` draws it, and what it draws.
    fn column_of(d: &WaveDraw, playhead: f64, spp: f64, j: i64) -> (f32, Column) {
        let first = ((playhead / spp) - f64::from(d.w) * 0.5).floor() as i64 - 1;
        let k = (j - first) as usize;
        (d.first_x + k as f32 * d.column_width, d.columns[k])
    }

    #[test]
    fn zoom_columns_are_fixed_to_the_timeline() {
        let e = ramp(30_000);
        let (s, st) = (WaveSettings::default(), Style::default());
        let spp = 0.0133;
        let (p, q) = (60.0, 60.0 + spp / 3.0);
        let a = zoom(&e, EPS, p, spp, area(), &s, &st);
        let b = zoom(&e, EPS, q, spp, area(), &s, &st);
        let playhead_col = (p / spp).floor() as i64;
        for j in playhead_col - 300..playhead_col + 300 {
            let (xa, ca) = column_of(&a, p, spp, j);
            let (xb, cb) = column_of(&b, q, spp, j);
            // A third of a pixel later the column sits a third of a pixel further left...
            assert!(
                (xa - xb - 1.0 / 3.0).abs() < 1e-3,
                "column {j}: {xa} -> {xb}"
            );
            // ... with the same value.
            assert_eq!(ca, cb, "column {j} changed");
        }
        // The playhead is at the centre of the area, inside its own column.
        let (x, _) = column_of(&a, p, spp, playhead_col);
        let centre = a.x + a.w * 0.5;
        assert!(x <= centre && centre < x + 1.0, "{x} vs {centre}");
    }

    #[test]
    fn zoom_stays_bright_unless_dimming_is_on_and_blanks_outside_the_track() {
        // Flat entries, so a column's colour depends on dimming only.
        let e = vec![
            Entry {
                low: 200,
                mid: 120,
                high: 60
            };
            3_000
        ]; // 20 s
        let st = Style::default();
        let colours = |s: &WaveSettings| {
            let d = zoom(&e, EPS, 1.0, 0.0133, area(), s, &st);
            // Before 0 s: empty columns.
            assert!(d.columns[0].segments().is_empty());
            let centre = ((d.x + d.w * 0.5 - d.first_x) / d.column_width) as usize;
            (
                d.columns[centre - 3].segments()[0].color,
                d.columns[centre + 3].segments()[0].color,
            )
        };
        let (played, ahead) = colours(&WaveSettings::default());
        assert_eq!(played, ahead, "the zoom is not dimmed by default");
        let (played, ahead) = colours(&WaveSettings {
            zoom_dims_played: true,
            ..WaveSettings::default()
        });
        assert!(played.0 <= ahead.0 && played.1 <= ahead.1 && played.2 <= ahead.2);
        assert_ne!(played, ahead);
    }

    #[test]
    fn overview_covers_the_track_and_caches() {
        let e = ramp(45_000); // 300 s
        let (s, st) = (WaveSettings::default(), Style::default());
        let mut cache = OverviewCache::default();
        let a = cache.draw(&e, EPS, 300.0, 150.0, (113.0, 502.0, 703.0, 67.0), &s, &st);
        assert_eq!(a.columns.len(), 703);
        assert!((a.column_width - 1.0).abs() < 1e-6);
        assert!(a.columns.iter().all(|c| !c.segments().is_empty()));
        // Half decoded: the right half is empty.
        let mut cache = OverviewCache::default();
        let b = cache.draw(
            &e[..22_500],
            EPS,
            300.0,
            0.0,
            (113.0, 502.0, 703.0, 67.0),
            &s,
            &st,
        );
        assert!(!b.columns[10].segments().is_empty());
        assert!(b.columns[600].segments().is_empty());
    }
}
