//! The window's state and how client events change it. No GPUI: events are
//! applied here as they arrive (in the event pump, not in `render`), so a
//! hidden or minimized window holds the current transcript and nothing
//! else; the view only turns the transcript's splice into rows per frame.

use synaps_client::protocol::SessionMeta;
use synaps_client::{AttachMode, Error, Event, SessionEvent, Status, Transcript};

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
    /// Sent, not yet confirmed by `TurnStarted` / `Steered`: given back to
    /// the user on a refusal or a detach.
    in_flight: Option<String>,
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
            in_flight: None,
        }
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
        self.in_flight = Some(text);
    }

    fn current(&self, attach_seq: u64) -> bool {
        self.wanted == Some(attach_seq)
    }

    fn give_back(&mut self, effects: &mut Vec<Effect>) {
        if let Some(t) = self.in_flight.take() {
            effects.push(Effect::Restore(t));
        }
    }

    pub fn apply(&mut self, ev: Event) -> Vec<Effect> {
        let mut effects = Vec::new();
        match ev {
            Event::Status(s) => {
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
            Event::Sessions(s) => self.sessions = s,
            Event::Attached { attach_seq, attached, mode, .. } if self.current(attach_seq) => {
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
                    SessionEvent::TurnStarted { user_text: Some(t), .. } | SessionEvent::Steered { text: t, .. }
                        if self.in_flight.as_deref() == Some(t.trim()) =>
                    {
                        self.in_flight = None;
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
                self.last_error = Some(format!("detached: {reason}"));
                self.give_back(&mut effects);
            }
            Event::Error { attach_seq, error } if attach_seq.is_none_or(|s| self.current(s)) => {
                let input = matches!(
                    &error,
                    Error::Refused { command: None, .. } | Error::NotAttached { .. }
                ) || matches!(&error, Error::Refused { command: Some(c), .. } if c == "submit" || c == "steer");
                if input {
                    self.give_back(&mut effects);
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
    use synaps_client::protocol::{LlmEvent, StreamEvent};
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
}
