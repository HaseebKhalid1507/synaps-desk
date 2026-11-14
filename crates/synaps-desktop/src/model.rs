//! The window's state and how client events change it. No GPUI: events are
//! applied here as they arrive (in the event pump, not in `render`), so a
//! hidden or minimized window holds the current transcript and nothing
//! else; the view only turns the transcript's splice into rows per frame.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use synaps_client::protocol::SessionMeta;
use synaps_client::{
    AttachMode, Error, Event, Expect, FEATURE_SAVED_SESSIONS, RenameRefusal, SAVED_SESSIONS_MAX, SavedSession, SessionEvent, Status,
    Transcript, validate_session_name,
};

use crate::named::{ResumeRequest, RowAction, parse_rfc3339};
use crate::rail::{Chip, State, is_parked};
use crate::stats::Stats;

/// HISTORY's newest unnamed sessions, asked for at startup; up to
/// [`SAVED_SESSIONS_MAX`] (the wire's cap) once OLDER opens or find is used.
pub const SAVED_RECENT: u32 = 200;

/// Most events the pump applies before it yields to the UI (a frame can run
/// between batches; a backlog never becomes one unbounded `Vec`).
pub const MAX_BATCH: usize = 1024;

/// The event pump. Waits for an event, takes whatever else is already
/// queued (up to [`MAX_BATCH`]) and hands the batch to `apply` at once,
/// then yields. Nothing here waits for a frame: a minimized or hidden
/// window drains the channel as fast as one that renders, so the channel
/// stays empty and the model is the only state. Ends when the channel
/// closes or `apply` returns `false` (the view is gone).
pub async fn pump(rx: async_channel::Receiver<Event>, mut apply: impl FnMut(Vec<Event>) -> bool) {
    while let Ok(first) = rx.recv().await {
        let mut batch = Vec::with_capacity((rx.len() + 1).min(MAX_BATCH));
        batch.push(first);
        while batch.len() < MAX_BATCH
            && let Ok(e) = rx.try_recv()
        {
            batch.push(e);
        }
        if !apply(batch) {
            break;
        }
        YieldNow(false).await;
    }
}

/// Pending once, so the executor runs other tasks (a frame) in between.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if std::mem::replace(&mut self.0, true) {
            return Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// A tool-call confirmation's answers, as the daemon reads them
/// (`session/actor.rs` `answer`: approved on "y" / "yes", any other text
/// denies; `None` cancels).
pub const CONFIRM_YES: &str = "y";
pub const CONFIRM_NO: &str = "n";

/// The connection pill.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Off(String),
    Connecting,
    Connected(String),
    Reconnecting,
    Incompatible(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub session_id: String,
    pub attach_seq: u64,
    /// Bumped on every `Attached`, reconnects included: a reconnect keeps
    /// the `attach_seq`, and a restarted daemon numbers its prompts from 1
    /// again, so `(attach_seq, prompt id)` could name a different prompt.
    pub epoch: u64,
    /// `Attached::connection`: answers go out on it only
    /// (`Client::answer_on`), never on a later one.
    pub connection: u64,
    pub mode: AttachMode,
}

/// What the view must do after an event (it owns the client and composer).
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Input the daemon refused (or never got): put it back in the composer.
    Restore(String),
    ListSessions,
    Resync,
    /// `Client::saved_sessions(named_only, limit)`; if it cannot be sent,
    /// tell [`Model::saved_failed`].
    ListSaved {
        named_only: bool,
        limit: u32,
    },
    /// Attach to a live session, then [`Model::attaching`]. `fallback`: a
    /// named session's journal, resumed if it ended meanwhile. `open`: an
    /// Open by name.
    Attach {
        session_id: String,
        fallback: Option<ResumeRequest>,
        open: bool,
    },
    /// `Client::resume(req.target, req.options())`, then [`Model::resuming`].
    Resume {
        req: ResumeRequest,
        open: bool,
    },
}

/// A resume in flight (`Client::resume`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resuming {
    pub req: ResumeRequest,
    pub attach_seq: u64,
}

/// A rename sent, not answered yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRename {
    pub session_id: String,
    pub name: String,
}

/// What to send: `Client::rename(session_id, name, expect)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameRequest {
    pub session_id: String,
    pub name: String,
    pub expect: Expect,
}

/// After `RenameRefusal::Named`: the offer to replace `current` with
/// `asked` on THIS session (compare-and-set on `current`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replace {
    pub session_id: String,
    pub asked: String,
    pub current: String,
}

/// An Open by name waiting for its outcome (the typed text stays).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opening {
    pub name: String,
    /// Sent as this attachment; `None`: not yet (waiting for the saved list).
    attach_seq: Option<u64>,
    /// A fresh saved list was asked for it.
    asked_saved: bool,
}

/// One `saved_sessions` request: the named list, or the newest `limit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedAsk {
    Named,
    Recent(u32),
}

/// What the rail knows of saved sessions, and the names and resumes.
#[derive(Debug, Default)]
pub struct Named {
    /// The daemon lists `saved_sessions` (else: no HISTORY; a name is
    /// resolved by the daemon).
    pub saved_supported: bool,
    /// The named saved sessions (`named_only`, up to the wire's cap).
    pub saved: Vec<SavedSession>,
    /// The daemon has more named ones than that.
    pub saved_truncated: bool,
    /// The newest saved sessions, named or not ([`SAVED_RECENT`], or the
    /// cap once `deep`).
    pub recent: Vec<SavedSession>,
    pub recent_truncated: bool,
    /// The newest list has landed at least once (HISTORY can be drawn).
    pub recent_loaded: bool,
    /// The cap has been asked for (OLDER opened, or find used).
    pub deep: bool,
    /// The named list must be asked for too if the newest list is cut:
    /// not listed on this connection yet, a rename, an Open by name.
    named_stale: bool,
    /// The saved list's own trouble (failed, entries skipped).
    pub saved_note: Option<String>,
    /// The last ask failed and nothing has landed since: HISTORY says "—",
    /// not "…" (find gaining focus asks again).
    pub saved_failed: bool,
    /// One request at a time: replies come in order, but this way a reply
    /// is never matched to the wrong request.
    saved_in_flight: Option<SavedAsk>,
    saved_queue: VecDeque<SavedAsk>,
    pub resuming: Option<Resuming>,
    /// An attach from a Named row or Open by name: if the session ended
    /// since the last list, resume its journal instead.
    attaching: Option<(u64, ResumeRequest)>,
    pub opening: Option<Opening>,
    /// An Open by name landed: the input can clear and close.
    pub opened: bool,
    /// The last resume / open-by-name error.
    pub error: Option<String>,
    /// Something to know that is not an error (a folder fell back to `~`).
    pub notice: Option<String>,
    /// Text the user was told the daemon may read as part of a session id
    /// (find → Enter, no saved list): Enter again with it sends it.
    confirm: Option<String>,
    /// How the notices name the daemon's machine.
    daemon: String,
    /// The "Name this session…" row is open.
    pub naming: bool,
    pub rename: Option<PendingRename>,
    /// The last rename error, for the naming row.
    pub rename_error: Option<String>,
    pub replace: Option<Replace>,
}

/// The `attach_seq` of local (bench) sessions fed straight into the pump.
pub const LOCAL_SEQ: u64 = u64::MAX;

/// The daemon's answer to attaching a session it does not hold
/// (`daemon/conn.rs`: `DaemonFrame::Error { message: "unknown session" }`).
const UNKNOWN_SESSION: &str = "unknown session";

pub struct Model {
    pub conn: ConnState,
    pub sessions: Vec<SessionMeta>,
    pub active: Option<Active>,
    /// The attachment asked for last: events of any other are stale.
    wanted: Option<u64>,
    /// The session it asked for (`None`: a create or a resume, whose id
    /// comes with `Attached`).
    wanted_session: Option<String>,
    /// Its state when it was asked for: under a chip that admitted it, its
    /// row stays until the attach lands.
    wanted_fact: Option<State>,
    /// Prompts answered from here that the daemon has not resolved yet, as
    /// `(epoch, prompt id)` ([`Active::epoch`]): prompt ids are per session
    /// and start at 1 (again after a daemon restart), so an id alone would
    /// carry over to the next attachment's prompt.
    answered: HashSet<(u64, u64)>,
    /// The last [`Active::epoch`] handed out.
    epoch: u64,
    pub transcript: Transcript,
    pub last_error: Option<String>,
    /// The session connection is being re-made (reload).
    pub reattaching: bool,
    /// Sent, not yet confirmed by `TurnStarted` / `Steered`, oldest first:
    /// given back to the user on a refusal (the oldest) or a detach (all).
    in_flight: VecDeque<String>,
    pub named: Named,
    /// The attached session's numbers (header, right-hand panel).
    pub stats: Stats,
    /// Bumped whenever what the rail lists may have changed (anything but
    /// a session's own stream, and a session list equal to the last): the
    /// view rebuilds its rows only then.
    pub rail_gen: u64,
    /// Sessions (and their journals) whose turn ended, or which got new
    /// messages, while not on screen: UNREAD until opened.
    unread: HashSet<String>,
    /// What the user has seen of each held session, `(messages, busy)`:
    /// the last poll for one off screen; for the one on screen its
    /// transcript, which runs ahead of the poll (taken at every poll and
    /// when the user moves away). UNREAD is the next poll against this.
    seen: HashMap<String, (usize, bool)>,
    /// Sessions whose turn this window sent: its end is not news.
    own_turns: HashSet<String>,
    /// Set by every event but a session list equal to the last: the view
    /// draws again only then (the 3-second poll of an idle daemon is free).
    pub view_dirty: bool,
}

/// The order OPEN shows: newest conversation first (`created_at`), then by
/// id. The daemon sends its list in hash-map order.
pub fn sort_sessions(s: &mut [SessionMeta]) {
    s.sort_by_cached_key(|m| (Reverse(parse_rfc3339(&m.created_at).unwrap_or(i64::MIN)), Reverse(m.id.clone())));
}

/// Text the daemon's resolver could match inside a session id
/// (`YYYYMMDD-HHMMSS-xxxx`, lowercase hex).
fn could_be_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c) || c == '-')
}

impl Model {
    pub fn new(conn: ConnState) -> Self {
        Self {
            conn,
            sessions: Vec::new(),
            active: None,
            wanted: None,
            wanted_session: None,
            wanted_fact: None,
            answered: HashSet::new(),
            epoch: 0,
            transcript: Transcript::new(),
            last_error: None,
            reattaching: false,
            in_flight: VecDeque::new(),
            named: Named { daemon: "the daemon's machine".into(), ..Named::default() },
            stats: Stats::default(),
            rail_gen: 0,
            unread: HashSet::new(),
            seen: HashMap::new(),
            own_turns: HashSet::new(),
            view_dirty: false,
        }
    }

    /// Ask for the saved lists again (on connect, after a rename, a resume,
    /// a row leaving OPEN, find gaining focus; never on a timer): the
    /// newest, and the named list only if the newest was cut (every
    /// request costs the daemon a read of every saved header). A request
    /// already waiting is not queued twice.
    fn refresh_saved(&mut self, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        if !n.saved_supported {
            return;
        }
        let recent = SavedAsk::Recent(if n.deep { SAVED_SESSIONS_MAX } else { SAVED_RECENT });
        if !n.saved_queue.contains(&recent) {
            n.saved_queue.push_back(recent);
        }
        self.pump_saved(effects);
    }

    /// A refresh is asked for or on its way.
    pub fn saved_listing(&self) -> bool {
        self.named.saved_in_flight.is_some() || !self.named.saved_queue.is_empty()
    }

    /// Find gained focus: the user is looking, so list what find searches
    /// (up to the cap), fresh.
    pub fn looking(&mut self) -> Vec<Effect> {
        if !self.named.deep {
            return self.want_deep();
        }
        let mut effects = Vec::new();
        self.refresh_saved(&mut effects);
        effects
    }

    /// Send the next queued request if none is in flight.
    fn pump_saved(&mut self, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        if n.saved_in_flight.is_some() {
            return;
        }
        if let Some(ask) = n.saved_queue.pop_front() {
            n.saved_in_flight = Some(ask);
            effects.push(match ask {
                SavedAsk::Named => Effect::ListSaved { named_only: true, limit: SAVED_SESSIONS_MAX },
                SavedAsk::Recent(limit) => Effect::ListSaved { named_only: false, limit },
            });
        }
    }

    /// OLDER was opened or find used: list up to the wire's cap, once.
    pub fn want_deep(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        let n = &mut self.named;
        if !n.saved_supported || n.deep {
            return effects;
        }
        n.deep = true;
        n.saved_queue.retain(|a| *a == SavedAsk::Named);
        n.saved_queue.push_front(SavedAsk::Recent(SAVED_SESSIONS_MAX));
        self.pump_saved(&mut effects);
        effects
    }

    /// `--offline`: the synthetic transport answers `saved_sessions` itself.
    pub fn offer_saved(&mut self) -> Vec<Effect> {
        self.named.saved_supported = true;
        self.named.named_stale = true;
        let mut effects = Vec::new();
        self.refresh_saved(&mut effects);
        effects
    }

    /// The saved list did not come (or could not be asked for).
    pub fn saved_failed(&mut self, reason: &str) -> Vec<Effect> {
        self.rail_gen += 1;
        let mut effects = Vec::new();
        self.on_saved_failed(reason, &mut effects);
        effects
    }

    fn on_saved_failed(&mut self, reason: &str, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        n.saved_in_flight = None;
        n.saved_queue.clear();
        n.saved_note = Some(format!("Saved sessions unavailable: {reason}"));
        n.saved_failed = true;
        // An Open by name that waited for the list: the daemon resolves it.
        if n.opening.as_ref().is_some_and(|o| o.attach_seq.is_none()) {
            self.open_by_resolver(effects);
        }
    }

