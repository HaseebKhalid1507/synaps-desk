//! The delivery adapter's fixtures (synaps-cards `docs/design.md` §9.3,
//! D2a's tests): each a sequence of real events, decoded from their wire
//! form and applied to the [`Model`] as the client hands them over, and
//! what the card's answer is after each step. The memo's own transitions
//! are S3's (`cards_spec::memo` tests); these pin what the desk feeds it:
//! which event, from which attachment, on which view, and what a click
//! sends where.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cards_spec::memo::{Cause, State, Trigger};
use cards_spec::{CardId, Reply};
use serde_json::{Value, json};
use synaps_client::protocol::{DaemonFrame, decode_event, decode_frame};
use synaps_client::{AttachMode, Attached, Event, HistoryUpdate};

use super::*;
use crate::live::LiveId;
use crate::model::{ConnState, Effect, Model};
use crate::pane::PaneId;

const S: &str = "20261124-120000-d2a0";
const S2: &str = "20261124-120000-d2b0";
const CARD: &str = "c4f9x2a";

fn ask() -> Value {
    json!({"question": "Which approach?", "options": ["Refactor the parser", "Rewrite from scratch"], "default": "Refactor the parser"})
}

fn doc() -> cards_spec::CardDoc {
    match cards_spec::parse_input(&ask()).unwrap() {
        cards_spec::Input::Create { doc, .. } => doc,
        cards_spec::Input::Close(_) => unreachable!(),
    }
}

/// What the extension returns for the card `card` asking `input`.
fn showing(card: &str, input: &Value) -> String {
    let Ok(cards_spec::Input::Create { doc, wait }) = cards_spec::parse_input(input) else { panic!() };
    format!(
        "{}\n  1. Refactor the parser\n  2. Rewrite from scratch\nIts answer will come as a user message starting \"[card {card}]\".",
        cards_spec::Envelope::showing(CardId::parse(card).unwrap(), &doc, wait)
    )
}

/// A session whose agent asked [`ask`] as [`CARD`] (call `t1`) and ended
/// its turn.
fn history() -> Vec<Value> {
    vec![
        json!({"role": "user", "content": "Fix the parser."}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "One question first."},
            {"type": "tool_use", "id": "t1", "name": "cards_ui", "input": ask()}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": showing(CARD, &ask())}]}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "Waiting on you."}]}),
    ]
}

/// `label` picked on [`ask`].
fn pick(label: &str) -> Reply {
    Reply::Answer(cards_spec::parse_answer(&doc(), &json!({"choice": label})).unwrap())
}

/// The message that answers [`CARD`] with `label`.
fn answer(label: &str) -> String {
    cards_spec::message::build(&CardId::parse(CARD).unwrap(), &doc(), &pick(label))
}

