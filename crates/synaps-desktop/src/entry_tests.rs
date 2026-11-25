//! Card entry from anywhere (Shady #83 P2-7; synaps-cards `docs/design.md`
//! §6.1, §7's F10): D3's ways in, Ctrl+Shift+J, a WAITING row in Ctrl+P
//! and a tab dot, enter the exact card they land on
//! ([`AppView::card_landed`], then [`AppView::enter_card`]), so the next
//! key answers it: exactly one `Submit`, on the card's attachment. (The
//! top bar's JUMP lands through the same `go_to_card`; its card branch
//! needs a live card in a session no pane shows, which no state holds
//! today, so it is not driven here.) From
//! the composer, another pane, another tab or session; the oldest wait
//! card first; with a confirm prompt pending, the card and never the
//! prompt, and Esc from the card never denies it. A held Ctrl+Shift+J
//! jumps once, a repeat X11 or macOS doesn't flag included. A card's OS
//! notification (#89), clicked: entered the same way; a stale one moves
//! nothing. A card's entry held for the window to come up gives way to a
//! newer jump to another pane's card (Shady #92 P2-D1), a rejected click
//! never.
//!
//! The window and key helpers are D2a's and D2b's ([`super::answer_tests`],
//! [`super::keys_tests`]). The client stays (its sends are the view's
//! record) except while a session is brought forward: the demo answers
//! the attach then.

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, SystemNotificationResponse, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::answer_tests::{SHIP, ask_live, attachment, card, heard, land, llm, message, open, play, sends, showing};
use super::keys_tests::{at_rest, composer_text, down, entered, keys_at, keystroke, press, prompt, prompt_pending, ship_input, up};
use super::settle;
use crate::app::AppView;
use crate::presence::Key;

/// The session the focused pane shows.
fn shown(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()))
}

/// `f` with the client away: the demo answers what attaches meanwhile
/// (a tab opened, a session brought forward), as D2a's `open` does.
fn by_the_demo<R>(cx: &mut TestAppContext, view: &Entity<AppView>, f: impl FnOnce(&mut TestAppContext) -> R) -> R {
    let client = view.update(cx, |v, _| v.client.take());
    let r = f(cx);
    view.update(cx, |v, _| v.client = client);
    r
}

/// Session `id` opened in a new tab, then the first tab shown again: `id`
/// behind, its cards as last seen (its tab's cache).
fn open_behind(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str) {
    by_the_demo(cx, view, |cx| {
        view.update(cx, |v, cx| {
            v.open_tab(crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false }, crate::tabs::Place::New);
            cx.notify();
        });
        cx.run_until_parked();
        play(cx, w, view);
        view.update(cx, |v, cx| {
            v.focus_tab(0);
            cx.notify();
        });
        cx.run_until_parked();
        play(cx, w, view);
    });
}

