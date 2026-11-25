//! The attach gate ([`crate::tabs::AttachGate`]) on the test's clock
//! (`AppView::gate_now`, the executor's, which its timer runs on): a
//! switch within [`crate::tabs::AttachGate::GAP`] of the last waits, and
//! its attach goes [`crate::tabs::AttachGate::GAP`] after it, as the
//! test's clock says, never as the wall's. Wall time goes by between the
//! two switches here, more than the gap: a gate that read it would attach
//! the second at once.

use std::time::Duration;

use gpui_kit::{Entity, TestAppContext};

use super::open_at;
use crate::app::{AppView, Open};
use crate::tabs::{AttachGate, Place};

/// `session` opened in a new tab of the pane, as a rail click does.
fn open_tab(cx: &mut TestAppContext, view: &Entity<AppView>, session: &str) {
    view.update(cx, |v, cx| {
        v.open_tab(Open::Attach { session_id: session.into(), fallback: None, open: false }, Place::New);
        cx.notify();
    });
    cx.run_until_parked();
}

/// The pane's store: whether the last attach it asked for is of `session`
/// (sent, not only shown), and whether an attach waits on the gate.
fn attach(cx: &mut TestAppContext, view: &Entity<AppView>, session: &str) -> (bool, bool) {
    view.read_with(cx, |v, _| {
        let l = &v.model.lives[v.panes.cur.live];
        (l.wanted.is_some() && l.wanted_session() == Some(session), v.panes.cur.attach_wait.is_some())
    })
}

#[gpui_kit::test]
fn a_second_switch_waits_out_the_gap_on_the_tests_clock(cx: &mut TestAppContext) {
    let (_w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    assert!(view.read_with(cx, |v, _| v.client.is_some()), "a client: the gate is on");
    open_tab(cx, &view, crate::demo::IDLE_CPU);
    assert_eq!(attach(cx, &view, crate::demo::IDLE_CPU), (true, false), "a lone switch attaches at once");
    // Twice the gap of wall time; none of the test's.
    std::thread::sleep(AttachGate::GAP * 2);
    open_tab(cx, &view, crate::demo::FLAKY);
    assert_eq!(attach(cx, &view, crate::demo::FLAKY), (false, true), "within the gap on the test's clock: it waits");
    cx.executor().advance_clock(AttachGate::GAP - Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(attach(cx, &view, crate::demo::FLAKY), (false, true), "1 ms short of the gap: still waiting");
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(attach(cx, &view, crate::demo::FLAKY), (true, false), "the gap gone by: attached");
}
