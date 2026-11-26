//! The right panel's prompt cache and keep-warm numbers. Two answers, each
//! asked on attach, at a turn's end, on a keep-warm notice or a ping's
//! booking, after a sleep, and when the model or the history changes: never
//! polled. Every ask is the model's ([`crate::model::Model::panel_due`] for
//! the timer's), each a fresh one with its own `stale` retry.
//!
//! - **The cache** ([`Expiry`]): the session's `cache_expiry` query
//!   (SynapsCLI #462): what its own requests show about the prompt cache,
//!   evidence and not provider state. `warm` counts down from receipt,
//!   "about", in a neutral colour; at zero it reads as the amber band (may
//!   have expired) and the daemon is asked once more: once per countdown
//!   (a `warm` of zero seconds starts none). Also asked on any setting
//!   change, and once more on `stale`. A daemon that can't answer it shows
//!   `—`, never `cold`, and no answer it gave before.
//! - **Keep-warm** ([`Warm`]): the session's `Status` (`keepwarm`,
//!   `pinned`): mode, pin, pings, spend and next ping. A ping scheduled is
//!   asked about once after it is due (its schedule only). A ping billed
//!   (its keep-warm tokens or spend grew) asks both. One in flight is
//!   looked at once more [`SETTLE`] on, its status only: SynapsCLI says
//!   nothing of a ping that failed unbilled (a refusal, a connection that
//!   failed), whether it retries or ends the period. The same ping still in
//!   flight waits twice as long, while that is within its period's TTL.
//!
//! The countdowns cost no idle frames: redrawn when what they say changes,
//! once a second under ten minutes, once a minute above, only while the
//! panel shows them, and not at all once nothing counts.
//!
//! An answer ages on both of this machine's clocks ([`Age`]): a suspended
//! laptop wakes to the cache's real age, not to the minutes it showed when
//! it slept, and a clock set back gives none of it back.

use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use synaps_client::{CacheExpiry, CacheReason, CacheState, Event, QueryError, SessionEvent, render_cache_expiry};

/// Under this a countdown shows seconds (and ticks at 1 Hz).
pub(crate) const SECONDS_UNDER: Duration = Duration::from_secs(600);
/// A scheduled ping's status is asked this much after it is due.
const AFTER_PING: Duration = Duration::from_secs(5);
/// ... and never sooner than this after the answer that scheduled it.
const REFRESH_MIN: Duration = Duration::from_secs(30);
/// A ping in flight has its status looked at again this long after the
/// answer that said so. A ping is one request (the transport retries none
/// of it), long over by then: booked, retried (in 5, 15 or 30 s) or ended.
pub(crate) const SETTLE: Duration = Duration::from_secs(60);
/// A period's TTL when its status doesn't say (the longest there is).
const TTL_MAX: Duration = Duration::from_secs(3600);

/// The wall clock ran this much further than the monotonic one since a
/// snapshot: the machine slept (or its clock was stepped forward). The
/// snapshot is aged by it and the daemon asked again.
const SLEPT: Duration = Duration::from_secs(5);

/// A moment on both local clocks. The monotonic one (Linux
/// `CLOCK_MONOTONIC`) stops while the machine is suspended; the wall clock
/// does not, but can be set back. A snapshot ages by the larger of the two
/// ([`Age`]: the provider's TTL keeps running while this machine sleeps),
/// never by the daemon's clock: a forwarded socket's daemon may disagree
/// with ours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Moment {
    pub mono: Instant,
    pub wall: SystemTime,
}

impl Moment {
    pub fn now() -> Moment {
        let now = Moment { mono: Instant::now(), wall: SystemTime::now() };
        #[cfg(test)]
        let now = {
            let (mono, wall) = SKEW.with(std::cell::Cell::get);
            Moment { mono: now.mono + mono, wall: now.wall + wall }
        };
        now
    }
}

/// How old an answer is: what it was last read as, and the larger of the
/// two clocks' steps since (a wall clock set back counts nothing; a
/// suspended machine's sleep counts). Each read keeps what it saw, so the
/// age never goes back (Shady #100 F3): a wall clock that ran ahead and
/// was then corrected, or a suspend and then a correction, gives none of
/// it back. Only a new answer counts afresh.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Age {
    /// When the answer came.
    at: Moment,
    /// The last read, and the age it saw.
    seen: std::cell::Cell<(Moment, Duration)>,
}

impl Age {
    pub fn new(at: Moment) -> Age {
        Age { at, seen: std::cell::Cell::new((at, Duration::ZERO)) }
    }

