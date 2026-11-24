//! The model picker's list from what Synaps already keeps: the TUI's
//! starred models (`favorite_models` in the Synaps config, the daemon's
//! profile's) less the favourites of an OAuth provider with no login
//! (`synaps auth list --json`), and the session's own model, always. The
//! daemon still decides: a pick is `Set{Model}`, and it refuses or clamps.
//!
//! Where the two lists come from is the `pickers.source` setting
//! ([`Source`]): the daemon's machine (read here when the daemon is,
//! through the launcher's reader when it is not), this machine, a file or
//! a command.
//!
//! **A Synaps config holds API keys.** Of a config only the
//! `favorite_models` line's value is kept. A config, a command's output
//! or the reader's is read as it streams ([`Favorites`]): a line is held
//! only while it can still be `favorite_models = …`, so no other line is
//! ever held whole, logged, shown or said in an error. The daemon's host
//! sends that line only, and of `auth list` only the providers and labels
//! (`scripts/synaps-desktop-remote`: both cut down there, its stderr never
//! sent). No command's stderr is read, an error names the source, never
//! what it printed, and a command's whole process tree ends with the read
//! ([`run`]).
//!
//! A `{"command": …}` source is the user's own code: what it reads and
//! where it sends it is its business. Only its stdout reaches the desk,
//! filtered as above; a command that reads another machine's config should
//! cut it down there, as the launcher does.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The config key read, and the only one.
pub(crate) const KEY: &str = "favorite_models";
/// The setting (`settings.json`).
pub(crate) const SETTING: &str = "pickers.source";
/// Set by the launcher (`synaps-desktop-remote`) for a daemon on another
/// machine: the reader, run as `<reader> --read <host> <what> <args…>`,
/// and the host.
pub(crate) const ENV_READ: &str = "SYNAPS_DESKTOP_HOST_READ";
pub(crate) const ENV_HOST: &str = "SYNAPS_DESKTOP_HOST";
/// A read's deadline (an SSH round trip, a broker's answer).
const TIMEOUT: Duration = Duration::from_secs(10);
/// A logins listing at most.
const OUT_MAX: usize = 256 << 10;
/// A config (a file, a command's or the reader's output) at most.
const FILE_MAX: usize = 1 << 20;
/// A `favorite_models` value at most (longer: not one).
const VALUE_MAX: usize = 64 << 10;

/// Where one list comes from. Its `Debug` says which kind only: a path or
/// a command can carry anything (a token inline).
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum Where {
    /// The daemon's machine: this one when the daemon is provably here
    /// ([`crate::views::locality`]), else the launcher's reader
    /// ([`ENV_READ`]).
    #[default]
    DaemonHost,
    /// This machine, whichever the daemon's.
    Local,
    /// A file. Favourites: a Synaps config (its `favorite_models` line
    /// only); logins: what `synaps auth list --json` prints.
    File(PathBuf),
    /// A shell command whose stdout is read as the file would be.
    Command(String),
    /// None: no favourites (the session's model only), or no login filter.
    Off,
}

impl std::fmt::Debug for Where {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Where::DaemonHost => "DaemonHost",
            Where::Local => "Local",
            Where::File(_) => "File(..)",
            Where::Command(_) => "Command(..)",
            Where::Off => "Off",
        })
    }
}

/// `pickers.source`: `"daemon-host"` (the default), `"local"` or `"off"`
/// for both lists; `{"file": path}` or `{"command": cmd}` for the
/// favourites (logins from the daemon's machine); or
/// `{"favorites": W, "logins": W}`, each `W` any of those (missing:
/// `"daemon-host"`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Source {
    pub favorites: Where,
    pub logins: Where,
    /// The setting was not understood (the default stands in): why.
    pub warning: Option<String>,
}

