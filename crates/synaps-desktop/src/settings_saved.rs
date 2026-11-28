//! Save receipts and the window's one typed app writer (settings-overhaul
//! PLAN §2.2, §3.6 Part 2).
//!
//! A receipt says that this app wrote a value, and when: an app value at
//! the moment its write took the settings file's lock, a daemon value at
//! the dash's 200. It is kept in settings.json (`settings.saved`) beside
//! the values, in the same locked write as an app value, so neither lands
//! without the other. A row shows its time only while the value it holds
//! is still the one saved (the digests are equal).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gpui_kit::{AppContext as _, Context, Task};
use serde_json::Value;

use crate::app::AppView;
use crate::theme;

// ── the keys and their values ─────────────────────────────────────────

/// THIS APP's keys (PLAN §1.1's 14 rows), one slot each in a batch. The
/// folds and the text size are the panels' writer's; the model picker's
/// source is read-only. Both keep their slots, so a key's slot is fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppKey {
    Theme,
    Accent,
    TextSize,
    Motion,
    Notify,
    Remember,
    Rail,
    Right,
    Split,
    DaemonLabel,
    DashUrl,
    UsageRange,
    UsageHost,
    PickersSource,
}

pub(crate) const APP_KEYS: usize = 14;

impl AppKey {
    pub(crate) const ALL: [AppKey; APP_KEYS] = [
        AppKey::Theme,
        AppKey::Accent,
        AppKey::TextSize,
        AppKey::Motion,
        AppKey::Notify,
        AppKey::Remember,
        AppKey::Rail,
        AppKey::Right,
        AppKey::Split,
        AppKey::DaemonLabel,
        AppKey::DashUrl,
        AppKey::UsageRange,
        AppKey::UsageHost,
        AppKey::PickersSource,
    ];

    /// Its key in settings.json.
    pub(crate) fn key(self) -> &'static str {
        match self {
            AppKey::Theme => "theme",
            AppKey::Accent => "accent",
            AppKey::TextSize => theme::Panels::TEXT_SIZE,
            AppKey::Motion => "motion",
            AppKey::Notify => crate::settings_view::NOTIFY,
            AppKey::Remember => crate::recall::SETTING,
            AppKey::Rail => theme::Panels::RAIL,
            AppKey::Right => theme::Panels::RIGHT,
            AppKey::Split => "panes.split",
            AppKey::DaemonLabel => "daemon_label",
            AppKey::DashUrl => "dash_url",
            AppKey::UsageRange => "usage_range",
            AppKey::UsageHost => "usage_host",
            AppKey::PickersSource => "pickers.source",
        }
    }

    pub(crate) fn of(key: &str) -> Option<AppKey> {
        AppKey::ALL.into_iter().find(|k| k.key() == key)
    }
}

/// An app value as the file keeps it: a string, a boolean, a number, or
/// `Null`, which removes the key.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AppValue {
    Str(Box<str>),
    Bool(bool),
    Num(i64),
    Null,
}

const _: () = assert!(std::mem::size_of::<Option<AppValue>>() <= 24);

impl AppValue {
    /// A string, or none (`Null`).
    pub(crate) fn of(v: Option<String>) -> AppValue {
        v.map_or(AppValue::Null, |s| AppValue::Str(s.into()))
    }

    pub(crate) fn json(&self) -> Value {
        match self {
            AppValue::Str(s) => Value::String(s.to_string()),
            AppValue::Bool(b) => Value::Bool(*b),
            AppValue::Num(n) => Value::from(*n),
            AppValue::Null => Value::Null,
        }
    }

    /// Compact JSON: what its digest is taken of.
    pub(crate) fn canonical(&self) -> String {
        serde_json::to_string(&self.json()).unwrap_or_default()
    }

    /// The bytes of heap it owns.
    #[cfg(test)]
    fn heap(&self) -> usize {
        match self {
            AppValue::Str(s) => s.len(),
            _ => 0,
        }
    }
}

/// Keys as bits, one per [`AppKey`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AppKeySet(pub(crate) u16);

impl AppKeySet {
    pub(crate) fn with(self, k: AppKey) -> AppKeySet {
        AppKeySet(self.0 | 1 << k as u16)
    }

    pub(crate) fn has(self, k: AppKey) -> bool {
        self.0 & 1 << k as u16 != 0
    }

    pub(crate) fn names(self) -> impl Iterator<Item = &'static str> {
        AppKey::ALL.into_iter().filter(move |&k| self.has(k)).map(AppKey::key)
    }
}

/// THIS APP's changes waiting for their write, or being written: a slot
/// per key, the latest value in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AppBatch([Option<AppValue>; APP_KEYS]);

impl AppBatch {
    pub(crate) fn set(&mut self, k: AppKey, v: AppValue) {
        self.0[k as usize] = Some(v);
    }

