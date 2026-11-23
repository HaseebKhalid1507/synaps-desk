//! The numbers the composer foot and the right-hand panel show, all from
//! the daemon: the runtime view (`Attached.view`, `setting_changed`), the
//! conversation's token totals and cost (`Attached.conversation`, every
//! `Conversation` digest), the last model call's prompt size (`Usage`, for
//! the context gauge, as the TUI computes it) and the files changed (the
//! history's file tools, [`crate::files`]). Nothing here is estimated: a
//! value the daemon has not sent is `None` and its widget says so.

use serde_json::Value;
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};
use synaps_client::{Attached, HistoryUpdate};

use crate::files::Ledger;
use crate::subagents::Subagents;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl Tokens {
    /// `ConversationTokens`: `None` unless it is an object with numbers.
    pub fn from_value(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let n = |k: &str| o.get(k).and_then(Value::as_u64);
        if o.is_empty() || ["input", "output", "cache_read", "cache_creation"].iter().all(|k| n(k).is_none()) {
            return None;
        }
        Some(Tokens {
            input: n("input").unwrap_or(0),
            output: n("output").unwrap_or(0),
            cache_read: n("cache_read").unwrap_or(0),
            cache_creation: n("cache_creation").unwrap_or(0),
        })
    }

    /// The conversation's own tokens: `total` less the keep-warm pings the
    /// daemon counts in it (`keepwarm_tokens`, absent from older daemons).
    pub fn conversation(total: &Value, keepwarm: &Value) -> Option<Self> {
        let t = Self::from_value(total)?;
        let k = Self::from_value(keepwarm).unwrap_or_default();
        Some(Tokens {
            input: t.input.saturating_sub(k.input),
            output: t.output.saturating_sub(k.output),
            cache_read: t.cache_read.saturating_sub(k.cache_read),
            cache_creation: t.cache_creation.saturating_sub(k.cache_creation),
        })
    }

    /// Share of prompt tokens served from the cache, if any were sent.
    pub fn cache_hit(&self) -> Option<f64> {
        let prompt = self.input + self.cache_read + self.cache_creation;
        (prompt > 0).then(|| self.cache_read as f64 / prompt as f64)
    }
}

/// `RuntimeView`'s fields the foot, the gauge and Settings' THIS
/// SESSION use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RuntimeInfo {
    pub model: String,
    /// `reasoning_level` (else `thinking_level`): "high", "low", "off", ...
    pub effort: String,
    /// Tokens; 0 when the daemon did not say.
    pub context_window: u64,
    pub compaction_model: String,
    /// `None`: not in the view (an older daemon).
    pub api_retries: Option<u64>,
    pub subagent_timeout: Option<u64>,
    pub max_tool_output: Option<u64>,
    pub bash_timeout: Option<u64>,
    /// The session's own system prompt (`None`: the daemon's default).
    pub system_prompt: Option<std::sync::Arc<str>>,
}

impl RuntimeInfo {
    pub fn from_value(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |k: &str| o.get(k).and_then(Value::as_u64);
        let effort = Some(s("reasoning_level")).filter(|e| !e.is_empty()).unwrap_or_else(|| s("thinking_level"));
        let info = RuntimeInfo {
            model: s("model"),
            effort,
            context_window: n("context_window").unwrap_or(0),
            compaction_model: s("compaction_model"),
            api_retries: n("api_retries"),
            subagent_timeout: n("subagent_timeout"),
            max_tool_output: n("max_tool_output"),
            bash_timeout: n("bash_timeout"),
            system_prompt: o.get("system_prompt").and_then(Value::as_str).map(std::sync::Arc::from),
        };
        (info != RuntimeInfo::default()).then_some(info)
    }
}

/// The same model, with or without a `provider/` prefix.
fn same_model(a: &str, b: &str) -> bool {
    let bare = |m: &str| m.rsplit_once('/').map_or(m, |(_, m)| m).to_string();
    a == b || bare(a) == bare(b)
}

