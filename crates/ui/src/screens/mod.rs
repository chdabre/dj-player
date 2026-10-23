//! The device screen's screens, laid out after the 1024×600 wireframe (canvas version 28).
//! Each is a pure function of a [`View`]: it fills a [`Scene`] and returns the touch
//! targets, so layouts can be tested and screenshotted without a device.
//!
//! - `chrome.rs`: the frame every screen shares (rail, tab row, lower bands);
//! - `deck.rs`: the deck displays the playback screens share (zoom waveform, overview, status
//!   band, phase meters, cue column);
//! - `main.rs`, `perform.rs`, `browse/`, `info.rs`, `settings.rs`: the screens.
//!
//! The waveforms come from the deck's computed waveform (`waves.rs`), drawn between their
//! panel and grid and the overlays (loop, cue, slip shadow, playhead), as the waveform crate's
//! reference renderer orders them. Until the first entries exist, the views show the grid and
//! markers alone.

pub mod browse;
pub mod chrome;
pub mod deck;
pub mod info;
pub mod main;
pub mod perform;
pub mod router;
pub mod settings;
pub mod view;

use crate::kit;
use crate::render::scene::Scene;
use crate::theme::Theme;
use crate::waves::OverviewCache;

pub use browse::{BrowseState, BrowseTarget};
pub use chrome::ChromeTarget;
pub use deck::{DeckDisplay, DeckTarget};
pub use perform::{PerfState, PerfTarget};
pub use router::{Cx, Screens, Show};
pub use settings::{SettingsState, SettingsTarget};
pub use view::{SaveState, View, Wave};

/// Things on screen that can be touched, by the part of the screen that owns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The rail and the tab row.
    Chrome(ChromeTarget),
    /// The deck displays of the playback screens.
    Deck(DeckTarget),
    Perform(PerfTarget),
    Browse(BrowseTarget),
    Settings(SettingsTarget),
}

/// Which screen is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenKind {
    #[default]
    Main,
    Performance,
    Browse,
    /// INFO: the loaded track's details.
    Info,
    /// SETTINGS (MENU from a playback screen).
    Settings,
}

/// A touch target on the device screen.
pub type HitBox = kit::HitBox<Target>;
/// The device screen's touch targets.
pub type Hits = kit::Hits<Target>;

/// The toolkit, drawing for the device screen's targets.
pub(crate) type Ui<'s> = kit::Ui<'s, Target>;

/// Lay out the screen into `scene`; returns the touch targets.
pub fn build(view: &View, theme: &Theme, overview: &mut OverviewCache, scene: &mut Scene) -> Hits {
    scene.clear();
    scene.background = theme.color.background;
    let mut ui = Ui::new(theme, scene, view.pressed);

    if view.screen == ScreenKind::Performance {
        perform::build(&mut ui, view, overview);
        debug_line(&mut ui, view);
        return ui.into_hits();
    }
    if view.screen == ScreenKind::Info
        && let Some(i) = view.info
    {
        info::build(&mut ui, view, i, overview);
        return ui.into_hits();
    }
    if view.screen == ScreenKind::Settings
        && let Some(s) = view.settings
    {
        settings::build(&mut ui, view, s, overview);
        return ui.into_hits();
    }
    if view.screen == ScreenKind::Browse
        && let Some(b) = view.browse
    {
        // No timing overlay here: it would cover the list.
        browse::build(&mut ui, view, b, overview);
        return ui.into_hits();
    }
    main::build(&mut ui, view, overview);
    debug_line(&mut ui, view);
    ui.into_hits()
}

/// The frame timing overlay, if on.
fn debug_line(ui: &mut Ui, view: &View) {
    let t = ui.t;
    if let Some(line) = view.debug.filter(|l| !l.is_empty()) {
        ui.scene.rounded(
            chrome::CONTENT_X,
            176.0,
            420.0,
            22.0,
            t.shape.chip_radius,
            t.color.overlay,
            0.0,
            t.color.background,
        );
        ui.text(line, 119.0, 178.0, t.text.label.color(t.color.cue_text));
    }
}
#[cfg(test)]
mod tests;
