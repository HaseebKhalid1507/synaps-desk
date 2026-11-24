//! Hand-written serde mirrors of the Synaps daemon wire protocol (v3).
//!
//! Source of truth: `crates/agent-engine/src/session/wire.rs` and
//! `session/types.rs` in Synaps (Apache-2.0). Only the frames this client
//! needs are mirrored, and every decoder is tolerant: unknown frame types,
//! event kinds and fields decode to an `Unknown` variant (or are ignored)
//! instead of failing the frame, so a newer daemon never breaks the client.
//! A *known* event whose body changed shape is a [`DecodeError`], not a
//! silent `Unknown`, so "new variant" and "shape drift" stay distinguishable.
//!
//! The mirror enums are `#[non_exhaustive]`: match them with a `_` arm.
//!
//! `Debug` on [`ClientFrame`] and [`SessionCommand`] redacts `Submit.text`,
//! `Steer.text` and `Answer.value` (secret prompts carry API keys), the same
//! as Synaps' own `Debug` impls.
//!
//! Framing: one JSON object per `\n`, at most [`MAX_FRAME_BYTES`].

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Exact-match protocol version (`wire::PROTOCOL_VERSION`).
pub const PROTOCOL_VERSION: u32 = 3;
/// Daemon frame cap, both directions (`wire::DAEMON_MAX_FRAME_BYTES`).
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// `Welcome.features` entry for owned sessions (newer daemons only).
pub const FEATURE_OWNED_SESSIONS: &str = "owned_sessions";
/// `Welcome.features` entry: the daemon answers `SavedSessions` (a newer
/// daemon only; an older one would answer `Error{unknown variant}`).
pub const FEATURE_SAVED_SESSIONS: &str = "saved_sessions";
/// Most entries a `SavedSessions` asks for (the client clamps to it).
pub const SAVED_SESSIONS_MAX: u32 = 1000;
/// `Welcome.features` entry: the daemon answers `CatalogPage` (saved
/// sessions as `SessionMeta` rows with `lifecycle: "saved"`, paged; the
/// watch-sessions replacement of `saved_sessions`). An older daemon
/// answers the frame with `Error{unknown variant}`.
pub const FEATURE_SESSION_PAGE: &str = "session_page";
/// `Welcome.features` entry: the daemon answers `Status{cache: true}` with
/// when the session's prompt cache expires (`cache`), to the asking client
/// only. An older daemon is never asked.
pub const FEATURE_CACHE_EXPIRY: &str = "cache_expiry";
/// The most rows one `CatalogPage` returns (`wire::CATALOG_PAGE_MAX_LIMIT`;
/// the client clamps to it).
pub const CATALOG_PAGE_MAX: u32 = 200;
/// The daemon's `Error` for a create whose `cwd` is relative or missing on
/// the daemon's machine (`daemon/conn.rs`: `format!("cwd must be an
/// absolute existing directory: {}", cwd.display())`, then `Bye`). Nothing
/// was created: the check runs before the create.
pub const MISSING_CWD: &str = "cwd must be an absolute existing directory: ";
/// Longest session name (`validate_name`: `[a-z0-9-]{1,40}`).
pub const SESSION_NAME_MAX: usize = 40;
/// Query ids at or above this are the daemon's and the TUI transport's own
/// (`wire::RESERVED_QUERY_ID_BASE`). A client picks its ids below it.
pub const RESERVED_QUERY_ID_BASE: u64 = 1 << 63;
/// `session::RELOADING_REFUSAL`: the `Error` frame text for a command
/// refused while the daemon drains for a reload.
pub const RELOADING_REFUSAL: &str = "daemon reloading; retry in a moment";

// ── client → daemon ───────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientFrame {
    Hello(Hello),
    Ping,
    Sessions,
    Attach(Attach),
    Cmd {
        session_id: String,
        cmd: SessionCommand,
    },
    /// Control fast path: the saved sessions on disk, newest first. Only to
    /// a daemon that lists [`FEATURE_SAVED_SESSIONS`]. Reply: `SavedSessionList`.
    SavedSessions {
        named_only: bool,
        limit: u32,
    },
    /// Control fast path: one page of the saved sessions (not held by the
    /// daemon), newest `updated_at` first. `before` (exclusive) continues
    /// from a reply's `next`. Only to a daemon that lists
    /// [`FEATURE_SESSION_PAGE`]. Reply: `CatalogPage` with the same `req`
    /// (not necessarily in order: the daemon serves pages as tasks).
    CatalogPage {
        req: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<CatalogCursor>,
        limit: u32,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        named_only: bool,
    },
    /// Control fast path: name (`Some`) or unname (`None`) a live or parked
    /// session (`/saveas` rules). `expect`: absent = replace any name,
    /// `Some(None)` = only if unnamed, `Some(Some(x))` = only if named `x`.
    /// Reply: `Renamed`, `RenameFailed` or `Refused{UnknownSession}`. Not
    /// accepted on an attached connection. An older daemon answers
    /// `Error{unknown variant}`.
    Rename {
        session_id: String,
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expect: Option<Option<String>>,
    },
    Bye,
}

