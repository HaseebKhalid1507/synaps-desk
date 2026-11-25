//! Changing an answer, and Skip (synaps-cards `docs/design.md` §3.3, §3.5,
//! §3.6, §6.1, §7; §9.3's D4 row), in the window: F15 by mouse (2 clicks)
//! and by keys (3: Ctrl+J, Enter, a key), each exactly one `Submit` of
//! `[card <id> changed]` on the card's attachment; F7, Skip, on a card
//! reopened, and Resend and Retry after a skip (its receipt says what they
//! would send); a change while the first answer is on its way (none) or
//! not confirmed (it replaces it); a closed card, or one gone from the
//! history, takes none; a change from the other pane; the later answer
//! wins; Keep, Esc and the answer already given send nothing; Tab on a
//! form's receipt opens nothing; a change never resolves a permission
//! prompt (§5.1). A change of an answer taken in, or not confirmed,
//! stays on its way when that answer's history lands after it is sent,
//! through a gap too, and a later answer of another desk's still wins
//! (Shady #94 P2-1).
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
/// sent after it: as the history asked for again gives it.
fn history_with(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str, card_id: &str, input: &Value, more: &[&str]) -> Vec<Value> {
    let (seq, _) = attachment(cx, view);
    let mut h = match crate::demo::attach(crate::demo::IDLE_CPU, seq).pop() {
        Some(synaps_client::Event::Attached { attached, .. }) => attached.conversation.api_messages.clone(),
        other => panic!("{other:?}"),
    };
    h.extend(super::answer_tests::asked_messages(tool, card_id, input, None));
    let held = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.history_len());
    assert_eq!(h.len(), held, "the history held");
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
    assert!(!has(cx, w, format!("ask-change {row}")) && !has(cx, w, format!("ask-change {row} off")), "{what}: no change offered");
    assert!(!has(cx, w, format!("ask-resend {row}")) && !has(cx, w, format!("ask-resend {row} off")), "{what}: no Resend");
    view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        let st = v.model.memos.get_state(&sid, card_id);
        assert!(st.is_some_and(|s| !matches!(s, State::InChat | State::Unconfirmed | State::NotSent)), "{what}: B in flight ({st:?})");
    });
    assert_eq!(sends(cx, view).len(), 2, "{what}: nothing more sent");
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

/// P2-1, the answer not confirmed: A (no echo in 10 s) is changed, B. A
/// did reach the chat, and its history lands late, before anything of
/// B's: B is still on its way. An answer this desk didn't send, another
/// desk's change (C), lands after it: the later answer wins, B's entry is
/// dropped, and the card shows C, with no Resend of B over it.
#[gpui_kit::test]
fn a_change_of_an_answer_not_confirmed_keeps_its_way_and_a_later_one_wins(cx: &mut TestAppContext) {
    let input = branch();
    let (w, view, row, _events) = asked(cx, "toolu_d3", "cd3uncf", &input);
    nothing_waits_elsewhere(cx, &view);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    let a = message("cd3uncf", &input, json!({"choice": "main"}));
    unconfirmed(cx, w, &view);
    assert!(has(cx, w, format!("ask-resend {row}")) && has(cx, w, format!("ask-change {row}")), "A not confirmed");
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-change {row}"));
    arm(cx, &view, row, true);
    click(cx, w, &view, format!("ask-option {row} 3"));
    let b = changed("cd3uncf", &input, json!({"choice": "hotfix"}));
    assert_eq!(texts(cx, &view), vec![a.clone(), b.clone()]);
    land(cx, w, &view, vec![json!({"role": "user", "content": a})]);
    play(cx, w, &view);
    assert!(has(cx, w, format!("ask-state {row} sending…")), "A's history, late: B still on its way");
    still_out(cx, w, &view, row, "cd3uncf", "A's history");
    let c = changed("cd3uncf", &input, json!({"choice": "release"}));
    land(cx, w, &view, vec![json!({"role": "user", "content": c})]);
    play(cx, w, &view);
    view.read_with(cx, |v, _| {
        let sid = v.card_session().unwrap();
        assert_eq!(v.model.memos.get_state(&sid, "cd3uncf"), None, "C, later: B's entry dropped");
    });
    assert!(has(cx, w, format!("ask-state {row} changed")), "the card shows C");
    assert!(!has(cx, w, format!("ask-resend {row}")), "no Resend of B over it");
    assert_eq!(sends(cx, &view).len(), 2);
    crate::app::pin_arm_clock(None);
}
