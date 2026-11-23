//! The right panel's prompt cache and keep-warm numbers: when the session's
//! prompt cache expires (a countdown), and keep-warm's mode, pin, pings,
//! spend and next ping. All from the session's `Status` (the daemon's
//! `cache`, `keepwarm`, `pinned`, `cache_ttl`), asked on attach, at a turn's
//! end, on a keep-warm notice and once after a due ping: never polled.
//!
//! The countdown costs no idle frames: it is redrawn when what it says
//! changes, once a second under ten minutes, once a minute above, only
//! while the panel shows it, and not at all once the cache is cold.

use std::time::{Duration, Instant};

use serde_json::Value;
use synaps_client::{Event, SessionEvent};

/// Under this the countdown shows seconds (and ticks at 1 Hz).
pub(crate) const SECONDS_UNDER: Duration = Duration::from_secs(600);
/// After a due ping, the status is asked again this much later (the ping's
/// new anchor).
const AFTER_PING: Duration = Duration::from_secs(5);

/// One `Status` answer, read at `at`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Warm {
    pub at: Instant,
    /// `off`, `auto`, `<N>h`; empty: the daemon did not say (parked).
    pub mode: String,
    pub pinned: bool,
    pub pings: u64,
    pub spent: f64,
    /// The active period's next ping, from `at`.
    pub next_ping: Option<Duration>,
    /// Until the cache expires, from `at`. `None`: nothing known cached.
    pub expires: Option<Duration>,
    /// The daemon says when the cache expires (newer daemons): without it
    /// the panel says it cannot tell, not "cold".
    pub knows_expiry: bool,
    /// The TTL the server honours (`5m`, `1h`), and whether a configured
    /// `1h` was downgraded to it.
    pub ttl: Option<String>,
    pub downgraded: bool,
    /// The configured TTL (`cache_ttl.setting`: `auto`, `1h` …).
    pub setting: Option<String>,
}

impl Warm {
    pub fn from_status(v: &Value, at: Instant) -> Warm {
        let kw = &v["keepwarm"];
        let secs = |v: &Value| v.as_u64().map(Duration::from_secs);
        let cache = &v["cache"];
        Warm {
            at,
            mode: kw["mode"].as_str().unwrap_or_default().to_string(),
            pinned: v["pinned"].as_bool().unwrap_or(false),
            pings: kw["session_pings"].as_u64().unwrap_or(0),
            spent: kw["session_keepwarm_usd"].as_f64().or_else(|| v["keepwarm_cost"].as_f64()).unwrap_or(0.),
            next_ping: kw["period"].is_object().then(|| secs(&kw["period"]["next_ping_in_secs"])).flatten(),
            expires: secs(&cache["expires_in_secs"]),
            knows_expiry: v.get("cache").is_some(),
            ttl: cache["ttl"].as_str().map(str::to_string),
            downgraded: cache["downgraded"].as_bool().unwrap_or(false),
            setting: v["cache_ttl"]["setting"].as_str().map(str::to_string),
        }
    }

    fn since_read(&self, d: Option<Duration>, now: Instant) -> Option<Duration> {
        d.map(|d| d.saturating_sub(now.saturating_duration_since(self.at)))
    }

    /// Until the cache expires, now (zero: cold).
    pub fn left(&self, now: Instant) -> Option<Duration> {
        self.since_read(self.expires, now)
    }

    /// Until the next keep-warm ping, now.
    pub fn ping_in(&self, now: Instant) -> Option<Duration> {
        self.since_read(self.next_ping, now)
    }

    /// When what the panel says next changes (a countdown's next second
    /// under ten minutes, its next minute above); `None`: nothing counts
    /// down (no cache known, cold, no ping due).
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        let step = |d: Duration| -> Option<Duration> {
            if d.is_zero() {
                return None;
            }
            let unit = if d <= SECONDS_UNDER { 1 } else { 60 };
            // The display shows whole units down: it changes as `d` passes
            // the next lower multiple of the unit (or the 10:00 line).
            let floor = Duration::from_secs(d.as_secs() / unit * unit);
            let w = if d > SECONDS_UNDER { (d - floor).min(d - SECONDS_UNDER) } else { d - floor };
            Some(if w.is_zero() { Duration::from_secs(unit).min(d) } else { w })
        };
        let soonest = [self.left(now), self.ping_in(now)].into_iter().flatten().filter_map(step).min()?;
        Some(now + soonest + Duration::from_millis(5))
    }

    /// When to ask the daemon again: a little after the next ping is due
    /// (it moves the cache's anchor). `None`: no ping is due.
    pub fn refresh_at(&self) -> Option<Instant> {
        self.next_ping.map(|d| self.at + d + AFTER_PING)
    }
}

