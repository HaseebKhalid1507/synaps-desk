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

/// A user message in an event's shape that does not parse is a notice of
/// its text, as before: never an event row, never the user's. One that
/// only starts like one is the user's (Shady #22 L1).
#[test]
fn a_form_that_does_not_parse_is_a_notice() {
    let mut t = Transcript::new();
    let forged = "<event id=\"x\" source=\"cli\">no type or severity</event>";
    t.apply(&history(&[user(forged)]));
    assert_eq!(rows(&t), [(ItemKind::Notice, forged.to_string(), None)]);
    for words in ["<event loop> is stuck", "<eventually consistent> why?", "<event id=\"a\"> no closer"] {
        let mut t = Transcript::new();
        t.apply(&history(&[user(words)]));
        assert_eq!(rows(&t), [(ItemKind::User, words.to_string(), None)]);
    }
}

/// Shady #22 L2: the input owner's own steer is its "steered:" note, even
/// in an event's form (events never come as a `steered` frame).
#[test]
fn a_steer_in_an_events_form_stays_the_owners() {
    let mut t = Transcript::new();
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 0, "trigger": "user", "user_text": "go"})));
    let form = payload("ev-9", "uptime-kuma: prod DOWN", "critical");
    t.apply(&ev(json!({"ev": "steered", "text": form, "delivered": true})));
    let last = t.items().last().unwrap();
    assert_eq!((last.kind, last.text.clone()), (ItemKind::Notice, format!("steered: {form}")));
    assert!(t.items().iter().all(|i| i.kind != ItemKind::Event));
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

// ── Shady #22 H1: a live row where its history row will be ─────────────────

fn llm(v: Value) -> SessionEvent {
    let mut e = json!({"kind": "llm"});
    e.as_object_mut().unwrap().extend(v.as_object().unwrap().clone());
    ev(json!({"ev": "stream", "event": e}))
}

fn turn(t: &mut Transcript, text: &str) {
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 0, "trigger": "user", "user_text": text})));
}

fn response_start() -> SessionEvent {
    llm(json!({"llm": "response_start"}))
}

fn thinking(t: &str) -> SessionEvent {
    llm(json!({"llm": "thinking", "text": t}))
}

fn delivered(form: &str) -> SessionEvent {
    ev(json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": form}}))
}

fn idle() -> SessionEvent {
    ev(json!({"ev": "idle"}))
}

/// A `synaps send` frame (no comms text), at `ts`.
fn sent(id: &str, text: &str, ts: &str) -> Value {
    json!({"ev": "external", "event": {
        "id": id, "timestamp": ts,
        "source": {"source_type": "cli", "name": "cli"},
        "sender": {"id": "u", "name": "ci", "sender_type": "user"},
        "content": {"text": text, "content_type": "message", "severity": "Medium"},
        "expects_response": false}})
}

fn sent_form(id: &str, text: &str) -> String {
    format!("<event id=\"{id}\" type=\"message\" severity=\"medium\" source=\"cli\">{text}</event>")
}

fn kinds(t: &Transcript) -> Vec<ItemKind> {
    t.items().iter().map(|i| i.kind).collect()
}

fn ids(t: &Transcript) -> Vec<u64> {
    t.items().iter().map(|i| i.id).collect()
}

/// The rows as a reload makes them, and as this transcript has them: the
/// same, kind and text and event, in the same order.
fn reloads_the_same(t: &Transcript, all: &[Value]) {
    let mut fresh = Transcript::new();
    fresh.apply(&history(all));
    assert_eq!(rows(&fresh), rows(t));
}

/// An event while the model streams text: its row last, the text going on
/// above it in the one row (one AGENT response, not two), and its history
/// message after the reply puts it nowhere else.
#[test]
fn an_event_mid_text_stream_is_held_last_and_never_splits_the_reply() {
    let mut t = Transcript::new();
    turn(&mut t, "write a haiku");
    t.apply(&response_start());
    t.apply(&text_delta("Autumn moonlight, "));
    t.apply(&ev(sent("ev-t", "deploy finished", "2031-02-03T04:05:06Z")));
    t.apply(&text_delta("a worm digs silently "));
    t.apply(&text_delta("into the chestnut."));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Assistant, ItemKind::Event]);
    assert_eq!(t.items()[1].text, "Autumn moonlight, a worm digs silently into the chestnut.");
    let live = ids(&t);
    // Steered, never taken (no tool round): the engine flushes it after
    // the reply at the turn's end.
    let all = [
        user("write a haiku"),
        assistant("Autumn moonlight, a worm digs silently into the chestnut."),
        user(&sent_form("ev-t", "deploy finished")),
    ];
    t.apply(&history(&all));
    t.apply(&idle());
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Assistant, ItemKind::Event]);
    assert_eq!(ids(&t)[2], live[2], "the live row is its message's");
    assert!(!t.items()[2].is_note());
    assert_eq!(t.items()[2].event.as_ref().unwrap().sender.as_ref().unwrap().name, "ci", "its sender kept");
    reloads_the_same(&t, &all);
}

