//! The continuity suite (settings-overhaul PLAN v3 §4, PR 2): today's
//! Settings page driven through the UI, one case per thing it can do.
//! Each case asserts the action's visible effect, where it went (the fake
//! dash's request, or the app's state), and what persisted: the settings
//! file read back through the loaders a relaunch uses. Green on the page as
//! it is: later PRs rebuild the page, and these say what it must still do.
//! The helpers (how a control is reached: its selector, the find text) may
//! follow the page; the assertions may not. Each case's doc names the one
//! route whose breakage turns it red (the PR body has the runs).
//!
//! Every case saves to a settings file of its own ([`own`]) and talks to a
//! fake dash on loopback ([`Dash`]). None asserts anything about idleness:
//! no counter yet tells a window paint from an AppView render (PR 6 adds
//! one, and the idle gates with it).

use std::io::{BufRead as _, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    PlatformInput, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase, VisualTestContext, point, px,
};
use serde_json::{Value, json};

use super::answer_tests::heard;
use super::keys_tests::press;
use super::{motion_level, open_at, settle, wait};
use crate::app::AppView;
use crate::theme::test_home::{Own, own};

// ── the page ──────────────────────────────────────────────────────────

/// Settings open, saving to a folder of its own.
pub(super) struct Page {
    pub(super) w: AnyWindowHandle,
    pub(super) view: Entity<AppView>,
    home: Own,
    _events: async_channel::Receiver<synaps_client::Event>,
}

/// From this pane width Settings has its index.
const WIDE: (f32, f32) = (1920., 1080.);
const NARROW: (f32, f32) = (1440., 900.);

/// The demo window, `size`, in use (active: a field left blurs), its
/// session shown. At Reduced: under Full a segment's bar (or the picker's
/// light) springs, and a Settings pane that moves asks for its next draw
/// in the frame (`app.rs`' `pane_wants`); with the tests' motion clock
/// pinned that spring never lands, and a test window's draw never ends.
/// Motion's own case starts from Off.
pub(super) fn window(cx: &mut TestAppContext, name: &str, size: (f32, f32)) -> Page {
    let home = own(&format!("continuity-{name}"));
    let (w, view, events) = open_at(cx, size, crate::theme::Panels::default().text_size);
    cx.update_window(w, |_, window, _| window.activate_window()).expect("the window");
    motion_level(cx, &view, crate::motion::Level::Reduced);
    settle(cx, w, 8);
    Page { w, view, home, _events: events }
}

/// Settings opened by its key (Ctrl+, / Cmd+,).
fn open_settings(cx: &mut TestAppContext, pg: &Page) {
    press(cx, pg.w, crate::views::view_keys(cfg!(target_os = "macos"), cfg!(target_os = "linux")).0);
    assert_eq!(pg.view.read_with(cx, |v, _| v.model.tabs.view()), Some(crate::tabs::Kind::Settings), "its key opens Settings");
    assert!(drawn(cx, pg.w, "settings-find"), "Settings, drawn");
}

/// [`window`], Settings open.
fn page(cx: &mut TestAppContext, name: &str, size: (f32, f32)) -> Page {
    let pg = window(cx, name, size);
    open_settings(cx, &pg);
    pg
}

pub(super) fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: &str) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel.to_string())).is_some()
}

fn bounds(cx: &mut TestAppContext, w: AnyWindowHandle, sel: &str) -> Bounds<Pixels> {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel.to_string())).unwrap_or_else(|| panic!("{sel}: not drawn"))
}

fn input(cx: &mut TestAppContext, w: AnyWindowHandle, ev: PlatformInput) {
    cx.update_window(w, |_, window, cx| {
        window.dispatch_event(ev, cx);
    })
    .expect("the window");
    cx.run_until_parked();
}

fn mouse(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<Pixels>, held: bool) {
    let pressed_button = held.then_some(MouseButton::Left);
    input(cx, w, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button, modifiers: Modifiers::default() }));
}

fn down(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<Pixels>) {
    let ev =
        MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false };
    input(cx, w, PlatformInput::MouseDown(ev));
}

fn up(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<Pixels>) {
    input(
        cx,
        w,
        PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }),
    );
}

/// A click on what `sel` names, and what it does drawn.
pub(super) fn click(cx: &mut TestAppContext, w: AnyWindowHandle, sel: &str) {
    let at = bounds(cx, w, sel).center();
    mouse(cx, w, at, false);
    down(cx, w, at);
    up(cx, w, at);
    settle(cx, w, 8);
}

/// Text typed into what has focus, after rubbing out what it held.
fn type_over(cx: &mut TestAppContext, w: AnyWindowHandle, text: &str) {
    let mut vcx = VisualTestContext::from_window(w, cx);
    vcx.simulate_keystrokes(&["backspace"; 64].join(" "));
    if !text.is_empty() {
        vcx.simulate_input(text);
    }
    settle(cx, w, 8);
}

/// The find box: `text` in it.
fn find(cx: &mut TestAppContext, w: AnyWindowHandle, text: &str) {
    click(cx, w, "settings-find");
    type_over(cx, w, text);
}

/// A field (`field-<name>`) edited: clicked, `text` typed over it, Enter.
pub(super) fn edit(cx: &mut TestAppContext, w: AnyWindowHandle, field: &str, text: &str) {
    click(cx, w, field);
    type_over(cx, w, text);
    press(cx, w, "enter");
}

/// Past the app's save delay: what waited is written.
fn saved(cx: &mut TestAppContext, pg: &Page) {
    wait(cx, pg.w, &pg.view, crate::app::SAVE_DELAY.as_millis() as u64 + 100);
}

pub(super) fn changed(cx: &mut TestAppContext, pg: &Page, f: impl FnOnce(&mut AppView)) {
    pg.view.update(cx, |v, cx| {
        f(v);
        v.views_changed(cx);
    });
    settle(cx, pg.w, 8);
}

/// The settings file as the next launch reads it (`main.rs`' loaders).
struct Relaunch {
    app: crate::settings_view::AppSettings,
    panels: crate::theme::Panels,
    theme: Option<String>,
    accent: Option<String>,
    motion: Option<crate::motion::Level>,
    remember: bool,
}