/// The cost unless it is the serde default of a daemon that sent neither
/// tokens nor a cost (an older one): then it is unknown, not "$0.00".
fn known_cost(cost: f64, tokens: Option<Tokens>) -> Option<f64> {
    (cost > 0.0 || tokens.is_some()).then_some(cost)
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// The session these are of (`Attached.meta.id`).
    pub session: Option<String>,
    pub view: Option<RuntimeInfo>,
    /// The conversation's tokens, keep-warm pings excluded.
    pub tokens: Option<Tokens>,
    /// The conversation's cost in USD as the daemon reports it.
    pub cost: Option<f64>,
    /// The last model call's prompt (input + cache read + cache creation):
    /// what the context window holds now.
    pub context: Option<u64>,
    /// The attached session's folder (`Attached.meta.cwd`).
    pub cwd: Option<String>,
    pub files: Ledger,
    /// Its subagents, by the call that started each.
    pub subagents: Subagents,
    /// The attachment ended: these are the last numbers seen, not live.
    pub detached: bool,
    /// Its prompt cache and keep-warm, as its last `Status` said
    /// ([`crate::warm`]); kept across a re-attach of the same session.
    pub warm: Option<crate::warm::Warm>,
}

impl Stats {
    /// A (re-)attach: everything from the snapshot, then the in-flight
    /// turn's replay. A re-attach of the same session reuses the analysed
    /// file edits.
    pub fn on_attached(&mut self, a: &Attached) {
        let cwd = a.meta.cwd.clone().filter(|c| !c.is_empty());
        let same = self.session.as_deref() == Some(a.meta.id.as_str());
        let mut files = std::mem::take(&mut self.files);
        files.reset(&a.conversation.api_messages, cwd.as_deref(), same);
        let mut subagents = std::mem::take(&mut self.subagents);
        if same {
            // What ran when this window last looked is "started" until a row says.
            subagents.reattached();
        }
        subagents.reset(&a.conversation.api_messages, same);
        let tokens = Tokens::conversation(&a.conversation.tokens, &a.conversation.keepwarm_tokens);
        let warm = if same { self.warm.take() } else { None };
        *self = Stats {
            session: Some(a.meta.id.clone()),
            view: RuntimeInfo::from_value(&a.view),
            tokens,
            cost: known_cost(a.conversation.cost, tokens),
            context: None,
            cwd,
            files,
            subagents,
            detached: false,
            warm,
        };
        self.subagents.replaying(true);
        for env in &a.replay {
            if let Ok(ev) = env.decode() {
                self.on_session(&ev, &env.event);
            }
        }
        self.subagents.replaying(false);
    }

    /// The attachment ended.
    pub fn on_detached(&mut self) {
        self.detached = true;
    }

    pub fn on_history(&mut self, update: &HistoryUpdate) {
        match update {
            HistoryUpdate::Append { messages, .. } => {
                self.files.append(messages);
                self.subagents.append(messages);
            }
            HistoryUpdate::Replace { messages } => {
                let cwd = self.cwd.clone();
                self.files.reset(messages, cwd.as_deref(), true);
                self.subagents.reset(messages, true);
            }
            _ => {}
        }
    }

