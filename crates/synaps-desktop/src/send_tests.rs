//! A message sent takes the transcript back to its end (Haseeb,
//! 2026-11-21: "if I'm scrolled up and I send a message, it should bring
//! me back down"): a submit, a steer into a running turn, with a room held
//! at the end; a command that adds nothing to the transcript leaves the
//! list where it is. The daemon's echo of the message is applied by hand
//! (there is none).
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`).

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext, px};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};

use super::{fluid_frame, motion_level, open_at, row_tops, settle};
use crate::app::AppView;
use crate::motion::Level;

/// The demo at 1440×900 and 12 turns after it (pages to scroll back
/// through), `level`, its turn `busy` or not.
fn open(cx: &mut TestAppContext, level: Level, busy: bool) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, level);
    view.update(cx, |v, cx| {
        for n in 0..12 {
            let t = &mut v.model.transcript;
            let user_text = Some(format!("Question {n}: where does the refresh race start, and what holds the lock?"));
            t.apply(&SessionEvent::TurnStarted { turn_baseline: 12 + 2 * n, trigger: "user".into(), user_text });
            let text = format!("Answer {n}: in the relay's token cache. ").repeat(6);
            t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text }) });
            t.apply(&SessionEvent::Idle);
        }
        v.sync_list();
        v.model.transcript.set_busy(busy);
        cx.notify();
    });
    settle(cx, w, 8);
    (w, view)
}

/// How far the list is from its end (past its bottom padding), whether it
/// follows the tail.
fn from_end(cx: &mut TestAppContext, view: &Entity<AppView>) -> (f32, bool) {
    view.read_with(cx, |v, cx| {
        let pad = (28. * crate::theme::metrics(cx).r).round();
        let max = f32::from(v.list.max_offset_for_scrollbar().y) + pad;
        (max + f32::from(v.list.scroll_px_offset_for_scrollbar().y), v.list.is_following_tail())
    })
}

/// Scrolled `n` px back from the end, as the wheel leaves it.
fn scroll_back(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, n: f32) {
    view.update(cx, |v, cx| {
        v.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    view.update(cx, |v, cx| {
        v.scroller.touch();
        v.list.scroll_by(px(-n));
        cx.notify();
    });
    for _ in 0..4 {
        fluid_frame(cx, w, view);
    }
    let (gap, following) = from_end(cx, view);
    assert!(!following && gap > n - 40., "scrolled back {n}: {gap} px from the end");
}

/// `text` in the composer, Enter.
fn send(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str) {
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.composer.update(cx, |s, cx| {
                s.set_value(text.to_string(), window, cx);
                s.focus(window, cx);
            })
        })
    })
    .expect("the window");
    cx.run_until_parked();
    VisualTestContext::from_window(w, cx).simulate_keystrokes("enter");
    cx.run_until_parked();
}

/// The daemon's echo: the message in the transcript (`busy`: a steer, its
/// notice row).
fn echo(cx: &mut TestAppContext, view: &Entity<AppView>, text: &str, busy: bool) {
    view.update(cx, |v, cx| {
        let e = if busy {
            SessionEvent::Steered { text: text.into(), delivered: true }
        } else {
            SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some(text.into()) }
        };
        v.model.transcript.apply(&e);
        v.sync_list();
        cx.notify();
    });
    cx.run_until_parked();
}

/// The last row's bounds and the view's, top and bottom.
/// The message's row (the last whose text has `text`: a steer's is its
/// notice), its bounds and the view's, top and bottom.
fn message_row(cx: &mut TestAppContext, view: &Entity<AppView>, text: &str) -> (Option<(f32, f32)>, (f32, f32)) {
    view.read_with(cx, |v, _| {
        let items = v.model.shown().items();
        let ix = (0..v.disp.len())
            .rev()
            .find(|&ix| items.get(v.disp[ix].item).is_some_and(|i| i.text.contains(text)))
            .expect("the message's row");
        let vb = v.list.viewport_bounds();
        let row = v.list.bounds_for_item(ix).map(|b| (f32::from(b.top()), f32::from(b.bottom())));
        (row, (f32::from(vb.top()), f32::from(vb.bottom())))
    })
}

/// Sent `text` from `back` px up, the echo two frames later: every frame
/// moves the list down (never back up), the first within a view and a half
/// of the end (no glide through pages), and it lands at the end, following,
/// the message's row whole in the view. At Reduced and Off, at the end in
/// the send's frame. The frames it took.
fn sends_home(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, back: f32, text: &str, busy: bool) -> usize {
    let level = view.read_with(cx, |v, _| v.fx.level());
    let sent = view.read_with(cx, |v, _| v.submits.len());
    // The motion clock up to the last timer that woke the app (`ui_tests`'
    // `catch_up`): else every frame samples that instant until it is past.
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick()));
    send(cx, w, view, text);
    assert_eq!(view.read_with(cx, |v, _| v.submits.len()), sent + 1, "{text}: sent");
    let vh = view.read_with(cx, |v, _| f32::from(v.list.viewport_bounds().size.height));
    let mut frames = 0;
    // From the view's bottom (a dock that leaves with the send gives the
    // view its room: following, the end stays the bottom).
    let tops = |cx: &mut TestAppContext| {
        let bottom = view.read_with(cx, |v, _| f32::from(v.list.viewport_bounds().bottom()));
        row_tops(cx, view).into_iter().map(|(r, y)| (r, y - bottom)).collect::<std::collections::HashMap<_, _>>()
    };
    let mut last = tops(cx);
    if level != Level::Full {
        fluid_frame(cx, w, view);
        let (gap, following) = from_end(cx, view);
        assert!(gap <= 1. && following, "{level:?} {text}: at the end in its frame ({gap})");
    }
    for f in 0..240 {
        if f == 2 {
            echo(cx, view, text, busy);
        }
        fluid_frame(cx, w, view);
        frames += 1;
        let (gap, following) = from_end(cx, view);
        if f == 0 {
            assert!(gap <= 1.5 * vh + 1., "{text}: the first frame within a view and a half of the end ({gap}, from {back})");
        }
        // Down the list: every row on screen moves up, never down (by
        // identity: hinted rows above measured meanwhile move the offset).
        let now = tops(cx);
        for (r, y) in &now {
            if let Some(y0) = last.get(r) {
                assert!(*y <= y0 + 0.5, "{text}: frame {f}: row {r:?} went down ({y0} -> {y})");
            }
        }
        last = now;
        if following && gap <= 1. && f > 4 && !view.read_with(cx, |v, _| v.scroller.active()) {
            break;
        }
    }
    let (gap, following) = from_end(cx, view);
    assert!(gap <= 1. && following, "{text}: at the end, following ({gap}, {following})");
    let (row, (top, bottom)) = message_row(cx, view, text);
    let (t, b) = row.expect("the message's row drawn");
    assert!(t >= top && b <= bottom + 0.5, "{text}: the message's row in view ({t}..{b} in {top}..{bottom})");
    frames
}

/// Scrolled back a little and far (pages), a message is sent: the list
/// goes back to its end and follows it (Full: glides, Reduced and Off:
/// cuts).
#[gpui_kit::test]
fn a_send_returns_to_the_end(cx: &mut TestAppContext) {
    for level in [Level::Full, Level::Reduced, Level::Off] {
        for back in [300., 3000.] {
            let (w, view) = open(cx, level, false);
            scroll_back(cx, w, &view, back);
            let frames = sends_home(cx, w, &view, back, &format!("message from {back} px up"), false);
            assert!(level != Level::Full || frames > 10, "{level:?} {back}: it glided ({frames} frames)");
        }
    }
}

/// A steer into the running turn is a message too.
#[gpui_kit::test]
fn a_steer_returns_to_the_end(cx: &mut TestAppContext) {
    let (w, view) = open(cx, Level::Full, true);
    scroll_back(cx, w, &view, 900.);
    sends_home(cx, w, &view, 900., "steer: keep the public API", true);
}

/// A room held at the end (the demo's card answered elsewhere while
/// following): scrolled back from it, a send goes home and follows; the
/// room goes on `sheet`.
#[gpui_kit::test]
fn a_send_with_a_held_room_returns_and_follows(cx: &mut TestAppContext) {
    let (w, view) = open(cx, Level::Full, true);
    for _ in 0..60 {
        fluid_frame(cx, w, &view);
    }
    assert!(view.read_with(cx, |v, _| v.list.is_following_tail()), "following before the answer");
    let ids: Vec<u64> = view.read_with(cx, |v, _| v.model.transcript.pending_prompts().iter().map(|p| p.id).collect());
    assert!(!ids.is_empty(), "the demo's card");
    view.update(cx, |v, cx| {
        for prompt_id in ids {
            v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id });
        }
        cx.notify();
    });
    for _ in 0..120 {
        fluid_frame(cx, w, &view);
    }
    assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "a room held at the end");
    view.update(cx, |v, cx| {
        v.scroller.touch();
        v.list.scroll_by(px(-400.));
        cx.notify();
    });
    for _ in 0..4 {
        fluid_frame(cx, w, &view);
    }
    assert!(!view.read_with(cx, |v, _| v.list.is_following_tail()), "scrolled back");
    sends_home(cx, w, &view, 400., "and add a regression test", true);
    for _ in 0..120 {
        fluid_frame(cx, w, &view);
    }
    assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_none()), "the room went");
    let (gap, following) = from_end(cx, &view);
    assert!(gap <= 1. && following, "still at the end, following ({gap})");
}

/// A command that adds nothing to the transcript (`/compact`, run by the
/// engine) leaves the list where the reader has it.
#[gpui_kit::test]
fn a_command_leaves_the_scroll_alone(cx: &mut TestAppContext) {
    let (w, view) = open(cx, Level::Full, false);
    scroll_back(cx, w, &view, 600.);
    let before = from_end(cx, &view);
    let top = view.read_with(cx, |v, _| v.list.logical_scroll_top());
    send(cx, w, &view, "/compact");
    for _ in 0..30 {
        fluid_frame(cx, w, &view);
    }
    assert!(view.read_with(cx, |v, _| v.submits.len()) >= 1, "it went to submit");
    assert_eq!(view.read_with(cx, |v, _| v.list.logical_scroll_top()), top, "the list where it was");
    assert!(!from_end(cx, &view).1 && before.0 > 500., "not following (the dock's note may take the view's room)");
}
