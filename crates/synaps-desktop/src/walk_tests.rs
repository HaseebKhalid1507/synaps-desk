//! D7's window tests (synaps-cards `docs/blocks.md` v1.3 §3.8, §8.2's D7
//! row): the block walk, its revisions, the op log and its archive, the
//! hover actions and local undo, through the window's own key dispatch
//! and pointer, as the hand has them.
//!
//! §8.2's list: activation on paint; bare presses only; `Esc` to the
//! composer, never denying a prompt; no reflow on hover; keys act on
//! painted targets, checked before the lease releases, and never meet the
//! just-moved rule. And key precedence against the composer, an agent
//! card (Ctrl+J), the permission prompt, split panes and an IME.

use std::cell::RefCell;
use std::rc::Rc;

use cards_spec::block::{Ref, Slot};
use cards_spec::board::Place;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, point, px};
use serde_json::{Value, json};

use super::answer_tests::{ask_live, attachment, has, heard, llm};
use super::blocks_ui_tests::{click, drawn, mouse, open_session, round, split, turn};
use super::keys_tests::{composer_text, down, entered, keys_at, press, prompt, prompt_pending, repeat};
use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::walk::{Aim, Asked, At, BlockDesires, Wish};

type W = (AnyWindowHandle, Entity<AppView>);

fn row(name: &str) -> u64 {
    crate::blocks::row_id(&Ref::parse(name).expect("an id"))
}

/// The blocks demo in a window `w` px wide, the composer with the keys,
/// and a live turn's three notes `a`, `b`, `c` at the transcript's end.
fn walked_at(cx: &mut TestAppContext, w: f32) -> W {
    let (win, view, _events) = open_at(cx, (w, 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, win, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, win, &view);
    turn(cx, win, &view);
    let notes = json!([
        {"op":"put","id":"a","kind":"note","body":{"text":"A first"}},
        {"op":"put","id":"b","kind":"note","body":{"text":"B first"}},
        {"op":"put","id":"c","kind":"note","body":{"text":"C first"}}
    ]);
    round(cx, win, &view, &[("toolu_w0", notes, true)]);
    tail(cx, win, &view);
    (win, view)
}

fn walked(cx: &mut TestAppContext) -> W {
    walked_at(cx, 1500.)
}

/// The list at its end, settled.
fn tail(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, view, 1000);
}

/// Where the walk of the pane acted on is.
fn at(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<At> {
    view.read_with(cx, |v, _| v.panes.cur.walk.walk.as_ref().map(|w| w.at))
}

fn ring(cx: &mut TestAppContext, w: AnyWindowHandle, row: u64) -> bool {
    has(cx, w, format!("walk-ring {row}"))
}

/// The walk's status line drawn: its text.
fn status(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| {
        v.panes.cur.walk.walk.as_ref()?;
        Some(match v.panes.cur.walk.walk.as_ref().and_then(|w| w.rev.as_ref()) {
            Some(_) => "rev".into(),
            None => crate::walk::status_keys(v.block_desires().wired(), cfg!(target_os = "macos")),
        })
    })
}

/// The walk's keys are the walk's: neither the composer's nor the window's.
fn walking(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> bool {
    cx.update_window(w, |_, window, cx| view.read(cx).panes.cur.walk.holds(window)).expect("the window")
}

/// The pane's note on block row `row`.
fn note(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<(u64, &'static str)> {
    view.read_with(cx, |v, _| v.panes.cur.lease.note)
}

fn waiting(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some())
}

/// Whether top-level block `name` draws folded in the pane acted on.
fn folded(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> bool {
    let r = row(name);
    view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(r).expect("drawn");
        v.panes.cur.block_folds.folded(r, &s)
    })
}

/// Block `name`'s incarnation on the board drawn.
fn tag(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> synaps_client::blocks::Tag {
    let r = row(name);
    view.read_with(cx, |v, _| v.block_shown(r).expect("drawn").1.block.tag)
}

fn head(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    drawn(cx, w, format!("block-head {}", row(name))).center()
}

/// The pointer away from the list (over the window's chrome).
fn away(cx: &mut TestAppContext, w: AnyWindowHandle) {
    mouse(cx, w, point(px(8.), px(8.)), "move");
}

/// Ctrl+G, and the walk's first frames: its ring painted.
fn enter(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    press(cx, w, "ctrl-g");
    wait(cx, w, view, 300);
    assert!(walking(cx, w, view), "the walk has the keys");
}

/// One agent round of `ops` on the live turn.
fn ops(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: Value) {
    round(cx, w, view, &[(tool, ops, true)]);
}

// ── entering, walking, leaving ────────────────────────────────────────

/// Ctrl+G enters the walk on the newest block in view; `k`/`j` and the
/// arrows walk the pane's blocks in reading order, the ring and the
/// status line with it; Esc returns the keys to the composer.
#[gpui_kit::test]
fn ctrl_g_walks_from_the_newest_block_in_view_and_esc_returns_to_the_composer(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    assert_eq!(at(cx, &view), None);
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))), "the newest in view");
    assert!(ring(cx, w, row("c")));
    assert_eq!(keys_at(cx, w, &view), (false, false), "neither the composer's nor the window's");
    let s = status(cx, &view).expect("the status line");
    assert!(has(cx, w, format!("walk-status {s}")), "the status line drawn: {s}");
    assert!(s.contains("j k walk") && s.contains("esc composer") && s.contains("x dismiss · u show · p pin · m slot"), "D6b's, wired: {s}");
    press(cx, w, "k");
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))));
    wait(cx, w, &view, 300);
    assert!(ring(cx, w, row("b")) && !ring(cx, w, row("c")), "the ring moved with it");
    press(cx, w, "up");
    assert_eq!(at(cx, &view), Some(At::Block(row("a"))));
    press(cx, w, "down");
    press(cx, w, "j");
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
    press(cx, w, "escape");
    assert_eq!(at(cx, &view), None);
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer, never the window");
    assert!(!ring(cx, w, row("c")) && status(cx, &view).is_none());
    assert_eq!(composer_text(cx, &view), "", "nothing typed");
}

/// The rail demo (its blocks all the rail's) in a 1280 × 2400 window,
/// the composer with the keys.
fn rail(cx: &mut TestAppContext) -> W {
    let (w, view, _events) = open_at(cx, (1280., 2400.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::RAIL, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    (w, view)
}

/// A block walked to from far off (the op log's jump to the session's
/// first block: its row not laid out, which GPUI's reveal anchors at the
/// list's edge, under the sticky band) is placed clear of the band once
/// laid out. The next one, in view, moves nothing.
#[gpui_kit::test]
fn a_block_walked_to_comes_into_view_clear_of_the_band(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let first = view.read_with(cx, |v, _| v.walk_order()[0].row());
    assert_eq!(first, row("now"), "the session's first block");
    let ix = view.read_with(cx, |v, _| v.panes.cur.disp.iter().position(|d| d.id == first)).expect("its row");
    assert!(view.read_with(cx, |v, _| v.panes.cur.list.bounds_for_item(ix)).is_none(), "far off: not laid out");
    press(cx, w, "ctrl-o");
    let lines = oplog_lines(cx, &view);
    let k = lines.iter().position(|l| l.split_whitespace().nth(3) == Some("now")).expect("now's line");
    let at_ = drawn(cx, w, format!("oplog-line {k} {}", lines[k])).center();
    click(cx, w, at_);
    wait(cx, w, &view, 300);
    assert_eq!(at(cx, &view), Some(At::Block(first)));
    let row_top = view.read_with(cx, |v, _| v.panes.cur.list.bounds_for_item(ix)).expect("laid out").top();
    let (top, inset, bottom) = cx.update(|cx| {
        let v = view.read(cx);
        let (t, b) = v.list_view();
        (t, v.sticky_inset(cx), b)
    });
    assert!(f32::from(row_top) >= top + inset + 0.5, "clear of the band: its row at {row_top:?}, the band to {}", top + inset);
    assert!(f32::from(row_top) < bottom, "in view");
    assert!(ring(cx, w, first), "its ring painted");
    // The next, in view: nothing moves.
    let y = f32::from(drawn(cx, w, format!("block {first}")).top());
    press(cx, w, "j");
    wait(cx, w, &view, 300);
    assert_eq!(f32::from(drawn(cx, w, format!("block {first}")).top()), y, "the next in view: nothing moved");
}

/// Activation on paint: a key the walk gets before its ring was painted
/// in view (its fade begun) acts on nothing: the walk ends and the key is
/// typed into the composer. (GPUI draws a dirty window before it
/// dispatches a key, so the walk's first frame comes first: a ring it
/// didn't paint in view is the case; here the key is handed to the walk
/// as Ctrl+G's handler returns, no frame between.) Painted, `j` walks.
#[gpui_kit::test]
fn a_key_before_the_ring_was_painted_goes_to_the_composer(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let j = super::keys_tests::keystroke(cx, "j");
    let v = view.clone();
    cx.update_window(w, |_, window, cx| {
        v.update(cx, |v, cx| {
            assert!(v.walk_enter(window, cx), "entered");
            let ev = gpui_kit::KeyDownEvent { keystroke: j, is_held: false, prefer_character_input: false };
            v.walk_key(&ev, window, cx);
        });
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, w, 8);
    assert_eq!(at(cx, &view), None, "the walk ended");
    assert_eq!(composer_text(cx, &view), "j", "the key went to the composer");
    assert_eq!(keys_at(cx, w, &view), (true, false));
    // Painted, it walks.
    enter(cx, w, &view);
    press(cx, w, "k");
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))), "painted: k walks");
    assert_eq!(composer_text(cx, &view), "j");
}

/// Bare presses only: a repeat, a key held from before Ctrl+G, and a key
/// with Alt do nothing; a shifted letter is not the walk's, and is typed
/// into the composer, the walk ended.
#[gpui_kit::test]
fn only_fresh_bare_presses_act(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    press(cx, w, "k");
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))));
    repeat(cx, w, "k");
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))), "a repeat does nothing");
    press(cx, w, "alt-k");
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))), "a chord does nothing");
    press(cx, w, "alt-z");
    assert!(!folded(cx, &view, "b"));
    assert!(walking(cx, w, &view));
    // A key held from the composer, its repeats reaching the walk.
    press(cx, w, "escape");
    down(cx, w, "j");
    assert_eq!(composer_text(cx, &view), "j");
    press(cx, w, "ctrl-g");
    wait(cx, w, &view, 300);
    let was = at(cx, &view);
    repeat(cx, w, "j");
    assert_eq!(at(cx, &view), was, "held from before: nothing");
    super::keys_tests::up(cx, w, "j");
    assert_eq!(composer_text(cx, &view), "j", "nor typed");
    // A shifted letter: typed, the walk over.
    press(cx, w, "shift-z");
    assert_eq!(at(cx, &view), None);
    assert_eq!(composer_text(cx, &view), "jZ");
    assert!(!folded(cx, &view, "c") && !folded(cx, &view, "b"), "nothing folded");
}

/// Esc never denies a prompt: Esc in the walk goes to the composer, never
/// the window; the composer's Esc then hands the prompt its keys and
/// denies nothing. A letter in the walk is typed, never an answer.
#[gpui_kit::test]
fn esc_from_the_walk_never_denies_a_prompt(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    prompt(cx, w, &view);
    press(cx, w, "ctrl-g");
    wait(cx, w, &view, 300);
    assert!(walking(cx, w, &view), "Ctrl+G with a prompt up: the walk");
    press(cx, w, "j");
    prompt_pending(cx, &view, "j in the walk");
    press(cx, w, "escape");
    assert_eq!(at(cx, &view), None);
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer, not the window");
    prompt_pending(cx, &view, "one Esc");
    press(cx, w, "escape");
    prompt_pending(cx, &view, "a second Esc");
    // A letter that answers a prompt on the window's keys: in the walk,
    // typed into the composer.
    enter(cx, w, &view);
    press(cx, w, "n");
    assert_eq!(at(cx, &view), None);
    assert_eq!(composer_text(cx, &view), "n");
    prompt_pending(cx, &view, "n in the walk");
}

/// Key precedence with the composer: its letters are its own (no walk);
/// Ctrl+G from it walks; Ctrl+G again in the walk stays; a letter not the
/// walk's goes back to it, typed.
#[gpui_kit::test]
fn the_composers_keys_are_its_own_until_ctrl_g(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    press(cx, w, "j");
    press(cx, w, "z");
    assert_eq!(composer_text(cx, &view), "jz");
    assert_eq!(at(cx, &view), None);
    assert!(!folded(cx, &view, "c"));
    enter(cx, w, &view);
    press(cx, w, "ctrl-g");
    assert!(walking(cx, w, &view) && at(cx, &view) == Some(At::Block(row("c"))), "again: still walking");
    press(cx, w, "q");
    assert_eq!(at(cx, &view), None);
    assert_eq!(composer_text(cx, &view), "jzq");
    assert_eq!(keys_at(cx, w, &view), (true, false));
}

