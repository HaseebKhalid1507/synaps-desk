//! D3's tests (synaps-cards `docs/design.md` §6.1, §7's F10, §9.3):
//! the presence model and its counters, live against last seen, NEEDS YOU
//! and ASKED, and in the window the tab dot,
//! Ctrl+Shift+J (and an IME's composition), the palette's WAITING, and no
//! idle frames. F11 (a session only in the rail) stays the known gap
//! (§8 #1): nothing here reads a session the window doesn't hold.
//!
//! The window's helpers are `test_kit`'s (`ui_tests`' own), copied: that
//! kit is protected and its children are the files it lists.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, EntityInputHandler, Focusable, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
};
use serde_json::{Value, json};
use synaps_client::protocol::{SessionMeta, decode_event};
use synaps_client::{Event, Transcript};

use super::*;
// `super`'s `gpui_kit::*` brings the kit's `test` too: the std one here.
use crate::app::AppView;
use crate::demo::{ASK_FORM, ASK_OPEN, ASKS};
use crate::model::{ConnState, Model};
use crate::rail::State as Rail;
use core::prelude::v1::test;

// ── the model ─────────────────────────────────────────────────────────

fn cid(tool: &str) -> CardId {
    CardId::parse(&format!("c{:0>6}", tool)).expect("a card id")
}

fn card(session: &str, tool: &str, wait: bool, at: Option<i64>, live: bool) -> Open {
    Open { session: session.into(), tool_id: tool.into(), card: cid(tool), wait, question: format!("{tool}?"), at, live }
}

fn ids(cards: impl IntoIterator<Item = impl std::ops::Deref<Target = Open>>) -> Vec<String> {
    cards.into_iter().map(|c| c.tool_id.clone()).collect()
}

/// The presence counters: per session, wait and asked apart, live only;
/// as last seen apart again. The window's cards oldest first (a row
/// without a time after those with one); a tie (the same millisecond)
/// broken by session and call, never by which pane or tab a card was read
/// from (Shady #83 P3-1). Ctrl+Shift+J's and WAITING's order: live wait
/// cards, live open ones, then those as last seen. A change bumps the
/// revision, the same cards don't.
#[test]
fn presence_counts_and_orders_oldest_first() {
    let mut p = Presence::default();
    assert_eq!(p.next(), None);
    assert_eq!(p.of("a"), Count::default());
    let cards = vec![
        card("b", "t4", false, None, true),
        card("a", "t2", true, Some(300), true),
        card("b", "t3", true, Some(200), true),
        card("a", "t1", false, Some(100), true),
        card("c", "t5", true, Some(50), false),
    ];
    assert!(p.set(cards.clone()));
    let g = p.rev();
    assert_eq!(ids(p.cards()), ["t5", "t1", "t3", "t2", "t4"], "oldest first, no time last");
    assert_eq!(p.of("a"), Count { wait: 1, asked: 1 });
    assert_eq!(p.of("b"), Count { wait: 1, asked: 1 });
    assert_eq!(p.of("c"), Count::default(), "as last seen: not NEEDS YOU");
    assert_eq!(p.last_seen("c"), Count { wait: 1, asked: 0 });
    assert_eq!(p.last_seen("a"), Count::default());
    assert_eq!(p.next().map(|c| c.tool_id.as_str()), Some("t3"), "the oldest live wait card, though older ones are open or last seen");
    assert_eq!(ids(p.listed()), ["t3", "t2", "t1", "t4", "t5"], "live wait, live open, then as last seen");
    assert!(!p.set(cards.iter().rev().cloned().collect()), "the same cards, in any order: no change");
    assert_eq!(p.rev(), g);
    assert!(p.set(vec![card("a", "t1", false, Some(100), true)]));
    assert_eq!(p.next().map(|c| c.tool_id.as_str()), Some("t1"), "no wait card: the oldest open one");
    assert_eq!(p.of("b"), Count::default());
    assert!(p.rev() > g);
    // Ties: the same millisecond in two sessions, read live or as last
    // seen, from any pane, in any order: one order.
    let tie = |a_live: bool, flip: bool| {
        let mut v = vec![
            card("s2", "x1", true, Some(7_000), a_live),
            card("s1", "x2", true, Some(7_000), !a_live),
            card("s1", "x0", true, None, true),
        ];
        if flip {
            v.reverse();
        }
        let mut p = Presence::default();
        p.set(v);
        ids(p.cards())
    };
    for (live, flip) in [(true, false), (true, true), (false, false), (false, true)] {
        assert_eq!(tie(live, flip), ["x2", "x1", "x0"], "tied: by session, then call; no time last");
    }
    assert!(p.set(Vec::new()));
    assert_eq!(p.next(), None);
}

