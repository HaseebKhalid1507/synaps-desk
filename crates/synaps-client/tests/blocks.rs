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
use synaps_client::blocks::{Drawn, Fate, HOLD, RowBlocks, SLICE, Shown, Worker, event_bytes};
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

/// A live block call's two events: it starts, then its input is whole.
fn live_pair(k: usize) -> [SessionEvent; 2] {
    let id = format!("toolu_l{k}");
    let ops = json!([{"op":"put","id":format!("n{k}"),"kind":"note","body":{"text":format!("note {k}")}}]);
    [
        llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})),
        tool_use(&id, "cards_block", block_input(ops)),
    ]
}

/// A history of one prompt, no card in it (a card's row makes the
/// transcript fold the cards on every event, which these tests don't
/// measure).
fn plain() -> Vec<Value> {
    vec![json!({"role": "user", "content": "notes"})]
}

/// A turn's start and a response's.
fn live_start() -> [SessionEvent; 2] {
    [
        ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})),
        llm(json!({"kind": "llm", "llm": "response_start"})),
    ]
}

/// `0..n` scrambled: a fixed permutation (a step coprime to `n`).
fn scrambled(n: usize) -> Vec<usize> {
    let step = (n / 2..n).find(|s| gcd(*s, n) == 1).unwrap_or(1);
    (0..n).map(|j| j * step % n).collect()
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Shady #101 R3-1: a live call costs the frame no scan of the calls before
/// it and moves none of them. Counted as the elements a fact reads or
/// writes under the live log's lock (slots, rank nodes, cuts), not the
/// events it writes: 255, 256, 257, 600, 3,000 and 12,000 calls, in order
/// (each starts, then its input is whole) and out of order (all start,
/// then they are whole scrambled); then their results. In order, every
/// event touches O(log n) at most and the turn O(1) an event; out of
/// order, O(log n) an event. Either way the events are in model order, the
/// same.
#[test]
fn a_live_call_is_placed_without_reading_the_calls_before_it() {
    for n in [255usize, 256, 257, 600, 3_000, 12_000] {
        let log2 = (usize::BITS - n.leading_zeros()) as usize;
        let mut drawn = Vec::new();
        for out_of_order in [false, true] {
            let mut d = Desk::of(attach(&plain()));
            let (mut worst, mut events) = (0, 0);
            let mut each = |d: &mut Desk, e: &SessionEvent| {
                let v0 = d.t.blocks_live().visits();
                d.apply(e);
                worst = worst.max(d.t.blocks_live().visits() - v0);
                events += 1;
            };
            for e in live_start() {
                each(&mut d, &e);
            }
            if out_of_order {
                for k in 0..n {
                    each(&mut d, &live_pair(k)[0]);
                }
                for k in scrambled(n) {
                    each(&mut d, &live_pair(k)[1]);
                }
            } else {
                for k in 0..n {
                    for e in live_pair(k) {
                        each(&mut d, &e);
                    }
                }
            }
            for k in 0..n {
                each(&mut d, &tool_result(&format!("toolu_l{k}"), "cards_block: refused (a test)"));
            }
            let total = d.t.blocks_live().visits();
            let (each_at_most, in_all) = if out_of_order { (6 * log2 + 16, 4 * log2 + 8) } else { (log2 + 12, 8) };
            assert!(worst <= each_at_most, "{n}, out of order {out_of_order}: an event touched {worst}");
            assert!(total <= in_all * events, "{n}, out of order {out_of_order}: {total} touched for {events} events");
            let input = d.t.blocks_input().unwrap();
            let calls: Vec<String> = input.events()[input.history_len..]
                .iter()
                .filter_map(|e| match e {
                    Event::Call { tool_id, .. } => Some(tool_id.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(calls, (0..n).map(|k| format!("toolu_l{k}")).collect::<Vec<_>>(), "{n}: model order");
            drawn.push(input.events());
        }
        assert_eq!(drawn[0], drawn[1], "{n}: the same events, in order or not");
    }
}

/// Shady #101 R3-1: a reader holds the live log's lock for a batch of
/// handles at most (about HOLD elements touched), whatever it reads and
/// however long the turn: a job reading 14,000 live events, in order and
/// out of order (a round's ranks read for each), a re-fold's copy of
/// them, then a reset of them all (the worker cuts its copy of them off
/// the lock) and its tiny replacement. Its events are made events off the
/// lock, and what the reset let go is dropped off it too.
#[test]
fn a_reader_holds_the_live_log_for_a_batch_at_most() {
    for out_of_order in [false, true] {
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        let n = 7_000;
        if out_of_order {
            for k in 0..n {
                d.apply(&live_pair(k)[0]);
            }
            for k in scrambled(n) {
                d.apply(&live_pair(k)[1]);
            }
        } else {
            for k in 0..n {
                for e in live_pair(k) {
                    d.apply(&e);
                }
            }
        }
        for k in 0..n {
            d.apply(&tool_result(&format!("toolu_l{k}"), "cards_block: refused (a test)"));
        }
        let input = d.t.blocks_input().unwrap();
        assert!(input.live_len() > 3 * SLICE, "{}", input.live_len());
        let _ = d.fold();
        assert_eq!(d.w.retained().live, input.live_len(), "read whole");
        assert_eq!(input.live_events().map(|e| e.len()), Some(input.live_len()), "a re-fold's copy");
        let most = d.t.blocks_live().most_held();
        assert!(most <= HOLD + 64, "out of order {out_of_order}: a hold touched {most}");
        d.apply(&llm(json!({"kind": "llm", "llm": "response_reset"})));
        d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
        for e in live_pair(n) {
            d.apply(&e);
        }
        let _ = d.fold();
        assert!(d.w.retained().live < 8, "cut: {}", d.w.retained().live);
        let most = d.t.blocks_live().most_held();
        assert!(most <= HOLD + 64, "out of order {out_of_order}: after the reset, a hold touched {most}");
    }
}

/// Shady #101 R3-1 and R3-2: the worker cuts a long live turn (a reset
/// after it read 14,000 events of it) off the log's lock, a slice at a
/// time, asked between. Paused there on its own thread, the frame (a third
/// thread: this fails, never hangs) adds live events and takes its input
/// at once. Stopped there, its next run cuts on and ends on the board a
/// fresh worker draws, and the room the cut events took is given back:
/// the worker counts its journal as allocated, at least its events'
/// bytes enumerated apart (the history's events and the live turn's).
#[test]
fn a_worker_cutting_a_long_turn_never_makes_the_frame_wait() {
    use std::sync::mpsc;
    use std::time::Duration;
    let h = plain();
    let (mut d, mut r) = (Desk::of(attach(&h)), Desk::of(attach(&h)));
    let both = |d: &mut Desk, r: &mut Desk, e: &SessionEvent| {
        d.apply(e);
        r.apply(e);
    };
    for e in live_start() {
        both(&mut d, &mut r, &e);
    }
    let n = 7_000;
    for k in 0..n {
        for e in live_pair(k) {
            both(&mut d, &mut r, &e);
        }
        both(&mut d, &mut r, &tool_result(&format!("toolu_l{k}"), "cards_block: refused (a test)"));
    }
    let _ = d.fold();
    let read = d.w.retained();
    assert!(read.live > 3 * SLICE, "{read:?}");
    // The response again from its start: a tiny one.
    let again = [llm(json!({"kind": "llm", "llm": "response_reset"})), llm(json!({"kind": "llm", "llm": "response_start"}))];
    for e in again.iter().chain(&live_pair(n)) {
        both(&mut d, &mut r, e);
    }
    let reset = d.t.blocks_input().unwrap();
    let w = std::mem::take(&mut d.w);
    let (paused_tx, paused) = mpsc::channel();
    let (go_tx, go) = mpsc::channel::<()>();
    let job = std::thread::spawn(move || {
        let mut w = w;
        let mut asked = 0;
        let out = w.run(&reset, &mut || {
            asked += 1;
            if asked == 1 {
                paused_tx.send(()).unwrap();
                go.recv().unwrap();
            }
            asked < 2
        });
        (w, asked, out.is_some())
    });
    paused.recv_timeout(Duration::from_secs(60)).expect("the worker paused inside its cut");
    // The frame, meanwhile: live events and its input.
    let (frame_tx, frame) = mpsc::channel();
    let more: Vec<SessionEvent> = live_pair(n + 1).into_iter().chain(live_pair(n + 2)).collect();
    let more2 = more.clone();
    std::thread::spawn(move || {
        for e in &more {
            d.apply(e);
        }
        let len = d.t.blocks_input().unwrap().live_len();
        frame_tx.send((d, len)).unwrap();
    });
    let (mut d, len) = frame.recv_timeout(Duration::from_secs(20)).expect("the frame never waits for the worker's cut");
    for e in &more2 {
        r.apply(e);
    }
    assert!(len > 0 && len < 16, "{len}");
    go_tx.send(()).unwrap();
    let (w, asked, done) = job.join().unwrap();
    assert!(asked >= 2 && !done, "asked {asked}: stopped inside its cut");
    let cut = w.retained();
    assert!(cut.live < read.live && cut.live > 16, "cut on a slice at a time: {} of {}", cut.live, read.live);
    d.w = w;
    // The next run: cuts on, reads on, the board a fresh worker draws.
    let input = d.t.blocks_input().unwrap();
    let done = d.w.run(&input, &mut || true).expect("done");
    let fresh = (*r.fold()).clone();
    assert_eq!((done.rows, done.live, done.history, done.events), (fresh.rows, fresh.live, fresh.history, fresh.events));
    let kept = d.w.retained();
    assert_eq!(kept.journal, input.history_len + input.live_len());
    assert!(kept.capacity <= 2 * kept.journal + SLICE, "the room given back: {} for {} events", kept.capacity, kept.journal);
    let enumerated: usize = events(&h).iter().chain(&input.live_events().unwrap()).map(event_bytes).sum();
    let room = (kept.capacity - kept.journal) * std::mem::size_of::<Event>();
    assert!(kept.bytes >= enumerated + room + kept.board_bytes, "{} counted; {enumerated} of events, {room} of room", kept.bytes);
}

/// Shady #101 R3-2: the worker counts the card folds it keeps, the
/// board's and the overlay's (each its own copy), as B2 counts them
/// ([`cards_spec::board::Board::card_fold_bytes`]): a history of cards
/// and blocks, then a live turn of both. The folds enumerated apart: a
/// board of the history's events, and a copy of it read on over the live
/// turn's.
#[test]
fn the_worker_counts_its_card_folds() {
    let card = |k: usize| json!({"question": format!("Card {k}?"), "detail": "d".repeat(1_500), "options": ["Yes", "No", "Later"]});
    let mut h = blocks_history();
    for k in 0..40 {
        let id = format!("toolu_c{k}");
        h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_ui", "input": card(k)}]}));
        h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "shown"}]}));
    }
    let mut d = Desk::of(attach(&h));
    for e in live_start() {
        d.apply(&e);
    }
    for k in 0..20 {
        let id = format!("toolu_lc{k}");
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": id})));
        d.apply(&tool_use(&id, "cards_ui", card(100 + k)));
        d.apply(&tool_result(&id, "shown"));
    }
    for e in live_pair(0) {
        d.apply(&e);
    }
    let _ = d.fold();
    let input = d.t.blocks_input().unwrap();
    let history = events(&h);
    let generation = cards_spec::block::Generation::of(&h[0]);
    let mut base = cards_spec::board::Board::new(generation);
    base.read(&history);
    let mut over = base.clone();
    let all: Vec<Event> = history.iter().cloned().chain(input.live_events().unwrap()).collect();
    over.read(&all);
    let (a, b) = (base.card_fold_bytes(), over.card_fold_bytes());
    assert!(b > a && a > 40 * 1_500, "{a} {b}");
    let kept = d.w.retained();
    assert_eq!(kept.card_bytes, a + b, "the board's fold and the overlay's");
    assert!(kept.bytes >= a + b + kept.board_bytes, "{kept:?}");
    assert!(d.t.blocks_bytes() >= kept.bytes, "the tab counts them");
}

