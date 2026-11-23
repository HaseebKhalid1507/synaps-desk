//! #62's split-pane motion tests (fix/split-motion-tails), through the
//! view (`AppView`) against a client on a real socket: the divider on the
//! window's pixels, a takeover's carry, a ride past its start, a panels'
//! flight's wraps, the drop targets. Moved out of `split_tests.rs`, which
//! is protected and stays as on main (Shady #62 r3 R3-2); its helpers
//! these tests use are copied here as they are there, but for the waits:
//! those are `test_waits`' (#74), shared with `split_tests`, never a copy
//! (Shady #62 r5 R5-1: a copy kept the busy channel's way past the
//! backstop that #74 closed).

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
};
use serde_json::{Value, json};
use synaps_client::{Client, ClientOptions, Endpoint, Event, Identity};

use crate::app::AppView;
use crate::test_waits;

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
}

// ── `split_tests`' helpers, as they are there ───────────────────────────

/// `Hello` → `Welcome` (session A listed), an empty list for the control
/// connection, every `Attach` handed to the test.
fn start(name: &str) -> (PathBuf, mpsc::Receiver<Conn>) {
    let dir = std::env::temp_dir().join(format!("synaps-desktop-split-motion-{}-{name}", std::process::id()));
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

/// A frame drawn (its top runs what was deferred to it: the keys' move).
fn frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

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

/// From now on, `shot` of the view at each frame drawn, in order (#60
/// L1: a [`frame_144`] can draw two frames, a motion timer's and its own,
/// so a step's view after it is not every frame's).
fn draws<T: 'static>(cx: &mut TestAppContext, view: &Entity<AppView>, shot: impl Fn(&AppView) -> T + 'static) -> Rc<RefCell<Vec<T>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    view.update(cx, |v, _| v.on_draw = Some(Rc::new(move |v: &AppView| l.borrow_mut().push(shot(v)))));
    log
}

/// A selector's bounds as drawn: left, top, width, height.
fn drawn(cx: &mut TestAppContext, window: AnyWindowHandle, sel: String) -> Option<[f32; 4]> {
    let b = VisualTestContext::from_window(window, cx).debug_bounds(sel.leak())?;
    Some([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)])
}

fn level(cx: &mut TestAppContext, l: crate::motion::Level) {
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(l), lock: None });
        crate::motion::Level::apply(cx);
    });
}

/// The waits this module calls are `test_waits`' (Shady #62 r5 R5-1).
/// Its own tests take `until` and `recv` through a closed channel and a
/// busy one; this one takes `feed` through a view: a backlog past the
/// pump's batch is taken one batch a look, so each look draws and
/// checks (the copy it replaces drained the channel first, so a channel
/// that never emptied never let it look), and a closed channel fails at
/// once, named (the copy spun to its deadline).
#[gpui_kit::test]
fn the_feed_this_module_waits_with_looks_a_batch_at_a_time_and_fails_at_once_when_closed(cx: &mut TestAppContext) {
    let (window, view) = open_sized(cx, 1280., 800.);
    let (tx, rx) = async_channel::unbounded::<Event>();
    let backlog = 2 * crate::model::MAX_BATCH + 5;
    for _ in 0..backlog {
        tx.try_send(Event::Status(synaps_client::Status::Connecting)).expect("queued");
    }
    test_waits::feed(cx, window, &view, &rx, "its first look", |_| true);
    assert_eq!(rx.len(), backlog - crate::model::MAX_BATCH, "one look, one batch");
    drop(tx);
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        test_waits::feed(cx, window, &view, &rx, "an event that never comes", |_| false);
    }));
    let said = failed.expect_err("a closed channel fails").downcast::<String>().map(|s| *s).unwrap_or_default();
    assert!(said.contains("the client closed before an event that never comes"), "{said}");
    assert!(rx.is_empty(), "the backlog applied before it failed");
}

// ── fix/split-motion-tails ──────────────────────────────────────────────

/// `split_tests`' `on_a` in a window `w` × `h`.
#[allow(clippy::type_complexity)]
fn on_a_at(
    cx: &mut TestAppContext,
    name: &str,
    (w, h): (f32, f32),
) -> (AnyWindowHandle, Entity<AppView>, Client, Conn, mpsc::Receiver<Conn>, async_channel::Receiver<Event>) {
    let (window, view) = open_sized(cx, w, h);
    let (path, attaches) = start(name);
    let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::desktop())).expect("a client thread");
    test_waits::until(&events, "connected", |e| matches!(e, Event::Status(synaps_client::Status::Connected { .. })));
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        v.client = Some(client.clone());
        v.model.tabs.open(A, "a", crate::tabs::Place::Current);
        v.reconcile(None);
        cx.notify();
    });
    let mut a = test_waits::recv(&attaches).expect("A's attach at the daemon");
    a.send(attached(A, None));
    test_waits::feed(cx, window, &view, &events, "A attached", |v| v.model.lives[v.panes.cur.live].is_active(A));
    (window, view, client, a, attaches, events)
}

/// The first vertical divider's x in the window, and the frame's instant,
/// as a frame drew it (`AppView::rects_drawn`: the centre's left and the
/// first pane's box; its right edge is the rule's left).
fn divider_drawn(v: &AppView) -> (Instant, Option<f32>) {
    let drawn = v.rects_drawn.borrow();
    let (o, rects) = &*drawn;
    (v.fx.now(), rects.first().map(|(_, r)| o.0 + r.x + r.w))
}

/// Each draw's instant and its first divider's x ([`divider_drawn`]).
type DividerDraws = Rc<RefCell<Vec<(Instant, Option<f32>)>>>;

/// Each draw's divider from `at` on, one per frame instant (a step can
/// draw twice at one instant: #60 L1), the frames with none dropped.
fn divider_path(seen: &DividerDraws, at: usize) -> Vec<(Instant, f32)> {
    let mut out: Vec<(Instant, f32)> = Vec::new();
    for &(t, x) in &seen.borrow()[at..] {
        let Some(x) = x else { continue };
        match out.last_mut() {
            Some(last) if last.0 == t => last.1 = x,
            _ => out.push((t, x)),
        }
    }
    out
}

