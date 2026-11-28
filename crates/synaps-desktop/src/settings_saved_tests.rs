//! Save receipts, the one typed app writer and the strict lock
//! (settings-overhaul PLAN §2.2, §4 PR 5): through the page and the
//! window's close, a second process holding the settings lock where a
//! test needs one (this binary run again as [`settings_lock_holder`]).

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use super::settings_continuity_tests::{NARROW, Page, changed, click, daemon_page, drawn, edit, find, local, open_settings, page, saved};
use super::{motion_level, open_at, settle, wait};
use crate::app::AppView;
use crate::settings_saved::{self as s, AppBatch, AppKey, AppValue, Receipt, STORE};
use crate::settings_view::Pick;

// ── the fixture ───────────────────────────────────────────────────────

/// 13:58 on a Saturday, local time: what a row says it was saved at.
fn t1358() -> i64 {
    use chrono::TimeZone as _;
    chrono::Local.with_ymd_and_hms(2026, 11, 28, 13, 58, 0).single().expect("a local time").timestamp_millis()
}

/// The page's wall clock at 13:58 (its receipts' time).
fn at_1358() {
    s::set_clock(Some(t1358()));
}

fn path(pg: &Page) -> PathBuf {
    pg.home.0.clone()
}

/// The settings file as JSON (none: `{}`).
fn file(pg: &Page) -> Value {
    std::fs::read_to_string(path(pg)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
}

/// The ids of the file's receipts.
fn receipt_ids(pg: &Page) -> Vec<String> {
    let mut ids: Vec<String> = file(pg)[STORE]["e"].as_object().map(|e| e.keys().cloned().collect()).unwrap_or_default();
    ids.sort();
    ids
}

/// The window closed (its close flushes run), and gone.
fn close(cx: &mut TestAppContext, w: AnyWindowHandle, view: Entity<AppView>) {
    let gone = view.downgrade();
    cx.update_window(w, |_, window, _| window.remove_window()).expect("the window");
    drop(view);
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(gone.upgrade().is_none(), "the window is gone");
}

/// The desk closed at once and launched again on the same settings file
/// (main.rs' loaders), Settings open.
fn restart(cx: &mut TestAppContext, pg: Page) -> Page {
    let Page { w, view, home, _events } = pg;
    close(cx, w, view);
    another(cx, home)
}

/// A window on `home`'s settings, as a launch reads them, Settings open.
fn another(cx: &mut TestAppContext, home: crate::theme::test_home::Own) -> Page {
    let (w, view, events) = open_at(cx, NARROW, crate::theme::Panels::default().text_size);
    cx.update_window(w, |_, window, _| window.activate_window()).expect("the window");
    motion_level(cx, &view, crate::motion::Level::Reduced);
    view.update(cx, |v, _| {
        v.app_settings = crate::settings_view::AppSettings::load(Some(&home.0));
        v.panels = crate::theme::load_panels(&home.0);
    });
    settle(cx, w, 8);
    let pg = Page { w, view, home, _events: events };
    open_settings(cx, &pg);
    pg
}

/// [`another`] sharing `pg`'s file, both open.
fn second(cx: &mut TestAppContext, pg: &Page) -> Page {
    another(cx, crate::theme::test_home::Own(path(pg)))
}

/// The settings lock made impossible to take: a directory in its place.
fn unlockable(pg: &Page) -> PathBuf {
    let lock = path(pg).with_extension("json.lock");
    std::fs::create_dir_all(lock.parent().expect("its folder")).expect("the folder");
    let _ = std::fs::remove_file(&lock);
    std::fs::create_dir_all(&lock).expect("a directory where the lock goes");
    lock
}

fn pick(cx: &mut TestAppContext, pg: &Page, what: Pick, value: &str) {
    pg.view.update(cx, |v, cx| v.settings_pick(what, value, cx));
    settle(cx, pg.w, 8);
}

fn post(cx: &mut TestAppContext, pg: &Page, key: &'static str, value: &str) {
    pg.view.update(cx, |v, cx| v.config_write(key, Some(value.into()), cx));
    settle(cx, pg.w, 8);
}

/// The time a row shows beside its value, `saved 13:58`.
fn time_at(cx: &mut TestAppContext, pg: &Page, key: &str) -> bool {
    find(cx, pg.w, key);
    drawn(cx, pg.w, &format!("saved-at {key} = saved 13:58"))
}

fn note(cx: &mut TestAppContext, pg: &Page) -> Option<String> {
    pg.view.read_with(cx, |v, _| v.settings.app_note.as_ref().map(|n| n.why.to_string()))
}

const LOCKED: &str = "settings.json is locked by another process";

// ── a second process holding the lock ─────────────────────────────────

const HOLDER: &str = "SYNAPS_TEST_LOCK_HOLDER";

/// This test binary again, as [`settings_lock_holder`]: it has the lock
/// and has read the file when this returns.
struct Holder {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Holder {
    fn start(settings: &Path) -> Holder {
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", "test_kit::settings_saved_tests::settings_lock_holder", "--ignored", "--nocapture", "--test-threads=1"])
            .env(HOLDER, settings)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the holder");
        let mut lines = std::io::BufReader::new(child.stdout.take().expect("its stdout")).lines();
        loop {
            match lines.next() {
                Some(Ok(l)) if l.ends_with("holder: held") => break,
                Some(Ok(_)) => continue,
                other => panic!("the holder never held the lock: {other:?}"),
            }
        }
        std::thread::spawn(move || lines.for_each(drop));
        let stdin = child.stdin.take();
        Holder { child, stdin }
    }

    /// It writes `key = value` and that value's receipt over what it read,
    /// then lets go and exits.
    fn write(mut self, key: &str, value: &str) {
        let mut stdin = self.stdin.take().expect("its stdin");
        writeln!(stdin, "write {key} {value}").expect("told");
        drop(stdin);
        assert!(self.child.wait().expect("it exits").success(), "the holder wrote");
    }

    /// It lets go, writing nothing.
    fn release(mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The lock tests' second process (run by [`Holder`] alone): it takes the
/// settings lock, reads the file, says so, and waits. Told `write <key>
/// <value>`, it writes that value and its receipt over what it read (as
/// another window of this app would), and exits; told nothing, it exits.
#[test]
#[ignore = "the lock tests run it as their second process"]
fn settings_lock_holder() {
    let Some(path) = std::env::var_os(HOLDER).map(PathBuf::from) else { return };
    let lock = crate::theme::lock_settings(&path.with_extension("json.lock"), Duration::from_secs(10)).expect("the lock");
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    {
        let mut out = std::io::stdout().lock();
        writeln!(out, "holder: held").expect("said");
        out.flush().expect("said");
    }
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if let Some((key, value)) = line.trim().strip_prefix("write ").and_then(|r| r.split_once(' ')) {
        let k = AppKey::of(key).expect("an app key");
        let val = AppValue::Str(value.into());
        v[key] = val.json();
        s::write_into(&mut v, &[Receipt::app(k, &val, s::now_ms())]);
        let tmp = path.with_extension("json.holder.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&v).expect("json")).expect("written");
        std::fs::rename(&tmp, &path).expect("renamed");
    }
    drop(lock);
}

// ── receipts ──────────────────────────────────────────────────────────

/// An app value changed, the desk closed at once (before its save's
/// delay), launched again: the value is there, and so is its time.
#[gpui_kit::test]
fn app_change_then_close_keeps_the_value_and_its_time(cx: &mut TestAppContext) {
    at_1358();
    let pg = page(cx, "close-app", NARROW);
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "jade");
    let pg = restart(cx, pg);
    assert_eq!(crate::theme::load_str(&path(&pg), "daemon_label").as_deref(), Some("jade"), "the value");
    assert_eq!(receipt_ids(&pg), ["app#daemon_label"], "and its receipt");
    find(cx, pg.w, "daemon_label");
    assert!(drawn(cx, pg.w, "field-daemon-label = jade"));
    assert!(time_at(cx, &pg, "daemon_label"), "its time");
}

/// A daemon value saved (the dash's 200), the desk closed before its
/// receipt's own write was due, launched again: its time is there.
#[gpui_kit::test]
fn daemon_change_then_close_keeps_its_time(cx: &mut TestAppContext) {
    at_1358();
    let (pg, dash) = daemon_page(cx, "close-daemon", json!({"api_retries": "1"}));
    post(cx, &pg, "api_retries", "3");
    assert!(time_at(cx, &pg, "api_retries"), "saved");
    assert!(receipt_ids(&pg).is_empty(), "its receipt waits for its write");
    let pg = restart(cx, pg);
    local(cx, &pg, &dash);
    assert_eq!(receipt_ids(&pg).len(), 1, "the close wrote it");
    assert!(time_at(cx, &pg, "api_retries"), "its time, after the restart");
}

/// A value whose write fails gets no receipt: no time, the file as it
/// was, and the row says why.
#[gpui_kit::test]
fn a_failed_value_write_publishes_no_receipt(cx: &mut TestAppContext) {
    at_1358();
    let pg = page(cx, "no-receipt", NARROW);
    unlockable(&pg);
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "jade");
    saved(cx, &pg);
    assert!(note(cx, &pg).is_some_and(|n| n.starts_with("not saved: settings.json can't be locked: ")), "{:?}", note(cx, &pg));
    assert!(pg.view.read_with(cx, |v, _| v.settings.wrote.app(AppKey::DaemonLabel).is_none()), "no receipt");
    assert!(!path(&pg).exists(), "nothing written");
    assert!(!time_at(cx, &pg, "daemon_label"), "no time");
    assert!(drawn(cx, pg.w, "retry daemon_label"), "Retry, under the row");
}

/// A daemon value's receipt whose write fails stays for this run only:
/// its time shows, with `save time not kept: …`; after a restart, none.
#[gpui_kit::test]
fn a_failed_receipt_flush_keeps_it_for_this_run_only(cx: &mut TestAppContext) {
    at_1358();
    let (pg, dash) = daemon_page(cx, "receipt-kept", json!({"api_retries": "1"}));
    let lock = unlockable(&pg);
    post(cx, &pg, "api_retries", "3");
    saved(cx, &pg);
    let r = pg.view.read_with(cx, |v, _| v.settings.wrote.due().first().copied()).expect("still due");
    assert!(r.failed(), "its write failed");
    assert!(time_at(cx, &pg, "api_retries"), "this run keeps it");
    let kept = pg.view.read_with(cx, |v, _| v.settings.receipts_note.as_ref().map(|n| n.why.to_string())).expect("said");
    assert!(kept.starts_with("save time not kept: settings.json can't be locked: "), "{kept}");
    assert!(drawn(cx, pg.w, &format!("not-kept api_retries = {kept}")), "under the row");
    let pg = restart(cx, pg);
    std::fs::remove_dir(&lock).expect("the lock free again");
    local(cx, &pg, &dash);
    assert!(!time_at(cx, &pg, "api_retries"), "not after a restart: it never reached the file");
}

/// A close while a batch is in flight (held before its write): the stop
/// flag. The close writes the latest values, one receipt each; the old
/// batch, let go, changes nothing.
#[gpui_kit::test]
fn close_during_an_inflight_batch(cx: &mut TestAppContext) {
    at_1358();
    let pg = page(cx, "in-flight", NARROW);
    let gate = s::gate::hold();
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "old");
    saved(cx, &pg);
    assert!(pg.view.read_with(cx, |v, _| v.settings.app_writing.is_some()), "in flight, held");
    edit(cx, pg.w, "field-daemon-label", "new");
    pick(cx, &pg, Pick::Range, "7d");
    let pg = restart(cx, pg);
    let after_close = std::fs::read(path(&pg)).expect("the close wrote");
    let _ = gate.try_send(());
    cx.run_until_parked();
    assert_eq!(std::fs::read(path(&pg)).expect("the file"), after_close, "the stopped batch wrote nothing");
    let v = file(&pg);
    assert_eq!((v["daemon_label"].as_str(), v["usage_range"].as_str()), (Some("new"), Some("7d")), "the latest values");
    assert_eq!(receipt_ids(&pg), ["app#daemon_label", "app#usage_range"], "one receipt each");
}

/// Two windows on one file: each one's write keeps the other's receipts.
#[gpui_kit::test]
fn two_windows_keep_each_others_receipts(cx: &mut TestAppContext) {
    at_1358();
    let a = page(cx, "two", NARROW);
    let b = second(cx, &a);
    find(cx, a.w, "daemon_label");
    edit(cx, a.w, "field-daemon-label", "jade");
    saved(cx, &a);
    pick(cx, &b, Pick::Range, "7d");
    saved(cx, &b);
    assert_eq!(receipt_ids(&a), ["app#daemon_label", "app#usage_range"], "both");
    find(cx, a.w, "daemon_label");
    edit(cx, a.w, "field-daemon-label", "onyx");
    saved(cx, &a);
    assert_eq!(receipt_ids(&a), ["app#daemon_label", "app#usage_range"], "still both");
    let v = file(&a);
    assert_eq!((v["daemon_label"].as_str(), v["usage_range"].as_str()), (Some("onyx"), Some("7d")));
}

/// Values keep their JSON types: `panes.split` a boolean, `text_size` a
/// number, `theme` a string; `Null` removes the key.
#[gpui_kit::test]
fn json_types_survive(cx: &mut TestAppContext) {
    let pg = page(cx, "types", NARROW);
    pg.view.update(cx, |v, cx| {
        v.save_app(AppKey::Split, AppValue::Bool(false), cx);
        v.save_app(AppKey::Theme, AppValue::Str("nord".into()), cx);
    });
    pick(cx, &pg, Pick::TextSize, "18");
    saved(cx, &pg);
    let v = file(&pg);
    assert_eq!(v["panes.split"], json!(false), "a boolean");
    assert_eq!(v["text_size"], json!(18), "a number");
    assert_eq!(v["theme"], json!("nord"), "a string");
    assert!(!crate::theme::load_split(&path(&pg)), "read back");
    pg.view.update(cx, |v, cx| v.save_app(AppKey::Theme, AppValue::Null, cx));
    saved(cx, &pg);
    assert!(file(&pg).get("theme").is_none(), "null removes the key");
}

/// A receipts-only write stamps nothing: the daemon value's time is its
/// 200's, not the write's, and no app key gets a receipt.
#[gpui_kit::test]
fn receipt_writes_stamp_nothing(cx: &mut TestAppContext) {
    at_1358();
    let (pg, _dash) = daemon_page(cx, "stamp", json!({"api_retries": "1"}));
    post(cx, &pg, "api_retries", "3");
    s::set_clock(Some(t1358() + 3_600_000));
    saved(cx, &pg);
    let v = file(&pg);
    let e = v[STORE]["e"].as_object().expect("its receipts");
    assert_eq!(e.len(), 1, "one: {e:?}");
    let (id, r) = e.iter().next().expect("one");
    assert!(id.starts_with("daemon:") && id.ends_with("#api_retries"), "{id}");
    assert_eq!(r[0], json!(t1358()), "the 200's time, not the write's");
    assert_eq!(v.as_object().map(|o| o.len()), Some(1), "no value written: {v}");
}

/// A row's time shows only while its value is the one saved: another
/// client's change hides it (and setting it back shows it again); a hand
/// edit of an app value before a launch hides it too.
#[gpui_kit::test]
fn time_hides_when_the_value_changed_elsewhere(cx: &mut TestAppContext) {
    at_1358();
    let (pg, dash) = daemon_page(cx, "elsewhere", json!({"api_retries": "1"}));
    post(cx, &pg, "api_retries", "3");
    assert!(time_at(cx, &pg, "api_retries"));
    dash.set("api_retries", json!("5"));
    changed(cx, &pg, |v| v.dash.config = Some(dash.snapshot()));
    assert!(!time_at(cx, &pg, "api_retries"), "another client's 5");
    dash.set("api_retries", json!("3"));
    changed(cx, &pg, |v| v.dash.config = Some(dash.snapshot()));
    assert!(time_at(cx, &pg, "api_retries"), "3 again: the one saved");
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "jade");
    saved(cx, &pg);
    assert!(time_at(cx, &pg, "daemon_label"));
    let p = path(&pg);
    let mut v = file(&pg);
    v["daemon_label"] = json!("onyx");
    let pg = {
        let Page { w, view, home, _events } = pg;
        close(cx, w, view);
        std::fs::write(&p, v.to_string()).expect("a hand edit");
        another(cx, home)
    };
    find(cx, pg.w, "daemon_label");
    assert!(drawn(cx, pg.w, "field-daemon-label = onyx"), "the hand edit");
    assert!(!time_at(cx, &pg, "daemon_label"), "no time: not the value saved");
}

/// A refused POST records nothing: no time, no receipt to write.
#[gpui_kit::test]
fn refused_post_leaves_no_time(cx: &mut TestAppContext) {
    at_1358();
    let (pg, dash) = daemon_page(cx, "refused", json!({"api_retries": "1"}));
    dash.refuse("api_retries: not now");
    post(cx, &pg, "api_retries", "3");
    find(cx, pg.w, "api_retries");
    assert!(drawn(cx, pg.w, "refused api_retries = api_retries: not now"));
    assert!(!time_at(cx, &pg, "api_retries"));
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.wrote.len()), 0, "no receipt");
    saved(cx, &pg);
    assert!(!path(&pg).exists(), "nothing to write");
}

