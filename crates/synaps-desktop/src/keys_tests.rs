//! Answering agent cards from the keyboard, in the window (synaps-cards
//! `docs/design.md` §5.1, §6.1, §7; §9.3's D2b row): the keyboard halves
//! of F1–F7 and F9, F8 and F13, each from its start state with exactly
//! the keys §7 counts; §7's negative tests (digits in a field, an IME
//! composition, a key repeat, a key before the card was painted, a mouse
//! answer inside a batch), each sending nothing; and the keys never
//! resolve a prompt, nor does Esc deny one (§5.1). And D3's way in,
//! [`AppView::enter_card`]: one card, by its binding. Held keys act once
//! at every way in (Shady #86): an Enter or Esc in a field or a note,
//! Ctrl+J, Tab; a repeat that lost its held bit (macOS's Shift+Tab, Esc
//! and arrows) acts no more than one that says it; and Tab goes round
//! the note a pick opens.
//!
//! What would have gone to the daemon is the view's record
//! (`AppView::card_sends`): exactly one `Submit` or `Steer`, its text and
//! its attachment. Keys go in as the platform sends them: a new press is
//! a down and an up, then past a repeat's 30 ms (`ui_tests::tap`, the
//! app's `FoldKeys` rule); a repeat is a down that says it is held.
//!
//! The window helpers are D2a's ([`super::answer_tests`]).

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, EntityInputHandler, Focusable as _, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::answer_tests::{
    SHIP, arm, ask_live, asked_messages, attachment, card, click, has, heard, land, llm, message, open, play, sends, show, showing,
};
use super::settle;
use crate::app::AppView;
use crate::deliver::Outgoing;

pub(super) fn keystroke(cx: &mut TestAppContext, k: &str) -> gpui_kit::Keystroke {
    gpui_kit::Keystroke::parse_with_os(k, cx.update(|cx| cx.platform_info().os)).expect("a keystroke").with_simulated_ime()
}

/// `k` goes down, as a new press: what nothing takes is typed into the
/// focused input, as the platform does (`Window::dispatch_keystroke`).
pub(super) fn down(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    let ks = keystroke(cx, k);
    cx.update_window(w, |_, window, cx| {
        window.dispatch_keystroke(ks, cx);
    })
    .expect("the window");
    cx.run_until_parked();
}

/// `k` goes down again while held: the platform's repeat (`is_held`).
pub(super) fn repeat(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    let keystroke = keystroke(cx, k);
    let ev = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent { keystroke, is_held: true, prefer_character_input: false });
    cx.update_window(w, |_, window, cx| {
        window.dispatch_event(ev, cx);
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, w, 8);
}

/// `k` goes down again while held, as GPUI's macOS backend sends a held
/// non-printing key (Shift+Tab, Esc, ↓) back from the input context's
/// `doCommandBySelector:`: rebuilt with `is_held: false`, no text, and no
/// key-up since its last key-down (Shady #86 R2-2).
pub(super) fn lost(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    let mut keystroke = keystroke(cx, k);
    keystroke.key_char = None;
    let ev = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent { keystroke, is_held: false, prefer_character_input: false });
    cx.update_window(w, |_, window, cx| {
        window.dispatch_event(ev, cx);
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, w, 8);
}

pub(super) fn up(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    let keystroke = keystroke(cx, k);
    cx.update_window(w, |_, window, cx| {
        window.dispatch_event(gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }), cx);
    })
    .expect("the window");
    cx.run_until_parked();
}

/// A new press of `k`: down and up, the frames it asks for, then the
/// input clock past a repeat's 30 ms ([`crate::app::next_press`]): the
/// next press of it is new, whatever the frames between cost.
pub(super) fn press(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    down(cx, w, k);
    up(cx, w, k);
    settle(cx, w, 8);
    crate::app::next_press();
}

/// `events` heard, and drawn as they land (a test window draws when the
/// update flushes), but no frame after: what a key meets at once.
fn hear_now(cx: &mut TestAppContext, view: &Entity<AppView>, events: Vec<Value>) {
    let (seq, sid) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let batch = events
            .into_iter()
            .map(|e| {
                let ev = synaps_client::protocol::decode_event(&e).unwrap_or_else(|err| panic!("{err}: {e}"));
                synaps_client::Event::session(seq, sid.clone(), 0, ev, std::sync::Arc::new(e))
            })
            .collect();
        v.apply_batch(batch, cx);
    });
    cx.run_until_parked();
}

/// The session's input is another client's (`other`), or this window's.
fn owned(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, other: bool) {
    view.update(cx, |v, cx| {
        let a = v.model.lives[v.panes.cur.live].active.as_mut().unwrap();
        a.input = if other { synaps_client::ClientInput::Exclusive } else { synaps_client::ClientInput::Shared };
        a.owner = Some(if other { a.client + 1 } else { a.client });
        cx.notify();
    });
    settle(cx, w, 8);
}

/// D3's way in: `AppView::enter_card(session, binding)` on the card that
/// call `tool` bound to card id `card`, as D3's chord or row would call it.
fn enter_card(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, session: &str, tool: &str, card: &str) -> bool {
    let b = crate::ask_keys::Binding { tool_id: tool.into(), card: cards_spec::CardId::parse(card).expect("a card id") };
    let entered = cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.enter_card(session, &b, window, cx))).expect("the window");
    settle(cx, w, 8);
    entered
}

/// Text typed into whatever input has the keys.
fn typing(cx: &mut TestAppContext, w: AnyWindowHandle, text: &str) {
    cx.simulate_input(w, text);
    settle(cx, w, 8);
}

/// Card row `id` is entered: the keys are its (its ring is drawn).
pub(super) fn entered(cx: &mut TestAppContext, w: AnyWindowHandle, id: u64) -> bool {
    has(cx, w, format!("ask-keys {id}"))
}

pub(super) fn composer_text(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, cx| v.panes.cur.composer.read(cx).value().to_string())
}

/// Who has the keys: the composer, or the window's own focus (where the
/// authorize card's Allow and Deny keys are live).
pub(super) fn keys_at(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> (bool, bool) {
    cx.update_window(w, |_, window, cx| {
        let v = view.read(cx);
        (v.panes.cur.composer.read(cx).focus_handle(cx).is_focused(window), v.focus.is_focused(window))
    })
    .expect("the window")
}

/// §7's start: the window active (in use), the composer with the keys.
pub(super) fn at_rest(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    cx.update_window(w, |_, window, cx| {
        window.activate_window();
        view.update(cx, |v, cx| v.panes.cur.composer.update(cx, |s, cx| s.focus(window, cx)))
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, w, 8);
    assert_eq!(keys_at(cx, w, view), (true, false), "the composer has the keys");
}

pub(super) fn texts(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    sends(cx, view).into_iter().map(|o| o.text).collect()
}

pub(super) fn ship_input() -> Value {
    json!({"question": SHIP, "wait": true,
        "detail": "It locks the tokens table for about 40 seconds.\nRefreshes queue meanwhile; none are lost.",
        "options": [{"label": "Run it now"}, {"label": "Wait for the low-traffic window", "hint": "after 22:00"}, "Don't run it"],
        "default": "Wait for the low-traffic window"})
}

const SUITES: &str = "Which suites should pass before the cut-over?";

fn suites_input() -> Value {
    json!({"question": SUITES, "multi": true,
        "options": [{"label": "Unit", "hint": "2 min"}, {"label": "Integration", "hint": "9 min"},
            {"label": "Load test", "hint": "40 min, staging only"}, "Chaos"],
        "default": ["Unit", "Integration"]})
}

pub(super) type Asked = (AnyWindowHandle, Entity<AppView>, u64, async_channel::Receiver<synaps_client::Event>);

/// A quiet session (its turns over, no card) whose agent asks `input`
/// live as card `card`, and waits.
pub(super) fn asked(cx: &mut TestAppContext, tool: &str, card_id: &str, input: &Value) -> Asked {
    let (w, view, events) = open(cx, crate::demo::IDLE_CPU);
    ask_live(cx, w, &view, tool, card_id, input.clone());
    let q = input["question"].as_str().expect("a question").to_string();
    let (id, _) = card(cx, &view, &q);
    at_rest(cx, w, &view);
    (w, view, id, events)
}

/// The draft input of field `field` of call `tool` on screen: its value.
pub(super) fn field_value(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str, field: &str) -> String {
    view.read_with(cx, |v, cx| {
        let sid = v.card_session().expect("attached");
        v.card_drafts[&(sid, tool.to_string())].lines[field].read(cx).value().to_string()
    })
}

/// The row of the card asking `question` in the focused pane, read from
/// that pane's own store (with split panes, `model.transcript` is the
/// store shown last, not necessarily this pane's).
fn card_here(cx: &mut TestAppContext, view: &Entity<AppView>, question: &str) -> u64 {
    view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].transcript.items();
        v.panes
            .cur
            .disp
            .iter()
            .find(|d| {
                d.kind == crate::turns::Kind::Ask
                    && matches!(items.get(d.item).and_then(|i| i.card.as_deref()),
                        Some(synaps_client::cards::CardRow::Ask(a)) if a.doc.question == question)
            })
            .map(|d| d.id)
            .unwrap_or_else(|| panic!("a card asking {question:?} here"))
    })
}

