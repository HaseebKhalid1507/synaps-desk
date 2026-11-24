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
//! `favorite_models` line is kept: every other line is dropped as it is
//! read ([`favorite_line`]), never held, logged, shown or said in an
//! error, and the daemon's host sends that line only
//! (`scripts/synaps-desktop-remote`, filtered there). No command's stderr
//! is read, and an error names the source, never what it printed.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
/// A command's stdout at most.
const OUT_MAX: usize = 256 << 10;
/// A file read at most.
const FILE_MAX: u64 = 1 << 20;

/// Where one list comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    match &ask.source.favorites {
        Where::Off => Ok(Some(Vec::new())),
        Where::Local => read_favorites(&ask.config()?),
        Where::DaemonHost => match (&ask.here, &ask.reader) {
            (Ok(()), _) => read_favorites(&ask.config()?),
            (Err(_), Some(r)) => {
                let out = run(r.command(&["config-key", ask.profile()?.unwrap_or("-"), KEY]), &format!("the reader for {}", r.host))?;
                Ok(favorites_in(&out))
            }
            (Err(_), None) => Err(ask.no_reader()),
        },
        Where::File(p) => read_favorites(p),
        Where::Command(c) => Ok(favorites_in(&run(shell(c), &format!("the {SETTING} command"))?)),
    }
}

fn logins(ask: &Ask) -> Option<Result<Logins, String>> {
    let out = match &ask.source.logins {
        Where::Off => return None,
        Where::Local => ask.local_cli().and_then(|c| run(c, "synaps auth list")),
        Where::DaemonHost => match (&ask.here, &ask.reader) {
            (Ok(()), _) => ask.local_cli().and_then(|c| run(c, "synaps auth list")),
            (Err(_), Some(r)) => ask.profile().and_then(|p| {
                let pid = ask.pid.to_string();
                run(r.command(&["cli", &pid, p.unwrap_or("-"), "auth-list"]), &format!("synaps auth list on {}", r.host))
            }),
            (Err(_), None) => Err(ask.no_reader()),
        },
        Where::File(p) => read_capped(p),
        Where::Command(c) => run(shell(c), &format!("the {SETTING} logins command")),
    };
    Some(out.and_then(|b| Logins::parse(&b)))
}

/// The favourites a `favorite_models = a, b` line names: `None` for any
/// other line (another key, a comment, not one). Synaps' own reading: the
/// key trimmed, the values split on commas, trimmed, empties dropped.
pub(crate) fn favorite_line(line: &[u8]) -> Option<Vec<String>> {
    let start = line.iter().position(|b| !b.is_ascii_whitespace())?;
    let rest = line[start..].strip_prefix(KEY.as_bytes())?;
    let eq = rest.iter().position(|b| !b.is_ascii_whitespace())?;
    let value = rest[eq..].strip_prefix(b"=")?;
    let value = std::str::from_utf8(value).ok()?;
    Some(value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
}

/// The last `favorite_models` line's list in `text` (as Synaps: the last
/// wins); `None` without one.
pub(crate) fn favorites_in(text: &[u8]) -> Option<Vec<String>> {
    text.split(|&b| b == b'\n').rev().find_map(favorite_line)
}

/// A config's `favorite_models` (`None`: no file, or no such line). Read
/// a line at a time; a line that is not that key's goes as it is read.
pub(crate) fn read_favorites(path: &Path) -> Result<Option<Vec<String>>, String> {
    let shown = || crate::named::home_short(&path.display().to_string());
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {}", shown(), e.kind())),
    };
    let mut r = BufReader::new(file.take(FILE_MAX));
    let (mut line, mut found) = (Vec::new(), None);
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(v) = favorite_line(&line) {
                    found = Some(v);
                }
            }
            Err(e) => {
                line.clear();
                return Err(format!("{}: {}", shown(), e.kind()));
            }
        }
    }
    line.clear();
    Ok(found)
}

/// A file's bytes, at most [`OUT_MAX`].
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let shown = || crate::named::home_short(&path.display().to_string());
    let mut out = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(OUT_MAX as u64 + 1).read_to_end(&mut out))
        .map_err(|e| format!("{}: {}", shown(), e.kind()))?;
    if out.len() > OUT_MAX {
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

/// `c`'s stdout once it exits 0 within [`TIMEOUT`], at most [`OUT_MAX`].
/// Its stderr is never read; an error says `what` and how it ended only.
fn run(mut c: Command, what: &str) -> Result<Vec<u8>, String> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = c.spawn().map_err(|e| format!("{what} did not start ({})", e.kind()))?;
    let mut stdout = child.stdout.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let read = (&mut stdout).take(OUT_MAX as u64 + 1).read_to_end(&mut buf);
        // The rest drained (a full pipe would hold the child).
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = tx.send(read.map(|_| buf));
    });
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{what} did not answer within {} s", TIMEOUT.as_secs()));
            }
        }
    };
    let out = match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("{what}: its output did not read ({})", e.kind())),
        Err(_) => return Err(format!("{what} left its output open")),
    };
    if !status.success() {
        return Err(match status.code() {
            Some(255) => format!("{what} failed (exit 255: ssh could not reach the host?)"),
            Some(n) => format!("{what} failed (exit {n})"),
            None => format!("{what} was stopped by a signal"),
        });
    }
    if out.len() > OUT_MAX {
        return Err(format!("{what} printed more than {} KiB", OUT_MAX >> 10));
    }
    Ok(out)
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
        #[derive(serde::Deserialize)]
        struct Out {
            accounts: Vec<Account>,
            #[serde(default)]
            selectors: serde_json::Map<String, Value>,
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
            oauth: out.selectors.keys().cloned().collect(),
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
