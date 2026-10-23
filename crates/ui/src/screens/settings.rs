//! The SETTINGS screen: MENU on the playback screens opens it, and MENU or BACK closes it.
//! For now it has one section, SOULSEEK: the account's user name and password, and FORGET
//! ACCOUNT. A field is edited with the search keyboard (or a real one): DONE keeps the
//! account on the player (never on a stick) and logs in with it; BACK or Esc gives up. The
//! password shows as dots except while it is typed.

use soulseek::{Credentials, Snapshot, Status};
use winit::keyboard::{Key, NamedKey};

use super::browse::{BACKSPACE, CLEAR, DONE, SHIFT, Shift, key_char, keyboard};
use super::chrome::{self, CONTENT_X, Chrome, Top};
use super::deck::{bottom, status};
use super::{ChromeTarget, Cx, ScreenKind, Show, Target, Ui, View};
use crate::app::effects::Effect;
use crate::app::keymap::{self, KeyPress};
use crate::input::Nav;
use crate::kit::Rect;
use crate::waves::OverviewCache;

/// The settings screen's touch targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTarget {
    /// A setting's row ([`ROWS`] index).
    Row(u8),
    /// A key of the on-screen keyboard.
    Key(u8),
}

/// A field that is typed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    User,
    Password,
}

/// The SOULSEEK section's rows.
pub const ROWS: [&str; 3] = ["USERNAME", "PASSWORD", "FORGET ACCOUNT"];

/// Where the rows start, and their pitch.
const ROW_Y: f32 = 106.0;
const ROW_PITCH: f32 = 52.0;
const ROW_H: f32 = 46.0;
const ROW_W: f32 = 903.0;
/// The values' column.
const VALUE_X: f32 = CONTENT_X + 230.0;

#[derive(Debug, Default)]
pub struct SettingsState {
    /// The account as set here (either may be empty while it is being entered).
    pub user: String,
    pub password: String,
    /// The selected row.
    pub cursor: usize,
    /// The field being typed into, and its text so far.
    pub editing: Option<(Field, String)>,
    pub shift: Shift,
}

impl SettingsState {
    /// The settings as kept: `account`, if there is one.
    pub fn new(account: Option<&Credentials>) -> Self {
        Self {
            user: account.map(|a| a.user.clone()).unwrap_or_default(),
            password: account.map(|a| a.password.clone()).unwrap_or_default(),
            ..Self::default()
        }
    }

    /// The account, once both fields are set.
    fn account(&self) -> Option<Credentials> {
        (!self.user.trim().is_empty() && !self.password.is_empty()).then(|| Credentials {
            user: self.user.trim().to_string(),
            password: self.password.clone(),
        })
    }

    /// A key of a real keyboard: while a field is typed in, it takes the keys (Enter keeps
    /// the field, Esc gives up). True if it took the key.
    pub fn key(&mut self, key: &KeyPress, cx: &mut Cx) -> bool {
        if self.editing.is_none() {
            return false;
        }
        if key.pressed {
            if key.key == Key::Named(NamedKey::Escape) {
                self.editing = None;
            }
            for c in keymap::typed(key) {
                self.type_char(c, cx);
            }
        }
        true
    }

    pub fn tap(&mut self, t: SettingsTarget, cx: &mut Cx) {
        match t {
            SettingsTarget::Row(i) => {
                let i = usize::from(i);
                let again = self.cursor == i;
                self.cursor = i.min(ROWS.len() - 1);
                if again || self.editing.is_some() {
                    self.editing = None;
                    self.push(cx);
                }
            }
            SettingsTarget::Key(i) => {
                let Some(c) = key_char(i) else {
                    return;
                };
                if c == SHIFT {
                    self.shift = self.shift.tap();
                    return;
                }
                let (c, shift) = self.shift.apply(c);
                self.shift = shift;
                self.type_char(c, cx);
            }
        }
    }