/// The accent picked is applied at once and saved with the batch: the
/// file is unchanged until the batch's timer, then holds it and its
/// receipt.
#[gpui_kit::test]
fn accent_save_is_batched_not_synchronous(cx: &mut TestAppContext) {
    at_1358();
    let pg = page(cx, "accent-batch", NARROW);
    find(cx, pg.w, "accent");
    click(cx, pg.w, "app-accent-pick");
    let (name, hex) = crate::theme::PRESETS[1];
    click(cx, pg.w, &format!("picker-preset {name}"));
    assert_eq!(cx.update(|cx| cx.global::<crate::theme::Scheme>().choice.accent), Some(hex), "applied");
    assert!(!path(&pg).exists(), "not written on the UI thread");
    wait(cx, pg.w, &pg.view, crate::app::SAVE_DELAY.as_millis() as u64 - 50);
    assert!(!path(&pg).exists(), "nor before the batch's timer");
    saved(cx, &pg);
    assert_eq!(crate::theme::load_accent(&path(&pg)), Some(crate::theme::to_hex(hex)), "the batch wrote it");
    assert_eq!(receipt_ids(&pg), ["app#accent"]);
    let note = pg.view.read_with(cx, |v, _| v.picker_note.clone()).expect("the picker's note");
    assert!(note.starts_with("saved to "), "{note}");
}