/// A click at the centre of what `sel` names, anywhere in the window (a
/// tab's dot, the top bar).
fn click_at(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    let mut vcx = VisualTestContext::from_window(w, cx);
    let position = vcx.debug_bounds(String::leak(sel.clone())).unwrap_or_else(|| panic!("{sel} drawn")).center();
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

/// What was sent: exactly one `Submit` (the session idle, not a `Steer`)
/// of `text`, on attachment `seq`.
fn one_submit(cx: &mut TestAppContext, view: &Entity<AppView>, seq: u64, text: &str, what: &str) {
    let s = sends(cx, view);
    assert_eq!(s.len(), 1, "{what}: one answer, {s:?}");
    assert_eq!(s[0].text, text, "{what}");
    assert!(!s[0].busy, "{what}: a Submit");
    assert_eq!(s[0].seq, seq, "{what}: on the card's attachment");
}

/// A wait card asking `q`, four options, the first the default.
fn wait_card(q: &str) -> Value {
    json!({"question": q, "wait": true, "options": ["main", "release", "hotfix", "next"], "default": "main"})
}

/// A quiet session (`IDLE_CPU`) whose agent waits on card `card` (call
/// `tool`), the composer with the keys.
fn waits(cx: &mut TestAppContext, tool: &str, card_id: &str, input: &Value) -> (AnyWindowHandle, Entity<AppView>, u64) {
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    ask_live(cx, w, &view, tool, card_id, input.clone());
    let (id, _) = card(cx, &view, input["question"].as_str().expect("a question"));
    at_rest(cx, w, &view);
    (w, view, id)
}

/// F10 by keys from the composer: Ctrl+Shift+J lands on the card and
/// enters it (before D3 met D2b, it landed and the keys stayed in the
/// composer); a digit answers it.
#[gpui_kit::test]
fn ctrl_shift_j_from_the_composer_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    let input = wait_card("Which branch?");
    let (w, view, id) = waits(cx, "toolu_e1", "c1entry", &input);
    press(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert!(entered(cx, w, id), "the card it landed on, entered");
    assert_eq!(keys_at(cx, w, &view), (false, false), "the keys left the composer, never for the window");
    press(cx, w, "2");
    let seq = attachment(cx, &view).0;
    one_submit(cx, &view, seq, &message("c1entry", &input, json!({"choice": "release"})), "from the composer");
}

/// `ASKS` (its wait card, `SHIP`) in the first pane; `IDLE_CPU` in a
/// second, focused, its composer with the keys. The first pane, the
/// card's row and its attachment.
fn asks_in_the_other_pane(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>, crate::pane::PaneId, u64, u64) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let (ship, _) = card(cx, &view, SHIP);
    let (seq, _) = attachment(cx, &view);
    let first = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, _| v.app_settings.split = true);
    press(cx, w, "ctrl-\\");
    play(cx, w, &view);
    by_the_demo(cx, &view, |cx| {
        view.update(cx, |v, cx| {
            let open = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
            v.open_tab(open, crate::tabs::Place::Current);
            cx.notify();
        });
        play(cx, w, &view);
    });
    let other = view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 2);
        assert_ne!(v.panes.focused, first, "the other pane is focused");
        v.model.lives[v.panes.cur.live].attachment()
    });
    assert!(other.is_some_and(|o| o != seq), "the focused pane holds another attachment");
    at_rest(cx, w, &view);
    (w, view, first, ship, seq)
}

/// F10 by keys from another pane: Ctrl+Shift+J focuses the card's pane
/// and enters the card; a digit sends on that pane's attachment.
#[gpui_kit::test]
fn ctrl_shift_j_from_another_pane_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    let (w, view, first, ship, seq) = asks_in_the_other_pane(cx);
    press(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), first, "the card's pane, focused");
    assert!(entered(cx, w, ship));
    press(cx, w, "3");
    one_submit(cx, &view, seq, &message("c0p6r4n", &ship_input(), json!({"choice": "Don't run it"})), "from another pane");
}

/// F10 by keys from another tab: `ASKS` behind, its card as last seen.
/// Ctrl+Shift+J brings its tab forward (attached again, the card
/// confirmed), lands and enters; a digit sends on the new attachment.
#[gpui_kit::test]
fn ctrl_shift_j_from_another_tab_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    open_behind(cx, w, &view, crate::demo::ASKS);
    assert_eq!(shown(cx, &view).as_deref(), Some(crate::demo::IDLE_CPU));
    at_rest(cx, w, &view);
    by_the_demo(cx, &view, |cx| {
        press(cx, w, "ctrl-shift-j");
        play(cx, w, &view);
    });
    assert_eq!(shown(cx, &view).as_deref(), Some(crate::demo::ASKS), "its session in front");
    let (ship, _) = card(cx, &view, SHIP);
    assert!(entered(cx, w, ship));
    press(cx, w, "3");
    let seq = attachment(cx, &view).0;
    one_submit(cx, &view, seq, &message("c0p6r4n", &ship_input(), json!({"choice": "Don't run it"})), "from another tab");
}

