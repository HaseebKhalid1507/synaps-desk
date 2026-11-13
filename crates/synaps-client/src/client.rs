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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::conn::{Conn, Endpoint, Identity, Incompatible};
use crate::history::{HistoryMirror, HistoryUpdate};
use crate::protocol::*;

const BACKOFF_START: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
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

    pub fn config(&self) -> CreateConfig {
        CreateConfig {
            model_override: self.model.clone(),
            cwd: self.cwd.resolve(),
            name: self.name.clone(),
            auto_approve_confirms: false,
        }
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub enum Command {
    RefreshSessions,
    Attach { session_id: String, mode: AttachMode },
    Create(CreateOptions),
    Submit { text: String },
    Steer { text: String },
    Cancel,
    Answer { prompt_id: u64, value: Option<String> },
    /// Re-fetch the whole history (`Query{Messages}`) → `HistoryUpdate::Replace`.
    Resync,
    Detach,
    Shutdown,
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
            Command::Answer { .. } => "answer",
            Command::Resync => "query",
            Command::Detach => "detach",
            Command::Shutdown => "shutdown",
        }
    }
}

impl std::fmt::Debug for Command {
    /// `Submit`/`Steer` text and `Answer` values print `<redacted>`: a
    /// secret prompt's answer is a credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Attach { session_id, mode } => {
                f.debug_struct("Attach").field("session_id", session_id).field("mode", mode).finish()
            }
            Command::Create(o) => f.debug_tuple("Create").field(o).finish(),
            Command::Submit { .. } => f.debug_struct("Submit").field("text", &format_args!("<redacted>")).finish(),
            Command::Steer { .. } => f.debug_struct("Steer").field("text", &format_args!("<redacted>")).finish(),
            Command::Answer { prompt_id, .. } => {
                f.debug_struct("Answer").field("prompt_id", prompt_id).field("value", &format_args!("<redacted>")).finish()
            }
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

/// The control connection's health: the one source of connection status.
/// Attachment state travels separately (`Attached` / `Reattaching` /
/// `Detached`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Status {
    Connecting,
    Connected { daemon_version: String, profile: Option<String>, pid: u32 },
    Reconnecting { why: String },
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
    /// A (re-)attach: the full mirror. Replace, never merge. `mode` is what
    /// was asked (`Takeover` when re-taking ownership after a reload).
    Attached { attach_seq: u64, attached: Box<Attached>, mode: AttachMode, reconnect: bool },
    /// The session connection dropped (reload, EOF) and is being re-made;
    /// an `Attached { reconnect: true }` or a `Detached` follows. Commands
    /// meanwhile come back as `Error::Refused`.
    Reattaching { attach_seq: u64, session_id: String, reason: String },
    /// A live event, decoded, with the raw wire event alongside (for fields
    /// and variants the mirror does not carry). `MessageHistory` never
    /// comes here in `Full` mode: see [`Event::History`].
    Session { attach_seq: u64, session_id: String, seq: u64, event: SessionEvent, raw: Arc<Value> },
    /// The history changed (`Full` mode).
    History { attach_seq: u64, session_id: String, seq: u64, update: HistoryUpdate },
    /// Who owns input changed. `client` is this connection's id.
    InputOwner { attach_seq: u64, session_id: String, client: u64, owner: Option<u64>, reason: String },
    /// The attachment ended for good (ended, refused, gave up, detached,
    /// replaced). Session commands now fail with `Error::NotAttached`.
    Detached { attach_seq: u64, session_id: Option<String>, reason: String },
    Error { attach_seq: Option<u64>, error: Error },
}

