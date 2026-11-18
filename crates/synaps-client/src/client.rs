//! The connection thread: a small current-thread tokio runtime that owns a
//! control connection (status + session list) and at most one attached
//! session connection. The UI talks to it with [`Client`] methods and reads
//! [`Event`]s from an `async_channel` receiver (runtime-agnostic, so a GPUI
//! executor can await it).
//!
//! Cost: each [`Client`] is one OS thread with its own current-thread tokio
//! runtime, plus one control connection (and one more while attached).
//! Fine for an app; share one `Client` (it is cheap to clone) rather than
//! making one per view.
//!
//! Reload handling mirrors the TUI's `SocketTransport::reconnect`: after
//! `Reloading` / `Bye{Reloading}` (or an unexpected EOF) it reconnects with
//! `Hello{reconnect_of}` and `Attach::Existing`, `Takeover` iff this client
//! owned input (never for `Observe`), backing off 100 ms ×2 up to 5 s, for
//! at most 60 s. The first attach rides out a reload the same way.
//!
//! History (`HistoryMode::Full`): the thread keeps a [`HistoryMirror`] and
//! turns every `MessageHistory` into an [`Event::History`] (append or
//! replace; the folded prefix never crosses to the UI). A `Conversation`
//! digest that disagrees with the mirror (clear, resume, compaction, abort
//! repair) triggers a `Query{Messages}`, whose result lands as a
//! `HistoryUpdate::Replace`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::conn::{Conn, Endpoint, Identity, Incompatible};
use crate::history::{HistoryMirror, HistoryUpdate};
use crate::protocol::*;

const BACKOFF_START: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// A connection that stayed up this long resets the backoff; one that drops
/// sooner (accepts, greets, hangs up) keeps backing off instead of spinning.
const STABLE_AFTER: Duration = Duration::from_secs(10);
/// After a daemon reload's goodbye, how long the reconnect keeps to
/// [`RELOAD_RETRY`] instead of backing off: a planned handover is not a
/// failure. Longer than the `retry_after` cap (10 s) plus a boot.
pub const RELOAD_WINDOW: Duration = Duration::from_secs(15);
/// The reconnect's fixed retry inside a reload's window: back within a
/// quarter second of the new daemon answering.
pub const RELOAD_RETRY: Duration = Duration::from_millis(250);

/// The reconnect schedule (the control and session loops'): doubling from
/// 100 ms to 5 s, and back to 100 ms after a connection that stayed up
/// 10 s. A daemon reload's goodbye instead starts [`RELOAD_WINDOW`] of
/// tries every [`RELOAD_RETRY`] (after the daemon's `retry_after`), each
/// new goodbye restarting it; past it, the doubling starts over.
#[derive(Clone, Debug)]
pub struct Backoff {
    next: Duration,
    reload_until: Option<std::time::Instant>,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff { next: BACKOFF_START, reload_until: None }
    }
}

impl Backoff {
    /// A connection that was up for `up_for` ended at `now` (`reload`: on a
    /// daemon reload's goodbye).
    pub fn dropped(&mut self, up_for: Duration, reload: bool, now: std::time::Instant) {
        if up_for >= STABLE_AFTER {
            self.next = BACKOFF_START;
        }
        if reload {
            self.reload_until = Some(now + RELOAD_WINDOW);
        }
    }

    /// How long to wait at `now` before the next try (`after`: what the
    /// daemon asked for, if anything); the schedule steps on.
    pub fn wait(&mut self, after: Duration, now: std::time::Instant) -> Duration {
        if let Some(until) = self.reload_until {
            if now < until {
                return after.max(RELOAD_RETRY);
            }
            self.reload_until = None;
            self.next = BACKOFF_START;
        }
        let d = after.max(self.next);
        self.next = (self.next * 2).min(BACKOFF_MAX);
        d
    }
}
/// The reason of a plain detach.
const DETACHED: &str = "detached";
/// The hint when an attempt to create a session was cut short.
const MAY_EXIST: &str = "while the session was being created; it may exist now (see the session list)";
const RECONNECT_BUDGET: Duration = Duration::from_secs(60);
/// `transport::ATTACH_TIMEOUT` / `ATTACH_TIMEOUT_PARKED` (unpark ≤ 20 s).
const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const ATTACH_TIMEOUT_PARKED: Duration = Duration::from_secs(25);
/// How long a detach (`Detach` + `Bye`) may take before the task is aborted.
const DETACH_GRACE: Duration = Duration::from_secs(1);

/// How to build a [`Client`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ClientOptions {
    pub endpoint: Endpoint,
    pub identity: Identity,
    /// Poll `Sessions` this often. `None` (the default): only on
    /// [`Client::list_sessions`], on connect and after each attach.
    pub poll_sessions: Option<Duration>,
}

impl ClientOptions {
    pub fn new(endpoint: Endpoint, identity: Identity) -> Self {
        Self { endpoint, identity, poll_sessions: None }
    }

    pub fn poll_sessions(mut self, every: Duration) -> Self {
        self.poll_sessions = Some(every.max(Duration::from_millis(250)));
        self
    }
}

/// The working directory a created session's tools run in. The daemon uses
/// `Hello.cwd` (this process's cwd) only when none is given, which for a GUI
/// is `/` or wherever the launcher ran it; so a create always says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cwd {
    /// The user's home directory (`$HOME`, else `%USERPROFILE%`).
    #[default]
    Home,
    /// This process's current directory (a CLI-like consumer).
    Process,
    /// This directory: absolute and existing, or the daemon refuses the create.
    Path(PathBuf),
}

impl Cwd {
    pub fn resolve(&self) -> Option<PathBuf> {
        match self {
            Cwd::Home => crate::discovery::home_dir(),
            Cwd::Process => std::env::current_dir().ok(),
            Cwd::Path(p) => Some(p.clone()),
        }
    }
}

/// What [`Client::create`] creates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreateOptions {
    pub mode: AttachMode,
    pub cwd: Cwd,
    pub model: Option<String>,
    /// Name the session at create (unique among saved sessions). Also makes
    /// a create retry-safe: if the connection drops after the create was
    /// sent, the retry attaches to the session of that name instead of
    /// making a second one. Without a name such a create fails (`Detached`)
    /// rather than risk a duplicate.
    pub name: Option<String>,
    /// Resume this saved session (id or name) instead of starting a new
    /// one: `continue_session`. See [`Client::resume`].
    pub resume: Option<String>,
    /// If the daemon refuses `cwd` because that folder is gone on its
    /// machine ([`MISSING_CWD`]), create once more in this one and say so
    /// (`Event::CwdFallback`). The folder is never checked locally: with a
    /// forwarded socket it lives on another machine.
    pub fallback_cwd: Option<Cwd>,
}

impl CreateOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn mode(mut self, mode: AttachMode) -> Self {
        self.mode = mode;
        self
    }
    pub fn cwd(mut self, cwd: Cwd) -> Self {
        self.cwd = cwd;
        self
    }
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn resume(mut self, id_or_name: impl Into<String>) -> Self {
        self.resume = Some(id_or_name.into());
        self
    }
    pub fn fallback_cwd(mut self, cwd: Cwd) -> Self {
        self.fallback_cwd = Some(cwd);
        self
    }

    pub fn config(&self) -> CreateConfig {
        CreateConfig {
            model_override: self.model.clone(),
            cwd: self.cwd.resolve(),
            name: self.name.clone(),
            continue_session: self.resume.clone().map(Some),
            auto_approve_confirms: false,
        }
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub enum Command {
    RefreshSessions,
    Attach {
        session_id: String,
        mode: AttachMode,
    },
    Create(CreateOptions),
    Submit {
        text: String,
    },
    Steer {
        text: String,
    },
    Cancel,
    Answer {
        prompt_id: u64,
        value: Option<String>,
    },
    /// `Answer`, only on connection `connection` (`Attached::connection`):
    /// after a reconnect it is dropped, never sent, and refused back
    /// (`Error::Refused { command: "answer" }`). A restarted daemon numbers
    /// its prompts from 1 again, so the same id may be another prompt.
    AnswerOn {
        connection: u64,
        prompt_id: u64,
        value: Option<String>,
    },
    /// Re-fetch the whole history (`Query{Messages}`) → `HistoryUpdate::Replace`.
    Resync,
    /// `SavedSessions` on the control connection → `Event::SavedSessions`.
    SavedSessions {
        named_only: bool,
        limit: u32,
    },
    /// `CatalogPage` on the control connection → `Event::CatalogPage` /
    /// `CatalogPageFailed` with the same `req`. See [`Client::catalog_page`].
    CatalogPage {
        req: u64,
        before: Option<CatalogCursor>,
        limit: u32,
        named_only: bool,
    },
    /// `Rename` on the control connection → `Event::Renamed` / `RenameFailed`.
    Rename {
        session_id: String,
        name: Option<String>,
        expect: Option<Option<String>>,
    },
    Detach,
    Shutdown,
    /// One page of worker `handle_id`'s watch journal from `since`
    /// (`SessionQuery::SubagentWatch`, with the queue entries) →
    /// `Event::SubagentWatch`. See [`Client::watch_subagent`].
    SubagentWatch {
        handle_id: String,
        since: u64,
    },
    /// Steer a running worker (`SessionCommand::SubagentSteer`). See
    /// [`Client::steer_subagent`].
    SubagentSteer {
        handle_id: String,
        text: String,
    },
    /// Cancel a running worker (`/workers cancel sa_N`, an engine
    /// command). See [`Client::cancel_worker`].
    CancelWorker {
        handle_id: String,
    },
    /// A slash command the engine runs (`SessionCommand::EngineCommand`,
    /// `/compact`, `/context`, ...). See [`Client::engine_command`].
    EngineCommand {
        name: String,
        arg: String,
    },
}

impl Command {
    /// The daemon's name for it (`actor::command_name`), or ours.
    pub fn name(&self) -> &'static str {
        match self {
            Command::RefreshSessions => "sessions",
            Command::Attach { .. } => "attach",
            Command::Create(_) => "create",
            Command::Submit { .. } => "submit",
            Command::Steer { .. } => "steer",
            Command::Cancel => "cancel",
            Command::Answer { .. } | Command::AnswerOn { .. } => "answer",
            Command::Resync => "query",
            Command::SavedSessions { .. } => "saved_sessions",
            Command::CatalogPage { .. } => "catalog_page",
            Command::Rename { .. } => "rename",
            Command::Detach => "detach",
            Command::Shutdown => "shutdown",
            Command::SubagentWatch { .. } => "subagent_watch",
            Command::SubagentSteer { .. } => "subagent_steer",
            Command::CancelWorker { .. } | Command::EngineCommand { .. } => "engine_command",
        }
    }
}