impl std::fmt::Debug for ClientFrame {
    /// Frame types only for anything that can carry user text or a secret
    /// (`SessionCommand`'s own `Debug` redacts the bodies).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hello(h) => f.debug_tuple("Hello").field(h).finish(),
            Self::Ping => f.write_str("Ping"),
            Self::Sessions => f.write_str("Sessions"),
            Self::Attach(a) => f.debug_tuple("Attach").field(a).finish(),
            Self::Cmd { session_id, cmd } => f.debug_struct("Cmd").field("session_id", session_id).field("cmd", cmd).finish(),
            Self::SavedSessions { named_only, limit } => {
                f.debug_struct("SavedSessions").field("named_only", named_only).field("limit", limit).finish()
            }
            Self::CatalogPage { req, before, limit, named_only } => f
                .debug_struct("CatalogPage")
                .field("req", req)
                .field("before", before)
                .field("limit", limit)
                .field("named_only", named_only)
                .finish(),
            Self::Rename { session_id, name, expect } => {
                f.debug_struct("Rename").field("session_id", session_id).field("name", name).field("expect", expect).finish()
            }
            Self::Bye => f.write_str("Bye"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub client: ClientMeta,
    pub cwd: PathBuf,
    pub client_version: String,
    /// `None`: the daemon gives created sessions its own environment. The
    /// TUI sends a secret-stripped snapshot; this client does not (yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_of: Option<ClientReconnect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientMeta {
    pub kind: ClientKind,
    pub terminal: Option<String>,
    pub instance: String,
    pub history: HistoryMode,
    pub tail_items: usize,
    pub capabilities: Capabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `types::ClientKind`. The daemon's set is closed: an unknown kind is a
/// malformed `Hello`, so there is no `Desktop`. A GUI is `Attach` (the
/// interactive thin-client kind; keep-warm treats it like the TUI) and says
/// what it is in `ClientMeta.name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientKind {
    Tui,
    Chat,
    Attach,
    Rpc,
    Server,
    Test,
    /// A kind from a newer daemon (decode only; never sent).
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMode {
    #[default]
    Full,
    Digest,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Capabilities {
    pub tool_queue: bool,
    pub owner_reason_input: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientReconnect {
    pub previous_client: u64,
    pub session_id: String,
    pub was_owner: bool,
    pub generation: u64,
}

/// `Mirror` owns input iff nobody does, `Takeover` steals it, `Observe`
/// never owns it (a watcher: it can never become the active client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    #[default]
    Mirror,
    Takeover,
    Observe,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "attach", rename_all = "snake_case")]
pub enum Attach {
    Existing { session_id: String, mode: AttachMode },
    Create { config: CreateConfig, mode: AttachMode },
}

/// The subset of `SessionConfig` a desktop client sets on create. The
/// daemon fills `cwd` from `Hello.cwd` only when it is `None`
/// (`daemon/conn.rs`), and refuses a relative or missing directory.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// `--name`: must be unique among saved sessions (`/saveas` rules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `--continue <id|name>`: resume a saved session (the daemon resolves
    /// ids, names and compaction chains; a live one is attached instead).
    /// `Some(None)` is a bare `--continue` (the most recent); it goes out as
    /// `null`, which the daemon's `double_option` keeps apart from absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_session: Option<Option<String>>,
    pub auto_approve_confirms: bool,
}

#[derive(Clone, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionCommand {
    Submit {
        text: String,
        attachments: Vec<Value>,
    },
    Steer {
        text: String,
    },
    Cancel,
    Answer {
        prompt_id: u64,
        value: Option<String>,
    },
    /// Reply: `QueryResult{id, value}`, broadcast to every attached client,
    /// so pick an id below [`RESERVED_QUERY_ID_BASE`] that others won't.
    Query {
        id: u64,
        query: SessionQuery,
    },
    /// A clean detach (then `Bye`); the turn keeps running.
    Detach {
        client: u64,
    },
    /// Steer one of this session's running workers (`sa_N`) from its watch
    /// view (owner-only like `Steer`). The engine also notes it in the main
    /// conversation (`External`, content type `subagent_steer`); a failure
    /// is `Refused { command: "subagent_steer" }`. Daemons from v0.11.0.
    SubagentSteer {
        handle_id: String,
        text: String,
    },
    /// A runtime-backed slash command (`/workers cancel sa_N`, ...), owner-
    /// only. Reply: `QueryResult{id, value: {"kind": "output"|"error"|...,
    /// "text": ...}}`, broadcast like a query's; a non-owner's is
    /// `Refused { command: "engine_command" }`.
    EngineCommand {
        id: u64,
        name: String,
        arg: String,
    },
    /// Change one of the session's settings (this session only). Reply:
    /// `SettingChanged{applied: {id, ok, message, view}}`, broadcast to
    /// every attached client, so pick an id below [`RESERVED_QUERY_ID_BASE`]
    /// that others won't. Input-owner only: otherwise `Refused{command:
    /// "set"}`.
    Set {
        id: u64,
        setting: Setting,
    },
}

/// `types::SessionSetting`, the variants this client sets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "setting", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Setting {
    /// `try_set_model`: a runtime id (`anthropic/claude-opus-5-5`).
    Model { model: String },
    /// `set_reasoning_level_checked`: `off`, `adaptive`, `low` ... (the
    /// daemon parses it; one it does not know fails the frame's decode).
    ReasoningLevel { level: String },
}

impl Setting {
    /// The wire name (`SettingApplied.setting`, `Refused.command` is `set`).
    pub fn name(&self) -> &'static str {
        match self {
            Setting::Model { .. } => "model",
            Setting::ReasoningLevel { .. } => "reasoning_level",
        }
    }
}

impl std::fmt::Debug for SessionCommand {
    /// `Submit`/`Steer`/`Answer` print `<redacted>`, no lengths (as Synaps).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Submit { attachments, .. } => {
                f.debug_struct("Submit").field("text", &format_args!("<redacted>")).field("attachments", &attachments.len()).finish()
            }
            Self::Steer { .. } => f.debug_struct("Steer").field("text", &format_args!("<redacted>")).finish(),
            Self::Cancel => f.write_str("Cancel"),
            Self::Answer { prompt_id, .. } => {
                f.debug_struct("Answer").field("prompt_id", prompt_id).field("value", &format_args!("<redacted>")).finish()
            }
            Self::Query { id, query } => f.debug_struct("Query").field("id", id).field("query", query).finish(),
            Self::Detach { client } => f.debug_struct("Detach").field("client", client).finish(),
            Self::SubagentSteer { handle_id, .. } => {
                f.debug_struct("SubagentSteer").field("handle_id", handle_id).field("text", &format_args!("<redacted>")).finish()
            }
            Self::EngineCommand { id, name, arg } => {
                f.debug_struct("EngineCommand").field("id", id).field("name", name).field("arg", arg).finish()
            }
            Self::Set { id, setting } => f.debug_struct("Set").field("id", id).field("setting", setting).finish(),
        }
    }
}

/// `types::SessionQuery`, the variants this client asks.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "query", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionQuery {
    /// Reply value: the session's `api_messages` array.
    Messages,
    /// Reply value: the session's status object. `cache`: with when the
    /// prompt cache expires (`cache`), answered to this client only; ask it
    /// only of a daemon whose `Welcome.features` lists
    /// [`FEATURE_CACHE_EXPIRY`] (an off flag is not sent: the old bytes).
    Status {
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        cache: bool,
    },
    /// Reply value: a [`WatchPage`] of worker `handle_id`'s journal from
    /// `since` (the previous page's `next_seq`; 0 = all), or `{"error":
    /// "no subagent <id> in this session"}`. `tool_queue`: include the
    /// queue entries (an off flag is not sent: an older daemon sees the old
    /// bytes). Daemons from v0.11.0; an older one answers
    /// `Error{"malformed frame: unknown variant `subagent_watch`..."}`.
    SubagentWatch {
        handle_id: String,
        since: u64,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        tool_queue: bool,
    },
}

