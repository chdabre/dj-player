//! CPU reference renderer: the zoomed waveform and the whole-track overview, into an RGB
//! canvas. It defines the look pixel by pixel and is the golden reference for the GPU
//! shader, which should do the same per fragment.
//!
//! **Zoom** ([`render_zoom`]): `beats` beats wide (20 by default) at the tempo around the
//! playhead, fixed centre playhead, played part (left of it) dimmed. Paint order:
//! background, beat and bar lines (behind the waveform), the waveform, then loop, hot cue,
//! memory cue and cue overlays, then the playhead. Bar numbers sit in the top margin, hot
//! cue tags and the loop's beat count in the bottom margin.
//!
//! **Columns are fixed to the timeline:** column `j` always covers `[j, j + 1)·s` seconds
//! (`s` = seconds per pixel), so while the track plays the columns move by whole pixels and
//! each one keeps its value; they don't shimmer (Mixxx snaps the same way). A column draws
//! the band-wise max of the entries it touches, at least one.
//!
//! **For the GPU shader:** keep the column values fixed to the timeline, but don't snap the
//! scroll to whole pixels. At 128 BPM, 20 beats over 703 px move about 1.25 px per 60 Hz
//! frame, so whole-pixel steps go 1, 1, 1, 2 and judder. Instead render the columns as here
//! and translate the image by the fractional pixel offset of the (per-frame interpolated)
//! playhead, sampling with linear filtering: smooth scrolling without shimmer. This
//! renderer draws one still frame, so it has no fractional offset.
//!
//! **Sizes:** the defaults are the main screen's (1024×600): zoom 703×211, overview 703×67.
//!
//! **Colours** all come from `style.theme` ([`crate::display::Theme`]); the renderer
//! hard-codes none. A hot cue carries its own colour.
//!
//! **Alignment:** either view draws its columns mirrored around the middle (default) or
//! bottom-aligned, growing up from the bottom of the waveform area ([`Align`]). Heights are
//! then fractions of the full area instead of the half-height.
//!
//! **Overview** ([`render_overview`]): the whole track, 3-band stacked, played part
//! dimmed, 16-bar ticks along the bottom, cue triangles along the top, hot cue tags along
//! the bottom, the loop region and the playhead.

mod canvas;
mod markers;
mod overview;
mod zoom;

pub use canvas::Canvas;
pub use markers::{HotCue, Loop, Markers};
pub use overview::{OverviewView, render_overview};
pub use zoom::{ZoomView, render_zoom};

use crate::display::Column;

/// One beat of the grid as the renderer needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridBeat {
    /// Seconds from the start of the track.
    pub time: f64,
    /// First beat of a bar.
    pub downbeat: bool,
}

/// Where a view's columns start: mirrored around the middle line (heights are half-heights)
/// or up from the bottom of the waveform area (heights are of the full area).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Mirrored,
    Bottom,
}

/// Beat period around `t`: the grid interval containing it, else the nearest one; 0.5 s
/// without a grid.
pub fn beat_period(grid: &[GridBeat], t: f64) -> f64 {
    if grid.len() < 2 {
        return 0.5;
    }
    let i = grid
        .partition_point(|b| b.time <= t)
        .clamp(1, grid.len() - 1);
    grid[i].time - grid[i - 1].time
}

/// 1-based bar number of every downbeat in `grid`, in order.
fn bar_numbers(grid: &[GridBeat]) -> impl Iterator<Item = (GridBeat, Option<usize>)> + '_ {
    let mut bars = 0;
    grid.iter().map(move |&b| {
        if b.downbeat {
            bars += 1;
            (b, Some(bars))
        } else {
            (b, None)
        }
    })
}