/// F10 by mouse from another tab: its (hollow) dot, then a key. The click
/// brings the session forward, lands and enters; a digit answers.
#[gpui_kit::test]
fn a_tab_dot_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    open_behind(cx, w, &view, crate::demo::ASKS);
    at_rest(cx, w, &view);
    let tab = view.read_with(cx, |v, _| {
        v.model.tabs.iter().find(|t| t.session.as_deref() == Some(crate::demo::ASKS)).map(|t| t.id).expect("its tab")
    });
    by_the_demo(cx, &view, |cx| {
        click_at(cx, w, format!("tab {tab} last-seen"));
        play(cx, w, &view);
    });
    assert_eq!(shown(cx, &view).as_deref(), Some(crate::demo::ASKS));
    let (ship, _) = card(cx, &view, SHIP);
    assert!(entered(cx, w, ship), "the dot's card, entered");
    press(cx, w, "1");
    let seq = attachment(cx, &view).0;
    one_submit(cx, &view, seq, &message("c0p6r4n", &ship_input(), json!({"choice": "Run it now"})), "by the tab dot");
}

/// Ctrl+P's WAITING row, picked: its card, entered; Enter takes its
/// default.
#[gpui_kit::test]
fn a_waiting_row_then_enter_sends_one_submit(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    open_behind(cx, w, &view, crate::demo::ASKS);
    at_rest(cx, w, &view);
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.open_palette(crate::palette::Mode::GoTo, window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    let first = view.read_with(cx, |v, _| v.palette.as_ref().and_then(|p| p.selected().map(|e| e.act.clone())));
    assert!(
        matches!(&first, Some(crate::palette::Act::Card(k)) if k.session == crate::demo::ASKS && k.tool_id == "toolu_a8"),
        "WAITING's first row: {first:?}"
    );
    by_the_demo(cx, &view, |cx| {
        cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.palette_pick(window, cx))).expect("the window");
        play(cx, w, &view);
    });
    let (ship, _) = card(cx, &view, SHIP);
    assert!(entered(cx, w, ship), "the row's card, entered");
    press(cx, w, "enter");
    let seq = attachment(cx, &view).0;
    let text = message("c0p6r4n", &ship_input(), json!({"choice": "Wait for the low-traffic window"}));
    one_submit(cx, &view, seq, &text, "by a WAITING row");
}

/// Card `card` (call `tool`) asked live, its rows timed `at` (the
/// history's `_synaps_at`): the waiting order is its age.
fn ask_at(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, (tool, card_id): (&str, &str), input: &Value, at: &str) {
    heard(
        cx,
        w,
        view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"}),
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_ui", "tool_id": tool})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_ui", "tool_id": tool, "input": input})),
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": showing(card_id, input)})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    let mut messages = super::answer_tests::asked_messages(tool, card_id, input, None);
    for m in &mut messages {
        m["_synaps_at"] = json!(at);
    }
    land(cx, w, view, messages);
    play(cx, w, view);
}

/// Several wait cards: Ctrl+Shift+J takes the oldest (its row's time, not
/// its call's name), wait cards before an older open one. Each jump then
/// a key: one `Submit` each, in that order; the open card last.
#[gpui_kit::test]
fn ctrl_shift_j_takes_the_oldest_wait_card_first(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    let (open_q, older, newer) = (
        json!({"question": "Which log level?", "options": ["info", "debug"]}),
        wait_card("Which branch, first asked?"),
        wait_card("Which branch, asked since?"),
    );
    ask_at(cx, w, &view, ("toolu_o9", "c2open9"), &open_q, "2026-11-24T10:00:00Z");
    ask_at(cx, w, &view, ("toolu_w9", "c2wait9"), &older, "2026-11-24T10:20:00Z");
    ask_at(cx, w, &view, ("toolu_w1", "c2wait1"), &newer, "2026-11-24T10:40:00Z");
    let (rows, seq) = (
        [
            card(cx, &view, "Which branch, first asked?").0,
            card(cx, &view, "Which branch, asked since?").0,
            card(cx, &view, "Which log level?").0,
        ],
        attachment(cx, &view).0,
    );
    let mut want = Vec::new();
    for (n, (row, key, text)) in [
        (rows[0], "2", message("c2wait9", &older, json!({"choice": "release"}))),
        (rows[1], "enter", message("c2wait1", &newer, json!({"choice": "main"}))),
        (rows[2], "1", message("c2open9", &open_q, json!({"choice": "info"}))),
    ]
    .into_iter()
    .enumerate()
    {
        if n > 0 {
            // The batch went on to the next card: back to the composer.
            press(cx, w, "escape");
        }
        at_rest(cx, w, &view);
        press(cx, w, "ctrl-shift-j");
        play(cx, w, &view);
        assert!(entered(cx, w, row), "jump {n}: the oldest still waiting");
        press(cx, w, key);
        want.push(text);
        let s = sends(cx, &view);
        assert_eq!(s.iter().map(|o| o.text.clone()).collect::<Vec<_>>(), want, "jump {n}: one Submit");
        assert!(s.iter().all(|o| !o.busy && o.seq == seq));
    }
}