/// One page of a worker's watch journal (`runtime::subagent_watch::WatchPage`,
/// the `SessionQuery::SubagentWatch` reply). Every field defaults, so an
/// older or newer daemon's page still decodes.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct WatchPage {
    pub handle_id: String,
    pub subagent_id: u64,
    pub agent_name: String,
    /// The worker's model; empty from older daemons.
    pub model: String,
    /// `running`, `completed`, `cancelled`, `timed_out` or `failed`.
    pub status: String,
    /// It takes steering now (running, with a steer channel).
    pub steerable: bool,
    /// Ask for `since = next_seq` next time.
    pub next_seq: u64,
    /// Entries before the asked `since` were dropped (the journal keeps 2 MiB).
    pub truncated: bool,
    /// More entries are ready now (this page hit its 1 MiB budget).
    pub more: bool,
    pub events: Vec<WatchEvent>,
    /// When each of `events` was recorded, ms on the journal's clock (same
    /// length, or empty from an older daemon).
    pub at_ms: Vec<u64>,
    /// The journal's clock when the page was read.
    pub now_ms: Option<u64>,
}

impl WatchPage {
    /// The worker is still running (no later page can change that it ended).
    pub fn running(&self) -> bool {
        self.status == "running"
    }
}

/// One entry of a worker's journal (`runtime::subagent_watch::WatchEvent`),
/// in the shape the main transcript applies ([`WatchEvent::to_stream`]).
/// A kind this client does not know is `Unknown`, never a failed page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WatchEvent {
    ResponseStart,
    ResponseReset,
    Thinking {
        #[serde(default)]
        text: String,
    },
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUseStart {
        tool_id: String,
        #[serde(default)]
        tool_name: String,
    },
    ToolUse {
        tool_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResultDelta {
        tool_id: String,
        #[serde(default)]
        delta: String,
    },
    ToolResult {
        tool_id: String,
        #[serde(default)]
        result: String,
    },
    /// Only in pages asked for with `tool_queue: true`.
    ToolQueued {
        tool_id: String,
    },
    ToolStarted {
        tool_id: String,
    },
    /// A steering message the worker took in (from the user or its parent).
    Steered {
        #[serde(default)]
        text: String,
    },
    Done,
    /// It failed: the error's category label only, never its text.
    Failed {
        #[serde(default)]
        category: String,
    },
    #[serde(other)]
    Unknown,
}

impl WatchEvent {
    /// The stream event it was recorded from, to replay through the same
    /// transcript path as the main conversation. `None` for `Steered`,
    /// `Failed` and `Unknown`: [`crate::Transcript::apply_watch`] shows the
    /// first two itself.
    pub fn to_stream(&self) -> Option<StreamEvent> {
        let llm = |l| Some(StreamEvent::Llm(l));
        match self {
            Self::ResponseStart => llm(LlmEvent::ResponseStart),
            Self::ResponseReset => llm(LlmEvent::ResponseReset),
            Self::Thinking { text } => llm(LlmEvent::Thinking { text: text.clone() }),
            Self::Text { text } => llm(LlmEvent::Text { text: text.clone() }),
            Self::ToolUseStart { tool_id, tool_name } => {
                llm(LlmEvent::ToolUseStart { tool_name: tool_name.clone(), tool_id: tool_id.clone() })
            }
            Self::ToolUse { tool_id, tool_name, input } => {
                llm(LlmEvent::ToolUse { tool_name: tool_name.clone(), tool_id: tool_id.clone(), input: input.clone() })
            }
            Self::ToolResultDelta { tool_id, delta } => llm(LlmEvent::ToolResultDelta { tool_id: tool_id.clone(), delta: delta.clone() }),
            Self::ToolResult { tool_id, result } => llm(LlmEvent::ToolResult { tool_id: tool_id.clone(), result: result.clone() }),
            Self::ToolQueued { tool_id } => llm(LlmEvent::ToolQueued { tool_id: tool_id.clone() }),
            Self::ToolStarted { tool_id } => llm(LlmEvent::ToolStarted { tool_id: tool_id.clone() }),
            Self::Done => Some(StreamEvent::Session(SessionStreamEvent::Done)),
            Self::Steered { .. } | Self::Failed { .. } | Self::Unknown => None,
        }
    }
}

// ── daemon → client ───────────────────────────────────────────────────────────

/// A decoded daemon frame. `Unknown` is a frame type this client does not
/// mirror; `Malformed` is a known type whose body did not decode.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DaemonFrame {
    Welcome(Welcome),
    /// `reason` is `RefuseReason` raw (`{"reason":"busy"}`, ...); see
    /// [`refuse_kind`]. The daemon closes after it.
    Refused {
        reason: Value,
        message: String,
    },
    Pong {
        pid: u32,
        uptime_s: u64,
        sessions: usize,
    },
    /// `rejected`: entries that did not decode (skipped, not fatal).
    SessionList {
        sessions: Vec<SessionMeta>,
        rejected: usize,
    },
    Attached(Box<Attached>),
    Event(Envelope),
    Error {
        session_id: Option<String>,
        message: String,
    },
    /// Reply to `SavedSessions`. `rejected`: entries that did not decode.
    SavedSessionList {
        sessions: Vec<SavedSession>,
        truncated: bool,
        rejected: usize,
    },
    /// Reply to `CatalogPage` (same `req`). `sessions`: saved rows, newest
    /// first; `next`: the cursor of the following page, `None` at the end;
    /// `rejected`: rows that did not decode (skipped, the page is kept);
    /// `error`: the page could not be served (`sessions` is empty).
    CatalogPage {
        req: u64,
        sessions: Vec<SessionMeta>,
        next: Option<CatalogCursor>,
        rejected: usize,
        error: Option<String>,
    },
    /// Reply to `Rename`: the session's name is now `name` (`None`: cleared).
    Renamed {
        session_id: String,
        name: Option<String>,
        in_memory_only: bool,
    },
    /// Reply to `Rename`: not renamed.
    RenameFailed(RenameFailure),
    Bye(Option<ByeReason>),
    Unknown(String),
    Malformed {
        kind: String,
        error: String,
    },
}

