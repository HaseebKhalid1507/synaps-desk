//! D3's tests (synaps-cards `docs/design.md` §6.1, §7's F10 and F12, §9.3):
//! the presence model and its counters, the OS told once per wait card,
//! NEEDS YOU and ASKED, and in the window the tab dot, Ctrl+Shift+J, the
//! palette's WAITING, the notification's click, and no idle frames.
//! F11 (a session only in the rail) stays the known gap (§8 #1): nothing
//! here reads a session the window doesn't hold.
//!
//! The window's helpers are `test_kit`'s (`ui_tests`' own), copied: that
//! kit is protected and its children are the files it lists.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    SystemNotificationResponse, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
};
use serde_json::{Value, json};
use synaps_client::Transcript;
use synaps_client::protocol::{SessionMeta, decode_event};

use super::*;
// `super`'s `gpui_kit::*` brings the kit's `test` too: the std one here.
use crate::app::AppView;
use crate::demo::{ASK_FORM, ASK_OPEN, ASKS};
use crate::model::{ConnState, Model};
use crate::rail::State as Rail;
use core::prelude::v1::test;

// ── the model ─────────────────────────────────────────────────────────

fn card(session: &str, tool: &str, wait: bool, at: Option<i64>, order: (usize, usize)) -> Open {
    Open { session: session.into(), tool_id: tool.into(), wait, question: format!("{tool}?"), at, live: true, order }
}

/// The presence counters: per session, wait and asked apart; the window's
/// cards oldest first (a row without a time after those with one, then
/// the source's order); Ctrl+Shift+J's the oldest wait card, else the
/// oldest open one; WAITING lists wait cards first; a change bumps the
/// revision, the same cards don't.
#[test]
fn presence_counts_and_orders_oldest_first() {
    let mut p = Presence::default();
    assert_eq!(p.next(), None);
    assert_eq!(p.of("a"), Count::default());
    let cards = vec![
        card("b", "t4", false, None, (1, 9)),
        card("a", "t2", true, Some(300), (0, 5)),
        card("b", "t3", true, Some(200), (1, 2)),
        card("a", "t1", false, Some(100), (0, 1)),
    ];
    assert!(p.set(cards.clone()));
    let g = p.rev();
    let order: Vec<&str> = p.cards().iter().map(|c| c.tool_id.as_str()).collect();
    assert_eq!(order, ["t1", "t3", "t2", "t4"], "oldest first, no time last");
    assert_eq!(p.of("a"), Count { wait: 1, asked: 1 });
    assert_eq!(p.of("b"), Count { wait: 1, asked: 1 });
    assert_eq!(p.of("c"), Count::default());
    assert_eq!(p.next().map(|c| c.tool_id.as_str()), Some("t3"), "the oldest wait card, though an open one is older");
    let listed: Vec<&str> = p.listed().map(|c| c.tool_id.as_str()).collect();
    assert_eq!(listed, ["t3", "t2", "t1", "t4"], "wait cards first, each kind oldest first");
    assert!(!p.set(cards.into_iter().rev().collect()), "the same cards, in any order: no change");
    assert_eq!(p.rev(), g);
    assert!(p.set(vec![card("a", "t1", false, Some(100), (0, 1))]));
    assert_eq!(p.next().map(|c| c.tool_id.as_str()), Some("t1"), "no wait card: the oldest open one");
    assert_eq!(p.of("b"), Count::default());
    assert!(p.rev() > g);
    assert!(p.set(Vec::new()));
    assert_eq!(p.next(), None);
}

/// The demo's attach of `id`, folded as a client would.
fn transcript(id: &str) -> Transcript {
    let mut t = Transcript::new();
    for ev in crate::demo::attach(id, 1) {
        if let synaps_client::Event::Attached { attached, .. } = ev {
            t.load_attached(&attached).expect("the demo's history loads");
        }
    }
    t
}

