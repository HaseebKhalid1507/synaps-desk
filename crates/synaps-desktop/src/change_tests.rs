//! Changing an answer, and Skip (synaps-cards `docs/design.md` §3.3, §3.5,
//! §3.6, §6.1, §7; §9.3's D4 row), in the window: F15 by mouse (2 clicks)
//! and by keys (3: Ctrl+J, Enter, a key), each exactly one `Submit` of
//! `[card <id> changed]` on the card's attachment; F7, Skip, on a card
//! reopened, and Resend and Retry after a skip (its receipt says what they
//! would send); a change while the first answer is on its way (none) or
//! not confirmed (it replaces it); a closed card, or one gone from the
//! history, takes none; a change from the other pane; the later answer
//! wins; Keep, Esc and the answer already given send nothing (a
//! multi-select's picks as a set, Shady #94 P2-2); Tab on a form's
//! receipt opens nothing; a change never resolves a permission prompt
//! (§5.1). A change of an answer taken in stays on its way when that
//! answer's history lands after it is sent, through a gap too, however
//! many changes back, counted (an answer given twice); an answer never
//! taken in, or one whose turn lost it, isn't expected, and another
//! desk's answer of its text wins, as does a copy past the count, or
//! after the change (Shady #94 P2-1, rounds 1 and 2). With part B's
//! memory (#93): a reopened card shows the answer it changes, highlighted
//! and picked, with no `last time` mark; one whose answer has no pick
//! starts on the memory, unmarked; a change that reaches the chat is
//! remembered as a first answer is.
//!
//! What would have gone to the daemon is the view's record
//! (`AppView::card_sends`). Keys go in as new presses (`keys_tests::press`,
//! the input clock past a repeat's 30 ms after each: `next_press`).

use std::time::{Duration, Instant};

use cards_spec::memo::State;
use gpui_kit::{Entity, ListOffset, TestAppContext, px};
use serde_json::{Value, json};

use super::answer_tests::{arm, attachment, card, click, has, heard, land, message, open, play, sends, show};
use super::keys_tests::{Asked, asked, at_rest, down, entered, keys_at, press, prompt, repeat, texts, up};
use super::settle;
use crate::app::AppView;
use crate::deliver::Outgoing;
use crate::pane::PaneId;

const BRANCH: &str = "Which branch should the fix go to?";

fn branch() -> Value {
    json!({"question": BRANCH, "options": ["main", "release", "hotfix"], "default": "main"})
}

fn doc(input: &Value) -> cards_spec::CardDoc {
    match cards_spec::parse_input(input).unwrap() {
        cards_spec::Input::Create { doc, .. } => doc,
        cards_spec::Input::Close(_) => unreachable!(),
    }
}

/// The message changing card `card`'s answer to `values` (§3.3), as the
/// spec builds it.
fn changed(card: &str, input: &Value, values: Value) -> String {
    let d = doc(input);
    let reply = cards_spec::Reply::Change(cards_spec::parse_answer(&d, &values).unwrap());
    cards_spec::message::build(&cards_spec::CardId::parse(card).unwrap(), &d, &reply)
}

fn skipped(card: &str, question: &str) -> String {
    format!("[card {card} skipped] {question} → (skipped)\n{{}}")
}

fn id(card: &str) -> cards_spec::CardId {
    cards_spec::CardId::parse(card).unwrap()
}

/// Nothing waits off screen either: the session list as a daemon's with
/// none that needs you but the one on screen. The demo's has some, and
/// with split panes on (the tests' split-on lane) Ctrl+J opens a session
/// waiting off screen before anything that doesn't wait (split.md §7),
/// such as an answer to change.
fn nothing_waits_elsewhere(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, _| {
        let here: Vec<String> = v.model.sessions.iter().filter(|s| v.model.is_active(&s.id)).map(|s| s.id.clone()).collect();
        v.model.sessions.retain(|s| here.contains(&s.id));
        assert!(v.jump_elsewhere().is_none() && v.waiting_pane().is_none(), "nothing waits off screen");
    });
}

/// A quiet session whose agent asked `input` live as card `card_id` (call
/// `tool`), answered `values` in the history (another desk's, or this
/// one's, its turn over): start state A, the composer with the keys, and
/// nothing waiting anywhere.
fn answered(cx: &mut TestAppContext, tool: &str, card_id: &str, input: &Value, values: Value) -> Asked {
    let (w, view, row, events) = asked(cx, tool, card_id, input);
    nothing_waits_elsewhere(cx, &view);
    land(cx, w, &view, vec![json!({"role": "user", "content": message(card_id, input, values)})]);
    play(cx, w, &view);
    at_rest(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} answered")), "answered, its receipt");
    (w, view, row, events)
}

/// The agent's turn starts on something else: the session is busy (a
/// card's answer is then a `Steer`).
fn busy(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go on"})]);
}

/// The memos' clock 11 s on: an answer with no echo is checked, and
/// settles unconfirmed on the view held.
fn unconfirmed(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.model.tick_cards(Instant::now() + Duration::from_secs(11));
        v.cards_moved(cx);
    });
    settle(cx, w, 8);
}

/// The agent closes card `card_id` (call `tool`'s): its close and result
/// land.
fn closes(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>, card_id: &str) {
    land(
        cx,
        w,
        view,
        vec![
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_close", "name": "cards_ui", "input": {"close": card_id}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_close",
                "content": format!("Card {card_id} closed (cards/1).")}]}),
        ],
    );
    play(cx, w, view);
}

/// Card row `row` is reopened, its answer being changed.
fn changing(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, row: u64) -> bool {
    has(cx, w, format!("ask-state {row} changing"))
}

/// F15, by mouse: 2 clicks. `· change` on the receipt reopens the card,
/// `changing`, the answer given picked, Keep beside Skip; its controls arm
/// afresh, so a click that lands as it reopens sends nothing (§7). An
/// option sends exactly one `Submit` (the session idle) of
/// `[card <id> changed]` on the card's attachment. While it is on its
/// way, nothing offers a change (one answer per card in flight); its echo
/// reads `changed`, and the receipt offers one again.
#[gpui_kit::test]
fn f15_a_change_by_mouse_is_two_clicks(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = answered(cx, "toolu_c1", "c1chang", &input, json!({"choice": "main"}));
    assert!(has(cx, w, format!("ask-change {row}")), "· change, on its receipt");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    assert!(has(cx, w, format!("ask-state {row} changing")), "reopened");
    assert!(has(cx, w, format!("ask-picked {row} 1")), "the answer given, picked");
    assert!(has(cx, w, format!("ask-keep {row}")) && has(cx, w, format!("ask-skip {row}")));
    assert!(sends(cx, &view).is_empty(), "reopening sends nothing");
    click(cx, w, &view, format!("ask-option {row} 2"));
    assert!(sends(cx, &view).is_empty(), "an option drawn as it reopened, clicked at once: nothing");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 2"));
    let (seq, _) = attachment(cx, &view);
    let text = changed("c1chang", &input, json!({"choice": "release"}));
    assert_eq!(text, format!("[card c1chang changed] {BRANCH} → release\n{{\"choice\":\"release\"}}"));
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing { seq, busy: false, card: id("c1chang"), text: text.clone() }],
        "one Submit, `changed`, on the card's attachment"
    );
    assert!(has(cx, w, format!("ask-state {row} sending…")), "its receipt, on its way");
    assert!(!has(cx, w, format!("ask-change {row}")), "on its way: no change offered");
    assert!(!changing(cx, w, row));
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": text})]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "the echo: changed");
    assert!(has(cx, w, format!("ask-change {row}")), "in the chat: it may change again");
    assert_eq!(sends(cx, &view).len(), 1);
    crate::app::pin_arm_clock(None);
}