/// `RefuseReason`'s tag (`busy`, `version`, `unknown_session`, ...), or "".
pub fn refuse_kind(reason: &Value) -> &str {
    reason.get("reason").and_then(Value::as_str).or_else(|| reason.as_str()).unwrap_or("")
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawFrame {
    Welcome(RawWelcome),
    Refused {
        #[serde(default)]
        reason: Value,
        #[serde(default)]
        message: String,
    },
    Pong {
        #[serde(default)]
        pid: u32,
        #[serde(default)]
        uptime_s: u64,
        #[serde(default)]
        sessions: usize,
    },
    SessionList {
        #[serde(default)]
        sessions: Vec<Value>,
    },
    Attached(Box<Attached>),
    Event(Envelope),
    Error {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        message: String,
    },
    SavedSessionList {
        #[serde(default, deserialize_with = "null_vec")]
        sessions: Vec<Value>,
        #[serde(default)]
        truncated: bool,
    },
    CatalogPage {
        #[serde(default)]
        req: u64,
        #[serde(default, deserialize_with = "null_vec")]
        sessions: Vec<Value>,
        #[serde(default)]
        next: Option<CatalogCursor>,
        #[serde(default)]
        error: Option<String>,
    },
    Renamed {
        session_id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        in_memory_only: bool,
    },
    RenameFailed(RenameFailure),
    Bye {
        #[serde(default)]
        reason: Option<ByeReason>,
    },
}

/// Decode each element on its own; return the ones that decoded and how
/// many did not.
pub fn lenient<T: DeserializeOwned>(raw: Vec<Value>) -> (Vec<T>, usize) {
    let n = raw.len();
    let ok: Vec<T> = raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
    let rejected = n - ok.len();
    (ok, rejected)
}

/// A missing or `null` list is empty.
fn null_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Value>, D::Error> {
    Ok(Option::<Vec<Value>>::deserialize(d)?.unwrap_or_default())
}

/// Decode one NDJSON line. Never fails on a well-formed JSON object: an
/// unrecognised `type` is `Unknown`, an undecodable body `Malformed`.
pub fn decode_frame(line: &str) -> Result<DaemonFrame, String> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(format!("frame exceeds {MAX_FRAME_BYTES} bytes"));
    }
    match serde_json::from_str::<RawFrame>(line) {
        Ok(f) => Ok(match f {
            RawFrame::Welcome(w) => DaemonFrame::Welcome(w.into()),
            RawFrame::Refused { reason, message } => DaemonFrame::Refused { reason, message },
            RawFrame::Pong { pid, uptime_s, sessions } => DaemonFrame::Pong { pid, uptime_s, sessions },
            RawFrame::SessionList { sessions } => {
                let (sessions, rejected) = lenient(sessions);
                DaemonFrame::SessionList { sessions, rejected }
            }
            RawFrame::Attached(a) => DaemonFrame::Attached(a),
            RawFrame::Event(e) => DaemonFrame::Event(e),
            RawFrame::Error { session_id, message } => DaemonFrame::Error { session_id, message },
            RawFrame::SavedSessionList { sessions, truncated } => {
                let (sessions, rejected) = lenient(sessions);
                DaemonFrame::SavedSessionList { sessions, truncated, rejected }
            }
            RawFrame::CatalogPage { req, sessions, next, error } => {
                let (sessions, rejected) = lenient(sessions);
                DaemonFrame::CatalogPage { req, sessions, next, rejected, error }
            }
            RawFrame::Renamed { session_id, name, in_memory_only } => DaemonFrame::Renamed { session_id, name, in_memory_only },
            RawFrame::RenameFailed(f) => DaemonFrame::RenameFailed(f),
            RawFrame::Bye { reason } => DaemonFrame::Bye(reason),
        }),
        Err(e) => {
            #[derive(Deserialize)]
            struct Tag {
                #[serde(rename = "type", default)]
                kind: String,
            }
            let tag: Tag = serde_json::from_str(line).map_err(|e2| format!("not a JSON object: {e2}"))?;
            const KNOWN: &[&str] = &[
                "welcome",
                "refused",
                "pong",
                "session_list",
                "attached",
                "event",
                "error",
                "saved_session_list",
                "catalog_page",
                "renamed",
                "rename_failed",
                "bye",
            ];
            if KNOWN.contains(&tag.kind.as_str()) {
                Ok(DaemonFrame::Malformed { kind: tag.kind, error: e.to_string() })
            } else {
                Ok(DaemonFrame::Unknown(tag.kind))
            }
        }
    }
}

pub fn encode_frame(frame: &ClientFrame) -> Result<String, String> {
    let mut s = serde_json::to_string(frame).map_err(|e| e.to_string())?;
    if s.len() + 1 > MAX_FRAME_BYTES {
        return Err(format!("frame exceeds {MAX_FRAME_BYTES} bytes"));
    }
    s.push('\n');
    Ok(s)
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Welcome {
    pub protocol_version: u32,
    pub daemon_version: String,
    pub pid: u32,
    pub profile: Option<String>,
    pub sessions: Vec<SessionMeta>,
    /// `sessions` entries that did not decode (skipped).
    pub rejected_sessions: usize,
    pub generation: u64,
    pub features: Vec<String>,
}

#[derive(Deserialize)]
struct RawWelcome {
    protocol_version: u32,
    #[serde(default)]
    daemon_version: String,
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    sessions: Vec<Value>,
    #[serde(default = "one")]
    generation: u64,
    #[serde(default)]
    features: Vec<String>,
}

impl From<RawWelcome> for Welcome {
    fn from(w: RawWelcome) -> Self {
        let (sessions, rejected_sessions) = lenient(w.sessions);
        Welcome {
            protocol_version: w.protocol_version,
            daemon_version: w.daemon_version,
            pid: w.pid,
            profile: w.profile,
            sessions,
            rejected_sessions,
            generation: w.generation,
            features: w.features,
        }
    }
}

fn one() -> u64 {
    1
}

/// A position in the saved-session order (`wire::CatalogCursor`, today
/// `{updated_at, id}`). Opaque to the client: whatever JSON a reply's
/// `next` is goes back as the next request's `before`, unchanged, so a
/// change of the cursor's shape never makes a page undecodable. Unchanged
/// as a JSON value, not as bytes: built without serde_json's
/// `preserve_order` (the client crate on its own) its keys go back sorted;
/// the desktop's build keeps the daemon's order. The daemon reads it by
/// field, so either is the same cursor; compare it as a value (#28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogCursor(pub Value);

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub lifecycle: String,
    #[serde(default)]
    pub clients: usize,
    #[serde(default)]
    pub input_owner: Option<u64>,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub messages: usize,
    #[serde(default)]
    pub state_since: Option<String>,
    /// Pending prompts (questions, secrets) waiting for an answer.
    #[serde(default)]
    pub awaiting_input: usize,
    /// The conversation's journal id (`conv.session.id`): differs from `id`
    /// after a compaction into a successor. Empty from an older daemon.
    #[serde(default)]
    pub journal_id: String,
    /// The journal is held by another process (`"pid N (kind)"`).
    #[serde(default)]
    pub locked_by: Option<String>,
    /// Every attached client, named. Empty from an older daemon; an entry
    /// that does not decode is skipped (the session stays listed).
    #[serde(default, deserialize_with = "lenient_roster")]
    pub roster: Vec<ClientInfo>,
    /// RFC 3339: when the idle park fires (live, no client, grace armed).
    #[serde(default)]
    pub park_due: Option<String>,
    /// RFC 3339: when a parked session is evicted to history.
    #[serde(default)]
    pub evict_due: Option<String>,
    /// Background workers (subagents) running and not stopping, whatever
    /// `busy` says: a turn that ends can leave them going. Live in
    /// listings (`Welcome.sessions`, `SessionList`); the `Attached`
    /// snapshot's meta is the create-time copy (0). An older daemon omits
    /// it: 0.
    #[serde(default)]
    pub subagents_running: usize,
    /// Background workers as a daemon with the `session_workers` feature
    /// lists them (SynapsCLI #416, which replaces `subagents_running`).
    /// Live in listings only, like `subagents_running`. Missing, `null`, or
    /// a shape this client doesn't read: empty (the session stays listed).
    /// Read the count with [`SessionMeta::workers_running`].
    #[serde(default, deserialize_with = "lenient_workers")]
    pub workers: SessionWorkers,
    /// How the daemon resolved the id this session was created from (a
    /// resume, `--continue`): set at create, so the `Attached` snapshot's
    /// meta carries it (the create-time copy). `None` for a new session and
    /// from an older daemon, and from one whose shape this client doesn't
    /// read (it never fails the attach).
    #[serde(default, deserialize_with = "lenient_continue_info")]
    pub continue_info: Option<ContinueInfo>,
    /// RFC 3339: the conversation's last save. Set on saved rows
    /// (`CatalogPage`); held rows omit it.
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// `SessionMeta.continue_info` (the daemon's `ContinueInfoWire`, v0.11.1):
/// the query a resume asked for and the journal it opened. `resolved_via`
/// `"compacted"`: the query's journal was compacted, and the daemon followed
/// its chain forward to `session_id` (F24). `"chain"` / `"name"`: a chain or
/// a name of that query; `None`: the journal itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ContinueInfo {
    /// The journal id opened.
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub resolved_via: Option<String>,
    /// What was asked for.
    #[serde(default)]
    pub query: String,
    /// The daemon's note when it followed a compaction chain.
    #[serde(default)]
    pub compaction_notice: Option<String>,
}

