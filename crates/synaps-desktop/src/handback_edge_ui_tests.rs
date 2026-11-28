//! D8b's window tests for Shady #115 round 2 (synaps-cards `docs/blocks.md`
//! v1.3 §3.4, §3.5, §3.7, §5.4): who owns a hand-back while it waits and
//! after its session follows, a wait whose store goes, and what the
//! board's caches forgetting something may and may not be read as.
//!
//! The list: a failed wait lets go of its own draft, never a copied
//! session's (R2-1); a follow into a session that keeps a memo of its own
//! takes the source's hand-backs along, on the wire, waiting, or into a
//! full memo (R2-2); a wait whose split-pane store closes or is given up
//! sends nothing and keeps the work (R2-3); an outcome past every mark the
//! board keeps is re-folded from the history (R2-4); agent churn past the
//! revision records is never `another edit arrived`, a real one still is
//! (R2-5); an acknowledged overwrite stays seen past them, a new one shows
//! again (R2-6).

use std::time::Instant;

use cards_spec::memo::State as M;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use super::answer_tests::{attachment, has, heard};
use super::blocks_ui_tests::{board, open_session, split, turn};
use super::drafts_ui_tests::{footer, row, walk_take};
use super::handback_ui_tests::{
    Gate, W, agent, button, close_gate, hold, idle, lands, last, notice_of_waits, notice_waits, other_desks, planned, planned_history,
    refused, release, said_now, sends, sid, state, tag, take_and, the_draft, walk_up,
};
use super::keys_tests::press;
use super::wait;
use super::walk_tests::{enter, landed, said_ok, user};
use crate::app::AppView;
use crate::handback::SENDING;
use crate::live::LiveId;
use crate::pane::PaneId;
use crate::walk::At;

fn tail(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, view, 1000);
}

/// The history of the session on screen replaced by `h`, its board drawn.
fn replace(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, h: Vec<Value>) {
    let (seq, s) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let update = synaps_client::HistoryUpdate::Replace { messages: h };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: s, seq: 0, update }], cx);
    });
    landed(cx, w, view);
    board(cx, w, view);
    wait(cx, w, view, 300);
}

/// Pane `p` focused, the keys its own.
fn focus(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, p: PaneId) {
    view.update(cx, |v, cx| v.focus_pane(p, crate::pane::KeysTo::Pane, cx));
    wait(cx, w, view, 300);
}

fn live_of(cx: &mut TestAppContext, view: &Entity<AppView>, p: PaneId) -> Option<LiveId> {
    view.read_with(cx, |v, _| v.panes.iter().find(|q| q.id == p).map(|q| q.live))
}

/// Two panes: `a` on the blocks demo in the first store, `b` on its twin
/// in a store of its own (not the first), both holding [`planned_history`]:
/// copied history, the same blocks and incarnations.
fn twins(cx: &mut TestAppContext) -> (W, PaneId, PaneId) {
    let (w, view) = planned(cx);
    split(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    let (b, a) = (panes[0], panes[1]);
    focus(cx, w, &view, b);
    open_session(cx, w, &view, crate::demo::BLOCKS_TWIN, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, w, &view);
    replace(cx, w, &view, planned_history());
    super::rail_wishes_ui_tests::written(cx, &view);
    tail(cx, w, &view);
    assert_eq!(live_of(cx, &view, a), Some(LiveId::FIRST), "a: the first store");
    assert!(live_of(cx, &view, b).is_some_and(|l| l != LiveId::FIRST), "b: a store of its own");
    ((w, view), a, b)
}

/// Block `name` dismissed in the walk, its mark written, the walk left.
fn dismiss(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, name: &str) {
    enter(cx, w, view);
    walk_up(cx, w, view, At::Block(row(name)));
    press(cx, w, "x");
    landed(cx, w, view);
    press(cx, w, "escape");
    wait(cx, w, view, 300);
}

/// The walk up to `to`, however far (each `k` one stop up).
fn walk_to(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, to: At) {
    for _ in 0..64 {
        if super::walk_tests::at(cx, view) == Some(to) {
            break;
        }
        press(cx, w, "k");
    }
    wait(cx, w, view, 300);
    assert_eq!(super::walk_tests::at(cx, view), Some(to), "walked to it");
}

/// Session `id` opened again in the pane on screen: the daemon's
/// `Attached` for it, history `h`, on that pane's attachment (one batch:
/// no frame judges a draft on another history).
fn reopen(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str, h: &[Value]) {
    let (seq, _) = attachment(cx, view);
    let frame = json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": id, "model": "m", "lifecycle": "live"},
        "conversation": {"messages_len": h.len(), "api_messages": h}});
    let Ok(synaps_client::protocol::DaemonFrame::Attached(attached)) = synaps_client::protocol::decode_frame(&frame.to_string()) else {
        panic!("an attach frame")
    };
    view.update(cx, |v, cx| {
        let e = synaps_client::Event::Attached { attach_seq: seq, attached, mode: synaps_client::AttachMode::Mirror, reconnect: false };
        v.apply_batch(vec![e], cx)
    });
    cx.run_until_parked();
    board(cx, w, view);
    wait(cx, w, view, 600);
    assert_eq!(sid(cx, view), id, "attached");
    tail(cx, w, view);
}

fn waits(cx: &mut TestAppContext, view: &Entity<AppView>) -> usize {
    view.read_with(cx, |v, _| v.edit_waits.len())
}

// ── R2-1: a failed wait lets go of its own draft ───────────────────────

