//! Narrow panes (the 2026-11-25 note under split/split.md §0: room is the
//! only limit, so five panes across leave each 320 px). A pane draws by
//! its own width, split or not: a 384 px window (both panels at their
//! strips) has one 320 px pane, as each of five across in a 1668 px one.
//! What it draws gives way by its priority order, with nothing drawn over
//! anything else and nothing cut off at an edge.

use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext};

use super::answer_tests::{SHIP, card, show};
use super::{open_at, settle, wait};
use crate::app::AppView;

/// A selector's bounds as drawn: left, top, right, bottom.
fn edges(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Option<[f32; 4]> {
    let b = VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel))?;
    let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
    Some([x, y, x + f32::from(b.size.width), y + f32::from(b.size.height)])
}

/// The parts among `sels` drawn, by their left edges: each inside
/// `within` (left and right, a half pixel's rounding), none over the next.
fn in_a_row(cx: &mut TestAppContext, w: AnyWindowHandle, within: [f32; 4], sels: &[String]) -> Vec<String> {
    let mut parts: Vec<(String, [f32; 4])> = sels.iter().filter_map(|s| edges(cx, w, s.clone()).map(|b| (s.clone(), b))).collect();
    parts.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
    for (s, b) in &parts {
        assert!(b[0] + 0.5 >= within[0] && b[2] <= within[2] + 0.5, "{s} {b:?} out of {within:?}");
    }
    for p in parts.windows(2) {
        assert!(p[0].1[2] <= p[1].1[0] + 0.5, "{} {:?} runs into {} {:?}", p[0].0, p[0].1, p[1].0, p[1].1);
    }
    parts.into_iter().map(|(s, _)| s).collect()
}

/// The demo's window with one pane `pane` px wide (both panels at their
/// strips), on its session `id`.
fn one_pane(cx: &mut TestAppContext, pane: f32, id: &str) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view) = pane_of(cx, pane, id);
    assert!(view.read_with(cx, |v, _| v.panes.cur.layout.tight), "a {pane} px pane is tight");
    (w, view)
}