impl ContinueInfo {
    /// The daemon followed `query`'s compactions to `session_id`.
    pub fn compacted(&self) -> bool {
        self.resolved_via.as_deref() == Some("compacted") && !self.query.is_empty() && !self.session_id.is_empty()
    }
}

/// `SessionMeta.continue_info`: missing, `null` or a shape this client
/// doesn't read is `None` (an object only: serde would read an array as
/// the fields in order).
fn lenient_continue_info<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ContinueInfo>, D::Error> {
    Ok(match Value::deserialize(d)? {
        v @ Value::Object(_) => serde_json::from_value(v).ok(),
        _ => None,
    })
}

/// `SessionMeta.workers` (SynapsCLI #416): the count of workers running.
/// The daemon also sends `rows` and `more`, which this client doesn't read
/// yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct SessionWorkers {
    /// Running and not stopping (a cancelled worker is not counted).
    #[serde(default)]
    pub running: usize,
}

/// `SessionMeta.workers`: an object this client can read, else empty.
fn lenient_workers<'de, D: Deserializer<'de>>(d: D) -> Result<SessionWorkers, D::Error> {
    Ok(match Value::deserialize(d)? {
        v @ Value::Object(_) => serde_json::from_value(v).unwrap_or_default(),
        _ => SessionWorkers::default(),
    })
}

/// `SessionMeta.roster`: missing or `null` is empty, a bad entry is dropped.
fn lenient_roster<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ClientInfo>, D::Error> {
    Ok(lenient(null_vec(d)?).0)
}

impl SessionMeta {
    pub fn label(&self) -> String {
        self.name.clone().or_else(|| self.title.clone().filter(|t| !t.is_empty())).unwrap_or_else(|| self.id.clone())
    }

    pub fn is_parked(&self) -> bool {
        self.lifecycle == "parked"
    }

    /// A saved row (`CatalogPage`): on disk, not held by the daemon. Held
    /// sessions (live, parked) come from `Sessions` and never are.
    pub fn is_saved(&self) -> bool {
        self.lifecycle == "saved"
    }

    /// This saved row as a `saved_sessions` entry (`live: false`), for a
    /// client that already draws those. The id is the journal's.
    pub fn to_saved(&self) -> SavedSession {
        let id = if self.journal_id.is_empty() { self.id.clone() } else { self.journal_id.clone() };
        SavedSession {
            id,
            name: self.name.clone(),
            title: self.title.clone().filter(|t| !t.is_empty()),
            model: Some(self.model.clone()).filter(|m| !m.is_empty()),
            cwd: self.cwd.clone(),
            created_at: Some(self.created_at.clone()).filter(|c| !c.is_empty()),
            updated_at: self.updated_at.clone(),
            message_count: Some(self.messages as u64),
            live: false,
        }
    }

    /// Background workers running, from whichever field the daemon sends:
    /// `workers.running` (SynapsCLI #416, on `dev`) or `subagents_running`
    /// (#410, local builds only). 0 from a daemon that sends neither.
    pub fn workers_running(&self) -> usize {
        self.workers.running.max(self.subagents_running)
    }

    /// Whether `id` (a session or journal id) is this session.
    pub fn is(&self, id: &str) -> bool {
        !id.is_empty() && (self.id == id || self.journal_id == id)
    }
}

/// One saved session (`SavedSessionList.sessions`). `name`, `cwd` and
/// `message_count` may be null; `live`: the daemon holds it now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct SavedSession {
    /// The journal id (`20261110-202033-e5f6`).
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub message_count: Option<u64>,
    #[serde(default)]
    pub live: bool,
}

/// `wire::RenameRefusal`: why a `Rename` was not applied. `NotLive`,
/// `Unsupported` and `Offline` are this client's own (never on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RenameRefusal {
    /// Not `[a-z0-9-]{1,40}`.
    Invalid,
    /// Another saved session has the name (`taken_by`).
    Taken,
    /// A compaction chain has the name.
    ChainConflict,
    /// `expect` did not match: the session has (or no longer has) a name
    /// (`current_name`). To replace it, resend with
    /// `Expect::Named(current_name)` for the same session.
    Named,
    /// The journal is held by another process.
    Locked,
    /// Retry shortly (queue full, daemon reloading).
    Busy,
    /// No answer in time: it may still apply. Re-list before retrying.
    Unknown,
    /// `Refused{UnknownSession}`: no live or parked session with that id.
    NotLive,
    /// The daemon predates `Rename` (`Error{unknown variant}`).
    Unsupported,
    /// Not connected to the daemon: nothing was sent.
    Offline,
    /// Anything else (a failed save, a reason this client does not know).
    #[serde(other)]
    Failed,
}

/// `DaemonFrame::RenameFailed`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct RenameFailure {
    pub session_id: String,
    pub reason: RenameRefusal,
    #[serde(default)]
    pub message: String,
    /// The journal id of the saved session holding the name (`Taken`).
    #[serde(default)]
    pub taken_by: Option<String>,
    /// The session's name when `expect` failed (`Named`).
    #[serde(default)]
    pub current_name: Option<String>,
}

