//! D8b's window tests (synaps-cards `docs/blocks.md` v1.3 §3.2, §3.3,
//! §3.4, §3.6, §3.7, §5.2, §5.4; §8.2's D8 row, its second half): hand
//! back and restore through the window's own keys and pointer, the card
//! path's send, and what the block and its row say after.
//!
//! The list: §3.4's traces end to end (a skip the agent ran past, before
//! and after the send; an order over a tick; a replaced and a removed and
//! re-put plan; two desks each adding a step, and a forced collision;
//! `changed your edit`); §3.7's matrix as the desk feeds it (refused and
//! Retry, dequeued and Resend, held, an agent tick while sending, another
//! desk's disjoint and covering edits, Discard, a reattach mid-send, two
//! identical texts); compaction during a hold, a send and a queue; a
//! restore; the notices that ride it; one edit in flight per block.
//!
//! What would have gone to the daemon is the view's record
//! (`AppView::edit_sends`).

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use super::answer_tests::{attachment, has, heard};
use super::blocks_ui_tests::{board, turn};
use super::drafts_ui_tests::{compact, footer, row, walk_take};
use super::keys_tests::{keys_at, press};
use super::wait;
use super::walk_tests::{call, enter, landed, said_ok, settled_with, then, user, walk_on};
use crate::app::AppView;
use crate::handback::{EditOut, SENDING};
use crate::walk::At;

type W = (AnyWindowHandle, Entity<AppView>);

/// The plan `p` at r1: `fix`, `test`, `deploy`, todo.
fn plan_ops() -> Value {
    json!([{"op":"put","id":"p","kind":"plan","title":"Close the race","body":{"steps":[
        {"id":"fix","label":"Fix","state":"todo"},{"id":"test","label":"Test","state":"todo"},{"id":"deploy","label":"Deploy","state":"todo"}]}}])
}

/// The note `n` at r1.
fn note_ops() -> Value {
    json!([{"op":"put","id":"n","kind":"note","title":"Root cause","body":{"text":"It is the retry loop"}}])
}

fn rounds() -> Vec<Value> {
    let mut h: Vec<Value> = call("toolu_n", note_ops()).into();
    h.extend(call("toolu_p", plan_ops()));
    h
}

/// A settled session, idle, with the plan `p` (and the note `n` before
/// it), the list at its end.
fn planned(cx: &mut TestAppContext) -> W {
    settled_with(cx, rounds())
}

/// The history [`planned`] holds.
fn planned_history() -> Vec<Value> {
    let mut h = vec![user("start")];
    h.extend(rounds());
    h.push(said_ok());
    h
}

fn sends(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<EditOut> {
    view.read_with(cx, |v, _| v.edit_sends.clone())
}

fn last(cx: &mut TestAppContext, view: &Entity<AppView>) -> EditOut {
    sends(cx, view).last().cloned().expect("a hand-back went")
}

/// Block `name`'s incarnation tag, as the board binds it.
fn tag(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> String {
    view.read_with(cx, |v, _| v.block_shown(row(name)).expect("drawn").1.block.tag.to_string())
}

/// Step `step` of the plan `p` as the board draws it: its state and label.
fn on_board(cx: &mut TestAppContext, view: &Entity<AppView>, step: &str) -> (String, String) {
    view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(row("p")).expect("drawn");
        let synaps_client::blocks::Body::Plan(pl) = &s.block.body else { panic!("a plan") };
        let s = pl.steps.iter().find(|s| s.id.as_str() == step).expect("the step");
        (s.state.as_str().to_string(), s.label.clone())
    })
}

fn steps(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(row("p")).expect("drawn");
        let synaps_client::blocks::Body::Plan(pl) = &s.block.body else { panic!("a plan") };
        pl.steps.iter().map(|s| s.id.to_string()).collect()
    })
}

/// This desk's hand-back `body`: its delivery state, if the memo has it.
fn state(cx: &mut TestAppContext, view: &Entity<AppView>, body: &str) -> Option<cards_spec::memo::State> {
    view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref()?.session_id.clone();
        v.model.memos.edits.get(&s, body).map(|e| e.state)
    })
}