/// The divider's wobble on the 2-panel settle (#59's note, #54's
/// neighbour): two panes, the rail wide and the right panel yielded to
/// them. The rail folds and the right panel comes back, then the rail
/// unfolds and the right panel yields again: both panels fly, the layout
/// rides neither (the panes are the tree in the centre each frame). Draw
/// by draw (#61's hook) the divider's x in the window moves one way only.
/// On two springs of their own the panels stepped their half-pixels on
/// alternate frames in the tail, so the centre's width (1920 − MARK − both)
/// went 1407.5 / 1408 / 1407.5 and the divider back half a pixel, four
/// times; the yielding panel is now drawn off the causing panel's progress
/// ([`crate::chrome::Tie`]). And at other widths, scales and shares (Shady
/// #62 M2): at 1500 px, 2×, 480/240, the panes rounded from the centre's
/// own edge put the divider at 971.5 then 971 (rail 476.5 → 477, the first
/// pane 495 → 494); they are rounded on the window's pixels now
/// ([`crate::split::Node::rects_in`]).
#[gpui_kit::test]
fn the_divider_moves_one_way_through_a_two_panel_settle(cx: &mut TestAppContext) {
    let mut fails = Vec::new();
    // The window, its scale, the panels' widths, the first pane's share.
    let cases = [
        (1920., 2., 480, 520, 0.5),
        (1500., 2., 480, 240, 0.5),
        (1500., 1.25, 400, 300, 0.5),
        (1700., 1.5, 480, 240, 0.38),
        (1920., 2., 300, 520, 0.62),
    ];
    for (n, (w, s, rail, right, share)) in cases.into_iter().enumerate() {
        let what = format!("{w} px at {s}×, rail {rail}, right {right}, share {share}");
        let (window, view, client, _a, _attaches, _events) = on_a_at(cx, &format!("settle-{n}"), (w, 1080.));
        VisualTestContext::from_window(window, cx).simulate_scale_factor_change(s);
        view.update(cx, |v, cx| {
            let mut p = v.panels;
            (p.rail_width, p.right_width) = (Some(rail), Some(right));
            v.panels = p;
            cx.notify();
        });
        frame(cx, window);
        key(cx, window, "ctrl-\\");
        for _ in 0..10 {
            frame(cx, window);
        }
        view.update(cx, |v, cx| {
            if let crate::split::Node::Split { ratios, .. } = &mut v.panes.tree {
                *ratios = vec![share, 1. - share];
            }
            v.panes.drawn = v.panes.tree.clone();
            cx.notify();
        });
        frame(cx, window);
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 2, "{what}: two panes");
        level(cx, crate::motion::Level::Full);
        let seen = draws(cx, &view, divider_drawn);
        for motion in ["fold", "unfold"] {
            let at = seen.borrow().len();
            view.update(cx, |v, cx| v.toggle_panel(true, cx));
            let mut rest = 0;
            for _ in 0..400 {
                frame_144(cx, window, &view);
                let moving = view.read_with(cx, |v, _| v.panes.flight.is_some() || v.panel_fx.moving());
                rest = if moving { 0 } else { rest + 1 };
                if rest >= 5 {
                    break;
                }
            }
            let xs: Vec<f32> = divider_path(&seen, at).into_iter().map(|(_, x)| x).collect();
            assert!(xs.len() > 10 && xs[0] != xs[xs.len() - 1], "{what}, {motion}: the divider moved: {xs:?}");
            let dir = (xs[xs.len() - 1] - xs[0]).signum();
            for (i, d) in xs.windows(2).enumerate() {
                if (d[1] - d[0]) * dir < -0.01 {
                    fails.push(format!("{what}, {motion}, draw {}: the divider went back, {} -> {}", i + 1, d[0], d[1]));
                }
            }
        }
        // What the hook read is what was drawn: the rule's place, within
        // the device pixel GPUI snaps it to.
        let last = divider_path(&seen, 0).last().map(|p| p.1).expect("a divider drawn");
        let rule = drawn(cx, window, "pane-divider Row".into()).map(|b| b[0]).expect("the rule drawn");
        assert!((rule - last).abs() <= 1. / s + 0.01, "{what}: the rule at {rule}, the hook read {last}");
        view.update(cx, |v, _| v.on_draw = None);
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// A layout flight taken over mid-flight turns with its velocity (the
/// panels' rule, `PanelFx::fly`: fluid.md §2.1). Ctrl+\, and 10 frames
/// into its flight Ctrl+W on the new pane: at 2560 (nothing yields: the
/// split on its own spring) and at 1920 (the right panel yields to it and
/// the split rides it; the close turns it back). The divider was moving
/// left at speed; Ctrl+W comes between two frames, as a hand's does, and
/// the close's own frame still moves it left at least half as fast (from
/// rest it turned at once: the close started a spring of its own at 0;
/// and the film's close (1920, the right panel folded: nothing yields)
/// drew the layout as the frame before, 0 px, a 14 ms hold: the carried
/// spring started at the close's frame, not at the frame that drew where
/// it starts), then turns once and goes right to the end, one pane left.
/// Every draw is read (#61's hook). Off: the close lands in its frame, as
/// before.
#[gpui_kit::test]
fn a_close_mid_split_turns_with_the_splits_velocity(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_at(cx, "panes-carry", (2560., 1080.));
    view.update(cx, |v, cx| {
        let mut p = v.panels;
        (p.rail_width, p.right_width) = (Some(crate::theme::RAIL_WIDTHS.1), Some(crate::theme::RIGHT_WIDTHS.1));
        v.panels = p;
        cx.notify();
    });
    level(cx, crate::motion::Level::Full);
    let mut fails = Vec::new();
    for (what, w, yields) in [("its own spring", 2560., false), ("riding the right panel", 1920., true)] {
        VisualTestContext::from_window(window, cx).simulate_resize(size(px(w), px(1080.)));
        for _ in 0..3 {
            frame(cx, window);
        }
        // Every draw (#61's hook: Shady #62 r1, a step can draw twice).
        let seen = draws(cx, &view, divider_drawn);
        key(cx, window, "ctrl-\\");
        for _ in 0..10 {
            frame_144(cx, window, &view);
        }
        let (flying, yielded) = view.read_with(cx, |v, _| (v.panes.flight.is_some(), v.pane_yield.is_some_and(|y| y.1)));
        assert!(flying, "{what}: the split still in the air");
        assert_eq!(yielded, yields, "{what}: the right panel's yield");
        let k = divider_path(&seen, 0).len();
        // Ctrl+W between two frames, as a hand's comes: its frame is the
        // next one (a frame on), the first drawn after it.
        let at = view.read_with(cx, |v, _| v.fx.clock());
        view.read_with(cx, |v, _| v.fx.set_clock(at + Duration::from_micros(6944)));
        key(cx, window, "ctrl-w");
        let mut frames = 0;
        while view.read_with(cx, |v, _| v.panes.flight.is_some() || v.panel_fx.moving()) {
            frame_144(cx, window, &view);
            frames += 1;
            assert!(frames < 300, "{what}: the close never landed");
        }
        view.update(cx, |v, _| v.on_draw = None);
        let xs: Vec<f32> = divider_path(&seen, 0).into_iter().map(|(_, x)| x).collect();
        eprintln!("CARRY {what}: {k} frames of the split, then the close: {xs:?}");
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "{what}: one pane left");
        let steps: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        let (before, after) = (steps[k - 2], steps[k - 1]);
        assert!(before < -4., "{what}: the split's divider moving left at speed: {before} px a frame");
        if !(after < 0. && after <= before * 0.5) {
            fails.push(format!("{what}: the close's own frame moved {after} px after {before} (at least half as fast, still left)"));
        }
        // One turn: left while it carries on, then right to the end.
        match steps[k - 1..].iter().position(|&s| s > 0.) {
            Some(turn) if steps[k - 1 + turn..].iter().all(|&s| s >= 0.) => {}
            _ => fails.push(format!("{what}: not one turn, then right to the end: {:?}", &steps[k - 1..])),
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
    // Off: a split and its close land each in its frame.
    level(cx, crate::motion::Level::Off);
    key(cx, window, "ctrl-\\");
    frame(cx, window);
    key(cx, window, "ctrl-w");
    frame(cx, window);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.flight.is_some())), (1, false), "Off: landed in its frame");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── #62 round 3 (Shady #62 r1: M1, M3, M4, draw by draw) ────────────────