    /// A held session's glyph, first match wins (see [`State`]).
    pub fn state(&self, s: &SessionMeta) -> State {
        if self.opening(s) { State::Opening } else { self.fact(s) }
    }

    /// What a held session is doing, without OPENING (which is only this
    /// window's click): what the counts, the chips, the ticker and JUMP go
    /// by, so none of them changes while an attach is on its way. The
    /// attached one is read from its own transcript (answered prompts, a
    /// turn that just ended), which runs ahead of the 3-second poll.
    pub fn fact(&self, s: &SessionMeta) -> State {
        let (needs, busy) = if self.is_active(&s.id) {
            (!self.transcript.pending_prompts().is_empty(), self.transcript.busy())
        } else {
            (s.awaiting_input > 0, s.busy)
        };
        if needs {
            State::NeedsYou
        } else if busy {
            State::Running
        } else if s.locked_by.is_some() {
            State::Locked
        } else if self.is_unread(s) {
            State::Unread
        } else if is_parked(s) {
            State::Parked
        } else {
            State::Idle
        }
    }

    /// An attach or a resume of this held session is on its way.
    pub fn opening(&self, s: &SessionMeta) -> bool {
        (self.is_wanted(&s.id) && !self.is_active(&s.id))
            || self
                .named
                .resuming
                .as_ref()
                .is_some_and(|r| s.is(&r.req.target) || (r.req.by_name && s.name.as_deref() == Some(&r.req.label)))
    }

    /// Under a chip: its state; a row clicked under the chip stays where
    /// it was until its attach lands, whatever its state does meanwhile.
    pub fn chip_shows(&self, chip: Chip, s: &SessionMeta) -> bool {
        chip.admits(self.fact(s)) || (self.opening(s) && self.is_wanted(&s.id) && self.wanted_fact.is_some_and(|f| chip.admits(f)))
    }

    fn is_unread(&self, s: &SessionMeta) -> bool {
        self.unread.contains(&s.id) || (!s.journal_id.is_empty() && self.unread.contains(&s.journal_id))
    }

    /// A saved (not held) session's glyph.
    pub fn saved_state(&self, v: &SavedSession) -> State {
        if self.saved_opening(v) { State::Opening } else { self.saved_fact(v) }
    }

    /// A saved session's state without OPENING (see [`Model::fact`]).
    pub fn saved_fact(&self, v: &SavedSession) -> State {
        if self.unread.contains(&v.id) { State::Unread } else { State::Saved }
    }

    fn saved_opening(&self, v: &SavedSession) -> bool {
        self.named.resuming.as_ref().is_some_and(|r| r.req.target == v.id || (r.req.by_name && v.name.as_deref() == Some(&r.req.label)))
    }

    /// [`Model::chip_shows`] for a saved session (its UNREAD mark holds
    /// until the resume lands).
    pub fn chip_shows_saved(&self, chip: Chip, v: &SavedSession) -> bool {
        chip.admits(self.saved_fact(v))
    }

    pub fn is_active(&self, session_id: &str) -> bool {
        self.active.as_ref().is_some_and(|a| a.session_id == session_id)
    }

    /// Live sessions waiting on the user (a question, a secret, a tool-call
    /// confirmation), in list order. One being attached still counts.
    pub fn attention(&self) -> impl Iterator<Item = &SessionMeta> {
        self.sessions.iter().filter(|s| s.lifecycle != "ending" && self.fact(s) == State::NeedsYou)
    }

    /// The ticker's session: a waiting one other than the attached, else
    /// the attached one.
    pub fn ticker(&self) -> Option<&SessionMeta> {
        self.attention().find(|s| !self.is_active(&s.id)).or_else(|| self.attention().next())
    }

    /// JUMP's target: the ticker's session, unless it is on screen. It
    /// stays the same while its attach is on its way, so a second click
    /// is a no-op ([`Model::open_row`]).
    pub fn jump_target(&self) -> Option<&str> {
        self.ticker().map(|s| s.id.as_str()).filter(|id| !self.is_active(id))
    }

    /// The active session's list entry.
    pub fn active_meta(&self) -> Option<&SessionMeta> {
        let a = self.active.as_ref()?;
        self.sessions.iter().find(|s| s.id == a.session_id)
    }

    /// The active session's name, if it has one.
    pub fn active_name(&self) -> Option<&str> {
        let a = self.active.as_ref()?;
        self.sessions.iter().find(|s| s.id == a.session_id)?.name.as_deref().filter(|n| !n.is_empty())
    }

    /// A live session's journal, to resume if it ended since the last list.
    fn journal_of(&self, session_id: &str) -> Option<ResumeRequest> {
        let s = self.sessions.iter().find(|s| s.id == session_id)?;
        let name = s.name.clone().filter(|n| !n.is_empty())?;
        let target = if s.journal_id.is_empty() { s.id.clone() } else { s.journal_id.clone() };
        Some(ResumeRequest { label: name, target, cwd: s.cwd.clone().filter(|c| !c.is_empty()), by_name: false })
    }

    /// A Named row was clicked.
    pub fn open_row(&mut self, action: &RowAction) -> Vec<Effect> {
        self.named.error = None;
        self.named.notice = None;
        match action {
            // Already the one on screen (or on its way): nothing to do.
            RowAction::Attach(id) if self.is_wanted(id) => Vec::new(),
            RowAction::Attach(id) => vec![Effect::Attach { session_id: id.clone(), fallback: self.journal_of(id), open: false }],
            RowAction::Resume(req) if self.named.resuming.as_ref().is_some_and(|r| r.req.target == req.target) => Vec::new(),
            RowAction::Resume(req) => vec![Effect::Resume { req: req.clone(), open: false }],
        }
    }

    /// Enter in find, in this order: a session of exactly that name in the
    /// loaded lists; else the one row find left (`visible`: the rows the
    /// rail shows); else, with no row showing, the daemon (see
    /// [`Model::open_by_name`]). Several rows and no exact name: nothing
    /// opens, the user picks.
    pub fn find_enter(&mut self, typed: &str, visible: &[RowAction]) -> Vec<Effect> {
        let typed = typed.trim();
        if self.named.confirm.as_deref() != Some(typed) {
            self.named.confirm = None;
        }
        if typed.is_empty() {
            return Vec::new();
        }
        if self.has_name(typed) {
            return self.open_by_name(typed);
        }
        match visible {
            [one] => self.open_row(one),
            [] if validate_session_name(typed).is_ok() => self.open_by_name(typed),
            rows => {
                self.named.notice = None;
                self.named.error = Some(match rows.len() {
                    0 => format!("Nothing matches '{typed}'."),
                    k => format!("{k} sessions match '{typed}': click one, or type its exact name."),
                });
                Vec::new()
            }
        }
    }

    /// A session of exactly this name is listed (held or saved).
    fn has_name(&self, name: &str) -> bool {
        let n = &self.named;
        self.sessions.iter().any(|s| s.name.as_deref() == Some(name))
            || n.saved.iter().chain(&n.recent).any(|v| v.name.as_deref() == Some(name))
    }

    /// "Open by name…": the name exactly, never a partial id. A live
    /// session of that name is attached; a saved one is resumed by its
    /// journal id in its folder. Not in the lists: a daemon with the saved
    /// list is asked once more (it may be new), then it is "no session
    /// named"; a daemon without it resolves the name itself, and the
    /// attached session must carry that name.
    pub fn open_by_name(&mut self, typed: &str) -> Vec<Effect> {
        let name = typed.trim();
        let n = &mut self.named;
        n.error = None;
        n.notice = None;
        n.opened = false;
        if let Err(e) = validate_session_name(name) {
            n.error = Some(e.to_string());
            return Vec::new();
        }
        n.opening = Some(Opening { name: name.to_string(), attach_seq: None, asked_saved: false });
        let mut effects = Vec::new();
        self.resolve_open(&mut effects);
        effects
    }

    /// Resolve a waiting Open by name against the lists (exact name).
    fn resolve_open(&mut self, effects: &mut Vec<Effect>) {
        let Some((name, asked_saved)) =
            self.named.opening.as_ref().filter(|o| o.attach_seq.is_none()).map(|o| (o.name.clone(), o.asked_saved))
        else {
            return;
        };
        if let Some(s) = self.sessions.iter().find(|s| s.name.as_deref() == Some(name.as_str())) {
            let id = s.id.clone();
            effects.push(Effect::Attach { fallback: self.journal_of(&id), session_id: id, open: true });
            return;
        }
        let n = &self.named;
        if n.saved_supported {
            if let Some(v) = n.saved.iter().chain(&n.recent).find(|v| v.name.as_deref() == Some(name.as_str())) {
                let req = ResumeRequest { label: name, target: v.id.clone(), cwd: v.cwd.clone().filter(|c| !c.is_empty()), by_name: false };
                effects.push(Effect::Resume { req, open: true });
                return;
            }
            if !n.saved_truncated {
                if asked_saved {
                    self.named.error = Some(format!("No session named '{name}'."));
                    self.named.opening = None;
                } else {
                    if let Some(o) = &mut self.named.opening {
                        o.asked_saved = true;
                    }
                    self.named.named_stale = true;
                    self.refresh_saved(effects);
                }
                return;
            }
        }
        self.open_by_resolver(effects);
    }

    /// No list can resolve the name: let the daemon, and check on attach.
    /// Its resolver (v0.11.1 `resolve_session`: chain, name, then the one
    /// id containing the text) can open a session nobody picked. Ids are
    /// `YYYYMMDD-HHMMSS-xxxx`, so only text of digits, `a`-`f` and `-` can
    /// be part of one: such text goes only on a second Enter, after the
    /// user was told.
    fn open_by_resolver(&mut self, effects: &mut Vec<Effect>) {
        let Some(name) = self.named.opening.as_ref().map(|o| o.name.clone()) else { return };
        let n = &mut self.named;
        if could_be_id(&name) && n.confirm.as_deref() != Some(name.as_str()) {
            n.opening = None;
            n.error = Some(format!(
                "'{name}' could be part of a session id: if no session has this name, the daemon opens the one whose id contains it. \
                 Press Enter again to ask it."
            ));
            n.confirm = Some(name);
            return;
        }
        n.confirm = None;
        let req = ResumeRequest { label: name.clone(), target: name, cwd: None, by_name: true };
        effects.push(Effect::Resume { req, open: true });
    }

    /// `Effect::Attach` went out as `attach_seq`.
    pub fn attaching(&mut self, attach_seq: u64, fallback: Option<ResumeRequest>, open: bool) {
        self.sent_open(attach_seq, open);
        self.want(attach_seq);
        self.named.attaching = fallback.map(|f| (attach_seq, f));
    }

    /// `Effect::Resume` went out as `attach_seq`.
    pub fn resuming(&mut self, req: ResumeRequest, attach_seq: u64, open: bool) {
        self.sent_open(attach_seq, open);
        self.want(attach_seq);
        self.named.error = None;
        self.named.resuming = Some(Resuming { req, attach_seq });
    }

    fn sent_open(&mut self, attach_seq: u64, open: bool) {
        if open && let Some(o) = &mut self.named.opening {
            o.attach_seq = Some(attach_seq);
        }
    }

    /// "Name this session…" toggled: open it fresh (or close it).
    pub fn toggle_naming(&mut self) {
        let n = &mut self.named;
        n.naming = !n.naming;
        n.rename_error = None;
        n.replace = None;
    }

    /// "Name this session…" → Save: `Some` to send. Checked first; one at
    /// a time; the name it already has closes the row. Asks for an unnamed
    /// session (`Expect::Unnamed`): a named one is refused with its name,
    /// and the user is offered to replace it.
    pub fn rename_request(&mut self, typed: &str) -> Option<RenameRequest> {
        if self.named.rename.is_some() {
            return None;
        }
        let name = typed.trim();
        let same = self.active_name() == Some(name);
        let n = &mut self.named;
        n.replace = None;
        let Some(a) = &self.active else {
            n.rename_error = Some("Not attached to a session.".into());
            return None;
        };
        if let Err(e) = validate_session_name(name) {
            n.rename_error = Some(e.to_string());
            return None;
        }
        n.rename_error = None;
        if same {
            n.naming = false;
            return None;
        }
        n.rename = Some(PendingRename { session_id: a.session_id.clone(), name: name.to_string() });
        Some(RenameRequest { session_id: a.session_id.clone(), name: name.to_string(), expect: Expect::Unnamed })
    }

    /// The Replace offer was taken: to the session it was made for, and
    /// only if that session is still named what the user saw.
    pub fn replace_request(&mut self) -> Option<RenameRequest> {
        if self.named.rename.is_some() {
            return None;
        }
        let r = self.named.replace.take()?;
        self.named.rename = Some(PendingRename { session_id: r.session_id.clone(), name: r.asked.clone() });
        Some(RenameRequest { session_id: r.session_id, name: r.asked, expect: Expect::Named(r.current) })
    }

    /// The rename could not be sent at all.
    pub fn rename_not_sent(&mut self, why: String) {
        self.named.rename = None;
        self.named.rename_error = Some(why);
    }

    /// The user asked for an attachment (`Client::attach`/`create`/`resume`
    /// returned `attach_seq`). Whatever the previous one left in the Named
    /// section (a resume, an Open by name, the naming row and its Replace
    /// offer) belonged to it and goes; a rename already sent still gets
    /// its answer.
    pub fn want(&mut self, attach_seq: u64) {
        self.wanted = Some(attach_seq);
        self.wanted_session = None;
        self.wanted_fact = None;
        self.rail_gen += 1;
        let n = &mut self.named;
        n.resuming.take_if(|r| r.attach_seq != attach_seq);
        n.attaching.take_if(|(s, _)| *s != attach_seq);
        n.opening.take_if(|o| o.attach_seq != Some(attach_seq));
        n.naming = false;
        n.replace = None;
        n.rename_error = None;
    }

