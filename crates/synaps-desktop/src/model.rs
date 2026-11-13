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
    AttachMode, Error, Event, FEATURE_SAVED_SESSIONS, RenameRefusal, SavedSession, SessionEvent, Status, Transcript,
    validate_session_name,
};

use crate::named::{Marker, NamedRow, RowAction};

/// The saved list is re-asked at most this often on its own (on startup,
/// after a rename and after a resume it is asked at once).
pub const SAVED_REFRESH: Duration = Duration::from_secs(30);
/// `saved_sessions` asks for at most this many.
pub const SAVED_LIMIT: usize = 200;

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
    /// Ask for the saved named sessions (`Client::saved_sessions`).
    ListSaved,
}

/// A resume in flight (`Client::resume`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resuming {
    /// What the user knows it by (the name).
    pub label: String,
    /// What was sent (`continue_session`): a journal id or a name.
    pub target: String,
    pub attach_seq: u64,
}

/// A rename sent, not answered yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRename {
    pub session_id: String,
    pub name: String,
}

/// The Named section's state.
#[derive(Debug, Default)]
pub struct Named {
    /// The daemon lists `saved_sessions` (else: live named sessions only).
    pub saved_supported: bool,
    pub saved: Vec<SavedSession>,
    saved_at: Option<Instant>,
    saved_in_flight: bool,
    pub resuming: Option<Resuming>,
    /// The last resume / open-by-name error.
    pub error: Option<String>,
    pub rename: Option<PendingRename>,
    /// The last rename error, for the naming row.
    pub rename_error: Option<String>,
    /// After `RenameRefusal::Named`: `(asked, current)`; the user may
    /// replace the current name (resend with `force`).
    pub replace: Option<(String, String)>,
    /// Set by a successful rename: the naming row can close.
    pub renamed: bool,
}

