//! Input: what taps, drags and the browse controls do.

use engine_core::DeckCommand;

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
    assert_eq!(b.source(), engined::browse::Source::Library);
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
