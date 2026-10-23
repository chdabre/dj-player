//! The search's SoulSeek source: the query typed for it, the cursor on its hits, what input
//! does there, and the list it shows. Typing doesn't search as it does on the stick: DONE
//! (labelled SEARCH) or Enter sends the query. The hits come best quality first; picking one
//! downloads it into the stick's downloads folder, and picking it once it is there loads it.

use std::path::Path;

use soulseek::{Hit, HitKey, Snapshot, Status, Transfer};

use super::state::BrowseState;
use super::{BrowseView, ROWS, Row, RowKind, SearchSource, length};
use crate::app::effects::Effect;
use crate::screens::Cx;

/// The SoulSeek search as the screen holds it.
#[derive(Debug, Default)]
pub struct SoulseekSearch {
    /// The query being typed (sent with DONE).
    pub query: String,
    /// The hit under the cursor, by key: hits re-rank as they come in, and it stays on its
    /// hit. `None`: the first.
    picked: Option<HitKey>,
    /// The first row shown.
    top: usize,
}

impl SoulseekSearch {
    /// The cursor's index among `hits`.
    pub fn cursor(&self, hits: &[Hit]) -> usize {
        self.picked
            .as_ref()
            .and_then(|k| {
                hits.iter()
                    .position(|h| h.user == k.user && h.path == k.path)
            })
            .unwrap_or(0)
    }

    /// The first row shown, kept in range.
    fn top(&self, hits: &[Hit]) -> usize {
        self.top.min(hits.len().saturating_sub(ROWS))
    }

    /// Put the cursor on hit `i` (clamped), keeping it among the rows shown.
    pub fn select(&mut self, i: usize, hits: &[Hit]) {
        let Some(last) = hits.len().checked_sub(1) else {
            return;
        };
        let i = i.min(last);
        self.picked = Some(hits[i].key());
        self.top = self.top(hits);
        if i < self.top {
            self.top = i;
        } else if i >= self.top + ROWS {
            self.top = i + 1 - ROWS;
        }
    }

    pub fn move_by(&mut self, delta: isize, hits: &[Hit]) {
        self.select(self.cursor(hits).saturating_add_signed(delta), hits);
    }

    /// Scroll by `delta` rows; the cursor stays among the rows shown.
    pub fn scroll_by(&mut self, delta: isize, hits: &[Hit]) {
        if hits.is_empty() {
            return;
        }
        self.top = self
            .top(hits)
            .saturating_add_signed(delta)
            .min(hits.len().saturating_sub(ROWS));
        let bottom = (self.top + ROWS).min(hits.len()) - 1;
        let cursor = self.cursor(hits).clamp(self.top, bottom);
        self.picked = Some(hits[cursor].key());
    }

    /// A new search: the cursor back to the top.
    fn restart(&mut self) {
        self.picked = None;
        self.top = 0;
    }
}

impl BrowseState {
    /// The search is under way and looks through SoulSeek.
    pub(super) fn on_soulseek(&self, cx: &Cx) -> bool {
        self.source == SearchSource::Soulseek
            && cx
                .browser
                .as_deref()
                .is_some_and(|b| b.search_query().is_some())
    }

    /// A source chip: search the stick or SoulSeek, taking the query typed so far along.
    pub(super) fn set_source(&mut self, source: SearchSource, cx: &mut Cx) {
        if source == self.source {
            return;
        }
        let Some(b) = cx.browser.as_deref_mut() else {
            return;
        };
        match source {
            SearchSource::Soulseek => {
                self.soulseek.query = b.search_query().unwrap_or_default().to_string();
            }
            SearchSource::Library => b.set_query(&self.soulseek.query),
        }
        self.source = source;
        self.side = false;
        self.keyboard = true;
    }

    /// A key typed into the SoulSeek query; DONE sends it.
    pub(super) fn type_soulseek(&mut self, c: char, cx: &mut Cx) {
        let q = &mut self.soulseek.query;
        match c {
            super::BACKSPACE => {
                q.pop();
            }
            super::CLEAR => q.clear(),
            super::DONE => {
                if !q.trim().is_empty() && cx.soulseek.is_some() {
                    cx.effects
                        .push(Effect::SoulseekSearch(q.trim().to_string()));
                    self.soulseek.restart();
                }
                self.keyboard = false;
            }
            c if !c.is_control() => q.push(c),
            _ => {}
        }
    }

