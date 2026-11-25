//! Answering agent cards by mouse, in the window (synaps-cards
//! `docs/design.md` §6.1, §7; §9.3's D2a row): the mouse halves of
//! F1–F7, F16, F17 and F19 as the card draws them; a click before the
//! card armed (§7's negative test); a card answer never resolves a
//! permission prompt (§5.1); a receipt opens to the whole answer (Shady
//! #80). What would have gone to the daemon is the view's record
//! (`AppView::card_sends`): exactly one `Submit` or `Steer`, its text and
//! its attachment. The daemon's echoes are applied as it sends them.
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, ListOffset, Pixels, Point, TestAppContext, VisualTestContext, px};
use serde_json::{Value, json};
use synaps_client::cards::{CardRow, State};
use synaps_client::protocol::decode_event;

use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::deliver::Outgoing;
use crate::pane::PaneId;
use crate::turns::Kind;

type Window = (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>);

/// The demo's window, showing its session `id`, with a client (whose
/// daemon is nowhere: what is sent is the view's record, and goes no
/// further).
fn open(cx: &mut TestAppContext, id: &str) -> Window {
    let (w, view, events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    let client = view.update(cx, |v, cx| {
        // The demo answers the attach; the client sends.
        let c = v.client.take();
        let open = crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false };
        v.open_tab(open, crate::tabs::Place::Current);
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.update(cx, |v, _| v.client = client);
    (w, view, events)
}

fn key(cx: &mut TestAppContext, window: AnyWindowHandle, k: &str) {
    let keystroke = gpui_kit::Keystroke::parse_with_os(k, cx.update(|cx| cx.platform_info().os)).expect("a keystroke");
    let down = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.update_window(window, |_, w, cx| {
        w.dispatch_event(down, cx);
    })
    .expect("the window");
    cx.run_until_parked();
    cx.update_window(window, |_, w, cx| {
        w.dispatch_event(gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }), cx);
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, window, 8);
}

fn has(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel)).is_some()
}

fn center(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Point<Pixels> {
    let b = VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel.clone())).unwrap_or_else(|| panic!("{sel} drawn"));
    b.center()
}

/// A click on what `sel` names, at its centre: in some pane's list, as
/// the hand would have it (a row past the list's edge is under the
/// composer, never clicked).
fn click(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, sel: String) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    let position = center(cx, w, sel.clone());
    let lists: Vec<_> = view.read_with(cx, |v, _| v.panes.iter().map(|p| p.list.viewport_bounds()).collect());
    assert!(lists.iter().any(|b| b.contains(&position)), "{sel} at {position:?}: in no list's view {lists:?}");
    let mut vcx = VisualTestContext::from_window(w, cx);
    vcx.simulate_event(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() });
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1 });
    vcx.run_until_parked();
    settle(cx, w, 8);
}

/// The card asking `question`: its row id and display index.
fn card(cx: &mut TestAppContext, view: &Entity<AppView>, question: &str) -> (u64, usize) {
    view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        v.disp
            .iter()
            .enumerate()
            .find(|(_, d)| {
                d.kind == Kind::Ask
                    && matches!(items.get(d.item).and_then(|i| i.card.as_deref()), Some(CardRow::Ask(a)) if a.doc.question == question)
            })
            .map(|(ix, d)| (d.id, ix))
            .unwrap_or_else(|| panic!("a card asking {question:?}"))
    })
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

/// The card's arm (`crate::app::ARM` from the first frame each of its
/// controls is painted in view): run out for every control drawn so far,
/// or a fifth of the way for the first drawn (less for the rest).
fn arm(cx: &mut TestAppContext, view: &Entity<AppView>, row: u64, armed: bool) {
    let shown: Vec<Instant> =
        view.read_with(cx, |v, _| v.panes.cur.ask_shown.borrow().iter().filter(|((r, _), _)| *r == row).map(|(_, at)| *at).collect());
    let at = if armed { shown.iter().max().map(|t| *t + crate::app::ARM) } else { shown.iter().min().map(|t| *t + crate::app::ARM / 5) };
    crate::app::pin_arm_clock(Some(at.expect("the card's controls were drawn")));
}