/// An IME composition owns its keys: Ctrl+G and Ctrl+O mid-composition do
/// nothing; committed, Ctrl+G walks.
#[gpui_kit::test]
fn an_ime_composition_keeps_ctrl_g_and_ctrl_o(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler as _;
    let (w, view) = walked(cx);
    let c = view.read_with(cx, |v, _| v.panes.cur.composer.clone());
    cx.update_window(w, |_, window, cx| c.update(cx, |s, cx| s.replace_and_mark_text_in_range(None, "か", None, window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    press(cx, w, "ctrl-g");
    assert_eq!(at(cx, &view), None, "mid-composition: no walk");
    press(cx, w, "ctrl-o");
    assert!(view.read_with(cx, |v, _| v.oplog.is_none()), "nor the op log");
    cx.update_window(w, |_, window, cx| c.update(cx, |s, cx| s.replace_text_in_range(None, "か", window, cx))).expect("the window");
    settle(cx, w, 8);
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
}

/// Ctrl+J from the walk enters the agent card (the walk ends with its
/// keys); Ctrl+G from the card walks (the card left); Esc, the composer.
#[gpui_kit::test]
fn ctrl_j_and_ctrl_g_hand_the_keys_between_a_card_and_the_walk(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    ask_live(cx, w, &view, "toolu_wc", "c7walk0", input);
    tail(cx, w, &view);
    let (card, _) = super::answer_tests::card(cx, &view, "Which branch?");
    super::keys_tests::at_rest(cx, w, &view);
    enter(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, card), "Ctrl+J: the card");
    assert_eq!(at(cx, &view), None, "the walk ended with its keys");
    press(cx, w, "ctrl-g");
    wait(cx, w, &view, 300);
    assert!(walking(cx, w, &view) && !entered(cx, w, card), "Ctrl+G: the walk, the card left");
    press(cx, w, "1");
    assert!(super::answer_tests::sends(cx, &view).is_empty(), "a card's key in the walk answers nothing");
    assert_eq!(composer_text(cx, &view), "1");
}

/// Split panes: Ctrl+G walks the focused pane only, its ring in that
/// pane; the keys going to the other pane end it.
#[gpui_kit::test]
fn split_panes_walk_the_focused_pane_only(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    split(cx, w, &view);
    tail(cx, w, &view);
    enter(cx, w, &view);
    let walks = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.iter().map(|p| p.walk.walk.is_some()).collect::<Vec<_>>());
    let focused = view.read_with(cx, |v, _| v.panes.focused);
    let ids = view.read_with(cx, |v, _| v.panes.iter().map(|p| p.id).collect::<Vec<_>>());
    assert_eq!(walks(cx).iter().filter(|x| **x).count(), 1, "one pane walks");
    let k = ids.iter().position(|i| *i == focused).expect("the focused pane");
    assert!(walks(cx)[k], "the focused one");
    assert!(ring(cx, w, row("c")));
    // The other pane's composer takes the keys: the walk is over.
    let other = ids.iter().position(|i| *i != focused).expect("another pane");
    let c = view.read_with(cx, |v, _| v.panes.iter().nth(other).map(|p| p.composer.clone()).expect("its composer"));
    cx.update_window(w, |_, window, cx| c.update(cx, |s, cx| s.focus(window, cx))).expect("the window");
    settle(cx, w, 8);
    wait(cx, w, &view, 100);
    assert_eq!(walks(cx), vec![false, false], "the keys went: no walk");
    assert!(!ring(cx, w, row("c")));
}

// ── painted targets, the lease, just moved ────────────────────────────

/// Keys act on the painted target, checked against the bound board before
/// the lease releases: the walked block replaced under the pointer (the
/// board waiting), `z` does nothing and says why; the release draws the
/// new one, and once its ring is painted `z` folds it.
#[gpui_kit::test]
fn a_key_acts_on_the_painted_target_checked_before_the_lease_releases(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    press(cx, w, "k");
    wait(cx, w, &view, 300);
    let t0 = tag(cx, &view, "b");
    let at_ = head(cx, w, "b");
    mouse(cx, w, at_, "move");
    ops(cx, w, &view, "toolu_wr", json!([{"op":"replace","id":"b","kind":"note","body":{"text":"B again"}}]));
    assert!(waiting(cx, &view), "the board waits under the pointer");
    press(cx, w, "z");
    assert!(!folded(cx, &view, "b"), "the painted target is gone: nothing");
    assert_eq!(note(cx, &view), Some((row("b"), crate::lease::CHANGED)), "and why");
    assert!(!waiting(cx, &view), "the key released the lease");
    assert_ne!(tag(cx, &view, "b"), t0, "the new incarnation drawn");
    wait(cx, w, &view, 300);
    press(cx, w, "z");
    assert!(folded(cx, &view, "b"), "painted again: it folds");
    assert_eq!(note(cx, &view), None);
}

/// A key names its target by identity: an agent op that just moved the
/// walked block never blocks it (a click on a block it moved does
/// nothing, once).
#[gpui_kit::test]
fn a_key_never_meets_the_just_moved_rule(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    press(cx, w, "k");
    wait(cx, w, &view, 300);
    away(cx, w);
    let long = (0..120).map(|k| format!("w{k}")).collect::<Vec<_>>().join(" ");
    let y0 = f32::from(drawn(cx, w, format!("block {}", row("b"))).top());
    // At the tail, c growing moves what is above it up.
    ops(cx, w, &view, "toolu_wm", json!([{"op":"patch","id":"c","set":{"text":long}}]));
    wait(cx, w, &view, 100);
    let y1 = f32::from(drawn(cx, w, format!("block {}", row("b"))).top());
    assert!((y1 - y0).abs() > 4., "the agent's op moved b: {y0} → {y1}");
    press(cx, w, "z");
    assert!(folded(cx, &view, "b"), "the key acts at once");
    assert_eq!(note(cx, &view), None, "no note");
    // The rule was live: a click on a, which it moved too, does nothing.
    let at_ = head(cx, w, "a");
    click(cx, w, at_);
    assert!(!folded(cx, &view, "a"), "a click on what just moved does nothing");
    assert_eq!(note(cx, &view), Some((row("a"), crate::lease::MOVED)));
}

// ── hover actions ─────────────────────────────────────────────────────

/// The pointer over a header shows its hover actions over the header's
/// right end, on its ground: nothing reflows (the block, its header, its
/// title and the next block keep their bounds). `fold` acts as the
/// chevron does; `rev` shows only with a past revision.
#[gpui_kit::test]
fn hover_actions_draw_over_the_header_and_nothing_reflows(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_wh", json!([{"op":"patch","id":"b","set":{"text":"B second"}}]));
    tail(cx, w, &view);
    let (b, c) = (row("b"), row("c"));
    let sels = [format!("block {b}"), format!("block-head {b}"), format!("block-title {b}"), format!("block {c}")];
    let before: Vec<_> = sels.iter().map(|s| drawn(cx, w, s.clone())).collect();
    let at_ = head(cx, w, "b");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("hover-acts {b}")), "the hover actions");
    let after: Vec<_> = sels.iter().map(|s| drawn(cx, w, s.clone())).collect();
    assert_eq!(before, after, "nothing reflows");
    let acts = drawn(cx, w, format!("hover-acts {b}"));
    let h = drawn(cx, w, format!("block-head {b}"));
    assert!(acts.right() <= h.right() + px(0.5) && acts.top() >= h.top() && acts.bottom() <= h.bottom(), "over the header's end");
    assert!(has(cx, w, format!("hover-act {b} rev")) && has(cx, w, format!("hover-act {b} fold")));
    assert!(
        has(cx, w, format!("hover-act {b} ×")) && has(cx, w, format!("hover-act {b} rail")),
        "D6b's: a transcript block's pin and × (its tombstone draws)"
    );
    // c has one revision: no rev.
    let at_ = head(cx, w, "c");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    assert!(!has(cx, w, format!("hover-acts {b}")) && has(cx, w, format!("hover-acts {c}")));
    assert!(!has(cx, w, format!("hover-act {c} rev")));
    // `fold` folds, as a click on its chevron would; Ctrl+Z in the walk
    // undoes it.
    let f = drawn(cx, w, format!("hover-act {c} fold")).center();
    click(cx, w, f);
    assert!(folded(cx, &view, "c"), "fold acts");
    away(cx, w);
    wait(cx, w, &view, 300);
    assert!(!has(cx, w, format!("hover-acts {c}")), "gone with the pointer");
    enter(cx, w, &view);
    press(cx, w, "ctrl-z");
    assert!(!folded(cx, &view, "c"), "undone");
}

/// A hover action's press is a painted target, as the chevron's: the
/// block replaced between its down and its up, nothing acts, and the
/// note says why.
#[gpui_kit::test]
fn a_hover_action_obeys_the_painted_target(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let at_ = head(cx, w, "b");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    let f = drawn(cx, w, format!("hover-act {} fold", row("b"))).center();
    mouse(cx, w, f, "move");
    mouse(cx, w, f, "down");
    ops(cx, w, &view, "toolu_wp", json!([{"op":"replace","id":"b","kind":"note","body":{"text":"B again"}}]));
    mouse(cx, w, f, "up");
    assert!(!folded(cx, &view, "b"), "replaced under the press: nothing");
    assert_eq!(note(cx, &view), Some((row("b"), crate::lease::CHANGED)));
}

// ── revisions ─────────────────────────────────────────────────────────

/// `[` shows the walked block's previous revision, drawn from the re-fold
/// (its label in place of `rN`, its body then); `[` again the one before;
/// `]` forward, and from the newest past one the latest. An agent op
/// meanwhile never yanks it: the label says `rN new`. Esc goes back to the
/// latest first, then to the composer; the scratch goes with the view.
#[gpui_kit::test]
fn revisions_step_back_and_an_op_meanwhile_never_yanks_the_view(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_v2", json!([{"op":"patch","id":"b","set":{"text":"B second"}}]));
    ops(cx, w, &view, "toolu_v3", json!([{"op":"patch","id":"b","set":{"text":"B third"}}]));
    tail(cx, w, &view);
    let b = row("b");
    let past = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.walk_past(b).map(|p| {
                let text = p.ready().and_then(|s| match &s.block.body {
                    synaps_client::blocks::Body::Note(n) => Some(n.text.to_string()),
                    _ => None,
                });
                (p.label, p.newer, text)
            })
        })
    };
    enter(cx, w, &view);
    press(cx, w, "k");
    wait(cx, w, &view, 300);
    assert_eq!(past(cx), None, "the latest");
    press(cx, w, "[");
    wait(cx, w, &view, 300);
    let (label, newer, text) = past(cx).expect("a past revision");
    assert!(label.starts_with("r2 of 3"), "{label}");
    assert_eq!((newer, text.as_deref()), (None, Some("B second")), "its body then");
    assert!(has(cx, w, format!("block-rev {b} {label}")), "its label drawn in place of rN");
    press(cx, w, "[");
    wait(cx, w, &view, 300);
    let (label, _, text) = past(cx).expect("r1");
    assert!(label.starts_with("r1 of 3"), "{label}");
    assert_eq!(text.as_deref(), Some("B first"));
    press(cx, w, "[");
    assert!(past(cx).expect("still r1").0.starts_with("r1 of 3"), "nothing older");
    // An agent op meanwhile: the view stays.
    ops(cx, w, &view, "toolu_v4", json!([{"op":"patch","id":"b","set":{"text":"B fourth"}}]));
    wait(cx, w, &view, 300);
    let (label, newer, text) = past(cx).expect("still r1");
    assert!(label.starts_with("r1 of 3"), "no yanking: {label}");
    assert_eq!((newer, text.as_deref()), (Some(4), Some("B first")));
    assert!(has(cx, w, format!("block-rev {b} {label} · r4 new")), "a dot says so");
    press(cx, w, "]");
    wait(cx, w, &view, 300);
    assert!(past(cx).expect("r2").0.starts_with("r2 of 3"));
    press(cx, w, "escape");
    assert_eq!(past(cx), None, "Esc: the latest first");
    assert!(walking(cx, w, &view));
    wait(cx, w, &view, 300);
    assert!(view.read_with(cx, |v, _| v.refolds.done.is_none() && !v.panes.cur.walk.refolding), "the scratch went with the view");
    press(cx, w, "escape");
    assert_eq!(keys_at(cx, w, &view), (true, false));
}

/// A hover's `rev` enters the walk on that block with its previous
/// revision in view.
#[gpui_kit::test]
fn the_hovers_rev_enters_the_walk_on_the_previous_revision(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_hr", json!([{"op":"patch","id":"a","set":{"text":"A second"}}]));
    tail(cx, w, &view);
    let a = row("a");
    let at_ = head(cx, w, "a");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    let r = drawn(cx, w, format!("hover-act {a} rev")).center();
    click(cx, w, r);
    wait(cx, w, &view, 300);
    assert_eq!(at(cx, &view), Some(At::Block(a)));
    assert!(walking(cx, w, &view));
    let label = view.read_with(cx, |v, _| v.walk_past(a).map(|p| p.label)).expect("a past revision");
    assert!(label.starts_with("r1 of 2"), "{label}");
}

// ── undo ──────────────────────────────────────────────────────────────

/// Ctrl+Z in the walk undoes the last local action, one level per block,
/// newest first, the walk going to it: a key's fold, and a click's. A
/// block since replaced (another incarnation) is not undone: the note.
#[gpui_kit::test]
fn undo_reverses_the_last_local_action_per_block(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    // A click's fold of a, then the walk's of c and b.
    let at_ = head(cx, w, "a");
    click(cx, w, at_);
    away(cx, w);
    assert!(folded(cx, &view, "a"));
    enter(cx, w, &view);
    press(cx, w, "z");
    press(cx, w, "k");
    wait(cx, w, &view, 300);
    press(cx, w, "z");
    press(cx, w, "z");
    assert!(folded(cx, &view, "c") && !folded(cx, &view, "b"), "b folded and unfolded: one level");
    press(cx, w, "ctrl-z");
    assert!(folded(cx, &view, "b"), "b's last: its unfold, undone");
    press(cx, w, "ctrl-z");
    assert!(!folded(cx, &view, "c"), "then c's");
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))), "the walk went to it");
    // a, replaced since its fold: not undone.
    ops(cx, w, &view, "toolu_wu", json!([{"op":"replace","id":"a","kind":"note","body":{"text":"A again"}}]));
    wait(cx, w, &view, 300);
    press(cx, w, "ctrl-z");
    assert_eq!(note(cx, &view), Some((row("a"), crate::lease::CHANGED)), "another incarnation: the note");
    press(cx, w, "ctrl-z");
    assert!(folded(cx, &view, "b"), "nothing left to undo");
}

// ── D6b's seam ────────────────────────────────────────────────────────

/// What D6b was asked: the wish, the block, the session, a rail section.
type Ask = (Wish, String, String, bool);

/// An ask's placement and tombstone token.
type Aimed = Rc<RefCell<Vec<(Place, Option<u64>)>>>;

/// A stand-in for D6b's side: what it was asked, its answer, and each
/// ask's placement and tombstone token.
#[derive(Clone, Default)]
struct Asks(Rc<RefCell<Vec<Ask>>>, Rc<RefCell<Option<String>>>, Aimed);

impl BlockDesires for Asks {
    fn wired(&self) -> bool {
        true
    }
    fn dismiss(&mut self, a: &Aim) -> Asked {
        self.ask(Wish::Dismiss, a)
    }
    fn show(&mut self, a: &Aim) -> Asked {
        self.ask(Wish::Show, a)
    }
    fn pin(&mut self, a: &Aim) -> Asked {
        self.ask(Wish::Pin, a)
    }
    fn slot(&mut self, a: &Aim) -> Asked {
        self.ask(Wish::Slot, a)
    }
    fn undo(&mut self, a: &Aim, wish: Wish, token: u64) -> Asked {
        assert_eq!(token, self.token(wish, a), "its own token back");
        self.0.borrow_mut().push((wish, format!("undo {}", a.block.block.id), a.session.to_string(), on_rail(a)));
        Asked::Done(0)
    }
}

