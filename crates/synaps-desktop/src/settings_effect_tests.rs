//! Settings PR 4 (settings-overhaul PLAN §2.1, §4): when a daemon row's
//! change takes effect, and what the app can see of the value in force.
//! A row's tag is its effect word; under it, `saved · takes effect at
//! <event>` after this run's save of the value the file holds, or idle
//! exit's observed `running N s` in warn. Nothing else makes a row wait:
//! not a write, a `Welcome` (another generation, another `boot_id`, none),
//! the dash's `pending`, or a TUI's relaunch. Idle exit's comparison moves
//! only when the dash is read again.
//!
//! The page and the fake dash are the continuity suite's. A `Welcome`
//! reaches the desk as the client maps it: `Status::Connected` (pid,
//! version) and the life an attach carries (`Attached::daemon`, on the
//! session's stats), which is all the desk sees of `boot_id` and
//! `generation`.

use gpui_kit::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::settings_continuity_tests::{Dash, Page, changed, click, daemon_page, drawn, edit};
use super::settle;

const IDLE: &str = "daemon.idle_exit_secs";
const UNLESS: &str = ", unless the starting client sets its own (an option or SYNAPS_DAEMON_IDLE_EXIT_SECS)";
const OVERRIDES: &str = "A daemon a client starts on its own reads this, unless that client passes its own idle-exit option or has SYNAPS_DAEMON_IDLE_EXIT_SECS set. A reload keeps the running daemon's argument. A daemon started by hand (synaps daemon) doesn't read it.";
const NOTE: &str = "on 0.13.0 and later, new sessions use it at once";

/// The 21 dash keys other than idle exit, a value each to write, and the
/// event their save says on 0.13.0.
const UNOBSERVED: [(&str, &str, &str); 21] = [
    ("model", "anthropic/claude-sonnet-4-5", "the next new session"),
    ("thinking", "max", "the next new session"),
    ("context_window", "1m", "the next new session"),
    ("compaction_model", "anthropic/claude-haiku-4-5", "the next new session"),
    ("compaction_mode", "local", "the next new session"),
    ("api_retries", "7", "the next new session"),
    ("refusal_retries", "4", "the next new session"),
    ("subagent_timeout", "900", "the next new session"),
    ("max_tool_output", "131072", "the next new session"),
    ("bash_timeout", "300", "the next new session"),
    ("bash_max_timeout", "1200", "the next new session"),
    ("tools.activation_confirm", "deny", "the next new session"),
    ("context_management.mode", "off", "the next new session"),
    ("cache_ttl", "5m", "the next new session"),
    ("events.auto_turn", "false", "the next new session"),
    ("events.auto_turn_cap", "20", "the next new session"),
    ("startup.extensions_ready_timeout_secs", "60", "the next new session"),
    ("progressive_tool_disclosure", "true", "the next daemon reload"),
    ("daemon.prompt_abandon_secs", "300", "the daemon's next use"),
    ("daemon.parked_evict_secs", "7200", "the daemon's next use"),
    ("startup.quick_start", "off", "the next TUI launch"),
];

/// Settings over a local daemon of `version` (pid 7) whose dash holds
/// `values` and answers `idleExitRunning: running`.
fn effect_page(cx: &mut TestAppContext, name: &str, values: Value, version: &str, running: Value) -> (Page, Dash) {
    let (pg, dash) = daemon_page(cx, &format!("effect-{name}"), values);
    dash.answer("idleExitRunning", running);
    changed(cx, &pg, |v| {
        v.model.daemon.as_mut().expect("connected").version = version.into();
        v.dash.config = Some(dash.snapshot());
    });
    (pg, dash)
}

/// The find box holding `text` alone (whatever it held, the caret moved to
/// its end first), and what it finds drawn.
fn find(cx: &mut TestAppContext, pg: &Page, text: &str) {
    click(cx, pg.w, "settings-find");
    let mut vcx = VisualTestContext::from_window(pg.w, cx);
    vcx.simulate_keystrokes(&format!("end {}", ["backspace"; 64].join(" ")));
    vcx.simulate_input(text);
    settle(cx, pg.w, 8);
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.search.borrow().query().text().to_string()), text, "the find box");
}

/// `key`'s row shown alone (the find box), and its tag.
fn tagged(cx: &mut TestAppContext, pg: &Page, key: &str, word: &str) -> bool {
    find(cx, pg, key);
    drawn(cx, pg.w, &format!("tag {key} = {word}"))
}

/// The words under `key`'s row, all of them, as drawn (` | `-joined).
fn under(cx: &mut TestAppContext, pg: &Page, key: &str, lines: &[&str]) -> bool {
    find(cx, pg, key);
    drawn(cx, pg.w, &format!("effect {key} = {}", lines.join(" | ")))
}

/// A line under `key`'s row is in warn (a wait).
fn warned(cx: &mut TestAppContext, pg: &Page, key: &str) -> bool {
    find(cx, pg, key);
    drawn(cx, pg.w, &format!("warn {key}"))
}

fn running(n: u64) -> String {
    format!("running {n} s · takes effect at the next daemon start{UNLESS}")
}

fn saved(event: &str) -> String {
    format!("saved · takes effect at {event}")
}

/// `POST {key, value}` from the page, its reply landed.
fn write(cx: &mut TestAppContext, pg: &Page, key: &'static str, value: &str) {
    post(cx, pg, key, Some(value));
}

/// [`write`], `None` unsetting.
fn post(cx: &mut TestAppContext, pg: &Page, key: &'static str, value: Option<&str>) {
    pg.view.update(cx, |v, cx| v.config_write(key, value.map(String::from), cx));
    settle(cx, pg.w, 8);
}

/// What the config shown holds for `key`.
fn shows(cx: &mut TestAppContext, pg: &Page, key: &str) -> Option<String> {
    pg.view.read_with(cx, |v, _| v.shown_config().and_then(|c| c.value(key)).map(String::from))
}

/// The dash's config read again, as a read lands.
fn reread(cx: &mut TestAppContext, pg: &Page, dash: &Dash) {
    changed(cx, pg, |v| v.dash.config = Some(dash.snapshot()));
}

/// A `Welcome` from the same daemon process (pid 7, as the dash serves),
/// as the desk takes it: the client's `Status::Connected`, and the life
/// the session on screen was attached in (`boot` empty: none named).
fn welcome(cx: &mut TestAppContext, pg: &Page, version: &str, boot: &str, generation: u64) {
    pg.view.update(cx, |v, cx| {
        let hello = synaps_client::Status::Connected { daemon_version: version.into(), profile: None, pid: 7, features: vec![] };
        let _ = v.model.apply(synaps_client::Event::Status(hello));
        let mut life = synaps_client::DaemonLife::default();
        (life.boot_id, life.generation, life.pid) = (boot.into(), generation, 7);
        let live = v.panes.cur.live;
        v.model.lives[live].stats.daemon = life;
        v.dash_on_welcome(cx);
        v.views_changed(cx);
    });
    settle(cx, pg.w, 8);
}

/// A no-op `POST` (the file already held the value) neither creates nor
/// clears a wait: `progressive_tool_disclosure` already `false` says it
/// was saved and nothing more; idle exit at 600 with the daemon running 30
/// waits before a `POST` of 600, and after it.
#[gpui_kit::test]
fn a_no_op_post_neither_creates_nor_clears_a_wait(cx: &mut TestAppContext) {
    let values = json!({"progressive_tool_disclosure": "false", IDLE: "600"});
    let (pg, dash) = effect_page(cx, "noop", values, "0.13.0", json!(30));
    let ptd = "progressive_tool_disclosure";
    assert!(tagged(cx, &pg, ptd, "daemon reload"), "its effect word");
    assert!(!under(cx, &pg, ptd, &[&saved("the next daemon reload")]), "nothing saved yet");
    find(cx, &pg, ptd);
    click(cx, pg.w, "choice progressive_tool_disclosure off");
    assert_eq!(dash.posts(), vec![json!({"key": ptd, "value": "false"})], "the same value, posted");
    assert!(under(cx, &pg, ptd, &[&saved("the next daemon reload")]), "saved, when it takes effect");
    assert!(!warned(cx, &pg, ptd), "no wait");

    assert!(tagged(cx, &pg, IDLE, "next daemon start"));
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "the running daemon differs: a wait, before");
    edit(cx, pg.w, "field-daemon.idle_exit_secs", "600");
    assert_eq!(dash.posts()[1], json!({"key": IDLE, "value": "600"}), "600 posted again");
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "and after");
}

/// Writing back what an outside edit changed clears a wait only when the
/// value it leaves equals the running one: an unobserved key never waits;
/// idle exit running 600 is in force again at 600; running 30 (say from
/// `SYNAPS_DAEMON_IDLE_EXIT_SECS`) with the file back at 600 still waits.
#[gpui_kit::test]
fn reverting_clears_a_wait_only_when_the_saved_value_equals_the_running_one(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "revert", json!({"api_retries": "3", IDLE: "600"}), "0.13.0", json!(600));
    dash.set("api_retries", json!("5"));
    reread(cx, &pg, &dash);
    assert!(!warned(cx, &pg, "api_retries"), "edited outside: no wait");
    write(cx, &pg, "api_retries", "3");
    assert!(under(cx, &pg, "api_retries", &[&saved("the next new session")]) && !warned(cx, &pg, "api_retries"), "written back: no wait");
    dash.set("api_retries", json!("5"));
    reread(cx, &pg, &dash);
    assert!(!under(cx, &pg, "api_retries", &[&saved("the next new session")]), "edited outside again: not this app's save");

    assert!(under(cx, &pg, IDLE, &["in force"]), "running 600, the file 600");
    dash.set(IDLE, json!("30"));
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, IDLE, &[&running(600)]) && warned(cx, &pg, IDLE), "set to 30 outside: the running 600 waits");
    write(cx, &pg, IDLE, "600");
    assert!(under(cx, &pg, IDLE, &["saved · in force"]) && !warned(cx, &pg, IDLE), "written back to 600: in force");

    dash.answer("idleExitRunning", json!(30));
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, IDLE, &[&running(30)]), "running 30, the file 600");
    dash.set(IDLE, json!("120"));
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, IDLE, &[&running(30)]), "set to 120 outside");
    write(cx, &pg, IDLE, "600");
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "written back to 600: still running 30");
}

/// Each of the 21 keys the daemon doesn't report, written in the current
/// daemon life, says when it takes effect, never that it waits.
#[gpui_kit::test]
fn a_receipt_without_a_loaded_value_never_waits(cx: &mut TestAppContext) {
    let (pg, _dash) = effect_page(cx, "receipt", json!({}), "0.13.0", json!(null));
    welcome(cx, &pg, "0.13.0", "b-1", 1);
    for (key, value, event) in UNOBSERVED {
        write(cx, &pg, key, value);
        assert!(under(cx, &pg, key, &[&saved(event)]), "{key}: saved, when it takes effect");
        assert!(!warned(cx, &pg, key), "{key}: never a wait");
    }
}

