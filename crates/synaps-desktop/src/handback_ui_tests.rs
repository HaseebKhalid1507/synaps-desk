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
    let drawn = has(cx, w, format!("hand-back {id} {} · {words} · {outcome}", head.trim_start_matches("[block ")));
    if !drawn {
        let said = view.read_with(cx, |v, _| {
            let it = v.model.lives[v.panes.cur.live].transcript.items().iter().find(|i| i.id == id).cloned()?;
            v.hand_back_said(&it)
        });
        eprintln!("row {id} says {said:?}, not {outcome:?}");
    }
    drawn
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

// ── Shady #115 round 1 ───────────────────────────────────────────────

/// The writer held (a test's): what it is handed waits.
fn hold(cx: &mut TestAppContext, view: &Entity<AppView>) -> crate::blockfiles::Hold {
    view.read_with(cx, |v, _| v.wishes.home.writer.clone()).hold()
}

/// The writer let go: what it held is written, and what waited on it goes.
fn release(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, hold: crate::blockfiles::Hold) {
    drop(hold);
    view.read_with(cx, |v, _| v.wishes.home.writer.clone()).drain();
    super::rail_wishes_ui_tests::written(cx, view);
    wait(cx, w, view, 300);
    super::rail_wishes_ui_tests::written(cx, view);
}

/// A notice that waits for its mark: the note `n` dismissed in the walk
/// (written), the walk left, the writer held. A hand-back now rides it
/// once its sent mark is written (§3.5).
fn notice_waits(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> crate::blockfiles::Hold {
    notice_of_waits(cx, w, view, "n")
}

/// [`notice_waits`], block `name`'s dismissal.
fn notice_of_waits(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, name: &str) -> crate::blockfiles::Hold {
    enter(cx, w, view);
    walk_up(cx, w, view, At::Block(row(name)));
    press(cx, w, "x");
    landed(cx, w, view);
    press(cx, w, "escape");
    wait(cx, w, view, 300);
    hold(cx, view)
}

/// The history after the view's holds lands, its board drawn, the
/// writer held (nothing asked of it is waited for).
fn lands_held(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<Value>) {
    super::answer_tests::land(cx, w, view, messages);
    board(cx, w, view);
    wait(cx, w, view, 300);
}

/// The session on screen.
fn sid(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone())
}

/// Block `name`'s draft on the session on screen: its text sent, waiting,
/// and whether it ended.
fn the_draft(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> Option<(Option<String>, bool, bool)> {
    let s = sid(cx, view);
    let id = cards_spec::block::Ref::parse(name).unwrap();
    view.read_with(cx, |v, _| v.block_drafts.of(&s).find(|d| d.id == id).map(|d| (d.sent.clone(), d.waiting, d.ended.is_some())))
}

/// Shady #115 finding 2 (P1): a hand-back waiting for its notices' marks has
/// no memo entry yet, so its draft owns it: frames go by, and it is still
/// there, marked waiting, taking no new hold; nothing is on the wire.
/// Once the marks are written it goes, once, the notice riding it, and
/// the draft draws on as any sent one.
#[gpui_kit::test]
fn a_hand_back_waiting_for_its_notices_keeps_its_draft(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    for _ in 0..4 {
        wait(cx, w, &view, 300);
    }
    assert!(sends(cx, &view).is_empty(), "not on the wire yet");
    let (sent, waiting, ended) = the_draft(cx, &view, "p").expect("its draft is kept: nothing else owns the edit");
    assert!(sent.is_some() && waiting && !ended, "waiting for the marks");
    assert!(has(cx, w, format!("draft-head {p} WAITING · r1")));
    assert!(footer(cx, w, "p", "waiting for the rail's notices to be saved"));
    // A second take makes no competing hold.
    walk_take(cx, w, &view, 0);
    assert!(footer(cx, w, "p", SENDING), "no new hold while it waits");
    press(cx, w, "escape");
    press(cx, w, "escape");
    release(cx, w, &view, h);
    assert_eq!(sends(cx, &view).len(), 1, "written: it goes, once");
    let out = last(cx, &view);
    assert!(out.text.ends_with("\n[block n v1 dismissed]"), "its notice rides it: {}", out.text);
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Sending));
    let (_, waiting, _) = the_draft(cx, &view, "p").expect("sent: it draws on");
    assert!(!waiting);
}