/// Shady #115 R2-1: two sessions with copied history (one incarnation of
/// `p`), the same edit in each; A's draft made first, B's hand-back
/// waiting first. B's input taken away, the notices' marks written: B's
/// wait fails and lets go of B's draft, held again; A's, never touched by
/// B's failure, goes on its own ticket. Neither waits after.
#[gpui_kit::test]
fn a_failed_wait_lets_go_of_its_own_draft_never_a_copied_sessions(cx: &mut TestAppContext) {
    let ((w, view), a, b) = twins(cx);
    focus(cx, w, &view, a);
    let (sa, ta) = (sid(cx, &view), tag(cx, &view, "p"));
    let seq_a = attachment(cx, &view).0;
    focus(cx, w, &view, b);
    let (sb, tb) = (sid(cx, &view), tag(cx, &view, "p"));
    assert_ne!(sa, sb);
    assert_eq!(ta, tb, "copied history: one incarnation");
    focus(cx, w, &view, a);
    dismiss(cx, w, &view, "n");
    focus(cx, w, &view, b);
    dismiss(cx, w, &view, "n");
    let h = hold(cx, &view);
    // A's draft first in the drafts' order (held: the keys move on).
    focus(cx, w, &view, a);
    take_and(cx, w, &view, &["x"]);
    // B's wait first in the tickets'.
    focus(cx, w, &view, b);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    focus(cx, w, &view, a);
    take_and(cx, w, &view, &[]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    assert!(sends(cx, &view).is_empty(), "both wait");
    assert_eq!(waits(cx, &view), 2);
    let da = the_draft(cx, &view, "p").expect("A's draft");
    focus(cx, w, &view, b);
    let db = the_draft(cx, &view, "p").expect("B's draft");
    assert!(da.1 && db.1 && da.0.is_some() && da.0 == db.0, "the same body, both waiting: {da:?} {db:?}");
    close_gate(cx, w, &view, Gate::Authority);
    release(cx, w, &view, h);
    assert_eq!(sends(cx, &view).iter().map(|o| o.seq).collect::<Vec<_>>(), [seq_a], "A's goes, B's doesn't");
    assert_eq!(the_draft(cx, &view, "p"), Some((None, false, false)), "B's own draft held again, not waiting");
    focus(cx, w, &view, a);
    let (sent, waiting, ended) = the_draft(cx, &view, "p").expect("A's draft");
    assert!(sent.is_some() && !waiting && !ended, "A's on the wire");
    assert_eq!(waits(cx, &view), 0, "no wait left");
}

/// The daemon's `Attached` for `id` with history `h` (and `queued`, the
/// steer its slot holds) on the attachment on screen: the session follows
/// to `id` when it is another.
fn attach(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str, h: &[Value], queued: Option<&str>) {
    let (seq, _) = attachment(cx, view);
    let frame = json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": id, "model": "m", "lifecycle": "live"},
        "conversation": {"messages_len": h.len(), "api_messages": h, "queued_message": queued}});
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

/// Shady #115 R2-1, across a follow: a waiting hand-back whose session
/// follows to `T`, which holds a changed draft of the same incarnation (the
/// one that came is doubled, in the dock). The follow moved the
/// attachment: nothing goes, and only the doubled draft is let go of; T's
/// own stays as it was, held. Neither waits. Then T's own is handed back
/// and waits: the wait is its own, and it goes once.
#[gpui_kit::test]
fn a_failed_wait_across_a_follow_lets_go_of_the_doubled_draft_only(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let s = sid(cx, &view);
    let t = format!("{s}-t");
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    let p = cards_spec::block::Ref::parse("p").unwrap();
    let ours = view.update(cx, |v, _| {
        let d = v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board");
        let b = d.rows.values().flat_map(|r| r.blocks.iter()).find(|x| x.block.id == p).expect("drawn").block.clone();
        let mine = v.block_drafts.of(&s).find(|d| d.id == p).map(|d| d.key).expect("ours");
        let theirs = v.block_drafts.take(&t, &b, d.generation.expect("a generation")).expect("room");
        theirs.stop(&cards_spec::block::ItemId::parse("test").unwrap()).expect("todo");
        (mine, theirs.key)
    });
    // T's history: the same, and a note `m` after (a notice to wait on
    // later).
    let mut th = planned_history();
    th.extend(agent("toolu_m", json!([{"op":"put","id":"m","kind":"note","body":{"text":"Mind the cache"}}])));
    attach(cx, w, &view, &t, &th, None);
    assert_eq!(sid(cx, &view), t, "followed");
    let fates = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            [ours.0, ours.1].map(|k| v.block_drafts.by_key(k).map(|(s, d)| (s.to_string(), d.sent.is_some(), d.ended)))
        })
    };
    let [m, th] = fates(cx);
    assert_eq!(m.as_ref().map(|m| (m.0.as_str(), m.2)), Some((t.as_str(), Some(crate::drafts::Fate::Doubled))), "ours doubled: {m:?}");
    assert_eq!(th, Some((t.clone(), false, None)), "T's held");
    release(cx, w, &view, h);
    assert!(sends(cx, &view).is_empty(), "doubled: nothing goes");
    let [m, th] = fates(cx);
    assert_eq!(m, Some((t.clone(), false, Some(crate::drafts::Fate::Doubled))), "the doubled one let go of, its text kept");
    assert_eq!(th, Some((t.clone(), false, None)), "T's as it was");
    assert_eq!(waits(cx, &view), 0);
    // T's own handed back while the notice's mark waits again: the wait is
    // its own draft's, not the doubled one's (first in the drafts' order).
    let h = notice_of_waits(cx, w, &view, "m");
    walk_take(cx, w, &view, 1);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    assert_eq!(waits(cx, &view), 1, "T's own waits");
    let waiting = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| [ours.0, ours.1].map(|k| v.block_drafts.by_key(k).is_some_and(|(_, d)| d.waiting.is_some())))
    };
    assert_eq!(waiting(cx), [false, true], "the wait is T's own draft's");
    release(cx, w, &view, h);
    assert_eq!(sends(cx, &view).len(), 1, "it goes, once");
    assert_eq!(waiting(cx), [false, false], "neither waits");
    let [m, th] = fates(cx);
    assert_eq!(th.map(|x| x.1), Some(true), "T's own on the wire");
    assert_eq!(m.map(|x| x.2), Some(Some(crate::drafts::Fate::Doubled)), "the doubled one as it was");
}

