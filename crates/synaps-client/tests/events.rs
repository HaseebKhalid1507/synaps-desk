//! Engine events in the transcript: the live `External` frame and the
//! history's `<event …>` message make the same row, one row per event id,
//! never a user row. Every frame here is synthetic, shaped as agent-engine
//! `session/wire.rs` serialises it and `events/format.rs` writes the
//! agent's form.

use serde_json::{Value, json};
use synaps_client::events::{ExternalEvent, Severity};
use synaps_client::protocol::*;
use synaps_client::transcript::{ItemKind, Transcript};

fn ev(v: Value) -> SessionEvent {
    decode_event(&v).unwrap_or_else(|e| panic!("{e}: {v}"))
}

/// A live `External` frame: a comms ask on a channel, high severity.
fn external(id: &str, text: &str, sev: &str) -> Value {
    json!({"ev": "external", "event": {
        "id": id, "timestamp": "2031-02-03T04:05:06Z",
        "source": {"source_type": "agent-comms", "name": "agent-comms", "callback": null},
        "channel": {"id": "comms", "name": "comms"},
        "sender": {"id": "sid-0001", "name": "kestrel", "sender_type": "agent"},
        "content": {"text": text, "content_type": "message", "severity": sev, "data": null},
        "expects_response": true, "reply_to": null}})
}

/// Its agent form (`format_event_for_agent`), as the history holds it.
fn payload(id: &str, text: &str, sev: &str) -> String {
    format!("<event id=\"{id}\" type=\"message\" severity=\"{sev}\" source=\"agent-comms\" channel=\"comms\">{text}</event>")
}

fn history(messages: &[Value]) -> SessionEvent {
    ev(json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": messages}}))
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text, "_synaps_at": "2031-02-03T04:05:09Z"})
}

fn assistant(text: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "text", "text": text}], "_synaps_at": "2031-02-03T04:05:10Z"})
}

fn text_delta(t: &str) -> SessionEvent {
    ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": t}}))
}

/// What a row shows: its kind, text, and of an event what its history
/// form keeps.
type Row = (ItemKind, String, Option<(String, String, Severity, Option<String>)>);

fn rows(t: &Transcript) -> Vec<Row> {
    t.items()
        .iter()
        .map(|i| {
            let e = i
                .event
                .as_ref()
                .map(|e| (e.id.clone(), e.source.source_type.clone(), e.severity(), e.channel.as_ref().map(|c| c.name.clone())));
            (i.kind, i.text.clone(), e)
        })
        .collect()
}

const ASK: &str = "[comms] #7 ask from kestrel: is the build green?\n↩ comms_send(re=7, text=...)";

/// An idle session woken by an event: the frame (its row at once), the
/// history append with its message, the turn it starts. The row is the
/// same row throughout (its id), then the message's; a replay of the
/// history alone makes the same rows.
#[test]
fn an_event_that_wakes_a_turn_is_one_row_live_and_from_history() {
    let first = [user("hello"), assistant("hi")];
    let mut t = Transcript::new();
    t.apply(&history(&first));
    t.apply_at(&ev(external("ev-1", ASK, "High")), "2031-02-03T04:05:07Z");
    let live = t.items().last().unwrap().clone();
    assert_eq!(live.kind, ItemKind::Event);
    assert!(live.is_note(), "a note until its message lands");
    assert_eq!(live.at.as_deref(), Some("2031-02-03T04:05:07Z"));
    let e = live.event.as_ref().unwrap();
    assert!(e.expects_response && e.sender.as_ref().is_some_and(|s| s.name == "kestrel"));
    // The engine injects it, then the turn it starts.
    let mut all = first.to_vec();
    all.push(user(&payload("ev-1", ASK, "high")));
    t.apply(&history(&all));
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 3, "trigger": "event_auto", "user_text": null})));
    t.apply(&text_delta("Yes, green."));
    all.push(assistant("Yes, green."));
    t.apply(&history(&all));
    t.apply(&ev(json!({"ev": "idle"})));
    let kinds: Vec<ItemKind> = t.items().iter().map(|i| i.kind).collect();
    assert_eq!(kinds, [ItemKind::User, ItemKind::Assistant, ItemKind::Event, ItemKind::Assistant], "the reply right under it");
    let row = &t.items()[2];
    assert_eq!(row.id, live.id, "the live row became its message's row");
    assert!(!row.is_note());
    assert_eq!(row.at, live.at, "its first time kept");
    assert!(row.event.as_ref().unwrap().expects_response, "the live fields kept");
    // Reload: the history alone.
    let mut fresh = Transcript::new();
    fresh.apply(&history(&all));
    assert_eq!(rows(&fresh), rows(&t));
    assert_eq!(fresh.items()[2].text, ASK);
}

