//! The list and its header: the path and the chips (SEARCH, SORT, SCAN, ANALYZE ALL, and
//! the search's sources), seven
//! rows with jacket, title, artist and details, and the scroll bar.

use super::*;
use crate::screens::chrome::CONTENT_X;

pub(super) const HEADER: Rect = Rect::new(CONTENT_X, 69.0, 903.0, 31.0);

pub(super) const LIST_Y: f32 = 103.0;
/// The rows' area, dimmed under a modal.
pub(super) const LIST_AREA: Rect = Rect::new(107.0, LIST_Y, 913.0, ROW_H * ROWS as f32);

pub(super) const JACKET_X: f32 = CONTENT_X;

pub(super) const JACKET_W: f32 = 95.0;

/// The artwork square in the jacket column.
pub(super) const JACKET_SIDE: f32 = 34.0;

pub(super) const TITLE_X: f32 = 208.0;

pub(super) const TITLE_W: f32 = 352.0;

pub(super) const ARTIST_X: f32 = 560.0;

pub(super) const ARTIST_W: f32 = 456.0;

pub(super) const PAD: f32 = 11.0;

/// Room for the BPM, key and time at the right of the artist column.
pub(super) const DETAIL_W: f32 = 190.0;

/// The header chips, from the right: SEARCH, SORT, SCAN.
pub(super) const SEARCH_CHIP: (f32, f32) = (916.0, 100.0);

pub(super) const SORT_CHIP: (f32, f32) = (796.0, 114.0);

pub(super) const SCAN_CHIP: (f32, f32) = (706.0, 84.0);

pub(super) const ANALYZE_CHIP: (f32, f32) = (576.0, 124.0);

/// While searching, the source chips left of SORT: LIBRARY, SOULSEEK.
pub(super) const LIBRARY_CHIP: (f32, f32) = (586.0, 100.0);

pub(super) const SOULSEEK_CHIP: (f32, f32) = (690.0, 100.0);

/// The chips' touch targets run from the section's top to the first row.
const CHIP_HIT: (f32, f32) = (66.0, LIST_Y - 66.0);

pub(super) fn header(ui: &mut Ui, b: &BrowseView) {
    let t = ui.t;
    let Rect { x, y, w, h } = HEADER;
    let (hit_y, hit_h) = CHIP_HIT;
    let pressed_back = ui.pressed == Some(Target::Browse(BrowseTarget::Back));
    ui.scene.fill(x, y + h - 1.0, w, 1.0, t.color.button_edge);
    let chips_left = chips(ui, b);
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
        ui.hit(
            Target::Browse(BrowseTarget::Back),
            Rect::new(x, hit_y, back_w, hit_h),
        );
    }
}

/// The header's chips and progress, from the right; returns where they start (the path
/// ends before it).
fn chips(ui: &mut Ui, b: &BrowseView) -> f32 {
    let t = ui.t;
    let Rect { y, h, .. } = HEADER;
    let (hit_y, hit_h) = CHIP_HIT;
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
    chip(
        ui,
        Target::Browse(BrowseTarget::Search),
        SEARCH_CHIP,
        "SEARCH",
        b.searching,
    );
    // SoulSeek's hits are in quality order.
    if b.search_source == SearchSource::Library || !b.searching {
        chip(
            ui,
            Target::Browse(BrowseTarget::Sort),
            SORT_CHIP,
            &format!("SORT: {}", b.sort),
            false,
        );
    }
    if b.searching {
        for (source, pos, label) in [
            (SearchSource::Library, LIBRARY_CHIP, "LIBRARY"),
            (SearchSource::Soulseek, SOULSEEK_CHIP, "SOULSEEK"),
        ] {
            chip(
                ui,
                Target::Browse(BrowseTarget::SearchSource(source)),
                pos,
                label,
                b.search_source == source,
            );
        }
        return LIBRARY_CHIP.0;
    }
    if b.library && b.can_scan && b.scan.is_none() {
        chip(
            ui,
            Target::Browse(BrowseTarget::Scan),
            SCAN_CHIP,
            "SCAN",
            false,
        );
        chips_left = SCAN_CHIP.0;
        match b.analyzing {
            Some((done, total, stopping)) => {
                chip(
                    ui,
                    Target::Browse(BrowseTarget::AnalyzeAll),
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
                chip(
                    ui,
                    Target::Browse(BrowseTarget::AnalyzeAll),
                    ANALYZE_CHIP,
                    "ANALYZE ALL",
                    false,
                );
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
    chips_left
}

pub(super) fn list(ui: &mut Ui, b: &BrowseView) {
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
    for (i, row) in b.rows.iter().enumerate().take(ROWS) {
        list_row(ui, b, i, row, title_w);
    }
    scroll_bar(ui, b, list_h);
}

/// Shown row `i`: the cursor, the rule, the jacket, the title and (without the info panel)
/// the artist and details.
fn list_row(ui: &mut Ui, b: &BrowseView, i: usize, row: &Row, title_w: f32) {
    let t = ui.t;
    let right = list_right(b);
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
            Target::Browse(BrowseTarget::Row(i as u8)),
            Rect::new(JACKET_X, y, right - JACKET_X, ROW_H),
        );
        return;
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
        Target::Browse(BrowseTarget::Row(i as u8)),
        Rect::new(JACKET_X, y, ARTIST_X + ARTIST_W - JACKET_X, ROW_H),
    );
}

/// The list's right edge: the artist column's, or the title column's with the info panel.
pub(super) fn list_right(b: &BrowseView) -> f32 {
    if b.side.is_some() {
        TITLE_X + TITLE_W
    } else {
        ARTIST_X + ARTIST_W
    }
}

/// A row's cell text, ellipsized to `w`.
pub(super) fn cell(ui: &mut Ui, s: &str, x: f32, y: f32, w: f32, weight: Weight, color: Rgba) {
    let t = ui.t;
    ui.text(
        s.to_string(),
        x,
        y,
        t.text.row.weight(weight).color(color).max_width(w),
    );
}

/// The jacket column: the artwork, a frame where there is none (yet), or a folder.
pub(super) fn jacket(ui: &mut Ui, row: &Row, y: f32) {
    let t = ui.t;
    let side = JACKET_SIDE;
    let (x, top) = (JACKET_X + (JACKET_W - side) * 0.5, y + (ROW_H - side) * 0.5);
    match (row.kind, &row.art) {
        (RowKind::Track, Some(art)) => ui.scene.image(Image {
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
pub(super) fn scroll_bar(ui: &mut Ui, b: &BrowseView, list_h: f32) {
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
