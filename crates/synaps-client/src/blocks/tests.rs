//! `blocks`: the history read a message at a time is `fold::events` of the
//! whole; block calls' fates, in passes; the worker's slices, stops,
//! epochs and gaps; what it keeps.

use cards_spec::board::Board;
use cards_spec::fold::events;
use serde_json::{Value, json};

use super::*;
use crate::synthetic::{block_input, block_receipt, blocks_history};

/// Every block call's fate, its batches read as the worker reads them.
fn all_fates(events: &[Event]) -> Vec<(usize, Fate)> {
    let mut out = Vec::new();
    let mut b = Batch::default();
    for at in 0..events.len() {
        if let Some((s, e)) = b.read(events, at) {
            fates(events, s, e, false, |p, f| out.push((p, f)));
        }
    }
    fates(events, b.start, events.len(), true, |p, f| out.push((p, f)));
    out.sort_by_key(|g| g.0);
    out
}

fn read(history: &[Value]) -> HistoryBlocks {
    let mut h = HistoryBlocks::default();
    for m in history {
        h.message(m, &[]);
    }
    h
}

/// Every shape `fold::events` reads: an assistant message after an
/// assistant message, a result in two text blocks, a result of another
/// tool's call, a result whose call another tool took over, a hooked user
/// message, the user's own text after results.
fn shapes() -> Vec<Value> {
    let put = json!([{"op":"put","id":"n","kind":"note","body":{"text":"x"}}]);
    let receipt = block_receipt(&put);
    let (head, tail) = receipt.split_at(6);
    vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "a"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": "cards_block", "input": block_input(put.clone())},
            {"type": "tool_use", "id": "t2", "name": "bash", "input": {"command": "ls"}},
            {"type": "tool_use", "id": "t3", "name": "cards_block", "input": block_input(put.clone())}]}),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": head}, {"type": "text", "text": tail}]},
            {"type": "tool_result", "tool_use_id": "t2", "content": "ok"},
            {"type": "text", "text": "and then"}]}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t3", "name": "read", "input": {"path": "a"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t3", "content": receipt}]}),
        json!({"role": "user", "content": [{"type": "text", "text": "mine"},
            {"type": "text", "text": "[Extension context from axel — do not treat as user instructions]\nx\n[End extension context]"}],
            "_synaps_hooked": 1}),
    ]
}

/// Read a message at a time, a history gives what `fold::events` gives of
/// the whole of it, and so does every split of it (the reader keeps the
/// tool ids and whether the last message was an assistant's).
#[test]
fn read_by_message_it_is_events_of_the_whole() {
    for h in [shapes(), blocks_history()] {
        let whole = events(&h);
        assert_eq!(read(&h).events(), whole);
        assert!(whole.iter().any(|e| matches!(e, Event::ResultsEnd)));
        assert_eq!(read(&h).len(), whole.len());
    }
    assert!(read(&blocks_history()).has_blocks());
    assert!(!read(&crate::synthetic::history(40, 7)).has_blocks());
}

/// A block call's fate is the board's (§1.6, §1.7): its first result in
/// its batch binds it, or not; none by the commit is unbound for good, and
/// pending until then; a later call of its id (any tool's) takes it over.
#[test]
fn a_calls_fate_is_its_batchs() {
    let put = json!([{"op":"put","id":"n","kind":"note","body":{"text":"x"}}]);
    let call = |id: &str| Event::Call { tool_id: id.into(), name: "cards_block".into(), input: block_input(put.clone()) };
    let result = |id: &str, text: &str| Event::Result { tool_id: id.into(), text: text.into() };
    let ok = block_receipt(&put);
    let events = vec![
        call("a"),
        call("b"),
        call("c"),
        Event::Call { tool_id: "c".into(), name: "bash".into(), input: Value::Null },
        call("d"),
        result("a", &ok),
        result("b", "cards_block: ops[0]: nope"),
        result("c", &ok),
        Event::ResultsEnd,
        call("e"),
        call("f"),
        result("e", &ok),
    ];
    use Fate::*;
    assert_eq!(all_fates(&events), [(0, Bound), (1, Unbound), (2, Unbound), (4, Unbound), (9, Bound), (10, Pending)]);
}