fn relaunch(pg: &Page) -> Relaunch {
    let path: &Path = &pg.home.0;
    let saved_motion = crate::theme::load_str(path, "motion");
    Relaunch {
        app: crate::settings_view::AppSettings::load(Some(path)),
        panels: crate::theme::load_panels(path),
        theme: crate::theme::load_theme(path),
        accent: crate::theme::load_accent(path),
        motion: crate::motion::Setting::resolve(None, None, saved_motion.as_deref()).0.level,
        remember: crate::recall::enabled(crate::theme::load_value(path, crate::recall::SETTING).as_ref()),
    }
}

/// The settings file's keys (none for a daemon's config: that is the dash's).
fn file_keys(pg: &Page) -> Vec<String> {
    let v: Value = std::fs::read_to_string(&pg.home.0).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

// ── a fake synaps-dash ────────────────────────────────────────────────

/// A synaps-dash on loopback, as `dash.rs`' own fake but for every
/// request: `POST /api/config` sets the key in its config (its file) and
/// answers with the config, or, refusing, a 400 in its words; a `GET`
/// answers the config or `/api/info`. It keeps what each `POST` carried
/// and counts the config's reads. Its answer's other fields
/// (`idleExitRunning`, `pending`, `writePath`) are the test's.
pub(super) struct Dash {
    port: u16,
    state: Arc<Mutex<DashState>>,
}

struct DashState {
    values: serde_json::Map<String, Value>,
    refuse: Option<String>,
    posts: Vec<Value>,
    extra: serde_json::Map<String, Value>,
    /// Set by something else as the next `POST` has written, before its
    /// reply reads the file (the real dash's lock covers the write only).
    during_post: Option<(String, Value)>,
    reads: usize,
    stop: bool,
}

impl Dash {
    fn new(values: Value) -> Dash {
        let l = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = l.local_addr().expect("its address").port();
        let values = values.as_object().cloned().unwrap_or_default();
        let state = Arc::new(Mutex::new(DashState {
            values,
            refuse: None,
            posts: Vec::new(),
            extra: serde_json::Map::new(),
            during_post: None,
            reads: 0,
            stop: false,
        }));
        let s = state.clone();
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(conn) = conn else { continue };
                if serve(conn, &s).is_none() {
                    return;
                }
            }
        });
        Dash { port, state }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?token=c0ffee", self.port)
    }

    pub(super) fn snapshot(&self) -> crate::dash::ConfigSnapshot {
        serde_json::from_value(config_json(&self.state.lock().unwrap())).expect("a snapshot")
    }

    pub(super) fn posts(&self) -> Vec<Value> {
        self.state.lock().unwrap().posts.clone()
    }

    /// What its config holds for `key` now (`Null`: unset).
    fn holds(&self, key: &str) -> Value {
        self.state.lock().unwrap().values.get(key).cloned().unwrap_or(Value::Null)
    }

    pub(super) fn refuse(&self, why: &str) {
        self.state.lock().unwrap().refuse = Some(why.into());
    }

    /// Its file's `key` changed by something else (`Null`: removed).
    pub(super) fn set(&self, key: &str, value: Value) {
        self.state.lock().unwrap().values.insert(key.into(), value);
    }

    /// The next `POST` written, `key` set to `value` by something else
    /// before its reply is read.
    pub(super) fn during_post(&self, key: &str, value: Value) {
        self.state.lock().unwrap().during_post = Some((key.into(), value));
    }

    /// How many times `GET /api/config` was answered.
    pub(super) fn reads(&self) -> usize {
        self.state.lock().unwrap().reads
    }

    /// Its answer's field `field` from now on.
    pub(super) fn answer(&self, field: &str, value: Value) {
        self.state.lock().unwrap().extra.insert(field.into(), value);
    }
}

impl Drop for Dash {
    fn drop(&mut self) {
        // Its thread ends at the next request: this one.
        self.state.lock().unwrap().stop = true;
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
    }
}

const PATH: &str = "~/.synaps-cli/config";

fn config_json(s: &DashState) -> Value {
    let mut v = json!({"ok": true, "writable": true, "path": PATH, "values": s.values, "pending": [], "reloadCmd": "synaps daemon reload"});
    v.as_object_mut().expect("an object").extend(s.extra.clone());
    v
}