/// [`history`] with `texts` sent after it as user messages, each answered.
fn with(texts: &[&str]) -> Vec<Value> {
    let mut h = history();
    for t in texts {
        h.push(json!({"role": "user", "content": t}));
        h.push(json!({"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}));
    }
    h
}

fn attached_frame(sid: &str, msgs: &[Value], patch: impl FnOnce(&mut Value)) -> Box<Attached> {
    let mut v = json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": sid, "model": "m", "lifecycle": "live"},
        "conversation": {"api_messages": msgs, "messages_len": msgs.len()}, "streaming": false});
    patch(&mut v);
    match decode_frame(&v.to_string()).unwrap() {
        DaemonFrame::Attached(a) => a,
        other => panic!("{other:?}"),
    }
}

/// Store `lid` attaches `sid` as attachment `seq`, its history `msgs`.
fn attach(m: &mut Model, lid: LiveId, seq: u64, sid: &str, msgs: &[Value]) {
    attach_with(m, lid, seq, sid, msgs, |_| {}, false);
}

/// [`attach`], the frame patched; `reconnect`: the client's own re-attach
/// of the attachment it holds (after a drop or a daemon restart).
fn attach_with(m: &mut Model, lid: LiveId, seq: u64, sid: &str, msgs: &[Value], patch: impl FnOnce(&mut Value), reconnect: bool) {
    if !reconnect {
        m.want(lid, seq);
        m.asked_for(lid, sid);
    }
    m.apply(Event::Attached { attach_seq: seq, attached: attached_frame(sid, msgs, patch), mode: AttachMode::Mirror, reconnect });
}

/// A session event of attachment `seq` (of `sid`), from its wire form.
fn ev(m: &mut Model, seq: u64, sid: &str, v: Value) -> Vec<Effect> {
    let e = decode_event(&v).unwrap_or_else(|err| panic!("{err}: {v}"));
    m.apply(Event::session(seq, sid, 0, e, Arc::new(v)))
}

/// The history, as the client hands a resync's reply over.
fn replace(m: &mut Model, seq: u64, sid: &str, msgs: Vec<Value>) {
    m.apply(Event::History { attach_seq: seq, session_id: sid.into(), seq: 0, update: HistoryUpdate::Replace { messages: msgs } });
}

/// The digest after a change: `msgs` and the slot `queued`.
fn digest(msgs: &[Value], queued: Option<&str>) -> Value {
    json!({"ev": "conversation", "digest": {"messages_len": msgs.len(),
        "messages_hash": synaps_client::history::messages_hash(msgs), "queued_message": queued}})
}

/// The card as store `lid` draws it now.
fn painted(m: &Model, lid: LiveId) -> Painted {
    let c = m.lives[lid].transcript.cards().card("t1").expect("the card");
    Painted { tool_id: c.tool_id.clone(), id: c.id.clone().unwrap(), doc: Arc::new(c.doc.clone().unwrap()), wait: c.wait }
}

/// A click on `label` in store `lid`, sent: what went.
fn press(m: &mut Model, lid: LiveId, label: &str) -> Outgoing {
    let p = painted(m, lid);
    let out = m.press_card(lid, &p, pick(label), Instant::now()).expect("sent");
    m.card_sent(lid, out.clone());
    out
}

/// The answer's state on `sid`'s card, and its cause line.
fn state(m: &Model, lid: LiveId, sid: &str) -> Option<(State, Option<String>)> {
    let c = m.lives[lid].transcript.cards().card("t1")?;
    m.memos.entry(sid, c).map(|e| (e.state, e.cause.as_ref().map(Cause::line)))
}

fn one() -> Model {
    let mut m = Model::new(ConnState::Connecting);
    attach(&mut m, LiveId::FIRST, 1, S, &history());
    m
}

/// F1/F2: idle, a click sends one `Submit` (not busy) of the message the
/// spec builds, on the card's attachment; its echo starts the turn and
/// the fold has it: in the chat. The composer gets nothing back.
#[test]
fn a_click_sends_once_on_its_attachment_and_its_echo_lands() {
    let mut m = one();
    let out = press(&mut m, LiveId::FIRST, "Rewrite from scratch");
    assert_eq!(out, Outgoing { seq: 1, busy: false, card: CardId::parse(CARD).unwrap(), text: answer("Rewrite from scratch") });
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Sending, None)));
    // A second click while it is out sends nothing.
    let p = painted(&m, LiveId::FIRST);
    assert_eq!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()), Err(OUT.into()));
    let fx = ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": out.text}));
    assert!(fx.is_empty(), "nothing for a composer: {fx:?}");
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::InChat, None)));
    assert!(m.lives[LiveId::FIRST].in_flight.is_empty(), "the echo confirmed it");
}

/// F16: refused (not the input owner): not sent, the daemon's reason on
/// the card, never in a composer; Retry sends the same text again.
#[test]
fn a_refused_answer_is_the_cards_and_retry_sends_it_again() {
    let mut m = one();
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    let refused = synaps_client::Error::Refused { command: Some("submit".into()), reason: "input is owned by client #1".into() };
    let fx = m.apply(Event::Error { attach_seq: Some(1), error: refused });
    assert!(!fx.iter().any(|e| matches!(e, Effect::Restore(..))), "never to a composer: {fx:?}");
    let (st, cause) = state(&m, LiveId::FIRST, S).unwrap();
    assert_eq!(st, State::NotSent);
    assert!(cause.unwrap().contains("input is owned by client #1"));
    let p = painted(&m, LiveId::FIRST);
    let again = m.retry_card(LiveId::FIRST, &p, Instant::now()).expect("Retry");
    assert_eq!(again.text, out.text);
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Sending));
}