/// The hand-back row of user message `text`, as drawn: `p edited r1 ·
/// words · outcome`.
fn hand_back_row(cx: &mut TestAppContext, view: &Entity<AppView>, text: &str) -> Option<String> {
    let id =
        view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.items().iter().rev().find(|i| i.text == text).map(|i| i.id))?;
    assert!(view.read_with(cx, |v, _| v.panes.cur.disp.iter().any(|d| d.id == id)), "its row is in the list");
    view.read_with(cx, |v, _| {
        let it = v.model.lives[v.panes.cur.live].transcript.items().iter().find(|i| i.id == id).cloned()?;
        let d = v.model.lives[v.panes.cur.live].transcript.drawn()?;
        let o = d.outcomes.iter().find(|o| o.row == Some(id))?;
        let edit = cards_spec::block::message::parse(cards_spec::block::message::split(&it.text).0);
        Some(crate::handback::outcome_words(&o.outcome, edit.as_ref(), false))
    })
}

/// The row `text`'s hand-back row drawn with `outcome`.
fn row_says(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str, outcome: &str) -> bool {
    let Some(id) =
        view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.items().iter().rev().find(|i| i.text == text).map(|i| i.id))
    else {
        return false;
    };
    let line1 = text.split('\n').next().unwrap_or_default();
    let (head, words) = line1.split_once("] ").expect("line 1");
    has(cx, w, format!("hand-back {id} {} · {words} · {outcome}", head.trim_start_matches("[block ")))
}

/// The daemon refuses the oldest input this attachment sent.
fn refused(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, reason: &str) {
    let (seq, _) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let error = synaps_client::Error::Refused { command: Some("submit".into()), reason: reason.into() };
        v.apply_batch(vec![synaps_client::Event::Error { attach_seq: Some(seq), error }], cx);
    });
    cx.run_until_parked();
    wait(cx, w, view, 300);
}

/// The walk up to `to`.
fn walk_up(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, to: At) {
    for _ in 0..16 {
        if super::walk_tests::at(cx, view) == Some(to) {
            break;
        }
        press(cx, w, "k");
    }
    wait(cx, w, view, 300);
    assert_eq!(super::walk_tests::at(cx, view), Some(to), "walked to it");
}

/// A click on the drawn button `sel`.
fn button(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) {
    let at = super::blocks_ui_tests::drawn(cx, w, sel).center();
    super::blocks_ui_tests::click(cx, w, at);
}

fn idle(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "idle"})]);
    wait(cx, w, view, 300);
}

/// The history after the view's holds lands (`messages`), its board
/// drawn.
fn lands(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<Value>) {
    then(cx, w, view, messages);
    board(cx, w, view);
    wait(cx, w, view, 300);
}

/// An agent round of `ops` in the history (a call and its receipt).
fn agent(tool: &str, ops: Value) -> Vec<Value> {
    call(tool, ops).into()
}

/// Take `p`, the walk on the newest block (it), and `keys` pressed.
fn take_and(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, keys: &[&str]) {
    walk_take(cx, w, view, 0);
    for k in keys {
        press(cx, w, k);
    }
}

// ── handing back ─────────────────────────────────────────────────────