/// One request answered; `None`: the dash is dropped.
fn serve(conn: std::net::TcpStream, state: &Mutex<DashState>) -> Option<()> {
    if state.lock().unwrap().stop {
        return None;
    }
    let mut r = std::io::BufReader::new(conn.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut len = 0;
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end().to_ascii_lowercase();
        if h.is_empty() {
            break;
        }
        if let Some(v) = h.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    let (status, reply) = {
        let mut s = state.lock().unwrap();
        if line.starts_with("POST /api/config") {
            let got: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            s.posts.push(got.clone());
            match s.refuse.clone() {
                Some(why) => ("400 Bad Request", json!({"ok": false, "error": why})),
                None => {
                    let key = got["key"].as_str().unwrap_or_default().to_string();
                    s.values.insert(key, got["value"].clone());
                    if let Some((k, v)) = s.during_post.take() {
                        s.values.insert(k, v);
                    }
                    ("200 OK", config_json(&s))
                }
            }
        } else if line.starts_with("GET /api/info") {
            ("200 OK", json!({"pid": 7, "profile": null}))
        } else {
            s.reads += 1;
            ("200 OK", config_json(&s))
        }
    };
    let reply = reply.to_string();
    let mut conn = conn;
    write!(conn, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).ok()?;
    Some(())
}

/// The daemon connected, local, and `dash` its own: every link of the
/// config gate proven (as `views.rs`' `proven`), so its rows write.
fn local(cx: &mut TestAppContext, pg: &Page, dash: &Dash) {
    let (url, snap) = (dash.url(), dash.snapshot());
    changed(cx, pg, |v| {
        v.model.daemon = Some(crate::model::Daemon { version: "0.13.0".into(), profile: None, pid: 7 });
        v.model.conn = crate::model::ConnState::Connected("0.13.0".into());
        let target = crate::dash::Target::parse(&url).expect("the dash's URL");
        v.dash.local = Some(Ok(()));
        v.dash.found = Some(Ok((target, crate::dash::Source::UrlFile("/run/synaps-dash.url".into()))));
        v.dash.serves = Some(Ok(crate::dash::Info { pid: 7, profile: None }));
        v.dash.answer = crate::views::Answer::Up;
        v.dash.config = Some(snap);
        assert_eq!(v.config_gate(), Ok(()), "the gate is open");
    });
}

/// Settings over a local daemon whose dash holds `values`.
pub(super) fn daemon_page(cx: &mut TestAppContext, name: &str, values: Value) -> (Page, Dash) {
    let pg = page(cx, name, NARROW);
    let dash = Dash::new(values);
    local(cx, &pg, &dash);
    (pg, dash)
}

// ── DAEMON CONFIG ─────────────────────────────────────────────────────

/// 1. Thinking's dropdown (a default for new sessions, under its session
/// row) takes a choice past the fourth: the dash receives `max` and holds
/// it, and the row shows it. Red: `Pick::Config`'s arm skips `config_write`.
#[gpui_kit::test]
fn thinking_picks_a_choice_past_the_fourth(cx: &mut TestAppContext) {
    let (pg, dash) = daemon_page(cx, "thinking", json!({"thinking": "high"}));
    let w = pg.w;
    find(cx, w, "thinking");
    assert!(drawn(cx, w, "choice thinking = high"));
    click(cx, w, "dropdown thinking");
    click(cx, w, "option thinking max");
    assert_eq!(dash.posts(), vec![json!({"key": "thinking", "value": "max"})], "the dash receives it");
    assert_eq!(dash.holds("thinking"), json!("max"), "and holds it");
    assert!(drawn(cx, w, "choice thinking = max") && !drawn(cx, w, "option thinking max"), "shown; the dropdown closed");
    assert!(!file_keys(&pg).contains(&"thinking".to_string()), "never this app's file");
}

/// 2. A segmented daemon choice (`cache_ttl`): `hybrid` is posted, then
/// `auto` as `null`, the key removed; the row shows each. Red: `to_dash`
/// sends `"auto"`.
#[gpui_kit::test]
fn config_segment_posts_and_auto_removes(cx: &mut TestAppContext) {
    let (pg, dash) = daemon_page(cx, "segment", json!({"cache_ttl": "5m"}));
    let w = pg.w;
    find(cx, w, "cache_ttl");
    assert!(drawn(cx, w, "choice cache_ttl = 5m"));
    click(cx, w, "choice cache_ttl hybrid");
    assert_eq!(dash.posts(), vec![json!({"key": "cache_ttl", "value": "hybrid"})]);
    assert_eq!(dash.holds("cache_ttl"), json!("hybrid"));
    assert!(drawn(cx, w, "choice cache_ttl = hybrid"));
    click(cx, w, "choice cache_ttl auto");
    assert_eq!(dash.posts()[1], json!({"key": "cache_ttl", "value": null}), "auto: unset");
    assert_eq!(dash.holds("cache_ttl"), Value::Null, "the key removed");
    assert!(drawn(cx, w, "choice cache_ttl = auto"));
}

/// 3. A number field: 300, Enter: posted, held, shown. Red:
/// `settings_commit`'s Config arm skips the write.
#[gpui_kit::test]
fn config_number_edits_and_posts(cx: &mut TestAppContext) {
    let (pg, dash) = daemon_page(cx, "number", json!({"bash_timeout": "120"}));
    let w = pg.w;
    find(cx, w, "bash_timeout");
    assert!(drawn(cx, w, "field-bash_timeout = 120"));
    edit(cx, w, "field-bash_timeout", "300");
    assert_eq!(dash.posts(), vec![json!({"key": "bash_timeout", "value": "300"})], "the dash receives it");
    assert_eq!(dash.holds("bash_timeout"), json!("300"));
    assert!(drawn(cx, w, "field-bash_timeout = 300"), "shown");
}

/// 4. A field typed in, then clicked away from: nothing is posted, the
/// value as it was. Red: Blur commits.
#[gpui_kit::test]
fn config_field_left_unsent(cx: &mut TestAppContext) {
    let (pg, dash) = daemon_page(cx, "unsent", json!({"bash_timeout": "120"}));
    let w = pg.w;
    find(cx, w, "bash_timeout");
    click(cx, w, "field-bash_timeout");
    type_over(cx, w, "300");
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.editing), Some(crate::settings_view::Edit::Config("bash_timeout")));
    // Away: the find box.
    click(cx, w, "settings-find");
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.editing), None, "left");
    assert!(dash.posts().is_empty(), "nothing posted");
    assert_eq!(dash.holds("bash_timeout"), json!("120"));
    assert!(drawn(cx, w, "field-bash_timeout = 120"), "the value as it was");
}

/// 5. A write the dash refuses says so under the control, in its words,
/// the value as it was; one out of range is refused here and never sent.
/// Red: `config_under` drops `Refused`.
#[gpui_kit::test]
fn refused_write_shows_under_the_control(cx: &mut TestAppContext) {
    let (pg, dash) = daemon_page(cx, "refused", json!({"bash_timeout": "120"}));
    let w = pg.w;
    dash.refuse("invalid value for bash_timeout");
    find(cx, w, "bash_timeout");
    edit(cx, w, "field-bash_timeout", "300");
    assert_eq!(dash.posts(), vec![json!({"key": "bash_timeout", "value": "300"})], "sent");
    assert!(drawn(cx, w, "refused bash_timeout = invalid value for bash_timeout"), "the dash's words, under it");
    assert_eq!(dash.holds("bash_timeout"), json!("120"));
    assert!(drawn(cx, w, "field-bash_timeout = 120"), "the value as it was");
    edit(cx, w, "field-bash_timeout", "99999");
    assert_eq!(dash.posts().len(), 1, "out of range: never sent");
    assert!(drawn(cx, w, "refused bash_timeout = bash_timeout: expected 1 to 3600, got 99999"), "why, under it");
}

