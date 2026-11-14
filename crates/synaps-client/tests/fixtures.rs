//! Decoding and reducer tests against frames captured from a real 0.11.1
//! daemon (sandbox profile; thinking signatures and home paths redacted),
//! plus hand-written frames shaped exactly as `wire.rs` serialises them
//! (`session_swaps.ndjson`, `daemon_refusals.ndjson`, `bye_detached.json`).
//!
//! Redacting the signatures changed the history, so the `messages_hash` of
//! the two digests after a thinking block (seq 81, 91, 93) in
//! `turn_events.ndjson` were recomputed. The first digest (seq 5) is the
//! daemon's own and checks hash parity.

use synaps_client::history::{HistoryMirror, HistoryUpdate, messages_hash};
use synaps_client::protocol::*;
use synaps_client::transcript::{ItemKind, ToolStatus, Transcript, TranscriptError};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn attached(name: &str) -> Box<Attached> {
    let DaemonFrame::Attached(a) = decode_frame(fixture(name).trim()).unwrap() else { panic!("{name}") };
    a
}

fn events(name: &str) -> Vec<(Envelope, SessionEvent)> {
    fixture(name)
        .lines()
        .map(|line| {
            let DaemonFrame::Event(env) = decode_frame(line).unwrap() else { panic!("{line}") };
            let ev = env.decode().unwrap_or_else(|e| panic!("{e}: {line}"));
            assert!(!matches!(ev, SessionEvent::Unknown), "unknown event: {line}");
            (env, ev)
        })
        .collect()
}

fn texts(t: &Transcript) -> Vec<(ItemKind, String)> {
    t.items().iter().map(|i| (i.kind, i.text.clone())).collect()
}

fn history_of(ev: &SessionEvent) -> Option<&Vec<serde_json::Value>> {
    match ev {
        SessionEvent::Stream { event: StreamEvent::Session(SessionStreamEvent::MessageHistory { messages }) } => Some(messages),
        _ => None,
    }
}

#[test]
fn welcome_and_lists_decode() {
    let DaemonFrame::Welcome(w) = decode_frame(fixture("welcome.json").trim()).unwrap() else { panic!() };
    assert_eq!(w.protocol_version, PROTOCOL_VERSION);
    assert_eq!(w.profile.as_deref(), Some("desktopproto"));
    assert_eq!(w.sessions.len(), 2);
    assert_eq!(w.rejected_sessions, 0);
    assert!(w.sessions[0].title.is_some());
    let DaemonFrame::SessionList { sessions, rejected } = decode_frame(fixture("session_list.json").trim()).unwrap() else { panic!() };
    assert!(sessions.is_empty() || !sessions[0].id.is_empty());
    assert_eq!(rejected, 0);
    assert!(matches!(decode_frame(fixture("pong.json").trim()).unwrap(), DaemonFrame::Pong { .. }));
    assert!(matches!(decode_frame(fixture("bye.json").trim()).unwrap(), DaemonFrame::Bye(None)));
    assert!(matches!(decode_frame(fixture("bye_detached.json").trim()).unwrap(), DaemonFrame::Bye(Some(ByeReason::Detached))));
}

/// M5: one bad entry no longer costs the whole `Welcome` (and so the
/// connection) or freezes the session list.
#[test]
fn one_bad_session_entry_is_skipped_and_counted() {
    let mut w: serde_json::Value = serde_json::from_str(fixture("welcome.json").trim()).unwrap();
    w["sessions"][1]["clients"] = "two".into();
    let DaemonFrame::Welcome(w) = decode_frame(&w.to_string()).unwrap() else { panic!("welcome must survive a bad entry") };
    assert_eq!((w.sessions.len(), w.rejected_sessions), (1, 1));
    let list = r#"{"type":"session_list","sessions":[{"id":"a"},{"no_id":true},{"id":"b","busy":"yes"}]}"#;
    let DaemonFrame::SessionList { sessions, rejected } = decode_frame(list).unwrap() else { panic!() };
    assert_eq!((sessions.len(), rejected), (1, 2));
}

/// M5: a new variant is `Ok(Unknown)`; a known one that changed shape is a
/// `DecodeError` naming it.
#[test]
fn shape_drift_is_an_error_not_unknown() {
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"brand_new","x":1}}"#).unwrap();
    assert!(matches!(env.decode(), Ok(SessionEvent::Unknown)));
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"prompt","request":{"id":"seven"}}}"#).unwrap();
    let err = env.decode().unwrap_err();
    assert_eq!(err.kind, "event:prompt");
    assert!(env.into_event().is_err());
}

