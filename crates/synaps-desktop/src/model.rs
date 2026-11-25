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
    AttachMode, CATALOG_PAGE_MAX, CatalogCursor, ClientInput, Error, Event, Expect, FEATURE_SAVED_SESSIONS, FEATURE_SESSION_PAGE,
    RenameRefusal, SAVED_SESSIONS_MAX, SavedSession, SessionEvent, Status, Transcript, validate_session_name,
};

use crate::live::{LiveId, Lives};
use crate::named::{ResumeRequest, RowAction, parse_rfc3339};
use crate::pane::PaneId;
use crate::rail::{Chip, State, is_parked};
use crate::stats::Stats;
use crate::tabs::{Place, Tabs};

/// HISTORY's newest unnamed sessions, asked for at startup; up to
/// [`SAVED_SESSIONS_MAX`] (the wire's cap) once OLDER opens or find is used.
pub const SAVED_RECENT: u32 = 200;

/// Failed page fetches asked again (one per `Sessions` poll) on one
/// connection.
pub const PAGE_RETRIES: u32 = 3;

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

/// The daemon this window is connected to, as its `Welcome` said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Daemon {
    pub version: String,
    pub profile: Option<String>,
    pub pid: u32,
}

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
    /// This connection's client id in the session, who owns its input,
    /// and the session's input rule (who may send it).
    pub client: u64,
    pub owner: Option<u64>,
    pub input: ClientInput,
}

/// An attachment a tab switch left, with its input in flight ([`Model::left`]).
type Left = (u64, String, VecDeque<(crate::live::Target, String)>);

/// What the view must do after an event (it owns the client and composer).
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Input the daemon refused (or never got): put it back in the composer
    /// of the pane that sent it.
    Restore(PaneId, String),
    /// Input sent to `session` before its tab was left, refused (or never
    /// confirmed) since: back into that tab's draft, which says so.
    Unsent {
        /// The pane that sent it.
        pane: PaneId,
        session: String,
        text: String,
    },
    ListSessions,
    /// The history of this attachment no longer applies: fetch it whole.
    Resync(u64),
    /// `Client::saved_sessions(named_only, limit)`; if it cannot be sent,
    /// tell [`Model::saved_failed`].
    ListSaved {
        named_only: bool,
        limit: u32,
    },
    /// `Client::catalog_page(before, limit, named_only)`, then
    /// [`Model::page_sent`] with its `req`; if it cannot be sent, tell
    /// [`Model::saved_failed`].
    CatalogPage {
        before: Option<CatalogCursor>,
        limit: u32,
        named_only: bool,
    },
    /// Attach to a live session, then [`Model::attaching`]. `fallback`: a
    /// named session's journal, resumed if it ended meanwhile. `open`: an
    /// Open by name. `place`: the tab it opens in (its own if it has one).
    Attach {
        session_id: String,
        fallback: Option<ResumeRequest>,
        open: bool,
        place: Place,
    },
    /// `Client::resume(req.target, req.options())`, then [`Model::resuming`].
    Resume {
        req: ResumeRequest,
        open: bool,
        place: Place,
    },
    /// What the composer's note line says: of a worker (a cancel
    /// refused), or a slash command's answer.
    WorkerNote(String),
    /// A slash command answered on the attachment on screen: the
    /// composer's note says what became of it, and a draft that sent it
    /// goes once it is done ([`synaps_client::CommandReply`]).
    Command {
        name: String,
        reply: synaps_client::CommandReply,
    },
    /// A model or thinking pick answered ([`crate::choice`]).
    Setting {
        attach_seq: u64,
        setting: String,
        reply: Result<Option<String>, String>,
    },
    /// Ask attachment `attach_seq`'s session for its `Status` (the right
    /// panel's cache and keep-warm numbers, [`crate::warm`]).
    Status(u64),
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
    /// The tab it opens in (Ctrl+Enter: a new one).
    place: Place,
}

/// One `saved_sessions` request: the named list, or the newest `limit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedAsk {
    Named,
    Recent(u32),
}

/// A [`SavedAsk`] served by `catalog_page` (a daemon with
/// `session_page`): pages of saved rows, collected until the ask's limit or
/// the end, then landed as a `saved_sessions` reply would be.
#[derive(Debug)]
struct PageFetch {
    named_only: bool,
    /// The ask's limit: past it, the list is `truncated`.
    want: usize,
    rows: Vec<SavedSession>,
    rejected: usize,
    /// The page on its way, once sent: a reply with another `req` (asked on
    /// a connection since dropped, or by another fetch) is not this one's.
    req: Option<u64>,
    /// Pages asked for, and the `before` of the last: the loop's own bound
    /// (a daemon that kept sending `next` with no rows kept, or the same
    /// `next`, would otherwise be asked forever).
    pages: usize,
    before: Option<CatalogCursor>,
}

impl PageFetch {
    fn new(named_only: bool, want: usize) -> Self {
        PageFetch { named_only, want, rows: Vec::new(), rejected: 0, req: None, pages: 0, before: None }
    }

    fn ask(&mut self, before: Option<CatalogCursor>) -> Effect {
        let limit = (self.want - self.rows.len()).min(CATALOG_PAGE_MAX as usize) as u32;
        self.req = None;
        self.pages += 1;
        self.before = before.clone();
        Effect::CatalogPage { before, limit, named_only: self.named_only }
    }

    /// Ask on from `next`? Not past the ask's rows, nor past `want / 200 +
    /// 2` pages, nor from the cursor just sent.
    fn more(&self, next: &CatalogCursor) -> bool {
        self.rows.len() < self.want && self.pages < self.want / CATALOG_PAGE_MAX as usize + 2 && self.before.as_ref() != Some(next)
    }
}