/// Shady #101 R3-2: the board the rows draw is counted with the input, at
/// least its blocks' bodies and titles enumerated apart; a row's blocks are
/// the board's, counted once; and a board published again (the same
/// blocks) lets the one before go: its rows take the new board's, so the
/// last owner of the old one is gone.
#[test]
fn the_published_board_is_counted_once_and_let_go() {
    let mut h = vec![json!({"role": "user", "content": "notes"})];
    for k in 0..30 {
        let id = format!("toolu_p{k}");
        let ops = json!([{"op":"put","id":format!("p{k}"),"kind":"note","title":format!("Note {k}"),"body":{"text":"n".repeat(1_400)}}]);
        h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}]}));
        h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": block_receipt(&ops)}]}));
    }
    let mut d = Desk::of(attach(&h));
    let input = d.t.blocks_input().unwrap();
    let first = Arc::new(d.w.run(&input, &mut || true).expect("done"));
    let before = d.t.blocks_bytes();
    d.t.set_drawn(Some(first.clone()));
    let after = d.t.blocks_bytes();
    let shown: Vec<&Shown> = first.rows.values().flat_map(|r| &r.blocks).collect();
    assert!(shown.len() >= 30, "{} shown", shown.len());
    let bodies: usize = shown
        .iter()
        .map(|s| std::mem::size_of::<cards_spec::board::Block>() + s.block.body.bytes() + s.block.title.as_ref().map_or(0, |t| t.len()))
        .sum();
    assert!(after - before >= bodies, "{} counted for the board, its blocks {bodies} at least", after - before);
    assert!(after - before <= 4 * bodies + 64 * 1024, "{} counted for the board, once", after - before);
    // The same board again: the rows take it; the one before goes.
    let old = Arc::downgrade(first.rows.values().find(|r| !r.blocks.is_empty()).unwrap());
    let old_block = Arc::downgrade(&shown[0].block);
    drop(shown);
    drop(first);
    let _ = d.fold();
    assert!(old.upgrade().is_none() && old_block.upgrade().is_none(), "the board before is let go");
    let again = d.t.blocks_bytes();
    assert!(again.abs_diff(after) <= after / 10, "counted once: {after} then {again}");
}

/// A response of `n` block calls (each starts, then its input is whole),
/// then their results, every call refused: a large round, block-only, its
/// results in. Its tool ids are `toolu_{tag}{k}`.
fn big_round(d: &mut Desk, tag: &str, n: usize) {
    d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    for k in 0..n {
        let id = format!("toolu_{tag}{k}");
        let ops = json!([{"op":"put","id":format!("{tag}{k}"),"kind":"note","body":{"text":format!("note {k}")}}]);
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})));
        d.apply(&tool_use(&id, "cards_block", block_input(ops)));
    }
    for k in 0..n {
        d.apply(&tool_result(&format!("toolu_{tag}{k}"), "cards_block: refused (a test)"));
    }
}

/// `big_round(tag, n)` as the history holds it, after the turn's prompt.
fn big_round_landed(tag: &str, n: usize) -> Vec<Value> {
    let calls: Vec<Value> = (0..n)
        .map(|k| {
            let ops = json!([{"op":"put","id":format!("{tag}{k}"),"kind":"note","body":{"text":format!("note {k}")}}]);
            json!({"type": "tool_use", "id": format!("toolu_{tag}{k}"), "name": "cards_block", "input": block_input(ops)})
        })
        .collect();
    let results: Vec<Value> = (0..n)
        .map(|k| json!({"type": "tool_result", "tool_use_id": format!("toolu_{tag}{k}"), "content": "cards_block: refused (a test)"}))
        .collect();
    vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": calls}),
        json!({"role": "user", "content": results}),
    ]
}

