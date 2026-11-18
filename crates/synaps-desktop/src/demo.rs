//! `--offline`: a synthetic transport. It feeds the window the same `Event`s
//! a daemon connection would (decoded from wire JSON by `synaps_client`'s own
//! decoders, then through the pump and the model), so every widget it fills
//! is one the live daemon fills the same way. Nothing here reaches a widget
//! by any other path, and nothing it does not send (plan steps, burn rate)
//! is shown.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use synaps_client::protocol::{DaemonFrame, SessionMeta, decode_event, decode_frame};
use synaps_client::{AttachMode, CatalogCursor, Event, synthetic};

const CWD: &str = "/home/you/work/relay-gateway";
/// The session `--offline` opens: it waits on a tool-call confirmation.
pub(crate) const SID: &str = "20261113-041207-a1f0";
/// A parked session whose turns are over: runs that fold (one with a
/// failed call, one all ok), an edit's diff in each.
pub(crate) const IDLE_CPU: &str = "20261113-161706-a1b9";
/// A busy session whose turn is running a call.
pub(crate) const FLAKY: &str = "20261113-033001-c3d9";
/// A session from before message times: its history has none.
pub(crate) const LEGACY: &str = "20261111-201530-0a6e";

/// Message times (`_synaps_at`) on a demo history, as the host records
/// them: from the local wall time in the session's id on, `step` seconds
/// apart; from message `resumed` on (a session taken up again), the next
/// morning at 09:12. Fixed local times: the same rows and day lines on any
/// machine, at any hour.
fn stamp(msgs: &mut [Value], id: &str, step: i64, resumed: Option<usize>) {
    use chrono::{Duration, NaiveDateTime, NaiveTime, SecondsFormat, TimeZone as _, Utc};
    let Some(start) = id.get(..15).and_then(|s| NaiveDateTime::parse_from_str(s, "%Y%m%d-%H%M%S").ok()) else { return };
    let next = (start.date() + Duration::days(1)).and_time(NaiveTime::from_hms_opt(9, 12, 0).unwrap_or_default());
    for (k, m) in msgs.iter_mut().enumerate() {
        let at = match resumed {
            Some(r) if k >= r => next + Duration::seconds(step * (k - r) as i64),
            _ => start + Duration::seconds(step * k as i64),
        };
        if let (Some(t), Some(m)) = (chrono::Local.from_local_datetime(&at).earliest(), m.as_object_mut()) {
            m.insert("_synaps_at".into(), json!(t.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true)));
        }
    }
}