impl RenameFailure {
    pub fn new(session_id: impl Into<String>, reason: RenameRefusal, message: impl Into<String>) -> Self {
        Self { session_id: session_id.into(), reason, message: message.into(), taken_by: None, current_name: None }
    }
}

/// A session name the daemon would refuse (`validate_name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSessionName {
    pub name: String,
    pub why: &'static str,
}

impl std::fmt::Display for InvalidSessionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.name.is_empty() { f.write_str(self.why) } else { write!(f, "invalid name '{}': {}", self.name, self.why) }
    }
}

impl std::error::Error for InvalidSessionName {}

/// agent-core's `validate_name`: 1–40 of lowercase ASCII letters, digits
/// and `-`. Session ids (`20261110-202033-e5f6`) pass too.
pub fn validate_session_name(name: &str) -> Result<(), InvalidSessionName> {
    let bad = |why| Err(InvalidSessionName { name: name.to_string(), why });
    if name.is_empty() {
        return bad("name cannot be empty");
    }
    if name.len() > SESSION_NAME_MAX {
        return bad("must be 40 chars or less");
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return bad("allowed characters are lowercase letters, digits, and '-'");
    }
    Ok(())
}

/// `config::ClientInput`: who may drive a session with several clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientInput {
    /// Every non-observing client may send input; the last one to do so is
    /// the active client.
    Shared,
    /// Only the input owner may; others are refused until they take over.
    Exclusive,
}

impl<'de> Deserialize<'de> for ClientInput {
    /// Absent (an older daemon) or unknown means `Exclusive`: the rule an
    /// older daemon enforces.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(d)?.as_deref() {
            Some("shared") => ClientInput::Shared,
            _ => ClientInput::Exclusive,
        })
    }
}

fn exclusive() -> ClientInput {
    ClientInput::Exclusive
}

/// `types::ClientInfo`: one attached client, as the roster lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClientInfo {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    /// `mirror`, `takeover` or `observe`.
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub attached_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawAttached")]
#[non_exhaustive]
pub struct Attached {
    /// This connection's client id in the session.
    pub client: u64,
    pub meta: SessionMeta,
    pub view: Value,
    pub conversation: ConversationSnapshot,
    pub streaming: bool,
    pub replay: Vec<Envelope>,
    pub pending_prompts: Vec<PromptRequest>,
    pub clients: Vec<(u64, ClientKind)>,
    pub roster: Vec<ClientInfo>,
    pub client_input: ClientInput,
    pub input_owner: Option<u64>,
    /// Entries of the lists above that did not decode (skipped). The client
    /// also reports them as an `Error::Decode`: a skipped prompt is one the
    /// user would otherwise never see.
    pub rejected: Rejected,
    /// Not on the wire: this client's number for the connection the
    /// snapshot came on, new on every (re-)attach and never reused by a
    /// `Client`. Pass it to [`crate::Client::answer_on`] so an answer meant
    /// for these prompts is never sent on a later connection. 0 when the
    /// snapshot did not come from a `Client` (decoded by hand).
    pub connection: u64,
}

/// Per list, how many entries of an `Attached` snapshot did not decode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Rejected {
    pub pending_prompts: usize,
    pub clients: usize,
    pub roster: usize,
}

impl Rejected {
    pub fn total(&self) -> usize {
        self.pending_prompts + self.clients + self.roster
    }
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = [("pending_prompts", self.pending_prompts), ("clients", self.clients), ("roster", self.roster)]
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        write!(f, "{} attach snapshot entries did not decode (skipped)", parts.join(", "))
    }
}

#[derive(Deserialize)]
struct RawAttached {
    client: u64,
    meta: SessionMeta,
    #[serde(default)]
    view: Value,
    #[serde(default)]
    conversation: ConversationSnapshot,
    #[serde(default)]
    streaming: bool,
    #[serde(default)]
    replay: Vec<Envelope>,
    #[serde(default, deserialize_with = "null_vec")]
    pending_prompts: Vec<Value>,
    #[serde(default, deserialize_with = "null_vec")]
    clients: Vec<Value>,
    #[serde(default, deserialize_with = "null_vec")]
    roster: Vec<Value>,
    #[serde(default = "exclusive")]
    client_input: ClientInput,
    #[serde(default)]
    input_owner: Option<u64>,
}

impl From<RawAttached> for Attached {
    fn from(a: RawAttached) -> Self {
        let (pending_prompts, p) = lenient(a.pending_prompts);
        let (clients, c) = lenient(a.clients);
        let (roster, r) = lenient(a.roster);
        Attached {
            client: a.client,
            meta: a.meta,
            view: a.view,
            conversation: a.conversation,
            streaming: a.streaming,
            replay: a.replay,
            pending_prompts,
            clients,
            roster,
            client_input: a.client_input,
            input_owner: a.input_owner,
            rejected: Rejected { pending_prompts: p, clients: c, roster: r },
            connection: 0,
        }
    }
}

impl Attached {
    /// Whether this client could send input **at attach time**, by the
    /// snapshot's `client_input` / `input_owner` alone. It goes stale at
    /// the first `Event::InputOwner`, and it does not know about owned
    /// sessions, which refuse non-owners even when input is shared. Treat
    /// it as a hint; a refusal still comes back as `Error::Refused`.
    pub fn may_drive(&self, mode: AttachMode) -> bool {
        mode != AttachMode::Observe && (self.client_input == ClientInput::Shared || self.input_owner == Some(self.client))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConversationSnapshot {
    #[serde(default)]
    pub header: Value,
    #[serde(default)]
    pub api_messages: Vec<Value>,
    #[serde(default)]
    pub messages_len: usize,
    #[serde(default)]
    pub cost: f64,
    /// `ConversationTokens` (`{input, output, cache_read, cache_creation}`),
    /// as on [`ConversationDigest::tokens`]; `Null` from an older daemon.
    #[serde(default)]
    pub tokens: Value,
    /// Prompt-cache keep-warm ping tokens, already included in `tokens`
    /// (`ConversationTokens`); `Null` from a daemon that predates them.
    #[serde(default)]
    pub keepwarm_tokens: Value,
    /// Events the engine holds, not in the history yet (buffered in a
    /// turn, or kept after a failed turn or compaction until the next
    /// turn's end), as on [`ConversationDigest::pending_events_len`].
    #[serde(default)]
    pub pending_events_len: usize,
    /// The one steer the engine holds for its next round or turn (the
    /// actor's `queued_message`), as on [`ConversationDigest::queued_message`].
    #[serde(default)]
    pub queued_message: Option<String>,
}

/// `wire::ConversationDigest`: what `Conversation` carries on the wire.
/// `messages_hash` is FNV-1a over each message's JSON ([`crate::history::messages_hash`]).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConversationDigest {
    #[serde(default)]
    pub header: Value,
    #[serde(default)]
    pub messages_len: usize,
    #[serde(default)]
    pub messages_hash: u64,
    #[serde(default)]
    pub tokens: Value,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub queued_message: Option<String>,
    /// Events the engine holds, not in the history yet: buffered while a
    /// turn or a compaction runs (flushed at the turn's end), or put back
    /// by a failed turn (`clear_stream`) or kept by a failed compaction,
    /// until the next turn's end.
    #[serde(default)]
    pub pending_events_len: usize,
}

/// What the daemon applied when a prompt resolved (`PromptResolved.outcome`,
/// SynapsCLI #437).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptOutcome {
    /// A Confirm answered yes (or Allow all, where its gate takes it): the
    /// gate opened.
    Allowed,
    /// A Confirm answered anything else, or a Secret answered with nothing:
    /// the gate stayed shut.
    Denied,
    /// A Secret's value, handed to its call.
    Answered,
    /// No answer was applied: the turn or session ended while it waited, or
    /// the waiting call was gone. The call never passed its gate.
    Cancelled,
    /// Nobody was attached by the abandonment deadline: the daemon answered
    /// for them, fail-closed.
    TimedOut,
    /// One this build does not know (a newer daemon's): as if none came.
    #[serde(other)]
    Unknown,
}