    /// [`AppBatch::set`] by the key's name, a string or none.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, key: &str, v: Option<String>) {
        if let Some(k) = AppKey::of(key) {
            self.set(k, AppValue::of(v));
        }
    }

    pub(crate) fn get(&self, k: AppKey) -> Option<&AppValue> {
        self.0[k as usize].as_ref()
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        AppKey::of(key).is_some_and(|k| self.get(k).is_some())
    }

    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        *self = AppBatch::default();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }

    pub(crate) fn keys(&self) -> AppKeySet {
        self.iter().fold(AppKeySet::default(), |s, (k, _)| s.with(k))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (AppKey, &AppValue)> {
        AppKey::ALL.into_iter().zip(&self.0).filter_map(|(k, v)| Some((k, v.as_ref()?)))
    }

    /// `older` (a batch that failed, or one a close stopped) under this
    /// one: each key it has that this one hasn't comes back; a newer value
    /// wins. One slot per key, so a value is never held twice.
    pub(crate) fn merge_older(&mut self, older: &AppBatch) {
        for (slot, was) in self.0.iter_mut().zip(&older.0) {
            if slot.is_none() {
                slot.clone_from(was);
            }
        }
    }

    /// The bytes of heap its values own.
    #[cfg(test)]
    pub(crate) fn heap(&self) -> usize {
        self.0.iter().flatten().map(AppValue::heap).sum()
    }
}

// ── receipts ──────────────────────────────────────────────────────────

/// Whose a receipt is: this app's settings, or a daemon's config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(crate) enum Scope {
    App = 0,
    Daemon = 1,
}

impl Scope {
    fn word(self) -> &'static str {
        match self {
            Scope::App => "app",
            Scope::Daemon => "daemon",
        }
    }
}

/// One save's receipt, 48 B: the key (this build's own, static), the
/// daemon it went to (zeros for an app key), when, the value's digest, and
/// two flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub(crate) key: &'static str,
    pub(crate) daemon: [u8; 8],
    pub(crate) at_ms: i64,
    pub(crate) digest: u64,
    pub(crate) scope: Scope,
    flags: u8,
}

const _: () = assert!(std::mem::size_of::<Receipt>() == 48);

/// Not in the file yet (a daemon value's, until a write carries it).
const DUE: u8 = 1;
/// A write that carried it failed: kept for this run, and said so.
const FAILED: u8 = 2;

impl Receipt {
    pub(crate) fn app(k: AppKey, v: &AppValue, at_ms: i64) -> Receipt {
        Receipt { key: k.key(), daemon: [0; 8], at_ms, digest: digest(Scope::App, k.key(), &v.canonical()), scope: Scope::App, flags: 0 }
    }

    /// A daemon value saved through the dash (its 200 at `at_ms`), due.
    pub(crate) fn daemon(daemon: [u8; 8], key: &'static str, value: Option<&str>, at_ms: i64) -> Receipt {
        Receipt { key, daemon, at_ms, digest: daemon_digest(key, value), scope: Scope::Daemon, flags: DUE }
    }

    /// Its id in the file: `app#<key>`, `daemon:<d>#<key>`. At most 72
    /// bytes, as the keys are this build's: an app key, or one of the 22
    /// the dash writes (the longest, 37 bytes, makes 61).
    pub(crate) fn id(&self) -> String {
        match self.scope {
            Scope::App => format!("app#{}", self.key),
            Scope::Daemon => format!("daemon:{}#{}", hex(&self.daemon), self.key),
        }
    }

    /// The same save's place: the same id.
    fn same(&self, o: &Receipt) -> bool {
        (self.scope, self.daemon, self.key) == (o.scope, o.daemon, o.key)
    }

    /// Eviction's order, `(at_ms, id)` (ids compare as these do: `app#`
    /// before `daemon:`, the daemon's hex as its bytes, then the key).
    fn order(&self) -> (i64, Scope, [u8; 8], &'static str) {
        (self.at_ms, self.scope, self.daemon, self.key)
    }

    pub(crate) fn due(&self) -> bool {
        self.flags & DUE != 0
    }

    pub(crate) fn failed(&self) -> bool {
        self.flags & FAILED != 0
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The first 16 hex digits of SHA-256 over `v1\0<scope>\0<key>\0<canonical>`.
pub(crate) fn digest(scope: Scope, key: &str, canonical: &str) -> u64 {
    // STAND-IN (tests-first): this process's hash, as main's record of
    // saves keyed its owners: stable within a run, not across processes.
    if !key.is_empty() || key.is_empty() {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (scope.word(), key, canonical).hash(&mut h);
        return h.finish();
    }
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    for part in ["v1", scope.word(), key] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.update(canonical.as_bytes());
    let d = h.finalize();
    u64::from_be_bytes(d[..8].try_into().unwrap_or_default())
}

/// A daemon value's digest: the dash's value string, `null` when unset.
pub(crate) fn daemon_digest(key: &str, value: Option<&str>) -> u64 {
    digest(Scope::Daemon, key, value.unwrap_or("null"))
}

/// The daemon a config is: its socket and profile, and the snapshot's
/// `writePath` (the first 8 bytes of their SHA-256). Not its pid: a cold
/// start is the same daemon's.
pub(crate) fn daemon_id(socket: Option<&Path>, profile: Option<&str>, write_path: Option<&str>) -> [u8; 8] {
    // STAND-IN (tests-first): this process's hash, as main's
    // `config_owner` was.
    if socket.is_none() || socket.is_some() {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (socket, profile, write_path).hash(&mut h);
        return h.finish().to_le_bytes();
    }
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"v1\0");
    h.update(socket.map(|s| s.to_string_lossy()).unwrap_or_default().as_bytes());
    h.update([0]);
    h.update(profile.unwrap_or_default().as_bytes());
    h.update([0]);
    h.update(write_path.unwrap_or_default().as_bytes());
    h.finalize()[..8].try_into().unwrap_or_default()
}

/// The receipts a window holds: at most [`Receipts::CAP`], reserved once;
/// at the cap the smallest `(at_ms, id)` goes.
pub(crate) struct Receipts(Vec<Receipt>);

impl Default for Receipts {
    fn default() -> Self {
        Receipts(Vec::with_capacity(Receipts::CAP))
    }
}

impl Receipts {
    pub(crate) const CAP: usize = 256;

