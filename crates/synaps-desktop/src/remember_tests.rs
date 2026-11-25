//! Desk-local memory of card choices in the window (synaps-cards
//! `docs/design.md` §9.3's D4 row, "smart defaults"; [`crate::recall`]):
//! the same question in the same folder starts on the last choice, marked
//! "last time", and Enter on it sends exactly one `Submit` of it; another
//! folder or changed options recall nothing; a multi pick's set is marked,
//! never ticked; only an answer that reached the chat (answered, or a
//! steer taken in) is remembered (not sent, queued, unconfirmed and
//! Resend waiting are not); off, the file is neither read nor written;
//! Forget, and Settings' link to it; an unreadable file; no free text in
//! the file; no frame at rest; a pending permission prompt untouched; a
//! choice remembered meanwhile never moves an open card.
//!
//! Each test's memory has a file of its own (`remembering`); the window's
//! default (on, no file) is every other test's. The window and key helpers
//! are D2a's and D2b's ([`super::answer_tests`], [`super::keys_tests`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use super::answer_tests::{arm, ask_live, attachment, click, has, heard, land, llm, message, play, sends, show};
use super::keys_tests::{asked, at_rest, entered, keys_at, press, prompt, prompt_pending, ship_input, texts};
use super::settle;
use crate::app::AppView;
use crate::deliver::Outgoing;
use crate::recall::Recall;
use crate::settings_view::Pick;

/// A session in the folder `IDLE_CPU` is in (`/home/you/work/synaps-desktop`).
const SAME_FOLDER: &str = "20261113-184102-9d0e";
/// A session in another folder (`/home/you/docs`).
const OTHER_FOLDER: &str = "20261112-180203-44aa";

/// The window's memory on, with a file of this test's own (its folder
/// emptied first).
fn remembering(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("synaps-desktop-remember-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join(crate::recall::FILE);
    view.update(cx, |v, _| v.recall = Recall::new(true, Some(path.clone())));
    path
}

/// Questions remembered, and the file's writes started.
fn kept(cx: &mut TestAppContext, view: &Entity<AppView>) -> (usize, u32) {
    view.read_with(cx, |v, _| (v.recall.len(), v.recall.writes))
}

/// The file's reads started.
fn reads(cx: &mut TestAppContext, view: &Entity<AppView>) -> u32 {
    view.read_with(cx, |v, _| v.recall.reads)
}

/// The row and display index of the card of call `tool`, in the pane.
fn row(cx: &mut TestAppContext, view: &Entity<AppView>, tool: &str) -> (u64, usize) {
    view.read_with(cx, |v, _| {
        let id = v.ask_row(tool).unwrap_or_else(|| panic!("the card of {tool}"));
        (id, v.panes.cur.disp.iter().position(|d| d.id == id).expect("shown"))
    })
}

/// Option `n` (1-based) of card row `id` is marked "last time".
fn last(cx: &mut TestAppContext, w: AnyWindowHandle, id: u64, n: usize) -> bool {
    has(cx, w, format!("ask-last {id} {n}"))
}

fn marked(cx: &mut TestAppContext, w: AnyWindowHandle, id: u64) -> Vec<usize> {
    (1..=9).filter(|n| last(cx, w, id, *n)).collect()
}

/// The entered card's highlight (1-based).
fn hi(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<usize> {
    view.read_with(cx, |v, _| v.panes.cur.ask.entry.as_ref().and_then(|e| e.hi).map(|i| i + 1))
}

/// The answer `text`, sent to the quiet session on screen, reaches the
/// chat: its echo starts the turn (the card: answered), which ends, and
/// the history lands with it.
fn reached(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str) {
    heard(
        cx,
        w,
        view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 44, "trigger": "user", "user_text": text}),
            llm(json!({"kind": "session", "session": "done"})),
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

/// The agent asks `input` again as card `card` (call `tool`): its row,
/// the composer with the keys.
fn asks_again(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, card: &str, input: &Value) -> u64 {
    ask_live(cx, w, view, tool, card, input.clone());
    at_rest(cx, w, view);
    row(cx, view, tool).0
}

/// Session `id` opened in a new tab, shown (the demo answers the attach).
fn open_session(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str) {
    let client = view.update(cx, |v, _| v.client.take());
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
    });
    cx.run_until_parked();
    play(cx, w, view);
    view.update(cx, |v, _| v.client = client);
    let shown = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()));
    assert_eq!(shown.as_deref(), Some(id), "{id} on screen");
}

