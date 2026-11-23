//! Every transition of §3.8's outbox (and v3.4 #7's cap), one test per row
//! of its table, plus §6.1.6's invariant over every step and input.

use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::surface::Verb;

const CARD: &str = "s_3f2a91c0d4e2a871";
const CONTRACT: &str = "c1:dd88bb46e6b36d702d326c325b6b5c61";
const OURS: &str = "5b0c5b0c5b0c5b0c5b0c5b0c5b0c5b0c";
const THEIRS: &str = "9e9e9e9e9e9e9e9e9e9e9e9e9e9e9e9e";
const CONN: u64 = 7;

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn action(verb: Verb, expect_rev: u32) -> SurfaceAction {
    SurfaceAction {
        surface_id: CARD.into(),
        contract: CONTRACT.into(),
        action_id: OURS.into(),
        expect_rev,
        epoch: 4,
        verb,
        values: (verb != Verb::Skip).then(|| json!({"choice": "Refactor the parser"})),
    }
}

/// A1's card: open (`answer: None`) offering answer and skip, or answered
/// at `rev` by `by_action` offering revise.
fn card(answered: Option<(u32, &str)>) -> SurfaceState {
    let mut v = json!({
        "id": CARD, "call_id": "toolu_01", "rev": 3, "contract": CONTRACT, "catalog": 1,
        "mode": "live", "phase": "open", "offers": ["answer", "skip"],
        "doc": {"question": "Which approach?", "options": [{"label": "Refactor the parser"}, {"label": "Leave it"}], "multi": false},
        "origin": {"turn": null, "task": "fix the parser"}, "epoch": 4, "resume": false, "actions": [],
        "raised_at": "2026-11-23T05:42:00Z",
    });
    if let Some((rev, by)) = answered {
        v["phase"] = json!("answered");
        v["offers"] = json!(["revise"]);
        v["actions"] = json!([by]);
        v["answer"] = json!({
            "rev": rev, "action_id": by, "answer_id": "a_0123456789abcdef", "status": if rev == 1 { "answered" } else { "revised" },
            "values": {"choice": "Leave it"}, "delivery": "in_context", "wake": true, "by": "desk-2", "at": "2026-11-23T05:43:00Z",
        });
    }
    crate::surface::read_card(v).expect("a card this build reads")
}

fn result(outcome: SurfaceOutcome, reason: Option<SurfaceRefusal>) -> SurfaceResult {
    SurfaceResult { client: 1, action_id: OURS.into(), outcome, reason, retry: None, answer_rev: None, detail: None }
}

fn refused(reason: SurfaceRefusal) -> SurfaceResult {
    result(SurfaceOutcome::Refused, Some(reason))
}

fn sent(t: Instant) -> Entry {
    Entry::commit(action(Verb::Answer, 0), CONN, t)
}

fn at(step: Step) -> Entry {
    Entry { action: action(Verb::Answer, 0), step }
}

/// One of every step, for the rows that hold whatever the step.
fn every_step(t: Instant) -> Vec<Step> {
    vec![
        Step::Sent { connection: CONN, sends: 1, at: t, backoff: None },
        Step::Sent { connection: CONN, sends: 3, at: t, backoff: Some(s(4)) },
        Step::Pending { connection: CONN, since: t },
        Step::Backoff { connection: CONN, due: t + s(2), delay: s(2), reason: SurfaceRefusal::Busy },
        Step::Held(NotSent::Storage),
        Step::Held(NotSent::Unavailable(Some(SurfaceUnavailable::QueueFull))),
        Step::Held(NotSent::Unanswered),
        Step::Held(NotSent::Unknown),
        Step::Detached,
    ]
}

const EVERY_REFUSAL: [SurfaceRefusal; 13] = [
    SurfaceRefusal::RateLimited,
    SurfaceRefusal::Forbidden,
    SurfaceRefusal::Reloading,
    SurfaceRefusal::Invalid,
    SurfaceRefusal::Busy,
    SurfaceRefusal::Storage,
    SurfaceRefusal::Resolved,
    SurfaceRefusal::Gone,
    SurfaceRefusal::Stale,
    SurfaceRefusal::Stopped,
    SurfaceRefusal::Conflict,
    SurfaceRefusal::Unavailable,
    SurfaceRefusal::Unknown,
];