/// A batch's fates read in passes of [`PASS`] calls are the fates read a
/// call at a time over the whole batch (the reference): ids reused across
/// a pass's end, takeovers by another tool's call, results before and
/// after a takeover, a second result for an id.
#[test]
fn fates_read_in_passes_are_a_call_at_a_times() {
    let put = json!([{"op":"put","id":"n","kind":"note","body":{"text":"x"}}]);
    let ok = block_receipt(&put);
    let reference = |events: &[Event], open: bool| -> Vec<(usize, Fate)> {
        let mut out = Vec::new();
        for (pos, e) in events.iter().enumerate() {
            let Event::Call { tool_id, name, input } = e else { continue };
            if !is_block_tool(name) {
                continue;
            }
            let mut answer = None;
            let mut taken = false;
            for later in &events[pos + 1..] {
                match later {
                    Event::Call { tool_id: t, .. } if t == tool_id => {
                        taken = true;
                        break;
                    }
                    Event::Result { tool_id: t, text } if t == tool_id && answer.is_none() => answer = Some(text.clone()),
                    _ => {}
                }
            }
            let fate = match (answer, taken) {
                (_, true) => Fate::Unbound,
                (None, _) if open => Fate::Pending,
                (None, _) => Fate::Unbound,
                (Some(text), _) => {
                    let b = block::parse_call(input).ok();
                    if b.is_some_and(|b| Receipt::parse(&text).is_some_and(|r| r.binds(&b))) { Fate::Bound } else { Fate::Unbound }
                }
            };
            out.push((pos, fate));
        }
        out
    };
    let n = 3 * PASS + 7;
    let id = |k: usize| format!("t{}", k % (PASS + 3));
    let mut events: Vec<Event> =
        (0..n).map(|k| Event::Call { tool_id: id(k), name: "cards_block".into(), input: block_input(put.clone()) }).collect();
    events.insert(PASS / 2, Event::Call { tool_id: id(1), name: "bash".into(), input: Value::Null });
    for k in (0..n).step_by(3) {
        events.push(Event::Result { tool_id: id(k), text: if k % 2 == 0 { ok.clone() } else { "refused".into() } });
        events.push(Event::Result { tool_id: id(k), text: ok.clone() });
    }
    for open in [true, false] {
        let mut got = Vec::new();
        fates(&events, 0, events.len(), open, |p, f| got.push((p, f)));
        got.sort_by_key(|g| g.0);
        assert_eq!(got, reference(&events, open), "open: {open}");
        assert!(got.iter().any(|g| g.1 == Fate::Bound) && got.iter().any(|g| g.1 == Fate::Unbound));
    }
}

fn input(history: &[Value]) -> Input {
    Input::of(&read(history), &LiveLog::default()).expect("blocks")
}