/// One frame `dt` after the last on the test's clock, drawn: [`frame_144`]
/// at any rate (60 Hz; a late frame).
fn frame_dt(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>, dt: Duration) {
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

/// The panels at their widest (480 / 520), the layout's moves at Full.
fn wide_panels(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        let mut p = v.panels;
        (p.rail_width, p.right_width) = (Some(crate::theme::RAIL_WIDTHS.1), Some(crate::theme::RIGHT_WIDTHS.1));
        v.panels = p;
        cx.notify();
    });
    frame(cx, window);
    level(cx, crate::motion::Level::Full);
}

/// Shady #62 M1: a close 1, 2 or 3 frames into a split the right panel
/// yields to (Ctrl+\ at 1920; the layout riding it, #51). The panel
/// turns back with its velocity a few px past where it was, and the
/// layout rides it on: draw by draw, the divider moves while the panel
/// does (the ride's progress held at −1 kept it still: the panel 12, then
/// 22 px on), and it turns once. A new pane on the left, and an overshoot
/// many times the way back: `split::tests::a_ride_past_its_start_keeps_every_pane_in_its_room`.
#[gpui_kit::test]
fn an_early_close_mid_ride_moves_with_its_panel(cx: &mut TestAppContext) {
    let mut fails = Vec::new();
    for k in 1..=3 {
        let what = format!("a split closed {k} frame(s) in");
        let (window, view, client, _a, _attaches, _events) = on_a_at(cx, &format!("early-{k}"), (1920., 1080.));
        wide_panels(cx, window, &view);
        let seen = draws(cx, &view, |v| {
            let (t, x) = divider_drawn(v);
            (t, x, v.panel_fx.sides[1].shown)
        });
        key(cx, window, "ctrl-\\");
        for _ in 0..k {
            frame_144(cx, window, &view);
        }
        let rides = view.read_with(cx, |v, _| v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some()));
        assert!(rides, "{what}: the layout rides the right panel");
        let at = view.read_with(cx, |v, _| v.fx.clock());
        view.read_with(cx, |v, _| v.fx.set_clock(at + Duration::from_micros(6944)));
        key(cx, window, "ctrl-w");
        let mut frames = 0;
        while view.read_with(cx, |v, _| v.panes.flight.is_some() || v.panel_fx.moving()) {
            frame_144(cx, window, &view);
            frames += 1;
            assert!(frames < 300, "{what}: the close never landed");
        }
        view.update(cx, |v, _| v.on_draw = None);
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "{what}: one pane left");
        // Frame by frame (one per instant): the divider, the panel.
        let mut path: Vec<(Instant, f32, f32)> = Vec::new();
        for &(t, x, p) in seen.borrow().iter() {
            let Some(x) = x else { continue };
            match path.last_mut() {
                Some(l) if l.0 == t => (l.1, l.2) = (x, p),
                _ => path.push((t, x, p)),
            }
        }
        // The ride moves the divider in proportion to the panel (its
        // ratio over the split's frames): held while the panel went
        // far enough to move it 1.5 px or more, it stood still.
        let k1 = (k + 1).min(path.len() - 1);
        let ratio = ((path[k1].1 - path[0].1) / (path[k1].2 - path[0].2)).abs();
        let mut i = 0;
        while i + 1 < path.len() {
            let mut j = i;
            while j + 1 < path.len() && path[j + 1].1 == path[i].1 {
                j += 1;
            }
            let swept = path[i..=j].windows(2).map(|w| (w[1].2 - w[0].2).abs()).sum::<f32>();
            if j > i && swept * ratio >= 1.5 {
                fails.push(format!(
                    "{what}: the divider held at {} for {} frames while the panel moved {swept} px (ratio {ratio:.2})",
                    path[i].1,
                    j - i
                ));
                break;
            }
            i = j + 1;
        }
        let steps: Vec<f32> = path.windows(2).map(|w| w[1].1 - w[0].1).filter(|d| *d != 0.).collect();
        let turns = steps.windows(2).filter(|w| w[0].signum() != w[1].signum()).count();
        if turns != 1 {
            fails.push(format!("{what}: the divider turned {turns} times: {steps:?}"));
        }
        eprintln!("EARLY {what}: {:?}", path.iter().map(|p| (p.1, p.2)).collect::<Vec<_>>());
        view.update(cx, |v, _| v.client = None);
        let _ = client.shutdown();
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Shady #62 M3: a flight taken over starts from one frame, the last one
/// drawn: its position, its velocity and its instant. Ctrl+\ at 2560
/// (nothing yields: the split on its own spring), 10 frames at 144 Hz, 5
/// at 60 Hz, or 10 at 144 Hz and then 33 ms of nothing; then Ctrl+W. The close's
/// spring starts at that frame's instant with the split's velocity there,
/// carried (`Flight::carried`), not with its velocity a frame (or a wait)
/// later, planted a frame back.
#[gpui_kit::test]
fn a_takeover_starts_from_its_last_frames_position_velocity_and_instant(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_at(cx, "panes-m3", (2560., 1080.));
    wide_panels(cx, window, &view);
    let mut fails = Vec::new();
    for (what, dt, n, wait) in [("144 Hz", 6944, 10, 0), ("60 Hz", 16_667, 5, 0), ("144 Hz, then 33 ms", 6944, 10, 33_000)] {
        let dt = Duration::from_micros(dt);
        key(cx, window, "ctrl-\\");
        for _ in 0..n {
            frame_dt(cx, window, &view, dt);
        }
        let (drawn_at, v_old, old) = view.read_with(cx, |v, _| {
            let f = v.panes.flight.as_ref().expect("the split in the air");
            assert!(f.ride.is_none(), "{what}: on its own spring");
            let at = v.panes.drawn_at.expect("drawn");
            let v_old = v.fx.spring_velocity_at(crate::app::PANE_FLIGHT, at).expect("moving");
            (at, v_old, (f.from.clone(), f.to.clone(), f.travel))
        });
        let t = view.read_with(cx, |v, _| v.fx.clock());
        view.read_with(cx, |v, _| v.fx.set_clock(t + Duration::from_micros(wait) + dt / 2));
        key(cx, window, "ctrl-w");
        frame_dt(cx, window, &view, dt);
        let (v_new, want) = view.read_with(cx, |v, _| {
            let f = v.panes.flight.as_ref().expect("the close in the air");
            let (o, (w, h)) = (v.centre_at.get(), v.centre.get());
            let area = crate::split::Rect { x: o.0, y: o.1, w, h };
            (v.fx.spring_velocity_at(crate::app::PANE_FLIGHT, drawn_at), f.carried(&old, v_old, area))
        });
        eprintln!("M3 {what}: the split at {v_old} px/s; the close starts at {v_new:?}, carried {want}");
        match v_new {
            Some(v) if want.abs() > 1. && (v - want).abs() <= 1e-3 * want.abs() => {}
            _ => fails.push(format!("{what}: the close's spring at the last frame's instant {v_new:?}, carried from it {want}")),
        }
        let mut frames = 0;
        while view.read_with(cx, |v, _| v.panes.flight.is_some()) {
            frame_144(cx, window, &view);
            frames += 1;
            assert!(frames < 300, "{what}: never landed");
        }
        assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 1, "{what}: one pane again");
    }
    assert!(fails.is_empty(), "{fails:#?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #62 M4: a takeover mid-ride that changes no yield rides on.
/// Ctrl+\ at 1920 (the right panel yields to it, the layout rides the
/// panel), 10 frames in Ctrl+Shift+\ in the new pane: no yield changes,
/// and the split down rides the same flight on from where it was drawn
/// (it flew on its own spring from rest: the divider stopped while the
/// panel went on). Draw by draw (#61's hook) the vertical divider stays on
/// the ride's line, its x by the centre's width as drawn, to the end:
/// position and velocity go on as they were.
#[gpui_kit::test]
fn a_split_down_mid_ride_rides_on(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_at(cx, "panes-m4", (1920., 1080.));
    wide_panels(cx, window, &view);
    // Each draw: its instant, the vertical divider's x in the window and
    // the centre's width (where the panes end).
    let seen = draws(cx, &view, |v| {
        let (t, x) = divider_drawn(v);
        let w = v.rects_drawn.borrow().1.iter().map(|(_, r)| r.x + r.w).fold(0., f32::max);
        (t, x, w)
    });
    type Draws = Rc<RefCell<Vec<(Instant, Option<f32>, f32)>>>;
    let path = |seen: &Draws| {
        let mut out: Vec<(Instant, f32, f32)> = Vec::new();
        for &(t, x, w) in seen.borrow().iter() {
            let Some(x) = x else { continue };
            match out.last_mut() {
                Some(l) if l.0 == t => (l.1, l.2) = (x, w),
                _ => out.push((t, x, w)),
            }
        }
        out
    };
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame_144(cx, window, &view);
    }
    let riding = view.read_with(cx, |v, _| v.panes.flight.as_ref().is_some_and(|f| f.ride.is_some()));
    assert!(riding, "the split rides the right panel");
    let before = path(&seen);
    let k = before.len();
    assert!(k >= 5, "the split drawn: {before:?}");
    let t = view.read_with(cx, |v, _| v.fx.clock());
    view.read_with(cx, |v, _| v.fx.set_clock(t + Duration::from_micros(3472)));
    key(cx, window, "ctrl-shift-\\");
    frame_144(cx, window, &view);
    let (rides, yields) = view.read_with(cx, |v, _| {
        let f = v.panes.flight.as_ref().expect("the split down in the air");
        (f.ride.is_some(), v.pane_yield.map(|y| (y.0, y.1)))
    });
    assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 3, "three panes");
    assert_eq!(yields, Some((false, true)), "the right panel still yielded, the rail not");
    assert!(rides, "the split down rides on: no yield changed and the panel still flies");
    let mut frames = 0;
    while view.read_with(cx, |v, _| v.panes.flight.is_some() || v.panel_fx.moving()) {
        frame_144(cx, window, &view);
        frames += 1;
        assert!(frames < 300, "never landed");
    }
    view.update(cx, |v, _| v.on_draw = None);
    let all = path(&seen);
    eprintln!("M4 (x, w): {:?}", all.iter().map(|p| (p.1, p.2)).collect::<Vec<_>>());
    // The ride's slope (x by the centre's width), from its first draw
    // that moved to its last before the takeover.
    let first = before.iter().find(|p| (p.2 - before[0].2).abs() >= 1.).expect("the centre widening");
    let last = before[k - 1];
    assert!(last.2 - first.2 >= 20., "the ride well under way: {first:?} .. {last:?}");
    let slope = (last.1 - first.1) / (last.2 - first.2);
    assert!(slope < -0.2, "the vertical divider going left as the centre widens: {slope}");
    // Where it lands: the tree's, in the centre as drawn last.
    let end = *all.last().expect("drawn");
    let tree_x = view.read_with(cx, |v, _| {
        let (o, (w, h)) = (v.centre_at.get(), v.centre.get());
        let r = v.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w, h })[0].1;
        o.0 + r.x + r.w
    });
    assert!((end.1 - tree_x).abs() <= 1., "it lands at the tree's: {} vs {tree_x}", end.1);
    assert!(end.2 - last.2 >= 20., "the panel still had a way to go: {last:?} .. {end:?}");
    // From the takeover to the end, on one line (x by the width) ...
    let after = (end.1 - last.1) / (end.2 - last.2);
    let mut fails = Vec::new();
    for p in &all[k..] {
        let want = last.1 + after * (p.2 - last.2);
        if (p.1 - want).abs() > 1.5 {
            fails.push(format!("x {} at w {} (the line: {want:.1})", p.1, p.2));
        }
    }
    assert!(fails.is_empty(), "after the takeover the divider leaves its line: {fails:?}");
    // ... the ride's: at the speed the centre moves, as before.
    assert!((after - slope).abs() <= 0.02, "the split down's slope {after} vs the ride's {slope}");
    assert!(all[k].1 <= last.1, "the split down's own frame still goes left: {} after {}", all[k].1, last.1);
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── #62 round 4 (Shady #62 r3: R3-1, R3-3) ──────────────────────────────

