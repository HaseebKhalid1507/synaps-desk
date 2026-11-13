//! Thin wire client for the Synaps daemon. No GUI dependencies; builds on
//! Linux, macOS and Windows (Unix socket / named pipe).
//!
//! # Public API (spike: may change)
//!
//! ```no_run
//! use synaps_client::{Client, Event, Identity};
//! // Discover `~/.synaps-cli/run/daemon-<profile>.json` and connect on a
//! // background thread (its own small tokio runtime).
//! let (client, events) = Client::connect(Some("desktopproto"), Identity::desktop());
//! client.list_sessions();          // -> Event::Sessions (also polled every 3 s)
//! client.create(None);             // or client.attach("<session id>") -> Event::Attached
//! client.submit("hello");          // -> Event::Session { event, .. } stream, then Idle
//! client.abort();                  // cancel the running turn
//! while let Ok(ev) = events.recv_blocking() {
//!     match ev {
//!         Event::Session { event, .. } => { /* fold into a transcript::Transcript */ }
//!         Event::Status(_) | Event::Sessions(_) | Event::Attached { .. } => {}
//!         _ => {}
//!     }
//! }
//! ```
//!
//! `events` is an `async_channel::Receiver`, so a GUI executor can `.await`
//! it. `Event::Attached` is a full mirror (replace, never merge); after a
//! `synaps daemon reload` the client reconnects and re-attaches by itself
//! and sends `Attached { reconnect: true, .. }`.
//!
//! Modules:
//! - [`discovery`]: run dir + `daemon[-profile].json`
//! - [`ipc`]: Unix socket / Windows named pipe (pipe name ported from Synaps)
//! - [`protocol`]: tolerant serde mirrors of the v3 frames
//! - [`conn`]: one handshaken connection
//! - [`client`]: the connection thread (control + attached session, reload reconnect)
//! - [`transcript`]: history + live events folded into display rows
//! - [`synthetic`]: deterministic load for benches

pub mod client;
pub mod conn;
pub mod discovery;
pub mod ipc;
pub mod protocol;
pub mod synthetic;
pub mod transcript;

pub use client::{Client, Command, Event, Status};
pub use conn::Identity;
pub use protocol::{SessionEvent, SessionMeta};
pub use transcript::Transcript;