/// A row's time is kept to the millisecond (a tie within a second is no
/// tie).
#[test]
fn row_times_keep_their_milliseconds() {
    assert_eq!(at_ms("2026-11-24T10:15:00Z"), Some(1_795_515_300_000));
    assert_eq!(at_ms("2026-11-24T10:15:00.25Z"), Some(1_795_515_300_250));
    assert_eq!(at_ms("2026-11-24T10:15:00.123456+00:00"), Some(1_795_515_300_123));
    assert_eq!(at_ms("not a time"), None);
}

/// The demo's attach of `id`, folded as a client would.
fn transcript(id: &str) -> Transcript {
    let mut t = Transcript::new();
    for ev in crate::demo::attach(id, 1) {
        if let Event::Attached { attached, .. } = ev {
            t.load_attached(&attached).expect("the demo's history loads");
        }
    }
    t
}

/// A session's open cards are its fold's, as its rows draw it: bound, not
/// answered, not closed or replaced; each with its binding, question, mode
/// and row time. The ones this desk answered (on their way or in) are not.
#[test]
fn open_cards_are_the_folds_less_what_this_desk_answered() {
    let t = transcript(ASKS);
    let open = open_cards(ASKS, &t, true, |_| false);
    let got: Vec<(&str, &str, bool, &str)> =
        open.iter().map(|c| (c.tool_id.as_str(), c.card.as_str(), c.wait, c.question.as_str())).collect();
    assert_eq!(
        got,
        [
            ("toolu_a7", "c5h8k2v", false, "Which suites should pass before the cut-over?"),
            ("toolu_a8", "c0p6r4n", true, "Run the migration on staging now?")
        ],
        "of seven cards: answered, changed, skipped, closed and replaced are not open"
    );
    assert!(open.iter().all(|c| c.at.is_some() && c.live && c.session == ASKS));
    assert!(open[0].at < open[1].at, "its row's time");
    let less = open_cards(ASKS, &t, true, |c| c.tool_id == "toolu_a8");
    assert_eq!(ids(&less), ["toolu_a7"], "answered here: not waiting");
    assert_eq!(open_cards(ASK_OPEN, &transcript(ASK_OPEN), false, |_| false).len(), 1);
    assert!(open_cards(crate::demo::SID, &transcript(crate::demo::SID), true, |_| false).is_empty(), "no cards, none open");
}

#[test]
fn a_waiting_card_is_needs_you_and_an_open_one_asked() {
    let mut m = Model::new(ConnState::Connecting);
    let metas: Vec<SessionMeta> = serde_json::from_value(json!([
        {"id": "w", "lifecycle": "live"},
        {"id": "o", "lifecycle": "live"},
        {"id": "r", "lifecycle": "live", "busy": true},
        {"id": "p", "lifecycle": "live", "awaiting_input": 1},
        {"id": "n", "lifecycle": "live"},
        {"id": "l", "lifecycle": "live"},
    ]))
    .unwrap();
    m.apply(Event::Sessions(metas));
    let st = |m: &Model, id: &str| m.state(m.sessions.iter().find(|s| s.id == id).unwrap());
    assert_eq!(st(&m, "w"), Rail::Idle);
    m.presence.set(vec![
        card("w", "t1", true, Some(1), true),
        card("o", "t2", false, Some(2), true),
        card("r", "t3", false, Some(3), true),
        card("p", "t4", false, Some(4), true),
        card("l", "t5", true, Some(5), false),
        card("l", "t6", false, Some(6), false),
    ]);
    assert_eq!(
        ["w", "o", "r", "p", "n", "l"].map(|id| st(&m, id)),
        [Rail::NeedsYou, Rail::Asked, Rail::Running, Rail::NeedsYou, Rail::Idle, Rail::Idle]
    );
    assert_eq!(Rail::Asked.word(), Some(crate::fonts::label::ASKED));
    assert_eq!(crate::glyph::session(Rail::Asked), (Some(crate::glyph::Glyph::Needs), crate::glyph::Family::Ask));
    let mut attention: Vec<&str> = m.attention().map(|s| s.id.as_str()).collect();
    attention.sort_unstable();
    assert_eq!(attention, ["p", "w"], "the top bar's NEEDS YOU: the live wait card's session, never one as last seen");
    let r = crate::rail::readout_live(&m, m.sessions.iter().find(|s| s.id == "w").unwrap(), 0);
    assert!(r.lines[0].2.starts_with("1 card waits on you"), "{:?}", r.lines[0]);
    let r = crate::rail::readout_live(&m, m.sessions.iter().find(|s| s.id == "o").unwrap(), 0);
    assert_eq!((r.lines[0].1.as_str(), r.lines[0].2.as_str()), ("asked", "1 card open"));
}