/// Shady #62 r3 R3-1: a panels' flight from a fractional origin rewraps
/// each pane once. At 1280 px, 125 %, rail 247 and right 240, two equal
/// panes: the right panel yields to them. The rail folds and the right
/// panel comes back on its tie: the centre only widens (1000.8 at 279.2
/// to 1008 at 32), so each column keeps its wrap from the frame before
/// until the panels land, and rewraps there, once. The frozen centre was
/// laid out at the landing's origin, where the same width is a fraction
/// wider a pane (the panes are rounded on the window's pixels): pane 0
/// went 467 → 468 at the start and → 472 as they landed. Then the rail
/// unfolds (the centre narrows): one rewrap, in the flight's first frame.
/// Every draw (#61's hook), from the frame before each flight.
#[gpui_kit::test]
fn a_panels_flight_from_a_fractional_origin_rewraps_each_pane_once(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_at(cx, "wrap-origin", (1280., 1080.));
    VisualTestContext::from_window(window, cx).simulate_scale_factor_change(1.25);
    view.update(cx, |v, cx| {
        let mut p = v.panels;
        (p.rail_width, p.right_width) = (Some(247), Some(240));
        v.panels = p;
        cx.notify();
    });
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame(cx, window);
    }
    let order = view.read_with(cx, |v, _| v.reading_order());
    assert_eq!(order.len(), 2, "two panes");
    assert_eq!(view.read_with(cx, |v, _| v.pane_yield.map(|y| y.1)), Some(true), "the right panel yielded to them");
    level(cx, crate::motion::Level::Full);
    let ids = order.clone();
    let seen = draws(cx, &view, move |v: &AppView| {
        let cols: Vec<f32> = ids.iter().map(|id| v.panes.get(*id).map_or(f32::NAN, |p| p.layout.column)).collect();
        (v.panel_fx.moving(), cols)
    });
    let mut fails = Vec::new();
    for (motion, widens) in [("fold", true), ("unfold", false)] {
        // The frame before the flight, drawn.
        frame_144(cx, window, &view);
        let at = seen.borrow().len() - 1;
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        let mut rest = 0;
        for _ in 0..400 {
            frame_144(cx, window, &view);
            let moving = view.read_with(cx, |v, _| v.panes.flight.is_some() || v.panel_fx.moving());
            rest = if moving { 0 } else { rest + 1 };
            if rest >= 5 {
                break;
            }
        }
        let got: Vec<(bool, Vec<f32>)> = seen.borrow()[at..].to_vec();
        assert!(got.iter().any(|d| d.0), "{motion}: the panels flew");
        for (i, id) in order.iter().enumerate() {
            let path: Vec<(bool, f32)> = got.iter().map(|(moving, cols)| (*moving, cols[i])).collect();
            let (before, landed) = (path[0].1, path[path.len() - 1].1);
            eprintln!("WRAP {motion}, pane {i} ({id:?}): {path:?}");
            let mut columns: Vec<f32> = path.iter().map(|p| p.1).collect();
            columns.dedup();
            if columns.len() > 2 {
                fails.push(format!("{motion}: pane {i}'s column rewrapped more than once: {columns:?}"));
            }
            if landed != before && (landed > before) != widens {
                fails.push(format!("{motion}: pane {i}'s column went {before} -> {landed}"));
            }
            // Widening, the column from the frame before while they fly;
            // narrowing, the landed one from the flight's first frame.
            let held = if widens { before } else { landed };
            if let Some(&(_, c)) = path.iter().find(|&&(moving, c)| moving && c != held) {
                fails.push(format!("{motion}: pane {i}'s column {c} in the flight (the frame before {before}, landed {landed})"));
            }
        }
    }
    view.update(cx, |v, _| v.on_draw = None);
    assert!(fails.is_empty(), "{fails:#?}");
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