#[test]
fn attached_with_history_renders() {
    let a = attached("attached_history.json");
    assert_eq!(a.conversation.api_messages.len(), 4);
    assert_eq!(a.client, 2);
    assert_eq!(a.client_input, ClientInput::Shared);
    assert_eq!(a.roster.len(), 1);
    assert_eq!(a.roster[0].mode, "observe");
    assert_eq!(a.clients, vec![(2, ClientKind::Attach)]);
    assert_eq!(a.conversation.tokens["cache_creation"], 4748);
    assert_eq!(a.conversation.tokens["input"], 84);
    let mut t = Transcript::new();
    t.load_attached(&a).unwrap();
    let kinds: Vec<_> = t.items().iter().map(|i| i.kind).collect();
    assert_eq!(kinds[0], ItemKind::User);
    assert!(kinds.contains(&ItemKind::Thinking));
    assert!(kinds.contains(&ItemKind::Assistant));
    let tool = t.items().iter().find(|i| i.kind == ItemKind::Tool).unwrap().tool.as_ref().unwrap();
    assert_eq!(tool.name, "bash");
    assert_eq!(tool.status, Some(ToolStatus::Done));
    assert_eq!(tool.result, "hi\n");
    assert!(t.items().iter().any(|i| i.text.contains("```python")));
}

/// API 6: a `Digest` attach is refused, not silently rendered empty.
#[test]
fn a_digest_attach_is_refused() {
    let mut v: serde_json::Value = serde_json::from_str(fixture("attached_history.json").trim()).unwrap();
    v["conversation"]["api_messages"] = serde_json::json!([]);
    let DaemonFrame::Attached(a) = decode_frame(&v.to_string()).unwrap() else { panic!() };
    assert_eq!(Transcript::new().load_attached(&a), Err(TranscriptError::DigestHistory));
}

/// L4: a re-attach to the same session (a reload) keeps every row id.
#[test]
fn reattach_keeps_row_ids() {
    let a = attached("attached_history.json");
    let mut t = Transcript::new();
    t.load_attached(&a).unwrap();
    let ids: Vec<u64> = t.items().iter().map(|i| i.id).collect();
    t.load_attached(&a).unwrap();
    assert_eq!(ids, t.items().iter().map(|i| i.id).collect::<Vec<_>>());
}

#[test]
fn a_real_turn_streams_into_the_same_transcript_as_its_history() {
    let mut t = Transcript::new();
    t.load_attached(&attached("attached_create.json")).unwrap();
    assert!(t.is_empty());
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for (_, ev) in events("turn_events.ndjson") {
        *kinds.entry(format!("{ev:?}").split([' ', '{', '(']).next().unwrap().to_string()).or_default() += 1;
        t.apply(&ev);
    }
    assert!(kinds["Stream"] > 50, "{kinds:?}");
    assert!(!t.busy());
    // Live rows were replaced by the final MessageHistory: same shape as a re-attach.
    let mut fresh = Transcript::new();
    fresh.load_attached(&attached("attached_history.json")).unwrap();
    assert_eq!(texts(&t), texts(&fresh));
    let splice = t.take_splice().unwrap();
    assert_eq!(splice.start, 0);
    assert_eq!(splice.new_len, t.len());
    assert!(t.take_splice().is_none());
}

/// H1/M6: the client's path. Every `MessageHistory` of a real turn is an
/// append carrying only the new messages, every digest matches the mirror
/// (no resync), and the result equals a fresh attach.
#[test]
fn a_real_turn_through_the_mirror_appends_and_digests_match() {
    let a = attached("attached_create.json");
    let mut mirror = HistoryMirror::new(&a.conversation.api_messages);
    let mut t = Transcript::new();
    t.load_attached(&a).unwrap();
    let mut appended = Vec::new();
    for (_, ev) in events("turn_events.ndjson") {
        if let Some(messages) = history_of(&ev) {
            let up = mirror.update(messages.clone());
            let HistoryUpdate::Append { base_len, messages } = &up else { panic!("not append-shaped: {up:?}") };
            appended.push((*base_len, messages.len()));
            t.apply_update(&up).unwrap();
            continue;
        }
        if let SessionEvent::Conversation { digest } = &ev {
            assert!(mirror.matches(digest), "digest {} != mirror", digest.messages_len);
        }
        t.apply(&ev);
    }
    assert_eq!(appended, vec![(0, 1), (1, 2), (3, 1)]);
    let mut fresh = Transcript::new();
    fresh.load_attached(&attached("attached_history.json")).unwrap();
    assert_eq!(texts(&t), texts(&fresh));
}