/// The dots NEEDS YOU and ASKED draw stay apart (OKLab ΔE ≥ `DOT_APART`)
/// in every stock theme with every stock accent; an accent that is (near)
/// the theme's warning colour (a custom one can be; phosphor's own is)
/// draws ASKED's in the chrome's ink, prose or dim text instead.
#[test]
fn needs_you_and_asked_dots_stay_apart() {
    let (mut min, mut fallback, mut n) = (f64::MAX, 0, 0);
    for (name, raw) in crate::palettes::THEMES {
        let accents = std::iter::once(None).chain(crate::theme::PRESETS.iter().map(|p| Some(p.1)));
        for accent in accents.chain(std::iter::once(Some(raw.warning_color))) {
            let (r, _) = crate::theme::resolve(&raw, accent);
            let p = Palette::of(&r);
            let (wait, asked) = dot_colors(&p);
            let d = apart(wait, asked);
            assert!(d >= DOT_APART, "{name} {accent:06x?}: the dots ΔE {d:.3}");
            assert_eq!(wait, p.warn_mark_ch);
            if accent == Some(raw.warning_color) {
                assert_ne!(asked, p.focus_ch, "{name}: the accent is its warning colour: ASKED's dot falls back");
            }
            if asked != p.focus_ch && accent != Some(raw.warning_color) {
                eprintln!("dots: {name} {accent:06x?}: the accent ΔE {:.3} off NEEDS YOU's: ASKED's falls back", apart(wait, p.focus_ch));
            }
            fallback += usize::from(asked != p.focus_ch);
            min = min.min(d);
            n += 1;
        }
    }
    eprintln!("dots: {n} theme × accent cases, least ΔE {min:.3}, {fallback} fall back");
}

// ── the window ────────────────────────────────────────────────────────

type Win = (AnyWindowHandle, Entity<AppView>);

/// The `--offline` demo's window (as `test_kit::open_at`), its synthetic
/// daemon answering: attaches and card answers alike. What would have
/// gone to a daemon is the view's record (`AppView::card_sends`).
fn open(cx: &mut TestAppContext) -> Win {
    let (w, h) = (1440., 900.);
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        cx.set_app_identity("synaps-desktop", "Synaps");
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        let text = crate::theme::Panels::default().text_size;
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(w), px(h)) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        crate::beat::open_window(options, cx, true, |window, cx| {
            cx.new(|cx| {
                let v = AppView::new(None, Some(crate::demo::events()), window, cx);
                v.fx.pin_clock();
                v
            })
        })
        .expect("a test window")
    });
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, w, _| w.activate_window());
    wait(cx, window, &view, 1000);
    (window, view)
}

fn wait(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ms: u64) {
    let d = Duration::from_millis(ms);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
    cx.executor().advance_clock(d);
    settle(cx, w, 8);
}

fn settle(cx: &mut TestAppContext, w: AnyWindowHandle, max: usize) -> usize {
    for n in 0..max {
        let ran = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        if ran == 0 {
            return n;
        }
    }
    max
}

/// What came plays out (rows arrive and grow): two seconds of frames.
fn play(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    for _ in 0..40 {
        wait(cx, w, view, 50);
    }
}

/// Session `id` opened in a new tab (the demo answers its attach).
fn open_tab(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str) {
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
    });
    cx.run_until_parked();
    play(cx, w, view);
}

