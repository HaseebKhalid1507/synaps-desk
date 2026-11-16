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
//!   finished worker whose journal has been read, not a background tab,
//!   not a hidden window. A page that changes nothing draws nothing.
//! - **Steering.** Enter in a worker's view sends the text to that worker
//!   (`SessionCommand::SubagentSteer`, when this window may drive the
//!   session); the engine also notes it in the main conversation. A
//!   finished worker's view is read-only. A refused steer's text comes
//!   back into the composer once its view is on screen again (the
//!   session's, with a note, if its view has gone).
//! - **Bounds.** A worker's transcript keeps its newest [`WORKER_ITEMS`]
//!   rows and about [`WORKER_BYTES`], trimmed in batches and only while
//!   its list follows the tail (scrolled back, twice as many); a view
//!   nobody looks at is dropped once its worker no longer runs.
//! - **Reused ids.** A cold-started daemon numbers its workers from 1
//!   again: a reconnect drops the views nobody looks at and the focused
//!   one takes no steer until a page says so (from 0 if the daemon is
//!   another process); a page whose cursor goes back, or that names
//!   another worker, starts the view over from 0.
//! - **Worker tabs.** A worker can have a tab of its own
//!   ([`crate::tabs::Kind::Worker`]): the same view, opened with
//!   [`Watch::open_tab`]. Esc does not leave it and the worker keys do not
//!   walk from it (it is that worker's tab); closing it never touches the
//!   worker. In the background it polls nothing, as any tab left.
//! - **Older daemons.** One before v0.11.0 cannot decode the query: the
//!   view says so once ([`UNSUPPORTED`]) and nothing polls until the next
//!   reconnect.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
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
/// How long a worker seen ended is remembered for the tray (the daemon
/// keeps a finished worker's journal as long).
pub const FINISHED_TTL: Duration = Duration::from_secs(900);
/// A worker's transcript keeps this many rows and about this many bytes
/// (the TUI's scrollback for a socket client's transcript): past either,
/// its oldest go, down to three quarters of both (the list drops the rows
/// at its top once a batch, not one a page).
pub const WORKER_ITEMS: usize = 400;
pub const WORKER_BYTES: usize = 2 << 20;
/// Scrolled back, a worker's transcript is trimmed only past this many
/// times the bounds (the rows being read stay put).
pub const SCROLLED_BACK: usize = 2;
/// Steers kept until the engine notes or the daemon refuses each.
const STEERS_MAX: usize = 8;
/// Every view's generation, unique: a view started over is another list.
static GENERATION: AtomicU64 = AtomicU64::new(1);
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
    /// The tool id of the call that started it, once that call's own
    /// result named it (who it is: [`crate::subagents::Subagents::ident`]).
    pub call: Option<String>,
}

impl Candidate {
    pub fn of(id: u64, name: &str) -> Self {
        Candidate { handle_id: format!("sa_{id}"), subagent_id: id, name: name.to_string(), call: None }
    }

