//! The browse screen ("Browse" on the 1024×600 canvas): the stick's folder tree (USB) or
//! every track its library knows (LIBRARY) as a list of seven rows (jacket, title, artist,
//! BPM, key, length), with the main screen's rail, tabs, status band and overview around it.
//!
//! Tapping a folder opens it; tapping a track selects it and tapping the selected track
//! loads it. The header goes up a level when not at the top; its chips change the sort
//! order, search (with an on-screen keyboard in place of the status band) and, in LIBRARY,
//! scan the stick. A drag on the list scrolls it. The browse knob (stand-in) and the arrow
//! keys move the cursor, push / Enter loads, BACK / Esc goes up. A stick with no library
//! asks once whether to scan it.

use std::sync::Arc;

use engined::browse::Thumb;

use crate::kit::Rect;
use crate::render::scene::{Image, Rgba, Weight};
use crate::screen::{Target, Ui, View, bottom, rail, status, tabs};
use crate::theme::TextStyle;
use crate::waves::OverviewCache;

/// Rows shown at once.
pub const ROWS: usize = 7;
/// Row height, design pixels.
pub const ROW_H: f32 = 40.0;

const HEADER: Rect = Rect::new(113.0, 69.0, 903.0, 31.0);
const LIST_Y: f32 = 103.0;
const JACKET_X: f32 = 113.0;
const JACKET_W: f32 = 95.0;
/// The artwork square in the jacket column.
const JACKET_SIDE: f32 = 34.0;
const TITLE_X: f32 = 208.0;
const TITLE_W: f32 = 352.0;
const ARTIST_X: f32 = 560.0;
const ARTIST_W: f32 = 456.0;
const PAD: f32 = 11.0;
/// Room for the BPM, key and time at the right of the artist column.
const DETAIL_W: f32 = 190.0;
/// The header chips, from the right: SEARCH, SORT, SCAN.
const SEARCH_CHIP: (f32, f32) = (916.0, 100.0);
const SORT_CHIP: (f32, f32) = (796.0, 114.0);
const SCAN_CHIP: (f32, f32) = (706.0, 84.0);
const ANALYZE_CHIP: (f32, f32) = (576.0, 124.0);
/// Where the keyboard goes: over the status band and the bottom strip.
const KEYBOARD: Rect = Rect::new(106.0, 395.0, 915.0, 202.0);

/// The on-screen keyboard's special keys, as characters.
pub const BACKSPACE: char = '\u{8}';
pub const CLEAR: char = '\u{18}';
pub const DONE: char = '\n';
pub const SHIFT: char = '\u{f}';

/// The on-screen keyboard's shift: off, for the next letter, or locked (tap again to cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shift {
    #[default]
    Off,
    Once,
    Lock,
}

impl Shift {
    /// The next state on a tap of SHIFT.
    pub fn tap(self) -> Shift {
        match self {
            Shift::Off => Shift::Once,
            Shift::Once => Shift::Lock,
            Shift::Lock => Shift::Off,
        }
    }

    /// `c` as typed with this shift, and the shift after it (a letter uses up `Once`).
    pub fn apply(self, c: char) -> (char, Shift) {
        if self == Shift::Off || !c.is_alphabetic() {
            return (c, self);
        }
        let up = c.to_uppercase().next().unwrap_or(c);
        (
            up,
            if self == Shift::Once {
                Shift::Off
            } else {
                self
            },
        )
    }
}

/// No key: a gap in a row (the stagger of a real keyboard).
const GAP: char = '\0';

/// The on-screen keyboard, laid out as a computer's: rows of (label, character, width in
/// key units), 11 units a row. Digits with backspace on top, the letter rows staggered,
/// SHIFT bottom left, and a bottom row of CLEAR, the space bar and DONE (as on a phone).
pub const KEYS: [&[(&str, char, f32)]; 5] = [
    &[
        ("1", '1', 1.0),
        ("2", '2', 1.0),
        ("3", '3', 1.0),
        ("4", '4', 1.0),
        ("5", '5', 1.0),
        ("6", '6', 1.0),
        ("7", '7', 1.0),
        ("8", '8', 1.0),
        ("9", '9', 1.0),
        ("0", '0', 1.0),
        ("⌫", BACKSPACE, 1.0),
    ],
    &[
        ("", GAP, 0.5),
        ("Q", 'q', 1.0),
        ("W", 'w', 1.0),
        ("E", 'e', 1.0),
        ("R", 'r', 1.0),
        ("T", 't', 1.0),
        ("Y", 'y', 1.0),
        ("U", 'u', 1.0),
        ("I", 'i', 1.0),
        ("O", 'o', 1.0),
        ("P", 'p', 1.0),
        ("", GAP, 0.5),
    ],
    &[
        ("", GAP, 1.0),
        ("A", 'a', 1.0),
        ("S", 's', 1.0),
        ("D", 'd', 1.0),
        ("F", 'f', 1.0),
        ("G", 'g', 1.0),
        ("H", 'h', 1.0),
        ("J", 'j', 1.0),
        ("K", 'k', 1.0),
        ("L", 'l', 1.0),
        ("", GAP, 1.0),
    ],
    &[
        ("SHIFT", SHIFT, 1.0),
        ("Z", 'z', 1.0),
        ("X", 'x', 1.0),
        ("C", 'c', 1.0),
        ("V", 'v', 1.0),
        ("B", 'b', 1.0),
        ("N", 'n', 1.0),
        ("M", 'm', 1.0),
        ("-", '-', 1.0),
        (".", '.', 1.0),
        ("'", '\'', 1.0),
    ],
    &[
        ("CLEAR", CLEAR, 2.0),
        ("&", '&', 1.0),
        ("SPACE", ' ', 6.0),
        ("DONE", DONE, 2.0),
    ],
];

