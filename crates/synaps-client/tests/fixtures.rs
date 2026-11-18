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

/// `roster`, `park_due` and `evict_due` (the daemon's `SessionMeta`):
/// decoded when sent, empty or `None` from a daemon that omits them, and a
/// bad roster entry costs only itself.
#[test]
fn session_meta_mirrors_roster_and_park_times() {
    let DaemonFrame::SessionList { sessions, .. } = decode_frame(fixture("session_list.json").trim()).unwrap() else { panic!() };
    assert_eq!(sessions[0].park_due.as_deref(), Some("2026-11-13T07:35:22.076213017Z"));
    assert_eq!(sessions[0].evict_due, None);
    assert!(sessions[0].roster.is_empty());
    let list = r#"{"type":"session_list","sessions":[{"id":"a","lifecycle":"parked","evict_due":"2026-11-13T08:35:22Z",
        "roster":[{"id":3,"name":"tui","kind":"tui","mode":"mirror","attached_at_ms":1794555143000},{"id":"x"},
                  {"id":4,"name":"desktop","kind":"rpc","mode":"observe","attached_at_ms":1}]},
        {"id":"b","roster":null}]}"#;
    let DaemonFrame::SessionList { sessions, rejected } = decode_frame(list).unwrap() else { panic!() };
    assert_eq!(rejected, 0);
    let names: Vec<(u64, &str, &str)> = sessions[0].roster.iter().map(|c| (c.id, c.name.as_str(), c.mode.as_str())).collect();
    assert_eq!(names, [(3, "tui", "mirror"), (4, "desktop", "observe")]);
    assert_eq!(sessions[0].evict_due.as_deref(), Some("2026-11-13T08:35:22Z"));
    assert!(sessions[1].roster.is_empty());
    assert_eq!(sessions[1], SessionMeta { id: "b".into(), ..SessionMeta::default() });
}

/// `SessionMeta.subagents_running` (SynapsCLI #410): read from a session
/// list and a welcome; an older daemon omits it, and that is 0.
#[test]
fn session_meta_counts_running_workers() {
    let list = r#"{"type":"session_list","sessions":[{"id":"a","busy":false,"subagents_running":2},{"id":"b","busy":true}]}"#;
    let DaemonFrame::SessionList { sessions, rejected } = decode_frame(list).unwrap() else { panic!() };
    assert_eq!(rejected, 0);
    assert_eq!(
        sessions.iter().map(|s| (s.id.as_str(), s.busy, s.subagents_running)).collect::<Vec<_>>(),
        [("a", false, 2), ("b", true, 0)]
    );
    let DaemonFrame::SessionList { sessions, .. } = decode_frame(fixture("session_list.json").trim()).unwrap() else { panic!() };
    assert!(sessions.iter().all(|s| s.subagents_running == 0), "a daemon from before the field");
    let mut welcome: serde_json::Value = serde_json::from_str(fixture("welcome.json").trim()).unwrap();
    welcome["sessions"][0]["subagents_running"] = serde_json::json!(1);
    let DaemonFrame::Welcome(w) = decode_frame(&welcome.to_string()).unwrap() else { panic!() };
    assert_eq!(w.sessions[0].subagents_running, 1);
}

/// `SessionMeta.workers` (SynapsCLI #416, which replaces #410's
/// `subagents_running`): the count comes from either field. A `workers`
/// value this client can't read leaves the session listed, with 0.
#[test]
fn session_meta_reads_workers_from_either_daemon() {
    let row = r#"{"handle_id":"sa_1","subagent_id":1,"agent_name":"scout","status":"running","started_at":"2026-11-18T12:00:00Z","new_field":true}"#;
    let list = format!(
        r#"{{"type":"session_list","sessions":[
            {{"id":"dev","workers":{{"running":3,"rows":[{row}],"more":2}}}},
            {{"id":"local","subagents_running":2}},
            {{"id":"done","workers":{{"rows":[{row}]}}}},
            {{"id":"odd","workers":"three"}},
            {{"id":"odd2","workers":{{"running":"three"}}}},
            {{"id":"null","workers":null}},
            {{"id":"old"}}
        ]}}"#
    );
    let DaemonFrame::SessionList { sessions, rejected } = decode_frame(&list).unwrap() else { panic!() };
    assert_eq!(rejected, 0, "no session is dropped for its workers");
    assert_eq!(
        sessions.iter().map(|s| (s.id.as_str(), s.workers_running())).collect::<Vec<_>>(),
        [("dev", 3), ("local", 2), ("done", 0), ("odd", 0), ("odd2", 0), ("null", 0), ("old", 0)]
    );
    let mut welcome: serde_json::Value = serde_json::from_str(fixture("welcome.json").trim()).unwrap();
    welcome["sessions"][0]["workers"] = serde_json::json!({"running": 1});
    let DaemonFrame::Welcome(w) = decode_frame(&welcome.to_string()).unwrap() else { panic!() };
    assert_eq!(w.sessions[0].workers_running(), 1);
}

