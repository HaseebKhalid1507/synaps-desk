//! One daemon connection: `Hello` → `Welcome`, then NDJSON frames.
//!
//! A reader task owns the read half and forwards decoded frames, so callers
//! can `select!` on frames and their own commands without cancel-safety
//! concerns.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::ipc;
use crate::protocol::*;

/// The daemon drops a connection that sends no `Hello` within 2 s; we wait
/// this long for its `Welcome`.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(5);
/// A read buffer that grew past this (one big `Attached`) is released after
/// the frame, back to [`FRAME_BUF_KEEP`] (as the daemon's `conn.rs` does).
const FRAME_BUF_SHRINK_ABOVE: usize = 1024 * 1024;
const FRAME_BUF_KEEP: usize = 64 * 1024;

/// Longest client name (`types::CLIENT_NAME_MAX`).
pub const CLIENT_NAME_MAX: usize = 32;

/// `types::validate_client_name`: 1–32 ASCII letters, digits, `-` or `_`,
/// starting with a letter or digit. The daemon refuses the handshake
/// otherwise.
pub fn validate_client_name(name: &str) -> Result<(), InvalidName> {
    let ok_first = name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let ok_rest = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if name.is_empty() || name.len() > CLIENT_NAME_MAX || !ok_first || !ok_rest {
        return Err(InvalidName(name.to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidName(pub String);

impl std::fmt::Display for InvalidName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid client name {:?}: use 1-{CLIENT_NAME_MAX} letters, digits, '-' or '_', starting with a letter or digit", self.0)
    }
}

impl std::error::Error for InvalidName {}

/// What a client says about itself in `Hello`. Build with [`Identity::new`]
/// (or [`Identity::desktop`]) and the `with_*` methods.
#[derive(Debug, Clone)]
pub struct Identity {
    name: String,
    kind: ClientKind,
    client_version: String,
    instance: String,
    history: HistoryMode,
    tail_items: usize,
}

impl Identity {
    /// A client called `name` (shown in the session's roster), kind
    /// `Attach`, `Full` history. Fails on a name the daemon would refuse.
    pub fn new(name: &str) -> Result<Self, InvalidName> {
        validate_client_name(name)?;
        Ok(Self {
            name: name.to_string(),
            kind: ClientKind::Attach,
            client_version: format!("synaps-client/{}", env!("CARGO_PKG_VERSION")),
            instance: new_instance_id(),
            history: HistoryMode::Full,
            tail_items: 200,
        })
    }

    pub fn desktop() -> Self {
        Self::new("desktop").expect("a valid name").with_version(format!("synaps-desktop/{}", env!("CARGO_PKG_VERSION")))
    }

    pub fn with_kind(mut self, kind: ClientKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_version(mut self, client_version: impl Into<String>) -> Self {
        self.client_version = client_version.into();
        self
    }

    /// `Digest` gets a digest plus a display tail instead of the history.
    /// [`crate::Transcript`] needs `Full` and refuses a digest attach, and
    /// the client's drift resync only runs in `Full`.
    pub fn with_history(mut self, history: HistoryMode, tail_items: usize) -> Self {
        self.history = history;
        self.tail_items = tail_items;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> ClientKind {
        self.kind
    }

    pub fn history(&self) -> HistoryMode {
        self.history
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    pub fn hello(&self, reconnect_of: Option<ClientReconnect>) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            client: ClientMeta {
                kind: self.kind,
                terminal: None,
                instance: self.instance.clone(),
                history: self.history,
                tail_items: self.tail_items,
                capabilities: Capabilities { tool_queue: true, owner_reason_input: true },
                name: Some(self.name.clone()),
            },
            // Only a default: created sessions get an explicit
            // `CreateConfig.cwd` (see `client::Cwd`).
            cwd: std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
            client_version: self.client_version.clone(),
            env: None,
            reconnect_of,
        }
    }
}

/// A random-enough instance id without pulling in a uuid crate.
pub fn new_instance_id() -> String {
    let (a, b) = random_pair();
    format!("desktop-{a:016x}{b:016x}")
}

/// Two hard-to-guess u64s (`RandomState` keys + time + pid).
pub(crate) fn random_pair() -> (u64, u64) {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.write_u32(std::process::id());
    let a = h.finish();
    h.write_u64(a);
    (a, h.finish())
}

/// The daemon speaks another protocol version (or refused ours): retrying
/// cannot help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incompatible {
    pub daemon_protocol: Option<u32>,
    pub client_protocol: u32,
    pub daemon_version: String,
}

impl std::fmt::Display for Incompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.daemon_protocol {
            Some(p) if self.daemon_version.is_empty() => write!(f, "daemon speaks protocol {p}, this client {}", self.client_protocol),
            Some(p) => write!(f, "daemon {} speaks protocol {p}, this client {}", self.daemon_version, self.client_protocol),
            None => write!(f, "daemon refused protocol {}", self.client_protocol),
        }
    }
}