/// A confirm prompt pending on the session on screen.
pub(super) fn prompt(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(
        cx,
        w,
        view,
        vec![json!({"ev": "prompt", "request": {"id": 7, "kind": "confirm", "title": "Confirm tool call",
            "prompt": "Extension: guard\nTool: edit\n\nAllow this tool call?"}})],
    );
    play(cx, w, view);
}

/// The prompt is still pending, unanswered, and no answer to it went.
pub(super) fn prompt_pending(cx: &mut TestAppContext, view: &Entity<AppView>, what: &str) {
    view.read_with(cx, |v, _| {
        let l = &v.model.lives[v.panes.cur.live];
        assert!(l.transcript.pending_prompts().iter().any(|p| p.id == 7), "{what}: the prompt is still pending");
        assert!(!l.prompt_answered(7), "{what}: and unanswered");
        assert!(v.sent.is_none(), "{what}: no answer to it went");
    });
}

/// F1 (start A, 2 keys): the chip says what waits; Ctrl+J enters the card
/// the agent waits on (wait cards before open ones), its default
/// highlighted; Enter takes it: one `Submit` of the spec's message on the
/// card's attachment. No auto-focus: until Ctrl+J, the composer had the
/// keys.
#[gpui_kit::test]
fn f1_ctrl_j_then_enter_takes_the_suggested_option(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    assert!(has(cx, w, "composer-chip ◇ 2 questions · Ctrl+J".into()), "the chip: two open cards");
    assert!(!entered(cx, w, ship), "a card arriving never takes the keys");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, ship), "the card the agent waits on, first");
    assert!(!has(cx, w, "composer-chip ◇ 2 questions · Ctrl+J".into()), "no chip while a card is entered");
    assert!(has(cx, w, format!("ask-hi {ship} 2")), "its default highlighted");
    assert!(sends(cx, &view).is_empty());
    press(cx, w, "enter");
    let (seq, _) = attachment(cx, &view);
    let text = message("c0p6r4n", &ship_input(), json!({"choice": "Wait for the low-traffic window"}));
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing { seq, busy: false, card: cards_spec::CardId::parse("c0p6r4n").unwrap(), text }],
        "one Submit, on the card's attachment"
    );
}

/// F2 (A, 2 keys): Ctrl+J, a digit: that option.
#[gpui_kit::test]
fn f2_ctrl_j_then_a_digit_picks_another_option(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release", "hotfix"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_k2", "c2k2aaa", &input);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "2");
    let (seq, _) = attachment(cx, &view);
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing {
            seq,
            busy: false,
            card: cards_spec::CardId::parse("c2k2aaa").unwrap(),
            text: message("c2k2aaa", &input, json!({"choice": "release"}))
        }]
    );
    assert!(!entered(cx, w, id), "answered: the keys are back in the composer");
    assert_eq!(keys_at(cx, w, &view), (true, false));
}

/// F3 (A, 3 keys and typing): Ctrl+J, the option that asks for a note
/// (its note opens, the keys in it), the note, Enter: both.
#[gpui_kit::test]
fn f3_an_option_with_a_note(cx: &mut TestAppContext) {
    let input = json!({"question": "Ship it?", "wait": true, "options": ["Yes", {"label": "Not yet", "note": "What's missing?"}]});
    let (w, view, id, _events) = asked(cx, "toolu_k3", "c3k3not", &input);
    press(cx, w, "ctrl-j");
    press(cx, w, "2");
    assert!(sends(cx, &view).is_empty(), "a note to write first");
    assert!(has(cx, w, format!("ask-note {id}")));
    typing(cx, w, "the changelog");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c3k3not", &input, json!({"choice": "Not yet", "note": "the changelog"}))]);
}

/// F4 (A, 2 keys): a form prefilled and valid: Ctrl+J, Enter sends it as
/// prefilled, its option on the agent's default.
#[gpui_kit::test]
fn f4_a_prefilled_form_is_ctrl_j_enter(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASK_FORM);
    at_rest(cx, w, &view);
    let (id, _) = card(cx, &view, "How should the backfill run?");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "enter");
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1, "one Submit");
    let values = json!({"choice": "In batches", "fields": {"batch": "10000", "window": "22:00 to 23:30, rolled back by 23:45",
        "verify": true, "notes": "Page #relay-oncall if a batch takes over a minute.\nThe dashboards are under Relay / Tokens.", "ticket": ""}});
    let (cid, doc) = view.read_with(cx, |v, _| {
        let c = v.model.transcript.cards().cards().last().cloned().unwrap();
        (c.id.unwrap(), c.doc.unwrap())
    });
    let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&doc, &values).unwrap());
    assert_eq!(s[0].text, cards_spec::message::build(&cid, &doc, &reply));
}

/// F5 (A, 1 + (m−1) + 1 keys and typing): a form with m = 2 fields to
/// fill: Ctrl+J puts the keys in the first, Tab in the next, Enter in the
/// last sends the form, whole.
#[gpui_kit::test]
fn f5_a_form_to_fill_tab_between_fields_enter_sends(cx: &mut TestAppContext) {
    let input = json!({"question": "Who reviews the migration?", "wait": true, "fields": [
        {"id": "who", "label": "Reviewer", "placeholder": "a name"},
        {"id": "when", "label": "When"}]});
    let (w, view, _id, _events) = asked(cx, "toolu_k5", "c5k5frm", &input);
    press(cx, w, "ctrl-j");
    typing(cx, w, "Ana");
    press(cx, w, "tab");
    typing(cx, w, "Friday");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c5k5frm", &input, json!({"fields": {"who": "Ana", "when": "Friday"}}))]);
}

/// F6 (A, k + 2 keys): a multi pick, k of n: Ctrl+J, k digits, Enter.
#[gpui_kit::test]
fn f6_a_multi_pick_is_digits_then_enter(cx: &mut TestAppContext) {
    let input = json!({"question": "Which suites?", "wait": true, "multi": true, "options": ["Unit", "Integration", "Load", "Chaos"]});
    let (w, view, _id, _events) = asked(cx, "toolu_k6", "c6k6mul", &input);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    press(cx, w, "3");
    assert!(sends(cx, &view).is_empty(), "a form's pick sends nothing");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c6k6mul", &input, json!({"choice": ["Unit", "Load"]}))]);
}

/// Space: on a multi pick, the highlighted option (↓ moves it); on a
/// check (Tab to it), the check.
#[gpui_kit::test]
fn space_ticks_the_highlighted_option_and_a_check(cx: &mut TestAppContext) {
    let input = json!({"question": "What runs?", "wait": true, "multi": true, "options": ["lint", "unit", "e2e"],
        "fields": [{"id": "ci", "label": "On CI", "type": "check", "value": false}]});
    let (w, view, id, _events) = asked(cx, "toolu_ks", "c8kspac", &input);
    press(cx, w, "ctrl-j");
    assert!(has(cx, w, format!("ask-hi {id} 1")), "a multi pick starts on its first option");
    press(cx, w, "space");
    press(cx, w, "down");
    press(cx, w, "down");
    assert!(has(cx, w, format!("ask-hi {id} 3")));
    press(cx, w, "space");
    press(cx, w, "tab");
    assert!(has(cx, w, format!("ask-at {id} ci")), "Tab: the check");
    press(cx, w, "space");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c8kspac", &input, json!({"choice": ["lint", "e2e"], "fields": {"ci": true}}))]);
}

/// F7 (A, 2 keys): Ctrl+J, 0 skips.
#[gpui_kit::test]
fn f7_ctrl_j_then_0_skips(cx: &mut TestAppContext) {
    let input = json!({"question": "Rename it?", "wait": true, "options": ["Yes", "No"]});
    let (w, view, _id, _events) = asked(cx, "toolu_k7", "c7k7skp", &input);
    press(cx, w, "ctrl-j");
    press(cx, w, "0");
    assert_eq!(texts(cx, &view), vec!["[card c7k7skp skipped] Rename it? → (skipped)\n{}".to_string()]);
}

