//! `model_list`: the setting's shape, the favourites line, the login
//! filter, and the hard rule: nothing of a config but its
//! `favorite_models` line reaches the desk, its errors or its log.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;

/// Lines no read may let through: provider keys, a token, a malformed line,
/// a key's value split over a line, a comment.
const SECRETS: &[&str] = &["gsk_SECRET_ONE", "sk-SECRET-TWO", "SECRET_THREE", "SECRET_FOUR", "SECRET_FIVE", "SECRET_SIX"];

fn config_text() -> String {
    "model = anthropic/claude-opus-5-5\n\
     provider.groq = gsk_SECRET_ONE\n\
     \tprovider.openai=sk-SECRET-TWO\n\
     auth.token = SECRET_THREE\n\
     SECRET_FOUR without a key\n\
     # favorite_models = commented/out, SECRET_FIVE\n\
     favorite_models_backup = SECRET_SIX\n\
     favorite_models = anthropic/claude-fable-5-2, openai-codex/gpt-5.6-sol ,, xai-auth/grok-5, groq/llama-4\r\n\
     thinking = high\n"
        .to_string()
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("synaps-desktop-model-list-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const FAVS: [&str; 4] = ["anthropic/claude-fable-5-2", "openai-codex/gpt-5.6-sol", "xai-auth/grok-5", "groq/llama-4"];

fn ids(l: &List) -> Vec<&str> {
    l.models.iter().map(|m| m.id.as_str()).collect()
}

/// What `synaps auth list --json` printed on avante (dev, 2026-11-23),
/// its identities and fingerprints left out.
fn auth_list() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": 1, "source": "local",
        "accounts": [
            {"provider": "anthropic", "label": "default", "selected": false, "expires": 0, "identity": "someone@example.com"},
            {"provider": "anthropic", "label": "inference2", "selected": false},
            {"provider": "openai-codex", "label": "default", "selected": true}
        ],
        "selectors": {"anthropic": {"kind": "auto"}, "github-copilot": {"kind": "auto"}, "google-gemini": {"kind": "auto"},
            "kimi-code": {"kind": "auto"}, "muse": {"kind": "auto"}, "openai-codex": {"kind": "account", "account": "default"},
            "xai-auth": {"kind": "auto"}},
        "malformed_keys": [], "duplicate_identity_keys": []
    }))
    .unwrap()
}

fn ask(source: Source, here: Result<(), String>, base: &Path) -> Ask {
    Ask { source, pid: 0, profile: None, here, base: base.to_path_buf(), reader: None }
}

fn only(favorites: Where, logins: Where) -> Source {
    Source { favorites, logins, warning: None }
}

#[test]
fn the_setting_s_shapes() {
    let p = |v: serde_json::Value| Source::parse(Some(&v));
    assert_eq!(Source::parse(None), Source::default(), "unset: the daemon's machine");
    assert_eq!(Source::default().favorites, Where::DaemonHost);
    assert_eq!(p(json!(null)), Source::default());
    assert_eq!(p(json!("local")), only(Where::Local, Where::Local));
    assert_eq!(p(json!("off")), only(Where::Off, Where::Off));
    assert_eq!(
        p(json!({"file": "/etc/x"})),
        only(Where::File("/etc/x".into()), Where::DaemonHost),
        "favourites there, logins the daemon's"
    );
    assert_eq!(p(json!({"command": "ssh box cat f"})), only(Where::Command("ssh box cat f".into()), Where::DaemonHost));
    assert_eq!(
        p(json!({"favorites": "local", "logins": {"command": "synaps-dev auth list --json"}})),
        only(Where::Local, Where::Command("synaps-dev auth list --json".into()))
    );
    assert_eq!(p(json!({"logins": "off"})), only(Where::DaemonHost, Where::Off));
    if let Some(h) = std::env::var_os("HOME").filter(|_| cfg!(unix)) {
        assert_eq!(p(json!({"file": "~/c"})).favorites, Where::File(PathBuf::from(h).join("c")));
    }
    // Not understood: the default, and why (never the value written).
    for bad in [json!("remote"), json!(3), json!({"file": ""}), json!({"file": "a", "command": "b"}), json!({"favourites": "local"})] {
        let s = p(bad.clone());
        assert_eq!((&s.favorites, &s.logins), (&Where::DaemonHost, &Where::DaemonHost), "{bad}");
        let w = s.warning.expect("a warning");
        assert!(w.starts_with("pickers.source: ") && w.ends_with("using daemon-host"), "{w}");
        assert!(!w.contains("remote") && !w.contains("favourites\""), "{w}");
    }
}