impl Source {
    pub fn parse(v: Option<&Value>) -> Source {
        let Some(v) = v.filter(|v| !v.is_null()) else { return Source::default() };
        let bad = |why: &str| Source { warning: Some(format!("{SETTING}: {why}; using daemon-host")), ..Source::default() };
        match v {
            Value::String(_) => match place(v) {
                Ok(w) => Source { favorites: w.clone(), logins: w, warning: None },
                Err(e) => bad(e),
            },
            Value::Object(o) if o.contains_key("file") || o.contains_key("command") => match place(v) {
                Ok(favorites) => Source { favorites, ..Source::default() },
                Err(e) => bad(e),
            },
            Value::Object(o) => {
                if o.keys().any(|k| k != "favorites" && k != "logins") {
                    return bad("its keys are favorites and logins");
                }
                let get = |k: &str| o.get(k).map_or(Ok(Where::DaemonHost), place);
                match (get("favorites"), get("logins")) {
                    (Ok(favorites), Ok(logins)) => Source { favorites, logins, warning: None },
                    (Err(e), _) | (_, Err(e)) => bad(e),
                }
            }
            _ => bad(EXPECTED),
        }
    }
}

const EXPECTED: &str = "expected daemon-host, local, off, {\"file\": path} or {\"command\": cmd}";

/// One list's place. Never echoes what was written (a command can carry
/// anything).
fn place(v: &Value) -> Result<Where, &'static str> {
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match v {
        Value::String(s) => match s.as_str() {
            "daemon-host" => Ok(Where::DaemonHost),
            "local" => Ok(Where::Local),
            "off" => Ok(Where::Off),
            _ => Err(EXPECTED),
        },
        Value::Object(o) if o.len() == 1 => match (text(o.get("file")), text(o.get("command"))) {
            (Some(f), None) => Ok(Where::File(home(&f))),
            (None, Some(c)) => Ok(Where::Command(c)),
            _ => Err(EXPECTED),
        },
        _ => Err(EXPECTED),
    }
}

/// `~/…` from the home folder.
fn home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ => PathBuf::from(path),
    }
}

/// The launcher's reader for the daemon's machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reader {
    pub program: PathBuf,
    pub host: String,
}

impl Reader {
    /// From [`ENV_READ`] and [`ENV_HOST`] (both, or none).
    pub fn from_env() -> Option<Reader> {
        let program = std::env::var_os(ENV_READ).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())?;
        let host = std::env::var(ENV_HOST).ok().filter(|h| plain(h))?;
        Some(Reader { program, host })
    }

    /// Its two reads, and nothing else: no caller names a config key.
    /// `favorite_models`' lines of the profile's config.
    fn favorites(&self, profile: Option<&str>) -> Command {
        self.command(&["config-key", profile.unwrap_or("-"), KEY])
    }

    /// `auth list`'s providers and labels, from daemon `pid`'s CLI.
    fn auth_list(&self, pid: u32, profile: Option<&str>) -> Command {
        self.command(&["cli", &pid.to_string(), profile.unwrap_or("-"), "auth-list"])
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(&self.program);
        c.arg("--read").arg(&self.host).args(args);
        c
    }
}

/// What a list is read for: the daemon (its `Welcome`), where it runs.
#[derive(Clone, Debug)]
pub(crate) struct Ask {
    pub source: Source,
    pub pid: u32,
    pub profile: Option<String>,
    /// The daemon is on this machine (`Err`: why not).
    pub here: Result<(), String>,
    /// This machine's Synaps folder (`$SYNAPS_BASE_DIR`, `~/.synaps-cli`).
    pub base: PathBuf,
    pub reader: Option<Reader>,
}

/// A model as the picker lists it: sections are the id's provider.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Listed {
    pub id: String,
    pub label: String,
    /// Its section's name and key.
    pub provider: String,
    pub provider_key: String,
}

