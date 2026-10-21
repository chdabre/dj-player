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

use crate::compute::{Entry, Overview, Waveform};
use crate::display::{Layout, Mode, Rgb, Style, column};

/// An RGB image, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgb>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: Rgb) -> Self {
        Canvas {
            width,
            height,
            pixels: vec![fill; width * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Rgb {
        self.pixels[y * self.width + x]
    }

    /// Blend `c` at opacity `alpha` into one pixel; outside the canvas is ignored.
    pub fn put(&mut self, x: i64, y: i64, c: Rgb, alpha: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let p = &mut self.pixels[y as usize * self.width + x as usize];
        *p = if alpha >= 1.0 { c } else { c.over(*p, alpha) };
    }

    /// Fill `[x0, x1) × [y0, y1)`.
    pub fn fill(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgb, alpha: f32) {
        for y in y0.max(0)..y1.min(self.height as i64) {
            for x in x0.max(0)..x1.min(self.width as i64) {
                self.put(x, y, c, alpha);
            }
        }
    }

    /// A vertical line over `[y0, y1)`; `dash` = (on, off) pixels.
    pub fn vline(&mut self, x: i64, y0: i64, y1: i64, c: Rgb, dash: Option<(i64, i64)>) {
        for y in y0..y1 {
            if dash.is_none_or(|(on, off)| (y - y0) % (on + off) < on) {
                self.put(x, y, c, 1.0);
            }
        }
    }

    /// A filled triangle pointing down (`down`) or up, apex at `(x, y)`, `size` px tall.
    pub fn triangle(&mut self, x: i64, y: i64, size: i64, down: bool, c: Rgb) {
        for i in 0..size {
            let row = if down { y - i } else { y + i };
            self.fill(x - i, row, x + i + 1, row + 1, c, 1.0);
        }
    }

    /// Text in the 3×5 pixel font, top-left at `(x, y)`, each font pixel `scale` px.
    pub fn text(&mut self, x: i64, y: i64, s: &str, scale: i64, c: Rgb) {
        for (i, ch) in s.chars().enumerate() {
            let rows = glyph(ch);
            let gx = x + i as i64 * 4 * scale;
            for (r, bits) in rows.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) != 0 {
                        let px = gx + col * scale;
                        let py = y + r as i64 * scale;
                        self.fill(px, py, px + scale, py + scale, c, 1.0);
                    }
                }
            }
        }
    }

    /// Copy `other` with its top-left corner at `(x, y)`.
    pub fn blit(&mut self, other: &Canvas, x: usize, y: usize) {
        for row in 0..other.height.min(self.height.saturating_sub(y)) {
            let w = other.width.min(self.width.saturating_sub(x));
            let dst = (y + row) * self.width + x;
            self.pixels[dst..dst + w]
                .copy_from_slice(&other.pixels[row * other.width..row * other.width + w]);
        }
    }

    /// Packed 8-bit RGB, for PNG encoders.
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| [p.0, p.1, p.2]).collect()
    }
}

/// 3×5 glyphs for digits and the hot cue letters A–H; anything else is blank.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        _ => [0; 5],
    }
}

/// One beat of the grid as the renderer needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridBeat {
    /// Seconds from the start of the track.
    pub time: f64,
    /// First beat of a bar.
    pub downbeat: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HotCue {
    pub time: f64,
    /// A–H.
    pub label: char,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    pub start: f64,
    pub end: f64,
    /// Length in beats, shown in the zoom.
    pub beats: u32,
}

/// Cue, memory cue, hot cue and loop overlays.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Markers {
    pub cue: Option<f64>,
    pub memory_cues: Vec<f64>,
    pub hot_cues: Vec<HotCue>,
    pub active_loop: Option<Loop>,
}

/// Where a view's columns start: mirrored around the middle line (heights are half-heights)
/// or up from the bottom of the waveform area (heights are of the full area).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Mirrored,
    Bottom,
}

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

const ZOOM_TOP: usize = 14;
const ZOOM_BOTTOM: usize = 14;
const OVERVIEW_TOP: usize = 7;
const OVERVIEW_BOTTOM: usize = 11;

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
    col: &crate::display::Column,
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
    use super::*;
    use crate::compute::{Params, Provenance, Timeline};
    use crate::display::Theme;

    /// 10 s of constant entries, 120 BPM grid from 0 (bars every 2 s).
    fn fixture() -> (Waveform, Vec<GridBeat>) {
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