/// With a confirm prompt pending: Ctrl+Shift+J enters the card, never
/// the prompt; Esc from the card goes back to the composer and never
/// denies it; jumped to again, a digit answers the card, and the prompt
/// is still pending, unanswered.
#[gpui_kit::test]
fn ctrl_shift_j_with_a_prompt_pending_never_touches_it(cx: &mut TestAppContext) {
    let input = json!({"question": "Go ahead?", "wait": true, "options": ["yes", "always"]});
    let (w, view, id) = waits(cx, "toolu_ep", "c9entpr", &input);
    prompt(cx, w, &view);
    at_rest(cx, w, &view);
    press(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert!(entered(cx, w, id), "the card, never the prompt");
    prompt_pending(cx, &view, "entered");
    press(cx, w, "escape");
    assert!(!entered(cx, w, id));
    assert_eq!(keys_at(cx, w, &view), (true, false), "to the composer, never the window (whose Esc denies)");
    prompt_pending(cx, &view, "Esc from the card");
    press(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert!(entered(cx, w, id));
    press(cx, w, "2");
    let seq = attachment(cx, &view).0;
    one_submit(cx, &view, seq, &message("c9entpr", &input, json!({"choice": "always"})), "with a prompt pending");
    prompt_pending(cx, &view, "answered always");
}

/// Ctrl+Shift+J held jumps once: its repeats, flagged or not (X11's key-up
/// right before each; macOS's with no key-up), ask for no landing, so the
/// card stays as the keys left it (no new entry, its highlight kept).
#[gpui_kit::test]
fn a_held_ctrl_shift_j_jumps_once(cx: &mut TestAppContext) {
    let input = wait_card("Which branch?");
    let (w, view, id) = waits(cx, "toolu_eh", "c1enthd", &input);
    let hi = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.cur.ask.entry.as_ref().and_then(|e| e.hi));
    down(cx, w, "ctrl-shift-j");
    play(cx, w, &view);
    assert!(entered(cx, w, id));
    press(cx, w, "down");
    assert_eq!(hi(cx), Some(1));
    let repeat = |cx: &mut TestAppContext, up_first: bool| {
        let ks = keystroke(cx, "ctrl-shift-j");
        cx.update_window(w, |_, window, cx| {
            if up_first {
                window.dispatch_event(gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke: ks.clone() }), cx);
            }
            let ev = gpui_kit::KeyDownEvent { keystroke: ks, is_held: false, prefer_character_input: false };
            window.dispatch_event(gpui_kit::PlatformInput::KeyDown(ev), cx);
            assert!(view.read(cx).lookout.land.is_none(), "up first {up_first}: a repeat asks for no landing");
        })
        .expect("the window");
        play(cx, w, &view);
    };
    repeat(cx, false);
    repeat(cx, true);
    assert!(entered(cx, w, id), "still the card");
    assert_eq!(hi(cx), Some(1), "as the keys left it");
    up(cx, w, "ctrl-shift-j");
    crate::app::next_press();
    press(cx, w, "enter");
    let seq = attachment(cx, &view).0;
    one_submit(cx, &view, seq, &message("c1enthd", &input, json!({"choice": "release"})), "held Ctrl+Shift+J");
}