/// F8 (B, 2 keys): the card scrolled out of view: Ctrl+J brings it into
/// view and enters it; a digit answers.
#[gpui_kit::test]
fn f8_a_card_out_of_view_is_ctrl_j_then_a_key(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    show(cx, w, &view, 0);
    assert!(!has(cx, w, format!("ask-card {ship}")), "out of view");
    press(cx, w, "ctrl-j");
    assert!(has(cx, w, format!("ask-card {ship}")), "in view");
    assert!(entered(cx, w, ship));
    press(cx, w, "1");
    assert_eq!(texts(cx, &view), vec![message("c0p6r4n", &ship_input(), json!({"choice": "Run it now"}))]);
}

/// F9 (C, 2 keys): the card in the other pane: Ctrl+J from a pane with
/// none focuses the card's pane and enters it; a digit sends on that
/// pane's attachment, not the focused one's.
#[gpui_kit::test]
fn f9_a_card_in_the_other_pane_is_ctrl_j_then_a_key(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    let (seq, _) = attachment(cx, &view);
    let first = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, _| v.app_settings.split = true);
    press(cx, w, "ctrl-\\");
    play(cx, w, &view);
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        let open = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(open, crate::tabs::Place::Current);
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
    press(cx, w, "ctrl-j");
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), first, "the card's pane, focused");
    assert!(entered(cx, w, ship));
    press(cx, w, "3");
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1, "one Submit");
    assert_eq!(s[0].seq, seq, "on the card's pane's attachment");
    assert_eq!(s[0].text, message("c0p6r4n", &ship_input(), json!({"choice": "Don't run it"})));
}

/// F13 (A, N + 1 keys): N = 2 cards open at Ctrl+J: a batch, frozen in
/// the order (the card the agent waits on first); its footer says which
/// and what next; an answer enters the next; one key each.
#[gpui_kit::test]
fn f13_a_batch_is_one_key_a_card(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    let (suites, _) = card(cx, &view, SUITES);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, ship));
    assert!(has(cx, w, format!("ask-batch {ship} 1 of 2")), "1 of 2, next: the suites");
    press(cx, w, "2");
    assert!(entered(cx, w, suites), "the batch's next card");
    assert!(has(cx, w, format!("ask-batch {suites} 2 of 2")));
    press(cx, w, "enter");
    assert_eq!(
        texts(cx, &view),
        vec![
            message("c0p6r4n", &ship_input(), json!({"choice": "Wait for the low-traffic window"})),
            message("c5h8k2v", &suites_input(), json!({"choice": ["Unit", "Integration"]})),
        ],
        "one Submit a card"
    );
    assert!(!entered(cx, w, suites), "the batch is over: the composer");
    assert_eq!(keys_at(cx, w, &view), (true, false));
}

/// §7: digits (and `0`) typed in a field edit it; nothing is sent.
#[gpui_kit::test]
fn digits_typed_in_a_field_send_nothing(cx: &mut TestAppContext) {
    let input = json!({"question": "How many workers?", "wait": true, "options": ["One", "Two"], "default": "One",
        "fields": [{"id": "n", "label": "Workers"}]});
    let (w, view, _id, _events) = asked(cx, "toolu_kd", "c9kdigt", &input);
    press(cx, w, "ctrl-j");
    for k in ["2", "0", "1"] {
        press(cx, w, k);
    }
    assert!(sends(cx, &view).is_empty(), "digits in a field are its text");
    assert_eq!(field_value(cx, &view, "toolu_kd", "n"), "201");
}

/// §7: an IME composition owns every key. In a field mid-composition,
/// Esc (which leaves the card) is the composition's: the kit commits it,
/// and the card stays entered. Mid-composition again, Enter (which sends
/// a whole form) sends nothing. With no composition, Enter sends.
#[gpui_kit::test]
fn an_ime_composition_owns_every_key(cx: &mut TestAppContext) {
    let input = json!({"question": "Who reviews?", "wait": true, "fields": [{"id": "who", "label": "Reviewer", "value": "Ana"}]});
    let (w, view, id, _events) = asked(cx, "toolu_ki", "c1kime0", &input);
    press(cx, w, "ctrl-j");
    press(cx, w, "tab");
    let e = view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        v.card_drafts[&(sid, "toolu_ki".to_string())].lines["who"].clone()
    });
    let compose = |cx: &mut TestAppContext, text: &str| {
        cx.update_window(w, |_, window, cx| e.update(cx, |s, cx| s.replace_and_mark_text_in_range(None, text, None, window, cx)))
            .expect("the window");
        settle(cx, w, 8);
    };
    compose(cx, "あ");
    press(cx, w, "escape");
    assert!(entered(cx, w, id), "Esc mid-composition leaves nothing");
    assert_eq!(field_value(cx, &view, "toolu_ki", "who"), "Anaあ", "the composition, committed");
    compose(cx, "い");
    press(cx, w, "enter");
    assert!(sends(cx, &view).is_empty(), "Enter mid-composition sends nothing");
    let who = field_value(cx, &view, "toolu_ki", "who");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c1kime0", &input, json!({"fields": {"who": who}}))], "no composition: Enter sends");
}

/// §7: only new presses act. A repeat on the card does nothing; nor does
/// a key held from before Ctrl+J (it went down in the composer) when its
/// repeats reach the card. Let go and pressed again, it answers.
#[gpui_kit::test]
fn a_key_repeat_never_answers(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_kr", "c4krept", &input);
    down(cx, w, "2");
    settle(cx, w, 8);
    assert_eq!(composer_text(cx, &view), "2", "typed in the composer");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    repeat(cx, w, "2");
    repeat(cx, w, "2");
    assert!(sends(cx, &view).is_empty(), "a key held from before Ctrl+J");
    up(cx, w, "2");
    crate::app::next_press();
    down(cx, w, "1");
    repeat(cx, w, "1");
    up(cx, w, "1");
    settle(cx, w, 8);
    assert_eq!(texts(cx, &view), vec![message("c4krept", &input, json!({"choice": "main"}))], "the press, not its repeat");
    crate::app::next_press();
    press(cx, w, "2");
    assert_eq!(sends(cx, &view).len(), 1, "answered: nothing more");
}

/// §7: a key before the card was painted is no card key. GPUI draws a
/// window before it dispatches a key to it (`Window::dispatch_key_event`),
/// so such a key is one whose card that frame didn't show: here a card
/// just asked, still growing in (no frame of the motion clock yet). Its
/// key goes to the composer, typed; nothing is sent; the entry ends.
/// Grown in, Ctrl+J again and a key answer it.
#[gpui_kit::test]
fn a_key_before_the_card_was_painted_goes_to_the_composer(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    at_rest(cx, w, &view);
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    let (tool, card_id) = ("toolu_kb", "c2kbfor");
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"}),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": tool})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": tool, "input": input})),
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": showing(card_id, &input)})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    land(cx, w, &view, asked_messages(tool, card_id, &input, None));
    let (id, _) = card(cx, &view, "Which branch?");
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    assert!(sends(cx, &view).is_empty(), "nothing sent");
    assert_eq!(composer_text(cx, &view), "1", "the key went to the composer");
    assert!(!entered(cx, w, id), "the entry ended");
    assert_eq!(keys_at(cx, w, &view), (true, false));
    play(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "2");
    assert_eq!(texts(cx, &view), vec![message(card_id, &input, json!({"choice": "release"}))], "grown in, it answers");
}

/// §7: a mouse answer inside a batch ends it: one `Submit` for the click,
/// and the next key goes to the composer, answering nothing.
#[gpui_kit::test]
fn a_mouse_answer_inside_a_batch_ends_it(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    let (suites, _) = card(cx, &view, SUITES);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, ship));
    arm(cx, &view, ship, true);
    click(cx, w, &view, format!("ask-option {ship} 1"));
    crate::app::pin_arm_clock(None);
    assert_eq!(sends(cx, &view).len(), 1, "the click's answer");
    assert!(!entered(cx, w, suites), "never chained to the batch's next");
    press(cx, w, "1");
    assert_eq!(sends(cx, &view).len(), 1, "the key answered nothing");
    assert_eq!(composer_text(cx, &view), "1", "it went to the composer");
}

/// §5.1: the keys never resolve a prompt. With a confirm prompt pending,
/// Ctrl+J goes to it first (the window's keys); Ctrl+J again to the card;
/// `1` answers the card `yes`: its `Submit`, the prompt still pending;
/// Ctrl+J again, back to the prompt.
#[gpui_kit::test]
fn keys_moving_between_a_card_and_a_prompt_never_resolve_it(cx: &mut TestAppContext) {
    let input = json!({"question": "Allow it?", "options": ["yes", "always"]});
    let (w, view, id, _events) = asked(cx, "toolu_kp", "c6kprmt", &input);
    prompt(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_at(cx, w, &view), (false, true), "the prompt first: the window's keys");
    assert!(!entered(cx, w, id));
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "again: the card");
    assert_eq!(keys_at(cx, w, &view), (false, false), "never the window's keys");
    press(cx, w, "1");
    assert_eq!(texts(cx, &view), vec![message("c6kprmt", &input, json!({"choice": "yes"}))]);
    prompt_pending(cx, &view, "the card answered yes");
    press(cx, w, "ctrl-j");
    assert_eq!(keys_at(cx, w, &view), (false, true), "back to the prompt");
    prompt_pending(cx, &view, "back at the prompt");
}