/// Shady #101 R4-1: no event destroys a round. A large block-only round
/// (3,000 calls and their refused results, no card anywhere) goes at a
/// delivered steer, at the next response but one (the round before the
/// one it seals), at a reset, and when the history lands (its log
/// replaced). Each time its calls and ids (and a reset's facts, a log's
/// store) are handed to the test's own disposer, its thread held: the
/// event finishes before any of it is dropped (a reset's inputs alive),
/// the frame destroys O(1) elements (a hand-over is one) and touches O(1)
/// under the log's lock. A reset's facts are counted with their inputs
/// (their rows went with the response, the inputs are the disposer's
/// now); the hold gone, the disposer lets every one of them go.
#[test]
fn a_round_let_go_is_destroyed_off_the_frame() {
    use synaps_client::blocks::dispose;
    let disposer = dispose::Disposer::start();
    let _in = disposer.install();
    let n = 3_000;
    // Every boundary checked, the ones that miss listed together.
    let mut wrong = Vec::new();
    for boundary in ["steer", "next response", "reset", "history"] {
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        big_round(&mut d, "big", n);
        let event = match boundary {
            "steer" => Some(llm(json!({"kind": "agent", "agent": "steering_delivered", "message": "and the tests"}))),
            "next response" => {
                // A small round after it; the next response's first call
                // seals the small one, and the big one goes.
                d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
                for e in live_pair(0) {
                    d.apply(&e);
                }
                d.apply(&tool_result("toolu_l0", "cards_block: refused (a test)"));
                d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
                Some(llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": "toolu_next"})))
            }
            "reset" => Some(llm(json!({"kind": "llm", "llm": "response_reset"}))),
            _ => None,
        };
        let inputs: Vec<std::sync::Weak<Value>> =
            d.t.items()
                .iter()
                .filter(|i| i.tool.as_ref().is_some_and(|t| t.tool_id.starts_with("toolu_big")))
                .filter_map(|i| i.kept_allocation())
                .map(Arc::downgrade)
                .collect();
        let payload: usize =
            d.t.items()
                .iter()
                .filter(|i| i.tool.as_ref().is_some_and(|t| t.tool_id.starts_with("toolu_big")))
                .map(|i| i.kept_bytes())
                .sum();
        assert_eq!(inputs.len(), n, "{boundary}: the round's inputs");
        let hold = disposer.hold();
        let (destroyed, visits, was) = (dispose::destroyed(), d.t.blocks_live().visits(), disposer.backlog());
        match &event {
            Some(e) => d.apply(e),
            None => d.t.apply_update(&HistoryUpdate::Append { base_len: 1, messages: big_round_landed("big", n) }).unwrap(),
        }
        let frame = dispose::destroyed() - destroyed;
        let now = disposer.backlog();
        let (items, elements, bytes) = (now.items - was.items, now.elements - was.elements, now.bytes - was.bytes);
        if frame > 4 {
            wrong.push(format!("{boundary}: the frame destroyed {frame} elements"));
        }
        if elements < 2 * n {
            wrong.push(format!("{boundary}: the disposer's {elements} elements ({items} items), the round's {}", 2 * n));
        }
        if event.is_some() {
            let touched = d.t.blocks_live().visits() - visits;
            if touched > 16 {
                wrong.push(format!("{boundary}: the frame touched {touched} under the log's lock"));
            }
        }
        if boundary == "reset" {
            assert!(d.t.items().iter().all(|i| i.kept_allocation().is_none()), "the response's rows went");
            let alive = inputs.iter().filter(|w| w.strong_count() > 0).count();
            assert_eq!(alive, n, "the reset's inputs alive, the disposer held: the event finished before any was dropped");
            if bytes < payload {
                wrong.push(format!("reset: its facts' {alive} live inputs ({payload} bytes, their rows gone) counted {bytes}"));
            }
        }
        drop(hold);
        disposer.flush();
        if boundary == "reset" {
            let alive = inputs.iter().filter(|w| w.strong_count() > 0).count();
            if alive > 0 {
                wrong.push(format!("reset: {alive} of its inputs outlive their rows"));
            }
        }
    }
    assert!(wrong.is_empty(), "rounds let go on the frame: {wrong:#?}");
}

/// Shady #101 R4-2: a call whole again with the same input keeps one
/// allocation: its row, its call and its fact share it, so the row's count
/// is every copy the transcript keeps (the worker's event is its own,
/// counted with its journal), and when the transcript goes none outlives
/// it.
#[test]
fn the_same_input_again_is_one_allocation() {
    let disposer = synaps_client::blocks::dispose::Disposer::start();
    let _in = disposer.install();
    let mut d = Desk::of(attach(&plain()));
    for e in live_start() {
        d.apply(&e);
    }
    let calls = 40;
    let input = |d: &Desk, k: usize| -> Arc<Value> {
        let id = format!("toolu_l{k}");
        let row = d.t.items().iter().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == id)).expect("its row");
        row.kept_allocation().expect("its input").clone()
    };
    let mut seen = Vec::new();
    for k in 0..calls {
        for e in live_pair(k) {
            d.apply(&e);
        }
        seen.push(Arc::downgrade(&input(&d, k)));
    }
    let _ = d.fold();
    for _ in 0..3 {
        for k in 0..calls {
            d.apply(&live_pair(k)[1]);
            seen.push(Arc::downgrade(&input(&d, k)));
        }
        let _ = d.fold();
    }
    let rows: Vec<Arc<Value>> = (0..calls).map(|k| input(&d, k)).collect();
    let alive: Vec<Arc<Value>> = seen.iter().filter_map(std::sync::Weak::upgrade).collect();
    let lost = alive.iter().filter(|a| !rows.iter().any(|r| Arc::ptr_eq(a, r))).count();
    assert_eq!(lost, 0, "{} inputs alive for {calls} rows: {lost} no row's (uncounted)", alive.len());
    let kept: usize = d.t.items().iter().map(|i| i.kept_bytes()).sum();
    let distinct: usize = rows.iter().map(|r| std::mem::size_of::<Value>() + synaps_client::blocks::value_bytes(r)).sum();
    assert_eq!(kept, distinct, "the rows count every allocation once");
    drop((rows, alive));
    drop(d);
    disposer.flush();
    assert!(seen.iter().all(|w| w.strong_count() == 0), "none outlives the transcript");
}

/// A response of `n` block calls whose inputs carry `pad` (a note's
/// text), then their results, every call refused: their inputs outweigh
/// the round's own bookkeeping. Its tool ids are `toolu_{tag}{k}`.
fn fat_round(d: &mut Desk, tag: &str, n: usize, pad: &str) {
    d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    for k in 0..n {
        let id = format!("toolu_{tag}{k}");
        let ops = json!([{"op":"put","id":format!("{tag}{k}"),"kind":"note","body":{"text":pad}}]);
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})));
        d.apply(&tool_use(&id, "cards_block", block_input(ops)));
    }
    for k in 0..n {
        d.apply(&tool_result(&format!("toolu_{tag}{k}"), "cards_block: refused (a test)"));
    }
}

/// The inputs of the calls whose tool ids start `toolu_{tag}`, as their
/// rows keep them: each one's allocation and its bytes.
fn inputs_of(d: &Desk, tag: &str) -> Vec<(std::sync::Weak<Value>, usize)> {
    let prefix = format!("toolu_{tag}");
    d.t.items()
        .iter()
        .filter(|i| i.tool.as_ref().is_some_and(|t| t.tool_id.starts_with(&prefix)))
        .filter_map(|i| Some((Arc::downgrade(i.kept_allocation()?), i.kept_bytes())))
        .collect()
}

/// Of `inputs`, the bytes of those still alive.
fn alive_bytes(inputs: &[(std::sync::Weak<Value>, usize)]) -> usize {
    inputs.iter().filter(|(w, _)| w.strong_count() > 0).map(|(_, b)| b).sum()
}

fn reset() -> SessionEvent {
    llm(json!({"kind": "llm", "llm": "response_reset"}))
}