    /// The attach just asked for (after [`Model::want`]) is of this session.
    pub fn asked_for(&mut self, session_id: &str) {
        self.wanted_session = Some(session_id.to_string());
        self.wanted_fact = self.sessions.iter().find(|s| s.id == session_id).map(|s| self.fact(s));
    }

    /// The last attach asked for this session (attached or on its way): a
    /// click on it again has nothing to do.
    pub fn is_wanted(&self, session_id: &str) -> bool {
        self.wanted_session.as_deref() == Some(session_id)
    }

    /// A local session (benches): its events carry [`LOCAL_SEQ`].
    pub fn set_local_session(&mut self, session_id: &str) {
        self.wanted = Some(LOCAL_SEQ);
        self.wanted_session = Some(session_id.into());
        self.epoch += 1;
        self.active = Some(Active {
            session_id: session_id.into(),
            attach_seq: LOCAL_SEQ,
            epoch: self.epoch,
            connection: 0,
            mode: AttachMode::Mirror,
        });
        self.stats = Stats::default();
    }

    /// Attached, connected and allowed to drive, and no switch under way:
    /// while another attachment is on its way (`wanted` is not the active
    /// one), input, answers and HALT would reach that one, not the session
    /// on screen (the connection thread routes them to its latest attach).
    pub fn can_send(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.mode != AttachMode::Observe && self.wanted == Some(a.attach_seq)) && !self.reattaching
    }

    /// A switch to another attachment is under way.
    pub fn switching(&self) -> bool {
        self.wanted.is_some() && self.wanted != self.active.as_ref().map(|a| a.attach_seq)
    }

    /// The prompt the authorization card shows, as `(epoch, prompt id)`:
    /// the attached session's first pending one.
    pub fn card_key(&self) -> Option<(u64, u64)> {
        Some((self.active.as_ref()?.epoch, self.transcript.pending_prompts().first()?.id))
    }

    /// Prompt `id` of the session on screen may be answered now.
    pub fn may_answer(&self, prompt_id: u64) -> bool {
        self.can_send() && !self.prompt_answered(prompt_id) && self.transcript.pending_prompts().iter().any(|p| p.id == prompt_id)
    }

    /// An answer to prompt `id` of the session on screen went out.
    pub fn prompt_answered(&self, prompt_id: u64) -> bool {
        self.active.as_ref().is_some_and(|a| self.answered.contains(&(a.epoch, prompt_id)))
    }

    /// The answer to prompt `id` went to the daemon.
    pub fn answer_sent(&mut self, prompt_id: u64) {
        if let Some(a) = &self.active {
            self.answered.insert((a.epoch, prompt_id));
        }
    }

    /// `text` went to the daemon: the turn it starts (or steers) is the
    /// user's own, so its end is not UNREAD.
    pub fn sent(&mut self, text: String) {
        self.in_flight.push_back(text);
        if let Some(a) = &self.active
            && !self.own_turns.contains(&a.session_id)
        {
            self.own_turns.insert(a.session_id.clone());
        }
    }

    /// The session on screen as the user sees it (its transcript): the
    /// baseline UNREAD compares the next poll with. Its own turn is over
    /// once nothing sent is unconfirmed and it is idle.
    fn saw_active(&mut self) {
        let Some(a) = &self.active else { return };
        if self.transcript.session_id() != Some(a.session_id.as_str()) {
            return;
        }
        let now = (self.transcript.history_len(), self.transcript.busy());
        match self.seen.get_mut(&a.session_id) {
            Some(e) => *e = now,
            None => {
                self.seen.insert(a.session_id.clone(), now);
            }
        }
        if self.in_flight.is_empty() && !now.1 {
            self.own_turns.remove(&a.session_id);
        }
    }

    /// UNREAD marks of sessions in no list (closed, or past what is listed).
    fn prune_unread(&mut self) {
        if self.unread.is_empty() {
            return;
        }
        let (sessions, n) = (&self.sessions, &self.named);
        self.unread.retain(|k| sessions.iter().any(|s| s.is(k)) || n.saved.iter().chain(&n.recent).any(|v| v.id == *k));
    }

    fn current(&self, attach_seq: u64) -> bool {
        self.wanted == Some(attach_seq)
    }

    /// A refusal: the oldest unconfirmed text was refused (the daemon
    /// handles input in order, and earlier echoes confirmed the rest).
    fn give_back_oldest(&mut self, effects: &mut Vec<Effect>) {
        if let Some(t) = self.in_flight.pop_front() {
            effects.push(Effect::Restore(t));
        }
    }

    fn give_back_all(&mut self, effects: &mut Vec<Effect>) {
        if !self.in_flight.is_empty() {
            effects.push(Effect::Restore(Vec::from(std::mem::take(&mut self.in_flight)).join("\n")));
        }
    }

    /// Apply one event.
    pub fn apply(&mut self, ev: Event) -> Vec<Effect> {
        let mut effects = Vec::new();
        let ev = match ev {
            Event::Sessions(mut s) => {
                sort_sessions(&mut s);
                // The poll of an idle daemon: nothing to rebuild or draw.
                if s == self.sessions {
                    return effects;
                }
                Event::Sessions(s)
            }
            ev => ev,
        };
        self.view_dirty = true;
        if !matches!(ev, Event::Session { .. } | Event::History { .. }) {
            self.rail_gen += 1;
        }
        match ev {
            Event::Status(s) => {
                // A reply in flight died with the old connection.
                self.named.saved_in_flight = None;
                self.named.saved_queue.clear();
                if let Status::Connected { features, profile, .. } = &s {
                    self.named.daemon = match profile {
                        Some(p) => format!("the daemon's machine (profile {p})"),
                        None => "the daemon's machine".into(),
                    };
                    self.named.saved_supported = features.iter().any(|f| f == FEATURE_SAVED_SESSIONS);
                    self.named.named_stale = true;
                    if self.named.saved_supported {
                        self.refresh_saved(&mut effects);
                    } else {
                        self.clear_saved();
                    }
                }
                self.conn = match s {
                    Status::Connecting => ConnState::Connecting,
                    Status::Connected { daemon_version, profile, .. } => {
                        ConnState::Connected(format!("{}{}", daemon_version, profile.map(|p| format!(" · {p}")).unwrap_or_default()))
                    }
                    Status::Reconnecting { .. } => ConnState::Reconnecting,
                    Status::Offline(e) => ConnState::Off(e),
                    Status::Incompatible(e) => ConnState::Incompatible(e.to_string()),
                    _ => ConnState::Reconnecting,
                }
            }
            Event::Sessions(s) => {
                self.saw_active();
                // UNREAD: off screen, a turn ended or messages came that the
                // user has not seen. Not news: a turn this window sent, and
                // a session a TUI is attached to (it is on a screen there).
                for new in &s {
                    if self.is_active(&new.id) {
                        continue;
                    }
                    let own = self.own_turns.contains(&new.id);
                    if own && !new.busy {
                        self.own_turns.remove(&new.id);
                    }
                    let elsewhere = new.roster.iter().any(|c| c.kind == "tui");
                    if let Some(&(messages, busy)) = self.seen.get(&new.id)
                        && !own
                        && !elsewhere
                        && ((busy && !new.busy) || new.messages > messages)
                    {
                        self.unread.insert(new.id.clone());
                        if !new.journal_id.is_empty() {
                            self.unread.insert(new.journal_id.clone());
                        }
                    }
                    match self.seen.get_mut(&new.id) {
                        Some(e) => *e = (new.messages, new.busy),
                        None => {
                            self.seen.insert(new.id.clone(), (new.messages, new.busy));
                        }
                    }
                }
                self.seen.retain(|id, _| s.iter().any(|n| n.id == *id));
                // A row leaving OPEN (closed, or evicted to history).
                let left = self.sessions.iter().any(|o| !s.iter().any(|n| n.id == o.id));
                self.sessions = s;
                if left {
                    self.refresh_saved(&mut effects);
                }
                if !self.named.saved_supported {
                    self.prune_unread();
                }
            }
            Event::SavedSessions { sessions, truncated, rejected } => {
                let n = &mut self.named;
                let asked = n.saved_in_flight.take();
                n.saved_note = (rejected > 0).then(|| format!("{rejected} saved sessions could not be read (skipped)."));
                if asked.is_some() {
                    n.saved_failed = false;
                }
                match asked {
                    Some(SavedAsk::Named) => {
                        n.saved = sessions;
                        n.saved_truncated = truncated;
                        n.named_stale = false;
                    }
                    Some(SavedAsk::Recent(_)) => {
                        n.recent = sessions;
                        n.recent_truncated = truncated;
                        n.recent_loaded = true;
                        if !truncated {
                            // Every saved session, the named ones too.
                            n.saved.clear();
                            n.saved_truncated = false;
                            n.named_stale = false;
                        } else if n.named_stale && !n.saved_queue.contains(&SavedAsk::Named) {
                            n.saved_queue.push_back(SavedAsk::Named);
                        }
                    }
                    // Asked on a connection since dropped.
                    None => {}
                }
                self.pump_saved(&mut effects);
                // The refresh is done: an Open by name waiting for it, and
                // UNREAD marks of sessions in no list.
                if !self.saved_listing() {
                    self.resolve_open(&mut effects);
                    self.prune_unread();
                }
            }
            Event::SavedSessionsFailed { reason } => self.on_saved_failed(&reason, &mut effects),
            Event::Renamed { session_id, name, .. } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.name = name;
                }
                if self.named.rename.as_ref().is_some_and(|r| r.session_id == session_id) {
                    self.renamed();
                }
                effects.push(Effect::ListSessions);
                self.named.named_stale = true;
                self.refresh_saved(&mut effects);
            }
            Event::RenameFailed(f) => {
                let asked = match self.named.rename.take() {
                    Some(r) if r.session_id == f.session_id => r.name,
                    other => {
                        self.named.rename = other;
                        return effects;
                    }
                };
                // Already named what was asked (a double Save): done.
                if f.reason == RenameRefusal::Named && f.current_name.as_deref() == Some(asked.as_str()) {
                    self.renamed();
                    effects.push(Effect::ListSessions);
                    return effects;
                }
                let msg = crate::named::rename_message(&f, &asked);
                if self.active.as_ref().is_some_and(|a| a.session_id == f.session_id) {
                    // Shown in the naming row, opened again if it was closed.
                    let n = &mut self.named;
                    n.naming = true;
                    n.rename_error = Some(msg);
                    if f.reason == RenameRefusal::Named
                        && let Some(current) = f.current_name.clone()
                    {
                        n.replace = Some(Replace { session_id: f.session_id.clone(), asked, current });
                    }
                } else {
                    // The user has moved on: say it where it stays visible.
                    self.named.error = Some(format!("Naming a session '{asked}': {msg}"));
                }
            }
            Event::CwdFallback { attach_seq, missing, .. } if self.current(attach_seq) => {
                self.named.notice = Some(format!("Folder {missing} is gone on {}; resumed in ~.", self.named.daemon));
            }
            Event::Attached { attach_seq, attached, mode, .. } if self.current(attach_seq) => {
                if let Some(r) = self.named.resuming.take_if(|r| r.attach_seq == attach_seq) {
                    // The daemon's resolver took a chain of that name, or
                    // the id containing it (after the user was told): it
                    // stays on screen, said so, never detached and left
                    // loaded behind the user's back.
                    if r.req.by_name && attached.meta.name.as_deref() != Some(r.req.label.as_str()) {
                        self.named.notice = Some(format!(
                            "No session is named '{}': the daemon opened {} (a chain of that name, or the id containing it).",
                            r.req.label,
                            attached.meta.label()
                        ));
                    } else if r.req.by_name {
                        self.named.notice =
                            Some(format!("Opened '{}' by name: this daemon does not list saved folders, so it runs in ~.", r.req.label));
                    }
                    self.refresh_saved(&mut effects);
                }
                // The one on screen until now, as the user saw it.
                self.saw_active();
                // Opened: read.
                self.unread.remove(&attached.meta.id);
                self.unread.remove(&attached.meta.journal_id);
                self.named.attaching.take_if(|(s, _)| *s == attach_seq);
                if self.named.opening.take_if(|o| o.attach_seq == Some(attach_seq)).is_some() {
                    self.named.opened = true;
                }
                self.named.error = None;
                // A new epoch even on a reconnect (see `Active::epoch`).
                self.epoch += 1;
                self.active = Some(Active {
                    session_id: attached.meta.id.clone(),
                    attach_seq,
                    epoch: self.epoch,
                    connection: attached.connection,
                    mode,
                });
                self.wanted_session = Some(attached.meta.id.clone());
                // Whatever was answered belongs to the attachment before.
                self.answered.clear();
                self.reattaching = false;
                self.last_error = None;
                if let Err(e) = self.transcript.load_attached(&attached) {
                    self.last_error = Some(e.to_string());
                }
                self.stats.on_attached(&attached);
                effects.push(Effect::ListSessions);
            }
            Event::Reattaching { attach_seq, .. } if self.current(attach_seq) => self.reattaching = true,
            Event::Session { attach_seq, event, raw, .. } if self.current(attach_seq) => {
                self.stats.on_session(&event, &raw);
                match &event {
                    SessionEvent::TurnStarted { user_text: Some(t), .. } | SessionEvent::Steered { text: t, .. } => {
                        if let Some(i) = self.in_flight.iter().position(|s| s.trim() == t.trim()) {
                            self.in_flight.remove(i);
                        }
                    }
                    _ => {}
                }
                self.transcript.apply(&event);
                if !self.answered.is_empty() {
                    let pending = self.transcript.pending_prompts();
                    self.answered.retain(|(_, id)| pending.iter().any(|p| p.id == *id));
                }
            }
            Event::History { attach_seq, update, .. } if self.current(attach_seq) => {
                if self.transcript.apply_update(&update).is_err() {
                    effects.push(Effect::Resync);
                } else {
                    self.stats.on_history(&update);
                }
            }
            Event::Detached { attach_seq, reason, .. } if self.current(attach_seq) => {
                self.saw_active();
                self.active = None;
                self.wanted_session = None;
                self.answered.clear();
                self.reattaching = false;
                self.transcript.set_detached();
                self.stats.on_detached();
                self.give_back_all(&mut effects);
                let n = &mut self.named;
                // A named live session that ended since the last list:
                // resume its journal (an Open by name keeps waiting).
                if let Some((_, req)) = n.attaching.take_if(|(s, _)| *s == attach_seq)
                    && reason == UNKNOWN_SESSION
                {
                    let open = n.opening.as_mut().filter(|o| o.attach_seq == Some(attach_seq)).map(|o| o.attach_seq = None).is_some();
                    effects.push(Effect::Resume { req, open });
                    return effects;
                }
                let opening = n.opening.take_if(|o| o.attach_seq == Some(attach_seq));
                match n.resuming.take_if(|r| r.attach_seq == attach_seq) {
                    Some(r) => n.error = Some(crate::named::resume_error(&r.req.label, &reason)),
                    None => match opening {
                        Some(o) => n.error = Some(format!("Could not open '{}': {reason}", o.name)),
                        None => self.last_error = Some(format!("detached: {reason}")),
                    },
                }
            }
            // The saved list's own failures stay in the Named section.
            Event::Error { attach_seq: None, error: Error::Unsupported { .. } } => {
                self.clear_saved();
                let n = &mut self.named;
                if n.opening.as_ref().is_some_and(|o| o.attach_seq.is_none()) {
                    self.open_by_resolver(&mut effects);
                }
            }
            Event::Error { attach_seq, error } if attach_seq.is_none_or(|s| self.current(s)) => {
                // An answer the daemon may not have got (a refusal that names
                // no command could be one): the card asks again. A repeat is
                // harmless, the daemon takes the first answer per prompt.
                let answer = matches!(&error, Error::Refused { command: Some(c), .. } if c == "answer")
                    || matches!(&error, Error::NotAttached { command: "answer" });
                if answer || matches!(&error, Error::Refused { command: None, .. }) {
                    self.answered.clear();
                }
                let input = !answer && matches!(&error, Error::Refused { command: None, .. } | Error::NotAttached { .. })
                    || matches!(&error, Error::Refused { command: Some(c), .. } if c == "submit" || c == "steer");
                if input {
                    self.give_back_oldest(&mut effects);
                }
                self.last_error = Some(error.to_string());
            }
            _ => {}
        }
        effects
    }

    /// A daemon without `saved_sessions`: no lists, nothing in flight.
    fn clear_saved(&mut self) {
        let n = &mut self.named;
        n.saved_supported = false;
        n.saved_in_flight = None;
        n.saved_queue.clear();
        n.saved.clear();
        n.recent.clear();
        n.saved_truncated = false;
        n.recent_truncated = false;
        n.recent_loaded = false;
        n.saved_note = None;
    }

    /// The pending rename landed (or was already so): close the row.
    fn renamed(&mut self) {
        let n = &mut self.named;
        n.rename = None;
        n.rename_error = None;
        n.replace = None;
        n.naming = false;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use synaps_client::protocol::{DaemonFrame, LlmEvent, StreamEvent, decode_frame};
    use synaps_client::{Attached, HistoryUpdate};

    use super::*;

    fn attached(id: &str) -> Box<Attached> {
        let v = json!({"type": "attached", "client": 2, "meta": {"id": id}, "conversation": {"api_messages": []}, "input_owner": 2});
        match synaps_client::protocol::decode_frame(&v.to_string()).unwrap() {
            synaps_client::protocol::DaemonFrame::Attached(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn session(attach_seq: u64, event: SessionEvent) -> Event {
        Event::Session { attach_seq, session_id: "s".into(), seq: 0, event, raw: Arc::new(Value::Null) }
    }

    fn attach(m: &mut Model, seq: u64) {
        m.want(seq);
        m.apply(Event::Attached { attach_seq: seq, attached: attached("s"), mode: AttachMode::Mirror, reconnect: false });
    }

    #[test]
    fn states_and_the_attached_sessions_own_state() {
        use crate::rail::Chip;
        let mk = |id: &str, busy: bool, awaiting: usize, lifecycle: &str| -> SessionMeta {
            serde_json::from_value(json!({"id": id, "busy": busy, "awaiting_input": awaiting, "lifecycle": lifecycle})).unwrap()
        };
        let mut m = Model::new(ConnState::Connecting);
        m.sessions = vec![mk("s", true, 1, "live"), mk("b", true, 0, "live"), mk("w", false, 2, "live"), mk("p", false, 0, "parked")];
        let states: Vec<State> = m.sessions.iter().map(|s| m.state(s)).collect();
        assert_eq!(states, [State::NeedsYou, State::Running, State::NeedsYou, State::Parked]);
        assert_eq!(m.attention().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s", "w"]);
        // Attached to "s": its transcript says idle, nothing pending (the
        // poll has not caught up yet).
        attach(&mut m, 1);
        assert!(m.is_active("s"));
        assert_eq!(m.state(&m.sessions[0]), State::Idle);
        assert_eq!(m.attention().count(), 1);
        assert_eq!(m.active_meta().map(|s| s.id.as_str()), Some("s"));
        for (c, admitted) in [(Chip::NeedsYou, vec!["w"]), (Chip::Running, vec!["b"]), (Chip::Unread, vec![])] {
            let got: Vec<&str> = m.sessions.iter().filter(|s| m.chip_shows(c, s)).map(|s| s.id.as_str()).collect();
            assert_eq!(got, admitted, "{c:?}");
        }
    }

    /// The 3-second poll of an idle daemon changes nothing: no rail
    /// rebuild, nothing to draw. The same list in another order is the
    /// same list (the daemon sends hash-map order; OPEN is newest first).
    #[test]
    fn an_unchanged_poll_is_free() {
        let mut m = Model::new(ConnState::Connecting);
        let list = json!([{"id": "a", "lifecycle": "live", "created_at": "2026-11-13T10:00:00Z"},
                          {"id": "b", "lifecycle": "live", "created_at": "2026-11-13T11:00:00.5Z"}]);
        m.apply(Event::Sessions(metas(list.clone())));
        assert_eq!(m.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b", "a"], "newest first");
        let generation = m.rail_gen;
        m.view_dirty = false;
        let mut reversed = metas(list.clone());
        reversed.reverse();
        assert!(m.apply(Event::Sessions(reversed)).is_empty());
        assert_eq!(m.rail_gen, generation, "no rebuild");
        assert!(!m.view_dirty, "nothing to draw");
        let mut busy = metas(list);
        busy[0].busy = true;
        m.apply(Event::Sessions(busy));
        assert!(m.rail_gen > generation && m.view_dirty);
        assert_eq!(m.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b", "a"], "a state change moves nothing");
    }

    /// UNREAD is the client's: the poll saw a turn end, or messages rise,
    /// while the session was not on screen. Opening it clears it; the one
    /// on screen never is; evicted to history it stays (by journal id).
    #[test]
    fn unread_is_kept_by_the_client() {
        let mut m = Model::new(ConnState::Connecting);
        let list = |busy: bool, messages: usize| {
            metas(json!([{"id": "s", "lifecycle": "live", "busy": busy, "messages": messages},
                         {"id": "u", "lifecycle": "live", "busy": busy, "messages": messages, "journal_id": "j-u"}]))
        };
        let st = |m: &Model, id: &str| m.state(m.sessions.iter().find(|s| s.id == id).unwrap());
        m.apply(Event::Sessions(list(true, 2)));
        attach(&mut m, 1);
        m.apply(Event::Sessions(list(false, 2)));
        assert_eq!(st(&m, "u"), State::Unread, "its turn ended off screen");
        assert_eq!(st(&m, "s"), State::Idle, "on screen");
        let saved: SavedSession = serde_json::from_value(json!({"id": "j-u", "live": false})).unwrap();
        assert_eq!(m.saved_state(&saved), State::Unread);
        m.want(2);
        let a = json!({"type": "attached", "client": 2, "meta": {"id": "u", "journal_id": "j-u"}, "conversation": {"api_messages": []}});
        let DaemonFrame::Attached(a) = decode_frame(&a.to_string()).unwrap() else { panic!() };
        m.apply(attached_ev(2, a));
        assert_eq!(st(&m, "u"), State::Idle, "opened: read");
        assert_eq!(m.saved_state(&saved), State::Saved);
        m.apply(Event::Sessions(list(false, 3)));
        assert_eq!(st(&m, "s"), State::Unread, "new messages while s is off screen");
        assert_eq!(st(&m, "u"), State::Idle);
    }

    /// The ids of the held rows a chip shows, in order.
    fn under(m: &Model, chip: Chip) -> Vec<String> {
        use crate::rail::{Item, View, build};
        let rail = build(m, &View { chip: Some(chip), ..View::default() }, 0);
        rail.items
            .iter()
            .filter_map(|i| match i {
                Item::Row(r) => match &rail.rows[*r].action {
                    RowAction::Attach(id) => Some(id.clone()),
                    RowAction::Resume(_) => None,
                },
                _ => None,
            })
            .collect()
    }

    fn three(x_busy: bool, r_busy: bool) -> Vec<SessionMeta> {
        metas(json!([
            {"id": "x", "lifecycle": "live", "awaiting_input": 1, "busy": x_busy, "created_at": "2026-11-13T12:00:00Z"},
            {"id": "y", "lifecycle": "live", "awaiting_input": 1, "busy": true, "created_at": "2026-11-13T11:00:00Z"},
            {"id": "r", "lifecycle": "live", "busy": r_busy, "created_at": "2026-11-13T10:00:00Z"},
        ]))
    }

    /// H1: OPENING is the glyph only. Under a chip a clicked row stays
    /// where it was until its attach lands, so the second click of a
    /// double-click hits it again (a no-op), and no count dips meanwhile.
    #[test]
    fn a_clicked_row_stays_put_under_its_chip() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        m.apply(Event::Sessions(three(true, true)));
        assert_eq!(under(&m, Chip::NeedsYou), ["x", "y"]);
        assert_eq!(m.open_row(&RowAction::Attach("x".into())).len(), 1);
        m.attaching(4, None, false);
        m.asked_for("x");
        assert_eq!(m.state(&m.sessions[0]), State::Opening, "the glyph says so");
        assert_eq!(under(&m, Chip::NeedsYou), ["x", "y"], "x keeps its place");
        assert_eq!(m.attention().count(), 2, "the counts do not dip");
        let second = under(&m, Chip::NeedsYou)[0].clone();
        assert!(m.open_row(&RowAction::Attach(second)).is_empty(), "the second click: x again, nothing to do");
        // Under RUNNING: r's turn ends while its attach is on its way; it
        // stays until the attach lands, then goes.
        assert_eq!(under(&m, Chip::Running), ["r"]);
        assert_eq!(m.open_row(&RowAction::Attach("r".into())).len(), 1);
        m.attaching(5, None, false);
        m.asked_for("r");
        m.apply(Event::Sessions(three(true, false)));
        assert_eq!(under(&m, Chip::Running), ["r"], "on its way: stays");
        m.apply(attached_ev(5, attached("r")));
        assert!(under(&m, Chip::Running).is_empty(), "landed, idle: it goes");
    }

    /// H1: JUMP clicked twice sends one attach, to the same session; once
    /// it lands, JUMP moves on to the next one waiting.
    #[test]
    fn jump_twice_attaches_once() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        m.apply(Event::Sessions(three(true, true)));
        let mut sent = Vec::new();
        for _ in 0..2 {
            let target = m.jump_target().unwrap().to_string();
            for fx in m.open_row(&RowAction::Attach(target)) {
                if let Effect::Attach { session_id, .. } = &fx {
                    m.attaching(4, None, false);
                    m.asked_for(session_id);
                }
                sent.push(fx);
            }
        }
        assert!(matches!(sent.as_slice(), [Effect::Attach { session_id, .. }] if session_id == "x"), "{sent:?}");
        assert_eq!(m.jump_target(), Some("x"), "the same target while on its way");
        assert_eq!(m.ticker().map(|s| s.id.as_str()), Some("x"));
        m.apply(attached_ev(4, attached("x")));
        assert_eq!(m.jump_target(), Some("y"));
    }

    /// M1: Enter in find. An exact name in the lists first; else the one
    /// row showing; several rows: the user picks; no row: the daemon. With
    /// the saved list it is asked once more, then "no session named".
    /// Without it, the daemon's resolver gets at once only text no id can
    /// contain; other text goes on a second Enter, after the user was told.
    #[test]
    fn enter_in_find() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        m.apply(Event::Sessions(metas(json!([
            {"id": "l1", "name": "kite", "lifecycle": "live"},
            {"id": "l2", "name": "kite-notes", "lifecycle": "live"},
        ]))));
        let all = json!([{"id": "j-axel", "name": "axel", "live": false}, {"id": "j-t", "title": "Smoke tests", "live": false}]);
        lists(&mut m, all.clone());
        let at = |id: &str| RowAction::Attach(id.into());
        let fx = m.find_enter("kite", &[at("l1"), at("l2")]);
        assert!(matches!(fx.as_slice(), [Effect::Attach { session_id, open: true, .. }] if session_id == "l1"), "exact: {fx:?}");
        let fx = m.find_enter("axel", &[]);
        assert!(matches!(fx.as_slice(), [Effect::Resume { req, open: true }] if req.target == "j-axel"), "exact, saved: {fx:?}");
        let smoke = req("Smoke tests", "j-t", None);
        assert_eq!(m.find_enter("smoke", &[RowAction::Resume(smoke.clone())]), [Effect::Resume { req: smoke, open: false }], "single");
        assert!(m.find_enter("kit", &[at("l1"), at("l2")]).is_empty(), "ambiguous");
        assert_eq!(m.named.error.as_deref(), Some("2 sessions match 'kit': click one, or type its exact name."));
        assert_eq!(m.find_enter("ghost", &[]), [ASK_RECENT], "none: asked once more");
        assert!(m.apply(saved_list(all)).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(m.find_enter("Axel Memory", &[]).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("Nothing matches 'Axel Memory'."));

        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["owned_sessions"]));
        let by_name = |n: &str| Effect::Resume { req: ResumeRequest { by_name: true, ..req_(n) }, open: true };
        assert_eq!(m.find_enter("ghost", &[]), [by_name("ghost")], "no id can contain it");
        assert!(m.find_enter("cafe", &[]).is_empty(), "could be part of an id");
        assert!(m.named.error.as_deref().unwrap().contains("Press Enter again"));
        assert_eq!(m.find_enter("cafe", &[]), [by_name("cafe")], "told, and Enter again");
        assert!(m.find_enter("add", &[]).is_empty());
        assert!(m.find_enter("cafe", &[]).is_empty(), "another text in between: told again");
    }

    /// M2: UNREAD is what the user has not seen. The session on screen is
    /// seen as its transcript has it (ahead of the poll): moving away right
    /// after its turn ends marks nothing. A turn this window sent never
    /// marks; nor does one in a session a TUI shows (read there).
    #[test]
    fn unread_is_what_was_not_seen() {
        let mut m = Model::new(ConnState::Connecting);
        let list = |busy: bool, messages: usize, roster: Value| {
            metas(json!([{"id": "s", "lifecycle": "live", "busy": busy, "messages": messages, "roster": roster},
                         {"id": "b", "lifecycle": "live"}]))
        };
        let st = |m: &Model| m.state(m.sessions.iter().find(|s| s.id == "s").unwrap());
        let history = |seq: u64, base_len: usize, n: usize| Event::History {
            attach_seq: seq,
            session_id: "s".into(),
            seq: 0,
            update: HistoryUpdate::Append {
                base_len,
                messages: (0..n).map(|i| json!({"role": "assistant", "content": format!("m{i}")})).collect(),
            },
        };
        let turn = |text: Option<&str>| SessionEvent::TurnStarted {
            turn_baseline: 0,
            trigger: "user".into(),
            user_text: text.map(str::to_string),
        };
        m.apply(Event::Sessions(list(true, 0, json!([]))));
        attach(&mut m, 1);
        // Another client's turn on s, watched to its end; the poll lags.
        m.apply(session(1, turn(None)));
        m.apply(history(1, 0, 2));
        m.apply(session(1, SessionEvent::Idle));
        m.want(2);
        m.apply(attached_ev(2, attached("b")));
        m.apply(Event::Sessions(list(false, 2, json!([]))));
        assert_eq!(st(&m), State::Idle, "seen on screen, left within the poll");
        // Off screen, another client adds a message: news.
        m.apply(Event::Sessions(list(false, 3, json!([]))));
        assert_eq!(st(&m), State::Unread);

        // The user's own turn, left running: its end is not news.
        attach(&mut m, 3);
        m.sent("do it".into());
        m.apply(session(3, turn(Some("do it"))));
        m.want(4);
        m.apply(attached_ev(4, attached("b")));
        m.apply(Event::Sessions(list(true, 5, json!([]))));
        m.apply(Event::Sessions(list(false, 7, json!([]))));
        assert_eq!(st(&m), State::Idle, "own turn");
        // A turn driven from a TUI ends: read there.
        let tui = json!([{"id": 9, "name": "tui", "kind": "tui", "mode": "mirror", "attached_at_ms": 0}]);
        m.apply(Event::Sessions(list(true, 7, tui.clone())));
        m.apply(Event::Sessions(list(false, 9, tui)));
        assert_eq!(st(&m), State::Idle, "a TUI shows it");
        // The next one, no TUI, not ours: news again.
        m.apply(Event::Sessions(list(true, 9, json!([]))));
        m.apply(Event::Sessions(list(false, 11, json!([]))));
        assert_eq!(st(&m), State::Unread);
    }

    /// L10: a resume is a held session's OPENING by id or journal, and by
    /// name only when it was asked by name.
    #[test]
    fn a_resume_by_id_is_not_a_held_namesake() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        m.apply(Event::Sessions(metas(json!([{"id": "l1", "name": "notes", "lifecycle": "live"}]))));
        m.resuming(req("notes", "j-other", None), 3, false);
        assert_eq!(m.state(&m.sessions[0]), State::Idle);
        m.resuming(ResumeRequest { by_name: true, ..req_("notes") }, 4, true);
        assert_eq!(m.state(&m.sessions[0]), State::Opening);
    }

    /// L8: an UNREAD mark follows the journal (resumed elsewhere under a new
    /// id, still unread) and goes when the session is in no list.
    #[test]
    fn unread_follows_the_journal() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        let one = |id: &str, busy: bool| metas(json!([{"id": id, "journal_id": "j-u", "lifecycle": "live", "busy": busy}]));
        m.apply(Event::Sessions(one("u", true)));
        m.apply(Event::Sessions(one("u", false)));
        assert_eq!(m.state(&m.sessions[0]), State::Unread);
        m.apply(Event::Sessions(one("u2", false)));
        assert_eq!(m.state(&m.sessions[0]), State::Unread, "a new id, the same journal");
        m.apply(Event::Sessions(Vec::new()));
        assert!(m.unread.is_empty(), "in no list: pruned");
    }

    /// H4: a window that never renders applies every event as it comes;
    /// the only state is the transcript, which holds one history, however
    /// many rounds (each a full-history refold) arrive meanwhile.
    #[test]
    fn events_apply_without_rendering_and_memory_stays_bounded() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        for round in 0..400 {
            let messages: Vec<Value> = (0..50)
                .map(|i| json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": format!("round {round} msg {i}")}))
                .collect();
            m.apply(session(1, SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: "delta ".repeat(20) }) }));
            m.apply(Event::History { attach_seq: 1, session_id: "s".into(), seq: 0, update: HistoryUpdate::Replace { messages } });
        }
        assert_eq!(m.transcript.len(), 50);
        assert_eq!(m.transcript.items()[0].text, "round 399 msg 0");
    }

    /// H2: refused input goes back to the composer; accepted input does not.
    #[test]
    fn refused_input_is_given_back() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        m.sent("hello there".into());
        let refused = Error::Refused { command: Some("submit".into()), reason: "input owned by #1".into() };
        assert_eq!(m.apply(Event::Error { attach_seq: Some(1), error: refused }), vec![Effect::Restore("hello there".into())]);
        assert!(m.last_error.as_deref().unwrap().contains("input owned by #1"));
        // Accepted: TurnStarted echoes it; a later refusal of something else
        // gives nothing back.
        m.sent("second".into());
        m.apply(session(1, SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some("second".into()) }));
        let cancel = Error::Refused { command: Some("cancel".into()), reason: "no".into() };
        assert!(m.apply(Event::Error { attach_seq: Some(1), error: cancel }).is_empty());
        // A reload-drain refusal (`Error` frame, command unknown) gives back.
        m.sent("third".into());
        let drain = Error::Refused { command: None, reason: "daemon reloading; retry in a moment".into() };
        assert_eq!(m.apply(Event::Error { attach_seq: Some(1), error: drain }), vec![Effect::Restore("third".into())]);
    }

    /// N5: two sends before the first echo: the echo confirms the one it
    /// echoes (whitespace aside), a refusal gives back the oldest left, a
    /// detach everything left.
    #[test]
    fn several_sends_in_flight_are_tracked() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        for t in ["one", "two", "three"] {
            m.sent(t.into());
        }
        m.apply(session(1, SessionEvent::Steered { text: " two\n".into(), delivered: true }));
        let refused = Error::Refused { command: Some("submit".into()), reason: "busy".into() };
        assert_eq!(m.apply(Event::Error { attach_seq: Some(1), error: refused }), vec![Effect::Restore("one".into())]);
        m.sent("four".into());
        let fx = m.apply(Event::Detached { attach_seq: 1, session_id: None, reason: "gone".into() });
        assert_eq!(fx, vec![Effect::Restore("three\nfour".into())]);
    }

    /// H3: `Detached` leaves no active session (the composer is disabled)
    /// and gives unconfirmed input back; a stale `Detached` is ignored.
    #[test]
    fn detached_clears_the_session() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        attach(&mut m, 2);
        m.apply(Event::Detached { attach_seq: 1, session_id: None, reason: "replaced".into() });
        assert!(m.can_send(), "a stale Detached must not end the new attachment");
        m.apply(session(2, SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: None }));
        assert!(m.transcript.busy());
        m.sent("lost?".into());
        let fx = m.apply(Event::Detached { attach_seq: 2, session_id: Some("s".into()), reason: "session ended".into() });
        assert_eq!(fx, vec![Effect::Restore("lost?".into())]);
        assert!(m.active.is_none() && !m.can_send());
        assert!(!m.transcript.busy());
        // Stale events of the old attachment change nothing.
        m.apply(session(1, SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some("x".into()) }));
        assert!(!m.transcript.busy());
    }

    fn prompt(id: u64) -> SessionEvent {
        synaps_client::protocol::decode_event(
            &json!({"ev": "prompt", "request": {"id": id, "kind": "confirm", "title": "t", "prompt": "p"}}),
        )
        .unwrap()
    }

    #[test]
    fn nothing_drives_while_switching_and_answers_are_per_attachment() {
        let mut m = Model::new(ConnState::Connected("x".into()));
        attach(&mut m, 1);
        m.apply(session(1, prompt(1)));
        assert!(m.can_send() && m.may_answer(1));
        assert!(!m.may_answer(2), "not pending");
        m.answer_sent(1);
        assert!(m.prompt_answered(1) && !m.may_answer(1), "sent once");
        // A switch: A's card is still on screen, nothing may answer it.
        m.want(2);
        assert!(m.switching());
        assert!(!m.can_send() && !m.may_answer(1));
        // B lands with its own prompt #1: not "answered" (that was A's).
        m.apply(Event::Attached { attach_seq: 2, attached: attached("s"), mode: AttachMode::Mirror, reconnect: false });
        m.apply(session(2, prompt(1)));
        assert!(!m.switching());
        assert!(!m.prompt_answered(1) && m.may_answer(1));
    }

    #[test]
    fn an_answer_that_did_not_arrive_lets_the_card_ask_again() {
        for error in [
            Error::Refused { command: Some("answer".into()), reason: "not connected to the session (attaching or reconnecting)".into() },
            Error::NotAttached { command: "answer" },
            Error::Refused { command: None, reason: "daemon reloading; retry in a moment".into() },
        ] {
            let mut m = Model::new(ConnState::Connected("x".into()));
            attach(&mut m, 1);
            m.apply(session(1, prompt(3)));
            m.answer_sent(3);
            assert!(!m.may_answer(3));
            m.apply(Event::Error { attach_seq: Some(1), error });
            assert!(m.may_answer(3), "the card is not stuck at 'answer sent'");
        }
        // A detach drops them too; the resolved prompt drops its own.
        let mut m = Model::new(ConnState::Connected("x".into()));
        attach(&mut m, 1);
        m.apply(session(1, prompt(4)));
        m.answer_sent(4);
        m.apply(session(1, synaps_client::protocol::decode_event(&json!({"ev": "prompt_resolved", "prompt_id": 4})).unwrap()));
        assert!(m.answered.is_empty());
        m.apply(session(1, prompt(5)));
        m.answer_sent(5);
        m.apply(Event::Detached { attach_seq: 1, session_id: None, reason: "gone".into() });
        assert!(m.answered.is_empty() && !m.may_answer(5));
        assert!(m.stats.detached, "the numbers are the last seen, not live");
    }

    #[test]
    fn asking_for_a_session_again_is_by_the_last_ask() {
        let mut m = Model::new(ConnState::Connected("x".into()));
        m.want(1);
        m.asked_for("a");
        m.apply(Event::Attached { attach_seq: 1, attached: attached("a"), mode: AttachMode::Mirror, reconnect: false });
        assert!(m.is_wanted("a"));
        // Click B, then A again before B lands: A is asked for again.
        m.want(2);
        m.asked_for("b");
        assert!(m.is_active("a") && !m.is_wanted("a"));
    }

    #[test]
    fn observers_and_reattaching_clients_cannot_send() {
        let mut m = Model::new(ConnState::Connecting);
        m.want(3);
        m.apply(Event::Attached { attach_seq: 3, attached: attached("s"), mode: AttachMode::Observe, reconnect: false });
        assert!(!m.can_send());
        attach(&mut m, 4);
        assert!(m.can_send());
        m.apply(Event::Reattaching { attach_seq: 4, session_id: "s".into(), reason: "reload".into() });
        assert!(!m.can_send());
    }

    fn connected(features: &[&str]) -> Event {
        Event::Status(Status::Connected {
            daemon_version: "0.0.0".into(),
            profile: None,
            pid: 1,
            features: features.iter().map(|f| f.to_string()).collect(),
        })
    }

    fn metas(v: Value) -> Vec<SessionMeta> {
        serde_json::from_value(v).unwrap()
    }

    fn saved_list(v: Value) -> Event {
        Event::SavedSessions { sessions: serde_json::from_value(v).unwrap(), truncated: false, rejected: 0 }
    }

    fn attached_named(id: &str, name: Option<&str>) -> Box<Attached> {
        let v = json!({"type": "attached", "client": 2, "meta": {"id": id, "name": name}, "conversation": {"api_messages": []}, "input_owner": 2});
        match decode_frame(&v.to_string()).unwrap() {
            DaemonFrame::Attached(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn attached_ev(attach_seq: u64, a: Box<Attached>) -> Event {
        Event::Attached { attach_seq, attached: a, mode: AttachMode::Mirror, reconnect: false }
    }

    fn detached(attach_seq: u64, reason: &str) -> Event {
        Event::Detached { attach_seq, session_id: None, reason: reason.into() }
    }

    fn req(label: &str, target: &str, cwd: Option<&str>) -> ResumeRequest {
        ResumeRequest { label: label.into(), target: target.into(), cwd: cwd.map(Into::into), by_name: false }
    }

    fn list_saved(fx: &[Effect]) -> bool {
        fx.iter().any(|f| matches!(f, Effect::ListSaved { .. }))
    }

    const ASK_NAMED: Effect = Effect::ListSaved { named_only: true, limit: SAVED_SESSIONS_MAX };
    const ASK_RECENT: Effect = Effect::ListSaved { named_only: false, limit: SAVED_RECENT };

    /// The newest list, whole (not cut): every saved session, named too.
    fn lists(m: &mut Model, all: Value) {
        m.apply(saved_list(all));
    }

    /// A newest list the daemon cut at the limit.
    fn cut_list(v: Value) -> Event {
        Event::SavedSessions { sessions: serde_json::from_value(v).unwrap(), truncated: true, rejected: 0 }
    }

    /// The rail's rows (every bucket open) as `(title, state, action)`.
    fn rows(m: &Model) -> Vec<(String, State, RowAction)> {
        use crate::rail::{Item, View, build};
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let rail = build(m, &View { open: 0x1f, ..View::default() }, now);
        rail.items
            .iter()
            .filter_map(|i| match i {
                Item::Row(r) | Item::Hist(r) => {
                    let row = &rail.rows[*r];
                    let st = row.live.map(|i| m.state(&m.sessions[i])).unwrap_or(row.state);
                    Some((row.title.to_string(), st, row.action.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn row_action(m: &Model, title: &str) -> RowAction {
        rows(m).into_iter().find(|r| r.0 == title).unwrap_or_else(|| panic!("no row {title}")).2
    }

    fn state_of(m: &Model, title: &str) -> State {
        rows(m).into_iter().find(|r| r.0 == title).unwrap_or_else(|| panic!("no row {title}")).1
    }

    #[test]
    fn the_saved_lists_are_asked_only_of_a_daemon_that_has_them_on_events() {
        let renamed = || Event::Renamed { session_id: "s".into(), name: Some("a".into()), in_memory_only: false };
        let one = metas(json!([{"id": "a", "lifecycle": "live"}]));
        let mut m = Model::new(ConnState::Connecting);
        assert!(!list_saved(&m.apply(connected(&["owned_sessions"]))));
        assert!(!list_saved(&m.apply(Event::Sessions(one.clone()))));
        assert!(!list_saved(&m.apply(Event::Sessions(Vec::new()))), "a row left OPEN, but there is no saved list");
        let fx = m.apply(renamed());
        assert!(!list_saved(&fx) && fx.contains(&Effect::ListSessions));
        assert!(rows(&m).is_empty());

        let mut m = Model::new(ConnState::Connecting);
        assert_eq!(m.apply(connected(&["saved_sessions"])), [ASK_RECENT], "the newest 200 first");
        assert!(m.apply(Event::Sessions(one.clone())).is_empty(), "one request at a time");
        assert_eq!(m.apply(cut_list(json!([]))), [ASK_NAMED], "cut: the named list too, once per connection");
        assert!(m.apply(saved_list(json!([]))).is_empty());
        for _ in 0..20 {
            let mut changed = one.clone();
            changed[0].messages += 1;
            assert!(!list_saved(&m.apply(Event::Sessions(changed))), "never on the poll");
        }
        assert_eq!(m.apply(Event::Sessions(Vec::new())), [ASK_RECENT], "a row left OPEN");
        assert!(m.apply(cut_list(json!([]))).is_empty(), "cut, but the named list is fresh: one daemon read");
        assert_eq!(m.apply(renamed()), [Effect::ListSessions, ASK_RECENT], "after a rename");
        // Another rename meanwhile: asked again after, never twice at once.
        assert!(!list_saved(&m.apply(renamed())));
        assert_eq!(m.apply(cut_list(json!([]))), [ASK_RECENT], "the second rename's");
        assert_eq!(m.apply(cut_list(json!([]))), [ASK_NAMED], "a rename makes the named list stale");
        assert!(m.apply(saved_list(json!([]))).is_empty());
        // Not cut: it holds every saved session, named too; one request.
        assert_eq!(m.apply(renamed()), [Effect::ListSessions, ASK_RECENT]);
        assert!(m.apply(saved_list(json!([{"id": "j", "name": "kept", "live": false}]))).is_empty());
        assert_eq!(rows(&m).len(), 1);
        // A dropped connection drops what was in flight; an older daemon
        // behind the same socket (Unsupported) stops the asking and HISTORY.
        assert_eq!(m.apply(renamed()), [Effect::ListSessions, ASK_RECENT]);
        m.apply(Event::Status(Status::Reconnecting { why: "x".into() }));
        assert!(m.apply(saved_list(json!([{"id": "j2", "name": "late", "live": false}]))).is_empty());
        assert!(!m.named.recent.iter().any(|v| v.id == "j2"), "not asked on this connection");
        assert_eq!(m.apply(connected(&["saved_sessions"])), [ASK_RECENT]);
        lists(&mut m, json!([{"id": "j", "name": "kept", "live": false}]));
        assert_eq!(rows(&m).len(), 1);
        m.apply(Event::Error { attach_seq: None, error: Error::Unsupported { feature: "saved_sessions" } });
        assert!(rows(&m).is_empty());
        assert!(m.last_error.is_none(), "not a transcript error");
        assert!(!list_saved(&m.apply(renamed())));
    }

    #[test]
    fn the_rail_groups_live_and_saved_sessions() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        m.apply(Event::Sessions(metas(json!([
            {"id": "l1", "name": "zed", "lifecycle": "live", "pinned": true},
            {"id": "l2", "lifecycle": "live"},
            {"id": "l3", "name": "axel", "lifecycle": "live", "journal_id": "20261110-202033-e5f6", "awaiting_input": 1},
        ]))));
        let fixture =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../synaps-client/tests/fixtures/saved_session_list.json"))
                .unwrap();
        let DaemonFrame::SavedSessionList { sessions, truncated, .. } = decode_frame(fixture.trim()).unwrap() else { panic!() };
        m.apply(Event::SavedSessions { sessions, truncated, rejected: 0 });
        m.apply(saved_list(json!([])));
        let got: Vec<(String, State)> = rows(&m).into_iter().map(|(t, s, _)| (t, s)).collect();
        let want = [
            ("zed", State::Idle),
            ("axel", State::NeedsYou),
            ("New session", State::Idle),
            ("bebop", State::Saved),
            ("old-notes", State::Saved),
        ];
        assert_eq!(got, want.map(|(n, k)| (n.to_string(), k)).to_vec());
    }

    /// Resume (H1, L1, L4): the row's saved folder reaches the create as
    /// is, with `~` as the fallback; the row says "resuming…" until the
    /// attach; a folder gone on the daemon's machine is said; the daemon's
    /// real failures ("no session matching", a held journal) read clearly
    /// in the Named section, not as transcript errors; a success clears the
    /// error and asks for the saved list again.
    #[test]
    fn resuming_a_saved_session() {
        use synaps_client::Cwd;
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        lists(&mut m, json!([{"id": "j-bebop", "name": "bebop", "cwd": "/home/u/Projects/bebop", "live": false}]));
        let fx = m.open_row(&row_action(&m, "bebop"));
        let [Effect::Resume { req: bebop, open: false }] = fx.as_slice() else { panic!("{fx:?}") };
        assert_eq!(bebop.target, "j-bebop");
        let o = bebop.options();
        assert_eq!((o.cwd, o.fallback_cwd), (Cwd::Path("/home/u/Projects/bebop".into()), Some(Cwd::Home)));
        m.resuming(bebop.clone(), 7, false);
        assert_eq!(state_of(&m, "bebop"), State::Opening);
        assert!(m.open_row(&row_action(&m, "bebop")).is_empty(), "L4: a double-click resumes once");
        m.apply(Event::CwdFallback { attach_seq: 7, missing: "/home/u/Projects/bebop".into(), cwd: None });
        assert_eq!(m.named.notice.as_deref(), Some("Folder /home/u/Projects/bebop is gone on the daemon's machine; resumed in ~."));
        let fx = m.apply(attached_ev(7, attached("live-bebop")));
        assert!(m.named.resuming.is_none());
        assert!(list_saved(&fx), "the resumed session left the saved list");
        assert_eq!(m.active.as_ref().unwrap().session_id, "live-bebop");

        m.resuming(req_("ghost"), 8, false);
        m.apply(detached(8, crate::named::NO_SUCH_GHOST));
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(m.named.resuming.is_none() && m.active.is_none() && m.last_error.is_none());

        m.resuming(req("axel", "j-axel", None), 9, false);
        m.apply(detached(9, crate::named::LOCKED_AXEL));
        assert!(m.named.error.as_deref().unwrap().contains("locked by another process"));
        // A plain detach is still a transcript error; an attach clears the
        // Named section's error (L4).
        attach(&mut m, 10);
        assert!(m.named.error.is_none());
        m.apply(detached(10, "session ended"));
        assert_eq!(m.last_error.as_deref(), Some("detached: session ended"));
    }

    fn req_(name: &str) -> ResumeRequest {
        req(name, name, None)
    }

    /// M1: clicking elsewhere before a resume lands drops its "resuming…"
    /// (the client stops it; that `Detached` is stale and says nothing).
    #[test]
    fn switching_away_from_a_resume_clears_it() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        lists(&mut m, json!([{"id": "j-bebop", "name": "bebop", "live": false}]));
        m.resuming(req("bebop", "j-bebop", None), 7, false);
        assert_eq!(state_of(&m, "bebop"), State::Opening);
        m.want(8);
        assert_eq!(state_of(&m, "bebop"), State::Saved);
        m.apply(detached(7, "replaced by another attach"));
        assert_eq!(state_of(&m, "bebop"), State::Saved);
        assert!(m.named.error.is_none() && m.last_error.is_none());
    }

    /// "Open by name…" (M3): the name exactly. A live session of that name
    /// is attached; a saved one is resumed by its journal id, in its
    /// folder. What only an id contains is not a name: the saved list is
    /// asked once more (the name may be new), then it is "no session
    /// named". The typed text stays until `Attached` (`opened`).
    #[test]
    fn open_by_name_is_exact() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        m.apply(Event::Sessions(metas(json!([{"id": "l1", "name": "live-one", "journal_id": "j-l1", "cwd": "/w", "lifecycle": "live"}]))));
        let axel = json!({"id": "20261110-202033-e5f6", "name": "axel", "cwd": "/home/u/axel", "live": false});
        lists(&mut m, json!([axel]));

        assert!(m.open_by_name("Axel Memory").is_empty());
        assert!(m.named.error.as_deref().unwrap().contains("lowercase"));
        assert_eq!(
            m.open_by_name(" live-one "),
            vec![Effect::Attach { session_id: "l1".into(), fallback: Some(req("live-one", "j-l1", Some("/w"))), open: true }]
        );
        assert!(m.named.error.is_none());
        let want = Effect::Resume { req: req("axel", "20261110-202033-e5f6", Some("/home/u/axel")), open: true };
        assert_eq!(m.open_by_name("axel"), vec![want.clone()]);

        // Part of an id, not a name: asked again, then refused locally.
        assert_eq!(m.open_by_name("64fb"), [ASK_RECENT]);
        assert!(m.apply(saved_list(json!([axel]))).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("No session named '64fb'."));
        assert!(m.named.opening.is_none());
        // A name made since the last list is in the fresh one.
        assert_eq!(m.open_by_name("new-one"), [ASK_RECENT]);
        let fx = m.apply(saved_list(json!([axel, {"id": "j-new", "name": "new-one", "live": false}])));
        assert_eq!(fx, [Effect::Resume { req: req("new-one", "j-new", None), open: true }]);

        // The outcome: the input clears on `Attached`, not before.
        m.open_by_name("axel");
        m.resuming(req("axel", "20261110-202033-e5f6", Some("/home/u/axel")), 5, true);
        assert!(!m.named.opened && m.named.opening.is_some());
        m.apply(attached_ev(5, attached_named("live-axel", Some("axel"))));
        assert!(m.named.opened && m.named.opening.is_none());
    }

    /// M3 on a daemon without the saved list: it resolves the name, in `~`
    /// (no folder known, and said so). Its resolver also takes the one id
    /// containing the text: text that could be part of an id goes only on
    /// a second Enter, after the user was told, and whatever the daemon
    /// opened then stays on screen, said so (never detached and left
    /// loaded). A failure keeps the typed text.
    #[test]
    fn open_by_name_without_the_saved_list_checks_the_name() {
        use synaps_client::Cwd;
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["owned_sessions"]));
        assert!(m.open_by_name("fe").is_empty(), "could be part of an id: told first");
        assert!(m.named.error.as_deref().unwrap().contains("Press Enter again"));
        let by_name = ResumeRequest { by_name: true, ..req_("fe") };
        assert_eq!(m.open_by_name("fe"), vec![Effect::Resume { req: by_name.clone(), open: true }]);
        assert_eq!((by_name.options().cwd, by_name.options().fallback_cwd), (Cwd::Home, None));
        m.resuming(by_name.clone(), 3, true);
        let fx = m.apply(attached_ev(3, attached_named("20261110-202033-64fe", None)));
        assert_eq!(fx, [Effect::ListSessions], "stays attached");
        let notice = m.named.notice.as_deref().unwrap();
        assert!(notice.contains("No session is named 'fe'") && notice.contains("20261110-202033-64fe"), "{notice}");
        assert_eq!(m.active.as_ref().unwrap().session_id, "20261110-202033-64fe");

        let axel = ResumeRequest { by_name: true, ..req_("axel") };
        m.open_by_name("axel");
        m.resuming(axel, 4, true);
        m.apply(attached_ev(4, attached_named("s-axel", Some("axel"))));
        assert!(m.named.opened && m.named.error.is_none());
        assert!(m.named.notice.as_deref().unwrap().contains("runs in ~"));
        assert_eq!(m.active.as_ref().unwrap().session_id, "s-axel");

        m.open_by_name("ghost");
        m.resuming(ResumeRequest { by_name: true, ..req_("ghost") }, 5, true);
        m.apply(detached(5, crate::named::NO_SUCH_GHOST));
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(!m.named.opened, "the typed text stays");
    }

    /// L5: a Named live row whose session ended since the last list
    /// (`unknown session`) is resumed from its journal, in its folder.
    #[test]
    fn a_named_live_session_that_ended_is_resumed() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        m.apply(Event::Sessions(metas(
            json!([{"id": "l3", "name": "axel", "journal_id": "j-axel", "cwd": "/home/u/axel", "lifecycle": "parked"}]),
        )));
        let fx = m.open_row(&row_action(&m, "axel"));
        let fallback = Some(req("axel", "j-axel", Some("/home/u/axel")));
        assert_eq!(fx, vec![Effect::Attach { session_id: "l3".into(), fallback: fallback.clone(), open: false }]);
        m.attaching(4, fallback.clone(), false);
        // daemon/conn.rs: `DaemonFrame::Error { message: "unknown session" }`.
        let fx = m.apply(detached(4, "unknown session"));
        assert_eq!(fx, vec![Effect::Resume { req: fallback.clone().unwrap(), open: false }]);
        assert!(m.last_error.is_none());
        // Any other end is reported as before.
        m.attaching(5, fallback, false);
        m.apply(detached(5, "daemon closed the connection"));
        assert_eq!(m.last_error.as_deref(), Some("detached: daemon closed the connection"));
    }

    /// A click on the row of the session on screen (or on its way) does not
    /// attach it again (re-attaching would reload it and drop the card).
    #[test]
    fn the_attached_sessions_row_does_not_reattach() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        m.apply(Event::Sessions(metas(json!([{"id": "l3", "name": "axel", "lifecycle": "live"}]))));
        let action = row_action(&m, "axel");
        assert_eq!(m.open_row(&action).len(), 1);
        m.attaching(4, None, false);
        m.asked_for("l3");
        assert_eq!(m.open_row(&action), vec![], "on its way");
        m.apply(attached_ev(4, attached("l3")));
        assert_eq!(m.open_row(&action), vec![], "on screen");
        m.apply(detached(4, "detached"));
        assert_eq!(m.open_row(&action).len(), 1, "after a detach it attaches again");
    }

    #[test]
    fn the_saved_lists_own_failures_and_refreshes() {
        let mut m = Model::new(ConnState::Connecting);
        assert!(list_saved(&m.apply(connected(&["saved_sessions"]))));
        let fx = m.apply(Event::SavedSessionsFailed { reason: "saved sessions: Permission denied (os error 13)".into() });
        assert!(fx.is_empty(), "what was queued behind it goes too");
        assert!(m.named.saved_note.as_deref().unwrap().contains("Permission denied"));
        assert!(m.last_error.is_none());
        // L2: HISTORY says it failed ("—"), not "…" forever.
        let shape = |m: &Model| {
            let rail = crate::rail::build(m, &crate::rail::View::default(), 0);
            rail.items.iter().map(|i| format!("{i:?}")).collect::<Vec<_>>().join(" | ")
        };
        assert!(shape(&m).contains("History, count: \"—\"") && shape(&m).contains("could not be listed"), "{}", shape(&m));
        // Asked again on the next event (or find gaining focus), not on the poll.
        assert!(!list_saved(&m.apply(Event::Sessions(metas(json!([{"id": "a", "lifecycle": "live"}]))))));
        let renamed = Event::Renamed { session_id: "s".into(), name: Some("a".into()), in_memory_only: false };
        assert!(list_saved(&m.apply(renamed)));
        let fx = m.apply(Event::SavedSessions { sessions: Vec::new(), truncated: true, rejected: 2 });
        assert_eq!(fx, [ASK_NAMED]);
        assert_eq!(m.named.saved_note.as_deref(), Some("2 saved sessions could not be read (skipped)."));
        assert!(m.named.recent_truncated && m.named.recent_loaded && !m.named.saved_failed);
        m.apply(Event::SavedSessions { sessions: Vec::new(), truncated: true, rejected: 0 });
        assert!(m.named.saved_truncated);
        assert!(m.last_error.is_none());
        assert_eq!(m.looking(), [Effect::ListSaved { named_only: false, limit: SAVED_SESSIONS_MAX }], "find focused: the cap");
    }

    /// "Name this session…" (M2, L3): validated first, one at a time, the
    /// name it already has closes the row, every refusal a sentence; a
    /// `Named` refusal offers to replace the name on THAT session,
    /// compare-and-set on the name the user saw; `Renamed` moves the
    /// session into the Named section at once.
    #[test]
    fn naming_the_current_session() {
        use synaps_client::RenameFailure;
        let mut m = Model::new(ConnState::Connecting);
        assert!(m.rename_request("axon").is_none());
        assert_eq!(m.named.rename_error.as_deref(), Some("Not attached to a session."));
        attach(&mut m, 1);
        m.apply(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "name": "kite"}, {"id": "t", "lifecycle": "live"}]))));
        m.toggle_naming();
        assert!(m.rename_request("Bad Name").is_none());
        assert!(m.named.rename_error.as_deref().unwrap().contains("lowercase"));
        assert!(m.named.rename.is_none(), "nothing pending after a local refusal");
        assert!(m.rename_request("kite").is_none() && !m.named.naming, "its own name: nothing to do");

        m.toggle_naming();
        let r = m.rename_request(" axon ").unwrap();
        assert_eq!(r, RenameRequest { session_id: "s".into(), name: "axon".into(), expect: Expect::Unnamed });
        assert!(m.rename_request("axon").is_none(), "one at a time");
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("kite".into());
        m.apply(Event::RenameFailed(f));
        assert_eq!(m.named.replace, Some(Replace { session_id: "s".into(), asked: "axon".into(), current: "kite".into() }));
        assert_eq!(m.named.rename_error.as_deref(), Some("This session is already named 'kite'."));
        let r = m.replace_request().unwrap();
        assert_eq!(r, RenameRequest { session_id: "s".into(), name: "axon".into(), expect: Expect::Named("kite".into()) });
        // A reply for another session is not ours.
        m.apply(Event::RenameFailed(RenameFailure::new("other", RenameRefusal::Busy, "")));
        assert!(m.named.rename.is_some());
        let fx = m.apply(Event::Renamed { session_id: "s".into(), name: Some("axon".into()), in_memory_only: false });
        assert!(fx.contains(&Effect::ListSessions));
        assert!(!m.named.naming && m.named.rename.is_none() && m.named.replace.is_none());
        assert_eq!(m.active_name(), Some("axon"));
        assert!(rows(&m).iter().any(|r| r.0 == "axon"), "the rail shows the name at once");

        // A double Save: "already named what you asked" is success.
        m.toggle_naming();
        m.rename_request("zeta").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("zeta".into());
        m.apply(Event::RenameFailed(f));
        assert!(!m.named.naming && m.named.rename.is_none() && m.named.rename_error.is_none() && m.named.replace.is_none());

        m.toggle_naming();
        m.rename_request("taken").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Taken, "taken");
        f.taken_by = Some("20261028-045932-a1b2".into());
        m.apply(Event::RenameFailed(f));
        assert!(m.named.rename_error.as_deref().unwrap().contains("already used"));
        assert!(m.named.replace.is_none());
    }

    /// M2b / L4: switching sessions closes the naming row and drops its
    /// Replace offer (it was about the other session); an answer that
    /// comes after the user moved on shows in the Named section, and one
    /// for the current session reopens a row the user closed.
    #[test]
    fn a_replace_offer_stays_with_its_session() {
        use synaps_client::RenameFailure;
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        m.apply(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "name": "kite"}, {"id": "t", "lifecycle": "live"}]))));
        m.toggle_naming();
        m.rename_request("axon").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("kite".into());
        m.apply(Event::RenameFailed(f));
        assert!(m.named.replace.is_some());
        m.want(2);
        m.apply(attached_ev(2, attached("t")));
        assert!(!m.named.naming && m.named.replace.is_none() && m.named.rename_error.is_none());
        assert!(m.replace_request().is_none(), "no offer to take to t");

        // A late answer for s while t is shown.
        attach(&mut m, 3);
        m.toggle_naming();
        m.rename_request("axon").unwrap();
        m.want(4);
        m.apply(attached_ev(4, attached("t")));
        m.apply(Event::RenameFailed(RenameFailure::new("s", RenameRefusal::Busy, "")));
        assert!(!m.named.naming);
        assert!(m.named.error.as_deref().unwrap().starts_with("Naming a session 'axon': "), "{:?}", m.named.error);

        // The row was closed (Cancel) on the session the answer is for.
        attach(&mut m, 5);
        m.toggle_naming();
        m.rename_request("axon").unwrap();
        m.named.naming = false;
        m.apply(Event::RenameFailed(RenameFailure::new("s", RenameRefusal::Busy, "")));
        assert!(m.named.naming && m.named.rename_error.as_deref().unwrap().contains("busy"));
    }

    /// Drive a future to completion on this thread.
    fn block_on<F: Future>(fut: F) -> F::Output {
        struct Unpark(std::thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Arc::new(Unpark(std::thread::current())).into();
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::pin::pin!(fut);
        loop {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
            std::thread::park();
        }
    }

    fn delta(text: &str) -> Event {
        session(1, SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: text.into() }) })
    }

    /// H4, the pump itself: a backlog (what piles up behind a window that
    /// got no frames) is coalesced into batches of at most `MAX_BATCH`,
    /// each applied to the model the moment it is taken, so the channel is
    /// drained to the cap on every delivery and ends empty. No render, no
    /// GPUI: the pump does not wait for one.
    #[test]
    fn the_pump_coalesces_a_backlog_into_bounded_batches_and_drains_it() {
        let (tx, rx) = async_channel::unbounded();
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        m.apply(session(1, SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: None }));
        let n = 2 * MAX_BATCH + 100;
        for _ in 0..n {
            tx.try_send(delta("x")).unwrap();
        }
        drop(tx);
        let (mut sizes, mut left) = (Vec::new(), Vec::new());
        let probe = rx.clone();
        block_on(pump(rx, |batch| {
            sizes.push(batch.len());
            left.push(probe.len());
            for ev in batch {
                m.apply(ev);
            }
            true
        }));
        assert_eq!(sizes, vec![MAX_BATCH, MAX_BATCH, 100]);
        assert_eq!(left, vec![n - MAX_BATCH, 100, 0]);
        assert_eq!(probe.len(), 0);
        assert_eq!(m.transcript.items().last().unwrap().text.len(), n);
    }

    /// The pump stops when the view is gone, leaving the rest unapplied,
    /// and applies events that arrive while it waits one batch at a time.
    #[test]
    fn the_pump_stops_with_the_view_and_follows_a_live_stream() {
        let (tx, rx) = async_channel::unbounded();
        for _ in 0..MAX_BATCH + 5 {
            tx.try_send(delta("x")).unwrap();
        }
        let mut calls = 0;
        block_on(pump(rx.clone(), |_| {
            calls += 1;
            false
        }));
        assert_eq!((calls, rx.len()), (1, 5));

        let (tx2, rx2) = async_channel::unbounded();
        let producer = std::thread::spawn(move || {
            for _ in 0..5000 {
                tx2.send_blocking(delta("y")).unwrap();
            }
        });
        let mut total = 0;
        block_on(pump(rx2.clone(), |b| {
            assert!(b.len() <= MAX_BATCH);
            total += b.len();
            true
        }));
        producer.join().unwrap();
        assert_eq!((total, rx2.len()), (5000, 0));
        drop(tx);
    }
}