/// The worker's board is the history's fold, read on from where it stopped
/// while the history grows, in slices it may stop between; a stopped job
/// keeps what it read, and the next finishes it; another epoch starts over.
#[test]
fn the_worker_reads_on_stops_between_slices_and_starts_over_on_another_epoch() {
    let h = blocks_history();
    let whole = input(&h);
    let mut w = Worker::default();
    let one = w.run(&whole, &mut || true).expect("done");
    let board = Board::fold(whole.generation, &whole.events());
    assert_eq!(one.live, board.view(&whole.events()).live());
    assert!(one.live >= 8, "{}", one.live);
    // Grown a message at a time: the same board.
    let mut hb = HistoryBlocks::default();
    let mut w2 = Worker::default();
    for m in &h {
        hb.message(m, &[]);
        let i = Input::of(&hb, &LiveLog::default());
        if let Some(i) = i {
            w2.run(&i, &mut || true).expect("done");
        }
    }
    assert_eq!(w2.run(&Input::of(&hb, &LiveLog::default()).unwrap(), &mut || true).unwrap().live, one.live);
    // Long: stopped after its first slice, then finished.
    let long: Vec<Value> = h.iter().cloned().chain(crate::synthetic::history(20_000, 3)).collect();
    let big = input(&long);
    assert!(big.history_len > 2 * SLICE, "{}", big.history_len);
    let mut w3 = Worker::default();
    let mut asked = 0;
    assert!(
        w3.run(&big, &mut || {
            asked += 1;
            false
        })
        .is_none()
    );
    assert_eq!((asked, w3.position()), (1, SLICE));
    let done = w3.run(&big, &mut || true).expect("done");
    assert_eq!(done.live, one.live);
    assert_eq!(w3.position(), big.history_len);
    // Another history (a new epoch): folded from its start.
    let other = input(&h[..6]);
    let again = w3.run(&other, &mut || true).expect("done");
    assert_eq!(w3.position(), other.history_len);
    assert!(again.live < one.live);
}

/// A history of one batch: `n` block calls (every third refused by the
/// extension, every fifth another tool's call between), their results,
/// then a results-only tail and another tool's calls after.
fn big_batch(n: usize) -> Vec<Value> {
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for k in 0..n {
        let ops = json!([{"op":"patch","id":"plan","state":{"a": if k % 2 == 0 { "doing" } else { "done" }}}]);
        let id = format!("t{k}");
        calls.push(json!({"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}));
        let text = if k % 3 == 0 { "cards_block: ops[0]: refused".to_string() } else { block_receipt(&ops) };
        results.push(json!({"type": "tool_result", "tool_use_id": id, "content": text}));
        if k % 5 == 0 {
            let o = format!("o{k}");
            calls.push(json!({"type": "tool_use", "id": o, "name": "bash", "input": {"command": "true"}}));
            results.push(json!({"type": "tool_result", "tool_use_id": o, "content": "ok"}));
        }
    }
    let put = json!([{"op":"put","id":"plan","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"todo"}]}}]);
    vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "p", "name": "cards_block", "input": block_input(put.clone())}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "p", "content": block_receipt(&put)}]}),
        json!({"role": "assistant", "content": calls}),
        json!({"role": "user", "content": results}),
        // Results only, then other tools' calls.
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "late"}]}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "z", "name": "read", "input": {"path": "a"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "z", "content": "a"}]}),
    ]
}

/// What a board's rows must draw, from the reference fold alone (Shady
/// #101 P3-1: the whole of it, not a count): each placed list's blocks in
/// order (their members inside), the view's, each with the bound prefix's
/// of the same incarnation, on the row of the event the list is placed
/// at (the input's own row map); each run's title on its call's row.
type Normal = (Block, Option<Block>, Vec<(Block, Option<Block>)>);
fn reference(i: &Input, events: &[Event]) -> (std::collections::BTreeMap<u64, Vec<Normal>>, std::collections::BTreeMap<u64, Arc<str>>) {
    assert_eq!(i.from, 0, "a fresh read: every event staged");
    let rows: Vec<Option<u64>> = i.chunks.iter().flat_map(|c| c.rows.iter().copied()).chain(i.live_rows()).collect();
    assert_eq!(rows.len(), events.len());
    let board = Board::fold(i.generation, events);
    let (view, bound) = (board.view(events), board.bound(events));
    let of = |r: &Ref| {
        let b = view.block(r).expect("listed: live");
        (b.clone(), bound.block(r).filter(|x| x.tag == b.tag).cloned())
    };
    let mut want = std::collections::BTreeMap::new();
    for (place, refs) in view.lists() {
        let Place::Transcript(pos) = place else { continue };
        let row = rows[pos].expect("a list is placed at an event with a row");
        let list: &mut Vec<Normal> = want.entry(row).or_default();
        for r in refs {
            let (b, bd) = of(r);
            let members = b.members().iter().map(|m| of(&Ref::Block(m.clone()))).collect();
            list.push((b, bd, members));
        }
    }
    let runs = view.runs().filter_map(|run| Some((rows[run.at.event]?, Arc::from(&*run.summary)))).collect();
    (want, runs)
}

