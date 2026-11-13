//! The transcript model a UI renders: history (`api_messages`) folded into
//! display items, then live stream events applied on top. Pure data, no GUI,
//! so it is unit-tested here and shared by the desktop app and its benches.
//!
//! Feed it [`Transcript::load_attached`] on every `Event::Attached`,
//! [`Transcript::apply_update`] on every `Event::History` and
//! [`Transcript::apply`] on every `Event::Session`. It needs
//! `HistoryMode::Full` and refuses a `Digest` attach.
//!
//! Notices and errors (turn errors, "turn aborted", subagent start/done,
//! system notices) are not part of the history; they are kept across
//! refolds, anchored to the history position they arrived at.
//!
//! Change tracking is splice-shaped for a virtual list: after a batch of
//! updates, [`Transcript::take_splice`] reports the first changed index and
//! the old/new lengths, so the UI remeasures only the rows that changed.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use crate::history::HistoryUpdate;
use crate::protocol::*;

/// Bound on the tool input/result previews kept per tool row.
const PREVIEW_MAX: usize = 4 * 1024;
/// How far ahead id reuse looks for a row of the same kind.
const REUSE_LOOKAHEAD: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ItemKind {
    User,
    Assistant,
    Thinking,
    Tool,
    Notice,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolStatus {
    Streaming,
    Queued,
    Running,
    Done,
    Failed,
    /// The turn ended (abort, interruption) before the tool reported.
    Cancelled,
}

impl ToolStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, ToolStatus::Done | ToolStatus::Failed | ToolStatus::Cancelled)
    }
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
    /// `Some(n)`: a client-side row (notice, error), not part of the
    /// history, shown after the first `n` history messages.
    anchor: Option<usize>,
}

impl Item {
    /// A notice or error this client added (not part of the history).
    pub fn is_note(&self) -> bool {
        self.anchor.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub old_len: usize,
    pub new_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TranscriptError {
    /// The attach carries a digest, not the history (`HistoryMode::Digest`).
    DigestHistory,
    /// An append for a history this transcript does not have (`base_len`
    /// differs from what was folded): call `Client::resync`.
    OutOfSync { have: usize, base_len: usize },
}

impl std::fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DigestHistory => f.write_str("the transcript needs HistoryMode::Full; this attach carries a digest"),
            Self::OutOfSync { have, base_len } => {
                write!(f, "history append from message {base_len}, but {have} are folded; resync")
            }
        }
    }
}

impl std::error::Error for TranscriptError {}

/// Ids (and revs) of rows about to be rebuilt, for reuse by kind.
type Reuse = VecDeque<(ItemKind, u64, u64)>;

#[derive(Debug, Default)]
pub struct Transcript {
    items: Vec<Item>,
    next_id: u64,
    session_id: Option<String>,
    /// `api_messages` folded so far, and where their items end.
    history_len: usize,
    history_end: usize,
    /// Index of the first live item of the current response (`ResponseReset`).
    response_start: Option<usize>,
    tools: HashMap<String, usize>,
    busy: bool,
    title: Option<String>,
    pending_prompts: Vec<PromptRequest>,
    subagents: Vec<Value>,
    cost_usd: f64,
    /// The conversation was swapped (clear, resume, compaction): the next
    /// replacement drops the notes of the old one.
    swapped: bool,
    dirty_from: Option<usize>,
    clean_len: usize,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// A turn is running.
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn pending_prompts(&self) -> &[PromptRequest] {
        &self.pending_prompts
    }

    pub fn subagents(&self) -> &[Value] {
        &self.subagents
    }

    pub fn cost_usd(&self) -> f64 {
        self.cost_usd
    }

    /// History messages folded so far.
    pub fn history_len(&self) -> usize {
        self.history_len
    }

    /// The session of the last [`Transcript::load_attached`].
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
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

    fn push(&mut self, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool, reuse: &mut Reuse) -> usize {
        // Reuse the next row of this kind within a short lookahead, dropping
        // the rows skipped over (each entry is consumed at most once).
        let found = reuse.iter().take(REUSE_LOOKAHEAD).position(|&(k, ..)| k == kind);
        let (id, rev) = match found {
            Some(p) => {
                let (_, id, rev) = reuse.drain(..=p).next_back().expect("p is in range");
                (id, rev + 1)
            }
            None => {
                self.next_id += 1;
                (self.next_id, 0)
            }
        };
        let ix = self.items.len();
        if let Some(t) = &tool {
            self.tools.insert(t.tool_id.clone(), ix);
        }
        self.items.push(Item { id, rev, kind, text, tool, streaming, anchor: None });
        self.mark(ix);
        ix
    }

