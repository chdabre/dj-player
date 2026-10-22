//! The browse screen's own state and what input does to it: the list (taps, drags, the knob),
//! the header chips, the search keyboard, the side panel, playlist editing, and the "scan
//! this stick?" question. Handlers take a [`Cx`] and push [`Effect`]s; a handler that changes
//! the screen returns it ([`Show`]).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use engined::browse::{Browser, Entered, Kind, Source};
use engined::persist::Listing;

use super::{BACKSPACE, BrowseTarget, CLEAR, DONE, ROW_H, SHIFT, Shift, key_char};
use crate::app::effects::Effect;
use crate::input::Nav;
use crate::playlists::{self, Action, Editing};
use crate::screens::{Cx, ScreenKind, Show};

/// Finger travel (design pixels) before a touch on the list counts as a drag.
const DRAG_SLOP: f32 = 8.0;

/// A finger on the list: where it went down (design y, row), how far the list has scrolled
/// under it, and whether it moved enough to be a drag rather than a tap.
#[derive(Debug, Clone, Copy)]
struct ListDrag {
    y: f32,
    row: usize,
    scrolled: isize,
    moved: bool,
}

#[derive(Default)]
pub struct BrowseState {
    /// The search keyboard is up.
    pub keyboard: bool,
    /// The info panel is up beside the list.
    pub side: bool,
    /// The on-screen keyboard's shift.
    pub shift: Shift,
    /// Playlist editing: a modal over the list (MENU, ADD TO PLAYLIST, naming, a
    /// confirmation), or an entry the knob is moving.
    pub editing: Editing,
    /// Ask whether to scan this stick (it had no library).
    pub ask_scan: bool,
    list_drag: Option<ListDrag>,
    /// The unanalysed track count, and the listing it was counted from.
    unanalysed: RefCell<Option<(Arc<Listing>, usize)>>,
}

impl BrowseState {
    /// A touch on one of the browse screen's targets at design y `y` (BACK is the router's).
    pub fn tap(&mut self, t: BrowseTarget, y: f32, cx: &mut Cx) {
        match t {
            BrowseTarget::Back => {}
            BrowseTarget::Sort => {
                if let Some(b) = cx.browser.as_deref_mut() {
                    b.set_sort(b.sort().next());
                }
            }
            BrowseTarget::Search => {
                self.search(cx);
            }
            BrowseTarget::ModalRow(i) => {
                if let Some(m) = self.editing.modal.as_mut() {
                    m.select(usize::from(i));
                }
                self.modal_push(cx);
            }
            BrowseTarget::Scan => cx.effects.push(Effect::Scan),
            BrowseTarget::AnalyzeAll => self.analyze_all(cx),
            BrowseTarget::ScanYes => self.answer_scan(true, cx),
            BrowseTarget::ScanNo => self.answer_scan(false, cx),
            BrowseTarget::Key(i) => self.key(i, cx),
            BrowseTarget::Row(row) => {
                self.list_drag = Some(ListDrag {
                    y,
                    row: usize::from(row),
                    scrolled: 0,
                    moved: false,
                });
            }
        }
    }

    /// The finger moved to design y `y`. `None` if it is not on the list; else whether it
    /// drags the list (and so has left the row it went down on).
    pub fn drag(&mut self, y: f32, cx: &mut Cx) -> Option<bool> {
        let mut d = self.list_drag?;
        let travel = y - d.y;
        d.moved |= travel.abs() > DRAG_SLOP;
        if d.moved {
            // The list moves with the finger: dragging up shows later rows.
            let want = (-travel / ROW_H).round() as isize;
            if want != d.scrolled
                && let Some(b) = cx.browser.as_deref_mut()
            {
                b.scroll_by(want - d.scrolled);
                d.scrolled = want;
            }
        }
        self.list_drag = Some(d);
        Some(d.moved)
    }

