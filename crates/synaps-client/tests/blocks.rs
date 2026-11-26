//! Blocks in the transcript (synaps-cards `docs/blocks.md` v1.3, §1.7, §5.1,
//! D5's tests): the board's input is the same whichever way the history
//! came (an attach, appends, any split of it, the live turn), so the three
//! readings agree exactly: the live overlay at the turn's end, the history
//! once it lands, and a reattach of any split. And what each row draws:
//! a bound block call no tool row, an unbound one a plain row, blocks at
//! their call's row (a converted card's at its card's), a run's title.

use std::collections::BTreeMap;
use std::sync::Arc;

use cards_spec::fold::events;
use serde_json::{Value, json};
use synaps_client::blocks::{Drawn, Fate, RowBlocks, Worker};
use synaps_client::history::HistoryUpdate;
use synaps_client::protocol::*;
use synaps_client::synthetic::{BLOCKS_CARD, block_input, block_receipt, blocks_history};
use synaps_client::transcript::{ItemKind, Transcript};

fn ev(v: Value) -> SessionEvent {
    decode_event(&v).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn llm(v: Value) -> SessionEvent {
    ev(json!({"ev": "stream", "event": v}))
}

fn attached(messages: &[Value], streaming: bool) -> Attached {
    let mut a: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/tests/fixtures/attached_history.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    a["conversation"]["api_messages"] = json!(messages);
    a["conversation"]["messages_len"] = json!(messages.len());
    a["conversation"]["pending_events_len"] = json!(0);
    a["replay"] = json!([]);
    a["streaming"] = json!(streaming);
    let DaemonFrame::Attached(a) = decode_frame(&a.to_string()).unwrap() else { panic!() };
    *a
}

fn attach(h: &[Value]) -> Transcript {
    let mut t = Transcript::new();
    t.load_attached(&attached(h, false)).unwrap();
    t
}

/// `messages` as the live stream sends them: a user message that is text
/// starts a turn; an assistant message is a response (its text, its calls
/// whole); a message of results reports them. `idle`: the turn ends.
fn stream(t: &mut Transcript, messages: &[Value], idle: bool) {
    let mut busy = false;
    for m in messages {
        let blocks = m["content"].as_array().cloned().unwrap_or_default();
        match m["role"].as_str() {
            Some("user") if blocks.iter().any(|b| b["type"] == "tool_result") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    let text = b["content"].as_str().map(str::to_string).unwrap_or_default();
                    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": b["tool_use_id"], "result": text})));
                }
            }
            Some("user") => {
                if busy {
                    t.apply(&ev(json!({"ev": "idle"})));
                }
                busy = true;
                let text = m["content"].as_str().unwrap_or_default();
                t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": text})));
            }
            Some("assistant") => {
                t.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
                for b in &blocks {
                    match b["type"].as_str() {
                        Some("text") => t.apply(&llm(json!({"kind": "llm", "llm": "text", "text": b["text"]}))),
                        Some("tool_use") => {
                            t.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": b["name"], "tool_id": b["id"]})));
                            t.apply(&llm(
                                json!({"kind": "llm", "llm": "tool_use", "tool_name": b["name"], "tool_id": b["id"], "input": b["input"]}),
                            ));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if idle && busy {
        t.apply(&ev(json!({"ev": "idle"})));
    }
}

/// `t`'s board, folded (as the desk's worker folds it, off the frame).
fn fold(t: &Transcript) -> Arc<Drawn> {
    let input = t.blocks_input().expect("blocks");
    Arc::new(Worker::default().run(&input, &mut || true).expect("done"))
}

/// What `t`'s rows draw, by their call's tool id (rows are numbered per
/// transcript, tool ids are the history's).
fn drawn(t: &mut Transcript) -> BTreeMap<String, RowBlocks> {
    let d = fold(t);
    t.set_drawn(Some(d));
    t.items().iter().filter_map(|i| Some((i.tool.as_ref()?.tool_id.clone(), (**i.blocks.as_ref()?).clone()))).collect()
}

/// The board's input is the history's events, whichever way it came: an
/// attach of it whole, an attach of a part and appends of the rest, the
/// rest streamed live (results, `ResultsEnd`, prompts as the history will
/// hold them).
#[test]
fn the_input_is_the_historys_events_however_it_came() {
    let h = blocks_history();
    let whole = events(&h);
    assert_eq!(attach(&h).blocks_input().unwrap().events(), whole);
    for k in 1..h.len() {
        let mut t = attach(&h[..k]);
        t.apply_update(&HistoryUpdate::Append { base_len: k, messages: h[k..].to_vec() }).unwrap();
        assert_eq!(t.blocks_input().unwrap().events(), whole, "appended at {k}");
    }
    // Live from each turn's start (a user message of text).
    for k in (1..h.len()).filter(|&k| h[k]["content"].is_string()) {
        let mut t = attach(&h[..k]);
        stream(&mut t, &h[k..], false);
        assert_eq!(t.blocks_input().unwrap().events(), whole, "live from {k}");
    }
}

/// D5's three-way equivalence (§1.7): the live overlay at the turn's end,
/// the history once it lands, and a reattach's fold of any split of it
/// draw the same blocks on the same calls, exactly.
#[test]
fn live_history_and_any_split_draw_the_same() {
    let h = blocks_history();
    let want = drawn(&mut attach(&h));
    assert!(want.values().any(|r| !r.blocks.is_empty()));
    for k in 1..h.len() {
        let mut t = attach(&h[..k]);
        t.apply_update(&HistoryUpdate::Append { base_len: k, messages: h[k..].to_vec() }).unwrap();
        assert_eq!(drawn(&mut t), want, "a split at {k}");
    }
    for k in (1..h.len()).filter(|&k| h[k]["content"].is_string()) {
        let mut t = attach(&h[..k]);
        stream(&mut t, &h[k..], false);
        assert_eq!(drawn(&mut t), want, "live from {k}");
        // The history lands: the same, and no row lost its blocks between.
        t.apply_update(&HistoryUpdate::Replace { messages: h.clone() }).unwrap();
        let carried: BTreeMap<String, RowBlocks> =
            t.items().iter().filter_map(|i| Some((i.tool.as_ref()?.tool_id.clone(), (**i.blocks.as_ref()?).clone()))).collect();
        assert_eq!(carried, want, "landed from {k}, before the next board");
        assert_eq!(drawn(&mut t), want, "landed from {k}");
    }
}

/// What the rows draw of the demo history: a bound call no tool row of its
/// own, a refused one a plain row; the blocks at the call that placed them
/// (a merge's group at its first member's place, a converted card at its
/// card's row); the run a `collapse` titled.
#[test]
fn rows_draw_the_board() {
    let d = drawn(&mut attach(&blocks_history()));
    let call = |id: &str| d.get(id).and_then(|r| r.call);
    assert_eq!(call("toolu_b01"), Some(Fate::Bound));
    assert_eq!(call("toolu_b08"), Some(Fate::Unbound), "refused: a plain row");
    assert!(!d.contains_key("toolu_r01"), "another tool's row draws nothing of the board");
    let ids = |id: &str| d.get(id).map(|r| r.blocks.iter().map(|s| s.block.id.to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert_eq!(ids("toolu_b01"), ["now", "plan"]);
    assert_eq!(ids("toolu_b03"), ["edits", "file"]);
    assert_eq!(ids("toolu_b04"), ["why", "retry"], "the group sits at its first member's place");
    assert_eq!(ids("toolu_c01"), [format!("card:{BLOCKS_CARD}")], "a converted card draws at its card");
    let why = &d["toolu_b04"].blocks[0];
    assert_eq!(why.members.iter().map(|m| m.block.id.to_string()).collect::<Vec<_>>(), ["cause", "rot"]);
    assert_eq!(d["toolu_b02"].run.as_deref(), Some("Explored the relay's refresh path"));
    let now = &d["toolu_b01"].blocks[0].block;
    assert!(now.replaced.is_some() && now.rev == 1, "r1 · replaced");
    let plan = &d["toolu_b01"].blocks[1].block;
    assert!(plan.skipped.is_some(), "the skipped chip");
    assert!(d["toolu_b03"].blocks[0].block.collapsed, "the agent folded the edits");
}

/// Live: a block call whose result hasn't come draws its ops pending (its
/// bound board has none of them), and no tool row; its result binds it.
#[test]
fn a_live_call_is_pending_until_its_result_binds() {
    let h = blocks_history();
    let mut t = attach(&h);
    let put = json!([{"op":"put","id":"live","kind":"status","body":{"text":"Bisecting","state":"working"}}]);
    let next = vec![
        json!({"role": "user", "content": "go on"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_l1", "name": "cards_block", "input": block_input(put.clone())},
            {"type": "tool_use", "id": "toolu_l2", "name": "bash", "input": {"command": "sleep 9"}}]}),
    ];
    stream(&mut t, &next, false);
    let d = drawn(&mut t);
    assert_eq!(d["toolu_l1"].call, Some(Fate::Pending));
    let live = &d["toolu_l1"].blocks[0];
    assert!(live.block.pending && live.bound.is_none());
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_l1", "result": block_receipt(&put)})));
    let d = drawn(&mut t);
    assert_eq!(d["toolu_l1"].call, Some(Fate::Bound));
    let live = &d["toolu_l1"].blocks[0];
    assert!(!live.block.pending && live.bound.as_deref() == Some(&*live.block), "bound while the bash call still runs");
}

/// A response whose first call reports before the model has finished the
/// next call's input: the board reads the round's calls first, as the
/// history will hold them, not the order the events came.
#[test]
fn a_rounds_calls_come_before_its_results() {
    let mut t = attach(&blocks_history());
    let a = json!([{"op":"put","id":"a","kind":"note","body":{"text":"a"}}]);
    let b = json!([{"op":"put","id":"b","kind":"note","body":{"text":"b"}}]);
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "more"})));
    t.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    for (id, ops) in [("toolu_x1", &a), ("toolu_x2", &b)] {
        t.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})));
        t.apply(&llm(
            json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_block", "tool_id": id, "input": block_input(ops.clone())}),
        ));
        if id == "toolu_x1" {
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": block_receipt(&a)})));
        }
    }
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_x2", "result": block_receipt(&b)})));
    let mut h = blocks_history();
    h.push(json!({"role": "user", "content": "more"}));
    h.push(json!({"role": "assistant", "content": [
        {"type": "tool_use", "id": "toolu_x1", "name": "cards_block", "input": block_input(a.clone())},
        {"type": "tool_use", "id": "toolu_x2", "name": "cards_block", "input": block_input(b.clone())}]}));
    h.push(json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_x1", "content": block_receipt(&a)},
        {"type": "tool_result", "tool_use_id": "toolu_x2", "content": block_receipt(&b)}]}));
    assert_eq!(t.blocks_input().unwrap().events(), events(&h));
    let d = drawn(&mut t);
    assert_eq!(d["toolu_x1"].blocks[0].block.id.to_string(), "a");
    assert_eq!(d["toolu_x2"].blocks[0].block.id.to_string(), "b");
    let silent = |id: &str| {
        t.items().iter().any(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == id) && i.blocks.as_ref().is_some_and(|b| b.silent()))
    };
    assert!(silent("toolu_x1") && silent("toolu_x2"), "bound: no tool rows of their own");
    assert!(t.items().iter().filter(|i| i.kind == ItemKind::Tool).count() > 2);
}

/// A session with no block call has no board: nothing to fold, and its
/// input's revision stays put through a turn.
#[test]
fn no_block_call_no_board() {
    let h = synaps_client::synthetic::history(30, 5);
    let mut t = attach(&h);
    assert!(t.blocks_input().is_none());
    let rev = t.blocks_rev();
    stream(
        &mut t,
        &[json!({"role": "user", "content": "hi"}), json!({"role": "assistant", "content": [{"type": "text", "text": "yo"}]})],
        true,
    );
    assert!(t.blocks_input().is_none());
    assert_eq!(t.blocks_rev(), rev);
}