/// A `Welcome` with a new generation (a reload) after a `POST`'s reply
/// changes no row's words.
#[gpui_kit::test]
fn a_welcome_delayed_across_a_post_and_a_reload_changes_nothing(cx: &mut TestAppContext) {
    let (pg, _dash) = effect_page(cx, "delayed", json!({"cache_ttl": "1h", IDLE: "600"}), "0.13.0", json!(30));
    welcome(cx, &pg, "0.13.0", "b-1", 1);
    write(cx, &pg, "cache_ttl", "5m");
    let before = (under(cx, &pg, "cache_ttl", &[&saved("the next new session")]), under(cx, &pg, IDLE, &[&running(30)]));
    assert_eq!(before, (true, true), "saved; idle exit waits");
    welcome(cx, &pg, "0.13.0", "b-1", 2);
    let after = (under(cx, &pg, "cache_ttl", &[&saved("the next new session")]), under(cx, &pg, IDLE, &[&running(30)]));
    assert_eq!(after, before, "the reload's Welcome, late: the same words");
    assert!(tagged(cx, &pg, "cache_ttl", "new sessions") && warned(cx, &pg, IDLE) && !warned(cx, &pg, "cache_ttl"));
}

/// Idle exit running 30 with the file at 600 waits, in warn, and its open
/// row says what the running daemon does; equal, it is in force.
#[gpui_kit::test]
fn an_observed_mismatch_waits(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "mismatch", json!({IDLE: "600"}), "0.13.0", json!(30));
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "a wait, in warn");
    click(cx, pg.w, "help daemon.idle_exit_secs");
    let waits = running(30);
    assert!(under(cx, &pg, IDLE, &[&waits, "The running daemon exits after 30 s idle (its --idle-exit).", OVERRIDES]), "open: what runs");
    dash.answer("idleExitRunning", json!(600));
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, IDLE, &["in force", "The running daemon exits after 600 s idle (its --idle-exit).", OVERRIDES]), "equal");
    assert!(!warned(cx, &pg, IDLE), "no wait");
    dash.set(IDLE, Value::Null);
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, IDLE, &[&running(600), "The running daemon exits after 600 s idle (its --idle-exit).", OVERRIDES]), "unset: 10");
    dash.answer("idleExitRunning", json!(10));
    reread(cx, &pg, &dash);
    assert!(
        under(cx, &pg, IDLE, &["in force", "The running daemon exits after 10 s idle (its --idle-exit).", OVERRIDES]),
        "unset, running 10"
    );
}

/// Observed or not, idle exit's open row names what wins over the file:
/// the starting client's option, `SYNAPS_DAEMON_IDLE_EXIT_SECS`, a reload
/// keeping the argument, a daemon started by hand.
#[gpui_kit::test]
fn idle_exit_effect_names_what_overrides_the_file(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "overrides", json!({IDLE: "600"}), "0.13.0", json!(600));
    find(cx, &pg, IDLE);
    click(cx, pg.w, "help daemon.idle_exit_secs");
    let seen = "The running daemon exits after 600 s idle (its --idle-exit).";
    assert!(under(cx, &pg, IDLE, &["in force", seen, OVERRIDES]), "observed");
    dash.answer("idleExitRunning", Value::Null);
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, IDLE, &[OVERRIDES]), "not observed");
    for part in ["passes its own idle-exit option", "SYNAPS_DAEMON_IDLE_EXIT_SECS", "A reload keeps", "started by hand"] {
        assert!(OVERRIDES.contains(part), "{part}");
    }
}

/// `idleExitRunning` null, a string, negative or fractional: idle exit
/// takes effect at the next daemon start, and never waits.
#[gpui_kit::test]
fn idle_exit_unobserved_without_a_whole_number(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "unobserved", json!({IDLE: "120"}), "0.13.0", Value::Null);
    write(cx, &pg, IDLE, "600");
    for running in [Value::Null, json!("30"), json!(-30), json!(30.5)] {
        dash.answer("idleExitRunning", running.clone());
        reread(cx, &pg, &dash);
        assert!(tagged(cx, &pg, IDLE, "next daemon start"), "{running}");
        assert!(under(cx, &pg, IDLE, &[&format!("{}{UNLESS}", saved("the next daemon start"))]), "{running}: when it takes effect");
        assert!(!warned(cx, &pg, IDLE), "{running}: never a wait");
    }
}

/// A cold start (a new `boot_id`) alone moves nothing: the old snapshot
/// keeps its words. The dash read again (the window back in front, Settings
/// showing), the new comparison shows.
#[gpui_kit::test]
fn a_cold_boot_alone_changes_nothing_until_the_dash_is_read(cx: &mut TestAppContext) {
    let values = json!({"progressive_tool_disclosure": "false", IDLE: "600"});
    let (pg, dash) = effect_page(cx, "cold", values, "0.13.0", json!(30));
    welcome(cx, &pg, "0.13.0", "b-1", 1);
    write(cx, &pg, "progressive_tool_disclosure", "true");
    let ptd = saved("the next daemon reload");
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && under(cx, &pg, "progressive_tool_disclosure", &[&ptd]));
    // The new daemon started from the file; the desk hasn't read the dash.
    dash.answer("idleExitRunning", json!(600));
    welcome(cx, &pg, "0.13.0", "b-2", 1);
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "the old snapshot: the same words");
    assert!(under(cx, &pg, "progressive_tool_disclosure", &[&ptd]));
    pg.view.update(cx, |v, cx| {
        let g = v.model.daemon_gen;
        v.dash.found_now(g);
        v.window_activated(false, cx);
        v.window_activated(true, cx);
    });
    settle(cx, pg.w, 8);
    assert!(under(cx, &pg, IDLE, &["in force"]) && !warned(cx, &pg, IDLE), "read again on activation: in force");
    assert!(under(cx, &pg, "progressive_tool_disclosure", &[&ptd]), "the save, the same words");
}

/// The dash's `pending` (it compares the file with what the dash read when
/// it started, and a dash-only reload resets that) moves no row's words.
#[gpui_kit::test]
fn dash_only_reload_changes_nothing(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "dashonly", json!({"progressive_tool_disclosure": "false"}), "0.13.0", Value::Null);
    write(cx, &pg, "progressive_tool_disclosure", "true");
    let ptd = saved("the next daemon reload");
    for pending in [json!(["progressive_tool_disclosure"]), json!([])] {
        dash.answer("pending", pending.clone());
        reread(cx, &pg, &dash);
        assert!(under(cx, &pg, "progressive_tool_disclosure", &[&ptd]), "pending {pending}: the same words");
        assert!(!warned(cx, &pg, "progressive_tool_disclosure"), "pending {pending}: no wait");
    }
}

/// A `Welcome` that names no life (no `boot_id`) moves nothing.
#[gpui_kit::test]
fn unknown_life_changes_nothing(cx: &mut TestAppContext) {
    let (pg, _dash) = effect_page(cx, "nolife", json!({"cache_ttl": "1h", IDLE: "600"}), "0.13.0", json!(30));
    welcome(cx, &pg, "0.13.0", "b-1", 1);
    write(cx, &pg, "cache_ttl", "5m");
    welcome(cx, &pg, "0.13.0", "", 0);
    assert!(under(cx, &pg, "cache_ttl", &[&saved("the next new session")]), "the same words");
    assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "the wait, as it was");
}

/// A TUI reads `startup.quick_start` when it attaches, and nothing tells
/// the desk when one relaunched: the row says when its save takes effect
/// and never waits; a relaunch (the dash read again, the file the same)
/// neither sets idle exit's wait nor clears it.
#[gpui_kit::test]
fn tui_relaunch_never_sets_or_clears_a_wait(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "tui", json!({"startup.quick_start": "on", IDLE: "600"}), "0.13.0", json!(30));
    assert!(tagged(cx, &pg, "startup.quick_start", "next TUI launch"));
    write(cx, &pg, "startup.quick_start", "off");
    let said = saved("the next TUI launch");
    for round in 0..2 {
        assert!(under(cx, &pg, "startup.quick_start", &[&said]) && !warned(cx, &pg, "startup.quick_start"), "{round}: no wait");
        assert!(under(cx, &pg, IDLE, &[&running(30)]) && warned(cx, &pg, IDLE), "{round}: the wait kept");
        reread(cx, &pg, &dash);
    }
}

/// On 0.13.0 and later a new-session key says so, and its save takes
/// effect at the next new session, never a wait.
#[gpui_kit::test]
fn new_session_key_has_no_wait_on_0_13(cx: &mut TestAppContext) {
    for version in ["0.13.0", "0.13.2", "1.0.0"] {
        let (pg, _dash) = effect_page(cx, &format!("ns-{version}"), json!({"context_window": "200k"}), version, Value::Null);
        assert!(tagged(cx, &pg, "context_window", "new sessions"), "{version}");
        write(cx, &pg, "context_window", "1m");
        assert!(under(cx, &pg, "context_window", &[&saved("the next new session")]), "{version}");
        assert!(!warned(cx, &pg, "context_window"), "{version}: no wait");
    }
}

/// Before 0.13.0 (a pre-release of it too) a new-session key takes effect
/// at a reload, and never waits for one.
#[gpui_kit::test]
fn new_session_key_says_reload_before_0_13_and_never_waits(cx: &mut TestAppContext) {
    for version in ["0.12.9", "0.13.0-rc.1", "0.0.0-fake"] {
        let (pg, _dash) = effect_page(cx, &format!("old-{version}"), json!({"bash_timeout": "120"}), version, Value::Null);
        assert!(tagged(cx, &pg, "bash_timeout", "daemon reload"), "{version}");
        write(cx, &pg, "bash_timeout", "300");
        assert!(under(cx, &pg, "bash_timeout", &[&saved("the next daemon reload")]), "{version}");
        assert!(!warned(cx, &pg, "bash_timeout"), "{version}: no wait");
        click(cx, pg.w, "help bash_timeout");
        assert!(under(cx, &pg, "bash_timeout", &[&saved("the next daemon reload")]), "{version}: open, no 0.13.0 note");
    }
}