fn sends(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<Outgoing> {
    view.read_with(cx, |v, _| v.card_sends.clone())
}

/// The attachment on screen: its seq and session.
fn attachment(cx: &mut TestAppContext, view: &Entity<AppView>) -> (u64, String) {
    view.read_with(cx, |v, _| {
        let l = &v.model.lives[v.panes.cur.live];
        (l.attachment().expect("attached"), l.active.as_ref().unwrap().session_id.clone())
    })
}

/// Session events as the daemon sends them, through the model as the
/// client hands them over.
fn heard(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, events: Vec<Value>) {
    let (seq, sid) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let batch = events
            .into_iter()
            .map(|e| {
                let ev = decode_event(&e).unwrap_or_else(|err| panic!("{err}: {e}"));
                synaps_client::Event::session(seq, sid.clone(), 0, ev, Arc::new(e))
            })
            .collect();
        v.apply_batch(batch, cx);
    });
    cx.run_until_parked();
    settle(cx, w, 8);
}

fn doc(input: &Value) -> (cards_spec::CardDoc, bool) {
    match cards_spec::parse_input(input).unwrap() {
        cards_spec::Input::Create { doc, wait } => (doc, wait),
        cards_spec::Input::Close(_) => unreachable!(),
    }
}

/// The message answering card `card` (asked `input`) with `values`.
fn message(card: &str, input: &Value, values: Value) -> String {
    let (d, _) = doc(input);
    let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&d, &values).unwrap());
    cards_spec::message::build(&cards_spec::CardId::parse(card).unwrap(), &d, &reply)
}

fn showing(card: &str, input: &Value) -> String {
    let (d, wait) = doc(input);
    format!(
        "{}\nIts answer will come as a user message starting \"[card {card}]\".",
        cards_spec::Envelope::showing(cards_spec::CardId::parse(card).unwrap(), &d, wait)
    )
}

fn llm(v: Value) -> Value {
    json!({"ev": "stream", "event": v})
}

/// The agent asks `input` live as card `card` (call `tool`), and waits:
/// its turn ends, and its history lands (the round's checkpoint): the
/// card's binding is the history's (Shady #81 P2-1).
fn ask_live(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, card: &str, input: Value) {
    heard(
        cx,
        w,
        view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"}),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": tool})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": tool, "input": input})),
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": showing(card, &input)})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    land(cx, w, view, asked_messages(tool, card, &input, None));
    play(cx, w, view);
}

/// The history of a turn that asked `input` as card `card` (call `tool`),
/// then said `after`.
fn asked_messages(tool: &str, card: &str, input: &Value, after: Option<&str>) -> Vec<Value> {
    let mut out = vec![
        json!({"role": "user", "content": "go"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": tool, "name": "cards_ui", "input": input}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": showing(card, input)}]}),
    ];
    if let Some(a) = after {
        out.push(json!({"role": "assistant", "content": [{"type": "text", "text": a}]}));
    }
    out
}

/// The history after what the view holds lands: `messages`, appended (as
/// the client hands a round's checkpoint over).
fn land(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<Value>) {
    let (seq, sid) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let base_len = v.model.lives[v.panes.cur.live].transcript.history_len();
        let update = synaps_client::HistoryUpdate::Append { base_len, messages };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx);
    });
    cx.run_until_parked();
    settle(cx, w, 8);
}

/// What just came plays out: its rows arrive and grow to their height.
/// The motion clock is the test's, and a growing row steps at most 50 ms
/// a frame (`PaneFx::grow_on`): two seconds of frames, 50 ms apart.
fn play(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    for _ in 0..40 {
        wait(cx, w, view, 50);
    }
}

const SHIP: &str = "Run the migration on staging now?";

