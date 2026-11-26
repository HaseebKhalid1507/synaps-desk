//! The card sync reads what changed (`Transcript::sync_cards`): a live call
//! costs it the rows it touched, not every live row; and over any stream
//! it draws what reading every row at every event draws.

use cards_spec::{CardId, Envelope, Input, Reply, message, parse_answer, parse_input};
use serde_json::{Value, json};

use super::*;
use crate::cards::{CardRow, State};
use crate::protocol::{Attached, DaemonFrame, SessionEvent, decode_event, decode_frame};
use crate::synthetic::Rng;

const TOOLS: [&str; 6] = ["t0", "t1", "t2", "t3", "t4", "t5"];
const CARDS: [&str; 3] = ["c4f9x2a", "c81kq0z", "c2m7p4d"];

fn ev(v: Value) -> SessionEvent {
    decode_event(&v).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn llm(v: Value) -> SessionEvent {
    ev(json!({"ev": "stream", "event": v}))
}

fn attached(messages: &[Value], streaming: bool, journal: &str, replay: &[Value]) -> Attached {
    let mut a: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/tests/fixtures/attached_history.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    a["conversation"]["api_messages"] = json!(messages);
    a["conversation"]["messages_len"] = json!(messages.len());
    a["conversation"]["pending_events_len"] = json!(0);
    a["conversation"]["header"]["id"] = json!(journal);
    let sid = a["meta"]["id"].clone();
    a["replay"] = replay.iter().enumerate().map(|(k, e)| json!({"session_id": sid, "seq": k + 1, "ts": "t", "event": e})).collect();
    a["streaming"] = json!(streaming);
    let DaemonFrame::Attached(a) = decode_frame(&a.to_string()).unwrap() else { panic!() };
    *a
}

fn ask() -> Value {
    json!({"question": "Which approach?", "options": ["Refactor the parser", "Rewrite from scratch", "Leave it"]})
}

fn ship() -> Value {
    json!({"question": "Ship it?", "options": ["Yes", "No"], "wait": true})
}

fn doc(input: &Value) -> Option<(cards_spec::CardDoc, bool)> {
    match parse_input(input) {
        Ok(Input::Create { doc, wait }) => Some((doc, wait)),
        _ => None,
    }
}

fn card(s: &str) -> CardId {
    CardId::parse(s).unwrap()
}

/// The result the extension writes for `input` under `id`.
fn showing(id: &str, input: &Value) -> String {
    let (d, wait) = doc(input).unwrap();
    format!("{}\n  1. A\nIts answer will come…", Envelope::showing(card(id), &d, wait))
}

/// A desk's answer to card `id`, asked with `input`.
fn said(id: &str, input: &Value) -> String {
    let (d, _) = doc(input).unwrap();
    let choice = if input == &ship() { "Yes" } else { "Leave it" };
    message::build(&card(id), &d, &Reply::Answer(parse_answer(&d, &json!({ "choice": choice })).unwrap()))
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text})
}

/// A history with one card, bound, and a call of another tool.
fn with_a_card() -> Vec<Value> {
    vec![
        user("Fix the parser."),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "h1", "name": "cards_ui", "input": ask()},
            {"type": "tool_use", "id": "h2", "name": "bash", "input": {"command": "ls"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "h1", "content": showing(CARDS[0], &ask())},
            {"type": "tool_result", "tool_use_id": "h2", "content": "a b"}]}),
    ]
}

fn attach(h: &[Value]) -> Transcript {
    let mut t = Transcript::new();
    t.load_attached(&attached(h, false, "20261113-073427-5fde", &[])).unwrap();
    t
}

fn call(name: &str, id: &str, input: Value) -> [SessionEvent; 2] {
    [
        llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": name, "tool_id": id})),
        llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input})),
    ]
}

fn result(id: &str, text: &str) -> SessionEvent {
    llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": text}))
}

fn turn(text: Option<&str>) -> SessionEvent {
    ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": text}))
}

