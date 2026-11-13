//! The connection thread: a small current-thread tokio runtime that owns a
//! control connection (session list) and at most one attached session
//! connection. The UI talks to it with [`Client`] commands and reads
//! [`Event`]s from an `async_channel` receiver (runtime-agnostic, so a GPUI
//! executor can await it).
//!
//! Reload handling mirrors the TUI's `SocketTransport::reconnect`: after
//! `Reloading` / `Bye{Reloading}` (or an unexpected EOF) it reconnects with
//! `Hello{reconnect_of}` and `Attach::Existing`, `Takeover` iff this client
//! owned input, backing off 100 ms ×2 up to 5 s, for at most 60 s.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::conn::{Conn, Identity};
use crate::protocol::*;

#[derive(Debug, Clone)]
pub enum Command {
    RefreshSessions,
    Attach { session_id: String },
    Create { model: Option<String> },
    Submit { text: String },
    Steer { text: String },
    Cancel,
    Answer { prompt_id: u64, value: Option<String> },
    Detach,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Status {
    Connecting,
    Connected { daemon_version: String, profile: Option<String>, pid: u32 },
    Reconnecting { why: String },
    Offline(String),
}

#[derive(Debug, Clone)]
pub enum Event {
    Status(Status),
    Sessions(Vec<SessionMeta>),
    /// A (re-)attach: the full mirror. Replace, never merge.
    Attached { attached: Box<Attached>, reconnect: bool },
    Session { session_id: String, seq: u64, event: SessionEvent },
    /// The session connection ended for good (ended, refused, gave up).
    Detached { session_id: Option<String>, reason: String },
    /// A daemon `Error` frame or a client-side failure worth showing.
    Error(String),
}

/// Handle to the connection thread. Cheap to clone; dropping every clone
/// stops the thread.
#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Command>,
}

impl Client {
    pub fn spawn(socket: PathBuf, identity: Identity) -> (Client, async_channel::Receiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (etx, erx) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("synaps-conn".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
                rt.block_on(run(socket, identity, rx, etx));
            })
            .expect("spawn connection thread");
        (Client { tx }, erx)
    }

    /// Discover the daemon for `profile` (`None` = default) and connect.
    pub fn connect(profile: Option<&str>, identity: Identity) -> (Client, async_channel::Receiver<Event>) {
        Self::spawn(crate::discovery::resolve_socket(None, profile), identity)
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// Ask for a fresh `Event::Sessions` now (it is also polled every 3 s).
    pub fn list_sessions(&self) {
        self.send(Command::RefreshSessions)
    }

    /// Attach to a live session (replaces any current attachment).
    pub fn attach(&self, session_id: impl Into<String>) {
        self.send(Command::Attach { session_id: session_id.into() })
    }

    /// Create a session (optionally with a model override) and attach to it.
    pub fn create(&self, model: Option<String>) {
        self.send(Command::Create { model })
    }

    pub fn submit(&self, text: impl Into<String>) {
        self.send(Command::Submit { text: text.into() })
    }

    /// Abort the running turn (`SessionCommand::Cancel`, the TUI's Esc).
    pub fn abort(&self) {
        self.send(Command::Cancel)
    }

    pub fn detach(&self) {
        self.send(Command::Detach)
    }
}

type Events = async_channel::Sender<Event>;

fn emit(events: &Events, e: Event) {
    let _ = events.try_send(e);
}

async fn run(socket: PathBuf, identity: Identity, mut rx: mpsc::UnboundedReceiver<Command>, events: Events) {
    let (refresh_tx, refresh_rx) = mpsc::unbounded_channel();
    let control = tokio::spawn(control_loop(socket.clone(), identity.clone(), events.clone(), refresh_rx));
    let mut session: Option<(mpsc::UnboundedSender<Command>, tokio::task::JoinHandle<()>)> = None;
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::RefreshSessions => {
                let _ = refresh_tx.send(());
            }
            Command::Attach { .. } | Command::Create { .. } => {
                if let Some((_, task)) = session.take() {
                    task.abort();
                }
                let target = match cmd {
                    Command::Attach { session_id } => Target::Existing(session_id),
                    Command::Create { model } => Target::Create(model),
                    _ => unreachable!(),
                };
                let (stx, srx) = mpsc::unbounded_channel();
                let task = tokio::spawn(session_loop(socket.clone(), identity.clone(), target, events.clone(), srx));
                session = Some((stx, task));
                let _ = refresh_tx.send(());
            }
            Command::Detach => {
                if let Some((_, task)) = session.take() {
                    task.abort();
                }
            }
            Command::Shutdown => break,
            other => match &session {
                Some((stx, _)) => {
                    let _ = stx.send(other);
                }
                None => emit(&events, Event::Error("not attached to a session".into())),
            },
        }
    }
    control.abort();
    if let Some((_, task)) = session.take() {
        task.abort();
    }
}