/// RFC 3339 (UTC) of `secs` after the epoch.
pub(crate) fn rfc3339(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The daemon's session list: every rail state but OPENING (the client's
/// own) and UNREAD (the client sees it happen: `settled` is the next poll,
/// in which the single-flight session's turn has ended off screen).
fn sessions(now: i64, settled: bool) -> Vec<SessionMeta> {
    let ago = |min: i64| rfc3339(now - min * 60);
    let opus = "anthropic/claude-opus-4-1";
    let sonnet = "anthropic/claude-sonnet-4-5";
    let rows = [
        json!({"id": "20261101-030919-c3d4", "name": "subconscious", "title": "Day ledger, heartbeat and handoff", "model": opus,
            "cwd": "/home/you/notes", "pinned": true, "lifecycle": "live", "messages": 388, "cost_usd": 3.12, "state_since": ago(12),
            "created_at": ago(18_000)}),
        json!({"id": "20261113-184102-9d0e", "model": opus, "cwd": "/home/you/work/synaps-desktop", "lifecycle": "live", "messages": 0,
            "state_since": ago(0), "created_at": ago(0)}),
        json!({"id": FLAKY, "title": "Flaky e2e: checkout latency", "model": sonnet,
            "cwd": "/home/you/work/shop", "busy": true, "cost_usd": 0.45, "lifecycle": "live", "messages": 12, "state_since": ago(7),
            "created_at": ago(30)}),
        json!({"id": SID, "name": "relay-refresh", "title": "Refresh race in session relay", "model": opus,
            "cwd": CWD, "busy": true, "awaiting_input": 1, "cost_usd": 4.95, "lifecycle": "live", "clients": 1, "messages": 9,
            "state_since": ago(4), "created_at": ago(90),
            "roster": [{"id": 2, "name": "desktop", "kind": "rpc", "mode": "mirror", "attached_at_ms": 0}]}),
        json!({"id": "20261113-161706-a1b9", "title": "Idle CPU: hybrid against main", "model": sonnet, "cwd": "/home/you/work/synaps-desktop",
            "lifecycle": "parked", "messages": 64, "cost_usd": 0.88, "state_since": ago(38), "evict_due": rfc3339(now + 22 * 60),
            "created_at": ago(120)}),
        json!({"id": "20261113-035512-77c2", "name": "ingest-core", "title": "Migrate ingest workers to async", "model": sonnet,
            "cwd": "/home/you/work/ingest", "busy": true, "awaiting_input": 2, "cost_usd": 1.45, "lifecycle": "live", "messages": 31,
            "state_since": ago(18), "created_at": ago(200)}),
        json!({"id": "20261113-151233-8f70", "title": "Saved-sessions query: single-flight load", "model": opus,
            "cwd": "/home/you/work/runtime", "busy": !settled, "cost_usd": 6.12, "lifecycle": "live",
            "messages": if settled { 243 } else { 241 }, "state_since": ago(if settled { 0 } else { 13 }),
            "park_due": rfc3339(now + 50), "created_at": ago(300)}),
        json!({"id": "20261113-021944-0b1e", "title": "Bisect allocator regression", "model": opus,
            "cwd": "/home/you/work/alloc", "cost_usd": 2.17, "lifecycle": "parked", "messages": 58, "state_since": ago(51),
            "locked_by": "pid 48213 (tui)", "created_at": ago(400)}),
        json!({"id": "20261112-224410-9f00", "name": "ledger-drift", "title": "Schema drift report: ledger", "model": "anthropic/claude-haiku-4-5",
            "cwd": "/home/you/work/ledger", "cost_usd": 0.09, "lifecycle": "live", "clients": 1, "messages": 6, "state_since": ago(130),
            // Its turn is over, two of its subagents still run (WORKERS, not idle).
            "subagents_running": 2,
            "created_at": ago(800), "roster": [{"id": 3, "name": "tui", "kind": "tui", "mode": "mirror", "attached_at_ms": 0}]}),
        json!({"id": "20261112-180203-44aa", "title": "Operator handbook, chapter 4", "model": sonnet,
            "cwd": "/home/you/docs", "cost_usd": 0.27, "lifecycle": "parked", "messages": 22, "state_since": ago(190),
            "created_at": ago(1500)}),
        // Enough sessions for a tab strip that overflows on a wide window.
        json!({"id": "20261112-151010-5c21", "name": "docs-sync", "title": "Sync API docs with the wire spec", "model": sonnet,
            "cwd": "/home/you/work/docs", "cost_usd": 0.31, "lifecycle": "live", "messages": 14, "state_since": ago(240),
            "created_at": ago(1700)}),
        json!({"id": "20261112-120455-e7d3", "title": "CI cache misses on the arm runners", "model": opus,
            "cwd": "/home/you/work/ci", "cost_usd": 1.02, "lifecycle": "live", "messages": 27, "state_since": ago(300),
            "created_at": ago(1900)}),
        json!({"id": "20261111-201530-0a6e", "name": "release-notes", "title": "Release notes for 0.12", "model": sonnet,
            "cwd": "/home/you/work/runtime", "cost_usd": 0.58, "lifecycle": "parked", "messages": 19, "state_since": ago(900),
            "created_at": ago(2600)}),
    ];
    rows.into_iter().filter_map(|r| serde_json::from_value(r).ok()).collect()
}

/// One page of the synthetic daemon's saved sessions (`CatalogPage`,
/// decoded by the client's own decoder), as `catalog_page(before, limit,
/// named_only)` would answer `req`: saved rows only (a held session is
/// never in a page), newest `updated_at` first, then `id`; `before`
/// exclusive; `next` while rows are left.
pub fn catalog_page(req: u64, before: Option<&CatalogCursor>, limit: u32, named_only: bool) -> Event {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let key = |v: &Value| (v["updated_at"].as_str().unwrap_or_default().to_string(), v["id"].as_str().unwrap_or_default().to_string());
    let after = |v: &Value| before.is_none_or(|c| key(v) < (c.updated_at.clone(), c.id.clone()));
    let rows: Vec<Value> = catalog(now).into_iter().filter(|v| (!named_only || v["name"].is_string()) && after(v)).collect();
    let limit = limit.clamp(1, synaps_client::CATALOG_PAGE_MAX) as usize;
    let more = rows.len() > limit;
    let page: Vec<Value> = rows.into_iter().take(limit).collect();
    let next = if more { page.last().map(|v| json!({"updated_at": v["updated_at"], "id": v["id"]})) } else { None };
    let frame = json!({"type": "catalog_page", "req": req, "sessions": page, "next": next});
    match decode_frame(&frame.to_string()) {
        Ok(DaemonFrame::CatalogPage { req, sessions, next, rejected, error: None }) => Event::CatalogPage { req, sessions, next, rejected },
        other => Event::CatalogPageFailed { req, reason: format!("synthetic catalog page did not decode: {other:?}") },
    }
}

/// The synthetic daemon's saved rows (`SessionMeta`, `lifecycle: saved`),
/// newest first. The held `ledger-drift` is not one.
fn catalog(now: i64) -> Vec<Value> {
    let named = [
        ("myx", "Mixer: per-channel gain staging", 3 * 3600),
        ("reload-bugs", "Daemon reload: owned sessions and aliases", 11 * 3600),
        ("syn", "Synaps CLI: release checklist", 3 * 86_400),
        ("watchcard-test", "Watch card: worker rows at 1 Hz", 5 * 86_400),
        ("smoke-ab-rel2", "Smoke A/B, release build, second run", 9 * 86_400),
        ("smoke-ab-rel", "Smoke A/B, release build", 9 * 86_400 + 900),
        ("smoke-ab-new", "Smoke A/B, new build", 9 * 86_400 + 2000),
        ("subconscious-old", "Day ledger, heartbeat and handoff", 15 * 86_400),
        ("handbook", "Operator handbook, chapter 3", 21 * 86_400),
        ("kite", "Kite: offline maps", 40 * 86_400),
        ("bebop", "Bebop: audio routing", 90 * 86_400),
    ];
    let titles = [
        "Fork: relay refresh race",
        "Review PR #5: outcomes, per-attachment answers",
        "Accent floors in OKLCH: 4.5 and 3.0",
        "Subset Rajdhani and Plex, rename the families",
        "Bench: launch to first frame, loaded daemon",
        "Do owned sessions survive a reload?",
        "Prompt answers on the wire: fixtures",
        "GPUI or Tauri for the desktop client",
        "Corp demo: six looks, one accent",
        "Tolerant decoding: unknown frames and events",
        "Rename refusals, for the user",
        "Sessions modal: facets and day groups",
        "Multi-account broker: refresh locks",
        "TTFT: cold start against llama.cpp",
        "Forum: tombstones and exact retries",
        "Compaction chains: resolve the head exactly",
        "Park grace and eviction timers",
        "Decode ceiling: streaming read",
        "Flash attention: fallback tests",
        "Keep-warm pings: what they cost",
        "Retention: thirty days by default",
        "Input ownership: shared or exclusive",
        "Subagent rows after the turn ends",
        "Session lock: live in another process",
        "Journal generations and snapshots",
        "Title from the first user message",
        "Cost cap per session",
    ];
    let folders = ["/home/you/work/runtime", "/home/you/work/synaps-desktop", "/home/you/work/relay-gateway", "/home/you/notes"];
    let mut rows: Vec<(i64, Value)> = named
        .iter()
        .enumerate()
        .map(|(i, (name, title, age))| {
            (
                now - age,
                json!({"id": format!("20261020-0900{i:02}-n{i:03}"), "name": name, "title": title, "model": "anthropic/claude-opus-4-1",
                "cwd": folders[i % folders.len()], "messages": 40 + i * 37, "updated_at": rfc3339(now - age), "lifecycle": "saved"}),
            )
        })
        .collect();
    // 7 today, 14 this week, 30 this month, 69 older.
    for i in 0..120i64 {
        let age = match i {
            0..7 => 2 * 3600 + i * 2 * 3600,
            7..21 => 86_400 + (i - 7) * 36_000,
            21..51 => 7 * 86_400 + (i - 21) * 70_000,
            _ => 30 * 86_400 + (i - 51) * 400_000,
        };
        let t = titles[i as usize % titles.len()];
        let title = if i < titles.len() as i64 { t.to_string() } else { format!("{t} ({})", i / titles.len() as i64 + 1) };
        rows.push((
            now - age,
            json!({"id": format!("20261001-1000{i:03}-u{i:03}"), "title": title, "model": "anthropic/claude-sonnet-4-5",
            "cwd": folders[i as usize % folders.len()], "messages": 12 + i * 7, "updated_at": rfc3339(now - age), "lifecycle": "saved"}),
        ));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1["id"].as_str().cmp(&a.1["id"].as_str())));
    rows.into_iter().map(|r| r.1).collect()
}