/// F1, F2 and §7's negative test: a click on an option before the card
/// armed sends nothing; once armed, one click is the answer: one
/// `Submit` (the session idle) of the spec's message, on the card's
/// attachment. The card is its receipt at once, `sending…`; the echo
/// starts the turn and it reads `answered`. Its options are gone: a
/// second click has nothing to land on.
#[gpui_kit::test]
fn one_click_on_an_armed_option_answers_a_quick_card(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    assert!(has(cx, w, format!("ask-option {id} 2")));
    arm(cx, &view, id, false);
    click(cx, w, &view, format!("ask-option {id} 2"));
    assert!(sends(cx, &view).is_empty(), "a click before the card armed sends nothing");
    assert!(has(cx, w, format!("ask-card {id}")), "still open");
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-option {id} 2"));
    let (seq, _) = attachment(cx, &view);
    let input = json!({"question": SHIP, "wait": true,
        "detail": "It locks the tokens table for about 40 seconds.\nRefreshes queue meanwhile; none are lost.",
        "options": [{"label": "Run it now"}, {"label": "Wait for the low-traffic window", "hint": "after 22:00"}, "Don't run it"],
        "default": "Wait for the low-traffic window"});
    let text = message("c0p6r4n", &input, json!({"choice": "Wait for the low-traffic window"}));
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing { seq, busy: false, card: cards_spec::CardId::parse("c0p6r4n").unwrap(), text: text.clone() }],
        "one Submit, on the card's attachment"
    );
    assert!(has(cx, w, format!("ask-receipt {id}")), "its receipt at once");
    assert!(has(cx, w, format!("ask-state {id} sending…")));
    assert!(!has(cx, w, format!("ask-option {id} 1")), "nothing left to click");
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": text})]);
    assert!(has(cx, w, format!("ask-state {id} answered")), "the echo: answered");
    assert!(has(cx, w, format!("ask-said {id}")));
    assert_eq!(sends(cx, &view).len(), 1);
    crate::app::pin_arm_clock(None);
}

/// `--offline`: the synthetic daemon takes a card's answer as a daemon
/// would: `sending…`, then its echo (answered), then the agent's line.
#[gpui_kit::test]
fn offline_the_synthetic_daemon_answers_a_card(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    view.update(cx, |v, _| v.client = None);
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(has(cx, w, format!("ask-state {id} sending…")));
    wait(cx, w, &view, crate::demo::ECHO_AFTER.as_millis() as u64 + 50);
    assert!(has(cx, w, format!("ask-state {id} answered")), "echoed");
    wait(cx, w, &view, crate::demo::REPLY_AFTER.as_millis() as u64 + 50);
    view.read_with(cx, |v, _| assert!(!v.model.transcript.busy(), "its turn ran and ended"));
    crate::app::pin_arm_clock(None);
}

/// F7: Skip, one click: `[card <id> skipped]`.
#[gpui_kit::test]
fn skip_is_one_click(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-skip {id}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].text, format!("[card c0p6r4n skipped] {SHIP} → (skipped)\n{{}}"));
    crate::app::pin_arm_clock(None);
}

/// F4: a form prefilled and valid is one click, Send: every field as
/// prefilled, its option on the agent's default.
#[gpui_kit::test]
fn a_prefilled_form_is_one_click_send(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASK_FORM);
    let (id, ix) = card(cx, &view, "How should the backfill run?");
    show(cx, w, &view, ix);
    assert!(has(cx, w, format!("ask-picked {id} 1")), "the default, picked");
    // The optional field says so; the others don't (Shady #81 r2, polish).
    assert!(has(cx, w, format!("ask-optional {id} ticket")), "Ticket: optional");
    for f in ["batch", "window", "notes"] {
        assert!(!has(cx, w, format!("ask-optional {id} {f}")), "{f}: required");
    }
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-send {id}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1, "one Submit");
    let values = json!({"choice": "In batches", "fields": {"batch": "10000", "window": "22:00 to 23:30, rolled back by 23:45",
        "verify": true, "notes": "Page #relay-oncall if a batch takes over a minute.\nThe dashboards are under Relay / Tokens.", "ticket": ""}});
    let input = view.read_with(cx, |v, _| {
        let c = v.model.transcript.cards().cards().last().cloned().unwrap();
        (c.id.unwrap(), c.doc.unwrap())
    });
    let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&input.1, &values).unwrap());
    assert_eq!(s[0].text, cards_spec::message::build(&input.0, &input.1, &reply));
    crate::app::pin_arm_clock(None);
}