const BACKOFF_START: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
const RECONNECT_BUDGET: Duration = Duration::from_secs(60);
const SESSIONS_POLL: Duration = Duration::from_secs(3);

async fn control_loop(
    socket: PathBuf,
    identity: Identity,
    events: Events,
    mut refresh: mpsc::UnboundedReceiver<()>,
) {
    let mut backoff = BACKOFF_START;
    emit(&events, Event::Status(Status::Connecting));
    loop {
        match Conn::connect(&socket, identity.hello(None)).await {
            Ok(mut c) => {
                backoff = BACKOFF_START;
                let w = &c.welcome;
                emit(
                    &events,
                    Event::Status(Status::Connected {
                        daemon_version: w.daemon_version.clone(),
                        profile: w.profile.clone(),
                        pid: w.pid,
                    }),
                );
                emit(&events, Event::Sessions(w.sessions.clone()));
                let why = 'conn: loop {
                    tokio::select! {
                        _ = tokio::time::sleep(SESSIONS_POLL) => {}
                        _ = refresh.recv() => {}
                    }
                    if let Err(e) = c.send(&ClientFrame::Sessions).await {
                        break 'conn e.to_string();
                    }
                    loop {
                        match c.recv().await {
                            Ok(Some(DaemonFrame::SessionList(s))) => {
                                emit(&events, Event::Sessions(s));
                                break;
                            }
                            Ok(Some(DaemonFrame::Bye(Some(ByeReason::Reloading { retry_after_ms, .. })))) => {
                                tokio::time::sleep(Duration::from_millis(retry_after_ms.min(10_000))).await;
                                break 'conn "daemon reloading".into();
                            }
                            Ok(Some(DaemonFrame::Bye(_))) => break 'conn "daemon said bye".into(),
                            Ok(Some(_)) => continue,
                            Ok(None) => break 'conn "daemon closed the connection".into(),
                            Err(e) => break 'conn e.to_string(),
                        }
                    }
                };
                emit(&events, Event::Status(Status::Reconnecting { why }));
            }
            Err(e) => {
                emit(&events, Event::Status(Status::Offline(format!("{e:#}"))));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

enum Target {
    Existing(String),
    Create(Option<String>),
}

struct Previous {
    client: u64,
    session_id: String,
    was_owner: bool,
    generation: u64,
}

async fn session_loop(
    socket: PathBuf,
    identity: Identity,
    mut target: Target,
    events: Events,
    mut cmds: mpsc::UnboundedReceiver<Command>,
) {
    let mut previous: Option<Previous> = None;
    let mut backoff = BACKOFF_START;
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let reconnect_of = previous.as_ref().map(|p| ClientReconnect {
            previous_client: p.client,
            session_id: p.session_id.clone(),
            was_owner: p.was_owner,
            generation: p.generation,
        });
        let attach = match &target {
            Target::Existing(id) => Attach::Existing {
                session_id: id.clone(),
                mode: if previous.as_ref().is_some_and(|p| p.was_owner) { AttachMode::Takeover } else { AttachMode::Mirror },
            },
            Target::Create(model) => Attach::Create {
                config: CreateConfig { model_override: model.clone(), cwd: None, auto_approve_confirms: false },
                mode: AttachMode::Mirror,
            },
        };
        let attempt = async {
            let mut c = Conn::connect(&socket, identity.hello(reconnect_of)).await?;
            c.send(&ClientFrame::Attach(attach)).await?;
            loop {
                match c.recv().await? {
                    Some(DaemonFrame::Attached(a)) => return Ok::<_, anyhow::Error>(Ok((c, a))),
                    Some(DaemonFrame::Error { message, .. }) => return Ok(Err(message)),
                    Some(DaemonFrame::Refused { message, .. }) => return Ok(Err(message)),
                    Some(DaemonFrame::Bye(_)) | None => anyhow::bail!("daemon closed before attaching"),
                    Some(_) => continue,
                }
            }
        };
        let (mut conn, attached) = match attempt.await {
            Ok(Ok(x)) => x,
            Ok(Err(refusal)) => {
                emit(&events, Event::Detached { session_id: previous.map(|p| p.session_id), reason: refusal });
                return;
            }
            Err(e) => {
                let Some(d) = deadline else {
                    emit(&events, Event::Detached { session_id: None, reason: format!("{e:#}") });
                    return;
                };
                if tokio::time::Instant::now() + backoff > d {
                    emit(&events, Event::Detached { session_id: previous.map(|p| p.session_id), reason: format!("reconnect gave up: {e:#}") });
                    return;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        backoff = BACKOFF_START;
        let session_id = attached.meta.id.clone();
        let me = attached.client;
        let mut owner = attached.input_owner;
        let mut generation = conn.welcome.generation;
        target = Target::Existing(session_id.clone());
        emit(&events, Event::Status(Status::Connected {
            daemon_version: conn.welcome.daemon_version.clone(),
            profile: conn.welcome.profile.clone(),
            pid: conn.welcome.pid,
        }));
        emit(&events, Event::Attached { attached, reconnect: previous.is_some() });

        // Pump until the connection ends. `Some(delay)` = reconnect after it.
        let reconnect_after: Option<Duration> = loop {
            tokio::select! {
                cmd = cmds.recv() => {
                    let Some(cmd) = cmd else { return };
                    let cmd = match cmd {
                        Command::Submit { text } => SessionCommand::Submit { text, attachments: Vec::new() },
                        Command::Steer { text } => SessionCommand::Steer { text },
                        Command::Cancel => SessionCommand::Cancel,
                        Command::Answer { prompt_id, value } => SessionCommand::Answer { prompt_id, value },
                        _ => continue,
                    };
                    if let Err(e) = conn.send(&ClientFrame::Cmd { session_id: session_id.clone(), cmd }).await {
                        emit(&events, Event::Error(format!("send failed: {e:#}")));
                    }
                }
                frame = conn.recv() => match frame {
                    Ok(Some(DaemonFrame::Event(env))) => {
                        let event = env.decode();
                        match &event {
                            SessionEvent::Reloading { generation: g, .. } => generation = *g,
                            SessionEvent::InputOwnerChanged { to, .. } => owner = *to,
                            _ => {}
                        }
                        let ended = matches!(event, SessionEvent::Ended { .. });
                        emit(&events, Event::Session { session_id: env.session_id, seq: env.seq, event });
                        if ended {
                            emit(&events, Event::Detached { session_id: Some(session_id.clone()), reason: "session ended".into() });
                            return;
                        }
                    }
                    Ok(Some(DaemonFrame::Bye(Some(ByeReason::Reloading { generation: g, retry_after_ms })))) => {
                        generation = g;
                        break Some(Duration::from_millis(retry_after_ms.min(10_000)));
                    }
                    Ok(Some(DaemonFrame::Bye(reason))) => {
                        emit(&events, Event::Detached { session_id: Some(session_id.clone()), reason: format!("daemon said bye ({reason:?})") });
                        return;
                    }
                    Ok(Some(DaemonFrame::Error { message, .. })) => emit(&events, Event::Error(message)),
                    Ok(Some(_)) => {}
                    Ok(None) => break Some(Duration::ZERO),
                    Err(e) => {
                        emit(&events, Event::Error(format!("connection failed: {e:#}")));
                        break Some(Duration::ZERO);
                    }
                }
            }
        };
        let Some(delay) = reconnect_after else { return };
        emit(&events, Event::Status(Status::Reconnecting { why: "daemon reloading or connection lost".into() }));
        previous = Some(Previous { client: me, session_id: session_id.clone(), was_owner: owner == Some(me), generation });
        deadline = Some(tokio::time::Instant::now() + RECONNECT_BUDGET);
        drop(conn);
        tokio::time::sleep(delay).await;
    }
}