#[test]
fn only_the_favorite_models_line_is_taken() {
    let l = |s: &str| favorite_line(s.as_bytes());
    assert_eq!(l("favorite_models = a/b, c/d"), Some(vec!["a/b".into(), "c/d".into()]));
    assert_eq!(l("  favorite_models=a/b ,  ,c/d\r\n"), Some(vec!["a/b".into(), "c/d".into()]));
    assert_eq!(l("\tfavorite_models   =   "), Some(vec![]), "set to none");
    for other in
        ["# favorite_models = a", "favorite_models_x = a", "favorite_models", "xfavorite_models = a", "provider.x = favorite_models", ""]
    {
        assert_eq!(l(other), None, "{other:?}");
    }
    let text = config_text();
    assert_eq!(favorites_in(text.as_bytes()), Some(FAVS.iter().map(|s| s.to_string()).collect::<Vec<String>>()));
    assert_eq!(favorites_in(b"favorite_models = a\nfavorite_models = b\n"), Some(vec!["b".into()]), "the last wins, as Synaps reads it");
    assert_eq!(favorites_in(b"provider.groq = x\n"), None);
}

#[test]
fn logins_filter_oauth_providers_only() {
    let l = Logins::parse(&auth_list()).unwrap();
    assert_eq!(l.accounts.len(), 3);
    assert!(l.has_login("anthropic/claude-fable-5-2"));
    assert!(l.has_login("claude-opus-4-6"), "a bare id is Anthropic's");
    assert!(l.has_login("openai-codex/gpt-5.6-sol"));
    assert!(!l.has_login("xai-auth/grok-5"), "an OAuth provider with no account");
    assert!(!l.has_login("github-copilot/claude-sonnet-4.6"));
    assert!(l.has_login("groq/llama-4"), "an API key's provider is not listed there: kept");
    assert!(l.has_login("anthropic@inference2/claude-opus-5-5"), "that account");
    assert!(!l.has_login("anthropic@work/claude-opus-5-5"), "no such account");
    // An older listing without selectors: the accounts' providers are OAuth.
    let old = Logins::parse(br#"{"accounts": [{"provider": "anthropic", "label": "default"}]}"#).unwrap();
    assert!(old.has_login("anthropic/x") && old.has_login("xai-auth/y"), "unknown: kept");
    // Not a listing: an error that quotes none of it.
    for bad in [&b"provider.groq = gsk_SECRET_ONE"[..], &br#"{"accounts": "gsk_SECRET_ONE"}"#[..], &b""[..]] {
        let e = Logins::parse(bad).unwrap_err();
        assert!(!e.contains("SECRET"), "{e}");
    }
}

#[test]
fn a_local_daemon_s_profile_config_and_logins() {
    let d = dir("profile");
    write(&d.join("config"), "favorite_models = base/one\n");
    write(&d.join("work").join("config"), &config_text());
    let logins = d.join("auth.json");
    std::fs::write(&logins, auth_list()).unwrap();
    let mut a = ask(only(Where::DaemonHost, Where::File(logins.clone())), Ok(()), &d);
    assert_eq!(ids(&load(&a)), ["base/one"], "no profile: the base config");
    a.profile = Some("work".into());
    let l = load(&a);
    assert_eq!(ids(&l), ["anthropic/claude-fable-5-2", "openai-codex/gpt-5.6-sol", "groq/llama-4"], "the profile's, less xai-auth");
    assert_eq!(l.notice.as_deref(), Some("1 favourite hidden: no login for xai-auth"));
    a.profile = Some("nope".into());
    assert_eq!(ids(&load(&a)), ["base/one"], "a profile with no config of its own: the base one, as Synaps");
    a.profile = Some("../x".into());
    assert!(load(&a).notice.unwrap().contains("not a plain name"));
    // Local: this machine's, the daemon anywhere.
    a.profile = Some("work".into());
    a.source = only(Where::Local, Where::Off);
    a.here = Err("no registry entry".into());
    assert_eq!(ids(&load(&a)).len(), 4, "no login filter: all four");
}

#[test]
fn what_could_not_be_read_is_said() {
    let d = dir("notes");
    let l = load(&ask(only(Where::DaemonHost, Where::Off), Ok(()), &d));
    assert!(l.models.is_empty());
    assert!(l.notice.as_deref().unwrap().starts_with("no favorite_models in "), "{:?}", l.notice);
    let l = load(&ask(Source::default(), Err("no registry entry on this machine has its pid 7".into()), &d));
    let n = l.notice.unwrap();
    assert!(n.contains("not on this machine (no registry entry") && n.contains("synaps-desktop-remote"), "{n}");
    write(&d.join("config"), "favorite_models = a/b\n");
    let l = load(&ask(only(Where::Local, Where::File(d.join("missing.json"))), Ok(()), &d));
    assert_eq!(ids(&l), ["a/b"]);
    assert!(l.notice.unwrap().starts_with("logins not checked: "), "the list stands, unfiltered");
    let warned = Source { warning: Some("pickers.source: x; using daemon-host".into()), ..only(Where::Local, Where::Off) };
    assert_eq!(load(&ask(warned, Ok(()), &d)).notice.as_deref(), Some("pickers.source: x; using daemon-host"));
    let dupes = d.join("dupes");
    write(&dupes, "favorite_models = a/b, a/b, c/d\n");
    assert_eq!(ids(&load(&ask(only(Where::File(dupes), Where::Off), Ok(()), &d))), ["a/b", "c/d"]);
}

/// Captures what the desk logs (its own targets) while a test runs.
struct Capture;
static LOGGED: Mutex<String> = Mutex::new(String::new());
static CAPTURE: Capture = Capture;
impl log::Log for Capture {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("synaps_desktop")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata())
            && let Ok(mut s) = LOGGED.lock()
            && s.len() < 1 << 20
        {
            s.push_str(&format!("{}\n", r.args()));
        }
    }
    fn flush(&self) {}
}