    /// The finger came up: a touch on a row that did not drag is a tap on it.
    pub fn release(&mut self, cx: &mut Cx) {
        if let Some(d) = self.list_drag.take()
            && !d.moved
        {
            self.tap_row(d.row, cx);
        }
    }

    /// The knob, BACK and TAG TRACK on the browse screen.
    pub fn nav(&mut self, n: Nav, cx: &mut Cx) -> Show {
        if let Some(m) = self.editing.modal.as_mut() {
            match n {
                Nav::Up => m.move_by(-1),
                Nav::Down => m.move_by(1),
                Nav::Push => self.modal_push(cx),
                Nav::Back => self.editing.modal = None,
                Nav::Tag | Nav::Untag => {}
            }
            return None;
        }
        if self.editing.moving.is_some() {
            match n {
                Nav::Up => self.move_entry(-1, cx),
                Nav::Down => self.move_entry(1, cx),
                _ => self.editing.moving = None,
            }
            return None;
        }
        let b = cx.browser.as_deref_mut()?;
        match n {
            Nav::Tag | Nav::Untag => {}
            Nav::Up => b.move_by(-1),
            Nav::Down => b.move_by(1),
            Nav::Push => self.enter(cx),
            Nav::Back if self.side => self.side = false,
            Nav::Back => {
                let up = b.back();
                if b.search_query().is_none() {
                    self.keyboard = false;
                }
                if !up {
                    return Some(ScreenKind::Main);
                }
            }
        }
        None
    }

    /// A tap on shown row `row`: a folder opens; a track is selected, and loads if it was
    /// selected already.
    fn tap_row(&mut self, row: usize, cx: &mut Cx) {
        let Some(b) = cx.browser.as_deref_mut() else {
            return;
        };
        let i = b.level().top + row;
        let Some(entry) = b.level().entries.get(i) else {
            return;
        };
        let open = entry.kind != Kind::Track || b.level().cursor == i;
        b.select(i);
        if open {
            self.enter(cx);
        }
    }

    /// Open the selected folder, or load the selected track.
    fn enter(&mut self, cx: &mut Cx) {
        if let Some(b) = cx.browser.as_deref_mut()
            && let Entered::Track(path) = b.enter()
        {
            cx.effects.push(Effect::Load(path));
        }
    }

    /// SEARCH: start a search with the keyboard up, or show / hide the keyboard of the one
    /// under way. False without a browser.
    pub fn search(&mut self, cx: &mut Cx) -> bool {
        let Some(b) = cx.browser.as_deref_mut() else {
            return false;
        };
        self.side = false;
        if b.search_query().is_some() {
            self.keyboard = !self.keyboard;
        } else {
            b.search_begin();
            self.keyboard = true;
        }
        true
    }

    /// INFO on the browse screen: the selected track's details beside the list, or not.
    pub fn toggle_side(&mut self) {
        self.side = !self.side;
        if self.side {
            self.keyboard = false;
        }
    }

    /// A key of the on-screen keyboard: into the name being typed, or the search.
    fn key(&mut self, i: u8, cx: &mut Cx) {
        let Some(c) = key_char(i) else {
            return;
        };
        if c == SHIFT {
            self.shift = self.shift.tap();
            return;
        }
        let (c, shift) = self.shift.apply(c);
        self.shift = shift;
        if self.editing.naming() {
            self.type_name(c, cx);
        } else {
            self.type_char(c, cx);
        }
    }

    /// A key typed into the search (on-screen keyboard or a real one).
    pub fn type_char(&mut self, c: char, cx: &mut Cx) {
        let Some(b) = cx.browser.as_deref_mut() else {
            return;
        };
        let Some(q) = b.search_query() else {
            return;
        };
        let mut q = q.to_string();
        match c {
            BACKSPACE => {
                q.pop();
            }
            CLEAR => q.clear(),
            DONE => self.keyboard = false,
            c if !c.is_control() => q.push(c),
            _ => {}
        }
        b.set_query(&q);
    }