    /// `r`, in place of the same save's.
    pub(crate) fn put(&mut self, r: Receipt) {
        // STAND-IN (tests-first): kept as main's record of saves was: at
        // the cap the first in goes.
        if self.0.len() <= Self::CAP {
            if let Some(had) = self.0.iter_mut().find(|e| e.same(&r)) {
                *had = r;
                return;
            }
            if self.0.len() == Self::CAP {
                self.0.remove(0);
            }
            self.0.push(r);
            return;
        }
        if let Some(had) = self.0.iter_mut().find(|e| e.same(&r)) {
            *had = r;
            return;
        }
        if self.0.len() == Self::CAP
            && let Some(i) = self.0.iter().enumerate().min_by_key(|(_, e)| e.order()).map(|(i, _)| i)
        {
            self.0.swap_remove(i);
        }
        self.0.push(r);
    }

    pub(crate) fn app(&self, k: AppKey) -> Option<&Receipt> {
        self.0.iter().find(|r| r.scope == Scope::App && r.key == k.key())
    }

    pub(crate) fn daemon(&self, d: [u8; 8], key: &str) -> Option<&Receipt> {
        self.0.iter().find(|r| r.scope == Scope::Daemon && r.daemon == d && r.key == key)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// Those no write has carried yet (and those whose write failed).
    pub(crate) fn due(&self) -> Vec<Receipt> {
        self.0.iter().filter(|r| r.due()).copied().collect()
    }

    #[cfg(test)]
    pub(crate) fn any_due(&self) -> bool {
        self.0.iter().any(Receipt::due)
    }

    /// `rs` are in the file now (each unless a newer save replaced it).
    fn written(&mut self, rs: &[Receipt]) {
        for e in self.0.iter_mut() {
            if rs.iter().any(|r| r.same(e) && r.at_ms == e.at_ms && r.digest == e.digest) {
                e.flags = 0;
            }
        }
    }

    /// The write carrying `rs` failed: each still due, and failed.
    fn not_written(&mut self, rs: &[Receipt]) {
        for e in self.0.iter_mut().filter(|e| e.due() && rs.iter().any(|r| r.same(e))) {
            e.flags |= FAILED;
        }
    }

    /// The file's, read again: each replaces this window's, unless this
    /// window's is due or failed (the file doesn't have it yet).
    pub(crate) fn merge_file(&mut self, file: Vec<Receipt>) {
        for r in file {
            if !self.0.iter().any(|e| e.same(&r) && e.due()) {
                self.put(r);
            }
        }
    }

    /// The bytes it holds: its `Vec` and its reserve.
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        std::mem::size_of::<Vec<Receipt>>() + self.0.capacity() * std::mem::size_of::<Receipt>()
    }
}

// ── the file ──────────────────────────────────────────────────────────

/// The receipts' key in settings.json: `{"v": 1, "e": {"<id>": [at_ms, "<digest>"]}}`.
pub(crate) const STORE: &str = "settings.saved";

/// `rs` merged into the file's receipts `v`, per entry (every other entry
/// kept, a key this build doesn't know too), then down to
/// [`Receipts::CAP`], the smallest `(at_ms, id)` first.
pub(crate) fn write_into(v: &mut Value, rs: &[Receipt]) {
    // STAND-IN (tests-first): no receipt is written.
    if rs.is_empty() || v.is_null() || !v.is_null() {
        return;
    }
    let ok = v.get(STORE).is_some_and(|s| s.get("v").and_then(Value::as_u64) == Some(1) && s.get("e").is_some_and(Value::is_object));
    if !ok {
        v[STORE] = serde_json::json!({"v": 1, "e": {}});
    }
    let Some(e) = v[STORE]["e"].as_object_mut() else { return };
    for r in rs {
        e.insert(r.id(), serde_json::json!([r.at_ms, format!("{:016x}", r.digest)]));
    }
    if e.len() > Receipts::CAP {
        let at = |x: &Value| x.get(0).and_then(Value::as_i64).unwrap_or(i64::MIN);
        let mut order: Vec<(i64, String)> = e.iter().map(|(id, x)| (at(x), id.clone())).collect();
        order.sort_unstable();
        for (_, id) in order.into_iter().take(e.len() - Receipts::CAP) {
            e.remove(&id);
        }
    }
}

/// The file's receipts for this build's keys (at most [`Receipts::CAP`]).
pub(crate) fn read_from(v: &Value) -> Vec<Receipt> {
    // STAND-IN (tests-first): no receipt is read.
    if v.is_null() || !v.is_null() {
        return Vec::new();
    }
    let Some(e) = v.get(STORE).filter(|s| s.get("v").and_then(Value::as_u64) == Some(1)).and_then(|s| s.get("e")?.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, x) in e {
        let (Some(at_ms), Some(d)) = (x.get(0).and_then(Value::as_i64), x.get(1).and_then(Value::as_str)) else { continue };
        let Ok(digest) = u64::from_str_radix(d, 16) else { continue };
        let r = if let Some(k) = id.strip_prefix("app#").and_then(AppKey::of) {
            Receipt { key: k.key(), daemon: [0; 8], at_ms, digest, scope: Scope::App, flags: 0 }
        } else if let Some((d, key)) = id.strip_prefix("daemon:").and_then(|r| r.split_once('#')) {
            // Only a key the dash writes has a receipt.
            let (Some(def), Some(daemon)) = (crate::settings::def(key).filter(|d| crate::settings::is_config(d.key)), unhex(d)) else {
                continue;
            };
            Receipt { key: def.key, daemon, at_ms, digest, scope: Scope::Daemon, flags: 0 }
        } else {
            continue;
        };
        out.push(r);
        if out.len() == Receipts::CAP {
            break;
        }
    }
    out
}

fn unhex(s: &str) -> Option<[u8; 8]> {
    if s.len() != 16 {
        return None;
    }
    let mut out = [0; 8];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The values of `batch` and their receipts, and the receipts `due`, in
/// one write under the settings file's lock ([`theme::try_update_settings`]):
/// the values set (`Null` removes a key), the time taken then, one receipt
/// per value merged into the file's. Nothing, if `stale()` (a later write
/// holds these). The receipts written.
pub(crate) fn write_batch(path: &Path, batch: &AppBatch, due: &[Receipt], stale: impl FnOnce() -> bool) -> std::io::Result<Vec<Receipt>> {
    let mut written = Vec::new();
    theme::update_settings_if(path, |v| {
        if stale() {
            return Ok(false);
        }
        let at = now_ms();
        for (k, val) in batch.iter() {
            match val {
                AppValue::Null => {
                    if let Some(o) = v.as_object_mut() {
                        o.remove(k.key());
                    }
                }
                val => v[k.key()] = val.json(),
            }
            written.push(Receipt::app(k, val, at));
        }
        written.extend(due.iter().map(|r| Receipt { flags: 0, ..*r }));
        write_into(v, &written);
        Ok(true)
    })?;
    Ok(written)
}

/// The file's receipts at `path` (none: no file, or none in it).
pub(crate) fn load(path: &Path) -> Vec<Receipt> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map_or_else(Vec::new, |v| read_from(&v))
}

// ── time ──────────────────────────────────────────────────────────────

#[cfg(test)]
thread_local! {
    static CLOCK: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Wall-clock milliseconds (a test's own clock, while it sets one).
pub(crate) fn now_ms() -> i64 {
    #[cfg(test)]
    if let Some(t) = CLOCK.get() {
        return t;
    }
    chrono::Utc::now().timestamp_millis()
}

/// Tests: this thread's wall clock stands at `ms` (`None`: the real one).
#[cfg(test)]
pub(crate) fn set_clock(ms: Option<i64>) {
    CLOCK.set(ms);
}

/// A save's time as a row shows it, in local time: `13:58` today, `Thu`
/// this week, `27 Nov` before.
pub(crate) fn when(at_ms: i64, now_ms: i64) -> String {
    use chrono::TimeZone as _;
    let (Some(at), Some(now)) = (chrono::Local.timestamp_millis_opt(at_ms).single(), chrono::Local.timestamp_millis_opt(now_ms).single())
    else {
        return String::new();
    };
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        0 => at.format("%H:%M").to_string(),
        1..=6 => at.format("%a").to_string(),
        _ => at.format("%-d %b").to_string(),
    }
}

// ── the writer ────────────────────────────────────────────────────────

/// A writer's failure, under its rows: its keys as bits, its words (at
/// most [`Note::MAX`] bytes, then `…`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Note {
    pub(crate) keys: u16,
    pub(crate) why: Box<str>,
}

impl Note {
    pub(crate) const MAX: usize = 256;

