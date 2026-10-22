//! Layout: every screen fits the panel, and its touch targets don't overlap.

use engine_core::DeckGrid;

use super::fixtures::{deck, view};
use crate::render::scene::Scene;
use crate::screens::perform::PadMode;
use crate::screens::*;
use crate::theme::Theme;
use crate::waves::OverviewCache;

#[test]
fn info_screen_fits_and_shows_the_rating_and_the_whole_title() {
    use crate::screens::browse::InfoView;
    let info = InfoView {
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
    };
    let d = deck(None);
    let mut v = view(&d);
    v.screen = ScreenKind::Info;
    v.info = Some(&info);
    let mut scene = Scene::default();
    let hits = build(
        &v,
        &Theme::default(),
        &mut OverviewCache::default(),
        &mut scene,
    );
    for r in scene.rects() {
        assert!(
            r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
            "{r:?}"
        );
    }
    assert!(
        hits.iter()
            .any(|h| h.target == Target::Chrome(ChromeTarget::Info))
    );
    assert!(
        scene.texts().any(|t| t.text == "★★★☆☆"),
        "the rating shows as stars"
    );
    let title = &info.fields[0].1;
    assert!(
        scene
            .texts()
            .any(|t| &t.text == title && t.max_width.is_some())
    );
}

#[test]
fn browse_screen_fits_and_targets_do_not_overlap() {
    use crate::screens::browse::{BrowseView, InfoView, MODAL_ROWS, ModalView, ROWS, Row, RowKind};
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
        for r in scene.rects() {
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1024.0 + 0.5 && r.y + r.h <= 600.0,
                "{r:?}"
            );
        }
        // What can be touched never overlaps (a modal's scrim blocks what is under it).
        let reachable: Vec<_> = hits.iter().filter(|h| !hits.blocked(h)).collect();
        for (i, a) in reachable.iter().enumerate() {
            for c in &reachable[i + 1..] {
                let overlap = a.rect.overlaps(c.rect);
                assert!(!overlap, "{:?} overlaps {:?}", a.target, c.target);
            }
        }
        for h in hits
            .iter()
            .filter(|h| h.target != Target::Deck(DeckTarget::Overview))
        {
            assert!(
                h.rect.w >= 36.0 && h.rect.h >= 36.0,
                "{:?} is {}×{}",
                h.target,
                h.rect.w,
                h.rect.h
            );
        }
        let has = |t: Target| hits.iter().any(|h| h.target == t);
        let rows: Vec<_> = hits
            .iter()
            .filter(|h| matches!(h.target, Target::Browse(BrowseTarget::Row(_))))
            .collect();
        assert_eq!(rows.len(), ROWS);
        // Under a modal or the question the rows stay, dimmed, and take no touch.
        let covered = b.ask_scan || b.modal.is_some();
        assert!(rows.iter().all(|h| hits.blocked(h) == covered));
        if covered {
            let mid = (500.0, 120.0);
            let found = hits.find(mid.0, mid.1).map(|h| h.target);
            assert!(
                !matches!(found, Some(Target::Browse(BrowseTarget::Row(_)))),
                "{found:?}"
            );
        }
        if let Some(m) = &b.modal {
            // Inside the list area, clear of the status band.
            for h in hits
                .iter()
                .filter(|h| matches!(h.target, Target::Browse(BrowseTarget::ModalRow(_))))
            {
                assert!(h.rect.bottom() <= 386.0, "{h:?}");
            }
            let shown = hits
                .iter()
                .filter(|h| matches!(h.target, Target::Browse(BrowseTarget::ModalRow(_))));
            assert_eq!(shown.count(), m.rows.len().min(MODAL_ROWS));
            // Scrolled to keep the cursor in view.
            let sel = Target::Browse(BrowseTarget::ModalRow(m.selected as u16));
            assert!(hits.iter().any(|h| h.target == sel));
        }
        if let Some(h) = &b.header_override {
            assert!(scene.texts().any(|t| &t.text == h));
        }
        assert_eq!(has(Target::Browse(BrowseTarget::Back)), !b.at_root);
        assert_eq!(
            has(Target::Browse(BrowseTarget::Scan)),
            b.library && b.scan.is_none()
        );
        assert_eq!(has(Target::Browse(BrowseTarget::ScanYes)), b.ask_scan);
        let analyze = b.library && b.scan.is_none() && (b.unanalysed > 0 || b.analyzing.is_some());
        assert_eq!(has(Target::Browse(BrowseTarget::AnalyzeAll)), analyze);
        if b.analyzing.is_some() {
            assert!(scene.texts().any(|t| t.text == "ANALYZING 3 / 12"));
        }
        let keys = hits
            .iter()
            .filter(|h| matches!(h.target, Target::Browse(BrowseTarget::Key(_))))
            .count();
        let all_keys = crate::screens::browse::key_count();
        assert_eq!(keys, if b.keyboard { all_keys } else { 0 });
        assert_eq!(
            has(Target::Deck(DeckTarget::Overview)),
            !b.keyboard,
            "the keyboard covers the overview"
        );
        let tab = if b.tag_list {
            Target::Chrome(ChromeTarget::TagList)
        } else {
            Target::Chrome(ChromeTarget::Browse)
        };
        assert!(hits.iter().any(|h| h.target == tab));
        // The title column's divider: only with tracks (or beside the info panel).
        let divider = scene
            .rects()
            .any(|r| r.x == 559.0 && r.y == 103.0 && r.w == 1.0);
        if !b.ask_scan {
            assert_eq!(divider, b.has_tracks || b.side.is_some());
        }
        if b.side.is_some() {
            // The list keeps its jacket and title columns; the panel is beside it.
            for h in hits
                .iter()
                .filter(|h| matches!(h.target, Target::Browse(BrowseTarget::Row(_))))
            {
                assert!(h.rect.right() <= 560.0, "{h:?}");
            }
            assert!(!scene.texts().any(|t| t.text == "128.0   8A   5:12"));
            assert!(scene.texts().any(|t| t.text == "★★★☆☆"));
            assert!(scene.texts().any(|t| t.text == "uk garage"));
        }
        // Row text is ellipsized to its column.
        assert!(
            scene
                .texts()
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
    for r in scene.rects() {
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
    for h in hits
        .iter()
        .filter(|h| h.target != Target::Deck(DeckTarget::Overview))
    {
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
        for r in scene.rects() {
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
        for h in hits
            .iter()
            .filter(|h| h.target != Target::Deck(DeckTarget::Overview))
        {
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
            .filter(|h| matches!(h.target, Target::Perform(PerfTarget::Pad(_))))
            .count();
        assert_eq!(pads, crate::screens::perform::PADS, "{perf:?}");
        let loops = hits
            .iter()
            .filter(|h| matches!(h.target, Target::Perform(PerfTarget::BeatLoop(_))))
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
    assert!(scene.texts().any(|t| t.text == "--.-"));
}