// ── R2-2: a follow into a session that keeps a memo ───────────────────

/// Hand-back `text` (an edit of the session on screen's board) made on
/// `session`'s memo directly, as a desk that sent it there would have.
fn memo_hand_back(cx: &mut TestAppContext, view: &Entity<AppView>, session: &str, text: &str) -> String {
    let e = cards_spec::block::message::parse(text).expect("an edit");
    let words = text.split('\n').next().and_then(|l| l.split_once("] ")).map(|(_, w)| w).expect("line 1");
    let (title, piece) = words.split_once(" → ").expect("title and piece");
    let l1 = crate::handback::Line1 { title: Some(title.into()), back_to: None, pieces: vec![piece.into()] };
    view.update(cx, |v, _| {
        let lid = v.panes.cur.live;
        let m = &mut v.model;
        let t = &m.lives[lid].transcript;
        let d = t.drawn().cloned();
        m.memos.edits.hand_back(session, &e, &l1, &Default::default(), t, d.as_deref(), Instant::now()).expect("admitted")
    })
}

/// `session`'s hand-back `text` refused: not sent, recoverable, no flight.
fn memo_refused(cx: &mut TestAppContext, view: &Entity<AppView>, session: &str, text: &str) {
    let hit = view.update(cx, |v, _| v.model.memos.edits.refused(session, text, "busy", Instant::now()));
    assert!(hit, "refused");
}

/// `session`'s hand-back `text` echoed, then taken in by the engine: in
/// the chat.
fn memo_delivered(cx: &mut TestAppContext, view: &Entity<AppView>, session: &str, text: &str) {
    view.update(cx, |v, _| {
        let e = &mut v.model.memos.edits;
        e.on(session, cards_spec::memo::Event::Echo { text, driver: false }, Instant::now());
        e.on(session, cards_spec::memo::Event::Delivered { text }, Instant::now());
    });
}

fn note_edit(of: &str, k: usize) -> String {
    format!("[block n edited r1] Root cause → text\n{{\"of\":\"{of}\",\"set\":{{\"text\":\"T's cause {k}\"}}}}")
}

fn test_skip(of: &str) -> String {
    format!("[block p edited r1] Close the race → skip test\n{{\"of\":\"{of}\",\"set\":{{\"steps.test.state\":\"skipped\"}}}}")
}

/// What the desk's hand-back memos keep: memos, entries, bytes.
fn kept(cx: &mut TestAppContext, view: &Entity<AppView>, session: &str) -> (usize, usize, usize) {
    view.read_with(cx, |v, _| {
        let e = &v.model.memos.edits;
        (e.len(), e.entries(session).count(), e.bytes())
    })
}