    pub(crate) fn new(keys: u16, why: &str) -> Note {
        Note { keys, why: cut(why, Note::MAX).into() }
    }
}

/// `s` in at most `max` bytes, cut at a character with `…` when longer.
pub(crate) fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max - '…'.len_utf8();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// The window's settings writer (PLAN §3.6 Part 2): what was changed and
/// isn't written yet, what failed, and the receipts. It outlives the
/// Settings tab; the window's close is its last write.
#[derive(Default)]
pub(crate) struct SettingsWriter {
    /// THIS APP keys waiting for their save ([`crate::app::SAVE_DELAY`]
    /// after the last change, all in one write, in order), and that save.
    pub(crate) app_dirty: AppBatch,
    pub(crate) app_save: Option<Task<()>>,
    /// The batch being written off the UI thread, shared with its job, and
    /// its stop: a close writes it again with what came after it, and the
    /// late write then changes nothing (checked under the file's lock).
    pub(crate) app_writing: Option<(Arc<AppBatch>, Arc<AtomicBool>)>,
    /// THIS APP changes so far, and the last that a write saved (with when
    /// it finished): a value's afterglow starts there, never on a guess.
    pub(crate) app_gen: u64,
    pub(crate) app_wrote: Option<(u64, Instant)>,
    /// What failed: the app values' write, the panels', the receipts'.
    pub(crate) app_note: Option<Note>,
    pub(crate) panels_note: Option<Note>,
    pub(crate) receipts_note: Option<Note>,
    /// The receipts' own write ([`crate::app::SAVE_DELAY`] after a daemon
    /// value's 200), when no value write carries them first.
    pub(crate) receipts_save: Option<Task<()>>,
    /// The receipts.
    pub(crate) wrote: Receipts,
}

impl AppView {
    /// A THIS APP key, saved [`crate::app::SAVE_DELAY`] after the last
    /// change with the others waiting, in one write off the UI thread (two
    /// quick changes of a key cannot land out of order). A failed write
    /// waits in it too, and goes with this one.
    pub(crate) fn save_app(&mut self, key: AppKey, value: AppValue, cx: &mut Context<Self>) {
        if theme::settings_path().is_none() {
            self.settings.app_note = Some(Note::new(AppKeySet::default().with(key).0, "not saved: no config folder"));
            return;
        }
        self.settings.app_gen += 1;
        // STAND-IN (tests-first): written at once, as before this PR: no batch, no receipt.
        // Each value as its JSON, a null too.
        if let Some(path) = theme::settings_path() {
            let json = value.json();
            if let Err(e) = theme::try_update_settings(&path, |v| {
                v[key.key()] = json;
                Ok(())
            }) {
                self.settings.app_note = Some(Note::new(AppKeySet::default().with(key).0, &format!("not saved: {e}")));
            }
            self.views_changed(cx);
            return;
        }
        self.settings.app_dirty.set(key, value);
        self.arm_app_save(cx);
    }