/// Enter on a held plan hands it back (§3.2, §3.7): one `Submit` on the
/// block's attachment (idle), its text the grammar's (§3.3), the keys back
/// to the composer. The draft then draws over the board marked `SENDING`,
/// and takes no key or new hold (one edit in flight per block). Its echo
/// puts it in the chat; once the board holds it the block draws the board
/// (the skip and the rename applied), and the hand-back's row says what it
/// did: `applied`. In the chat is never applied: the row, not the memo,
/// says so.
#[gpui_kit::test]
fn enter_hands_back_and_the_row_says_what_it_did(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["j", "j", "x"]);
    assert!(has(cx, w, format!("draft-button {p} Hand back on ready")), "the memo admits it: on");
    press(cx, w, "enter");
    let out = last(cx, &view);
    let of = tag(cx, &view, "p");
    let text =
        format!("[block p edited r1] Close the race → skip deploy\n{{\"of\":\"{of}\",\"set\":{{\"steps.deploy.state\":\"skipped\"}}}}");
    assert_eq!(out.text, text, "§3.3's grammar");
    assert_eq!((out.busy, out.body.as_str()), (false, text.as_str()), "idle: a Submit; no notice rides it");
    assert_eq!(sends(cx, &view).len(), 1, "once");
    assert_eq!(keys_at(cx, w, &view), (true, false), "the keys to the composer");
    assert!(has(cx, w, format!("draft-head {p} SENDING · r1")), "drawn over the board, marked sending");
    assert!(footer(cx, w, "p", "sending…"));
    assert_eq!(on_board(cx, &view, "deploy").0, "todo", "a drawing only: the board is the agent's");
    // One edit in flight: no new hold, no second send.
    walk_take(cx, w, &view, 0);
    assert!(footer(cx, w, "p", SENDING), "the block says why");
    press(cx, w, "escape");
    press(cx, w, "escape");
    assert_eq!(sends(cx, &view).len(), 1);
    // Its echo: in the chat. Until the board holds it the draft draws on,
    // never the board as it was.
    let (seq, sid) = attachment(cx, &view);
    view.update(cx, |v, cx| {
        let e = json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": text});
        let ev = synaps_client::protocol::decode_event(&e).expect("an event");
        v.apply_batch(vec![synaps_client::Event::session(seq, sid, 0, ev, std::sync::Arc::new(e))], cx);
        let s = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone();
        assert_eq!(v.model.memos.edits.get(&s, &text).map(|e| e.state), Some(cards_spec::memo::State::InChat));
        // A frame before the board's job lands judges the drafts.
        v.sync_drafts(cx);
        assert!(v.block_drafts.of(&s).any(|d| d.sent.is_some()), "in the chat, not yet on the board: the draft draws on");
    });
    // The board holds it, and draws it.
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::InChat));
    assert!(!has(cx, w, format!("block-draft {p}")), "the board holds it: the block draws the board");
    assert_eq!(on_board(cx, &view, "deploy").0, "skipped", "applied");
    assert!(!has(cx, w, format!("edit-delivery {p} in the chat")), "nothing to say on the block");
    lands(cx, w, &view, vec![user(&text), said_ok()]);
    assert!(row_says(cx, w, &view, &text, "applied"), "its row: what it did");
    assert_eq!(hand_back_row(cx, &view, &text).as_deref(), Some("applied"));
}

/// §3.4's first trace, both ways. `deploy` todo at r1, struck: handed back
/// after the agent's `doing` is drawn, the pre-check drops the skip (the
/// rest goes); sent before it landed, the fold skips it as a conflict, and
/// the row says `deploy already ran`. Either way `deploy` stays the
/// agent's.
#[gpui_kit::test]
fn a_skip_the_agent_ran_past_is_dropped_before_the_send_and_a_conflict_after(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    // After: the pre-check.
    take_and(cx, w, &view, &["x", "j", "j", "x"]);
    lands(cx, w, &view, agent("toolu_t1", json!([{"op":"patch","id":"p","set":{"steps.deploy.state":"doing"}}])));
    assert!(footer(cx, w, "p", "1 change no longer applies: deploy already ran"));
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    assert!(text.starts_with("[block p edited r1] Close the race → skip fix\n"), "the skip of deploy left out: {text}");
    assert!(!text.contains("deploy"), "{text}");
    lands(cx, w, &view, vec![user(&text), said_ok()]);
    assert!(row_says(cx, w, &view, &text, "applied"));
    assert_eq!(on_board(cx, &view, "deploy").0, "doing");
    // Before: the fold's guard.
    take_and(cx, w, &view, &["j", "x"]);
    press(cx, w, "enter");
    let skip = last(cx, &view).text;
    assert!(skip.starts_with("[block p edited r3] Close the race → skip test\n"), "r1, the tick r2, the skip r3: {skip}");
    let mut h = agent("toolu_t2", json!([{"op":"patch","id":"p","set":{"steps.test.state":"done"}}]));
    h.extend([user(&skip), said_ok()]);
    lands(cx, w, &view, h);
    assert!(row_says(cx, w, &view, &skip, "applied 0 of 1 · test already ran"), "a conflict: its row says why");
    assert_eq!(on_board(cx, &view, "test").0, "done", "the agent's truth");
    assert!(!has(cx, w, format!("block-draft {p}")));
}

/// §3.4's second trace: the user reorders while `test` goes `doing`: the
/// order applies, `test` stays `doing`.
#[gpui_kit::test]
fn an_order_lands_over_the_agents_tick(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["j", "shift-j"]);
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    assert!(text.contains("\"order\":{\"steps\":[\"fix\",\"deploy\",\"test\"]}"), "{text}");
    let mut h = agent("toolu_t", json!([{"op":"patch","id":"p","set":{"steps.test.state":"doing"}}]));
    h.extend([user(&text), said_ok()]);
    lands(cx, w, &view, h);
    assert!(row_says(cx, w, &view, &text, "applied"));
    assert_eq!(steps(cx, &view), ["fix", "deploy", "test"]);
    assert_eq!(on_board(cx, &view, "test").0, "doing");
}