/// §5.1: Esc never denies a prompt: Esc on an entered card goes to the
/// composer, never the window; a second Esc moves the keys to the prompt
/// (its keys wait again) and denies nothing.
#[gpui_kit::test]
fn esc_from_a_card_never_denies_a_prompt(cx: &mut TestAppContext) {
    let input = json!({"question": "Allow it?", "options": ["yes", "always"]});
    let (w, view, id, _events) = asked(cx, "toolu_ke", "c5kescp", &input);
    prompt(cx, w, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "escape");
    assert!(!entered(cx, w, id));
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer, not the window");
    prompt_pending(cx, &view, "one Esc");
    press(cx, w, "escape");
    prompt_pending(cx, &view, "a second Esc");
    assert!(sends(cx, &view).is_empty());
}

/// The chip is Ctrl+J: a click on it enters the card (F8's mouse half
/// starts here). A letter on an entered card goes to the composer, typed.
#[gpui_kit::test]
fn the_chip_is_ctrl_j_and_a_letter_goes_to_the_composer(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    let chip = "composer-chip ◇ 2 questions · Ctrl+J";
    let position = VisualTestContext::from_window(w, cx).debug_bounds(chip).expect("the chip").center();
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
    assert!(entered(cx, w, ship), "the chip's click is Ctrl+J");
    press(cx, w, "o");
    assert!(!entered(cx, w, ship), "a letter leaves");
    assert_eq!(composer_text(cx, &view), "o", "typed in the composer, never dropped");
    assert!(sends(cx, &view).is_empty());
}

/// The entry ends when the card is answered elsewhere (F14: another
/// desk's answer, its turn started): the keys go back to the composer,
/// and a digit is typed there.
#[gpui_kit::test]
fn an_entered_card_answered_elsewhere_hands_the_keys_back(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"]});
    let (w, view, id, _events) = asked(cx, "toolu_kx", "c3kelse", &input);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    let text = message("c3kelse", &input, json!({"choice": "release"}));
    // A key on the frame the answer lands: no frame between.
    hear_now(cx, &view, vec![json!({"ev": "turn_started", "turn_baseline": 44, "trigger": "user", "user_text": text})]);
    press(cx, w, "2");
    assert!(sends(cx, &view).is_empty(), "the key at once: nothing sent");
    assert_eq!(composer_text(cx, &view), "2", "the key at once: the composer's");
    assert!(!entered(cx, w, id), "answered elsewhere: no longer entered");
    assert_eq!(keys_at(cx, w, &view), (true, false));
    press(cx, w, "1");
    assert!(sends(cx, &view).is_empty());
    assert_eq!(composer_text(cx, &view), "21");
}

/// The keys on a card this window can't answer (another client owns the
/// session's input) send nothing, as a click there would; once it can,
/// a key answers.
#[gpui_kit::test]
fn keys_on_a_card_this_window_cannot_answer_send_nothing(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    owned(cx, w, &view, true);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, ship), "entered: its footer says why it can't be answered");
    for k in ["1", "enter", "0"] {
        press(cx, w, k);
    }
    assert!(sends(cx, &view).is_empty(), "read-only: nothing sent");
    owned(cx, w, &view, false);
    press(cx, w, "1");
    assert_eq!(texts(cx, &view), vec![message("c0p6r4n", &ship_input(), json!({"choice": "Run it now"}))]);
}

/// The entry ends when its card scrolls out of view (§6.1), or the window
/// goes inactive: the keys go back to the composer, and a digit is typed
/// there.
#[gpui_kit::test]
fn an_entered_card_scrolled_away_or_the_window_left_hands_the_keys_back(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, ship));
    show(cx, w, &view, 0);
    assert!(!entered(cx, w, ship), "scrolled away: no longer entered");
    assert_eq!(keys_at(cx, w, &view), (true, false));
    press(cx, w, "1");
    assert!(sends(cx, &view).is_empty());
    assert_eq!(composer_text(cx, &view), "1");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, ship), "Ctrl+J again");
    VisualTestContext::from_window(w, cx).deactivate_window();
    settle(cx, w, 8);
    cx.update_window(w, |_, window, _| window.activate_window()).expect("the window");
    settle(cx, w, 8);
    assert!(!entered(cx, w, ship), "the window left: no longer entered");
    assert_eq!(keys_at(cx, w, &view), (true, false), "back: the composer has the keys, never a card");
    press(cx, w, "2");
    assert!(sends(cx, &view).is_empty());
}

/// Ctrl+J's order across panes (§6.1): of two cards the agents wait on,
/// the focused pane's comes first, whatever the reading order.
#[gpui_kit::test]
fn ctrl_j_takes_the_focused_panes_card_first(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (ship, _) = card(cx, &view, SHIP);
    view.update(cx, |v, _| v.app_settings.split = true);
    press(cx, w, "ctrl-\\");
    play(cx, w, &view);
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        let open = crate::app::Open::Attach { session_id: crate::demo::ASK_FORM.into(), fallback: None, open: false };
        v.open_tab(open, crate::tabs::Place::Current);
        cx.notify();
        c
    });
    play(cx, w, &view);
    view.update(cx, |v, _| v.client = client);
    at_rest(cx, w, &view);
    let plan = card_here(cx, &view, "How should the backfill run?");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, plan), "this pane's card");
    assert!(!entered(cx, w, ship), "not the other pane's, first in reading order");
}

/// A card scrolled out of view in a window at rest (a jump: one frame,
/// nothing else animating) still hands the keys back: the frame that
/// didn't paint it asks for one more, whose start ends the entry. A key
/// after it is the composer's, never an answer to a card out of sight.
#[gpui_kit::test]
fn a_key_after_the_card_scrolled_away_at_rest_is_the_composers(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"]});
    let (w, view, id, _events) = asked(cx, "toolu_kw", "c5kaway", &input);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    show(cx, w, &view, 0);
    assert!(!has(cx, w, format!("ask-card {id}")), "out of view");
    press(cx, w, "1");
    assert!(sends(cx, &view).is_empty(), "nothing sent");
    assert_eq!(composer_text(cx, &view), "1", "the composer's");
}

/// Read-only, a multi pick's keys change nothing either: no option ticks,
/// as no click would (its controls have no listener then). Once the
/// window can answer, a digit ticks, and Enter sends only that.
#[gpui_kit::test]
fn keys_on_a_read_only_multi_pick_tick_nothing(cx: &mut TestAppContext) {
    let input = json!({"question": "Which suites?", "wait": true, "multi": true, "options": ["Unit", "Integration", "Load", "Chaos"]});
    let (w, view, id, _events) = asked(cx, "toolu_kq", "c7kronl", &input);
    owned(cx, w, &view, true);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "1");
    press(cx, w, "2");
    press(cx, w, "enter");
    assert!(sends(cx, &view).is_empty(), "read-only: nothing sent");
    owned(cx, w, &view, false);
    press(cx, w, "3");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c7kronl", &input, json!({"choice": ["Load"]}))], "only what was ticked once it could");
}

/// D3's way in (Shady #83 P2-7): `enter_card(session, binding)` enters the
/// card named, not the first that waits. Checked again first, nothing
/// changes when it isn't open there now: the id's earlier call (answered,
/// the history's binding moved on), another session. The card named is
/// entered with its pane's other card after it (the batch rule); Enter
/// answers it: one `Submit` of its text on its attachment. The batch goes
/// on; the card answered (its answer on its way, the memo's) isn't
/// entered again.
#[gpui_kit::test]
fn enter_card_enters_the_card_named_and_a_key_answers_it(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    let (suites, _) = card(cx, &view, SUITES);
    let (seq, sid) = attachment(cx, &view);
    assert!(!enter_card(cx, w, &view, &sid, "toolu_a6", "c5h8k2v"), "the id's earlier call: not its binding now");
    assert!(!enter_card(cx, w, &view, "20990101-000000-none", "toolu_a7", "c5h8k2v"), "another session");
    assert!(!entered(cx, w, suites) && !entered(cx, w, ship), "nothing entered");
    assert_eq!(keys_at(cx, w, &view), (true, false), "the keys where they were");
    assert!(enter_card(cx, w, &view, &sid, "toolu_a7", "c5h8k2v"));
    assert!(entered(cx, w, suites), "the card named, not Ctrl+J's first (the migration's)");
    assert!(has(cx, w, format!("ask-batch {suites} 1 of 2")), "its pane's other card after it");
    press(cx, w, "enter");
    let text = message("c5h8k2v", &suites_input(), json!({"choice": ["Unit", "Integration"]}));
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing { seq, busy: false, card: cards_spec::CardId::parse("c5h8k2v").unwrap(), text }],
        "one Submit, on its attachment"
    );
    assert!(entered(cx, w, ship), "the batch goes on");
    press(cx, w, "escape");
    assert!(!enter_card(cx, w, &view, &sid, "toolu_a7", "c5h8k2v"), "answered: not entered again");
    assert_eq!(keys_at(cx, w, &view), (true, false));
}

