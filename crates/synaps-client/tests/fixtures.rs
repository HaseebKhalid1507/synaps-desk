//! Decoding against frames captured from a real 0.11.1 daemon (sandbox
//! profile; thinking signatures and home paths redacted).

use synaps_client::protocol::*;
use synaps_client::transcript::{ItemKind, ToolStatus, Transcript};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn welcome_and_lists_decode() {
    let DaemonFrame::Welcome(w) = decode_frame(fixture("welcome.json").trim()).unwrap() else { panic!() };
    assert_eq!(w.protocol_version, PROTOCOL_VERSION);
    assert_eq!(w.profile.as_deref(), Some("desktopproto"));
    assert_eq!(w.sessions.len(), 2);
    assert!(w.sessions[0].title.is_some());
    let DaemonFrame::SessionList(s) = decode_frame(fixture("session_list.json").trim()).unwrap() else { panic!() };
    assert!(s.is_empty() || !s[0].id.is_empty());
    assert!(matches!(decode_frame(fixture("pong.json").trim()).unwrap(), DaemonFrame::Pong { .. }));
    assert!(matches!(decode_frame(fixture("bye.json").trim()).unwrap(), DaemonFrame::Bye(None)));
}

#[test]
fn attached_with_history_renders() {
    let DaemonFrame::Attached(a) = decode_frame(fixture("attached_history.json").trim()).unwrap() else { panic!() };
    assert_eq!(a.conversation.api_messages.len(), 4);
    let mut t = Transcript::new();
    t.load_attached(&a);
    let kinds: Vec<_> = t.items.iter().map(|i| i.kind).collect();
    assert_eq!(kinds[0], ItemKind::User);
    assert!(kinds.contains(&ItemKind::Thinking));
    assert!(kinds.contains(&ItemKind::Assistant));
    let tool = t.items.iter().find(|i| i.kind == ItemKind::Tool).unwrap().tool.as_ref().unwrap();
    assert_eq!(tool.name, "bash");
    assert_eq!(tool.status, Some(ToolStatus::Done));
    assert_eq!(tool.result, "hi\n");
    assert!(t.items.iter().any(|i| i.text.contains("```python")));
}

#[test]
fn a_real_turn_streams_into_the_same_transcript_as_its_history() {
    let DaemonFrame::Attached(a) = decode_frame(fixture("attached_create.json").trim()).unwrap() else { panic!() };
    let mut t = Transcript::new();
    t.load_attached(&a);
    assert!(t.is_empty());
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for line in fixture("turn_events.ndjson").lines() {
        let DaemonFrame::Event(env) = decode_frame(line).unwrap() else { panic!("{line}") };
        let ev = env.decode();
        assert!(!matches!(ev, SessionEvent::Unknown), "unknown event: {line}");
        *kinds.entry(format!("{ev:?}").split([' ', '{', '(']).next().unwrap().to_string()).or_default() += 1;
        t.apply(&ev);
    }
    assert!(kinds["Stream"] > 50, "{kinds:?}");
    assert!(!t.busy);
    // Live rows were replaced by the final MessageHistory: same shape as a re-attach.
    let DaemonFrame::Attached(h) = decode_frame(fixture("attached_history.json").trim()).unwrap() else { panic!() };
    let mut fresh = Transcript::new();
    fresh.load_attached(&h);
    let a: Vec<_> = t.items.iter().map(|i| (i.kind, i.text.clone())).collect();
    let b: Vec<_> = fresh.items.iter().map(|i| (i.kind, i.text.clone())).collect();
    assert_eq!(a, b);
    let splice = t.take_splice().unwrap();
    assert_eq!(splice.start, 0);
    assert_eq!(splice.new_len, t.len());
    assert!(t.take_splice().is_none());
}

#[test]
fn unknown_frames_events_and_fields_are_tolerated() {
    assert!(matches!(decode_frame(r#"{"type":"from_the_future","x":1}"#).unwrap(), DaemonFrame::Unknown(k) if k == "from_the_future"));
    assert!(matches!(decode_frame(r#"{"type":"welcome","protocol_version":"three"}"#).unwrap(), DaemonFrame::Malformed { .. }));
    let f = decode_frame(r#"{"type":"event","session_id":"s","seq":1,"ts":"t","event":{"ev":"brand_new","payload":[1]},"extra":true}"#).unwrap();
    let DaemonFrame::Event(env) = f else { panic!() };
    assert!(matches!(env.decode(), SessionEvent::Unknown));
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"stream","event":{"kind":"llm","llm":"hologram","x":1}}}"#).unwrap();
    assert!(matches!(env.decode(), SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Unknown) }));
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"stream","event":{"kind":"telepathy"}}}"#).unwrap();
    assert!(matches!(env.decode(), SessionEvent::Stream { event: StreamEvent::Unknown }));
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"idle","new_field":{"a":1}}}"#).unwrap();
    assert!(matches!(env.decode(), SessionEvent::Idle));
    let bye = decode_frame(r#"{"type":"bye","reason":{"bye":"reloading","generation":2,"retry_after_ms":500}}"#).unwrap();
    assert!(matches!(bye, DaemonFrame::Bye(Some(ByeReason::Reloading { generation: 2, retry_after_ms: 500 }))));
    let bye = decode_frame(r#"{"type":"bye","reason":{"bye":"sabbatical"}}"#).unwrap();
    assert!(matches!(bye, DaemonFrame::Bye(Some(ByeReason::Unknown))));
    assert!(decode_frame("not json").is_err());
}

#[test]
fn client_frames_encode_as_the_daemon_expects() {
    let id = synaps_client::Identity::desktop();
    let hello = encode_frame(&ClientFrame::Hello(id.hello(None))).unwrap();
    let v: serde_json::Value = serde_json::from_str(&hello).unwrap();
    assert_eq!(v["type"], "hello");
    assert_eq!(v["protocol_version"], 3);
    assert_eq!(v["client"]["kind"], "attach");
    assert_eq!(v["client"]["name"], "desktop");
    assert_eq!(v["client"]["history"], "full");
    assert!(v.get("env").is_none());
    let submit = encode_frame(&ClientFrame::Cmd {
        session_id: "s".into(),
        cmd: SessionCommand::Submit { text: "hi".into(), attachments: vec![] },
    })
    .unwrap();
    assert_eq!(submit, "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"submit\",\"text\":\"hi\",\"attachments\":[]}}\n");
    let cancel = encode_frame(&ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Cancel }).unwrap();
    assert_eq!(cancel, "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"cancel\"}}\n");
    let attach = encode_frame(&ClientFrame::Attach(Attach::Existing { session_id: "s".into(), mode: AttachMode::Mirror })).unwrap();
    assert_eq!(attach, "{\"type\":\"attach\",\"attach\":\"existing\",\"session_id\":\"s\",\"mode\":\"mirror\"}\n");
}