impl Event {
    /// The attachment this event belongs to, if it is an attachment event.
    pub fn attach_seq(&self) -> Option<u64> {
        match self {
            Event::Attached { attach_seq, .. }
            | Event::Reattaching { attach_seq, .. }
            | Event::Session { attach_seq, .. }
            | Event::History { attach_seq, .. }
            | Event::InputOwner { attach_seq, .. }
            | Event::Detached { attach_seq, .. } => Some(*attach_seq),
            Event::Error { attach_seq, .. } => *attach_seq,
            Event::Status(_) | Event::Sessions(_) => None,
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

struct Msg {
    cmd: Command,
    attach_seq: u64,
}

/// Handle to the connection thread. Cheap to clone; dropping every clone
/// (or [`Client::shutdown`]) detaches cleanly and stops the thread.
#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Msg>,
    seq: Arc<AtomicU64>,
}

impl Client {
    /// Start the connection thread. Fails only if the thread or its runtime
    /// cannot be created.
    pub fn spawn(options: ClientOptions) -> std::io::Result<(Client, async_channel::Receiver<Event>)> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let (etx, erx) = async_channel::unbounded();
        std::thread::Builder::new().name("synaps-conn".into()).spawn(move || rt.block_on(run(options, rx, etx)))?;
        Ok((Client { tx, seq: Arc::new(AtomicU64::new(0)) }, erx))
    }

    /// Discover the daemon for `profile` (`None`: `$SYNAPS_PROFILE`, else
    /// the default daemon) and connect. No session polling.
    pub fn connect(profile: Option<&str>, identity: Identity) -> std::io::Result<(Client, async_channel::Receiver<Event>)> {
        Self::spawn(ClientOptions::new(Endpoint::profile(profile), identity))
    }

    fn post(&self, cmd: Command) -> Result<u64, ClientClosed> {
        let attach_seq = match cmd {
            Command::Attach { .. } | Command::Create(_) => self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            _ => self.seq.load(Ordering::Relaxed),
        };
        self.tx.send(Msg { cmd, attach_seq }).map_err(|_| ClientClosed)?;
        Ok(attach_seq)
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

    /// Answer a prompt (`None` cancels it).
    pub fn answer(&self, prompt_id: u64, value: Option<String>) -> Result<(), ClientClosed> {
        self.send(Command::Answer { prompt_id, value })
    }

    /// Re-fetch the whole history: an `Event::History` with a `Replace`.
    pub fn resync(&self) -> Result<(), ClientClosed> {
        self.send(Command::Resync)
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
}

enum Ctl {
    Refresh,
    Shutdown,
}

struct SessionTask {
    attach_seq: u64,
    tx: mpsc::UnboundedSender<Command>,
    task: tokio::task::JoinHandle<()>,
}

struct Exit {
    session_id: Option<String>,
    reason: String,
}

async fn run(options: ClientOptions, mut rx: mpsc::UnboundedReceiver<Msg>, events: Events) {
    let sh = Shared { endpoint: options.endpoint, identity: options.identity, events };
    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let control = tokio::spawn(control_loop(sh.clone(), ctl_rx, options.poll_sessions));
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
                            let _ = done.send((attach_seq, exit));
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
/// after [`DETACH_GRACE`], and report it `Detached`.
async fn stop(mut s: SessionTask, reason: &str, events: &Events) {
    let _ = s.tx.send(Command::Detach);
    if tokio::time::timeout(DETACH_GRACE, &mut s.task).await.is_err() {
        s.task.abort();
    }
    emit(events, Event::Detached { attach_seq: s.attach_seq, session_id: None, reason: reason.into() });
}

/// Run `fut` until it ends or `ctl` says shut down (`None`). A `Refresh`
/// meanwhile is dropped: the next connect lists sessions anyway.
async fn until_shutdown<F: Future>(ctl: &mut mpsc::UnboundedReceiver<Ctl>, fut: F) -> Option<F::Output> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return Some(out),
            m = ctl.recv() => match m {
                Some(Ctl::Refresh) => {}
                Some(Ctl::Shutdown) | None => return None,
            },
        }
    }
}

async fn control_loop(sh: Shared, mut ctl: mpsc::UnboundedReceiver<Ctl>, poll: Option<Duration>) {
    let events = &sh.events;
    let mut backoff = BACKOFF_START;
    emit(events, Event::Status(Status::Connecting));
    loop {
        let path = sh.endpoint.resolve();
        let Some(res) = until_shutdown(&mut ctl, Conn::connect(&path, sh.identity.hello(None))).await else { return };
        let mut c = match res {
            Ok(c) => c,
            Err(e) => {
                if let Some(inc) = e.downcast_ref::<Incompatible>() {
                    // Terminal: retrying cannot help. A refresh tries again.
                    emit(events, Event::Status(Status::Incompatible(inc.clone())));
                    match ctl.recv().await {
                        Some(Ctl::Refresh) => continue,
                        Some(Ctl::Shutdown) | None => return,
                    }
                }
                emit(events, Event::Status(Status::Offline(format!("{e:#}"))));
                if until_shutdown(&mut ctl, tokio::time::sleep(backoff)).await.is_none() {
                    return;
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        backoff = BACKOFF_START;
        let w = &c.welcome;
        emit(
            events,
            Event::Status(Status::Connected { daemon_version: w.daemon_version.clone(), profile: w.profile.clone(), pid: w.pid }),
        );
        emit(events, Event::Sessions(w.sessions.clone()));
        if w.rejected_sessions > 0 {
            emit_error(events, None, rejected_error(w.rejected_sessions));
        }
        let mut next_poll = poll.map(|p| Instant::now() + p);
        let (why, delay) = loop {
            tokio::select! {
                m = ctl.recv() => match m {
                    Some(Ctl::Refresh) => {
                        if let Err(e) = c.send(&ClientFrame::Sessions).await {
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
                        break ("daemon reloading".into(), Duration::from_millis(retry_after_ms.min(10_000)));
                    }
                    Ok(Some(DaemonFrame::Bye(_))) => break ("daemon said bye".into(), Duration::ZERO),
                    Ok(Some(DaemonFrame::Malformed { kind, error })) => {
                        emit_error(events, None, Error::Decode(DecodeError { kind, error }));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break ("daemon closed the connection".into(), Duration::ZERO),
                    Err(e) => break (e.to_string(), Duration::ZERO),
                },
            }
        };
        drop(c);
        emit(events, Event::Status(Status::Reconnecting { why }));
        if until_shutdown(&mut ctl, tokio::time::sleep(delay)).await.is_none() {
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
    Retry { why: String, after: Duration },
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
        Target::Create(o) if *create_sent => {
            let named = o.name.as_ref().and_then(|n| c.welcome.sessions.iter().find(|s| s.name.as_deref() == Some(n.as_str())));
            match named {
                Some(s) => Attach::Existing { session_id: s.id.clone(), mode },
                None => {
                    c.bye().await;
                    return Attempt::Fatal(
                        "the connection dropped while the session was being created; it may exist now (see the session list)".into(),
                    );
                }
            }
        }
        Target::Create(o) => Attach::Create { config: o.config(), mode },
    };
    let (creating, budget) = match &attach {
        Attach::Create { .. } => (true, ATTACH_TIMEOUT),
        Attach::Existing { session_id, .. } => {
            (false, if parked(session_id, &c) { ATTACH_TIMEOUT_PARKED } else { ATTACH_TIMEOUT })
        }
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
                return Attempt::Retry {
                    why: "daemon reloading".into(),
                    after: Duration::from_millis(retry_after_ms.min(10_000)),
                };
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
async fn without_session<F: Future>(cmds: &mut mpsc::UnboundedReceiver<Command>, events: &Events, attach_seq: u64, fut: F) -> Option<F::Output> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return Some(out),
            cmd = cmds.recv() => match cmd {
                None | Some(Command::Detach) | Some(Command::Shutdown) => return None,
                Some(Command::Resync) | Some(Command::RefreshSessions) => {}
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

/// Daemon `Error` frames that mean "this command was not carried out".
fn is_refusal(message: &str) -> bool {
    message.starts_with("refused: ") || message == RELOADING_REFUSAL || message.starts_with("backpressure:")
}

async fn session_loop(sh: Shared, attach_seq: u64, mut target: Target, mode: AttachMode, mut cmds: mpsc::UnboundedReceiver<Command>) -> Exit {
    let events = sh.events.clone();
    let full = sh.identity.history() == HistoryMode::Full;
    let mut session_id = match &target {
        Target::Existing(id) => Some(id.clone()),
        Target::Create(_) => None,
    };
    let detached = |session_id: Option<String>| Exit { session_id, reason: "detached".into() };
    let mut previous: Option<Previous> = None;
    let mut backoff = BACKOFF_START;
    let mut deadline = Instant::now() + RECONNECT_BUDGET;
    let mut create_sent = false;
    loop {
        let res = without_session(&mut cmds, &events, attach_seq, attempt(&sh, &target, mode, previous.as_ref(), &mut create_sent)).await;
        let Some(res) = res else { return detached(session_id) };
        let (mut conn, attached) = match res {
            Attempt::Attached(c, a) => (c, a),
            Attempt::Fatal(reason) => return Exit { session_id, reason },
            Attempt::Retry { why, after } => {
                let delay = after.max(backoff);
                if Instant::now() + delay > deadline {
                    let what = if previous.is_some() { "reconnect" } else { "attach" };
                    return Exit { session_id, reason: format!("{what} gave up: {why}") };
                }
                if without_session(&mut cmds, &events, attach_seq, tokio::time::sleep(delay)).await.is_none() {
                    return detached(session_id);
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        backoff = BACKOFF_START;
        create_sent = false;
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
        let query = |sid: &str| ClientFrame::Cmd {
            session_id: sid.to_string(),
            cmd: SessionCommand::Query { id: query_id, query: SessionQuery::Messages },
        };
        emit(&events, Event::Attached { attach_seq, attached, mode, reconnect: previous.is_some() });

        // Pump until the connection ends: `(why, delay)` = reconnect after it.
        let (why, delay): (String, Duration) = 'conn: loop {
            tokio::select! {
                cmd = cmds.recv() => {
                    let cmd = match cmd {
                        None | Some(Command::Detach) | Some(Command::Shutdown) => {
                            let _ = conn.send(&ClientFrame::Cmd { session_id: sid.clone(), cmd: SessionCommand::Detach { client: me } }).await;
                            conn.bye().await;
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
                        Command::Resync if mirror.is_some() && resync.is_none() => {
                            resync = Some(None);
                            SessionCommand::Query { id: query_id, query: SessionQuery::Messages }
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
                        let Envelope { session_id: esid, seq, event: mut v, .. } = env;
                        if let Some(m) = mirror.as_mut() {
                            if is_message_history(&v) {
                                let messages = match v.pointer_mut("/event/messages").map(Value::take) {
                                    Some(Value::Array(a)) => a,
                                    _ => Vec::new(),
                                };
                                drop(v);
                                let update = m.update(messages);
                                emit(&events, Event::History { attach_seq, session_id: esid, seq, update });
                                continue;
                            }
                            let ours = v.get("ev").and_then(Value::as_str) == Some("query_result")
                                && v.get("id").and_then(Value::as_u64) == Some(query_id);
                            if ours {
                                if let Some(trigger) = resync.take() {
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
                            SessionEvent::Refused { client, command, reason } if *client == me => {
                                emit_error(&events, Some(attach_seq), Error::Refused { command: Some(command.clone()), reason: reason.clone() });
                            }
                            SessionEvent::Conversation { digest } => {
                                if let Some(m) = mirror.as_mut().filter(|m| !m.matches(digest)) {
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
                        emit(&events, Event::Session { attach_seq, session_id: esid, seq, event, raw });
                        if ended {
                            return Exit { session_id, reason: "session ended".into() };
                        }
                        if send_query && let Err(e) = conn.send(&query(&sid)).await {
                            break 'conn (format!("send failed: {e:#}"), Duration::ZERO);
                        }
                    }
                    Ok(Some(DaemonFrame::Bye(Some(ByeReason::Reloading { generation: g, retry_after_ms })))) => {
                        generation = g;
                        break 'conn ("daemon reloading".into(), Duration::from_millis(retry_after_ms.min(10_000)));
                    }
                    Ok(Some(DaemonFrame::Bye(reason))) => {
                        return Exit { session_id, reason: format!("daemon said bye ({reason:?})") };
                    }
                    Ok(Some(DaemonFrame::Error { session_id: esid, message })) => {
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
        emit(&events, Event::Reattaching { attach_seq, session_id: sid.clone(), reason: why });
        previous = Some(Previous { client: me, session_id: sid, was_owner: owner == Some(me), generation });
        deadline = Instant::now() + RECONNECT_BUDGET;
        drop(conn);
        if without_session(&mut cmds, &events, attach_seq, tokio::time::sleep(delay)).await.is_none() {
            return detached(session_id);
        }
    }
}
