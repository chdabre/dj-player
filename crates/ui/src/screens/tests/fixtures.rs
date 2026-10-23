//! What the screens' tests share: a deck, a view of it, a handler's context, a stick.

use browser::Browser;
use player_api::DeckGrid;

use crate::app::effects::{Effect, Effects};
use crate::app::engine::{DeckSnapshot, LibraryView};
use crate::kit::Rect;
use crate::screens::{Cx, HitBox, PerfState, SaveState, ScreenKind, Target, View, browse};

/// A deck 300 s long, paused at about 21 s in a loop, with hot cues A and C.
pub fn deck(grid: Option<DeckGrid>) -> DeckSnapshot {
    DeckSnapshot {
        key: Some("Em".into()),
        position: 1_000_000.0,
        length: 48_000.0 * 300.0,
        loaded_fraction: 1.0,
        keylock: true,
        quantize: 1.0,
        cue: 900_000.0,
        hot_cues: 0b101,
        looping: true,
        loop_start: 990_000.0,
        loop_end: 1_090_000.0,
        grid: grid.map(std::sync::Arc::new),
        ..DeckSnapshot::empty(48_000.0)
    }
}

pub fn view(deck: &DeckSnapshot) -> View<'_> {
    View {
        deck,
        title: "Test",
        position: deck.position,
        zoom_beats: 16.0,
        pressed: None,
        debug: None,
        wave: None,
        screen: ScreenKind::Main,
        perf: PerfState::default(),
        save: Some(SaveState::Unsaved),
        browse: None,
        info: None,
    }
}

pub fn cx<'a>(
    deck: &'a DeckSnapshot,
    browser: Option<&'a mut Browser>,
    fx: &'a mut Effects,
) -> Cx<'a> {
    Cx {
        deck,
        browser,
        library: LibraryView::default(),
        soulseek: None,
        loaded: None,
        shown: None,
        effects: fx,
    }
}

pub fn hit(target: Target, rect: Rect) -> HitBox {
    HitBox {
        target,
        rect,
        layer: crate::render::scene::Layer::Base,
    }
}

pub fn said(fx: &[Effect]) -> Vec<String> {
    fx.iter().map(|e| format!("{e:?}")).collect()
}

/// A stick with ten tracks in its root, in a folder named after the test.
pub fn stick(name: &str) -> (Browser, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("ui-screens-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for i in 0..10 {
        std::fs::write(d.join(format!("{i:02}.mp3")), b"").unwrap();
    }
    (Browser::new(d.clone(), browse::ROWS), d)
}

pub fn key(k: winit::keyboard::Key) -> crate::app::keymap::KeyPress {
    crate::app::keymap::KeyPress {
        key: k,
        code: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyA),
        pressed: true,
        repeat: false,
    }
}
