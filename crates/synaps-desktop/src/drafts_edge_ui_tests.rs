//! D8a's window tests at its edges (Shady #114, round 1): a field holds
//! what its draft holds, pasted, cut, marked by an IME, across panes and
//! in its undo history (P2-2); a session followed to a new id takes its
//! drafts (P2-4); a pending remove before the bound board ends no draft
//! (P2-5); a held block stays where it was taken while the agent moves it
//! (P2-6); the pre-check's footer remeasures the row in every pane
//! (P2-7). Each fails on `044dc05`. Round 2 (Shady #114 at `8186107`):
//! held blocks stay in their places within their own anchor (R2 P2-1);
//! drafts of two incarnations meeting in a follow, one in the row, the
//! other in the dock, each its own Copy and Discard (R2 P2-2); a follow
//! keeps a held block at the anchor it was taken at, and one whose
//! anchor's call is gone where it is drawn, and while the pane watches a
//! worker (R2 P2-3); marked
//! text past a note's cap, and a composition's preedits in a field's
//! history, refused as they come (R2 P2-4).

use std::sync::Arc;

use cards_spec::block::Ref;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, EntityInputHandler as _, ListOffset, TestAppContext, point, px};
use serde_json::{Value, json};
use synaps_client::synthetic::{block_input, block_receipt};

use super::answer_tests::{attachment, has, heard, llm};
use super::blocks_ui_tests::{board, click, drawn, mouse, open_session, round, split, turn};
use super::keys_tests::press;
use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::drafts::{DOUBLED, Fate};

type W = (AnyWindowHandle, Entity<AppView>);

fn row(name: &str) -> u64 {
    crate::blocks::row_id(&Ref::parse(name).expect("an id"))
}

fn tail(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, view, 1000);
}

/// The blocks demo, a live turn, then `ops` at the transcript's end.
fn with(cx: &mut TestAppContext, ops: Value) -> W {
    with_at(cx, 1500., ops)
}

fn with_at(cx: &mut TestAppContext, width: f32, ops: Value) -> W {
    let (win, view, _events) = open_at(cx, (width, 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, win, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, win, &view);
    turn(cx, win, &view);
    round(cx, win, &view, &[("toolu_d0", ops, true)]);
    tail(cx, win, &view);
    (win, view)
}

fn three() -> Value {
    json!([
        {"op":"put","id":"n","kind":"note","title":"Root cause","body":{"text":"It is the retry loop"}},
        {"op":"put","id":"d","kind":"diff","title":"Fix: serialize","body":{"file":"src/a.rs","state":"proposed","hunks":[
            {"id":"h1","lines":["+a"],"state":"open"}]}},
        {"op":"put","id":"p","kind":"plan","title":"Close the race","body":{"steps":[
            {"id":"fix","label":"Fix","state":"todo"},{"id":"test","label":"Test","state":"todo"},{"id":"deploy","label":"Deploy","state":"todo"}]}}
    ])
}

fn planned(cx: &mut TestAppContext) -> W {
    with(cx, three())
}

fn ops(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: Value) {
    round(cx, w, view, &[(tool, ops, true)]);
    wait(cx, w, view, 300);
}

/// Ctrl+G, `k` `ups` times, then `e`: the walked block taken.
fn walk_take(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ups: usize) {
    press(cx, w, "ctrl-g");
    wait(cx, w, view, 300);
    for _ in 0..ups {
        press(cx, w, "k");
        wait(cx, w, view, 200);
    }
    press(cx, w, "e");
    wait(cx, w, view, 300);
}

/// The session on screen.
fn session(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].active.as_ref().expect("attached").session_id.clone())
}

/// Session `s`'s drafts of block `name`: their views' bodies and fates.
fn drafts_of(cx: &mut TestAppContext, view: &Entity<AppView>, s: &str, name: &str) -> Vec<(synaps_client::blocks::Body, Option<Fate>)> {
    let id = Ref::parse(name).expect("an id");
    view.read_with(cx, |v, _| v.block_drafts.of(s).filter(|d| d.id == id).map(|d| (d.view().body, d.ended)).collect())
}

fn label_of(cx: &mut TestAppContext, view: &Entity<AppView>, step: &str) -> String {
    let s = session(cx, view);
    let Some((synaps_client::blocks::Body::Plan(p), _)) = drafts_of(cx, view, &s, "p").pop() else { panic!("a plan's draft") };
    p.steps.iter().find(|x| x.id.as_str() == step).expect("the step").label.clone()
}

fn note_of(cx: &mut TestAppContext, view: &Entity<AppView>) -> (String, Option<String>) {
    let s = session(cx, view);
    let n = Ref::parse("n").expect("an id");
    view.read_with(cx, |v, _| {
        let d = v.block_drafts.of(&s).filter(|d| d.id == n).last().expect("the note's draft").view();
        let synaps_client::blocks::Body::Note(x) = &d.body else { panic!("a note") };
        (x.text.clone(), d.title.clone())
    })
}

fn line(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<Entity<gpui_kit::component::input::InputState>> {
    view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned())
}

fn area(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<Entity<gpui_kit::component::input::TextareaState>> {
    view.read_with(cx, |v, _| v.panes.cur.edit.inputs().1.cloned())
}

fn line_value(cx: &mut TestAppContext, f: &Entity<gpui_kit::component::input::InputState>) -> String {
    cx.read(|cx| f.read(cx).value().to_string())
}

fn area_value(cx: &mut TestAppContext, f: &Entity<gpui_kit::component::input::TextareaState>) -> String {
    cx.read(|cx| f.read(cx).value().to_string())
}

/// `text` put in at the caret (a paste, typed text, an IME's commit).
fn put_in(cx: &mut TestAppContext, w: AnyWindowHandle, f: &Entity<gpui_kit::component::input::InputState>, text: &str) {
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(None, text, window, cx))).expect("the window");
    settle(cx, w, 8);
}

fn put_in_area(cx: &mut TestAppContext, w: AnyWindowHandle, f: &Entity<gpui_kit::component::input::TextareaState>, text: &str) {
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(None, text, window, cx))).expect("the window");
    settle(cx, w, 8);
}

fn says(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| v.panes.cur.edit.note.as_ref().map(|n| n.1.clone()))
}

// ── P2-2: a field holds what its draft holds ──────────────────────────

/// Shady #114 P2-2: a paste past a label's or a title's 100 or 80
/// characters is refused in the field itself, which holds what the draft
/// holds, and the block says why; a paste past a note's 1500 is cut, in
/// the text area too, to what the draft took. Left and opened again, each
/// field holds the draft's value.
#[gpui_kit::test]
fn a_field_holds_what_its_draft_holds_pasted_or_cut(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 0);
    press(cx, w, "r");
    let f = line(cx, &view).expect("the label's field");
    put_in(cx, w, &f, &"x".repeat(1000));
    wait(cx, w, &view, 100);
    assert_eq!(line_value(cx, &f), label_of(cx, &view, "fix"), "the field holds the draft's label");
    assert_eq!(label_of(cx, &view, "fix"), "Fix", "the paste refused");
    assert_eq!(says(cx, &view).as_deref(), Some("at most 100 characters"));
    put_in(cx, w, &f, &"é".repeat(90));
    assert_eq!(line_value(cx, &f), label_of(cx, &view, "fix"));
    assert_eq!(label_of(cx, &view, "fix").chars().count(), 93, "one that fits is taken");
    press(cx, w, "enter");
    press(cx, w, "r");
    let f = line(cx, &view).expect("opened again");
    assert_eq!(line_value(cx, &f), label_of(cx, &view, "fix"), "opened again: the draft's");
    press(cx, w, "enter");
    // The note: its title (one line), its text (a text area).
    walk_take(cx, w, &view, 2);
    press(cx, w, "t");
    let f = line(cx, &view).expect("the title's field");
    put_in(cx, w, &f, &"t".repeat(500));
    wait(cx, w, &view, 100);
    assert_eq!(Some(line_value(cx, &f)), note_of(cx, &view).1.or(Some("Root cause".into())), "the title's field holds the draft's");
    assert_eq!(says(cx, &view).as_deref(), Some("at most 80 characters"));
    press(cx, w, "enter");
    press(cx, w, "r");
    let a = area(cx, &view).expect("the text's area");
    put_in_area(cx, w, &a, &"é".repeat(3000));
    let held = note_of(cx, &view).0;
    assert_eq!(held.chars().count(), 1500, "cut at 1500 characters");
    assert_eq!(area_value(cx, &a), held, "the text area holds what the draft took");
    assert_eq!(says(cx, &view).as_deref(), Some("at most 1500 characters"));
    press(cx, w, "escape");
    press(cx, w, "r");
    let a = area(cx, &view).expect("opened again");
    assert_eq!(area_value(cx, &a), note_of(cx, &view).0, "opened again: the draft's");
}