/// The aim is a rail section's.
fn on_rail(a: &Aim) -> bool {
    matches!(a.place, cards_spec::board::Place::Rail(_))
}

impl Asks {
    fn token(&self, wish: Wish, a: &Aim) -> u64 {
        crate::blocks::row_id(&a.block.block.id) ^ wish as u64
    }

    fn ask(&self, wish: Wish, a: &Aim) -> Asked {
        assert!(a.drawn.rows.values().any(|r| r.blocks.iter().any(|s| s.block.id == a.block.block.id)) || on_rail(a));
        self.0.borrow_mut().push((wish, a.block.block.id.to_string(), a.session.to_string(), on_rail(a)));
        self.2.borrow_mut().push((a.place, a.tomb));
        match self.1.borrow().clone() {
            Some(why) => Asked::Refused(why),
            None => Asked::Done(self.token(wish, a)),
        }
    }
}

/// `x u p m` are D6b's: with D7's default nothing happens and nothing
/// offers them; wired, each asks D6b on the walked block's painted
/// target, a refusal says so, and Ctrl+Z undoes through it.
#[gpui_kit::test]
fn dismiss_show_pin_and_slot_go_to_d6bs_desires(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    view.update(cx, |v, _| v.desires = Some(Box::new(crate::walk::NoDesires)));
    enter(cx, w, &view);
    for k in ["x", "u", "p", "m"] {
        press(cx, w, k);
    }
    assert!(walking(cx, w, &view) && !folded(cx, &view, "c") && note(cx, &view).is_none(), "not wired: nothing");
    press(cx, w, "escape");
    let asks = Asks::default();
    view.update(cx, |v, _| v.desires = Some(Box::new(asks.clone())));
    enter(cx, w, &view);
    assert!(status(cx, &view).is_some_and(|s| s.contains("x dismiss · u show · p pin · m slot")));
    let sid = attachment(cx, &view).1;
    press(cx, w, "x");
    press(cx, w, "p");
    assert_eq!(
        *asks.0.borrow(),
        vec![(Wish::Dismiss, "c".to_string(), sid.clone(), false), (Wish::Pin, "c".to_string(), sid.clone(), false)]
    );
    press(cx, w, "ctrl-z");
    assert_eq!(asks.0.borrow().last().cloned(), Some((Wish::Pin, "undo c".to_string(), sid.clone(), false)), "the last, undone");
    // The hover offers them, wired.
    let at_ = head(cx, w, "c");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    let c = row("c");
    assert!(has(cx, w, format!("hover-act {c} rail")) && has(cx, w, format!("hover-act {c} ×")));
    away(cx, w);
    *asks.1.borrow_mut() = Some("a write failed".into());
    press(cx, w, "u");
    assert_eq!(note(cx, &view), Some((row("c"), crate::walk::REFUSED)), "refused: the note");
}

// ── D6b's stores behind the seam ──────────────────────────────────────

/// Two agent sections, `r1` and `r2`, in the rail's upper slot.
fn rail_blocks(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    ops(
        cx,
        w,
        view,
        "toolu_rb",
        json!([
            {"op":"put","id":"r1","kind":"note","target":"rail","slot":"upper","body":{"text":"R one"}},
            {"op":"put","id":"r2","kind":"note","target":"rail","slot":"upper","body":{"text":"R two"}}
        ]),
    );
    wait(cx, w, view, 1000);
}

/// Section `name` as the rail lays it out now: its slot, whether the user
/// placed it, whether it draws as a tombstone.
fn sec(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> Option<(Slot, bool, bool)> {
    let id = Ref::parse(name).expect("an id");
    view.read_with(cx, |v, _| {
        let (_, l) = v.rail_layout()?;
        l.section(&id).map(|s| (s.slot, s.user, matches!(s.state, crate::agent_rail::State::Tombstone(_))))
    })
}

/// D6b's writes landed (a dismissal draws once it is written), and the
/// frames after.
fn landed(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    super::rail_wishes_ui_tests::written(cx, view);
    wait(cx, w, view, 300);
    super::rail_wishes_ui_tests::written(cx, view);
}

fn desires(cx: &mut TestAppContext, view: &Entity<AppView>) -> usize {
    view.read_with(cx, |v, _| v.wishes.desires.len())
}

/// The desk restarts on the same files (the old one's locks let go): what
/// it draws is what they kept.
fn restart(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    landed(cx, w, view);
    let (d, p) = view.read_with(cx, |v, _| (v.wishes.desires.path().map(Into::into), v.wishes.pres.path().map(Into::into)));
    view.update(cx, |v, _| v.wishes = crate::rail_wishes::Stores::at_launch());
    let home = crate::rail_wishes::Home::take(d, p);
    view.update(cx, |v, cx| v.start_wishes(home, cx));
    landed(cx, w, view);
    assert!(view.read_with(cx, |v, _| v.wishes.desires.owns_file() && v.wishes.pres.owns_file()), "the files are its own");
}

/// `x` on an agent's rail section is D6b's dismissal: the section goes
/// once it is written, the zone counts it, and a restart keeps it; Ctrl+Z
/// cancels one no notice of which went, and the section comes back.
#[gpui_kit::test]
fn x_dismisses_a_rail_section_and_ctrl_z_cancels_it_unsent(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "dismissed: the section goes");
    assert_eq!(view.read_with(cx, |v, _| v.rail_layout().map(|(_, l)| l.dismissed)), Some(1), "the zone counts it");
    assert_eq!(desires(cx, &view), 1, "a desire, written");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, false)), "undone: back");
    assert_eq!(desires(cx, &view), 0, "never sent: cancelled");
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "x");
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "a restart keeps it");
    assert_eq!(desires(cx, &view), 1);
}

/// A dismissal whose notice went isn't undone by Ctrl+Z (`u` shows it
/// again): the section stays dismissed, and the rail says why.
#[cfg(unix)]
#[gpui_kit::test]
fn a_dismissal_whose_notice_went_isnt_undone(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "x");
    landed(cx, w, &view);
    press(cx, w, "escape");
    view.update(cx, |v, cx| assert!(v.enter("go on".into(), cx)));
    landed(cx, w, &view);
    let sent = view.read_with(cx, |v, _| v.wishes.outbox.clone());
    assert!(sent.last().is_some_and(|m| m.starts_with("go on\n[block r1 v") && m.ends_with(" dismissed]")), "it rode: {sent:?}");
    enter(cx, w, &view);
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "still dismissed");
    let said = view.read_with(cx, |v, _| v.wishes.said.clone()).unwrap_or_default();
    assert!(said.contains("its notice went"), "{said}");
}

/// `m` moves a rail section to the other slot, written to the file (a
/// restart keeps it); Ctrl+Z puts back the agent's placement.
#[gpui_kit::test]
fn m_moves_a_rail_section_and_ctrl_z_puts_it_back(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "m");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::End, true, false)), "the end slot, the user's");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, false)), "undone: the agent's");
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "m");
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::End, true, false)), "a restart keeps it");
}

/// `p` on a transcript block pins it into the rail's upper slot (a
/// restart keeps it); `p` on its rail section takes it out again, and
/// Ctrl+Z puts the pin back (its section found wherever the board has it).
#[gpui_kit::test]
fn p_pins_a_transcript_block_into_the_rail_and_back(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "pinned into the rail");
    walk_on(cx, w, &view, At::Rail(row("c")));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), None, "out of the rail");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "undone: pinned again");
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "a restart keeps it");
}

/// The walk finds a transcript block where this desk draws it (D6b's
/// placement, `BlockDesires::placed`): pinned into the rail, its
/// transcript row is placed in the rail's upper slot, and `m` on it moves
/// its rail section.
#[gpui_kit::test]
fn a_pinned_blocks_row_is_placed_where_its_pin_is(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    view.update(cx, |v, cx| {
        let conv = v.conversation(v.panes.cur.live);
        let (_, s) = v.block_shown(row("c")).expect("drawn");
        assert!(v.wishes.pres.pin(&conv, &s.block).is_ok());
        v.pump_wishes(cx);
        cx.notify();
    });
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "pinned into the rail");
    let place = view.read_with(cx, |v, _| v.bound_board().and_then(|d| v.find_on(&d, At::Block(row("c")))).map(|f| f.place));
    assert_eq!(place, Some(Place::Rail(Slot::Upper)), "its row, placed where its pin is");
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
    press(cx, w, "m");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::End, true, false)), "its pin, moved");
}

/// `u` on a soft tombstone ends it, and Ctrl+Z puts it back; `x` on one
/// sends a real notice of the new generation, and the tombstone ends.
#[gpui_kit::test]
fn u_ends_a_soft_tombstone_and_x_dismisses_it(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    view.update(cx, |v, cx| {
        let conv = v.conversation(v.panes.cur.live);
        let ids = ["r1", "r2"].map(|i| cards_spec::block::BlockId::parse(i).expect("an id"));
        v.wishes.pres.compacted("before-compaction", &conv, &ids);
        v.pump_wishes(cx);
        cx.notify();
    });
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, true)), "a soft tombstone");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "u");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, false)), "it ends");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, true)), "undone: the tombstone again");
    walk_on(cx, w, &view, At::Rail(row("r2")));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r2"), None, "dismissed");
    assert_eq!(desires(cx, &view), 1, "a real notice's desire");
    let soft = view.read_with(cx, |v, _| {
        let conv = v.conversation(v.panes.cur.live);
        v.wishes.pres.soft(&conv, &cards_spec::block::BlockId::parse("r2").expect("an id"))
    });
    assert!(!soft, "its tombstone ended");
}

/// The hover offers D6b's actions where this desk draws them: a transcript
/// block `rail` (a pin) and `×` (its tombstone); an agent's rail section
/// `transcript` (into it), `⇅` and `×`; a pinned one's section `rail` (out
/// again). They act.
#[gpui_kit::test]
fn the_hover_offers_d6bs_actions_where_they_draw(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    let c = row("c");
    let at_ = head(cx, w, "c");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("hover-act {c} rail")) && has(cx, w, format!("hover-act {c} ×")));
    let pin = drawn(cx, w, format!("hover-act {c} rail")).center();
    click(cx, w, pin);
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "pinned into the rail");
    let rail_head = |cx: &mut TestAppContext, name: &str| {
        let at_ = drawn(cx, w, format!("agent-head {}", row(name))).center();
        mouse(cx, w, at_, "move");
        wait(cx, w, &view, 300);
    };
    let r1 = row("r1");
    rail_head(cx, "r1");
    assert!(has(cx, w, format!("hover-act {r1} ⇅")) && has(cx, w, format!("hover-act {r1} ×")));
    assert!(
        has(cx, w, format!("hover-act {r1} transcript")) && !has(cx, w, format!("hover-act {r1} rail")),
        "an agent's section: into the transcript"
    );
    rail_head(cx, "c");
    assert!(has(cx, w, format!("hover-act {c} rail")), "a pinned one's: out again");
    let unpin = drawn(cx, w, format!("hover-act {c} rail")).center();
    click(cx, w, unpin);
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), None, "out of the rail");
    rail_head(cx, "r1");
    let into = drawn(cx, w, format!("hover-act {r1} transcript")).center();
    click(cx, w, into);
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "out of the rail");
    assert!(has(cx, w, format!("block-head {r1}")), "into the transcript");
}

// ── D6b's undo: its scope, its two stores, the caps, the order ────────

/// The walk to `to`, back (`k`) or on (`j`).
fn walk_to(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, to: At) {
    for _ in 0..16 {
        if at(cx, view) == Some(to) {
            break;
        }
        press(cx, w, "k");
    }
    walk_on(cx, w, view, to);
}

/// The walk has the keys (entered again if it ended).
fn walk_again(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    if !walking(cx, w, view) {
        enter(cx, w, view);
    }
}

/// What the user set for `name` in the pane's conversation (its marks).
fn mark(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> crate::presentation::Mark {
    view.read_with(cx, |v, _| v.wishes.pres.mark(&v.conversation(v.panes.cur.live), &Ref::parse(name).expect("an id")))
}

/// The writer let go: what it held runs, and lands.
fn release(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, hold: crate::blockfiles::Hold) {
    drop(hold);
    view.read_with(cx, |v, _| v.wishes.home.writer.clone()).drain();
    landed(cx, w, view);
}

/// What the rail's footer says last.
fn said(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, _| v.wishes.said.clone()).unwrap_or_default()
}

/// Shady #106 P2-1, with the transcript's tombstones drawn: Ctrl+Z acts
/// where its block draws now. A dismissal asked and cancelled before it
/// was written (`x`, `u`, the writer held), the section then moved by the
/// agent into the transcript (the same incarnation): undoing that `u`
/// dismisses it there, and the transcript draws its tombstone; the notice
/// rides the next message, as drawn.
#[cfg(unix)]
#[gpui_kit::test]
fn ctrl_z_follows_its_block_into_the_transcript(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    let writer = view.read_with(cx, |v, _| v.wishes.home.writer.clone());
    let hold = writer.hold();
    press(cx, w, "x");
    wait(cx, w, &view, 300);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, false)), "not written yet: it still draws");
    press(cx, w, "u");
    release(cx, w, &view, hold);
    assert_eq!(desires(cx, &view), 0, "cancelled before it was written");
    ops(cx, w, &view, "toolu_mv", json!([{"op":"move","id":"r1","target":"transcript"}]));
    wait(cx, w, &view, 1000);
    assert_eq!(sec(cx, &view, "r1"), None, "the agent moved it into the transcript");
    walk_again(cx, w, &view);
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(desires(cx, &view), 1, "dismissed where it draws now");
    assert!(has(cx, w, tomb_of("r1")), "its tombstone in the transcript");
    press(cx, w, "escape");
    view.update(cx, |v, cx| assert!(v.enter("go on".into(), cx)));
    landed(cx, w, &view);
    let sent = view.read_with(cx, |v, _| v.wishes.outbox.clone());
    assert!(sent.last().is_some_and(|m| m.starts_with("go on\n[block r1 v") && m.ends_with(" dismissed]")), "it rode: {sent:?}");
}

// ── the transcript's tombstones and stubs (§3.5, D6b × D7) ──────────

/// Block `name`'s transcript tombstone, dismissed (its debug selector).
fn tomb_of(name: &str) -> String {
    format!("block-tomb {} ◇ {name} · dismissed · u show", row(name))
}

/// Block `name`'s transcript pin stub, `more` after its words.
fn stub_of(name: &str, more: &str) -> String {
    format!("block-stub {} ◇ {name} · in the rail · p{more}", row(name))
}

