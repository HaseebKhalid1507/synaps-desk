//! Agent cards in the transcript (synaps-cards `docs/design.md` v1.2, §2,
//! §9.3's D1 row): which rows are cards, answers and closes, and their
//! state, over the history (an attach, appends, any split of it) and the
//! live turn; the binding cases of §2.2; the fallbacks of §6.3; F14, a card
//! answered on another desk. Results are written as the `cards` extension
//! writes them (`cards_spec::Envelope`), answers as a desk sends them
//! (`cards_spec::message::build`).

use cards_spec::{CardId, Envelope, Input, Reply, message, parse_answer, parse_input};
use serde_json::{Value, json};
use synaps_client::cards::{CardRow, State};
use synaps_client::protocol::*;
use synaps_client::transcript::{ItemKind, Transcript};

const A: &str = "c4f9x2a";
const B: &str = "c81kq0z";

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

fn ask() -> Value {
    json!({"question": "Which approach?", "detail": "The parser is 2k lines.",
        "options": [{"label": "Refactor the parser", "hint": "half a day"}, "Rewrite from scratch", "Leave it"],
        "default": "Refactor the parser"})
}

fn ship() -> Value {
    json!({"question": "Ship it?", "options": ["Yes", "No"], "wait": true})
}

fn id(s: &str) -> CardId {
    CardId::parse(s).unwrap()
}

fn doc(input: &Value) -> (cards_spec::CardDoc, bool) {
    match parse_input(input).unwrap() {
        Input::Create { doc, wait } => (doc, wait),
        Input::Close(_) => unreachable!(),
    }
}

/// The result the extension writes for `input` under `card`.
fn showing(card: &str, input: &Value) -> String {
    let (d, wait) = doc(input);
    format!(
        "{}\n  1. Refactor the parser (suggested)\nIts answer will come as a user message starting \"[card {card}]\".",
        Envelope::showing(id(card), &d, wait)
    )
}

fn reply(input: &Value, values: Value) -> Reply {
    Reply::Answer(parse_answer(&doc(input).0, &values).unwrap())
}

/// The message a desk sends for `reply` to `card`.
fn said(card: &str, input: &Value, reply: &Reply) -> String {
    message::build(&id(card), &doc(input).0, reply)
}

fn call(tool: &str, name: &str, input: Value) -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": tool, "name": name, "input": input}]})
}

fn result(tool: &str, text: &str) -> Value {
    json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": text}]})
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text})
}

fn asked(tool: &str, card: &str, input: Value) -> [Value; 2] {
    let r = result(tool, &showing(card, &input));
    [call(tool, "cards_ui", input), r]
}

fn attach(messages: &[Value]) -> Transcript {
    let mut t = Transcript::new();
    t.load_attached(&attached(messages, false)).unwrap();
    t
}

/// Each row that draws a card: its kind, and what it draws.
fn cards(t: &Transcript) -> Vec<(ItemKind, CardRow)> {
    t.items().iter().filter_map(|i| i.card.as_deref().map(|c| (i.kind, c.clone()))).collect()
}

/// A card row's state, wait and answer count.
fn state(c: &CardRow) -> (State, bool, usize) {
    match c {
        CardRow::Ask(a) => (a.state, a.wait, a.answers),
        other => panic!("not a card: {other:?}"),
    }
}

fn question(c: &CardRow) -> &str {
    match c {
        CardRow::Ask(a) => &a.doc.question,
        CardRow::Answer(a) => &a.doc.question,
        CardRow::Close(c) => &c.doc.question,
    }
}