/// A session's open cards are its fold's, as its rows draw it: bound, not
/// answered, not closed or replaced; each with its question, mode and row
/// time. The ones this desk answered (on their way or in) are not.
#[test]
fn open_cards_are_the_folds_less_what_this_desk_answered() {
    let t = transcript(ASKS);
    let open = open_cards(ASKS, &t, true, 0, |_| false);
    let got: Vec<(&str, bool, &str)> = open.iter().map(|c| (c.tool_id.as_str(), c.wait, c.question.as_str())).collect();
    assert_eq!(
        got,
        [("toolu_a7", false, "Which suites should pass before the cut-over?"), ("toolu_a8", true, "Run the migration on staging now?")],
        "of seven cards: answered, changed, skipped, closed and replaced are not open"
    );
    assert!(open.iter().all(|c| c.at.is_some() && c.live && c.session == ASKS));
    assert!(open[0].at < open[1].at, "its row's time");
    let less = open_cards(ASKS, &t, true, 0, |c| c.tool_id == "toolu_a8");
    assert_eq!(less.iter().map(|c| c.tool_id.as_str()).collect::<Vec<_>>(), ["toolu_a7"], "answered here: not waiting");
    assert_eq!(open_cards(ASK_OPEN, &transcript(ASK_OPEN), true, 0, |_| false).len(), 1);
    assert!(open_cards(crate::demo::SID, &transcript(crate::demo::SID), true, 0, |_| false).is_empty(), "no cards, none open");
}

/// F12's rule: each wait card told once, only one that came while the
/// window held its session (never one an attach brought), and only while
/// the window is away or its session isn't the one on screen; seen all
/// the same (never told later); the setting off tells nothing; a card as
/// last seen (a background tab's) is never new.
#[test]
fn the_os_is_told_of_each_wait_card_once() {
    let mut t = Told::default();
    let held = vec!["a".to_string(), "b".to_string()];
    let a1 = card("a", "t1", true, Some(1), (0, 0));
    // The attach brings a1: it was there before.
    assert!(t.fresh(std::slice::from_ref(&a1), &held, Some("a"), false, true).is_empty(), "an attach's card: no");
    // a2 comes in a, the window away: told.
    let a2 = card("a", "t2", true, Some(2), (0, 1));
    let now = vec![a1.clone(), a2.clone()];
    assert_eq!(t.fresh(&now, &held, Some("a"), false, true), vec![a2.clone()]);
    assert!(t.fresh(&now, &held, Some("a"), false, true).is_empty(), "once");
    // b1 comes in b while a is on screen in an active window: told (another session).
    let b1 = card("b", "t3", true, Some(3), (1, 0));
    let now = vec![a1.clone(), a2.clone(), b1.clone()];
    assert_eq!(t.fresh(&now, &held, Some("a"), true, true), vec![b1.clone()]);
    // b2 comes in b, b on screen, the window active: seen, not told; not later either.
    let b2 = card("b", "t4", true, Some(4), (1, 1));
    let mut now = now;
    now.push(b2.clone());
    assert!(t.fresh(&now, &held, Some("b"), true, true).is_empty(), "watched");
    assert!(t.fresh(&now, &held, Some("b"), false, true).is_empty(), "never told later");
    // An open card (not waited on) is never told.
    now.push(card("a", "t5", false, Some(5), (0, 2)));
    assert!(t.fresh(&now, &held, None, false, true).is_empty(), "ASKED is not NEEDS YOU");
    // Off: nothing, and seen.
    let a6 = card("a", "t6", true, Some(6), (0, 3));
    now.push(a6.clone());
    assert!(t.fresh(&now, &held, None, false, false).is_empty(), "notify.cards off");
    assert!(t.fresh(&now, &held, None, false, true).is_empty(), "not later when on again");
    // A background tab's card, as last seen: never new.
    let c1 = Open { live: false, ..card("c", "t7", true, Some(7), (1000, 0)) };
    now.push(c1);
    assert!(t.fresh(&now, &held, None, false, true).is_empty(), "as last seen");
    // A session attached only now: its cards are the attach's.
    let d1 = card("d", "t8", true, Some(8), (2, 0));
    now.push(d1.clone());
    let held2 = vec!["a".to_string(), "b".to_string(), "d".to_string()];
    assert!(t.fresh(&now, &held2, None, false, true).is_empty(), "d's attach brought it");
    let d2 = card("d", "t9", true, Some(9), (2, 1));
    now.push(d2.clone());
    assert_eq!(t.fresh(&now, &held2, None, false, true), vec![d2]);
}

#[test]
fn notify_cards_off_is_false_or_off() {
    assert!(!quiet(None));
    assert!(!quiet(Some(&json!(true))));
    assert!(!quiet(Some(&json!("on"))));
    assert!(quiet(Some(&json!(false))));
    assert!(quiet(Some(&json!("off"))));
}

