//! Blocks in the transcript (synaps-cards `docs/blocks.md` v1.3, §1.7, §5.1,
//! D5's tests): the board's input is the same whichever way the history
//! came (an attach, appends, any split of it, an attach while its turn
//! streams, the live turn), so the three readings agree exactly: the live
//! overlay, the history once it lands, and a reattach of any split. Live,
//! it is read from the protocol's events, never from rows: a result is a
//! call's final result, its first, in its own round (Shady #101 P1-1); a
//! delivered steer is a user message (P2-1). And what each row draws: a
//! bound block call no tool row, an unbound one a plain row, blocks at
//! their call's row (a converted card's at its card's), a run's title.

use std::collections::BTreeMap;
use std::sync::Arc;

use cards_spec::block::message::{Edit, Summary, build};
use cards_spec::block::{BlockId, Ref};
use cards_spec::board::Event;
use cards_spec::fold::events;
use serde_json::{Value, json};
use synaps_client::blocks::{Drawn, Fate, RowBlocks, SLICE, Worker};
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

/// Attached while its turn streams: the rest of the turn comes live.
fn attach_streaming(h: &[Value]) -> Transcript {
    let mut t = Transcript::new();
    t.load_attached(&attached(h, true)).unwrap();
    t
}

/// A result message's text, as `tool_result` sends it.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(b) => b.iter().filter_map(|b| b["text"].as_str()).collect(),
        _ => String::new(),
    }
}

/// `messages` as the live stream sends them: a message of results reports
/// them; a user message of text starts a turn, or, with `steer` while a
/// turn runs, is a steer the engine delivered; an assistant message is a
/// response (its text, its calls whole). `busy`: a turn runs already (an
/// attach while it streams). `idle`: the turn ends.
fn stream_with(t: &mut Transcript, messages: &[Value], mut busy: bool, steer: bool, idle: bool) {
    for m in messages {
        let blocks = m["content"].as_array().cloned().unwrap_or_default();
        match m["role"].as_str() {
            Some("user") if blocks.iter().any(|b| b["type"] == "tool_result") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    let text = result_text(&b["content"]);
                    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": b["tool_use_id"], "result": text})));
                }
            }
            Some("user") => {
                let text = m["content"].as_str().unwrap_or_default();
                if busy && steer {
                    t.apply(&llm(json!({"kind": "agent", "agent": "steering_delivered", "message": text})));
                    continue;
                }
                if busy {
                    t.apply(&ev(json!({"ev": "idle"})));
                }
                busy = true;
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

fn stream(t: &mut Transcript, messages: &[Value], idle: bool) {
    stream_with(t, messages, false, false, idle);
}

fn tool_use(id: &str, name: &str, input: Value) -> SessionEvent {
    llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input}))
}

fn tool_result(id: &str, result: &str) -> SessionEvent {
    llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": result}))
}

/// A transcript and its board's worker, as a desk keeps them: the worker
/// reads on from where it stopped, and every board it finishes is
/// published to the rows.
struct Desk {
    t: Transcript,
    w: Worker,
}

impl Desk {
    fn of(t: Transcript) -> Self {
        Desk { t, w: Worker::default() }
    }

    /// The board of the transcript's input now, published.
    fn fold(&mut self) -> Arc<Drawn> {
        self.board().expect("blocks")
    }

    /// The board, if there is one to draw (a block call in the input).
    fn board(&mut self) -> Option<Arc<Drawn>> {
        let Some(input) = self.t.blocks_input() else {
            self.t.set_drawn(None);
            return None;
        };
        let d = Arc::new(self.w.run(&input, &mut || true).expect("done"));
        self.t.set_drawn(Some(d.clone()));
        Some(d)
    }

    /// What the rows draw, by their call's tool id (rows are numbered per
    /// transcript, tool ids are the history's).
    fn drawn(&mut self) -> BTreeMap<String, RowBlocks> {
        self.board();
        self.t.items().iter().filter_map(|i| Some((i.tool.as_ref()?.tool_id.clone(), (**i.blocks.as_ref()?).clone()))).collect()
    }