/// A board's rows, as [`reference`] normalizes them.
fn normal(d: &Drawn) -> (std::collections::BTreeMap<u64, Vec<Normal>>, std::collections::BTreeMap<u64, Arc<str>>) {
    let n = |s: &Shown| ((*s.block).clone(), s.bound.as_deref().cloned());
    let blocks = d
        .rows
        .iter()
        .filter(|(_, r)| !r.blocks.is_empty())
        .map(|(k, r)| (*k, r.blocks.iter().map(|s| (n(s).0, n(s).1, s.members.iter().map(n).collect())).collect()))
        .collect();
    let runs = d.rows.iter().filter_map(|(k, r)| Some((*k, r.run.clone()?))).collect();
    (blocks, runs)
}

/// D5's traces: at and past a replay's 256-call pass (`cards_spec::board`),
/// with refused block calls, a results-only tail, other tools' calls and a
/// last call still pending, the worker's board is the reference fold's,
/// whole (Shady #101 P3-1): every block's body, revision, order and row,
/// its bound value, the runs' titles; read whole or a slice at a time the
/// same; the plan's step is the last bound patch's in the bound prefix and
/// the pending one's in the view; each call's fate is its batch's.
#[test]
fn traces_at_and_past_a_replays_pass() {
    for n in [255, 256, 257, 600] {
        let mut h = big_batch(n);
        // The last bound patch's state, and the pending one flips it.
        let bound_k = (0..n).rev().find(|k| k % 3 != 0).expect("a bound patch");
        let last = if bound_k % 2 == 0 { "done" } else { "doing" };
        let pending = json!([{"op":"patch","id":"plan","state":{"a": last}}]);
        h.push(json!({"role": "user", "content": "one more"}));
        h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": "q", "name": "cards_block", "input": block_input(pending)}]}));
        // Read by a transcript: its rows, each event's.
        let mut tr = crate::transcript::Transcript::default();
        tr.apply_history(&h);
        let i = tr.blocks_input().expect("blocks");
        let events = i.events();
        let want = Board::fold(i.generation, &events);
        let mut w = Worker::default();
        let mut asked = 0;
        let mut d = None;
        while d.is_none() {
            d = w.run(&i, &mut || {
                asked += 1;
                false
            });
        }
        let d = d.unwrap();
        assert_eq!(d.live, want.view(&events).live(), "{n}");
        assert_eq!(normal(&d), reference(&i, &events), "{n}: the reference fold's, whole");
        assert_eq!(d, Worker::default().run(&i, &mut || true).unwrap(), "{n}: sliced or whole, the same");
        // By hand: the plan's step, as the last bound patch set it, and in
        // the view the pending one's.
        let step = |b: &Block| match &b.body {
            cards_spec::block::Body::Plan(p) => p.steps[0].state.as_str().to_string(),
            _ => panic!("a plan"),
        };
        let plan = d.rows.values().flat_map(|r| &r.blocks).find(|s| s.block.id.to_string() == "plan").expect("the plan drawn");
        assert_eq!(step(plan.bound.as_deref().expect("bound")), if bound_k % 2 == 0 { "doing" } else { "done" }, "{n}");
        assert_eq!(step(&plan.block), last, "{n}: the pending patch, in the view");
        assert!(plan.block.rev > plan.bound.as_ref().unwrap().rev, "{n}: the view ahead of the bound prefix");
        // Fates: every third refused (unbound), the rest bound; the late
        // result after the batch's end binds nothing.
        let fates: Vec<Fate> = all_fates(&events).into_iter().map(|x| x.1).collect();
        let count = |x: Fate| fates.iter().filter(|f| **f == x).count();
        assert_eq!(count(Fate::Unbound), n.div_ceil(3), "{n}");
        assert_eq!(count(Fate::Bound), 1 + n - n.div_ceil(3), "{n}");
        assert_eq!(count(Fate::Pending), 1, "{n}: the last call's");
    }
}