/// F15, by keys: 3. Nothing waits, on screen or off: Ctrl+J enters the
/// answer this desk may change, its receipt (its ring, its keys' line);
/// Enter reopens it, entered, the highlight on the answer given (not the
/// agent's default); `1` sends exactly one `Submit` of the change, and the
/// keys go back to the composer. Held, the key acts once.
#[gpui_kit::test]
fn f15_a_change_by_keys_is_three(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = answered(cx, "toolu_c2", "c2chang", &input, json!({"choice": "release"}));
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row), "Ctrl+J, nothing waiting: the answer to change");
    assert!(has(cx, w, format!("ask-receipt {row}")) && has(cx, w, format!("ask-footer {row}")), "its receipt, its keys' line");
    assert!(!has(cx, w, format!("ask-option {row} 1")), "not reopened yet");
    press(cx, w, "enter");
    assert!(has(cx, w, format!("ask-state {row} changing")), "Enter reopens it");
    assert!(entered(cx, w, row), "entered, reopened");
    assert!(has(cx, w, format!("ask-hi {row} 2")), "the highlight on the answer given, not the default");
    assert!(sends(cx, &view).is_empty());
    down(cx, w, "1");
    repeat(cx, w, "1");
    up(cx, w, "1");
    settle(cx, w, 8);
    crate::app::next_press();
    let (seq, _) = attachment(cx, &view);
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing { seq, busy: false, card: id("c2chang"), text: changed("c2chang", &input, json!({"choice": "main"})) }],
        "Ctrl+J, Enter, 1: one Submit of the change (held, once)"
    );
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer has the keys again");
}

/// F7 on a card reopened: 0 changes the answer to a skip,
/// `[card <id> skipped]` (a skip is a skip). Not confirmed, its receipt
/// says `→ (skipped)`, what Resend would send; Resend sends it again.
/// By mouse, Skip on a reopened card is one click too.
#[gpui_kit::test]
fn f7_skip_on_a_reopened_card_and_resend_after_it(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = answered(cx, "toolu_c3", "c3skipc", &input, json!({"choice": "main"}));
    press(cx, w, "ctrl-j");
    press(cx, w, "enter");
    press(cx, w, "0");
    assert_eq!(texts(cx, &view), vec![skipped("c3skipc", BRANCH)], "0: one skip");
    assert!(has(cx, w, format!("ask-said {row}")), "on its way, it says it is a skip");
    unconfirmed(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} not confirmed")));
    assert!(has(cx, w, format!("ask-said {row}")), "not confirmed: `→ (skipped)`, what Resend sends");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-resend {row}"));
    assert_eq!(texts(cx, &view), vec![skipped("c3skipc", BRANCH); 2], "Resend: the same skip");
    // By mouse: another card, reopened, Skip.
    let other = json!({"question": "Squash the commits?", "options": ["Yes", "No"]});
    super::answer_tests::ask_live(cx, w, &view, "toolu_c3b", "c3skipd", other.clone());
    land(cx, w, &view, vec![json!({"role": "user", "content": message("c3skipd", &other, json!({"choice": "Yes"}))})]);
    play(cx, w, &view);
    let (row2, ix2) = card(cx, &view, "Squash the commits?");
    show(cx, w, &view, ix2);
    arm(cx, &view, row2, true);
    click(cx, w, &view, format!("ask-change {row2}"));
    arm(cx, &view, row2, true);
    click(cx, w, &view, format!("ask-skip {row2}"));
    assert_eq!(texts(cx, &view).last(), Some(&skipped("c3skipd", "Squash the commits?")), "Skip, reopened: one click");
    assert_eq!(sends(cx, &view).len(), 3);
    crate::app::pin_arm_clock(None);
}

/// F7, Retry after a skip: no connection, the skip is not sent; its
/// receipt says `→ (skipped)` and offers Retry, which sends the same skip.
#[gpui_kit::test]
fn a_skip_not_sent_says_so_and_retry_sends_it(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = asked(cx, "toolu_c4", "c4skipr", &input);
    let (_, ix) = card(cx, &view, BRANCH);
    show(cx, w, &view, ix);
    arm(cx, &view, row, true);
    let client = view.update(cx, |v, _| {
        v.synthetic = false;
        v.client.take()
    });
    click(cx, w, &view, format!("ask-skip {row}"));
    assert!(has(cx, w, format!("ask-state {row} not sent")));
    assert!(has(cx, w, format!("ask-said {row}")), "not sent: `→ (skipped)`, what Retry sends");
    view.update(cx, |v, _| v.client = client);
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-retry {row}"));
    assert_eq!(texts(cx, &view), vec![skipped("c4skipr", BRANCH); 2], "Retry: the same skip");
    crate::app::pin_arm_clock(None);
}