/// Shady #115 R2-2 (P1): a steer on the wire, then the session follows to
/// `T`, whose memo keeps two hand-backs not sent (the note's, and a skip
/// of p's `test`: the same incarnation, disjoint, no flight). The steer
/// in T's slot: ours is T's now, in flight, its draft drawn on; no second
/// hand-back of p goes; T's own two are still there, recoverable. Both
/// memos are counted. Its history then shows it: in the chat, and once
/// the board read it the draft goes; T's two stay.
#[gpui_kit::test]
fn a_follow_into_a_session_with_a_memo_takes_the_hand_back_on_the_wire(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    let s = sid(cx, &view);
    let t = format!("{s}-t");
    let (of_n, of_p) = (tag(cx, &view, "n"), tag(cx, &view, "p"));
    let tn = memo_hand_back(cx, &view, &t, &note_edit(&of_n, 0));
    memo_refused(cx, &view, &t, &tn);
    let tp = memo_hand_back(cx, &view, &t, &test_skip(&of_p));
    memo_refused(cx, &view, &t, &tp);
    turn(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view);
    assert!(ours.busy, "a steer");
    attach(cx, w, &view, &t, &planned_history(), Some(&ours.text));
    assert_eq!(sid(cx, &view), t, "followed");
    let st = state(cx, &view, &ours.body);
    assert!(matches!(st, Some(M::Held | M::Queued | M::Checking | M::Sending)), "ours, T's now, in flight: {st:?}");
    assert_eq!((state(cx, &view, &tn), state(cx, &view, &tp)), (Some(M::NotSent), Some(M::NotSent)), "T's own, as they were");
    let (sent, waiting, _) = the_draft(cx, &view, "p").expect("its draft draws on: its memo is the session's");
    assert!(sent.is_some() && !waiting);
    let (memos, entries, bytes) = kept(cx, &view, &t);
    assert!(memos <= crate::handback::SESSIONS_MAX && entries == 3, "{memos} memos, {entries} entries");
    assert!(bytes >= entries * cards_spec::memo::block::ENTRY_MAX_BYTES, "each entry counted: {bytes}");
    // One edit of p in flight: no second hand-back.
    let before = sends(cx, &view).len();
    walk_take(cx, w, &view, 0);
    assert!(footer(cx, w, "p", SENDING), "the block says why");
    press(cx, w, "enter");
    press(cx, w, "escape");
    press(cx, w, "escape");
    assert_eq!(sends(cx, &view).len(), before, "no second hand-back of p");
    let retry = |cx: &mut TestAppContext, body: &str| {
        view.read_with(cx, |v, _| {
            let l = &v.model.lives[v.panes.cur.live];
            v.model.memos.edits.may_retry(&t, body, &l.transcript, l.transcript.drawn().map(|d| &**d))
        })
    };
    assert!(!retry(cx, &tp), "nor a Retry of T's own of p (another memo's) while ours is out");
    assert!(retry(cx, &tn), "the note's may go");
    // Delivered under T.
    heard(cx, w, &view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": ours.text}})]);
    lands(cx, w, &view, vec![user(&ours.text), said_ok()]);
    idle(cx, w, &view);
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert_eq!(state(cx, &view, &ours.body), Some(M::InChat), "the exact entry settles");
    assert!(the_draft(cx, &view, "p").is_none(), "the board read it: the draft went");
    assert_eq!((state(cx, &view, &tn), state(cx, &view, &tp)), (Some(M::NotSent), Some(M::NotSent)), "T's own stay");
    let d = super::handback_ui_tests::delivery(cx, &view, &tp);
    assert!(has(cx, w, format!("edit-delivery {p} {d}")), "T's skip of test, recoverable on its block: {d}");
    assert!(retry(cx, &tp), "ours settled: T's own of p may go");
}

/// Shady #115 R2-2 (P1): a hand-back waiting for its notices' marks when
/// its session follows to `T` (on the same attachment), whose memo keeps
/// one of its own: the draft is T's, still waiting; once the marks are
/// written it goes on T, once, into the memo T keeps, its draft on the
/// wire, and T's own entry stays. A second hand-back of p doesn't go.
#[gpui_kit::test]
fn a_follow_into_a_session_with_a_memo_sends_a_waiting_hand_back_there(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let s = sid(cx, &view);
    let t = format!("{s}-t");
    let of_n = tag(cx, &view, "n");
    let tn = memo_hand_back(cx, &view, &t, &note_edit(&of_n, 0));
    memo_refused(cx, &view, &t, &tn);
    let h = notice_waits(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    wait(cx, w, &view, 300);
    attach(cx, w, &view, &t, &planned_history(), None);
    assert_eq!(sid(cx, &view), t, "followed");
    let (sent, waiting, _) = the_draft(cx, &view, "p").expect("T's now");
    assert!(sent.is_some() && waiting, "still waiting");
    release(cx, w, &view, h);
    assert_eq!(sends(cx, &view).len(), 1, "the same attachment, the same conversation: it goes, once");
    let out = last(cx, &view).body;
    assert_eq!(state(cx, &view, &out), Some(M::Sending), "T's memo has it");
    let (sent, waiting, _) = the_draft(cx, &view, "p").expect("on the wire");
    assert!(sent.as_deref() == Some(out.as_str()) && !waiting);
    assert_eq!(state(cx, &view, &tn), Some(M::NotSent), "T's own stays");
    assert_eq!(kept(cx, &view, &t).0, 1, "one memo: T's, ours in it");
    walk_take(cx, w, &view, 0);
    assert!(footer(cx, w, "p", SENDING), "one in flight");
    press(cx, w, "escape");
    assert_eq!(sends(cx, &view).len(), 1);
}

/// Shady #115 R2-2 (P1): a follow into `T` whose memo is full (32 entries,
/// in the chat or covered): nothing of either is lost or merged past a
/// cap. Ours is T's, in flight, its draft on; the desk keeps two memos,
/// each within B4's 32, all counted; a new hand-back of the note goes
/// into T's memo and keeps it at 32.
#[gpui_kit::test]
fn a_follow_into_a_full_memo_keeps_both_within_their_caps(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let s = sid(cx, &view);
    let t = format!("{s}-t");
    let of_n = tag(cx, &view, "n");
    for k in 0..40 {
        let x = memo_hand_back(cx, &view, &t, &note_edit(&of_n, k));
        memo_delivered(cx, &view, &t, &x);
    }
    assert_eq!(kept(cx, &view, &t).1, cards_spec::memo::block::CAP, "T's memo is full");
    turn(cx, w, &view);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view);
    attach(cx, w, &view, &t, &planned_history(), Some(&ours.text));
    assert_eq!(sid(cx, &view), t, "followed");
    let st = state(cx, &view, &ours.body);
    assert!(matches!(st, Some(M::Held | M::Queued | M::Checking | M::Sending)), "ours, T's now: {st:?}");
    assert!(the_draft(cx, &view, "p").is_some_and(|d| d.0.is_some()), "its draft draws on");
    let (memos, entries, bytes) = kept(cx, &view, &t);
    assert_eq!(entries, cards_spec::memo::block::CAP + 1, "none lost");
    assert!(memos <= crate::handback::SESSIONS_MAX && bytes >= entries * cards_spec::memo::block::ENTRY_MAX_BYTES, "{memos} {bytes}");
    super::handback_ui_tests::retitle_note(cx, w, &view, "The cause");
    let n = last(cx, &view).body;
    assert!(n.starts_with("[block n "), "{n}");
    assert_eq!(state(cx, &view, &n), Some(M::Sending), "a note's hand-back goes");
    assert_eq!(kept(cx, &view, &t).1, cards_spec::memo::block::CAP + 1, "T's memo kept at 32, ours beside it");
    assert!(matches!(state(cx, &view, &ours.body), Some(M::Held | M::Queued | M::Checking | M::Sending)), "ours untouched");
}

// ── R2-3: a wait whose store goes ──────────────────────────────────────

/// What waits for its notices' marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Waits {
    New,
    Retry,
    Resend,
    Restore,
}

