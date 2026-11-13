//! The window's state and how client events change it. No GPUI: events are
//! applied here as they arrive (in the event pump, not in `render`), so a
//! hidden or minimized window holds the current transcript and nothing
//! else; the view only turns the transcript's splice into rows per frame.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use synaps_client::protocol::SessionMeta;
use synaps_client::{
    AttachMode, Error, Event, Expect, FEATURE_SAVED_SESSIONS, RenameRefusal, SavedSession, SessionEvent, Status, Transcript,
    validate_session_name,
};

use crate::named::{Marker, NamedRow, ResumeRequest, RowAction};

/// The saved list is re-asked at most this often on its own (on startup,
/// after a rename and after a resume it is asked at once).
pub const SAVED_REFRESH: Duration = Duration::from_secs(30);
/// `saved_sessions` asks for at most this many.
pub const SAVED_LIMIT: u32 = 200;

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
    pub mode: AttachMode,
}

/// What the view must do after an event (it owns the client and composer).
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Input the daemon refused (or never got): put it back in the composer.
    Restore(String),
    ListSessions,
    Resync,
    /// Ask for the saved named sessions (`Client::saved_sessions`); if it
    /// cannot be sent, tell [`Model::saved_failed`].
    ListSaved,
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
    /// Leave the current attachment (`Client::detach`).
    Detach,
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

/// The Named section's state.
#[derive(Debug, Default)]
pub struct Named {
    /// The daemon lists `saved_sessions` (else: live named sessions only).
    pub saved_supported: bool,
    pub saved: Vec<SavedSession>,
    /// The daemon has more than [`SAVED_LIMIT`].
    pub saved_truncated: bool,
    /// The saved list's own trouble (failed, entries skipped).
    pub saved_note: Option<String>,
    saved_at: Option<Instant>,
    saved_in_flight: bool,
    /// A forced refresh came while one was in flight: ask again after it.
    saved_dirty: bool,
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
    /// An `Attached` that was not the session asked for, being detached.
    discard: Option<u64>,
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
    pub transcript: Transcript,
    pub last_error: Option<String>,
    /// The session connection is being re-made (reload).
    pub reattaching: bool,
    /// Sent, not yet confirmed by `TurnStarted` / `Steered`, oldest first:
    /// given back to the user on a refusal (the oldest) or a detach (all).
    in_flight: VecDeque<String>,
    pub named: Named,
}

impl Model {
    pub fn new(conn: ConnState) -> Self {
        Self {
            conn,
            sessions: Vec::new(),
            active: None,
            wanted: None,
            transcript: Transcript::new(),
            last_error: None,
            reattaching: false,
            in_flight: VecDeque::new(),
            named: Named { daemon: "the daemon's machine".into(), ..Named::default() },
        }
    }

    /// `Effect::ListSaved` if the daemon supports it and it is due (`force`:
    /// startup, after a rename or a resume), never two at once (a forced
    /// one during a flight is asked when that lands).
    fn want_saved(&mut self, now: Instant, force: bool, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        if !n.saved_supported {
            return;
        }
        if n.saved_in_flight {
            n.saved_dirty |= force;
            return;
        }
        if force || n.saved_at.is_none_or(|t| now.duration_since(t) >= SAVED_REFRESH) {
            n.saved_in_flight = true;
            n.saved_dirty = false;
            n.saved_at = Some(now);
            effects.push(Effect::ListSaved);
        }
    }

    /// The saved list did not come (or could not be asked for).
    pub fn saved_failed(&mut self, reason: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.on_saved_failed(reason, &mut effects);
        effects
    }

    fn on_saved_failed(&mut self, reason: &str, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        n.saved_in_flight = false;
        n.saved_dirty = false;
        n.saved_note = Some(format!("Saved sessions unavailable: {reason}"));
        // An Open by name that waited for the list: the daemon resolves it.
        if n.opening.as_ref().is_some_and(|o| o.attach_seq.is_none()) {
            self.open_by_resolver(effects);
        }
    }

    /// The Named rows: live named + saved named not live, alphabetical; the
    /// one being resumed says so.
    pub fn named_rows(&self) -> Vec<NamedRow> {
        let saved: &[SavedSession] = if self.named.saved_supported { &self.named.saved } else { &[] };
        let mut rows = crate::named::named_rows(&self.sessions, saved);
        if let Some(r) = &self.named.resuming {
            for row in &mut rows {
                if matches!(&row.action, RowAction::Resume(q) if q.target == r.req.target) || row.name == r.req.label {
                    row.marker = Marker::Resuming;
                }
            }
        }
        rows
    }