/// Shady #62 r3 R3-3: the drop targets are the panes as drawn, on the
/// window's pixels. At rest at 1280 px, 125 %, rail 221 (drawn 220.8) and
/// the right panel yielded, two equal panes: each pane's drop body has its
/// drawn box's left and width (rounded from the centre's own edge, the
/// first pane's ended at 733.8 in the window, drawn at 734).
#[gpui_kit::test]
fn the_drop_targets_are_the_panes_as_drawn_at_a_fractional_origin(cx: &mut TestAppContext) {
    let (window, view, client, _a, _attaches, _events) = on_a_at(cx, "drop-origin", (1280., 1080.));
    VisualTestContext::from_window(window, cx).simulate_scale_factor_change(1.25);
    view.update(cx, |v, cx| {
        let mut p = v.panels;
        (p.rail_width, p.right_width) = (Some(221), Some(240));
        v.panels = p;
        cx.notify();
    });
    frame(cx, window);
    key(cx, window, "ctrl-\\");
    for _ in 0..10 {
        frame(cx, window);
    }
    let (bodies, (origin, rects)) = view.read_with(cx, |v, _| (v.drop_bodies(), v.rects_drawn.borrow().clone()));
    assert_eq!(rects.len(), 2, "two panes drawn");
    assert!(origin.0.fract().abs() > 0.01, "the centre at a fractional left: {origin:?}");
    for (id, r) in &rects {
        let b = bodies.iter().find(|(i, _)| i == id).map(|(_, b)| *b).expect("a drop body for each pane");
        assert!(
            (b.x - r.x).abs() < 0.01 && (b.w - r.w).abs() < 0.01,
            "pane {id:?}: its drop body {b:?}, drawn {r:?} (the centre at {origin:?})"
        );
    }
    view.update(cx, |v, _| v.client = None);
    let _ = client.shutdown();
}