/// F19: an exclusive session owned by another client: read-only here,
/// nothing sent, the gate's reason.
#[test]
fn without_input_authority_nothing_is_sent() {
    let mut m = Model::new(ConnState::Connecting);
    attach_with(
        &mut m,
        LiveId::FIRST,
        1,
        S,
        &history(),
        |v| {
            v["client_input"] = json!("exclusive");
            v["input_owner"] = json!(1);
        },
        false,
    );
    let p = painted(&m, LiveId::FIRST);
    let why = m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()).unwrap_err();
    assert!(why.contains("another client owns"), "{why}");
    assert!(m.memos.entry(S, m.lives[LiveId::FIRST].transcript.cards().card("t1").unwrap()).is_none());
}

/// The binding (Shady #80): a click names the card as painted; once the
/// history binds its id to another card (replaced), or while the history
/// isn't settled, nothing is sent.
#[test]
fn a_click_on_a_stale_binding_or_an_unsettled_view_sends_nothing() {
    let mut m = one();
    let p = painted(&m, LiveId::FIRST);
    // A digest the history doesn't match: the client asks again.
    let mut more = history();
    more.push(json!({"role": "user", "content": "and also"}));
    ev(&mut m, 1, S, digest(&more, None));
    assert_eq!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()), Err(UNSETTLED.into()));
    // The history lands: the extension, restarted, bound the id again.
    let other = json!({"question": "Which branch?", "options": ["main", "dev"]});
    more.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t2", "name": "cards_ui", "input": other}]}));
    more.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t2", "content": showing(CARD, &other)}]}));
    ev(&mut m, 1, S, digest(&more, None));
    replace(&mut m, 1, S, more);
    assert!(m.lives[LiveId::FIRST].transcript.history_settled());
    assert_eq!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()), Err(CHANGED.into()));
    // The history lands with the painted call holding another card, still
    // open (the live rows drew it before the history said what it is): the
    // click names a card the fold no longer holds for that call.
    let mut m = one();
    let p = painted(&m, LiveId::FIRST);
    let mut asked = ask();
    asked["question"] = json!("Which approach, for the lexer?");
    let mut rewritten = history();
    rewritten[1]["content"][1]["input"] = asked.clone();
    rewritten[2]["content"][0]["content"] = json!(showing("c7n2k4d", &asked));
    ev(&mut m, 1, S, digest(&rewritten, None));
    replace(&mut m, 1, S, rewritten);
    let t = &m.lives[LiveId::FIRST].transcript;
    assert!(t.history_settled());
    assert!(t.cards().card("t1").is_some_and(|c| c.answerable()), "the call's card is open");
    assert_eq!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()), Err(CHANGED.into()));
}

/// No answering during a known compaction (§3.4): the card says so, and
/// nothing is sent until it ends.
#[test]
fn no_answer_during_a_known_compaction() {
    let mut m = one();
    let p = painted(&m, LiveId::FIRST);
    ev(&mut m, 1, S, json!({"ev": "compaction_started", "source": "manual", "disclosure": ""}));
    assert_eq!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()), Err(COMPACTING.into()));
    ev(&mut m, 1, S, json!({"ev": "compaction_failed", "message": "no", "panicked": false}));
    assert!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()).is_ok());
}

/// A card in pane A's session after another store (pane B) attaches its
/// own: A's answer goes on A's attachment, and B's events reach none of it.
#[test]
fn a_card_in_one_store_answers_on_its_attachment_after_another_attaches() {
    let mut m = one();
    let b = m.lives.add();
    attach(&mut m, b, 2, S2, &[]);
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    assert_eq!(out.seq, 1, "A's attachment, not B's");
    // B's echo of the same words is B's session's: A's answer stays out.
    ev(&mut m, 2, S2, json!({"ev": "turn_started", "turn_baseline": 0, "trigger": "user", "user_text": out.text}));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Sending));
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": out.text}));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat));
}