    /// A live event; `raw` is the wire event (fields the mirror drops).
    /// `true`: the session's prompt cache is not what it was (the history
    /// was replaced, or the model changed): the countdown stops, and the
    /// daemon is asked again.
    pub fn on_session(&mut self, ev: &SessionEvent, raw: &Value) -> bool {
        self.subagents.on_session(ev);
        let mut new_cache = false;
        match ev {
            SessionEvent::Conversation { digest } => {
                // `keepwarm_tokens` is not on the decoded digest: from the wire event.
                if let Some(t) = Tokens::conversation(&digest.tokens, &raw["digest"]["keepwarm_tokens"]) {
                    self.tokens = Some(t);
                }
                if let Some(c) = known_cost(digest.cost, self.tokens) {
                    self.cost = Some(c);
                }
                if digest.messages_len == 0 {
                    self.context = None;
                }
            }
            SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ToolUse { tool_name, tool_id, input }) } => {
                self.files.tool_use(tool_name, tool_id, input);
            }
            SessionEvent::Stream { event: StreamEvent::Session(_) } => {
                let e = &raw["event"];
                // A side call on another model (a compaction model, say) is
                // not what the session's window holds.
                let other_model = match (e["model"].as_str(), self.view.as_ref().map(|v| v.model.as_str())) {
                    (Some(m), Some(v)) if !m.is_empty() && !v.is_empty() => !same_model(m, v),
                    _ => false,
                };
                if e["session"] == "usage" && !other_model {
                    let n = |k: &str| e.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let prompt = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                    if prompt > 0 {
                        self.context = Some(prompt);
                    }
                }
            }
            SessionEvent::Cleared { .. } | SessionEvent::Resumed { .. } | SessionEvent::CompactionApplied { .. } => {
                self.context = None;
                new_cache = true;
            }
            _ if raw["ev"] == "setting_changed" => {
                if let Some(v) = RuntimeInfo::from_value(&raw["applied"]["view"]) {
                    // Only an effective change of model: a refused or
                    // same-model `Set`, or a thinking change, keeps it.
                    new_cache = self.view.as_ref().is_some_and(|o| !same_model(&o.model, &v.model));
                    self.view = Some(v);
                }
            }
            _ => {}
        }
        if new_cache && let Some(w) = self.warm.as_mut() {
            w.invalidate();
        }
        new_cache
    }

    /// Context use, 0..=1, when both the prompt size and the window are known.
    pub fn context_ratio(&self) -> Option<f64> {
        let w = self.view.as_ref().map(|v| v.context_window).filter(|&w| w > 0)?;
        Some((self.context? as f64 / w as f64).min(1.0))
    }
}