/// Row 1: `accepted` / `duplicate` (whatever the step), or a state whose
/// `actions` hold ours: done. Another action's result or another card's
/// state is nothing to this entry.
#[test]
fn accepted_duplicate_or_a_state_listing_ours_is_done() {
    let t = Instant::now();
    for step in every_step(t) {
        for outcome in [SurfaceOutcome::Accepted, SurfaceOutcome::Duplicate] {
            let r = SurfaceResult { answer_rev: Some(1), ..result(outcome, None) };
            assert_eq!(at(step).on_result(&r, t), Next::Done, "{step:?} {outcome:?}");
            let other = SurfaceResult { action_id: THEIRS.into(), ..r };
            assert_eq!(at(step).on_result(&other, t), Next::Stay(step), "{step:?}: another action's");
        }
        // The latest answer is ours, or ours is an earlier one (§3.7: found
        // in any later snapshot, not only while it is the latest).
        let mut earlier = card(Some((2, THEIRS)));
        earlier.actions = vec![OURS.into(), THEIRS.into()];
        for c in [card(Some((1, OURS))), earlier] {
            assert_eq!(at(step).on_state(&c), Next::Done, "{step:?}");
        }
        let mut elsewhere = card(Some((1, OURS)));
        elsewhere.id = "s_0000000000000001".into();
        assert_eq!(at(step).on_state(&elsewhere), Next::Stay(step), "{step:?}: another card");
        assert_eq!(at(step).on_state(&card(Some((1, THEIRS)))), Next::Stay(step), "{step:?}: resolved by another client");
        assert_eq!(at(step).on_state(&card(None)), Next::Stay(step), "{step:?}: still open");
    }
}

/// Row 2: `pending` keeps the entry, shows "saving…" after 2 s, and is
/// never re-sent on its connection; the final result settles it.
#[test]
fn pending_waits_for_its_final_result_and_is_not_resent() {
    let t = Instant::now();
    let pending = sent(t).on_result(&result(SurfaceOutcome::Pending, None), t + s(1));
    assert_eq!(pending, Next::Stay(Step::Pending { connection: CONN, since: t + s(1) }));
    let e = sent(t).apply(pending).unwrap();
    assert!(!e.saving(t + s(1) + Duration::from_millis(1999)));
    assert!(e.saving(t + s(3)));
    assert_eq!(e.due(t + s(1)), Some(t + s(3)), "one wake, for \"saving…\"");
    assert_eq!(e.due(t + s(3)), None, "then nothing until the result");
    for later in [s(6), s(16), s(600)] {
        assert_eq!(e.tick(t + later), Next::Stay(e.step), "no re-send at +{later:?}");
    }
    assert_eq!(e.on_result(&result(SurfaceOutcome::Accepted, None), t + s(9)), Next::Done);
    assert_eq!(e.on_result(&refused(SurfaceRefusal::Storage), t + s(9)), Next::Stay(Step::Held(NotSent::Storage)));
}

/// Row 3: `reloading`, `rate_limited`, `busy` re-send the same id after
/// 1, 2, 4 … 30 s, on the same connection.
#[test]
fn retryable_refusals_back_off_1_2_4_to_30_seconds() {
    for reason in [SurfaceRefusal::Reloading, SurfaceRefusal::RateLimited, SurfaceRefusal::Busy] {
        let mut now = Instant::now();
        let mut e = sent(now);
        let mut delays = Vec::new();
        for _ in 0..8 {
            let next = e.on_result(&refused(reason), now);
            let Next::Stay(Step::Backoff { connection, due, delay, reason: r }) = next else { panic!("{reason:?}: {next:?}") };
            assert_eq!((connection, due, r), (CONN, now + delay, reason));
            delays.push(delay.as_secs());
            e = e.apply(next).unwrap();
            assert_eq!(e.due(now), Some(due));
            assert_eq!(e.tick(due - Duration::from_millis(1)), Next::Stay(e.step), "not before it is due");
            now = due;
            let resend = e.tick(now);
            assert_eq!(resend, Next::Send(Step::Sent { connection: CONN, sends: 1, at: now, backoff: Some(delay) }), "{reason:?}");
            e = e.apply(resend).unwrap();
            assert_eq!(e.action.action_id, OURS, "the same id");
        }
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30], "{reason:?}");
        // A new connection starts the schedule again.
        let fresh = e.on_snapshot(Some(&card(None)), CONN + 1, now);
        let e = e.apply(fresh).unwrap();
        assert_eq!(
            e.on_result(&refused(reason), now),
            Next::Stay(Step::Backoff { connection: CONN + 1, due: now + s(1), delay: s(1), reason })
        );
    }
}

