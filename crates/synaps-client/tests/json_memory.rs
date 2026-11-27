//! What a JSON input is charged with (`synaps_client::blocks::value_bytes`
//! and `kept_bytes`, Shady #101 R7-1) against what it allocates, read on
//! #102's counting allocator ([`heap`]), not on the estimate itself:
//! - every shape is charged at least what it allocates, parsed off the
//!   wire, cloned, and kept as a live call's row keeps it (an `Arc` of the
//!   clone); the demo's tool inputs within 2× of it;
//! - a map's storage is charged as it is allocated, length by length;
//! - a round of object-heavy inputs queued alone at a steer, its log's
//!   store refused, is covered by its charge before and between drains.
//!
//! Under the map backend serde_json has in this build: `IndexMap` built
//! with the desk (`cargo test --workspace`: GPUI turns on serde_json's
//! `preserve_order`), `BTreeMap` with the client alone (`cargo test -p
//! synaps-client`). CI runs both.

use std::sync::Arc;

use serde_json::{Value, json};
use synaps_client::blocks::dispose::{Disposer, MAX_BYTES};
use synaps_client::blocks::{kept_bytes, value_bytes};
use synaps_client::protocol::*;
use synaps_client::transcript::Transcript;

mod heap;
use heap::{charging, held_by, here, mine};

/// serde_json's maps keep their order here (an `IndexMap` each).
fn ordered() -> bool {
    let m: serde_json::Map<String, Value> = serde_json::from_str(r#"{"b":0,"a":0}"#).unwrap();
    m.keys().next().is_some_and(|k| k == "b")
}

fn backend() -> &'static str {
    if ordered() { "IndexMap (preserve_order)" } else { "BTreeMap" }
}

/// What `f` makes and keeps on the heap (bytes: what this thread allocated
/// meanwhile less what it freed), and what it made.
fn made<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let at = here();
    let v = f();
    let n = here() - at;
    (v, usize::try_from(n).expect("nothing of before freed"))
}

/// `n` copies of `one`, as a JSON array.
fn many(one: &str, n: usize) -> String {
    format!("[{}]", vec![one; n].join(","))
}