/// A tab switch with an answer out: its attachment's refusal and echo
/// still reach the card's memo by its session (never a composer or a
/// tab's draft); back on the tab, it reads what they said.
#[test]
fn a_tab_switch_with_an_answer_out_keeps_it_the_cards() {
    let mut m = one();
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    let _ = m.switch_out(LiveId::FIRST);
    attach(&mut m, LiveId::FIRST, 2, S2, &[]);
    let refused = synaps_client::Error::Refused { command: Some("submit".into()), reason: "input is owned by client #1".into() };
    let fx = m.apply(Event::Error { attach_seq: Some(1), error: refused });
    assert!(fx.is_empty(), "no draft gets it: {fx:?}");
    assert_eq!(m.memos.get_state(S, CARD), Some(State::NotSent));
    // Back on the tab: Retry; then away again before its echo, which the
    // left attachment still delivers.
    let _ = m.switch_out(LiveId::FIRST);
    attach(&mut m, LiveId::FIRST, 3, S, &history());
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::NotSent));
    let p = painted(&m, LiveId::FIRST);
    let again = m.retry_card(LiveId::FIRST, &p, Instant::now()).expect("Retry");
    assert_eq!((again.seq, &again.text), (3, &out.text));
    m.card_sent(LiveId::FIRST, again);
    let _ = m.switch_out(LiveId::FIRST);
    attach(&mut m, LiveId::FIRST, 4, S2, &[]);
    let fx = ev(&mut m, 3, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": out.text}));
    assert!(fx.is_empty(), "{fx:?}");
    assert_eq!(m.memos.get_state(S, CARD), Some(State::Queued), "echoed: on its way");
    let _ = m.switch_out(LiveId::FIRST);
    attach(&mut m, LiveId::FIRST, 5, S, &with(&[&out.text]));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat), "back: the history has it");
}

/// A reconnect with both sessions open, an answer out on each: each
/// checks on its own session's view. One the restarted daemon has is in
/// the chat; one it lost is unconfirmed, the reconnect's cause, with
/// Resend.
#[test]
fn a_reconnect_with_both_sessions_open_checks_each_on_its_own_view() {
    let mut m = one();
    let b = m.lives.add();
    attach(&mut m, b, 2, S2, &history());
    let a_out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    let b_out = press(&mut m, b, "Rewrite from scratch");
    for (seq, sid) in [(1, S), (2, S2)] {
        m.apply(Event::Reattaching { attach_seq: seq, session_id: sid.into(), reason: "daemon reloading".into() });
    }
    // A's daemon kept it; B's lost it.
    attach_with(&mut m, LiveId::FIRST, 1, S, &with(&[&a_out.text]), |_| {}, true);
    attach_with(&mut m, b, 2, S2, &history(), |_| {}, true);
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat));
    let (st, cause) = state(&m, b, S2).unwrap();
    assert_eq!(st, State::Unconfirmed);
    assert_eq!(cause, Some(Cause::Lost(Trigger::Attached).line()));
    let p = painted(&m, b);
    assert!(m.memos.may_resend(S2, &p.id, m.lives[b].transcript.cards()));
    let again = m.resend_card(b, &p, Instant::now()).expect("Resend");
    assert_eq!(again.text, b_out.text);
    assert_eq!(again.seq, 2);
}

/// A daemon-only restart after the answer was in the chat but before it
/// was saved, a rollback below what was seen: `seen` was [A], the
/// history comes back with no answer, and B is offered again (§3.5 rule
/// 4: a shorter F is still a prefix).
#[test]
fn a_rollback_below_what_was_seen_offers_resend() {
    let first = answer("Refactor the parser");
    let mut m = Model::new(ConnState::Connecting);
    attach(&mut m, LiveId::FIRST, 1, S, &with(&[&first]));
    // A change to the second option, sent with seen = [A].
    let p = painted(&m, LiveId::FIRST);
    let change = Reply::Change(cards_spec::parse_answer(&doc(), &json!({"choice": "Rewrite from scratch"})).unwrap());
    let out = m.press_card(LiveId::FIRST, &p, change, Instant::now()).expect("sent");
    m.card_sent(LiveId::FIRST, out.clone());
    // The daemon restarts with neither saved.
    attach_with(&mut m, LiveId::FIRST, 1, S, &history(), |_| {}, true);
    let (st, cause) = state(&m, LiveId::FIRST, S).unwrap();
    assert_eq!((st, cause), (State::Unconfirmed, Some(Cause::Lost(Trigger::Attached).line())));
    let p = painted(&m, LiveId::FIRST);
    assert!(m.memos.may_resend(S, &p.id, m.lives[LiveId::FIRST].transcript.cards()));
}