/// `provider/model` under `provider`, named `model`; a bare id is
/// Anthropic's, as the daemon reads it ([`crate::stats::same_model_id`]).
pub(crate) fn listed(id: &str) -> Listed {
    let (key, label) = match id.split_once('/') {
        Some((p, rest)) if !p.is_empty() && !rest.is_empty() => (p, rest),
        _ => ("anthropic", id),
    };
    Listed { id: id.to_string(), label: label.to_string(), provider: key.to_string(), provider_key: key.to_string() }
}

/// The picker's list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct List {
    pub models: Vec<Listed>,
    /// What it lacks and why, in one line.
    pub notice: Option<String>,
}

impl List {
    #[cfg(test)]
    pub fn of(ids: &[&str]) -> List {
        List { models: ids.iter().copied().map(listed).collect(), notice: None }
    }
}

/// The list for `ask` (blocking: a background thread). Never fails: what
/// could not be read is its notice.
pub(crate) fn load(ask: &Ask) -> List {
    let mut notes: Vec<String> = ask.source.warning.iter().cloned().collect();
    let read = match favorites(ask) {
        Ok(Some(ids)) => ids,
        Ok(None) => {
            notes.push(format!("no {KEY} {}: star models with /models in the TUI", ask.said(&ask.source.favorites)));
            Vec::new()
        }
        Err(e) => {
            notes.push(format!("favourites not read: {e}"));
            Vec::new()
        }
    };
    let mut ids: Vec<String> = Vec::new();
    for id in read {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if !ids.is_empty() {
        match logins(ask) {
            None => {}
            Some(Ok(l)) => {
                let mut gone: Vec<String> = Vec::new();
                let before = ids.len();
                ids.retain(|id| {
                    let ok = l.has_login(id);
                    let p = listed(id).provider_key;
                    if !ok && !gone.contains(&p) {
                        gone.push(p);
                    }
                    ok
                });
                if !gone.is_empty() {
                    let n = before - ids.len();
                    notes.push(format!("{n} favourite{} hidden: no login for {}", if n == 1 { "" } else { "s" }, gone.join(", ")));
                }
            }
            Some(Err(e)) => notes.push(format!("logins not checked: {e}")),
        }
    }
    List { models: ids.iter().map(|id| listed(id.as_str())).collect(), notice: (!notes.is_empty()).then(|| notes.join("; ")) }
}

impl Ask {
    /// The daemon's profile as a plain name (`None`: the default).
    fn profile(&self) -> Result<Option<&str>, String> {
        match self.profile.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            None => Ok(None),
            Some(p) if plain(p) && !p.starts_with('.') => Ok(Some(p)),
            Some(_) => Err("the daemon's profile is not a plain name".into()),
        }
    }

    /// The config this machine's Synaps reads for the daemon's profile.
    fn config(&self) -> Result<PathBuf, String> {
        Ok(crate::theme::read_path(&self.base, self.profile()?, "config"))
    }

    /// Where `w` is, for a note.
    fn said(&self, w: &Where) -> String {
        match (w, &self.here, &self.reader) {
            (Where::DaemonHost, Err(_), Some(r)) => format!("on {}", r.host),
            (Where::DaemonHost | Where::Local, ..) => {
                self.config().map(|p| format!("in {}", crate::named::home_short(&p.display().to_string()))).unwrap_or_default()
            }
            (Where::File(p), ..) => format!("in {}", crate::named::home_short(&p.display().to_string())),
            (Where::Command(_), ..) => format!("from the {SETTING} command"),
            (Where::Off, ..) => String::new(),
        }
    }

    /// The daemon's machine is not this one, and nothing here reads it.
    fn no_reader(&self) -> String {
        let why = self.here.as_ref().err().map_or("", String::as_str);
        format!(
            "the daemon is not on this machine ({why}) and nothing reads its host: launch through synaps-desktop-remote, or set {SETTING}"
        )
    }

    /// `synaps auth list --json` here: the daemon's own binary when the
    /// daemon is here (Linux: its `/proc/<pid>/exe`), else `synaps`.
    fn local_cli(&self) -> Result<Command, String> {
        let exe = PathBuf::from(format!("/proc/{}/exe", self.pid));
        let own = cfg!(target_os = "linux") && self.here.is_ok() && self.pid != 0 && exe.exists();
        let mut c = Command::new(if own { exe } else { PathBuf::from("synaps") });
        c.args(["auth", "list", "--json"]);
        match self.profile()? {
            Some(p) => c.env("SYNAPS_PROFILE", p),
            None => c.env_remove("SYNAPS_PROFILE"),
        };
        Ok(c)
    }
}