/// Every state of §2.3 from one history, and what each row draws.
#[test]
fn a_history_draws_every_state() {
    let refactor = reply(&ask(), json!({"choice": "Refactor the parser"}));
    let rewrite = Reply::Change(parse_answer(&doc(&ask()).0, &json!({"choice": "Rewrite from scratch"})).unwrap());
    let mut h = vec![user("Fix the parser.")];
    h.extend(asked("t1", A, ask())); // open
    h.extend(asked("t2", B, ship())); // open, the agent waits
    h.extend(asked("t3", "c000003", ask()));
    h.push(user(&said("c000003", &ask(), &refactor))); // answered
    h.extend(asked("t4", "c000004", ask()));
    h.push(user(&said("c000004", &ask(), &refactor)));
    h.push(user(&said("c000004", &ask(), &rewrite))); // changed: answered, rev 2
    h.extend(asked("t5", "c000005", ask()));
    h.push(user(&said("c000005", &ask(), &Reply::Skip))); // skipped
    h.extend(asked("t6", "c000006", ask()));
    h.push(call("t7", "cards_ui", json!({"close": "c000006"})));
    h.push(result("t7", "Card c000006 closed (cards/1)."));
    let t = attach(&h);
    let got = cards(&t);
    let asks: Vec<_> = got.iter().filter(|(_, c)| matches!(c, CardRow::Ask(_))).map(|(k, c)| (*k, state(c))).collect();
    assert_eq!(
        asks,
        [
            (ItemKind::Tool, (State::Open, false, 0)),
            (ItemKind::Tool, (State::Open, true, 0)),
            (ItemKind::Tool, (State::Answered, false, 1)),
            (ItemKind::Tool, (State::Answered, false, 2)),
            (ItemKind::Tool, (State::Skipped, false, 1)),
            (ItemKind::Tool, (State::Closed, false, 0)),
        ]
    );
    let CardRow::Ask(changed) = &got.iter().filter(|(_, c)| matches!(c, CardRow::Ask(_))).nth(3).unwrap().1 else { panic!() };
    assert_eq!(changed.reply.as_ref(), Some(&rewrite), "the latest answer wins");
    // The answers' messages draw as answers; the close's call as a close.
    let others: Vec<_> = got.iter().filter(|(_, c)| !matches!(c, CardRow::Ask(_))).collect();
    assert_eq!(others.len(), 5);
    assert!(others[..4].iter().all(|(k, c)| *k == ItemKind::User && matches!(c, CardRow::Answer(_))));
    assert!(matches!(others[4], (ItemKind::Tool, CardRow::Close(_))));
    assert_eq!(question(&others[4].1), "Which approach?");
    let CardRow::Answer(skip) = &others[3].1 else { panic!() };
    assert_eq!(skip.reply, Reply::Skip);
    // Ordinary text stays ordinary.
    assert!(t.items()[0].card.is_none());
}

/// §2.2: what binds, and what is an ordinary tool row (the fail-safe).
#[test]
fn only_a_bound_card_draws() {
    let (d, _) = doc(&ask());
    let bound = Envelope::showing(id(A), &d, false).to_string();
    // A hook rewrote the input before the tool ran: the digest is the
    // extension's doc, the transcript keeps the model's.
    let rewritten = Envelope::showing(id(A), &doc(&json!({"question": "Something else?", "options": ["a", "b"]})).0, false).to_string();
    let cases: [(&str, Value, String, Option<State>); 10] = [
        ("bound", ask(), bound.clone(), Some(State::Open)),
        ("the runtime name", ask(), bound.clone(), Some(State::Open)),
        ("a result rewritten, line 1 kept", ask(), format!("{bound}\nsomething a hook wrote"), Some(State::Open)),
        ("the input rewritten", ask(), rewritten, None),
        (
            "wait flipped",
            json!({"question": "Which approach?", "detail": "The parser is 2k lines.",
            "options": [{"label": "Refactor the parser", "hint": "half a day"}, "Rewrite from scratch", "Leave it"],
            "default": "Refactor the parser", "wait": true}),
            bound.clone(),
            None,
        ),
        ("an error", ask(), "cards_ui: 6 cards in the last 10 minutes; ask in plain text".into(), None),
        // A transformer cut it in the digest (its question may be cut).
        ("line 1 cut", ask(), bound[..bound.find('#').unwrap() + 9].to_string(), None),
        ("an unknown version", ask(), bound.replace("(cards/1 ", "(cards/2 "), None),
        // A newer extension's parameter (§6.3): the input doesn't validate.
        ("a newer catalog", json!({"question": "Which?", "options": ["a", "b"], "layout": "grid"}), bound.clone(), None),
        ("a collision suffix", ask(), bound.clone(), None),
    ];
    for (what, input, text, want) in cases {
        let name = match what {
            "the runtime name" => "cards:ui",
            "a collision suffix" => "cards_ui_2",
            _ => "cards_ui",
        };
        let t = attach(&[call("t1", name, input), result("t1", &text)]);
        let row = &t.items()[0];
        assert_eq!(row.kind, ItemKind::Tool, "{what}");
        assert_eq!(row.card.as_deref().map(|c| state(c).0), want, "{what}");
        // A plain row is a tool row with its result, as before.
        assert_eq!(row.tool.as_ref().unwrap().result, text, "{what}");
    }
}

