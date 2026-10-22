//! The zoomed waveform around the playhead.

use super::{Align, Canvas, GridBeat, Markers, bar_numbers, beat_period, paint_column};
use crate::compute::{Entry, Waveform};
use crate::display::{Layout, Mode, Style, column};

/// Size of the zoomed view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomView {
    pub width: usize,
    pub height: usize,
    /// Beats across the view.
    pub beats: f64,
    /// How the 3-band mode lays out the bands; overlaid by default.
    pub layout: Layout,
    /// Mirrored around the middle (default) or growing up from the bottom.
    pub align: Align,
}

impl Default for ZoomView {
    fn default() -> Self {
        ZoomView {
            width: 703,
            height: 211,
            beats: 20.0,
            layout: Layout::Overlaid,
            align: Align::Mirrored,
        }
    }
}

pub(super) const ZOOM_TOP: usize = 14;
pub(super) const ZOOM_BOTTOM: usize = 14;

/// The zoomed waveform around `playhead` (seconds).
pub fn render_zoom(
    wf: &Waveform,
    grid: &[GridBeat],
    playhead: f64,
    markers: &Markers,
    mode: Mode,
    style: &Style,
    view: &ZoomView,
) -> Canvas {
    let (w, h) = (view.width, view.height);
    let theme = &style.theme;
    let mut canvas = Canvas::new(w, h, theme.background);
    let spp = view.beats * beat_period(grid, playhead) / w as f64;
    let cx = (w / 2) as i64;
    let j0 = (playhead / spp).floor() as i64;
    let x_of = |t: f64| (t / spp).floor() as i64 - j0 + cx;
    let (top, bottom) = (ZOOM_TOP, h - ZOOM_BOTTOM);
    let (t_min, t_max) = (
        playhead - (cx + 1) as f64 * spp,
        playhead + (w as f64 + 1.0) * spp,
    );

    // Grid, behind the waveform.
    for (beat, bar) in bar_numbers(grid) {
        if beat.time < t_min || beat.time > t_max {
            continue;
        }
        let x = x_of(beat.time);
        match bar {
            Some(n) => {
                canvas.vline(x, 0, bottom as i64, theme.bar_line, None);
                canvas.text(x + 3, 4, &n.to_string(), 1, theme.label);
            }
            None => canvas.vline(x, top as i64, bottom as i64, theme.beat_line, None),
        }
    }

    // Waveform.
    let eps = f64::from(wf.params.entries_per_second);
    let n = wf.entries.len() as i64;
    for x in 0..w as i64 {
        let j = j0 + x - cx;
        let (t0, t1) = (j as f64 * spp, (j + 1) as f64 * spp);
        let a = ((t0 * eps).floor() as i64).max(0);
        let b = ((t1 * eps).ceil() as i64).min(n).max(a + 1);
        if t1 <= 0.0 || a >= n {
            continue;
        }
        let e = wf.entries[a as usize..b as usize]
            .iter()
            .fold(Entry::SILENT, |m, &e| m.max(e));
        let mut col = column(e, mode, view.layout, style);
        if j < j0 {
            col = col.dimmed(style.played);
        }
        paint_column(&mut canvas, x, top, bottom, view.align, &col);
    }

    // Overlays, in front.
    if let Some(l) = markers.active_loop {
        let (x0, x1) = (x_of(l.start), x_of(l.end));
        canvas.fill(
            x0,
            top as i64,
            x1,
            bottom as i64,
            theme.loop_color,
            theme.loop_shade[0],
        );
        canvas.vline(x0, top as i64, bottom as i64, theme.loop_color, None);
        canvas.vline(x1, top as i64, bottom as i64, theme.loop_color, None);
        let label = l.beats.to_string();
        let lx = (x0 + x1) / 2 - 2 * label.len() as i64;
        canvas.text(lx, bottom as i64 + 4, &label, 1, theme.loop_color);
    }
    for hc in &markers.hot_cues {
        let x = x_of(hc.time);
        canvas.vline(x, top as i64, bottom as i64, hc.color, None);
        canvas.fill(x, bottom as i64 + 1, x + 9, h as i64 - 1, hc.color, 1.0);
        canvas.text(
            x + 3,
            bottom as i64 + 4,
            &hc.label.to_string(),
            1,
            theme.tag_text,
        );
    }
    for &t in &markers.memory_cues {
        canvas.triangle(x_of(t), top as i64 + 5, 6, true, theme.memory_cue);
    }
    if let Some(t) = markers.cue {
        let x = x_of(t);
        canvas.vline(x, top as i64, bottom as i64, theme.cue, Some((4, 3)));
        canvas.triangle(x, top as i64 + 6, 7, true, theme.cue);
    }
    canvas.fill(cx - 1, 0, cx + 1, h as i64, theme.playhead, 1.0);
    canvas
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::super::{HotCue, Loop};
    use super::*;
    use crate::display::Theme;

    #[test]
    fn zoom_spans_the_beats_with_a_fixed_centre_playhead() {
        let (wf, grid) = fixture();
        let view = ZoomView {
            width: 200,
            height: 100,
            beats: 20.0,
            ..ZoomView::default()
        };
        let style = Style::default();
        let c = render_zoom(
            &wf,
            &grid,
            5.0,
            &Markers::default(),
            Mode::ThreeBand,
            &style,
            &view,
        );
        assert_eq!((c.width, c.height), (200, 100));
        // Playhead, two pixels wide, full height.
        for y in [0, 50, 99] {
            assert_eq!(c.get(100, y), Theme::DEFAULT.playhead);
            assert_eq!(c.get(99, y), Theme::DEFAULT.playhead);
        }
        // The centre row is the highs' inner core; unplayed on the right, dimmed on the left.
        assert_eq!(c.get(150, 50), style.theme.bands[2]);
        assert_eq!(c.get(50, 50), style.theme.bands[2].scale(style.played));
        // Lows fill the band area's edge (gain 1, value 255).
        assert_eq!(c.get(150, ZOOM_TOP), style.theme.bands[0]);
        // 20 beats over 200 px at 120 BPM: 10 s, so bar lines every 40 px. The bar at 4 s
        // is 1 s (20 px) left of the playhead; its line shows in the top margin.
        assert_eq!(c.get(80, 2), Theme::DEFAULT.bar_line);
        assert_eq!(c.get(81, 2), Theme::DEFAULT.background);
        // Nothing is drawn before the start of the track.
        let c = render_zoom(
            &wf,
            &grid,
            0.0,
            &Markers::default(),
            Mode::Single,
            &style,
            &view,
        );
        assert_eq!(c.get(50, 50), Theme::DEFAULT.background);
        assert_eq!(c.get(150, 50), style.theme.single);
    }

    #[test]
    fn zoom_columns_are_fixed_to_the_timeline() {
        // Scrolling by a whole column moves the picture by exactly one pixel.
        let (mut wf, grid) = fixture();
        for (k, e) in wf.entries.iter_mut().enumerate() {
            e.low = (k * 37 % 256) as u8;
        }
        let view = ZoomView {
            width: 300,
            height: 60,
            beats: 20.0,
            ..ZoomView::default()
        };
        let spp = 20.0 * 0.5 / 300.0;
        let at = |t: f64| {
            render_zoom(
                &wf,
                &grid,
                t,
                &Markers::default(),
                Mode::Single,
                &Style::default(),
                &view,
            )
        };
        let (a, b) = (at(4.0 + 0.3 * spp), at(4.0 + 1.3 * spp));
        for x in 160..290 {
            for y in 14..46 {
                assert_eq!(a.get(x + 1, y), b.get(x, y), "{x},{y}");
            }
        }
    }

    #[test]
    fn markers_are_drawn_in_front() {
        let (wf, grid) = fixture();
        let view = ZoomView {
            width: 200,
            height: 100,
            beats: 20.0,
            ..ZoomView::default()
        };
        let markers = Markers {
            cue: Some(6.0),
            memory_cues: vec![],
            hot_cues: vec![HotCue {
                time: 7.0,
                label: 'A',
                color: Theme::DEFAULT.pads[0],
            }],
            active_loop: Some(Loop {
                start: 8.0,
                end: 9.0,
                beats: 2,
            }),
        };
        let c = render_zoom(
            &wf,
            &grid,
            5.0,
            &markers,
            Mode::ThreeBand,
            &Style::default(),
            &view,
        );
        assert_eq!(c.get(120, 50), Theme::DEFAULT.cue); // dashed line starts at the band's top
        assert_eq!(c.get(140, 50), Theme::DEFAULT.pads[0]);
        assert_eq!(c.get(160, 50), Theme::DEFAULT.loop_color);
        assert_ne!(c.get(170, 50), Style::default().theme.bands[2]); // shaded
    }

    #[test]
    fn translucent_bands_blend_over_the_grid_and_background() {
        let (wf, grid) = fixture();
        let style = Style {
            theme: Theme::BLENDED,
            ..Style::default()
        };
        let view = ZoomView {
            width: 200,
            height: 100,
            ..ZoomView::default()
        };
        let c = render_zoom(
            &wf,
            &grid,
            5.0,
            &Markers::default(),
            Mode::ThreeBand,
            &style,
            &view,
        );
        // Mid (60 %) over low (opaque), between the high core and the low edge.
        let mid = Theme::BLENDED.bands[1].over(Theme::BLENDED.bands[0], 0.6);
        let row = (30..50).find(|&y| c.get(150, y) == mid);
        assert!(row.is_some(), "no blended mid pixel in column 150");
    }
}
