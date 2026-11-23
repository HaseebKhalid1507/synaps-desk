//! The right panel's prompt cache and keep-warm numbers: when the session's
//! prompt cache expires (a countdown), and keep-warm's mode, pin, pings,
//! spend and next ping. All from the session's `Status` (the daemon's
//! `cache`, `keepwarm`, `pinned`, `cache_ttl`), asked on attach, at a turn's
//! end, on a keep-warm notice, once after a due ping, after a sleep, and
//! when the model or the history changes (which also stops the countdown at
//! once: [`Warm::invalidate`]): never polled.
//!
//! The countdown costs no idle frames: it is redrawn when what it says
//! changes, once a second under ten minutes, once a minute above, only
//! while the panel shows it, and not at all once the cache is cold.
//!
//! A snapshot ages on both of this machine's clocks ([`Moment`]): a suspended
//! laptop wakes to `cold`, not to the minutes it showed when it slept.

use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use synaps_client::{Event, SessionEvent};

/// Under this the countdown shows seconds (and ticks at 1 Hz).
pub(crate) const SECONDS_UNDER: Duration = Duration::from_secs(600);
/// After a due ping, the status is asked again this much later (the ping's
/// new anchor).
const AFTER_PING: Duration = Duration::from_secs(5);
/// Never asked again sooner than this after the last answer, whatever the
/// next ping says (a ping held at `0 s` must not become a poll).
const REFRESH_MIN: Duration = Duration::from_secs(30);

/// The wall clock ran this much further than the monotonic one since a
/// snapshot: the machine slept (or its clock was stepped forward). The
/// snapshot is aged by it and the daemon asked again.
const SLEPT: Duration = Duration::from_secs(5);

/// A moment on both local clocks. The monotonic one (Linux
/// `CLOCK_MONOTONIC`) stops while the machine is suspended; the wall clock
/// does not, but can be set back. A snapshot ages by the larger of the two
/// (the provider's TTL keeps running while this machine sleeps), never by
/// the daemon's clock: a forwarded socket's daemon may disagree with ours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Moment {
    pub mono: Instant,
    pub wall: SystemTime,
}

impl Moment {
    pub fn now() -> Moment {
        Moment { mono: Instant::now(), wall: SystemTime::now() }
    }
}