/// The 22 writable keys, each with a value.
fn every_config_value() -> Value {
    json!({
        "model": "anthropic/claude-opus-4-1", "thinking": "high", "context_window": "200k", "compaction_model": null,
        "api_retries": "5", "subagent_timeout": "600", "events.auto_turn": "true", "events.auto_turn_cap": "10",
        "cache_ttl": "1h", "context_management.mode": "auto", "compaction_mode": "remote", "refusal_retries": "2",
        "max_tool_output": "65536", "bash_timeout": "120", "bash_max_timeout": "600", "progressive_tool_disclosure": "false",
        "tools.activation_confirm": "prompt", "startup.quick_start": "on", "startup.extensions_ready_timeout_secs": "30",
        "daemon.idle_exit_secs": "0", "daemon.prompt_abandon_secs": "600", "daemon.parked_evict_secs": "3600",
    })
}

/// 6. The gate closed (the daemon not provably on this machine): all 22
/// rows are read-only, each value shown and no control, DAEMON CONFIG's
/// line names the first link that failed, and nothing posts. Red:
/// `config_gate` opens.
#[gpui_kit::test]
fn remote_rows_say_why(cx: &mut TestAppContext) {
    let (pg, dash) = daemon_page(cx, "remote", every_config_value());
    let w = pg.w;
    let why = "pid 7 is not in this machine's registry";
    changed(cx, &pg, |v| v.dash.local = Some(Err(why.into())));
    let link = format!("the daemon is not provably on this machine: {why}");
    assert!(drawn(cx, w, &format!("group-line 3 = {PATH} · CONFIG read-only: {link}")), "why, the first link");
    let keys: Vec<&str> = crate::settings::CONFIG_GROUPS.iter().flat_map(|(_, k)| k.iter().copied()).collect();
    assert_eq!(keys.len(), 22);
    for key in keys {
        let shown = crate::settings::shown(key, every_config_value()[key].as_str());
        assert!(drawn(cx, w, &format!("locked {key} = {shown}")), "{key}: its value, read-only");
        assert!(!drawn(cx, w, &format!("field-{key}")) && !drawn(cx, w, &format!("dropdown {key}")), "{key}: no field, no dropdown");
        let opts = crate::settings::config_choices(key).unwrap_or_default();
        assert!(opts.iter().all(|o| !drawn(cx, w, &format!("choice {key} {o}"))), "{key}: no segments");
    }
    click(cx, w, "locked bash_timeout = 120");
    assert!(dash.posts().is_empty(), "nothing posts");
    assert_eq!(pg.view.read_with(cx, |v, _| v.config_gate()), Err(link), "the gate, closed");
}

// ── THIS APP ──────────────────────────────────────────────────────────

/// 7. Text size's dropdown takes a step past the fourth, 18: the app's
/// text, the row, and `text_size: 18` read back; Esc closes it unpicked.
/// Red: `set_text_size` skips `set_panels_pub`.
#[gpui_kit::test]
fn text_size_picks_a_step_past_the_fourth(cx: &mut TestAppContext) {
    let pg = page(cx, "text", NARROW);
    let w = pg.w;
    find(cx, w, "text_size");
    assert_eq!(crate::theme::TEXT_STEPS[4], 18, "the fifth step");
    assert!(drawn(cx, w, &format!("choice text-size = {}", crate::theme::DEFAULT_TEXT)));
    click(cx, w, "dropdown text-size");
    click(cx, w, "option text-size 18");
    assert_eq!(pg.view.read_with(cx, |v, _| v.panels.text_size), 18, "picked");
    assert_eq!(cx.update(|cx| cx.global::<crate::theme::Scheme>().choice.text), 18, "the app's text");
    assert!(drawn(cx, w, "choice text-size = 18") && !drawn(cx, w, "option text-size 18"), "shown; closed");
    saved(cx, &pg);
    assert_eq!(relaunch(&pg).panels.text_size, 18, "read back");
    click(cx, w, "dropdown text-size");
    press(cx, w, "escape");
    assert!(!drawn(cx, w, "option text-size 20"), "Esc closes it");
    assert_eq!(pg.view.read_with(cx, |v, _| v.panels.text_size), 18, "nothing picked");
}

/// 8. Motion's `reduced`: the level applied, shown, and read back. Red:
/// `Pick::Motion` skips `save_app`.
#[gpui_kit::test]
fn motion_pick_applies_and_saves(cx: &mut TestAppContext) {
    let pg = page(cx, "motion", NARROW);
    let w = pg.w;
    // From Off (Full's springs never land under the tests' pinned clock).
    motion_level(cx, &pg.view, crate::motion::Level::Off);
    find(cx, w, "motion");
    assert!(drawn(cx, w, "choice motion = off"));
    click(cx, w, "choice motion reduced");
    let level = cx.update(|cx| cx.try_global::<crate::motion::Setting>().and_then(|s| s.level));
    assert_eq!(level, Some(crate::motion::Level::Reduced), "applied");
    assert!(drawn(cx, w, "choice motion = reduced"));
    saved(cx, &pg);
    assert_eq!(relaunch(&pg).motion, Some(crate::motion::Level::Reduced), "read back");
}

/// 9. Card notifications off, then on: quiet, then not; read back each
/// time. Red: `Pick::Notify` skips `save_app`.
#[gpui_kit::test]
fn card_notifications_toggle_saves(cx: &mut TestAppContext) {
    let pg = page(cx, "notify", NARROW);
    let w = pg.w;
    let key = crate::settings_view::NOTIFY;
    find(cx, w, "notification");
    assert!(drawn(cx, w, &format!("choice {key} = on")));
    for (pick, quiet) in [("off", true), ("on", false)] {
        click(cx, w, &format!("choice {key} {pick}"));
        assert_eq!(pg.view.read_with(cx, |v, _| v.app_settings.quiet), quiet, "{pick}");
        assert!(drawn(cx, w, &format!("choice {key} = {pick}")));
        saved(cx, &pg);
        assert_eq!(relaunch(&pg).app.quiet, quiet, "{pick}: read back");
    }
}