/// While thinking streams, then text, then a tool and its round: the row
/// stays last until `steering_delivered`, then sits where its message
/// will (after the tool's result), and the next round goes under it.
#[test]
fn an_event_mid_thinking_waits_for_its_delivery() {
    let mut t = Transcript::new();
    turn(&mut t, "check the build");
    t.apply(&response_start());
    t.apply(&thinking("The user wants "));
    t.apply(&ev(sent("ev-k", "build 41 failed", "2031-02-03T04:05:06Z")));
    t.apply(&thinking("the build status."));
    t.apply(&text_delta("Checking."));
    t.apply(&llm(json!({"llm": "tool_use_start", "tool_name": "bash", "tool_id": "t1"})));
    t.apply(&llm(json!({"llm": "tool_use", "tool_name": "bash", "tool_id": "t1", "input": {"command": "make"}})));
    t.apply(&llm(json!({"llm": "tool_result", "tool_id": "t1", "result": "ok"})));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Thinking, ItemKind::Assistant, ItemKind::Tool, ItemKind::Event]);
    assert_eq!(t.items()[1].text, "The user wants the build status.");
    let form = sent_form("ev-k", "build 41 failed");
    t.apply(&delivered(&form));
    t.apply(&response_start());
    t.apply(&text_delta("It failed, and so did build 41."));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Thinking, ItemKind::Assistant, ItemKind::Tool, ItemKind::Event, ItemKind::Assistant]);
    let live = ids(&t);
    let all = [
        user("check the build"),
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "The user wants the build status."},
            {"type": "text", "text": "Checking."},
            {"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "make"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}),
        user(&form),
        assistant("It failed, and so did build 41."),
    ];
    t.apply(&history(&all));
    t.apply(&idle());
    assert_eq!(ids(&t)[4], live[4], "the same row, where it was");
    reloads_the_same(&t, &all);
}