/// `context_window` edited, then a new session on the same daemon life
/// (its view holding the new value): the row's words don't move: the
/// session's view is its own, not what the daemon loaded.
#[gpui_kit::test]
fn same_boot_new_session_after_editing_context_window(cx: &mut TestAppContext) {
    let (pg, _dash) = effect_page(cx, "sameboot", json!({"context_window": "200k"}), "0.13.0", Value::Null);
    welcome(cx, &pg, "0.13.0", "b-1", 1);
    write(cx, &pg, "context_window", "1m");
    let said = saved("the next new session");
    assert!(under(cx, &pg, "context_window", &[&said]));
    changed(cx, &pg, |v| {
        let live = v.panes.cur.live;
        v.model.lives[live].stats.view.get_or_insert_with(Default::default).context_window = 1_000_000;
    });
    assert!(under(cx, &pg, "context_window", &[&said]), "the same words");
    assert!(!under(cx, &pg, "context_window", &["saved · in force"]) && !warned(cx, &pg, "context_window"), "never in force, never a wait");
}

/// An empty or unreadable `daemon_version`: a new-session key says
/// `daemon reload`, and its open row adds that 0.13.0 and later read it
/// at once.
#[gpui_kit::test]
fn unknown_version_says_reload_with_the_0_13_note(cx: &mut TestAppContext) {
    for version in ["", "x", "0.13", "00.13.0", "0.13.0+", "0.12.9-"] {
        let (pg, _dash) = effect_page(cx, &format!("unknown-{version}"), json!({"cache_ttl": "1h"}), version, Value::Null);
        assert!(tagged(cx, &pg, "cache_ttl", "daemon reload"), "{version:?}");
        click(cx, pg.w, "help cache_ttl");
        assert!(under(cx, &pg, "cache_ttl", &[NOTE]), "{version:?}: open, the note");
        write(cx, &pg, "cache_ttl", "5m");
        assert!(under(cx, &pg, "cache_ttl", &[&saved("the next daemon reload"), NOTE]), "{version:?}: saved");
        assert!(!warned(cx, &pg, "cache_ttl"), "{version:?}: no wait");
    }
}

/// A `POST` is this app's save only if its reply holds what it sent. The
/// dash's lock covers its write, not the read of the file its reply is
/// (`main.ts` `handleConfigPost`): another writer's value can land
/// between. That value shows, never as this app's save; so for an unset
/// sent, or found. A refusal saves nothing (Shady #112 P2-1).
#[gpui_kit::test]
fn a_post_is_a_save_only_if_its_reply_holds_what_it_sent(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "race", json!({"api_retries": "1"}), "0.13.0", Value::Null);
    let said = saved("the next new session");
    dash.during_post("api_retries", json!("5"));
    write(cx, &pg, "api_retries", "3");
    assert_eq!(dash.posts().last(), Some(&json!({"key": "api_retries", "value": "3"})), "3 sent");
    assert_eq!(shows(cx, &pg, "api_retries").as_deref(), Some("5"), "the reply's 5 shows");
    assert!(!under(cx, &pg, "api_retries", &[&said]), "3 sent, 5 replied: not this app's save");
    dash.set("api_retries", json!("3"));
    reread(cx, &pg, &dash);
    assert!(!under(cx, &pg, "api_retries", &[&said]), "3 there later, by another hand: still not this app's");
    dash.during_post("api_retries", json!("5"));
    post(cx, &pg, "api_retries", None);
    assert!(!under(cx, &pg, "api_retries", &[&said]), "unset sent, 5 replied: not this app's save");
    dash.during_post("api_retries", Value::Null);
    write(cx, &pg, "api_retries", "3");
    assert_eq!(shows(cx, &pg, "api_retries"), None, "the reply has it unset");
    assert!(!under(cx, &pg, "api_retries", &[&said]), "3 sent, unset replied: not this app's save");
    write(cx, &pg, "api_retries", "4");
    assert!(under(cx, &pg, "api_retries", &[&said]), "a reply holding what was sent: saved");
    post(cx, &pg, "api_retries", None);
    assert!(under(cx, &pg, "api_retries", &[&said]), "an unset that stuck: saved");
    dash.during_post("api_retries", json!("6"));
    write(cx, &pg, "api_retries", "8");
    assert!(!under(cx, &pg, "api_retries", &[&said]), "8 sent, 6 replied: the earlier save never becomes 6's");
    dash.refuse("refusal_retries: not a number");
    write(cx, &pg, "refusal_retries", "2");
    find(cx, &pg, "refusal_retries");
    assert!(drawn(cx, pg.w, "refused refusal_retries = refusal_retries: not a number"), "refused");
    assert!(!under(cx, &pg, "refusal_retries", &[&said]), "a refusal saves nothing");
}

/// The record of this run's saves keeps the last 256 (whose config, key)
/// pairs, a digest of each value, not the value (Shady #112 P2-2): 300
/// saves of 1 KiB values to 300 files keep 256, the first 44 gone with
/// their `saved ·`, the rest there; a save of a pair held replaces it.
#[gpui_kit::test]
fn the_record_of_saves_is_bounded(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "cap", json!({}), "0.13.0", Value::Null);
    let said = saved("the next new session");
    let value = |i: usize| format!("{}-{i}", "m".repeat(1024));
    let file = |i: usize| json!(format!("/home/u/{i}/.synaps-cli/config"));
    let kept = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.settings.wrote.len());
    for i in 0..300 {
        dash.answer("writePath", file(i));
        pg.view.update(cx, |v, cx| v.config_write("model", Some(value(i)), cx));
        cx.run_until_parked();
        assert_eq!(dash.posts().len(), i + 1, "save {i} sent");
    }
    assert_eq!(kept(cx), 256, "the last 256");
    for i in [299, 299, 100] {
        dash.answer("writePath", file(i));
        write(cx, &pg, "model", &value(i));
    }
    assert_eq!(kept(cx), 256, "a pair held again: replaced");
    let saved_in = |cx: &mut TestAppContext, i: usize| {
        dash.answer("writePath", file(i));
        dash.set("model", json!(value(i)));
        reread(cx, &pg, &dash);
        assert_eq!(shows(cx, &pg, "model"), Some(value(i)), "file {i} read");
        under(cx, &pg, "model", &[&said])
    };
    for i in [0, 43] {
        assert!(!saved_in(cx, i), "save {i}: gone, its line with it");
    }
    for i in [44, 100, 299] {
        assert!(saved_in(cx, i), "save {i}: kept");
    }
}

/// A save is the config it went to's: the dash's socket, the daemon's
/// profile and the file. The same value in another's is no save of this
/// app's; another pid or life of the same daemon keeps it (Shady #112
/// P2-2).
#[gpui_kit::test]
fn a_save_is_its_daemons_and_files_whatever_the_pid(cx: &mut TestAppContext) {
    let (pg, dash) = effect_page(cx, "owner", json!({"api_retries": "1"}), "0.13.0", Value::Null);
    let said = saved("the next new session");
    dash.answer("writePath", json!("/a/config"));
    write(cx, &pg, "api_retries", "3");
    assert!(under(cx, &pg, "api_retries", &[&said]), "saved");
    dash.answer("writePath", json!("/b/config"));
    reread(cx, &pg, &dash);
    assert!(!under(cx, &pg, "api_retries", &[&said]), "another file holding 3");
    dash.answer("writePath", json!("/a/config"));
    reread(cx, &pg, &dash);
    assert!(under(cx, &pg, "api_retries", &[&said]), "its file again");
    let socket = pg.view.read_with(cx, |v, _| v.dash.socket.clone());
    changed(cx, &pg, |v| v.dash.socket = Some("/run/user/1000/other.sock".into()));
    assert!(!under(cx, &pg, "api_retries", &[&said]), "another daemon's socket");
    changed(cx, &pg, |v| v.dash.socket = socket);
    assert!(under(cx, &pg, "api_retries", &[&said]), "its socket again");
    let serving = |pid: u32, profile: Option<&'static str>| {
        move |v: &mut crate::app::AppView| {
            let d = v.model.daemon.as_mut().expect("connected");
            (d.pid, d.profile) = (pid, profile.map(String::from));
            v.dash.serves = Some(Ok(crate::dash::Info { pid, profile: profile.map(String::from) }));
        }
    };
    changed(cx, &pg, serving(7, Some("work")));
    assert_eq!(shows(cx, &pg, "api_retries").as_deref(), Some("3"), "the work profile's config shown");
    assert!(!under(cx, &pg, "api_retries", &[&said]), "another profile");
    changed(cx, &pg, serving(8, None));
    assert_eq!(shows(cx, &pg, "api_retries").as_deref(), Some("3"));
    assert!(under(cx, &pg, "api_retries", &[&said]), "another pid of the same daemon: kept");
    welcome(cx, &pg, "0.13.0", "b-2", 2);
    changed(cx, &pg, serving(8, None));
    assert!(under(cx, &pg, "api_retries", &[&said]), "another life: kept");
}

/// Settings on screen in a pane that isn't focused is read again as the
/// window comes back to the front (Shady #112 P2-3). Folded out of sight,
/// or open behind a session tab, it isn't. A read under way takes the
/// activation's on; a find under way does, with no find started again.
#[gpui_kit::test]
fn settings_in_a_pane_not_focused_reads_again_on_activation(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "panes", json!({IDLE: "600"}), "0.13.0", json!(30));
    find(cx, &pg, IDLE);
    assert!(under(cx, &pg, IDLE, &[&running(30)]));
    pg.view.update(cx, |v, _| v.app_settings.split = true);
    {
        use gpui_kit::AppContext as _;
        cx.update_window(pg.w, |_, window, cx| pg.view.update(cx, |v, cx| v.split_pane(None, window, cx))).expect("the window");
    }
    settle(cx, pg.w, 8);
    let left = pg.view.read_with(cx, |v, _| {
        assert_eq!(v.panes.len(), 2, "split");
        assert_eq!(v.model.tabs.view(), None, "the new pane, focused, on the session");
        let left = v.panes.rest.iter().find(|p| p.parked.tabs.view() == Some(Kind::Settings)).expect("Settings, in the other pane");
        assert!(v.panes.shown_split(), "both on screen");
        left.id
    });
    // Found (a synthetic view never finds: the read its opening asked for
    // still waits).
    let activate = |cx: &mut TestAppContext, before: fn(&mut crate::app::AppView)| {
        pg.view.update(cx, |v, cx| {
            let g = v.model.daemon_gen;
            v.dash.found_now(g);
            v.dash.under_way(false, false);
            v.window_activated(false, cx);
            before(v);
            v.window_activated(true, cx);
        });
        settle(cx, pg.w, 8);
    };
    let reads = dash.reads();
    dash.answer("idleExitRunning", json!(600));
    activate(cx, |_| {});
    assert_eq!(dash.reads(), reads + 1, "read again");
    let running = pg.view.read_with(cx, |v, _| v.shown_config().map(|c| c.idle_exit_running));
    assert_eq!(running, Some(Some(600)), "the running value read");
    assert!(drawn(cx, pg.w, "effect daemon.idle_exit_secs = in force"), "in force, in the pane not focused");
    // Folded: the focused pane alone on screen.
    activate(cx, |v| v.panes.folded = true);
    assert_eq!(dash.reads(), reads + 1, "folded out of sight: no read");
    // A read under way takes it on; a find under way does, none started.
    activate(cx, |v| v.dash.under_way(true, false));
    let (wants, finds) = pg.view.read_with(cx, |v, _| (v.dash.wants_config(), v.dash.finds));
    assert!(wants && dash.reads() == reads + 1, "a read under way: one after it, none beside it");
    pg.view.update(cx, |v, _| v.dash.under_way(false, false));
    activate(cx, |v| v.dash.under_way(false, true));
    let (wants, finds_then) = pg.view.read_with(cx, |v, _| (v.dash.wants_config(), v.dash.finds));
    assert!(wants && finds_then == finds && dash.reads() == reads + 1, "a find under way: it reads as it ends; no find again");
    pg.view.update(cx, |v, _| v.dash.under_way(false, false));
    // Open behind the session tab in the other pane: not on screen.
    pg.view.update(cx, |v, _| {
        v.in_pane(left, |v| {
            let i = v.model.tabs.iter().position(|t| t.kind == Kind::Session).expect("its session tab");
            v.model.tabs.focus(i);
        });
    });
    settle(cx, pg.w, 8);
    activate(cx, |_| {});
    assert_eq!(dash.reads(), reads + 1, "Settings open, not shown: no read");
}

