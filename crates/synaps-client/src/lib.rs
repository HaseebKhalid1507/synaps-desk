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
//! // Resume a saved session by journal id or name (every v3 daemon): a
//! // create with `continue_session`. "no session named 'x'" or a journal
//! // live in another process comes back as `Event::Detached { reason }`.
//! let seq = client.resume("axel", CreateOptions::new().cwd(Cwd::Home))?;
//! // The saved sessions on disk, only from a daemon whose `Welcome.features`
//! // lists `saved_sessions` (`Status::Connected { features }`); otherwise
//! // `Err(RequestError::Unsupported)` and nothing is sent.
//! if client.supports(FEATURE_SAVED_SESSIONS) {
//!     client.saved_sessions(true, 200)?;               // -> Event::SavedSessions
//! }
//! // Name a live or parked session (checked against `[a-z0-9-]{1,40}` first).
//! // Without `force` an already named one is refused (`RenameFailed` with
//! // `RenameRefusal::Named` and its `current_name`): ask, then use `force`.
//! client.rename("<session id>", Some("my-task"), false)?;  // -> Event::Renamed / RenameFailed
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

pub use client::{Client, ClientClosed, ClientOptions, Command, CreateOptions, Cwd, Error, Event, RequestError, Status};
pub use conn::{Endpoint, Identity, Incompatible, InvalidName};
pub use history::HistoryUpdate;
pub use protocol::{
    AttachMode, Attached, ClientInput, ClientKind, FEATURE_SAVED_SESSIONS, HistoryMode, InvalidSessionName, Rejected, RenameFailure,
    RenameRefusal, SavedSession, SessionEvent, SessionMeta, validate_session_name,
};
pub use transcript::{Transcript, TranscriptError};