/// Shady #101 R5-2 A: what the disposer holds stays charged until the last
/// owner of its payload is dropped, checked between drains (its thread
/// held, then let drop one item at a time), not only before the first and
/// after the last. Three ways a large round's inputs (their rows gone)
/// end up the disposer's: a reset (its round, facts and results), its log
/// dropped (the tab gone: its rounds and store), and its log dropped while
/// a job still reads its store (the store stays the job's until it lets
/// go). At every point the inputs still alive are covered by the charge.
#[test]
fn a_retired_input_stays_charged_until_its_last_owner_goes() {
    use synaps_client::blocks::dispose::Disposer;
    let (n, pad) = (300, "c".repeat(4_000));
    let mut wrong = Vec::new();
    for case in ["reset", "log dropped", "log dropped, a job reading it"] {
        let disposer = Disposer::start();
        let _in = disposer.install();
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        fat_round(&mut d, "fat", n, &pad);
        let inputs = inputs_of(&d, "fat");
        assert_eq!(inputs.len(), n, "{case}: the round's inputs");
        let mut job = (case == "log dropped, a job reading it").then(|| d.t.blocks_input().expect("blocks"));
        let hold = disposer.hold();
        let desk = match case {
            "reset" => {
                d.apply(&reset());
                assert!(d.t.items().iter().all(|i| i.kept_allocation().is_none()), "the response's rows went");
                Some(d)
            }
            _ => {
                drop(d);
                None
            }
        };
        let mut drains = 0;
        loop {
            let (alive, b) = (alive_bytes(&inputs), disposer.backlog());
            if alive > b.bytes {
                wrong.push(format!(
                    "{case}: after {drains} drains, {alive} bytes of inputs alive, {} charged ({} items queued)",
                    b.bytes, b.items
                ));
            }
            if b.items > 0 {
                hold.step(1);
                drains += 1;
            } else if let Some(j) = job.take() {
                // The job lets go: its store goes to the disposer.
                drop(j);
            } else {
                break;
            }
        }
        drop(hold);
        disposer.flush();
        if alive_bytes(&inputs) > 0 {
            wrong.push(format!("{case}: inputs outlive the disposer's drains"));
        }
        let b = disposer.backlog();
        if (b.items, b.bytes, b.in_place) != (0, 0, 0) {
            wrong.push(format!("{case}: drained, the disposer still holds {b:?}"));
        }
        drop(desk);
    }
    assert!(wrong.is_empty(), "a payload alive past its charge: {wrong:#?}");
}

/// Shady #101 R5-1: the disposer admits a bounded backlog and never makes
/// the frame wait. Its thread held (nothing it is handed goes), two
/// sessions reset response after response (1 MiB of inputs each, 96 MiB
/// in all), then a third's tab is evicted and a fourth's history lands
/// (its log replaced), then a flood of small resets passes its item cap.
/// Every event finishes (the thread held throughout); at every point what
/// it holds stays within `MAX_BYTES` and `MAX_ITEMS`, every retired input
/// alive is charged, and what it refuses is dropped where it was let go.
/// The hold gone, everything goes.
#[test]
fn the_disposer_admits_a_bounded_backlog_and_drops_the_rest_in_place() {
    use synaps_client::blocks::dispose::{Disposer, MAX_BYTES, MAX_ITEMS};
    let disposer = Disposer::start();
    let _in = disposer.install();
    let hold = disposer.hold();
    let pad = "f".repeat(64 * 1024);
    let mut wrong = Vec::new();
    let mut retired = Vec::new();
    let check = |what: &str, retired: &[(std::sync::Weak<Value>, usize)]| {
        let (b, alive) = (disposer.backlog(), alive_bytes(retired));
        let mut wrong = Vec::new();
        if b.bytes > MAX_BYTES || b.items > MAX_ITEMS {
            wrong.push(format!("{what}: it holds {} items, {} bytes", b.items, b.bytes));
        }
        if alive > b.bytes {
            wrong.push(format!("{what}: {alive} bytes of retired inputs alive, {} charged", b.bytes));
        }
        wrong
    };
    let mut desks = [Desk::of(attach(&plain())), Desk::of(attach(&plain()))];
    for d in &mut desks {
        for e in live_start() {
            d.apply(&e);
        }
    }
    for r in 0..96 {
        let d = &mut desks[r % 2];
        let tag = format!("f{r}_");
        fat_round(d, &tag, 16, &pad);
        let inputs = inputs_of(d, &tag);
        assert_eq!(inputs.len(), 16);
        d.apply(&reset());
        retired.extend(inputs);
        wrong.extend(check(&format!("reset {r}"), &retired));
    }
    let refused = disposer.backlog().in_place;
    if refused == 0 {
        wrong.push("96 MiB of resets: none refused".into());
    }
    // A tab evicted and a history landed while it is full.
    let mut evicted = Desk::of(attach(&plain()));
    for e in live_start() {
        evicted.apply(&e);
    }
    fat_round(&mut evicted, "ev", 16, &pad);
    let gone = inputs_of(&evicted, "ev");
    drop(evicted);
    retired.extend(gone);
    wrong.extend(check("a tab evicted", &retired));
    let mut landed = Desk::of(attach(&plain()));
    for e in live_start() {
        landed.apply(&e);
    }
    big_round(&mut landed, "big", 50);
    landed.t.apply_update(&HistoryUpdate::Append { base_len: 1, messages: big_round_landed("big", 50) }).unwrap();
    wrong.extend(check("a history landed", &retired));
    // Items: drained, then small resets past the item cap (their bytes
    // far below the byte cap).
    drop(hold);
    disposer.flush();
    if alive_bytes(&retired) > 0 {
        wrong.push("retired inputs outlive the hold".into());
    }
    let hold = disposer.hold();
    let refused = disposer.backlog().in_place;
    let [a, _] = &mut desks;
    for k in 0..MAX_ITEMS + 100 {
        a.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
        for e in live_pair(k) {
            a.apply(&e);
        }
        a.apply(&reset());
        let b = disposer.backlog();
        if b.items > MAX_ITEMS || b.bytes > MAX_BYTES {
            wrong.push(format!("small reset {k}: it holds {} items, {} bytes", b.items, b.bytes));
            break;
        }
    }
    let b = disposer.backlog();
    if b.items != MAX_ITEMS || b.in_place < refused + 100 {
        wrong.push(format!("{} small resets: {} items held, {} refused", MAX_ITEMS + 100, b.items, b.in_place - refused));
    }
    drop(hold);
    disposer.flush();
    let b = disposer.backlog();
    if (b.items, b.bytes) != (0, 0) {
        wrong.push(format!("drained, it still holds {b:?}"));
    }
    assert!(wrong.is_empty(), "an unbounded disposer: {wrong:#?}");
}