/// `what` made to wait in the pane on screen: the writer held, and the
/// hand-back's body (none for a restore: nothing went yet).
fn make_wait(
    cx: &mut TestAppContext,
    w: AnyWindowHandle,
    view: &Entity<AppView>,
    what: Waits,
) -> (crate::blockfiles::Hold, Option<String>) {
    let p = row("p");
    let (h, body) = match what {
        Waits::New => {
            let h = notice_waits(cx, w, view);
            take_and(cx, w, view, &["x"]);
            press(cx, w, "enter");
            (h, None)
        }
        Waits::Retry => {
            take_and(cx, w, view, &["x"]);
            press(cx, w, "enter");
            let body = last(cx, view).body;
            refused(cx, w, view, "busy");
            assert_eq!(state(cx, view, &body), Some(M::NotSent));
            let h = notice_waits(cx, w, view);
            button(cx, w, format!("edit-button {p} Retry"));
            (h, Some(body))
        }
        Waits::Resend => {
            turn(cx, w, view);
            take_and(cx, w, view, &["x"]);
            press(cx, w, "enter");
            let out = last(cx, view);
            heard(cx, w, view, vec![json!({"ev": "steered", "text": out.text, "delivered": true})]);
            heard(cx, w, view, vec![json!({"ev": "dequeued", "text": out.text})]);
            idle(cx, w, view);
            assert_eq!(state(cx, view, &out.body), Some(M::Unconfirmed));
            let h = notice_waits(cx, w, view);
            button(cx, w, format!("edit-button {p} Resend"));
            (h, Some(out.body))
        }
        Waits::Restore => {
            lands(cx, w, view, agent("toolu_e", json!([{"op":"patch","id":"n","set":{"text":"It is the cache"}}])));
            let h = notice_of_waits(cx, w, view, "p");
            enter(cx, w, view);
            walk_up(cx, w, view, At::Block(row("n")));
            press(cx, w, "[");
            wait(cx, w, view, 600);
            press(cx, w, "shift-r");
            board(cx, w, view);
            (h, None)
        }
    };
    wait(cx, w, view, 300);
    assert_eq!(waits(cx, view), 1, "{what:?} waits");
    (h, body)
}

/// How a store goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leave {
    /// Its last pane closes.
    Close,
    /// Its last pane switches to a session another pane's store shows.
    Share,
}