/// `enter_card` with a confirm prompt pending: the card, never the prompt
/// (§5.1). A card answered "always" by key leaves the prompt pending, and
/// Esc from the card doesn't deny it.
#[gpui_kit::test]
fn enter_card_never_answers_a_prompt(cx: &mut TestAppContext) {
    let input = json!({"question": "Go ahead?", "wait": true, "options": ["yes", "always"]});
    let (w, view, id, _events) = asked(cx, "toolu_kp", "c8kprmt", &input);
    prompt(cx, w, &view);
    let sid = attachment(cx, &view).1;
    assert!(enter_card(cx, w, &view, &sid, "toolu_kp", "c8kprmt"));
    assert!(entered(cx, w, id));
    press(cx, w, "escape");
    prompt_pending(cx, &view, "Esc");
    assert!(enter_card(cx, w, &view, &sid, "toolu_kp", "c8kprmt"));
    press(cx, w, "2");
    assert_eq!(texts(cx, &view), vec![message("c8kprmt", &input, json!({"choice": "always"}))]);
    prompt_pending(cx, &view, "answered always");
}

/// Ctrl+J mid IME composition in the composer is the IME's (§6.1: it owns
/// every key; Shady #83): no card is entered, the composition stays.
/// Committed, Ctrl+J enters, and Enter answers.
#[gpui_kit::test]
fn ctrl_j_mid_composition_is_the_imes(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_kj", "c9kjime", &input);
    let c = view.read_with(cx, |v, _| v.panes.cur.composer.clone());
    cx.update_window(w, |_, window, cx| {
        c.update(cx, |s, cx| EntityInputHandler::replace_and_mark_text_in_range(s, None, "か", None, window, cx))
    })
    .expect("the window");
    settle(cx, w, 8);
    press(cx, w, "ctrl-j");
    assert!(!entered(cx, w, id), "mid-composition: no card");
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer keeps the keys");
    let marked = cx
        .update_window(w, |_, window, cx| c.update(cx, |s, cx| EntityInputHandler::marked_text_range(s, window, cx)))
        .expect("the window");
    assert!(marked.is_some(), "the composition stays");
    cx.update_window(w, |_, window, cx| c.update(cx, |s, cx| EntityInputHandler::unmark_text(s, window, cx))).expect("the window");
    settle(cx, w, 8);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "committed: Ctrl+J enters");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c9kjime", &input, json!({"choice": "main"}))]);
}

/// Where the keys are on the card of call `tool`: `body` (the pane's card
/// focus), `note`, a text field's id (a line or a long one); else
/// `elsewhere`.
pub(super) fn keys_in(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str) -> String {
    cx.update_window(w, |_, window, cx| {
        let v = view.read(cx);
        if v.panes.cur.ask.focus.as_ref().is_some_and(|f| f.is_focused(window)) {
            return "body".to_string();
        }
        let Some(d) = v.card_session().and_then(|s| v.card_drafts.get(&(s, tool.to_string()))) else { return "elsewhere".to_string() };
        if d.note.as_ref().is_some_and(|i| i.read(cx).focus_handle(cx).is_focused(window)) {
            return "note".to_string();
        }
        let line = d.lines.iter().find(|(_, i)| i.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id);
        let long = d.longs.iter().find(|(_, i)| i.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id);
        line.or(long).cloned().unwrap_or_else(|| "elsewhere".to_string())
    })
    .expect("the window")
}

/// The note's text on the card of call `tool`.
fn note_text(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str) -> String {
    view.read_with(cx, |v, cx| {
        let sid = v.card_session().expect("attached");
        v.card_drafts[&(sid, tool.to_string())].note.as_ref().expect("a note").read(cx).value().to_string()
    })
}

/// The prompt's keys are live (its card on screen, the window's focus):
/// where a Ctrl+J pressed again goes on to the agent cards. Frames until
/// its row has come into view (the motion's time, not the keys').
fn prompt_keys_live(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    for _ in 0..100 {
        if view.read_with(cx, |v, _| v.panes.cur.card_keys.get().is_some()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        settle(cx, w, 1);
    }
    panic!("the prompt's keys never came live");
}

/// A form whole as asked: two text fields, prefilled.
fn deploy_input() -> Value {
    json!({"question": "Deploy where?", "wait": true, "fields": [
        {"id": "env", "label": "Environment", "value": "staging"},
        {"id": "region", "label": "Region", "value": "eu-west-1"}]})
}

/// Shady #86 P2-1: Enter in a field acts on a new, bare press only, as on
/// the card. An Enter held from the composer, its repeats reaching the
/// prefilled field the keys moved to, sends nothing and moves nothing;
/// nor does Shift+Enter. Let go and pressed again, Enter sends the form,
/// once.
#[gpui_kit::test]
fn a_held_enter_in_a_field_sends_nothing(cx: &mut TestAppContext) {
    let input = deploy_input();
    let (w, view, id, _events) = asked(cx, "toolu_he", "c1hfent", &input);
    down(cx, w, "enter");
    settle(cx, w, 8);
    assert!(sends(cx, &view).is_empty(), "an empty composer sends nothing");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    assert_eq!(keys_in(cx, w, &view, "toolu_he"), "body", "a whole form: the keys on the card");
    press(cx, w, "tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_he"), "env");
    for n in 1..=2 {
        repeat(cx, w, "enter");
        assert!(sends(cx, &view).is_empty(), "repeat {n}: Enter held from the composer, no Submit");
        assert_eq!(keys_in(cx, w, &view, "toolu_he"), "env", "repeat {n}: and no move");
    }
    up(cx, w, "enter");
    crate::app::next_press();
    press(cx, w, "shift-enter");
    assert!(sends(cx, &view).is_empty(), "Shift+Enter is no card's key");
    assert_eq!(keys_in(cx, w, &view, "toolu_he"), "env");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c1hfent", &input, json!({"fields": {"env": "staging", "region": "eu-west-1"}}))]);
}

/// P2-1 in the note: an Enter held from the composer, its repeats in the
/// note the pick opened, sends nothing; pressed again, it sends.
#[gpui_kit::test]
fn a_held_enter_in_a_note_sends_nothing(cx: &mut TestAppContext) {
    let input = json!({"question": "Ship it?", "wait": true, "options": ["Yes", {"label": "Not yet", "note": "What's missing?"}]});
    let (w, view, id, _events) = asked(cx, "toolu_hn", "c2hnote", &input);
    down(cx, w, "enter");
    settle(cx, w, 8);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "2");
    assert_eq!(keys_in(cx, w, &view, "toolu_hn"), "note");
    typing(cx, w, "the changelog");
    repeat(cx, w, "enter");
    assert!(sends(cx, &view).is_empty(), "Enter held from the composer: no Submit");
    assert_eq!(keys_in(cx, w, &view, "toolu_hn"), "note");
    up(cx, w, "enter");
    crate::app::next_press();
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c2hnote", &input, json!({"choice": "Not yet", "note": "the changelog"}))]);
}

/// P2-1 in a form to fill: Enter in its first field moves to the next;
/// its repeat there moves nothing more (it went round to the card).
#[gpui_kit::test]
fn a_held_enter_in_a_form_to_fill_moves_once(cx: &mut TestAppContext) {
    let input = json!({"question": "Who reviews the migration?", "wait": true, "fields": [
        {"id": "who", "label": "Reviewer"}, {"id": "when", "label": "When"}]});
    let (w, view, _id, _events) = asked(cx, "toolu_hf", "c3hform", &input);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_in(cx, w, &view, "toolu_hf"), "who");
    typing(cx, w, "Ana");
    down(cx, w, "enter");
    settle(cx, w, 8);
    assert_eq!(keys_in(cx, w, &view, "toolu_hf"), "when", "a press: on to the next field");
    for n in 1..=2 {
        repeat(cx, w, "enter");
        assert_eq!(keys_in(cx, w, &view, "toolu_hf"), "when", "repeat {n}: nowhere");
    }
    up(cx, w, "enter");
    crate::app::next_press();
    typing(cx, w, "Friday");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c3hform", &input, json!({"fields": {"who": "Ana", "when": "Friday"}}))]);
}

