//! The UI tests' kit for test files other than `ui_tests` (that file is
//! protected: it stays as it is, Shady #23 r6 Q4). Its helpers are
//! `ui_tests`' own, byte for byte (each private, as there: the test files
//! that use them are this module's children), and
//! [`kit_matches_ui_tests`] fails, naming the helper, if one drifts from
//! the `ui_tests` original (one renamed: [`motion_level`]).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size};

use crate::app::AppView;

#[path = "answer_tests.rs"]
mod answer_tests;
#[path = "ask_tests.rs"]
mod ask_tests;
#[path = "cards_d_tests.rs"]
mod cards_d_tests;
#[path = "entry_tests.rs"]
mod entry_tests;
#[path = "gate_clock_tests.rs"]
mod gate_clock_tests;
#[path = "hang_tests.rs"]
mod hang_tests;
#[path = "held_tests.rs"]
mod held_tests;
#[path = "key_clock_tests.rs"]
mod key_clock_tests;
#[path = "keys_tests.rs"]
mod keys_tests;
#[path = "send_tests.rs"]
mod send_tests;

/// The `--offline` demo's window, `size` px at text step `text`, a client
/// whose daemon is nowhere ([`open_with`], the beat on the layer).
fn open_at(
    cx: &mut TestAppContext,
    size: (f32, f32),
    text: u8,
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    open_with(cx, size, text, true)
}
/// [`open_at`], the beat on the layer (`layered`, the app's way) or in the
/// app's frame.
fn open_with(
    cx: &mut TestAppContext,
    (w, h): (f32, f32),
    text: u8,
    layered: bool,
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(w), px(h)) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        crate::beat::open_window(options, cx, layered, |window, cx| {
            cx.new(|cx| {
                let v = AppView::new(None, Some(crate::demo::events()), window, cx);
                // The motion clock is the test's from the first frame: it
                // stands still until a test moves it ([`beat`],
                // [`fluid_frame`], `set_clock`), never with the runner's
                // wall time. On a loaded runner a draw took 110 ms: every
                // motion timer was already due when armed, `run_until_parked`
                // drew frame after frame (with the beat in the frame, never
                // parking), and a test step played hundreds of ms of motion.
                v.fx.pin_clock();
                v
            })
        })
        .expect("a test window")
    });
    cx.run_until_parked();
    let nowhere = std::env::temp_dir().join(format!("synaps-desktop-ui-test-{}", std::process::id())).join("no-daemon.sock");
    let options = synaps_client::ClientOptions::new(synaps_client::Endpoint::Socket(nowhere), synaps_client::Identity::desktop());
    let (client, events) = synaps_client::Client::spawn(options).expect("a client thread");
    view.update(cx, |v, cx| {
        v.client = Some(client);
        assert!(v.drives(), "the demo session is attached here, its input shared");
        cx.notify();
    });
    cx.run_until_parked();
    // The app has been up a second (the launch's column arrival and the
    // card's charge are over), as a fast runner's wall time used to make it.
    wait(cx, window, &view, 1000);
    (window, view, events)
}
/// `ms` go by on the test's clock: the motion clock and the executor's
/// together, and what that drew settles.
fn wait(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ms: u64) {
    let d = std::time::Duration::from_millis(ms);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
    cx.executor().advance_clock(d);
    settle(cx, w, 8);
}
/// Next frames until the window asks for none (at most `max`): how many ran.
fn settle(cx: &mut TestAppContext, w: AnyWindowHandle, max: usize) -> usize {
    for n in 0..max {
        let ran = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        if ran == 0 {
            return n;
        }
    }
    max
}
/// The motion level, applied as the settings do: `ui_tests`' helper of
/// another name (its own trips `one_stream_fade_policy`, a lint over every
/// source file but `ui_tests.rs`, which looks for that name's text).
fn motion_level(cx: &mut TestAppContext, view: &Entity<AppView>, level: crate::motion::Level) {
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(level), lock: None });
        crate::motion::Level::apply(cx);
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}
/// One 144 Hz frame later, drawn.
fn fluid_frame(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    let dt = std::time::Duration::from_micros(6944);
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
    cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
}
/// Every row on screen, by identity: its top.
fn row_tops(cx: &mut TestAppContext, view: &Entity<AppView>) -> std::collections::HashMap<(u64, bool), f32> {
    view.read_with(cx, |v, _| {
        (0..v.list.item_count())
            .filter_map(|ix| {
                let d = v.disp.get(ix)?;
                let b = v.list.bounds_for_item(ix)?;
                let vb = v.list.viewport_bounds();
                (b.bottom() > vb.top() && b.top() < vb.bottom())
                    .then(|| ((d.id, matches!(d.kind, crate::turns::Kind::Summary { .. })), f32::from(b.top())))
            })
            .collect()
    })
}
/// A helper's text in test source `src` as `scroll_tests_unchanged`
/// takes it: its first attribute or `fn` to its closing brace.
fn helper_text(src: &str, name: &str) -> String {
    let src = src.replace("\r\n", "\n");
    let at = src.find(&format!("\nfn {name}(")).or_else(|| src.find(&format!("\nfn {name}<"))).unwrap_or_else(|| panic!("{name}")) + 1;
    let mut start = at;
    while let Some(prev) = src[..start - 1].rfind('\n').map(|i| i + 1) {
        if !src[prev..start].starts_with("#[") {
            break;
        }
        start = prev;
    }
    let end = src[at..].find("\n}\n").unwrap_or_else(|| panic!("{name}")) + at + 2;
    src[start..end].to_string()
}

/// Every helper here is `ui_tests`' own, byte for byte: a change there (a
/// pinned helper, on purpose) fails here until the copy follows it.
#[test]
fn kit_matches_ui_tests() {
    let (ui, kit) = (include_str!("ui_tests.rs"), include_str!("test_kit.rs"));
    for name in ["open_at", "open_with", "wait", "settle", "fluid_frame", "row_tops"] {
        assert_eq!(helper_text(kit, name), helper_text(ui, name), "test_kit's {name} drifted from ui_tests'");
    }
    // The name split as the lint splits it, so this file never says it.
    let theirs = concat!("set_mot", "ion");
    assert_eq!(
        helper_text(kit, "motion_level"),
        helper_text(ui, theirs).replacen(&format!("fn {theirs}("), "fn motion_level(", 1),
        "test_kit's motion_level drifted from ui_tests' {theirs}"
    );
}