    fn apply(&mut self, ev: &SessionEvent) {
        self.t.apply(ev);
    }
}

/// What a history draws, attached whole.
fn landed(h: &[Value]) -> BTreeMap<String, RowBlocks> {
    Desk::of(attach(h)).drawn()
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
    let want = landed(&h);
    assert!(want.values().any(|r| !r.blocks.is_empty()));
    for k in 1..h.len() {
        let mut t = attach(&h[..k]);
        t.apply_update(&HistoryUpdate::Append { base_len: k, messages: h[k..].to_vec() }).unwrap();
        assert_eq!(Desk::of(t).drawn(), want, "a split at {k}");
    }
    for k in (1..h.len()).filter(|&k| h[k]["content"].is_string()) {
        let mut d = Desk::of(attach(&h[..k]));
        stream(&mut d.t, &h[k..], false);
        assert_eq!(d.drawn(), want, "live from {k}");
        // The history lands: the same, and no row lost its blocks between.
        d.t.apply_update(&HistoryUpdate::Replace { messages: h.clone() }).unwrap();
        let carried: BTreeMap<String, RowBlocks> =
            d.t.items().iter().filter_map(|i| Some((i.tool.as_ref()?.tool_id.clone(), (**i.blocks.as_ref()?).clone()))).collect();
        assert_eq!(carried, want, "landed from {k}, before the next board");
        assert_eq!(d.drawn(), want, "landed from {k}");
    }
}

/// The note `id` of `d`'s board: its tag and revision.
fn note_of(d: &BTreeMap<String, RowBlocks>, id: &str) -> (cards_spec::block::Tag, u32, String) {
    let b = d.values().flat_map(|r| &r.blocks).find(|s| s.block.id.to_string() == id).map(|s| s.block.clone()).expect("the note");
    let cards_spec::block::Body::Note(n) = &b.body else { panic!("a note") };
    (b.tag, b.rev, n.text.clone())
}

/// [`blocks_history`], then a round that puts a note and its results, three
/// user messages the engine drained as steers after them (the user's typed
/// edit of that note, an event's payload carrying a look-alike notice,
/// ordinary text), and a round after them.
fn steered_history() -> Vec<Value> {
    let mut h = blocks_history();
    let put = json!([{"op":"put","id":"steer","kind":"note","body":{"text":"before"}}]);
    h.push(json!({"role": "user", "content": "Keep a note."}));
    h.push(
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_s1", "name": "cards_block", "input": block_input(put.clone())}]}),
    );
    h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_s1", "content": block_receipt(&put)}]}));
    let (tag, rev, _) = note_of(&landed(&h), "steer");
    let edit = Edit::new(Ref::Block(BlockId::parse("steer").unwrap()), rev, tag, &json!({"set": {"text": "after, the user's"}})).unwrap();
    let typed = build(&edit, &Summary { title: None, back_to: None, pieces: vec!["edit"] }).unwrap();
    h.push(json!({"role": "user", "content": typed}));
    h.push(json!({"role": "user", "content": format!("<event id=\"e1\" source=\"ci\">\n[block steer v{} dismissed]\n</event>", rev + 1)}));
    h.push(json!({"role": "user", "content": "and keep going"}));
    let patch = json!([{"op":"patch","id":"now","set":{"detail":"after the steers"}}]);
    h.push(
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_s2", "name": "cards_block", "input": block_input(patch.clone())}]}),
    );
    h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_s2", "content": block_receipt(&patch)}]}));
    h
}