/// A skip steered into a busy session, taken in (`SteeringDelivered`)
/// before its history: `skipped`, never `answered`. A change taken in
/// the same way reads `changed` before its history comes, not the old
/// answer's `answered`; once the history has it, the fold's receipt.
#[gpui_kit::test]
fn a_skip_or_a_change_taken_in_reads_as_it_is(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = asked(cx, "toolu_c5", "c5taken", &input);
    busy(cx, w, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "0");
    let skip = skipped("c5taken", BRANCH);
    assert_eq!(sends(cx, &view).last().map(|o| (o.busy, o.text.clone())), Some((true, skip.clone())), "busy: a Steer");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": skip, "delivered": true})]);
    heard(cx, w, &view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": skip}})]);
    assert!(has(cx, w, format!("ask-state {row} skipped")), "taken in: skipped");
    assert!(!has(cx, w, format!("ask-state {row} answered")));
    // Another card, answered: its change, steered in.
    let other = json!({"question": "Tag the release?", "options": ["v2.4.0", "v2.4.0-rc1"]});
    let msg = message("c5takeo", &other, json!({"choice": "v2.4.0"}));
    super::answer_tests::ask_live(cx, w, &view, "toolu_c5b", "c5takeo", other.clone());
    land(cx, w, &view, vec![json!({"role": "user", "content": msg})]);
    play(cx, w, &view);
    at_rest(cx, w, &view);
    busy(cx, w, &view);
    let (row2, ix2) = card(cx, &view, "Tag the release?");
    show(cx, w, &view, ix2);
    arm(cx, &view, row2, true);
    click(cx, w, &view, format!("ask-change {row2}"));
    arm(cx, &view, row2, true);
    click(cx, w, &view, format!("ask-option {row2} 2"));
    let change = changed("c5takeo", &other, json!({"choice": "v2.4.0-rc1"}));
    assert_eq!(sends(cx, &view).last().map(|o| (o.busy, o.text.clone())), Some((true, change.clone())));
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": change, "delivered": true})]);
    heard(cx, w, &view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": change}})]);
    assert!(has(cx, w, format!("ask-state {row2} changed")), "taken in, its history to come: changed, not the old answer");
    land(cx, w, &view, vec![json!({"role": "user", "content": change})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row2} changed")), "its history: the fold's receipt says it too");
    view.read_with(cx, |v, _| {
        let c = v.model.lives[v.panes.cur.live].transcript.cards().card("toolu_c5b").cloned().unwrap();
        assert_eq!(c.answers.len(), 2);
        assert_eq!(c.answers[1].text, change, "the change, latest");
    });
    crate::app::pin_arm_clock(None);
}

/// One answer per card in flight (§3.5): the first answer on its way,
/// nothing offers a change, and Ctrl+J has none to enter. Not confirmed
/// (no echo in 10 s), its receipt offers Resend and a change: the card
/// reopens with that answer in it, and the change replaces it (one more
/// `Submit`, `changed`; Resend gone).
#[gpui_kit::test]
fn a_change_waits_for_an_answer_on_its_way_and_replaces_one_unconfirmed(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = asked(cx, "toolu_c6", "c6inflt", &input);
    nothing_waits_elsewhere(cx, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    assert_eq!(texts(cx, &view), vec![message("c6inflt", &input, json!({"choice": "main"}))]);
    assert!(has(cx, w, format!("ask-state {row} sending…")));
    assert!(!has(cx, w, format!("ask-change {row}")), "on its way: no change");
    press(cx, w, "ctrl-j");
    assert!(!entered(cx, w, row), "Ctrl+J: nothing to change while it is on its way");
    assert_eq!(keys_at(cx, w, &view), (true, false));
    unconfirmed(cx, w, &view);
    assert!(has(cx, w, format!("ask-resend {row}")), "not confirmed: Resend");
    assert!(has(cx, w, format!("ask-change {row}")), "and a change");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    assert!(has(cx, w, format!("ask-state {row} changing")));
    assert!(has(cx, w, format!("ask-picked {row} 1")), "the answer not confirmed, in it");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 3"));
    let (seq, _) = attachment(cx, &view);
    let s = sends(cx, &view);
    assert_eq!(s.len(), 2);
    assert_eq!(s[1], Outgoing { seq, busy: false, card: id("c6inflt"), text: changed("c6inflt", &input, json!({"choice": "hotfix"})) });
    assert!(has(cx, w, format!("ask-state {row} sending…")));
    assert!(!has(cx, w, format!("ask-resend {row}")), "the change replaced it: no Resend of the first");
    crate::app::pin_arm_clock(None);
}

/// §3.6: a change is allowed while the card is in the history and not
/// closed. Reopened, then closed by the agent: the closed receipt, no
/// option left, no change offered; Ctrl+J enters none; and the press
/// itself is refused (the binding checked on the press, not as painted).
#[gpui_kit::test]
fn a_closed_card_takes_no_change(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = answered(cx, "toolu_c7", "c7closd", &input, json!({"choice": "main"}));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    assert!(has(cx, w, format!("ask-state {row} changing")));
    closes(cx, w, &view, "c7closd");
    assert!(has(cx, w, format!("ask-state {row} closed")), "closed: its receipt");
    assert!(!has(cx, w, format!("ask-option {row} 2")), "nothing left to click");
    assert!(!has(cx, w, format!("ask-change {row}")), "no change offered");
    at_rest(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert!(!entered(cx, w, row), "Ctrl+J: a closed card's answer can't change");
    let pressed = view.update(cx, |v, cx| {
        let s = v.card_session().unwrap();
        let items = v.model.lives[v.panes.cur.live].transcript.items();
        let ask = items.iter().find_map(|i| match i.card.as_deref() {
            Some(synaps_client::cards::CardRow::Ask(a)) if a.tool_id == "toolu_c7" => Some(a.clone()),
            _ => None,
        });
        let painted = crate::deliver::Painted::of(&ask.unwrap()).unwrap();
        let reply = cards_spec::Reply::Answer(cards_spec::parse_answer(&painted.doc, &json!({"choice": "release"})).unwrap());
        v.ask_send(&s, row, &painted, reply, cx)
    });
    assert!(!pressed, "closed: a press sends nothing");
    assert!(sends(cx, &view).is_empty());
}

/// §3.6: once the card has left the history (a compaction, `/new`), no
/// change: its row is gone, and Ctrl+J enters nothing.
#[gpui_kit::test]
fn a_card_gone_from_the_history_takes_no_change(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = answered(cx, "toolu_c8", "c8goneh", &input, json!({"choice": "main"}));
    assert!(has(cx, w, format!("ask-change {row}")));
    let (seq, sid) = attachment(cx, &view);
    view.update(cx, |v, cx| {
        let update = synaps_client::HistoryUpdate::Replace {
            messages: vec![json!({"role": "user", "content": "<context-summary>…</context-summary>"})],
        };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx);
    });
    play(cx, w, &view);
    assert!(!has(cx, w, format!("ask-change {row}")) && !has(cx, w, format!("ask-receipt {row}")), "gone");
    at_rest(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_at(cx, w, &view), (true, false), "Ctrl+J: nothing to enter");
    assert!(sends(cx, &view).is_empty());
}

/// Keep, Esc and the answer already given send nothing. Keep closes the
/// reopened card as it was; picking the answer it shows closes it too
/// (no needless turn); Esc on a card reopened from the keys keeps the
/// answer, and the keys go back to the composer.
#[gpui_kit::test]
fn keep_esc_and_the_same_answer_send_nothing(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = answered(cx, "toolu_c9", "c9keepx", &input, json!({"choice": "main"}));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-keep {row}"));
    assert!(has(cx, w, format!("ask-state {row} answered")) && !changing(cx, w, row), "Keep: as it was");
    assert!(!has(cx, w, format!("ask-whole {row}")), "the change click was its own: the receipt under it didn't open");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 1"));
    assert!(has(cx, w, format!("ask-state {row} answered")) && !changing(cx, w, row), "the same answer: as it was");
    at_rest(cx, w, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "enter");
    assert!(changing(cx, w, row));
    press(cx, w, "escape");
    assert!(!changing(cx, w, row), "Esc keeps it");
    assert!(has(cx, w, format!("ask-state {row} answered")));
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer has the keys");
    press(cx, w, "ctrl-j");
    press(cx, w, "enter");
    press(cx, w, "enter");
    assert!(!changing(cx, w, row), "Enter on the answer given: as it was");
    assert!(sends(cx, &view).is_empty(), "nothing sent, all along");
    crate::app::pin_arm_clock(None);
}

/// A form's answer reopened: its fields as the answer gave them (a
/// field's text, a check), its option as picked. A field changed, Send:
/// one `Submit` of the change, every field.
#[gpui_kit::test]
fn a_forms_answer_reopens_with_its_values(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let (row, ix) = card(cx, &view, "When can it cut over?");
    show(cx, w, &view, ix);
    assert!(has(cx, w, format!("ask-state {row} changed")), "the demo changed it once already");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    let values = view.read_with(cx, |v, cx| {
        let s = v.card_session().unwrap();
        let d = &v.card_drafts[&(s, "toolu_a2".to_string())];
        let items = v.model.transcript.items();
        let doc = items
            .iter()
            .find_map(|i| match i.card.as_deref() {
                Some(synaps_client::cards::CardRow::Ask(a)) if a.tool_id == "toolu_a2" => Some(a.doc.clone()),
                _ => None,
            })
            .unwrap();
        ["day", "backfill", "oncall"].map(|f| d.value(&doc, f, cx))
    });
    use cards_spec::FieldValue::{Check, Text};
    assert_eq!(values, [Some(Text("Monday".into())), Some(Check(true)), Some(Text("Watch p99 on /refresh".into()))], "as answered last");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-field {row} backfill"));
    click(cx, w, &view, format!("ask-send {row}"));
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1);
    assert!(s[0].text.starts_with("[card c81kq0z changed] When can it cut over? → "), "{}", s[0].text);
    assert!(
        s[0].text.ends_with("\n{\"fields\":{\"day\":\"Monday\",\"backfill\":false,\"oncall\":\"Watch p99 on /refresh\"}}"),
        "{}",
        s[0].text
    );
    crate::app::pin_arm_clock(None);
}

/// A change from the other pane: the card's pane not focused, `· change`
/// and an option there, two clicks, one `Submit` of the change on that
/// pane's attachment (not the focused pane's, another session's). The
/// later answer wins (§3.6): another desk's change after ours lands, the
/// fold's latest is theirs, and this desk's entry is dropped (no Resend
/// of ours over it).
#[gpui_kit::test]
fn a_change_from_the_other_pane_and_the_later_answer_wins(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    let question = "Where should the tokens live?";
    let (row, ix) = card(cx, &view, question);
    let (seq, sid) = attachment(cx, &view);
    let first: PaneId = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, _| v.app_settings.split = true);
    super::answer_tests::key(cx, w, "ctrl-\\");
    play(cx, w, &view);
    // The other session opens in the new pane: its attach gate, a switch
    // just made, is played out (#91: the test's clock moves past it).
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
        assert_ne!(v.panes.focused, first, "the other pane is focused");
        v.model.lives[v.panes.cur.live].attachment()
    });
    assert!(other.is_some_and(|o| o != seq), "the focused pane holds another attachment");
    view.update(cx, |v, cx| {
        let _ = v.in_pane(first, |v| v.panes.cur.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) }));
        cx.notify();
    });
    settle(cx, w, 8);
    crate::app::pin_arm_clock(Some(Instant::now() + Duration::from_secs(3600)));
    click(cx, w, &view, format!("ask-change {row}"));
    assert!(has(cx, w, format!("ask-state {row} changing")), "reopened in its pane");
    crate::app::pin_arm_clock(Some(Instant::now() + Duration::from_secs(7200)));
    click(cx, w, &view, format!("ask-option {row} 2"));
    let input = json!({"question": question, "detail": "The file store loses writes when two refreshes race.",
        "options": [{"label": "Postgres", "hint": "already runs the sessions table"}, {"label": "Redis", "hint": "fast, but one more service"},
            "Keep the file store"],
        "default": "Postgres"});
    let ours = changed("c4f9x2a", &input, json!({"choice": "Redis"}));
    assert_eq!(
        sends(cx, &view),
        vec![Outgoing { seq, busy: false, card: id("c4f9x2a"), text: ours.clone() }],
        "on the card's pane's attachment"
    );
    // Ours in the history, then another desk's change after it.
    assert_eq!(attachment(cx, &view).0, seq, "the card's pane, focused by its click");
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": ours})]);
    let theirs = changed("c4f9x2a", &input, json!({"choice": "Keep the file store"}));
    land(cx, w, &view, vec![json!({"role": "user", "content": ours}), json!({"role": "user", "content": theirs})]);
    play(cx, w, &view);
    view.read_with(cx, |v, _| {
        let c = v.model.lives[v.panes.cur.live].transcript.cards().card("toolu_a1").cloned().unwrap();
        assert_eq!(c.answers.last().map(|a| a.text.as_str()), Some(theirs.as_str()), "the later answer wins");
        assert_eq!(v.model.memos.get_state(&sid, "c4f9x2a"), None, "superseded: this desk's entry dropped");
    });
    assert!(!has(cx, w, format!("ask-resend {row}")));
    assert_eq!(sends(cx, &view).len(), 1);
    crate::app::pin_arm_clock(None);
}

