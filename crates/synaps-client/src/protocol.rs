//! Hand-written serde mirrors of the Synaps daemon wire protocol (v3).
//!
//! Source of truth: `crates/agent-engine/src/session/wire.rs` and
//! `session/types.rs` in Synaps (Apache-2.0). Only the frames this client
//! needs are mirrored, and every decoder is tolerant: unknown frame types,
//! event kinds and fields decode to an `Unknown` variant (or are ignored)
//! instead of failing the frame, so a newer daemon never breaks the client.
//!
//! Framing: one JSON object per `\n`, at most [`MAX_FRAME_BYTES`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Exact-match protocol version (`wire::PROTOCOL_VERSION`).
pub const PROTOCOL_VERSION: u32 = 3;
/// Daemon frame cap, both directions (`wire::DAEMON_MAX_FRAME_BYTES`).
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// `Welcome.features` entry for owned sessions (newer daemons only).
pub const FEATURE_OWNED_SESSIONS: &str = "owned_sessions";

// ── client → daemon ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Hello(Hello),
    Ping,
    Sessions,
    Attach(Attach),
    Cmd { session_id: String, cmd: SessionCommand },
    Bye,
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
    /// The daemon has no `desktop` kind: `attach` is the interactive thin
    /// client kind (keep-warm treats it like the TUI). `name` says desktop.
    pub kind: &'static str,
    pub terminal: Option<String>,
    pub instance: String,
    pub history: HistoryMode,
    pub tail_items: usize,
    pub capabilities: Capabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMode {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
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

/// The subset of `SessionConfig` a desktop client sets on create.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub auto_approve_confirms: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum SessionCommand {
    Submit { text: String, attachments: Vec<Value> },
    Steer { text: String },
    Cancel,
    Answer { prompt_id: u64, value: Option<String> },
}

// ── daemon → client ───────────────────────────────────────────────────────────

/// A decoded daemon frame. `Unknown` is a frame type this client does not
/// mirror; `Malformed` is a known type whose body did not decode (kept raw).
#[derive(Debug, Clone)]
pub enum DaemonFrame {
    Welcome(Welcome),
    Refused { reason: Value, message: String },
    Pong { pid: u32, uptime_s: u64, sessions: usize },
    SessionList(Vec<SessionMeta>),
    Attached(Box<Attached>),
    Event(Envelope),
    Error { session_id: Option<String>, message: String },
    Bye(Option<ByeReason>),
    Unknown(String),
    Malformed { kind: String, error: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawFrame {
    Welcome(Welcome),
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
        sessions: Vec<SessionMeta>,
    },
    Attached(Box<Attached>),
    Event(Envelope),
    Error {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        message: String,
    },
    Bye {
        #[serde(default)]
        reason: Option<ByeReason>,
    },
}

/// Decode one NDJSON line. Never fails on a well-formed JSON object: an
/// unrecognised `type` is `Unknown`, an undecodable body `Malformed`.
pub fn decode_frame(line: &str) -> Result<DaemonFrame, String> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(format!("frame exceeds {MAX_FRAME_BYTES} bytes"));
    }
    match serde_json::from_str::<RawFrame>(line) {
        Ok(f) => Ok(match f {
            RawFrame::Welcome(w) => DaemonFrame::Welcome(w),
            RawFrame::Refused { reason, message } => DaemonFrame::Refused { reason, message },
            RawFrame::Pong { pid, uptime_s, sessions } => DaemonFrame::Pong { pid, uptime_s, sessions },
            RawFrame::SessionList { sessions } => DaemonFrame::SessionList(sessions),
            RawFrame::Attached(a) => DaemonFrame::Attached(a),
            RawFrame::Event(e) => DaemonFrame::Event(e),
            RawFrame::Error { session_id, message } => DaemonFrame::Error { session_id, message },
            RawFrame::Bye { reason } => DaemonFrame::Bye(reason),
        }),
        Err(e) => {
            #[derive(Deserialize)]
            struct Tag {
                #[serde(rename = "type", default)]
                kind: String,
            }
            let tag: Tag = serde_json::from_str(line).map_err(|e2| format!("not a JSON object: {e2}"))?;
            const KNOWN: &[&str] =
                &["welcome", "refused", "pong", "session_list", "attached", "event", "error", "bye"];
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

#[derive(Debug, Clone, Deserialize)]
pub struct Welcome {
    pub protocol_version: u32,
    #[serde(default)]
    pub daemon_version: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub sessions: Vec<SessionMeta>,
    #[serde(default = "one")]
    pub generation: u64,
    #[serde(default)]
    pub features: Vec<String>,
}

fn one() -> u64 {
    1
}

#[derive(Debug, Clone, Default, Deserialize)]
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
}

impl SessionMeta {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.title.clone().filter(|t| !t.is_empty()))
            .unwrap_or_else(|| self.id.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attached {
    pub client: u64,
    pub meta: SessionMeta,
    #[serde(default)]
    pub view: Value,
    #[serde(default)]
    pub conversation: ConversationSnapshot,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub replay: Vec<Envelope>,
    #[serde(default)]
    pub pending_prompts: Vec<PromptRequest>,
    #[serde(default)]
    pub input_owner: Option<u64>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "bye", rename_all = "snake_case")]
pub enum ByeReason {
    Reloading {
        #[serde(default)]
        generation: u64,
        #[serde(default)]
        retry_after_ms: u64,
    },
    Shutdown,
    Detached,
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
    pub fn decode(&self) -> SessionEvent {
        decode_event(&self.event)
    }
}

pub fn decode_event(v: &Value) -> SessionEvent {
    SessionEvent::deserialize(v).unwrap_or(SessionEvent::Unknown)
}

/// `WireSessionEvent`, the variants a UI acts on. Everything else is `Unknown`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
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
    Conversation {
        #[serde(default)]
        digest: Value,
    },
    Prompt {
        request: PromptRequest,
    },
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
    ClientJoined {
        client: u64,
        #[serde(default)]
        kind: String,
        #[serde(default)]
        name: String,
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
    CompactionStarted,
    CompactionApplied,
    CompactionFailed {
        #[serde(default)]
        message: String,
    },
    SubagentRows {
        #[serde(default)]
        rows: Vec<Value>,
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
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    Llm(LlmEvent),
    Session(SessionStreamEvent),
    Agent(AgentEvent),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "llm", rename_all = "snake_case")]
pub enum LlmEvent {
    Thinking { text: String },
    Text { text: String },
    ToolUseStart { tool_name: String, tool_id: String },
    ToolUseDelta { tool_id: String, delta: String },
    ToolUse { tool_name: String, tool_id: String, #[serde(default)] input: Value },
    ToolResult { tool_id: String, result: String },
    ToolResultDelta { tool_id: String, delta: String },
    ResponseStart,
    ResponseReset,
    ToolQueued { tool_id: String },
    ToolStarted { tool_id: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "session", rename_all = "snake_case")]
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