/// Shady #115 R2-3 (P1): `what` waits in split pane `b`'s store (not the
/// first); `leave` takes the store away; the marks are written. Nothing
/// panics or goes on any wire, no wait is left, the notices are the
/// queue's again (they ride the next message), and the work is there once
/// the session is opened again: a draft held, an entry not sent or not
/// confirmed with its button, a restore pressed again.
fn a_wait_whose_store_goes(cx: &mut TestAppContext, what: Waits, leave: Leave) {
    let ((w, view), a, b) = twins(cx);
    focus(cx, w, &view, b);
    let gone = live_of(cx, &view, b).expect("b's store");
    let sb = sid(cx, &view);
    let (h, body) = make_wait(cx, w, &view, what);
    let before = sends(cx, &view).len();
    match leave {
        Leave::Close => {
            view.update(cx, |v, cx| {
                v.in_pane(b, |v| v.close_pane());
                cx.notify();
            });
            for _ in 0..4 {
                wait(cx, w, &view, 500);
            }
        }
        Leave::Share => {
            open_session(cx, w, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
            assert_eq!(live_of(cx, &view, b), Some(LiveId::FIRST), "b shares a's store");
        }
    }
    assert!(view.read_with(cx, |v, _| v.model.lives.get(gone).is_none()), "{what:?} {leave:?}: b's store went");
    // Its waits went with it, at once: not left for the marks to find.
    assert_eq!(waits(cx, &view), 0, "{what:?} {leave:?}: its waits went with it");
    let waiting = view.read_with(cx, |v, _| v.block_drafts.of(&sb).any(|d| d.waiting.is_some() || d.sent.is_some()));
    assert!(!waiting, "{what:?} {leave:?}: no draft of it waits or is out");
    let carried = view.read_with(cx, |v, _| v.wishes.desires.carried());
    release(cx, w, &view, h);
    assert_eq!(sends(cx, &view).len(), before, "{what:?} {leave:?}: nothing on the wire");
    let now = view.read_with(cx, |v, _| v.wishes.desires.carried());
    assert_eq!(now + 1, carried, "{what:?} {leave:?}: its notices re-armed as the marks landed, its carrier let go of");
    assert_eq!(waits(cx, &view), 0, "{what:?} {leave:?}: no wait left");
    // The session again, in a's pane.
    focus(cx, w, &view, a);
    let mut h = planned_history();
    if what == Waits::Restore {
        h.extend(agent("toolu_e", json!([{"op":"patch","id":"n","set":{"text":"It is the cache"}}])));
    }
    reopen(cx, w, &view, &sb, &h);
    let p = row("p");
    match what {
        Waits::New => {
            assert_eq!(the_draft(cx, &view, "p"), Some((None, false, false)), "{leave:?}: the draft held, its text kept");
            take_and(cx, w, &view, &[]);
            press(cx, w, "enter");
            landed(cx, w, &view);
        }
        Waits::Retry => {
            let body = body.expect("a body");
            assert_eq!(state(cx, &view, &body), Some(M::NotSent), "{leave:?}: as it was");
            button(cx, w, format!("edit-button {p} Retry"));
        }
        Waits::Resend => {
            let body = body.expect("a body");
            assert_eq!(state(cx, &view, &body), Some(M::Unconfirmed), "{leave:?}: as it was");
            button(cx, w, format!("edit-button {p} Resend"));
        }
        Waits::Restore => {
            enter(cx, w, &view);
            walk_up(cx, w, &view, At::Block(row("n")));
            press(cx, w, "[");
            wait(cx, w, &view, 600);
            press(cx, w, "shift-r");
            board(cx, w, &view);
            wait(cx, w, &view, 300);
        }
    }
    assert_eq!(sends(cx, &view).len(), before + 1, "{what:?} {leave:?}: sent again, once");
    let out = last(cx, &view);
    let notice = if what == Waits::Restore { "[block p v1 dismissed]" } else { "[block n v1 dismissed]" };
    assert!(out.text.ends_with(&format!("\n{notice}")), "{what:?} {leave:?}: its notice rides it again: {}", out.text);
}

#[gpui_kit::test]
fn a_wait_whose_pane_closes_sends_nothing_and_keeps_the_work(cx: &mut TestAppContext) {
    for what in [Waits::New, Waits::Retry, Waits::Resend, Waits::Restore] {
        a_wait_whose_store_goes(cx, what, Leave::Close);
    }
}

#[gpui_kit::test]
fn a_wait_whose_pane_shares_another_store_sends_nothing_and_keeps_the_work(cx: &mut TestAppContext) {
    for what in [Waits::New, Waits::Retry, Waits::Resend, Waits::Restore] {
        a_wait_whose_store_goes(cx, what, Leave::Share);
    }
}

// ── R2-4: an outcome past every mark ──────────────────────────────────

/// `k` agent patches of block `id`'s `path`, 16 a call (`name` makes the
/// calls' ids and the values).
fn churn(id: &str, path: &str, k: usize, name: &str) -> Vec<Value> {
    let all: Vec<usize> = (0..k).collect();
    all.chunks(16)
        .enumerate()
        .flat_map(|(c, ks)| {
            let ops: Vec<Value> = ks
                .iter()
                .map(|i| {
                    let mut set = serde_json::Map::new();
                    set.insert(path.into(), json!(format!("{name} {i}")));
                    json!({"op":"patch","id":id,"set":set})
                })
                .collect();
            agent(&format!("toolu_{name}{c}"), json!(ops))
        })
        .collect()
}

/// Past the board's 1,024 records in all: 17 notes put, each patched to
/// its 64 (`name` makes the ids).
fn churn_all(name: &str) -> Vec<Value> {
    let ids: Vec<String> = (0..17).map(|k| format!("{name}{k}")).collect();
    let puts: Vec<Value> = ids.iter().map(|id| json!({"op":"put","id":id,"kind":"note","body":{"text":"churn"}})).collect();
    let mut h: Vec<Value> = puts.chunks(16).enumerate().flat_map(|(c, ops)| agent(&format!("toolu_{name}put{c}"), json!(ops))).collect();
    for id in &ids {
        h.extend(churn(id, "text", 63, &format!("{id}x")));
    }
    h
}

/// Whether the board on screen still marks row `text`'s message anywhere:
/// its outcome, an op log line, a revision record.
fn marked(cx: &mut TestAppContext, view: &Entity<AppView>, text: &str) -> (bool, bool, bool) {
    view.read_with(cx, |v, _| {
        let t = &v.model.lives[v.panes.cur.live].transcript;
        let id = t.items().iter().find(|i| i.text == text).map(|i| i.id).expect("its row");
        let d = t.drawn().expect("a board");
        (
            d.outcomes.iter().any(|o| o.row == Some(id)),
            d.log.iter().any(|(_, m)| m.row == Some(id)),
            d.records.iter().any(|(_, m)| m.row == Some(id)),
        )
    })
}

/// Row `text` drawn until it says `want` (at most 20 frames' worth): what
/// it said each time.
fn drawn_until(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str, want: &str) -> Vec<Option<String>> {
    let mut said = Vec::new();
    for _ in 0..20 {
        let now = super::handback_ui_tests::drawn_says(cx, w, view, text, want);
        said.push(said_now(cx, view, text));
        if now {
            break;
        }
        landed(cx, w, view);
    }
    said
}

/// Shady #115 R2-4: our hand-back, then 34 others (its outcome let go of)
/// and 300 agent patches of p (its log line and its revision record let
/// go of): nothing the board keeps marks it, the history still holds it,
/// far below the re-fold's reach. Its row re-folds it: `loading…`, then
/// `applied`, never `too far back`. One that applied nothing (the agent
/// ran its step first: no revision record ever) the same: what it said
/// while the board kept it. A re-fold that is past its reach says so.
#[gpui_kit::test]
fn an_outcome_past_every_mark_the_board_keeps_is_refolded_from_the_history(cx: &mut TestAppContext) {
    use crate::handback_view::{LOADING, TOO_FAR};
    for ran_first in [false, true] {
        let (w, view) = planned(cx);
        take_and(cx, w, &view, &["x"]);
        press(cx, w, "enter");
        let ours = last(cx, &view).text;
        let mut h = Vec::new();
        if ran_first {
            h.extend(agent("toolu_ran", json!([{"op":"patch","id":"p","set":{"steps.fix.state":"doing"}}])));
        }
        h.extend([user(&ours), said_ok()]);
        lands(cx, w, &view, h);
        let then = said_now(cx, &view, &ours).expect("what it did");
        assert_eq!(then == "applied", !ran_first, "{then}");
        let mut h = other_desks(&tag(cx, &view, "n"), 34);
        h.extend(churn("p", "steps.test.label", 300, "lab"));
        lands(cx, w, &view, h);
        assert_eq!(marked(cx, &view, &ours), (false, false, false), "ran first {ran_first}: no mark of it is kept");
        let said = drawn_until(cx, w, &view, &ours, &then);
        assert_eq!(said.last().cloned().flatten().as_deref(), Some(then.as_str()), "ran first {ran_first}: re-folded ({said:?})");
        assert!(
            said.iter().all(|s| s.as_deref() == Some(LOADING) || s.as_deref() == Some(then.as_str())),
            "ran first {ran_first}: loading until then, never too far: {said:?}"
        );
    }
    // A re-fold past the reach (200,000 events: the scratch's verdict,
    // put here) says so.
    let (w, view) = planned(cx);
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view).text;
    let mut h = vec![user(&ours), said_ok()];
    h.extend(other_desks(&tag(cx, &view, "n"), 34));
    h.extend(churn("p", "steps.test.label", 300, "lab"));
    lands(cx, w, &view, h);
    view.update(cx, |v, _| {
        v.outcomes_kept.clear();
        v.outcome_asked = None;
        v.refold_cancel();
        v.refolds.far = true;
    });
    let said = drawn_until(cx, w, &view, &ours, TOO_FAR);
    assert_eq!(said.last().cloned().flatten().as_deref(), Some(TOO_FAR), "past the reach: too far back ({said:?})");
    assert!(view.read_with(cx, |v, _| v.refolds.started) > 0);
}