/// §5.1: a change never resolves a permission prompt. With a confirm
/// prompt pending, Ctrl+J goes to the prompt (prompts first), never to
/// an answer to change. A card answered `yes`, changed to `always` by
/// mouse: one `Submit` of the change; the prompt still pending, and no
/// answer to it went. A receipt entered before a prompt comes: its Enter
/// reopens the card, a key changes it, and the prompt is untouched.
#[gpui_kit::test]
fn a_change_never_resolves_a_prompt(cx: &mut TestAppContext) {
    let input = json!({"question": "Allow it?", "options": ["yes", "always"]});
    let (w, view, row, _events) = answered(cx, "toolu_cp", "c8prmpt", &input, json!({"choice": "yes"}));
    // A receipt entered, then a prompt.
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row));
    prompt(cx, w, &view);
    press(cx, w, "enter");
    assert!(has(cx, w, format!("ask-state {row} changing")), "Enter: the card reopened, not the prompt allowed");
    press(cx, w, "2");
    let pending = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let l = &v.model.lives[v.panes.cur.live];
            (l.transcript.pending_prompts().iter().any(|p| p.id == 7), l.prompt_answered(7), v.sent.is_none())
        })
    };
    assert_eq!(texts(cx, &view), vec![changed("c8prmpt", &input, json!({"choice": "always"}))]);
    assert_eq!(pending(cx), (true, false, true), "the prompt still pending, unanswered");
    // The change in the chat: it may change again, by mouse.
    let text = texts(cx, &view)[0].clone();
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": text})]);
    land(cx, w, &view, vec![json!({"role": "user", "content": text})]);
    heard(cx, w, &view, vec![json!({"ev": "idle"})]);
    play(cx, w, &view);
    at_rest(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert!(!entered(cx, w, row), "Ctrl+J: the prompt first, never an answer to change");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 1"));
    assert_eq!(texts(cx, &view).last(), Some(&changed("c8prmpt", &input, json!({"choice": "yes"}))));
    assert_eq!(sends(cx, &view).len(), 2);
    assert_eq!(pending(cx), (true, false, true), "still pending, unanswered");
    crate::app::pin_arm_clock(None);
}

/// A form's receipt entered from the keys: Tab opens none of its fields
/// (a receipt draws none); Enter reopens it, its field as answered, and
/// Esc keeps the answer.
#[gpui_kit::test]
fn tab_on_a_forms_receipt_opens_nothing(cx: &mut TestAppContext) {
    let input = json!({"question": "Who reviews the migration?", "fields": [{"id": "who", "label": "Reviewer"}]});
    let (w, view, row, _events) = answered(cx, "toolu_ct", "c9tabrc", &input, json!({"fields": {"who": "Ana"}}));
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row));
    press(cx, w, "tab");
    assert!(entered(cx, w, row) && !changing(cx, w, row), "Tab on a receipt: nothing");
    assert!(view.read_with(cx, |v, _| v.card_drafts.is_empty()), "no field opened under a receipt");
    press(cx, w, "enter");
    assert!(changing(cx, w, row));
    let who = view.read_with(cx, |v, cx| {
        let s = v.card_session().unwrap();
        v.card_drafts[&(s, "toolu_ct".to_string())].value(&doc(&input), "who", cx)
    });
    assert_eq!(who, Some(cards_spec::FieldValue::Text("Ana".into())), "its field as answered");
    press(cx, w, "escape");
    assert!(!changing(cx, w, row) && sends(cx, &view).is_empty(), "Esc keeps it");
}

/// The session's history as the view holds it, a quiet session that
/// asked `input` as card `card_id` (call `tool`, [`asked`]), with `more`
/// sent after it: as the history asked for again gives it. The view may
/// hold some of `more` already.
fn history_with(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str, card_id: &str, input: &Value, more: &[&str]) -> Vec<Value> {
    let (seq, _) = attachment(cx, view);
    let mut h = match crate::demo::attach(crate::demo::IDLE_CPU, seq).pop() {
        Some(synaps_client::Event::Attached { attached, .. }) => attached.conversation.api_messages.clone(),
        other => panic!("{other:?}"),
    };
    h.extend(super::answer_tests::asked_messages(tool, card_id, input, None));
    let held = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.history_len());
    assert!((h.len()..=h.len() + more.len()).contains(&held), "the history held: {held}, from {}", h.len());
    h.extend(more.iter().map(|t| json!({"role": "user", "content": t})));
    h
}

/// The session's digest of `messages`, the engine's slot holding `queued`.
fn slot_digest(messages: &[Value], queued: Option<&str>) -> Value {
    json!({"ev": "conversation", "digest": {"messages_len": messages.len(),
        "messages_hash": synaps_client::history::messages_hash(messages), "queued_message": queued}})
}

/// Shady #94 P2-1's start: a busy session's card, answered `main` by the
/// keys (a `Steer`, A), which the engine took in (`SteeringDelivered`),
/// the slot cleared on the old history: its checkpoint, the first history
/// with A, still to come. The receipt offers a change; by mouse it goes,
/// `hotfix` (B), a `Steer`, on its way. Returns A and B.
fn taken_in_then_changed(cx: &mut TestAppContext, tool: &str, card_id: &str) -> (Asked, String, String) {
    let input = branch();
    let (w, view, row, events) = asked(cx, tool, card_id, &input);
    nothing_waits_elsewhere(cx, &view);
    busy(cx, w, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    let a = message(card_id, &input, json!({"choice": "main"}));
    assert_eq!(sends(cx, &view).last().map(|o| (o.busy, o.text.clone())), Some((true, a.clone())), "A: a Steer");
    let old = history_with(cx, &view, tool, card_id, &input, &[]);
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "steered", "text": a, "delivered": true}),
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": a}}),
            slot_digest(&old, None),
        ],
    );
    assert!(has(cx, w, format!("ask-state {row} answered")), "A taken in, its history to come");
    assert!(has(cx, w, format!("ask-change {row}")), "a change offered");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 3"));
    let b = changed(card_id, &input, json!({"choice": "hotfix"}));
    assert_eq!(texts(cx, &view), vec![a.clone(), b.clone()]);
    assert!(has(cx, w, format!("ask-state {row} sending…")), "B on its way");
    ((w, view, row, events), a, b)
}

/// B, on its way, is still this desk's: no change offered, nor Resend,
/// and its entry (one for the session, whatever pane shows it) in flight.
fn still_out(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>, row: u64, card_id: &str, what: &str) {
    out_of(cx, w, view, row, card_id, 2, what);
}

/// [`still_out`], `n` answers sent in all.
fn out_of(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>, row: u64, card_id: &str, n: usize, what: &str) {
    assert!(!has(cx, w, format!("ask-change {row}")) && !has(cx, w, format!("ask-change {row} off")), "{what}: no change offered");
    assert!(!has(cx, w, format!("ask-resend {row}")) && !has(cx, w, format!("ask-resend {row} off")), "{what}: no Resend");
    view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        let st = v.model.memos.get_state(&sid, card_id);
        assert!(st.is_some_and(|s| !matches!(s, State::InChat | State::Unconfirmed | State::NotSent)), "{what}: B in flight ({st:?})");
    });
    assert_eq!(sends(cx, view).len(), n, "{what}: nothing more sent");
}