/// A split beside `kind`'s pane, the new pane focused on the session:
/// (`kind`'s pane, the new one).
fn split_beside(cx: &mut TestAppContext, pg: &Page, kind: crate::tabs::Kind) -> (crate::pane::PaneId, crate::pane::PaneId) {
    pg.view.update(cx, |v, _| v.app_settings.split = true);
    {
        use gpui_kit::AppContext as _;
        cx.update_window(pg.w, |_, window, cx| pg.view.update(cx, |v, cx| v.split_pane(None, window, cx))).expect("the window");
    }
    settle(cx, pg.w, 8);
    pg.view.read_with(cx, |v, _| {
        let at = v.panes.rest.iter().find(|p| p.parked.tabs.view() == Some(kind)).expect("the view, in the other pane");
        assert!(v.panes.shown_split() && v.model.tabs.view().is_none(), "the new pane, focused, on the session");
        (at.id, v.panes.cur.id)
    })
}

fn focus(cx: &mut TestAppContext, pg: &Page, id: crate::pane::PaneId) {
    pg.view.update(cx, |v, cx| v.focus_pane(id, crate::pane::KeysTo::Pane, cx));
    settle(cx, pg.w, 8);
}

/// Two panes side by side fold under this width (each 320 at least).
const FOLDS: f32 = 640.;

/// The window resized to `w` wide (900 high), and its frames run.
fn resize(cx: &mut TestAppContext, pg: &Page, w: f32) {
    resize_frame(cx, pg, w);
    settle(cx, pg.w, 8);
}

/// The window resized to `w` wide (900 high): the one frame the resize
/// draws, and what it deferred (the fold is that frame's).
fn resize_frame(cx: &mut TestAppContext, pg: &Page, w: f32) {
    use gpui_kit::{px, size};
    VisualTestContext::from_window(pg.w, cx).simulate_resize(size(px(w), px(900.)));
    assert_eq!(pg.view.read_with(cx, |v, _| v.width), w, "drawn at its new size");
    cx.run_until_parked();
}

fn folded(cx: &mut TestAppContext, pg: &Page) -> bool {
    pg.view.read_with(cx, |v, _| v.panes.split() && !v.panes.shown_split())
}

/// What Settings holds that goes with it: the config, what the dash
/// serves, the reference, the writes; [`hold_settings`] gives it each.
fn settings_held(v: &crate::app::AppView) -> [bool; 4] {
    [v.dash.config.is_some(), v.dash.serves.is_some(), v.settings.reference.is_some(), !v.settings.writes.is_empty()]
}

fn hold_settings(v: &mut crate::app::AppView) {
    v.dash.serves = Some(Err("held".into()));
    v.settings.reference = Some(crate::settings::Reference { path: "held".into(), values: Ok(Default::default()) });
    v.settings.writes.insert(IDLE, crate::settings_view::Write::Refused("held".into()));
}

/// What Usage holds that goes with it: the board, its error, when it was
/// fetched, the hover.
fn usage_held(v: &crate::app::AppView) -> [bool; 4] {
    let u = &v.usage;
    [u.board.is_some(), u.error.is_some(), u.fetched.is_some(), u.hover.is_some()]
}

/// Settings in a split keeps its config as the session's pane beside it
/// takes the focus: it is still on screen (Shady #112 round 2: the focused
/// pane's tabs alone decided). Open behind a tab in a pane on screen, it
/// keeps it too; folded out of sight, it goes.
#[gpui_kit::test]
fn settings_in_a_split_keeps_its_config_as_the_session_pane_takes_focus(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "split-kept", json!({IDLE: "600"}), "0.13.0", json!(600));
    find(cx, &pg, IDLE);
    let in_force = "effect daemon.idle_exit_secs = in force";
    assert!(drawn(cx, pg.w, in_force));
    let (left, right) = split_beside(cx, &pg, Kind::Settings);
    let reads = dash.reads();
    let config = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.shown_config().map(|c| c.idle_exit_running));
    assert_eq!(config(cx), Some(Some(600)), "split: the session's pane focused, Settings keeps its config");
    assert!(drawn(cx, pg.w, in_force), "and draws it, in the pane not focused");
    focus(cx, &pg, left);
    focus(cx, &pg, right);
    assert_eq!(pg.view.read_with(cx, |v, _| v.model.tabs.view()), None, "the session's pane focused again");
    assert_eq!(config(cx), Some(Some(600)), "focused away again: kept");
    assert!(drawn(cx, pg.w, in_force), "drawn");
    assert_eq!(dash.reads(), reads, "kept, not read again");
    // Open behind its pane's session tab, that pane on screen: kept, as
    // the focused pane's always was.
    pg.view.update(cx, |v, _| {
        v.in_pane(left, |v| {
            let i = v.model.tabs.iter().position(|t| t.kind == Kind::Session).expect("its session tab");
            v.model.tabs.focus(i);
        });
    });
    settle(cx, pg.w, 8);
    focus(cx, &pg, left);
    focus(cx, &pg, right);
    assert_eq!(config(cx), Some(Some(600)), "open behind a tab, in a pane on screen: kept");
    // A resize folds the split: the session's pane alone on screen,
    // Settings out of sight; all it held goes, focus and tabs untouched
    // (Shady #113 P2-1: the focused pane's tabs alone decided when).
    pg.view.update(cx, |v, _| hold_settings(v));
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [true; 4]);
    resize_frame(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (right, None), "the session's pane, focused");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "folded out of sight, its first frame: all Settings held goes");
}

/// Usage in a split keeps its board as the session's pane beside it takes
/// the focus, as Settings keeps its config; folded out of sight, it goes.
#[gpui_kit::test]
fn usage_in_a_split_keeps_its_board_as_the_session_pane_takes_focus(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let pg = super::settings_continuity_tests::window(cx, "usage-split", (1440., 900.));
    pg.view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    settle(cx, pg.w, 8);
    let board = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.usage.board.clone());
    let first = board(cx).expect("the demo's board, read as Usage opened");
    // Fetched now (a synthetic load never says when): fresh.
    pg.view.update(cx, |v, _| v.usage.fetched = Some(std::time::Instant::now()));
    let loads = pg.view.read_with(cx, |v, _| v.usage.loads);
    let (left, right) = split_beside(cx, &pg, Kind::Usage);
    assert!(board(cx).is_some_and(|b| std::sync::Arc::ptr_eq(&b, &first)), "split: the session's pane focused, Usage keeps its board");
    focus(cx, &pg, left);
    focus(cx, &pg, right);
    assert_eq!(pg.view.read_with(cx, |v, _| v.model.tabs.view()), None, "the session's pane focused again");
    assert!(board(cx).is_some_and(|b| std::sync::Arc::ptr_eq(&b, &first)), "focused away again: kept");
    assert_eq!(pg.view.read_with(cx, |v, _| v.usage.loads), loads, "fresh: never loaded again");
    // A resize folds the split: all Usage held goes.
    pg.view.update(cx, |v, _| {
        v.usage.error = Some("held".into());
        v.usage.hover = Some((crate::usage_view::Chart::Spend, 0));
    });
    assert_eq!(pg.view.read_with(cx, |v, _| usage_held(v)), [true; 4]);
    resize_frame(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (right, None), "the session's pane, focused");
    assert_eq!(pg.view.read_with(cx, |v, _| usage_held(v)), [false; 4], "folded out of sight, its first frame: all Usage held goes");
}

/// Settings let go as a fold put it out of sight, then shown again as the
/// window widens, its pane not focused: its config is read, once, and
/// drawn; no focus, no activation (Shady #113 P2-2).
#[gpui_kit::test]
fn settings_shown_again_by_an_unfold_reads_its_config(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "unfold", json!({IDLE: "600"}), "0.13.0", json!(600));
    find(cx, &pg, IDLE);
    let in_force = "effect daemon.idle_exit_secs = in force";
    let (left, right) = split_beside(cx, &pg, Kind::Settings);
    resize(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    focus(cx, &pg, left);
    focus(cx, &pg, right);
    assert!(pg.view.read_with(cx, |v, _| v.dash.config.is_none()), "folded, the session's pane focused: Settings' config gone");
    // Found (a synthetic view never finds).
    pg.view.update(cx, |v, _| {
        let g = v.model.daemon_gen;
        v.dash.found_now(g);
        v.dash.under_way(false, false);
    });
    let (reads, finds) = (dash.reads(), pg.view.read_with(cx, |v, _| v.dash.finds));
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (right, None), "the session's pane, still focused");
    assert_eq!(dash.reads(), reads + 1, "shown again: read, once");
    assert_eq!(pg.view.read_with(cx, |v, _| v.dash.finds), finds, "found: no find");
    let running = pg.view.read_with(cx, |v, _| v.shown_config().map(|c| c.idle_exit_running));
    assert_eq!(running, Some(Some(600)), "its config again");
    assert!(drawn(cx, pg.w, in_force), "drawn, in the pane not focused");
    // Focused away and back: shown all along, nothing read.
    focus(cx, &pg, left);
    focus(cx, &pg, right);
    assert_eq!(dash.reads(), reads + 1, "kept: not read again");
}

