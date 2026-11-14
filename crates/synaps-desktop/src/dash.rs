//! synaps-dash on the daemon's machine: the one channel Settings
//! (`/api/config`) and Usage (`/api/telemetry/*`) read, with no daemon
//! change (`usage-design/spec.md` section 6).
//!
//! Where it is: a local daemon's dash writes `<run_dir>/synaps-dash[-<profile>].url`
//! (0600, same user); a remote one is named by `--dash URL|FILE`, else
//! `$SYNAPS_DASH_URL`, else the settings file's `dash_url`.
//!
//! The token (`?token=…`, new at every daemon start) gates the dash's config
//! writes and its `/ws` bridge (running agents, answering their prompts), so
//! it is taken only from places other local users cannot read: a URL file,
//! `$SYNAPS_DASH_URL`, or a URL pasted into `dash_url` (kept for this run,
//! saved without it). Never from the command line (`/proc/<pid>/cmdline` is
//! world-readable): `--dash` takes a URL without one, or a URL file. It is
//! sent as the `synaps_dash` cookie, never written, logged or shown.
//!
//! Requests are plain HTTP/1.1 to loopback only (`Connection: close`),
//! blocking, each with a deadline: callers run them on a background thread.
//! A `POST` also sends `Origin: http://<Host>`, which the dash checks. No TLS
//! and no client crate: the dash only listens on 127.0.0.1, a remote one is
//! reached through a forwarded port, and any other host is refused (the
//! cookie would cross the network in the clear).

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// The URL variable a launcher sets for a remote daemon's dash.
pub const ENV_DASH: &str = "SYNAPS_DASH_URL";
const CONNECT: Duration = Duration::from_secs(2);
/// One request's whole time: connect, send, read to the end.
pub const REQUEST: Duration = Duration::from_secs(10);
/// A reply larger than this is refused (the largest, `latency` over all
/// time, is about 60 KB).
const MAX_REPLY: usize = 2 << 20;

/// A dash: host, port and token. `Debug` never prints the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    token: String,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").field("host", &self.host).field("port", &self.port).field("token", &"<redacted>").finish()
    }
}

impl Target {
    /// `http://host:port[/path][?token=hex]` (a bare `host:port` too).
    pub fn parse(url: &str) -> Result<Target, String> {
        let url = url.trim();
        let rest = match url.split_once("://") {
            Some(("http", r)) => r,
            Some((scheme, _)) => return Err(format!("{scheme}:// is not supported: synaps-dash speaks plain http on loopback")),
            None => url,
        };
        let (authority, tail) = rest.find(['/', '?']).map_or((rest, ""), |i| rest.split_at(i));
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("bad port in {}", strip_token(url)))?),
            None => (authority, 80),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) {
            return Err(format!("bad host in {}", strip_token(url)));
        }
        if !loopback(host) {
            return Err(format!(
                "{host} is not loopback: synaps-dash is reached on 127.0.0.1 only (forward its port: ssh -L 7717:127.0.0.1:7717)"
            ));
        }
        let token = tail
            .split_once('?')
            .map(|(_, q)| q)
            .into_iter()
            .flat_map(|q| q.split('&'))
            .find_map(|kv| kv.strip_prefix("token="))
            .unwrap_or("")
            .to_string();
        if !token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("the token is not hex".into());
        }
        Ok(Target { host: host.to_string(), port, token })
    }

    /// What may be shown: the URL without its token.
    pub fn shown(&self) -> String {
        if self.host.contains(':') {
            format!("http://[{}]:{}", self.host, self.port)
        } else {
            format!("http://{}:{}", self.host, self.port)
        }
    }

    pub fn has_token(&self) -> bool {
        !self.token.is_empty()
    }
}

/// `localhost`, 127.0.0.0/8 or `::1`.
pub fn loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `--dash`'s value: a URL without a token, or a file holding the dash's
/// URL (as its own URL file does). A token on the command line is refused.
pub fn flag_target(v: &str) -> Result<Target, String> {
    let path = Path::new(v.trim());
    if path.is_file() {
        let text = std::fs::read_to_string(path).map_err(|e| format!("--dash {}: {e}", path.display()))?;
        return Target::parse(text.lines().next().unwrap_or(""));
    }
    if flag_has_token(v) {
        return Err(FLAG_TOKEN.into());
    }
    Target::parse(v)
}