/// The last message sent, after `text` is: what rode it.
fn send(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str) -> String {
    if walking(cx, w, view) {
        press(cx, w, "escape");
    }
    view.update(cx, |v, cx| assert!(v.enter(text.into(), cx)));
    landed(cx, w, view);
    view.read_with(cx, |v, _| v.wishes.outbox.last().cloned()).unwrap_or_default()
}

/// A one-line row's height at the pane's text step: §3.5's 24 px.
fn line_h(cx: &mut TestAppContext, view: &Entity<AppView>) -> gpui_kit::Pixels {
    view.read_with(cx, |v, _| px((24. * v.panes.cur.metrics_last.get().expect("drawn").r).round()))
}

/// What the pane's painted quads within `b` draw: an outline, a rounded
/// corner (the app's look has neither, `cards_d_tests`), and their fills.
fn quads_in(cx: &mut TestAppContext, w: AnyWindowHandle, b: gpui_kit::Bounds<gpui_kit::Pixels>) -> (bool, bool, Vec<gpui_kit::Hsla>) {
    cx.update_window(w, |_, window, _| {
        let k = window.scale_factor();
        let (x0, y0) = (f32::from(b.origin.x) * k - 0.5, f32::from(b.origin.y) * k - 0.5);
        let (x1, y1) = (f32::from(b.right()) * k + 0.5, f32::from(b.bottom()) * k + 0.5);
        let inside = |q: &gpui_kit::Bounds<gpui_kit::ScaledPixels>| {
            q.origin.x.0 >= x0 && q.origin.y.0 >= y0 && (q.origin.x + q.size.width).0 <= x1 && (q.origin.y + q.size.height).0 <= y1
        };
        let qs: Vec<_> = window.painted_quads().iter().filter(|q| inside(&q.bounds)).cloned().collect();
        let outline = qs.iter().any(|q| {
            let bw = q.border_widths;
            [bw.top, bw.right, bw.bottom, bw.left].iter().any(|x| x.0 > 0.01)
        });
        let round = qs.iter().any(|q| {
            let c = q.corner_radii;
            [c.top_left, c.top_right, c.bottom_right, c.bottom_left].iter().any(|r| r.0 > 0.01)
        });
        (outline, round, qs.iter().filter_map(|q| q.background.as_solid()).collect())
    })
    .expect("the window")
}

/// `x` on a transcript block dismisses it there (§3.5): once written it
/// shrinks to its 24 px tombstone, `◇ c · dismissed · u show`, square, no
/// outline, on the card's ground; Ctrl+Z cancels it unsent. Done again,
/// the next message tells the agent what the transcript draws, and the
/// tombstone stays; `u` on it shows the block again, and the next message
/// tells the agent that too.
#[gpui_kit::test]
fn x_and_u_on_a_transcript_block_draw_what_they_tell_the_agent(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let c = row("c");
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(c)));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert!(has(cx, w, tomb_of("c")), "its tombstone");
    assert!(!has(cx, w, format!("block-head {c}")), "nothing of the block");
    assert_eq!(desires(cx, &view), 1, "a desire, written");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert!(has(cx, w, format!("block-head {c}")) && !has(cx, w, tomb_of("c")), "undone: the block");
    assert_eq!(desires(cx, &view), 0, "never sent: cancelled");
    walk_on(cx, w, &view, At::Block(c));
    press(cx, w, "x");
    landed(cx, w, &view);
    let tomb = drawn(cx, w, tomb_of("c"));
    assert!((tomb.size.height - line_h(cx, &view)).abs() < px(0.5), "24 px: {tomb:?}");
    press(cx, w, "escape");
    wait(cx, w, &view, 300);
    let card = cx.update(|cx| crate::theme::palette(cx).card_d);
    let (outline, round, fills) = quads_in(cx, w, tomb);
    assert!(!outline && !round, "square, no outline");
    assert!(fills.contains(&card), "on the card's ground: {fills:?}");
    // v2: the cancelled one's version is never minted again.
    assert_eq!(send(cx, w, &view, "go on"), "go on\n[block c v2 dismissed]", "told as drawn");
    assert!(has(cx, w, tomb_of("c")), "still its tombstone");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Block(c));
    assert!(ring(cx, w, c), "a tombstone is walked to");
    press(cx, w, "u");
    landed(cx, w, &view);
    assert!(has(cx, w, format!("block-head {c}")) && !has(cx, w, tomb_of("c")), "shown again");
    assert_eq!(send(cx, w, &view, "and on"), "and on\n[block c v3 shown]", "told as drawn");
    restart(cx, w, &view);
    assert!(has(cx, w, format!("block-head {c}")), "a restart keeps it");
}

/// The hover's `×` on a transcript block dismisses it as `x` does; the
/// tombstone's click is its `u`, checked as a hover action is: the block
/// draws again, and no notice of a dismissal that never went rides the
/// next message (nothing to tell).
#[gpui_kit::test]
fn a_transcript_tombstones_click_shows_its_block(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let c = row("c");
    let at_ = head(cx, w, "c");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("hover-act {c} ×")), "offered");
    let x = drawn(cx, w, format!("hover-act {c} ×")).center();
    click(cx, w, x);
    landed(cx, w, &view);
    assert!(has(cx, w, tomb_of("c")), "its tombstone");
    away(cx, w);
    wait(cx, w, &view, 300);
    let at_ = drawn(cx, w, tomb_of("c")).center();
    click(cx, w, at_);
    landed(cx, w, &view);
    assert!(has(cx, w, format!("block-head {c}")) && !has(cx, w, tomb_of("c")), "shown again");
    assert_eq!(desires(cx, &view), 0, "unsent: cancelled");
    assert_eq!(send(cx, w, &view, "go on"), "go on", "nothing to tell");
}

/// `p` on a transcript block pins it into the rail (§3.5) and leaves its
/// 24 px stub in the transcript, `◇ c · in the rail · p`; the agent moving
/// it since adds `· the agent moved it`. The stub's click is its `p`: back
/// in the transcript. Nothing of it is told to the agent.
#[gpui_kit::test]
fn a_pin_leaves_its_stub_in_the_transcript(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    let c = row("c");
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(c)));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "pinned into the rail");
    assert!(has(cx, w, stub_of("c", "")) && !has(cx, w, format!("block-head {c}")), "its stub in the transcript");
    let stub = drawn(cx, w, stub_of("c", ""));
    assert!((stub.size.height - line_h(cx, &view)).abs() < px(0.5), "24 px: {stub:?}");
    press(cx, w, "escape");
    wait(cx, w, &view, 300);
    let (outline, round, _) = quads_in(cx, w, stub);
    assert!(!outline && !round, "square, no outline");
    ops(cx, w, &view, "toolu_mc", json!([{"op":"move","id":"c","target":"transcript"}]));
    wait(cx, w, &view, 1000);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "the pin outranks the agent's move");
    assert!(has(cx, w, stub_of("c", " · the agent moved it")), "the stub says so");
    let at_ = drawn(cx, w, stub_of("c", " · the agent moved it")).center();
    click(cx, w, at_);
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), None, "out of the rail");
    assert!(has(cx, w, format!("block-head {c}")), "the block, where the agent has it");
    assert_eq!(send(cx, w, &view, "go on"), "go on", "presentation: nothing told");
}

/// `p` on an agent's rail section puts it into the transcript (§3.5), at
/// its creating call: the rail no longer draws it (nor counts it
/// dismissed), the transcript draws the block, and the walk reaches it
/// there; `p` again gives it back to the rail, Ctrl+Z undoes that, and a
/// restart keeps it. Nothing of it is told to the agent.
#[gpui_kit::test]
fn p_puts_an_agents_rail_section_into_the_transcript_and_back(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    let r1 = row("r1");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(r1));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "out of the rail");
    assert_eq!(view.read_with(cx, |v, _| v.rail_layout().map(|(_, l)| l.dismissed)), Some(0), "not dismissed");
    assert!(has(cx, w, format!("block-head {r1}")), "drawn in the transcript");
    let (r1_ix, c_ix) = view.read_with(cx, |v, _| {
        let ix = |r: u64| v.panes.cur.disp.iter().position(|d| d.id == r).expect("a row");
        (ix(r1), ix(row("c")))
    });
    assert!(r1_ix > c_ix, "at its creating call, after the run before it: {r1_ix} vs {c_ix}");
    assert_eq!(send(cx, w, &view, "go on"), "go on", "presentation: nothing told");
    enter(cx, w, &view);
    walk_to(cx, w, &view, At::Block(r1));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, false)), "back in the rail, the agent's");
    assert!(view.read_with(cx, |v, _| v.block_shown(r1).is_none()), "gone from the transcript");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "undone: in the transcript again");
    assert!(view.read_with(cx, |v, _| v.block_shown(r1).is_some()));
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "a restart keeps it");
    assert!(view.read_with(cx, |v, _| v.block_shown(r1).is_some()));
    // The hover offers it back where it draws.
    let at_ = head(cx, w, "r1");
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("hover-act {r1} rail")) && has(cx, w, format!("hover-act {r1} ×")));
    let back = drawn(cx, w, format!("hover-act {r1} rail")).center();
    click(cx, w, back);
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, false)), "back by the hover");
}

/// A card's block is no desire's (D6b keys desires by block id): `x` on
/// it does nothing, and neither the hover nor the palette offers `x` or
/// `u`; its pin is offered.
#[gpui_kit::test]
fn a_cards_block_isnt_offered_a_dismissal(cx: &mut TestAppContext) {
    use crate::palette::Mode;
    let (w, view) = walked(cx);
    let name = format!("card:{}", synaps_client::synthetic::BLOCKS_CARD);
    let k = row(&name);
    enter(cx, w, &view);
    walk_to(cx, w, &view, At::Block(k));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert_eq!(desires(cx, &view), 0, "nothing asked");
    assert!(has(cx, w, format!("block-head {k}")), "still drawn");
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.open_palette(Mode::Commands, window, cx))).expect("the window");
    wait(cx, w, &view, 300);
    let off = |cx: &mut TestAppContext, title: &str| {
        view.read_with(cx, |v, cx| v.palette_entries(cx).1.iter().find(|e| e.title.as_ref() == title).map(|e| e.off.clone()))
    };
    assert_eq!(off(cx, "Dismiss the walked block"), Some(Some("not on this block".into())));
    assert_eq!(off(cx, "Show the walked block again"), Some(Some("not on this block".into())));
    assert_eq!(off(cx, "Pin the walked block"), Some(None), "its pin is");
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.close_palette(window, cx))).expect("the window");
    let at_ = head(cx, w, &name);
    mouse(cx, w, at_, "move");
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("hover-act {k} rail")) && !has(cx, w, format!("hover-act {k} ×")), "the hover: its pin, no ×");
}

/// A section put into the transcript is one of its rows to the lease
/// (§6.2): hovered, the agent replacing it waits under the pointer, and
/// the release draws it.
#[gpui_kit::test]
fn a_section_put_into_the_transcript_is_under_the_lease(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    let r1 = row("r1");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(r1));
    press(cx, w, "p");
    landed(cx, w, &view);
    press(cx, w, "escape");
    tail(cx, w, &view);
    assert!(has(cx, w, format!("block-head {r1}")), "in the transcript");
    let t0 = view.read_with(cx, |v, _| v.block_shown(r1).expect("its row").1.block.tag);
    let at_ = head(cx, w, "r1");
    mouse(cx, w, at_, "move");
    ops(cx, w, &view, "toolu_rr", json!([{"op":"replace","id":"r1","kind":"note","body":{"text":"R one again"}}]));
    assert!(waiting(cx, &view), "the board waits under the pointer");
    assert_eq!(view.read_with(cx, |v, _| v.block_shown(r1).expect("its row").1.block.tag), t0, "the old one still drawn");
    press(cx, w, "escape");
    wait(cx, w, &view, 300);
    assert!(!waiting(cx, &view), "a key released the lease");
    assert_ne!(view.read_with(cx, |v, _| v.block_shown(r1).expect("its row").1.block.tag), t0, "the new incarnation drawn");
}

/// The palette's commands are the walk's keys on the block walked when it
/// opened (the walk ends as the keys go to the palette): `x` dismisses it,
/// drawn as its tombstone and told as drawn; without a walk they say how
/// to get one.
#[gpui_kit::test]
fn the_palettes_block_commands_act_on_the_walked_block(cx: &mut TestAppContext) {
    use crate::palette::Mode;
    let (w, view) = walked(cx);
    let entry = |cx: &mut TestAppContext, title: &str| {
        view.read_with(cx, |v, cx| {
            let (_, cmds) = v.palette_entries(cx);
            cmds.iter().find(|e| e.title.as_ref() == title).map(|e| e.off.clone())
        })
    };
    let dismiss = "Dismiss the walked block";
    assert_eq!(entry(cx, dismiss), Some(Some("walk to a block first · Ctrl+G".into())), "no walk: off");
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.open_palette(Mode::Commands, window, cx))).expect("the window");
    wait(cx, w, &view, 300);
    assert!(!walking(cx, w, &view), "the keys went to the palette");
    assert_eq!(entry(cx, dismiss), Some(None), "offered on the walked block");
    view.update(cx, |v, _| {
        let p = v.palette.as_mut().expect("open");
        p.set_query("dismiss the walked");
    });
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.palette_pick(window, cx))).expect("the window");
    landed(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.palette.is_none()), "closed");
    assert!(has(cx, w, tomb_of("c")), "dismissed: its tombstone");
    assert_eq!(send(cx, w, &view, "go on"), "go on\n[block c v1 dismissed]", "told as drawn");
    // On its tombstone: `u` shows it, `p` isn't offered.
    let pick = |cx: &mut TestAppContext, query: &str| {
        cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.open_palette(Mode::Commands, window, cx))).expect("the window");
        wait(cx, w, &view, 300);
        view.update(cx, |v, _| v.palette.as_mut().expect("open").set_query(query));
        cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.palette_pick(window, cx))).expect("the window");
        landed(cx, w, &view);
    };
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Block(row("c")));
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.open_palette(Mode::Commands, window, cx))).expect("the window");
    wait(cx, w, &view, 300);
    assert_eq!(entry(cx, "Pin the walked block"), Some(Some("a tombstone: x or u".into())), "a tombstone: no pin");
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.close_palette(window, cx))).expect("the window");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Block(row("c")));
    pick(cx, "show the walked");
    assert!(has(cx, w, format!("block-head {}", row("c"))) && !has(cx, w, tomb_of("c")), "shown again");
    assert_eq!(send(cx, w, &view, "and on"), "and on\n[block c v2 shown]", "told as drawn");
    // `p` on a transcript block: its stub.
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Block(row("c")));
    pick(cx, "pin the walked");
    assert!(has(cx, w, stub_of("c", "")), "pinned: its stub");
}

