//! The rail's **Named** section: grouping, sort and dedupe of live and
//! saved sessions, row markers and labels, and the human messages for
//! rename refusals and failed resumes. No GPUI.

use synaps_client::protocol::SessionMeta;
use synaps_client::{CreateOptions, Cwd, RenameFailure, RenameRefusal, SavedSession};

/// The one state marker a named row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Busy,
    Idle,
    Parked,
    Pinned,
    Awaiting,
    Saved,
    Resuming,
}

impl Marker {
    /// A live session's marker: awaiting input beats busy, busy beats
    /// parked, parked beats pinned.
    pub fn of(s: &SessionMeta) -> Marker {
        if s.awaiting_input > 0 {
            Marker::Awaiting
        } else if s.busy {
            Marker::Busy
        } else if s.is_parked() {
            Marker::Parked
        } else if s.pinned {
            Marker::Pinned
        } else {
            Marker::Idle
        }
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedRow {
    pub name: String,
    pub marker: Marker,
    /// Short model label (`opus-4-6`).
    pub model: String,
    /// RFC 3339 time of the last activity, if known.
    pub activity: Option<String>,
    pub action: RowAction,
}

impl NamedRow {
    /// The session or journal id the row stands for.
    pub fn id(&self) -> &str {
        match &self.action {
            RowAction::Attach(id) => id,
            RowAction::Resume(r) => &r.target,
        }
    }
}

/// The Named section: live and parked sessions with a name, then saved
/// named sessions the daemon does not hold (deduped by id, journal id and
/// name), alphabetical.
pub fn named_rows(sessions: &[SessionMeta], saved: &[SavedSession]) -> Vec<NamedRow> {
    let mut rows: Vec<NamedRow> = sessions
        .iter()
        .filter_map(|s| {
            let name = s.name.as_deref().filter(|n| !n.is_empty())?;
            Some(NamedRow {
                name: name.to_string(),
                marker: Marker::of(s),
                model: short_model(&s.model),
                activity: s.state_since.clone().or_else(|| Some(s.created_at.clone()).filter(|c| !c.is_empty())),
                action: RowAction::Attach(s.id.clone()),
            })
        })
        .collect();
    for v in saved {
        let Some(name) = v.name.as_deref().filter(|n| !n.is_empty()) else { continue };
        let held = sessions.iter().any(|s| s.is(&v.id) || s.name.as_deref() == Some(name));
        let dup = rows.iter().any(|r| r.name == name);
        if held || dup {
            continue;
        }
        rows.push(NamedRow {
            name: name.to_string(),
            marker: Marker::Saved,
            model: v.model.as_deref().map(short_model).unwrap_or_default(),
            activity: v.updated_at.clone().or_else(|| v.created_at.clone()),
            action: RowAction::Resume(ResumeRequest {
                label: name.to_string(),
                target: v.id.clone(),
                cwd: v.cwd.clone().filter(|c| !c.is_empty()),
                by_name: false,
            }),
        });
    }
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id().cmp(b.id())));
    rows
}