/// The rail (§6.1): an attached session's wait card is NEEDS YOU, an open
/// one ASKED once its turn is over (RUNNING while it runs: the agent works
/// on), a prompt still NEEDS YOU; ASKED has its word, the accent's mark,
/// and ranks after RUNNING. A session the window holds nothing of has no
/// badge from cards (F11, §8 #1).
#[test]
fn a_waiting_card_is_needs_you_and_an_open_one_asked() {
    let mut m = Model::new(ConnState::Connecting);
    let metas: Vec<SessionMeta> = serde_json::from_value(json!([
        {"id": "w", "lifecycle": "live"},
        {"id": "o", "lifecycle": "live"},
        {"id": "r", "lifecycle": "live", "busy": true},
        {"id": "p", "lifecycle": "live", "awaiting_input": 1},
        {"id": "n", "lifecycle": "live"},
    ]))
    .unwrap();
    m.apply(synaps_client::Event::Sessions(metas));
    let st = |m: &Model, id: &str| m.state(m.sessions.iter().find(|s| s.id == id).unwrap());
    assert_eq!(st(&m, "w"), Rail::Idle);
    m.presence.set(vec![
        card("w", "t1", true, Some(1), (0, 0)),
        card("o", "t2", false, Some(2), (1, 0)),
        card("r", "t3", false, Some(3), (2, 0)),
        card("p", "t4", false, Some(4), (3, 0)),
    ]);
    assert_eq!(["w", "o", "r", "p", "n"].map(|id| st(&m, id)), [Rail::NeedsYou, Rail::Asked, Rail::Running, Rail::NeedsYou, Rail::Idle]);
    assert_eq!(Rail::Asked.word(), Some(crate::fonts::label::ASKED));
    assert_eq!(crate::glyph::session(Rail::Asked), (Some(crate::glyph::Glyph::Needs), crate::glyph::Family::Ask));
    assert_eq!(m.attention().count(), 2, "the top bar's NEEDS YOU counts the wait card's session");
    let r = crate::rail::readout_live(&m, m.sessions.iter().find(|s| s.id == "w").unwrap(), 0);
    assert!(r.lines[0].2.starts_with("1 card waits on you"), "{:?}", r.lines[0]);
    let r = crate::rail::readout_live(&m, m.sessions.iter().find(|s| s.id == "o").unwrap(), 0);
    assert_eq!((r.lines[0].1.as_str(), r.lines[0].2.as_str()), ("asked", "1 card open"));
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

/// The agent asks `input` live as card `card` (call `tool`) and its turn
/// ends; the round's history lands.
fn ask_live(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, card: &str, input: Value) {
    let (doc, wait) = match cards_spec::parse_input(&input).unwrap() {
        cards_spec::Input::Create { doc, wait } => (doc, wait),
        cards_spec::Input::Close(_) => unreachable!(),
    };
    let result = format!(
        "{}\nIts answer will come as a user message starting \"[card {card}]\".",
        cards_spec::Envelope::showing(cards_spec::CardId::parse(card).unwrap(), &doc, wait)
    );
    let llm = |v: Value| json!({"ev": "stream", "event": v});
    heard(
        cx,
        w,
        view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"}),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": tool})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": tool, "input": input})),
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": result})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    let messages = vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": tool, "name": "cards_ui", "input": input}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": result}]}),
    ];
    let (seq, sid) = view.read_with(cx, |v, _| {
        let l = &v.model.lives[v.panes.cur.live];
        (l.attachment().expect("attached"), l.active.as_ref().unwrap().session_id.clone())
    });
    view.update(cx, |v, cx| {
        let base_len = v.model.lives[v.panes.cur.live].transcript.history_len();
        let update = synaps_client::HistoryUpdate::Append { base_len, messages };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx);
    });
    cx.run_until_parked();
    play(cx, w, view);
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
/// dot says so (as last seen: its tab cache); the rail and the tab say
/// NEEDS YOU of it. A click on the dot shows the session and lands on its
/// wait card, in view; one click on an armed option answers it: exactly
/// one `Submit` of its message, on its attachment.
#[gpui_kit::test]
fn f10_the_tab_dot_then_an_option(cx: &mut TestAppContext) {
    let (w, view) = asks_behind(cx);
    view.update(cx, |v, _| v.client = None);
    let tab = tab_of(cx, &view, ASKS);
    assert_eq!(view.read_with(cx, |v, _| v.model.presence.of(ASKS)), Count { wait: 1, asked: 1 }, "as last seen");
    assert!(has(cx, w, format!("tab {tab} dot")), "its tab's dot");
    let others: Vec<u64> = view.read_with(cx, |v, _| v.model.tabs.iter().map(|t| t.id).filter(|t| *t != tab).collect());
    for t in others {
        assert!(!has(cx, w, format!("tab {t} dot")), "no card, no dot");
    }
    // Click 1: the dot.
    click(cx, w, format!("tab {tab} dot"));
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS), "its session on screen");
    let landed = view.read_with(cx, |v, _| v.lookout.landed.clone());
    assert_eq!(landed, Some((ASKS.to_string(), "toolu_a8".to_string())), "on its wait card (the D2b seam)");
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
/// Ctrl+Shift+J shows the session of the oldest waiting card (a wait card
/// before an older open one) and lands on it, in view; keyboard entry is
/// D2b's (the seam records the card). Pressed again (40 ms on, never a
/// repeat), it stays. With no card open anywhere it does nothing.
#[gpui_kit::test]
fn f10_ctrl_shift_j_lands_on_the_oldest_waiting_card(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    key(cx, w, "ctrl-shift-j");
    assert_eq!(view.read_with(cx, |v, _| v.lookout.landed.clone()), None, "nothing waits: nothing");
    open_tab(cx, w, &view, ASK_OPEN);
    open_tab(cx, w, &view, ASKS);
    show_tab(cx, w, &view, 0);
    let first = shown(cx, &view);
    // ASK_OPEN's open card is not waited on: ASKS's wait card goes first.
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS));
    assert_eq!(view.read_with(cx, |v, _| v.lookout.landed.clone()), Some((ASKS.into(), "toolu_a8".into())));
    assert!(row(cx, &view, "toolu_a8").is_some_and(|r| r.1), "in view");
    key(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASKS), "again: the same card");
    assert_ne!(first.as_deref(), Some(ASKS));
}