/// §3.4's third and fourth traces: the agent replaces `p` (keeping `fix`
/// for a new task), or removes and puts it again, while a hand-back is on
/// its way: nothing lands on the new plan, and the row says
/// `not applied: the block was rewritten`. The sent draft gives way to the
/// board; the delivery carries on, in the chat.
#[gpui_kit::test]
fn a_hand_back_to_a_replaced_or_re_put_plan_lands_nothing(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    let mut h = agent(
        "toolu_r",
        json!([{"op":"replace","id":"p","kind":"plan","title":"Another","body":{"steps":[{"id":"fix","label":"Another task","state":"todo"}]}}]),
    );
    h.extend([user(&text), said_ok()]);
    lands(cx, w, &view, h);
    assert!(row_says(cx, w, &view, &text, "not applied: the block was rewritten"));
    assert_eq!(on_board(cx, &view, "fix"), ("todo".into(), "Another task".into()), "nothing on the new plan");
    assert!(!has(cx, w, format!("block-draft {p}")), "its block ended: the board draws");
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::InChat), "the delivery carried on");
    // Removed, then put again: another incarnation.
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let again = last(cx, &view).text;
    let mut h = agent("toolu_x", json!([{"op":"remove","id":"p"}]));
    h.extend(agent("toolu_y", plan_ops()));
    h.extend([user(&again), said_ok()]);
    lands(cx, w, &view, h);
    assert!(row_says(cx, w, &view, &again, "not applied: the block was rewritten"));
    assert_eq!(on_board(cx, &view, "fix").0, "todo");
}

/// §5.4 and §3.4's sixth trace: another hand-back lands on the block this
/// desk holds: `another edit arrived · r2` (never "on another desk"). Two
/// desks each add a step: the ids differ, both land. A forced collision
/// (the other's added step has this one's id) skips this one's, visibly.
#[gpui_kit::test]
fn another_edit_arrives_and_two_added_steps_both_land_a_collision_skipped(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    let of = tag(cx, &view, "p");
    take_and(cx, w, &view, &["a"]);
    let f = view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned()).expect("the added step's field");
    cx.update_window(w, |_, window, cx| {
        f.update(cx, |s, cx| gpui_kit::EntityInputHandler::replace_text_in_range(s, None, "Check logs", window, cx))
    })
    .expect("the window");
    press(cx, w, "enter");
    let other = format!(
        "[block p edited r1] Close the race → add “Docs”\n{{\"of\":\"{of}\",\"append\":{{\"steps\":[{{\"id\":\"u-zzzzzz\",\"label\":\"Docs\",\"state\":\"todo\"}}]}}}}"
    );
    lands(cx, w, &view, vec![user(&other), said_ok()]);
    assert!(row_says(cx, w, &view, &other, "applied"), "the other desk's");
    assert!(has(cx, w, format!("draft-head {p} EDITING · r1 · another edit arrived · r2")));
    press(cx, w, "enter");
    let mine = last(cx, &view).text;
    assert!(mine.contains("“Check logs”"), "{mine}");
    lands(cx, w, &view, vec![user(&mine), said_ok()]);
    assert!(row_says(cx, w, &view, &mine, "applied"));
    let ids = steps(cx, &view);
    assert_eq!(ids.len(), 5, "both landed: {ids:?}");
    // A forced collision: the other's step has the id of this desk's.
    take_and(cx, w, &view, &["a"]);
    let f = view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned()).expect("the field");
    cx.update_window(w, |_, window, cx| {
        f.update(cx, |s, cx| gpui_kit::EntityInputHandler::replace_text_in_range(s, None, "Again", window, cx))
    })
    .expect("the window");
    press(cx, w, "enter");
    press(cx, w, "enter");
    let mine = last(cx, &view).text;
    let added = mine.split("\"id\":\"").nth(1).and_then(|r| r.split('"').next()).expect("its u- id").to_string();
    let theirs = format!(
        "[block p edited r3] Close the race → add “Theirs”\n{{\"of\":\"{of}\",\"append\":{{\"steps\":[{{\"id\":\"{added}\",\"label\":\"Theirs\",\"state\":\"todo\"}}]}}}}"
    );
    lands(cx, w, &view, vec![user(&theirs), user(&mine), said_ok()]);
    assert!(
        row_says(cx, w, &view, &mine, "applied 0 of 2 · an added step collided · add it again"),
        "skipped, visibly: {:?}",
        hand_back_row(cx, &view, &mine)
    );
}