/// One 144 Hz frame of the clock, its timers firing, and a frame drawn
/// only if the window asked for one (`ask_tests`' own).
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

/// The same question in the same folder: the card marks last time's
/// choice and Ctrl+J starts on it, not on the default; nothing is sent
/// until Enter, which sends exactly one `Submit` of it, on the card's
/// attachment. Remembered only once the first answer reached the chat.
#[gpui_kit::test]
fn the_same_question_in_the_same_folder_starts_on_the_last_choice(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, first, _events) = asked(cx, "toolu_m1", "c1mem01", &input);
    let path = remembering(cx, &view, "same");
    assert!(marked(cx, w, first).is_empty(), "nothing remembered yet");
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(2), "the default");
    press(cx, w, "1");
    let text = message("c1mem01", &input, json!({"choice": "Run it now"}));
    assert_eq!(texts(cx, &view), vec![text.clone()]);
    assert_eq!(kept(cx, &view), (0, 0), "pressed: nothing remembered");
    assert!(!path.exists());
    reached(cx, w, &view, &text);
    assert!(has(cx, w, format!("ask-state {first} answered")));
    assert_eq!(kept(cx, &view), (1, 1), "answered: remembered, and written");
    assert!(path.exists(), "written off the UI thread");
    let again = asks_again(cx, w, &view, "toolu_m2", "c2mem02", &input);
    assert_eq!(marked(cx, w, again), vec![1], "Run it now: last time");
    assert!(has(cx, w, format!("ask-suggested {again} 2")), "the agent's default still says so");
    assert_eq!(sends(cx, &view).len(), 1, "recalled: nothing sent");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, again));
    assert_eq!(hi(cx, &view), Some(1), "the highlight starts on last time's choice");
    assert!(has(cx, w, format!("ask-hi {again} 1")) && !has(cx, w, format!("ask-hi {again} 2")));
    assert_eq!(sends(cx, &view).len(), 1, "entered: nothing sent");
    press(cx, w, "enter");
    let (seq, _) = attachment(cx, &view);
    let s = sends(cx, &view);
    assert_eq!(s.len(), 2, "Enter: exactly one more");
    assert_eq!(
        s[1],
        Outgoing {
            seq,
            busy: false,
            card: cards_spec::CardId::parse("c2mem02").unwrap(),
            text: message("c2mem02", &input, json!({"choice": "Run it now"}))
        },
        "one Submit of that choice, on the card's attachment"
    );
}

/// The options changed: another question, nothing recalled. Another
/// session in another folder: nothing. Another session in the same
/// folder: recalled.
#[gpui_kit::test]
fn another_folder_or_changed_options_recall_nothing(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, _first, _events) = asked(cx, "toolu_n1", "c1nnn01", &input);
    remembering(cx, &view, "other");
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    reached(cx, w, &view, &message("c1nnn01", &input, json!({"choice": "Run it now"})));
    assert_eq!(kept(cx, &view).0, 1);
    let mut changed = input.clone();
    changed["options"].as_array_mut().unwrap().push(json!("Ask Ana first"));
    let id = asks_again(cx, w, &view, "toolu_n2", "c2nnn02", &changed);
    assert!(marked(cx, w, id).is_empty(), "an option more: another question");
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(2), "the default");
    press(cx, w, "escape");
    open_session(cx, w, &view, OTHER_FOLDER);
    let id = asks_again(cx, w, &view, "toolu_n3", "c3nnn03", &input);
    assert!(marked(cx, w, id).is_empty(), "another folder");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id));
    assert_eq!(hi(cx, &view), Some(2), "the default");
    press(cx, w, "escape");
    open_session(cx, w, &view, SAME_FOLDER);
    let id = asks_again(cx, w, &view, "toolu_n4", "c4nnn04", &input);
    assert_eq!(marked(cx, w, id), vec![1], "the same folder, another session: recalled");
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(1));
    assert_eq!(sends(cx, &view).len(), 1, "nothing sent but the first answer");
}