/// F5 and F6: a form's clicks pick and unpick (several, `multi`), tick a
/// check, open a field to type in; Send sends what they made, the picks
/// in the doc's order.
#[gpui_kit::test]
fn a_forms_clicks_pick_tick_and_type_then_send(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, "Which suites should pass before the cut-over?");
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    assert!(has(cx, w, format!("ask-picked {id} 1")) && has(cx, w, format!("ask-picked {id} 2")), "the default");
    click(cx, w, &view, format!("ask-option {id} 4"));
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(has(cx, w, format!("ask-picked {id} 4")) && !has(cx, w, format!("ask-picked {id} 1")), "Chaos picked, Unit unpicked");
    assert!(sends(cx, &view).is_empty(), "a form's pick sends nothing");
    click(cx, w, &view, format!("ask-send {id}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1);
    assert!(s[0].text.ends_with("\n{\"choice\":[\"Integration\",\"Chaos\"]}"), "{}", s[0].text);
    // A live form: a check ticked, a field typed into.
    let form = json!({"question": "Who reviews the migration?", "wait": true, "fields": [
        {"id": "who", "label": "Reviewer", "placeholder": "a name"},
        {"id": "ci", "label": "Run CI first", "type": "check", "value": true}]});
    ask_live(cx, w, &view, "toolu_f5", "c5f5aaa", form.clone());
    let (id, ix) = card(cx, &view, "Who reviews the migration?");
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    assert!(has(cx, w, format!("ask-send {id} off")), "Reviewer is needed: Send waits");
    click(cx, w, &view, format!("ask-field {id} ci"));
    click(cx, w, &view, format!("ask-field {id} who"));
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            let sid = v.card_session().unwrap();
            let e = v.card_drafts[&(sid, "toolu_f5".to_string())].lines["who"].clone();
            e.update(cx, |s, cx| s.set_value("Ana", window, cx));
        })
    })
    .unwrap();
    settle(cx, w, 8);
    click(cx, w, &view, format!("ask-send {id}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].text, message("c5f5aaa", &form, json!({"fields": {"who": "Ana", "ci": false}})));
    crate::app::pin_arm_clock(None);
}

/// F3: an option that asks for a note opens its note under it; Send
/// waits for the note, then sends both.
#[gpui_kit::test]
fn an_option_that_asks_for_a_note_opens_it(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let input = json!({"question": "Ship it?", "wait": true, "options": ["Yes", {"label": "Not yet", "note": "What's missing?"}]});
    ask_live(cx, w, &view, "toolu_f3", "c3n0te1", input.clone());
    let (id, ix) = card(cx, &view, "Ship it?");
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-option {id} 2"));
    assert!(sends(cx, &view).is_empty(), "a note to write first");
    assert!(has(cx, w, format!("ask-note {id}")));
    assert!(has(cx, w, format!("ask-send {id} off")));
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            let sid = v.card_session().unwrap();
            let e = v.card_drafts[&(sid, "toolu_f3".to_string())].note.clone().unwrap();
            e.update(cx, |s, cx| s.set_value("the changelog", window, cx));
        })
    })
    .unwrap();
    settle(cx, w, 8);
    // Send came with the note: it arms from its own first frame, not the
    // options' (Shady #81 P2-3).
    click(cx, w, &view, format!("ask-send {id}"));
    assert!(sends(cx, &view).is_empty(), "Send, new, not armed yet");
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-send {id}"));
    assert_eq!(
        sends(cx, &view).iter().map(|o| o.text.clone()).collect::<Vec<_>>(),
        vec![message("c3n0te1", &input, json!({"choice": "Not yet", "note": "the changelog"}))]
    );
    crate::app::pin_arm_clock(None);
}

