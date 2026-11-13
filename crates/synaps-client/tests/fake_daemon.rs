//! The connection thread against an in-process fake daemon on a real Unix
//! socket: the attach handshake, history resync, refusals, detach, reload
//! and shutdown. No real daemon, no model.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::{Value, json};
use synaps_client::history::{HistoryUpdate, messages_hash};
use synaps_client::{AttachMode, Client, ClientOptions, CreateOptions, Cwd, Endpoint, Error, Event, Identity, Status};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};

const T: Duration = Duration::from_secs(10);

type Writer = Arc<Mutex<OwnedWriteHalf>>;

async fn write(w: &Writer, v: Value) {
    let mut line = v.to_string();
    line.push('\n');
    let _ = w.lock().await.write_all(line.as_bytes()).await;
}

/// One accepted connection that sent `Attach` (handed to the test).
struct Conn {
    w: Writer,
    r: Lines<BufReader<OwnedReadHalf>>,
    hello: Value,
    attach: Value,
}

impl Conn {
    async fn send(&self, v: Value) {
        write(&self.w, v).await;
    }

    /// The next frame from the client; `None` at EOF.
    async fn recv(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(T, self.r.next_line()).await.expect("client went quiet").ok()??;
        Some(serde_json::from_str(&line).unwrap())
    }

    async fn recv_cmd(&mut self) -> Value {
        loop {
            let f = self.recv().await.expect("eof waiting for a cmd");
            if f["type"] == "cmd" {
                return f["cmd"].clone();
            }
        }
    }

    async fn event(&self, seq: u64, event: Value) {
        self.send(json!({"type": "event", "session_id": SID, "seq": seq, "ts": "t", "event": event})).await;
    }
}

const SID: &str = "20261113-073427-5fde";

struct Fake {
    path: PathBuf,
    /// Connections that sent `Attach`.
    attaches: mpsc::UnboundedReceiver<Conn>,
    /// Every accepted connection's writer, in accept order (the first is
    /// the control connection). Weak: dropping a `Conn` closes its socket.
    accepted: mpsc::UnboundedReceiver<Weak<Mutex<OwnedWriteHalf>>>,
    /// Every frame after `Hello` that was not an `Attach`, in order.
    control: mpsc::UnboundedReceiver<Value>,
}

/// What the fake daemon is.
#[derive(Clone, Copy, Default)]
struct Opts {
    /// `Welcome.features` lists `saved_sessions` (and answers the frame).
    saved: bool,
    /// Predates `Rename`: answers it `Error{unknown variant}`.
    old: bool,
}

/// A second live session, unnamed.
const SID2: &str = "20261113-080000-aaaa";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn attached_frame(client: u64) -> Value {
    let mut a: Value = serde_json::from_str(fixture("attached_history.json").trim()).unwrap();
    a["client"] = json!(client);
    a["input_owner"] = json!(client);
    a
}

fn welcome(opts: Opts) -> Value {
    let features: Vec<&str> = if opts.saved { vec!["owned_sessions", "saved_sessions"] } else { vec!["owned_sessions"] };
    json!({"type": "welcome", "protocol_version": 3, "daemon_version": "0.0.0-fake", "pid": 1, "profile": null,
           "sessions": [{"id": SID, "model": "m", "lifecycle": "live", "name": "named-one"},
                        {"id": SID2, "model": "m", "lifecycle": "live", "journal_id": "20261101-000000-bbbb"}],
           "progressive_tool_disclosure": false, "generation": 1, "features": features})
}

/// The fake's `Rename` (`daemon/rename.rs`): `None` = `Refused`, then close.
fn rename_reply(opts: Opts, f: &Value) -> Option<Value> {
    if opts.old {
        return Some(json!({"type": "error", "session_id": null,
            "message": "malformed frame: unknown variant `rename`, expected one of `hello`, `ping`, `sessions`"}));
    }
    let sid = f["session_id"].as_str().unwrap_or_default();
    let current = match sid {
        SID => Some("named-one"),
        SID2 => None,
        _ => return None,
    };
    let name = &f["name"];
    if name == "taken-one" {
        return Some(json!({"type": "rename_failed", "session_id": sid, "reason": "taken",
            "message": "name 'taken-one' is taken", "taken_by": "20261028-045932-a1b2"}));
    }
    if let Some(expect) = f.get("expect")
        && expect.as_str() != current
    {
        return Some(json!({"type": "rename_failed", "session_id": sid, "reason": "named",
            "message": "the session is named", "current_name": current}));
    }
    Some(json!({"type": "renamed", "session_id": sid, "name": name}))
}