/// Shady #94 P2-1: an answer taken in (A), its history still to come, is
/// changed (B). The history with A lands before anything of B's: A is
/// this desk's, and older than B, not an answer it didn't know of. B is
/// still on its way, in both panes on the session (they share its memo):
/// no change offered, no Resend. B's echo, its delivery and its history
/// settle it: `changed`, the fold's latest, two `Steer`s in all.
#[gpui_kit::test]
fn a_change_keeps_its_way_when_the_answer_it_changes_lands(cx: &mut TestAppContext) {
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, "toolu_d1", "cd1takn");
    view.update(cx, |v, _| v.app_settings.split = true);
    super::answer_tests::key(cx, w, "ctrl-\\");
    play(cx, w, &view);
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 2, "two panes");
        assert!(v.panes.iter().all(|p| p.live == v.panes.cur.live), "on the one session");
    });
    land(cx, w, &view, vec![json!({"role": "user", "content": a})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} sending…")), "A's history: B still on its way");
    still_out(cx, w, &view, row, "cd1takn", "A's history");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": b, "delivered": true})]);
    still_out(cx, w, &view, row, "cd1takn", "B's echo");
    heard(cx, w, &view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": b}})]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "B taken in");
    land(cx, w, &view, vec![json!({"role": "user", "content": b})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} changed")), "B's history");
    view.read_with(cx, |v, _| {
        let c = v.model.lives[v.panes.cur.live].transcript.cards().card("toolu_d1").cloned().unwrap();
        let got: Vec<&str> = c.answers.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(got, [a.as_str(), b.as_str()], "A, then B: the latest");
    });
    assert_eq!(sends(cx, &view).len(), 2, "two Steers in all");
    crate::app::pin_arm_clock(None);
}

/// P2-1 through a gap (Shady #81 rounds 4–7): B on its way, the daemon
/// drops events for this desk, and the history asked for again lands
/// with A, nothing since saying the engine's slot. B is still this
/// desk's: no change offered, no Resend (an unknown slot never offers
/// one). B's echo after the gap, a digest saying the slot holds it, its
/// delivery and its history settle it: `changed`, nothing sent again.
#[gpui_kit::test]
fn a_change_keeps_its_way_through_a_gap_when_the_answer_it_changes_lands(cx: &mut TestAppContext) {
    let input = branch();
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, "toolu_d2", "cd2takn");
    heard(cx, w, &view, vec![json!({"ev": "system_notice", "text": "event stream lagged; 6 dropped"})]);
    let with_a = history_with(cx, &view, "toolu_d2", "cd2takn", &input, &[&a]);
    let (seq, sid) = attachment(cx, &view);
    let update = synaps_client::HistoryUpdate::Replace { messages: with_a.clone() };
    view.update(cx, |v, cx| v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx));
    play(cx, w, &view);
    view.read_with(cx, |v, _| {
        let t = &v.model.lives[v.panes.cur.live].transcript;
        assert!(!t.resyncing() && !t.slot_known(), "the history asked for again; the slot unknown");
        assert_eq!(t.cards().card("toolu_d2").map(|c| c.answers.len()), Some(1), "A in it");
    });
    still_out(cx, w, &view, row, "cd2takn", "the gap's history");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": b, "delivered": true})]);
    still_out(cx, w, &view, row, "cd2takn", "B's echo, the slot unknown");
    heard(cx, w, &view, vec![slot_digest(&with_a, Some(&b))]);
    still_out(cx, w, &view, row, "cd2takn", "the slot holds B");
    heard(cx, w, &view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": b}})]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "B taken in");
    land(cx, w, &view, vec![json!({"role": "user", "content": b})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} changed")), "B's history");
    assert_eq!(sends(cx, &view).len(), 2, "nothing sent again");
    crate::app::pin_arm_clock(None);
}

/// The history after what the view holds, `more`, lands, and a digest
/// says it is the session's, the engine's slot empty: a settled view.
fn lands_settled(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>, tool: &str, card_id: &str, more: &[&str]) {
    let held = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.cards().card(tool).map_or(0, |c| c.answers.len()));
    land(cx, w, view, more.iter().map(|t| json!({"role": "user", "content": t})).collect());
    let texts: Vec<String> = view.read_with(cx, |v, _| {
        v.model.lives[v.panes.cur.live].transcript.cards().card(tool).unwrap().answers.iter().map(|a| a.text.clone()).collect()
    });
    assert_eq!(texts.len(), held + more.len(), "the card's answers");
    let all: Vec<&str> = texts.iter().map(String::as_str).collect();
    let h = history_with(cx, view, tool, card_id, &branch(), &all);
    heard(cx, w, view, vec![slot_digest(&h, None)]);
    play(cx, w, view);
    view.read_with(cx, |v, _| assert!(v.model.lives[v.panes.cur.live].transcript.history_settled(), "settled"));
}

/// The card shows the history's latest answer, the fold's receipt
/// (`answered` for its first, `changed` past it), with nothing of this
/// desk's out: no entry, no Resend, a change offered, and `n` answers
/// sent in all.
fn shows_theirs(
    cx: &mut TestAppContext,
    w: gpui_kit::AnyWindowHandle,
    view: &Entity<AppView>,
    row: u64,
    card_id: &str,
    n: usize,
    what: &str,
) {
    let word = view.read_with(cx, |v, _| {
        let t = &v.model.lives[v.panes.cur.live].transcript;
        let c = t.cards().bound(&id(card_id)).unwrap();
        if c.answers.len() > 1 { "changed" } else { "answered" }
    });
    view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        assert_eq!(v.model.memos.get_state(&sid, card_id), None, "{what}: our entry dropped");
    });
    assert!(has(cx, w, format!("ask-state {row} {word}")), "{what}: the card shows the latest, theirs ({word})");
    assert!(!has(cx, w, format!("ask-resend {row}")) && !has(cx, w, format!("ask-resend {row} off")), "{what}: no Resend over it");
    assert!(has(cx, w, format!("ask-change {row}")), "{what}: a change offered");
    assert_eq!(sends(cx, view).len(), n, "{what}: nothing more sent");
}