/// A history replaced during a job (another epoch: a clear, a compaction):
/// the job for the old one stops between slices, and the next one starts
/// over on the new history; what it publishes is the new history's board,
/// whole, never a mix.
#[test]
fn a_history_replaced_during_a_job_starts_over() {
    let long: Vec<Value> = blocks_history().into_iter().chain(crate::synthetic::history(20_000, 5)).collect();
    let old = input(&long);
    let mut w = Worker::default();
    assert!(w.run(&old, &mut || false).is_none(), "stopped after its first slice");
    assert_eq!(w.position(), SLICE);
    let new_h = blocks_history()[..6].to_vec();
    let new = input(&new_h);
    assert_ne!(new.epoch, old.epoch);
    let d = w.run(&new, &mut || true).expect("done");
    assert_eq!(d.epoch, new.epoch);
    assert_eq!(d, Worker::default().run(&new, &mut || true).unwrap(), "the new history's board, whole");
    assert_eq!(w.position(), new.history_len);
}

/// The review's owners (Shady #101 P1-2): many refused calls, many
/// incarnations put and removed, a batch of more calls than a pass and a
/// results-only tail. The worker keeps the journal (one copy: the history
/// stages nothing once taken) and B2's board, bounded; committed fates are
/// published once and let go once the rows took them; a publication's rows
/// are the board's and the open batch's, not the history's calls.
#[test]
fn the_worker_keeps_one_journal_and_nothing_per_call() {
    let refused = json!([{"op":"put","id":"x","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"finished"}]}}]);
    let mut h = big_batch(PASS + 44);
    for k in 0..300 {
        let ops = if k % 3 == 0 {
            refused.clone()
        } else {
            json!([{"op":"put","id":format!("b{k}"),"kind":"note","body":{"text":"x"}}, {"op":"remove","id":format!("b{k}")}])
        };
        let id = format!("u{k}");
        h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}]}));
        let text = if k % 3 == 0 { "cards_block: refused".to_string() } else { block_receipt(&ops) };
        h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}));
    }
    // Rows: one per call, as a transcript's.
    let mut hb = HistoryBlocks::default();
    let mut w = Worker::default();
    let mut row = 0u64;
    let mut last = None;
    for (k, m) in h.iter().enumerate() {
        let rows: Vec<Item> = m["content"]
            .as_array()
            .map(|b| b.iter().filter(|b| b["type"] == "tool_use").map(|b| tool_row(&mut row, b["id"].as_str().unwrap())).collect())
            .unwrap_or_default();
        hb.message(m, &rows);
        if (k % 50 == 0 || k + 1 == h.len())
            && let Some(i) = Input::of(&hb, &LiveLog::default())
        {
            let d = w.run(&i, &mut || true).expect("done");
            hb.took(d.committed_to);
            last = Some(d);
        }
    }
    let d = last.unwrap();
    let r = w.retained();
    assert_eq!(r.journal, hb.len(), "one journal");
    // `fold::events` keeps the results-only tail's result; the reader, only
    // the last assistant message's ids (module docs).
    assert_eq!(hb.len() + 1, events(&h).len());
    hb.prune();
    assert_eq!(hb.staged(), 0, "the history keeps no second copy");
    assert!(d.committed.len() <= 400, "{}: the last publication carries what committed since the one before", d.committed.len());
    // Taken: the next publication carries nothing of the history's calls.
    let i = Input::of(&hb, &LiveLog::default()).unwrap();
    let again = w.run(&i, &mut || true).unwrap();
    assert!(again.committed.is_empty(), "{}", again.committed.len());
    assert_eq!(w.retained().committed, 0);
    assert!(again.rows.len() <= 4, "{} rows: the board's", again.rows.len());
    assert!(r.board_bytes <= cards_spec::board::WORKING_SET_MAX_BYTES + 4096, "{}", r.board_bytes);
}