/// A multi pick: its set is remembered and marked, the highlight starts
/// on its first; the ticks stay the agent's default (highlight only), so
/// Enter sends what is ticked.
#[gpui_kit::test]
fn a_multi_pick_marks_its_set_and_never_ticks_it(cx: &mut TestAppContext) {
    let input = json!({"question": "Which suites should pass?", "wait": true, "multi": true,
        "options": ["Unit", "Integration", "Load test", "Chaos"], "default": ["Unit", "Integration"]});
    let (w, view, _first, _events) = asked(cx, "toolu_s1", "c1sss01", &input);
    remembering(cx, &view, "multi");
    press(cx, w, "ctrl-j");
    for k in ["1", "2", "3", "4"] {
        press(cx, w, k);
    }
    press(cx, w, "enter");
    let text = message("c1sss01", &input, json!({"choice": ["Load test", "Chaos"]}));
    assert_eq!(texts(cx, &view), vec![text.clone()]);
    reached(cx, w, &view, &text);
    let id = asks_again(cx, w, &view, "toolu_s2", "c2sss02", &input);
    assert_eq!(marked(cx, w, id), vec![3, 4], "the set: last time");
    for n in [1, 2] {
        assert!(has(cx, w, format!("ask-picked {id} {n}")), "{n}: the agent's default, ticked");
    }
    for n in [3, 4] {
        assert!(!has(cx, w, format!("ask-picked {id} {n}")), "{n}: marked, not ticked");
    }
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(3), "the set's first");
    press(cx, w, "enter");
    assert_eq!(texts(cx, &view)[1], message("c2sss02", &input, json!({"choice": ["Unit", "Integration"]})), "what is ticked");
}

/// Only an answer that reached the chat is remembered: not one not sent
/// (Retry), nor one unconfirmed (Resend waiting); the Resend's echo, yes.
/// A skip that reaches the chat keeps nothing.
#[gpui_kit::test]
fn only_an_answer_that_reached_the_chat_is_remembered(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, id, _events) = asked(cx, "toolu_c1", "c1ccc01", &input);
    let path = remembering(cx, &view, "confirmed");
    // No connection, and no synthetic daemon to take it: not sent.
    let client = view.update(cx, |v, _| {
        v.synthetic = false;
        v.client.take()
    });
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    assert!(has(cx, w, format!("ask-state {id} not sent")));
    assert_eq!(kept(cx, &view), (0, 0), "not sent: nothing");
    view.update(cx, |v, _| v.client = client);
    let (_, ix) = row(cx, &view, "toolu_c1");
    show(cx, w, &view, ix);
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-retry {id}"));
    assert!(has(cx, w, format!("ask-state {id} sending…")));
    assert_eq!(kept(cx, &view), (0, 0), "sending: nothing");
    // No echo in 10 s: not confirmed, Resend offered.
    view.update(cx, |v, cx| {
        v.model.tick_cards(Instant::now() + Duration::from_secs(11));
        v.cards_moved(cx);
    });
    settle(cx, w, 8);
    assert!(has(cx, w, format!("ask-state {id} not confirmed")));
    assert!(has(cx, w, format!("ask-resend {id}")));
    assert_eq!(kept(cx, &view), (0, 0), "Resend waiting: nothing");
    arm(cx, &view, id, true);
    click(cx, w, &view, format!("ask-resend {id}"));
    assert_eq!(kept(cx, &view), (0, 0), "sent again: nothing yet");
    assert!(!path.exists());
    let text = message("c1ccc01", &input, json!({"choice": "Run it now"}));
    assert_eq!(texts(cx, &view), vec![text.clone(); 3], "the press, Retry, Resend");
    reached(cx, w, &view, &text);
    assert_eq!(kept(cx, &view), (1, 1), "its echo: remembered, written");
    let file = std::fs::read(&path).expect("the file");
    // A skip keeps nothing.
    let skip = asks_again(cx, w, &view, "toolu_c2", "c2ccc02", &input);
    press(cx, w, "ctrl-j");
    press(cx, w, "0");
    let skipped = texts(cx, &view).pop().unwrap();
    assert!(skipped.starts_with("[card c2ccc02 skipped]"));
    reached(cx, w, &view, &skipped);
    assert!(has(cx, w, format!("ask-state {skip} skipped")));
    assert_eq!(kept(cx, &view), (1, 1), "a skip: nothing more");
    assert_eq!(std::fs::read(&path).unwrap(), file);
}