fn show_tab(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ix: usize) {
    view.update(cx, |v, cx| {
        v.focus_tab(ix);
        cx.notify();
    });
    cx.run_until_parked();
    play(cx, w, view);
}

/// A key pressed and let go, then 40 ms (as `ui_tests::tap`: the app reads
/// a press of the same key within 30 ms of its release as a repeat).
fn key(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    let keystroke = gpui_kit::Keystroke::parse_with_os(k, cx.update(|cx| cx.platform_info().os)).expect("a keystroke");
    let down = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.update_window(w, |_, win, cx| {
        win.dispatch_event(down, cx);
    })
    .expect("the window");
    cx.run_until_parked();
    cx.update_window(w, |_, win, cx| {
        win.dispatch_event(gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }), cx);
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, w, 8);
    std::thread::sleep(Duration::from_millis(40));
}

fn has(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel)).is_some()
}

fn center(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Point<Pixels> {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel.clone())).unwrap_or_else(|| panic!("{sel} drawn")).center()
}

fn click(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) {
    let position = center(cx, w, sel);
    let mut vcx = VisualTestContext::from_window(w, cx);
    vcx.simulate_event(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() });
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1 });
    vcx.run_until_parked();
    settle(cx, w, 8);
}

/// The session on screen.
fn shown(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()))
}

fn tab_of(cx: &mut TestAppContext, view: &Entity<AppView>, s: &str) -> u64 {
    view.read_with(cx, |v, _| v.model.tabs.iter().find(|t| t.session.as_deref() == Some(s)).map(|t| t.id).expect("its tab"))
}

/// Card `tool`'s row: its id, and whether it is in the list's view.
fn row(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str) -> Option<(u64, bool)> {
    view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].transcript.items();
        let (ix, d) = v.panes.cur.disp.iter().enumerate().find(|(_, d)| {
            d.kind == crate::turns::Kind::Ask && items.get(d.item).and_then(|i| i.tool.as_ref()).is_some_and(|t| t.tool_id == tool)
        })?;
        let view_b = v.panes.cur.list.viewport_bounds();
        let seen = v.panes.cur.list.bounds_for_item(ix).is_some_and(|b| b.top() < view_b.bottom() && b.bottom() > view_b.top());
        Some((d.id, seen))
    })
}

/// The card's controls armed (`crate::app::ARM` past their first paint).
fn arm(cx: &mut TestAppContext, view: &Entity<AppView>, row: u64) {
    let at = view.read_with(cx, |v, _| v.panes.cur.ask_shown.borrow().iter().filter(|((r, _), _)| *r == row).map(|(_, t)| *t).max());
    crate::app::pin_arm_clock(Some(at.expect("its controls were drawn") + crate::app::ARM));
}

fn sends(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<crate::deliver::Outgoing> {
    view.read_with(cx, |v, _| v.card_sends.clone())
}

/// Session events on the attachment on screen, as the client hands them over.
fn heard(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, events: Vec<Value>) {
    let (seq, sid) = view.read_with(cx, |v, _| {
        let l = &v.model.lives[v.panes.cur.live];
        (l.attachment().expect("attached"), l.active.as_ref().unwrap().session_id.clone())
    });
    view.update(cx, |v, cx| {
        let batch = events
            .into_iter()
            .map(|e| {
                let ev = decode_event(&e).unwrap_or_else(|err| panic!("{err}: {e}"));
                synaps_client::Event::session(seq, sid.clone(), 0, ev, Arc::new(e))
            })
            .collect();
        v.apply_batch(batch, cx);
    });
    cx.run_until_parked();
    settle(cx, w, 8);
}

fn landed(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<(String, String)> {
    view.read_with(cx, |v, _| v.lookout.landed.as_ref().map(|k| (k.session.clone(), k.tool_id.clone())))
}

const SHIP: &str = "Run the migration on staging now?";

/// The window's first tab on screen, `ASKS` (a wait card and an open one)
/// in a background tab, as last seen.
fn asks_behind(cx: &mut TestAppContext) -> Win {
    let (w, view) = open(cx);
    open_tab(cx, w, &view, ASKS);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS));
    show_tab(cx, w, &view, 0);
    assert_ne!(shown(cx, &view).as_deref(), Some(ASKS), "back on the first tab");
    (w, view)
}