/// An IME's marked text past the label's cap is refused as it is marked,
/// and said; committed within it, the field and the draft agree. A
/// multibyte label past the draft's room: refused in its field.
#[gpui_kit::test]
fn marked_text_and_a_multibyte_label_at_the_cap_are_refused_in_the_field(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 0);
    press(cx, w, "r");
    let f = line(cx, &view).expect("the label's field");
    put_in(cx, w, &f, &"a".repeat(96));
    assert_eq!(label_of(cx, &view, "fix").chars().count(), 99);
    let mark = |cx: &mut TestAppContext, s: &str| {
        cx.update_window(w, |_, window, cx| f.update(cx, |st, cx| st.replace_and_mark_text_in_range(None, s, None, window, cx)))
            .expect("the window");
        settle(cx, w, 8);
    };
    mark(cx, "かな");
    wait(cx, w, &view, 100);
    assert_eq!(line_value(cx, &f).chars().count(), 99, "marked past the cap: refused as marked");
    assert_eq!(says(cx, &view).as_deref(), Some("at most 100 characters"), "and said");
    put_in(cx, w, &f, "かな");
    assert_eq!(line_value(cx, &f), label_of(cx, &view, "fix"), "its commit refused too: the field is the draft's");
    mark(cx, "か");
    put_in(cx, w, &f, "か");
    assert_eq!(label_of(cx, &view, "fix").chars().count(), 100, "committed within the cap: the draft's");
    assert_eq!(line_value(cx, &f), label_of(cx, &view, "fix"));
}

/// A multibyte label past the draft's room (§1.8: 8 KiB) is refused in
/// its field, which holds what the draft holds, and the block says why.
#[gpui_kit::test]
fn a_multibyte_label_past_the_drafts_room_is_refused_in_its_field(cx: &mut TestAppContext) {
    let steps: Vec<Value> = (0..20).map(|k| json!({"id": format!("s{k}"), "label": format!("S{k}"), "state": "todo"})).collect();
    let (w, view) = with(cx, json!([{"op":"put","id":"p","kind":"plan","title":"Twenty","body":{"steps": steps}}]));
    walk_take(cx, w, &view, 0);
    let big = "𝄞".repeat(100);
    let k = view.update(cx, |v, _| {
        let key = v.panes.cur.edit.at.as_ref().expect("editing").key;
        let d = v.block_drafts.by_key_mut(key).expect("the draft");
        (0..20).find(|k| d.rename(&cards_spec::block::ItemId::parse(&format!("s{k}")).expect("an id"), &big).is_err()).expect("the cap met")
    });
    view.update(cx, |v, cx| {
        v.panes.cur.edit.at.as_mut().expect("editing").cursor = cards_spec::block::ItemId::parse(&format!("s{k}"));
        cx.notify();
    });
    wait(cx, w, &view, 100);
    press(cx, w, "r");
    let f = line(cx, &view).expect("the label's field");
    // The label replaced (ASCII: its UTF-16 range is its bytes').
    let n = line_value(cx, &f).len();
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(Some(0..n), &big, window, cx))).expect("the window");
    settle(cx, w, 8);
    wait(cx, w, &view, 100);
    assert_eq!(line_value(cx, &f), label_of(cx, &view, &format!("s{k}")), "the field holds the draft's");
    assert_eq!(label_of(cx, &view, &format!("s{k}")), format!("S{k}"));
    assert_eq!(says(cx, &view).as_deref(), Some(crate::drafts::DRAFT_FULL));
    // Marked by an IME over the label: refused as it is marked.
    view.update(cx, |v, _| v.panes.cur.edit.note = None);
    let n = line_value(cx, &f).len();
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_and_mark_text_in_range(Some(0..n), &big, None, window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    wait(cx, w, &view, 100);
    assert_eq!(line_value(cx, &f), format!("S{k}"), "marked past the draft's room: refused in the field");
    assert_eq!(says(cx, &view).as_deref(), Some(crate::drafts::DRAFT_FULL), "and said");
    let own = view.read_with(cx, |v, _| v.editing_draft().map(crate::drafts::Draft::own_bytes)).expect("a draft");
    assert!(own <= crate::drafts::DRAFT_MAX_BYTES, "{own}");
}

/// One field a draft on the desk: opened in a second pane, the first's
/// closes, its text the draft's, and the second's holds it.
#[gpui_kit::test]
fn a_draft_is_edited_in_one_field_across_panes(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    split(cx, w, &view);
    tail(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    wait(cx, w, &view, 300);
    walk_take(cx, w, &view, 0);
    press(cx, w, "r");
    let f = line(cx, &view).expect("pane 1's field");
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(Some(0..3), "Fix A", window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    assert_eq!(label_of(cx, &view, "fix"), "Fix A");
    view.update(cx, |v, cx| v.focus_pane(panes[1], crate::pane::KeysTo::Pane, cx));
    wait(cx, w, &view, 300);
    view.update(cx, |v, cx| {
        v.in_pane(panes[1], |v| v.panes.cur.list.scroll_to_end());
        cx.notify();
    });
    wait(cx, w, &view, 500);
    walk_take(cx, w, &view, 0);
    press(cx, w, "r");
    let g = line(cx, &view).expect("pane 2's field");
    assert_eq!(line_value(cx, &g), "Fix A", "the draft's, typed in pane 1");
    let first = view.update(cx, |v, _| v.in_pane(panes[0], |v| v.panes.cur.edit.inputs().0.is_some()).unwrap_or(true));
    assert!(!first, "pane 1's field closed: one field a draft");
    cx.update_window(w, |_, window, cx| g.update(cx, |s, cx| s.replace_text_in_range(None, "B", window, cx))).expect("the window");
    settle(cx, w, 8);
    assert_eq!(label_of(cx, &view, "fix"), "Fix AB");
    let n = view.read_with(cx, |v, _| v.panes.iter().filter(|p| p.edit.at.as_ref().is_some_and(|e| e.field.is_some())).count());
    assert_eq!(n, 1, "one field on the desk");
}

/// The kit's undo history of a field is cleared once the changes it may
/// hold pass its budget: undone past that, the text stays where the
/// history was cleared (its caret, and the draft's, kept).
#[gpui_kit::test]
fn a_fields_undo_history_is_bounded(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 0);
    press(cx, w, "r");
    let f = line(cx, &view).expect("the label's field");
    for _ in 0..97 {
        put_in(cx, w, &f, "é");
    }
    assert_eq!(label_of(cx, &view, "fix").chars().count(), 100);
    for _ in 0..120 {
        press(cx, w, "ctrl-z");
    }
    let left = line_value(cx, &f);
    assert!(left.chars().filter(|c| *c == 'é').count() >= 80, "undone only past the history kept: {} left", left.chars().count());
    assert_eq!(left, label_of(cx, &view, "fix"), "the field and the draft agree");
}

// ── P2-4: a session followed to a new id takes its drafts ─────────────

/// The history the sessions hold: a plan, a note and a note to be removed.
fn conv() -> Vec<Value> {
    let ops = json!([
        {"op":"put","id":"n","kind":"note","title":"Root cause","body":{"text":"It is the retry loop"}},
        {"op":"put","id":"g","kind":"note","title":"Gone soon","body":{"text":"here a while"}},
        {"op":"put","id":"p","kind":"plan","title":"Close the race","body":{"steps":[
            {"id":"fix","label":"Fix","state":"todo"},{"id":"test","label":"Test","state":"todo"}]}}
    ]);
    vec![
        json!({"role": "user", "content": "Plan the fix"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_h0", "name": "cards_block", "input": block_input(ops.clone())}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_h0", "content": block_receipt(&ops)}]}),
    ]
}