/// F9: the card in the other pane, one click: sent on that pane's
/// attachment, not the focused pane's (another session's).
#[gpui_kit::test]
fn a_card_in_the_other_pane_is_one_click(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    let (seq, _) = attachment(cx, &view);
    let first: PaneId = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, _| v.app_settings.split = true);
    key(cx, w, "ctrl-\\");
    play(cx, w, &view);
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::SID.into(), fallback: None, open: false },
            crate::tabs::Place::Current,
        );
        cx.notify();
        c
    });
    play(cx, w, &view);
    view.update(cx, |v, _| v.client = client);
    let other = view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 2);
        assert_ne!(v.panes.focused, first, "the other pane is focused");
        v.model.lives[v.panes.cur.live].attachment()
    });
    assert!(other.is_some_and(|o| o != seq), "the focused pane holds another attachment");
    // The card's pane: its row in view, past its arm.
    view.update(cx, |v, cx| {
        let _ = v.in_pane(first, |v| v.panes.cur.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) }));
        cx.notify();
    });
    settle(cx, w, 8);
    crate::app::pin_arm_clock(Some(Instant::now() + Duration::from_secs(3600)));
    click(cx, w, &view, format!("ask-option {id} 1"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1, "one click, one Submit");
    assert_eq!(s[0].seq, seq, "on the card's pane's attachment");
    crate::app::pin_arm_clock(None);
}

/// §5.1: with a permission prompt pending, a card whose options read
/// `yes` and `always` answered `yes`: one `Submit` of the card's message,
/// and the prompt is still pending, unanswered (no `Answer` went).
#[gpui_kit::test]
fn a_card_click_never_resolves_a_prompt(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let input = json!({"question": "Allow it?", "options": ["yes", "always"]});
    ask_live(cx, w, &view, "toolu_pr", "c7pr0mp", input.clone());
    heard(
        cx,
        w,
        &view,
        vec![json!({"ev": "prompt", "request": {"id": 7, "kind": "confirm", "title": "Confirm tool call",
            "prompt": "Extension: guard\nTool: edit\n\nAllow this tool call?"}})],
    );
    play(cx, w, &view);
    let (id, ix) = card(cx, &view, "Allow it?");
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert_eq!(
        sends(cx, &view).iter().map(|o| o.text.clone()).collect::<Vec<_>>(),
        vec![message("c7pr0mp", &input, json!({"choice": "yes"}))]
    );
    view.read_with(cx, |v, _| {
        let l = &v.model.lives[v.panes.cur.live];
        assert!(l.transcript.pending_prompts().iter().any(|p| p.id == 7), "the prompt is still pending");
        assert!(!l.prompt_answered(7), "and unanswered");
        assert!(v.sent.is_none(), "no answer to it went");
    });
    crate::app::pin_arm_clock(None);
}

/// F16: not sent (here: no connection), the card says why and offers
/// Retry; Retry sends the same text.
#[gpui_kit::test]
fn not_sent_says_why_and_retry_sends_it(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    // No connection, and no synthetic daemon (`--offline`'s) to take it.
    let client = view.update(cx, |v, _| {
        v.synthetic = false;
        v.client.take()
    });
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(has(cx, w, format!("ask-state {id} not sent")));
    assert!(has(cx, w, format!("ask-retry {id}")));
    view.update(cx, |v, _| v.client = client);
    // Retry came with the receipt: it arms from its own first frame.
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-retry {id}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].text, s[1].text, "the same answer");
    assert!(has(cx, w, format!("ask-state {id} sending…")));
    crate::app::pin_arm_clock(None);
}

