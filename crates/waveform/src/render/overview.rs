//! The whole-track overview.

use super::{Align, Canvas, GridBeat, Markers, bar_numbers, paint_column};
use crate::compute::{Entry, Overview};
use crate::display::{Layout, Mode, Style, column};

/// Size of the overview.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewView {
    pub width: usize,
    pub height: usize,
    /// How the 3-band mode lays out the bands; stacked by default.
    pub layout: Layout,
    /// Mirrored around the middle (default) or growing up from the bottom, as on the
    /// CDJ-3000's overview.
    pub align: Align,
}

impl Default for OverviewView {
    fn default() -> Self {
        OverviewView {
            width: 703,
            height: 67,
            layout: Layout::Stacked,
            align: Align::Mirrored,
        }
    }
}

pub(super) const OVERVIEW_TOP: usize = 7;
pub(super) const OVERVIEW_BOTTOM: usize = 11;

/// The whole-track overview, with the playhead at `playhead` seconds.
pub fn render_overview(
    ov: &Overview,
    grid: &[GridBeat],
    playhead: f64,
    markers: &Markers,
    mode: Mode,
    style: &Style,
    view: &OverviewView,
) -> Canvas {
    let (w, h) = (view.width, view.height);
    let theme = &style.theme;
    let mut canvas = Canvas::new(w, h, theme.background);
    let duration = ov.duration.max(f64::MIN_POSITIVE);
    let x_of = |t: f64| (t / duration * w as f64).floor() as i64;
    let (top, bottom) = (OVERVIEW_TOP, h - OVERVIEW_BOTTOM);
    let n = ov.columns.len();
    let played = x_of(playhead);

    for x in 0..w {
        let a = x * n / w;
        let b = ((x + 1) * n / w).max(a + 1).min(n);
        if a >= n {
            continue;
        }
        let e = ov.columns[a..b]
            .iter()
            .fold(Entry::SILENT, |m, &e| m.max(e));
        let mut col = column(e, mode, view.layout, style);
        if (x as i64) < played {
            col = col.dimmed(style.played);
        }
        paint_column(&mut canvas, x as i64, top, bottom, view.align, &col);
    }

    // 16-bar scale along the bottom.
    canvas.fill(
        0,
        bottom as i64,
        w as i64,
        bottom as i64 + 1,
        theme.border,
        1.0,
    );
    for (beat, bar) in bar_numbers(grid) {
        if bar.is_some_and(|n| (n - 1) % 16 == 0) {
            let x = x_of(beat.time);
            canvas.vline(x, bottom as i64, bottom as i64 + 4, theme.label, None);
        }
    }

    if let Some(l) = markers.active_loop {
        let (x0, x1) = (x_of(l.start), x_of(l.end).max(x_of(l.start) + 1));
        canvas.fill(
            x0,
            top as i64,
            x1,
            bottom as i64,
            theme.loop_color,
            theme.loop_shade[1],
        );
    }
    for hc in &markers.hot_cues {
        let x = x_of(hc.time);
        canvas.fill(x - 2, bottom as i64 + 5, x + 3, h as i64, hc.color, 1.0);
    }
    for &t in &markers.memory_cues {
        canvas.triangle(x_of(t), 4, 4, true, theme.memory_cue);
    }
    if let Some(t) = markers.cue {
        canvas.triangle(x_of(t), 5, 5, true, theme.cue);
    }
    canvas.fill(played, 0, played + 1, h as i64, theme.playhead, 1.0);
    canvas
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::*;
    use crate::compute::Waveform;
    use crate::display::Theme;

    #[test]
    fn overview_dims_the_played_part_and_marks_16_bars() {
        let (wf, _) = fixture();
        // 60 s at 120 BPM: 30 bars, ticks at bars 1 and 17 (0 s and 32 s).
        let wf = Waveform {
            duration: 60.0,
            entries: vec![wf.entries[0]; 9000],
            ..wf
        };
        let grid: Vec<GridBeat> = (0..120)
            .map(|k| GridBeat {
                time: k as f64 * 0.5,
                downbeat: k % 4 == 0,
            })
            .collect();
        let view = OverviewView {
            width: 600,
            height: 40,
            ..OverviewView::default()
        };
        let style = Style::default();
        let ov = wf.overview();
        let c = render_overview(
            &ov,
            &grid,
            30.0,
            &Markers::default(),
            Mode::ThreeBand,
            &style,
            &view,
        );
        let cy = (OVERVIEW_TOP + 40 - OVERVIEW_BOTTOM) / 2;
        assert_eq!(c.get(300, 10), Theme::DEFAULT.playhead);
        // Stacked: lows at the centre.
        assert_eq!(c.get(400, cy), style.theme.bands[0]);
        assert_eq!(c.get(200, cy), style.theme.bands[0].scale(style.played));
        let tick_y = 40 - OVERVIEW_BOTTOM + 2;
        assert_eq!(c.get(320, tick_y), Theme::DEFAULT.label);
        assert_eq!(c.get(0, tick_y), Theme::DEFAULT.label);
        assert_eq!(c.get(160, tick_y), Theme::DEFAULT.background);
    }
}