fn favorites(ask: &Ask) -> Result<Option<Vec<String>>, String> {
    let found = match &ask.source.favorites {
        Where::Off => return Ok(Some(Vec::new())),
        Where::Local => return read_favorites(&ask.config()?),
        Where::DaemonHost => match (&ask.here, &ask.reader) {
            (Ok(()), _) => return read_favorites(&ask.config()?),
            (Err(_), Some(r)) => run(r.favorites(ask.profile()?), &format!("the reader for {}", r.host), Favorites::default(), TIMEOUT)?,
            (Err(_), None) => return Err(ask.no_reader()),
        },
        Where::File(p) => return read_favorites(p),
        Where::Command(c) => run(shell(c), &format!("the {SETTING} command"), Favorites::default(), TIMEOUT)?,
    };
    Ok(found.finish())
}

fn logins(ask: &Ask) -> Option<Result<Logins, String>> {
    let out = match &ask.source.logins {
        Where::Off => return None,
        Where::Local => ask.local_cli().and_then(|c| run(c, "synaps auth list", Capped::default(), TIMEOUT)),
        Where::DaemonHost => match (&ask.here, &ask.reader) {
            (Ok(()), _) => ask.local_cli().and_then(|c| run(c, "synaps auth list", Capped::default(), TIMEOUT)),
            (Err(_), Some(r)) => ask
                .profile()
                .and_then(|p| run(r.auth_list(ask.pid, p), &format!("synaps auth list on {}", r.host), Capped::default(), TIMEOUT)),
            (Err(_), None) => Err(ask.no_reader()),
        },
        Where::File(p) => read_capped(p).map(Capped),
        Where::Command(c) => run(shell(c), &format!("the {SETTING} logins command"), Capped::default(), TIMEOUT),
    };
    Some(out.and_then(|mut b| {
        let l = Logins::parse(&b.0);
        b.0.fill(0);
        l
    }))
}

/// A config's `favorite_models`, read as it streams in (a file, a
/// command's stdout). A line is held only while it can still be that
/// key's: past its leading blanks, the key and the `=`, only that line's
/// value is held; any other line is dropped byte by byte as it arrives,
/// never whole. The last such line wins, as Synaps reads a config.
#[derive(Default)]
pub(crate) struct Favorites {
    at: At,
    /// The value of the `favorite_models` line being read.
    value: Vec<u8>,
    found: Option<Vec<String>>,
    read: usize,
}

/// Where in a line [`Favorites`] is.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum At {
    /// Its leading blanks.
    #[default]
    Lead,
    /// `KEY`'s first `n` bytes matched.
    Key(usize),
    /// The blanks between the key and `=`.
    Gap,
    /// After `=`: the value.
    Value,
    /// Not the key's line: dropped to its end.
    Skip,
}