    /// The age at `now` (kept: no later read says less).
    pub fn at(&self, now: Moment) -> Duration {
        let (last, age) = self.seen.get();
        let step = now.mono.saturating_duration_since(last.mono).max(now.wall.duration_since(last.wall).unwrap_or_default());
        let age = age + step;
        // A read with an earlier monotonic moment than the last: the age
        // stands, and the last read stays the base.
        if now.mono >= last.mono {
            self.seen.set((now, age));
        }
        age
    }

    /// The wall clock ran more than [`SLEPT`] ahead of the monotonic one
    /// since the answer: the machine slept (or its clock was stepped).
    pub fn slept(&self, now: Moment) -> bool {
        self.at(now) > now.mono.saturating_duration_since(self.at.mono) + SLEPT
    }
}

#[cfg(test)]
thread_local! {
    /// How far a test moved this thread's clocks ([`skew`]): monotonic, wall.
    static SKEW: std::cell::Cell<(Duration, Duration)> = const { std::cell::Cell::new((Duration::ZERO, Duration::ZERO)) };
}

/// Tests: [`Moment::now`] on this thread `mono` and `wall` later than it
/// was (a suspend: the wall clock alone).
#[cfg(test)]
pub(crate) fn skew(mono: Duration, wall: Duration) {
    SKEW.with(|s| s.set((s.get().0 + mono, s.get().1 + wall)));
}

/// How long until a countdown showing `d` next changes (its next second
/// under ten minutes, its next minute above, or the 10:00 line); `None` at
/// zero.
fn step(d: Duration) -> Option<Duration> {
    if d.is_zero() {
        return None;
    }
    let unit = if d <= SECONDS_UNDER { 1 } else { 60 };
    // The display shows whole units down: it changes as `d` passes the next
    // lower multiple of the unit (or the 10:00 line).
    let floor = Duration::from_secs(d.as_secs() / unit * unit);
    let w = if d > SECONDS_UNDER { (d - floor).min(d - SECONDS_UNDER) } else { d - floor };
    Some(if w.is_zero() { Duration::from_secs(unit).min(d) } else { w })
}

/// One `Status` answer's keep-warm, read at `at`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Warm {
    pub at: Age,
    /// `off`, `auto`, `<N>h`; empty: the daemon did not say (parked).
    pub mode: String,
    pub pinned: bool,
    pub pings: u64,
    pub spent: f64,
    /// The active period's next ping, from `at`.
    pub next_ping: Option<Duration>,
    /// The status is asked once after that ping is due ([`Warm::refresh_at`]):
    /// a ping scheduled (`next_ping_in_secs` above zero, not `in_flight`),
    /// not asked yet.
    pub refresh: bool,
    /// A ping in flight (SynapsCLI says `0`, or `in_flight`): its status is
    /// looked at once more ([`Flight`]), never polled (Shady #100 F4).
    pub flight: Option<Flight>,
}

/// A ping in flight, as a status said it. SynapsCLI emits nothing when a
/// ping fails unbilled, so its end is looked for: once, [`SETTLE`] after
/// the answer; an answer that says the same ping waits twice as long as
/// the last, and none past the period's TTL (no retry is scheduled past
/// it), so one ping costs a few asks at most, status only (Shady #100 R2.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flight {
    /// The period's `id`, `pings` and `ledger_usd`: what its booking changes.
    key: (Option<u64>, Option<u64>, Option<u64>),
    /// The look, from the answer.
    pub wait: Duration,
    /// Looked (asked since), or none: past the TTL.
    pub looked: bool,
}

impl Warm {
    /// The status `v`, answered at `at`; `prev`: what the store held (the
    /// same ping still in flight waits twice as long as its last look).
    pub fn from_status(v: &Value, at: Moment, prev: Option<&Warm>) -> Warm {
        let kw = &v["keepwarm"];
        let period = &kw["period"];
        let next_ping = period["next_ping_in_secs"].as_u64().map(Duration::from_secs);
        let in_flight = period.is_object() && (period["in_flight"] == true || next_ping.is_some_and(|d| d.is_zero()));
        let flight = in_flight.then(|| {
            let key = (period["id"].as_u64(), period["pings"].as_u64(), period["ledger_usd"].as_f64().map(f64::to_bits));
            let wait = prev.and_then(|w| w.flight).filter(|f| f.key == key).map_or(SETTLE, |f| f.wait * 2);
            let ttl = period["ttl_secs"].as_u64().map_or(TTL_MAX, Duration::from_secs);
            Flight { key, wait, looked: wait > ttl }
        });
        Warm {
            at: Age::new(at),
            mode: kw["mode"].as_str().unwrap_or_default().to_string(),
            pinned: v["pinned"].as_bool().unwrap_or(false),
            pings: kw["session_pings"].as_u64().unwrap_or(0),
            spent: kw["session_keepwarm_usd"].as_f64().or_else(|| v["keepwarm_cost"].as_f64()).unwrap_or(0.),
            next_ping,
            refresh: next_ping.is_some_and(|d| !d.is_zero()) && !in_flight,
            flight,
        }
    }