/// Taken in: busy, the answer goes as a `Steer`. Queued (its echo) is not
/// remembered; the engine's `SteeringDelivered` is (D2a's "taken in").
#[gpui_kit::test]
fn a_steer_is_remembered_once_taken_in(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, id, _events) = asked(cx, "toolu_d1", "c1ddd01", &input);
    remembering(cx, &view, "steer");
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 44, "trigger": "user", "user_text": "go on"})]);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    let s = sends(cx, &view);
    assert_eq!(s.len(), 1);
    assert!(s[0].busy, "a Steer: the session is busy");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": s[0].text, "delivered": true})]);
    assert!(has(cx, w, format!("ask-state {id} queued · the agent reads it at its next step")));
    assert_eq!(kept(cx, &view), (0, 0), "queued: nothing");
    heard(cx, w, &view, vec![llm(json!({"kind": "agent", "agent": "steering_delivered", "message": s[0].text}))]);
    assert!(has(cx, w, format!("ask-state {id} answered")), "taken in");
    assert_eq!(kept(cx, &view), (1, 1), "taken in: remembered, written");
}

/// Off (Settings): the file is neither read nor written, nothing is
/// marked or highlighted from it, and an answer is not kept. On again:
/// read then, and the next card recalls what the file kept.
#[gpui_kit::test]
fn off_the_file_is_neither_read_nor_written(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, _first, _events) = asked(cx, "toolu_o1", "c1ooo01", &input);
    let path = remembering(cx, &view, "off");
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    reached(cx, w, &view, &message("c1ooo01", &input, json!({"choice": "Run it now"})));
    let before = std::fs::read(&path).expect("written while on");
    view.update(cx, |v, cx| v.settings_pick(Pick::Remember, "off", cx));
    assert_eq!(kept(cx, &view).0, 0, "off: what was read, let go");
    // As the next launch finds it: off, its file there.
    view.update(cx, |v, _| v.recall = Recall::new(false, Some(path.clone())));
    assert_eq!(reads(cx, &view), 0, "off: not read");
    let id = asks_again(cx, w, &view, "toolu_o2", "c2ooo02", &input);
    assert!(marked(cx, w, id).is_empty(), "off: nothing marked");
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(2), "the default");
    press(cx, w, "3");
    reached(cx, w, &view, &message("c2ooo02", &input, json!({"choice": "Don't run it"})));
    assert_eq!((kept(cx, &view), reads(cx, &view)), ((0, 0), 0), "off: no read, no write");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the file as it was");
    view.update(cx, |v, cx| v.settings_pick(Pick::Remember, "on", cx));
    assert_eq!((kept(cx, &view), reads(cx, &view)), ((1, 0), 1), "on: read now");
    let id = asks_again(cx, w, &view, "toolu_o3", "c3ooo03", &input);
    assert_eq!(marked(cx, w, id), vec![1], "what the file kept (Don't run it never was)");
}

