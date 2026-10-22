//! The device screen's screens, laid out after the 1024×600 wireframe (canvas version 28).
//! Each is a pure function of a [`View`]: it fills a [`Scene`] and returns the touch
//! targets, so layouts can be tested and screenshotted without a device.
//!
//! - `chrome.rs`: the frame every screen shares (rail, tab row, lower bands);
//! - `deck.rs`: the deck displays the playback screens share (zoom waveform, overview, status
//!   band, phase meters, cue column);
//! - `main.rs`, `perform.rs`, `browse/`: the screens.
//!
//! The waveforms come from the deck's computed waveform (`waves.rs`), drawn between their
//! panel and grid and the overlays (loop, cue, slip shadow, playhead), as the waveform crate's
//! reference renderer orders them. Until the first entries exist, the views show the grid and
//! markers alone.

pub mod browse;
pub mod chrome;
pub mod deck;
pub mod main;
pub mod perform;

use crate::app::engine::DeckSnapshot;
use crate::kit;
use crate::render::scene::Scene;
use crate::theme::Theme;
use crate::waves::{OverviewCache, WaveSettings};

pub use perform::PerfState;

/// Things on screen that can be touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Library,
    Link,
    Usb,
    Slip,
    Browse,
    TagList,
    Info,
    Menu,
    Perform,
    Delete,
    Memory,
    CallPrev,
    CallNext,
    Zoom,
    Grid,
    Sync,
    Master,
    TempoRange,
    JumpSize,
    /// The overview: touching it seeks (needle search).
    Overview,
    /// The zoom waveform: touching it takes the track, dragging scrubs it.
    Waveform,
    // Performance screen.
    HotCueMode,
    BeatJumpMode,
    DeleteCall,
    Bank,
    /// One of the four pads (0..4, left to right).
    Pad(u8),
    /// One of the beat loop lengths ([`crate::screens::perform::BEAT_LOOPS`] index).
    BeatLoop(u8),
    // Browse screen.
    /// A shown row of the list (0 = top).
    Row(u8),
    /// The list header: up a level.
    BrowseBack,
    /// Header chips: the sort order, search, scan the stick.
    Sort,
    Search,
    Scan,
    /// ANALYZE ALL (or STOP while it runs).
    AnalyzeAll,
    /// The "scan this stick?" question's answers.
    ScanYes,
    ScanNo,
    /// A key of the on-screen keyboard ([`crate::screens::browse::KEYS`] index).
    Key(u8),
    /// A row of a modal over the browse list (its index among all the modal's rows).
    ModalRow(u16),
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
}

/// A touch target on the device screen.
pub type HitBox = kit::HitBox<Target>;

/// Everything the screen shows: the deck as of this frame, and the UI's own state.
pub struct View<'a> {
    pub deck: &'a DeckSnapshot,
    pub title: &'a str,
    /// The playhead the screen shows (frames): smoothed between engine updates, or where the
    /// finger has moved the track.
    pub position: f64,
    /// Beats across the zoom view.
    pub zoom_beats: f64,
    pub pressed: Option<Target>,
    /// Frame timing line, if the overlay is on.
    pub debug: Option<&'a str>,
    /// The track's waveform so far, if any.
    pub wave: Option<Wave<'a>>,
    pub screen: ScreenKind,
    pub perf: PerfState,
    /// Whether the library is saving edits; `None` until it says.
    pub save: Option<SaveState>,
    /// The browse screen's list, when there is something to browse.
    pub browse: Option<&'a crate::screens::browse::BrowseView>,
}

/// What the status band says about the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    /// Every edit is on the stick.
    Saved,
    /// Written, flushed within a second or two.
    Saving,
    /// Edits are in RAM only and lost at power-off (no library, a locked or failing stick).
    Unsaved,
}

impl SaveState {
    fn label(self) -> &'static str {
        match self {
            SaveState::Saved => "SAVED",
            SaveState::Saving => "SAVING",
            SaveState::Unsaved => "UNSAVED",
        }
    }
}

/// A deck's waveform as the screen draws it.
#[derive(Clone, Copy)]
pub struct Wave<'a> {
    pub entries: &'a [waveform::compute::Entry],
    pub entries_per_second: f64,
    pub settings: WaveSettings,
    /// The theme's waveform style with this track's gain set.
    pub style: waveform::display::Style,
}