impl std::fmt::Debug for Command {
    /// `Submit`/`Steer` text and `Answer` values print `<redacted>`: a
    /// secret prompt's answer is a credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Attach { session_id, mode } => f.debug_struct("Attach").field("session_id", session_id).field("mode", mode).finish(),
            Command::Create(o) => f.debug_tuple("Create").field(o).finish(),
            Command::Submit { .. } => f.debug_struct("Submit").field("text", &format_args!("<redacted>")).finish(),
            Command::Steer { .. } => f.debug_struct("Steer").field("text", &format_args!("<redacted>")).finish(),
            Command::Answer { prompt_id, .. } => {
                f.debug_struct("Answer").field("prompt_id", prompt_id).field("value", &format_args!("<redacted>")).finish()
            }
            Command::AnswerOn { connection, prompt_id, .. } => f
                .debug_struct("AnswerOn")
                .field("connection", connection)
                .field("prompt_id", prompt_id)
                .field("value", &format_args!("<redacted>"))
                .finish(),
            Command::SavedSessions { named_only, limit } => {
                f.debug_struct("SavedSessions").field("named_only", named_only).field("limit", limit).finish()
            }
            Command::CatalogPage { req, before, limit, named_only } => f
                .debug_struct("CatalogPage")
                .field("req", req)
                .field("before", before)
                .field("limit", limit)
                .field("named_only", named_only)
                .finish(),
            Command::Rename { session_id, name, expect } => {
                f.debug_struct("Rename").field("session_id", session_id).field("name", name).field("expect", expect).finish()
            }
            Command::SubagentWatch { handle_id, since } => {
                f.debug_struct("SubagentWatch").field("handle_id", handle_id).field("since", since).finish()
            }
            Command::SubagentSteer { handle_id, .. } => {
                f.debug_struct("SubagentSteer").field("handle_id", handle_id).field("text", &format_args!("<redacted>")).finish()
            }
            Command::CancelWorker { handle_id } => f.debug_struct("CancelWorker").field("handle_id", handle_id).finish(),
            Command::EngineCommand { name, .. } => f.debug_struct("EngineCommand").field("name", name).finish(),
            other => f.write_str(match other {
                Command::RefreshSessions => "RefreshSessions",
                Command::Cancel => "Cancel",
                Command::Resync => "Resync",
                Command::Detach => "Detach",
                _ => "Shutdown",
            }),
        }
    }
}

/// `Status::Reconnecting`'s `why` when the daemon said goodbye for a reload
/// (a planned handover: the sessions come back), not a lost transport.
pub const RELOADING: &str = "daemon reloading";

/// The control connection's health: the one source of connection status.
/// Attachment state travels separately (`Attached` / `Reattaching` /
/// `Detached`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Status {
    Connecting,
    /// `features`: the daemon's `Welcome.features` (e.g.
    /// [`FEATURE_SAVED_SESSIONS`]); empty from an older daemon.
    Connected {
        daemon_version: String,
        profile: Option<String>,
        pid: u32,
        features: Vec<String>,
    },
    Reconnecting {
        why: String,
    },
    Offline(String),
    /// The daemon speaks another protocol. Not retried by itself:
    /// [`Client::list_sessions`] tries again (after a daemon upgrade).
    Incompatible(Incompatible),
}

/// What went wrong, typed so a consumer can tell a refusal from a transport
/// failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The command was not carried out; nothing happened. From the session
    /// (`Refused` addressed to this client), a daemon `Error` frame such as
    /// "refused: ..." or a reload drain, or this client while it is not
    /// connected. `command` is the daemon's name (`submit`, `steer`,
    /// `cancel`, `answer`, ...) when known. Input that was refused should go
    /// back to the user.
    Refused { command: Option<String>, reason: String },
    /// A daemon `Error` frame that is not a refusal.
    Daemon { session_id: Option<String>, message: String },
    /// The connection failed (it is being re-established).
    Transport(String),
    /// A known frame or event did not decode (shape drift).
    Decode(DecodeError),
    /// A session command with no session attached.
    NotAttached { command: &'static str },
    /// The daemon does not list this `Welcome.features` entry; nothing was
    /// sent.
    Unsupported { feature: &'static str },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused { command: Some(c), reason } => write!(f, "{c} refused: {reason}"),
            Error::Refused { command: None, reason } => write!(f, "refused: {reason}"),
            Error::Daemon { message, .. } => f.write_str(message),
            Error::Transport(e) => write!(f, "connection: {e}"),
            Error::Decode(e) => e.fmt(f),
            Error::NotAttached { command } => write!(f, "{command}: not attached to a session"),
            Error::Unsupported { feature } => write!(f, "this daemon does not support {feature}"),
        }
    }
}

impl std::error::Error for Error {}

/// `attach_seq` ties attachment events to the [`Client::attach`] /
/// [`Client::create`] call that returned it: anything carrying an older
/// one is stale (a session the user has left).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    Status(Status),
    Sessions(Vec<SessionMeta>),
    /// A (re-)attach: the full mirror. Replace, never merge. `mode` is the
    /// mode that was asked for, also after a reload (where an owner
    /// re-attaches with `Takeover` on the wire to keep input). If any of
    /// `pending_prompts` / `clients` / `roster` did not decode, an
    /// `Error::Decode` follows (see `Attached::rejected`).
    Attached {
        attach_seq: u64,
        attached: Box<Attached>,
        mode: AttachMode,
        reconnect: bool,
    },
    /// The session connection dropped (reload, EOF) and is being re-made;
    /// an `Attached { reconnect: true }` or a `Detached` follows. Commands
    /// meanwhile come back as `Error::Refused`.
    Reattaching {
        attach_seq: u64,
        session_id: String,
        reason: String,
    },
    /// A live event, decoded, with the raw wire event alongside (for fields
    /// and variants the mirror does not carry). `MessageHistory` never
    /// comes here in `Full` mode: see [`Event::History`].
    ///
    /// `#[non_exhaustive]`: build it with [`Event::session`] (and
    /// [`Event::with_ts`]), match it with `..`; a field added later breaks
    /// neither.
    #[non_exhaustive]
    Session {
        attach_seq: u64,
        session_id: String,
        seq: u64,
        /// When the daemon sent it (its envelope's `ts`, RFC 3339; empty
        /// if it had none): the time of the rows it starts
        /// ([`crate::transcript::Transcript::apply_at`]).
        ts: String,
        event: SessionEvent,
        raw: Arc<Value>,
    },
    /// The history changed (`Full` mode).
    History {
        attach_seq: u64,
        session_id: String,
        seq: u64,
        update: HistoryUpdate,
    },
    /// Who owns input changed. `client` is this connection's id.
    InputOwner {
        attach_seq: u64,
        session_id: String,
        client: u64,
        owner: Option<u64>,
        reason: String,
    },
    /// The attachment ended for good (ended, refused, gave up, detached,
    /// replaced). Session commands now fail with `Error::NotAttached`.
    Detached {
        attach_seq: u64,
        session_id: Option<String>,
        reason: String,
    },
    Error {
        attach_seq: Option<u64>,
        error: Error,
    },
    /// Reply to [`Client::saved_sessions`]. `truncated`: there are more
    /// than the limit; `rejected`: entries that did not decode (skipped).
    SavedSessions {
        sessions: Vec<SavedSession>,
        truncated: bool,
        rejected: usize,
    },
    /// A [`Client::saved_sessions`] that got no list: the daemon's `Error`,
    /// a dropped connection, or not connected. (A daemon without the
    /// feature answers `Error::Unsupported` instead.)
    SavedSessionsFailed {
        reason: String,
    },
    /// Reply to [`Client::catalog_page`] (its `req`). `sessions`: saved
    /// rows (`lifecycle: "saved"`), newest `updated_at` first; `next`: pass
    /// it as `before` for the following page, `None` at the end;
    /// `rejected`: rows that did not decode (skipped, the page is kept).
    CatalogPage {
        req: u64,
        sessions: Vec<SessionMeta>,
        next: Option<CatalogCursor>,
        rejected: usize,
    },
    /// A [`Client::catalog_page`] that got no page: the daemon's `error`
    /// (an unreadable sessions dir, too many pages in flight), a dropped
    /// connection, or not connected.
    CatalogPageFailed {
        req: u64,
        reason: String,
    },
    /// The daemon refused the create's `cwd` (gone on its machine) and the
    /// create was sent again in [`CreateOptions::fallback_cwd`]. `missing`:
    /// the folder as the daemon printed it; `cwd`: where it runs now.
    CwdFallback {
        attach_seq: u64,
        missing: String,
        cwd: Option<PathBuf>,
    },
    /// Reply to [`Client::rename`]: `session_id` is now called `name`.
    Renamed {
        session_id: String,
        name: Option<String>,
        in_memory_only: bool,
    },
    /// Reply to [`Client::rename`]: not renamed (see [`RenameRefusal`]).
    RenameFailed(RenameFailure),
    /// Reply to [`Client::watch_subagent`] for worker `handle_id` from
    /// `since` (as asked: a late reply to an earlier ask can be told apart).
    SubagentWatch {
        attach_seq: u64,
        session_id: String,
        handle_id: String,
        since: u64,
        reply: WatchReply,
    },
    /// Reply to [`Client::cancel_worker`] for worker `handle_id`: `Ok` with
    /// the daemon's text (`Cancelled sa_3 (they stop at their next step;
    /// …)`: its rows say `cancel_requested`, then `cancelled`), or `Err`
    /// with why not: the daemon's (`sa_3 is already completed — nothing to
    /// cancel`, a refusal: not this client's input), or this client's (not
    /// connected, the connection dropped before the daemon answered).
    WorkerCancel {
        attach_seq: u64,
        handle_id: String,
        result: Result<String, String>,
    },
    /// What became of [`Client::engine_command`] `name` ([`CommandReply`]).
    EngineReply {
        attach_seq: u64,
        name: String,
        reply: CommandReply,
    },
}

/// What became of a slash command ([`Event::EngineReply`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandReply {
    /// Done: the daemon's text (`output`, `notice`), empty when it prints
    /// nothing (`none`: `/compact` was asked; whether it started, the
    /// session's log says).
    Done(String),
    /// Not run, and why: the daemon's error (a usage line, an unknown
    /// worker), a command it does not run, a refusal (not this client's
    /// input), not connected, or `/model x` answered with another model in
    /// use (the daemon keeps the old one when it cannot switch).
    NotRun(String),
    /// Sent, and no answer this client can read: it may have run (the link
    /// dropped, the session ended, a reply of a kind this client does not
    /// know).
    Unknown(String),
}