    /// The knob and BACK: move, edit (or FORGET), give up typing, or leave the screen.
    pub fn nav(&mut self, n: Nav, cx: &mut Cx) -> Show {
        match n {
            Nav::Up if self.editing.is_none() => self.cursor = self.cursor.saturating_sub(1),
            Nav::Down if self.editing.is_none() => {
                self.cursor = (self.cursor + 1).min(ROWS.len() - 1);
            }
            Nav::Push if self.editing.is_some() => self.type_char(DONE, cx),
            Nav::Push => self.push(cx),
            Nav::Back if self.editing.is_some() => self.editing = None,
            Nav::Back => return Some(ScreenKind::Main),
            _ => {}
        }
        None
    }

    /// The selected row: type its field, or forget the account.
    fn push(&mut self, cx: &mut Cx) {
        match self.cursor {
            0 => self.editing = Some((Field::User, self.user.clone())),
            1 => self.editing = Some((Field::Password, self.password.clone())),
            _ => {
                self.user.clear();
                self.password.clear();
                cx.effects.push(Effect::SetAccount(None));
            }
        }
    }

    /// A key typed into the field; DONE keeps it.
    fn type_char(&mut self, c: char, cx: &mut Cx) {
        let Some((field, text)) = self.editing.as_mut() else {
            return;
        };
        match c {
            BACKSPACE => {
                text.pop();
            }
            CLEAR => text.clear(),
            DONE => {
                let text = std::mem::take(text);
                match field {
                    Field::User => self.user = text,
                    Field::Password => self.password = text,
                }
                self.editing = None;
                cx.effects.push(Effect::SetAccount(self.account()));
            }
            c if !c.is_control() => text.push(c),
            _ => {}
        }
    }

    /// What the screen shows; `soulseek` is the client's state (`None` without a stick).
    pub fn view(&self, soulseek: Option<&Snapshot>) -> SettingsView {
        let shown = |field, value: &str| match &self.editing {
            Some((f, text)) if *f == field => format!("{text}▏"),
            _ if value.is_empty() => "not set".into(),
            _ if field == Field::Password => "•".repeat(value.chars().count().min(12)),
            _ => value.to_string(),
        };
        SettingsView {
            rows: vec![
                (ROWS[0], shown(Field::User, &self.user)),
                (ROWS[1], shown(Field::Password, &self.password)),
                (ROWS[2], String::new()),
            ],
            selected: self.cursor,
            editing: self.editing.is_some(),
            shift: self.shift,
            status: account_status(self.account().is_some(), soulseek),
        }
    }
}

/// A line on the account: whether it is set, and how the last login went.
fn account_status(set: bool, soulseek: Option<&Snapshot>) -> String {
    let Some(s) = soulseek else {
        return "SoulSeek needs a USB stick: downloads go to its SoulSeek Downloads folder".into();
    };
    if !set {
        return "No account: set a user name and password to search SoulSeek".into();
    }
    match &s.status {
        Status::Idle => "Logs in at the first search".into(),
        Status::Connecting => "Logging in…".into(),
        Status::Searching | Status::Done => "Logged in".into(),
        Status::Failed(e) => format!("Last login: {e}"),
    }
}

/// What the settings screen shows.
#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    /// (label, value) per row.
    pub rows: Vec<(&'static str, String)>,
    pub selected: usize,
    /// A field is typed in: the keyboard is up.
    pub editing: bool,
    pub shift: Shift,
    pub status: String,
}