/// 10. Remember card choices off: the recall is off, shown, read back.
/// Red: `Pick::Remember` skips `save_app`.
#[gpui_kit::test]
fn remember_toggle_saves(cx: &mut TestAppContext) {
    let pg = page(cx, "remember", NARROW);
    let w = pg.w;
    let key = crate::recall::SETTING;
    find(cx, w, "remember");
    assert!(pg.view.read_with(cx, |v, _| v.recall.on()) && drawn(cx, w, &format!("choice {key} = on")), "on");
    click(cx, w, &format!("choice {key} off"));
    assert!(!pg.view.read_with(cx, |v, _| v.recall.on()), "off");
    assert!(drawn(cx, w, &format!("choice {key} = off")));
    saved(cx, &pg);
    assert!(!relaunch(&pg).remember, "read back: off");
}

/// 11. Forget remembered choices: two questions remembered, in memory and
/// in their file; Forget empties the memory, the file goes, a relaunch
/// recalls nothing, and the row says nothing is remembered. Red:
/// `forget_card_choices` does nothing; `Recall::forget` keeps the memory.
#[gpui_kit::test]
fn forget_remembered_choices(cx: &mut TestAppContext) {
    use crate::recall::{Recall, Store};
    let pg = page(cx, "forget", NARROW);
    let w = pg.w;
    let path: PathBuf = pg.home.0.with_file_name(crate::recall::FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // The file as the app writes it: two questions, the second answered
    // with two ticks.
    let mut store = Store::default();
    store.put("0123456789abcdef".into(), vec![1]);
    store.put("fedcba9876543210".into(), vec![0, 2]);
    std::fs::write(&path, store.to_text()).unwrap();
    assert_eq!(Store::parse(&std::fs::read_to_string(&path).unwrap()), Ok((store, 0)), "a good file");
    changed(cx, &pg, |v| v.recall = Recall::new(true, Some(path.clone())));
    let live = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.recall.len());
    assert_eq!(live(cx), 2, "both remembered, read in");
    find(cx, w, "remember");
    assert!(drawn(cx, w, "recall-forget") && !drawn(cx, w, "recall-none"), "something remembered: Forget");
    click(cx, w, "recall-forget");
    assert_eq!(live(cx), 0, "the memory is empty");
    assert!(!path.exists(), "the file is gone");
    assert!(!crate::recall::Recall::new(true, Some(path.clone())).offers_forget(), "a relaunch: nothing there");
    assert!(drawn(cx, w, "recall-none") && !drawn(cx, w, "recall-forget"), "nothing remembered");
}

fn folds(cx: &mut TestAppContext, pg: &Page) -> (bool, bool) {
    pg.view.read_with(cx, |v, _| (v.panels.rail_collapsed, v.panels.right_collapsed))
}

/// Sessions rail (`rail`) or Right panel: folded, then shown, from its
/// row; the other untouched; read back each time.
fn fold_from_settings(cx: &mut TestAppContext, rail: bool) {
    let pg = page(cx, if rail { "fold-rail" } else { "fold-right" }, NARROW);
    let w = pg.w;
    let id = if rail { "app-rail" } else { "app-right" };
    find(cx, w, "strip");
    assert_eq!(folds(cx, &pg), (false, false));
    for (pick, folded) in [("folded", true), ("shown", false)] {
        click(cx, w, &format!("choice {id} {pick}"));
        assert_eq!(folds(cx, &pg), if rail { (folded, false) } else { (false, folded) }, "{pick}; the other as it was");
        assert!(drawn(cx, w, &format!("choice {id} = {pick}")));
        wait(cx, w, &pg.view, 1000);
        let p = relaunch(&pg).panels;
        assert_eq!(if rail { p.rail_collapsed } else { p.right_collapsed }, folded, "{pick}: read back");
    }
}

/// 12. Red: `Pick::Rail` skips `toggle_panel`.
#[gpui_kit::test]
fn rail_fold_from_settings(cx: &mut TestAppContext) {
    fold_from_settings(cx, true);
}

/// 13. Red: `Pick::Right` skips `toggle_panel`.
#[gpui_kit::test]
fn right_panel_fold_from_settings(cx: &mut TestAppContext) {
    fold_from_settings(cx, false);
}

/// Its edge dragged `by` px wider (the rail's to the right, the right
/// panel's to the left), let go, saved.
fn drag_edge(cx: &mut TestAppContext, pg: &Page, rail: bool, by: f32) {
    let w = pg.w;
    let at = bounds(cx, w, if rail { "rail-edge" } else { "right-edge" }).center();
    let to = |d: f32| point(at.x + px(if rail { d } else { -d }), at.y);
    mouse(cx, w, at, false);
    down(cx, w, at);
    // GPUI starts the drag on the first move.
    mouse(cx, w, to(2.), true);
    for d in [by / 2., by] {
        mouse(cx, w, to(d), true);
    }
    up(cx, w, to(by));
    wait(cx, w, &pg.view, 1000);
}

/// A side panel's width as laid out (the right panel is not drawn beside
/// Settings in one pane).
fn laid_out(cx: &mut TestAppContext, pg: &Page, rail: bool) -> f32 {
    pg.view.read_with(cx, |v, _| {
        let (l, r) = crate::app::sides(v.width, v.panels, crate::chrome::MARK);
        if rail { l.width() } else { r.width() }
    })
}