/// Why `--dash` takes no token (said at startup and in Settings).
pub const FLAG_TOKEN: &str =
    "--dash takes no token (any local user can read a command line): pass the dash's URL file (--dash FILE) or set $SYNAPS_DASH_URL";

/// A `--dash` value carries `token=` (refused before anything starts).
pub fn flag_has_token(v: &str) -> bool {
    !Path::new(v.trim()).is_file() && v.contains("token=")
}

/// A URL as it may be shown or saved: its query (the token) dropped.
pub fn strip_token(url: &str) -> String {
    url.trim().split('?').next().unwrap_or("").trim_end_matches('/').to_string()
}

/// Where the dash's URL came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The local dash's own file.
    UrlFile(PathBuf),
    Flag,
    Env,
    Setting,
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::UrlFile(p) => crate::named::home_short(&p.display().to_string()),
            Source::Flag => "--dash".into(),
            Source::Env => format!("${ENV_DASH}"),
            Source::Setting => "dash_url in settings.json".into(),
        }
    }
}

/// The file a local dash writes its URL and token to.
pub fn url_file(run_dir: &Path, profile: Option<&str>) -> PathBuf {
    run_dir.join(match profile.filter(|p| !p.is_empty()) {
        Some(p) => format!("synaps-dash-{p}.url"),
        None => "synaps-dash.url".into(),
    })
}

/// Where to find the dash. Local daemon: `--dash`, else its URL file.
/// Remote: `--dash`, else `$SYNAPS_DASH_URL`, else `dash_url`. `Err`: a URL
/// was given and is not one (said in Settings). Only a [`Source::UrlFile`]
/// dash can ever write config: `--dash` and the rest are read-only.
pub fn discover(
    local: bool,
    run_dir: &Path,
    profile: Option<&str>,
    flag: Option<&str>,
    env: Option<&str>,
    setting: Option<&str>,
) -> Option<Result<(Target, Source), String>> {
    fn given(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    if let Some(u) = given(flag) {
        return Some(flag_target(u).map(|t| (t, Source::Flag)));
    }
    if local {
        let path = url_file(run_dir, profile);
        let text = std::fs::read_to_string(&path).ok()?;
        return Some(Target::parse(text.lines().next().unwrap_or("")).map(|t| (t, Source::UrlFile(path))));
    }
    if let Some(u) = given(env) {
        return Some(Target::parse(u).map(|t| (t, Source::Env)));
    }
    given(setting).map(|u| Target::parse(u).map(|t| (t, Source::Setting)))
}

/// Why a request got no usable answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// No connection (nothing listening, a dropped forward, a timeout).
    Unreachable(String),
    /// 401: the daemon restarted and the token with it.
    Token,
    /// 401 to a URL that carries no token.
    NoToken,
    /// 503: the telemetry store is still starting (or off).
    Starting(String),
    /// Any other refusal, with the dash's own message.
    Http(u16, String),
    /// Not the JSON it should be.
    Bad(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Unreachable(e) => write!(f, "synaps-dash is not reachable: {e}"),
            Fail::Token => f.write_str("synaps-dash token changed: reconnect"),
            Fail::NoToken => f.write_str("synaps-dash wants its token and this URL has none"),
            Fail::Starting(m) if m.is_empty() => f.write_str("telemetry is starting"),
            Fail::Starting(m) => f.write_str(m),
            Fail::Http(_, m) => f.write_str(m),
            Fail::Bad(e) => write!(f, "synaps-dash answered something unexpected: {e}"),
        }
    }
}

/// `GET path` → the body (status 200), within [`REQUEST`] and by `by`.
pub fn get(t: &Target, path: &str, by: Option<Instant>) -> Result<String, Fail> {
    answer(t, request(t, "GET", path, None, deadline(by)).map_err(Fail::Unreachable)?)
}

/// `POST path` with a JSON body → the body (status 200).
pub fn post_json(t: &Target, path: &str, body: &str) -> Result<String, Fail> {
    answer(t, request(t, "POST", path, Some(body), deadline(None)).map_err(Fail::Unreachable)?)
}

