//! The client outbox (agent-ui §3.8, v3.4 #7): one committed action, from
//! its first send until the daemon has it or it becomes a recovered draft,
//! as pure functions of what arrives. Each takes the entry and an input
//! (a `surface_result`, a card state, a new connection's snapshot, the
//! clock, the user's Retry) and returns what to do: [`Next`]. Nothing here
//! sends, sleeps or renders; D3 holds the entries, sends with
//! [`crate::Client::surface_action_on`], and keeps one timer at
//! [`Entry::due`] (idle costs nothing).
//!
//! The rules, each one a row of §3.8's table:
//! - `accepted` / `duplicate`, or a state whose `actions` (or answer)
//!   holds our id: done.
//! - `pending`: keep, "saving…" after 2 s; never re-sent on this
//!   connection, only after a reattach.
//! - `reloading`, `rate_limited`, `busy`: re-send the same id after 1, 2,
//!   4 … 30 s, while connected.
//! - `storage`, `unavailable` (with its `detail`): "Not sent: … · Retry";
//!   Retry re-sends the same id.
//! - `conflict`, `resolved`, `stale`, `stopped`, `gone`: the values become
//!   a recovered draft (§6.1.6), the entry goes.
//! - No reply in 5 s: re-send the same id; after 3 sends on one connection
//!   with no reply, "Not sent · Retry" (v3.4 #7: a connection `Error`
//!   carries no action id, and this cap ends the loop).
//! - Reattach: the snapshot holds our id → done; the card still takes this
//!   action unchanged (offered, same contract, `answer.rev == expect_rev`,
//!   same epoch) → re-sent at once, with no user action (F16); otherwise →
//!   a recovered draft (F16b when the epoch moved).
//!
//! A committed value is never dropped by an event (§6.1.6): every way out
//! is [`Next::Done`] (the daemon has it) or [`Next::Recover`] (the values
//! go to a draft). Three cases §3.8 leaves open take the conservative
//! reading: `forbidden` and `invalid` (retry false: the same id would get
//! the same answer) become recovered drafts; a reason this build doesn't
//! know waits for the user ("Not sent · Retry"); an outcome it doesn't
//! know is as if no reply came.
//!
//! Which client a `surface_result` is for (`client`) is the caller's check,
//! as with `Refused`; an entry only checks the `action_id`.

use std::time::{Duration, Instant};

use super::types::SurfaceState;
use crate::protocol::{SurfaceAction, SurfaceOutcome, SurfaceRefusal, SurfaceResult, SurfaceUnavailable};

/// No reply in this long: re-send the same id (§3.8).
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Sends with no reply on one connection before "Not sent · Retry"
/// (v3.4 #7). The first send counts.
pub const SENDS_MAX: u8 = 3;
/// A retryable refusal's first backoff, doubled each time up to
/// [`BACKOFF_MAX`] (§3.8: 1, 2, 4 … 30 s).
pub const BACKOFF_FIRST: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// `pending` shows "saving…" from this long after it came (§3.8).
pub const SAVING_AFTER: Duration = Duration::from_secs(2);

/// Where one committed action is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Sent on `connection`, `sends` times with no reply, the last at `at`.
    /// `backoff`: the last retry delay on this connection, which the next
    /// retryable refusal doubles.
    Sent { connection: u64, sends: u8, at: Instant, backoff: Option<Duration> },
    /// The daemon answered `pending` at `since` (§3.4): it holds the
    /// action, its outcome isn't known yet, and a final result follows.
    Pending { connection: u64, since: Instant },
    /// Refused for now (`reason`: reloading, rate limited, busy): re-sent
    /// on `connection` at `due`, `delay` after the refusal.
    Backoff { connection: u64, due: Instant, delay: Duration, reason: SurfaceRefusal },
    /// Not sent: the card shows "Not sent: … · Retry". Re-sent by the
    /// user's Retry, or by a reattach whose snapshot allows it.
    Held(NotSent),
    /// Its connection is gone: nothing is sent until a new connection's
    /// snapshot reconciles it ([`Entry::on_snapshot`]).
    Detached,
}