impl Favorites {
    /// More of the stream. `Err`: more than [`FILE_MAX`] in all.
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.read += chunk.len();
        if self.read > FILE_MAX {
            self.value.clear();
            self.at = At::Skip;
            return Err(format!("more than {} KiB", FILE_MAX >> 10));
        }
        let key = KEY.as_bytes();
        for &b in chunk {
            if b == b'\n' {
                self.end_line();
                continue;
            }
            self.at = match self.at {
                At::Skip => At::Skip,
                At::Lead | At::Gap if b.is_ascii_whitespace() => self.at,
                At::Lead if b == key[0] => At::Key(1),
                At::Key(n) if b == key[n] => {
                    if n + 1 == key.len() {
                        At::Gap
                    } else {
                        At::Key(n + 1)
                    }
                }
                At::Gap if b == b'=' => At::Value,
                At::Value if self.value.len() < VALUE_MAX => {
                    self.value.push(b);
                    At::Value
                }
                _ => {
                    self.value.clear();
                    At::Skip
                }
            };
        }
        Ok(())
    }

    fn end_line(&mut self) {
        if self.at == At::Value {
            // Synaps' reading: split on commas, trimmed, empties dropped.
            if let Ok(v) = std::str::from_utf8(&self.value) {
                self.found = Some(v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect());
            }
        }
        self.value.clear();
        self.at = At::Lead;
    }

    /// The last `favorite_models` line's list (`None`: there was none).
    pub fn finish(mut self) -> Option<Vec<String>> {
        self.end_line();
        self.found
    }

    /// What it holds now (tests: a value of `favorite_models`, or nothing).
    #[cfg(test)]
    pub fn held(&self) -> &[u8] {
        &self.value
    }
}

impl Sink for Favorites {
    fn take(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.push(chunk)
    }
}

/// The favourites a `favorite_models = a, b` line names: `None` for any
/// other line (another key, a comment, not one).
#[cfg(test)]
pub(crate) fn favorite_line(line: &[u8]) -> Option<Vec<String>> {
    favorites_in(line)
}

/// The last `favorite_models` line's list in `text` (as Synaps: the last
/// wins); `None` without one.
#[cfg(test)]
pub(crate) fn favorites_in(text: &[u8]) -> Option<Vec<String>> {
    let mut f = Favorites::default();
    f.push(text).ok()?;
    f.finish()
}

/// A regular file, opened (`Ok(None)`: there is none). Not a FIFO or a
/// device: a read of one could block past any deadline.
fn open_regular(path: &Path) -> Result<Option<std::fs::File>, String> {
    let shown = || crate::named::home_short(&path.display().to_string());
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(format!("{}: not a regular file", shown())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {}", shown(), e.kind())),
    }
    std::fs::File::open(path).map(Some).map_err(|e| format!("{}: {}", shown(), e.kind()))
}

/// A config's `favorite_models` (`None`: no file, or no such line),
/// through [`Favorites`] a block at a time.
pub(crate) fn read_favorites(path: &Path) -> Result<Option<Vec<String>>, String> {
    let shown = || crate::named::home_short(&path.display().to_string());
    let Some(mut file) = open_regular(path)? else { return Ok(None) };
    let mut f = Favorites::default();
    let mut buf = [0u8; 8192];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("{}: {}", shown(), e.kind())),
        };
        let took = f.push(&buf[..n]);
        buf[..n].fill(0);
        took.map_err(|e| format!("{}: {e}", shown()))?;
    }
    Ok(f.finish())
}

/// A file's bytes, at most [`OUT_MAX`].
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let shown = || crate::named::home_short(&path.display().to_string());
    let Some(file) = open_regular(path)? else { return Err(format!("{}: {}", shown(), std::io::ErrorKind::NotFound)) };
    let mut out = Vec::new();
    file.take(OUT_MAX as u64 + 1).read_to_end(&mut out).map_err(|e| format!("{}: {}", shown(), e.kind()))?;
    if out.len() > OUT_MAX {
        out.fill(0);
        return Err(format!("{}: larger than {} KiB", shown(), OUT_MAX >> 10));
    }
    Ok(out)
}

/// `cmd` by this platform's shell.
fn shell(cmd: &str) -> Command {
    let mut c = if cfg!(windows) { Command::new("cmd") } else { Command::new("sh") };
    c.args([if cfg!(windows) { "/C" } else { "-c" }, cmd]);
    c
}

/// What a read keeps of a command's stdout, given it as it arrives.
/// `Err`: no more (too much); the command is stopped.
pub(crate) trait Sink: Send + 'static {
    fn take(&mut self, chunk: &[u8]) -> Result<(), String>;
}