    /// The live sessions without a name (the list below Named).
    pub fn unnamed_sessions(&self) -> impl Iterator<Item = &SessionMeta> {
        crate::named::unnamed(&self.sessions)
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
            RowAction::Attach(id) => vec![Effect::Attach { session_id: id.clone(), fallback: self.journal_of(id), open: false }],
            RowAction::Resume(req) => vec![Effect::Resume { req: req.clone(), open: false }],
        }
    }

    /// "Open by name…" was opened (again): a fresh start.
    pub fn open_shown(&mut self) {
        self.named.error = None;
        self.named.opened = false;
    }

    /// "Open by name…": the name exactly, never a partial id. A live
    /// session of that name is attached; a saved one is resumed by its
    /// journal id in its folder. Not in the lists: a daemon with the saved
    /// list is asked once more (it may be new), then it is "no session
    /// named"; a daemon without it resolves the name itself, and the
    /// attached session must carry that name.
    pub fn open_by_name(&mut self, typed: &str) -> Vec<Effect> {
        self.open_by_name_at(typed, Instant::now())
    }

    pub fn open_by_name_at(&mut self, typed: &str, now: Instant) -> Vec<Effect> {
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
        self.resolve_open(now, &mut effects);
        effects
    }

    /// Resolve a waiting Open by name against the lists (exact name).
    fn resolve_open(&mut self, now: Instant, effects: &mut Vec<Effect>) {
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
            if let Some(v) = n.saved.iter().find(|v| v.name.as_deref() == Some(name.as_str())) {
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
                    self.want_saved(now, true, effects);
                }
                return;
            }
        }
        self.open_by_resolver(effects);
    }

    /// No list can resolve the name: let the daemon, and check on attach.
    fn open_by_resolver(&mut self, effects: &mut Vec<Effect>) {
        let Some(o) = &self.named.opening else { return };
        let req = ResumeRequest { label: o.name.clone(), target: o.name.clone(), cwd: None, by_name: true };
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

    /// Events as of now.
    pub fn apply(&mut self, ev: Event) -> Vec<Effect> {
        self.apply_at(ev, Instant::now())
    }

    /// The user asked for an attachment (`Client::attach`/`create`/`resume`
    /// returned `attach_seq`). Whatever the previous one left in the Named
    /// section (a resume, an Open by name, the naming row and its Replace
    /// offer) belonged to it and goes; a rename already sent still gets
    /// its answer.
    pub fn want(&mut self, attach_seq: u64) {
        self.wanted = Some(attach_seq);
        let n = &mut self.named;
        n.resuming.take_if(|r| r.attach_seq != attach_seq);
        n.attaching.take_if(|(s, _)| *s != attach_seq);
        n.opening.take_if(|o| o.attach_seq != Some(attach_seq));
        n.discard = None;
        n.naming = false;
        n.replace = None;
        n.rename_error = None;
    }

    /// A local session (benches): its events carry [`LOCAL_SEQ`].
    pub fn set_local_session(&mut self, session_id: &str) {
        self.wanted = Some(LOCAL_SEQ);
        self.active = Some(Active { session_id: session_id.into(), attach_seq: LOCAL_SEQ, mode: AttachMode::Mirror });
    }

    /// Attached, connected and allowed to drive.
    pub fn can_send(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.mode != AttachMode::Observe) && !self.reattaching
    }

    /// `text` went to the daemon.
    pub fn sent(&mut self, text: String) {
        self.in_flight.push_back(text);
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

    /// Apply one event; `now` paces the saved-list refresh.
    pub fn apply_at(&mut self, ev: Event, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        match ev {
            Event::Status(s) => {
                // A reply in flight died with the old connection.
                self.named.saved_in_flight = false;
                if let Status::Connected { features, profile, .. } = &s {
                    self.named.daemon = match profile {
                        Some(p) => format!("the daemon's machine (profile {p})"),
                        None => "the daemon's machine".into(),
                    };
                    self.named.saved_supported = features.iter().any(|f| f == FEATURE_SAVED_SESSIONS);
                    if self.named.saved_supported {
                        self.want_saved(now, true, &mut effects);
                    } else {
                        self.named.saved.clear();
                        self.named.saved_note = None;
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
                self.sessions = s;
                self.want_saved(now, false, &mut effects);
            }
            Event::SavedSessions { sessions, truncated, rejected } => {
                let n = &mut self.named;
                n.saved = sessions;
                n.saved_truncated = truncated;
                n.saved_in_flight = false;
                n.saved_note = (rejected > 0).then(|| format!("{rejected} saved sessions could not be read (skipped)."));
                // A refresh asked meanwhile (after a rename, or for an Open
                // by name) goes out now, and the Open by name waits for it.
                if std::mem::take(&mut n.saved_dirty) {
                    self.want_saved(now, true, &mut effects);
                } else {
                    self.resolve_open(now, &mut effects);
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
                self.want_saved(now, true, &mut effects);
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
                    // The daemon's resolver takes any id containing the
                    // text when no name matches: not the session asked for.
                    if r.req.by_name && attached.meta.name.as_deref() != Some(r.req.label.as_str()) {
                        self.named.error = Some(format!("No session named '{}'.", r.req.label));
                        self.named.opening = None;
                        self.named.discard = Some(attach_seq);
                        effects.push(Effect::Detach);
                        return effects;
                    }
                    if r.req.by_name {
                        self.named.notice =
                            Some(format!("Opened '{}' by name: this daemon does not list saved folders, so it runs in ~.", r.req.label));
                    }
                    self.want_saved(now, true, &mut effects);
                }
                self.named.attaching.take_if(|(s, _)| *s == attach_seq);
                if self.named.opening.take_if(|o| o.attach_seq == Some(attach_seq)).is_some() {
                    self.named.opened = true;
                }
                self.named.error = None;
                self.active = Some(Active { session_id: attached.meta.id.clone(), attach_seq, mode });
                self.reattaching = false;
                self.last_error = None;
                if let Err(e) = self.transcript.load_attached(&attached) {
                    self.last_error = Some(e.to_string());
                }
                effects.push(Effect::ListSessions);
            }
            Event::Reattaching { attach_seq, .. } if self.current(attach_seq) => self.reattaching = true,
            Event::Session { attach_seq, event, .. } if self.current(attach_seq) => {
                match &event {
                    SessionEvent::TurnStarted { user_text: Some(t), .. } | SessionEvent::Steered { text: t, .. } => {
                        if let Some(i) = self.in_flight.iter().position(|s| s.trim() == t.trim()) {
                            self.in_flight.remove(i);
                        }
                    }
                    _ => {}
                }
                self.transcript.apply(&event);
            }
            Event::History { attach_seq, update, .. } if self.current(attach_seq) => {
                if self.transcript.apply_update(&update).is_err() {
                    effects.push(Effect::Resync);
                }
            }
            Event::Detached { attach_seq, reason, .. } if self.current(attach_seq) => {
                self.active = None;
                self.reattaching = false;
                self.transcript.set_detached();
                self.give_back_all(&mut effects);
                let n = &mut self.named;
                if n.discard.take_if(|s| *s == attach_seq).is_some() {
                    return effects;
                }
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
                let n = &mut self.named;
                n.saved_supported = false;
                n.saved_in_flight = false;
                n.saved.clear();
                n.saved_note = None;
                if n.opening.as_ref().is_some_and(|o| o.attach_seq.is_none()) {
                    self.open_by_resolver(&mut effects);
                }
            }
            Event::Error { attach_seq, error } if attach_seq.is_none_or(|s| self.current(s)) => {
                let input = matches!(&error, Error::Refused { command: None, .. } | Error::NotAttached { .. })
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
        fx.contains(&Effect::ListSaved)
    }

    /// Feature gating: a daemon without `saved_sessions` is never asked
    /// (no `ListSaved`, however many session lists or renames arrive); one
    /// with it is asked on connect, then at most every 30 s, never twice at
    /// once, and again at once after a rename.
    #[test]
    fn the_saved_list_is_asked_only_of_a_daemon_that_has_it_and_paced() {
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        assert!(!list_saved(&m.apply_at(connected(&["owned_sessions"]), t0)));
        for i in 0..20 {
            let fx = m.apply_at(Event::Sessions(Vec::new()), t0 + SAVED_REFRESH * i);
            assert!(!list_saved(&fx));
        }
        let fx = m.apply_at(Event::Renamed { session_id: "s".into(), name: Some("a".into()), in_memory_only: false }, t0);
        assert!(!list_saved(&fx) && fx.contains(&Effect::ListSessions));
        assert!(m.named_rows().is_empty());

        let mut m = Model::new(ConnState::Connecting);
        assert!(list_saved(&m.apply_at(connected(&["saved_sessions"]), t0)), "asked on startup");
        assert!(!list_saved(&m.apply_at(Event::Sessions(Vec::new()), t0 + SAVED_REFRESH * 2)), "one in flight");
        m.apply_at(saved_list(json!([])), t0 + Duration::from_secs(1));
        assert!(!list_saved(&m.apply_at(Event::Sessions(Vec::new()), t0 + Duration::from_secs(29))), "not due yet");
        assert!(list_saved(&m.apply_at(Event::Sessions(Vec::new()), t0 + SAVED_REFRESH)), "due after 30 s");
        m.apply_at(saved_list(json!([])), t0 + SAVED_REFRESH);
        let t1 = t0 + SAVED_REFRESH + Duration::from_secs(2);
        let fx = m.apply_at(Event::Renamed { session_id: "s".into(), name: Some("a".into()), in_memory_only: false }, t1);
        assert!(list_saved(&fx), "after a rename at once");
        // A dropped connection frees the slot; an older daemon behind the
        // same socket (Unsupported) stops the asking and the saved rows.
        m.apply_at(Event::Status(Status::Reconnecting { why: "x".into() }), t1);
        m.apply_at(saved_list(json!([{"id": "j", "name": "kept", "live": false}])), t1);
        assert_eq!(m.named_rows().len(), 1);
        m.apply_at(Event::Error { attach_seq: None, error: Error::Unsupported { feature: "saved_sessions" } }, t1);
        assert!(m.named_rows().is_empty());
        assert!(m.last_error.is_none(), "not a transcript error");
        assert!(!list_saved(&m.apply_at(Event::Sessions(Vec::new()), t1 + SAVED_REFRESH * 3)));
    }

    /// The rail through the model: live named + saved named not live,
    /// deduped and alphabetical; unnamed live sessions below.
    #[test]
    fn the_rail_groups_live_and_saved_sessions() {
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&["saved_sessions"]), t0);
        m.apply_at(
            Event::Sessions(metas(json!([
                {"id": "l1", "name": "zed", "lifecycle": "live", "pinned": true},
                {"id": "l2", "lifecycle": "live"},
                {"id": "l3", "name": "axel", "lifecycle": "live", "journal_id": "20261110-202033-e5f6", "awaiting_input": 1},
            ]))),
            t0,
        );
        let fixture =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../synaps-client/tests/fixtures/saved_session_list.json"))
                .unwrap();
        let DaemonFrame::SavedSessionList { sessions, truncated, .. } = decode_frame(fixture.trim()).unwrap() else { panic!() };
        m.apply_at(Event::SavedSessions { sessions, truncated, rejected: 0 }, t0);
        let rows: Vec<(String, Marker)> = m.named_rows().into_iter().map(|r| (r.name, r.marker)).collect();
        let want = [("axel", Marker::Awaiting), ("bebop", Marker::Saved), ("old-notes", Marker::Saved), ("zed", Marker::Pinned)];
        assert_eq!(rows, want.map(|(n, k)| (n.to_string(), k)).to_vec());
        assert_eq!(m.unnamed_sessions().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["l2"]);
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
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&["saved_sessions"]), t0);
        m.apply_at(saved_list(json!([{"id": "j-bebop", "name": "bebop", "cwd": "/home/u/Projects/bebop", "live": false}])), t0);
        let row = m.named_rows().remove(0);
        let fx = m.open_row(&row.action);
        let [Effect::Resume { req: bebop, open: false }] = fx.as_slice() else { panic!("{fx:?}") };
        assert_eq!(bebop.target, "j-bebop");
        let o = bebop.options();
        assert_eq!((o.cwd, o.fallback_cwd), (Cwd::Path("/home/u/Projects/bebop".into()), Some(Cwd::Home)));
        m.resuming(bebop.clone(), 7, false);
        assert_eq!(m.named_rows()[0].marker, Marker::Resuming);
        m.apply_at(Event::CwdFallback { attach_seq: 7, missing: "/home/u/Projects/bebop".into(), cwd: None }, t0);
        assert_eq!(m.named.notice.as_deref(), Some("Folder /home/u/Projects/bebop is gone on the daemon's machine; resumed in ~."));
        let fx = m.apply_at(attached_ev(7, attached("live-bebop")), t0);
        assert!(m.named.resuming.is_none());
        assert!(list_saved(&fx), "the resumed session left the saved list");
        assert_eq!(m.active.as_ref().unwrap().session_id, "live-bebop");

        m.resuming(req_("ghost"), 8, false);
        m.apply_at(detached(8, crate::named::NO_SUCH_GHOST), t0);
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(m.named.resuming.is_none() && m.active.is_none() && m.last_error.is_none());

        m.resuming(req("axel", "j-axel", None), 9, false);
        m.apply_at(detached(9, crate::named::LOCKED_AXEL), t0);
        assert!(m.named.error.as_deref().unwrap().contains("locked by another process"));
        // A plain detach is still a transcript error; an attach clears the
        // Named section's error (L4).
        attach(&mut m, 10);
        assert!(m.named.error.is_none());
        m.apply_at(detached(10, "session ended"), t0);
        assert_eq!(m.last_error.as_deref(), Some("detached: session ended"));
    }

    fn req_(name: &str) -> ResumeRequest {
        req(name, name, None)
    }

    /// M1: clicking elsewhere before a resume lands drops its "resuming…"
    /// (the client stops it; that `Detached` is stale and says nothing).
    #[test]
    fn switching_away_from_a_resume_clears_it() {
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&["saved_sessions"]), t0);
        m.apply_at(saved_list(json!([{"id": "j-bebop", "name": "bebop", "live": false}])), t0);
        m.resuming(req("bebop", "j-bebop", None), 7, false);
        assert_eq!(m.named_rows()[0].marker, Marker::Resuming);
        m.want(8);
        assert_eq!(m.named_rows()[0].marker, Marker::Saved);
        m.apply_at(detached(7, "replaced by another attach"), t0);
        assert_eq!(m.named_rows()[0].marker, Marker::Saved);
        assert!(m.named.error.is_none() && m.last_error.is_none());
    }

    /// "Open by name…" (M3): the name exactly. A live session of that name
    /// is attached; a saved one is resumed by its journal id, in its
    /// folder. What only an id contains is not a name: the saved list is
    /// asked once more (the name may be new), then it is "no session
    /// named". The typed text stays until `Attached` (`opened`).
    #[test]
    fn open_by_name_is_exact() {
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&["saved_sessions"]), t0);
        m.apply_at(
            Event::Sessions(metas(json!([{"id": "l1", "name": "live-one", "journal_id": "j-l1", "cwd": "/w", "lifecycle": "live"}]))),
            t0,
        );
        let axel = json!({"id": "20261110-202033-e5f6", "name": "axel", "cwd": "/home/u/axel", "live": false});
        m.apply_at(saved_list(json!([axel])), t0);

        assert!(m.open_by_name_at("Axel Memory", t0).is_empty());
        assert!(m.named.error.as_deref().unwrap().contains("lowercase"));
        assert_eq!(
            m.open_by_name_at(" live-one ", t0),
            vec![Effect::Attach { session_id: "l1".into(), fallback: Some(req("live-one", "j-l1", Some("/w"))), open: true }]
        );
        assert!(m.named.error.is_none());
        let want = Effect::Resume { req: req("axel", "20261110-202033-e5f6", Some("/home/u/axel")), open: true };
        assert_eq!(m.open_by_name_at("axel", t0), vec![want.clone()]);

        // Part of an id, not a name: asked again, then refused locally.
        assert_eq!(m.open_by_name_at("64fb", t0), vec![Effect::ListSaved]);
        assert!(m.apply_at(saved_list(json!([axel])), t0).is_empty());
        assert_eq!(m.named.error.as_deref(), Some("No session named '64fb'."));
        assert!(m.named.opening.is_none());
        // A name made since the last list is in the fresh one.
        assert_eq!(m.open_by_name_at("new-one", t0), vec![Effect::ListSaved]);
        let fx = m.apply_at(saved_list(json!([axel, {"id": "j-new", "name": "new-one", "live": false}])), t0);
        assert_eq!(fx, vec![Effect::Resume { req: req("new-one", "j-new", None), open: true }]);

        // The outcome: the input clears on `Attached`, not before.
        m.open_by_name_at("axel", t0);
        m.resuming(req("axel", "20261110-202033-e5f6", Some("/home/u/axel")), 5, true);
        assert!(!m.named.opened && m.named.opening.is_some());
        m.apply_at(attached_ev(5, attached_named("live-axel", Some("axel"))), t0);
        assert!(m.named.opened && m.named.opening.is_none());
    }

    /// M3 on a daemon without the saved list: it resolves the name, in `~`
    /// (no folder known, and said so), and the attached session must carry
    /// that name: the daemon's resolver also takes any id containing it,
    /// and such a session is detached at once, quietly. A failure keeps
    /// the typed text.
    #[test]
    fn open_by_name_without_the_saved_list_checks_the_name() {
        use synaps_client::Cwd;
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&["owned_sessions"]), t0);
        let by_name = ResumeRequest { by_name: true, ..req_("fe") };
        assert_eq!(m.open_by_name_at("fe", t0), vec![Effect::Resume { req: by_name.clone(), open: true }]);
        assert_eq!((by_name.options().cwd, by_name.options().fallback_cwd), (Cwd::Home, None));
        m.resuming(by_name.clone(), 3, true);
        let fx = m.apply_at(attached_ev(3, attached_named("20261110-202033-64fe", None)), t0);
        assert_eq!(fx, vec![Effect::Detach]);
        assert_eq!(m.named.error.as_deref(), Some("No session named 'fe'."));
        assert!(m.active.is_none() && !m.named.opened);
        m.apply_at(detached(3, "detached"), t0);
        assert!(m.last_error.is_none(), "the detach we asked for is quiet");

        let axel = ResumeRequest { by_name: true, ..req_("axel") };
        m.open_by_name_at("axel", t0);
        m.resuming(axel, 4, true);
        m.apply_at(attached_ev(4, attached_named("s-axel", Some("axel"))), t0);
        assert!(m.named.opened && m.named.error.is_none());
        assert!(m.named.notice.as_deref().unwrap().contains("runs in ~"));
        assert_eq!(m.active.as_ref().unwrap().session_id, "s-axel");

        m.open_by_name_at("ghost", t0);
        m.resuming(ResumeRequest { by_name: true, ..req_("ghost") }, 5, true);
        m.apply_at(detached(5, crate::named::NO_SUCH_GHOST), t0);
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(!m.named.opened, "the typed text stays");
    }

    /// L5: a Named live row whose session ended since the last list
    /// (`unknown session`) is resumed from its journal, in its folder.
    #[test]
    fn a_named_live_session_that_ended_is_resumed() {
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&[]), t0);
        m.apply_at(
            Event::Sessions(metas(
                json!([{"id": "l3", "name": "axel", "journal_id": "j-axel", "cwd": "/home/u/axel", "lifecycle": "parked"}]),
            )),
            t0,
        );
        let row = m.named_rows().remove(0);
        let fx = m.open_row(&row.action);
        let fallback = Some(req("axel", "j-axel", Some("/home/u/axel")));
        assert_eq!(fx, vec![Effect::Attach { session_id: "l3".into(), fallback: fallback.clone(), open: false }]);
        m.attaching(4, fallback.clone(), false);
        // daemon/conn.rs: `DaemonFrame::Error { message: "unknown session" }`.
        let fx = m.apply_at(detached(4, "unknown session"), t0);
        assert_eq!(fx, vec![Effect::Resume { req: fallback.clone().unwrap(), open: false }]);
        assert!(m.last_error.is_none());
        // Any other end is reported as before.
        m.attaching(5, fallback, false);
        m.apply_at(detached(5, "daemon closed the connection"), t0);
        assert_eq!(m.last_error.as_deref(), Some("detached: daemon closed the connection"));
    }

    /// M4 / L2 / L7: a failed saved list stays in the Named section (not
    /// the transcript), frees the slot and is asked again 30 s later; a
    /// forced refresh during a flight goes out when it lands; skipped
    /// entries and a truncated list are said.
    #[test]
    fn the_saved_lists_own_failures_and_refreshes() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut m = Model::new(ConnState::Connecting);
        assert!(list_saved(&m.apply_at(connected(&["saved_sessions"]), t0)));
        m.apply_at(Event::SavedSessionsFailed { reason: "saved sessions: Permission denied (os error 13)".into() }, t0);
        assert!(m.named.saved_note.as_deref().unwrap().contains("Permission denied"));
        assert!(m.last_error.is_none());
        assert!(!list_saved(&m.apply_at(Event::Sessions(Vec::new()), t0 + s(10))));
        assert!(list_saved(&m.apply_at(Event::Sessions(Vec::new()), t0 + SAVED_REFRESH)));
        // A rename while that one is in flight: asked again when it lands.
        let renamed = Event::Renamed { session_id: "s".into(), name: Some("a".into()), in_memory_only: false };
        assert!(!list_saved(&m.apply_at(renamed, t0 + s(31))));
        let fx = m.apply_at(Event::SavedSessions { sessions: Vec::new(), truncated: true, rejected: 2 }, t0 + s(32));
        assert!(list_saved(&fx), "the forced refresh goes out now");
        assert_eq!(m.named.saved_note.as_deref(), Some("2 saved sessions could not be read (skipped)."));
        assert!(m.named.saved_truncated);
        assert!(m.last_error.is_none());
    }

    /// "Name this session…" (M2, L3): validated first, one at a time, the
    /// name it already has closes the row, every refusal a sentence; a
    /// `Named` refusal offers to replace the name on THAT session,
    /// compare-and-set on the name the user saw; `Renamed` moves the
    /// session into the Named section at once.
    #[test]
    fn naming_the_current_session() {
        use synaps_client::RenameFailure;
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        assert!(m.rename_request("axon").is_none());
        assert_eq!(m.named.rename_error.as_deref(), Some("Not attached to a session."));
        attach(&mut m, 1);
        m.apply_at(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "name": "kite"}, {"id": "t", "lifecycle": "live"}]))), t0);
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
        m.apply_at(Event::RenameFailed(f), t0);
        assert_eq!(m.named.replace, Some(Replace { session_id: "s".into(), asked: "axon".into(), current: "kite".into() }));
        assert_eq!(m.named.rename_error.as_deref(), Some("This session is already named 'kite'."));
        let r = m.replace_request().unwrap();
        assert_eq!(r, RenameRequest { session_id: "s".into(), name: "axon".into(), expect: Expect::Named("kite".into()) });
        // A reply for another session is not ours.
        m.apply_at(Event::RenameFailed(RenameFailure::new("other", RenameRefusal::Busy, "")), t0);
        assert!(m.named.rename.is_some());
        let fx = m.apply_at(Event::Renamed { session_id: "s".into(), name: Some("axon".into()), in_memory_only: false }, t0);
        assert!(fx.contains(&Effect::ListSessions));
        assert!(!m.named.naming && m.named.rename.is_none() && m.named.replace.is_none());
        assert_eq!(m.active_name(), Some("axon"));
        assert_eq!(m.named_rows()[0].name, "axon");
        assert_eq!(m.unnamed_sessions().count(), 1);

        // A double Save: "already named what you asked" is success.
        m.toggle_naming();
        m.rename_request("zeta").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("zeta".into());
        m.apply_at(Event::RenameFailed(f), t0);
        assert!(!m.named.naming && m.named.rename.is_none() && m.named.rename_error.is_none() && m.named.replace.is_none());

        m.toggle_naming();
        m.rename_request("taken").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Taken, "taken");
        f.taken_by = Some("20261028-045932-a1b2".into());
        m.apply_at(Event::RenameFailed(f), t0);
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
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        attach(&mut m, 1);
        m.apply_at(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "name": "kite"}, {"id": "t", "lifecycle": "live"}]))), t0);
        m.toggle_naming();
        m.rename_request("axon").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("kite".into());
        m.apply_at(Event::RenameFailed(f), t0);
        assert!(m.named.replace.is_some());
        m.want(2);
        m.apply_at(attached_ev(2, attached("t")), t0);
        assert!(!m.named.naming && m.named.replace.is_none() && m.named.rename_error.is_none());
        assert!(m.replace_request().is_none(), "no offer to take to t");

        // A late answer for s while t is shown.
        attach(&mut m, 3);
        m.toggle_naming();
        m.rename_request("axon").unwrap();
        m.want(4);
        m.apply_at(attached_ev(4, attached("t")), t0);
        m.apply_at(Event::RenameFailed(RenameFailure::new("s", RenameRefusal::Busy, "")), t0);
        assert!(!m.named.naming);
        assert!(m.named.error.as_deref().unwrap().starts_with("Naming a session 'axon': "), "{:?}", m.named.error);

        // The row was closed (Cancel) on the session the answer is for.
        attach(&mut m, 5);
        m.toggle_naming();
        m.rename_request("axon").unwrap();
        m.named.naming = false;
        m.apply_at(Event::RenameFailed(RenameFailure::new("s", RenameRefusal::Busy, "")), t0);
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