/// F12: a wait card that comes while the window is away notifies once:
/// its session's name, its question, a tag of its own. Its click brings
/// the window up and lands on the card (scrolled away meanwhile), and one
/// option click answers it.
/// A card an attach brings, an open card, and any card with
/// `notify.cards` off notify nothing.
#[gpui_kit::test]
fn f12_a_wait_card_that_comes_while_away_notifies_once(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    view.update(cx, |v, _| v.client = None);
    open_tab(cx, w, &view, ASK_OPEN);
    VisualTestContext::from_window(w, cx).deactivate_window();
    settle(cx, w, 8);
    assert!(!view.read_with(cx, |v, _| v.window_active), "the window away");
    // An attach's wait card (ASK_FORM's) was there before: nothing.
    open_tab(cx, w, &view, ASK_FORM);
    assert!(cx.shown_system_notifications().is_empty(), "an attach's card: no notification");
    show_tab(cx, w, &view, 1);
    assert_eq!(shown(cx, &view).as_deref(), Some(ASK_OPEN));
    // An open card comes: nothing.
    let note = json!({"question": "Keep the old log format too?", "options": ["Yes", "No"]});
    ask_live(cx, w, &view, "toolu_n1", "c3n9w2a", note);
    assert!(cx.shown_system_notifications().is_empty(), "ASKED: no notification");
    // A wait card comes: one.
    let cut = json!({"question": "Cut the old log format over tonight?", "wait": true, "options": ["Tonight", "Tomorrow"]});
    ask_live(cx, w, &view, "toolu_n2", "c3n9w2b", cut);
    let shown_n = cx.shown_system_notifications();
    assert_eq!(shown_n.len(), 1, "one notification: {shown_n:?}");
    assert_eq!(shown_n[0].title.as_ref(), "Needs you · Relay log levels");
    assert_eq!(shown_n[0].body.as_ref(), "Cut the old log format over tonight?");
    assert!(shown_n[0].tag.len() <= 64);
    wait(cx, w, &view, 500);
    heard(cx, w, &view, vec![json!({"ev": "idle"})]);
    assert_eq!(cx.shown_system_notifications().len(), 1, "once");
    // The list scrolled away from it (the synthetic daemon never had this
    // card: a switch's re-attach would fold it away), then its click.
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to(gpui_kit::ListOffset { item_ix: 0, offset_in_item: px(0.) });
        cx.notify();
    });
    settle(cx, w, 8);
    assert!(row(cx, &view, "toolu_n2").is_some_and(|r| !r.1), "out of view");
    cx.simulate_system_notification_response(SystemNotificationResponse { tag: shown_n[0].tag.clone(), action_id: None });
    cx.run_until_parked();
    play(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.window_active), "the window up");
    assert_eq!(shown(cx, &view).as_deref(), Some(ASK_OPEN), "its session");
    assert_eq!(view.read_with(cx, |v, _| v.lookout.landed.clone()), Some((ASK_OPEN.into(), "toolu_n2".into())));
    let (id, seen) = row(cx, &view, "toolu_n2").expect("its row");
    assert!(seen, "in view");
    arm(cx, &view, id);
    click(cx, w, format!("ask-option {id} 1"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1, "one answer");
    assert!(s[0].text.starts_with("[card c3n9w2b] "), "{:?}", s[0].text);
    crate::app::pin_arm_clock(None);
    // Off: nothing.
    view.update(cx, |v, _| v.app_settings.quiet = true);
    VisualTestContext::from_window(w, cx).deactivate_window();
    settle(cx, w, 8);
    let more = json!({"question": "And the archive?", "wait": true, "options": ["Keep", "Drop"]});
    ask_live(cx, w, &view, "toolu_n3", "c3n9w2c", more);
    assert_eq!(cx.shown_system_notifications().len(), 1, "notify.cards off: none");
}