    /// A key typed into a name (on-screen keyboard or a real one); DONE names it.
    pub fn type_name(&mut self, c: char, cx: &mut Cx) {
        self.editing.type_name(c, cx.effects);
    }

    /// MENU: the actions on the selection, as a modal.
    pub fn open_menu(&mut self, cx: &mut Cx) {
        let Some(b) = cx.browser.as_deref() else {
            return;
        };
        if !self.editing.open_menu(b) {
            eprintln!("menu: nothing to do here");
            return;
        }
        self.keyboard = false;
    }

    /// The modal's push (or a touch on one of its rows). A name to type starts with a
    /// capital when it is empty.
    fn modal_push(&mut self, cx: &mut Cx) {
        self.editing.push(cx.browser.as_deref_mut(), cx.effects);
        self.shift = if self.editing.naming_empty() {
            Shift::Once
        } else {
            Shift::Off
        };
    }

    /// MOVE: the moving entry one place up (`-1`) or down, the cursor with it.
    fn move_entry(&mut self, delta: isize, cx: &mut Cx) {
        if let Some(b) = cx.browser.as_deref_mut() {
            self.editing.move_entry(delta, b, cx.effects);
        }
    }

    /// TAG TRACK: the selected track (on the browse screen) or the loaded one onto the tag
    /// list; `remove` (a long press) takes it off.
    pub fn tag_track(&mut self, remove: bool, browsing: bool, cx: &mut Cx) {
        let loaded = cx.loaded.map(|p| p.to_path_buf());
        let Some(path) = playlists::tag_target(cx.browser.as_deref(), browsing, loaded) else {
            return;
        };
        let tagged = cx
            .browser
            .as_deref()
            .map(|b| b.tag_entries(&path))
            .unwrap_or_default();
        let action = match (remove, tagged.is_empty()) {
            (true, false) => Action::Untag(tagged),
            (false, true) => Action::Tag(vec![path]),
            // Already tagged, or nothing to remove.
            _ => return,
        };
        self.editing
            .act(action, cx.browser.as_deref_mut(), cx.effects);
    }

    /// ANALYZE ALL: analyse every library track without our analysis, in a batch; or stop.
    pub fn analyze_all(&mut self, cx: &mut Cx) {
        let effect = if cx.library.batch.is_some() {
            Effect::StopAnalyzeAll
        } else {
            Effect::AnalyzeAll(unanalysed(cx.browser.as_deref(), cx.library.listing))
        };
        cx.effects.push(effect);
    }

    /// How many library tracks lack our analysis, counted once per listing.
    pub fn unanalysed_count(&self, b: &Browser, listing: Option<&Arc<Listing>>) -> usize {
        let Some(listing) = listing else {
            return 0;
        };
        if let Some((l, n)) = &*self.unanalysed.borrow()
            && Arc::ptr_eq(l, listing)
        {
            return *n;
        }
        let n = unanalysed(Some(b), Some(listing)).len();
        *self.unanalysed.borrow_mut() = Some((Arc::clone(listing), n));
        n
    }

    /// The answer to "scan this stick?": a scan shows the library filling in.
    pub fn answer_scan(&mut self, scan: bool, cx: &mut Cx) {
        self.ask_scan = false;
        if scan {
            cx.effects.push(Effect::Scan);
            if let Some(b) = cx.browser.as_deref_mut() {
                b.set_source(Source::Library);
            }
        }
    }
}

/// The library's tracks without our analysis, one file each, in path order.
fn unanalysed(b: Option<&Browser>, listing: Option<&Arc<Listing>>) -> Vec<PathBuf> {
    let (Some(b), Some(listing)) = (b, listing) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<_> = listing.values().filter(|l| !l.analysed).collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows.into_iter()
        .filter(|l| seen.insert(l.track))
        .map(|l| b.root().join(&l.path))
        .collect()
}
