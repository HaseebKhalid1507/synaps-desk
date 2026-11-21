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