/// F10 by mouse, 2 clicks (§7): `ASKS` waits in a background tab, whose
/// dot says so as last seen (hollow: its tab cache, unverified); the rail
/// and the tab don't say NEEDS YOU of it. A click on the dot shows the
/// session, whose attach confirms the card, and lands on it, in view; then
/// its dot is filled (live). One click on an armed option answers it:
/// exactly one `Submit` of its message, on its attachment.
#[gpui_kit::test]
fn f10_the_tab_dot_then_an_option(cx: &mut TestAppContext) {
    let (w, view) = asks_behind(cx);
    view.update(cx, |v, _| v.client = None);
    let tab = tab_of(cx, &view, ASKS);
    let (live, seen) = view.read_with(cx, |v, _| (v.model.presence.of(ASKS), v.model.presence.last_seen(ASKS)));
    assert_eq!((live, seen), (Count::default(), Count { wait: 1, asked: 1 }), "as last seen: never NEEDS YOU");
    assert!(has(cx, w, format!("tab {tab} last-seen")) && !has(cx, w, format!("tab {tab} dot")), "its tab's dot, hollow");
    let others: Vec<u64> = view.read_with(cx, |v, _| v.model.tabs.iter().map(|t| t.id).filter(|t| *t != tab).collect());
    for t in others {
        assert!(!has(cx, w, format!("tab {t} dot")) && !has(cx, w, format!("tab {t} last-seen")), "no card, no dot");
    }
    // Click 1: the dot.
    click(cx, w, format!("tab {tab} last-seen"));
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS), "its session on screen");
    assert_eq!(landed(cx, &view), Some((ASKS.to_string(), "toolu_a8".to_string())), "on its wait card (the D2b seam)");
    assert_eq!(view.read_with(cx, |v, _| v.model.presence.of(ASKS)), Count { wait: 1, asked: 1 }, "live now");
    assert!(has(cx, w, format!("tab {tab} dot")), "its dot filled");
    let (id, seen) = row(cx, &view, "toolu_a8").expect("its row");
    assert!(seen, "the card in view");
    // Click 2: an option, once armed.
    arm(cx, &view, id);
    click(cx, w, format!("ask-option {id} 1"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1, "one answer");
    assert!(s[0].text.starts_with("[card c0p6r4n] ") && s[0].text.contains(SHIP), "{:?}", s[0].text);
    assert!(!s[0].busy, "a Submit: the session idle");
    let seq = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].attachment());
    assert_eq!(Some(s[0].seq), seq, "on its attachment");
    crate::app::pin_arm_clock(None);
}

/// F10 by keys (§7: Ctrl+Shift+J, then a key): from another session,
/// Ctrl+Shift+J shows the session of the first waiting card (a wait card
/// before an older open one) and lands on it, in view; keyboard entry is
/// D2b's (the seam records the card). Pressed again (40 ms on, never a
/// repeat), it stays. With no card open anywhere it does nothing.
#[gpui_kit::test]
fn f10_ctrl_shift_j_lands_on_the_oldest_waiting_card(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    key(cx, w, "ctrl-shift-j");
    assert_eq!(landed(cx, &view), None, "nothing waits: nothing");
    open_tab(cx, w, &view, ASK_OPEN);
    open_tab(cx, w, &view, ASKS);
    show_tab(cx, w, &view, 0);
    let first = shown(cx, &view);
    // ASK_OPEN's open card is not waited on: ASKS's wait card goes first.
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS));
    assert_eq!(landed(cx, &view), Some((ASKS.into(), "toolu_a8".into())));
    assert!(row(cx, &view, "toolu_a8").is_some_and(|r| r.1), "in view");
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS), "again: the same card");
    assert_ne!(first.as_deref(), Some(ASKS));
}