/// §3.4 step 6: once the agent overwrites a path the user wrote last, the
/// block's header says `changed your edit` (its path, for the hover) and
/// the row `1 overwritten by the agent`, until the user next focuses the
/// block: the walk's ring on it.
#[gpui_kit::test]
fn changed_your_edit_until_the_user_focuses_the_block(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["r"]);
    let f = view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned()).expect("the label's field");
    cx.update_window(w, |_, window, cx| {
        f.update(cx, |s, cx| gpui_kit::EntityInputHandler::replace_text_in_range(s, Some(0..3), "Fix it now", window, cx))
    })
    .expect("the window");
    press(cx, w, "enter");
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    lands(cx, w, &view, vec![user(&text), said_ok()]);
    assert_eq!(on_board(cx, &view, "fix").1, "Fix it now");
    assert!(!has(cx, w, format!("block-changed {p} steps.fix.label")), "nothing overwritten yet");
    lands(cx, w, &view, agent("toolu_o", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race"}}])));
    assert!(has(cx, w, format!("block-changed {p} steps.fix.label")), "the chip, its path");
    assert!(row_says(cx, w, &view, &text, "applied · 1 overwritten by the agent"));
    enter(cx, w, &view);
    walk_on(cx, w, &view, At::Block(p));
    wait(cx, w, &view, 300);
    assert!(!has(cx, w, format!("block-changed {p} steps.fix.label")), "focused: seen");
    press(cx, w, "escape");
    wait(cx, w, &view, 300);
    assert!(!has(cx, w, format!("block-changed {p} steps.fix.label")), "and it stays seen");
}

// ── delivery (§3.7) ──────────────────────────────────────────────────

/// A hand-back refused: not sent, the block's line says why, with Retry
/// and Discard; Retry sends the same body once more; Discard drops the
/// entry and the line.
#[gpui_kit::test]
fn a_refused_hand_back_is_not_sent_and_retry_sends_it_again(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    refused(cx, w, &view, "input is owned by client #1");
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::NotSent));
    assert!(!has(cx, w, format!("block-draft {p}")), "not in flight: the board draws");
    let cause = view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone();
        v.model.memos.edits.get(&s, &text).and_then(|e| e.cause.as_ref().map(cards_spec::memo::Cause::line)).unwrap()
    });
    assert!(cause.contains("input is owned by client #1"), "{cause}");
    assert!(has(cx, w, format!("edit-delivery {p} hand-back not sent · {cause}")), "the block's line says why");
    assert!(has(cx, w, format!("edit-button {p} Retry")) && has(cx, w, format!("edit-button {p} Discard")));
    button(cx, w, format!("edit-button {p} Retry"));
    assert_eq!(sends(cx, &view).len(), 2);
    assert_eq!(last(cx, &view).body, text, "the same body");
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::Sending));
    refused(cx, w, &view, "busy");
    button(cx, w, format!("edit-button {p} Discard"));
    assert_eq!(state(cx, &view, &text), None, "discarded");
    assert!(!has(cx, w, format!("edit-delivery {p} hand-back not sent")));
}

/// Steered into a running turn: queued at its echo; the daemon dequeues
/// it (a cancel): checking, then on the settled view without it, not
/// confirmed, with Resend; the Resend goes, once, the same body. Held:
/// a turn that ends with it still in the slot says so.
#[gpui_kit::test]
fn dequeued_it_is_not_confirmed_and_resend_sends_it_held_it_says_so(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    turn(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let out = last(cx, &view);
    assert!(out.busy, "a turn runs: a Steer");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("draft-head {p} QUEUED · r1")));
    heard(cx, w, &view, vec![json!({"ev": "dequeued", "text": out.text})]);
    idle(cx, w, &view);
    wait(cx, w, &view, 300);
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed));
    assert!(has(cx, w, format!("edit-button {p} Resend")));
    button(cx, w, format!("edit-button {p} Resend"));
    assert_eq!(sends(cx, &view).len(), 2, "once");
    assert_eq!(last(cx, &view).body, out.body);
    // Held: the turn ends, its slot still holding ours.
    turn(cx, w, &view);
    let again = last(cx, &view);
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": again.text, "delivered": true})]);
    heard(cx, w, &view, vec![super::answer_tests::slot_digest(&planned_history(), Some(&again.text)), json!({"ev": "idle"})]);
    wait(cx, w, &view, 300);
    assert_eq!(state(cx, &view, &again.body), Some(cards_spec::memo::State::Held));
    assert!(has(cx, w, format!("edit-delivery {p} held · it goes in after the agent's next turn")));
}

