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

/// Settings in a split keeps its config as the session's pane beside it
/// takes the focus: it is still on screen (Shady #112 round 2: the focused
/// pane's tabs alone decided). Open behind a tab in a pane on screen, it
/// keeps it too; folded out of sight, it goes.
#[gpui_kit::test]
fn settings_in_a_split_keeps_its_config_as_the_session_pane_takes_focus(cx: &mut TestAppContext) {
    use crate::pane::KeysTo;
    use crate::tabs::Kind;
    let (pg, dash) = effect_page(cx, "split-kept", json!({IDLE: "600"}), "0.13.0", json!(600));
    find(cx, &pg, IDLE);
    let in_force = "effect daemon.idle_exit_secs = in force";
    assert!(drawn(cx, pg.w, in_force));
    pg.view.update(cx, |v, _| v.app_settings.split = true);
    {
        use gpui_kit::AppContext as _;
        cx.update_window(pg.w, |_, window, cx| pg.view.update(cx, |v, cx| v.split_pane(None, window, cx))).expect("the window");
    }
    settle(cx, pg.w, 8);
    let (left, right) = pg.view.read_with(cx, |v, _| {
        let left = v.panes.rest.iter().find(|p| p.parked.tabs.view() == Some(Kind::Settings)).expect("Settings, in the other pane");
        assert!(v.panes.shown_split() && v.model.tabs.view().is_none(), "the new pane, focused, on the session");
        (left.id, v.panes.cur.id)
    });
    let reads = dash.reads();
    let config = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.shown_config().map(|c| c.idle_exit_running));
    let focus = |cx: &mut TestAppContext, id| {
        pg.view.update(cx, |v, cx| v.focus_pane(id, KeysTo::Pane, cx));
        settle(cx, pg.w, 8);
    };
    assert_eq!(config(cx), Some(Some(600)), "split: the session's pane focused, Settings keeps its config");
    assert!(drawn(cx, pg.w, in_force), "and draws it, in the pane not focused");
    focus(cx, left);
    focus(cx, right);
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
    focus(cx, left);
    focus(cx, right);
    assert_eq!(config(cx), Some(Some(600)), "open behind a tab, in a pane on screen: kept");
    // Folded (each frame folds it again or not, by the window's size): the
    // focused pane alone on screen, Settings out of sight.
    pg.view.update(cx, |v, cx| {
        v.panes.folded = true;
        v.view_shown(cx);
        assert!(v.dash.config.is_none(), "folded out of sight: gone");
    });
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
