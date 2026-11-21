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
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};
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

// ── Round 3 (Shady #36 r2): #33's label hang, per pane ──────────────────

/// One 144 Hz frame on the test's clock, drawn (`test_kit`'s
/// `fluid_frame`).
fn frame_144(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) {
    let dt = Duration::from_micros(6944);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
    cx.executor().advance_clock(dt);
    view.update(cx, |_, cx| cx.notify());
    cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
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
            let mut b = attaches.recv_timeout(T).expect("B's attach at the daemon");
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
    assert_eq!(steps[steps.len() - 2], landed, "the last frame that moves draws the landed layout");
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
        key(cx, window, "ctrl-w");
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
    assert_eq!(steps[steps.len() - 2].1[2], w, "the last frame that moves: the one pane across the centre");
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
    let m = crate::theme::Metrics::at(16);
    let mut checked = 0;
    let mut seen = Vec::new();
    // Down to panes under their minimum (a window shrunk after the split;
    // its fold is 3b-2's): the composer reaches its most rows above that.
    for h in (400..560).step_by(2) {
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
    VisualTestContext::from_window(window, cx).simulate_resize(size(px(1920.), px(1080.)));
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
        key(cx, window, first);
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
        key(cx, window, then);
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
    key(cx, window, "ctrl-w");
    assert!(drawn(cx, window, format!("pane-list {}", right.index())).is_none(), "Reduced: the close is one frame");
    assert!(view.read_with(cx, |v, _| v.panes.flight.is_none() && v.panes.closing.is_empty()));
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #42 r2-L1: four panes, one closes, and a split within its flight
/// is handed the closing pane's id (the only one free): no pane jumps
/// (its share keeps closing, a ghost's ground), and every frame steps by
/// at most one frame's spring.
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
    key(cx, window, "ctrl-\\");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "the split took the id");
    assert!(view.read_with(cx, |v, _| v.panes.get(closing).is_some()), "the closing pane's id is the new pane's");
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