    fn push_new(&mut self, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool) -> usize {
        self.push(kind, text, tool, streaming, &mut Reuse::new())
    }

    /// Re-insert a kept note (same id; rev bumped as its index may differ).
    fn push_note(&mut self, mut item: Item, anchor: usize) {
        let ix = self.items.len();
        item.anchor = Some(anchor);
        item.rev += 1;
        self.items.push(item);
        self.mark(ix);
    }

    fn touch(&mut self, ix: usize) {
        self.items[ix].rev += 1;
        self.mark(ix);
    }

    /// Take every row, split into notes (kept) and reusable ids.
    fn take_rows(&mut self, from: usize) -> (Vec<Item>, Reuse) {
        self.mark(from);
        self.tools.retain(|_, ix| *ix < from);
        let mut notes = Vec::new();
        let mut reuse = Reuse::new();
        for it in self.items.drain(from..) {
            if it.anchor.is_some() {
                notes.push(it);
            } else {
                reuse.push_back((it.kind, it.id, it.rev));
            }
        }
        (notes, reuse)
    }

    /// Replace everything with an attach snapshot (`Attached`), then replay
    /// the in-flight turn's events. A re-attach to the same session (after a
    /// reload) keeps row ids and notes; another session starts fresh.
    pub fn load_attached(&mut self, a: &Attached) -> Result<(), TranscriptError> {
        if a.conversation.api_messages.is_empty() && a.conversation.messages_len > 0 {
            return Err(TranscriptError::DigestHistory);
        }
        let same = self.session_id.as_deref() == Some(a.meta.id.as_str());
        let (notes, mut reuse) = if same { self.take_rows(0) } else { (Vec::new(), Reuse::new()) };
        let (next_id, clean_len) = (self.next_id, self.clean_len);
        *self = Transcript { next_id, clean_len, session_id: Some(a.meta.id.clone()), ..Transcript::default() };
        self.dirty_from = Some(0);
        self.title = a.meta.title.clone().or_else(|| a.conversation.header.get("title").and_then(Value::as_str).map(str::to_string));
        self.cost_usd = a.conversation.cost;
        self.pending_prompts = a.pending_prompts.clone();
        self.fold_with_notes(&a.conversation.api_messages, notes, &mut reuse);
        self.busy = a.streaming;
        for env in &a.replay {
            if let Ok(ev) = env.decode() {
                self.apply(&ev);
            }
        }
        if !a.streaming {
            self.busy = false;
            self.settle();
        }
        Ok(())
    }

    /// An `Event::History` update from the client.
    pub fn apply_update(&mut self, update: &HistoryUpdate) -> Result<(), TranscriptError> {
        match update {
            HistoryUpdate::Append { base_len, messages } => {
                if *base_len != self.history_len {
                    return Err(TranscriptError::OutOfSync { have: self.history_len, base_len: *base_len });
                }
                self.append_history(messages);
            }
            HistoryUpdate::Replace { messages } => self.replace_history(messages),
        }
        Ok(())
    }

    /// A full `MessageHistory` without a mirror (benches, digest-less
    /// replays): longer or equal is taken as an append, shorter as a
    /// replacement. Clients get [`HistoryUpdate`]s instead, which tell.
    pub fn apply_history(&mut self, messages: &[Value]) {
        if messages.len() >= self.history_len {
            let start = self.history_len;
            self.append_history(&messages[start..]);
        } else {
            self.replace_history(messages);
        }
    }

    /// Fold `messages` after the history folded so far. Live rows since
    /// the last fold are replaced by their history form (ids reused by
    /// kind); notes among them stay, after the new rows.
    fn append_history(&mut self, messages: &[Value]) {
        let (notes, mut reuse) = self.take_rows(self.history_end);
        for m in messages {
            self.fold_message(m, &mut reuse);
        }
        self.history_len += messages.len();
        for n in notes {
            self.push_note(n, self.history_len);
        }
        self.history_end = self.items.len();
        self.response_start = None;
        if !self.busy {
            self.settle();
        }
    }