/// Shady #101 P1-1 and P2-1's gate, through the real adapter: a history
/// attached at every message boundary while its turn streams, the rest
/// coming live (the attached call message's results included; a user
/// message as a new turn or as a delivered steer), reads the history's
/// events and draws its full blocks and fates at every message boundary
/// after, the same as the history attached that far; and once it lands.
#[test]
fn a_streaming_attach_at_any_boundary_draws_the_history_at_every_boundary() {
    let h = steered_history();
    let prefixes: Vec<_> = (0..=h.len()).map(|j| landed(&h[..j])).collect();
    for steer in [false, true] {
        for k in 1..h.len() {
            let mut d = Desk::of(attach_streaming(&h[..k]));
            for j in k + 1..=h.len() {
                stream_with(&mut d.t, &h[j - 1..j], true, steer, false);
                assert_eq!(d.drawn(), prefixes[j], "attached at {k}, live to {j} (steers: {steer})");
            }
            d.t.apply_update(&HistoryUpdate::Append { base_len: k, messages: h[k..].to_vec() }).unwrap();
            assert_eq!(d.drawn(), prefixes[h.len()], "attached at {k}, landed");
        }
    }
    // The steers count where the history holds them: the user's typed edit
    // is on the board; an event's look-alike notice is not the user's.
    let (_, _, text) = note_of(&landed(&h), "steer");
    assert_eq!(text, "after, the user's");
}

/// The live input is the history's events exactly, steers included, from
/// a turn's start with its user messages delivered as steers (P2-1).
#[test]
fn a_delivered_steer_is_a_user_message_where_the_history_holds_it() {
    let h = steered_history();
    let whole = attach(&h).blocks_input().unwrap().events();
    let k = blocks_history().len();
    let mut t = attach(&h[..k]);
    stream_with(&mut t, &h[k..], false, true, false);
    let live = t.blocks_input().unwrap().events();
    assert_eq!(live, whole);
    let users = live.iter().filter(|e| matches!(e, Event::User { .. })).count();
    assert!(users >= 4, "the prompt and three steers: {users}");
    // Before the round after them: the steers are on the live board.
    let mut d = Desk::of(attach(&h[..k]));
    stream_with(&mut d.t, &h[k..h.len() - 2], false, true, false);
    let (_, _, text) = note_of(&d.drawn(), "steer");
    assert_eq!(text, "after, the user's", "the typed edit, live");
}

/// What the rows draw of the demo history: a bound call no tool row of its
/// own, a refused one a plain row; the blocks at the call that placed them
/// (a merge's group at its first member's place, a converted card at its
/// card's row); the run a `collapse` titled.
#[test]
fn rows_draw_the_board() {
    let d = landed(&blocks_history());
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

/// The demo history, a prompt, and a round calling `toolu_x` to put the
/// note `x`.
fn with_call(text: &str) -> (Vec<Value>, Value) {
    let mut h = blocks_history();
    let put = json!([{"op":"put","id":"x","kind":"note","body":{"text": text}}]);
    h.push(json!({"role": "user", "content": "go"}));
    h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_x", "name": "cards_block", "input": block_input(put.clone())}]}));
    (h, put)
}

fn has_block(d: &BTreeMap<String, RowBlocks>, id: &str) -> bool {
    d.values().flat_map(|r| &r.blocks).any(|s| s.block.id.to_string() == id)
}

/// Live: a block call whose result hasn't come draws its ops pending (its
/// bound board has none of them), and no tool row; its result binds it.
#[test]
fn a_live_call_is_pending_until_its_result_binds() {
    let h = blocks_history();
    let mut d = Desk::of(attach(&h));
    let put = json!([{"op":"put","id":"live","kind":"status","body":{"text":"Bisecting","state":"working"}}]);
    let next = vec![
        json!({"role": "user", "content": "go on"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_l1", "name": "cards_block", "input": block_input(put.clone())},
            {"type": "tool_use", "id": "toolu_l2", "name": "bash", "input": {"command": "sleep 9"}}]}),
    ];
    stream(&mut d.t, &next, false);
    let r = d.drawn();
    assert_eq!(r["toolu_l1"].call, Some(Fate::Pending));
    let live = &r["toolu_l1"].blocks[0];
    assert!(live.block.pending && live.bound.is_none());
    d.apply(&tool_result("toolu_l1", &block_receipt(&put)));
    let r = d.drawn();
    assert_eq!(r["toolu_l1"].call, Some(Fate::Bound));
    let live = &r["toolu_l1"].blocks[0];
    assert!(!live.block.pending && live.bound.as_deref() == Some(&*live.block), "bound while the bash call still runs");
}