/// The toolkit, drawing for the device screen's targets.
pub(crate) type Ui<'s> = kit::Ui<'s, Target>;

/// Lay out the screen into `scene`; returns the touch targets.
pub fn build(
    view: &View,
    theme: &Theme,
    overview: &mut OverviewCache,
    scene: &mut Scene,
) -> Vec<HitBox> {
    scene.clear();
    scene.background = theme.color.background;
    let mut ui = Ui {
        t: theme,
        scene,
        hits: Vec::new(),
        pressed: view.pressed,
    };

    if view.screen == ScreenKind::Performance {
        perform::build(&mut ui, view, overview);
        debug_line(&mut ui, view);
        return ui.hits;
    }
    if matches!(view.screen, ScreenKind::Browse | ScreenKind::Info)
        && let Some(b) = view.browse
    {
        // No timing overlay here: it would cover the list.
        browse::build(&mut ui, view, b, overview);
        return ui.hits;
    }
    main::build(&mut ui, view, overview);
    debug_line(&mut ui, view);
    ui.hits
}

/// The frame timing overlay, if on.
fn debug_line(ui: &mut Ui, view: &View) {
    let t = ui.t;
    if let Some(line) = view.debug.filter(|l| !l.is_empty()) {
        ui.scene.rounded(
            113.0,
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
mod tests {
    use super::*;
    use crate::screens::perform::PadMode;
    use engine_core::DeckGrid;

    /// A deck 300 s long, paused at about 21 s in a loop, with hot cues A and C.
    fn deck(grid: Option<DeckGrid>) -> DeckSnapshot {
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

    fn view(deck: &DeckSnapshot) -> View<'_> {
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
        }
    }

    #[test]
    fn browse_screen_fits_and_targets_do_not_overlap() {
        use crate::screens::browse::{
            BrowseView, InfoView, MODAL_ROWS, ModalView, ROWS, Row, RowKind,
        };
        let rows: Vec<Row> = (0..ROWS)
            .map(|i| Row {
                kind: match i {
                    0 => RowKind::Folder,
                    1 => RowKind::List,
                    _ => RowKind::Track,
                },
                tagged: i == 4,
                missing: i == 5,
                title: "A title far too long for its column ".repeat(4),
                artist: "Artist".into(),
                detail: "128.0   8A   5:12".into(),
                loaded: i == 3,
                art: None,
            })
            .collect();
        let base = BrowseView {
            header: "DJ  /  UKG".into(),
            at_root: false,
            sort: "TITLE",
            rows,
            selected: Some(3),
            first: 10,
            total: 40,
            has_tracks: true,
            ..BrowseView::default()
        };
        let variants = [
            base.clone(),
            BrowseView {
                library: true,
                can_scan: true,
                ..base.clone()
            },
            BrowseView {
                library: true,
                can_scan: true,
                scan: Some((120, 586)),
                ..base.clone()
            },
            BrowseView {
                library: true,
                can_scan: true,
                unanalysed: 12,
                ..base.clone()
            },
            BrowseView {
                library: true,
                can_scan: true,
                unanalysed: 12,
                analyzing: Some((3, 12, false)),
                ..base.clone()
            },
            BrowseView {
                searching: true,
                keyboard: true,
                ..base.clone()
            },
            BrowseView {
                has_tracks: false,
                ..base.clone()
            },
            BrowseView {
                tag_list: true,
                modal: Some(ModalView {
                    title: "MENU".into(),
                    rows: (0..7).map(|i| format!("ACTION {i}")).collect(),
                    selected: 6,
                    danger_first: false,
                }),
                ..base.clone()
            },
            BrowseView {
                modal: Some(ModalView {
                    title: "DELETE PEAK?".into(),
                    rows: vec!["YES".into(), "CANCEL".into()],
                    selected: 1,
                    danger_first: true,
                }),
                ..base.clone()
            },
            BrowseView {
                keyboard: true,
                header_override: Some("NEW PLAYLIST:  PEAK▏".into()),
                ..base.clone()
            },
            BrowseView {
                ask_scan: true,
                at_root: true,
                ..base.clone()
            },
            BrowseView {
                side: Some(InfoView {
                    source: "USB",
                    fields: vec![
                        ("TITLE", "A title far too long for its column ".repeat(4)),
                        ("ARTIST", "Artist".into()),
                        ("GENRE", "uk garage".into()),
                    ],
                    rating: Some(3),
                    comment: Some("word ".repeat(80)),
                    ..InfoView::default()
                }),
                ..base.clone()
            },
            BrowseView {
                library: true,
                info: Some(InfoView {
                    source: "USB",
                    position: "TRACK 004/040".into(),
                    fields: vec![
                        ("TITLE", "A title far too long for its column ".repeat(4)),
                        ("ARTIST", "Artist".into()),
                        ("ALBUM", "Album".into()),
                        ("TIME", "5:12".into()),
                        ("BPM", "128.0".into()),
                        ("KEY", "8A".into()),
                    ],
                    rating: Some(3),
                    comment: Some("word ".repeat(80)),
                    ..InfoView::default()
                }),
                ..base.clone()
            },
        ];
        for b in &variants {
            let d = deck(None);
            let mut v = view(&d);
            v.screen = ScreenKind::Browse;
            v.browse = Some(b);
            let mut scene = Scene::default();
            let hits = build(
                &v,
                &Theme::default(),
                &mut OverviewCache::default(),
                &mut scene,
            );
            for r in &scene.rects {
                assert!(
                    r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
                    "{r:?}"
                );
            }
            for (i, a) in hits.iter().enumerate() {
                for c in &hits[i + 1..] {
                    let overlap = a.rect.overlaps(c.rect);
                    assert!(!overlap, "{:?} overlaps {:?}", a.target, c.target);
                }
            }
            for h in hits.iter().filter(|h| h.target != Target::Overview) {
                assert!(
                    h.rect.w >= 36.0 && h.rect.h >= 36.0,
                    "{:?} is {}×{}",
                    h.target,
                    h.rect.w,
                    h.rect.h
                );
            }
            let has = |t: Target| hits.iter().any(|h| h.target == t);
            let rows = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Row(_)))
                .count();
            let listing = b.info.is_none();
            let covered = b.ask_scan || b.modal.is_some() || !listing;
            assert_eq!(rows, if covered { 0 } else { ROWS });
            if let Some(m) = &b.modal {
                // Inside the list area, clear of the status band.
                for h in hits
                    .iter()
                    .filter(|h| matches!(h.target, Target::ModalRow(_)))
                {
                    assert!(h.rect.bottom() <= 386.0, "{h:?}");
                }
                let shown = hits
                    .iter()
                    .filter(|h| matches!(h.target, Target::ModalRow(_)));
                assert_eq!(shown.count(), m.rows.len().min(MODAL_ROWS));
                // Scrolled to keep the cursor in view.
                let sel = Target::ModalRow(m.selected as u16);
                assert!(hits.iter().any(|h| h.target == sel));
            }
            if let Some(h) = &b.header_override {
                assert!(scene.texts.iter().any(|t| &t.text == h));
            }
            assert_eq!(has(Target::BrowseBack), !b.at_root && listing);
            assert_eq!(has(Target::Scan), b.library && b.scan.is_none() && listing);
            assert_eq!(has(Target::ScanYes), b.ask_scan);
            let analyze = listing
                && b.library
                && b.scan.is_none()
                && (b.unanalysed > 0 || b.analyzing.is_some());
            assert_eq!(has(Target::AnalyzeAll), analyze);
            if b.analyzing.is_some() {
                assert!(scene.texts.iter().any(|t| t.text == "ANALYZING 3 / 12"));
            }
            let keys = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Key(_)))
                .count();
            let all_keys = crate::screens::browse::key_count();
            assert_eq!(keys, if b.keyboard { all_keys } else { 0 });
            assert_eq!(
                has(Target::Overview),
                !b.keyboard,
                "the keyboard covers the overview"
            );
            let tab = if b.tag_list {
                Target::TagList
            } else {
                Target::Browse
            };
            assert!(hits.iter().any(|h| h.target == tab));
            // The title column's divider: only with tracks (or beside the info panel).
            let divider = scene
                .rects
                .iter()
                .any(|r| r.x == 559.0 && r.y == 103.0 && r.w == 1.0);
            if b.info.is_none() && !b.ask_scan {
                assert_eq!(divider, b.has_tracks || b.side.is_some());
            }
            if b.side.is_some() {
                // The list keeps its jacket and title columns; the panel is beside it.
                for h in hits.iter().filter(|h| matches!(h.target, Target::Row(_))) {
                    assert!(h.rect.right() <= 560.0, "{h:?}");
                }
                assert!(!scene.texts.iter().any(|t| t.text == "128.0   8A   5:12"));
                assert!(scene.texts.iter().any(|t| t.text == "★★★☆☆"));
                assert!(scene.texts.iter().any(|t| t.text == "uk garage"));
            }
            if let Some(info) = &b.info {
                let stars = scene.texts.iter().any(|t| t.text == "★★★☆☆");
                assert!(stars, "the rating shows as stars");
                let title = &info.fields[0].1;
                assert!(
                    scene
                        .texts
                        .iter()
                        .any(|t| &t.text == title && t.max_width.is_some())
                );
            }
            // Row text is ellipsized to its column.
            assert!(
                scene
                    .texts
                    .iter()
                    .filter(|t| t.text.starts_with("A title"))
                    .all(|t| t.max_width.is_some())
            );
        }
    }

    #[test]
    fn everything_stays_on_the_panel_and_targets_do_not_overlap() {
        let d = deck(Some(
            DeckGrid::constant(1_000.0, 123.0, 48_000.0 * 300.0, 48_000).unwrap(),
        ));
        let mut scene = Scene::default();
        let hits = build(
            &view(&d),
            &Theme::default(),
            &mut OverviewCache::default(),
            &mut scene,
        );
        for r in &scene.rects {
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
                "{r:?}"
            );
        }
        for (i, a) in hits.iter().enumerate() {
            for b in &hits[i + 1..] {
                let overlap = a.rect.overlaps(b.rect);
                assert!(!overlap, "{:?} overlaps {:?}", a.target, b.target);
            }
        }
        // Touch targets the design pass sized for a 7" panel: at least 36 px on each side.
        for h in hits.iter().filter(|h| h.target != Target::Overview) {
            assert!(
                h.rect.w >= 36.0 && h.rect.h >= 36.0,
                "{:?} is {}×{}",
                h.target,
                h.rect.w,
                h.rect.h
            );
        }
    }

    #[test]
    fn performance_screen_fits_and_targets_do_not_overlap() {
        let mut d = deck(Some(
            DeckGrid::constant(1_000.0, 123.0, 48_000.0 * 300.0, 48_000).unwrap(),
        ));
        d.hot_cue_positions[0] = 1_010_000.0;
        let variants = [
            PerfState::default(),
            PerfState {
                mode: PadMode::BeatJump,
                jump_bank: true,
                ..PerfState::default()
            },
            PerfState {
                delete_armed: true,
                bank: 1,
                ..PerfState::default()
            },
        ];
        for perf in variants {
            let mut v = view(&d);
            v.screen = ScreenKind::Performance;
            v.perf = perf;
            let mut scene = Scene::default();
            let hits = build(
                &v,
                &Theme::default(),
                &mut OverviewCache::default(),
                &mut scene,
            );
            for r in &scene.rects {
                assert!(
                    r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.5 && r.y + r.h <= 600.0,
                    "{perf:?}: {r:?}"
                );
            }
            for (i, a) in hits.iter().enumerate() {
                for b in &hits[i + 1..] {
                    let overlap = a.rect.overlaps(b.rect);
                    assert!(!overlap, "{perf:?}: {:?} overlaps {:?}", a.target, b.target);
                }
            }
            for h in hits.iter().filter(|h| h.target != Target::Overview) {
                assert!(
                    h.rect.w >= 36.0 && h.rect.h >= 36.0,
                    "{:?} is {}×{}",
                    h.target,
                    h.rect.w,
                    h.rect.h
                );
            }
            let pads = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Pad(_)))
                .count();
            assert_eq!(pads, crate::screens::perform::PADS, "{perf:?}");
            let loops = hits
                .iter()
                .filter(|h| matches!(h.target, Target::BeatLoop(_)))
                .count();
            assert_eq!(loops, engine_core::BEAT_LOOPS.len());
        }
    }

    #[test]
    fn builds_without_a_grid() {
        let mut scene = Scene::default();
        build(
            &view(&deck(None)),
            &Theme::default(),
            &mut OverviewCache::default(),
            &mut scene,
        );
        assert!(scene.texts.iter().any(|t| t.text == "--.-"));
    }
}