/// Lay out the settings screen.
pub(crate) fn build(ui: &mut Ui, v: &View, s: &SettingsView, overview: &mut OverviewCache) {
    let t = ui.t;
    chrome::frame(
        ui,
        v,
        Chrome {
            top: Top::Tabs {
                lit: Target::Chrome(ChromeTarget::Usb),
                tab: Some(Target::Chrome(ChromeTarget::Menu)),
            },
            lower: !s.editing,
        },
    );
    ui.text("SOULSEEK", CONTENT_X + 11.0, 74.0, t.text.row.semibold());
    ui.scene
        .fill(CONTENT_X, 100.0, ROW_W, 1.0, t.color.button_edge);
    for (i, (label, value)) in s.rows.iter().enumerate() {
        let r = Rect::new(CONTENT_X, ROW_Y + i as f32 * ROW_PITCH, ROW_W, ROW_H);
        ui.button_styled(
            Target::Settings(SettingsTarget::Row(i as u8)),
            r,
            "",
            None,
            s.selected == i,
            t.text.row,
        );
        let label_style = if value.is_empty() && i == ROWS.len() - 1 {
            t.text.row.semibold().color(t.color.danger_text)
        } else {
            t.text.row.semibold()
        };
        ui.text_in(*label, r.x + 16.0, r.y, r.h, label_style);
        ui.text_in(
            value.clone(),
            VALUE_X,
            r.y,
            r.h,
            t.text.row.max_width(r.x + r.w - 16.0 - VALUE_X),
        );
    }
    let note_y = ROW_Y + ROWS.len() as f32 * ROW_PITCH + 6.0;
    ui.text(
        s.status.clone(),
        CONTENT_X + 11.0,
        note_y,
        t.text.label.color(t.color.text).max_width(ROW_W - 22.0),
    );
    ui.text(
        "The account is kept on this player, never on a USB stick.",
        CONTENT_X + 11.0,
        note_y + t.text.label.line_box() + 4.0,
        t.text.label,
    );
    if s.editing {
        keyboard(ui, s.shift, "DONE", |i| {
            Target::Settings(SettingsTarget::Key(i))
        });
    } else {
        status(ui, v);
        bottom(ui, v, overview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::tests::fixtures::{cx, deck, said};

    #[test]
    fn typing_both_fields_sets_the_account_and_forget_clears_it() {
        let d = deck(None);
        let mut fx = Vec::new();
        let mut s = SettingsState::default();
        let mut c = cx(&d, None, &mut fx);
        // USERNAME: push, type, DONE.
        s.nav(Nav::Push, &mut c);
        for ch in "dj ".chars() {
            s.type_char(ch, &mut c);
        }
        s.nav(Nav::Push, &mut c);
        assert_eq!(s.user, "dj ");
        // PASSWORD: the knob moves down; BACK while typing gives up.
        s.nav(Nav::Down, &mut c);
        s.nav(Nav::Push, &mut c);
        s.type_char('x', &mut c);
        assert_eq!(s.view(None).rows[1].1, "x▏", "shown while typed");
        s.nav(Nav::Back, &mut c);
        assert!(s.editing.is_none() && s.password.is_empty());
        s.nav(Nav::Push, &mut c);
        for ch in "secret".chars() {
            s.type_char(ch, &mut c);
        }
        s.type_char(DONE, &mut c);
        assert_eq!(s.view(None).rows[1].1, "••••••");
        // FORGET ACCOUNT.
        s.nav(Nav::Down, &mut c);
        s.nav(Nav::Push, &mut c);
        assert_eq!(s.nav(Nav::Back, &mut c), Some(ScreenKind::Main));
        let account = Credentials {
            user: "dj".into(),
            password: "secret".into(),
        };
        assert_eq!(
            said(&fx),
            [
                format!("{:?}", Effect::SetAccount(None)),
                format!("{:?}", Effect::SetAccount(Some(account))),
                format!("{:?}", Effect::SetAccount(None)),
            ]
        );
        assert_eq!(s.view(None).rows[0].1, "not set");
    }

    #[test]
    fn the_status_line_says_how_the_login_went() {
        let failed = Snapshot {
            account: true,
            status: Status::Failed("Authentication failed".into()),
            ..Snapshot::default()
        };
        assert_eq!(
            account_status(true, Some(&failed)),
            "Last login: Authentication failed"
        );
        assert!(account_status(false, Some(&failed)).starts_with("No account"));
        assert!(account_status(true, None).contains("USB"));
    }
}