/// Shady #86 P2-2: a held Ctrl+J jumps once. At a prompt and a card, it
/// goes to the prompt, and its repeats never go on to the card; let go
/// and pressed again, it does.
#[gpui_kit::test]
fn a_held_ctrl_j_stays_at_the_prompt(cx: &mut TestAppContext) {
    let input = json!({"question": "Allow it?", "options": ["yes", "always"]});
    let (w, view, id, _events) = asked(cx, "toolu_hj", "c4hjump", &input);
    prompt(cx, w, &view);
    down(cx, w, "ctrl-j");
    settle(cx, w, 8);
    assert_eq!(keys_at(cx, w, &view), (false, true), "the prompt first");
    prompt_keys_live(cx, w, &view);
    for n in 1..=3 {
        repeat(cx, w, "ctrl-j");
        assert!(!entered(cx, w, id), "repeat {n}: still the prompt");
        assert_eq!(keys_at(cx, w, &view), (false, true), "repeat {n}");
    }
    up(cx, w, "ctrl-j");
    crate::app::next_press();
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "a new press: the card");
    prompt_pending(cx, &view, "Ctrl+J held, then pressed");
    assert!(sends(cx, &view).is_empty());
}

/// P2-2: the batch a Ctrl+J froze stays while it is held: a card that
/// comes meanwhile does not join it. A new press freezes a new one.
#[gpui_kit::test]
fn a_held_ctrl_j_freezes_one_batch(cx: &mut TestAppContext) {
    let first = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_hb", "c5hbtch", &first);
    down(cx, w, "ctrl-j");
    settle(cx, w, 8);
    assert!(entered(cx, w, id));
    assert!(!has(cx, w, format!("ask-batch {id} 1 of 2")), "a batch of one");
    let second = json!({"question": "Tag it?", "options": ["yes", "no"]});
    ask_live(cx, w, &view, "toolu_hb2", "c5hbtc2", second);
    repeat(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "the card stays entered");
    assert!(!has(cx, w, format!("ask-batch {id} 1 of 2")), "held: the batch as frozen");
    up(cx, w, "ctrl-j");
    crate::app::next_press();
    press(cx, w, "ctrl-j");
    assert!(has(cx, w, format!("ask-batch {id} 1 of 2")), "a new press: a new batch");
    assert!(sends(cx, &view).is_empty());
}

/// P2-2: Tab and Shift+Tab move one stop a press, from a field (the
/// input's Tab) and from the card (its focus's): a repeat moves nothing.
#[gpui_kit::test]
fn a_held_tab_moves_one_stop(cx: &mut TestAppContext) {
    let input = json!({"question": "Who reviews the migration?", "wait": true, "fields": [
        {"id": "who", "label": "Reviewer"}, {"id": "when", "label": "When"}]});
    let (w, view, _id, _events) = asked(cx, "toolu_ht", "c6htab0", &input);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_in(cx, w, &view, "toolu_ht"), "who");
    down(cx, w, "tab");
    settle(cx, w, 8);
    assert_eq!(keys_in(cx, w, &view, "toolu_ht"), "when");
    for n in 1..=2 {
        repeat(cx, w, "tab");
        assert_eq!(keys_in(cx, w, &view, "toolu_ht"), "when", "a field's Tab held, repeat {n}: one stop");
    }
    up(cx, w, "tab");
    crate::app::next_press();
    press(cx, w, "shift-tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_ht"), "who");
    down(cx, w, "shift-tab");
    settle(cx, w, 8);
    assert_eq!(keys_in(cx, w, &view, "toolu_ht"), "body");
    for n in 1..=2 {
        repeat(cx, w, "shift-tab");
        assert_eq!(keys_in(cx, w, &view, "toolu_ht"), "body", "the card's Shift+Tab held, repeat {n}: one stop");
    }
    up(cx, w, "shift-tab");
    assert!(sends(cx, &view).is_empty());
}

/// Shady #86 P2-3: the note a pick opens is a stop of Tab's round. On a
/// quick card: the card, the note; both ways, its text kept, nothing sent.
#[gpui_kit::test]
fn tab_goes_round_the_note(cx: &mut TestAppContext) {
    let input = json!({"question": "Ship it?", "wait": true, "options": ["Yes", {"label": "Not yet", "note": "What's missing?"}]});
    let (w, view, _id, _events) = asked(cx, "toolu_tn", "c7tnote", &input);
    press(cx, w, "ctrl-j");
    press(cx, w, "2");
    assert_eq!(keys_in(cx, w, &view, "toolu_tn"), "note");
    typing(cx, w, "the change");
    for (k, at) in [("tab", "body"), ("tab", "note"), ("shift-tab", "body"), ("shift-tab", "note")] {
        press(cx, w, k);
        assert_eq!(keys_in(cx, w, &view, "toolu_tn"), at, "{k}");
    }
    assert_eq!(note_text(cx, &view, "toolu_tn"), "the change", "the note as written");
    assert!(sends(cx, &view).is_empty());
    typing(cx, w, "log");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c7tnote", &input, json!({"choice": "Not yet", "note": "the changelog"}))]);
}

/// P2-3 on a form: the card, its pick's note, then its fields, round, both
/// ways: from the note and from a named field. Each draft kept, nothing
/// sent until Enter.
#[gpui_kit::test]
fn tab_goes_round_a_forms_note_and_fields(cx: &mut TestAppContext) {
    let input = json!({"question": "Roll it out?", "wait": true,
        "options": ["Now", {"label": "Later", "note": "When?"}],
        "fields": [{"id": "owner", "label": "Owner"}, {"id": "ticket", "label": "Ticket", "optional": true}]});
    let (w, view, _id, _events) = asked(cx, "toolu_tf", "c8tform", &input);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_in(cx, w, &view, "toolu_tf"), "owner", "the first field to fill");
    typing(cx, w, "Ana");
    press(cx, w, "shift-tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_tf"), "body");
    press(cx, w, "2");
    assert_eq!(keys_in(cx, w, &view, "toolu_tf"), "note", "the pick's note opens");
    typing(cx, w, "after the freeze");
    let round = [
        ("tab", "owner"),
        ("shift-tab", "note"),
        ("shift-tab", "body"),
        ("shift-tab", "ticket"),
        ("shift-tab", "owner"),
        ("shift-tab", "note"),
        ("tab", "owner"),
        ("tab", "ticket"),
        ("tab", "body"),
        ("tab", "note"),
    ];
    for (k, at) in round {
        press(cx, w, k);
        assert_eq!(keys_in(cx, w, &view, "toolu_tf"), at, "{k} to {at}");
    }
    assert_eq!(note_text(cx, &view, "toolu_tf"), "after the freeze");
    assert_eq!(field_value(cx, &view, "toolu_tf", "owner"), "Ana");
    assert!(sends(cx, &view).is_empty(), "Tab sends nothing");
    press(cx, w, "enter");
    assert_eq!(
        texts(cx, &view),
        vec![message("c8tform", &input, json!({"choice": "Later", "note": "after the freeze", "fields": {"owner": "Ana", "ticket": ""}}))]
    );
}

/// Shady #86 P3-1: the card's keys and the card chord (Enter, Esc, Space)
/// read one clock, the input clock (`crate::app::input_now`), put here an
/// hour ahead of the wall's: a stamp left on the wall would be an hour
/// off. Enter let go in the composer, then pressed on the entered card
/// with only the wall's 40 ms between, and again 29 ms later on the input
/// clock: a repeat to both gates, nothing sent. At 30 ms, a press to
/// both: it answers.
#[gpui_kit::test]
fn card_keys_and_the_chord_read_one_input_clock(cx: &mut TestAppContext) {
    use std::time::Duration;
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_kc", "c9kclck", &input);
    crate::app::advance_input_clock(Duration::from_secs(3600));
    let gates = |cx: &mut TestAppContext| view.read_with(cx, |v, _| (v.panes.cur.chord_press, v.ask_presses.last));
    down(cx, w, "enter");
    up(cx, w, "enter");
    settle(cx, w, 8);
    assert!(sends(cx, &view).is_empty(), "an empty composer sends nothing");
    down(cx, w, "ctrl-j");
    up(cx, w, "ctrl-j");
    settle(cx, w, 8);
    assert!(entered(cx, w, id));
    std::thread::sleep(Duration::from_millis(40));
    for (gap, press) in [(Duration::ZERO, false), (Duration::from_millis(29), false), (Duration::from_millis(30), true)] {
        crate::app::advance_input_clock(gap);
        down(cx, w, "enter");
        assert_eq!(gates(cx), (press, press), "{gap:?} after the key-up: the chord's and the card's gates agree");
        up(cx, w, "enter");
        settle(cx, w, 8);
        let sent = if press { vec![message("c9kclck", &input, json!({"choice": "main"}))] } else { vec![] };
        assert_eq!(texts(cx, &view), sent, "{gap:?} after the key-up");
    }
}

