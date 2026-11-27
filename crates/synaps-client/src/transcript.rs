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

use std::collections::{HashMap, HashSet, VecDeque};
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

/// What a gated call's result says, in the engine's own words, about its
/// gate (synaps-desk #25): its whole result checked against the forms the
/// daemon writes, never a phrase searched for in what a tool printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateNote {
    /// Held at the gate: `Tool call blocked by extension: Tool call
    /// confirmation denied: …` (agent-engine `runtime/mod.rs`, wrapped by
    /// `runtime/stream.rs`), or the activation gate's refusal.
    Denied,
    /// Cancelled with nothing after the phrase (`Canceled by user`): a call
    /// held at its gate gets that, and so does one that ran silently with
    /// no side effects; it says nothing either way.
    BareCancel,
    /// Cancelled after it started: the partial-output note (`…\n\n[Canceled
    /// by user before the tool finished; the output above is partial.…]`)
    /// or `Canceled by user. It had already started and may have partially
    /// applied its effects.` (`canceled_tool_result_content`).
    StartedCancel,
}

/// The side-effect warning `canceled_tool_result_content` appends.
const STARTED: &str = " It had already started and may have partially applied its effects.";
/// The deny `resolve_before_tool_call_result` returns, as the stream wraps it.
const CONFIRM_DENIED: &str = "Tool call blocked by extension: Tool call confirmation denied: ";

/// [`GateNote`] of a result (`tail`: its end when [`preview`] cut it, as
/// [`ToolCall::tail`]). `None`: none of the daemon's forms, tool output
/// included. A tool whose whole output is byte for byte one of these forms
/// reads as it; only the daemon saying what it applied can tell them apart.
pub fn gate_note(result: &str, tail: &str) -> Option<GateNote> {
    // The confirm's deny ends with its extension's reason (any text); the
    // activation refusal is one fixed text, matched whole (Shady #64 r2,
    // C64-2): a prefix of it, or it followed by more, is output.
    if result.starts_with(CONFIRM_DENIED) || result.strip_prefix(ACTIVATION_DENIED) == Some(ACTIVATION_REFUSED_WHOLE) {
        return Some(GateNote::Denied);
    }
    if CANCEL_PHRASES.contains(&result) {
        return Some(GateNote::BareCancel);
    }
    if CANCEL_PHRASES.iter().any(|p| result.strip_prefix(p).and_then(|r| r.strip_prefix('.')) == Some(STARTED)) {
        return Some(GateNote::StartedCancel);
    }
    let end = if tail.is_empty() { result } else { tail };
    let last = end.trim_end().rsplit('\n').next().unwrap_or("");
    let note = |p: &str| {
        last.strip_prefix('[')
            .and_then(|l| l.strip_prefix(p))
            .and_then(|l| l.strip_prefix(" before the tool finished; the output above is partial."))
            .is_some_and(|l| l == "]" || l.strip_suffix(']') == Some(STARTED))
    };
    CANCEL_PHRASES.iter().any(|p| note(p)).then_some(GateNote::StartedCancel)
}

/// `activation_denied` (agent-engine `tools/discovery.rs`) as a tool error:
/// every `HostActivationError`, refusals and batch failures alike.
const ACTIVATION_DENIED: &str = "Tool execution failed: tool activation denied:";
/// The one `HostActivationError` that is a refusal, `ConfirmationRequired`
/// (`tools/activation.rs`): the user denied the prompt, the policy is
/// `deny`, or nothing could show a prompt. Its whole text after
/// [`ACTIVATION_DENIED`] (wrapped by `activation_denied` and
/// `RuntimeError::Tool`, SynapsCLI `dev`), matched whole (Shady #64 r2,
/// C64-2): a prefix of it, or it followed by more, is a failure's text.
const ACTIVATION_REFUSED_WHOLE: &str =
    " tool activation requires host confirmation authority; model-initiated activation was not confirmed";

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
        return if why == ACTIVATION_REFUSED_WHOLE { ToolOutcome::Denied } else { ToolOutcome::Failed };
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
    /// Its place among the calls this transcript heard start
    /// (`ToolStarted`, 1 for the first; 0: none heard). The rail's NOW
    /// names the earliest-started call still running (synaps-cards
    /// `docs/blocks.md` §4), so parallel calls never trade the label.
    pub started: u64,
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
    /// The agent card it draws ([`crate::cards`]): a card call's, an
    /// answer's, a close's. Part of `rev`: a row whose card changes (an
    /// answer came, it closed) is changed.
    pub card: Option<Arc<crate::cards::CardRow>>,
    /// What it draws of the blocks' board ([`crate::blocks`]): a block
    /// call's fate, the blocks anchored at it, its run's title. Part of
    /// `rev`: a row whose blocks change is changed.
    pub blocks: Option<Arc<crate::blocks::RowBlocks>>,
    /// A card or block call's input, whole ([`crate::blocks::keeps_input`]):
    /// [`ToolCall::input`] is a preview, and either input may be up to
    /// 8 KiB (synaps-cards design §2.1, blocks.md §5.1): the live call's
    /// (`ToolUse`), the history call's from its message. Rebuilt as kept
    /// ([`crate::blocks::canonical`]), never changed after: what it holds
    /// is what [`Item::kept_bytes`] charges.
    kept_input: Option<Arc<Value>>,
    /// A live row's place in the stream ([`Transcript::came`]; 0: not
    /// yet): its call, once no longer streaming (a user row: its message),
    /// and its result. The cards fold the live turn in this order, not the
    /// rows' ([`crate::cards`]): results need not come in call order.
    seq: [u64; 2],
    /// A history block call's committed fate (a board published it once,
    /// [`crate::blocks::Drawn::committed`]): its row keeps it, so no board
    /// keeps or publishes it again.
    fate: Option<crate::blocks::Fate>,
}

impl Item {
    /// What its kept input holds (bytes, heap included: [`Item`]'s
    /// `kept_input`, a card or block call's input whole).
    pub fn kept_bytes(&self) -> usize {
        self.kept_input.as_deref().map_or(0, crate::blocks::kept_bytes)
    }

    /// A notice or error this client added (not part of the history).
    pub fn is_note(&self) -> bool {
        self.anchor.is_some()
    }

    /// A card or block call's whole input.
    pub(crate) fn kept_input(&self) -> Option<&Value> {
        self.kept_input.as_deref()
    }

    /// Its kept input's allocation (tests: who else keeps it).
    pub fn kept_allocation(&self) -> Option<&Arc<Value>> {
        self.kept_input.as_ref()
    }