/// The daemon's `Attached` for `id` with history `h`, on the attachment
/// on screen (a reattach, or a follow to a new id).
fn attach(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str, h: &[Value]) {
    let (seq, _) = attachment(cx, view);
    let frame = json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": id, "model": "m", "lifecycle": "live"},
        "conversation": {"messages_len": h.len(), "api_messages": h}});
    let Ok(synaps_client::protocol::DaemonFrame::Attached(attached)) = synaps_client::protocol::decode_frame(&frame.to_string()) else {
        panic!("an attach frame")
    };
    view.update(cx, |v, cx| {
        v.apply_batch(
            vec![synaps_client::Event::Attached { attach_seq: seq, attached, mode: synaps_client::AttachMode::Mirror, reconnect: false }],
            cx,
        )
    });
    cx.run_until_parked();
    board(cx, w, view);
    wait(cx, w, view, 600);
}

/// Shady #114 P2-4: the session on screen follows to a new id (the same
/// conversation), through the attach and the tab's follow: its held,
/// unfocused plan, the note open in its text field, and the removed
/// block's draft in the dock are the new id's, none left under the old;
/// the field is still open and typing reaches the draft; both panes draw
/// them; Copy and Discard act in the dock.
#[gpui_kit::test]
fn a_session_followed_to_a_new_id_takes_its_drafts(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    let s = session(cx, &view);
    attach(cx, w, &view, &s, &conv());
    split(cx, w, &view);
    tail(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    tail(cx, w, &view);
    // g: taken, edited, then removed by the agent: in the dock.
    walk_take(cx, w, &view, 1);
    press(cx, w, "t");
    let f = line(cx, &view).expect("g's title");
    put_in(cx, w, &f, " (mine)");
    press(cx, w, "enter");
    turn(cx, w, &view);
    let rm = json!([{"op":"remove","id":"g"}]);
    round(cx, w, &view, &[("toolu_rm", rm.clone(), true)]);
    wait(cx, w, &view, 300);
    assert_eq!(drafts_of(cx, &view, &s, "g").pop().map(|d| d.1), Some(Some(Fate::Gone)));
    // p: taken, struck, left held; n: its text field open, typed.
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    walk_take(cx, w, &view, 1);
    press(cx, w, "r");
    let a = area(cx, &view).expect("n's text area");
    put_in_area(cx, w, &a, " in the worker");
    assert!(note_of(cx, &view).0.starts_with(" in the worker"), "typed at the caret: {:?}", note_of(cx, &view));
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.count(&s)), 3);
    // The same conversation under a new id: its history, the live round's
    // call in it.
    let mut h = conv();
    h.push(json!({"role": "user", "content": "go on"}));
    h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_rm", "name": "cards_block", "input": block_input(rm.clone())}]}));
    h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_rm", "content": block_receipt(&rm)}]}));
    attach(cx, w, &view, "20261126-093000-bk01-t", &h);
    let t = session(cx, &view);
    assert_eq!(t, "20261126-093000-bk01-t", "followed");
    assert_eq!(view.read_with(cx, |v, _| (v.block_drafts.count(&s), v.block_drafts.count(&t))), (0, 3), "every draft followed");
    assert_eq!(drafts_of(cx, &view, &t, "p").pop().map(|d| d.1), Some(None), "p held");
    assert_eq!(drafts_of(cx, &view, &t, "g").pop().map(|d| d.1), Some(Some(Fate::Gone)), "g in the dock");
    let e = view.read_with(cx, |v, _| v.panes.cur.edit.at.as_ref().map(|e| (e.session.clone(), e.field.is_some())));
    assert_eq!(e, Some((t.clone(), false)), "n's editing followed, its field closed, its text the draft's");
    assert!(note_of(cx, &view).0.starts_with(" in the worker"), "{:?}", note_of(cx, &view));
    press(cx, w, "r");
    let a = area(cx, &view).expect("n's field open again");
    assert_eq!(area_value(cx, &a), note_of(cx, &view).0, "the draft's text");
    put_in_area(cx, w, &a, ", again");
    assert!(note_of(cx, &view).0.starts_with(", again in the worker"), "typing reaches the draft under the new id");
    let both = view.update(cx, |v, _| {
        panes.iter().all(|&p| v.in_pane(p, |v| v.block_shown(row("p")).is_some_and(|(_, s)| v.row_draft(&s).is_some())).unwrap_or(false))
    });
    assert!(both, "both panes draw p's draft");
    assert!(has(cx, w, "dock-draft Gone soon · this block is gone".into()), "the dock");
    let at = drawn(cx, w, "dock-button Discard Gone soon".into()).center();
    click(cx, w, at);
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.count(&t)), 2, "discarded from the dock");
}

/// A follow into a session that holds a draft of the same incarnation:
/// none lost, the one that came doubled, in the dock, its own Discard.
#[gpui_kit::test]
fn a_follow_onto_a_draft_of_the_same_block_doubles_it_in_the_dock(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    let s = session(cx, &view);
    attach(cx, w, &view, &s, &conv());
    tail(cx, w, &view);
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    // The new id holds a draft of p already, with a change (another pane's
    // of the same conversation, before: its draft kept on the desk).
    let t = "20261126-093000-bk01-t";
    view.update(cx, |v, _| {
        let (b, g) = {
            let d = v.editing_draft().expect("held");
            (d.base_block.clone(), d.generation)
        };
        let d = v.block_drafts.take(t, &b, g).expect("room");
        d.stop(&cards_spec::block::ItemId::parse("test").expect("an id")).expect("todo");
    });
    attach(cx, w, &view, t, &conv());
    assert_eq!(view.read_with(cx, |v, _| (v.block_drafts.count(&s), v.block_drafts.count(t))), (0, 2), "none lost");
    let mut fates: Vec<Option<Fate>> = drafts_of(cx, &view, t, "p").into_iter().map(|d| d.1).collect();
    fates.sort_by_key(Option::is_some);
    assert_eq!(fates, vec![None, Some(Fate::Doubled)], "the one that came is doubled");
    assert!(has(cx, w, format!("dock-draft Close the race · {DOUBLED}")), "in the dock");
    let at = drawn(cx, w, "dock-button Discard Close the race".into()).center();
    click(cx, w, at);
    let fates: Vec<Option<Fate>> = drafts_of(cx, &view, t, "p").into_iter().map(|d| d.1).collect();
    assert_eq!(fates, vec![None], "its Discard, the held one kept");
}

// ── P2-5: a pending remove before the bound board ─────────────────────