/// Shady #94 round 2, P2-1 (A): an answer never said to be taken in is
/// not expected. A (no echo in 10 s) is not confirmed, and changed (B).
/// Run `lost` (B lost too, or not yet), then another desk sends an
/// answer byte for byte A's, the history settles with it, the slot
/// empty, and the clock checks B: that answer is the later one, and
/// wins. No Resend of B over it.
fn not_taken_in(
    cx: &mut TestAppContext,
    tool: &str,
    card_id: &str,
    lost: impl FnOnce(&mut TestAppContext, gpui_kit::AnyWindowHandle, &Entity<AppView>, u64),
) {
    let input = branch();
    let (w, view, row, _events) = asked(cx, tool, card_id, &input);
    nothing_waits_elsewhere(cx, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    let a = message(card_id, &input, json!({"choice": "main"}));
    unconfirmed(cx, w, &view);
    assert!(has(cx, w, format!("ask-resend {row}")) && has(cx, w, format!("ask-change {row}")), "A not confirmed");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 3"));
    let b = changed(card_id, &input, json!({"choice": "hotfix"}));
    assert_eq!(texts(cx, &view), vec![a.clone(), b.clone()]);
    lost(cx, w, &view, row);
    lands_settled(cx, w, &view, tool, card_id, &[&a]);
    unconfirmed(cx, w, &view);
    shows_theirs(cx, w, &view, row, card_id, 2, "their A");
    crate::app::pin_arm_clock(None);
}

#[gpui_kit::test]
fn an_answer_not_taken_in_is_not_expected_and_a_copy_of_it_wins(cx: &mut TestAppContext) {
    not_taken_in(cx, "toolu_d3", "cd3uncf", |cx, w, _view, row| {
        assert!(has(cx, w, format!("ask-state {row} sending…")), "B on its way");
    });
}

#[gpui_kit::test]
fn an_answer_not_taken_in_is_not_expected_once_its_change_is_lost_too(cx: &mut TestAppContext) {
    not_taken_in(cx, "toolu_d5", "cd5lost", |cx, w, view, row| {
        unconfirmed(cx, w, view);
        assert!(has(cx, w, format!("ask-resend {row}")), "B not confirmed, nothing over it: Resend");
    });
}

/// B, taken in in its turn too, the history still before A: the slot
/// cleared on the old history. The receipt reads `changed`, and offers
/// a change.
fn taken_in(
    cx: &mut TestAppContext,
    w: gpui_kit::AnyWindowHandle,
    view: &Entity<AppView>,
    row: u64,
    tool: &str,
    card_id: &str,
    text: &str,
) {
    let old = history_with(cx, view, tool, card_id, &branch(), &[]);
    heard(
        cx,
        w,
        view,
        vec![
            json!({"ev": "steered", "text": text, "delivered": true}),
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": text}}),
            slot_digest(&old, None),
        ],
    );
    assert!(has(cx, w, format!("ask-state {row} changed")), "taken in, its history to come");
    assert!(has(cx, w, format!("ask-change {row}")), "a change offered");
}

/// A change by mouse to option `option` (from 1), its `values`: sent,
/// on its way. Returns its text.
fn change_to(
    cx: &mut TestAppContext,
    w: gpui_kit::AnyWindowHandle,
    view: &Entity<AppView>,
    row: u64,
    card_id: &str,
    option: usize,
    values: Value,
) -> String {
    arm(cx, view, row, true);
    click(cx, w, view, format!("ask-change {row}"));
    arm(cx, view, row, true);
    click(cx, w, view, format!("ask-option {row} {option}"));
    let text = changed(card_id, &branch(), values);
    assert_eq!(texts(cx, view).last(), Some(&text), "sent");
    assert!(has(cx, w, format!("ask-state {row} sending…")), "on its way");
    text
}

/// Its echo and its delivery: in the chat, `changed`.
fn echoed_and_delivered(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>, row: u64, text: &str) {
    heard(cx, w, view, vec![json!({"ev": "steered", "text": text, "delivered": true})]);
    heard(cx, w, view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": text}})]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "taken in");
}

/// The card's answers in the history, in order.
fn answers(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str) -> Vec<String> {
    view.read_with(cx, |v, _| {
        v.model.lives[v.panes.cur.live].transcript.cards().card(tool).unwrap().answers.iter().map(|a| a.text.clone()).collect()
    })
}

/// Shady #94 round 2, P2-1 (B): a third answer keeps the chain. A is
/// taken in, changed (B), and B is taken in too, the history still before
/// A throughout; B is changed (C). C expects both: the history with A,
/// then with B, lands before anything of C's, settled, and C is still on
/// its way (no fourth change, no Resend). C's echo, delivery and history
/// settle it: `changed`, A, B, C in the history, three answers sent.
#[gpui_kit::test]
fn a_third_answer_expects_both_before_it(cx: &mut TestAppContext) {
    let (tool, id) = ("toolu_d6", "cd6chan");
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, tool, id);
    taken_in(cx, w, &view, row, tool, id, &b);
    let c = change_to(cx, w, &view, row, id, 2, json!({"choice": "release"}));
    lands_settled(cx, w, &view, tool, id, &[&a]);
    out_of(cx, w, &view, row, id, 3, "[A]");
    lands_settled(cx, w, &view, tool, id, &[&b]);
    out_of(cx, w, &view, row, id, 3, "[A, B]");
    echoed_and_delivered(cx, w, &view, row, &c);
    lands_settled(cx, w, &view, tool, id, &[&c]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "[A, B, C]: C, the latest");
    assert_eq!(answers(cx, &view, tool), [a, b, c], "A, B, C");
    assert_eq!(sends(cx, &view).len(), 3, "three answers sent");
    crate::app::pin_arm_clock(None);
}

/// The chain counts, not just matches: A, B (`hotfix`), C (`release`),
/// then D, `hotfix` again, B's text byte for byte, each taken in before
/// the next, the history still before A. The history with A and B lands:
/// that B is the one expected, not D, which is still on its way; nor is
/// D's own arrival matched by B's text while it is to come. D is taken in
/// and changed (E, `main`) once the history holds A and B: E expects C
/// and D (B's text, a second time), and their history lands before
/// anything of E's: E is still on its way. Then E settles: five answers.
#[gpui_kit::test]
fn a_chain_counts_an_answer_given_twice(cx: &mut TestAppContext) {
    let (tool, id) = ("toolu_d7", "cd7twce");
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, tool, id);
    taken_in(cx, w, &view, row, tool, id, &b);
    let c = change_to(cx, w, &view, row, id, 2, json!({"choice": "release"}));
    taken_in(cx, w, &view, row, tool, id, &c);
    let d = change_to(cx, w, &view, row, id, 3, json!({"choice": "hotfix"}));
    assert_eq!(d, b, "D is B's text");
    lands_settled(cx, w, &view, tool, id, &[&a, &b]);
    out_of(cx, w, &view, row, id, 4, "[A, B]: D");
    echoed_and_delivered(cx, w, &view, row, &d);
    let e = change_to(cx, w, &view, row, id, 1, json!({"choice": "main"}));
    lands_settled(cx, w, &view, tool, id, &[&c, &d]);
    out_of(cx, w, &view, row, id, 5, "[A, B, C, D]: E");
    echoed_and_delivered(cx, w, &view, row, &e);
    lands_settled(cx, w, &view, tool, id, &[&e]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "E, the latest");
    assert_eq!(answers(cx, &view, tool), [a, b.clone(), c, b, e], "A, B, C, D (B's text), E");
    assert_eq!(sends(cx, &view).len(), 5);
    crate::app::pin_arm_clock(None);
}

/// A taken in, changed (B, on its way); the history lands, settled, with
/// `got(A, B)`: an answer of A's text past the one expected, or after B,
/// is another's and later. It wins: B's entry dropped, the card shows it.
fn a_copy_lands(cx: &mut TestAppContext, tool: &str, card_id: &str, got: impl FnOnce(&str, &str) -> Vec<String>) {
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, tool, card_id);
    let more = got(&a, &b);
    let more: Vec<&str> = more.iter().map(String::as_str).collect();
    lands_settled(cx, w, &view, tool, card_id, &more);
    shows_theirs(cx, w, &view, row, card_id, 2, &format!("{} answers", more.len()));
    unconfirmed(cx, w, &view);
    shows_theirs(cx, w, &view, row, card_id, 2, "checked");
    crate::app::pin_arm_clock(None);
}

/// [A, A]: the second is past the count expected.
#[gpui_kit::test]
fn a_copy_past_the_one_expected_wins(cx: &mut TestAppContext) {
    a_copy_lands(cx, "toolu_d8", "cd8copy", |a, _| vec![a.into(), a.into()]);
}

/// [B, A]: history order, not the order clicked: A after B is the later.
#[gpui_kit::test]
fn a_copy_after_the_change_wins(cx: &mut TestAppContext) {
    a_copy_lands(cx, "toolu_d9", "cd9aftr", |a, b| vec![b.into(), a.into()]);
}

/// [A, B, A].
#[gpui_kit::test]
fn a_copy_after_both_wins(cx: &mut TestAppContext) {
    a_copy_lands(cx, "toolu_e1", "ce1both", |a, b| vec![a.into(), b.into(), a.into()]);
}

