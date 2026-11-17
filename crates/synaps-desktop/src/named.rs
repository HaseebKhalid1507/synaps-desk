//! Names and resumes: what a rail row's click does, model names and times
//! for people, and the human messages for rename refusals and failed
//! resumes. No GPUI.

use synaps_client::{CreateOptions, Cwd, RenameFailure, RenameRefusal};

/// What clicking a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowAction {
    /// A live or parked session: attach.
    Attach(String),
    /// A saved session the daemon does not hold: resume it by journal id.
    Resume(ResumeRequest),
}

/// A resume to send: a saved row, Open by name, or a live row whose
/// session ended since the last list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeRequest {
    /// What the user knows it by (the name).
    pub label: String,
    /// `continue_session`: the exact journal id when a list had it, else
    /// the name (the daemon's resolver).
    pub target: String,
    /// The chat's folder on the DAEMON's machine, sent as is (never checked
    /// here: with a forwarded socket it is another machine). `None`: `~`.
    pub cwd: Option<String>,
    /// Resolved by the daemon from the name: its resolver also takes any id
    /// containing the text, so the attached session must carry `label`.
    pub by_name: bool,
}

impl ResumeRequest {
    /// `Client::resume`'s options: the saved folder as is, and `~` if the
    /// daemon says it is gone there (`Event::CwdFallback`); `~` when no
    /// folder is known.
    pub fn options(&self) -> CreateOptions {
        match &self.cwd {
            Some(c) => CreateOptions::new().cwd(Cwd::Path(c.into())).fallback_cwd(Cwd::Home),
            None => CreateOptions::new().cwd(Cwd::Home),
        }
    }
}

/// `anthropic/claude-sonnet-4-5-20250929` → `sonnet-4-5`.
pub fn short_model(model: &str) -> String {
    let m = model.rsplit('/').next().unwrap_or(model);
    let m = m.strip_prefix("claude-").unwrap_or(m);
    let m = match m.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => m,
    };
    m.to_string()
}

/// Seconds since the Unix epoch of an RFC 3339 time
/// (`2026-11-13T07:32:23.264213931Z`, `…+02:00`).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        rest = &frac[n..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let v = o[1..3].parse::<i64>().ok()? * 3600 + o[4..6].parse::<i64>().ok()? * 60;
            if o.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    // Howard Hinnant's days_from_civil.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

/// `now`, `5m`, `3h`, `2d`, `5w`, `4mo`, `2y`.
pub fn relative(ts: &str, now: i64) -> String {
    let Some(t) = parse_rfc3339(ts) else { return String::new() };
    let d = (now - t).max(0);
    match d {
        0..60 => "now".into(),
        60..3600 => format!("{}m", d / 60),
        3600..86_400 => format!("{}h", d / 3600),
        86_400..1_209_600 => format!("{}d", d / 86_400),
        1_209_600..5_184_000 => format!("{}w", d / 604_800),
        5_184_000..31_536_000 => format!("{}mo", d / 2_592_000),
        _ => format!("{}y", d / 31_536_000),
    }
}

/// A `RenameFailed`, for the user. `name` is the name that was asked for.
pub fn rename_message(f: &RenameFailure, name: &str) -> String {
    match f.reason {
        RenameRefusal::Invalid => format!("'{name}' is not a valid name: use lowercase letters, digits and '-' (at most 40)."),
        RenameRefusal::Taken => match &f.taken_by {
            Some(id) => format!("The name '{name}' is already used by another saved session ({id})."),
            None => format!("The name '{name}' is already used by another saved session."),
        },
        RenameRefusal::ChainConflict => format!("'{name}' is already the name of a session chain."),
        RenameRefusal::Named => match &f.current_name {
            Some(c) => format!("This session is already named '{c}'."),
            None => "This session's name changed meanwhile.".into(),
        },
        RenameRefusal::Locked => "This session is open in another process; rename it there.".into(),
        RenameRefusal::Busy => "The daemon is busy; try again in a moment.".into(),
        RenameRefusal::Unknown => "No answer from the session: the name may still apply. Check the list before trying again.".into(),
        RenameRefusal::NotLive => "This session is no longer live in the daemon.".into(),
        RenameRefusal::Unsupported => "This daemon is too old to rename sessions; update Synaps.".into(),
        RenameRefusal::Offline => "Not connected to the daemon; nothing was renamed.".into(),
        _ if f.message.is_empty() => "Could not rename the session.".into(),
        _ => format!("Could not rename the session: {}", f.message),
    }
}

