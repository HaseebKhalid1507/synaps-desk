//! Agent cards on the wire (agent-ui D1): SynapsCLI #444's shapes, read
//! as its own tests read them (`session/wire_surfaces_tests.rs` at
//! `6e895d27`), by this client's decoders. Nothing here is sent to this
//! build by a daemon (it declares `surfaces: 0`); the decoders are ready
//! for when D3 declares 1, and never fail a frame meanwhile.

use serde_json::{Value, json};
use synaps_client::protocol::*;
use synaps_client::surface::{AnswerStatus, Phase, SurfaceState, Verb, WaitMode};

/// #444's `wire::test_card`: A1 answered, `revise` offered.
fn card() -> Value {
    json!({
        "id": "s_3f2a91c0d4e2a871", "call_id": "toolu_01", "rev": 3,
        "contract": "c1:dd88bb46e6b36d702d326c325b6b5c61", "catalog": 1, "mode": "live", "phase": "answered",
        "offers": ["revise"],
        "doc": {"question": "Which approach?", "options": [{"label": "Refactor the parser"}, {"label": "Leave it"}], "multi": false},
        "origin": {"turn": "3f2a91c0-d4e2-4a87-9b1f-0123456789ab", "task": "fix the parser"},
        "epoch": 4, "resume": false, "actions": ["5b0c".repeat(8)], "raised_at": "2026-11-23T05:42:00Z",
        "answer": {
            "rev": 1, "action_id": "5b0c".repeat(8), "answer_id": "a_0123456789abcdef", "status": "answered",
            "values": {"choice": "Refactor the parser"}, "delivery": "in_context", "wake": true, "by": "desk-1",
            "at": "2026-11-23T05:43:00Z",
        },
    })
}

fn state(v: Value) -> SurfaceState {
    serde_json::from_value(v).unwrap()
}

fn event(ev: Value) -> SessionEvent {
    decode_event(&ev).unwrap_or_else(|e| panic!("{e}: {ev}"))
}

fn attached_with(surfaces: Option<Value>) -> Attached {
    let mut v: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/attached_create.json")).unwrap(),
    )
    .unwrap();
    if let Some(s) = surfaces {
        v["surfaces"] = s;
    }
    let DaemonFrame::Attached(a) = decode_frame(&v.to_string()).unwrap() else { panic!("attached") };
    *a
}

