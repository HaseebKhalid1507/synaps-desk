//! Agent cards in the window (synaps-cards `docs/design.md` §6.1, §9.3's
//! D1 row): every state drawn as its card or its receipt, an answer's
//! message as its slim row, a close as its run row, no card folded into a
//! run; a live card from its call to an answer from another desk (F14);
//! and the fallback of §6.3, a call this desk can't read drawn as a tool
//! row. Read-only: nothing here clicks or types.
//!
//! The demo's [`crate::demo::ASKS`] session holds a card in each state.
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`).

use std::time::Duration;

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, ListOffset, TestAppContext, VisualTestContext, px};
use serde_json::{Value, json};
use synaps_client::cards::{CardRow, State};
use synaps_client::protocol::decode_event;

use super::{open_at, settle};
use crate::app::AppView;
use crate::turns::Kind;

type Window = (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>);

/// The demo's window, `h` px tall, showing [`crate::demo::ASKS`].
fn asks(cx: &mut TestAppContext, h: f32) -> Window {
    session(cx, h, crate::demo::ASKS)
}

/// The demo's window, `h` px tall, showing its session `id`.
fn session(cx: &mut TestAppContext, h: f32, id: &str) -> Window {
    let (w, view, events) = open_at(cx, (1440., h), crate::theme::Panels::default().text_size);
    view.update(cx, |v, cx| {
        v.client = None;
        let open = crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false };
        v.open_tab(open, crate::tabs::Place::Current);
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    (w, view, events)
}

fn has(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel)).is_some()
}

/// Display row `ix` at the view's top, drawn.
fn show(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ix: usize) {
    view.update(cx, |v, cx| {
        v.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
}

/// A row of the list and what its item draws of the cards.
#[derive(Clone, Debug)]
struct Row {
    ix: usize,
    id: u64,
    kind: Kind,
    card: Option<CardRow>,
}

fn rows(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<Row> {
    view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        v.disp
            .iter()
            .enumerate()
            .map(|(ix, d)| Row { ix, id: d.id, kind: d.kind, card: items.get(d.item).and_then(|i| i.card.as_deref().cloned()) })
            .collect()
    })
}

fn ask(r: &Row) -> &synaps_client::cards::Ask {
    match &r.card {
        Some(CardRow::Ask(a)) => a,
        other => panic!("row {} is no card: {other:?}", r.ix),
    }
}

/// Every state the demo holds draws as itself: open cards whole (their
/// options numbered, the agent's default `suggested`, a footer that says
/// how to answer, `NEEDS YOU` only where the agent waits), the others as
/// their one-line receipts with what was answered; every answer's message
/// as its slim row; the close as a run row naming the card it closed. No
/// card sits in a run, so none folds away; and nothing asks for a frame.
#[gpui_kit::test]
fn every_card_state_draws_as_its_card_or_its_receipt(cx: &mut TestAppContext) {
    let (w, view, _events) = asks(cx, 900.);
    let all = rows(cx, &view);
    let cards: Vec<&Row> = all.iter().filter(|r| r.kind == Kind::Ask).collect();
    let states: Vec<(State, usize, bool)> = cards.iter().map(|r| (ask(r).state, ask(r).answers, ask(r).wait)).collect();
    assert_eq!(
        states,
        [
            (State::Answered, 1, false),
            (State::Answered, 2, false),
            (State::Skipped, 1, false),
            (State::Closed, 0, false),
            (State::Replaced, 1, false),
            (State::Open, 0, false),
            (State::Open, 0, true),
        ]
    );
    let words = ["answered", "changed", "skipped", "closed", "replaced", "open", "the agent is waiting"];
    for (r, word) in cards.iter().zip(words) {
        show(cx, w, &view, r.ix);
        let (id, a) = (r.id, ask(r));
        assert!(has(cx, w, format!("ask-state {id} {word}")), "card {id}: {word}");
        let open = a.state == State::Open;
        assert_eq!(has(cx, w, format!("ask-card {id}")), open, "card {id} drawn whole");
        assert_eq!(has(cx, w, format!("ask-receipt {id}")), !open, "card {id} as its receipt");
        assert_eq!(has(cx, w, format!("ask-needs {id}")), open && a.wait, "card {id}: NEEDS YOU only where the agent waits");
        if open {
            for (n, o) in a.doc.options.iter().enumerate() {
                let n = n + 1;
                assert!(has(cx, w, format!("ask-option {id} {n}")), "card {id}: option {n}");
                let default = match &a.doc.default {
                    Some(synaps_client::cards::DefaultChoice::One(l)) => *l == o.label,
                    Some(synaps_client::cards::DefaultChoice::Many(ls)) => ls.contains(&o.label),
                    None => false,
                };
                assert_eq!(has(cx, w, format!("ask-suggested {id} {n}")), default, "card {id}: option {n} suggested");
            }
            assert!(has(cx, w, format!("ask-footer {id}")), "card {id}: how to answer");
            assert_eq!(has(cx, w, format!("ask-detail {id}")), a.doc.detail.is_some(), "card {id}: its detail");
        } else {
            // What was answered, if anything was: a skip and a close
            // without an answer say only their state.
            let said = matches!(&a.reply, Some(synaps_client::cards::Reply::Answer(_) | synaps_client::cards::Reply::Change(_)));
            assert_eq!(has(cx, w, format!("ask-said {id}")), said, "card {id}: what was answered");
        }
    }
    // The answers' messages: one slim row each, never their two lines.
    let answers: Vec<&Row> = all.iter().filter(|r| r.kind == Kind::Answer).collect();
    assert_eq!(answers.len(), 5);
    assert!(all.iter().all(|r| r.kind != Kind::User || r.card.is_none()));
    for r in &answers {
        show(cx, w, &view, r.ix);
        assert!(has(cx, w, format!("ask-answer {}", r.id)), "answer row {}", r.id);
    }
    // The close: a row of its run, which says the card it closed.
    let close = view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        let ix = items.iter().position(|i| matches!(i.card.as_deref(), Some(CardRow::Close(_)))).expect("the close's row");
        let d = v.disp.iter().find(|d| d.item == ix).copied().expect("drawn");
        (d.kind, v.rows[ix].card.verb.to_string(), v.rows[ix].card.target.to_string())
    });
    assert_eq!(close, (Kind::Work, "Close".into(), "«Which schema should the table go in?»".into()));
    // No card inside a run: a fold never hides a question.
    view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        for d in v.disp.iter() {
            if let Kind::Summary { end, .. } = d.kind {
                assert!(items[d.item..end].iter().all(|i| !crate::ask::asks(i)), "a run holds a card");
            }
        }
    });
    // Static rows (§6.1 Motion): with the open cards on screen, the
    // window at rest draws nothing and asks for nothing.
    show(cx, w, &view, cards[5].ix);
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| window.focus(&v.focus, cx))).expect("the window");
    cx.run_until_parked();
    for _ in 0..144 {
        idle_frame(cx, w, &view);
    }
    for i in 0..144 {
        assert_eq!(idle_frame(cx, w, &view), (0, 0), "at rest, frame {i}: drawn, queued");
    }
}

/// Square, as every box since #76: no card, receipt, option number, field
/// or answer row paints a rounded corner; only the open card's dot, a
/// circle, is round. Read off the painted quads with each card and answer
/// row on screen.
#[gpui_kit::test]
fn every_card_and_receipt_is_square(cx: &mut TestAppContext) {
    let (w, view, _events) = asks(cx, 900.);
    let rounded = |cx: &mut TestAppContext| -> Vec<String> {
        cx.update_window(w, |_, window, _| {
            window
                .painted_quads()
                .iter()
                .filter_map(|q| {
                    let c = q.corner_radii;
                    let r = [c.top_left.0, c.top_right.0, c.bottom_right.0, c.bottom_left.0];
                    let (wd, ht) = (q.bounds.size.width.0, q.bounds.size.height.0);
                    let circle = (wd - ht).abs() <= 0.5 && r.iter().all(|&r| r >= wd / 2. - 0.5);
                    (r.iter().any(|&r| r > 0.01) && !circle).then(|| format!("{:?} radii {r:?}", q.bounds))
                })
                .collect()
        })
        .expect("the window")
    };
    let all = rows(cx, &view);
    let shown: Vec<&Row> = all.iter().filter(|r| matches!(r.kind, Kind::Ask | Kind::Answer)).collect();
    assert_eq!(shown.len(), 12, "7 cards, 5 answers");
    for r in shown {
        show(cx, w, &view, r.ix);
        let what = match r.kind {
            Kind::Ask if ask(r).state == State::Open => format!("ask-card {}", r.id),
            Kind::Ask => format!("ask-receipt {}", r.id),
            _ => format!("ask-answer {}", r.id),
        };
        assert!(has(cx, w, what.clone()), "{what} drawn");
        let bad = rounded(cx);
        assert!(bad.is_empty(), "with {what} on screen, rounded corners:\n{}", bad.join("\n"));
    }
}

/// One 144 Hz frame of the clock, its timers firing, and a frame drawn
/// only if the window asked for one: the frames the app drew in it, and
/// the animation frames it had queued (`cards_d_tests`' own).
fn idle_frame(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> (u64, usize) {
    let dt = Duration::from_micros(6944);
    let f0 = view.read_with(cx, |v, _| v.fx.frames());
    let (t0, at) = (cx.executor().now(), view.read_with(cx, |v, _| v.fx.clock()));
    let _end = cx.executor().timer(dt);
    loop {
        cx.dispatcher.advance_clock_to_next_timer();
        let t = cx.executor().now() - t0;
        view.read_with(cx, |v, _| v.fx.set_clock(at + t));
        cx.run_until_parked();
        if t >= dt {
            break;
        }
    }
    let ran = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
    (view.read_with(cx, |v, _| v.fx.frames()) - f0, ran)
}

fn live(cx: &mut TestAppContext, view: &Entity<AppView>, events: Vec<Value>) {
    view.update(cx, |v, cx| {
        for e in events {
            let ev = decode_event(&e).unwrap_or_else(|err| panic!("{err}: {e}"));
            v.model.transcript.apply(&ev);
        }
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
}

fn llm(v: Value) -> Value {
    json!({"ev": "stream", "event": v})
}

/// The form the live tests ask: a field of each kind.
fn form() -> Value {
    json!({"question": "Who reviews the migration?", "fields": [
        {"id": "who", "label": "Reviewer", "placeholder": "a name"},
        {"id": "ci", "label": "Run CI first", "type": "check", "value": true},
        {"id": "why", "label": "Context for them", "type": "long", "optional": true}]})
}

fn showing(card: &str, input: &Value) -> String {
    let Ok(cards_spec::Input::Create { doc, wait }) = cards_spec::parse_input(input) else { panic!() };
    format!(
        "{}\nIts answer will come as a user message starting \"[card {card}]\".",
        cards_spec::Envelope::showing(cards_spec::CardId::parse(card).unwrap(), &doc, wait)
    )
}

/// A card asked live: `asking…` once its input is whole, whole and open
/// once its result binds (its fields drawn with their prefills); then
/// answered on another desk (F14), with no click and no key here: its turn
/// starts, the card reads answered at once and the answer is the turn's
/// slim row.
#[gpui_kit::test]
fn a_live_card_asks_opens_and_reads_an_answer_from_another_desk(cx: &mut TestAppContext) {
    let (w, view, _events) = asks(cx, 900.);
    view.update(cx, |v, _| v.model.transcript.set_busy(true));
    live(
        cx,
        &view,
        vec![
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": "toolu_live"})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "toolu_live", "input": form()})),
        ],
    );
    settle(cx, w, 8);
    let card = rows(cx, &view).into_iter().rfind(|r| r.kind == Kind::Ask).expect("the live card");
    assert_eq!(ask(&card).state, State::Pending);
    assert!(has(cx, w, format!("ask-state {} asking…", card.id)));
    assert!(!has(cx, w, format!("ask-footer {}", card.id)), "nothing to answer yet");
    live(
        cx,
        &view,
        vec![llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_live", "result": showing("c7live0", &form())}))],
    );
    settle(cx, w, 8);
    let id = card.id;
    assert!(has(cx, w, format!("ask-state {id} open")));
    for f in ["who", "ci", "why"] {
        assert!(has(cx, w, format!("ask-field {id} {f}")), "field {f}");
    }
    assert!(has(cx, w, format!("ask-footer {id}")));
    // Another desk answers: its turn starts here too.
    let (doc, _) = match cards_spec::parse_input(&form()).unwrap() {
        cards_spec::Input::Create { doc, wait } => (doc, wait),
        cards_spec::Input::Close(_) => unreachable!(),
    };
    let values = json!({"fields": {"who": "Ana", "ci": true, "why": ""}});
    let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&doc, &values).unwrap());
    let text = cards_spec::message::build(&cards_spec::CardId::parse("c7live0").unwrap(), &doc, &reply);
    live(
        cx,
        &view,
        vec![
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": text}),
        ],
    );
    settle(cx, w, 8);
    let after = rows(cx, &view);
    let card = after.iter().find(|r| r.id == id).expect("the card's row");
    assert_eq!(ask(card).state, State::Answered);
    assert!(has(cx, w, format!("ask-receipt {id}")));
    assert!(has(cx, w, format!("ask-said {id}")));
    let last = after.last().unwrap();
    assert_eq!(last.kind, Kind::Answer);
    assert!(has(cx, w, format!("ask-answer {}", last.id)));
}

/// §6.3: a call this desk can't read is an ordinary tool row with its
/// result, the user answering in text: a newer catalog's parameter, a
/// refusal, a collision suffix.
#[gpui_kit::test]
fn a_card_the_desk_cant_read_is_a_tool_row(cx: &mut TestAppContext) {
    let (w, view, _events) = asks(cx, 900.);
    view.update(cx, |v, _| v.model.transcript.set_busy(true));
    let newer = json!({"question": "Which layout?", "options": ["Grid", "List"], "layout": "cards"});
    let bound = showing("c7newer", &json!({"question": "Which layout?", "options": ["Grid", "List"]}));
    let mut events = vec![llm(json!({"kind": "llm", "llm": "response_start"}))];
    for (id, name, input, result) in [
        ("toolu_newer", "cards_ui", newer.clone(), bound.clone()),
        (
            "toolu_refused",
            "cards_ui",
            json!({"question": "Which layout?", "options": ["Grid", "List"]}),
            "cards_ui: 6 cards in the last 10 minutes; ask in plain text".to_string(),
        ),
        ("toolu_other", "cards_ui_2", json!({"question": "Which layout?", "options": ["Grid", "List"]}), bound),
    ] {
        events.push(llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input})));
        events.push(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": result})));
    }
    live(cx, &view, events);
    settle(cx, w, 8);
    let calls = view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        v.disp
            .iter()
            .filter(|d| {
                items[d.item]
                    .tool
                    .as_ref()
                    .is_some_and(|t| t.tool_id.starts_with("toolu_newer") || t.tool_id == "toolu_refused" || t.tool_id == "toolu_other")
            })
            .map(|d| (d.kind, d.id, items[d.item].card.is_some()))
            .collect::<Vec<_>>()
    });
    assert_eq!(calls.len(), 3);
    for (kind, id, card) in calls {
        assert_eq!((kind, card), (Kind::Work, false), "row {id}");
        assert!(has(cx, w, format!("tool-row {id}")), "row {id} is a tool row");
        assert!(!has(cx, w, format!("ask-card {id}")));
    }
    // It reads as what it asks, so it can be answered in text; a tool of
    // another name keeps its own row.
    let targets = view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        ["toolu_newer", "toolu_refused", "toolu_other"].map(|id| {
            let ix = items.iter().position(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == id)).expect("the call");
            let name = &items[ix].tool.as_ref().expect("a call").name;
            (crate::cards::verb(name).to_string(), v.rows[ix].card.target.to_string())
        })
    });
    assert_eq!(targets[0], ("Ask".into(), "«Which layout?»".into()));
    assert_eq!(targets[1], ("Ask".into(), "«Which layout?»".into()));
    assert_ne!(targets[2].1, "«Which layout?»", "cards_ui_2 is another tool");
}

/// A card asked between two calls of one response is a row of its own:
/// the run before it ends, another starts after it, and neither holds it.
#[test]
fn a_card_between_two_calls_splits_their_run() {
    let showing = showing("c7split", &form());
    let msgs = vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "r1", "name": "read", "input": {"path": "a"}},
            {"type": "tool_use", "id": "r2", "name": "read", "input": {"path": "b"}},
            {"type": "tool_use", "id": "q1", "name": "cards_ui", "input": form()},
            {"type": "tool_use", "id": "r3", "name": "read", "input": {"path": "c"}}]}),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "r1", "content": "1"},
            {"type": "tool_result", "tool_use_id": "r2", "content": "2"},
            {"type": "tool_result", "tool_use_id": "q1", "content": showing},
            {"type": "tool_result", "tool_use_id": "r3", "content": "3"}]}),
    ];
    let mut t = synaps_client::Transcript::new();
    t.apply_history(&msgs);
    let rows = crate::turns::build(t.items(), &|_| None, 0, false, &Default::default(), &Default::default());
    let kinds: Vec<Kind> = rows.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, [Kind::User, Kind::Work, Kind::Work, Kind::Ask, Kind::Work]);
    assert_eq!(rows[3].gap, crate::turns::Gap::Part, "the card is a part of the turn");
    assert_eq!(rows[4].gap, crate::turns::Gap::Part, "the run after it starts anew");
}

/// What a card draws is sanitized for display (synaps-cards §5.2): an
/// answer's values are validated, never rewritten, so a bidi override, a
/// tab, a line break or an invisible in a note or a field stays in the
/// reply and never reaches the receipt or the answer's row. And a call
/// this desk can't read says its question on one line, sanitized and cut
/// to the spec's bound, or, nothing of it left, its input, sanitized.
#[gpui_kit::test]
fn what_a_card_draws_is_sanitized(cx: &mut TestAppContext) {
    let (w, view, _events) = asks(cx, 900.);
    view.update(cx, |v, _| v.model.transcript.set_busy(true));
    let input = json!({"question": "Who signs off on the cut-over?",
        "options": [{"label": "Ana", "note": "Why her?"}, "Ben"],
        "fields": [{"id": "name", "label": "Their handle"}, {"id": "why", "label": "Context", "type": "long", "optional": true}]});
    let mut events = vec![
        llm(json!({"kind": "llm", "llm": "response_start"})),
        llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "toolu_signoff", "input": input})),
        llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_signoff", "result": showing("c7bidi0", &input)})),
    ];
    for (id, question) in [
        ("toolu_bidi", "safe\u{202e}evil\u{202c}".to_string()),
        ("toolu_blank", "\u{202e}\u{200b}".to_string()),
        ("toolu_long", "x".repeat(300)),
    ] {
        events.push(llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": id,
            "input": {"question": question, "options": ["A", "B"]}})));
        events.push(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": "cards_ui: refused"})));
    }
    live(cx, &view, events);
    settle(cx, w, 8);
    // Another desk answers, every value as it was typed.
    let doc = match cards_spec::parse_input(&input).unwrap() {
        cards_spec::Input::Create { doc, .. } => doc,
        cards_spec::Input::Close(_) => unreachable!(),
    };
    let values = json!({"choice": "Ana", "note": "her\u{202e}eh\u{202c}\u{200b}",
        "fields": {"name": "left\u{202e}right\u{202c}", "why": "one\ttwo\nthree\u{ad}\u{feff}"}});
    let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&doc, &values).expect("valid values"));
    let text = cards_spec::message::build(&cards_spec::CardId::parse("c7bidi0").unwrap(), &doc, &reply);
    live(
        cx,
        &view,
        vec![
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": text}),
        ],
    );
    settle(cx, w, 8);
    let hostile = |s: &str| s.chars().any(|c| c.is_control() || cards_spec::sanitize::is_invisible(c));
    let all = rows(cx, &view);
    let card = all.iter().rfind(|r| r.kind == Kind::Ask).expect("the card");
    let a = ask(card);
    assert_eq!(a.state, State::Answered);
    // The reply is what was sent.
    let Some(synaps_client::cards::Reply::Answer(sent)) = a.reply.as_ref() else { panic!("its answer: {:?}", a.reply) };
    assert!(sent.note.as_deref().is_some_and(hostile), "the note as it was sent");
    // What the receipt and the answer's row draw of it: one clean line.
    let said = crate::ask::summary(&a.doc, a.reply.as_ref().unwrap()).expect("what was answered");
    assert_eq!(said, "Ana · “hereh” · Their handle: leftright · Context: one two three");
    show(cx, w, &view, card.ix);
    assert!(has(cx, w, format!("ask-said {}", card.id)));
    let answer = all.last().expect("the answer's row");
    let Some(CardRow::Answer(an)) = &answer.card else { panic!("the answer's row: {:?}", answer.card) };
    assert_eq!(crate::ask::summary(&an.doc, &an.reply).as_deref(), Some(said.as_str()));
    show(cx, w, &view, answer.ix);
    assert!(has(cx, w, format!("ask-answer {}", answer.id)));
    // The calls this desk can't read: tool rows, their targets clean.
    let targets = view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        ["toolu_bidi", "toolu_blank", "toolu_long"].map(|id| {
            let ix = items.iter().position(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == id)).expect("the call");
            (items[ix].card.is_none(), v.rows[ix].card.target.to_string())
        })
    });
    assert!(targets.iter().all(|(plain, t)| *plain && !hostile(t.as_str())), "{targets:?}");
    assert_eq!(targets[0].1, "«safeevil»");
    assert!(!targets[1].1.is_empty() && !targets[1].1.starts_with('«'), "nothing of its question left: its input, {:?}", targets[1].1);
    assert_eq!(targets[2].1.chars().count(), cards_spec::QUESTION_MAX + 2, "cut to the spec's bound: {:?}", targets[2].1);
    assert!(targets[2].1.ends_with("…»"));
}

/// An open form draws whole: its options (the agent's default
/// `suggested`), every field with its prefill, a long one over its lines,
/// the check, an empty optional field with its placeholder; `NEEDS YOU`,
/// the agent waiting; a footer that says how to answer.
#[gpui_kit::test]
fn an_open_form_draws_its_fields_and_prefills(cx: &mut TestAppContext) {
    let (w, view, _events) = session(cx, 900., crate::demo::ASK_FORM);
    let card = rows(cx, &view).into_iter().find(|r| r.kind == Kind::Ask).expect("the form");
    let a = ask(&card);
    assert_eq!((a.state, a.wait, a.doc.options.len(), a.doc.fields.len()), (State::Open, true, 2, 5));
    show(cx, w, &view, card.ix);
    let id = card.id;
    assert!(has(cx, w, format!("ask-card {id}")));
    assert!(has(cx, w, format!("ask-needs {id}")));
    for f in ["batch", "window", "verify", "notes", "ticket"] {
        assert!(has(cx, w, format!("ask-field {id} {f}")), "field {f}");
    }
    for n in 1..=2 {
        assert!(has(cx, w, format!("ask-option {id} {n}")), "option {n}");
    }
    assert!(has(cx, w, format!("ask-suggested {id} 1")));
    assert!(!has(cx, w, format!("ask-suggested {id} 2")));
    assert!(has(cx, w, format!("ask-footer {id}")));
}
