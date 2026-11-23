//! Cards D's follow-ups (#26, from #23's reviews): the late-frame rule at
//! a slow display's pace, a card's bottom edge as a call
//! is appended, a streamed line that wraps, two folds at once, and the room
//! a fold at the end holds.
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`); the few
//! here are this file's.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, Modifiers, MouseMoveEvent, Point, TestAppContext, VisualTestContext, px};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};

use super::{fluid_frame, motion_level, open_at, row_tops, settle, wait};
use crate::app::AppView;
use crate::turns::Kind;

type Window = (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>);

/// The demo's window at `h` px tall, at Full, showing the idle-CPU
/// session's finished runs (the synthetic transport, no client).
fn idle_cpu(cx: &mut TestAppContext, h: f32) -> Window {
    let (w, view, events) = open_at(cx, (1440., h), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.client = None;
        let other = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::Current);
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    (w, view, events)
}

/// The runs' summaries, in order.
fn runs(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<u64> {
    view.read_with(cx, |v, _| v.disp.iter().filter(|d| matches!(d.kind, Kind::Summary { .. })).map(|d| d.id).collect())
}

fn flip(cx: &mut TestAppContext, view: &Entity<AppView>, id: u64) {
    view.update(cx, |v, cx| {
        v.toggle_run(id);
        cx.notify();
    });
    cx.run_until_parked();
}

/// Run `id`'s header `by` px under the view's top.
fn header_at(cx: &mut TestAppContext, view: &Entity<AppView>, id: u64, by: f32) {
    view.update(cx, |v, cx| {
        let ix = v.disp.iter().position(|d| d.id == id && matches!(d.kind, Kind::Summary { .. })).expect("its header");
        v.list.scroll_to(gpui::ListOffset { item_ix: ix, offset_in_item: px(0.) });
        v.list.scroll_by(px(-by));
        cx.notify();
    });
}

fn header(cx: &mut TestAppContext, w: AnyWindowHandle, id: u64) -> Option<f32> {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(format!("run-summary {id}"))).map(|b| f32::from(b.top()))
}

fn folding(cx: &mut TestAppContext, w: AnyWindowHandle) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds("fold-gap").is_some()
}

/// The largest move of a row on screen (by identity) between two frames.
fn moved(a: &std::collections::HashMap<(u64, bool), f32>, b: &std::collections::HashMap<(u64, bool), f32>) -> f32 {
    b.iter().filter_map(|(r, y)| a.get(r).map(|y0| (y - y0).abs())).fold(0f32, f32::max)
}

/// [`fluid_frame`] at `dt` a frame: a slower display's.
fn frame_every(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, dt: Duration) {
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

/// One 144 Hz frame of the clock, its timers firing, and a frame drawn
/// only if the window asked for one: the app's frames drawn in it (its
/// renders, `fx.frames()`), and the animation frames it had queued.
/// Nothing forces a draw, so what it counts is demand. A timer's
/// `cx.notify` draws a frame with none queued: it counts (Shady #73 R2-M1).
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

/// The window at rest: the focus off the composer (its caret blinks a
/// frame every 500 ms), a second idle, then a second more with no frame
/// drawn and none queued.
fn at_rest(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| window.focus(&v.focus, cx))).expect("the window");
    cx.run_until_parked();
    for _ in 0..144 {
        idle_frame(cx, w, view);
    }
    for i in 0..144 {
        assert_eq!(idle_frame(cx, w, view), (0, 0), "at rest, frame {i}: drawn, queued");
    }
}

/// Each frame the app draws from now on, at its paint ([`AppView::on_draw`]):
/// whether its pane holds a fold's room, and whether it asked for the next
/// frame.
fn paints(cx: &mut TestAppContext, view: &Entity<AppView>) -> Rc<RefCell<Vec<(bool, bool)>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    view.update(cx, |v, _| {
        v.on_draw = Some(Rc::new(move |v: &AppView| l.borrow_mut().push((v.fold_slack.get().is_some(), v.fx.pending().0))))
    });
    log
}

/// Shady #73 R2-M1: [`idle_frame`] counts the frames drawn, not only the
/// animation frames queued: a timer's `cx.notify` on a window at rest draws
/// one with none queued, and it is seen.
#[gpui_kit::test]
fn an_idle_frame_sees_a_timer_s_draw(cx: &mut TestAppContext) {
    let (w, view, _events) = idle_cpu(cx, 900.);
    at_rest(cx, w, &view);
    view.update(cx, |_, cx| {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(3)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    });
    let (drawn, ran) = idle_frame(cx, w, &view);
    assert!(drawn >= 1 && ran == 0, "a timer's draw: {drawn} drawn, {ran} queued");
}

/// L5: a frame is late past half again the display's pace, not past a
/// 60 Hz frame's: at 30 Hz (VNC, a throttled window) a fold lands in about
/// the time it takes at 144 Hz (each frame paused for 16.6 ms, it took
/// twice as long). A hitch still pauses it (`fold_frames_hitched`).
#[gpui_kit::test]
fn a_fold_at_30_hz_takes_the_time_it_takes_at_144(cx: &mut TestAppContext) {
    let (w, view, _events) = idle_cpu(cx, 1400.);
    let runs = runs(cx, &view);
    let a = runs[runs.len() - 2];
    let mut took = Vec::new();
    for dt in [Duration::from_micros(6944), Duration::from_micros(33_333)] {
        flip(cx, &view, a);
        for _ in 0..240 {
            fluid_frame(cx, w, &view);
        }
        header_at(cx, &view, a, 40.);
        for _ in 0..8 {
            fluid_frame(cx, w, &view);
        }
        flip(cx, &view, a);
        let mut n = 0u32;
        while n == 0 || folding(cx, w) {
            frame_every(cx, w, &view, dt);
            n += 1;
            assert!(n < 400, "never landed");
        }
        took.push(dt * n);
    }
    assert!(took[1] <= took[0] * 6 / 5 + Duration::from_millis(34), "a fold at 30 Hz took {:?}, at 144 Hz {:?}", took[1], took[0]);
}

/// #23 round 2 L3 (round 1's H2): a call appended to a three-row run at
/// Full grows the card under the run's last row. In every frame exactly
/// one of its rows draws the card's bottom edge, that row's slice is the
/// card's bottom, and the card's bottom never rises.
#[gpui_kit::test]
fn a_call_appended_to_a_run_keeps_one_bottom_edge(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    let call = |cx: &mut TestAppContext, k: usize| {
        view.update(cx, |v, cx| {
            let t = &mut v.model.transcript;
            if k == 0 {
                t.set_busy(true);
                t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
            }
            let id = format!("edge-run-{k}");
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.clone() }),
            });
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id,
                    input: serde_json::json!({"command": format!("cargo test -p crate{k}")}),
                }),
            });
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
    };
    for k in 0..3 {
        call(cx, k);
    }
    wait(cx, w, &view, 1000);
    let rows = |cx: &mut TestAppContext| -> Vec<(usize, u64)> {
        view.read_with(cx, |v, _| {
            let items = v.model.transcript.items();
            v.disp
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    d.kind == Kind::Work
                        && items.get(d.item).and_then(|i| i.tool.as_ref()).is_some_and(|t| t.tool_id.starts_with("edge-run-"))
                })
                .map(|(ix, d)| (ix, d.id))
                .collect()
        })
    };
    assert_eq!(rows(cx).len(), 3, "a three-row run");
    call(cx, 3);
    let mut low: Option<f32> = None;
    for f in 0..90 {
        fluid_frame(cx, w, &view);
        let rows = rows(cx);
        assert_eq!(rows.len(), 4, "frame {f}: the run's rows");
        let edged: Vec<u64> =
            view.read_with(cx, |v, _| rows.iter().filter(|(ix, _)| crate::cards::edges(&v.disp, *ix).1).map(|r| r.1).collect());
        assert_eq!(edged.len(), 1, "frame {f}: one row draws the bottom edge: {edged:?}");
        let mut vcx = VisualTestContext::from_window(w, cx);
        let slices: Vec<(u64, f32)> = rows
            .iter()
            .filter_map(|(_, id)| vcx.debug_bounds(String::leak(format!("card-slice {id}"))).map(|b| (*id, f32::from(b.bottom()))))
            .collect();
        let bottom = slices.iter().map(|s| s.1).fold(f32::MIN, f32::max);
        let edge = slices.iter().find(|s| s.0 == edged[0]).map(|s| s.1);
        assert_eq!(edge, Some(bottom), "frame {f}: the edged row is the card's bottom: {slices:?}");
        if let Some(l) = low {
            assert!(bottom >= l - 0.5, "frame {f}: the card's bottom rose {l} -> {bottom}");
        }
        low = Some(bottom);
    }
}

/// #23 round 2 L6: a streamed line that wraps grows to all of its wraps on
/// `flick`, so its body never pops a line or two taller when the line's
/// entry retires (it was clipped to one line until then).
#[gpui_kit::test]
fn a_wrapped_streamed_line_never_pops(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.client = None;
        let t = &mut v.model.transcript;
        t.set_busy(true);
        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
        t.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: "wrap-0".into() }),
        });
        t.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolUse {
                tool_name: "bash".into(),
                tool_id: "wrap-0".into(),
                input: serde_json::json!({"command": "cargo test"}),
            }),
        });
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    let delta = |cx: &mut TestAppContext, text: &str| {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResultDelta { tool_id: "wrap-0".into(), delta: text.into() }),
            });
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
    };
    delta(cx, "running 3 tests\n");
    wait(cx, w, &view, 600);
    let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
    let well = |cx: &mut TestAppContext| {
        VisualTestContext::from_window(w, cx).debug_bounds(String::leak(format!("well {id}"))).map(|b| f32::from(b.size.height))
    };
    let line = cx.update(|cx| (19. * crate::theme::metrics(cx).r).round());
    let h0 = well(cx).expect("a running call with output opens on it");
    let long = format!("{}\n", "error[E0308]: mismatched types, expected `u16`, found `u32` in this argument; ".repeat(5));
    delta(cx, &long);
    let mut hs = vec![h0];
    for _ in 0..90 {
        fluid_frame(cx, w, &view);
        hs.push(well(cx).expect("the well"));
    }
    let end = hs[hs.len() - 1];
    assert!(end >= h0 + 2. * line, "the line wraps: {h0} -> {end}");
    let step = hs.windows(2).map(|p| p[1] - p[0]).fold(0f32, f32::max);
    assert!(step <= 0.75 * line, "the well grew {step} px in a frame (a line {line}): {hs:?}");
    assert!(hs.windows(2).all(|p| p[1] >= p[0]), "it only grows: {hs:?}");
}

/// Two folds at once close on one spring (#26, the design call: a card's
/// room and a fold already share one): the rows under both rooms move as
/// one fold of their sum would, at most a fold's top speed (1617 px/s, 11.2
/// px a 144 Hz frame, and a pixel's rounding), never two springs' sum.
#[gpui_kit::test]
fn two_folds_at_once_close_on_one_spring(cx: &mut TestAppContext) {
    let (w, view, _events) = idle_cpu(cx, 1400.);
    let runs = runs(cx, &view);
    let (a, b) = (runs[runs.len() - 2], runs[runs.len() - 1]);
    flip(cx, &view, a);
    flip(cx, &view, b);
    for _ in 0..240 {
        fluid_frame(cx, w, &view);
    }
    header_at(cx, &view, a, 40.);
    for _ in 0..8 {
        fluid_frame(cx, w, &view);
    }
    let ya = header(cx, w, a).expect("a's header on screen");
    flip(cx, &view, a);
    for _ in 0..3 {
        fluid_frame(cx, w, &view);
    }
    flip(cx, &view, b);
    assert_eq!(view.read_with(cx, |v, _| v.fold_gap.borrow().len()), 2, "both rooms closing");
    let mut last = row_tops(cx, &view);
    let (mut step, mut n) = (0f32, 0);
    while n == 0 || folding(cx, w) {
        fluid_frame(cx, w, &view);
        let now = row_tops(cx, &view);
        step = step.max(moved(&last, &now));
        last = now;
        n += 1;
        assert!(n < 600, "never landed");
    }
    assert!(step <= 12.5, "two folds: a row moved {step} px in one frame");
    assert_eq!(header(cx, w, a), Some(ya), "the first header holds");
}

/// The design gate of #23 r4, S6: the room a fold at the list's end holds
/// goes on `sheet` once its header can't be about to be clicked again:
/// with the pointer never over it, a second after it landed; with the
/// pointer on it, not while it stays, and once it leaves. Then the list is
/// at its end, and nothing moved in one frame.
#[gpui_kit::test]
fn a_held_fold_room_goes_when_the_pointer_leaves_its_header(cx: &mut TestAppContext) {
    let (w, view, _events) = idle_cpu(cx, 900.);
    let runs = runs(cx, &view);
    let last = runs[runs.len() - 1];
    let pointer = |cx: &mut TestAppContext, at: Point<gpui_kit::Pixels>| {
        let mut vcx = VisualTestContext::from_window(w, cx);
        vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() });
        vcx.run_until_parked();
    };
    flip(cx, &view, last);
    for over in [false, true] {
        pointer(cx, gpui_kit::point(px(2.), px(2.)));
        for _ in 0..240 {
            fluid_frame(cx, w, &view);
        }
        view.update(cx, |v, cx| {
            v.list.scroll_to_end();
            cx.notify();
        });
        for _ in 0..8 {
            fluid_frame(cx, w, &view);
        }
        flip(cx, &view, last);
        if over {
            let b = VisualTestContext::from_window(w, cx)
                .debug_bounds(String::leak(format!("run-summary {last}")))
                .expect("the header on screen");
            pointer(cx, b.center());
        }
        let mut n = 0;
        while n == 0 || folding(cx, w) {
            fluid_frame(cx, w, &view);
            n += 1;
            assert!(n < 600, "never landed");
        }
        assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "over {over}: the fold's room, held at the end");
        let held = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fold_slack.get().map(|s| s.last()));
        let r0 = held(cx).expect("held");
        // Most of a second: it holds.
        for _ in 0..120 {
            fluid_frame(cx, w, &view);
        }
        assert_eq!(held(cx), Some(r0), "over {over}: held for most of a second");
        if over {
            for _ in 0..300 {
                fluid_frame(cx, w, &view);
            }
            assert_eq!(held(cx), Some(r0), "the pointer on its header: held");
            pointer(cx, gpui_kit::point(px(2.), px(2.)));
        }
        let mut last_tops = row_tops(cx, &view);
        let (mut step, mut n) = (0f32, 0);
        while held(cx).is_some() {
            fluid_frame(cx, w, &view);
            let now = row_tops(cx, &view);
            step = step.max(moved(&last_tops, &now));
            last_tops = now;
            n += 1;
            assert!(n < 400, "over {over}: the room never went");
        }
        for _ in 0..8 {
            fluid_frame(cx, w, &view);
        }
        assert_eq!(view.read_with(cx, |v, _| v.at_end()), Some(true), "over {over}: at its end");
        assert!(step <= 30., "over {over}: a row moved {step} px in a frame as the room went");
        flip(cx, &view, last);
    }
}

/// Shady #73 M1: a held room asks for nothing once it is gone (nothing
/// after a landing, #58). Released on its timer with the pointer away, its
/// sheet lands, the frame that draws it gone asks for no other, and the
/// window draws no frame after that; used up by rows growing in before its
/// deadline, no release timer wakes it later. Only the clock and its
/// timers move: no frame is forced, and every frame drawn counts (R2-M1:
/// a timer's draw too, not only an animation frame queued). Once a frame
/// draws the room gone, none draws it back.
#[gpui_kit::test]
fn a_held_room_asks_for_nothing_once_it_is_gone(cx: &mut TestAppContext) {
    let mut seen = Vec::new();
    for used_up in [false, true] {
        let (w, view, _events) = idle_cpu(cx, 900.);
        let runs = runs(cx, &view);
        let last = runs[runs.len() - 1];
        let mut vcx = VisualTestContext::from_window(w, cx);
        vcx.simulate_event(MouseMoveEvent {
            position: gpui_kit::point(px(2.), px(2.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        vcx.run_until_parked();
        at_rest(cx, w, &view);
        flip(cx, &view, last);
        for _ in 0..240 {
            fluid_frame(cx, w, &view);
        }
        view.update(cx, |v, cx| {
            v.list.scroll_to_end();
            cx.notify();
        });
        for _ in 0..8 {
            fluid_frame(cx, w, &view);
        }
        flip(cx, &view, last);
        let mut n = 0;
        while n == 0 || folding(cx, w) {
            fluid_frame(cx, w, &view);
            n += 1;
            assert!(n < 600, "never landed");
        }
        let held = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fold_slack.get().map(|s| s.last()));
        assert!(held(cx).is_some_and(|r| r >= 1.), "used up {used_up}: the fold's room, held");
        if used_up {
            // Rows grow in under it (a history append): they take the room.
            view.update(cx, |v, cx| {
                let t = &mut v.model.transcript;
                let text = "A long answer that fills the room the fold left. ".repeat(60);
                let messages =
                    (0..6).map(|_| serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": text}]})).collect();
                t.apply_update(&synaps_client::HistoryUpdate::Append { base_len: t.history_len(), messages }).expect("an append");
                v.sync_list();
                cx.notify();
            });
            cx.run_until_parked();
        }
        // Idle until the room is gone, then for two seconds more: the draws
        // after it are what its release left behind.
        let drawn = paints(cx, &view);
        let mut n = 0;
        while held(cx).is_some() {
            idle_frame(cx, w, &view);
            n += 1;
            assert!(n < 1200, "used up {used_up}: the room never went");
        }
        let gone_at = view.read_with(cx, |v, _| v.fx.clock());
        let mut late = Vec::new();
        for _ in 0..288 {
            if idle_frame(cx, w, &view).0 > 0 {
                late.push(view.read_with(cx, |v, _| v.fx.clock()).saturating_duration_since(gone_at).as_millis());
            }
        }
        view.update(cx, |v, _| v.on_draw = None);
        // (Used up, the rows' growth may take it in a frame of their own.)
        let drawn = drawn.take();
        let went = drawn.iter().position(|(room, _)| !room);
        assert!(used_up || went.is_some(), "released: no frame drew the room gone ({drawn:?})");
        if let Some(went) = went {
            assert!(drawn[went..].iter().all(|(room, _)| !room), "used up {used_up}: the room drawn back after it went ({drawn:?})");
            assert!(used_up || !drawn[went].1, "released: the frame that drew the room gone asked for another ({drawn:?})");
        }
        seen.push((used_up, late));
    }
    // Released: nothing after its sheet lands. Used up: rows that grew in
    // may still be landing, but nothing past that, and nothing at the
    // deadline the room had (a second from its landing).
    let ok = |(used_up, late): &(bool, Vec<u128>)| if *used_up { late.iter().all(|ms| *ms < 700) } else { late.is_empty() };
    assert!(seen.iter().all(ok), "draws after the room went (used up?, ms after): {seen:?}");
}

/// Shady #73 M2: a streamed line retires (drawn whole, unclipped) only once
/// its clip is within half a pixel of its measured height, however tall
/// it wraps: a long line in a narrow window draws no step in the frame its
/// clip goes. Before, it retired at 1 % of its spring left: a step of
/// about 1 % of its height.
#[gpui_kit::test]
fn a_tall_wrapped_line_retires_without_a_step(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (900., 1400.), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.client = None;
        let t = &mut v.model.transcript;
        t.set_busy(true);
        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
        t.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: "tall-0".into() }),
        });
        t.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolUse {
                tool_name: "bash".into(),
                tool_id: "tall-0".into(),
                input: serde_json::json!({"command": "cargo build"}),
            }),
        });
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    let delta = |cx: &mut TestAppContext, text: &str| {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResultDelta { tool_id: "tall-0".into(), delta: text.into() }),
            });
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
    };
    delta(cx, "compiling\n");
    wait(cx, w, &view, 600);
    let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
    let line = cx.update(|cx| (19. * crate::theme::metrics(cx).r).round());
    let tall = format!("{}\n", "warning: unused variable `x` in this long diagnostic line; ".repeat(60));
    delta(cx, &tall);
    // Each frame as painted: the clipped line's height (while it has a
    // clip), and the well's.
    let mut frames: Vec<(Option<f32>, f32)> = Vec::new();
    for _ in 0..150 {
        fluid_frame(cx, w, &view);
        let mut vcx = VisualTestContext::from_window(w, cx);
        let clip = vcx.debug_bounds(String::leak(format!("stream-line {id} 1"))).map(|b| f32::from(b.size.height));
        let well = vcx.debug_bounds(String::leak(format!("well {id}"))).map(|b| f32::from(b.size.height)).expect("the well");
        frames.push((clip, well));
    }
    let retired = frames.windows(2).position(|p| p[0].0.is_some() && p[1].0.is_none()).expect("the line's clip goes") + 1;
    let (before, after) = (frames[retired - 1], frames[retired]);
    let tall_h = frames.iter().filter_map(|f| f.0).fold(0f32, f32::max);
    assert!(tall_h >= 12. * line, "the line wraps tall: {tall_h} px (a line {line})");
    assert!(
        (after.1 - before.1).abs() <= 1.,
        "the frame its clip went, the well stepped {} px (clipped {:?} -> unclipped): {frames:?}",
        after.1 - before.1,
        before.0
    );
    assert!(frames.windows(2).all(|p| p[1].1 >= p[0].1), "it only grows: {frames:?}");
}