/// Shady #101 R5-1: an owned disposer shuts down. With work queued, its
/// owner's shutdown drains it and joins its thread; a log that outlives
/// it drops what it lets go in place. Its thread wedged (held), shutdown
/// waits `JOIN` at most and detaches it, and the thread drops what it
/// holds once free. A disposer whose thread did not start drops all in
/// place, counted.
#[test]
fn an_owned_disposer_drains_and_joins_at_shutdown() {
    use std::time::{Duration, Instant};
    use synaps_client::blocks::dispose::{Disposer, JOIN};
    let pad = "s".repeat(4_000);
    let mut wrong = Vec::new();
    let round = |d: &mut Desk, tag: &str| {
        fat_round(d, tag, 40, &pad);
        let inputs = inputs_of(d, tag);
        assert_eq!(inputs.len(), 40);
        d.apply(&reset());
        inputs
    };
    let started = || {
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        d
    };
    // Work queued, then shut down.
    {
        let disposer = Disposer::start();
        let bin = disposer.bin();
        let _in = disposer.install();
        let mut d = started();
        let hold = disposer.hold();
        let inputs = round(&mut d, "a");
        if bin.backlog().items == 0 || alive_bytes(&inputs) == 0 {
            wrong.push(format!("queued: {:?}", bin.backlog()));
        }
        drop(hold);
        if !disposer.shutdown() {
            wrong.push("work queued: shutdown did not join".into());
        }
        if alive_bytes(&inputs) > 0 || bin.backlog().items > 0 || bin.backlog().bytes > 0 {
            wrong.push(format!("shut down: not drained ({:?})", bin.backlog()));
        }
        let was = bin.backlog().in_place;
        let after = round(&mut d, "b");
        if alive_bytes(&after) > 0 || bin.backlog().in_place != was + 1 {
            wrong.push(format!("after shutdown: not dropped in place ({:?})", bin.backlog()));
        }
    }
    // Its thread wedged: detached within the deadline, done once free.
    {
        let disposer = Disposer::start();
        let bin = disposer.bin();
        let _in = disposer.install();
        let mut d = started();
        let hold = disposer.hold();
        let inputs = round(&mut d, "w");
        let t0 = Instant::now();
        let joined = disposer.shutdown();
        let took = t0.elapsed();
        if joined || took > JOIN * 4 {
            wrong.push(format!("wedged: joined {joined} after {took:?}"));
        }
        drop(hold);
        let t0 = Instant::now();
        while alive_bytes(&inputs) > 0 && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if alive_bytes(&inputs) > 0 || bin.backlog().bytes > 0 {
            wrong.push(format!("detached: never drained ({:?})", bin.backlog()));
        }
    }
    // Its thread never started.
    {
        let disposer = Disposer::failed();
        let bin = disposer.bin();
        let _in = disposer.install();
        let mut d = started();
        let inputs = round(&mut d, "f");
        if alive_bytes(&inputs) > 0 || bin.backlog().in_place != 1 {
            wrong.push(format!("no thread: not dropped in place ({:?})", bin.backlog()));
        }
        if !disposer.shutdown() {
            wrong.push("no thread: shutdown failed".into());
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Shady #101 R5-2 B: a log retired whole (its tab gone) is charged as it
/// is allocated. Six large rounds, sealed one after another (their calls
/// whole out of order: their slots and ranks have room to spare), and the
/// open one: the charge its drop takes, kept as the log grew (O(1) at the
/// drop), is every allocation the drop moves, walked one by one (the
/// sealed segments' slots, ranks and room, the segments' vector, the
/// starts and cuts, the rounds and the head); and it is what the disposer
/// holds once the log is handed over.
#[test]
fn a_log_is_retired_charged_as_it_is_allocated() {
    use synaps_client::blocks::dispose::Disposer;
    let disposer = Disposer::start();
    let _in = disposer.install();
    let mut d = Desk::of(attach(&plain()));
    for e in live_start() {
        d.apply(&e);
    }
    let n = 300;
    for r in 0..7 {
        d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
        let id = |k: usize| format!("toolu_r{r}_{k}");
        for k in 0..n {
            d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id(k)})));
        }
        for k in scrambled(n) {
            let ops = json!([{"op":"put","id":format!("r{r}_{k}"),"kind":"note","body":{"text":format!("note {k}")}}]);
            d.apply(&tool_use(&id(k), "cards_block", block_input(ops)));
        }
        if r < 6 {
            for k in 0..n {
                d.apply(&tool_result(&id(k), "cards_block: refused (a test)"));
            }
        }
    }
    let mut wrong = Vec::new();
    let (charge, walk) = d.t.blocks_live().retirement();
    if charge != walk {
        wrong.push(format!("its charge {charge}, its allocations {walk}"));
    }
    let hold = disposer.hold();
    let was = disposer.backlog();
    drop(d);
    let b = disposer.backlog();
    if (b.items - was.items, b.bytes - was.bytes) != (1, charge) {
        wrong.push(format!("handed over: {} items, {} bytes; its charge {charge}", b.items - was.items, b.bytes - was.bytes));
    }
    drop(hold);
    disposer.flush();
    if disposer.backlog().bytes != 0 {
        wrong.push(format!("drained, still charged: {:?}", disposer.backlog()));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// `n` block calls in the open response, whose inputs carry `pad` (a
/// note's text), and their results. Its tool ids are `toolu_{tag}{k}`.
fn fat_calls(d: &mut Desk, tag: &str, n: usize, pad: &str) {
    for k in 0..n {
        let id = format!("toolu_{tag}{k}");
        let ops = json!([{"op":"put","id":format!("{tag}{k}"),"kind":"note","body":{"text":pad}}]);
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": id})));
        d.apply(&tool_use(&id, "cards_block", block_input(ops)));
    }
    for k in 0..n {
        d.apply(&tool_result(&format!("toolu_{tag}{k}"), "cards_block: refused (a test)"));
    }
}

/// A history landing on [`plain`]'s: the turn's prompt and an answer.
fn landing() -> HistoryUpdate {
    HistoryUpdate::Append {
        base_len: 1,
        messages: vec![json!({"role": "user", "content": "go"}), json!({"role": "assistant", "content": "done"})],
    }
}

/// Haseeb's #101 round 7 (Shady R6-1): one unit past the byte cap is
/// admitted into an empty queue. A 96 MiB round goes four ways: retired
/// alone at a steer, its reset (its round, facts and results), its log
/// replaced by a history landing, its log dropped (the tab gone). The
/// disposer's thread held, each is queued, charged with all it keeps (the
/// round alone with its calls' inputs, though its rows keep them too), and
/// the frame destroys none of it (a hand-over is one element). The hold
/// gone, all of it goes.
#[test]
fn a_unit_past_the_byte_cap_is_admitted_into_an_empty_queue() {
    use synaps_client::blocks::dispose::{self, Disposer, MAX_BYTES};
    let (n, pad) = (384, "u".repeat(256 * 1024));
    let mut wrong = Vec::new();
    for case in ["steer", "reset", "history landed", "tab gone"] {
        let disposer = Disposer::start();
        let _in = disposer.install();
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        fat_round(&mut d, "u", n, &pad);
        let inputs = inputs_of(&d, "u");
        let total: usize = inputs.iter().map(|(_, b)| b).sum();
        assert!(inputs.len() == n && total > MAX_BYTES + (16 << 20), "{case}: {} inputs, {total} bytes", inputs.len());
        let hold = disposer.hold();
        let before = dispose::destroyed();
        let desk = match case {
            "steer" => {
                d.apply(&steer("more"));
                Some(d)
            }
            "reset" => {
                d.apply(&reset());
                Some(d)
            }
            "history landed" => {
                d.t.apply_update(&landing()).unwrap();
                Some(d)
            }
            _ => {
                drop(d);
                None
            }
        };
        let (b, frame) = (disposer.backlog(), dispose::destroyed() - before);
        if b.in_place > 0 || frame > 8 {
            wrong.push(format!("{case}: {} refused, the frame destroyed {frame} elements", b.in_place));
        }
        if b.items == 0 || b.bytes < total {
            wrong.push(format!("{case}: {} items queued, {} bytes charged for {total} bytes of inputs", b.items, b.bytes));
        }
        drop(hold);
        disposer.flush();
        drop(desk);
        disposer.flush();
        let b = disposer.backlog();
        if alive_bytes(&inputs) > 0 || (b.items, b.bytes, b.in_place) != (0, 0, 0) {
            wrong.push(format!("{case}: drained, {} bytes of inputs alive, it holds {b:?}", alive_bytes(&inputs)));
        }
    }
    assert!(wrong.is_empty(), "a unit past the cap refused into an empty queue: {wrong:#?}");
}

/// Elements dropped in place as a probe saw them, each with its store's
/// lock held or not ([`a_unit_refused_under_saturation_is_dropped_outside_the_stores_lock`]).
type Seen = std::rc::Rc<std::cell::RefCell<Vec<(usize, bool)>>>;

/// What `seen` saw at `what`, taken: something dropped in place, none of
/// it under the store's lock. Its most elements in one drop.
fn in_place_at(what: &str, seen: &Seen, wrong: &mut Vec<String>) -> usize {
    let s = std::mem::take(&mut *seen.borrow_mut());
    if s.is_empty() {
        wrong.push(format!("{what}: nothing dropped in place"));
    }
    let locked: usize = s.iter().filter(|(_, l)| *l).map(|(e, _)| e).sum();
    if locked > 0 {
        wrong.push(format!("{what}: {locked} elements dropped under the store's lock"));
    }
    s.iter().map(|(e, _)| *e).max().unwrap_or(0)
}

/// Haseeb's #101 round 7 (Shady R6-1), the overload contract: with the
/// queue saturated, by its items (`MAX_ITEMS` queued) or its bytes (a
/// store a job still reads charged past `MAX_BYTES`, an item queued), a
/// unit let go is refused and dropped where it is let go, and never under
/// the log's store lock. The disposer's thread held, a large round goes
/// at each boundary (a steer, the next response, a reset, a history
/// landing, the tab gone), at 255, 256, 257 and 3,000 calls (and 12,000,
/// its bytes full): each is dropped in place (its whole unit, the
/// contract), and a probe trying the store's lock as each drop starts
/// finds it free.
#[test]
fn a_unit_refused_under_saturation_is_dropped_outside_the_stores_lock() {
    use synaps_client::blocks::dispose::{self, Disposer, MAX_BYTES, MAX_ITEMS};
    let pad = "p".repeat(64);
    let mut wrong = Vec::new();
    let mut sizes = Vec::new();
    let runs = [
        ("items", 255),
        ("items", 256),
        ("items", 257),
        ("items", 3_000),
        ("bytes", 255),
        ("bytes", 256),
        ("bytes", 257),
        ("bytes", 3_000),
        ("bytes", 12_000),
    ];
    for (cap, n) in runs {
        let disposer = Disposer::start();
        let _in = disposer.install();
        let hold = disposer.hold();
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        let mut job = None;
        if cap == "items" {
            let mut k = 0;
            while disposer.backlog().items < MAX_ITEMS {
                d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
                for e in live_pair(k) {
                    d.apply(&e);
                }
                d.apply(&reset());
                k += 1;
            }
        } else {
            let mut full = Desk::of(attach(&plain()));
            for e in live_start() {
                full.apply(&e);
            }
            fat_round(&mut full, "full", 72, &"s".repeat(1 << 20));
            job = Some(full.t.blocks_input().expect("blocks"));
            drop(full);
            assert!(disposer.backlog().bytes > MAX_BYTES, "{cap}: a job's store charged past the cap");
        }
        let seen = Seen::default();
        let lock = std::rc::Rc::new(std::cell::RefCell::new(d.t.blocks_live().lock_probe()));
        let probe = {
            let (seen, lock) = (seen.clone(), lock.clone());
            dispose::on_in_place(move |elements| {
                if elements > 0 {
                    seen.borrow_mut().push((elements, lock.borrow().held()));
                }
            })
        };
        let at = |what: &str| format!("{cap} full, {n} calls, {what}");
        // A steer: the open round retired alone.
        fat_round(&mut d, "a", n, &pad);
        seen.borrow_mut().clear();
        d.apply(&steer("more"));
        let steer = in_place_at(&at("a steer"), &seen, &mut wrong);
        // The next response: the round before the open one goes as the
        // open one is sealed.
        fat_round(&mut d, "b", n, &pad);
        fat_round(&mut d, "c", n, &pad);
        seen.borrow_mut().clear();
        d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
        d.apply(&llm(json!({"kind": "llm", "llm": "text", "text": "and"})));
        let next = in_place_at(&at("the next response"), &seen, &mut wrong);
        // A reset of the open response, its calls in.
        fat_calls(&mut d, "r", n, &pad);
        seen.borrow_mut().clear();
        d.apply(&reset());
        let reset = in_place_at(&at("a reset"), &seen, &mut wrong);
        // A history landing: the log replaced.
        fat_round(&mut d, "e", n, &pad);
        seen.borrow_mut().clear();
        d.t.apply_update(&landing()).unwrap();
        let landed = in_place_at(&at("a history landing"), &seen, &mut wrong);
        *lock.borrow_mut() = d.t.blocks_live().lock_probe();
        // The tab gone: its log with it.
        fat_round(&mut d, "f", n, &pad);
        seen.borrow_mut().clear();
        drop(d);
        let gone = in_place_at(&at("the tab gone"), &seen, &mut wrong);
        sizes.push(format!("{cap} {n}: steer {steer}, next {next}, reset {reset}, landed {landed}, gone {gone}"));
        drop(probe);
        drop(job);
        drop(hold);
        disposer.flush();
        let b = disposer.backlog();
        if (b.items, b.bytes) != (0, 0) {
            wrong.push(format!("{cap} {n}: drained, it holds {b:?}"));
        }
    }
    eprintln!("dropped in place, the most elements in one drop: {sizes:#?}");
    assert!(wrong.is_empty(), "refused units dropped under the store's lock: {wrong:#?}");
}

/// Haseeb's #101 round 7 (Shady R6-2 A): a round retired on its own (at a
/// steer) is charged with the inputs it keeps. So when a later owner of
/// the same inputs is refused, the round still queued keeps them charged:
/// the log's store (over 64 MiB, the round queued) refused as its tab
/// goes, or the store a job read, refused as the job lets go. Checked
/// before any drain and after each.
#[test]
fn a_round_queued_alone_keeps_its_inputs_charged_when_a_later_owner_is_refused() {
    use synaps_client::blocks::dispose::{Disposer, MAX_BYTES};
    let (n, pad) = (288, "q".repeat(256 * 1024));
    let mut wrong = Vec::new();
    for case in ["its store refused", "a job's store refused"] {
        let disposer = Disposer::start();
        let _in = disposer.install();
        let mut d = Desk::of(attach(&plain()));
        for e in live_start() {
            d.apply(&e);
        }
        fat_round(&mut d, "q", n, &pad);
        let inputs = inputs_of(&d, "q");
        let total: usize = inputs.iter().map(|(_, b)| b).sum();
        assert!(inputs.len() == n && total > MAX_BYTES, "{case}: {} inputs, {total} bytes", inputs.len());
        let hold = disposer.hold();
        d.apply(&steer("more"));
        let job = (case == "a job's store refused").then(|| d.t.blocks_input().expect("blocks"));
        drop(d);
        drop(job);
        if disposer.backlog().in_place == 0 {
            wrong.push(format!("{case}: the store was not refused"));
        }
        let mut drains = 0;
        loop {
            let (alive, b) = (alive_bytes(&inputs), disposer.backlog());
            if alive > b.bytes {
                wrong.push(format!(
                    "{case}: after {drains} drains, {alive} bytes of inputs alive, {} charged ({} items queued)",
                    b.bytes, b.items
                ));
            }
            if b.items == 0 {
                break;
            }
            hold.step(1);
            drains += 1;
        }
        drop(hold);
        disposer.flush();
        let b = disposer.backlog();
        if alive_bytes(&inputs) > 0 || (b.items, b.bytes) != (0, 0) {
            wrong.push(format!("{case}: drained, {} bytes of inputs alive, it holds {b:?}", alive_bytes(&inputs)));
        }
    }
    assert!(wrong.is_empty(), "a queued round's inputs alive past its charge: {wrong:#?}");
}

/// Haseeb's #101 round 7 (Shady R6-2 B): a log dropped with no job reading
/// its store moves the store into its unit, so the disposer dropping the
/// unit drops the store, whenever it runs. Here it runs right after the
/// hand-over, the dropping thread paused there (a probe), before the log's
/// own fields go: nothing of the store's outlives the unit's charge.
#[test]
fn a_logs_store_goes_with_its_unit_before_the_log_is_gone() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use synaps_client::blocks::dispose::{self, Backlog, Disposer};
    let disposer = Disposer::start();
    let _in = disposer.install();
    let mut d = Desk::of(attach(&plain()));
    for e in live_start() {
        d.apply(&e);
    }
    fat_round(&mut d, "w", 60, &"w".repeat(64 * 1024));
    let inputs = inputs_of(&d, "w");
    assert_eq!(inputs.len(), 60);
    let hold = Rc::new(disposer.hold());
    let seen: Rc<RefCell<Option<(usize, Backlog)>>> = Rc::default();
    let probe = {
        let (hold, seen, inputs, bin) = (hold.clone(), seen.clone(), inputs.clone(), disposer.bin());
        dispose::on_handed(move || {
            if seen.borrow().is_some() {
                return;
            }
            // The unit just handed over, dropped now: the consumer first.
            hold.step(bin.backlog().items);
            *seen.borrow_mut() = Some((alive_bytes(&inputs), bin.backlog()));
        })
    };
    drop(d);
    drop(probe);
    let (alive, b) = seen.take().expect("the log handed over");
    assert!(
        alive <= b.bytes,
        "its unit dropped before the log's fields went: {alive} bytes of its store's inputs alive, {} charged ({b:?})",
        b.bytes
    );
    drop(hold);
    disposer.flush();
    let b = disposer.backlog();
    assert_eq!((alive_bytes(&inputs), b.items, b.bytes), (0, 0, 0), "drained: {b:?}");
}

