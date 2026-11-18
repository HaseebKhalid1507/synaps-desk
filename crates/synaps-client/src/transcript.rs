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
//! An engine event ([`crate::events`]: `synaps send`, agent-comms, a
//! watcher) is one [`ItemKind::Event`] row, never a user row, whichever way
//! it came: the live `External` frame, the `steering_delivered` that
//! carried it into a running turn, or its `<event …>` message in the
//! history. One event is one row: a live row is a note until its history
//! message lands, then it is that message's row (same id, its live time and
//! sender kept). The engine's notes about the session's own workers
//! ([`crate::events::is_worker_note`]) stay notices from the history, as
//! before, and add no live row.
//!
//! The frame comes when the engine drains the event; the history has it
//! where the engine delivered it, which can be later: between two rounds
//! (a steer), at the turn's end (one buffered, or steered and not taken).
//! Until then its live row is held last, every row that streams meanwhile
//! going above it, so it never splits a response and never jumps when its
//! message lands. Delivered (`steering_delivered` with its form, a turn's
//! start, end or abort, or its message in the history), it stays where it
//! is, where the history has it. A stream retry (`response_reset`) keeps
//! it. A `never_persist` event never reaches the history: its row is placed
//! as any live row, and stays a note.
//!
//! A turn that fails (its stream's `error`, or a stream that ends with no
//! end) or a compaction that fails or is cancelled ends with events the
//! engine still holds (put back, or buffered): they reach the history only
//! at the next turn's end (agent-engine `actor.rs` `clear_stream`, the
//! `Done` flush). Their rows are stuck: held last through the next turn,
//! until their message lands (or the engine, by its digest's
//! `pending_events_len`, holds none). A `/clear` drops what the engine
//! held, and so their rows.
//!
//! The owner's steer in an event's form is the owner's `steered:` note;
//! its `steering_delivered`, with no frame, adds the event's row under it
//! (Shady #22 L2′), as the history has an event's message there: the TUI
//! shows both too.
//!
//! Change tracking is splice-shaped for a virtual list: after a batch of
//! updates, [`Transcript::take_splice`] reports the first changed index and
//! the old/new lengths, so the UI remeasures only the rows that changed.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde_json::Value;

use crate::events::ExternalEvent;
use crate::history::HistoryUpdate;
use crate::protocol::*;

/// Bound on the tool input/result previews kept per tool row.
const PREVIEW_MAX: usize = 4 * 1024;
/// How much of a cut result's end a call keeps ([`ToolCall::tail`]).
const TAIL_MAX: usize = 512;
/// How far ahead id reuse looks for a row of the same kind.
const REUSE_LOOKAHEAD: usize = 8;
/// The host's record time of a history message (agent-core
/// `session_journal::MESSAGE_AT_KEY`): RFC 3339, UTC, whole seconds. Only
/// new messages carry it; a legacy one has none.
pub const MESSAGE_AT_KEY: &str = "_synaps_at";

/// When a history message was recorded ([`MESSAGE_AT_KEY`]), if it was.
pub fn message_at(m: &Value) -> Option<&str> {
    m.get(MESSAGE_AT_KEY).and_then(Value::as_str).filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ItemKind {
    User,
    Assistant,
    Thinking,
    Tool,
    Notice,
    Error,
    /// A slash command's answer ([`Transcript::output`]): this client's,
    /// never sent and not in the history.
    Output,
    /// An engine event ([`Item::event`]); `text` is its text as shown.
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolStatus {
    Streaming,
    Queued,
    Running,
    Done,
    Failed,
    /// The turn ended (abort, interruption) before the tool reported, or
    /// the result says the call was canceled.
    Cancelled,
    /// Not run: an extension blocked it (a denied confirmation included) or
    /// the session's tool gate refused it.
    Denied,
    /// Ran, but did not finish on its own and the daemon does not know
    /// whether it succeeded: a `bash` command it stopped at its output limit
    /// (the result is still a success result).
    Unknown,
}

impl ToolStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, ToolStatus::Done | ToolStatus::Failed | ToolStatus::Cancelled | ToolStatus::Denied | ToolStatus::Unknown)
    }
}

/// How a finished tool call ended, read from its `tool_result`.
///
/// The engine sets `is_error` only on budget, parse and checkpoint refusals
/// (agent-engine `runtime/stream.rs`). A tool that failed, a call an
/// extension blocked (a denied confirmation is one), a gate denial and a
/// canceled call all come back as plain text, so the result is classified by
/// its text as the Synaps TUI does (`agent-tui render.rs`: a result starting
/// `Tool execution failed` or `Unknown tool` is an error), extended to the
/// engine's other non-success texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolOutcome {
    Ok,
    Failed,
    Denied,
    Cancelled,
    /// Neither a success nor a failure: the daemon stopped the command at
    /// its output limit and says its exit status is unknown.
    Unknown,
}

impl ToolOutcome {
    pub fn status(self) -> ToolStatus {
        match self {
            ToolOutcome::Ok => ToolStatus::Done,
            ToolOutcome::Failed => ToolStatus::Failed,
            ToolOutcome::Denied => ToolStatus::Denied,
            ToolOutcome::Cancelled => ToolStatus::Cancelled,
            ToolOutcome::Unknown => ToolStatus::Unknown,
        }
    }
}

/// `CancelCause::phrase()` (agent-engine `runtime/types.rs`): a canceled
/// call's whole result, or `<phrase>.` plus a side-effect warning.
const CANCEL_PHRASES: [&str; 5] = [
    "Canceled by user",
    "Canceled (session cost cap reached)",
    "Canceled (Synaps restarted)",
    "Canceled (session stopped by the host)",
    "Canceled (session driver revoked)",
];

/// `activation_denied` (agent-engine `tools/discovery.rs`) as a tool error:
/// every `HostActivationError`, refusals and batch failures alike.
const ACTIVATION_DENIED: &str = "Tool execution failed: tool activation denied:";
/// The one `HostActivationError` that is a refusal, `ConfirmationRequired`
/// (`tools/activation.rs`): the user denied the prompt, the policy is
/// `deny`, or nothing could show a prompt.
const ACTIVATION_REFUSED: &str = "tool activation requires host confirmation authority";

/// The last line of a `bash` result whose output passed the daemon's limit
/// (agent-engine `tools/bash.rs`): `[output truncated at 262144]` (v0.11)
/// or, since `output_limit_notice`, `[output truncated at N bytes: the
/// command was stopped at this limit before it finished, so its exit status
/// is unknown. …]`. Either way the daemon killed the command there (the
/// process group, `kill_now`) and returned the result as a success.
const OUTPUT_LIMIT: &str = "[output truncated at ";
/// The note that ends a virtualised history result (agent-engine
/// `runtime/tool_spill.rs`, on by default above 12 KiB): the result's head,
/// a gap, its tail (the notice above ends it), a blank line, then this.
const VIRTUALISED: &str = "[Output virtualised: ";

