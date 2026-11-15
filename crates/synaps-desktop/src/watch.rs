//! Watching a worker (a subagent) in its own view and steering it from
//! there, as the Synaps TUI does (`tui/watch.rs`, `tui/threads.rs`). No
//! GPUI: the view asks this what to show and when to poll.
//!
//! - **Switching.** Ctrl+↑ or Alt+↑ (Option+↑ on macOS, where Ctrl+↑ is
//!   Mission Control) from the session's own thread focuses the newest
//!   worker (the tray's bottom row); more ↑ go to older ones, ↓ back to
//!   newer ones, and past the newest to the session again. Esc goes back
//!   too, as does a click on MAIN in the view's breadcrumb; a click on a
//!   tray row opens that worker. The workers on offer are the running
//!   background ones plus any whose view is open.
//! - **Feeding.** While a worker's view is focused (and the window draws),
//!   its watch journal is paged with `SessionQuery::SubagentWatch`: one
//!   query in flight, every [`POLL_INTERVAL`], at once while a page says
//!   `more`. Each entry replays into the worker's own [`Transcript`]
//!   ([`Transcript::apply_watch`]), so a worker's rows are drawn by the
//!   same code as the session's and never touch its transcript or its
//!   scroll. Nothing polls otherwise: not the session's thread, not a
//!   finished worker whose journal has been read, not a background tab.
//! - **Steering.** Enter in a worker's view sends the text to that worker
//!   (`SessionCommand::SubagentSteer`, when this window may drive the
//!   session); the engine also notes it in the main conversation. A
//!   finished worker's view is read-only.
//! - **Bounds.** A worker's transcript keeps its newest [`WORKER_ITEMS`]
//!   rows and about [`WORKER_BYTES`]; a view nobody looks at is dropped
//!   once its worker is gone or [`FINISHED_TTL`] after it finished (the
//!   daemon keeps a finished worker's journal as long).
//! - **Older daemons.** One before v0.11.0 cannot decode the query: the
//!   view says so once ([`UNSUPPORTED`]) and nothing polls until the next
//!   reconnect.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use gpui_kit::SharedString;
use synaps_client::protocol::{SessionEvent, WatchEvent, WatchPage};
use synaps_client::{Transcript, WatchReply};

use crate::files::Ledger;
use crate::subagents::{Live, Subagents};

/// The poll's cadence while a worker's view is focused (the TUI's).
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// A poll with no reply after this is given up; the next asks from the
/// same cursor again, so nothing is lost.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// A poll that could not go out (reconnecting) is tried again after this.
pub const RETRY: Duration = Duration::from_secs(1);
/// After Esc leaves a worker's view, an Esc this soon does not also act
/// on the session's thread (its card).
pub const ESC_GUARD: Duration = Duration::from_millis(300);
/// A finished worker's view, while not focused, is dropped this long after
/// it finished (the daemon keeps its journal as long).
pub const FINISHED_TTL: Duration = Duration::from_secs(900);
/// A worker's transcript keeps this many rows and about this many bytes
/// (the TUI's scrollback for a socket client's transcript).
pub const WORKER_ITEMS: usize = 400;
pub const WORKER_BYTES: usize = 2 << 20;
/// A finished worker stays in the tray this long (the TUI's HUD flash).
pub const FLASH: Duration = Duration::from_secs(5);
/// Why a view gets nothing: no daemon (`--offline`).
const OFFLINE: &str = "offline";

/// What the view says to a daemon from before worker watch.
pub const UNSUPPORTED: &str = "this daemon can't show worker threads: it predates Synaps v0.11.0 (update it, then `synaps daemon reload`)";

/// A worker that can be watched: `sa_N`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub handle_id: String,
    pub subagent_id: u64,
    pub name: String,
}

