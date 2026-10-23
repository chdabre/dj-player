//! The router: which screen is up, each screen's own state (kept while another is up), the
//! touch in progress, and where input goes. Handlers get a [`Cx`]; a handler that changes the
//! screen returns it ([`Show`]).

use std::path::Path;

use browser::{Browser, Source};
use soulseek::Snapshot;

use super::{BrowseState, BrowseTarget, DeckDisplay, HitBox, PerfState, ScreenKind, Target};
use super::{chrome, perform};
use crate::app::effects::Effects;
use crate::app::engine::{DeckSnapshot, LibraryView};
use crate::input::Nav;

/// What an input handler may use: the deck as the screen shows it, the browser, the library
/// to look at, and the effects to ask for.
pub struct Cx<'a> {
    pub deck: &'a DeckSnapshot,
    pub browser: Option<&'a mut Browser>,
    pub library: LibraryView<'a>,
    /// What SoulSeek has, as of this frame; `None` without an account (or a stick).
    pub soulseek: Option<&'a Snapshot>,
    /// The file on the deck, if any.
    pub loaded: Option<&'a Path>,
    /// The playhead the last frame showed (frames).
    pub shown: Option<f64>,
    pub effects: &'a mut Effects,
}

/// A screen a handler asks to show.
pub type Show = Option<ScreenKind>;

/// Which screen is up, each screen's own state (kept while another is up), and the touch
/// in progress. Input from the device screen and the browse controls comes here and goes to
/// the part of the screen it is for.
#[derive(Default)]
pub struct Screens {
    pub screen: ScreenKind,
    pub deck: DeckDisplay,
    pub perform: PerfState,
    pub browse: BrowseState,
    /// The target the finger is on (drawn pressed), until it lifts or drags the list.
    pub pressed: Option<Target>,
}

impl Screens {
    fn show(&mut self, s: Show) {
        if let Some(s) = s {
            self.screen = s;
        }
    }

    /// A finger (or click) went down on `hit` at design pixels (`x`, `y`).
    pub fn tap(&mut self, hit: HitBox, (x, y): (f32, f32), cx: &mut Cx) {
        self.pressed = Some(hit.target);
        match hit.target {
            Target::Chrome(t) => chrome::tap(self, t, cx),
            Target::Deck(t) => self.deck.tap(t, hit.rect, x, cx),
            Target::Perform(t) => {
                let (hot, beats) = (cx.deck.hot_cues, cx.deck.loop_beats());
                if let Some(cmd) = perform::press(&mut self.perform, t, hot, beats) {
                    cx.effects.push(crate::app::effects::Effect::Deck(cmd));
                }
            }
            Target::Browse(BrowseTarget::Back) => self.nav(Nav::Back, cx),
            Target::Browse(BrowseTarget::Search) => {
                if self.browse.search(cx) {
                    self.screen = ScreenKind::Browse;
                }
            }
            Target::Browse(t) => self.browse.tap(t, y, cx),
        }
    }

    /// The finger moved to design pixels (`x`, `y`).
    pub fn drag(&mut self, (x, y): (f32, f32), cx: &mut Cx) {
        let pressed = match self.pressed {
            Some(Target::Deck(t)) => Some(t),
            _ => None,
        };
        if self.deck.drag(pressed, x, cx) {
            return;
        }
        if self.browse.drag(y, cx) == Some(true) {
            self.pressed = None;
        }
    }

    /// The finger came up.
    pub fn release(&mut self, cx: &mut Cx) {
        self.pressed = None;
        self.deck.release(cx);
        self.browse.release(cx);
    }

    /// A browse control: the knob turned or pushed, BACK, TAG TRACK.
    pub fn nav(&mut self, n: Nav, cx: &mut Cx) {
        if self.screen == ScreenKind::Info && n == Nav::Back {
            self.screen = ScreenKind::Main;
            return;
        }
        if n == Nav::Tag || n == Nav::Untag {
            let browsing = self.screen == ScreenKind::Browse;
            return self.browse.tag_track(n == Nav::Untag, browsing, cx);
        }
        if cx.browser.is_none() {
            return;
        }
        if self.screen != ScreenKind::Browse {
            // Pushing the knob elsewhere opens the browser.
            if n == Nav::Push {
                self.screen = ScreenKind::Browse;
            }
            return;
        }
        let show = self.browse.nav(n, cx);
        self.show(show);
    }

    /// The mouse wheel turned `rows` over the device screen.
    pub fn wheel(&mut self, rows: i32, cx: &mut Cx) {
        if self.screen == ScreenKind::Browse {
            self.browse.scroll_by(rows as isize, cx);
        }
    }

    /// PERFORM: the performance screen, or back to the main screen.
    pub fn toggle_perform(&mut self) {
        self.screen = match self.screen {
            ScreenKind::Performance => ScreenKind::Main,
            _ => ScreenKind::Performance,
        };
    }

    /// BROWSE: the browse screen, or back to the main screen (with a browser).
    pub fn toggle_browse(&mut self, cx: &Cx) {
        if cx.browser.is_some() {
            self.screen = match self.screen {
                ScreenKind::Browse => ScreenKind::Main,
                _ => ScreenKind::Browse,
            };
        }
    }

    /// SEARCH from anywhere.
    pub fn search(&mut self, cx: &mut Cx) {
        if self.browse.search(cx) {
            self.screen = ScreenKind::Browse;
        }
    }

    /// INFO: in the browser, the selected track's details beside the list (or not); on any
    /// other screen, the INFO screen of the loaded track (or back to the main screen).
    pub fn toggle_info(&mut self) {
        match self.screen {
            ScreenKind::Browse => self.browse.toggle_side(),
            ScreenKind::Info => self.screen = ScreenKind::Main,
            _ => self.screen = ScreenKind::Info,
        }
    }

    /// Show the browser at `source`.
    pub(super) fn browse_source(&mut self, source: Source, cx: &mut Cx) {
        if let Some(b) = cx.browser.as_deref_mut() {
            b.set_source(source);
            self.browse.keyboard = false;
            self.screen = ScreenKind::Browse;
        }
    }

    /// A track loaded: the main screen shows it.
    pub fn loaded(&mut self) {
        self.browse.keyboard = false;
        self.deck.loaded();
        self.screen = ScreenKind::Main;
    }
}