const OLD: &str = "        if self.token.expires_within(REFRESH_SKEW) {
            // Best-effort retry: the store sometimes hands back a token
            // that upstream has already invalidated.
            let token = match self.store.rotate(self.id).await {
                Err(Error::Stale) => self.store.rotate(self.id).await?,
                other => other?,
            };
            self.attach(token);
        }";
const NEW: &str = "        if self.token.expires_within(REFRESH_SKEW) {
            let token = self.store.rotate(self.id).await?;
            self.attach(token);
        }";
const TEST: &str = "use relay::testing::{MockUpstream, Relay};

#[tokio::test(flavor = \"multi_thread\", worker_threads = 8)]
async fn concurrent_refreshes_rotate_once() {
    let upstream = MockUpstream::expiring_in(Duration::from_millis(5));
    let relay = Relay::start(&upstream).await;
    let session = relay.open().await.unwrap();
    let tasks: Vec<_> = (0..32).map(|_| session.clone().refresh()).collect();
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(upstream.rotations(), 1);
    assert!(session.is_live());
}";

/// The relay's read, numbered as the engine's `read` numbers it (`N\t…`,
/// then how many lines it left).
const READ: &str = "41\tpub struct TokenStore {\n42\t    inner: Mutex<Inner>,\n43\t    rotation: Mutex<()>,\n44\t}\n45\t\n\
    46\timpl TokenStore {\n47\t    pub async fn rotate(&self, id: SessionId) -> Result<Token> {\n48\t        let mut inner = self.inner.lock().await;\n\
    49\t        inner.rotate(id).await\n50\t    }\n\n... (164 more lines)";