/// Why an action waits for the user's Retry: D3's "Not sent: <reason>".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSent {
    /// `storage`: the daemon's store failed.
    Storage,
    /// `unavailable`, with its `detail` (v3.4 #2; `None` from a daemon that
    /// sent none: "Not sent · Retry").
    Unavailable(Option<SurfaceUnavailable>),
    /// [`SENDS_MAX`] sends on one connection, no reply (v3.4 #7).
    Unanswered,
    /// A refusal reason this build doesn't know.
    Unknown,
}

/// What to do with an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Keep it, in this step; send nothing.
    Stay(Step),
    /// Send the action now, unchanged, on the step's connection; the entry
    /// is then in this step (always [`Step::Sent`]).
    Send(Step),
    /// The daemon has it: drop the entry.
    Done,
    /// It can't be sent as it is: move its values to a recovered draft
    /// (§6.1.6) and drop the entry. The reason is the refusal, or at a
    /// reattach the one admission would give a re-send (§3.3 #6–#10):
    /// `gone`, `resolved`, `stale`, `stopped` (F16b), `conflict`; or a
    /// refusal's `forbidden` / `invalid`.
    Recover(SurfaceRefusal),
}

/// One committed action and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub action: SurfaceAction,
    pub step: Step,
}

impl Entry {
    /// The user committed `action` on a card drawn from `connection`'s
    /// snapshot or events: the caller sends it once, now.
    pub fn commit(action: SurfaceAction, connection: u64, now: Instant) -> Self {
        Entry { action, step: Step::Sent { connection, sends: 1, at: now, backoff: None } }
    }

    /// This entry with `next` applied: `None` when it leaves the outbox
    /// (the caller has already acted on `Done` / `Recover`).
    pub fn apply(mut self, next: Next) -> Option<Self> {
        match next {
            Next::Stay(step) | Next::Send(step) => {
                self.step = step;
                Some(self)
            }
            Next::Done | Next::Recover(_) => None,
        }
    }

    /// The connection a send in this step goes on, if it has one.
    pub fn connection(&self) -> Option<u64> {
        match self.step {
            Step::Sent { connection, .. } | Step::Pending { connection, .. } | Step::Backoff { connection, .. } => Some(connection),
            Step::Held(_) | Step::Detached => None,
        }
    }

    /// A `surface_result` (§2.3). Another action's is nothing to this one.
    pub fn on_result(&self, r: &SurfaceResult, now: Instant) -> Next {
        if r.action_id != self.action.action_id {
            return Next::Stay(self.step);
        }
        match r.outcome {
            SurfaceOutcome::Accepted | SurfaceOutcome::Duplicate => Next::Done,
            SurfaceOutcome::Pending => match self.connection() {
                Some(connection) => Next::Stay(Step::Pending { connection, since: now }),
                // Held or detached: a reattach or the user decides.
                None => Next::Stay(self.step),
            },
            SurfaceOutcome::Refused => self.refused(r, now),
            // A newer daemon's outcome: as if no reply came.
            _ => Next::Stay(self.step),
        }
    }

    fn refused(&self, r: &SurfaceResult, now: Instant) -> Next {
        let reason = r.reason.unwrap_or(SurfaceRefusal::Unknown);
        match reason {
            SurfaceRefusal::Reloading | SurfaceRefusal::RateLimited | SurfaceRefusal::Busy => {
                let last = match self.step {
                    Step::Sent { backoff, .. } => backoff,
                    Step::Backoff { delay, .. } => Some(delay),
                    _ => None,
                };
                match self.connection() {
                    Some(connection) => {
                        let delay = last.map_or(BACKOFF_FIRST, |d| (d * 2).min(BACKOFF_MAX));
                        Next::Stay(Step::Backoff { connection, due: now + delay, delay, reason })
                    }
                    None => Next::Stay(self.step),
                }
            }
            SurfaceRefusal::Storage => Next::Stay(Step::Held(NotSent::Storage)),
            SurfaceRefusal::Unavailable => Next::Stay(Step::Held(NotSent::Unavailable(r.detail))),
            SurfaceRefusal::Conflict
            | SurfaceRefusal::Resolved
            | SurfaceRefusal::Stale
            | SurfaceRefusal::Stopped
            | SurfaceRefusal::Gone
            | SurfaceRefusal::Forbidden
            | SurfaceRefusal::Invalid => Next::Recover(reason),
            _ => Next::Stay(Step::Held(NotSent::Unknown)),
        }
    }