#[test]
fn a_surface_event_decodes() {
    let SessionEvent::Surface { surface: Some(s) } = event(json!({"ev": "surface", "surface": card()})) else { panic!() };
    assert_eq!(*s, state(card()));
    assert_eq!((s.mode, s.phase, s.offers.as_slice(), s.rev, s.epoch), (WaitMode::Live, Phase::Answered, &[Verb::Revise][..], 3, 4));
    let a = s.answer.as_ref().unwrap();
    assert_eq!((a.rev, a.status, a.values.to_json()), (1, AnswerStatus::Answered, String::from(r#"{"choice":"Refactor the parser"}"#)));
    assert_eq!(s.doc.question, "Which approach?");
}

#[test]
fn every_surface_result_decodes() {
    let outcomes = [
        ("accepted", SurfaceOutcome::Accepted),
        ("pending", SurfaceOutcome::Pending),
        ("duplicate", SurfaceOutcome::Duplicate),
        ("refused", SurfaceOutcome::Refused),
    ];
    let reasons = [
        ("rate_limited", SurfaceRefusal::RateLimited),
        ("forbidden", SurfaceRefusal::Forbidden),
        ("reloading", SurfaceRefusal::Reloading),
        ("invalid", SurfaceRefusal::Invalid),
        ("busy", SurfaceRefusal::Busy),
        ("storage", SurfaceRefusal::Storage),
        ("resolved", SurfaceRefusal::Resolved),
        ("gone", SurfaceRefusal::Gone),
        ("stale", SurfaceRefusal::Stale),
        ("stopped", SurfaceRefusal::Stopped),
        ("conflict", SurfaceRefusal::Conflict),
        ("unavailable", SurfaceRefusal::Unavailable),
    ];
    let details = [
        ("context_blocked", SurfaceUnavailable::ContextBlocked),
        ("queue_full", SurfaceUnavailable::QueueFull),
        ("ledger_full", SurfaceUnavailable::LedgerFull),
        ("awaiting_owner", SurfaceUnavailable::AwaitingOwner),
    ];
    let result = |v: Value| match event(v.clone()) {
        SessionEvent::SurfaceResult(r) => r,
        other => panic!("{v}: {other:?}"),
    };
    for (s, o) in outcomes {
        let r = result(json!({"ev": "surface_result", "client": 3, "action_id": "5b0c".repeat(8), "outcome": s, "answer_rev": 1}));
        assert_eq!(
            r,
            SurfaceResult {
                client: 3,
                action_id: "5b0c".repeat(8),
                outcome: o,
                reason: None,
                retry: None,
                answer_rev: Some(1),
                detail: None
            }
        );
        assert_eq!(serde_json::to_value(&r).unwrap()["outcome"], s);
    }
    for (s, why) in reasons {
        let r = result(json!({"ev": "surface_result", "client": 3, "action_id": "x", "outcome": "refused", "reason": s, "retry": true}));
        assert_eq!((r.outcome, r.reason, r.retry, r.detail), (SurfaceOutcome::Refused, Some(why), Some(true), None), "{s}");
    }
    // v3.4 #2: every detail, absent, and one this build doesn't know.
    for (s, d) in details.into_iter().chain([("from_the_future", SurfaceUnavailable::Unknown)]) {
        let r = result(
            json!({"ev": "surface_result", "client": 3, "action_id": "x", "outcome": "refused", "reason": "unavailable", "retry": true, "detail": s}),
        );
        assert_eq!((r.reason, r.detail), (Some(SurfaceRefusal::Unavailable), Some(d)), "{s}");
    }
    let r = result(json!({"ev": "surface_result", "client": 3, "action_id": "x", "outcome": "refused", "reason": "unavailable"}));
    assert_eq!(r.detail, None);
    assert!(serde_json::to_value(&r).unwrap().get("detail").is_none(), "omitted when absent");
}

/// #444's `a_card_this_build_cannot_read_never_fails_the_frame`, here: a
/// card a newer daemon shaped differently (an unknown phase, an extra
/// field) never fails a frame. In `Attached` it is dropped, counted in
/// `Rejected`, and the rest stay; as an event it is `surface: None`. A verb
/// this build doesn't know leaves `offers`, and the card stays (v3.4 #4).
/// A `surface_result` with an outcome or reason this build doesn't know
/// reads them as `Unknown`. A doc that doesn't revalidate is dropped too:
/// the desk's own rule (a renderer trusts the doc's bounds).
#[test]
fn a_card_this_build_cannot_read_never_fails_the_frame() {
    let c = card();
    let mut phase = c.clone();
    phase["phase"] = json!("from_the_future");
    let mut field = c.clone();
    field["from_the_future"] = json!(1);
    let mut verb = c.clone();
    verb["offers"] = json!(["revise", "forget"]);
    let mut verbs = c.clone();
    verbs["offers"] = json!(["forget", 7]);
    let mut no_verbs = c.clone();
    no_verbs["offers"] = json!([]);
    let mut unnormalized = c.clone();
    // Decodes, but normalizing drops an empty hint: not the daemon's doc.
    unnormalized["doc"]["options"][1]["hint"] = json!("");
    let mut over_bounds = c.clone();
    over_bounds["doc"]["options"] = json!((0..10).map(|i| json!({"label": format!("Option {i}")})).collect::<Vec<_>>());
    let mut hostile = c.clone();
    hostile["doc"]["question"] = json!("Which \u{202E}approach?");

    for bad in [&phase, &field, &unnormalized, &over_bounds, &hostile] {
        match event(json!({"ev": "surface", "surface": bad})) {
            SessionEvent::Surface { surface: None } => {}
            other => panic!("{bad}: {other:?}"),
        }
    }
    for (newer, kept) in [(&verb, &c), (&verbs, &no_verbs)] {
        match event(json!({"ev": "surface", "surface": newer})) {
            SessionEvent::Surface { surface: Some(s) } => assert_eq!(*s, state(kept.clone()), "{newer}"),
            other => panic!("{newer}: {other:?}"),
        }
    }

    let a = attached_with(Some(json!([c, phase, field, verb, verbs, 7, unnormalized, over_bounds, hostile])));
    assert_eq!(a.surfaces, vec![state(c.clone()), state(c.clone()), state(no_verbs)]);
    assert_eq!(a.surfaces[1].offers, [Verb::Revise]);
    assert_eq!(a.rejected.surfaces, 6, "phase, field, 7, and three docs that don't revalidate");
    assert_eq!(a.rejected.total(), 6);
    assert!(a.rejected.to_string().contains("6 surfaces"), "{}", a.rejected);
    let a = attached_with(Some(json!({"not": "a list"})));
    assert!(a.surfaces.is_empty());
    assert_eq!(a.rejected.surfaces, 1, "not a list: the frame decodes, the oddity is counted");
    let a = attached_with(Some(Value::Null));
    assert_eq!((a.surfaces.len(), a.rejected.total()), (0, 0));

    let newer = json!({"ev": "surface_result", "client": 1, "action_id": "x", "outcome": "deferred",
                       "reason": "cosmic_rays", "from_the_future": true});
    match event(newer) {
        SessionEvent::SurfaceResult(r) => assert_eq!((r.outcome, r.reason), (SurfaceOutcome::Unknown, Some(SurfaceRefusal::Unknown))),
        other => panic!("{other:?}"),
    }
}

/// What every daemon sends today: no `surfaces` key, no presence counts.
#[test]
fn an_attach_without_cards_is_as_before() {
    let a = attached_with(None);
    assert!(a.surfaces.is_empty());
    assert_eq!(a.rejected.total(), 0);
    assert_eq!((a.meta.surfaces_waiting, a.meta.surfaces_open), (0, 0));
}

/// The capability is declared 0, so the `Hello` is byte for byte the one
/// before D1 (`surfaces` omitted); the presence counts read when sent.
#[test]
fn the_capability_and_the_presence_counts_are_additive() {
    assert_eq!(SURFACES_CATALOG, 0, "D1 is inert: D3 declares 1");
    let hello = serde_json::to_value(synaps_client::Identity::new("desk").unwrap().hello(None)).unwrap();
    let caps = hello.pointer("/client/capabilities").unwrap_or_else(|| panic!("{hello}"));
    assert_eq!(*caps, json!({"tool_queue": true, "owner_reason_input": true}), "no `surfaces` key");
    let meta: SessionMeta = serde_json::from_value(json!({"id": "a", "surfaces_waiting": 1, "surfaces_open": 2})).unwrap();
    assert_eq!((meta.surfaces_waiting, meta.surfaces_open, meta.awaiting_input), (1, 2, 0));
}