/// What a [`Client::watch_subagent`] got back.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WatchReply {
    Page(Box<WatchPage>),
    /// The session has no such worker: unknown, or finished longer ago than
    /// the daemon keeps a journal (15 minutes). The daemon's text.
    Gone(String),
    /// The daemon predates worker watch (before v0.11.0): it could not
    /// decode the query. Asking again is pointless until it is replaced.
    Unsupported,
    /// The reply did not decode (shape drift); an `Error::Decode` came too.
    Undecodable(String),
}

impl Event {
    /// An [`Event::Session`] sent at no known time (`ts` empty).
    pub fn session(attach_seq: u64, session_id: impl Into<String>, seq: u64, event: SessionEvent, raw: Arc<Value>) -> Self {
        Event::Session { attach_seq, session_id: session_id.into(), seq, ts: String::new(), event, raw }
    }

    /// This [`Event::Session`] sent at `ts` (its envelope's, RFC 3339). Any
    /// other event is returned as it is.
    pub fn with_ts(mut self, ts: impl Into<String>) -> Self {
        if let Event::Session { ts: t, .. } = &mut self {
            *t = ts.into();
        }
        self
    }

    /// The attachment this event belongs to, if it is an attachment event.
    pub fn attach_seq(&self) -> Option<u64> {
        match self {
            Event::Attached { attach_seq, .. }
            | Event::Reattaching { attach_seq, .. }
            | Event::Session { attach_seq, .. }
            | Event::History { attach_seq, .. }
            | Event::InputOwner { attach_seq, .. }
            | Event::CwdFallback { attach_seq, .. }
            | Event::SubagentWatch { attach_seq, .. }
            | Event::WorkerCancel { attach_seq, .. }
            | Event::EngineReply { attach_seq, .. }
            | Event::Detached { attach_seq, .. } => Some(*attach_seq),
            Event::Error { attach_seq, .. } => *attach_seq,
            Event::Status(_)
            | Event::Sessions(_)
            | Event::SavedSessions { .. }
            | Event::SavedSessionsFailed { .. }
            | Event::CatalogPage { .. }
            | Event::CatalogPageFailed { .. }
            | Event::Renamed { .. }
            | Event::RenameFailed(_) => None,
        }
    }
}

/// The connection thread has stopped (shut down, or it panicked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientClosed;

impl std::fmt::Display for ClientClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the synaps connection thread has stopped")
    }
}

impl std::error::Error for ClientClosed {}

/// The compare-and-set precondition of [`Client::rename`] (`expect`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Expect {
    /// Only if the session has no name (`expect: null`).
    Unnamed,
    /// Only if it is still named this (`expect: "kite"`): replacing a name
    /// after a `RenameRefusal::Named`, or clearing one.
    Named(String),
}

/// Why a control request ([`Client::saved_sessions`], [`Client::rename`])
/// was not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestError {
    Closed,
    /// No control connection yet (or now): the daemon's features are unknown.
    NotConnected,
    /// The connected daemon does not list this feature.
    Unsupported {
        feature: &'static str,
    },
    InvalidName(InvalidSessionName),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Closed => ClientClosed.fmt(f),
            RequestError::NotConnected => f.write_str("not connected to the daemon"),
            RequestError::Unsupported { feature } => write!(f, "this daemon does not support {feature}"),
            RequestError::InvalidName(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RequestError {}

impl From<ClientClosed> for RequestError {
    fn from(_: ClientClosed) -> Self {
        RequestError::Closed
    }
}

struct Msg {
    cmd: Command,
    attach_seq: u64,
}

/// Handle to the connection thread. Cheap to clone; dropping every clone
/// (or [`Client::shutdown`]) detaches cleanly and stops the thread.
#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Msg>,
    /// The last `attach_seq`. Held across the send, so clones on several
    /// threads enqueue attaches in `attach_seq` order.
    seq: Arc<Mutex<u64>>,
    /// The control connection's `Welcome.features`; `None` while it is down.
    features: Features,
    /// The last `CatalogPage.req` handed out.
    reqs: Arc<AtomicU64>,
}

type Features = Arc<Mutex<Option<Vec<String>>>>;

impl Client {
    /// Start the connection thread. Fails only if the thread or its runtime
    /// cannot be created.
    pub fn spawn(options: ClientOptions) -> std::io::Result<(Client, async_channel::Receiver<Event>)> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let (etx, erx) = async_channel::unbounded();
        let features: Features = Arc::default();
        let f2 = features.clone();
        std::thread::Builder::new().name("synaps-conn".into()).spawn(move || rt.block_on(run(options, rx, etx, f2)))?;
        Ok((Client { tx, seq: Arc::new(Mutex::new(0)), features, reqs: Arc::default() }, erx))
    }

    /// Discover the daemon for `profile` (`None`: `$SYNAPS_PROFILE`, else
    /// the default daemon) and connect. No session polling.
    pub fn connect(profile: Option<&str>, identity: Identity) -> std::io::Result<(Client, async_channel::Receiver<Event>)> {
        Self::spawn(ClientOptions::new(Endpoint::profile(profile), identity))
    }

    fn post(&self, cmd: Command) -> Result<u64, ClientClosed> {
        let mut seq = self.seq.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(cmd, Command::Attach { .. } | Command::Create(_)) {
            *seq += 1;
        }
        self.tx.send(Msg { cmd, attach_seq: *seq }).map_err(|_| ClientClosed)?;
        Ok(*seq)
    }

    /// Queue a command. `Err` only when the thread is gone; what the daemon
    /// makes of it comes back as events (`Error::Refused`, ...).
    pub fn send(&self, cmd: Command) -> Result<(), ClientClosed> {
        self.post(cmd).map(drop)
    }

    /// Ask for a fresh `Event::Sessions` now.
    pub fn list_sessions(&self) -> Result<(), ClientClosed> {
        self.send(Command::RefreshSessions)
    }

    /// Attach to a live session (replacing any current attachment). Returns
    /// the `attach_seq` its events will carry.
    pub fn attach(&self, session_id: impl Into<String>, mode: AttachMode) -> Result<u64, ClientClosed> {
        self.post(Command::Attach { session_id: session_id.into(), mode })
    }

    /// Create a session and attach to it. Returns the `attach_seq`.
    pub fn create(&self, options: CreateOptions) -> Result<u64, ClientClosed> {
        self.post(Command::Create(options))
    }

    /// Resume a saved session by journal id or name (`Attach::Create` with
    /// `continue_session`), and attach to it; works on every v3 daemon. A
    /// session that is live in the daemon is attached instead (its `cwd`
    /// then does not apply). Pass the saved folder as `options.cwd`: the
    /// daemon does not restore it from the journal. Failures ("no session
    /// matching 'x'", a journal live in another process) come back as
    /// `Event::Detached { reason }` for the returned `attach_seq`.
    ///
    /// The daemon resolves `id_or_name` as a chain, a name, then **any id
    /// containing it**: to open a name exactly, resolve it to an id first
    /// ([`Client::saved_sessions`]), or check the attached session's name.
    /// A resume is idempotent on the daemon (a live target is attached), so
    /// a dropped or slow one (25 s budget: a long journal loads first) is
    /// sent again rather than given up.
    pub fn resume(&self, id_or_name: impl Into<String>, options: CreateOptions) -> Result<u64, ClientClosed> {
        self.post(Command::Create(options.resume(id_or_name)))
    }

    /// The connected daemon's `Welcome.features`; `None` while the control
    /// connection is down.
    pub fn features(&self) -> Option<Vec<String>> {
        self.features.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Whether the connected daemon lists `feature`.
    pub fn supports(&self, feature: &str) -> bool {
        self.features().is_some_and(|f| f.iter().any(|x| x == feature))
    }

    /// Ask for the saved sessions (`named_only`: only named ones), newest
    /// first, at most `limit` (clamped to 1..=[`SAVED_SESSIONS_MAX`]) →
    /// `Event::SavedSessions` or `Event::SavedSessionsFailed`. Only a daemon that
    /// lists [`FEATURE_SAVED_SESSIONS`] is asked: otherwise
    /// `Err(Unsupported)` (or `NotConnected` while its features are
    /// unknown), and nothing is sent. The connection thread checks again
    /// (a reconnect to another daemon) and answers `Error::Unsupported`.
    pub fn saved_sessions(&self, named_only: bool, limit: u32) -> Result<(), RequestError> {
        let limit = limit.clamp(1, SAVED_SESSIONS_MAX);
        match self.features() {
            None => Err(RequestError::NotConnected),
            Some(f) if !f.iter().any(|x| x == FEATURE_SAVED_SESSIONS) => Err(RequestError::Unsupported { feature: FEATURE_SAVED_SESSIONS }),
            Some(_) => Ok(self.send(Command::SavedSessions { named_only, limit })?),
        }
    }

    /// Ask for one page of the saved sessions (the daemon does not hold
    /// them), newest `updated_at` first: from the newest (`before: None`) or
    /// after a reply's `next`; at most `limit` rows (clamped to
    /// 1..=[`CATALOG_PAGE_MAX`]); `named_only`: only named ones. Returns the
    /// `req` the reply carries: `Event::CatalogPage` or
    /// `Event::CatalogPageFailed`. Replies to several pages in flight can
    /// come in any order. Only a daemon that lists [`FEATURE_SESSION_PAGE`]
    /// is asked: otherwise `Err(Unsupported)` (or `NotConnected` while its
    /// features are unknown), and nothing is sent. A cursor is only good on
    /// the connection that gave it out: after a reconnect, page again from
    /// `before: None`.
    pub fn catalog_page(&self, before: Option<CatalogCursor>, limit: u32, named_only: bool) -> Result<u64, RequestError> {
        let limit = limit.clamp(1, CATALOG_PAGE_MAX);
        match self.features() {
            None => Err(RequestError::NotConnected),
            Some(f) if !f.iter().any(|x| x == FEATURE_SESSION_PAGE) => Err(RequestError::Unsupported { feature: FEATURE_SESSION_PAGE }),
            Some(_) => {
                let req = self.reqs.fetch_add(1, Ordering::Relaxed) + 1;
                self.send(Command::CatalogPage { req, before, limit, named_only })?;
                Ok(req)
            }
        }
    }

    /// Name (`Some`) or unname (`None`) the live or parked session
    /// `session_id`, over the control connection (no attach needed) →
    /// `Event::Renamed` or `Event::RenameFailed`. The name is checked first
    /// (`[a-z0-9-]{1,40}`). Always compare-and-set: the daemon renames only
    /// if the session's name is still what `expect` says, else it answers
    /// `RenameRefusal::Named` with the `current_name`. Start with
    /// [`Expect::Unnamed`]; to replace a name the user has seen, send
    /// [`Expect::Named`] with it (a name changed meanwhile is refused again,
    /// never overwritten blindly).
    pub fn rename(&self, session_id: impl Into<String>, name: Option<&str>, expect: Expect) -> Result<(), RequestError> {
        if let Some(n) = name {
            validate_session_name(n).map_err(RequestError::InvalidName)?;
        }
        let expect = Some(match expect {
            Expect::Unnamed => None,
            Expect::Named(n) => Some(n),
        });
        Ok(self.send(Command::Rename { session_id: session_id.into(), name: name.map(str::to_string), expect })?)
    }

    pub fn submit(&self, text: impl Into<String>) -> Result<(), ClientClosed> {
        self.send(Command::Submit { text: text.into() })
    }

    /// Text typed while a turn runs (`Steer`).
    pub fn steer(&self, text: impl Into<String>) -> Result<(), ClientClosed> {
        self.send(Command::Steer { text: text.into() })
    }

    /// Abort the running turn (`SessionCommand::Cancel`, the TUI's Esc).
    pub fn abort(&self) -> Result<(), ClientClosed> {
        self.send(Command::Cancel)
    }

    /// Answer a prompt (`None` cancels it), on whatever connection is up
    /// when the connection thread gets to it. Prefer [`Client::answer_on`].
    pub fn answer(&self, prompt_id: u64, value: Option<String>) -> Result<(), ClientClosed> {
        self.send(Command::Answer { prompt_id, value })
    }

    /// Answer a prompt of the snapshot or events that came on `connection`
    /// (`Attached::connection`). If the session has reconnected since (a
    /// reload, a restarted daemon), the answer is dropped, never sent, and
    /// comes back as `Error::Refused { command: "answer" }`.
    pub fn answer_on(&self, connection: u64, prompt_id: u64, value: Option<String>) -> Result<(), ClientClosed> {
        self.send(Command::AnswerOn { connection, prompt_id, value })
    }

    /// Re-fetch the whole history: an `Event::History` with a `Replace`.
    pub fn resync(&self) -> Result<(), ClientClosed> {
        self.send(Command::Resync)
    }

    /// Ask for one page of worker `handle_id`'s (`sa_N`) watch journal from
    /// `since` (0: everything it has shown so far; then the last page's
    /// `next_seq`) → `Event::SubagentWatch` with a [`WatchReply`]. Pages
    /// include the tool-queue entries. Replies are matched to their query
    /// (another client's never comes here); keep one in flight. Not
    /// attached: `Error::NotAttached`; reconnecting: `Error::Refused`.
    pub fn watch_subagent(&self, handle_id: impl Into<String>, since: u64) -> Result<(), ClientClosed> {
        self.send(Command::SubagentWatch { handle_id: handle_id.into(), since })
    }

    /// Steer the running worker `handle_id` (owner-only, like `steer`). A
    /// refusal (not running, not the owner, an older daemon) comes back as
    /// `Error::Refused { command: Some("subagent_steer") }`; an accepted
    /// steer is noted in the main conversation (an `External` event).
    pub fn steer_subagent(&self, handle_id: impl Into<String>, text: impl Into<String>) -> Result<(), ClientClosed> {
        self.send(Command::SubagentSteer { handle_id: handle_id.into(), text: text.into() })
    }

    /// Cancel the running worker `handle_id` (`sa_N`) as `/workers cancel
    /// sa_N` does: `SessionCommand::EngineCommand { name: "workers", arg:
    /// "cancel sa_N" }`, owner-only like `steer`. It stops at its next step
    /// and sends no completion wake; the engine notes the cancel in the
    /// main conversation. The answer is `Event::WorkerCancel`, also for a
    /// cancel that never reached the daemon (the connection dropped, the
    /// session was detached or ended first). One worker only: a handle
    /// that is not `sa_` and digits (`all` would cancel every worker) is
    /// never sent, and answered `Err`. Daemons from v0.11.0.
    pub fn cancel_worker(&self, handle_id: impl Into<String>) -> Result<(), ClientClosed> {
        self.send(Command::CancelWorker { handle_id: handle_id.into() })
    }

    /// Run slash command `name` with `arg` in the attached session, as the
    /// TUI's `/compact`, `/context` ... do: `SessionCommand::EngineCommand`,
    /// owner-only like `steer`. The answer is `Event::EngineReply`, also for
    /// a command that never reached the daemon.
    pub fn engine_command(&self, name: impl Into<String>, arg: impl Into<String>) -> Result<(), ClientClosed> {
        self.send(Command::EngineCommand { name: name.into(), arg: arg.into() })
    }

    /// Leave the session cleanly (`Detach` + `Bye`); its turn keeps running.
    pub fn detach(&self) -> Result<(), ClientClosed> {
        self.send(Command::Detach)
    }

    /// Detach, say `Bye` on both connections and stop the thread.
    pub fn shutdown(&self) -> Result<(), ClientClosed> {
        self.send(Command::Shutdown)
    }
}