impl PromptOutcome {
    /// A raw `prompt_resolved` event's `outcome`. `None`: another event, or
    /// a daemon before SynapsCLI #437 (no `outcome`); a value this build
    /// does not know is `Unknown`.
    pub fn of_event(raw: &Value) -> Option<Self> {
        if raw.get("ev").and_then(Value::as_str) != Some("prompt_resolved") {
            return None;
        }
        serde_json::from_value(raw.get("outcome")?.clone()).ok()
    }
}

/// What a session event says beyond its typed [`SessionEvent`]: fields a
/// variant leaves out so that code building it (struct literals) stays as
/// it is. Decoded with the event ([`EventMeta::of`]); every
/// [`crate::Event::Session`] carries it, and [`crate::Transcript::apply_with`]
/// takes the two together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EventMeta {
    /// A `prompt_resolved`'s `outcome`: what the daemon applied (SynapsCLI
    /// #437). `None` from a daemon before it, and for any other event.
    pub prompt_outcome: Option<PromptOutcome>,
}

impl EventMeta {
    /// The metadata of the raw session event `raw` (its envelope's `ev` and
    /// fields).
    pub fn of(raw: &Value) -> Self {
        Self { prompt_outcome: PromptOutcome::of_event(raw) }
    }
}

/// A `compaction_applied`'s `queued_restored` in the raw event `raw`: the
/// steer held through the compaction, which the daemon appended to the
/// compacted history (the typed variant keeps its three fields). `None`
/// for any other event.
pub fn queued_restored(raw: &Value) -> Option<&str> {
    if raw["ev"] != "compaction_applied" {
        return None;
    }
    raw["queued_restored"].as_str()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptRequest {
    pub id: u64,
    #[serde(default)]
    pub kind: Value,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub prompt: String,
    /// When the daemon raised it (RFC 3339, UTC). Missing or not a string:
    /// `None`, never a failed decode.
    #[serde(default, deserialize_with = "lenient_string")]
    pub raised_at: Option<String>,
    /// An extension tool-call confirm's source and tool (agent-engine
    /// `ConfirmScope`): what "Allow all" would latch. Missing, `null` or a
    /// shape this client doesn't read: `None` (no Allow all offered).
    #[serde(default, deserialize_with = "lenient_scope")]
    pub confirm_scope: Option<ConfirmScope>,
}

impl PromptRequest {
    /// `PromptKind::Secret`: the answer is a credential. Never log it.
    pub fn is_secret(&self) -> bool {
        self.kind.as_str() == Some("secret")
    }

    /// Its scope, if "Allow all" can be offered for it (a confirm with at
    /// least one extension to latch).
    pub fn allow_all(&self) -> Option<&ConfirmScope> {
        self.confirm_scope.as_ref().filter(|s| !s.extensions.is_empty() && !self.is_secret())
    }
}

/// The answer that allows a confirm and latches its extensions for the
/// session (agent-engine `CONFIRM_ANSWER_ALLOW_ALL`); input owner only.
pub const CONFIRM_ANSWER_ALLOW_ALL: &str = "always";

/// `PromptRequest.confirm_scope` (agent-engine `session::types::ConfirmScope`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ConfirmScope {
    /// The extensions whose confirm it resolves: "Allow all" latches these.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// The tool the call targets (display only).
    #[serde(default)]
    pub tool: Option<String>,
    /// Extensions whose `Modify` rewrote the input it approves.
    #[serde(default)]
    pub rewritten_by: Vec<String>,
}

impl ConfirmScope {
    /// `a` or `a, b` (the daemon's `extensions_label`).
    pub fn extensions_label(&self) -> String {
        self.extensions.join(", ")
    }

    /// The daemon's own "Allow all" text (`allow_all_label`).
    pub fn allow_all_label(&self) -> String {
        format!("Allow all from {} this session", self.extensions_label())
    }

    /// What "Allow all" reaches (`allow_all_scope`).
    pub fn allow_all_scope(&self) -> String {
        format!(
            "Allow all: later confirms from {} (any tool) are approved without asking until /new or /confirms reset. Other extensions still ask.",
            self.extensions_label()
        )
    }
}

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        _ => None,
    })
}

fn lenient_scope<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ConfirmScope>, D::Error> {
    Ok(match Value::deserialize(d)? {
        v @ Value::Object(_) => serde_json::from_value(v).ok(),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "bye", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ByeReason {
    Reloading {
        #[serde(default)]
        generation: u64,
        #[serde(default)]
        retry_after_ms: u64,
    },
    Shutdown,
    Detached,
    OwnerLagged {
        #[serde(default)]
        dropped: u64,
    },
    #[serde(other)]
    Unknown,
}

/// `WireEnvelope`. The event stays raw until [`Envelope::decode`], so one
/// event this client cannot read never costs the frame (or the connection).
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    pub session_id: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub ts: String,
    pub event: Value,
}

impl Envelope {
    /// Decode by reference (clones the event's strings; keeps `event`).
    pub fn decode(&self) -> Result<SessionEvent, DecodeError> {
        decode_event(&self.event)
    }

    /// Decode by value: moves the event's strings instead of cloning them.
    pub fn into_event(self) -> Result<SessionEvent, DecodeError> {
        let kind = event_kind(&self.event);
        SessionEvent::deserialize(self.event).map_err(|e| DecodeError { kind, error: e.to_string() })
    }
}

/// A known event (or frame) whose body did not decode: shape drift, not a
/// new variant (new variants decode to `Unknown`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// `event:<ev>` for an event, else the frame `type`.
    pub kind: String,
    pub error: String,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not decode {}: {}", self.kind, self.error)
    }
}