/// F20, d4: busy, the answer is steered, then a Cancel withdraws it
/// before the engine reads it. `Idle` comes ahead of the history the
/// client asked for again: no Resend until that history settles the view.
#[test]
fn a_cancel_before_the_drain_settles_only_on_the_refreshed_history() {
    let mut m = one();
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "go on"}));
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    assert!(out.busy, "a steer while busy");
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": true}));
    // The slot holds it, as the digest after it says.
    ev(&mut m, 1, S, digest(&history(), Some(&out.text)));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Queued));
    ev(&mut m, 1, S, json!({"ev": "dequeued", "text": out.text}));
    let mut after = history();
    after.push(json!({"role": "user", "content": "go on"}));
    ev(&mut m, 1, S, digest(&after, None));
    ev(&mut m, 1, S, json!({"ev": "idle"}));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Checking), "not settled: no Resend yet");
    let p = painted(&m, LiveId::FIRST);
    assert!(!m.memos.may_resend(S, &p.id, m.lives[LiveId::FIRST].transcript.cards()));
    replace(&mut m, 1, S, after);
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::Lost(Trigger::Dequeued).line()))));
    assert!(m.memos.may_resend(S, &p.id, m.lives[LiveId::FIRST].transcript.cards()));
}

/// d3: two answers at the terminal edge, ours steered, then another
/// steer replaces the one slot before the engine reads ours; the turn
/// ends: ours didn't reach the chat.
#[test]
fn two_answers_at_the_terminal_edge_leave_ours_unconfirmed() {
    let mut m = one();
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "go on"}));
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": true}));
    ev(&mut m, 1, S, json!({"ev": "steered", "text": "actually, stop", "delivered": true}));
    let mut after = history();
    after.push(json!({"role": "user", "content": "go on"}));
    after.push(json!({"role": "assistant", "content": [{"type": "text", "text": "Done."}]}));
    ev(&mut m, 1, S, digest(&after, Some("actually, stop")));
    replace(&mut m, 1, S, after);
    ev(&mut m, 1, S, json!({"ev": "idle"}));
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::Lost(Trigger::Idle).line()))));
}

/// F21, d5: the turn fails with ours in the slot: held (no Resend: the
/// held copy would follow it); the next turn carries it into the chat.
#[test]
fn held_after_an_error_then_in_the_chat() {
    let mut m = one();
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "go on"}));
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": true}));
    let mut after = history();
    after.push(json!({"role": "user", "content": "go on"}));
    ev(&mut m, 1, S, digest(&after, Some(&out.text)));
    replace(&mut m, 1, S, after.clone());
    ev(&mut m, 1, S, json!({"ev": "idle"}));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Held));
    let p = painted(&m, LiveId::FIRST);
    assert!(!m.memos.may_resend(S, &p.id, m.lives[LiveId::FIRST].transcript.cards()));
    // The next turn ends normally: the slot goes in after it.
    after.push(json!({"role": "user", "content": out.text}));
    ev(&mut m, 1, S, digest(&after, None));
    replace(&mut m, 1, S, after);
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat));
}