impl Candidate {
    pub fn of(id: u64, name: &str) -> Self {
        Candidate { handle_id: format!("sa_{id}"), subagent_id: id, name: name.to_string() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Ctrl/Alt+↑: toward older workers (up the tray).
    Up,
    /// Ctrl/Alt+↓: toward newer ones, then the session.
    Down,
}

/// Where a [`Watch::step`] went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Moved {
    /// Into this worker's view.
    Worker(String),
    /// Back to the session's thread.
    Main,
    /// Up on the oldest worker: stayed.
    Oldest,
    /// Nowhere to go: no workers, or ↓ on the session's thread.
    No,
}

/// One worker's view: its transcript, fed by its journal.
#[derive(Debug)]
pub struct Worker {
    pub handle_id: String,
    pub subagent_id: u64,
    pub name: String,
    pub model: String,
    pub transcript: Transcript,
    /// What its rows read (the session's are another's): its file tools'
    /// diffs, and nothing of the session's workers.
    pub files: Ledger,
    pub subagents: Subagents,
    /// The `since` of the next page.
    pub next_seq: u64,
    /// From its last page (`running`, `completed`, …); empty before the first.
    pub status: String,
    pub steerable: bool,
    /// Finished, and everything it showed has been read: no more polls.
    pub drained: bool,
    /// The daemon no longer knows it (or its pages do not decode): why.
    pub gone: Option<String>,
    /// When this window first saw it finished.
    pub finished_at: Option<Instant>,
    /// Rows its transcript dropped ([`WORKER_ITEMS`], [`WORKER_BYTES`]),
    /// and whether the daemon's journal had dropped some before.
    pub dropped: usize,
    pub truncated: bool,
    /// What the breadcrumb and the composer say, made when it changes:
    /// `sleeper (sa_3)`; its model and the rows dropped; the hint.
    pub crumb: SharedString,
    pub facts: SharedString,
    pub hint: SharedString,
}

impl Worker {
    fn new(c: &Candidate, cwd: Option<&str>) -> Self {
        let mut transcript = crate::model::transcript();
        transcript.set_busy(true);
        let mut files = Ledger::default();
        files.reset(&[], cwd, false);
        let mut w = Worker {
            handle_id: c.handle_id.clone(),
            subagent_id: c.subagent_id,
            name: c.name.clone(),
            model: String::new(),
            transcript,
            files,
            subagents: Subagents::default(),
            next_seq: 0,
            status: String::new(),
            steerable: false,
            drained: false,
            gone: None,
            finished_at: None,
            dropped: 0,
            truncated: false,
            crumb: SharedString::default(),
            facts: SharedString::default(),
            hint: SharedString::default(),
        };
        w.strings();
        w
    }

    /// The breadcrumb's and the composer's strings, again.
    fn strings(&mut self) {
        self.crumb = format!("{} ({})", self.name, self.handle_id).into();
        let model = self.model.rsplit_once('/').map_or(self.model.as_str(), |(_, m)| m);
        let dropped = (self.dropped > 0).then(|| format!("{} earlier rows dropped", self.dropped));
        self.facts = [(!model.is_empty()).then(|| model.to_string()), dropped].into_iter().flatten().collect::<Vec<_>>().join(" · ").into();
        self.hint = if self.read_only() {
            format!("{} {}: read-only · Esc returns to the session", self.name, self.state()).into()
        } else if self.status.is_empty() {
            format!("{} is connecting…", self.name).into()
        } else {
            format!("steer {}: it takes it in at its next step · Esc returns to the session", self.name).into()
        };
    }

    /// What the breadcrumb says it is doing.
    pub fn state(&self) -> &str {
        match (&self.gone, self.status.as_str()) {
            (Some(g), "") if g == OFFLINE => "offline",
            (Some(_), _) => "gone",
            (None, "") => "connecting",
            (None, "timed_out") => "timed out",
            (None, s) => s,
        }
    }

    /// Its view takes no input: it finished (or is gone).
    pub fn read_only(&self) -> bool {
        self.gone.is_some() || (!self.status.is_empty() && self.status != "running")
    }

    fn notice(&mut self, text: String) {
        self.transcript.apply(&SessionEvent::SystemNotice { text });
    }

    /// One page into its transcript.
    fn apply(&mut self, page: WatchPage, now: Instant) {
        let was = std::mem::take(&mut self.status);
        if !page.agent_name.is_empty() {
            self.name = page.agent_name;
        }
        if !page.model.is_empty() {
            self.model = page.model;
        }
        if page.subagent_id != 0 {
            self.subagent_id = page.subagent_id;
        }
        if page.truncated && !self.truncated {
            self.truncated = true;
            self.notice("… earlier output was dropped (the daemon keeps a worker's last 2 MiB)".into());
        }
        for ev in &page.events {
            if let WatchEvent::ToolUse { tool_id, tool_name, input } = ev {
                self.files.tool_use(tool_name, tool_id, input);
            }
            self.transcript.apply_watch(ev);
        }
        self.next_seq = page.next_seq;
        self.steerable = page.steerable;
        let finished = page.status != "running";
        if finished && (was.is_empty() || was == "running") {
            self.transcript.set_busy(false);
            self.notice(format!("{} {}: this view is read-only", self.name, page.status.replace('_', " ")));
            self.finished_at = Some(now);
        }
        self.drained = finished && !page.more;
        self.status = page.status;
        self.dropped += self.transcript.trim_live_front(WORKER_ITEMS, WORKER_BYTES);
        if self.files.bytes() > WORKER_BYTES {
            // Its oldest rows are long gone: their diffs go too.
            self.files.reset(&[], None, false);
        }
        self.strings();
    }