impl Fake {
    async fn start(name: &str) -> Fake {
        Self::start_with(name, Opts::default()).await
    }

    async fn start_with(name: &str, opts: Opts) -> Fake {
        let dir = std::env::temp_dir().join(format!("synaps-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("d.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (atx, attaches) = mpsc::unbounded_channel();
        let (ctx, accepted) = mpsc::unbounded_channel();
        let (ftx, control) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (r, w) = stream.into_split();
                let w: Writer = Arc::new(Mutex::new(w));
                let _ = ctx.send(Arc::downgrade(&w));
                let (atx, ftx) = (atx.clone(), ftx.clone());
                tokio::spawn(async move {
                    let mut r = BufReader::new(r).lines();
                    let Ok(Some(hello)) = r.next_line().await else { return };
                    let hello: Value = serde_json::from_str(&hello).unwrap();
                    write(&w, welcome(opts)).await;
                    while let Ok(Some(line)) = r.next_line().await {
                        let f: Value = serde_json::from_str(&line).unwrap();
                        if f["type"] != "attach" {
                            let _ = ftx.send(f.clone());
                        }
                        match f["type"].as_str() {
                            Some("sessions") => write(&w, json!({"type": "session_list", "sessions": []})).await,
                            Some("saved_sessions") if opts.saved => {
                                let mut list: Value = serde_json::from_str(fixture("saved_session_list.json").trim()).unwrap();
                                list["truncated"] = json!(f["limit"] == 1);
                                write(&w, list).await;
                            }
                            Some("saved_sessions") => {
                                write(&w, json!({"type": "error", "session_id": null,
                                    "message": "malformed frame: unknown variant `saved_sessions`"})).await;
                            }
                            Some("rename") => match rename_reply(opts, &f) {
                                Some(reply) => write(&w, reply).await,
                                None => {
                                    write(&w, json!({"type": "refused", "reason": {"reason": "unknown_session"},
                                        "message": format!("no live session {}", f["session_id"])})).await;
                                    return;
                                }
                            },
                            Some("attach") => {
                                let _ = atx.send(Conn { w, r, hello, attach: f });
                                return;
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        Fake { path, attaches, accepted, control }
    }

    fn client(&self) -> (Client, async_channel::Receiver<Event>) {
        Client::spawn(ClientOptions::new(Endpoint::Socket(self.path.clone()), Identity::new("tester").unwrap())).unwrap()
    }

    async fn next_attach(&mut self) -> Conn {
        tokio::time::timeout(T, self.attaches.recv()).await.expect("no attach").unwrap()
    }

    /// The next control frame (not `Hello`, not `Attach`).
    async fn next_control(&mut self) -> Value {
        tokio::time::timeout(T, self.control.recv()).await.expect("no control frame").unwrap()
    }
}

/// The next event matching `pick`, skipping others.
async fn wait<R>(events: &async_channel::Receiver<Event>, what: &str, mut pick: impl FnMut(&Event) -> Option<R>) -> R {
    let until = tokio::time::Instant::now() + T;
    loop {
        let ev = tokio::time::timeout_at(until, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .expect("event channel closed");
        if let Some(r) = pick(&ev) {
            return r;
        }
    }
}

async fn attached(fake: &mut Fake, events: &async_channel::Receiver<Event>, client: u64) -> Conn {
    let conn = fake.next_attach().await;
    conn.send(attached_frame(client)).await;
    wait(events, "attached", |e| matches!(e, Event::Attached { .. }).then_some(())).await;
    conn
}

/// H1: `/resume` sends only a digest. The client re-queries `Messages`
/// with an id below the reserved range and turns the result into a
/// `Replace`; an append-shaped `MessageHistory` crosses as its suffix only;
/// `/clear` (an empty digest) replaces without a query.
#[tokio::test]
async fn h1_digest_drift_resyncs_the_history() {
    let mut fake = Fake::start("h1").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;

    let resumed = vec![json!({"content": "What is 2+2?", "role": "user"}), json!({"content": [{"text": "4", "type": "text"}], "role": "assistant"})];
    conn.event(10, json!({"ev": "resumed", "id": 7, "old_id": SID, "new_id": "r", "via": null, "clamp_notice": null})).await;
    conn.event(11, json!({"ev": "conversation", "digest": {"messages_len": 2, "messages_hash": messages_hash(&resumed), "cost": 0.1}})).await;
    let q = conn.recv_cmd().await;
    assert_eq!(q["cmd"], "query");
    assert_eq!(q["query"]["query"], "messages");
    let id = q["id"].as_u64().unwrap();
    assert!(id < 1 << 63, "a client query id must be below RESERVED_QUERY_ID_BASE");
    // Another client's query result is not ours.
    conn.event(12, json!({"ev": "query_result", "id": id ^ 1, "value": []})).await;
    conn.event(13, json!({"ev": "query_result", "id": id, "value": resumed})).await;
    let up = wait(&events, "history", |e| match e {
        Event::History { update, attach_seq, .. } => Some((update.clone(), *attach_seq)),
        _ => None,
    })
    .await;
    assert_eq!(up, (HistoryUpdate::Replace { messages: resumed.clone() }, seq));

    // The next round grows it: only the new message crosses.
    let mut grown = resumed.clone();
    grown.push(json!({"content": "and 3+3?", "role": "user"}));
    conn.event(14, json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": grown}})).await;
    let up = wait(&events, "append", |e| match e {
        Event::History { update, .. } => Some(update.clone()),
        _ => None,
    })
    .await;
    assert_eq!(up, HistoryUpdate::Append { base_len: 2, messages: vec![json!({"content": "and 3+3?", "role": "user"})] });

    // A matching digest: no query. Then /clear: an empty replace, no query.
    conn.event(15, json!({"ev": "conversation", "digest": {"messages_len": 3, "messages_hash": messages_hash(&grown)}})).await;
    conn.event(16, json!({"ev": "cleared", "session_id": "n"})).await;
    conn.event(17, json!({"ev": "conversation", "digest": {"messages_len": 0, "messages_hash": messages_hash(&[])}})).await;
    let up = wait(&events, "clear", |e| match e {
        Event::History { update, .. } => Some(update.clone()),
        _ => None,
    })
    .await;
    assert_eq!(up, HistoryUpdate::Replace { messages: vec![] });
    // Nothing else was asked of the daemon: the next frame is our submit.
    client.submit("x").unwrap();
    assert_eq!(conn.recv_cmd().await["cmd"], "submit");
    client.shutdown().unwrap();
}

/// H2: a `Refused` addressed to this client is a typed error (so the app
/// can give the text back); one for another client is not. A daemon
/// "refused: ..." / reload-drain `Error` frame is a refusal too.
#[tokio::test]
async fn h2_refusals_reach_the_consumer() {
    let mut fake = Fake::start("h2").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    client.submit("my precious text").unwrap();
    assert_eq!(conn.recv_cmd().await["text"], "my precious text");
    conn.event(20, json!({"ev": "refused", "client": 9, "command": "cancel", "reason": "not you"})).await;
    conn.event(21, json!({"ev": "refused", "client": 2, "command": "submit", "reason": "input is owned by client #1"})).await;
    let err = wait(&events, "refused", |e| match e {
        Event::Error { error, attach_seq } => Some((error.clone(), *attach_seq)),
        _ => None,
    })
    .await;
    assert_eq!(err, (Error::Refused { command: Some("submit".into()), reason: "input is owned by client #1".into() }, Some(seq)));
    conn.send(json!({"type": "error", "session_id": SID, "message": "daemon reloading; retry in a moment"})).await;
    let err = wait(&events, "drain refusal", |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(matches!(err, Error::Refused { command: None, .. }), "{err:?}");
    client.shutdown().unwrap();
}

/// H3: after `Detached` there is no dead handle: commands report
/// `NotAttached` instead of vanishing.
#[tokio::test]
async fn h3_commands_after_detached_report_an_error() {
    let mut fake = Fake::start("h3").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let conn = attached(&mut fake, &events, 2).await;
    conn.event(30, json!({"ev": "ended", "reason": "client_quit"})).await;
    let (s, reason) = wait(&events, "detached", |e| match e {
        Event::Detached { attach_seq, reason, .. } => Some((*attach_seq, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!((s, reason.as_str()), (seq, "session ended"));
    client.submit("hello?").unwrap();
    let err = wait(&events, "not attached", |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(err, Error::NotAttached { command: "submit" });
    client.shutdown().unwrap();
}

/// M1: a `Refused{busy}` (reload drain) on the first create is retried, and
/// the create goes out once more; the create carries an explicit cwd (M8).
#[tokio::test]
async fn m1_m8_first_create_rides_out_a_busy_daemon() {
    let mut fake = Fake::start("m1").await;
    let (client, events) = fake.client();
    let seq = client.create(CreateOptions::new().cwd(Cwd::Path("/tmp".into())).model("m")).unwrap();
    let first = fake.next_attach().await;
    assert_eq!(first.attach["attach"], "create");
    assert_eq!(first.attach["config"]["cwd"], "/tmp");
    assert_eq!(first.attach["config"]["model_override"], "m");
    first.send(json!({"type": "refused", "reason": {"reason": "busy"}, "message": "daemon reloading; retry in a moment"})).await;
    drop(first);
    let second = fake.next_attach().await;
    assert_eq!(second.attach["attach"], "create");
    second.send(attached_frame(5)).await;
    let s = wait(&events, "attached", |e| match e {
        Event::Attached { attach_seq, attached, .. } => Some((*attach_seq, attached.client)),
        _ => None,
    })
    .await;
    assert_eq!(s, (seq, 5));
    client.shutdown().unwrap();
}

/// M1: an `Attached` that does not decode ends the attach with the decode
/// error instead of waiting forever for a frame that already came.
#[tokio::test]
async fn m1_an_undecodable_attached_does_not_hang() {
    let mut fake = Fake::start("m1b").await;
    let (client, events) = fake.client();
    client.attach(SID, AttachMode::Mirror).unwrap();
    let conn = fake.next_attach().await;
    conn.send(json!({"type": "attached", "client": "two"})).await;
    let reason = wait(&events, "detached", |e| match e {
        Event::Detached { reason, .. } => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert!(reason.contains("could not decode attached"), "{reason}");
    client.shutdown().unwrap();
}

/// M1: a create whose connection drops after the create went out is not
/// blindly re-sent (it may have happened); with a name, the retry attaches
/// to the session of that name.
#[tokio::test]
async fn m1_a_named_create_retry_attaches_instead_of_duplicating() {
    let mut fake = Fake::start("m1c").await;
    let (client, events) = fake.client();
    client.create(CreateOptions::new().name("named-one")).unwrap();
    let first = fake.next_attach().await;
    assert_eq!(first.attach["config"]["name"], "named-one");
    drop(first);
    let second = fake.next_attach().await;
    assert_eq!(second.attach["attach"], "existing");
    assert_eq!(second.attach["session_id"], SID);
    second.send(attached_frame(3)).await;
    wait(&events, "attached", |e| matches!(e, Event::Attached { .. }).then_some(())).await;

    // Unnamed: no retry, a Detached that says why.
    client.create(CreateOptions::new()).unwrap();
    drop(fake.next_attach().await);
    let reason = wait(&events, "detached", |e| match e {
        Event::Detached { reason, .. } if reason.contains("may exist") => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert!(reason.contains("being created"));
    client.shutdown().unwrap();
}

/// M2 / API 8: the control connection reads frames while idle (no polling
/// by default), so a `Bye{Reloading}` is seen at once, not on the next poll.
#[tokio::test]
async fn m2_control_sees_a_reload_at_once() {
    let mut fake = Fake::start("m2").await;
    let (client, events) = fake.client();
    wait(&events, "connected", |e| matches!(e, Event::Status(Status::Connected { .. })).then_some(())).await;
    let control = tokio::time::timeout(T, fake.accepted.recv()).await.unwrap().unwrap().upgrade().unwrap();
    let t0 = tokio::time::Instant::now();
    write(&control, json!({"type": "bye", "reason": {"bye": "reloading", "generation": 2, "retry_after_ms": 50}})).await;
    wait(&events, "reconnecting", |e| matches!(e, Event::Status(Status::Reconnecting { .. })).then_some(())).await;
    assert!(t0.elapsed() < Duration::from_secs(2));
    wait(&events, "connected again", |e| matches!(e, Event::Status(Status::Connected { .. })).then_some(())).await;
    client.shutdown().unwrap();
}

/// Reload: an observer re-attaches as an observer (never `Takeover`), with
/// `reconnect_of`, and input sent meanwhile is refused, not delivered late
/// (L5). Shutdown then sends `Detach` + `Bye` (API 7).
#[tokio::test]
async fn reload_reattaches_observe_and_refuses_input_meanwhile() {
    let mut fake = Fake::start("reload").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Observe).unwrap();
    let conn = fake.next_attach().await;
    assert_eq!(conn.attach["mode"], "observe");
    // An observer that is (oddly) listed as owner must still not take over.
    conn.send(attached_frame(4)).await;
    wait(&events, "attached", |e| matches!(e, Event::Attached { .. }).then_some(())).await;
    conn.send(json!({"type": "bye", "reason": {"bye": "reloading", "generation": 2, "retry_after_ms": 300}})).await;
    drop(conn);
    wait(&events, "reattaching", |e| matches!(e, Event::Reattaching { attach_seq, .. } if *attach_seq == seq).then_some(())).await;
    client.abort().unwrap();
    let err = wait(&events, "refused while reconnecting", |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(matches!(&err, Error::Refused { command: Some(c), .. } if c == "cancel"), "{err:?}");
    let mut again = fake.next_attach().await;
    assert_eq!(again.attach["mode"], "observe");
    assert_eq!(again.hello["reconnect_of"]["previous_client"], 4);
    assert_eq!(again.hello["reconnect_of"]["generation"], 2);
    again.send(attached_frame(6)).await;
    let reconnect = wait(&events, "re-attached", |e| match e {
        Event::Attached { reconnect, attach_seq, .. } => Some((*reconnect, *attach_seq)),
        _ => None,
    })
    .await;
    assert_eq!(reconnect, (true, seq));
    client.shutdown().unwrap();
    let detach = again.recv_cmd().await;
    assert_eq!(detach, json!({"cmd": "detach", "client": 6}));
    assert_eq!(again.recv().await.unwrap()["type"], "bye");
}

/// API 3: switching sessions reports the old attachment `Detached` with its
/// own `attach_seq`, after a clean detach.
#[tokio::test]
async fn switching_detaches_the_old_attachment() {
    let mut fake = Fake::start("switch").await;
    let (client, events) = fake.client();
    let first = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    let second = client.attach(SID, AttachMode::Mirror).unwrap();
    assert!(second > first);
    assert_eq!(conn.recv_cmd().await, json!({"cmd": "detach", "client": 2}));
    let s = wait(&events, "old detached", |e| match e {
        Event::Detached { attach_seq, .. } => Some(*attach_seq),
        _ => None,
    })
    .await;
    assert_eq!(s, first);
    let next = fake.next_attach().await;
    next.send(attached_frame(3)).await;
    let s = wait(&events, "attached", |e| match e {
        Event::Attached { attach_seq, .. } => Some(*attach_seq),
        _ => None,
    })
    .await;
    assert_eq!(s, second);
    client.shutdown().unwrap();
}

/// Input-owner changes surface as `InputOwner` with this client's id.
#[tokio::test]
async fn input_owner_changes_are_events() {
    let mut fake = Fake::start("owner").await;
    let (client, events) = fake.client();
    client.attach(SID, AttachMode::Mirror).unwrap();
    let conn = attached(&mut fake, &events, 2).await;
    conn.event(40, json!({"ev": "input_owner_changed", "from": 2, "to": 1, "reason": "takeover"})).await;
    let got = wait(&events, "owner", |e| match e {
        Event::InputOwner { client, owner, reason, .. } => Some((*client, *owner, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(got, (2, Some(1), "takeover".to_string()));
    client.shutdown().unwrap();
}

fn history_frame(messages: &[Value]) -> Value {
    json!({"ev": "stream", "event": {"kind": "session", "session": "message_history", "messages": messages}})
}

/// N1 through the client loop: a fresh `Messages` result that still
/// disagrees with the digest by hash stops the re-querying (no storm), and
/// from then on every history is a `Replace`, never spliced onto old rows.
#[tokio::test]
async fn n1_under_hash_distrust_histories_replace_and_do_not_requery() {
    let mut fake = Fake::start("n1").await;
    let (client, events) = fake.client();
    client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    let a = attached_frame(2);
    let history: Vec<Value> = a["conversation"]["api_messages"].as_array().unwrap().clone();
    let n = history.len();
    let off = json!({"ev": "conversation", "digest": {"messages_len": n, "messages_hash": 7}});
    conn.event(50, off.clone()).await;
    let q = conn.recv_cmd().await;
    assert_eq!(q["cmd"], "query");
    conn.event(51, json!({"ev": "query_result", "id": q["id"], "value": history})).await;
    let up = wait(&events, "resync", |e| match e {
        Event::History { update, .. } => Some(update.clone()),
        _ => None,
    })
    .await;
    assert_eq!(up, HistoryUpdate::Replace { messages: history.clone() });

    // Still "off" by hash: no second query.
    conn.event(52, off).await;
    // A same-length rewrite and a longer rewrite: both replace.
    let mut same_len = history.clone();
    same_len[n - 1] = json!({"role": "assistant", "content": "repaired"});
    conn.event(53, history_frame(&same_len)).await;
    let longer: Vec<Value> = (0..n + 2).map(|i| json!({"role": "user", "content": format!("other {i}")})).collect();
    conn.event(54, history_frame(&longer)).await;
    for want in [same_len, longer] {
        let up = wait(&events, "history", |e| match e {
            Event::History { update, .. } => Some(update.clone()),
            _ => None,
        })
        .await;
        assert_eq!(up, HistoryUpdate::Replace { messages: want });
    }
    client.submit("x").unwrap();
    assert_eq!(conn.recv_cmd().await["cmd"], "submit", "no query after the distrusted digest");
    client.shutdown().unwrap();
}

/// N2: `QueryResult`s that are not this client's resync (another client's,
/// which is a whole history when a co-attached TUI resyncs, and the
/// daemon's reserved ids) never reach the consumer.
#[tokio::test]
async fn n2_foreign_and_reserved_query_results_are_dropped() {
    let mut fake = Fake::start("n2").await;
    let (client, events) = fake.client();
    client.attach(SID, AttachMode::Mirror).unwrap();
    let conn = attached(&mut fake, &events, 2).await;
    let big: Vec<Value> = (0..50).map(|i| json!({"role": "user", "content": format!("m{i}")})).collect();
    conn.event(60, json!({"ev": "query_result", "id": 12345, "value": big})).await;
    conn.event(61, json!({"ev": "query_result", "id": (1u64 << 63) + 1, "value": big})).await;
    conn.event(62, json!({"ev": "query_result", "id": u64::MAX, "value": null})).await;
    conn.event(63, json!({"ev": "idle"})).await;
    let first = wait(&events, "a session event", |e| match e {
        Event::Session { raw, .. } => Some(raw["ev"].clone()),
        Event::History { .. } => Some(json!("history")),
        _ => None,
    })
    .await;
    assert_eq!(first, "idle");
    client.shutdown().unwrap();
}

/// M5: a drifted prompt in the attach snapshot is counted on `Attached`
/// and reported as `Error::Decode`, not silently dropped.
#[tokio::test]
async fn m5_attach_snapshot_rejects_surface_as_decode_errors() {
    let mut fake = Fake::start("m5").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let conn = fake.next_attach().await;
    let mut a = attached_frame(2);
    a["pending_prompts"] = json!([{"id": "p-1", "title": "Allow bash?"}]);
    conn.send(a).await;
    let rejected = wait(&events, "attached", |e| match e {
        Event::Attached { attached, .. } => Some((attached.pending_prompts.len(), attached.rejected.pending_prompts)),
        _ => None,
    })
    .await;
    assert_eq!(rejected, (0, 1));
    let err = wait(&events, "decode error", |e| match e {
        Event::Error { error: Error::Decode(d), attach_seq } => Some((d.kind.clone(), d.error.clone(), *attach_seq)),
        _ => None,
    })
    .await;
    assert_eq!(err.0, "attached");
    assert!(err.1.contains("1 pending_prompts"), "{}", err.1);
    assert_eq!(err.2, Some(seq));
    client.shutdown().unwrap();
}

/// N8: a daemon that accepts, greets and hangs up at once is retried with
/// a growing backoff, not in a tight loop.
#[tokio::test]
async fn n8_a_daemon_that_hangs_up_at_once_is_backed_off() {
    let dir = std::env::temp_dir().join(format!("synaps-fake-{}-n8", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.sock");
    let listener = UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (r, mut w) = stream.into_split();
            let mut r = BufReader::new(r).lines();
            let _ = r.next_line().await;
            let mut line = welcome(Opts::default()).to_string();
            line.push('\n');
            let _ = w.write_all(line.as_bytes()).await;
        }
    });
    let (client, events) = Client::spawn(ClientOptions::new(Endpoint::Socket(path), Identity::new("tester").unwrap())).unwrap();
    let until = tokio::time::Instant::now() + Duration::from_millis(1500);
    let mut connects = 0;
    while let Ok(Ok(e)) = tokio::time::timeout_at(until, events.recv()).await {
        if matches!(e, Event::Status(Status::Connected { .. })) {
            connects += 1;
        }
    }
    // 100 + 200 + 400 + 800 ms of backoff fit in 1.5 s: about 4 connects.
    assert!((2..=6).contains(&connects), "{connects} connects in 1.5 s");
    client.shutdown().unwrap();
}

/// N10: a detach while a create is in flight says the session may exist.
#[tokio::test]
async fn n10_a_detach_during_a_create_says_it_may_exist() {
    let mut fake = Fake::start("n10").await;
    let (client, events) = fake.client();
    let seq = client.create(CreateOptions::new()).unwrap();
    let conn = fake.next_attach().await;
    assert_eq!(conn.attach["attach"], "create");
    client.detach().unwrap();
    let (s, reason) = wait(&events, "detached", |e| match e {
        Event::Detached { attach_seq, reason, .. } => Some((*attach_seq, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(s, seq);
    assert!(reason.contains("may exist"), "{reason}");
    client.shutdown().unwrap();
}

/// N9: attaches from several clones on several threads reach the
/// connection thread in `attach_seq` order (each replaces the one before,
/// so the `Detached` reports come out increasing).
#[tokio::test]
async fn n9_attach_seqs_are_enqueued_in_order() {
    let fake = Fake::start("n9").await;
    let (client, events) = fake.client();
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let c = client.clone();
            std::thread::spawn(move || {
                for _ in 0..25 {
                    c.attach(SID, AttachMode::Observe).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    client.detach().unwrap();
    let mut seen = Vec::new();
    while seen.len() < 100 {
        let s = wait(&events, "detached", |e| match e {
            Event::Detached { attach_seq, .. } => Some(*attach_seq),
            _ => None,
        })
        .await;
        seen.push(s);
    }
    assert_eq!(seen, (1..=100).collect::<Vec<u64>>());
    drop(fake);
    client.shutdown().unwrap();
}

async fn connected(events: &async_channel::Receiver<Event>) -> Vec<String> {
    wait(events, "connected", |e| match e {
        Event::Status(Status::Connected { features, .. }) => Some(features.clone()),
        _ => None,
    })
    .await
}

/// Named sessions: a daemon that lists `saved_sessions` is asked with the
/// contract's frame, and the reply arrives as `Event::SavedSessions`.
#[tokio::test]
async fn saved_sessions_are_asked_of_a_daemon_that_lists_them() {
    let mut fake = Fake::start_with("saved", Opts { saved: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    let features = connected(&events).await;
    assert!(features.iter().any(|f| f == "saved_sessions"));
    assert!(client.supports(synaps_client::FEATURE_SAVED_SESSIONS));
    client.saved_sessions(true, 200).unwrap();
    assert_eq!(fake.next_control().await, json!({"type": "saved_sessions", "named_only": true, "limit": 200}));
    let (sessions, truncated) = wait(&events, "saved sessions", |e| match e {
        Event::SavedSessions { sessions, truncated } => Some((sessions.clone(), *truncated)),
        _ => None,
    })
    .await;
    assert_eq!((sessions.len(), truncated), (3, false));
    assert_eq!(sessions[1].name.as_deref(), Some("bebop"));
    client.saved_sessions(false, 1).unwrap();
    assert_eq!(fake.next_control().await["named_only"], false);
    let truncated = wait(&events, "saved sessions", |e| match e {
        Event::SavedSessions { truncated, .. } => Some(*truncated),
        _ => None,
    })
    .await;
    assert!(truncated);
    client.shutdown().unwrap();
}

/// Feature gating: without `saved_sessions` in `Welcome.features` no
/// `saved_sessions` frame ever goes out: `saved_sessions()` says
/// `Unsupported`, and a raw `Command::SavedSessions` (the thread's own
/// check) answers `Error::Unsupported`.
#[tokio::test]
async fn without_the_feature_no_saved_sessions_frame_is_sent() {
    use synaps_client::{Command, RequestError};
    let mut fake = Fake::start("nosaved").await;
    let (client, events) = fake.client();
    assert!(!connected(&events).await.iter().any(|f| f == "saved_sessions"));
    assert_eq!(client.saved_sessions(true, 200), Err(RequestError::Unsupported { feature: "saved_sessions" }));
    client.send(Command::SavedSessions { named_only: true, limit: 200 }).unwrap();
    let err = wait(&events, "unsupported", |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(err, Error::Unsupported { feature: "saved_sessions" });
    // A marker: everything sent before it has been read by the fake.
    client.list_sessions().unwrap();
    assert_eq!(fake.next_control().await["type"], "sessions", "no saved_sessions frame may precede it");
    client.shutdown().unwrap();
    assert_eq!(fake.next_control().await["type"], "bye");
}

/// Before the control connection is up the features are unknown:
/// `NotConnected`, nothing queued.
#[tokio::test]
async fn saved_sessions_before_connecting_is_not_connected() {
    let dir = std::env::temp_dir().join(format!("synaps-fake-{}-nobody", std::process::id()));
    let (client, _events) =
        Client::spawn(ClientOptions::new(Endpoint::Socket(dir.join("none.sock")), Identity::new("tester").unwrap())).unwrap();
    assert_eq!(client.saved_sessions(true, 200), Err(synaps_client::RequestError::NotConnected));
    assert!(client.features().is_none());
    client.shutdown().unwrap();
}

async fn rename_reply_event(events: &async_channel::Receiver<Event>) -> Event {
    wait(events, "rename reply", |e| matches!(e, Event::Renamed { .. } | Event::RenameFailed(_)).then(|| e.clone())).await
}

/// `rename`: checked before sending, `expect: null` unless forced, every
/// reply typed: `Renamed`, `Named` (with `current_name`) then a forced
/// replace, `Taken` (with `taken_by`), and `Refused{UnknownSession}` →
/// `NotLive` (the daemon closes; the control connection comes back).
#[tokio::test]
async fn rename_validates_and_types_every_reply() {
    use synaps_client::{RenameRefusal, RequestError};
    let mut fake = Fake::start("rename").await;
    let (client, events) = fake.client();
    connected(&events).await;

    assert!(matches!(client.rename(SID2, Some("Not Valid"), false), Err(RequestError::InvalidName(_))));
    client.rename(SID2, Some("fresh"), false).unwrap();
    assert_eq!(fake.next_control().await, json!({"type": "rename", "session_id": SID2, "name": "fresh", "expect": null}));
    let ev = rename_reply_event(&events).await;
    assert!(matches!(&ev, Event::Renamed { session_id, name: Some(n), .. } if session_id == SID2 && n == "fresh"), "{ev:?}");

    client.rename(SID, Some("other"), false).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!((f.reason, f.current_name.as_deref()), (RenameRefusal::Named, Some("named-one")));
    client.rename(SID, Some("other"), true).unwrap();
    let frame = fake.next_control().await;
    assert!(frame.get("expect").is_none(), "force replaces any name: {frame}");
    assert!(matches!(rename_reply_event(&events).await, Event::Renamed { .. }));

    client.rename(SID2, Some("taken-one"), false).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!((f.reason, f.taken_by.as_deref()), (RenameRefusal::Taken, Some("20261028-045932-a1b2")));

    client.rename("20261113-999999-dead", Some("ghost"), false).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!(f.reason, RenameRefusal::NotLive);
    // Clearing a name: `name: null`, no precondition.
    connected(&events).await;
    client.rename(SID, None, false).unwrap();
    assert_eq!(fake.next_control().await, json!({"type": "rename", "session_id": SID, "name": null}));
    client.shutdown().unwrap();
}

/// A daemon that predates `Rename` answers `Error{unknown variant}`: that
/// is `RenameRefusal::Unsupported`, and the connection stays usable.
#[tokio::test]
async fn rename_on_an_older_daemon_is_unsupported() {
    let mut fake = Fake::start_with("oldrename", Opts { old: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    connected(&events).await;
    client.rename(SID2, Some("fresh"), false).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!(f.reason, synaps_client::RenameRefusal::Unsupported);
    assert!(f.message.contains("unknown variant"));
    client.list_sessions().unwrap();
    assert_eq!(fake.next_control().await["type"], "sessions");
    client.shutdown().unwrap();
}

/// `resume`: a create with `continue_session` (no feature needed). The
/// daemon's "no session named" ends it with that reason; a resume whose
/// connection drops after the create is not re-sent but attaches to the
/// session it resumed, by name or journal id.
#[tokio::test]
async fn resume_continues_by_name_and_reports_failures() {
    let mut fake = Fake::start("resume").await;
    let (client, events) = fake.client();
    let seq = client.resume("nope", CreateOptions::new().cwd(Cwd::Path("/tmp".into()))).unwrap();
    let conn = fake.next_attach().await;
    assert_eq!(conn.attach["attach"], "create");
    assert_eq!(conn.attach["config"]["continue_session"], "nope");
    assert_eq!(conn.attach["config"]["cwd"], "/tmp");
    conn.send(json!({"type": "error", "session_id": null, "message": "create session: no session named 'nope'"})).await;
    let (s, reason) = wait(&events, "detached", |e| match e {
        Event::Detached { attach_seq, reason, .. } => Some((*attach_seq, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(s, seq);
    assert!(reason.contains("no session named 'nope'"), "{reason}");

    // By name: the retry after a drop attaches to the session of that name.
    client.resume("named-one", CreateOptions::new()).unwrap();
    drop(fake.next_attach().await);
    let again = fake.next_attach().await;
    assert_eq!((again.attach["attach"].as_str(), again.attach["session_id"].as_str()), (Some("existing"), Some(SID)));
    again.send(attached_frame(4)).await;
    wait(&events, "attached", |e| matches!(e, Event::Attached { .. }).then_some(())).await;

    // By journal id (a compacted successor's `id` differs from it).
    client.resume("20261101-000000-bbbb", CreateOptions::new()).unwrap();
    drop(fake.next_attach().await);
    let again = fake.next_attach().await;
    assert_eq!(again.attach["session_id"], SID2);
    client.shutdown().unwrap();
}