fn tool_row(next: &mut u64, id: &str) -> Item {
    *next += 1;
    let mut t = crate::transcript::Transcript::new();
    t.apply(&crate::protocol::SessionEvent::Stream {
        event: crate::protocol::StreamEvent::Llm(crate::protocol::LlmEvent::ToolUseStart { tool_name: "x".into(), tool_id: id.into() }),
    });
    let mut it = t.items()[0].clone();
    it.id = *next;
    it
}

/// A worker started over on a history whose staged events it let go (its
/// store torn down, the transcript kept) has a gap: nothing drawn, until
/// the history is folded again (another epoch).
#[test]
fn a_gap_draws_nothing_until_another_epoch() {
    let h = blocks_history();
    let mut hb = HistoryBlocks::default();
    for m in &h[..4] {
        hb.message(m, &[]);
    }
    let mut w = Worker::default();
    w.run(&Input::of(&hb, &LiveLog::default()).unwrap(), &mut || true).expect("done");
    for m in &h[4..] {
        hb.message(m, &[]);
    }
    assert!(hb.staged() < hb.len(), "what the worker took is let go");
    let mut fresh = Worker::default();
    assert!(fresh.run(&Input::of(&hb, &LiveLog::default()).unwrap(), &mut || true).is_none());
    assert!(fresh.gap());
    assert!(w.run(&Input::of(&hb, &LiveLog::default()).unwrap(), &mut || true).is_some(), "its own worker reads on");
    let again = read(&h);
    assert!(fresh.run(&Input::of(&again, &LiveLog::default()).unwrap(), &mut || true).is_some());
    assert!(!fresh.gap());
}

/// Shady #101 R7 (hardening): an unwind while a store's real lock is held
/// ([`hold`]) and something is set aside under it (dropped in place: here
/// a bin with no disposer, as refused or closed) still drops it after the
/// lock is let go, never under it, as the stack unwinds ([`Held`]'s
/// fields: the lock, then the guard that drains the spill). It leaves the
/// thread holding no lock and nothing set aside, and the store usable.
#[test]
fn an_unwind_under_a_stores_lock_drops_what_it_set_aside_after_the_lock() {
    use std::sync::atomic::AtomicU8;
    use std::sync::{TryLockError, Weak};
    /// Dropped: 1 with the store's lock free, 2 under it (0: not yet).
    struct Payload(Weak<Mutex<Store>>, Arc<AtomicU8>);
    impl Drop for Payload {
        fn drop(&mut self) {
            let store = self.0.upgrade().expect("the store outlives it");
            let under = matches!(store.try_lock(), Err(TryLockError::WouldBlock));
            self.1.store(if under { 2 } else { 1 }, Ordering::SeqCst);
        }
    }
    let store = Arc::new(Mutex::new(Store::in_bin(dispose::Bin::default())));
    let seen = Arc::new(AtomicU8::new(0));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = hold(&store);
        dispose::Bin::default().retire(Payload(Arc::downgrade(&store), seen.clone()), 1, 64);
        assert_eq!(seen.load(Ordering::SeqCst), 0, "set aside under the lock, not dropped");
        panic!("an unwind, the store's lock and what it set aside live");
    }));
    assert!(unwound.is_err(), "it unwound");
    assert_eq!(seen.load(Ordering::SeqCst), 1, "dropped as the stack unwound, after the lock was let go (2: under it)");
    let again = Arc::new(AtomicU8::new(0));
    dispose::Bin::default().retire(Payload(Arc::downgrade(&store), again.clone()), 1, 64);
    assert_eq!(again.load(Ordering::SeqCst), 1, "no lock left counted on the thread: dropped at once");
    // Poisoned by the unwind, taken anyway.
    drop(hold(&store));
}