/// The model against a client on a real socket: a fake daemon (std threads,
/// no runtime) that hands each attach to the test.
#[cfg(all(test, unix))]
mod fake_daemon {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use synaps_client::{AttachMode, Client, ClientOptions, Endpoint, Identity};

    use super::*;

    const A: &str = "20261113-080000-aaaa";
    const B: &str = "20261113-080000-bbbb";
    const T: Duration = Duration::from_secs(10);

    struct Conn {
        r: BufReader<UnixStream>,
        w: UnixStream,
    }

    impl Conn {
        fn send(&mut self, v: Value) {
            let _ = writeln!(self.w, "{v}");
        }

        fn event(&mut self, sid: &str, seq: u64, event: Value) {
            self.send(json!({"type": "event", "session_id": sid, "seq": seq, "ts": "t", "event": event}));
        }

        /// The next frame; `None` at EOF or after `wait` of silence.
        fn recv(&mut self, wait: Duration) -> Option<Value> {
            self.r.get_ref().set_read_timeout(Some(wait)).ok()?;
            let mut line = String::new();
            match self.r.read_line(&mut line) {
                Ok(n) if n > 0 => serde_json::from_str(&line).ok(),
                _ => None,
            }
        }

        /// The session commands among the next frames, until EOF or `wait` of silence.
        fn cmds(&mut self, wait: Duration) -> Vec<Value> {
            std::iter::from_fn(|| self.recv(wait)).filter(|f| f["type"] == "cmd").map(|f| f["cmd"].clone()).collect()
        }
    }