fn card(k: usize) -> Value {
    json!({"question": format!("Card {k}?"), "detail": "d".repeat(600), "options": ["Yes", "No", "Later"]})
}

fn card_id(k: usize) -> cards_spec::CardId {
    cards_spec::CardId::parse(&format!("c{k:06}")).expect("a card id")
}

fn card_doc(k: usize) -> (cards_spec::CardDoc, bool) {
    match cards_spec::parse_input(&card(k)).expect("a card") {
        cards_spec::Input::Create { doc, wait } => (doc, wait),
        cards_spec::Input::Close(_) => unreachable!(),
    }
}

/// Card `k`'s result: shown, it binds.
fn card_shown(k: usize) -> String {
    let (doc, wait) = card_doc(k);
    format!(
        "{}\nIts answer will come as a user message starting \"[card {}]\".",
        cards_spec::Envelope::showing(card_id(k), &doc, wait),
        card_id(k).as_str()
    )
}

/// A user's answer to card `k`.
fn card_answer(k: usize, choice: &str) -> String {
    let (doc, _) = card_doc(k);
    let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&doc, &json!({"choice": choice})).expect("an answer"));
    cards_spec::message::build(&card_id(k), &doc, &reply)
}

/// Cards asked and bound in a history, after its prompt.
fn bound_cards(h: &mut Vec<Value>, ks: &[usize]) {
    for &k in ks {
        let id = format!("toolu_c{k}");
        h.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_ui", "input": card(k)}]}));
        h.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": card_shown(k)}]}));
    }
}