/// Soft dismissals of `ids` carried into the pane's conversation (a
/// compaction's), written.
fn softly(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ids: &[String]) {
    view.update(cx, |v, cx| {
        let conv = v.conversation(v.panes.cur.live);
        let ids: Vec<_> = ids.iter().map(|i| cards_spec::block::BlockId::parse(i).expect("an id")).collect();
        v.wishes.pres.compacted("before-compaction", &conv, &ids);
        v.pump_wishes(cx);
        cx.notify();
    });
    landed(cx, w, view);
}

fn is_soft(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> bool {
    view.read_with(cx, |v, _| {
        v.wishes.pres.soft(&v.conversation(v.panes.cur.live), &cards_spec::block::BlockId::parse(name).expect("an id"))
    })
}

/// `r1` and `r2` soft tombstones, and `x` on `r1`'s: its dismissal
/// written, its tombstone ended.
fn x_on_a_tombstone(cx: &mut TestAppContext) -> W {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    softly(cx, w, &view, &["r1".into(), "r2".into()]);
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None, "dismissed");
    assert!(desires(cx, &view) == 1 && !is_soft(cx, &view, "r1"), "a desire, and its tombstone ended");
    (w, view)
}

/// Shady #106 P2-2: Ctrl+Z of `x` on a soft tombstone is both stores'
/// (the dismissal cancelled, the tombstone back). The disk refusing the
/// desire's write puts the presentation back too: both as `x` left them,
/// in memory, on the rail and in the files; its token gone.
#[cfg(unix)]
#[gpui_kit::test]
fn an_undo_whose_desire_isnt_written_leaves_both_stores_as_its_key_did(cx: &mut TestAppContext) {
    let (w, view) = x_on_a_tombstone(cx);
    let path = view.read_with(cx, |v, _| v.wishes.desires.path().expect("a file").to_path_buf());
    let file = std::fs::read(&path).expect("x's dismissal, written");
    // The disk refuses: a folder where the file goes.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert!(said(cx, &view).contains("not saved"), "{}", said(cx, &view));
    assert_eq!(sec(cx, &view, "r1"), None, "still dismissed");
    assert_eq!(desires(cx, &view), 1, "the dismissal, as x left it");
    assert!(!is_soft(cx, &view, "r1"), "and its tombstone, as x left it: ended");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert!(desires(cx, &view) == 1 && !is_soft(cx, &view, "r1"), "its token went: nothing again");
    // The disk back as x wrote it: a restart reads both as x left them.
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, file).unwrap();
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None);
    assert!(desires(cx, &view) == 1 && !is_soft(cx, &view, "r1") && is_soft(cx, &view, "r2"));
}

/// Shady #106 P2-2: the same undo, its write late: until it lands the
/// rail draws what `x` left; then both stores, the rail and the files are
/// what was before `x`, together.
#[gpui_kit::test]
fn a_late_undo_puts_both_stores_back_together(cx: &mut TestAppContext) {
    let (w, view) = x_on_a_tombstone(cx);
    let hold = view.read_with(cx, |v, _| v.wishes.home.writer.clone()).hold();
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 300);
    assert_eq!(sec(cx, &view, "r1"), None, "not written yet: the dismissal draws");
    release(cx, w, &view, hold);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, true)), "undone: its tombstone");
    assert!(desires(cx, &view) == 0 && is_soft(cx, &view, "r1"), "both stores");
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), Some((Slot::Upper, false, true)), "and the files");
    assert_eq!(desires(cx, &view), 0);
}

/// Shady #106 P2-2, round 2: what a failed undo write needs kept is
/// never dropped for room. `r1`'s undo held (its cancel a write away),
/// then `x` and Ctrl+Z on `r2`'s tombstone again and again, past
/// [`UNDO_KEPT`](crate::rail_wishes::UNDO_KEPT) (two wishes waiting all
/// along), and the disk refuses the writes: `r1` both as `x` left it, `r2`
/// as before its first `x`; in memory, on the rail and in the files, and
/// no token left to undo.
#[cfg(unix)]
#[gpui_kit::test]
fn a_failed_undo_keeps_both_stores_together_however_many_undos_follow(cx: &mut TestAppContext) {
    let (w, view) = x_on_a_tombstone(cx);
    let path = view.read_with(cx, |v, _| v.wishes.desires.path().expect("a file").to_path_buf());
    let file = std::fs::read(&path).expect("x's dismissal, written");
    let hold = view.read_with(cx, |v, _| v.wishes.home.writer.clone()).hold();
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 300);
    assert!(sec(cx, &view, "r1").is_none() && is_soft(cx, &view, "r1"), "its tombstone back, the dismissal drawn until written");
    walk_again(cx, w, &view);
    walk_to(cx, w, &view, At::Rail(row("r2")));
    for n in 0..=crate::rail_wishes::UNDO_KEPT {
        press(cx, w, "x");
        wait(cx, w, &view, 20);
        assert!(!is_soft(cx, &view, "r2"), "x {n}: r2's tombstone ended: {}", said(cx, &view));
        press(cx, w, "ctrl-z");
        wait(cx, w, &view, 20);
        assert!(is_soft(cx, &view, "r2"), "undo {n}: back: {}", said(cx, &view));
    }
    // The disk refuses: a folder where the file goes.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    release(cx, w, &view, hold);
    assert!(said(cx, &view).contains("not saved"), "{}", said(cx, &view));
    assert_eq!(sec(cx, &view, "r1"), None, "r1 still dismissed");
    assert_eq!(desires(cx, &view), 1, "r1's dismissal as x left it, none of r2's");
    assert!(!is_soft(cx, &view, "r1"), "and r1's tombstone as x left it: ended");
    assert_eq!(sec(cx, &view, "r2"), Some((Slot::Upper, false, true)), "r2 as before its first x");
    assert!(is_soft(cx, &view, "r2"));
    walk_again(cx, w, &view);
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert!(desires(cx, &view) == 1 && !is_soft(cx, &view, "r1") && is_soft(cx, &view, "r2"), "no token left: nothing again");
    // The disk back as x wrote it: a restart reads it all so.
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, file).unwrap();
    restart(cx, w, &view);
    assert_eq!(sec(cx, &view, "r1"), None);
    assert_eq!(sec(cx, &view, "r2"), Some((Slot::Upper, false, true)));
    assert!(desires(cx, &view) == 1 && !is_soft(cx, &view, "r1") && is_soft(cx, &view, "r2"));
}

/// The rail demo's history (`first` its first message: its generation),
/// `plan`'s dismissal shared: a notice of it in the history.
fn plan_shared(first: Option<&str>) -> Vec<Value> {
    let mut h = synaps_client::synthetic::rail_history();
    if let Some(first) = first {
        h[0] = json!({"role": "user", "content": first});
    }
    h.push(json!({"role": "user", "content": "noted\n[block plan v1 dismissed]"}));
    h.push(json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]}));
    h
}

/// The footer's `show`, clicked (come a moment after the board, D6a's
/// guard).
fn footer_show(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    wait(cx, w, view, 700);
    let show = drawn(cx, w, "agent-footer 1 dismissed · show".into()).center();
    click(cx, w, show);
    wait(cx, w, view, 300);
    away(cx, w);
}

/// Shady #106 P2-2, round 3: an undo's recovery is its own write's, by
/// that write's ticket. `plan` soft, its dismissal shared; the footer's
/// Show saved (its tombstone draws), `x` on the tombstone saved (the Show
/// cancelled to none, the tombstone ended), and Ctrl+Z with the writer
/// held (the Show again, the tombstone back). The history's generation
/// switches, retiring that write; then the footer's Show in the new one,
/// and the disk refuses: that Show changed no presentation, so the
/// tombstone stays, in memory and in the files.
#[cfg(unix)]
#[gpui_kit::test]
fn a_retired_undos_recovery_never_acts_on_another_generations_write(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    history_now(cx, w, &view, plan_shared(None));
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "plan"), None, "shared: dismissed");
    softly(cx, w, &view, &["plan".into()]);
    footer_show(cx, w, &view);
    landed(cx, w, &view);
    assert_eq!(desires(cx, &view), 1, "a local show");
    assert_eq!(sec(cx, &view, "plan").map(|s| s.2), Some(true), "shown: its soft tombstone");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("plan")));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert!(desires(cx, &view) == 0 && !is_soft(cx, &view, "plan"), "the show cancelled, the tombstone ended");
    let path = view.read_with(cx, |v, _| v.wishes.desires.path().expect("a file").to_path_buf());
    let file = std::fs::read(&path).expect("x's, written");
    let hold = view.read_with(cx, |v, _| v.wishes.home.writer.clone()).hold();
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 300);
    assert!(is_soft(cx, &view, "plan"), "the tombstone back");
    assert_eq!(view.read_with(cx, |v, _| (v.wishes.undoing(), v.wishes.desires.queued_wishes())), (1, 1), "its show a write away");
    // Another generation: the undo's write retired.
    history_now(cx, w, &view, plan_shared(Some("another start")));
    wait(cx, w, &view, 300);
    let kept = mark(cx, &view, "plan");
    footer_show(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| v.wishes.desires.queued_wishes()), 1, "the new generation's show, a write away");
    // The disk refuses: a folder where the file goes.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    release(cx, w, &view, hold);
    assert!(said(cx, &view).contains("not saved"), "{}", said(cx, &view));
    assert_eq!(desires(cx, &view), 0, "the show not kept");
    assert_eq!(sec(cx, &view, "plan"), None, "dismissed, as shared");
    assert_eq!(mark(cx, &view, "plan"), kept, "the show's failure touched no presentation");
    assert!(is_soft(cx, &view, "plan"), "its soft dismissal stays");
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, file).unwrap();
    restart(cx, w, &view);
    assert!(is_soft(cx, &view, "plan"), "and in the files");
}

/// Shady #106 P2-2, round 3: an undo's recovery goes with its write, and
/// only its own: retired by another generation's settle, or by a
/// compaction, it goes too (records never outnumber the writes queued),
/// and nothing is put back.
fn a_retired_write_retires_its_recovery(cx: &mut TestAppContext, how: &str) {
    let (w, view) = x_on_a_tombstone(cx);
    let hold = view.read_with(cx, |v, _| v.wishes.home.writer.clone()).hold();
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 300);
    let counts = |cx: &mut TestAppContext| view.read_with(cx, |v, _| (v.wishes.undoing(), v.wishes.desires.queued_wishes()));
    assert_eq!(counts(cx), (1, 1), "the undo's cancel a write away, and its recovery");
    let marks = |cx: &mut TestAppContext| (is_soft(cx, &view, "r1"), is_soft(cx, &view, "r2"));
    assert_eq!(marks(cx), (true, true), "r1's tombstone back");
    match how {
        "switch" => {
            let mut h = synaps_client::synthetic::blocks_history();
            h[0] = json!({"role": "user", "content": "another start"});
            history_now(cx, w, &view, h);
        }
        _ => {
            let batch = compaction_onto(cx, &view, "j-next", synaps_client::synthetic::blocks_history());
            view.update(cx, |v, cx| v.apply_batch(batch, cx));
            cx.run_until_parked();
            wait(cx, w, &view, 300);
        }
    }
    assert_eq!(counts(cx), (0, 0), "{how}: the write retired, its recovery too");
    let before = view.read_with(cx, |v, _| v.wishes.pres.mark("j-next", &Ref::parse("r1").expect("an id")));
    release(cx, w, &view, hold);
    assert_eq!(counts(cx), (0, 0), "{how}");
    if how == "switch" {
        assert_eq!(marks(cx), (true, true), "{how}: nothing put back");
    } else {
        let after = view.read_with(cx, |v, _| v.wishes.pres.mark("j-next", &Ref::parse("r1").expect("an id")));
        assert_eq!(after, before, "{how}: nothing put back");
    }
}

#[gpui_kit::test]
fn a_generation_switch_retires_an_undos_recovery_with_its_write(cx: &mut TestAppContext) {
    a_retired_write_retires_its_recovery(cx, "switch");
}

#[gpui_kit::test]
fn a_compaction_retires_an_undos_recovery_with_its_write(cx: &mut TestAppContext) {
    a_retired_write_retires_its_recovery(cx, "compaction");
}

/// Shady #106 P2-2, round 3: `r1` hidden while its undo's write waits
/// (the dismissal `x` saved draws until then), the agent moves it into
/// the transcript, and `p` pins it there: a newer placement. The disk
/// refusing the undo's write leaves that placement, in memory and in the
/// files (its key left the mark another one).
#[cfg(unix)]
#[gpui_kit::test]
fn a_failed_undo_leaves_a_placement_made_since(cx: &mut TestAppContext) {
    let (w, view) = x_on_a_tombstone(cx);
    let path = view.read_with(cx, |v, _| v.wishes.desires.path().expect("a file").to_path_buf());
    let file = std::fs::read(&path).expect("x's dismissal, written");
    let hold = view.read_with(cx, |v, _| v.wishes.home.writer.clone()).hold();
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 300);
    assert!(sec(cx, &view, "r1").is_none() && is_soft(cx, &view, "r1"), "hidden until written, its tombstone back");
    let undone = mark(cx, &view, "r1");
    ops(cx, w, &view, "toolu_mv", json!([{"op":"move","id":"r1","target":"transcript"}]));
    wait(cx, w, &view, 1000);
    // There it draws its tombstone (the dismissal draws until the undo is
    // written), which takes `x` and `u` only: the newer placement is the
    // store's own pin.
    assert!(has(cx, w, tomb_of("r1")), "a tombstone in the transcript");
    view.update(cx, |v, cx| {
        let conv = v.conversation(v.panes.cur.live);
        let (_, s) = v.block_shown(row("r1")).expect("its row");
        assert!(v.wishes.pres.pin(&conv, &s.block).is_ok());
        v.pump_wishes(cx);
        cx.notify();
    });
    wait(cx, w, &view, 300);
    let pinned = mark(cx, &view, "r1");
    assert_ne!(pinned, undone, "p: a newer placement");
    // The disk refuses: a folder where the file goes.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    release(cx, w, &view, hold);
    assert!(said(cx, &view).contains("not saved"), "{}", said(cx, &view));
    assert_eq!(desires(cx, &view), 1, "r1's dismissal, as x left it");
    assert_eq!(mark(cx, &view, "r1"), pinned, "the pin stays");
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, file).unwrap();
    restart(cx, w, &view);
    assert_eq!(mark(cx, &view, "r1"), pinned, "and in the files");
}