    fn welcome(saved: bool) -> Value {
        let features: Vec<&str> = if saved { vec!["owned_sessions", "saved_sessions"] } else { vec!["owned_sessions"] };
        json!({"type": "welcome", "protocol_version": 3, "daemon_version": "0.0.0-fake", "pid": 1, "profile": null,
               "sessions": [{"id": A, "model": "m", "lifecycle": "live"}, {"id": B, "model": "m", "lifecycle": "live"}],
               "progressive_tool_disclosure": false, "generation": 1, "features": features})
    }

    /// The fake's saved sessions (`saved::rows` on #408): the named ones for
    /// `named_only`, else the newest `limit`; one is held (`live`).
    fn saved_reply(f: &Value) -> Value {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let at = |ago: i64| crate::demo::rfc3339(now - ago);
        let all = [
            json!({"id": A, "title": "held by the daemon", "updated_at": at(5), "live": true}),
            json!({"id": "20261113-070000-0001", "title": "Fix the relay", "updated_at": at(600), "live": false}),
            json!({"id": "20261113-060000-0002", "title": "Bench the decoder", "updated_at": at(3600), "live": false}),
            json!({"id": "20261111-060000-0003", "name": "kite", "title": "Kite notes", "updated_at": at(2 * 86_400), "live": false}),
            json!({"id": "20261013-060000-0004", "title": "Old work", "updated_at": at(60 * 86_400), "live": false}),
        ];
        let named_only = f["named_only"].as_bool().unwrap_or(false);
        let limit = f["limit"].as_u64().unwrap_or(200) as usize;
        let rows: Vec<&Value> = all.iter().filter(|v| !named_only || v["name"].is_string()).collect();
        json!({"type": "saved_session_list", "sessions": rows.iter().take(limit).collect::<Vec<_>>(), "truncated": rows.len() > limit})
    }

