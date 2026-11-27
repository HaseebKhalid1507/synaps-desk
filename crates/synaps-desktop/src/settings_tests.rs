//! Settings, drawn (settings-overhaul PLAN §4, PR 1). THIS SESSION's rows
//! say what the session says, each value at `session-<key>-value <value>`,
//! as the right panel's cells are named (`right-cache-expires <says>`).

use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext};
use serde_json::json;

use super::answer_tests::heard;
use super::{open_at, settle};
use crate::app::AppView;

/// Settings over the demo session, its rows filtered by `find`.
fn settings(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, find: &str) {
    view.update(cx, |v, cx| {
        v.open_view(crate::tabs::Kind::Settings, cx);
        v.settings_find(find.into(), cx);
    });
    settle(cx, w, 8);
}

fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: &str) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel.to_string())).is_some()
}

/// Cache keep-warm shows the session's own mode, from its `Status`
/// (`keepwarm.mode`, the effective one), which a 0.13.0 daemon is asked
/// for: `—` until it answers, the pin never in it (another setting).
#[gpui_kit::test]
fn keep_warm_row_shows_the_sessions_mode(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    let seq = view.update(cx, |v, cx| {
        v.model.cache_expiry = true;
        cx.notify();
        v.model.lives[v.panes.cur.live].attachment().expect("the demo session attached")
    });
    settings(cx, w, &view, "keep-warm");
    assert!(drawn(cx, w, "session-keepwarm-value —"), "no status yet: unknown");
    let status =
        |mode: &str| synaps_client::Event::SessionStatus { attach_seq: seq, value: json!({"keepwarm": {"mode": mode}, "pinned": true}) };
    view.update(cx, |v, cx| v.apply_batch(vec![status("4h")], cx));
    settle(cx, w, 8);
    assert!(drawn(cx, w, "session-keepwarm-value 4h"), "the session's mode, as its status says");
    assert!(!drawn(cx, w, "session-keepwarm-value 4h · pinned") && !drawn(cx, w, "session-keepwarm-value —"));
    view.update(cx, |v, cx| v.apply_batch(vec![status("off")], cx));
    settle(cx, w, 8);
    assert!(drawn(cx, w, "session-keepwarm-value off") && !drawn(cx, w, "session-keepwarm-value 4h"), "its next status");
}

/// Bash max timeout is a THIS SESSION row: the session's value, from its
/// `RuntimeView` (here a `setting_changed`'s), beside Bash timeout's.
#[gpui_kit::test]
fn session_rows_show_bash_max_timeout(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    settings(cx, w, &view, "bash");
    heard(
        cx,
        w,
        &view,
        vec![json!({"ev": "setting_changed", "applied": {"id": 1, "setting": "bash_timeout", "ok": true, "message": null,
            "view": {"model": "anthropic/claude-opus-4-1", "thinking_level": "medium", "bash_timeout": 120, "bash_max_timeout": 600}}})],
    );
    settle(cx, w, 8);
    assert!(drawn(cx, w, "session-bash_timeout-value 120 s"), "Bash timeout, as the view says");
    assert!(drawn(cx, w, "session-bash_max_timeout-value 600 s"), "and Bash max timeout beside it");
}
