//! Input: what taps, drags and the browse controls do.

use player_api::DeckCommand;

use super::fixtures::{cx, deck, hit, key, said, stick};
use crate::app::effects::Effect;
use crate::input::Nav;
use crate::kit::Rect;
use crate::screens::*;

#[test]
fn tabs_and_the_knob_switch_screens_only_with_a_browser() {
    let d = deck(None);
    let mut fx = Vec::new();
    let mut s = Screens::default();
    let tab = hit(Target::Chrome(ChromeTarget::Browse), Rect::default());
    s.tap(tab, (0.0, 0.0), &mut cx(&d, None, &mut fx));
    assert_eq!(s.screen, ScreenKind::Main, "no stick, no browser");
    s.nav(Nav::Push, &mut cx(&d, None, &mut fx));
    assert_eq!(s.screen, ScreenKind::Main);

    let (mut b, dir) = stick("tabs");
    s.tap(tab, (0.0, 0.0), &mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(s.screen, ScreenKind::Browse);
    s.tap(tab, (0.0, 0.0), &mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(s.screen, ScreenKind::Main);
    // Pushing the knob elsewhere opens the browser; BACK at its top leaves it.
    s.nav(Nav::Push, &mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(s.screen, ScreenKind::Browse);
    s.nav(Nav::Back, &mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(s.screen, ScreenKind::Main);
    // INFO from the main screen, BACK out of it.
    s.toggle_info();
    assert_eq!(s.screen, ScreenKind::Info);
    s.nav(Nav::Back, &mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(s.screen, ScreenKind::Main);
    assert!(fx.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn call_jumps_or_resizes_the_loop_and_slip_toggles() {
    let mut d = deck(None);
    let mut fx = Vec::new();
    let mut s = Screens::default();
    for (looping, t) in [(true, DeckTarget::CallPrev), (false, DeckTarget::CallNext)] {
        d.looping = looping;
        s.tap(
            hit(Target::Deck(t), Rect::default()),
            (0.0, 0.0),
            &mut cx(&d, None, &mut fx),
        );
        s.release(&mut cx(&d, None, &mut fx));
    }
    let slip = hit(Target::Chrome(ChromeTarget::Slip), Rect::default());
    s.tap(slip, (0.0, 0.0), &mut cx(&d, None, &mut fx));
    let deck_cmds = |c: &[DeckCommand]| {
        c.iter()
            .map(|c| format!("{:?}", Effect::Deck(c.clone())))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        said(&fx),
        deck_cmds(&[
            DeckCommand::LoopResize(0.5),
            DeckCommand::BeatJump(4.0),
            DeckCommand::ToggleSlip
        ])
    );
}

#[test]
fn the_waveform_scrubs_with_the_finger_and_the_overview_seeks() {
    let d = deck(None);
    let mut fx = Vec::new();
    let mut s = Screens::default();
    let wave = main::ZOOM;
    s.tap(
        hit(Target::Deck(DeckTarget::Waveform), wave),
        (400.0, 200.0),
        &mut cx(&d, None, &mut fx),
    );
    // Dragging left moves the track forward under the finger.
    s.drag((390.0, 200.0), &mut cx(&d, None, &mut fx));
    let scrubbed = s.deck.scrubbed().unwrap();
    assert!(scrubbed > d.position, "{scrubbed}");
    s.release(&mut cx(&d, None, &mut fx));
    assert!(s.deck.scrubbed().is_none() && s.pressed.is_none());
    match fx.as_slice() {
        [
            Effect::Deck(DeckCommand::ScratchBegin),
            Effect::Deck(DeckCommand::ScratchBy(f)),
            Effect::Deck(DeckCommand::ScratchEnd),
        ] => assert!((d.position + f - scrubbed).abs() < 1e-6),
        other => panic!("{other:?}"),
    }

    // The overview: a touch seeks, and a drag keeps seeking.
    fx.clear();
    let o = deck::OVERVIEW;
    s.tap(
        hit(Target::Deck(DeckTarget::Overview), o),
        (o.x, o.y),
        &mut cx(&d, None, &mut fx),
    );
    s.drag((o.x + o.w * 0.5, o.y), &mut cx(&d, None, &mut fx));
    let seeks: Vec<f64> = fx
        .iter()
        .map(|e| match e {
            Effect::Deck(DeckCommand::Seek(at)) => *at,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(seeks, [0.0, d.length * 0.5]);
}

#[test]
fn a_row_taps_selects_then_loads_and_a_drag_scrolls_instead() {
    let d = deck(None);
    let (mut b, dir) = stick("rows");
    let mut fx = Vec::new();
    let mut s = Screens {
        screen: ScreenKind::Browse,
        ..Screens::default()
    };
    let row = |i: u8| hit(Target::Browse(BrowseTarget::Row(i)), Rect::default());
    // A drag of two rows up scrolls by two and is no tap.
    s.tap(row(3), (500.0, 250.0), &mut cx(&d, Some(&mut b), &mut fx));
    s.drag(
        (500.0, 250.0 - 2.0 * browse::ROW_H),
        &mut cx(&d, Some(&mut b), &mut fx),
    );
    assert_eq!(s.pressed, None, "the drag leaves the row");
    s.release(&mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(b.level().top, 2);
    assert!(fx.is_empty());
    // A tap selects; a tap on the selection loads it.
    for _ in 0..2 {
        s.tap(row(1), (500.0, 150.0), &mut cx(&d, Some(&mut b), &mut fx));
        s.release(&mut cx(&d, Some(&mut b), &mut fx));
    }
    assert_eq!(b.level().cursor, 3);
    assert_eq!(
        said(&fx),
        [format!("{:?}", Effect::Load(dir.join("03.mp3")))]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_question_has_the_focus_the_knob_answers_it() {
    use crate::screens::browse::Focus;
    let d = deck(None);
    let (mut b, dir) = stick("question");
    let mut fx = Vec::new();
    let mut s = Screens {
        screen: ScreenKind::Browse,
        ..Screens::default()
    };
    s.browse.ask_scan = true;
    assert_eq!(s.browse.focus(), Focus::AskScan);
    // Turning does not move the list under it; a push scans.
    s.nav(Nav::Down, &mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(b.level().cursor, 0);
    s.nav(Nav::Push, &mut cx(&d, Some(&mut b), &mut fx));
    assert!(!s.browse.ask_scan);
    assert_eq!(said(&fx), [format!("{:?}", Effect::Scan)]);
    assert_eq!(b.source(), browser::Source::Library);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_search_field_types_but_enter_and_esc_still_browse() {
    use crate::screens::browse::Focus;
    use winit::keyboard::{Key, NamedKey, SmolStr};
    let d = deck(None);
    let (mut b, dir) = stick("search");
    let mut fx = Vec::new();
    let mut s = Screens::default();
    s.search(&mut cx(&d, Some(&mut b), &mut fx));
    assert_eq!(
        (s.screen, s.browse.focus()),
        (ScreenKind::Browse, Focus::Search)
    );
    let c = key(Key::Character(SmolStr::new("c")));
    assert!(s.browse.key(&c, &mut cx(&d, Some(&mut b), &mut fx)));
    assert_eq!(b.search_query(), Some("c"), "c types; it does not cue");
    for k in [Key::Named(NamedKey::Enter), Key::Named(NamedKey::Escape)] {
        assert!(!s.browse.key(&key(k), &mut cx(&d, Some(&mut b), &mut fx)));
    }
    assert!(fx.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key on the browse screen, with SoulSeek's `net`.
fn typed(
    s: &mut Screens,
    d: &crate::app::engine::DeckSnapshot,
    b: &mut browser::Browser,
    fx: &mut Vec<Effect>,
    net: &soulseek::Snapshot,
    k: winit::keyboard::Key,
) -> bool {
    let mut c = cx(d, Some(b), fx);
    c.soulseek = Some(net);
    s.browse.key(&key(k), &mut c)
}

/// A browse control, with SoulSeek's `net`.
fn with(
    s: &mut Screens,
    d: &crate::app::engine::DeckSnapshot,
    b: &mut browser::Browser,
    fx: &mut Vec<Effect>,
    net: &soulseek::Snapshot,
    n: Nav,
) {
    let mut c = cx(d, Some(b), fx);
    c.soulseek = Some(net);
    s.nav(n, &mut c);
}

#[test]
fn soulseek_searches_on_done_and_a_picked_hit_downloads_then_loads() {
    use soulseek::{Format, Hit, Quality, Queued, Snapshot, Status, Transfer};
    use std::sync::Arc;
    use winit::keyboard::{Key, NamedKey, SmolStr};
    let d = deck(None);
    let (mut b, dir) = stick("soulseek");
    let mut fx = Vec::new();
    let mut s = Screens::default();
    let mut net = Snapshot {
        account: true,
        ..Snapshot::default()
    };
    s.search(&mut cx(&d, Some(&mut b), &mut fx));
    // What was typed for the stick comes along to SoulSeek.
    typed(
        &mut s,
        &d,
        &mut b,
        &mut fx,
        &net,
        Key::Character(SmolStr::new("a")),
    );
    let chip = |src| {
        hit(
            Target::Browse(BrowseTarget::SearchSource(src)),
            Rect::default(),
        )
    };
    s.tap(
        chip(browse::SearchSource::Soulseek),
        (0.0, 0.0),
        &mut cx(&d, Some(&mut b), &mut fx),
    );
    assert_eq!(s.browse.soulseek.query, "a");
    // Typing doesn't search; Enter (DONE) does, and the field takes it.
    typed(
        &mut s,
        &d,
        &mut b,
        &mut fx,
        &net,
        Key::Character(SmolStr::new("b")),
    );
    assert!(fx.is_empty());
    assert!(typed(
        &mut s,
        &d,
        &mut b,
        &mut fx,
        &net,
        Key::Named(NamedKey::Enter)
    ));
    assert_eq!(
        said(&fx),
        [format!("{:?}", Effect::SoulseekSearch("ab".into()))]
    );
    assert!(!s.browse.keyboard, "the keyboard goes down for the hits");
    fx.clear();

    // Hits arrive; the knob moves down one and pushes: a download.
    let hits: Vec<Hit> = ["x.mp3", "y.flac"]
        .iter()
        .map(|n| Hit {
            user: "peer".into(),
            path: format!("@@share\\{n}"),
            size: 1,
            quality: Quality::new(Format::of(n).unwrap(), &Default::default()),
            free_slot: true,
            speed: 1,
        })
        .collect();
    net.status = Status::Searching;
    net.hits = Arc::new(hits.clone());
    with(&mut s, &d, &mut b, &mut fx, &net, Nav::Down);
    with(&mut s, &d, &mut b, &mut fx, &net, Nav::Push);
    assert_eq!(
        said(&fx),
        [format!("{:?}", Effect::Download(hits[1].clone()))]
    );
    fx.clear();
    // Under way: pushing again does nothing. Downloaded: it loads.
    net.transfers = vec![Queued {
        hit: hits[1].clone(),
        transfer: Transfer::Queued,
    }];
    with(&mut s, &d, &mut b, &mut fx, &net, Nav::Push);
    assert!(fx.is_empty());
    let file = dir.join("SoulSeek Downloads/y.flac");
    net.transfers[0].transfer = Transfer::Done(file.clone());
    with(&mut s, &d, &mut b, &mut fx, &net, Nav::Push);
    assert_eq!(said(&fx), [format!("{:?}", Effect::Load(file))]);

    // BACK leaves the search; the source is kept for the next one.
    with(&mut s, &d, &mut b, &mut fx, &net, Nav::Back);
    assert!(b.search_query().is_none());
    assert_eq!(
        said(&fx).last(),
        Some(&format!("{:?}", Effect::StopSearch)),
        "and stops the network's search"
    );
    assert_eq!(s.browse.source, browse::SearchSource::Soulseek);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_stick_soulseek_says_so_and_sends_nothing() {
    use winit::keyboard::{Key, NamedKey};
    let d = deck(None);
    let (mut b, dir) = stick("soulseek-none");
    let mut fx = Vec::new();
    let mut s = Screens::default();
    s.search(&mut cx(&d, Some(&mut b), &mut fx));
    s.browse.source = browse::SearchSource::Soulseek;
    s.browse.soulseek.query = "x".into();
    s.browse.key(
        &key(Key::Named(NamedKey::Enter)),
        &mut cx(&d, Some(&mut b), &mut fx),
    );
    assert!(fx.is_empty());
    let v = s.browse.view(&b, &Default::default(), None, None);
    assert!(v.empty.unwrap().contains("USB"));
    assert!(v.header.starts_with("SOULSEEK:  x"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn menu_on_a_playback_screen_opens_the_settings_and_back_leaves_them() {
    let d = deck(None);
    let mut fx = Vec::new();
    let mut s = Screens::default();
    let menu = hit(Target::Chrome(ChromeTarget::Menu), Rect::default());
    s.tap(menu, (0.0, 0.0), &mut cx(&d, None, &mut fx));
    assert_eq!(s.screen, ScreenKind::Settings);
    s.tap(menu, (0.0, 0.0), &mut cx(&d, None, &mut fx));
    assert_eq!(s.screen, ScreenKind::Main, "MENU again closes them");
    s.toggle_perform();
    s.tap(menu, (0.0, 0.0), &mut cx(&d, None, &mut fx));
    assert_eq!(s.screen, ScreenKind::Settings, "from PERFORM too");
    // Tapping USERNAME twice types into it; BACK gives up, then leaves.
    let row = hit(Target::Settings(SettingsTarget::Row(0)), Rect::default());
    for _ in 0..2 {
        s.tap(row, (0.0, 0.0), &mut cx(&d, None, &mut fx));
    }
    assert!(s.settings.editing.is_some());
    s.nav(Nav::Back, &mut cx(&d, None, &mut fx));
    assert!(s.settings.editing.is_none());
    s.nav(Nav::Back, &mut cx(&d, None, &mut fx));
    assert_eq!(s.screen, ScreenKind::Main);
    assert!(fx.is_empty());
}

#[test]
fn the_queue_chip_opens_the_queue_where_cancel_stops_the_selected_download() {
    use soulseek::{Format, Hit, Quality, Queued, Snapshot, Transfer};
    let d = deck(None);
    let (mut b, dir) = stick("queue");
    let mut fx = Vec::new();
    let mut s = Screens {
        screen: ScreenKind::Browse,
        ..Screens::default()
    };
    let file = |name: &str, transfer| Queued {
        hit: Hit {
            user: "peer".into(),
            path: format!("@@share\\{name}"),
            size: 100,
            quality: Quality::new(Format::Mp3, &Default::default()),
            free_slot: true,
            speed: 1,
        },
        transfer,
    };
    let done = dir.join("SoulSeek Downloads/b.mp3");
    let net = Snapshot {
        account: true,
        transfers: vec![
            file(
                "a.mp3",
                Transfer::Receiving {
                    done: 10,
                    total: 100,
                },
            ),
            file("b.mp3", Transfer::Done(done.clone())),
        ],
        ..Snapshot::default()
    };
    let tap = |s: &mut Screens, b: &mut browser::Browser, fx: &mut Vec<Effect>, t| {
        let mut c = cx(&d, Some(b), fx);
        c.soulseek = Some(&net);
        s.tap(hit(Target::Browse(t), Rect::default()), (0.0, 0.0), &mut c);
        s.release(&mut c);
    };
    let v = s.browse.view(&b, &Default::default(), Some(&net), None);
    assert_eq!(v.queue_chip.as_deref(), Some("QUEUE 1 · 10 %"));
    tap(&mut s, &mut b, &mut fx, BrowseTarget::Queue);
    let v = s.browse.view(&b, &Default::default(), Some(&net), None);
    assert!(v.queue_open && v.header == "DOWNLOADS");
    assert_eq!(v.rows.len(), 2);
    assert_eq!(v.cancel, Some("CANCEL"));
    tap(&mut s, &mut b, &mut fx, BrowseTarget::Cancel);
    assert_eq!(
        said(&fx),
        [format!(
            "{:?}",
            Effect::CancelDownload(net.transfers[0].hit.key())
        )]
    );
    fx.clear();
    // The finished one: REMOVE, and a second tap on it loads it.
    tap(&mut s, &mut b, &mut fx, BrowseTarget::Row(1));
    let v = s.browse.view(&b, &Default::default(), Some(&net), None);
    assert_eq!(v.cancel, Some("REMOVE"));
    tap(&mut s, &mut b, &mut fx, BrowseTarget::Row(1));
    assert_eq!(said(&fx), [format!("{:?}", Effect::Load(done))]);
    // BACK closes the queue, not the browser.
    let mut c = cx(&d, Some(&mut b), &mut fx);
    c.soulseek = Some(&net);
    s.nav(Nav::Back, &mut c);
    assert!(s.browse.queue.is_none() && s.screen == ScreenKind::Browse);
    let _ = std::fs::remove_dir_all(&dir);
}