/// A daemon-reload key saved, the desk restarted: its row says when it
/// was saved, and that it takes effect at the next daemon reload, with no
/// wait.
#[gpui_kit::test]
fn desk_restart_keeps_the_time_and_shows_no_wait(cx: &mut TestAppContext) {
    at_1358();
    let key = "progressive_tool_disclosure";
    let (pg, dash) = daemon_page(cx, "restart", json!({key: "false"}));
    post(cx, &pg, key, "true");
    saved(cx, &pg);
    let pg = restart(cx, pg);
    local(cx, &pg, &dash);
    assert!(time_at(cx, &pg, key), "saved 13:58");
    assert!(drawn(cx, pg.w, &format!("effect {key} = saved · takes effect at the next daemon reload")), "and when it takes effect");
    assert!(!drawn(cx, pg.w, &format!("warn {key}")), "no wait");
}

/// A receipt due or failed in this window is kept over the file's when
/// Settings reads the file again.
#[gpui_kit::test]
fn reopening_keeps_due_and_failed_receipts_over_the_file(cx: &mut TestAppContext) {
    at_1358();
    let (pg, dash) = daemon_page(cx, "reopen", json!({"api_retries": "1"}));
    post(cx, &pg, "api_retries", "3");
    // Another window's older save of 2 for the same daemon, in the file.
    let (d, older) = pg.view.read_with(cx, |v, _| {
        let r = v.settings.wrote.due()[0];
        (r.daemon, Receipt::daemon(r.daemon, "api_retries", Some("2"), t1358() - 86_400_000))
    });
    let mut v = json!({});
    s::write_into(&mut v, &[older]);
    std::fs::create_dir_all(path(&pg).parent().expect("its folder")).expect("the folder");
    std::fs::write(path(&pg), v.to_string()).expect("written");
    let reopen = |cx: &mut TestAppContext| {
        pg.view.update(cx, |v, cx| v.view_shown(cx));
        cx.run_until_parked();
        local(cx, &pg, &dash);
    };
    reopen(cx);
    let held = |cx: &mut TestAppContext| pg.view.read_with(cx, |v, _| v.settings.wrote.daemon(d, "api_retries").copied()).expect("held");
    assert!(held(cx).due() && held(cx).at_ms == t1358(), "due: this window's, kept");
    assert!(time_at(cx, &pg, "api_retries"));
    unlockable(&pg);
    saved(cx, &pg);
    assert!(held(cx).failed());
    reopen(cx);
    assert!(held(cx).failed() && held(cx).at_ms == t1358(), "failed: kept too");
    assert!(time_at(cx, &pg, "api_retries"));
}