/// An agent tick while the hand-back is sending moves nothing of its
/// delivery: its base is what was sent (r1), and it reaches the chat; the
/// tick and the skip both stand.
#[gpui_kit::test]
fn an_agent_tick_while_sending_moves_nothing(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["j", "j", "x"]);
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    lands(cx, w, &view, agent("toolu_t", json!([{"op":"patch","id":"p","set":{"steps.fix.state":"doing"}}])));
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::Sending), "a tick never moves it");
    view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone();
        assert_eq!(v.model.memos.edits.get(&s, &text).unwrap().base, 1, "its base as sent");
    });
    lands(cx, w, &view, vec![user(&text), said_ok()]);
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::InChat));
    assert!(row_says(cx, w, &view, &text, "applied"));
    assert_eq!((on_board(cx, &view, "fix").0, on_board(cx, &view, "deploy").0), ("doing".into(), "skipped".into()));
}

/// Another desk's edits (§3.7, coverage): a disjoint one leaves this
/// desk's entry, not confirmed, with its Resend; a covering one retires
/// it: the block's line says a later edit covered it, with no Resend.
#[gpui_kit::test]
fn another_desks_disjoint_edit_leaves_the_entry_and_its_covering_edit_retires_it(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    let of = tag(cx, &view, "p");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    refused(cx, w, &view, "the link dropped");
    let other =
        |r: u32, set: &str, words: &str| format!("[block p edited r{r}] Close the race → {words}\n{{\"of\":\"{of}\",\"set\":{{{set}}}}}");
    let disjoint = other(1, "\"steps.test.stop\":true", "stop before test");
    lands(cx, w, &view, vec![user(&disjoint), said_ok()]);
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::NotSent), "disjoint: it stays");
    assert!(has(cx, w, format!("edit-button {p} Retry")));
    let covering = other(2, "\"steps.fix.state\":\"skipped\",\"steps.test.stop\":false", "skip fix · no stop before test");
    lands(cx, w, &view, vec![user(&covering), said_ok()]);
    assert!(has(cx, w, format!("edit-delivery {p} covered by a later edit in the chat")), "retired, and says whose");
    assert!(!has(cx, w, format!("edit-button {p} Retry")) && !has(cx, w, format!("edit-button {p} Resend")), "no Resend or Retry");
}

/// A reattach mid-send: the memo checks what it had out on the new view;
/// ours in its history is in the chat. Two identical texts: a second
/// identical hand-back, after the first is in the chat, is matched past
/// what was seen when it went, never by the first.
#[gpui_kit::test]
fn a_reattach_mid_send_checks_and_two_identical_texts_are_told_apart(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let text = last(cx, &view).text;
    view.update(cx, |v, _| {
        let lid = v.panes.cur.live;
        let s = v.model.lives[lid].active.as_ref().unwrap().session_id.clone();
        v.model.memos.on(&s, cards_spec::memo::Event::Attached, std::time::Instant::now());
    });
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::Checking), "an attach: checking");
    lands(cx, w, &view, vec![user(&text), said_ok()]);
    assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::InChat), "its history shows it");
    // The same skip, undone by the agent, and handed back again: the same
    // text (r2 is its base now, so not quite: the same edit of r1 is
    // matched only past what was seen).
    lands(cx, w, &view, agent("toolu_u", json!([{"op":"patch","id":"p","set":{"steps.fix.state":"todo"}}])));
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let second = last(cx, &view).text;
    assert_ne!(second, text, "its base moved: r3");
    assert_eq!(state(cx, &view, &second), Some(cards_spec::memo::State::Sending), "not the first's history");
    lands(cx, w, &view, vec![user(&second), said_ok()]);
    assert_eq!(state(cx, &view, &second), Some(cards_spec::memo::State::InChat));
}

// ── compaction (§5.2) ────────────────────────────────────────────────