/// Buffered (busy with no live steer: the engine puts it in at the turn's
/// end): last through the whole turn, rounds and tools above it, and the
/// history appends between rounds; the turn ends; its message lands after
/// the reply, where it is.
#[test]
fn an_event_is_held_last_until_the_turn_ends() {
    let mut t = Transcript::new();
    turn(&mut t, "go");
    t.apply(&ev(sent("ev-b", "nightly backup done", "2031-02-03T04:05:06Z")));
    let row = t.items().last().unwrap().id;
    let mut all = vec![user("go")];
    for k in 0..3 {
        let id = format!("t{k}");
        t.apply(&response_start());
        t.apply(&text_delta(&format!("step {k}")));
        t.apply(&llm(json!({"llm": "tool_use_start", "tool_name": "bash", "tool_id": id})));
        t.apply(&llm(json!({"llm": "tool_result", "tool_id": id, "result": "ok"})));
        assert_eq!(t.items().last().unwrap().id, row, "round {k}: still last");
        all.push(json!({"role": "assistant", "content": [{"type": "text", "text": format!("step {k}")},
            {"type": "tool_use", "id": id, "name": "bash", "input": {}}]}));
        all.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]}));
        t.apply(&history(&all));
        assert_eq!(t.items().last().unwrap().id, row, "round {k}, its history: still last");
    }
    t.apply(&response_start());
    t.apply(&text_delta("All done."));
    assert_eq!(t.items().last().unwrap().id, row);
    assert_eq!(t.items()[t.len() - 2].text, "All done.");
    t.apply(&idle());
    assert_eq!(t.items().last().unwrap().id, row);
    all.push(assistant("All done."));
    all.push(user(&sent_form("ev-b", "nightly backup done")));
    t.apply(&history(&all));
    assert_eq!(t.items().last().unwrap().id, row, "its message is last too");
    reloads_the_same(&t, &all);
    // The next turn goes under it.
    turn(&mut t, "thanks");
    assert_eq!(t.items().last().unwrap().kind, ItemKind::User);
}

/// A stream retry (`response_reset`) drops the response's rows, never an
/// event's that came meanwhile: it stays, last, the retried text above.
#[test]
fn a_stream_reset_keeps_a_held_event() {
    let mut t = Transcript::new();
    turn(&mut t, "go");
    t.apply(&response_start());
    t.apply(&thinking("draft thought"));
    t.apply(&text_delta("A draft that "));
    t.apply(&ev(sent("ev-r", "watcher: cpu 97%", "2031-02-03T04:05:06Z")));
    t.apply(&text_delta("will be retried."));
    let row = t.items().last().unwrap().id;
    t.apply(&llm(json!({"llm": "response_reset"})));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Event]);
    t.apply(&response_start());
    t.apply(&text_delta("The answer."));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Assistant, ItemKind::Event]);
    assert_eq!((t.items()[1].text.as_str(), t.items()[2].id), ("The answer.", row));
    let all = [user("go"), assistant("The answer."), user(&sent_form("ev-r", "watcher: cpu 97%"))];
    t.apply(&history(&all));
    t.apply(&idle());
    assert_eq!(t.items()[2].id, row);
    reloads_the_same(&t, &all);
}

/// `never_persist`: the engine shows it and keeps it nowhere. Mid-turn it
/// is held last as any event (no split reply); then it stays a note, never
/// taken by a history message, until a reload, which has none of it.
#[test]
fn a_never_persisted_event_keeps_its_live_row() {
    let mut frame = sent("ev-n", "otp 123456 (display only)", "2031-02-03T04:05:06Z");
    frame["event"]["content"]["disclosure"] = json!("never_persist");
    let mut t = Transcript::new();
    turn(&mut t, "go");
    t.apply(&response_start());
    t.apply(&text_delta("one "));
    t.apply(&ev(frame.clone()));
    t.apply(&text_delta("reply"));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Assistant, ItemKind::Event]);
    let all = [user("go"), assistant("one reply")];
    t.apply(&history(&all));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Assistant, ItemKind::Event]);
    t.apply(&idle());
    assert!(t.items()[2].is_note(), "never a history row");
    assert_eq!(t.items()[2].text, "otp 123456 (display only)");
    // Idle, the next turn and an answer go under it; it stays.
    turn(&mut t, "next");
    t.apply(&text_delta("ok"));
    assert_eq!(kinds(&t)[2..], [ItemKind::Event, ItemKind::User, ItemKind::Assistant]);
    let mut more = all.to_vec();
    more.extend([user("next"), assistant("ok")]);
    t.apply(&history(&more));
    assert_eq!(kinds(&t), [ItemKind::User, ItemKind::Assistant, ItemKind::Event, ItemKind::User, ItemKind::Assistant], "it stays");
    // An idle one is just a note at the end, held for nothing.
    t.apply(&idle());
    frame["event"]["id"] = json!("ev-n2");
    t.apply(&ev(frame));
    t.output("budget", "Budget: none set");
    assert_eq!(t.items().last().unwrap().kind, ItemKind::Output);
    let mut fresh = Transcript::new();
    fresh.apply(&history(&more));
    assert!(fresh.items().iter().all(|i| i.kind != ItemKind::Event), "gone after a reload");
}