/// All of it, at most [`OUT_MAX`] (a logins listing).
#[derive(Default)]
pub(crate) struct Capped(pub Vec<u8>);

impl Sink for Capped {
    fn take(&mut self, chunk: &[u8]) -> Result<(), String> {
        if self.0.len() + chunk.len() > OUT_MAX {
            self.0.fill(0);
            self.0.clear();
            return Err(format!("more than {} KiB", OUT_MAX >> 10));
        }
        self.0.extend_from_slice(chunk);
        Ok(())
    }
}

/// How a read of a command's stdout ended, if not at its end.
enum Stop {
    /// Not by the deadline.
    Late,
    Io(std::io::ErrorKind),
    /// The sink's: too much.
    Full(String),
}

/// `c` run with its stdout given to `sink` as it arrives, within `within`;
/// the sink once `c` exited 0. Its stderr is never read; an error says
/// `what` and how it ended only. `c` runs as a tree of its own (a process
/// group, a job object), in it before any of its code runs, or not at all:
/// at the deadline, or once the sink has had enough, the whole tree is
/// killed, and so is whatever it left running when it ends, so no
/// descendant outlives the read and no reader waits on one.
fn run<S: Sink>(mut c: Command, what: &str, sink: S, within: Duration) -> Result<S, String> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let deadline = Instant::now() + within;
    let (mut child, tree) = tree::spawn(&mut c).map_err(|e| format!("{what} {e}"))?;
    let stdout = child.stdout.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut sink = sink;
        let r = pump(stdout, &mut sink, deadline);
        let _ = tx.send(r.map(|()| sink));
    });
    let left = || deadline.saturating_duration_since(Instant::now());
    // The reader ends at the end of the output: every writer has closed
    // it, or was killed with the tree (and on Unix at the deadline anyway).
    let (read, late) = match rx.recv_timeout(left()) {
        Ok(r) => {
            let late = matches!(r, Err(Stop::Late));
            (Some(r), late)
        }
        Err(_) => {
            tree.kill(&mut child);
            (rx.recv_timeout(Duration::from_secs(2)).ok(), true)
        }
    };
    if !matches!(read, Some(Ok(_))) {
        tree.kill(&mut child);
    }
    // Its own end (not yet reaped: the tree's id stays its own).
    let mut killed = false;
    while !tree.exited(&mut child) {
        if left().is_zero() && !killed {
            tree.kill(&mut child);
            killed = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    tree.kill(&mut child);
    let status = child.wait().map_err(|e| format!("{what} did not end ({})", e.kind()))?;
    let late = late || killed;
    let secs = within.as_secs_f32();
    match read {
        None => Err(format!("{what} did not answer within {secs} s and left its output open")),
        Some(_) if late => Err(format!("{what} did not answer within {secs} s")),
        Some(Err(Stop::Full(e))) => Err(format!("{what} printed {e}")),
        Some(Err(Stop::Io(k))) => Err(format!("{what}: its output did not read ({k})")),
        Some(Err(Stop::Late)) => Err(format!("{what} did not answer within {secs} s")),
        Some(Ok(sink)) => {
            if status.success() {
                Ok(sink)
            } else {
                Err(match status.code() {
                    Some(255) => format!("{what} failed (exit 255: ssh could not reach the host?)"),
                    Some(n) => format!("{what} failed (exit {n})"),
                    None => format!("{what} was stopped by a signal"),
                })
            }
        }
    }
}

/// `out` into `sink` until its end, the sink's `Err` or `deadline` (on
/// Unix while it waits; elsewhere between reads, the job's kill ending a
/// read that waits).
/// Each block is cleared once given.
fn pump<S: Sink>(mut out: ChildStdout, sink: &mut S, deadline: Instant) -> Result<(), Stop> {
    let mut buf = [0u8; 8192];
    loop {
        #[cfg(unix)]
        if !tree::readable(&out, deadline) {
            return Err(Stop::Late);
        }
        #[cfg(not(unix))]
        if Instant::now() >= deadline {
            return Err(Stop::Late);
        }
        let n = match out.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Stop::Io(e.kind())),
        };
        let took = sink.take(&buf[..n]);
        buf[..n].fill(0);
        took.map_err(Stop::Full)?;
    }
}

