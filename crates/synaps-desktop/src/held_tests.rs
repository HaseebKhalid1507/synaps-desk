//! An answered card's room held at the list's end, as the app met it
//! (#23 r6): the card's row goes from the list in the frame its room
//! lands, in a splice that may hold the list's top row. GPUI moves that
//! top row to the splice's start, offset 0 (`ListState::splice_focusable`),
//! and the rows of the splice have no height until they are laid out
//! again. `ui_tests`' held-room tests have their top row above it, every
//! row measured; here the window is short enough that it is in it (the
//! bench's h5 film at 1920×1080: the column jumped 64 px down as the room
//! landed, and once the output had used the room up the list stayed 88 px
//! short of its end, the output streaming under the composer).
//!
//! The helpers are `ui_tests`' own (that file is protected: it stays as it is).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};

use crate::app::AppView;

/// The `--offline` demo's window, `w`×`h` px, a client whose daemon is
/// nowhere (`ui_tests::open_with`, the beat on the layer).
fn open_at(
    cx: &mut TestAppContext,
    (w, h): (f32, f32),
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    let text = crate::theme::Panels::default().text_size;
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text }, cx);
        crate::app::bind_keys(cx);
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
    let nowhere = std::env::temp_dir().join(format!("synaps-desktop-held-test-{}", std::process::id())).join("no-daemon.sock");
    let options = synaps_client::ClientOptions::new(synaps_client::Endpoint::Socket(nowhere), synaps_client::Identity::desktop());
    let (client, events) = synaps_client::Client::spawn(options).expect("a client thread");
    view.update(cx, |v, cx| {
        v.client = Some(client);
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, window, &view, 1000);
    (window, view, events)
}

/// Next frames until the window asks for none (at most `max`).
fn settle(cx: &mut TestAppContext, w: AnyWindowHandle, max: usize) {
    for _ in 0..max {
        let ran = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        if ran == 0 {
            return;
        }
    }
}

/// `ms` on the test's clock: the motion clock and the executor's together.
fn wait(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ms: u64) {
    let d = std::time::Duration::from_millis(ms);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
    cx.executor().advance_clock(d);
    settle(cx, w, 8);
}

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
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
    cx.executor().advance_clock(dt);
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

/// The largest move of a row on screen between two of these.
fn moved(a: &std::collections::HashMap<(u64, bool), f32>, b: &std::collections::HashMap<(u64, bool), f32>) -> f32 {
    b.iter().filter_map(|(r, y)| a.get(r).map(|y0| (y - y0).abs())).fold(0f32, f32::max)
}

/// How far the list is short of its end (past its bottom padding).
fn short(cx: &mut TestAppContext, view: &Entity<AppView>) -> f32 {
    view.read_with(cx, |v, cx| {
        let pad = (28. * crate::theme::metrics(cx).r).round();
        f32::from(v.list.max_offset_for_scrollbar().y) + pad + f32::from(v.list.scroll_px_offset_for_scrollbar().y)
    })
}