/// D6b's visibility maps are counted (Shady #103 R3-P2-7): a history of
/// notices for ids that have no live block holds no block of its own, yet
/// its publication and its worker each hold the records' map (64 records,
/// counted by both); a live turn's own notice makes the publication a
/// second map, counted once more; records equal to the history's are its
/// map, shared.
#[test]
fn the_visibility_maps_are_counted() {
    let put = json!([{"op":"put","id":"x","kind":"note","body":{"text":"x"}}, {"op":"remove","id":"x"}]);
    let history = |notice: bool, n: usize| {
        let mut h = vec![
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t0", "name": "cards_block", "input": block_input(put.clone())}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t0", "content": block_receipt(&put)}]}),
        ];
        for k in 0..n {
            h.push(json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]}));
            // Not a notice: the same bytes in the journal.
            let word = if notice { "block" } else { "blokk" };
            h.push(json!({"role": "user", "content": format!("m\n[{word} i{k:02} v1 dismissed]")}));
        }
        h
    };
    let heap = |h: &[Value]| {
        let i = input(h);
        Board::fold(i.generation, &i.events()).view(&i.events()).owned_by_owner()[3]
    };
    let run = |h: &[Value]| {
        let mut w = Worker::default();
        let d = w.run(&input(h), &mut || true).expect("done");
        (w, d)
    };
    let (_, d0) = run(&history(true, 0));
    let (wp, dp) = run(&history(false, 64));
    let (wn, dn) = run(&history(true, 64));
    let h64 = heap(&history(true, 64));
    assert_eq!((dn.live, dn.settled.len(), dp.settled.len()), (0, 64, 0), "64 records, no live block");
    assert!(h64 >= 64 * std::mem::size_of::<block::BlockId>(), "{h64}");
    assert!(Arc::ptr_eq(&dn.visibility, &dn.settled), "the history's records: its map, shared");
    assert!(dn.bytes >= d0.bytes + h64, "the publication counts the map: {} vs {} + {h64}", dn.bytes, d0.bytes);
    assert!(dp.bytes < dn.bytes);
    let own = |w: &Worker| w.bytes() - w.retained().board_bytes;
    assert!(own(&wn) >= own(&wp) + h64, "the worker counts its own: {} vs {} + {h64}", own(&wn), own(&wp));
    // Exactly its map's heap, read from the map it holds.
    let mut bare = wn;
    let counted = bare.bytes();
    assert_eq!(bare.settled_heap, h64, "its map's heap, read");
    bare.settled_heap = 0;
    assert_eq!(counted - bare.bytes(), h64, "the worker counts its map");
    // A live turn's notice: a map of its own, the history's kept.
    let hb = read(&history(true, 64));
    let mut log = LiveLog::default();
    log.user(&hb, "go\n[block z v1 dismissed]", None);
    let mut w = Worker::default();
    let dl = w.run(&Input::of(&hb, &log).expect("blocks"), &mut || true).expect("done");
    // (At the cap, 64: the live one evicts the oldest.)
    let z = block::BlockId::parse("z").expect("an id");
    assert!(!Arc::ptr_eq(&dl.visibility, &dl.settled) && dl.visibility.dismissed(&z) && !dl.settled.dismissed(&z));
    assert!(dl.bytes >= dn.bytes + h64, "both maps: {} vs {} + {h64}", dl.bytes, dn.bytes);
}

/// One block call of `ops` as the history holds it: its message, then
/// its result (a receipt).
fn call_of(id: &str, ops: Value) -> [Value; 2] {
    [
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": block_receipt(&ops)}]}),
    ]
}

/// History `h`'s board, a tool row each call (rows 1, 2, … in order).
fn drawn_with_rows(h: &[Value]) -> Drawn {
    let mut hb = HistoryBlocks::default();
    let mut row = 0u64;
    for m in h {
        let rows: Vec<Item> = m["content"]
            .as_array()
            .map(|b| b.iter().filter(|b| b["type"] == "tool_use").map(|b| tool_row(&mut row, b["id"].as_str().unwrap())).collect())
            .unwrap_or_default();
        hb.message(m, &rows);
    }
    Worker::default().run(&Input::of(&hb, &LiveLog::default()).unwrap(), &mut || true).expect("done")
}