/// Usage let go as a fold put it out of sight, then shown again as the
/// window widens, its pane not focused: its board is loaded, once, and
/// drawn (Shady #113 P2-2).
#[gpui_kit::test]
fn usage_shown_again_by_an_unfold_loads_its_board(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let pg = super::settings_continuity_tests::window(cx, "usage-unfold", (1440., 900.));
    pg.view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    settle(cx, pg.w, 8);
    pg.view.update(cx, |v, _| v.usage.fetched = Some(std::time::Instant::now()));
    let (left, right) = split_beside(cx, &pg, Kind::Usage);
    resize(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    focus(cx, &pg, left);
    focus(cx, &pg, right);
    assert!(pg.view.read_with(cx, |v, _| v.usage.board.is_none()), "folded, the session's pane focused: the board gone");
    assert!(!drawn(cx, pg.w, "usage-board"));
    let loads = pg.view.read_with(cx, |v, _| v.usage.loads);
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (right, None), "the session's pane, still focused");
    assert_eq!(pg.view.read_with(cx, |v, _| v.usage.loads), loads + 1, "shown again: loaded, once");
    assert!(pg.view.read_with(cx, |v, _| v.usage.board.is_some()), "its board again");
    assert!(drawn(cx, pg.w, "usage-board"), "drawn, in the pane not focused");
    // Its pane's session tab in front, then Usage again, stale by then, its
    // pane still not focused: loaded, once.
    let front = |cx: &mut TestAppContext, kind: Kind| {
        pg.view.update(cx, |v, cx| {
            v.in_pane(left, |v| {
                let i = v.model.tabs.iter().position(|t| t.kind == kind).expect("the tab");
                v.model.tabs.focus(i);
            });
            cx.notify();
        });
        settle(cx, pg.w, 8);
    };
    pg.view.update(cx, |v, _| v.usage.fetched = Some(std::time::Instant::now()));
    front(cx, Kind::Session);
    let loads = pg.view.read_with(cx, |v, _| v.usage.loads);
    pg.view.update(cx, |v, _| v.usage.fetched = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(3600)));
    front(cx, Kind::Usage);
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.usage.loads)), (right, loads + 1), "in front again, stale: loaded, once");
}

/// The focused pane closes, the one left focused with the same visible
/// view and as many views open (a session tab in front, one view behind
/// it): the closed pane's view goes, all it held; the other's stays
/// (Shady #113 P2-1).
#[gpui_kit::test]
fn closing_a_pane_lets_go_of_its_view_whatever_the_focused_tabs(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, _dash) = effect_page(cx, "close", json!({IDLE: "600"}), "0.13.0", json!(600));
    let (a, b) = split_beside(cx, &pg, Kind::Settings);
    let front = |v: &mut crate::app::AppView| {
        let i = v.model.tabs.iter().position(|t| t.kind == Kind::Session).expect("its session tab");
        v.model.tabs.focus(i);
    };
    // B: Usage behind its session tab; A: Settings behind its own.
    pg.view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    settle(cx, pg.w, 8);
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (b, Some(Kind::Usage)), "Usage, in B");
    pg.view.update(cx, |v, cx| {
        v.usage.fetched = Some(std::time::Instant::now());
        front(v);
        cx.notify();
    });
    settle(cx, pg.w, 8);
    let board = pg.view.read_with(cx, |v, _| v.usage.board.clone()).expect("Usage's board");
    pg.view.update(cx, |v, cx| {
        v.in_pane(a, front);
        cx.notify();
    });
    settle(cx, pg.w, 8);
    focus(cx, &pg, a);
    // What the trigger watched alone (af9a9e8): the focused pane's view and
    // its views open.
    let sig = |v: &crate::app::AppView| (v.model.tabs.view(), v.model.tabs.iter().filter(|t| t.kind != Kind::Session).count());
    let before = pg.view.read_with(cx, |v, _| sig(v));
    assert_eq!(before, (None, 1), "A: a session tab in front, one view behind");
    pg.view.update(cx, |v, _| hold_settings(v));
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [true; 4]);
    // Close pane (the palette's): A's tabs, then A.
    pg.view.update(cx, |v, cx| {
        v.close_pane_tabs();
        cx.notify();
    });
    settle(cx, pg.w, 8);
    pg.view.read_with(cx, |v, _| {
        assert_eq!((v.panes.len(), v.panes.focused), (1, b), "B alone, focused");
        assert_eq!(sig(v), before, "B: as A was");
        assert!(!v.model.tabs.iter().any(|t| t.kind == Kind::Settings), "Settings closed with A");
        assert_eq!(settings_held(v), [false; 4], "all Settings held goes");
        assert!(v.usage.board.as_ref().is_some_and(|b| std::sync::Arc::ptr_eq(b, &board)), "Usage, open in B: its board kept");
    });
}

/// This machine's Synaps folder for a test, its `config` holding `text`:
/// [`crate::views::DashUi::base`].
struct Base(std::path::PathBuf);

impl Base {
    fn new(name: &str, text: &str) -> Base {
        let dir = std::env::temp_dir().join(format!("synaps-desk-base-{name}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the folder");
        std::fs::write(dir.join("config"), text).expect("its config");
        Base(dir)
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The channel found for this daemon, nothing under way (a synthetic view
/// never finds without [`Base`]).
fn found(cx: &mut TestAppContext, pg: &Page) {
    pg.view.update(cx, |v, _| {
        let g = v.model.daemon_gen;
        v.dash.found_now(g);
        v.dash.under_way(false, false);
    });
}

/// The gate up: replies are held as they land.
fn gate(cx: &mut TestAppContext, pg: &Page) {
    pg.view.update(cx, |v, _| v.dash.gate = Some(Vec::new()));
}

/// How many replies the gate holds.
fn held(cx: &mut TestAppContext, pg: &Page) -> usize {
    pg.view.read_with(cx, |v, _| v.dash.gate.as_ref().map_or(0, Vec::len))
}

/// The first reply the gate holds lands, alone.
fn let_one(cx: &mut TestAppContext, pg: &Page) {
    pg.view.update(cx, |v, cx| {
        let apply = v.dash.gate.as_mut().expect("the gate up").remove(0);
        apply(v, cx);
    });
    settle(cx, pg.w, 8);
}

/// What the gate holds lands, then what those ask for, until it holds
/// nothing (the gate stays up): how many landed.
fn drain(cx: &mut TestAppContext, pg: &Page) -> usize {
    let mut n = 0;
    for _ in 0..8 {
        let k = held(cx, pg);
        if k == 0 {
            break;
        }
        n += k;
        pg.view.update(cx, |v, cx| v.let_through(cx));
        settle(cx, pg.w, 8);
    }
    n
}

/// Settings let go as a fold put it out of sight, then shown again as the
/// window widens, its pane not focused: MORE DAEMON CONFIG's REFERENCE
/// values are read again with its config, and drawn; nothing is found
/// again (Shady #113 round 3, 1).
#[gpui_kit::test]
fn settings_shown_again_by_an_unfold_reads_its_reference(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "unfold-reference", json!({IDLE: "600"}), "0.13.0", json!(600));
    let base = Base::new("unfold-reference", "process_scopes = off\n");
    // As its opening's find read them.
    changed(cx, &pg, |v| {
        v.dash.base = Some(base.0.clone());
        v.settings.reference = Some(crate::settings::read_reference(&base.0, None));
    });
    find(cx, &pg, "process_scopes");
    let (row, none) = ("reference process_scopes = off", "reference process_scopes = —");
    assert!(drawn(cx, pg.w, row), "its REFERENCE value");
    let (_left, right) = split_beside(cx, &pg, Kind::Settings);
    assert!(drawn(cx, pg.w, row), "split: drawn in the pane not focused");
    resize(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "folded out of sight: all Settings held goes");
    found(cx, &pg);
    let (reads, finds) = (dash.reads(), pg.view.read_with(cx, |v, _| v.dash.finds));
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (right, None), "the session's pane, still focused");
    let value = pg.view.read_with(cx, |v, _| {
        v.settings.reference.as_ref().and_then(|r| r.values.as_ref().ok()).and_then(|m| m.get("process_scopes").cloned())
    });
    assert_eq!(value, Some("off".to_string()), "shown again: its REFERENCE values read again");
    assert!(drawn(cx, pg.w, row) && !drawn(cx, pg.w, none), "and drawn, in the pane not focused");
    assert_eq!(dash.reads(), reads + 1, "its config read, once");
    assert_eq!(pg.view.read_with(cx, |v, _| v.dash.finds), finds, "found: no find");
}

/// Replies Settings asked for while on screen, held at the gate as a fold
/// puts it out of sight, land after it let go: a config read's; the reads
/// an unfold asks for, folded away again before they land; a find's (its
/// REFERENCE values, the config read after it). Nothing Settings let go of
/// comes back (Shady #113 round 3, 2). On screen all along, Settings takes
/// its reply, whatever else let go meanwhile (Usage's hold is its own).
#[gpui_kit::test]
fn settings_replies_landing_after_a_fold_bring_nothing_back(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "late-fold", json!({IDLE: "600"}), "0.13.0", json!(600));
    let base = Base::new("late-fold", "process_scopes = off\n");
    pg.view.update(cx, |v, _| v.dash.base = Some(base.0.clone()));
    let _ = split_beside(cx, &pg, Kind::Settings);
    let all = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| settings_held(v));
    let activate = |cx: &mut TestAppContext| {
        pg.view.update(cx, |v, cx| {
            v.window_activated(false, cx);
            v.window_activated(true, cx);
        });
        settle(cx, pg.w, 8);
    };
    found(cx, &pg);
    gate(cx, &pg);
    // On screen all along: a read (900), held as Usage opens beside it and
    // closes (Usage let go).
    dash.set(IDLE, json!("900"));
    activate(cx);
    assert_eq!(held(cx, &pg), 1, "a read, its reply held");
    pg.view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    settle(cx, pg.w, 8);
    pg.view.update(cx, |v, cx| {
        let ix = v.model.tabs.iter().position(|t| t.kind == Kind::Usage).expect("Usage, beside Settings");
        v.close_tab(ix);
        cx.notify();
    });
    settle(cx, pg.w, 8);
    assert!(!pg.view.read_with(cx, |v, _| v.model.tabs.iter().any(|t| t.kind == Kind::Usage)), "Usage closed");
    assert_eq!(drain(cx, &pg), 1, "the read's reply lands");
    let idle = pg.view.read_with(cx, |v, _| v.dash.config.as_ref().and_then(|c| c.value(IDLE)).map(String::from));
    assert_eq!(idle.as_deref(), Some("900"), "on screen all along: its reply taken");
    // A config read (the window back in front), held.
    let reads = dash.reads();
    activate(cx);
    assert_eq!((dash.reads(), held(cx, &pg)), (reads + 1, 1), "read; its reply held");
    pg.view.update(cx, |v, _| hold_settings(v));
    assert_eq!(all(cx), [true; 4]);
    resize_frame(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(all(cx), [false; 4], "folded out of sight: all Settings held goes");
    assert_eq!(drain(cx, &pg), 1, "the read's reply lands");
    assert_eq!(all(cx), [false; 4], "a read's reply after the fold brings nothing back");
    // Unfolded: its config and REFERENCE values read, both held; folded
    // again before they land.
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!((dash.reads(), held(cx, &pg)), (reads + 2, 2), "its config read, and its REFERENCE values; both held");
    resize(cx, &pg, FOLDS);
    assert_eq!(drain(cx, &pg), 2);
    assert_eq!(all(cx), [false; 4], "an unfold's reads, landing after the fold again, bring nothing back");
    // Shown again, the gate down: all of it back.
    pg.view.update(cx, |v, _| v.dash.gate = None);
    resize(cx, &pg, 1440.);
    assert_eq!(all(cx)[..3], [true; 3], "shown again: its config, what the dash serves, its REFERENCE values");
    // A find (the dash URL setting changed), held; the fold; it lands, and
    // the config read it asks for after it.
    gate(cx, &pg);
    pg.view.update(cx, |v, cx| v.dash_refind(cx));
    settle(cx, pg.w, 8);
    assert_eq!(held(cx, &pg), 1, "a find, held");
    resize_frame(cx, &pg, FOLDS);
    assert_eq!(all(cx), [false; 4], "folded out of sight: all Settings held goes");
    assert_eq!(drain(cx, &pg), 2, "the find lands, then the read it asked for");
    assert_eq!(all(cx), [false; 4], "a find landing after the fold brings nothing back");
}

/// Replies Settings asked for, held at the gate as a fold puts it out of
/// sight, Settings shown again by an unfold before they land: a config
/// read's, a find's, a REFERENCE read's. Asked for before Settings let go,
/// each drops, though Settings is on screen as it lands (its hold moved
/// on: what is on screen then is not asked); what the unfold asked for
/// brings the config and the values as they are now (Shady #113 round 3,
/// 2).
#[gpui_kit::test]
fn replies_asked_before_a_fold_drop_though_settings_is_back(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "late-back", json!({IDLE: "600"}), "0.13.0", json!(600));
    let base = Base::new("late-back", "process_scopes = off\n");
    let _ = split_beside(cx, &pg, Kind::Settings);
    let idle = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.dash.config.as_ref().and_then(|c| c.value(IDLE)).map(String::from));
    let scopes = |cx: &mut TestAppContext| {
        pg.view.read_with(cx, |v, _| {
            v.settings.reference.as_ref().and_then(|r| r.values.as_ref().ok()).and_then(|m| m.get("process_scopes").cloned())
        })
    };
    let refold = |cx: &mut TestAppContext| {
        resize_frame(cx, &pg, FOLDS);
        assert!(folded(cx, &pg), "folded");
        assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "folded out of sight: all Settings held goes");
        resize(cx, &pg, 1440.);
        assert!(!folded(cx, &pg), "unfolded");
    };
    found(cx, &pg);
    gate(cx, &pg);
    // A config read (600).
    pg.view.update(cx, |v, cx| {
        v.window_activated(false, cx);
        v.window_activated(true, cx);
    });
    settle(cx, pg.w, 8);
    assert_eq!(held(cx, &pg), 1, "a read (600), its reply held");
    dash.set(IDLE, json!("900"));
    refold(cx);
    assert_eq!(held(cx, &pg), 1, "the unfold's read waits for the one under way");
    let_one(cx, &pg);
    assert_eq!(idle(cx), None, "the read asked for before the fold drops, Settings on screen as it lands");
    assert_eq!(drain(cx, &pg), 1, "the unfold's read lands");
    assert_eq!(idle(cx).as_deref(), Some("900"), "the config as it is now");
    // A find (`on`); the unfold's REFERENCE read (`off`).
    std::fs::write(base.0.join("config"), "process_scopes = on\n").expect("its config");
    pg.view.update(cx, |v, cx| {
        v.dash.base = Some(base.0.clone());
        v.dash_refind(cx);
    });
    settle(cx, pg.w, 8);
    assert_eq!(held(cx, &pg), 1, "a find, held");
    std::fs::write(base.0.join("config"), "process_scopes = off\n").expect("its config");
    refold(cx);
    assert_eq!(held(cx, &pg), 2, "the find, and the unfold's REFERENCE read");
    let_one(cx, &pg);
    assert_eq!(scopes(cx), None, "the find asked for before the fold: its values drop, Settings on screen as they land");
    drain(cx, &pg);
    assert_eq!(scopes(cx).as_deref(), Some("off"), "the unfold's: the values as they are now");
    // A REFERENCE read (`on`) under way, the next unfold's (`off`).
    std::fs::write(base.0.join("config"), "process_scopes = on\n").expect("its config");
    refold(cx);
    assert_eq!(held(cx, &pg), 2, "the unfold's config and REFERENCE reads");
    std::fs::write(base.0.join("config"), "process_scopes = off\n").expect("its config");
    refold(cx);
    assert_eq!(held(cx, &pg), 3, "the first unfold's two, the second's REFERENCE read (its config read waits)");
    let_one(cx, &pg);
    let_one(cx, &pg);
    assert_eq!((idle(cx), scopes(cx)), (None, None), "the reads asked for before the fold drop, Settings on screen as they land");
    drain(cx, &pg);
    assert_eq!((idle(cx).as_deref(), scopes(cx).as_deref()), (Some("900"), Some("off")), "the second unfold's: as they are now");
}