/// Classify a tool result's text (`is_error` as the block carries it).
pub fn tool_outcome(result: &str, is_error: bool) -> ToolOutcome {
    // `RuntimeError::Tool` and the activation gate's `Unknown tool:` (the
    // TUI's own test), the budget's `Tool call not executed`, and a
    // checkpoint refusal.
    const FAILED: [&str; 4] = ["Tool execution failed", "Unknown tool: ", "Tool call not executed", "No tools executed:"];
    // An extension's `Block` (`Tool call blocked by extension: Tool call
    // confirmation denied: …` after a denied prompt), `GateDenial`, and an
    // `activate_tools` the user did not confirm (`tools/discovery.rs`
    // `activation_denied`, a `RuntimeError::Tool`). Its other errors (a
    // guessed id, a duplicate, an already active tool: after AUTHORIZE)
    // are failures.
    const DENIED: [&str; 2] = ["Tool call blocked by extension", "Tool call denied:"];
    if let Some(why) = result.strip_prefix(ACTIVATION_DENIED) {
        return if why.trim_start().starts_with(ACTIVATION_REFUSED) { ToolOutcome::Denied } else { ToolOutcome::Failed };
    }
    if DENIED.iter().any(|p| result.starts_with(p)) {
        return ToolOutcome::Denied;
    }
    if FAILED.iter().any(|p| result.starts_with(p)) {
        return ToolOutcome::Failed;
    }
    let canceled = CANCEL_PHRASES.iter().any(|p| result.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with('.')))
        || result == "Operation canceled"
        // Partial output, then `[<phrase> before the tool finished; …]`.
        || result.trim_end().rsplit('\n').next().is_some_and(|l| {
            l.starts_with('[') && CANCEL_PHRASES.iter().any(|p| l[1..].starts_with(p)) && l.contains("before the tool finished")
        });
    let stopped = || {
        let mut lines = result.trim_end().rsplit('\n').filter(|l| !l.trim().is_empty());
        let last = lines.next().filter(|l| !l.starts_with(VIRTUALISED)).or_else(|| lines.next());
        last.is_some_and(|l| l.starts_with(OUTPUT_LIMIT) && l.trim_end().ends_with(']'))
    };
    if canceled {
        ToolOutcome::Cancelled
    } else if is_error {
        ToolOutcome::Failed
    } else if stopped() {
        ToolOutcome::Unknown
    } else {
        ToolOutcome::Ok
    }
}

