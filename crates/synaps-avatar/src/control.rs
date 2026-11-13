//! Dev control channel: a Unix socket with newline commands. It stands in for the
//! session stream phase 2 brings; both produce the same `Command`s, so the
//! command → state boundary stays in `view::Avatar::apply`.

use crate::anim::Mood;
use anyhow::{Context as _, Result, bail};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Command {
    Mood(Mood),
    Clip(String),
    Say(String),
    /// Put the avatar's ground point (feet) at root pixel (x, y).
    Pos(i32, i32),
    /// One-line JSON snapshot of the avatar's state.
    State,
    Quit,
    /// Internal: the compositor appeared or went away; re-apply shapes.
    Reshape,
}

impl Command {
    pub fn parse(line: &str) -> Result<Command> {
        let line = line.trim();
        let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        Ok(match verb {
            "mood" => Command::Mood(Mood::parse(rest).with_context(|| {
                let all: Vec<_> = Mood::ALL.iter().map(|m| m.key()).collect();
                format!("unknown mood `{rest}` (one of: {})", all.join(", "))
            })?),
            "clip" if !rest.is_empty() => Command::Clip(rest.to_owned()),
            // `\n` in the line stands for a newline (the protocol is one command per line).
            "say" if !rest.is_empty() => Command::Say(rest.replace("\\n", "\n")),
            "pos" => {
                let mut it = rest.split_whitespace().map(str::parse::<i32>);
                match (it.next(), it.next(), it.next()) {
                    (Some(Ok(x)), Some(Ok(y)), None) => Command::Pos(x, y),
                    _ => bail!("usage: pos <x> <y>"),
                }
            }
            "state" => Command::State,
            "quit" => Command::Quit,
            _ => bail!("unknown command `{line}` (mood <name> | clip <id> | say <text> | pos <x> <y> | state | quit)"),
        })
    }
}

pub type Reply = std::sync::mpsc::Sender<String>;

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("synaps-avatar.sock")
}

/// Bind the socket. A live socket means another avatar is running: refuse. A dead one
/// (left by a killed process) is replaced.
pub fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("another synaps-avatar is already running ({} answers)", path.display());
        }
        std::fs::remove_file(path).with_context(|| format!("cannot remove stale {}", path.display()))?;
    }
    UnixListener::bind(path).with_context(|| format!("cannot bind {}", path.display()))
}

/// Serve connections on a thread; every parsed command goes to `tx` with a reply channel.
pub fn serve(listener: UnixListener, tx: async_channel::Sender<(Command, Reply)>) {
    std::thread::Builder::new()
        .name("control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || handle(stream, tx));
            }
        })
        .expect("spawn control thread");
}

fn handle(stream: UnixStream, tx: async_channel::Sender<(Command, Reply)>) {
    let Ok(mut out) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match Command::parse(&line) {
            Ok(cmd) => {
                let (rtx, rrx) = std::sync::mpsc::channel();
                if tx.send_blocking((cmd, rtx)).is_err() {
                    return;
                }
                rrx.recv_timeout(Duration::from_secs(3)).unwrap_or_else(|_| "ok".into())
            }
            Err(e) => format!("error: {e:#}"),
        };
        if writeln!(out, "{reply}").is_err() {
            return;
        }
    }
}