/// A live call costs the card sync the rows it changed: on a history with
/// a card, 100, 1,000 and 4,000 calls of another tool (each starts, is
/// whole, starts running, streams, reports), five cards among them
/// (asked, bound) and one answered, every event reads a few rows at most,
/// whatever came before it.
#[test]
fn a_live_call_costs_the_card_sync_only_its_rows() {
    for n in [100usize, 1_000, 4_000] {
        let mut t = attach(&with_a_card());
        let (mut worst, mut events) = (0, 0u64);
        let v0 = t.card_visits;
        let mut each = |t: &mut Transcript, e: &SessionEvent| {
            let v = t.card_visits;
            t.apply(e);
            worst = worst.max(t.card_visits - v);
            events += 1;
        };
        each(&mut t, &turn(Some("go")));
        for k in 0..n {
            let id = format!("l{k}");
            for e in call("bash", &id, json!({"command": "ls"})) {
                each(&mut t, &e);
            }
            each(&mut t, &llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": id})));
            each(&mut t, &llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": id, "delta": "a"})));
            each(&mut t, &result(&id, "a b"));
            if k % (n / 5) == n / 10 {
                let c = format!("c{k}");
                for e in call("cards_ui", &c, ship()) {
                    each(&mut t, &e);
                }
                each(&mut t, &result(&c, &showing(CARDS[1], &ship())));
            }
        }
        // The first card answered, live (the turn that sends it).
        each(&mut t, &turn(Some(&said(CARDS[0], &ask()))));
        let total = t.card_visits - v0;
        assert_eq!(t.cards().cards().len(), 6, "{n}: every card");
        assert!(t.cards().bound(&card(CARDS[0])).is_some_and(|c| c.answers.len() == 1), "{n}: the answer counted");
        assert!(worst <= 24, "{n}: an event read {worst} rows");
        assert!(total <= 3 * events, "{n}: {total} rows read for {events} events");
    }
}

/// A live `External` frame: one of a few, each the same every time.
fn external(r: &mut Rng) -> Value {
    let k = r.below(3);
    json!({"ev": "external", "event": {
        "id": format!("ev-{k}"), "timestamp": format!("2031-02-03T04:05:0{k}Z"),
        "source": {"source_type": "agent-comms", "name": "agent-comms", "callback": null},
        "channel": {"id": "comms", "name": "comms"},
        "sender": {"id": "sid-0001", "name": "kestrel", "sender_type": "agent"},
        "content": {"text": format!("ping {k}"), "content_type": "message", "severity": "low", "data": null,
            "disclosure": if k == 2 { json!("never_persist") } else { Value::Null }},
        "expects_response": false, "reply_to": null}})
}

/// What one step of a random stream does.
fn step(r: &mut Rng, hist: &mut Vec<Value>, inputs: &mut HashMap<String, Value>) -> Op {
    let pick = |r: &mut Rng, xs: &[&str]| xs[r.below(xs.len() as u64) as usize].to_string();
    let tool = pick(r, &TOOLS);
    let name = pick(r, &["cards_ui", "cards_ui", "cards_ui", "bash", "bash", "cards:ui", "cards_ui_2"]);
    let id = pick(r, &CARDS);
    let input = match r.below(6) {
        0 | 1 => ask(),
        2 => ship(),
        3 => json!({ "close": id }),
        4 => json!({"question": 1}),
        _ => json!({"command": "ls"}),
    };
    // A result for `tool`, as its input would have it.
    let answer_for = |r: &mut Rng, input: Option<&Value>| match (input, r.below(4)) {
        (Some(i), 0..=1) if doc(i).is_some() => showing(&id, i),
        (Some(i), 0..=1) if i.get("close").is_some() => Envelope::closed(card(i["close"].as_str().unwrap())).to_string(),
        (_, 2) => showing(&id, &ask()),
        _ => "ok".to_string(),
    };
    let user_text = |r: &mut Rng| match r.below(3) {
        0 => said(&id, &ask()),
        1 => said(&id, &ship()),
        _ => "go on".to_string(),
    };
    match r.below(40) {
        0..=2 => {
            let text = user_text(r);
            Op::Event(turn((r.below(3) != 0).then_some(text.as_str())))
        }
        3 => Op::Event(llm(json!({"kind": "llm", "llm": "response_start"}))),
        4 => Op::Event(llm(json!({"kind": "llm", "llm": "response_reset"}))),
        5 | 6 => Op::Event(llm(json!({"kind": "llm", "llm": "text", "text": "x"}))),
        7..=9 => Op::Event(llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": name, "tool_id": tool}))),
        10 => Op::Event(llm(json!({"kind": "llm", "llm": "tool_use_delta", "tool_id": tool, "delta": "{"}))),
        11..=14 => {
            inputs.insert(tool.clone(), input.clone());
            Op::Event(llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": tool, "input": input})))
        }
        15 => Op::Event(llm(json!({"kind": "llm", "llm": "tool_queued", "tool_id": tool}))),
        16 => Op::Event(llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": tool}))),
        17 => Op::Event(llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": tool, "delta": "y"}))),
        18..=21 => Op::Event(result(&tool, &answer_for(r, inputs.get(&tool)))),
        22 => Op::Event(ev(json!({"ev": "idle"}))),
        23 => Op::Event(ev(json!({"ev": "aborted"}))),
        // An event (again: its row, delivered, goes last again; the rows
        // under it move up).
        24 => Op::Event(ev(external(r))),
        // A gap: events dropped, the history asked for again.
        25 => Op::Event(ev(json!({"ev": "system_notice", "text": "event stream lagged; 3 dropped"}))),
        26 => Op::Event(ev(json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": user_text(r)}}))),
        // The history grows (the turn's messages land), or is replaced
        // (a resync, a rewrite): shorter, or another one.
        27..=31 => {
            for _ in 0..=r.below(3) {
                let m = match r.below(4) {
                    0 => user(&user_text(r)),
                    1 => {
                        inputs.insert(tool.clone(), input.clone());
                        json!({"role": "assistant", "content": [{"type": "text", "text": "ok"},
                            {"type": "tool_use", "id": tool, "name": name, "input": input}]})
                    }
                    2 => json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool,
                        "content": answer_for(r, inputs.get(&tool))}]}),
                    _ => json!({"role": "assistant", "content": "done"}),
                };
                hist.push(m);
            }
            Op::History(hist.clone())
        }
        32 | 33 => {
            let keep = r.below(hist.len() as u64 + 1) as usize;
            hist.truncate(keep);
            Op::Replace(hist.clone())
        }
        34 | 35 => {
            let journal = if r.below(4) == 0 { "another-journal" } else { "20261113-073427-5fde" };
            let replay: Vec<Value> = (0..r.below(4))
                .map(|_| match r.below(4) {
                    3 => external(r),
                    0 => json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": tool, "input": input}}),
                    1 => json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": answer_for(r, Some(&input))}}),
                    _ => json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": user_text(r)}),
                })
                .collect();
            Op::Attach(hist.clone(), r.below(2) == 0, journal.to_string(), replay)
        }
        36 => Op::Detach,
        37 => Op::Trim(r.below(6) as usize),
        _ => Op::Event(turn(Some(&user_text(r)))),
    }
}

enum Op {
    Event(SessionEvent),
    History(Vec<Value>),
    Replace(Vec<Value>),
    Attach(Vec<Value>, bool, String, Vec<Value>),
    Detach,
    Trim(usize),
}

fn run(t: &mut Transcript, op: &Op) {
    match op {
        Op::Event(e) => t.apply(e),
        Op::History(h) => t.apply(&ev(json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": h}}))),
        Op::Replace(h) => t.apply_update(&crate::history::HistoryUpdate::Replace { messages: h.clone() }).unwrap(),
        Op::Attach(h, streaming, journal, replay) => t.load_attached(&attached(h, *streaming, journal, replay)).unwrap(),
        Op::Detach => t.set_detached(),
        Op::Trim(n) => {
            t.trim_live_front(*n, usize::MAX);
        }
    }
}

/// What the rows draw and what the cards say, as a reader sees them.
fn seen(t: &mut Transcript) -> String {
    let splice = t.take_splice();
    let f = t.cards();
    let calls: Vec<_> = TOOLS.iter().map(|id| (f.card(id), f.close(id))).collect();
    let bound: Vec<_> = CARDS.iter().map(|id| f.bound(&card(id))).collect();
    format!("{:?}\n{:?}\n{calls:?}\n{bound:?}\n{:?}\n{splice:?}", t.items, f.cards(), (t.card_rows, t.card_times))
}

/// `ops` on a transcript attached to `h`, the sync that reads only what
/// changed against reading every row, at every step.
fn both(h: &[Value], ops: &[Op], what: &str) -> Transcript {
    let mut a = attach(h);
    let mut b = a.clone();
    for (k, op) in ops.iter().enumerate() {
        run(&mut a, op);
        CARD_REREAD.set(true);
        run(&mut b, op);
        CARD_REREAD.set(false);
        assert_eq!(seen(&mut a), seen(&mut b), "{what}, step {k}");
    }
    a
}

/// A delivered event sent again goes last again, and the rows under it
/// move up: a call changed before it moved (its result streamed on,
/// binding now), or a card answered after, is read where it is now.
#[test]
fn a_row_that_moved_is_read_where_it_is() {
    let e = || {
        let mut r = Rng::new(1);
        Op::Event(ev(external(&mut r)))
    };
    let shown = showing(CARDS[1], &ship());
    let (head, rest) = shown.split_at(10);
    let mut ops = vec![e(), Op::Event(turn(Some("go")))];
    ops.extend(call("cards_ui", "x1", ship()).map(Op::Event));
    ops.push(Op::Event(result("x1", head)));
    ops.push(Op::Event(llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": "x1", "delta": rest}))));
    ops.push(e());
    let t = both(&with_a_card(), &ops, "its result streamed on");
    let x1 = t.items().iter().find(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == "x1")).unwrap();
    assert!(matches!(x1.card.as_deref(), Some(CardRow::Ask(a)) if a.state == State::Open), "it binds now");

    let mut ops = vec![e(), Op::Event(turn(Some("go")))];
    ops.extend(call("cards_ui", "x1", ship()).map(Op::Event));
    ops.push(Op::Event(result("x1", &shown)));
    ops.push(Op::Event(ev(json!({"ev": "idle"}))));
    ops.push(e());
    ops.push(Op::Event(turn(Some(&said(CARDS[1], &ship())))));
    let t = both(&with_a_card(), &ops, "answered after");
    let x1 = t.items().iter().find(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == "x1")).unwrap();
    assert!(matches!(x1.card.as_deref(), Some(CardRow::Ask(a)) if a.answers == 1), "the answer is its");
}

/// Over random streams (calls of cards and of other tools, results that
/// bind or not, answers, closes, a tool id called again, reset responses,
/// events, gaps, history appends and replacements, re-attaches with a
/// replay, detaches, trims), the sync that reads only what changed draws
/// at every step what reading every row draws: the same rows (cards,
/// revisions, the splice), the same fold, the same times, and the fold's
/// revision moves at the same steps.
#[test]
fn reading_what_changed_draws_what_reading_every_row_draws() {
    let (mut kept, mut full) = (0, 0);
    for seed in 1..=300u64 {
        let mut r = Rng::new(seed);
        let mut hist = if seed % 3 == 0 { vec![user("go")] } else { with_a_card() };
        let mut inputs = HashMap::new();
        let mut a = attach(&hist);
        // A worker's transcript (no history): what a trim cuts.
        if seed % 7 == 0 {
            a = Transcript::new();
            hist.clear();
        }
        let mut b = a.clone();
        let mut revs = (a.cards_rev(), b.cards_rev());
        for k in 0..120 {
            let op = step(&mut r, &mut hist, &mut inputs);
            if seed % 7 == 0 && matches!(op, Op::History(_) | Op::Replace(_) | Op::Attach(..)) {
                continue;
            }
            run(&mut a, &op);
            // Every row read again, at every sync.
            CARD_REREAD.set(true);
            run(&mut b, &op);
            CARD_REREAD.set(false);
            assert_eq!(seen(&mut a), seen(&mut b), "seed {seed}, step {k}");
            assert_eq!(a.cards_rev() != revs.0, b.cards_rev() != revs.1, "seed {seed}, step {k}: the fold's revision");
            revs = (a.cards_rev(), b.cards_rev());
        }
        kept += a.card_visits;
        full += b.card_visits;
    }
    // Not the same sync twice: this one read far fewer rows.
    assert!(kept * 3 < full, "{kept} rows read, {full} by reading them all");
}