    fn arm_app_save(&mut self, cx: &mut Context<Self>) {
        self.settings.app_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::app::SAVE_DELAY).await;
            let _ = this.update(cx, |v, cx| v.write_app(cx));
        }));
    }

    /// A daemon value's receipt is due: written by the next app write, or
    /// by a write of its own [`crate::app::SAVE_DELAY`] from now.
    pub(crate) fn arm_receipts_save(&mut self, cx: &mut Context<Self>) {
        self.settings.receipts_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::app::SAVE_DELAY).await;
            let _ = this.update(cx, |v, cx| v.write_app(cx));
        }));
    }

    /// What failed, tried again now (Retry, the window back to the front).
    pub(crate) fn retry_saves(&mut self, cx: &mut Context<Self>) {
        if self.settings.app_note.is_some() || self.settings.receipts_note.is_some() {
            self.write_app(cx);
        }
        if self.settings.panels_note.is_some() {
            self.save_panels_now(cx);
        }
    }

    /// The batch and the due receipts, written off the UI thread.
    pub(crate) fn write_app(&mut self, cx: &mut Context<Self>) {
        if self.settings.app_writing.is_some() {
            // The last write is still out: this one goes after it.
            return self.arm_app_save(cx);
        }
        self.settings.app_save = None;
        self.settings.receipts_save = None;
        let due = self.settings.wrote.due();
        if self.settings.app_dirty.is_empty() && due.is_empty() {
            return;
        }
        let Some(path) = theme::settings_path() else { return };
        let batch = Arc::new(std::mem::take(&mut self.settings.app_dirty));
        let stop = Arc::new(AtomicBool::new(false));
        let generation = self.settings.app_gen;
        self.settings.app_writing = Some((batch.clone(), stop.clone()));
        #[cfg(test)]
        let gate = gate::taken();
        cx.spawn(async move |this, cx| {
            let job = {
                let (batch, stop, due) = (batch.clone(), stop.clone(), due.clone());
                async move {
                    #[cfg(test)]
                    if let Some(g) = gate {
                        let _ = g.recv().await;
                    }
                    write_batch(&path, &batch, &due, || stop.load(Ordering::SeqCst))
                }
            };
            let saved = cx.background_spawn(job).await;
            let _ = this.update(cx, |v, cx| v.app_written(&batch, &stop, &due, generation, saved, cx));
        })
        .detach();
    }

    fn app_written(
        &mut self,
        batch: &AppBatch,
        stop: &AtomicBool,
        due: &[Receipt],
        generation: u64,
        saved: std::io::Result<Vec<Receipt>>,
        cx: &mut Context<Self>,
    ) {
        // A close took it (and wrote it again): nothing of it is this one's.
        if stop.load(Ordering::SeqCst) {
            return;
        }
        self.settings.app_writing = None;
        let theme_too = [AppKey::Theme, AppKey::Accent].into_iter().any(|k| batch.get(k).is_some());
        match saved {
            Ok(written) => {
                self.settings.wrote.written(&written);
                for r in written.iter().filter(|r| r.scope == Scope::App) {
                    self.settings.wrote.put(*r);
                }
                if !batch.is_empty() {
                    self.settings.app_wrote = Some((generation, self.fx.now()));
                    self.settings.app_note = None;
                }
                if !due.is_empty() {
                    self.settings.receipts_note = None;
                }
                if theme_too {
                    self.picker_note = Some(crate::app::saved_note(theme::settings_path().map(|p| (Ok(()), p)), &self.picker_overridden()));
                }
            }
            Err(e) => {
                // Back under what came since: tried again with the next
                // change, the window's return, or Retry.
                self.settings.app_dirty.merge_older(batch);
                if !batch.is_empty() {
                    self.settings.app_note = Some(Note::new(batch.keys().0, &format!("not saved: {e}")));
                }
                if !due.is_empty() {
                    self.settings.wrote.not_written(due);
                    self.settings.receipts_note = Some(Note::new(0, &format!("save time not kept: {e}")));
                }
                if theme_too {
                    self.picker_note = Some(format!("not saved: {e}").into());
                }
            }
        }
        self.views_changed(cx);
    }

    /// What waits, written now: the window is closing (within the close's
    /// deadline, [`theme::closing`]). A batch still being written is stopped
    /// and goes again under what came after it (it never lands last).
    pub(crate) fn flush_app(&mut self) {
        self.settings.app_save = None;
        self.settings.receipts_save = None;
        let mut batch = std::mem::take(&mut self.settings.app_dirty);
        if let Some((out, stop)) = self.settings.app_writing.take() {
            stop.store(true, Ordering::SeqCst);
            batch.merge_older(&out);
        }
        let due = self.settings.wrote.due();
        if self.bench.is_some() || (batch.is_empty() && due.is_empty()) {
            return;
        }
        let Some(path) = theme::settings_path() else { return };
        match write_batch(&path, &batch, &due, || false) {
            Ok(written) => self.settings.wrote.written(&written),
            Err(e) => {
                let mut keys: Vec<&str> = batch.keys().names().collect();
                if !due.is_empty() {
                    keys.push("save times");
                }
                theme::not_saved_at_close(&keys, &e);
            }
        }
    }

    /// The value an app key holds in this window, as its write would
    /// carry it (its receipt's time shows while the two agree).
    pub(crate) fn app_value(&self, k: AppKey, cx: &gpui_kit::App) -> Option<AppValue> {
        if let Some(v) = self.settings.app_dirty.get(k) {
            return Some(v.clone());
        }
        let s = &self.app_settings;
        let str_or_null = |v: Option<&str>| AppValue::of(v.map(String::from));
        Some(match k {
            AppKey::Theme => match self.theme_follows_config {
                true => AppValue::Null,
                false => AppValue::Str(cx.global::<theme::Scheme>().choice.theme.as_str().into()),
            },
            AppKey::Accent => AppValue::of(cx.global::<theme::Scheme>().choice.accent.map(theme::to_hex)),
            AppKey::TextSize => AppValue::Num(self.panels.text_size.into()),
            AppKey::Motion => AppValue::of(cx.try_global::<crate::motion::Setting>().and_then(|s| s.level).map(|l| l.key().to_string())),
            AppKey::Notify => AppValue::Str(if s.quiet { "off" } else { "on" }.into()),
            AppKey::Remember => AppValue::Str(if self.recall.on() { "on" } else { "off" }.into()),
            AppKey::Rail => AppValue::Bool(self.panels.rail_collapsed),
            AppKey::Right => AppValue::Bool(self.panels.right_collapsed),
            AppKey::DaemonLabel => str_or_null(s.daemon_label.as_deref()),
            AppKey::DashUrl => AppValue::of(s.dash_url.as_deref().map(crate::dash::strip_token)),
            AppKey::UsageRange => AppValue::Str(s.usage_range.key().into()),
            AppKey::UsageHost => str_or_null(s.usage_host.as_deref()),
            AppKey::Split | AppKey::PickersSource => return None,
        })
    }

    /// When this app last saved the value app key `k` holds now
    /// (`saved 13:58`), while it is still that value.
    pub(crate) fn app_saved_at(&self, k: AppKey, cx: &gpui_kit::App) -> Option<String> {
        let r = self.settings.wrote.app(k)?;
        let v = self.app_value(k, cx)?;
        (r.digest == digest(Scope::App, k.key(), &v.canonical())).then(|| format!("saved {}", when(r.at_ms, now_ms())))
    }

    /// The same for a daemon key holding `value` in the config shown.
    pub(crate) fn daemon_saved_at(&self, key: &str, value: Option<&str>) -> Option<&Receipt> {
        let c = self.shown_config()?;
        self.settings.wrote.daemon(self.config_owner(c), key).filter(|r| r.digest == daemon_digest(key, value))
    }

    /// The receipts in the file, read again off the UI thread as Settings
    /// opens; this window's due and failed ones are kept over them.
    pub(crate) fn read_receipts(&mut self, cx: &mut Context<Self>) {
        let Some(path) = theme::settings_path() else { return };
        cx.spawn(async move |this, cx| {
            let file = cx.background_spawn(async move { load(&path) }).await;
            let _ = this.update(cx, |v, cx| {
                v.settings.wrote.merge_file(file);
                v.views_changed(cx);
            });
        })
        .detach();
    }

    /// The bytes the writer holds, its parts each counted (PLAN §3.6 Part
    /// 2): the batches and their values, the stop flag, the notes, the
    /// receipts, and the daemon queue.
    #[cfg(test)]
    pub(crate) fn retained_writer(&self) -> usize {
        let SettingsWriter {
            app_dirty,
            app_save,
            app_writing,
            app_gen,
            app_wrote,
            app_note,
            panels_note,
            receipts_note,
            receipts_save,
            wrote,
        } = &self.settings.writer;
        let _ = (app_save, app_gen, app_wrote, receipts_save);
        let arc = 2 * std::mem::size_of::<usize>();
        let in_flight = app_writing
            .as_ref()
            .map_or(0, |(b, _)| arc + std::mem::size_of::<AppBatch>() + b.heap() + arc + std::mem::size_of::<AtomicBool>().max(8));
        let notes = [app_note, panels_note, receipts_note].iter().map(|n| n.as_ref().map_or(0, |n| n.why.len())).sum::<usize>();
        std::mem::size_of::<SettingsWriter>() + app_dirty.heap() + in_flight + notes + wrote.retained() + self.dash.order.retained()
    }
}