/// Shady #114 P2-5: a board published without the bound board (a job
/// begun before the take kept it), its overlay hiding the block a pending
/// `remove` names, ends no draft. The remove refused (rolled back): the
/// same draft, editable. Then one bound: gone.
#[gpui_kit::test]
fn a_pending_remove_before_the_bound_board_ends_no_draft(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    let s = session(cx, &view);
    round(cx, w, &view, &[("toolu_rm", json!([{"op":"remove","id":"p"}]), false)]);
    wait(cx, w, &view, 300);
    // That board as a job without the bound board would have published it.
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        let d = v.model.lives[live].transcript.drawn().cloned().expect("a board");
        assert!(crate::lease::find_any(&d, row("p")).is_none(), "the pending remove hides it");
        let mut bare = (*d).clone();
        bare.board = None;
        v.board_ready(live, Arc::new(bare), cx);
    });
    wait(cx, w, &view, 300);
    assert_eq!(drafts_of(cx, &view, &s, "p").pop().map(|d| d.1), Some(None), "not ended: the bound board can't say yet");
    heard(cx, w, &view, vec![llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_rm", "result": "cards_block: refused"}))]);
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert_eq!(drafts_of(cx, &view, &s, "p").pop().map(|d| d.1), Some(None), "rolled back: held");
    walk_take(cx, w, &view, 0);
    press(cx, w, "j");
    press(cx, w, "x");
    let Some((synaps_client::blocks::Body::Plan(pl), _)) = drafts_of(cx, &view, &s, "p").pop() else { panic!("a plan") };
    assert_eq!(pl.steps[1].state.as_str(), "skipped", "the same draft, editable");
    ops(cx, w, &view, "toolu_rm2", json!([{"op":"remove","id":"p"}]));
    assert_eq!(drafts_of(cx, &view, &s, "p").pop().map(|d| d.1), Some(Some(Fate::Gone)), "bound: gone");
}

// ── P2-6: a held block stays where it was taken ───────────────────────

/// The block rows the pane acted on draws, in order, by name.
fn order(cx: &mut TestAppContext, view: &Entity<AppView>, names: &[&str]) -> Vec<String> {
    view.read_with(cx, |v, _| {
        v.panes
            .cur
            .disp
            .iter()
            .filter(|d| matches!(d.kind, crate::turns::Kind::Block { .. }))
            .filter_map(|d| names.iter().find(|n| row(n) == d.id).map(|n| n.to_string()))
            .collect()
    })
}

fn in_rail(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> bool {
    let id = Ref::parse(name).expect("an id");
    view.read_with(cx, |v, _| {
        v.model.lives[v.panes.cur.live].transcript.drawn().is_some_and(|d| d.rail.iter().flatten().any(|s| s.block.id == id))
    })
}

/// Shady #114 P2-6 (§3.4): the agent moves a held plan to another anchor,
/// then to the rail, then folds it; the pointer's lease long over, the
/// draft stays where it was taken, whole, in both panes, never in the rail
/// too. Let go, the plan is where the agent put it, folded, once.
#[gpui_kit::test]
fn a_held_block_stays_where_it_was_taken_while_the_agent_moves_it(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    split(cx, w, &view);
    tail(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    tail(cx, w, &view);
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    ops(cx, w, &view, "toolu_x", json!([{"op":"put","id":"x","kind":"note","body":{"text":"after"}}]));
    let names = ["n", "d", "p", "x"];
    assert_eq!(order(cx, &view, &names), ["n", "d", "p", "x"]);
    ops(cx, w, &view, "toolu_m", json!([{"op":"move","id":"p"}]));
    // The pointer gone, its lease over.
    mouse(cx, w, point(px(2.), px(2.)), "move");
    wait(cx, w, &view, 3000);
    assert_eq!(order(cx, &view, &names), ["n", "d", "p", "x"], "moved by the agent: held where it was taken");
    let other = view.update(cx, |v, _| v.in_pane(panes[1], |v| v.block_shown(row("p")).map(|(ix, _)| ix)).flatten());
    let after = view.update(cx, |v, _| v.in_pane(panes[1], |v| v.block_shown(row("x")).map(|(ix, _)| ix)).flatten());
    assert!(other.is_some() && other < after, "the other pane too: {other:?} {after:?}");
    ops(cx, w, &view, "toolu_r", json!([{"op":"move","id":"p","target":"rail"}]));
    wait(cx, w, &view, 3000);
    assert_eq!(order(cx, &view, &names), ["n", "d", "p", "x"], "to the rail: held in the transcript");
    assert!(!in_rail(cx, &view, "p"), "and not in the rail too");
    ops(cx, w, &view, "toolu_c", json!([{"op":"collapse","id":"p"}]));
    wait(cx, w, &view, 3000);
    assert!(has(cx, w, format!("block-draft {}", row("p"))), "folded by the agent: the draft draws whole");
    assert_eq!(order(cx, &view, &names), ["n", "d", "p", "x"]);
    // Let go: where the agent put it, folded, once.
    press(cx, w, "escape");
    press(cx, w, "escape");
    board(cx, w, &view);
    wait(cx, w, &view, 1000);
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.len()), 0);
    assert!(in_rail(cx, &view, "p"), "in the rail, where the agent put it");
    assert_eq!(order(cx, &view, &names), ["n", "d", "x"], "not in the transcript too");
    let collapsed = view.read_with(cx, |v, _| {
        let id = Ref::parse("p").expect("an id");
        v.model.lives[v.panes.cur.live]
            .transcript
            .drawn()
            .and_then(|d| d.rail.iter().flatten().find(|s| s.block.id == id).map(|s| s.block.collapsed))
    });
    assert_eq!(collapsed, Some(true), "folded, as the agent left it");
}

// ── P2-7: the footer's height ─────────────────────────────────────────

/// Pane `q`'s list bounds of rows `ix` and `ix + 1`, if measured.
fn two_rows(
    cx: &mut TestAppContext,
    view: &Entity<AppView>,
    q: crate::pane::PaneId,
    ix: usize,
) -> Option<(gpui_kit::Bounds<gpui_kit::Pixels>, gpui_kit::Bounds<gpui_kit::Pixels>)> {
    view.update(cx, |v, _| v.in_pane(q, |v| v.panes.cur.list.bounds_for_item(ix).zip(v.panes.cur.list.bounds_for_item(ix + 1))).flatten())
}

/// Both panes at their list's end, drawn.
fn ends(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, panes: &[crate::pane::PaneId]) {
    for &q in panes {
        view.update(cx, |v, cx| {
            v.in_pane(q, |v| v.panes.cur.list.scroll_to_end());
            cx.notify();
        });
    }
    wait(cx, w, view, 600);
}

/// The draft row's height and the gap to the next row's top, in pane
/// `q` and in pane `r`, agree (each measured as drawn).
fn agree(cx: &mut TestAppContext, view: &Entity<AppView>, q: crate::pane::PaneId, r: crate::pane::PaneId, ix: (usize, usize), at: &str) {
    let (a, an) = two_rows(cx, view, q, ix.0).expect("drawn in the first pane");
    let (b, bn) = two_rows(cx, view, r, ix.1).expect("drawn in the second pane");
    assert!((a.size.height - b.size.height).abs() < px(0.5), "{at}: the row {a:?} {b:?}");
    let (ga, gb) = (an.top() - a.bottom(), bn.top() - b.bottom());
    assert!((ga - gb).abs() < px(0.5) && ga >= px(0.), "{at}: the next row's top {ga:?} {gb:?}");
}

const LONG1: &str = "rewrite_pool_retry_loop";
const LONG2: &str = "serialize_writes_queue";

/// Shady #114 P2-7: the bound board arrives (the pre-check: what fails)
/// with the rows' blocks as they were: the draft's row is measured again
/// in every pane, the one scrolled away from it too, and the row after it
/// is where it goes: `checking…`, then a failure footer that wraps, then
/// one line. The same while the board waits behind a pointer lease.
#[gpui_kit::test]
fn the_pre_checks_footer_remeasures_the_draft_row_in_every_pane(cx: &mut TestAppContext) {
    let notes: Vec<Value> =
        (0..24).map(|k| json!({"op":"put","id":format!("m{k}"),"kind":"note","body":{"text":format!("above {k}")}})).collect();
    let (w, view) = with_at(cx, 1100., Value::Array(notes));
    ops(
        cx,
        w,
        &view,
        "toolu_p",
        json!([
            {"op":"put","id":"p","kind":"plan","title":"Close the race","body":{"steps":[
                {"id":LONG1,"label":"Retry","state":"todo"},{"id":LONG2,"label":"Queue","state":"todo"},{"id":"ship","label":"Ship","state":"todo"}]}},
            {"op":"put","id":"x","kind":"note","body":{"text":"after"}}
        ]),
    );
    split(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    ends(cx, w, &view, &panes);
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    wait(cx, w, &view, 300);
    walk_take(cx, w, &view, 1);
    press(cx, w, "x");
    press(cx, w, "j");
    press(cx, w, "x");
    press(cx, w, "j");
    press(cx, w, "x");
    let p = row("p");
    let ix =
        |cx: &mut TestAppContext, q| view.update(cx, |v, _| v.in_pane(q, |v| v.block_shown(p).map(|(ix, _)| ix)).flatten().expect("drawn"));
    let real = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board"));
    let bare = |real: &Arc<synaps_client::blocks::Drawn>| {
        let mut b = (**real).clone();
        b.board = None;
        Arc::new(b)
    };
    let publish = |cx: &mut TestAppContext, d: Arc<synaps_client::blocks::Drawn>| {
        view.update(cx, |v, cx| {
            let live = v.panes.cur.live;
            v.board_ready(live, d, cx);
        });
    };
    // `checking…`, measured in both.
    publish(cx, bare(&real));
    ends(cx, w, &view, &panes);
    assert!(has(cx, w, format!("draft-footer {p} checking…")));
    let at = (ix(cx, panes[0]), ix(cx, panes[1]));
    agree(cx, &view, panes[0], panes[1], at, "checking…");
    let checking = two_rows(cx, &view, panes[1], at.1).expect("drawn").0.size.height;
    // Pane 2 scrolled away; the agent runs both struck steps: the bound
    // board, the same rows but for their states, then lands.
    view.update(cx, |v, cx| {
        v.in_pane(panes[1], |v| v.panes.cur.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) }));
        cx.notify();
    });
    wait(cx, w, &view, 600);
    ops(
        cx,
        w,
        &view,
        "toolu_t1",
        json!([{"op":"patch","id":"p","set":{format!("steps.{LONG1}.state"):"done", format!("steps.{LONG2}.state"):"done"}}]),
    );
    let footer = format!("draft-footer {p} 2 changes no longer apply: {LONG1} already ran · {LONG2} already ran");
    assert!(has(cx, w, footer.clone()), "the failure footer");
    let foot = drawn(cx, w, footer);
    assert!(foot.size.height > px(30.), "it wraps: {foot:?}");
    let cached = view.update(cx, |v, _| v.in_pane(panes[1], |v| v.panes.cur.list.bounds_for_item(at.1)).flatten());
    assert!(cached.is_none_or(|b| b.size.height > checking), "pane 2's row not left at `checking…`'s height: {cached:?}");
    ends(cx, w, &view, &panes);
    let at = (ix(cx, panes[0]), ix(cx, panes[1]));
    agree(cx, &view, panes[0], panes[1], at, "the wrapped failure");
    // Behind a pointer lease: the pointer over x; the agent grows it and
    // runs the third struck step: the board waits, the footer says one
    // more.
    let xb = drawn(cx, w, format!("block {}", row("x")));
    mouse(cx, w, point(xb.left() + px(40.), xb.top() + px(8.)), "move");
    wait(cx, w, &view, 100);
    let hov = view.read_with(cx, |v, _| v.panes.iter().map(|p| p.lease.hover.map(|h| h.row)).collect::<Vec<_>>());
    assert!(hov.contains(&Some(row("x"))), "the pointer over x: {hov:?} {xb:?}");
    round(
        cx,
        w,
        &view,
        &[(
            "toolu_t2",
            json!([
                {"op":"patch","id":"x","set":{"text":"after\nand\nnow\ntaller"}},
                {"op":"patch","id":"p","set":{"steps.ship.state":"done"}}
            ]),
            true,
        )],
    );
    wait(cx, w, &view, 100);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()), "the board waits under the pointer");
    let footer = format!("draft-footer {p} 3 changes no longer apply: {LONG1} already ran · {LONG2} already ran · ship already ran");
    assert!(has(cx, w, footer), "the footer of the board that waits");
    let at = (ix(cx, panes[0]), ix(cx, panes[1]));
    agree(cx, &view, panes[0], panes[1], at, "behind the lease");
}