fn messages() -> Vec<Value> {
    let p = |f: &str| format!("{CWD}/{f}");
    vec![
        json!({"role": "user", "content": "The relay drops sessions when two token refreshes race. Fix it and add a regression test. Keep the public API unchanged."}),
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "Two refreshes both see the token as expiring, both rotate, and the second rotation invalidates the first caller's token. The retry in Session papers over that. Serialize rotation in the store instead.", "signature": "x"},
            {"type": "text", "text": "Looking at the refresh path first."},
            {"type": "tool_use", "id": "toolu_d1", "name": "read", "input": {"path": p("crates/relay/src/token_store.rs"), "offset": 40, "limit": 10}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d1", "content": READ}]}),
        // Two workers: one still running (its rows below), one done.
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_s1", "name": "subagent_start", "input": {"agent": "scout",
                "task": "Find every caller of TokenStore::rotate and say which ones can race."}},
            {"type": "tool_use", "id": "toolu_s2", "name": "subagent_start", "input": {"agent": "reviewer",
                "task": "Review the retry path in Session for lost tokens.", "model": "anthropic/claude-sonnet-4-5"}}]}),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_s1", "content": "{\"agent_name\":\"scout\",\"handle_id\":\"sa_1\",\"status\":\"running\"}"},
            {"type": "tool_result", "tool_use_id": "toolu_s2", "content": "{\"agent_name\":\"reviewer\",\"handle_id\":\"sa_2\",\"status\":\"running\"}"}]}),
        // The reviewer's end, collected: its spawn row says its result.
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_c2", "name": "subagent_collect", "input": {"handle_id": "sa_2"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_c2", "content": json!({
            "handle_id": "sa_2", "agent_name": "reviewer", "status": "completed", "elapsed_secs": 83.0, "tool_count": 9,
            "output": "No token is lost in Session: a failed rotate re-reads the store before it retries.\n\nChecked session.rs and upstream.rs."}).to_string()}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d2", "name": "edit", "input": {"path": p("crates/relay/src/token_store.rs"),
                "old_string": "    pub async fn rotate(&self, id: SessionId) -> Result<Token> {\n        let mut inner = self.inner.lock().await;\n        inner.rotate(id).await\n    }",
                "new_string": "    pub async fn rotate(&self, id: SessionId) -> Result<Token> {\n        let _turn = self.rotation.lock().await;\n        let mut inner = self.inner.lock().await;\n        if let Some(t) = inner.fresh(id) {\n            return Ok(t);\n        }\n        inner.rotate(id).await\n    }"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d2", "content": "Edited crates/relay/src/token_store.rs — replaced 4 line(s) with 8 line(s)"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d3", "name": "edit", "input": {"path": p("crates/relay/src/session.rs"), "old_string": OLD, "new_string": NEW}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d3", "content": format!("Edited crates/relay/src/session.rs — replaced {} line(s) with {} line(s)", OLD.lines().count(), NEW.lines().count())}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d4", "name": "write", "input": {"path": p("crates/relay/tests/refresh_race.rs"), "content": TEST}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d4", "content": "Wrote crates/relay/tests/refresh_race.rs"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d4b", "name": "bash", "input": {"command": "rm crates/relay/src/retry.rs && sed -i '/^mod retry;$/d' crates/relay/src/lib.rs"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d4b", "content": ""}]}),
        json!({"role": "assistant", "content": [
            {"type": "text", "text": "Fix is in. The retry band-aid in `Session` is gone and rotation is serialized in the store. The regression test drives 32 concurrent refreshes and asserts upstream rotates exactly once ([RFC 6749 §10.4](https://www.rfc-editor.org/rfc/rfc6749#section-10.4)). Running it needs shell access."},
            {"type": "tool_use", "id": "toolu_d5", "name": "bash", "input": {"command": "cargo test -p relay --test refresh_race"}}]}),
    ]
}