/// Both widths dragged, then one reset from its row (`rail`'s, or the
/// right panel's): that panel at its default width again, the other's
/// width kept, in the app and read back; neither folds; its reset goes,
/// the other's stays.
fn width_reset_keeps_the_other(cx: &mut TestAppContext, rail: bool) {
    // The right panel is beside the session, not Settings: dragged there.
    let pg = window(cx, if rail { "reset-rail" } else { "reset-right" }, WIDE);
    let w = pg.w;
    let default = laid_out(cx, &pg, rail);
    drag_edge(cx, &pg, true, 90.);
    drag_edge(cx, &pg, false, 110.);
    let (widths, p) = (pg.view.read_with(cx, |v, _| v.panels), relaunch(&pg).panels);
    let (Some(rail_w), Some(right_w)) = (widths.rail_width, widths.right_width) else { panic!("both dragged: {widths:?}") };
    assert_eq!((p.rail_width, p.right_width), (Some(rail_w), Some(right_w)), "both saved");
    open_settings(cx, &pg);
    find(cx, w, "dragged");
    assert!(drawn(cx, w, "rail-reset") && drawn(cx, w, "right-reset"));
    let (this, other) = if rail { ("rail-reset", "right-reset") } else { ("right-reset", "rail-reset") };
    click(cx, w, this);
    let now = pg.view.read_with(cx, |v, _| (v.panels.rail_width, v.panels.right_width));
    assert_eq!(now, if rail { (None, Some(right_w)) } else { (Some(rail_w), None) }, "this one reset, the other kept");
    assert!((laid_out(cx, &pg, rail) - default).abs() < 0.5, "at its default width again");
    assert!(!drawn(cx, w, this) && drawn(cx, w, other), "its reset gone, the other's still there");
    saved(cx, &pg);
    let p = relaunch(&pg).panels;
    assert_eq!((p.rail_width, p.right_width), now, "read back");
    assert_eq!((p.rail_collapsed, p.right_collapsed), (false, false), "read back: neither folded");
    assert_eq!(folds(cx, &pg), (false, false), "neither folded");
}

/// 14. Red: the `rail-reset` handler does nothing.
#[gpui_kit::test]
fn rail_width_reset_keeps_the_right_width(cx: &mut TestAppContext) {
    width_reset_keeps_the_other(cx, true);
}

/// 15. Red: the `right-reset` handler does nothing.
#[gpui_kit::test]
fn right_width_reset_keeps_the_rail_width(cx: &mut TestAppContext) {
    width_reset_keeps_the_other(cx, false);
}

fn scheme(cx: &mut TestAppContext) -> (String, Option<u32>) {
    cx.update(|cx| {
        let c = &cx.global::<crate::theme::Scheme>().choice;
        (c.theme.clone(), c.accent)
    })
}

/// 16. Theme's door, then `nord`: the app's theme, the row, `theme:
/// "nord"` read back. Then Follow the Synaps config: the config's theme,
/// and no theme saved. Red (two runs): `show_theme` skips `save_theme`;
/// `follow_config` skips it.
#[gpui_kit::test]
fn theme_pick_and_follow_save(cx: &mut TestAppContext) {
    let pg = page(cx, "theme", NARROW);
    let w = pg.w;
    find(cx, w, "theme");
    click(cx, w, "app-theme-pick");
    click(cx, w, "picker-theme nord");
    assert_eq!(scheme(cx).0, "nord", "picked");
    assert!(!pg.view.read_with(cx, |v, _| v.theme_follows_config));
    assert!(drawn(cx, w, "value theme = nord"), "the row says so");
    assert_eq!(relaunch(&pg).theme.as_deref(), Some("nord"), "read back");
    click(cx, w, "picker-follow");
    let config = pg.view.read_with(cx, |v, _| v.config_choice().0);
    assert_eq!(scheme(cx).0, config, "the config's theme");
    assert!(pg.view.read_with(cx, |v, _| v.theme_follows_config));
    assert!(drawn(cx, w, &format!("value theme = {config} · follows the Synaps config")), "the row says so");
    assert_eq!(relaunch(&pg).theme, None, "read back: no theme, so it follows");
    assert!(file_keys(&pg).contains(&"theme".to_string()), "written, as null");
}

/// 17. Accent's door, its hex field, `#35f2ff`, Enter: the app's accent,
/// the row, the file; then a preset, the same. Red: `set_accent` skips
/// `save_accent`.
#[gpui_kit::test]
fn accent_hex_through_the_door_applies_and_persists(cx: &mut TestAppContext) {
    let pg = page(cx, "accent", NARROW);
    let w = pg.w;
    find(cx, w, "accent");
    assert!(drawn(cx, w, "value accent = none (the theme's)"));
    click(cx, w, "app-accent-pick");
    click(cx, w, "picker-hex");
    type_over(cx, w, "#35f2ff");
    press(cx, w, "enter");
    assert_eq!(scheme(cx).1, Some(0x35f2ff), "applied");
    assert!(drawn(cx, w, "value accent = #35f2ff"), "the row says so");
    assert_eq!(relaunch(&pg).accent.as_deref(), Some("#35f2ff"), "read back");
    let (name, hex) = crate::theme::PRESETS[1];
    click(cx, w, &format!("picker-preset {name}"));
    assert_eq!(scheme(cx).1, Some(hex), "a preset");
    assert!(drawn(cx, w, &format!("value accent = {}", crate::theme::to_hex(hex))));
    assert_eq!(relaunch(&pg).accent, Some(crate::theme::to_hex(hex)), "read back");
}

/// 18. A theme pinned by `--theme` (the accent by its variable, motion by
/// its flag): read-only, with what pinned it, no door, nothing saved.
/// Red: `pinned()` returns None.
#[gpui_kit::test]
fn pinned_theme_is_read_only_with_its_reason(cx: &mut TestAppContext) {
    let pg = page(cx, "pinned", NARROW);
    let w = pg.w;
    let (theme, _) = scheme(cx);
    changed(cx, &pg, |v| {
        v.theme_source = crate::theme::Source::Flag;
        v.accent_source = crate::theme::Source::Env;
    });
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(crate::motion::Level::Reduced), lock: Some(crate::motion::Lock::Flag) })
    });
    changed(cx, &pg, |_| {});
    assert!(drawn(cx, w, &format!("locked theme = {theme} · set by --theme for this run")), "the theme, by its flag");
    assert!(drawn(cx, w, &format!("locked accent = none (the theme's) · set by ${}", crate::theme::ENV_ACCENT)), "the accent");
    assert!(drawn(cx, w, "locked motion = reduced · set by --motion for this run"), "motion");
    assert!(!drawn(cx, w, "app-theme-pick") && !drawn(cx, w, "app-accent-pick"), "no door");
    assert!(!drawn(cx, w, "choice motion full"), "no choice");
    assert!(pg.view.read_with(cx, |v, _| !v.picker_open), "no picker");
    let r = relaunch(&pg);
    assert_eq!((r.theme, r.accent), (None, None), "nothing saved");
}