/// One `Status` answer, read at `at`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Warm {
    pub at: Moment,
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
    pub fn from_status(v: &Value, at: Moment) -> Warm {
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

    /// Since the answer: the larger of the two clocks' (a wall clock set
    /// back counts nothing; a suspended machine's sleep counts).
    fn age(&self, now: Moment) -> Duration {
        let mono = now.mono.saturating_duration_since(self.at.mono);
        let wall = now.wall.duration_since(self.at.wall).unwrap_or_default();
        mono.max(wall)
    }

    /// Until the cache expires, now (zero: cold).
    pub fn left(&self, now: Moment) -> Option<Duration> {
        self.expires.map(|d| d.saturating_sub(self.age(now)))
    }

    /// Until the next keep-warm ping, now.
    pub fn ping_in(&self, now: Moment) -> Option<Duration> {
        self.next_ping.map(|d| d.saturating_sub(self.age(now)))
    }

    /// The machine slept (or its clock jumped ahead) since the answer.
    pub fn slept(&self, now: Moment) -> bool {
        let mono = now.mono.saturating_duration_since(self.at.mono);
        now.wall.duration_since(self.at.wall).is_ok_and(|wall| wall > mono + SLEPT)
    }

    /// The snapshot as it stands now, read again at `now` (after a sleep:
    /// both clocks agree from here on).
    pub fn rebase(&mut self, now: Moment) {
        (self.expires, self.next_ping) = (self.left(now), self.ping_in(now));
        self.at = now;
    }

    /// The history was replaced or the model changed: whatever was cached
    /// is not this session's any more (cold until the daemon says).
    pub fn invalidate(&mut self) {
        self.expires = None;
        self.ttl = None;
        self.downgraded = false;
    }

    /// When what the panel says next changes (a countdown's next second
    /// under ten minutes, its next minute above); `None`: nothing counts
    /// down (no cache known, cold, no ping due).
    pub fn next_change(&self, now: Moment) -> Option<Instant> {
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
        Some(now.mono + soonest + Duration::from_millis(5))
    }

    /// When to ask the daemon again: a little after the next ping is due
    /// (it moves the cache's anchor), and not within [`REFRESH_MIN`] of the
    /// last answer. `None`: no ping is due.
    pub fn refresh_at(&self, now: Moment) -> Option<Instant> {
        self.next_ping.map(|d| now.mono + (d + AFTER_PING).max(REFRESH_MIN).saturating_sub(self.age(now)))
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

/// The TTL as honoured, and what was asked when it differs, short enough
/// for the panel's half column (at most [`TTL_CHARS`]): `5m ↓ 1h` (a 1h
/// request the server kept for 5m), `5m of 1h` (asked 1h, not proved yet).
pub(crate) fn ttl_line(w: &Warm) -> Option<String> {
    let ttl = w.ttl.as_deref()?;
    Some(match w.setting.as_deref() {
        _ if w.downgraded => format!("{ttl} ↓ 1h"),
        Some(s) if s != ttl && s != "auto" => format!("{ttl} of {s}"),
        _ => ttl.to_string(),
    })
}

/// The TTL line's longest, in mono characters: the half column of a
/// 296 px panel holds about 124 px (7.8 px a character at mono 13).
pub(crate) const TTL_CHARS: usize = 15;

/// What a short TTL line means, in words (its tooltip); `None`: it says it.
pub(crate) fn ttl_tip(w: &Warm) -> Option<String> {
    let ttl = w.ttl.as_deref()?;
    match w.setting.as_deref() {
        _ if w.downgraded => Some(format!("1h asked; the server kept {ttl} (a silent downgrade)")),
        Some(s) if s != ttl && s != "auto" => Some(format!("{s} asked; counted as {ttl} until a {s} write proves it")),
        _ => None,
    }
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

    /// `d` s later on both clocks (the machine awake, its clock steady).
    fn after(t: Moment, secs: u64) -> Moment {
        let d = Duration::from_secs(secs);
        Moment { mono: t.mono + d, wall: t.wall + d }
    }

    #[test]
    fn reads_the_status_and_counts_down() {
        let t0 = Moment::now();
        let w = Warm::from_status(&status(json!({"ttl": "5m", "ttl_secs": 300, "expires_in_secs": 222, "downgraded": true})), t0);
        assert_eq!((w.mode.as_str(), w.pinned, w.pings), ("auto", true, 3));
        assert_eq!(mode_line(&w), "auto · pinned");
        assert_eq!(ttl_line(&w).as_deref(), Some("5m ↓ 1h"));
        assert!(ttl_tip(&w).is_some_and(|t| t.contains("downgrade")));
        assert_eq!(clock(w.left(t0).unwrap()), "3:42");
        assert_eq!(clock(w.left(after(t0, 222)).unwrap()), "cold");
        assert_eq!(clock(w.left(after(t0, 400)).unwrap()), "cold", "never below zero");
        assert_eq!(clock(Duration::from_secs(3125)), "52m");
        assert_eq!(clock(Duration::from_secs(600)), "10:00");
    }

    #[test]
    fn ticks_once_a_second_under_ten_minutes_else_once_a_minute_and_stops_cold() {
        let t0 = Moment::now();
        let mut w = Warm::from_status(&status(json!({"ttl": "1h", "expires_in_secs": 3125})), t0);
        w.next_ping = None;
        let wait = |w: &Warm, now: Moment| w.next_change(now).map(|at| at.duration_since(now.mono));
        // 52:05 left: the minute changes in 5 s.
        assert_eq!(wait(&w, t0), Some(Duration::from_millis(5005)));
        // 10:30 left: at the 10:00 line, then every second.
        w.expires = Some(Duration::from_secs(630));
        assert_eq!(wait(&w, t0), Some(Duration::from_millis(30_005)));
        w.expires = Some(Duration::from_millis(599_400));
        assert_eq!(wait(&w, t0), Some(Duration::from_millis(405)));
        // Cold: nothing ticks.
        assert_eq!(wait(&w, after(t0, 700)), None);
        // An older daemon (no `cache`): nothing known, nothing ticks.
        let old = Warm::from_status(&json!({"keepwarm": {"mode": "off"}, "pinned": false}), t0);
        assert!(!old.knows_expiry && old.left(t0).is_none() && old.next_change(t0).is_none());
        assert_eq!(old.refresh_at(t0), None);
        // A due ping: asked again after it.
        let w = Warm::from_status(&status(Value::Null), t0);
        assert!(w.knows_expiry && w.left(t0).is_none());
        assert_eq!(w.refresh_at(t0), Some(t0.mono + Duration::from_secs(3005)));
        // A ping the daemon says is due now, answer after answer: asked at
        // most every 30 s, never a poll.
        let mut w = w;
        w.next_ping = Some(Duration::ZERO);
        assert_eq!(w.refresh_at(t0), Some(t0.mono + REFRESH_MIN));
    }

    /// Suspend: the monotonic clock stops, the wall clock (and the
    /// provider's TTL) runs on. 4:00 left, a minute awake, then a 10-minute
    /// nap: the panel wakes to `cold`, not to 3:00, and nothing ticks.
    #[test]
    fn a_suspended_machine_wakes_to_the_cache_s_real_age() {
        let t0 = Moment::now();
        let mut w = Warm::from_status(&status(json!({"ttl": "5m", "expires_in_secs": 240})), t0);
        w.next_ping = Some(Duration::from_secs(120));
        let awake = after(t0, 60);
        assert_eq!(clock(w.left(awake).unwrap()), "3:00");
        assert!(!w.slept(awake));
        let woke = Moment { mono: awake.mono, wall: awake.wall + Duration::from_secs(600) };
        assert!(w.slept(woke), "the wall clock ran on");
        assert_eq!(clock(w.left(woke).unwrap()), "cold", "not 3:00");
        assert_eq!(w.ping_in(woke), Some(Duration::ZERO), "the ping was due while asleep");
        assert_eq!(w.next_change(woke), None, "cold: nothing ticks");
        assert_eq!(w.refresh_at(woke), Some(woke.mono), "ask the daemon now");
        // Read again at the wake: from here both clocks agree.
        let mut w2 = w.clone();
        w2.rebase(woke);
        assert!(!w2.slept(after(woke, 1)));
        assert_eq!(w2.left(after(woke, 1)), Some(Duration::ZERO));
        // A short nap: aged by it, still counting.
        let w = Warm::from_status(&status(json!({"ttl": "1h", "expires_in_secs": 3000})), t0);
        let napped = Moment { mono: t0.mono + Duration::from_secs(10), wall: t0.wall + Duration::from_secs(610) };
        assert_eq!(clock(w.left(napped).unwrap()), "39m");
    }

    /// A wall clock set back (NTP, a manual change) takes nothing back: the
    /// monotonic clock's age stands, and it is no sleep.
    #[test]
    fn a_clock_set_back_does_not_rewind_the_countdown() {
        let t0 = Moment::now();
        let w = Warm::from_status(&status(json!({"ttl": "5m", "expires_in_secs": 240})), t0);
        let now = Moment { mono: t0.mono + Duration::from_secs(60), wall: t0.wall - Duration::from_secs(3600) };
        assert_eq!(clock(w.left(now).unwrap()), "3:00");
        assert!(!w.slept(now));
        // Ages past zero saturate at cold, on either clock.
        let far = Moment { mono: t0.mono + Duration::from_secs(86_400), wall: t0.wall - Duration::from_secs(1) };
        assert_eq!(w.left(far), Some(Duration::ZERO));
        assert_eq!(clock(w.left(far).unwrap()), "cold");
        assert_eq!(w.next_change(far), None);
    }

    /// A replaced history or another model: the countdown is not this
    /// session's any more (cold until the daemon answers again).
    #[test]
    fn invalidated_the_countdown_stops() {
        let t0 = Moment::now();
        let mut w = Warm::from_status(&status(json!({"ttl": "5m", "expires_in_secs": 240, "downgraded": true})), t0);
        w.next_ping = None;
        assert!(w.next_change(t0).is_some());
        w.invalidate();
        assert_eq!((w.left(t0), w.ttl.as_deref(), w.downgraded), (None, None, false));
        assert!(w.knows_expiry, "cold, not unknown");
        assert_eq!(w.next_change(t0), None);
    }

    /// Every TTL line fits the half column of the default 296 px panel
    /// (finish gate M2: "5m (1h downgraded)" was ~140 px in ~124).
    #[test]
    fn the_ttl_line_fits_its_column() {
        let t0 = Moment::now();
        for (cache, setting) in [
            (json!({"ttl": "5m", "downgraded": true}), "1h"),
            (json!({"ttl": "5m"}), "1h"),
            (json!({"ttl": "1h"}), "1h"),
            (json!({"ttl": "5m"}), "auto"),
            (json!({"ttl": "5m"}), "hybrid"),
        ] {
            let mut st = status(cache);
            st["cache_ttl"]["setting"] = json!(setting);
            let w = Warm::from_status(&st, t0);
            let line = ttl_line(&w).unwrap();
            assert!(line.chars().count() <= TTL_CHARS, "{line:?}");
        }
    }
}