/// The `attach_seq` of local (bench) sessions fed straight into the pump.
pub const LOCAL_SEQ: u64 = u64::MAX;

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
            named: Named::default(),
        }
    }

    /// `Effect::ListSaved` if the daemon supports it and it is due (`force`:
    /// startup, after a rename or a resume), never two at once.
    fn want_saved(&mut self, now: Instant, force: bool, effects: &mut Vec<Effect>) {
        let n = &mut self.named;
        let due = force || n.saved_at.is_none_or(|t| now.duration_since(t) >= SAVED_REFRESH);
        if n.saved_supported && !n.saved_in_flight && due {
            n.saved_in_flight = true;
            n.saved_at = Some(now);
            effects.push(Effect::ListSaved);
        }
    }

    /// `Effect::ListSaved` could not be sent.
    pub fn saved_not_sent(&mut self) {
        self.named.saved_in_flight = false;
    }

    /// The Named rows: live named + saved named not live, alphabetical; the
    /// one being resumed says so.
    pub fn named_rows(&self) -> Vec<NamedRow> {
        let saved: &[SavedSession] = if self.named.saved_supported { &self.named.saved } else { &[] };
        let mut rows = crate::named::named_rows(&self.sessions, saved);
        if let Some(r) = &self.named.resuming {
            for row in &mut rows {
                if matches!(&row.action, RowAction::Resume { id, .. } if *id == r.target) || row.name == r.label {
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

    /// "Open by name…": check what was typed. `Ok(name)`: resume it.
    pub fn open_by_name(&mut self, typed: &str) -> Result<String, String> {
        let name = typed.trim();
        match validate_session_name(name) {
            Ok(()) => Ok(name.to_string()),
            Err(e) => {
                let msg = e.to_string();
                self.named.error = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// A resume went out (`Client::resume` returned `attach_seq`).
    pub fn resuming(&mut self, label: &str, target: &str, attach_seq: u64) {
        self.want(attach_seq);
        self.named.error = None;
        self.named.resuming = Some(Resuming { label: label.into(), target: target.into(), attach_seq });
    }

    /// "Name this session…": check the name. `Ok((session_id, name))`:
    /// send `Client::rename(session_id, Some(name), force)`.
    pub fn rename_request(&mut self, typed: &str) -> Result<(String, String), String> {
        let name = typed.trim();
        let n = &mut self.named;
        n.renamed = false;
        n.replace = None;
        let result = match (&self.active, validate_session_name(name)) {
            (None, _) => Err("Not attached to a session.".to_string()),
            (_, Err(e)) => Err(e.to_string()),
            (Some(a), Ok(())) => Ok((a.session_id.clone(), name.to_string())),
        };
        match &result {
            Ok((session_id, name)) => {
                n.rename_error = None;
                n.rename = Some(PendingRename { session_id: session_id.clone(), name: name.clone() });
            }
            Err(e) => n.rename_error = Some(e.clone()),
        }
        result
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

    /// The user asked for an attachment (`Client::attach`/`create` returned
    /// `attach_seq`).
    pub fn want(&mut self, attach_seq: u64) {
        self.wanted = Some(attach_seq);
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
                if let Status::Connected { features, .. } = &s {
                    self.named.saved_supported = features.iter().any(|f| f == FEATURE_SAVED_SESSIONS);
                    if self.named.saved_supported {
                        self.want_saved(now, true, &mut effects);
                    } else {
                        self.named.saved.clear();
                    }
                }
                self.conn = match s {
                    Status::Connecting => ConnState::Connecting,
                    Status::Connected { daemon_version, profile, .. } => ConnState::Connected(format!(
                        "{}{}",
                        daemon_version,
                        profile.map(|p| format!(" · {p}")).unwrap_or_default()
                    )),
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
            Event::SavedSessions { sessions, .. } => {
                self.named.saved = sessions;
                self.named.saved_in_flight = false;
            }
            Event::Renamed { session_id, name, .. } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.name = name;
                }
                if self.named.rename.as_ref().is_some_and(|r| r.session_id == session_id) {
                    self.named.rename = None;
                    self.named.rename_error = None;
                    self.named.replace = None;
                    self.named.renamed = true;
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
                self.named.rename_error = Some(crate::named::rename_message(&f, &asked));
                if f.reason == RenameRefusal::Named
                    && let Some(current) = f.current_name.clone()
                {
                    self.named.replace = Some((asked, current));
                }
            }
            Event::Attached { attach_seq, attached, mode, .. } if self.current(attach_seq) => {
                if self.named.resuming.as_ref().is_some_and(|r| r.attach_seq == attach_seq) {
                    self.named.resuming = None;
                    self.want_saved(now, true, &mut effects);
                }
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
                match self.named.resuming.take_if(|r| r.attach_seq == attach_seq) {
                    Some(r) => {
                        self.named.error = Some(crate::named::resume_error(&r.label, &reason));
                        self.last_error = None;
                    }
                    None => self.last_error = Some(format!("detached: {reason}")),
                }
                self.give_back_all(&mut effects);
            }
            // The saved list's own failures stay in the Named section.
            Event::Error { attach_seq: None, error: Error::Unsupported { .. } } => {
                self.named.saved_supported = false;
                self.named.saved_in_flight = false;
                self.named.saved.clear();
            }
            Event::Error { attach_seq: None, error: Error::Refused { command: Some(c), .. } } if c == "saved_sessions" => {
                self.named.saved_in_flight = false;
            }
            Event::Error { attach_seq, error } if attach_seq.is_none_or(|s| self.current(s)) => {
                let input = matches!(
                    &error,
                    Error::Refused { command: None, .. } | Error::NotAttached { .. }
                ) || matches!(&error, Error::Refused { command: Some(c), .. } if c == "submit" || c == "steer");
                if input {
                    self.give_back_oldest(&mut effects);
                }
                self.last_error = Some(error.to_string());
            }
            _ => {}
        }
        effects
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
            let messages: Vec<Value> =
                (0..50).map(|i| json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": format!("round {round} msg {i}")})).collect();
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
        Event::SavedSessions { sessions: serde_json::from_value(v).unwrap(), truncated: false }
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
        let fixture = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../synaps-client/tests/fixtures/saved_session_list.json")).unwrap();
        let DaemonFrame::SavedSessionList { sessions, truncated, .. } = decode_frame(fixture.trim()).unwrap() else { panic!() };
        m.apply_at(Event::SavedSessions { sessions, truncated }, t0);
        let rows: Vec<(String, Marker)> = m.named_rows().into_iter().map(|r| (r.name, r.marker)).collect();
        let want = [("axel", Marker::Awaiting), ("bebop", Marker::Saved), ("old-notes", Marker::Saved), ("zed", Marker::Pinned)];
        assert_eq!(rows, want.map(|(n, k)| (n.to_string(), k)).to_vec());
        assert_eq!(m.unnamed_sessions().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["l2"]);
    }

    /// Resume: the row says "resuming…" until the attach; a failure ("no
    /// session named", locked) lands in the Named section, not as a
    /// transcript error; a success asks for the saved list again.
    #[test]
    fn resuming_a_saved_session() {
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        m.apply_at(connected(&["saved_sessions"]), t0);
        m.apply_at(saved_list(json!([{"id": "j-bebop", "name": "bebop", "live": false}])), t0);
        m.resuming("bebop", "j-bebop", 7);
        assert_eq!(m.named_rows()[0].marker, Marker::Resuming);
        let fx = m.apply_at(Event::Attached { attach_seq: 7, attached: attached("live-bebop"), mode: AttachMode::Mirror, reconnect: false }, t0);
        assert!(m.named.resuming.is_none());
        assert!(list_saved(&fx), "the resumed session left the saved list");
        assert_eq!(m.active.as_ref().unwrap().session_id, "live-bebop");

        m.resuming("ghost", "ghost", 8);
        m.apply_at(Event::Detached { attach_seq: 8, session_id: None, reason: "create session: no session named 'ghost'".into() }, t0);
        assert_eq!(m.named.error.as_deref(), Some("No session named 'ghost'."));
        assert!(m.named.resuming.is_none() && m.active.is_none() && m.last_error.is_none());

        m.resuming("axel", "axel", 9);
        let locked = "create session: session 20261110-202033-e5f6 is live in another process (pid 7, tui) — use `synaps --attach`";
        m.apply_at(Event::Detached { attach_seq: 9, session_id: None, reason: locked.into() }, t0);
        assert!(m.named.error.as_deref().unwrap().contains("locked by another process"));
        // A plain detach is still a transcript error.
        attach(&mut m, 10);
        m.apply_at(Event::Detached { attach_seq: 10, session_id: None, reason: "session ended".into() }, t0);
        assert_eq!(m.last_error.as_deref(), Some("detached: session ended"));
    }

    /// "Open by name…" checks the name before anything is sent.
    #[test]
    fn open_by_name_validates() {
        let mut m = Model::new(ConnState::Connecting);
        assert_eq!(m.open_by_name("  axel "), Ok("axel".into()));
        assert_eq!(m.open_by_name("20261110-202033-e5f6"), Ok("20261110-202033-e5f6".into()));
        assert!(m.open_by_name("Axel Memory").unwrap_err().contains("lowercase"));
        assert!(m.named.error.is_some());
        assert_eq!(m.open_by_name("").unwrap_err(), "name cannot be empty");
    }

    /// "Name this session…": validated first, every refusal a sentence, a
    /// `Named` refusal offers to replace the current name, and `Renamed`
    /// moves the session into the Named section at once.
    #[test]
    fn naming_the_current_session() {
        use synaps_client::RenameFailure;
        let t0 = Instant::now();
        let mut m = Model::new(ConnState::Connecting);
        assert_eq!(m.rename_request("axon").unwrap_err(), "Not attached to a session.");
        attach(&mut m, 1);
        m.apply_at(Event::Sessions(metas(json!([{"id": "s", "lifecycle": "live", "name": "kite"}]))), t0);
        assert!(m.rename_request("Bad Name").unwrap_err().contains("lowercase"));
        assert!(m.named.rename.is_none(), "nothing pending after a local refusal");
        assert_eq!(m.rename_request(" axon "), Ok(("s".into(), "axon".into())));

        let mut f = RenameFailure::new("s", RenameRefusal::Named, "named");
        f.current_name = Some("kite".into());
        m.apply_at(Event::RenameFailed(f), t0);
        assert_eq!(m.named.replace, Some(("axon".into(), "kite".into())));
        assert_eq!(m.named.rename_error.as_deref(), Some("This session is already named 'kite'."));
        // A reply for another session is not ours.
        m.rename_request("axon").unwrap();
        m.apply_at(Event::RenameFailed(RenameFailure::new("other", RenameRefusal::Busy, "")), t0);
        assert!(m.named.rename.is_some());

        let fx = m.apply_at(Event::Renamed { session_id: "s".into(), name: Some("axon".into()), in_memory_only: false }, t0);
        assert!(fx.contains(&Effect::ListSessions));
        assert!(m.named.renamed && m.named.rename.is_none() && m.named.replace.is_none());
        assert_eq!(m.active_name(), Some("axon"));
        assert_eq!(m.named_rows()[0].name, "axon");
        assert_eq!(m.unnamed_sessions().count(), 0);

        m.rename_request("taken").unwrap();
        let mut f = RenameFailure::new("s", RenameRefusal::Taken, "taken");
        f.taken_by = Some("20261028-045932-a1b2".into());
        m.apply_at(Event::RenameFailed(f), t0);
        assert!(m.named.rename_error.as_deref().unwrap().contains("already used"));
        assert!(m.named.replace.is_none());
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