/// Shady #115 finding 2 (P1): while it waits, its block is replaced, or the
/// session compacted: the draft ends as any held one (stale in its row,
/// an orphan in the dock), its text kept, and once the marks are written
/// nothing is sent: the work is the user's, recoverable.
#[gpui_kit::test]
fn a_waiting_hand_back_whose_block_ends_sends_nothing_and_keeps_its_text(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    lands_held(
        cx,
        w,
        &view,
        agent(
            "toolu_r",
            json!([{"op":"replace","id":"p","kind":"plan","title":"Another","body":{"steps":[{"id":"fix","label":"Another","state":"todo"}]}}]),
        ),
    );
    let (sent, _, ended) = the_draft(cx, &view, "p").expect("its draft is kept");
    assert!(sent.is_some() && ended, "ended while it waits, its text kept");
    release(cx, w, &view, h);
    assert!(sends(cx, &view).is_empty(), "its block ended: nothing goes");
    let (sent, waiting, ended) = the_draft(cx, &view, "p").expect("still kept: Copy to composer");
    assert!(sent.is_none() && !waiting && ended, "not sent: the stale draft, as any");
    // A compaction while one waits: an orphan in the dock.
    let (w, view) = planned(cx);
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    compact(cx, w, &view);
    let (seq, s) = attachment(cx, &view);
    view.update(cx, |v, cx| {
        let update =
            synaps_client::HistoryUpdate::Replace { messages: vec![user("<context-summary>the plan p</context-summary>"), said_ok()] };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: s, seq: 0, update }], cx);
    });
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert!(has(cx, w, "dock-draft Close the race · compacted · this draft has no block".into()), "an orphan, its text kept");
    release(cx, w, &view, h);
    assert!(sends(cx, &view).is_empty(), "compacted: nothing goes");
    assert!(has(cx, w, "dock-draft Close the race · compacted · this draft has no block".into()), "still in the dock");
}

/// Shady #115 finding 2 and 3 (P1): while it waits, the attachment changes,
/// or the connection stops: once the marks are written nothing goes,
/// and the draft is held again, so the user hands it back: once.
#[gpui_kit::test]
fn a_waiting_hand_back_across_a_reattach_or_a_stopped_connection_is_held_again(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    let client = view.update(cx, |v, _| v.client.take());
    release(cx, w, &view, h);
    assert!(sends(cx, &view).is_empty(), "the connection didn't take it");
    let (sent, waiting, ended) = the_draft(cx, &view, "p").expect("held again");
    assert!(sent.is_none() && !waiting && !ended);
    view.update(cx, |v, _| v.client = client);
    // Reattached meanwhile (another attachment of the session).
    let (w, view) = planned(cx);
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    view.update(cx, |v, _| {
        let l = &mut v.model.lives[v.panes.cur.live];
        let a = l.active.as_mut().unwrap();
        a.attach_seq += 100;
        l.wanted = Some(a.attach_seq);
    });
    release(cx, w, &view, h);
    assert!(sends(cx, &view).is_empty(), "another attachment: nothing goes");
    let (sent, _, ended) = the_draft(cx, &view, "p").expect("held again");
    assert!(sent.is_none() && !ended);
    // Handed back again, explicitly: once.
    take_and(cx, w, &view, &[]);
    press(cx, w, "enter");
    landed(cx, w, &view);
    assert_eq!(sends(cx, &view).len(), 1, "once");
    assert!(has(cx, w, format!("draft-head {p} SENDING · r1")));
}

/// Shady #115 finding 3 (P1): at the wire, a waiting hand-back is checked
/// again on the board as it is then. The agent ran a step it skips
/// meanwhile: nothing goes (never the message pre-checked before, nor
/// what is left of it), and the draft is held again, its text the user's.
/// Skipping `fix` alone: nothing would land. Skipping `fix` and `test`,
/// the agent running `test`: the pre-check would drop that skip, another
/// message than the one waiting; the user hands it back, once.
#[gpui_kit::test]
fn a_waiting_hand_back_is_pre_checked_again_at_the_wire(cx: &mut TestAppContext) {
    for (keys, ran) in [(&["x"][..], "fix"), (&["x", "j", "x"][..], "test")] {
        let (w, view) = planned(cx);
        let h = notice_waits(cx, w, &view);
        take_and(cx, w, &view, keys);
        press(cx, w, "enter");
        wait(cx, w, &view, 300);
        let set = json!({ format!("steps.{ran}.state"): "done" });
        lands_held(cx, w, &view, agent("toolu_ran", json!([{"op":"patch","id":"p","set": set}])));
        release(cx, w, &view, h);
        assert!(sends(cx, &view).is_empty(), "{ran} ran: nothing goes");
        let (sent, waiting, ended) = the_draft(cx, &view, "p").expect("held again");
        assert!(sent.is_none() && !waiting && !ended, "{ran}: its text the user's");
        if ran == "test" {
            take_and(cx, w, &view, &[]);
            press(cx, w, "enter");
            landed(cx, w, &view);
            assert_eq!(sends(cx, &view).len(), 1, "handed back again: once");
            assert!(!last(cx, &view).text.contains("test"), "what is left: {}", last(cx, &view).text);
        }
    }
}

/// Store `lid`'s gate taken away three ways while a send waits (Shady
/// #115 finding 3): input authority (another client owns the input), a
/// compaction starting, events dropped.
#[derive(Clone, Copy, Debug)]
enum Gate {
    Authority,
    Compacting,
    Dropped,
}

/// The daemon says the session's input is owned by this client plus
/// `other` (0: this one).
fn owned_by(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, other: u64) {
    let (seq, sid) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let client = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().client;
        let ev = synaps_client::Event::InputOwner {
            attach_seq: seq,
            session_id: sid,
            client,
            owner: Some(client + other),
            reason: "takeover".into(),
        };
        v.apply_batch(vec![ev], cx);
    });
    cx.run_until_parked();
    wait(cx, w, view, 300);
}