/// The chain through a gap (Shady #81 rounds 4–7): A and B taken in, C
/// on its way; the daemon drops events for this desk, and the history
/// asked for again lands with A, nothing since saying the engine's slot;
/// then B. C is still this desk's: no change, no Resend. C's echo, a
/// digest saying the slot holds it, its delivery and its history settle
/// it: three answers sent.
#[gpui_kit::test]
fn a_chain_keeps_its_way_through_a_gap(cx: &mut TestAppContext) {
    let input = branch();
    let (tool, id) = ("toolu_e2", "ce2gapc");
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, tool, id);
    taken_in(cx, w, &view, row, tool, id, &b);
    let c = change_to(cx, w, &view, row, id, 2, json!({"choice": "release"}));
    heard(cx, w, &view, vec![json!({"ev": "system_notice", "text": "event stream lagged; 6 dropped"})]);
    let with_a = history_with(cx, &view, tool, id, &input, &[&a]);
    let (seq, sid) = attachment(cx, &view);
    let update = synaps_client::HistoryUpdate::Replace { messages: with_a };
    view.update(cx, |v, cx| v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx));
    play(cx, w, &view);
    view.read_with(cx, |v, _| {
        let t = &v.model.lives[v.panes.cur.live].transcript;
        assert!(!t.resyncing() && !t.slot_known(), "the history asked for again; the slot unknown");
    });
    out_of(cx, w, &view, row, id, 3, "the gap's history, [A]");
    land(cx, w, &view, vec![json!({"role": "user", "content": b})]);
    play(cx, w, &view);
    out_of(cx, w, &view, row, id, 3, "[A, B], the slot unknown");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": c, "delivered": true})]);
    out_of(cx, w, &view, row, id, 3, "C's echo, the slot unknown");
    let with_ab = history_with(cx, &view, tool, id, &input, &[&a, &b]);
    heard(cx, w, &view, vec![slot_digest(&with_ab, Some(&c))]);
    out_of(cx, w, &view, row, id, 3, "the slot holds C");
    heard(cx, w, &view, vec![json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": c}})]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "C taken in");
    lands_settled(cx, w, &view, tool, id, &[&c]);
    assert!(has(cx, w, format!("ask-state {row} changed")), "C's history");
    assert_eq!(answers(cx, &view, tool), [a, b, c]);
    assert_eq!(sends(cx, &view).len(), 3, "nothing sent again");
    crate::app::pin_arm_clock(None);
}

/// A taken in, changed (B), B taken in; then the turn ends without
/// either (a failed turn's repair): its last history, settled, is the
/// old one, then its `Idle`. B is not confirmed, the turn's end its
/// cause, with Resend (nothing in the card over it); A, which B expected,
/// the turn's end lost too.
fn both_lost(cx: &mut TestAppContext, tool: &str, card_id: &str) -> (Asked, String, String) {
    let ((w, view, row, events), a, b) = taken_in_then_changed(cx, tool, card_id);
    taken_in(cx, w, &view, row, tool, card_id, &b);
    let old = history_with(cx, &view, tool, card_id, &branch(), &[]);
    heard(cx, w, &view, vec![slot_digest(&old, None), json!({"ev": "idle"})]);
    play(cx, w, &view);
    unconfirmed(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} not confirmed")), "B: the turn ended without it");
    assert!(has(cx, w, format!("ask-resend {row}")), "B: Resend");
    ((w, view, row, events), a, b)
}

/// Then another desk's answer, byte for byte A's, lands, settled. B's
/// entry saw A expected: the memo reads this one as that, so B stays not
/// confirmed, but A was lost, and this one is another's, later than B:
/// Resend is off, as it is whenever its gate doesn't hold, and a press of
/// it sends nothing; it would put B over it. A change is offered.
#[gpui_kit::test]
fn an_answer_a_turn_lost_is_another_desks_when_it_shows(cx: &mut TestAppContext) {
    let (tool, id) = ("toolu_e3", "ce3miss");
    let ((w, view, row, _events), a, _b) = both_lost(cx, tool, id);
    lands_settled(cx, w, &view, tool, id, &[&a]);
    assert!(has(cx, w, format!("ask-state {row} not confirmed")), "B, as the memo has it");
    assert!(!has(cx, w, format!("ask-resend {row}")) && has(cx, w, format!("ask-resend {row} off")), "no Resend over theirs");
    assert!(has(cx, w, format!("ask-change {row}")), "a change offered");
    let (lid, painted) = painted(cx, &view, tool);
    let got = view.update(cx, |v, _| v.model.resend_card(lid, &painted, Instant::now()));
    assert!(got.is_err(), "Resend pressed: nothing ({got:?})");
    assert_eq!(sends(cx, &view).len(), 2);
    crate::app::pin_arm_clock(None);
}

/// Or B is changed (C) first: C expects neither, A lost and B not
/// confirmed. Another desk's answer of A's text lands, settled: it is
/// the later, and wins over C. No Resend.
#[gpui_kit::test]
fn an_answer_a_turn_lost_is_not_expected(cx: &mut TestAppContext) {
    let (tool, id) = ("toolu_e4", "ce4miss");
    let ((w, view, row, _events), a, _b) = both_lost(cx, tool, id);
    change_to(cx, w, &view, row, id, 2, json!({"choice": "release"}));
    lands_settled(cx, w, &view, tool, id, &[&a]);
    shows_theirs(cx, w, &view, row, id, 3, "their A");
    unconfirmed(cx, w, &view);
    shows_theirs(cx, w, &view, row, id, 3, "checked");
    crate::app::pin_arm_clock(None);
}

/// Call `tool`'s card as the rows draw it, and its store: what a press
/// names.
fn painted(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str) -> (crate::live::LiveId, crate::deliver::Painted) {
    view.read_with(cx, |v, _| {
        let t = &v.model.lives[v.panes.cur.live].transcript;
        let ask = t.items().iter().find_map(|i| match i.card.as_deref() {
            Some(synaps_client::cards::CardRow::Ask(a)) if a.tool_id == tool => Some(a),
            _ => None,
        });
        (v.panes.cur.live, crate::deliver::Painted::of(ask.unwrap()).unwrap())
    })
}

/// What a change expected came, and it is not another's: A taken in,
/// changed (B); A's history lands, settled, and B gets no echo. B is not
/// confirmed, with Resend (A is this desk's, older), and still once the
/// turn ends: A came, and no turn's end lost it.
#[gpui_kit::test]
fn a_change_whose_expected_answer_came_can_be_resent(cx: &mut TestAppContext) {
    let (tool, id) = ("toolu_e6", "ce6came");
    let ((w, view, row, _events), a, b) = taken_in_then_changed(cx, tool, id);
    lands_settled(cx, w, &view, tool, id, &[&a]);
    unconfirmed(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} not confirmed")), "B lost");
    assert!(has(cx, w, format!("ask-resend {row}")), "Resend: A is ours, older");
    let with_a = history_with(cx, &view, tool, id, &branch(), &[&a]);
    heard(cx, w, &view, vec![slot_digest(&with_a, None), json!({"ev": "idle"})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-resend {row}")), "the turn's end: still Resend");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-resend {row}"));
    assert_eq!(texts(cx, &view), vec![a, b.clone(), b], "B sent again");
    crate::app::pin_arm_clock(None);
}

/// Retry too: A taken in, changed (B) with no connection, so B is not
/// sent; the turn then ends without A. Retry shows, nothing over B. Then
/// another desk's answer of A's text lands, settled: Retry is off, and a
/// press of it sends nothing.
#[gpui_kit::test]
fn an_answer_a_turn_lost_is_another_desks_and_retry_keeps_off(cx: &mut TestAppContext) {
    let (tool, id) = ("toolu_e5", "ce5retr");
    let input = branch();
    let (w, view, row, _events) = asked(cx, tool, id, &input);
    nothing_waits_elsewhere(cx, &view);
    busy(cx, w, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    let a = message(id, &input, json!({"choice": "main"}));
    let old = history_with(cx, &view, tool, id, &input, &[]);
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "steered", "text": a, "delivered": true}),
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": a}}),
            slot_digest(&old, None),
        ],
    );
    assert!(has(cx, w, format!("ask-state {row} answered")), "A taken in");
    let client = view.update(cx, |v, _| {
        v.synthetic = false;
        v.client.take()
    });
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 3"));
    assert!(has(cx, w, format!("ask-state {row} not sent")), "B not sent");
    view.update(cx, |v, _| v.client = client);
    heard(cx, w, &view, vec![slot_digest(&old, None), json!({"ev": "idle"})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-retry {row}")), "nothing over B: Retry");
    lands_settled(cx, w, &view, tool, id, &[&a]);
    assert!(has(cx, w, format!("ask-state {row} not sent")), "B, as the memo has it");
    assert!(!has(cx, w, format!("ask-retry {row}")) && has(cx, w, format!("ask-retry {row} off")), "no Retry over theirs");
    let sent = sends(cx, &view).len();
    let (lid, painted) = painted(cx, &view, tool);
    let got = view.update(cx, |v, _| v.model.retry_card(lid, &painted, Instant::now()));
    assert!(got.is_err(), "Retry pressed: nothing ({got:?})");
    assert_eq!(sends(cx, &view).len(), sent, "nothing sent");
    crate::app::pin_arm_clock(None);
}

const SUITES: &str = "Which suites gate the cut-over?";

/// Shady #94 P2-2: a multi-select answer from the chat lists its picks in
/// its own order (`Load`, `Unit`), the draft in the doc's. Reopened and
/// sent untouched, by Send and by Enter, and with a pick ticked off and
/// on again, it is the answer shown: nothing sent, the receipt as it was.
/// A pick added is a change: one `Submit`.
#[gpui_kit::test]
fn an_untouched_multi_select_answer_sends_nothing(cx: &mut TestAppContext) {
    let input = json!({"question": SUITES, "multi": true, "options": ["Unit", "Integration", "Load"]});
    let (w, view, row, _events) = answered(cx, "toolu_cm", "c9multi", &input, json!({"choice": ["Load", "Unit"]}));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    assert!(changing(cx, w, row));
    assert!(has(cx, w, format!("ask-picked {row} 1")) && has(cx, w, format!("ask-picked {row} 3")), "Load and Unit, picked");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-send {row}"));
    assert!(sends(cx, &view).is_empty(), "Send, untouched: nothing");
    assert!(has(cx, w, format!("ask-state {row} answered")) && !changing(cx, w, row), "as it was");
    press(cx, w, "ctrl-j");
    press(cx, w, "enter");
    assert!(changing(cx, w, row), "Enter reopens it");
    press(cx, w, "enter");
    assert!(sends(cx, &view).is_empty(), "Enter, untouched: nothing");
    assert!(has(cx, w, format!("ask-state {row} answered")) && !changing(cx, w, row));
    press(cx, w, "ctrl-j");
    press(cx, w, "enter");
    press(cx, w, "1");
    assert!(!has(cx, w, format!("ask-picked {row} 1")), "Unit off");
    press(cx, w, "1");
    assert!(has(cx, w, format!("ask-picked {row} 1")), "and on again");
    press(cx, w, "enter");
    assert!(sends(cx, &view).is_empty(), "the same picks: nothing");
    assert!(has(cx, w, format!("ask-state {row} answered")) && !changing(cx, w, row));
    press(cx, w, "ctrl-j");
    press(cx, w, "enter");
    press(cx, w, "2");
    press(cx, w, "enter");
    assert_eq!(
        texts(cx, &view),
        vec![changed("c9multi", &input, json!({"choice": ["Unit", "Integration", "Load"]}))],
        "a pick added: a change"
    );
    crate::app::pin_arm_clock(None);
}

/// D4 part B's memory ([`crate::recall`]) meets a change: the answer
/// `text`, sent to the quiet session on screen, reaches the chat (its
/// echo starts the turn, which ends, and the history lands with it).
fn reaches(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, view: &Entity<AppView>, text: &str) {
    heard(
        cx,
        w,
        view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 44, "trigger": "user", "user_text": text}),
            json!({"ev": "stream", "event": {"kind": "session", "session": "done"}}),
            json!({"ev": "idle"}),
        ],
    );
    land(
        cx,
        w,
        view,
        vec![json!({"role": "user", "content": text}), json!({"role": "assistant", "content": [{"type": "text", "text": "On it."}]})],
    );
    play(cx, w, view);
}

