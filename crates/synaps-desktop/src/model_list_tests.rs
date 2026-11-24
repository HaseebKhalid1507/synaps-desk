//! `model_list`: the setting's shape, the favourites line, the login
//! filter, and the hard rule: nothing of a config but its
//! `favorite_models` line reaches the desk, its errors or its log.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// The launcher's reads, run here as its host would run them (the script's
/// remote half, `scripts/synaps-desktop-remote`): a config key's lines
/// only, picked out before anything is printed; a secret key refused on
/// both sides; anything but plain words refused; a pid that is not this
/// user's daemon refused.
#[cfg(unix)]
#[test]
fn the_launcher_reads_one_key_s_lines_only() {
    use std::process::Command;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/synaps-desktop-remote");
    let text = std::fs::read_to_string(&script).unwrap();
    let start = text.find("<<'REMOTE'\n").expect("the remote half") + "<<'REMOTE'\n".len();
    let remote = &text[start..start + text[start..].find("\nREMOTE\n").expect("its end")];
    let d = dir("launcher");
    write(&d.join(".synaps-cli").join("config"), &config_text());
    write(&d.join(".synaps-cli").join("work").join("config"), "favorite_models = w/1\nprovider.x = SECRET_ONE_MORE\n");
    let run = |args: &[&str]| {
        let mut c = Command::new("bash");
        c.arg("-c").arg(remote).arg("remote").args(args).env("HOME", &d).env_remove("SYNAPS_BASE_DIR");
        let out = c.output().expect("bash");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    };
    let (code, out) = run(&["config-key", "-", "favorite_models"]);
    assert_eq!((code, favorites_in(out.as_bytes()).map(|v| v.len())), (Some(0), Some(4)), "{out}");
    assert_eq!(out.lines().count(), 1, "that line only: {out}");
    assert_no_secret("the base config's read", &out);
    let (code, out) = run(&["config-key", "work", "favorite_models"]);
    assert_eq!((code, out.trim()), (Some(0), "favorite_models = w/1"));
    for (args, want) in [
        (&["config-key", "-", "provider.groq"][..], 65),
        (&["config-key", "-", "auth.token"][..], 65),
        (&["config-key", "-", "x;cat ~/.synaps-cli/config"][..], 64),
        (&["config-key", "-"][..], 64),
        (&["cli", "1", "-", "auth-list"][..], 66),
        (&["cli", "1", "-", "status"][..], 64),
        (&["dump"][..], 64),
    ] {
        let (code, out) = run(args);
        assert_eq!(code, Some(want), "{args:?}: {out}");
        assert_no_secret(&format!("{args:?}"), &out);
    }
    // The launcher's own checks, before any ssh.
    for (args, want) in [
        (&["--read", "h", "config-key", "-", "provider.groq"][..], 65),
        (&["--read", "h", "config-key", "-", "a b"][..], 64),
        (&["--read", "h;x", "config-key", "-", "k"][..], 64),
        (&["--read", "h", "cli", "12", "-", "rm-rf"][..], 64),
    ] {
        let out = Command::new("bash").arg(&script).args(args).output().expect("bash");
        assert_eq!(out.status.code(), Some(want), "{args:?}");
    }
}