fn close_gate(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, g: Gate) {
    match g {
        Gate::Authority => {
            view.update(cx, |v, _| {
                v.model.lives[v.panes.cur.live].active.as_mut().unwrap().input = synaps_client::protocol::ClientInput::Exclusive
            });
            owned_by(cx, w, view, 1);
        }
        Gate::Compacting => heard(cx, w, view, vec![json!({"ev": "compaction_started", "source": "manual", "disclosure": ""})]),
        Gate::Dropped => heard(cx, w, view, vec![json!({"ev": "system_notice", "text": "event stream lagged; 6 dropped"})]),
    }
    let shut = view.read_with(cx, |v, _| v.model.card_gate(v.panes.cur.live).is_err());
    assert!(shut, "{g:?}: the gate is shut");
}

/// Shady #115 finding 3 (P1): a hand-back that waits for its notices' marks
/// meets the card's gate again at the wire. Shut meanwhile (input
/// authority lost, a compaction started, events dropped) with the same
/// attachment: nothing goes, the draft is held again (recoverable).
/// Authority back, the user's explicit hand-back goes, once.
#[gpui_kit::test]
fn a_waiting_hand_back_meets_the_gate_again_at_the_wire(cx: &mut TestAppContext) {
    for g in [Gate::Authority, Gate::Compacting, Gate::Dropped] {
        let (w, view) = planned(cx);
        let h = notice_waits(cx, w, &view);
        take_and(cx, w, &view, &["x"]);
        press(cx, w, "enter");
        wait(cx, w, &view, 300);
        let seq = attachment(cx, &view).0;
        close_gate(cx, w, &view, g);
        assert_eq!(attachment(cx, &view).0, seq, "{g:?}: the same attachment");
        release(cx, w, &view, h);
        assert!(sends(cx, &view).is_empty(), "{g:?}: nothing goes through a shut gate");
        let (sent, waiting, ended) = the_draft(cx, &view, "p").unwrap_or_else(|| panic!("{g:?}: the draft is kept"));
        assert!(sent.is_none() && !waiting && !ended, "{g:?}: held again");
        if let Gate::Authority = g {
            owned_by(cx, w, &view, 0);
            take_and(cx, w, &view, &[]);
            press(cx, w, "enter");
            landed(cx, w, &view);
            assert_eq!(sends(cx, &view).len(), 1, "authority back: the explicit hand-back goes, once");
        }
    }
}

/// Shady #115 finding 3 (P1): a Retry and a Resend that wait for their
/// notices' marks meet the gate again too: shut meanwhile, nothing goes,
/// and the entry is as it was (not sent; not confirmed), its buttons
/// back once the gate is open.
#[gpui_kit::test]
fn a_waiting_retry_or_resend_meets_the_gate_again_at_the_wire(cx: &mut TestAppContext) {
    for g in [Gate::Authority, Gate::Compacting, Gate::Dropped] {
        // Retry.
        let (w, view) = planned(cx);
        let p = row("p");
        take_and(cx, w, &view, &["x"]);
        press(cx, w, "enter");
        let text = last(cx, &view).text;
        refused(cx, w, &view, "busy");
        assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::NotSent));
        let h = notice_waits(cx, w, &view);
        button(cx, w, format!("edit-button {p} Retry"));
        wait(cx, w, &view, 300);
        close_gate(cx, w, &view, g);
        release(cx, w, &view, h);
        assert_eq!(sends(cx, &view).len(), 1, "{g:?}: the Retry didn't go");
        assert_eq!(state(cx, &view, &text), Some(cards_spec::memo::State::NotSent), "{g:?}: as it was");
        if let Gate::Authority = g {
            owned_by(cx, w, &view, 0);
            wait(cx, w, &view, 300);
            button(cx, w, format!("edit-button {p} Retry"));
            assert_eq!(sends(cx, &view).len(), 2, "authority back: the explicit Retry goes, once");
            assert_eq!(last(cx, &view).body, text);
        }
        // Resend.
        let (w, view) = planned(cx);
        turn(cx, w, &view);
        take_and(cx, w, &view, &["x"]);
        press(cx, w, "enter");
        let out = last(cx, &view);
        heard(cx, w, &view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
        heard(cx, w, &view, vec![json!({"ev": "dequeued", "text": out.text})]);
        idle(cx, w, &view);
        assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed));
        let h = notice_waits(cx, w, &view);
        button(cx, w, format!("edit-button {p} Resend"));
        wait(cx, w, &view, 300);
        close_gate(cx, w, &view, g);
        release(cx, w, &view, h);
        assert_eq!(sends(cx, &view).len(), 1, "{g:?}: the Resend didn't go");
        assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed), "{g:?}: as it was");
        if let Gate::Authority = g {
            owned_by(cx, w, &view, 0);
            wait(cx, w, &view, 300);
            button(cx, w, format!("edit-button {p} Resend"));
            assert_eq!(sends(cx, &view).len(), 2, "authority back: the explicit Resend goes, once");
            assert_eq!(last(cx, &view).body, out.body);
        }
    }
}