/// Every key, in order, without the gaps.
fn keys() -> impl Iterator<Item = &'static (&'static str, char, f32)> {
    KEYS.iter().flat_map(|r| r.iter()).filter(|k| k.1 != GAP)
}

/// How many keys the keyboard has.
#[cfg(test)]
pub fn key_count() -> usize {
    keys().count()
}

/// The character of keyboard key `i` (counted across the rows).
pub fn key_char(i: u8) -> Option<char> {
    keys().nth(usize::from(i)).map(|k| k.1)
}

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Folder,
    Track,
    /// A playlist (or the tag list).
    List,
}

/// One shown row.
#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    pub title: String,
    pub artist: String,
    /// BPM, key and length, right-aligned in the artist column.
    pub detail: String,
    /// The track on the deck.
    pub loaded: bool,
    pub art: Option<Arc<Thumb>>,
    /// On the tag list.
    pub tagged: bool,
    /// A playlist entry whose file isn't known: shown dimmed, can't load.
    pub missing: bool,
}

/// A list modal over the browse list (MENU, ADD TO PLAYLIST, a confirmation).
#[derive(Debug, Clone, Default)]
pub struct ModalView {
    pub title: String,
    pub rows: Vec<String>,
    pub selected: usize,
    /// The first row: a confirmation, its YES drawn as a destructive action.
    pub danger_first: bool,
}

/// Rows a modal shows at once.
pub const MODAL_ROWS: usize = 5;