/// d6: a compaction the desk hadn't heard of when it sent (the
/// unheard-compaction gap): the slot rides it into the compacted history
/// (`queued_restored`), which no longer holds the card: the entry is
/// dropped, never offered again.
#[test]
fn an_answer_in_the_unheard_compaction_gap_rides_it_and_drops() {
    let mut m = one();
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "go on"}));
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": false}));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Queued));
    ev(&mut m, 1, S, json!({"ev": "compaction_started", "source": "auto", "disclosure": ""}));
    ev(
        &mut m,
        1,
        S,
        json!({"ev": "compaction_applied", "previous_session_id": S, "session_id": "j2", "chains_advanced": [],
        "queued_restored": out.text, "msg_count": 3}),
    );
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat), "restored into the history");
    let compacted = vec![
        json!({"role": "user", "content": "<context-summary>parser work</context-summary>"}),
        json!({"role": "assistant", "content": "Understood."}),
        json!({"role": "user", "content": out.text}),
    ];
    ev(&mut m, 1, S, digest(&compacted, None));
    replace(&mut m, 1, S, compacted);
    assert!(m.memos.get_state(S, CARD).is_none(), "the card is gone: so is its entry");
}

/// d7–d9: a driver's FIFO. Full, the only sign is the broadcast notice:
/// checking, then (no echo came) unconfirmed with the notice's cause.
/// Revoked with ours in it: the driver's cause.
#[test]
fn a_drivers_full_queue_and_its_revocation() {
    let mut m = one();
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    ev(&mut m, 1, S, json!({"ev": "system_notice", "text": "steering queue full (16 messages) — message dropped"}));
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::Lost(Trigger::Notice).line()))));
    // Resend: into the FIFO again (idle: a driver's echo), then revoked.
    let p = painted(&m, LiveId::FIRST);
    let again = m.resend_card(LiveId::FIRST, &p, Instant::now()).expect("Resend");
    m.card_sent(LiveId::FIRST, again.clone());
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": false}));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Queued));
    ev(&mut m, 1, S, json!({"ev": "driver_revoked", "reason": "lease ended", "undelivered_steering": [out.text]}));
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::Lost(Trigger::DriverRevoked).line()))));
}

/// A driver's echo across a reattach (rule 5): idle, `Steered{delivered:
/// false}` means the FIFO took it; a reconnect can't say whether it is
/// still there.
#[test]
fn a_drivers_echo_across_a_reattach_may_still_be_queued() {
    let mut m = one();
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": false}));
    attach_with(&mut m, LiveId::FIRST, 1, S, &history(), |_| {}, true);
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::DriverQueue.line()))));
}

/// F17: no echo within 10 s: checking, settled at once on the view held
/// (nothing of ours): unconfirmed, Resend; checked again on the click.
#[test]
fn no_echo_in_ten_seconds_is_checked_then_offered_again() {
    let mut m = one();
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    m.tick_cards(Instant::now() + Duration::from_secs(5));
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Sending));
    m.tick_cards(Instant::now() + Duration::from_secs(11));
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::Lost(Trigger::Timeout).line()))));
    // It did arrive, unseen: the view shows it now, and the click sends
    // nothing (it is in the chat).
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": out.text}));
    let p = painted(&m, LiveId::FIRST);
    assert!(m.resend_card(LiveId::FIRST, &p, Instant::now()).is_err());
    assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat));
}

/// A detach with an answer out: a composer's text goes back to it, the
/// card's stays the card's (no `Restore` of an answer's two lines).
#[test]
fn a_detach_gives_a_composer_its_text_and_keeps_the_cards() {
    let mut m = one();
    m.lives[LiveId::FIRST].sent(PaneId::FIRST, "typed".into());
    press(&mut m, LiveId::FIRST, "Refactor the parser");
    let fx = m.apply(Event::Detached { attach_seq: 1, session_id: Some(S.into()), reason: "gone".into() });
    assert_eq!(
        fx.iter().filter(|e| matches!(e, Effect::Restore(..))).collect::<Vec<_>>(),
        vec![&Effect::Restore(PaneId::FIRST, "typed".into())]
    );
    assert_eq!(m.memos.get_state(S, CARD), Some(State::Sending));
}

/// The card store `lid` draws for call `tool` now.
fn painted_at(m: &Model, lid: LiveId, tool: &str) -> Painted {
    let c = m.lives[lid].transcript.cards().card(tool).expect("the card");
    Painted { tool_id: c.tool_id.clone(), id: c.id.clone().unwrap(), doc: Arc::new(c.doc.clone().unwrap()), wait: c.wait }
}

