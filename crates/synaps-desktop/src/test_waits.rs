//! Waits for the view tests that run a client on a real socket
//! (`split_tests`): on the event itself, or on the executor the test
//! drives, never against a short wall-clock deadline. A loaded runner
//! takes longer; it does not fail for it. On ubuntu CI, where dozens of UI
//! tests ran past 60 s, two split tests gave up on their 10 s waits
//! ("timed out waiting for A attached", #71) or clicked a card that had
//! armed in the wall time a frame took (#69).
//!
//! A channel closed under a wait (its client or daemon gone) fails at
//! once. A wait whose event never comes on a live channel fails at its
//! [`Backstop`]: generous wall time ([`BACKSTOP`], or
//! `SYNAPS_TEST_WAIT_SECS`) that a slow run that is right never reaches,
//! and that turns a hang into a failure that names the wait and what it
//! last saw (Shady #74 P1).

use std::sync::mpsc;
use std::time::{Duration, Instant};

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

/// A wait's wall-clock backstop: 30 times the 10 s deadline these waits
/// had, which a loaded ubuntu runner went past once. Not a success
/// deadline: only a wait that would otherwise hang reaches it.
pub(crate) const BACKSTOP: Duration = Duration::from_secs(300);

/// The backstop for a wait starting now: `SYNAPS_TEST_WAIT_SECS` (whole
/// seconds) when set, [`BACKSTOP`] otherwise.
fn budget() -> Duration {
    std::env::var("SYNAPS_TEST_WAIT_SECS").ok().and_then(|s| s.trim().parse().ok()).map_or(BACKSTOP, Duration::from_secs)
}

/// One wait's wall-clock backstop: [`Backstop::check`] fails the test once
/// the wait has run its budget.
pub(crate) struct Backstop {
    from: Instant,
    budget: Duration,
}

impl Backstop {
    /// A wait starting now, on [`budget`].
    pub(crate) fn start() -> Self {
        Self::of(budget())
    }

    fn of(budget: Duration) -> Self {
        Backstop { from: Instant::now(), budget }
    }

    /// Fails the test once the budget is spent: `what` the wait waited
    /// for, `state` what it last saw.
    #[track_caller]
    pub(crate) fn check(&self, what: &str, state: impl FnOnce() -> String) {
        if self.from.elapsed() >= self.budget {
            panic!("{what}: still waiting after {:?}, {} (test_waits' backstop; SYNAPS_TEST_WAIT_SECS sets it)", self.budget, state());
        }
    }
}

/// The client's events, each as it comes, until one matches `done`.
pub(crate) fn until<T: std::fmt::Debug>(events: &async_channel::Receiver<T>, what: &str, done: impl Fn(&T) -> bool) {
    until_within(events, what, done, budget());
}