/// 19. Daemon label typed, Enter: the row shows it, so does a remote
/// daemon's Connection line, and it reads back; Esc leaves it as it was.
/// Red: `Edit::DaemonLabel` skips `save_app`.
#[gpui_kit::test]
fn daemon_label_saves(cx: &mut TestAppContext) {
    let pg = page(cx, "label", NARROW);
    let w = pg.w;
    find(cx, w, "daemon_label");
    assert!(drawn(cx, w, "field-daemon-label = none"));
    edit(cx, w, "field-daemon-label", "jade");
    assert_eq!(pg.view.read_with(cx, |v, _| v.app_settings.daemon_label.clone()), Some("jade".into()));
    assert!(drawn(cx, w, "field-daemon-label = jade"), "the row says so");
    saved(cx, &pg);
    assert_eq!(relaunch(&pg).app.daemon_label.as_deref(), Some("jade"), "read back");
    click(cx, w, "field-daemon-label");
    type_over(cx, w, "x");
    press(cx, w, "escape");
    assert!(drawn(cx, w, "field-daemon-label = jade"), "Esc: as it was");
    changed(cx, &pg, |v| {
        v.model.daemon = Some(crate::model::Daemon { version: "0.13.0".into(), profile: None, pid: 7 });
        v.model.conn = crate::model::ConnState::Connected("0.13.0".into());
        v.dash.local = Some(Err("pid 7 is not in this machine's registry".into()));
    });
    find(cx, w, "");
    assert!(drawn(cx, w, "fact Daemon = jade · remote daemon"), "a remote daemon, named by it");
    saved(cx, &pg);
    assert_eq!(relaunch(&pg).app.daemon_label.as_deref(), Some("jade"));
}

/// 20. Dash URL typed with its token, Enter: used with the token this run,
/// shown and saved without it, and a find starts for it. Red (two runs):
/// `strip_token` skipped; `dash_refind` skipped.
#[gpui_kit::test]
fn dash_url_saves_without_its_token_and_refinds(cx: &mut TestAppContext) {
    let pg = page(cx, "dash-url", NARROW);
    let w = pg.w;
    find(cx, w, "dash_url");
    assert!(drawn(cx, w, "field-dash-url = discover"));
    let finds = pg.view.read_with(cx, |v, _| v.dash.finds);
    edit(cx, w, "field-dash-url", "http://127.0.0.1:7999/?token=ab12");
    assert_eq!(pg.view.read_with(cx, |v, _| v.app_settings.dash_url.clone()), Some("http://127.0.0.1:7999/?token=ab12".into()), "this run");
    assert!(drawn(cx, w, "field-dash-url = http://127.0.0.1:7999"), "shown without its token");
    assert_eq!(pg.view.read_with(cx, |v, _| v.dash.finds), finds + 1, "a find starts");
    saved(cx, &pg);
    assert_eq!(relaunch(&pg).app.dash_url.as_deref(), Some("http://127.0.0.1:7999"), "read back without its token");
}

/// 21. Usage opens on: range 7d and host bella, each shown and read back;
/// a relaunch's Usage opens on them. Red (two runs): `Pick::Range` or
/// `Pick::Host` skips `save_app`.
#[gpui_kit::test]
fn usage_range_and_host_save(cx: &mut TestAppContext) {
    let pg = page(cx, "usage", NARROW);
    let w = pg.w;
    changed(cx, &pg, |v| v.usage.chips = vec!["bella".into()]);
    find(cx, w, "usage");
    let range = crate::usage::Range::D7;
    assert_ne!(pg.view.read_with(cx, |v, _| v.app_settings.usage_range), range);
    click(cx, w, &format!("choice usage-range {}", range.label()));
    click(cx, w, "choice usage-host bella");
    assert_eq!(
        pg.view.read_with(cx, |v, _| (v.app_settings.usage_range, v.app_settings.usage_host.clone())),
        (range, Some("bella".into()))
    );
    assert!(drawn(cx, w, &format!("choice usage-range = {}", range.label())) && drawn(cx, w, "choice usage-host = bella"));
    saved(cx, &pg);
    let s = relaunch(&pg).app;
    assert_eq!((s.usage_range, s.usage_host.as_deref()), (range, Some("bella")), "read back");
    let usage = crate::usage_view::UsageUi::new(s.usage_host.clone(), s.usage_range);
    assert_eq!((usage.range, usage.host.as_deref()), (range, Some("bella")), "Usage opens on them");
}

// ── the page ──────────────────────────────────────────────────────────

/// 22. At a pane of 1000 px and more, the index's DAEMON CONFIG: that
/// group the top item, at the top, its bar on it. Red: the index click
/// doesn't scroll.
#[gpui_kit::test]
fn index_jump_scrolls_to_the_group(cx: &mut TestAppContext) {
    let pg = page(cx, "index", WIDE);
    let w = pg.w;
    assert!(drawn(cx, w, "settings-index-bar 0") && !drawn(cx, w, "settings-index-bar 3"), "Connection in view");
    let top = f32::from(bounds(cx, w, "settings-find").top());
    assert!(f32::from(bounds(cx, w, "group 3").top()) > top + 400., "DAEMON CONFIG starts far down");
    click(cx, w, "settings-index 3");
    // Child 0 is the title; every group is shown.
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.scroll.top_item()), 4, "the top item");
    let at = f32::from(bounds(cx, w, "group 3").top());
    assert!(at < top + 120., "at the top: {at} (the find box was at {top})");
    assert!(drawn(cx, w, "settings-index-bar 3") && !drawn(cx, w, "settings-index-bar 0"), "its bar");
}