/// F17: no echo within 10 s: checked, and unconfirmed on the view held,
/// its cause and Resend under its receipt; Resend sends it again.
#[gpui_kit::test]
fn unconfirmed_offers_resend(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-option {id} 3"));
    view.update(cx, |v, cx| {
        v.model.tick_cards(Instant::now() + Duration::from_secs(11));
        v.cards_moved(cx);
    });
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-state {id} not confirmed")));
    assert!(has(cx, w, format!("ask-cause {id}")));
    // Resend came with the receipt: it arms from its own first frame.
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-resend {id}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].text, s[1].text);
    crate::app::pin_arm_clock(None);
}

/// F19: an exclusive session another client owns: the card says whose it
/// is and how to take it; a click sends nothing. A known compaction:
/// the card says to wait.
#[gpui_kit::test]
fn a_card_this_window_cannot_answer_says_why(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    view.update(cx, |v, cx| {
        let a = v.model.lives[v.panes.cur.live].active.as_mut().unwrap();
        a.input = synaps_client::ClientInput::Exclusive;
        a.owner = Some(a.client + 1);
        cx.notify();
    });
    settle(cx, w, 8);
    let footer = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].input_gate().is_err());
    assert!(footer(cx));
    assert!(has(cx, w, format!("ask-skip {id} off")));
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(sends(cx, &view).is_empty(), "read-only: nothing sent");
    view.update(cx, |v, cx| {
        let a = v.model.lives[v.panes.cur.live].active.as_mut().unwrap();
        a.input = synaps_client::ClientInput::Shared;
        cx.notify();
    });
    heard(cx, w, &view, vec![json!({"ev": "compaction_started", "source": "manual", "disclosure": ""})]);
    assert!(has(cx, w, format!("ask-skip {id} off")), "compacting: wait");
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(sends(cx, &view).is_empty());
    heard(cx, w, &view, vec![json!({"ev": "compaction_cancelled"})]);
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert_eq!(sends(cx, &view).len(), 1, "done: it answers");
    crate::app::pin_arm_clock(None);
}

/// Shady #80: a long receipt is reachable. A click opens it to the whole
/// answer, a line a part; another closes it.
#[gpui_kit::test]
fn a_receipt_opens_to_the_whole_answer(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let first = view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        v.disp
            .iter()
            .enumerate()
            .find(|(_, d)| {
                d.kind == Kind::Ask && matches!(items[d.item].card.as_deref(), Some(CardRow::Ask(a)) if a.state == State::Answered)
            })
            .map(|(ix, d)| (d.id, ix))
            .unwrap()
    });
    show(cx, w, &view, first.1);
    let id = first.0;
    assert!(!has(cx, w, format!("ask-whole {id}")));
    // Its chevron says a click opens it (Shady #81 r2, polish).
    assert!(has(cx, w, format!("ask-opens {id} shut")), "a closed chevron");
    click(cx, w, &view, format!("ask-receipt {id}"));
    assert!(has(cx, w, format!("ask-whole {id}")), "opened");
    assert!(has(cx, w, format!("ask-opens {id} open")), "an open chevron");
    click(cx, w, &view, format!("ask-receipt {id}"));
    assert!(!has(cx, w, format!("ask-whole {id}")), "closed");
    assert!(has(cx, w, format!("ask-opens {id} shut")), "closed again");
}