/// Shady #114 P2-7's counterexample: the draft's row measured with
/// `checking…` in both panes, pane 2 scrolled on past it (the row above
/// its view, its height kept by the list), then the bound board lands with
/// the same rows: the footer loses its line, and pane 2's row is measured
/// again. Scrolled back up 30 px, the row after it is 30 px down.
#[gpui_kit::test]
fn the_bound_board_landing_remeasures_a_draft_row_scrolled_past(cx: &mut TestAppContext) {
    let mut calls: Vec<Value> = vec![json!({"op":"put","id":"p","kind":"plan","title":"Close the race","body":{"steps":[
        {"id":"fix","label":"Fix","state":"todo"},{"id":"test","label":"Test","state":"todo"}]}})];
    calls.push(json!({"op":"put","id":"x","kind":"note","body":{"text":"after"}}));
    calls.extend(
        (0..12).map(|k| json!({"op":"put","id":format!("y{k}"),"kind":"note","body":{"text":format!("below {k}\nline two\nline three")}})),
    );
    let (w, view) = with(cx, Value::Array(calls));
    split(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    let p = row("p");
    let ix = |cx: &mut TestAppContext, q, name: &str| {
        view.update(cx, |v, _| v.in_pane(q, |v| v.block_shown(row(name)).map(|(ix, _)| ix)).flatten().expect("drawn"))
    };
    let top_at = |cx: &mut TestAppContext, q, at: usize| {
        view.update(cx, |v, cx| {
            v.in_pane(q, |v| v.panes.cur.list.scroll_to(ListOffset { item_ix: at, offset_in_item: px(0.) }));
            cx.notify();
        });
    };
    // Both panes with the plan at their top; taken, a step struck.
    for &q in &panes {
        let at = ix(cx, q, "p");
        top_at(cx, q, at);
    }
    wait(cx, w, &view, 600);
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    wait(cx, w, &view, 300);
    walk_take_at(cx, w, &view, "p");
    press(cx, w, "x");
    let real = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board"));
    let publish = |cx: &mut TestAppContext, d: Arc<synaps_client::blocks::Drawn>| {
        view.update(cx, |v, cx| {
            let live = v.panes.cur.live;
            v.board_ready(live, d, cx);
        });
    };
    // The board without the bound board (the first job kept none yet):
    // `checking…`, measured in both panes.
    let mut bare = (*real).clone();
    bare.board = None;
    publish(cx, Arc::new(bare));
    for &q in &panes {
        let at = ix(cx, q, "p");
        top_at(cx, q, at);
    }
    wait(cx, w, &view, 600);
    assert!(has(cx, w, format!("draft-footer {p} checking…")));
    let h0 = |cx: &mut TestAppContext, q| {
        let at = ix(cx, q, "p");
        view.update(cx, |v, _| v.in_pane(q, |v| v.panes.cur.list.bounds_for_item(at)).flatten()).expect("measured").size.height
    };
    let checking = (h0(cx, panes[0]), h0(cx, panes[1]));
    assert!((checking.0 - checking.1).abs() < px(0.5), "{checking:?}");
    // Pane 2 on past it: x at its top, the plan above its view.
    let x2 = ix(cx, panes[1], "x");
    top_at(cx, panes[1], x2);
    wait(cx, w, &view, 600);
    assert_eq!(view.update(cx, |v, _| v.in_pane(panes[1], |v| v.panes.cur.list.logical_scroll_top().item_ix)), Some(x2));
    // The bound board lands: the same rows, the footer one line shorter.
    publish(cx, real);
    wait(cx, w, &view, 600);
    assert!(!has(cx, w, format!("draft-footer {p} checking…")), "checked");
    let ready = h0(cx, panes[0]);
    assert!(ready < checking.0 - px(4.), "a line shorter: {ready:?} {checking:?}");
    // Pane 2 scrolled back up 30 px: x is 30 px down its view.
    view.update(cx, |v, cx| {
        v.in_pane(panes[1], |v| v.panes.cur.list.scroll_by(px(-30.)));
        cx.notify();
    });
    wait(cx, w, &view, 600);
    let (xb, vb) = view
        .update(cx, |v, _| {
            v.in_pane(panes[1], |v| v.panes.cur.list.bounds_for_item(x2).zip(Some(v.panes.cur.list.viewport_bounds()))).flatten()
        })
        .expect("x drawn in pane 2");
    let down = xb.top() - vb.top();
    assert!((down - px(30.)).abs() < px(0.5), "x {down:?} down pane 2's view: its row above measured as `checking…` still");
    let shown =
        view.update(cx, |v, _| v.in_pane(panes[1], |v| v.panes.cur.list.bounds_for_item(x2 - 1)).flatten()).expect("the plan drawn");
    assert!((shown.size.height - ready).abs() < px(0.5), "{shown:?} {ready:?}");
}

/// Ctrl+G, then `k` until the walk is on block `name`, then `e`.
fn walk_take_at(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, name: &str) {
    press(cx, w, "ctrl-g");
    wait(cx, w, view, 300);
    for _ in 0..40 {
        if view.read_with(cx, |v, _| v.panes.cur.walk.walk.as_ref().is_some_and(|k| k.at == crate::walk::At::Block(row(name)))) {
            break;
        }
        press(cx, w, "k");
        wait(cx, w, view, 100);
    }
    press(cx, w, "e");
    wait(cx, w, view, 300);
}

// ── Round 2 (Shady #114 at 8186107) ───────────────────────────────────

/// Pane `q`'s block rows, in order, by name (of `names`).
fn order_in(cx: &mut TestAppContext, view: &Entity<AppView>, q: crate::pane::PaneId, names: &[&str]) -> Vec<String> {
    view.update(cx, |v, _| {
        v.in_pane(q, |v| {
            v.panes
                .cur
                .disp
                .iter()
                .filter(|d| matches!(d.kind, crate::turns::Kind::Block { .. }))
                .filter_map(|d| names.iter().find(|n| row(n) == d.id).map(|n| n.to_string()))
                .collect()
        })
        .unwrap_or_default()
    })
}

/// The pointer gone, its lease long over.
fn lease_over(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    mouse(cx, w, point(px(2.), px(2.)), "move");
    wait(cx, w, view, 3000);
}

/// Shady #114 R2 P2-1: `n`, first in its anchor, and `d` after it are
/// held, each changed; the agent moves `n` last in the same anchor, then
/// `d` after it. The pointer's lease long over, both panes draw them where
/// they were taken, `n` first. Let go of both: each where the agent put
/// it, in its order.
#[gpui_kit::test]
fn held_blocks_stay_in_their_places_within_their_own_anchor(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    split(cx, w, &view);
    tail(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    tail(cx, w, &view);
    let names = ["n", "d", "p"];
    assert_eq!(order(cx, &view, &names), names);
    walk_take_at(cx, w, &view, "d");
    press(cx, w, "n");
    walk_take_at(cx, w, &view, "n");
    press(cx, w, "t");
    let f = line(cx, &view).expect("n's title");
    put_in(cx, w, &f, " (mine)");
    press(cx, w, "enter");
    let s = session(cx, &view);
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.of(&s).filter(|d| d.changed()).count()), 2, "both held, changed");
    ops(cx, w, &view, "toolu_m1", json!([{"op":"move","id":"n","at":{"after":"p"}}]));
    ops(cx, w, &view, "toolu_m2", json!([{"op":"move","id":"d","at":{"after":"n"}}]));
    lease_over(cx, w, &view);
    for &q in &panes {
        assert_eq!(order_in(cx, &view, q, &names), names, "held where they were taken, n first: pane {q:?}");
    }
    // Let go of n (the pane's), then of d: the agent's order.
    press(cx, w, "escape");
    press(cx, w, "escape");
    walk_take_at(cx, w, &view, "d");
    press(cx, w, "escape");
    press(cx, w, "escape");
    board(cx, w, &view);
    lease_over(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.len()), 0, "both let go");
    for &q in &panes {
        assert_eq!(order_in(cx, &view, q, &names), ["p", "n", "d"], "let go: where the agent put them, pane {q:?}");
    }
}