/// The info panel: what is known of the selected track, in place of the list.
#[derive(Debug, Clone, Default)]
pub struct InfoView {
    /// Where the track is: `USB`, `LIBRARY`.
    pub source: &'static str,
    /// The selection's place in the list: `TRACK 002/030`.
    pub position: String,
    pub folder: bool,
    /// (label, value) rows: title, artist, album, time, BPM, key.
    pub fields: Vec<(&'static str, String)>,
    /// Stars, 0–5, if rated.
    pub rating: Option<u8>,
    /// The artwork (large once read, the list's thumbnail until then).
    pub art: Option<Arc<Thumb>>,
    pub comment: Option<String>,
    /// The track on the deck.
    pub loaded: bool,
}

/// What the browse screen shows, prepared from the browser once per frame.
#[derive(Debug, Clone, Default)]
pub struct BrowseView {
    /// Where the list is: the source, then the folders down to it (or the search).
    pub header: String,
    pub at_root: bool,
    /// Showing the library's tracks (LIBRARY) rather than the folders (USB).
    pub library: bool,
    /// The sort order's name.
    pub sort: &'static str,
    /// Searching; the keyboard is up.
    pub searching: bool,
    pub keyboard: bool,
    /// The keyboard's shift: letters show (and type) in capitals while it is on.
    pub shift: Shift,
    /// A scan under way: files done, total.
    pub scan: Option<(usize, usize)>,
    /// The library can be scanned (offered in LIBRARY).
    pub can_scan: bool,
    /// Tracks in the library without our analysis (ANALYZE ALL is offered while > 0).
    pub unanalysed: usize,
    /// ANALYZE ALL under way: files done, total, and whether it is stopping.
    pub analyzing: Option<(usize, usize, bool)>,
    /// Ask whether to scan this stick, which has no library.
    pub ask_scan: bool,
    /// The rows from the first one shown, at most [`ROWS`].
    pub rows: Vec<Row>,
    /// The cursor's row among `rows`.
    pub selected: Option<usize>,
    /// Index of the first row shown, and the entry count, for the scroll bar.
    pub first: usize,
    pub total: usize,
    /// Shown instead of rows when there are none.
    pub empty: Option<String>,
    /// The level holds tracks: the list has its title and artist columns. Without (folders
    /// only, the library menu) it is one column, with no divider.
    pub has_tracks: bool,
    /// The INFO screen: a track's details instead of the list.
    pub info: Option<InfoView>,
    /// INFO in the browser: the selected track's details beside the list.
    pub side: Option<InfoView>,
    /// The tag list is shown (the TAG LIST tab lit).
    pub tag_list: bool,
    /// A modal over the list.
    pub modal: Option<ModalView>,
    /// Shown in the header instead of `header` (naming, moving).
    pub header_override: Option<String>,
}

pub(crate) fn build(ui: &mut Ui, v: &View, b: &BrowseView, overview: &mut OverviewCache) {
    let t = ui.t;
    ui.scene.fill(0.0, 0.0, 101.0, 600.0, t.color.background);
    ui.scene.fill(100.0, 0.0, 1.0, 600.0, t.color.rule);
    ui.section(Rect::new(106.0, 2.0, 915.0, 64.0));
    ui.section(Rect::new(106.0, 66.0, 915.0, 325.0));
    // The tag list belongs to no source on the rail.
    let lit = if b.library {
        Target::Library
    } else if b.tag_list {
        Target::TagList
    } else {
        Target::Usb
    };
    rail(ui, v, lit);
    match &b.info {
        Some(info) => {
            tabs(ui, Some(Target::Info));
            info_panel(ui, info);
        }
        None => {
            tabs(
                ui,
                Some(if b.tag_list {
                    Target::TagList
                } else {
                    Target::Browse
                }),
            );
            header(ui, b);
            list(ui, b);
            if let Some(side) = &b.side {
                side_panel(ui, side);
            }
            if let Some(m) = &b.modal {
                modal(ui, m);
            }
        }
    }
    if b.keyboard {
        keyboard(ui, b.shift);
    } else {
        ui.section(Rect::new(106.0, 395.0, 915.0, 99.0));
        ui.section(Rect::new(106.0, 498.0, 915.0, 99.0));
        ui.scene.fill(822.0, 400.0, 1.0, 89.0, t.color.section_edge);
        ui.scene.fill(822.0, 502.0, 1.0, 90.0, t.color.section_edge);
        status(ui, v);
        bottom(ui, v, overview);
    }
    if b.ask_scan {
        ask_scan(ui);
    }
}

fn header(ui: &mut Ui, b: &BrowseView) {
    let t = ui.t;
    let Rect { x, y, w, h } = HEADER;
    // The chips' touch targets run from the section's top to the first row.
    let (hit_y, hit_h) = (66.0, LIST_Y - 66.0);
    let mut chips_left = SORT_CHIP.0;
    let chip = |ui: &mut Ui, target: Target, (cx, cw): (f32, f32), label: &str, on: bool| {
        ui.button_styled(
            target,
            Rect::new(cx, y + 1.0, cw, h - 3.0),
            label,
            None,
            on,
            t.text.small.semibold().color(t.color.text),
        );
        // Taller than drawn, for a finger.
        if let Some(last) = ui.hits.last_mut() {
            last.rect.y = hit_y;
            last.rect.h = hit_h;
        }
    };
    let pressed_back = ui.pressed == Some(Target::BrowseBack);
    ui.scene.fill(x, y + h - 1.0, w, 1.0, t.color.button_edge);
    chip(ui, Target::Search, SEARCH_CHIP, "SEARCH", b.searching);
    chip(
        ui,
        Target::Sort,
        SORT_CHIP,
        &format!("SORT: {}", b.sort),
        false,
    );
    if b.library && b.can_scan && b.scan.is_none() {
        chip(ui, Target::Scan, SCAN_CHIP, "SCAN", false);
        chips_left = SCAN_CHIP.0;
        match b.analyzing {
            Some((done, total, stopping)) => {
                chip(
                    ui,
                    Target::AnalyzeAll,
                    ANALYZE_CHIP,
                    if stopping { "STOPPING" } else { "STOP" },
                    true,
                );
                ui.text(
                    format!("ANALYZING {done} / {total}"),
                    ANALYZE_CHIP.0 - 10.0,
                    y + (h - t.font.small * t.font.line_height) * 0.5,
                    t.text.small.semibold().color(t.color.cue_text).right(),
                );
                chips_left = ANALYZE_CHIP.0 - 150.0;
            }
            None if b.unanalysed > 0 => {
                chip(ui, Target::AnalyzeAll, ANALYZE_CHIP, "ANALYZE ALL", false);
                chips_left = ANALYZE_CHIP.0;
            }
            None => {}
        }
    }
    if let Some((done, total)) = b.scan {
        ui.text(
            format!("SCANNING {done} / {total}"),
            SORT_CHIP.0 - 10.0,
            y + (h - t.font.small * t.font.line_height) * 0.5,
            t.text.small.semibold().color(t.color.cue_text).right(),
        );
        chips_left = SORT_CHIP.0 - 150.0;
    }
    let label = match &b.header_override {
        Some(h) => h.clone(),
        None if b.at_root => b.header.clone(),
        None => format!("‹  {}", b.header),
    };
    ui.text(
        label,
        x + PAD,
        y + (h - t.font.row * t.font.line_height) * 0.5,
        t.text.row.semibold().max_width(chips_left - x - 2.0 * PAD),
    );
    if !b.at_root {
        // Up to the chips (or the progress text left of them).
        let back_w = chips_left - x - 6.0;
        if pressed_back {
            ui.scene.fill(x, y, back_w, h - 1.0, t.color.button_pressed);
        }
        ui.hit(Target::BrowseBack, Rect::new(x, hit_y, back_w, hit_h));
    }
}

fn list(ui: &mut Ui, b: &BrowseView) {
    let t = ui.t;
    let list_h = ROW_H * ROWS as f32;
    // With the info panel beside it, the list is the jacket and title columns.
    let right = list_right(b);
    // The title column's right edge, through all rows: only where there are artists to
    // divide off (and always beside the info panel).
    let one_column = !b.has_tracks && b.side.is_none();
    if !one_column {
        ui.scene.fill(
            TITLE_X + TITLE_W - 1.0,
            LIST_Y,
            1.0,
            list_h - 3.0,
            t.color.button_edge,
        );
    }
    let title_w = if one_column {
        right - TITLE_X - 2.0 * PAD
    } else {
        TITLE_W - 2.0 * PAD
    };
    if let Some(msg) = &b.empty {
        ui.text(
            msg.clone(),
            TITLE_X + PAD,
            LIST_Y + (ROW_H - t.font.row * t.font.line_height) * 0.5,
            t.text.row.color(t.color.label),
        );
    }
    // Text and pictures draw above every rectangle, so under the question the rows go.
    let shown = if b.ask_scan || b.modal.is_some() {
        0
    } else {
        ROWS
    };
    for (i, row) in b.rows.iter().enumerate().take(shown) {
        let y = LIST_Y + i as f32 * ROW_H;
        let selected = b.selected == Some(i);
        if selected {
            ui.scene
                .fill(JACKET_X, y, right - JACKET_X, ROW_H, t.color.row_selected);
        }
        ui.scene.fill(
            TITLE_X,
            y + ROW_H - 1.0,
            right - TITLE_X,
            1.0,
            t.color.row_rule,
        );
        jacket(ui, row, y);
        let weight = if selected {
            Weight::SemiBold
        } else {
            Weight::Regular
        };
        let text_y = y + (ROW_H - t.font.row * t.font.line_height) * 0.5;
        let title_color = if row.missing {
            t.color.dim
        } else if row.loaded {
            t.color.accent_text
        } else {
            t.color.text
        };
        if row.tagged {
            // The tag mark, in the title's left padding.
            let d = 6.0;
            ui.scene.rounded(
                TITLE_X + 2.0,
                y + (ROW_H - d) * 0.5,
                d,
                d,
                d * 0.5,
                t.color.cue,
                0.0,
                t.color.cue,
            );
        }
        cell(
            ui,
            &row.title,
            TITLE_X + PAD,
            text_y,
            title_w,
            weight,
            title_color,
        );
        if b.side.is_some() {
            ui.hit(
                Target::Row(i as u8),
                Rect::new(JACKET_X, y, right - JACKET_X, ROW_H),
            );
            continue;
        }
        let detail_w = if row.detail.is_empty() { 0.0 } else { DETAIL_W };
        cell(
            ui,
            &row.artist,
            ARTIST_X + PAD,
            text_y,
            ARTIST_W - 2.0 * PAD - detail_w,
            weight,
            t.color.text,
        );
        if !row.detail.is_empty() {
            // As large and as bold as the title and artist.
            ui.text(
                row.detail.clone(),
                ARTIST_X + ARTIST_W - PAD,
                text_y,
                t.text.row.weight(weight).right(),
            );
        }
        ui.hit(
            Target::Row(i as u8),
            Rect::new(JACKET_X, y, ARTIST_X + ARTIST_W - JACKET_X, ROW_H),
        );
    }
    scroll_bar(ui, b, list_h);
}

/// The list's right edge: the artist column's, or the title column's with the info panel.
fn list_right(b: &BrowseView) -> f32 {
    if b.side.is_some() {
        TITLE_X + TITLE_W
    } else {
        ARTIST_X + ARTIST_W
    }
}

/// A row's cell text, ellipsized to `w`.
fn cell(ui: &mut Ui, s: &str, x: f32, y: f32, w: f32, weight: Weight, color: Rgba) {
    let t = ui.t;
    ui.text(
        s.to_string(),
        x,
        y,
        t.text.row.weight(weight).color(color).max_width(w),
    );
}

/// The jacket column: the artwork, a frame where there is none (yet), or a folder.
fn jacket(ui: &mut Ui, row: &Row, y: f32) {
    let t = ui.t;
    let side = JACKET_SIDE;
    let (x, top) = (JACKET_X + (JACKET_W - side) * 0.5, y + (ROW_H - side) * 0.5);
    match (row.kind, &row.art) {
        (RowKind::Track, Some(art)) => ui.scene.images.push(Image {
            picture: Arc::clone(art),
            x,
            y: top,
            w: side,
            h: side,
            radius: t.shape.panel_radius,
        }),
        (RowKind::Track, None) => ui.scene.rounded(
            x,
            top,
            side,
            side,
            t.shape.panel_radius,
            t.color.background,
            t.shape.border,
            t.color.chip_edge,
        ),
        (RowKind::List, _) => {
            // Three lines: a list.
            let (lx, lw) = (x - 2.0, side + 4.0);
            for k in 0..3 {
                ui.scene.rounded(
                    lx,
                    top + 7.0 + k as f32 * 8.0,
                    lw,
                    4.0,
                    2.0,
                    t.color.folder,
                    0.0,
                    t.color.folder,
                );
            }
        }
        (RowKind::Folder, _) => {
            // A tab and a body.
            let (fx, fy, fw, fh) = (x - 3.0, top + 8.0, side + 6.0, side - 14.0);
            ui.scene.rounded(
                fx,
                fy - 4.0,
                fw * 0.45,
                6.0,
                t.shape.panel_radius,
                t.color.folder,
                0.0,
                t.color.folder,
            );
            ui.scene.rounded(
                fx,
                fy,
                fw,
                fh,
                t.shape.panel_radius,
                t.color.folder,
                0.0,
                t.color.folder,
            );
        }
    }
}

/// Where the shown rows are in the folder, when it has more than fit.
fn scroll_bar(ui: &mut Ui, b: &BrowseView, list_h: f32) {
    if b.total <= ROWS {
        return;
    }
    let t = ui.t;
    let x = list_right(b) - if b.side.is_some() { 5.0 } else { 3.0 };
    let thumb_h = (list_h * ROWS as f32 / b.total as f32).max(16.0);
    let max_first = (b.total - ROWS) as f32;
    let thumb_y = LIST_Y + (list_h - thumb_h) * (b.first as f32 / max_first).min(1.0);
    ui.scene.fill(x, LIST_Y, 2.0, list_h, t.color.row_rule);
    ui.scene.fill(x, thumb_y, 2.0, thumb_h, t.color.label);
}

/// The search keyboard, over the status band and the bottom strip.
fn keyboard(ui: &mut Ui, shift: Shift) {
    let t = ui.t;
    let Rect { x: x0, y: y0, w, h } = KEYBOARD;
    ui.section(Rect::new(x0, y0, w, h));
    let gap = 3.0;
    let (inner_x, inner_w) = (x0 + 7.0, w - 14.0);
    // A key of `u` units is `u * unit - gap` wide, so a row of 11 units fills the width.
    let unit = (inner_w + gap) / 11.0;
    let rows = KEYS.len() as f32;
    let row_h = (h - 2.0 * gap - (rows - 1.0) * gap) / rows;
    let mut index = 0u8;
    for (r, keys) in KEYS.iter().enumerate() {
        let y = y0 + gap + r as f32 * (row_h + gap);
        let mut u = 0.0;
        for &(label, ch, units) in keys.iter() {
            let x = inner_x + u * unit;
            u += units;
            if ch == GAP {
                continue;
            }
            let kw = units * unit - gap;
            let on = ch == DONE || (ch == SHIFT && shift != Shift::Off);
            // Letters show the case they type.
            let shown = match (ch, shift) {
                (SHIFT, Shift::Lock) => "CAPS".to_string(),
                (c, Shift::Off) if c.is_alphabetic() => label.to_lowercase(),
                _ => label.to_string(),
            };
            let style = if label.chars().count() > 1 {
                t.text.small.semibold().color(t.color.text)
            } else {
                t.text.button
            };
            ui.button_styled(
                Target::Key(index),
                Rect::new(x, y, kw, row_h),
                &shown,
                None,
                on,
                style,
            );
            index += 1;
        }
    }
}

/// The INFO screen: a header with the source and the track's place, then the artwork as
/// large as the panel is tall, and beside it the names, large BPM / key / time / rating
/// readouts and the comment, using the whole width. (The canvas's "Info" artboard puts the
/// fields in a narrow list and leaves a third of the panel empty.)
fn info_panel(ui: &mut Ui, i: &InfoView) {
    let t = ui.t;
    let line_y = |y: f32, h: f32, size: f32| y + (h - size * t.font.line_height) * 0.5;
    // Header bar.
    let (hx, hy, hw, hh) = (113.0, 69.0, 903.0, 35.0);
    ui.scene.rounded(
        hx,
        hy,
        hw,
        hh,
        t.shape.button_radius,
        t.color.title_bar,
        t.shape.border,
        t.color.button_edge,
    );
    ui.text(
        format!("{}  @  PLAYER 1", i.source),
        hx + PAD,
        line_y(hy, hh, t.font.row),
        t.text.row.semibold(),
    );
    ui.text(
        i.position.clone(),
        hx + hw - PAD,
        line_y(hy, hh, t.font.row),
        t.text.row.semibold().right(),
    );

    // Artwork: the panel's full height.
    let (ax, ay) = (113.0, 111.0);
    let side = 384.0 - ay;
    match &i.art {
        Some(art) => ui.scene.images.push(Image {
            picture: Arc::clone(art),
            x: ax,
            y: ay,
            w: side,
            h: side,
            radius: t.shape.button_radius,
        }),
        None => {
            ui.scene.rounded(
                ax,
                ay,
                side,
                side,
                t.shape.button_radius,
                t.color.background,
                t.shape.border,
                t.color.chip_edge,
            );
            ui.text(
                if i.folder { "FOLDER" } else { "NO ARTWORK" },
                ax + side * 0.5,
                line_y(ay, side, t.font.label),
                t.text.label.color(t.color.dim).center(),
            );
        }
    }

    // Names.
    let x0 = ax + side + 18.0;
    let w = 1016.0 - PAD - x0;
    let title_label = if i.folder { "FOLDER" } else { "TITLE" };
    let title_color = if i.loaded {
        t.color.accent_text
    } else {
        t.color.text
    };
    let album = [i.field("ALBUM"), i.field("GENRE")]
        .into_iter()
        .filter(|s| *s != "–")
        .collect::<Vec<_>>()
        .join("  ·  ");
    let names = [
        (
            i.field(title_label).to_string(),
            t.text.title.color(title_color),
        ),
        (i.field("ARTIST").to_string(), t.text.row),
        (album, t.text.label),
    ];
    let mut y = ay - 3.0;
    for (text, style) in names {
        ui.text(text, x0, y, style.max_width(w));
        y += style.line_box() + 3.0;
    }
    if i.folder {
        return;
    }

    // Readouts.
    let gy = 196.0;
    ui.scene.fill(x0, gy, w, 1.0, t.color.row_rule);
    let stars = i.stars();
    let cells = [
        ("BPM", i.field("BPM")),
        ("KEY", i.field("KEY")),
        ("TIME", i.field("TIME")),
        ("RATING", stars.as_str()),
    ];
    let cw = w / cells.len() as f32;
    for (k, (label, value)) in cells.into_iter().enumerate() {
        let cx = x0 + k as f32 * cw;
        if k > 0 {
            ui.scene
                .fill(cx - 8.0, gy + 10.0, 1.0, 52.0, t.color.row_rule);
        }
        ui.text(label, cx, gy + 8.0, t.text.label);
        let style = match label {
            // Five stars at the readout size don't fit a cell.
            "RATING" => TextStyle {
                size: t.font.row,
                ..t.text.counter
            },
            "BPM" => t.text.counter.color(t.color.accent_text),
            _ => t.text.counter,
        };
        let y = gy + 8.0 + t.text.label.line_box();
        ui.text(value.to_string(), cx, y, style.max_width(cw - 16.0));
    }

    // Comment, across the rest.
    let cy = 270.0;
    ui.scene.fill(x0, cy, w, 1.0, t.color.row_rule);
    ui.text("COMMENT", x0, cy + 8.0, t.text.label);
    let (text, color) = match &i.comment {
        Some(c) => (c.as_str(), t.color.text),
        None => ("No comment", t.color.dim),
    };
    let top = cy + 8.0 + t.font.label * t.font.line_height + 2.0;
    let line = t.font.small * t.font.line_height;
    let lines = (((384.0 - top) / line) as usize).max(1);
    for (k, l) in wrap(text, w, t.font.small, lines).iter().enumerate() {
        ui.text(
            l.clone(),
            x0,
            top + k as f32 * line,
            t.text.small.color(color).max_width(w),
        );
    }
}

impl InfoView {
    /// The value of field `label`, or a dash.
    fn field(&self, label: &str) -> &str {
        self.fields
            .iter()
            .find(|f| f.0 == label)
            .map_or("–", |f| f.1.as_str())
    }

