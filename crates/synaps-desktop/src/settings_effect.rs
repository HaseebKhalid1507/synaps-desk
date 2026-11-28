//! When a change to a daemon setting takes effect, and what the app can
//! see of the value in force (settings-overhaul PLAN §2.1).
//!
//! [`Effect`] is static per key and daemon version: SynapsCLI 0.13.0
//! re-reads its config whenever a session is created, so 17 keys reach new
//! sessions without a reload. [`Evidence`] is recomputed from what the dash
//! read, never from the write history: a row waits only where the app read
//! the value in force and it differs from the file's. Of the daemon's keys
//! only idle exit has one, the dash's `idleExitRunning` (the running
//! daemon's `--idle-exit`). The daemon doesn't report what it loaded for
//! the other 21, so they never wait (§0 #11).

/// When a change to a daemon key applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// The daemon reads the file at each detach (prompt abandon) or park
    /// (parked eviction).
    NextUse,
    /// Sessions created after the save read it (0.13.0 and later).
    NewSessions,
    DaemonReload,
    /// The client that starts a daemon on its own reads it.
    ColdStart,
    /// Each TUI reads it when it attaches.
    NextTui,
}

impl Effect {
    /// The row's tag.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Effect::NextUse => "next use",
            Effect::NewSessions => "new sessions",
            Effect::DaemonReload => "daemon reload",
            Effect::ColdStart => "next daemon start",
            Effect::NextTui => "next TUI launch",
        }
    }

    /// `takes effect at <event>`.
    pub(crate) fn event(self) -> &'static str {
        match self {
            Effect::NextUse => "the daemon's next use",
            Effect::NewSessions => "the next new session",
            Effect::DaemonReload => "the next daemon reload",
            Effect::ColdStart => "the next daemon start",
            Effect::NextTui => "the next TUI launch",
        }
    }
}

const IDLE_EXIT: &str = "daemon.idle_exit_secs";
/// Idle exit when the file doesn't set it (`daemon/mod.rs`).
const IDLE_EXIT_DEFAULT: u64 = 10;

/// The keys a session reads from the config when it's created, on 0.13.0
/// and later (`EngineHost::create_session`); a reload before that.
const NEW_SESSIONS: [&str; 17] = [
    "model",
    "thinking",
    "context_window",
    "compaction_model",
    "compaction_mode",
    "api_retries",
    "refusal_retries",
    "subagent_timeout",
    "max_tool_output",
    "bash_timeout",
    "bash_max_timeout",
    "tools.activation_confirm",
    "context_management.mode",
    "cache_ttl",
    "events.auto_turn",
    "events.auto_turn_cap",
    "startup.extensions_ready_timeout_secs",
];

/// What idle exit's lines add: the file is the third choice.
const UNLESS: &str = ", unless the starting client sets its own (an option or SYNAPS_DAEMON_IDLE_EXIT_SECS)";
/// Idle exit's open row, observed or not (`daemon/mod.rs:1006–1034`).
const IDLE_EXIT_OPEN: &str = "A daemon a client starts on its own reads this, unless that client passes its own idle-exit option or has SYNAPS_DAEMON_IDLE_EXIT_SECS set. A reload keeps the running daemon's argument. A daemon started by hand (synaps daemon) doesn't read it.";
/// A new-session key's open row when the daemon's version can't be read.
const VERSION_NOTE: &str = "on 0.13.0 and later, new sessions use it at once";