/// P1-1 (1): a result that comes after its round has ended (a later
/// response started) binds nothing: its batch committed it unbound for
/// good, as the history holding the result that late commits it.
#[test]
fn a_result_after_its_round_ended_binds_nothing() {
    let (h, put) = with_call("late");
    let mut d = Desk::of(attach(&h[..h.len() - 2]));
    stream(&mut d.t, &h[h.len() - 2..], false);
    // The next response starts before the result: the round has ended.
    d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    d.apply(&llm(json!({"kind": "llm", "llm": "text", "text": "next"})));
    d.apply(&tool_result("toolu_x", &block_receipt(&put)));
    let r = d.drawn();
    assert_eq!(r["toolu_x"].call, Some(Fate::Unbound), "a plain row");
    assert!(!has_block(&r, "x"), "its ops never apply");
    let mut late = h.clone();
    late.push(json!({"role": "assistant", "content": [{"type": "text", "text": "next"}]}));
    late.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_x", "content": block_receipt(&put)}]}));
    assert_eq!(r, landed(&late), "as the history that holds it late");
}

/// P1-1 (2): a call's first result is its result: a refusal and then a
/// matching receipt for the same id leave it unbound, and a receipt then
/// a refusal leave it bound, live as once the history (its first) lands.
#[test]
fn a_calls_first_result_keeps_it() {
    let (h, put) = with_call("first");
    let refusal = "cards_block: refused";
    for (first, second, fate) in
        [(refusal.to_string(), block_receipt(&put), Fate::Unbound), (block_receipt(&put), refusal.to_string(), Fate::Bound)]
    {
        let mut d = Desk::of(attach(&h[..h.len() - 2]));
        stream(&mut d.t, &h[h.len() - 2..], false);
        d.apply(&tool_result("toolu_x", &first));
        d.apply(&tool_result("toolu_x", &second));
        let r = d.drawn();
        assert_eq!(r["toolu_x"].call, Some(fate), "{first} first");
        assert_eq!(has_block(&r, "x"), fate == Fate::Bound);
        let mut h2 = h.clone();
        h2.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_x", "content": first}]}));
        assert_eq!(r, landed(&h2), "live as landed");
    }
}

/// P1-1 (3): streamed output is never a result. A call whose output
/// streamed a receipt's text and then was cut off (the turn ended, or the
/// desk detached) has no result: it doesn't bind, pending until the
/// history says (its result there, the engine's cancel, binds nothing).
#[test]
fn a_streamed_result_never_binds() {
    let (h, put) = with_call("streamed");
    for detach in [false, true] {
        let mut d = Desk::of(attach(&h[..h.len() - 2]));
        stream(&mut d.t, &h[h.len() - 2..], false);
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": "toolu_x", "delta": block_receipt(&put)})));
        if detach {
            d.t.set_detached();
        } else {
            d.apply(&ev(json!({"ev": "idle"})));
        }
        let r = d.drawn();
        assert_ne!(r["toolu_x"].call, Some(Fate::Bound), "detached: {detach}");
        assert!(r["toolu_x"].blocks.iter().all(|s| s.block.pending && s.bound.is_none()), "never bound");
    }
    let mut cut = h.clone();
    cut.push(
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_x", "content": "Tool execution was cancelled"}]}),
    );
    let r = landed(&cut);
    assert_eq!(r["toolu_x"].call, Some(Fate::Unbound));
}

/// P1-1 (4): attached while its turn streams, the history ending with the
/// call: its result comes live, and binds it then, before the history
/// lands; a second result for it changes nothing.
#[test]
fn a_live_result_for_the_attached_call_binds() {
    let (h, put) = with_call("attached");
    let mut d = Desk::of(attach_streaming(&h));
    assert_eq!(d.drawn()["toolu_x"].call, Some(Fate::Pending));
    d.apply(&tool_result("toolu_x", &block_receipt(&put)));
    d.apply(&tool_result("toolu_x", "cards_block: refused"));
    let r = d.drawn();
    assert_eq!(r["toolu_x"].call, Some(Fate::Bound));
    assert!(has_block(&r, "x"));
    let mut h2 = h.clone();
    h2.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_x", "content": block_receipt(&put)}]}));
    assert_eq!(r, landed(&h2), "live as landed");
}

