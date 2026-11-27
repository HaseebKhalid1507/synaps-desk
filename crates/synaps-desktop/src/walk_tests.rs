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

use cards_spec::block::Ref;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, point, px};
use serde_json::{Value, json};

use super::answer_tests::{ask_live, attachment, has, heard};
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
        Some(match v.panes.cur.walk.walk.as_ref().and_then(|w| w.rev) {
            Some(_) => "rev".into(),
            None => crate::walk::status_keys(v.desires.wired(), cfg!(target_os = "macos")),
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
    assert!(s.contains("j k walk") && s.contains("esc composer") && !s.contains("dismiss"), "{s}");
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
    assert!(!has(cx, w, format!("hover-act {b} ×")) && !has(cx, w, format!("hover-act {b} rail")), "D6b's not wired");
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
                let text = p.shown.as_ref().and_then(|s| match &s.block.body {
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

/// A stand-in for D6b's side: what it was asked, and its answer.
#[derive(Clone, Default)]
struct Asks(Rc<RefCell<Vec<Ask>>>, Rc<RefCell<Option<String>>>);

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
    fn undo(&mut self, a: &Aim, wish: Wish) -> Asked {
        self.0.borrow_mut().push((wish, format!("undo {}", a.block.block.id), a.session.to_string(), a.rail.is_some()));
        Asked::Done
    }
}

impl Asks {
    fn ask(&self, wish: Wish, a: &Aim) -> Asked {
        assert!(a.drawn.rows.values().any(|r| r.blocks.iter().any(|s| s.block.id == a.block.block.id)) || a.rail.is_some());
        self.0.borrow_mut().push((wish, a.block.block.id.to_string(), a.session.to_string(), a.rail.is_some()));
        match self.1.borrow().clone() {
            Some(why) => Asked::Refused(why),
            None => Asked::Done,
        }
    }
}

/// `x u p m` are D6b's: with D7's default nothing happens and nothing
/// offers them; wired, each asks D6b on the walked block's painted
/// target, a refusal says so, and Ctrl+Z undoes through it.
#[gpui_kit::test]
fn dismiss_show_pin_and_slot_go_to_d6bs_desires(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    enter(cx, w, &view);
    for k in ["x", "u", "p", "m"] {
        press(cx, w, k);
    }
    assert!(walking(cx, w, &view) && !folded(cx, &view, "c") && note(cx, &view).is_none(), "not wired: nothing");
    press(cx, w, "escape");
    let asks = Asks::default();
    view.update(cx, |v, _| v.desires = Box::new(asks.clone()));
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
/// predecessor's final blocks and op log, read-only, its text copied
/// whole. Its lines don't jump.
#[gpui_kit::test]
fn the_archive_tab_keeps_the_predecessors_blocks_and_log(cx: &mut TestAppContext) {
    let (w, view) = walked(cx);
    press(cx, w, "ctrl-o");
    press(cx, w, "tab");
    assert!(has(cx, w, "oplog-archive-empty".into()), "nothing before a compaction yet");
    press(cx, w, "escape");
    let sid = attachment(cx, &view).1;
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "compaction_applied", "previous_session_id": sid, "session_id": "j2walk", "chains_advanced": [], "msg_count": 4}),
        ],
    );
    let a = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].archive.clone()).expect("the archive");
    assert!(a.blocks.iter().any(|l| l.starts_with("b · note · ") && l.ends_with("· B first")), "{:#?}", a.blocks);
    assert!(a.log.iter().any(|l| l.split_whitespace().collect::<Vec<_>>().join(" ").contains("agent put c applied")), "{:#?}", a.log);
    assert!(a.bytes() <= crate::oplog::ARCHIVE_MAX);
    press(cx, w, "ctrl-o");
    press(cx, w, "tab");
    let b0 = a.blocks.iter().position(|l| l.starts_with("b · note")).expect("b");
    assert!(has(cx, w, format!("oplog-archive-block {b0} {}", a.blocks[b0])));
    let copy = drawn(cx, w, "oplog-copy".into()).center();
    click(cx, w, copy);
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(a.text()), "copied whole");
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
