//! What the card sync keeps to read what changed (`Transcript::sync_cards`,
//! Shady #102 P2-1), measured on the heap: digests and places of the live
//! events, never a second copy of their inputs or text, or of a card a row
//! draws (the rows keep those); counted (`Transcript::card_sync_bytes`);
//! and let go as soon as it can't be used: a detach, a trim, a tab cached.
//!
//! The heap is counted by the allocator ([`heap`]), by thread, two ways:
//! - [`held`]: each block is charged to the account of the thread that
//!   allocated it ([`mine`]: each test's its own) and taken off that
//!   account wherever it is freed. What a test's transcript still holds,
//!   not the harness's or another test's heap; what another thread frees
//!   of it (a disposer's) counts once freed.
//! - [`here`]: what this thread allocated less what it freed, whoever's.
//!   Read across something this thread does (a shed), it is what that
//!   frees, whatever another thread does meanwhile.
//!
//! Each test installs a disposer of its own before its account starts, as
//! the app does (`synaps_client::blocks::dispose`): what a transcript's
//! live log lets go (a trim's, a reattach's) is freed on the disposer's
//! thread, off this one. So every [`held`] sample is taken after the
//! disposer is flushed ([`held_after`]: all it was handed before is
//! dropped), never after a quiet spell; a [`here`] baseline after a flush
//! too. With the disposer held, what it was handed stays this account's
//! until it drains ([`a_trim_with_the_disposer_held_frees_its_inputs_once_it_drains`]).

use std::alloc::{GlobalAlloc, Layout};

use serde_json::{Value, json};
use synaps_client::blocks::dispose::Disposer;
use synaps_client::protocol::*;
use synaps_client::transcript::Transcript;

mod heap;
use heap::{COUNTING, held, here, mine, outer};

/// [`held`] once `d` has dropped all it was handed before (a hold of it
/// standing, this waits for the hold to go: drop it first).
fn held_after(d: &Disposer) -> isize {
    d.flush();
    held()
}

/// A disposer installed on this thread, as the app's (its thread and
/// channels made before [`mine`]: not the test's account).
fn disposer() -> (Disposer, synaps_client::blocks::dispose::Installed) {
    let d = Disposer::start();
    let installed = d.install();
    (d, installed)
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
    let (d, _in) = disposer();
    mine();
    let mut t = attach(&with_a_card());
    let base = held_after(&d);
    turn(&mut t);
    for k in 0..9 {
        refused(&mut t, &format!("r{k}"), 8 * MIB);
    }
    let inputs = (9 * 8 * MIB) as isize;
    let kept = held_after(&d) - base;
    assert!(kept >= inputs, "the rows keep their inputs: {kept} bytes held");
    assert!(kept < inputs + 2 * MIB as isize, "{kept} bytes held for {inputs} of inputs: a copy more");
    let sync = t.card_sync_bytes();
    assert!(sync > 0 && sync < 16 << 10, "the card sync keeps {sync} bytes for nine calls");
    let cards = t.cards().cards().to_vec();
    assert_eq!(cards.len(), 10, "the history's card and the nine refused");
    t.set_detached();
    let detached = held_after(&d) - base;
    assert!(detached < inputs + 2 * MIB as isize, "detached: {detached} bytes held");
    assert_eq!(t.card_sync_bytes(), 0, "detached: the card sync's state let go");
    assert_eq!(t.cards().cards(), &cards[..], "detached: the cards kept");
    t.load_attached(&attached(&with_a_card())).unwrap();
    let again = held_after(&d) - base;
    assert!(again < MIB as isize, "reattached: the live rows gone, {again} bytes held");
}

/// A trim takes the rows with the inputs (a session with no history: the
/// front of its live turn) and, with no event after it, the card sync
/// keeps none of them (ecd8856 kept a copy of each in `seen` until the
/// next sync).
#[test]
fn a_trim_lets_go_of_the_card_sync_of_the_rows_it_takes() {
    let (d, _in) = disposer();
    mine();
    let mut t = attach(&[]);
    let base = held_after(&d);
    turn(&mut t);
    for k in 0..4 {
        refused(&mut t, &format!("r{k}"), 8 * MIB);
    }
    for k in 0..20 {
        call(&mut t, "bash", &format!("b{k}"), json!({"command": "ls"}), "a b");
    }
    assert!(held_after(&d) - base >= (32 * MIB) as isize, "the rows keep their inputs");
    assert!(t.card_sync_bytes() > 0, "the card sync kept");
    let n = t.trim_live_front(10, usize::MAX);
    assert!(n > 4, "the front trimmed: {n} rows");
    let kept = held_after(&d) - base;
    assert!(kept < MIB as isize, "trimmed, no event since: {kept} bytes held");
    assert_eq!(t.card_sync_bytes(), 0, "trimmed: the card sync's state let go");
    // The next event reads every row again, and keeps what it reads.
    call(&mut t, "bash", "b20", json!({"command": "ls"}), "a b");
    assert!(t.card_sync_bytes() > 0, "read again and kept");
}