/// 23. MORE DAEMON CONFIG's head unfolds it: its rows, read-only, with
/// the local config's values; again, folded. Red: the `more_open` toggle
/// does nothing.
#[gpui_kit::test]
fn more_daemon_config_unfolds(cx: &mut TestAppContext) {
    let pg = page(cx, "more", NARROW);
    let w = pg.w;
    let values = std::collections::HashMap::from([("process_scopes".to_string(), "off".to_string())]);
    changed(cx, &pg, |v| v.settings.reference = Some(crate::settings::Reference { path: PATH.into(), values: Ok(Arc::new(values)) }));
    let row = "reference process_scopes = off";
    assert!(!drawn(cx, w, row), "folded");
    // Its head in view (child 0 is the title).
    pg.view.update(cx, |v, _| v.settings.scroll.scroll_to_top_of_item(5));
    settle(cx, w, 8);
    click(cx, w, "group 4");
    assert!(pg.view.read_with(cx, |v, _| v.settings.more_open));
    assert!(drawn(cx, w, row) && drawn(cx, w, "reference disabled_tools = default"), "its rows, with their values");
    click(cx, w, "group 4");
    assert!(!drawn(cx, w, row), "folded again");
}

/// 24. Find `bash`: only the bash rows, MORE DAEMON CONFIG's among them,
/// unfolded; emptied, every group again. Red: `settings::matches` always
/// true.
#[gpui_kit::test]
fn find_narrows_and_clearing_restores(cx: &mut TestAppContext) {
    let pg = page(cx, "find", NARROW);
    let w = pg.w;
    let groups = |cx: &mut TestAppContext| (0..7).filter(|ix| drawn(cx, w, &format!("group {ix}"))).collect::<Vec<_>>();
    assert_eq!(groups(cx), (0..7).collect::<Vec<_>>(), "every group");
    find(cx, w, "bash");
    let bash = ["row session-bash_timeout", "row session-bash_max_timeout", "row config-bash_timeout", "row config-bash_max_timeout"];
    for row in bash.into_iter().chain(["reference process_scopes = —", "reference disabled_tools = —"]) {
        assert!(drawn(cx, w, row), "{row}");
    }
    for row in ["row app-theme", "row app-motion", "row session-model", "row session-system", "row config-api_retries"] {
        assert!(!drawn(cx, w, row), "{row}: filtered out");
    }
    assert_eq!(groups(cx), vec![2, 3, 4], "THIS SESSION, DAEMON CONFIG and MORE DAEMON CONFIG alone");
    find(cx, w, "zzqx");
    assert!(drawn(cx, w, "settings-nothing zzqx") && groups(cx).is_empty(), "nothing matches: it says so");
    find(cx, w, "");
    assert_eq!(groups(cx), (0..7).collect::<Vec<_>>(), "emptied: every group again");
    assert!(drawn(cx, w, "row app-theme") && !drawn(cx, w, "reference process_scopes = —"), "MORE folded again");
}

/// A view of the session with `prompt`.
fn session_view(prompt: Option<&str>) -> Value {
    json!({"ev": "setting_changed", "applied": {"id": 1, "setting": "thinking", "ok": true, "message": null,
        "view": {"model": "anthropic/claude-opus-4-1", "thinking_level": "medium", "system_prompt": prompt}}})
}

/// "Remember card choices"' help as today's page words it: its first
/// sentence, and the two after it.
const RECALL_FIRST: &str = "When an agent asks the same card question again in the same folder, the card starts with your last choice highlighted, marked “last time”.";
const RECALL_REST: &str = "The highlight is only a starting point: nothing is ticked or sent for you, and Enter sends what's ticked (on a one-choice card, the option highlighted). Only which option you picked is kept, on this machine, never a note or a field's text.";

/// 25. A click on a row's help shows all of it, another its first
/// sentence; the session's prompt is in its well, every word of it there
/// to scroll through; on the daemon's prompt, it says so. Each text is
/// read from the selector of the element it was handed to. Red: the help
/// click does nothing; the help is always its first sentence; the well
/// draws another body of as many lines.
#[gpui_kit::test]
fn full_help_and_the_prompt_body_are_readable(cx: &mut TestAppContext) {
    let pg = page(cx, "help", NARROW);
    let w = pg.w;
    let key = crate::recall::SETTING;
    let (first, full) = (format!("help-text {key} = {RECALL_FIRST}"), format!("help-text {key} = {RECALL_FIRST} {RECALL_REST}"));
    find(cx, w, "remember");
    assert!(drawn(cx, w, &first) && !drawn(cx, w, &full), "its first sentence");
    click(cx, w, &format!("help {key}"));
    assert!(drawn(cx, w, &full) && !drawn(cx, w, &first), "all of it");
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.open_help), Some(key));
    click(cx, w, &format!("help {key}"));
    assert!(drawn(cx, w, &first) && !drawn(cx, w, &full), "its first sentence again");
    // Sixty rules, each its own, and a last line no other text has.
    let lines = 60;
    let mut prompt: String = (1..lines).map(|n| format!("Rule {n}: say what you did in step {n}, then why.\n")).collect();
    prompt.push_str("Sentinel 7f3a: the sixtieth line, the prompt's last.\n");
    heard(cx, w, &pg.view, vec![session_view(Some(&prompt))]);
    find(cx, w, "prompt");
    let body = format!("system-prompt = {prompt}");
    assert!(drawn(cx, w, "locked system_prompt = custom") && drawn(cx, w, &body), "this session's prompt, every word, in the well");
    let well = bounds(cx, w, &body);
    let (shown, max) = (f32::from(well.size.height), pg.view.read_with(cx, |v, _| -f32::from(v.bars.prompt.scroll.max_offset().y)));
    assert!(shown <= 12. * 20. + 1. && shown + max.abs() >= lines as f32 * 20., "all {lines} lines, in a well {shown} px tall ({max})");
    let at = well.center();
    mouse(cx, w, at, false);
    let delta = ScrollDelta::Pixels(point(px(0.), px(-300.)));
    let ev =
        ScrollEvent { position: at, delta, modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved, source: ScrollSource::Pointer };
    input(cx, w, PlatformInput::Scroll(ev));
    settle(cx, w, 8);
    let y = pg.view.read_with(cx, |v, _| f32::from(v.bars.prompt.scroll.offset().y));
    assert!(y < -100., "it scrolls: {y}");
    heard(cx, w, &pg.view, vec![session_view(None)]);
    assert!(drawn(cx, w, "locked system_prompt = default") && drawn(cx, w, "system-prompt-none"), "the daemon's prompt");
}