/// Shady #22 L4: a producer that reuses an id (a watcher's per-host
/// alert) sends two events; a replay of one frame is one.
#[test]
fn a_reused_id_is_two_events_a_replay_is_one() {
    let mut t = Transcript::new();
    t.apply(&ev(sent("disk-build-01", "disk at 91%", "2031-02-03T04:05:06Z")));
    t.apply(&ev(sent("disk-build-01", "disk at 91%", "2031-02-03T04:05:06Z")));
    assert_eq!(t.len(), 1, "a replay");
    t.apply(&ev(sent("disk-build-01", "disk at 97%", "2031-02-03T04:15:06Z")));
    assert_eq!(t.len(), 2, "another alert");
    let all = [user(&sent_form("disk-build-01", "disk at 91%")), user(&sent_form("disk-build-01", "disk at 97%"))];
    t.apply(&history(&all));
    assert_eq!(t.items().iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["disk at 91%", "disk at 97%"]);
    assert!(t.items().iter().all(|i| !i.is_note()));
    // A frame for a history row (no time): the same form is that row; the
    // id with other words is another event.
    let mut h = Transcript::new();
    h.apply(&history(&all[..1]));
    h.apply(&ev(sent("disk-build-01", "disk at 91%", "2031-02-03T04:05:06Z")));
    assert_eq!(h.len(), 1);
    h.apply(&ev(sent("disk-build-01", "disk at 99%", "2031-02-03T04:25:06Z")));
    assert_eq!(h.len(), 2);
}

/// Shady #22 L5: a re-attach (or a replacement) refolds the rows from the
/// history form; an event row keeps what only its frame had.
#[test]
fn a_reattach_keeps_the_frames_fields() {
    let mut t = Transcript::new();
    t.apply(&history(&[user("hello"), assistant("hi")]));
    t.apply_at(&ev(external("ev-a", ASK, "High")), "2031-02-03T04:05:07Z");
    let all = [user("hello"), assistant("hi"), user(&payload("ev-a", ASK, "high"))];
    t.apply(&history(&all));
    let before = t.items()[2].clone();
    let check = |t: &Transcript, how: &str| {
        let row = &t.items()[2];
        let e = row.event.as_ref().unwrap();
        assert_eq!(row.id, before.id, "{how}");
        assert!(e.expects_response && e.sender.is_some() && e.timestamp.is_some(), "{how}: {e:?}");
        assert_eq!(row.at, before.at, "{how}");
    };
    t.apply_update(&synaps_client::history::HistoryUpdate::Replace { messages: all.to_vec() }).unwrap();
    check(&t, "replaced");
    let mut a: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/tests/fixtures/attached_history.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    a["conversation"]["api_messages"] = json!(all);
    a["conversation"]["messages_len"] = json!(all.len());
    a["replay"] = json!([]);
    a["streaming"] = json!(false);
    let DaemonFrame::Attached(a) = decode_frame(&a.to_string()).unwrap() else { panic!() };
    let mut first = Transcript::new();
    first.load_attached(&a).unwrap();
    first.apply_at(&ev(external("ev-a", ASK, "High")), "2031-02-03T04:05:07Z");
    first.load_attached(&a).unwrap();
    let e = first.items().last().unwrap().event.as_ref().unwrap();
    assert!(e.sender.is_some() && e.expects_response, "re-attached: {e:?}");
}