/// Row 4: `storage` and `unavailable` (each `detail`, or none) hold for
/// the user's Retry, which re-sends the same id; nothing else re-sends it.
#[test]
fn storage_and_unavailable_wait_for_retry() {
    let t = Instant::now();
    let details = [
        Some(SurfaceUnavailable::ContextBlocked),
        Some(SurfaceUnavailable::QueueFull),
        Some(SurfaceUnavailable::LedgerFull),
        Some(SurfaceUnavailable::AwaitingOwner),
        Some(SurfaceUnavailable::Unknown),
        None,
    ];
    let mut cases: Vec<(SurfaceResult, NotSent)> = vec![(refused(SurfaceRefusal::Storage), NotSent::Storage)];
    for d in details {
        cases.push((SurfaceResult { detail: d, ..refused(SurfaceRefusal::Unavailable) }, NotSent::Unavailable(d)));
    }
    // A `detail` is meaningful only with `unavailable` (v3.4 #2).
    let stray = SurfaceResult { detail: Some(SurfaceUnavailable::QueueFull), ..refused(SurfaceRefusal::Storage) };
    cases.push((stray, NotSent::Storage));
    for (r, why) in cases {
        let next = sent(t).on_result(&r, t);
        assert_eq!(next, Next::Stay(Step::Held(why)), "{r:?}");
        let e = sent(t).apply(next).unwrap();
        assert_eq!(e.due(t), None);
        assert_eq!(e.tick(t + s(3600)), Next::Stay(e.step), "held: never re-sent by the clock");
        assert_eq!(e.retry(CONN + 1, t + s(9)), Next::Send(Step::Sent { connection: CONN + 1, sends: 1, at: t + s(9), backoff: None }));
    }
    // Retry has nothing to do on an action that isn't held.
    for step in every_step(t).into_iter().filter(|s| !matches!(s, Step::Held(_))) {
        assert_eq!(at(step).retry(CONN, t), Next::Stay(step), "{step:?}");
    }
}

/// Row 5: `conflict`, `resolved`, `stale`, `stopped`, `gone` (and the
/// final `forbidden`, `invalid`) move the values to a recovered draft.
#[test]
fn refusals_that_cannot_be_sent_recover_the_values() {
    let t = Instant::now();
    for reason in [
        SurfaceRefusal::Conflict,
        SurfaceRefusal::Resolved,
        SurfaceRefusal::Stale,
        SurfaceRefusal::Stopped,
        SurfaceRefusal::Gone,
        SurfaceRefusal::Forbidden,
        SurfaceRefusal::Invalid,
    ] {
        for step in every_step(t) {
            assert_eq!(at(step).on_result(&refused(reason), t), Next::Recover(reason), "{step:?} {reason:?}");
        }
    }
}

/// A newer daemon's reason holds for the user; a newer daemon's outcome is
/// as if no reply came (the reply timer still runs).
#[test]
fn a_newer_daemons_reason_holds_and_its_outcome_is_no_reply() {
    let t = Instant::now();
    for r in [refused(SurfaceRefusal::Unknown), result(SurfaceOutcome::Refused, None)] {
        assert_eq!(sent(t).on_result(&r, t), Next::Stay(Step::Held(NotSent::Unknown)), "{r:?}");
    }
    let e = sent(t);
    assert_eq!(e.on_result(&result(SurfaceOutcome::Unknown, None), t + s(1)), Next::Stay(e.step));
    assert!(matches!(e.tick(t + s(5)), Next::Send(Step::Sent { sends: 2, .. })));
    // As it reads off the wire.
    let wire: SurfaceResult =
        serde_json::from_value(json!({"client": 1, "action_id": OURS, "outcome": "deferred", "reason": "cosmic_rays"})).unwrap();
    assert_eq!((wire.outcome, wire.reason), (SurfaceOutcome::Unknown, Some(SurfaceRefusal::Unknown)));
}