/// Shady #105 R2-1: a board's marks keep a call's id once, one allocation
/// shared by its lines and records whatever its ops; an id past
/// [`CALL_ID_MAX`] is not kept (its time unknown, its row kept); and a
/// board keeps [`CALL_IDS_MAX`] bytes of ids at most, the newest lines'
/// first, then the newest records', counted in its bytes.
#[test]
fn a_boards_marks_share_their_calls_ids_within_a_budget() {
    let marks = |d: &Drawn| d.log.iter().map(|(_, m)| m.clone()).chain(d.records.iter().map(|(_, m)| m.clone())).collect::<Vec<Mark>>();
    // One call of 16 ops, its id as long as is kept; one of 8, longer.
    let kept = format!("toolu_{}", "k".repeat(CALL_ID_MAX - 6));
    let over = format!("toolu_{}", "o".repeat(CALL_ID_MAX - 5));
    let puts = |p: &str, n: usize| {
        Value::Array((0..n).map(|k| json!({"op":"put","id":format!("{p}{k}"),"kind":"note","body":{"text":"x"}})).collect())
    };
    let mut h = vec![json!({"role": "user", "content": "go"})];
    h.extend(call_of(&kept, puts("a", 16)));
    h.extend(call_of(&over, puts("b", 8)));
    let d = drawn_with_rows(&h);
    let all = marks(&d);
    let first: Vec<&Mark> = all.iter().filter(|m| m.row == Some(1)).collect();
    let second: Vec<&Mark> = all.iter().filter(|m| m.row == Some(2)).collect();
    assert_eq!((first.len(), second.len()), (32, 16), "a line and a record an op");
    let one = first[0].call.clone().expect("its id kept");
    assert_eq!(&*one, kept.as_str());
    let shared = first.iter().filter(|m| m.call.as_ref().is_some_and(|c| Arc::ptr_eq(c, &one))).count();
    assert_eq!(shared, 32, "one allocation for the call's 32 marks");
    assert!(second.iter().all(|m| m.call.is_none()), "an id past {CALL_ID_MAX} bytes: not kept");
    // Six hundred calls, ids 120 bytes long, a revision each (of twenty
    // plans, within B2's records a block): past the budget.
    let id = |k: usize| format!("toolu_{k:0>114}");
    let mut h = vec![json!({"role": "user", "content": "go"})];
    for half in [0..10, 10..20] {
        let plans =
            half.map(|j| json!({"op":"put","id":format!("p{j}"),"kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"todo"}]}}));
        h.extend(call_of("toolu_plans", Value::Array(plans.collect())));
    }
    for k in 0..600 {
        let state = if (k / 20) % 2 == 0 { "doing" } else { "done" };
        h.extend(call_of(&id(k), json!([{"op":"patch","id":format!("p{}", k % 20),"state":{"a": state}}])));
    }
    let d = drawn_with_rows(&h);
    assert!(d.records.len() >= 600 && d.log.len() == cards_spec::board::LOG_MAX, "{} records, {} lines", d.records.len(), d.log.len());
    let mut seen = HashSet::new();
    let ids: usize =
        marks(&d).iter().filter_map(|m| m.call.as_ref()).filter(|c| seen.insert(Arc::as_ptr(c).cast::<u8>())).map(|c| c.len() + ARC).sum();
    assert!(ids <= CALL_IDS_MAX, "{ids} bytes of ids kept");
    assert!(d.log.iter().all(|(_, m)| m.call.is_some()), "every line keeps its call's id: the newest first");
    let without = d.records.iter().position(|(_, m)| m.call.is_none()).expect("a record past the budget");
    assert!(d.records[without].1.row.is_some(), "its row kept");
    assert!(d.records.iter().rev().take(256).all(|(_, m)| m.call.is_some()), "the newest records keep theirs");
}
