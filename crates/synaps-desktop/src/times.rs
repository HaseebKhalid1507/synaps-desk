//! Message times, as the Synaps TUI shows them (`transcript.rs`
//! `push_msg_at`, `note_day`, `day_line`): a row's local `HH:MM`, and a day
//! line where the local date changes between dated rows (the first dated
//! row sets the day without one). A row with no recorded time
//! (`Item::at`) shows none, never "now". The logic only: `turns.rs` places
//! and draws it.
//!
//! The times are the host's: a live row's event `ts` (kept when its history
//! lands, as the TUI's live rows), a row first seen in history its message
//! time (`_synaps_at`), a worker's journal time. Local means the system's
//! zone, read when the row's strings are built; 24-hour, as the TUI.

use chrono::{DateTime, Datelike as _, FixedOffset, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use gpui_kit::SharedString;

/// A row's time, parsed and read in the local zone once (when its row
/// strings are built, `RowCache::time`), never per frame: its date (day
/// lines), its `HH:MM` (the label) and the instant, for the tooltip's
/// readout ([`Stamp::readout`], made only when that opens).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stamp {
    pub(crate) at: DateTime<FixedOffset>,
    pub(crate) hhmm: SharedString,
}

impl Stamp {
    /// An RFC 3339 time read in `tz`.
    pub(crate) fn in_zone<Tz: TimeZone>(at: &str, tz: &Tz) -> Option<Self> {
        let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(tz).fixed_offset();
        Some(Stamp { at, hhmm: at.format("%H:%M").to_string().into() })
    }

    /// An RFC 3339 time read in this system's zone.
    pub(crate) fn local(at: &str) -> Option<Self> {
        Self::in_zone(at, &Local)
    }

    pub(crate) fn day(&self) -> NaiveDate {
        self.at.date_naive()
    }

    /// The whole of it, for the time's tooltip: `Sun Nov 16 2026, 14:05:09`.
    pub(crate) fn readout(&self) -> String {
        self.at.format("%a %b %-d %Y, %H:%M:%S").to_string()
    }
}

/// A day line's date: `Sun Nov 16` (the TUI's `day_line`, without its rules).
pub(crate) fn day_label(day: NaiveDate) -> String {
    day.format("%a %b %-d").to_string()
}

/// A day as a number (a day line's `rev`), and back.
pub(crate) fn day_number(day: NaiveDate) -> u64 {
    day.num_days_from_ce().max(0) as u64
}

pub(crate) fn day_of(n: u64) -> Option<NaiveDate> {
    NaiveDate::from_num_days_from_ce_opt(i32::try_from(n).ok()?)
}