/// The hash is the daemon's: the first digest of the capture is unredacted.
#[test]
fn messages_hash_matches_the_daemon() {
    let evs = events("turn_events.ndjson");
    let first = evs.iter().find_map(|(_, e)| history_of(e)).unwrap();
    let digest = evs
        .iter()
        .find_map(|(_, e)| match e {
            SessionEvent::Conversation { digest } => Some(digest.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(digest.messages_len, 1);
    assert_eq!(messages_hash(first), digest.messages_hash);
    assert_eq!(digest.messages_hash, 10052227065817030572);
}

/// H1: `/resume`, compaction and `/clear` send no `MessageHistory`, only a
/// digest. The mirror flags each one and the re-queried history replaces
/// the transcript: no old rows, no splice of two conversations.
#[test]
fn resume_compaction_and_clear_replace_the_transcript() {
    let a = attached("attached_history.json");
    let mut mirror = HistoryMirror::new(&a.conversation.api_messages);
    let mut t = Transcript::new();
    t.load_attached(&a).unwrap();
    let old_rows = texts(&t);
    let evs = events("session_swaps.ndjson");
    let mut queries = 0;
    let mut pending = None;
    for (_, ev) in &evs {
        match ev {
            SessionEvent::Conversation { digest } if !mirror.matches(digest) => {
                if digest.messages_len == 0 {
                    let up = HistoryUpdate::Replace { messages: vec![] };
                    mirror.reset(&[]);
                    t.apply_update(&up).unwrap();
                } else {
                    queries += 1;
                    pending = Some(digest.clone());
                }
            }
            SessionEvent::QueryResult { value, .. } => {
                let messages = value.as_array().unwrap().clone();
                let up = mirror.resynced(messages, pending.take().as_ref());
                assert!(matches!(up, HistoryUpdate::Replace { .. }));
                t.apply_update(&up).unwrap();
                assert!(mirror.hash_trusted(), "the hash must agree with the daemon's");
            }
            _ => {}
        }
        t.apply(ev);
        if let SessionEvent::QueryResult { .. } = ev {
            let rows = texts(&t);
            assert!(old_rows.iter().all(|r| !rows.contains(r) || r.0 == ItemKind::Notice), "old conversation still shown: {rows:?}");
        }
        if let SessionEvent::Resumed { .. } = ev {
            assert!(texts(&t).iter().any(|(_, s)| s.contains("resumed")));
        }
    }
    assert_eq!(queries, 2, "resume and compaction need a re-query; clear does not");
    // After /clear: no history rows at all, only this turn's notes.
    assert_eq!(t.history_len(), 0);
    assert!(t.items().iter().all(|i| i.is_note()), "{:?}", texts(&t));
    assert!(texts(&t).iter().any(|(_, s)| s == "conversation cleared"));
}

/// H2: the typed `Refused` event decodes (it used to be `Unknown`) and says
/// whose command it was.
#[test]
fn refusals_decode() {
    let evs = events("session_swaps.ndjson");
    let refused: Vec<_> = evs
        .iter()
        .filter_map(|(_, e)| match e {
            SessionEvent::Refused { client, command, reason } => Some((*client, command.clone(), reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(refused.len(), 2);
    assert_eq!(refused[0].0, 2);
    assert_eq!(refused[0].1, "submit");
    assert!(refused[0].2.contains("exclusive"));
    assert!(evs.iter().any(|(_, e)| matches!(e, SessionEvent::InputOwnerChanged { to: Some(2), reason, .. } if reason == "takeover")));
    let frames: Vec<_> = fixture("daemon_refusals.ndjson").lines().map(|l| decode_frame(l).unwrap()).collect();
    assert!(matches!(&frames[0], DaemonFrame::Error { message, .. } if message.starts_with("refused: ")));
    assert!(matches!(&frames[1], DaemonFrame::Error { message, .. } if message == RELOADING_REFUSAL));
    assert!(matches!(&frames[3], DaemonFrame::Refused { reason, .. } if refuse_kind(reason) == "busy"));
}

/// M3: notes (subagent, notices, errors) survive the per-round history
/// refold, after the round's rows; reused ids skip over other kinds.
#[test]
fn notes_survive_refolds() {
    use serde_json::json;
    let mut t = Transcript::new();
    t.apply_history(&[json!({"role": "user", "content": "go"})]);
    t.apply(&SessionEvent::TurnStarted { turn_baseline: 1, trigger: "user".into(), user_text: None });
    t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: "working".into() }) });
    let streamed_id = t.items().last().unwrap().id;
    t.apply(&SessionEvent::Stream {
        event: StreamEvent::Agent(AgentEvent::SubagentStart { subagent_id: 1, agent_name: "scout".into(), task_preview: "look".into() }),
    });
    t.apply(&SessionEvent::Stream { event: StreamEvent::Session(SessionStreamEvent::Error { message: "rate limited".into() }) });
    let round =
        vec![json!({"role": "user", "content": "go"}), json!({"role": "assistant", "content": [{"type": "text", "text": "working"}]})];
    t.apply_history(&round);
    let rows = texts(&t);
    assert_eq!(rows[1], (ItemKind::Assistant, "working".into()));
    assert!(rows[2].1.contains("subagent scout started"), "{rows:?}");
    assert_eq!(rows[3], (ItemKind::Error, "rate limited".into()));
    // The assistant row kept its id although a note came before it in the
    // rows being replaced.
    assert_eq!(t.items()[1].id, streamed_id);
    // Another round: the notes stay where they were.
    let mut round2 = round.clone();
    round2.push(json!({"role": "user", "content": "more"}));
    t.apply_history(&round2);
    let rows = texts(&t);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[4], (ItemKind::User, "more".into()));
    // A rewrite (same length, replaced) keeps them too, anchored.
    t.apply_update(&HistoryUpdate::Replace { messages: round2.clone() }).unwrap();
    assert_eq!(texts(&t), rows);
}

/// M4: an abort leaves no tool "running" and nothing streaming.
#[test]
fn abort_settles_tools() {
    let mut t = Transcript::new();
    t.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some("run".into()) });
    for id in ["a", "b"] {
        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.into() }) });
    }
    t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ToolQueued { tool_id: "b".into() }) });
    t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: "and".into() }) });
    t.apply(&SessionEvent::Aborted { context_saved: true });
    assert!(!t.busy());
    assert!(t.items().iter().all(|i| !i.streaming));
    let statuses: Vec<_> = t.items().iter().filter_map(|i| i.tool.as_ref()?.status).collect();
    assert_eq!(statuses, vec![ToolStatus::Cancelled, ToolStatus::Cancelled]);
    assert!(texts(&t).iter().any(|(_, s)| s == "turn aborted"));
}

