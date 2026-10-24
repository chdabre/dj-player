//! The download queue: SoulSeek's downloads in place of the list, in the order they were
//! asked for, opened from the QUEUE chip (the downloads' progress) in the header. CANCEL
//! stops the selected download; on a finished one it reads REMOVE and only takes it off the
//! list. Picking a downloaded file loads it, and picking a failed one tries again. BACK (or
//! the chip) closes the queue.

use std::path::Path;

use soulseek::{Hit, Snapshot, Transfer};

use super::soulseek::hit_row;
use super::state::BrowseState;
use super::{BrowseView, HitCursor, ROWS};
use crate::app::effects::Effect;
use crate::input::Nav;
use crate::screens::Cx;

impl BrowseState {
    /// The QUEUE chip: show the queue in place of the list, or close it.
    pub(super) fn toggle_queue(&mut self) {
        self.queue = match self.queue {
            Some(_) => None,
            None => Some(HitCursor::default()),
        };
        if self.queue.is_some() {
            self.side = false;
            self.keyboard = false;
        }
    }

    /// The knob and BACK in the queue.
    pub(super) fn queue_nav(&mut self, n: Nav, cx: &mut Cx) {
        let hits = queued(cx.soulseek);
        let Some(q) = self.queue.as_mut() else {
            return;
        };
        match n {
            Nav::Up => q.move_by(-1, &hits),
            Nav::Down => q.move_by(1, &hits),
            Nav::Push => self.queue_pick(cx),
            Nav::Back => self.queue = None,
            Nav::Tag | Nav::Untag => {}
        }
    }

    pub(super) fn queue_scroll(&mut self, delta: isize, cx: &Cx) {
        if let Some(q) = self.queue.as_mut() {
            q.scroll_by(delta, &queued(cx.soulseek));
        }
    }

    /// A tap on shown row `row`: select it, and pick it if it was selected already.
    pub(super) fn queue_tap(&mut self, row: usize, cx: &mut Cx) {
        let hits = queued(cx.soulseek);
        let Some(q) = self.queue.as_mut() else {
            return;
        };
        let i = q.top(&hits) + row;
        if i >= hits.len() {
            return;
        }
        let again = q.cursor(&hits) == i;
        q.select(i, &hits);
        if again {
            self.queue_pick(cx);
        }
    }

    /// The selected download: load it once it is here, or try a failed one again.
    fn queue_pick(&mut self, cx: &mut Cx) {
        let Some((hit, transfer)) = self.queue_selected(cx) else {
            return;
        };
        match transfer {
            Transfer::Done(path) => cx.effects.push(Effect::Load(path)),
            Transfer::Failed(_) => cx.effects.push(Effect::Download(hit)),
            Transfer::Queued | Transfer::Receiving { .. } => {}
        }
    }

    /// CANCEL (or REMOVE) on the selected download.
    pub(super) fn queue_cancel(&mut self, cx: &mut Cx) {
        if let Some((hit, _)) = self.queue_selected(cx) {
            cx.effects.push(Effect::CancelDownload(hit.key()));
        }
    }

    /// The download under the queue's cursor.
    fn queue_selected(&self, cx: &Cx) -> Option<(Hit, Transfer)> {
        let s = cx.soulseek?;
        let q = self.queue.as_ref()?;
        let hits = queued(Some(s));
        let hit = hits.get(q.cursor(&hits))?;
        let transfer = s.transfer(hit)?.clone();
        Some((hit.clone(), transfer))
    }

    /// The browse view while there are downloads: the QUEUE chip, and with the queue open,
    /// its rows in place of the list.
    pub(super) fn queue_view(
        &self,
        v: &mut BrowseView,
        s: Option<&Snapshot>,
        loaded: Option<&Path>,
    ) {
        if let Some((label, lit)) = s.and_then(queue_chip) {
            v.queue_chip = Some(label);
            v.queue_lit = lit || self.queue.is_some();
        }
        let Some(q) = &self.queue else {
            return;
        };
        let hits = queued(s);
        let (top, cursor) = (q.top(&hits), q.cursor(&hits));
        v.rows = hits
            .iter()
            .skip(top)
            .take(ROWS)
            .map(|h| hit_row(h, s.and_then(|s| s.transfer(h)), loaded))
            .collect();
        v.selected = cursor.checked_sub(top).filter(|&r| r < v.rows.len());
        v.first = top;
        v.total = hits.len();
        v.has_tracks = !hits.is_empty();
        v.empty = hits.is_empty().then(|| "No downloads".to_string());
        v.header = "DOWNLOADS".into();
        v.at_root = false;
        v.side = None;
        v.keyboard = false;
        v.queue_open = true;
        v.cancel = hits
            .get(cursor)
            .and_then(|h| s?.transfer(h))
            .map(|t| if t.active() { "CANCEL" } else { "REMOVE" });
    }
}

/// The queue's files, in its order.
fn queued(s: Option<&Snapshot>) -> Vec<Hit> {
    s.map_or_else(Vec::new, |s| {
        s.transfers.iter().map(|q| q.hit.clone()).collect()
    })
}

/// The QUEUE chip's label while there are downloads, and whether any is under way:
/// `QUEUE 2 · 43 %` while downloading, `QUEUE · 3` once all have finished.
fn queue_chip(s: &Snapshot) -> Option<(String, bool)> {
    if s.transfers.is_empty() {
        return None;
    }
    Some(match s.progress() {
        (0, _) => (format!("QUEUE · {}", s.transfers.len()), false),
        (n, pct) => (format!("QUEUE {n} · {pct} %"), true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soulseek::{Format, Quality, Queued};

    fn hit(name: &str) -> Hit {
        Hit {
            user: "peer".into(),
            path: format!("@@share\\{name}"),
            size: 100,
            quality: Quality::new(Format::Mp3, &Default::default()),
            free_slot: true,
            speed: 1,
        }
    }

    #[test]
    fn the_chip_shows_downloads_under_way_and_their_progress() {
        let mut s = Snapshot::default();
        assert_eq!(queue_chip(&s), None, "no downloads, no chip");
        s.transfers = vec![
            Queued {
                path: Default::default(),
                hit: hit("a.mp3"),
                transfer: Transfer::Receiving {
                    done: 50,
                    total: 100,
                },
            },
            Queued {
                path: Default::default(),
                hit: hit("b.mp3"),
                transfer: Transfer::Queued,
            },
            Queued {
                path: Default::default(),
                hit: hit("c.mp3"),
                transfer: Transfer::Done("/x/c.mp3".into()),
            },
        ];
        assert_eq!(queue_chip(&s), Some(("QUEUE 2 · 25 %".into(), true)));
        s.transfers.clear();
        s.transfers.push(Queued {
            path: Default::default(),
            hit: hit("c.mp3"),
            transfer: Transfer::Failed("gone".into()),
        });
        assert_eq!(queue_chip(&s), Some(("QUEUE · 1".into(), false)));
    }
}