    /// Its status asked: nothing more is due of this answer.
    pub fn asked(&mut self) {
        self.refresh = false;
        if let Some(f) = self.flight.as_mut() {
            f.looked = true;
        }
    }

    /// Until the next keep-warm ping, now.
    pub fn ping_in(&self, now: Moment) -> Option<Duration> {
        self.next_ping.map(|d| d.saturating_sub(self.at.at(now)))
    }

    /// The machine slept (or its clock jumped ahead) since the answer.
    pub fn slept(&self, now: Moment) -> bool {
        self.at.slept(now)
    }

    /// The snapshot as it stands now, read again at `now` (after a sleep:
    /// both clocks agree from here on).
    pub fn rebase(&mut self, now: Moment) {
        self.next_ping = self.ping_in(now);
        self.at = Age::new(now);
    }

    /// When the next-ping countdown next changes; `None`: no ping due.
    pub fn next_change(&self, now: Moment) -> Option<Instant> {
        self.ping_in(now).and_then(step).map(|d| now.mono + d + Duration::from_millis(5))
    }

    /// When to ask for the status again: a little after the ping it
    /// schedules is due, and not within [`REFRESH_MIN`] of the answer; or a
    /// ping in flight's look ([`Flight`]). `None`: neither, or asked already.
    pub fn refresh_at(&self, now: Moment) -> Option<Instant> {
        let ping = self.next_ping.filter(|d| self.refresh && !d.is_zero()).map(|d| (d + AFTER_PING).max(REFRESH_MIN));
        let look = self.flight.filter(|f| !f.looked).map(|f| f.wait);
        Some(now.mono + ping.or(look)?.saturating_sub(self.at.at(now)))
    }
}

/// One `cache_expiry` answer, read at `at`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Expiry {
    pub at: Age,
    pub answer: Result<CacheExpiry, QueryError>,
    /// The daemon is asked once more when the `warm` countdown reaches zero
    /// ([`Expiry::zero_at`]): a countdown of its own (`warm_for_secs` above
    /// zero), not asked yet. Spent by that ask, and never set again by an
    /// answer with nothing left to count: SynapsCLI says `warm` with `0`
    /// through the last second, and asking at each of those is a loop
    /// (Shady #100 F1).
    pub zero: bool,
}

impl Expiry {
    pub fn new(answer: Result<CacheExpiry, QueryError>, at: Moment) -> Expiry {
        let zero = answer.as_ref().is_ok_and(|c| c.state == CacheState::Warm && c.warm_for_secs.is_some_and(|s| s > 0));
        Expiry { at: Age::new(at), answer, zero }
    }

    /// `warm`: what is left of `warm_for_secs` now (zero: the band).
    pub fn warm_left(&self, now: Moment) -> Option<Duration> {
        let c = self.answer.as_ref().ok().filter(|c| c.state == CacheState::Warm)?;
        Some(Duration::from_secs(c.warm_for_secs?).saturating_sub(self.at.at(now)))
    }

    /// The answer as it stands now: its durations aged (lower bounds
    /// floored, upper bounds ceiled), a `warm` run out read as the band.
    /// Never extended: nothing here counts up.
    pub fn aged(&self, now: Moment) -> Result<CacheExpiry, QueryError> {
        let mut c = self.answer.clone()?;
        let age = self.at.at(now);
        let down = |s: Option<u64>| s.map(|s| Duration::from_secs(s).saturating_sub(age).as_secs());
        let up = |s: Option<u64>| {
            s.map(|s| {
                let d = Duration::from_secs(s).saturating_sub(age);
                d.as_secs() + u64::from(d.subsec_nanos() > 0)
            })
        };
        // A lapse run out says nothing more (the daemon, asked again, says
        // what it is now).
        (c.lapse_by_secs, c.next_ping_in_secs) = (up(c.lapse_by_secs).filter(|s| *s > 0), down(c.next_ping_in_secs));
        if c.state == CacheState::Warm && self.warm_left(now).is_some_and(|d| d.is_zero()) {
            (c.state, c.reason, c.warm_for_secs) = (CacheState::Unknown, Some(CacheReason::MayHaveExpired), None);
        } else {
            c.warm_for_secs = down(c.warm_for_secs);
        }
        Ok(c)
    }

    /// When the `warm` countdown next changes; `None`: nothing counts.
    pub fn next_change(&self, now: Moment) -> Option<Instant> {
        self.warm_left(now).and_then(step).map(|d| now.mono + d + Duration::from_millis(5))
    }

