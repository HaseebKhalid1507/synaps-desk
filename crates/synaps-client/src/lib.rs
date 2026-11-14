//! Thin wire client for the Synaps daemon. No GUI dependencies; builds on
//! Linux, macOS and Windows (Unix socket / named pipe).
//!
//! # Public API (pre-1.0: see `CHANGELOG.md`)
//!
//! ```no_run
//! use synaps_client::{AttachMode, Client, CreateOptions, Cwd, Event, Identity, Transcript};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // `None`: `$SYNAPS_PROFILE`, else the default daemon. The registry entry
//! // (`~/.synaps-cli/run/daemon[-<profile>].json`) is re-read on every
//! // reconnect. One OS thread + one small tokio runtime per `Client`.
//! let (client, events) = Client::connect(Some("desktopproto"), Identity::new("avatar")?)?;
//! client.list_sessions()?;                                   // -> Event::Sessions
//! let seq = client.create(CreateOptions::new().cwd(Cwd::Home))?;  // -> Event::Attached
//! // or: client.attach("<session id>", AttachMode::Observe)?  (a watcher never drives)
//! client.submit("hello")?;     // -> Event::Session / Event::History ..., then Idle
//! client.abort()?;             // cancel the running turn
//! let mut transcript = Transcript::new();
//! while let Ok(ev) = events.recv_blocking() {
//!     if ev.attach_seq().is_some_and(|s| s != seq) {
//!         continue; // stale: an attachment the user has left
//!     }
//!     match ev {
//!         Event::Attached { attached, .. } => transcript.load_attached(&attached)?,
//!         Event::History { update, .. } => {
//!             if transcript.apply_update(&update).is_err() {
//!                 client.resync()?;
//!             }
//!         }
//!         Event::Session { event, raw, .. } => {
//!             transcript.apply(&event); // `raw` has every field the mirror drops
//!         }
//!         Event::Error { error, .. } => eprintln!("{error}"), // typed: Refused, Daemon, ...
//!         Event::Detached { reason, .. } => eprintln!("detached: {reason}"),
//!         _ => {}
//!     }
//! }
//! client.shutdown()?; // Detach + Bye, then the thread stops
//! # Ok(()) }
//! ```
//!
//! `events` is an `async_channel::Receiver`, so a GUI executor can `.await`
//! it. `Event::Attached` is a full mirror (replace, never merge); after a
//! `synaps daemon reload` the client reconnects and re-attaches by itself
//! and sends `Attached { reconnect: true, .. }`. Every attachment event
//! carries the `attach_seq` that [`Client::attach`] / [`Client::create`]
//! returned. Input the daemon refuses comes back as
//! `Event::Error { error: Error::Refused { .. } }`: give it back to the user.
//! [`ClientOptions`] adds session polling ([`ClientOptions::poll_sessions`])
//! and an explicit socket ([`Endpoint::Socket`]).
//!
//! # Named and saved sessions
//!
//! ```no_run
//! # use synaps_client::*;
//! # fn f(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
//! // The saved sessions on disk, only from a daemon whose `Welcome.features`
//! // lists `saved_sessions` (`Status::Connected { features }`); otherwise
//! // `Err(RequestError::Unsupported)` and nothing is sent.
//! if client.supports(FEATURE_SAVED_SESSIONS) {
//!     client.saved_sessions(true, 200)?;  // -> Event::SavedSessions / SavedSessionsFailed
//! }
//! // Resume one by its exact journal id (every v3 daemon: a create with
//! // `continue_session`), in its saved folder: the daemon does not restore
//! // it. If that folder is gone on the daemon's machine, run it in `~`
//! // instead (-> Event::CwdFallback). Failures ("no session matching 'x'",
//! // a journal live in another process) come back as `Event::Detached`.
//! let seq = client.resume(
//!     "20261110-202033-e5f6",
//!     CreateOptions::new().cwd(Cwd::Path("/home/me/Projects/axel".into())).fallback_cwd(Cwd::Home),
//! )?;
//! // Name a live or parked session (checked against `[a-z0-9-]{1,40}` first),
//! // compare-and-set: an already named one is refused (`RenameFailed` with
//! // `RenameRefusal::Named` and its `current_name`). To replace it, ask,
//! // then send `Expect::Named(current_name)` for that same session.
//! client.rename("<session id>", Some("my-task"), Expect::Unnamed)?;  // -> Event::Renamed / RenameFailed
//! # let _ = seq; Ok(()) }
//! ```
//!
//! Modules:
//! - [`discovery`]: run dir + `daemon[-profile].json`
//! - [`ipc`]: Unix socket (same-user check) / Windows named pipe (pipe name
//!   and same-user check ported from Synaps)
//! - [`protocol`]: tolerant serde mirrors of the v3 frames (`#[non_exhaustive]`)
//! - [`conn`]: one handshaken connection, [`Identity`], [`Endpoint`]
//! - [`client`]: the connection thread (control + attached session, reload
//!   reconnect, history drift resync)
//! - [`history`]: the digest-checked history mirror ([`HistoryUpdate`])
//! - [`transcript`]: history + live events folded into display rows
//! - [`synthetic`]: deterministic load for benches

pub mod client;
pub mod conn;
pub mod discovery;
pub mod history;
pub mod ipc;
pub mod protocol;
pub mod synthetic;
pub mod transcript;

pub use client::{Client, ClientClosed, ClientOptions, Command, CreateOptions, Cwd, Error, Event, Expect, RequestError, Status};
pub use conn::{Endpoint, Identity, Incompatible, InvalidName};
pub use history::HistoryUpdate;
pub use protocol::{
    AttachMode, Attached, ClientInput, ClientKind, FEATURE_SAVED_SESSIONS, HistoryMode, InvalidSessionName, MISSING_CWD, Rejected,
    RenameFailure, RenameRefusal, SAVED_SESSIONS_MAX, SavedSession, SessionEvent, SessionMeta, Setting, validate_session_name,
};
pub use transcript::{Transcript, TranscriptError};