/// A card above a view that follows its tail, not laid out (two cards
/// asked since pushed it up): `chord` lands on it and enters it, and it
/// is brought into view, painted, its keys live; a digit answers it.
/// GPUI's reveal only anchors: a list following its tail snapped back to
/// the end at its next layout, the card entered but never painted, and
/// the digit typed into the composer.
fn a_card_above_the_tail(cx: &mut TestAppContext, chord: &str) {
    let input = wait_card("Which branch, first asked?");
    let (w, view, id) = waits(cx, "toolu_t1", "c3above", &input);
    ask_live(cx, w, &view, "toolu_t2", "c3belw1", json!({"question": "Which log level?", "options": ["info", "debug"]}));
    ask_live(cx, w, &view, "toolu_t3", "c3belw2", json!({"question": "Which region?", "options": ["eu", "us"]}));
    at_rest(cx, w, &view);
    let (following, laid_out) = view.read_with(cx, |v, _| {
        let ix = v.panes.cur.disp.iter().position(|d| d.id == id).expect("its row");
        (v.panes.cur.list.is_following_tail(), v.panes.cur.list.bounds_for_item(ix).is_some())
    });
    assert!(following && !laid_out, "the view follows its tail; the card is above it, not laid out");
    press(cx, w, chord);
    play(cx, w, &view);
    assert!(entered(cx, w, id), "{chord}: the card, in view, its keys live");
    press(cx, w, "2");
    let seq = attachment(cx, &view).0;
    one_submit(cx, &view, seq, &message("c3above", &input, json!({"choice": "release"})), chord);
    assert_eq!(composer_text(cx, &view), "", "{chord}: nothing typed into the composer");
}

#[gpui_kit::test]
fn ctrl_j_to_a_card_above_the_tail_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    a_card_above_the_tail(cx, "ctrl-j");
}

#[gpui_kit::test]
fn ctrl_shift_j_to_a_card_above_the_tail_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    a_card_above_the_tail(cx, "ctrl-shift-j");
}

/// The window away (another app in front): what a card's notification is
/// for.
fn away(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    VisualTestContext::from_window(w, cx).deactivate_window();
    settle(cx, w, 8);
    assert!(!view.read_with(cx, |v, _| v.window_active), "the window away");
}

/// A click on notification `tag`, as the OS reports it (#89's
/// `AppView::notification_clicked`).
fn click_note(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tag: &str) {
    cx.simulate_system_notification_response(SystemNotificationResponse { tag: tag.to_string().into(), action_id: None });
    cx.run_until_parked();
    play(cx, w, view);
}

/// F12 by a notification (#89, then D2b's entry): the window away, a wait
/// card comes and the OS is told. Its click brings the window up, lands
/// on the card and enters it; a digit answers: exactly one `Submit`.
/// Then a stale click, a tag still held for that binding (a click racing
/// its answer): checked before anything moves, so no window raised, no
/// landing, nothing entered, and the keys stay where they were; a digit
/// sends nothing more.
#[gpui_kit::test]
fn a_notification_click_then_a_digit_sends_one_submit(cx: &mut TestAppContext) {
    // The platform shows notifications for a named app (as `main`).
    cx.update(|cx| cx.set_app_identity("synaps-desktop", "Synaps"));
    let (w, view, _events) = open(cx, crate::demo::IDLE_CPU);
    at_rest(cx, w, &view);
    away(cx, w, &view);
    let input = wait_card("Which branch, told?");
    ask_live(cx, w, &view, "toolu_e9", "c9note1", input.clone());
    let (id, _) = card(cx, &view, "Which branch, told?");
    let told = cx.shown_system_notifications();
    assert_eq!(told.len(), 1, "one notification: {told:?}");
    let tag = told[0].tag.to_string();
    assert!(!entered(cx, w, id), "told, not entered");
    click_note(cx, w, &view, &tag);
    assert!(view.read_with(cx, |v, _| v.window_active), "the window up");
    assert!(entered(cx, w, id), "the notification's card, entered");
    assert_eq!(keys_at(cx, w, &view), (false, false), "the keys left the composer, never for the window");
    press(cx, w, "2");
    let seq = attachment(cx, &view).0;
    let text = message("c9note1", &input, json!({"choice": "release"}));
    one_submit(cx, &view, seq, &text, "by a notification");
    // On its way: taken down. The same binding's tag, still held (posted
    // again as a click races its answer), then clicked.
    play(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.lookout.notes.card(&tag).is_none()), "answered: taken down");
    let key = view.read_with(cx, |v, _| v.lookout.landed.clone()).expect("the card it landed on");
    let note = crate::notify::note(&key, "Relay log levels", "Which branch, told?");
    assert_eq!(note.tag, tag, "the same binding's tag");
    view.update(cx, |v, cx| v.lookout.notes.post(key, note, cx));
    cx.run_until_parked();
    away(cx, w, &view);
    let before = keys_at(cx, w, &view);
    click_note(cx, w, &view, &tag);
    view.read_with(cx, |v, _| {
        assert!(v.lookout.notes.card(&tag).is_none(), "stale: taken down");
        assert!(v.lookout.land.is_none() && v.lookout.entering.is_none(), "no landing, no entry waiting");
        assert!(!v.window_active, "no window raised");
    });
    assert!(!entered(cx, w, id), "nothing entered");
    assert_eq!(keys_at(cx, w, &view), before, "the keys where they were");
    press(cx, w, "3");
    one_submit(cx, &view, seq, &text, "a stale click, then a digit: nothing more");
}