/// The pane's conversation's presentation entries.
fn entries(cx: &mut TestAppContext, view: &Entity<AppView>) -> usize {
    view.read_with(cx, |v, _| {
        let conv = v.conversation(v.panes.cur.live);
        (0..=crate::presentation::ENTRIES_MAX)
            .filter(|i| v.wishes.pres.soft(&conv, &cards_spec::block::BlockId::parse(&format!("s{i}")).expect("an id")))
            .count()
            + v.wishes.pres.user_rail(&conv).len()
    })
}

/// Shady #106 P2-3: Ctrl+Z puts back what it says, or nothing. At the
/// conversation's cap (64 entries), `c`'s pin the oldest: pane 1 unpins
/// it, pane 2 (the same conversation) pins `b` into the room, and pane
/// 1's Ctrl+Z would put back an entry the cap drops at once. Refused, and
/// said why: nothing changed.
#[gpui_kit::test]
fn an_undo_the_caps_would_drop_is_refused_and_changes_nothing(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
    press(cx, w, "p");
    landed(cx, w, &view);
    let fill: Vec<String> = (1..crate::presentation::ENTRIES_MAX).map(|i| format!("s{i}")).collect();
    softly(cx, w, &view, &fill);
    assert_eq!(entries(cx, &view), crate::presentation::ENTRIES_MAX, "at the cap, c's pin the oldest");
    walk_on(cx, w, &view, At::Rail(row("c")));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), None, "unpinned");
    // Pane 2, on the same conversation: `b` pinned into the room.
    press(cx, w, "escape");
    let first = view.read_with(cx, |v, _| v.panes.focused);
    split(cx, w, &view);
    tail(cx, w, &view);
    enter(cx, w, &view);
    walk_to(cx, w, &view, At::Block(row("b")));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "b"), Some((Slot::Upper, true, false)), "b pinned");
    assert_eq!(entries(cx, &view), crate::presentation::ENTRIES_MAX);
    // Pane 1's Ctrl+Z.
    let c1 = view.read_with(cx, |v, _| v.panes.iter().find(|p| p.id == first).map(|p| p.composer.clone()).expect("pane 1"));
    cx.update_window(w, |_, window, cx| c1.update(cx, |s, cx| s.focus(window, cx))).expect("the window");
    settle(cx, w, 8);
    wait(cx, w, &view, 100);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), first, "pane 1");
    enter(cx, w, &view);
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), None, "not pinned again");
    assert!(said(cx, &view).contains("no room to put it back"), "refused, said: {}", said(cx, &view));
    assert_eq!(sec(cx, &view, "b"), Some((Slot::Upper, true, false)), "b as it was");
    assert_eq!(entries(cx, &view), crate::presentation::ENTRIES_MAX, "every entry as it was");
}

/// Shady #106 P2-3: an undo of both stores at the cap (`x` on a soft
/// tombstone, its soft entry the oldest, a carry filling the room since)
/// is refused before either store changes: the dismissal stays.
#[gpui_kit::test]
fn an_undo_of_both_stores_the_caps_would_drop_changes_neither(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    rail_blocks(cx, w, &view);
    softly(cx, w, &view, &["r1".into()]);
    let fill: Vec<String> = (1..crate::presentation::ENTRIES_MAX).map(|i| format!("s{i}")).collect();
    softly(cx, w, &view, &fill);
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "x");
    landed(cx, w, &view);
    assert!(sec(cx, &view, "r1").is_none() && desires(cx, &view) == 1 && !is_soft(cx, &view, "r1"), "dismissed");
    softly(cx, w, &view, &[format!("s{}", crate::presentation::ENTRIES_MAX)]);
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(desires(cx, &view), 1, "the dismissal stays");
    assert!(sec(cx, &view, "r1").is_none() && !is_soft(cx, &view, "r1"), "neither store changed");
    assert!(said(cx, &view).contains("no room to put it back"), "refused, said: {}", said(cx, &view));
}

/// Shady #106 P2-4: an undo that brings an emptied conversation back
/// keeps its counter past what it puts back. `c` pinned and moved twice
/// (its entry the conversation's third), unpinned (the conversation
/// gone), Ctrl+Z; after a restart `b` pinned into the same slot sorts
/// after `c`, and `c`, the older, is the first the cap drops.
#[gpui_kit::test]
fn an_undo_keeps_the_conversations_order_past_a_restart(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    assert_eq!(at(cx, &view), Some(At::Block(row("c"))));
    press(cx, w, "p");
    landed(cx, w, &view);
    walk_to(cx, w, &view, At::Rail(row("c")));
    press(cx, w, "m");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::End, true, false)));
    walk_to(cx, w, &view, At::Rail(row("c")));
    press(cx, w, "m");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)));
    walk_to(cx, w, &view, At::Rail(row("c")));
    press(cx, w, "p");
    landed(cx, w, &view);
    assert!(sec(cx, &view, "c").is_none() && entries(cx, &view) == 0, "unpinned: the conversation's last");
    press(cx, w, "ctrl-z");
    landed(cx, w, &view);
    assert_eq!(sec(cx, &view, "c"), Some((Slot::Upper, true, false)), "undone");
    press(cx, w, "escape");
    restart(cx, w, &view);
    enter(cx, w, &view);
    walk_to(cx, w, &view, At::Block(row("b")));
    press(cx, w, "p");
    landed(cx, w, &view);
    let rail = view.read_with(cx, |v, _| v.wishes.pres.user_rail(&v.conversation(v.panes.cur.live)));
    let (c, b) = (Ref::parse("c").expect("an id"), Ref::parse("b").expect("an id"));
    assert_eq!(rail, [(c, Slot::Upper), (b, Slot::Upper)], "c, the older, first");
    // One past the cap: the oldest goes.
    let fill: Vec<String> = (1..crate::presentation::ENTRIES_MAX).map(|i| format!("s{i}")).collect();
    softly(cx, w, &view, &fill);
    assert_eq!(sec(cx, &view, "c"), None, "c went first");
    assert_eq!(sec(cx, &view, "b"), Some((Slot::Upper, true, false)), "b stays");
}

// ── the op log ────────────────────────────────────────────────────────

fn oplog_lines(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    view.read_with(cx, |v, _| v.oplog_lines().iter().map(crate::oplog::text).collect())
}

/// Ctrl+O opens the op log over the pane: the board's ops newest first,
/// a skipped one with why; a click on a line whose block is live walks to
/// it; Esc closes it, the keys back with the composer.
#[gpui_kit::test]
fn the_op_log_lists_the_boards_ops_and_a_line_walks_to_its_block(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_ol", json!([{"op":"patch","id":"nope","set":{"text":"x"}}]));
    press(cx, w, "ctrl-o");
    assert!(has(cx, w, "oplog".into()), "the op log drawn");
    let lines = oplog_lines(cx, &view);
    let first = lines[0].split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(first.contains("agent patch nope skipped · no live block"), "newest first, why skipped: {first}");
    assert!(lines.iter().any(|l| l.split_whitespace().collect::<Vec<_>>().join(" ").contains("agent put b applied")), "{lines:#?}");
    assert!(has(cx, w, "oplog-line 0".to_string() + " " + &lines[0]));
    // A line of b's: it walks there.
    let k = lines.iter().position(|l| l.split_whitespace().nth(3) == Some("b")).expect("b's line");
    let at_ = drawn(cx, w, format!("oplog-line {k} {}", lines[k])).center();
    click_over(cx, w, at_);
    wait(cx, w, &view, 300);
    assert!(view.read_with(cx, |v, _| v.oplog.is_none()), "the jump closes it");
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))));
    assert!(walking(cx, w, &view));
    press(cx, w, "escape");
    // Opened again: Esc closes it, the composer's keys back.
    press(cx, w, "ctrl-o");
    assert!(has(cx, w, "oplog".into()));
    press(cx, w, "j");
    assert_eq!(composer_text(cx, &view), "", "its keys are its own");
    press(cx, w, "escape");
    assert!(!has(cx, w, "oplog".into()));
    assert_eq!(keys_at(cx, w, &view), (true, false));
    // Ctrl+O toggles.
    press(cx, w, "ctrl-o");
    press(cx, w, "ctrl-o");
    assert!(!has(cx, w, "oplog".into()));
}

/// A click anywhere, as a hand would (not only in a list).
fn click_over(cx: &mut TestAppContext, w: AnyWindowHandle, at_: gpui_kit::Point<gpui_kit::Pixels>) {
    click(cx, w, at_);
}

/// The archive tab (§5.2): empty until a compaction; then the
/// predecessor's final blocks, whole, and its op log, read-only, its text
/// copied whole. Its lines don't jump.
#[gpui_kit::test]
fn the_archive_tab_keeps_the_predecessors_blocks_and_log(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_ml", json!([{"op":"put","id":"m","kind":"note","body":{"text":"first line\nimportant second line"}}]));
    press(cx, w, "ctrl-o");
    press(cx, w, "tab");
    assert!(has(cx, w, "oplog-archive-empty".into()), "nothing before a compaction yet");
    press(cx, w, "escape");
    compact(cx, w, &view);
    let all = copied(cx, w, &view);
    assert!(all.contains("b · note · ") && all.contains("\"text\": \"B first\""), "{all}");
    assert!(all.contains("important second line"), "a note whole: {all}");
    assert!(all.split('\n').any(|l| l.split_whitespace().collect::<Vec<_>>().join(" ").contains("agent put c applied")), "{all}");
    let a = view.read_with(cx, |v, _| v.archive_shown().and_then(|h| h.archive.clone())).expect("the archive");
    assert!(a.bytes() <= crate::oplog::ARCHIVE_MAX && !a.cut);
    press(cx, w, "ctrl-o");
    press(cx, w, "tab");
    assert!(has(cx, w, format!("oplog-archive-block 0 {}", a.blocks[0])), "drawn, a line at a time");
}

/// The session's compaction, as the daemon says it: the conversation it
/// begins is `journal` (none: the one the session holds, as an attach of
/// it says).
fn compact_into(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, journal: Option<&str>) {
    let sid = attachment(cx, view).1;
    let j =
        journal.map_or_else(|| view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.journal().to_string()), str::to_string);
    heard(
        cx,
        w,
        view,
        vec![json!({"ev": "compaction_applied", "previous_session_id": sid, "session_id": j, "chains_advanced": [], "msg_count": 4})],
    );
    wait(cx, w, view, 300);
}

fn compact(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    compact_into(cx, w, view, None);
}

/// The archive tab's text as its copy gives it (none drawn: `None`).
fn copied_now(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> Option<String> {
    press(cx, w, "ctrl-o");
    press(cx, w, "tab");
    let copy = super::blocks_ui_tests::bounds(cx, w, "oplog-copy".into()).map(|b| b.center());
    let text = copy.map(|c| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
        click(cx, w, c);
        cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default()
    });
    press(cx, w, "escape");
    wait(cx, w, view, 100);
    text
}

fn copied(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> String {
    copied_now(cx, w, view).expect("an archive to copy")
}

// ── the rail ──────────────────────────────────────────────────────────

/// The rail's agent sections are walked after the transcript's blocks, as
/// the rail draws them; `z` folds a section locally and Ctrl+Z undoes it.
#[gpui_kit::test]
fn the_rails_sections_are_walked_after_the_transcripts_blocks(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 2400.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::RAIL, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    let order = view.read_with(cx, |v, _| v.walk_order());
    let first_rail = order.iter().position(|a| matches!(a, At::Rail(_))).expect("the rail's sections in the walk");
    assert!(order[first_rail..].iter().all(|a| matches!(a, At::Rail(_))), "after the transcript's: {order:?}");
    enter(cx, w, &view);
    let k0 = order.iter().position(|a| Some(*a) == at(cx, &view)).expect("in the order");
    for _ in k0..first_rail {
        press(cx, w, "j");
    }
    wait(cx, w, &view, 300);
    let At::Rail(r) = order[first_rail] else { unreachable!() };
    assert_eq!(at(cx, &view), Some(At::Rail(r)));
    assert!(ring(cx, w, r), "its ring on the rail");
    let open = |cx: &mut TestAppContext| has(cx, w, format!("agent-body {r}"));
    let was = open(cx);
    press(cx, w, "z");
    wait(cx, w, &view, 1000);
    assert_ne!(open(cx), was, "z folds it, locally");
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 1000);
    assert_eq!(open(cx), was, "undone");
    assert_eq!(settle(cx, w, 8), 0, "at rest");
}

// ── idle ──────────────────────────────────────────────────────────────

/// No new idle frames: walking, a header's hover actions and the op log,
/// each once its fade is in, ask no frame. (A rail header's hover reflows
/// nothing either.)
#[gpui_kit::test]
fn the_walk_its_hover_and_the_op_log_ask_no_frame_at_rest(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    wait(cx, w, &view, 1000);
    assert_eq!(settle(cx, w, 8), 0, "the session at rest");
    enter(cx, w, &view);
    wait(cx, w, &view, 1000);
    assert_eq!(settle(cx, w, 8), 0, "walking, at rest");
    let At::Rail(r) = at(cx, &view).expect("walking") else { panic!("a rail section") };
    let sels = [format!("agent-section {r}"), format!("agent-head {r}"), format!("agent-body {r}")];
    let before: Vec<_> = sels.iter().map(|s| drawn(cx, w, s.clone())).collect();
    let h = drawn(cx, w, format!("agent-head {r}")).center();
    mouse(cx, w, h, "move");
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, format!("hover-acts {r}")), "its hover actions");
    let after: Vec<_> = sels.iter().map(|s| drawn(cx, w, s.clone())).collect();
    assert_eq!(before, after, "a rail header's hover reflows nothing");
    assert_eq!(settle(cx, w, 8), 0, "hovered, at rest");
    press(cx, w, "ctrl-o");
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, "oplog".into()));
    assert_eq!(settle(cx, w, 8), 0, "the op log, at rest");
}

// ── Shady #105 ────────────────────────────────────────────────────────

/// A past revision is in view in the pane acted on.
fn rev_open(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    view.read_with(cx, |v, _| v.panes.cur.walk.walk.as_ref().is_some_and(|w| w.rev.is_some()))
}

/// Re-folds started.
fn started(cx: &mut TestAppContext, view: &Entity<AppView>) -> u64 {
    view.read_with(cx, |v, _| v.refolds.started)
}