/// An append that does not follow the folded history is refused (resync).
#[test]
fn an_out_of_sync_append_is_refused() {
    let mut t = Transcript::new();
    let up = HistoryUpdate::Append { base_len: 3, messages: vec![serde_json::json!({"role": "user", "content": "x"})] };
    assert_eq!(t.apply_update(&up), Err(TranscriptError::OutOfSync { have: 0, base_len: 3 }));
}

#[test]
fn unknown_frames_events_and_fields_are_tolerated() {
    assert!(matches!(decode_frame(r#"{"type":"from_the_future","x":1}"#).unwrap(), DaemonFrame::Unknown(k) if k == "from_the_future"));
    assert!(matches!(decode_frame(r#"{"type":"welcome","protocol_version":"three"}"#).unwrap(), DaemonFrame::Malformed { .. }));
    let f = decode_frame(r#"{"type":"event","session_id":"s","seq":1,"ts":"t","event":{"ev":"brand_new","payload":[1]},"extra":true}"#)
        .unwrap();
    let DaemonFrame::Event(env) = f else { panic!() };
    assert!(matches!(env.decode(), Ok(SessionEvent::Unknown)));
    let env: Envelope =
        serde_json::from_str(r#"{"session_id":"s","event":{"ev":"stream","event":{"kind":"llm","llm":"hologram","x":1}}}"#).unwrap();
    assert!(matches!(env.decode(), Ok(SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Unknown) })));
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"stream","event":{"kind":"telepathy"}}}"#).unwrap();
    assert!(matches!(env.decode(), Ok(SessionEvent::Stream { event: StreamEvent::Unknown })));
    let env: Envelope = serde_json::from_str(r#"{"session_id":"s","event":{"ev":"idle","new_field":{"a":1}}}"#).unwrap();
    assert!(matches!(env.decode(), Ok(SessionEvent::Idle)));
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
    let submit =
        encode_frame(&ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Submit { text: "hi".into(), attachments: vec![] } })
            .unwrap();
    assert_eq!(submit, "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"submit\",\"text\":\"hi\",\"attachments\":[]}}\n");
    let cancel = encode_frame(&ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Cancel }).unwrap();
    assert_eq!(cancel, "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"cancel\"}}\n");
    let attach = encode_frame(&ClientFrame::Attach(Attach::Existing { session_id: "s".into(), mode: AttachMode::Observe })).unwrap();
    assert_eq!(attach, "{\"type\":\"attach\",\"attach\":\"existing\",\"session_id\":\"s\",\"mode\":\"observe\"}\n");
    let query =
        encode_frame(&ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Query { id: 5, query: SessionQuery::Messages } })
            .unwrap();
    assert_eq!(query, "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"query\",\"id\":5,\"query\":{\"query\":\"messages\"}}}\n");
    let detach = encode_frame(&ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Detach { client: 3 } }).unwrap();
    assert_eq!(detach, "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"detach\",\"client\":3}}\n");
    let create = CreateConfig { cwd: Some("/work".into()), ..Default::default() };
    let create = encode_frame(&ClientFrame::Attach(Attach::Create { config: create, mode: AttachMode::Mirror })).unwrap();
    assert!(create.contains("\"cwd\":\"/work\""), "{create}");
    assert!(!create.contains("\"system\""), "no system prompt: the key is absent, {create}");
}

/// `Set` and `SessionConfig.system` exactly as agent-engine's `types.rs`
/// (`command_json_round_trip`, `SessionSetting`) and `SessionConfig` read them.
#[test]
fn settings_and_the_create_system_prompt_encode_as_the_daemon_expects() {
    let set =
        |setting: Setting| encode_frame(&ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Set { id: 7, setting } }).unwrap();
    assert_eq!(
        set(Setting::reasoning_level("ultra")),
        "{\"type\":\"cmd\",\"session_id\":\"s\",\"cmd\":{\"cmd\":\"set\",\"id\":7,\"setting\":{\"setting\":\"reasoning_level\",\"level\":\"ultra\"}}}\n"
    );
    assert!(set(Setting::model("claude-opus-4-6")).contains("{\"setting\":\"model\",\"model\":\"claude-opus-4-6\"}"));
    assert!(set(Setting::system_prompt("You are Capri.")).contains("{\"setting\":\"system_prompt\",\"text\":\"You are Capri.\"}"));
    assert_eq!(Setting::system_prompt("x").name(), "system_prompt");
    assert_eq!(format!("{:?}", Setting::system_prompt("You are Capri.")), "SystemPrompt { text: <14 bytes> }");
    let create = CreateConfig { system: Some("You are Capri.".into()), ..Default::default() };
    let create = encode_frame(&ClientFrame::Attach(Attach::Create { config: create, mode: AttachMode::Observe })).unwrap();
    assert!(create.contains("\"system\":\"You are Capri.\""), "{create}");
}

