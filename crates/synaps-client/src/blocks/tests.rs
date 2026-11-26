//! `blocks`: the history read a message at a time is `fold::events` of the
//! whole; block calls' fates; the worker's slices, stops and epochs.

use cards_spec::board::Board;
use cards_spec::fold::events;
use serde_json::{Value, json};

use super::*;
use crate::synthetic::{block_input, block_receipt, blocks_history};

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
    let mut f = Fates::default();
    for at in 0..events.len() {
        f.read(&events, at);
    }
    let mut got: Vec<(usize, Fate)> = f.done.iter().map(|(&p, &f)| (p, f)).collect();
    got.extend(f.fates(&events, true));
    got.sort_by_key(|g| g.0);
    use Fate::*;
    assert_eq!(got, [(0, Bound), (1, Unbound), (2, Unbound), (4, Unbound), (9, Bound), (10, Pending)]);
}

fn input(history: &[Value], live: Live) -> Input {
    Input::of(&read(history), Arc::new(live)).expect("blocks")
}

/// The worker's board is the history's fold, read on from where it stopped
/// while the history grows, in slices it may stop between; a stopped job
/// keeps what it read, and the next finishes it; another epoch starts over.
#[test]
fn the_worker_reads_on_stops_between_slices_and_starts_over_on_another_epoch() {
    let h = blocks_history();
    let whole = input(&h, Live::default());
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
        let i = Input::of(&hb, Arc::new(Live::default()));
        if let Some(i) = i {
            w2.run(&i, &mut || true).expect("done");
        }
    }
    assert_eq!(w2.run(&Input::of(&hb, Arc::new(Live::default())).unwrap(), &mut || true).unwrap().live, one.live);
    // Long: stopped after its first slice, then finished.
    let long: Vec<Value> = h.iter().cloned().chain(crate::synthetic::history(20_000, 3)).collect();
    let big = input(&long, Live::default());
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
    let other = input(&h[..6], Live::default());
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

/// D5's traces: at and past a replay's 256-call pass (`cards_spec::board`),
/// with refused block calls, a results-only tail and other tools' calls,
/// the worker's board is the reference fold's, read whole or a slice at a
/// time, and each call's fate is its batch's.
#[test]
fn traces_at_and_past_a_replays_pass() {
    for n in [255, 256, 257, 600] {
        let h = big_batch(n);
        let i = input(&h, Live::default());
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
        assert_eq!(d, Worker::default().run(&i, &mut || true).unwrap(), "{n}: sliced or whole, the same");
        // Fates: every third refused (unbound), the rest bound; the late
        // result after the batch's end binds nothing.
        let mut f = Fates::default();
        for at in 0..events.len() {
            f.read(&events, at);
        }
        let mut fates: Vec<Fate> = f.done.values().copied().collect();
        fates.extend(f.fates(&events, true).into_iter().map(|x| x.1));
        let count = |x: Fate| fates.iter().filter(|f| **f == x).count();
        assert_eq!(count(Fate::Unbound), n.div_ceil(3), "{n}");
        assert_eq!(count(Fate::Bound), 1 + n - n.div_ceil(3), "{n}");
        assert_eq!(count(Fate::Pending), 0, "{n}");
    }
}

/// A history replaced during a job (another epoch: a clear, a compaction):
/// the job for the old one stops between slices, and the next one starts
/// over on the new history; what it publishes is the new history's board,
/// whole, never a mix.
#[test]
fn a_history_replaced_during_a_job_starts_over() {
    let long: Vec<Value> = blocks_history().into_iter().chain(crate::synthetic::history(20_000, 5)).collect();
    let old = input(&long, Live::default());
    let mut w = Worker::default();
    assert!(w.run(&old, &mut || false).is_none(), "stopped after its first slice");
    assert_eq!(w.position(), SLICE);
    let new_h = blocks_history()[..6].to_vec();
    let new = input(&new_h, Live::default());
    assert_ne!(new.epoch, old.epoch);
    let d = w.run(&new, &mut || true).expect("done");
    assert_eq!(d.epoch, new.epoch);
    assert_eq!(d, Worker::default().run(&new, &mut || true).unwrap(), "the new history's board, whole");
    assert_eq!(w.position(), new.history_len);
}
