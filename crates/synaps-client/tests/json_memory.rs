//! What a JSON input is charged with (`synaps_client::blocks::value_bytes`
//! and `kept_bytes`) against what keeping it allocates, read on #102's
//! counting allocator ([`heap`]), not on the estimate itself. An input is
//! kept as `canonical` rebuilds it (Shady #101 R8-1), whatever made it:
//! - every shape, kept, its clone, and a row's `Arc` of it, is charged
//!   exactly what it allocates (Shady #101 R7-1's shapes, the demo's
//!   inputs);
//! - so is a map of every length to 300, from a parse with and without a
//!   duplicate key: a layout drift in either backend fails here;
//! - duplicate keys at each index table's capacity (3, 7, 14, 28, 56), off
//!   the wire, cloned and nested, and a key set again after a clone, which
//!   grow a map's table but not its length (R8-1), are charged as kept;
//! - so is a raw attach of such inputs, through `decode_frame`, as the
//!   history's rows and calls retain it;
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
use synaps_client::blocks::{canonical, kept_bytes, value_bytes};
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

/// The charge is the allocation exactly where the locked layouts are the
/// ones charged (x86-64's 16-byte control group); at least it elsewhere.
const EXACT: bool = cfg!(target_arch = "x86_64");

/// What `f` makes and keeps on the heap (bytes: what this thread allocated
/// meanwhile less what it freed), and what it made.
fn made<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let at = here();
    let v = f();
    let n = here() - at;
    (v, usize::try_from(n).expect("nothing of before freed"))
}

/// `v` as an input is kept: a history call's, a row's.
fn keep(v: &Value) -> Value {
    canonical(v)
}