/// `label` picked on the card asked `input`.
fn pick_in(input: &Value, label: &str) -> Reply {
    let Ok(cards_spec::Input::Create { doc, .. }) = cards_spec::parse_input(input) else { panic!() };
    Reply::Answer(cards_spec::parse_answer(&doc, &json!({"choice": label})).unwrap())
}

/// The agent calls card `tool` asking `input` live, and its result binds
/// `card`: the stream's events, no digest and no history after them.
fn ask_live(m: &mut Model, tool: &str, card: &str, input: &Value) {
    let llm = |v: Value| json!({"ev": "stream", "event": v});
    ev(m, 1, S, llm(json!({"kind": "llm", "llm": "response_start"})));
    ev(m, 1, S, llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": tool})));
    ev(m, 1, S, llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": tool, "input": input})));
    ev(m, 1, S, llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": showing(card, input)})));
}

/// Shady #81 P2-1: the history matches its digest, but the card is the
/// live turn's alone (its call and its result streamed; no digest, no
/// history since). It draws, and a click, Skip included, sends nothing:
/// the history hasn't confirmed its binding. The history lands with it:
/// exactly one click sends.
#[test]
fn a_live_only_binding_sends_nothing_until_the_history_has_it() {
    let before = history()[..1].to_vec();
    let mut m = Model::new(ConnState::Connecting);
    attach(&mut m, LiveId::FIRST, 1, S, &before);
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user"}));
    ask_live(&mut m, "t1", CARD, &ask());
    let t = &m.lives[LiveId::FIRST].transcript;
    assert!(t.history_settled(), "the old history still matches its digest");
    assert!(t.cards().card("t1").is_some_and(|c| c.answerable()), "the live card draws, open");
    let p = painted(&m, LiveId::FIRST);
    assert!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()).is_err(), "a click on a live-only binding");
    assert!(m.press_card(LiveId::FIRST, &p, Reply::Skip, Instant::now()).is_err(), "Skip on it");
    assert!(m.memos.get_state(S, CARD).is_none(), "nothing went");
    // The round's history: the call and its result, as drawn.
    let landed = history()[..3].to_vec();
    ev(&mut m, 1, S, digest(&landed, None));
    replace(&mut m, 1, S, landed);
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    assert_eq!(out.text, answer("Refactor the parser"));
    assert_eq!(m.press_card(LiveId::FIRST, &p, pick("Refactor the parser"), Instant::now()), Err(OUT.into()), "exactly one");
}

/// Shady #81 P2-1, the reused id: the extension restarted and a live call
/// binds the card's id again (another question). Before the history has
/// it, neither card answers: the first is replaced, the second is the live
/// turn's alone. A history that binds the id elsewhere leaves the painted
/// click dead; the matching history lets it send once.
#[test]
fn a_live_rebind_of_the_id_answers_only_once_the_history_has_it() {
    let other = json!({"question": "Which branch?", "options": ["main", "dev"]});
    let mut m = one();
    let first = painted(&m, LiveId::FIRST);
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "and the branch?"}));
    ask_live(&mut m, "t2", CARD, &other);
    let second = painted_at(&m, LiveId::FIRST, "t2");
    assert!(m.lives[LiveId::FIRST].transcript.history_settled());
    assert_eq!(m.press_card(LiveId::FIRST, &first, pick("Refactor the parser"), Instant::now()), Err(CHANGED.into()), "replaced live");
    assert!(m.press_card(LiveId::FIRST, &second, pick_in(&other, "main"), Instant::now()).is_err(), "live only");
    assert!(m.memos.get_state(S, CARD).is_none(), "nothing went");
    // A history where the call bound another id: the painted card isn't it.
    let mut elsewhere = history();
    elsewhere.push(json!({"role": "user", "content": "and the branch?"}));
    elsewhere.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t2", "name": "cards_ui", "input": other}]}));
    let bound_elsewhere = showing("c9z9z9z", &other);
    elsewhere.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t2", "content": bound_elsewhere}]}));
    ev(&mut m, 1, S, digest(&elsewhere, None));
    replace(&mut m, 1, S, elsewhere.clone());
    assert!(m.press_card(LiveId::FIRST, &second, pick_in(&other, "main"), Instant::now()).is_err(), "bound elsewhere");
    // The history as the live turn drew it: one click sends.
    let mut landed = elsewhere;
    landed[6] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t2", "content": showing(CARD, &other)}]});
    ev(&mut m, 1, S, digest(&landed, None));
    replace(&mut m, 1, S, landed);
    let out = m.press_card(LiveId::FIRST, &second, pick_in(&other, "main"), Instant::now()).expect("sent");
    m.card_sent(LiveId::FIRST, out);
    assert!(m.press_card(LiveId::FIRST, &second, pick_in(&other, "dev"), Instant::now()).is_err(), "exactly one");
}