impl std::error::Error for Incompatible {}

pub enum Incoming {
    Frame(DaemonFrame),
    /// Clean EOF.
    Closed,
    /// Read error or oversize frame; the connection is dead.
    Failed(String),
}

pub struct Conn {
    writer: ipc::WriteHalf,
    pub frames: mpsc::UnboundedReceiver<Incoming>,
    pub welcome: Welcome,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Conn {
    /// Connect and handshake. Fails on a refusal or a protocol mismatch
    /// (an [`Incompatible`] error, `downcast_ref`-able).
    pub async fn connect(path: &Path, hello: Hello) -> anyhow::Result<Conn> {
        let stream = ipc::connect(path).await.with_context(|| format!("connect {}", path.display()))?;
        let (r, mut writer) = tokio::io::split(stream);
        let line = encode_frame(&ClientFrame::Hello(hello)).map_err(|e| anyhow!(e))?;
        writer.write_all(line.as_bytes()).await?;
        let (tx, mut frames) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read_loop(BufReader::with_capacity(64 * 1024, r), tx));
        let first = tokio::time::timeout(WELCOME_TIMEOUT, frames.recv())
            .await
            .map_err(|_| anyhow!("no welcome from the daemon within {WELCOME_TIMEOUT:?}"))?;
        let welcome = match first {
            Some(Incoming::Frame(DaemonFrame::Welcome(w))) => w,
            Some(Incoming::Frame(DaemonFrame::Refused { reason, message })) => {
                if matches!(refuse_kind(&reason), "version" | "protocol") {
                    return Err(Incompatible {
                        daemon_protocol: reason.get("daemon_version").and_then(|v| v.as_u64()).map(|v| v as u32),
                        client_protocol: PROTOCOL_VERSION,
                        daemon_version: String::new(),
                    }
                    .into());
                }
                bail!("daemon refused: {message} ({})", refuse_kind(&reason))
            }
            Some(Incoming::Frame(DaemonFrame::Malformed { kind, error })) => bail!("could not decode the daemon's {kind}: {error}"),
            Some(Incoming::Frame(f)) => bail!("expected welcome, got {f:?}"),
            Some(Incoming::Failed(e)) => bail!("handshake failed: {e}"),
            Some(Incoming::Closed) | None => bail!("daemon closed the connection during the handshake"),
        };
        if welcome.protocol_version != PROTOCOL_VERSION {
            return Err(Incompatible {
                daemon_protocol: Some(welcome.protocol_version),
                client_protocol: PROTOCOL_VERSION,
                daemon_version: welcome.daemon_version,
            }
            .into());
        }
        Ok(Conn { writer, frames, welcome, reader })
    }

    pub async fn send(&mut self, frame: &ClientFrame) -> anyhow::Result<()> {
        let line = encode_frame(frame).map_err(|e| anyhow!(e))?;
        self.writer.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Next frame; `Ok(None)` on clean EOF.
    pub async fn recv(&mut self) -> anyhow::Result<Option<DaemonFrame>> {
        match self.frames.recv().await {
            Some(Incoming::Frame(f)) => Ok(Some(f)),
            Some(Incoming::Failed(e)) => Err(anyhow!(e)),
            Some(Incoming::Closed) | None => Ok(None),
        }
    }

    /// `Bye`, best effort, then drop.
    pub async fn bye(mut self) {
        let _ = tokio::time::timeout(Duration::from_millis(200), self.send(&ClientFrame::Bye)).await;
        let _ = tokio::time::timeout(Duration::from_millis(200), self.writer.shutdown()).await;
    }
}

/// Where the daemon is: an explicit socket, or a profile whose registry
/// entry (`daemon[-profile].json`) is re-read on every (re)connect, so a
/// daemon that moved its socket is found again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Socket(PathBuf),
    Profile(Option<String>),
}