/// Shady #83 P2-3: a background tab's card is as last seen: hollow dot,
/// never NEEDS YOU or the top bar's count. Meanwhile
/// `change` happens to the session elsewhere; a click on its dot opens
/// the tab, whose attach says the card no longer waits: nothing lands,
/// and the last-seen dot goes.
fn last_seen_then(cx: &mut TestAppContext, change: fn(&mut Vec<Value>), still_open: bool) {
    let (w, view) = open(cx);
    let changed = Rc::new(Cell::new(false));
    let flag = changed.clone();
    view.update(cx, |v, _| {
        v.client = None;
        v.attach_source = Some(Box::new(move |sid: &str, seq: u64| {
            let mut evs = crate::demo::attach(sid, seq);
            if sid == ASKS
                && flag.get()
                && let Some(Event::Attached { attached, .. }) = evs.first_mut()
            {
                change(&mut attached.conversation.api_messages);
                attached.conversation.messages_len = attached.conversation.api_messages.len();
            }
            evs
        }));
    });
    open_tab(cx, w, &view, ASKS);
    let tab = tab_of(cx, &view, ASKS);
    assert_eq!(view.read_with(cx, |v, _| v.model.presence.of(ASKS)), Count { wait: 1, asked: 1 }, "live");
    assert!(has(cx, w, format!("tab {tab} dot")));
    show_tab(cx, w, &view, 0);
    let meta: SessionMeta = serde_json::from_value(json!({"id": ASKS, "lifecycle": "live"})).unwrap();
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.presence.of(ASKS), Count::default(), "as last seen: not counted");
        assert_eq!(v.model.presence.last_seen(ASKS), Count { wait: 1, asked: 1 });
        assert!(!matches!(v.model.fact(&meta), Rail::NeedsYou | Rail::Asked), "not NEEDS YOU, not ASKED");
        assert!(v.model.attention().all(|s| s.id != ASKS), "not in the top bar's count");
    });
    assert!(has(cx, w, format!("tab {tab} last-seen")) && !has(cx, w, format!("tab {tab} dot")), "hollow");
    // Elsewhere, meanwhile; then its dot.
    changed.set(true);
    click(cx, w, format!("tab {tab} last-seen"));
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS), "its tab, attached again");
    view.read_with(cx, |v, _| {
        assert!(v.model.presence.get(ASKS, "toolu_a8").is_none(), "the attach says it no longer waits");
        assert!(v.lookout.land.is_none() && v.lookout.landed.is_none(), "nothing landed");
        assert_eq!(v.model.presence.of(ASKS), Count { wait: 0, asked: usize::from(still_open) });
    });
    assert!(!has(cx, w, format!("tab {tab} last-seen")), "the last-seen dot gone");
    assert_eq!(has(cx, w, format!("tab {tab} dot")), still_open, "ASKED's dot only for the open card left");
}

/// The ship card's input, from `ASKS`'s history.
fn ship_input(messages: &[Value]) -> Value {
    messages
        .iter()
        .flat_map(|m| m["content"].as_array().into_iter().flatten())
        .find(|b| b["id"] == "toolu_a8")
        .map(|b| b["input"].clone())
        .expect("the ship card")
}

#[gpui_kit::test]
fn a_last_seen_card_answered_elsewhere_lands_nowhere(cx: &mut TestAppContext) {
    last_seen_then(
        cx,
        |m| {
            let input = ship_input(m);
            let cards_spec::Input::Create { doc, .. } = cards_spec::parse_input(&input).unwrap() else { unreachable!() };
            let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&doc, &json!({"choice": "Run it now"})).unwrap());
            let id = CardId::parse("c0p6r4n").unwrap();
            m.push(json!({"role": "user", "content": cards_spec::message::build(&id, &doc, &reply)}));
        },
        true,
    );
}