// ── R2-5: `another edit arrived` is a user's revision, seen ───────────

/// The held draft of `p`: whether it says another edit arrived.
fn other(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    let s = sid(cx, view);
    let p = cards_spec::block::Ref::parse("p").unwrap();
    view.read_with(cx, |v, _| v.block_drafts.of(&s).find(|d| d.id == p).map(|d| d.other)).expect("p's draft")
}

/// `p` at revision `rev` on the board, and whether its record of `r2` is
/// still kept.
fn p_records(cx: &mut TestAppContext, view: &Entity<AppView>) -> (u32, bool) {
    view.read_with(cx, |v, _| {
        let d = v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board");
        let p = cards_spec::block::Ref::parse("p").unwrap();
        let b = d.rows.values().flat_map(|r| r.blocks.iter()).find(|x| x.block.id == p).expect("drawn").block.clone();
        (b.rev, d.records.iter().any(|(r, _)| r.tag == b.tag && r.rev == 2))
    })
}

/// Another desk's hand-back of p at r1: `test` renamed.
fn theirs(of: &str) -> Vec<Value> {
    let t = format!("[block p edited r1] Close the race → rename test\n{{\"of\":\"{of}\",\"set\":{{\"steps.test.label\":\"Theirs\"}}}}");
    vec![user(&t), said_ok()]
}

/// Shady #115 R2-5: p held at r1; in one history, the agent alone takes it
/// past its 64 records (r2's let go of), or past the board's 1,024 in all:
/// no `another edit arrived`, for nobody else's edit arrived. The same
/// with another desk's edit at r2 first: it arrived, and it says so.
#[gpui_kit::test]
fn another_edit_arrived_is_a_users_revision_never_a_record_let_go_of(cx: &mut TestAppContext) {
    for (global, real) in [(false, false), (true, false), (false, true), (true, true)] {
        let (w, view) = planned(cx);
        take_and(cx, w, &view, &["x"]);
        let of = tag(cx, &view, "p");
        let mut h =
            if real { theirs(&of) } else { agent("toolu_a2", json!([{"op":"patch","id":"p","set":{"steps.test.label":"Agent's"}}])) };
        h.extend(if global { churn_all("g") } else { churn("p", "steps.deploy.label", 66, "dep") });
        lands(cx, w, &view, h);
        let (rev, r2) = p_records(cx, &view);
        assert!(!r2 && rev >= 2, "global {global} real {real}: r2's record let go of (r{rev})");
        assert_eq!(other(cx, &view), real, "global {global} real {real}: another edit arrived only for a real one");
    }
}

// ── R2-6: an acknowledged overwrite stays seen ─────────────────────────

/// `changed your edit` on p, as it shows now.
fn chip(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.update(cx, |v, _| v.block_shown(row("p")).and_then(|(_, s)| v.changed_your_edit(&s)).map(|p| p.as_str().to_string()))
}