/// A write Settings sent while on screen (`refused`: the dash refuses
/// it), its reply held at the gate as Close pane closes Settings' pane,
/// lands after Settings let go: nothing it let go of comes back (its
/// config, its row's refusal), and the write's own course ends (Shady
/// #113 round 3, 2).
fn write_landing_after_its_pane_closed(cx: &mut TestAppContext, refused: bool) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, &format!("late-close-{refused}"), json!({IDLE: "600"}), "0.13.0", json!(600));
    if refused {
        dash.refuse("no");
    }
    let (left, right) = split_beside(cx, &pg, Kind::Settings);
    focus(cx, &pg, left);
    found(cx, &pg);
    gate(cx, &pg);
    pg.view.update(cx, |v, cx| v.config_write(IDLE, Some("900".into()), cx));
    settle(cx, pg.w, 8);
    assert_eq!((dash.posts().len(), held(cx, &pg)), (1, 1), "posted; its reply held");
    pg.view.update(cx, |v, _| hold_settings(v));
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [true; 4]);
    pg.view.update(cx, |v, cx| {
        v.close_pane_tabs();
        cx.notify();
    });
    settle(cx, pg.w, 8);
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (1, right), "the session's pane alone");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "Settings closed: all it held goes");
    assert!(drain(cx, &pg) >= 1, "the write's reply lands");
    pg.view.read_with(cx, |v, _| {
        assert_eq!(settings_held(v), [false; 4], "a write's reply after the close brings nothing back");
        assert!(!v.dash.order.writing(), "the write ended");
    });
    assert_eq!(dash.holds(IDLE), json!(if refused { "600" } else { "900" }), "the dash holds what it took");
}

#[gpui_kit::test]
fn a_write_landing_after_its_pane_closed_brings_nothing_back(cx: &mut TestAppContext) {
    write_landing_after_its_pane_closed(cx, false);
}

#[gpui_kit::test]
fn a_refusal_landing_after_its_pane_closed_brings_nothing_back(cx: &mut TestAppContext) {
    write_landing_after_its_pane_closed(cx, true);
}

/// Loads Usage asked for while on screen, their replies held at the gate:
/// one as a fold puts it out of sight; one as a fold does and an unfold
/// shows it again; one as Close pane closes its pane. Each lands after
/// Usage let go: nothing it let go of comes back, and nothing is loaded
/// for it (Shady #113 round 3, 2).
#[gpui_kit::test]
fn usage_loads_landing_after_a_fold_or_a_close_bring_nothing_back(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, _dash) = effect_page(cx, "late-usage", json!({IDLE: "600"}), "0.13.0", json!(600));
    pg.view.update(cx, |v, _| v.usage.live = true);
    found(cx, &pg);
    pg.view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    settle(cx, pg.w, 8);
    assert!(pg.view.read_with(cx, |v, _| v.usage.board.is_some()), "Usage's board, loaded from the dash");
    let (left, right) = split_beside(cx, &pg, Kind::Usage);
    let all = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| usage_held(v));
    let loads = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.usage.loads);
    let hold = |v: &mut crate::app::AppView| {
        v.usage.fetched = Some(std::time::Instant::now());
        v.usage.error = Some("held".into());
        v.usage.hover = Some((crate::usage_view::Chart::Spend, 0));
    };
    // A load under way, its reply held; the fold.
    gate(cx, &pg);
    pg.view.update(cx, |v, cx| v.usage_load(cx));
    settle(cx, pg.w, 8);
    assert_eq!(held(cx, &pg), 1, "loading; its reply held");
    pg.view.update(cx, |v, _| hold(v));
    assert_eq!(all(cx), [true; 4]);
    resize_frame(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(all(cx), [false; 4], "folded out of sight: all Usage held goes");
    let n = loads(cx);
    assert_eq!(drain(cx, &pg), 1, "the load's reply lands");
    assert_eq!(all(cx), [false; 4], "a load's reply after the fold brings nothing back");
    assert_eq!(loads(cx), n, "and nothing is loaded for it");
    assert!(!pg.view.read_with(cx, |v, _| v.usage.loading), "the load ended");
    // Shown again: the unfold's load under way, its reply held; folded,
    // and shown again before it lands: asked for before Usage let go, it
    // drops, though Usage is on screen as it lands; the load the second
    // unfold asked for brings the board.
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!(held(cx, &pg), 1, "the unfold's load, Usage on screen; its reply held");
    resize_frame(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!((held(cx, &pg), pg.view.read_with(cx, |v, _| v.usage.queued)), (1, true), "the second unfold's load waits");
    let_one(cx, &pg);
    assert_eq!(all(cx), [false; 4], "the load asked for before the fold drops, Usage on screen as it lands");
    assert_eq!(drain(cx, &pg), 1, "the unfold's load lands");
    assert!(pg.view.read_with(cx, |v, _| v.usage.board.is_some()), "its board");
    pg.view.update(cx, |v, _| v.dash.gate = None);
    // Its pane focused, a load under way, a selection made meanwhile; the
    // pane closed (the palette's Close pane) before the reply lands.
    focus(cx, &pg, left);
    gate(cx, &pg);
    pg.view.update(cx, |v, cx| {
        v.usage_load(cx);
        v.usage_load(cx);
    });
    settle(cx, pg.w, 8);
    assert_eq!((held(cx, &pg), pg.view.read_with(cx, |v, _| v.usage.queued)), (1, true), "loading, another queued; the reply held");
    pg.view.update(cx, |v, cx| {
        hold(v);
        v.close_pane_tabs();
        cx.notify();
    });
    settle(cx, pg.w, 8);
    assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.len(), v.panes.focused)), (1, right), "the session's pane alone");
    assert_eq!(all(cx), [false; 4], "Usage closed: all it held goes");
    let n = loads(cx);
    assert_eq!(drain(cx, &pg), 1, "the load's reply lands");
    assert_eq!(all(cx), [false; 4], "a load's reply after the close brings nothing back");
    assert_eq!(loads(cx), n, "and what was queued is not loaded");
}