/// M9: `Debug` never prints user text or a secret answer.
#[test]
fn debug_redacts_text_and_answers() {
    use synaps_client::Command;
    let secret = "sk-ant-SECRET-VALUE";
    let shown = [
        format!("{:?}", Command::Submit { text: secret.into() }),
        format!("{:?}", Command::Steer { text: secret.into() }),
        format!("{:?}", Command::Answer { prompt_id: 4, value: Some(secret.into()) }),
        format!("{:?}", SessionCommand::Answer { prompt_id: 4, value: Some(secret.into()) }),
        format!("{:?}", SessionCommand::Submit { text: secret.into(), attachments: vec![] }),
        format!("{:?}", SessionCommand::Steer { text: secret.into() }),
        format!(
            "{:?}",
            ClientFrame::Cmd { session_id: "s".into(), cmd: SessionCommand::Answer { prompt_id: 4, value: Some(secret.into()) } }
        ),
    ];
    for s in &shown {
        assert!(!s.contains(secret), "{s}");
        assert!(s.contains("<redacted>"), "{s}");
    }
    assert!(shown[2].contains("prompt_id: 4"));
}

/// M5: a drifted entry in the attach snapshot's lists is skipped and
/// counted per list, not silently dropped.
#[test]
fn attach_snapshot_rejects_are_counted() {
    let mut v: serde_json::Value = serde_json::from_str(fixture("attached_history.json").trim()).unwrap();
    v["pending_prompts"] = serde_json::json!([{"id": "p-1", "title": "drifted id"}, {"id": 7, "title": "Allow bash?"}]);
    v["clients"] = serde_json::json!([[2, "attach"], ["three", "attach"]]);
    let DaemonFrame::Attached(a) = decode_frame(&v.to_string()).unwrap() else { panic!() };
    assert_eq!(a.pending_prompts.len(), 1);
    assert_eq!(a.pending_prompts[0].id, 7);
    assert_eq!((a.rejected.pending_prompts, a.rejected.clients, a.rejected.roster), (1, 1, 0));
    assert_eq!(a.rejected.total(), 2);
    assert!(a.rejected.to_string().contains("1 pending_prompts"));
    assert_eq!(attached("attached_history.json").rejected.total(), 0);
}