/// What the worker's card folds keep, as B2's own folds count them: the
/// history's board read as far as the worker read it and, once it read
/// all of it, the overlay's, read on over the live turn's events as far
/// as the worker read them.
fn card_counts(d: &Desk, h: &[Value]) -> usize {
    let history = events(h);
    let mut base = cards_spec::board::Board::new(cards_spec::block::Generation::of(&h[0]));
    base.read(&history[..d.w.position()]);
    if d.w.position() < history.len() {
        return base.card_fold_bytes();
    }
    let live = d.t.blocks_input().unwrap().live_events().unwrap();
    let all: Vec<Event> = history.iter().cloned().chain(live[..d.w.live_read()].iter().cloned()).collect();
    let mut over = base.clone();
    over.read(&all);
    base.card_fold_bytes() + over.card_fold_bytes()
}

fn steer(text: &str) -> SessionEvent {
    llm(json!({"kind": "agent", "agent": "steering_delivered", "message": text}))
}

/// Shady #101 R4-2: the worker's card counts are B2's folds' at every
/// boundary that changes them, a card call's or not: a pending card call,
/// its valid result (the card binds), a user's valid answer and another
/// (allowed while the card is answerable), an answer to the history's
/// card. And wherever a job stops, the history read part way (between
/// its slices) or the live turn (between its batches), what it publishes
/// counts what it holds then.
#[test]
fn the_card_counts_follow_every_fold_and_pause() {
    let mut h = plain();
    bound_cards(&mut h, &[0, 2, 3]);
    let mut d = Desk::of(attach(&h));
    for e in live_start() {
        d.apply(&e);
    }
    let start = |id: &str| llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": id}));
    let steps: Vec<(&str, Vec<SessionEvent>)> = vec![
        ("a block call", live_pair(0).to_vec()),
        ("a pending card", vec![start("toolu_c1"), tool_use("toolu_c1", "cards_ui", card(1))]),
        ("its result binds it", vec![tool_result("toolu_c1", &card_shown(1))]),
        ("answered", vec![steer(&card_answer(1, "Yes"))]),
        ("answered again", vec![steer(&card_answer(1, "No"))]),
        ("the history's card answered", vec![steer(&card_answer(0, "Later"))]),
    ];
    // Every boundary checked, the ones that miss listed together.
    let mut wrong = Vec::new();
    let mut last = 0;
    for (step, events) in steps {
        for e in &events {
            d.apply(e);
        }
        let _ = d.fold();
        let (kept, want) = (d.w.retained(), card_counts(&d, &h));
        if kept.card_bytes != want {
            wrong.push(format!("{step}: {} for {want}", kept.card_bytes));
        }
        assert!(want != last, "{step}: B2's fold changed");
        assert!(d.t.blocks_bytes() >= kept.bytes, "{step}: published");
        last = want;
    }
    // Paused: a history longer than a slice (its cards in the first), then
    // a live turn longer than a batch (a card, its binding, an answer).
    let mut h = plain();
    bound_cards(&mut h, &[0, 2, 3]);
    for m in 0..50 {
        let calls: Vec<Value> = (0..100)
            .map(|k| json!({"type": "tool_use", "id": format!("toolu_r{m}_{k}"), "name": "read", "input": {"path": "a"}}))
            .collect();
        let results: Vec<Value> =
            (0..100).map(|k| json!({"type": "tool_result", "tool_use_id": format!("toolu_r{m}_{k}"), "content": "a"})).collect();
        h.push(json!({"role": "assistant", "content": calls}));
        h.push(json!({"role": "user", "content": results}));
    }
    assert!(events(&h).len() > SLICE + 64);
    let mut d = Desk::of(attach(&h));
    for e in live_start() {
        d.apply(&e);
    }
    for e in live_pair(0).into_iter().chain([
        start("toolu_c1"),
        tool_use("toolu_c1", "cards_ui", card(1)),
        tool_result("toolu_c1", &card_shown(1)),
        steer(&card_answer(1, "Yes")),
    ]) {
        d.apply(&e);
    }
    d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    for k in 0..600 {
        let id = format!("toolu_p{k}");
        d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "read", "tool_id": id})));
        d.apply(&tool_use(&id, "read", json!({"path": "b"})));
    }
    let input = d.t.blocks_input().unwrap();
    let (mut history, mut live) = (0, 0);
    while d.w.run(&input, &mut || false).is_none() {
        let (kept, want) = (d.w.retained(), card_counts(&d, &h));
        let at = (d.w.position(), d.w.live_read());
        if kept.card_bytes != want {
            wrong.push(format!("paused at {at:?}: {} for {want}", kept.card_bytes));
        }
        assert!(d.t.blocks_bytes() >= kept.bytes, "paused at {at:?}: published as it is");
        if at.0 < events(&h).len() {
            history += 1
        } else {
            live += 1
        }
        assert!(history + live < 100, "it reads on");
    }
    assert!(history >= 1 && live >= 1, "paused in the history {history}, in the live turn {live}");
    let (kept, want) = (d.w.retained().card_bytes, card_counts(&d, &h));
    if kept != want {
        wrong.push(format!("done: {kept} for {want}"));
    }
    assert!(wrong.is_empty(), "the card counts missed B2's folds: {wrong:#?}");
}

/// Shady #101 R4-1, the bound: one hold of the live log's lock touches at
/// most max(HOLD, P + G) elements. A reader's batch stops before the next
/// handle's read could pass HOLD (G: the most one read touches, its
/// segment found among S and its slot's rank-select among N slots,
/// ⌊log₂ S⌋ + ⌊log₂ N⌋ + 4), one handle at least; before it come its cut
/// queries, P: two at most, each b(b+1)/2 tree nodes for b = ⌊log₂ C⌋ + 1,
/// C the log's cuts. Here the 7,000-call scrambled turn, read whole, and
/// a stale input (taken before 4,000 calls whole out of order: its cut
/// queries read every cut since).
#[test]
fn a_hold_of_the_live_log_is_bounded_by_its_batch_and_its_cut_queries() {
    let bound = |d: &Desk| {
        let (cuts, segments, widest) = d.t.blocks_live().shape();
        let bits = |n: usize| (usize::BITS - n.leading_zeros()) as usize;
        let per_query = bits(cuts) * (bits(cuts) + 1) / 2;
        HOLD.max(2 * per_query + bits(segments) + 2 + bits(widest))
    };
    let mut d = Desk::of(attach(&plain()));
    for e in live_start() {
        d.apply(&e);
    }
    let n = 7_000;
    for k in 0..n {
        d.apply(&live_pair(k)[0]);
    }
    for k in scrambled(n) {
        d.apply(&live_pair(k)[1]);
    }
    for k in 0..n {
        d.apply(&tool_result(&format!("toolu_l{k}"), "cards_block: refused (a test)"));
    }
    let _ = d.fold();
    let (most, b) = (d.t.blocks_live().most_held(), bound(&d));
    assert!(most <= b, "scrambled: a hold touched {most}, bound {b} ({:?})", d.t.blocks_live().shape());
    let mut d = Desk::of(attach(&plain()));
    for e in live_start() {
        d.apply(&e);
    }
    for k in 0..2_000 {
        for e in live_pair(k) {
            d.apply(&e);
        }
    }
    let stale = d.t.blocks_input().unwrap();
    for k in 2_000..6_000 {
        d.apply(&live_pair(k)[0]);
    }
    for k in scrambled(4_000) {
        d.apply(&live_pair(2_000 + k)[1]);
    }
    assert!(d.t.blocks_live().shape().0 > 2_000, "cuts {:?}", d.t.blocks_live().shape());
    assert!(d.w.run(&stale, &mut || true).is_some(), "the stale input holds (every cut past it)");
    let _ = d.fold();
    let (most, b) = (d.t.blocks_live().most_held(), bound(&d));
    assert!(most <= b, "stale: a hold touched {most}, bound {b} ({:?})", d.t.blocks_live().shape());
}