/// Settings' REFERENCE value for `process_scopes`, as read.
fn scopes(cx: &mut TestAppContext, pg: &Page) -> Option<String> {
    pg.view.read_with(cx, |v, _| {
        v.settings.reference.as_ref().and_then(|r| r.values.as_ref().ok()).and_then(|m| m.get("process_scopes").cloned())
    })
}

/// As at launch: where the daemon runs not known (its first find to
/// come), nothing of Settings read.
fn unknown(cx: &mut TestAppContext, pg: &Page) {
    changed(cx, pg, |v| {
        v.dash.local = None;
        v.dash.config = None;
        v.dash.serves = None;
        v.settings.reference = None;
    });
}

/// Where the daemon runs not known yet, its first find under way as a fold
/// lets Settings go and an unfold shows it again, its pane not focused:
/// the find lands under a hold Settings let go of (its values drop) and
/// says the daemon is here; Settings' config and REFERENCE values are read
/// then, and drawn, with no focus and no activation. So with a find asked
/// for while Settings is folded away. Folded away again before the find
/// lands, no REFERENCE read is made for it; a find asked for under the
/// hold Settings shows under brings the values, and no other read is made
/// (Shady #113 round 4, P2-1).
#[gpui_kit::test]
fn settings_shown_again_before_its_first_find_reads_its_reference(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "first-find", json!({IDLE: "600"}), "0.13.0", json!(600));
    let base = Base::new("first-find", "process_scopes = off\n");
    find(cx, &pg, "process_scopes");
    let (row, none) = ("reference process_scopes = off", "reference process_scopes = —");
    let (_left, right) = split_beside(cx, &pg, Kind::Settings);
    pg.view.update(cx, |v, _| v.dash.base = Some(base.0.clone()));
    let idle = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.dash.config.as_ref().and_then(|c| c.value(IDLE)).map(String::from));
    let reference_reads = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.dash.reference_reads);
    let unfold = |cx: &mut TestAppContext| {
        resize(cx, &pg, 1440.);
        assert!(!folded(cx, &pg), "unfolded");
        assert_eq!(pg.view.read_with(cx, |v, _| (v.panes.focused, v.model.tabs.view())), (right, None), "the session pane, focused");
    };
    let fold = |cx: &mut TestAppContext| {
        resize(cx, &pg, FOLDS);
        assert!(folded(cx, &pg), "folded");
        assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "folded out of sight: all Settings held goes");
    };
    for folded_first in [false, true] {
        unknown(cx, &pg);
        gate(cx, &pg);
        if folded_first {
            fold(cx);
        }
        pg.view.update(cx, |v, cx| v.dash_refind(cx));
        settle(cx, pg.w, 8);
        assert_eq!(held(cx, &pg), 1, "its find, held");
        if !folded_first {
            fold(cx);
        }
        let (reads, refs) = (dash.reads(), reference_reads(cx));
        unfold(cx);
        assert_eq!(held(cx, &pg), 1, "nothing read yet: the find is under way");
        assert_eq!(drain(cx, &pg), 3, "the find lands; then the config read and the REFERENCE read");
        assert_eq!(reference_reads(cx), refs + 1, "one REFERENCE read, as the find said where the daemon runs");
        assert_eq!(scopes(cx, &pg).as_deref(), Some("off"), "its REFERENCE values");
        assert_eq!((idle(cx).as_deref(), dash.reads()), (Some("600"), reads + 1), "its config, read once");
        assert!(drawn(cx, pg.w, row) && !drawn(cx, pg.w, none), "drawn, in the pane not focused");
        pg.view.update(cx, |v, _| v.dash.gate = None);
    }
    // Shown again, then folded away again before the find lands: no
    // REFERENCE read for a Settings let go of.
    unknown(cx, &pg);
    gate(cx, &pg);
    pg.view.update(cx, |v, cx| v.dash_refind(cx));
    settle(cx, pg.w, 8);
    fold(cx);
    unfold(cx);
    fold(cx);
    let refs = reference_reads(cx);
    drain(cx, &pg);
    assert_eq!(reference_reads(cx), refs, "let go of before the find landed: no REFERENCE read");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "and nothing comes back");
    // Shown again with no find under way, then one asked for under the hold
    // it shows under: the find brings its values, and no other read is made.
    unknown(cx, &pg);
    unfold(cx);
    let refs = reference_reads(cx);
    pg.view.update(cx, |v, cx| v.dash_refind(cx));
    settle(cx, pg.w, 8);
    drain(cx, &pg);
    assert_eq!(scopes(cx, &pg).as_deref(), Some("off"), "the find's REFERENCE values");
    assert_eq!(reference_reads(cx), refs, "brought by the find: no other read");
}

/// Settings in a split over a writable config, its find on `bash_timeout`,
/// the gate up: write A (idle exit 900; `refused`: the dash refuses it)
/// sent, its reply held; write B (bash timeout 300) queued behind it.
/// `close`: Settings' pane focused (Close pane closes it).
fn two_writes(cx: &mut TestAppContext, name: &str, refused: bool, close: bool) -> (Page, Dash) {
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, name, json!({IDLE: "600", "bash_timeout": "120"}), "0.13.0", json!(600));
    find(cx, &pg, "bash_timeout");
    let (left, _right) = split_beside(cx, &pg, Kind::Settings);
    if close {
        focus(cx, &pg, left);
    }
    found(cx, &pg);
    gate(cx, &pg);
    if refused {
        dash.refuse("no");
    }
    pg.view.update(cx, |v, cx| v.config_write(IDLE, Some("900".into()), cx));
    settle(cx, pg.w, 8);
    assert_eq!((dash.posts().len(), held(cx, &pg)), (1, 1), "A posted, its reply held");
    dash.accept();
    pg.view.update(cx, |v, cx| v.config_write("bash_timeout", Some("300".into()), cx));
    settle(cx, pg.w, 8);
    assert_eq!(dash.posts().len(), 1, "B queued behind A");
    assert!(matches!(row(cx, &pg), Some(crate::settings_view::Write::Sending(_))), "B's row, sending");
    (pg, dash)
}

/// B's row: sending, refused, or neither.
fn row(cx: &mut TestAppContext, pg: &Page) -> Option<crate::settings_view::Write> {
    pg.view.read_with(cx, |v, _| v.settings.writes.get("bash_timeout").cloned())
}

/// Nothing out, nothing waiting: no write, no read asked for.
fn idle_order(cx: &mut TestAppContext, pg: &Page) -> bool {
    held(cx, pg) == 0 && pg.view.read_with(cx, |v, _| !v.dash.order.writing() && !v.dash.wants_config())
}

/// Writes A and B (two_writes), a fold letting Settings go and an unfold
/// showing it again before A's reply lands: A's reply drops (Settings let
/// go of it), and B, its gate closed with what Settings let go of, waits
/// for the read the unfold asked for; that read proves the gate again, B's
/// row says sending and B goes. The config comes back with B in it, and
/// nothing is left waiting (Shady #113 round 4, P2-2).
fn writes_across_a_fold_and_an_unfold(cx: &mut TestAppContext, refused: bool) {
    let (pg, dash) = two_writes(cx, &format!("fold-writes-{refused}"), refused, false);
    resize(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "folded out of sight: all Settings held goes");
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!(held(cx, &pg), 1, "the unfold's read waits for A");
    let_one(cx, &pg);
    assert_eq!(dash.posts().len(), 1, "B waits for a read to prove its gate again");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "A's reply, asked for before the fold, brings nothing back");
    assert_eq!(held(cx, &pg), 1, "the unfold's read, under way");
    let_one(cx, &pg);
    assert_eq!(dash.posts().len(), 2, "proven again: B goes");
    assert!(matches!(row(cx, &pg), Some(crate::settings_view::Write::Sending(_))), "B's row, sending again");
    assert_eq!(drain(cx, &pg), 1, "B's reply lands");
    assert_eq!(dash.holds("bash_timeout"), json!("300"), "the dash took B");
    assert_eq!(dash.holds(IDLE), json!(if refused { "600" } else { "900" }), "and what it took of A");
    let shown = pg.view.read_with(cx, |v, _| v.dash.config.as_ref().and_then(|c| c.value("bash_timeout")).map(String::from));
    assert_eq!(shown.as_deref(), Some("300"), "the config, back, with B");
    assert_eq!(row(cx, &pg), None, "B's row, done");
    assert!(drawn(cx, pg.w, "field-bash_timeout = 300"), "drawn");
    assert!(idle_order(cx, &pg), "nothing out, nothing waiting");
}

#[gpui_kit::test]
fn a_write_queued_across_a_fold_goes_once_a_read_proves_its_gate(cx: &mut TestAppContext) {
    writes_across_a_fold_and_an_unfold(cx, false);
}

#[gpui_kit::test]
fn a_write_queued_behind_a_refusal_across_a_fold_goes_once_a_read_proves_its_gate(cx: &mut TestAppContext) {
    writes_across_a_fold_and_an_unfold(cx, true);
}