// ── #62 rounds 5 and 6 (Shady #62 r4 R4-1; r5 R5-2, R5-3) ───────────────

/// An action in a row of [`every_flight_rewraps_each_pane_at_most_once`].
#[derive(Clone, Copy, Debug)]
enum Step {
    /// Ctrl-\ on the focused pane.
    Split,
    Equalise,
    /// The rightmost pane closes.
    CloseLast,
    /// The rail folds or unfolds.
    Rail,
}

fn take_step(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<AppView>, step: Step) {
    match step {
        Step::Split => key(cx, window, "ctrl-\\"),
        Step::Equalise => {
            cx.update_window(window, |_, w, cx| view.update(cx, |v, cx| v.app_act(crate::palette::AppAct::Equalise, w, cx)))
                .expect("the window");
        }
        Step::CloseLast => view.update(cx, |v, cx| {
            let last = *v.reading_order().last().expect("a pane");
            v.in_pane(last, |v| v.close_pane());
            cx.notify();
        }),
        Step::Rail => view.update(cx, |v, cx| v.toggle_panel(true, cx)),
    }
}

/// What a row lands on (Shady #62 r5 R5-2: its landing is checked, never
/// its last frame taken for one): its panes, an equalised tree, the
/// rail folded or open (`None`: either).
#[derive(Clone, Copy, Debug)]
struct Lands {
    panes: usize,
    equal: bool,
    rail_folded: Option<bool>,
}

/// One draw (#61's hook, every frame drawn): the layout's flight, by its
/// target (`None`: none), whether a panel flies, each pane's column.
#[derive(Clone, Debug)]
struct Draw<P> {
    flight: Option<String>,
    panels: bool,
    cols: Vec<(P, f32)>,
}

impl<P> Draw<P> {
    fn moving(&self) -> bool {
        self.flight.is_some() || self.panels
    }
}

/// The 144 Hz frames a row's last flight has to land in, and the resting
/// frames after it that count as landed.
const CAP: usize = 400;
const RESTING: usize = 5;