/// The successor's history `messages` lands after a compaction.
fn successor(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<Value>) {
    compact(cx, w, view);
    view.update(cx, |v, cx| {
        let l = &v.model.lives[v.panes.cur.live];
        let (seq, sid) = (l.attachment().unwrap(), l.active.as_ref().unwrap().session_id.clone());
        let update = synaps_client::HistoryUpdate::Replace { messages };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx);
    });
    landed(cx, w, view);
    board(cx, w, view);
    wait(cx, w, view, 300);
}

/// §5.2's tests, a hold and a queue at once: a hand-back queued when
/// compaction lands moves to the successor with the queued message, where
/// its id names no block: ordinary text to the board, in the chat for the
/// memo, its row `delivered · its block was compacted away`. The draft
/// held alongside is an orphan in the dock (D8a's).
#[gpui_kit::test]
fn a_compaction_while_queued_delivers_it_and_a_hold_becomes_an_orphan(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    turn(cx, w, &view);
    walk_take(cx, w, &view, 1);
    take_and(cx, w, &view, &["x"]);
    view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone();
        assert_eq!(v.block_drafts.count(&s), 2, "the note held, the plan taken");
    });
    press(cx, w, "enter");
    let out = last(cx, &view);
    assert!(out.busy, "steered");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
    successor(cx, w, &view, vec![user("<context-summary>the plan p</context-summary>"), user(&out.text), said_ok()]);
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::InChat));
    assert!(row_says(cx, w, &view, &out.text, "delivered · its block was compacted away"));
    assert!(!has(cx, w, format!("block-draft {p}")));
    assert!(has(cx, w, "dock-draft Root cause · compacted · this draft has no block".into()), "the hold: an orphan");
}

/// During a send (idle, a `Submit` not yet echoed): the successor holds
/// it, and the memo finds it there; the row says it was delivered.
#[gpui_kit::test]
fn a_compaction_during_a_send_finds_it_in_the_successor(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let out = last(cx, &view);
    assert!(!out.busy, "a Submit");
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Sending));
    successor(cx, w, &view, vec![user("<context-summary>the plan p</context-summary>"), user(&out.text), said_ok()]);
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::InChat), "its history shows it");
    assert!(row_says(cx, w, &view, &out.text, "delivered · its block was compacted away"));
}

// ── restore (§3.6) ───────────────────────────────────────────────────

/// `R` on a past revision restores it: an ordinary edit of the current
/// incarnation, the user paths of r1 that differ now, line 1 `back to
/// r1:`, its base the current revision; the walk back on the latest. On
/// the latest itself there is nothing to restore.
#[gpui_kit::test]
fn r_restores_a_past_revision_as_an_edit(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let n = row("n");
    lands(cx, w, &view, agent("toolu_e", json!([{"op":"patch","id":"n","set":{"text":"It is the cache"}}])));
    let of = tag(cx, &view, "n");
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    press(cx, w, "shift-r");
    // No draft holds the bound board: the restore waits for the job that
    // keeps it.
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    let out = last(cx, &view);
    assert_eq!(sends(cx, &view).len(), 1, "once");
    assert!(view.read_with(cx, |v, _| v.panes.cur.walk.walk.as_ref().is_some_and(|w| w.rev.is_none())), "the walk on the latest");
    assert_eq!(
        out.text,
        format!(
            "[block n edited r2] Root cause → back to r1: text “It is the retry loop”\n{{\"of\":\"{of}\",\"set\":{{\"text\":\"It is the retry loop\"}}}}"
        )
    );
    lands(cx, w, &view, vec![user(&out.text), said_ok()]);
    assert!(row_says(cx, w, &view, &out.text, "applied"));
}

// ── notices (§3.5) ───────────────────────────────────────────────────

/// The notices that ride typed text ride a hand-back: a dismissal here,
/// its notice not sent yet, goes as the line after line 2; the memo's
/// entry is the body alone.
#[gpui_kit::test]
fn a_dismissals_notice_rides_the_hand_back(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let n = row("n");
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "x");
    landed(cx, w, &view);
    press(cx, w, "escape");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    landed(cx, w, &view);
    let out = last(cx, &view);
    assert!(out.text.ends_with("\n[block n v1 dismissed]"), "{}", out.text);
    assert!(!out.body.contains("dismissed"), "the body alone: {}", out.body);
    assert_eq!(state(cx, &view, &out.text), Some(cards_spec::memo::State::Sending), "matched by its body, whatever rides it");
}