/// Writes A and B, a fold and an unfold before A's reply lands, and the
/// dash meanwhile unable to lock the file: the unfold's read does not
/// prove B's gate, and B's row says why; B is never sent. The config comes
/// back (read-only), and nothing is left waiting (Shady #113 round 4,
/// P2-2).
#[gpui_kit::test]
fn a_write_queued_across_a_fold_says_why_on_its_row_when_its_gate_stays_shut(cx: &mut TestAppContext) {
    let (pg, dash) = two_writes(cx, "fold-shut", false, false);
    resize(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    dash.answer("writable", json!(false));
    resize(cx, &pg, 1440.);
    assert!(!folded(cx, &pg), "unfolded");
    assert_eq!(drain(cx, &pg), 2, "A's reply, then the unfold's read");
    let why = "not sent: synaps-dash cannot lock the file";
    assert_eq!(row(cx, &pg), Some(crate::settings_view::Write::Refused(why.into())), "B's row says why");
    assert!(drawn(cx, pg.w, &format!("refused bash_timeout = {why}")), "drawn, under it");
    assert_eq!((dash.posts().len(), dash.holds("bash_timeout")), (1, json!("120")), "B never sent");
    assert!(pg.view.read_with(cx, |v, _| v.dash.config.as_ref().is_some_and(|c| !c.writable)), "the config, back, read-only");
    assert!(idle_order(cx, &pg), "nothing out, nothing waiting");
}

/// Writes A and B, Close pane closing Settings before A's reply lands: A's
/// reply drops, and B is not Settings' to let go of: the read asked for
/// then proves its gate on its own reply (shown nowhere) and B goes, its
/// reply shown nowhere either. `shut`: the dash cannot lock the file as
/// that read lands; B waits, unsent, for a read under a Settings that
/// shows it, and goes as Settings opens again. Either way Settings, open
/// again, shows the config with B in it (Shady #113 round 4, P2-2).
fn writes_across_a_close(cx: &mut TestAppContext, shut: bool) {
    use crate::tabs::Kind;
    let (pg, dash) = two_writes(cx, &format!("close-writes-{shut}"), false, true);
    pg.view.update(cx, |v, cx| {
        v.close_pane_tabs();
        cx.notify();
    });
    settle(cx, pg.w, 8);
    assert_eq!(pg.view.read_with(cx, |v, _| v.panes.len()), 1, "the session's pane alone");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "Settings closed: all it held goes");
    if shut {
        dash.answer("writable", json!(false));
    }
    assert_eq!(drain(cx, &pg), if shut { 2 } else { 3 }, "A's reply, the read it asks for (then B's reply)");
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "nothing shown for a Settings closed");
    assert_eq!(dash.posts().len(), if shut { 1 } else { 2 }, "B goes once its gate is proven");
    assert_eq!(held(cx, &pg), 0, "nothing under way");
    if shut {
        dash.answer("writable", json!(true));
    }
    let base = Base::new(&format!("close-writes-{shut}"), "");
    pg.view.update(cx, |v, cx| {
        v.dash.gate = None;
        v.dash.base = Some(base.0.clone());
        v.open_view(Kind::Settings, cx);
    });
    settle(cx, pg.w, 8);
    assert_eq!((dash.posts().len(), dash.holds("bash_timeout")), (2, json!("300")), "the dash took B");
    let shown = pg.view.read_with(cx, |v, _| v.dash.config.as_ref().and_then(|c| c.value("bash_timeout")).map(String::from));
    assert_eq!(shown.as_deref(), Some("300"), "Settings open again: the config, with B");
    assert_eq!(row(cx, &pg), None, "B's row, done");
    assert!(pg.view.read_with(cx, |v, _| !v.dash.order.writing() && !v.dash.wants_config()), "nothing out, nothing waiting");
}

#[gpui_kit::test]
fn a_write_queued_as_settings_closes_goes_once_a_read_proves_its_gate(cx: &mut TestAppContext) {
    writes_across_a_close(cx, false);
}

#[gpui_kit::test]
fn a_write_queued_as_settings_closes_waits_for_settings_when_its_gate_stays_shut(cx: &mut TestAppContext) {
    writes_across_a_close(cx, true);
}

/// Usage asked for a load as its channel was to be found again (the dash
/// did not answer), the find held as a fold lets Usage go: the find lands
/// and loads nothing for it, and shows nothing, a URL found or none; a
/// load asked for while Usage is let go of shows no error either. Shown
/// again, Usage loads, or says there is no URL (Shady #113 round 4, P3).
#[gpui_kit::test]
fn a_find_landing_after_usage_let_go_loads_and_shows_nothing_for_it(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, _dash) = effect_page(cx, "usage-find", json!({IDLE: "600"}), "0.13.0", json!(600));
    let base = Base::new("usage-find", "");
    pg.view.update(cx, |v, _| {
        v.usage.live = true;
        v.dash.base = Some(base.0.clone());
    });
    found(cx, &pg);
    pg.view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    settle(cx, pg.w, 8);
    assert!(pg.view.read_with(cx, |v, _| v.usage.board.is_some()), "Usage's board, loaded from the dash");
    let _ = split_beside(cx, &pg, Kind::Usage);
    let loads = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.usage.loads);
    let all = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| usage_held(v));
    for no_url in [false, true] {
        changed(cx, &pg, |v| {
            v.dash.answer = crate::views::Answer::Down(crate::dash::Fail::Unreachable("down".into()));
            if no_url {
                v.dash.found = Some(Err("no URL for synaps-dash".into()));
            }
        });
        gate(cx, &pg);
        pg.view.update(cx, |v, cx| v.usage_load(cx));
        settle(cx, pg.w, 8);
        assert_eq!(held(cx, &pg), 1, "the find first, held");
        resize(cx, &pg, FOLDS);
        assert!(folded(cx, &pg), "folded");
        assert_eq!(all(cx), [false; 4], "folded out of sight: all Usage held goes");
        let n = loads(cx);
        assert_eq!(drain(cx, &pg), 1, "the find lands");
        assert_eq!(loads(cx), n, "it loads nothing for a Usage let go of");
        assert_eq!(all(cx), [false; 4], "and shows nothing: no board, no error");
        if no_url {
            pg.view.update(cx, |v, cx| v.usage_load(cx));
            settle(cx, pg.w, 8);
            assert_eq!(all(cx), [false; 4], "a load asked for while Usage is let go of: no error");
        }
        pg.view.update(cx, |v, _| v.dash.gate = None);
        resize(cx, &pg, 1440.);
        assert!(!folded(cx, &pg), "unfolded");
        let (board, error) = pg.view.read_with(cx, |v, _| (v.usage.board.is_some(), v.usage.error.clone()));
        if no_url {
            assert_eq!((board, error.as_deref()), (false, Some("no URL for synaps-dash")), "shown again: no URL, said");
        } else {
            assert!(board && error.is_none(), "shown again: its board");
        }
    }
}

/// A hold at its last value moves on by wrapping: a reply asked for under
/// it, landing after the fold, drops; one asked for after, under the
/// wrapped hold, is taken (Shady #113 round 4).
#[gpui_kit::test]
fn a_hold_wraps_and_still_moves_on(cx: &mut TestAppContext) {
    use crate::tabs::Kind;
    let (pg, _dash) = effect_page(cx, "wrap", json!({IDLE: "600"}), "0.13.0", json!(600));
    let _ = split_beside(cx, &pg, Kind::Settings);
    found(cx, &pg);
    pg.view.update(cx, |v, _| v.dash.hold_at(Kind::Settings, u64::MAX));
    gate(cx, &pg);
    pg.view.update(cx, |v, cx| {
        v.window_activated(false, cx);
        v.window_activated(true, cx);
    });
    settle(cx, pg.w, 8);
    assert_eq!(held(cx, &pg), 1, "a read under the last hold, held");
    resize(cx, &pg, FOLDS);
    assert!(folded(cx, &pg), "folded");
    assert_eq!(drain(cx, &pg), 1);
    assert_eq!(pg.view.read_with(cx, |v, _| settings_held(v)), [false; 4], "its reply after the fold brings nothing back");
    resize(cx, &pg, 1440.);
    assert_eq!(drain(cx, &pg), 1, "the unfold's read, under the wrapped hold");
    assert!(pg.view.read_with(cx, |v, _| v.dash.config.is_some()), "taken");
}

/// `daemon_version` is read as semver 2.0.0, whole: anything else is
/// unreadable (`None`), and a new-session key says `daemon reload` with the
/// 0.13.0 note (Shady #112 P3).
#[test]
fn versions_are_read_as_semver_whole() {
    use crate::settings_effect::from_0_13;
    let valid = [
        ("0.12.9", false),
        ("0.13.0", true),
        ("0.13.10", true),
        ("1.0.0", true),
        ("0.13.0-rc.1", false),
        ("0.13.0-0", false),
        ("0.13.0-x-y.0a", false),
        ("0.13.0+build.5", true),
        ("0.13.0+001.-", true),
        ("0.13.0-rc.1+b", false),
        ("0.14.0-rc.1", true),
        ("0.12.99999999999999999", false),
    ];
    for (v, from) in valid {
        assert_eq!(from_0_13(v), Some(from), "{v:?}");
    }
    let invalid = [
        "",
        "x",
        "0.13",
        "0.13.0.1",
        "00.13.0",
        "0.013.0",
        "0.13.00",
        "0.13.0+",
        "0.13.0+bad!",
        "0.13.0+one+two",
        "0.13.0+a..b",
        "0.13.1-",
        "0.12.9-",
        "0.13.0-rc..1",
        "0.13.0-01",
        "0.13.0-rc.1+",
        "0.13.0-é",
        "v0.13.0",
        " 0.13.0",
        "0.13.0 ",
        "-0.13.0",
        "+0.13.0",
        "0.18446744073709551616.0",
        "0.13.18446744073709551616",
    ];
    for v in invalid {
        assert_eq!(from_0_13(v), None, "{v:?}");
    }
}

/// Idle exit's evidence, whole: unset is 10; 0 is a value like any (never
/// exits idle); a file value that isn't a whole number, or nothing read of
/// the running one, is unobserved. No other key has any.
#[test]
fn idle_exit_evidence_table() {
    use crate::settings_effect::{Evidence, Observed, evidence, lines};
    let seen = |n: Option<u64>| Observed { idle_exit_running: n };
    let table = [
        (None, Some(10), Evidence::InForce),
        (None, Some(0), Evidence::Waits { running: 0 }),
        (Some("0"), Some(0), Evidence::InForce),
        (Some("0"), Some(10), Evidence::Waits { running: 10 }),
        (Some("600"), Some(30), Evidence::Waits { running: 30 }),
        (Some("600"), None, Evidence::Unobserved),
        (Some("-1"), Some(10), Evidence::Unobserved),
        (Some("1.5"), Some(10), Evidence::Unobserved),
        (Some(" 10"), Some(10), Evidence::Unobserved),
    ];
    for (file, running, want) in table {
        assert_eq!(evidence(IDLE, file, &seen(running)), want, "file {file:?}, running {running:?}");
        assert_eq!(evidence("api_retries", file, &seen(running)), Evidence::Unobserved, "api_retries: never observed");
    }
    let open = lines(IDLE, "0.13.0", Some("0"), &seen(Some(0)), false, true);
    let open: Vec<&str> = open.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(open, ["in force", "The running daemon never exits idle (its --idle-exit 0).", OVERRIDES], "0, open");
}