/// A row's verdict on its draws (Shady #62 r5 R5-2, R5-3): what fails,
/// each named with `row`. `starts[i]` is the last draw before phase `i`'s
/// action, with the columns it comes in with; the phase runs to the next
/// one's or to the row's end, on `rest` resting frames.
/// - The draw before the first action is at rest, and each phase flies
///   after its action (not before it: a flight that never landed is the
///   last row's).
/// - A phase after the first takes over: the flight it comes in on is
///   under way, and its action starts another (a target of its own; an
///   action that changes nothing fails).
/// - In each phase, each pane in it from its first draw to its last
///   rewraps at most once.
/// - The row lands: [`RESTING`] frames at rest within [`CAP`].
fn judge<P: PartialEq + std::fmt::Debug>(row: &str, draws: &[Draw<P>], starts: &[usize], rest: usize) -> Vec<String> {
    let mut fails = Vec::new();
    let Some(&first) = starts.first() else { return vec![format!("{row}: no action")] };
    if draws[first].moving() {
        fails.push(format!("{row}: the frame before its action was still moving: {:?}", draws[first]));
    }
    for (n, &s) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(draws.len() - 1);
        let after = &draws[s + 1..=end];
        if !after.iter().any(Draw::moving) {
            fails.push(format!("{row}, phase {n}: nothing flew after its action"));
        }
        if n > 0 {
            let was = &draws[s].flight;
            if was.is_none() {
                fails.push(format!("{row}, phase {n}: no flight under way to take over"));
            }
            if !after.iter().any(|d| d.flight.is_some() && d.flight != *was) {
                fails.push(format!("{row}, phase {n}: its action started no flight of its own"));
            }
        }
        let phase = &draws[s..=end];
        let (incoming, out) = (&phase[0].cols, &phase[phase.len() - 1].cols);
        for (id, _) in incoming.iter().filter(|(id, _)| out.iter().any(|(o, _)| o == id)) {
            let mut columns: Vec<f32> = phase.iter().filter_map(|d| d.cols.iter().find(|(i, _)| i == id).map(|c| c.1)).collect();
            columns.dedup();
            if columns.len() > 2 {
                fails.push(format!("{row}, phase {n}: pane {id:?}'s column rewrapped {} times: {columns:?}", columns.len() - 1));
            }
        }
    }
    let last = &draws[draws.len() - 1];
    if rest < RESTING || last.moving() {
        fails.push(format!(
            "{row}: never landed: {rest} resting frames of {RESTING} within {CAP}; the last draw's flight {:?}, panels moving {}, columns {:?}",
            last.flight, last.panels, last.cols
        ));
    }
    fails
}