/// A call whose input is still streaming isn't read (its input isn't
/// whole), and its round isn't over: the round's other call, its result
/// in, binds and stays in the open batch.
#[test]
fn a_call_still_streaming_its_input_is_not_read() {
    let (h, put) = with_call("partial");
    let mut d = Desk::of(attach(&h[..h.len() - 2]));
    stream(&mut d.t, &h[h.len() - 2..], false);
    d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": "toolu_y"})));
    d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_delta", "tool_id": "toolu_y", "delta": "{\"ops\":[{\"op\":\"pu"})));
    d.apply(&tool_result("toolu_x", &block_receipt(&put)));
    let live = d.t.blocks_input().unwrap().events();
    assert!(!live.iter().any(|e| matches!(e, Event::Call { tool_id, .. } if tool_id == "toolu_y")), "no partial input");
    assert!(!matches!(live.last(), Some(Event::ResultsEnd)), "the round isn't over");
    assert_eq!(d.drawn()["toolu_x"].call, Some(Fate::Bound));
}

/// A response whose first call reports before the model has finished the
/// next call's input: the board reads the round's calls first, as the
/// history will hold them, not the order the events came.
#[test]
fn a_rounds_calls_come_before_its_results() {
    let mut d = Desk::of(attach(&blocks_history()));
    let a = json!([{"op":"put","id":"a","kind":"note","body":{"text":"a"}}]);
    let b = json!([{"op":"put","id":"b","kind":"note","body":{"text":"b"}}]);
    d.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "more"})));
    d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    for (id, ops) in [("toolu_x1", &a), ("toolu_x2", &b)] {
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})));
        d.apply(&tool_use(id, "cards_block", block_input(ops.clone())));
        if id == "toolu_x1" {
            d.apply(&tool_result(id, &block_receipt(&a)));
        }
    }
    d.apply(&tool_result("toolu_x2", &block_receipt(&b)));
    let mut h = blocks_history();
    h.push(json!({"role": "user", "content": "more"}));
    h.push(json!({"role": "assistant", "content": [
        {"type": "tool_use", "id": "toolu_x1", "name": "cards_block", "input": block_input(a.clone())},
        {"type": "tool_use", "id": "toolu_x2", "name": "cards_block", "input": block_input(b.clone())}]}));
    h.push(json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_x1", "content": block_receipt(&a)},
        {"type": "tool_result", "tool_use_id": "toolu_x2", "content": block_receipt(&b)}]}));
    assert_eq!(d.t.blocks_input().unwrap().events(), events(&h));
    let r = d.drawn();
    assert_eq!(r["toolu_x1"].blocks[0].block.id.to_string(), "a");
    assert_eq!(r["toolu_x2"].blocks[0].block.id.to_string(), "b");
    let silent = |id: &str| {
        d.t.items().iter().any(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == id) && i.blocks.as_ref().is_some_and(|b| b.silent()))
    };
    assert!(silent("toolu_x1") && silent("toolu_x2"), "bound: no tool rows of their own");
    assert!(d.t.items().iter().filter(|i| i.kind == ItemKind::Tool).count() > 2);
}