    fn stars(&self) -> String {
        self.rating.map_or_else(
            || "–".to_string(),
            |r| {
                let r = usize::from(r.min(5));
                format!("{}{}", "★".repeat(r), "☆".repeat(5 - r))
            },
        )
    }
}

/// INFO in the browser: the selected track beside the list (jacket and title columns), as
/// the players show it: artwork with title, artist, album and genre; BPM, key, time and
/// rating; the comment.
fn side_panel(ui: &mut Ui, i: &InfoView) {
    let t = ui.t;
    let x0 = TITLE_X + TITLE_W + 12.0;
    let w = ARTIST_X + ARTIST_W - PAD - x0;
    let line_y = |y: f32, h: f32, size: f32| y + (h - size * t.font.line_height) * 0.5;

    // Artwork, and the names beside it.
    let side = 104.0;
    let ay = LIST_Y + 8.0;
    match &i.art {
        Some(art) => ui.scene.images.push(Image {
            picture: Arc::clone(art),
            x: x0,
            y: ay,
            w: side,
            h: side,
            radius: t.shape.button_radius,
        }),
        None => {
            ui.scene.rounded(
                x0,
                ay,
                side,
                side,
                t.shape.button_radius,
                t.color.background,
                t.shape.border,
                t.color.chip_edge,
            );
            ui.text(
                if i.folder { "FOLDER" } else { "NO ARTWORK" },
                x0 + side * 0.5,
                line_y(ay, side, t.font.small),
                t.text.small.color(t.color.dim).center(),
            );
        }
    }
    let tx = x0 + side + 12.0;
    let tw = x0 + w - tx;
    let title_label = if i.folder { "FOLDER" } else { "TITLE" };
    let title_color = if i.loaded {
        t.color.accent_text
    } else {
        t.color.text
    };
    let names = [
        (
            i.field(title_label),
            t.text.row.semibold().color(title_color),
        ),
        (i.field("ARTIST"), t.text.row),
        (i.field("ALBUM"), t.text.label),
        (i.field("GENRE"), t.text.label),
    ];
    let mut y = ay - 2.0;
    for (k, (text, style)) in names.into_iter().enumerate() {
        if i.folder && k > 0 {
            break;
        }
        ui.text(text.to_string(), tx, y, style.max_width(tw));
        y += style.line_box() + 2.0;
    }
    if i.folder {
        return;
    }

    // BPM, key, time, rating.
    let gy = ay + side + 10.0;
    ui.scene.fill(x0, gy, w, 1.0, t.color.row_rule);
    let stars = i.stars();
    let cells = [
        ("BPM", i.field("BPM")),
        ("KEY", i.field("KEY")),
        ("TIME", i.field("TIME")),
        ("RATING", stars.as_str()),
    ];
    let cw = w / cells.len() as f32;
    for (k, (label, value)) in cells.into_iter().enumerate() {
        let cx = x0 + k as f32 * cw;
        ui.text(label, cx, gy + 6.0, t.text.label);
        ui.text(
            value.to_string(),
            cx,
            gy + 6.0 + t.font.label * t.font.line_height,
            t.text.row.semibold().max_width(cw - 6.0),
        );
    }

    // Comment.
    let cy = gy + 62.0;
    ui.scene.fill(x0, cy, w, 1.0, t.color.row_rule);
    ui.text("COMMENT", x0, cy + 6.0, t.text.label);
    let (text, color) = match &i.comment {
        Some(c) => (c.as_str(), t.color.text),
        None => ("No comment", t.color.dim),
    };
    let top = cy + 6.0 + t.font.label * t.font.line_height;
    let room = LIST_Y + ROW_H * ROWS as f32 - top;
    let lines = ((room / (t.font.small * t.font.line_height)) as usize).max(1);
    for (k, l) in wrap(text, w, t.font.small, lines).iter().enumerate() {
        ui.text(
            l.clone(),
            x0,
            top + k as f32 * t.font.small * t.font.line_height,
            t.text.small.color(color).max_width(w),
        );
    }
}

/// Break `text` into at most `max` lines of about `width` design pixels at `size`, by words
/// (Barlow Condensed averages under half an em per character). The last line takes the
/// rest; the renderer ellipsizes it.
fn wrap(text: &str, width: f32, size: f32, max: usize) -> Vec<String> {
    let per_line = ((width / (size * 0.45)) as usize).max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if lines.len() + 1 == max {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
            continue;
        }
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > per_line {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// A list modal over the list (MENU, ADD TO PLAYLIST, a confirmation): a panel with its
/// title and up to [`MODAL_ROWS`] rows, scrolled to keep the cursor in view.
fn modal(ui: &mut Ui, m: &ModalView) {
    let t = ui.t;
    // Five rows fit between the header and the list's bottom edge (y 386).
    let (x, y, w) = (263.0, 108.0, 603.0);
    let (row_h, top) = (40.0, 46.0);
    let shown = m.rows.len().min(MODAL_ROWS);
    let h = top + shown as f32 * (row_h + 4.0) + 6.0;
    ui.scene.rounded(
        x,
        y,
        w,
        h,
        t.shape.section_radius,
        t.color.title_bar,
        t.shape.border_active,
        t.color.accent,
    );
    ui.text(
        m.title.clone(),
        x + 20.0,
        y + 10.0,
        t.text.row.semibold().max_width(w - 40.0),
    );
    let first = m
        .selected
        .saturating_sub(MODAL_ROWS - 1)
        .min(m.rows.len().saturating_sub(shown));
    for (k, label) in m.rows.iter().enumerate().skip(first).take(shown) {
        let ry = y + top + (k - first) as f32 * (row_h + 4.0);
        let rect = Rect::new(x + 16.0, ry, w - 32.0, row_h);
        let danger = m.danger_first && k == 0;
        if danger {
            let (fill, edge) = if m.selected == k {
                (t.color.danger_fill, t.color.danger)
            } else {
                (t.color.button, t.color.danger)
            };
            ui.scene.rounded(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                t.shape.button_radius,
                fill,
                t.shape.border,
                edge,
            );
            ui.text_in(
                label.clone(),
                rect.center_x(),
                ry,
                row_h,
                t.text.button.color(t.color.danger_text).center(),
            );
            ui.hit(Target::ModalRow(k as u16), rect);
        } else {
            ui.button(
                Target::ModalRow(k as u16),
                rect,
                label,
                None,
                m.selected == k,
            );
        }
    }
    if m.rows.len() > shown {
        // Where the rows are, at the right edge.
        let list_h = shown as f32 * (row_h + 4.0) - 4.0;
        let thumb = list_h * shown as f32 / m.rows.len() as f32;
        let max_first = (m.rows.len() - shown) as f32;
        let ty = y + top + (list_h - thumb) * (first as f32 / max_first);
        ui.scene
            .fill(x + w - 10.0, y + top, 2.0, list_h, t.color.row_rule);
        ui.scene.fill(x + w - 10.0, ty, 2.0, thumb, t.color.label);
    }
}

/// "Scan this stick?": over the list, until answered.
fn ask_scan(ui: &mut Ui) {
    let t = ui.t;
    let (x, y, w, h) = (263.0, 140.0, 603.0, 206.0);
    ui.scene.rounded(
        x,
        y,
        w,
        h,
        t.shape.section_radius,
        t.color.title_bar,
        t.shape.border_active,
        t.color.accent,
    );
    ui.text("This USB has no library", x + 24.0, y + 18.0, t.text.title);
    for (i, line) in [
        "Scan it to list every track under LIBRARY, with its tags?",
        "Scanning reads each file once. Without it, browse the folders;",
        "a track joins the library when it is loaded.",
    ]
    .into_iter()
    .enumerate()
    {
        ui.text(line, x + 24.0, y + 56.0 + i as f32 * 22.0, t.text.label);
    }
    let by = y + h - 68.0;
    ui.button(
        Target::ScanYes,
        Rect::new(x + 24.0, by, 260.0, 48.0),
        "SCAN",
        None,
        true,
    );
    ui.button(
        Target::ScanNo,
        Rect::new(x + w - 24.0 - 260.0, by, 260.0, 48.0),
        "FOLDERS ONLY",
        None,
        false,
    );
}

/// `m:ss` for a track length.
pub fn length(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_rows_are_eleven_units_and_keys_map_to_characters() {
        for row in KEYS {
            let units: f32 = row.iter().map(|k| k.2).sum();
            assert!((units - 11.0).abs() < 1e-6, "{units}");
        }
        assert_eq!(key_char(0), Some('1'));
        assert_eq!(key_char(10), Some(BACKSPACE));
        assert_eq!(key_char(11), Some('q'), "the row's gap is no key");
        assert_eq!(key_char(20), Some('p'));
        assert_eq!(key_char(30), Some(SHIFT));
        let n = key_count();
        assert_eq!(key_char(n as u8 - 1), Some(DONE), "DONE bottom right");
        assert_eq!(key_char(n as u8), None);
        assert!(keys().any(|k| k.1 == ' ' && k.2 >= 5.0), "a wide space bar");
    }

    #[test]
    fn shift_capitalises_one_letter_or_locks() {
        let s = Shift::Off.tap();
        assert_eq!(s, Shift::Once);
        let (c, s) = s.apply('p');
        assert_eq!((c, s), ('P', Shift::Off), "once: one letter");
        assert_eq!(
            Shift::Once.apply('1'),
            ('1', Shift::Once),
            "digits don't use it up"
        );
        let lock = Shift::Once.tap();
        assert_eq!(lock.apply('e'), ('E', Shift::Lock));
        assert_eq!(lock.tap(), Shift::Off);
        assert_eq!(Shift::Off.apply('k'), ('k', Shift::Off));
    }

    #[test]
    fn comments_wrap_by_words_into_at_most_the_lines_given() {
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let lines = wrap(text, 60.0, 14.0, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].chars().count() <= 9, "{lines:?}");
        assert!(lines[2].ends_with("twelve"), "the last line takes the rest");
        assert!(wrap("", 100.0, 14.0, 4).is_empty());
    }

    #[test]
    fn lengths_read_as_minutes_and_seconds() {
        assert_eq!(length(312.4), "5:12");
        assert_eq!(length(59.6), "1:00");
    }
}