    /// The hits shown, as of this frame.
    fn hits<'c>(cx: &'c Cx) -> &'c [Hit] {
        cx.soulseek.map_or(&[], |s| s.hits.as_slice())
    }

    pub(super) fn soulseek_move(&mut self, delta: isize, cx: &Cx) {
        self.soulseek.move_by(delta, Self::hits(cx));
    }

    pub(super) fn soulseek_scroll(&mut self, delta: isize, cx: &Cx) {
        self.soulseek.scroll_by(delta, Self::hits(cx));
    }

    /// A tap on shown row `row`: select the hit, and pick it if it was selected already.
    pub(super) fn soulseek_tap(&mut self, row: usize, cx: &mut Cx) {
        let hits = Self::hits(cx);
        let i = self.soulseek.top(hits) + row;
        if i >= hits.len() {
            return;
        }
        let again = self.soulseek.cursor(hits) == i;
        self.soulseek.select(i, hits);
        if again {
            self.soulseek_pick(cx);
        }
    }

    /// The hit under the cursor: download it, or load it once it is downloaded.
    pub(super) fn soulseek_pick(&mut self, cx: &mut Cx) {
        let Some(s) = cx.soulseek else {
            return;
        };
        let Some(hit) = s.hits.get(self.soulseek.cursor(&s.hits)) else {
            return;
        };
        let effect = match s.transfer(hit) {
            Some(Transfer::Done(path)) => Effect::Load(path.clone()),
            Some(t) if t.active() => return,
            _ => Effect::Download(hit.clone()),
        };
        cx.effects.push(effect);
    }

    /// The browse view's list while searching SoulSeek: the hits, the header with the query,
    /// and what the list says while there are none. `loaded` is the file on the deck.
    pub(super) fn soulseek_view(
        &self,
        v: &mut BrowseView,
        s: Option<&Snapshot>,
        loaded: Option<&Path>,
    ) {
        let hits = s.map_or(&[][..], |s| s.hits.as_slice());
        let top = self.soulseek.top(hits);
        let cursor = self.soulseek.cursor(hits);
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
        v.side = None;
        let typing = if v.keyboard { "▏" } else { "" };
        let status = match s.map(|s| &s.status) {
            Some(Status::Searching) if !hits.is_empty() => format!("  ·  SEARCHING {}", hits.len()),
            Some(Status::Done) if !hits.is_empty() => format!("  ·  {} FILES", hits.len()),
            _ => String::new(),
        };
        v.header = format!("SOULSEEK:  {}{typing}{status}", self.soulseek.query);
        v.empty = hits.is_empty().then(|| empty(s));
    }
}

/// A hit as a row: its name; the download's state, the peer and the size; its quality and
/// length.
fn hit_row(h: &Hit, transfer: Option<&Transfer>, loaded: Option<&Path>) -> Row {
    let state = match transfer {
        None => String::new(),
        Some(Transfer::Queued) => "QUEUED  ·  ".into(),
        Some(Transfer::Receiving { done, total }) => {
            let pct = (*done * 100).checked_div(*total).unwrap_or(0);
            format!("{pct} %  ·  ")
        }
        Some(Transfer::Done(_)) => "DOWNLOADED  ·  ".into(),
        Some(Transfer::Failed(e)) => format!("FAILED ({e})  ·  "),
    };
    let detail = [
        Some(h.quality.label()),
        h.quality.duration.map(|d| length(f64::from(d))),
    ];
    Row {
        kind: RowKind::Track,
        title: h.title().to_string(),
        artist: format!("{state}{}  ·  {}", h.user, megabytes(h.size)),
        detail: detail.into_iter().flatten().collect::<Vec<_>>().join("   "),
        loaded: matches!(transfer, Some(Transfer::Done(p)) if Some(p.as_path()) == loaded),
        art: None,
        tagged: false,
        missing: false,
    }
}

/// `12.3 MB`.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// What the list says before there are hits.
fn empty(s: Option<&Snapshot>) -> String {
    let Some(s) = s else {
        return "Set SOULSEEK_USERNAME and SOULSEEK_PASSWORD to search SoulSeek".into();
    };
    match &s.status {
        Status::Idle => "Type what to look for, then SEARCH".into(),
        Status::Connecting => "Logging in to SoulSeek…".into(),
        Status::Searching => "Searching SoulSeek…".into(),
        Status::Done => "Nothing found on SoulSeek".into(),
        Status::Failed(e) => format!("SoulSeek: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soulseek::{Format, Quality};

    fn hits(n: usize) -> Vec<Hit> {
        (0..n)
            .map(|i| Hit {
                user: format!("peer{i}"),
                path: format!("@@share\\Artist\\{i:02} Track.flac"),
                size: 30_000_000,
                quality: Quality::new(Format::Flac, &[(5, 16), (4, 44_100), (1, 312)].into()),
                free_slot: true,
                speed: 1,
            })
            .collect()
    }

    #[test]
    fn the_cursor_stays_on_its_hit_as_hits_rerank() {
        let mut hs = hits(10);
        let mut s = SoulseekSearch::default();
        assert_eq!(s.cursor(&hs), 0);
        s.move_by(3, &hs);
        assert_eq!(s.cursor(&hs), 3);
        // A better hit arrives at the top: the cursor follows its hit down.
        hs.insert(0, hits(11).pop().unwrap());
        assert_eq!(s.cursor(&hs), 4);
        s.move_by(100, &hs);
        assert_eq!(s.cursor(&hs), 10);
        assert_eq!(s.top(&hs), 10 + 1 - ROWS, "kept in view");
        s.scroll_by(-100, &hs);
        assert_eq!(
            (s.top(&hs), s.cursor(&hs)),
            (0, ROWS - 1),
            "the cursor stays shown"
        );
    }

    #[test]
    fn rows_show_quality_length_peer_size_and_the_download() {
        let h = &hits(1)[0];
        let r = hit_row(h, None, None);
        assert_eq!(r.title, "00 Track");
        assert_eq!(r.detail, "FLAC 16/44.1   5:12");
        assert_eq!(r.artist, "peer0  ·  30.0 MB");
        let r = hit_row(
            h,
            Some(&Transfer::Receiving {
                done: 15,
                total: 60,
            }),
            None,
        );
        assert!(r.artist.starts_with("25 %"), "{}", r.artist);
        let path = std::path::PathBuf::from("/stick/SoulSeek Downloads/00 Track.flac");
        let r = hit_row(h, Some(&Transfer::Done(path.clone())), Some(&path));
        assert!(r.artist.starts_with("DOWNLOADED") && r.loaded);
    }

    #[test]
    fn the_list_says_why_it_is_empty() {
        assert!(empty(None).contains("SOULSEEK_USERNAME"));
        let s = Snapshot {
            status: Status::Failed("Authentication failed".into()),
            ..Snapshot::default()
        };
        assert_eq!(empty(Some(&s)), "SoulSeek: Authentication failed");
    }
}