/// The attach snapshot's `meta.continue_info` (a resume the daemon followed
/// down a compaction chain, F24): decoded; `null` (a 0.11.1 capture),
/// missing (an older daemon) and a shape this client doesn't read (a newer
/// one) are `None`, and the attach still decodes.
#[test]
fn session_meta_carries_continue_info() {
    assert_eq!(attached("attached_history.json").meta.continue_info, None, "null");
    let mut frame: serde_json::Value = serde_json::from_str(fixture("attached_history.json").trim()).unwrap();
    frame["meta"]["continue_info"] = serde_json::json!({"session_id": "j3", "resolved_via": "compacted", "query": "j1",
        "compaction_notice": "Resumed from j1, followed 2 compactions to j3"});
    let DaemonFrame::Attached(a) = decode_frame(&frame.to_string()).unwrap() else { panic!() };
    let ci = a.meta.continue_info.expect("decoded");
    assert_eq!((ci.query.as_str(), ci.session_id.as_str(), ci.compacted()), ("j1", "j3", true));
    frame["meta"]["continue_info"] = serde_json::json!({"session_id": "j1", "resolved_via": null, "query": "j1"});
    let DaemonFrame::Attached(a) = decode_frame(&frame.to_string()).unwrap() else { panic!() };
    assert!(!a.meta.continue_info.unwrap().compacted(), "the journal itself");
    frame["meta"].as_object_mut().unwrap().remove("continue_info");
    let DaemonFrame::Attached(a) = decode_frame(&frame.to_string()).unwrap() else { panic!() };
    assert_eq!(a.meta.continue_info, None, "missing");
    for bad in [
        serde_json::json!("j1"),
        serde_json::json!(["j1", "j3"]),
        serde_json::json!({"session_id": 3, "resolved_via": "compacted", "query": "j1"}),
        serde_json::json!({"session_id": "j3", "resolved_via": {"kind": "compacted"}, "query": "j1"}),
    ] {
        frame["meta"]["continue_info"] = bad.clone();
        let DaemonFrame::Attached(a) = decode_frame(&frame.to_string()).unwrap() else { panic!("{bad}: the attach decodes") };
        assert_eq!(a.meta.continue_info, None, "{bad}");
        assert!(!a.meta.id.is_empty(), "{bad}: the rest of the meta is read");
    }
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

/// A re-attach that brings the same history back keeps every row's `rev`
/// too (a UI's cached rows stay valid); a row that changed gets a `rev`
/// past every one it had.
#[test]
fn reattach_keeps_revs_of_unchanged_rows() {
    let a = attached("attached_history.json");
    let mut t = Transcript::new();
    t.load_attached(&a).unwrap();
    let before: Vec<(u64, u64)> = t.items().iter().map(|i| (i.id, i.rev)).collect();
    t.load_attached(&a).unwrap();
    assert_eq!(before, t.items().iter().map(|i| (i.id, i.rev)).collect::<Vec<_>>());
    let mut b = a.clone();
    let text = b
        .conversation
        .api_messages
        .iter_mut()
        .rev()
        .filter(|m| m["role"] == "assistant")
        .find_map(|m| m["content"].as_array_mut()?.iter_mut().find(|blk| blk["type"] == "text"))
        .expect("a reply with text");
    text["text"] = serde_json::json!("a different reply");
    t.load_attached(&b).unwrap();
    let after: Vec<(u64, u64)> = t.items().iter().map(|i| (i.id, i.rev)).collect();
    assert_eq!(after.len(), before.len());
    let changed: Vec<usize> = (0..after.len()).filter(|&k| after[k] != before[k]).collect();
    assert!(!changed.is_empty(), "the edited reply changed");
    for k in changed {
        assert_eq!(after[k].0, before[k].0, "same id");
        assert!(after[k].1 > before[k].1, "a newer rev");
    }
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

/// Message times: a history row carries its message's `_synaps_at`; a live
/// row the `ts` of the event that started it, and keeps it when its
/// history form lands (the daemon dates a message when its round ends:
/// here the thinking streamed from 07:34:28, the history says 07:34:30).
/// A legacy message has none.
#[test]
fn rows_carry_their_message_times() {
    let at = |t: &Transcript| t.items().iter().map(|i| (i.kind, i.at.as_deref().map(str::to_string))).collect::<Vec<_>>();
    let mut fresh = Transcript::new();
    fresh.load_attached(&attached("attached_history.json")).unwrap();
    let want: Vec<_> = at(&fresh);
    let s = |v: &str| Some(v.to_string());
    assert_eq!(want[0], (ItemKind::User, s("2026-11-13T07:34:27Z")));
    assert!(want[1..want.len() - 1].iter().all(|(_, t)| *t == s("2026-11-13T07:34:30Z")), "{want:?}");
    assert_eq!(want.last().unwrap(), &(ItemKind::Assistant, s("2026-11-13T07:34:32Z")));

    // Live: a row has the `ts` of the event that started it, from then on.
    let mut t = Transcript::new();
    t.load_attached(&attached("attached_create.json")).unwrap();
    let mut first: Vec<(u64, String)> = Vec::new();
    for (env, ev) in events("turn_events.ndjson") {
        t.apply_at(&ev, &env.ts);
        for i in t.items() {
            let at = i.at.as_deref().unwrap_or_else(|| panic!("row {} has a time", i.id));
            match first.iter().find(|(id, _)| *id == i.id) {
                Some((_, ts)) => assert_eq!(at, ts, "row {} keeps its first time", i.id),
                None => first.push((i.id, at.to_string())),
            }
        }
    }
    let thinking = t.items().iter().find(|i| i.kind == ItemKind::Thinking).unwrap();
    assert_eq!(thinking.at.as_deref(), Some("2026-11-13T07:34:28.569949873Z"), "not the history's 07:34:30");
    assert_eq!(at(&t).iter().map(|r| r.0).collect::<Vec<_>>(), want.iter().map(|r| r.0).collect::<Vec<_>>());
    // A re-attach (a reload) keeps them; a fresh one has the history's.
    let seen = at(&t);
    t.load_attached(&attached("attached_history.json")).unwrap();
    assert_eq!(at(&t), seen);

    // Legacy history: no time, never one made up.
    let mut old = Transcript::new();
    old.apply_history(&[serde_json::json!({"role": "user", "content": "hi"})]);
    assert_eq!(old.items()[0].at, None);
    // A live row keeps its time when its history form has none; another
    // message at its id never takes it.
    old.apply_at(
        &SessionEvent::TurnStarted { turn_baseline: 1, trigger: "user".into(), user_text: Some("again".into()) },
        "2026-11-13T07:40:00.5Z",
    );
    let row = old.items()[1].id;
    let both = [serde_json::json!({"role": "user", "content": "hi"}), serde_json::json!({"role": "user", "content": "again"})];
    old.apply_history(&both);
    assert_eq!((old.items()[1].id, old.items()[1].at.as_deref()), (row, Some("2026-11-13T07:40:00.5Z")));
    // A swapped conversation (cleared, resumed, compacted) reuses the ids,
    // even for the same text: its legacy rows get no time of the old one's.
    old.apply_at(&SessionEvent::Cleared { session_id: String::new() }, "2026-11-13T07:41:00Z");
    old.apply_update(&HistoryUpdate::Replace { messages: both.to_vec() }).unwrap();
    assert_eq!(old.items()[1].id, row, "the id is reused");
    assert!(old.items().iter().filter(|i| !i.is_note()).all(|i| i.at.is_none()), "{:?}", at(&old));
    old.apply_update(&HistoryUpdate::Replace { messages: vec![serde_json::json!({"role": "user", "content": "other"}); 2] }).unwrap();
    assert!(old.items().iter().filter(|i| !i.is_note()).all(|i| i.at.is_none()), "{:?}", at(&old));
}

/// A swap the client sees no replacement for (a resume of the journal it
/// shows: the digest matches) leaves first times on: the turn after it
/// keeps its live times when its history lands, as any other.
#[test]
fn a_swap_with_no_replacement_keeps_live_times() {
    let mut t = Transcript::new();
    let a = attached("attached_create.json");
    t.load_attached(&a).unwrap();
    let id = a.meta.journal_id.clone();
    let resumed = SessionEvent::Resumed { id: 1, old_id: id.clone(), new_id: id, via: None, clamp_notice: None };
    t.apply_at(&resumed, "2026-11-13T07:34:20Z");
    let mut appends = 0;
    for (env, ev) in events("turn_events.ndjson") {
        appends += usize::from(history_of(&ev).is_some());
        t.apply_at(&ev, &env.ts);
    }
    assert_eq!(appends, 3, "every round's history is an append");
    let thinking = t.items().iter().find(|i| i.kind == ItemKind::Thinking).unwrap();
    assert_eq!(thinking.at.as_deref(), Some("2026-11-13T07:34:28.569949873Z"), "not the history's 07:34:30");
}

/// A re-attach after a clear made while the tab was detached (in the TUI,
/// another window): the daemon session is the same, its journal is new,
/// and the same question asked again is another message. Its row reuses
/// the id and has the snapshot's time, not the old one's. The journal is
/// the conversation header's: the attach's `meta.journal_id` still names
/// the one the session started with, as 0.11.1 sends it. A reload of the
/// same journal keeps the first times, and a clear seen live moves the
/// journal the next re-attach is compared with.
#[test]
fn a_reattach_after_a_clear_elsewhere_carries_no_time() {
    let mut t = Transcript::new();
    t.load_attached(&attached("attached_history.json")).unwrap();
    let asked = t.items()[0].clone();
    assert_eq!((asked.kind, asked.at.as_deref()), (ItemKind::User, Some("2026-11-13T07:34:27Z")));
    let again = |journal: &str, at: &str| {
        let mut b = attached("attached_history.json");
        assert_eq!(b.meta.journal_id, "20261113-073427-5fde", "the meta keeps the first journal");
        b.conversation.header["id"] = serde_json::json!(journal);
        let first = b.conversation.api_messages[0].clone();
        b.conversation.api_messages =
            vec![first, serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "Another answer."}]})];
        for m in &mut b.conversation.api_messages {
            m["_synaps_at"] = serde_json::json!(at);
        }
        b.conversation.messages_len = 2;
        b
    };
    let at = |t: &Transcript| t.items().iter().filter(|i| !i.is_note()).map(|i| i.at.as_deref().map(str::to_string)).collect::<Vec<_>>();
    let s = |v: &str| Some(v.to_string());

    t.load_attached(&again("20261113-090000-c1ea", "2026-11-13T09:00:00Z")).unwrap();
    assert_eq!((t.items()[0].id, t.items()[0].text.as_str()), (asked.id, asked.text.as_str()), "the same session: the id is reused");
    assert_eq!(at(&t), [s("2026-11-13T09:00:00Z"), s("2026-11-13T09:00:00Z")], "the new conversation's times");

    // A reload of that journal: the first times stay.
    t.load_attached(&again("20261113-090000-c1ea", "2026-11-13T09:05:00Z")).unwrap();
    assert_eq!(at(&t), [s("2026-11-13T09:00:00Z"), s("2026-11-13T09:00:00Z")]);

    // A clear seen live: the next re-attach to its journal is a reload.
    t.apply_at(&SessionEvent::Cleared { session_id: "20261113-091000-d2fb".into() }, "2026-11-13T09:10:00Z");
    let cleared = again("20261113-091000-d2fb", "2026-11-13T09:11:00Z");
    t.apply_update(&HistoryUpdate::Replace { messages: cleared.conversation.api_messages.clone() }).unwrap();
    assert_eq!(at(&t), [s("2026-11-13T09:11:00Z"), s("2026-11-13T09:11:00Z")], "swapped: no carry");
    t.load_attached(&again("20261113-091000-d2fb", "2026-11-13T09:15:00Z")).unwrap();
    assert_eq!(at(&t), [s("2026-11-13T09:11:00Z"), s("2026-11-13T09:11:00Z")], "the same journal: kept");
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