/// [`one_pane`], tight or not.
fn pane_of(cx: &mut TestAppContext, pane: f32, id: &str) -> (AnyWindowHandle, Entity<AppView>) {
    let win = pane + 2. * crate::app::STRIP;
    let (w, view, _events) = open_at(cx, (win, 900.), crate::theme::Panels::default().text_size);
    let client = view.update(cx, |v, cx| {
        // The demo answers the attach; the client sends.
        let c = v.client.take();
        v.open_tab(crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.update(cx, |v, _| v.client = client);
    wait(cx, w, &view, 500);
    let frame = edges(cx, w, "composer-frame 0".into()).expect("the composer");
    assert!(frame[0] >= crate::app::STRIP && frame[2] <= win - crate::app::STRIP, "one pane between the strips: {frame:?}");
    (w, view)
}

fn at_320(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    one_pane(cx, 320., crate::demo::SID)
}

/// The composer's row of a narrow pane: the state glyph, model · thinking
/// while they fit, a hint at its least, HALT and the primary button, in
/// that order, inside the composer and none over another; what drops goes
/// in the foot's one order (the hint, the thinking level, the model), the
/// word gone as in every tight pane. At 525 px (tight, as each of three
/// across beside a panel) the model and thinking level stay: they fit.
/// The placeholder drops its key hints rather than being cut off mid-word
/// at the field's edge (the stills at 320 and 525 had "(Ent" and "new li").
#[gpui_kit::test]
fn a_320_px_panes_composer_row_gives_way_in_order(cx: &mut TestAppContext) {
    let sels: Vec<String> =
        ["foot-glyph", "choice-model 0", "choice-thinking 0", "composer-note", "halt", "foot-send"].map(String::from).to_vec();
    let has = |shown: &[String], s: &str| shown.iter().any(|x| x == s);
    let (w, view) = one_pane(cx, 525., crate::demo::SID);
    let frame = edges(cx, w, "composer-frame 0".into()).expect("the composer");
    let shown = in_a_row(cx, w, frame, &sels);
    assert!(has(&shown, "choice-model 0") && has(&shown, "choice-thinking 0"), "525: model · thinking kept: {shown:?}");
    VisualTestContext::from_window(w, cx).simulate_resize(gpui_kit::size(gpui_kit::px(384.), gpui_kit::px(900.)));
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.panes.cur.layout.column) <= 320., "320 now");
    let frame = edges(cx, w, "composer-frame 0".into()).expect("the composer");
    let shown = in_a_row(cx, w, frame, &sels);
    assert!(has(&shown, "foot-glyph") && has(&shown, "foot-send"), "{shown:?}");
    assert!(!has(&shown, "choice-thinking 0") || has(&shown, "choice-model 0"), "the thinking level goes before the model: {shown:?}");
    assert!(!has(&shown, "composer-note") || has(&shown, "choice-thinking 0"), "a hint goes before the thinking level: {shown:?}");
    assert!(edges(cx, w, "foot-word".into()).is_none(), "a tight pane's word gone");
    let said = view.read_with(cx, |v, _| v.panes.cur.placeholder_said.to_string());
    assert!(!said.is_empty() && !said.contains("(Enter"), "its key hints dropped: {said:?}");
    // Wide, judged again at the column of the frame it is drawn in (the
    // test's text system sets it 0.6 em a character, 672 px at 16: wider
    // than any column, so whole only in the app, where it is ~470).
    VisualTestContext::from_window(w, cx).simulate_resize(gpui_kit::size(gpui_kit::px(1440.), gpui_kit::px(900.)));
    settle(cx, w, 8);
    let (judged, column) = view.read_with(cx, |v, _| (v.panes.cur.placeholder_column, v.panes.cur.layout.column));
    assert_eq!(judged, column, "the placeholder judged at the column drawn");
}

/// The tab strip at 320 px: three tabs overflow to one shown between the
/// arrows, every tab drawn inside the pane.
#[gpui_kit::test]
fn a_320_px_panes_tabs_stay_in_it(cx: &mut TestAppContext) {
    let (w, view) = at_320(cx);
    for s in [crate::demo::IDLE_CPU, crate::demo::FLAKY] {
        view.update(cx, |v, cx| {
            v.open_tab(crate::app::Open::Attach { session_id: s.into(), fallback: None, open: false }, crate::tabs::Place::New);
            cx.notify();
        });
        wait(cx, w, &view, 500);
    }
    let ids: Vec<u64> = view.read_with(cx, |v, _| v.model.tabs.iter().map(|t| t.id).collect());
    assert_eq!(ids.len(), 3);
    let pane = [crate::app::STRIP, 0., 384. - crate::app::STRIP, 900.];
    let mut drawn = 0;
    for id in ids {
        if let Some(b) = edges(cx, w, format!("tab {id}")) {
            drawn += 1;
            assert!(b[0] + 0.5 >= pane[0] && b[2] <= pane[2] + 0.5, "tab {id} {b:?} in the pane {pane:?}");
            assert!(b[2] - b[0] >= crate::tabstrip::TAB_MIN - 0.5, "a tab never under its minimum: {b:?}");
        }
    }
    assert!(drawn >= 1, "a tab shown");
}

/// An agent card at 320 px: an option's hint gives way, then its label
/// ellipsizes; its key and "suggested" stay whole inside the row (the
/// still at 320 had "suggested" cut off at the card's edge).
#[gpui_kit::test]
fn a_320_px_panes_card_options_keep_their_parts_inside(cx: &mut TestAppContext) {
    let (w, view) = one_pane(cx, 320., crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    let frame = edges(cx, w, "composer-frame 0".into()).expect("the composer");
    let mut suggested = 0;
    for n in 1..=3 {
        let Some(row) = edges(cx, w, format!("ask-option {id} {n}")) else { continue };
        assert!(row[2] <= frame[2] + 0.5, "option {n}'s row {row:?} inside the column {frame:?}");
        if let Some(s) = edges(cx, w, format!("ask-suggested {id} {n}")) {
            suggested += 1;
            assert!(s[0] >= row[0] && s[2] <= row[2] + 0.5, "option {n}'s \"suggested\" {s:?} inside its row {row:?}");
        }
    }
    assert!(suggested >= 1, "a suggested option shown");
}

/// The placeholder's fit is judged again when the text size changes and
/// the column does not (a pane narrower than 42 rem at either size: its
/// column is the pane's): bigger, its key hints go rather than being cut
/// off; smaller again, they come back (Shady #95 P3). The test's text
/// system sets 0.6 em a character: a worker's 58 fit 552 px at 14 (488 of
/// 524) and not at 18 (627 of 516).
#[gpui_kit::test]
fn the_placeholders_fit_follows_the_text_size(cx: &mut TestAppContext) {
    let (w, view) = pane_of(cx, 600., crate::demo::SID);
    view.update(cx, |v, cx| {
        v.model.lives[v.panes.cur.live].watch.open(&crate::watch::Candidate::of(1, "scout"), None, std::time::Instant::now());
        cx.notify();
    });
    settle(cx, w, 8);
    let at = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, cx| (crate::theme::metrics(cx).step, v.panes.cur.layout.column, v.panes.cur.placeholder_said.to_string()))
    };
    let step = |cx: &mut TestAppContext, k: &str, n: usize| {
        for _ in 0..n {
            super::answer_tests::key(cx, w, k);
            settle(cx, w, 8);
        }
    };
    step(cx, "ctrl--", 2);
    let (s14, c14, said) = at(cx);
    assert_eq!(s14, 14);
    assert!(said.starts_with("Steer scout") && said.contains("  (Enter sends)"), "14: whole: {said:?}");
    step(cx, "ctrl-=", 4);
    let (s18, c18, said) = at(cx);
    assert_eq!((s18, c18), (18, c14), "18, the same column");
    assert!(said.starts_with("Steer scout") && !said.contains("  ("), "18: its key hints gone: {said:?}");
    step(cx, "ctrl--", 4);
    let (_, c, said) = at(cx);
    assert_eq!(c, c14);
    assert!(said.contains("  (Enter sends)"), "14 again: whole again: {said:?}");
}
