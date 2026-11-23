//! Waits for the view tests that run a client on a real socket
//! (`split_tests`): on the event itself, or on the executor the test
//! drives, never against a wall-clock deadline. A loaded runner takes
//! longer; it never fails for it. On ubuntu CI, where dozens of UI tests
//! ran past 60 s, two split tests gave up on their 10 s waits ("timed out
//! waiting for A attached", #71) or clicked a card that had armed in the
//! wall time a frame took (#69).
//!
//! A wait whose event never comes hangs instead of failing, and the test
//! harness names it ("has been running for over 60 seconds"). A channel
//! closed under a wait (its client or daemon gone) fails at once.

use std::sync::mpsc;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use synaps_client::Event;

use crate::app::AppView;

/// How far the test's clock moves while an attach waits on its pane's
/// [`crate::tabs::AttachGate`] (its timer runs on the test's clock), as
/// `split_tests::next_attach` moved it on every poll.
const GATE_STEP: Duration = Duration::from_millis(50);

/// How long a poll lets the client's and the daemon's threads run before
/// the next look: a pace, not a deadline.
const POLL: Duration = Duration::from_millis(5);

/// The client's events, each as it comes, until one matches `done`.
pub(crate) fn until(events: &async_channel::Receiver<Event>, what: &str, done: impl Fn(&Event) -> bool) {
    loop {
        let ev = events.recv_blocking().unwrap_or_else(|_| panic!("the client closed before {what}"));
        if done(&ev) {
            return;
        }
    }
}

/// The client's events into the view as its pump hands them
/// (`apply_batch`), a frame drawn after each look, until `done`.
pub(crate) fn feed(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<AppView>,
    events: &async_channel::Receiver<Event>,
    what: &str,
    done: impl Fn(&AppView) -> bool,
) {
    loop {
        let batch: Vec<Event> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        if !batch.is_empty() {
            view.update(cx, |v, cx| v.apply_batch(batch, cx));
        }
        frame(cx, window);
        if view.read_with(cx, |v, _| done(v)) {
            return;
        }
        if events.is_closed() && events.is_empty() {
            panic!("the client closed before {what}");
        }
        std::thread::sleep(POLL);
    }
}

/// The next attach at the daemon (`attaches`, as its listener hands each
/// over). While a pane's attach waits on its gate, the test's clock moves
/// [`GATE_STEP`] a look, so its timer comes due; once none waits, it
/// stands still and only frames are drawn, however long the client and
/// the daemon's threads take: the test's clock never moves with the
/// machine's speed.
pub(crate) fn next_attach<T>(cx: &mut TestAppContext, window: AnyWindowHandle, attaches: &mpsc::Receiver<T>, what: &str) -> T {
    let view = app_of(cx, window);
    loop {
        match attaches.try_recv() {
            Ok(c) => return c,
            Err(mpsc::TryRecvError::Disconnected) => panic!("{what}: the daemon closed"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if view.read_with(cx, |v, _| v.panes.iter().any(|p| p.attach_wait.is_some())) {
            cx.executor().advance_clock(GATE_STEP);
        }
        frame(cx, window);
        std::thread::sleep(POLL);
    }
}

/// The app's view in `window`: the Base root's content, the app's shell
/// (`beat::open_window`).
fn app_of(cx: &mut TestAppContext, window: AnyWindowHandle) -> Entity<AppView> {
    cx.update_window(window, |root, _, cx| {
        let root = root.downcast::<gpui_kit::base::Root>().unwrap_or_else(|_| panic!("a Base root"));
        let shell = root.read(cx).view().clone().downcast::<crate::beat::Shell>().unwrap_or_else(|_| panic!("the app's shell"));
        shell.read(cx).app()
    })
    .expect("the window")
}

/// One frame drawn, the executor run until parked on both sides (as
/// `split_tests`' own `frame`).
fn frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}