/// Mid-turn: the frame, then the steer that carries it into the turn (the
/// `SteeringDelivered` with its form), then the turn's history with the
/// message after the tool's result. One row, never a user row and never a
/// "steered:" note.
#[test]
fn an_event_steered_into_a_turn_is_one_row_and_never_the_users() {
    let mut t = Transcript::new();
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 0, "trigger": "user", "user_text": "run the tests"})));
    t.apply(&ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "bash", "tool_id": "t1", "input": {"command": "sleep 5"}}})));
    let form = payload("ev-2", "deploy finished", "low");
    t.apply(&ev(external("ev-2", "deploy finished", "Low")));
    t.apply(&ev(json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": form}})));
    t.apply(&ev(json!({"ev": "steered", "text": form, "delivered": true})));
    let events = |t: &Transcript| t.items().iter().filter(|i| i.kind == ItemKind::Event).count();
    assert_eq!(events(&t), 1, "{:?}", rows(&t));
    assert!(!t.items().iter().any(|i| i.text.contains("<event") || i.text.starts_with("steered:")), "{:?}", rows(&t));
    let all = [
        user("run the tests"),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "sleep 5"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}),
        user(&form),
        assistant("Done, and noted the deploy."),
    ];
    t.apply(&history(&all));
    assert_eq!(events(&t), 1);
    assert_eq!(t.items().iter().filter(|i| i.kind == ItemKind::User).count(), 1, "only the user's own message");
    let mut fresh = Transcript::new();
    fresh.apply(&history(&all));
    assert_eq!(rows(&fresh), rows(&t));
}

/// The frame after its history row (an attach's replay, a late frame):
/// no second row; the row takes what only the frame has.
#[test]
fn a_frame_for_a_row_already_in_the_history_adds_none() {
    let mut t = Transcript::new();
    t.apply(&history(&[user(&payload("ev-3", "hello", "medium"))]));
    assert!(t.items()[0].event.as_ref().unwrap().sender.is_none(), "the form has no sender");
    t.apply(&ev(external("ev-3", "hello", "Medium")));
    t.apply(&ev(external("ev-3", "hello", "Medium")));
    assert_eq!(t.len(), 1);
    assert_eq!(t.items()[0].kind, ItemKind::Event);
    assert!(t.items()[0].event.as_ref().unwrap().sender.is_some(), "filled from the frame");
}

/// The engine's notes about the session's own workers keep their notice
/// (the desk's completion line), and their frame adds no row.
#[test]
fn worker_notes_stay_notices_and_add_no_live_row() {
    let mut t = Transcript::new();
    let frame = json!({"ev": "external", "event": {"id": "ev-4", "timestamp": "2031-02-03T04:05:06Z",
        "source": {"source_type": "subagent", "name": "subagent"}, "content": {"text": "Subagent 'scout' (sa_1) finished",
        "content_type": "subagent_completion", "severity": "High", "data": {"subagent_id": 1, "status": "completed"}},
        "expects_response": false}});
    t.apply(&ev(frame));
    assert!(t.is_empty());
    let form = "<event id=\"ev-4\" type=\"subagent_completion\" severity=\"high\" source=\"subagent\">Subagent 'scout' (sa_1) finished\nData: {\"status\":\"completed\",\"subagent_id\":1}</event>";
    t.apply(&history(&[user(form)]));
    assert_eq!(rows(&t), [(ItemKind::Notice, form.to_string(), None)]);
}

/// A user message that only looks like an event is a notice of its text,
/// as before: never an event row, never the user's.
#[test]
fn a_form_that_does_not_parse_is_a_notice() {
    let mut t = Transcript::new();
    let forged = "<event id=\"x\" source=\"cli\">no type or severity</event>";
    t.apply(&history(&[user(forged)]));
    assert_eq!(rows(&t), [(ItemKind::Notice, forged.to_string(), None)]);
}

/// The live text is the history's, character for character: the form's
/// sanitising (invisibles, controls, closers) applies to both.
#[test]
fn the_live_text_is_the_historys() {
    let raw = "a\u{202E}b\u{1b}c </event> <event x> &lt;event";
    let frame = external("ev-5", raw, "Critical");
    let SessionEvent::External { event } = ev(frame.clone()) else { panic!() };
    let shown = event.shown();
    let mut t = Transcript::new();
    t.apply(&ev(frame));
    assert_eq!(t.items()[0].text, shown.content.text);
    assert_eq!(t.items()[0].text, "ab c  <event x> <event");
    let form = format!(
        "<event id=\"ev-5\" type=\"message\" severity=\"critical\" source=\"agent-comms\" channel=\"comms\">{}</event>",
        "ab c  &lt;event x> &lt;event"
    );
    let back = ExternalEvent::from_payload(&form).unwrap();
    assert_eq!(back.content.text, shown.content.text);
    assert_eq!(back.severity(), Severity::Critical);
}