/// The live list below the Named section: sessions without a name.
pub fn unnamed(sessions: &[SessionMeta]) -> impl Iterator<Item = &SessionMeta> {
    sessions.iter().filter(|s| s.name.as_deref().is_none_or(str::is_empty))
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

    fn live(v: serde_json::Value) -> SessionMeta {
        serde_json::from_value(v).unwrap()
    }

    fn saved(v: serde_json::Value) -> SavedSession {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn markers_follow_the_session_state() {
        let base = json!({"id": "s", "lifecycle": "live"});
        let with = |k: &str, v: serde_json::Value| {
            let mut b = base.clone();
            b[k] = v;
            live(b)
        };
        assert_eq!(Marker::of(&live(base.clone())), Marker::Idle);
        assert_eq!(Marker::of(&with("busy", json!(true))), Marker::Busy);
        assert_eq!(Marker::of(&with("lifecycle", json!("parked"))), Marker::Parked);
        assert_eq!(Marker::of(&with("pinned", json!(true))), Marker::Pinned);
        assert_eq!(Marker::of(&with("awaiting_input", json!(1))), Marker::Awaiting);
        let mut both = base.clone();
        both["busy"] = json!(true);
        both["awaiting_input"] = json!(2);
        assert_eq!(Marker::of(&live(both)), Marker::Awaiting);
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

    /// Grouping, sort and dedupe: named live sessions and saved named
    /// sessions the daemon does not hold, alphabetical; a saved entry for a
    /// live session (by id, journal id or name) is dropped; unnamed live
    /// sessions go to the list below.
    #[test]
    fn live_and_saved_are_grouped_sorted_and_deduped() {
        let sessions = vec![
            live(
                json!({"id": "l1", "name": "zeta", "model": "claude-opus-4-6", "lifecycle": "live", "busy": true, "state_since": "2026-11-13T07:00:00Z"}),
            ),
            live(json!({"id": "l2", "name": null, "lifecycle": "live"})),
            live(json!({"id": "l3", "name": "axel", "lifecycle": "parked", "journal_id": "j-axel"})),
            live(json!({"id": "l4-successor", "name": "mid", "lifecycle": "live", "journal_id": "j-mid"})),
            live(json!({"id": "l5", "name": "", "lifecycle": "live"})),
        ];
        let saved = vec![
            saved(json!({"id": "j-axel", "name": "axel", "live": true})),
            saved(json!({"id": "l1", "name": "zeta-old-name", "live": true})),
            saved(json!({"id": "j-mid", "name": "mid", "live": true})),
            saved(
                json!({"id": "j-beta", "name": "beta", "model": "claude-haiku-4-5", "updated_at": "2026-11-12T00:00:00Z", "live": false}),
            ),
            saved(json!({"id": "j-anon", "name": null, "live": false})),
            saved(json!({"id": "j-ghost", "name": "Gamma", "live": true})),
        ];
        let rows = named_rows(&sessions, &saved);
        let got: Vec<(&str, Marker, &str)> = rows.iter().map(|r| (r.name.as_str(), r.marker, r.id())).collect();
        assert_eq!(
            got,
            vec![
                ("axel", Marker::Parked, "l3"),
                ("beta", Marker::Saved, "j-beta"),
                ("Gamma", Marker::Saved, "j-ghost"),
                ("mid", Marker::Idle, "l4-successor"),
                ("zeta", Marker::Busy, "l1"),
            ]
        );
        assert_eq!(
            rows[1].action,
            RowAction::Resume(ResumeRequest { label: "beta".into(), target: "j-beta".into(), cwd: None, by_name: false })
        );
        assert_eq!(rows[1].model, "haiku-4-5");
        assert_eq!(rows[1].activity.as_deref(), Some("2026-11-12T00:00:00Z"));
        assert_eq!(rows[4].action, RowAction::Attach("l1".into()));
        assert_eq!(rows[4].activity.as_deref(), Some("2026-11-13T07:00:00Z"));
        let below: Vec<&str> = unnamed(&sessions).map(|s| s.id.as_str()).collect();
        assert_eq!(below, vec!["l2", "l5"]);
        // Without saved sessions (an older daemon): live named only.
        assert_eq!(named_rows(&sessions, &[]).len(), 3);
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

    /// H1: a saved row carries its folder into `CreateOptions` as is (no
    /// local check), with `~` as the fallback; no folder: `~`.
    #[test]
    fn a_saved_rows_folder_reaches_the_create() {
        let rows = named_rows(&[], &[saved(json!({"id": "j-axel", "name": "axel", "cwd": "/home/u/Projects/axel", "live": false}))]);
        let RowAction::Resume(req) = &rows[0].action else { panic!() };
        let o = req.options();
        assert_eq!(o.cwd, Cwd::Path("/home/u/Projects/axel".into()));
        assert_eq!(o.fallback_cwd, Some(Cwd::Home));
        assert_eq!(o.clone().resume(&req.target).config().continue_session, Some(Some("j-axel".into())));
        let req = ResumeRequest { label: "x".into(), target: "x".into(), cwd: None, by_name: true };
        assert_eq!((req.options().cwd, req.options().fallback_cwd), (Cwd::Home, None));
    }
}
