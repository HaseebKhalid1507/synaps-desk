//! D5's desk side after Shady's review of #101 (`review-pr-desk101-shady.md`):
//! the lease's geometry as the renderer draws it (P2-5), one painted-target
//! rule for every action (P2-6), a press released anywhere (P2-7), just moved
//! by an agent op's frame, not by the clock (P2-8), the edit preview's text
//! drawn as D1 draws agent text (P2-9), a file's header (P2-10), a metric at
//! 320 px (P2-11), the `op skipped` chip, and the lease's note readable at 320
//! px and every text size (decision #5). Each fails on `653c30c`.

use gpui_kit::{
    AnyWindowHandle, Entity, ListOffset, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, point, px,
};
use serde_json::{Value, json};
use synaps_client::synthetic::{block_input, block_receipt};

use super::answer_tests::{has, heard, llm};
use super::{motion_level, open_at, settle, wait};
use crate::app::AppView;
use crate::turns::Kind;

type W = (AnyWindowHandle, Entity<AppView>);

/// Session `id` opened in `place`, attached by the demo's transport.
fn open_session(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str, place: crate::tabs::Place) {
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        v.open_tab(crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false }, place);
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.update(cx, |v, _| v.client = client);
    board(cx, w, view);
    wait(cx, w, view, 600);
}

/// The blocks demo session in a window `w` px wide at text step `text`,
/// its board drawn.
fn blocks_at(cx: &mut TestAppContext, w: f32, text: u8) -> W {
    let (win, view, _events) = open_at(cx, (w, 1000.), text);
    open_session(cx, win, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    (win, view)
}

fn blocks(cx: &mut TestAppContext, w: f32) -> W {
    blocks_at(cx, w, crate::theme::Panels::default().text_size)
}

/// The board's job runs and lands, and what it drew settles.
fn board(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    for _ in 0..4 {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        settle(cx, w, 8);
    }
}

/// Block `name`'s row in the pane acted on: its id and display index.
fn row(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> Option<(u64, usize)> {
    view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].shown().items();
        v.panes.cur.disp.iter().enumerate().find_map(|(ix, d)| {
            let Kind::Block { at } = d.kind else { return None };
            let s = items.get(d.item)?.blocks.as_ref()?.blocks.get(at as usize)?;
            (s.block.id.to_string() == name).then_some((d.id, ix))
        })
    })
}

fn id(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> u64 {
    row(cx, view, name).unwrap_or_else(|| panic!("block {name} drawn")).0
}

/// A group member's row id.
fn member(cx: &mut TestAppContext, view: &Entity<AppView>, group: u64, k: usize) -> u64 {
    view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(group).expect("the group drawn");
        crate::blocks::row_id(&s.members[k].block.id)
    })
}

fn show_block(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, name: &str) -> u64 {
    let (id, ix) = row(cx, view, name).unwrap_or_else(|| panic!("block {name} drawn"));
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    id
}

fn bounds(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel))
}

fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    bounds(cx, w, sel.clone()).unwrap_or_else(|| panic!("{sel} drawn"))
}

/// A live round of block calls (`(tool, ops, bound)`); the board lands.
fn round(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, calls: &[(&str, Value, bool)]) {
    let mut evs = vec![llm(json!({"kind": "llm", "llm": "response_start"}))];
    for (tool, ops, _) in calls {
        evs.push(llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": tool})));
        evs.push(llm(
            json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_block", "tool_id": tool, "input": block_input(ops.clone())}),
        ));
    }
    for (tool, ops, bound) in calls {
        if *bound {
            evs.push(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": block_receipt(ops)})));
        }
    }
    heard(cx, w, view, evs);
    board(cx, w, view);
}

fn turn(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "turn_started", "turn_baseline": 13, "trigger": "user", "user_text": "go on"})]);
}

fn mouse(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>, ev: &str) {
    let mut vcx = VisualTestContext::from_window(w, cx);
    match ev {
        "move" => vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }),
        "drag" => {
            vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() })
        }
        "down" => vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        _ => vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }),
    }
    vcx.run_until_parked();
    settle(cx, w, 8);
}