    /// Rebuild from a whole history (resync, rewrite, clear, compaction).
    fn replace_history(&mut self, messages: &[Value]) {
        // This turn's notes (after the last fold) go at the end; after a
        // conversation swap they are the only ones that still apply.
        let live_from = self.history_end.min(self.items.len());
        for it in &mut self.items[live_from..] {
            if it.anchor.is_some() {
                it.anchor = Some(usize::MAX);
            }
        }
        let (mut notes, mut reuse) = self.take_rows(0);
        if std::mem::take(&mut self.swapped) {
            notes.retain(|n| n.anchor == Some(usize::MAX));
        }
        self.fold_with_notes(messages, notes, &mut reuse);
        self.response_start = None;
        if !self.busy {
            self.settle();
        }
    }

    /// Fold a whole history from empty, interleaving `notes` by anchor.
    fn fold_with_notes(&mut self, messages: &[Value], mut notes: Vec<Item>, reuse: &mut Reuse) {
        notes.sort_by_key(|n| n.anchor);
        let mut notes = notes.into_iter().peekable();
        for (i, m) in messages.iter().enumerate() {
            while let Some(n) = notes.next_if(|n| n.anchor.unwrap_or(0) <= i) {
                self.push_note(n, i);
            }
            self.fold_message(m, reuse);
        }
        for n in notes {
            self.push_note(n, messages.len());
        }
        self.history_len = messages.len();
        self.history_end = self.items.len();
    }

    /// No turn runs: nothing streams and no tool is still pending. Tools
    /// without a result (aborted, interrupted) end `Cancelled`.
    fn settle(&mut self) {
        for ix in 0..self.items.len() {
            let it = &mut self.items[ix];
            let open_tool = it.tool.as_ref().and_then(|t| t.status).is_some_and(|s| !s.is_terminal());
            if !it.streaming && !open_tool {
                continue;
            }
            it.streaming = false;
            if let Some(t) = it.tool.as_mut().filter(|_| open_tool) {
                t.status = Some(ToolStatus::Cancelled);
            }
            self.touch(ix);
        }
    }

    /// The attachment ended: nothing will stream any more.
    pub fn set_detached(&mut self) {
        self.busy = false;
        self.settle();
    }

    fn fold_message(&mut self, m: &Value, reuse: &mut Reuse) {
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
                let flush = |this: &mut Self, text: &mut String, reuse: &mut Reuse| {
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

    /// A client-side row: kept across history refolds.
    fn notice(&mut self, kind: ItemKind, text: impl Into<String>) {
        self.end_streaming_text();
        let ix = self.push_new(kind, text.into(), None, false);
        self.items[ix].anchor = Some(self.history_len);
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
                if let Some(t) = digest.header.get("title").and_then(Value::as_str) {
                    self.title = Some(t.to_string());
                }
                self.cost_usd = digest.cost;
            }
            SessionEvent::Idle => {
                self.busy = false;
                self.settle();
            }
            SessionEvent::Prompt { request } => self.pending_prompts.push(request.clone()),
            SessionEvent::PromptResolved { prompt_id } => self.pending_prompts.retain(|p| p.id != *prompt_id),
            SessionEvent::Steered { text, .. } => self.notice(ItemKind::Notice, format!("steered: {text}")),
            SessionEvent::SystemNotice { text } => self.notice(ItemKind::Notice, text.clone()),
            SessionEvent::Aborted { .. } => {
                self.busy = false;
                self.settle();
                self.notice(ItemKind::Notice, "turn aborted");
            }
            SessionEvent::Ended { reason } => self.notice(ItemKind::Notice, format!("session ended ({reason})")),
            SessionEvent::Cleared { .. } => {
                self.swapped = true;
                self.notice(ItemKind::Notice, "conversation cleared");
            }
            SessionEvent::Resumed { new_id, clamp_notice, .. } => {
                self.swapped = true;
                self.notice(ItemKind::Notice, format!("resumed {new_id}"));
                if let Some(n) = clamp_notice {
                    self.notice(ItemKind::Notice, n.clone());
                }
            }
            SessionEvent::CompactionApplied { msg_count, .. } => {
                self.swapped = true;
                self.notice(ItemKind::Notice, format!("conversation compacted ({msg_count} messages)"));
            }
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