    /// Whether `card` shows this action accepted: its `actions` hold our
    /// id (§3.7: any later state, not only while it is the latest), or its
    /// answer is ours.
    pub fn shown_in(&self, card: &SurfaceState) -> bool {
        let id = &self.action.action_id;
        card.id == self.action.surface_id && (card.actions.contains(id) || card.answer.as_ref().is_some_and(|a| a.action_id == *id))
    }

    /// A card state (a `surface` upsert): done if it shows this action;
    /// any other change is the daemon's to judge when the action arrives.
    pub fn on_state(&self, card: &SurfaceState) -> Next {
        if self.shown_in(card) { Next::Done } else { Next::Stay(self.step) }
    }

    /// The connection is gone. Nothing is sent until a new one's snapshot
    /// reconciles the entry; a held one keeps its "Not sent · Retry".
    pub fn on_disconnect(&self) -> Next {
        match self.step {
            Step::Held(_) => Next::Stay(self.step),
            _ => Next::Stay(Step::Detached),
        }
    }

    /// A new connection's `Attached.surfaces` (§3.8 Reattach): `card` is
    /// this action's card in it, if it is there. Every entry goes through
    /// here, whatever its step: a re-send happens only on the connection
    /// it was reconciled against.
    pub fn on_snapshot(&self, card: Option<&SurfaceState>, connection: u64, now: Instant) -> Next {
        let Some(card) = card else { return Next::Recover(SurfaceRefusal::Gone) };
        if self.shown_in(card) {
            return Next::Done;
        }
        let a = &self.action;
        let rev = card.answer.as_ref().map_or(0, |s| s.rev);
        // In admission's order (§3.3 #7–#10): what a re-send would get.
        let refusal = if !card.offers.contains(&a.verb) {
            Some(SurfaceRefusal::Resolved)
        } else if card.contract != a.contract {
            Some(SurfaceRefusal::Stale)
        } else if card.epoch != a.epoch {
            Some(SurfaceRefusal::Stopped)
        } else if rev != a.expect_rev {
            Some(SurfaceRefusal::Conflict)
        } else {
            None
        };
        match refusal {
            Some(why) => Next::Recover(why),
            None => Next::Send(Step::Sent { connection, sends: 1, at: now, backoff: None }),
        }
    }

    /// The clock at `now`: a reply overdue (re-send, or held after
    /// [`SENDS_MAX`]), or a backoff over (re-send).
    pub fn tick(&self, now: Instant) -> Next {
        match self.step {
            Step::Sent { connection, sends, at, backoff } if now >= at + REPLY_TIMEOUT => {
                if sends < SENDS_MAX {
                    Next::Send(Step::Sent { connection, sends: sends + 1, at: now, backoff })
                } else {
                    Next::Stay(Step::Held(NotSent::Unanswered))
                }
            }
            Step::Backoff { connection, due, delay, .. } if now >= due => {
                Next::Send(Step::Sent { connection, sends: 1, at: now, backoff: Some(delay) })
            }
            step => Next::Stay(step),
        }
    }

    /// The user's Retry on a held action: the same id, on `connection` (the
    /// one the card is drawn from now). Anything else has nothing to retry.
    pub fn retry(&self, connection: u64, now: Instant) -> Next {
        match self.step {
            Step::Held(_) => Next::Send(Step::Sent { connection, sends: 1, at: now, backoff: None }),
            step => Next::Stay(step),
        }
    }

    /// Whether the card shows "saving…" (§3.8: `pending` for 2 s).
    pub fn saving(&self, now: Instant) -> bool {
        matches!(self.step, Step::Pending { since, .. } if now >= since + SAVING_AFTER)
    }

    /// When to [`Entry::tick`] next, for one timer (an instant already
    /// past: tick now). A pending action's is when "saving…" shows, until
    /// it does. `None`: nothing changes until an event or the user.
    pub fn due(&self, now: Instant) -> Option<Instant> {
        match self.step {
            Step::Sent { at, .. } => Some(at + REPLY_TIMEOUT),
            Step::Backoff { due, .. } => Some(due),
            Step::Pending { since, .. } => Some(since + SAVING_AFTER).filter(|t| *t > now),
            Step::Held(_) | Step::Detached => None,
        }
    }
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