/// Install [`Capture`] (once per test process; another logger first: none
/// is captured, and the test says so by failing).
pub(crate) fn capture_logs() -> &'static Mutex<String> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        log::set_logger(&CAPTURE).expect("no other logger in the tests");
        log::set_max_level(log::LevelFilter::Trace);
    });
    &LOGGED
}

/// No secret of `text`'s config in it.
#[track_caller]
pub(crate) fn assert_no_secret(what: &str, text: &str) {
    for s in SECRETS {
        assert!(!text.contains(s), "{what} leaked {s}: {text}");
    }
    assert!(!text.contains("SECRET"), "{what}: {text}");
}

/// The hard rule: a config with `provider.*` keys, a token and loose
/// secret lines, read every way the setting reads one (the daemon's
/// machine here, a profile, a file, a command that prints all of it, one
/// that fails after printing it, one that prints it to stderr, a logins
/// "listing" that is the config): the list, its notes and the log hold
/// none of them, and the favourites are what is listed.
#[test]
fn a_config_s_secrets_never_leave_the_favourites_line() {
    let logged = capture_logs();
    let d = dir("secrets");
    write(&d.join("config"), &config_text());
    write(&d.join("work").join("config"), &config_text());
    let cfg = d.join("config");
    let mut asks = vec![
        ask(only(Where::DaemonHost, Where::Off), Ok(()), &d),
        Ask { profile: Some("work".into()), ..ask(only(Where::Local, Where::Off), Err("remote".into()), &d) },
        ask(only(Where::File(cfg.clone()), Where::File(cfg.clone())), Ok(()), &d),
    ];
    if cfg!(unix) {
        let q = |p: &Path| format!("'{}'", p.display());
        asks.push(ask(only(Where::Command(format!("cat {}", q(&cfg))), Where::Command(format!("cat {}", q(&cfg)))), Ok(()), &d));
        asks.push(ask(only(Where::Command(format!("cat {}; exit 3", q(&cfg))), Where::Off), Ok(()), &d));
        asks.push(ask(only(Where::Command(format!("cat {} >&2; exit 4", q(&cfg))), Where::Off), Ok(()), &d));
    }
    for (i, a) in asks.iter().enumerate() {
        let l = load(a);
        assert_no_secret(&format!("read {i}"), &format!("{l:?}"));
        match i {
            0..=3 => assert_eq!(ids(&l)[..3], FAVS[..3], "read {i}: {:?}", l.notice),
            _ => assert!(l.models.is_empty() && l.notice.as_deref().is_some_and(|n| n.contains("failed (exit")), "read {i}: {l:?}"),
        }
        if i == 2 || i == 3 {
            assert!(l.notice.as_deref().is_some_and(|n| n.starts_with("logins not checked")), "{l:?}");
        }
    }
    // Every error path: the file unreadable, a directory, too big.
    for (p, what) in [(d.join("work"), "a directory"), (d.join("nope").join("config"), "missing")] {
        let r = read_favorites(&p);
        assert_no_secret(what, &format!("{r:?}"));
    }
    let big = d.join("big");
    write(&big, &format!("{}{}", config_text(), "provider.x = SECRET_ONE_MORE\n".repeat(50_000)));
    assert_no_secret("a big file", &format!("{:?}", read_favorites(&big)));
    assert_no_secret("the log", &logged.lock().unwrap());
}