/// Block `name` as the board on screen draws it, and the board's
/// generation.
fn drawn_block(
    cx: &mut TestAppContext,
    view: &Entity<AppView>,
    name: &str,
) -> (Arc<cards_spec::board::Block>, cards_spec::block::Generation) {
    let id = Ref::parse(name).expect("an id");
    view.read_with(cx, |v, _| {
        let d = v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board");
        let s = d.rows.values().flat_map(|r| r.blocks.iter()).find(|s| s.block.id == id).expect("drawn").block.clone();
        (s, d.generation.expect("a generation"))
    })
}

fn composer_text(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, cx| v.panes.cur.composer.read(cx).value().to_string())
}

const T: &str = "20261126-093000-bk01-t";

/// `S` holds a changed draft of `p` that went stale (the agent replaced
/// `p`), and `T` a changed one of `p`'s new incarnation, made first if
/// `stale_first`; then `S` follows to `T`. The two drafts' keys.
fn converge(cx: &mut TestAppContext, stale_first: bool) -> (W, u64, u64) {
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    let s = session(cx, &view);
    attach(cx, w, &view, &s, &conv());
    tail(cx, w, &view);
    let rp = json!([{"op":"replace","id":"p","kind":"plan","body":{"steps":[{"id":"fix","label":"Again","state":"todo"}]}}]);
    let (old, g) = drawn_block(cx, &view, "p");
    let fix = || cards_spec::block::ItemId::parse("fix").expect("an id");
    let take = |cx: &mut TestAppContext, sid: &str, b: &Arc<cards_spec::board::Block>, strike: bool| {
        view.update(cx, |v, _| {
            let d = v.block_drafts.take(sid, b, g).expect("room");
            if strike { d.strike(&fix()) } else { d.stop(&fix()) }.expect("todo");
            d.key
        })
    };
    // Made first: taken in the window, its keys the pane's.
    let ks = stale_first.then(|| {
        walk_take_at(cx, w, &view, "p");
        press(cx, w, "x");
        view.read_with(cx, |v, _| v.panes.cur.edit.at.as_ref().map(|e| e.key)).expect("taken")
    });
    turn(cx, w, &view);
    ops(cx, w, &view, "toolu_rp", rp.clone());
    let (new, _) = drawn_block(cx, &view, "p");
    assert_ne!(new.tag, old.tag, "a new incarnation");
    let kt = take(cx, T, &new, false);
    let ks = ks.unwrap_or_else(|| take(cx, &s, &old, true));
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    let fate = view.read_with(cx, |v, _| v.block_drafts.by_key(ks).map(|(_, d)| d.ended));
    assert!(matches!(fate, Some(Some(Fate::Rewritten(_)))), "S's went stale: {fate:?}");
    let mut h = conv();
    h.push(json!({"role": "user", "content": "go on"}));
    h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_rp", "name": "cards_block", "input": block_input(rp.clone())}]}));
    h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_rp", "content": block_receipt(&rp)}]}));
    attach(cx, w, &view, T, &h);
    assert_eq!(session(cx, &view), T, "followed");
    tail(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.count(T)), 2, "both kept");
    ((w, view), ks, kt)
}