/// `h`'s board three ways, at each of its message boundaries from `from`
/// (a turn's start) on: streamed live a message at a time, each board the
/// landed prefix's there (every block's body, revision, pending and bound
/// value, every call's fate); landed whole at the end, before and after
/// the next board; and reattached at each boundary with the rest appended.
fn three_ways(h: &[Value], from: usize) {
    let want = landed(h);
    let mut d = Desk::of(attach(&h[..from]));
    for k in from..h.len() {
        stream_with(&mut d.t, &h[k..=k], k > from, false, false);
        assert_eq!(d.drawn(), landed(&h[..=k]), "live through message {k}");
    }
    d.t.apply_update(&HistoryUpdate::Replace { messages: h.to_vec() }).unwrap();
    assert_eq!(d.drawn(), want, "landed");
    for k in from..h.len() {
        let mut t = attach(&h[..k]);
        t.apply_update(&HistoryUpdate::Append { base_len: k, messages: h[k..].to_vec() }).unwrap();
        assert_eq!(Desk::of(t).drawn(), want, "reattached at {k}");
    }
}

/// The note `id`'s text, revision and pending flag where `d` draws it, and
/// its bound value's text (`None`: the bound board has none of it).
fn note_state(d: &BTreeMap<String, RowBlocks>, id: &str) -> Option<(String, u32, bool, Option<String>)> {
    let s = d.values().flat_map(|r| &r.blocks).find(|s| s.block.id.to_string() == id)?;
    let text = |b: &cards_spec::board::Block| match &b.body {
        cards_spec::block::Body::Note(n) => n.text.clone(),
        _ => panic!("a note"),
    };
    Some((text(&s.block), s.block.rev, s.block.pending, s.bound.as_deref().map(text)))
}

/// Shady #101 round 2, the first untested case: a block call the
/// extension refused, re-run with its input fixed, is another call with
/// facts of its own. The refused call stays unbound for good (a plain row,
/// none of its ops applied, nothing drawn at it); the fixed one binds and
/// its note is the board's. Live, landed and reattached alike, at every
/// boundary. And live, a response the provider streamed again (a reset)
/// before its result is not a result of the call it held: the call is gone
/// with its rows, and the re-streamed call alone binds.
#[test]
fn a_refused_call_rerun_with_its_input_fixed_binds_on_its_own() {
    let mut h = blocks_history();
    let from = h.len();
    let bad = json!([{"op":"put","id":"fix","kind":"note","body":{"text":"first try"}},{"op":"patch","id":"nope","set":{"text":"x"}}]);
    let good = json!([{"op":"put","id":"fix","kind":"note","body":{"text":"second try"}}]);
    let call = |id: &str, ops: &Value| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}]});
    let result = |id: &str, text: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]});
    h.push(json!({"role": "user", "content": "keep a note"}));
    h.push(call("toolu_bad", &bad));
    h.push(result("toolu_bad", "cards_block: refused: patch `nope`: no such block"));
    h.push(call("toolu_fix", &good));
    h.push(result("toolu_fix", &block_receipt(&good)));
    three_ways(&h, from);
    let d = landed(&h);
    assert_eq!(d["toolu_bad"].call, Some(Fate::Unbound), "refused: a plain row, for good");
    assert!(d["toolu_bad"].blocks.is_empty(), "nothing drawn at the refused call");
    assert_eq!(d["toolu_fix"].call, Some(Fate::Bound));
    assert_eq!(note_state(&d, "fix"), Some(("second try".into(), 1, false, Some("second try".into()))));
    // Between the two: the refused call's note was never the board's.
    assert_eq!(note_state(&landed(&h[..from + 3]), "fix"), None, "after the refusal: no note");
    // Live, a reset before the result: the re-streamed call alone.
    let mut d = Desk::of(attach(&h[..from]));
    stream(&mut d.t, &h[from..from + 1], false);
    d.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": "toolu_bad"})));
    d.apply(&tool_use("toolu_bad", "cards_block", block_input(bad.clone())));
    assert_eq!(note_state(&d.drawn(), "fix").map(|n| n.2), Some(true), "pending before the reset");
    d.apply(&llm(json!({"kind": "llm", "llm": "response_reset"})));
    d.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": "toolu_fix"})));
    d.apply(&tool_use("toolu_fix", "cards_block", block_input(good.clone())));
    d.apply(&tool_result("toolu_bad", "cards_block: refused: patch `nope`: no such block"));
    d.apply(&tool_result("toolu_fix", &block_receipt(&good)));
    let r = d.drawn();
    assert!(!r.contains_key("toolu_bad"), "the reset call's row is gone");
    assert_eq!(r["toolu_fix"].call, Some(Fate::Bound));
    let mut once = h[..from + 1].to_vec();
    once.push(call("toolu_fix", &good));
    once.push(result("toolu_fix", &block_receipt(&good)));
    assert_eq!(r, landed(&once), "the history that holds the re-streamed call alone");
}

/// Shady #101 round 2, the second untested case: a pending call whose
/// prerequisite's fate changes what it does. One response puts note `dep`
/// and patches it: both pending, the view draws the patch on the put, the
/// bound board has neither. The put binds first (live, results come one at
/// a time): the bound note is the put's, the view still the patch's on it.
/// Then, three ways: the patch binds too (the patch's text, bound); the
/// patch is refused (the put's text, bound); the put is refused (and so is
/// the patch, which the extension couldn't apply: no note at all). Each
/// checked live, landed and reattached at every boundary.
#[test]
fn a_pending_calls_prerequisite_decides_what_it_does() {
    let put = json!([{"op":"put","id":"dep","kind":"note","body":{"text":"v1"}}]);
    let patch = json!([{"op":"patch","id":"dep","set":{"text":"v2"}}]);
    let refused = "cards_block: refused (a test)";
    let round = json!({"role": "assistant", "content": [
        {"type": "tool_use", "id": "toolu_p", "name": "cards_block", "input": block_input(put.clone())},
        {"type": "tool_use", "id": "toolu_q", "name": "cards_block", "input": block_input(patch.clone())}]});
    let mut h = blocks_history();
    let from = h.len();
    h.push(json!({"role": "user", "content": "a note, then its fix"}));
    h.push(round);
    // Both pending.
    let mut d = Desk::of(attach(&h[..from]));
    stream(&mut d.t, &h[from..], false);
    let r = d.drawn();
    assert_eq!((r["toolu_p"].call, r["toolu_q"].call), (Some(Fate::Pending), Some(Fate::Pending)));
    assert_eq!(note_state(&r, "dep"), Some(("v2".into(), 2, true, None)), "the patch on the put, pending; none bound");
    // The put binds first.
    d.apply(&tool_result("toolu_p", &block_receipt(&put)));
    let r = d.drawn();
    assert_eq!((r["toolu_p"].call, r["toolu_q"].call), (Some(Fate::Bound), Some(Fate::Pending)));
    assert_eq!(note_state(&r, "dep"), Some(("v2".into(), 2, true, Some("v1".into()))), "the put's bound, the patch's pending");
    for (p, q, want) in [
        (block_receipt(&put), block_receipt(&patch), Some(("v2".to_string(), 2, false, Some("v2".to_string())))),
        (block_receipt(&put), refused.to_string(), Some(("v1".to_string(), 1, false, Some("v1".to_string())))),
        (refused.to_string(), refused.to_string(), None),
    ] {
        let mut h = h.clone();
        h.push(json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_p", "content": p},
            {"type": "tool_result", "tool_use_id": "toolu_q", "content": q}]}));
        three_ways(&h, from);
        assert_eq!(note_state(&landed(&h), "dep"), want);
    }
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