    fn bytes(&self) -> usize {
        self.transcript
            .items()
            .iter()
            .map(|i| 256 + i.text.len() + i.tool.as_ref().map_or(0, |t| t.input.len() + t.result.len()))
            .sum::<usize>()
            + self.files.bytes()
    }
}

/// The poll in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    handle_id: String,
    since: u64,
}

/// A session's worker views and the watch bookkeeping (one per tab: it
/// goes to the tab cache with the transcript).
#[derive(Debug, Default)]
pub struct Watch {
    focus: Option<String>,
    workers: BTreeMap<String, Worker>,
    pending: Option<Pending>,
    /// When the next poll is due (`None`: nothing to poll).
    next_poll: Option<Instant>,
    /// An Esc before this does not act on the session's thread ([`ESC_GUARD`]).
    esc_guard: Option<Instant>,
    /// The last steer sent, until the engine notes it or refuses it.
    steer: Option<(String, String)>,
    /// The daemon cannot decode the query (before v0.11.0). Cleared on a
    /// reconnect.
    pub unsupported: bool,
    /// The tray's workers seen running here, and when each was first seen
    /// ended ([`FLASH`]).
    seen: HashMap<u64, Option<Instant>>,
    /// Bumped on every change the view shows (the breadcrumb, the
    /// composer's state, the focused transcript's rows aside).
    pub revision: u64,
}

impl Watch {
    /// The focused worker's handle id; `None`: the session's own thread.
    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    pub fn focused(&self) -> Option<&Worker> {
        self.workers.get(self.focus.as_deref()?)
    }

    pub fn focused_mut(&mut self) -> Option<&mut Worker> {
        self.workers.get_mut(self.focus.as_deref()?)
    }

    #[cfg(test)]
    pub fn worker(&self, handle_id: &str) -> Option<&Worker> {
        self.workers.get(handle_id)
    }

    /// What it keeps, roughly (bytes).
    pub fn bytes(&self) -> usize {
        self.workers.values().map(Worker::bytes).sum()
    }