// ── the strict lock ───────────────────────────────────────────────────

/// A text size write while another process holds the lock: after its
/// tries it fails, says so under the row with Retry, writes nothing, and
/// publishes no receipt.
#[gpui_kit::test]
fn a_held_lock_fails_visibly_and_writes_nothing(cx: &mut TestAppContext) {
    let pg = page(cx, "held", NARROW);
    std::fs::create_dir_all(path(&pg).parent().expect("its folder")).expect("the folder");
    std::fs::write(path(&pg), r#"{"daemon_label": "start"}"#).expect("a file");
    let before = std::fs::read(path(&pg)).expect("it");
    let holder = Holder::start(&path(&pg));
    pick(cx, &pg, Pick::TextSize, "18");
    let t0 = Instant::now();
    saved(cx, &pg);
    let tries = crate::theme::LOCK_WAIT * crate::theme::LOCK_TRIES + crate::theme::LOCK_GAP * (crate::theme::LOCK_TRIES - 1);
    assert!(t0.elapsed() >= tries, "after its tries: {:?}", t0.elapsed());
    find(cx, pg.w, "text_size");
    assert!(drawn(cx, pg.w, &format!("not-saved text_size = not saved: {LOCKED}")), "under the row");
    assert!(drawn(cx, pg.w, "retry text_size"));
    assert_eq!(std::fs::read(path(&pg)).expect("it"), before, "byte for byte");
    assert!(pg.view.read_with(cx, |v, _| v.settings.wrote.app(AppKey::TextSize).is_none()), "no receipt");
    assert!(!time_at(cx, &pg, "text_size"));
    holder.release();
}

/// The holder writes its own value and receipt from what it read, then
/// lets go; this app's Retry lands: both values, both receipts.
#[gpui_kit::test]
fn the_holders_write_and_ours_both_survive(cx: &mut TestAppContext) {
    let pg = page(cx, "both", NARROW);
    std::fs::create_dir_all(path(&pg).parent().expect("its folder")).expect("the folder");
    std::fs::write(path(&pg), r#"{"daemon_label": "start"}"#).expect("a file");
    let holder = Holder::start(&path(&pg));
    pick(cx, &pg, Pick::TextSize, "18");
    saved(cx, &pg);
    find(cx, pg.w, "text_size");
    assert!(drawn(cx, pg.w, &format!("not-saved text_size = not saved: {LOCKED}")));
    holder.write("daemon_label", "holder");
    click(cx, pg.w, "retry text_size");
    cx.run_until_parked();
    let v = file(&pg);
    assert_eq!((v["daemon_label"].as_str(), v["text_size"].as_u64()), (Some("holder"), Some(18)), "both values");
    assert_eq!(receipt_ids(&pg), ["app#daemon_label", "app#text_size"], "both receipts");
    assert!(!drawn(cx, pg.w, "retry text_size"), "saved: no note");
}

/// A lock that can't be taken at all (a directory where it goes): the
/// write is refused at once, and the file is untouched.
#[gpui_kit::test]
fn a_lock_that_cannot_be_taken_writes_nothing(cx: &mut TestAppContext) {
    let pg = page(cx, "no-lock", NARROW);
    unlockable(&pg);
    std::fs::write(path(&pg), r#"{"text_size": 15}"#).expect("a file");
    let before = std::fs::read(path(&pg)).expect("it");
    pick(cx, &pg, Pick::TextSize, "18");
    let t0 = Instant::now();
    saved(cx, &pg);
    assert!(t0.elapsed() < crate::theme::LOCK_WAIT * crate::theme::LOCK_TRIES, "at once, no tries: {:?}", t0.elapsed());
    let n = pg.view.read_with(cx, |v, _| v.settings.panels_note.as_ref().map(|n| n.why.to_string())).expect("said");
    assert!(n.starts_with("not saved: settings.json can't be locked: "), "{n}");
    assert_eq!(std::fs::read(path(&pg)).expect("it"), before, "untouched");
}

/// The close, the lock held by another process, a batch in flight held
/// before its write: within the 1 s deadline it writes nothing, and names
/// every key it couldn't save on stderr.
#[gpui_kit::test]
fn close_with_a_held_lock_writes_nothing_and_says_which(cx: &mut TestAppContext) {
    let pg = page(cx, "close-held", NARROW);
    std::fs::create_dir_all(path(&pg).parent().expect("its folder")).expect("the folder");
    std::fs::write(path(&pg), r#"{"daemon_label": "start"}"#).expect("a file");
    let before = std::fs::read(path(&pg)).expect("it");
    let gate = s::gate::hold();
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "jade");
    saved(cx, &pg);
    assert!(pg.view.read_with(cx, |v, _| v.settings.app_writing.is_some()), "a batch in flight, held");
    let holder = Holder::start(&path(&pg));
    pg.view.update(cx, |v, cx| v.save_app(AppKey::Theme, AppValue::Str("nord".into()), cx));
    pick(cx, &pg, Pick::TextSize, "18");
    let _ = crate::theme::said_at_close();
    let Page { w, view, home, _events } = pg;
    let t0 = Instant::now();
    close(cx, w, view);
    let took = t0.elapsed();
    assert!(took >= crate::theme::CLOSE_DEADLINE - Duration::from_millis(50), "it waited for the lock: {took:?}");
    assert!(took < crate::theme::CLOSE_DEADLINE + Duration::from_millis(900), "the deadline held: {took:?}");
    assert_eq!(
        crate::theme::said_at_close(),
        ["synaps-desktop: not saved at close, settings.json is locked: text_size, theme, daemon_label"],
        "every key named"
    );
    assert_eq!(std::fs::read(&home.0).expect("it"), before, "nothing written");
    holder.release();
    let _ = gate.try_send(());
    cx.run_until_parked();
    assert_eq!(std::fs::read(&home.0).expect("it"), before, "the stopped batch writes nothing either");
}

/// Tabs, the layout and favourites never write unlocked: each write
/// fails and leaves the file; their jobs record nothing written, so the
/// next change writes again, and lands once the lock can be taken.
#[test]
fn tabs_layout_and_favourites_never_write_unlocked() {
    let home = crate::theme::test_home::own("never-unlocked");
    let path = home.0.clone();
    std::fs::create_dir_all(path.parent().expect("its folder")).expect("the folder");
    std::fs::write(&path, r#"{"theme": "nord"}"#).expect("a file");
    let before = std::fs::read(&path).expect("it");
    let lock = path.with_extension("json.lock");
    std::fs::create_dir(&lock).expect("a directory where the lock goes");
    let saved = crate::tabs::Saved { sessions: vec!["s1".into()], journals: Vec::new(), labels: Vec::new(), active: Some(0) };
    let layout = crate::theme::Layout { focused: 0, tree: crate::theme::LayoutNode::Pane(0), panes: vec![saved.clone()] };
    let fav = crate::favourites::from_json(&json!([{"id": "s1"}]));
    let job = std::sync::Mutex::new(crate::app::SaveJob::default());
    use crate::app::SaveKind;
    let tabs = crate::app::SaveJob::write(&job, SaveKind::Tabs, 1, || crate::theme::save_tabs(&path, "k", &saved));
    let lay = crate::app::SaveJob::write(&job, SaveKind::Layout, 1, || crate::theme::save_layout(&path, "k", &layout));
    let favs = crate::theme::save_favourites(&path, "k", &[], &fav);
    for (what, e) in [("tabs", tabs.err()), ("layout", lay.err()), ("favourites", favs.err())] {
        let e = e.unwrap_or_else(|| panic!("{what}: written unlocked"));
        assert!(e.to_string().starts_with("settings.json can't be locked: "), "{what}: {e}");
    }
    assert_eq!(std::fs::read(&path).expect("it"), before, "the file untouched");
    assert_eq!(job.lock().unwrap().writes, 0, "no write recorded: their generation stays unwritten");
    std::fs::remove_dir(&lock).expect("the lock free");
    let tabs = crate::app::SaveJob::write(&job, SaveKind::Tabs, 2, || crate::theme::save_tabs(&path, "k", &saved));
    let lay = crate::app::SaveJob::write(&job, SaveKind::Layout, 2, || crate::theme::save_layout(&path, "k", &layout));
    assert!(tabs.is_ok_and(|w| w) && lay.is_ok_and(|w| w), "the next change writes");
    crate::theme::save_favourites(&path, "k", &[], &fav).expect("so do favourites");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("it")).expect("json");
    assert!(v["tabs"]["k"].is_object() && v["layout"]["k"].is_object() && v["favourites"]["k"].is_array(), "{v}");
    assert_eq!(v["theme"], json!("nord"), "merged");
}

/// A failed batch merges back under what came since, a newer value for a
/// key winning: one pending slot per key, never held twice; it then lands
/// whole.
#[gpui_kit::test]
fn a_failed_batch_merges_back_and_is_never_held_twice(cx: &mut TestAppContext) {
    let pg = page(cx, "merge-back", NARROW);
    let lock = unlockable(&pg);
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "old");
    pick(cx, &pg, Pick::Range, "7d");
    saved(cx, &pg);
    assert!(note(cx, &pg).is_some(), "failed");
    find(cx, pg.w, "daemon_label");
    edit(cx, pg.w, "field-daemon-label", "new");
    let mut want = AppBatch::default();
    want.set(AppKey::DaemonLabel, AppValue::Str("new".into()));
    want.set(AppKey::UsageRange, AppValue::Str("7d".into()));
    pg.view.read_with(cx, |v, _| {
        assert_eq!(v.settings.app_dirty, want, "one slot per key, the newer value in it");
        assert!(v.settings.app_writing.is_none(), "no copy held in flight");
    });
    std::fs::remove_dir(&lock).expect("the lock free");
    saved(cx, &pg);
    let v = file(&pg);
    assert_eq!((v["daemon_label"].as_str(), v["usage_range"].as_str()), (Some("new"), Some("7d")));
    assert_eq!(receipt_ids(&pg), ["app#daemon_label", "app#usage_range"]);
    assert!(note(cx, &pg).is_none(), "the note goes with it");
}

// ── the writer's ceiling ──────────────────────────────────────────────

/// Every writer owner filled at once (PLAN §4's budget gate): the lock
/// held by another process; a 4,096 B daemon label and dash URL failed;
/// an older batch in flight, held, under a newer value for every app key;
/// the daemon queue at 22 keys of 4,096 B behind a write out; 256
/// receipts, their write failing. Settings closed and opened again: the
/// writer holds at most 147,904 B, and nothing unsaved was dropped: let
/// go, every value lands, with its receipt, and the writer empties.
#[gpui_kit::test]
fn settings_writer_state_holds_its_ceiling(cx: &mut TestAppContext) {
    assert_eq!(std::mem::size_of::<Receipt>(), 48);
    assert!(std::mem::size_of::<Option<AppValue>>() <= 24);
    let (pg, dash) = daemon_page(cx, "ceiling", json!({}));
    std::fs::create_dir_all(path(&pg).parent().expect("its folder")).expect("the folder");
    std::fs::write(path(&pg), "{}").expect("a file");
    let holder = Holder::start(&path(&pg));
    let big = |c: char| c.to_string().repeat(crate::settings_editor::VALUE_CAP);
    pg.view.update(cx, |v, cx| {
        for i in 0..s::Receipts::CAP {
            v.settings.wrote.put(Receipt::daemon([i as u8, (i >> 8) as u8, 9, 9, 9, 9, 9, 9], "api_retries", Some("1"), i as i64));
        }
        v.save_app(AppKey::DaemonLabel, AppValue::Str(big('l').into()), cx);
        v.save_app(AppKey::DashUrl, AppValue::Str(big('u').into()), cx);
    });
    saved(cx, &pg);
    assert!(note(cx, &pg).is_some_and(|n| n.contains(LOCKED)), "failed: {:?}", note(cx, &pg));
    let gate = s::gate::hold();
    pg.view.update(cx, |v, cx| v.write_app(cx));
    assert!(pg.view.read_with(cx, |v, _| v.settings.app_writing.is_some()), "in flight, held");
    let newest = |k: AppKey| match k {
        AppKey::DaemonLabel => AppValue::Str(big('m').into()),
        AppKey::DashUrl => AppValue::Str(big('v').into()),
        AppKey::UsageHost => AppValue::Str("h".repeat(crate::settings_view::HOST_MAX).into()),
        AppKey::TextSize => AppValue::Num(18),
        AppKey::Rail | AppKey::Right | AppKey::Split => AppValue::Bool(true),
        _ => AppValue::Str("x".repeat(64).into()),
    };
    let writable: Vec<&'static str> = crate::settings::CONFIG_GROUPS.iter().flat_map(|(_, k)| k.iter().copied()).collect();
    assert_eq!(writable.len(), crate::views::ConfigOrder::QUEUE);
    pg.view.update(cx, |v, cx| {
        for k in AppKey::ALL {
            v.settings.app_dirty.set(k, newest(k));
        }
        v.settings.app_gen += 1;
        v.dash.order.fill(&writable, &big('q'));
        let _ = cx;
    });
    let close_tab = |cx: &mut TestAppContext| {
        pg.view.update(cx, |v, cx| {
            let ix = v.model.tabs.iter().position(|t| t.kind == crate::tabs::Kind::Settings).expect("Settings");
            v.close_tab(ix);
            v.views_changed(cx);
        });
        settle(cx, pg.w, 8);
    };
    close_tab(cx);
    open_settings(cx, &pg);
    local(cx, &pg, &dash);
    let held = pg.view.read_with(cx, |v, _| v.retained_writer());
    assert!(held <= 147_904, "the writer holds {held} B");
    assert!(held >= 110_000, "every owner was filled: {held} B");
    // Let go: the lock, the batch held, the queue's write out answered.
    holder.release();
    let _ = gate.try_send(());
    cx.run_until_parked();
    pg.view.update(cx, |v, cx| v.resume_config_queue(cx));
    cx.run_until_parked();
    pg.view.update(cx, |v, cx| v.retry_saves(cx));
    saved(cx, &pg);
    let v = file(&pg);
    for k in AppKey::ALL.into_iter().filter(|k| !matches!(k, AppKey::TextSize | AppKey::Rail | AppKey::Right)) {
        assert_eq!(v[k.key()], newest(k).json(), "{}: landed", k.key());
        assert!(v[STORE]["e"].get(format!("app#{}", k.key())).is_some(), "{}: with its receipt", k.key());
    }
    let posts = dash.posts();
    for k in &writable {
        assert!(posts.iter().any(|p| p["key"] == json!(k) && p["value"] == json!(big('q'))), "{k}: sent");
    }
    pg.view.read_with(cx, |v, _| {
        assert!(v.settings.app_dirty.is_empty() && v.settings.app_writing.is_none(), "the batches are empty");
        assert!(v.dash.order.retained() == crate::views::ConfigOrder::QUEUE * 32, "the queue is empty");
        assert!(!v.settings.wrote.any_due(), "every receipt written");
        assert!(v.settings.app_note.is_none() && v.settings.receipts_note.is_none(), "no note");
    });
}
