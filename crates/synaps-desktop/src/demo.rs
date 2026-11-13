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
use synaps_client::{AttachMode, Event};

use crate::model::LOCAL_SEQ;

const CWD: &str = "/home/you/work/relay-gateway";
const SID: &str = "20261113-041207-a1f0";

/// RFC 3339 (UTC) of `secs` after the epoch.
fn rfc3339(secs: i64) -> String {
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

fn sessions(now: i64) -> Vec<SessionMeta> {
    let ago = |min: i64| rfc3339(now - min * 60);
    let rows = [
        json!({"id": SID, "name": "relay-refresh", "title": "Refresh race in session relay", "model": "anthropic/claude-opus-4-1",
            "cwd": CWD, "busy": true, "awaiting_input": 1, "cost_usd": 4.95, "lifecycle": "live", "clients": 1, "messages": 9, "state_since": ago(4)}),
        json!({"id": "20261113-035512-77c2", "name": "ingest-core", "title": "Migrate ingest workers to async", "model": "anthropic/claude-sonnet-4-5",
            "cwd": "/home/you/work/ingest", "busy": true, "awaiting_input": 1, "cost_usd": 1.45, "lifecycle": "live", "messages": 31, "state_since": ago(18)}),
        json!({"id": "20261113-033001-c3d9", "title": "Flaky e2e: checkout latency", "model": "anthropic/claude-sonnet-4-5",
            "cwd": "/home/you/work/shop", "busy": true, "cost_usd": 0.45, "lifecycle": "live", "messages": 12, "state_since": ago(7)}),
        json!({"id": "20261113-021944-0b1e", "title": "Bisect allocator regression", "model": "anthropic/claude-opus-4-1",
            "cwd": "/home/you/work/alloc", "busy": false, "cost_usd": 2.17, "lifecycle": "live", "messages": 58, "state_since": ago(41)}),
        json!({"id": "20261112-224410-9f00", "name": "ledger-drift", "title": "Schema drift report: ledger", "model": "anthropic/claude-haiku-4-5",
            "cwd": "/home/you/work/ledger", "busy": false, "cost_usd": 0.09, "lifecycle": "live", "messages": 6, "state_since": ago(130)}),
        json!({"id": "20261112-180203-44aa", "title": "Operator handbook, chapter 4", "model": "anthropic/claude-sonnet-4-5",
            "cwd": "/home/you/docs", "busy": false, "cost_usd": 0.27, "lifecycle": "parked", "messages": 22, "state_since": ago(190)}),
    ];
    rows.into_iter().filter_map(|r| serde_json::from_value(r).ok()).collect()
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

fn messages() -> Vec<Value> {
    let p = |f: &str| format!("{CWD}/{f}");
    vec![
        json!({"role": "user", "content": "The relay drops sessions when two token refreshes race. Fix it and add a regression test. Keep the public API unchanged."}),
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "Two refreshes both see the token as expiring, both rotate, and the second rotation invalidates the first caller's token. The retry in Session papers over that. Serialize rotation in the store instead.", "signature": "x"},
            {"type": "text", "text": "Looking at the refresh path first."},
            {"type": "tool_use", "id": "toolu_d1", "name": "read", "input": {"path": p("crates/relay/src/token_store.rs")}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d1", "content": "pub struct TokenStore {\n    inner: Mutex<Inner>,\n}\n…"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d2", "name": "edit", "input": {"path": p("crates/relay/src/token_store.rs"),
                "old_string": "    pub async fn rotate(&self, id: SessionId) -> Result<Token> {\n        let mut inner = self.inner.lock().await;\n        inner.rotate(id).await\n    }",
                "new_string": "    pub async fn rotate(&self, id: SessionId) -> Result<Token> {\n        let _turn = self.rotation.lock().await;\n        let mut inner = self.inner.lock().await;\n        if let Some(t) = inner.fresh(id) {\n            return Ok(t);\n        }\n        inner.rotate(id).await\n    }"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d2", "content": "Edited crates/relay/src/token_store.rs"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d3", "name": "edit", "input": {"path": p("crates/relay/src/session.rs"), "old_string": OLD, "new_string": NEW}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d3", "content": "Edited crates/relay/src/session.rs"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_d4", "name": "write", "input": {"path": p("crates/relay/tests/refresh_race.rs"), "content": TEST}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_d4", "content": "Wrote crates/relay/tests/refresh_race.rs"}]}),
        json!({"role": "assistant", "content": [
            {"type": "text", "text": "Fix is in. The retry band-aid in `Session` is gone and rotation is serialized in the store. The regression test drives 32 concurrent refreshes and asserts upstream rotates exactly once ([RFC 6749 §10.4](https://www.rfc-editor.org/rfc/rfc6749#section-10.4)). Running it needs shell access."},
            {"type": "tool_use", "id": "toolu_d5", "name": "bash", "input": {"command": "cargo test -p relay --test refresh_race"}}]}),
    ]
}

/// The events of a daemon connection with six sessions, attached to one
/// that waits on a tool-call confirmation.
pub fn events() -> Vec<Event> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut out = vec![Event::Sessions(sessions(now))];
    let msgs = messages();
    let attached = json!({"type": "attached", "client": 2,
        "meta": {"id": SID, "name": "relay-refresh", "title": "Refresh race in session relay", "model": "anthropic/claude-opus-4-1", "cwd": CWD,
            "lifecycle": "live", "busy": true, "awaiting_input": 1},
        "view": {"model": "anthropic/claude-opus-4-1", "thinking_level": "high", "reasoning_level": "high", "context_window": 200000},
        "conversation": {"header": {"title": "Refresh race in session relay"}, "messages_len": msgs.len(), "api_messages": msgs, "cost": 4.95,
            "tokens": {"input": 18_500, "output": 10_040, "cache_read": 1_410_000, "cache_creation": 96_200}},
        "streaming": true,
        "pending_prompts": [{"id": 7, "kind": "confirm", "title": "Confirm tool call",
            // The daemon's confirm body (agent-engine `confirm_prompt_body`).
            "prompt": "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/work/relay-gateway:\n$ cargo test -p relay --test refresh_race\n\nAllow this tool call?"}],
        "client_input": "shared", "input_owner": 2});
    if let Ok(DaemonFrame::Attached(a)) = decode_frame(&attached.to_string()) {
        out.push(Event::Attached { attach_seq: LOCAL_SEQ, attached: a, mode: AttachMode::Mirror, reconnect: false });
    }
    let usage = json!({"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 2_100, "output_tokens": 640,
        "cache_read_input_tokens": 86_000, "cache_creation_input_tokens": 4_100, "model": "anthropic/claude-opus-4-1"}});
    let subagents = json!({"ev": "subagent_rows", "rows": [
        {"subagent_id": 1, "agent_name": "scout", "status": "running", "cancel_requested": false, "elapsed_secs": 41.0,
            "finished_elapsed": null, "step": "reading crates/relay/src/upstream.rs", "tools": 6}]});
    for (seq, raw) in [(1, usage), (2, subagents)] {
        if let Ok(event) = decode_event(&raw) {
            out.push(Event::Session { attach_seq: LOCAL_SEQ, session_id: SID.into(), seq, event, raw: Arc::new(raw) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnState, Model};

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
        m.set_local_session("demo");
        for ev in events() {
            m.apply(ev);
        }
        assert_eq!(m.sessions.len(), 6);
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
        assert_eq!(m.transcript.subagents().len(), 1);
    }
}