/// A click at `at`: the pointer there (the window's hit test is the
/// pointer's), down and up.
fn click(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>) {
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    mouse(cx, w, at, "up");
}

fn head_at(cx: &mut TestAppContext, w: AnyWindowHandle, id: u64) -> gpui_kit::Point<gpui_kit::Pixels> {
    drawn(cx, w, format!("block-head {id}")).center()
}

/// Whether block row `id` (a top-level block or a member) draws folded
/// in the pane acted on.
fn folded(cx: &mut TestAppContext, view: &Entity<AppView>, id: u64) -> bool {
    view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].shown().items();
        let s = v
            .panes
            .cur
            .disp
            .iter()
            .filter_map(|d| {
                let Kind::Block { at } = d.kind else { return None };
                items.get(d.item)?.blocks.as_ref()?.blocks.get(at as usize).cloned()
            })
            .find_map(|s| {
                if crate::blocks::row_id(&s.block.id) == id {
                    return Some(s);
                }
                s.members.iter().find(|m| crate::blocks::row_id(&m.block.id) == id).cloned()
            })
            .expect("drawn");
        v.panes.cur.block_folds.folded(id, &s)
    })
}

fn note(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<(u64, &'static str)> {
    view.read_with(cx, |v, _| v.panes.cur.lease.note)
}

fn waiting(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some())
}

/// A live turn putting `ops` and twelve notes under them, the first block
/// at the view's top (the list not following its tail).
fn put_mid(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ops: Value) {
    turn(cx, w, view);
    let mut all = ops.as_array().cloned().unwrap_or_default();
    let first = all[0]["id"].as_str().unwrap_or_default().to_string();
    all.extend((0..12).map(|k| json!({"op":"put","id":format!("under{k}"),"kind":"note","body":{"text":"under"}})));
    round(cx, w, view, &[("toolu_p", json!(all), true)]);
    show_block(cx, w, view, &first);
    wait(cx, w, view, 1000);
}

/// Split panes on, and the pane acted on split: a second pane on its
/// session (its store).
fn split(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        cx.notify();
    });
    settle(cx, w, 8);
    super::answer_tests::key(cx, w, "ctrl-\\");
    board(cx, w, view);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2, "two panes");
}

/// P2-5: under the lease, a note above the hovered block whose text change
/// only wraps it to another line at 320 px waits, as a note the user
/// opened past 6 lines whose text grows does: the lease reads a note's
/// height as the renderer draws it, in this pane.
#[gpui_kit::test]
fn a_wrap_only_change_above_the_pointer_waits_at_320(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 320. + 2. * crate::app::STRIP);
    let long = |n: usize| (0..n).map(|k| format!("word{k}")).collect::<Vec<_>>().join(" ");
    let put = json!([
        {"op":"put","id":"a","kind":"note","body":{"text":"A short line"}},
        {"op":"put","id":"o","kind":"note","body":{"text":long(80)}},
        {"op":"put","id":"b","kind":"status","body":{"text":"B","state":"working"}}
    ]);
    put_mid(cx, w, &view, put);
    // `o` opened past its 6 lines by the user.
    let o = id(cx, &view, "o");
    let more = drawn(cx, w, format!("block-more {o}")).center();
    click(cx, w, more);
    assert!(view.read_with(cx, |v, _| v.panes.cur.block_more.contains(&o)), "o opened");
    let b = id(cx, &view, "b");
    let y = |cx: &mut TestAppContext| f32::from(drawn(cx, w, format!("block {b}")).top());
    for (k, ops) in [
        // 56 characters on one line: one line at 60 a line, two at 320 px.
        json!([{"op":"patch","id":"a","set":{"text":"A line that is longer now, still under sixty characters"}}]),
        // The opened note grows.
        json!([{"op":"patch","id":"o","set":{"text":long(120)}}]),
    ]
    .into_iter()
    .enumerate()
    {
        let y0 = y(cx);
        let at_ = head_at(cx, w, b);
        mouse(cx, w, at_, "move");
        round(cx, w, &view, &[(if k == 0 { "toolu_w0" } else { "toolu_w1" }, ops, true)]);
        assert!(waiting(cx, &view), "case {k}: the board waits under the pointer");
        assert_eq!(y(cx), y0, "case {k}: B stays where it is");
        // The pointer leaves: it draws.
        mouse(cx, w, point(px(2.), px(500.)), "move");
        board(cx, w, &view);
        wait(cx, w, &view, 1000);
        assert!(!waiting(cx, &view));
    }
}