/// `version` (a `Welcome`'s `daemon_version`) is 0.13.0 or later. `None`:
/// it isn't a semver 2.0.0 version, whole. A pre-release of 0.13.0 comes
/// before it; build metadata changes nothing.
pub(crate) fn from_0_13(version: &str) -> Option<bool> {
    let (rest, build) = version.split_once('+').map_or((version, None), |(r, b)| (r, Some(b)));
    let (core, pre) = rest.split_once('-').map_or((rest, None), |(c, p)| (c, Some(p)));
    let digits = |i: &str| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit());
    // Dot-separated, none empty, `[0-9A-Za-z-]` only; a pre-release's
    // numeric ones without a leading zero.
    let ids = |s: &str, pre: bool| {
        s.split('.').all(|i| {
            !i.is_empty()
                && i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !(pre && digits(i) && i.len() > 1 && i.starts_with('0'))
        })
    };
    if !pre.is_none_or(|p| ids(p, true)) || !build.is_none_or(|b| ids(b, false)) {
        return None;
    }
    let num = |i: &str| if digits(i) && (i == "0" || !i.starts_with('0')) { i.parse::<u64>().ok() } else { None };
    let mut n = core.split('.').map(num);
    let (Some(Some(a)), Some(Some(b)), Some(Some(c)), None) = (n.next(), n.next(), n.next(), n.next()) else { return None };
    Some((a, b, c) > (0, 13, 0) || ((a, b, c) == (0, 13, 0) && pre.is_none()))
}

/// When a change to `key` takes effect on a daemon of `version`; `None`:
/// not a key synaps-dash writes.
pub(crate) fn effect_of(key: &str, version: &str) -> Option<Effect> {
    Some(match key {
        "daemon.prompt_abandon_secs" | "daemon.parked_evict_secs" => Effect::NextUse,
        "progressive_tool_disclosure" => Effect::DaemonReload,
        IDLE_EXIT => Effect::ColdStart,
        "startup.quick_start" => Effect::NextTui,
        k if NEW_SESSIONS.contains(&k) => match from_0_13(version) {
            Some(true) => Effect::NewSessions,
            _ => Effect::DaemonReload,
        },
        _ => return None,
    })
}

/// What the app can see of the value in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    InForce,
    /// The running value differs from the file's.
    Waits {
        running: u64,
    },
    Unobserved,
}

/// What the app read of the values in force: idle exit's, from the dash.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observed {
    pub idle_exit_running: Option<u64>,
}

/// `key`'s evidence, the file holding `saved` (`None`: unset).
pub(crate) fn evidence(key: &str, saved: Option<&str>, seen: &Observed) -> Evidence {
    let file = saved.map_or(Some(IDLE_EXIT_DEFAULT), |v| v.parse::<u64>().ok());
    match (key == IDLE_EXIT, seen.idle_exit_running, file) {
        (true, Some(running), Some(file)) if running != file => Evidence::Waits { running },
        (true, Some(_), Some(_)) => Evidence::InForce,
        _ => Evidence::Unobserved,
    }
}

/// A daemon row's words under it: its evidence, or this run's save of the
/// value the file holds (`wrote`); open (its whole help shown), idle exit's
/// running value and what overrides the file, or the 0.13.0 note. Each
/// line, and whether it is a wait (`warn`).
pub(crate) fn lines(key: &str, version: &str, saved: Option<&str>, seen: &Observed, wrote: bool, open: bool) -> Vec<(String, bool)> {
    let Some(effect) = effect_of(key, version) else { return Vec::new() };
    let unless = if key == IDLE_EXIT { UNLESS } else { "" };
    let mut out = Vec::new();
    match evidence(key, saved, seen) {
        Evidence::Waits { running } => out.push((format!("running {running} s · takes effect at {}{unless}", effect.event()), true)),
        Evidence::InForce => out.push((if wrote { "saved · in force" } else { "in force" }.to_string(), false)),
        Evidence::Unobserved if wrote => out.push((format!("saved · takes effect at {}{unless}", effect.event()), false)),
        Evidence::Unobserved => {}
    }
    if open && key == IDLE_EXIT {
        match seen.idle_exit_running {
            Some(0) => out.push(("The running daemon never exits idle (its --idle-exit 0).".into(), false)),
            Some(n) => out.push((format!("The running daemon exits after {n} s idle (its --idle-exit)."), false)),
            None => {}
        }
        out.push((IDLE_EXIT_OPEN.into(), false));
    }
    if open && NEW_SESSIONS.contains(&key) && from_0_13(version).is_none() {
        out.push((VERSION_NOTE.into(), false));
    }
    out
}