    /// When the `warm` countdown reaches zero (the daemon is asked once
    /// more then); `None`: no countdown of its own, or asked already.
    pub fn zero_at(&self, now: Moment) -> Option<Instant> {
        self.warm_left(now).filter(|_| self.zero).map(|d| now.mono + d)
    }
}

/// A cell's colour: the panel's ink, amber, or dimmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Ink,
    Warn,
    Dim,
}

/// What the EXPIRES cell says: its text, colour and tooltip (the daemon's
/// recommended words for the answer, aged).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cell {
    pub text: String,
    pub tone: Tone,
    pub tip: Option<String>,
}

/// This daemon can't say (no `cache_expiry`, or it failed the query).
pub(crate) const CANT_TELL: &str = "Can't tell: this daemon doesn't report the prompt cache's expiry";

/// The EXPIRES cell for `e`, now. `asked`: this daemon is asked (it lists
/// `cache_expiry` and has not refused it). Without an answer the cell says
/// it cannot tell, never `cold`; from a daemon that is not asked, nothing
/// it said before either (Shady #100 F2).
pub(crate) fn expires(e: Option<&Expiry>, asked: bool, now: Moment) -> Cell {
    let cell = |text: &str, tone, tip: Option<String>| Cell { text: text.to_string(), tone, tip };
    let Some(e) = e.filter(|_| asked) else {
        return cell("—", Tone::Dim, (!asked).then(|| CANT_TELL.to_string()));
    };
    let c = match e.aged(now) {
        Ok(c) => c,
        Err(err) => return cell("—", Tone::Dim, Some(format!("couldn't check ({err})"))),
    };
    let tip = Some(render_cache_expiry(&c));
    match (c.state, c.reason) {
        (CacheState::Warm, _) => match e.warm_left(now) {
            Some(left) => Cell { text: format!("about {}", clock(left)), tone: Tone::Ink, tip },
            None => cell("warm", Tone::Ink, tip),
        },
        (CacheState::Unknown, Some(CacheReason::MayHaveExpired)) => cell("expired?", Tone::Warn, tip),
        (CacheState::Cold, Some(CacheReason::Uncached)) => cell("uncached", Tone::Dim, tip),
        (CacheState::Cold, _) => cell("cold", Tone::Dim, tip),
        (_, Some(CacheReason::NoMatchingRequest)) => cell("no match", Tone::Dim, tip),
        _ => cell("—", Tone::Dim, tip),
    }
}

/// `3:42` under ten minutes, `52m` above; `0:00` at zero.
pub(crate) fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if d <= SECONDS_UNDER { format!("{}:{:02}", s / 60, s % 60) } else { format!("{}m", s / 60) }
}

