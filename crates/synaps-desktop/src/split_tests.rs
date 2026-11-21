//! Split panes step 2, through the view (`AppView`) against a client on a
//! real socket: a fake daemon (std threads) that hands each attach to the
//! test, as `model::fake_daemon`'s.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size};
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
        "conversation": {"api_messages": [], "messages_len": 0}, "pending_prompts": prompts})
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

/// A mouse-down and up at the centre of pane `i` (its `pane i` bounds).
fn click_pane(cx: &mut TestAppContext, window: AnyWindowHandle, i: usize) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, VisualTestContext};
    let mut vcx = VisualTestContext::from_window(window, cx);
    let b = vcx.debug_bounds(format!("pane {i}").leak()).expect("the pane drawn");
    let position = b.center();
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
    a.send(attached(A, prompt));
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
/// view with two panes on two sessions, each with its prompt #1 up: the
/// Allow, HALT and message go out on the focused pane's own attachment,
/// never the other's, in both directions; focus moves by Alt+1 and by a
/// click in the pane. B's attach goes beside A's (A's connection gets no
/// detach), and the pane on B closing detaches B only.
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
    let act = |cx: &mut TestAppContext, what: &str| {
        view.update(cx, |v, _| {
            let key = v.panes.cur.card.as_ref().map(|c| c.key).expect("the focused pane's card");
            assert!(v.answer(key, Some(crate::model::CONFIRM_YES.into())), "{what}: Allow went");
            v.halt();
            assert!(v.enter(format!("hello {what}")), "{what}: the message went");
        })
    };
    act(cx, "B");
    assert_eq!(names(&mut b), ["answer", "cancel", "submit"], "on B's attachment");
    assert!(names(&mut a).is_empty(), "nothing on A's");
    // Alt+1: the first pane in reading order.
    key(cx, window, "alt-1");
    view.read_with(cx, |v, _| assert_eq!((v.panes.focused, v.panes.cur.id), (pa, pa), "Alt+1 focused the first pane"));
    feed(cx, window, &view, &events, "A's card armed again", |v| v.panes.cur.card.as_ref().is_some_and(|c| c.armed()));
    act(cx, "A");
    assert_eq!(names(&mut a), ["answer", "cancel", "submit"], "on A's attachment");
    assert!(names(&mut b).is_empty(), "nothing on B's");
    // A click in the second pane focuses it (a capture: whatever it hit).
    click_pane(cx, window, 1);
    view.read_with(cx, |v, _| assert_eq!(v.panes.focused, pb, "the click focused the second pane"));
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
    feed(cx, window, &view, &events, "armed again", |v| v.panes.cur.card.as_ref().is_some_and(|c| c.armed()));
    view.update(cx, |v, _| {
        let key = v.panes.cur.card.as_ref().map(|c| c.key).expect("the card");
        assert!(v.answer(key, Some(crate::model::CONFIRM_YES.into())));
    });
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
    key(cx, window, "alt-1");
    key(cx, window, "ctrl-shift-\\");
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 4, "2×2 at 1280×800");
    // A fifth is refused, with the reason on the composer's line.
    key(cx, window, "ctrl-\\");
    view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 4);
        assert_eq!(v.panes.cur.watch_note.as_deref(), Some("4 panes is the most"));
    });
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