/// Shady #81 P2-2: busy, the answer is steered and the engine takes it in
/// (`SteeringDelivered`); the `Conversation` that clears the slot comes
/// with the persisted history still the old one (the round's checkpoint
/// comes later). It stays answered: no Resend and nothing sent again,
/// whatever comes meanwhile (an unrelated event, the clock), until the
/// history lands with it. With ours in the slot first, and without.
#[test]
fn a_delivered_answer_stays_answered_until_its_history_lands() {
    for slot in [true, false] {
        let mut m = one();
        ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "go on"}));
        let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
        assert!(out.busy, "a steer while busy");
        ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": true}));
        if slot {
            ev(&mut m, 1, S, digest(&history(), Some(&out.text)));
        }
        assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::Queued), "slot {slot}");
        ev(&mut m, 1, S, json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": out.text}}));
        assert_eq!(state(&m, LiveId::FIRST, S).map(|s| s.0), Some(State::InChat), "slot {slot}: delivered");
        ev(&mut m, 1, S, digest(&history(), None));
        assert_eq!(state(&m, LiveId::FIRST, S), Some((State::InChat, None)), "slot {slot}: the old history, the slot cleared");
        ev(&mut m, 1, S, json!({"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": "Refactoring."}}));
        m.tick_cards(Instant::now() + Duration::from_secs(30));
        assert_eq!(state(&m, LiveId::FIRST, S), Some((State::InChat, None)), "slot {slot}: an event, the clock");
        let p = painted(&m, LiveId::FIRST);
        assert!(!m.memos.may_resend(S, &p.id, m.lives[LiveId::FIRST].transcript.cards()), "slot {slot}: no Resend");
        assert!(m.resend_card(LiveId::FIRST, &p, Instant::now()).is_err(), "slot {slot}: nothing sent again");
        // The round's checkpoint: the history with it.
        let mut after = with(&["go on"]);
        after.push(json!({"role": "user", "content": out.text}));
        ev(&mut m, 1, S, digest(&after, None));
        replace(&mut m, 1, S, after);
        assert_eq!(state(&m, LiveId::FIRST, S), Some((State::InChat, None)), "slot {slot}: landed");
    }
}

/// Shady #81 P2-2, the other side: the daemon restarts after the delivery
/// and before its checkpoint, and the reattach's history doesn't have it.
/// That one may be lost: not confirmed, with Resend.
#[test]
fn a_restart_between_the_delivery_and_its_checkpoint_is_not_confirmed() {
    let mut m = one();
    ev(&mut m, 1, S, json!({"ev": "turn_started", "turn_baseline": 4, "trigger": "user", "user_text": "go on"}));
    let out = press(&mut m, LiveId::FIRST, "Refactor the parser");
    ev(&mut m, 1, S, json!({"ev": "steered", "text": out.text, "delivered": true}));
    ev(&mut m, 1, S, json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": out.text}}));
    ev(&mut m, 1, S, digest(&history(), None));
    attach_with(&mut m, LiveId::FIRST, 1, S, &history(), |_| {}, true);
    assert_eq!(state(&m, LiveId::FIRST, S), Some((State::Unconfirmed, Some(Cause::Lost(Trigger::Attached).line()))));
    let p = painted(&m, LiveId::FIRST);
    assert!(m.memos.may_resend(S, &p.id, m.lives[LiveId::FIRST].transcript.cards()));
    let again = m.resend_card(LiveId::FIRST, &p, Instant::now()).expect("Resend sends");
    assert_eq!(again.text, out.text);
}
