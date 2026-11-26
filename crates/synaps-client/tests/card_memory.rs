//! What the card sync keeps to read what changed (`Transcript::sync_cards`,
//! Shady #102 P2-1), measured on the heap: digests and places of the live
//! events, never a second copy of their inputs or text (the rows keep
//! those); counted (`Transcript::card_sync_bytes`); and let go as soon as
//! it can't be used: a detach, a trim, a tab cached.
//!
//! The heap is the process's, counted by the allocator below: what is
//! allocated and not freed, whichever thread frees it (a disposer thread
//! may drop what a transcript lets go: what is let go is waited for,
//! [`settled`]). The tests here run one at a time ([`one`]).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use synaps_client::protocol::*;
use synaps_client::transcript::Transcript;

struct Counting;

static HELD: AtomicIsize = AtomicIsize::new(0);

fn count(n: isize) {
    HELD.fetch_add(n, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            count(l.size() as isize);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            count(l.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        count(-(l.size() as isize));
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, size) };
        if !q.is_null() {
            count(size as isize - l.size() as isize);
        }
        q
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The bytes the process holds.
fn held() -> isize {
    HELD.load(Ordering::Relaxed)
}

/// One test at a time: the heap counted is the process's.
fn one() -> std::sync::MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The bytes held over `base` once what was let go is freed: once `done`
/// holds of them, or, if it never does, once they stopped moving (5 s at
/// most).
fn settled(base: isize, done: impl Fn(isize) -> bool) -> isize {
    let start = Instant::now();
    let mut last = held() - base;
    while !done(last) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
        let now = held() - base;
        if now == last && start.elapsed() > Duration::from_millis(500) {
            break;
        }
        last = now;
    }
    last
}

const MIB: usize = 1 << 20;
const CARD: &str = "c4f9x2a";

fn ev(v: Value) -> SessionEvent {
    decode_event(&v).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn llm(v: Value) -> SessionEvent {
    ev(json!({"ev": "stream", "event": v}))
}

fn attached(messages: &[Value]) -> Attached {
    let mut a: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/tests/fixtures/attached_history.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    a["conversation"]["api_messages"] = json!(messages);
    a["conversation"]["messages_len"] = json!(messages.len());
    a["conversation"]["pending_events_len"] = json!(0);
    a["replay"] = json!([]);
    a["streaming"] = json!(false);
    let DaemonFrame::Attached(a) = decode_frame(&a.to_string()).unwrap() else { panic!() };
    *a
}

fn ask() -> Value {
    json!({"question": "Ship it?", "options": ["Yes", "No"]})
}

/// The result the extension writes for `input` under `card`.
fn showing(card: &str, input: &Value) -> String {
    let Ok(cards_spec::Input::Create { doc, wait }) = cards_spec::parse_input(input) else { panic!() };
    format!("{}\nIts answer will come…", cards_spec::Envelope::showing(cards_spec::CardId::parse(card).unwrap(), &doc, wait))
}

/// A history with one card, bound: the card sync runs on every event.
fn with_a_card() -> Vec<Value> {
    vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "h1", "name": "cards_ui", "input": ask()}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "h1", "content": showing(CARD, &ask())}]}),
    ]
}

fn attach(h: &[Value]) -> Transcript {
    let mut t = Transcript::new();
    t.load_attached(&attached(h)).unwrap();
    t
}

fn apply(t: &mut Transcript, e: Value) {
    let e = llm(e);
    t.apply(&e);
}

/// A call of `name`, whole, then its result.
fn call(t: &mut Transcript, name: &str, id: &str, input: Value, result: &str) {
    apply(t, json!({"kind": "llm", "llm": "tool_use_start", "tool_name": name, "tool_id": id}));
    apply(t, json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input}));
    apply(t, json!({"kind": "llm", "llm": "tool_started", "tool_id": id}));
    apply(t, json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": result}));
}

/// A card call the extension refuses (its input doesn't validate), with
/// `size` bytes of string in it.
fn refused(t: &mut Transcript, id: &str, size: usize) {
    call(t, "cards_ui", id, json!({"bogus": "x".repeat(size)}), "cards_ui: refused: no question");
}

fn turn(t: &mut Transcript) {
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})));
}