/// The agent asks `input` again as card `card_id` (call `tool`): its row.
fn asks_again(
    cx: &mut TestAppContext,
    w: gpui_kit::AnyWindowHandle,
    view: &Entity<AppView>,
    tool: &str,
    card_id: &str,
    input: &Value,
) -> u64 {
    super::answer_tests::ask_live(cx, w, view, tool, card_id, input.clone());
    at_rest(cx, w, view);
    view.read_with(cx, |v, _| v.ask_row(tool).unwrap_or_else(|| panic!("the card of {tool}")))
}

/// The options of card row `row` (from 1) marked `last time`.
fn marked(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, row: u64) -> Vec<usize> {
    (1..=9).filter(|n| has(cx, w, format!("ask-last {row} {n}"))).collect()
}

/// The options of card row `row` (from 1) picked.
fn picked(cx: &mut TestAppContext, w: gpui_kit::AnyWindowHandle, row: u64) -> Vec<usize> {
    (1..=9).filter(|n| has(cx, w, format!("ask-picked {row} {n}"))).collect()
}

/// The entered card's highlight (from 1).
fn hi(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<usize> {
    view.read_with(cx, |v, _| v.panes.cur.ask.entry.as_ref().and_then(|e| e.hi).map(|i| i + 1))
}

/// This desk answers card `card_id` (call `tool`, `input`) by keys,
/// `keys` then the answer `values`, and it reaches the chat: its choice
/// is remembered.
fn remembered(cx: &mut TestAppContext, tool: &str, card_id: &str, input: &Value, keys: &[&str], values: Value) -> Asked {
    let (w, view, row, events) = asked(cx, tool, card_id, input);
    nothing_waits_elsewhere(cx, &view);
    press(cx, w, "ctrl-j");
    for k in keys {
        press(cx, w, k);
    }
    let text = message(card_id, input, values);
    assert_eq!(texts(cx, &view), vec![text.clone()], "sent");
    reaches(cx, w, &view, &text);
    assert_eq!(view.read_with(cx, |v, _| v.recall.len()), 1, "remembered");
    (w, view, row, events)
}

/// The rule #93 and #94 agreed (PR #94, "For part B"): a card reopened to
/// change its answer shows that answer, not the memory. This desk answers
/// `release` (remembered); the same question again starts there, marked
/// `last time`, as ever. Another desk answers it `hotfix` (not this
/// desk's: the memory is still `release`). Reopened, the card shows no
/// `last time` mark, and its highlight and pick are `hotfix`, neither the
/// memory nor the default (`main`). A change from it that reaches the
/// chat is remembered as a first answer is: the next card is marked on
/// it.
#[gpui_kit::test]
fn a_change_starts_on_the_answer_it_changes_and_is_remembered(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, _first, _events) = remembered(cx, "toolu_r1", "cr1mem1", &input, &["2"], json!({"choice": "release"}));
    let row = asks_again(cx, w, &view, "toolu_r2", "cr2mem2", &input);
    assert_eq!(marked(cx, w, row), [2], "a fresh card: last time, release");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row));
    assert_eq!(hi(cx, &view), Some(2), "a fresh card starts on the memory");
    press(cx, w, "escape");
    land(cx, w, &view, vec![json!({"role": "user", "content": message("cr2mem2", &input, json!({"choice": "hotfix"}))})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} answered")), "another desk's answer, hotfix");
    assert_eq!(view.read_with(cx, |v, _| v.recall.len()), 1);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row), "Ctrl+J: the answer to change");
    press(cx, w, "enter");
    assert!(changing(cx, w, row), "reopened");
    assert_eq!(marked(cx, w, row), Vec::<usize>::new(), "reopened: no last time mark");
    assert_eq!(hi(cx, &view), Some(3), "the highlight on the answer changed, hotfix");
    assert!(has(cx, w, format!("ask-hi {row} 3")) && !has(cx, w, format!("ask-hi {row} 2")));
    assert_eq!(picked(cx, w, row), [3], "hotfix picked");
    let sent = sends(cx, &view).len();
    press(cx, w, "1");
    let c = changed("cr2mem2", &input, json!({"choice": "main"}));
    assert_eq!(texts(cx, &view).last(), Some(&c), "the change: main");
    assert_eq!(sends(cx, &view).len(), sent + 1);
    reaches(cx, w, &view, &c);
    assert!(has(cx, w, format!("ask-state {row} changed")));
    let next = asks_again(cx, w, &view, "toolu_r3", "cr3mem3", &input);
    assert_eq!(marked(cx, w, next), [1], "the change, remembered: last time, main");
    crate::app::pin_arm_clock(None);
}

/// A reopened card whose answer has no pick, a skip, falls back to the
/// memory (`release`), unmarked, before the default (`main`); nothing
/// picked.
#[gpui_kit::test]
fn a_reopened_skip_starts_on_the_memory_unmarked(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, _first, _events) = remembered(cx, "toolu_r4", "cr4mem4", &input, &["2"], json!({"choice": "release"}));
    let row = asks_again(cx, w, &view, "toolu_r5", "cr5mem5", &input);
    land(cx, w, &view, vec![json!({"role": "user", "content": skipped("cr5mem5", BRANCH)})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} skipped")), "another desk's skip");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row));
    press(cx, w, "enter");
    assert!(changing(cx, w, row), "reopened");
    assert_eq!(marked(cx, w, row), Vec::<usize>::new(), "no last time mark");
    assert_eq!(hi(cx, &view), Some(2), "the memory, unmarked: release");
    assert_eq!(picked(cx, w, row), Vec::<usize>::new(), "nothing picked");
    crate::app::pin_arm_clock(None);
}

/// So does a multi pick answered with none: the memory's first (`Load`),
/// unmarked, nothing ticked.
#[gpui_kit::test]
fn a_reopened_empty_multi_pick_starts_on_the_memory_unmarked(cx: &mut TestAppContext) {
    let input = json!({"question": SUITES, "multi": true, "options": ["Unit", "Integration", "Load"]});
    let (w, view, _first, _events) = remembered(cx, "toolu_r6", "cr6mem6", &input, &["3", "enter"], json!({"choice": ["Load"]}));
    let row = asks_again(cx, w, &view, "toolu_r7", "cr7mem7", &input);
    assert_eq!(marked(cx, w, row), [3], "a fresh card: last time, Load");
    assert_eq!(picked(cx, w, row), Vec::<usize>::new(), "never ticked");
    land(cx, w, &view, vec![json!({"role": "user", "content": message("cr7mem7", &input, json!({"choice": []}))})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} answered")), "another desk's answer, no pick");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, row));
    press(cx, w, "enter");
    assert!(changing(cx, w, row), "reopened");
    assert_eq!(marked(cx, w, row), Vec::<usize>::new(), "no last time mark");
    assert_eq!(hi(cx, &view), Some(3), "the memory, unmarked: Load");
    assert_eq!(picked(cx, w, row), Vec::<usize>::new(), "nothing ticked");
    crate::app::pin_arm_clock(None);
}