/// P2-6: `more` is an action like any other: replaced between down and
/// up, it does nothing and says so; moved just now, it does nothing once.
#[gpui_kit::test]
fn more_obeys_the_painted_target(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let long = (0..200).map(|k| format!("w{k}")).collect::<Vec<_>>().join(" ");
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
        {"op":"put","id":"n","kind":"note","body":{"text":long}}
    ]);
    put_mid(cx, w, &view, put);
    let n = id(cx, &view, "n");
    let opened = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.cur.block_more.contains(&n));
    // Replaced between down and up.
    let at = drawn(cx, w, format!("block-more {n}")).center();
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    round(cx, w, &view, &[("toolu_r", json!([{"op":"replace","id":"n","kind":"note","body":{"text":long.clone()}}]), true)]);
    mouse(cx, w, at, "up");
    assert!(!opened(cx), "replaced under the press: `more` did nothing");
    assert_eq!(note(cx, &view), Some((n, crate::lease::CHANGED)));
    mouse(cx, w, point(px(2.), px(500.)), "move");
    board(cx, w, &view);
    wait(cx, w, &view, 1000);
    // Moved just now by an agent op (the plan above it grew): nothing, once.
    motion_level(cx, &view, crate::motion::Level::Off);
    round(
        cx,
        w,
        &view,
        &[("toolu_g", json!([{"op":"patch","id":"a","append":{"steps":[{"id":"s2","label":"Two","state":"todo"}]}}]), true)],
    );
    wait(cx, w, &view, 300);
    let at = drawn(cx, w, format!("block-more {n}")).center();
    click(cx, w, at);
    assert!(!opened(cx), "300 ms after it moved: nothing");
    assert_eq!(note(cx, &view), Some((n, crate::lease::MOVED)));
    click(cx, w, at);
    assert!(opened(cx), "once: the next click opens it");
}

/// P2-6: a group member's header is its own target: a click folds the
/// member, never its group; the member replaced under a held press, the
/// release does nothing and the member says so.
#[gpui_kit::test]
fn a_member_is_its_own_target(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let why = show_block(cx, w, &view, "why");
    let at_ = head_at(cx, w, why);
    click(cx, w, at_);
    assert!(!folded(cx, &view, why), "the group open");
    wait(cx, w, &view, 1000);
    let rot = member(cx, &view, why, 1);
    let was = folded(cx, &view, rot);
    let at_ = head_at(cx, w, rot);
    click(cx, w, at_);
    assert!(!folded(cx, &view, why), "the group stays open");
    assert_ne!(folded(cx, &view, rot), was, "the member folded");
    let at_ = head_at(cx, w, rot);
    click(cx, w, at_);
    assert_eq!(folded(cx, &view, rot), was);
    // Replaced under a held press.
    let at = head_at(cx, w, rot);
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    turn(cx, w, &view);
    let metric = json!({"label":"rotations per 32 refreshes","value":"1.0","state":"ok"});
    round(cx, w, &view, &[("toolu_r", json!([{"op":"replace","id":"rot","kind":"metric","body":metric}]), true)]);
    mouse(cx, w, at, "up");
    assert!(!folded(cx, &view, why), "the group untouched");
    assert_eq!(folded(cx, &view, rot), was, "the replaced member untouched");
    assert_eq!(note(cx, &view), Some((rot, crate::lease::CHANGED)), "the member says so");
}