/// Paint `col` at column `x` over rows `[top, bottom)`, mirrored around the middle or
/// growing up from the bottom: per pixel, the segments covering its centre, blended in
/// paint order onto what is there.
fn paint_column(
    canvas: &mut Canvas,
    x: i64,
    top: usize,
    bottom: usize,
    align: Align,
    col: &Column,
) {
    let (top_f, bottom_f) = (top as f32, bottom as f32);
    for y in top..bottom {
        let yc = y as f32 + 0.5;
        // Distance from the baseline, 0..1 of the drawable height.
        let d = match align {
            Align::Mirrored => {
                let cy = (top_f + bottom_f) / 2.0;
                (yc - cy).abs() / ((bottom_f - top_f) / 2.0)
            }
            Align::Bottom => (bottom_f - yc) / (bottom_f - top_f),
        };
        let under = canvas.get(x as usize, y);
        if let Some(c) = col.color_at(d, under) {
            canvas.put(x, y as i64, c, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::overview::{OVERVIEW_BOTTOM, OVERVIEW_TOP};
    use super::zoom::ZOOM_BOTTOM;
    use super::*;
    use crate::compute::{Entry, Params, Provenance, Timeline, Waveform};
    use crate::display::{Layout, Mode, Style};

    /// 10 s of constant entries, 120 BPM grid from 0 (bars every 2 s).
    pub(super) fn fixture() -> (Waveform, Vec<GridBeat>) {
        let wf = Waveform {
            params: Params::default(),
            provenance: Provenance::Own,
            timeline: Timeline::default(),
            duration: 10.0,
            entries: vec![
                Entry {
                    low: 255,
                    mid: 128,
                    high: 64,
                };
                1500
            ],
        };
        let grid = (0..20)
            .map(|k| GridBeat {
                time: k as f64 * 0.5,
                downbeat: k % 4 == 0,
            })
            .collect();
        (wf, grid)
    }

    #[test]
    fn either_view_takes_either_layout() {
        let (wf, grid) = fixture();
        let style = Style::default();
        let bands = style.theme.bands;
        // Zoom stacked, overview style: lows at the centre, highs outside.
        let view = ZoomView {
            width: 200,
            height: 100,
            layout: Layout::Stacked,
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
        assert_eq!(c.get(150, 50), bands[0]);
        let overlaid = ZoomView {
            layout: Layout::Overlaid,
            ..view
        };
        let c2 = render_zoom(
            &wf,
            &grid,
            5.0,
            &Markers::default(),
            Mode::ThreeBand,
            &style,
            &overlaid,
        );
        assert_eq!(c2.get(150, 50), bands[2]);
        // Overview overlaid, zoom style: highs at the centre.
        let ov = wf.overview();
        let view = OverviewView {
            width: 300,
            height: 40,
            layout: Layout::Overlaid,
            ..OverviewView::default()
        };
        let cy = (OVERVIEW_TOP + 40 - OVERVIEW_BOTTOM) / 2;
        let c = render_overview(
            &ov,
            &grid,
            0.0,
            &Markers::default(),
            Mode::ThreeBand,
            &style,
            &view,
        );
        assert_eq!(c.get(150, cy), bands[2]);
    }

    #[test]
    fn bottom_aligned_columns_grow_up_from_the_bottom() {
        let (wf, grid) = fixture();
        let style = Style::default();
        let bands = style.theme.bands;
        let ov = wf.overview();
        let view = OverviewView {
            width: 300,
            height: 40,
            align: Align::Bottom,
            ..OverviewView::default()
        };
        let c = render_overview(
            &ov,
            &grid,
            0.0,
            &Markers::default(),
            Mode::ThreeBand,
            &style,
            &view,
        );
        let (top, bottom) = (OVERVIEW_TOP, 40 - OVERVIEW_BOTTOM);
        let area = (bottom - top) as f32;
        // Stacked from the bottom: lows on the baseline, mids above them.
        let low = 1.0 / 3f32.sqrt(); // 255 at gain 1, times the stack scale
        let row = |frac: f32| bottom - 1 - (frac * area) as usize;
        assert_eq!(c.get(150, bottom - 1), bands[0]);
        assert_eq!(c.get(150, row(low + 0.05)), bands[1]);
        // Nothing above the column's top.
        assert_eq!(c.get(150, top), style.theme.background);
        // The zoom too: overlaid from the bottom puts the highs' core on the baseline, where
        // a mirrored column has its low edge.
        let z = ZoomView {
            width: 200,
            height: 100,
            align: Align::Bottom,
            ..ZoomView::default()
        };
        let render = |z: &ZoomView| {
            render_zoom(
                &wf,
                &grid,
                5.0,
                &Markers::default(),
                Mode::ThreeBand,
                &style,
                z,
            )
        };
        let zb = 100 - ZOOM_BOTTOM;
        assert_eq!(render(&z).get(150, zb - 1), bands[2]);
        let mirrored = ZoomView {
            align: Align::Mirrored,
            ..z
        };
        assert_eq!(render(&mirrored).get(150, zb - 1), bands[0]);
    }
}