/// The trim's case with the disposer's thread held (Shady #102 P3-2): the
/// trim takes the rows, but the inputs the live log kept go with its log,
/// handed to the disposer, so they stay alive, still this account's and
/// charged there, for as long as the disposer is held. The hold gone and
/// the disposer drained, they are freed on its thread and leave this
/// account: the release is the disposer's, seen after it, not a quiet
/// sample.
#[test]
fn a_trim_with_the_disposer_held_frees_its_inputs_once_it_drains() {
    let (d, _in) = disposer();
    mine();
    let mut t = attach(&[]);
    let base = held_after(&d);
    turn(&mut t);
    for k in 0..3 {
        refused(&mut t, &format!("r{k}"), 8 * MIB);
    }
    for k in 0..20 {
        call(&mut t, "bash", &format!("b{k}"), json!({"command": "ls"}), "a b");
    }
    let inputs = (3 * 8 * MIB) as isize;
    assert!(held_after(&d) - base >= inputs, "the rows keep their inputs");
    let hold = d.hold();
    let n = t.trim_live_front(10, usize::MAX);
    assert!(n > 3, "the front trimmed: {n} rows");
    assert_eq!(t.card_sync_bytes(), 0, "trimmed: the card sync's state let go");
    // Held: nothing it was handed is dropped, so this reads the same now
    // as at any later moment before the hold goes.
    let queued = held() - base;
    assert!(queued >= inputs, "the disposer held: the trimmed inputs alive, {queued} bytes held");
    let b = d.backlog();
    assert!(b.items > 0 && b.bytes >= inputs as usize, "and queued there, charged: {b:?}");
    drop(hold);
    let kept = held_after(&d) - base;
    assert!(kept < MIB as isize, "drained: {kept} bytes held");
    let b = d.backlog();
    assert_eq!((b.items, b.bytes, b.in_place), (0, 0, 0), "drained: {b:?}");
}

/// What [`Transcript::card_sync_bytes`] counts is what the card sync
/// holds: over 2,000 calls and five cards, the heap shedding it frees, or
/// a little more (its tables' estimate), never less.
#[test]
fn the_card_sync_counts_what_it_keeps() {
    let (d, _in) = disposer();
    mine();
    let mut t = attach(&with_a_card());
    turn(&mut t);
    for k in 0..2_000 {
        call(&mut t, "bash", &format!("b{k}"), json!({"command": "ls"}), "a b");
        if k % 400 == 0 {
            call(&mut t, "cards_ui", &format!("c{k}"), ask(), &showing("c81kq0z", &ask()));
        }
    }
    let counted = t.card_sync_bytes();
    d.flush();
    let before = here();
    t.shed_card_sync();
    let freed = before - here();
    assert!(freed > 0, "shedding frees what it kept");
    assert!(counted as isize >= freed, "{counted} bytes counted, {freed} freed");
    assert!((counted as isize) < freed * 3 / 2 + 4096, "{counted} bytes counted, {freed} freed: near");
    assert_eq!(t.card_sync_bytes(), 0);
}

/// A block another thread frees is taken off the account of the thread
/// that allocated it, once freed, not before (what a disposer's thread
/// drops of a transcript's counts so).
#[test]
fn a_block_freed_on_another_thread_leaves_the_account_it_was_charged_to() {
    let (give, take) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
    let (go, wait) = std::sync::mpsc::sync_channel::<()>(1);
    let (freed, done) = std::sync::mpsc::sync_channel::<()>(1);
    let other = std::thread::spawn(move || {
        let block = take.recv().unwrap();
        wait.recv().unwrap();
        drop(block);
        freed.send(()).unwrap();
    });
    // The thread and channels are not this account's: the block alone is.
    mine();
    let base = held();
    give.send(vec![7u8; MIB]).unwrap();
    assert_eq!(held() - base, MIB as isize, "allocated here, held by the other thread");
    go.send(()).unwrap();
    // Not `recv`: a thread's first wait allocates (its waiting context).
    while done.try_recv().is_err() {
        std::thread::yield_now();
    }
    assert_eq!(held() - base, 0, "freed there");
    other.join().unwrap();
}