    /// The workers on offer, oldest first: `running` (the tray's running
    /// background workers) and every open view the daemon still knows.
    pub fn candidates(&self, running: &[Candidate]) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = running.to_vec();
        for (id, w) in &self.workers {
            if w.gone.is_none() && !out.iter().any(|c| c.handle_id == *id) {
                out.push(Candidate { handle_id: id.clone(), subagent_id: w.subagent_id, name: w.name.clone() });
            }
        }
        out.sort_by_key(|c| c.subagent_id);
        out
    }

    /// Open (if need be) and focus a worker's view; its journal is asked
    /// for at once. `cwd`: the session's folder (its diffs' paths).
    pub fn open(&mut self, c: &Candidate, cwd: Option<&str>, now: Instant) {
        let w = self.workers.entry(c.handle_id.clone()).or_insert_with(|| {
            let mut w = Worker::new(c, cwd);
            w.notice(format!("{} ({}): Esc returns to the session · {} switches workers", c.name, c.handle_id, crate::app::WORKER_KEY));
            w
        });
        if self.unsupported && w.status.is_empty() {
            w.notice(UNSUPPORTED.into());
        }
        self.focus = Some(c.handle_id.clone());
        self.next_poll = (!self.unsupported).then_some(now);
        self.pending = None;
        self.revision += 1;
    }

    /// Back to the session's thread (its list is exactly as it was left).
    pub fn focus_main(&mut self) {
        if self.focus.take().is_some() {
            self.revision += 1;
        }
        self.next_poll = None;
        self.pending = None;
    }

    /// Esc in a worker's view: back, and an Esc right after is not the
    /// session's.
    pub fn leave_on_esc(&mut self, now: Instant) {
        self.focus_main();
        self.esc_guard = Some(now + ESC_GUARD);
    }

    pub fn esc_guarded(&self, now: Instant) -> bool {
        self.esc_guard.is_some_and(|t| now < t)
    }

    /// Ctrl/Alt+↑ / ↓ over `running` ([`Watch::candidates`]).
    pub fn step(&mut self, step: Step, running: &[Candidate], cwd: Option<&str>, now: Instant) -> Moved {
        let list = self.candidates(running);
        let at = self.focus.as_deref().and_then(|id| list.iter().position(|c| c.handle_id == id));
        let target = match (step, at) {
            (Step::Up, None) => list.len().checked_sub(1),
            (Step::Up, Some(0)) => return Moved::Oldest,
            (Step::Up, Some(i)) => Some(i - 1),
            (Step::Down, None) if self.focus.is_some() => {
                // Focused on one no longer on offer (gone): back.
                self.focus_main();
                return Moved::Main;
            }
            (Step::Down, None) => None,
            (Step::Down, Some(i)) if i + 1 < list.len() => Some(i + 1),
            (Step::Down, Some(_)) => {
                self.focus_main();
                return Moved::Main;
            }
        };
        match target {
            Some(i) => {
                self.open(&list[i], cwd, now);
                Moved::Worker(list[i].handle_id.clone())
            }
            None => Moved::No,
        }
    }

    /// Nothing to ask (no daemon: `--offline`): the focused view says so
    /// once, and nothing polls.
    pub fn no_daemon(&mut self, why: &str) {
        self.next_poll = None;
        self.pending = None;
        if let Some(w) = self.focused_mut()
            && w.gone.is_none()
        {
            w.notice(why.to_string());
            w.steerable = false;
            w.gone = Some(OFFLINE.into());
            w.strings();
        }
    }

    /// When the poll should fire (`None`: nothing to poll).
    pub fn poll_due(&self) -> Option<Instant> {
        self.focus.as_ref()?;
        self.next_poll
    }

    /// The query to send now, if one is due: `(handle id, since)`. Taken
    /// as sent; [`Watch::poll_failed`] if it could not go.
    pub fn take_poll(&mut self, now: Instant) -> Option<(String, u64)> {
        let due = self.poll_due()?;
        if now < due {
            return None;
        }
        let id = self.focus.clone()?;
        let w = self.workers.get(&id)?;
        if self.unsupported || w.gone.is_some() || w.drained {
            self.next_poll = None;
            return None;
        }
        // One unanswered past its timeout is given up (same cursor again).
        self.pending = Some(Pending { handle_id: id.clone(), since: w.next_seq });
        self.next_poll = Some(now + POLL_TIMEOUT);
        Some((id, w.next_seq))
    }

    /// The poll could not go out (the session is reconnecting): later.
    pub fn poll_failed(&mut self, now: Instant) {
        if self.pending.take().is_some() && self.focus.is_some() {
            self.next_poll = Some(now + RETRY);
        }
    }

    /// The daemon's reply to a poll. Only the one asked for counts (a late
    /// reply to an earlier ask is dropped: its page was asked again).
    /// Whether anything changed.
    pub fn on_reply(&mut self, handle_id: &str, since: u64, reply: WatchReply, now: Instant) -> bool {
        if self.pending.as_ref() != Some(&Pending { handle_id: handle_id.to_string(), since }) {
            return false;
        }
        self.pending = None;
        let focused = self.focus.as_deref() == Some(handle_id);
        if let WatchReply::Unsupported = reply {
            let first = !self.unsupported;
            self.unsupported = true;
            self.next_poll = None;
            if first && let Some(w) = self.workers.get_mut(handle_id) {
                w.notice(UNSUPPORTED.into());
            }
            self.revision += 1;
            return true;
        }
        let Some(w) = self.workers.get_mut(handle_id) else { return false };
        let next = match reply {
            WatchReply::Page(page) => {
                let more = page.more;
                w.apply(*page, now);
                if more {
                    Some(now)
                } else if w.drained {
                    None
                } else {
                    Some(now + POLL_INTERVAL)
                }
            }
            WatchReply::Gone(e) => {
                w.steerable = false;
                w.notice(format!("{e}: a finished worker stays watchable for 15 minutes"));
                w.gone = Some(e);
                w.transcript.set_busy(false);
                w.strings();
                None
            }
            WatchReply::Undecodable(e) => {
                w.steerable = false;
                w.notice(format!("this worker's journal did not decode ({e}): not asked again"));
                w.gone = Some(e);
                w.transcript.set_busy(false);
                w.strings();
                None
            }
            // Handled above; a variant from a newer client: ask again later.
            _ => Some(now + RETRY),
        };
        if focused {
            self.next_poll = next;
        }
        self.revision += 1;
        true
    }

    /// The session connection was made again (a reload, a restarted
    /// daemon, the tab came back): watch again from where it was.
    pub fn on_reconnect(&mut self, now: Instant) {
        self.unsupported = false;
        self.pending = None;
        self.next_poll = self.focus.is_some().then_some(now);
    }

    /// Its tab was left: nothing polls in the background.
    pub fn suspend(&mut self) {
        self.pending = None;
        self.next_poll = None;
        self.steer = None;
    }

    /// Drop the views nobody is looking at whose worker is gone, or that
    /// finished [`FINISHED_TTL`] ago.
    pub fn prune(&mut self, now: Instant) {
        let focus = self.focus.clone();
        self.workers.retain(|id, w| {
            focus.as_deref() == Some(id.as_str())
                || (w.gone.is_none() && w.finished_at.is_none_or(|t| now.saturating_duration_since(t) < FINISHED_TTL))
        });
        self.seen.retain(|_, ended| ended.is_none_or(|t| now.saturating_duration_since(t) < FINISHED_TTL));
    }

    // ── steering ───────────────────────────────────────────────────────

    /// Enter in a worker's view: the worker to steer, or why not.
    /// `None`: no worker's view is focused.
    pub fn steer_target(&self) -> Option<Result<String, String>> {
        let id = self.focus.as_deref()?;
        let w = self.workers.get(id)?;
        Some(if self.unsupported {
            Err(UNSUPPORTED.into())
        } else if w.steerable {
            Ok(id.to_string())
        } else if w.status.is_empty() && w.gone.is_none() {
            Err(format!("{} is still connecting: try again in a moment", w.name))
        } else {
            Err(format!("{} is not running: its view is read-only", w.name))
        })
    }

    /// A steer went out: the view says so; the worker shows it as a user
    /// row once it takes it in (at its next step).
    pub fn steer_sent(&mut self, handle_id: &str, text: &str) {
        self.steer = Some((handle_id.to_string(), text.to_string()));
        if let Some(w) = self.workers.get_mut(handle_id) {
            let name = w.name.clone();
            // Its text shows once the worker takes it in (a YOU row).
            w.notice(format!("→ steer sent: {name} takes it in at its next step"));
        }
        self.revision += 1;
    }

    /// The engine noted the steer of `handle_id` in the main conversation.
    pub fn steer_accepted(&mut self, handle_id: &str) {
        self.steer.take_if(|(h, _)| h == handle_id);
    }

    /// The daemon refused the last steer: the view says why. Returns its
    /// text to give back when that worker's view is still the focused one.
    pub fn steer_refused(&mut self, reason: &str) -> Option<String> {
        let (handle_id, text) = self.steer.take()?;
        if let Some(w) = self.workers.get_mut(&handle_id) {
            w.notice(format!("steer refused: {reason}"));
        }
        self.revision += 1;
        (self.focus.as_deref() == Some(handle_id.as_str())).then_some(text)
    }

    // ── the tray ───────────────────────────────────────────────────────

    /// The tray's workers: the running ones, a finished one for [`FLASH`]
    /// after this window first saw it end (if it had seen it running), and
    /// the one whose view is focused. Also when the next of them drops.
    pub fn tray(&mut self, live: Vec<Live>, now: Instant) -> (Vec<Live>, Option<Instant>) {
        let focus = self.focused().map(|w| w.subagent_id);
        let mut next: Option<Instant> = None;
        let mut out = Vec::new();
        for l in live {
            let shown = if l.status.running() {
                self.seen.insert(l.id, None);
                true
            } else if !l.status.terminal() {
                // Started: only the history says so; nothing runs that this
                // window knows of.
                false
            } else {
                match self.seen.get_mut(&l.id) {
                    Some(ended) => {
                        let at = *ended.get_or_insert(now);
                        let until = at + FLASH;
                        if now < until {
                            next = Some(next.map_or(until, |n| n.min(until)));
                        }
                        now < until
                    }
                    None => false,
                }
            };
            if shown || focus == Some(l.id) {
                out.push(l);
            }
        }
        (out, next)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use synaps_client::transcript::ItemKind;

    use super::*;

    fn page(v: serde_json::Value) -> WatchReply {
        WatchReply::Page(Box::new(serde_json::from_value(v).unwrap()))
    }

    fn running(next_seq: u64, more: bool, events: serde_json::Value) -> WatchReply {
        page(json!({"handle_id": "sa_2", "subagent_id": 2, "agent_name": "sleeper", "status": "running", "steerable": true,
            "next_seq": next_seq, "more": more, "events": events}))
    }

    fn texts(w: &Worker) -> Vec<(ItemKind, String)> {
        w.transcript.items().iter().map(|i| (i.kind, i.text.clone())).collect()
    }

    fn cands(ids: &[u64]) -> Vec<Candidate> {
        ids.iter().map(|&i| Candidate::of(i, &format!("w{i}"))).collect()
    }

    /// Paging: the first poll asks from 0 at once; each page moves the
    /// cursor; a page that says `more` is followed at once, else after
    /// [`POLL_INTERVAL`]; one query is in flight at a time, and a late
    /// reply to an earlier ask is dropped (its page was asked again).
    #[test]
    fn pages_move_the_cursor_one_query_at_a_time() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)));
        assert_eq!(w.take_poll(t0), None, "one in flight");
        assert!(w.on_reply("sa_2", 0, running(3, true, json!([{"kind": "text", "text": "Step 1."}])), t0));
        assert_eq!(w.poll_due(), Some(t0), "more: at once");
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 3)));
        assert!(w.on_reply("sa_2", 3, running(5, false, json!([{"kind": "text", "text": " Step 2."}])), t0));
        assert_eq!(w.poll_due(), Some(t0 + POLL_INTERVAL));
        assert_eq!(w.take_poll(t0 + Duration::from_millis(100)), None, "not yet");
        let t1 = t0 + POLL_INTERVAL;
        assert_eq!(w.take_poll(t1), Some(("sa_2".into(), 5)));
        // No reply: after the timeout the same cursor is asked again; the
        // first ask's late reply changes nothing.
        assert_eq!(w.take_poll(t1 + POLL_TIMEOUT), Some(("sa_2".into(), 5)));
        assert!(w.on_reply("sa_2", 5, running(6, false, json!([{"kind": "text", "text": " Step 3."}])), t1));
        assert!(!w.on_reply("sa_2", 5, running(6, false, json!([{"kind": "text", "text": " Step 3."}])), t1));
        let v = w.focused().unwrap();
        assert_eq!(texts(v)[1], (ItemKind::Assistant, "Step 1. Step 2. Step 3.".into()));
        assert_eq!(v.next_seq, 6);
        // Finished: read-only, and once its journal is read, no more polls.
        let done = page(json!({"handle_id": "sa_2", "status": "completed", "next_seq": 7, "events": [{"kind": "done"}]}));
        let t2 = t1 + POLL_TIMEOUT + POLL_INTERVAL;
        assert_eq!(w.take_poll(t2), Some(("sa_2".into(), 6)));
        assert!(w.on_reply("sa_2", 6, done, t2));
        let v = w.focused().unwrap();
        assert!(v.read_only() && v.drained && !v.transcript.busy());
        assert_eq!(texts(v).last().unwrap().1, "sleeper completed: this view is read-only");
        assert_eq!((w.poll_due(), w.take_poll(t2 + FINISHED_TTL)), (None, None));
        assert_eq!(w.steer_target(), Some(Err("sleeper is not running: its view is read-only".into())));
    }

    /// Nothing polls on the session's own thread or in a background tab;
    /// a reconnect (or the tab coming back) asks again from the cursor.
    #[test]
    fn only_a_focused_view_polls() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        assert_eq!(w.poll_due(), None);
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.focus_main();
        assert_eq!((w.poll_due(), w.take_poll(t0 + POLL_TIMEOUT)), (None, None));
        assert!(!w.on_reply("sa_2", 0, running(1, false, json!([])), t0), "asked before it left");
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        w.suspend();
        assert_eq!(w.poll_due(), None);
        w.on_reconnect(t0);
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)));
        // Reconnecting: the poll could not go; again a second later.
        w.poll_failed(t0);
        assert_eq!(w.poll_due(), Some(t0 + RETRY));
    }

    /// A worker's entries go to its own transcript only: the session's,
    /// and another worker's, never change.
    #[test]
    fn a_worker_never_touches_the_session_or_another_worker() {
        let t0 = Instant::now();
        let mut main = crate::model::transcript();
        main.apply_history(&[json!({"role": "user", "content": "start two workers"})]);
        let before: Vec<_> = main.items().iter().map(|i| (i.id, i.rev)).collect();
        let mut w = Watch::default();
        w.open(&Candidate::of(1, "a"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply(
            "sa_1",
            0,
            page(json!({"handle_id": "sa_1", "status": "running", "next_seq": 1, "events": [{"kind": "text", "text": "A"}]})),
            t0,
        );
        w.open(&Candidate::of(2, "b"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, page(json!({"handle_id": "sa_2", "status": "running", "next_seq": 4,
            "events": [{"kind": "tool_use", "tool_id": "t", "tool_name": "edit", "input": {"path": "/p/x.rs", "old_string": "a", "new_string": "b"}}]})), t0);
        assert_eq!(main.items().iter().map(|i| (i.id, i.rev)).collect::<Vec<_>>(), before);
        assert_eq!(texts(w.worker("sa_1").unwrap())[1], (ItemKind::Assistant, "A".into()));
        let b = w.worker("sa_2").unwrap();
        assert!(b.transcript.items().iter().all(|i| i.text != "A"));
        assert!(b.files.preview("t").is_some(), "its edit has a diff of its own");
        assert_eq!(w.focus(), Some("sa_2"));
    }

    /// Ctrl+↑ from the session goes to the newest worker, more ↑ to older
    /// ones (and stays on the oldest), ↓ back down and past the newest to
    /// the session; Esc goes back and guards a second Esc.
    #[test]
    fn the_keys_walk_the_workers_and_come_back() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        let run = cands(&[3, 1, 2]);
        assert_eq!(w.step(Step::Down, &run, None, t0), Moved::No);
        assert_eq!(w.step(Step::Up, &[], None, t0), Moved::No);
        assert_eq!(w.step(Step::Up, &run, None, t0), Moved::Worker("sa_3".into()));
        assert_eq!(w.step(Step::Up, &run, None, t0), Moved::Worker("sa_2".into()));
        assert_eq!(w.step(Step::Up, &run, None, t0), Moved::Worker("sa_1".into()));
        assert_eq!(w.step(Step::Up, &run, None, t0), Moved::Oldest);
        assert_eq!(w.focus(), Some("sa_1"));
        assert_eq!(w.step(Step::Down, &run, None, t0), Moved::Worker("sa_2".into()));
        assert_eq!(w.step(Step::Down, &run, None, t0), Moved::Worker("sa_3".into()));
        assert_eq!(w.step(Step::Down, &run, None, t0), Moved::Main);
        assert_eq!(w.focus(), None);
        // A view that is open stays on offer after its worker stopped running.
        assert_eq!(w.step(Step::Up, &cands(&[1]), None, t0), Moved::Worker("sa_3".into()));
        w.leave_on_esc(t0);
        assert_eq!(w.focus(), None);
        assert!(w.esc_guarded(t0 + Duration::from_millis(100)) && !w.esc_guarded(t0 + ESC_GUARD));
    }

    /// Steering only a running worker that takes it; a refusal says so in
    /// the view and gives the text back while that view is focused; the
    /// engine's note accepts it.
    #[test]
    fn steering_is_for_a_running_worker_and_a_refusal_comes_back() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        assert_eq!(w.steer_target(), None, "the session's thread steers the session");
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        assert_eq!(w.steer_target(), Some(Err("sleeper is still connecting: try again in a moment".into())));
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, running(1, false, json!([])), t0);
        assert_eq!(w.steer_target(), Some(Ok("sa_2".into())));
        w.steer_sent("sa_2", "stop after the next step");
        assert_eq!(w.steer_refused("input is owned by client #1").as_deref(), Some("stop after the next step"));
        let notes = texts(w.focused().unwrap());
        assert_eq!(notes[notes.len() - 2].1, "→ steer sent: sleeper takes it in at its next step");
        assert_eq!(notes.last().unwrap().1, "steer refused: input is owned by client #1");
        assert_eq!(w.steer_refused("again"), None, "nothing pending");
        // Accepted: nothing comes back; a refusal after the view left goes
        // to the view, not to the composer.
        w.steer_sent("sa_2", "x");
        w.steer_accepted("sa_2");
        assert_eq!(w.steer_refused("late"), None);
        w.steer_sent("sa_2", "y");
        w.focus_main();
        assert_eq!(w.steer_refused("nope"), None);
        // Not steerable (no steer channel): read-only too.
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 1, page(json!({"handle_id": "sa_2", "status": "running", "steerable": false, "next_seq": 1, "events": []})), t0);
        assert!(matches!(w.steer_target(), Some(Err(_))));
    }

    /// A daemon before v0.11.0: the view says so once, nothing polls and
    /// nothing steers until a reconnect; a worker it no longer knows is
    /// gone (and dropped once nobody looks at it).
    #[test]
    fn an_older_daemon_and_a_gone_worker() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        assert!(w.on_reply("sa_2", 0, WatchReply::Unsupported, t0));
        assert!(w.unsupported);
        assert_eq!((w.poll_due(), w.steer_target()), (None, Some(Err(UNSUPPORTED.into()))));
        w.open(&Candidate::of(3, "other"), None, t0);
        assert_eq!(w.poll_due(), None);
        let notes = |w: &Watch, id: &str| texts(w.worker(id).unwrap()).iter().filter(|(_, t)| t == UNSUPPORTED).count();
        assert_eq!((notes(&w, "sa_2"), notes(&w, "sa_3")), (1, 1));
        w.on_reconnect(t0);
        assert!(!w.unsupported);
        assert_eq!(w.take_poll(t0), Some(("sa_3".into(), 0)));
        assert!(w.on_reply("sa_3", 0, WatchReply::Gone("no subagent sa_3 in this session".into()), t0));
        assert_eq!((w.focused().unwrap().state(), w.poll_due()), ("gone", None));
        assert_eq!(w.candidates(&[]).iter().map(|c| c.handle_id.as_str()).collect::<Vec<_>>(), ["sa_2"]);
        w.focus_main();
        w.prune(t0);
        assert!(w.worker("sa_3").is_none() && w.worker("sa_2").is_some());
    }

    /// Without a daemon (`--offline`) a view says so once, is read-only
    /// ("offline", not "connecting" for ever) and asks for nothing.
    #[test]
    fn an_offline_view_says_so_and_asks_nothing() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(1, "scout"), None, t0);
        w.no_daemon("offline: no daemon");
        w.no_daemon("offline: no daemon");
        let v = w.focused().unwrap();
        assert_eq!((v.state(), v.read_only()), ("offline", true));
        assert_eq!(texts(v).iter().filter(|(_, t)| t == "offline: no daemon").count(), 1, "said once");
        assert_eq!(w.poll_due(), None);
        assert_eq!(w.take_poll(t0 + POLL_INTERVAL), None);
        assert!(matches!(w.steer_target(), Some(Err(e)) if e.contains("read-only")));
    }

    /// A finished worker's view goes [`FINISHED_TTL`] after it finished,
    /// unless it is the one on screen.
    #[test]
    fn finished_views_are_dropped_after_their_ttl() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply(
            "sa_2",
            0,
            page(json!({"handle_id": "sa_2", "status": "failed", "next_seq": 1, "events": [{"kind": "failed", "category": "overloaded"}]})),
            t0,
        );
        assert_eq!(w.focused().unwrap().state(), "failed");
        w.prune(t0 + FINISHED_TTL);
        assert!(w.worker("sa_2").is_some(), "on screen");
        w.focus_main();
        w.prune(t0 + FINISHED_TTL - Duration::from_secs(1));
        assert!(w.worker("sa_2").is_some());
        w.prune(t0 + FINISHED_TTL);
        assert!(w.worker("sa_2").is_none());
    }

    /// The tray: running workers; a finished one for [`FLASH`] after it
    /// was first seen ended, only if this window saw it running; one only
    /// the history knows of, never; the focused one always.
    #[test]
    fn the_tray_flashes_a_finished_worker_then_drops_it() {
        use crate::subagents::Status;
        let l = |id, status| Live { id, name: format!("w{id}"), status, line: String::new(), tools: None, elapsed: None, watchable: true };
        let t0 = Instant::now();
        let mut w = Watch::default();
        let ids = |v: &(Vec<Live>, Option<Instant>)| v.0.iter().map(|l| l.id).collect::<Vec<_>>();
        let r = w.tray(vec![l(1, Status::Running), l(2, Status::Done), l(3, Status::Started)], t0);
        assert_eq!((ids(&r), r.1), (vec![1], None));
        let t1 = t0 + Duration::from_secs(2);
        let r = w.tray(vec![l(1, Status::Done), l(2, Status::Done)], t1);
        assert_eq!((ids(&r), r.1), (vec![1], Some(t1 + FLASH)));
        assert_eq!(ids(&w.tray(vec![l(1, Status::Done)], t1 + FLASH - Duration::from_millis(1))), vec![1]);
        assert_eq!(w.tray(vec![l(1, Status::Done)], t1 + FLASH), (vec![], None));
        w.open(&Candidate::of(2, "w2"), None, t1);
        assert_eq!(ids(&w.tray(vec![l(1, Status::Done), l(2, Status::Done)], t1 + FLASH)), vec![2], "its view is on screen");
    }

    /// A worker's transcript keeps its newest rows.
    #[test]
    fn a_long_worker_keeps_its_newest_rows() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let events: Vec<_> = (0..WORKER_ITEMS + 50)
            .map(|i| json!({"kind": "tool_use", "tool_id": format!("t{i}"), "tool_name": "bash", "input": {"command": "ls"}}))
            .collect();
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, running(1, false, json!(events)), t0);
        let v = w.focused().unwrap();
        assert_eq!(v.transcript.len(), WORKER_ITEMS);
        assert_eq!(v.dropped, 51, "the opening notice and the 50 oldest calls");
    }
}