impl std::error::Error for DecodeError {}

fn event_kind(v: &Value) -> String {
    format!("event:{}", v.get("ev").and_then(Value::as_str).unwrap_or("?"))
}

/// `Ok(Unknown)` for an event kind this client does not mirror; `Err` for
/// a known kind whose body no longer matches the mirror.
pub fn decode_event(v: &Value) -> Result<SessionEvent, DecodeError> {
    SessionEvent::deserialize(v).map_err(|e| DecodeError { kind: event_kind(v), error: e.to_string() })
}

/// `WireSessionEvent`, the variants a UI acts on. Everything else is `Unknown`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    Stream {
        event: StreamEvent,
    },
    TurnStarted {
        #[serde(default)]
        turn_baseline: usize,
        #[serde(default)]
        trigger: String,
        #[serde(default)]
        user_text: Option<String>,
    },
    /// After every change to the conversation. A mirror whose length or
    /// hash differs from `digest` has drifted (clear, resume, compaction,
    /// abort repair) and must re-query `Messages`. From [`crate::Client`]
    /// in `Full` mode this passes through at once, so it can run ahead of
    /// the history: when it disagrees, an `Event::History` with the
    /// `Replace` follows. Compare lengths only after that.
    Conversation {
        #[serde(default)]
        digest: ConversationDigest,
    },
    Prompt {
        request: PromptRequest,
    },
    /// What the daemon applied (`outcome`, SynapsCLI #437) is the event's
    /// [`EventMeta`]: this variant keeps its one field, so code that builds
    /// it (the desktop's tests) is unchanged.
    PromptResolved {
        prompt_id: u64,
    },
    Idle,
    Steered {
        #[serde(default)]
        text: String,
        #[serde(default)]
        delivered: bool,
    },
    Dequeued {
        #[serde(default)]
        text: String,
    },
    SystemNotice {
        #[serde(default)]
        text: String,
    },
    /// The reply to a `Query`. Broadcast: check `id`. [`crate::Client`]
    /// never passes these on (it sends no queries of yours; its own resync
    /// arrives as `Event::History`, and other clients' replies and the
    /// daemon's reserved ids are dropped).
    QueryResult {
        id: u64,
        #[serde(default)]
        value: Value,
    },
    ClientJoined {
        client: u64,
        #[serde(default)]
        kind: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        mode: Option<String>,
    },
    ClientLeft {
        client: u64,
    },
    Ended {
        #[serde(default)]
        reason: Value,
    },
    Aborted {
        #[serde(default)]
        context_saved: bool,
    },
    /// `/clear` (`NewSession`): a new, empty conversation. No `MessageHistory`
    /// follows, only a `Conversation` digest.
    Cleared {
        #[serde(default)]
        session_id: String,
    },
    CompactionStarted,
    /// The conversation was replaced by its compacted form. No
    /// `MessageHistory` follows, only a `Conversation` digest.
    CompactionApplied {
        #[serde(default)]
        previous_session_id: String,
        #[serde(default)]
        session_id: String,
        #[serde(default)]
        msg_count: usize,
    },
    CompactionFailed {
        #[serde(default)]
        message: String,
    },
    CompactionCancelled,
    /// A session driver's lease ended (SynapsCLI E-P0): the steers still
    /// in its queue were never sent, and nothing sends them now.
    DriverRevoked {
        #[serde(default)]
        reason: String,
        #[serde(default)]
        undelivered_steering: Vec<String>,
    },
    SubagentRows {
        #[serde(default)]
        rows: Vec<Value>,
    },
    /// `/resume`: another conversation was swapped in. No `MessageHistory`
    /// follows, only a `Conversation` digest.
    Resumed {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        old_id: String,
        #[serde(default)]
        new_id: String,
        #[serde(default)]
        via: Option<String>,
        #[serde(default)]
        clamp_notice: Option<String>,
    },
    Reloading {
        #[serde(default)]
        generation: u64,
        #[serde(default)]
        retry_after_ms: u64,
    },
    AttachRefused {
        #[serde(default)]
        message: String,
    },
    InputOwnerChanged {
        #[serde(default)]
        from: Option<u64>,
        #[serde(default)]
        to: Option<u64>,
        /// `attach`, `takeover`, `owner_detached`, `reload`, `input`.
        #[serde(default)]
        reason: String,
    },
    /// An input command from `client` was refused with no side effect
    /// (exclusive input owned by another client, an unverified context head,
    /// a refused prompt answer, ...). Broadcast: check `client`.
    Refused {
        client: u64,
        #[serde(default)]
        command: String,
        #[serde(default)]
        reason: String,
    },
    Lifecycle {
        #[serde(default)]
        lifecycle: String,
    },
    /// An engine event the session drained (`synaps send`, agent-comms, a
    /// watcher, a worker's completion): [`crate::events`].
    External {
        event: Box<crate::events::ExternalEvent>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StreamEvent {
    Llm(LlmEvent),
    Session(SessionStreamEvent),
    Agent(AgentEvent),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "llm", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LlmEvent {
    Thinking {
        text: String,
    },
    Text {
        text: String,
    },
    ToolUseStart {
        tool_name: String,
        tool_id: String,
    },
    ToolUseDelta {
        tool_id: String,
        delta: String,
    },
    ToolUse {
        tool_name: String,
        tool_id: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_id: String,
        result: String,
    },
    ToolResultDelta {
        tool_id: String,
        delta: String,
    },
    ResponseStart,
    ResponseReset,
    ToolQueued {
        tool_id: String,
    },
    ToolStarted {
        tool_id: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "session", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionStreamEvent {
    MessageHistory {
        #[serde(default)]
        messages: Vec<Value>,
    },
    Usage {
        #[serde(default)]
        input_tokens: u64,
        #[serde(default)]
        output_tokens: u64,
        #[serde(default)]
        model: Option<String>,
    },
    Done,
    Error {
        #[serde(default)]
        message: String,
    },
    Notice {
        #[serde(default)]
        text: String,
        #[serde(default)]
        retry: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "agent", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    SubagentStart {
        subagent_id: u64,
        #[serde(default)]
        agent_name: String,
        #[serde(default)]
        task_preview: String,
    },
    SubagentUpdate {
        subagent_id: u64,
        #[serde(default)]
        agent_name: String,
        #[serde(default)]
        status: String,
    },
    SubagentDone {
        subagent_id: u64,
        #[serde(default)]
        agent_name: String,
        #[serde(default)]
        result_preview: String,
        #[serde(default)]
        duration_secs: f64,
    },
    SteeringDelivered {
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Unknown,
}