/// P1-2: what a board publishes doesn't grow with the history's refused
/// calls: a committed call's fate is published once, and its row keeps
/// it. Neither does what the transcript stages for the board: once its
/// worker took the history's events, they are the worker's alone.
#[test]
fn refused_calls_grow_no_publication_and_no_second_copy() {
    let refused = |k: usize| {
        let ops = json!([{"op":"put","id":"x","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"finished"}]}}]);
        vec![
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": format!("toolu_n{k}"), "name": "cards_block", "input": block_input(ops)}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": format!("toolu_n{k}"), "content": "cards_block: refused"}]}),
        ]
    };
    let mut sizes = Vec::new();
    for n in [10, 400] {
        let mut h = blocks_history();
        h.push(json!({"role": "user", "content": "again"}));
        for k in 0..n {
            h.extend(refused(k));
        }
        let mut d = Desk::of(attach(&h));
        let first = d.fold();
        assert_eq!(d.drawn()[&format!("toolu_n{}", n - 1)].call, Some(Fate::Unbound), "{n}: a plain row");
        // The next publication, after a live tick: its rows are the board's.
        d.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "tick"})));
        let next = d.fold();
        sizes.push((first.rows.len(), next.rows.len()));
        // The transcript stages only what its worker hasn't taken.
        h.push(json!({"role": "user", "content": "one more"}));
        d.t.apply_update(&HistoryUpdate::Append { base_len: h.len() - 1, messages: h[h.len() - 1..].to_vec() }).unwrap();
        let staged = d.t.blocks_input().unwrap().events().len();
        assert!(staged <= 2, "{n}: {staged} events staged");
    }
    assert_eq!(sizes[0].1, sizes[1].1, "a tick's publication doesn't grow with refused calls: {sizes:?}");
    assert!(sizes[1].1 <= 16, "{sizes:?}");
}

/// A live block call of a note `pad` long: it starts, then its input is
/// whole.
fn live_call(d: &mut Desk, k: usize, pad: &str) -> [SessionEvent; 2] {
    let _ = d;
    let id = format!("toolu_l{k}");
    let ops = json!([{"op":"put","id":format!("n{k}"),"kind":"note","body":{"text":pad}}]);
    [
        llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})),
        tool_use(&id, "cards_block", block_input(ops)),
    ]
}

/// Shady #101 R2-1: a live event costs the frame the events it adds to
/// the board's input, never the turn so far. One response of `n` block
/// calls, at and past a replay's 256-call pass and far past it, each input
/// near the 8 KiB limit, the batch open (no result yet); then its results
/// (a results-only tail, every call refused); then 300 more rounds, the
/// history never landing. Each event writes at most two events; a round's
/// sealing moves its own once, so the turn costs at most twice its events.
/// Besides its events, the live log keeps two rounds' calls. The inputs are
/// the rows' (shared): what a job read of them is counted, heap included.
#[test]
fn the_live_turn_costs_the_frame_only_what_it_adds() {
    let pad = "p".repeat(6_000);
    for n in [255, 256, 257, 600, 3_000] {
        let mut d = Desk::of(attach(&blocks_history()));
        let mut worst = 0;
        let mut each = |d: &mut Desk, e: &SessionEvent| {
            let w0 = d.t.blocks_work();
            d.apply(e);
            worst = worst.max(d.t.blocks_work() - w0);
        };
        each(&mut d, &ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})));
        each(&mut d, &llm(json!({"kind": "llm", "llm": "response_start"})));
        for k in 0..n {
            for e in live_call(&mut d, k, &pad) {
                each(&mut d, &e);
            }
        }
        let open = d.t.blocks_input().unwrap();
        let calls = open.events().iter().filter(|e| matches!(e, Event::Call { tool_id, .. } if tool_id.starts_with("toolu_l"))).count();
        assert_eq!(calls, n, "{n}: the open batch's calls, whole");
        // Its results: a results-only tail.
        for k in 0..n {
            each(&mut d, &tool_result(&format!("toolu_l{k}"), "cards_block: refused (a test)"));
        }
        // 300 more rounds before the history lands.
        for r in 0..300 {
            each(&mut d, &llm(json!({"kind": "llm", "llm": "response_start"})));
            for e in live_call(&mut d, n + r, "short") {
                each(&mut d, &e);
            }
            each(&mut d, &tool_result(&format!("toolu_l{}", n + r), "cards_block: refused (a test)"));
        }
        assert!(worst <= 2, "{n}: an event wrote {worst} events");
        let len = d.t.blocks_live().len();
        assert!(d.t.blocks_work() <= 2 * len, "{n}: {} written and moved for {len} events", d.t.blocks_work());
        assert!(d.t.blocks_live().calls() <= 2, "{n}: the log keeps {} calls besides its events", d.t.blocks_live().calls());
        // The payloads are the rows': the log keeps none of them, a few
        // hundred bytes an event however long its input.
        let payload = n * pad.len();
        let kept = d.t.blocks_live().bytes();
        assert!(kept < 400 * len && kept < payload / 2, "{n}: the log keeps {kept} bytes for {len} events");
        // A job read them: counted, heap included.
        let drawn = d.fold();
        assert_eq!(drawn.rows.values().filter(|r| r.call == Some(Fate::Unbound)).count(), n + 300, "{n}: every call refused");
        assert!(d.t.blocks_bytes() >= payload, "{n}: {} bytes counted for {payload} of inputs", d.t.blocks_bytes());
    }
}