/// p taken (walked up to), `fix` renamed `label`, handed back and in the
/// history.
fn rename_fix(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, label: &str) {
    enter(cx, w, view);
    walk_to(cx, w, view, At::Block(row("p")));
    press(cx, w, "e");
    wait(cx, w, view, 300);
    press(cx, w, "r");
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
}

/// p taken and let go of, unchanged: a focus.
fn focus_p(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    enter(cx, w, view);
    walk_to(cx, w, view, At::Block(row("p")));
    press(cx, w, "e");
    wait(cx, w, view, 300);
    assert!(view.read_with(cx, |v, _| v.panes.cur.edit.at.is_some()), "taken");
    press(cx, w, "escape");
    wait(cx, w, view, 300);
    assert!(view.read_with(cx, |v, _| v.panes.cur.edit.at.is_none()), "let go of");
}

/// Shady #115 R2-6: the user renames `fix`, the agent overwrites it, a
/// focus sees it. Then the agent alone takes p past its 64 records, or
/// the board past its 1,024, `fix` still the agent's: still seen (no new
/// overwrite). The user renames it again, the agent overwrites it again:
/// the chip again, and the next focus clears it.
#[gpui_kit::test]
fn an_overwrite_seen_stays_seen_past_the_records_and_a_new_one_shows(cx: &mut TestAppContext) {
    for global in [false, true] {
        let (w, view) = planned(cx);
        rename_fix(cx, w, &view, "Fix it now");
        lands(cx, w, &view, agent("toolu_o1", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race"}}])));
        assert_eq!(chip(cx, &view).as_deref(), Some("steps.fix.label"), "global {global}: the overwrite");
        focus_p(cx, w, &view);
        assert_eq!(chip(cx, &view), None, "global {global}: seen");
        let mut h = agent("toolu_t1", json!([{"op":"patch","id":"p","set":{"steps.test.state":"doing"}}]));
        h.extend(if global { churn_all("g") } else { churn("p", "steps.deploy.label", 66, "dep") });
        lands(cx, w, &view, h);
        assert!(!p_records(cx, &view).1, "global {global}: p's early records let go of");
        assert_eq!(chip(cx, &view), None, "global {global}: no new overwrite: still seen");
        rename_fix(cx, w, &view, "Fix it again");
        assert_eq!(chip(cx, &view), None, "global {global}: the user's now");
        lands(cx, w, &view, agent("toolu_o2", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race again"}}])));
        assert_eq!(chip(cx, &view).as_deref(), Some("steps.fix.label"), "global {global}: the new overwrite shows");
        focus_p(cx, w, &view);
        assert_eq!(chip(cx, &view), None, "global {global}: and its focus clears it");
    }
}

/// Shady #115 R2-6, its other side: the overwrite seen, then the user
/// renames `fix` again and the agent overwrites it again, and the agent
/// alone takes p past its records (the user's revision let go of): the
/// user's hand-back since says the user wrote it, so the new overwrite
/// shows.
#[gpui_kit::test]
fn a_new_overwrite_past_the_records_shows(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    rename_fix(cx, w, &view, "Fix it now");
    lands(cx, w, &view, agent("toolu_o1", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race"}}])));
    focus_p(cx, w, &view);
    assert_eq!(chip(cx, &view), None, "seen");
    rename_fix(cx, w, &view, "Fix it again");
    let mut h = agent("toolu_o2", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix the race again"}}]));
    h.extend(churn("p", "steps.deploy.label", 66, "dep"));
    lands(cx, w, &view, h);
    assert!(!p_records(cx, &view).1, "p's early records let go of");
    let users = view.read_with(cx, |v, _| {
        let d = v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board");
        let p = cards_spec::block::Ref::parse("p").unwrap();
        let tag = d.rows.values().flat_map(|r| r.blocks.iter()).find(|x| x.block.id == p).expect("drawn").block.tag;
        d.records.iter().filter(|(r, _)| r.tag == tag && r.author == cards_spec::block::Author::User).count()
    });
    assert_eq!(users, 0, "no user record of p is kept");
    assert_eq!(chip(cx, &view).as_deref(), Some("steps.fix.label"), "the user's hand-back since says so: it shows");
}

/// Shady #115 R2-2 (P1), settlement's side: a draft on the wire whose
/// session's memos don't have its edit (another owns it: here put there
/// by hand, the state a stranded follow left) is never settled, whatever
/// the history shows; once its owner is the session's again, its entry
/// settles and the draft goes.
#[gpui_kit::test]
fn a_sent_draft_is_settled_only_by_the_memo_that_owns_its_edit(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let s = sid(cx, &view);
    let elsewhere = format!("{s}-x");
    take_and(cx, w, &view, &["x"]);
    press(cx, w, "enter");
    let ours = last(cx, &view);
    view.update(cx, |v, _| v.model.memos.edits.follow(&s, &elsewhere, Instant::now()));
    assert_eq!(state(cx, &view, &ours.body), None, "the session's memos don't have it");
    lands(cx, w, &view, vec![user(&ours.text), said_ok()]);
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    let (sent, waiting, _) = the_draft(cx, &view, "p").expect("in flight elsewhere: never settled");
    assert!(sent.is_some() && !waiting);
    view.update(cx, |v, cx| {
        v.model.memos.edits.follow(&elsewhere, &s, Instant::now());
        cx.notify();
    });
    board(cx, w, &view);
    wait(cx, w, &view, 300);
    assert_eq!(state(cx, &view, &ours.body), Some(M::InChat), "its owner's again: it settles");
    assert!(the_draft(cx, &view, "p").is_none(), "and the draft goes");
}