/// Forget: the memory empties and its file goes; an open card's mark goes
/// with it, and the next card starts on the default. Off, Forget removes
/// the file too, unread.
#[gpui_kit::test]
fn forget_empties_the_memory_and_removes_the_file(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, _first, _events) = asked(cx, "toolu_g1", "c1ggg01", &input);
    let path = remembering(cx, &view, "forget");
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    reached(cx, w, &view, &message("c1ggg01", &input, json!({"choice": "Run it now"})));
    let saved = std::fs::read(&path).expect("written");
    let open = asks_again(cx, w, &view, "toolu_g2", "c2ggg02", &input);
    assert_eq!(marked(cx, w, open), vec![1]);
    view.update(cx, |v, cx| v.forget_card_choices(cx));
    cx.run_until_parked();
    settle(cx, w, 8);
    assert!(!path.exists(), "the file is gone");
    assert_eq!(kept(cx, &view).0, 0);
    assert!(marked(cx, w, open).is_empty(), "the open card's mark went with it");
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(2), "the default");
    press(cx, w, "escape");
    std::fs::write(&path, &saved).unwrap();
    view.update(cx, |v, _| v.recall = Recall::new(false, Some(path.clone())));
    view.update(cx, |v, cx| v.forget_card_choices(cx));
    cx.run_until_parked();
    assert!(!path.exists(), "off: removed all the same");
    assert_eq!(reads(cx, &view), 0, "and never read");
}

/// An unreadable file: the window starts empty (and says why), works, and
/// the next answer writes a good file over it.
#[gpui_kit::test]
fn an_unreadable_file_starts_empty_and_the_next_answer_replaces_it(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, first, _events) = asked(cx, "toolu_u1", "c1uuu01", &input);
    let path = remembering(cx, &view, "corrupt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{\"version\": 1, \"choices\": [{\"key\": \x00\xff").unwrap();
    view.update(cx, |v, _| v.recall = Recall::new(true, Some(path.clone())));
    let said = view.read_with(cx, |v, _| v.recall.said.clone());
    assert!(said.is_some(), "it says why");
    assert_eq!((kept(cx, &view), reads(cx, &view)), ((0, 0), 1), "empty");
    settle(cx, w, 8);
    assert!(marked(cx, w, first).is_empty());
    press(cx, w, "ctrl-j");
    press(cx, w, "3");
    reached(cx, w, &view, &message("c1uuu01", &input, json!({"choice": "Don't run it"})));
    let (store, bad) = crate::recall::Store::parse(&std::fs::read_to_string(&path).unwrap()).expect("a good file now");
    assert_eq!((store.len(), bad), (1, 0));
}