impl Endpoint {
    /// `$SYNAPS_PROFILE` (as the TUI's `config::get_profile`), else default.
    pub fn from_env() -> Self {
        Self::profile_with(None, |k| std::env::var(k).ok())
    }

    /// `Some(profile)`, else [`Endpoint::from_env`].
    pub fn profile(profile: Option<&str>) -> Self {
        Self::profile_with(profile, |k| std::env::var(k).ok())
    }

    fn profile_with(profile: Option<&str>, env: impl Fn(&str) -> Option<String>) -> Self {
        match profile {
            Some(p) => Endpoint::Profile(Some(p.to_string())),
            None => Endpoint::Profile(env("SYNAPS_PROFILE").filter(|p| !p.trim().is_empty())),
        }
    }

    pub fn resolve(&self) -> PathBuf {
        match self {
            Endpoint::Socket(p) => p.clone(),
            Endpoint::Profile(p) => crate::discovery::resolve_socket(None, p.as_deref()),
        }
    }
}

async fn read_loop(mut r: BufReader<ipc::ReadHalf>, tx: mpsc::UnboundedSender<Incoming>) {
    let mut buf = Vec::with_capacity(FRAME_BUF_KEEP);
    loop {
        if buf.capacity() > FRAME_BUF_SHRINK_ABOVE {
            buf = Vec::with_capacity(FRAME_BUF_KEEP);
        }
        buf.clear();
        let n = match (&mut r).take(MAX_FRAME_BYTES as u64 + 1).read_until(b'\n', &mut buf).await {
            Ok(n) => n,
            // Windows reports a closed pipe as BrokenPipe: the same clean EOF
            // a Unix socket gives (Synaps' `EofOnBrokenPipe`).
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::BrokenPipe => 0,
            Err(e) => {
                let _ = tx.send(Incoming::Failed(e.to_string()));
                return;
            }
        };
        if n == 0 {
            let _ = tx.send(Incoming::Closed);
            return;
        }
        if buf.last() != Some(&b'\n') {
            let why = if buf.len() > MAX_FRAME_BYTES { "frame over the 64 MiB cap" } else { "truncated frame at EOF" };
            let _ = tx.send(Incoming::Failed(why.into()));
            return;
        }
        let line = match std::str::from_utf8(&buf[..buf.len() - 1]) {
            Ok(l) => l.trim_end_matches('\r'),
            Err(_) => {
                let _ = tx.send(Incoming::Failed("frame is not UTF-8".into()));
                return;
            }
        };
        if line.is_empty() {
            continue;
        }
        match decode_frame(line) {
            Ok(f) => {
                if tx.send(Incoming::Frame(f)).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Incoming::Failed(e));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_names_follow_the_daemon_rule() {
        for ok in ["desktop", "avatar-1", "a", "A_b-9", &"x".repeat(32)] {
            assert!(Identity::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-lead", "_lead", "has space", "dot.ted", "émoji", &"x".repeat(33)] {
            assert!(Identity::new(bad).is_err(), "{bad}");
        }
        let id = Identity::new("avatar").unwrap().with_kind(ClientKind::Attach);
        let v = serde_json::to_value(id.hello(None)).unwrap();
        assert_eq!(v["client"]["kind"], "attach");
        assert_eq!(v["client"]["name"], "avatar");
    }

    /// L6: no explicit profile means `$SYNAPS_PROFILE` (blank: the default
    /// daemon); an explicit one wins over it.
    #[test]
    fn endpoint_profile_falls_back_to_the_env() {
        let env = |v: &'static str| move |k: &str| (k == "SYNAPS_PROFILE").then(|| v.to_string());
        assert_eq!(Endpoint::profile_with(None, env("work")), Endpoint::Profile(Some("work".into())));
        assert_eq!(Endpoint::profile_with(Some("x"), env("work")), Endpoint::Profile(Some("x".into())));
        assert_eq!(Endpoint::profile_with(None, env("  ")), Endpoint::Profile(None));
        assert_eq!(Endpoint::profile_with(None, |_: &str| None), Endpoint::Profile(None));
    }
}
