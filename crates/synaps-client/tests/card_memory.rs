//! What the card sync keeps to read what changed (`Transcript::sync_cards`,
//! Shady #102 P2-1), measured on the heap: digests and places of the live
//! events, never a second copy of their inputs or text, or of a card a row
//! draws (the rows keep those); counted (`Transcript::card_sync_bytes`);
//! and let go as soon as it can't be used: a detach, a trim, a tab cached.
//!
//! The heap is counted by the allocator below, by thread, two ways:
//! - [`held`]: each block is charged to the account of the thread that
//!   allocated it ([`mine`]: each test's its own) and taken off that
//!   account wherever it is freed. What a test's transcript still holds,
//!   not the harness's or another test's heap; what another thread frees
//!   of it (a disposer's) counts once freed.
//! - [`here`]: what this thread allocated less what it freed, whoever's.
//!   Read across something this thread does (a shed), it is what that
//!   frees, whatever another thread does meanwhile.
//!
//! Here nothing is freed elsewhere (no disposer): what a transcript lets
//! go is freed as it is let go, so every measurement is read directly,
//! nothing waited for.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use serde_json::{Value, json};
use synaps_client::protocol::*;
use synaps_client::transcript::Transcript;

struct Counting;

/// The accounts: the bytes each holds. 0: the threads that took none.
static HELD: [AtomicIsize; 64] = [const { AtomicIsize::new(0) }; 64];
static NEXT: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    static ACCOUNT: Cell<usize> = const { Cell::new(0) };
    static HERE: Cell<isize> = const { Cell::new(0) };
}

/// A block of `l` and its account before it: the block's alignment (a
/// word at least) of room, the account at its start. None: too big.
fn outer(l: Layout) -> Option<(Layout, usize)> {
    let room = l.align().max(std::mem::size_of::<usize>());
    Some((Layout::from_size_align(l.size().checked_add(room)?, room).ok()?, room))
}

/// `l`'s block at `base` (null: none), charged to this thread's account.
unsafe fn opened(base: *mut u8, l: Layout, room: usize) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    let account = ACCOUNT.get();
    unsafe { (base as *mut usize).write(account) };
    HELD[account].fetch_add(l.size() as isize, Ordering::Relaxed);
    HERE.set(HERE.get() + l.size() as isize);
    unsafe { base.add(room) }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let Some((o, room)) = outer(l) else { return std::ptr::null_mut() };
        unsafe { opened(System.alloc(o), l, room) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let Some((o, room)) = outer(l) else { return std::ptr::null_mut() };
        unsafe { opened(System.alloc_zeroed(o), l, room) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let (o, room) = outer(l).expect("a block allocated");
        let base = unsafe { p.sub(room) };
        let account = unsafe { (base as *const usize).read() };
        HELD[account].fetch_sub(l.size() as isize, Ordering::Relaxed);
        HERE.set(HERE.get() - l.size() as isize);
        unsafe { System.dealloc(base, o) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let (o, room) = outer(l).expect("a block allocated");
        let Some(grown) = size.checked_add(room) else { return std::ptr::null_mut() };
        let base = unsafe { p.sub(room) };
        let account = unsafe { (base as *const usize).read() };
        let q = unsafe { System.realloc(base, o, grown) };
        if q.is_null() {
            return q;
        }
        // The block's account is the one it was allocated on, moved with it.
        HELD[account].fetch_add(size as isize - l.size() as isize, Ordering::Relaxed);
        HERE.set(HERE.get() + size as isize - l.size() as isize);
        unsafe { q.add(room) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// This thread's own account, from now on (a test's first line).
fn mine() {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    assert!(n < HELD.len(), "more tests than accounts");
    ACCOUNT.set(n);
}

/// The bytes this thread's account holds: what it allocated (since
/// [`mine`]) and is not freed, wherever it is freed.
fn held() -> isize {
    HELD[ACCOUNT.get()].load(Ordering::Relaxed)
}

/// The bytes this thread allocated less those it freed, whoever's.
fn here() -> isize {
    HERE.get()
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
    mine();
    let mut t = attach(&with_a_card());
    let base = held();
    turn(&mut t);
    for k in 0..9 {
        refused(&mut t, &format!("r{k}"), 8 * MIB);
    }
    let inputs = (9 * 8 * MIB) as isize;
    let kept = held() - base;
    assert!(kept >= inputs, "the rows keep their inputs: {kept} bytes held");
    assert!(kept < inputs + 2 * MIB as isize, "{kept} bytes held for {inputs} of inputs: a copy more");
    let sync = t.card_sync_bytes();
    assert!(sync > 0 && sync < 16 << 10, "the card sync keeps {sync} bytes for nine calls");
    let cards = t.cards().cards().to_vec();
    assert_eq!(cards.len(), 10, "the history's card and the nine refused");
    t.set_detached();
    let detached = held() - base;
    assert!(detached < inputs + 2 * MIB as isize, "detached: {detached} bytes held");
    assert_eq!(t.card_sync_bytes(), 0, "detached: the card sync's state let go");
    assert_eq!(t.cards().cards(), &cards[..], "detached: the cards kept");
    t.load_attached(&attached(&with_a_card())).unwrap();
    let again = held() - base;
    assert!(again < MIB as isize, "reattached: the live rows gone, {again} bytes held");
}

/// A trim takes the rows with the inputs (a session with no history: the
/// front of its live turn) and, with no event after it, the card sync
/// keeps none of them (ecd8856 kept a copy of each in `seen` until the
/// next sync).
#[test]
fn a_trim_lets_go_of_the_card_sync_of_the_rows_it_takes() {
    mine();
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
    let kept = held() - base;
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
    let before = here();
    t.shed_card_sync();
    let freed = before - here();
    assert!(freed > 0, "shedding frees what it kept");
    assert!(counted as isize >= freed, "{counted} bytes counted, {freed} freed: a card kept apart");
}