type Events = async_channel::Sender<Event>;

fn emit(events: &Events, e: Event) {
    let _ = events.try_send(e);
}

fn emit_error(events: &Events, attach_seq: Option<u64>, error: Error) {
    emit(events, Event::Error { attach_seq, error });
}

#[derive(Clone)]
struct Shared {
    endpoint: Endpoint,
    identity: Identity,
    events: Events,
    /// The last `Attached::connection` handed out.
    connections: Arc<AtomicU64>,
}

enum Ctl {
    Refresh,
    SavedSessions { named_only: bool, limit: u32 },
    CatalogPage { req: u64, before: Option<CatalogCursor>, limit: u32, named_only: bool },
    Rename { session_id: String, name: Option<String>, expect: Option<Option<String>> },
    Shutdown,
}

/// A control request waiting for its reply. Matching replies to requests
/// by order relies on the daemon answering control frames in order: its
/// connection loop (`daemon/conn.rs`) awaits `rename_session` and
/// `saved::saved_sessions` inline before it reads the next frame. If it
/// ever spawns either, an `Error` could answer the wrong request.
///
/// `Catalog` is the exception: the daemon serves a page as a task, so its
/// reply is matched by `req`, and can come after later replies.
enum Pending {
    Saved,
    Catalog(u64),
    Rename(String),
}

/// A control request that cannot be sent (no connection): answer it now.
fn not_sent(events: &Events, ctl: Ctl) {
    match ctl {
        Ctl::SavedSessions { .. } => emit(events, Event::SavedSessionsFailed { reason: "not connected to the daemon".into() }),
        Ctl::CatalogPage { req, .. } => emit(events, Event::CatalogPageFailed { req, reason: "not connected to the daemon".into() }),
        Ctl::Rename { session_id, .. } => emit(
            events,
            Event::RenameFailed(RenameFailure::new(session_id, RenameRefusal::Offline, "not connected to the daemon; nothing was renamed")),
        ),
        Ctl::Refresh | Ctl::Shutdown => {}
    }
}

/// The connection ended with `p` unanswered.
fn lost(events: &Events, p: Pending, why: &str) {
    match p {
        Pending::Saved => emit(events, Event::SavedSessionsFailed { reason: format!("the connection dropped ({why})") }),
        Pending::Catalog(req) => emit(events, Event::CatalogPageFailed { req, reason: format!("the connection dropped ({why})") }),
        Pending::Rename(sid) => emit(
            events,
            Event::RenameFailed(RenameFailure::new(
                sid,
                RenameRefusal::Unknown,
                format!("the connection dropped before the reply ({why}); the rename may have applied"),
            )),
        ),
    }
}

/// Take the oldest pending request `want` accepts.
fn take_pending(pending: &mut VecDeque<Pending>, want: impl Fn(&Pending) -> bool) -> Option<Pending> {
    let i = pending.iter().position(want)?;
    pending.remove(i)
}

struct SessionTask {
    attach_seq: u64,
    tx: mpsc::UnboundedSender<Command>,
    task: tokio::task::JoinHandle<Exit>,
}

#[derive(Clone)]
struct Exit {
    session_id: Option<String>,
    reason: String,
}

async fn run(options: ClientOptions, mut rx: mpsc::UnboundedReceiver<Msg>, events: Events, features: Features) {
    let sh = Shared { endpoint: options.endpoint, identity: options.identity, events, connections: Arc::default() };
    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let control = tokio::spawn(control_loop(sh.clone(), ctl_rx, options.poll_sessions, features));
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(u64, Exit)>();
    let mut session: Option<SessionTask> = None;
    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some(Msg { cmd, attach_seq }) = msg else { break };
                match cmd {
                    Command::RefreshSessions => {
                        let _ = ctl_tx.send(Ctl::Refresh);
                    }
                    Command::SavedSessions { named_only, limit } => {
                        let _ = ctl_tx.send(Ctl::SavedSessions { named_only, limit });
                    }
                    Command::CatalogPage { req, before, limit, named_only } => {
                        let _ = ctl_tx.send(Ctl::CatalogPage { req, before, limit, named_only });
                    }
                    Command::Rename { session_id, name, expect } => {
                        let _ = ctl_tx.send(Ctl::Rename { session_id, name, expect });
                    }
                    Command::Attach { .. } | Command::Create(_) => {
                        if let Some(old) = session.take() {
                            stop(old, "replaced by another attach", &sh.events).await;
                        }
                        let (target, mode) = match cmd {
                            Command::Attach { session_id, mode } => (Target::Existing(session_id), mode),
                            Command::Create(o) => {
                                let mode = o.mode;
                                (Target::Create(o), mode)
                            }
                            _ => unreachable!(),
                        };
                        let (stx, srx) = mpsc::unbounded_channel();
                        let (sh2, done) = (sh.clone(), done_tx.clone());
                        let task = tokio::spawn(async move {
                            let exit = session_loop(sh2, attach_seq, target, mode, srx).await;
                            let _ = done.send((attach_seq, exit.clone()));
                            exit
                        });
                        session = Some(SessionTask { attach_seq, tx: stx, task });
                    }
                    Command::Detach => {
                        if let Some(old) = session.take() {
                            stop(old, "detached", &sh.events).await;
                        }
                    }
                    Command::Shutdown => break,
                    other => {
                        let name = other.name();
                        match &session {
                            Some(s) if s.tx.send(other).is_ok() => {}
                            _ => emit_error(&sh.events, None, Error::NotAttached { command: name }),
                        }
                    }
                }
            }
            Some((attach_seq, exit)) = done_rx.recv() => {
                // A session task ended by itself; a stale one (already
                // stopped and reported) is ignored.
                if session.as_ref().is_some_and(|s| s.attach_seq == attach_seq) {
                    session = None;
                    emit(&sh.events, Event::Detached { attach_seq, session_id: exit.session_id, reason: exit.reason });
                }
            }
        }
    }
    if let Some(old) = session.take() {
        stop(old, "client shut down", &sh.events).await;
    }
    let _ = ctl_tx.send(Ctl::Shutdown);
    let _ = tokio::time::timeout(DETACH_GRACE, control).await;
}