fn until_within<T: std::fmt::Debug>(events: &async_channel::Receiver<T>, what: &str, done: impl Fn(&T) -> bool, budget: Duration) {
    let stop = Backstop::of(budget);
    let (mut seen, mut last) = (0_usize, None::<String>);
    loop {
        match events.try_recv() {
            Ok(ev) => {
                if done(&ev) {
                    return;
                }
                seen += 1;
                last = Some(format!("{ev:?}"));
            }
            Err(async_channel::TryRecvError::Closed) => panic!("the client closed before {what}"),
            Err(async_channel::TryRecvError::Empty) => {
                stop.check(what, || format!("{seen} other events, the last {last:?}"));
                std::thread::sleep(POLL);
            }
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
    let stop = Backstop::start();
    let (mut frames, mut applied) = (0_u64, 0_usize);
    loop {
        let batch: Vec<Event> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        if !batch.is_empty() {
            applied += batch.len();
            view.update(cx, |v, cx| v.apply_batch(batch, cx));
        }
        frame(cx, window);
        frames += 1;
        if view.read_with(cx, |v, _| done(v)) {
            return;
        }
        if events.is_closed() && events.is_empty() {
            panic!("the client closed before {what}");
        }
        stop.check(what, || format!("{applied} events applied, {frames} frames drawn"));
        std::thread::sleep(POLL);
    }
}

/// The next attach at the daemon (`attaches`, as its listener hands each
/// over). While a pane's attach waits on its gate, the test's clock moves
/// [`GATE_STEP`] a look, so its timer comes due; once none waits, it
/// stands still and only frames are drawn: the test's clock does not
/// move while the wait is only on the socket (the client's and the
/// daemon's threads). The gate's own readiness still reads wall time
/// (a follow-up).
pub(crate) fn next_attach<T>(cx: &mut TestAppContext, window: AnyWindowHandle, attaches: &mpsc::Receiver<T>, what: &str) -> T {
    let view = app_of(cx, window);
    let stop = Backstop::start();
    let (mut frames, mut gated) = (0_u64, 0_u32);
    loop {
        match attaches.try_recv() {
            Ok(c) => return c,
            Err(mpsc::TryRecvError::Disconnected) => panic!("{what}: the daemon closed"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if view.read_with(cx, |v, _| v.panes.iter().any(|p| p.attach_wait.is_some())) {
            cx.executor().advance_clock(GATE_STEP);
            gated += 1;
        }
        frame(cx, window);
        frames += 1;
        stop.check(what, || {
            format!("no attach at the daemon, {frames} frames drawn, the test's clock moved {:?} for a gate", GATE_STEP * gated)
        });
        std::thread::sleep(POLL);
    }
}

/// Why [`recv`] came back empty; its `Debug` is what a test's `.expect`
/// prints after its own message.
pub(crate) enum Gone {
    /// Every sender dropped.
    Closed,
    /// Nothing came within the backstop.
    Waited(Duration),
}

impl std::fmt::Debug for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gone::Closed => f.write_str("its sender closed"),
            Gone::Waited(d) => write!(f, "nothing came in {d:?} (test_waits' backstop; SYNAPS_TEST_WAIT_SECS sets it)"),
        }
    }
}

/// `rx.recv()` with the [`Backstop`]: the next one (the daemon's next
/// attach, a test thread's signal), or why there is none.
pub(crate) fn recv<T>(rx: &mpsc::Receiver<T>) -> Result<T, Gone> {
    recv_within(rx, budget())
}

fn recv_within<T>(rx: &mpsc::Receiver<T>, budget: Duration) -> Result<T, Gone> {
    rx.recv_timeout(budget).map_err(|e| match e {
        mpsc::RecvTimeoutError::Timeout => Gone::Waited(budget),
        mpsc::RecvTimeoutError::Disconnected => Gone::Closed,
    })
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

/// The backstop itself, on short budgets (Shady #74 P1): a closed channel
/// fails at once, a live one that never brings the event fails named at
/// its budget, and a wait that gets its event returns.
#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    /// The panic `f` ended in, its message, and how long it ran.
    fn failure(f: impl FnOnce()) -> (String, Duration) {
        let at = Instant::now();
        let err = catch_unwind(AssertUnwindSafe(f)).expect_err("the wait fails");
        let said = err.downcast_ref::<String>().cloned().or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()));
        (said.expect("a message"), at.elapsed())
    }

    #[test]
    fn until_fails_at_once_on_a_closed_channel_and_names_its_wait() {
        let (tx, rx) = async_channel::unbounded::<u32>();
        tx.send_blocking(1).expect("open");
        drop(tx);
        let (said, took) = failure(|| until_within(&rx, "two", |n| *n == 2, Duration::from_secs(3600)));
        assert_eq!(said, "the client closed before two");
        assert!(took < Duration::from_secs(10), "at once, not at the backstop: {took:?}");
    }

    #[test]
    fn until_on_a_live_channel_fails_at_its_backstop_with_what_it_saw() {
        let (tx, rx) = async_channel::unbounded::<u32>();
        tx.send_blocking(1).expect("open");
        let budget = Duration::from_millis(60);
        let (said, took) = failure(|| until_within(&rx, "two", |n| *n == 2, budget));
        assert!(said.starts_with("two: still waiting after 60ms, 1 other events, the last Some(\"1\")"), "{said}");
        assert!(took >= budget, "not before its budget: {took:?}");
        drop(tx);
    }

    #[test]
    fn until_returns_on_the_event() {
        let (tx, rx) = async_channel::unbounded::<u32>();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            tx.send_blocking(1).and_then(|()| tx.send_blocking(2)).expect("open");
            tx
        });
        until_within(&rx, "two", |n| *n == 2, Duration::from_secs(3600));
        drop(sender.join().expect("the sender"));
    }

    #[test]
    fn recv_says_closed_or_waited_and_returns_what_comes() {
        let (tx, rx) = mpsc::channel::<u32>();
        let at = Instant::now();
        let waited = recv_within(&rx, Duration::from_millis(40)).expect_err("nothing sent");
        assert!(at.elapsed() >= Duration::from_millis(40));
        assert_eq!(format!("{waited:?}"), "nothing came in 40ms (test_waits' backstop; SYNAPS_TEST_WAIT_SECS sets it)");
        tx.send(7).expect("open");
        assert_eq!(recv_within(&rx, Duration::from_secs(3600)).ok(), Some(7));
        drop(tx);
        let at = Instant::now();
        assert_eq!(format!("{:?}", recv_within(&rx, Duration::from_secs(3600)).expect_err("closed")), "its sender closed");
        assert!(at.elapsed() < Duration::from_secs(10), "at once");
    }

    #[test]
    fn a_backstop_fails_only_once_its_budget_is_spent() {
        Backstop::of(Duration::from_secs(3600)).check("never", || unreachable!("not spent"));
        let (said, _) = failure(|| Backstop::of(Duration::ZERO).check("the draft", || "nothing back".into()));
        assert_eq!(said, "the draft: still waiting after 0ns, nothing back (test_waits' backstop; SYNAPS_TEST_WAIT_SECS sets it)");
    }
}