/// [`tool_outcome`] of a history `tool_result` block (`content` a string or
/// text parts, `is_error` optional).
pub fn tool_result_outcome(block: &Value) -> ToolOutcome {
    let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    tool_outcome(&result_text(block.get("content").unwrap_or(&Value::Null)), is_error)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub tool_id: String,
    pub name: String,
    pub input: String,
    pub result: String,
    /// The result's end when [`preview`] cut it (`result` then ends `…`):
    /// its last whole lines, at most [`TAIL_MAX`] bytes; empty otherwise. A
    /// row says what the whole result held from it (a read's last line, a
    /// note the engine ends with), not from the preview's slice.
    pub tail: String,
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
    /// When it happened, RFC 3339, the first time the row had: a live
    /// row's is the `ts` of the event that started it
    /// ([`Transcript::apply_at`], an attach's replay), kept when its history
    /// form lands; a row first seen in history has its message time
    /// ([`message_at`]). `None`: no recorded time (legacy history, a row fed
    /// without one), never "now". Not part of `rev`: it can change with
    /// `rev` unchanged (a live row without one, then its history form; a
    /// row of a swapped conversation), so key anything derived from it on
    /// it.
    pub at: Option<Arc<str>>,
    /// An [`ItemKind::Event`] row's event, as shown
    /// ([`ExternalEvent::shown`]; from the history: what its form keeps).
    pub event: Option<Arc<ExternalEvent>>,
    /// An event row's history form, read ([`ExternalEvent::history`]): what
    /// tells its message, a replay of its frame, or another event with the
    /// same id.
    form: Option<Arc<ExternalEvent>>,
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
    /// The journal of the conversation shown (the attach's header `id`),
    /// kept current by clear, resume and compaction, whose events name the
    /// new one. The session's id stays the same through those; a re-attach
    /// to another journal is another conversation, and carries no time of
    /// this one's.
    journal_id: String,
    /// `api_messages` folded so far, and where their items end.
    history_len: usize,
    history_end: usize,
    /// Index of the first live item of the current response (`ResponseReset`).
    response_start: Option<usize>,
    tools: HashMap<String, usize>,
    busy: bool,
    title: Option<String>,
    pending_prompts: Vec<PromptRequest>,
    /// The last prompts resolved, each with whether a turn ran as it did
    /// ([`Transcript::resolved_busy`]).
    resolved: VecDeque<(u64, bool)>,
    subagents: Vec<Value>,
    cost_usd: f64,
    /// The conversation was swapped (clear, resume, compaction): the next
    /// replacement drops the notes of the old one.
    swapped: bool,
    dirty_from: Option<usize>,
    clean_len: usize,
    /// The rows a rebuild took out, until [`Transcript::keep_revs`].
    before: Vec<Item>,
    /// No notices for subagent starts and ends (a UI that shows the
    /// workers itself): [`Transcript::set_subagent_notices`].
    no_subagent_notices: bool,
    /// The time new rows get ([`Item::at`]): the message being folded's,
    /// or the event being applied's once a row takes it.
    stamp: Option<Arc<str>>,
    /// The `ts` of the event being applied (empty: none), shared into
    /// `stamp` by the first row it starts: most events start no row, so
    /// most never allocate.
    event_ts: String,
    /// Live event rows whose history message is in the fold under way: the
    /// message takes its row ([`Transcript::fold_event`]).
    held: Vec<Item>,
    /// Live event rows the engine has not delivered yet (row ids): always
    /// the last rows, live rows going in above them ([`Transcript::live_end`]).
    pending: Vec<u64>,
    /// Of `pending`, the rows of events the engine kept past their turn (a
    /// failed turn or compaction): a turn's start or end does not deliver
    /// them; their message does ([`Transcript::stick`]).
    stuck: Vec<u64>,
    /// Events the engine holds, by its last digest (`pending_events_len`).
    engine_pending: usize,
    /// An attach's replay is applied: the one time a frame can come after
    /// its own history row ([`same_event`]).
    replaying: bool,
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

    /// Whether a turn ran when prompt `id` resolved: an answer lets it go
    /// on, a cancel resolves it first and ends the turn after. `None`: not
    /// among the last few resolved here.
    pub fn resolved_busy(&self, id: u64) -> Option<bool> {
        self.resolved.iter().rev().find(|r| r.0 == id).map(|r| r.1)
    }

    pub fn subagents(&self) -> &[Value] {
        &self.subagents
    }

    /// Whether a live `SubagentStart` / `SubagentDone` adds a notice
    /// ("subagent scout started: …"); on by default. A UI that shows the
    /// workers under their calls turns them off. Kept across attaches.
    pub fn set_subagent_notices(&mut self, on: bool) {
        self.no_subagent_notices = !on;
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
        self.push_at(self.items.len(), kind, text, tool, streaming, reuse)
    }

    /// A row at `ix` (rows after it move down one).
    fn push_at(&mut self, ix: usize, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool, reuse: &mut Reuse) -> usize {
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
        if ix < self.items.len() {
            for t in self.tools.values_mut().filter(|t| **t >= ix) {
                *t += 1;
            }
        }
        if let Some(t) = &tool {
            self.tools.insert(t.tool_id.clone(), ix);
        }
        if self.stamp.is_none() && !self.event_ts.is_empty() {
            self.stamp = Some(Arc::from(self.event_ts.as_str()));
        }
        self.items.insert(ix, Item { id, rev, kind, text, tool, streaming, at: self.stamp.clone(), event: None, form: None, anchor: None });
        self.mark(ix);
        ix
    }

    /// A live row: above the events not delivered yet.
    fn push_new(&mut self, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool) -> usize {
        self.push_at(self.live_end(), kind, text, tool, streaming, &mut Reuse::new())
    }

    /// Where live rows end: the first of the last rows that are events not
    /// delivered yet ([`Transcript::pending`]), else the end.
    fn live_end(&self) -> usize {
        let mut end = self.items.len();
        while end > self.history_end && self.pending.contains(&self.items[end - 1].id) {
            end -= 1;
        }
        end
    }

    /// The engine dropped the events it held, with the conversation they
    /// were for (a `/clear`, Shady #22 N10; a `/resume`, N10′): so their
    /// rows, the held run at the end.
    fn drop_held(&mut self) {
        let run = self.live_end();
        if run < self.items.len() {
            self.items.truncate(run);
            self.mark(run);
        }
        self.pending.clear();
        self.stuck.clear();
    }

    /// Every event not delivered yet is (a turn started, ended or was
    /// aborted: the engine has put them in the conversation), but those it
    /// kept ([`Transcript::stuck`]): their rows stay where they are, and
    /// live rows go after them. The stuck ones stay held, last (under the
    /// others, if any came after them). A `never_persist` one right after
    /// the history is part of it now, kept there as a note is (no message
    /// will ever take it).
    fn deliver_all(&mut self) {
        let run = self.live_end();
        let pending = std::mem::take(&mut self.pending);
        if !self.stuck.is_empty() {
            let stuck = &self.stuck;
            let (mut rows, held): (Vec<Item>, Vec<Item>) = self.items.drain(run..).partition(|i| !stuck.contains(&i.id));
            rows.extend(held);
            self.items.extend(rows);
            self.mark(run);
            self.pending = pending.iter().copied().filter(|p| self.stuck.contains(p)).collect();
        }
        while let Some(it) = self.items.get(self.history_end).filter(|i| pending.contains(&i.id) && !self.pending.contains(&i.id)) {
            if !never_persist(it) {
                break;
            }
            self.history_end += 1;
        }
    }

    /// The engine kept the events it had not delivered (a failed turn puts
    /// back those steered and never taken; a failed or cancelled compaction
    /// keeps those buffered): until the next turn's end, which flushes them
    /// into the history. Their rows (not a `never_persist` one's: the
    /// engine keeps none) stay held until their message lands.
    fn stick(&mut self) {
        let from = self.history_end.min(self.items.len());
        for it in &self.items[from..] {
            if self.pending.contains(&it.id) && !never_persist(it) && !self.stuck.contains(&it.id) {
                self.stuck.push(it.id);
            }
        }
    }

    /// Stop holding row `id` (delivered).
    fn release(&mut self, id: u64) {
        self.pending.retain(|&p| p != id);
        self.stuck.retain(|&p| p != id);
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

    /// Take every row, split into notes (kept) and reusable ids; the rows
    /// themselves are kept in `self.before` for [`Transcript::keep_revs`].
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
                self.before.push(it);
            }
        }
        (notes, reuse)
    }

    /// Rows rebuilt from history (an attach, a history append or replace)
    /// that came out exactly as they were keep their old `rev`: a UI's
    /// strings and measured heights for `(id, rev)` stay valid, so a
    /// re-attach redraws only what changed. A row that differs keeps the
    /// newer `rev`, which is past every one it had.
    ///
    /// A row's time is the first one it had ([`Item::at`]), as the TUI's
    /// live rows: the daemon dates a message when its round ends, which can
    /// be minutes after its row started streaming, and a time that moved
    /// would move the row's day too. The history's time fills only a row
    /// that had none. Unless `carry` is false (the conversation was
    /// swapped: its ids are reused for other messages), and only from the
    /// row's own form.
    fn keep_revs(&mut self, carry: bool) {
        if self.before.is_empty() {
            return;
        }
        let before: HashMap<u64, Item> = std::mem::take(&mut self.before).into_iter().map(|i| (i.id, i)).collect();
        for it in self.items.iter_mut().filter(|i| i.anchor.is_none()) {
            let Some(o) = before.get(&it.id) else { continue };
            // An event row refolded from its form keeps what only its frame
            // had (sender, ASK, time; a `local_only` text): the same event.
            if carry
                && o.kind == ItemKind::Event
                && it.kind == ItemKind::Event
                && o.event.as_ref().is_some_and(|e| e.timestamp.is_some())
                && it.event.as_ref().is_some_and(|e| e.timestamp.is_none())
                && o.form.is_some()
                && o.form == it.form
            {
                it.event.clone_from(&o.event);
                it.text.clone_from(&o.text);
            }
            let same = match (&o.tool, &it.tool) {
                (Some(a), Some(b)) => a.tool_id == b.tool_id,
                (None, None) => o.text == it.text,
                _ => false,
            };
            if carry && o.at.is_some() && o.kind == it.kind && same {
                it.at = o.at.clone();
            }
            if o.kind == it.kind && o.streaming == it.streaming && o.text == it.text && o.tool == it.tool && o.event == it.event {
                it.rev = o.rev;
            }
        }
    }

    /// Replace everything with an attach snapshot (`Attached`), then replay
    /// the in-flight turn's events. A re-attach to the same session (after a
    /// reload) keeps row ids and notes; another session starts fresh.
    pub fn load_attached(&mut self, a: &Attached) -> Result<(), TranscriptError> {
        if a.conversation.api_messages.is_empty() && a.conversation.messages_len > 0 {
            return Err(TranscriptError::DigestHistory);
        }
        let same = self.session_id.as_deref() == Some(a.meta.id.as_str());
        // A clear, resume or compaction made while detached (in the TUI,
        // another window) keeps the session's id and changes its journal.
        // The journal is the header's: the attach's `meta.journal_id` is
        // the one the session started with (0.11.1 sends its create-time
        // meta), and only an older daemon's header has no id.
        let journal = a.conversation.header.get("id").and_then(Value::as_str).unwrap_or(&a.meta.journal_id).to_string();
        let carry = same && self.journal_id == journal;
        let (notes, mut reuse) = if same { self.take_rows(0) } else { (Vec::new(), Reuse::new()) };
        let (next_id, clean_len, before) = (self.next_id, self.clean_len, std::mem::take(&mut self.before));
        let no_subagent_notices = self.no_subagent_notices;
        let pending = if same { std::mem::take(&mut self.pending) } else { Vec::new() };
        let stuck = if same { std::mem::take(&mut self.stuck) } else { Vec::new() };
        *self = Transcript {
            next_id,
            clean_len,
            before,
            no_subagent_notices,
            pending,
            stuck,
            engine_pending: a.conversation.pending_events_len,
            session_id: Some(a.meta.id.clone()),
            journal_id: journal,
            ..Transcript::default()
        };
        self.dirty_from = Some(0);
        self.title = a.meta.title.clone().or_else(|| a.conversation.header.get("title").and_then(Value::as_str).map(str::to_string));
        self.cost_usd = a.conversation.cost;
        self.pending_prompts = a.pending_prompts.clone();
        self.fold_with_notes(&a.conversation.api_messages, notes, &mut reuse);
        self.busy = a.streaming;
        self.replaying = true;
        for env in &a.replay {
            if let Ok(ev) = env.decode() {
                self.apply_at(&ev, &env.ts);
            }
        }
        self.replaying = false;
        if !a.streaming {
            // No turn runs: a row still held is the engine's to flush at
            // the next turn's end, if it holds any (a turn failed while
            // detached), else delivered (and a `never_persist` one placed).
            self.busy = false;
            if self.engine_pending > 0 {
                self.stick();
            } else {
                self.stuck.clear();
            }
            self.deliver_all();
            self.settle();
        }
        self.keep_revs(carry);
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
        let notes = self.hold_events(notes, messages);
        for m in messages {
            self.fold_message(m, &mut reuse);
        }
        self.history_len += messages.len();
        let (pending, mut notes): (Vec<Item>, Vec<Item>) = notes.into_iter().partition(|n| self.pending.contains(&n.id));
        notes.append(&mut self.held);
        for n in notes {
            self.push_note(n, self.history_len);
        }
        self.history_end = self.items.len();
        self.push_pending(pending);
        self.response_start = None;
        if !self.busy {
            self.settle();
        }
        // An append matched this conversation's prefix: the rows it took
        // are its own live ones, whatever swap came before it (a resume of
        // the same journal has no replacement to consume the flag).
        self.keep_revs(true);
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
        let swapped = std::mem::take(&mut self.swapped);
        if swapped {
            notes.retain(|n| n.anchor == Some(usize::MAX));
        }
        self.fold_with_notes(messages, notes, &mut reuse);
        self.response_start = None;
        if !self.busy {
            self.settle();
        }
        self.keep_revs(!swapped);
    }

    /// Fold a whole history from empty, interleaving `notes` by anchor.
    /// Events not delivered yet go last, whatever their anchors.
    fn fold_with_notes(&mut self, messages: &[Value], notes: Vec<Item>, reuse: &mut Reuse) {
        let notes = self.hold_events(notes, messages);
        let (pending, mut notes): (Vec<Item>, Vec<Item>) = notes.into_iter().partition(|n| self.pending.contains(&n.id));
        notes.sort_by_key(|n| n.anchor);
        let mut notes = notes.into_iter().peekable();
        for (i, m) in messages.iter().enumerate() {
            while let Some(n) = notes.next_if(|n| n.anchor.unwrap_or(0) <= i) {
                self.push_note(n, i);
            }
            self.fold_message(m, reuse);
        }
        for n in notes.chain(std::mem::take(&mut self.held)) {
            self.push_note(n, messages.len());
        }
        self.history_len = messages.len();
        self.history_end = self.items.len();
        self.push_pending(pending);
    }

    /// The rows of events not delivered yet, back after a fold: last, and
    /// after the history's end, so the next fold takes them again (for their
    /// message, or to put them last again).
    fn push_pending(&mut self, pending: Vec<Item>) {
        for n in pending {
            self.push_note(n, self.history_len);
        }
    }

    /// The live event rows among `notes` whose message is in `messages`,
    /// held for it ([`Transcript::held`]; delivered); the other notes back.
    fn hold_events(&mut self, notes: Vec<Item>, messages: &[Value]) -> Vec<Item> {
        if !notes.iter().any(|n| n.kind == ItemKind::Event) {
            return notes;
        }
        let mut forms: Vec<ExternalEvent> =
            messages.iter().flat_map(event_forms).filter_map(ExternalEvent::from_payload).filter(|e| !e.is_worker_note()).collect();
        // Every note's own form first; then, for those left, its id (a form
        // that drifted), never for a `never_persist` one (it has no message).
        // A note held takes its message's form: the fold finds it by that.
        let mut left = Vec::new();
        for mut n in notes {
            match forms.iter().position(|f| n.kind == ItemKind::Event && n.form.as_deref() == Some(f)) {
                Some(k) => {
                    n.form = Some(Arc::new(forms.remove(k)));
                    self.release(n.id);
                    self.held.push(n);
                }
                None => left.push(n),
            }
        }
        let mut rest = Vec::new();
        for mut n in left {
            let id = n.event.as_ref().filter(|_| n.kind == ItemKind::Event && !never_persist(&n)).map(|e| e.id.clone());
            match id.and_then(|id| forms.iter().position(|f| f.id == id)) {
                Some(k) => {
                    n.form = Some(Arc::new(forms.remove(k)));
                    self.release(n.id);
                    self.held.push(n);
                }
                None => rest.push(n),
            }
        }
        rest
    }

    /// A history message that is an event's agent form (`<event …>`): its
    /// row, the live one if it was seen ([`Transcript::held`]: same id, its
    /// live time and the fields only the live frame has), else a new one. A
    /// worker's note, or a form that does not parse, is a notice of the
    /// raw text, as before.
    fn fold_event(&mut self, s: &str, reuse: &mut Reuse) {
        let Some(ev) = ExternalEvent::from_payload(s).filter(|e| !e.is_worker_note()) else {
            self.push(ItemKind::Notice, s.to_string(), None, false, reuse);
            return;
        };
        let ev = Arc::new(ev);
        // `hold_events` gave each row it held its message's form.
        if let Some(k) = self.held.iter().position(|h| h.form.as_ref() == Some(&ev)) {
            let mut it = self.held.remove(k);
            it.anchor = None;
            it.form = Some(ev);
            let ix = self.items.len();
            self.items.push(it);
            self.mark(ix);
            return;
        }
        let ix = self.push(ItemKind::Event, ev.content.text.clone(), None, false, reuse);
        self.items[ix].event = Some(ev.clone());
        self.items[ix].form = Some(ev);
    }

    /// The `External` frame: its row, last (above it, every row streamed
    /// before; under it, none until the engine delivers it), a note until
    /// its message lands. A `never_persist` event, which the engine never
    /// delivers, is held so for the turn it came in (it stays where it was
    /// when the turn ends); idle, it goes where any live row goes. Not again for the same
    /// event ([`same_event`]: a replay, a frame after its history row),
    /// whose row then takes what only the frame has; another event with
    /// the same id is another row.
    fn live_event(&mut self, ev: &ExternalEvent) {
        if ev.is_worker_note() {
            return;
        }
        let shown = ev.shown();
        let form = ev.history();
        let hold = self.busy || ev.content.disclosure.as_deref() != Some("never_persist");
        if let Some(ix) = self.items.iter().rposition(|i| same_event(i, &shown, &form, self.replaying)) {
            if self.items[ix].event.as_ref().is_some_and(|e| e.timestamp.is_none()) && shown.timestamp.is_some() {
                self.items[ix].text.clone_from(&shown.content.text);
                self.items[ix].event = Some(Arc::new(shown));
                self.touch(ix);
            }
            // A replay (an attach's) of one not delivered: last again, until
            // its delivery replays too.
            let id = self.items[ix].id;
            if hold && self.items[ix].is_note() && !self.pending.contains(&id) && ix >= self.history_end {
                let it = self.items.remove(ix);
                for t in self.tools.values_mut().filter(|t| **t > ix) {
                    *t -= 1;
                }
                if let Some(s) = self.response_start.as_mut().filter(|s| **s > ix) {
                    *s -= 1;
                }
                self.items.push(it);
                self.mark(ix);
                self.pending.push(id);
            }
            return;
        }
        let at = if hold { self.items.len() } else { self.live_end() };
        let ix = self.push_at(at, ItemKind::Event, shown.content.text.clone(), None, false, &mut Reuse::new());
        let it = &mut self.items[ix];
        it.event = Some(Arc::new(shown));
        it.form = Some(Arc::new(form));
        it.anchor = Some(self.history_len);
        if hold {
            self.pending.push(it.id);
        }
    }

    /// `steering_delivered` with an event's form: the engine put it in the
    /// conversation, between two rounds, where its message will be. Its row
    /// (the frame's) goes first among those not delivered yet, and live rows
    /// go under it. Without a frame (attached after it), a row from the
    /// form, there.
    fn delivered(&mut self, h: ExternalEvent) {
        if h.is_worker_note() {
            return;
        }
        let run = self.live_end();
        let rows = &self.items[run..];
        let k = rows
            .iter()
            .position(|i| i.form.as_deref() == Some(&h))
            .or_else(|| rows.iter().position(|i| i.event.as_ref().is_some_and(|e| e.id == h.id)));
        if let Some(k) = k {
            let it = self.items.remove(run + k);
            self.release(it.id);
            self.items.insert(run, it);
            self.mark(run);
            return;
        }
        if self.items.iter().any(|i| i.form.as_deref() == Some(&h)) {
            return;
        }
        let ix = self.push_new(ItemKind::Event, h.content.text.clone(), None, false);
        let h = Arc::new(h);
        self.items[ix].event = Some(h.clone());
        self.items[ix].form = Some(h);
        self.items[ix].anchor = Some(self.history_len);
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
        // Its rows have its time or none: never the event's it came in.
        let outer = (std::mem::replace(&mut self.stamp, message_at(m).map(Arc::from)), std::mem::take(&mut self.event_ts));
        self.fold_blocks(m, reuse);
        (self.stamp, self.event_ts) = outer;
    }

    fn fold_blocks(&mut self, m: &Value, reuse: &mut Reuse) {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        let content = m.get("content").unwrap_or(&Value::Null);
        match (role, content) {
            ("user", Value::String(s)) if is_event_form(role, s) => self.fold_event(s, reuse),
            ("user", Value::String(s)) => {
                let kind = if s.starts_with("<context-summary>") { ItemKind::Notice } else { ItemKind::User };
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
                        // An event's form in a user message's blocks: its row.
                        "text" if b.get("text").and_then(Value::as_str).is_some_and(|t| is_event_form(role, t)) => {
                            flush(self, &mut text, reuse);
                            self.fold_event(b.get("text").and_then(Value::as_str).unwrap_or(""), reuse);
                        }
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
                                tail: String::new(),
                                status: Some(ToolStatus::Running),
                            };
                            self.push(ItemKind::Tool, String::new(), Some(tool), false, reuse);
                        }
                        "tool_result" => {
                            flush(self, &mut text, reuse);
                            let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                            let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            let result = result_text(b.get("content").unwrap_or(&Value::Null));
                            if let Some(&ix) = self.tools.get(id) {
                                if let Some(t) = self.items[ix].tool.as_mut() {
                                    t.status = Some(tool_outcome(&result, is_error).status());
                                    t.result = preview(&result);
                                    t.tail = tail(&result);
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
        let ix = self.live_end().checked_sub(1)?;
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
        if let Some(ix) = self.live_end().checked_sub(1)
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

    /// Slash command `name`'s answer, as the TUI prints it into its
    /// scrollback: a client-side row at the end ([`ItemKind::Output`]), its
    /// command on the first line, then `text`. Never sent, not in the
    /// history; kept across history refolds as a notice is.
    pub fn output(&mut self, name: &str, text: &str) {
        self.notice(ItemKind::Output, format!("/{name}\n{}", text.trim_end()));
    }

    /// A client-side row: kept across history refolds.
    fn notice(&mut self, kind: ItemKind, text: impl Into<String>) {
        self.end_streaming_text();
        let ix = self.push_new(kind, text.into(), None, false);
        self.items[ix].anchor = Some(self.history_len);
    }

    /// One entry of a worker's watch journal (`Client::watch_subagent`), on
    /// a transcript of the worker's own: it replays through the same path
    /// as [`Transcript::apply`]'s stream events, so a worker's rows are the
    /// main conversation's rows. A steer it took in is a user row (unless
    /// it is a drained event's payload, `<event …>`: its parent's
    /// business); a failure is an error row with its category.
    pub fn apply_watch(&mut self, ev: &WatchEvent) {
        self.apply_watch_at(ev, "");
    }

    /// [`Transcript::apply_watch`] of an entry recorded at `at` (RFC 3339;
    /// empty: unknown): the rows it starts carry it ([`Item::at`]).
    pub fn apply_watch_at(&mut self, ev: &WatchEvent, at: &str) {
        self.event_ts.push_str(at);
        self.watch_event(ev);
        self.event_ts.clear();
        self.stamp = None;
    }

    fn watch_event(&mut self, ev: &WatchEvent) {
        match ev {
            WatchEvent::Steered { text } if !ExternalEvent::is_payload(text.trim_start()) => {
                self.end_streaming_text();
                self.push_new(ItemKind::User, text.clone(), None, false);
            }
            // A drained event's payload: its parent's business.
            WatchEvent::Steered { .. } => {}
            WatchEvent::Failed { category } => self.notice(ItemKind::Error, format!("subagent failed [{category}]")),
            other => {
                if let Some(s) = other.to_stream() {
                    self.apply_stream(&s);
                }
            }
        }
    }

    /// A transcript fed by hand (a worker's): whether its run is still
    /// going. Settled when it stops (no row streams, an open tool ends
    /// `Cancelled`), as at a turn's end.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if !busy {
            self.settle();
        }
    }

    /// Drop the oldest rows of a transcript that has no history folded (a
    /// worker's, fed by [`Transcript::apply_watch`]) until at most
    /// `max_items` rows and about `max_bytes` of text, tool inputs and
    /// results are left (the newest row always stays). Returns how many
    /// went; the next splice starts at 0. A transcript with history is
    /// never trimmed (0).
    pub fn trim_live_front(&mut self, max_items: usize, max_bytes: usize) -> usize {
        if self.history_len != 0 || self.history_end != 0 {
            return 0;
        }
        let size =
            |i: &Item| 64 + i.text.len() + i.tool.as_ref().map_or(0, |t| t.tool_id.len() + t.name.len() + t.input.len() + t.result.len());
        let mut bytes: usize = self.items.iter().map(size).sum();
        let mut n = 0;
        while n + 1 < self.items.len() && (self.items.len() - n > max_items || bytes > max_bytes) {
            bytes -= size(&self.items[n]);
            n += 1;
        }
        if n == 0 {
            return 0;
        }
        self.items.drain(..n);
        self.tools.retain(|_, ix| *ix >= n);
        for ix in self.tools.values_mut() {
            *ix -= n;
        }
        self.response_start = self.response_start.map(|s| s.saturating_sub(n));
        self.mark(0);
        n
    }

    /// Apply one live session event.
    pub fn apply(&mut self, ev: &SessionEvent) {
        self.apply_at(ev, "");
    }

    /// [`Transcript::apply`] of an event sent at `ts` (its envelope's, RFC
    /// 3339; empty: unknown): the rows it starts carry it ([`Item::at`]).
    pub fn apply_at(&mut self, ev: &SessionEvent, ts: &str) {
        self.event_ts.push_str(ts);
        self.apply_event(ev);
        self.event_ts.clear();
        self.stamp = None;
    }

    fn apply_event(&mut self, ev: &SessionEvent) {
        match ev {
            SessionEvent::TurnStarted { user_text, .. } => {
                self.busy = true;
                self.deliver_all();
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
                self.engine_pending = digest.pending_events_len;
                if self.engine_pending == 0 {
                    // It holds none: none of ours is stuck (flushed, or
                    // dropped), so the next delivery delivers them.
                    self.stuck.clear();
                }
            }
            SessionEvent::Idle => {
                // Idle with events the engine still holds: the turn ended
                // with no end (its stream closed), or failed as an error
                // frame said.
                if self.engine_pending > 0 {
                    self.stick();
                }
                self.busy = false;
                self.deliver_all();
                self.settle();
            }
            SessionEvent::Prompt { request } => self.pending_prompts.push(request.clone()),
            SessionEvent::PromptResolved { prompt_id } => {
                self.pending_prompts.retain(|p| p.id != *prompt_id);
                if self.resolved.len() == 16 {
                    self.resolved.pop_front();
                }
                self.resolved.push_back((*prompt_id, self.busy));
            }
            SessionEvent::External { event } => self.live_event(event),
            // The input owner's own steer, whatever it looks like: events
            // come as `External` and `steering_delivered`, never so.
            SessionEvent::Steered { text, .. } => self.notice(ItemKind::Notice, format!("steered: {text}")),
            SessionEvent::SystemNotice { text } => self.notice(ItemKind::Notice, text.clone()),
            SessionEvent::Aborted { .. } => {
                self.busy = false;
                self.deliver_all();
                self.settle();
                self.notice(ItemKind::Notice, "turn aborted");
            }
            SessionEvent::Ended { reason } => self.notice(ItemKind::Notice, format!("session ended ({reason})")),
            SessionEvent::Cleared { session_id } => {
                self.drop_held();
                self.journal_id.clone_from(session_id);
                // Clearing an empty conversation changes nothing: its digest
                // matches, no replacement follows to consume the flag, and
                // a later unrelated one would drop this conversation's notes.
                self.swapped |= self.history_len > 0;
                self.notice(ItemKind::Notice, "conversation cleared");
            }
            SessionEvent::Resumed { new_id, clamp_notice, .. } => {
                self.drop_held();
                self.swapped = true;
                self.journal_id.clone_from(new_id);
                self.notice(ItemKind::Notice, format!("resumed {new_id}"));
                if let Some(n) = clamp_notice {
                    self.notice(ItemKind::Notice, n.clone());
                }
            }
            SessionEvent::CompactionApplied { session_id, msg_count, .. } => {
                self.swapped = true;
                self.journal_id.clone_from(session_id);
                self.notice(ItemKind::Notice, format!("conversation compacted ({msg_count} messages)"));
            }
            SessionEvent::CompactionFailed { message } => {
                self.stick();
                self.notice(ItemKind::Error, format!("compaction failed: {message}"))
            }
            SessionEvent::CompactionCancelled => self.stick(),
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
                    self.response_start = Some(self.live_end());
                }
                // The response's rows go; an event's that came meanwhile stays.
                LlmEvent::ResponseReset => {
                    if let Some(s) = self.response_start.filter(|&s| s >= self.history_end && s <= self.items.len()) {
                        let kept: Vec<Item> = self.items.drain(s..).filter(|i| i.kind == ItemKind::Event && i.is_note()).collect();
                        self.items.extend(kept);
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
                            t.tail = tail(result);
                            t.status = Some(tool_outcome(result, false).status());
                        }
                        self.items[ix].streaming = false;
                        self.touch(ix);
                    }
                }
                LlmEvent::Unknown => {}
            },
            StreamEvent::Session(s) => match s {
                SessionStreamEvent::MessageHistory { messages } => self.apply_history(messages),
                SessionStreamEvent::Error { message } => {
                    self.stick();
                    self.notice(ItemKind::Error, message.clone())
                }
                SessionStreamEvent::Notice { text, .. } => self.notice(ItemKind::Notice, text.clone()),
                SessionStreamEvent::Done => self.end_streaming_text(),
                _ => {}
            },
            StreamEvent::Agent(a) => match a {
                AgentEvent::SteeringDelivered { message } => {
                    if let Some(ev) = steered_event(message) {
                        self.delivered(ev);
                    }
                }
                AgentEvent::SubagentStart { agent_name, task_preview, .. } if !self.no_subagent_notices => {
                    self.notice(ItemKind::Notice, format!("subagent {agent_name} started: {task_preview}"))
                }
                AgentEvent::SubagentDone { agent_name, duration_secs, .. } if !self.no_subagent_notices => {
                    self.notice(ItemKind::Notice, format!("subagent {agent_name} done ({duration_secs:.1}s)"))
                }
                _ => {}
            },
            StreamEvent::Unknown => {}
        }
    }
}