/// A worker's journal entry recorded at `at_ms` on a clock that read
/// `now_ms` when the page was read, which was `wall_ms` (unix ms) here:
/// when it happened, RFC 3339 UTC (as a live row's `ts`).
pub(crate) fn journal(at_ms: u64, now_ms: u64, wall_ms: i64) -> Option<String> {
    let ago = i64::try_from(now_ms.saturating_sub(at_ms)).ok()?;
    let t = DateTime::<Utc>::from_timestamp_millis(wall_ms.checked_sub(ago)?)?;
    Some(t.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The date of the last dated row, walking a transcript in order: a day
/// line where it changes. Only messages' dates (the host's clock, in
/// conversation order) move it; a client note sits after the round it came
/// in, so its time can be earlier than the messages before it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Days(Option<NaiveDate>);

impl Days {
    /// After a row dated `last` (the last dated row before a rebuild's start).
    pub(crate) fn after(last: Option<NaiveDate>) -> Self {
        Days(last)
    }

    /// A row dated `day` would start a day: on another date than the last
    /// dated row.
    pub(crate) fn starts_day(&self, day: Option<NaiveDate>) -> bool {
        self.0.is_some_and(|last| day.is_some_and(|d| d != last))
    }

    /// A row dated `day` comes next: the date it starts, if it starts one.
    pub(crate) fn next(&mut self, day: Option<NaiveDate>) -> Option<NaiveDate> {
        let day = day?;
        self.0.replace(day).filter(|last| *last != day).map(|_| day)
    }
}

/// An RFC 3339 UTC time for a wall time in `tz`: tests that read dates.
#[cfg(test)]
pub(crate) fn utc_in<Tz: TimeZone>(tz: &Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> String {
    let t = tz.with_ymd_and_hms(y, mo, d, h, mi, s).single().expect("not in a zone change");
    t.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Fixed zones for tests: west, a half hour, the farthest east. CI runs in
/// UTC, so a test in one of these fails if a local reading slips to UTC.
#[cfg(test)]
pub(crate) fn zones() -> [FixedOffset; 3] {
    [
        FixedOffset::west_opt(5 * 3600).unwrap(),
        FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap(),
        FixedOffset::east_opt(14 * 3600).unwrap(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse_and_read_in_the_zone() {
        for tz in zones() {
            let at = utc_in(&tz, 2026, 11, 16, 14, 5, 9);
            let t = Stamp::in_zone(&at, &tz).unwrap();
            assert_eq!(t.hhmm, "14:05", "{tz}");
            assert_eq!(t.readout(), "Mon Nov 16 2026, 14:05:09");
            assert_eq!(day_label(t.day()), "Mon Nov 16");
            assert_eq!(day_of(day_number(t.day())), Some(t.day()));
            // 24-hour, both ways round midnight.
            assert_eq!(Stamp::in_zone(&utc_in(&tz, 2026, 11, 16, 23, 59, 59), &tz).unwrap().hhmm, "23:59");
            assert_eq!(Stamp::in_zone(&utc_in(&tz, 2026, 11, 17, 0, 0, 0), &tz).unwrap().hhmm, "00:00");
            // A live event's `ts` (nanoseconds, UTC) and a message time
            // (whole seconds): one minute, one text.
            let live = at.replace('Z', ".794878606Z");
            assert_eq!(Stamp::in_zone(&live, &tz).unwrap().hhmm, "14:05");
            // An offset is honoured, not dropped.
            let fixed = DateTime::parse_from_rfc3339("2026-11-16T14:05:09+02:00").unwrap();
            assert_eq!(Stamp::in_zone("2026-11-16T14:05:09+02:00", &tz).unwrap().at, fixed);
            for bad in ["", "now", "2026-11-16", "2026-11-16 14:05:09", "14:05"] {
                assert!(Stamp::in_zone(bad, &tz).is_none(), "{bad:?}");
            }
        }
        // The zone is the reader's, not UTC's: 19:30 UTC is 00:30 the next
        // day at UTC+5:30, and 14:30 the same day at UTC-5.
        let [west, half, _] = zones();
        let at = "2026-11-16T19:30:00Z";
        let (w, h) = (Stamp::in_zone(at, &west).unwrap(), Stamp::in_zone(at, &half).unwrap());
        assert_eq!((w.hhmm.as_ref(), day_label(w.day()).as_str()), ("14:30", "Mon Nov 16"));
        assert_eq!((h.hhmm.as_ref(), day_label(h.day()).as_str()), ("01:00", "Tue Nov 17"));
        assert_eq!(Stamp::local(at), Stamp::in_zone(at, &Local));
    }

    #[test]
    fn a_journal_time_is_the_wall_clock_less_its_age() {
        // Read at journal 10 000 ms = wall 1 794 556 800 000 (2026-11-13T08:00:00Z).
        let wall = 1_794_556_800_000;
        assert_eq!(journal(4_000, 10_000, wall).as_deref(), Some("2026-11-13T07:59:54.000Z"));
        assert_eq!(journal(10_000, 10_000, wall).as_deref(), Some("2026-11-13T08:00:00.000Z"));
        // A clock read before the entry (never seen): not in the future.
        assert_eq!(journal(12_000, 10_000, wall).as_deref(), Some("2026-11-13T08:00:00.000Z"));
    }
}