/// Detach a session task cleanly (it sends `Detach` + `Bye`), or abort it
/// after [`DETACH_GRACE`], and report it `Detached`, with what the task
/// knew (its session, and a create that may have happened).
async fn stop(mut s: SessionTask, reason: &str, events: &Events) {
    let _ = s.tx.send(Command::Detach);
    let (session_id, reason) = match tokio::time::timeout(DETACH_GRACE, &mut s.task).await {
        Ok(Ok(exit)) if exit.reason != DETACHED => (exit.session_id, format!("{reason}: {}", exit.reason)),
        Ok(Ok(exit)) => (exit.session_id, reason.to_string()),
        Ok(Err(_)) => (None, reason.to_string()),
        Err(_) => {
            s.task.abort();
            (None, reason.to_string())
        }
    };
    emit(events, Event::Detached { attach_seq: s.attach_seq, session_id, reason });
}

/// Run `fut` until it ends or `ctl` says shut down (`None`). A `Refresh`
/// meanwhile is dropped: the next connect lists sessions anyway.
async fn until_shutdown<F: Future>(ctl: &mut mpsc::UnboundedReceiver<Ctl>, events: &Events, fut: F) -> Option<F::Output> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return Some(out),
            m = ctl.recv() => match m {
                Some(Ctl::Refresh) => {}
                Some(Ctl::Shutdown) | None => return None,
                Some(other) => not_sent(events, other),
            },
        }
    }
}

async fn control_loop(sh: Shared, mut ctl: mpsc::UnboundedReceiver<Ctl>, poll: Option<Duration>, features: Features) {
    let events = &sh.events;
    let set_features = |f: Option<Vec<String>>| *features.lock().unwrap_or_else(|e| e.into_inner()) = f;
    let mut backoff = Backoff::default();
    emit(events, Event::Status(Status::Connecting));
    loop {
        let path = sh.endpoint.resolve();
        let Some(res) = until_shutdown(&mut ctl, events, Conn::connect(&path, sh.identity.hello(None))).await else { return };
        let mut c = match res {
            Ok(c) => c,
            Err(e) => {
                if let Some(inc) = e.downcast_ref::<Incompatible>() {
                    // Terminal: retrying cannot help. A refresh tries again.
                    emit(events, Event::Status(Status::Incompatible(inc.clone())));
                    loop {
                        match ctl.recv().await {
                            Some(Ctl::Refresh) => break,
                            Some(Ctl::Shutdown) | None => return,
                            Some(other) => not_sent(events, other),
                        }
                    }
                    continue;
                }
                emit(events, Event::Status(Status::Offline(format!("{e:#}"))));
                let wait = backoff.wait(Duration::ZERO, Instant::now().into_std());
                if until_shutdown(&mut ctl, events, tokio::time::sleep(wait)).await.is_none() {
                    return;
                }
                continue;
            }
        };
        let up_at = Instant::now();
        let w = &c.welcome;
        set_features(Some(w.features.clone()));
        let saved_ok = w.features.iter().any(|f| f == FEATURE_SAVED_SESSIONS);
        let page_ok = w.features.iter().any(|f| f == FEATURE_SESSION_PAGE);
        emit(
            events,
            Event::Status(Status::Connected {
                daemon_version: w.daemon_version.clone(),
                profile: w.profile.clone(),
                pid: w.pid,
                features: w.features.clone(),
            }),
        );
        emit(events, Event::Sessions(w.sessions.clone()));
        if w.rejected_sessions > 0 {
            emit_error(events, None, rejected_error(w.rejected_sessions));
        }
        let mut next_poll = poll.map(|p| Instant::now() + p);
        let mut pending: VecDeque<Pending> = VecDeque::new();
        let (why, delay): (String, Duration) = loop {
            tokio::select! {
                m = ctl.recv() => match m {
                    Some(Ctl::Refresh) => {
                        if let Err(e) = c.send(&ClientFrame::Sessions).await {
                            break (e.to_string(), Duration::ZERO);
                        }
                    }
                    // Never to a daemon that does not list it: it would
                    // answer `Error{unknown variant}`.
                    Some(Ctl::SavedSessions { .. }) if !saved_ok => {
                        emit_error(events, None, Error::Unsupported { feature: FEATURE_SAVED_SESSIONS });
                    }
                    Some(Ctl::SavedSessions { named_only, limit }) => {
                        pending.push_back(Pending::Saved);
                        if let Err(e) = c.send(&ClientFrame::SavedSessions { named_only, limit }).await {
                            break (e.to_string(), Duration::ZERO);
                        }
                    }
                    Some(Ctl::CatalogPage { req, .. }) if !page_ok => {
                        emit(events, Event::CatalogPageFailed { req, reason: format!("this daemon does not support {FEATURE_SESSION_PAGE}") });
                        emit_error(events, None, Error::Unsupported { feature: FEATURE_SESSION_PAGE });
                    }
                    Some(Ctl::CatalogPage { req, before, limit, named_only }) => {
                        pending.push_back(Pending::Catalog(req));
                        if let Err(e) = c.send(&ClientFrame::CatalogPage { req, before, limit: limit.clamp(1, CATALOG_PAGE_MAX), named_only }).await {
                            break (e.to_string(), Duration::ZERO);
                        }
                    }
                    Some(Ctl::Rename { session_id, name, expect }) => {
                        pending.push_back(Pending::Rename(session_id.clone()));
                        if let Err(e) = c.send(&ClientFrame::Rename { session_id, name, expect }).await {
                            break (e.to_string(), Duration::ZERO);
                        }
                    }
                    Some(Ctl::Shutdown) | None => {
                        c.bye().await;
                        return;
                    }
                },
                _ = tokio::time::sleep_until(next_poll.unwrap_or_else(Instant::now)), if next_poll.is_some() => {
                    next_poll = poll.map(|p| Instant::now() + p);
                    if let Err(e) = c.send(&ClientFrame::Sessions).await {
                        break (e.to_string(), Duration::ZERO);
                    }
                }
                f = c.recv() => match f {
                    Ok(Some(DaemonFrame::SessionList { sessions, rejected })) => {
                        emit(events, Event::Sessions(sessions));
                        if rejected > 0 {
                            emit_error(events, None, rejected_error(rejected));
                        }
                    }
                    Ok(Some(DaemonFrame::Bye(Some(ByeReason::Reloading { retry_after_ms, .. })))) => {
                        break (RELOADING.into(), Duration::from_millis(retry_after_ms.min(10_000)));
                    }
                    Ok(Some(DaemonFrame::Bye(_))) => break ("daemon said bye".into(), Duration::ZERO),
                    Ok(Some(DaemonFrame::SavedSessionList { sessions, truncated, rejected })) => {
                        take_pending(&mut pending, |p| matches!(p, Pending::Saved));
                        emit(events, Event::SavedSessions { sessions, truncated, rejected });
                    }
                    // Matched by `req`; one not pending (asked on a dropped
                    // connection, or never) is not ours to report.
                    Ok(Some(DaemonFrame::CatalogPage { req, sessions, next, rejected, error })) => {
                        if take_pending(&mut pending, |p| matches!(p, Pending::Catalog(r) if *r == req)).is_some() {
                            emit(events, match error {
                                Some(reason) => Event::CatalogPageFailed { req, reason },
                                None => Event::CatalogPage { req, sessions, next, rejected },
                            });
                        }
                    }
                    Ok(Some(DaemonFrame::Renamed { session_id, name, in_memory_only })) => {
                        take_pending(&mut pending, |p| matches!(p, Pending::Rename(_)));
                        emit(events, Event::Renamed { session_id, name, in_memory_only });
                    }
                    Ok(Some(DaemonFrame::RenameFailed(f))) => {
                        take_pending(&mut pending, |p| matches!(p, Pending::Rename(_)));
                        emit(events, Event::RenameFailed(f));
                    }
                    // `Refused{UnknownSession}` answers a rename; the daemon
                    // closes the connection after it (and we reconnect).
                    Ok(Some(DaemonFrame::Refused { reason, message })) => {
                        if let Some(Pending::Rename(sid)) = take_pending(&mut pending, |p| matches!(p, Pending::Rename(_))) {
                            let r = if refuse_kind(&reason) == "unknown_session" { RenameRefusal::NotLive } else { RenameRefusal::Failed };
                            emit(events, Event::RenameFailed(RenameFailure::new(sid, r, message)));
                        }
                    }
                    // A control `Error` answers the oldest pending request:
                    // an older daemon's "malformed frame: unknown variant".
                    // It carries no `req`: a page is its target only when it
                    // names `catalog_page` or no in-order request waits.
                    Ok(Some(DaemonFrame::Error { session_id: None, message })) if !pending.is_empty() => {
                        let unknown = message.contains("unknown variant");
                        let page = message.contains("catalog_page") || pending.iter().all(|p| matches!(p, Pending::Catalog(_)));
                        let p = if page {
                            take_pending(&mut pending, |p| matches!(p, Pending::Catalog(_)))
                        } else {
                            take_pending(&mut pending, |p| !matches!(p, Pending::Catalog(_)))
                        }
                        .or_else(|| pending.pop_front());
                        match p {
                            Some(Pending::Catalog(req)) => emit(events, Event::CatalogPageFailed { req, reason: message }),
                            Some(Pending::Rename(sid)) => {
                                let r = if unknown { RenameRefusal::Unsupported } else { RenameRefusal::Failed };
                                emit(events, Event::RenameFailed(RenameFailure::new(sid, r, message)));
                            }
                            _ if unknown => emit_error(events, None, Error::Unsupported { feature: FEATURE_SAVED_SESSIONS }),
                            _ => emit(events, Event::SavedSessionsFailed { reason: message }),
                        }
                    }
                    // A page reply that did not decode carries no `req` we
                    // can trust: it answers the oldest page (replies come in
                    // order), which fails rather than waiting for good.
                    Ok(Some(DaemonFrame::Malformed { kind, error })) => {
                        if kind == "catalog_page"
                            && let Some(Pending::Catalog(req)) = take_pending(&mut pending, |p| matches!(p, Pending::Catalog(_)))
                        {
                            emit(events, Event::CatalogPageFailed { req, reason: format!("catalog_page did not decode: {error}") });
                        }
                        emit_error(events, None, Error::Decode(DecodeError { kind, error }));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break ("daemon closed the connection".into(), Duration::ZERO),
                    Err(e) => break (e.to_string(), Duration::ZERO),
                },
            }
        };
        set_features(None);
        for p in pending.drain(..) {
            lost(events, p, &why);
        }
        drop(c);
        let now = Instant::now().into_std();
        backoff.dropped(up_at.elapsed(), why == RELOADING, now);
        let wait = backoff.wait(delay, now);
        emit(events, Event::Status(Status::Reconnecting { why }));
        if until_shutdown(&mut ctl, events, tokio::time::sleep(wait)).await.is_none() {
            return;
        }
    }
}

fn rejected_error(n: usize) -> Error {
    Error::Decode(DecodeError { kind: "session_meta".into(), error: format!("{n} session list entries did not decode (skipped)") })
}

#[derive(Debug)]
enum Target {
    Existing(String),
    Create(CreateOptions),
}

struct Previous {
    client: u64,
    session_id: String,
    was_owner: bool,
    generation: u64,
}

enum Attempt {
    Attached(Conn, Box<Attached>),
    /// Transient (reload, EOF, busy, timeout): retry within the budget.
    Retry {
        why: String,
        after: Duration,
    },
    /// Retrying cannot help (refused, unknown session, bad cwd, undecodable
    /// `Attached`, incompatible daemon, or a create that may have happened).
    Fatal(String),
}

/// One connect + `Attach`, bounded by `ATTACH_TIMEOUT` (parked: longer).
async fn attempt(sh: &Shared, target: &Target, mode: AttachMode, previous: Option<&Previous>, create_sent: &mut bool) -> Attempt {
    let reconnect_of = previous.map(|p| ClientReconnect {
        previous_client: p.client,
        session_id: p.session_id.clone(),
        was_owner: p.was_owner,
        generation: p.generation,
    });
    let mut c = match Conn::connect(&sh.endpoint.resolve(), sh.identity.hello(reconnect_of)).await {
        Ok(c) => c,
        Err(e) if e.downcast_ref::<Incompatible>().is_some() => return Attempt::Fatal(format!("{e:#}")),
        Err(e) => return Attempt::Retry { why: format!("{e:#}"), after: Duration::ZERO },
    };
    let parked = |id: &str, c: &Conn| c.welcome.sessions.iter().any(|s| s.id == id && s.is_parked());
    let attach = match target {
        Target::Existing(id) => {
            // Re-take ownership after a reload, never as an observer.
            let mode = if previous.is_some_and(|p| p.was_owner) && mode != AttachMode::Observe { AttachMode::Takeover } else { mode };
            Attach::Existing { session_id: id.clone(), mode }
        }
        // A resume is idempotent on the daemon (`live_continue_target`: a
        // live target, by id, journal id, name or chain, is attached, not
        // made twice), so it is simply sent again: the first one may still
        // be loading its journal, not yet in `Welcome.sessions`.
        Target::Create(o) if *create_sent && o.resume.is_none() => {
            // The create may have happened: attach to the session of that
            // name instead of making a second one.
            let named = c.welcome.sessions.iter().find(|s| o.name.as_ref().is_some_and(|n| s.name.as_ref() == Some(n)));
            match named {
                Some(s) => Attach::Existing { session_id: s.id.clone(), mode },
                None => {
                    c.bye().await;
                    return Attempt::Fatal(format!("the connection dropped {MAY_EXIST}"));
                }
            }
        }
        Target::Create(o) => Attach::Create { config: o.config(), mode },
    };
    let (creating, budget) = match &attach {
        // A resume loads and folds a journal before `Attached`.
        Attach::Create { config, .. } => (true, if config.continue_session.is_some() { ATTACH_TIMEOUT_PARKED } else { ATTACH_TIMEOUT }),
        Attach::Existing { session_id, .. } => (false, if parked(session_id, &c) { ATTACH_TIMEOUT_PARKED } else { ATTACH_TIMEOUT }),
    };
    if creating {
        *create_sent = true;
    }
    if let Err(e) = c.send(&ClientFrame::Attach(attach)).await {
        return Attempt::Retry { why: format!("{e:#}"), after: Duration::ZERO };
    }
    let deadline = Instant::now() + budget;
    loop {
        let Ok(frame) = tokio::time::timeout_at(deadline, c.recv()).await else {
            return Attempt::Retry { why: format!("attach timed out after {budget:?}"), after: Duration::ZERO };
        };
        match frame {
            Ok(Some(DaemonFrame::Attached(a))) => return Attempt::Attached(c, a),
            Ok(Some(DaemonFrame::Refused { reason, message })) if refuse_kind(&reason) == "busy" => {
                // Refused before anything was created: safe to create again.
                *create_sent = false;
                return Attempt::Retry { why: message, after: Duration::ZERO };
            }
            Ok(Some(DaemonFrame::Refused { message, .. })) => return Attempt::Fatal(message),
            Ok(Some(DaemonFrame::Error { message, .. })) => return Attempt::Fatal(message),
            Ok(Some(DaemonFrame::Malformed { kind, error })) => {
                return Attempt::Fatal(DecodeError { kind, error }.to_string());
            }
            Ok(Some(DaemonFrame::Bye(Some(ByeReason::Reloading { retry_after_ms, .. })))) => {
                return Attempt::Retry { why: RELOADING.into(), after: Duration::from_millis(retry_after_ms.min(10_000)) };
            }
            Ok(Some(DaemonFrame::Bye(_))) | Ok(None) => {
                return Attempt::Retry { why: "daemon closed the connection before attaching".into(), after: Duration::ZERO };
            }
            Ok(Some(_)) => continue,
            Err(e) => return Attempt::Retry { why: format!("{e:#}"), after: Duration::ZERO },
        }
    }
}

/// Run `fut` while answering session commands: there is no session to send
/// them to, so input is refused (back to the user) instead of being
/// delivered late to whatever turn runs after the reconnect. `None`: a
/// detach (or the client went away).
async fn without_session<F: Future>(
    cmds: &mut mpsc::UnboundedReceiver<Command>,
    events: &Events,
    attach_seq: u64,
    fut: F,
) -> Option<F::Output> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return Some(out),
            cmd = cmds.recv() => match cmd {
                None | Some(Command::Detach) | Some(Command::Shutdown) => return None,
                Some(Command::Resync) | Some(Command::RefreshSessions) => {}
                Some(Command::CancelWorker { handle_id }) => emit(events, Event::WorkerCancel {
                    attach_seq,
                    handle_id,
                    result: Err("not connected to the session (attaching or reconnecting): not cancelled".into()),
                }),
                Some(Command::EngineCommand { name, .. }) => emit(events, Event::EngineReply {
                    attach_seq,
                    name,
                    reply: CommandReply::NotRun("not connected to the session (attaching or reconnecting)".into()),
                }),
                Some(c) => emit_error(events, Some(attach_seq), Error::Refused {
                    command: Some(c.name().into()),
                    reason: "not connected to the session (attaching or reconnecting)".into(),
                }),
            },
        }
    }
}