/// N4: `/clear` on an empty conversation sends no replacement, so it must
/// not leave a "swapped" mark for the next unrelated one, which would drop
/// this conversation's notes.
#[test]
fn clearing_an_empty_conversation_keeps_later_notes() {
    use serde_json::json;
    let mut t = Transcript::new();
    t.apply(&SessionEvent::Cleared { session_id: "n".into() });
    let msgs = vec![json!({"role": "user", "content": "hi"})];
    t.apply_update(&HistoryUpdate::Append { base_len: 0, messages: msgs.clone() }).unwrap();
    t.apply(&SessionEvent::SystemNotice { text: "a note".into() });
    // The round ends: the note is anchored in the history, not live.
    let reply = json!({"role": "assistant", "content": "hello"});
    t.apply_update(&HistoryUpdate::Append { base_len: 1, messages: vec![reply.clone()] }).unwrap();
    // A resync of the same conversation.
    t.apply_update(&HistoryUpdate::Replace { messages: vec![msgs[0].clone(), reply] }).unwrap();
    assert!(texts(&t).iter().any(|(_, s)| s == "a note"), "{:?}", texts(&t));
}

/// Saved sessions (`saved_session_list`, the `saved_sessions` contract):
/// null `name` / `cwd` / `message_count`, unknown fields, a bad entry
/// skipped and counted, `truncated`.
#[test]
fn saved_session_lists_decode() {
    let DaemonFrame::SavedSessionList { sessions, truncated, rejected } = decode_frame(fixture("saved_session_list.json").trim()).unwrap()
    else {
        panic!()
    };
    assert_eq!((sessions.len(), truncated, rejected), (3, false, 0));
    let axel = &sessions[0];
    assert_eq!((axel.id.as_str(), axel.name.as_deref(), axel.live), ("20261110-202033-e5f6", Some("axel"), true));
    assert_eq!(axel.message_count, Some(800));
    assert_eq!(axel.updated_at.as_deref(), Some("2026-11-13T07:30:00Z"));
    let bebop = &sessions[1];
    assert_eq!((bebop.cwd.as_deref(), bebop.message_count, bebop.title.as_deref()), (None, None, Some("")));

    let odd = r#"{"type":"saved_session_list","sessions":[{"id":"a","name":null,"live":false},{"name":"no-id"},{"id":"b","live":"yes"}],"truncated":true}"#;
    let DaemonFrame::SavedSessionList { sessions, truncated, rejected } = decode_frame(odd).unwrap() else { panic!() };
    assert_eq!((sessions.len(), truncated, rejected), (1, true, 2));
    assert_eq!(sessions[0].name, None);
    let DaemonFrame::SavedSessionList { sessions, .. } = decode_frame(r#"{"type":"saved_session_list","sessions":null}"#).unwrap() else {
        panic!()
    };
    assert!(sessions.is_empty());
    assert!(matches!(decode_frame(r#"{"type":"saved_session_list","truncated":"no"}"#).unwrap(), DaemonFrame::Malformed { .. }));
}

/// `Renamed` / `RenameFailed` (`wire.rs`): every refusal, `taken_by`,
/// `current_name`, and a reason from a newer daemon (`Failed`).
#[test]
fn rename_replies_decode() {
    let frames: Vec<DaemonFrame> = fixture("rename_replies.ndjson").lines().map(|l| decode_frame(l).unwrap()).collect();
    assert!(matches!(&frames[0], DaemonFrame::Renamed { name: Some(n), in_memory_only: false, .. } if n == "axon"));
    assert!(matches!(&frames[1], DaemonFrame::Renamed { name: None, in_memory_only: true, .. }));
    let failures: Vec<&RenameFailure> = frames[2..]
        .iter()
        .map(|f| match f {
            DaemonFrame::RenameFailed(r) => r,
            other => panic!("{other:?}"),
        })
        .collect();
    let reasons: Vec<RenameRefusal> = failures.iter().map(|f| f.reason).collect();
    use RenameRefusal::*;
    assert_eq!(reasons, vec![Taken, Named, Invalid, ChainConflict, Locked, Busy, Unknown, Failed]);
    assert_eq!(failures[0].taken_by.as_deref(), Some("20261028-045932-a1b2"));
    assert_eq!(failures[1].current_name.as_deref(), Some("kite"));
    assert!(matches!(decode_frame(r#"{"type":"rename_failed","session_id":"s"}"#).unwrap(), DaemonFrame::Malformed { .. }));
}

#[test]
fn session_names_are_validated_as_the_daemon_does() {
    for ok in ["a", "axel", "my-task-2", "20261110-202033-e5f6", &"x".repeat(SESSION_NAME_MAX)] {
        assert_eq!(validate_session_name(ok), Ok(()), "{ok}");
    }
    for bad in ["", "Axel", "my task", "my_task", "naïve", "a.b", &"x".repeat(SESSION_NAME_MAX + 1)] {
        let e = validate_session_name(bad).unwrap_err();
        assert!(!e.to_string().is_empty(), "{bad}");
    }
    assert_eq!(validate_session_name("").unwrap_err().to_string(), "name cannot be empty");
    assert!(validate_session_name("My").unwrap_err().to_string().contains("lowercase"));
    assert!(validate_session_name(&"x".repeat(41)).unwrap_err().to_string().contains("40"));
}

/// `SavedSessions` / `Rename` / a resuming create, byte for byte as the
/// contract and `wire.rs` read them.
#[test]
fn named_session_frames_encode_as_the_daemon_expects() {
    let saved = encode_frame(&ClientFrame::SavedSessions { named_only: true, limit: 200 }).unwrap();
    assert_eq!(saved, "{\"type\":\"saved_sessions\",\"named_only\":true,\"limit\":200}\n");
    let rename = |name: Option<&str>, expect: Option<Option<&str>>| {
        encode_frame(&ClientFrame::Rename { session_id: "s".into(), name: name.map(Into::into), expect: expect.map(|e| e.map(Into::into)) })
            .unwrap()
    };
    assert_eq!(rename(Some("axon"), Some(None)), "{\"type\":\"rename\",\"session_id\":\"s\",\"name\":\"axon\",\"expect\":null}\n");
    assert_eq!(rename(Some("axon"), None), "{\"type\":\"rename\",\"session_id\":\"s\",\"name\":\"axon\"}\n");
    assert_eq!(rename(None, None), "{\"type\":\"rename\",\"session_id\":\"s\",\"name\":null}\n");
    assert_eq!(rename(Some("b"), Some(Some("a"))), "{\"type\":\"rename\",\"session_id\":\"s\",\"name\":\"b\",\"expect\":\"a\"}\n");
    let opts = synaps_client::CreateOptions::new().cwd(synaps_client::Cwd::Path("/work".into())).resume("axel");
    let create = encode_frame(&ClientFrame::Attach(Attach::Create { config: opts.config(), mode: AttachMode::Mirror })).unwrap();
    let v: serde_json::Value = serde_json::from_str(&create).unwrap();
    assert_eq!(v["config"]["continue_session"], "axel");
    assert_eq!(v["config"]["cwd"], "/work");
    // A plain create says nothing about continuing (absent, not null: null
    // is a bare `--continue`).
    let plain = encode_frame(&ClientFrame::Attach(Attach::Create { config: CreateConfig::default(), mode: AttachMode::Mirror })).unwrap();
    assert!(!plain.contains("continue_session"), "{plain}");
    let bare = CreateConfig { continue_session: Some(None), ..Default::default() };
    let bare = encode_frame(&ClientFrame::Attach(Attach::Create { config: bare, mode: AttachMode::Mirror })).unwrap();
    assert!(bare.contains("\"continue_session\":null"), "{bare}");
}