/// `j` until the walk is on `to`, painted.
fn walk_on(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, to: At) {
    for _ in 0..16 {
        if at(cx, view) == Some(to) {
            break;
        }
        press(cx, w, "j");
    }
    wait(cx, w, view, 300);
    assert_eq!(at(cx, view), Some(to), "walked to it");
}

/// P2-1: `[` and `]` act on the painted target as `z` does (§6.2): a block
/// just reached, its ring not painted on it yet, gets nothing; a
/// replacement waiting under the lease refuses them, an open view's `]`
/// too, with why, before the lease releases; painted again, they act.
#[gpui_kit::test]
fn revision_keys_act_on_the_painted_target_checked_before_the_lease_releases(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_r2", json!([{"op":"patch","id":"b","set":{"text":"B second"}}]));
    tail(cx, w, &view);
    enter(cx, w, &view);
    // Just reached: no ring painted on b yet.
    super::keys_tests::down(cx, w, "k");
    super::keys_tests::up(cx, w, "k");
    crate::app::next_press();
    assert_eq!(at(cx, &view), Some(At::Block(row("b"))));
    assert!(view.read_with(cx, |v, _| v.panes.cur.walk.walk.as_ref().is_some_and(|w| w.painted.get().is_none())), "not painted on b yet");
    let n0 = started(cx, &view);
    super::keys_tests::down(cx, w, "[");
    super::keys_tests::up(cx, w, "[");
    crate::app::next_press();
    assert!(!rev_open(cx, &view) && started(cx, &view) == n0, "nothing asked of a target not painted");
    // Painted: `[` opens r1.
    wait(cx, w, &view, 300);
    press(cx, w, "[");
    wait(cx, w, &view, 300);
    assert!(rev_open(cx, &view), "painted: r1 in view");
    // A replacement waiting under the pointer: `]` refuses, the view as it
    // was, and why; the key releases the lease.
    let at_ = head(cx, w, "b");
    mouse(cx, w, at_, "move");
    ops(cx, w, &view, "toolu_rr", json!([{"op":"replace","id":"b","kind":"note","body":{"text":"B again"}}]));
    assert!(waiting(cx, &view), "the board waits under the pointer");
    let n1 = started(cx, &view);
    press(cx, w, "]");
    assert!(rev_open(cx, &view) && started(cx, &view) == n1, "the view as it was");
    assert_eq!(note(cx, &view), Some((row("b"), crate::lease::CHANGED)), "and why");
    assert!(!waiting(cx, &view), "the key released the lease");
    // Painted again, the new incarnation's ring: `]` acts, the latest.
    away(cx, w);
    wait(cx, w, &view, 300);
    press(cx, w, "]");
    assert!(!rev_open(cx, &view), "painted again: the latest");
    // And with no view: a replacement waiting refuses `[`.
    ops(cx, w, &view, "toolu_r3", json!([{"op":"patch","id":"b","set":{"text":"B again, twice"}}]));
    wait(cx, w, &view, 300);
    let at_ = head(cx, w, "b");
    mouse(cx, w, at_, "move");
    ops(cx, w, &view, "toolu_r4", json!([{"op":"replace","id":"b","kind":"note","body":{"text":"B thrice"}}]));
    assert!(waiting(cx, &view));
    let n2 = started(cx, &view);
    press(cx, w, "[");
    assert!(!rev_open(cx, &view) && started(cx, &view) == n2, "no revision asked of a block replaced");
    assert_eq!(note(cx, &view), Some((row("b"), crate::lease::CHANGED)));
}

/// P2-2: a past label never wears the live body (§3.6): until the scratch
/// has r2 the block says `loading r2…`, its live title and body not
/// drawn; one that can't be had says `r2 unavailable`; a folded rail
/// section's summary is its revision's.
#[gpui_kit::test]
fn a_past_revision_never_wears_the_live_body(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_l2", json!([{"op":"patch","id":"b","set":{"text":"B second"}}]));
    ops(cx, w, &view, "toolu_l3", json!([{"op":"patch","id":"b","set":{"text":"B third"}}]));
    tail(cx, w, &view);
    let b = row("b");
    enter(cx, w, &view);
    press(cx, w, "k");
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("block-note {b}")) && has(cx, w, format!("block-title {b}")), "the latest drawn");
    // `[`, the frame drawn as the key's update flushes: the scratch hasn't
    // folded r2 yet.
    let ks = super::keys_tests::keystroke(cx, "[");
    cx.update_window(w, |_, window, cx| window.dispatch_keystroke(ks, cx)).expect("the window");
    assert!(has(cx, w, format!("block-past-wait {b} loading r2…")), "loading");
    assert!(!has(cx, w, format!("block-note {b}")) && !has(cx, w, format!("block-title {b}")), "nothing of the live block");
    super::keys_tests::up(cx, w, "[");
    wait(cx, w, &view, 300);
    crate::app::next_press();
    assert!(!has(cx, w, format!("block-past-wait {b} loading r2…")));
    assert!(has(cx, w, format!("block-note {b}")), "r2's body, once the scratch has it");
    // One the scratch can't have: unavailable, never the live block.
    view.update(cx, |v, cx| {
        if let Some(d) = v.refolds.done.as_mut() {
            d.state = Err(cards_spec::board::TooFar);
        }
        cx.notify();
    });
    settle(cx, w, 8);
    assert!(has(cx, w, format!("block-past-wait {b} r2 unavailable · too far back to show")), "unavailable");
    assert!(!has(cx, w, format!("block-note {b}")) && !has(cx, w, format!("block-title {b}")), "never the live block");
    press(cx, w, "escape");
    wait(cx, w, &view, 300);
    // A rail section folded, then its r1: the summary is r1's.
    ops(cx, w, &view, "toolu_rn", json!([{"op":"put","id":"rn","kind":"note","target":"rail","slot":"upper","body":{"text":"R first"}}]));
    ops(cx, w, &view, "toolu_rp", json!([{"op":"patch","id":"rn","set":{"text":"R second"}}]));
    let rn = row("rn");
    walk_on(cx, w, &view, At::Rail(rn));
    if !has(cx, w, format!("agent-summary {rn} R second")) {
        press(cx, w, "z");
        wait(cx, w, &view, 1000);
    }
    assert!(has(cx, w, format!("agent-summary {rn} R second")), "folded: the latest's summary");
    press(cx, w, "[");
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, format!("agent-summary {rn} R first")), "r1's own summary");
    assert!(!has(cx, w, format!("agent-summary {rn} R second")));
}

/// P2-4: the archive is its session's (§5.2): another session in the same
/// tab (its store reused) shows its own or none, and the first's comes
/// back with it.
#[gpui_kit::test]
fn the_archive_is_its_sessions_whatever_the_tab_shows(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(cx, w, &view, "toolu_oa", json!([{"op":"put","id":"onlya","kind":"note","body":{"text":"A's own"}}]));
    compact(cx, w, &view);
    assert!(copied(cx, w, &view).contains("A's own"));
    // Session B in the same tab: none yet.
    open_session(cx, w, &view, crate::demo::BLOCKS_TWIN, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    assert_eq!(copied_now(cx, w, &view), None, "B has none");
    press(cx, w, "ctrl-o");
    press(cx, w, "tab");
    assert!(has(cx, w, "oplog-archive-empty".into()));
    press(cx, w, "escape");
    // B compacted: its own.
    compact(cx, w, &view);
    let b = copied(cx, w, &view);
    assert!(b.contains(" · plan · ") && !b.contains("A's own"), "{b}");
    // A again: A's.
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    assert!(copied(cx, w, &view).contains("A's own"), "A's archive, back with A");
}

/// P2-4: every compaction replaces the archive, an empty predecessor with
/// an empty one.
#[gpui_kit::test]
fn every_compaction_replaces_the_archive_an_empty_predecessor_too(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    compact(cx, w, &view);
    assert!(copied(cx, w, &view).contains(" · plan · "), "the demo's blocks");
    // The successor's history: no block call, no board.
    let (seq, sid) = attachment(cx, &view);
    let update = synaps_client::HistoryUpdate::Replace { messages: vec![json!({"role": "user", "content": "after the compaction"})] };
    view.update(cx, |v, cx| v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx));
    cx.run_until_parked();
    wait(cx, w, &view, 300);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.drawn().is_none()), "no board");
    compact(cx, w, &view);
    assert_eq!(copied(cx, w, &view), "before compaction: blocks\n\nbefore compaction: op log\n", "replaced: empty");
}

/// P2-4: the archive is the predecessor's final board, folded to its end:
/// not the board painted while a newer one waits under the lease.
#[gpui_kit::test]
fn the_archive_is_the_final_predecessor_not_the_board_painted(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let at_ = head(cx, w, "b");
    mouse(cx, w, at_, "move");
    ops(cx, w, &view, "toolu_aw", json!([{"op":"replace","id":"b","kind":"note","body":{"text":"B again"}}]));
    assert!(waiting(cx, &view), "the board waits under the pointer");
    compact(cx, w, &view);
    away(cx, w);
    let all = copied(cx, w, &view);
    assert!(all.contains("B again") && !all.contains("B first"), "{all}");
}

/// D6a's visibility as D6b will answer it: a transcript block pinned into
/// the rail, a section moved to the other slot, one a tombstone.
struct Placed {
    placed: Vec<(Ref, Slot)>,
    tomb: Ref,
}

impl crate::agent_rail::RailVisibility for Placed {
    fn seen(&self, id: &Ref) -> crate::agent_rail::Seen {
        if *id == self.tomb {
            crate::agent_rail::Seen::Tombstone(crate::agent_rail::Tomb { text: "dismissed before compaction · u show".into(), token: 7 })
        } else {
            crate::agent_rail::Seen::Shown
        }
    }

    fn placed(&self) -> &[(Ref, Slot)] {
        &self.placed
    }
}

/// P2-5: the walk finds its targets where this desk draws them: a block
/// pinned into the rail folds there and D6b is asked of its slot as
/// drawn; a section moved to the end slot is asked of the end; a
/// tombstone is walked to, `u` names its token as painted, `z` nothing.
#[gpui_kit::test]
fn the_walk_finds_its_targets_where_this_desk_draws_them(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    ops(
        cx,
        w,
        &view,
        "toolu_rl",
        json!([
            {"op":"put","id":"r1","kind":"note","target":"rail","slot":"upper","body":{"text":"R one"}},
            {"op":"put","id":"r2","kind":"note","target":"rail","slot":"upper","body":{"text":"R two"}}
        ]),
    );
    let id = |s: &str| Ref::parse(s).expect("an id");
    let asks = Asks::default();
    view.update(cx, |v, cx| {
        v.agent_rail.vis = Some(Rc::new(Placed { placed: vec![(id("a"), Slot::Upper), (id("r1"), Slot::End)], tomb: id("r2") }));
        v.desires = Some(Box::new(asks.clone()));
        cx.notify();
    });
    wait(cx, w, &view, 1000);
    let order = view.read_with(cx, |v, _| v.walk_order());
    for r in ["a", "r1", "r2"] {
        assert!(order.contains(&At::Rail(row(r))), "{r} in the walk: {order:?}");
    }
    enter(cx, w, &view);
    // a, pinned into the rail: `z` folds the section drawn there.
    walk_on(cx, w, &view, At::Rail(row("a")));
    let open = |cx: &mut TestAppContext, r: &str| has(cx, w, format!("agent-body {}", row(r)));
    let was = open(cx, "a");
    press(cx, w, "z");
    wait(cx, w, &view, 1000);
    assert_eq!(view.read_with(cx, |v, _| v.agent_rail.lease.note), None, "the block as painted: no note");
    assert_ne!(open(cx, "a"), was, "folded where it is drawn");
    press(cx, w, "p");
    assert_eq!(asks.2.borrow().last().copied(), Some((Place::Rail(Slot::Upper), None)), "asked of its slot as drawn");
    // r2, a tombstone: walked to; `z` does nothing, `u` names its token.
    walk_on(cx, w, &view, At::Rail(row("r2")));
    assert!(ring(cx, w, row("r2")), "its ring");
    let asked = asks.0.borrow().len();
    press(cx, w, "z");
    press(cx, w, "p");
    assert_eq!(asks.0.borrow().len(), asked, "a tombstone: only x and u");
    press(cx, w, "u");
    assert_eq!(asks.2.borrow().last().copied(), Some((Place::Rail(Slot::Upper), Some(7))), "its token as painted");
    // r1, moved to the end slot: `m` names the end.
    walk_on(cx, w, &view, At::Rail(row("r1")));
    press(cx, w, "m");
    assert_eq!(asks.2.borrow().last().copied(), Some((Place::Rail(Slot::End), None)), "the slot the user put it in");
}

/// P2-6: undo puts back what the action set, only while the block still
/// shows it (§3.5): the agent's `collapse` since makes it a no-op that
/// says so, never a new fold.
#[gpui_kit::test]
fn an_agent_collapse_since_voids_the_undo(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    press(cx, w, "k");
    wait(cx, w, &view, 300);
    press(cx, w, "z");
    assert!(folded(cx, &view, "b"), "the user's fold");
    // The agent opens it: its collapse wins over the user's fold.
    ops(cx, w, &view, "toolu_co", json!([{"op":"collapse","id":"b","open":true}]));
    wait(cx, w, &view, 300);
    assert!(!folded(cx, &view, "b"), "the agent's collapse wins");
    press(cx, w, "ctrl-z");
    assert!(!folded(cx, &view, "b"), "not folded again: the fold no longer holds");
    assert_eq!(note(cx, &view), Some((row("b"), "changed since · not undone")), "and says so");
}

/// P2-6: the rail's fifth expansion folded back the oldest of the user's
/// four (§4); its undo folds the fifth back and gives the oldest its
/// place back.
#[gpui_kit::test]
fn undoing_a_fifth_expansion_gives_the_displaced_one_back(cx: &mut TestAppContext) {
    use crate::agent_rail::State;
    let (w, view, _events) = open_at(cx, (1280., 2400.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::RAIL, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    // The user's choices: plan folded; findings (oldest), retry, rot, file
    // expanded.
    view.update(cx, |v, cx| {
        let sid = v.rail_sid().expect("a session").to_string();
        let to = |v: &mut AppView, name: &str, open: bool| {
            let r = Ref::parse(name).expect("an id");
            for _ in 0..3 {
                let (_, lay) = v.rail_layout().expect("the rail");
                let s = lay.section(&r).expect("a section");
                if (open && s.state == State::Open && s.expanded_by_user) || (!open && s.state == State::Summary) {
                    return;
                }
                let _ = v.agent_rail.local_mut(&sid).toggle(&lay, &r);
            }
            panic!("{name} not set");
        };
        to(v, "plan", false);
        for name in ["findings", "retry", "rot", "file"] {
            to(v, name, true);
        }
        cx.notify();
    });
    wait(cx, w, &view, 1000);
    let open = |cx: &mut TestAppContext, r: &str| has(cx, w, format!("agent-body {}", row(r)));
    assert!(open(cx, "findings") && !open(cx, "plan"), "as set");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("plan")));
    press(cx, w, "z");
    wait(cx, w, &view, 1000);
    assert!(open(cx, "plan") && !open(cx, "findings"), "the fifth expansion folds the oldest back");
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 1000);
    assert!(!open(cx, "plan"), "the fifth folded back");
    assert!(open(cx, "findings"), "and the oldest given its place back");
}