/// A resume that ended before attaching (`Detached { reason }`), for the user.
/// The daemon's texts (Synaps `origin/dev`): `daemon/conn.rs` sends
/// `format!("create session: {e}")`; `engine/setup.rs` wraps a failed
/// lookup as `RuntimeError::Tool("Failed to load session '{}': {}")`
/// (`"Tool execution failed: {0}"`) around `core/session.rs`'s `"no session
/// matching '{}'"` / `"ambiguous: {} sessions match '{}'"`; a held journal
/// is `RuntimeError::Session` (`"Session error: {0}"`) around
/// `session_lock.rs`'s `"session {} is live in another process (pid {},
/// {}) — use …"`.
pub fn resume_error(label: &str, reason: &str) -> String {
    let mut why = reason;
    for prefix in ["create session: ", "create owned session: ", "Tool execution failed: ", "Session error: "] {
        why = why.strip_prefix(prefix).unwrap_or(why);
    }
    if why.contains("no session matching") || why.contains("No sessions to continue") {
        format!("No session named '{label}'.")
    } else if why.contains("ambiguous:") {
        format!("'{label}' matches more than one saved session; open it from the list.")
    } else if why.contains("is live in another process") {
        format!("'{label}' is locked by another process (a TUI or another daemon has it open). Close it there first.")
    } else {
        let why = why.strip_prefix(&format!("Failed to load session '{label}': ")).unwrap_or(why);
        format!("Could not resume '{label}': {why}")
    }
}

/// This machine's home folder, read once: `$HOME` (on Windows
/// `%USERPROFILE%` first: the daemon's paths are native there), without its
/// trailing separator. `None` when unset or a root (`/`, `C:\\`).
pub(crate) fn home() -> Option<&'static str> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let vars = if cfg!(windows) { ["USERPROFILE", "HOME"] } else { ["HOME", "USERPROFILE"] };
        let h = vars.iter().filter_map(|v| std::env::var(v).ok()).find(|h| !h.is_empty())?;
        let h = h.trim_end_matches(['/', '\\']);
        (h.len() > 1 && !h.ends_with(':')).then(|| h.to_string())
    })
    .as_deref()
}

/// How many bytes `home` takes at the start of `s`, if it is there whole
/// (the end, a separator, a space or a quote follows). A drive path's
/// (`C:\\Users\\me`) case is ignored and its separators go either way,
/// JSON's escaped `\\\\` too, as a model writes them.
pub(crate) fn home_len(s: &str, home: &str) -> Option<usize> {
    let (s, h) = (s.as_bytes(), home.as_bytes());
    let drive = h.len() > 2 && h[0].is_ascii_alphabetic() && h[1] == b':';
    let mut i = 0;
    for &c in h {
        let d = *s.get(i)?;
        if drive && matches!(c, b'/' | b'\\') {
            i += match (d, s.get(i + 1)) {
                (b'/', _) => 1,
                (b'\\', Some(b'\\')) => 2,
                (b'\\', _) => 1,
                _ => return None,
            };
        } else if d == c || (drive && d.eq_ignore_ascii_case(&c)) {
            i += 1;
        } else {
            return None;
        }
    }
    s.get(i).is_none_or(|c| matches!(c, b'/' | b'\\' | b' ' | b'"' | b'\'' | b'\n')).then_some(i)
}

/// `/home/me/x` → `~/x` when it is this machine's home.
pub(crate) fn home_short(path: &str) -> String {
    match home().and_then(|h| home_len(path, h)) {
        Some(n) => format!("~{}", &path[n..]),
        None => path.to_string(),
    }
}