fn is_message_history(v: &Value) -> bool {
    v.get("ev").and_then(Value::as_str) == Some("stream")
        && v.pointer("/event/kind").and_then(Value::as_str) == Some("session")
        && v.pointer("/event/session").and_then(Value::as_str) == Some("message_history")
}

/// Watch queries one connection keeps waiting for (the app keeps one).
const WATCHES_MAX: usize = 8;
/// Engine commands (worker cancels, slash commands) one connection keeps
/// waiting for.
const CANCELS_MAX: usize = 16;

/// An engine command in flight: a worker cancel (its handle) or a slash
/// command (its name). The daemon refuses them in order, without an id:
/// one queue keeps their order.
enum Asked {
    Cancel(String),
    /// Its name and argument.
    Run(String, String),
}

/// `asked` answered (a cancel: `Ok` done, else `Err` why).
fn answer(events: &Events, attach_seq: u64, asked: Asked, reply: CommandReply) {
    match asked {
        Asked::Cancel(handle_id) => {
            let result = match reply {
                CommandReply::Done(t) => Ok(t),
                CommandReply::NotRun(why) | CommandReply::Unknown(why) => Err(why),
            };
            emit(events, Event::WorkerCancel { attach_seq, handle_id, result })
        }
        Asked::Run(name, _) => emit(events, Event::EngineReply { attach_seq, name, reply }),
    }
}