/// The launcher (`scripts/synaps-desktop-remote`): its text, and its
/// remote half, the script it sends `<host>`.
#[cfg(unix)]
fn launcher() -> (PathBuf, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/synaps-desktop-remote");
    let text = std::fs::read_to_string(&script).unwrap();
    let start = text.find("<<'REMOTE'\n").expect("the remote half") + "<<'REMOTE'\n".len();
    let remote = text[start..start + text[start..].find("\nREMOTE\n").expect("its end")].to_string();
    (script, remote)
}

/// What a run printed: its exit code, stdout and stderr, apart.
#[cfg(unix)]
fn output(c: &mut std::process::Command) -> (Option<i32>, String, String) {
    let out = c.output().expect("bash");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// A stand-in `ssh` on `PATH` for the launcher's local half: it records
/// that it ran (`<dir>/ssh-ran`), drops ssh's options and the host and
/// runs the rest here, the remote half on its stdin, as sshd would.
#[cfg(unix)]
fn fake_ssh(dir: &Path) -> std::ffi::OsString {
    let bin = dir.join("bin");
    write(
        &bin.join("ssh"),
        &format!(
            "#!/bin/sh\ntouch '{}'\nwhile [ $# -gt 0 ]; do case \"$1\" in -T) shift;; -o) shift 2;; *) shift; break;; esac; done\nexec \"$@\"\n",
            dir.join("ssh-ran").display()
        ),
    );
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = bin.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    path
}

/// `config-key` takes `favorite_models` and nothing else, on both sides:
/// innocent-looking keys as much as secret ones. The host sends only that
/// key's lines and nothing on stderr; a refused key never reaches ssh.
#[cfg(unix)]
#[test]
fn the_launcher_reads_favorite_models_only() {
    use std::process::Command;
    let (script, remote) = launcher();
    let d = dir("launcher");
    write(&d.join(".synaps-cli").join("config"), &format!("{}identity = SECRET_IDENTITY\ncustom_key = SECRET_CUSTOM\n", config_text()));
    write(&d.join(".synaps-cli").join("work").join("config"), "favorite_models = w/1\nprovider.x = SECRET_ONE_MORE\n");
    let host = |args: &[&str]| {
        output(Command::new("bash").arg("-c").arg(&remote).arg("remote").args(args).env("HOME", &d).env_remove("SYNAPS_BASE_DIR"))
    };
    let (code, out, err) = host(&["config-key", "-", "favorite_models"]);
    assert_eq!((code, favorites_in(out.as_bytes()).map(|v| v.len()), err.as_str()), (Some(0), Some(4), ""), "{out}");
    assert_eq!(out.lines().count(), 1, "that line only: {out}");
    assert_no_secret("the base config's read", &out);
    let (code, out, err) = host(&["config-key", "work", "favorite_models"]);
    assert_eq!((code, out.trim(), err.as_str()), (Some(0), "favorite_models = w/1", ""));
    let others = [
        "identity",
        "custom_key",
        "model",
        "theme",
        "favorite_models_backup",
        "Favorite_models",
        "favorite_models.x",
        "provider.groq",
        "auth.token",
    ];
    for key in others {
        let (code, out, err) = host(&["config-key", "-", key]);
        assert_eq!((code, out.as_str(), err.as_str()), (Some(65), "", ""), "{key}");
    }
    for (args, want) in [
        (&["config-key", "-", "x;cat ~/.synaps-cli/config"][..], 64),
        (&["config-key", "-"][..], 64),
        (&["cli", "1", "-", "auth-list"][..], 66),
        (&["cli", "1", "-", "status"][..], 64),
        (&["dump"][..], 64),
    ] {
        let (code, out, err) = host(args);
        assert_eq!((code, out.as_str(), err.as_str()), (Some(want), "", ""), "{args:?}");
    }
    // The launcher's own checks, before any ssh.
    let path = fake_ssh(&d);
    let local = |args: &[&str]| output(Command::new("bash").arg(&script).args(args).env("PATH", &path).env("HOME", &d));
    for key in others {
        let (code, out, _) = local(&["--read", "h", "config-key", "-", key]);
        assert_eq!((code, out.as_str()), (Some(65), ""), "{key}");
    }
    for args in [
        &["--read", "h", "config-key", "-", "a b"][..],
        &["--read", "h;x", "config-key", "-", "favorite_models"][..],
        &["--read", "h", "cli", "12", "-", "rm-rf"][..],
    ] {
        assert_eq!(local(args).0, Some(64), "{args:?}");
    }
    assert!(!d.join("ssh-ran").exists(), "nothing refused reached ssh");
    // And through it: the favourites line only.
    let (code, out, err) = local(&["--read", "h", "config-key", "-", "favorite_models"]);
    assert!(d.join("ssh-ran").exists());
    assert_eq!((code, out.lines().count(), err.as_str()), (Some(0), 1, ""), "{out}");
    assert_no_secret("through the launcher", &out);
}

/// The worst `auth list` there could be, on `<host>`: it prints every line
/// of a config full of secrets on stderr, and puts them in its listing
/// (identities, selector values, an invalid selector's reason, malformed
/// keys, fields of its own, a label and a provider that are not plain).
/// What the host half sends, stdout and stderr both, captured before the
/// desk could drop anything: the providers and labels, none of the rest.
/// Run as the host runs it and through the launcher (an `ssh` that runs
/// it here); then as the CLI failing, or printing the config: nothing.
#[cfg(target_os = "linux")]
#[test]
fn the_host_cuts_auth_list_down_before_it_leaves() {
    use std::process::Command;
    let (script, remote) = launcher();
    let d = dir("auth-list");
    let config = format!(
        "{}events.auto_turn = SECRET_CONFIG_VALUE\nauth.account.anthropic = SECRET_SELECTOR\nidentity = SECRET_IDENTITY\n",
        config_text()
    );
    write(&d.join(".synaps-cli").join("config"), &config);
    let listing = |extra: &str| {
        format!(
            r#"cat "$HOME/.synaps-cli/config" >&2
echo "warning: events.auto_turn = SECRET_CONFIG_VALUE" >&2
cat <<'JSON'
{{
  "schema_version": 1, "source": "local",
  "accounts": [
    {{"provider": "anthropic", "label": "default", "identity": "SECRET_IDENTITY@example.com", "seat_fingerprint": "SECRET_FP", "expires": 1795000000000, "selected": false}},
    {{"provider": "anthropic", "label": "inference2", "selected": true, "x": {{"nested": ["SECRET_NESTED", {{"a": null}}]}}}},
    {{"provider": "openai-codex", "label": "SECRET\"_QUOTED"}},
    {{"provider": "SECRET_\u0041", "label": "x"}},
    {{"provider": "SECRET PROVIDER", "label": "x"}}
  ],
  "selectors": {{
    "anthropic": {{"kind": "account", "account": "SECRET_SELECTOR"}},
    "xai-auth": {{"kind": "invalid", "source": "config", "reason": "SECRET_CONFIG_VALUE \"quoted\" \\ back"}},
    "openai-codex": {{"kind": "auto"}}
  }},
  "malformed_keys": ["provider.groq = gsk_SECRET_ONE"], "duplicate_identity_keys": []
}}
JSON
{extra}
"#
        )
    };
    // The daemon: a process of this user's with `daemon` in its command
    // line. Its executable is bash, so its "CLI" (`/proc/<pid>/exe auth
    // list --json`) is bash running `./auth`, the stand-in above.
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let daemon = Daemon(Command::new("bash").args(["-c", "sleep 60; :", "daemon"]).spawn().unwrap());
    let pid = daemon.0.id().to_string();
    let t = Instant::now();
    while !std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|&b| b == 0).any(|a| a == b"daemon")) {
        assert!(t.elapsed() < Duration::from_secs(5), "the stand-in daemon never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let path = fake_ssh(&d);
    let host = |cli: &str, via_launcher: bool| {
        write(&d.join("auth"), cli);
        let mut c = Command::new("bash");
        if via_launcher {
            c.arg(&script).args(["--read", "h", "cli", &pid, "-", "auth-list"]).env("PATH", &path);
        } else {
            c.arg("-c").arg(&remote).args(["remote", "cli", &pid, "-", "auth-list"]);
        }
        output(c.current_dir(&d).env("HOME", &d).env_remove("SYNAPS_BASE_DIR"))
    };
    for via_launcher in [false, true] {
        let (code, out, err) = host(&listing(""), via_launcher);
        assert_eq!((code, err.as_str()), (Some(0), ""), "{out}");
        assert_no_secret("what the host sent", &out);
        assert_eq!(
            out.trim(),
            r#"{"accounts":[{"provider":"anthropic","label":"default"},{"provider":"anthropic","label":"inference2"}],"selectors":{"anthropic":{},"xai-auth":{},"openai-codex":{}}}"#
        );
        let l = Logins::parse(out.as_bytes()).unwrap();
        assert!(l.has_login("anthropic@inference2/m") && !l.has_login("xai-auth/grok-5") && l.has_login("groq/x"), "{l:?}");
        for (cli, why) in
            [(listing("exit 3"), "the CLI failed"), (format!("cat \"$HOME/.synaps-cli/config\"; {}", listing("")), "not a listing")]
        {
            let (code, out, err) = host(&cli, via_launcher);
            assert_eq!((code, out.as_str(), err.as_str()), (Some(67), "", ""), "{why}");
        }
    }
}

/// Every source of favourites streams through [`Favorites`]: whatever the
/// block sizes, it never holds more than a `favorite_models` value, so no
/// other line of a config is held whole at any point.
#[test]
fn a_stream_holds_no_other_line() {
    let text = format!("{}provider.x = {}\n", config_text(), "SECRET_LONG ".repeat(10_000));
    for size in [1, 2, 3, 7, 64, 4096, text.len()] {
        let mut f = Favorites::default();
        for chunk in text.as_bytes().chunks(size) {
            f.push(chunk).unwrap();
            let held = String::from_utf8_lossy(f.held());
            assert!(!held.contains("SECRET") && !held.contains("provider"), "{size}: {held}");
            assert!(f.held().len() < 100, "the favourites' value at most");
        }
        assert_eq!(f.finish(), Some(FAVS.iter().map(|s| s.to_string()).collect::<Vec<String>>()), "{size}");
    }
    // More than a config is not one.
    let mut f = Favorites::default();
    assert!(f.push("x".repeat(FILE_MAX + 1).as_bytes()).is_err());
    // A value past its bound is not a list, and is not held.
    let mut f = Favorites::default();
    f.push(format!("favorite_models = {}\n", "a,".repeat(VALUE_MAX)).as_bytes()).unwrap();
    assert!(f.held().is_empty());
    assert_eq!(f.finish(), None);
}

/// A command's whole tree ends with its read: at the deadline (its shell
/// waiting on a child that holds stdout; a shell gone, its child holding
/// it), once it printed too much, and after it answered (a child it left
/// running). The read returns by its deadline, not the child's.
#[cfg(unix)]
#[test]
fn a_command_s_whole_tree_ends_with_its_read() {
    let d = dir("tree");
    // Gone, or a zombie its new parent has yet to reap.
    let gone = |pid: i32| {
        let t = Instant::now();
        let zombie = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z')))
        };
        // SAFETY: signal 0 only checks the pid.
        while unsafe { libc::kill(pid, 0) } == 0 && !zombie() {
            assert!(t.elapsed() < Duration::from_secs(5), "{pid} still runs");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let child = |f: &Path| std::fs::read_to_string(f).unwrap().trim().parse::<i32>().unwrap();
    let within = Duration::from_millis(800);
    for (i, (cmd, want)) in [
        ("sleep 60 & echo $! > {f}; wait", "did not answer within 0.8 s"),
        ("sleep 60 & echo $! > {f}", "did not answer within 0.8 s"),
        ("sleep 60 & echo $! > {f}; yes 'provider.x = SECRET'", "printed more than 1024 KiB"),
        ("sleep 60 > /dev/null & echo $! > {f}; echo favorite_models = a/b", ""),
    ]
    .into_iter()
    .enumerate()
    {
        let f = d.join(format!("pid{i}"));
        let t = Instant::now();
        let r = run(shell(&cmd.replace("{f}", &f.display().to_string())), "the command", Favorites::default(), within);
        assert!(t.elapsed() < within + Duration::from_secs(3), "{cmd}: {:?}", t.elapsed());
        match r {
            Ok(f) => assert_eq!((want, f.finish()), ("", Some(vec!["a/b".to_string()])), "{cmd}"),
            Err(e) => {
                assert_eq!(e, format!("the command {want}"), "{cmd}");
                assert_no_secret(cmd, &e);
            }
        }
        gone(child(&f));
    }
}

/// On Windows: a late command's children go with it (a job object), so
/// the read ends by its deadline instead of waiting on what holds stdout.
#[cfg(windows)]
#[test]
fn a_late_command_and_its_children_are_killed() {
    let within = Duration::from_millis(800);
    let t = Instant::now();
    let Err(e) = run(shell("ping -n 30 127.0.0.1"), "the command", Capped::default(), within) else { panic!("it answered") };
    assert_eq!(e, "the command did not answer within 0.8 s", "not left waiting on ping");
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    let Ok(r) = run(shell("echo favorite_models = a/b"), "the command", Favorites::default(), Duration::from_secs(5)) else {
        panic!("it did not answer")
    };
    assert_eq!(r.finish(), Some(vec!["a/b".to_string()]));
}

/// `Debug` of a source says its kind, never the path or the command (a
/// token can be written inline).
#[test]
fn debug_names_no_command_or_path() {
    let s = Source::parse(Some(&json!({"favorites": {"command": "curl -H 'Bearer SECRET_TOKEN' x"}, "logins": {"file": "/SECRET_DIR/a"}})));
    assert_eq!(format!("{s:?}"), "Source { favorites: Command(..), logins: File(..), warning: None }");
    let a = ask(s, Ok(()), Path::new("/tmp"));
    assert!(!format!("{a:?}").contains("SECRET"), "{a:?}");
}

/// A FIFO (or a device) is not read: a read of one could block past any
/// deadline.
#[cfg(unix)]
#[test]
fn a_fifo_is_not_read() {
    let d = dir("fifo");
    let p = d.join("config");
    let c = std::ffi::CString::new(p.display().to_string()).unwrap();
    // SAFETY: a path we made.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let t = Instant::now();
    assert_eq!(read_favorites(&p).unwrap_err(), format!("{}: not a regular file", crate::named::home_short(&p.display().to_string())));
    assert!(read_capped(&p).is_err());
    assert!(t.elapsed() < Duration::from_secs(1));
}