/// After [`converge`]: the held draft draws in `p`'s row; the stale one is
/// in the dock, with its own Copy and Discard, no pane's keys on it;
/// neither is hidden.
fn one_in_the_row_one_in_the_dock(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ks: u64, kt: u64) {
    let keys: Vec<u64> = view.read_with(cx, |v, _| v.panes.iter().filter_map(|p| p.edit.at.as_ref().map(|e| e.key)).collect());
    assert!(!keys.contains(&ks), "no pane's keys on the docked one: {keys:?}");
    let rowed = view.update(cx, |v, _| v.block_shown(row("p")).and_then(|(_, s)| v.row_draft(&s).map(|d| d.key)));
    assert_eq!(rowed, Some(kt), "the held one draws in p's row");
    let says = view.read_with(cx, |v, _| v.block_drafts.by_key(ks).and_then(|(_, d)| d.ended).and_then(Fate::says)).expect("stale");
    assert!(has(cx, w, format!("dock-draft Close the race · {says}")), "the stale one in the dock");
    assert!(has(cx, w, "dock-button Copy to composer Close the race".into()) && has(cx, w, "dock-button Discard Close the race".into()));
}

/// Shady #114 R2 P2-2: the stale draft made first: one draft in the row,
/// the other in the dock; the dock's Copy takes the stale one's words and
/// lets it go, the row's held; that one's own keys let it go after.
#[gpui_kit::test]
fn drafts_of_two_incarnations_meeting_in_a_follow_are_each_reached_stale_first(cx: &mut TestAppContext) {
    let ((w, view), ks, kt) = converge(cx, true);
    one_in_the_row_one_in_the_dock(cx, w, &view, ks, kt);
    let at = drawn(cx, w, "dock-button Copy to composer Close the race".into()).center();
    click(cx, w, at);
    assert_eq!(composer_text(cx, &view), "Close the race → skip fix", "the stale one's words");
    let keys: Vec<u64> = view.read_with(cx, |v, _| v.block_drafts.of(T).map(|d| d.key).collect());
    assert_eq!(keys, vec![kt], "the held one kept");
    walk_take_at(cx, w, &view, "p");
    assert_eq!(view.read_with(cx, |v, _| v.panes.cur.edit.at.as_ref().map(|e| e.key)), Some(kt), "its keys: the row's");
    press(cx, w, "escape");
    press(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.count(T)), 0);
}

/// The held draft made first: the same; the dock's Discard lets the
/// stale one go, the held one in its row.
#[gpui_kit::test]
fn drafts_of_two_incarnations_meeting_in_a_follow_are_each_reached_held_first(cx: &mut TestAppContext) {
    let ((w, view), ks, kt) = converge(cx, false);
    one_in_the_row_one_in_the_dock(cx, w, &view, ks, kt);
    let at = drawn(cx, w, "dock-button Discard Close the race".into()).center();
    click(cx, w, at);
    let keys: Vec<u64> = view.read_with(cx, |v, _| v.block_drafts.of(T).map(|d| d.key).collect());
    assert_eq!(keys, vec![kt], "discarded from the dock, the held one kept");
    assert_eq!(composer_text(cx, &view), "", "nothing copied");
    let rowed = view.update(cx, |v, _| v.block_shown(row("p")).and_then(|(_, s)| v.row_draft(&s).map(|d| d.key)));
    assert_eq!(rowed, Some(kt), "still in its row");
}

/// A call `id` of `ops` in a history, and its receipt.
fn called(h: &mut Vec<Value>, id: &str, ops: &Value) {
    h.push(
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}]}),
    );
    h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": block_receipt(ops)}]}));
}

/// Shady #114 R2 P2-3: `p` taken under `S` at its anchor (the history's
/// first call), held; the agent puts `x` after it, then moves `p` to the
/// end, the move waiting. `S` follows to `T`, its rows numbered anew, the
/// move in its history; under `T` the agent moves `p` to the rail; `T`
/// follows to `U`. Through each, `p` stays at the anchor it was taken
/// at, in both panes, never in the rail too. Let go: in the rail, where
/// the agent put it.
#[gpui_kit::test]
fn a_follow_keeps_a_held_block_at_the_anchor_it_was_taken_at(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    let s = session(cx, &view);
    attach(cx, w, &view, &s, &conv());
    split(cx, w, &view);
    tail(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    tail(cx, w, &view);
    walk_take_at(cx, w, &view, "p");
    press(cx, w, "x");
    let (px_, pm, pr) = (
        json!([{"op":"put","id":"x","kind":"note","body":{"text":"after"}}]),
        json!([{"op":"move","id":"p"}]),
        json!([{"op":"move","id":"p","target":"rail"}]),
    );
    turn(cx, w, &view);
    ops(cx, w, &view, "toolu_x", px_.clone());
    ops(cx, w, &view, "toolu_m", pm.clone());
    lease_over(cx, w, &view);
    let names = ["n", "g", "p", "x"];
    assert_eq!(order(cx, &view, &names), names, "moved by the agent: held where it was taken");
    let held = |cx: &mut TestAppContext, view: &Entity<AppView>, at: &str| {
        for &q in &panes {
            assert_eq!(order_in(cx, view, q, &names), names, "{at}: at its anchor, pane {q:?}");
        }
        assert!(!in_rail(cx, view, "p"), "{at}: not in the rail too");
    };
    let mut h = conv();
    h.push(json!({"role": "user", "content": "go on"}));
    called(&mut h, "toolu_x", &px_);
    called(&mut h, "toolu_m", &pm);
    attach(cx, w, &view, T, &h);
    assert_eq!(session(cx, &view), T, "followed");
    lease_over(cx, w, &view);
    held(cx, &view, "followed to T");
    turn(cx, w, &view);
    ops(cx, w, &view, "toolu_r", pr.clone());
    lease_over(cx, w, &view);
    held(cx, &view, "to the rail under T");
    h.push(json!({"role": "user", "content": "go on"}));
    called(&mut h, "toolu_r", &pr);
    let u = "20261126-094500-bk01-u";
    attach(cx, w, &view, u, &h);
    assert_eq!(session(cx, &view), u, "followed again");
    lease_over(cx, w, &view);
    held(cx, &view, "followed to U");
    assert_eq!(drafts_of(cx, &view, u, "p").pop().map(|d| d.1), Some(None), "held all along");
    // Let go: where the agent put it.
    walk_take_at(cx, w, &view, "p");
    press(cx, w, "escape");
    press(cx, w, "escape");
    board(cx, w, &view);
    lease_over(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| v.block_drafts.len()), 0);
    assert!(in_rail(cx, &view, "p"), "let go: in the rail");
    assert_eq!(order(cx, &view, &names), ["n", "g", "x"], "not in the transcript too");
}