/// Ctrl+P: WAITING first, each open card a row (wait cards first: its
/// question, its session, NEEDS YOU or ASKED); picking one lands on it.
#[gpui_kit::test]
fn the_palette_lists_waiting_cards_first(cx: &mut TestAppContext) {
    let (w, view) = asks_behind(cx);
    open_tab(cx, w, &view, ASK_OPEN);
    show_tab(cx, w, &view, 0);
    let goto = view.update(cx, |v, cx| v.palette_entries(cx).0);
    let waiting: Vec<(&str, &str)> = goto
        .iter()
        .take_while(|e| e.group == crate::palette::Group::Waiting)
        .map(|e| {
            (
                e.title.as_ref(),
                match &e.act {
                    crate::palette::Act::Card(_, t) => t.as_str(),
                    _ => "",
                },
            )
        })
        .collect();
    // ASK_OPEN's card (10:17:30) is older than ASKS's open one (10:25:30).
    assert_eq!(
        waiting,
        [
            (SHIP, "toolu_a8"),
            ("Which level should the backfill log at?", "toolu_o1"),
            ("Which suites should pass before the cut-over?", "toolu_a7")
        ]
    );
    assert!(goto[0].meta.contains("needs you") && goto[1].meta.contains("asked") && goto[2].meta.contains("asked"));
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
    assert_eq!(shown(cx, &view).as_deref(), Some(ASK_OPEN));
    assert!(row(cx, &view, "toolu_o1").is_some_and(|r| r.1));
}

/// §6.1's motion: dots and badges are still. With cards waiting in a
/// background tab and on screen, at Full, Reduced and Off, the window
/// settles and asks for no frame.
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
        assert!(has(cx, w, format!("tab {tab} dot")), "{level:?}: the dot");
        assert!(!view.read_with(cx, |v, _| v.fx.pending().0), "{level:?}: no frame pending");
        assert_eq!(settle(cx, w, 8), 0, "{level:?}: 0 idle frames");
    }
}

/// The landing waits for its session and its row, never jumps the view
/// later: one whose card is gone, or past its wait, is dropped.
#[gpui_kit::test]
fn a_landing_that_cannot_happen_is_dropped(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    let sid = shown(cx, &view).expect("attached");
    cx.update_window(w, |_, win, cx| view.update(cx, |v, cx| v.go_to_card(sid.clone(), "toolu_none".into(), win, cx))).unwrap();
    play(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.lookout.land.is_none()), "no such card in a settled history: dropped");
    view.update(cx, |v, _| {
        v.lookout.land = Some(Land { session: "elsewhere".into(), tool_id: "t".into(), at: Instant::now() - LAND_WAIT * 2 })
    });
    view.update(cx, |_, cx| cx.notify());
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.lookout.land.is_none()), "past its wait: dropped");
}