    pub fn with_call(mut self, call: Option<&str>) -> Self {
        self.call = call.map(str::to_string);
        self
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
    /// Unique to this view: one started over (a reused handle) has another.
    pub generation: u64,
    /// The session's folder (its diffs' paths).
    cwd: Option<String>,
    pub name: String,
    /// Its call's tool id, if known when it opened ([`Candidate::call`]).
    call: Option<String>,
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
    /// What its breadcrumb says while its session waits on a prompt.
    pub needs: SharedString,
    /// What the right panel's FILES says over this view (they are the
    /// session's thread's, not this worker's).
    pub files_note: SharedString,
    /// A refused steer's text and the daemon's reason, for the composer
    /// once this view is on screen again.
    unsent: Option<(String, String)>,
    /// Shown in a worker tab (not in place over the session's thread):
    /// what its hints say.
    pub tabbed: bool,
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
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            cwd: cwd.map(str::to_string),
            name: c.name.clone(),
            call: c.call.clone(),
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
            needs: SharedString::default(),
            files_note: SharedString::default(),
            unsent: None,
            tabbed: false,
        };
        w.strings();
        w
    }

    /// This view started over: the same handle, from 0, saying why.
    fn restart(&mut self, why: String) {
        let c = Candidate {
            handle_id: self.handle_id.clone(),
            subagent_id: self.subagent_id,
            name: self.name.clone(),
            call: self.call.clone(),
        };
        let (unsent, tabbed) = (self.unsent.take(), self.tabbed);
        *self = Worker::new(&c, self.cwd.as_deref());
        self.unsent = unsent;
        self.tabbed = tabbed;
        self.notice(why);
    }

    /// The breadcrumb's and the composer's strings, again.
    fn strings(&mut self) {
        self.crumb = format!("{} ({})", self.name, self.handle_id).into();
        self.needs = format!("{}'s session needs you → open", self.name).into();
        self.files_note = format!("the session's thread, not {}'s: tool inputs, not the disk", self.name).into();
        let model = self.model.rsplit_once('/').map_or(self.model.as_str(), |(_, m)| m);
        let dropped = (self.dropped > 0).then(|| format!("{} earlier rows dropped", self.dropped));
        self.facts = [(!model.is_empty()).then(|| model.to_string()), dropped].into_iter().flatten().collect::<Vec<_>>().join(" · ").into();
        // A worker tab's notice says what closing it does: not the hint.
        let back = if self.tabbed { "" } else { " · Esc returns to the session" };
        self.hint = if self.read_only() {
            format!("{} {}: read-only{back}", self.name, self.state()).into()
        } else if self.status.is_empty() {
            format!("{} is connecting…", self.name).into()
        } else if !self.steerable {
            format!("{} can't be steered right now{back}", self.name).into()
        } else {
            format!("steer {}: it takes it in at its next step{back}", self.name).into()
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

    /// The page is another worker's (the handle was reused: a cold-started
    /// daemon numbers its workers from 1 again): its cursor went back (the
    /// daemon clamps a cursor past a journal's end to that end), or it
    /// names another worker. A worker tab's view is of one worker: even
    /// its first page must name it ([`Worker::other`]).
    fn reused(&self, page: &WatchPage) -> bool {
        let named = !page.agent_name.is_empty() && page.agent_name != self.name;
        (self.tabbed && named) || (!self.status.is_empty() && (page.next_seq < self.next_seq || named))
    }

    /// The handle names another worker now: this view is gone (read-only,
    /// no polls, no steer), saying why; whatever it showed stays.
    fn other(&mut self, why: String) {
        self.steerable = false;
        self.notice(why.clone());
        self.gone = Some(why);
        self.transcript.set_busy(false);
        self.strings();
    }

    /// Whether this view is of worker `c` (its call if both know it, else
    /// its name), not an earlier one under the same handle.
    fn is(&self, c: &Candidate) -> bool {
        match (self.call.as_deref(), c.call.as_deref()) {
            (Some(a), Some(b)) => a == b,
            _ => self.name == c.name,
        }
    }

    /// One page into its transcript. `scrolled_back`: its list does not
    /// follow the tail ([`SCROLLED_BACK`]). Whether anything it shows
    /// changed.
    fn apply(&mut self, page: WatchPage, now: Instant, scrolled_back: bool) -> bool {
        // What the breadcrumb and the composer say ([`Worker::strings`]).
        let mut said = false;
        let mut changed = !page.events.is_empty();
        if !page.agent_name.is_empty() && page.agent_name != self.name {
            self.name = page.agent_name;
            said = true;
        }
        if !page.model.is_empty() && page.model != self.model {
            self.model = page.model;
            said = true;
        }
        if page.subagent_id != 0 {
            self.subagent_id = page.subagent_id;
        }
        if page.truncated && !self.truncated {
            self.truncated = true;
            self.notice("… earlier output was dropped (the daemon keeps a worker's last 2 MiB)".into());
            changed = true;
        }
        for ev in &page.events {
            if let WatchEvent::ToolUse { tool_id, tool_name, input } = ev {
                self.files.tool_use(tool_name, tool_id, input);
            }
            self.transcript.apply_watch(ev);
        }
        self.next_seq = page.next_seq;
        said |= self.steerable != page.steerable;
        self.steerable = page.steerable;
        let finished = page.status != "running";
        if finished && (self.status.is_empty() || self.status == "running") {
            self.transcript.set_busy(false);
            self.notice(format!("{} {}: this view is read-only", self.name, page.status.replace('_', " ")));
            self.finished_at = Some(now);
            changed = true;
        }
        self.drained = finished && !page.more;
        if page.status != self.status {
            self.status = page.status;
            said = true;
        }
        if changed {
            let k = if scrolled_back { SCROLLED_BACK } else { 1 };
            if self.transcript.len() > k * WORKER_ITEMS || live_bytes(&self.transcript) > k * WORKER_BYTES {
                let n = self.transcript.trim_live_front(WORKER_ITEMS * 3 / 4, WORKER_BYTES * 3 / 4);
                log::debug!(target: "synaps_desktop::watch", "{} trimmed {n} rows ({} left, scrolled back: {scrolled_back})", self.handle_id, self.transcript.len());
                self.dropped += n;
                said |= n > 0;
            }
            if self.files.bytes() > WORKER_BYTES {
                // Its oldest rows are long gone: their diffs go too.
                self.files.reset(&[], self.cwd.as_deref(), false);
            }
        }
        if said {
            self.strings();
        }
        changed || said
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

/// What [`Transcript::trim_live_front`] counts of a transcript.
fn live_bytes(t: &Transcript) -> usize {
    t.items()
        .iter()
        .map(|i| 64 + i.text.len() + i.tool.as_ref().map_or(0, |t| t.tool_id.len() + t.name.len() + t.input.len() + t.result.len()))
        .sum()
}

/// The handle a refusal's reason names (`… sa_3 …`, `nasa_scout (sa_3)
/// is not running`), if any: a whole word, never inside a worker's name.
fn named_handle(reason: &str) -> Option<String> {
    reason.match_indices("sa_").find_map(|(at, _)| {
        if reason[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let n: String = reason[at + 3..].chars().take_while(char::is_ascii_digit).collect();
        (!n.is_empty()).then(|| format!("sa_{n}"))
    })
}

/// A steer sent: to `handle_id` (the worker `name`), `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Steer {
    handle_id: String,
    name: String,
    text: String,
}

/// What the session's composer says of a refused steer's text given back
/// to it (its view has gone).
fn refused_note(name: &str, handle_id: &str, reason: &str) -> String {
    format!("your steer to {name} ({handle_id}) was refused ({reason}): Enter now sends it to the session")
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
    /// Steers sent, oldest first, until the engine notes each or the
    /// daemon refuses it ([`STEERS_MAX`]).
    steers: Vec<Steer>,
    /// A refused steer whose view has gone: `(what the composer says of
    /// it, text)`, for the session's composer once its thread is on screen.
    unsent: Option<(String, String)>,
    /// The daemon cannot decode the query (before v0.11.0). Cleared on a
    /// reconnect.
    pub unsupported: bool,
    /// The tray's workers seen running here, and when each was first seen
    /// ended ([`FLASH`]).
    seen: HashMap<u64, Option<Instant>>,
    /// The focused view's list does not follow its tail (the user scrolled
    /// back): set by the view each frame ([`SCROLLED_BACK`]).
    pub scrolled_back: bool,
    /// The daemon's process the views were read from: another (a restart)
    /// starts the focused view over at the next attach.
    daemon: Option<u32>,
    /// The handles the session's open worker tabs show ([`Watch::hold`]):
    /// their views stay while the tab does, finished or not.
    held: Vec<String>,
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

    /// What the view's list is built from: the focused view and its
    /// generation (one started over is a list of its own).
    pub fn focus_key(&self) -> Option<(&str, u64)> {
        self.focused().map(|w| (w.handle_id.as_str(), w.generation))
    }

    #[cfg(test)]
    pub fn worker(&self, handle_id: &str) -> Option<&Worker> {
        self.workers.get(handle_id)
    }

    /// What it keeps, roughly (bytes).
    pub fn bytes(&self) -> usize {
        self.workers.values().map(Worker::bytes).sum()
    }

    /// Worker `handle_id`'s view's word ([`Worker::state`]), if it has one.
    pub fn view_state(&self, handle_id: &str) -> Option<&str> {
        self.workers.get(handle_id).map(Worker::state)
    }

    /// The workers on offer, oldest first: `running` (the tray's running
    /// background workers) and the focused view's, whatever its state (a
    /// walk from it goes to its neighbours).
    pub fn candidates(&self, running: &[Candidate]) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = running.to_vec();
        if let Some(w) = self.focused()
            && !out.iter().any(|c| c.handle_id == w.handle_id)
        {
            out.push(Candidate { handle_id: w.handle_id.clone(), subagent_id: w.subagent_id, name: w.name.clone(), call: w.call.clone() });
        }
        out.sort_by_key(|c| c.subagent_id);
        out
    }

    /// Open (if need be) and focus a worker's view in place, over the
    /// session's thread; its journal is asked for at once. `cwd`: the
    /// session's folder (its diffs' paths).
    pub fn open(&mut self, c: &Candidate, cwd: Option<&str>, now: Instant) {
        self.open_as(c, cwd, now, false);
    }

    /// The same, as a worker tab's view.
    pub fn open_tab(&mut self, c: &Candidate, cwd: Option<&str>, now: Instant) {
        self.open_as(c, cwd, now, true);
    }

    /// Worker `handle_id`'s view's generation, if it has one.
    pub fn generation_of(&self, handle_id: &str) -> Option<u64> {
        self.workers.get(handle_id).map(|w| w.generation)
    }

    /// The focused view is of worker `c` ([`Worker::is`]).
    pub fn focused_is(&self, c: &Candidate) -> bool {
        self.focused().is_some_and(|w| w.is(c))
    }

    /// The focused view is a worker tab's.
    pub fn in_tab(&self) -> bool {
        self.focused().is_some_and(|w| w.tabbed)
    }

    fn open_as(&mut self, c: &Candidate, cwd: Option<&str>, now: Instant, tab: bool) {
        // A view under this handle of an earlier worker (a cold-started
        // daemon reuses `sa_N`) is not this one's: it goes.
        if self.workers.get(&c.handle_id).is_some_and(|w| !w.is(c)) {
            self.workers.remove(&c.handle_id);
        }
        let w = self.workers.entry(c.handle_id.clone()).or_insert_with(|| {
            let mut w = Worker::new(c, cwd);
            w.notice(if tab {
                format!("{} ({}) in its own tab: closing the tab leaves the worker running", c.name, c.handle_id)
            } else {
                format!(
                    "{} ({}): Esc returns to the session · {} switches workers · {} opens it in a tab",
                    c.name,
                    c.handle_id,
                    crate::app::WORKER_KEY,
                    crate::wtabs::TAB_KEY
                )
            });
            w
        });
        if w.tabbed != tab {
            w.tabbed = tab;
            w.strings();
        }
        if self.unsupported && w.status.is_empty() {
            w.notice(UNSUPPORTED.into());
        }
        self.focus = Some(c.handle_id.clone());
        self.next_poll = (!self.unsupported).then_some(now);
        self.pending = None;
        self.scrolled_back = false;
    }

    /// Back to the session's thread (its list is exactly as it was left).
    pub fn focus_main(&mut self) {
        self.focus = None;
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

    /// The session's open worker tabs show `handles`: their views stay
    /// (no page read twice, the scroll kept, a refused steer's text with
    /// its tab) until the tab closes.
    pub fn hold(&mut self, handles: Vec<String>) {
        self.held = handles;
    }

    fn kept(&self, id: &str) -> bool {
        self.focus.as_deref() == Some(id) || self.held.iter().any(|h| h == id)
    }

    /// Worker `handle_id`'s handle names another worker now (its tab's
    /// worker is not the one the session's workers say): its view is
    /// gone, saying why. Whether that changed it.
    pub fn disown(&mut self, handle_id: &str, why: impl FnOnce() -> String) -> bool {
        let Some(w) = self.workers.get_mut(handle_id).filter(|w| w.gone.is_none()) else { return false };
        w.other(why());
        if self.focus.as_deref() == Some(handle_id) {
            self.next_poll = None;
            self.pending = None;
        }
        true
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
            return true;
        }
        let scrolled_back = self.scrolled_back;
        let Some(w) = self.workers.get_mut(handle_id) else { return false };
        let (next, changed) = match reply {
            WatchReply::Page(page) if w.reused(&page) && w.tabbed => {
                let who = if page.agent_name.is_empty() { String::new() } else { format!(" ({})", page.agent_name) };
                w.other(format!("{handle_id} is another worker now{who} (the daemon restarted): {} is gone", w.name));
                (None, true)
            }
            WatchReply::Page(page) if w.reused(&page) => {
                w.restart(format!("{handle_id} is another worker now (the daemon restarted): its thread from the start"));
                (Some(now), true)
            }
            WatchReply::Page(page) => {
                let more = page.more;
                let changed = w.apply(*page, now, scrolled_back);
                let next = if more {
                    Some(now)
                } else if w.drained {
                    None
                } else {
                    Some(now + POLL_INTERVAL)
                };
                (next, changed)
            }
            WatchReply::Gone(e) => {
                w.steerable = false;
                w.notice(format!("{e}: a finished worker stays watchable for 15 minutes"));
                w.gone = Some(e);
                w.transcript.set_busy(false);
                w.strings();
                (None, true)
            }
            WatchReply::Undecodable(e) => {
                w.steerable = false;
                w.notice(format!("this worker's journal did not decode ({e}): not asked again"));
                w.gone = Some(e);
                w.transcript.set_busy(false);
                w.strings();
                (None, true)
            }
            // Handled above; a variant from a newer client: ask again later.
            _ => (Some(now + RETRY), false),
        };
        if focused {
            self.next_poll = next;
        }
        changed
    }

    /// The session was attached again (a reconnect, a reload, a restarted
    /// daemon, its tab came back) to `daemon` (its pid): the views nobody
    /// looks at go (a cold-started daemon reuses `sa_N`), and the focused
    /// one takes no steer until a page says it may; it starts over from 0
    /// if the daemon is another process, else it reads on from its cursor.
    pub fn on_reconnect(&mut self, now: Instant, daemon: Option<u32>) {
        self.unsupported = false;
        self.reset(now);
        if let Some(pid) = daemon {
            self.daemon_changed(pid, now);
        }
    }

    /// The daemon is process `pid` (its control connection says so, or an
    /// attach): another one than the views were read from (a restart,
    /// which numbers its workers from 1 again) starts the focused view
    /// over from 0, whichever of the two connections comes back first.
    pub fn daemon_changed(&mut self, pid: u32, now: Instant) {
        let restarted = self.daemon.is_some_and(|d| d != pid);
        self.daemon = Some(pid);
        if !restarted {
            return;
        }
        self.reset(now);
        // The views kept read from 0 again (a worker tab's goes if its
        // handle names another worker now).
        let held = std::mem::take(&mut self.held);
        for (id, w) in self.workers.iter_mut() {
            if self.focus.as_deref() == Some(id.as_str()) || held.contains(id) {
                let why = format!("the daemon restarted: {}'s thread from the start", w.handle_id);
                w.restart(why);
            }
        }
        self.held = held;
    }

    /// The connection the views were read on went: nothing in flight, no
    /// steer waits for word, the views nobody looks at go and the focused
    /// one takes no steer until a page says it may.
    fn reset(&mut self, now: Instant) {
        self.pending = None;
        self.steers.clear();
        let (focus, held) = (self.focus.clone(), std::mem::take(&mut self.held));
        self.retain(|id, _| focus.as_deref() == Some(id) || held.iter().any(|h| h == id));
        self.held = held;
        for w in self.workers.values_mut() {
            w.steerable = false;
            w.strings();
        }
        self.next_poll = self.focus.is_some().then_some(now);
    }

    /// Its tab was left: nothing polls in the background.
    pub fn suspend(&mut self) {
        self.pending = None;
        self.next_poll = None;
        self.steers.clear();
    }

    /// Drop the views nobody is looking at (on screen, or held by a
    /// worker tab: [`Watch::hold`]) whose worker no longer runs: gone,
    /// finished by its own pages, or (a view that was not on screen when it
    /// ended) by the tray's word, `live`. Forget the tray's ended workers
    /// after [`FINISHED_TTL`].
    pub fn prune(&mut self, now: Instant, live: &[Live]) {
        let keep: Vec<String> = self.workers.keys().filter(|id| self.kept(id)).cloned().collect();
        self.retain(|id, w| {
            let ended = w.read_only() || w.finished_at.is_some() || live.iter().any(|l| l.id == w.subagent_id && l.status.terminal());
            keep.iter().any(|k| k == id) || !ended
        });
        self.seen.retain(|_, ended| ended.is_none_or(|t| now.saturating_duration_since(t) < FINISHED_TTL));
    }

    /// Keep the views `keep` says; a refused steer's text in one that goes
    /// waits for the session's thread instead.
    fn retain(&mut self, keep: impl Fn(&str, &Worker) -> bool) {
        let mut lost: Option<(String, String)> = self.unsent.take();
        self.workers.retain(|id, w| {
            let kept = keep(id, w);
            if !kept && let Some((reason, text)) = w.unsent.take() {
                lost = Some(match lost.take() {
                    Some((note, prev)) => (note, format!("{prev}\n{text}")),
                    None => (refused_note(&w.name, id, &reason), text),
                });
            }
            kept
        });
        self.unsent = lost;
    }

    // ── steering ───────────────────────────────────────────────────────

    /// Enter in a worker's view: the worker to steer, or why not.
    /// `None`: no worker's view is focused.
    pub fn steer_target(&self) -> Option<Result<String, String>> {
        let id = self.focus.as_deref()?;
        // Never `None` with a view focused: that would steer the session.
        let Some(w) = self.workers.get(id) else { return Some(Err("this worker's view has gone".into())) };
        Some(if self.unsupported {
            Err(UNSUPPORTED.into())
        } else if w.read_only() {
            Err(format!("{} is not running: its view is read-only", w.name))
        } else if w.steerable {
            Ok(id.to_string())
        } else if w.status.is_empty() {
            Err(format!("{} is still connecting: try again in a moment", w.name))
        } else {
            Err(format!("{} can't be steered right now: try again in a moment", w.name))
        })
    }

    /// A steer went out: the view says so; the worker shows it as a user
    /// row once it takes it in (at its next step).
    pub fn steer_sent(&mut self, handle_id: &str, text: &str) {
        if self.steers.len() == STEERS_MAX {
            self.steers.remove(0);
        }
        let name = self.workers.get(handle_id).map_or_else(|| handle_id.to_string(), |w| w.name.clone());
        self.steers.push(Steer { handle_id: handle_id.to_string(), name: name.clone(), text: text.to_string() });
        if let Some(w) = self.workers.get_mut(handle_id) {
            // Its text shows once the worker takes it in (a YOU row).
            w.notice(format!("→ steer sent: {name} takes it in at its next step"));
        }
    }

    /// The engine noted the steer of `handle_id` in the main conversation.
    pub fn steer_accepted(&mut self, handle_id: &str) {
        if let Some(i) = self.steers.iter().position(|s| s.handle_id == handle_id) {
            self.steers.remove(i);
        }
    }

    /// The daemon refused a steer: the one its reason names (`sa_N`), else
    /// the oldest. Its view says why and keeps the text for the composer
    /// ([`Watch::take_unsent`]); if the view has gone, the session's
    /// thread gets it. Whether one was pending.
    pub fn steer_refused(&mut self, reason: &str) -> bool {
        let named = named_handle(reason);
        let i = named.and_then(|h| self.steers.iter().position(|s| s.handle_id == h)).unwrap_or(0);
        if i >= self.steers.len() {
            return false;
        }
        let Steer { handle_id, name, text } = self.steers.remove(i);
        match self.workers.get_mut(&handle_id) {
            Some(w) => {
                w.notice(format!("steer refused: {reason}"));
                w.unsent = Some(match w.unsent.take() {
                    Some((why, prev)) => (why, format!("{prev}\n{text}")),
                    None => (reason.to_string(), text),
                });
            }
            None => {
                self.unsent = Some(match self.unsent.take() {
                    Some((note, prev)) => (note, format!("{prev}\n{text}")),
                    None => (refused_note(&name, &handle_id, reason), text),
                })
            }
        }
        true
    }

    /// A refused steer's text waits for the thread on screen ([`Watch::take_unsent`]).
    pub fn has_unsent(&self) -> bool {
        match self.focus.as_deref() {
            Some(id) => self.workers.get(id).is_some_and(|w| w.unsent.is_some()),
            None => self.unsent.is_some(),
        }
    }

    /// A refused steer's text for the composer, once the thread it is for
    /// is on screen: the focused view's own, or one whose view has gone
    /// when the session's thread is; with what the composer says of it.
    pub fn take_unsent(&mut self) -> Option<(String, String)> {
        match self.focus.as_deref() {
            Some(id) => {
                let w = self.workers.get_mut(id)?;
                let (_, text) = w.unsent.take()?;
                Some((text, format!("your refused steer is back: Enter sends it to {} again", w.name)))
            }
            None => {
                let (note, text) = self.unsent.take()?;
                Some((text, note))
            }
        }
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
        w.on_reconnect(t0, None);
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)));
        // Reconnecting: the poll could not go; again a second later.
        w.poll_failed(t0);
        assert_eq!(w.poll_due(), Some(t0 + RETRY));
    }

    /// A worker tab's view: the same view, steered from its tab, its hints
    /// its tab's; left for another tab (its session's thread, or another
    /// session: suspended) it asks for nothing.
    #[test]
    fn a_worker_tab_steers_its_worker_and_polls_nothing_in_the_background() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open_tab(&Candidate::of(2, "sleeper"), None, t0);
        assert!(w.in_tab());
        let v = w.focused().unwrap();
        assert!(texts(v)[0].1.contains("in its own tab"), "{:?}", texts(v));
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)));
        w.on_reply("sa_2", 0, running(1, false, json!([])), t0);
        // Steered from its tab: Enter goes to the worker, never the session.
        assert_eq!(w.steer_target(), Some(Ok("sa_2".into())));
        let hint = w.focused().unwrap().hint.to_string();
        assert!(hint.starts_with("steer sleeper") && !hint.contains("Esc"), "{hint}");
        w.steer_sent("sa_2", "stop after the next step");
        w.steer_accepted("sa_2");
        // Its session's tab shown (same attachment): the session's thread,
        // and no poll.
        w.focus_main();
        assert!(!w.in_tab());
        assert_eq!((w.poll_due(), w.take_poll(t0 + POLL_TIMEOUT), w.steer_target()), (None, None, None));
        // Back to the tab, then another session's tab (its watch stashed).
        w.open_tab(&Candidate::of(2, "sleeper"), None, t0);
        w.suspend();
        assert_eq!((w.poll_due(), w.take_poll(t0 + POLL_TIMEOUT)), (None, None));
        // The same worker shown in place over the session's thread: the
        // session's hints again.
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        assert!(!w.in_tab());
        assert!(w.focused().unwrap().hint.contains("Esc returns to the session"));
        // Gone on attach (a handle does not survive a park or a restart):
        // its tab says so and is read-only.
        w.open_tab(&Candidate::of(2, "sleeper"), None, t0);
        let (_, since) = w.take_poll(t0).unwrap();
        w.on_reply("sa_2", since, WatchReply::Gone("no subagent sa_2 in this session".into()), t0);
        let v = w.focused().unwrap();
        assert!(v.read_only() && v.state() == "gone");
        assert_eq!(w.view_state("sa_2"), Some("gone"));
        assert!(matches!(w.steer_target(), Some(Err(_))));
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
        // A view that is not on screen is not on offer once its worker
        // stopped running; the focused one is, so a walk from it goes to
        // its neighbours.
        assert_eq!(w.step(Step::Up, &cands(&[1]), None, t0), Moved::Worker("sa_1".into()));
        w.open(&Candidate::of(3, "w3"), None, t0);
        assert_eq!(w.step(Step::Up, &cands(&[1, 2]), None, t0), Moved::Worker("sa_2".into()));
        assert_eq!(w.step(Step::Down, &cands(&[1, 2]), None, t0), Moved::Main, "sa_3 is no longer on offer");
        w.open(&Candidate::of(3, "w3"), None, t0);
        w.leave_on_esc(t0);
        assert_eq!(w.focus(), None);
        assert!(w.esc_guarded(t0 + Duration::from_millis(100)) && !w.esc_guarded(t0 + ESC_GUARD));
    }

    /// Steering only a running worker that takes it; a refusal says so in
    /// the view and its text comes back into the composer while that view
    /// is on screen; the engine's note accepts it.
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
        assert!(w.steer_refused("input is owned by client #1"));
        assert_eq!(w.take_unsent().map(|u| u.0).as_deref(), Some("stop after the next step"));
        assert_eq!(w.take_unsent(), None, "once");
        let notes = texts(w.focused().unwrap());
        assert_eq!(notes[notes.len() - 2].1, "→ steer sent: sleeper takes it in at its next step");
        assert_eq!(notes.last().unwrap().1, "steer refused: input is owned by client #1");
        assert!(!w.steer_refused("again"), "nothing pending");
        // Accepted: nothing comes back.
        w.steer_sent("sa_2", "x");
        w.steer_accepted("sa_2");
        assert!(!w.steer_refused("late"));
        // Not steerable (no steer channel): no steer goes.
        let _ = w.take_poll(t0 + POLL_INTERVAL);
        w.on_reply("sa_2", 1, page(json!({"handle_id": "sa_2", "status": "running", "steerable": false, "next_seq": 1, "events": []})), t0);
        assert_eq!(w.steer_target(), Some(Err("sleeper can't be steered right now: try again in a moment".into())));
    }

    /// M2: a worker tab's view stays while its tab does ([`Watch::hold`]),
    /// finished or not: nothing is paged twice, and a steer refused after
    /// its tab was left comes back into that tab's composer, never the
    /// session's. Its tab closed, it goes as any other view.
    #[test]
    fn a_worker_tabs_view_stays_with_its_tab() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.hold(vec!["sa_2".into()]);
        w.open_tab(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, running(4, false, json!([])), t0);
        w.steer_sent("sa_2", "stop after the next step");
        // Its session's tab is shown: the steer is refused there.
        w.focus_main();
        assert!(w.steer_refused("subagent sa_2 is not running"));
        let done = Live {
            id: 2,
            call: None,
            name: "sleeper".into(),
            status: crate::subagents::Status::Done,
            line: String::new(),
            tools: None,
            elapsed: None,
            watchable: true,
        };
        w.prune(t0, std::slice::from_ref(&done));
        assert_eq!(w.worker("sa_2").map(|v| v.next_seq), Some(4), "kept: its cursor too");
        assert!(!w.has_unsent() && w.take_unsent().is_none(), "never the session's composer");
        // A reconnect keeps it as well.
        w.on_reconnect(t0, None);
        assert!(w.worker("sa_2").is_some());
        // Back on its tab: its text, for its composer.
        w.open_tab(&Candidate::of(2, "sleeper"), None, t0);
        assert!(w.has_unsent());
        assert_eq!(w.take_unsent().map(|u| u.0).as_deref(), Some("stop after the next step"));
        // Its tab closed: it goes once nobody looks at it.
        w.focus_main();
        w.hold(Vec::new());
        w.prune(t0, std::slice::from_ref(&done));
        assert!(w.worker("sa_2").is_none());
    }

    /// H2: a worker tab's view is of one worker. A page that names another
    /// (a cold-started daemon gave the handle to it), even the first, makes
    /// it gone: nothing of the other worker is read into it, nothing
    /// steers it. In place, the view starts over as before. Opening the
    /// handle's new worker replaces the old one's view.
    #[test]
    fn a_worker_tab_never_shows_another_worker() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open_tab(&Candidate::of(2, "scout").with_call(Some("t_old")), None, t0);
        let _ = w.take_poll(t0);
        let builder = page(json!({"handle_id": "sa_2", "subagent_id": 2, "agent_name": "builder", "status": "running", "steerable": true,
            "next_seq": 3, "events": [{"kind": "text", "text": "builder's secret"}]}));
        assert!(w.on_reply("sa_2", 0, builder, t0));
        let v = w.focused().unwrap();
        assert_eq!(v.state(), "gone");
        assert!(texts(v).iter().all(|(_, t)| !t.contains("builder's secret")), "nothing of builder's");
        assert!(texts(v).last().unwrap().1.contains("sa_2 is another worker now (builder)"));
        assert_eq!(w.poll_due(), None, "nothing polls");
        assert!(w.steer_target().is_some_and(|t| t.is_err()), "nothing steers");
        // The handle's new worker opened (its own call): a view of its own.
        w.open_tab(&Candidate::of(2, "builder").with_call(Some("t_new")), None, t0);
        assert_eq!(w.focused().unwrap().state(), "connecting");
        // Disowned from outside (the session's workers say so): gone, once.
        assert!(w.disown("sa_2", || "sa_2 is another worker now".into()));
        assert!(!w.disown("sa_2", || unreachable!()));
    }

    /// A refusal goes to the steer its reason names (two in flight), and
    /// its text waits for its view: back in the composer once that view is
    /// on screen again, never while another thread is; a view that has
    /// gone hands it to the session's thread, with a note.
    #[test]
    fn a_refused_steer_waits_for_its_thread() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        for (id, name) in [(2, "sleeper"), (3, "scout")] {
            w.open(&Candidate::of(id, name), None, t0);
            let _ = w.take_poll(t0);
            let p = json!({"handle_id": format!("sa_{id}"), "agent_name": name, "status": "running", "steerable": true, "next_seq": 1, "events": []});
            w.on_reply(&format!("sa_{id}"), 0, page(p), t0);
        }
        w.steer_sent("sa_2", "to sleeper");
        w.steer_sent("sa_3", "to scout");
        // The second one's refusal, while scout's view is on screen.
        assert!(w.steer_refused("subagent sa_3 is not running"));
        assert_eq!(w.take_unsent().map(|u| u.0).as_deref(), Some("to scout"));
        // The first one's, after its view was left: it waits for it.
        assert!(w.steer_refused("input is owned by client #1"));
        assert_eq!(w.take_unsent(), None, "scout's view is on screen");
        w.focus_main();
        assert_eq!(w.take_unsent(), None, "nor on the session's thread");
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let (text, note) = w.take_unsent().unwrap();
        assert_eq!((text.as_str(), note.as_str()), ("to sleeper", "your refused steer is back: Enter sends it to sleeper again"));
        // Its view gone (finished, not on screen): the session's thread.
        w.steer_sent("sa_3", "late");
        w.prune(
            t0,
            &[Live {
                id: 3,
                call: None,
                name: "scout".into(),
                status: crate::subagents::Status::Done,
                line: String::new(),
                tools: None,
                elapsed: None,
                watchable: true,
            }],
        );
        assert!(w.worker("sa_3").is_none());
        assert!(w.steer_refused("no subagent sa_3"));
        assert_eq!(w.take_unsent(), None, "sleeper's view is on screen");
        w.focus_main();
        let (text, note) = w.take_unsent().unwrap();
        assert_eq!(text, "late");
        assert!(note.contains("Enter now sends it to the session"), "{note}");
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
        w.on_reconnect(t0, None);
        assert!(!w.unsupported);
        assert!(w.worker("sa_2").is_none(), "a view nobody looks at goes with the connection");
        assert_eq!(w.take_poll(t0), Some(("sa_3".into(), 0)));
        assert!(w.on_reply("sa_3", 0, WatchReply::Gone("no subagent sa_3 in this session".into()), t0));
        assert_eq!((w.focused().unwrap().state(), w.poll_due()), ("gone", None));
        assert_eq!(w.candidates(&[]).iter().map(|c| c.handle_id.as_str()).collect::<Vec<_>>(), ["sa_3"], "the focused one");
        w.focus_main();
        w.prune(t0, &[]);
        assert!(w.worker("sa_3").is_none());
        assert!(w.candidates(&[]).is_empty());
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

    /// A finished worker's view goes once nobody looks at it: one that saw
    /// the end, and one that was not on screen when it came (the tray's
    /// word); a running worker's view stays.
    #[test]
    fn a_finished_view_goes_once_nobody_looks_at_it() {
        use crate::subagents::Status;
        let l = |id, status| Live {
            id,
            call: None,
            name: format!("w{id}"),
            status,
            line: String::new(),
            tools: None,
            elapsed: None,
            watchable: true,
        };
        let t0 = Instant::now();
        let mut w = Watch::default();
        for id in [1, 3] {
            w.open(&Candidate::of(id, &format!("w{id}")), None, t0);
            let _ = w.take_poll(t0);
            let p = json!({"handle_id": format!("sa_{id}"), "status": "running", "steerable": true, "next_seq": 1, "events": []});
            w.on_reply(&format!("sa_{id}"), 0, page(p), t0);
        }
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply(
            "sa_2",
            0,
            page(json!({"handle_id": "sa_2", "status": "failed", "next_seq": 1, "events": [{"kind": "failed", "category": "overloaded"}]})),
            t0,
        );
        assert_eq!(w.focused().unwrap().state(), "failed");
        let live = [l(1, Status::Done), l(2, Status::Failed), l(3, Status::Running)];
        w.prune(t0, &live);
        assert!(w.worker("sa_2").is_some(), "on screen");
        assert!(w.worker("sa_1").is_none(), "ended while not on screen: the tray says so");
        assert!(w.worker("sa_3").is_some(), "still running");
        w.focus_main();
        w.prune(t0, &live);
        assert!(w.worker("sa_2").is_none() && w.worker("sa_3").is_some());
    }

    /// The tray: running workers; a finished one for [`FLASH`] after it
    /// was first seen ended, only if this window saw it running; one only
    /// the history knows of, never; the focused one always.
    #[test]
    fn the_tray_flashes_a_finished_worker_then_drops_it() {
        use crate::subagents::Status;
        let l = |id, status| Live {
            id,
            call: None,
            name: format!("w{id}"),
            status,
            line: String::new(),
            tools: None,
            elapsed: None,
            watchable: true,
        };
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

    /// A worker's transcript keeps its newest rows: past the bound, down to
    /// three quarters of it at once; scrolled back, nothing goes until
    /// twice the bound.
    #[test]
    fn a_long_worker_keeps_its_newest_rows() {
        let t0 = Instant::now();
        let calls = |from: usize, n: usize| {
            json!(
                (from..from + n)
                    .map(|i| json!({"kind": "tool_use", "tool_id": format!("t{i}"), "tool_name": "bash", "input": {"command": "ls"}}))
                    .collect::<Vec<_>>()
            )
        };
        let mut w = Watch::default();
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, running(1, false, calls(0, WORKER_ITEMS - 1)), t0);
        assert_eq!((w.focused().unwrap().transcript.len(), w.focused().unwrap().dropped), (WORKER_ITEMS, 0), "the notice and 399 calls");
        // Scrolled back: kept, up to twice the bound.
        w.scrolled_back = true;
        let _ = w.take_poll(t0 + POLL_INTERVAL);
        w.on_reply("sa_2", 1, running(2, false, calls(1000, WORKER_ITEMS)), t0);
        assert_eq!((w.focused().unwrap().transcript.len(), w.focused().unwrap().dropped), (2 * WORKER_ITEMS, 0));
        let _ = w.take_poll(t0 + 2 * POLL_INTERVAL);
        w.on_reply("sa_2", 2, running(3, false, calls(2000, 1)), t0);
        let v = w.focused().unwrap();
        assert_eq!((v.transcript.len(), v.dropped), (WORKER_ITEMS * 3 / 4, 2 * WORKER_ITEMS + 1 - WORKER_ITEMS * 3 / 4));
        assert_eq!(v.facts.as_ref(), format!("{} earlier rows dropped", v.dropped));
        // At the tail: past the bound, down to three quarters.
        w.scrolled_back = false;
        let _ = w.take_poll(t0 + 3 * POLL_INTERVAL);
        w.on_reply("sa_2", 3, running(4, false, calls(3000, WORKER_ITEMS / 4)), t0);
        assert_eq!(w.focused().unwrap().transcript.len(), WORKER_ITEMS, "at the bound: kept");
        let _ = w.take_poll(t0 + 4 * POLL_INTERVAL);
        w.on_reply("sa_2", 4, running(5, false, calls(4000, 1)), t0);
        assert_eq!(w.focused().unwrap().transcript.len(), WORKER_ITEMS * 3 / 4);
    }

    /// An idle worker's empty pages change nothing to draw; a page with
    /// rows, or a new state, does.
    #[test]
    fn an_empty_page_changes_nothing() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        assert!(w.on_reply("sa_2", 0, running(1, false, json!([])), t0), "the first page names it and says it runs");
        let hint = w.focused().unwrap().hint.clone();
        for i in 1..5 {
            let _ = w.take_poll(t0 + POLL_INTERVAL * i);
            assert!(!w.on_reply("sa_2", 1, running(1, false, json!([])), t0), "idle: nothing to draw");
            assert_eq!(w.poll_due(), Some(t0 + POLL_INTERVAL), "and the next poll is due");
        }
        assert!(w.focused().unwrap().hint.as_ptr() == hint.as_ptr(), "its strings were not made again");
        let _ = w.take_poll(t0 + POLL_INTERVAL * 5);
        assert!(w.on_reply("sa_2", 1, running(2, false, json!([{"kind": "text", "text": "Step 1."}])), t0));
    }

    /// A cold-started daemon numbers its workers from 1 again. A page whose
    /// cursor goes back (the daemon clamps a cursor past a journal's end),
    /// or that names another worker, starts the view over from 0; a
    /// reconnect takes the focused view's steer away until a page gives it
    /// back, drops the views nobody looks at, and starts the focused one
    /// over if the daemon is another process.
    #[test]
    fn a_reused_handle_starts_the_view_over() {
        let t0 = Instant::now();
        let mut w = Watch::default();
        w.on_reconnect(t0, Some(100));
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, running(9, false, json!([{"kind": "text", "text": "old worker"}])), t0);
        let generation = w.focus_key().unwrap().1;
        // A cursor that went back: another journal behind sa_2.
        let _ = w.take_poll(t0 + POLL_INTERVAL);
        assert!(w.on_reply("sa_2", 9, running(3, false, json!([{"kind": "text", "text": "new worker"}])), t0));
        let v = w.focused().unwrap();
        assert!(w.focus_key().unwrap().1 != generation, "another list");
        assert_eq!((v.next_seq, v.status.as_str(), v.steerable), (0, "", false));
        assert!(texts(v).iter().all(|(_, t)| t != "old worker" && t != "new worker"));
        assert_eq!(texts(v).last().unwrap().1, "sa_2 is another worker now (the daemon restarted): its thread from the start");
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)), "asked from 0 at once");
        // Another name at a cursor ahead: also another worker.
        assert!(w.on_reply("sa_2", 0, running(4, false, json!([])), t0));
        let generation = w.focus_key().unwrap().1;
        let _ = w.take_poll(t0 + POLL_INTERVAL);
        let other =
            json!({"handle_id": "sa_2", "agent_name": "scout", "status": "running", "steerable": true, "next_seq": 6, "events": []});
        assert!(w.on_reply("sa_2", 4, page(other), t0));
        assert!(w.focus_key().unwrap().1 != generation && w.focused().unwrap().next_seq == 0);
        // A reconnect to the same daemon: no steer until a page says so;
        // the views nobody looks at go.
        let _ = w.take_poll(t0);
        w.on_reply("sa_2", 0, running(4, false, json!([])), t0);
        w.open(&Candidate::of(5, "idle"), None, t0);
        w.open(&Candidate::of(2, "sleeper"), None, t0);
        assert_eq!(w.steer_target(), Some(Ok("sa_2".into())));
        let generation = w.focus_key().unwrap().1;
        w.on_reconnect(t0, Some(100));
        assert!(w.worker("sa_5").is_none());
        assert_eq!(w.steer_target(), Some(Err("sleeper can't be steered right now: try again in a moment".into())));
        assert_eq!(w.focused().unwrap().hint, "sleeper can't be steered right now · Esc returns to the session", "nor says it can");
        assert_eq!((w.focus_key().unwrap().1, w.take_poll(t0)), (generation, Some(("sa_2".into(), 4))), "from its cursor");
        assert!(w.on_reply("sa_2", 4, running(4, false, json!([])), t0), "steerable again: drawn");
        assert_eq!(w.steer_target(), Some(Ok("sa_2".into())));
        assert!(w.focused().unwrap().hint.starts_with("steer sleeper: "));
        // Another daemon process: from 0.
        w.on_reconnect(t0, Some(200));
        assert!(w.focus_key().unwrap().1 != generation);
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)));
        assert!(matches!(w.steer_target(), Some(Err(_))));
    }

    /// N-L1: a cold start is seen whichever connection comes back first:
    /// the session's attach (still under the old pid), then the control
    /// connection's new pid; or the other way round. Either way the
    /// focused view starts over once, from 0, and takes no steer.
    #[test]
    fn a_restart_is_seen_whichever_connection_comes_back_first() {
        let t0 = Instant::now();
        let at_cursor = |pid: u32| {
            let mut w = Watch::default();
            w.daemon_changed(pid, t0);
            w.open(&Candidate::of(2, "sleeper"), None, t0);
            let _ = w.take_poll(t0);
            w.on_reply("sa_2", 0, running(40, false, json!([{"kind": "text", "text": "old worker"}])), t0);
            assert_eq!(w.steer_target(), Some(Ok("sa_2".into())));
            w
        };
        // The attach first: the model still holds the old pid.
        let mut w = at_cursor(100);
        let generation = w.focus_key().unwrap().1;
        w.on_reconnect(t0, Some(100));
        assert_eq!(w.focus_key().unwrap().1, generation, "the same daemon, as far as it knows");
        let _ = w.take_poll(t0);
        w.daemon_changed(200, t0);
        assert!(w.focus_key().unwrap().1 != generation, "started over");
        assert!(texts(w.focused().unwrap()).iter().all(|(_, t)| t != "old worker"));
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)), "from 0, the old query dropped");
        assert!(matches!(w.steer_target(), Some(Err(_))));
        // The control connection first: the attach after it starts nothing over again.
        let mut w = at_cursor(100);
        let generation = w.focus_key().unwrap().1;
        w.daemon_changed(200, t0);
        let restarted = w.focus_key().unwrap().1;
        assert!(restarted != generation);
        w.on_reconnect(t0, Some(200));
        assert_eq!(w.focus_key().unwrap().1, restarted, "once");
        assert_eq!(w.take_poll(t0), Some(("sa_2".into(), 0)));
        // The same daemon again: nothing starts over.
        w.daemon_changed(200, t0);
        assert_eq!(w.focus_key().unwrap().1, restarted);
    }

    /// N-L3: a refusal names its worker's handle, not a `sa_` inside a name.
    #[test]
    fn a_refusal_names_the_handle_not_a_name() {
        assert_eq!(named_handle("nasa_scout (sa_3) is not running").as_deref(), Some("sa_3"));
        assert_eq!(named_handle("visa_7 (sa_12) is not running").as_deref(), Some("sa_12"));
        assert_eq!(named_handle("no subagent sa_4").as_deref(), Some("sa_4"));
        assert_eq!(named_handle("input is owned by client #1"), None);
        assert_eq!(named_handle("nasa_scout is not running"), None);
    }
}
