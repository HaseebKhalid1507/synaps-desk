//! Split panes step 2, through the view (`AppView`) against a client on a
//! real socket: a fake daemon (std threads) that hands each attach to the
//! test, as `model::fake_daemon`'s.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Focusable as _, Pixels, Point, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, px, size,
};
use serde_json::{Value, json};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};
use synaps_client::{Client, ClientOptions, Endpoint, Event, Identity};

use crate::app::AppView;

const A: &str = "20261121-020000-aaaa";
const B: &str = "20261121-020000-bbbb";

struct Conn {
    r: BufReader<UnixStream>,
    w: UnixStream,
}

impl Conn {
    fn send(&mut self, v: Value) {
        let _ = writeln!(self.w, "{v}");
    }

    /// The session commands among the next frames, until EOF or `wait` of silence.
    fn cmds(&mut self, wait: Duration) -> Vec<Value> {
        let mut out = Vec::new();
        let _ = self.r.get_ref().set_read_timeout(Some(wait));
        let mut line = String::new();
        while self.r.read_line(&mut line).is_ok_and(|n| n > 0) {
            let f: Value = serde_json::from_str(&line).unwrap_or_default();
            line.clear();
            if f["type"] == "cmd" {
                out.push(f["cmd"].clone());
            }
        }
        out
    }
}

/// `Hello` → `Welcome` (session A listed), an empty list for the control
/// connection, every `Attach` handed to the test.
fn start(name: &str) -> (PathBuf, mpsc::Receiver<Conn>) {
    let dir = std::env::temp_dir().join(format!("synaps-desktop-split-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut c = Conn { w: stream.try_clone().unwrap(), r: BufReader::new(stream) };
                let mut line = String::new();
                if !c.r.read_line(&mut line).is_ok_and(|n| n > 0) {
                    return;
                }
                c.send(json!({"type": "welcome", "protocol_version": 3, "daemon_version": "0.0.0-fake", "pid": 1, "profile": null,
                    "sessions": [{"id": A, "model": "m", "lifecycle": "live"}, {"id": B, "model": "m", "lifecycle": "live"}],
                    "progressive_tool_disclosure": false, "generation": 1, "features": ["owned_sessions"]}));
                line.clear();
                while c.r.read_line(&mut line).is_ok_and(|n| n > 0) {
                    let f: Value = serde_json::from_str(&line).unwrap_or_default();
                    line.clear();
                    match f["type"].as_str() {
                        Some("sessions") => c.send(json!({"type": "session_list", "sessions": []})),
                        Some("attach") => {
                            let _ = tx.send(c);
                            return;
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    (path, rx)
}

/// The client's events until one matches `done` (the test holds them: the
/// view under test is driven by the `--offline` demo's, as `ui_tests`').
fn until(events: &async_channel::Receiver<Event>, what: &str, done: impl Fn(&Event) -> bool) {
    crate::test_waits::until(events, what, done)
}

/// The `--offline` demo's window at 1280×800, text step 16 (`ui_tests`'
/// `open_at` without its client: the test gives it one).
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    open_sized(cx, 1280., 800.)
}

/// [`open`] at `w`×`h`.
fn open_sized(cx: &mut TestAppContext, w: f32, h: f32) -> (AnyWindowHandle, Entity<AppView>) {
    let wv = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text: 16 }, cx);
        crate::app::bind_keys(cx);
        // The layout lands in one frame (split.md §10 at Reduced): what
        // these tests read is where the panes are; the motion tests pin Full.
        cx.set_global(crate::motion::Setting { level: Some(crate::motion::Level::Reduced), lock: None });
        crate::motion::Level::apply(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(w), px(h)) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        crate::beat::open_window(options, cx, true, |window, cx| {
            cx.new(|cx| {
                let v = AppView::new(None, Some(crate::demo::events()), window, cx);
                v.fx.pin_clock();
                v
            })
        })
        .expect("a test window")
    });
    cx.run_until_parked();
    wv
}

/// `Attached` for `sid`, one pending confirm (`prompt`) if any.
fn attached(sid: &str, prompt: Option<u64>) -> Value {
    let prompts: Vec<Value> = prompt
        .map(|id| {
            json!({"id": id, "kind": "confirm", "title": "Confirm tool call",
            "prompt": "Extension: guard\nTool: edit\n\nMessage from guard:\nok\n\nAllow this tool call?"})
        })
        .into_iter()
        .collect();
    json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": sid, "model": "m", "lifecycle": "live"},
        "conversation": conversation(), "pending_prompts": prompts})
}

/// A short conversation (a list with rows: a prompt's card is drawn as one).
fn conversation() -> Value {
    let messages = synaps_client::synthetic::history(4, 7);
    json!({"api_messages": messages, "messages_len": messages.len()})
}

/// The client's events into the view, as its pump hands them
/// (`apply_batch`), frames drawn, until `done`.
fn feed(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<AppView>,
    events: &async_channel::Receiver<Event>,
    what: &str,
    done: impl Fn(&AppView) -> bool,
) {
    crate::test_waits::feed(cx, window, view, events, what, done)
}

/// Shady #32 H1: the last tab closed detaches from its session, as on main
/// (`c.detach()`). Its close switches out of the tab first (`stash`), which
/// clears the attachment the store wants; the one it holds is detached.
#[gpui_kit::test]
fn closing_the_last_tab_detaches_its_attachment(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let (path, attaches) = start("close-last");
    let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::desktop())).expect("a client thread");
    until(&events, "connected", |e| matches!(e, Event::Status(synaps_client::Status::Connected { .. })));
    // The one tab shows A: its attach goes out on the client.
    let seq = view.update(cx, |v, cx| {
        v.client = Some(client);
        v.model.tabs.open(A, "a", crate::tabs::Place::Current);
        v.reconcile(None);
        cx.notify();
        v.model.lives[v.panes.cur.live].wanted()
    });
    cx.run_until_parked();
    let seq = seq.expect("A's attach asked for");
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.iter().count()), 1, "one tab");
    let mut a = crate::test_waits::recv(&attaches).expect("A's attach at the daemon");
    a.send(attached(A, None));
    until(&events, "A attached", |e| matches!(e, Event::Attached { attach_seq, .. } if *attach_seq == seq));
    // Ctrl+W on it.
    view.update(cx, |v, cx| {
        v.close_tab(0);
        cx.notify();
    });
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.iter().count()), 0, "no tab");
    // The client still runs (the view holds it): what A's connection got
    // came from the close, not from a shutdown.
    let on_a = a.cmds(Duration::from_millis(500));
    assert!(on_a.iter().any(|c| c["cmd"] == "detach"), "the last tab closed, A's connection got no detach: {on_a:?}");
    view.update(cx, |v, _| {
        if let Some(c) = v.client.take() {
            let _ = c.shutdown();
        }
    });
}

/// Shady #32 L3, step 2's half of plan §6.6: a second store attached to
/// another session (step 3's other pane, as the tab bench's `mem_lives`
/// adds one), each with its prompt #1 up. The view's Allow, HALT and
/// message, driven as the user does (`AppView::answer`, `halt`, the
/// composer's Enter), go out on the shown store's attachment only.
#[gpui_kit::test]
fn the_views_allow_halt_and_message_reach_only_the_shown_stores_session(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let (path, attaches) = start("two-stores");
    let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::desktop())).expect("a client thread");
    until(&events, "connected", |e| matches!(e, Event::Status(synaps_client::Status::Connected { .. })));
    view.update(cx, |v, cx| {
        v.client = Some(client.clone());
        v.model.tabs.open(A, "a", crate::tabs::Place::Current);
        v.reconcile(None);
        cx.notify();
    });
    let mut a = crate::test_waits::recv(&attaches).expect("A's attach at the daemon");
    a.send(attached(A, Some(1)));
    // The other store, on B: its prompt is #1 too.
    let b = view.update(cx, |v, _| {
        let b = v.model.lives.add();
        let seq = client.attach_more(B, synaps_client::AttachMode::Mirror).expect("B's attach");
        v.model.attaching(b, seq, None, false);
        v.model.asked_for(b, B);
        b
    });
    let mut on_b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    on_b.send(attached(B, Some(1)));
    // Its buttons act once it has been up `ARM` (wall time).
    feed(cx, window, &view, &events, "both attached, A's card up and armed", |v| {
        v.model.lives[v.panes.cur.live].is_active(A)
            && v.model.lives[b].is_active(B)
            && v.panes.cur.card.as_ref().is_some_and(|c| c.armed())
    });
    view.read_with(cx, |v, _| {
        assert_ne!(v.panes.cur.live, b, "the pane shows the first store");
        assert!(v.model.lives[b].may_answer(1), "B's prompt #1 is up in its store");
    });
    // Allow, HALT, a message: through the view.
    view.update(cx, |v, _| {
        let key = v.panes.cur.card.as_ref().map(|c| c.key).expect("A's card");
        assert!(v.answer(key, Some(crate::model::CONFIRM_YES.into())), "Allow went");
        v.halt();
        assert!(v.enter("hello A".into()), "the message went");
    });
    let names =
        |c: &mut Conn| c.cmds(Duration::from_millis(400)).iter().filter_map(|c| c["cmd"].as_str().map(str::to_string)).collect::<Vec<_>>();
    let (sent_a, sent_b) = (names(&mut a), names(&mut on_b));
    assert_eq!(sent_a, ["answer", "cancel", "submit"], "on A's attachment");
    assert!(sent_b.is_empty(), "nothing on B's attachment: {sent_b:?}");
    view.update(cx, |v, _| {
        v.client = None;
    });
    let _ = client.shutdown();
}

// ── Split panes step 3: panes on screen ─────────────────────────────────

/// `k` down and up, then a frame. Raw: the input clock does not move
/// (`crate::app::input_now`), so a key of the same gate let go just before
/// makes this one its repeat. A new press is [`press`].
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
    let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// A new press of `k`: the input clock past a repeat's `FoldKeys::PAIR`
/// first (`crate::app::next_press`: no frame, no wall time), then [`key`].
fn press(cx: &mut TestAppContext, window: AnyWindowHandle, k: &str) {
    crate::app::next_press();
    key(cx, window, k);
}

fn names(c: &mut Conn) -> Vec<String> {
    c.cmds(Duration::from_millis(400)).iter().filter_map(|c| c["cmd"].as_str().map(str::to_string)).collect()
}

/// The view with split panes on, on the fake daemon, its one tab on A
/// (with prompt `prompt` up), attached: the window, the view, A's
/// connection, the daemon's attaches and the client's events.
#[allow(clippy::type_complexity)]
fn on_a(
    cx: &mut TestAppContext,
    name: &str,
    prompt: Option<u64>,
) -> (AnyWindowHandle, Entity<AppView>, Client, Conn, mpsc::Receiver<Conn>, async_channel::Receiver<Event>) {
    on_a_as(cx, name, attached(A, prompt))
}

/// [`on_a`] with A's `Attached` as given (a secret prompt up, say).
#[allow(clippy::type_complexity)]
fn on_a_as(
    cx: &mut TestAppContext,
    name: &str,
    attached_a: Value,
) -> (AnyWindowHandle, Entity<AppView>, Client, Conn, mpsc::Receiver<Conn>, async_channel::Receiver<Event>) {
    on_a_in(cx, name, attached_a, (1280., 800.))
}

/// [`on_a_as`] in a window `w`×`h`.
#[allow(clippy::type_complexity)]
fn on_a_in(
    cx: &mut TestAppContext,
    name: &str,
    attached_a: Value,
    (w, h): (f32, f32),
) -> (AnyWindowHandle, Entity<AppView>, Client, Conn, mpsc::Receiver<Conn>, async_channel::Receiver<Event>) {
    let (window, view) = open_sized(cx, w, h);
    let (path, attaches) = start(name);
    let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::desktop())).expect("a client thread");
    until(&events, "connected", |e| matches!(e, Event::Status(synaps_client::Status::Connected { .. })));
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        v.client = Some(client.clone());
        v.model.tabs.open(A, "a", crate::tabs::Place::Current);
        v.reconcile(None);
        cx.notify();
    });
    let mut a = crate::test_waits::recv(&attaches).expect("A's attach at the daemon");
    a.send(attached_a);
    feed(cx, window, &view, &events, "A attached", |v| v.model.lives[v.panes.cur.live].is_active(A));
    (window, view, client, a, attaches, events)
}

/// split.md A1 and A11: Ctrl+\ gives a second pane on the right, on the
/// same session (the same store: no second attachment), focused, its
/// composer empty, its list built and following the tail. Ctrl+W on its
/// one tab closes the pane: one pane again, focus back on the first, and
/// A stays attached (the store is the other pane's too).
#[gpui_kit::test]
fn ctrl_backslash_splits_beside_and_the_last_tab_closes_the_pane(cx: &mut TestAppContext) {
    let (window, view, client, mut a, attaches, _events) = on_a(cx, "panes-split", None);
    let first = view.read_with(cx, |v, _| (v.panes.cur.id, v.panes.cur.live, v.panes.cur.disp.len()));
    key(cx, window, "ctrl-\\");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 2, "two panes");
        assert_ne!(v.panes.focused, first.0, "the new one is focused");
        assert_eq!(v.panes.cur.id, v.panes.focused);
        assert_eq!(v.panes.cur.live, first.1, "the same store: one attachment, two lists");
        assert_eq!(v.model.tabs.current_session(), Some(A), "a tab of A");
        assert_eq!(v.model.tabs.len(), 1);
        assert!(v.panes.cur.list.is_following_tail(), "following the tail");
        assert_eq!(v.panes.cur.disp.len(), first.2, "its list is A's");
        let rects: Vec<_> = v.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w: 1000., h: 600. });
        assert!(rects[0].1.x < rects[1].1.x && rects[0].1.y == rects[1].1.y, "the new pane is on the right: {rects:?}");
    });
    assert!(view.read_with(cx, |v, cx| v.panes.cur.composer.read(cx).value().is_empty()), "an empty composer");
    assert!(attaches.recv_timeout(Duration::from_millis(300)).is_err(), "no second attachment");
    // Ctrl+W in the new pane: it closes, focus goes left.
    key(cx, window, "ctrl-w");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 1, "one pane");
        assert_eq!(v.panes.focused, first.0);
        assert_eq!(v.panes.cur.id, first.0);
        assert_eq!(v.model.tabs.current_session(), Some(A), "the first pane's tab is untouched");
        assert!(v.model.lives[v.panes.cur.live].is_active(A));
    });
    let on_a = names(&mut a);
    assert!(!on_a.iter().any(|c| c == "detach"), "A's attachment stays: {on_a:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split/architecture.md §6.4 (the highest product risk), through the
/// window's input (Shady #36 M3: keys and clicks, never the methods) with
/// two panes on two sessions, each with its prompt #1 up: a message typed
/// and sent with Enter, a click on Allow and Esc Esc (HALT) go out on the
/// focused pane's own attachment, never the other's, in both directions.
/// A click on the unfocused pane's Allow focuses it and answers at once
/// (split.md §7), and typing follows it there; Alt+1 moves the focus and
/// the keys. B's attach goes beside A's (A's connection gets no detach),
/// and the pane on B closing detaches B only.
#[gpui_kit::test]
fn input_goes_only_to_the_focused_panes_own_attachment(cx: &mut TestAppContext) {
    let (window, view, client, mut a, attaches, events) = on_a(cx, "panes-two", Some(1));
    key(cx, window, "ctrl-\\");
    // The second pane opens B (as a rail click does): beside A, not
    // replacing it.
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon, on its own connection");
    b.send(attached(B, Some(1)));
    feed(cx, window, &view, &events, "B attached in the second pane, its card armed", |v| {
        v.model.lives[v.panes.cur.live].is_active(B) && v.panes.cur.card.as_ref().is_some_and(|c| c.armed())
    });
    let (pa, pb) = view.read_with(cx, |v, _| {
        let pa = v.panes.rest[0].id;
        assert!(v.model.lives[v.panes.rest[0].live].is_active(A), "the first pane still on A");
        assert_ne!(v.panes.rest[0].live, v.panes.cur.live, "two stores");
        assert_eq!(v.panes.rest[0].card_keys.get(), None, "the unfocused pane's card keys are never live");
        (pa, v.panes.cur.id)
    });
    assert!(names(&mut a).is_empty(), "A's attachment untouched by B's attach");
    // #31 M2: a worker cancel (a tray row, a menu) for A's session from the
    // pane on B is refused: it would go out on B's attachment.
    view.read_with(cx, |v, _| {
        assert_eq!(v.may_drive(B), Ok(()));
        assert_eq!(v.may_drive(A), Err("its session is in another pane: focus that pane first"));
    });
    // B's pane has the keys (the split's composer): a message, Enter.
    assert!(keys_in(cx, window, &view, pb, false), "the new pane's composer has the keys");
    type_enter(cx, window, "hello B");
    assert_eq!(names(&mut b), ["submit"], "the message on B's attachment");
    assert!(names(&mut a).is_empty(), "nothing on A's");
    // A click on A's Allow, in the unfocused pane: it focuses that pane and
    // answers A's prompt, at once.
    let allow = in_pane_at(cx, window, "allow-fill", pa);
    click_at(cx, window, allow);
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!((v.panes.focused, v.panes.cur.id), (pa, pa), "the click focused the first pane"));
    assert_eq!(names(&mut a), ["answer"], "Allow on A's attachment");
    assert!(names(&mut b).is_empty(), "nothing on B's");
    // The click gave A's card the keys (the root: its own target, never
    // overridden), so B's composer has none.
    assert!(root_has_keys(cx, window, &view), "the card the click hit has the keys");
    assert!(!keys_in(cx, window, &view, pb, false), "B's composer gave up the keys");
    // A click in A's composer, then a message: A's session.
    let at = composer_at(cx, window, pa);
    click_at(cx, window, at);
    frame(cx, window);
    assert!(keys_in(cx, window, &view, pa, false), "A's composer has the keys after a click in it");
    type_enter(cx, window, "hello A");
    assert_eq!(names(&mut a), ["submit"], "the message on A's attachment");
    assert!(names(&mut b).is_empty(), "nothing on B's");
    // Esc Esc in A's composer, A's turn running and its card answered: HALT.
    a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t", "event": {"ev": "prompt_resolved", "prompt_id": 1}}));
    a.send(json!({"type": "event", "session_id": A, "seq": 2, "ts": "t", "event": {"ev": "turn_started", "turn_baseline": 0, "trigger": "user"}}));
    feed(cx, window, &view, &events, "A's card gone, its turn running", |v| v.panes.cur.card.is_none() && v.halt_shown());
    // (A key let go and pressed again within `FoldKeys::PAIR` is a repeat.)
    key(cx, window, "escape");
    press(cx, window, "escape");
    assert_eq!(names(&mut a), ["cancel"], "HALT on A's attachment");
    assert!(names(&mut b).is_empty(), "nothing on B's");
    // Alt+2: the second pane, its keys; B's Allow by a click there.
    key(cx, window, "alt-2");
    view.read_with(cx, |v, _| assert_eq!((v.panes.focused, v.panes.cur.id), (pb, pb), "Alt+2 focused the second pane"));
    assert!(keys_in(cx, window, &view, pb, false), "B's composer has the keys after Alt+2");
    type_enter(cx, window, "hello again B");
    let allow = in_pane_at(cx, window, "allow-fill", pb);
    click_at(cx, window, allow);
    assert_eq!(names(&mut b), ["submit", "answer"], "the message and Allow on B's attachment");
    assert!(names(&mut a).is_empty(), "nothing on A's");
    // Its last tab closed: the pane goes, B is detached, A is not.
    key(cx, window, "ctrl-w");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 1);
        assert_eq!(v.panes.focused, pa);
        assert!(v.model.lives[v.panes.cur.live].is_active(A));
    });
    assert!(names(&mut b).iter().any(|c| c == "detach"), "B detached with its pane");
    assert!(!names(&mut a).iter().any(|c| c == "detach"), "A stays");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §1, §6: the same session in two panes is one store and one
/// card per pane; F6 cycles the focus, and only the focused pane's card
/// has live keys; its Allow answers once, on the one attachment, and the
/// answer collapses both panes' cards as it lands.
#[gpui_kit::test]
fn the_same_session_in_two_panes_answers_once(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, events) = on_a(cx, "panes-twins", Some(1));
    key(cx, window, "ctrl-\\");
    feed(cx, window, &view, &events, "the card up in the new pane, armed", |v| v.panes.cur.card.as_ref().is_some_and(|c| c.armed()));
    let (p0, p1) = view.read_with(cx, |v, _| {
        assert!(v.panes.rest[0].card.is_some(), "the card is in both panes");
        assert_eq!(v.panes.rest[0].card_keys.get(), None, "the unfocused one's keys are not live");
        (v.panes.rest[0].id, v.panes.cur.id)
    });
    key(cx, window, "f6");
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, p0, "F6: the next pane, round"));
    key(cx, window, "shift-f6");
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, p1, "Shift+F6: back"));
    // A click on the other pane's Allow (Shady #36 M3: the input, not the
    // method): that pane focused, one answer, on the one attachment.
    let allow = in_pane_at(cx, window, "allow-fill", p0);
    click_at(cx, window, allow);
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, p0, "the click focused the pane it hit"));
    let allow = in_pane_at(cx, window, "allow-fill", p1);
    click_at(cx, window, allow);
    assert_eq!(names(&mut a), ["answer"], "one answer, on the one attachment");
    a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t", "event": {"ev": "prompt_resolved", "prompt_id": 1}}));
    feed(cx, window, &view, &events, "the answer landed in both panes", |v| {
        v.model.lives[v.panes.cur.live].card_key().is_none() && v.panes.cur.card.is_none() && v.panes.rest[0].card.is_none()
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md A18: four panes at rest ask for no frame; only the rail's
/// spinners keep their beat (the demo's running sessions).
#[gpui_kit::test]
fn four_panes_at_rest_draw_nothing(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    // Active, as a window in use is: GPUI tells an input its blur only then.
    let _ = cx.update_window(window, |_, window, _| window.activate_window());
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(crate::motion::Level::Full), lock: None });
        crate::motion::Level::apply(cx);
    });
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        let other = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::Current);
        cx.notify();
    });
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
    key(cx, window, "ctrl-\\");
    key(cx, window, "ctrl-shift-\\");
    // Alt+1 and a split before a frame is drawn: the focus goes through the
    // first pane's composer to the new one's without a frame between.
    let _ = cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.focus_nth(0, cx);
            v.split_pane(Some(crate::split::Side::Down), window, cx);
        })
    });
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "2×2 at 1280×800");
    // A fifth is refused, with the reason on the composer's line.
    key(cx, window, "ctrl-\\");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 4);
        assert_eq!(v.panes.cur.watch_note.as_deref(), Some("4 panes is the most"));
    });
    // The keys on the window's root, as at launch: a focused composer's
    // caret blinks (a frame every 500 ms, as on main), a left one's never
    // does, however fast the focus went through the panes' composers.
    let _ = cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| window.focus(&v.focus, cx)));
    for _ in 0..240 {
        let ran = idle_step(cx, window, &view, Duration::from_micros(6944));
        if ran == 0 && !view.read_with(cx, |v, _| v.fx.pending().0) {
            break;
        }
    }
    let (frame, _) = view.read_with(cx, |v, _| v.fx.pending());
    assert!(!frame, "four panes at rest: a display frame");
    // Nothing wakes on a timer: 2 s at rest, no render (a composer left
    // with its caret blinking would draw every 500 ms). The root's focus
    // drawn first: the blur repaints the caret away, the scrollbars'
    // linger runs out (`scrollbar::LINGER`, then their fade).
    for _ in 0..60 {
        idle_step(cx, window, &view, Duration::from_millis(50));
    }
    let f0 = view.read_with(cx, |v, _| v.fx.frames());
    for i in 0..40 {
        idle_step(cx, window, &view, Duration::from_millis(50));
        assert_eq!(view.read_with(cx, |v, _| v.fx.frames()), f0, "at rest, {} ms on: a render", (i + 1) * 50);
    }
    let (sites, rail) = view.read_with(cx, |v, _| (v.fx.sites(), v.rail_width));
    assert!(sites.iter().all(|s| matches!(s.beat, crate::motion::Beat::Spin(_)) && f32::from(s.origin.x) < rail), "{sites:?}");
    // Each pane drew its own column, every one IDLE_CPU's list.
    view.read_with(cx, |v, _| {
        let n = v.panes.cur.disp.len();
        assert!(n > 0);
        assert!(v.panes.rest.iter().all(|p| p.disp.len() == n), "every pane's list built");
        let lives: std::collections::HashSet<_> = v.panes.iter().map(|p| p.live).collect();
        assert_eq!(lives.len(), 1, "one store for the one session");
    });
}

/// #31 (per-pane worker focus): two panes on one session keep their own
/// worker view. A worker opened in one is not the other's, whose Enter
/// messages the session (never steers that worker); back in the first,
/// its view is as it was.
#[gpui_kit::test]
fn two_panes_on_one_session_keep_their_own_worker_view(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, _events) = on_a(cx, "panes-views", None);
    key(cx, window, "ctrl-\\");
    let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        v.model.lives[live].watch.open(&crate::watch::Candidate::of(2, "sleeper"), None, std::time::Instant::now());
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.model.lives[v.panes.cur.live].watch.focus(), Some("sa_2"), "the second pane on the worker"));
    key(cx, window, "alt-1");
    view.update(cx, |v, _| {
        assert_eq!(v.panes.focused, p0);
        assert_eq!(v.model.lives[v.panes.cur.live].watch.focus(), None, "the first pane on the thread");
        assert!(v.enter("to the session".into()));
        assert_eq!(v.submits.last(), Some(&("to the session".to_string(), false)), "a message, not a steer");
    });
    // (The worker's view polls it: `query`.)
    let sent: Vec<String> = names(&mut a).into_iter().filter(|c| c != "query").collect();
    assert_eq!(sent, ["submit"], "a message on the session, no steer");
    key(cx, window, "alt-2");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.focused, p1);
        assert_eq!(v.model.lives[v.panes.cur.live].watch.focus(), Some("sa_2"), "the second pane's view kept");
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── Round 2 (Shady #36): input-level ────────────────────────────────────

/// A frame drawn (its top runs what was deferred to it: the keys' move).
fn frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// `text` typed into what has the keys, as the platform's text input,
/// then Enter.
fn type_enter(cx: &mut TestAppContext, window: AnyWindowHandle, text: &str) {
    VisualTestContext::from_window(window, cx).simulate_input(text);
    frame(cx, window);
    key(cx, window, "enter");
}

/// The pointer moved to `position` (as a real one always is before its
/// click), then a mouse-down and up there.
fn click_at(cx: &mut TestAppContext, window: AnyWindowHandle, position: Point<Pixels>) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    let mut vcx = VisualTestContext::from_window(window, cx);
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
}

/// The centre of `sel`, a part of the card, as pane `id` draws it. A
/// debug selector names the last pane drawn; with split panes each card
/// is named by its pane (`card N`), so the part's place in that card is
/// moved to pane `id`'s card (both cards are laid out alike).
fn in_pane_at(cx: &mut TestAppContext, window: AnyWindowHandle, sel: &'static str, id: crate::pane::PaneId) -> Point<Pixels> {
    let mut vcx = VisualTestContext::from_window(window, cx);
    // Allow is drawn filled or outlined (the theme's style).
    let alt = if sel == "allow-fill" { "allow-outline" } else { sel };
    let b = vcx.debug_bounds(sel).or_else(|| vcx.debug_bounds(alt)).unwrap_or_else(|| panic!("{sel} drawn"));
    let from =
        (0..4).find(|j| vcx.debug_bounds(format!("pane {j}").leak()).is_some_and(|p| p.contains(&b.center()))).expect("drawn in a pane");
    let card =
        |vcx: &mut VisualTestContext, j: usize| vcx.debug_bounds(format!("card {j}").leak()).unwrap_or_else(|| panic!("pane {j}'s card"));
    let (c_from, c_to) = (card(&mut vcx, from), card(&mut vcx, id.index()));
    b.center() - c_from.origin + c_to.origin
}

/// A point in pane `id`'s composer: the focused pane's is found by its
/// lit bar, and every pane's composer sits alike at its pane's foot.
fn composer_at(cx: &mut TestAppContext, window: AnyWindowHandle, id: crate::pane::PaneId) -> Point<Pixels> {
    let mut vcx = VisualTestContext::from_window(window, cx);
    let bar = vcx.debug_bounds("composer-bar").expect("the focused composer's bar");
    let at = gpui_kit::point(bar.origin.x + px(80.), bar.center().y);
    let from =
        (0..4).filter_map(|j| vcx.debug_bounds(format!("pane {j}").leak())).find(|p| p.contains(&bar.center())).expect("drawn in a pane");
    let to = vcx.debug_bounds(format!("pane {}", id.index()).leak()).expect("the pane drawn");
    at - from.origin + to.origin
}

/// Whether the window's root has the keys (a card's keys).
fn root_has_keys(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) -> bool {
    cx.update_window(window, |_, w, cx| view.read(cx).focus.is_focused(w)).expect("the window")
}

/// Whether pane `id`'s composer (`secret`: its card's secret field) has
/// the keys.
fn keys_in(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>, id: crate::pane::PaneId, secret: bool) -> bool {
    cx.update_window(window, |_, w, cx| {
        let p = view.read(cx).panes.get(id).expect("the pane");
        let h = if secret { p.secret_input.read(cx).focus_handle(cx) } else { p.composer.read(cx).focus_handle(cx) };
        h.is_focused(w)
    })
    .expect("the window")
}

/// A click outside the panes and their inputs (the right panel's foot),
/// as a rail click's mouse-down is: a focused composer lets go of the keys.
fn click_outside(cx: &mut TestAppContext, window: AnyWindowHandle) {
    click_at(cx, window, gpui_kit::point(px(1180.), px(760.)));
}

/// The messages (`submit`) among a connection's next commands.
fn submits(c: &mut Conn) -> usize {
    names(c).iter().filter(|n| *n == "submit").count()
}

/// Shady #36 H1: one session in two panes, each on its own worker, of
/// different lengths. A page into the focused pane's (longer) worker is
/// that pane's list's only: never pended into the other pane's list,
/// whose start it overran (a panic in `sync_list`).
#[gpui_kit::test]
fn two_panes_on_one_session_each_on_its_own_worker(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-two-workers", None);
    key(cx, window, "ctrl-\\");
    let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let open = |cx: &mut TestAppContext, id: u64, n: usize| {
        view.update(cx, |v, cx| {
            let live = v.panes.cur.live;
            let watch = &mut v.model.lives[live].watch;
            watch.open(&crate::watch::Candidate::of(id, "w"), None, Instant::now());
            let w = watch.worker_mut(&format!("sa_{id}")).expect("its view");
            w.transcript.apply_history(&synaps_client::synthetic::history(n, id));
            cx.notify();
        });
        frame(cx, window);
    };
    // The second pane on a short worker, the first on a long one.
    open(cx, 3, 2);
    key(cx, window, "alt-1");
    open(cx, 2, 40);
    // Pages into the long one.
    for i in 0..3 {
        view.update(cx, |v, cx| {
            let live = v.panes.cur.live;
            let ev: synaps_client::protocol::SessionEvent =
                serde_json::from_value(json!({"ev": "system_notice", "text": format!("page {i}")})).expect("an event");
            v.model.lives[live].watch.worker_mut("sa_2").expect("the long view").transcript.apply(&ev);
            cx.notify();
        });
        frame(cx, window);
    }
    let len = |v: &AppView, h: &str| v.model.lives[v.panes.cur.live].watch.worker(h).map_or(0, |w| w.transcript.items().len());
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.cur.id, p0);
        assert!(len(v, "sa_2") > len(v, "sa_3") + 10, "the long worker is longer: {} / {}", len(v, "sa_2"), len(v, "sa_3"));
        assert_eq!(v.panes.cur.rows.len(), len(v, "sa_2"), "the first pane's list is its worker's");
        assert_eq!(v.panes.rest[0].rows.len(), len(v, "sa_3"), "the second pane's list is its own worker's");
    });
    key(cx, window, "alt-2");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.cur.id, p1);
        assert_eq!(v.model.lives[v.panes.cur.live].watch.focus(), Some("sa_3"), "the second pane's view kept");
        assert_eq!(v.panes.cur.rows.len(), len(v, "sa_3"), "its list its worker's");
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// `Attached` for `sid` with secret prompt #`id` up.
fn attached_secret(sid: &str, id: u64) -> Value {
    json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": sid, "model": "m", "lifecycle": "live"},
        "conversation": conversation(),
        "pending_prompts": [{"id": id, "kind": "secret", "title": "Password", "prompt": "The vault's password"}]})
}

/// Shady #36 H2: the keys in one pane's composer, a click on the other
/// pane's secret field. The click's own focus stands: the field keeps the
/// keys after the next frame (no deferred move into that pane's plain
/// composer), and what is typed there is the answer, never a message.
#[gpui_kit::test]
fn a_click_on_the_other_panes_secret_field_keeps_the_keys(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, events) = on_a_as(cx, "panes-secret", attached_secret(A, 1));
    key(cx, window, "ctrl-\\");
    // Both armed: the new pane's card is new and waits its own 500 ms (the
    // first pane's keeps its arm: `a_split_or_a_yield_leaves_…`).
    feed(cx, window, &view, &events, "the card up in both panes, armed", |v| {
        v.panes.cur.card.as_ref().is_some_and(|c| c.armed()) && v.panes.rest[0].card.as_ref().is_some_and(|c| c.armed())
    });
    let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    assert!(keys_in(cx, window, &view, p1, false), "the new pane's composer has the keys");
    let field = in_pane_at(cx, window, "card-secret", p0);
    click_at(cx, window, field);
    frame(cx, window);
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, p0, "the click focused the first pane"));
    assert!(keys_in(cx, window, &view, p0, true), "the secret field the click focused keeps the keys");
    assert!(!keys_in(cx, window, &view, p0, false), "never its plain composer");
    type_enter(cx, window, "hunter2");
    let sent = names(&mut a);
    assert!(!sent.iter().any(|c| c == "submit"), "the secret never went out as a message: {sent:?}");
    assert!(view.read_with(cx, |v, cx| v.panes.cur.composer.read(cx).value().is_empty()), "nothing typed into the composer");
    assert_eq!(sent, ["answer"], "the secret went as the prompt's answer");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #44 L8: a split never re-arms the other pane's card, nor does a
/// yield of a side panel under the split. On screen in the first pane, its
/// arm stays the one it was shown with through Ctrl+\ as it comes, a
/// second split once armed, and the window narrowing until the right panel
/// and the rail yield and widening back: its list is laid out anew in each
/// new column, and nothing takes it off screen. It watches the buttons'
/// arm (`Arm::shown_at`, `armed`), not `Arm::keys`: the keys' own wait
/// when the focus comes back to a card is intended (Shady #48 n7).
#[gpui_kit::test]
fn a_split_or_a_yield_leaves_the_other_panes_card_armed(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, events) = on_a_as(cx, "panes-l8", attached_secret(A, 1));
    feed(cx, window, &view, &events, "the card up", |v| v.panes.cur.card.is_some());
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let card = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.panes.get(p0).and_then(|p| p.card.as_ref()).map(|c| (c.key, c.arm.shown_at(), c.armed())))
    };
    let before = card(cx).expect("the card");
    let mut yields = std::collections::BTreeSet::new();
    let mut check = |cx: &mut TestAppContext, what: &str, armed: bool| {
        for f in 0..40 {
            frame(cx, window);
            std::thread::sleep(Duration::from_millis(2));
            let now = card(cx).expect("the first pane's card");
            assert_eq!((now.0, now.1), (before.0, before.1), "{what}, frame {f}: the first pane's card waits again");
            assert!(!armed || now.2, "{what}, frame {f}: armed");
            let y = view.read_with(cx, |v, _| (v.pane_yield, v.panes.folded));
            assert!(!y.1, "{what}: never folded (that hides the first pane)");
            yields.insert(y.0);
        }
    };
    key(cx, window, "ctrl-\\");
    check(cx, "split as it comes", false);
    feed(cx, window, &view, &events, "armed", |v| v.panes.get(p0).and_then(|p| p.card.as_ref()).is_some_and(|c| c.armed()));
    key(cx, window, "ctrl-\\");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 3, "split again");
    check(cx, "split once armed", true);
    for w in [1180., 1040., 980., 1280.] {
        VisualTestContext::from_window(window, cx).simulate_resize(size(px(w), px(800.)));
        check(cx, &format!("the window {w} wide"), true);
    }
    assert!(yields.len() >= 2, "a side panel yielded and came back: {yields:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 M1: the focus mark and the keys never come apart. With the
/// keys in the second pane's composer (on B), each way of focusing
/// another pane without a click (a rail click or a find, an Open, the
/// Settings key reopening Settings where another pane holds it, Alt+N
/// onto a pane showing Settings) moves the keys too: what is typed then
/// goes to the marked pane's session, or (Settings) to no session.
#[gpui_kit::test]
fn typing_after_a_focus_change_goes_to_the_marked_pane(cx: &mut TestAppContext) {
    let (window, view, client, mut a, attaches, events) = on_a(cx, "panes-marked", None);
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let (pa, pb) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let _ = (names(&mut a), names(&mut b));
    assert!(keys_in(cx, window, &view, pb, false), "B's composer has the keys");
    // A rail click on A's row: its mouse-down outside the panes (the
    // composer lets go of the keys, as on screen), then `AppView::rail_click`
    // with the row's action (a find's Enter takes the same way,
    // `Model::find_enter_in`).
    click_outside(cx, window);
    assert!(!keys_in(cx, window, &view, pb, false), "the click outside took the keys from B's composer");
    view.update(cx, |v, cx| {
        for fx in v.model.open_row_in(&crate::named::RowAction::Attach(A.into()), crate::tabs::Place::Current) {
            v.effect(fx);
        }
        cx.notify();
    });
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, pa, "the rail click focused A's pane"));
    type_enter(cx, window, "after the rail click");
    assert_eq!((submits(&mut a), submits(&mut b)), (1, 0), "after a rail click: on A's session");
    // Back to B by Alt+2, then an Open of A.
    key(cx, window, "alt-2");
    type_enter(cx, window, "on B");
    assert_eq!((submits(&mut a), submits(&mut b)), (0, 1), "after Alt+2: on B's session");
    click_outside(cx, window);
    view.update(cx, |v, cx| {
        v.effect(crate::model::Effect::Attach { session_id: A.into(), fallback: None, open: true, place: crate::tabs::Place::Current });
        cx.notify();
    });
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, pa, "the Open focused A's pane"));
    type_enter(cx, window, "after the open");
    assert_eq!((submits(&mut a), submits(&mut b)), (1, 0), "after an Open: on A's session");
    // Settings in the second pane; from the first, its key again.
    let settings = crate::views::view_keys(cfg!(target_os = "macos"), cfg!(target_os = "linux")).0;
    key(cx, window, "alt-2");
    key(cx, window, settings);
    view.read_with(cx, |v, _| assert!(v.model.tabs.view().is_some(), "Settings in the second pane"));
    key(cx, window, "alt-1");
    type_enter(cx, window, "on A");
    assert_eq!((submits(&mut a), submits(&mut b)), (1, 0), "after Alt+1: on A's session");
    key(cx, window, settings);
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, pb, "the Settings key focused the pane holding Settings"));
    assert!(!keys_in(cx, window, &view, pa, false), "A's composer gave up the keys");
    type_enter(cx, window, "into Settings");
    assert_eq!((submits(&mut a), submits(&mut b)), (0, 0), "with Settings marked: no session's");
    // Alt+1, then Alt+2 onto the pane showing Settings.
    key(cx, window, "alt-1");
    key(cx, window, "alt-2");
    assert!(!keys_in(cx, window, &view, pa, false), "Alt+2 onto Settings: A's composer gave up the keys");
    type_enter(cx, window, "into Settings again");
    assert_eq!((submits(&mut a), submits(&mut b)), (0, 0), "Alt+N onto Settings: no session's");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 M2: a worker's view in the unfocused pane stays when that
/// worker ends, as the focused pane's does (its final output is what the
/// pane is watching for).
#[gpui_kit::test]
fn an_unfocused_panes_worker_view_stays_when_its_worker_ends(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-worker-ends", None);
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        v.model.lives[live].watch.open(&crate::watch::Candidate::of(2, "sleeper"), None, Instant::now());
        cx.notify();
    });
    frame(cx, window);
    key(cx, window, "alt-1");
    // It ends; the first pane's tray syncs (and prunes what nobody watches).
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        v.model.lives[live].watch.worker_mut("sa_2").expect("its view").finished_at = Some(Instant::now());
        v.tray.key = None;
        cx.notify();
    });
    frame(cx, window);
    frame(cx, window);
    key(cx, window, "alt-2");
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.lives[v.panes.cur.live].watch.focus(), Some("sa_2"), "the second pane's view of the ended worker stayed");
        assert!(v.panes.cur.column_of.is_some(), "its list still the worker's");
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 L1: with split panes off, the pane keys are not keys at all
/// (main's window): Alt+1 (Ctrl+1 on macOS) in the palette's field leaves
/// the keys there.
#[gpui_kit::test]
fn with_split_panes_off_alt_1_is_not_a_pane_key(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    // Off, whatever the tests' lane starts with (`split-on-tests`).
    view.update(cx, |v, _| v.app_settings.split = false);
    assert!(!view.read_with(cx, |v, _| v.split_on()), "split panes off");
    key(cx, window, crate::app::palette_keys(cfg!(target_os = "macos")).1);
    frame(cx, window);
    let in_palette = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, w, cx| view.read(cx).palette_input.read(cx).focus_handle(cx).is_focused(w)).unwrap()
    };
    assert!(in_palette(cx), "the palette's field has the keys");
    let nth = if cfg!(target_os = "macos") { "ctrl-1" } else { "alt-1" };
    key(cx, window, nth);
    frame(cx, window);
    assert!(in_palette(cx), "{nth} with split panes off: the palette's field keeps the keys");
}

/// Shady #36 L2 (#31 M4.1): another pane's store attaching leaves the
/// focused pane's open menu open.
#[gpui_kit::test]
fn another_panes_attach_leaves_the_open_menu(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "panes-menu", None);
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    key(cx, window, "alt-1");
    view.update(cx, |v, cx| {
        // A worker's menu, opened with the mouse (it stays until closed).
        let at = crate::menu::At::Point(gpui_kit::point(px(200.), px(200.)));
        let target = crate::wtabs::Target { session: A.into(), who: crate::wtabs::Who::new(2, "w", None), tab: None };
        v.menu = Some((crate::menu::Menu::new(vec![crate::menu::Item::new(crate::wtabs::Act::Open, "Open")], at, false), target));
        cx.notify();
    });
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.menu.is_some()), "the menu open");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the other pane", |v| v.panes.rest.iter().any(|p| v.model.lives[p.live].is_active(B)));
    assert!(view.read_with(cx, |v, _| v.menu.is_some()), "the first pane's menu stayed open");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 L3a: Ctrl+W closes the pane whose composer had the keys:
/// the pane focused after it has them (never nothing, where `/` opens
/// find).
#[gpui_kit::test]
fn closing_the_focused_pane_gives_the_keys_to_the_next(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-close-keys", None);
    key(cx, window, "ctrl-\\");
    let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    assert!(keys_in(cx, window, &view, p1, false), "the new pane's composer has the keys");
    key(cx, window, "ctrl-w");
    frame(cx, window);
    view.read_with(cx, |v, _| assert_eq!((v.panes.len(), v.panes.focused), (1, p0)));
    assert!(keys_in(cx, window, &view, p0, false), "the pane left has the keys");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 L3b: a pane closed by an event (its last tab's session went,
/// while the other pane is in use) leaves the focus and the keys where
/// they are.
#[gpui_kit::test]
fn a_pane_an_event_closes_leaves_the_focus(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-close-quiet", None);
    key(cx, window, "ctrl-\\");
    key(cx, window, "ctrl-\\");
    let (first, focused) = view.read_with(cx, |v, _| (v.reading_order()[0], v.panes.focused));
    assert_ne!(first, focused, "a pane other than the focused one");
    assert!(keys_in(cx, window, &view, focused, false), "the focused pane's composer has the keys");
    view.update(cx, |v, cx| {
        v.in_pane(first, |v| v.close_pane());
        cx.notify();
    });
    frame(cx, window);
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 2);
        assert_eq!((v.panes.focused, v.panes.cur.id), (focused, focused), "the focus stayed");
    });
    assert!(keys_in(cx, window, &view, focused, false), "and the keys");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── Round 3 (Shady #36 r2): #33's label hang, per pane ──────────────────

/// One 144 Hz frame on the test's clock, drawn (`test_kit`'s
/// `fluid_frame`). Timer by timer, the clock at each one's deadline (#52:
/// `ui_tests`' `fluid_frame`).
fn frame_144(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) {
    let dt = Duration::from_micros(6944);
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
    view.update(cx, |_, cx| cx.notify());
    cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
}

/// One 144 Hz frame on the test's clock, as [`frame_144`], drawn only if
/// the app asked for one (a frame, or a timer that came due): a frame
/// drawn is one the app drew.
fn asked_frame(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) {
    let dt = Duration::from_micros(6944);
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
    cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
}

/// `dt` on the test's clock, timer by timer with the motion clock at each
/// deadline (as [`frame_144`]; Shady #58 n2), then the frame the app asked
/// for, if it asked: how many frame callbacks ran.
fn idle_step(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>, dt: Duration) -> usize {
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
    let ran = cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
    ran
}

/// From now on, `shot` of the view at each frame drawn, in order (#60
/// L1: a [`frame_144`] can draw two frames, a motion timer's and its own,
/// so a step's view after it is not every frame's).
fn draws<T: 'static>(cx: &mut TestAppContext, view: &Entity<AppView>, shot: impl Fn(&AppView) -> T + 'static) -> Rc<RefCell<Vec<T>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    view.update(cx, |v, _| v.on_draw = Some(Rc::new(move |v: &AppView| l.borrow_mut().push(shot(v)))));
    log
}

/// A label as one frame drew it: its pane's width, its left from the
/// pane's edge and top from its row's, and the pane's column.
type Placed = (f32, (f32, f32), f32);

/// A labelled row whole in pane `id`'s view and in no other pane's (a
/// debug selector names the last pane drawn: this row's `label N` is this
/// pane's), not its top row (its label may be pinned).
fn own_label(view: &Entity<AppView>, cx: &mut TestAppContext, id: crate::pane::PaneId) -> u64 {
    view.read_with(cx, |v, _| {
        let pane = v.panes.get(id).expect("the pane");
        let (vb, top) = (pane.list.viewport_bounds(), pane.list.logical_scroll_top().item_ix);
        let drawn_elsewhere = |ix: usize| {
            v.panes.ids().into_iter().filter(|&o| o != id).any(|o| {
                let other = v.panes.get(o).expect("the pane");
                other.list.bounds_for_item(ix).is_some_and(|b| {
                    let ovb = other.list.viewport_bounds();
                    b.bottom() > ovb.top() - px(400.) && b.top() < ovb.bottom() + px(400.)
                })
            })
        };
        (top + 1..pane.disp.len())
            .find(|&ix| {
                pane.disp[ix].who != crate::turns::Who::None
                    && pane.list.bounds_for_item(ix).is_some_and(|b| b.top() > vb.top() + px(60.) && b.bottom() < vb.bottom() - px(60.))
                    && !drawn_elsewhere(ix)
            })
            .map(|ix| pane.disp[ix].id)
            .unwrap_or_else(|| {
                let seen: Vec<_> = v
                    .panes
                    .ids()
                    .into_iter()
                    .map(|o| {
                        let p = v.panes.get(o).expect("the pane");
                        let drawn: Vec<usize> = (0..p.disp.len()).filter(|&ix| p.list.bounds_for_item(ix).is_some()).collect();
                        (o, p.disp.len(), p.list.logical_scroll_top(), p.list.viewport_bounds(), drawn)
                    })
                    .collect();
                panic!("a labelled row of pane {id:?}'s own: {seen:?}")
            })
    })
}

/// #33 in split panes (Shady #36 r2, the blocker): each pane's labels hang
/// by its own width. Two panes side by side, the left wider; the rail's
/// flight takes the left across the hang width (872 px at this step) and
/// leaves the right under it. Every frame of the flight, both ways: the
/// left's hang is its flight's end's from the start and its labels glide
/// (`hang_k` between 0 and 1, the label moving no more than its pane's
/// step); the right's stays inline (`hang_k` 0), its rows are never
/// measured again, and its label moves no more than its own pane's step.
/// Landed, each pane's layout is a fresh one's at its width. One window-
/// wide hang (or the window's `pane_to` for every pane) hangs the right
/// pane's labels as well.
#[gpui_kit::test]
fn each_panes_labels_hang_by_its_own_width_in_a_panel_flight(cx: &mut TestAppContext) {
    let (window, view) = open_sized(cx, 1700., 900.);
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(crate::motion::Level::Full), lock: None });
        crate::motion::Level::apply(cx);
    });
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        v.panels.rail_collapsed = false;
        v.panels.right_collapsed = true;
        // Short turns after the demo's: a label every few lines, a list
        // long enough for the two panes to show apart.
        for n in 0..30 {
            let t = &mut v.model.transcript;
            let user_text = Some(format!("Question {n}: where does the refresh race start?"));
            t.apply(&SessionEvent::TurnStarted { turn_baseline: 12 + 2 * n, trigger: "user".into(), user_text });
            let text = format!("Answer {n}: in the relay's token cache, two lines down from the lock.");
            t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text }) });
            t.apply(&SessionEvent::Idle);
        }
        v.sync_list();
        cx.notify();
    });
    let settle = |cx: &mut TestAppContext, view: &Entity<AppView>| {
        for _ in 0..600 {
            if !view.read_with(cx, |v, _| v.fx.pending().0 || v.panel_fx.moving()) {
                break;
            }
            frame_144(cx, window, view);
        }
    };
    settle(cx, &view);
    key(cx, window, "ctrl-\\");
    let (left, right) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    // The left pane's share: the hang width between the centre with the
    // rail open and with it closed.
    let (m, rail_open, rail_shut) = view.read_with(cx, |v, cx| {
        let m = crate::theme::metrics(cx);
        let [rail, right] = [0, 1].map(|i| v.panel_fx.sides[i].shown);
        (m, v.width - crate::chrome::MARK - rail - right, v.width - crate::chrome::MARK - right)
    });
    let hang_w = (672. + 200.) * m.r;
    let share = hang_w / ((rail_open + rail_shut) / 2. - crate::split::DIVIDER);
    view.update(cx, |v, cx| {
        v.panes.tree = crate::split::Node::Split {
            axis: crate::split::Axis::Row,
            kids: vec![crate::split::Node::Leaf(left), crate::split::Node::Leaf(right)],
            ratios: vec![share, 1. - share],
        };
        cx.notify();
    });
    settle(cx, &view);
    // The two apart: the left on the first short turns, the right at the end.
    view.update(cx, |v, cx| {
        v.in_pane(left, |v| v.panes.cur.list.scroll_to(gpui_kit::ListOffset { item_ix: 16, offset_in_item: px(0.) }));
        v.in_pane(right, |v| v.panes.cur.list.scroll_to_end());
        cx.notify();
    });
    settle(cx, &view);
    for _ in 0..4 {
        frame(cx, window);
    }
    let width = |cx: &mut TestAppContext, id| view.read_with(cx, |v, _| v.panes.get(id).expect("the pane").rect.get().w);
    let lay = |cx: &mut TestAppContext, id| view.read_with(cx, |v, _| v.panes.get(id).expect("the pane").layout);
    let measured = |cx: &mut TestAppContext, id| view.read_with(cx, |v, _| v.panes.get(id).expect("the pane").metrics_gen);
    let label = |cx: &mut TestAppContext, id: u64| {
        VisualTestContext::from_window(window, cx)
            .debug_bounds(format!("label {id}").leak())
            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
    };
    // Its pane's left edge as drawn (the rail's flight moves the centre).
    let pane_x = |cx: &mut TestAppContext, pane: crate::pane::PaneId| {
        let b = VisualTestContext::from_window(window, cx).debug_bounds(format!("pane {}", pane.index()).leak()).expect("the pane drawn");
        f32::from(b.origin.x)
    };
    let row_top = |cx: &mut TestAppContext, pane, id: u64| {
        view.read_with(cx, |v, _| {
            let p = v.panes.get(pane).expect("the pane");
            let ix = p.disp.iter().position(|d| d.id == id).expect("the row");
            p.list.bounds_for_item(ix).map(|b| f32::from(b.top())).expect("the row drawn")
        })
    };
    // The setup: the left crosses the hang width in the flight, the right
    // does not.
    let (l0, r0) = (width(cx, left), width(cx, right));
    assert!(l0 < hang_w && r0 < hang_w, "the rail open: both panes inline ({l0}, {r0} < {hang_w})");
    assert!(!lay(cx, left).hang && !lay(cx, right).hang && lay(cx, left).hang_k == 0. && lay(cx, right).hang_k == 0.);
    for close in [true, false] {
        let ids = [(left, own_label(&view, cx, left)), (right, own_label(&view, cx, right))];
        let rects = view.read_with(cx, |v, _| ids.map(|(p, _)| v.panes.get(p).expect("the pane").rect.get()));
        // Each label on its pane and its row: its left from the pane's
        // edge, its top from its row's; and its pane's width.
        let place = |cx: &mut TestAppContext, p: crate::pane::PaneId, id: u64| {
            let at = label(cx, id).expect("the label in every frame");
            (width(cx, p), (at.0 - pane_x(cx, p), at.1 - row_top(cx, p, id)), lay(cx, p).column)
        };
        let mut track: Vec<Vec<Placed>> = Vec::new();
        for (i, (p, id)) in ids.iter().enumerate() {
            let at = label(cx, *id).unwrap_or_else(|| panic!("close {close}: pane {i}'s label {id} drawn"));
            let x = pane_x(cx, *p);
            assert!(at.0 >= x && at.0 < x + rects[i].w, "close {close}: label {id} {at:?} in its pane at {x}, {:?}", rects[i]);
            track.push(vec![place(cx, *p, *id)]);
        }
        let right_gen = measured(cx, right);
        let tight = lay(cx, right).tight;
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        cx.run_until_parked();
        let (mut between, mut frames) = (0, 0);
        while view.read_with(cx, |v, _| v.fx.pending().0 || v.panel_fx.moving()) && frames < 600 {
            frame_144(cx, window, &view);
            frames += 1;
            let (ll, rl) = (lay(cx, left), lay(cx, right));
            assert_eq!(ll.hang, close, "close {close}, frame {frames}: the left pane's hang is its flight's end's from the start");
            assert!(!rl.hang && rl.hang_k == 0., "close {close}, frame {frames}: the right pane stays inline ({rl:?})");
            if ll.hang_k > 0. && ll.hang_k < 1. {
                between += 1;
            }
            for (i, (p, id)) in ids.iter().enumerate() {
                let now = place(cx, *p, *id);
                track[i].push(now);
            }
        }
        assert!(frames < 600, "close {close}: the flight lands");
        // No label jumps: in a frame, on its pane and its row, it moves its
        // way's share of the pane's step (up to `turns::lead`'s 3.52 times
        // the line's rate mid-way), the column's own move (half the step)
        // and a pixel's rounding either way. Each by its own pane. The
        // column keeps the flight's narrowest wrap until it lands, then
        // takes the pane's (fluid.md §3.8, main's cut): that one frame
        // moves the column, not the labels on it.
        for (i, frames) in track.iter().enumerate() {
            let rewraps = frames.windows(2).filter(|f| f[0].2 != f[1].2).count();
            assert!(
                rewraps <= 1,
                "close {close}: pane {i}'s column rewraps once, as it lands ({rewraps}): {:?}",
                frames.iter().map(|f| (f.0, f.2)).collect::<Vec<_>>()
            );
            let (first, end) = (frames[0], frames[frames.len() - 1]);
            let column = |f: &Placed| (f.0 - f.2) / 2.;
            let way = ((end.1.0 - column(&end)) - (first.1.0 - column(&first))).hypot(end.1.1 - first.1.1);
            let delta = (end.0 - first.0).abs().max(1.);
            for (n, pair) in frames.windows(2).enumerate() {
                let ((w0, (x0, y0), c0), (w1, (x1, y1), c1)) = (pair[0], pair[1]);
                if c0 != c1 {
                    continue;
                }
                let step = (w1 - w0).abs();
                let moved = (x1 - x0).hypot(y1 - y0);
                let bound = 3.6 * way * step / delta + step + std::f32::consts::SQRT_2 + 1e-3;
                assert!(
                    moved <= bound,
                    "close {close}, frame {}: pane {i}'s label moved {moved} px in a frame its pane moved {step} (way {way}, its pane {delta}, bound {bound}): {:?}",
                    n + 1,
                    &frames[n.saturating_sub(4)..(n + 4).min(frames.len())]
                );
            }
        }
        assert!(between >= 3, "close {close}: the left pane's labels glide ({between} frames between)");
        let (l1, r1) = (width(cx, left), width(cx, right));
        assert_eq!(close, l1 >= hang_w, "close {close}: the left pane crossed ({l0} -> {l1})");
        assert!(r1 < hang_w, "close {close}: the right pane stays under the hang width ({r1})");
        let (ll, rl) = (lay(cx, left), lay(cx, right));
        assert_eq!((ll.hang, ll.hang_k), (close, if close { 1. } else { 0. }), "close {close}: the left landed a fresh layout's");
        assert_eq!((rl.hang, rl.hang_k), (false, 0.), "close {close}: the right landed inline");
        if rl.tight == tight {
            assert_eq!(measured(cx, right), right_gen, "close {close}: the right pane's rows are not measured again for the left's hang");
        }
    }
}

// ── Round 3 (Shady #36 r2): L-b, L-c, L-d, n9 ───────────────────────────

/// Shady #36 r2 L-b (M2's reconnect half): two panes on A, a worker's view
/// in the second; the first focused, so the second's view is parked. Each
/// frame syncs both panes' trays (the last from the second pane, whose
/// view is the store's focus then: a snapshot taken there missed it). A
/// reconnect between frames keeps the parked view: the second pane is
/// still on its worker, not flipped to the thread.
#[gpui_kit::test]
fn a_reconnect_keeps_the_unfocused_panes_worker_view(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-reconnect", None);
    key(cx, window, "ctrl-\\");
    let p1 = view.read_with(cx, |v, _| v.panes.cur.id);
    view.update(cx, |v, cx| {
        let live = v.panes.cur.live;
        v.model.lives[live].watch.open(&crate::watch::Candidate::of(2, "sleeper"), None, std::time::Instant::now());
        cx.notify();
    });
    cx.run_until_parked();
    key(cx, window, "alt-1");
    for _ in 0..3 {
        frame(cx, window);
    }
    view.read_with(cx, |v, _| {
        let w = &v.model.lives[v.panes.cur.live].watch;
        assert_eq!((w.focus(), w.parked()), (None, &["sa_2".to_string()][..]), "the first pane on the thread, the second's view parked");
    });
    // A's attachment comes back (the daemon reloaded): `Attached`, a
    // reconnect, between frames.
    let seq = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].active.as_ref().expect("attached").attach_seq);
    let again = match synaps_client::protocol::decode_frame(&attached(A, None).to_string()).expect("a frame") {
        synaps_client::protocol::DaemonFrame::Attached(a) => a,
        other => panic!("{other:?}"),
    };
    view.update(cx, |v, cx| {
        let ev = Event::Attached { attach_seq: seq, attached: again, mode: synaps_client::AttachMode::Mirror, reconnect: true };
        v.apply_batch(vec![ev], cx);
    });
    key(cx, window, "alt-2");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.focused, p1);
        assert_eq!(v.model.lives[v.panes.cur.live].watch.focus(), Some("sa_2"), "the second pane's view kept over the reconnect");
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 r2 L-c (L5's close half): two panes on A; the second sends
/// a message (typed, Enter), and before the daemon confirms it the second
/// pane leaves A: its tab switched to B, or its pane closed (its last tab
/// closed: `close_tab`, where Ctrl+W ends; Ctrl+W right after a send does
/// not close a tab, split panes off too, so the key is not this test's).
/// The daemon refuses the message: it comes back to the first
/// pane's composer, the one still on A. Never the second's on B, never
/// dropped with the closed pane.
#[gpui_kit::test]
fn a_refused_message_from_a_pane_that_left_goes_back_to_the_pane_on_its_session(cx: &mut TestAppContext) {
    for close in [false, true] {
        let name = if close { "panes-unsent-close" } else { "panes-unsent-switch" };
        let (window, view, client, mut a, attaches, events) = on_a(cx, name, None);
        key(cx, window, "ctrl-\\");
        let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
        let text = "from the second pane";
        frame(cx, window);
        assert!(keys_in(cx, window, &view, p1, false), "close {close}: the second pane's composer has the keys");
        type_enter(cx, window, text);
        assert_eq!(submits(&mut a), 1, "close {close}: one message sent on A");
        view.read_with(cx, |v, _| {
            let sent: Vec<_> = v.model.lives[v.panes.cur.live].in_flight.iter().cloned().collect();
            assert_eq!((v.panes.cur.id, sent), (p1, vec![text.to_string()]), "close {close}: in flight from the second pane");
        });
        if close {
            view.update(cx, |v, cx| {
                let ix = v.model.tabs.active().expect("its tab");
                v.close_tab(ix);
                cx.notify();
            });
            frame(cx, window);
            assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "close {close}: the second pane closed");
        } else {
            view.update(cx, |v, cx| {
                v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
                cx.notify();
            });
            let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
            b.send(attached(B, None));
            feed(cx, window, &view, &events, "B attached in the second pane", |v| {
                v.panes.get(p1).is_some_and(|p| v.model.lives[p.live].is_active(B))
            });
        }
        a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t",
            "event": {"ev": "refused", "client": 2, "command": "submit", "reason": "busy"}}));
        feed(cx, window, &view, &events, "the refusal", |v| v.panes.get(p0).is_some_and(|p| v.model.lives[p.live].in_flight.is_empty()));
        for _ in 0..3 {
            frame(cx, window);
        }
        let value = |cx: &mut TestAppContext, id| {
            view.read_with(cx, |v, cx| v.panes.get(id).map(|p| (p.restore.clone(), p.composer.read(cx).value().to_string())))
        };
        let first = value(cx, p0).expect("the first pane");
        assert!(first.1 == text || first.0.as_deref() == Some(text), "close {close}: back in the first pane's composer, on A: {first:?}");
        if let Some(second) = value(cx, p1) {
            assert!(second.0.is_none() && second.1.is_empty(), "close {close}: nothing in the second pane's, on B: {second:?}");
        }
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
}

/// Shady #36 r2 L-d: a menu opened in the first pane (its composer had the
/// keys) saves them to give back. A click on the second pane's body (not
/// an input: nothing there takes the keys) closes the menu and marks the
/// second pane. The menu's restore no longer pulls the mark back: the
/// second pane stays marked, and the first pane's composer does not get
/// the keys.
#[gpui_kit::test]
fn a_click_in_another_pane_closes_the_menu_and_keeps_the_mark(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-menu-back", None);
    key(cx, window, "ctrl-\\");
    let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    key(cx, window, "alt-1");
    frame(cx, window);
    assert!(keys_in(cx, window, &view, p0, false), "the first pane's composer has the keys");
    let tab = view.read_with(cx, |v, _| v.model.tabs.iter().next().expect("a tab").id);
    cx.update_window(window, |_, w, cx| {
        view.update(cx, |v, cx| v.open_session_menu(crate::app::SessionAt::Tab(tab), gpui_kit::point(px(400.), px(120.)), w, cx))
    })
    .expect("the window");
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.smenu.is_some()), "the session menu open");
    assert!(root_has_keys(cx, window, &view), "the menu took the keys");
    // The second pane's body: its list, under its tab strip.
    let at = {
        let b = VisualTestContext::from_window(window, cx).debug_bounds(format!("pane {}", p1.index()).leak()).expect("the second pane");
        gpui_kit::point(b.center().x, b.origin.y + px(160.))
    };
    click_at(cx, window, at);
    for _ in 0..3 {
        frame(cx, window);
    }
    assert!(view.read_with(cx, |v, _| v.smenu.is_none()), "the click closed the menu");
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), p1, "the second pane stays marked");
    assert!(!keys_in(cx, window, &view, p0, false), "the first pane's composer did not get the keys back");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 r2 n9: with split panes on and one pane, the pane focus keys
/// (Alt+N, F6, Shift+F6) are not bound (main's window); the split keys
/// are. Two panes bind both; split panes off, neither.
#[test]
fn the_pane_focus_keys_need_two_panes() {
    let has = |split_on, split, ctx: &str| crate::app::root_context(false, false, split_on, split).is_some_and(|c| c.contains(ctx));
    assert!(!has(false, false, "Panes") && !has(false, false, "PaneFocus"), "split panes off: none");
    assert!(has(true, false, "Panes") && !has(true, false, "PaneFocus"), "one pane: the split keys only");
    assert!(has(true, true, "Panes") && has(true, true, "PaneFocus"), "two panes: both");
}

// ── Round 3: the design gate (cohesion/finish-gate-pr36.md) ─────────────

/// A selector's bounds as drawn: left, top, width, height.
fn drawn(cx: &mut TestAppContext, window: AnyWindowHandle, sel: String) -> Option<[f32; 4]> {
    let b = VisualTestContext::from_window(window, cx).debug_bounds(sel.leak())?;
    Some([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)])
}

/// The finish gate's M1 and M3, at 1280×800: two panes across, the right
/// one split down (the left full height, full form; the two on the right
/// under the short height). Focus moves by Alt+N and F6 to each pane in
/// turn, and the split itself: the top bar stays 52 px, every pane keeps
/// its place and size, and the panes reach the window's foot (M1: a short
/// pane focused made the bar 40 px and moved the whole window 12 px). The
/// left and bottom-right composers, side by side on the foot, end on one
/// line (M3: each pane's own form put them 10 px apart).
#[gpui_kit::test]
fn focus_never_moves_the_window_and_the_foot_composers_share_a_line(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-gate-m1", None);
    let bar = |cx: &mut TestAppContext| drawn(cx, window, "top-bar".into()).expect("the top bar")[3];
    assert_eq!(bar(cx), crate::chrome::TOP, "one pane: the full bar");
    key(cx, window, "ctrl-\\");
    key(cx, window, "ctrl-shift-\\");
    frame(cx, window);
    let ids: Vec<crate::pane::PaneId> = view.read_with(cx, |v, _| v.panes.ids());
    assert_eq!(ids.len(), 3, "three panes");
    let short = view.read_with(cx, |v, _| v.panes.cur.layout.short);
    assert!(short, "the focused pane (bottom right) is under the short height");
    let place = |cx: &mut TestAppContext| -> Vec<[f32; 4]> {
        ids.iter().map(|id| drawn(cx, window, format!("pane {}", id.index())).expect("the pane drawn")).collect()
    };
    let at_rest = place(cx);
    assert_eq!(bar(cx), crate::chrome::TOP, "the split: the bar stays 52 px");
    for (i, p) in at_rest.iter().enumerate() {
        assert!(p[1] >= crate::chrome::TOP - 0.5, "pane {i} under the bar: {p:?}");
    }
    let foot = at_rest.iter().map(|p| p[1] + p[3]).fold(0f32, f32::max);
    assert!((foot - 800.).abs() < 0.5, "the panes reach the window's foot: {at_rest:?}");
    for k in ["alt-1", "alt-2", "alt-3", "f6", "f6", "shift-f6"] {
        key(cx, window, k);
        frame(cx, window);
        assert_eq!(bar(cx), crate::chrome::TOP, "{k}: the bar stays 52 px");
        assert_eq!(place(cx), at_rest, "{k}: no pane moves");
    }
    // M3: the left pane and the bottom-right one, both on the foot.
    let (left, bottom_right) = {
        let on_foot: Vec<usize> = (0..3).filter(|&i| (at_rest[i][1] + at_rest[i][3] - 800.).abs() < 0.5).collect();
        assert_eq!(on_foot.len(), 2, "two panes on the foot: {at_rest:?}");
        let (a, b) = (on_foot[0], on_foot[1]);
        if at_rest[a][0] < at_rest[b][0] { (ids[a], ids[b]) } else { (ids[b], ids[a]) }
    };
    let forms = view.read_with(cx, |v, _| [left, bottom_right].map(|id| v.panes.get(id).expect("the pane").layout.short));
    assert_eq!(forms, [false, true], "the left pane full, the bottom-right short");
    let ends = [left, bottom_right]
        .map(|id| drawn(cx, window, format!("composer-frame {}", id.index())).map(|b| b[1] + b[3]).expect("its composer"));
    assert!((ends[0] - ends[1]).abs() < 0.5, "the foot's composers end on one line: {ends:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// What [`AppView::probe`] logged since the last read, as `name`: one box
/// a frame, in order.
fn probed(cx: &mut TestAppContext, view: &Entity<AppView>) -> impl Fn(&str) -> Vec<[f32; 4]> + use<> {
    let log = view.read_with(cx, |v, _| v.chrome_log.take());
    move |name| log.iter().filter(|(n, _)| *n == name).map(|(_, b)| *b).collect()
}

/// The bar's 1 px rule along its foot: [`AppView::probe`] reads the box
/// inside it.
const BAR_RULE: f32 = 1.;

/// Whether the bar is the short one at a window `h` tall (text 16).
fn short_at(h: f32) -> bool {
    h - crate::chrome::TOP < crate::fit::short_below(&crate::theme::Metrics::at(16))
}

/// The finish gate's R1, one pane (the flag off, main's path): a launch's
/// first frame draws the bar for its window, 52 px at 1080 (it read the
/// height 0 the view starts with: the short 40 px bar, and the next frame
/// moved everything under it 12 px). Nothing in the chrome moves after.
#[gpui_kit::test]
fn a_launch_draws_the_windows_bar_on_its_first_frame(cx: &mut TestAppContext) {
    assert!(!short_at(1080.));
    let (window, view) = open_sized(cx, 1920., 1080.);
    frame(cx, window);
    let of = probed(cx, &view);
    let (bar, body) = (of("top-bar"), of("body"));
    assert!(!bar.is_empty() && bar.len() == body.len(), "frames drawn: {bar:?} / {body:?}");
    assert_eq!(bar[0], [0., 0., 1920., crate::chrome::TOP - BAR_RULE], "the first frame: the full bar at 1080: {bar:?}");
    assert_eq!(body[0][1..], [crate::chrome::TOP, body[0][2], 1080. - crate::chrome::TOP], "the first frame: the rest under it: {body:?}");
    assert!(bar.iter().all(|b| *b == bar[0]), "the bar never moves: {bar:?}");
    assert!(body.iter().all(|b| *b == body[0]), "nothing under it moves: {body:?}");
}

/// The finish gate's R1, one pane: a resize across the bar's threshold
/// (700 px at text 16) draws the new bar on its own frame (it drew the
/// old one, from the last frame's height, until something else drew),
/// down and back up; the frames after it are the same.
#[gpui_kit::test]
fn a_resize_across_the_threshold_draws_the_new_bar_on_its_frame(cx: &mut TestAppContext) {
    let (window, view) = open_sized(cx, 1280., 800.);
    frame(cx, window);
    let _ = probed(cx, &view);
    for h in [600., 800., 640., 1080.] {
        let bar_h = if short_at(h) { crate::chrome::top_h(true) } else { crate::chrome::TOP };
        VisualTestContext::from_window(window, cx).simulate_resize(size(px(1280.), px(h)));
        let of = probed(cx, &view);
        let (bar, body) = (of("top-bar"), of("body"));
        assert!(!bar.is_empty(), "{h}: the resize drew a frame");
        assert_eq!(bar[0][3], bar_h - BAR_RULE, "{h}: the resize's own frame draws its bar: {bar:?}");
        assert_eq!([body[0][1], body[0][1] + body[0][3]], [bar_h, h], "{h}: the rest under it, to the foot: {body:?}");
        frame(cx, window);
        let of = probed(cx, &view);
        assert!(of("top-bar").iter().all(|b| *b == bar[0]), "{h}: the bar stays: {:?}", of("top-bar"));
        assert!(of("body").iter().all(|b| *b == body[0]), "{h}: nothing under it moves: {:?}", of("body"));
    }
    assert!(short_at(600.) && !short_at(800.) && short_at(640.) && !short_at(1080.), "every step crosses the threshold");
}

/// The finish gate's R1, split: two panes across, the resize's own frame
/// draws the window's bar and lays both panes under it to the foot.
#[gpui_kit::test]
fn a_resize_across_the_threshold_lays_split_panes_under_the_new_bar(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-gate-r1", None);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let _ = probed(cx, &view);
    for h in [600., 800.] {
        let bar_h = if short_at(h) { crate::chrome::top_h(true) } else { crate::chrome::TOP };
        VisualTestContext::from_window(window, cx).simulate_resize(size(px(1280.), px(h)));
        let of = probed(cx, &view);
        let (bar, panes) = (of("top-bar"), of("pane"));
        assert!(!bar.is_empty() && panes.len() >= 2, "{h}: the resize drew a frame: {bar:?} / {panes:?}");
        assert_eq!(bar[0][3], bar_h - BAR_RULE, "{h}: the resize's own frame draws its bar: {bar:?}");
        for p in &panes[..2] {
            assert_eq!([p[1], p[1] + p[3]], [bar_h, h], "{h}: each pane under the bar, to the foot: {panes:?}");
        }
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The finish gate's M2: the divider between panes side by side is the
/// 1 px `hair` down their whole height (it drew nothing, and the seam was
/// the chrome showing through); the one between stacked panes across
/// their width.
#[gpui_kit::test]
fn the_dividers_are_drawn_across_their_panes(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-gate-m2", None);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let row = drawn(cx, window, "pane-divider Row".into()).expect("the vertical divider");
    let pane = drawn(cx, window, "pane 0".into()).expect("the first pane");
    assert_eq!((row[2], row[3]), (crate::split::DIVIDER, pane[3]), "1 px wide, the panes' height: {row:?} / {pane:?}");
    assert_eq!((row[0], row[1]), (pane[0] + pane[2], pane[1]), "between the panes, from their top: {row:?} / {pane:?}");
    key(cx, window, "ctrl-shift-\\");
    frame(cx, window);
    let column = drawn(cx, window, "pane-divider Column".into()).expect("the horizontal divider");
    let right = view.read_with(cx, |v, _| v.panes.cur.id);
    let low = drawn(cx, window, format!("pane {}", right.index())).expect("the new pane");
    assert_eq!((column[2], column[3]), (low[2], crate::split::DIVIDER), "across the stacked panes: {column:?} / {low:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The finish gate's M4: an unfocused pane's active tab title is
/// `prose_ch` in every state (running, unread, NEEDS YOU, workers ...),
/// as an idle one's; the focused pane's is `ink`. So the same session in
/// two panes, NEEDS YOU, has two different strips. A tab not shown keeps
/// its state's colour either way.
#[gpui_kit::test]
fn an_unfocused_panes_tab_title_dims_in_every_state(cx: &mut TestAppContext) {
    use crate::rail::State;
    cx.update(|cx| {
        gpui_kit::init(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text: 16 }, cx);
        let p = crate::theme::palette(cx);
        let states = [
            State::Opening,
            State::NeedsYou,
            State::Running,
            State::Workers,
            State::Locked,
            State::Unread,
            State::Idle,
            State::Parked,
            State::Saved,
        ];
        for st in states {
            for plain in [false, true] {
                let shown = |unfocused| crate::tabstrip::tab_title(st, plain, unfocused, 1., &p);
                assert_eq!(shown(false), p.ink, "{st:?} (plain {plain}): the focused pane's title is ink");
                assert_eq!(shown(true), p.prose_ch, "{st:?} (plain {plain}): the unfocused pane's title is prose_ch");
                let hidden = |unfocused| crate::tabstrip::tab_title(st, plain, unfocused, 0., &p);
                assert_eq!(hidden(true), hidden(false), "{st:?} (plain {plain}): a tab not shown is the same in either pane");
            }
        }
    });
}

/// The finish gate's S1: a note (a refusal) in the foot is read whole:
/// model · effort, the word and the name drop before it does. In a 271 px
/// row (a 335 px pane's), "no room to split this pane" fits.
#[test]
fn a_note_in_the_foot_keeps_its_hint_whole() {
    let cell = 7.8f32;
    let hint = ("no room to split this pane".chars().count() as f32 * cell).ceil();
    let i = crate::app::FootIn {
        room: 271.,
        lead: 20.,
        word: 48.,
        name: 0.,
        model: 72.,
        effort: 40.,
        hint,
        halt: 0.,
        primary: 60.,
        tight: true,
    };
    let plain = crate::app::fit_foot(&i);
    assert!(!plain.hint, "as a hint it goes first: {plain:?}");
    let note = crate::app::fit_foot_with(&i, true);
    assert!(note.hint && !note.model && !note.effort, "as a note it stays, model · effort gone: {note:?}");
}

/// Pane `id`'s box as the last frame drew it.
fn pane_box(cx: &mut TestAppContext, window: AnyWindowHandle, id: crate::pane::PaneId) -> [f32; 4] {
    drawn(cx, window, format!("pane {}", id.index())).expect("the pane drawn")
}

/// Frames at 144 Hz until the layout's flight has landed (the frame that
/// lands it drawn): how many.
fn land(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) -> usize {
    for i in 0..400 {
        frame_144(cx, window, view);
        if view.read_with(cx, |v, _| v.panes.flight.is_none()) {
            return i + 1;
        }
    }
    panic!("the layout's flight never landed");
}

/// The mouse at `at`: down (`down`), up (`up`), or moved with the left
/// button held, the `n`th click of a run.
fn mouse(cx: &mut TestAppContext, window: AnyWindowHandle, at: Point<Pixels>, how: &str, n: usize) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    let mut vcx = VisualTestContext::from_window(window, cx);
    let m = Modifiers::default();
    match how {
        "down" => {
            vcx.simulate_event(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: n, first_mouse: false })
        }
        "up" => vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: n }),
        _ => vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: m }),
    }
    vcx.run_until_parked();
}

fn level(cx: &mut TestAppContext, l: crate::motion::Level) {
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(l), lock: None });
        crate::motion::Level::apply(cx);
    });
}

/// split.md §4, A10: a divider has a 6 px handle on its rule, and follows
/// the pointer 1:1 (moves between frames laid out by the one frame); it
/// stops at the 320 px minimum on either side; let go within 8 px of
/// equal it lands on equal, on `sheet`; let go further off it stays; a
/// double-click equalises, on `sheet`.
#[gpui_kit::test]
fn a_divider_follows_the_pointer_stops_at_the_minimum_and_lands_on_equal(cx: &mut TestAppContext) {
    use gpui_kit::point;
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-divider", None);
    level(cx, crate::motion::Level::Full);
    // A pane's box and its column's width, as one frame drew them.
    let widening = |cx: &mut TestAppContext, id: crate::pane::PaneId| {
        let column = view.read_with(cx, |v, _| v.panes.get(id).expect("the pane").layout.column);
        (pane_box(cx, window, id), column)
    };
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let ids = view.read_with(cx, |v, _| v.panes.ids());
    let (left, right) = (ids[0], ids[1]);
    let grab = drawn(cx, window, "divider-grab 0".into()).expect("the divider's handle");
    let (l0, r0) = (pane_box(cx, window, left), pane_box(cx, window, right));
    assert_eq!(grab[2], 6., "a 6 px handle: {grab:?}");
    assert_eq!((grab[0] + 2., grab[1], grab[3]), (l0[0] + l0[2], l0[1], l0[3]), "on the rule, the panes' height: {grab:?} / {l0:?}");
    let (x0, y) = (grab[0] + 3., grab[1] + grab[3] / 2.);
    // The pointer at the left pane's width `w`.
    let at = |w: f32| point(px(x0 + w - l0[2]), px(y));
    mouse(cx, window, at(l0[2]), "down", 1);
    for dx in [1., 4., 30., 60.] {
        mouse(cx, window, at(l0[2] + dx), "move", 1);
    }
    mouse(cx, window, at(l0[2] + 100.), "move", 1);
    frame(cx, window);
    let (l1, r1) = (pane_box(cx, window, left), pane_box(cx, window, right));
    assert_eq!(l1[2], l0[2] + 100., "1:1: {l1:?}");
    assert_eq!((r1[0], r1[0] + r1[2]), (r0[0] + 100., r0[0] + r0[2]), "its neighbour's near edge with it, its far edge still: {r1:?}");
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_none()), "a drag is 1:1, never a flight");
    mouse(cx, window, at(l0[2] + 2000.), "move", 1);
    frame(cx, window);
    assert_eq!(pane_box(cx, window, right)[2], crate::split::PANE_MIN_W, "stops at the right pane's minimum");
    mouse(cx, window, at(-2000.), "move", 1);
    frame(cx, window);
    assert_eq!(pane_box(cx, window, left)[2], crate::split::PANE_MIN_W, "stops at the left pane's minimum");
    // Let go 6 px off equal: it lands on equal, on `sheet`.
    let pair = l0[2] + r0[2];
    let off = (pair / 2.).floor() + 6.;
    mouse(cx, window, at(off), "move", 1);
    frame(cx, window);
    assert_eq!(pane_box(cx, window, left)[2], off);
    mouse(cx, window, at(off), "up", 1);
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_some()), "let go near equal: on `sheet`");
    let mut ws = Vec::new();
    let mut seen = vec![widening(cx, right)];
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        ws.push(pane_box(cx, window, left)[2]);
        seen.push(widening(cx, right));
        assert!(ws.len() < 200, "the detent never landed");
    }
    rewraps_by_its_last_move(
        cx,
        &mut seen,
        &mut |cx| {
            frame_144(cx, window, &view);
            widening(cx, right)
        },
        "the detent",
    );
    assert!(ws.iter().any(|w| *w < off && *w > (pair / 2.).floor()), "on its way, not there in one frame: {ws:?}");
    assert!(ws.windows(2).all(|w| w[1] <= w[0]), "one way: {ws:?}");
    let (l2, r2) = (pane_box(cx, window, left), pane_box(cx, window, right));
    assert!((l2[2] - r2[2]).abs() <= 1., "landed on equal: {l2:?} / {r2:?}");
    // Let go 40 px off: it stays.
    let grab = drawn(cx, window, "divider-grab 0".into()).expect("the handle, landed");
    let (x1, l2w) = (grab[0] + 3., l2[2]);
    let at = |w: f32| point(px(x1 + w - l2w), px(y));
    mouse(cx, window, at(l2w), "down", 1);
    for w in [l2w + 2., l2w + 10., l2w + 40.] {
        mouse(cx, window, at(w), "move", 1);
    }
    mouse(cx, window, at(l2w + 40.), "up", 1);
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_none()), "no detent 40 px off");
    assert_eq!(pane_box(cx, window, left)[2], l2w + 40., "it stays where it was let go");
    // A double-click: equal, on `sheet`.
    let grab = drawn(cx, window, "divider-grab 0".into()).expect("the handle");
    let c = point(px(grab[0] + 3.), px(y));
    for n in [1, 2] {
        mouse(cx, window, c, "down", n);
        mouse(cx, window, c, "up", n);
    }
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_some()), "a double-click equalises on `sheet`");
    let mut seen = vec![widening(cx, right)];
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        seen.push(widening(cx, right));
        assert!(seen.len() < 200, "the equalise never landed");
    }
    rewraps_by_its_last_move(
        cx,
        &mut seen,
        &mut |cx| {
            frame_144(cx, window, &view);
            widening(cx, right)
        },
        "the equalise",
    );
    let (l3, r3) = (pane_box(cx, window, left), pane_box(cx, window, right));
    assert!((l3[2] - r3[2]).abs() <= 1., "equal: {l3:?} / {r3:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §10, A1: at Full a split's new pane grows from 0 at the split
/// line on `sheet` and lands in ≤ 450 ms, every frame on whole pixels, the
/// pane it split off yielding as it grows. The last frame that moves draws
/// the landed layout: nothing moves after it. At Reduced and at Off the
/// split is there in one frame.
#[gpui_kit::test]
fn a_split_grows_on_sheet_and_its_last_moving_frame_is_the_landed_layout(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-grow", None);
    level(cx, crate::motion::Level::Full);
    frame(cx, window);
    let first = view.read_with(cx, |v, _| v.panes.cur.id);
    key(cx, window, "ctrl-\\");
    let new = view.read_with(cx, |v, _| v.panes.cur.id);
    assert_ne!(new, first);
    let travel = view.read_with(cx, |v, _| v.panes.flight.as_ref().map(|f| f.travel)).expect("in flight");
    let mut steps = vec![(pane_box(cx, window, first), pane_box(cx, window, new))];
    let mut frames = 0;
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 200, "the split never landed");
        steps.push((pane_box(cx, window, first), pane_box(cx, window, new)));
    }
    let landed = *steps.last().expect("frames");
    assert!(steps[0].1[2] < landed.1[2] / 4., "the new pane starts narrow, at the split line: {:?}", steps[0]);
    assert_eq!(steps[0].1[0], steps[0].0[0] + steps[0].0[2] + crate::split::DIVIDER, "beside the pane it split off");
    let lands = steps.iter().position(|s| *s == landed).expect("landed");
    eprintln!("SPLIT-LANDS frame {lands} ({:.0} ms), settles frame {frames}, travel {} px", lands as f32 * 6.944, landed.1[2]);
    assert!(lands <= sheet_lands(travel), "lands on `sheet`: frame {lands}, the spring's {}", sheet_lands(travel));
    for w in steps.windows(2) {
        assert!(w[1].1[2] >= w[0].1[2] && w[1].0[2] <= w[0].0[2], "the new pane only grows, its neighbour only yields: {w:?}");
    }
    assert!(steps.iter().all(|(a, b)| a.iter().chain(b).all(|v| v.fract() == 0.)), "whole pixels");
    // #52: it lands in the frame it draws the landed layout, the last that
    // moves: the frame before it is still on its way (no still frame
    // before the landing).
    assert_ne!(steps[steps.len() - 2], landed, "the frame before the landing already drew the landed layout");
    let tree = view.read_with(cx, |v, _| {
        let (w, h) = v.centre.get();
        v.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w, h })
    });
    assert_eq!(landed.1[2], tree.iter().find(|(i, _)| *i == new).expect("in the tree").1.w, "landed as the tree lays it");
    for _ in 0..20 {
        frame_144(cx, window, &view);
        assert_eq!((pane_box(cx, window, first), pane_box(cx, window, new)), landed, "nothing moves after it lands");
    }
    // Reduced and Off: in one frame.
    for l in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        level(cx, l);
        key(cx, window, "ctrl-shift-\\");
        assert!(view.read_with(cx, |v, _| v.panes.flight.is_none()), "{l:?}: the split in one frame");
        let id = view.read_with(cx, |v, _| v.panes.cur.id);
        let (w, h) = view.read_with(cx, |v, _| v.centre.get());
        let r = view.read_with(cx, |v, _| v.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w, h }));
        let b = pane_box(cx, window, id);
        assert_eq!(b[3], r.iter().find(|(i, _)| *i == id).expect("in the tree").1.h, "{l:?}: landed on its first frame");
        press(cx, window, "ctrl-w");
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §8, §10, A11: at Full a pane's last tab closing springs its
/// share to 0 on `sheet`: its ground (`pane-closing`) narrows frame by
/// frame and the pane beside it takes the space; the focus is the left
/// pane's at once. The last frame that moves draws the one pane landed,
/// and nothing moves after (one pane again, today's layout).
#[gpui_kit::test]
fn a_close_collapses_on_sheet_and_lands_as_one_pane(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-close", None);
    level(cx, crate::motion::Level::Full);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let ids = view.read_with(cx, |v, _| v.panes.ids());
    let (left, right) = (ids[0], ids[1]);
    let r0 = pane_box(cx, window, right);
    let bar = |cx: &mut TestAppContext| drawn(cx, window, "composer-bar".into()).expect("the focused composer");
    key(cx, window, "ctrl-w");
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (1, left), "closed, the focus left at once");
    let travel = view.read_with(cx, |v, _| v.panes.flight.as_ref().map(|f| f.travel)).expect("in flight");
    let ghost = format!("pane-closing {}", right.index());
    let mut steps = vec![(drawn(cx, window, ghost.clone()).expect("its share, closing"), pane_box(cx, window, left), bar(cx))];
    assert!(steps[0].0[2] > r0[2] / 2., "it starts near where it was: {:?} / {r0:?}", steps[0].0);
    let mut frames = 0;
    let column = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.get(left).expect("the pane left").layout.column);
    let mut columns = vec![column(cx)];
    let mut rewrapped = 0;
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 200, "the close never landed");
        if columns.last() != Some(&column(cx)) {
            columns.push(column(cx));
            rewrapped = frames;
        }
        // Landed: one pane, drawn as main draws it (no pane boxes).
        let flying = view.read_with(cx, |v, _| v.panes.flight.is_some());
        let g = drawn(cx, window, ghost.clone()).filter(|_| flying);
        let l = if flying { pane_box(cx, window, left) } else { [0., 0., f32::MAX, 0.] };
        steps.push((g.unwrap_or([0.; 4]), l, bar(cx)));
    }
    for w in steps.windows(2) {
        assert!(w[1].0[2] <= w[0].0[2] && w[1].1[2] >= w[0].1[2], "it only narrows, its neighbour only widens: {w:?}");
    }
    let landed = bar(cx);
    let (w, _) = view.read_with(cx, |v, _| v.centre.get());
    let lands = steps.iter().position(|s| s.1[2] >= w).expect("landed");
    eprintln!("CLOSE-LANDS frame {lands} ({:.0} ms), settles frame {frames}", lands as f32 * 6.944);
    assert!(lands <= sheet_lands(travel), "lands on `sheet`: frame {lands}, the spring's {}", sheet_lands(travel));
    // #52: it lands in the frame the one pane is across the centre, the
    // last that moves: the frame before it is still on its way.
    assert!(steps[steps.len() - 2].1[2] < w, "the frame before the landing already drew the one pane across the centre");
    // Its column kept the narrow wrap through the flight and rewraps
    // once, as it lands: the panels' rule, `PanelFx.freeze` (Shady #42 L4).
    assert!(columns.len() <= 2, "one rewrap: {columns:?}");
    // On the frame it lands, never after (finish gate #42 F1).
    assert!(rewrapped <= lands, "rewraps at frame {rewrapped}, after the frame it lands ({lands})");
    for _ in 0..20 {
        frame_144(cx, window, &view);
        assert_eq!(bar(cx), landed, "nothing moves after it lands");
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #36 L-e and L-f: a split pane's composer rows are fit to the
/// room it has, its own height under its own form's bars (a Short pane
/// was given the full bar's 12 px more), less what a foot pane's composer
/// insets beyond its own form's (the window's form's, M3: 10 px more).
/// Two panes stacked, at window heights where the two differ by a row.
#[gpui_kit::test]
fn a_short_foot_pane_fits_its_composer_to_the_room_it_has(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-fit", None);
    key(cx, window, "ctrl-shift-\\");
    land(cx, window, &view);
    let low = view.read_with(cx, |v, _| v.panes.cur.id);
    // The naming row open and an error on the notice line: what is over
    // the composer takes enough that the rows are short of their most in
    // panes at their minimum height and above.
    view.update(cx, |v, cx| {
        v.model.named.naming = true;
        let live = v.panes.cur.live;
        v.model.lives[live].last_error = Some("the daemon said no".into());
        cx.notify();
    });
    let m = crate::theme::Metrics::at(16);
    let mut checked = 0;
    let mut seen = Vec::new();
    for h in (524..760).step_by(2) {
        VisualTestContext::from_window(window, cx).simulate_resize(size(px(1280.), px(h as f32)));
        frame(cx, window);
        let got = view.update(cx, |v, _| {
            v.in_pane(low, |v| {
                let (lay, land, rows) = (v.panes.cur.layout, v.panes.cur.land.get(), v.panes.cur.composer_rows);
                let ex = crate::fit::Extras { tray: v.tray_rows as u8, stub: v.stub_shown(), ..v.extras() };
                let win = if v.top_short { crate::fit::Form::Short } else { crate::fit::Form::Full };
                let extra = (crate::fit::Composer::of(&m, win).bottom - crate::fit::Composer::of(&m, lay.form()).bottom).max(0.);
                let room = land.h + crate::chrome::top_h(lay.short) - extra;
                let was = crate::fit::composer_rows(&m, lay.form(), land.h + crate::chrome::TOP, false, ex);
                (rows, crate::fit::composer_rows(&m, lay.form(), room, false, ex), was, lay.form())
            })
        });
        let (rows, want, was, form) = got.expect("the low pane");
        assert!(!view.read_with(cx, |v, _| v.panes.folded), "{h}: two panes at their minimum height fit");
        seen.push((h, form, rows, want, was));
        assert_eq!(rows, want, "{h}: {form:?}: the rows its room has");
        if want != was {
            checked += 1;
        }
    }
    assert!(checked > 0, "a height where the room differs by a row: {seen:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── 3b-2: narrow windows, the pane map, moving tabs ─────────────────────

/// A demo window `w` × `h` with split panes on and IDLE_CPU's history on
/// screen (its rows drawn).
fn demo_split(cx: &mut TestAppContext, w: f32, h: f32) -> (AnyWindowHandle, Entity<AppView>) {
    let (window, view) = open_sized(cx, w, h);
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        let other = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::Current);
        cx.notify();
    });
    frame(cx, window);
    (window, view)
}

fn resize(cx: &mut TestAppContext, window: AnyWindowHandle, w: f32, h: f32) {
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(w), px(h)));
    frame(cx, window);
    frame(cx, window);
}

/// split.md §5, §6, A12, A13, A16: with two panes the right panel's header
/// and the rail row of a session on screen show the pane map (none with
/// one pane). In a narrow window the right panel yields to its strip
/// first, then the rail, while every pane keeps its minimum; past that the
/// split folds: the focused pane fills the centre, its strip ends in the
/// pane map, F6 and a click on a map cell switch panes, no divider is
/// drawn, and widening brings back the identical layout.
#[gpui_kit::test]
fn a_narrow_window_yields_the_panels_then_folds_and_widening_restores_it(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    let shown = |cx: &mut TestAppContext| view.read_with(cx, |v, _| (v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown));
    let strip = crate::app::STRIP;
    assert!(
        drawn(cx, window, "right-pane-map".into()).is_none() && drawn(cx, window, "rail-pane-map".into()).is_none(),
        "one pane: no map"
    );
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    assert!(drawn(cx, window, "right-pane-map".into()).is_some(), "two panes: the right panel's header has the map");
    assert!(drawn(cx, window, "rail-pane-map".into()).is_some(), "the rail row of the session on screen has it");
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let ids = view.read_with(cx, |v, _| v.panes.ids());
    assert_eq!(ids.len(), 3, "three across");
    // Equal thirds (a double-click's).
    view.update(cx, |v, cx| {
        let (w, h) = v.centre.get();
        v.panes.tree.equalise(&[], crate::split::Rect { x: 0., y: 0., w, h });
        cx.notify();
    });
    frame(cx, window);
    let tree = view.read_with(cx, |v, _| v.panes.tree.clone());
    // 1920: the right panel at its strip, the rail open, every pane ≥ 480.
    let (rail, right) = shown(cx);
    assert!(rail > strip && right == strip, "1920: the right panel yields first: {rail} / {right}");
    let wide: Vec<[f32; 4]> = ids.iter().map(|id| pane_box(cx, window, *id)).collect();
    assert!(wide.iter().all(|b| b[2] >= 480.), "1920: every pane at least 480: {wide:?}");
    // 1280: the rail yields too; every pane at least its minimum.
    resize(cx, window, 1280., 800.);
    assert_eq!(shown(cx), (strip, strip), "1280: both panels at their strips");
    assert!(ids.iter().all(|id| pane_box(cx, window, *id)[2] >= crate::split::PANE_MIN_W), "1280: every pane at least 320");
    assert!(!view.read_with(cx, |v, _| v.panes.folded));
    // 960: folded. One pane drawn, its strip ends in the map.
    resize(cx, window, 960., 700.);
    assert!(view.read_with(cx, |v, _| v.panes.folded), "960: folded");
    assert!(ids.iter().all(|id| drawn(cx, window, format!("pane {}", id.index())).is_none()), "960: one pane, drawn as one");
    assert!(drawn(cx, window, "divider-grab 0".into()).is_none(), "no divider while folded");
    let map = drawn(cx, window, "fold-map".into()).expect("the folded strip's map");
    assert!(map[1] >= crate::chrome::top_h(true) - 1. && map[3] <= crate::tabstrip::TAB_H, "in the tab strip: {map:?}");
    let focused = view.read_with(cx, |v, _| v.panes.focused);
    key(cx, window, "f6");
    let next = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(next, focused, "F6 moves through the folded panes");
    let cell = drawn(cx, window, format!("pane-map {}", focused.index())).expect("its cell");
    click_at(cx, window, gpui_kit::point(px(cell[0] + cell[2] / 2.), px(cell[1] + cell[3] / 2.)));
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), focused, "a click on a cell focuses its pane");
    // Wide again: the same tree, the same boxes.
    resize(cx, window, 1920., 1080.);
    assert!(!view.read_with(cx, |v, _| v.panes.folded));
    assert_eq!(view.read_with(cx, |v, _| v.panes.tree.clone()), tree, "the tree untouched");
    let back: Vec<[f32; 4]> = ids.iter().map(|id| pane_box(cx, window, *id)).collect();
    assert_eq!(back, wide, "the identical layout");
}

/// The session of each pane's tabs, in reading order.
fn pane_tabs(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<Vec<String>> {
    view.read_with(cx, |v, _| {
        v.reading_order()
            .into_iter()
            .map(|id| {
                let tabs = if id == v.panes.cur.id { &v.model.tabs } else { &v.panes.get(id).expect("the pane").parked.tabs };
                tabs.iter().map(|t| t.session.clone().unwrap_or_default()).collect()
            })
            .collect()
    })
}

/// split.md §9: Ctrl+Alt+→ with one pane and two tabs splits the active
/// tab off into a new pane on the right, focused; Ctrl+Alt+← moves it
/// back, and the pane it leaves (its only tab) closes.
#[gpui_kit::test]
fn ctrl_alt_arrows_move_the_active_tab_across_panes(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    view.update(cx, |v, cx| {
        let other = crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::New);
        cx.notify();
    });
    frame(cx, window);
    let one = pane_tabs(cx, &view);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].len(), 2, "one pane, two tabs: {one:?}");
    let active = view.read_with(cx, |v, _| v.model.tabs.current_session().map(str::to_string)).expect("a session");
    key(cx, window, "ctrl-alt-right");
    frame(cx, window);
    let two = pane_tabs(cx, &view);
    assert_eq!(two.len(), 2, "split off: {two:?}");
    assert_eq!(two[1], std::slice::from_ref(&active), "the tab in the new pane");
    assert!(!two[0].contains(&active), "and gone from the first: {two:?}");
    let new = view.read_with(cx, |v, _| v.reading_order()[1]);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), new, "focused with it");
    press(cx, window, "ctrl-alt-left");
    frame(cx, window);
    let back = pane_tabs(cx, &view);
    assert_eq!(back.len(), 1, "its pane closed with its only tab: {back:?}");
    assert_eq!(back[0].len(), 2, "both tabs in the first pane: {back:?}");
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current_session().map(str::to_string)), Some(active), "the moved tab shown");
}

/// split.md §7 (A14): with nothing waiting in the focused pane, Ctrl+J
/// focuses the first pane in reading order whose session waits, and its
/// card's keys go with it after their wait (never answered by the press).
/// The palette's "Jump to what is waiting" is offered then too.
#[gpui_kit::test]
fn ctrl_j_with_nothing_waiting_here_focuses_the_pane_that_waits(cx: &mut TestAppContext) {
    let (window, view, _client, mut a, attaches, events) = on_a(cx, "panes-ctrl-j", Some(1));
    feed(cx, window, &view, &events, "A's card up", |v| v.panes.cur.card.is_some());
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let (pa, pb) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let _ = (names(&mut a), names(&mut b));
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pb, "B's pane focused, nothing waiting in it");
    let offered = view.read_with(cx, |v, cx| {
        let (_, cmds) = v.palette_entries(cx);
        cmds.iter().find(|e| matches!(e.act, crate::palette::Act::App(crate::palette::AppAct::JumpWaiting))).map(|e| e.off.is_none())
    });
    assert_eq!(offered, Some(true), "the palette offers the jump: A's pane waits");
    // The card's arm on the test's clock (`pin_arm_clock`): its keys read
    // at the jump's instant, however long two frames take to draw.
    crate::app::pin_arm_clock(Some(Instant::now()));
    key(cx, window, "ctrl-j");
    frame(cx, window);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pa, "Ctrl+J focused A's pane, where the card waits");
    assert!(root_has_keys(cx, window, &view), "the card's keys (the window's root) have the focus");
    // The press that jumped answers nothing: the card waits its arm.
    view.update(cx, |v, _| v.in_pane(pa, |v| assert!(!v.panes.cur.card.as_ref().expect("A's card").keys_armed(), "its keys wait again")));
    crate::app::pin_arm_clock(None);
    assert!(!names(&mut a).iter().any(|n| n == "answer"), "nothing answered");
    // Again, with the card's pane focused: Ctrl+J stays (its own card).
    key(cx, window, "ctrl-j");
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pa, "a second Ctrl+J keeps A's pane");
}

/// split.md §7: a card arriving in an unfocused pane fires the family's
/// cues there: its tab's needs bar and its composer foot's glyph turn in
/// that pane's own column (none in the focused pane's), and the card's
/// keys stay unarmed while its pane is not focused. (The rail's row is
/// the window's, by `Model::fact` over every pane's store; this daemon
/// lists no sessions.)
#[gpui_kit::test]
fn a_card_in_an_unfocused_pane_fires_its_cues_there(cx: &mut TestAppContext) {
    let (window, view, _client, mut a, attaches, events) = on_a(cx, "panes-needs", None);
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let (pa, pb) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let _ = (names(&mut a), names(&mut b));
    frame(cx, window);
    let t0 = view.read_with(cx, |v, _| v.fx.clock());
    view.read_with(cx, |v, _| v.fx.set_clock(t0 + Duration::from_millis(16)));
    a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t",
        "event": {"ev": "prompt", "request": {"id": 7, "kind": "confirm", "title": "Confirm tool call", "prompt": "Allow this tool call?"}}}));
    feed(cx, window, &view, &events, "A's prompt in", |v| {
        v.model.lives.iter().any(|l| l.is_active(A) && !l.transcript.pending_prompts().is_empty())
    });
    frame(cx, window);
    frame(cx, window);
    let (in_a, in_b) = view.read_with(cx, |v, _| (v.fx.col(pa).changed_since(t0), v.fx.col(pb).changed_since(t0)));
    let has = |v: &[(crate::motion::Tag, u64)], t: crate::motion::Tag| v.iter().any(|(k, _)| *k == t);
    assert!(has(&in_a, crate::motion::Tag::TabNeeds), "A's tab's needs bar draws in A's pane: {in_a:?}");
    assert!(has(&in_a, crate::motion::Tag::Foot), "A's composer foot's glyph turns in A's pane: {in_a:?}");
    assert!(!has(&in_b, crate::motion::Tag::TabNeeds) && !has(&in_b, crate::motion::Tag::Foot), "nothing in B's pane: {in_b:?}");
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pb, "the focus stays on B's pane");
    let armed = view.update(cx, |v, _| v.in_pane(pa, |v| v.panes.cur.card.as_ref().map(|c| c.keys_armed())));
    assert_ne!(armed, Some(Some(true)), "A's card's keys are not armed while its pane is not focused");
}

/// split.md §2, §8 (A19): with split panes on, the palette lists every pane
/// action with its key and greys what can't run, with why ("one pane",
/// "4 panes is the most"). Equalise pane sizes evens every split (on
/// `sheet`, as a double-click), Join all panes puts the other panes' session tabs into the
/// focused one (once each) and closes them, and Close pane closes the
/// focused pane's tabs and the pane.
#[gpui_kit::test]
fn the_palette_lists_the_pane_actions_and_runs_them(cx: &mut TestAppContext) {
    use crate::palette::{Act, AppAct};
    let (window, view, _client, mut a, attaches, events) = on_a(cx, "panes-palette", None);
    let entries = |cx: &mut TestAppContext| -> Vec<(AppAct, String, Option<String>, Option<String>)> {
        view.read_with(cx, |v, cx| {
            let (_, cmds) = v.palette_entries(cx);
            cmds.iter()
                .filter_map(|e| match e.act {
                    Act::App(a) => {
                        Some((a, e.title.to_string(), e.key.as_ref().map(|k| k.to_string()), e.off.as_ref().map(|o| o.to_string())))
                    }
                    _ => None,
                })
                .collect()
        })
    };
    let find = |es: &[(AppAct, String, Option<String>, Option<String>)], a: AppAct| es.iter().find(|e| e.0 == a).cloned().expect("listed");
    let one = entries(cx);
    let titles: Vec<&str> = one.iter().map(|e| e.1.as_str()).collect();
    for t in [
        "Split right",
        "Split down",
        "Focus pane 1",
        "Focus pane 4",
        "Focus next pane",
        "Focus previous pane",
        "Move tab to the next pane",
        "Move tab to the previous pane",
        "Equalise pane sizes",
        "Close pane",
        "Join all panes",
        "Jump to what is waiting",
        "Settings",
    ] {
        assert!(titles.contains(&t) || t == "Settings", "the palette lists {t}: {titles:?}");
    }
    assert_eq!(find(&one, AppAct::SplitRight).2.as_deref(), Some("Ctrl+\\"), "Split right shows its key");
    assert_eq!(find(&one, AppAct::FocusNextPane).2.as_deref(), Some("F6"), "F6");
    assert_eq!(find(&one, AppAct::SplitRight).3, None, "one pane: it splits");
    for act in [AppAct::FocusNextPane, AppAct::Equalise, AppAct::ClosePane, AppAct::JoinPanes] {
        assert_eq!(find(&one, act).3.as_deref(), Some("one pane"), "{act:?} with one pane");
    }
    let act = |cx: &mut TestAppContext, a: AppAct| {
        cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(a, w, cx))).unwrap();
        frame(cx, window);
    };
    // Three panes: A, a second A (Ctrl+\'s), and B beside it.
    act(cx, AppAct::SplitRight);
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    act(cx, AppAct::SplitDown);
    let _ = (names(&mut a), names(&mut b));
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 3, "three panes");
    let three = entries(cx);
    // A third row is the layout's cap, not the window (design gate #59 M1:
    // "no room at this window size" in a maximised window).
    assert_eq!(find(&three, AppAct::SplitDown).3.as_deref(), Some("2 rows is the most"), "a third row: the cap named");
    let menu_says = view.read_with(cx, |v, _| v.split_refusal(Some(crate::split::Side::Down)));
    assert_eq!(menu_says, Some("2 rows is the most"), "the pane menu's reason, the same words");
    // Even as split: greyed (Shady #44 n16); uneven, it runs (below).
    assert_eq!(find(&three, AppAct::Equalise).3.as_deref(), Some("already even"), "split even: Equalise greyed");
    assert_eq!(find(&three, AppAct::FocusPane(3)).3.as_deref(), Some("no such pane"), "a fourth pane isn't there");
    // Uneven, then Equalise: every split even on its flight's landing.
    view.update(cx, |v, cx| {
        if let crate::split::Node::Split { ratios, .. } = &mut v.panes.tree {
            ratios[0] = 0.7;
            ratios[1] = 0.3;
        }
        cx.notify();
    });
    frame(cx, window);
    assert_eq!(find(&entries(cx), AppAct::Equalise).3, None, "uneven: Equalise runs");
    act(cx, AppAct::Equalise);
    let even = |n: &crate::split::Node| -> bool {
        fn all(n: &crate::split::Node) -> bool {
            match n {
                crate::split::Node::Leaf(_) => true,
                crate::split::Node::Split { kids, ratios, .. } => {
                    ratios.iter().all(|r| (r - 1. / ratios.len() as f32).abs() < 1e-4) && kids.iter().all(all)
                }
            }
        }
        all(n)
    };
    assert!(view.read_with(cx, |v, _| even(&v.panes.tree)), "every split even");
    // Four panes: Split is greyed with why.
    act(cx, AppAct::SplitDown);
    let four = entries(cx);
    if view.read_with(cx, |v, _| v.panes.len()) == 4 {
        assert_eq!(find(&four, AppAct::SplitRight).3.as_deref(), Some("4 panes is the most"), "four panes");
    }
    // Join: every session tab in the focused pane, once each; one pane.
    // One flight for all the closing panes (each its ghost), landing on
    // the pane kept with all of it.
    let kept = view.read_with(cx, |v, _| v.panes.focused);
    let others: Vec<crate::pane::PaneId> = view.read_with(cx, |v, _| v.panes.ids().into_iter().filter(|id| *id != kept).collect());
    level(cx, crate::motion::Level::Full);
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JoinPanes, w, cx))).unwrap();
    let flight = view.read_with(cx, |v, _| v.panes.flight.as_ref().map(|f| (f.ghosts.clone(), f.to.clone())));
    let (mut ghosts, to) = flight.expect("Join flies");
    ghosts.sort_by_key(|id| id.index());
    assert_eq!(ghosts, others, "every other pane closing in the one flight");
    assert_eq!(
        to.rects(crate::split::Rect { x: 0., y: 0., w: 900., h: 700. }).iter().filter(|(_, r)| r.w * r.h > 1. + 900. + 700.).count(),
        1,
        "it lands on one pane"
    );
    // The kept pane only grows, frame by frame, and nothing moves after.
    let mut widths = Vec::new();
    let mut frames = 0;
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 200, "Join never landed");
        if view.read_with(cx, |v, _| v.panes.flight.is_some()) {
            widths.push(pane_box(cx, window, kept)[2]);
        }
    }
    assert!(widths.len() > 10 && widths.windows(2).all(|w| w[1] >= w[0]), "the kept pane widens on `sheet`: {widths:?}");
    let bar = |cx: &mut TestAppContext| drawn(cx, window, "composer-bar".into()).expect("the composer");
    let landed = bar(cx);
    for _ in 0..10 {
        frame_144(cx, window, &view);
        assert_eq!(bar(cx), landed, "nothing moves after Join lands");
    }
    level(cx, crate::motion::Level::Reduced);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "Join leaves one pane");
    let sessions: Vec<String> = view.read_with(cx, |v, _| v.model.tabs.iter().filter_map(|t| t.session.clone()).collect());
    let mut uniq = sessions.clone();
    uniq.dedup();
    assert!(sessions.contains(&A.to_string()) && sessions.contains(&B.to_string()), "A and B both in the pane: {sessions:?}");
    assert_eq!(
        sessions.len(),
        {
            let mut s = sessions.clone();
            s.sort();
            s.dedup();
            s.len()
        },
        "each once: {sessions:?}"
    );
    // Close pane: split again, close the new pane from the palette.
    act(cx, AppAct::SplitRight);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2, "split again");
    act(cx, AppAct::ClosePane);
    for _ in 0..40 {
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "Close pane closed it");
    assert!(view.read_with(cx, |v, _| !v.model.tabs.is_empty()), "the other pane's tabs stay");
}

/// split.md §2 (A6): Open to the side (Alt+click on a rail row) opens in
/// the pane to the right of the focused one; with none, in a new pane on
/// the right; and with no room for one, in the next pane in reading order.
/// A session already on screen focuses its pane, as a click does (§0: a
/// click never makes a second copy).
#[gpui_kit::test]
fn open_to_the_side_follows_its_order(cx: &mut TestAppContext) {
    const C: &str = "20261121-030000-cccc";
    const D: &str = "20261121-040000-dddd";
    let (window, view, _client, mut a, attaches, events) = on_a(cx, "panes-side", None);
    let side = |cx: &mut TestAppContext, s: &str| {
        let action = crate::named::RowAction::Attach(s.into());
        cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.open_to_side(&action, w, cx))).unwrap();
        frame(cx, window);
    };
    let attach = |cx: &mut TestAppContext, s: &'static str| {
        let mut c = next_attach(cx, window, &attaches, "an attach at the daemon");
        c.send(attached(s, None));
        feed(cx, window, &view, &events, "attached", |v| v.model.lives.iter().any(|l| l.is_active(s)));
        let _ = names(&mut c);
        c
    };
    let visible = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.tabs.visible().and_then(|t| t.session.clone()));
    // One pane: a new pane on the right, B in it, focused.
    side(cx, B);
    let _b = attach(cx, B);
    let _ = names(&mut a);
    let (n, focused, order) = view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused, v.reading_order()));
    assert_eq!(n, 2, "one pane: a new pane");
    assert_eq!(order.last(), Some(&focused), "on the right, focused");
    assert_eq!(visible(cx).as_deref(), Some(B), "B in it");
    // The left pane focused: C into its right neighbour, no new pane.
    view.update(cx, |v, cx| v.focus_pane(order[0], crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    side(cx, C);
    let _c = attach(cx, C);
    let (n, focused) = view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused));
    assert_eq!((n, focused), (2, order[1]), "into the right neighbour");
    let tabs = view.read_with(cx, |v, _| v.model.tabs.iter().filter_map(|t| t.session.clone()).collect::<Vec<_>>());
    assert_eq!(tabs, [B.to_string(), C.to_string()], "a new tab there, B's kept");
    assert_eq!(visible(cx).as_deref(), Some(C), "C shown");
    // A, on screen in the left pane: its pane is focused, no copy.
    side(cx, A);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (2, order[0]), "A's pane focused");
    // The right pane focused in a window with no room for a third column:
    // D in the next pane in reading order (round to the left one).
    resize(cx, window, 1000., 800.);
    view.update(cx, |v, cx| v.focus_pane(order[1], crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    let refusal = view.read_with(cx, |v, _| v.split_refusal(Some(crate::split::Side::Right)));
    assert!(refusal.is_some(), "no room for a third column at 1000");
    side(cx, D);
    let _d = attach(cx, D);
    let (n, focused) = view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused));
    assert_eq!((n, focused), (2, order[0]), "the next pane in reading order");
    assert_eq!(visible(cx).as_deref(), Some(D), "D there");
}

/// Every pane's box as this frame drew it, a closing pane's ground under
/// its own name (a pane that starts closing is the same pane moving).
fn boxes(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) -> Vec<(u8, [f32; 4])> {
    let (ids, ghosts) = view.read_with(cx, |v, _| (v.panes.ids(), v.panes.flight.as_ref().map_or(Vec::new(), |f| f.ghosts.clone())));
    let mut out = Vec::new();
    for id in ids {
        if let Some(b) = drawn(cx, window, format!("pane {}", id.index())) {
            out.push((id.index() as u8, b));
        }
    }
    for g in ghosts {
        if let Some(b) = drawn(cx, window, format!("pane-closing {}", g.index())) {
            out.push((g.index() as u8, b));
        }
    }
    out
}

/// The most any edge of a pane on both frames moved between them.
fn most_moved(a: &[(u8, [f32; 4])], b: &[(u8, [f32; 4])]) -> f32 {
    let mut most = 0f32;
    for (id, x) in a {
        if let Some((_, y)) = b.iter().find(|(i, _)| i == id) {
            for d in [y[0] - x[0], y[1] - x[1], y[0] + y[2] - x[0] - x[2], y[1] + y[3] - x[1] - x[3]] {
                most = most.max(d.abs());
            }
        }
    }
    most
}

/// Shady #42 M1, L4: a second split or close mid-flight starts from where
/// the panes are drawn. Ctrl+\ twice, close then close, a split reversed
/// by its close, and a close then a split, each 10 frames into the first
/// flight: no frame moves any edge by more than one frame of its flight's
/// spring (its fastest step from rest, and a pixel's rounding), across the
/// handover and after it, and each lands.
#[gpui_kit::test]
fn a_second_split_or_close_mid_flight_starts_from_where_the_panes_are_drawn(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-handover", None);
    level(cx, crate::motion::Level::Full);
    // Wide enough for three columns beside both panels: nothing yields (a
    // panel's own slide would move every pane; split.md §5).
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(2560.), px(1080.)));
    frame(cx, window);
    // One frame of a flight's spring at its fastest, from rest: travel·ω/e
    // a second, at 144 Hz; and a pixel for the rounding of each end.
    let step = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.panes
                .flight
                .as_ref()
                .map(|f| f.travel * std::f32::consts::TAU / crate::motion::SHEET_RESPONSE / std::f32::consts::E / 144. + 2.)
        })
    };
    let run = |cx: &mut TestAppContext, first: &str, then: &str, what: &str, panes: usize| {
        press(cx, window, first);
        let mut was = boxes(cx, window, &view);
        let mut bound = step(cx).unwrap_or(0.);
        for _ in 0..10 {
            frame_144(cx, window, &view);
            bound = bound.max(step(cx).unwrap_or(0.));
            let now = boxes(cx, window, &view);
            assert!(most_moved(&was, &now) <= bound, "{what}: the first flight steps {} > {bound}", most_moved(&was, &now));
            was = now;
        }
        assert!(view.read_with(cx, |v, _| v.panes.flight.is_some()), "{what}: still in the air");
        let before = step(cx).unwrap_or(0.);
        press(cx, window, then);
        let now = boxes(cx, window, &view);
        let handover = most_moved(&was, &now);
        let bound = before.max(step(cx).unwrap_or(0.));
        eprintln!("HANDOVER {what}: {handover} px (bound {bound:.1})");
        assert!(
            handover <= bound,
            "{what}: the handover moved an edge {handover} px in one frame (one frame's spring: {bound:.1}): {was:?} / {now:?}"
        );
        was = now;
        let mut frames = 0;
        let mut bound = bound;
        while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
            frame_144(cx, window, &view);
            frames += 1;
            assert!(frames < 200, "{what}: never landed");
            bound = bound.max(step(cx).unwrap_or(0.));
            let now = boxes(cx, window, &view);
            assert!(
                most_moved(&was, &now) <= bound,
                "{what}: frame {frames} steps {} > {bound:.1}: {was:?} / {now:?}",
                most_moved(&was, &now)
            );
            was = now;
        }
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), panes, "{what}: the panes left");
    };
    run(cx, "ctrl-\\", "ctrl-\\", "Ctrl+\\ twice", 3);
    run(cx, "ctrl-w", "ctrl-w", "close, then close", 1);
    run(cx, "ctrl-\\", "ctrl-w", "a split, then its close", 1);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    run(cx, "ctrl-w", "ctrl-\\", "a close, then a split", 2);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #42 M2: a pane that closes while a divider is held takes the
/// grab with it: moves after it move nothing, and the release (its split
/// one kid short) neither panics nor moves a divider.
#[gpui_kit::test]
fn a_pane_closed_under_a_held_divider_ends_the_drag(cx: &mut TestAppContext) {
    use gpui_kit::point;
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-grab-close", None);
    level(cx, crate::motion::Level::Full);
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let three = view.read_with(
        cx,
        |v, _| matches!(&v.panes.tree, crate::split::Node::Split { axis: crate::split::Axis::Row, kids, .. } if kids.len() == 3),
    );
    assert!(three, "three across");
    let grab = drawn(cx, window, "divider-grab 1".into()).expect("the second divider's handle");
    let (x, y) = (grab[0] + 3., grab[1] + grab[3] / 2.);
    mouse(cx, window, point(px(x), px(y)), "down", 1);
    for dx in [1., 4., 20., 40.] {
        mouse(cx, window, point(px(x + dx), px(y)), "move", 1);
    }
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.panes.grab.as_ref().is_some_and(|g| g.moved)), "dragging");
    // The right pane (focused, the last split) closes under the drag.
    key(cx, window, "ctrl-w");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2);
    assert!(view.read_with(cx, |v, _| v.panes.grab.is_none()), "the grab went with the layout it was found in");
    land(cx, window, &view);
    let ids = view.read_with(cx, |v, _| v.panes.ids());
    let landed: Vec<_> = ids.iter().map(|id| pane_box(cx, window, *id)).collect();
    mouse(cx, window, point(px(x + 200.), px(y)), "move", 1);
    frame(cx, window);
    mouse(cx, window, point(px(x + 200.), px(y)), "up", 1);
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_none()), "no detent");
    let after: Vec<_> = ids.iter().map(|id| pane_box(cx, window, *id)).collect();
    assert_eq!(after, landed, "nothing moved after the close");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #42 L4: a stacked divider stops at the 240 px minimum on either
/// side; a release outside the window (a move with no button held) ends
/// the drag where it was, and a move after it drags nothing.
#[gpui_kit::test]
fn a_stacked_divider_stops_at_240_and_a_release_outside_the_window_ends_it(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseMoveEvent, point};
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-down-drag", None);
    level(cx, crate::motion::Level::Full);
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    key(cx, window, "ctrl-shift-\\");
    land(cx, window, &view);
    let ids = view.read_with(cx, |v, _| v.panes.ids());
    let (top, bottom) = (ids[0], ids[1]);
    let grab = drawn(cx, window, "divider-grab 0".into()).expect("the divider's handle");
    assert_eq!(grab[3], 6., "a 6 px handle across: {grab:?}");
    let t0 = pane_box(cx, window, top);
    let (x, y0) = (grab[0] + grab[2] / 2., grab[1] + 3.);
    // The pointer at the top pane's height `h`.
    let at = |h: f32| point(px(x), px(y0 + h - t0[3]));
    mouse(cx, window, at(t0[3]), "down", 1);
    for dy in [1., 4., 30., 2000.] {
        mouse(cx, window, at(t0[3] + dy), "move", 1);
    }
    frame(cx, window);
    assert_eq!(pane_box(cx, window, bottom)[3], crate::split::PANE_MIN_H, "stops at the bottom pane's minimum");
    mouse(cx, window, at(-2000.), "move", 1);
    frame(cx, window);
    assert_eq!(pane_box(cx, window, top)[3], crate::split::PANE_MIN_H, "stops at the top pane's minimum");
    mouse(cx, window, at(t0[3] + 100.), "move", 1);
    frame(cx, window);
    assert_eq!(pane_box(cx, window, top)[3], t0[3] + 100.);
    // Let go outside the window: the next move comes with no button held.
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_event(MouseMoveEvent { position: point(px(x), px(1300.)), pressed_button: None, modifiers: Modifiers::default() });
    vcx.run_until_parked();
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.panes.grab.is_none() && v.panes.flight.is_none()), "ended, no detent 100 px off");
    assert_eq!(pane_box(cx, window, top)[3], t0[3] + 100., "where it was let go");
    mouse(cx, window, at(t0[3] + 300.), "move", 1);
    frame(cx, window);
    assert_eq!(pane_box(cx, window, top)[3], t0[3] + 100., "a move after it drags nothing");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The frames at 144 Hz `sheet` takes to bring `travel` px from rest to
/// within a pixel (critically damped: `(1 + ωt)·e^(−ωt)·travel < 1`), and
/// one more: the frame that draws it.
fn sheet_lands(travel: f32) -> usize {
    let w = std::f32::consts::TAU / crate::motion::SHEET_RESPONSE;
    (0..400)
        .find(|k| {
            let t = *k as f32 / 144.;
            travel * (1. + w * t) * (-w * t).exp() < 1.
        })
        .expect("lands")
        + 1
}

/// A flight's frames of a pane (its box, its column) and 5 at rest after
/// (`next` draws one and reads it):
/// its column's last change is on or before its box's last move (finish
/// gate #42 F1: a widening pane rewrapped 3 frames after the rule
/// stopped).
fn rewraps_by_its_last_move(
    cx: &mut TestAppContext,
    seen: &mut Vec<([f32; 4], f32)>,
    next: &mut dyn FnMut(&mut TestAppContext) -> ([f32; 4], f32),
    what: &str,
) {
    for _ in 0..5 {
        seen.push(next(cx));
    }
    let moved = seen.windows(2).rposition(|w| w[0].0 != w[1].0);
    let rewrapped = seen.windows(2).rposition(|w| w[0].1 != w[1].1);
    assert!(rewrapped <= moved, "{what}: its column changes at frame {rewrapped:?}, after its last move ({moved:?}): {seen:?}");
}

/// Finish gate #42 F3: on a stacked split and its close the top pane's
/// composer moves only with the line (up as it splits, down as it
/// closes), never a frame against it, and is still after it lands; and
/// the rows over it with it (its inset above changes with its form: a
/// close's first frame lifted them 4 px).
#[gpui_kit::test]
fn a_stacked_split_and_close_move_the_composer_only_with_the_line(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-foot", None);
    level(cx, crate::motion::Level::Full);
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    frame(cx, window);
    let top = view.read_with(cx, |v, _| v.panes.cur.id);
    let y = |cx: &mut TestAppContext| {
        let list = drawn(cx, window, format!("pane-list {}", top.index())).expect("the top pane's list");
        (drawn(cx, window, format!("composer-frame {}", top.index())).expect("the top pane's composer")[1], list[1] + list[3])
    };
    for (k, what) in [("ctrl-shift-\\", "split down"), ("ctrl-w", "close down")] {
        let mut ys = vec![y(cx)];
        key(cx, window, k);
        ys.push(y(cx));
        let mut frames = 0;
        while view.read_with(cx, |v, _| v.panes.flight.is_some()) || frames < 5 {
            frame_144(cx, window, &view);
            frames += 1;
            assert!(frames < 200, "{what}: never landed");
            ys.push(y(cx));
        }
        let up = what == "split down";
        for (part, ys) in
            [("the composer", ys.iter().map(|y| y.0).collect::<Vec<_>>()), ("the rows over it", ys.iter().map(|y| y.1).collect())]
        {
            assert!(ys.windows(2).all(|w| if up { w[1] <= w[0] } else { w[1] >= w[0] }), "{what}: {part} only with the line: {ys:?}");
            assert!(if up { ys[ys.len() - 1] < ys[0] } else { ys[ys.len() - 1] > ys[0] }, "{what}: {part} moved: {ys:?}");
        }
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #44 M1: Open to the side with room for a third column, on a row
/// whose session is on screen in another pane, focuses that pane and
/// splits nothing (the split came first and stayed, a copy of this
/// pane's session); on the focused pane's own session it does nothing.
#[gpui_kit::test]
fn open_to_the_side_never_splits_for_a_session_on_screen(cx: &mut TestAppContext) {
    let (window, view, _client, mut a, attaches, events) = on_a(cx, "panes-side-wide", None);
    resize(cx, window, 1920., 1080.);
    let side = |cx: &mut TestAppContext, s: &str| {
        let action = crate::named::RowAction::Attach(s.into());
        cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.open_to_side(&action, w, cx))).unwrap();
        frame(cx, window);
    };
    side(cx, B);
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached", |v| v.model.lives.iter().any(|l| l.is_active(B)));
    let _ = (names(&mut a), names(&mut b));
    let (order, focused) = view.read_with(cx, |v, _| (v.reading_order(), v.panes.focused));
    assert_eq!((order.len(), focused), (2, order[1]), "B on the right, focused");
    let refusal = view.read_with(cx, |v, _| v.split_refusal(Some(crate::split::Side::Right)));
    assert!(refusal.is_none(), "room for a third column at 1920");
    // A, on screen in the left pane: that pane, no third.
    side(cx, A);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (2, order[0]), "A's pane focused, nothing split");
    // B, in the focused pane itself: nothing.
    view.update(cx, |v, cx| v.focus_pane(order[1], crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    side(cx, B);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (2, order[1]), "its own: nothing");
}

/// The composer's text in pane `id`.
fn composer_text(cx: &mut TestAppContext, view: &Entity<AppView>, id: crate::pane::PaneId) -> String {
    view.read_with(cx, |v, cx| v.panes.get(id).expect("the pane").composer.read(cx).value().to_string())
}

/// Shady #44 M2: a moved tab's draft goes with it. Split off with
/// Ctrl+Alt+→, the text is in the new pane's composer and not left in the
/// old one; moved back with Ctrl+Alt+← (its pane closing with it), the
/// text is in the first pane's composer again.
#[gpui_kit::test]
fn a_moved_tab_takes_its_draft(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    view.update(cx, |v, cx| {
        let other = crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::New);
        cx.notify();
    });
    frame(cx, window);
    let first = view.read_with(cx, |v, _| v.panes.cur.id);
    cx.update_window(window, |_, w, cx| {
        view.update(cx, |v, cx| v.panes.cur.composer.update(cx, |c, cx| c.set_value("half a thought", w, cx)))
    })
    .unwrap();
    frame(cx, window);
    key(cx, window, "ctrl-alt-right");
    frame(cx, window);
    frame(cx, window);
    let new = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(new, first, "split off");
    assert_eq!(composer_text(cx, &view, new), "half a thought", "the draft in the new pane");
    assert_eq!(composer_text(cx, &view, first), "", "not left behind");
    press(cx, window, "ctrl-alt-left");
    frame(cx, window);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "its pane closed");
    assert_eq!(composer_text(cx, &view, first), "half a thought", "back with it");
}

/// Shady #44 M3, L1: Close pane's tabs come back with Ctrl+Shift+T in
/// the pane that stays (its reopen stack is that pane's now), and Join
/// all panes keeps every kind of tab (a Settings tab, not only sessions).
#[gpui_kit::test]
fn close_pane_reopens_and_join_keeps_every_tab(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    let open = |cx: &mut TestAppContext, s: &str| {
        view.update(cx, |v, cx| {
            v.open_tab(crate::app::Open::Attach { session_id: s.into(), fallback: None, open: false }, crate::tabs::Place::New);
            cx.notify();
        });
        frame(cx, window);
    };
    let act = |cx: &mut TestAppContext, a: AppAct| {
        cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(a, w, cx))).unwrap();
        frame(cx, window);
        frame(cx, window);
    };
    open(cx, crate::demo::FLAKY);
    key(cx, window, "ctrl-alt-right");
    frame(cx, window);
    open(cx, crate::demo::LEGACY);
    assert_eq!(pane_tabs(cx, &view)[1].len(), 2, "the right pane: FLAKY and LEGACY: {:?}", pane_tabs(cx, &view));
    act(cx, AppAct::ClosePane);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "closed");
    key(cx, window, "ctrl-shift-t");
    frame(cx, window);
    let tabs = pane_tabs(cx, &view);
    assert!(tabs[0].iter().any(|s| s == crate::demo::LEGACY), "Ctrl+Shift+T brings back the closed pane's tab: {tabs:?}");
    // Join: a Settings tab in the other pane comes too.
    press(cx, window, "ctrl-alt-right");
    frame(cx, window);
    view.update(cx, |v, cx| {
        v.model.tabs.open_view(crate::tabs::Kind::Settings);
        v.reconcile(None);
        cx.notify();
    });
    frame(cx, window);
    let left = view.read_with(cx, |v, _| v.reading_order()[0]);
    view.update(cx, |v, cx| v.focus_pane(left, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    act(cx, AppAct::JoinPanes);
    land(cx, window, &view);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "joined");
    let kinds = view.read_with(cx, |v, _| v.model.tabs.iter().map(|t| t.kind).collect::<Vec<_>>());
    assert!(kinds.contains(&crate::tabs::Kind::Settings), "the Settings tab came: {kinds:?}");
}

/// A key held: its repeat's key-down (`is_held`), no key-up before it.
fn key_held(cx: &mut TestAppContext, window: AnyWindowHandle, k: &str) {
    let keystroke = gpui_kit::Keystroke::parse_with_os(k, cx.update(|cx| cx.platform_info().os)).expect("a keystroke");
    let down = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent { keystroke, is_held: true, prefer_character_input: false });
    cx.update_window(window, |_, w, cx| {
        w.dispatch_event(down, cx);
    })
    .expect("the window");
    cx.run_until_parked();
    frame(cx, window);
}

/// Shady #44 L3: a held Ctrl+Alt+→ moves the tab once: its repeat does
/// not carry it on (from the last pane, with two tabs there, it split one
/// more off).
#[gpui_kit::test]
fn a_held_move_key_moves_the_tab_once(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    let open = |cx: &mut TestAppContext, s: &str| {
        view.update(cx, |v, cx| {
            v.open_tab(crate::app::Open::Attach { session_id: s.into(), fallback: None, open: false }, crate::tabs::Place::New);
            cx.notify();
        });
        frame(cx, window);
    };
    open(cx, crate::demo::FLAKY);
    key(cx, window, "ctrl-alt-right");
    let left = view.read_with(cx, |v, _| v.reading_order()[0]);
    view.update(cx, |v, cx| v.focus_pane(left, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    open(cx, crate::demo::LEGACY);
    press(cx, window, "ctrl-alt-right");
    assert_eq!(pane_tabs(cx, &view).iter().map(Vec::len).collect::<Vec<_>>(), [1, 2], "LEGACY moved right");
    key_held(cx, window, "ctrl-alt-right");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2, "its repeat moved nothing: {:?}", pane_tabs(cx, &view));
}

/// Shady #44 L4, L13: Ctrl+J to another pane's card, then its answer
/// key before the next frame: the card's keys wait their arm (they were
/// live in that pane long before, unfocused). As in use: the keys at the
/// window's root (the focused pane shows Settings), A's card shown and
/// its keys live two seconds ago (its arm's instants set: the wait is
/// driven by them, not by the wall clock), and Ctrl+J and Ctrl+Enter in
/// one update (no frame between them).
#[gpui_kit::test]
fn a_card_key_right_after_a_ctrl_j_waits_its_arm(cx: &mut TestAppContext) {
    let (window, view, _client, mut a, attaches, events) = on_a(cx, "panes-ctrl-j-batch", Some(1));
    feed(cx, window, &view, &events, "A's card up", |v| v.panes.cur.card.is_some());
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let _ = names(&mut b);
    view.update(cx, |v, cx| {
        v.model.tabs.open_view(crate::tabs::Kind::Settings);
        v.reconcile(None);
        cx.notify();
    });
    cx.update_window(window, |_, w, cx| {
        w.activate_window();
        view.update(cx, |v, cx| w.focus(&v.focus, cx))
    })
    .unwrap();
    for _ in 0..3 {
        frame(cx, window);
    }
    let _ = names(&mut a);
    let pa = view.read_with(cx, |v, _| v.panes.rest[0].id);
    assert!(view.read_with(cx, |v, _| v.panes.rest[0].keys_live), "A's card's keys live in its pane (the keys at the root)");
    let long_ago = std::time::Instant::now() - Duration::from_secs(2);
    view.update(cx, |v, _| v.panes.rest[0].card.as_mut().expect("A's card").arm = crate::app::Arm::new(long_ago));
    assert!(view.read_with(cx, |v, _| v.panes.rest[0].card.as_ref().is_some_and(|c| c.keys_armed())), "armed long ago");
    let keystroke = gpui_kit::Keystroke::parse_with_os("ctrl-j", cx.update(|cx| cx.platform_info().os)).expect("a keystroke");
    let down = gpui_kit::PlatformInput::KeyDown(gpui_kit::KeyDownEvent { keystroke, is_held: false, prefer_character_input: false });
    let (focused, live, acts) = cx
        .update_window(window, |_, w, cx| {
            w.dispatch_event(down, cx);
            view.update(cx, |v, cx| {
                let live = v.panes.cur.keys_live;
                // Ctrl+Enter, a new press, before any frame.
                v.panes.cur.chord_press = true;
                (v.panes.focused, live, v.card_key_armed(cx))
            })
        })
        .expect("the window");
    assert_eq!(focused, pa, "Ctrl+J went to A's pane");
    assert!(!live, "its keys are not live until its next frame");
    assert!(!acts, "the card's keys wait their arm");
}

/// Shady #44 L9: Open to the side on a row whose session is a tab of
/// the focused pane (behind) shows that tab here: no split, no copy of
/// the tab on screen.
#[gpui_kit::test]
fn open_to_the_side_on_a_tab_of_this_pane_shows_it_here(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    view.update(cx, |v, cx| {
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false },
            crate::tabs::Place::New,
        );
        cx.notify();
    });
    frame(cx, window);
    frame(cx, window);
    let action = crate::named::RowAction::Attach(crate::demo::IDLE_CPU.into());
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.open_to_side(&action, w, cx))).unwrap();
    frame(cx, window);
    frame(cx, window);
    let tabs = pane_tabs(cx, &view);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "nothing split: {tabs:?}");
    let shown = view.read_with(cx, |v, _| v.model.tabs.visible().and_then(|t| t.session.clone()));
    assert_eq!(shown.as_deref(), Some(crate::demo::IDLE_CPU), "its tab shown here");
    assert_eq!(tabs.iter().flatten().filter(|s| *s == crate::demo::FLAKY).count(), 1, "no copy: {tabs:?}");
}

/// Shady #44 L10: Join carries a background worker tab's draft under its
/// own key (`session#sa_N`), not its session's.
#[gpui_kit::test]
fn join_carries_a_background_worker_tabs_draft(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let k = view.update(cx, |v, cx| {
        let s = v.model.tabs.visible().and_then(|t| t.session.clone()).expect("its session");
        let ix = v.model.tabs.active().expect("its tab");
        v.model.tabs.open_worker(&s, "sa_9", "scout", "scout");
        let k = v.model.tabs.iter().find(|t| t.worker.is_some()).and_then(|t| t.draft_key()).expect("a worker tab").to_string();
        v.model.tabs.focus(ix);
        v.reconcile(None);
        v.drafts.put(&k, crate::tabs::Draft { text: "for the scout".into(), cursor: 0, note: None });
        cx.notify();
        k
    });
    frame(cx, window);
    let left = view.read_with(cx, |v, _| v.reading_order()[0]);
    view.update(cx, |v, cx| v.focus_pane(left, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JoinPanes, w, cx))).unwrap();
    land(cx, window, &view);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "joined");
    let kept = view.update(cx, |v, _| v.drafts.take(&k).map(|d| d.text));
    assert_eq!(kept.as_deref(), Some("for the scout"), "the worker tab's draft came with it, under {k}");
}

/// Shady #44 L11: a send from a pane Join closes (on the store the pane
/// that stays shows too), refused after the Join: the text comes back to
/// the pane that stays, never dropped with the closed pane.
#[gpui_kit::test]
fn a_refused_send_from_a_pane_join_closed_comes_back(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view, client, mut a, _attaches, events) = on_a(cx, "panes-unsent-join", None);
    key(cx, window, "ctrl-\\");
    let (p0, p1) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let text = "from the second pane";
    frame(cx, window);
    type_enter(cx, window, text);
    assert_eq!(submits(&mut a), 1, "one message sent on A");
    view.update(cx, |v, cx| v.focus_pane(p0, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JoinPanes, w, cx))).unwrap();
    land(cx, window, &view);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (1, p0), "joined into the first pane");
    a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t",
        "event": {"ev": "refused", "client": 2, "command": "submit", "reason": "busy"}}));
    feed(cx, window, &view, &events, "the refusal", |v| v.model.lives[v.panes.cur.live].in_flight.is_empty());
    for _ in 0..3 {
        frame(cx, window);
    }
    let first = view.read_with(cx, |v, cx| (v.panes.cur.restore.clone(), v.panes.cur.composer.read(cx).value().to_string()));
    assert!(first.1 == text || first.0.as_deref() == Some(text), "back in the pane that stays: {first:?} (the second was {p1:?})");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #44 L12: a tab still being created in a pane Join closes goes
/// with its pane (its create is its store's, detached as the pane goes):
/// the pane that stays has no tab nothing would ever fill, and no store
/// is left behind.
#[gpui_kit::test]
fn join_leaves_a_tab_still_being_created_with_its_pane(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-join-pending", None);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Create, crate::tabs::Place::New);
        cx.notify();
    });
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.model.tabs.visible().is_some_and(|t| t.pending())), "a tab being created on the right");
    let left = view.read_with(cx, |v, _| v.reading_order()[0]);
    view.update(cx, |v, cx| v.focus_pane(left, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JoinPanes, w, cx))).unwrap();
    land(cx, window, &view);
    frame(cx, window);
    let (n, pending, stores) =
        view.read_with(cx, |v, _| (v.panes.len(), v.model.tabs.iter().filter(|t| t.pending()).count(), v.model.lives.ids().len()));
    assert_eq!((n, pending), (1, 0), "joined, no tab nothing will fill");
    assert_eq!(stores, 1, "no store left behind");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #44 L6: the right panel's header names the session behind its
/// numbers and lights the panes that show it: on a Settings tab in the
/// focused pane, that pane's session (not an empty title, not this pane).
#[gpui_kit::test]
fn the_right_panels_header_names_the_session_behind_its_numbers(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let (left, right) = view.read_with(cx, |v, _| (v.reading_order()[0], v.reading_order()[1]));
    let label = view.read_with(cx, |v, _| v.model.tabs.visible().map(|t| t.label.clone()).expect("its tab"));
    assert_eq!(view.read_with(cx, |v, _| v.numbers_session()), (label.clone(), vec![right, left]), "both panes show it");
    view.update(cx, |v, cx| {
        v.model.tabs.open_view(crate::tabs::Kind::Settings);
        v.reconcile(None);
        cx.notify();
    });
    frame(cx, window);
    assert_eq!(
        view.read_with(cx, |v, _| v.numbers_session()),
        (label, vec![left]),
        "Settings here: the session's title, the pane that shows it"
    );
}

/// Finish gate #44 F4: the pane map's unlit cells read on the rail's
/// grounds (the chrome, a raised row, a pressed one) in every theme, and
/// a lit cell reads against an unlit one.
#[test]
fn the_pane_maps_cells_read_on_every_rail_ground_in_every_theme() {
    let rgb = |c: gpui_kit::Hsla| {
        let c = gpui_kit::Rgba::from(c);
        let b = |v: f32| (v * 255.).round() as u32;
        (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
    };
    for (name, raw) in crate::palettes::THEMES.iter() {
        let (r, _) = crate::theme::resolve(raw, None);
        let p = crate::theme::Palette::of(&r);
        let unlit = rgb(crate::app::map_cell(&p, false));
        for (ground, g) in [("chrome", p.chrome), ("raised", p.raised), ("press_ch", p.press_ch)] {
            let c = crate::theme::contrast(unlit, rgb(g));
            assert!(c >= crate::theme::FRAME, "{name}: an unlit cell on {ground} {c:.2}:1");
        }
        let c = crate::theme::contrast(rgb(crate::app::map_cell(&p, true)), unlit);
        assert!(c >= crate::theme::FRAME, "{name}: lit against unlit {c:.2}:1");
    }
}

/// Finish gate #42 F2: a close is the split's mirror. From the close's
/// first frame the closing pane's list and rows are drawn (laid out at
/// its size, its share clipping them) and ride the line out, frame by
/// frame; the frame the layout lands draws none of it, nothing moves
/// after, and the pane, its list and its store are gone. At Reduced the
/// close is one frame.
#[gpui_kit::test]
fn a_closing_pane_slides_out_with_its_rows(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-slide", None);
    level(cx, crate::motion::Level::Full);
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let right = view.read_with(cx, |v, _| v.panes.focused);
    // Its list as it is (the same list draws it as it closes), and where
    // its first row on screen is.
    let (list, n) = view.read_with(cx, |v, _| (v.panes.cur.list.clone(), v.panes.cur.disp.len()));
    assert!(n > 0, "rows in the right pane");
    let row = move || (0..n).find_map(|ix| list.bounds_for_item(ix)).map(|b| f32::from(b.origin.x));
    let edge0 = pane_box(cx, window, right)[0];
    let off = row().expect("a row on screen") - edge0;
    let list_off = drawn(cx, window, format!("pane-list {}", right.index())).expect("its list")[0] - edge0;
    key(cx, window, "ctrl-w");
    let share = format!("pane-closing {}", right.index());
    let mut seen = Vec::new();
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        let f = seen.len();
        let g = drawn(cx, window, share.clone());
        if let Some(g) = g.filter(|g| g[2] >= 1.) {
            let l = drawn(cx, window, format!("pane-list {}", right.index()));
            let l = l.unwrap_or_else(|| panic!("frame {f}: its list drawn as it closes (share {g:?})"));
            let x = row().unwrap_or(f32::NAN);
            assert_eq!(l[0] - g[0], list_off, "frame {f}: its list rides the line: share {g:?}, list {l:?}");
            assert_eq!(x - g[0], off, "frame {f}: its rows ride the line: share {g:?}, a row at {x}");
            seen.push(g[0]);
        }
        frame_144(cx, window, &view);
        assert!(seen.len() < 200, "never landed");
    }
    assert!(
        seen.len() > 10 && seen.windows(2).all(|w| w[1] >= w[0]) && seen[seen.len() - 1] > seen[0],
        "it slid out with the line: {seen:?}"
    );
    // Landed: none of it drawn, and nothing after.
    assert!(drawn(cx, window, share.clone()).is_none() && drawn(cx, window, format!("pane-list {}", right.index())).is_none());
    let rest = boxes(cx, window, &view);
    for _ in 0..10 {
        frame_144(cx, window, &view);
        assert_eq!(boxes(cx, window, &view), rest, "nothing moves after the landing");
    }
    assert!(
        view.read_with(cx, |v, _| v.panes.closing.is_empty() && v.model.lives.ids().len() == 1),
        "the pane and any store of its own gone"
    );
    // At Reduced: one frame, none of it drawn.
    level(cx, crate::motion::Level::Reduced);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let right = view.read_with(cx, |v, _| v.panes.focused);
    press(cx, window, "ctrl-w");
    assert!(drawn(cx, window, format!("pane-list {}", right.index())).is_none(), "Reduced: the close is one frame");
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_none() && v.panes.closing.is_empty()));
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #42 r2-L1: four panes, one closes, and a split within its flight
/// is handed the closing pane's id (the only one free): no pane jumps
/// (its share keeps closing, a ghost's ground), and every frame steps by
/// at most one frame's spring. The new pane and the closing share draw
/// under different element ids (Shady #59 L5.1).
#[gpui_kit::test]
fn a_split_that_takes_a_closing_panes_id_moves_no_pane_at_once(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "panes-reuse", None);
    level(cx, crate::motion::Level::Full);
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    frame(cx, window);
    for k in ["ctrl-\\", "ctrl-shift-\\", "alt-1", "ctrl-shift-\\"] {
        key(cx, window, k);
        land(cx, window, &view);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "four panes");
    let closing = view.read_with(cx, |v, _| v.panes.focused);
    key(cx, window, "ctrl-w");
    for _ in 0..3 {
        frame_144(cx, window, &view);
    }
    let step = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.panes
                .flight
                .as_ref()
                .map_or(0., |f| f.travel * std::f32::consts::TAU / crate::motion::SHEET_RESPONSE / std::f32::consts::E / 144. + 2.)
        })
    };
    let mut bound = step(cx);
    let open = |b: Vec<(u8, [f32; 4])>| b.into_iter().filter(|(i, _)| *i != closing.index() as u8).collect::<Vec<_>>();
    let mut was = open(boxes(cx, window, &view));
    // Down, where the closing pane was: a split across (a third column)
    // would make the right panel yield, every pane moving with it (§5).
    key(cx, window, "ctrl-shift-\\");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "the split took the id");
    assert!(view.read_with(cx, |v, _| v.panes.get(closing).is_some()), "the closing pane's id is the new pane's");
    // Shady #59 L5.1 (#55 n1), the id's reuse: in the same frame the new
    // pane draws under its own `("pane", n)` and the closing share under a
    // ghost's, past `PANES_MAX` (never an open pane's: their element state
    // stays apart).
    let ghosts = view.read_with(cx, |v, _| v.panes.flight.as_ref().map(|f| f.ghosts.clone()).unwrap_or_default());
    assert_eq!(ghosts.len(), 1, "the closing share, a ghost now: {ghosts:?}");
    assert!(ghosts[0].index() >= crate::pane::PANES_MAX, "a ghost's id is past the open panes': {ghosts:?}");
    assert!(drawn(cx, window, format!("pane {}", closing.index())).is_some(), "the new pane drawn under its id");
    assert!(drawn(cx, window, format!("pane-closing {}", ghosts[0].index())).is_some(), "the closing share drawn under the ghost's");
    assert!(drawn(cx, window, format!("pane-closing {}", closing.index())).is_none(), "no closing share under the open pane's id");
    let mut frames = 0;
    loop {
        bound = bound.max(step(cx));
        let now = open(boxes(cx, window, &view));
        assert!(
            most_moved(&was, &now) <= bound,
            "frame {frames}: a pane moved {} px (one frame's spring: {bound:.1}): {was:?} / {now:?}",
            most_moved(&was, &now)
        );
        was = now;
        if view.read_with(cx, |v, _| v.panes.flight.is_none()) {
            break;
        }
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 200, "never landed");
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── Round 4 (Shady #44 r3, #42 r3, design F5) ───────────────────────────

/// Every pane's box in the tree as laid out in the centre now.
fn tree_rects(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<(crate::pane::PaneId, crate::split::Rect)> {
    view.read_with(cx, |v, _| {
        let (w, h) = v.centre.get();
        v.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w, h })
    })
}

/// Shady #44 M1: the palette's Equalise pane sizes keeps every pane's
/// minimum, as the double-click's does. `A | (B / (C | D))` at 1280,
/// fitting: an even 1/n put C and D under 320 and folded the split.
#[gpui_kit::test]
fn the_palettes_equalise_keeps_every_panes_minimum(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    for k in ["ctrl-\\", "ctrl-shift-\\", "ctrl-\\"] {
        key(cx, window, k);
        frame(cx, window);
    }
    let tree = view.read_with(cx, |v, _| v.panes.tree.clone());
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "four panes: {tree:?}");
    // The first pane narrow: everything fits at 1280.
    view.update(cx, |v, cx| {
        if let crate::split::Node::Split { ratios, .. } = &mut v.panes.tree {
            ratios[0] = 0.3;
            ratios[1] = 0.7;
        }
        cx.notify();
    });
    resize(cx, window, 1280., 800.);
    land(cx, window, &view);
    let fits = |rs: &[(crate::pane::PaneId, crate::split::Rect)]| {
        rs.iter().all(|(_, r)| r.w >= crate::split::PANE_MIN_W && r.h >= crate::split::PANE_MIN_H)
    };
    let before = tree_rects(cx, &view);
    assert!(!view.read_with(cx, |v, _| v.panes.folded) && fits(&before), "fits at 1280 before: {before:?}");
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::Equalise, w, cx))).unwrap();
    land(cx, window, &view);
    frame(cx, window);
    let after = tree_rects(cx, &view);
    assert!(!view.read_with(cx, |v, _| v.panes.folded), "Equalise never folds the split: {after:?}");
    assert!(fits(&after), "every pane at its minimum or more: {after:?}");
    assert_ne!(before, after, "it equalised");
}

/// Shady #44 L14: Open to the side on a session shown in the pane beside
/// goes to that pane, even when it is a background tab of this one (right
/// after a split: P = [X, S], Q shows S): Q focused, P still on X.
#[gpui_kit::test]
fn open_to_the_side_goes_to_the_pane_beside_before_this_panes_own_tab(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let (p, q) = view.read_with(cx, |v, _| (v.reading_order()[0], v.reading_order()[1]));
    view.update(cx, |v, cx| v.focus_pane(p, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    view.update(cx, |v, cx| {
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false },
            crate::tabs::Place::New,
        );
        cx.notify();
    });
    frame(cx, window);
    let shows = |cx: &mut TestAppContext, id: crate::pane::PaneId| {
        view.update(cx, |v, _| v.in_pane(id, |v| v.model.tabs.visible().and_then(|t| t.session.clone()))).flatten()
    };
    assert_eq!(shows(cx, p).as_deref(), Some(crate::demo::FLAKY), "P on X");
    let action = crate::named::RowAction::Attach(crate::demo::IDLE_CPU.into());
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.open_to_side(&action, w, cx))).unwrap();
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (2, q), "the pane beside, focused; nothing split");
    assert_eq!(shows(cx, p).as_deref(), Some(crate::demo::FLAKY), "P still on X");
    assert_eq!(shows(cx, q).as_deref(), Some(crate::demo::IDLE_CPU), "Q on S");
}

/// Shady #44 L15 (#42 r3-L1's root): Join closes two panes on one store
/// while the pane kept is on another; a message the second sent and the
/// daemon never confirmed comes back (its session's draft in the pane
/// kept), never dropped as the store goes (the first closing pane counted
/// as sharing it, so the second never let it go).
#[gpui_kit::test]
fn join_closing_two_panes_on_one_store_keeps_their_unsent_text(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view, client, mut a, attaches, events) = on_a(cx, "panes-unsent-join3", None);
    resize(cx, window, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let order = view.read_with(cx, |v, _| v.reading_order());
    assert_eq!(order.len(), 3, "three panes on A");
    let (p0, p2) = (order[0], order[2]);
    view.update(cx, |v, cx| v.focus_pane(p0, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = next_attach(cx, window, &attaches, "B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the first pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let _ = (names(&mut a), names(&mut b));
    view.update(cx, |v, cx| v.focus_pane(p2, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    let text = "from the third pane";
    type_enter(cx, window, text);
    assert_eq!(submits(&mut a), 1, "one message sent on A");
    view.update(cx, |v, cx| v.focus_pane(p0, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    level(cx, crate::motion::Level::Full);
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JoinPanes, w, cx))).unwrap();
    land(cx, window, &view);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (1, p0), "joined into the first pane");
    a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t",
        "event": {"ev": "refused", "client": 2, "command": "submit", "reason": "busy"}}));
    let stop = crate::test_waits::Backstop::start();
    let back = loop {
        let batch = crate::test_waits::batch(&events);
        if !batch.is_empty() {
            view.update(cx, |v, cx| v.apply_batch(batch, cx));
        }
        frame(cx, window);
        let got = view.update(cx, |v, cx| {
            let shown = (v.panes.cur.restore.clone(), v.panes.cur.composer.read(cx).value().to_string());
            v.drafts.take(A).map(|d| d.text).or(shown.0).or((shown.1 == text).then_some(shown.1))
        });
        if got.is_some() {
            break got;
        }
        // A closed client fails at once (Shady #74 P2); a live one that
        // never brings it back, at the backstop.
        if events.is_closed() && events.is_empty() {
            panic!("the client closed before the unsent text came back");
        }
        stop.check("the unsent text back in the pane", || "no draft or restore yet".into());
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(back.as_deref(), Some(text), "the unsent text back in the pane kept, as A's draft");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #42 r3-L1: two panes on one session, the right one closes, and
/// the left switches tabs within the flight (at Full). The closing pane's
/// rows stay what they were every frame (they read a store of their own:
/// the switch emptied the shared one, then filled it with the other
/// session), and the left pane's session goes to the tab cache.
#[gpui_kit::test]
fn a_closing_pane_on_a_shared_store_keeps_its_rows_as_the_other_switches(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    level(cx, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false },
            crate::tabs::Place::New,
        );
        cx.notify();
    });
    frame(cx, window);
    view.update(cx, |v, cx| {
        let ix = v.model.tabs.iter().position(|t| t.session.as_deref() == Some(crate::demo::IDLE_CPU)).expect("its tab");
        v.focus_tab(ix);
        cx.notify();
    });
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let right = view.read_with(cx, |v, _| v.panes.focused);
    let shared = view.read_with(cx, |v, _| v.panes.rest[0].live == v.panes.cur.live);
    assert!(shared, "one store, two panes");
    let rows = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.panes.closing.iter().find(|p| p.id == right).map(|p| {
                let t = v.model.lives[p.live].shown();
                (t.session_id().map(str::to_string), t.items().len(), p.disp.iter().all(|d| d.item < t.items().len()))
            })
        })
    };
    key(cx, window, "ctrl-w");
    let at_close = rows(cx).expect("the right pane closing");
    assert!(at_close.0.as_deref() == Some(crate::demo::IDLE_CPU) && at_close.1 > 0 && at_close.2, "its rows: {at_close:?}");
    for _ in 0..3 {
        frame_144(cx, window, &view);
    }
    key(cx, window, "ctrl-tab");
    let mut frames = 0;
    while let Some(now) = rows(cx) {
        assert_eq!(now, at_close, "frame {frames}: the closing pane's rows as they were");
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 200, "never landed");
    }
    assert!(frames > 5, "it closed over frames, the switch within them ({frames})");
    let left_shows = view.read_with(cx, |v, _| v.model.tabs.visible().and_then(|t| t.session.clone()));
    assert_eq!(left_shows.as_deref(), Some(crate::demo::FLAKY), "the left pane switched");
    assert!(view.update(cx, |v, _| v.tab_cache.take(&crate::demo::IDLE_CPU.to_string()).is_some()), "its old session in the tab cache");
}

/// Shady #42 r2-L2: Equalise from the palette while a pane is closing:
/// its share keeps closing (drawn, narrowing, every frame) as the panes
/// left even out, never snapped away in one frame.
#[gpui_kit::test]
fn equalise_while_a_pane_closes_keeps_it_closing(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let gone = view.read_with(cx, |v, _| v.panes.focused);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 3, "three panes");
    level(cx, crate::motion::Level::Full);
    key(cx, window, "ctrl-w");
    let share = format!("pane-closing {}", gone.index());
    let mut ws = Vec::new();
    for _ in 0..3 {
        frame_144(cx, window, &view);
        ws.push(drawn(cx, window, share.clone()).expect("closing")[2]);
    }
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::Equalise, w, cx))).unwrap();
    let mut frames = 0;
    while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 200, "never landed");
        match drawn(cx, window, share.clone()) {
            Some(g) if g[2] >= 1. => ws.push(g[2]),
            _ => break,
        }
    }
    assert!(ws.len() > 8, "it kept closing after the Equalise: {ws:?}");
    assert!(ws.windows(2).all(|w| w[1] <= w[0]), "narrowing all the way: {ws:?}");
    land(cx, window, &view);
    let rs = tree_rects(cx, &view);
    assert!(rs.len() == 2 && (rs[0].1.w - rs[1].1.w).abs() <= 1., "the two left even: {rs:?}");
    assert!(view.read_with(cx, |v, _| v.panes.closing.is_empty()), "and the closed pane gone");
}

/// Shady #42 L2: a layout flight with a panel's flight over it (Equalise
/// as the right panel springs wider to a detent, on `snap`, faster than
/// the layout's `sheet`): each pane's column rewraps at most twice (as it
/// narrows, at the start; as it widens, as it lands), never frame by frame
/// as the centre narrows (a widening pane's drawn width fell for several
/// frames, and each new least was a new wrap).
#[gpui_kit::test]
fn a_panel_flight_over_a_layout_flight_rewraps_each_pane_at_most_twice(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let ids = view.read_with(cx, |v, _| v.reading_order());
    view.update(cx, |v, cx| {
        v.panels.right_collapsed = false;
        v.panels.right_width = None;
        if let crate::split::Node::Split { ratios, .. } = &mut v.panes.tree {
            ratios[0] = 0.4;
            ratios[1] = 0.6;
        }
        cx.notify();
    });
    level(cx, crate::motion::Level::Full);
    for _ in 0..200 {
        frame_144(cx, window, &view);
        if !view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some()) {
            break;
        }
    }
    let columns = |cx: &mut TestAppContext| -> Vec<f32> {
        view.read_with(cx, |v, _| ids.iter().map(|id| v.panes.get(*id).expect("the pane").layout.column).collect())
    };
    let mut seen = vec![columns(cx)];
    let right0 = view.read_with(cx, |v, _| v.panel_fx.sides[1].shown);
    cx.update_window(window, |_, w, cx| {
        view.update(cx, |v, cx| {
            let mut p = v.panels;
            p.right_width = Some((right0 + 200.).round() as u16);
            v.fly_to(p, crate::motion::SNAP_RESPONSE);
            v.panels = p;
            v.app_act(AppAct::Equalise, w, cx);
        })
    })
    .unwrap();
    let mut frames = 0;
    while frames < 5 || view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some()) {
        frame_144(cx, window, &view);
        seen.push(columns(cx));
        frames += 1;
        assert!(frames < 400, "never landed");
    }
    for _ in 0..5 {
        frame_144(cx, window, &view);
        seen.push(columns(cx));
    }
    let right1 = view.read_with(cx, |v, _| v.panel_fx.sides[1].shown);
    assert!(right1 > right0 + 100., "the panel went wider: {right0} → {right1}");
    for (k, id) in ids.iter().enumerate() {
        let col: Vec<f32> = seen.iter().map(|c| c[k]).collect();
        let changes = col.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(changes <= 2, "pane {id:?}'s column changed {changes} times: {col:?}");
    }
}

/// #46 (the gate's N1, pr-44 round 4 f802): the scrollbar thumbs land
/// with the panes. Each pane's thumb, as painted frame by frame, takes
/// its new length in the frame its pane lands (the frame its column
/// re-wraps in), never a frame after: from its pane's last move on, every
/// frame paints it where the landing did. Equalise (Full, Reduced), a
/// split, a close, and the rail's fold over a split.
#[gpui_kit::test]
fn the_thumbs_land_with_the_panes(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    level(cx, crate::motion::Level::Full);
    let moving = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some());
    let fly = |cx: &mut TestAppContext| {
        let mut n = 0;
        while n < 3 || moving(cx) {
            frame_144(cx, window, &view);
            n += 1;
            assert!(n < 400, "never landed");
        }
        for _ in 0..8 {
            frame_144(cx, window, &view);
        }
    };
    // Every pane's bar (each keeps its own as panes swap: `Parked::bar`).
    let bars = |cx: &mut TestAppContext| -> Vec<crate::scrollbar::BarState> {
        view.read_with(cx, |v, _| {
            let mut b = vec![v.bars.transcript.clone()];
            b.extend(
                v.panes.ids().into_iter().filter(|id| *id != v.panes.cur.id).filter_map(|id| v.panes.get(id).map(|p| p.parked.bar.clone())),
            );
            b
        })
    };
    // Each bar's frames since `drain`, one at rest first; its pane lands
    // in the last frame its rectangle or its column changed.
    let drain = |cx: &mut TestAppContext, bars: &[crate::scrollbar::BarState]| {
        bars.iter().for_each(|b| drop(b.thumbs()));
        frame_144(cx, window, &view);
    };
    let mut fails = Vec::new();
    let mut check = |case: &str, bars: &[crate::scrollbar::BarState]| {
        let mut changed = false;
        for (k, bar) in bars.iter().enumerate() {
            let log = bar.thumbs();
            let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01);
            let moved = |i: usize| !near(log[i].0, log[i - 1].0) || log[i].1 != log[i - 1].1;
            let Some(last) = (1..log.len()).rev().find(|&i| moved(i)) else { continue };
            let same = |a: Option<[f32; 4]>, b: Option<[f32; 4]>| match (a, b) {
                (Some(a), Some(b)) => near(a, b),
                (a, b) => a.is_none() && b.is_none(),
            };
            let rect = (1..log.len()).rev().find(|&i| !near(log[i].0, log[i - 1].0)).unwrap_or(0);
            let col = (1..log.len()).rev().find(|&i| log[i].1 != log[i - 1].1).unwrap_or(0);
            eprintln!("{case}: bar {k}: {} frames, its rectangle last moved at {rect}, its column at {col}", log.len());
            changed |= !same(log[0].2, log[last].2);
            if let Some(i) = (last + 1..log.len()).find(|&i| !same(log[i].2, log[last].2)) {
                fails.push(format!(
                    "{case}: bar {k}'s pane landed at frame {last} of {} (column {}), its thumb {:?}; frame {i} paints it at {:?}",
                    log.len(),
                    log[last].1,
                    log[last].2,
                    log[i].2
                ));
            }
        }
        if !changed {
            fails.push(format!("{case}: no thumb changed (nothing to read)"));
        }
    };
    // A split.
    let one = bars(cx);
    drain(cx, &one);
    key(cx, window, "ctrl-\\");
    fly(cx);
    let two = bars(cx);
    assert_eq!(two.len(), 2, "two panes");
    check("a split", &two);
    // Equalise from 30/70, at Full and at Reduced.
    for l in [crate::motion::Level::Full, crate::motion::Level::Reduced] {
        level(cx, crate::motion::Level::Full);
        view.update(cx, |v, cx| {
            if let crate::split::Node::Split { ratios, .. } = &mut v.panes.tree {
                ratios[0] = 0.3;
                ratios[1] = 0.7;
            }
            cx.notify();
        });
        fly(cx);
        level(cx, l);
        let b = bars(cx);
        drain(cx, &b);
        cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::Equalise, w, cx))).unwrap();
        fly(cx);
        check(&format!("Equalise at {l:?}"), &b);
    }
    level(cx, crate::motion::Level::Full);
    // The rail's fold (every pane widens), and back.
    for case in ["the rail's fold", "the rail's unfold"] {
        let b = bars(cx);
        drain(cx, &b);
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        cx.run_until_parked();
        fly(cx);
        check(case, &b);
    }
    // A close (the pane left widens), a split, a close: at Reduced, then Full.
    for (case, l) in
        [("a close", crate::motion::Level::Reduced), ("a split", crate::motion::Level::Reduced), ("a close", crate::motion::Level::Full)]
    {
        level(cx, l);
        let b = bars(cx);
        drain(cx, &b);
        if case == "a split" {
            key(cx, window, "ctrl-\\");
        } else {
            cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::ClosePane, w, cx))).unwrap();
        }
        fly(cx);
        let b = if case == "a split" { bars(cx) } else { b };
        check(&format!("{case} at {l:?}"), &b);
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// #47 L1 (Shady): a divider held and dragged holds each pane's transcript
/// thumb where the frame before the press painted it, as a panel's edge
/// does (the gate's S3): every frame of the drag re-wraps the rows of a
/// pane under the column's cap. Let go, the thumbs paint from the rows.
#[gpui_kit::test]
fn a_divider_drag_holds_the_thumbs(cx: &mut TestAppContext) {
    use gpui_kit::point;
    let (window, view) = demo_split(cx, 1920., 1080.);
    level(cx, crate::motion::Level::Full);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let ids = view.read_with(cx, |v, _| v.reading_order());
    let bars: Vec<crate::scrollbar::BarState> = view.read_with(cx, |v, _| {
        ids.iter()
            .map(
                |id| if *id == v.panes.cur.id { v.bars.transcript.clone() } else { v.panes.get(*id).expect("the pane").parked.bar.clone() },
            )
            .collect()
    });
    let columns = |cx: &mut TestAppContext| -> Vec<f32> {
        view.read_with(cx, |v, _| ids.iter().map(|id| v.panes.get(*id).expect("the pane").layout.column).collect())
    };
    frame(cx, window);
    let before: Vec<(f32, f32, bool)> = bars.iter().map(|b| *b.paints().last().expect("painted")).collect();
    let col0 = columns(cx);
    let grab = drawn(cx, window, "divider-grab 0".into()).expect("the divider's handle");
    let (x, y) = (grab[0] + 3., grab[1] + grab[3] / 2.);
    mouse(cx, window, point(px(x), px(y)), "down", 1);
    let mut cols = Vec::new();
    for dx in [-4., -12., -24., -40., -60., -80.] {
        mouse(cx, window, point(px(x + dx), px(y)), "move", 1);
        frame(cx, window);
        cols.push(columns(cx));
    }
    assert!(view.read_with(cx, |v, _| v.panes.grab.as_ref().is_some_and(|g| g.moved)), "dragging");
    assert!(cols.iter().any(|c| *c != col0), "the drag re-wrapped a pane's rows: {col0:?} {cols:?}");
    for (k, bar) in bars.iter().enumerate() {
        let paints = bar.paints();
        let b = before[k];
        let off: Vec<_> = paints.iter().filter(|p| !p.2 || (p.0 - b.0).abs() >= 0.5 || (p.1 - b.1).abs() >= 0.5).collect();
        assert!(!paints.is_empty() && off.is_empty(), "pane {k}: held as before the press {b:?} through the drag: {off:?}");
    }
    mouse(cx, window, point(px(x - 80.), px(y)), "up", 1);
    land(cx, window, &view);
    frame(cx, window);
    for (k, bar) in bars.iter().enumerate() {
        let end = *bar.paints().last().expect("painted");
        assert!(!end.2, "pane {k}: let go, from the rows: {end:?}");
    }
}

/// Haseeb's rule, for the side panels' flights (#47): nothing moves after
/// the panels and the panes stop. A panel's spring settles under a pixel
/// for a while after its drawn width reaches its target; the column, the
/// labels and the thumbs land in the frame the width does, not when the
/// spring stops. Frame by frame through the rail's and the right panel's
/// fold and unfold, in one pane and in two: from the last frame a panel's
/// edge or a pane's moves, no row, no column (the labels' hang and left)
/// and no thumb changes. On `d8dc3a0` the folds' columns re-wrapped 6 or 7
/// frames (at 144 Hz) after the panes stopped.
#[gpui_kit::test]
fn nothing_moves_after_the_panels_and_panes_stop(cx: &mut TestAppContext) {
    let mut fails = Vec::new();
    for panes in [1, 2] {
        // One pane at 1280 (at 1920 its column is at its cap either side of
        // a fold, and nothing re-wraps); two at 1920.
        let (window, view) = if panes == 1 { open_sized(cx, 1280., 800.) } else { demo_split(cx, 1920., 1080.) };
        if panes == 2 {
            key(cx, window, "ctrl-\\");
            frame(cx, window);
        }
        view.update(cx, |v, cx| {
            (v.panels.rail_collapsed, v.panels.right_collapsed) = (false, false);
            cx.notify();
        });
        frame(cx, window);
        level(cx, crate::motion::Level::Full);
        let ids = view.read_with(cx, |v, _| v.reading_order());
        assert_eq!(ids.len(), panes, "{panes} pane(s)");
        let bars: Vec<crate::scrollbar::BarState> =
            view.read_with(cx, |v, _| {
                ids.iter()
                    .map(|id| {
                        if *id == v.panes.cur.id {
                            v.bars.transcript.clone()
                        } else {
                            v.panes.get(*id).expect("the pane").parked.bar.clone()
                        }
                    })
                    .collect()
            });
        // The thumbs painted (else the thumb check is vacuous: one not
        // painted is NaN on both sides).
        let thumbed = std::cell::Cell::new(0usize);
        // A frame: the edges (the panels as drawn, each pane's list's box),
        // and what is drawn in the panes (each one's column, its left and
        // the labels' hang, its first rows' boxes, its thumb).
        let snap = |cx: &mut TestAppContext| -> (Vec<f32>, Vec<f32>) {
            let thumbs: Vec<Option<[f32; 4]>> = bars.iter().map(|b| b.thumbs().last().and_then(|t| t.2)).collect();
            thumbed.set(thumbed.get() + thumbs.iter().flatten().count());
            view.read_with(cx, |v, _| {
                let mut edges = vec![v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown];
                let mut drawn = Vec::new();
                for (k, id) in ids.iter().enumerate() {
                    let pane = v.panes.get(*id).expect("the pane");
                    let b = pane.list.viewport_bounds();
                    edges.extend([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]);
                    drawn.extend([pane.layout.column, pane.layout.left, pane.layout.hang_k]);
                    let top = pane.list.logical_scroll_top().item_ix;
                    for ix in top..(top + 10).min(pane.list.item_count()) {
                        match pane.list.bounds_for_item(ix) {
                            Some(r) => drawn.extend([
                                f32::from(r.origin.x),
                                f32::from(r.origin.y),
                                f32::from(r.size.width),
                                f32::from(r.size.height),
                            ]),
                            None => drawn.extend([f32::NAN; 4]),
                        }
                    }
                    drawn.extend(thumbs[k].unwrap_or([f32::NAN; 4]));
                }
                (edges, drawn)
            })
        };
        let same =
            |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01 || (a.is_nan() && b.is_nan()));
        for rail in [true, false] {
            for fold in [true, false] {
                let case = format!(
                    "{panes} pane(s), the {} {}",
                    if rail { "rail" } else { "right panel" },
                    if fold { "folds" } else { "unfolds" }
                );
                let mut seen = vec![snap(cx)];
                view.update(cx, |v, cx| v.toggle_panel(rail, cx));
                cx.run_until_parked();
                let mut rest = 0;
                while rest < 10 {
                    frame_144(cx, window, &view);
                    seen.push(snap(cx));
                    let busy = view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some() || v.fx.pending().0);
                    rest = if busy { 0 } else { rest + 1 };
                    assert!(seen.len() < 500, "{case}: never landed");
                }
                assert!(thumbed.replace(0) > 0, "{case}: no thumb painted");
                let last = (1..seen.len()).rev().find(|&i| !same(&seen[i].0, &seen[i - 1].0)).unwrap_or(0);
                if last == 0 {
                    fails.push(format!("{case}: nothing moved"));
                    continue;
                }
                if let Some(i) = (last + 1..seen.len()).find(|&i| !same(&seen[i].1, &seen[last].1)) {
                    let what: Vec<usize> = (0..seen[i].1.len().min(seen[last].1.len()))
                        .filter(|&k| !same(&seen[i].1[k..k + 1], &seen[last].1[k..k + 1]))
                        .collect();
                    fails.push(format!(
                        "{case}: the edges stop at frame {last} of {}; frame {i} draws the panes apart (values {what:?}: {:?} -> {:?})",
                        seen.len(),
                        what.iter().map(|&k| seen[last].1[k]).collect::<Vec<_>>(),
                        what.iter().map(|&k| seen[i].1[k]).collect::<Vec<_>>()
                    ));
                }
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// #51 (the design gate of #50, FU1): a panel that yields to the split,
/// or comes back from it, moves on the flight of what made it. With a
/// split on screen it cut from its strip to open (or back) in one frame:
/// a two-pane rail fold re-wrapped both columns twice and landed them 20 px
/// narrower than they started, after a 228 px jump. 1920 px, the rail at
/// 480 and the right panel at 520, at Full, frame by frame (144 Hz) through
/// a split (the right panel yields), the rail's fold (it comes back) and
/// unfold (it yields), and a close (it comes back): no panel's width steps
/// further in a frame than its spring does (its top speed, `D·ω/e`), each
/// pane's column re-wraps at most once a flight, and from the last frame
/// an edge moves nothing drawn in the panes changes.
#[gpui_kit::test]
fn a_yielding_panel_flies_with_the_move_that_made_it(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    view.update(cx, |v, cx| {
        v.panels.rail_collapsed = false;
        v.panels.right_collapsed = false;
        v.panels.rail_width = Some(480);
        v.panels.right_width = Some(520);
        cx.notify();
    });
    frame(cx, window);
    level(cx, crate::motion::Level::Full);
    frame(cx, window);
    // A frame: the panels as drawn, each pane's box and column, and what
    // is drawn in it (the column, its left and the labels' hang, its
    // first rows' boxes).
    type Snap = ([f32; 2], Vec<(crate::pane::PaneId, [f32; 4], f32)>, Vec<f32>);
    let snap = |cx: &mut TestAppContext| -> Snap {
        view.read_with(cx, |v, _| {
            let mut panes = Vec::new();
            let mut drawn = Vec::new();
            for id in v.reading_order() {
                let pane = v.panes.get(id).expect("the pane");
                let b = pane.list.viewport_bounds();
                panes.push((
                    id,
                    [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)],
                    pane.layout.column,
                ));
                drawn.extend([pane.layout.column, pane.layout.left, pane.layout.hang_k]);
                let top = pane.list.logical_scroll_top().item_ix;
                for ix in top..(top + 10).min(pane.list.item_count()) {
                    match pane.list.bounds_for_item(ix) {
                        Some(r) => {
                            drawn.extend([f32::from(r.origin.x), f32::from(r.origin.y), f32::from(r.size.width), f32::from(r.size.height)])
                        }
                        None => drawn.extend([f32::NAN; 4]),
                    }
                }
            }
            ([v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown], panes, drawn)
        })
    };
    let same =
        |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01 || (a.is_nan() && b.is_nan()));
    let edges = |s: &Snap| -> Vec<f32> { s.0.iter().copied().chain(s.1.iter().flat_map(|(_, b, _)| *b)).collect() };
    let mut fails = Vec::new();
    let mut yields = 0;
    for case in ["the split", "the rail folds", "the rail unfolds", "the close"] {
        let mut seen = vec![snap(cx)];
        // Each frame's own instant, what its springs sampled: the test's
        // clock, or a timer's due instant when it woke the frame a hair
        // ahead of it (`Motion::tick`).
        let mut at = vec![view.read_with(cx, |v, _| v.fx.frame_now())];
        let right_was = view.read_with(cx, |v, _| v.panel_fx.sides[1].shown);
        match case {
            "the split" => key(cx, window, "ctrl-\\"),
            "the close" => view.update(cx, |v, cx| {
                let last = *v.reading_order().last().expect("a pane");
                v.in_pane(last, |v| v.close_pane());
                cx.notify();
            }),
            _ => view.update(cx, |v, cx| v.toggle_panel(true, cx)),
        }
        cx.run_until_parked();
        let mut rest = 0;
        while rest < 10 {
            frame_144(cx, window, &view);
            seen.push(snap(cx));
            at.push(view.read_with(cx, |v, _| v.fx.frame_now()));
            let busy = view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some() || v.fx.pending().0);
            rest = if busy { 0 } else { rest + 1 };
            assert!(seen.len() < 500, "{case}: never landed");
        }
        let n = seen.len();
        if (seen[n - 1].0[1] - right_was).abs() > 100. {
            yields += 1;
        }
        // No cut and no doubled step: each panel steps at most its
        // spring's top speed over that frame's time (a sheet spring from
        // rest: travel · ω / e per second), and a pixel for the rounding.
        // A cut is the whole travel, 20 times it; a step taken twice, 2.
        let speed = std::f32::consts::TAU / crate::motion::SHEET_RESPONSE / std::f32::consts::E;
        for i in 0..2 {
            let travel = (seen[n - 1].0[i] - seen[0].0[i]).abs();
            for k in 1..n {
                let (step, dt) = ((seen[k].0[i] - seen[k - 1].0[i]).abs(), (at[k] - at[k - 1]).as_secs_f32());
                if step > travel * speed * dt + 1. {
                    fails.push(format!(
                        "{case}: the {} steps {step} px in frame {k} (travel {travel}, its spring's top step over {:.2} ms {:.1})",
                        ["rail", "right panel"][i],
                        dt * 1e3,
                        travel * speed * dt
                    ));
                }
            }
        }
        // Riding a panel, each pane only grows or only narrows (a split's
        // new pane grows, the pane beside it yields), and no pane moves
        // after the panels have stopped (Shady #54 L2). A pane's edge
        // that travels less than the panel's can stop a frame or two
        // before it (the same step, rounded).
        let rides = matches!(case, "the split" | "the close");
        for id in seen[n - 1].1.iter().map(|p| p.0).filter(|_| rides) {
            let ws: Vec<f32> = seen.iter().filter_map(|s| s.1.iter().find(|p| p.0 == id).map(|p| p.1[2])).collect();
            let (up, down) = ws.windows(2).fold((false, false), |(u, d), w| (u || w[1] > w[0] + 0.5, d || w[1] < w[0] - 0.5));
            if up && down {
                fails.push(format!("{case}: pane {} both grows and narrows: {ws:?}", id.index()));
            }
        }
        let moved = |k: usize, f: &dyn Fn(&Snap) -> Vec<f32>| !same(&f(&seen[k]), &f(&seen[k - 1]));
        let panels_last = (1..n).rev().find(|&k| moved(k, &|s| s.0.to_vec())).unwrap_or(0);
        let panes_last = (1..n).rev().find(|&k| moved(k, &|s| s.1.iter().flat_map(|(_, b, _)| *b).collect())).unwrap_or(0);
        if panes_last > panels_last && panels_last > 0 {
            fails.push(format!("{case}: the panels stop at frame {panels_last}, the panes at {panes_last}"));
        }
        // Each pane's column re-wraps at most once a flight.
        for id in seen[n - 1].1.iter().map(|p| p.0) {
            let cols: Vec<f32> = seen.iter().filter_map(|s| s.1.iter().find(|p| p.0 == id).map(|p| p.2)).collect();
            let wraps = cols.windows(2).filter(|w| (w[0] - w[1]).abs() > 0.01).count();
            if wraps > 1 {
                fails.push(format!("{case}: pane {} re-wraps {wraps} times: {cols:?}", id.index()));
            }
        }
        // Nothing moves after the last frame an edge moves.
        let last = (1..n).rev().find(|&k| !same(&edges(&seen[k]), &edges(&seen[k - 1]))).unwrap_or(0);
        if last == 0 {
            fails.push(format!("{case}: nothing moved"));
        } else if let Some(k) = (last + 1..n).find(|&k| !same(&seen[k].2, &seen[last].2)) {
            fails.push(format!("{case}: the edges stop at frame {last} of {n}; frame {k} draws the panes apart"));
        }
    }
    assert_eq!(yields, 4, "the right panel yields or comes back in every case: {fails:#?}");
    assert!(fails.is_empty(), "{fails:#?}");
}

/// The panels as a test wants them: the rail open or folded, both widths.
fn panels_at(cx: &mut TestAppContext, view: &Entity<AppView>, rail_open: bool, rail: u16, right: u16) {
    view.update(cx, |v, cx| {
        v.panels.rail_collapsed = !rail_open;
        v.panels.right_collapsed = false;
        v.panels.rail_width = Some(rail);
        v.panels.right_width = Some(right);
        cx.notify();
    });
}

/// Pane `id`'s width in the layout as the last frame drew it.
fn drawn_w(cx: &mut TestAppContext, view: &Entity<AppView>, id: crate::pane::PaneId) -> f32 {
    view.read_with(cx, |v, _| {
        let (w, h) = v.centre.get();
        v.panes.drawn.rects(crate::split::Rect { x: 0., y: 0., w, h }).iter().find(|(i, _)| *i == id).map_or(0., |(_, r)| r.w)
    })
}

/// Shady #54 M1: with Settings on screen in the pane that stays, a close
/// is drawn without the right panel (a view has none), so the layout has
/// nothing to ride: the closing share goes on the layout's own spring,
/// frame by frame, not frozen for the hidden panel's flight and then gone
/// in one frame.
#[gpui_kit::test]
fn a_close_beside_a_view_flies_on_its_own_spring(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    panels_at(cx, &view, true, 480, 520);
    frame(cx, window);
    level(cx, crate::motion::Level::Full);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    let order = view.read_with(cx, |v, _| v.reading_order());
    let (kept, gone) = (order[0], order[1]);
    view.update(cx, |v, cx| {
        v.focus_pane(kept, crate::pane::KeysTo::Pane, cx);
        v.model.tabs.open_view(crate::tabs::Kind::Settings);
        v.reconcile(None);
        cx.notify();
    });
    for _ in 0..60 {
        frame_144(cx, window, &view);
    }
    assert!(view.read_with(cx, |v, _| v.model.tabs.view().is_some()), "Settings on screen in the pane that stays");
    let right_was = view.read_with(cx, |v, _| v.panel_fx.sides[1].shown);
    view.update(cx, |v, cx| {
        v.in_pane(gone, |v| v.close_pane());
        cx.notify();
    });
    cx.run_until_parked();
    // From the close's first frame (the centre loses the right panel's
    // strip in it: a view has none, as on main).
    frame_144(cx, window, &view);
    let mut widths = vec![drawn_w(cx, &view, kept)];
    let mut rest = 0;
    while rest < 10 {
        frame_144(cx, window, &view);
        widths.push(drawn_w(cx, &view, kept));
        let busy = view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some() || v.fx.pending().0);
        rest = if busy { 0 } else { rest + 1 };
        assert!(widths.len() < 500, "never landed");
    }
    assert!((view.read_with(cx, |v, _| v.panel_fx.sides[1].shown) - right_was).abs() > 100., "the right panel comes back from its strip");
    let travel = widths[widths.len() - 1] - widths[0];
    let top = std::f32::consts::TAU / crate::motion::SHEET_RESPONSE * 0.006944 / std::f32::consts::E;
    let step = widths.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0., f32::max);
    let moving = widths.windows(2).filter(|w| (w[1] - w[0]).abs() > 0.5).count();
    assert!(travel > 100., "the kept pane takes the closing share: {widths:?}");
    // From its first frames (Shady #54 n6: not a freeze, then a fast
    // flight that keeps under the step).
    assert!(widths[..4].windows(2).any(|w| w[1] - w[0] > 0.5), "the kept pane widens within the first 3 frames: {widths:?}");
    assert!(
        step <= travel * top * 1.25 + 2. && moving >= 10,
        "the kept pane steps at most its spring's top step ({:.1} px), in {moving} frames: {step} px; {widths:?}",
        travel * top
    );
}

/// Shady #54 M2: a ride whose ends move under it (the rail folds or
/// unfolds, the window is resized) hands the layout back to its own
/// spring from where it is drawn: the new pane doesn't stall and then
/// jump the rest of the way as the panel lands. No frame steps it more
/// than a tenth of its landed width (but the resize's own), and it lands.
#[gpui_kit::test]
fn a_ride_whose_ends_move_hands_the_layout_back(cx: &mut TestAppContext) {
    let mut fails = Vec::new();
    for case in ["the rail unfolds", "the rail folds", "a resize"] {
        let (w, rail_open) = if case == "the rail unfolds" { (1400., false) } else { (1920., true) };
        let (window, view) = demo_split(cx, w, 1080.);
        panels_at(cx, &view, rail_open, 480, 520);
        frame(cx, window);
        level(cx, crate::motion::Level::Full);
        frame(cx, window);
        let before = view.read_with(cx, |v, _| v.panes.ids());
        key(cx, window, "ctrl-\\");
        cx.run_until_parked();
        frame_144(cx, window, &view);
        let new = view.read_with(cx, |v, _| v.panes.ids().into_iter().find(|i| !before.contains(i))).expect("a new pane");
        assert!(
            view.read_with(cx, |v, _| v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some())),
            "{case}: the split rides the right panel"
        );
        let mut widths = vec![drawn_w(cx, &view, new)];
        let mut rest = 0;
        for k in 1.. {
            if k == 15 {
                match case {
                    "a resize" => VisualTestContext::from_window(window, cx).simulate_resize(size(px(1700.), px(1080.))),
                    _ => view.update(cx, |v, cx| v.toggle_panel(true, cx)),
                }
                cx.run_until_parked();
            }
            frame_144(cx, window, &view);
            widths.push(drawn_w(cx, &view, new));
            let busy = view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some() || v.fx.pending().0);
            rest = if busy || k <= 15 { 0 } else { rest + 1 };
            if rest == 10 || k == 500 {
                break;
            }
        }
        let landed = widths[widths.len() - 1];
        // The resize's own frame scales the layout with the window (a cut:
        // the window manager's, as on main).
        let step = widths
            .windows(2)
            .enumerate()
            .filter(|(i, _)| !(case == "a resize" && *i == 14))
            .map(|(_, w)| (w[1] - w[0]).abs())
            .fold(0., f32::max);
        // A tenth of the landed width a frame: about 2.2x the `sheet`
        // spring's top step (4.46 % of its travel a frame at 144 Hz), room
        // for main's double drive after the handback (the ratios lerped over
        // a centre still moving: up to near 2x). It catches a stall's jump
        // (292-357 px on `eae7eda`), not a handback that restarts from a
        // little off where the layout was drawn (Shady #54 n5).
        if rest < 10 {
            fails.push(format!("{case}: never landed"));
        } else if step > landed * 0.1 {
            fails.push(format!("{case}: the new pane steps {step} px in a frame (landed {landed}): {widths:?}"));
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Shady #54 L1: at 125 % a panel that a layout rides lands in the frame
/// it is drawn at its target, as at 100 %, with a target off the device
/// grid (the right panel at 481, snapped 480.8), and nothing moves after.
#[gpui_kit::test]
fn a_ride_lands_on_arrival_at_a_fractional_scale(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    VisualTestContext::from_window(window, cx).simulate_scale_factor_change(1.25);
    panels_at(cx, &view, true, 480, 481);
    frame(cx, window);
    level(cx, crate::motion::Level::Full);
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    for _ in 0..60 {
        frame_144(cx, window, &view);
    }
    let target = view.read_with(cx, |v, _| v.fx.snap(481.));
    assert!((target - 481.).abs() > 0.01, "481 is off the device grid at 125 %: {target}");
    let last = view.read_with(cx, |v, _| *v.reading_order().last().expect("a pane"));
    view.update(cx, |v, cx| {
        v.in_pane(last, |v| v.close_pane());
        cx.notify();
    });
    cx.run_until_parked();
    let mut seen = Vec::new();
    let mut rest = 0;
    while rest < 10 {
        frame_144(cx, window, &view);
        let s = view.read_with(cx, |v, _| {
            (v.panel_fx.sides[1].shown, v.panel_fx.moving(), v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some()))
        });
        seen.push(s);
        rest = if s.1 || s.2 || view.read_with(cx, |v, _| v.fx.pending().0) { 0 } else { rest + 1 };
        assert!(seen.len() < 500, "never landed");
    }
    assert!(seen.iter().any(|s| s.2), "the close rides the right panel: {seen:?}");
    // Drawn at its target (within half a pixel, as rounded) only once it
    // has landed, and it stays where it landed.
    let late: Vec<usize> = (0..seen.len()).filter(|&k| (seen[k].0 - target).abs() < 0.5 && seen[k].1).collect();
    let first_rest = seen.iter().position(|s| !s.1).expect("it rests");
    let after: Vec<f32> = seen[first_rest..].iter().map(|s| s.0).filter(|w| (w - seen[first_rest].0).abs() > 0.001).collect();
    assert!(
        late.is_empty() && after.is_empty(),
        "lands in the frame it arrives: still moving at its target in frames {late:?}, moves after {after:?}; {seen:?}"
    );
}

/// #52: a layout's flight lands in the frame it draws the landed layout,
/// as the side panels do (#50), not when its spring settles 6 or 7 frames
/// (144 Hz) later. In that frame the flight is over, a closing pane goes
/// and the dividers take their handles (none in any frame before it);
/// nothing asks for a frame or an app timer after it (the beat's apart:
/// the demo's sessions run). A composer's inset that flies with it stops
/// in the frame its composer is drawn where it lands. A split and a close
/// on the layout's own spring and riding the right panel (#51), a split
/// down (the top pane's composer leaves the window's foot: its inset
/// flies), and a 2-pane rail fold. On `3f92c1d` the handles came 6-7
/// frames after the panes stopped (one, riding), the settling woke the
/// app, and the inset moved on 6-7 frames after its composer stopped.
#[gpui_kit::test]
fn a_layouts_flight_lands_with_its_handles_and_nothing_wakes_after(cx: &mut TestAppContext) {
    use crate::motion::Wake;
    let mut fails = Vec::new();
    for case in ["a split", "a split riding", "a close", "a close riding", "a split down", "the rail folds, two panes"] {
        let riding = case.ends_with("riding");
        let (window, view) = demo_split(cx, 1920., 1080.);
        panels_at(cx, &view, riding || case.starts_with("the rail"), 480, 520);
        frame(cx, window);
        level(cx, crate::motion::Level::Full);
        frame(cx, window);
        if case.starts_with("a close") || case.starts_with("the rail") {
            key(cx, window, "ctrl-\\");
            land(cx, window, &view);
            for _ in 0..60 {
                frame_144(cx, window, &view);
            }
        }
        // A frame: the panels' edges and every pane's box as drawn (a
        // closing share too); then what waits for the landing. Each frame
        // drawn, from the move's (#60 L1: a step can draw two).
        let seen = draws(cx, &view, |v| {
            let (w, h) = v.centre.get();
            let mut edges = vec![v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown];
            for (_, r) in v.panes.drawn.rects(crate::split::Rect { x: 0., y: 0., w, h }) {
                edges.extend([r.x, r.y, r.w, r.h]);
            }
            let insets = v.panes.ids().iter().filter_map(|id| v.panes.get(*id)).any(|p| p.foot_moving.get() || p.top_moving.get());
            let (frame, timer) = v.fx.pending();
            let woke = frame || (timer.is_some() && v.fx.wakes() == Some(Wake::App));
            let ride = v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some());
            (edges, v.handles_drawn.get(), v.panes.flight.is_some(), !v.panes.closing.is_empty(), insets, woke, ride)
        });
        match case {
            "a split" | "a split riding" => key(cx, window, "ctrl-\\"),
            "a split down" => key(cx, window, "ctrl-shift-\\"),
            "the rail folds, two panes" => view.update(cx, |v, cx| v.toggle_panel(true, cx)),
            _ => view.update(cx, |v, cx| {
                let last = *v.reading_order().last().expect("a pane");
                v.in_pane(last, |v| v.close_pane());
                cx.notify();
            }),
        }
        cx.run_until_parked();
        let ids = view.read_with(cx, |v, _| v.panes.ids());
        // Each pane's composer as drawn, from its pane's foot (its inset)
        // and its height (the inset above it), and whether its insets move.
        let composers = |cx: &mut TestAppContext| -> Vec<(f32, f32, bool)> {
            ids.iter()
                .map(|id| {
                    let pane = drawn(cx, window, format!("pane {}", id.index())).unwrap_or([0.; 4]);
                    let c = drawn(cx, window, format!("composer-frame {}", id.index())).unwrap_or([0.; 4]);
                    let moving = view.read_with(cx, |v, _| v.panes.get(*id).is_some_and(|p| p.foot_moving.get() || p.top_moving.get()));
                    (pane[1] + pane[3] - c[1] - c[3], c[3], moving)
                })
                .collect()
        };
        let mut insets = vec![composers(cx)];
        let mut rest = 0;
        while rest < 10 {
            frame_144(cx, window, &view);
            insets.push(composers(cx));
            let busy = view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some() || v.fx.pending().0);
            rest = if busy { 0 } else { rest + 1 };
            assert!(insets.len() < 500, "{case}: never landed");
        }
        view.update(cx, |v, _| v.on_draw = None);
        let seen = seen.take();
        let same = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01);
        let last = (1..seen.len()).rev().find(|&i| !same(&seen[i].0, &seen[i - 1].0)).unwrap_or(0);
        if last < 10 {
            fails.push(format!("{case}: it didn't fly ({last} frames)"));
            continue;
        }
        if seen.iter().any(|s| s.6) != (riding || case.starts_with("the rail")) && case != "the rail folds, two panes" {
            fails.push(format!("{case}: rides {}", seen.iter().any(|s| s.6)));
        }
        // An inset that flew stops moving in the frame its composer is
        // drawn where it lands, not as its spring settles after.
        for k in 0..ids.len() {
            if !insets.iter().any(|f| f[k].2) {
                continue;
            }
            let at = |i: usize| (insets[i][k].0, insets[i][k].1);
            let arrives = (1..insets.len()).rev().find(|&i| at(i) != at(i - 1)).unwrap_or(0);
            let late: Vec<usize> = (arrives..insets.len()).filter(|&i| insets[i][k].2).collect();
            if !late.is_empty() {
                fails.push(format!(
                    "{case}: pane {k}'s composer is drawn where it lands at frame {arrives}, its inset moves on in {late:?}"
                ));
            }
        }
        if case == "a split down" && !insets.iter().any(|f| f.iter().any(|c| c.2)) {
            fails.push(format!("{case}: no inset flew"));
        }
        let panes = view.read_with(cx, |v, _| v.panes.len());
        let (_, handles, flying, closing, insets, woke, _) = seen[last].clone();
        let early: Vec<usize> = (1..last).filter(|&k| seen[k].1).collect();
        if flying || closing || insets || handles != (panes > 1) || woke || !early.is_empty() {
            fails.push(format!(
                "{case}: in the frame it lands ({last} of {}): in flight {flying}, a closing pane {closing}, an inset moving {insets}, \
                 handles {handles} (panes {panes}; before it in {early:?}), a frame or an app timer asked for {woke}; the handles come at {:?}",
                seen.len(),
                seen.iter().position(|s| s.1)
            ));
        }
        if seen[last + 1..].iter().any(|s| s.5) {
            fails.push(format!("{case}: woke after the landing (frame {last})"));
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Shady #54 L3: Settings opened in the pane a close leaves while the
/// close rides the right panel: from then the layout is drawn without the
/// right panel (a view has none, as M1), so it flies on from where it is
/// drawn on its own spring. The closing share goes on closing frame by
/// frame, never back to where it started (on `3f92c1d` it jumped back to
/// its full share, held, then went in one frame).
#[gpui_kit::test]
fn a_view_opened_mid_close_hands_the_layout_back(cx: &mut TestAppContext) {
    let (window, view) = demo_split(cx, 1920., 1080.);
    panels_at(cx, &view, true, 480, 520);
    frame(cx, window);
    level(cx, crate::motion::Level::Full);
    key(cx, window, "ctrl-\\");
    land(cx, window, &view);
    for _ in 0..60 {
        frame_144(cx, window, &view);
    }
    let order = view.read_with(cx, |v, _| v.reading_order());
    let (kept, gone) = (order[0], order[1]);
    view.update(cx, |v, cx| {
        v.focus_pane(kept, crate::pane::KeysTo::Pane, cx);
        v.in_pane(gone, |v| v.close_pane());
        cx.notify();
    });
    cx.run_until_parked();
    let share = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let (w, h) = v.centre.get();
            v.panes.drawn.rects(crate::split::Rect { x: 0., y: 0., w, h }).iter().find(|(i, _)| *i == gone).map_or(0., |(_, r)| r.w / w)
        })
    };
    let mut shares = vec![share(cx)];
    let mut rest = 0;
    for k in 1.. {
        if k == 10 {
            assert!(
                view.read_with(cx, |v, _| v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some())),
                "the close rides the right panel"
            );
            view.update(cx, |v, cx| {
                v.model.tabs.open_view(crate::tabs::Kind::Settings);
                v.reconcile(None);
                cx.notify();
            });
            cx.run_until_parked();
        }
        frame_144(cx, window, &view);
        shares.push(share(cx));
        let busy = view.read_with(cx, |v, _| v.panel_fx.moving() || v.panes.flight.is_some() || v.fx.pending().0);
        rest = if busy || k <= 10 { 0 } else { rest + 1 };
        if rest == 10 || k == 500 {
            break;
        }
    }
    assert!(view.read_with(cx, |v, _| v.model.tabs.view().is_some()), "Settings on screen in the pane that stays");
    assert!(rest == 10 && shares[shares.len() - 1] == 0., "it lands closed: {shares:?}");
    // A share's step a frame: at most its spring's top step (`sheet` from
    // rest, 4.46 % of the travel a frame at 144 Hz) of the share it
    // started at, with room for whole pixels.
    let top = std::f32::consts::TAU / crate::motion::SHEET_RESPONSE * 0.006944 / std::f32::consts::E;
    let back: Vec<usize> = (1..shares.len()).filter(|&k| shares[k] > shares[k - 1] + 0.002).collect();
    let step = shares.windows(2).map(|w| w[0] - w[1]).fold(0., f32::max);
    let moving = shares[10..].windows(2).filter(|w| w[0] - w[1] > 0.0005).count();
    assert!(
        back.is_empty() && step <= shares[0] * top * 1.25 + 0.002 && moving >= 10,
        "the closing share only closes, on its spring ({:.4} a frame), from the view on in {moving} frames: back in {back:?}, \
         steps {step:.4}; {shares:?}",
        shares[0] * top
    );
}

/// Design gate F5 (#44 r3): after Join lands, the pane kept's worker chips
/// take their landed grid in the landing frame (one a line in a quarter of
/// the centre, two in the whole): no chip slides after the panes stop.
#[gpui_kit::test]
fn the_chips_take_their_landed_grid_as_join_lands(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view) = demo_split(cx, 1920., 1080.);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    // The narrowest pane (the second split's), kept.
    let kept = view.read_with(cx, |v, _| v.reading_order()[2]);
    view.update(cx, |v, cx| v.focus_pane(kept, crate::pane::KeysTo::Pane, cx));
    let names = ["scout", "reviewer", "trace-cart", "flake-stats"];
    let rows: Vec<serde_json::Value> = (11..=14u64)
        .map(|n| {
            json!({"subagent_id": n, "agent_name": names[n as usize - 11], "status": "running", "cancel_requested": false,
                "elapsed_secs": 9., "finished_elapsed": null, "step": "ls", "tools": n, "handle_id": format!("sa_{n}")})
        })
        .collect();
    let ev = synaps_client::protocol::decode_event(&json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event");
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&ev);
        cx.notify();
    });
    level(cx, crate::motion::Level::Full);
    for _ in 0..200 {
        frame_144(cx, window, &view);
    }
    let per =
        |cx: &mut TestAppContext| view.read_with(cx, |v, cx| crate::tray::grid(v.panes.cur.layout.column, &crate::theme::metrics(cx)).0);
    assert_eq!(per(cx), 1, "one chip a line in a quarter of the centre");
    let chips =
        |cx: &mut TestAppContext| -> Vec<Option<[f32; 4]>> { (11..=14).map(|id| drawn(cx, window, format!("chip {id}"))).collect() };
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JoinPanes, w, cx))).unwrap();
    land(cx, window, &view);
    assert!(per(cx) >= 2, "two or more a line in the whole centre");
    let landed = chips(cx);
    assert!(landed.iter().filter(|c| c.is_some()).count() >= 2, "chips drawn: {landed:?}");
    for f in 0..30 {
        frame_144(cx, window, &view);
        assert_eq!(chips(cx), landed, "frame {f} after the landing: no chip moves");
    }
}

// ── Step 4: drag and drop (split/architecture.md §4, split.md §2–§3) ──

/// The mouse at `at`, as [`mouse`], with `m` held.
fn mouse_m(cx: &mut TestAppContext, window: AnyWindowHandle, at: Point<Pixels>, how: &str, m: gpui_kit::Modifiers) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    let mut vcx = VisualTestContext::from_window(window, cx);
    match how {
        "down" => {
            vcx.simulate_event(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: 1, first_mouse: false })
        }
        "up" => vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: 1 }),
        "hover" => vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: m }),
        _ => vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: m }),
    }
    vcx.run_until_parked();
}

/// The middle of what `sel` names, as drawn.
fn mid(cx: &mut TestAppContext, window: AnyWindowHandle, sel: String) -> Point<Pixels> {
    let b = drawn(cx, window, sel.clone()).unwrap_or_else(|| panic!("{sel} drawn"));
    gpui_kit::point(px(b[0] + b[2] / 2.), px(b[1] + b[3] / 2.))
}

/// A drag by real mouse input from `from` (pressed there, past GPUI's
/// 2 px threshold) to `to`, frames drawn between; let go there (`drop`)
/// with `m` held, or left held.
fn drag_to(cx: &mut TestAppContext, window: AnyWindowHandle, from: Point<Pixels>, to: Point<Pixels>, m: gpui_kit::Modifiers, drop: bool) {
    let none = gpui_kit::Modifiers::default();
    mouse_m(cx, window, from, "hover", none);
    mouse_m(cx, window, from, "down", none);
    mouse_m(cx, window, gpui_kit::point(from.x + px(8.), from.y + px(6.)), "move", none);
    frame(cx, window);
    let half = gpui_kit::point((from.x + to.x) / 2., (from.y + to.y) / 2.);
    mouse_m(cx, window, half, "move", none);
    frame(cx, window);
    mouse_m(cx, window, to, "move", m);
    frame(cx, window);
    if drop {
        mouse_m(cx, window, to, "up", m);
        frame(cx, window);
    }
}

/// A point in pane `id`'s body at `(fx, fy)` of it (under its tab strip).
fn in_body(cx: &mut TestAppContext, window: AnyWindowHandle, id: crate::pane::PaneId, fx: f32, fy: f32) -> Point<Pixels> {
    let b = match drawn(cx, window, format!("pane {}", id.index())) {
        Some(b) => b,
        // One pane: the centre.
        None => drawn(cx, window, "pane-centre".into()).expect("the centre drawn"),
    };
    let strip = crate::tabstrip::TAB_H;
    gpui_kit::point(px(b[0] + fx * b[2]), px(b[1] + strip + fy * (b[3] - strip)))
}

/// The next attach at the daemon. A switch soon after the last one waits
/// for the attach gate ([`crate::tabs::AttachGate`]): its timer runs on
/// the test's clock, so frames are drawn and the clock moved meanwhile.
fn next_attach(cx: &mut TestAppContext, window: AnyWindowHandle, attaches: &mpsc::Receiver<Conn>, what: &str) -> Conn {
    crate::test_waits::next_attach(cx, window, attaches, what)
}

/// B opened as a new tab of the focused pane, attached.
fn open_b(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<AppView>,
    attaches: &mpsc::Receiver<Conn>,
    events: &async_channel::Receiver<Event>,
) -> Conn {
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
    });
    let mut b = next_attach(cx, window, attaches, "B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, view, events, "B attached", |v| v.model.lives[v.panes.cur.live].is_active(B));
    b
}

/// Pane `id`'s tabs' sessions, in order.
fn tabs_of(cx: &mut TestAppContext, view: &Entity<AppView>, id: crate::pane::PaneId) -> Vec<String> {
    view.read_with(cx, |v, _| {
        let p = v.panes.get(id).expect("the pane");
        let tabs = if p.id == v.panes.cur.id { &v.model.tabs } else { &p.parked.tabs };
        tabs.iter().filter_map(|t| t.session.clone()).collect()
    })
}

/// The tab id of `session` in pane `id`.
fn tab_id(cx: &mut TestAppContext, view: &Entity<AppView>, id: crate::pane::PaneId, session: &str) -> u64 {
    view.read_with(cx, |v, _| {
        let p = v.panes.get(id).expect("the pane");
        let tabs = if p.id == v.panes.cur.id { &v.model.tabs } else { &p.parked.tabs };
        tabs.iter().find(|t| t.session.as_deref() == Some(session)).map(|t| t.id).expect("its tab")
    })
}

/// split.md §2, A3: a tab dragged to a pane's right edge by real mouse
/// input splits it there and moves into the new pane (focused, keys
/// there); the preview showed NEW PANE over the right half while over
/// the band; what is typed after goes to the new pane's composer, never
/// the pane it left.
#[gpui_kit::test]
fn a_tab_dragged_to_the_right_edge_splits_and_moves(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-right", None);
    let _b = open_b(cx, window, &view, &attaches, &events);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    assert_eq!(tabs_of(cx, &view, p0), [A, B]);
    let tab = tab_id(cx, &view, p0, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.92, 0.5);
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    let hint = view.read_with(cx, |v, _| v.dnd.hint);
    assert_eq!(hint.map(|h| (h.pane, h.zone)), Some((p0, Some(crate::dnd::Zone::Right))), "over the right band");
    let pv = drawn(cx, window, "drop-preview".into()).expect("the preview");
    let pb = drawn(cx, window, "pane-centre".into()).expect("the centre");
    assert!(pv[0] > pb[0] + pb[2] / 2. - 1. && pv[0] < pb[0] + pb[2] / 2. + 8., "the right half: {pv:?} in {pb:?}");
    mouse_m(cx, window, to, "up", gpui_kit::Modifiers::default());
    frame(cx, window);
    let (n, focused) = view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused));
    assert_eq!(n, 2, "split");
    assert_ne!(focused, p0, "the new pane focused");
    assert_eq!(tabs_of(cx, &view, focused), [B], "the tab in the new pane");
    assert_eq!(tabs_of(cx, &view, p0), [A], "and out of the one it left");
    let rects = tree_rects(cx, &view);
    let (r0, r1) = (rects.iter().find(|r| r.0 == p0).unwrap().1, rects.iter().find(|r| r.0 == focused).unwrap().1);
    assert!(r1.x > r0.x, "on the right: {rects:?}");
    for _ in 0..3 {
        frame(cx, window);
    }
    VisualTestContext::from_window(window, cx).simulate_input("to b");
    frame(cx, window);
    view.read_with(cx, |v, cx| {
        assert_eq!(v.panes.get(focused).unwrap().composer.read(cx).value().to_string(), "to b", "typed into the new pane");
        assert_eq!(v.panes.get(p0).unwrap().composer.read(cx).value().to_string(), "", "nothing in the pane it left");
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §2, A4: a rail row dragged to a pane's bottom band splits it
/// down and opens the row's session in the new pane; nothing moves (the
/// first pane keeps its tab).
#[gpui_kit::test]
fn a_rail_row_dragged_to_the_bottom_splits_and_opens(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-rail", None);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let key = view.read_with(cx, |v, _| {
        v.rail.rail.rows.iter().find(|r| r.action == crate::named::RowAction::Attach(B.into())).map(|r| r.key).expect("B's row")
    });
    let from = mid(cx, window, format!("rail-row {key}"));
    let to = in_body(cx, window, p0, 0.5, 0.95);
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    assert_eq!(view.read_with(cx, |v, _| v.dnd.hint.map(|h| (h.pane, h.zone))), Some((p0, Some(crate::dnd::Zone::Bottom))));
    assert!(drawn(cx, window, "drop-preview".into()).is_some(), "the preview");
    mouse_m(cx, window, to, "up", gpui_kit::Modifiers::default());
    frame(cx, window);
    let mut b = next_attach(cx, window, &attaches, "B's attach at the daemon");
    b.send(attached(B, None));
    let new = view.read_with(cx, |v, _| v.panes.focused);
    feed(cx, window, &view, &events, "B attached in the new pane", |v| {
        v.panes.focused == new && v.model.lives[v.panes.cur.live].is_active(B)
    });
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2);
    assert_ne!(new, p0);
    assert_eq!(tabs_of(cx, &view, new), [B], "opened in the new pane");
    assert_eq!(tabs_of(cx, &view, p0), [A], "nothing moved");
    let rects = tree_rects(cx, &view);
    let (r0, r1) = (rects.iter().find(|r| r.0 == p0).unwrap().1, rects.iter().find(|r| r.0 == new).unwrap().1);
    assert!(r1.y > r0.y && (r1.x - r0.x).abs() < 0.5, "below: {rects:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §2–§3, A3, A9: a tab dragged to another pane's centre moves
/// there (its pane keeps its other tab); held Ctrl at the drop, it copies
/// (it stays where it was too). The pane the drag started in has the
/// keys during the drag (its mouse-down focused it), the drop's pane
/// after: typing never lands in another.
#[gpui_kit::test]
fn a_tab_dragged_to_another_panes_centre_moves_and_ctrl_copies(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-centre", None);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    key(cx, window, "ctrl-\\");
    let p1 = view.read_with(cx, |v, _| v.panes.focused);
    let _b = open_b(cx, window, &view, &attaches, &events);
    assert_eq!(tabs_of(cx, &view, p1), [A, B]);
    // Focus in p0 first: the drag's mouse-down moves it to p1.
    view.update(cx, |v, cx| v.focus_pane(p0, crate::pane::KeysTo::Pane, cx));
    for _ in 0..3 {
        frame(cx, window);
    }
    let tab = tab_id(cx, &view, p1, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.5, 0.5);
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    assert_eq!(view.read_with(cx, |v, _| v.dnd.hint.map(|h| (h.pane, h.zone))), Some((p0, Some(crate::dnd::Zone::Center))));
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), p1, "the pane the drag started in is focused");
    // The press on the tab left no pane's input the keys (a click outside
    // a field): typed during the drag lands in no composer.
    assert!(!keys_in(cx, window, &view, p0, false) && !keys_in(cx, window, &view, p1, false), "no composer has the keys mid-drag");
    VisualTestContext::from_window(window, cx).simulate_input("mid");
    frame(cx, window);
    // GPUI hovers nothing after a key until the pointer moves (its
    // `last_input_was_keyboard`): a pixel's move, as a hand makes.
    mouse_m(cx, window, gpui_kit::point(to.x + px(1.), to.y), "move", gpui_kit::Modifiers::default());
    frame(cx, window);
    mouse_m(cx, window, gpui_kit::point(to.x + px(1.), to.y), "up", gpui_kit::Modifiers::default());
    frame(cx, window);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), p0, "the drop's pane focused");
    assert_eq!(tabs_of(cx, &view, p0), [A, B], "moved in");
    assert_eq!(tabs_of(cx, &view, p1), [A], "and out");
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current_session().map(str::to_string)), Some(B.to_string()), "shown there");
    VisualTestContext::from_window(window, cx).simulate_input("after");
    frame(cx, window);
    view.read_with(cx, |v, cx| {
        let text = |id| v.panes.get(id).unwrap().composer.read(cx).value().to_string();
        assert_eq!(text(p0), "after", "typed after the drop: the drop's pane, and nothing from mid-drag");
        assert_eq!(text(p1), "", "nothing in the pane it left");
    });
    // Ctrl held at the drop: a copy.
    let tab = tab_id(cx, &view, p0, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p1, 0.5, 0.5);
    let ctrl = gpui_kit::Modifiers { control: true, ..Default::default() };
    drag_to(cx, window, from, to, ctrl, true);
    assert_eq!(tabs_of(cx, &view, p1), [A, B], "copied in");
    assert_eq!(tabs_of(cx, &view, p0), [A, B], "and kept");
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), p1);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The dnd round 2 tests' start: two panes, `p0` with A and `p1`
/// (focused) with A and B; the window, its view, the client, A's and B's
/// connections, and the panes.
fn two_panes(
    cx: &mut TestAppContext,
    name: &str,
) -> (AnyWindowHandle, Entity<AppView>, Client, Kept, crate::pane::PaneId, crate::pane::PaneId) {
    let (window, view, client, a, attaches, events) = on_a(cx, name, None);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    key(cx, window, "ctrl-\\");
    let p1 = view.read_with(cx, |v, _| v.panes.focused);
    let b = open_b(cx, window, &view, &attaches, &events);
    assert_eq!(tabs_of(cx, &view, p1), [A, B]);
    assert_eq!(tabs_of(cx, &view, p0), [A]);
    for _ in 0..3 {
        frame(cx, window);
    }
    (window, view, client, (a, b, attaches, events), p0, p1)
}

/// What a test keeps alive and never reads: A's and B's connections, the
/// daemon's attaches, the client's events.
type Kept = (Conn, Conn, mpsc::Receiver<Conn>, async_channel::Receiver<Event>);

/// Shady #48 L2: Ctrl pressed after the last move (stop, press, let go)
/// is what the drop reads, and the preview follows it as it changes: a
/// pane's own last tab to its edge is nothing as a move, a new pane as a
/// copy; a tab let go on another pane's centre with Ctrl pressed only
/// after it stopped is a copy.
#[gpui_kit::test]
fn ctrl_pressed_after_the_last_move_copies_and_the_preview_follows(cx: &mut TestAppContext) {
    let (window, view, client, _conns, p0, p1) = two_panes(cx, "dnd-ctrl-late");
    // Wide enough for a half pane to split again (its minimum).
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    for _ in 0..3 {
        frame(cx, window);
    }
    let none = gpui_kit::Modifiers::default();
    let ctrl = gpui_kit::Modifiers { control: true, ..Default::default() };
    let hint = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.dnd.hint.map(|h| (h.pane, h.zone)));
    // p0's own last tab over its right edge: nothing, until Ctrl.
    let tab = tab_id(cx, &view, p0, A);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.92, 0.5);
    drag_to(cx, window, from, to, none, false);
    assert_eq!(hint(cx), Some((p0, None)), "a move of its last tab to its own edge: nothing");
    VisualTestContext::from_window(window, cx).simulate_modifiers_change(ctrl);
    frame(cx, window);
    assert!(view.read_with(cx, |v, _| v.dnd.copy), "Ctrl heard mid-drag");
    assert_eq!(hint(cx), Some((p0, Some(crate::dnd::Zone::Right))), "Ctrl pressed with the pointer still: a copy, a new pane");
    assert!(drawn(cx, window, "drop-preview".into()).is_some(), "and its preview drawn");
    VisualTestContext::from_window(window, cx).simulate_modifiers_change(none);
    frame(cx, window);
    assert_eq!(hint(cx), Some((p0, None)), "let go of: nothing again");
    key(cx, window, "escape");
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2);
    // B from p1 to p0's centre, still; Ctrl pressed, then let go there.
    let tab = tab_id(cx, &view, p1, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.5, 0.5);
    drag_to(cx, window, from, to, none, false);
    assert_eq!(hint(cx), Some((p0, Some(crate::dnd::Zone::Center))));
    VisualTestContext::from_window(window, cx).simulate_modifiers_change(ctrl);
    frame(cx, window);
    mouse_m(cx, window, to, "up", ctrl);
    frame(cx, window);
    frame(cx, window);
    assert_eq!(tabs_of(cx, &view, p0), [A, B], "copied in");
    assert_eq!(tabs_of(cx, &view, p1), [A, B], "and kept: Ctrl was held at the drop");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #48 L3: a key typed mid-drag leaves GPUI hovering nothing until
/// the pointer moves; let go without moving, the drop still lands where
/// it was let go: on a pane's body, its zone; on its tab strip, that
/// pane's centre.
#[gpui_kit::test]
fn a_key_typed_mid_drag_then_a_release_that_never_moved_still_drops(cx: &mut TestAppContext) {
    let (window, view, client, _conns, p0, p1) = two_panes(cx, "dnd-typed");
    let none = gpui_kit::Modifiers::default();
    let tab = tab_id(cx, &view, p1, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.5, 0.5);
    drag_to(cx, window, from, to, none, false);
    VisualTestContext::from_window(window, cx).simulate_input("x");
    frame(cx, window);
    mouse_m(cx, window, to, "up", none);
    frame(cx, window);
    frame(cx, window);
    assert_eq!(tabs_of(cx, &view, p0), [A, B], "dropped on p0's centre: moved in");
    assert_eq!(tabs_of(cx, &view, p1), [A], "and out");
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), p0, "the drop's pane focused");
    // Back onto p1's tab strip (its A tab), a key typed, let go there.
    let tab = tab_id(cx, &view, p0, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let onto = tab_id(cx, &view, p1, A);
    let to = mid(cx, window, format!("tab {onto}"));
    drag_to(cx, window, from, to, none, false);
    VisualTestContext::from_window(window, cx).simulate_input("y");
    frame(cx, window);
    mouse_m(cx, window, to, "up", none);
    frame(cx, window);
    frame(cx, window);
    assert_eq!(tabs_of(cx, &view, p1), [A, B], "let go on p1's strip: into p1");
    assert_eq!(tabs_of(cx, &view, p0), [A]);
    view.read_with(cx, |v, cx| {
        for id in [p0, p1] {
            assert_eq!(v.panes.get(id).unwrap().composer.read(cx).value().to_string(), "", "typed mid-drag lands in no composer");
        }
    });
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #48 L4: the pane under the drop closed after the last move (a
/// key mid-drag can close it). The drop is asked again where it is let
/// go: on the dragged tab's own pane, that is nothing, and the tab stays
/// where it was; never opened and taken in its own pane, gone with no
/// reopen entry.
#[gpui_kit::test]
fn a_drop_on_a_pane_closed_since_the_last_move_leaves_the_tab(cx: &mut TestAppContext) {
    let (window, view, client, _conns, p0, p1) = two_panes(cx, "dnd-closed");
    let none = gpui_kit::Modifiers::default();
    let tab = tab_id(cx, &view, p1, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.5, 0.5);
    drag_to(cx, window, from, to, none, false);
    assert_eq!(view.read_with(cx, |v, _| v.dnd.hint.map(|h| (h.pane, h.zone))), Some((p0, Some(crate::dnd::Zone::Center))));
    view.update(cx, |v, _| v.in_pane(p0, |v| v.close_pane()));
    for _ in 0..40 {
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "p0 closed mid-drag");
    mouse_m(cx, window, to, "up", none);
    frame(cx, window);
    frame(cx, window);
    assert_eq!(tabs_of(cx, &view, p1), [A, B], "B stays in its own pane");
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current_session().map(str::to_string)), Some(B.to_string()), "shown there");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The brief's 5th pane: with 4 panes, a tab dragged to another pane's
/// edge never makes a 5th. The preview shows the centre (the drop that
/// will happen), the tab moves there, and the focused composer says
/// "4 panes is the most", as Ctrl+\ does.
#[gpui_kit::test]
fn a_drag_to_a_fifth_pane_is_refused_with_the_note(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-fifth", None);
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    key(cx, window, "ctrl-|");
    let p0 = view.read_with(cx, |v, _| v.reading_order()[0]);
    view.update(cx, |v, cx| v.focus_pane(p0, crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    key(cx, window, "ctrl-|");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "four panes");
    for _ in 0..80 {
        frame(cx, window);
    }
    let src = view.read_with(cx, |v, _| v.panes.focused);
    let _b = open_b(cx, window, &view, &attaches, &events);
    let target = view.read_with(cx, |v, _| v.reading_order().into_iter().find(|&id| id != src).expect("another pane"));
    let tab = tab_id(cx, &view, src, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, target, 0.97, 0.5);
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    let hint = view.read_with(cx, |v, _| v.dnd.hint).expect("a hint");
    assert_eq!(
        (hint.pane, hint.zone, hint.note),
        (target, Some(crate::dnd::Zone::Center), Some("4 panes is the most")),
        "the edge is the centre"
    );
    // Its reason on the preview while it shows (finish gate #48 S1).
    frame(cx, window);
    assert!(drawn(cx, window, "drop-label-note".into()).is_some(), "the preview's label says why, during the drag");
    mouse_m(cx, window, to, "up", gpui_kit::Modifiers::default());
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "no 5th pane");
    assert!(tabs_of(cx, &view, target).contains(&B.to_string()), "moved to that pane's centre");
    assert_eq!(view.read_with(cx, |v, _| v.panes.cur.watch_note.as_deref().map(str::to_string)), Some("4 panes is the most".into()));
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §3: a drag released outside the window (the button let go
/// there: the next move in comes with none) drops nothing and leaves no
/// hint, target or preview; Esc mid-drag does the same, and the release
/// after it drops nothing.
#[gpui_kit::test]
fn a_drag_released_outside_or_cancelled_leaves_nothing(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-outside", None);
    let _b = open_b(cx, window, &view, &attaches, &events);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let tab = tab_id(cx, &view, p0, B);
    let from = mid(cx, window, format!("tab {tab}"));
    let to = in_body(cx, window, p0, 0.95, 0.5);
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    assert!(view.read_with(cx, |v, _| v.dnd.hint.is_some()));
    // Let go outside: the next move comes with no button.
    let at = in_body(cx, window, p0, 0.5, 0.5);
    mouse_m(cx, window, at, "hover", gpui_kit::Modifiers::default());
    for _ in 0..30 {
        frame(cx, window);
    }
    view.read_with(cx, |v, cx| {
        assert!(v.dnd.on.is_none() && v.dnd.hint.is_none() && v.dnd.drawn.is_none(), "nothing of the drag left");
        assert!(!cx.has_active_drag(), "GPUI's drag ended");
        assert_eq!(v.panes.len(), 1, "nothing dropped");
    });
    assert!(drawn(cx, window, "drop-target 0".into()).is_none() && drawn(cx, window, "drop-preview".into()).is_none());
    assert_eq!(tabs_of(cx, &view, p0), [A, B]);
    // Esc mid-drag.
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    assert!(view.read_with(cx, |v, _| v.dnd.hint.is_some()));
    key(cx, window, "escape");
    assert!(view.read_with(cx, |v, cx| v.dnd.on.is_none() && v.dnd.hint.is_none() && !cx.has_active_drag()), "Esc ended it");
    mouse_m(cx, window, to, "up", gpui_kit::Modifiers::default());
    for _ in 0..30 {
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "nothing dropped");
    assert_eq!(tabs_of(cx, &view, p0), [A, B]);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split/architecture.md §4.4: a click on a rail row still opens it (a
/// press and a release with a pixel's wobble, under GPUI's drag
/// threshold): no drag, no targets.
#[gpui_kit::test]
fn a_click_on_a_rail_row_still_opens(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, _events) = on_a(cx, "dnd-click", None);
    let key = view.read_with(cx, |v, _| {
        v.rail.rail.rows.iter().find(|r| r.action == crate::named::RowAction::Attach(B.into())).map(|r| r.key).expect("B's row")
    });
    let at = mid(cx, window, format!("rail-row {key}"));
    let none = gpui_kit::Modifiers::default();
    mouse_m(cx, window, at, "hover", none);
    mouse_m(cx, window, at, "down", none);
    mouse_m(cx, window, gpui_kit::point(at.x + px(1.), at.y), "move", none);
    assert!(view.read_with(cx, |v, cx| v.dnd.on.is_none() && !cx.has_active_drag()), "no drag under the threshold");
    mouse_m(cx, window, gpui_kit::point(at.x + px(1.), at.y), "up", none);
    frame(cx, window);
    let _b = next_attach(cx, window, &attaches, "B's attach: the click opened it");
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current_session().map(str::to_string)), Some(B.to_string()));
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// split.md §10: the preview glides between zones on `snap` at Full (in
/// between frames drawn), then holds still and asks no frame; at Reduced
/// and Off it jumps (the next frame is the new zone's).
#[gpui_kit::test]
fn the_drop_preview_glides_at_full_and_jumps_at_reduced_and_off(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-preview", None);
    let _b = open_b(cx, window, &view, &attaches, &events);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let tab = tab_id(cx, &view, p0, B);
    for lv in [crate::motion::Level::Full, crate::motion::Level::Reduced, crate::motion::Level::Off] {
        level(cx, lv);
        let from = mid(cx, window, format!("tab {tab}"));
        let at = in_body(cx, window, p0, 0.05, 0.5);
        drag_to(cx, window, from, at, gpui_kit::Modifiers::default(), false);
        for _ in 0..80 {
            frame_144(cx, window, &view);
        }
        let left = drawn(cx, window, "drop-preview".into()).expect("the left half");
        let at = in_body(cx, window, p0, 0.95, 0.5);
        mouse_m(cx, window, at, "move", gpui_kit::Modifiers::default());
        let mut xs = Vec::new();
        for _ in 0..80 {
            frame_144(cx, window, &view);
            xs.push(drawn(cx, window, "drop-preview".into()).expect("the preview")[0]);
        }
        let end = *xs.last().unwrap();
        assert!(end > left[0] + 100., "{lv:?}: on the right half: {xs:?}");
        let between = xs.iter().filter(|&&x| x > left[0] + 1. && x < end - 1.).count();
        match lv {
            crate::motion::Level::Full => assert!(between >= 3, "Full: it glides: {xs:?}"),
            _ => assert_eq!(between, 0, "{lv:?}: it jumps: {xs:?}"),
        }
        let still = xs.iter().position(|&x| x == end).unwrap();
        assert!(xs[still..].iter().all(|&x| x == end), "{lv:?}: nothing moves once it lands: {xs:?}");
        assert!(!view.read_with(cx, |v, _| v.fx.pending().0), "{lv:?}: at rest, no frame asked");
        key(cx, window, "escape");
        let at = in_body(cx, window, p0, 0.5, 0.5);
        mouse_m(cx, window, at, "up", gpui_kit::Modifiers::default());
        for _ in 0..80 {
            frame_144(cx, window, &view);
        }
        assert!(drawn(cx, window, "drop-preview".into()).is_none(), "{lv:?}: gone");
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #48 r2 L1 and L2, finish gate #48 r2 N1 and N2: a rail row
/// dropped on an edge at Full, frame by frame at 144 Hz, its session's
/// prompt up as it attaches early in the flight.
/// While more than [`crate::pane::HOLD_UNTIL`] px are left the new pane
/// draws nothing from its rows: no row, no sticky label, no thumb. Its
/// rows come in before it lands at 70 % and lift to full on `quick`,
/// done before the last frame that moves; nothing (rows, column, thumb,
/// strength) changes after it. The card's buttons arm from the frame its
/// row first draws: a real click on Allow then is refused, though the
/// drop was over 500 ms before; one after the arm answers.
#[gpui_kit::test]
fn a_dropped_rows_pane_holds_its_rows_lifts_them_in_its_tail_and_arms_its_card_from_their_first_frame(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, events) = on_a(cx, "dnd-hold", None);
    level(cx, crate::motion::Level::Full);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let key = view.read_with(cx, |v, _| {
        v.rail.rail.rows.iter().find(|r| r.action == crate::named::RowAction::Attach(B.into())).map(|r| r.key).expect("B's row")
    });
    let from = mid(cx, window, format!("rail-row {key}"));
    let to = in_body(cx, window, p0, 0.97, 0.5);
    drag_to(cx, window, from, to, gpui_kit::Modifiers::default(), false);
    assert_eq!(view.read_with(cx, |v, _| v.dnd.hint.map(|h| (h.pane, h.zone))), Some((p0, Some(crate::dnd::Zone::Right))));
    mouse_m(cx, window, to, "up", gpui_kit::Modifiers::default());
    let new = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(new, p0, "a new pane");
    // Its first frame, then B attaches with its prompt up, the flight
    // held there (its clock moves only in `frame_144`).
    frame_144(cx, window, &view);
    let mut b = next_attach(cx, window, &attaches, "B's attach at the daemon");
    b.send(attached(B, Some(1)));
    feed(cx, window, &view, &events, "B's card in the new pane", |v| v.panes.get(new).is_some_and(|p| p.card.is_some()));
    assert!(view.read_with(cx, |v, _| v.panes.get(new).is_some_and(|p| p.hold_rows)), "its rows still held");
    let _ = names(&mut b);
    // Past the arm the card's first frame would have started: 600 ms on,
    // the card's arm on the test's clock (`pin_arm_clock`, as in
    // `arm_from_whole`), then 6.944 ms a frame, the click at its frame's
    // instant, however long the machine takes to draw it.
    let mut t = Instant::now() + Duration::from_millis(600);
    // A frame: how far from landing, held, its rows' boxes, sticky layers
    // painted (the first pane's is one), its column, its rows' strength.
    #[derive(Clone, Debug, PartialEq)]
    struct Snap {
        left: f32,
        rect: [f32; 4],
        held: bool,
        rows: Vec<[f32; 4]>,
        sticky: usize,
        column: f32,
        k: f32,
        flight: bool,
        thumb: Option<[f32; 4]>,
    }
    let snap = |cx: &mut TestAppContext, thumb: Option<[f32; 4]>| -> Snap {
        view.read_with(cx, |v, _| {
            let p = v.panes.get(new).expect("the new pane");
            let (r, l) = (p.rect.get(), p.land.get());
            let top = p.list.logical_scroll_top().item_ix;
            let rows = (top..(top + 10).min(p.list.item_count()))
                .filter_map(|ix| p.list.bounds_for_item(ix))
                .map(|b| [b.origin.x, b.origin.y, b.size.width, b.size.height].map(f32::from))
                .collect();
            Snap {
                left: (l.w - r.w).abs() + (l.h - r.h).abs(),
                rect: [r.x, r.y, r.w, r.h],
                held: p.hold_rows,
                rows,
                sticky: v.fx.paint_log.borrow().iter().filter(|w| **w == "sticky").count(),
                column: p.layout.column,
                k: p.drawn_k.get(),
                flight: v.panes.flight.is_some(),
                thumb,
            }
        })
    };
    let bar = view
        .read_with(cx, |v, _| if v.panes.cur.id == new { v.bars.transcript.clone() } else { v.panes.get(new).unwrap().parked.bar.clone() });
    drop(bar.thumbs());
    let card_at =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.get(new).and_then(|p| p.card.as_ref().map(|c| c.arm.shown_at())));
    let mut seen: Vec<Snap> = Vec::new();
    let mut fails = Vec::new();
    let (mut first, mut rest) = (None, 0);
    while rest < 12 {
        t += Duration::from_micros(6944);
        crate::app::pin_arm_clock(Some(t));
        let before = t;
        frame_144(cx, window, &view);
        let thumb = bar.thumbs().last().and_then(|t| t.2);
        let s = snap(cx, thumb);
        let i = seen.len();
        if first.is_none() && !s.rows.is_empty() {
            first = Some(i);
            let at = card_at(cx).expect("the card");
            if at < before {
                fails.push(format!("frame {i}: the card's arm counts from {:?} before the frame its row first draws", before - at));
            }
            // Allow, clicked by real input right after that frame: refused
            // (it arms 500 ms from there).
            let allow = in_pane_at(cx, window, "allow-fill", new);
            click_at(cx, window, allow);
            let said = names(&mut b);
            if !said.is_empty() {
                fails.push(format!("a click right after the card's first frame: {said:?}"));
            }
        }
        rest = if s.flight || i < 3 { 0 } else { rest + 1 };
        seen.push(s);
        assert!(seen.len() < 400, "never landed");
    }
    let first = first.expect("the rows came in");
    let last = (1..seen.len()).rev().find(|&i| seen[i].rect != seen[i - 1].rect).expect("the pane moved");
    eprintln!(
        "rows in at frame {first}, last move at {last} of {}: {:?}",
        seen.len(),
        seen.iter()
            .map(|s| (s.left.round(), s.held, s.rows.len(), s.sticky, (s.k * 1000.).round() / 1000., s.thumb.is_some()))
            .collect::<Vec<_>>()
    );
    for (i, s) in seen.iter().enumerate() {
        if s.left > crate::pane::HOLD_UNTIL && (!s.held || !s.rows.is_empty() || s.sticky != 1 || s.thumb.is_some()) {
            fails.push(format!(
                "frame {i}, {} px left: held {}, rows {}, sticky layers {} (the first pane's is 1), thumb {:?}",
                s.left,
                s.held,
                s.rows.len(),
                s.sticky,
                s.thumb
            ));
        }
        if !s.held && s.sticky != 2 {
            fails.push(format!("frame {i}: rows in, {} sticky layers (want 2)", s.sticky));
        }
    }
    let f = &seen[first];
    if f.left > crate::pane::HOLD_UNTIL || f.held || first >= last {
        fails.push(format!("the rows came in at frame {first} ({} px left), the pane last moved at {last}", f.left));
    }
    if (f.k - crate::motion::LIFT).abs() > 1e-3 {
        fails.push(format!("the rows came in at {} (want {})", f.k, crate::motion::LIFT));
    }
    // The lift: rising every frame to full, full before the last that moves.
    let full = (first..seen.len()).find(|&i| seen[i].k >= 1.).expect("the lift ends");
    let ks: Vec<f32> = seen[first..=full].iter().map(|s| s.k).collect();
    if ks.windows(2).any(|w| w[1] <= w[0]) || full >= last || seen[full..].iter().any(|s| s.k < 1.) {
        fails.push(format!("the lift {ks:?} over frames {first}..={full}; the pane last moved at {last}"));
    }
    if !seen[first..last].iter().any(|s| s.thumb.is_some()) {
        fails.push(format!("no thumb painted before the last frame that moves ({last})"));
    }
    for (i, s) in seen.iter().enumerate().skip(last + 1) {
        let l = &seen[last];
        if s.rows != l.rows || s.column != l.column || s.k != l.k || s.thumb.is_some_and(|t| Some(t) != l.thumb) {
            fails.push(format!("frame {i} after the last that moves ({last}): {s:?} vs {l:?}"));
        }
    }
    assert!(
        fails.is_empty(),
        "{fails:#?}\n{:#?}",
        seen.iter().map(|s| (s.left, s.held, s.rows.len(), s.sticky, s.k, s.thumb.is_some())).collect::<Vec<_>>()
    );
    // The arm run out: Allow answers, once (its repaint on the test's
    // clock too, as in `arm_from_whole`).
    crate::app::pin_arm_clock(Some(t + Duration::from_millis(600)));
    cx.executor().advance_clock(Duration::from_millis(600));
    frame(cx, window);
    let allow = in_pane_at(cx, window, "allow-fill", new);
    click_at(cx, window, allow);
    assert_eq!(names(&mut b), ["answer"], "armed, the click answers");
    crate::app::pin_arm_clock(None);
    drop(attaches);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// A click by real input at `at` with `m` held.
fn click_m(cx: &mut TestAppContext, window: AnyWindowHandle, at: Point<Pixels>, m: gpui_kit::Modifiers) {
    mouse_m(cx, window, at, "hover", m);
    mouse_m(cx, window, at, "down", m);
    mouse_m(cx, window, at, "up", m);
}

/// Whether `inner` lies whole in `outer` (boxes as [`drawn`] gives them).
fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5
        && inner[1] >= outer[1] - 0.5
        && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
        && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
}

/// #53 R3-L1, the rest of a growing pane's flight at Full, frame by frame
/// at 144 Hz, its card up (its arm, if counted from the pane's first
/// frame, run out in the 600 ms before): the card's buttons as drawn
/// against the pane's share. A click on Allow while it shows but the
/// buttons are cut off, and on the first frame they are whole a click and
/// a Ctrl+Enter (the root has the keys after the click), answer nothing;
/// the arm counts from that frame. Then, 600 ms on, Allow answers. What
/// failed, by `what`.
///
/// The card's arm is on the test's clock (`pin_arm_clock`): those 600 ms,
/// then 6.944 ms a frame, the clicks at their frame's instant. On wall
/// time a loaded runner's frame and click took longer than the arm itself
/// (#69: CI's ubuntu runner, the side case's first whole frame).
fn arm_from_whole(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<AppView>,
    new: crate::pane::PaneId,
    conn: &mut Conn,
    what: &str,
) -> Vec<String> {
    let mut fails = Vec::new();
    let mut t = Instant::now() + Duration::from_millis(600);
    let card_at =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.get(new).and_then(|p| p.card.as_ref().map(|c| c.arm.shown_at())));
    let allow = |b: [f32; 4]| gpui_kit::point(px(b[0] + b[2] - 24.), px(b[1] + b[3] / 2.));
    let (mut whole_at, mut cut_clicked, mut rest, mut i) = (None, false, 0, 0);
    while rest < 12 {
        t += Duration::from_micros(6944);
        crate::app::pin_arm_clock(Some(t));
        let before = t;
        frame_144(cx, window, view);
        let share = drawn(cx, window, format!("pane {}", new.index()));
        let buttons = drawn(cx, window, format!("card-buttons {}", new.index()));
        let flight = view.read_with(cx, |v, _| v.panes.flight.is_some());
        if let (Some(share), Some(b)) = (share, buttons)
            && whole_at.is_none()
        {
            let at = allow(b);
            let (x, y) = (f32::from(at.x), f32::from(at.y));
            let shows = x > share[0] && x < share[0] + share[2] && y > share[1] && y < share[1] + share[3];
            if inside(b, share) {
                whole_at = Some(i);
                let arm = card_at(cx).expect("the card");
                if arm < before {
                    fails.push(format!("{what}: frame {i}, its buttons' first whole: the arm counts from {:?} before it", before - arm));
                }
                click_at(cx, window, at);
                let said = names(conn);
                if !said.is_empty() {
                    fails.push(format!("{what}: a click on Allow on its buttons' first whole frame ({i}): {said:?}"));
                }
                key(cx, window, "ctrl-enter");
                let said = names(conn);
                if !said.is_empty() {
                    fails.push(format!("{what}: a Ctrl+Enter on its buttons' first whole frame ({i}): {said:?}"));
                }
            } else if shows && !cut_clicked {
                cut_clicked = true;
                click_at(cx, window, at);
                let said = names(conn);
                if !said.is_empty() {
                    fails.push(format!("{what}: a click on Allow at frame {i}, its buttons cut off ({b:?} in {share:?}): {said:?}"));
                }
            }
        }
        rest = if flight || i < 3 { 0 } else { rest + 1 };
        i += 1;
        assert!(i < 400, "{what}: never landed");
    }
    if whole_at.is_none() {
        crate::app::pin_arm_clock(None);
        fails.push(format!("{what}: its buttons never drawn whole"));
        return fails;
    }
    crate::app::pin_arm_clock(Some(t + Duration::from_millis(600)));
    cx.executor().advance_clock(Duration::from_millis(600));
    frame(cx, window);
    match drawn(cx, window, format!("card-buttons {}", new.index())) {
        Some(b) => {
            click_at(cx, window, allow(b));
            let said = names(conn);
            if said != ["answer"] {
                fails.push(format!("{what}: armed, a click on Allow: {said:?}"));
            }
        }
        None => fails.push(format!("{what}: no card left to answer")),
    }
    crate::app::pin_arm_clock(None);
    fails
}

/// #53 R3-L1: a card in a pane that grows in arms from the first frame its
/// buttons are drawn whole, never while its share cuts them off, on every
/// path that grows a pane, by real input: a tab dragged to an edge, Ctrl+\,
/// Open to the side (Alt+click on a rail row), a rail row dropped on an
/// edge (here the bottom: its card cut off from below).
#[gpui_kit::test]
fn a_growing_panes_card_arms_from_the_first_frame_its_buttons_are_whole(cx: &mut TestAppContext) {
    let none = gpui_kit::Modifiers::default();
    let mut fails = Vec::new();
    // A tab dragged to the right edge: B, its prompt up, into a new pane.
    {
        let (window, view, client, _a, attaches, events) = on_a(cx, "arm-tab", None);
        view.update(cx, |v, cx| {
            v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::New);
            cx.notify();
        });
        let mut b = next_attach(cx, window, &attaches, "B's attach at the daemon");
        b.send(attached(B, Some(1)));
        feed(cx, window, &view, &events, "B's card", |v| v.model.lives[v.panes.cur.live].is_active(B) && v.panes.cur.card.is_some());
        let _ = names(&mut b);
        level(cx, crate::motion::Level::Full);
        let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
        let tab = tab_id(cx, &view, p0, B);
        let from = mid(cx, window, format!("tab {tab}"));
        let to = in_body(cx, window, p0, 0.92, 0.5);
        drag_to(cx, window, from, to, none, false);
        mouse_m(cx, window, to, "up", none);
        let new = view.read_with(cx, |v, _| v.panes.focused);
        assert_ne!(new, p0, "a new pane");
        frame_144(cx, window, &view);
        fails.extend(arm_from_whole(cx, window, &view, new, &mut b, "a tab to the right edge"));
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
    // Ctrl+\ with A's prompt up: a second pane on A, its card too.
    {
        let (window, view, client, mut a, _attaches, events) = on_a(cx, "arm-split", Some(1));
        feed(cx, window, &view, &events, "A's card", |v| v.panes.cur.card.is_some());
        let _ = names(&mut a);
        level(cx, crate::motion::Level::Full);
        let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
        key(cx, window, "ctrl-\\");
        let new = view.read_with(cx, |v, _| v.panes.focused);
        assert_ne!(new, p0, "a new pane");
        fails.extend(arm_from_whole(cx, window, &view, new, &mut a, "Ctrl+\\"));
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
    // Open to the side, and a rail row dropped on the bottom edge: B's row,
    // B attaching with its prompt up early in the flight (held there).
    for side in [true, false] {
        let what = if side { "Open to the side" } else { "a rail row to the bottom edge" };
        let (window, view, client, _a, attaches, events) = on_a(cx, if side { "arm-side" } else { "arm-row" }, None);
        level(cx, crate::motion::Level::Full);
        let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
        let key_b = view.read_with(cx, |v, _| {
            v.rail.rail.rows.iter().find(|r| r.action == crate::named::RowAction::Attach(B.into())).map(|r| r.key).expect("B's row")
        });
        let row = mid(cx, window, format!("rail-row {key_b}"));
        if side {
            click_m(cx, window, row, gpui_kit::Modifiers { alt: true, ..Default::default() });
        } else {
            let to = in_body(cx, window, p0, 0.5, 0.97);
            drag_to(cx, window, row, to, none, false);
            mouse_m(cx, window, to, "up", none);
        }
        let new = view.read_with(cx, |v, _| v.panes.focused);
        assert_ne!(new, p0, "{what}: a new pane");
        frame_144(cx, window, &view);
        let mut b = next_attach(cx, window, &attaches, "B's attach at the daemon");
        b.send(attached(B, Some(1)));
        feed(cx, window, &view, &events, "B's card in the new pane", |v| v.panes.get(new).is_some_and(|p| p.card.is_some()));
        assert!(view.read_with(cx, |v, _| v.panes.flight.is_some()), "{what}: still in flight");
        let _ = names(&mut b);
        fails.extend(arm_from_whole(cx, window, &view, new, &mut b, what));
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// #53 R3-L2: a card in a pane the fold hides waits again when widening
/// shows it: a click on its Allow on the first frame it is drawn again
/// answers nothing (it had been up for longer than its arm); 600 ms on,
/// one answers. The card's arm on the test's clock (`pin_arm_clock`), as
/// in `arm_from_whole`: the click at its frame's instant, however long the
/// machine takes to draw it.
#[gpui_kit::test]
fn widening_back_from_a_fold_rearms_the_card_it_shows(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, events) = on_a(cx, "arm-fold", Some(1));
    feed(cx, window, &view, &events, "A's card", |v| v.panes.cur.card.is_some());
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    key(cx, window, "ctrl-\\");
    let p1 = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(p1, p0);
    // Armed in both panes, then the first one hidden by the fold.
    let mut t = Instant::now() + Duration::from_millis(600);
    crate::app::pin_arm_clock(Some(t));
    frame(cx, window);
    let _ = names(&mut a);
    resize(cx, window, 700., 800.);
    assert!(view.read_with(cx, |v, _| v.panes.folded && v.panes.focused == p1), "folded on the second pane");
    assert!(drawn(cx, window, format!("pane {}", p0.index())).is_none(), "the first pane not drawn");
    t += Duration::from_millis(600);
    crate::app::pin_arm_clock(Some(t));
    frame(cx, window);
    // Widened: its first frame shows the first pane's card, armed or not.
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1280.), px(800.)));
    frame(cx, window);
    assert!(!view.read_with(cx, |v, _| v.panes.folded), "unfolded");
    let b = drawn(cx, window, format!("card-buttons {}", p0.index())).expect("the first pane's buttons drawn");
    let allow = gpui_kit::point(px(b[0] + b[2] - 24.), px(b[1] + b[3] / 2.));
    click_at(cx, window, allow);
    assert_eq!(names(&mut a), Vec::<String>::new(), "a click on its first frame back answers nothing");
    // Its arm runs out (its repaint on the test's clock too).
    crate::app::pin_arm_clock(Some(t + Duration::from_millis(600)));
    cx.executor().advance_clock(Duration::from_millis(600));
    frame(cx, window);
    click_at(cx, window, allow);
    crate::app::pin_arm_clock(None);
    assert_eq!(names(&mut a), ["answer"], "armed, the click answers");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// A session no list has: a saved tab of it never comes back.
const GONE: &str = "20261121-050000-gone";

/// The view with split panes on, on the fake daemon, nothing open, `layout`
/// (or else `tabs`) to restore as a launch reads them (before its first
/// frame); that frame drawn. The window, the view, the client, the
/// daemon's attaches and the client's events.
#[allow(clippy::type_complexity)]
fn launch_with(
    cx: &mut TestAppContext,
    name: &str,
    layout: Option<crate::theme::Layout>,
    tabs: Option<crate::tabs::Saved>,
) -> (AnyWindowHandle, Entity<AppView>, Client, mpsc::Receiver<Conn>, async_channel::Receiver<Event>) {
    launch_in(cx, name, layout, tabs, true, (1280., 800.))
}

/// [`launch_with`], split panes on or off, in a window `w`×`h`.
#[allow(clippy::type_complexity)]
fn launch_in(
    cx: &mut TestAppContext,
    name: &str,
    layout: Option<crate::theme::Layout>,
    tabs: Option<crate::tabs::Saved>,
    split: bool,
    (w, h): (f32, f32),
) -> (AnyWindowHandle, Entity<AppView>, Client, mpsc::Receiver<Conn>, async_channel::Receiver<Event>) {
    let (window, view) = open_sized(cx, w, h);
    let (path, attaches) = start(name);
    let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::desktop())).expect("a client thread");
    until(&events, "connected", |e| matches!(e, Event::Status(synaps_client::Status::Connected { .. })));
    view.update(cx, |v, cx| {
        v.app_settings.split = split;
        v.client = Some(client.clone());
        // Nothing open, as a launch on a daemon (not the demo's tab).
        v.model.tabs = crate::tabs::Tabs::default();
        v.reconcile(None);
        v.restore_layout = layout;
        v.restore_tabs = tabs;
        cx.notify();
    });
    frame(cx, window);
    (window, view, client, attaches, events)
}

/// A session list with `ids` live, into the view as the pump hands it.
fn list(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>, ids: &[&str]) {
    let metas: Vec<synaps_client::protocol::SessionMeta> =
        ids.iter().map(|id| serde_json::from_value(json!({"id": id, "model": "m", "lifecycle": "live"})).expect("a session")).collect();
    view.update(cx, |v, cx| v.apply_batch(vec![Event::Sessions(metas)], cx));
    frame(cx, window);
}

fn saved(sessions: &[&str]) -> crate::tabs::Saved {
    crate::tabs::Saved { sessions: sessions.iter().map(|s| s.to_string()).collect(), active: Some(0), ..Default::default() }
}

fn two_across(a: &[&str], b: &[&str], focused: usize) -> crate::theme::Layout {
    crate::theme::Layout {
        focused,
        tree: crate::theme::LayoutNode::Split {
            axis: crate::split::Axis::Row,
            ratios: vec![0.3, 0.7],
            kids: vec![crate::theme::LayoutNode::Pane(0), crate::theme::LayoutNode::Pane(1)],
        },
        panes: vec![saved(a), saved(b)],
    }
}

/// Split panes step 5 (split/architecture.md §5, split.md §11): a saved
/// layout of two panes, the focused one's session gone. The launch's first
/// frame draws both panes where they were (no flight: #36 R1), focused as
/// saved; their tabs wait OPENING from the first list, and the gone one's
/// pane stays until the [`crate::tabs::RESTORE_LISTS`]th list, when its
/// tab closes and the pane collapses, said in one notice, once; the focus
/// goes to the pane left, its session attached.
#[gpui_kit::test]
fn a_restored_pane_whose_session_is_gone_collapses_after_the_lists_with_one_notice(cx: &mut TestAppContext) {
    let (window, view, client, attaches, _events) = launch_with(cx, "persist-gone", Some(two_across(&[A], &[GONE], 1)), None);
    let (ids, focused, flight, drawn_is_tree) =
        view.read_with(cx, |v, _| (v.panes.ids(), v.panes.focused, v.panes.flight.is_some(), v.panes.drawn == v.panes.tree));
    assert_eq!(ids.len(), 2, "two panes on the first frame");
    assert!(!flight && drawn_is_tree, "drawn as the layout, no flight");
    let order = view.read_with(cx, |v, _| v.reading_order());
    assert_eq!(focused, order[1], "focused as saved");
    let (b0, b1) = (pane_box(cx, window, order[0]), pane_box(cx, window, order[1]));
    assert!(b0[2] >= crate::split::PANE_MIN_W && b1[2] > b0[2], "their shares as saved, at least the minimum: {b0:?} {b1:?}");
    for n in 1..=crate::tabs::RESTORE_LISTS {
        list(cx, window, &view, &[A, B]);
        let (len, notice) = view.read_with(cx, |v, _| (v.panes.len(), v.model.named.notice.clone()));
        if n < crate::tabs::RESTORE_LISTS {
            assert_eq!(len, 2, "list {n}: the pane still waits");
            assert_eq!(tabs_of(cx, &view, order[1]), [GONE], "list {n}: its tab OPENING");
            assert_eq!(notice, None, "list {n}: nothing said yet");
        } else {
            assert_eq!(len, 1, "list {n}: collapsed");
            assert_eq!(notice.as_deref(), Some("1 saved tab closed: its session is no longer open (Ctrl+Shift+T reopens it)."));
        }
    }
    view.update(cx, |v, _| v.model.named.notice = None);
    list(cx, window, &view, &[A, B]);
    assert_eq!(view.read_with(cx, |v, _| v.model.named.notice.clone()), None, "said once");
    let (focused, tabs) =
        view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.iter().filter_map(|t| t.session.clone()).collect::<Vec<_>>()));
    assert_eq!((focused, tabs), (order[0], vec![A.to_string()]), "the pane left, focused");
    let a = next_attach(cx, window, &attaches, "A's attach");
    drop(a);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Split panes step 5 (Shady #55 L2; #36 R1): a saved pane with no
/// session tabs (an empty pane split off, or one that showed only a view)
/// is left out before the first frame, so frame 1 is the final layout: one
/// pane here, focused, with the other pane's tab; no flight then, nor when
/// the first lists come.
#[gpui_kit::test]
fn a_saved_pane_with_no_session_tabs_is_left_out_before_the_first_frame(cx: &mut TestAppContext) {
    let (window, view, client, attaches, _events) = launch_with(cx, "persist-empty", Some(two_across(&[A], &[], 1)), Some(saved(&[A])));
    let first = |cx: &mut TestAppContext| view.read_with(cx, |v, _| (v.panes.len(), v.panes.flight.is_some()));
    assert_eq!(first(cx), (1, false), "frame 1: one pane, no flight");
    for n in 1..=crate::tabs::RESTORE_LISTS {
        list(cx, window, &view, &[A, B]);
        assert_eq!(first(cx), (1, false), "list {n}: still one pane, no flight");
    }
    let tabs = view.read_with(cx, |v, _| v.model.tabs.iter().filter_map(|t| t.session.clone()).collect::<Vec<_>>());
    assert_eq!(tabs, [A], "the saved pane's tab");
    let a = next_attach(cx, window, &attaches, "A's attach");
    drop(a);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Split panes step 5 (architecture §5 ¶4): the same session saved in two
/// panes restores as a tab in each on one store: one attachment.
#[gpui_kit::test]
fn the_same_session_in_two_restored_panes_is_on_one_live(cx: &mut TestAppContext) {
    let (window, view, client, attaches, events) = launch_with(cx, "persist-twins", Some(two_across(&[A], &[A], 0)), None);
    list(cx, window, &view, &[A, B]);
    let mut a = next_attach(cx, window, &attaches, "A's attach");
    a.send(attached(A, None));
    feed(cx, window, &view, &events, "A attached", |v| v.model.lives.iter().any(|l| l.is_active(A)));
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_millis(100));
        frame(cx, window);
    }
    assert!(attaches.recv_timeout(Duration::from_millis(300)).is_err(), "no second attachment");
    let order = view.read_with(cx, |v, _| v.reading_order());
    for id in &order {
        assert_eq!(tabs_of(cx, &view, *id), [A], "a tab of A in each pane");
    }
    let lives: Vec<_> = view.read_with(cx, |v, _| order.iter().map(|id| v.panes.get(*id).unwrap().live).collect());
    assert_eq!(lives[0], lives[1], "one store");
    drop(a);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Split panes step 5 (architecture §5 ¶2): a first launch after the
/// upgrade, the file with only `tabs.<key>`: one pane with every tab.
#[gpui_kit::test]
fn a_first_launch_with_only_the_tabs_gives_one_pane(cx: &mut TestAppContext) {
    let (window, view, client, _attaches, _events) = launch_with(cx, "persist-first", None, Some(saved(&[A, B])));
    list(cx, window, &view, &[A, B]);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "one pane");
    let tabs = view.read_with(cx, |v, _| v.model.tabs.iter().filter_map(|t| t.session.clone()).collect::<Vec<_>>());
    assert_eq!(tabs, [A, B], "every tab");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Split panes step 5 (architecture §5 ¶3): the layout is saved
/// [`crate::app::SAVE_DELAY`] after a split (the generation moves), with
/// `tabs.<key>` its union; a focus change is saved too; a change left
/// unsaved is written by the quit's flush, and a launch on that file
/// restores it.
#[gpui_kit::test]
fn the_layout_is_saved_after_a_change_and_on_quit(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, _events) = on_a(cx, "persist-save", None);
    let k = format!("test:persist-save-{}", std::process::id());
    let path = crate::theme::settings_path().expect("the tests' settings file");
    view.update(cx, |v, _| v.tabs_key = Some(k.clone()));
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    let wait_save = |cx: &mut TestAppContext| {
        cx.executor().advance_clock(crate::app::SAVE_DELAY + Duration::from_millis(50));
        frame(cx, window);
        cx.run_until_parked();
    };
    wait_save(cx);
    let l = crate::theme::load_layout(&path, &k).expect("the layout saved");
    assert_eq!(l.panes.len(), 2, "two panes");
    assert_eq!(l.focused, 1, "the new one focused");
    assert_eq!(crate::theme::load_tabs(&path, &k).map(|t| t.sessions), Some(vec![A.to_string()]), "the union");
    // Nothing changed: the frames hash no layout (Shady #55 L4).
    view.update(cx, |v, _| v.layout_hashes.set(0));
    for _ in 0..10 {
        view.update(cx, |_, cx| cx.notify());
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.layout_hashes.get()), 0, "idle frames: the key alone");
    key(cx, window, "f6");
    assert!(view.read_with(cx, |v, _| v.layout_hashes.get()) > 0, "a focus change: hashed again");
    wait_save(cx);
    assert_eq!(crate::theme::load_layout(&path, &k).map(|l| l.focused), Some(0), "a focus change saved");
    // Unsaved at quit: the flush writes it.
    key(cx, window, "f6");
    view.update(cx, |v, _| v.flush_tabs());
    assert_eq!(crate::theme::load_layout(&path, &k).map(|l| l.focused), Some(1), "flushed on quit");
    let _ = names(&mut a);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
    let (layout, tabs) = crate::theme::load_restore(&path, &k, Some(true));
    assert!(layout.is_some() && tabs.is_some(), "a launch reads it back");
}

/// This key's entries in the tests' settings file, `layout.<k>` and
/// `tabs.<k>` (the file is shared by the tests that run at once).
fn file_keys(path: &std::path::Path, k: &str) -> (Value, Value) {
    let v: Value = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    (v["layout"][k].clone(), v["tabs"][k].clone())
}

/// The labels of pane `id`'s tabs.
fn labels_of(cx: &mut TestAppContext, view: &Entity<AppView>, id: crate::pane::PaneId) -> Vec<String> {
    view.update(cx, |v, _| v.in_pane(id, |v| v.model.tabs.iter().map(|t| t.label.clone()).collect()).unwrap_or_default())
}

/// The delayed save's wait, its frame and its write.
fn wait_saved(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.executor().advance_clock(crate::app::SAVE_DELAY + Duration::from_millis(50));
    frame(cx, window);
    cx.run_until_parked();
}

/// Shady #55 n5 (a launch writes nothing of its own) and r2 n7: a layout
/// restored, its lists in and the save's delay over, `layout.<k>` and
/// `tabs.<k>` are as they were. Then B, renamed at the daemon, is named
/// so in its tab in the pane not acted on, and that is saved.
#[gpui_kit::test]
fn a_launch_writes_nothing_of_its_own_and_a_daemon_rename_is_saved_from_any_pane(cx: &mut TestAppContext) {
    let k = format!("test:persist-quiet-{}", std::process::id());
    let path = crate::theme::settings_path().expect("the tests' settings file");
    let layout = two_across(&[A], &[B], 0);
    crate::theme::save_layout(&path, &k, &layout).unwrap();
    let before = file_keys(&path, &k);
    let (window, view, client, attaches, _events) = launch_with(cx, "persist-quiet", Some(layout), None);
    view.update(cx, |v, _| v.tabs_key = Some(k.clone()));
    for _ in 0..=crate::tabs::RESTORE_LISTS {
        list(cx, window, &view, &[A, B]);
        wait_saved(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2, "both panes");
    assert_eq!(file_keys(&path, &k), before, "the launch wrote nothing");
    // Seen, not inferred (Shady #59 L5: an identical rewrite passes the above).
    assert_eq!(writes(cx, &view), 0, "no write at all");
    let order = view.read_with(cx, |v, _| v.reading_order());
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), order[0]);
    // B renamed at the daemon: that is written (once).
    let metas: Vec<synaps_client::protocol::SessionMeta> = [(A, None), (B, Some("Relay v2"))]
        .iter()
        .map(|(id, name)| serde_json::from_value(json!({"id": id, "model": "m", "lifecycle": "live", "name": name})).expect("a session"))
        .collect();
    view.update(cx, |v, cx| v.apply_batch(vec![Event::Sessions(metas)], cx));
    frame(cx, window);
    assert_eq!(labels_of(cx, &view, order[1]), ["Relay v2"], "its tab, in the other pane");
    wait_saved(cx, window);
    let saved = crate::theme::load_layout(&path, &k).expect("the layout");
    assert_eq!(saved.panes[1].labels, ["Relay v2"], "saved");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #55 n5: split panes off, a layout in the file. The launch is one
/// pane from `tabs.<k>` (the layout never read), and a change of its tabs
/// is saved there, `layout.<k>` left as it was.
#[gpui_kit::test]
fn flag_off_a_saved_layout_is_neither_read_nor_written(cx: &mut TestAppContext) {
    let k = format!("test:persist-off-{}", std::process::id());
    let path = crate::theme::settings_path().expect("the tests' settings file");
    crate::theme::save_layout(&path, &k, &two_across(&[A], &[B], 1)).unwrap();
    let (restore, tabs) = crate::theme::load_restore(&path, &k, Some(false));
    assert!(restore.is_none(), "flag off: no layout read");
    let (window, view, client, attaches, _events) = launch_in(cx, "persist-off", restore, tabs, false, (1280., 800.));
    view.update(cx, |v, _| v.tabs_key = Some(k.clone()));
    list(cx, window, &view, &[A, B]);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "one pane");
    assert_eq!(tabs_of(cx, &view, p0), [A, B], "the union, as one pane");
    let layout_before = file_keys(&path, &k).0;
    key(cx, window, "ctrl-w");
    wait_saved(cx, window);
    let (layout, tabs) = file_keys(&path, &k);
    assert_eq!(layout, layout_before, "layout.<k> untouched");
    assert_eq!(tabs["sessions"], json!([A]), "the change saved to tabs.<k>");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #55 n5, L3: a restored layout's ratios (0.1 / 0.9) are clamped to
/// the window on frame 1, the first pane at its minimum, no flight. In a
/// window too small for two panes at launch it folds, and once the window
/// widens it is clamped to fit, not left folded under the saved ratios.
#[gpui_kit::test]
fn a_restored_layout_is_clamped_on_frame_1_and_again_once_a_small_window_widens(cx: &mut TestAppContext) {
    use crate::split::PANE_MIN_W;
    let mut layout = two_across(&[A], &[B], 0);
    if let crate::theme::LayoutNode::Split { ratios, .. } = &mut layout.tree {
        *ratios = vec![0.1, 0.9];
    }
    let (window, view, client, attaches, _events) = launch_with(cx, "persist-clamp", Some(layout.clone()), None);
    let (order, flight, folded) = view.read_with(cx, |v, _| (v.reading_order(), v.panes.flight.is_some(), v.panes.folded));
    assert!(!flight && !folded, "frame 1: laid out, no flight");
    let b0 = pane_box(cx, window, order[0]);
    assert!((b0[2] - PANE_MIN_W).abs() <= 1., "frame 1: the first pane at its minimum: {b0:?}");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
    // Too small for two panes: folded; widened, clamped.
    let (window, view, client, attaches, _events) = launch_in(cx, "persist-clamp-small", Some(layout), None, true, (700., 800.));
    assert!(view.read_with(cx, |v, _| v.panes.folded), "700 px: folded");
    resize(cx, window, 1280., 800.);
    frame(cx, window);
    let (order, folded) = view.read_with(cx, |v, _| (v.reading_order(), v.panes.folded));
    assert!(!folded, "widened to 1280: the layout clamped to fit, unfolded");
    let b0 = pane_box(cx, window, order[0]);
    assert!((b0[2] - PANE_MIN_W).abs() <= 1., "widened: the first pane at its minimum: {b0:?}");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #55 n4: a restored pane closed while its tabs still wait leaves
/// no id behind for a restore to act on: the next pane takes the id, and
/// the lists that come leave it as made.
#[gpui_kit::test]
fn a_restored_pane_closed_while_it_waits_leaves_its_id_to_the_next_pane(cx: &mut TestAppContext) {
    let (window, view, client, attaches, _events) = launch_with(cx, "persist-reuse", Some(two_across(&[A], &[GONE], 0)), None);
    let order = view.read_with(cx, |v, _| v.reading_order());
    view.update(cx, |v, cx| {
        v.in_pane(order[1], |v| v.close_pane());
        cx.notify();
    });
    for _ in 0..40 {
        frame(cx, window);
    }
    let left = view.read_with(cx, |v, _| (v.restored.contains(&order[1]), v.restore_panes.iter().any(|(p, _)| *p == order[1])));
    assert_eq!(left, (false, false), "its id in no restore");
    key(cx, window, "ctrl-\\");
    for _ in 0..40 {
        frame(cx, window);
    }
    assert!(view.read_with(cx, |v, _| v.panes.ids().contains(&order[1])), "the id reused");
    for _ in 0..crate::tabs::RESTORE_LISTS {
        list(cx, window, &view, &[A, B]);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2, "the new pane stays");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #55 r2 L5: Ctrl+\ at Full, the right panel yielding to the
/// split (#51: the layout rides it, the centre growing under the flight;
/// the new pane laid out at its width in the centre it lands in). Every
/// frame its share is not counted as clipping it, the share is within
/// [`crate::pane::HOLD_UNTIL`] px of what is laid out in it; and its card
/// arms from the first frame its buttons are whole.
#[gpui_kit::test]
fn a_riding_grow_panes_clip_is_measured_against_what_is_laid_out_in_it(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, events) = on_a(cx, "arm-yield", Some(1));
    feed(cx, window, &view, &events, "A's card", |v| v.panes.cur.card.is_some());
    let _ = names(&mut a);
    level(cx, crate::motion::Level::Full);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    key(cx, window, "ctrl-\\");
    let new = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(new, p0, "a new pane");
    let mut fails = Vec::new();
    let mut rode = false;
    for i in 0..400 {
        frame_144(cx, window, &view);
        let (flight, ride, clipped, laid, drawn) = view.read_with(cx, |v, _| {
            let p = v.panes.get(new).expect("the new pane");
            (v.panes.flight.is_some(), v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some()), p.clipped, p.laid_w.get(), p.rect.get().w)
        });
        rode |= ride;
        if !flight {
            break;
        }
        if !clipped && laid - drawn > crate::pane::HOLD_UNTIL + 0.5 {
            fails.push(format!("frame {i}: not clipped, but its share is {} px short of what is laid out in it", laid - drawn));
        }
    }
    assert!(rode, "the layout rode the right panel's yield");
    assert!(fails.is_empty(), "{fails:#?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #44 L7, split.md §7's third clause: nothing waits on screen and
/// B waits off it. The palette's Jump is not greyed, and Ctrl+J opens B
/// in the focused pane (no new pane).
#[gpui_kit::test]
fn ctrl_j_with_nothing_waiting_on_screen_opens_the_waiting_session_in_the_focused_pane(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, _events) = on_a(cx, "jump-open", None);
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame(cx, window);
    }
    let metas: Vec<synaps_client::protocol::SessionMeta> = [(A, 0), (B, 1)]
        .iter()
        .map(|(id, n)| {
            serde_json::from_value(json!({"id": id, "model": "m", "lifecycle": "live", "awaiting_input": n})).expect("a session")
        })
        .collect();
    view.update(cx, |v, cx| v.apply_batch(vec![Event::Sessions(metas)], cx));
    frame(cx, window);
    let (focused, n) = view.read_with(cx, |v, _| (v.panes.focused, v.panes.len()));
    assert_eq!(view.read_with(cx, |v, _| v.jump_elsewhere()), Some(B.to_string()), "B is the jump's");
    key(cx, window, "ctrl-j");
    let b = next_attach(cx, window, &attaches, "B's attach");
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.focused, v.panes.len())), (focused, n), "the focused pane, no new pane");
    assert!(tabs_of(cx, &view, focused).contains(&B.to_string()), "B opened in it");
    assert_eq!(jump_off(cx, &view), Some(None), "the palette's Jump offered while B still waits");
    drop(b);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// #53 nit: Open to the side (Alt+click on a rail row) holds its new
/// pane's rows while its split flies, as a rail row dropped on an edge
/// does, and lets them in before it lands.
#[gpui_kit::test]
fn open_to_the_side_holds_its_rows_until_its_split_lands(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, _events) = on_a(cx, "side-hold", None);
    level(cx, crate::motion::Level::Full);
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let key_b = view.read_with(cx, |v, _| {
        v.rail.rail.rows.iter().find(|r| r.action == crate::named::RowAction::Attach(B.into())).map(|r| r.key).expect("B's row")
    });
    let row = mid(cx, window, format!("rail-row {key_b}"));
    click_m(cx, window, row, gpui_kit::Modifiers { alt: true, ..Default::default() });
    let new = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(new, p0, "a new pane");
    frame_144(cx, window, &view);
    assert!(view.read_with(cx, |v, _| v.panes.get(new).is_some_and(|p| p.hold_rows)), "its rows held as it flies");
    for _ in 0..400 {
        if view.read_with(cx, |v, _| v.panes.flight.is_none()) {
            break;
        }
        frame_144(cx, window, &view);
    }
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_none() && v.panes.get(new).is_some_and(|p| !p.hold_rows)), "landed, its rows in");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #54 n2: the rail let go onto a width at `snap` (as a drag's
/// detent is), the right panel yielding to the panes because of it: the
/// yield flies on the rail's `snap`, not on `sheet` beside it.
#[gpui_kit::test]
fn a_yield_flies_on_the_spring_of_the_panel_that_made_it(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_in(cx, "yield-snap", attached(A, None), (1600., 900.));
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.pane_yield.map(|y| y.1)), Some(false), "two panes, the right panel open");
    level(cx, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        let mut p = v.panels;
        p.rail_width = Some(crate::theme::RAIL_WIDTHS.1);
        v.fly_to(p, crate::motion::SNAP_RESPONSE);
        v.panels = p;
        cx.notify();
    });
    frame_144(cx, window, &view);
    let (yielded, response) = view.read_with(cx, |v, _| (v.pane_yield.map(|y| y.1), v.panel_fx.sides[1].response));
    assert_eq!(yielded, Some(true), "the wider rail: the right panel yields");
    assert_eq!(response, crate::motion::SNAP_RESPONSE, "on the rail's spring");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The session menu's items now: each label, and why it can't run.
fn menu_items(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<(String, Option<String>)> {
    view.read_with(cx, |v, _| {
        v.smenu
            .as_ref()
            .map(|(m, _)| m.items.iter().map(|i| (i.label.to_string(), i.why_not.as_ref().map(|w| w.to_string()))).collect())
            .unwrap_or_default()
    })
}

/// Pane `id`'s tab of `session`: its menu opened, `act` picked.
fn tab_menu_pick(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<AppView>,
    tab: u64,
    act: Option<crate::app::SessionAct>,
) -> Vec<(String, Option<String>)> {
    cx.update_window(window, |_, w, cx| {
        view.update(cx, |v, cx| v.open_session_menu(crate::app::SessionAt::Tab(tab), Default::default(), w, cx));
    })
    .expect("the window");
    let items = menu_items(cx, view);
    if let Some(act) = act {
        cx.update_window(window, |_, w, cx| {
            view.update(cx, |v, cx| v.session_menu_pick(act, crate::menu::How::Key { press: true }, w, cx));
        })
        .expect("the window");
    }
    for _ in 0..10 {
        frame(cx, window);
    }
    items
}

/// Split panes step 6 (split.md §2, the pane menu): a tab's menu leads
/// with Open to the side, Split right, Split down, Move to new pane and
/// Close pane, each greyed with why when it can't run, then today's
/// three. Move to new pane takes the tab into a new pane, out of its own;
/// Close pane, from the new pane's tab, closes it again. Split panes off:
/// today's three alone.
#[gpui_kit::test]
fn a_tabs_menu_leads_with_the_pane_items(cx: &mut TestAppContext) {
    let (window, view, client, _a, attaches, _events) = on_a(cx, "pane-menu", None);
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
    });
    let mut b = next_attach(cx, window, &attaches, "B's attach");
    b.send(attached(B, None));
    for _ in 0..10 {
        frame(cx, window);
    }
    let p0 = view.read_with(cx, |v, _| v.panes.cur.id);
    let tab_b = tab_id(cx, &view, p0, B);
    let labels = |items: &[(String, Option<String>)]| items.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>();
    let items = tab_menu_pick(cx, window, &view, tab_b, None);
    assert_eq!(
        labels(&items)[..5],
        ["Open to the side", "Split right", "Split down", "Move to new pane", "Close pane"].map(String::from),
        "{items:?}"
    );
    assert_eq!(labels(&items)[5..].first().map(String::as_str), Some("Rename…"), "then today's");
    assert_eq!(items[4].1.as_deref(), Some("one pane"), "Close pane greyed");
    assert!(items[..4].iter().all(|(_, off)| off.is_none()), "the others run: {items:?}");
    view.update(cx, |v, cx| {
        v.close_menu();
        cx.notify();
    });
    let _ = tab_menu_pick(cx, window, &view, tab_b, Some(crate::app::SessionAct::MoveToNewPane));
    let (n, focused) = view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused));
    assert_eq!(n, 2, "a new pane");
    assert_ne!(focused, p0);
    assert_eq!((tabs_of(cx, &view, p0), tabs_of(cx, &view, focused)), (vec![A.to_string()], vec![B.to_string()]), "B moved");
    let tab_b = tab_id(cx, &view, focused, B);
    let items = tab_menu_pick(cx, window, &view, tab_b, None);
    assert_eq!(items[3].1.as_deref(), Some("its pane's only tab"), "{items:?}");
    assert_eq!(items[4].1, None, "Close pane runs");
    view.update(cx, |v, cx| {
        v.close_menu();
        cx.notify();
    });
    let _ = tab_menu_pick(cx, window, &view, tab_b, Some(crate::app::SessionAct::ClosePane));
    for _ in 0..40 {
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "Close pane closed it");
    // Split panes off: today's three.
    view.update(cx, |v, cx| {
        v.app_settings.split = false;
        cx.notify();
    });
    let tab_a = tab_id(cx, &view, p0, A);
    let items = tab_menu_pick(cx, window, &view, tab_a, None);
    assert_eq!(labels(&items), ["Rename…", "Favourite", "Close tab"].map(String::from), "{items:?}");
    drop(b);
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Split panes step 6 (architecture §7, split.md §10): at Reduced and at
/// Off a split, an equalise and a close each land in the frame that makes
/// them, the panels with them. Every frame drawn is recorded at its paint
/// (#61's [`draws`]; the equalise's uneven start set up before): the first
/// is the move's, landed (nothing flying, the drawn layout the tree), with
/// its panes (2, 2, 1) and, for the equalise, even ratios; so is every one
/// after. At Off the move draws its frame alone (a split one more: its new
/// pane's keys follow at the next frame). With the window's focus on its
/// root (no caret) and a Reduced fade's `quick` over, a second at 144 Hz
/// drawing only the frames the app asks for ([`idle_step`]) runs no frame
/// callback and paints nothing: no spring is left armed at rest (Shady #59
/// r2 L5.3).
#[gpui_kit::test]
fn reduced_and_off_land_splits_closes_and_equalise_in_one_frame_with_nothing_armed(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    // The frames an Off split, equalise and close draw, all told: the
    // move's, and a split's one more (its new pane's keys follow at the
    // next frame: `apply_keys`).
    const OFF_DRAWS: (usize, usize, usize) = (2, 1, 1);
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "step6-levels", None);
    let _ = cx.update_window(window, |_, window, _| window.activate_window());
    for l in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        level(cx, l);
        // Each step: its setup (not observed), its act, and the panes it
        // leaves.
        type Step<'a> = (&'a str, Box<dyn Fn(&mut TestAppContext) + 'a>, Box<dyn Fn(&mut TestAppContext) + 'a>, usize);
        let steps: [Step; 3] = [
            ("split", Box::new(|_: &mut TestAppContext| {}), Box::new(|cx: &mut TestAppContext| key(cx, window, "ctrl-\\")), 2),
            (
                "equalise",
                // Off even first, as a divider's drag leaves it, drawn so.
                Box::new(|cx: &mut TestAppContext| {
                    view.update(cx, |v, cx| {
                        if let crate::split::Node::Split { ratios, .. } = &mut v.panes.tree {
                            *ratios = vec![0.3, 0.7];
                        }
                        v.panes.drawn = v.panes.tree.clone();
                        cx.notify();
                    });
                    frame(cx, window);
                    idle_step(cx, window, &view, Duration::from_millis(300));
                }),
                Box::new(|cx: &mut TestAppContext| {
                    let _ = cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::Equalise, w, cx)));
                    frame(cx, window);
                }),
                2,
            ),
            ("close", Box::new(|_: &mut TestAppContext| {}), Box::new(|cx: &mut TestAppContext| press(cx, window, "ctrl-w")), 1),
        ];
        for (what, setup, act, panes) in steps.iter() {
            setup(cx);
            // Every frame drawn from here (#61's per-draw probe, Shady #59
            // L5.3): whether anything moved, the panes, the drawn layout's
            // ratios, and whether the drawn layout is the tree.
            let seen = draws(cx, &view, |v| {
                let ratios = match &v.panes.drawn {
                    crate::split::Node::Split { ratios, .. } => ratios.clone(),
                    crate::split::Node::Leaf(_) => vec![1.],
                };
                (v.panes.flight.is_some() || v.panel_fx.moving(), v.panes.len(), ratios, v.panes.drawn == v.panes.tree)
            });
            let was = view.read_with(cx, |v, _| {
                let ratios = match &v.panes.drawn {
                    crate::split::Node::Split { ratios, .. } => ratios.clone(),
                    crate::split::Node::Leaf(_) => vec![1.],
                };
                (v.panes.len(), ratios)
            });
            act(cx);
            // The move's own frame (`key` and the equalise's `frame` draw
            // it), then one more only if the app asks for it.
            asked_frame(cx, window, &view);
            let drawn = seen.borrow().clone();
            // A key's own frame can come before the move it defers to the
            // next frame's top: the same picture as before, nothing moved.
            // The first frame that differs is the move's, landed: its panes,
            // its layout the tree's, an equalise's ratios even; so is every
            // frame after it.
            let first = drawn.iter().position(|d| (d.1, &d.2) != (was.0, &was.1));
            let Some(first) = first else { panic!("{l:?}, {what}: no frame drawn shows the move: {drawn:?}") };
            assert!(drawn[..first].iter().all(|d| !d.0 && d.3), "{l:?}, {what}: a frame before the move's is still: {drawn:?}");
            let moved = &drawn[first..];
            assert!(
                moved.iter().all(|d| !d.0 && d.3 && d.1 == *panes),
                "{l:?}, {what}: from the move's frame on, landed with {panes} pane(s): {drawn:?}"
            );
            if *what == "equalise" {
                assert!(
                    moved.iter().all(|d| d.2.iter().all(|r| (r - 0.5).abs() < 1e-4)),
                    "{l:?}, {what}: even from its frame on: {drawn:?}"
                );
            }
            // Off: OFF_DRAWS frames for the move, all told.
            if l == crate::motion::Level::Off {
                let expected = match *what {
                    "split" => OFF_DRAWS.0,
                    "equalise" => OFF_DRAWS.1,
                    _ => OFF_DRAWS.2,
                };
                assert_eq!(drawn.len(), expected, "{l:?}, {what}: frames drawn for the move: {drawn:?}");
            }
            // Off the composer (its caret blinks a frame every 500 ms), a
            // Reduced fade's frames over (`quick`), then a second at 144
            // Hz drawing only what the app asks for: no frame callback and
            // no paint.
            let _ = cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| window.focus(&v.focus, cx)));
            idle_step(cx, window, &view, Duration::from_millis(300));
            let at_rest = seen.borrow().len();
            let ran: usize = (0..144).map(|_| idle_step(cx, window, &view, Duration::from_micros(6944))).sum();
            assert_eq!(ran, 0, "{l:?}, {what}: frame callbacks at rest: {:?}", &seen.borrow()[at_rest..]);
            assert_eq!(seen.borrow().len(), at_rest, "{l:?}, {what}: paints at rest: {:?}", &seen.borrow()[at_rest..]);
            assert!(!view.read_with(cx, |v, _| v.fx.pending().0), "{l:?}, {what}: a frame asked for at rest");
            view.update(cx, |v, _| v.on_draw = None);
        }
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "{l:?}: one pane again");
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// The divider's wobble on the 2-panel settle (not fixed: PR #59): two
/// panes at 1920, the rail wide and the right panel yielded to them. The
/// rail folds and the right panel comes back (both fly; no layout flight,
/// no ride: the panes are the tree in the centre each frame), then the
/// rail unfolds and the right panel yields again. Frame by frame at 144 Hz
/// the divider's x on screen should move one way only. It goes back half a
/// pixel four times in the unfold's tail (frames 59-66 at `cf28780`): the
/// two panels step their half-pixels on alternate frames, so the centre's
/// width (1920 - MARK - both) alternates 1407.5 / 1408, and pane 0 (the
/// tree's 0.5 of it) 703 / 704. The fix: the yielding panel drawn off the
/// causing panel's progress, so their edges step together (`PanelFx`).
#[gpui_kit::test]
#[ignore = "the 2-panel settle's wobble, not fixed yet: see the doc comment"]
fn the_divider_moves_one_way_through_a_two_panel_settle(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_in(cx, "settle", attached(A, None), (1920., 1080.));
    view.update(cx, |v, cx| {
        let mut p = v.panels;
        (p.rail_width, p.right_width) = (Some(crate::theme::RAIL_WIDTHS.1), Some(crate::theme::RIGHT_WIDTHS.1));
        v.panels = p;
        cx.notify();
    });
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame(cx, window);
    }
    assert_eq!(
        view.read_with(cx, |v, _| (v.panes.len(), v.pane_yield.map(|y| y.1))),
        (2, Some(true)),
        "two panes, the right panel yielded"
    );
    level(cx, crate::motion::Level::Full);
    let mut fails = Vec::new();
    for (what, yields) in [("fold", false), ("unfold", true)] {
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        let (mut xs, mut rest, mut both) = (Vec::new(), 0, false);
        for _ in 0..400 {
            frame_144(cx, window, &view);
            xs.extend(drawn(cx, window, "pane-divider Row".into()).map(|b| b[0]));
            let (moving, sides) = view.read_with(cx, |v, _| {
                (v.panes.flight.is_some() || v.panel_fx.moving(), v.panel_fx.sides.iter().filter(|f| f.moving).count())
            });
            both |= sides == 2;
            rest = if moving { 0 } else { rest + 1 };
            if rest >= 5 {
                break;
            }
        }
        assert!(both, "{what}: both panels flew");
        assert_eq!(view.read_with(cx, |v, _| v.pane_yield.map(|y| y.1)), Some(yields), "{what}: the right panel's yield");
        let dir = (xs[xs.len() - 1] - xs[0]).signum();
        eprintln!("SETTLE {what}: {} frames, {:?}", xs.len(), xs);
        for (i, w) in xs.windows(2).enumerate() {
            if (w[1] - w[0]) * dir < -0.01 {
                fails.push(format!("{what}, frame {}: the divider went back, {} -> {}", i + 1, w[0], w[1]));
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── #59 round 2 (Shady L1–L5, the design gate's M1) ─────────────────────

/// The writes this window made to `tabs.<k>` / `layout.<k>` ([`crate::app::SaveJob`]).
fn writes(cx: &mut TestAppContext, view: &Entity<AppView>) -> u32 {
    view.read_with(cx, |v, _| v.save_job.lock().unwrap_or_else(std::sync::PoisonError::into_inner).writes)
}

/// The palette's "Jump to what is waiting": `Some(None)` offered, `Some(Some(why))` greyed.
fn jump_off(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<Option<String>> {
    use crate::palette::{Act, AppAct};
    view.read_with(cx, |v, cx| {
        v.palette_entries(cx).1.into_iter().find(|e| matches!(e.act, Act::App(AppAct::JumpWaiting))).map(|e| e.off.map(|o| o.to_string()))
    })
}

/// The session menu opened at `at` (a right-click's), its items' acts.
fn menu_acts(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<AppView>,
    at: crate::app::SessionAt,
) -> Vec<crate::app::SessionAct> {
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.open_session_menu(at, gpui_kit::point(px(400.), px(300.)), w, cx)))
        .unwrap();
    view.read_with(cx, |v, _| v.smenu.as_ref().map(|(m, _)| m.items.iter().map(|i| i.act).collect()).unwrap_or_default())
}

/// Shady #59 L2: a rail row's menu has Open to the side and no tab items
/// (Split right/down, Move to new pane, Close pane), whether or not its
/// session has a tab in the focused pane; a tab's menu has all five.
#[gpui_kit::test]
fn a_rail_rows_menu_has_open_to_the_side_and_no_tab_items(cx: &mut TestAppContext) {
    use crate::app::{SessionAct, SessionAt};
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "row-menu", None);
    list(cx, window, &view, &[A, B]);
    let row_of = |cx: &mut TestAppContext, s: &str| {
        view.read_with(cx, |v, _| {
            v.rail.rail.rows.iter().find(|r| matches!(&r.action, crate::named::RowAction::Attach(id) if id == s)).map(|r| r.key)
        })
        .unwrap_or_else(|| panic!("{s}'s row"))
    };
    let tab_items = [SessionAct::SplitRight, SessionAct::SplitDown, SessionAct::MoveToNewPane, SessionAct::ClosePane];
    for (what, s) in [("a row whose session has a tab here", A), ("a row with no tab", B)] {
        let at = SessionAt::Row(row_of(cx, s));
        let got = menu_acts(cx, window, &view, at);
        view.update(cx, |v, _| v.close_menu());
        assert_eq!(got.first(), Some(&SessionAct::OpenSide), "{what}: Open to the side first: {got:?}");
        assert!(!got.iter().any(|a| tab_items.contains(a)), "{what}: no tab items: {got:?}");
    }
    let tab = view.read_with(cx, |v, _| v.model.tabs.iter().find(|t| t.session.as_deref() == Some(A)).map(|t| t.id)).expect("A's tab");
    let got = menu_acts(cx, window, &view, SessionAt::Tab(tab));
    assert!(tab_items.iter().all(|a| got.contains(a)) && got.contains(&SessionAct::OpenSide), "a tab's menu: all five: {got:?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #59 L4: the pane menu's refusals follow the window while it is
/// open. A tab's menu at 1920 × 1080, the keyboard on Split down; the
/// window 500 px tall: Split down greyed with why (no room for two rows),
/// the selection moved on to an item that can act; tall again: offered
/// again, and the keyboard's Enter on it splits down.
#[gpui_kit::test]
fn the_pane_menus_refusals_follow_the_window_while_it_is_open(cx: &mut TestAppContext) {
    use crate::app::{SessionAct, SessionAt};
    let (window, view, client, _a, _attaches, _events) = on_a(cx, "menu-resize", None);
    let resize = |cx: &mut TestAppContext, h: f32| {
        VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(h)));
        frame(cx, window);
        frame(cx, window);
    };
    resize(cx, 1080.);
    let tab = view.read_with(cx, |v, _| v.model.tabs.iter().find(|t| t.session.as_deref() == Some(A)).map(|t| t.id)).expect("A's tab");
    menu_acts(cx, window, &view, SessionAt::Tab(tab));
    let state = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let (m, _) = v.smenu.as_ref().expect("the menu open");
            let i = m.items.iter().position(|it| it.act == SessionAct::SplitDown).expect("Split down");
            (i, m.items[i].why_not.as_ref().map(|s| s.to_string()), m.sel, m.sel.is_some_and(|s| m.items[s].enabled()))
        })
    };
    let (down, why, _, _) = state(cx);
    assert_eq!(why, None, "1080: Split down offered");
    for _ in 0..3 {
        key(cx, window, "down");
    }
    assert_eq!(state(cx).2, Some(down), "the keyboard on Split down");
    resize(cx, 500.);
    let (_, why, sel, ok) = state(cx);
    assert_eq!(why.as_deref(), Some("no room to split this pane"), "500 tall: greyed, with why");
    assert!(sel != Some(down) && ok, "the selection moved on to an item that can act: {sel:?}");
    resize(cx, 1080.);
    assert_eq!(state(cx).1, None, "tall again: offered again");
    key(cx, window, "home");
    key(cx, window, "down");
    key(cx, window, "down");
    assert_eq!(state(cx).2, Some(down), "the keyboard back on Split down");
    key(cx, window, "enter");
    for _ in 0..3 {
        frame(cx, window);
    }
    let split = view.read_with(cx, |v, _| {
        (v.panes.len(), matches!(&v.panes.tree, crate::split::Node::Split { axis, .. } if *axis == crate::split::Axis::Column))
    });
    assert_eq!(split, (2, true), "Enter split down");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #59 L3, split.md §7's third clause: pane P shows A with B a
/// background tab; the focused pane Q shows A; nothing waits on screen
/// and B waits. The palette offers Jump, and picking it opens B in Q (the
/// focused pane), not by focusing P's background tab; P keeps its tabs.
#[gpui_kit::test]
fn jump_opens_a_session_waiting_as_a_background_tab_elsewhere_in_the_focused_pane(cx: &mut TestAppContext) {
    use crate::palette::AppAct;
    let (window, view, client, _a, attaches, _events) = on_a(cx, "jump-background", None);
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
    });
    let mut b = next_attach(cx, window, &attaches, "B's attach");
    b.send(attached(B, None));
    frame(cx, window);
    view.update(cx, |v, cx| {
        let ix = v.model.tabs.find(A).expect("A's tab");
        v.focus_tab(ix);
        v.reconcile(None);
        cx.notify();
    });
    frame(cx, window);
    let p = view.read_with(cx, |v, _| v.panes.focused);
    assert_eq!(tabs_of(cx, &view, p), [A, B], "P: A, then B");
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame(cx, window);
    }
    let q = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(p, q, "Q, the new pane, focused");
    let metas: Vec<synaps_client::protocol::SessionMeta> = [(A, 0), (B, 1)]
        .iter()
        .map(|(id, n)| {
            serde_json::from_value(json!({"id": id, "model": "m", "lifecycle": "live", "awaiting_input": n})).expect("a session")
        })
        .collect();
    view.update(cx, |v, cx| v.apply_batch(vec![Event::Sessions(metas)], cx));
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.jump_elsewhere()), Some(B.to_string()), "B is the jump's");
    assert_eq!(jump_off(cx, &view), Some(None), "the palette offers Jump");
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(AppAct::JumpWaiting, w, cx))).unwrap();
    for _ in 0..3 {
        frame(cx, window);
    }
    while attaches.try_recv().is_ok() {}
    assert_eq!(view.read_with(cx, |v, _| (v.panes.focused, v.panes.len())), (q, 2), "Q still focused, no new pane");
    assert!(tabs_of(cx, &view, q).contains(&B.to_string()), "B opened in Q: {:?}", tabs_of(cx, &view, q));
    assert_eq!(tabs_of(cx, &view, p), [A, B], "P's tabs as they were");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #59 L1 (#55 n6): a delayed layout write under way at quit never
/// lands after the quit's flush. G1 (the focus to the first pane): its
/// delayed save has read its layout, its write not yet run; then G2 (the
/// focus back) is flushed, as the window closes; then G1's write runs. The
/// file has G2's focus, not G1's ([`crate::app::SaveJob`]: the flush closes
/// the slot, the job finds it closed).
#[gpui_kit::test]
fn a_delayed_layout_write_under_way_never_lands_after_the_quit_flush(cx: &mut TestAppContext) {
    let (window, view, client, mut a, _attaches, _events) = on_a(cx, "persist-race", None);
    let k = format!("test:persist-race-{}", std::process::id());
    let path = crate::theme::settings_path().expect("the tests' settings file");
    view.update(cx, |v, _| v.tabs_key = Some(k.clone()));
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    wait_saved(cx, window);
    let focused = |_: &mut TestAppContext| crate::theme::load_layout(&path, &k).map(|l| l.focused);
    assert_eq!(focused(cx), Some(1), "G0: the new pane focused, saved");
    let order = view.read_with(cx, |v, _| v.reading_order());
    let taken =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.save_job.lock().unwrap_or_else(std::sync::PoisonError::into_inner).taken);
    view.update(cx, |v, cx| v.focus_pane(order[0], crate::pane::KeysTo::Pane, cx));
    frame(cx, window);
    let before = taken(cx);
    // Timer by timer, one task at a time (the executor's `advance_clock`
    // runs every task it wakes): stopped as G1's job has read its layout.
    let mut n = 0;
    while taken(cx) == before {
        if !cx.dispatcher.tick(false) {
            assert!(cx.dispatcher.advance_clock_to_next_timer(), "no timer left: G1's save never came");
        }
        n += 1;
        assert!(n < 100_000, "G1's delayed save never read its layout");
    }
    assert_eq!(focused(cx), Some(1), "G1's write is still to come");
    view.update(cx, |v, cx| v.focus_pane(order[1], crate::pane::KeysTo::Pane, cx));
    view.update(cx, |v, _| v.flush_tabs());
    assert_eq!(focused(cx), Some(1), "G2 flushed");
    cx.run_until_parked();
    assert_eq!(focused(cx), Some(1), "G2's layout last: G1's write found the slot closed");
    let _ = names(&mut a);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #59 r2 L1: a delayed layout write that finished while the quit
/// came does not fool the flush. G0 (the new pane focused) is saved; G1
/// (the focus to the first pane) is written; the focus goes back (G0
/// again: the layout's generation is its content's) and the window's
/// flush runs. The file must end on G0. Two orderings: (a) the app's own
/// delayed write has finished, its continuation (which notes what it
/// wrote) not yet run; (b) a write of G1 holds the slot's lock as the flush
/// comes (a thread in [`crate::app::SaveJob::write`], as the delayed save's
/// background task is, held at a barrier): the flush waits for it, then
/// writes G0 after it. Before, the flush trusted `layout_written` (G0) and
/// wrote nothing: the file kept G1.
#[gpui_kit::test]
fn a_delayed_layout_write_that_finished_as_the_quit_came_is_written_over(cx: &mut TestAppContext) {
    use crate::app::{SaveJob, SaveKind};
    for held in [false, true] {
        let what = if held { "(b) a write holding the lock" } else { "(a) a write finished, its continuation not run" };
        let name = if held { "persist-held" } else { "persist-done" };
        let (window, view, client, mut a, _attaches, _events) = on_a(cx, name, None);
        let k = format!("test:{name}-{}", std::process::id());
        let path = crate::theme::settings_path().expect("the tests' settings file");
        view.update(cx, |v, _| v.tabs_key = Some(k.clone()));
        frame(cx, window);
        key(cx, window, "ctrl-\\");
        wait_saved(cx, window);
        let focused = |_: &mut TestAppContext| crate::theme::load_layout(&path, &k).map(|l| l.focused);
        assert_eq!(focused(cx), Some(1), "{what}: G0, the new pane focused, saved");
        let order = view.read_with(cx, |v, _| v.reading_order());
        let writes = |cx: &mut TestAppContext| {
            view.read_with(cx, |v, _| v.save_job.lock().unwrap_or_else(std::sync::PoisonError::into_inner).writes)
        };
        view.update(cx, |v, cx| v.focus_pane(order[0], crate::pane::KeysTo::Pane, cx));
        frame(cx, window);
        if !held {
            // Task by task until G1's write is done (a write counted), its
            // continuation still queued.
            let before = writes(cx);
            let mut n = 0;
            while writes(cx) == before {
                if !cx.dispatcher.tick(false) {
                    assert!(cx.dispatcher.advance_clock_to_next_timer(), "{what}: no timer left: G1's save never came");
                }
                n += 1;
                assert!(n < 100_000, "{what}: G1's delayed save never wrote");
            }
            assert_eq!(focused(cx), Some(0), "{what}: G1 in the file");
            view.update(cx, |v, cx| v.focus_pane(order[1], crate::pane::KeysTo::Pane, cx));
            view.update(cx, |v, _| v.flush_tabs());
        } else {
            let (slot, g1, l1) = view.read_with(cx, |v, _| (v.save_job.clone(), v.layout_gen(), v.layout_now()));
            let (holding_tx, holding) = mpsc::channel();
            let (go, go_rx) = mpsc::channel::<()>();
            let (p, key1) = (path.clone(), k.clone());
            let writer = std::thread::spawn(move || {
                SaveJob::write(&slot, SaveKind::Layout, g1, || {
                    holding_tx.send(()).expect("the test");
                    crate::test_waits::recv(&go_rx).expect("the test");
                    // The flush is waiting for the lock by now.
                    std::thread::sleep(Duration::from_millis(150));
                    crate::theme::save_layout(&p, &key1, &l1)
                })
            });
            crate::test_waits::recv(&holding).expect("the writer holds the lock");
            view.update(cx, |v, cx| v.focus_pane(order[1], crate::pane::KeysTo::Pane, cx));
            go.send(()).expect("the writer");
            // Waits for the writer's lock, then writes.
            view.update(cx, |v, _| v.flush_tabs());
            assert!(writer.join().expect("the writer").expect("G1 written"), "{what}: G1 was written");
        }
        assert_eq!(focused(cx), Some(1), "{what}: the flush wrote G0 after G1");
        cx.run_until_parked();
        assert_eq!(focused(cx), Some(1), "{what}: G0 last");
        let _ = names(&mut a);
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
}

/// Shady #59 L5.1 (#55 n1): what a pane draws keeps its element state
/// through split → one pane. A press held on pane A's Allow while the other
/// pane closes (Off: in its frame, the layout one pane now), let go on it
/// after, is a click: A's prompt is answered. The one-pane path draws the
/// pane under its `("pane", n)` as the split does; without it the press
/// was forgotten at the landing.
#[gpui_kit::test]
fn a_press_held_through_the_last_close_is_still_a_click(cx: &mut TestAppContext) {
    let (window, view, client, mut a, attaches, events) = on_a(cx, "n1-press", Some(1));
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let (pa, pb) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    level(cx, crate::motion::Level::Off);
    // A's card armed (its buttons act `ARM` after it is shown: a press
    // before then is refused, whatever the layout does).
    let armed =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.get(pa).is_some_and(|p| p.card.as_ref().is_some_and(|c| c.armed())));
    for _ in 0..100 {
        if armed(cx) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        frame(cx, window);
    }
    assert!(armed(cx), "A's card armed");
    let at = in_pane_at(cx, window, "allow-fill", pa);
    VisualTestContext::from_window(window, cx).simulate_mouse_move(at, None, Default::default());
    mouse(cx, window, at, "down", 1);
    assert!(names(&mut a).is_empty(), "a press alone answers nothing");
    view.update(cx, |v, cx| {
        v.in_pane(pb, |v| v.close_pane());
        cx.notify();
    });
    frame(cx, window);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.flight.is_some())), (1, false), "one pane, landed");
    let mut vcx = VisualTestContext::from_window(window, cx);
    let now = vcx.debug_bounds("allow-fill").or_else(|| vcx.debug_bounds("allow-outline")).expect("A's Allow").center();
    // The pointer, held, to where Allow is now; let go there.
    mouse(cx, window, now, "move", 1);
    frame(cx, window);
    mouse(cx, window, now, "up", 1);
    // The answer goes out on the client's thread: waited for (the tests
    // run at once, each 400 ms read can come up short).
    let mut sent = Vec::new();
    for _ in 0..10 {
        frame(cx, window);
        sent.extend(names(&mut a));
        if !sent.is_empty() {
            break;
        }
    }
    assert_eq!(sent, ["answer"], "the press held through the landing, let go on Allow: a click");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// #55 n5's second half, seen (Shady #59 L5.2): a restored layout whose
/// second pane's session is gone collapses after the restore's lists, and
/// that collapse is not written back: no write at all, the file as it was.
#[gpui_kit::test]
fn a_quiet_collapse_is_not_written_back(cx: &mut TestAppContext) {
    let k = format!("test:persist-collapse-{}", std::process::id());
    let path = crate::theme::settings_path().expect("the tests' settings file");
    let layout = two_across(&[A], &[GONE], 0);
    crate::theme::save_layout(&path, &k, &layout).unwrap();
    let before = file_keys(&path, &k);
    let (window, view, client, attaches, _events) = launch_with(cx, "persist-collapse", Some(layout), None);
    view.update(cx, |v, _| v.tabs_key = Some(k.clone()));
    for _ in 0..=crate::tabs::RESTORE_LISTS {
        list(cx, window, &view, &[A]);
        wait_saved(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "collapsed to one pane");
    assert_eq!(writes(cx, &view), 0, "no write at all");
    assert_eq!(file_keys(&path, &k), before, "the file as it was");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// #60 L2 (and #50 L4): nothing is drawn after a move lands, at Full and
/// at Off: a 2-pane rail fold and unfold and a close, the sessions at
/// rest and the pointer resting where the right panel's strip comes (the
/// film's scene). A new column's rows get their hint with no frame of
/// their own when the bar read it ahead: on `cf28780` every re-wrap drew
/// one more frame, the same pixels (a widening landing at Full, every
/// move at Off). The strip that comes in under the pointer (a split, an
/// unfold) is lit in the frame it is drawn: on `cf28780` its fill came a
/// frame later, at Off the one frame after the move. Frame by frame
/// drawn ([`draws`]), only the frames the app asks for ([`asked_frame`]).
/// A split's new pane takes the focus, and its frames after the move are
/// its own (its composer's), so a split is checked for its strip. At
/// Full, within 100 ms of the landing (a follow-up, a settling tail).
#[gpui_kit::test]
fn nothing_is_drawn_after_a_landing(cx: &mut TestAppContext) {
    use crate::motion::Level;
    // Each case's window goes: the next one's frames are its own.
    let close = |cx: &mut TestAppContext, window: AnyWindowHandle| {
        cx.update_window(window, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    };
    let strip = crate::app::tkey("right-strip", 0);
    let mut fails = Vec::new();
    for l in [Level::Full, Level::Off] {
        for case in ["a split", "the rail folds", "the rail unfolds", "a close"] {
            let (window, view) = demo_split(cx, 1920., 1080.);
            panels_at(cx, &view, true, 480, 520);
            view.update(cx, |v, cx| {
                for m in v.model.sessions.iter_mut() {
                    (m.busy, m.subagents_running) = (false, 0);
                }
                v.model.rail_gen += 1;
                cx.notify();
            });
            frame(cx, window);
            level(cx, l);
            frame(cx, window);
            // The pointer at rest over the strip's place: the last input
            // is the pointer's (a key since would hide every hover).
            VisualTestContext::from_window(window, cx).simulate_mouse_move(gpui_kit::point(px(1900.), px(1070.)), None, Default::default());
            let split = |cx: &mut TestAppContext| {
                cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.split_pane(None, w, cx))).expect("the window");
            };
            let unfocus = |cx: &mut TestAppContext| {
                cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| w.focus(&v.focus, cx))).expect("the window");
            };
            let settle = |cx: &mut TestAppContext| {
                land(cx, window, &view);
                for _ in 0..60 {
                    frame_144(cx, window, &view);
                }
            };
            if case != "a split" {
                split(cx);
                unfocus(cx);
                settle(cx);
                if case == "the rail unfolds" {
                    view.update(cx, |v, cx| v.toggle_panel(true, cx));
                    settle(cx);
                }
                view.update(cx, |v, _| v.panels_save = None);
            }
            let seen = draws(cx, &view, move |v| {
                let (w, h) = v.centre.get();
                let mut edges = vec![v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown];
                for (_, r) in v.panes.drawn.rects(crate::split::Rect { x: 0., y: 0., w, h }) {
                    edges.extend([r.x, r.w]);
                }
                (v.fx.now(), edges, v.fx.hover_to(strip))
            });
            match case {
                "a split" => split(cx),
                "a close" => view.update(cx, |v, cx| {
                    let last = *v.reading_order().last().expect("a pane");
                    v.in_pane(last, |v| v.close_pane());
                    cx.notify();
                }),
                _ => view.update(cx, |v, cx| v.toggle_panel(true, cx)),
            }
            view.update(cx, |v, _| v.panels_save = None);
            cx.run_until_parked();
            let comes = matches!(case, "a split" | "the rail unfolds");
            if l == Level::Off {
                // Its frame's asks, if it made any.
                cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx)).expect("the window");
                cx.run_until_parked();
                let seen = seen.take();
                if seen.len() != 1 && case != "a split" {
                    fails.push(format!("{l:?}, {case}: {} frames drawn, not the move's alone: {seen:?}", seen.len()));
                }
                if comes && seen.first().is_some_and(|s| s.2 != Some(1.)) {
                    fails.push(format!("{l:?}, {case}: the strip under the pointer unlit in the move's frame: {seen:?}"));
                }
                close(cx, window);
                continue;
            }
            // A split focuses its new pane: its caret's blink is a frame
            // every 500 ms.
            unfocus(cx);
            // A second at 144 Hz, a frame drawn only when the app asks for
            // one ([`frame_144`] draws one each step).
            for _ in 0..144 {
                asked_frame(cx, window, &view);
            }
            view.update(cx, |v, _| v.on_draw = None);
            let landed = view.read_with(cx, |v, _| !v.panel_fx.moving() && v.panes.flight.is_none());
            let seen = seen.take();
            let last = (1..seen.len()).rev().find(|&i| seen[i].1 != seen[i - 1].1).unwrap_or(0);
            if !landed || last < 10 {
                fails.push(format!("{l:?}, {case}: it didn't fly and land ({last} frames, landed {landed})"));
                close(cx, window);
                continue;
            }
            // The landing's own: within 100 ms (a follow-up frame, a
            // spring's settling tail: 6-7 frames). A later one is not the
            // landing's: no motion ask and none of the app's frame
            // requests drew it (one, ~0.5 s after a fold or an unfold
            // under the resting pointer, is a notify of its own).
            let after: Vec<f64> = seen[last + 1..].iter().map(|s| s.0.duration_since(seen[last].0).as_secs_f64() * 1000.).collect();
            if case != "a split" && after.iter().any(|&ms| ms < 100.) {
                fails.push(format!("{l:?}, {case}: frames drawn after the landing (frame {last}), ms after it: {after:.1?}"));
            }
            if comes && seen[last].2 != Some(1.) {
                fails.push(format!("{l:?}, {case}: the strip under the pointer unlit as it lands: {:?}", seen[last].2));
            }
            close(cx, window);
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Shady #58 n1: [`frame_144`] is `ui_tests`' `fluid_frame` (the harness
/// clock, timer by timer) in this file's spelling, and [`asked_frame`] is
/// it without the step's own draw: a change to `fluid_frame` (a protected
/// helper, changed on purpose) fails here until these follow it, as
/// `test_kit`'s `kit_matches_ui_tests` does for its copies.
#[test]
fn frame_144_matches_fluid_frame() {
    // A helper's body: from the line after its `fn` to its closing brace.
    let body = |src: &str, name: &str| -> String {
        let src = src.replace("\r\n", "\n");
        let at = src.find(&format!("\nfn {name}(")).unwrap_or_else(|| panic!("{name}"));
        let open = src[at..].find("{\n").expect("its body") + at + 2;
        let end = src[open..].find("\n}\n").expect("its end") + open + 1;
        src[open..end].to_string()
    };
    let (ui, ours) = (include_str!("ui_tests.rs"), include_str!("split_tests.rs"));
    let fluid = body(ui, "fluid_frame");
    // This file's spelling: `Duration` imported, the window `window`.
    let spelt = |b: String| {
        b.replace("Duration::from_micros", "std::time::Duration::from_micros").replace(
            "cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx))",
            "cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx))",
        )
    };
    assert_eq!(spelt(body(ours, "frame_144")), fluid, "frame_144 drifted from ui_tests' fluid_frame");
    let notify = "    view.update(cx, |_, cx| cx.notify());\n";
    assert_eq!(spelt(body(ours, "asked_frame")), fluid.replacen(notify, "", 1), "asked_frame drifted from fluid_frame");
}

/// Shady #59 M1: the tests' split-on lane is what it says. Every test's
/// view is built by [`AppView::new`] (here as in the protected fixtures:
/// `test_kit`'s `open_with`, which `held_tests`, `send_tests` and
/// `hang_tests` use, and `ui_tests`' own `open`), and none of the
/// protected suites sets `app_settings` after: with `--features
/// split-on-tests` every view starts with split panes on, one pane;
/// without, off. A launch with no settings file has them on (the run's
/// override unset).
#[gpui_kit::test]
fn the_split_on_lane_builds_every_view_split_on(cx: &mut TestAppContext) {
    let (_window, view) = open(cx);
    let (on, panes) = view.read_with(cx, |v, _| (v.split_on(), v.panes.len()));
    assert_eq!(on, cfg!(feature = "split-on-tests"), "a test's view, split panes on in the lane alone");
    assert_eq!(panes, 1, "one pane");
    let protected = [
        ("ui_tests.rs", include_str!("ui_tests.rs")),
        ("held_tests.rs", include_str!("held_tests.rs")),
        ("send_tests.rs", include_str!("send_tests.rs")),
        ("test_kit.rs", include_str!("test_kit.rs")),
        ("hang_tests.rs", include_str!("hang_tests.rs")),
    ];
    for (file, src) in protected {
        assert!(src.contains("AppView::new(") || !src.contains("cx.new("), "{file}: its views by AppView::new");
        assert!(!src.contains("app_settings"), "{file} sets app_settings: the lane would not be split on");
    }
    if crate::settings_view::split_env().is_none() {
        assert!(crate::settings_view::AppSettings::load(None).split, "no settings file: split panes on");
    }
}

/// Shady #59 M1, the split-on lane at two panes: the protected suites' key
/// scenarios at one pane and at two, split panes on both times (the second
/// pane on A by Ctrl+\, focused). A long A, the focused pane's list
/// scrolled back as the wheel leaves it (`send_tests`' `scroll_back`): a
/// message typed in its composer goes out on A's attachment and brings that
/// pane back to its end (`send_tests`' submit), and text streamed after
/// keeps it there, the other pane at its end too (`held_tests`' tail
/// follow). Ctrl+T and Ctrl+W after the send act in the focused pane alone
/// (the window's fallbacks). With the keys off the composer, a second at
/// rest draws nothing (`ui_tests`' idle, by #61's [`idle_step`]).
#[gpui_kit::test]
fn the_key_scenarios_hold_at_one_and_at_two_panes(cx: &mut TestAppContext) {
    let mut fails = Vec::new();
    for panes in [1usize, 2] {
        let messages = synaps_client::synthetic::history(60, 7);
        let long = json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
            "meta": {"id": A, "model": "m", "lifecycle": "live"},
            "conversation": {"api_messages": messages, "messages_len": messages.len()}, "pending_prompts": []});
        let (window, view, client, mut a, _attaches, events) = on_a_as(cx, &format!("lane-{panes}"), long);
        if panes == 2 {
            key(cx, window, "ctrl-\\");
            land(cx, window, &view);
        }
        let (focused, other) = view.read_with(cx, |v, _| (v.panes.focused, v.panes.ids().into_iter().find(|i| *i != v.panes.focused)));
        assert_eq!(view.read_with(cx, |v, _| (v.split_on(), v.panes.len())), (true, panes), "split panes on, {panes} pane(s)");
        let _ = cx.update_window(window, |_, w, cx| {
            let h = view.read(cx).panes.cur.composer.read(cx).focus_handle(cx);
            w.focus(&h, cx);
        });
        for _ in 0..10 {
            frame(cx, window);
        }
        let at_end = |cx: &mut TestAppContext, id: crate::pane::PaneId| view.update(cx, |v, _| v.in_pane(id, |v| v.at_end()).flatten());
        // Its end, then 600 px back.
        view.update(cx, |v, cx| {
            v.panes.cur.list.scroll_to_end();
            cx.notify();
        });
        for _ in 0..4 {
            frame(cx, window);
        }
        view.update(cx, |v, cx| {
            v.panes.cur.scroller.touch();
            v.panes.cur.list.scroll_by(px(-600.));
            cx.notify();
        });
        for _ in 0..4 {
            frame(cx, window);
        }
        if at_end(cx, focused) != Some(false) {
            fails.push(format!("{panes} pane(s): the focused pane not scrolled back: {:?}", at_end(cx, focused)));
        }
        type_enter(cx, window, "lane: where does the refresh race start?");
        let sent = names(&mut a);
        if sent != ["submit"] {
            fails.push(format!("{panes} pane(s): the message on A's attachment: {sent:?}"));
        }
        for _ in 0..30 {
            frame(cx, window);
        }
        if at_end(cx, focused) != Some(true) {
            fails.push(format!("{panes} pane(s): the send didn't bring its pane to the end: {:?}", at_end(cx, focused)));
        }
        // The reply, streamed.
        let items = |v: &AppView| v.model.lives[v.panes.cur.live].transcript.items().len();
        let n0 = view.read_with(cx, |v, _| items(v));
        a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t", "event": {"ev": "turn_started", "turn_baseline": 0, "trigger": "user"}}));
        for (i, seq) in (2..8).enumerate() {
            let text = format!("Line {i}: the relay's token cache, two lines down from the lock, under the read guard.\n").repeat(3);
            a.send(json!({"type": "event", "session_id": A, "seq": seq, "ts": "t",
                "event": {"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": text}}}));
        }
        a.send(json!({"type": "event", "session_id": A, "seq": 8, "ts": "t", "event": {"ev": "idle"}}));
        feed(cx, window, &view, &events, "the reply", |v| items(v) > n0 && !v.model.lives[v.panes.cur.live].transcript.busy());
        for _ in 0..30 {
            frame(cx, window);
        }
        for id in std::iter::once(focused).chain(other) {
            if at_end(cx, id) != Some(true) {
                fails.push(format!("{panes} pane(s): pane {} not at its end as the reply streams: {:?}", id.index(), at_end(cx, id)));
            }
        }
        // Ctrl+T, Ctrl+W: the focused pane's.
        let count = |cx: &mut TestAppContext, id: crate::pane::PaneId| {
            view.read_with(cx, |v, _| {
                let p = v.panes.get(id).expect("the pane");
                if p.id == v.panes.cur.id { v.model.tabs.len() } else { p.parked.tabs.len() }
            })
        };
        let before = (count(cx, focused), other.map(|o| count(cx, o)));
        press(cx, window, "ctrl-t");
        let now = (count(cx, focused), other.map(|o| count(cx, o)));
        if now != (before.0 + 1, before.1) {
            fails.push(format!("{panes} pane(s): Ctrl+T, the tabs (focused, other): {before:?} → {now:?}"));
        }
        // A hand's pause: Ctrl+W within `FoldKeys::PAIR` (30 ms) of Ctrl+T's
        // key-up is its repeat (the tab keys share one gate).
        press(cx, window, "ctrl-w");
        let now = (count(cx, focused), other.map(|o| count(cx, o)), view.read_with(cx, |v, _| v.panes.len()));
        if now != (before.0, before.1, panes) {
            fails.push(format!("{panes} pane(s): Ctrl+W, the tabs (focused, other) and panes: {before:?} → {now:?}"));
        }
        // At rest: nothing drawn (the rail's rows at rest too, as in
        // `nothing_is_drawn_after_a_landing`: the fake daemon's list).
        view.update(cx, |v, cx| {
            for m in v.model.sessions.iter_mut() {
                (m.busy, m.subagents_running) = (false, 0);
            }
            v.model.rail_gen += 1;
            cx.notify();
        });
        let _ = cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| w.focus(&v.focus, cx)));
        idle_step(cx, window, &view, Duration::from_millis(600));
        let ran: usize = (0..144).map(|_| idle_step(cx, window, &view, Duration::from_micros(6944))).sum();
        if ran != 0 {
            fails.push(format!("{panes} pane(s): {ran} frames drawn in a second at rest"));
        }
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

// ── #67's composer pickers, per pane (Shady #69 r3) ─────────────────────

/// The `set` commands among a connection's next frames.
fn sets(c: &mut Conn) -> Vec<Value> {
    c.cmds(Duration::from_millis(400)).into_iter().filter(|c| c["cmd"] == "set").collect()
}

/// In the open picker, `value`'s row chosen by the keyboard (Down to it,
/// Enter), as a user would.
fn choose(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>, value: &str) {
    let row = view.read_with(cx, |v, _| {
        let (m, t) = v.choice.menu.as_ref().expect("a picker open");
        let i = t.values.iter().position(|(x, _)| x == value).unwrap_or_else(|| panic!("{value} listed: {:?}", t.values));
        m.items.iter().position(|it| it.act == i).expect("its row")
    });
    let sel = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.choice.menu.as_ref().and_then(|(m, _)| m.sel));
    for _ in 0..20 {
        if sel(cx) == Some(row) {
            break;
        }
        key(cx, window, "down");
    }
    assert_eq!(sel(cx), Some(row), "{value}'s row selected");
    key(cx, window, "enter");
}

/// #67 with split panes on (its PR: "each pane's labels open their own
/// session's picker; the pick goes to the attachment the menu opened
/// for"). Two panes, A (thinking `low`) and B (`high`), B focused. A click
/// on A's level opens A's picker over A's label (A's pane focused by the
/// press) and its pick is a `set` on A's attachment only; A's label
/// follows A's `setting_changed`, B's stays. B's picker, picked, then
/// focus back on A before the daemon answers: B's refusal is said on B's
/// composer, never A's. Ctrl+K's "Change thinking…" opens the focused
/// pane's picker.
#[gpui_kit::test]
fn the_composer_pickers_are_each_panes_own(cx: &mut TestAppContext) {
    let with_view = |mut v: Value, model: &str, level: &str| {
        v["view"] = json!({"model": model, "reasoning_level": level});
        v
    };
    let (window, view, client, mut a, attaches, events) =
        on_a_in(cx, "panes-choice", with_view(attached(A, None), "ma", "low"), (1920., 1080.));
    key(cx, window, "ctrl-\\");
    view.update(cx, |v, cx| {
        v.open_tab(crate::app::Open::Attach { session_id: B.into(), fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    let mut b = crate::test_waits::recv(&attaches).expect("B's attach at the daemon");
    b.send(with_view(attached(B, None), "mb", "high"));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    frame(cx, window);
    let (pa, pb) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let _ = (a.cmds(Duration::from_millis(400)), b.cmds(Duration::from_millis(400)));
    let live = |v: &AppView, id| v.panes.get(id).expect("the pane").live;
    let level = |cx: &mut TestAppContext, id| {
        view.read_with(cx, |v, _| v.model.lives[live(v, id)].stats.view.as_ref().map(|s| s.effort.clone()).unwrap_or_default())
    };
    let seq = |cx: &mut TestAppContext, id| view.read_with(cx, |v, _| v.model.lives[live(v, id)].attachment().expect("attached"));
    let open = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.choice.menu.as_ref().map(|(m, t)| (t.pane, t.attach_seq, t.current.clone(), m.at, m.sel)))
    };
    let note =
        |cx: &mut TestAppContext, id| view.read_with(cx, |v, _| v.panes.get(id).and_then(|p| p.watch_note.as_ref().map(|n| n.to_string())));
    let over = |cx: &mut TestAppContext, at: crate::menu::At, id: crate::pane::PaneId| {
        let crate::menu::At::Point(pt) = at else { panic!("a picker sits at a point") };
        let s = drawn(cx, window, format!("pane {}", id.index())).expect("the pane drawn");
        let x = f32::from(pt.x);
        x >= s[0] && x <= s[0] + s[2]
    };
    assert_eq!((level(cx, pa), level(cx, pb)), ("low".into(), "high".into()), "each session's level");
    assert!(drawn(cx, window, format!("choice-thinking {}", pa.index())).is_some(), "A's level drawn as a picker's label");
    assert!(drawn(cx, window, format!("choice-thinking {}", pb.index())).is_some(), "B's level drawn as a picker's label");

    // A's level, from B's pane: A's picker, over A's label.
    let at = mid(cx, window, format!("choice-thinking {}", pa.index()));
    click_at(cx, window, at);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pa, "the press focused A's pane");
    let (pane, s, current, at, _) = open(cx).expect("a click on A's level opens a picker");
    assert_eq!((pane, s, current.as_str()), (pa, seq(cx, pa), "low"), "A's picker: A's pane, A's attachment, A's level");
    assert!(over(cx, at, pa), "over A's label, in A's pane: {at:?}");
    choose(cx, window, &view, "xhigh");
    let (to_a, to_b) = (sets(&mut a), sets(&mut b));
    assert!(to_b.is_empty(), "nothing set on B: {to_b:?}");
    assert_eq!(to_a.len(), 1, "one set on A: {to_a:?}");
    assert_eq!(to_a[0]["setting"], json!({"setting": "reasoning_level", "level": "xhigh"}));
    a.send(json!({"type": "event", "session_id": A, "seq": 1, "ts": "t", "event": {"ev": "setting_changed",
        "applied": {"id": to_a[0]["id"], "setting": "reasoning_level", "ok": true, "message": null,
        "view": {"model": "ma", "reasoning_level": "xhigh"}}}}));
    feed(cx, window, &view, &events, "A's level applied", |v| {
        v.choice.menu.is_none() && v.model.lives[live(v, pa)].stats.view.as_ref().is_some_and(|s| s.effort == "xhigh")
    });
    assert_eq!((level(cx, pa), level(cx, pb)), ("xhigh".into(), "high".into()), "A's label follows A's session; B's stays");

    // B's level: B's picker; focus back on A before the daemon refuses.
    let at = mid(cx, window, format!("choice-thinking {}", pb.index()));
    click_at(cx, window, at);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pb, "the press focused B's pane");
    let (pane, s, current, at, _) = open(cx).expect("a click on B's level opens a picker");
    assert_eq!((pane, s, current.as_str()), (pb, seq(cx, pb), "high"), "B's picker: B's pane, B's attachment, B's level");
    assert!(over(cx, at, pb), "over B's label, in B's pane: {at:?}");
    choose(cx, window, &view, "off");
    let (to_a, to_b) = (sets(&mut a), sets(&mut b));
    assert!(to_a.is_empty(), "nothing set on A: {to_a:?}");
    assert_eq!(to_b.len(), 1, "one set on B: {to_b:?}");
    assert_eq!(to_b[0]["setting"], json!({"setting": "reasoning_level", "level": "off"}));
    key(cx, window, "escape");
    assert!(open(cx).is_none(), "Esc closed the picker");
    let at = mid(cx, window, format!("composer-frame {}", pa.index()));
    click_at(cx, window, at);
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.focused), pa, "A's pane focused");
    b.send(json!({"type": "event", "session_id": B, "seq": 1, "ts": "t", "event": {"ev": "setting_changed",
        "applied": {"id": to_b[0]["id"], "setting": "reasoning_level", "ok": false, "message": "off is not for this model",
        "view": {"model": "mb", "reasoning_level": "high"}}}}));
    feed(cx, window, &view, &events, "B's refusal", |v| v.panes.get(pb).is_some_and(|p| p.watch_note.is_some()));
    assert_eq!(note(cx, pb).as_deref(), Some("thinking not changed: off is not for this model"), "said on B's composer");
    assert!(note(cx, pa).is_none_or(|n| !n.contains("not changed")), "never on A's: {:?}", note(cx, pa));
    assert_eq!((level(cx, pa), level(cx, pb)), ("xhigh".into(), "high".into()), "neither level moved");

    // From the keyboard: Ctrl+K's "Change thinking…" in the focused pane (A).
    key(cx, window, crate::app::palette_keys(cfg!(target_os = "macos")).1);
    frame(cx, window);
    VisualTestContext::from_window(window, cx).simulate_input("change thinking");
    frame(cx, window);
    key(cx, window, "enter");
    frame(cx, window);
    let (pane, s, current, at, sel) = open(cx).expect("Ctrl+K's Change thinking… opens a picker");
    assert_eq!((pane, s, current.as_str()), (pa, seq(cx, pa), "xhigh"), "the focused pane's: A's");
    assert!(over(cx, at, pa), "over A's label: {at:?}");
    assert!(sel.is_some(), "from the keyboard, its current row selected");
    key(cx, window, "escape");
    assert!(sets(&mut a).is_empty() && sets(&mut b).is_empty(), "nothing set");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── Room is the only limit (split.md §0, the 2026-11-25 note) ───────────

/// The pane key `n` (1-based) as this platform binds it: Alt+N, Ctrl+N on
/// macOS.
fn nth_key(n: usize) -> String {
    format!("{}-{n}", if cfg!(target_os = "macos") { "ctrl" } else { "alt" })
}

/// Haseeb, 2026-11-25: "i can only have 3 columns?? thats too less." No
/// cap across or down: at 1920 × 1080 with both panels at their strips,
/// Ctrl+\ four times gives five panes side by side, each at least 320
/// wide (a halved pane under 320 evens out its row instead). A sixth
/// across would be under 320: an explicit split right is refused with "no
/// room to split this pane", and Ctrl+\ splits down instead. Nine panes
/// in all; the tenth is refused, "9 panes is the most". Alt+1–9 focus
/// each in reading order.
#[gpui_kit::test]
fn five_across_fit_at_1920_with_both_panels_folded_and_a_tenth_is_refused(cx: &mut TestAppContext) {
    use crate::split::{PANE_MIN_H, PANE_MIN_W};
    let (window, view) = demo_split(cx, 1920., 1080.);
    let (rail, right) = crate::app::panel_keys(cfg!(target_os = "macos"));
    press(cx, window, rail);
    press(cx, window, right);
    frame(cx, window);
    let strip = crate::app::STRIP;
    let shown = view.read_with(cx, |v, _| (v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown));
    assert_eq!(shown, (strip, strip), "both panels at their strips");
    for n in 2..=5 {
        key(cx, window, "ctrl-\\");
        frame(cx, window);
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), n, "Ctrl+\\ made pane {n}");
    }
    let order = view.read_with(cx, |v, _| v.reading_order());
    let boxes: Vec<[f32; 4]> = order.iter().map(|id| pane_box(cx, window, *id)).collect();
    assert!(boxes.iter().all(|b| b[1] == boxes[0][1] && b[3] == boxes[0][3]), "five across, one row: {boxes:?}");
    assert!(boxes.windows(2).all(|w| w[1][0] > w[0][0]), "side by side: {boxes:?}");
    assert!(boxes.iter().all(|b| b[2] >= PANE_MIN_W), "each at least 320 wide: {boxes:?}");
    assert!(!view.read_with(cx, |v, _| v.panes.folded), "not folded");
    // A sixth across: refused, in the composer's words; nothing changed.
    let tree = view.read_with(cx, |v, _| v.panes.tree.clone());
    let _ = cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.split_pane(Some(crate::split::Side::Right), w, cx)));
    frame(cx, window);
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.tree, tree, "no sixth across");
        assert_eq!(v.panes.cur.watch_note.as_deref(), Some("no room to split this pane"));
    });
    // Ctrl+\ splits the narrow pane down instead.
    let at = view.read_with(cx, |v, _| v.panes.focused);
    let above = pane_box(cx, window, at);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    let below = view.read_with(cx, |v, _| v.panes.focused);
    assert_ne!(below, at, "a new pane");
    let (a, b) = (pane_box(cx, window, at), pane_box(cx, window, below));
    assert!(a[0] == above[0] && b[0] == above[0] && b[1] > a[1], "split down: {a:?} over {b:?}");
    // Down to nine: the other columns' tops split down too.
    for n in 1..=3 {
        key(cx, window, &nth_key(n));
        key(cx, window, "ctrl-\\");
        frame(cx, window);
    }
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 9, "nine panes");
    let order = view.read_with(cx, |v, _| v.reading_order());
    assert!(
        order.iter().map(|id| pane_box(cx, window, *id)).all(|b| b[2] >= PANE_MIN_W && b[3] >= PANE_MIN_H),
        "every pane at least 320 × 240"
    );
    // The tenth: refused, by the total alone.
    key(cx, window, "ctrl-\\");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 9, "no tenth pane");
        assert_eq!(v.panes.cur.watch_note.as_deref(), Some("9 panes is the most"));
    });
    // Alt+1–9 (Ctrl on macOS): pane N in reading order, each.
    for n in (1..=9).rev() {
        key(cx, window, &nth_key(n));
        assert_eq!(view.read_with(cx, |v, _| v.panes.focused), order[n - 1], "{} focuses pane {n}", nth_key(n));
    }
}

/// A saved layout is read by its total alone (split.md §0, the 2026-11-25
/// note): six across restored into a 1280 window takes the too-small path
/// (folded, nothing lost); widened to 2560 it is six across, each at
/// least 320; narrowed to 1920 (six need 1925 between the strips) it
/// folds again, the tree untouched. Five across at 1920 keep the rail at
/// its strip when Ctrl+B would open it (the panes win, split.md §5).
#[gpui_kit::test]
fn six_across_restored_into_1280_fold_and_widened_lay_out(cx: &mut TestAppContext) {
    use crate::split::PANE_MIN_W;
    let six = crate::theme::Layout {
        focused: 0,
        tree: crate::theme::LayoutNode::Split {
            axis: crate::split::Axis::Row,
            ratios: vec![1. / 6.; 6],
            kids: (0..6).map(crate::theme::LayoutNode::Pane).collect(),
        },
        panes: (0..6).map(|_| saved(&[A])).collect(),
    };
    let (window, view, client, attaches, _events) = launch_in(cx, "six-across", Some(six), None, true, (1280., 800.));
    let (len, folded) = view.read_with(cx, |v, _| (v.panes.len(), v.panes.folded));
    assert_eq!(len, 6, "the six panes restored");
    assert!(folded, "1280: too small for six, folded");
    let tree = view.read_with(cx, |v, _| v.panes.tree.clone());
    resize(cx, window, 2560., 1080.);
    let (order, folded) = view.read_with(cx, |v, _| (v.reading_order(), v.panes.folded));
    assert!(!folded, "2560: laid out");
    let boxes: Vec<[f32; 4]> = order.iter().map(|id| pane_box(cx, window, *id)).collect();
    assert!(boxes.iter().all(|b| b[1] == boxes[0][1] && b[2] >= PANE_MIN_W), "six across, each at least 320: {boxes:?}");
    resize(cx, window, 1920., 1080.);
    assert!(view.read_with(cx, |v, _| v.panes.folded), "1920: six don't fit between the strips, folded");
    assert!(view.read_with(cx, |v, _| v.panes.tree.same_shape(&tree)), "the tree untouched");
    // Five across at 1920: Ctrl+B leaves the rail at its strip.
    view.update(cx, |v, cx| {
        v.focus_nth(5, cx);
        v.panels.rail_collapsed = true;
        cx.notify();
    });
    cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(crate::palette::AppAct::ClosePane, w, cx))).unwrap();
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 5, "five left");
    assert!(!view.read_with(cx, |v, _| v.panes.folded), "five fit at 1920");
    let strip = crate::app::STRIP;
    key(cx, window, crate::app::panel_keys(cfg!(target_os = "macos")).0);
    frame(cx, window);
    frame(cx, window);
    let (rail, folded) = view.read_with(cx, |v, _| (v.panel_fx.sides[0].shown, v.panes.folded));
    assert!(rail <= strip + 0.5 && !folded, "the rail yields to five across: {rail}, folded {folded}");
    while attaches.try_recv().is_ok() {}
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}