/// [`judge`] on made-up draws (Shady #62 r5: each loophole, one row): a
/// flight frozen mid-way with its column still, a takeover rewrapping
/// twice in one phase (0 + 2, 2 + 0) though once in each flight on
/// average, an action that changes nothing taken for a takeover, a row
/// that starts mid-flight. Each fails; a row that rewraps once a phase
/// and lands holds.
#[test]
fn the_tables_verdict_fails_each_loophole() {
    let d = |flight: Option<&str>, panels: bool, c: f32| Draw { flight: flight.map(str::to_owned), panels, cols: vec![(0_u8, c)] };
    let rest = |c: f32| d(None, false, c);
    let fly = |t: &str, c: f32| d(Some(t), false, c);
    let frozen: Vec<Draw<u8>> = std::iter::once(rest(672.)).chain(std::iter::repeat_n(fly("a", 445.), CAP)).collect();
    // Each: its draws, its phases' starts, its resting frames, and the
    // failures it must give, in order (none: it holds).
    type Case = (&'static str, Vec<Draw<u8>>, Vec<usize>, usize, Vec<&'static str>);
    let cases: Vec<Case> = vec![
        ("one rewrap, landed", vec![rest(672.), fly("a", 445.), fly("a", 445.), rest(445.)], vec![0], RESTING, vec![]),
        (
            "a takeover, one rewrap a phase",
            vec![rest(672.), fly("a", 445.), fly("a", 445.), fly("b", 445.), fly("b", 589.), rest(589.)],
            vec![0, 2],
            RESTING,
            vec![],
        ),
        ("a panel's flight", vec![rest(672.), d(None, true, 520.), rest(520.)], vec![0], RESTING, vec![]),
        ("frozen mid-flight, its column still", frozen, vec![0], 0, vec!["never landed"]),
        (
            "at rest, short of five frames",
            vec![rest(672.), fly("a", 445.), fly("a", 445.), rest(445.)],
            vec![0],
            RESTING - 1,
            vec!["never landed"],
        ),
        (
            "a takeover, 2 + 0",
            vec![rest(672.), fly("a", 600.), fly("a", 445.), fly("b", 445.), rest(445.)],
            vec![0, 2],
            RESTING,
            vec!["phase 0: pane 0's column rewrapped 2 times"],
        ),
        (
            "a takeover, 0 + 2",
            vec![rest(672.), fly("a", 672.), fly("a", 672.), fly("b", 600.), fly("b", 589.), rest(589.)],
            vec![0, 2],
            RESTING,
            vec!["phase 1: pane 0's column rewrapped 2 times"],
        ),
        (
            "a second action that changes nothing",
            vec![rest(672.), fly("a", 445.), fly("a", 445.), fly("a", 445.), rest(589.)],
            vec![0, 2],
            RESTING,
            vec!["started no flight of its own"],
        ),
        (
            "a takeover of a flight that has landed",
            vec![rest(672.), fly("a", 445.), rest(445.), fly("b", 589.), rest(589.)],
            vec![0, 2],
            RESTING,
            vec!["no flight under way to take over"],
        ),
        (
            "a row that starts mid-flight, its own action doing nothing",
            vec![fly("a", 445.), rest(445.), rest(445.)],
            vec![0],
            RESTING,
            vec!["still moving", "nothing flew after its action"],
        ),
    ];
    let mut wrong = Vec::new();
    for (name, draws, starts, rested, want) in &cases {
        let got = judge(name, draws, starts, *rested);
        if got.len() != want.len() || !got.iter().zip(want).all(|(g, w)| g.contains(w)) {
            wrong.push(format!("{name}: wanted {want:?}, got {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Shady #62 r4 R4-1: every flight, the panels', the layout's or both at
/// once, rewraps each pane's column at most once. Each flight lays its
/// starting tree out in its own starting centre, at that centre's left
/// edge: the panes are rounded on the window's pixels, so the same width
/// at another edge is another layout. A layout riding a yielding panel's
/// return read its start at the landing's edge: at 1200 px, 125 %, rail
/// 247 and right 240, three equal panes (both panels yielded to strips),
/// the rightmost closing brings the rail back, and pane 0, 378 wide, was
/// read 377.8: its column went 346 → 345 → 427. A table, at 100, 125 and
/// 150 %: in a window 2400 px wide, a split to two and to three, an
/// equalise, a close to two, and a split to three taken over six frames
/// in by an equalise (two phases), which leaves three equal panes; then
/// at 1200 and at 1500 px (a resize: a cut), a close to two (at 1200 px,
/// 125 %: the rail's return), the rail's fold and unfold, a split (down,
/// where a column has no room: no column moves) and a close. Every draw
/// (#61's hook), each row from the draw before its action, judged by
/// [`judge`] (Shady #62 r5 R5-2, R5-3): it starts at rest, flies after
/// each action, rewraps each pane at most once a phase and lands, on the
/// panes, the equalised tree or the rail it was for. The panes judged
/// are those in a phase from its first draw to its last: a split's new
/// pane and a closing one are not. The takeover is two layout flights on
/// their own springs (2400 px: nothing yields); a takeover of a ride
/// (`Ride.from_x` carried) is not a row.
#[gpui_kit::test]
fn every_flight_rewraps_each_pane_at_most_once(cx: &mut TestAppContext) {
    use Step::*;
    let lands = |panes: usize| Lands { panes, equal: false, rail_folded: None };
    let equal = Lands { equal: true, ..lands(3) };
    let wide: [(&[Step], &str, Lands); 5] = [
        (&[Split], "split to 2", lands(2)),
        (&[Split], "split to 3", lands(3)),
        (&[Equalise], "equalise", equal),
        (&[CloseLast], "close to 2", lands(2)),
        (&[Split, Equalise], "split to 3 taken over by an equalise", equal),
    ];
    let narrow: [(&[Step], &str, Lands); 5] = [
        (&[CloseLast], "close to 2", lands(2)),
        (&[Rail], "fold", Lands { rail_folded: Some(true), ..lands(2) }),
        (&[Rail], "unfold", Lands { rail_folded: Some(false), ..lands(2) }),
        (&[Split], "split to 3", lands(3)),
        (&[CloseLast], "close to 2 again", lands(2)),
    ];
    let mut fails = Vec::new();
    for (n, (w, s)) in [(1200., 1.25), (1200., 1.), (1200., 1.5), (1500., 1.), (1500., 1.25), (1500., 1.5)].into_iter().enumerate() {
        let (window, view, client, _a, _attaches, _events) = on_a_at(cx, &format!("wraps-{n}"), (2400., 1080.));
        VisualTestContext::from_window(window, cx).simulate_scale_factor_change(s);
        view.update(cx, |v, cx| {
            let mut p = v.panels;
            (p.rail_width, p.right_width) = (Some(247), Some(240));
            v.panels = p;
            cx.notify();
        });
        frame(cx, window);
        level(cx, crate::motion::Level::Full);
        let seen = draws(cx, &view, |v: &AppView| Draw {
            flight: v.panes.flight.as_ref().map(|f| format!("{:?}", f.to)),
            panels: v.panel_fx.moving(),
            cols: v.reading_order().iter().filter_map(|id| v.panes.get(*id).map(|p| (*id, p.layout.column))).collect(),
        });
        for (k, (row_steps, name, to)) in wide.into_iter().chain(narrow).enumerate() {
            if k == wide.len() {
                assert_eq!(view.read_with(cx, |v, _| v.panes.len()), 3, "{w} px, {s}: three panes");
                VisualTestContext::from_window(window, cx).simulate_resize(size(px(w), px(1080.)));
                for _ in 0..10 {
                    frame(cx, window);
                }
            }
            let row = format!("{} px, {}%, {name}", if k < wide.len() { 2400. } else { w }, s * 100.);
            let rail_was = view.read_with(cx, |v, _| v.pane_yield.is_some_and(|y| y.0));
            // Each action after the draw before it; a takeover six frames
            // into the flight before (about 42 ms: well inside it).
            let mut starts = Vec::new();
            for (i, &step) in row_steps.iter().enumerate() {
                for _ in 0..if i == 0 { 1 } else { 6 } {
                    frame_144(cx, window, &view);
                }
                starts.push(seen.borrow().len() - 1);
                take_step(cx, window, &view, step);
            }
            let mut rest = 0;
            for _ in 0..CAP {
                frame_144(cx, window, &view);
                let moving = view.read_with(cx, |v, _| v.panes.flight.is_some() || v.panel_fx.moving());
                rest = if moving { 0 } else { rest + 1 };
                if rest >= RESTING {
                    break;
                }
            }
            let at = starts[0];
            let starts: Vec<usize> = starts.iter().map(|s| s - at).collect();
            let got: Vec<Draw<crate::pane::PaneId>> = seen.borrow()[at..].to_vec();
            let row_fails = judge(&row, &got, &starts, rest);
            let (rail_now, panes, equalised, rail_folded) = view.read_with(cx, |v, _| {
                let (w, h) = v.centre.get();
                let mut even = v.panes.tree.clone();
                even.equalise_all(crate::split::Rect { x: 0., y: 0., w, h });
                (v.pane_yield.is_some_and(|y| y.0), v.panes.len(), even == v.panes.tree, v.panels.rail_collapsed)
            });
            eprintln!("WRAPS {row}: {} draws, phases from {starts:?}, the rail yielded {rail_was} -> {rail_now}", got.len());
            for (id, _) in &got[0].cols {
                let mut columns: Vec<f32> = got.iter().filter_map(|d| d.cols.iter().find(|(i, _)| i == id).map(|c| c.1)).collect();
                columns.dedup();
                eprintln!("WRAPS {row}, pane {id:?}: {columns:?}");
            }
            fails.extend(row_fails);
            if panes != to.panes {
                fails.push(format!("{row}: landed on {panes} panes, not {}", to.panes));
            }
            if to.equal && !equalised {
                fails.push(format!("{row}: landed on a tree that is not equalised"));
            }
            if to.rail_folded.is_some_and(|f| f != rail_folded) {
                fails.push(format!("{row}: the rail folded {rail_folded}, not {:?}", to.rail_folded));
            }
            if n == 0 && k == wide.len() && !(rail_was && !rail_now) {
                fails.push(format!("{row}: the rail comes back from its strip (yielded {rail_was} -> {rail_now})"));
            }
        }
        view.update(cx, |v, _| {
            v.on_draw = None;
            v.client = None;
        });
        let _ = client.shutdown();
    }
    assert!(fails.is_empty(), "{fails:#?}");
}