/// Shady #114 R2 P2-3's other case, apart from a renumbering: the anchor
/// itself gone (the history followed to, of the same generation, has no
/// row of the call `p` was taken at: another call put it). `p` is held
/// where the board draws it, that its place now, named by its row's call
/// there; drawn once, in its row. A guard: the rule as it was.
#[gpui_kit::test]
fn a_follow_without_the_anchors_call_holds_the_block_where_it_is_drawn(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    let s = session(cx, &view);
    attach(cx, w, &view, &s, &conv());
    tail(cx, w, &view);
    walk_take_at(cx, w, &view, "p");
    press(cx, w, "x");
    let h: Vec<Value> = serde_json::from_str(&serde_json::to_string(&conv()).expect("json").replace("toolu_h0", "toolu_h9")).expect("json");
    attach(cx, w, &view, T, &h);
    assert_eq!(session(cx, &view), T, "followed");
    lease_over(cx, w, &view);
    let p = Ref::parse("p").expect("an id");
    let (place, at) = view.read_with(cx, |v, _| {
        let d = v.block_drafts.in_row(T, &p).map(|d| d.place.clone());
        let at = v.model.lives[v.panes.cur.live]
            .transcript
            .items()
            .iter()
            .find(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == "toolu_h9"))
            .map(|i| i.id);
        (d, at)
    });
    let Some(Some(crate::drafts::Place::Row { item, call, .. })) = place else { panic!("held in a row: {place:?}") };
    assert_eq!((Some(item), call.as_deref()), (at, Some("toolu_h9")), "its place where the board draws it, named by that row's call");
    assert_eq!(order(cx, &view, &["n", "g", "p"]), ["n", "g", "p"], "drawn once, in its row");
}

/// Found in round 2, in R2 P2-3's path: the pane watching a worker (its
/// transcript on screen, not the session's), boards that land keep a
/// held block where it was taken: its rows are the session's, the
/// board's. Back on the thread: there still, and never moved.
#[gpui_kit::test]
fn a_held_block_stays_in_its_place_while_the_pane_watches_a_worker(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take_at(cx, w, &view, "p");
    press(cx, w, "x");
    ops(cx, w, &view, "toolu_x", json!([{"op":"put","id":"x","kind":"note","body":{"text":"after"}}]));
    ops(cx, w, &view, "toolu_m", json!([{"op":"move","id":"p"}]));
    lease_over(cx, w, &view);
    let names = ["n", "d", "p", "x", "y"];
    assert_eq!(order(cx, &view, &names), ["n", "d", "p", "x"], "held where it was taken");
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        v.model.lives[live].watch.open(&crate::watch::Candidate::of(2, "sleeper"), None, std::time::Instant::now());
        cx.notify();
    });
    wait(cx, w, &view, 300);
    assert_eq!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].watch.focus().map(str::to_string)), Some("sa_2".into()));
    ops(cx, w, &view, "toolu_y", json!([{"op":"put","id":"y","kind":"note","body":{"text":"while watched"}}]));
    board(cx, w, &view);
    lease_over(cx, w, &view);
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        v.model.lives[live].watch.focus_main();
        cx.notify();
    });
    board(cx, w, &view);
    lease_over(cx, w, &view);
    assert_eq!(order(cx, &view, &names), ["n", "d", "p", "x", "y"], "back on the thread: where it was taken");
}

/// The field's composition, as the platform's IME would make it: `text`
/// marked in place of what is marked (or selected).
fn mark_in<E: gpui_kit::EntityInputHandler>(cx: &mut TestAppContext, w: AnyWindowHandle, f: &Entity<E>, text: &str) {
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_and_mark_text_in_range(None, text, None, window, cx)))
        .expect("the window");
    settle(cx, w, 8);
}

fn marked<E: gpui_kit::EntityInputHandler>(cx: &mut TestAppContext, w: AnyWindowHandle, f: &Entity<E>) -> bool {
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.marked_text_range(window, cx))).expect("the window").is_some()
}

/// What the open field counts its history at (bytes).
fn history(cx: &mut TestAppContext, view: &Entity<AppView>) -> usize {
    view.read_with(cx, |v, _| v.panes.cur.edit.at.as_ref().and_then(|e| e.field.as_ref()).map_or(0, |f| f.history))
}

/// Shady #114 R2 P2-4: an IME marks a note's text past its 1,500
/// characters, and again and again: each refused as it is marked (no
/// `Change` comes), the text area the draft's, no composition left under
/// way, and said. A composition within the cap is the area's as it is
/// marked, and the draft's once committed, its characters whole. Closed
/// and opened again: the draft's.
#[gpui_kit::test]
fn marked_text_past_a_notes_cap_is_refused_as_it_is_marked(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take_at(cx, w, &view, "n");
    press(cx, w, "r");
    let a = area(cx, &view).expect("n's text area");
    let held = note_of(cx, &view).0;
    let cap = cards_spec::block::NOTE_TEXT_MAX;
    for (k, big) in ["a".repeat(cap + 1), "é".repeat(cap), "かな".repeat(cap)].iter().enumerate() {
        mark_in(cx, w, &a, big);
        wait(cx, w, &view, 100);
        assert_eq!(area_value(cx, &a), held, "marked past the cap ({k}): refused as marked");
        assert!(!marked(cx, w, &a), "({k}) no composition left under way");
        assert_eq!(says(cx, &view).as_deref(), Some("at most 1500 characters"), "({k}) said");
        assert_eq!(note_of(cx, &view).0, held, "({k}) the draft as it was");
    }
    mark_in(cx, w, &a, "かな");
    assert!(marked(cx, w, &a) && area_value(cx, &a).contains("かな"), "within the cap: the area's as marked");
    put_in_area(cx, w, &a, "かな");
    assert!(note_of(cx, &view).0.contains("かな"), "committed: the draft's, {:?}", note_of(cx, &view).0);
    assert_eq!(area_value(cx, &a), note_of(cx, &view).0);
    press(cx, w, "escape");
    press(cx, w, "r");
    let a = area(cx, &view).expect("opened again");
    assert_eq!(area_value(cx, &a), note_of(cx, &view).0, "opened again: the draft's");
}

/// Shady #114 R2 P2-4's history case: a label selected whole, then marked
/// over and over with 100-byte candidates, each one the field takes (no
/// `Change` comes); the kit keeps each preedit in its undo history, the
/// text it took out and the text it marked. The field counts them as they
/// come, and ends the composition (the field the draft's, said, its
/// history cleared) before what it keeps passes its budget. Another
/// composition, committed, is the draft's.
#[gpui_kit::test]
fn a_compositions_preedits_count_in_its_fields_history(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take_at(cx, w, &view, "p");
    press(cx, w, "r");
    let f = line(cx, &view).expect("the label's field");
    let label = label_of(cx, &view, "fix");
    let n = label.len();
    cx.update_window(w, |_, _, cx| f.update(cx, |s, cx| s.set_selected_range(0..n, cx))).expect("the window");
    settle(cx, w, 4);
    let mut kept = n;
    let mut ended = None;
    for k in 0..200 {
        mark_in(cx, w, &f, &(if k % 2 == 0 { "a" } else { "b" }).repeat(100));
        if !marked(cx, w, &f) {
            ended = Some(k);
            break;
        }
        kept += if k == 0 { 100 } else { 200 };
        let h = history(cx, &view);
        assert!(h >= kept, "preedit {k}: counted {h} of the {kept} bytes the kit keeps");
        assert!(h <= crate::draft_view::FIELD_HISTORY_MAX, "preedit {k}: under way past the budget ({h})");
    }
    let k = ended.expect("the composition ended before its history passed the budget");
    assert!(kept + 200 > crate::draft_view::FIELD_HISTORY_MAX, "not before it would: {k} preedits, {kept} bytes");
    assert_eq!(label_of(cx, &view, "fix"), label, "the draft as it was");
    assert_eq!(line_value(cx, &f), label, "the field the draft's");
    assert_eq!(says(cx, &view).as_deref(), Some(crate::draft_view::COMPOSED_LONG), "said");
    assert_eq!(history(cx, &view), 0, "its history cleared");
    mark_in(cx, w, &f, "かな");
    put_in(cx, w, &f, "かな");
    assert!(label_of(cx, &view, "fix").contains("かな"), "committed: the draft's");
    assert_eq!(line_value(cx, &f), label_of(cx, &view, "fix"));
}