/// The h5 flow (the bench's `SYNAPS_BENCH_HELD`): the demo's card answered
/// elsewhere while the list follows the tail, `extra` calls after its own
/// (the bench's card sits on its run's first call), its room held at the
/// end; the composer grows three lines (or not); a new call at the run's
/// end streams until the room is used up, and on. At these window heights
/// r5 (49a3c6a) strands the list: 6 calls at 560 px land the room at the
/// end, its base read before the splice took the card's row out above the
/// view (123 px short by line 30); 6 at 660 px and 10 at 800 px have the
/// list's top row in the splice (the column jumps 51 and 68 px down as the
/// room lands, short of its end from then on). The card as the list's
/// last row (no call after it) drew the room inside its own row: the
/// splice takes both out of GPUI's sums (a control: the room must not be
/// read as used up there). Every case: the column holds still as the room
/// lands (no row faster than a spring step), the list is at its end after
/// every line, and once the room is used up it follows.
#[gpui_kit::test]
fn a_card_room_landing_in_the_top_rows_splice_still_follows(cx: &mut TestAppContext) {
    for (extra, h, grow) in [(6usize, 560f32, true), (6, 660., false), (10, 800., true), (0, 600., true)] {
        let case = format!("{extra} calls after the card, {h} px{}", if grow { ", the composer three lines" } else { "" });
        let (w, view, _events) = open_at(cx, (1440., h));
        motion_level(cx, &view, crate::motion::Level::Full);
        view.update(cx, |v, _| {
            v.client = None;
            v.model.transcript.set_busy(true);
        });
        settle(cx, w, 8);
        let apply = |cx: &mut TestAppContext, e: LlmEvent| {
            view.update(cx, |v, cx| {
                v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(e) });
                v.sync_list();
                cx.notify();
            });
            cx.run_until_parked();
        };
        for c in 0..extra {
            let id = format!("x{c}");
            apply(
                cx,
                LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id.clone(),
                    input: serde_json::json!({"command": format!("cargo check -p c{c}")}),
                },
            );
            apply(cx, LlmEvent::ToolResult { tool_id: id, result: "ok".into() });
        }
        for _ in 0..60 {
            fluid_frame(cx, w, &view);
        }
        assert!(view.read_with(cx, |v, _| v.list.is_following_tail()), "{case}: following before the answer");
        let ids: Vec<u64> = view.read_with(cx, |v, _| v.model.transcript.pending_prompts().iter().map(|p| p.id).collect());
        assert!(!ids.is_empty(), "{case}: the demo's card");
        view.update(cx, |v, cx| {
            for prompt_id in ids {
                v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id });
            }
            cx.notify();
        });
        // The card folds into its row, its room lands: a frame at a time.
        let mut last = row_tops(cx, &view);
        let mut landing = 0f32;
        for _ in 0..120 {
            fluid_frame(cx, w, &view);
            let now = row_tops(cx, &view);
            landing = landing.max(moved(&last, &now));
            last = now;
        }
        assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "{case}: the room it left, held");
        let landed = short(cx, &view);
        if grow {
            cx.update_window(w, |_, window, cx| {
                view.update(cx, |v, cx| {
                    v.composer
                        .update(cx, |s, cx| s.set_value("steer: and the relay too\nkeep the old token\nthen run it again", window, cx))
                })
            })
            .expect("the window");
            for _ in 0..80 {
                fluid_frame(cx, w, &view);
            }
            assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "{case}: held as the composer grew");
        }
        // A new call at the run's end: its output grows into the room,
        // then past it.
        apply(
            cx,
            LlmEvent::ToolUse {
                tool_name: "bash".into(),
                tool_id: "h0".into(),
                input: serde_json::json!({"command": "cargo test -p relay --test held"}),
            },
        );
        apply(cx, LlmEvent::ToolStarted { tool_id: "h0".into() });
        let (mut used, mut worst) = (None, (0f32, 0usize));
        for k in 0..30 {
            apply(cx, LlmEvent::ToolResultDelta { tool_id: "h0".into(), delta: format!("test held_{k} ... ok\n") });
            for _ in 0..10 {
                fluid_frame(cx, w, &view);
            }
            if used.is_none() && view.read_with(cx, |v, _| v.fold_slack.get().is_none()) {
                used = Some(k);
            }
            let s = short(cx, &view);
            if s > worst.0 {
                worst = (s, k);
            }
        }
        let (following, end) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.at_end()));
        eprintln!(
            "{case}: landing step {landing} px, {landed} px short as it landed; used up at line {used:?}; worst {} px short (line {}); following {following}, at end {end:?}",
            worst.0, worst.1
        );
        assert!(landing <= 17.5, "{case}: as the room landed a row moved {landing} px in a frame");
        assert!(landed <= crate::app::END_SLOP, "{case}: landed {landed} px short of its end");
        assert!(used.is_some(), "{case}: the output used the room up");
        assert!(worst.0 <= crate::app::END_SLOP, "{case}: line {}: {} px short of its end", worst.1, worst.0);
        assert!(following && end == Some(true), "{case}: it follows the tail ({following}, {end:?})");
    }
}
