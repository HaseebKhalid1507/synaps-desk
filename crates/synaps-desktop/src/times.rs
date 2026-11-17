//! Message times, as the Synaps TUI shows them (`transcript.rs`
//! `push_msg_at`, `note_day`, `day_line`): a row's local `HH:MM`, and a day
//! line where the local date changes between dated rows (the first dated
//! row sets the day without one). A row with no recorded time
//! ([`Item::at`]) shows none, never "now". The logic only: `turns.rs`
//! places and draws it.
//!
//! The times are the host's: a history row's message time (`_synaps_at`),
//! a live row's event `ts`, a worker's journal time. Local means the
//! system's zone, read when the rows are built; 24-hour, as the TUI.

use chrono::{DateTime, Datelike as _, Local, NaiveDate, SecondsFormat, Utc};
use synaps_client::transcript::Item;

/// An RFC 3339 time in local time.
pub(crate) fn local(at: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(at).ok().map(|t| t.with_timezone(&Local))
}

/// When `item` happened, in local time.
pub(crate) fn of(item: &Item) -> Option<DateTime<Local>> {
    item.at.as_deref().and_then(local)
}

/// The time a row's label shows: `14:05`.
pub(crate) fn hhmm(t: DateTime<Local>) -> String {
    t.format("%H:%M").to_string()
}

/// The whole of it, for the time's tooltip: `Sun Nov 16 2026, 14:05:09`.
pub(crate) fn readout(t: DateTime<Local>) -> String {
    t.format("%a %b %-d %Y, %H:%M:%S").to_string()
}

/// A day line's date: `Sun Nov 16` (the TUI's `day_line`, without its rules).
pub(crate) fn day_label(day: NaiveDate) -> String {
    day.format("%a %b %-d").to_string()
}

/// A day as a number (a display row's `rev`).
pub(crate) fn day_number(day: NaiveDate) -> u64 {
    day.num_days_from_ce().max(0) as u64
}

/// A worker's journal entry recorded at `at_ms` on a clock that read
/// `now_ms` when the page was read, which was `wall_ms` (unix ms) here:
/// when it happened, RFC 3339 UTC (as a live row's `ts`).
pub(crate) fn journal(at_ms: u64, now_ms: u64, wall_ms: i64) -> Option<String> {
    let ago = i64::try_from(now_ms.saturating_sub(at_ms)).ok()?;
    let t = DateTime::<Utc>::from_timestamp_millis(wall_ms.checked_sub(ago)?)?;
    Some(t.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The local date of the last dated row, walking a transcript in order.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Days(Option<NaiveDate>);

impl Days {
    /// After `items` (the rows before a rebuild's start).
    pub(crate) fn after(items: &[Item]) -> Self {
        Days(items.iter().rev().find_map(|i| of(i).map(|t| t.date_naive())))
    }

    /// `item` would start a day: dated, on another date than the last dated row.
    pub(crate) fn turns(&self, item: &Item) -> bool {
        self.0.is_some_and(|last| of(item).is_some_and(|t| t.date_naive() != last))
    }

    /// `item` comes next: the date it starts, if it starts one.
    pub(crate) fn next(&mut self, item: &Item) -> Option<NaiveDate> {
        let day = of(item)?.date_naive();
        self.0.replace(day).filter(|last| *last != day).map(|_| day)
    }
}

/// An RFC 3339 UTC time for a local wall time (whatever this machine's
/// zone): tests that read local dates.
#[cfg(test)]
pub(crate) fn utc_of(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> String {
    use chrono::TimeZone as _;
    let t = Local.with_ymd_and_hms(y, mo, d, h, mi, s).single().expect("not in a zone change");
    t.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse_and_read_in_local_time() {
        let at = utc_of(2026, 11, 16, 14, 5, 9);
        let t = local(&at).unwrap();
        assert_eq!(hhmm(t), "14:05");
        assert_eq!(readout(t), "Mon Nov 16 2026, 14:05:09");
        assert_eq!(day_label(t.date_naive()), "Mon Nov 16");
        // 24-hour, both ways round midnight.
        assert_eq!(hhmm(local(&utc_of(2026, 11, 16, 23, 59, 59)).unwrap()), "23:59");
        assert_eq!(hhmm(local(&utc_of(2026, 11, 17, 0, 0, 0)).unwrap()), "00:00");
        // A live event's `ts` (nanoseconds, UTC) and the message time it
        // becomes (whole seconds): one minute, one text.
        let live = at.replace('Z', ".794878606Z");
        assert_eq!(hhmm(local(&live).unwrap()), "14:05");
        // An offset is honoured, not dropped.
        let fixed = DateTime::parse_from_rfc3339("2026-11-16T14:05:09+02:00").unwrap();
        assert_eq!(local("2026-11-16T14:05:09+02:00").unwrap(), fixed.with_timezone(&Local));
        for bad in ["", "now", "2026-11-16", "2026-11-16 14:05:09", "14:05"] {
            assert!(local(bad).is_none(), "{bad:?}");
        }
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