/// `18.5k`, `1.41M`, `512`.
pub fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..100_000 => format!("{:.1}k", n as f64 / 1e3),
        100_000..1_000_000 => format!("{:.0}k", n as f64 / 1e3),
        1_000_000..10_000_000 => format!("{:.2}M", n as f64 / 1e6),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use synaps_client::protocol::{DaemonFrame, decode_event, decode_frame};

    use super::*;

    fn attached(v: Value) -> Box<Attached> {
        match decode_frame(&v.to_string()).unwrap() {
            DaemonFrame::Attached(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn ev(v: Value) -> (SessionEvent, Arc<Value>) {
        (decode_event(&v).unwrap(), Arc::new(v))
    }

    #[test]
    fn attach_snapshot_feeds_view_tokens_cost_and_files() {
        let a = attached(json!({"type": "attached", "client": 2, "meta": {"id": "s", "cwd": "/w/r"},
            "view": {"model": "anthropic/claude-opus-4-1", "thinking_level": "low", "reasoning_level": "high", "context_window": 200000},
            "conversation": {"api_messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t", "name": "edit", "input": {"path": "/w/r/a.rs", "old_string": "a", "new_string": "b\nc"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": "ok"}]}],
                "cost": 1.25, "tokens": {"input": 10, "output": 20, "cache_read": 70, "cache_creation": 20}},
            "input_owner": 2}));
        let mut s = Stats::default();
        s.on_attached(&a);
        let v = s.view.clone().unwrap();
        assert_eq!((v.model.as_str(), v.effort.as_str(), v.context_window), ("anthropic/claude-opus-4-1", "high", 200000));
        assert_eq!(s.tokens, Some(Tokens { input: 10, output: 20, cache_read: 70, cache_creation: 20 }));
        assert_eq!(s.tokens.unwrap().cache_hit(), Some(0.7));
        assert_eq!(s.cost, Some(1.25));
        assert_eq!(s.context, None, "no model call seen yet: the gauge has nothing");
        assert_eq!(s.context_ratio(), None);
        assert_eq!(s.files.files()[0].path, "a.rs");
        assert_eq!(s.files.totals(), (2, 1));
    }

    #[test]
    fn an_older_daemon_without_view_or_tokens_shows_nothing() {
        let a = attached(json!({"type": "attached", "client": 2, "meta": {"id": "s"}, "conversation": {"api_messages": []}}));
        let mut s = Stats::default();
        s.on_attached(&a);
        assert_eq!((s.view.is_none(), s.tokens.is_none()), (true, true));
        assert_eq!(s.cost, None, "no tokens and a default 0.0: unknown, not $0.00");
        assert_eq!(Tokens::from_value(&json!({})), None);
        assert_eq!(Tokens::from_value(&json!({"foo": 1})), None);
        assert_eq!(Tokens::from_value(&json!("x")), None);
        assert_eq!(Tokens::default().cache_hit(), None);
    }

    #[test]
    fn usage_drives_the_gauge_and_digests_the_totals() {
        let mut s = Stats { view: Some(RuntimeInfo { context_window: 200_000, ..Default::default() }), ..Default::default() };
        let (e, raw) = ev(json!({"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 1000,
            "output_tokens": 50, "cache_read_input_tokens": 90000, "cache_creation_input_tokens": 9000, "model": "m"}}));
        s.on_session(&e, &raw);
        assert_eq!(s.context, Some(100_000));
        assert_eq!(s.context_ratio(), Some(0.5));
        // A call on another model (a compaction model) does not move the gauge.
        s.view.as_mut().unwrap().model = "anthropic/claude-opus-4-1".into();
        let (e, raw) = ev(json!({"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 5,
            "output_tokens": 5, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "model": "anthropic/claude-haiku-4-5"}}));
        s.on_session(&e, &raw);
        assert_eq!(s.context, Some(100_000));
        // The session's model, with or without the provider prefix, does.
        let (e, raw) = ev(json!({"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 1000,
            "output_tokens": 5, "cache_read_input_tokens": 99000, "cache_creation_input_tokens": 0, "model": "claude-opus-4-1"}}));
        s.on_session(&e, &raw);
        assert_eq!(s.context, Some(100_000));
        assert_eq!(s.context_ratio(), Some(0.5));
        let (e, raw) = ev(json!({"ev": "conversation", "digest": {"messages_len": 4, "messages_hash": 1, "cost": 0.5,
            "tokens": {"input": 1, "output": 2, "cache_read": 3, "cache_creation": 4}}}));
        s.on_session(&e, &raw);
        assert_eq!(s.tokens.unwrap().output, 2);
        assert_eq!(s.cost, Some(0.5));
        let (e, raw) = ev(json!({"ev": "setting_changed", "applied": {"setting": "model", "ok": true,
            "view": {"model": "m2", "thinking_level": "off", "reasoning_level": "off", "context_window": 1000000}}}));
        s.on_session(&e, &raw);
        assert_eq!(s.view.as_ref().unwrap().model, "m2");
        assert_eq!(s.context_ratio(), Some(0.1));
        let (e, raw) = ev(json!({"ev": "cleared", "session_id": "x"}));
        s.on_session(&e, &raw);
        assert_eq!(s.context, None);
    }

    #[test]
    fn keep_warm_pings_are_not_the_conversations_cache() {
        let a = attached(json!({"type": "attached", "client": 2, "meta": {"id": "s"},
            "conversation": {"api_messages": [], "cost": 0.0,
                "tokens": {"input": 100, "output": 50, "cache_read": 900, "cache_creation": 100},
                "keepwarm_tokens": {"input": 0, "output": 10, "cache_read": 800, "cache_creation": 0}}}));
        let mut s = Stats::default();
        s.on_attached(&a);
        assert_eq!(s.tokens, Some(Tokens { input: 100, output: 40, cache_read: 100, cache_creation: 100 }));
        assert_eq!(s.tokens.unwrap().cache_hit(), Some(1.0 / 3.0));
        assert_eq!(s.cost, Some(0.0), "tokens known: a real zero");
        // A digest's keep-warm share comes off too (from the wire event).
        let (e, raw) = ev(json!({"ev": "conversation", "digest": {"messages_len": 2, "messages_hash": 1, "cost": 0.1,
            "tokens": {"input": 100, "output": 50, "cache_read": 1900, "cache_creation": 100},
            "keepwarm_tokens": {"input": 0, "output": 10, "cache_read": 1800, "cache_creation": 0}}}));
        s.on_session(&e, &raw);
        assert_eq!(s.tokens.unwrap().cache_read, 100);
        assert!(!s.detached);
        s.on_detached();
        assert!(s.detached);
        // A re-attach of the same session is live again.
        s.on_attached(&a);
        assert!(!s.detached);
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(512), "512");
        assert_eq!(compact(18_500), "18.5k");
        assert_eq!(compact(250_000), "250k");
        assert_eq!(compact(1_850), "1.9k");
        assert_eq!(compact(1_410_000), "1.41M");
        assert_eq!(compact(25_000_000), "25.0M");
    }
}
