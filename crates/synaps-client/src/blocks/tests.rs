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