#[gpui_kit::test]
fn a_last_seen_card_closed_elsewhere_lands_nowhere(cx: &mut TestAppContext) {
    last_seen_then(
        cx,
        |m| {
            m.push(json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_x9", "name": "cards_ui", "input": {"close": "c0p6r4n"}}]}));
            m.push(json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_x9", "content": "Card c0p6r4n closed (cards/1)."}]}));
        },
        true,
    );
}

#[gpui_kit::test]
fn a_last_seen_card_compacted_away_lands_nowhere(cx: &mut TestAppContext) {
    last_seen_then(cx, |m| *m = vec![json!({"role": "user", "content": "Summary: tokens moved to Postgres; staging next."})], false);
}

#[gpui_kit::test]
fn a_last_seen_card_gone_with_new_lands_nowhere(cx: &mut TestAppContext) {
    last_seen_then(cx, Vec::clear, false);
}

/// Shady #83 P2-8: Ctrl+Shift+J while a field composes (an IME's marked
/// text) is the IME's: nothing moves, the composition stays. Over a
/// session whose authorize prompt waits, in the palette's field: the
/// prompt untouched too. Then in the composer of a session without one.
/// Committed, it jumps to the card (never the prompt: its keys are
/// Ctrl+J's).
#[gpui_kit::test]
fn ctrl_shift_j_leaves_a_composition_to_the_ime(cx: &mut TestAppContext) {
    let (w, view) = asks_behind(cx);
    let prompts = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.pending_prompts().len());
    let first = shown(cx, &view);
    assert_eq!(prompts(cx), 1, "the first session's authorize prompt waits");
    let mark = |cx: &mut TestAppContext, f: &dyn Fn(&mut Window, &mut App)| {
        cx.update_window(w, |_, window, cx| f(window, cx)).expect("the window");
        settle(cx, w, 8);
    };
    let composing =
        |cx: &mut TestAppContext| cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composing_now(window, cx))).unwrap();
    // The palette's field, over the prompt.
    let p = view.read_with(cx, |v, _| v.palette_input.clone());
    mark(cx, &|window, cx| {
        view.update(cx, |v, cx| v.open_palette(crate::palette::Mode::GoTo, window, cx));
        window.focus(&p.focus_handle(cx), cx);
        p.update(cx, |s, cx| EntityInputHandler::replace_and_mark_text_in_range(s, None, "じ", None, window, cx));
    });
    assert!(composing(cx), "the palette's field composes");
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view), first, "composing: nothing moves");
    assert!(view.read_with(cx, |v, _| v.palette.is_some() && v.lookout.land.is_none()), "the palette stays, nothing lands");
    assert!(
        cx.update_window(w, |_, window, cx| p.update(cx, |s, cx| s.marked_text_range(window, cx))).unwrap().is_some(),
        "the composition stays"
    );
    assert_eq!(prompts(cx), 1, "the prompt untouched");
    mark(cx, &|window, cx| {
        p.update(cx, |s, cx| EntityInputHandler::unmark_text(s, window, cx));
        view.update(cx, |v, cx| v.close_palette(window, cx));
    });
    // The composer, on a session without a prompt (an idle one; the
    // window drives it, so its composer takes text).
    open_tab(cx, w, &view, crate::demo::IDLE_CPU);
    let c = view.read_with(cx, |v, _| v.panes.cur.composer.clone());
    view.update(cx, |v, cx| {
        v.owner_here = true;
        v.focus_composer(cx);
    });
    settle(cx, w, 8);
    mark(cx, &|window, cx| {
        c.update(cx, |s, cx| EntityInputHandler::replace_and_mark_text_in_range(s, None, "か", None, window, cx));
    });
    assert!(composing(cx), "the composer composes");
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(crate::demo::IDLE_CPU), "composing: nothing moves");
    assert!(view.read_with(cx, |v, _| v.lookout.land.is_none()));
    assert!(
        cx.update_window(w, |_, window, cx| c.update(cx, |s, cx| s.marked_text_range(window, cx))).unwrap().is_some(),
        "the composition stays"
    );
    mark(cx, &|window, cx| c.update(cx, |s, cx| EntityInputHandler::unmark_text(s, window, cx)));
    assert!(!composing(cx));
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS), "committed: the card");
    assert_eq!(landed(cx, &view), Some((ASKS.into(), "toolu_a8".into())));
}