/// Shady #115 finding 9 (P1): a Retry the connection didn't take moves only
/// its own entry, never the oldest one sending. A card's answer out on
/// the wire, then a not-sent hand-back's Retry with the connection
/// stopped: the answer is still sending, the hand-back still not sent;
/// the daemon's refusal then is the answer's (the wire's oldest). The
/// same with another block's hand-back out, and with a Resend.
#[gpui_kit::test]
fn a_retry_the_connection_didnt_take_moves_only_its_own_entry(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let b = last(cx, &view).text;
    refused(cx, w, &view, "busy");
    assert_eq!(state(cx, &view, &b), Some(cards_spec::memo::State::NotSent));
    // A card's answer, out.
    let ask = json!({"question": "Squash the commits?", "options": ["Yes", "No"]});
    super::answer_tests::ask_live(cx, w, &view, "toolu_c9", "c9squas", ask);
    let (card, ix) = super::answer_tests::card(cx, &view, "Squash the commits?");
    super::answer_tests::show(cx, w, &view, ix);
    super::answer_tests::arm(cx, &view, card, true);
    super::answer_tests::click(cx, w, &view, format!("ask-option {card} 1"));
    assert!(has(cx, w, format!("ask-state {card} sending…")), "the answer is out");
    let client = view.update(cx, |v, _| v.client.take());
    button(cx, w, format!("edit-button {p} Retry"));
    assert_eq!(sends(cx, &view).len(), 1, "the Retry didn't go");
    assert!(has(cx, w, format!("ask-state {card} sending…")), "the answer on the wire is untouched");
    assert_eq!(state(cx, &view, &b), Some(cards_spec::memo::State::NotSent), "the hand-back as it was");
    view.update(cx, |v, _| v.client = client);
    refused(cx, w, &view, "input is owned by client #1");
    assert!(has(cx, w, format!("ask-state {card} not sent")), "the refusal is the wire's oldest: the answer's");
    assert_eq!(state(cx, &view, &b), Some(cards_spec::memo::State::NotSent));
    button(cx, w, format!("edit-button {p} Retry"));
    assert_eq!(sends(cx, &view).len(), 2, "then the Retry goes, once");
    // Another block's hand-back out (the note's), then p's Resend with
    // the connection stopped.
    let (w, view) = planned(cx);
    turn(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let out = last(cx, &view);
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
    heard(cx, w, &view, vec![json!({"ev": "dequeued", "text": out.text})]);
    idle(cx, w, &view);
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed));
    retitle_note(cx, w, &view, "The cause");
    let a = last(cx, &view).body;
    assert!(a.starts_with("[block n "), "{a}");
    assert_eq!(state(cx, &view, &a), Some(cards_spec::memo::State::Sending), "the note's hand-back is out");
    let before = sends(cx, &view).len();
    let client = view.update(cx, |v, _| v.client.take());
    button(cx, w, format!("edit-button {p} Resend"));
    assert_eq!(sends(cx, &view).len(), before, "the Resend didn't go");
    assert_eq!(state(cx, &view, &a), Some(cards_spec::memo::State::Sending), "the note's, on the wire, untouched");
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed), "p's as it was");
    view.update(cx, |v, _| v.client = client);
    refused(cx, w, &view, "busy");
    assert_eq!(state(cx, &view, &a), Some(cards_spec::memo::State::NotSent), "the wire's oldest: the note's");
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed));
}

/// The note `n` retitled `title` in its draft's field and handed back.
fn retitle_note(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, title: &str) {
    walk_take(cx, w, view, 1);
    press(cx, w, "t");
    let f = view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned()).expect("the title's field");
    cx.update_window(w, |_, window, cx| {
        f.update(cx, |s, cx| {
            let n = s.value().len();
            gpui_kit::EntityInputHandler::replace_text_in_range(s, Some(0..n), title, window, cx)
        })
    })
    .expect("the window");
    press(cx, w, "enter");
    press(cx, w, "enter");
}

/// The delivery line of this desk's hand-back `body`, as its block says
/// it (its word, its cause).
fn delivery(cx: &mut TestAppContext, view: &Entity<AppView>, body: &str) -> String {
    view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone();
        let (word, cause) = crate::handback::words(v.model.memos.edits.get(&s, body).expect("its entry"));
        match cause {
            Some(c) => format!("{word} · {c}"),
            None => word.to_string(),
        }
    })
}

/// The hand-back with body `body` (one of this desk's): its entry's key.
fn key_of(body: &str) -> u64 {
    crate::handback_view::entry_key(body)
}