/// Shady #81 P2-3: a card's controls arm from the first frame they are
/// drawn in view, actionable; not from when its row was first built. Asked
/// live, it reads `asking…` (nothing to click) longer than the arm; its
/// result binds it and the history lands: a click on the first frame its
/// options are drawn sends nothing. After the arm, one click sends once.
#[gpui_kit::test]
fn a_bound_cards_options_arm_from_their_first_frame(cx: &mut TestAppContext) {
    let t0 = Instant::now();
    crate::app::pin_arm_clock(Some(t0));
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let input = json!({"question": "Squash the commits?", "options": ["Squash", "Keep them"]});
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"}),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": "toolu_sq"})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "toolu_sq", "input": input})),
        ],
    );
    play(cx, w, &view);
    let (id, _) = card(cx, &view, "Squash the commits?");
    assert!(has(cx, w, format!("ask-state {id} asking…")), "drawn, unbound");
    crate::app::pin_arm_clock(Some(t0 + crate::app::ARM * 3));
    heard(
        cx,
        w,
        &view,
        vec![
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_sq", "result": showing("c5qu4sh", &input)})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    land(cx, w, &view, asked_messages("toolu_sq", "c5qu4sh", &input, None));
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-option {id} 1")), "bound: its options drawn");
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(sends(cx, &view).is_empty(), "a click as its options first show sends nothing");
    crate::app::pin_arm_clock(Some(t0 + crate::app::ARM * 4 + Duration::from_millis(1)));
    click(cx, w, &view, format!("ask-option {id} 1"));
    let s = sends(cx, &view);
    assert_eq!(s.iter().map(|o| o.text.clone()).collect::<Vec<_>>(), vec![message("c5qu4sh", &input, json!({"choice": "Squash"}))]);
    crate::app::pin_arm_clock(None);
}

/// Shady #81 P2-3, off screen: the list builds a row above the view to
/// measure it (GPUI's overdraw), and paints one whose controls its edge
/// clips; neither shows them. Revealed past the arm since, a click on
/// their first frame in view sends nothing; after the arm, one click
/// sends once.
#[gpui_kit::test]
fn a_cards_options_measured_or_clipped_off_screen_arm_once_in_view(cx: &mut TestAppContext) {
    let t0 = Instant::now();
    crate::app::pin_arm_clock(Some(t0));
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    // Scrolled back: the card comes in far under the view.
    show(cx, w, &view, 0);
    let input = json!({"question": "Squash the commits?", "options": ["Squash", "Keep them"]});
    let after = (1..=60).map(|n| format!("Step {n} of the plan, in a line of its own.")).collect::<Vec<_>>().join("\n\n");
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"}),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": "toolu_sq"})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": "toolu_sq", "input": input})),
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_sq", "result": showing("c5qu4sh", &input)})),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "text", "text": after})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    land(cx, w, &view, asked_messages("toolu_sq", "c5qu4sh", &input, Some(&after)));
    play(cx, w, &view);
    let (id, ix) = card(cx, &view, "Squash the commits?");
    // The row under it at the top: the card is built above the view.
    show(cx, w, &view, ix + 1);
    assert!(!has(cx, w, format!("ask-card {id}")), "not drawn");
    // Its last few pixels in view: drawn, its options clipped.
    view.update(cx, |v, cx| {
        v.list.scroll_by(px(-6.));
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.list.bounds_for_item(ix).is_some()), "its foot drawn");
    crate::app::pin_arm_clock(Some(t0 + crate::app::ARM * 3));
    show(cx, w, &view, ix);
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert!(sends(cx, &view).is_empty(), "a click as its options first show sends nothing");
    crate::app::pin_arm_clock(Some(t0 + crate::app::ARM * 4 + Duration::from_millis(1)));
    click(cx, w, &view, format!("ask-option {id} 1"));
    assert_eq!(sends(cx, &view).len(), 1, "after the arm, one click sends once");
    crate::app::pin_arm_clock(None);
}

/// After dropped events: the history asked for again (the demo's), and
/// then, unless `history_only`, a digest on the stream that says the slot
/// (empty).
fn resynced(cx: &mut TestAppContext, view: &Entity<AppView>, history_only: bool) {
    let (seq, sid) = attachment(cx, view);
    let messages = match crate::demo::attach(crate::demo::ASKS, seq).pop() {
        Some(synaps_client::Event::Attached { attached, .. }) => attached.conversation.api_messages.clone(),
        other => panic!("{other:?}"),
    };
    let d = json!({"ev": "conversation", "digest": {"messages_len": messages.len(),
        "messages_hash": synaps_client::history::messages_hash(&messages), "queued_message": null}});
    let update = synaps_client::HistoryUpdate::Replace { messages };
    view.update(cx, |v, cx| {
        let mut batch = vec![synaps_client::Event::History { attach_seq: seq, session_id: sid.clone(), seq: 0, update }];
        if !history_only {
            batch.push(synaps_client::Event::session(seq, sid, 0, decode_event(&d).unwrap(), Arc::new(d)));
        }
        v.apply_batch(batch, cx)
    });
}