/// The live turn: no card while the input streams, pending once it is
/// whole, open once the result binds; the history landing changes nothing.
#[test]
fn a_live_card_goes_pending_then_open_and_the_history_keeps_it() {
    let mut t = attach(&[user("Fix the parser.")]);
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": null})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": "t1"})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_use_delta", "tool_id": "t1", "delta": "{\"question\":"})));
    assert!(cards(&t).is_empty(), "its input is still streaming");
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "t1", "input": ask()})));
    assert_eq!(cards(&t).iter().map(|(_, c)| state(c)).collect::<Vec<_>>(), [(State::Pending, false, 0)]);
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "t1", "result": showing(A, &ask())})));
    assert_eq!(cards(&t).iter().map(|(_, c)| state(c)).collect::<Vec<_>>(), [(State::Open, false, 0)]);
    let live = t.items().iter().find(|i| i.card.is_some()).map(|i| (i.id, i.rev)).unwrap();
    // The turn ends; its history lands with the same call and result.
    let mut h = vec![user("Fix the parser.")];
    h.extend(asked("t1", A, ask()));
    t.apply(&ev(json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": h}})));
    t.apply(&ev(json!({"ev": "idle"})));
    let card = t.items().iter().find(|i| i.card.is_some()).unwrap();
    assert_eq!(state(card.card.as_deref().unwrap()), (State::Open, false, 0));
    assert_eq!((card.id, card.rev), live, "the same row, unchanged: nothing redraws");
}

/// A card call's input whole, live: the tool row's preview stops at
/// 4 KiB, and a card's input may be 8 KiB (§2.1).
#[test]
fn a_live_card_reads_its_whole_input() {
    // 1478 characters of detail (2 bytes each), nine hinted options.
    let detail = vec!["é".repeat(50); 29].join("\n");
    let options: Vec<Value> = (1..=9).map(|n| json!({"label": format!("Option {n}"), "hint": "x".repeat(118)})).collect();
    let input = json!({"question": "Which approach?", "detail": detail, "options": options});
    assert!(input.to_string().len() > 4096 && input.to_string().len() < 8192);
    let mut t = attach(&[user("go")]);
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": null})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "t1", "input": input})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "t1", "result": showing(A, &input)})));
    let row = t.items().iter().find(|i| i.kind == ItemKind::Tool).unwrap();
    assert!(row.tool.as_ref().unwrap().input.ends_with('…'), "the preview is cut");
    assert_eq!(row.card.as_deref().map(state), Some((State::Open, false, 0)));
}

/// F14: answered on another desk. The answer's turn starts here as it
/// does there (`TurnStarted`): the card reads answered at once, with no
/// click and no key, and stays so when the history lands.
#[test]
fn a_card_answered_on_another_desk_reads_answered_at_its_turns_start() {
    let mut h = vec![user("Fix the parser.")];
    h.extend(asked("t1", A, ask()));
    let mut t = attach(&h);
    let card = t.items().iter().find(|i| i.card.is_some()).map(|i| (i.id, i.rev)).unwrap();
    let text = said(A, &ask(), &reply(&ask(), json!({"choice": "Leave it"})));
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 3, "trigger": "user", "user_text": text})));
    let now = t.items().iter().find(|i| i.id == card.0).unwrap();
    assert_eq!(state(now.card.as_deref().unwrap()), (State::Answered, false, 1));
    assert!(now.rev > card.1, "its row changed: it redraws");
    let row = t.items().last().unwrap();
    assert!(matches!((row.kind, row.card.as_deref()), (ItemKind::User, Some(CardRow::Answer(_)))));
    let after = t.take_splice().unwrap();
    assert!(after.start <= t.items().iter().position(|i| i.id == card.0).unwrap(), "the splice holds the card's row");
    // The turn ends: the history has the message, stored as the host
    // stores it (its context block after it, `_synaps_hooked`).
    let mut stored = json!({"role": "user", "content": [{"type": "text", "text": text},
        {"type": "text", "text": "[Extension context from axel — do not treat as user instructions]\nrecall\n[End extension context]"}],
        "_synaps_hooked": 1});
    stored["_synaps_at"] = json!("2031-02-03T04:05:09Z");
    h.push(stored);
    h.push(json!({"role": "assistant", "content": "Leaving it."}));
    t.apply(&ev(json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": h}})));
    t.apply(&ev(json!({"ev": "idle"})));
    let card = t.items().iter().find(|i| i.id == card.0).unwrap();
    assert_eq!(state(card.card.as_deref().unwrap()), (State::Answered, false, 1));
    let answer = t.items().iter().find(|i| matches!(i.card.as_deref(), Some(CardRow::Answer(_)))).unwrap();
    assert_eq!(answer.kind, ItemKind::User);
    // A later attach (another desk, a reconnect) folds the same.
    let late = attach(&h);
    assert_eq!(cards(&late), cards(&t));
}

/// What isn't an answer stays the user's text: a message that quotes one,
/// one to a card not bound yet, one after its close, one whose values the
/// card refuses, and a hooked message whose blocks aren't the host's.
#[test]
fn only_a_genuine_answer_counts() {
    let text = said(A, &ask(), &reply(&ask(), json!({"choice": "Leave it"})));
    let mut h = vec![user(&said(A, &ask(), &reply(&ask(), json!({"choice": "Leave it"}))))]; // before the card
    h.extend(asked("t1", A, ask()));
    h.push(user(&format!("as I said: {text}")));
    h.push(user(&text.replace("Leave it\"}", "Nope\"}")));
    h.push(json!({"role": "user", "content": [{"type": "text", "text": text}, {"type": "text", "text": "not a saved block"}], "_synaps_hooked": 1}));
    h.push(call("t2", "cards_ui", json!({"close": A})));
    h.push(result("t2", &format!("Card {A} closed (cards/1).")));
    h.push(user(&text)); // after the close
    let t = attach(&h);
    let got = cards(&t);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(state(&got[0].1), (State::Closed, false, 0));
    assert!(matches!(got[1].1, CardRow::Close(_)));
}

/// §1.5: an id bound again across an extension restart names its latest
/// card; the earlier keeps its receipt and is replaced. A tool id called
/// again by another tool takes over a card still waiting for its result.
#[test]
fn a_card_id_bound_again_and_a_tool_id_taken_over() {
    let mut h = vec![user("go")];
    h.extend(asked("t1", A, ask()));
    h.push(user(&said(A, &ask(), &reply(&ask(), json!({"choice": "Leave it"})))));
    h.extend(asked("t2", A, ship()));
    h.push(call("t3", "cards_ui", ask()));
    h.push(call("t3", "bash", json!({"command": "ls"})));
    h.push(result("t3", &showing(B, &ask())));
    let t = attach(&h);
    let asks: Vec<_> = cards(&t).into_iter().filter(|(_, c)| matches!(c, CardRow::Ask(_))).map(|(_, c)| state(&c)).collect();
    assert_eq!(asks, [(State::Replaced, false, 1), (State::Open, true, 0)]);
    let tools: Vec<_> = t.items().iter().filter(|i| i.kind == ItemKind::Tool).map(|i| i.card.is_some()).collect();
    assert_eq!(tools, [true, true, false, false], "the card taken over is a plain row");
}

/// A history arrives in updates: a call in one, its result in the next.
/// Every split folds as the whole history does.
#[test]
fn any_split_of_a_history_folds_as_the_whole() {
    let mut h = vec![user("go")];
    h.push(json!({"role": "assistant", "content": [
        {"type": "text", "text": "Two questions."},
        {"type": "tool_use", "id": "t1", "name": "cards_ui", "input": ask()},
        {"type": "tool_use", "id": "t2", "name": "bash", "input": {"command": "ls"}},
        {"type": "tool_use", "id": "t3", "name": "cards:ui", "input": ship()}]}));
    h.push(json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": showing(A, &ask())}]},
        {"type": "tool_result", "tool_use_id": "t2", "content": "a\nb"},
        {"type": "tool_result", "tool_use_id": "t3", "content": showing(B, &ship())}]}));
    h.push(user(&said(B, &ship(), &reply(&ship(), json!({"choice": "Yes"})))));
    h.push(call("t4", "cards_ui", json!({"close": A})));
    h.push(result("t4", &format!("Card {A} closed (cards/1).")));
    let whole = cards(&attach(&h));
    assert_eq!(whole.len(), 4);
    for k in 1..h.len() {
        let mut t = attach(&h[..k]);
        t.apply(&ev(json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": h}})));
        assert_eq!(cards(&t), whole, "split at {k}");
    }
}

/// A turn retried mid-stream (`response_reset`) takes its rows back, and
/// with them its card.
#[test]
fn a_reset_response_takes_its_card_back() {
    let mut t = attach(&[user("go")]);
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": null})));
    t.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "t1", "input": ask()})));
    assert_eq!(cards(&t).len(), 1);
    t.apply(&llm(json!({"kind": "llm", "llm": "response_reset"})));
    assert!(cards(&t).is_empty());
    assert!(t.items().iter().all(|i| i.card.is_none()));
}

