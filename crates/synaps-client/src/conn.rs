//! One daemon connection: `Hello` → `Welcome`, then NDJSON frames.
//!
//! A reader task owns the read half and forwards decoded frames, so callers
//! can `select!` on frames and their own commands without cancel-safety
//! concerns.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::ipc;
use crate::protocol::*;

/// The daemon drops a connection that sends no `Hello` within 2 s; we wait
/// this long for its `Welcome`.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(5);

/// What a client says about itself in `Hello`.
#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub client_version: String,
    pub instance: String,
    pub history: HistoryMode,
    pub tail_items: usize,
}

impl Identity {
    pub fn desktop() -> Self {
        Self {
            name: "desktop".into(),
            client_version: format!("synaps-desktop/{}", env!("CARGO_PKG_VERSION")),
            instance: new_instance_id(),
            history: HistoryMode::Full,
            tail_items: 200,
        }
    }

    pub fn hello(&self, reconnect_of: Option<ClientReconnect>) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            client: ClientMeta {
                kind: "attach",
                terminal: None,
                instance: self.instance.clone(),
                history: self.history,
                tail_items: self.tail_items,
                capabilities: Capabilities { tool_queue: true, owner_reason_input: true },
                name: Some(self.name.clone()),
            },
            cwd: std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
            client_version: self.client_version.clone(),
            env: None,
            reconnect_of,
        }
    }
}

/// A random-enough instance id without pulling in a uuid crate.
pub fn new_instance_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0),
    );
    h.write_u32(std::process::id());
    let a = h.finish();
    h.write_u64(a);
    format!("desktop-{a:016x}{:016x}", h.finish())
}

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
    /// Connect and handshake. Fails on a refusal or a protocol mismatch.
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
                bail!("daemon refused: {message} ({reason})")
            }
            Some(Incoming::Frame(f)) => bail!("expected welcome, got {f:?}"),
            Some(Incoming::Failed(e)) => bail!("handshake failed: {e}"),
            Some(Incoming::Closed) | None => bail!("daemon closed the connection during the handshake"),
        };
        if welcome.protocol_version != PROTOCOL_VERSION {
            bail!(
                "daemon speaks protocol {} (daemon {}), this client {}",
                welcome.protocol_version,
                welcome.daemon_version,
                PROTOCOL_VERSION
            );
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
}

async fn read_loop(mut r: BufReader<ipc::ReadHalf>, tx: mpsc::UnboundedSender<Incoming>) {
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        let n = match (&mut r).take(MAX_FRAME_BYTES as u64 + 1).read_until(b'\n', &mut buf).await {
            Ok(n) => n,
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