/// A worker's watch page as v0.11.1 serializes it (`subagent_watch.rs`):
/// it decodes, a kind this client does not know is `Unknown` (never a
/// failed page), and the entries replay into a transcript of their own
/// exactly as the main conversation's stream events would.
#[test]
fn a_watch_page_decodes_and_replays_like_the_main_stream() {
    let page: WatchPage = serde_json::from_value(serde_json::json!({
        "handle_id": "sa_3", "subagent_id": 3, "agent_name": "sleeper", "model": "claude-sonnet-5-5",
        "status": "running", "steerable": true, "next_seq": 9, "truncated": false, "more": false,
        "events": [
            {"kind": "response_start"},
            {"kind": "thinking", "text": "slow steps"},
            {"kind": "text", "text": "Step 1."},
            {"kind": "tool_use_start", "tool_id": "t1", "tool_name": "bash"},
            {"kind": "tool_use", "tool_id": "t1", "tool_name": "bash", "input": {"command": "sleep 5"}},
            {"kind": "tool_queued", "tool_id": "t1"},
            {"kind": "tool_started", "tool_id": "t1"},
            {"kind": "tool_result_delta", "tool_id": "t1", "delta": "ok"},
            {"kind": "tool_result", "tool_id": "t1", "result": "ok\n"},
            {"kind": "steered", "text": "stop after the next step"},
            {"kind": "steered", "text": "<event id=\"e\" source=\"subagent\">x</event>"},
            {"kind": "from_the_future", "x": 1},
            {"kind": "failed", "category": "rate_limit"},
            {"kind": "done"}
        ],
        "at_ms": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
        "now_ms": 20
    }))
    .unwrap();
    assert_eq!((page.handle_id.as_str(), page.next_seq, page.running(), page.steerable), ("sa_3", 9, true, true));
    assert_eq!(page.events[11], WatchEvent::Unknown);
    let mut t = Transcript::new();
    for ev in &page.events {
        t.apply_watch(ev);
    }
    assert_eq!(
        texts(&t),
        vec![
            (ItemKind::Thinking, "slow steps".into()),
            (ItemKind::Assistant, "Step 1.".into()),
            (ItemKind::Tool, String::new()),
            (ItemKind::User, "stop after the next step".into()),
            (ItemKind::Error, "subagent failed [rate_limit]".into()),
        ]
    );
    let tool = t.items()[2].tool.as_ref().unwrap();
    assert_eq!(
        (tool.name.as_str(), tool.input.as_str(), tool.result.as_str(), tool.status),
        ("bash", "sleep 5", "ok\n", Some(ToolStatus::Done))
    );
    // An older daemon's page: no model, no timing.
    let old: WatchPage = serde_json::from_str(r#"{"handle_id":"sa_1","subagent_id":1,"agent_name":"a","status":"completed","steerable":false,"next_seq":0,"truncated":true,"more":false,"events":[]}"#).unwrap();
    assert_eq!((old.model.as_str(), old.at_ms.len(), old.now_ms, old.running(), old.truncated), ("", 0, None, false, true));
}

/// The watch query and the worker steer go out in the daemon's shapes
/// (`types.rs` v0.11.1: `{"query":"subagent_watch",…}`, `tool_queue`
/// left out when off), and a steer's text never prints.
#[test]
fn the_watch_query_and_the_worker_steer_encode_as_the_daemon_reads_them() {
    let q =
        |tool_queue| SessionCommand::Query { id: 7, query: SessionQuery::SubagentWatch { handle_id: "sa_2".into(), since: 4, tool_queue } };
    assert_eq!(
        serde_json::to_value(q(true)).unwrap(),
        serde_json::json!({"cmd": "query", "id": 7, "query": {"query": "subagent_watch", "handle_id": "sa_2", "since": 4, "tool_queue": true}})
    );
    assert!(!serde_json::to_string(&q(false)).unwrap().contains("tool_queue"));
    let steer = SessionCommand::SubagentSteer { handle_id: "sa_2".into(), text: "focus on X".into() };
    assert_eq!(
        serde_json::to_value(&steer).unwrap(),
        serde_json::json!({"cmd": "subagent_steer", "handle_id": "sa_2", "text": "focus on X"})
    );
    assert!(!format!("{steer:?}").contains("focus"));
}

/// A worker's transcript keeps its newest rows: the oldest go past the
/// caps (the newest always stays), its tools keep working, and a
/// transcript with history is never trimmed.
#[test]
fn a_live_only_transcript_trims_its_oldest_rows() {
    let mut t = Transcript::new();
    for i in 0..10 {
        t.apply_watch(&WatchEvent::ToolUse {
            tool_id: format!("t{i}"),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
        });
    }
    let _ = t.take_splice();
    assert_eq!(t.trim_live_front(4, usize::MAX), 6);
    assert_eq!(t.len(), 4);
    assert_eq!(t.take_splice().map(|s| s.start), Some(0));
    t.apply_watch(&WatchEvent::ToolResult { tool_id: "t9".into(), result: "done".into() });
    assert_eq!(t.items()[3].tool.as_ref().unwrap().result, "done");
    // A dropped tool's result goes nowhere.
    t.apply_watch(&WatchEvent::ToolResult { tool_id: "t0".into(), result: "late".into() });
    assert!(t.items().iter().all(|i| i.tool.as_ref().unwrap().result != "late"));
    assert_eq!(t.trim_live_front(0, 0), 3, "the newest row stays");
    let mut h = Transcript::new();
    h.apply_history(&[serde_json::json!({"role": "user", "content": "hi"}), serde_json::json!({"role": "assistant", "content": "yo"})]);
    assert_eq!(h.trim_live_front(1, 0), 0);
}

/// A prompt's `raised_at` and `confirm_scope` (agent-engine
/// `session::types::PromptRequest`) decode when they are there; missing,
/// `null` or a shape this client doesn't read is `None`, never a failed
/// prompt (or a failed attach).
#[test]
fn a_prompts_raised_at_and_scope_are_read_leniently() {
    let full: PromptRequest = serde_json::from_value(serde_json::json!({
        "id": 7, "kind": "confirm", "title": "Confirm tool call", "prompt": "Allow?",
        "raised_at": "2026-11-17T14:21:05.123Z",
        "confirm_scope": {"extensions": ["guard"], "tool": "bash"}
    }))
    .unwrap();
    assert_eq!(full.raised_at.as_deref(), Some("2026-11-17T14:21:05.123Z"));
    let scope = full.allow_all().expect("a confirm with an extension offers Allow all");
    assert_eq!(scope.tool.as_deref(), Some("bash"));
    assert_eq!(scope.allow_all_label(), "Allow all from guard this session");
    assert!(scope.rewritten_by.is_empty());
    for odd in [
        serde_json::json!({"id": 1}),
        serde_json::json!({"id": 1, "raised_at": null, "confirm_scope": null}),
        serde_json::json!({"id": 1, "raised_at": 17, "confirm_scope": ["guard"]}),
        serde_json::json!({"id": 1, "raised_at": {}, "confirm_scope": {"extensions": "guard"}}),
    ] {
        let pr: PromptRequest = serde_json::from_value(odd.clone()).unwrap_or_else(|e| panic!("{odd}: {e}"));
        assert_eq!((pr.raised_at, pr.confirm_scope), (None, None), "{odd}");
    }
    // No extension to latch, or a secret: no Allow all.
    let empty: PromptRequest =
        serde_json::from_value(serde_json::json!({"id": 2, "confirm_scope": {"extensions": [], "tool": "bash"}})).unwrap();
    assert!(empty.confirm_scope.is_some() && empty.allow_all().is_none());
    let secret: PromptRequest =
        serde_json::from_value(serde_json::json!({"id": 3, "kind": "secret", "confirm_scope": {"extensions": ["guard"]}})).unwrap();
    assert!(secret.allow_all().is_none());
}
