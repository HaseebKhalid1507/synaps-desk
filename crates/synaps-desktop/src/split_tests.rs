//! Split panes step 2, through the view (`AppView`) against a client on a
//! real socket: a fake daemon (std threads) that hands each attach to the
//! test, as `model::fake_daemon`'s.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Focusable as _, Pixels, Point, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, px, size,
};
use serde_json::{Value, json};
use synaps_client::{Client, ClientOptions, Endpoint, Event, Identity};

use crate::app::AppView;

const A: &str = "20261121-020000-aaaa";
const B: &str = "20261121-020000-bbbb";
const T: Duration = Duration::from_secs(10);

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
    let deadline = Instant::now() + T;
    loop {
        while let Ok(ev) = events.try_recv() {
            if done(&ev) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The `--offline` demo's window at 1280×800, text step 16 (`ui_tests`'
/// `open_at` without its client: the test gives it one).
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    let wv = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text: 16 }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(1280.), px(800.)) };
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
    let deadline = Instant::now() + T;
    loop {
        let batch: Vec<Event> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        if !batch.is_empty() {
            view.update(cx, |v, cx| v.apply_batch(batch, cx));
        }
        cx.run_until_parked();
        let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if view.read_with(cx, |v, _| done(v)) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
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
    let mut a = attaches.recv_timeout(T).expect("A's attach at the daemon");
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
    let mut a = attaches.recv_timeout(T).expect("A's attach at the daemon");
    a.send(attached(A, Some(1)));
    // The other store, on B: its prompt is #1 too.
    let b = view.update(cx, |v, _| {
        let b = v.model.lives.add();
        let seq = client.attach_more(B, synaps_client::AttachMode::Mirror).expect("B's attach");
        v.model.attaching(b, seq, None, false);
        v.model.asked_for(b, B);
        b
    });
    let mut on_b = attaches.recv_timeout(T).expect("B's attach at the daemon");
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
    let (window, view) = open(cx);
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
    let mut a = attaches.recv_timeout(T).expect("A's attach at the daemon");
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
    let mut b = attaches.recv_timeout(T).expect("B's attach at the daemon, on its own connection");
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
    std::thread::sleep(Duration::from_millis(40));
    key(cx, window, "escape");
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
        let dt = Duration::from_micros(6944);
        view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
        cx.executor().advance_clock(dt);
        let ran = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
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
        let dt = Duration::from_millis(50);
        view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
        cx.executor().advance_clock(dt);
        let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    let f0 = view.read_with(cx, |v, _| v.fx.frames());
    for i in 0..40 {
        let dt = Duration::from_millis(50);
        view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
        cx.executor().advance_clock(dt);
        let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
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

/// A mouse-down and up at `position`.
fn click_at(cx: &mut TestAppContext, window: AnyWindowHandle, position: Point<Pixels>) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
    let mut vcx = VisualTestContext::from_window(window, cx);
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
    feed(cx, window, &view, &events, "the card up in both panes, armed", |v| {
        v.panes.cur.card.as_ref().is_some_and(|c| c.armed()) && v.panes.rest[0].card.is_some()
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
    let mut b = attaches.recv_timeout(T).expect("B's attach at the daemon");
    b.send(attached(B, None));
    feed(cx, window, &view, &events, "B attached in the second pane", |v| v.model.lives[v.panes.cur.live].is_active(B));
    let (pa, pb) = view.read_with(cx, |v, _| (v.panes.rest[0].id, v.panes.cur.id));
    let _ = (names(&mut a), names(&mut b));
    assert!(keys_in(cx, window, &view, pb, false), "B's composer has the keys");
    // A rail click on A: the effect its row gives (`Model::open_row_in`;
    // a find's Enter gives the same).
    view.update(cx, |v, cx| {
        v.effect(crate::model::Effect::Attach { session_id: A.into(), fallback: None, open: false, place: crate::tabs::Place::Current });
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
    let mut b = attaches.recv_timeout(T).expect("B's attach at the daemon");
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