/// [`IDLE_CPU`]'s history: three finished turns whose runs fold, the last
/// one with ten tools (more than its tally bar draws).
fn idle_cpu() -> Vec<Value> {
    let p = |f: &str| format!("/home/you/work/synaps-desktop/crates/synaps-desktop/src/{f}");
    let mut out =
        vec![json!({"role": "user", "content": "The hybrid build idles at 3 % CPU where main sits at 0. Find out why and fix it."})];
    fn call(out: &mut Vec<Value>, id: &str, name: &str, input: Value, result: &str, error: bool, text: Option<&str>) {
        let mut blocks = Vec::new();
        if let Some(t) = text {
            blocks.push(json!({"type": "text", "text": t}));
        }
        blocks.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
        out.push(json!({"role": "assistant", "content": blocks}));
        out.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": result, "is_error": error}]}));
    }
    let idle = "scripts/idle-cpu-linux.sh target/release/synaps-desktop";
    call(
        &mut out,
        "toolu_i1",
        "bash",
        json!({"command": idle}),
        "synaps-desktop: frames/15s=212 cpu_ticks=46 rss_kb=184212",
        false,
        Some("Measuring it first: frames and CPU over 15 s with nothing happening."),
    );
    call(
        &mut out,
        "toolu_i2",
        "grep",
        json!({"pattern": "request_animation_frame|on_next_frame", "path": p("")}),
        "app.rs:2210:        window.request_animation_frame();",
        false,
        None,
    );
    call(
        &mut out,
        "toolu_i3",
        "read",
        json!({"path": p("app.rs")}),
        "    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) {\n…",
        false,
        None,
    );
    call(
        &mut out,
        "toolu_i4",
        "read",
        json!({"path": p("tray.rs")}),
        "    pub(crate) fn sync_tray(&mut self, cx: &mut Context<Self>) {\n…",
        false,
        None,
    );
    call(
        &mut out,
        "toolu_i5",
        "edit",
        json!({"path": p("app.rs"),
        "old_string": "        if self.tray.until.is_some() {\n            window.request_animation_frame();\n        }",
        "new_string": "        // The tray's flash ends on its own timer (`tray_due`): no frame\n        // is asked for while nothing changes.\n        let _ = due;"}),
        "Edited app.rs",
        false,
        None,
    );
    call(
        &mut out,
        "toolu_i6",
        "bash",
        json!({"command": "cargo build --release -p synaps-desktop"}),
        "error[E0425]: cannot find value `due` in this scope\n --> crates/synaps-desktop/src/app.rs:2212:17",
        true,
        None,
    );
    call(
        &mut out,
        "toolu_i7",
        "edit",
        json!({"path": p("app.rs"), "old_string": "        let _ = due;", "new_string": ""}),
        "Edited app.rs",
        false,
        None,
    );
    call(&mut out, "toolu_i8", "bash", json!({"command": idle}), "synaps-desktop: frames/15s=0 cpu_ticks=1 rss_kb=183960", false, None);
    out.push(json!({"role": "assistant", "content": [{"type": "text",
        "text": "Idle is back to 0 frames in 15 s. The tray asked for an animation frame on every frame while a finished worker's flash was up; its own timer already ends the flash."}]}));
    out.push(json!({"role": "user", "content": "Bench it once more against main, then commit."}));
    call(
        &mut out,
        "toolu_j1",
        "bash",
        json!({"command": "scripts/bench-linux.sh target/release/synaps-desktop"}),
        "bench-results/20261113-1702.json",
        false,
        Some("Running the bench."),
    );
    call(
        &mut out,
        "toolu_j2",
        "read",
        json!({"path": "/home/you/work/synaps-desktop/bench-results/20261113-1702.json"}),
        "{\"stream\": {\"fps\": 143.1, \"dropped_frames\": 2}, …}",
        false,
        None,
    );
    call(
        &mut out,
        "toolu_j3",
        "edit",
        json!({"path": "/home/you/work/synaps-desktop/docs/bench/README.md",
        "old_string": "| idle | 212 frames / 15 s |", "new_string": "| idle | 0 frames / 15 s |"}),
        "Edited README.md",
        false,
        None,
    );
    call(
        &mut out,
        "toolu_j4",
        "bash",
        json!({"command": "git commit -am 'Tray: no frame per frame while a flash is up'"}),
        "[fix/idle 4c1d2e7] Tray: no frame per frame while a flash is up",
        false,
        None,
    );
    out.push(json!({"role": "assistant", "content": [{"type": "text", "text": "Committed. The bench matches main: 143 fps streaming, 2 dropped."}]}));
    out.push(
        json!({"role": "user", "content": "One more pass: anything else that asks for frames while idle? Note what you find in the docs."}),
    );
    let ok = |out: &mut Vec<Value>, id: &str, name: &str, input: Value, result: &str| call(out, id, name, input, result, false, None);
    call(
        &mut out,
        "toolu_k01",
        "grep",
        json!({"pattern": "request_animation_frame", "path": p("")}),
        "chart.rs:88:            window.request_animation_frame();",
        false,
        Some("Every other frame request first."),
    );
    ok(
        &mut out,
        "toolu_k02",
        "read",
        json!({"path": p("chart.rs"), "offset": 84, "limit": 8}),
        "85\timpl Element for Chart {\n86\t    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {\n87\t        if self.revealing() {\n\
         88\t            window.request_animation_frame();\n89\t        }\n90\t        self.draw(bounds, window);\n91\t    }\n92\t}\n\n... (212 more lines)",
    );
    ok(&mut out, "toolu_k03", "find", json!({"pattern": "*.rs", "path": p("")}), "app.rs\nbench.rs\nchart.rs\ndemo.rs\ntray.rs");
    ok(&mut out, "toolu_k04", "ls", json!({"path": p("")}), "app.rs  bench.rs  chart.rs  demo.rs  tray.rs  …");
    ok(
        &mut out,
        "toolu_k05",
        "bash",
        json!({"command": "cargo test -p synaps-desktop chart -- --nocapture"}),
        "running 14 tests\n…\n[output truncated at 30000 chars]",
    );
    ok(
        &mut out,
        "toolu_k06",
        "ext__github__search_code",
        json!({"q": "request_animation_frame language:rust repo:zed-industries/zed"}),
        "3 results: gpui/src/window.rs, …",
    );
    ok(
        &mut out,
        "toolu_k07",
        "ext__web_tools__fetch",
        json!({"url": "https://docs.rs/gpui/latest/gpui/struct.Window.html"}),
        "request_animation_frame: schedules a frame for the window, once.",
    );
    ok(&mut out, "toolu_k08", "read", json!({"path": p("app.rs")}), "    fn on_usage(&mut self, cx: &mut Context<Self>) {\n…");
    ok(
        &mut out,
        "toolu_k09",
        "edit",
        json!({"path": p("chart.rs"),
        "old_string": "            window.request_animation_frame();\n",
        "new_string": "            // Drawn once per change of its data: no frame asked for.\n"}),
        "Edited chart.rs",
    );
    ok(
        &mut out,
        "toolu_k10",
        "bash",
        json!({"command": "cargo build --release -p synaps-desktop"}),
        "Finished `release` profile [optimized] target(s)",
    );
    ok(&mut out, "toolu_k11", "grep", json!({"pattern": "request_animation_frame", "path": p("")}), "(no matches)");
    ok(
        &mut out,
        "toolu_k12",
        "edit",
        json!({"path": "/home/you/work/synaps-desktop/docs/bench/README.md",
        "old_string": "## Idle\n", "new_string": "## Idle\n\nNo view asks for a frame while nothing changes.\n"}),
        "Edited README.md",
    );
    ok(
        &mut out,
        "toolu_k13",
        "write",
        json!({"path": "/home/you/work/synaps-desktop/docs/idle.md",
        "content": "# Idle\n\nFrames are drawn on a change only.\nTimers: the tray's flash, the session poll.\n"}),
        "Wrote docs/idle.md",
    );
    ok(
        &mut out,
        "toolu_k14",
        "todo_write",
        json!({"todos": [{"content": "chart: no frame request", "status": "done"}, {"content": "docs: idle", "status": "done"}]}),
        "Updated 2 todos",
    );
    ok(&mut out, "toolu_k15", "read", json!({"path": "/home/you/work/synaps-desktop/docs/idle.md"}), "# Idle\n…");
    ok(&mut out, "toolu_k16", "bash", json!({"command": idle}), "synaps-desktop: frames/15s=0 cpu_ticks=1 rss_kb=183812");
    ok(
        &mut out,
        "toolu_k17",
        "bash",
        json!({"command": "git commit -am 'Chart: no frame request; docs: idle'"}),
        "[fix/idle 9b3e1a0] Chart: no frame request; docs: idle",
    );
    out.push(json!({"role": "assistant", "content": [{"type": "text",
        "text": "The usage chart asked for a frame on every paint; now it draws once per change. Idle stays at 0 frames. The chart test run hit the output limit, so I left its log alone."}]}));
    out
}