/// Shady #115 finding 7: every recoverable hand-back of the incarnation
/// says so on its block, each with its own buttons. A refused, then a
/// disjoint B taken in the chat: A is still there, its Retry aimed at A
/// (by its key), never at B; a covering hand-back retires it instead.
#[gpui_kit::test]
fn an_older_disjoint_hand_back_stays_on_its_block_and_acts_on_itself(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let a = last(cx, &view).body;
    refused(cx, w, &view, "busy");
    take_and(cx, w, &view, &["j", "b"]);
    press(cx, w, "enter");
    let b = last(cx, &view).body;
    assert_ne!(a, b);
    // Its echo (the wire's input confirmed), then its history.
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": b})]);
    idle(cx, w, &view);
    lands(cx, w, &view, vec![user(&b), said_ok()]);
    assert_eq!(state(cx, &view, &b), Some(cards_spec::memo::State::InChat));
    assert_eq!(state(cx, &view, &a), Some(cards_spec::memo::State::NotSent), "B doesn't cover A");
    let line = delivery(cx, &view, &a);
    assert!(line.starts_with("hand-back not sent"), "{line}");
    assert!(has(cx, w, format!("edit-delivery {p} {line}")), "A's line is still there");
    let retry = format!("edit-button {p} Retry {}", key_of(&a));
    assert!(has(cx, w, retry.clone()), "A's own Retry");
    button(cx, w, retry);
    assert_eq!(last(cx, &view).body, a, "the Retry is A's");
    assert_eq!(state(cx, &view, &a), Some(cards_spec::memo::State::Sending));
    refused(cx, w, &view, "busy");
    // A Retry makes A the newest: its buttons are the block's then.
    let keyed = format!("edit-button {p} Discard {}", key_of(&a));
    let sel = if has(cx, w, keyed.clone()) { keyed } else { format!("edit-button {p} Discard") };
    button(cx, w, sel);
    assert_eq!(state(cx, &view, &a), None, "A's Discard");
    assert_eq!(state(cx, &view, &b), Some(cards_spec::memo::State::InChat), "B as it was");
}

/// Shady #115 finding 7: a hand-back not confirmed whose block then ends
/// (removed here) is listed in the drafts dock, its words, Copy and
/// Discard, never Resend (it would apply nothing); Discard makes its room.
#[gpui_kit::test]
fn an_ended_blocks_hand_back_is_in_the_dock_with_copy_and_discard(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    turn(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let out = last(cx, &view);
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
    heard(cx, w, &view, vec![json!({"ev": "dequeued", "text": out.text})]);
    idle(cx, w, &view);
    assert_eq!(state(cx, &view, &out.body), Some(cards_spec::memo::State::Unconfirmed));
    lands(cx, w, &view, agent("toolu_rm", json!([{"op":"remove","id":"p"}])));
    let k = key_of(&out.body);
    let line = format!("hand-back of p r1 · {}", delivery(cx, &view, &out.body));
    assert!(line.contains("hand-back not confirmed"), "{line}");
    assert!(has(cx, w, format!("dock-hand-back {line}")), "in the dock: {line}");
    assert!(has(cx, w, format!("dock-button Copy to composer hand-back {k}")) && has(cx, w, format!("dock-button Discard hand-back {k}")));
    assert!(!has(cx, w, format!("edit-button {p} Resend")), "no Resend: its block is gone");
    button(cx, w, format!("dock-button Discard hand-back {k}"));
    assert_eq!(state(cx, &view, &out.body), None, "discarded: its room is free");
    assert!(!has(cx, w, format!("dock-hand-back {line}")));
}

/// The note `n` (its tag `of`) handed back by another desk `k` times,
/// its text each time (each at r1: they land as they can). None covers
/// a hand-back of the plan.
fn other_desks(of: &str, k: usize) -> Vec<Value> {
    (0..k)
        .flat_map(|k| {
            let text = format!("[block n edited r1] Root cause → text\n{{\"of\":\"{of}\",\"set\":{{\"text\":\"Cause {k}\"}}}}");
            [user(&text), said_ok()]
        })
        .collect()
}

/// Row `text`, scrolled to: whether its outcome line says `outcome`.
fn drawn_says(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str, outcome: &str) -> bool {
    let id = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.items().iter().find(|i| i.text == text).map(|i| i.id));
    let Some(ix) = id.and_then(|id| view.read_with(cx, |v, _| v.panes.cur.disp.iter().position(|d| d.id == id))) else { return false };
    super::answer_tests::show(cx, w, view, ix);
    row_says(cx, w, view, text, outcome)
}

/// What row `text` says the board did, now.
fn said_now(cx: &mut TestAppContext, view: &Entity<AppView>, text: &str) -> Option<String> {
    view.read_with(cx, |v, _| {
        let it = v.model.lives[v.panes.cur.live].transcript.items().iter().find(|i| i.text == text).cloned()?;
        v.hand_back_said(&it)
    })
}

/// Shady #115 finding 6: past the 32 outcomes the board keeps, an older
/// hand-back's row is re-folded when it draws: `loading…`, then what it
/// did, never a blank line.
#[gpui_kit::test]
fn an_evicted_outcome_is_refolded_when_its_row_draws(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view).text;
    let of = tag(cx, &view, "n");
    let mut h = vec![user(&ours), said_ok()];
    h.extend(other_desks(&of, 34));
    lands(cx, w, &view, h);
    let kept = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.drawn().map_or(0, |d| d.outcomes.len()));
    assert_eq!(kept, cards_spec::board::OUTCOMES_MAX, "the board keeps 32");
    assert!(hand_back_row(cx, &view, &ours).is_none(), "ours isn't one of them");
    let mut said = Vec::new();
    for _ in 0..20 {
        let applied = drawn_says(cx, w, &view, &ours, "applied");
        said.push(said_now(cx, &view, &ours));
        if applied {
            break;
        }
        landed(cx, w, &view);
    }
    assert_eq!(said.last().cloned().flatten().as_deref(), Some("applied"), "re-folded: what it did ({said:?})");
    assert!(
        said.iter().all(|s| matches!(s.as_deref(), Some(crate::handback_view::LOADING) | Some("applied"))),
        "loading until then, never blank: {said:?}"
    );
}