/// The daemon's real create errors (see `resume_error`), word for word.
#[cfg(test)]
pub(crate) const NO_SUCH_GHOST: &str = "create session: Tool execution failed: Failed to load session 'ghost': no session matching 'ghost'";
#[cfg(test)]
pub(crate) const LOCKED_AXEL: &str = "create session: Session error: session 20261110-202033-e5f6 is live in another process \
    (pid 4242, tui) — use `synaps --attach 20261110-202033-e5f6`, or `synaps daemon sessions`";

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The home at a path's start, whole: on Windows whatever the case and
    /// the separators (escaped in JSON too); elsewhere exactly.
    #[test]
    fn the_home_is_found_however_a_windows_path_spells_it() {
        let win = "C:\\Users\\me";
        for s in ["C:\\Users\\me\\x", "c:\\users\\ME\\x", "C:/Users/me/x", "C:\\\\Users\\\\me\\\\x", "C:\\Users\\me"] {
            let n = home_len(s, win).unwrap_or_else(|| panic!("{s}"));
            assert!(!s[n..].contains("me") && !s[n..].contains("ME"), "{s}: {}", &s[n..]);
        }
        assert_eq!(home_len("C:\\Users\\meg\\x", win), None, "another user");
        assert_eq!(home_len("D:\\Users\\me\\x", win), None, "another drive");
        assert_eq!(home_len("/home/me/x", "/home/me"), Some(8));
        assert_eq!(home_len("/HOME/me/x", "/home/me"), None, "case counts off Windows");
        assert_eq!(home_len("\\home\\me", "/home/me"), None);
        assert_eq!(home_len("/home/me\"", "/home/me"), Some(8));
    }

    #[test]
    fn models_are_shortened() {
        assert_eq!(short_model("anthropic/claude-haiku-4-5"), "haiku-4-5");
        assert_eq!(short_model("claude-sonnet-4-5-20250929"), "sonnet-4-5");
        assert_eq!(short_model("claude-opus-4-6"), "opus-4-6");
        assert_eq!(short_model("gpt-5"), "gpt-5");
        assert_eq!(short_model(""), "");
    }

    #[test]
    fn times_parse_and_read_relative() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-11-13T07:32:23.264213931Z"), Some(1_794_555_143));
        assert_eq!(parse_rfc3339("2026-11-13T09:32:23+02:00"), Some(1_794_555_143));
        assert_eq!(parse_rfc3339("2000-02-29T12:00:00Z"), Some(951_825_600));
        for bad in ["", "yesterday", "2026-13-01T00:00:00Z", "2026-11-13T07:32:23", "2026-11-13T07:32:23+0200"] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
        let t = "2026-11-13T07:32:23Z";
        let now = parse_rfc3339(t).unwrap();
        assert_eq!(relative(t, now + 5), "now");
        assert_eq!(relative(t, now + 300), "5m");
        assert_eq!(relative(t, now + 3 * 3600), "3h");
        assert_eq!(relative(t, now + 2 * 86_400), "2d");
        assert_eq!(relative(t, now + 21 * 86_400), "3w");
        assert_eq!(relative(t, now + 120 * 86_400), "4mo");
        assert_eq!(relative(t, now + 800 * 86_400), "2y");
        assert_eq!(relative(t, now - 60), "now", "a clock skew is not negative");
        assert_eq!(relative("garbage", now), "");
    }

    /// Every `RenameRefusal` reads as a sentence, never as a code.
    #[test]
    fn every_rename_refusal_has_a_human_message() {
        use RenameRefusal::*;
        let frame = |reason: &str, extra: serde_json::Value| {
            let mut v = json!({"type": "rename_failed", "session_id": "s", "reason": reason, "message": "raw text"});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            match synaps_client::protocol::decode_frame(&v.to_string()).unwrap() {
                synaps_client::protocol::DaemonFrame::RenameFailed(f) => f,
                other => panic!("{other:?}"),
            }
        };
        let cases = [
            ("invalid", json!({}), Invalid, "not a valid name"),
            ("taken", json!({"taken_by": "20261028-045932-a1b2"}), Taken, "already used by another saved session (20261028-045932-a1b2)"),
            ("chain_conflict", json!({}), ChainConflict, "session chain"),
            ("named", json!({"current_name": "kite"}), Named, "already named 'kite'"),
            ("locked", json!({}), Locked, "another process"),
            ("busy", json!({}), Busy, "busy"),
            ("unknown", json!({}), Unknown, "may still apply"),
            ("from_the_future", json!({}), Failed, "raw text"),
        ];
        for (code, extra, reason, needle) in cases {
            let f = frame(code, extra);
            assert_eq!(f.reason, reason);
            let msg = rename_message(&f, "axon");
            assert!(msg.contains(needle), "{code}: {msg}");
        }
        for (reason, needle) in [(NotLive, "no longer live"), (Unsupported, "too old"), (Offline, "Not connected")] {
            let msg = rename_message(&RenameFailure::new("s", reason, ""), "axon");
            assert!(msg.contains(needle), "{reason:?}: {msg}");
        }
    }

    #[test]
    fn resume_failures_read_clearly() {
        assert_eq!(resume_error("ghost", NO_SUCH_GHOST), "No session named 'ghost'.");
        let locked = resume_error("axel", LOCKED_AXEL);
        assert!(locked.contains("locked by another process"), "{locked}");
        let amb = "create session: Tool execution failed: Failed to load session '2026': ambiguous: 3 sessions match '2026'";
        assert!(resume_error("2026", amb).contains("more than one"));
        let other = "create session: Tool execution failed: Failed to load session 'x': Permission denied (os error 13)";
        assert_eq!(resume_error("x", other), "Could not resume 'x': Permission denied (os error 13)");
        assert_eq!(resume_error("x", "create session: disk full"), "Could not resume 'x': disk full");
    }
}
