//! Settings, drawn (settings-overhaul PLAN §4, PR 1). THIS SESSION's rows
//! say what the session says, each value at `session-<key>-value <value>`,
//! as the right panel's cells are named (`right-cache-expires <says>`).

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext};
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

/// The dash's config, writable, for this daemon: every link of the gate
/// proven (as `views.rs`'s `proven`), `compaction_model` unset. Settings
/// open first: a closed one forgets the config.
fn writable_dash(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.model.daemon = Some(crate::model::Daemon { version: "0.13.0".into(), profile: None, pid: 7 });
        v.model.conn = crate::model::ConnState::Connected("0.13.0".into());
        let target = crate::dash::Target::parse("http://127.0.0.1:7801/?token=ab").expect("a dash URL");
        v.dash.local = Some(Ok(()));
        v.dash.found = Some(Ok((target, crate::dash::Source::UrlFile("/run/synaps-dash.url".into()))));
        v.dash.serves = Some(Ok(crate::dash::Info { pid: 7, profile: None }));
        v.dash.answer = crate::views::Answer::Up;
        v.dash.config = Some(serde_json::from_value(json!({"writable": true, "values": {"compaction_model": null}})).expect("a snapshot"));
        assert_eq!(v.config_gate(), Ok(()), "the editor is live");
        v.views_changed(cx);
    });
}

/// Type `typed` into Compaction model's editor and press Enter.
fn commit_compaction(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, typed: &str) {
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.settings_edit(crate::settings_view::Edit::Config("compaction_model"), window, cx);
            v.settings_input.update(cx, |s, cx| s.set_value(typed.to_string(), window, cx));
            v.settings_commit(cx);
        })
    })
    .unwrap();
    settle(cx, w, 8);
}

/// Compaction model's help is the CLI's: `match`, `provider-small`, `auto`
/// and `default` mean something to SynapsCLI. The dash-backed editor beside
/// it takes provider/model or empty (unset) only, as the dash's validator
/// does, so the row says where those words can be set: the config file or
/// the TUI (Shady #107 P3-1).
#[gpui_kit::test]
fn compaction_editor_says_which_values_it_takes(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    settings(cx, w, &view, "compaction model");
    writable_dash(cx, &view);
    settle(cx, w, 8);
    let help = crate::settings::def("compaction_model").expect("the row").help;
    assert!(help.contains("match") && help.contains("provider-small"), "the catalog is the CLI's");
    assert!(drawn(cx, w, "config-note-compaction_model"), "the editor says what it takes, and where the rest is set");
    let write = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.settings.writes.get("compaction_model").cloned());
    for word in ["match", "provider-small", "auto", "default"] {
        commit_compaction(cx, w, &view, word);
        let refused = format!("compaction_model: expected provider/model, got \"{word}\"");
        assert_eq!(write(cx), Some(crate::settings_view::Write::Refused(refused)), "{word}: refused here, not sent");
        assert!(drawn(cx, w, "config-note-compaction_model"), "{word}: the note stays beside the refusal");
    }
    // Empty unsets. Another write is out, so this one waits its turn (no
    // request leaves the test).
    view.update(cx, |v, _| assert!(v.dash.order.write("api_retries", None).is_some()));
    commit_compaction(cx, w, &view, "");
    assert_eq!(crate::settings::to_dash("compaction_model", ""), None);
    assert_eq!(write(cx), Some(crate::settings_view::Write::Sending("default".into())), "empty: an unset, on its way");
}