/// What the file keeps is a key and option ids: never a note, a field's
/// text, the question, a label or the folder.
#[gpui_kit::test]
fn no_free_text_reaches_the_file(cx: &mut TestAppContext) {
    let note = json!({"question": "Ship it?", "wait": true, "options": ["Yes", {"label": "Not yet", "note": "What's missing?"}]});
    let (w, view, _first, _events) = asked(cx, "toolu_t1", "c1ttt01", &note);
    let path = remembering(cx, &view, "text");
    press(cx, w, "ctrl-j");
    press(cx, w, "2");
    cx.simulate_input(w, "SECRET-NOTE-7f3a");
    settle(cx, w, 8);
    press(cx, w, "enter");
    let text = message("c1ttt01", &note, json!({"choice": "Not yet", "note": "SECRET-NOTE-7f3a"}));
    assert_eq!(texts(cx, &view), vec![text.clone()], "the note goes to the agent");
    reached(cx, w, &view, &text);
    let form = json!({"question": "Who reviews the migration?", "wait": true, "options": ["Ana", "Ben"],
        "fields": [{"id": "who", "label": "Reviewer handle", "value": "hunter2-field"},
            {"id": "why", "label": "Context", "type": "long", "value": "LONG-SECRET-ctx"},
            {"id": "ci", "label": "Run CI first", "type": "check", "value": true}]});
    asks_again(cx, w, &view, "toolu_t2", "c2ttt02", &form);
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    press(cx, w, "enter");
    let text = texts(cx, &view).pop().unwrap();
    assert!(text.contains("hunter2-field") && text.contains("LONG-SECRET-ctx"), "the fields go to the agent: {text}");
    reached(cx, w, &view, &text);
    assert_eq!(kept(cx, &view).0, 2);
    let file = std::fs::read_to_string(&path).expect("the file");
    for s in [
        "SECRET-NOTE",
        "hunter2",
        "LONG-SECRET",
        "true",
        "Ship it",
        "Not yet",
        "missing",
        "Who reviews",
        "Ana",
        "Reviewer",
        "synaps-desktop",
        "/home",
    ] {
        assert!(!file.contains(s), "{s:?} in the file:\n{file}");
    }
    let v: Value = serde_json::from_str(&file).unwrap();
    assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["version", "choices"]);
    for e in v["choices"].as_array().unwrap() {
        let o = e.as_object().unwrap();
        assert_eq!(o.keys().collect::<Vec<_>>(), ["key", "pick"], "{e}");
        assert!(o["key"].as_str().unwrap().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(o["pick"].as_array().unwrap().iter().all(Value::is_u64));
    }
}

/// A remembered card is a static row (§6.1 Motion): shown with its mark,
/// and entered on it, the window at rest draws nothing and asks for
/// nothing.
#[gpui_kit::test]
fn a_remembered_card_asks_for_no_frame_at_rest(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, _first, _events) = asked(cx, "toolu_r1", "c1rrr01", &input);
    remembering(cx, &view, "idle");
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    reached(cx, w, &view, &message("c1rrr01", &input, json!({"choice": "Run it now"})));
    let id = asks_again(cx, w, &view, "toolu_r2", "c2rrr02", &input);
    assert_eq!(marked(cx, w, id), vec![1]);
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| window.focus(&v.focus, cx))).expect("the window");
    cx.run_until_parked();
    for _ in 0..144 {
        idle_frame(cx, w, &view);
    }
    for i in 0..144 {
        assert_eq!(idle_frame(cx, w, &view), (0, 0), "shown, at rest, frame {i}: drawn, queued");
    }
    press(cx, w, "ctrl-j");
    assert_eq!(hi(cx, &view), Some(1));
    for _ in 0..144 {
        idle_frame(cx, w, &view);
    }
    for i in 0..144 {
        assert_eq!(idle_frame(cx, w, &view), (0, 0), "entered, at rest, frame {i}: drawn, queued");
    }
    assert!(entered(cx, w, id) && last(cx, w, id, 1));
    assert_eq!(sends(cx, &view).len(), 1);
}

/// §5.1: a remembered choice never touches a pending permission prompt.
/// Ctrl+J goes to the prompt first; again, to the card, on last time's
/// `always`; Enter answers the card, and the prompt is still pending,
/// unanswered; Ctrl+J goes back to it.
#[gpui_kit::test]
fn a_pending_prompt_is_untouched(cx: &mut TestAppContext) {
    let input = json!({"question": "Allow it?", "wait": true, "options": ["yes", "always", "no"]});
    let (w, view, _first, _events) = asked(cx, "toolu_p1", "c1ppp01", &input);
    remembering(cx, &view, "prompt");
    press(cx, w, "ctrl-j");
    press(cx, w, "2");
    reached(cx, w, &view, &message("c1ppp01", &input, json!({"choice": "always"})));
    let id = asks_again(cx, w, &view, "toolu_p2", "c2ppp02", &input);
    prompt(cx, w, &view);
    assert_eq!(marked(cx, w, id), vec![2]);
    press(cx, w, "ctrl-j");
    assert_eq!(keys_at(cx, w, &view), (false, true), "the prompt first: the window's keys");
    assert!(!entered(cx, w, id));
    prompt_pending(cx, &view, "at the prompt");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, id), "again: the card");
    assert_eq!(hi(cx, &view), Some(2), "on last time's always");
    press(cx, w, "enter");
    let t = texts(cx, &view);
    assert_eq!(t.len(), 2);
    assert_eq!(t[1], message("c2ppp02", &input, json!({"choice": "always"})));
    prompt_pending(cx, &view, "the card answered always");
    press(cx, w, "ctrl-j");
    assert_eq!(keys_at(cx, w, &view), (false, true), "back to the prompt");
    prompt_pending(cx, &view, "back at the prompt");
}