/// Shady #92 P2-D1's two panes. `IDLE_CPU` in the first, focused, and its
/// wait card A, told while the window was away; in the second, `FLAKY`
/// in front and `ASKS` behind (its card `SHIP`, B, as last seen, and its
/// notification). A's jump went and landed with the window still away
/// (its activation on its way, or refused): its entry held for the window
/// to come up ([`AppView::landed_up`]). A rejected click (a card no longer
/// open) leaves it held. The client stays: B's attach, when its jump
/// brings `ASKS` forward, waits for [`b_attached`]. The first pane, A's
/// row, B's key and tag, and the client's events (kept open).
fn a_held(
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<AppView>, crate::pane::PaneId, u64, Key, String, async_channel::Receiver<synaps_client::Event>) {
    cx.update(|cx| cx.set_app_identity("synaps-desktop", "Synaps"));
    let (w, view, events) = open(cx, crate::demo::IDLE_CPU);
    let first = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, _| v.app_settings.split = true);
    press(cx, w, "ctrl-\\");
    play(cx, w, &view);
    by_the_demo(cx, &view, |cx| {
        view.update(cx, |v, cx| {
            let open = crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false };
            v.open_tab(open, crate::tabs::Place::Current);
            cx.notify();
        });
        play(cx, w, &view);
    });
    open_behind(cx, w, &view, crate::demo::ASKS);
    assert_eq!(shown(cx, &view).as_deref(), Some(crate::demo::FLAKY), "ASKS behind in the second pane");
    view.update(cx, |v, cx| v.focus_pane(first, crate::pane::KeysTo::Pane, cx));
    play(cx, w, &view);
    at_rest(cx, w, &view);
    assert_eq!(shown(cx, &view).as_deref(), Some(crate::demo::IDLE_CPU), "the first pane");
    away(cx, w, &view);
    let input = wait_card("Which branch, held?");
    ask_live(cx, w, &view, "toolu_e10", "c10held", input);
    let a = card_here(cx, &view, "Which branch, held?");
    let told = cx.shown_system_notifications();
    assert_eq!(told.len(), 1, "A told: {told:?}");
    let ka = view.read_with(cx, |v, _| v.lookout.notes.card(&told[0].tag).cloned()).expect("A's binding");
    // B, as the OS was told of it when it came.
    let kb = view.read_with(cx, |v, _| {
        v.model.presence.listed().find(|c| c.session == crate::demo::ASKS && c.question == SHIP).map(|c| c.key()).expect("B waits")
    });
    let note = crate::notify::note(&kb, "Asks", SHIP);
    let tag = note.tag.clone();
    view.update(cx, |v, cx| v.lookout.notes.post(kb.clone(), note, cx));
    cx.run_until_parked();
    // A's jump: it goes, lands, and its entry waits for the window.
    let went = cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.go_to_card(ka.clone(), window, cx))).expect("the window");
    assert!(went, "A's jump goes");
    play(cx, w, &view);
    let held = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.lookout.entering.as_ref().map(|t| t.key.clone()));
    assert_eq!(held(cx).as_ref(), Some(&ka), "A landed, held for the window");
    assert!(!entered(cx, w, a), "not entered while the window is away");
    // A click on a card no longer open: rejected, A still held.
    let gone =
        Key { session: crate::demo::IDLE_CPU.into(), tool_id: "toolu_gone".into(), card: cards_spec::CardId::parse("c10gone").unwrap() };
    let stale = crate::notify::note(&gone, "Relay log levels", "Gone?");
    let stale_tag = stale.tag.clone();
    view.update(cx, |v, cx| v.lookout.notes.post(gone, stale, cx));
    cx.run_until_parked();
    click_note(cx, w, &view, &stale_tag);
    assert_eq!(held(cx).as_ref(), Some(&ka), "a rejected click: A still held");
    assert!(!view.read_with(cx, |v, _| v.window_active), "no window raised");
    (w, view, first, a, kb, tag, events)
}