/// Row 6 and v3.4 #7: no reply in 5 s re-sends the same id; the third
/// send unanswered holds it ("Not sent · Retry"). Any reply starts the
/// count again.
#[test]
fn no_reply_resends_until_three_sends_then_holds() {
    let t = Instant::now();
    let mut e = sent(t);
    assert_eq!(e.due(t), Some(t + s(5)));
    assert_eq!(e.tick(t + Duration::from_millis(4999)), Next::Stay(e.step));
    let second = e.tick(t + s(5));
    assert_eq!(second, Next::Send(Step::Sent { connection: CONN, sends: 2, at: t + s(5), backoff: None }));
    e = e.apply(second).unwrap();
    let third = e.tick(t + s(10));
    assert_eq!(third, Next::Send(Step::Sent { connection: CONN, sends: 3, at: t + s(10), backoff: None }));
    e = e.apply(third).unwrap();
    assert_eq!(e.tick(t + s(15)), Next::Stay(Step::Held(NotSent::Unanswered)));
    // A late tick (the timer fired late) still counts one send, not three.
    assert_eq!(sent(t).tick(t + s(60)), Next::Send(Step::Sent { connection: CONN, sends: 2, at: t + s(60), backoff: None }));
    // A reply (here `busy`) resets the count: its re-send is a first again.
    let mut e = sent(t).apply(sent(t).tick(t + s(5))).unwrap();
    let busy = e.on_result(&refused(SurfaceRefusal::Busy), t + s(6));
    e = e.apply(busy).unwrap();
    let resend = e.tick(t + s(7));
    assert!(matches!(resend, Next::Send(Step::Sent { sends: 1, .. })), "{resend:?}");
    // The user's Retry after "Not sent" is a first send again.
    let held = at(Step::Held(NotSent::Unanswered));
    let retry = held.retry(CONN, t + s(20));
    let retried = held.apply(retry).unwrap();
    assert_eq!(retried.tick(t + s(25)), Next::Send(Step::Sent { connection: CONN, sends: 2, at: t + s(25), backoff: None }));
}

/// A lost connection sends nothing until a new one's snapshot reconciles
/// the entry; a held one keeps its Retry. Replies that still arrive for
/// it settle it only when they are final.
#[test]
fn a_lost_connection_waits_for_the_snapshot() {
    let t = Instant::now();
    for step in every_step(t) {
        let want = if matches!(step, Step::Held(_)) { step } else { Step::Detached };
        assert_eq!(at(step).on_disconnect(), Next::Stay(want), "{step:?}");
    }
    let e = at(Step::Detached);
    assert_eq!(e.due(t), None);
    assert_eq!(e.tick(t + s(600)), Next::Stay(Step::Detached));
    assert_eq!(e.connection(), None);
    assert_eq!(e.on_result(&refused(SurfaceRefusal::Busy), t), Next::Stay(Step::Detached), "no backoff without a connection");
    assert_eq!(e.on_result(&result(SurfaceOutcome::Pending, None), t), Next::Stay(Step::Detached));
    assert_eq!(e.on_result(&result(SurfaceOutcome::Accepted, None), t), Next::Done);
    assert_eq!(e.on_result(&refused(SurfaceRefusal::Stale), t), Next::Recover(SurfaceRefusal::Stale));
}

/// Row 7, reattach: the snapshot holds ours → done; the card still takes
/// the action unchanged → re-sent at once on the new connection (F16),
/// whatever the step; otherwise → a recovered draft, with the refusal a
/// re-send would get, in admission's order (§3.3 #6–#10; F16b: `stopped`).
#[test]
fn reattach_reconciles_every_entry_against_the_snapshot() {
    let t = Instant::now();
    let new = CONN + 1;
    for step in every_step(t) {
        let e = at(step);
        assert_eq!(e.on_snapshot(Some(&card(Some((1, OURS)))), new, t), Next::Done, "{step:?}");
        let resend = Next::Send(Step::Sent { connection: new, sends: 1, at: t, backoff: None });
        assert_eq!(e.on_snapshot(Some(&card(None)), new, t), resend, "{step:?}: unchanged, re-sent (F16)");
        assert_eq!(e.on_snapshot(None, new, t), Next::Recover(SurfaceRefusal::Gone), "{step:?}");
        assert_eq!(e.on_snapshot(Some(&card(Some((1, THEIRS)))), new, t), Next::Recover(SurfaceRefusal::Resolved), "{step:?}");
        let mut stale = card(None);
        stale.contract = "c1:00000000000000000000000000000000".into();
        assert_eq!(e.on_snapshot(Some(&stale), new, t), Next::Recover(SurfaceRefusal::Stale), "{step:?}");
        let mut stopped = card(None);
        stopped.epoch = 5;
        assert_eq!(e.on_snapshot(Some(&stopped), new, t), Next::Recover(SurfaceRefusal::Stopped), "{step:?}: F16b");
        // Two changes: the one admission checks first.
        stopped.contract = stale.contract.clone();
        assert_eq!(e.on_snapshot(Some(&stopped), new, t), Next::Recover(SurfaceRefusal::Stale), "{step:?}");
    }
    // A revision: re-sent while the answer is still the one it changes;
    // another client's revision since is a conflict (§3.7).
    let revise = Entry { action: action(Verb::Revise, 1), step: Step::Detached };
    assert_eq!(
        revise.on_snapshot(Some(&card(Some((1, THEIRS)))), new, t),
        Next::Send(Step::Sent { connection: new, sends: 1, at: t, backoff: None })
    );
    assert_eq!(revise.on_snapshot(Some(&card(Some((2, THEIRS)))), new, t), Next::Recover(SurfaceRefusal::Conflict));
    assert_eq!(revise.on_snapshot(Some(&card(None)), new, t), Next::Recover(SurfaceRefusal::Resolved), "revise not offered");
    // A skip, like an answer.
    let skip = Entry { action: action(Verb::Skip, 0), step: Step::Held(NotSent::Unanswered) };
    assert!(matches!(skip.on_snapshot(Some(&card(None)), new, t), Next::Send(_)));
}