/// `sa_` and digits: one worker's handle. `workers cancel all` (or
/// anything else the engine command reads) is never sent.
fn is_worker_handle(h: &str) -> bool {
    h.strip_prefix("sa_").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The engine commands still in flight as the connection or the
/// attachment ends: each answered (a cancel `Err(why)` with where to look,
/// a slash command [`CommandReply::Unknown`]: it may have run), never left
/// without an answer.
fn unanswered(events: &Events, attach_seq: u64, cancels: &mut VecDeque<(u64, Asked)>, why: &str) {
    for (_, asked) in cancels.drain(..) {
        let reply = match asked {
            Asked::Cancel(_) => CommandReply::NotRun(format!("{why}: look at the worker's row")),
            Asked::Run(..) => CommandReply::Unknown(why.to_string()),
        };
        answer(events, attach_seq, asked, reply);
    }
}

/// A slash command's `value` (agent-engine `engine_command_reply`, the same
/// on v0.11 and v0.12): `output` and `notice` are done with their text,
/// `none` done with none (`/compact`: its outcome comes as a session event),
/// `error` not run and why, `unhandled` not a command the daemon runs. A
/// `/model x` notice names the model in use after it: another is not a
/// switch. Any other kind: unknown.
fn run_reply(name: &str, arg: &str, value: &Value) -> CommandReply {
    let text = value.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    match value.get("kind").and_then(Value::as_str) {
        Some("notice")
            if name == "model"
                && let Some(now) = value.get("model").and_then(Value::as_str)
                && now != arg.trim() =>
        {
            CommandReply::NotRun(format!("not switched: the model is still {now}"))
        }
        Some("output" | "notice" | "quit" | "none") => CommandReply::Done(text),
        Some("error") if !text.is_empty() => CommandReply::NotRun(text),
        Some("error") => CommandReply::NotRun("the daemon refused it".into()),
        Some("unhandled") => CommandReply::NotRun(format!("the daemon does not run /{name}")),
        Some(k) => CommandReply::Unknown(format!("the daemon answered `{k}`, which this app does not read")),
        None => CommandReply::Unknown("the daemon's answer did not say".into()),
    }
}

/// A `workers cancel` engine command's `value`: `{"kind": "output", "text":
/// "Cancelled sa_3 (…)"}`, or `{"kind": "error", "text": "sa_3 is already
/// completed — nothing to cancel"}`.
fn cancel_reply(value: &Value) -> Result<String, String> {
    let text = value.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    match value.get("kind").and_then(Value::as_str) {
        Some("output") => Ok(text),
        Some("error") if !text.is_empty() => Err(text),
        Some(k) => Err(format!("the daemon did not cancel it (it answered `{k}`)")),
        None => Err("the daemon's answer did not say whether it cancelled".into()),
    }
}

/// Why a worker steer was refused by a daemon before v0.11.0.
pub const STEER_UNSUPPORTED: &str = "this daemon predates steering workers (Synaps v0.11.0)";

/// The tag in an older daemon's `malformed frame: unknown variant `x`,
/// expected one of …` (serde names it in backticks).
fn unknown_variant(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("malformed frame")?;
    let (_, rest) = rest.split_once("unknown variant `")?;
    Some(rest.split_once('`')?.0)
}

/// A `SubagentWatch` query's `value`: a page, or the session's "no subagent".
fn watch_reply(value: Value, handle_id: &str) -> WatchReply {
    if let Some(e) = value.get("error").and_then(Value::as_str) {
        return WatchReply::Gone(e.to_string());
    }
    match serde_json::from_value::<WatchPage>(value) {
        Ok(p) if p.handle_id.is_empty() || p.handle_id == handle_id => WatchReply::Page(Box::new(p)),
        Ok(p) => WatchReply::Undecodable(format!("a page of {} for {handle_id}", p.handle_id)),
        Err(e) => WatchReply::Undecodable(e.to_string()),
    }
}

/// Daemon `Error` frames that mean "this command was not carried out".
fn is_refusal(message: &str) -> bool {
    message.starts_with("refused: ") || message == RELOADING_REFUSAL || message.starts_with("backpressure:")
}

async fn session_loop(
    sh: Shared,
    attach_seq: u64,
    mut target: Target,
    mode: AttachMode,
    mut cmds: mpsc::UnboundedReceiver<Command>,
) -> Exit {
    let events = sh.events.clone();
    let full = sh.identity.history() == HistoryMode::Full;
    let mut session_id = match &target {
        Target::Existing(id) => Some(id.clone()),
        Target::Create(_) => None,
    };
    let detached = |session_id: Option<String>| Exit { session_id, reason: DETACHED.into() };
    let mut previous: Option<Previous> = None;
    let mut backoff = Backoff::default();
    let mut deadline = Instant::now() + RECONNECT_BUDGET;
    let mut create_sent = false;
    loop {
        let res = without_session(&mut cmds, &events, attach_seq, attempt(&sh, &target, mode, previous.as_ref(), &mut create_sent)).await;
        let Some(res) = res else {
            if create_sent {
                return Exit { session_id, reason: format!("detached {MAY_EXIST}") };
            }
            return detached(session_id);
        };
        let (mut conn, mut attached) = match res {
            Attempt::Attached(c, a) => (c, a),
            Attempt::Fatal(reason) => {
                // The folder is gone on the daemon's machine: nothing was
                // created (the daemon checks `cwd` first), so create once
                // more in the fallback, and say so.
                if let Target::Create(o) = &mut target
                    && let Some(missing) = reason.strip_prefix(MISSING_CWD)
                    && let Some(fallback) = o.fallback_cwd.take()
                {
                    o.cwd = fallback;
                    create_sent = false;
                    emit(&events, Event::CwdFallback { attach_seq, missing: missing.to_string(), cwd: o.cwd.resolve() });
                    continue;
                }
                return Exit { session_id, reason };
            }
            Attempt::Retry { why, after } => {
                let now = Instant::now();
                if why == RELOADING {
                    backoff.dropped(Duration::ZERO, true, now.into_std());
                }
                let delay = backoff.wait(after, now.into_std());
                if now + delay > deadline {
                    let what = if previous.is_some() { "reconnect" } else { "attach" };
                    return Exit { session_id, reason: format!("{what} gave up: {why}") };
                }
                if without_session(&mut cmds, &events, attach_seq, tokio::time::sleep(delay)).await.is_none() {
                    return detached(session_id);
                }
                continue;
            }
        };
        create_sent = false;
        let up_at = Instant::now();
        let sid = attached.meta.id.clone();
        session_id = Some(sid.clone());
        target = Target::Existing(sid.clone());
        let me = attached.client;
        let mut owner = attached.input_owner;
        let mut generation = conn.welcome.generation;
        let mut mirror = full.then(|| HistoryMirror::new(&attached.conversation.api_messages));
        // Our resync query id: below the reserved range, random so another
        // client's `QueryResult` (broadcast) is never taken for ours.
        let query_id = crate::conn::random_pair().0 & (RESERVED_QUERY_ID_BASE - 1);
        // `Some(trigger)`: a `Messages` query is in flight; `trigger` is the
        // newest digest that disagreed with the mirror, if any.
        let mut resync: Option<Option<ConversationDigest>> = None;
        // Watch queries in flight on this connection: `(query id, handle id, since)`.
        let mut watches: VecDeque<(u64, String, u64)> = VecDeque::new();
        // Engine commands in flight: `(query id, what)`, oldest first.
        let mut cancels: VecDeque<(u64, Asked)> = VecDeque::new();
        let query = |sid: &str| ClientFrame::Cmd {
            session_id: sid.to_string(),
            cmd: SessionCommand::Query { id: query_id, query: SessionQuery::Messages },
        };
        let rejected = attached.rejected;
        let connection = sh.connections.fetch_add(1, Ordering::Relaxed) + 1;
        attached.connection = connection;
        emit(&events, Event::Attached { attach_seq, attached, mode, reconnect: previous.is_some() });
        if rejected.total() > 0 {
            emit_error(&events, Some(attach_seq), Error::Decode(DecodeError { kind: "attached".into(), error: rejected.to_string() }));
        }

        // Pump until the connection ends: `(why, delay)` = reconnect after it.
        let (why, delay): (String, Duration) = 'conn: loop {
            tokio::select! {
                cmd = cmds.recv() => {
                    let cmd = match cmd {
                        None | Some(Command::Detach) | Some(Command::Shutdown) => {
                            let _ = conn.send(&ClientFrame::Cmd { session_id: sid.clone(), cmd: SessionCommand::Detach { client: me } }).await;
                            conn.bye().await;
                            unanswered(&events, attach_seq, &mut cancels, "detached before the daemon answered");
                            return detached(session_id);
                        }
                        Some(c) => c,
                    };
                    let name = cmd.name();
                    let wire = match cmd {
                        Command::Submit { text } => SessionCommand::Submit { text, attachments: Vec::new() },
                        Command::Steer { text } => SessionCommand::Steer { text },
                        Command::Cancel => SessionCommand::Cancel,
                        Command::Answer { prompt_id, value } => SessionCommand::Answer { prompt_id, value },
                        Command::AnswerOn { connection: c, prompt_id, value } if c == connection => SessionCommand::Answer { prompt_id, value },
                        Command::AnswerOn { .. } => {
                            emit_error(&events, Some(attach_seq), Error::Refused {
                                command: Some(name.into()),
                                reason: "the prompt was raised on an earlier connection; not sent".into(),
                            });
                            continue;
                        }
                        Command::Resync if mirror.is_some() && resync.is_none() => {
                            resync = Some(None);
                            SessionCommand::Query { id: query_id, query: SessionQuery::Messages }
                        }
                        Command::SubagentWatch { handle_id, since } => {
                            let id = loop {
                                let id = crate::conn::random_pair().0 & (RESERVED_QUERY_ID_BASE - 1);
                                if id != query_id && !watches.iter().any(|w| w.0 == id) && !cancels.iter().any(|c| c.0 == id) {
                                    break id;
                                }
                            };
                            if watches.len() == WATCHES_MAX {
                                watches.pop_front();
                            }
                            watches.push_back((id, handle_id.clone(), since));
                            SessionCommand::Query { id, query: SessionQuery::SubagentWatch { handle_id, since, tool_queue: true } }
                        }
                        Command::SubagentSteer { handle_id, text } => SessionCommand::SubagentSteer { handle_id, text },
                        Command::CancelWorker { handle_id } if !is_worker_handle(&handle_id) => {
                            emit(&events, Event::WorkerCancel {
                                attach_seq,
                                result: Err(format!("not a worker's handle ({handle_id:?}): one worker is cancelled by its sa_N")),
                                handle_id,
                            });
                            continue;
                        }
                        Command::CancelWorker { handle_id } => {
                            let id = loop {
                                let id = crate::conn::random_pair().0 & (RESERVED_QUERY_ID_BASE - 1);
                                if id != query_id && !watches.iter().any(|w| w.0 == id) && !cancels.iter().any(|c| c.0 == id) {
                                    break id;
                                }
                            };
                            if cancels.len() == CANCELS_MAX
                                && let Some((_, a)) = cancels.pop_front()
                            {
                                answer(&events, attach_seq, a, CommandReply::Unknown("no answer from the daemon".into()));
                            }
                            let arg = format!("cancel {handle_id}");
                            cancels.push_back((id, Asked::Cancel(handle_id)));
                            SessionCommand::EngineCommand { id, name: "workers".into(), arg }
                        }
                        Command::EngineCommand { name, arg } => {
                            let id = loop {
                                let id = crate::conn::random_pair().0 & (RESERVED_QUERY_ID_BASE - 1);
                                if id != query_id && !watches.iter().any(|w| w.0 == id) && !cancels.iter().any(|c| c.0 == id) {
                                    break id;
                                }
                            };
                            if cancels.len() == CANCELS_MAX
                                && let Some((_, a)) = cancels.pop_front()
                            {
                                answer(&events, attach_seq, a, CommandReply::Unknown("no answer from the daemon".into()));
                            }
                            cancels.push_back((id, Asked::Run(name.clone(), arg.clone())));
                            SessionCommand::EngineCommand { id, name, arg }
                        }
                        _ => continue,
                    };
                    if let Err(e) = conn.send(&ClientFrame::Cmd { session_id: sid.clone(), cmd: wire }).await {
                        emit_error(&events, Some(attach_seq), Error::Refused { command: Some(name.into()), reason: format!("send failed: {e:#}") });
                        break 'conn (format!("send failed: {e:#}"), Duration::ZERO);
                    }
                }
                frame = conn.recv() => match frame {
                    Ok(Some(DaemonFrame::Event(env))) => {
                        let Envelope { session_id: esid, seq, ts, event: mut v } = env;
                        if let Some(m) = mirror.as_mut()
                            && is_message_history(&v)
                        {
                            let messages = match v.pointer_mut("/event/messages").map(Value::take) {
                                Some(Value::Array(a)) => a,
                                _ => Vec::new(),
                            };
                            drop(v);
                            let update = m.update(messages);
                            emit(&events, Event::History { attach_seq, session_id: esid, seq, update });
                            continue;
                        }
                        if v.get("ev").and_then(Value::as_str) == Some("query_result")
                            && let Some(k) = v.get("id").and_then(Value::as_u64).and_then(|q| watches.iter().position(|w| w.0 == q))
                        {
                            let (_, handle_id, since) = watches.remove(k).expect("present");
                            let reply = watch_reply(v.get_mut("value").map(Value::take).unwrap_or_default(), &handle_id);
                            if let WatchReply::Undecodable(e) = &reply {
                                emit_error(&events, Some(attach_seq), Error::Decode(DecodeError { kind: "query_result:subagent_watch".into(), error: e.clone() }));
                            }
                            emit(&events, Event::SubagentWatch { attach_seq, session_id: esid, handle_id, since, reply });
                            continue;
                        }
                        if v.get("ev").and_then(Value::as_str) == Some("query_result")
                            && let Some(k) = v.get("id").and_then(Value::as_u64).and_then(|q| cancels.iter().position(|c| c.0 == q))
                        {
                            let (_, asked) = cancels.remove(k).expect("present");
                            let value = v.get("value").unwrap_or(&Value::Null);
                            let reply = match &asked {
                                Asked::Cancel(_) => cancel_reply(value).map_or_else(CommandReply::NotRun, CommandReply::Done),
                                Asked::Run(name, arg) => run_reply(name, arg, value),
                            };
                            answer(&events, attach_seq, asked, reply);
                            continue;
                        }
                        if v.get("ev").and_then(Value::as_str) == Some("query_result") {
                            // Only our own resync is ours. Another client's
                            // result (broadcast: a co-attached TUI's resync is
                            // the whole history) and daemon-internal reserved
                            // ids are dropped, as `absorb` does.
                            let ours = v.get("id").and_then(Value::as_u64) == Some(query_id);
                            if ours
                                && let Some(m) = mirror.as_mut()
                                && let Some(trigger) = resync.take()
                            {
                                match v.get_mut("value").map(Value::take) {
                                    Some(Value::Array(messages)) => {
                                        let update = m.resynced(messages, trigger.as_ref());
                                        emit(&events, Event::History { attach_seq, session_id: esid, seq, update });
                                    }
                                    other => emit_error(&events, Some(attach_seq), Error::Decode(DecodeError {
                                        kind: "query_result:messages".into(),
                                        error: format!("expected an array, got {}", other.map(|o| o.to_string()).unwrap_or_default().chars().take(120).collect::<String>()),
                                    })),
                                }
                            }
                            continue;
                        }
                        let raw = Arc::new(v);
                        let event = match decode_event(&raw) {
                            Ok(e) => e,
                            Err(e) => {
                                emit_error(&events, Some(attach_seq), Error::Decode(e));
                                SessionEvent::Unknown
                            }
                        };
                        let mut send_query = false;
                        match &event {
                            SessionEvent::Reloading { generation: g, .. } => generation = *g,
                            SessionEvent::InputOwnerChanged { to, reason, .. } => {
                                owner = *to;
                                emit(&events, Event::InputOwner { attach_seq, session_id: esid.clone(), client: me, owner, reason: reason.clone() });
                            }
                            // A cancel refused (this client does not own the
                            // session's input): the oldest one in flight.
                            SessionEvent::Refused { client, command, reason } if *client == me && command == "engine_command" && !cancels.is_empty() => {
                                let (_, asked) = cancels.pop_front().expect("not empty");
                                answer(&events, attach_seq, asked, CommandReply::NotRun(format!("refused: {reason}")));
                            }
                            SessionEvent::Refused { client, command, reason } if *client == me => {
                                emit_error(&events, Some(attach_seq), Error::Refused { command: Some(command.clone()), reason: reason.clone() });
                            }
                            SessionEvent::Conversation { digest } => {
                                if let Some(m) = mirror.as_mut()
                                    && !m.matches(digest)
                                {
                                    if digest.messages_len == 0 {
                                        m.reset(&[]);
                                        emit(&events, Event::History { attach_seq, session_id: esid.clone(), seq, update: HistoryUpdate::Replace { messages: Vec::new() } });
                                    } else {
                                        send_query = resync.is_none();
                                        resync = Some(Some(digest.clone()));
                                    }
                                }
                            }
                            // The conversation was swapped: re-fetch even if a
                            // length-only comparison would not notice.
                            SessionEvent::Resumed { .. } | SessionEvent::CompactionApplied { .. } if mirror.is_some() && resync.is_none() => {
                                send_query = true;
                                resync = Some(None);
                            }
                            _ => {}
                        }
                        let ended = matches!(event, SessionEvent::Ended { .. });
                        emit(&events, Event::Session { attach_seq, session_id: esid, seq, ts, event, raw });
                        if ended {
                            unanswered(&events, attach_seq, &mut cancels, "the session ended before the daemon answered");
                            return Exit { session_id, reason: "session ended".into() };
                        }
                        if send_query && let Err(e) = conn.send(&query(&sid)).await {
                            break 'conn (format!("send failed: {e:#}"), Duration::ZERO);
                        }
                    }
                    Ok(Some(DaemonFrame::Bye(Some(ByeReason::Reloading { generation: g, retry_after_ms })))) => {
                        generation = g;
                        break 'conn (RELOADING.into(), Duration::from_millis(retry_after_ms.min(10_000)));
                    }
                    Ok(Some(DaemonFrame::Bye(reason))) => {
                        unanswered(&events, attach_seq, &mut cancels, "the daemon said bye before it answered");
                        return Exit { session_id, reason: format!("daemon said bye ({reason:?})") };
                    }
                    Ok(Some(DaemonFrame::Error { session_id: esid, message })) => {
                        // A daemon before v0.11.0 cannot decode a watch query
                        // or a worker steer (the connection survives).
                        match unknown_variant(&message) {
                            Some("subagent_watch") => {
                                for (_, handle_id, since) in watches.drain(..) {
                                    emit(&events, Event::SubagentWatch { attach_seq, session_id: sid.clone(), handle_id, since, reply: WatchReply::Unsupported });
                                }
                                continue;
                            }
                            Some("subagent_steer") => {
                                emit_error(&events, Some(attach_seq), Error::Refused {
                                    command: Some("subagent_steer".into()),
                                    reason: STEER_UNSUPPORTED.into(),
                                });
                                continue;
                            }
                            _ => {}
                        }
                        let error = if is_refusal(&message) {
                            Error::Refused { command: None, reason: message.strip_prefix("refused: ").unwrap_or(&message).to_string() }
                        } else {
                            Error::Daemon { session_id: esid, message }
                        };
                        emit_error(&events, Some(attach_seq), error);
                    }
                    Ok(Some(DaemonFrame::Malformed { kind, error })) => {
                        emit_error(&events, Some(attach_seq), Error::Decode(DecodeError { kind, error }));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break 'conn ("connection closed".into(), Duration::ZERO),
                    Err(e) => {
                        emit_error(&events, Some(attach_seq), Error::Transport(format!("{e:#}")));
                        break 'conn (format!("{e:#}"), Duration::ZERO);
                    }
                }
            }
        };
        unanswered(&events, attach_seq, &mut cancels, "the link dropped before the daemon answered");
        let reload = why == RELOADING;
        emit(&events, Event::Reattaching { attach_seq, session_id: sid.clone(), reason: why });
        previous = Some(Previous { client: me, session_id: sid, was_owner: owner == Some(me), generation });
        drop(conn);
        // A session that keeps dropping right after attaching backs off and
        // runs out of budget instead of reconnecting in a tight loop.
        if up_at.elapsed() >= STABLE_AFTER {
            deadline = Instant::now() + RECONNECT_BUDGET;
        }
        let now = Instant::now();
        backoff.dropped(up_at.elapsed(), reload, now.into_std());
        let delay = backoff.wait(delay, now.into_std());
        if now + delay > deadline {
            return Exit { session_id, reason: "reconnect gave up: the session connection keeps dropping".into() };
        }
        if without_session(&mut cmds, &events, attach_seq, tokio::time::sleep(delay)).await.is_none() {
            return detached(session_id);
        }
    }
}

#[cfg(test)]
mod backoff_tests {
    use super::*;

    /// A failing daemon is backed off from (100 ms doubling to 5 s); a
    /// reload's goodbye is not a failure: the daemon's `retry_after`, then a
    /// try every `RELOAD_RETRY` for `RELOAD_WINDOW` from the last goodbye,
    /// however many came before; past the window, the doubling starts over.
    #[test]
    fn a_reload_retries_on_a_short_fixed_schedule() {
        let t0 = std::time::Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        let mut b = Backoff::default();
        let waits: Vec<u64> = (0..8).map(|_| b.wait(Duration::ZERO, t0).as_millis() as u64).collect();
        assert_eq!(waits, [100, 200, 400, 800, 1600, 3200, 5000, 5000], "a failing daemon");
        // A storm: four goodbyes, each after a short time up, the reconnect
        // failing a few times in between (the new daemon booting).
        let (mut t, mut last) = (t0, t0);
        for _ in 0..4 {
            last = t;
            b.dropped(ms(300), true, t);
            assert_eq!(b.wait(ms(50), t), RELOAD_RETRY, "the goodbye: retry_after, at least RELOAD_RETRY");
            assert_eq!(b.wait(ms(3000), t), ms(3000), "a longer retry_after is kept");
            for _ in 0..6 {
                t += RELOAD_RETRY;
                assert_eq!(b.wait(Duration::ZERO, t), RELOAD_RETRY, "booting: fixed, never doubling");
            }
        }
        t = last + RELOAD_WINDOW - ms(1);
        assert_eq!(b.wait(Duration::ZERO, t), RELOAD_RETRY, "the window runs from the last goodbye");
        t += ms(1);
        assert_eq!(b.wait(Duration::ZERO, t), BACKOFF_START, "past it, a failing daemon again: from the start");
        assert_eq!(b.wait(Duration::ZERO, t), ms(200));
        // A connection that stayed up resets the doubling; a short one doesn't.
        b.dropped(ms(500), false, t);
        assert_eq!(b.wait(Duration::ZERO, t), ms(400));
        b.dropped(STABLE_AFTER, false, t);
        assert_eq!(b.wait(Duration::ZERO, t), BACKOFF_START);
    }
}