/// A command's process tree.
#[cfg(unix)]
mod tree {
    use std::os::fd::AsRawFd;
    use std::process::{Child, ChildStdout, Command};
    use std::time::Instant;

    /// `c` started as the leader of a process group of its own, which its
    /// descendants join (set in the child before it execs). `Err`: why
    /// not, after "did not start".
    pub fn spawn(c: &mut Command) -> Result<(Child, Tree), String> {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
        let child = c.spawn().map_err(|e| format!("did not start ({})", e.kind()))?;
        let tree = Tree(child.id() as libc::pid_t);
        Ok((child, tree))
    }

    /// The group, by its leader's pid.
    pub struct Tree(libc::pid_t);

    impl Tree {
        /// SIGKILL to every process in the group, the leader too. Sound
        /// until the leader is reaped: till then its pid, the group's id,
        /// is not reused.
        pub fn kill(&self, child: &mut Child) {
            // SAFETY: a plain syscall; a group that is gone is ESRCH.
            unsafe { libc::kill(-self.0, libc::SIGKILL) };
            let _ = child.kill();
        }

        /// The leader has exited, left unreaped (`WNOWAIT`).
        pub fn exited(&self, _: &mut Child) -> bool {
            // SAFETY: `info` is ours, zeroed; `waitid` writes it.
            unsafe {
                let mut info: libc::siginfo_t = std::mem::zeroed();
                let r = libc::waitid(libc::P_PID, self.0 as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT);
                r != 0 || info.si_pid() != 0
            }
        }
    }

    /// `out` has something to read (or is closed) before `deadline`.
    pub fn readable(out: &ChildStdout, deadline: Instant) -> bool {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let mut p = libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ms = left.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
            // SAFETY: one pollfd, ours.
            match unsafe { libc::poll(&mut p, 1, ms) } {
                0 => continue,
                n if n > 0 => return true,
                _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
                // The read says what is wrong.
                _ => return true,
            }
        }
    }
}