/// A live card whose answer lands in the same history update as its call
/// (the turn ended while this desk wasn't drawing): its row redraws.
#[test]
fn a_card_answered_in_the_history_that_lands_redraws() {
    let mut t = attach(&[user("go")]);
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": null})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "t1", "input": ask()})));
    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "t1", "result": showing(A, &ask())})));
    let before = t.items().iter().find(|i| i.card.is_some()).map(|i| (i.id, i.rev)).unwrap();
    let mut h = vec![user("go")];
    h.extend(asked("t1", A, ask()));
    h.push(user(&said(A, &ask(), &reply(&ask(), json!({"choice": "Leave it"})))));
    t.apply(&ev(json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": h}})));
    let row = t.items().iter().find(|i| i.id == before.0).expect("the same row");
    assert_eq!(state(row.card.as_deref().unwrap()), (State::Answered, false, 1));
    assert!(row.rev > before.1, "its card changed: its rev did");
}

/// A history replaced (a compaction, `/clear`, a resync): the cards are
/// the new history's only; a card no longer in it is gone (§2.3).
#[test]
fn a_replaced_history_takes_its_cards_with_it() {
    let mut h = vec![user("go")];
    h.extend(asked("t1", A, ask()));
    h.extend(asked("t2", B, ship()));
    let mut t = attach(&h);
    assert_eq!(cards(&t).len(), 2);
    let summary = vec![user("<context-summary>Asked two questions.</context-summary>"), json!({"role": "assistant", "content": "Noted."})];
    t.apply_update(&synaps_client::HistoryUpdate::Replace { messages: summary.clone() }).unwrap();
    assert!(cards(&t).is_empty(), "{:?}", cards(&t));
    // And one asked after it is the only one.
    let mut after = summary;
    after.extend(asked("t3", A, ship()));
    t.apply_update(&synaps_client::HistoryUpdate::Append { base_len: 2, messages: after[2..].to_vec() }).unwrap();
    assert_eq!(cards(&t).iter().map(|(_, c)| state(c)).collect::<Vec<_>>(), [(State::Open, true, 0)]);
}