/// A block grown to a size the caller may ask for (it fits rounded up to
/// the block's alignment) but too big for a layout once the room before
/// it is added is refused: null, the block left as it was, and its
/// account and this thread's count with it. At each alignment's boundary:
/// the largest size whose block (room added) is a layout, and one more.
#[test]
fn a_block_grown_past_the_largest_layout_is_refused_and_kept() {
    mine();
    for align in [8, 32] {
        let room = align;
        let largest = isize::MAX as usize - (room - 1) - room;
        let fits = |size| outer(Layout::from_size_align(size, align).unwrap()).map(|(o, _)| o.size());
        assert_eq!(fits(largest), Some(largest + room), "align {align}: the largest, room added");
        assert_eq!(fits(largest + 1), None, "align {align}: one more");
        let l = Layout::from_size_align(64, align).unwrap();
        unsafe {
            let p = COUNTING.alloc(l);
            assert!(!p.is_null());
            p.write_bytes(7, l.size());
            let (base, there) = (held(), here());
            assert!(COUNTING.realloc(p, l, largest + 1).is_null(), "align {align}: refused");
            assert_eq!((held(), here()), (base, there), "align {align}: counted as it was");
            assert!(std::slice::from_raw_parts(p, l.size()).iter().all(|&b| b == 7), "align {align}: kept");
            COUNTING.dealloc(p, l);
            assert_eq!((held(), here()), (base - 64, there - 64), "align {align}: freed whole");
        }
    }
}

/// A card of the most its schema lets an input say, more or less.
fn big(k: usize) -> Value {
    json!({"question": format!("Ship build {k}?"), "detail": "d".repeat(1_400),
        "options": (0..9).map(|i| json!({"label": format!("{i}{}", "l".repeat(50)), "hint": "h".repeat(110)})).collect::<Vec<_>>()})
}

fn big_card(k: usize) -> String {
    format!("cbig00{k}")
}

/// A live close read again whose meaning did not change (its result
/// reported again, a line more: the fold reads its first) keeps the card
/// its row draws, not the copy the read made of it (31d66e7 kept that copy
/// with the close, a card's heap per close that no count had, Shady #102
/// P3-1): shedding the card sync frees what it counts, no more.
#[test]
fn a_close_read_again_keeps_no_copy_of_its_card() {
    let (d, _in) = disposer();
    mine();
    let mut h = vec![json!({"role": "user", "content": "go"})];
    h.push(json!({"role": "assistant", "content": (0..4).map(|k|
        json!({"type": "tool_use", "id": format!("h{k}"), "name": "cards_ui", "input": big(k)})).collect::<Vec<_>>()}));
    h.push(json!({"role": "user", "content": (0..4).map(|k|
        json!({"type": "tool_result", "tool_use_id": format!("h{k}"), "content": showing(&big_card(k), &big(k))})).collect::<Vec<_>>()}));
    let mut t = attach(&h);
    turn(&mut t);
    for k in 0..4 {
        let closed = cards_spec::Envelope::closed(cards_spec::CardId::parse(&big_card(k)).unwrap()).to_string();
        call(&mut t, "cards_ui", &format!("x{k}"), json!({ "close": big_card(k) }), &closed);
        apply(&mut t, json!({"kind": "llm", "llm": "tool_result", "tool_id": format!("x{k}"), "result": format!("{closed}\nagain")}));
    }
    let closed = t.cards().cards().iter().filter(|c| c.state == cards_spec::fold::State::Closed).count();
    assert_eq!(closed, 4, "the four closed");
    let counted = t.card_sync_bytes();
    d.flush();
    let before = here();
    t.shed_card_sync();
    let freed = before - here();
    assert!(freed > 0, "shedding frees what it kept");
    assert!(counted as isize >= freed, "{counted} bytes counted, {freed} freed: a card kept apart");
}