    fn attached_with_prompt(sid: &str, prompt_id: u64) -> Value {
        attached_with(sid, prompt_id, sid)
    }

    /// `Attached` with one pending confirm whose guard message is `message`.
    fn attached_with(sid: &str, prompt_id: u64, message: &str) -> Value {
        json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
               "meta": {"id": sid, "model": "m", "lifecycle": "live", "awaiting_input": 1},
               "conversation": {"api_messages": [], "messages_len": 0},
               "pending_prompts": [{"id": prompt_id, "kind": "confirm", "title": "Confirm tool call",
                   "prompt": format!("Extension: guard\nTool: edit\n\nMessage from guard:\n{message}\n\nAllow this tool call?")}]})
    }

    /// Accepts connections: `Hello` → `Welcome`, a session list for the
    /// control connection, and every `Attach` handed to the test.
    fn start(name: &str) -> (PathBuf, mpsc::Receiver<Conn>) {
        let (path, attaches, _control) = start_with(name, false);
        (path, attaches)
    }

    /// `saved`: `Welcome.features` lists `saved_sessions` and the fake
    /// answers it (else: `Error{unknown variant}`, as an older daemon).
    /// Every control frame but `Hello` and `Attach` goes to the third
    /// receiver.
    fn start_with(name: &str, saved: bool) -> (PathBuf, mpsc::Receiver<Conn>, mpsc::Receiver<Value>) {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("d.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        let (ctl_tx, ctl_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (tx, ctl_tx) = (tx.clone(), ctl_tx.clone());
                std::thread::spawn(move || {
                    let mut c = Conn { w: stream.try_clone().unwrap(), r: BufReader::new(stream) };
                    if c.recv(T).is_none() {
                        return;
                    }
                    c.send(welcome(saved));
                    let _ = c.r.get_ref().set_read_timeout(None);
                    let mut line = String::new();
                    while c.r.read_line(&mut line).is_ok_and(|n| n > 0) {
                        let f: Value = serde_json::from_str(&line).unwrap_or_default();
                        line.clear();
                        if f["type"] != "attach" {
                            let _ = ctl_tx.send(f.clone());
                        }
                        match f["type"].as_str() {
                            Some("sessions") => c.send(json!({"type": "session_list", "sessions": []})),
                            Some("saved_sessions") if saved => c.send(saved_reply(&f)),
                            Some("saved_sessions") => c.send(json!({"type": "error", "session_id": null,
                                "message": "malformed frame: unknown variant `saved_sessions`"})),
                            Some("attach") => {
                                let _ = tx.send(c);
                                return;
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        (path, rx, ctl_rx)
    }

    /// Apply the client's events to `m` until `done(m)`.
    fn pump(m: &mut Model, events: &async_channel::Receiver<Event>, what: &str, done: impl Fn(&Model) -> bool) {
        let until = Instant::now() + T;
        loop {
            while let Ok(ev) = events.try_recv() {
                m.apply(ev);
            }
            if done(m) {
                return;
            }
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn connect(name: &str) -> (Client, async_channel::Receiver<Event>, mpsc::Receiver<Conn>, Model) {
        let (path, attaches) = start(name);
        let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::new("tester").unwrap())).unwrap();
        let mut m = Model::new(ConnState::Connecting);
        pump(&mut m, &events, "connected", |m| matches!(m.conn, ConnState::Connected(_)));
        (client, events, attaches, m)
    }

    /// The UI's attach (`AppView::attach`).
    fn ui_attach(m: &mut Model, client: &Client, sid: &str) {
        let seq = client.attach(sid, AttachMode::Mirror).unwrap();
        m.want(seq);
        m.asked_for(sid);
    }

    /// The UI's AUTHORIZE (`AppView::answer`): only what the model allows goes out.
    fn ui_authorize(m: &mut Model, client: &Client, prompt_id: u64) -> bool {
        if !m.may_answer(prompt_id) {
            return false;
        }
        client.answer(prompt_id, Some(CONFIRM_YES.into())).unwrap();
        m.answer_sent(prompt_id);
        true
    }

    /// H2: JUMP from A (prompt #1 on screen) to B (its own prompt #1). Until
    /// B's `Attached` is applied, A's card cannot answer: the client routes
    /// every command to its latest attach, so that answer would approve B's
    /// tool call unseen. Then B's own prompt is answered, once, on B's
    /// connection; A's connection never sees an answer.
    #[test]
    fn a_switch_never_answers_the_other_sessions_prompt() {
        let (client, events, attaches, mut m) = connect("switch");
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.may_answer(1));
        // JUMP.
        ui_attach(&mut m, &client, B);
        assert!(!ui_authorize(&mut m, &client, 1), "A's card while switching");
        let mut b = attaches.recv_timeout(T).unwrap();
        b.send(attached_with_prompt(B, 1));
        // The connection thread has B; the window still shows A (race (b)).
        std::thread::sleep(Duration::from_millis(50));
        assert!(m.is_active(A) && !ui_authorize(&mut m, &client, 1), "A's card after B attached on the wire");
        pump(&mut m, &events, "B attached", |m| m.is_active(B));
        assert!(!m.prompt_answered(1), "B's prompt #1 is not A's");
        assert!(ui_authorize(&mut m, &client, 1));
        assert!(!ui_authorize(&mut m, &client, 1), "a second click sends nothing");
        let on_b: Vec<Value> = b.cmds(Duration::from_millis(300)).into_iter().filter(|c| c["cmd"] == "answer").collect();
        assert_eq!(on_b, [json!({"cmd": "answer", "prompt_id": 1, "value": "y"})]);
        client.shutdown().unwrap();
        let on_a = a.cmds(Duration::from_millis(300));
        assert!(!on_a.iter().any(|c| c["cmd"] == "answer"), "{on_a:?}");
    }

    /// H2: an answer the daemon refused does not leave the card stuck at
    /// "answer sent", and an answered prompt id does not carry over to the
    /// next session's prompt with the same id.
    #[test]
    fn a_refused_answer_does_not_leave_the_card_stuck() {
        let (client, events, attaches, mut m) = connect("stuck");
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.may_answer(1));
        assert!(ui_authorize(&mut m, &client, 1));
        assert!(a.cmds(Duration::from_millis(300)).iter().any(|c| c["cmd"] == "answer"));
        a.event(A, 5, json!({"ev": "refused", "client": 2, "command": "answer", "reason": "input is owned by client #1"}));
        pump(&mut m, &events, "the refusal", |m| m.may_answer(1));
        // Answered again (it went out), then a switch before it resolved.
        assert!(ui_authorize(&mut m, &client, 1));
        ui_attach(&mut m, &client, B);
        let mut b = attaches.recv_timeout(T).unwrap();
        b.send(attached_with_prompt(B, 1));
        pump(&mut m, &events, "B attached", |m| m.is_active(B));
        assert!(m.may_answer(1), "B's card is live, not 'answer sent'");
        client.shutdown().unwrap();
        let _ = b.cmds(Duration::from_millis(100));
    }

    /// N2: the daemon restarts while prompt #1 is on the card, armed and
    /// read. The client reconnects under the same `attach_seq`, and the
    /// restarted daemon (prompt ids from 1 again) has a different prompt #1
    /// by then: the card shows that one's text, unarmed, and an answer
    /// waits for it to arm.
    #[test]
    fn a_restarted_daemons_prompt_1_is_a_new_card() {
        use crate::app::{ARM, PromptCard};
        let (client, events, attaches, mut m) = connect("restart");
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with(A, 1, "edit README.md"));
        pump(&mut m, &events, "A attached", |m| m.may_answer(1));
        let mut card = None;
        assert!(PromptCard::sync(&mut card, &m, Instant::now() - ARM), "a new card");
        assert!(card.as_ref().is_some_and(|c| c.armed() && c.body.contains("edit README.md")));
        assert!(!PromptCard::sync(&mut card, &m, Instant::now()), "the same prompt keeps its card");
        // The daemon goes away; the client reconnects (same attach_seq).
        let seq = m.active.as_ref().unwrap().attach_seq;
        drop(a);
        pump(&mut m, &events, "reattaching", |m| m.reattaching);
        assert!(!m.may_answer(1), "nothing answers while reconnecting");
        let mut a = attaches.recv_timeout(T).unwrap();
        let stale = m.active.as_ref().unwrap().connection;
        a.send(attached_with(A, 1, "rm -rf ~/work"));
        // AUTHORIZE on the old card as the client thread re-attaches, before
        // the UI has seen it (`AppView::answer` sends on the card's
        // connection): refused, during the reconnect or after it, never sent.
        client.answer_on(stale, 1, Some(CONFIRM_YES.into())).unwrap();
        pump(&mut m, &events, "A reattached", |m| !m.reattaching && m.may_answer(1));
        assert_eq!(m.active.as_ref().unwrap().attach_seq, seq, "a reconnect keeps the attach_seq");
        assert_ne!(m.active.as_ref().unwrap().connection, stale, "a new connection");
        assert!(PromptCard::sync(&mut card, &m, Instant::now()), "the restarted daemon's #1 is a new card");
        let c = card.as_ref().unwrap();
        assert!(c.body.contains("rm -rf ~/work") && !c.body.contains("README"), "the new text: {}", c.body);
        assert!(!c.armed(), "unarmed");
        // The new card's own answer goes out.
        client.answer_on(m.active.as_ref().unwrap().connection, 1, Some("n".into())).unwrap();
        client.shutdown().unwrap();
        let answers: Vec<Value> = a.cmds(Duration::from_millis(500)).into_iter().filter(|c| c["cmd"] == "answer").collect();
        assert_eq!(answers, vec![json!({"cmd": "answer", "prompt_id": 1, "value": "n"})], "only the new connection's answer");
    }

    /// HISTORY end to end, both ways (the rail through the client, the
    /// model and the rail builder). A daemon that lists `saved_sessions`
    /// is asked for the newest 200 (the named list too only if that was
    /// cut), and HISTORY is drawn in its buckets, less what the daemon holds. One
    /// that does not is never asked, and HISTORY says why and what works.
    #[test]
    fn history_with_and_without_saved_sessions() {
        use crate::rail::{Bucket, Item, View, build};
        for saved in [true, false] {
            let (path, _attaches, control) = start_with(if saved { "history" } else { "nohistory" }, saved);
            let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::new("tester").unwrap())).unwrap();
            let mut m = Model::new(ConnState::Connecting);
            let until = Instant::now() + T;
            let done = |m: &Model| matches!(m.conn, ConnState::Connected(_)) && (!saved || m.named.recent_loaded);
            while !done(&m) {
                assert!(Instant::now() < until, "timed out (saved: {saved})");
                while let Ok(ev) = events.try_recv() {
                    for fx in m.apply(ev) {
                        // The UI's `Effect::ListSaved` (`AppView::effect`).
                        if let Effect::ListSaved { named_only, limit } = fx
                            && let Err(e) = client.saved_sessions(named_only, limit)
                        {
                            m.saved_failed(&e.to_string());
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            // A marker: every frame sent before it has reached the fake.
            client.list_sessions().unwrap();
            let asks: Vec<Value> = std::iter::from_fn(|| control.recv_timeout(T).ok())
                .take_while(|f| f["type"] != "sessions")
                .filter(|f| f["type"] == "saved_sessions")
                .collect();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
            let rail = build(&m, &View { open: Bucket::DEFAULT_OPEN, ..View::default() }, now);
            let shape: Vec<String> = rail
                .items
                .iter()
                .map(|i| match i {
                    Item::Group { group, count } => format!("{group:?} {count}"),
                    Item::Row(r) | Item::Hist(r) => rail.rows[*r].title.to_string(),
                    Item::Bucket { bucket, count, .. } => format!("{bucket:?} {count}"),
                    Item::More { text, .. } => text.to_string(),
                    Item::Note(t) => format!("note: {t}"),
                    Item::Empty(t) => t.to_string(),
                })
                .collect();
            if saved {
                // The newest 200 hold everything (not cut): one request.
                assert_eq!(asks, [json!({"type": "saved_sessions", "named_only": false, "limit": 200})]);
                assert_eq!(
                    shape,
                    [
                        "Open 2",
                        "New session",
                        "New session",
                        "History 4",
                        "Named 1",
                        "kite",
                        "Today 2",
                        "Fix the relay",
                        "Bench the decoder",
                        "Older 1"
                    ],
                    "{shape:#?}"
                );
            } else {
                assert!(asks.is_empty(), "{asks:?}");
                assert_eq!(&shape[..3], ["Open 2", "New session", "New session"]);
                assert_eq!(shape[3], "History —");
                assert!(shape[4].starts_with("note: This daemon does not list saved sessions") && shape[4].contains("exact name"));
                assert_eq!(shape.len(), 5);
            }
            client.shutdown().unwrap();
        }
    }
}