/// `3:42` under ten minutes, `52m` above; `cold` at zero.
pub(crate) fn clock(d: Duration) -> String {
    if d.is_zero() {
        return "cold".into();
    }
    let s = d.as_secs();
    if d <= SECONDS_UNDER { format!("{}:{:02}", s / 60, s % 60) } else { format!("{}m", s / 60) }
}

/// The session's keep-warm, in a word or three: `auto · pinned`.
pub(crate) fn mode_line(w: &Warm) -> String {
    let mode = if w.mode.is_empty() { "—" } else { w.mode.as_str() };
    if w.pinned { format!("{mode} · pinned") } else { mode.to_string() }
}

/// The TTL as honoured, and what was asked when it differs: `5m (1h asked)`.
pub(crate) fn ttl_line(w: &Warm) -> Option<String> {
    let ttl = w.ttl.as_deref()?;
    Some(match w.setting.as_deref() {
        _ if w.downgraded => format!("{ttl} (1h downgraded)"),
        Some(s) if s != ttl && s != "auto" => format!("{ttl} ({s} asked)"),
        _ => ttl.to_string(),
    })
}

/// The status is asked again for this event: an attach (or a reconnect),
/// a turn's end, a keep-warm notice, a park or unpark. Its attachment.
pub(crate) fn asks(ev: &Event) -> Option<u64> {
    match ev {
        Event::Attached { attach_seq, .. } => Some(*attach_seq),
        Event::Session { attach_seq, event, .. } => match event {
            SessionEvent::Idle | SessionEvent::Lifecycle { .. } => Some(*attach_seq),
            SessionEvent::SystemNotice { text } if text.contains("keep-warm") || text.contains("keepwarm") => Some(*attach_seq),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(cache: Value) -> Value {
        json!({
            "pinned": true,
            "cache_ttl": {"setting": "1h", "resolved": "1h"},
            "keepwarm": {"mode": "auto", "active": true, "session_pings": 3, "session_keepwarm_usd": 0.0123,
                         "period": {"next_ping_in_secs": 3000, "ttl_secs": 3600}},
            "cache": cache,
        })
    }

    #[test]
    fn reads_the_status_and_counts_down() {
        let t0 = Instant::now();
        let w = Warm::from_status(&status(json!({"ttl": "5m", "ttl_secs": 300, "expires_in_secs": 222, "downgraded": true})), t0);
        assert_eq!((w.mode.as_str(), w.pinned, w.pings), ("auto", true, 3));
        assert_eq!(mode_line(&w), "auto · pinned");
        assert_eq!(ttl_line(&w).as_deref(), Some("5m (1h downgraded)"));
        assert_eq!(clock(w.left(t0).unwrap()), "3:42");
        assert_eq!(clock(w.left(t0 + Duration::from_secs(222)).unwrap()), "cold");
        assert_eq!(clock(w.left(t0 + Duration::from_secs(400)).unwrap()), "cold", "never below zero");
        assert_eq!(clock(Duration::from_secs(3125)), "52m");
        assert_eq!(clock(Duration::from_secs(600)), "10:00");
    }

    #[test]
    fn ticks_once_a_second_under_ten_minutes_else_once_a_minute_and_stops_cold() {
        let t0 = Instant::now();
        let mut w = Warm::from_status(&status(json!({"ttl": "1h", "expires_in_secs": 3125})), t0);
        w.next_ping = None;
        let wait = |w: &Warm, now: Instant| w.next_change(now).map(|at| at.duration_since(now));
        // 52:05 left: the minute changes in 5 s.
        assert_eq!(wait(&w, t0), Some(Duration::from_millis(5005)));
        // 10:30 left: at the 10:00 line, then every second.
        w.expires = Some(Duration::from_secs(630));
        assert_eq!(wait(&w, t0), Some(Duration::from_millis(30_005)));
        w.expires = Some(Duration::from_millis(599_400));
        assert_eq!(wait(&w, t0), Some(Duration::from_millis(405)));
        // Cold: nothing ticks.
        assert_eq!(wait(&w, t0 + Duration::from_secs(700)), None);
        // An older daemon (no `cache`): nothing known, nothing ticks.
        let old = Warm::from_status(&json!({"keepwarm": {"mode": "off"}, "pinned": false}), t0);
        assert!(!old.knows_expiry && old.left(t0).is_none() && old.next_change(t0).is_none());
        assert_eq!(old.refresh_at(), None);
        // A due ping: asked again after it.
        let w = Warm::from_status(&status(Value::Null), t0);
        assert!(w.knows_expiry && w.left(t0).is_none());
        assert_eq!(w.refresh_at(), Some(t0 + Duration::from_secs(3005)));
    }
}