/// P2-7: a press released anywhere but on its target ends: over blank
/// transcript space, another pane, outside the window, or lost (a move
/// with no button down). Nothing folds, and what waited on it draws at the
/// lease's end.
#[gpui_kit::test]
fn a_press_released_elsewhere_ends(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1800.);
    split(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
        {"op":"put","id":"b","kind":"note","body":{"text":"B"}}
    ]);
    put_mid(cx, w, &view, put);
    let b = id(cx, &view, "b");
    let other = view.read_with(cx, |v, _| v.panes.get(panes[1]).map(|p| p.list.viewport_bounds()).expect("pane 2"));
    let list = view.read_with(cx, |v, _| v.panes.cur.list.viewport_bounds());
    let blank = point(list.left() + px(4.), list.bottom() - px(4.));
    let pressed = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.iter().any(|p| p.lease.pressed || p.lease.down.is_some()));
    for (k, (to, how)) in
        [(blank, "up"), (other.center(), "up"), (point(px(-40.), px(-40.)), "up"), (point(px(4000.), px(300.)), "up"), (blank, "move")]
            .into_iter()
            .enumerate()
    {
        let was = folded(cx, &view, b);
        let at = head_at(cx, w, b);
        mouse(cx, w, at, "move");
        mouse(cx, w, at, "down");
        assert!(pressed(cx), "case {k}: pressed");
        if how == "up" {
            mouse(cx, w, to, "drag");
        }
        mouse(cx, w, to, how);
        assert!(!pressed(cx), "case {k}: released at {to:?} ({how}): the press is over");
        assert_eq!(folded(cx, &view, b), was, "case {k}: nothing folded");
        wait(cx, w, &view, 1600);
    }
    // No immortal lease: a change that would move B waits under a moving
    // pointer, then draws at the lease's end.
    let at_ = head_at(cx, w, b);
    mouse(cx, w, at_, "move");
    let at_ = head_at(cx, w, b);
    mouse(cx, w, at_, "down");
    let grow = json!([{"op":"patch","id":"a","append":{"steps":[{"id":"s2","label":"Two","state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_g", grow, true)]);
    assert!(waiting(cx, &view), "held by the press");
    mouse(cx, w, blank, "drag");
    mouse(cx, w, blank, "up");
    wait(cx, w, &view, 1600);
    board(cx, w, &view);
    assert!(!waiting(cx, &view), "drawn at the lease's end");
}

/// P2-8: the user's own change right after an agent op never starts the
/// just-moved window: an agent tick, then 200 ms later the user's fold, or
/// `more`, or a scroll, moves the target: a click at once acts. A frame
/// that draws both an agent op and a user's change stays the agent's.
#[gpui_kit::test]
fn just_moved_is_the_agents_frame_not_the_clock(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let long = (0..200).map(|k| format!("w{k}")).collect::<Vec<_>>().join(" ");
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"},{"id":"s2","label":"Two","state":"todo"}]}},
        {"op":"put","id":"n","kind":"note","body":{"text":long}},
        {"op":"put","id":"b","kind":"note","body":{"text":"B"}},
        {"op":"put","id":"c","kind":"status","body":{"text":"C","state":"working"}}
    ]);
    put_mid(cx, w, &view, put);
    let (a, n, b) = (id(cx, &view, "a"), id(cx, &view, "n"), id(cx, &view, "b"));
    let tick = |cx: &mut TestAppContext, k: usize| {
        round(cx, w, &view, &[(&format!("toolu_t{k}"), json!([{"op":"patch","id":"c","set":{"text":format!("C {k}")}}]), true)]);
        wait(cx, w, &view, 200);
    };
    let acts = |cx: &mut TestAppContext, why: &str| {
        let y0 = f32::from(drawn(cx, w, format!("block {b}")).top());
        let was = folded(cx, &view, b);
        let at_ = head_at(cx, w, b);
        click(cx, w, at_);
        assert_ne!(folded(cx, &view, b), was, "{why}: the click acts at once (B at {y0})");
        assert_eq!(note(cx, &view), None, "{why}: no note");
        let at_ = head_at(cx, w, b);
        click(cx, w, at_);
        wait(cx, w, &view, 700);
    };
    // The user's fold.
    tick(cx, 1);
    let y0 = f32::from(drawn(cx, w, format!("block {b}")).top());
    let at_ = head_at(cx, w, a);
    click(cx, w, at_);
    assert!(f32::from(drawn(cx, w, format!("block {b}")).top()) < y0, "the fold moved B up");
    acts(cx, "a fold");
    let at_ = head_at(cx, w, a);
    click(cx, w, at_);
    wait(cx, w, &view, 700);
    // The user's `more`.
    tick(cx, 2);
    let at_ = drawn(cx, w, format!("block-more {n}")).center();
    click(cx, w, at_);
    acts(cx, "more");
    let at_ = drawn(cx, w, format!("block-more {n}")).center();
    click(cx, w, at_);
    wait(cx, w, &view, 700);
    // The user's scroll.
    tick(cx, 3);
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_by(px(-40.));
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    acts(cx, "a scroll");
    // One frame drawing an agent op (a release) and the user's fold: the
    // agent's (the safe side).
    motion_level(cx, &view, crate::motion::Level::Off);
    let at_ = head_at(cx, w, b);
    mouse(cx, w, at_, "move");
    round(
        cx,
        w,
        &view,
        &[("toolu_g", json!([{"op":"patch","id":"a","append":{"steps":[{"id":"s3","label":"Three","state":"todo"}]}}]), true)],
    );
    assert!(waiting(cx, &view));
    let pane = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, cx| {
        v.lease_over(pane, false, cx);
        let (ix, s) = v.block_shown(n).expect("n drawn");
        v.panes.cur.block_folds.toggle(n, &s);
        v.panes.cur.list.remeasure_items(ix..ix + 1);
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    wait(cx, w, &view, 200);
    let was = folded(cx, &view, b);
    let at_ = head_at(cx, w, b);
    click(cx, w, at_);
    assert_eq!(folded(cx, &view, b), was, "both in one frame: the agent's, the click does nothing");
    assert_eq!(note(cx, &view), Some((b, crate::lease::MOVED)));
}

/// P2-9: an edit preview's lines are drawn as D1 draws agent text: a
/// successful edit, write or multi-edit whose text holds bidi controls,
/// isolates, control characters or invisibles reaches the row without
/// them (the requested edit, and the ledger, keep their bytes).
#[test]
fn edit_preview_lines_are_sanitized() {
    let evil = "let s = \"\u{202E}lave\u{202C}\"; \u{2066}x\u{2069} \u{200B}zw\u{0007}bell\u{001B}[31m";
    let tool = |id: &str, name: &str, input: Value| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]});
    let ok = |id: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]});
    let msgs = vec![
        json!({"role": "user", "content": "go"}),
        tool("t1", "edit", json!({"path": "/w/a.rs", "old_string": "x", "new_string": evil})),
        ok("t1"),
        tool("t2", "write", json!({"path": "/w/a.rs", "content": format!("{evil}\nline two")})),
        ok("t2"),
        tool("t3", "multi_edit", json!({"path": "/w/a.rs", "edits": [{"old_string": "y", "new_string": evil}]})),
        ok("t3"),
    ];
    let mut t = synaps_client::Transcript::new();
    t.apply_history(&msgs);
    let mut ledger = crate::files::Ledger::default();
    ledger.reset(&msgs, None, false);
    let (groups, _) = crate::blocks::edit_preview(t.items(), &ledger, "/w/a.rs");
    assert!(groups.len() >= 2, "the edits drawn: {}", groups.len());
    let bad = |c: char| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}') || (c.is_control() && c != '\t');
    for g in &groups {
        for l in &g.lines {
            assert!(!l.text.chars().any(bad), "a raw character reached the row: {:?}", l.text);
        }
    }
    assert!(groups.iter().flat_map(|g| &g.lines).any(|l| l.text.contains("lave")), "the text itself is drawn");
}