/// Shady's case: nine card calls refused, 8 MiB of string in each input,
/// before the turn lands in the history. The rows keep the inputs
/// whole (72 MiB): the card sync keeps no second copy (ecd8856 kept one
/// in `seen`, 72 MiB more that no tab estimate counted). A detach keeps
/// the cards and lets the sync's state go; a reattach keeps none of it.
#[test]
fn nine_refused_inputs_of_8_mib_are_kept_once() {
    let _one = one();
    let mut t = attach(&with_a_card());
    let base = held();
    turn(&mut t);
    for k in 0..9 {
        refused(&mut t, &format!("r{k}"), 8 * MIB);
    }
    let inputs = (9 * 8 * MIB) as isize;
    let kept = settled(base, |n| n < inputs + 2 * MIB as isize);
    assert!(kept >= inputs, "the rows keep their inputs: {kept} bytes held");
    assert!(kept < inputs + 2 * MIB as isize, "{kept} bytes held for {inputs} of inputs: a copy more");
    let sync = t.card_sync_bytes();
    assert!(sync > 0 && sync < 16 << 10, "the card sync keeps {sync} bytes for nine calls");
    let cards = t.cards().cards().to_vec();
    assert_eq!(cards.len(), 10, "the history's card and the nine refused");
    t.set_detached();
    let detached = settled(base, |n| n < inputs + 2 * MIB as isize);
    assert!(detached < inputs + 2 * MIB as isize, "detached: {detached} bytes held");
    assert_eq!(t.card_sync_bytes(), 0, "detached: the card sync's state let go");
    assert_eq!(t.cards().cards(), &cards[..], "detached: the cards kept");
    t.load_attached(&attached(&with_a_card())).unwrap();
    let again = settled(base, |n| n < MIB as isize);
    assert!(again < MIB as isize, "reattached: the live rows gone, {again} bytes held");
}

/// A trim takes the rows with the inputs (a session with no history: the
/// front of its live turn) and, with no event after it, the card sync
/// keeps none of them (ecd8856 kept a copy of each in `seen` until the
/// next sync).
#[test]
fn a_trim_lets_go_of_the_card_sync_of_the_rows_it_takes() {
    let _one = one();
    let mut t = attach(&[]);
    let base = held();
    turn(&mut t);
    for k in 0..4 {
        refused(&mut t, &format!("r{k}"), 8 * MIB);
    }
    for k in 0..20 {
        call(&mut t, "bash", &format!("b{k}"), json!({"command": "ls"}), "a b");
    }
    assert!(held() - base >= (32 * MIB) as isize, "the rows keep their inputs");
    assert!(t.card_sync_bytes() > 0, "the card sync kept");
    let n = t.trim_live_front(10, usize::MAX);
    assert!(n > 4, "the front trimmed: {n} rows");
    let kept = settled(base, |n| n < MIB as isize);
    assert!(kept < MIB as isize, "trimmed, no event since: {kept} bytes held");
    assert_eq!(t.card_sync_bytes(), 0, "trimmed: the card sync's state let go");
    // The next event reads every row again, and keeps what it reads.
    call(&mut t, "bash", "b20", json!({"command": "ls"}), "a b");
    assert!(t.card_sync_bytes() > 0, "read again and kept");
}

/// What [`Transcript::card_sync_bytes`] counts is what the card sync
/// holds: over 2,000 calls and five cards, the heap shedding it frees, or
/// a little more (its tables' estimate), never less.
#[test]
fn the_card_sync_counts_what_it_keeps() {
    let _one = one();
    let mut t = attach(&with_a_card());
    turn(&mut t);
    for k in 0..2_000 {
        call(&mut t, "bash", &format!("b{k}"), json!({"command": "ls"}), "a b");
        if k % 400 == 0 {
            call(&mut t, "cards_ui", &format!("c{k}"), ask(), &showing("c81kq0z", &ask()));
        }
    }
    let counted = t.card_sync_bytes();
    // Nothing let go before is still being freed.
    settled(0, |_| false);
    let before = held();
    t.shed_card_sync();
    let freed = before - held();
    assert!(freed > 0, "shedding frees what it kept");
    assert!(counted as isize >= freed, "{counted} bytes counted, {freed} freed");
    assert!((counted as isize) < freed * 3 / 2 + 4096, "{counted} bytes counted, {freed} freed: near");
    assert_eq!(t.card_sync_bytes(), 0);
}