/// `get` and decode.
pub fn get_json<T: serde::de::DeserializeOwned>(t: &Target, path: &str) -> Result<T, Fail> {
    get_json_by(t, path, None)
}

/// `get_json` that also ends by `by` (a whole load's deadline).
pub fn get_json_by<T: serde::de::DeserializeOwned>(t: &Target, path: &str, by: Option<Instant>) -> Result<T, Fail> {
    serde_json::from_str(&get(t, path, by)?).map_err(|e| Fail::Bad(format!("{path}: {e}")))
}

/// A request's deadline: [`REQUEST`] from now, or `by` if sooner.
fn deadline(by: Option<Instant>) -> Instant {
    let own = Instant::now() + REQUEST;
    by.map_or(own, |b| b.min(own))
}

fn answer(t: &Target, (status, body): (u16, String)) -> Result<String, Fail> {
    let said = || {
        serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| crate::app::one_line(&body, 160))
    };
    match status {
        200 => Ok(body),
        401 if !t.has_token() => Err(Fail::NoToken),
        401 => Err(Fail::Token),
        503 => Err(Fail::Starting(said())),
        s => {
            let m = said();
            Err(Fail::Http(s, if m.is_empty() { format!("HTTP {s}") } else { m }))
        }
    }
}

/// The time left before `by`, or the error that it is over.
fn left(by: Instant) -> Result<Duration, String> {
    by.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or_else(|| "timed out".to_string())
}

/// One request on its own connection, done by `by`: `(status, body)`.
/// Only loopback addresses are dialled, whatever the host resolves to.
fn request(t: &Target, method: &str, path: &str, body: Option<&str>, by: Instant) -> Result<(u16, String), String> {
    let addrs: Vec<_> = (t.host.as_str(), t.port).to_socket_addrs().map_err(|e| e.to_string())?.filter(|a| a.ip().is_loopback()).collect();
    let mut last = format!("{} does not resolve to a loopback address", t.shown());
    let mut stream = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT.min(left(by)?)) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let mut s = stream.ok_or(last)?;
    s.set_write_timeout(Some(left(by)?)).map_err(|e| e.to_string())?;
    let authority = if t.host.contains(':') { format!("[{}]:{}", t.host, t.port) } else { format!("{}:{}", t.host, t.port) };
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nUser-Agent: synaps-desktop\r\nConnection: close\r\n"
    );
    if t.has_token() {
        head.push_str(&format!("Cookie: synaps_dash={}\r\n", t.token));
    }
    if let Some(b) = body {
        head.push_str(&format!("Origin: http://{authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    if let Some(b) = body {
        s.write_all(b.as_bytes()).map_err(|e| e.to_string())?;
    }
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 << 10];
    loop {
        s.set_read_timeout(Some(left(by)?)).map_err(|e| e.to_string())?;
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > MAX_REPLY {
                    return Err("the reply is too large".into());
                }
                if complete(&raw) {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err("timed out".into()),
            Err(e) => return Err(e.to_string()),
        }
    }
    parse_response(raw)
}

/// The reply is all here: its `Content-Length` body, or a chunked body's
/// last chunk (else it ends when the dash closes).
fn complete(raw: &[u8]) -> bool {
    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    if head.contains("transfer-encoding: chunked") {
        return raw.ends_with(b"\r\n0\r\n\r\n") || raw[end + 4..].starts_with(b"0\r\n\r\n");
    }
    head.lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .is_some_and(|n| raw.len() >= end + 4 + n)
}