/// A command's process tree.
#[cfg(windows)]
mod tree {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::{ChildExt, CommandExt};
    use std::process::{Child, Command};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED, ResumeThread};

    /// Why a command was not run: no job could hold it.
    pub const UNCONTAINED: &str = "was not run (no job object to hold its processes)";

    /// A test's stand-in for [`Tree::enrol`]: it fails, or is slow.
    #[cfg(test)]
    pub type Enrol = Box<dyn FnOnce(&Child) -> io::Result<Tree>>;

    #[cfg(test)]
    thread_local! {
        /// Taken by the next [`spawn`] on this thread.
        pub static ENROL: std::cell::RefCell<Option<Enrol>> = const { std::cell::RefCell::new(None) };
    }

    /// `c` started in a job of its own, which its descendants join, before
    /// any of its code runs: created suspended, put in the job, then its
    /// one thread resumed. No window. If the job can't be made or joined,
    /// or the thread resumed, it is killed unrun (it never runs outside a
    /// job). `Err`: why not, after the command's name.
    pub fn spawn(c: &mut Command) -> Result<(Child, Tree), String> {
        #[cfg(test)]
        if let Some(enrol) = ENROL.take() {
            return spawn_with(c, enrol);
        }
        spawn_with(c, Tree::enrol)
    }

    fn spawn_with(c: &mut Command, enrol: impl FnOnce(&Child) -> io::Result<Tree>) -> Result<(Child, Tree), String> {
        c.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = c.spawn().map_err(|e| format!("did not start ({})", e.kind()))?;
        match enrol(&child).and_then(|tree| resume(&child).map(|()| tree)) {
            Ok(tree) => Ok((child, tree)),
            Err(_) => {
                // Still suspended (it never ran), or in the job just
                // closed, which killed it.
                let _ = child.kill();
                let _ = child.wait();
                Err(UNCONTAINED.into())
            }
        }
    }

    /// The child's first thread, created suspended, resumed. std keeps its
    /// handle from `CreateProcessW` (`ChildExt::main_thread_handle`), so
    /// this is that thread and no other; no pid lookup. `Err` unless it
    /// was suspended once (by `CREATE_SUSPENDED`) and now runs.
    fn resume(child: &Child) -> io::Result<()> {
        // SAFETY: the thread handle std holds for `child`, live while it is.
        match unsafe { ResumeThread(child.main_thread_handle().as_raw_handle()) } {
            1 => Ok(()),
            u32::MAX => Err(io::Error::last_os_error()),
            n => Err(io::Error::other(format!("suspended {n} times"))),
        }
    }

    /// A job object holding the command and its descendants. Closing it
    /// (dropping this) kills what is left in it.
    pub struct Tree(HANDLE);

    impl Tree {
        /// A new job that kills what it holds once closed, with `child`
        /// (still suspended) in it.
        pub fn enrol(child: &Child) -> io::Result<Tree> {
            // SAFETY: a new unnamed job, ours, closed once (by `Drop`), on
            // error too; `limits` is ours, of the size given; the child's
            // handle is live.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let tree = Tree(job);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                if SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size) == 0
                    || AssignProcessToJobObject(job, child.as_raw_handle()) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(tree)
            }
        }

        /// Every process in the job, the command too.
        pub fn kill(&self, child: &mut Child) {
            // SAFETY: our job handle.
            unsafe { TerminateJobObject(self.0, 1) };
            let _ = child.kill();
        }

        pub fn exited(&self, child: &mut Child) -> bool {
            !matches!(child.try_wait(), Ok(None))
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: our job handle, closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// A plain word: what the reader takes as an argument (no shell on either
/// side reads anything into it).
fn plain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 255 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `synaps auth list --json`, the parts read: which providers sign in
/// (its `selectors`' keys, every OAuth provider) and the accounts'
/// providers and labels. Nothing else of it is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Logins {
    pub oauth: Vec<String>,
    pub accounts: Vec<(String, String)>,
}

impl Logins {
    pub fn parse(bytes: &[u8]) -> Result<Logins, String> {
        // Selectors' keys only: their values (an account a config names)
        // are skipped unread, as is every other field.
        #[derive(serde::Deserialize)]
        struct Out {
            accounts: Vec<Account>,
            #[serde(default)]
            selectors: std::collections::BTreeMap<String, serde::de::IgnoredAny>,
        }
        #[derive(serde::Deserialize)]
        struct Account {
            provider: String,
            #[serde(default)]
            label: String,
        }
        // The decoder's error is not passed on: it can quote the input.
        let out: Out = serde_json::from_slice(bytes).map_err(|_| "the answer is not `synaps auth list --json`'s".to_string())?;
        Ok(Logins {
            oauth: out.selectors.into_keys().collect(),
            accounts: out.accounts.into_iter().map(|a| (a.provider, a.label)).collect(),
        })
    }

    /// `id` can be used: its provider is not an OAuth one (an API key's,
    /// a local one: not listed there), or has an account (`p@label/…`:
    /// that account).
    pub fn has_login(&self, id: &str) -> bool {
        let key = listed(id).provider_key;
        let (base, label) = key.split_once('@').map_or((key.as_str(), None), |(b, l)| (b, Some(l)));
        let oauth = self.oauth.iter().any(|p| p == base) || self.accounts.iter().any(|(p, _)| p == base);
        !oauth || self.accounts.iter().any(|(p, l)| p == base && label.is_none_or(|want| want == l.as_str()))
    }
}

#[cfg(test)]
#[path = "model_list_tests.rs"]
pub(crate) mod tests;