    /// Its place in the live stream: its call's (or message's), its
    /// result's; 0 for not yet.
    pub(crate) fn seq(&self) -> [u64; 2] {
        self.seq
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

/// How many prompts' outcomes [`Transcript::resolved_outcome`] remembers:
/// far more than a window could hold back unrendered (a minimised window
/// resolves its card frames later), 16 bytes each.
pub const OUTCOMES_KEPT: usize = 1024;

/// How many calls [`Transcript::past_gate`] holds before it lets go of
/// those no longer in the transcript (it never lets go of one that is).
pub const PAST_GATE_KEPT: usize = 256;

#[derive(Clone, Debug, Default)]
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
    /// Compactions applied since it was made (the journal moved to its
    /// successor): D6b carries the predecessor's dismissals across.
    compactions: u64,
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
    /// What the daemon applied to the last [`OUTCOMES_KEPT`] prompts this
    /// attachment raised ([`Transcript::resolved_outcome`]), newest last.
    outcomes: VecDeque<(u64, crate::protocol::PromptOutcome)>,
    /// The calls that showed they got past their gate
    /// ([`Transcript::past_gate`]), and the size past which those no longer
    /// in the transcript are let go.
    past_gate: std::collections::HashSet<String>,
    past_gate_cap: usize,
    /// Rows are being rebuilt (an attach with its replay, a history append
    /// or replace): some calls are out of `items` for now, so
    /// [`Transcript::past_gate`] lets go of none until it is done.
    rebuilding: bool,
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
    /// The message folded last was a compaction summary
    /// ([`is_compaction_summary`]): the next, if it is the acknowledgement
    /// the daemon writes after one ([`COMPACTION_ACKS`]), is not drawn
    /// either.
    after_summary: bool,
    /// The agent cards' fold over the history folded so far
    /// ([`crate::cards`]).
    cards: crate::cards::HistoryCards,
    /// Something the cards read changed since [`Transcript::sync_cards`].
    cards_dirty: bool,
    /// Rows that drew a card at the last [`Transcript::sync_cards`].
    card_rows: usize,
    /// The live stream's order so far ([`Item::seq()`]).
    live_seq: u64,
    /// The blocks' input over the history folded so far ([`crate::blocks`]).
    blocks: crate::blocks::HistoryBlocks,
    /// The live turn's protocol facts since the history's last fold
    /// ([`crate::blocks::LiveLog`]): the board's live input is read from
    /// them, never from rows.
    live_log: crate::blocks::LiveLog,
    /// The blocks' input's revision ([`Transcript::blocks_rev`]).
    blocks_rev: u64,
    /// The board the rows draw, once one is published ([`Transcript::set_drawn`]).
    drawn: Option<Arc<crate::blocks::Drawn>>,
    /// The epoch whose committed calls' fates the rows keep ([`Item::fate`]).
    fate_epoch: u64,
    /// Rows were taken to be folded again since the last sync: they draw
    /// the last board again ([`Transcript::sync_blocks`]).
    rows_moved: bool,
    /// The history's epoch and length, and the live log's version, at the
    /// last sync.
    blocks_seen: (u64, usize, (u64, u64)),
    /// A card call among the live rows: set as one starts, so a session
    /// without cards reads no row per event ([`Transcript::sync_cards`]).
    /// It may stay set once that row is gone: then the fold just runs.
    live_cards: bool,
    /// The history landed since [`Transcript::sync_cards`]: count
    /// [`Transcript::live_cards`] again.
    recount: bool,
    /// The cards the rows draw: the history's fold, then the live turn's
    /// on top ([`Transcript::cards`]).
    card_live: crate::cards::LiveCards,
    /// [`Transcript::card_live`] read every live row up to now but those
    /// in [`Transcript::card_touched`]: the next sync reads only those.
    /// Unset, what it kept let go ([`Transcript::card_unkeep`]), by what
    /// takes live rows away (a history's fold, a reset response, a trim)
    /// and by a detach: the next sync reads them all.
    card_kept: bool,
    /// The calls and user rows changed since the last sync, by id: where
    /// they were then.
    card_touched: HashMap<u64, usize>,
    /// Rows the card sync read, in all: its tests count them.
    card_visits: u64,
    /// The fold's revision ([`Transcript::cards_rev`]).
    card_rev: u64,
    /// The times of the rows that ask a card, hashed (0: none read):
    /// [`Transcript::sync_card_times`].
    card_times: u64,
    /// The length and hash of the history folded ([`Transcript::history_settled`]).
    mirror: crate::history::HistoryMirror,
    /// The latest `Conversation` digest since the attach (`None`: the
    /// attach's snapshot is the latest word).
    digest: Option<crate::protocol::ConversationDigest>,
    /// The steer the engine holds, by the latest word (the snapshot's or
    /// a digest's `queued_message`).
    queued: Option<String>,
    /// The calls heard start (`ToolStarted`): [`ToolCall::started`]'s count.
    started: u64,
    /// A compaction this attachment heard start has not ended.
    compacting: bool,
    /// The conversation was swapped (a compaction applied, a resume): the
    /// client asks for the new history at once, and until it lands the
    /// history held says nothing of the new one.
    swap_pending: bool,
    /// The daemon dropped events for this attachment
    /// ([`crate::protocol::stream_lagged`]): the client asks for the
    /// history again at once, and until it lands what is held may be behind
    /// what was dropped ([`Transcript::resyncing`]).
    resyncing: bool,
    /// Since the daemon dropped events, no word on the engine's slot
    /// ([`Transcript::slot_known`]).
    slot_unknown: bool,
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
    /// The journal of the conversation shown (module docs: clear, resume
    /// and compaction move it).
    pub fn journal_id(&self) -> &str {
        &self.journal_id
    }

    /// Compactions applied since it was made.
    pub fn compactions(&self) -> u64 {
        self.compactions
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// The agent cards' fold, as the rows draw them: the history's, then
    /// the live turn's on top ([`crate::cards`]). What the card memo
    /// follows (synaps-cards §3.5); a click is checked on it too (no
    /// replacement or close heard since), and on the history's alone
    /// ([`Transcript::history_cards`]).
    pub fn cards(&self) -> &cards_spec::fold::Fold {
        self.card_live.fold()
    }

    /// The heap the card sync keeps to read what changed since it last read
    /// (beyond [`Transcript::cards`]), capacities counted: digests and
    /// places of the live events, not their inputs or text (the rows').
    /// A tab cache counts it.
    pub fn card_sync_bytes(&self) -> usize {
        self.card_live.bytes() + crate::cards::map_bytes(&self.card_touched)
    }

    /// Let go of what the card sync keeps to read what changed (a tab left
    /// for the cache: its next sync, if any, reads every row).
    pub fn shed_card_sync(&mut self) {
        self.card_unkeep();
    }

    /// Moves whenever the cards of [`Transcript::cards`] change, or the
    /// times of the rows that ask them ([`Item::at`], as the history
    /// settled them): what is built from the cards and their times is
    /// built again only when this moves. One counter for the process, so
    /// a revision this moved to is no other transcript's (two that never
    /// moved, empty or cloned, share theirs).
    pub fn cards_rev(&self) -> u64 {
        self.card_rev
    }

    /// The blocks' input's revision (blocks.md §1.7): it moves when the
    /// board's input does (the history's events, the live turn's as the
    /// board reads them), and only then, so a desk schedules the board's
    /// work again only when this moves. One counter for the process.
    pub fn blocks_rev(&self) -> u64 {
        self.blocks_rev
    }

    /// The board's input now ([`crate::blocks::Input`]): `None` with no
    /// block call in the history or the live turn.
    pub fn blocks_input(&self) -> Option<crate::blocks::Input> {
        crate::blocks::Input::of(&self.blocks, &self.live_log)
    }

    /// A block call in the history or the live turn: the board draws them.
    pub fn has_blocks(&self) -> bool {
        self.blocks.has_blocks() || self.live_log.has_blocks()
    }

    /// The history's epoch ([`crate::blocks::HistoryBlocks::epoch`]): a
    /// board folded from another epoch's input is another history's.
    pub fn blocks_epoch(&self) -> u64 {
        self.blocks.epoch()
    }

    /// The history's events read so far: within an epoch it only grows.
    pub fn blocks_len(&self) -> usize {
        self.blocks.len()
    }

    /// What the board's input and its output keep, heap included (bytes):
    /// the history's blocks with its worker's fold
    /// ([`crate::blocks::HistoryBlocks::bytes`]), the live turn's log
    /// ([`crate::blocks::LiveLog::bytes`]), and the board the rows draw
    /// ([`crate::blocks::Drawn::bytes`]), each owner once: a row's blocks
    /// are the board's, but for a row's own entry (a committed fate added
    /// to the board's: [`crate::blocks::RowBlocks::of`]).
    pub fn blocks_bytes(&self) -> usize {
        use crate::blocks::RowBlocks;
        let drawn = self.drawn.as_deref();
        let own = |(id, b): (u64, &Arc<RowBlocks>)| {
            let board = drawn.and_then(|d| d.rows.get(&id)).is_some_and(|r| Arc::ptr_eq(r, b));
            if board || RowBlocks::is_shared(b) { 0 } else { b.own_bytes() }
        };
        let rows: usize = self.items.iter().filter_map(|i| Some((i.id, i.blocks.as_ref()?))).map(own).sum();
        self.blocks.bytes() + self.live_log.bytes() + drawn.map_or(0, |d| d.bytes) + rows
    }

    /// The frame's work for the board's live input so far: the events its
    /// live log wrote and moved (tests: the events each live event adds,
    /// never the turn so far).
    #[doc(hidden)]
    pub fn blocks_work(&self) -> usize {
        self.live_log.work()
    }

    /// The live turn's log (tests: what it keeps).
    #[doc(hidden)]
    pub fn blocks_live(&self) -> &crate::blocks::LiveLog {
        &self.live_log
    }

    /// What the history's worker keeps ([`crate::blocks::Retained`]).
    pub fn blocks_retained(&self) -> crate::blocks::Retained {
        self.blocks.worker().lock().unwrap_or_else(std::sync::PoisonError::into_inner).retained()
    }

    /// The board the rows draw, the last one published.
    pub fn drawn(&self) -> Option<&Arc<crate::blocks::Drawn>> {
        self.drawn.as_ref()
    }

    /// A finished board for the rows to draw ([`Item::blocks`]; `None`: no
    /// board, as after a `/clear`): the rows it or the last one draws on,
    /// and those of calls it says committed, are drawn again, and every
    /// changed row is touched; the first board of a history draws every row
    /// again. Rows are matched by id, so a board folded from an
    /// input the rows have moved past since draws on the rows still there,
    /// until the next one lands.
    pub fn set_drawn(&mut self, drawn: Option<Arc<crate::blocks::Drawn>>) {
        let old = std::mem::replace(&mut self.drawn, drawn);
        let Some(d) = self.drawn.clone() else {
            self.fate_epoch = 0;
            for it in &mut self.items {
                it.fate = None;
            }
            self.redraw_blocks();
            return;
        };
        let committed: HashMap<u64, crate::blocks::Fate> = d.committed.iter().copied().collect();
        if d.epoch != self.fate_epoch {
            // Another history's first board: its committed calls are all.
            self.fate_epoch = d.epoch;
            for it in &mut self.items {
                it.fate = committed.get(&it.id).copied();
            }
            self.redraw_blocks();
        } else {
            let mut ids: HashSet<u64> = d.rows.keys().chain(committed.keys()).copied().collect();
            ids.extend(old.iter().flat_map(|o| o.rows.keys().copied()));
            let mut changed = Vec::new();
            for (ix, it) in self.items.iter_mut().enumerate().filter(|(_, it)| ids.contains(&it.id)) {
                if let Some(&f) = committed.get(&it.id) {
                    it.fate = Some(f);
                }
                // The board's own, even if the same as the one before: an
                // older board's copy is let go (one owner, counted once).
                let want = crate::blocks::RowBlocks::of(Some(&d), it.id, it.fate);
                if it.blocks != want {
                    changed.push(ix);
                }
                it.blocks = want;
            }
            for ix in changed {
                self.touch(ix);
            }
        }
        if d.epoch == self.blocks.epoch() {
            self.blocks.took(d.committed_to);
        }
    }

    /// Every row's blocks as [`Transcript::drawn`] and its call's committed
    /// fate say; changed rows touched.
    fn redraw_blocks(&mut self) {
        let mut changed = Vec::new();
        for (ix, it) in self.items.iter_mut().enumerate() {
            let want = crate::blocks::RowBlocks::of(self.drawn.as_deref(), it.id, it.fate);
            if it.blocks != want {
                changed.push(ix);
            }
            it.blocks = want;
        }
        for ix in changed {
            self.touch(ix);
        }
    }

    /// The board's input again: [`Transcript::blocks_rev`] moves if the
    /// history's events or the live log's did (its version: nothing of the
    /// turn is read here, the log wrote each event as it came). Rows folded
    /// again (the history replaced them) draw the last board's blocks
    /// until the next one lands, so nothing blinks; no other row is read.
    fn sync_blocks(&mut self) {
        self.blocks.prune();
        // With no block call there is no board: its live turn is no input.
        let live = if self.has_blocks() { self.live_log.version() } else { (0, 0) };
        let seen = (self.blocks.epoch(), self.blocks.len(), live);
        if self.blocks_seen != seen {
            self.blocks_seen = seen;
            self.blocks_rev = next_card_rev();
        }
        if std::mem::take(&mut self.rows_moved) && (self.drawn.is_some() || self.fate_epoch != 0) {
            self.redraw_blocks();
        }
    }

    /// The agent cards' fold of the history alone, without the live turn:
    /// the bindings the history confirms. A click answers a card only
    /// while this holds it as it was drawn, on a settled history
    /// ([`Transcript::history_settled`]): the live rows draw a card before
    /// the history has it, and the history wins when it lands (Shady #81
    /// P2-1).
    pub fn history_cards(&self) -> &cards_spec::fold::Fold {
        self.cards.fold()
    }

    /// The history folded is the one the latest `Conversation` digest
    /// describes, its length and hash (synaps-cards §3.5, "settled view";
    /// [`crate::history::HistoryMirror::matches`]), or no digest came since
    /// the attach's snapshot. After a digest that disagrees the client asks
    /// for the history again: until it lands, not settled; nor after the
    /// daemon said it dropped events ([`Transcript::resyncing`]).
    pub fn history_settled(&self) -> bool {
        !self.swap_pending && !self.resyncing && self.digest.as_ref().is_none_or(|d| self.mirror.agrees(d))
    }

    /// The daemon said it dropped events for this attachment and carried on
    /// (its "event stream lagged" notice, [`crate::protocol::stream_lagged`]):
    /// the turn's checkpoint and its last digest may be among them, so the
    /// history held, and the digest before the notice, are no word on
    /// anything since. The client asks for the history again at once
    /// ([`crate::Client`], as after a swap); until that lands (a
    /// `HistoryUpdate::Replace`), not settled. Everything after the notice
    /// came after what was dropped (one ordered broadcast), so the history
    /// that lands then is past the gap. A reattach ends it too (its
    /// snapshot).
    pub fn resyncing(&self) -> bool {
        self.resyncing
    }

    /// The steer the engine holds for its next round or turn, by the latest
    /// word (meaningful on a settled view, [`Transcript::history_settled`],
    /// whose slot is known, [`Transcript::slot_known`]).
    pub fn queued_message(&self) -> Option<&str> {
        self.queued.as_deref()
    }

    /// What [`Transcript::queued_message`] says is the engine's word since
    /// the last events the daemon dropped for this attachment, if it
    /// dropped any. On its notice the slot is unknown: the digest that
    /// moved it, and the echoes, may be among what was dropped, and the
    /// history asked for again has no slot (a `Messages` reply). It is
    /// known again only from an event on this attachment's own ordered
    /// stream that states it, at its place: a `Conversation` digest after
    /// the notice, a `Dequeued` after it (the daemon sends one only once
    /// it took the slot: empty, SynapsCLI `189a5a19`,
    /// `session/actor.rs:3512-3514` and `:3630-3632`), or an attach's
    /// snapshot. Not `SteeringDelivered`: the daemon clears the slot only
    /// if it holds that text, and then says so with the digest it sends
    /// next (`:3839-3843`); nor `Steered` (a driver's leaves the slot,
    /// `:3327`). Nothing read out of band: no query answers the slot, and
    /// an attach on another connection can't be told from another
    /// client's (Shady #81 round 5). Not empty: unknown. A steer read as
    /// lost on a stale slot is sent twice when the engine holds it
    /// (Shady #81 round 4).
    pub fn slot_known(&self) -> bool {
        !self.slot_unknown
    }

    /// A compaction runs: one heard to start (`CompactionStarted`), not yet
    /// applied, failed or cancelled. An attach can't tell (its snapshot has
    /// no flag): `false` after one.
    pub fn compacting(&self) -> bool {
        self.compacting
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

    /// What the daemon applied when prompt `id` resolved (SynapsCLI #437),
    /// taken with its event ([`Transcript::apply_with`]). Kept for the
    /// last [`OUTCOMES_KEPT`] prompts this attachment raised and resolved,
    /// until the next attach (a restarted daemon numbers its prompts from 1
    /// again), so a card a window resolves frames later still has it
    /// however many resolved meanwhile (Shady #70 C70-3). `None`: a daemon
    /// that does not say, an outcome this build does not know.
    pub fn resolved_outcome(&self, id: u64) -> Option<crate::protocol::PromptOutcome> {
        self.outcomes.iter().rev().find(|o| o.0 == id).map(|o| o.1).filter(|o| *o != crate::protocol::PromptOutcome::Unknown)
    }

    /// [`Transcript::apply_at`] with what the event says beyond its typed
    /// form ([`crate::protocol::EventMeta`]), in one step: a resolution's
    /// outcome is the prompt's as it leaves the pending set.
    pub fn apply_with(&mut self, ev: &SessionEvent, ts: &str, meta: &crate::protocol::EventMeta) {
        let raised = match ev {
            SessionEvent::PromptResolved { prompt_id } => self.pending_prompts.iter().any(|p| p.id == *prompt_id).then_some(*prompt_id),
            _ => None,
        };
        self.apply_at(ev, ts);
        if let (Some(id), Some(o)) = (raised, meta.prompt_outcome) {
            if self.outcomes.len() == OUTCOMES_KEPT {
                self.outcomes.pop_front();
            }
            self.outcomes.push_back((id, o));
        }
    }

    /// Whether call `tool_id` showed it got past its gate: output streamed
    /// from it, or a result said it ran (done, failed, stopped at its
    /// limit) or that a cancel found it started ([`GateNote::StartedCancel`]).
    /// Kept once seen: a later bare cancel (the live result of a HALT is the
    /// bare phrase even after output, the history's says it started), a
    /// settle or a refold does not take it back (synaps-desk #64, C64-1).
    /// `ToolStarted` is not evidence: the daemon sends it before the gate.
    /// Kept as long as the call is in the transcript, however many calls
    /// come after it, and across a re-attach to the same session (Shady
    /// #64 r2, C64-1): a receipt reads it for as long as its call is there.
    pub fn past_gate(&self, tool_id: &str) -> bool {
        self.past_gate.contains(tool_id)
    }

    fn note_past_gate(&mut self, tool_id: &str) {
        if tool_id.is_empty() || !self.past_gate.insert(tool_id.to_string()) {
            return;
        }
        if !self.rebuilding {
            self.prune_past_gate();
        }
    }

    /// Past its size, let go of the calls no longer in the transcript (a
    /// bounded worker's dropped rows, another session's): only theirs. Never
    /// halfway through a rebuild (Shady #64 r3, C64-1), when a call still
    /// to be refolded is out of `items`.
    fn prune_past_gate(&mut self) {
        if self.past_gate.len() > self.past_gate_cap.max(PAST_GATE_KEPT) {
            let held: std::collections::HashSet<&str> =
                self.items.iter().filter_map(|i| i.tool.as_ref()).map(|t| t.tool_id.as_str()).collect();
            let mut kept = std::mem::take(&mut self.past_gate);
            kept.retain(|id| held.contains(id.as_str()));
            self.past_gate = kept;
            self.past_gate_cap = 2 * self.past_gate.len();
        }
    }

    /// Run `f`, a rebuild of the rows: [`Transcript::past_gate`] lets go
    /// of nothing until the outermost one is done.
    fn rebuild<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = std::mem::replace(&mut self.rebuilding, true);
        let r = f(self);
        self.rebuilding = outer;
        if !outer {
            self.prune_past_gate();
        }
        r
    }

    /// A result for `tool_id`: evidence it got past its gate, if it is.
    fn note_result(&mut self, tool_id: &str, result: &str, outcome: ToolOutcome) {
        let ran = matches!(outcome, ToolOutcome::Ok | ToolOutcome::Failed | ToolOutcome::Unknown)
            || gate_note(result, "") == Some(GateNote::StartedCancel);
        if ran {
            self.note_past_gate(tool_id);
        }
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
        let at = self.stamp.clone();
        self.items.insert(
            ix,
            Item {
                id,
                rev,
                kind,
                text,
                tool,
                streaming,
                at,
                event: None,
                form: None,
                anchor: None,
                card: None,
                blocks: None,
                kept_input: None,
                seq: [0; 2],
                fate: None,
            },
        );
        self.mark(ix);
        ix
    }

    /// A live row: above the events not delivered yet.
    fn push_new(&mut self, kind: ItemKind, text: String, tool: Option<ToolCall>, streaming: bool) -> usize {
        let ix = self.push_at(self.live_end(), kind, text, tool, streaming, &mut Reuse::new());
        // A user row is its message: it came now.
        if kind == ItemKind::User {
            self.came(ix, false);
        }
        ix
    }

    /// Row `ix`'s call (a user row's message), or its result, came now:
    /// its place in the live stream ([`Item::seq()`]), set once.
    fn came(&mut self, ix: usize, result: bool) {
        let k = usize::from(result);
        if self.items[ix].seq[k] == 0 {
            self.live_seq += 1;
            self.items[ix].seq[k] = self.live_seq;
            self.card_touch(ix);
        }
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
        self.card_touch(ix);
    }

    /// Let go of what the card sync keeps to read what changed (the cards'
    /// fold kept): the next sync reads every row. What takes live rows
    /// away frees it here, not at the next sync.
    fn card_unkeep(&mut self) {
        self.card_kept = false;
        self.card_touched = HashMap::new();
        self.card_live.unkeep();
    }

    /// Row `ix` changed: the next card sync reads it, if it is a call or a
    /// user message ([`Transcript::card_touched`]).
    fn card_touch(&mut self, ix: usize) {
        let it = &self.items[ix];
        if self.card_kept && matches!(it.kind, ItemKind::Tool | ItemKind::User) {
            self.card_touched.insert(it.id, ix);
        }
    }

    /// Take every row, split into notes (kept) and reusable ids; the rows
    /// themselves are kept in `self.before` for [`Transcript::keep_revs`].
    fn take_rows(&mut self, from: usize) -> (Vec<Item>, Reuse) {
        self.mark(from);
        self.rows_moved = true;
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
        self.card_unkeep();
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
        let drawn = self.drawn.clone();
        for it in self.items.iter_mut().filter(|i| i.anchor.is_none()) {
            let Some(o) = before.get(&it.id) else { continue };
            // The same call, folded again: it keeps its committed fate
            // until the new history's board says (nothing blinks).
            if carry
                && o.fate.is_some()
                && it.fate.is_none()
                && o.tool.as_ref().zip(it.tool.as_ref()).is_some_and(|(a, b)| a.tool_id == b.tool_id)
            {
                it.fate = o.fate;
                it.blocks = crate::blocks::RowBlocks::of(drawn.as_deref(), it.id, it.fate);
            }
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
            if o.kind == it.kind
                && o.streaming == it.streaming
                && o.text == it.text
                && o.tool == it.tool
                && o.event == it.event
                && o.card == it.card
                && o.blocks == it.blocks
            {
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
        // Which calls ran: the same session's calls still are what they were.
        let (past_gate, past_gate_cap) = if same { (std::mem::take(&mut self.past_gate), self.past_gate_cap) } else { Default::default() };
        // The same conversation's rows draw its last board until the next
        // one lands (a reattach folds it again: nothing blinks).
        let drawn = if carry { self.drawn.take() } else { None };
        *self = Transcript {
            next_id,
            clean_len,
            before,
            no_subagent_notices,
            pending,
            stuck,
            past_gate,
            past_gate_cap,
            rebuilding: true,
            engine_pending: a.conversation.pending_events_len,
            session_id: Some(a.meta.id.clone()),
            journal_id: journal,
            compactions: if same { self.compactions } else { 0 },
            drawn,
            // Its log's bin carries over ([`crate::blocks::dispose`]).
            live_log: self.live_log.renewed(),
            ..Transcript::default()
        };
        self.dirty_from = Some(0);
        self.title = a.meta.title.clone().or_else(|| a.conversation.header.get("title").and_then(Value::as_str).map(str::to_string));
        self.cost_usd = a.conversation.cost;
        self.pending_prompts = a.pending_prompts.clone();
        self.mirror = crate::history::HistoryMirror::new(&a.conversation.api_messages);
        self.queued.clone_from(&a.conversation.queued_message);
        self.fold_with_notes(&a.conversation.api_messages, notes, &mut reuse);
        self.busy = a.streaming;
        self.replaying = true;
        for env in &a.replay {
            if let Ok(ev) = env.decode() {
                self.apply_with(&ev, &env.ts, &crate::protocol::EventMeta::of(&env.event));
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
        self.sync_cards();
        self.keep_revs(carry);
        self.sync_card_times();
        // The rows, the history's and the replay's, are all back.
        self.rebuilding = false;
        self.prune_past_gate();
        Ok(())
    }

    /// An `Event::History` update from the client.
    pub fn apply_update(&mut self, update: &HistoryUpdate) -> Result<(), TranscriptError> {
        match update {
            HistoryUpdate::Append { base_len, messages } => {
                if *base_len != self.history_len {
                    return Err(TranscriptError::OutOfSync { have: self.history_len, base_len: *base_len });
                }
                self.mirror.append(messages);
                self.rebuild(|t| t.append_history(messages));
            }
            HistoryUpdate::Replace { messages } => {
                // The history asked for again: one that still disagrees with
                // the digest on the hash alone is trusted by its length, as
                // the client's mirror does.
                self.mirror.replaced(messages, self.digest.as_ref());
                self.swap_pending = false;
                self.resyncing = false;
                self.rebuild(|t| t.replace_history(messages));
            }
        }
        Ok(())
    }

    /// A full `MessageHistory` without a mirror (benches, digest-less
    /// replays): longer or equal is taken as an append, shorter as a
    /// replacement. Clients get [`HistoryUpdate`]s instead, which tell.
    pub fn apply_history(&mut self, messages: &[Value]) {
        self.rebuild(|t| {
            if messages.len() >= t.history_len {
                let start = t.history_len;
                t.mirror.append(&messages[start..]);
                t.append_history(&messages[start..]);
            } else {
                t.mirror.reset(messages);
                t.replace_history(messages);
            }
        });
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
        // The live turn it took is the history's now.
        self.live_log = self.live_log.renewed();
        if !self.busy {
            self.settle();
        }
        // The live rows it took were the cards' live ones too.
        self.cards_dirty = true;
        self.sync_cards();
        // An append matched this conversation's prefix: the rows it took
        // are its own live ones, whatever swap came before it (a resume of
        // the same journal has no replacement to consume the flag).
        self.keep_revs(true);
        self.sync_card_times();
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
        self.sync_cards();
        self.keep_revs(!swapped);
        self.sync_card_times();
    }

    /// Fold a whole history from empty, interleaving `notes` by anchor.
    /// Events not delivered yet go last, whatever their anchors.
    fn fold_with_notes(&mut self, messages: &[Value], notes: Vec<Item>, reuse: &mut Reuse) {
        self.after_summary = false;
        self.cards = crate::cards::HistoryCards::default();
        self.blocks = crate::blocks::HistoryBlocks::default();
        self.live_log = self.live_log.renewed();
        self.cards_dirty = true;
        self.recount = true;
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
            // A call cut off ends now: its call and its result came.
            if open_tool {
                self.came(ix, false);
                self.came(ix, true);
            }
            self.touch(ix);
        }
    }

    /// The attachment ended: nothing will stream any more.
    pub fn set_detached(&mut self) {
        self.busy = false;
        self.settle();
        self.cards_dirty = true;
        self.sync_cards();
        // Nothing streams into it now: what reads what changed is no use.
        self.card_unkeep();
    }

    fn fold_message(&mut self, m: &Value, reuse: &mut Reuse) {
        // Its rows have its time or none: never the event's it came in.
        let outer = (std::mem::replace(&mut self.stamp, message_at(m).map(Arc::from)), std::mem::take(&mut self.event_ts));
        let first = self.items.len();
        self.fold_blocks(m, reuse);
        (self.stamp, self.event_ts) = outer;
        // Its rows are the ones it just pushed (notes go between messages).
        self.cards.message(m, &self.items[first..]);
        self.blocks.message(m, &self.items[first..]);
        self.cards_dirty = true;
        self.recount = true;
    }

    /// Every row's card ([`Item::card`]) as the fold now says: the
    /// history's, then the live rows'. A row whose card changed is touched.
    /// Read from what changed since the last sync when it can be
    /// ([`Transcript::step_cards`]); after what takes live rows away, from
    /// every row.
    fn sync_cards(&mut self) {
        if !std::mem::take(&mut self.cards_dirty) {
            return;
        }
        self.sync_blocks();
        let live = self.history_end.min(self.items.len());
        if std::mem::take(&mut self.recount) {
            self.live_cards = self.items[live..].iter().any(|i| i.tool.as_ref().is_some_and(|t| crate::cards::is_card_tool(&t.name)));
        }
        // No card called anywhere and none drawn: nothing to fold (a
        // session without cards pays only this, no row read).
        if self.cards.is_empty() && !self.live_cards && self.card_rows == 0 {
            if !self.card_live.fold().cards().is_empty() {
                self.card_rev = next_card_rev();
            }
            self.card_live = crate::cards::LiveCards::default();
            self.card_kept = false;
            self.card_touched = HashMap::new();
            return;
        }
        #[cfg(test)]
        if CARD_REREAD.get() {
            self.card_kept = false;
        }
        let touched = std::mem::take(&mut self.card_touched);
        if self.card_kept && self.step_cards(live, &touched) {
            return;
        }
        let had = std::mem::replace(&mut self.card_live, crate::cards::LiveCards::of(&self.cards, &self.items[live..]));
        self.card_visits += (self.items.len() - live) as u64;
        if had.fold().cards() != self.card_live.fold().cards() {
            self.card_rev = next_card_rev();
        }
        self.card_rows = self.card_live.want.len();
        let mut changed = Vec::new();
        self.card_visits += self.items.len() as u64;
        for (ix, it) in self.items.iter_mut().enumerate() {
            match self.card_live.want.get_mut(&it.id) {
                Some(card) => {
                    self.card_live.at.insert(it.id, ix);
                    match &it.card {
                        // The same card: the row's, kept, not a copy.
                        Some(was) if **was == **card => *card = was.clone(),
                        _ => {
                            it.card = Some(card.clone());
                            changed.push(ix);
                        }
                    }
                }
                None => {
                    if it.card.take().is_some() {
                        changed.push(ix);
                    }
                }
            }
        }
        self.card_live.share_closes();
        for ix in changed {
            self.touch(ix);
        }
        self.card_kept = true;
        self.sync_card_times();
    }

    /// The cards' sync of the rows `touched` alone (the calls and user rows
    /// that changed since the last, [`crate::cards::LiveCards::step`]), and
    /// of the rows whose card that changes. False if it can't be (a row
    /// moved, or changed otherwise than by new events): read them all.
    fn step_cards(&mut self, live: usize, touched: &HashMap<u64, usize>) -> bool {
        self.card_visits += touched.len() as u64;
        if touched.iter().any(|(&id, &ix)| self.items.get(ix).is_none_or(|i| i.id != id)) {
            return false;
        }
        let rows = touched.values().filter(|&&ix| ix >= live).map(|&ix| &self.items[ix]);
        let Some(step) = self.card_live.step(&self.cards, rows) else {
            return false;
        };
        if step.cards {
            self.card_rev = next_card_rev();
        }
        if step.rows.is_empty() {
            return true;
        }
        // Where each row is: a touched one's place, else where it was drawn.
        let mut at = touched.clone();
        for id in &step.rows {
            if !at.contains_key(id)
                && let Some(&ix) = self.card_live.at.get(id)
            {
                at.insert(*id, ix);
            }
        }
        let asked = |c: Option<&crate::cards::CardRow>| matches!(c, Some(crate::cards::CardRow::Ask(_)));
        let mut asks = false;
        let mut changed = Vec::new();
        for id in &step.rows {
            let ix = match at.get(id).copied().filter(|&ix| self.items.get(ix).is_some_and(|i| i.id == *id)) {
                Some(ix) => ix,
                None => {
                    // It moved (a row above it went): look for it.
                    self.card_visits += self.items.len() as u64;
                    match self.items.iter().position(|i| i.id == *id) {
                        Some(ix) => ix,
                        None => {
                            // Gone: nothing draws it.
                            self.card_live.at.remove(id);
                            continue;
                        }
                    }
                }
            };
            self.card_visits += 1;
            let card = self.card_live.want.get(id);
            asks |= asked(card.map(|c| &**c)) != asked(self.items[ix].card.as_deref());
            match card {
                Some(_) => self.card_live.at.insert(*id, ix),
                None => self.card_live.at.remove(id),
            };
            if self.items[ix].card.as_deref() != card.map(|c| &**c) {
                self.items[ix].card = card.cloned();
                changed.push(ix);
            }
        }
        let rows = self.card_live.want.len();
        // Redundant now (Shady #102): a row drawn first or last is an ask
        // whose own change set `asks` above. Kept, cheap, for a fold that
        // may draw an answer or a close with no ask.
        asks |= (rows == 0) != (self.card_rows == 0);
        self.card_rows = rows;
        // Its own touches are no change to read again.
        self.card_kept = false;
        for ix in changed {
            self.touch(ix);
        }
        self.card_kept = true;
        // The rows that ask a card changed: their times, in row order.
        if asks {
            let mut ixs = Vec::new();
            for (id, c) in &self.card_live.want {
                if !asked(Some(&**c)) {
                    continue;
                }
                self.card_visits += 1;
                match self.card_live.at.get(id).copied().filter(|&ix| self.items.get(ix).is_some_and(|i| i.id == *id)) {
                    Some(ix) => ixs.push(ix),
                    None => return false,
                }
            }
            ixs.sort_unstable();
            self.card_times_of(&ixs);
        }
        true
    }

    /// The times of the rows that ask a card ([`Item::at`]), once the
    /// history settled them (a time carried or filled by
    /// [`Transcript::keep_revs`]): the waiting order reads them, so a
    /// change moves [`Transcript::cards_rev`] as the cards' own do. Only
    /// the rows read, and none while no row draws a card.
    fn sync_card_times(&mut self) {
        let mut asks = Vec::new();
        if self.card_rows != 0 {
            self.card_visits += self.items.len() as u64;
            asks.extend((0..self.items.len()).filter(|&ix| matches!(self.items[ix].card.as_deref(), Some(crate::cards::CardRow::Ask(_)))));
        }
        self.card_times_of(&asks);
    }

    /// [`Transcript::sync_card_times`] of the rows `asks` (in row order).
    fn card_times_of(&mut self, asks: &[usize]) {
        let times = if self.card_rows == 0 {
            0
        } else {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for i in asks.iter().map(|&ix| &self.items[ix]) {
                (i.tool.as_ref().map(|t| t.tool_id.as_str()), i.at.as_deref()).hash(&mut h);
            }
            h.finish().max(1)
        };
        if times != self.card_times {
            self.card_times = times;
            self.card_rev = next_card_rev();
        }
    }

    fn fold_blocks(&mut self, m: &Value, reuse: &mut Reuse) {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        let content = m.get("content").unwrap_or(&Value::Null);
        // A compaction's two messages (#14), drawn as nothing, as the TUI
        // and the daemon's display projection do: the summary, and right
        // after it the daemon's acknowledgement, word for word (Shady
        // C63-1: not whatever the assistant said next; the projection draws
        // no string-content assistant message, this client draws all but
        // this one).
        let after_summary = std::mem::take(&mut self.after_summary);
        if is_compaction_summary(m) {
            self.after_summary = true;
            return;
        }
        if after_summary && role == "assistant" && content.as_str().is_some_and(|s| COMPACTION_ACKS.contains(&s)) {
            return;
        }
        match (role, content) {
            ("user", Value::String(s)) if is_event_form(role, s) => self.fold_event(s, reuse),
            ("user", Value::String(s)) => {
                self.push(ItemKind::User, s.clone(), None, false, reuse);
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
                                started: 0,
                            };
                            let kept = crate::blocks::keeps_input(&tool.name)
                                .then(|| Arc::new(crate::blocks::canonical(b.get("input").unwrap_or(&Value::Null))));
                            let ix = self.push(ItemKind::Tool, String::new(), Some(tool), false, reuse);
                            self.items[ix].kept_input = kept;
                        }
                        "tool_result" => {
                            flush(self, &mut text, reuse);
                            let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                            let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            let result = result_text(b.get("content").unwrap_or(&Value::Null));
                            if let Some(&ix) = self.tools.get(id) {
                                let outcome = tool_outcome(&result, is_error);
                                self.note_result(id, &result, outcome);
                                if let Some(t) = self.items[ix].tool.as_mut() {
                                    t.status = Some(outcome.status());
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
                let ix = self.push_new(ItemKind::User, text.clone(), None, false);
                self.live_log.user(&self.blocks, text, Some(self.items[ix].id));
            }
            // A drained event's payload: its parent's business (its history
            // holds it as a user message all the same).
            WatchEvent::Steered { text } => self.live_log.user(&self.blocks, text, None),
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
        // No history: no board reads the turn (a worker's), and its facts
        // would outgrow the rows they left.
        self.live_log = self.live_log.renewed();
        self.tools.retain(|_, ix| *ix >= n);
        for ix in self.tools.values_mut() {
            *ix -= n;
        }
        self.response_start = self.response_start.map(|s| s.saturating_sub(n));
        self.card_unkeep();
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
        // What the cards read changes with a call's input or result, a
        // turn's start (its user message), a stream reset, a turn's end;
        // never with the deltas that stream text in.
        let delta = matches!(
            ev,
            SessionEvent::Stream {
                event: StreamEvent::Llm(
                    LlmEvent::Text { .. }
                        | LlmEvent::Thinking { .. }
                        | LlmEvent::ToolUseStart { .. }
                        | LlmEvent::ToolUseDelta { .. }
                        | LlmEvent::ToolResultDelta { .. }
                )
            }
        );
        self.cards_dirty |= !delta;
        self.sync_cards();
    }

    fn apply_event(&mut self, ev: &SessionEvent) {
        match ev {
            SessionEvent::TurnStarted { user_text, .. } => {
                self.busy = true;
                self.deliver_all();
                if let Some(t) = user_text {
                    let ix = self.push_new(ItemKind::User, t.clone(), None, false);
                    self.live_log.user(&self.blocks, t, Some(self.items[ix].id));
                }
            }
            SessionEvent::Stream { event } => self.apply_stream(event),
            SessionEvent::Conversation { digest } => {
                if let Some(t) = digest.header.get("title").and_then(Value::as_str) {
                    self.title = Some(t.to_string());
                }
                // A digest that matches by hash trusts the hash again.
                self.mirror.matches(digest);
                self.queued.clone_from(&digest.queued_message);
                self.slot_unknown = false;
                self.digest = Some(crate::protocol::ConversationDigest { header: Value::Null, tokens: Value::Null, ..digest.clone() });
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
            // An agent card's answer (its grammar, synaps-cards §3.3): its
            // card says where it is (queued, then answered), never its JSON.
            SessionEvent::Steered { text, .. } if cards_spec::message::parse(text).is_some() => {}
            SessionEvent::Steered { text, .. } => self.notice(ItemKind::Notice, format!("steered: {text}")),
            SessionEvent::SystemNotice { text } => {
                // Events were dropped here ([`Transcript::resyncing`]): the
                // digest before them may describe a history they moved past,
                // and one after them is checked against the history asked
                // for again. A replay's is a past connection's.
                if crate::protocol::stream_lagged(text) && !self.replaying {
                    self.resyncing = true;
                    self.digest = None;
                    self.slot_unknown = true;
                }
                self.notice(ItemKind::Notice, text.clone())
            }
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
                self.swap_pending = true;
                self.drop_held();
                self.swapped = true;
                self.journal_id.clone_from(new_id);
                self.notice(ItemKind::Notice, format!("resumed {new_id}"));
                if let Some(n) = clamp_notice {
                    self.notice(ItemKind::Notice, n.clone());
                }
            }
            SessionEvent::CompactionStarted => self.compacting = true,
            SessionEvent::CompactionApplied { session_id, msg_count, .. } => {
                self.compacting = false;
                self.compactions += 1;
                self.swap_pending = true;
                self.swapped = true;
                self.journal_id.clone_from(session_id);
                self.notice(ItemKind::Notice, format!("conversation compacted ({msg_count} messages)"));
            }
            SessionEvent::CompactionFailed { message } => {
                self.compacting = false;
                self.stick();
                self.notice(ItemKind::Error, format!("compaction failed: {message}"))
            }
            SessionEvent::CompactionCancelled => {
                self.compacting = false;
                self.stick();
            }
            // The daemon took the slot (`queued_message`) and gives its
            // text back: empty now, at this place, dropped events or not
            // ([`Transcript::slot_known`]). A replay's is from before the
            // snapshot, which says the slot.
            SessionEvent::Dequeued { .. } if !self.replaying => {
                self.queued = None;
                self.slot_unknown = false;
            }
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
                    self.live_log.response();
                }
                // The response's rows go; an event's that came meanwhile stays.
                LlmEvent::ResponseReset => {
                    if let Some(s) = self.response_start.filter(|&s| s >= self.history_end && s <= self.items.len()) {
                        let kept: Vec<Item> = self.items.drain(s..).filter(|i| i.kind == ItemKind::Event && i.is_note()).collect();
                        self.card_unkeep();
                        self.items.extend(kept);
                        self.tools.retain(|_, ix| *ix < s);
                        self.mark(s);
                        self.live_log.reset();
                    }
                }
                LlmEvent::Thinking { text } => {
                    self.live_log.content(&self.blocks);
                    self.append_live(ItemKind::Thinking, text)
                }
                LlmEvent::Text { text } => {
                    self.live_log.content(&self.blocks);
                    self.append_live(ItemKind::Assistant, text)
                }
                LlmEvent::ToolUseStart { tool_name, tool_id } => {
                    self.end_streaming_text();
                    self.live_cards |= crate::cards::is_card_tool(tool_name);
                    let tool = ToolCall {
                        tool_id: tool_id.clone(),
                        name: tool_name.clone(),
                        status: Some(ToolStatus::Streaming),
                        ..Default::default()
                    };
                    let ix = self.push_new(ItemKind::Tool, String::new(), Some(tool), true);
                    self.live_log.started(&self.blocks, self.items[ix].id, tool_id, tool_name);
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
                    let card = crate::cards::is_card_tool(tool_name);
                    let kept = crate::blocks::keeps_input(tool_name).then(|| Arc::new(crate::blocks::canonical(input)));
                    // A live call; the history's own (an attach's replay of
                    // a call its history holds) is the history's. The row
                    // keeps the input as the log does: the same input again
                    // is the one both kept, one allocation (counted once,
                    // the row's).
                    self.items[ix].kept_input = if ix >= self.history_end {
                        self.live_log.call(&self.blocks, self.items[ix].id, tool_id, tool_name, kept)
                    } else {
                        kept
                    };
                    self.live_cards |= card;
                    self.came(ix, false);
                    self.touch(ix);
                }
                LlmEvent::ToolQueued { tool_id } | LlmEvent::ToolStarted { tool_id } => {
                    let queued = matches!(l, LlmEvent::ToolQueued { .. });
                    if let Some(ix) = self.tool_mut(tool_id) {
                        if let Some(t) = self.items[ix].tool.as_mut() {
                            t.status = Some(if queued { ToolStatus::Queued } else { ToolStatus::Running });
                            if !queued && t.started == 0 {
                                self.started += 1;
                                t.started = self.started;
                            }
                        }
                        self.came(ix, false);
                        self.touch(ix);
                    }
                }
                LlmEvent::ToolResultDelta { tool_id, delta } => {
                    if !delta.is_empty() && self.tool_mut(tool_id).is_some() {
                        self.note_past_gate(tool_id);
                    }
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
                    // The final result, as the history will hold it: a
                    // delta never is one, nor a row's preview.
                    self.live_log.result(&self.blocks, tool_id, result);
                    if let Some(ix) = self.tool_mut(tool_id) {
                        let outcome = tool_outcome(result, false);
                        self.note_result(tool_id, result, outcome);
                        if let Some(t) = self.items[ix].tool.as_mut() {
                            t.result = preview(result);
                            t.tail = tail(result);
                            t.status = Some(outcome.status());
                        }
                        self.items[ix].streaming = false;
                        self.came(ix, false);
                        self.came(ix, true);
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
                    // The engine drained it into the history, after the
                    // round's results: a user message of its text there
                    // (an event's payload too; the board reads which text
                    // is the user's own, blocks.md §7).
                    self.live_log.user(&self.blocks, message, None);
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

/// The assistant message the daemon writes right after a compaction
/// summary (SynapsCLI `agent-core` `core/compaction.rs`
/// `compaction_context_messages`), and the one it wrote before T30.
pub const COMPACTION_ACKS: [&str; 2] = [
    "I've loaded the conversation summary. Ready to continue.",
    "I've loaded the conversation summary and system prompt. Ready to continue.",
];

/// A compaction summary in a history (#14): a message whose content is a
/// string holding `<context-summary>`, any role. The daemon's own rule
/// (SynapsCLI dev `agent-engine` `session/display.rs` `items_of`, and
/// `runtime/compaction.rs`): it writes the summary inside a sentence
/// (`agent-core` `compaction_context_messages`: "The conversation history
/// before this point was compacted into the following summary:\n\n
/// <context-summary>…"), with no structural marker; older daemons wrote it
/// at the start. Not a block's text (the daemon's rule reads only string
/// content). Parity holds for plain messages: the daemon reads
/// `user_content(msg)`, which turns a hooked one-text-block body back into
/// a string first; this reads the raw content only. Shared with the
/// daemon, and known: a plain string message that only quotes the tag (a
/// user asking about it, an assistant naming it) is not drawn either; the
/// message after it is, unless it is one of [`COMPACTION_ACKS`]. A
/// structural marker upstream would end that.
pub fn is_compaction_summary(m: &Value) -> bool {
    m.get("content").and_then(Value::as_str).is_some_and(|s| s.contains("<context-summary>"))
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

/// The next fold revision ([`Transcript::cards_rev`]): process-wide, so a
/// revision names one fold of one transcript.
fn next_card_rev() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// Tests: every card sync reads every row, as it did before it kept
    /// its read ([`Transcript::card_kept`]).
    static CARD_REREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
#[path = "card_sync_tests.rs"]
mod card_sync_tests;

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
            // Not that text whole (Shady #64 r2 C64-2): a failure's.
            ("Tool execution failed: tool activation denied: tool activation requires host confirmation authority", Failed),
            (
                "Tool execution failed: tool activation denied: tool activation requires host confirmation authority — example from a manual",
                Failed,
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

    /// synaps-desk #64 (Shady C64-2): the gate's forms are read from a
    /// whole result, never searched for in output.
    #[test]
    fn gate_notes_are_the_engines_whole_forms() {
        let gate = "Tool call blocked by extension: Tool call confirmation denied: guard wants to check this";
        let activation = "Tool execution failed: tool activation denied: tool activation requires host confirmation authority; \
                          model-initiated activation was not confirmed";
        let activation_cut = "Tool execution failed: tool activation denied: tool activation requires host confirmation authority";
        let started = "Canceled (session stopped by the host). It had already started and may have partially applied its effects.";
        let partial = "a\nb\n\n[Canceled by user before the tool finished; the output above is partial.]";
        let partial_fx = "a\n\n[Canceled by user before the tool finished; the output above is partial. It had already started and may have \
                          partially applied its effects.]";
        use GateNote::*;
        for (r, want) in [
            (gate, Some(Denied)),
            (activation, Some(Denied)),
            ("Canceled by user", Some(BareCancel)),
            ("Canceled (session driver revoked)", Some(BareCancel)),
            (started, Some(StartedCancel)),
            (partial, Some(StartedCancel)),
            (partial_fx, Some(StartedCancel)),
            // Output that quotes them, or a near miss, is output.
            (&format!("log:\n{gate}"), None),
            ("Canceled by user\n", None),
            ("Canceled by user. It had already started", None),
            ("a\n[Canceled by user before the tool finished; the output above is partial.] trailing", None),
            ("Tool call blocked by extension: policy says no", None),
            ("Tool call denied: x", None),
            ("ok", None),
            // The activation refusal, whole only: shortened, followed by
            // more, or spaced otherwise, it is output (Shady #64 r2 C64-2).
            (activation_cut, None),
            (&format!("{activation} — example from a manual, not a denial"), None),
            (&format!("{activation}\n"), None),
            (&activation.replacen("denied: tool", "denied:  tool", 1), None),
            ("Tool execution failed: tool activation denied: tool activation requires host confirmation authority — example", None),
        ] {
            assert_eq!(gate_note(r, ""), want, "{r:?}");
        }
        // A cut result: its tail holds the note.
        assert_eq!(gate_note("a…", "\n[Canceled by user before the tool finished; the output above is partial.]"), Some(StartedCancel));
    }

    /// synaps-desk #64 (Shady C64-1): a call that showed it got past its
    /// gate stays so. Output streams, then a HALT's live result is the
    /// bare phrase (the daemon's history has the started note): kept. Output,
    /// then the turn ends with no result (a settle): kept. A bare cancel
    /// with nothing before it: not evidence. `ToolStarted`: not evidence.
    #[test]
    fn a_call_that_got_past_its_gate_stays_past_it() {
        let ev = |v: Value| crate::protocol::decode_event(&v).unwrap();
        let llm = |v: Value| ev(json!({"ev": "stream", "event": v}));
        let start = |t: &mut Transcript, id: &str| {
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "bash", "tool_id": id, "input": {}})));
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": id})));
        };
        let delta = |id: &str| llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": id, "delta": "working…\n"}));
        let result = |id: &str, r: &str| llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": r}));
        let mut t = Transcript::new();
        t.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: String::new(), user_text: None });
        // Delta, bare cancel, then the history's started form.
        start(&mut t, "a");
        assert!(!t.past_gate("a"), "ToolStarted comes before the gate");
        t.apply(&delta("a"));
        assert!(t.past_gate("a"));
        t.apply(&result("a", "Canceled by user"));
        assert!(t.past_gate("a"), "the bare cancel overwrote the output, not the evidence");
        // A bare cancel with nothing before it.
        start(&mut t, "b");
        t.apply(&result("b", "Canceled by user"));
        assert!(!t.past_gate("b"));
        // Delta, then the turn ends with no result.
        start(&mut t, "c");
        t.apply(&delta("c"));
        t.apply(&SessionEvent::Idle);
        let c = t.items().iter().find_map(|i| i.tool.as_ref().filter(|t| t.tool_id == "c")).unwrap();
        assert_eq!(c.status, Some(ToolStatus::Cancelled), "settled");
        assert!(t.past_gate("c"));
        // The history refold (the started note for `a`, the bare phrase for `b`).
        let call = |id: &str| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "bash", "input": {}}]});
        let res = |id: &str, r: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": r}]});
        t.apply_history(&[
            json!({"role": "user", "content": "go"}),
            call("a"),
            res("a", "working…\n\n[Canceled by user before the tool finished; the output above is partial.]"),
            call("b"),
            res("b", "Canceled by user"),
            call("c"),
            res("c", "working…\n\n[Canceled by user before the tool finished; the output above is partial.]"),
        ]);
        assert!(t.past_gate("a") && t.past_gate("c") && !t.past_gate("b"));
        // A fresh transcript reading only the history learns it from the note.
        let mut h = Transcript::new();
        h.apply_history(&[call("a"), res("a", "working…\n\n[Canceled by user before the tool finished; the output above is partial.]")]);
        assert!(h.past_gate("a"));
        // A result that ran is evidence too; a deny is not.
        start(&mut t, "d");
        t.apply(&result("d", "done"));
        start(&mut t, "e");
        t.apply(&result("e", "Tool call blocked by extension: Tool call confirmation denied: guard"));
        assert!(t.past_gate("d") && !t.past_gate("e"));
    }

    /// Shady #64 r2 C64-1: a call's evidence that it ran lives as long as
    /// the call: 300 ordinary calls after it don't push it out (a receipt
    /// still on screen reads it), nor does a re-attach to the same session.
    /// Another session's calls are not kept.
    #[test]
    fn evidence_a_call_ran_lives_as_long_as_the_call() {
        let ev = |v: Value| crate::protocol::decode_event(&v).unwrap();
        let llm = |v: Value| ev(json!({"ev": "stream", "event": v}));
        let call = |t: &mut Transcript, id: &str| {
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "bash", "tool_id": id, "input": {}})));
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": id})));
        };
        let attach = |id: &str| {
            let a = json!({"type": "attached", "client": 2, "meta": {"id": id}, "conversation": {"api_messages": []}, "input_owner": 2});
            let crate::protocol::DaemonFrame::Attached(a) = crate::protocol::decode_frame(&a.to_string()).unwrap() else { panic!() };
            a
        };
        let mut t = Transcript::new();
        t.load_attached(&attach("s")).unwrap();
        t.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: String::new(), user_text: None });
        call(&mut t, "a");
        t.apply(&llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": "a", "delta": "removed 3 files\n"})));
        t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "a", "result": "Canceled by user"})));
        for k in 0..300 {
            let id = format!("o{k}");
            call(&mut t, &id);
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": "ok"})));
        }
        assert!(t.past_gate("a"), "300 calls later, `a` still ran");
        assert!(t.past_gate("o0") && t.past_gate("o299"));
        t.load_attached(&attach("s")).unwrap();
        assert!(t.past_gate("a"), "a re-attach to the same session keeps it");
        t.load_attached(&attach("other")).unwrap();
        assert!(!t.past_gate("a"), "another session's calls are not this one's");
    }

    /// Shady #64 r3 C64-1: a rebuild that crosses the size watermark lets
    /// go of nothing until its rows are all back. 256 calls ran, `a` among
    /// them, known only from its output (its result is the bare cancel, so
    /// the history can't tell again). A rebuild whose first call `b` is new
    /// and ran (the 257th) keeps `a`; a call the rebuild left out (`o0`)
    /// goes once it is done. A history replace, an append and a re-attach
    /// to the same session. On `7e35605`, `a` was let go at `b`.
    #[test]
    fn a_rebuild_past_the_watermark_keeps_a_later_calls_evidence() {
        let ev = |v: Value| crate::protocol::decode_event(&v).unwrap();
        let llm = |v: Value| ev(json!({"ev": "stream", "event": v}));
        let attach = |history: Vec<Value>| {
            let a =
                json!({"type": "attached", "client": 2, "meta": {"id": "s"}, "conversation": {"api_messages": history}, "input_owner": 2});
            let crate::protocol::DaemonFrame::Attached(a) = crate::protocol::decode_frame(&a.to_string()).unwrap() else { panic!() };
            a
        };
        let others: Vec<String> = (0..PAST_GATE_KEPT - 1).map(|k| format!("o{k}")).collect();
        let ran = || {
            let mut t = Transcript::new();
            t.load_attached(&attach(Vec::new())).unwrap();
            t.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: String::new(), user_text: None });
            for id in std::iter::once("a").chain(others.iter().map(String::as_str)) {
                t.apply(&llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "bash", "tool_id": id, "input": {}})));
                t.apply(&llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": id})));
                if id == "a" {
                    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result_delta", "tool_id": "a", "delta": "removed 3 files\n"})));
                    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "a", "result": "Canceled by user"})));
                } else {
                    t.apply(&llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": "ok"})));
                }
            }
            t.apply(&SessionEvent::Idle);
            assert!(t.past_gate("a") && t.past_gate("o0"));
            t
        };
        let call = |id: &str| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "bash", "input": {}}]});
        let res = |id: &str, r: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": r}]});
        let mut history =
            vec![json!({"role": "user", "content": "go"}), call("b"), res("b", "ok"), call("a"), res("a", "Canceled by user")];
        for id in &others[1..] {
            history.push(call(id));
            history.push(res(id, "ok"));
        }
        let (mut replace, mut append, mut attached) = (ran(), ran(), ran());
        replace.apply_update(&HistoryUpdate::Replace { messages: history.clone() }).unwrap();
        append.apply_update(&HistoryUpdate::Append { base_len: 0, messages: history.clone() }).unwrap();
        attached.load_attached(&attach(history)).unwrap();
        // Per rebuild: a's row is there, a still ran, b and o1 ran, o0 (no
        // longer there) went once the rebuild was done.
        let got: Vec<_> = [("replace", &replace), ("append", &append), ("re-attach", &attached)]
            .into_iter()
            .map(|(how, t)| {
                let row = t.items().iter().any(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == "a"));
                (how, row, t.past_gate("a"), t.past_gate("b") && t.past_gate("o1"), t.past_gate("o0"))
            })
            .collect();
        assert!(got.iter().all(|&(_, row, a, others, o0)| row && a && others && !o0), "{got:?}");
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

    /// SynapsCLI #437: a prompt remembers what the daemon applied, taken
    /// with its event ([`Transcript::apply_with`]); an older daemon's (no
    /// `outcome`) and one this build does not know read as nothing said.
    /// A resolution of a prompt this attachment never raised records
    /// nothing.
    #[test]
    fn a_resolved_prompt_remembers_what_the_daemon_applied() {
        use crate::protocol::{EventMeta, PromptOutcome, decode_event};
        let mut t = Transcript::new();
        let raise = |t: &mut Transcript, id: u64| {
            let raw = serde_json::json!({"ev": "prompt", "request": {"id": id, "kind": "confirm", "title": "t", "prompt": "p"}});
            t.apply(&decode_event(&raw).unwrap());
        };
        let resolve = |t: &mut Transcript, raw: serde_json::Value| t.apply_with(&decode_event(&raw).unwrap(), "", &EventMeta::of(&raw));
        for (id, outcome) in [(1, "allowed"), (2, "denied"), (3, "answered"), (4, "cancelled"), (5, "timed_out"), (6, "vetoed")] {
            raise(&mut t, id);
            resolve(&mut t, serde_json::json!({"ev": "prompt_resolved", "prompt_id": id, "outcome": outcome}));
        }
        raise(&mut t, 7);
        resolve(&mut t, serde_json::json!({"ev": "prompt_resolved", "prompt_id": 7}));
        resolve(&mut t, serde_json::json!({"ev": "prompt_resolved", "prompt_id": 9, "outcome": "allowed"}));
        let got: Vec<_> = (1..=9).map(|id| t.resolved_outcome(id)).collect();
        use PromptOutcome::*;
        assert_eq!(got, [Some(Allowed), Some(Denied), Some(Answered), Some(Cancelled), Some(TimedOut), None, None, None, None]);
        assert_eq!(EventMeta::of(&serde_json::json!({"ev": "idle", "outcome": "allowed"})), EventMeta::default(), "only prompt_resolved");
    }

    /// Shady #70 C70-3: a prompt's outcome is kept until the next attach,
    /// however many prompts resolve after it before a window reads it (a
    /// minimised window resolves its card frames later).
    #[test]
    fn a_resolved_prompts_outcome_outlives_later_resolutions() {
        use crate::protocol::{EventMeta, PromptOutcome, decode_event};
        let mut t = Transcript::new();
        let raise = |t: &mut Transcript, id: u64| {
            let raw = serde_json::json!({"ev": "prompt", "request": {"id": id, "kind": "confirm", "title": "t", "prompt": "p"}});
            t.apply(&decode_event(&raw).unwrap());
        };
        let resolve = |t: &mut Transcript, id: u64, o: &str| {
            let raw = serde_json::json!({"ev": "prompt_resolved", "prompt_id": id, "outcome": o});
            t.apply_with(&decode_event(&raw).unwrap(), "", &EventMeta::of(&raw));
        };
        raise(&mut t, 1);
        resolve(&mut t, 1, "allowed");
        for id in 2..40 {
            raise(&mut t, id);
            resolve(&mut t, id, "denied");
        }
        assert_eq!(t.resolved_outcome(1), Some(PromptOutcome::Allowed), "38 resolutions later");
        let a = json!({"type": "attached", "client": 2, "meta": {"id": "s"}, "conversation": {"api_messages": []}, "input_owner": 2});
        let crate::protocol::DaemonFrame::Attached(a) = crate::protocol::decode_frame(&a.to_string()).unwrap() else { panic!() };
        t.load_attached(&a).unwrap();
        assert_eq!(t.resolved_outcome(1), None, "an attach starts over (prompt ids may start over)");
    }
}