/// The shapes Shady #101 R7-1 names, as wire bytes.
fn shapes() -> Vec<(String, String)> {
    let deep = |n: usize| format!("{}{{}}{}", r#"{"a":"#.repeat(n), "}".repeat(n));
    let deep_mixed = |n: usize| format!("{}{{}}{}", r#"[{"a":"#.repeat(n), "}]".repeat(n));
    let wide = |n: usize| format!("{{{}}}", (0..n).map(|k| format!(r#""key{k}":{k}"#)).collect::<Vec<_>>().join(","));
    let long = "a long string of text, the kind a note's body carries. ".repeat(20_000);
    let escaped = r#"line one\nline two \"quoted\" \u00e9\u00e8 \t tab "#.repeat(10_000);
    vec![
        ("a singleton object".into(), r#"{"k":0}"#.into()),
        ("1,000 singleton objects".into(), many(r#"{"k":0}"#, 1_000)),
        ("an object of 200 singleton objects".into(), wide(200).replace(':', r#":{"v":"#).replace(',', "},") + "}"),
        ("an empty object".into(), "{}".into()),
        ("1,000 empty objects".into(), many("{}", 1_000)),
        ("100 objects deep".into(), deep(100)),
        ("60 arrays and 60 objects deep".into(), deep_mixed(60)),
        ("a wide object, 15 keys".into(), wide(15)),
        ("a wide object, 1,000 keys".into(), wide(1_000)),
        ("a wide object, 5,000 keys".into(), wide(5_000)),
        ("10,000 small objects".into(), many(r#"{"op":"put","id":"n12","kind":"note"}"#, 10_000)),
        ("1,000 objects of a long string each".into(), many(&format!(r#"{{"text":"{}"}}"#, "x".repeat(1_000)), 1_000)),
        ("a long string (1.1 MB)".into(), format!(r#"{{"text":"{long}"}}"#)),
        ("a long escaped string".into(), format!(r#"{{"text":"{escaped}"}}"#)),
        ("10,000 short strings".into(), many(r#""s""#, 10_000)),
        ("10,000 numbers".into(), many("12.5", 10_000)),
    ]
}

/// The demo session's tool inputs (`synthetic::blocks_history`: every
/// block kind, a card, reads and an edit), as wire bytes.
fn demo_inputs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for m in synaps_client::synthetic::blocks_history() {
        for b in m["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
            out.push((format!("demo {} {}", b["name"].as_str().unwrap(), b["id"].as_str().unwrap()), b["input"].to_string()));
        }
    }
    assert!(out.len() > 10, "the demo's calls: {}", out.len());
    out
}

/// A shape's three readings: (what, measured, charged).
fn readings(wire: &str) -> [(&'static str, usize, usize); 3] {
    let (parsed, p) = made(|| serde_json::from_str::<Value>(wire).unwrap());
    let (cloned, c) = made(|| parsed.clone());
    let (kept, k) = made(|| Arc::new(parsed.clone()));
    [("parsed", p, value_bytes(&parsed)), ("cloned", c, value_bytes(&cloned)), ("kept", k, kept_bytes(&kept))]
}

/// Shady #101 R7-1: every shape is charged at least what it allocates,
/// parsed off the wire, cloned, and kept (an `Arc` of the clone, as a live
/// call's row keeps it): many singleton objects, empty ones, deep, wide,
/// large arrays of small objects, long strings. The demo's tool inputs are
/// charged within 2× of what they allocate. The table is printed.
#[test]
fn every_shape_is_charged_at_least_what_it_allocates() {
    mine();
    let mut wrong = Vec::new();
    let mut table = vec![format!("{} maps (Value: {} bytes)", backend(), std::mem::size_of::<Value>())];
    let demo = demo_inputs();
    for (typical, (name, wire)) in shapes().into_iter().map(|s| (false, s)).chain(demo.into_iter().map(|s| (true, s))) {
        let r = readings(&wire);
        let row: Vec<String> = r.iter().map(|(how, m, e)| format!("{how} {m} → {e} ({:.2}×)", *e as f64 / (*m).max(1) as f64)).collect();
        table.push(format!("{name} ({} wire bytes): {}", wire.len(), row.join(", ")));
        for (how, measured, charged) in r {
            if charged < measured {
                wrong.push(format!("{name}, {how}: charged {charged}, allocates {measured}"));
            }
            if typical && charged > 2 * measured.max(1) {
                wrong.push(format!("{name}, {how}: charged {charged}, over 2× the {measured} it allocates"));
            }
        }
    }
    eprintln!("{}", table.join("\n"));
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A map's storage, its keys' and values' aside, is charged as it is
/// allocated for every length to 300, parsed and cloned: exactly under
/// `IndexMap` (on x86-64: its 16-byte control group; the 8-byte one's
/// tables are charged 8 bytes over), and under `BTreeMap` to 11 (one
/// leaf), at least the allocation past that.
#[test]
fn a_maps_storage_is_charged_as_it_is_allocated() {
    mine();
    let mut wrong = Vec::new();
    for n in 0..=300 {
        let wire = format!("{{{}}}", (0..n).map(|k| format!(r#""{k:03}":null"#)).collect::<Vec<_>>().join(","));
        for (how, measured, charged) in readings(&wire) {
            let exact = cfg!(target_arch = "x86_64") && (ordered() || n <= 11);
            if charged < measured || (exact && charged != measured) {
                wrong.push(format!("{n} entries, {how}: charged {charged}, allocates {measured}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{} maps: {wrong:#?}", backend());
}

fn ev(v: Value) -> SessionEvent {
    decode_event(&v).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn llm(v: Value) -> SessionEvent {
    ev(json!({"ev": "stream", "event": v}))
}

fn attach() -> Transcript {
    let mut a: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/tests/fixtures/attached_history.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    a["conversation"]["api_messages"] = json!([{"role": "user", "content": "notes"}]);
    a["conversation"]["messages_len"] = json!(1);
    a["conversation"]["pending_events_len"] = json!(0);
    a["replay"] = json!([]);
    a["streaming"] = json!(false);
    let DaemonFrame::Attached(a) = decode_frame(&a.to_string()).unwrap() else { panic!() };
    let mut t = Transcript::new();
    t.load_attached(&a).unwrap();
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})));
    t
}

/// A refused block call's input, `n` singleton objects (its tool's name
/// keeps it, whatever the extension makes of it).
fn objects(n: usize) -> Value {
    json!({"ops": (0..n).map(|j| json!({"op": j})).collect::<Vec<_>>()})
}

/// Round `r`'s calls of `input`, whole, then their results, each refused.
fn calls(t: &mut Transcript, r: usize, n: usize, input: &Value) {
    for k in 0..n {
        let id = format!("toolu_{r}x{k}");
        t.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})));
        t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_block", "tool_id": id, "input": input})));
    }
    for k in 0..n {
        t.apply(&llm(
            json!({"kind": "llm", "llm": "tool_result", "tool_id": format!("toolu_{r}x{k}"), "result": "cards_block: refused (a test)"}),
        ));
    }
}

/// Shady #101 R7-1's case, on the allocator: object-heavy inputs (a list
/// of singleton objects each, as a refused block call's can be), round
/// after round, each round retired alone at a steer and queued, below the
/// byte cap; then the tab goes, and its log's store, charged with the
/// same inputs again, is refused and dropped where it goes. The queued
/// rounds are the inputs' last owners. The calls are made on one account
/// (the inputs, the rounds' own vectors and tables), everything else on
/// another (the turn, the steers, the hand-overs, the queue's channel): so
/// once the transcript is gone, the first account holds what the queue
/// holds, read as allocated. Before any drain and after each, that is
/// covered by the queue's charge, and within the accepted bound (the cap:
/// no unit here passes it alone). Drained, the account holds nothing.
#[test]
fn a_round_queued_alone_is_charged_what_its_object_inputs_allocate() {
    let d = Disposer::start();
    let _in = d.install();
    // Made once before any account (anything made the first time a call
    // or a steer is applied is not the queue's).
    {
        let mut t = attach();
        calls(&mut t, 0, 2, &objects(4));
        t.apply(&llm(json!({"kind": "agent", "agent": "steering_delivered", "message": "warm"})));
        drop(t);
        d.flush();
    }
    let other = mine();
    let calls_account = mine();
    let base = held_by(calls_account);
    charging(other);
    let mut t = attach();
    let hold = d.hold();
    let (per, input) = (8, objects(256));
    // Rounds enough that their charge is 60% of the cap: the store,
    // charged with their inputs again, can't fit.
    let rounds = (MAX_BYTES * 6 / 10).div_ceil(per * kept_bytes(&input));
    let mut wrong = Vec::new();
    for r in 0..rounds {
        t.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
        let queued = d.backlog().items;
        charging(calls_account);
        calls(&mut t, r, per, &input);
        charging(other);
        if d.backlog().items != queued {
            wrong.push(format!("round {r}: handed over while its calls were made (that account must be the queue's alone)"));
        }
        t.apply(&llm(json!({"kind": "agent", "agent": "steering_delivered", "message": "more"})));
        if d.backlog().items != queued + 1 {
            wrong.push(format!("round {r}: not queued at its steer ({:?})", d.backlog()));
        }
    }
    let before = d.backlog();
    drop(t);
    let after = d.backlog();
    if after.in_place == before.in_place {
        wrong.push(format!("the log's store was not refused: {before:?} → {after:?}"));
    }
    let mut drains = 0;
    loop {
        let (alive, b) = (held_by(calls_account) - base, d.backlog());
        let alive = usize::try_from(alive).expect("an account holds no less than nothing");
        if alive > b.bytes {
            wrong.push(format!(
                "{} maps, after {drains} drains: the queue holds {alive} bytes, is charged {} ({} items)",
                backend(),
                b.bytes,
                b.items
            ));
        }
        if b.bytes > MAX_BYTES {
            wrong.push(format!("after {drains} drains: {} bytes queued, past the cap", b.bytes));
        }
        if b.items == 0 {
            break;
        }
        hold.step(1);
        drains += 1;
    }
    eprintln!("{} maps: {rounds} rounds of {per} calls, {} bytes charged at the start", backend(), after.bytes);
    drop(hold);
    d.flush();
    let left = held_by(calls_account) - base;
    if left != 0 || d.backlog().bytes != 0 {
        wrong.push(format!("drained: the calls' account holds {left} bytes, the queue {:?}", d.backlog()));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
