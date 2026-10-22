//! Over the list: a list modal (MENU, ADD TO PLAYLIST, a confirmation) and the "scan this
//! stick?" question.

use super::*;

/// A list modal over the list (MENU, ADD TO PLAYLIST, a confirmation): a panel with its
/// title and up to [`MODAL_ROWS`] rows, scrolled to keep the cursor in view.
pub(super) fn modal(ui: &mut Ui, m: &ModalView) {
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
            ui.hit(Target::Browse(BrowseTarget::ModalRow(k as u16)), rect);
        } else {
            ui.button(
                Target::Browse(BrowseTarget::ModalRow(k as u16)),
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
pub(super) fn ask_scan(ui: &mut Ui) {
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
        Target::Browse(BrowseTarget::ScanYes),
        Rect::new(x + 24.0, by, 260.0, 48.0),
        "SCAN",
        None,
        true,
    );
    ui.button(
        Target::Browse(BrowseTarget::ScanNo),
        Rect::new(x + w - 24.0 - 260.0, by, 260.0, 48.0),
        "FOLDERS ONLY",
        None,
        false,
    );
}