/// An HTTP/1.1 response read to its end: `(status, body)`. Chunked or
/// `Content-Length` bodies; else everything after the head. The body is
/// cut out of `raw` in place (no copy, but a chunked one's).
pub(crate) fn parse_response(mut raw: Vec<u8>) -> Result<(u16, String), String> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no HTTP head in the reply")?;
    let head = std::str::from_utf8(&raw[..end]).map_err(|_| "the reply's head is not text")?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.strip_prefix("HTTP/1."))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("not an HTTP reply")?;
    let (mut chunked, mut length) = (false, None);
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if k.eq_ignore_ascii_case("content-length") {
            length = v.parse::<usize>().ok();
        }
    }
    let body = if chunked {
        dechunk(&raw[end + 4..])?
    } else {
        if let Some(n) = length {
            if raw.len() < end + 4 + n {
                return Err("the reply ended early".into());
            }
            raw.truncate(end + 4 + n);
        }
        raw.drain(..end + 4);
        raw
    };
    Ok((status, String::from_utf8(body).map_err(|_| "the reply is not UTF-8")?))
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(b.len());
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").ok_or("a chunk has no size line")?;
        let size = std::str::from_utf8(&b[..eol]).ok().and_then(|s| usize::from_str_radix(s.split(';').next()?.trim(), 16).ok());
        let size = size.ok_or("bad chunk size")?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(b.get(..size).ok_or("the reply ended inside a chunk")?);
        b = b.get(size + 2..).ok_or("the reply ended after a chunk")?;
    }
}

// ── /api/info ─────────────────────────────────────────────────────────────

/// `GET /api/info`: the daemon this dash serves, from its own registry
/// entry (the dash only runs as a daemon's child).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Info {
    pub pid: u32,
    pub profile: Option<String>,
}

impl Info {
    /// It serves the daemon whose `Welcome` said `pid` and `profile`.
    pub fn serves(&self, pid: u32, profile: Option<&str>) -> bool {
        let norm = |p: Option<&str>| p.filter(|p| !p.is_empty()).map(str::to_string);
        pid != 0 && self.pid == pid && norm(self.profile.as_deref()) == norm(profile)
    }
}

// ── /api/config ───────────────────────────────────────────────────────────

/// `GET /api/config` (and a `POST`'s reply): the closed set of keys the
/// dash exposes, never a provider key or token.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfigSnapshot {
    pub profile: Option<String>,
    /// The file read, `~`-shortened.
    pub path: String,
    #[serde(rename = "writePath")]
    pub write_path: String,
    pub writable: bool,
    /// Key → raw value; `None`: unset (the default).
    pub values: std::collections::BTreeMap<String, Option<String>>,
    /// Key → `reload` | `live` | `start` | `launch`.
    pub applies: std::collections::BTreeMap<String, String>,
    /// Keys saved but not live until `reload_cmd`.
    pub pending: Vec<String>,
    #[serde(rename = "reloadCmd")]
    pub reload_cmd: String,
    pub plugins: Vec<Plugin>,
    pub providers: Vec<Provider>,
}