/// The events of a daemon connection with ten sessions: two polls (in the
/// second a turn has ended off screen: UNREAD). The window opens [`SID`]
/// (an attach, answered by [`attach`]).
pub fn events() -> Vec<Event> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    vec![Event::Sessions(sessions(now, false)), Event::Sessions(sessions(now, true))]
}

/// The synthetic daemon's reply to attaching `id` as `attach_seq`: [`SID`]
/// in full (its history, a pending confirmation, usage, a subagent); any
/// other listed session with a short history of its own size, its busy
/// state and as many confirmations as it awaits. An unlisted id: nothing
/// (never answered, as a daemon that is gone).
pub fn attach(id: &str, attach_seq: u64) -> Vec<Event> {
    if id == SID {
        return attach_sid(attach_seq);
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let Some(m) = sessions(now, true).into_iter().find(|s| s.id == id) else { return Vec::new() };
    let seed = id.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    let mut msgs = vec![json!({"role": "user", "content": m.title.clone().unwrap_or_else(|| "Start here.".into())})];
    msgs.extend(synthetic::history(m.messages.min(60), seed));
    if m.messages == 0 {
        msgs.clear();
    }
    if id == IDLE_CPU {
        msgs = idle_cpu();
        // Its last turn the next morning: a day line above it.
        let last = msgs.iter().rposition(|m| m["role"] == "user" && m["content"].is_string());
        stamp(&mut msgs, id, 25, last);
    } else if id != LEGACY {
        stamp(&mut msgs, id, 45, None);
    }
    // The flaky-e2e session's turn runs a call as it attaches: it started
    // 12 s ago (its row's clock counts from there).
    if id == FLAKY && m.busy {
        let mut call = json!({"role": "assistant", "content": [
            {"type": "text", "text": "Re-running the checkout suite with tracing on."},
            {"type": "tool_use", "id": "toolu_e2e", "name": "bash", "input": {"command": "npm run e2e -- --grep checkout --trace on"}}]});
        call["_synaps_at"] = json!(rfc3339(now - 12));
        msgs.push(call);
    }
    let prompts: Vec<Value> = (1..=m.awaiting_input as u64)
        .map(|i| {
            json!({"id": i, "kind": "confirm", "title": "Confirm tool call",
                "prompt": format!("Tool: bash\n\nInput that will run:\n$ cargo test --workspace -- --test-threads {}\n\nAllow this tool call?", i * 4)})
        })
        .collect();
    let frame = json!({"type": "attached", "client": 2,
        "meta": {"id": m.id, "name": m.name, "title": m.title, "model": m.model, "cwd": m.cwd, "lifecycle": m.lifecycle,
            "busy": m.busy, "awaiting_input": m.awaiting_input},
        "view": {"model": m.model, "thinking_level": "medium", "reasoning_level": "medium", "context_window": 200000},
        "conversation": {"header": {"title": m.title}, "messages_len": msgs.len(), "api_messages": msgs, "cost": m.cost_usd},
        "streaming": m.busy, "pending_prompts": prompts, "client_input": "shared", "input_owner": 2});
    let mut out = match decode_frame(&frame.to_string()) {
        Ok(DaemonFrame::Attached(a)) => vec![Event::Attached { attach_seq, attached: a, mode: AttachMode::Mirror, reconnect: false }],
        _ => Vec::new(),
    };
    // The flaky-e2e session fans out: more workers than two lines of
    // chips hold at 1280 (the rest in `+N more`).
    if id == FLAKY && !out.is_empty() {
        let row = |n: u64, name: &str, step: &str, secs: f64| {
            json!({"subagent_id": n, "agent_name": name, "status": "running", "cancel_requested": false, "elapsed_secs": secs,
                "finished_elapsed": null, "step": step, "tools": n + 2, "handle_id": format!("sa_{n}"), "model": "anthropic/claude-sonnet-4-5"})
        };
        let rows = json!({"ev": "subagent_rows", "rows": [
            row(1, "trace-cart", "reading e2e/checkout/cart.spec.ts", 52.),
            row(2, "trace-pay", "grep -rn \"waitForResponse\" e2e/", 47.),
            row(3, "har-diff", "comparing 14 HAR captures", 38.),
            row(4, "bisect", "git bisect run npm run e2e:checkout", 121.),
            row(5, "flake-stats", "reading ci/runs.json", 19.),
            row(6, "reviewer", "reading src/checkout/latency.ts", 7.)]});
        if let Ok(event) = decode_event(&rows) {
            out.push(Event::session(attach_seq, FLAKY, 1, event, Arc::new(rows)));
        }
    }
    out
}

fn attach_sid(attach_seq: u64) -> Vec<Event> {
    let mut out = Vec::new();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // The daemon's confirm body (agent-engine `confirm_prompt_body`), its
    // time and scope; `SYNAPS_DEMO_PROMPT=secret`: a secret prompt instead
    // (the authorize card's evidence).
    let prompt = if std::env::var("SYNAPS_DEMO_PROMPT").as_deref() == Ok("secret") {
        json!({"id": 7, "kind": "secret", "title": "API key", "raised_at": rfc3339(now - 7),
            "prompt": "relay-gateway needs RELAY_SIGNING_KEY to replay the refresh race against staging."})
    } else {
        json!({"id": 7, "kind": "confirm", "title": "Confirm tool call", "raised_at": rfc3339(now - 7),
            "confirm_scope": {"extensions": ["guard"], "tool": "bash"},
            "prompt": "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/work/relay-gateway:\n$ cargo test -p relay --test refresh_race\n\nAllow this tool call?"})
    };
    let mut msgs = messages();
    stamp(&mut msgs, SID, 40, None);
    let attached = json!({"type": "attached", "client": 2,
        "meta": {"id": SID, "name": "relay-refresh", "title": "Refresh race in session relay", "model": "anthropic/claude-opus-4-1", "cwd": CWD,
            "lifecycle": "live", "busy": true, "awaiting_input": 1},
        "view": {"model": "anthropic/claude-opus-4-1", "thinking_level": "high", "reasoning_level": "high", "context_window": 200000},
        "conversation": {"header": {"title": "Refresh race in session relay"}, "messages_len": msgs.len(), "api_messages": msgs, "cost": 4.95,
            "tokens": {"input": 18_500, "output": 10_040, "cache_read": 1_410_000, "cache_creation": 96_200}},
        "streaming": true,
        "pending_prompts": [prompt],
        "client_input": "shared", "input_owner": 2});
    if let Ok(DaemonFrame::Attached(a)) = decode_frame(&attached.to_string()) {
        out.push(Event::Attached { attach_seq, attached: a, mode: AttachMode::Mirror, reconnect: false });
    }
    let usage = json!({"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 2_100, "output_tokens": 640,
        "cache_read_input_tokens": 86_000, "cache_creation_input_tokens": 4_100, "model": "anthropic/claude-opus-4-1"}});
    let subagents = json!({"ev": "subagent_rows", "rows": [
        {"subagent_id": 1, "agent_name": "scout", "status": "running", "cancel_requested": false, "elapsed_secs": 41.0,
            "finished_elapsed": null, "step": "reading crates/relay/src/upstream.rs", "tools": 6, "handle_id": "sa_1",
            "model": "anthropic/claude-sonnet-4-5"},
        {"subagent_id": 2, "agent_name": "reviewer", "status": "completed", "cancel_requested": false, "elapsed_secs": 95.0,
            "finished_elapsed": {"secs": 12, "nanos": 0}, "step": "", "tools": 9, "handle_id": "sa_2",
            "model": "anthropic/claude-sonnet-4-5"}]});
    for (seq, raw) in [(1, usage), (2, subagents)] {
        if let Ok(event) = decode_event(&raw) {
            out.push(Event::session(attach_seq, SID, seq, event, Arc::new(raw)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnState, Effect, Model};
    use crate::rail::State;
    use crate::subagents::Status;

    #[test]
    fn rfc3339_formats_and_round_trips() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_794_596_342), "2026-11-13T18:59:02Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        for t in [0, 951_782_400, 1_794_596_342, 4_102_444_799] {
            assert_eq!(crate::named::parse_rfc3339(&rfc3339(t)), Some(t));
        }
    }

    /// Every widget the demo fills, it fills through the model.
    #[test]
    fn the_demo_goes_through_the_model() {
        let mut m = Model::new(ConnState::Off("offline".into()));
        m.attaching(1, None, false);
        m.asked_for(SID);
        for ev in events().into_iter().chain(attach(SID, 1)) {
            m.apply(ev);
        }
        assert_eq!(m.sessions.len(), 13);
        let states: std::collections::HashSet<State> = m.sessions.iter().map(|s| m.state(s)).collect();
        for st in [State::NeedsYou, State::Running, State::Workers, State::Locked, State::Unread, State::Idle, State::Parked] {
            assert!(states.contains(&st), "{st:?} missing from the demo");
        }
        assert_eq!(m.active.as_ref().unwrap().session_id, SID);
        assert_eq!(m.transcript.pending_prompts().len(), 1);
        assert!(m.transcript.busy());
        let s = &m.stats;
        assert_eq!(s.view.as_ref().unwrap().effort, "high");
        assert_eq!(s.context, Some(92_200));
        assert!((s.context_ratio().unwrap() - 0.461).abs() < 1e-9);
        assert_eq!(s.tokens.unwrap().cache_read, 1_410_000);
        let files: Vec<&str> = s.files.files().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(files, ["crates/relay/src/token_store.rs", "crates/relay/src/session.rs", "crates/relay/tests/refresh_race.rs"]);
        assert!(s.files.preview("toolu_d3").unwrap().removed >= 5);
        let shell: Vec<(&str, &str)> = s.files.shell().iter().map(|f| (f.path.as_str(), f.verb)).collect();
        assert_eq!(shell, [("crates/relay/src/retry.rs", "rm"), ("crates/relay/src/lib.rs", "sed -i")]);
        assert_eq!(m.transcript.subagents().len(), 2);
        let sub = |id: &str| s.subagents.view(id).map(|v| (v.status, v.facts.to_string()));
        assert_eq!(sub("toolu_s1"), Some((Some(Status::Running), "claude-sonnet-4-5 · 6 tools · 41.0s".into())));
        assert_eq!(sub("toolu_s2"), Some((Some(Status::Done), "claude-sonnet-4-5 · 9 tools · 1m 23s".into())));
        assert_eq!(m.tabs.visible_session(), Some(SID), "the attach is the visible tab's");
    }

    /// Every listed session attaches (the tabs switch between them); its
    /// waiting prompts and busy state are the list's. An unlisted one is
    /// never answered.
    #[test]
    fn every_demo_session_attaches() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        for (k, s) in sessions(now, true).iter().enumerate() {
            let mut m = Model::new(ConnState::Off("offline".into()));
            let seq = k as u64 + 1;
            m.attaching(seq, None, false);
            m.asked_for(&s.id);
            let evs = attach(&s.id, seq);
            assert!(!evs.is_empty(), "{} not answered", s.id);
            for ev in evs {
                m.apply(ev);
            }
            assert_eq!(m.active.as_ref().map(|a| a.session_id.as_str()), Some(s.id.as_str()));
            assert_eq!(m.transcript.pending_prompts().len(), s.awaiting_input, "{}", s.id);
            assert_eq!(m.transcript.busy(), s.busy, "{}", s.id);
        }
        assert!(attach("20990101-000000-ffff", 1).is_empty());
        // The parked one's turns are over: its runs fold, one with a
        // failed call, each with an edit, the last with ten tools; the
        // rail's count is its history's.
        let mut m = Model::new(ConnState::Off("offline".into()));
        m.attaching(1, None, false);
        m.asked_for(IDLE_CPU);
        for ev in attach(IDLE_CPU, 1) {
            m.apply(ev);
        }
        let rows = crate::turns::build(m.transcript.items(), &|_| None, 0, false, &Default::default(), &Default::default());
        let runs: Vec<usize> = rows
            .iter()
            .filter_map(|d| match d.kind {
                crate::turns::Kind::Summary { end, .. } => Some(end - d.item),
                _ => None,
            })
            .collect();
        assert_eq!(runs, [8, 4, 17]);
        let listed = sessions(now, true).into_iter().find(|s| s.id == IDLE_CPU).map(|s| s.messages);
        assert_eq!(listed, Some(idle_cpu().len()));
    }

    /// `--offline` answers `catalog_page` through the same path as a
    /// daemon: the model asks, the synthetic catalog replies, HISTORY fills.
    #[test]
    fn the_demo_answers_catalog_pages() {
        let mut m = Model::new(ConnState::Off("offline".into()));
        let mut asks = m.offer_saved();
        let mut req = 0;
        while let Some(Effect::CatalogPage { before, limit, named_only }) = asks.pop() {
            req += 1;
            m.page_sent(req);
            asks.extend(m.apply(catalog_page(req, before.as_ref(), limit, named_only)));
        }
        assert_eq!(req, 1, "one page of 200 holds them all: no named list asked");
        assert_eq!(m.named.recent.iter().filter(|v| v.name.is_some()).count(), 11, "the held one is in no page");
        assert!(m.named.recent_loaded && !m.named.recent_truncated);
        assert_eq!(crate::rail::history(&m).len(), 131);
        // Paged by its cursor: every row once, in order.
        let mut before = None;
        let mut ids = Vec::new();
        loop {
            let Event::CatalogPage { sessions, next, rejected: 0, .. } = catalog_page(9, before.as_ref(), 50, false) else { panic!() };
            assert!(sessions.iter().all(SessionMeta::is_saved));
            ids.extend(sessions.into_iter().map(|s| s.id));
            match next {
                Some(c) => before = Some(c),
                None => break,
            }
        }
        let all: Vec<String> = catalog(0).iter().map(|v| v["id"].as_str().unwrap_or_default().to_string()).collect();
        assert_eq!((ids.len(), ids), (131, all));
        let Event::CatalogPage { sessions, next: None, .. } = catalog_page(9, None, 200, true) else { panic!() };
        assert_eq!(sessions.len(), 11);
    }
}
