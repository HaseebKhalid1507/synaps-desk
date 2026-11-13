//! The transcript model a UI renders: history (`api_messages`) folded into
//! display items, then live stream events applied on top. Pure data, no GUI,
//! so it is unit-tested here and shared by the desktop app and its benches.
//!
//! Change tracking is splice-shaped for a virtual list: after a batch of
//! updates, [`Transcript::take_splice`] reports the first changed index and
//! the old/new lengths, so the UI remeasures only the rows that changed.

use std::collections::HashMap;

use serde_json::Value;

use crate::protocol::*;

/// Bound on the tool input/result previews kept per tool row.
const PREVIEW_MAX: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    User,
    Assistant,
    Thinking,
    Tool,
    Notice,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Streaming,
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    pub tool_id: String,
    pub name: String,
    pub input: String,
    pub result: String,
    pub status: Option<ToolStatus>,
}

#[derive(Debug, Clone)]
pub struct Item {
    /// Stable across edits (and reused when history replaces live rows).
    pub id: u64,
    /// Bumped on every change: a UI caches derived strings per `(id, rev)`.
    pub rev: u64,
    pub kind: ItemKind,
    pub text: String,
    pub tool: Option<ToolCall>,
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub old_len: usize,
    pub new_len: usize,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub items: Vec<Item>,
    next_id: u64,
    /// `api_messages` folded so far, and where their items end.
    history_len: usize,
    history_end: usize,
    /// Index of the first live item of the current response (`ResponseReset`).
    response_start: Option<usize>,
    tools: HashMap<String, usize>,
    pub busy: bool,
    pub title: Option<String>,
    pub pending_prompts: Vec<PromptRequest>,
    pub subagents: Vec<Value>,
    pub cost_usd: f64,
    dirty_from: Option<usize>,
    clean_len: usize,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn mark(&mut self, ix: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(ix, |d| d.min(ix)));
    }

    /// The change since the last call, if any.
    pub fn take_splice(&mut self) -> Option<Splice> {
        let start = self.dirty_from.take()?;
        let s = Splice { start: start.min(self.clean_len), old_len: self.clean_len, new_len: self.items.len() };
        self.clean_len = self.items.len();
        Some(s)
    }

    fn push(&mut self, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool, reuse: &mut Vec<(ItemKind, u64)>) -> usize {
        let id = match reuse.first() {
            Some(&(k, id)) if k == kind => {
                reuse.remove(0);
                id
            }
            _ => {
                self.next_id += 1;
                self.next_id
            }
        };
        let ix = self.items.len();
        if let Some(t) = &tool {
            self.tools.insert(t.tool_id.clone(), ix);
        }
        self.items.push(Item { id, rev: 0, kind, text, tool, streaming });
        self.mark(ix);
        ix
    }

    fn push_new(&mut self, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool) -> usize {
        self.push(kind, text, tool, streaming, &mut Vec::new())
    }

    fn touch(&mut self, ix: usize) {
        self.items[ix].rev += 1;
        self.mark(ix);
    }

    /// Replace everything with an attach snapshot (`Attached`), then replay
    /// the in-flight turn's events.
    pub fn load_attached(&mut self, a: &Attached) {
        let old_len = self.clean_len;
        *self = Transcript { next_id: self.next_id, ..Transcript::default() };
        self.clean_len = old_len;
        self.dirty_from = Some(0);
        self.title = a.meta.title.clone().or_else(|| a.conversation.header.get("title").and_then(Value::as_str).map(str::to_string));
        self.cost_usd = a.conversation.cost;
        self.pending_prompts = a.pending_prompts.clone();
        self.fold_history(&a.conversation.api_messages, &mut Vec::new());
        self.busy = a.streaming;
        for env in &a.replay {
            self.apply(&env.decode());
        }
        if !a.streaming {
            self.busy = false;
        }
    }

    /// `MessageHistory`: the authoritative history. Live rows since the last
    /// fold are replaced by their history form (ids reused by position).
    pub fn apply_history(&mut self, messages: &[Value]) {
        let mut reuse: Vec<(ItemKind, u64)>;
        if messages.len() < self.history_len {
            // Compaction / clear: start over.
            reuse = self.items.iter().map(|i| (i.kind, i.id)).collect();
            self.items.clear();
            self.tools.clear();
            self.history_len = 0;
            self.history_end = 0;
            self.mark(0);
            self.fold_history(messages, &mut reuse);
        } else {
            reuse = self.items[self.history_end..].iter().map(|i| (i.kind, i.id)).collect();
            self.items.truncate(self.history_end);
            self.tools.retain(|_, ix| *ix < self.history_end);
            self.mark(self.history_end);
            let start = self.history_len;
            self.fold_from(messages, start, &mut reuse);
        }
        self.response_start = None;
    }

    fn fold_history(&mut self, messages: &[Value], reuse: &mut Vec<(ItemKind, u64)>) {
        self.fold_from(messages, 0, reuse);
    }

    fn fold_from(&mut self, messages: &[Value], start: usize, reuse: &mut Vec<(ItemKind, u64)>) {
        for m in &messages[start.min(messages.len())..] {
            self.fold_message(m, reuse);
        }
        self.history_len = messages.len();
        self.history_end = self.items.len();
    }

    fn fold_message(&mut self, m: &Value, reuse: &mut Vec<(ItemKind, u64)>) {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        let content = m.get("content").unwrap_or(&Value::Null);
        match (role, content) {
            ("user", Value::String(s)) => {
                let kind = if s.starts_with("<event") || s.starts_with("<context-summary>") { ItemKind::Notice } else { ItemKind::User };
                self.push(kind, s.clone(), None, false, reuse);
            }
            ("assistant", Value::String(s)) => {
                self.push(ItemKind::Assistant, s.clone(), None, false, reuse);
            }
            (_, Value::Array(blocks)) => {
                let mut text = String::new();
                let flush = |this: &mut Self, text: &mut String, reuse: &mut Vec<(ItemKind, u64)>| {
                    if !text.trim().is_empty() {
                        let kind = if role == "assistant" { ItemKind::Assistant } else { ItemKind::User };
                        this.push(kind, std::mem::take(text), None, false, reuse);
                    }
                    text.clear();
                };
                for b in blocks {
                    match b.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text" => {
                            if !text.is_empty() {
                                text.push_str("\n\n");
                            }
                            text.push_str(b.get("text").and_then(Value::as_str).unwrap_or(""));
                        }
                        "thinking" => {
                            flush(self, &mut text, reuse);
                            let t = b.get("thinking").and_then(Value::as_str).unwrap_or("").to_string();
                            if !t.is_empty() {
                                self.push(ItemKind::Thinking, t, None, false, reuse);
                            }
                        }
                        "tool_use" => {
                            flush(self, &mut text, reuse);
                            let tool = ToolCall {
                                tool_id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                                name: b.get("name").and_then(Value::as_str).unwrap_or("tool").to_string(),
                                input: preview(&compact(b.get("input").unwrap_or(&Value::Null))),
                                result: String::new(),
                                status: Some(ToolStatus::Running),
                            };
                            self.push(ItemKind::Tool, String::new(), Some(tool), false, reuse);
                        }
                        "tool_result" => {
                            flush(self, &mut text, reuse);
                            let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                            let failed = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            let result = result_text(b.get("content").unwrap_or(&Value::Null));
                            if let Some(&ix) = self.tools.get(id) {
                                if let Some(t) = self.items[ix].tool.as_mut() {
                                    t.result = preview(&result);
                                    t.status = Some(if failed { ToolStatus::Failed } else { ToolStatus::Done });
                                }
                                self.touch(ix);
                            }
                        }
                        _ => {}
                    }
                }
                flush(self, &mut text, reuse);
            }
            _ => {}
        }
    }

    fn live_tail(&self, kind: ItemKind) -> Option<usize> {
        let ix = self.items.len().checked_sub(1)?;
        let it = &self.items[ix];
        (ix >= self.history_end && it.kind == kind && it.streaming).then_some(ix)
    }

    fn append_live(&mut self, kind: ItemKind, delta: &str) {
        if let Some(ix) = self.live_tail(kind) {
            self.items[ix].text.push_str(delta);
            self.touch(ix);
        } else {
            self.end_streaming_text();
            self.push_new(kind, delta.to_string(), None, true);
        }
    }

    fn end_streaming_text(&mut self) {
        if let Some(ix) = self.items.len().checked_sub(1)
            && self.items[ix].streaming
            && self.items[ix].kind != ItemKind::Tool
        {
            self.items[ix].streaming = false;
            self.touch(ix);
        }
    }

    fn tool_mut(&mut self, tool_id: &str) -> Option<usize> {
        self.tools.get(tool_id).copied().filter(|&ix| ix < self.items.len())
    }

    fn notice(&mut self, kind: ItemKind, text: impl Into<String>) {
        self.end_streaming_text();
        self.push_new(kind, text.into(), None, false);
    }

    /// Apply one live session event.
    pub fn apply(&mut self, ev: &SessionEvent) {
        match ev {
            SessionEvent::TurnStarted { user_text, .. } => {
                self.busy = true;
                if let Some(t) = user_text {
                    self.push_new(ItemKind::User, t.clone(), None, false);
                }
            }
            SessionEvent::Stream { event } => self.apply_stream(event),
            SessionEvent::Conversation { digest } => {
                if let Some(t) = digest.pointer("/header/title").and_then(Value::as_str) {
                    self.title = Some(t.to_string());
                }
                if let Some(c) = digest.get("cost").and_then(Value::as_f64) {
                    self.cost_usd = c;
                }
            }
            SessionEvent::Idle => {
                self.busy = false;
                self.end_streaming_text();
            }
            SessionEvent::Prompt { request } => self.pending_prompts.push(request.clone()),
            SessionEvent::PromptResolved { prompt_id } => self.pending_prompts.retain(|p| p.id != *prompt_id),
            SessionEvent::Steered { text, .. } => self.notice(ItemKind::Notice, format!("steered: {text}")),
            SessionEvent::SystemNotice { text } => self.notice(ItemKind::Notice, text.clone()),
            SessionEvent::Aborted { .. } => {
                self.busy = false;
                self.notice(ItemKind::Notice, "turn aborted");
            }
            SessionEvent::Ended { reason } => self.notice(ItemKind::Notice, format!("session ended ({reason})")),
            SessionEvent::CompactionFailed { message } => self.notice(ItemKind::Error, format!("compaction failed: {message}")),
            SessionEvent::SubagentRows { rows } => self.subagents = rows.clone(),
            SessionEvent::Reloading { .. } => self.notice(ItemKind::Notice, "daemon reloading, reconnecting..."),
            _ => {}
        }
    }

    fn apply_stream(&mut self, ev: &StreamEvent) {
        match ev {
            StreamEvent::Llm(l) => match l {
                LlmEvent::ResponseStart => {
                    self.end_streaming_text();
                    self.response_start = Some(self.items.len());
                }
                LlmEvent::ResponseReset => {
                    if let Some(s) = self.response_start.filter(|&s| s >= self.history_end && s <= self.items.len()) {
                        self.items.truncate(s);
                        self.tools.retain(|_, ix| *ix < s);
                        self.mark(s);
                    }
                }
                LlmEvent::Thinking { text } => self.append_live(ItemKind::Thinking, text),
                LlmEvent::Text { text } => self.append_live(ItemKind::Assistant, text),
                LlmEvent::ToolUseStart { tool_name, tool_id } => {
                    self.end_streaming_text();
                    let tool = ToolCall {
                        tool_id: tool_id.clone(),
                        name: tool_name.clone(),
                        status: Some(ToolStatus::Streaming),
                        ..Default::default()
                    };
                    self.push_new(ItemKind::Tool, String::new(), Some(tool), true);
                }
                LlmEvent::ToolUseDelta { tool_id, delta } => {
                    if let Some(ix) = self.tool_mut(tool_id) {
                        if let Some(t) = self.items[ix].tool.as_mut()
                            && t.input.len() < PREVIEW_MAX
                        {
                            t.input.push_str(delta);
                        }
                        self.touch(ix);
                    }
                }
                LlmEvent::ToolUse { tool_name, tool_id, input } => {
                    let ix = match self.tool_mut(tool_id) {
                        Some(ix) => ix,
                        None => self.push_new(
                            ItemKind::Tool,
                            String::new(),
                            Some(ToolCall { tool_id: tool_id.clone(), ..Default::default() }),
                            true,
                        ),
                    };
                    if let Some(t) = self.items[ix].tool.as_mut() {
                        t.name = tool_name.clone();
                        t.input = preview(&compact(input));
                        t.status = Some(ToolStatus::Running);
                    }
                    self.touch(ix);
                }
                LlmEvent::ToolQueued { tool_id } | LlmEvent::ToolStarted { tool_id } => {
                    let queued = matches!(l, LlmEvent::ToolQueued { .. });
                    if let Some(ix) = self.tool_mut(tool_id) {
                        if let Some(t) = self.items[ix].tool.as_mut() {
                            t.status = Some(if queued { ToolStatus::Queued } else { ToolStatus::Running });
                        }
                        self.touch(ix);
                    }
                }
                LlmEvent::ToolResultDelta { tool_id, delta } => {
                    if let Some(ix) = self.tool_mut(tool_id) {
                        if let Some(t) = self.items[ix].tool.as_mut()
                            && t.result.len() < PREVIEW_MAX
                        {
                            t.result.push_str(delta);
                        }
                        self.touch(ix);
                    }
                }
                LlmEvent::ToolResult { tool_id, result } => {
                    if let Some(ix) = self.tool_mut(tool_id) {
                        if let Some(t) = self.items[ix].tool.as_mut() {
                            t.result = preview(result);
                            t.status = Some(ToolStatus::Done);
                        }
                        self.items[ix].streaming = false;
                        self.touch(ix);
                    }
                }
                LlmEvent::Unknown => {}
            },
            StreamEvent::Session(s) => match s {
                SessionStreamEvent::MessageHistory { messages } => self.apply_history(messages),
                SessionStreamEvent::Error { message } => self.notice(ItemKind::Error, message.clone()),
                SessionStreamEvent::Notice { text, .. } => self.notice(ItemKind::Notice, text.clone()),
                SessionStreamEvent::Done => self.end_streaming_text(),
                _ => {}
            },
            StreamEvent::Agent(a) => match a {
                AgentEvent::SubagentStart { agent_name, task_preview, .. } => {
                    self.notice(ItemKind::Notice, format!("subagent {agent_name} started: {task_preview}"))
                }
                AgentEvent::SubagentDone { agent_name, duration_secs, .. } => {
                    self.notice(ItemKind::Notice, format!("subagent {agent_name} done ({duration_secs:.1}s)"))
                }
                _ => {}
            },
            StreamEvent::Unknown => {}
        }
    }
}

fn compact(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Object(m) if m.len() == 1 => match m.values().next() {
            Some(Value::String(s)) => s.clone(),
            _ => v.to_string(),
        },
        _ => v.to_string(),
    }
}

fn result_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn preview(s: &str) -> String {
    if s.len() <= PREVIEW_MAX {
        return s.to_string();
    }
    let mut end = PREVIEW_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