/// What a card recalls is fixed at its first sight: a choice that reaches
/// the chat while the user is in another card of the same question never
/// moves its highlight nor marks it, entered again included. The next
/// card recalls it.
#[gpui_kit::test]
fn a_choice_remembered_meanwhile_never_moves_an_open_card(cx: &mut TestAppContext) {
    let input = ship_input();
    let (w, view, first, _events) = asked(cx, "toolu_f1", "c1fff01", &input);
    remembering(cx, &view, "fixed");
    let second = asks_again(cx, w, &view, "toolu_f2", "c2fff02", &input);
    // The first answered by a click: sending.
    let (_, ix) = row(cx, &view, "toolu_f1");
    show(cx, w, &view, ix);
    arm(cx, &view, first, true);
    click(cx, w, &view, format!("ask-option {first} 1"));
    crate::app::pin_arm_clock(None);
    let text = message("c1fff01", &input, json!({"choice": "Run it now"}));
    assert_eq!(texts(cx, &view), vec![text.clone()]);
    at_rest(cx, w, &view);
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, second), "the other card");
    assert_eq!(hi(cx, &view), Some(2), "the default: nothing remembered yet");
    reached(cx, w, &view, &text);
    assert_eq!(kept(cx, &view).0, 1, "remembered meanwhile");
    assert!(entered(cx, w, second), "still in it");
    assert_eq!(hi(cx, &view), Some(2), "its highlight never moved");
    assert!(marked(cx, w, second).is_empty(), "nor is it marked");
    press(cx, w, "escape");
    press(cx, w, "ctrl-j");
    assert!(entered(cx, w, second));
    assert_eq!(hi(cx, &view), Some(2), "entered again: as it first was");
    press(cx, w, "escape");
    let third = asks_again(cx, w, &view, "toolu_f3", "c3fff03", &input);
    assert_eq!(marked(cx, w, third), vec![1], "the next card recalls it");
}

/// Settings, THIS APP: "Remember card choices" with "forget remembered
/// choices" beside it; a click forgets (the file goes) and it says
/// nothing is remembered.
#[gpui_kit::test]
fn settings_forget_remembered_choices(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, VisualTestContext};
    let input = ship_input();
    let (w, view, _first, _events) = asked(cx, "toolu_v1", "c1vvv01", &input);
    let path = remembering(cx, &view, "settings");
    press(cx, w, "ctrl-j");
    press(cx, w, "1");
    reached(cx, w, &view, &message("c1vvv01", &input, json!({"choice": "Run it now"})));
    assert!(path.exists());
    view.update(cx, |v, cx| {
        v.open_view(crate::tabs::Kind::Settings, cx);
        v.settings_find("remember card".into(), cx);
    });
    cx.run_until_parked();
    play(cx, w, &view);
    assert!(!has(cx, w, "recall-none".into()));
    let mut vcx = VisualTestContext::from_window(w, cx);
    let position = vcx.debug_bounds("recall-forget").expect("forget remembered choices, drawn").center();
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
    assert!(!path.exists(), "the file is gone");
    assert_eq!(kept(cx, &view).0, 0);
    assert!(has(cx, w, "recall-none".into()) && !has(cx, w, "recall-forget".into()), "nothing remembered");
}