/// Tests: a batch held in flight. Its job waits here, before its write,
/// until the test lets it go (a gate that blocked inside the write would
/// stop the test's own thread, which runs the background tasks).
#[cfg(test)]
pub(crate) mod gate {
    thread_local! {
        static NEXT: std::cell::RefCell<Option<async_channel::Receiver<()>>> = const { std::cell::RefCell::new(None) };
    }

    /// The next app write waits until the sender sends or goes.
    pub(crate) fn hold() -> async_channel::Sender<()> {
        let (tx, rx) = async_channel::bounded(1);
        NEXT.with(|n| *n.borrow_mut() = Some(rx));
        tx
    }

    pub(crate) fn taken() -> Option<async_channel::Receiver<()>> {
        NEXT.with(|n| n.borrow_mut().take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest is the same in every process and build: a golden value.
    #[test]
    fn the_digest_is_stable_across_processes() {
        assert_eq!(format!("{:016x}", digest(Scope::App, "theme", "\"nord\"")), "db9ebe0ccf7a5392");
        assert_eq!(format!("{:016x}", daemon_digest("model", None)), format!("{:016x}", digest(Scope::Daemon, "model", "null")));
        assert_ne!(digest(Scope::App, "theme", "\"nord\""), digest(Scope::Daemon, "theme", "\"nord\""), "the scope is in it");
        assert_eq!(AppValue::Num(18).canonical(), "18");
        assert_eq!(AppValue::Bool(false).canonical(), "false");
        assert_eq!(AppValue::Str("a\"b".into()).canonical(), r#""a\"b""#);
    }

    fn at(key: &'static str, at_ms: i64) -> Receipt {
        Receipt::daemon([7; 8], key, Some("x"), at_ms)
    }

    /// At the cap, the smallest `(at_ms, id)` goes; at a tie, the smaller
    /// id. In the file the same.
    #[test]
    fn the_257th_receipt_evicts_the_oldest_deterministically() {
        let keys: Vec<&'static str> = crate::settings::ALL.iter().map(|d| d.key).collect();
        let mut rs = Receipts::default();
        let mut file = serde_json::json!({});
        let mut all = Vec::new();
        for i in 0..Receipts::CAP {
            let r = Receipt::daemon([i as u8, 1, 0, 0, 0, 0, 0, 0], keys[0], Some("x"), 1_000 + i as i64);
            rs.put(r);
            all.push(r);
        }
        write_into(&mut file, &all);
        assert_eq!(rs.len(), 256);
        let next = at(keys[1], 5_000);
        rs.put(next);
        write_into(&mut file, &[next]);
        assert_eq!(rs.len(), 256, "still 256");
        assert!(rs.daemon([0, 1, 0, 0, 0, 0, 0, 0], keys[0]).is_none(), "the oldest went");
        assert!(rs.daemon([1, 1, 0, 0, 0, 0, 0, 0], keys[0]).is_some(), "the next oldest stays");
        let e = file[STORE]["e"].as_object().unwrap();
        assert_eq!(e.len(), 256);
        assert!(!e.contains_key(&all[0].id()) && e.contains_key(&all[1].id()) && e.contains_key(&next.id()), "the file the same");
        // A tie: the smaller id goes.
        let mut rs = Receipts::default();
        for i in 0..Receipts::CAP {
            rs.put(Receipt::daemon([0; 8], keys[i % keys.len()], Some("x"), 9 + (i / keys.len()) as i64 * 100));
        }
        let mut first = keys[..].to_vec();
        first.sort_unstable();
        let small = first[0];
        rs.put(Receipt::app(AppKey::Theme, &AppValue::Null, 9_999));
        assert!(rs.daemon([0; 8], small).is_none_or(|r| r.at_ms != 9), "the smallest id at the oldest time went");
        let mut file = serde_json::json!({});
        let tie: Vec<Receipt> =
            (0..=Receipts::CAP).map(|i| Receipt::daemon([i as u8, (i >> 8) as u8, 0, 0, 0, 0, 0, 0], keys[0], None, 5)).collect();
        write_into(&mut file, &tie);
        let e = file[STORE]["e"].as_object().unwrap();
        let mut ids: Vec<String> = tie.iter().map(Receipt::id).collect();
        ids.sort_unstable();
        assert!(!e.contains_key(&ids[0]) && e.contains_key(&ids[1]) && e.len() == 256, "a tie goes by id");
    }

    /// Ids are a fixed size whatever the paths: ≤ 72 bytes; the store is
    /// exactly 12,312 B. (Only the dash's 22 keys get a daemon receipt:
    /// some read-only keys are longer than an id has room for.)
    #[test]
    fn receipt_ids_are_fixed_size() {
        let long = "/".repeat(4096);
        let d = daemon_id(Some(Path::new(&long)), Some(&"p".repeat(4096)), Some(&long));
        let mut rs = Receipts::default();
        let writable = || crate::settings::CONFIG_GROUPS.iter().flat_map(|(_, keys)| keys.iter().copied());
        let longest = writable().max_by_key(|k| k.len()).unwrap();
        let ids = writable()
            .map(|k| Receipt::daemon(d, k, Some(&long), 1))
            .chain(AppKey::ALL.map(|k| Receipt::app(k, &AppValue::Str(long.clone().into()), 1)));
        for r in ids {
            assert!(r.id().len() <= 72, "{}: {} bytes", r.id(), r.id().len());
        }
        let mut file = serde_json::json!({});
        let r = Receipt::daemon(d, longest, Some(&long), 1);
        write_into(&mut file, &[r]);
        for id in file[STORE]["e"].as_object().unwrap().keys() {
            assert!(id.len() <= 72, "in the file: {id}");
        }
        for i in 0..300 {
            rs.put(Receipt::daemon([i as u8, (i >> 8) as u8, 0, 0, 0, 0, 0, 0], longest, Some(&long), i));
        }
        assert_eq!(std::mem::size_of::<Receipt>(), 48);
        assert_eq!(rs.retained(), 12_312, "256 × 48 B and the Vec");
    }

    /// An app key's slot holds its latest value; a failed batch merged back
    /// under a newer one keeps one slot per key, the newer value in it.
    #[test]
    fn a_batch_keeps_one_slot_per_key() {
        let mut failed = AppBatch::default();
        failed.set(AppKey::Theme, AppValue::Str("nord".into()));
        failed.set(AppKey::DaemonLabel, AppValue::Str("jade".into()));
        let mut pending = AppBatch::default();
        pending.set(AppKey::Theme, AppValue::Str("gruvbox".into()));
        pending.merge_older(&failed);
        assert_eq!(pending.get(AppKey::Theme), Some(&AppValue::Str("gruvbox".into())), "the newer wins");
        assert_eq!(pending.get(AppKey::DaemonLabel), Some(&AppValue::Str("jade".into())), "the rest comes back");
        assert_eq!(pending.iter().count(), 2, "one slot per key");
        assert_eq!(pending.keys().names().collect::<Vec<_>>(), ["theme", "daemon_label"]);
    }

    /// A note holds at most 256 B, cut at a character.
    #[test]
    fn notes_are_capped() {
        let n = Note::new(0, &"é".repeat(300));
        assert!(n.why.len() <= Note::MAX && n.why.ends_with('…'), "{}", n.why.len());
    }

    /// The time, as a row shows it: today, this week, before.
    #[test]
    fn a_time_reads_by_its_day() {
        use chrono::TimeZone as _;
        let t = |d, h, m| chrono::Local.with_ymd_and_hms(2026, 11, d, h, m, 0).single().unwrap().timestamp_millis();
        assert_eq!(when(t(26, 13, 58), t(26, 18, 0)), "13:58");
        assert_eq!(when(t(26, 13, 58), t(28, 9, 0)), "Thu");
        assert_eq!(when(t(12, 13, 58), t(28, 9, 0)), "12 Nov");
    }
}