/// §6.1.6: a committed value is never dropped by an event. Over every
/// step and every input, an entry leaves only as `Done` (the daemon has
/// it: accepted, duplicate, or a state that lists it) or `Recover` (its
/// values go to a draft); everything else keeps it.
#[test]
fn a_committed_value_is_never_dropped_by_an_event() {
    let t = Instant::now();
    let others = [card(None), card(Some((1, THEIRS))), card(Some((2, THEIRS)))];
    for step in every_step(t) {
        let e = at(step);
        for reason in EVERY_REFUSAL {
            for detail in [None, Some(SurfaceUnavailable::QueueFull)] {
                let r = SurfaceResult { detail, ..refused(reason) };
                assert_ne!(e.on_result(&r, t), Next::Done, "{step:?} {reason:?}");
            }
        }
        for outcome in [SurfaceOutcome::Pending, SurfaceOutcome::Unknown] {
            assert!(matches!(e.on_result(&result(outcome, None), t), Next::Stay(_)), "{step:?} {outcome:?}");
        }
        for c in &others {
            assert!(matches!(e.on_state(c), Next::Stay(_)), "{step:?}");
            assert_ne!(e.on_snapshot(Some(c), CONN + 1, t), Next::Done, "{step:?}");
        }
        for later in [s(0), s(5), s(60), s(3600)] {
            assert!(matches!(e.tick(t + later), Next::Stay(_) | Next::Send(_)), "{step:?}");
        }
        assert!(matches!(e.on_disconnect(), Next::Stay(_)));
        assert!(matches!(e.retry(CONN, t), Next::Stay(_) | Next::Send(_)));
        // Whatever is sent is the action as committed.
        for next in [e.tick(t + s(60)), e.retry(CONN, t), e.on_snapshot(Some(&card(None)), CONN + 1, t)] {
            if let Next::Send(step) = next {
                assert!(matches!(step, Step::Sent { .. }), "{next:?}");
                assert_eq!(e.clone().apply(next).unwrap().action, action(Verb::Answer, 0));
            }
        }
    }
}

/// The action as it goes on the wire (§2.4), its `Debug` without the
/// values, and its id.
#[test]
fn the_action_on_the_wire() {
    let a = action(Verb::Answer, 0);
    let cmd = crate::protocol::SessionCommand::SurfaceAction(a.clone());
    assert_eq!(
        serde_json::to_value(&cmd).unwrap(),
        json!({"cmd": "surface_action", "surface_id": CARD, "contract": CONTRACT, "action_id": OURS,
               "expect_rev": 0, "epoch": 4, "verb": "answer", "values": {"choice": "Refactor the parser"}})
    );
    let skip = serde_json::to_value(crate::protocol::SessionCommand::SurfaceAction(action(Verb::Skip, 0))).unwrap();
    assert!(skip.get("values").is_none(), "a skip carries no values: {skip}");
    for shown in [format!("{a:?}"), format!("{cmd:?}"), format!("{:?}", sent(Instant::now()))] {
        assert!(!shown.contains("Refactor"), "{shown}");
        assert!(shown.contains("<redacted>") && shown.contains(OURS), "{shown}");
    }
    let (x, y) = (SurfaceAction::mint_id(), SurfaceAction::mint_id());
    for id in [&x, &y] {
        assert!(id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{id}");
    }
    assert_ne!(x, y);
}