/// Shady #86 R2-1: Esc in a field goes back to the composer on a new press
/// only, as on the card. An Esc held from before the keys came to the
/// field (its repeats reach it) leaves nothing. Nor does the Esc that
/// ends an IME composition (the input's own `Escape` ends it, before its
/// key-down), nor its repeats: held, or one that lost its held bit (no
/// key-up between, past `FoldKeys::STALE`). Let go and pressed again, Esc
/// leaves, the draft kept; the prompt stays pending throughout.
#[gpui_kit::test]
fn a_held_esc_in_a_field_leaves_nothing(cx: &mut TestAppContext) {
    let input = json!({"question": "Who reviews?", "wait": true, "fields": [{"id": "who", "label": "Reviewer", "value": "Ana"}]});
    let (w, view, id, _events) = asked(cx, "toolu_hx", "c4hesc0", &input);
    prompt(cx, w, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "Ctrl+J again: the card");
    press(cx, w, "tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_hx"), "who");
    for n in 1..=2 {
        repeat(cx, w, "escape");
        assert!(entered(cx, w, id), "repeat {n}: an Esc held from before leaves nothing");
        assert_eq!(keys_in(cx, w, &view, "toolu_hx"), "who", "repeat {n}");
    }
    up(cx, w, "escape");
    crate::app::next_press();
    let e = view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        v.card_drafts[&(sid, "toolu_hx".to_string())].lines["who"].clone()
    });
    cx.update_window(w, |_, window, cx| e.update(cx, |s, cx| s.replace_and_mark_text_in_range(None, "あ", None, window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    down(cx, w, "escape");
    settle(cx, w, 8);
    assert!(entered(cx, w, id), "the Esc that ends a composition leaves nothing");
    assert_eq!(field_value(cx, &view, "toolu_hx", "who"), "Anaあ", "the composition, ended");
    repeat(cx, w, "escape");
    assert!(entered(cx, w, id), "its repeat, held");
    crate::app::advance_input_clock(std::time::Duration::from_millis(1200));
    lost(cx, w, "escape");
    assert!(entered(cx, w, id), "its repeat with no held bit, 1.2 s on");
    assert_eq!(keys_in(cx, w, &view, "toolu_hx"), "who");
    up(cx, w, "escape");
    crate::app::next_press();
    press(cx, w, "escape");
    assert!(!entered(cx, w, id), "let go and pressed again: Esc leaves");
    assert_eq!(keys_at(cx, w, &view), (true, false), "to the composer, never the window");
    assert_eq!(field_value(cx, &view, "toolu_hx", "who"), "Anaあ", "the draft kept");
    prompt_pending(cx, &view, "Esc in a field");
    assert!(sends(cx, &view).is_empty());
}

/// Shady #86 R2-2: a press is a key-down after that key's key-up, whatever
/// the key-down says. GPUI's macOS backend can send a held Shift+Tab's
/// repeats with no held bit ([`lost`]): it still moves one stop, from a
/// field and from the card, however far apart its repeats come (the old
/// rule took one past `FoldKeys::STALE` for a press here, and every one
/// on macOS). Let go and pressed again, it moves.
#[gpui_kit::test]
fn a_held_shift_tab_without_its_held_bit_moves_one_stop(cx: &mut TestAppContext) {
    use std::time::Duration;
    let input = json!({"question": "Who reviews the migration?", "wait": true, "fields": [
        {"id": "who", "label": "Reviewer"}, {"id": "when", "label": "When"}]});
    let (w, view, _id, _events) = asked(cx, "toolu_hl", "c5hlost", &input);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_in(cx, w, &view, "toolu_hl"), "who");
    press(cx, w, "tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_hl"), "when");
    for (from, to) in [("when", "who"), ("who", "body")] {
        assert_eq!(keys_in(cx, w, &view, "toolu_hl"), from);
        down(cx, w, "shift-tab");
        settle(cx, w, 8);
        assert_eq!(keys_in(cx, w, &view, "toolu_hl"), to, "a press: one stop back");
        for gap in [0, 30, 90, 1200, 2000] {
            crate::app::advance_input_clock(Duration::from_millis(gap));
            lost(cx, w, "shift-tab");
            assert_eq!(keys_in(cx, w, &view, "toolu_hl"), to, "from {from}: a repeat with no held bit, {gap} ms on, moves nothing");
        }
        up(cx, w, "shift-tab");
        crate::app::next_press();
    }
    press(cx, w, "shift-tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_hl"), "when", "let go and pressed again: round to the last field");
    assert!(sends(cx, &view).is_empty());
}

/// R2-2 on the card: a held ↓ whose repeats lost their held bit moves the
/// highlight once; let go and pressed again, once more, and Enter takes
/// the option it is on.
#[gpui_kit::test]
fn a_held_arrow_without_its_held_bit_moves_once(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release", "hotfix", "next"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_ha", "c6harrw", &input);
    let hi = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.cur.ask.entry.as_ref().and_then(|e| e.hi));
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    assert_eq!(hi(cx), Some(0), "on the default");
    down(cx, w, "down");
    settle(cx, w, 8);
    assert_eq!(hi(cx), Some(1), "a press: one down");
    for gap in [0, 90, 1200, 2000] {
        crate::app::advance_input_clock(std::time::Duration::from_millis(gap));
        lost(cx, w, "down");
        assert_eq!(hi(cx), Some(1), "a repeat with no held bit, {gap} ms on, moves nothing");
    }
    up(cx, w, "down");
    crate::app::next_press();
    press(cx, w, "down");
    assert_eq!(hi(cx), Some(2), "let go and pressed again: one more");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view), vec![message("c6harrw", &input, json!({"choice": "hotfix"}))]);
}

/// R2-2's one exception: AppKit drops the key-up of a key let go while
/// Cmd is held. After Cmd+2 (a tab, on macOS) with no key-up seen, `2` on
/// the entered card is still a new press (it types text, so it says when
/// it repeats): it answers.
#[gpui_kit::test]
fn a_key_up_lost_under_cmd_costs_no_press(cx: &mut TestAppContext) {
    let input = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"], "default": "main"});
    let (w, view, id, _events) = asked(cx, "toolu_hc", "c7hcmd2", &input);
    let ks = keystroke(cx, "cmd-2");
    let ev = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent { keystroke: ks, is_held: false, prefer_character_input: false });
    cx.update_window(w, |_, window, cx| {
        window.dispatch_event(ev, cx);
    })
    .expect("the window");
    settle(cx, w, 8);
    crate::app::next_press();
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    press(cx, w, "2");
    assert_eq!(texts(cx, &view), vec![message("c7hcmd2", &input, json!({"choice": "release"}))]);
}

