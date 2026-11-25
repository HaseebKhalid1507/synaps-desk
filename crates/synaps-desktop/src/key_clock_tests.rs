//! The once-a-press keys on the input clock (`crate::app::input_now`): a
//! key let go and pressed again less than `FoldKeys::PAIR` (30 ms) later
//! is its repeat; at 30 ms and after, a press. Each gate as the window
//! meets it, key-downs and key-ups through the window, the time between
//! them the test's ([`crate::app::advance_input_clock`]), never the wall's.
//!
//! Each test first puts the input clock an hour ahead of the wall: a
//! key-down or key-up stamped by the wall clock instead (a path left off
//! the input clock) would be an hour from the other, and the 29 ms repeat
//! would act or the 30 ms press would not.

use std::time::Duration;

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, PlatformInput, TestAppContext};

use super::open_at;
use crate::app::{AppView, advance_input_clock};

const BELOW: Duration = Duration::from_millis(29);
const AT: Duration = Duration::from_millis(30);
const ABOVE: Duration = Duration::from_millis(31);

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    advance_input_clock(Duration::from_secs(3600));
    (w, view)
}

/// `k` down and up, as the platform sends them, then a frame (a pane
/// move runs at its top). The input clock does not move.
fn key(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str) {
    let keystroke = gpui_kit::Keystroke::parse_with_os(k, cx.update(|cx| cx.platform_info().os)).expect("a keystroke");
    let down = gpui_kit::KeyDownEvent { keystroke: keystroke.clone(), is_held: false, prefer_character_input: false };
    for ev in [PlatformInput::KeyDown(down), PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke })] {
        cx.update_window(w, |_, window, cx| {
            window.dispatch_event(ev, cx);
        })
        .expect("the window");
        cx.run_until_parked();
    }
    let _ = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// `k` again `gap` after the last one was let go.
fn again(cx: &mut TestAppContext, w: AnyWindowHandle, k: &str, gap: Duration) {
    advance_input_clock(gap);
    key(cx, w, k);
}

/// Ctrl+B (the fold gate): 29 ms after its key-up a repeat, 30 and 31 ms
/// presses.
#[gpui_kit::test]
fn the_fold_key_is_a_press_from_30_ms(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    let rail = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panels.rail_collapsed);
    let r0 = rail(cx);
    key(cx, w, "ctrl-b");
    assert_eq!(rail(cx), !r0, "the first press folds");
    again(cx, w, "ctrl-b", BELOW);
    assert_eq!(rail(cx), !r0, "29 ms: a repeat, nothing");
    again(cx, w, "ctrl-b", AT);
    assert_eq!(rail(cx), r0, "30 ms: a press");
    again(cx, w, "ctrl-b", ABOVE);
    assert_eq!(rail(cx), !r0, "31 ms: a press");
}

/// Ctrl+T and Ctrl+W (one gate): Ctrl+T 29 ms after a Ctrl+W's key-up is
/// its repeat; at 30 and 31 ms a press.
#[gpui_kit::test]
fn the_tab_keys_are_a_press_from_30_ms(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    let tabs = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.tabs.len());
    let n = tabs(cx);
    key(cx, w, "ctrl-t");
    assert_eq!(tabs(cx), n + 1, "the first press opens a tab");
    again(cx, w, "ctrl-w", BELOW);
    assert_eq!(tabs(cx), n + 1, "Ctrl+W 29 ms after Ctrl+T: a repeat, nothing");
    again(cx, w, "ctrl-w", AT);
    assert_eq!(tabs(cx), n, "30 ms: a press, the tab closed");
    again(cx, w, "ctrl-t", ABOVE);
    assert_eq!(tabs(cx), n + 1, "31 ms: a press, a tab");
}

/// Esc in a worker's view (the card chord's gate: Enter, Esc, Space): a
/// new press goes back to the session, a repeat does nothing.
#[gpui_kit::test]
fn the_chord_keys_are_a_press_from_30_ms(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    let in_worker = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].watch.focus().is_some());
    let show = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| {
            v.model.lives[v.panes.cur.live].watch.open(&crate::watch::Candidate::of(1, "scout"), None, std::time::Instant::now());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(in_worker(cx), "scout's view in place");
    };
    show(cx);
    key(cx, w, "escape");
    assert!(!in_worker(cx), "the first press leaves it");
    show(cx);
    again(cx, w, "escape", BELOW);
    assert!(in_worker(cx), "29 ms: a repeat, still in it");
    again(cx, w, "escape", AT);
    assert!(!in_worker(cx), "30 ms: a press, back to the session");
    show(cx);
    again(cx, w, "escape", ABOVE);
    assert!(!in_worker(cx), "31 ms: a press");
}

/// Ctrl+Alt+→ and ← (one gate), split panes on: ← 29 ms after →'s
/// key-up is its repeat; at 30 ms a press, the tab back; → at 31 ms a
/// press. Not on Windows, as the split tests.
#[cfg(unix)]
#[gpui_kit::test]
fn the_move_keys_are_a_press_from_30_ms(cx: &mut TestAppContext) {
    let (w, view) = open(cx);
    view.update(cx, |v, cx| {
        v.app_settings.split = true;
        let other = crate::app::Open::Attach { session_id: crate::demo::FLAKY.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::New);
        cx.notify();
    });
    cx.run_until_parked();
    let _ = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let panes = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.panes.len());
    assert_eq!(panes(cx), 1);
    key(cx, w, "ctrl-alt-right");
    assert_eq!(panes(cx), 2, "the first press splits the tab off");
    again(cx, w, "ctrl-alt-left", BELOW);
    assert_eq!(panes(cx), 2, "29 ms: a repeat, nothing");
    again(cx, w, "ctrl-alt-left", AT);
    assert_eq!(panes(cx), 1, "30 ms: a press, the tab back");
    again(cx, w, "ctrl-alt-right", ABOVE);
    assert_eq!(panes(cx), 2, "31 ms: a press, split off again");
}