/// What the rail knows of saved sessions, and the names and resumes.
#[derive(Debug, Default)]
pub struct Named {
    /// The daemon lists `session_page` or `saved_sessions` (else: no
    /// HISTORY; a name is resolved by the daemon).
    pub saved_supported: bool,
    /// The daemon lists `session_page`: the saved lists are read from
    /// `catalog_page` (else `saved_sessions`).
    pub pages: bool,
    /// The ask in flight, served by pages.
    page: Option<PageFetch>,
    /// The `(id, journal_id)` set of the last polled `Sessions` on this
    /// connection (`None` until the first, the `Welcome`'s, which is not
    /// news to the first page asked on connecting). Not `sessions`: an
    /// attached swap writes its journal there at once, so the poll after a
    /// `/clear` would agree with it and ask nothing.
    polled: Option<HashSet<(String, String)>>,
    /// Failed page fetches asked again on a later poll, this connection
    /// (at most [`PAGE_RETRIES`]).
    retries: u32,
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
    /// The cap has been asked for (OLDER opened, or find used); dropped
    /// when neither is so any more, and on connecting (set again from
    /// [`Named::deep_wanted`]), so refreshes go back to the newest page.
    pub deep: bool,
    /// OLDER is open or find has text: the cap is what is wanted.
    pub deep_wanted: bool,
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
    /// Listings asked for so far, on any connection (never reset): a
    /// favourite's gone verdict holds against those (it was there before).
    pub asks: u64,
    /// The number (of [`Named::asks`]) of the one on its way; of the
    /// newest list of each kind landed (each says only what it knew when it
    /// was asked: a favourite's gone holds against an older one).
    asked: u64,
    pub recent_listed: u64,
    pub named_listed: u64,
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
/// Left attachments whose unconfirmed input is still followed.
const LEFT_MAX: usize = 4;

/// The daemon refused a submit or a steer, or a refusal that names no
/// command (it could be one): the oldest input in flight comes back.
fn refused_input(e: &Error) -> bool {
    match e {
        Error::Refused { command: None, .. } => true,
        Error::Refused { command: Some(c), .. } => c == "submit" || c == "steer",
        Error::NotAttached { command } => *command != "answer",
        _ => false,
    }
}

pub struct Model {
    pub conn: ConnState,
    /// The link went down for a daemon reload (a planned handover), not a
    /// lost transport: kept until it is up again or drops otherwise.
    pub reload: bool,
    /// How many reload goodbyes have come: a new one restarts the link's
    /// reload grace (a storm of reloads).
    pub reload_seq: u64,
    pub sessions: Vec<SessionMeta>,
    /// The live stores ([`crate::live`]): each attachment's session, its
    /// transcript, numbers, worker views and input in flight. Events go to
    /// the one that asked for their `attach_seq`.
    pub(crate) lives: Lives,
    /// The last [`Active::epoch`] handed out (window-wide: `(epoch, prompt
    /// id)` card keys never collide across stores).
    epoch: u64,
    /// The input in flight ([`Live::in_flight`]) of attachments a tab switch left (their last events still
    /// come, until their `Detached`): `(attach_seq, session, texts)`, each text with the pane that sent it.
    left: Vec<Left>,
    /// The agent cards' answers this window sent, by session
    /// ([`crate::deliver`]).
    pub(crate) memos: crate::deliver::Memos,
    /// The open agent cards of the sessions this window holds (D3,
    /// [`crate::presence`]): NEEDS YOU and ASKED ([`Model::fact`]).
    pub(crate) presence: crate::presence::Presence,
    /// Which wait cards came live, as the events brought them (F12's
    /// provenance, [`crate::presence::Arrivals`]).
    pub(crate) arrivals: crate::presence::Arrivals,
    pub named: Named,
    /// The favourite sessions (desktop only, per daemon): the rail's
    /// FAVOURITES. Never sent to the daemon.
    pub favs: crate::favourites::Favourites,
    /// The open tabs; the visible one's session is the one attached.
    pub tabs: Tabs,
    /// A session list has landed (saved tabs can be checked against it),
    /// and how many have (the polls of an idle daemon included).
    pub sessions_listed: bool,
    pub lists: u64,
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
    /// Set by every event but a session list equal to the last: the view
    /// draws again only then (the 3-second poll of an idle daemon is free).
    pub view_dirty: bool,
    /// The daemon at the last `Welcome`, and how many there have been (a
    /// reconnect may be to a restarted daemon: Settings and Usage look
    /// for its dash again).
    pub daemon: Option<Daemon>,
    pub daemon_gen: u64,
    /// The daemon's `Status` says when the prompt cache expires
    /// (`Welcome.features: cache_expiry`): only then is it asked for the
    /// right panel's cache and keep-warm ([`crate::warm`]).
    pub cache_expiry: bool,
}

/// Tests only (split/architecture.md §6.6): `v.model.transcript`,
/// `m.active`, `m.can_send()` ... read the one live store, so the
/// protected tests compile unchanged. Production code never sees it: it
/// names the store (`model.lives[pane.live]`).
#[cfg(test)]
impl std::ops::Deref for Model {
    type Target = crate::live::Live;
    fn deref(&self) -> &crate::live::Live {
        &self.lives[LiveId::FIRST]
    }
}

#[cfg(test)]
impl std::ops::DerefMut for Model {
    fn deref_mut(&mut self) -> &mut crate::live::Live {
        &mut self.lives[LiveId::FIRST]
    }
}

/// An empty transcript as the window keeps it: no notices for subagent
/// starts and ends (each worker shows under its call, [`crate::subagents`]).
pub fn transcript() -> Transcript {
    let mut t = Transcript::new();
    t.set_subagent_notices(false);
    t
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
            reload: false,
            reload_seq: 0,
            sessions: Vec::new(),
            lives: Lives::default(),
            epoch: 0,
            left: Vec::new(),
            memos: Default::default(),
            presence: Default::default(),
            arrivals: Default::default(),
            named: Named { daemon: "the daemon's machine".into(), ..Named::default() },
            favs: Default::default(),
            tabs: Tabs::default(),
            sessions_listed: false,
            lists: 0,
            rail_gen: 0,
            unread: HashSet::new(),
            seen: HashMap::new(),
            view_dirty: false,
            daemon: None,
            daemon_gen: 0,
            cache_expiry: false,
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
            n.asks += 1;
            n.asked = n.asks;
            if n.pages {
                let (named_only, want) = match ask {
                    SavedAsk::Named => (true, SAVED_SESSIONS_MAX),
                    SavedAsk::Recent(limit) => (false, limit),
                };
                let mut f = PageFetch::new(named_only, want.max(1) as usize);
                effects.push(f.ask(None));
                n.page = Some(f);
                return;
            }
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

    /// OLDER is open or find has text (`wanted`), or neither is any more:
    /// then later refreshes ask for the newest page only.
    pub fn deep_wanted(&mut self, wanted: bool) {
        self.named.deep_wanted = wanted;
        if !wanted {
            self.named.deep = false;
        }
    }

    /// `--offline`: the synthetic transport answers `catalog_page` itself.
    pub fn offer_saved(&mut self) -> Vec<Effect> {
        self.named.saved_supported = true;
        self.named.pages = true;
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

    /// A saved list landed (a `saved_sessions` reply, or a page fetch's
    /// rows): it answers the ask in flight.
    fn saved_landed(&mut self, sessions: Vec<SavedSession>, truncated: bool, rejected: usize, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        let asked = n.saved_in_flight.take();
        n.saved_note = (rejected > 0).then(|| format!("{rejected} saved sessions could not be read (skipped)."));
        if asked.is_some() {
            n.saved_failed = false;
        }
        match asked {
            Some(SavedAsk::Named) => {
                n.named_listed = n.asked;
                n.saved = sessions;
                n.saved_truncated = truncated;
                n.named_stale = false;
            }
            Some(SavedAsk::Recent(_)) => {
                n.recent_listed = n.asked;
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
        self.pump_saved(effects);
        // The refresh is done: an Open by name waiting for it, and
        // UNREAD marks of sessions in no list.
        if !self.saved_listing() {
            self.resolve_open(effects);
            self.prune_unread();
        }
    }

    /// Pages: nothing pushes saved-list changes (no `session_watch`), so
    /// any change in who is held since the last poll (a session ended,
    /// resumed, compacted into a successor, swapped by `/clear`) asks for a
    /// fresh first page, which replaces the list.
    fn poll_held(&mut self, s: &[SessionMeta], effects: &mut Vec<Effect>) {
        let now: HashSet<(String, String)> = s.iter().map(|s| (s.id.clone(), s.journal_id.clone())).collect();
        let changed = self.named.pages && self.named.polled.as_ref().is_some_and(|p| *p != now);
        self.named.polled = Some(now);
        if changed {
            self.named.named_stale = true;
            self.refresh_saved(effects);
        } else if self.named.pages && self.named.saved_failed && !self.saved_listing() && self.named.retries < PAGE_RETRIES {
            // A failed page is now a normal reply: ask again on the next
            // poll, a few times (a session that ended meanwhile is in
            // neither list until then).
            self.named.retries += 1;
            self.named.named_stale = true;
            self.refresh_saved(effects);
        }
    }

    /// The `Effect::CatalogPage` just handled went out as `req`.
    pub fn page_sent(&mut self, req: u64) {
        if let Some(f) = &mut self.named.page {
            f.req = Some(req);
        }
    }

    /// A page of saved rows: collect it, then ask for the next or land the
    /// list. A reply that is not the page on its way is dropped.
    fn on_page(&mut self, req: u64, sessions: Vec<SessionMeta>, next: Option<CatalogCursor>, rejected: usize, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        let Some(f) = n.page.as_mut().filter(|f| f.req == Some(req)) else {
            log::debug!(target: "synaps_desktop::saved", "catalog page {req}: not the one on its way (dropped)");
            return;
        };
        // Held sessions come from `Sessions`; a page never lists them.
        f.rows.extend(sessions.iter().filter(|s| s.is_saved()).map(SessionMeta::to_saved));
        f.rejected += rejected;
        match next {
            Some(c) if f.more(&c) => effects.push(f.ask(Some(c))),
            next => {
                let Some(mut f) = n.page.take() else { return };
                let truncated = next.is_some() || f.rows.len() > f.want;
                f.rows.truncate(f.want);
                self.saved_landed(f.rows, truncated, f.rejected, effects);
            }
        }
    }

    fn on_saved_failed(&mut self, reason: &str, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        n.page = None;
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
    /// turn that just ended), which runs ahead of the 3-second poll. An
    /// agent card waiting is NEEDS YOU too; one open, ASKED, once the turn
    /// is over (its open cards: [`crate::presence`]).
    pub fn fact(&self, s: &SessionMeta) -> State {
        let (needs, busy) = match self.lives.on(&s.id) {
            Some(l) => (!l.transcript.pending_prompts().is_empty(), l.transcript.busy()),
            None => (s.awaiting_input > 0, s.busy),
        };
        let cards = self.presence.of(&s.id);
        if needs || cards.wait > 0 {
            State::NeedsYou
        } else if busy {
            State::Running
        } else if cards.asked > 0 {
            State::Asked
        } else if self.workers(s) > 0 {
            State::Workers
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

    /// A held session's background workers running. The attached one's
    /// from its own transcript once a live source (an event, a row) has
    /// spoken of its workers since the attach: it runs ahead of the poll
    /// and needs no daemon support. Before that, and for the others, the
    /// listing's count ([`SessionMeta::workers_running`]: `workers.running`
    /// or `subagents_running`, 0 from a daemon that sends neither).
    pub fn workers(&self, s: &SessionMeta) -> usize {
        match self.lives.on(&s.id).map(|l| &l.stats.subagents) {
            Some(own) if own.heard() => own.running_count(),
            _ => s.workers_running(),
        }
    }

    /// Store `live`'s session's workers running ([`Model::workers`]; not
    /// listed yet, a create: its transcript's). Nothing attached (a cached
    /// tab on its way back, a detach): none, never a count last seen.
    pub fn active_workers(&self, live: LiveId) -> usize {
        match self.active_meta(live) {
            Some(s) => self.workers(s),
            None if self.lives[live].active.is_some() => self.lives[live].stats.subagents.running_count(),
            None => 0,
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
        chip.admits(self.fact(s))
            || (self.opening(s) && self.lives.iter().any(|l| l.is_wanted(&s.id) && l.wanted_fact.is_some_and(|f| chip.admits(f))))
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

    /// The attached session `session_id` holds `journal`, the daemon says
    /// (its conversation's header): the session list has it at once, so its
    /// row, its favourite star and the favourite key act on that journal,
    /// not on the one the last poll saw.
    fn holds(&mut self, session_id: &str, journal: &str) {
        if journal.is_empty() {
            return;
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id && s.journal_id != journal) {
            s.journal_id = journal.to_string();
            self.rail_gen += 1;
        }
    }

    /// A rail row's star (or its menu's Favourite), by what the row is,
    /// never by a bare id (a favourite's journal id is a live id too when
    /// the session it started as still runs, holding another journal): a
    /// held session's row toggles the journal it holds; a favourite's own
    /// row unfavourites it; a HISTORY row toggles that saved session. `true`
    /// if the favourites changed.
    pub fn toggle_fav_row(&mut self, action: &RowAction, fav: bool, now: i64) -> bool {
        use crate::favourites::Source;
        match action {
            RowAction::Attach(id) => match self.sessions.iter().find(|s| &s.id == id) {
                Some(s) => {
                    self.favs.toggle(Source::Live(s), now);
                    true
                }
                None => false,
            },
            RowAction::Resume(r) if r.by_name => false,
            RowAction::Resume(r) if fav => self.favs.remove(&r.target),
            RowAction::Resume(r) => match self.named.recent.iter().chain(self.named.saved.iter()).find(|v| v.id == r.target) {
                Some(v) => {
                    self.favs.toggle(Source::Saved(v), now);
                    true
                }
                None => false,
            },
        }
    }

    /// The favourites' snapshots brought up to what the lists say now (a
    /// held session, else its listing): run when the lists change, before
    /// the rail is built. `true` if any changed (and so is saved).
    pub fn sync_favs(&mut self) -> bool {
        if self.favs.is_empty() {
            return false;
        }
        let n = &self.named;
        let changed = self.favs.refresh(&self.sessions, &[(&n.recent, n.recent_listed), (&n.saved, n.named_listed)]);
        if changed {
            self.rail_gen += 1;
        }
        changed
    }

    /// A listing asked for (as a resume or a session leaving asks).
    #[cfg(test)]
    pub fn refresh_saved_for_test(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.refresh_saved(&mut effects);
        effects
    }

    /// The named list asked for (as a rename or an Open by name asks).
    #[cfg(test)]
    pub fn want_named_for_test(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.named.saved_queue.push_back(SavedAsk::Named);
        self.pump_saved(&mut effects);
        effects
    }

    #[cfg(test)]
    pub fn mark_unread_for_test(&mut self, id: &str) {
        self.unread.insert(id.to_string());
    }

    /// The favourite a held session is, if any.
    pub fn fav_of(&self, s: &SessionMeta) -> Option<usize> {
        self.favs.find(crate::favourites::Source::Live(s))
    }

    /// A favourite's state as its row shows it when nothing holds it:
    /// OPENING while its resume is on its way, UNREAD, else SAVED.
    pub fn fav_state(&self, f: &crate::favourites::Favourite) -> State {
        if self.named.resuming.as_ref().is_some_and(|r| f.knows(&r.req.target)) {
            State::Opening
        } else if f.ids().any(|id| self.unread.contains(id)) {
            State::Unread
        } else {
            State::Saved
        }
    }

    /// A live store is attached to this session.
    pub fn is_active(&self, session_id: &str) -> bool {
        self.lives.on(session_id).is_some()
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

    /// Store `live`'s session's list entry.
    pub fn active_meta(&self, live: LiveId) -> Option<&SessionMeta> {
        let a = self.lives[live].active.as_ref()?;
        self.sessions.iter().find(|s| s.id == a.session_id)
    }

    /// Store `live`'s session's name, if it has one.
    pub fn active_name(&self, live: LiveId) -> Option<&str> {
        let a = self.lives[live].active.as_ref()?;
        self.sessions.iter().find(|s| s.id == a.session_id)?.name.as_deref().filter(|n| !n.is_empty())
    }

    /// A live session's journal, to resume if it ended since the last list.
    pub(crate) fn journal_of(&self, session_id: &str) -> Option<ResumeRequest> {
        let s = self.sessions.iter().find(|s| s.id == session_id)?;
        let name = s.name.clone().filter(|n| !n.is_empty())?;
        let target = if s.journal_id.is_empty() { s.id.clone() } else { s.journal_id.clone() };
        Some(ResumeRequest { label: name, target, cwd: s.cwd.clone().filter(|c| !c.is_empty()), by_name: false })
    }

    /// A row was clicked: in the visible tab.
    pub fn open_row(&mut self, action: &RowAction) -> Vec<Effect> {
        self.open_row_in(action, Place::Current)
    }

    /// A row was clicked (Ctrl or middle: `Place::New`). A session open in
    /// a tab is focused there.
    pub fn open_row_in(&mut self, action: &RowAction, place: Place) -> Vec<Effect> {
        self.named.error = None;
        self.named.notice = None;
        match action {
            // Already the one on screen here (or on its way): nothing to do.
            // Another pane's (split panes): its attach effect focuses that
            // pane (`AppView::focus_session`; Shady #36 M1).
            RowAction::Attach(id) if self.lives[self.lives.shown()].is_wanted(id) => Vec::new(),
            RowAction::Attach(id) => vec![Effect::Attach { session_id: id.clone(), fallback: self.journal_of(id), open: false, place }],
            RowAction::Resume(req) if self.named.resuming.as_ref().is_some_and(|r| r.req.target == req.target) => Vec::new(),
            RowAction::Resume(req) => vec![Effect::Resume { req: req.clone(), open: false, place }],
        }
    }

    /// Enter in find, in this order: a session of exactly that name in the
    /// loaded lists; else the one row find left (`visible`: the rows the
    /// rail shows); else, with no row showing, the daemon (see
    /// [`Model::open_by_name`]). Several rows and no exact name: nothing
    /// opens, the user picks.
    #[cfg(test)]
    pub fn find_enter(&mut self, typed: &str, visible: &[RowAction]) -> Vec<Effect> {
        self.find_enter_in(typed, visible, Place::Current)
    }

    /// [`Model::find_enter`], opening in `place` (Ctrl+Enter: a new tab).
    pub fn find_enter_in(&mut self, typed: &str, visible: &[RowAction], place: Place) -> Vec<Effect> {
        let typed = typed.trim();
        if self.named.confirm.as_deref() != Some(typed) {
            self.named.confirm = None;
        }
        if typed.is_empty() {
            return Vec::new();
        }
        if self.has_name(typed) {
            return self.open_by_name_in(typed, place);
        }
        match visible {
            [one] => self.open_row_in(one, place),
            [] if validate_session_name(typed).is_ok() => self.open_by_name_in(typed, place),
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
    #[cfg(test)]
    pub fn open_by_name(&mut self, typed: &str) -> Vec<Effect> {
        self.open_by_name_in(typed, Place::Current)
    }

    /// [`Model::open_by_name`], opening in `place`.
    pub fn open_by_name_in(&mut self, typed: &str, place: Place) -> Vec<Effect> {
        let name = typed.trim();
        let n = &mut self.named;
        n.error = None;
        n.notice = None;
        n.opened = false;
        if let Err(e) = validate_session_name(name) {
            n.error = Some(e.to_string());
            return Vec::new();
        }
        n.opening = Some(Opening { name: name.to_string(), attach_seq: None, asked_saved: false, place });
        let mut effects = Vec::new();
        self.resolve_open(&mut effects);
        effects
    }

    /// Resolve a waiting Open by name against the lists (exact name).
    fn resolve_open(&mut self, effects: &mut Vec<Effect>) {
        let Some((name, asked_saved, place)) =
            self.named.opening.as_ref().filter(|o| o.attach_seq.is_none()).map(|o| (o.name.clone(), o.asked_saved, o.place))
        else {
            return;
        };
        if let Some(s) = self.sessions.iter().find(|s| s.name.as_deref() == Some(name.as_str())) {
            let id = s.id.clone();
            effects.push(Effect::Attach { fallback: self.journal_of(&id), session_id: id, open: true, place });
            return;
        }
        let n = &self.named;
        if n.saved_supported {
            if let Some(v) = n.saved.iter().chain(&n.recent).find(|v| v.name.as_deref() == Some(name.as_str())) {
                let req = ResumeRequest { label: name, target: v.id.clone(), cwd: v.cwd.clone().filter(|c| !c.is_empty()), by_name: false };
                effects.push(Effect::Resume { req, open: true, place });
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
        let Some((name, place)) = self.named.opening.as_ref().map(|o| (o.name.clone(), o.place)) else { return };
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
        effects.push(Effect::Resume { req, open: true, place });
    }

    /// `Effect::Attach` went out as `attach_seq`. Its fallback takes the
    /// slot unless another store's attach in flight holds it (split panes,
    /// Shady #48 L1: that pane's journal fallback stays; the slot is one,
    /// so this attach then goes without one).
    pub fn attaching(&mut self, live: LiveId, attach_seq: u64, fallback: Option<ResumeRequest>, open: bool) {
        self.sent_open(attach_seq, open);
        self.want(live, attach_seq);
        let theirs = self.attaches_of_others(live);
        if self.named.attaching.as_ref().is_none_or(|(s, _)| !theirs.contains(s)) {
            self.named.attaching = fallback.map(|f| (attach_seq, f));
        }
    }

    /// `Effect::Resume` went out as `attach_seq`: as [`Model::attaching`],
    /// another store's resume in flight keeps the slot.
    pub fn resuming(&mut self, live: LiveId, req: ResumeRequest, attach_seq: u64, open: bool) {
        self.sent_open(attach_seq, open);
        self.want(live, attach_seq);
        let theirs = self.attaches_of_others(live);
        if self.named.resuming.as_ref().is_none_or(|r| !theirs.contains(&r.attach_seq)) {
            self.named.error = None;
            self.named.resuming = Some(Resuming { req, attach_seq });
        }
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
    pub fn rename_request(&mut self, live: LiveId, typed: &str) -> Option<RenameRequest> {
        if self.named.rename.is_some() {
            return None;
        }
        let name = typed.trim();
        let same = self.active_name(live) == Some(name);
        let n = &mut self.named;
        n.replace = None;
        let Some(a) = &self.lives[live].active else {
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
    pub fn want(&mut self, live: LiveId, attach_seq: u64) {
        let l = &mut self.lives[live];
        l.wanted = Some(attach_seq);
        // `Client::attach`/`create`/`resume` replaced what it held.
        l.held = Some(attach_seq);
        l.wanted_session = None;
        l.wanted_fact = None;
        self.rail_gen += 1;
        // Another store's attach in flight is that pane's (Shady #44 n11).
        let theirs = self.attaches_of_others(live);
        let n = &mut self.named;
        n.resuming.take_if(|r| r.attach_seq != attach_seq && !theirs.contains(&r.attach_seq));
        n.attaching.take_if(|(s, _)| *s != attach_seq && !theirs.contains(s));
        n.opening.take_if(|o| o.attach_seq != Some(attach_seq) && !o.attach_seq.is_some_and(|s| theirs.contains(&s)));
        n.naming = false;
        n.replace = None;
        n.rename_error = None;
    }

    /// The attachments the stores other than `live` asked for or hold: a
    /// resume, a fallback attach or an Open by name sent as one of those is
    /// that store's pane's, and nothing `live` does cancels it (split
    /// panes, Shady #44 n11: a Join, a close or a move in one pane left the
    /// other pane's by-name open in flight forgotten).
    fn attaches_of_others(&self, live: LiveId) -> Vec<u64> {
        self.lives
            .ids()
            .into_iter()
            .filter(|&id| id != live)
            .flat_map(|id| [self.lives[id].wanted, self.lives[id].held])
            .flatten()
            .collect()
    }

    /// The attach just asked for (after [`Model::want`]) is of this session.
    pub fn asked_for(&mut self, live: LiveId, session_id: &str) {
        let fact = self.sessions.iter().find(|s| s.id == session_id).map(|s| self.fact(s));
        let l = &mut self.lives[live];
        l.wanted_session = Some(session_id.to_string());
        l.wanted_fact = fact;
    }

    /// The last attach a live store asked for is of this session (attached
    /// or on its way): a click on it again has nothing to do.
    pub fn is_wanted(&self, session_id: &str) -> bool {
        self.lives.iter().any(|l| l.is_wanted(session_id))
    }

    /// The visible tab shows this session (the rail's marker).
    pub fn is_shown(&self, session_id: &str) -> bool {
        self.tabs.visible_session() == Some(session_id)
    }

    /// The visible tab is leaving its session: what the user saw of it is
    /// the baseline UNREAD compares the next poll with; its transcript and
    /// numbers are handed over (to the tab cache); nothing of its
    /// attachment carries over. Until the next session attaches nothing
    /// can be sent, answered or halted (`active` is `None`): no input, no
    /// card, no HALT can reach the session left or the one to come.
    pub fn switch_out(&mut self, live: LiveId) -> (Transcript, Stats) {
        self.saw_active(live);
        let theirs = self.attaches_of_others(live);
        let l = &mut self.lives[live];
        l.wanted = None;
        l.wanted_session = None;
        l.wanted_fact = None;
        l.answered.clear();
        // Sent to the session left: its echo or refusal still comes, under
        // that attachment's seq; a refusal goes back to that tab's draft.
        if let Some(a) = l.active.take()
            && !l.in_flight.is_empty()
        {
            if self.left.len() == LEFT_MAX {
                self.left.remove(0);
            }
            self.left.push((a.attach_seq, a.session_id, l.in_flight.take().into_iter().collect()));
        }
        l.in_flight.take();
        l.reattaching = false;
        l.last_error = None;
        let n = &mut self.named;
        n.naming = false;
        n.replace = None;
        n.rename_error = None;
        // A resume or an attach in flight was the attachment left's, unless
        // another store's (another pane's) asked for it.
        n.resuming.take_if(|r| !theirs.contains(&r.attach_seq));
        n.attaching.take_if(|(s, _)| !theirs.contains(s));
        n.opening.take_if(|o| o.attach_seq.is_some_and(|s| !theirs.contains(&s)));
        self.rail_gen += 1;
        self.view_dirty = true;
        // Its worker views go with it (the view takes them); nothing polls.
        let l = &mut self.lives[live];
        l.watch.suspend();
        (std::mem::take(&mut l.transcript), std::mem::take(&mut l.stats))
    }

    /// The visible tab's session, as last seen (a cached tab) or nothing
    /// yet; its attach follows.
    pub fn switch_in(&mut self, live: LiveId, transcript: Transcript, stats: Stats) {
        let l = &mut self.lives[live];
        l.transcript = transcript;
        l.stats = stats;
        self.view_dirty = true;
    }

    /// The visible tab shows `session_id`, whose attach is deferred (a burst
    /// of tab switches attaches once): OPENING meanwhile, a click on it is
    /// a no-op.
    pub fn showing(&mut self, live: LiveId, session_id: &str) {
        let fact = self.sessions.iter().find(|s| s.id == session_id).map(|s| self.fact(s));
        let l = &mut self.lives[live];
        l.wanted = None;
        l.wanted_session = Some(session_id.to_string());
        l.wanted_fact = fact;
        self.rail_gen += 1;
    }

    /// If a tab's session ended since the last list: its journal, resumed
    /// in the tab instead (a named one's as its row would; else what the
    /// tab last saw listed).
    pub fn tab_fallback(&self, session_id: &str) -> Option<ResumeRequest> {
        self.journal_of(session_id).or_else(|| {
            let t = self.tabs.iter().find(|t| t.session.as_deref() == Some(session_id))?;
            let target = t.journal.clone()?;
            Some(ResumeRequest { label: t.label.clone(), target, cwd: t.cwd.clone(), by_name: false })
        })
    }

    /// A local session (benches): its events carry [`LOCAL_SEQ`].
    pub fn local_session(&mut self, live: LiveId, session_id: &str) {
        let l = &mut self.lives[live];
        l.wanted = Some(LOCAL_SEQ);
        l.wanted_session = Some(session_id.into());
        self.tabs.attached(session_id);
        self.epoch += 1;
        let l = &mut self.lives[live];
        l.active = Some(Active {
            session_id: session_id.into(),
            attach_seq: LOCAL_SEQ,
            epoch: self.epoch,
            connection: 0,
            mode: AttachMode::Mirror,
            client: 0,
            owner: None,
            input: ClientInput::Shared,
        });
        l.stats = Stats::default();
    }

    /// [`Model::local_session`] in the one store (tests only, as the
    /// protected tests call it).
    #[cfg(test)]
    pub fn set_local_session(&mut self, session_id: &str) {
        self.local_session(LiveId::FIRST, session_id)
    }

    /// A worker tab of the session on screen shows `handle`.
    pub fn holds_view(&self, live: LiveId, handle: &str) -> bool {
        let Some(s) = self.lives[live].active.as_ref().map(|a| a.session_id.as_str()) else { return false };
        self.tabs.iter().any(|t| {
            t.kind == crate::tabs::Kind::Worker && t.session.as_deref() == Some(s) && t.worker.as_ref().is_some_and(|w| w.handle == handle)
        })
    }

    /// The handles the session on screen's open worker tabs show: their
    /// views stay while the tabs do ([`Watch::hold`]).
    pub fn held_views(&self, live: LiveId) -> Vec<String> {
        let Some(s) = self.lives[live].active.as_ref().map(|a| a.session_id.as_str()) else { return Vec::new() };
        self.tabs
            .iter()
            .filter(|t| t.kind == crate::tabs::Kind::Worker && t.session.as_deref() == Some(s))
            .filter_map(|t| t.worker.as_ref().map(|w| w.handle.clone()))
            .collect()
    }

    /// The session on screen as the user sees it (its transcript): the
    /// baseline UNREAD compares the next poll with.
    fn saw_active(&mut self, live: LiveId) {
        let l = &self.lives[live];
        let Some(a) = &l.active else { return };
        if l.transcript.session_id() != Some(a.session_id.as_str()) {
            return;
        }
        let now = (l.transcript.history_len(), l.transcript.busy());
        match self.seen.get_mut(&a.session_id) {
            Some(e) => *e = now,
            None => {
                self.seen.insert(a.session_id.clone(), now);
            }
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

    /// An event of an attachment a tab switch left, while it still has
    /// input unconfirmed: an echo confirms it, a refusal gives the oldest
    /// back, the end gives back the rest (to that tab's draft). `None`:
    /// not one of those.
    fn left_event(&mut self, ev: &Event) -> Option<Vec<Effect>> {
        let seq = ev.attach_seq().filter(|&s| self.lives.wanting(s).is_none())?;
        let i = self.left.iter().position(|l| l.0 == seq)?;
        let mut effects = Vec::new();
        let now = std::time::Instant::now();
        let (_, session, texts) = &mut self.left[i];
        let memos = &mut self.memos;
        use crate::live::Target;
        match ev {
            Event::Session {
                event: SessionEvent::TurnStarted { user_text: Some(t), .. } | SessionEvent::Steered { text: t, .. }, ..
            } => {
                if let Some(k) = texts.iter().position(|(_, s)| s.trim() == t.trim()) {
                    // A card's answer, echoed: its memo hears it (a
                    // driver's echo can't be told here: no state of that
                    // session is kept).
                    if let (Target::Card(_), s) = texts.remove(k).expect("found") {
                        memos.on(session, cards_spec::memo::Event::Echo { text: &s, driver: false }, now);
                    }
                }
            }
            Event::Error { error, .. } if refused_input(error) => match texts.pop_front() {
                Some((Target::Pane(pane), text)) => effects.push(Effect::Unsent { pane, session: session.clone(), text }),
                Some((Target::Card(_), _)) => {
                    memos.on(session, cards_spec::memo::Event::Refused { reason: &error.to_string() }, now);
                }
                None => {}
            },
            // The rest, one text for each pane that sent some. A card's
            // answer stays its card's: its memo checks it when the session
            // is attached again, or says it couldn't.
            Event::Detached { .. } if !texts.is_empty() => {
                let mut by: Vec<(PaneId, Vec<String>)> = Vec::new();
                for (p, t) in std::mem::take(texts) {
                    let Target::Pane(p) = p else { continue };
                    match by.iter_mut().find(|(q, _)| *q == p) {
                        Some((_, ts)) => ts.push(t),
                        None => by.push((p, vec![t])),
                    }
                }
                for (pane, ts) in by {
                    effects.push(Effect::Unsent { pane, session: session.clone(), text: ts.join("\n") });
                }
            }
            _ => {}
        }
        if self.left[i].2.is_empty() || matches!(ev, Event::Detached { .. }) {
            self.left.remove(i);
        }
        Some(effects)
    }

    /// Apply one event.
    pub fn apply(&mut self, ev: Event) -> Vec<Effect> {
        // The wait cards it brings, as it left its attachment's fold: the
        // attach's (there before) or come live (D3's notifications).
        let look = crate::presence::Look::of(&ev);
        let effects = self.apply_event(ev);
        if let Some((how, seq)) = look
            && let Some(lid) = self.lives.ids().into_iter().find(|l| self.lives[*l].current(seq))
            && let Some(a) = self.lives[lid].active.as_ref()
        {
            self.arrivals.look(how, &a.session_id, &self.lives[lid].transcript);
        }
        effects
    }

    fn apply_event(&mut self, ev: Event) -> Vec<Effect> {
        if let Some(effects) = self.left_event(&ev) {
            return effects;
        }
        // The store that asked for this attachment. An event of no store's
        // attachment (one left, replaced or ended) is owed only what
        // `left_event` did with it: it reaches no store, draws nothing
        // (Shady #32 M1; split/architecture.md §3.3). One that names no
        // attachment (a control connection's) acts on the store the
        // visible tab shows: step 2 has only that one; step 3 routes those.
        let lid = match ev.attach_seq() {
            Some(seq) => match self.lives.wanting(seq) {
                Some(lid) => lid,
                None => return Vec::new(),
            },
            None => self.lives.shown(),
        };
        let mut effects = Vec::new();
        // The right panel's cache and keep-warm numbers are asked again
        // (attach, a turn's end, a keep-warm notice): the store's own
        // attachment only, from a daemon that says when the cache expires.
        if let Some(seq) = crate::warm::asks(&ev).filter(|s| self.cache_expiry && self.lives[lid].current(*s)) {
            effects.push(Effect::Status(seq));
        }
        let ev = match ev {
            Event::Sessions(mut s) => {
                self.lists += 1;
                self.sessions_listed = true;
                sort_sessions(&mut s);
                self.poll_held(&s, &mut effects);
                // The poll of an idle daemon: nothing to rebuild or draw.
                if s == self.sessions {
                    log::debug!(target: "synaps_desktop::sessions", "list {}: unchanged", self.lists);
                    return effects;
                }
                log::debug!(target: "synaps_desktop::sessions", "list {}: {} sessions", self.lists, s.len());
                Event::Sessions(s)
            }
            // A worker's page: a frame only if it changed what the view
            // shows (an idle worker's empty pages, 4 a second, draw
            // nothing), and nothing of the rail's.
            Event::SubagentWatch { attach_seq, handle_id, since, reply, .. } => {
                if self.lives[lid].current(attach_seq) {
                    let heard = self.lives[lid].watch.on_reply(&handle_id, since, reply, std::time::Instant::now());
                    // The page it took is a live source of the session's
                    // list too (a reply it dropped says nothing).
                    let fed = heard.page.as_deref().is_some_and(|word| self.lives[lid].heard_view(&handle_id, word));
                    match heard.page.as_deref() {
                        Some(word) if fed => {
                            log::debug!(target: "synaps_desktop::workers", "{handle_id}@{since}: its page says {word}: the list takes it")
                        }
                        None if !heard.shown => {
                            log::debug!(target: "synaps_desktop::workers", "{handle_id}@{since}: reply dropped (not the poll in flight)")
                        }
                        _ => {}
                    }
                    if fed || heard.shown {
                        self.view_dirty = true;
                    }
                }
                return effects;
            }
            // A poll that could not go: again later (nothing shows it).
            Event::Error { error: Error::Refused { command: Some(c), .. }, .. } if c == "subagent_watch" => {
                self.lives[lid].watch.poll_failed(std::time::Instant::now());
                return effects;
            }
            Event::Error { error: Error::NotAttached { command: "subagent_watch" }, .. } => {
                self.lives[lid].watch.poll_failed(std::time::Instant::now());
                return effects;
            }
            Event::Error { error: Error::Decode(d), .. } if d.kind == "query_result:subagent_watch" => return effects,
            // A worker steer refused: its view says why and keeps the text
            // for the composer ([`Watch::take_unsent`]).
            Event::Error { attach_seq, error: Error::Refused { command: Some(c), reason } }
                if c == "subagent_steer" && attach_seq.is_none_or(|s| self.lives[lid].current(s)) =>
            {
                self.view_dirty |= self.lives[lid].watch.steer_refused(&reason);
                return effects;
            }
            Event::Error { error: Error::NotAttached { command: "subagent_steer" }, .. } => {
                self.view_dirty |= self.lives[lid].watch.steer_refused("not attached to the session");
                return effects;
            }
            // A worker cancel answered: done, its rows will say so (it is
            // `cancelling` until then); not done, the composer's note says why.
            Event::WorkerCancel { attach_seq, handle_id, result } => {
                log::debug!(target: "synaps_desktop::watch", "cancel {handle_id}: {result:?}");
                if let Some(note) = self.lives[lid].cancels.answered(attach_seq, &handle_id, &result) {
                    effects.push(Effect::WorkerNote(note));
                    self.view_dirty = true;
                }
                return effects;
            }
            // A slash command answered (the palette's, the composer's): its
            // whole answer at the transcript's end, as the TUI prints it
            // (this client's row: never sent, not in the history), and a
            // line of it on the composer's note; another attachment's (a
            // switch since) says nothing here.
            Event::EngineReply { attach_seq, name, reply } => {
                log::debug!(target: "synaps_desktop::send", "/{name}: {reply:?}");
                if self.lives[lid].active.as_ref().is_some_and(|a| a.attach_seq == attach_seq) {
                    if let synaps_client::CommandReply::Done(text) = &reply
                        && !text.trim().is_empty()
                    {
                        self.lives[lid].transcript.output(&name, text);
                    }
                    effects.push(Effect::Command { name, reply });
                    self.view_dirty = true;
                }
                return effects;
            }
            // A pick answered: the picker's ([`crate::choice`]); the
            // label follows `setting_changed`.
            Event::SettingReply { attach_seq, setting, reply } => {
                log::debug!(target: "synaps_desktop::send", "set {setting}: {reply:?}");
                effects.push(Effect::Setting { attach_seq, setting, reply });
                self.view_dirty = true;
                return effects;
            }
            Event::SessionStatus { attach_seq, value } => {
                if self.lives[lid].current(attach_seq) {
                    self.lives[lid].stats.warm = Some(crate::warm::Warm::from_status(&value, crate::warm::Moment::now()));
                    self.view_dirty = true;
                }
                return effects;
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
                self.named.page = None;
                if let Status::Connected { features, profile, daemon_version, pid, .. } = &s {
                    self.daemon = Some(Daemon { version: daemon_version.clone(), profile: profile.clone(), pid: *pid });
                    self.daemon_gen += 1;
                    // Another daemon process: the worker on screen is read
                    // from 0, even before its session's attach comes back.
                    for l in self.lives.ids() {
                        let held = self.held_views(l);
                        self.lives[l].watch.hold(held);
                        self.lives[l].watch.daemon_changed(*pid, std::time::Instant::now());
                    }
                    self.named.daemon = match profile {
                        Some(p) => format!("the daemon's machine (profile {p})"),
                        None => "the daemon's machine".into(),
                    };
                    // Pages when the daemon has them; `saved_sessions` (#408,
                    // on local builds only) until it does not.
                    self.named.pages = features.iter().any(|f| f == FEATURE_SESSION_PAGE);
                    self.cache_expiry = features.iter().any(|f| f == synaps_client::FEATURE_CACHE_EXPIRY);
                    self.named.saved_supported = self.named.pages || features.iter().any(|f| f == FEATURE_SAVED_SESSIONS);
                    self.named.polled = None;
                    self.named.retries = 0;
                    self.named.deep = self.named.deep_wanted;
                    self.named.named_stale = true;
                    if self.named.saved_supported {
                        self.refresh_saved(&mut effects);
                    } else {
                        self.clear_saved();
                    }
                }
                match &s {
                    Status::Reconnecting { why } => {
                        self.reload = why == synaps_client::RELOADING;
                        self.reload_seq += u64::from(self.reload);
                    }
                    Status::Connected { .. } => self.reload = false,
                    _ => {}
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
                self.saw_active(lid);
                // UNREAD: off screen, a turn ended or messages came that the
                // user has not seen (their own turns too, if they left them
                // running). Not news: a session a TUI is attached to (it is
                // on a screen there).
                for new in &s {
                    if self.is_active(&new.id) {
                        continue;
                    }
                    let elsewhere = new.roster.iter().any(|c| c.kind == "tui");
                    if let Some(&(messages, busy)) = self.seen.get(&new.id)
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
                self.tabs.listed(&s);
                // A row leaving OPEN (closed, or evicted to history).
                let left = self.sessions.iter().any(|o| !s.iter().any(|n| n.id == o.id));
                self.sessions = s;
                if left && !self.named.pages {
                    self.refresh_saved(&mut effects);
                }
                if !self.named.saved_supported {
                    self.prune_unread();
                }
            }
            Event::SavedSessions { sessions, truncated, rejected } => {
                // A page fetch waits for pages, not for this.
                if self.named.page.is_none() {
                    self.saved_landed(sessions, truncated, rejected, &mut effects);
                }
            }
            Event::CatalogPage { req, sessions, next, rejected } => self.on_page(req, sessions, next, rejected, &mut effects),
            Event::CatalogPageFailed { req, reason } => {
                if self.named.page.as_ref().is_some_and(|f| f.req == Some(req)) {
                    self.on_saved_failed(&reason, &mut effects);
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
                if self.lives[lid].active.as_ref().is_some_and(|a| a.session_id == f.session_id) {
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
            Event::CwdFallback { attach_seq, missing, .. } if self.lives[lid].current(attach_seq) => {
                self.named.notice = Some(format!("Folder {missing} is gone on {}; resumed in ~.", self.named.daemon));
            }
            Event::Attached { attach_seq, attached, mode, .. } if self.lives[lid].current(attach_seq) => {
                let resumed = self.named.resuming.take_if(|r| r.attach_seq == attach_seq);
                if let Some(r) = &resumed {
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
                // The journal it holds is the header's: the attach's
                // `meta.journal_id` is the one the session started with
                // (0.11.1, after a swap). The daemon's word beats the
                // listing's (polled, up to seconds old): its row and the
                // favourite key act on what it holds.
                let journal =
                    attached.conversation.header.get("id").and_then(serde_json::Value::as_str).unwrap_or(&attached.meta.journal_id);
                self.holds(&attached.meta.id, journal);
                // A compaction of a favourite's conversation, said by the
                // daemon: the header names its parent (one not seen while
                // it ran), or this session is a resume the daemon followed
                // down the chain to the journal it still holds (any number
                // of compactions not seen here): the favourite follows.
                if self.favs.continued(&attached.conversation.header)
                    | attached.meta.continue_info.as_ref().is_some_and(|i| self.favs.resolved(i, journal))
                {
                    self.rail_gen += 1;
                }
                // A favourite's resume the daemon answered with a session
                // that holds another journal it does not say continues this
                // one (its live session `/clear`ed or `/resume`d elsewhere,
                // or compacted more than once while live: a live session is
                // handed back by its live id): the favourite stays on its
                // own conversation, and the user is told.
                if let Some(r) = resumed.filter(|r| !r.req.by_name)
                    && !journal.is_empty()
                    && self.favs.position(&r.req.target).is_some_and(|i| !self.favs.list[i].knows(journal))
                {
                    let (label, wanted) = (attached.meta.label(), &r.req.label);
                    self.named.notice = Some(if self.favs.position(journal).is_some() {
                        format!(
                            "The daemon opened {label} ({journal}) for '{wanted}': another favourite has that conversation, so both stay as they were."
                        )
                    } else {
                        format!(
                            "The daemon opened {label} ({journal}) for '{wanted}', and does not say it is the same conversation: the favourite stays as it was."
                        )
                    });
                }
                // The one on screen until now, as the user saw it.
                self.saw_active(lid);
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
                // The session this attach was asked for (`asked_for`,
                // `showing`), before it becomes the one that attached.
                let asked = self.lives[lid].wanted_session.take();
                self.lives[lid].active = Some(Active {
                    session_id: attached.meta.id.clone(),
                    attach_seq,
                    epoch: self.epoch,
                    connection: attached.connection,
                    mode,
                    client: attached.client,
                    owner: attached.input_owner,
                    input: attached.client_input,
                });
                self.lives[lid].wanted_session = Some(attached.meta.id.clone());
                // The visible tab holds what attached (a create or a resume
                // names it here): its own store's attach only. Another
                // store's (step 3's other pane) leaves the tab strip be, or
                // the visible tab would be taken to that session and its
                // own transcript stashed.
                if lid == self.lives.shown() {
                    self.tabs.attached(&attached.meta.id);
                }
                // Whatever was answered belongs to the attachment before.
                self.lives[lid].answered.clear();
                self.lives[lid].reattaching = false;
                self.lives[lid].last_error = None;
                // Another session's worker views mean nothing here; the
                // same session's (a reconnect, its tab back) watch again.
                // So does the view of a tab whose cache missed: its
                // transcript is new (no session yet), its view what the tab
                // showed when it was left, for the session asked for. A
                // follow to a new id (a resume's fallback) still resets.
                if self.lives[lid].transcript.session_id().or(asked.as_deref()) != Some(attached.meta.id.as_str()) {
                    self.lives[lid].watch = crate::watch::Watch::default();
                }
                let held = self.held_views(lid);
                self.lives[lid].watch.hold(held);
                self.lives[lid].watch.on_reconnect(std::time::Instant::now(), self.daemon.as_ref().map(|d| d.pid));
                if let Err(e) = self.lives[lid].transcript.load_attached(&attached) {
                    self.lives[lid].last_error = Some(e.to_string());
                }
                // A card's answer out when the link dropped (or the tab was
                // left) may be lost: its memo checks it on this view.
                let now = std::time::Instant::now();
                self.memos.on(&attached.meta.id, cards_spec::memo::Event::Attached, now);
                self.memos.observe(&attached.meta.id, &self.lives[lid].transcript, now);
                self.lives[lid].stats.on_attached(&attached);
                effects.push(Effect::ListSessions);
            }
            Event::Reattaching { attach_seq, .. } if self.lives[lid].current(attach_seq) => self.lives[lid].reattaching = true,
            Event::InputOwner { attach_seq, client, owner, .. } if self.lives[lid].current(attach_seq) => {
                if let Some(a) = self.lives[lid].active.as_mut() {
                    (a.client, a.owner) = (client, owner);
                    self.view_dirty = true;
                }
            }
            Event::Session { attach_seq, ts, event, meta, raw, .. } if self.lives[lid].current(attach_seq) => {
                // The engine's note of a worker steer in the main conversation.
                if raw["ev"] == "external" && raw["event"]["content"]["content_type"] == "subagent_steer" {
                    self.lives[lid].watch.steer_accepted(raw["event"]["content"]["data"]["handle_id"].as_str().unwrap_or(""));
                }
                if self.lives[lid].stats.on_session(&event, &raw) && self.cache_expiry {
                    effects.push(Effect::Status(attach_seq));
                }
                // After a swap (a compaction, `/clear`, `/resume`): only a
                // compaction's successor names its parent. The journal it
                // holds now, at once (the listing is polled).
                if let SessionEvent::Conversation { digest } = &event {
                    if let Some(id) = digest.header.get("id").and_then(serde_json::Value::as_str)
                        && let Some(a) = self.lives[lid].active.as_ref().map(|a| a.session_id.clone())
                    {
                        self.holds(&a, id);
                    }
                    if self.favs.continued(&digest.header) {
                        self.rail_gen += 1;
                    }
                }
                match &event {
                    SessionEvent::TurnStarted { user_text: Some(t), .. } | SessionEvent::Steered { text: t, .. } => {
                        self.lives[lid].in_flight.confirm(t);
                    }
                    _ => {}
                }
                // What the agent cards' answers hear of it (as it was before
                // this event: a driver's echo is one while idle).
                let heard = self.lives[lid]
                    .active
                    .as_ref()
                    .map(|a| a.session_id.clone())
                    .map(|s| (s, crate::deliver::heard(&event, &raw, &self.lives[lid].transcript)));
                // With what it says beyond its typed form (a prompt's
                // outcome, SynapsCLI #437: the receipt takes its word).
                self.lives[lid].transcript.apply_with(&event, &ts, &meta);
                if let Some((sid, heard)) = heard {
                    let now = std::time::Instant::now();
                    if let Some(h) = heard {
                        self.memos.on(&sid, h, now);
                    }
                    // The view after it, but not after a `Dequeued`: what
                    // the cancel left comes next (its history and digest, or
                    // at once its `Idle`: a cancelled compaction sends no
                    // digest, SynapsCLI `189a5a19` `actor.rs:3508-3518`), and
                    // the memo reads it there (§3.4 d4). The slot is empty
                    // from the `Dequeued` on (`Transcript::slot_known`).
                    if !matches!(heard, Some(cards_spec::memo::Event::Dequeued { .. })) {
                        self.memos.observe(&sid, &self.lives[lid].transcript, now);
                    }
                }
                let l = &mut self.lives[lid];
                if !l.answered.is_empty() {
                    let pending = l.transcript.pending_prompts();
                    l.answered.retain(|(_, id)| pending.iter().any(|p| p.id == *id));
                }
            }
            Event::History { attach_seq, update, .. } if self.lives[lid].current(attach_seq) => {
                if self.lives[lid].transcript.apply_update(&update).is_err() {
                    effects.push(Effect::Resync(attach_seq));
                } else {
                    self.lives[lid].stats.on_history(&update);
                    if let Some(a) = self.lives[lid].active.as_ref() {
                        self.memos.observe(&a.session_id, &self.lives[lid].transcript, std::time::Instant::now());
                    }
                }
            }
            Event::Detached { attach_seq, reason, .. } if self.lives[lid].current(attach_seq) => {
                self.saw_active(lid);
                self.lives[lid].active = None;
                self.lives[lid].wanted_session = None;
                self.lives[lid].answered.clear();
                self.lives[lid].reattaching = false;
                self.lives[lid].transcript.set_detached();
                self.lives[lid].stats.on_detached();
                self.lives[lid].watch.suspend();
                self.lives[lid].give_back_all(&mut effects);
                let n = &mut self.named;
                // A named live session that ended since the last list:
                // resume its journal (an Open by name keeps waiting).
                if let Some((_, req)) = n.attaching.take_if(|(s, _)| *s == attach_seq)
                    && reason == UNKNOWN_SESSION
                {
                    let open = n.opening.as_mut().filter(|o| o.attach_seq == Some(attach_seq)).map(|o| o.attach_seq = None).is_some();
                    effects.push(Effect::Resume { req, open, place: Place::Current });
                    return effects;
                }
                let opening = n.opening.take_if(|o| o.attach_seq == Some(attach_seq));
                match n.resuming.take_if(|r| r.attach_seq == attach_seq) {
                    Some(r) => {
                        n.error = Some(crate::named::resume_error(&r.req.label, &reason));
                        // A favourite whose journal is not there (the
                        // daemon's own refusal for that id, nothing else):
                        // its row says so (and offers Remove); never dropped
                        // here. A fresh listing is asked for: only one asked
                        // after this says it is there after all.
                        // Said again on a second click of a gone row.
                        if !r.req.by_name
                            && crate::favourites::journal_missing(&reason, &r.req.target)
                            && self.favs.position(&r.req.target).is_some()
                        {
                            n.error = Some(format!(
                                "'{}' is gone: the daemon has no journal for it. It stays in FAVOURITES until you remove it.",
                                r.req.label
                            ));
                            if self.favs.set_gone(&r.req.target, n.asks) {
                                self.rail_gen += 1;
                                self.refresh_saved(&mut effects);
                            }
                        }
                    }
                    None => match opening {
                        Some(o) => n.error = Some(format!("Could not open '{}': {reason}", o.name)),
                        None => self.lives[lid].last_error = Some(format!("detached: {reason}")),
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
            Event::Error { attach_seq, error } if attach_seq.is_none_or(|s| self.lives[lid].current(s)) => {
                // An answer the daemon may not have got (a refusal that names
                // no command could be one): the card asks again. A repeat is
                // harmless, the daemon takes the first answer per prompt.
                let answer = matches!(&error, Error::Refused { command: Some(c), .. } if c == "answer")
                    || matches!(&error, Error::NotAttached { command: "answer" });
                if answer || matches!(&error, Error::Refused { command: None, .. }) {
                    self.lives[lid].answered.clear();
                }
                if refused_input(&error) && self.lives[lid].give_back_oldest(&mut effects) {
                    // A card's answer: not sent (§3.5), Retry.
                    if let Some(a) = self.lives[lid].active.as_ref() {
                        let reason = error.to_string();
                        self.memos.on(&a.session_id, cards_spec::memo::Event::Refused { reason: &reason }, std::time::Instant::now());
                    }
                }
                self.lives[lid].last_error = Some(error.to_string());
            }
            _ => {}
        }
        effects
    }

    /// A daemon without `saved_sessions`: no lists, nothing in flight.
    fn clear_saved(&mut self) {
        let n = &mut self.named;
        n.saved_supported = false;
        n.pages = false;
        n.page = None;
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
        Event::session(attach_seq, "s", 0, event, Arc::new(Value::Null))
    }

    fn attach(m: &mut Model, seq: u64) {
        m.want(LiveId::FIRST, seq);
        m.apply(Event::Attached { attach_seq: seq, attached: attached("s"), mode: AttachMode::Mirror, reconnect: false });
    }

    /// A worker cancel's answer: refused or not done, the composer's note
    /// says why and the worker is not `cancelling` any more; done, it
    /// stays so until its rows say it.
    #[test]
    fn a_worker_cancel_answer_says_why_it_was_not_done() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        let t0 = std::time::Instant::now();
        m.cancels.sent("s", "sa_2", 1, t0);
        m.cancels.sent("s", "sa_3", 1, t0);
        let answer = |h: &str, result: Result<String, String>| Event::WorkerCancel { attach_seq: 1, handle_id: h.into(), result };
        assert!(
            m.apply(answer("sa_2", Ok("Cancelled sa_2 (they stop at their next step; no completion wake is sent).".into()))).is_empty()
        );
        assert!(m.cancels.pending("s", "sa_2"));
        let fx = m.apply(answer("sa_3", Err("refused: input is owned by client #1".into())));
        assert_eq!(fx, vec![Effect::WorkerNote("sa_3 not cancelled: refused: input is owned by client #1".into())]);
        assert!(!m.cancels.pending("s", "sa_3"));
        assert_eq!(m.last_error, None, "a cancel's answer is not a session error");
    }

    /// L5: who may steer or cancel a worker is who may send the session
    /// input, as the daemon rules it, and the reason given is the one that
    /// applies: an exclusive session's owner only (it follows the owner as
    /// it changes), a shared one's every client but an observer.
    #[test]
    fn the_input_gate_says_why_not() {
        let mut m = Model::new(ConnState::Connecting);
        assert_eq!(m.input_gate(), Err("not attached"));
        attach(&mut m, 1);
        assert_eq!(m.input_gate(), Ok(()), "client 2 owns its input");
        let owner = |to| Event::InputOwner { attach_seq: 1, session_id: "s".into(), client: 2, owner: to, reason: "takeover".into() };
        m.apply(owner(Some(1)));
        assert_eq!(m.input_gate(), Err("read-only here: another client owns this session's input"));
        m.apply(owner(Some(2)));
        assert_eq!(m.input_gate(), Ok(()));
        m.apply(Event::Reattaching { attach_seq: 1, session_id: "s".into(), reason: "x".into() });
        assert_eq!(m.input_gate(), Err("reconnecting to the session: try again in a moment"));
        // Shared: any client that drives, whoever owns it.
        let v = json!({"type": "attached", "client": 3, "meta": {"id": "s"}, "conversation": {"api_messages": []}, "input_owner": 1, "client_input": "shared"});
        let a = match synaps_client::protocol::decode_frame(&v.to_string()).unwrap() {
            synaps_client::protocol::DaemonFrame::Attached(a) => a,
            other => panic!("{other:?}"),
        };
        m.apply(Event::Attached { attach_seq: 1, attached: a.clone(), mode: AttachMode::Mirror, reconnect: true });
        assert_eq!(m.input_gate(), Ok(()));
        m.want(LiveId::FIRST, 2);
        m.apply(Event::Attached { attach_seq: 2, attached: a, mode: AttachMode::Observe, reconnect: false });
        assert_eq!(m.input_gate(), Err("read-only: this window observes the session"));
    }

    /// L2: leaving a tab drops the resume, the fallback attach and the
    /// Open by name that were that attachment's (closing the last tab
    /// mid-resume would otherwise leave its row OPENING and unclickable);
    /// an Open by name not sent yet stays.
    #[test]
    fn leaving_a_tab_drops_what_its_attachment_had_in_flight() {
        let req = |t: &str| ResumeRequest { label: t.into(), target: t.into(), cwd: None, by_name: false };
        let opening = |attach_seq| Opening { name: "n".into(), attach_seq, asked_saved: false, place: Place::Current };
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        m.named.resuming = Some(Resuming { req: req("j"), attach_seq: 2 });
        m.named.attaching = Some((2, req("k")));
        m.named.opening = Some(opening(Some(2)));
        let _ = m.switch_out(LiveId::FIRST);
        assert!(m.named.resuming.is_none() && m.named.attaching.is_none() && m.named.opening.is_none());
        m.named.opening = Some(opening(None));
        let _ = m.switch_out(LiveId::FIRST);
        assert!(m.named.opening.is_some(), "not sent yet: still to open");
    }

    /// Shady #44 n11: a store leaving its session (a Join, a pane's close, a
    /// tab moved out) drops only its own attachment's resume, fallback
    /// attach and Open by name: another pane's, in flight, stays; and a new
    /// attach asked in one pane leaves the other pane's alone too.
    #[test]
    fn leaving_a_tab_keeps_another_panes_open_by_name_in_flight() {
        let req = |t: &str| ResumeRequest { label: t.into(), target: t.into(), cwd: None, by_name: true };
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        let other = m.lives.add();
        m.named.opening = Some(Opening { name: "notes".into(), attach_seq: None, asked_saved: true, place: Place::Current });
        m.resuming(other, req("notes"), 2, true);
        m.named.attaching = Some((2, req("notes")));
        let _ = m.switch_out(LiveId::FIRST);
        assert_eq!(m.named.resuming.as_ref().map(|r| r.attach_seq), Some(2), "the other pane's resume");
        assert_eq!(m.named.attaching.as_ref().map(|a| a.0), Some(2), "the other pane's fallback attach");
        assert_eq!(m.named.opening.as_ref().and_then(|o| o.attach_seq), Some(2), "the other pane's Open by name");
        // Through the attach path every pane's attach takes (Shady #48 L1).
        m.attaching(LiveId::FIRST, 3, None, false);
        assert_eq!(m.named.attaching.as_ref().map(|a| a.0), Some(2), "a plain attach in this pane leaves the other's fallback");
        m.attaching(LiveId::FIRST, 4, Some(req("todo")), false);
        assert_eq!(m.named.attaching.as_ref().map(|a| a.0), Some(2), "and so does one with its own fallback");
        m.resuming(LiveId::FIRST, req("todo"), 5, false);
        assert!(
            m.named.resuming.as_ref().is_some_and(|r| r.attach_seq == 2) && m.named.opening.is_some(),
            "a resume in this pane leaves the other's resume and Open by name"
        );
        let _ = m.switch_out(other);
        assert!(m.named.resuming.is_none() && m.named.attaching.is_none() && m.named.opening.is_none(), "its own pane's leave drops them");
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
        assert_eq!(m.active_meta(LiveId::FIRST).map(|s| s.id.as_str()), Some("s"));
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

    /// A `dev` daemon (SynapsCLI #416) lists workers as `workers`, not
    /// `subagents_running`: a session with workers running is still
    /// WORKERS, never IDLE.
    #[test]
    fn workers_from_a_dev_daemon_are_not_idle() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(Event::Sessions(metas(json!([
            {"id": "w", "lifecycle": "live", "busy": false, "workers": {"running": 2, "rows": [], "more": 0}},
            {"id": "d", "lifecycle": "live", "busy": false, "workers": {"running": 0, "rows": [
                {"handle_id": "sa_1", "subagent_id": 1, "agent_name": "scout", "status": "completed",
                 "started_at": "2026-11-18T12:00:00Z", "finished_at": "2026-11-18T12:01:00Z"}]}},
            {"id": "o", "lifecycle": "live", "busy": false}
        ]))));
        let meta = |m: &Model, id: &str| m.sessions.iter().find(|s| s.id == id).unwrap().clone();
        assert_eq!(["w", "d", "o"].map(|id| m.state(&meta(&m, id))), [State::Workers, State::Idle, State::Idle]);
        assert_eq!(m.workers(&meta(&m, "w")), 2);
    }

    /// A session whose workers run on after its turn is WORKERS, never
    /// IDLE: from the listing's `subagents_running` (SynapsCLI #410; an
    /// older daemon omits it, so 0), and the attached one from its own
    /// transcript once a live source has spoken of its workers (no daemon
    /// support needed, and it runs ahead of the poll).
    #[test]
    fn workers_running_on_are_not_idle() {
        use crate::rail::Chip;
        let ev = |v: Value| session(1, synaps_client::protocol::decode_event(&v).unwrap());
        let mut m = Model::new(ConnState::Connecting);
        let list = |s_workers: usize| {
            Event::Sessions(metas(json!([
                {"id": "s", "lifecycle": "live", "busy": false, "subagents_running": s_workers},
                {"id": "w", "lifecycle": "live", "busy": false, "subagents_running": 2},
                {"id": "b", "lifecycle": "live", "busy": true, "subagents_running": 1},
                {"id": "o", "lifecycle": "live", "busy": false}
            ])))
        };
        let st = |m: &Model, id: &str| m.state(m.sessions.iter().find(|s| s.id == id).unwrap());
        let all = |m: &Model| ["s", "w", "b", "o"].map(|id| st(m, id));
        m.apply(list(0));
        assert_eq!(all(&m), [State::Idle, State::Workers, State::Running, State::Idle], "an older daemon's \"o\": idle");
        let meta = |m: &Model, id: &str| m.sessions.iter().find(|s| s.id == id).unwrap().clone();
        assert_eq!(m.workers(&meta(&m, "w")), 2);
        assert!(Chip::Running.admits(State::Workers) && !Chip::Unread.admits(State::Workers));
        // Attached to "s", from a daemon that lists no workers: its own.
        attach(&mut m, 1);
        let use_ = json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start", "tool_id": "t1",
            "input": {"agent": "scout", "task": "look"}}});
        let res = json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": "t1",
            "result": json!({"handle_id": "sa_1", "agent_name": "scout", "status": "running"}).to_string()}});
        let row = |status: &str| {
            json!({"ev": "subagent_rows", "rows": [{"subagent_id": 1, "agent_name": "scout", "status": status,
            "cancel_requested": false, "elapsed_secs": 3.0, "finished_elapsed": null, "step": "ls", "tools": 1}]})
        };
        m.apply(ev(use_));
        m.apply(ev(res));
        // Watchable from its start (before a live row says it runs).
        assert_eq!(m.watchable(), vec![crate::watch::Candidate::of(1, "scout")]);
        m.apply(ev(row("running")));
        assert_eq!((st(&m, "s"), m.workers(&meta(&m, "s"))), (State::Workers, 1));
        assert_eq!(m.watchable().len(), 1);
        // It ends: idle at once, whatever the last poll said; no longer on offer.
        m.apply(list(1));
        m.apply(ev(row("completed")));
        assert_eq!(st(&m, "s"), State::Idle);
        assert!(m.watchable().is_empty());
        // A re-attach, nothing live heard yet: the listing's count.
        attach(&mut m, 2);
        assert_eq!(st(&m, "s"), State::Workers);
        m.apply(list(0));
        assert_eq!(st(&m, "s"), State::Idle);
    }

    /// Worker views are their session's: an attach of another session drops
    /// them, a re-attach of the same one (a reconnect, its tab back) asks
    /// again at once; a detach or a tab left asks nothing more; a reply to an
    /// attachment before is dropped; the engine's note of a steer settles it
    /// and a refusal keeps the text for the composer. A page never rebuilds
    /// the rail, and an idle worker's empty page draws nothing.
    #[test]
    fn worker_views_are_their_sessions() {
        use std::time::Instant;
        use synaps_client::WatchReply;

        use crate::watch::Candidate;
        let page = |v: Value| WatchReply::Page(Box::new(serde_json::from_value(v).unwrap()));
        let reply_at = |seq: u64, since: u64, events: Value| Event::SubagentWatch {
            attach_seq: seq,
            session_id: "s".into(),
            handle_id: "sa_2".into(),
            since,
            reply: page(json!({"handle_id": "sa_2", "subagent_id": 2, "agent_name": "sleeper", "status": "running",
                "steerable": true, "next_seq": 1, "more": false, "events": events})),
        };
        let reply = |seq: u64| reply_at(seq, 0, json!([{"kind": "text", "text": "Step 1."}]));
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        let t0 = Instant::now();
        m.watch.open(&Candidate::of(2, "sleeper"), None, t0);
        assert_eq!(m.watch.take_poll(t0), Some(("sa_2".into(), 0)));
        // A reply to the attachment before is not this one's.
        let before = m.shown().len();
        m.apply(reply(9));
        assert_eq!(m.shown().len(), before, "dropped");
        let generation = m.rail_gen;
        m.apply(reply(1));
        assert_eq!(m.shown().items().last().map(|i| i.text.as_str()), Some("Step 1."));
        assert_eq!(m.transcript.len(), 0, "the session's own transcript is untouched");
        assert!(m.view_dirty && m.rail_gen == generation, "drawn, the rail not rebuilt");
        // Idle: an empty page draws nothing.
        m.view_dirty = false;
        assert_eq!(m.watch.take_poll(Instant::now() + crate::watch::POLL_INTERVAL), Some(("sa_2".into(), 1)));
        m.apply(reply_at(1, 1, json!([])));
        assert!(!m.view_dirty && m.rail_gen == generation, "nothing to draw");
        assert!(m.watch.poll_due().is_some(), "and the next poll is due");
        // Steer: the engine's note settles it; a refusal of a later one gives
        // its text back (its view is on screen).
        let steer = |h: &str| {
            let raw = json!({"ev": "external", "event": {"content": {"content_type": "subagent_steer", "data": {"handle_id": h}}}});
            Event::session(1, "s", 0, SessionEvent::Idle, Arc::new(raw))
        };
        m.watch.steer_sent("sa_2", "stop after the next step");
        m.apply(steer("sa_2"));
        let refused = |c: &str| Event::Error {
            attach_seq: Some(1),
            error: Error::Refused { command: Some(c.into()), reason: "not the input owner".into() },
        };
        assert!(m.apply(refused("subagent_steer")).is_empty());
        assert_eq!(m.watch.take_unsent(), None, "settled: nothing to give back");
        m.watch.steer_sent("sa_2", "reply STEERED");
        m.view_dirty = false;
        assert!(m.apply(refused("subagent_steer")).is_empty());
        assert!(m.view_dirty && m.rail_gen == generation);
        assert_eq!(m.watch.take_unsent().map(|u| u.0).as_deref(), Some("reply STEERED"));
        // The same session again: asked again at once, from where it was.
        attach(&mut m, 2);
        let t1 = Instant::now();
        assert_eq!(m.watch.focus(), Some("sa_2"));
        assert_eq!(m.watch.take_poll(t1), Some(("sa_2".into(), 1)), "at once, from its cursor");
        // Detached: nothing polls.
        m.apply(Event::Detached { attach_seq: 2, session_id: Some("s".into()), reason: "gone".into() });
        assert_eq!(m.watch.poll_due(), None);
        // Another session: its views are gone.
        m.want(LiveId::FIRST, 3);
        m.apply(Event::Attached { attach_seq: 3, attached: attached("other"), mode: AttachMode::Mirror, reconnect: false });
        assert_eq!((m.watch.focus(), m.watch.bytes()), (None, 0));
        // A tab left: what it hands out polls nothing.
        attach(&mut m, 4);
        m.watch.open(&Candidate::of(2, "sleeper"), None, t1);
        let _ = m.switch_out(LiveId::FIRST);
        assert_eq!(m.watch.poll_due(), None, "a background tab polls nothing");
    }

    /// Shady #32 H1: what a store holds open outlives the switch out of its
    /// tab, and a switch whose attach the tab keys hold back (`showing`):
    /// the last tab's close detaches it, once. The next attach replaces it.
    #[test]
    fn a_store_holds_its_attachment_until_the_last_tab_closes() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        let _ = m.switch_out(LiveId::FIRST);
        m.switch_in(LiveId::FIRST, Transcript::new(), Stats::default());
        m.showing(LiveId::FIRST, "b");
        assert_eq!(m.lives[LiveId::FIRST].wanted(), None, "nothing wanted while B waits");
        assert_eq!(m.lives[LiveId::FIRST].release(), Some(1), "A's, still open on the client");
        assert_eq!(m.lives[LiveId::FIRST].release(), None, "once");
        attach(&mut m, 2);
        attach(&mut m, 3);
        assert_eq!(m.lives[LiveId::FIRST].release(), Some(3), "an attach replaced the one before");
    }

    /// Shady #32 M1: after a tab switch, the attachment left still answers
    /// (a `NotAttached` now names its seq, a refused worker poll or steer):
    /// none of it reaches the store the switch went to. Its worker steer
    /// is not given back as refused, its poll in flight is not failed,
    /// nothing is drawn; that attachment's own refusal still is one.
    #[test]
    fn a_left_attachments_errors_reach_no_store() {
        use crate::watch::Candidate;
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        let _ = m.switch_out(LiveId::FIRST);
        m.switch_in(LiveId::FIRST, Transcript::new(), Stats::default());
        attach(&mut m, 2);
        let t0 = std::time::Instant::now();
        m.watch.open(&Candidate::of(2, "sleeper"), None, t0);
        assert_eq!(m.watch.take_poll(t0), Some(("sa_2".into(), 0)), "a poll in flight");
        m.watch.steer_sent("sa_2", "stop after the next step");
        let due = m.watch.poll_due();
        m.view_dirty = false;
        let generation = m.rail_gen;
        let error = |seq: u64, error: Error| Event::Error { attach_seq: Some(seq), error };
        for e in [
            Error::NotAttached { command: "subagent_steer" },
            Error::NotAttached { command: "subagent_watch" },
            Error::Refused { command: Some("subagent_steer".into()), reason: "no".into() },
            Error::Refused { command: Some("subagent_watch".into()), reason: "no".into() },
        ] {
            let what = e.to_string();
            assert!(m.apply(error(1, e)).is_empty(), "{what}");
            assert_eq!(m.watch.take_unsent(), None, "{what}: no steer of this session given back");
            assert_eq!(m.watch.poll_due(), due, "{what}: the poll in flight is not failed");
            assert!(!m.view_dirty && m.rail_gen == generation, "{what}: nothing drawn");
        }
        assert_eq!(m.lives[LiveId::FIRST].last_error, None);
        // The attachment on screen's own: its steer comes back.
        m.apply(error(2, Error::NotAttached { command: "subagent_steer" }));
        assert_eq!(m.watch.take_unsent().map(|u| u.0).as_deref(), Some("stop after the next step"));
    }

    /// Split panes step 2: a second store's attach (step 3's other pane)
    /// leaves the visible tab on its own session; only the shown store's
    /// attach repoints it (a create or a resume naming the session).
    #[test]
    fn another_stores_attach_leaves_the_visible_tab() {
        let mut m = Model::new(ConnState::Connecting);
        m.tabs.open("a", "a", crate::tabs::Place::Current);
        m.want(LiveId::FIRST, 1);
        m.asked_for(LiveId::FIRST, "a");
        m.apply(Event::Attached { attach_seq: 1, attached: attached("a"), mode: AttachMode::Mirror, reconnect: false });
        assert_eq!(m.tabs.visible_session(), Some("a"));
        let b = m.lives.add();
        m.want(b, 2);
        m.asked_for(b, "b");
        m.apply(Event::Attached { attach_seq: 2, attached: attached("b"), mode: AttachMode::Mirror, reconnect: false });
        assert!(m.lives[b].is_active("b") && m.lives[LiveId::FIRST].is_active("a"), "both attached");
        assert_eq!(m.tabs.visible_session(), Some("a"), "the visible tab is still the shown store's session");
        // The shown store's own attach to another id (a resume) still moves it.
        m.want(LiveId::FIRST, 3);
        m.apply(Event::Attached { attach_seq: 3, attached: attached("a2"), mode: AttachMode::Mirror, reconnect: false });
        assert_eq!(m.tabs.visible_session(), Some("a2"));
    }

    /// Shady M1/M2: a worker's pages reach the session's list through the
    /// pump, and only a page its view took: a reply it dropped (asked
    /// before its focus moved) says nothing, so a re-attach's demotion
    /// stands until a fresh page lifts it. A page's end, then the
    /// stream's: why and how long (H1).
    #[test]
    fn the_list_hears_only_the_pages_its_view_took() {
        use std::time::Instant;
        use synaps_client::WatchReply;

        use crate::subagents::Status;
        use crate::watch::{Candidate, POLL_INTERVAL};
        let ev = |seq: u64, v: Value| session(seq, synaps_client::protocol::decode_event(&v).unwrap());
        let page = |n: u64, name: &str, status: &str| {
            let v = json!({"handle_id": format!("sa_{n}"), "subagent_id": n, "agent_name": name, "status": status,
                "steerable": status == "running", "next_seq": 1, "more": false, "events": []});
            WatchReply::Page(Box::new(serde_json::from_value(v).unwrap()))
        };
        let watch = |seq: u64, n: u64, since: u64, reply: WatchReply| Event::SubagentWatch {
            attach_seq: seq,
            session_id: "s".into(),
            handle_id: format!("sa_{n}"),
            since,
            reply,
        };
        let status = |m: &Model, n: u64| m.stats.subagents.status(n);
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        for (n, name) in [(2, "sleeper"), (3, "scout")] {
            let t = format!("t{n}");
            m.apply(ev(
                1,
                json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start", "tool_id": t,
                "input": {"agent": name, "task": "go"}}}),
            ));
            let res = json!({"handle_id": format!("sa_{n}"), "agent_name": name, "status": "running"}).to_string();
            m.apply(ev(1, json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": t, "result": res}})));
            m.apply(ev(
                1,
                json!({"ev": "subagent_rows", "rows": [{"subagent_id": n, "agent_name": name, "status": "running",
                "cancel_requested": false, "elapsed_secs": 3.0, "finished_elapsed": null, "step": "ls", "tools": 1}]}),
            ));
        }
        assert_eq!((status(&m, 2), status(&m, 3)), (Some(Status::Running), Some(Status::Running)));
        // sa_2's view reads its first page: running.
        let t0 = Instant::now();
        m.watch.open(&Candidate::of(2, "sleeper"), None, t0);
        assert_eq!(m.watch.take_poll(t0), Some(("sa_2".into(), 0)));
        m.apply(watch(1, 2, 0, page(2, "sleeper", "running")));
        // A reconnect: both "started" until a live source says again.
        attach(&mut m, 2);
        assert_eq!((status(&m, 2), status(&m, 3)), (Some(Status::Started), Some(Status::Started)), "demoted");
        assert_eq!(m.watch.take_poll(Instant::now()), Some(("sa_2".into(), 1)), "asked again at once");
        // Its focus moves to sa_3 before the answer comes: the answer is
        // dropped, and the view's last page (the attachment before's) is
        // not heard again.
        m.watch.open(&Candidate::of(3, "scout"), None, Instant::now());
        m.view_dirty = false;
        m.apply(watch(2, 2, 1, page(2, "sleeper", "running")));
        assert_eq!(status(&m, 2), Some(Status::Started), "the demotion stands");
        assert!(!m.view_dirty, "nothing to draw");
        // sa_3's own page, fresh: lifted.
        assert_eq!(m.watch.take_poll(Instant::now()), Some(("sa_3".into(), 0)));
        m.apply(watch(2, 3, 0, page(3, "scout", "running")));
        assert_eq!(status(&m, 3), Some(Status::Running), "a page it took lifts it");
        assert!(m.view_dirty);
        // Its page says how it ended first; then the stream's end says why
        // and how long.
        assert_eq!(m.watch.take_poll(Instant::now() + 2 * POLL_INTERVAL), Some(("sa_3".into(), 1)));
        m.apply(watch(2, 3, 1, page(3, "scout", "failed")));
        assert_eq!(status(&m, 3), Some(Status::Failed));
        m.apply(ev(
            2,
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "subagent_done", "subagent_id": 3, "agent_name": "",
            "result_preview": "ERROR: boom", "duration_secs": 12.5}}),
        ));
        let info = m.stats.subagents.info(3).expect("sa_3 known");
        assert_eq!((info.status, info.line.as_str(), info.elapsed), (Some(Status::Failed), "reason: boom", Some(12.5)));
    }

    /// N-M1: a blocking `subagent` cut off by HALT (its tool dropped: no
    /// `SubagentDone`, no row) leaves no WORKERS once the turn ends; its
    /// block says cancelled. N-L3: with nothing attached (a cached tab
    /// shown while its attach is on its way, a detach), no count last
    /// seen is shown.
    #[test]
    fn a_halted_blocking_subagent_leaves_no_workers() {
        let ev = |v: Value| session(1, synaps_client::protocol::decode_event(&v).unwrap());
        let ev2 = |v: Value| session(2, synaps_client::protocol::decode_event(&v).unwrap());
        let llm = |v: Value| json!({"ev": "stream", "event": v});
        let mut m = Model::new(ConnState::Connecting);
        m.apply(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "busy": false, "subagents_running": 0}]))));
        let st = |m: &Model| m.state(&m.sessions[0]);
        attach(&mut m, 1);
        m.apply(ev(json!({"ev": "turn_started", "user_text": "dig"})));
        m.apply(ev(llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "subagent", "tool_id": "o",
            "input": {"agent": "digger", "task": "dig"}}))));
        m.apply(ev(llm(
            json!({"kind": "agent", "agent": "subagent_start", "subagent_id": 1, "agent_name": "digger", "task_preview": "dig"}),
        )));
        assert!(m.stats.subagents.running("o"));
        assert_eq!((st(&m), m.active_workers(LiveId::FIRST)), (State::Running, 0));
        // HALT: the canceled result, the turn aborted.
        m.apply(ev(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "o", "result": "Canceled by user"}))));
        m.apply(ev(json!({"ev": "aborted"})));
        assert_eq!((st(&m), m.active_workers(LiveId::FIRST)), (State::Idle, 0), "not WORKERS");
        assert_eq!(m.stats.subagents.view("o").and_then(|v| v.status), Some(crate::subagents::Status::Cancelled));
        // A background worker running on: WORKERS 1, from its own rows.
        let row = json!({"ev": "subagent_rows", "rows": [{"subagent_id": 2, "agent_name": "scout", "status": "running",
            "cancel_requested": false, "elapsed_secs": 3.0, "finished_elapsed": null, "step": "ls", "tools": 1}]});
        m.apply(ev(llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start", "tool_id": "t",
            "input": {"agent": "scout", "task": "look"}}))));
        m.apply(ev(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "t",
            "result": json!({"handle_id": "sa_2", "agent_name": "scout", "status": "running"}).to_string()}))));
        m.apply(ev(row.clone()));
        assert_eq!((st(&m), m.active_workers(LiveId::FIRST)), (State::Workers, 1));
        // The tab left and shown again from the cache, its attach on the way.
        let (t, s) = m.switch_out(LiveId::FIRST);
        m.switch_in(LiveId::FIRST, t, s);
        assert_eq!((m.stats.subagents.running_count(), m.active_workers(LiveId::FIRST)), (1, 0), "the cache's count is not shown");
        attach(&mut m, 2);
        m.apply(ev2(row));
        assert_eq!(m.active_workers(LiveId::FIRST), 1);
        m.apply(Event::Detached { attach_seq: 2, session_id: Some("s".into()), reason: "gone".into() });
        assert_eq!(m.active_workers(LiveId::FIRST), 0);
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
        m.want(LiveId::FIRST, 2);
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
        m.attaching(LiveId::FIRST, 4, None, false);
        m.asked_for(LiveId::FIRST, "x");
        assert_eq!(m.state(&m.sessions[0]), State::Opening, "the glyph says so");
        assert_eq!(under(&m, Chip::NeedsYou), ["x", "y"], "x keeps its place");
        assert_eq!(m.attention().count(), 2, "the counts do not dip");
        let second = under(&m, Chip::NeedsYou)[0].clone();
        assert!(m.open_row(&RowAction::Attach(second)).is_empty(), "the second click: x again, nothing to do");
        // Under RUNNING: r's turn ends while its attach is on its way; it
        // stays until the attach lands, then goes.
        assert_eq!(under(&m, Chip::Running), ["r"]);
        assert_eq!(m.open_row(&RowAction::Attach("r".into())).len(), 1);
        m.attaching(LiveId::FIRST, 5, None, false);
        m.asked_for(LiveId::FIRST, "r");
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
                    m.attaching(LiveId::FIRST, 4, None, false);
                    m.asked_for(LiveId::FIRST, session_id);
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
        assert!(matches!(fx.as_slice(), [Effect::Resume { req, open: true, .. }] if req.target == "j-axel"), "exact, saved: {fx:?}");
        let smoke = req("Smoke tests", "j-t", None);
        assert_eq!(
            m.find_enter("smoke", &[RowAction::Resume(smoke.clone())]),
            [Effect::Resume { req: smoke, open: false, place: Place::Current }],
            "single"
        );
        assert!(m.find_enter("kit", &[at("l1"), at("l2")]).is_empty(), "ambiguous");
        assert_eq!(m.named.error.as_deref(), Some("2 sessions match 'kit': click one, or type its exact name."));
        assert_eq!(m.find_enter("ghost", &[]), [ASK_RECENT], "none: asked once more");
        assert!(m.apply(saved_list(all)).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(m.find_enter("Axel Memory", &[]).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("Nothing matches 'Axel Memory'."));

        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["owned_sessions"]));
        let by_name = |n: &str| Effect::Resume { req: ResumeRequest { by_name: true, ..req_(n) }, open: true, place: Place::Current };
        assert_eq!(m.find_enter("ghost", &[]), [by_name("ghost")], "no id can contain it");
        assert!(m.find_enter("cafe", &[]).is_empty(), "could be part of an id");
        assert!(m.named.error.as_deref().unwrap().contains("Press Enter again"));
        assert_eq!(m.find_enter("cafe", &[]), [by_name("cafe")], "told, and Enter again");
        assert!(m.find_enter("add", &[]).is_empty());
        assert!(m.find_enter("cafe", &[]).is_empty(), "another text in between: told again");
    }

    #[test]
    fn an_event_on_a_session_not_on_screen_marks_it_unread() {
        // BRIEF-events §4: an event to a session not on screen is news, from
        // the watch listing alone (no live frame there). Injected while idle,
        // it is a message more (and with the auto-turn, a turn that ends);
        // steered into a running turn, the turn ends with it. On screen, or
        // on a TUI's screen, it is not.
        let mut m = Model::new(ConnState::Connecting);
        let list = |busy: bool, messages: usize, roster: Value| {
            metas(json!([{"id": "s", "lifecycle": "live"},
                         {"id": "b", "lifecycle": "live", "busy": busy, "messages": messages, "roster": roster}]))
        };
        let st = |m: &Model| m.state(m.sessions.iter().find(|s| s.id == "b").unwrap());
        m.apply(Event::Sessions(list(false, 4, json!([]))));
        attach(&mut m, 1);
        // auto_turn off: injected, no turn.
        m.apply(Event::Sessions(list(false, 5, json!([]))));
        assert_eq!(st(&m), State::Unread);
        // Seen (attached), then an event wakes it: the turn runs, then ends.
        let mut m = Model::new(ConnState::Connecting);
        m.apply(Event::Sessions(list(false, 4, json!([]))));
        attach(&mut m, 1);
        m.apply(Event::Sessions(list(true, 5, json!([]))));
        assert_eq!(st(&m), State::Running);
        m.apply(Event::Sessions(list(false, 6, json!([]))));
        assert_eq!(st(&m), State::Unread, "the tab and the rail row say so");
        // A TUI has it on screen: not news here.
        let mut m = Model::new(ConnState::Connecting);
        let tui = json!([{"id": 1, "kind": "tui", "name": "tui"}]);
        m.apply(Event::Sessions(list(false, 4, tui.clone())));
        attach(&mut m, 1);
        m.apply(Event::Sessions(list(false, 5, tui)));
        assert_eq!(st(&m), State::Idle);
    }

    /// M2: UNREAD is what the user has not seen. The session on screen is
    /// seen as its transcript has it (ahead of the poll): moving away right
    /// after its turn ends marks nothing. A turn this window sent and left
    /// running marks when it ends (PR #8 re-review N1); one in a session a
    /// TUI shows never does (read there).
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
        m.want(LiveId::FIRST, 2);
        m.apply(attached_ev(2, attached("b")));
        m.apply(Event::Sessions(list(false, 2, json!([]))));
        assert_eq!(st(&m), State::Idle, "seen on screen, left within the poll");
        // Off screen, another client adds a message: news.
        m.apply(Event::Sessions(list(false, 3, json!([]))));
        assert_eq!(st(&m), State::Unread);

        // The user's own turn, left running: its end is news.
        attach(&mut m, 3);
        m.apply(history(3, 0, 3));
        m.sent(PaneId::FIRST, "do it".into());
        m.apply(session(3, turn(Some("do it"))));
        m.want(LiveId::FIRST, 4);
        m.apply(attached_ev(4, attached("b")));
        m.apply(Event::Sessions(list(true, 3, json!([]))));
        assert_eq!(st(&m), State::Running, "still running off screen");
        m.apply(Event::Sessions(list(false, 7, json!([]))));
        assert_eq!(st(&m), State::Unread, "own turn, left running, ended off screen");
        // Opened (read), then left again.
        attach(&mut m, 5);
        m.apply(history(5, 0, 7));
        m.want(LiveId::FIRST, 6);
        m.apply(attached_ev(6, attached("b")));
        m.apply(Event::Sessions(list(false, 7, json!([]))));
        assert_eq!(st(&m), State::Idle, "opened, read, left");
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
        m.resuming(LiveId::FIRST, req("notes", "j-other", None), 3, false);
        assert_eq!(m.state(&m.sessions[0]), State::Idle);
        m.resuming(LiveId::FIRST, ResumeRequest { by_name: true, ..req_("notes") }, 4, true);
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
        m.sent(PaneId::FIRST, "hello there".into());
        let refused = Error::Refused { command: Some("submit".into()), reason: "input owned by #1".into() };
        assert_eq!(
            m.apply(Event::Error { attach_seq: Some(1), error: refused }),
            vec![Effect::Restore(PaneId::FIRST, "hello there".into())]
        );
        assert!(m.last_error.as_deref().unwrap().contains("input owned by #1"));
        // Accepted: TurnStarted echoes it; a later refusal of something else
        // gives nothing back.
        m.sent(PaneId::FIRST, "second".into());
        m.apply(session(1, SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some("second".into()) }));
        let cancel = Error::Refused { command: Some("cancel".into()), reason: "no".into() };
        assert!(m.apply(Event::Error { attach_seq: Some(1), error: cancel }).is_empty());
        // A reload-drain refusal (`Error` frame, command unknown) gives back.
        m.sent(PaneId::FIRST, "third".into());
        let drain = Error::Refused { command: None, reason: "daemon reloading; retry in a moment".into() };
        assert_eq!(m.apply(Event::Error { attach_seq: Some(1), error: drain }), vec![Effect::Restore(PaneId::FIRST, "third".into())]);
    }

    /// N5: two sends before the first echo: the echo confirms the one it
    /// echoes (whitespace aside), a refusal gives back the oldest left, a
    /// detach everything left.
    #[test]
    fn several_sends_in_flight_are_tracked() {
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        for t in ["one", "two", "three"] {
            m.sent(PaneId::FIRST, t.into());
        }
        m.apply(session(1, SessionEvent::Steered { text: " two\n".into(), delivered: true }));
        let refused = Error::Refused { command: Some("submit".into()), reason: "busy".into() };
        assert_eq!(m.apply(Event::Error { attach_seq: Some(1), error: refused }), vec![Effect::Restore(PaneId::FIRST, "one".into())]);
        m.sent(PaneId::FIRST, "four".into());
        let fx = m.apply(Event::Detached { attach_seq: 1, session_id: None, reason: "gone".into() });
        assert_eq!(fx, vec![Effect::Restore(PaneId::FIRST, "three\nfour".into())]);
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
        m.sent(PaneId::FIRST, "lost?".into());
        let fx = m.apply(Event::Detached { attach_seq: 2, session_id: Some("s".into()), reason: "session ended".into() });
        assert_eq!(fx, vec![Effect::Restore(PaneId::FIRST, "lost?".into())]);
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
        m.want(LiveId::FIRST, 2);
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
        m.want(LiveId::FIRST, 1);
        m.asked_for(LiveId::FIRST, "a");
        m.apply(Event::Attached { attach_seq: 1, attached: attached("a"), mode: AttachMode::Mirror, reconnect: false });
        assert!(m.is_wanted("a"));
        // Click B, then A again before B lands: A is asked for again.
        m.want(LiveId::FIRST, 2);
        m.asked_for(LiveId::FIRST, "b");
        assert!(m.is_active("a") && !m.is_wanted("a"));
    }

    #[test]
    fn observers_and_reattaching_clients_cannot_send() {
        let mut m = Model::new(ConnState::Connecting);
        m.want(LiveId::FIRST, 3);
        m.apply(Event::Attached { attach_seq: 3, attached: attached("s"), mode: AttachMode::Observe, reconnect: false });
        assert!(!m.can_send());
        attach(&mut m, 4);
        assert!(m.can_send());
        m.apply(Event::Reattaching { attach_seq: 4, session_id: "s".into(), reason: "reload".into() });
        assert!(!m.can_send());
    }

    /// N-L1: the control connection's new pid starts the worker on screen
    /// over, before its session's attach comes back; the same pid does not.
    #[test]
    fn a_new_daemon_pid_starts_the_worker_view_over() {
        use std::time::Instant;

        use crate::watch::Candidate;
        let at = |pid: u32| Event::Status(Status::Connected { daemon_version: "0.0.0".into(), profile: None, pid, features: vec![] });
        let mut m = Model::new(ConnState::Connecting);
        m.apply(at(100));
        attach(&mut m, 1);
        let t0 = Instant::now();
        m.watch.open(&Candidate::of(2, "sleeper"), None, t0);
        let generation = m.watch.focus_key().unwrap().1;
        m.apply(at(100));
        assert_eq!(m.watch.focus_key().unwrap().1, generation, "the same daemon");
        m.apply(at(200));
        assert!(m.watch.focus_key().unwrap().1 != generation, "another daemon: from 0");
        assert_eq!(m.watch.take_poll(Instant::now()), Some(("sa_2".into(), 0)));
    }

    /// The link's reason (motion.md §5.3): a reload's goodbye marks the
    /// link down for a reload, counted, through the failed reconnects
    /// (Offline) until it is up again; any other drop is not a reload.
    #[test]
    fn a_reload_goodbye_marks_the_link_until_it_is_up() {
        let st = |s: Status| Event::Status(s);
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        assert_eq!((m.reload, m.reload_seq), (false, 0));
        m.apply(st(Status::Reconnecting { why: synaps_client::RELOADING.into() }));
        assert_eq!((m.reload, m.reload_seq), (true, 1), "the goodbye");
        m.apply(st(Status::Offline("connection refused".into())));
        m.apply(st(Status::Connecting));
        assert_eq!((m.reload, m.reload_seq), (true, 1), "kept through the failed reconnects");
        m.apply(connected(&[]));
        assert!(!m.reload, "up: over");
        m.apply(st(Status::Reconnecting { why: synaps_client::RELOADING.into() }));
        assert_eq!((m.reload, m.reload_seq), (true, 2), "a second goodbye counts");
        m.apply(connected(&[]));
        m.apply(st(Status::Reconnecting { why: "daemon closed the connection".into() }));
        assert_eq!((m.reload, m.reload_seq), (false, 2), "any other drop is not a reload");
    }

    /// The right panel's cache and keep-warm (`crate::warm`): asked from a
    /// daemon that says when the cache expires, on the attachment's attach,
    /// turn end and keep-warm notices, nothing else; the answer is the store's.
    #[test]
    fn cache_status_is_asked_on_attach_turn_end_and_keep_warm_notices() {
        let asked = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Status(1))).count();
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[]));
        m.want(LiveId::FIRST, 1);
        let fx = m.apply(Event::Attached { attach_seq: 1, attached: attached("s"), mode: AttachMode::Mirror, reconnect: false });
        assert_eq!(asked(&fx), 0, "an older daemon is never asked");
        m.apply(connected(&[synaps_client::FEATURE_CACHE_EXPIRY]));
        let fx = m.apply(Event::Attached { attach_seq: 1, attached: attached("s"), mode: AttachMode::Mirror, reconnect: true });
        assert_eq!(asked(&fx), 1, "attach");
        assert_eq!(asked(&m.apply(session(1, SessionEvent::Idle))), 1, "turn end");
        let notice = |t: &str| session(1, SessionEvent::SystemNotice { text: t.into() });
        assert_eq!(asked(&m.apply(notice("cache keep-warm ended: bound reached"))), 1, "keep-warm notice");
        assert_eq!(asked(&m.apply(notice("model → x"))), 0, "any other notice");
        assert_eq!(asked(&m.apply(session(7, SessionEvent::Idle))), 0, "not this store's attachment");
        let st = json!({"pinned": true, "keepwarm": {"mode": "auto", "session_pings": 2}, "cache": {"ttl": "5m", "expires_in_secs": 200}});
        m.apply(Event::SessionStatus { attach_seq: 1, value: st });
        let w = m.lives[LiveId::FIRST].stats.warm.clone().expect("kept");
        assert_eq!((w.mode.as_str(), w.pinned, w.pings, w.expires.map(|d| d.as_secs())), ("auto", true, 2, Some(200)));
    }

    /// The countdown is not the session's any more after an effective model
    /// change (a `Set`, another client's `/model`: both `setting_changed`)
    /// or a replaced history (`/clear`, `/resume`, an applied compaction):
    /// it stops at once and the daemon is asked again. A refused or
    /// same-model change, a thinking change, a failed compaction, or
    /// another attachment's event keeps it, unasked.
    #[test]
    fn a_new_model_or_history_stops_the_countdown_and_asks_again() {
        let asked = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Status(1))).count();
        let raw = |seq: u64, v: Value| {
            let e = synaps_client::protocol::decode_event(&v).unwrap();
            Event::session(seq, "s", 0, e, Arc::new(v))
        };
        let set = |seq: u64, ok: bool, model: &str, level: &str| {
            raw(
                seq,
                json!({"ev": "setting_changed", "applied": {"id": 0, "setting": "model", "ok": ok,
                "view": {"model": model, "thinking_level": level, "reasoning_level": level, "context_window": 200000}}}),
            )
        };
        let warm = json!({"keepwarm": {"mode": "off"}, "cache": {"ttl": "1h", "expires_in_secs": 3000}});
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[synaps_client::FEATURE_CACHE_EXPIRY]));
        m.want(LiveId::FIRST, 1);
        let v = json!({"type": "attached", "client": 2, "meta": {"id": "s"}, "conversation": {"api_messages": []},
            "input_owner": 2, "view": {"model": "claude-a", "thinking_level": "low", "context_window": 200000}});
        let Ok(synaps_client::protocol::DaemonFrame::Attached(a)) = synaps_client::protocol::decode_frame(&v.to_string()) else { panic!() };
        m.apply(Event::Attached { attach_seq: 1, attached: a, mode: AttachMode::Mirror, reconnect: false });
        let counting = |m: &Model| m.lives[LiveId::FIRST].stats.warm.as_ref().and_then(|w| w.expires).is_some();
        let fresh = |m: &mut Model| {
            m.apply(Event::SessionStatus { attach_seq: 1, value: warm.clone() });
            assert!(counting(m));
        };
        fresh(&mut m);
        for (why, ev) in [
            ("refused", set(1, false, "claude-a", "low")),
            ("the same model", set(1, true, "claude-a", "low")),
            ("a thinking change", set(1, true, "claude-a", "high")),
            ("a failed compaction", raw(1, json!({"ev": "compaction_failed", "message": "no"}))),
            ("another attachment's", set(7, true, "claude-b", "low")),
        ] {
            assert_eq!(asked(&m.apply(ev)), 0, "{why}");
            assert!(counting(&m), "{why}: still counting");
        }
        for (why, ev) in [
            ("another model", set(1, true, "claude-b", "low")),
            ("/clear", raw(1, json!({"ev": "cleared", "session_id": "s2"}))),
            ("/resume", raw(1, json!({"ev": "resumed", "id": 1, "old_id": "s", "new_id": "s3"}))),
            (
                "an applied compaction",
                raw(1, json!({"ev": "compaction_applied", "previous_session_id": "s3", "session_id": "s4", "msg_count": 2})),
            ),
        ] {
            fresh(&mut m);
            assert_eq!(asked(&m.apply(ev)), 1, "{why}: asked again");
            let w = m.lives[LiveId::FIRST].stats.warm.as_ref().unwrap();
            assert!(w.expires.is_none() && w.next_change(crate::warm::Moment::now()).is_none(), "{why}: stopped");
        }
    }

    /// Providers don't share a prompt cache: a switch to another
    /// provider's model of the same name is a new model, so the countdown
    /// stops and the daemon is asked again (Shady #68 round 3). Only
    /// `anthropic/`, the default provider's documented alias, is the same
    /// model written two ways.
    #[test]
    fn another_provider_s_model_of_the_same_name_asks_again() {
        let asked = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Status(1))).count();
        let set = |model: &str| {
            let v = json!({"ev": "setting_changed", "applied": {"id": 0, "setting": "model", "ok": true,
                "view": {"model": model, "thinking_level": "low", "reasoning_level": "low", "context_window": 200000}}});
            let e = synaps_client::protocol::decode_event(&v).unwrap();
            Event::session(1, "s", 0, e, Arc::new(v))
        };
        let warm = json!({"keepwarm": {"mode": "off"}, "cache": {"ttl": "1h", "expires_in_secs": 3000}});
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&[synaps_client::FEATURE_CACHE_EXPIRY]));
        m.want(LiveId::FIRST, 1);
        let v = json!({"type": "attached", "client": 2, "meta": {"id": "s"}, "conversation": {"api_messages": []},
            "input_owner": 2, "view": {"model": "anthropic/claude-sonnet-4-6", "thinking_level": "low", "context_window": 200000}});
        let Ok(synaps_client::protocol::DaemonFrame::Attached(a)) = synaps_client::protocol::decode_frame(&v.to_string()) else { panic!() };
        m.apply(Event::Attached { attach_seq: 1, attached: a, mode: AttachMode::Mirror, reconnect: false });
        let counting = |m: &Model| m.lives[LiveId::FIRST].stats.warm.as_ref().and_then(|w| w.expires).is_some();
        for (model, new) in [
            ("bedrock/claude-sonnet-4-6", true),
            ("claude-sonnet-4-6", true),
            ("anthropic/claude-sonnet-4-6", false),
            ("ext-a/claude-sonnet-4-6", true),
            ("ext-b/claude-sonnet-4-6", true),
            ("ext-b/claude-sonnet-4-6", false),
        ] {
            m.apply(Event::SessionStatus { attach_seq: 1, value: warm.clone() });
            assert!(counting(&m));
            assert_eq!(asked(&m.apply(set(model))), usize::from(new), "to {model}: asked again");
            assert_eq!(!counting(&m), new, "to {model}: stopped");
        }
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
        let [Effect::Resume { req: bebop, open: false, .. }] = fx.as_slice() else { panic!("{fx:?}") };
        assert_eq!(bebop.target, "j-bebop");
        let o = bebop.options();
        assert_eq!((o.cwd, o.fallback_cwd), (Cwd::Path("/home/u/Projects/bebop".into()), Some(Cwd::Home)));
        m.resuming(LiveId::FIRST, bebop.clone(), 7, false);
        assert_eq!(state_of(&m, "bebop"), State::Opening);
        assert!(m.open_row(&row_action(&m, "bebop")).is_empty(), "L4: a double-click resumes once");
        m.apply(Event::CwdFallback { attach_seq: 7, missing: "/home/u/Projects/bebop".into(), cwd: None });
        assert_eq!(m.named.notice.as_deref(), Some("Folder /home/u/Projects/bebop is gone on the daemon's machine; resumed in ~."));
        let fx = m.apply(attached_ev(7, attached("live-bebop")));
        assert!(m.named.resuming.is_none());
        assert!(list_saved(&fx), "the resumed session left the saved list");
        assert_eq!(m.active.as_ref().unwrap().session_id, "live-bebop");

        m.resuming(LiveId::FIRST, req_("ghost"), 8, false);
        m.apply(detached(8, crate::named::NO_SUCH_GHOST));
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(m.named.resuming.is_none() && m.active.is_none() && m.last_error.is_none());

        m.resuming(LiveId::FIRST, req("axel", "j-axel", None), 9, false);
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
        m.resuming(LiveId::FIRST, req("bebop", "j-bebop", None), 7, false);
        assert_eq!(state_of(&m, "bebop"), State::Opening);
        m.want(LiveId::FIRST, 8);
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
            vec![Effect::Attach {
                session_id: "l1".into(),
                fallback: Some(req("live-one", "j-l1", Some("/w"))),
                open: true,
                place: Place::Current
            }]
        );
        assert!(m.named.error.is_none());
        let want = Effect::Resume { req: req("axel", "20261110-202033-e5f6", Some("/home/u/axel")), open: true, place: Place::Current };
        assert_eq!(m.open_by_name("axel"), vec![want.clone()]);

        // Part of an id, not a name: asked again, then refused locally.
        assert_eq!(m.open_by_name("64fb"), [ASK_RECENT]);
        assert!(m.apply(saved_list(json!([axel]))).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("No session named '64fb'."));
        assert!(m.named.opening.is_none());
        // A name made since the last list is in the fresh one.
        assert_eq!(m.open_by_name("new-one"), [ASK_RECENT]);
        let fx = m.apply(saved_list(json!([axel, {"id": "j-new", "name": "new-one", "live": false}])));
        assert_eq!(fx, [Effect::Resume { req: req("new-one", "j-new", None), open: true, place: Place::Current }]);

        // The outcome: the input clears on `Attached`, not before.
        m.open_by_name("axel");
        m.resuming(LiveId::FIRST, req("axel", "20261110-202033-e5f6", Some("/home/u/axel")), 5, true);
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
        assert_eq!(m.open_by_name("fe"), vec![Effect::Resume { req: by_name.clone(), open: true, place: Place::Current }]);
        assert_eq!((by_name.options().cwd, by_name.options().fallback_cwd), (Cwd::Home, None));
        m.resuming(LiveId::FIRST, by_name.clone(), 3, true);
        let fx = m.apply(attached_ev(3, attached_named("20261110-202033-64fe", None)));
        assert_eq!(fx, [Effect::ListSessions], "stays attached");
        let notice = m.named.notice.as_deref().unwrap();
        assert!(notice.contains("No session is named 'fe'") && notice.contains("20261110-202033-64fe"), "{notice}");
        assert_eq!(m.active.as_ref().unwrap().session_id, "20261110-202033-64fe");

        let axel = ResumeRequest { by_name: true, ..req_("axel") };
        m.open_by_name("axel");
        m.resuming(LiveId::FIRST, axel, 4, true);
        m.apply(attached_ev(4, attached_named("s-axel", Some("axel"))));
        assert!(m.named.opened && m.named.error.is_none());
        assert!(m.named.notice.as_deref().unwrap().contains("runs in ~"));
        assert_eq!(m.active.as_ref().unwrap().session_id, "s-axel");

        m.open_by_name("ghost");
        m.resuming(LiveId::FIRST, ResumeRequest { by_name: true, ..req_("ghost") }, 5, true);
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
        assert_eq!(fx, vec![Effect::Attach { session_id: "l3".into(), fallback: fallback.clone(), open: false, place: Place::Current }]);
        m.attaching(LiveId::FIRST, 4, fallback.clone(), false);
        // daemon/conn.rs: `DaemonFrame::Error { message: "unknown session" }`.
        let fx = m.apply(detached(4, "unknown session"));
        assert_eq!(fx, vec![Effect::Resume { req: fallback.clone().unwrap(), open: false, place: Place::Current }]);
        assert!(m.last_error.is_none());
        // Any other end is reported as before.
        m.attaching(LiveId::FIRST, 5, fallback, false);
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
        m.attaching(LiveId::FIRST, 4, None, false);
        m.asked_for(LiveId::FIRST, "l3");
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
        assert!(m.rename_request(LiveId::FIRST, "axon").is_none());
        assert_eq!(m.named.rename_error.as_deref(), Some("Not attached to a session."));
        attach(&mut m, 1);
        m.apply(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "name": "kite"}, {"id": "t", "lifecycle": "live"}]))));
        m.toggle_naming();
        assert!(m.rename_request(LiveId::FIRST, "Bad Name").is_none());
        assert!(m.named.rename_error.as_deref().unwrap().contains("lowercase"));
        assert!(m.named.rename.is_none(), "nothing pending after a local refusal");
        assert!(m.rename_request(LiveId::FIRST, "kite").is_none() && !m.named.naming, "its own name: nothing to do");

        m.toggle_naming();
        let r = m.rename_request(LiveId::FIRST, " axon ").unwrap();
        assert_eq!(r, RenameRequest { session_id: "s".into(), name: "axon".into(), expect: Expect::Unnamed });
        assert!(m.rename_request(LiveId::FIRST, "axon").is_none(), "one at a time");
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
        assert_eq!(m.active_name(LiveId::FIRST), Some("axon"));
        assert!(rows(&m).iter().any(|r| r.0 == "axon"), "the rail shows the name at once");

        // A double Save: "already named what you asked" is success.
        m.toggle_naming();
        m.rename_request(LiveId::FIRST, "zeta").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("zeta".into());
        m.apply(Event::RenameFailed(f));
        assert!(!m.named.naming && m.named.rename.is_none() && m.named.rename_error.is_none() && m.named.replace.is_none());

        m.toggle_naming();
        m.rename_request(LiveId::FIRST, "taken").unwrap();
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
        m.rename_request(LiveId::FIRST, "axon").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("kite".into());
        m.apply(Event::RenameFailed(f));
        assert!(m.named.replace.is_some());
        m.want(LiveId::FIRST, 2);
        m.apply(attached_ev(2, attached("t")));
        assert!(!m.named.naming && m.named.replace.is_none() && m.named.rename_error.is_none());
        assert!(m.replace_request().is_none(), "no offer to take to t");

        // A late answer for s while t is shown.
        attach(&mut m, 3);
        m.toggle_naming();
        m.rename_request(LiveId::FIRST, "axon").unwrap();
        m.want(LiveId::FIRST, 4);
        m.apply(attached_ev(4, attached("t")));
        m.apply(Event::RenameFailed(RenameFailure::new("s", RenameRefusal::Busy, "")));
        assert!(!m.named.naming);
        assert!(m.named.error.as_deref().unwrap().starts_with("Naming a session 'axon': "), "{:?}", m.named.error);

        // The row was closed (Cancel) on the session the answer is for.
        attach(&mut m, 5);
        m.toggle_naming();
        m.rename_request(LiveId::FIRST, "axon").unwrap();
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
    const C: &str = "20261113-080000-cccc";
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
        m.want(LiveId::FIRST, seq);
        m.asked_for(LiveId::FIRST, sid);
    }

    /// The UI's AUTHORIZE (`AppView::answer`): only what the model allows goes out.
    fn ui_authorize(m: &mut Model, client: &Client, prompt_id: u64) -> bool {
        if !m.may_answer(prompt_id) {
            return false;
        }
        client.on(m.attachment().unwrap()).answer(prompt_id, Some(CONFIRM_YES.into())).unwrap();
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

    /// The visible tab's switch, as `AppView::reconcile` does it: the
    /// session shown goes to the cache, the next one's cached state (or
    /// nothing) comes in, its attach goes out.
    fn ui_switch(m: &mut Model, client: &Client, sid: &str, cached: Option<(Transcript, Stats)>) -> (Transcript, Stats) {
        let left = m.switch_out(LiveId::FIRST);
        let (t, s) = cached.unwrap_or_default();
        m.switch_in(LiveId::FIRST, t, s);
        ui_attach(m, client, sid);
        left
    }

    /// Apply the client's events to `m` until `n` effects match `want`;
    /// every effect seen is returned.
    fn effects(m: &mut Model, events: &async_channel::Receiver<Event>, n: usize, want: impl Fn(&Effect) -> bool) -> Vec<Effect> {
        let until = Instant::now() + T;
        let mut seen = Vec::new();
        while seen.iter().filter(|e| want(e)).count() < n {
            while let Ok(ev) = events.try_recv() {
                seen.extend(m.apply(ev));
            }
            assert!(Instant::now() < until, "timed out; effects so far: {seen:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
        seen
    }

    /// M3: Ctrl+Tab held over three tabs (the press, then 30 Hz repeats for
    /// 1.5 s) shows each tab at once and sends one attach, for the tab it
    /// stops on, once it is let go. The app's gate, counted at the daemon.
    #[test]
    fn a_held_tab_key_attaches_once_when_let_go() {
        let (client, events, attaches, mut m) = connect("storm");
        for s in [A, B, C] {
            m.tabs.open(s, s, Place::New);
        }
        m.tabs.focus(0);
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A));
        let mut gate = crate::tabs::AttachGate::default();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut waiting = None;
        for t in std::iter::once(0).chain((0..45).map(|k| 600 + k * 33)) {
            gate.key_down(at(t));
            assert!(m.tabs.step(1));
            let s = m.tabs.visible_session().unwrap().to_string();
            let _ = m.switch_out(LiveId::FIRST);
            m.switch_in(LiveId::FIRST, Transcript::new(), Stats::default());
            if gate.switch(at(t)) {
                m.showing(LiveId::FIRST, &s);
                waiting = Some(s);
            } else {
                ui_attach(&mut m, &client, &s);
            }
            assert!(!gate.ready(at(t + 30)), "nothing is due while the key is held");
            assert!(!m.can_send(), "nothing drives a tab still opening");
        }
        assert!(attaches.recv_timeout(Duration::from_millis(200)).is_err(), "no attach while held");
        gate.key_up(at(2100));
        assert!(!gate.ready(at(2200)) && gate.ready(at(2220)));
        let s = waiting.take().expect("the last switch waited");
        ui_attach(&mut m, &client, &s);
        let mut last = attaches.recv_timeout(T).unwrap();
        assert!(attaches.recv_timeout(Duration::from_millis(300)).is_err(), "one attach for the whole storm");
        last.send(attached_with_prompt(&s, 1));
        pump(&mut m, &events, "the last tab attached", |m| m.is_active(&s));
        assert_eq!(m.tabs.visible_session(), Some(s.as_str()));
    }

    /// M4: a steer to A, then a switch to B before the daemon answers it; A
    /// refuses it on the attachment the switch left. It comes back as A's
    /// (into A's draft), never into the composer B shows.
    #[test]
    fn a_send_refused_after_its_tab_was_left_goes_back_to_that_tab() {
        let (client, events, attaches, mut m) = connect("unsent");
        m.tabs.open(A, "a", Place::Current);
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.can_send());
        client.on(m.attachment().unwrap()).submit("steer A this way").unwrap();
        m.sent(PaneId::FIRST, "steer A this way".into());
        m.tabs.open(B, "b", Place::New);
        let _ = ui_switch(&mut m, &client, B, None);
        a.event(A, 5, json!({"ev": "refused", "client": 2, "command": "submit", "reason": "input is owned by client #1"}));
        let seen = effects(&mut m, &events, 1, |e| matches!(e, Effect::Unsent { .. }));
        assert!(seen.contains(&Effect::Unsent { pane: PaneId::FIRST, session: A.into(), text: "steer A this way".into() }), "{seen:?}");
        assert!(!seen.iter().any(|e| matches!(e, Effect::Restore(..))), "nothing for B's composer: {seen:?}");
        let _b = attaches.recv_timeout(T).unwrap();
    }

    /// M4: the same send, never echoed nor refused before the attachment
    /// left ends: given back to A too (the daemon may not have it).
    #[test]
    fn a_send_unconfirmed_when_its_attachment_ends_goes_back_to_that_tab() {
        let (client, events, attaches, mut m) = connect("unsent2");
        m.tabs.open(A, "a", Place::Current);
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.can_send());
        for t in ["one", "two"] {
            client.on(m.attachment().unwrap()).submit(t).unwrap();
            m.sent(PaneId::FIRST, t.into());
        }
        // "one" is echoed (taken), "two" is not.
        a.event(A, 5, json!({"ev": "turn_started", "user_text": "one"}));
        pump(&mut m, &events, "the echo", |m| m.in_flight.iter().count() == 1);
        m.tabs.open(B, "b", Place::New);
        let _ = ui_switch(&mut m, &client, B, None);
        drop(a);
        let seen = effects(&mut m, &events, 1, |e| matches!(e, Effect::Unsent { .. }));
        assert_eq!(
            seen.iter().filter(|e| matches!(e, Effect::Unsent { .. })).collect::<Vec<_>>(),
            vec![&Effect::Unsent { pane: PaneId::FIRST, session: A.into(), text: "two".into() }]
        );
        assert!(m.left.is_empty(), "nothing left to follow");
        let _b = attaches.recv_timeout(T).unwrap();
    }

    /// Shady #36 L5: sends from two panes on A, unconfirmed when its
    /// attachment, left, ends: each goes back to the pane that sent it
    /// (`Effect::Unsent` names it), never the pane in use.
    #[test]
    fn unsent_input_left_behind_keeps_the_pane_that_sent_it() {
        let (client, events, attaches, mut m) = connect("unsent3");
        m.tabs.open(A, "a", Place::Current);
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.can_send());
        // A's connection is gone before the sends go out, so each is
        // refused on its own (a refusal gives back one text). Closed after
        // them, it raced the connection thread: sent first, the three were
        // given back by the end, one text per pane (n11, below).
        drop(a);
        let (p0, p1) = (PaneId::FIRST, PaneId::nth(1));
        for (p, t) in [(p0, "one"), (p1, "two"), (p0, "three")] {
            client.on(m.attachment().unwrap()).submit(t).unwrap();
            m.sent(p, t.into());
        }
        m.tabs.open(B, "b", Place::New);
        let _ = ui_switch(&mut m, &client, B, None);
        // Three refusals, three texts: wait for all of them (the first can
        // be applied before the next has come).
        let seen = effects(&mut m, &events, 3, |e| matches!(e, Effect::Unsent { .. }));
        assert_eq!(
            seen.iter().filter(|e| matches!(e, Effect::Unsent { .. })).collect::<Vec<_>>(),
            vec![
                &Effect::Unsent { pane: p0, session: A.into(), text: "one".into() },
                &Effect::Unsent { pane: p1, session: A.into(), text: "two".into() },
                &Effect::Unsent { pane: p0, session: A.into(), text: "three".into() }
            ]
        );
        let _b = attaches.recv_timeout(T).unwrap();
    }

    /// Shady #36 r2 n11: the same sends, and A's attachment, left, ends
    /// (`Detached`): what is left goes back one text per pane that sent
    /// it, in the order sent (the refusal arm above gives one at a time).
    #[test]
    fn unsent_input_left_behind_comes_back_one_text_per_pane() {
        let (client, events, attaches, mut m) = connect("unsent4");
        m.tabs.open(A, "a", Place::Current);
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.can_send());
        let seq = m.attachment().unwrap();
        let (p0, p1) = (PaneId::FIRST, PaneId::nth(1));
        for (p, t) in [(p0, "one"), (p1, "two"), (p0, "three")] {
            client.on(seq).submit(t).unwrap();
            m.sent(p, t.into());
        }
        m.tabs.open(B, "b", Place::New);
        let _ = ui_switch(&mut m, &client, B, None);
        let fx = m.apply(Event::Detached { attach_seq: seq, session_id: Some(A.into()), reason: "gone".into() });
        assert_eq!(
            fx,
            vec![
                Effect::Unsent { pane: p0, session: A.into(), text: "one\nthree".into() },
                Effect::Unsent { pane: p1, session: A.into(), text: "two".into() }
            ]
        );
        assert!(m.left.is_empty(), "nothing left to follow");
        drop(a);
        let _b = attaches.recv_timeout(T).unwrap();
    }

    /// Tabs: A (its prompt #1 on screen) in one tab, B (its own prompt #1,
    /// streaming) in another. However the tabs switch, an answer, a steer
    /// or a HALT can only reach the visible tab's session once it has
    /// attached: never the tab left, never the one coming (whose cached
    /// transcript shows A's old prompt), never a stale card.
    #[test]
    fn tabs_never_let_input_reach_a_background_session() {
        let (client, events, attaches, mut m) = connect("tabs");
        m.tabs.open(A, "a", Place::Current);
        ui_attach(&mut m, &client, A);
        let mut a1 = attaches.recv_timeout(T).unwrap();
        a1.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.may_answer(1));
        let a_card = m.card_key().unwrap();
        // Ctrl+click B: a new tab; A's state goes to the cache.
        m.tabs.open(B, "b", Place::New);
        let cached_a = ui_switch(&mut m, &client, B, None);
        assert_eq!(m.tabs.visible_session(), Some(B));
        assert!(!m.can_send() && m.card_key().is_none() && !ui_authorize(&mut m, &client, 1), "nothing drives while B attaches");
        let mut b = attaches.recv_timeout(T).unwrap();
        let mut b_frame = attached_with_prompt(B, 1);
        b_frame["streaming"] = json!(true);
        b.send(b_frame);
        pump(&mut m, &events, "B attached", |m| m.is_active(B));
        assert_ne!(m.card_key(), Some(a_card), "B's prompt #1 is another card");
        assert!(m.can_send(), "B drives (a steer goes to B)");
        assert!(ui_authorize(&mut m, &client, 1), "B's own prompt");
        // Back to A's tab: its cached transcript still shows prompt #1.
        let cached_b = ui_switch(&mut m, &client, A, Some(cached_a));
        assert_eq!(m.transcript.pending_prompts().len(), 1, "the cache shows at once");
        assert!(!m.can_send() && m.card_key().is_none() && !ui_authorize(&mut m, &client, 1), "a cached card is not answerable");
        let mut a2 = attaches.recv_timeout(T).unwrap();
        a2.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached again", |m| m.is_active(A));
        assert!(m.card_key().is_some_and(|k| k != a_card), "a new epoch: the old card's key never matches");
        assert!(!m.prompt_answered(1), "B's answer is not A's");
        assert!(ui_authorize(&mut m, &client, 1));
        drop(cached_b);
        client.shutdown().unwrap();
        let answers = |c: &mut Conn| c.cmds(Duration::from_millis(300)).into_iter().filter(|c| c["cmd"] == "answer").count();
        assert_eq!(answers(&mut a1), 0, "A's first connection: nothing (its card was never answered)");
        assert_eq!(answers(&mut b), 1, "B: its own answer, once");
        assert_eq!(answers(&mut a2), 1, "A: answered on the connection it attached on now");
    }

    /// Click landing across tabs: the card a switch brings up (a cached
    /// one, then the attached one) is new and unarmed, so the second click
    /// of a double-click that switched tabs cannot authorize it, even when
    /// the other session's prompt has the same id.
    #[test]
    fn a_tab_switch_shows_an_unarmed_card() {
        use crate::app::PromptCard;
        let (client, events, attaches, mut m) = connect("tab-arm");
        m.tabs.open(A, "a", Place::Current);
        ui_attach(&mut m, &client, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        pump(&mut m, &events, "A attached", |m| m.is_active(A) && m.may_answer(1));
        let mut card = None;
        let long_ago = Instant::now() - Duration::from_secs(5);
        assert!(PromptCard::sync(&mut card, &m, long_ago, (0., 1e4)));
        assert!(card.as_ref().unwrap().armed(), "A's card has been up for 5 s");
        // A click switches to B's tab (the view drops its card on a switch);
        // B's prompt #1 attaches.
        m.tabs.open(B, "b", Place::New);
        let _cached_a = ui_switch(&mut m, &client, B, None);
        card = None;
        assert!(!PromptCard::sync(&mut card, &m, Instant::now(), (0., 1e4)) && card.is_none(), "no card while B attaches");
        let mut b = attaches.recv_timeout(T).unwrap();
        b.send(attached_with_prompt(B, 1));
        pump(&mut m, &events, "B attached", |m| m.is_active(B) && m.may_answer(1));
        assert!(PromptCard::sync(&mut card, &m, Instant::now(), (0., 1e4)), "B's prompt #1 is a new card");
        let c = card.as_ref().unwrap();
        assert!(!c.armed(), "unarmed: the double-click's second click lands on nothing");
        assert!(!crate::app::authorize_ready(c.armed(), Some(0.), true));
        client.shutdown().unwrap();
        drop((a, b));
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
        assert!(PromptCard::sync(&mut card, &m, Instant::now() - ARM, (0., 1e4)), "a new card");
        assert!(card.as_ref().is_some_and(|c| c.armed() && c.body.contains("edit README.md")));
        assert!(!PromptCard::sync(&mut card, &m, Instant::now(), (0., 1e4)), "the same prompt keeps its card");
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
        client.on(m.attachment().unwrap()).answer_on(stale, 1, Some(CONFIRM_YES.into())).unwrap();
        pump(&mut m, &events, "A reattached", |m| !m.reattaching && m.may_answer(1));
        assert_eq!(m.active.as_ref().unwrap().attach_seq, seq, "a reconnect keeps the attach_seq");
        assert_ne!(m.active.as_ref().unwrap().connection, stale, "a new connection");
        assert!(PromptCard::sync(&mut card, &m, Instant::now(), (0., 1e4)), "the restarted daemon's #1 is a new card");
        let c = card.as_ref().unwrap();
        assert!(c.body.contains("rm -rf ~/work") && !c.body.contains("README"), "the new text: {}", c.body);
        assert!(!c.armed(), "unarmed");
        // The new card's own answer goes out.
        client.on(m.attachment().unwrap()).answer_on(m.active.as_ref().unwrap().connection, 1, Some("n".into())).unwrap();
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
                    Item::Favs { count, .. } => format!("Favs {count}"),
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

    /// Split panes step 2 (architecture §6.4, the highest product risk):
    /// two sessions attached at once, each in its own store, the client
    /// holding both attachments. What the desktop sends for a store (an
    /// Allow, HALT, a steer, a message) goes out on that store's
    /// attachment only (`client.on(store.attachment())`, the only way the
    /// desktop may send: `clippy.toml`), never to the other session. The
    /// same prompt id in both is two cards: card keys are window-wide. A
    /// refusal on B gives B's text back to the pane that sent it, never
    /// to A's composer or A's in-flight input.
    #[test]
    fn two_stores_answer_halt_steer_and_send_only_their_own_session() {
        use crate::pane::PaneId;
        let (client, events, attaches, mut m) = connect("two-stores");
        let (la, lb) = (LiveId::FIRST, m.lives.add());
        let (pa, pb) = (PaneId::FIRST, PaneId::nth(1));
        // A, as a tab switch attaches it; B alongside (step 3's second pane).
        let sa = client.attach(A, AttachMode::Mirror).unwrap();
        m.want(la, sa);
        m.asked_for(la, A);
        let mut a = attaches.recv_timeout(T).unwrap();
        a.send(attached_with_prompt(A, 1));
        let sb = client.attach_more(B, AttachMode::Mirror).unwrap();
        m.want(lb, sb);
        m.asked_for(lb, B);
        let mut b = attaches.recv_timeout(T).unwrap();
        b.send(attached_with_prompt(B, 1));
        pump(&mut m, &events, "both attached", |m| m.lives[la].may_answer(1) && m.lives[lb].may_answer(1));
        assert!(m.lives[la].is_active(A) && m.lives[lb].is_active(B), "each store has its own session");
        assert_ne!(m.lives[la].card_key(), m.lives[lb].card_key(), "prompt #1 in each: two cards, two keys");
        let seq = |m: &Model, l: LiveId| m.lives[l].attachment().expect("attached");
        // Allow on A's card (on A's connection), HALT B, a message to A, a
        // steer to B: as `AppView::answer`, `halt` and `submit` send them.
        let conn_a = m.lives[la].active.as_ref().unwrap().connection;
        client.on(seq(&m, la)).answer_on(conn_a, 1, Some(CONFIRM_YES.into())).unwrap();
        m.lives[la].answer_sent(1);
        client.on(seq(&m, lb)).abort().unwrap();
        client.on(seq(&m, la)).submit("to A").unwrap();
        m.lives[la].sent(pa, "to A".into());
        client.on(seq(&m, lb)).steer("to B").unwrap();
        m.lives[lb].sent(pb, "to B".into());
        // A's prompt answered: B's is still waiting, unanswered.
        assert!(m.lives[la].prompt_answered(1) && !m.lives[lb].prompt_answered(1));
        assert!(m.lives[lb].may_answer(1));
        // What each connection got, of input and answers (not its queries).
        let input = |c: &mut Conn| -> Vec<Value> {
            let kinds = ["answer", "submit", "steer", "cancel"];
            c.cmds(Duration::from_millis(400)).into_iter().filter(|f| kinds.contains(&f["cmd"].as_str().unwrap_or(""))).collect()
        };
        assert_eq!(
            input(&mut a),
            vec![json!({"cmd": "answer", "prompt_id": 1, "value": "y"}), json!({"cmd": "submit", "text": "to A", "attachments": []})],
            "A's connection: A's Allow and A's message only"
        );
        assert_eq!(
            input(&mut b),
            vec![json!({"cmd": "cancel"}), json!({"cmd": "steer", "text": "to B"})],
            "B's connection: B's HALT and B's steer only"
        );
        // B refuses its steer: back to the pane that sent it.
        b.event(B, 5, json!({"ev": "refused", "client": 2, "command": "steer", "reason": "input is owned by client #1"}));
        let seen = effects(&mut m, &events, 1, |e| matches!(e, Effect::Restore(..)));
        let restored: Vec<_> = seen.iter().filter(|e| matches!(e, Effect::Restore(..))).collect();
        assert_eq!(restored, vec![&Effect::Restore(pb, "to B".into())], "{seen:?}");
        assert_eq!(m.lives[la].in_flight.iter().collect::<Vec<_>>(), vec!["to A"], "A's message is still A's, in flight");
        assert!(m.lives[lb].in_flight.is_empty());
        client.shutdown().unwrap();
    }
}