/// The highlight on the entered card, if one.
fn hi(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<usize> {
    view.read_with(cx, |v, _| v.panes.cur.ask.entry.as_ref().and_then(|e| e.hi))
}

/// Shady #86 R3-1, after the keys reach card `id` (default highlighted:
/// option 1) with ↓ held from where an input's `MoveDown` took it (the
/// root's capture never heard that key-down): its repeats that lost their
/// held bit ([`lost`], macOS's), or say it, move nothing and send nothing,
/// however far apart. Let go, within 30 ms (X11's repeat) still nothing;
/// past it, a press moves one, and Enter takes that option.
fn held_down_moves_nothing(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, from: &str, card: (&str, &Value)) {
    use std::time::Duration;
    assert_eq!(hi(cx, view), Some(0), "{from}: on the default");
    for gap in [0, 90, 1200] {
        crate::app::advance_input_clock(Duration::from_millis(gap));
        lost(cx, w, "down");
        assert_eq!(hi(cx, view), Some(0), "{from}: ↓ held from before, a repeat with no held bit {gap} ms on, moves nothing");
    }
    repeat(cx, w, "down");
    assert_eq!(hi(cx, view), Some(0), "{from}: one that says it repeats");
    assert!(sends(cx, view).is_empty(), "{from}: nothing sent");
    up(cx, w, "down");
    down(cx, w, "down");
    settle(cx, w, 8);
    assert_eq!(hi(cx, view), Some(0), "{from}: within 30 ms of its key-up, a repeat");
    up(cx, w, "down");
    crate::app::next_press();
    press(cx, w, "down");
    assert_eq!(hi(cx, view), Some(1), "{from}: let go, 30 ms, pressed: one down");
    press(cx, w, "enter");
    let (id, input) = card;
    assert_eq!(texts(cx, view), vec![message(id, input, json!({"choice": "release"}))], "{from}");
}

/// Four options, the first the default: a held ↓ that moved shows.
fn branch_input() -> Value {
    json!({"question": "Which branch?", "wait": true, "options": ["main", "release", "hotfix", "next"], "default": "main"})
}

/// R3-1, Shady's sequence: ↓ goes down in the composer (its `MoveDown`
/// takes it) and stays down; `chord` (Ctrl+J, or D3's Ctrl+Shift+J)
/// enters the card; then ↓'s repeats.
fn down_held_in_the_composer(cx: &mut TestAppContext, chord: &str) {
    let input = branch_input();
    let (w, view, id, _events) = asked(cx, "toolu_hd", "c3hdown", &input);
    down(cx, w, "down");
    settle(cx, w, 8);
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer took the ↓");
    press(cx, w, chord);
    play(cx, w, &view);
    assert!(entered(cx, w, id), "{chord}: the card");
    held_down_moves_nothing(cx, w, &view, chord, ("c3hdown", &input));
}

#[gpui_kit::test]
fn a_down_held_in_the_composer_never_moves_the_card_ctrl_j_enters(cx: &mut TestAppContext) {
    down_held_in_the_composer(cx, "ctrl-j");
}

#[gpui_kit::test]
fn a_down_held_in_the_composer_never_moves_the_card_ctrl_shift_j_enters(cx: &mut TestAppContext) {
    down_held_in_the_composer(cx, "ctrl-shift-j");
}

/// R3-1 from a long card field (a textarea: its `MoveDown` takes ↓ as the
/// composer's does): another card's long field, the keys put there by a
/// click, ↓ held; Ctrl+J enters the card that waits.
#[gpui_kit::test]
fn a_down_held_in_a_long_field_never_moves_the_card_ctrl_j_enters(cx: &mut TestAppContext) {
    let input = branch_input();
    let (w, view, id, _events) = asked(cx, "toolu_lq", "c4lquik", &input);
    let form =
        json!({"question": "Anything for on-call?", "fields": [{"id": "why", "label": "Context", "type": "long", "optional": true}]});
    ask_live(cx, w, &view, "toolu_lf", "c4lform", form);
    let (f, _) = card(cx, &view, "Anything for on-call?");
    arm(cx, &view, f, true);
    click(cx, w, &view, format!("ask-field {f} why"));
    assert_eq!(keys_in(cx, w, &view, "toolu_lf"), "why", "the long field has the keys");
    down(cx, w, "down");
    settle(cx, w, 8);
    assert_eq!(keys_in(cx, w, &view, "toolu_lf"), "why", "its MoveDown took the ↓");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "the card that waits");
    held_down_moves_nothing(cx, w, &view, "a long field, then Ctrl+J", ("c4lquik", &input));
    crate::app::pin_arm_clock(None);
}

/// R3-1 inside one card: a form's long field has the keys (Tab), ↓ held
/// there, then Shift+Tab back to the card: ↓'s repeats move nothing.
#[gpui_kit::test]
fn a_down_held_in_the_entered_forms_long_field_never_moves_its_highlight(cx: &mut TestAppContext) {
    use std::time::Duration;
    let input = json!({"question": "Roll it out?", "wait": true, "options": ["Now", "Later", "Never"], "default": "Now",
        "fields": [{"id": "why", "label": "Why", "type": "long", "optional": true}]});
    let (w, view, id, _events) = asked(cx, "toolu_lt", "c4ltabs", &input);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    assert_eq!(keys_in(cx, w, &view, "toolu_lt"), "body");
    press(cx, w, "tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_lt"), "why");
    down(cx, w, "down");
    settle(cx, w, 8);
    press(cx, w, "shift-tab");
    assert_eq!(keys_in(cx, w, &view, "toolu_lt"), "body");
    assert_eq!(hi(cx, &view), Some(0), "on the default");
    for gap in [0, 90, 1200] {
        crate::app::advance_input_clock(Duration::from_millis(gap));
        lost(cx, w, "down");
        assert_eq!(hi(cx, &view), Some(0), "↓ held from the long field, a repeat with no held bit {gap} ms on, moves nothing");
    }
    up(cx, w, "down");
    crate::app::next_press();
    press(cx, w, "down");
    assert_eq!(hi(cx, &view), Some(1), "let go, 30 ms, pressed: one down");
    assert!(sends(cx, &view).is_empty());
}

/// Shady #86 P3-1: Cmd history, then a key-down that types text and lost
/// its repeat bit (no key-up since: AppKit's callback can send one, mid
/// IME composition or under an IME source), then the guards. Cmd+1 in the
/// composer (its key-up dropped, as AppKit does), Ctrl+J, `1`: the card
/// itself has no input handler, so that `1` said whether it repeats, and
/// the exception takes it for a press: it answers. Once: the next `1`, no
/// key-up between, is a repeat on the batch's next card. In a field after
/// Cmd+2, `2` edits (digits never act there); after Cmd+Enter, an Enter
/// mid-composition sends nothing (the field's token). Composition ended,
/// let go and pressed, Enter sends the form.
#[gpui_kit::test]
fn a_cmd_chord_then_text_without_its_held_bit_acts_only_where_it_may(cx: &mut TestAppContext) {
    let first = json!({"question": "Which branch?", "wait": true, "options": ["main", "release"]});
    let (w, view, q1, _events) = asked(cx, "toolu_c1", "c6cmda1", &first);
    let second = json!({"question": "Which tag?", "wait": true, "options": ["v1", "v2"]});
    ask_live(cx, w, &view, "toolu_c2", "c6cmda2", second.clone());
    let form = json!({"question": "Who reviews?", "fields": [{"id": "who", "label": "Reviewer"}]});
    ask_live(cx, w, &view, "toolu_c3", "c6cmda3", form.clone());
    let (q2, _) = card(cx, &view, "Which tag?");
    at_rest(cx, w, &view);
    let cmd = |cx: &mut TestAppContext, k: &str| {
        let ks = keystroke(cx, k);
        let ev = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent { keystroke: ks, is_held: false, prefer_character_input: false });
        cx.update_window(w, |_, window, cx| {
            window.dispatch_event(ev, cx);
        })
        .expect("the window");
        settle(cx, w, 8);
    };
    cmd(cx, "cmd-1");
    crate::app::next_press();
    press(cx, w, "ctrl-j");
    // The oldest card, above the view now: brought into view, painted.
    play(cx, w, &view);
    assert!(entered(cx, w, q1));
    down(cx, w, "1");
    settle(cx, w, 8);
    assert_eq!(texts(cx, &view), vec![message("c6cmda1", &first, json!({"choice": "main"}))], "after Cmd+1, on the card: a press");
    play(cx, w, &view);
    assert!(entered(cx, w, q2), "the batch's next card");
    down(cx, w, "1");
    settle(cx, w, 8);
    assert_eq!(texts(cx, &view).len(), 1, "the exception is spent: no key-up since, a repeat");
    up(cx, w, "1");
    crate::app::next_press();
    press(cx, w, "2");
    assert_eq!(texts(cx, &view).len(), 2, "let go and pressed: it answers");
    play(cx, w, &view);
    assert_eq!(keys_in(cx, w, &view, "toolu_c3"), "who", "the form's field to fill");
    cmd(cx, "cmd-2");
    down(cx, w, "2");
    settle(cx, w, 8);
    assert_eq!(field_value(cx, &view, "toolu_c3", "who"), "2", "in a field, after Cmd+2: `2` edits");
    assert_eq!(texts(cx, &view).len(), 2, "and acts on nothing");
    let e = view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        v.card_drafts[&(sid, "toolu_c3".to_string())].lines["who"].clone()
    });
    cx.update_window(w, |_, window, cx| e.update(cx, |s, cx| s.replace_and_mark_text_in_range(None, "あ", None, window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    cmd(cx, "cmd-enter");
    down(cx, w, "enter");
    settle(cx, w, 8);
    assert_eq!(texts(cx, &view).len(), 2, "after Cmd+Enter, an Enter mid-composition: the IME's");
    assert_eq!(keys_in(cx, w, &view, "toolu_c3"), "who", "the field keeps the keys");
    cx.update_window(w, |_, window, cx| e.update(cx, |s, cx| s.unmark_text(window, cx))).expect("the window");
    settle(cx, w, 8);
    let who = field_value(cx, &view, "toolu_c3", "who");
    assert!(who.starts_with('2'), "{who:?}");
    up(cx, w, "enter");
    crate::app::next_press();
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view)[2], message("c6cmda3", &form, json!({"fields": {"who": who}})), "one Submit of the form");
    assert_eq!(texts(cx, &view).len(), 3);
}