/// The event a steer carries (its agent form), if it is one.
fn steered_event(text: &str) -> Option<ExternalEvent> {
    ExternalEvent::is_payload(text).then(|| ExternalEvent::from_payload(text)).flatten()
}

/// Whether a history message's text (its content, or a text block of it)
/// from `role` is an event's agent form: the one test the fold and
/// [`Transcript::hold_events`] share. One that does not parse is a notice.
fn is_event_form(role: &str, text: &str) -> bool {
    role == "user" && ExternalEvent::is_payload(text)
}

/// The event forms in a history message ([`is_event_form`]).
fn event_forms(m: &Value) -> Vec<&str> {
    let role = m.get("role").and_then(Value::as_str).unwrap_or("");
    match m.get("content") {
        Some(Value::String(s)) if is_event_form(role, s) => vec![s.as_str()],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .filter(|t| is_event_form(role, t))
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether row `it` is the event `shown` (its history form `form`): the
/// same id and, if the row has a time (a frame's), the same time (a
/// replay); if not (from the history), the same form, in an attach's
/// replay only: else the engine sends a frame before the history that
/// holds it, and a frame after a row with its form is another event (a
/// watcher's re-alert, same id and words).
fn same_event(it: &Item, shown: &ExternalEvent, form: &ExternalEvent, replaying: bool) -> bool {
    let Some(e) = it.event.as_deref() else { return false };
    e.id == shown.id
        && match &e.timestamp {
            Some(t) => shown.timestamp.as_ref() == Some(t),
            None => replaying && it.form.as_deref() == Some(form),
        }
}

/// A `never_persist` event's row: the engine shows it and keeps it nowhere.
fn never_persist(it: &Item) -> bool {
    it.event.as_ref().is_some_and(|e| e.content.disclosure.as_deref() == Some("never_persist"))
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
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
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

/// The end of a result [`preview`] cuts: its last whole lines in at most
/// `TAIL_MAX` bytes (the line the window starts in is left out).
fn tail(s: &str) -> String {
    if s.len() <= PREVIEW_MAX {
        return String::new();
    }
    let mut start = s.len().saturating_sub(TAIL_MAX).max(PREVIEW_MAX);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    let window = &s[start..];
    match (s[..start].ends_with('\n'), window.find('\n')) {
        (true, _) => window.to_string(),
        (false, Some(at)) => window[at + 1..].to_string(),
        (false, None) => String::new(),
    }
}

#[cfg(test)]
mod outcome_tests {
    use serde_json::json;

    use super::*;

    /// A cut result keeps its last whole lines; one the preview holds, none.
    #[test]
    fn a_cut_result_keeps_its_end() {
        assert_eq!(tail("1\ta\n2\tb\n"), "");
        let read: String = (1..=600).map(|n| format!("{n}\tline {n}\n")).collect::<String>() + "\n... (64 more lines)";
        let t = tail(&read);
        assert!(t.len() <= TAIL_MAX && t.ends_with("600\tline 600\n\n... (64 more lines)"), "{t}");
        assert!(t.split('\n').next().is_some_and(|l| l.split_once('\t').is_some_and(|(n, l)| l == format!("line {n}"))), "{t}");
        assert_eq!(tail(&"x".repeat(9000)), "", "no whole line: nothing");
    }

    /// The engine's own result texts (agent-engine origin/dev): none of them
    /// sets `is_error`.
    #[test]
    fn results_are_classified_by_the_daemons_texts() {
        use ToolOutcome::*;
        let cases = [
            // `runtime/stream.rs` Block after `runtime/mod.rs` denied a Confirm.
            ("Tool call blocked by extension: Tool call confirmation denied: guard wants to check this edit", Denied),
            ("Tool call blocked by extension: Tool call requires confirmation but no interactive prompt is available: x", Denied),
            // `tools/activation.rs` GateDenial.
            ("Tool call denied: tool is not activated for this session: shell_start", Denied),
            ("Unknown tool: shell_strat", Failed),
            // `tools/discovery.rs` `activation_denied`: the prompt was denied
            // (the live sandbox's text), or the model guessed an id.
            (
                "Tool execution failed: tool activation denied: tool activation requires host confirmation authority; model-initiated activation was not confirmed",
                Denied,
            ),
            (
                "Tool execution failed: tool activation denied: unknown tool id 'x:y'; nothing was activated. activate_tools needs exact ids (namespace:name) as search_tools returns them: search for the tool instead of guessing its id.",
                Failed,
            ),
            // `activate_model_initiated` errors after AUTHORIZE
            // (`BulkActivationError`, `GrantIssuanceError`): not refusals.
            ("Tool execution failed: tool activation denied: duplicate tool id in one activation batch: builtin:shell_start", Failed),
            (
                "Tool execution failed: tool activation denied: activation batch rejected: tool is already activated for this session: builtin:shell_start",
                Failed,
            ),
            (
                "Tool execution failed: tool activation denied: activation batch rejected: tool is already in this session's configured core set: builtin:bash",
                Failed,
            ),
            (
                "Tool execution failed: tool activation denied: activation batch rejected: activation grant schema digest does not match the catalog record for builtin:shell_start",
                Failed,
            ),
            (
                "Tool execution failed: tool activation denied: cannot issue activation grant: source provenance is unverified: ext:x",
                Failed,
            ),
            // `RuntimeError::Tool` from `tools/edit.rs`.
            (
                "Tool execution failed: old_string not found in '/w/a.rs'. Make sure it matches exactly, including whitespace and indentation.",
                Failed,
            ),
            ("Tool execution failed: Failed to read file '/w/gone.rs': No such file or directory (os error 2)", Failed),
            // `runtime/budget.rs`.
            ("Tool call not executed: turn tool-call budget exhausted", Failed),
            // `CancelCause::phrase()` and `canceled_tool_result_content`.
            ("Canceled by user", Cancelled),
            ("Canceled (session cost cap reached)", Cancelled),
            ("Canceled by user. It had already started and may have partially applied its effects.", Cancelled),
            ("line 1\nline 2\n\n[Canceled by user before the tool finished; the output above is partial.]", Cancelled),
            ("Operation canceled", Cancelled),
            // `tools/bash.rs` `output_limit_notice` (its own test's text): the
            // command was killed at the limit, yet the result is a success.
            (
                "xxxx\n\n[output truncated at 256 bytes: the command was stopped at this limit before it finished, so its exit status is unknown. Redirect large output to a file (e.g. cmd > out.log 2>&1) and read only the parts you need.]",
                Unknown,
            ),
            // The same in a virtualised history result (the default above
            // 12 KiB: `runtime/tool_spill.rs` `assemble`), as the desktop
            // and the files ledger read it.
            (
                "y\ny\n[... lines 3-37445 omitted ...]\ny\ny\n\n[output truncated at 262144 bytes: the command was stopped at this limit before it finished, so its exit status is unknown. Redirect large output to a file (e.g. cmd > out.log 2>&1) and read only the parts you need.]\n\n[Output virtualised: 262356 bytes, 37449 lines; lines 3-37445 are omitted above. Full output: /home/u/.synaps-cli/spill/s1/toolu_1.txt (read it with offset=2 limit=37443, or grep it).]",
                Unknown,
            ),
            (
                "y\n[... lines 2-9000 omitted ...]\ny\n\n[Output virtualised: 18000 bytes, 9001 lines; lines 2-9000 are omitted above. Full output: /s/t.txt (read it with offset=1 limit=8999, or grep it).]",
                Ok,
            ),
            // v0.11.1's notice (the daemon desktopproto runs): killed too.
            ("overflow\no\n\n[output truncated at 262144]", Unknown),
            (
                "overflow\n[... lines 456-28790 omitted ...]\no\n\n[output truncated at 262144]\n\n[Output virtualised: 262174 bytes, 29130 lines; lines 456-28790 are omitted above. Full output: /home/u/.synaps-cli/spill/s/toolu_01.txt (read it with offset=455 limit=28335, or grep it).]",
                Unknown,
            ),
            // The notice quoted mid-output is not it, nor is the history
            // cut (`max_tool_output`), which says nothing about the command.
            ("[output truncated at 256]\nthen it went on", Ok),
            ("xxxx\n\n[truncated — 262174 total bytes, showing first 30000]", Ok),
            // Successes, including ones that mention the phrases.
            ("Edited /w/a.rs", Ok),
            ("Successfully wrote 3 lines", Ok),
            ("grep: Tool execution failed appears in this log", Ok),
            ("Canceled by users of the old API", Ok),
            ("", Ok),
        ];
        for (text, want) in cases {
            assert_eq!(tool_outcome(text, false), want, "{text:?}");
        }
        assert_eq!(tool_outcome("anything", true), Failed);
        assert_eq!(tool_outcome("Canceled by user", true), Cancelled);
        let block = json!({"type": "tool_result", "tool_use_id": "t", "content": [{"type": "text", "text": "Tool call denied: x"}]});
        assert_eq!(tool_result_outcome(&block), Denied);
        assert_eq!(tool_result_outcome(&json!({"type": "tool_result", "content": "ok", "is_error": true})), Failed);
    }

    #[test]
    fn the_transcript_marks_rows_from_the_result_text() {
        let mut t = Transcript::new();
        let call = |id: &str| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "edit", "input": {}}]});
        let result =
            |id: &str, text: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]});
        t.apply_history(&[
            call("a"),
            result("a", "Edited /w/a.rs"),
            call("b"),
            result("b", "Tool call blocked by extension: Tool call confirmation denied: guard"),
            call("c"),
            result("c", "Tool execution failed: old_string not found in '/w/c.rs'."),
            call("d"),
            result("d", "Canceled by user"),
        ]);
        let st: Vec<Option<ToolStatus>> = t.items().iter().filter_map(|i| i.tool.as_ref().map(|t| t.status)).collect();
        assert_eq!(st, [Some(ToolStatus::Done), Some(ToolStatus::Denied), Some(ToolStatus::Failed), Some(ToolStatus::Cancelled)]);
        // The live result: the same classification.
        let ev = |v: Value| crate::protocol::decode_event(&v).unwrap();
        t.apply(&ev(
            json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "edit", "tool_id": "e", "input": {}}}),
        ));
        t.apply(&ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": "e",
            "result": "Tool call blocked by extension: Tool call confirmation denied: guard"}})));
        let last = t.items().iter().rev().find_map(|i| i.tool.as_ref()).unwrap();
        assert_eq!((last.tool_id.as_str(), last.status), ("e", Some(ToolStatus::Denied)));
    }

    /// Subagent notices are on by default; off, they stay off across an
    /// attach, and nothing else changes.
    #[test]
    fn subagent_notices_can_be_turned_off() {
        let ev = |v: Value| crate::protocol::decode_event(&v).unwrap();
        let start = ev(
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "subagent_start", "subagent_id": 1, "agent_name": "scout", "task_preview": "look"}}),
        );
        let done = ev(
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "subagent_done", "subagent_id": 1, "agent_name": "scout", "result_preview": "ok", "duration_secs": 2.0}}),
        );
        let mut on = Transcript::new();
        on.apply(&start);
        on.apply(&done);
        assert_eq!(
            on.items().iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
            ["subagent scout started: look", "subagent scout done (2.0s)"]
        );
        let mut off = Transcript::new();
        off.set_subagent_notices(false);
        let a = json!({"type": "attached", "client": 2, "meta": {"id": "s"}, "conversation": {"api_messages": []}, "input_owner": 2});
        let crate::protocol::DaemonFrame::Attached(a) = crate::protocol::decode_frame(&a.to_string()).unwrap() else { panic!() };
        off.load_attached(&a).unwrap();
        off.apply(&start);
        off.apply(&done);
        assert!(off.items().is_empty());
    }

    /// A prompt remembers whether a turn ran when it resolved: an answer
    /// lets the turn go on, a cancel resolves it first and ends it after.
    #[test]
    fn a_resolved_prompt_remembers_whether_a_turn_ran() {
        let mut t = Transcript::new();
        t.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: String::new(), user_text: None });
        t.apply(&SessionEvent::PromptResolved { prompt_id: 3 });
        t.apply(&SessionEvent::Idle);
        t.apply(&SessionEvent::PromptResolved { prompt_id: 4 });
        assert_eq!((t.resolved_busy(3), t.resolved_busy(4), t.resolved_busy(5)), (Some(true), Some(false), None));
    }
}