/// A TTL in its own unit: `5m`, `1h`, `90s`.
fn ttl(secs: u64) -> String {
    match secs {
        s if s >= 3600 && s % 3600 == 0 => format!("{}h", s / 3600),
        s if s >= 60 && s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// The session's keep-warm, in a word or three: `auto · pinned`.
pub(crate) fn mode_line(w: &Warm) -> String {
    let mode = if w.mode.is_empty() { "—" } else { w.mode.as_str() };
    if w.pinned { format!("{mode} · pinned") } else { mode.to_string() }
}

/// The TTL the server is known to honour, and what was asked when it
/// differs, short enough for the panel's half column (at most 15 mono
/// characters: about 124 px at the default 296 px panel, 7.8 px a character
/// at mono 13): `5m of 1h` (1h asked, 5m proved). `None`: the answer names
/// none.
pub(crate) fn ttl_line(e: &Expiry) -> Option<String> {
    let c = e.answer.as_ref().ok()?;
    let proof = c.proof_ttl_secs?;
    Some(match c.requested_ttl_secs {
        Some(asked) if asked != proof => format!("{} of {}", ttl(proof), ttl(asked)),
        _ => ttl(proof),
    })
}

/// What a short TTL line means, in words (its tooltip); `None`: it says it.
pub(crate) fn ttl_tip(e: &Expiry) -> Option<String> {
    let c = e.answer.as_ref().ok()?;
    let (proof, asked) = (c.proof_ttl_secs?, c.requested_ttl_secs?);
    (asked != proof).then(|| {
        let (proof, asked) = (ttl(proof), ttl(asked));
        format!("{asked} asked; counted as {proof}, what the server is known to honour (no {asked} write seen yet, or a downgrade)")
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
    use serde_json::json;
    use synaps_client::QueryErrorCode;

    use super::*;

    fn status() -> Value {
        json!({
            "pinned": true,
            "cache_ttl": {"setting": "1h", "resolved": "1h"},
            "keepwarm": {"mode": "auto", "active": true, "session_pings": 3, "session_keepwarm_usd": 0.0123,
                         "period": {"next_ping_in_secs": 3000, "ttl_secs": 3600}},
        })
    }

    /// The daemon's answer `v`, read at `at`.
    fn expiry(v: Value, at: Moment) -> Expiry {
        Expiry::new(synaps_client::decode_query_reply(&v), at)
    }

    /// `d` s later on both clocks (the machine awake, its clock steady).
    fn after(t: Moment, secs: u64) -> Moment {
        let d = Duration::from_secs(secs);
        Moment { mono: t.mono + d, wall: t.wall + d }
    }

    fn says(c: &Cell) -> (&str, Tone) {
        (c.text.as_str(), c.tone)
    }

    #[test]
    fn reads_the_keep_warm_from_the_status() {
        let t0 = Moment::now();
        let w = Warm::from_status(&status(), t0, None);
        assert_eq!((w.mode.as_str(), w.pinned, w.pings, w.spent), ("auto", true, 3, 0.0123));
        assert_eq!(mode_line(&w), "auto · pinned");
        assert_eq!(w.ping_in(after(t0, 600)), Some(Duration::from_secs(2400)));
        // A status that names no `cache` is all it takes: nothing in it is
        // the cache's.
        let parked = Warm::from_status(&json!({"keepwarm": {}}), t0, None);
        assert_eq!((mode_line(&parked), parked.next_ping), ("—".to_string(), None));
    }

    /// Each state the panel shows: what the cell says, in which colour,
    /// and its tooltip (the daemon's recommended words).
    #[test]
    fn each_state_says_what_it_knows() {
        let t0 = Moment::now();
        let cell = |v: Value| expires(Some(&expiry(v, t0)), true, t0);
        let warm = json!({"state": "warm", "warm_for_secs": 222, "lapse_by_secs": 222, "proof_ttl_secs": 300,
            "requested_ttl_secs": 300, "proof_cache_tokens": 20000, "next_ping_in_secs": 200});
        let c = cell(warm);
        assert_eq!(says(&c), ("about 3:42", Tone::Ink), "neutral, never green");
        assert_eq!(
            c.tip.as_deref(),
            Some(
                "Recent cache activity · about 3m left · 20000 tokens read or written by the last measured request · next keep-warm attempt in about 3m"
            )
        );
        assert_eq!(says(&cell(json!({"state": "warm", "warm_for_secs": 3125}))), ("about 52m", Tone::Ink));
        let c = cell(json!({"state": "unknown", "reason": "may_have_expired", "lapse_by_secs": 2400}));
        assert_eq!(says(&c), ("expired?", Tone::Warn));
        assert_eq!(c.tip.as_deref(), Some("May have expired · this session's last possible refresh ends in about 40m"));
        let c = cell(json!({"state": "cold", "reason": "expired"}));
        assert_eq!((says(&c), c.tip.as_deref()), (("cold", Tone::Dim), Some("No unexpired evidence from this session")));
        let c = cell(json!({"state": "cold", "reason": "uncached"}));
        assert_eq!((says(&c), c.tip.as_deref()), (("uncached", Tone::Dim), Some("Last measured request reported no cache activity")));
        let c = cell(json!({"state": "unknown", "reason": "no_matching_request"}));
        assert_eq!((says(&c), c.tip.as_deref()), (("no match", Tone::Dim), Some("no matching request yet")));
        for (reason, said) in [
            ("parked", "parked"),
            ("busy", "a turn is running"),
            ("unmeasured", "last request not measured yet"),
            ("history_unsettled", "last turn didn't finish"),
            ("rollover_pending", "next turn starts a new context"),
            ("not_anthropic", "not tracked for this provider"),
            ("probe_failed", "couldn't check"),
            ("meteor", "couldn't check"),
        ] {
            let c = cell(json!({"state": "unknown", "reason": reason}));
            assert_eq!((says(&c), c.tip.as_deref()), (("—", Tone::Dim), Some(said)), "{reason}: no timer");
            assert_eq!(expiry(json!({"state": "unknown", "reason": reason}), t0).next_change(t0), None);
        }
        // A newer daemon's state: unknown, said as such.
        let c = cell(json!({"state": "lukewarm"}));
        assert_eq!((says(&c), c.tip.as_deref()), (("—", Tone::Dim), Some("couldn't check")));
        // A typed error: `—`, and why.
        let c = cell(json!({"error": {"code": "busy", "message": "four jobs live"}}));
        assert_eq!((says(&c), c.tip.as_deref()), (("—", Tone::Dim), Some("couldn't check (busy: four jobs live)")));
    }

    /// No answer: `—`, never `cold`. From a daemon that can't say (an older
    /// one, or one that refused the query) the tooltip says so.
    #[test]
    fn without_an_answer_it_cannot_tell() {
        let t0 = Moment::now();
        assert_eq!(expires(None, false, t0), Cell { text: "—".into(), tone: Tone::Dim, tip: Some(CANT_TELL.into()) });
        assert_eq!(expires(None, true, t0), Cell { text: "—".into(), tone: Tone::Dim, tip: None }, "asked, not answered yet");
    }

    /// `warm` counts down from receipt, never past zero; at zero it is the
    /// amber band (and the daemon is asked once more), not cold.
    #[test]
    fn warm_counts_down_then_is_the_band() {
        let t0 = Moment::now();
        let e = expiry(json!({"state": "warm", "warm_for_secs": 222, "lapse_by_secs": 3600, "proof_ttl_secs": 300}), t0);
        assert_eq!(says(&expires(Some(&e), true, after(t0, 100))), ("about 2:02", Tone::Ink));
        let zero = after(t0, 222);
        let c = expires(Some(&e), true, zero);
        assert_eq!(says(&c), ("expired?", Tone::Warn));
        assert_eq!(c.tip.as_deref(), Some("May have expired · this session's last possible refresh ends in about 57m"));
        let c = expires(Some(&e), true, after(t0, 9999));
        assert_eq!((says(&c), c.tip.as_deref()), (("expired?", Tone::Warn), Some("May have expired")), "never cold by itself");
        assert_eq!(e.next_change(zero), None, "nothing ticks at zero");
        let mut e = expiry(json!({"state": "warm", "warm_for_secs": 222}), t0);
        assert_eq!(e.zero_at(t0), Some(t0.mono + Duration::from_secs(222)));
        e.zero = false;
        assert_eq!(e.zero_at(t0), None, "asked once");
        // A `warm` with nothing left to count (its last second): no zero of
        // its own to ask at.
        let e = expiry(json!({"state": "warm", "warm_for_secs": 0}), t0);
        assert_eq!((says(&expires(Some(&e), true, t0)), e.zero_at(t0)), (("expired?", Tone::Warn), None));
        // Never extended: the answer's lapse is no timer.
        let band = expiry(json!({"state": "unknown", "reason": "may_have_expired", "lapse_by_secs": 2400}), t0);
        assert_eq!((band.next_change(t0), band.zero_at(t0)), (None, None));
    }

    #[test]
    fn ticks_once_a_second_under_ten_minutes_else_once_a_minute() {
        let t0 = Moment::now();
        let wait = |e: &Expiry, now: Moment| e.next_change(now).map(|at| at.duration_since(now.mono));
        let warm = |secs: u64| expiry(json!({"state": "warm", "warm_for_secs": secs}), t0);
        // 52:05 left: the minute changes in 5 s.
        assert_eq!(wait(&warm(3125), t0), Some(Duration::from_millis(5005)));
        // 10:30 left: at the 10:00 line, then every second.
        assert_eq!(wait(&warm(630), t0), Some(Duration::from_millis(30_005)));
        assert_eq!(wait(&warm(600), t0), Some(Duration::from_millis(1005)));
        assert_eq!(clock(Duration::from_secs(600)), "10:00");
        // The next ping counts the same way.
        let mut w = Warm::from_status(&status(), t0, None);
        w.next_ping = Some(Duration::from_secs(42));
        assert_eq!(w.next_change(t0).map(|at| at.duration_since(t0.mono)), Some(Duration::from_millis(1005)));
        // A scheduled ping: its status asked after it is due, not within
        // 30 s of the answer.
        assert_eq!(w.refresh_at(t0), Some(t0.mono + Duration::from_secs(47)));
        w.next_ping = Some(Duration::from_secs(3));
        assert_eq!(w.refresh_at(t0), Some(t0.mono + REFRESH_MIN));
        w.next_ping = Some(Duration::from_secs(3000));
        assert_eq!(w.refresh_at(t0), Some(t0.mono + Duration::from_secs(3005)));
        // Asked: spent.
        w.asked();
        assert_eq!(w.refresh_at(t0), None);
        // Due or in flight (`0`): looked at once, a minute on.
        for period in [
            json!({"next_ping_in_secs": 0, "in_flight": true}),
            json!({"next_ping_in_secs": 0}),
            json!({"next_ping_in_secs": 9, "in_flight": true}),
        ] {
            let mut w = Warm::from_status(&json!({"keepwarm": {"mode": "auto", "period": period}}), t0, None);
            assert_eq!(w.refresh_at(t0), Some(t0.mono + SETTLE), "{period}");
            w.asked();
            assert_eq!(w.refresh_at(t0), None, "{period}: once");
        }
        // No period: nothing in flight, nothing scheduled.
        let w = Warm::from_status(&json!({"keepwarm": {"mode": "auto", "period": null, "last_end": "4xx"}}), t0, None);
        assert_eq!((w.flight, w.refresh_at(t0)), (None, None));
        let mut w = Warm::from_status(&status(), t0, None);
        w.next_ping = None;
        assert_eq!((w.next_change(t0), w.refresh_at(t0)), (None, None));
    }

    /// Shady #100 R2.1: a ping in flight is looked at once, a minute on;
    /// each answer that says the same ping (its period, pings and ledger)
    /// waits twice as long as the last, and none past the period's TTL.
    /// Anything else (booked, retried, another period) starts again.
    #[test]
    fn a_ping_in_flight_is_looked_at_a_few_times_at_most() {
        let t0 = Moment::now();
        let st = |id: u64, pings: u64, ledger: f64| {
            json!({"keepwarm": {"mode": "auto", "period": {"id": id, "pings": pings, "ledger_usd": ledger,
                "ttl_secs": 300, "next_ping_in_secs": 0, "in_flight": true}}})
        };
        let look = |w: &Warm| w.refresh_at(t0).map(|at| at.duration_since(t0.mono).as_secs());
        let mut w = Warm::from_status(&st(1, 2, 0.5), t0, None);
        let mut looks = vec![look(&w)];
        for _ in 0..4 {
            w.asked();
            w = Warm::from_status(&st(1, 2, 0.5), t0, Some(&w));
            looks.push(look(&w));
        }
        assert_eq!(looks, [Some(60), Some(120), Some(240), None, None], "within the 5m TTL, then none");
        // Booked (pings, or a worst case's ledger), or another period: anew.
        for other in [st(1, 3, 0.5), st(1, 2, 0.75), st(2, 2, 0.5)] {
            assert_eq!(look(&Warm::from_status(&other, t0, Some(&w))), Some(60), "{other}");
        }
        // Scheduled in between (a retry): the next flight starts anew.
        let retry = Warm::from_status(
            &json!({"keepwarm": {"mode": "auto", "period": {"id": 1, "pings": 2, "ledger_usd": 0.5,
            "ttl_secs": 300, "next_ping_in_secs": 15}}}),
            t0,
            Some(&w),
        );
        assert_eq!((retry.flight, look(&retry)), (None, Some(30)));
        assert_eq!(look(&Warm::from_status(&st(1, 2, 0.5), t0, Some(&retry))), Some(60));
        // An hour's period: six looks at most.
        let hour = |prev: Option<&Warm>| {
            Warm::from_status(
                &json!({"keepwarm": {"mode": "auto", "period": {"id": 1, "in_flight": true, "next_ping_in_secs": 0}}}),
                t0,
                prev,
            )
        };
        let mut w = hour(None);
        let mut n = 0;
        while look(&w).is_some() {
            n += 1;
            w = hour(Some(&w));
        }
        assert_eq!(n, 6, "60 s to 32 min");
    }

    /// Suspend: the monotonic clock stops, the wall clock (and the
    /// provider's TTL) runs on. 4:00 left, a minute awake, then a 10-minute
    /// nap: the panel wakes to the band, not to 3:00, and the daemon is
    /// asked again.
    #[test]
    fn a_suspended_machine_wakes_to_the_cache_s_real_age() {
        let t0 = Moment::now();
        let e = expiry(json!({"state": "warm", "warm_for_secs": 240}), t0);
        let mut w = Warm::from_status(&status(), t0, None);
        w.next_ping = Some(Duration::from_secs(120));
        let awake = after(t0, 60);
        assert_eq!(expires(Some(&e), true, awake).text, "about 3:00");
        assert!(!w.slept(awake));
        let woke = Moment { mono: awake.mono, wall: awake.wall + Duration::from_secs(600) };
        assert!(w.slept(woke), "the wall clock ran on");
        assert_eq!(expires(Some(&e), true, woke).text, "expired?", "not 3:00");
        assert_eq!(w.ping_in(woke), Some(Duration::ZERO), "the ping was due while asleep");
        assert_eq!(e.next_change(woke), None, "nothing ticks");
        assert_eq!(w.refresh_at(woke), Some(woke.mono), "ask the daemon now");
        // Read again at the wake: from here both clocks agree.
        w.rebase(woke);
        assert!(!w.slept(after(woke, 1)));
        // A short nap: aged by it, still counting.
        let e = expiry(json!({"state": "warm", "warm_for_secs": 3000}), t0);
        let napped = Moment { mono: t0.mono + Duration::from_secs(10), wall: t0.wall + Duration::from_secs(610) };
        assert_eq!(expires(Some(&e), true, napped).text, "about 39m");
    }

    /// A wall clock set back (NTP, a manual change) takes nothing back: the
    /// monotonic clock's age stands, and it is no sleep.
    #[test]
    fn a_clock_set_back_does_not_rewind_the_countdown() {
        let t0 = Moment::now();
        let e = expiry(json!({"state": "warm", "warm_for_secs": 240}), t0);
        let now = Moment { mono: t0.mono + Duration::from_secs(60), wall: t0.wall - Duration::from_secs(3600) };
        assert_eq!(expires(Some(&e), true, now).text, "about 3:00");
        assert!(!Warm::from_status(&status(), t0, None).slept(now));
        let far = Moment { mono: t0.mono + Duration::from_secs(86_400), wall: t0.wall - Duration::from_secs(1) };
        assert_eq!(e.warm_left(far), Some(Duration::ZERO));
        assert_eq!(e.next_change(far), None);
    }

    /// Shady #100 F3: what one answer has been read as aged stays aged. A
    /// wall clock that ran ahead and was set back, or a suspend and then a
    /// correction, never gives a countdown back: the remaining time never
    /// rises, and a run-out answer never counts again, before the daemon
    /// answers anew.
    #[test]
    fn a_clock_corrected_back_never_raises_the_countdown() {
        let t0 = Moment::now();
        let at = |mono: u64, wall: u64| Moment { mono: t0.mono + Duration::from_secs(mono), wall: t0.wall + Duration::from_secs(wall) };
        // The wall clock 4 s ahead (under the 5 s sleep line), then corrected.
        let e = expiry(json!({"state": "warm", "warm_for_secs": 240}), t0);
        assert_eq!(expires(Some(&e), true, at(10, 14)).text, "about 3:46");
        assert_eq!(expires(Some(&e), true, at(11, 11)).text, "about 3:45", "one second on, not three back");
        let w = Warm::from_status(&status(), t0, None);
        assert_eq!(w.ping_in(at(10, 14)), Some(Duration::from_secs(2986)));
        assert_eq!(w.ping_in(at(11, 11)), Some(Duration::from_secs(2985)), "the next ping's countdown too");
        // Run out by a wall clock far ahead, then corrected: still run out.
        let e = expiry(json!({"state": "warm", "warm_for_secs": 240}), t0);
        assert_eq!(expires(Some(&e), true, at(10, 610)).text, "expired?");
        let back = at(20, 20);
        assert_eq!(expires(Some(&e), true, back).text, "expired?", "not resurrected");
        assert_eq!((e.warm_left(back), e.next_change(back)), (Some(Duration::ZERO), None));
        // Awake a minute, a suspend, then the clock corrected before any
        // new answer: the band stays.
        let e = expiry(json!({"state": "warm", "warm_for_secs": 240}), t0);
        assert_eq!(expires(Some(&e), true, at(60, 60)).text, "about 3:00");
        assert_eq!(expires(Some(&e), true, at(60, 660)).text, "expired?");
        assert_eq!(expires(Some(&e), true, at(61, 61)).text, "expired?");
        // Read in any order, never more left than the last read said.
        let e = expiry(json!({"state": "warm", "warm_for_secs": 240}), t0);
        let mut last = Duration::MAX;
        for (mono, wall) in [(1, 3), (2, 2), (3, 9), (4, 4), (5, 5), (5, 1), (9, 9)] {
            let left = e.warm_left(at(mono, wall)).unwrap();
            assert!(left <= last, "at ({mono}, {wall}): {left:?} after {last:?}");
            last = left;
        }
    }

    /// The TTL the server is known to honour, and what was asked when it
    /// differs; every line fits the half column of the default 296 px
    /// panel (finish gate M2).
    #[test]
    fn the_ttl_line_says_what_is_proved_and_fits_its_column() {
        let t0 = Moment::now();
        let line = |proof: u64, asked: u64| {
            let e = expiry(json!({"state": "warm", "warm_for_secs": 1, "proof_ttl_secs": proof, "requested_ttl_secs": asked}), t0);
            (ttl_line(&e).unwrap(), ttl_tip(&e))
        };
        assert_eq!(line(300, 300), ("5m".to_string(), None));
        assert_eq!(line(3600, 3600), ("1h".to_string(), None));
        let (l, tip) = line(300, 3600);
        assert_eq!(l, "5m of 1h");
        assert!(tip.is_some_and(|t| t.starts_with("1h asked; counted as 5m")));
        for (proof, asked) in [(300, 3600), (3599, 7200), (59, 3600), (5400, 7200)] {
            let l = line(proof, asked).0;
            assert!(l.chars().count() <= 15, "{l:?}");
        }
        // No TTL named (no anchor read, an error): `—`.
        assert_eq!(ttl_line(&expiry(json!({"state": "unknown", "reason": "parked"}), t0)), None);
        let e = expiry(json!({"error": {"code": "timeout"}}), t0);
        assert_eq!((ttl_line(&e), e.answer.unwrap_err().code), (None, QueryErrorCode::Timeout));
    }
}