/// Shady #115 finding 6, its delivery side: our hand-back in the chat,
/// then more than 32 others in one history before the next board: the
/// board that read it no longer keeps its outcome, and the sent draft
/// still goes (the board has read it), never stuck over the board.
#[gpui_kit::test]
fn a_sent_draft_goes_once_the_board_read_it_whatever_outcomes_it_keeps(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view).text;
    let of = tag(cx, &view, "n");
    assert!(has(cx, w, format!("block-draft {p}")), "sent: the draft draws");
    let mut h = vec![user(&ours), said_ok()];
    h.extend(other_desks(&of, 34));
    lands(cx, w, &view, h);
    view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref().unwrap().session_id.clone();
        let e = v.model.memos.edits.get(&s, &ours).expect("its entry");
        assert!(e.state == cards_spec::memo::State::InChat && e.retired.is_none(), "in the chat, nothing covers it: {e:?}");
    });
    assert!(hand_back_row(cx, &view, &ours).is_none(), "its outcome let go");
    assert!(the_draft(cx, &view, "p").is_none(), "the board read it: the draft went");
    let ix = view.read_with(cx, |v, _| v.panes.cur.disp.iter().position(|d| d.id == p)).expect("its row");
    super::answer_tests::show(cx, w, &view, ix);
    assert!(!has(cx, w, format!("block-draft {p}")), "the board draws");
    take_and(cx, w, &view, &[]);
    assert!(has(cx, w, format!("block-draft {p}")), "and takes a new hold");
}

/// The successor's history `messages` lands on a desk that never saw the
/// compaction (a late attach: no `compaction_applied` heard).
fn late_successor(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<Value>) {
    let (seq, sid) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let update = synaps_client::HistoryUpdate::Replace { messages };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx);
    });
    landed(cx, w, view);
    board(cx, w, view);
    wait(cx, w, view, 300);
}

/// Shady #115 finding 8: `compacted away` is the history's word. The same
/// successor history says it on the desk that saw the compaction and on
/// one that attached after; a hand-back of a block put and removed after
/// the compaction is `gone`, not compacted away.
#[gpui_kit::test]
fn compacted_away_is_the_historys_word_on_any_desk(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view).text;
    let h = vec![user("<context-summary>the plan p</context-summary>"), user(&ours), said_ok()];
    successor(cx, w, &view, h.clone());
    assert!(row_says(cx, w, &view, &ours, "delivered · its block was compacted away"), "the desk that saw it");
    let (w2, late) = planned(cx);
    late_successor(cx, w2, &late, h);
    assert_eq!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.compactions()), 1);
    assert_eq!(late.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.compactions()), 0, "never saw it");
    assert!(row_says(cx, w2, &late, &ours, "delivered · its block was compacted away"), "the same on a late attach");
    // After the compaction: `q` put, handed back, removed: gone.
    let (w, view) = planned(cx);
    let mut h = vec![user("<context-summary>the plan p</context-summary>")];
    h.extend(agent("toolu_q", json!([{"op":"put","id":"q","kind":"note","title":"Q","body":{"text":"q"}}])));
    h.push(said_ok());
    late_successor(cx, w, &view, h.clone());
    let q = tag(cx, &view, "q");
    let text = format!("[block q edited r1] Q → text\n{{\"of\":\"{q}\",\"set\":{{\"text\":\"q2\"}}}}");
    h.extend(agent("toolu_rq", json!([{"op":"remove","id":"q"}])));
    h.extend([user(&text), said_ok()]);
    late_successor(cx, w, &view, h);
    assert!(row_says(cx, w, &view, &text, "not applied: the block is gone"), "removed after it: not compacted away");
}

/// The plan `p` taken, `fix` renamed `label`, handed back and taken in.
fn user_renames_fix(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, label: &str) {
    take_and(cx, w, view, &["r"]);
    let f = view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned()).expect("the label's field");
    cx.update_window(w, |_, window, cx| {
        f.update(cx, |s, cx| {
            let n = s.value().len();
            gpui_kit::EntityInputHandler::replace_text_in_range(s, Some(0..n), label, window, cx)
        })
    })
    .expect("the window");
    press(cx, w, "enter");
    press(cx, w, "enter");
    let text = last(cx, view).text;
    lands(cx, w, view, vec![user(&text), said_ok()]);
    assert_eq!(on_board(cx, view, "fix").1, label);
}

/// The walk's ring on the plan `p`, then left.
fn focus_plan(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    enter(cx, w, view);
    walk_on(cx, w, view, At::Block(row("p")));
    wait(cx, w, view, 300);
    press(cx, w, "escape");
    wait(cx, w, view, 300);
}