/// Ctrl+P: WAITING first, each open card a row (live wait cards, live open
/// ones, then those as last seen: its question, its session, NEEDS YOU,
/// ASKED or "last seen"); picking one lands on it.
#[gpui_kit::test]
fn the_palette_lists_waiting_cards_first(cx: &mut TestAppContext) {
    let (w, view) = asks_behind(cx);
    open_tab(cx, w, &view, ASK_OPEN);
    let goto = view.update(cx, |v, cx| v.palette_entries(cx).0);
    let waiting: Vec<(&str, &str, &str)> = goto
        .iter()
        .take_while(|e| e.group == crate::palette::Group::Waiting)
        .map(|e| {
            let tool = match &e.act {
                crate::palette::Act::Card(_, t) => t.as_str(),
                _ => "",
            };
            (e.title.as_ref(), tool, e.meta.as_ref())
        })
        .collect();
    // ASK_OPEN on screen (live, ASKED); ASKS behind, as last seen.
    assert_eq!(
        waiting.iter().map(|w| (w.0, w.1)).collect::<Vec<_>>(),
        [
            ("Which level should the backfill log at?", "toolu_o1"),
            (SHIP, "toolu_a8"),
            ("Which suites should pass before the cut-over?", "toolu_a7")
        ]
    );
    assert!(waiting[0].2.contains("asked") && !waiting[0].2.contains("last seen"), "{}", waiting[0].2);
    assert!(waiting[1].2.contains("last seen · waited on you"), "{}", waiting[1].2);
    assert!(waiting[2].2.contains("last seen · asked"), "{}", waiting[2].2);
    assert!(goto.iter().skip(3).all(|e| e.group != crate::palette::Group::Waiting), "one group, first");
    let pick = goto[1].act.clone();
    cx.update_window(w, |_, win, cx| {
        view.update(cx, |v, cx| {
            if let crate::palette::Act::Card(s, t) = pick {
                v.go_to_card(s, t, win, cx);
            }
        })
    })
    .unwrap();
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS));
    assert!(row(cx, &view, "toolu_a8").is_some_and(|r| r.1));
}

/// §6.1's motion: dots and badges are still. With cards waiting in a
/// background tab and on screen, at Full, Reduced and Off, the window
/// settles and asks for no frame. And frames that move nothing never
/// derive the presence again (Shady #83 P3-2): no row is read.
#[gpui_kit::test]
fn dots_and_badges_draw_no_idle_frames(cx: &mut TestAppContext) {
    let (w, view) = asks_behind(cx);
    open_tab(cx, w, &view, ASK_FORM);
    for level in [crate::motion::Level::Full, crate::motion::Level::Reduced, crate::motion::Level::Off] {
        cx.update(|cx| {
            cx.set_global(crate::motion::Setting { level: Some(level), lock: None });
            crate::motion::Level::apply(cx);
        });
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        wait(cx, w, &view, 2000);
        let tab = tab_of(cx, &view, ASKS);
        assert!(has(cx, w, format!("tab {tab} last-seen")), "{level:?}: the dot");
        assert!(!view.read_with(cx, |v, _| v.fx.pending().0), "{level:?}: no frame pending");
        assert_eq!(settle(cx, w, 8), 0, "{level:?}: 0 idle frames");
    }
    let n = view.read_with(cx, |v, _| v.lookout.derivations);
    for _ in 0..5 {
        view.update(cx, |_, cx| cx.notify());
        settle(cx, w, 8);
        heard(cx, w, &view, vec![json!({"ev": "idle"})]);
    }
    assert_eq!(view.read_with(cx, |v, _| v.lookout.derivations), n, "nothing moved: nothing derived");
    show_tab(cx, w, &view, 0);
    assert!(view.read_with(cx, |v, _| v.lookout.derivations) > n, "a tab shown: derived again");
}

/// The landing waits for its session and its row, never jumps the view
/// later: one whose card is gone, or past its wait, is dropped. A jump to
/// a card the window doesn't hold goes nowhere.
#[gpui_kit::test]
fn a_landing_that_cannot_happen_is_dropped(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    let sid = shown(cx, &view).expect("attached");
    let went = cx.update_window(w, |_, win, cx| view.update(cx, |v, cx| v.go_to_card(sid.clone(), "toolu_none".into(), win, cx))).unwrap();
    assert!(!went, "no such open card: nowhere");
    let gone = Key { session: sid.clone(), tool_id: "toolu_none".into(), card: CardId::parse("c000000").unwrap() };
    view.update(cx, |v, cx| {
        v.lookout.land = Some(Land { key: gone.clone(), at: Instant::now() });
        cx.notify();
    });
    play(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.lookout.land.is_none()), "no such card in a settled history: dropped");
    view.update(cx, |v, _| {
        v.lookout.land = Some(Land { key: Key { session: "elsewhere".into(), ..gone }, at: Instant::now() - LAND_WAIT * 2 })
    });
    view.update(cx, |_, cx| cx.notify());
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.lookout.land.is_none()), "past its wait: dropped");
}