impl ConfigSnapshot {
    /// The key is exposed (it has a validator in the dash).
    pub fn exposes(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key)?.as_deref()
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Plugin {
    pub name: String,
    pub scope: String,
    pub description: String,
    pub version: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Provider {
    pub name: String,
    pub kind: String,
    pub source: String,
    pub note: Option<String>,
}

/// The body of a config write: `value: None` unsets the key.
pub fn config_body(key: &str, value: Option<&str>) -> String {
    serde_json::json!({ "key": key, "value": value }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    #[test]
    fn urls_parse_and_never_show_the_token() {
        let t = Target::parse("http://127.0.0.1:7717/?token=00ff\n").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.has_token()), ("127.0.0.1", 7717, true));
        assert_eq!(t.shown(), "http://127.0.0.1:7717");
        assert!(!format!("{t:?}").contains("00ff"));
        assert_eq!(Target::parse("localhost:8080").unwrap().shown(), "http://localhost:8080");
        assert!(!Target::parse("http://127.0.0.1:7717").unwrap().has_token());
        assert!(Target::parse("https://x:1").is_err());
        assert!(Target::parse("http://127.0.0.1:7717/?token=nothex").is_err());
        assert!(Target::parse("http://:7717").is_err());
        assert!(Target::parse("http://a b:1").is_err());
        assert_eq!(strip_token("http://127.0.0.1:7717/?token=abc"), "http://127.0.0.1:7717");
    }

    /// M6: the token never crosses the network and never sits on a command line.
    #[test]
    fn only_loopback_and_no_token_on_the_command_line() {
        for ok in ["http://127.0.0.1:1", "http://127.4.5.6:1", "http://localhost:1", "http://[::1]:1", "LOCALHOST:2"] {
            assert!(Target::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["http://avante.tailnet:7717/?token=ab", "http://10.0.0.2:7717", "http://[::2]:1", "http://0.0.0.0:1", "example.com:80"]
        {
            let e = Target::parse(bad).unwrap_err();
            assert!(e.contains("not loopback") && !e.contains("token=ab"), "{bad}: {e}");
        }
        assert!(flag_has_token("http://127.0.0.1:7717/?token=ab"));
        assert_eq!(flag_target("http://127.0.0.1:7717/?token=ab").unwrap_err(), FLAG_TOKEN);
        assert!(!flag_target("http://127.0.0.1:7717").unwrap().has_token());
        // A URL file: its token is read from the file.
        let f = std::env::temp_dir().join(format!("sd-flag-{}.url", std::process::id()));
        std::fs::write(&f, "http://127.0.0.1:7801/?token=ab\n").unwrap();
        let p = f.display().to_string();
        assert!(!flag_has_token(&p));
        let t = flag_target(&p).unwrap();
        assert_eq!((t.port, t.has_token()), (7801, true));
        std::fs::remove_file(&f).ok();
    }

    #[test]
    fn local_reads_the_url_file_remote_takes_flag_env_then_setting() {
        let dir = std::env::temp_dir().join(format!("sd-dash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("synaps-dash-p.url"), "http://127.0.0.1:7801/?token=ab\n").unwrap();
        let (t, src) = discover(true, &dir, Some("p"), None, Some("http://localhost:1"), None).unwrap().unwrap();
        assert_eq!((t.port, src), (7801, Source::UrlFile(dir.join("synaps-dash-p.url"))));
        // Local with no file: no dash (the environment is a remote's).
        assert!(discover(true, &dir, None, None, Some("http://localhost:1"), None).is_none());
        // `--dash` is Usage's for a local daemon too (and never writes config: views::config_gate).
        assert_eq!(discover(true, &dir, Some("p"), Some("http://localhost:2"), None, None).unwrap().unwrap().1, Source::Flag);
        assert_eq!(
            discover(false, &dir, Some("p"), None, Some("http://localhost:3"), Some("http://localhost:4")).unwrap().unwrap().0.port,
            3
        );
        assert_eq!(discover(false, &dir, None, None, Some("  "), Some("http://localhost:4")).unwrap().unwrap().1, Source::Setting);
        assert!(discover(false, &dir, None, None, None, None).is_none());
        assert!(discover(false, &dir, None, Some("ftp://x"), None, None).unwrap().is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn responses_parse_by_length_chunks_or_close() {
        let r = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}trailing";
        assert_eq!(parse_response(r.to_vec()).unwrap(), (200, "{}".into()));
        assert!(complete(r) && !complete(&r[..r.len() - 9]));
        let c = b"HTTP/1.1 503 Service Unavailable\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3;x=1\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_response(c.to_vec()).unwrap(), (503, "{\"a\":1}".into()));
        assert!(complete(c) && !complete(&c[..c.len() - 5]));
        assert_eq!(parse_response(b"HTTP/1.0 401 x\r\n\r\nno".to_vec()).unwrap(), (401, "no".into()));
        assert!(parse_response(b"garbage".to_vec()).is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort".to_vec()).is_err());
    }

    #[test]
    fn statuses_map_to_what_the_views_say() {
        let t = Target::parse("http://127.0.0.1:1/?token=ab").unwrap();
        assert_eq!(answer(&t, (401, String::new())), Err(Fail::Token));
        let bare = Target::parse("http://127.0.0.1:1").unwrap();
        assert_eq!(answer(&bare, (401, String::new())), Err(Fail::NoToken), "L4: a URL with no token never says the token changed");
        assert_eq!(answer(&t, (503, r#"{"error":"telemetry is starting"}"#.into())), Err(Fail::Starting("telemetry is starting".into())));
        assert_eq!(
            answer(&t, (400, r#"{"ok":false,"error":"invalid value for api_retries"}"#.into())).unwrap_err().to_string(),
            "invalid value for api_retries"
        );
        assert_eq!(Fail::Token.to_string(), "synaps-dash token changed: reconnect");
    }

    /// A fake dash on loopback: the request line, headers and body it got,
    /// and the reply it gave.
    fn fake(status: &str, body: &str) -> (Target, std::thread::JoinHandle<(String, Vec<String>, String)>) {
        let reply = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = std::io::BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut headers = Vec::new();
            let mut len = 0;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                let h = h.trim_end().to_string();
                if h.is_empty() {
                    break;
                }
                if let Some(v) = h.strip_prefix("Content-Length: ") {
                    len = v.parse().unwrap();
                }
                headers.push(h);
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            (&s).write_all(reply.as_bytes()).unwrap();
            (line.trim_end().to_string(), headers, String::from_utf8(body).unwrap())
        });
        (Target::parse(&format!("http://127.0.0.1:{port}/?token=c0ffee")).unwrap(), h)
    }

    #[test]
    fn a_get_sends_the_cookie_and_no_origin() {
        let (t, h) = fake("200 OK", r#"{"pending":[]}"#);
        let snap: ConfigSnapshot = get_json(&t, "/api/config").unwrap();
        let (line, headers, _) = h.join().unwrap();
        assert_eq!(line, "GET /api/config HTTP/1.1");
        assert!(!line.contains("token="), "the token is never in the request line");
        assert_eq!(headers.iter().filter(|h| h.starts_with("Cookie:")).collect::<Vec<_>>(), ["Cookie: synaps_dash=c0ffee"]);
        assert!(!headers.iter().any(|h| h.starts_with("Origin") || h.contains("token=")));
        assert!(snap.pending.is_empty());
    }

    #[test]
    fn a_config_write_posts_json_with_origin_and_reads_the_new_snapshot() {
        let reply =
            r#"{"ok":true,"writable":true,"values":{"api_retries":"5"},"pending":["api_retries"],"reloadCmd":"synaps daemon reload"}"#;
        let (t, h) = fake("200 OK", reply);
        let body = post_json(&t, "/api/config", &config_body("api_retries", Some("5"))).unwrap();
        let snap: ConfigSnapshot = serde_json::from_str(&body).unwrap();
        let (line, headers, sent) = h.join().unwrap();
        assert_eq!(line, "POST /api/config HTTP/1.1");
        let host = headers.iter().find_map(|h| h.strip_prefix("Host: ")).unwrap().to_string();
        assert!(headers.contains(&format!("Origin: http://{host}")));
        assert!(headers.contains(&"Content-Type: application/json".to_string()));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&sent).unwrap(), serde_json::json!({"key": "api_retries", "value": "5"}));
        assert_eq!(
            (snap.value("api_retries"), snap.pending.as_slice(), snap.reload_cmd.as_str()),
            (Some("5"), &["api_retries".to_string()][..], "synaps daemon reload")
        );
        // Unset is `null`.
        assert_eq!(config_body("cache_ttl", None), r#"{"key":"cache_ttl","value":null}"#);
    }

    #[test]
    fn a_refused_write_carries_the_dash_message() {
        let (t, h) = fake("400 Bad Request", r#"{"ok":false,"error":"invalid value for api_retries"}"#);
        let e = post_json(&t, "/api/config", &config_body("api_retries", Some("99"))).unwrap_err();
        h.join().unwrap();
        assert_eq!(e, Fail::Http(400, "invalid value for api_retries".into()));
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let t = Target::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(matches!(get(&t, "/api/config", None), Err(Fail::Unreachable(_))));
    }

    /// M4: a dash that accepts and then stalls costs one deadline, not 20 s
    /// a read.
    #[test]
    fn a_stalled_dash_times_out_by_the_deadline() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            std::thread::sleep(Duration::from_millis(900));
            drop(s);
        });
        let t = Target::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let t0 = Instant::now();
        let r = get(&t, "/api/config", Some(Instant::now() + Duration::from_millis(300)));
        assert_eq!(r, Err(Fail::Unreachable("timed out".into())));
        assert!(t0.elapsed() < Duration::from_millis(800), "{:?}", t0.elapsed());
        h.join().unwrap();
    }
}