/// P2-7: the op log's lines are the board's ≤ 256, each one's time looked
/// up by its call: unrelated history adds nothing to the work of drawing
/// it (D5's bounded views).
#[gpui_kit::test]
fn the_op_logs_view_is_bounded_by_its_lines_not_the_history(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    let work = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            crate::oplog::VISITS.with(|n| n.set(0));
            let n = v.oplog_lines().len();
            (n, crate::oplog::VISITS.with(std::cell::Cell::get))
        })
    };
    let (lines, before) = work(cx);
    assert!(lines > 0 && before <= lines, "{before} visits for {lines} lines");
    let rows0 = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.items().len());
    // Unrelated history: two thousand rows, no block op.
    let evs: Vec<Value> = (0..2000)
        .map(|k| llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "bash", "tool_id": format!("toolu_x{k}")})))
        .collect();
    heard(cx, w, &view, evs);
    let rows1 = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.items().len());
    assert!(rows1 >= rows0 + 2000, "{rows0} rows, then {rows1}");
    assert_eq!(work(cx), (lines, before), "the same work, whatever the history");
}

/// The rail demo, the user's choices made: plan folded, four sections
/// expanded (`oldest` first, then the rest of findings, retry, rot,
/// file); then `z` on plan in the walk, a fifth expansion that folds
/// `oldest` back (§4).
fn fifth_folded_back(cx: &mut TestAppContext, oldest: &str) -> W {
    use crate::agent_rail::State;
    let (w, view, _events) = open_at(cx, (1280., 2400.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::RAIL, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    let order: Vec<&str> =
        std::iter::once(oldest).chain(["findings", "retry", "rot", "file"].into_iter().filter(|n| *n != oldest)).collect();
    view.update(cx, |v, cx| {
        let sid = v.rail_sid().expect("a session").to_string();
        let to = |v: &mut AppView, name: &str, open: bool| {
            let r = Ref::parse(name).expect("an id");
            for _ in 0..3 {
                let (_, lay) = v.rail_layout().expect("the rail");
                let s = lay.section(&r).expect("a section");
                if (open && s.state == State::Open && s.expanded_by_user) || (!open && s.state == State::Summary) {
                    return;
                }
                let _ = v.agent_rail.local_mut(&sid).toggle(&lay, &r);
            }
            panic!("{name} not set");
        };
        to(v, "plan", false);
        for name in &order {
            to(v, name, true);
        }
        cx.notify();
    });
    wait(cx, w, &view, 1000);
    assert!(rail_open(cx, w, oldest) && !rail_open(cx, w, "plan"), "as set");
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Rail(row("plan")));
    press(cx, w, "z");
    wait(cx, w, &view, 1000);
    assert!(rail_open(cx, w, "plan") && !rail_open(cx, w, oldest), "the fifth expansion folds the oldest back");
    (w, view)
}

/// Whether rail section `name` draws its body.
fn rail_open(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> bool {
    has(cx, w, format!("agent-body {}", row(name)))
}

/// Shady #105 R2-3: the undo of a fifth expansion writes two sections and
/// checks both, all or nothing. The expansion it folded back changed
/// since (`rounds`: the agent's): undo leaves it folded, the fifth open,
/// and says `changed since · not undone`.
fn the_displaced_one_changed(cx: &mut TestAppContext, what: &str, rounds: Vec<Value>) {
    let (w, view) = fifth_folded_back(cx, "file");
    let was = view.read_with(cx, |v, _| v.rail_layout().and_then(|(_, l)| l.section(&Ref::parse("file").unwrap()).map(|s| s.s.block.tag)));
    turn(cx, w, &view);
    for (k, ops) in rounds.into_iter().enumerate() {
        round(cx, w, &view, &[(&format!("toolu_dc{k}"), ops, true)]);
    }
    wait(cx, w, &view, 1000);
    let (now, folded_at) = view
        .read_with(cx, |v, _| {
            v.rail_layout().and_then(|(_, l)| l.section(&Ref::parse("file").unwrap()).map(|s| (s.s.block.tag, s.s.block.folded_at)))
        })
        .expect("file laid out");
    assert!(now != was.expect("file was laid out") || folded_at.is_some(), "{what}: the agent's change landed");
    assert!(rail_open(cx, w, "plan") && !rail_open(cx, w, "file"), "{what}: as the agent left it");
    press(cx, w, "ctrl-z");
    wait(cx, w, &view, 1000);
    assert!(rail_open(cx, w, "plan"), "{what}: nothing undone, the fifth still open");
    assert!(!rail_open(cx, w, "file"), "{what}: the old choice never opens what is there now");
    assert_eq!(view.read_with(cx, |v, _| v.agent_rail.lease.note), Some((row("plan"), crate::walk::STALE)), "{what}: and says so");
}

#[gpui_kit::test]
fn undoing_a_fifth_expansion_refuses_if_the_displaced_one_was_replaced(cx: &mut TestAppContext) {
    let replace = json!([{"op":"replace","id":"file","kind":"file","body":{"path":"crates/relay/src/lock.rs","state":"editing"}}]);
    the_displaced_one_changed(cx, "replaced under its id", vec![replace]);
}

#[gpui_kit::test]
fn undoing_a_fifth_expansion_refuses_if_the_agent_collapsed_the_displaced_one(cx: &mut TestAppContext) {
    the_displaced_one_changed(cx, "collapsed by the agent", vec![json!([{"op":"collapse","id":"file","open":false}])]);
}

#[gpui_kit::test]
fn undoing_a_fifth_expansion_refuses_if_the_displaced_one_was_made_again(cx: &mut TestAppContext) {
    let again = json!([{"op":"put","id":"file","kind":"file","target":"rail","slot":"upper","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}}]);
    the_displaced_one_changed(cx, "removed and made again", vec![json!([{"op":"remove","id":"file"}]), again]);
}

/// Shady #105 R3: the expansion a fifth folded back, replaced by the agent
/// while the rail's lease holds (its board waiting, the old one still
/// drawn): the undo checks both sections on that newest bound board,
/// never the one drawn. Nothing undone, and once the board lands the
/// replacement stays folded.
#[gpui_kit::test]
fn undoing_a_fifth_expansion_refuses_if_the_displaced_one_was_replaced_under_the_lease(cx: &mut TestAppContext) {
    let (w, view) = fifth_folded_back(cx, "file");
    let file = Ref::parse("file").expect("an id");
    let laid =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.rail_layout().and_then(|(_, l)| l.section(&file).map(|s| s.s.block.tag)));
    let was = laid(cx).expect("file laid out");
    turn(cx, w, &view);
    // The pointer over the rail: its lease holds the replacement back.
    let over = drawn(cx, w, format!("agent-head {}", row("plan"))).center();
    mouse(cx, w, over, "move");
    let replace = json!([{"op":"replace","id":"file","kind":"file","body":{"path":"crates/relay/src/lock.rs","state":"editing"}}]);
    round(cx, w, &view, &[("toolu_dl", replace, true)]);
    assert!(waiting(cx, &view), "the replacement waits under the rail's lease");
    assert_eq!(laid(cx), Some(was), "the old file still drawn");
    assert!(rail_open(cx, w, "plan") && !rail_open(cx, w, "file"), "as the fifth left them");
    press(cx, w, "ctrl-z");
    assert!(!waiting(cx, &view), "the key released the lease");
    assert!(laid(cx).is_some_and(|t| t != was), "the replacement drawn");
    wait(cx, w, &view, 1000);
    assert!(!rail_open(cx, w, "file"), "the old choice never opens the replacement");
    assert!(rail_open(cx, w, "plan"), "nothing undone, the fifth still open");
    assert_eq!(view.read_with(cx, |v, _| v.agent_rail.lease.note), Some((row("plan"), crate::walk::STALE)), "and says so");
}

/// The successor's history (`successor`) after a compaction into
/// `journal`, in one batch with it, as a daemon can send them: the
/// capture begins with the predecessor's input, then the transcript lets
/// go of the predecessor's history, its own handle of the worker.
fn compaction_onto(cx: &mut TestAppContext, view: &Entity<AppView>, journal: &str, successor: Vec<Value>) -> Vec<synaps_client::Event> {
    let (seq, sid) = attachment(cx, view);
    let e = json!({"ev": "compaction_applied", "previous_session_id": sid, "session_id": journal, "chains_advanced": [], "msg_count": 4});
    let ev = synaps_client::protocol::decode_event(&e).expect("a compaction");
    vec![
        synaps_client::Event::session(seq, sid.clone(), 0, ev, std::sync::Arc::new(e)),
        synaps_client::Event::History {
            attach_seq: seq,
            session_id: sid,
            seq: 0,
            update: synaps_client::HistoryUpdate::Replace { messages: successor },
        },
    ]
}

/// The pane's history replaced by `messages`, its board folded.
fn history_now(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<Value>) {
    let (seq, sid) = attachment(cx, view);
    let update = synaps_client::HistoryUpdate::Replace { messages };
    view.update(cx, |v, cx| v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx));
    cx.run_until_parked();
    wait(cx, w, view, 300);
}

/// The history's worker, held weakly, and the events its journal holds.
fn worker_now(
    cx: &mut TestAppContext,
    view: &Entity<AppView>,
) -> (std::sync::Weak<std::sync::Mutex<synaps_client::blocks::Worker>>, usize) {
    view.read_with(cx, |v, _| {
        let t = &v.model.lives[v.panes.cur.live].transcript;
        (std::sync::Arc::downgrade(&t.blocks_input().expect("a board").worker), t.blocks_retained().journal)
    })
}

/// The blocks demo, its history replaced by a large one (the blocks
/// history, then 6,000 messages), the app's own disposer installed.
/// (Its fields drop in order: the disposer uninstalled, then shut down.)
struct Large {
    _in: synaps_client::blocks::dispose::Installed,
    disposer: synaps_client::blocks::dispose::Disposer,
    w: AnyWindowHandle,
    view: Entity<AppView>,
    sid: String,
}

fn large(cx: &mut TestAppContext) -> Large {
    let disposer = synaps_client::blocks::dispose::Disposer::start();
    let _in = disposer.install();
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    let sid = attachment(cx, &view).1;
    let big = synaps_client::synthetic::blocks_history().into_iter().chain(synaps_client::synthetic::history(6_000, 3)).collect();
    history_now(cx, w, &view, big);
    Large { _in, disposer, w, view, sid }
}

fn after(s: &str) -> Vec<Value> {
    vec![json!({"role": "user", "content": format!("after the compaction {s}")})]
}

fn archive_of(cx: &mut TestAppContext, l: &Large, journal: &str) -> Option<std::sync::Arc<crate::oplog::Archive>> {
    l.view.read_with(cx, |v, _| v.archives.of(&l.sid, journal).and_then(|h| h.archive.clone()))
}

/// Shady #105 R2-2: a compaction's capture can be the last owner of the
/// predecessor's history (the successor's replaced the transcript's while
/// it ran). Done, it lets go of it off the frame: handed to the app's
/// disposer (held here: the history alive in its queue, charged), the
/// frame's share of destruction a hand-over's.
#[gpui_kit::test]
fn a_done_captures_predecessor_is_let_go_off_the_frame(cx: &mut TestAppContext) {
    use synaps_client::blocks::dispose::destroyed;
    let l = large(cx);
    let (pred, journal) = worker_now(cx, &l.view);
    assert!(journal >= 6_000, "a large predecessor: {journal} events");
    let hold = l.disposer.hold();
    let (was, queued) = (destroyed(), l.disposer.backlog());
    let batch = compaction_onto(cx, &l.view, "j-one", after("one"));
    l.view.update(cx, |v, cx| v.apply_batch(batch, cx));
    cx.run_until_parked();
    wait(cx, l.w, &l.view, 300);
    assert!(archive_of(cx, &l, "j-one").is_some_and(|a| !a.blocks.is_empty()), "the capture is done");
    assert!(pred.upgrade().is_some(), "the predecessor's history not destroyed on the frame: in the disposer's queue");
    let now = l.disposer.backlog();
    assert!(now.items > queued.items && now.bytes >= queued.bytes + journal, "handed over, charged: {queued:?} then {now:?}");
    assert!(destroyed() - was <= 64, "the frame's share: hand-overs, {} elements", destroyed() - was);
    drop(hold);
    l.disposer.flush();
    assert!(pred.upgrade().is_none(), "the disposer let it go");
}

/// Shady #105 R2-2: the same, the capture superseded by the session's
/// next compaction before it ran (the same batch): its predecessor goes
/// to the disposer too, and it never publishes.
#[gpui_kit::test]
fn a_superseded_captures_predecessor_is_let_go_off_the_frame(cx: &mut TestAppContext) {
    use synaps_client::blocks::dispose::destroyed;
    let l = large(cx);
    let (pred, journal) = worker_now(cx, &l.view);
    assert!(journal >= 6_000, "a large predecessor: {journal} events");
    let hold = l.disposer.hold();
    let was = destroyed();
    let mut batch = compaction_onto(cx, &l.view, "j-two", after("two"));
    batch.extend(compaction_onto(cx, &l.view, "j-three", after("three")));
    l.view.update(cx, |v, cx| v.apply_batch(batch, cx));
    cx.run_until_parked();
    wait(cx, l.w, &l.view, 300);
    assert!(pred.upgrade().is_some(), "superseded: its predecessor in the disposer's queue, not destroyed on the frame");
    assert!(destroyed() - was <= 64, "the frame's share: hand-overs, {} elements", destroyed() - was);
    assert!(archive_of(cx, &l, "j-two").is_none(), "the superseded capture never published");
    assert!(
        archive_of(cx, &l, "j-three").is_some_and(|a| a.blocks.is_empty() && a.log.is_empty()),
        "the next one's: an empty predecessor's"
    );
    drop(hold);
    l.disposer.flush();
    assert!(pred.upgrade().is_none(), "the disposer let it go");
}