/// P2-10: a file's header holds its path, `:line` and note; a title comes
/// before them and never hides the file's name and line. At 320 px and
/// wide.
#[gpui_kit::test]
fn a_file_header_shows_its_path_line_and_note(cx: &mut TestAppContext) {
    for width in [320. + 2. * crate::app::STRIP, 1500.] {
        let (w, view) = blocks(cx, width);
        let wide = width > 600.;
        let put = json!([
            {"op":"put","id":"f1","kind":"file","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}},
            {"op":"put","id":"f2","kind":"file","title":"The token store, where rotate() takes its lock per session","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}},
            {"op":"put","id":"f3","kind":"file","body":{"path":"crates/relay/src/session.rs","state":"reading"}},
            {"op":"put","id":"f4","kind":"file","title":"Session","body":{"path":"crates/relay/src/session.rs","state":"done"}}
        ]);
        put_mid(cx, w, &view, put);
        for (name, shown, has_note, titled) in [
            ("f1", "token_store.rs:118", true, false),
            ("f2", "token_store.rs:118", true, true),
            ("f3", "session.rs", false, false),
            ("f4", "session.rs", false, true),
        ] {
            let f = id(cx, &view, name);
            let block = drawn(cx, w, format!("block {f}"));
            let path = drawn(cx, w, format!("block-path {f} {shown}"));
            assert!(path.left() >= block.left() && path.right() <= block.right() + px(0.5), "{width} {name}: the path inside its block");
            // The name and line whole: as wide as their characters.
            let m = view.read_with(cx, |v, _| v.panes.cur.metrics_last.get().expect("drawn"));
            let min = shown.chars().count() as f32 * m.mono * 0.5;
            assert!(
                f32::from(path.size.width) >= min,
                "{width} {name}: `{shown}` whole ({} px, {min} at least)",
                f32::from(path.size.width)
            );
            assert!((f32::from(block.size.height) - crate::blocks::Geo::of(&m).head).abs() < 0.5, "{width} {name}: one line");
            if titled {
                let title = drawn(cx, w, format!("block-title {f}"));
                assert!(title.right() <= path.left() + px(0.5), "{width} {name}: the title before the path");
            }
            if has_note && wide {
                let n = drawn(cx, w, format!("block-file-note {f} TokenStore::rotate"));
                assert!(f32::from(n.size.width) > 40. && n.right() <= block.right(), "{width} {name}: the note");
            }
        }
    }
}

/// P2-11: a metric's legal value and unit (16 characters each, wide ones
/// too) and `was` stay inside a 320 px pane, at the largest text step too.
#[gpui_kit::test]
fn a_long_metric_fits_a_320_px_pane(cx: &mut TestAppContext) {
    for (width, text) in [(320. + 2. * crate::app::STRIP, 16), (320. + 2. * crate::app::STRIP, 22), (1500., 22)] {
        let (w, view) = blocks_at(cx, width, text);
        let wide16 = "ＷＷＷＷＷＷＷＷＷＷＷＷＷＷＷＷ";
        let put = json!([
            {"op":"put","id":"m1","kind":"metric","body":{"label":"rotations per 32 refreshes","value":wide16,"unit":"ｍｍｍｍｍｍｍｍｍｍｍｍｍｍｍｍ","was":wide16,"state":"bad"}},
            {"op":"put","id":"m2","kind":"metric","body":{"label":"latency","value":"1234567890.12345","unit":"milliseconds/op","was":"999999999.999999","series":[1,2,3],"state":"warn"}}
        ]);
        put_mid(cx, w, &view, put);
        let list = view.read_with(cx, |v, _| v.panes.cur.list.viewport_bounds());
        for name in ["m1", "m2"] {
            let m = id(cx, &view, name);
            let block = drawn(cx, w, format!("block {m}"));
            assert!(block.right() <= list.right() + px(0.5), "{width}/{text} {name}: the block inside the pane ({block:?} in {list:?})");
            let value = drawn(cx, w, format!("block-value {m}"));
            assert!(
                value.left() >= block.left() && value.right() <= block.right() + px(0.5),
                "{width}/{text} {name}: the value inside its block ({value:?} in {block:?})"
            );
            // One line, in the metric's 44 px, the block its structure's
            // height (§6.2): nothing wraps over the label or the series.
            let g = view.read_with(cx, |v, _| crate::blocks::Geo::of(&v.panes.cur.metrics_last.get().expect("drawn")));
            let metric = drawn(cx, w, format!("block-metric {m}"));
            assert!(metric.right() <= block.right() + px(0.5), "{width}/{text} {name}: the body inside");
            assert!(
                value.top() >= metric.top() - px(0.5) && value.bottom() <= metric.top() + px(g.metric + 0.5),
                "{width}/{text} {name}: the value on its line ({value:?} in {metric:?})"
            );
            let series = if name == "m2" { g.series } else { 0. };
            let pad = view.read_with(cx, |v, _| (6. * v.panes.cur.metrics_last.get().expect("drawn").r).round());
            assert!(
                (f32::from(block.size.height) - (g.head + g.metric + series + pad)).abs() < 0.5,
                "{width}/{text} {name}: the block's height is its structure's ({block:?})"
            );
        }
    }
}

/// The plan's chip says `op skipped` (an op, not the plan), and its
/// tooltip says why it is there.
#[gpui_kit::test]
fn the_skipped_chip_is_an_ops_and_says_why(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let plan = show_block(cx, w, &view, "plan");
    assert!(has(cx, w, format!("block-skipped-says {plan} op skipped")), "`op skipped`");
    let chip = drawn(cx, w, format!("block-skipped {plan}")).center();
    mouse(cx, w, chip, "move");
    wait(cx, w, &view, 800);
    mouse(cx, w, chip, "move");
    assert!(has(cx, w, format!("block-skipped-tip {plan}")), "its reason in a tooltip");
}

/// Decision #5: the lease's note on a block reads whole inside its header
/// at 320 px, at the default and the largest text step: the title keeps
/// its room, and nothing moves as it comes.
#[gpui_kit::test]
fn the_lease_note_reads_whole_at_320_px(cx: &mut TestAppContext) {
    for (width, text) in [(320. + 2. * crate::app::STRIP, 16), (320. + 2. * crate::app::STRIP, 22), (1500., 16)] {
        let (w, view) = blocks_at(cx, width, text);
        let put = json!([
            {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
            {"op":"put","id":"b","kind":"status","title":"The relay's refresh","body":{"text":"B","state":"working"}}
        ]);
        put_mid(cx, w, &view, put);
        let b = id(cx, &view, "b");
        let before = drawn(cx, w, format!("block {b}"));
        // Replaced under the press: the note.
        let at = head_at(cx, w, b);
        mouse(cx, w, at, "move");
        mouse(cx, w, at, "down");
        round(
            cx,
            w,
            &view,
            &[(
                "toolu_r",
                json!([{"op":"replace","id":"b","kind":"status","title":"The relay's refresh","body":{"text":"B again","state":"working"}}]),
                true,
            )],
        );
        mouse(cx, w, at, "up");
        assert_eq!(note(cx, &view), Some((b, crate::lease::CHANGED)));
        let block = drawn(cx, w, format!("block {b}"));
        let head = drawn(cx, w, format!("block-head {b}"));
        let says = drawn(cx, w, format!("block-says {b} {}", crate::lease::CHANGED));
        assert!(
            says.left() >= block.left() && says.right() <= block.right() + px(0.5),
            "{width}/{text}: the note inside its block ({says:?} in {block:?})"
        );
        assert!(
            says.top() >= head.top() - px(0.5) && says.bottom() <= head.bottom() + px(0.5),
            "{width}/{text}: whole in the header ({says:?} in {head:?})"
        );
        let title = drawn(cx, w, format!("block-title {b}"));
        assert!(
            f32::from(title.size.width) > 20. && says.left() >= title.left() + px(20.),
            "{width}/{text}: the title keeps room ({title:?}, {says:?})"
        );
        assert_eq!(block.size.height, before.size.height, "{width}/{text}: nothing moved");
    }
}