/// The row of the card asking `question` in the focused pane (D2a's
/// `card` reads the first pane's).
fn card_here(cx: &mut TestAppContext, view: &Entity<AppView>, question: &str) -> u64 {
    view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].transcript.items();
        v.panes.cur.disp.iter().find(|d| {
            d.kind == crate::turns::Kind::Ask
                && matches!(items.get(d.item).and_then(|i| i.card.as_deref()), Some(synaps_client::cards::CardRow::Ask(a)) if a.doc.question == question)
        })
        .map(|d| d.id)
        .unwrap_or_else(|| panic!("a card asking {question:?} in the focused pane"))
    })
}

/// The window up, B's jump gone and its attach still on its way: A never
/// entered (the second pane keeps the focus, B's landing waits). B's
/// attach comes: B lands and is entered; a digit sends exactly one
/// `Submit`, on B's attachment.
fn b_attached(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, first: crate::pane::PaneId, a: u64, kb: &Key) {
    assert!(view.read_with(cx, |v, _| v.window_active), "the window up");
    view.read_with(cx, |v, _| {
        assert!(v.lookout.entering.is_none(), "A no longer held");
        assert_eq!(v.lookout.land.as_ref().map(|t| &t.key), Some(kb), "B's landing waits");
        assert_ne!(v.panes.focused, first, "B's pane keeps the focus");
    });
    assert!(!entered(cx, w, a), "A never entered");
    let seq = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].wanted()).expect("B's attach on its way");
    view.update(cx, |v, cx| v.apply_batch(crate::demo::attach(crate::demo::ASKS, seq), cx));
    cx.run_until_parked();
    play(cx, w, view);
    assert_eq!(shown(cx, view).as_deref(), Some(crate::demo::ASKS), "B's session in front");
    let b = card_here(cx, view, SHIP);
    assert!(entered(cx, w, b), "B entered");
    assert!(!entered(cx, w, a), "A never entered");
    press(cx, w, "3");
    assert_eq!(attachment(cx, view).0, seq);
    one_submit(cx, view, seq, &message("c0p6r4n", &ship_input(), json!({"choice": "Don't run it"})), "B, the latest jump");
}

/// Shady #92 P2-D1, by a notification: A held; B's notification clicked.
/// Its jump goes (the second pane, `ASKS` brought forward) and asks for
/// the window, which comes up before B's row is built.
#[gpui_kit::test]
fn a_newer_notification_outranks_an_entry_held_for_the_window(cx: &mut TestAppContext) {
    let (w, view, first, a, kb, tag, _events) = a_held(cx);
    click_note(cx, w, &view, &tag);
    b_attached(cx, w, &view, first, a, &kb);
}

/// Shady #92 P2-D1, by a tab dot: A held; B's tab's dot clicked in the
/// window still away (its activation after the click, as a click to
/// focus may bring it), then the window comes up before B's row is built.
#[gpui_kit::test]
fn a_newer_tab_dot_outranks_an_entry_held_for_the_window(cx: &mut TestAppContext) {
    let (w, view, first, a, kb, _tag, _events) = a_held(cx);
    let tab = view.read_with(cx, |v, _| {
        let p = v.panes.rest.iter().find(|p| p.id != first).expect("the second pane");
        p.parked.tabs.iter().find(|t| t.session.as_deref() == Some(crate::demo::ASKS)).map(|t| t.id).expect("B's tab")
    });
    click_at(cx, w, format!("tab {tab} last-seen"));
    cx.update_window(w, |_, window, _| window.activate_window()).expect("the window");
    cx.run_until_parked();
    play(cx, w, &view);
    b_attached(cx, w, &view, first, a, &kb);
}