/// What keeping `source` makes, three ways (how, allocated, charged): as
/// a history's call keeps it, a clone of that (as a journal copies it),
/// and as a row keeps it (an `Arc` of it).
fn readings(source: &Value) -> [(&'static str, usize, usize); 3] {
    let (kept, k) = made(|| keep(source));
    let (cloned, c) = made(|| kept.clone());
    let (row, r) = made(|| Arc::new(keep(source)));
    [("kept", k, value_bytes(&kept)), ("its clone", c, value_bytes(&cloned)), ("a row's", r, kept_bytes(&row))]
}

/// `source`'s readings checked: each charged what it allocates ([`EXACT`]).
fn check(name: &str, source: &Value, wrong: &mut Vec<String>) -> [(&'static str, usize, usize); 3] {
    let r = readings(source);
    for (how, measured, charged) in r {
        if charged < measured || (EXACT && charged != measured) {
            wrong.push(format!("{name}, {how}: charged {charged}, allocates {measured}"));
        }
    }
    r
}

fn parse(wire: &str) -> Value {
    serde_json::from_str(wire).unwrap()
}

/// `n` copies of `one`, as a JSON array.
fn many(one: &str, n: usize) -> String {
    format!("[{}]", vec![one; n].join(","))
}

/// An object of keys `k000`… (`n` of them, null each), then its first
/// `again` keys again, as wire bytes: what parsing makes of them is the
/// same map (the last value wins), its table grown by the first key again
/// if it was full.
fn object(n: usize, again: usize) -> String {
    let keys: Vec<String> = (0..n).chain(0..again).map(|k| format!(r#""k{k:03}":null"#)).collect();
    format!("{{{}}}", keys.join(","))
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

/// Shady #101 R7-1: every shape is charged what keeping it allocates,
/// parsed off the wire, then kept, its clone, and a row's `Arc` of it:
/// many singleton objects, empty ones, deep, wide, large arrays of small
/// objects, long strings; the demo's tool inputs. The table is printed.
#[test]
fn every_shape_is_charged_at_least_what_it_allocates() {
    mine();
    let mut wrong = Vec::new();
    let mut table = vec![format!("{} maps (Value: {} bytes)", backend(), std::mem::size_of::<Value>())];
    for (name, wire) in shapes().into_iter().chain(demo_inputs()) {
        let r = check(&name, &parse(&wire), &mut wrong);
        let row: Vec<String> = r.iter().map(|(how, m, e)| format!("{how} {m} → {e} ({:.2}×)", *e as f64 / (*m).max(1) as f64)).collect();
        table.push(format!("{name} ({} wire bytes): {}", wire.len(), row.join(", ")));
    }
    eprintln!("{}", table.join("\n"));
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A map of every length to 300 is charged what keeping it allocates,
/// kept, cloned and a row's, parsed with unique keys and with its first
/// key again (a grown table, its length the same): its storage, keys and
/// values, exactly ([`EXACT`]) under the backend this build has. Its
/// layout is the locked crates' (indexmap and hashbrown's, std's B-tree):
/// a drift in any of them fails here.
#[test]
fn a_maps_storage_is_charged_as_it_is_allocated() {
    mine();
    let mut wrong = Vec::new();
    for n in 0..=300 {
        check(&format!("{n} entries"), &parse(&object(n, 0)), &mut wrong);
        if n > 0 {
            check(&format!("{n} entries, its first key again"), &parse(&object(n, 1)), &mut wrong);
        }
    }
    assert!(wrong.is_empty(), "{} maps: {wrong:#?}", backend());
}

/// Each `IndexMap` table's capacity: full, a key again grows it.
const FULL: [usize; 5] = [3, 7, 14, 28, 56];

/// Shady #101 R8-1: duplicate keys on the wire. serde_json inserts every
/// pair; an `IndexMap` insert reserves before it looks the key up, so a
/// full table grows on a key again, its length the same, and a clone
/// copies the grown table and reserves entries to its capacity
/// (`{"a":null,"b":null,"c":null,"a":null}` cloned: 819 bytes for 367
/// charged). At each table's capacity, the key once again and every key
/// again: parsed, a clone of the parse, and nested (an array of them, an
/// input of them, arrays of arrays), each kept, cloned and a row's, is
/// charged what it allocates.
#[test]
fn duplicate_keys_at_each_tables_capacity_are_charged_as_kept() {
    mine();
    let mut wrong = Vec::new();
    for n in FULL {
        for again in [1, n] {
            let one = object(n, again);
            let parsed = parse(&one);
            let name = format!("{n} keys, {again} again");
            check(&format!("{name}: parsed"), &parsed, &mut wrong);
            check(&format!("{name}: a clone of the parse"), &parsed.clone(), &mut wrong);
            check(&format!("{name}: 64 in an array"), &parse(&many(&one, 64)), &mut wrong);
            check(&format!("{name}: an input of 16"), &parse(&format!(r#"{{"ops":{}}}"#, many(&one, 16))), &mut wrong);
            check(&format!("{name}: arrays of arrays"), &parse(&format!("[[{one},{one}],[{one}],[]]")), &mut wrong);
        }
    }
    assert!(wrong.is_empty(), "{} maps: {wrong:#?}", backend());
}

/// Shady #101 R8-1: a map changed after a clone. A key set again by
/// `insert` grows a full table as a duplicate does; a new key advances its
/// length, the table with it. Both, at each table's capacity, kept, cloned
/// and a row's, are charged what they allocate; so is a kept input set
/// again and kept again (anything that changed a kept input would keep it
/// again: nothing does).
#[test]
fn a_key_set_again_or_added_after_a_clone_is_charged_as_kept() {
    mine();
    let mut wrong = Vec::new();
    let set = |v: &Value, key: &str| {
        let mut v = v.clone();
        v.as_object_mut().unwrap().insert(key.into(), Value::Null);
        v
    };
    for n in FULL {
        let base = parse(&object(n, 0));
        check(&format!("{n} keys, the first set again after a clone"), &set(&base, "k000"), &mut wrong);
        check(&format!("{n} keys, a new one added after a clone"), &set(&base, "new"), &mut wrong);
        let mut kept = keep(&base);
        kept.as_object_mut().unwrap().insert("k000".into(), Value::Null);
        check(&format!("{n} keys kept, the first set again"), &kept, &mut wrong);
    }
    assert!(wrong.is_empty(), "{} maps: {wrong:#?}", backend());
}

/// An attach frame whose history has `calls` block calls of `input`
/// (raw wire bytes, spliced in after the frame is written, so nothing
/// rewrites them), each refused, as `decode_frame` makes it.
fn raw_attach(calls: usize, input: &str) -> Attached {
    let mut a: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/tests/fixtures/attached_history.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    let id = |k: usize| format!("toolu_dup{k}");
    let uses: Vec<Value> =
        (0..calls).map(|k| json!({"type": "tool_use", "id": id(k), "name": "cards_block", "input": "@INPUT@"})).collect();
    let results: Vec<Value> =
        (0..calls).map(|k| json!({"type": "tool_result", "tool_use_id": id(k), "content": "cards_block: refused (a test)"})).collect();
    let messages = json!([
        {"role": "user", "content": "go"},
        {"role": "assistant", "content": uses},
        {"role": "user", "content": results},
    ]);
    a["conversation"]["api_messages"] = messages;
    a["conversation"]["messages_len"] = json!(3);
    a["conversation"]["pending_events_len"] = json!(0);
    a["replay"] = json!([]);
    a["streaming"] = json!(false);
    let wire = a.to_string().replace(r#""@INPUT@""#, input);
    let DaemonFrame::Attached(a) = decode_frame(&wire).unwrap() else { panic!() };
    *a
}

/// What a transcript charges for what it keeps of its calls' inputs: its
/// rows' ([`synaps_client::transcript::Item::kept_bytes`]) and its
/// history's calls' ([`Transcript::blocks_bytes`]).
fn charged(t: &Transcript) -> usize {
    t.items().iter().map(|i| i.kept_bytes()).sum::<usize>() + t.blocks_bytes()
}

/// Shady #101 R8-1 on the real adapter: a raw `Attached` frame through
/// `decode_frame` (not `json!` or `decode_event`, which rebuild the maps
/// and would hide it), its history's block calls' inputs a list of objects
/// with a duplicate key at a full table's capacity. The frame keeps the
/// grown tables (read: its input's clone allocates more than the same
/// input without the duplicate). Attached, on an account of its own, the
/// history retains each input twice (its row's, its call's); against the
/// same attach without the duplicates (the same values), what it holds
/// more is charged more.
#[test]
fn a_raw_attach_with_duplicate_keys_is_charged_what_it_keeps() {
    mine();
    let calls = 64;
    let one = |again: usize| {
        let d = object(3, again);
        format!(r#"{{"ops":[{d},{d},{d},{d}]}}"#)
    };
    let (dup, plain) = (raw_attach(calls, &one(1)), raw_attach(calls, &one(0)));
    let input = |a: &Attached| a.conversation.api_messages[1]["content"][0]["input"].clone();
    assert_eq!(input(&dup), input(&plain), "the same values");
    let (_, dup_clone) = made(|| input(&dup));
    let (_, plain_clone) = made(|| input(&plain));
    // Under `BTreeMap` a key again only replaces its value.
    if ordered() {
        assert!(dup_clone > plain_clone, "the frame kept the duplicates' tables: a clone {dup_clone} bytes, {plain_clone} without");
    }
    let load = |a: &Attached| {
        let mut t = Transcript::new();
        t.load_attached(a).unwrap();
        t
    };
    drop(load(&dup)); // anything made once, first
    let held = |a: &Attached| {
        let account = mine();
        let t = load(a);
        (usize::try_from(held_by(account)).unwrap(), charged(&t), t)
    };
    let (dup_held, dup_charged, _dup) = held(&dup);
    let (plain_held, plain_charged, _plain) = held(&plain);
    eprintln!(
        "{} maps, {calls} calls: with the duplicates held {dup_held}, charged {dup_charged}; without, held {plain_held}, charged {plain_charged}",
        backend()
    );
    assert!(
        dup_held.saturating_sub(plain_held) <= dup_charged.saturating_sub(plain_charged),
        "{} maps: the duplicates' attach holds {} bytes more, is charged {} more",
        backend(),
        dup_held as isize - plain_held as isize,
        dup_charged as isize - plain_charged as isize
    );
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