/// Shady #115 finding 4: what a focus acknowledges is the overwrite then,
/// not the path for good. The user renames `fix`, the agent overwrites
/// it: the chip; a focus clears it, and an agent tick of another path
/// doesn't bring it back. The user renames it again, the agent overwrites
/// it again: the chip again, cleared by the next focus.
#[gpui_kit::test]
fn changed_your_edit_says_each_overwrite_until_its_focus(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let chip = format!("block-changed {} steps.fix.label", row("p"));
    user_renames_fix(cx, w, &view, "Fix it now");
    lands(cx, w, &view, agent("toolu_o1", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race"}}])));
    assert!(has(cx, w, chip.clone()), "the first overwrite");
    focus_plan(cx, w, &view);
    assert!(!has(cx, w, chip.clone()), "focused: seen");
    lands(cx, w, &view, agent("toolu_t1", json!([{"op":"patch","id":"p","set":{"steps.test.state":"doing"}}])));
    assert!(!has(cx, w, chip.clone()), "an agent write of another path: still seen");
    user_renames_fix(cx, w, &view, "Fix it again");
    assert!(!has(cx, w, chip.clone()), "the user's now");
    lands(cx, w, &view, agent("toolu_o2", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race again"}}])));
    assert!(has(cx, w, chip.clone()), "the second overwrite: the chip again");
    focus_plan(cx, w, &view);
    assert!(!has(cx, w, chip), "and its focus clears it");
}

/// Shady #115 finding 5: what waits is charged whole. A restore of a note
/// whose text is near the draft's ceiling waits behind the notices'
/// marks: the waits' charge covers its body and its draft (counted here
/// on their own), and stays within [`WAIT_MAX_BYTES`]. The 256 overwrites
/// seen at most are charged their ids and paths.
#[gpui_kit::test]
fn what_waits_and_what_was_seen_are_charged_whole(cx: &mut TestAppContext) {
    use crate::handback_view::WAIT_MAX_BYTES;
    let (w, view) = planned(cx);
    let n = row("n");
    let (x, y) = ("x".repeat(1400), "y".repeat(1400));
    lands(cx, w, &view, agent("toolu_e", json!([{"op":"patch","id":"n","set":{"text": x}}])));
    lands(cx, w, &view, agent("toolu_f", json!([{"op":"patch","id":"n","set":{"text": y}}])));
    let h = notice_of_waits(cx, w, &view, "p");
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    press(cx, w, "shift-r");
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert!(sends(cx, &view).is_empty(), "it waits for the marks");
    let (charged, own) = view.read_with(cx, |v, _| {
        let own: usize = v
            .edit_waits
            .values()
            .map(|s| {
                let (body, d) = s.parts();
                let d = d.expect("a restore's draft");
                let synaps_client::blocks::Body::Note(now) = &d.base_block.body else { panic!("the note") };
                assert!(body.contains(&x) && now.text == y, "r2 restored over r3");
                body.len() + std::mem::size_of::<crate::drafts::Draft>() + now.text.len() + d.own_bytes()
            })
            .sum();
        (v.edit_waits_bytes(), own + v.edit_waits.len() * std::mem::size_of::<(u64, crate::handback_view::EditSend)>())
    });
    assert_eq!(view.read_with(cx, |v, _| v.edit_waits.len()), 1);
    assert!(charged >= own, "its body, its draft and the block it holds: {charged} vs {own}");
    assert!(charged <= WAIT_MAX_BYTES, "{charged} within {WAIT_MAX_BYTES}");
    release(cx, w, &view, h);
    assert_eq!(sends(cx, &view).len(), 1, "then it goes");
    // 256 seen, long sessions and paths, each another.
    let of = cards_spec::block::Tag::parse(&tag(cx, &view, "n")).unwrap();
    view.update(cx, |v, _| {
        for k in 0..crate::handback_view::SEEN_MAX + 8 {
            v.see(&format!("{}-{k}", "s".repeat(60)), of, &format!("steps.{}{k}.label", "a".repeat(20)), 3);
        }
    });
    let (charged, own, len) = view.read_with(cx, |v, _| (v.changed_seen_bytes(), v.changed_seen_heap(), v.changed_seen.len()));
    assert_eq!(len, crate::handback_view::SEEN_MAX, "the oldest go");
    assert!(charged >= own + len * std::mem::size_of::<crate::handback_view::Seen>(), "{charged} vs {own}");
}

/// The walk's past revision `rev` of row `row` is ready (its re-fold
/// done), so a restore could read it now.
fn past_ready(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, row: u64, rev: u32) {
    for _ in 0..20 {
        if view.read_with(cx, |v, _| v.walk_past(row).and_then(|p| p.ready().map(|s| s.block.rev)) == Some(rev)) {
            return;
        }
        wait(cx, w, view, 300);
    }
    panic!("r{rev} never ready");
}

/// Shady #115 finding 11: a restore that waits for the bound board is
/// the revision pressed on, or nothing. `R` on r2, then the walk to r1
/// before the board: nothing is restored (never r1). `R` on r1, the walk
/// left and opened again on r1 before the board: nothing either (the
/// press ended with its walk). Then `R`, the board: r1, once. A prompt
/// up all along stays pending.
#[gpui_kit::test]
fn a_waiting_restore_is_what_was_pressed_or_nothing(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let n = row("n");
    lands(cx, w, &view, agent("toolu_e1", json!([{"op":"patch","id":"n","set":{"text":"r2's"}}])));
    lands(cx, w, &view, agent("toolu_e2", json!([{"op":"patch","id":"n","set":{"text":"r3's"}}])));
    super::keys_tests::prompt(cx, w, &view);
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    press(cx, w, "shift-r");
    super::keys_tests::prompt_pending(cx, &view, "R");
    assert!(view.read_with(cx, |v, _| v.restore_wanted.is_some()), "it waits for the board");
    press(cx, w, "[");
    past_ready(cx, w, &view, n, 1);
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert!(sends(cx, &view).is_empty(), "r1 in view now: nothing restored");
    assert!(view.read_with(cx, |v, _| v.restore_wanted.is_none()));
    let note = view.read_with(cx, |v, _| v.panes.cur.edit.note.clone());
    assert_eq!(note, Some((n, crate::handback_view::RESTORE_LEFT.to_string())), "the block says why, never a try at r1");
    super::keys_tests::prompt_pending(cx, &view, "the board");
    // R on r1, the walk left and opened again on r1.
    press(cx, w, "shift-r");
    press(cx, w, "escape");
    wait(cx, w, &view, 300);
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    press(cx, w, "[");
    past_ready(cx, w, &view, n, 1);
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert!(sends(cx, &view).is_empty(), "its walk ended: nothing restored");
    // Pressed now: r1, once.
    press(cx, w, "shift-r");
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert_eq!(sends(cx, &view).len(), 1, "once");
    assert!(last(cx, &view).text.contains("back to r1:"), "{}", last(cx, &view).text);
    super::keys_tests::prompt_pending(cx, &view, "the restore");
}

/// Shady #115 interpretation 7: Shift+R held is one press, its repeats
/// nothing; interpretation 9: a past revision of an earlier incarnation
/// (the note replaced since) is view-only.
#[gpui_kit::test]
fn shift_r_held_restores_once_and_an_earlier_incarnation_is_view_only(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let n = row("n");
    lands(cx, w, &view, agent("toolu_e1", json!([{"op":"patch","id":"n","set":{"text":"r2's"}}])));
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "escape");
    press(cx, w, "escape");
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    super::keys_tests::down(cx, w, "shift-r");
    super::settle(cx, w, 8);
    for _ in 0..4 {
        super::keys_tests::repeat(cx, w, "shift-r");
    }
    board(cx, w, &view);
    for _ in 0..4 {
        super::keys_tests::repeat(cx, w, "shift-r");
    }
    wait(cx, w, &view, 300);
    assert_eq!(sends(cx, &view).len(), 1, "held: one restore");
    // Replaced since: its past is another incarnation's.
    let (w, view) = planned(cx);
    lands(cx, w, &view, agent("toolu_r", json!([{"op":"replace","id":"n","kind":"note","title":"Another","body":{"text":"new"}}])));
    take_and(cx, w, &view, &["x"]);
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(n));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    press(cx, w, "shift-r");
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert!(sends(cx, &view).iter().all(|o| !o.text.contains("back to")), "view-only: nothing restored");
    assert_eq!(view.read_with(cx, |v, _| v.panes.cur.edit.note.clone()).map(|n| n.1).as_deref(), Some(crate::handback_view::VIEW_ONLY));
}

/// Shady #115's prompts: with a permission prompt up, Hand back, Retry,
/// Resend (by click) and Restore (by key) each send a message of their
/// own, and the prompt stays pending, unanswered.
#[gpui_kit::test]
fn hand_back_retry_resend_and_restore_never_resolve_a_prompt(cx: &mut TestAppContext) {
    use super::keys_tests::{prompt, prompt_pending};
    let (w, view) = planned(cx);
    let p = row("p");
    prompt(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    let at = super::blocks_ui_tests::drawn(cx, w, format!("draft-button {p} Hand back on ready")).center();
    super::blocks_ui_tests::click(cx, w, at);
    wait(cx, w, &view, 300);
    prompt_pending(cx, &view, "Hand back");
    assert_eq!(sends(cx, &view).len(), 1);
    refused(cx, w, &view, "busy");
    button(cx, w, format!("edit-button {p} Retry"));
    prompt_pending(cx, &view, "Retry");
    assert_eq!(sends(cx, &view).len(), 2);
    let out = last(cx, &view);
    turn(cx, w, &view);
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
    heard(cx, w, &view, vec![json!({"ev": "dequeued", "text": out.text})]);
    idle(cx, w, &view);
    prompt(cx, w, &view);
    button(cx, w, format!("edit-button {p} Resend"));
    prompt_pending(cx, &view, "Resend");
    assert_eq!(sends(cx, &view).len(), 3);
    lands(cx, w, &view, agent("toolu_e1", json!([{"op":"patch","id":"n","set":{"text":"r2's"}}])));
    prompt(cx, w, &view);
    enter(cx, w, &view);
    walk_up(cx, w, &view, At::Block(row("n")));
    press(cx, w, "[");
    wait(cx, w, &view, 600);
    press(cx, w, "shift-r");
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    prompt_pending(cx, &view, "Restore");
    assert!(last(cx, &view).text.contains("back to r1:"));
}
