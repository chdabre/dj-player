//! A track's details: the INFO screen's panel, and the browser's side panel beside the list.

use super::*;
use crate::screens::chrome::CONTENT_X;

/// The INFO screen: a header with the source and the track's place, then the artwork as
/// large as the panel is tall, and beside it the names, large BPM / key / time / rating
/// readouts and the comment, using the whole width. (The canvas's "Info" artboard puts the
/// fields in a narrow list and leaves a third of the panel empty.)
pub(crate) fn info_panel(ui: &mut Ui, i: &InfoView) {
    let t = ui.t;
    let line_y = |y: f32, h: f32, size: f32| y + (h - size * t.font.line_height) * 0.5;
    // Header bar.
    let (hx, hy, hw, hh) = (CONTENT_X, 69.0, 903.0, 35.0);
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
    let (ax, ay) = (CONTENT_X, 111.0);
    let side = 384.0 - ay;
    match &i.art {
        Some(art) => ui.scene.image(Image {
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
pub(super) fn side_panel(ui: &mut Ui, i: &InfoView) {
    let t = ui.t;
    let x0 = TITLE_X + TITLE_W + 12.0;
    let w = ARTIST_X + ARTIST_W - PAD - x0;
    let line_y = |y: f32, h: f32, size: f32| y + (h - size * t.font.line_height) * 0.5;

    // Artwork, and the names beside it.
    let side = 104.0;
    let ay = LIST_Y + 8.0;
    match &i.art {
        Some(art) => ui.scene.image(Image {
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
pub(super) fn wrap(text: &str, width: f32, size: f32, max: usize) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_wrap_by_words_into_at_most_the_lines_given() {
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let lines = wrap(text, 60.0, 14.0, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].chars().count() <= 9, "{lines:?}");
        assert!(lines[2].ends_with("twelve"), "the last line takes the rest");
        assert!(wrap("", 100.0, 14.0, 4).is_empty());
    }
}