/// Shady #81 round 3: the daemon dropped events for this desk (its lag
/// notice). Until the history asked for again lands, and an event after
/// the notice says the engine's slot (round 6: here a digest), a card's
/// controls wait, saying why, and a click sends nothing; then one click
/// answers. Unconfirmed, and another gap: its Resend waits too, its line
/// saying why.
#[gpui_kit::test]
fn dropped_events_hold_a_cards_clicks_until_the_history_asked_for_again(cx: &mut TestAppContext) {
    let lag = || vec![json!({"ev": "system_notice", "text": "event stream lagged; 6 dropped"})];
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    heard(cx, w, &view, lag());
    assert!(has(cx, w, format!("ask-skip {id} off")), "its controls wait");
    click(cx, w, &view, format!("ask-option {id} 3"));
    assert!(sends(cx, &view).is_empty(), "nothing sent");
    resynced(cx, &view, false);
    settle(cx, w, 8);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    assert!(has(cx, w, format!("ask-skip {id}")), "the history asked for again: they don't");
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-option {id} 3"));
    assert_eq!(sends(cx, &view).len(), 1, "one click answers");
    view.update(cx, |v, cx| {
        v.model.tick_cards(Instant::now() + Duration::from_secs(11));
        v.cards_moved(cx);
    });
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-state {id} not confirmed")));
    assert!(has(cx, w, format!("ask-resend {id}")), "Resend");
    heard(cx, w, &view, lag());
    assert!(has(cx, w, format!("ask-resend {id} off")) && !has(cx, w, format!("ask-resend {id}")), "Resend waits");
    assert!(has(cx, w, format!("ask-cause {id}")), "and says why");
    resynced(cx, &view, false);
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-resend {id}")), "then it doesn't");
    crate::app::pin_arm_clock(None);
}

/// Shady #81 rounds 4 and 6: an answer out through dropped events, no
/// event since saying the engine's slot. Past the wait its row says it
/// couldn't be confirmed, with "Check again" where Resend would be; a
/// click asks for the history again on the same attachment (no
/// reattach), and it checks for another wait, Resend waiting. That
/// history lands: still no Resend, the slot unknown. A digest after it
/// says the slot (empty): not confirmed, with Resend.
#[gpui_kit::test]
fn past_the_wait_a_card_offers_check_again_not_resend(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (id, ix) = card(cx, &view, SHIP);
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-option {id} 3"));
    assert_eq!(sends(cx, &view).len(), 1, "one click answers");
    heard(cx, w, &view, vec![json!({"ev": "system_notice", "text": "event stream lagged; 6 dropped"})]);
    let t0 = Instant::now();
    view.update(cx, |v, cx| {
        v.model.tick_cards(t0 + Duration::from_secs(11));
        v.model.tick_cards(t0 + Duration::from_secs(22));
        v.cards_moved(cx);
    });
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-state {id} not confirmed")), "it couldn't be checked");
    assert!(has(cx, w, format!("ask-check {id}")), "Check again");
    assert!(!has(cx, w, format!("ask-resend {id}")) && !has(cx, w, format!("ask-resend {id} off")), "no Resend");
    let before = attachment(cx, &view);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-check {id}"));
    settle(cx, w, 8);
    assert_eq!(attachment(cx, &view), before, "the same attachment: no reattach");
    assert!(!has(cx, w, format!("ask-check {id}")), "checking again");
    assert!(has(cx, w, format!("ask-resend {id} off")), "Resend waits");
    resynced(cx, &view, true);
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-resend {id} off")) && !has(cx, w, format!("ask-resend {id}")), "its history: Resend waits");
    resynced(cx, &view, false);
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-resend {id}")), "a digest says the slot: Resend");
    crate::app::pin_arm_clock(None);
}