/// Shady #101 R2-1: a job reads the live turn from its log a slice at a
/// time, asked whether to go on between slices, and keeps its place: each
/// stopped run's next reads on from there; a longer turn (the backlog
/// grown) is read on, not again; a reset is read again from its cut. Each
/// board it ends on is a single run's.
#[test]
fn a_job_reads_the_live_turn_a_slice_at_a_time_and_keeps_its_place() {
    // `d`, and its twin `r` (the same events) whose staging no worker took:
    // a fresh worker's single run on it is the reference.
    let (mut d, mut r) = (Desk::of(attach(&blocks_history())), Desk::of(attach(&blocks_history())));
    let both = |d: &mut Desk, r: &mut Desk, e: &SessionEvent| {
        d.apply(e);
        r.apply(e);
    };
    let reference = |r: &mut Desk| {
        let d = (*r.fold()).clone();
        (d.rows, d.live, d.history, d.events)
    };
    let of = |d: Drawn| (d.rows, d.live, d.history, d.events);
    both(&mut d, &mut r, &ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})));
    both(&mut d, &mut r, &llm(json!({"kind": "llm", "llm": "response_start"})));
    for k in 0..2_000 {
        for e in live_call(&mut d, k, "short") {
            both(&mut d, &mut r, &e);
        }
    }
    let input = d.t.blocks_input().unwrap();
    let live = input.events().len() - input.history_len;
    assert!(live > 2_000 && input.history_len < SLICE, "{live}");
    let mut w = Worker::default();
    let mut asked = 0;
    let first = w.run(&input, &mut || {
        asked += 1;
        false
    });
    assert!(first.is_none(), "stopped between slices");
    let mut copied = w.retained().journal - input.history_len;
    assert!(asked == 1 && copied > 0 && copied <= SLICE, "asked {asked}, {copied} copied");
    let mut runs = 1;
    let done = loop {
        if let Some(d) = w.run(&input, &mut || false) {
            break d;
        }
        runs += 1;
        let now = w.retained().journal - input.history_len;
        assert!(now > copied, "run {runs} read on: {now} after {copied}");
        copied = now;
    };
    assert!(runs > 2, "{runs} runs");
    assert_eq!(of(done), reference(&mut r), "a single run's board");
    // The backlog grows: read on from there.
    for k in 2_000..2_500 {
        for e in live_call(&mut d, k, "short") {
            both(&mut d, &mut r, &e);
        }
    }
    let more = d.t.blocks_input().unwrap();
    let before = w.retained().journal;
    assert_eq!(before, input.history_len + live, "the turn so far kept");
    assert!(w.run(&more, &mut || false).is_none());
    let after = w.retained().journal;
    assert!(after > before && after - before <= SLICE, "read on: {before} → {after}");
    let done = loop {
        if let Some(d) = w.run(&more, &mut || false) {
            break d;
        }
    };
    assert_eq!(of(done), reference(&mut r));
    // A reset: the response's calls go; read again from its cut.
    both(&mut d, &mut r, &llm(json!({"kind": "llm", "llm": "response_reset"})));
    let reset = d.t.blocks_input().unwrap();
    assert!(reset.events().len() < input.history_len + 10, "{}", reset.events().len());
    assert!(w.run(&more, &mut || true).is_none(), "an input its log changed: a newer one stands");
    let done = w.run(&reset, &mut || true).expect("done");
    assert_eq!(of(done), reference(&mut r));
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
