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
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn attached_frame(client: u64) -> Value {
    let mut a: Value = serde_json::from_str(fixture("attached_history.json").trim()).unwrap();
    a["client"] = json!(client);
    a["input_owner"] = json!(client);
    a
}

fn welcome() -> Value {
    json!({"type": "welcome", "protocol_version": 3, "daemon_version": "0.0.0-fake", "pid": 1, "profile": null,
           "sessions": [{"id": SID, "model": "m", "lifecycle": "live", "name": "named-one"}],
           "progressive_tool_disclosure": false, "generation": 1})
}

impl Fake {
    async fn start(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("synaps-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("d.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (atx, attaches) = mpsc::unbounded_channel();
        let (ctx, accepted) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (r, w) = stream.into_split();
                let w: Writer = Arc::new(Mutex::new(w));
                let _ = ctx.send(Arc::downgrade(&w));
                let atx = atx.clone();
                tokio::spawn(async move {
                    let mut r = BufReader::new(r).lines();
                    let Ok(Some(hello)) = r.next_line().await else { return };
                    let hello: Value = serde_json::from_str(&hello).unwrap();
                    write(&w, welcome()).await;
                    while let Ok(Some(line)) = r.next_line().await {
                        let f: Value = serde_json::from_str(&line).unwrap();
                        match f["type"].as_str() {
                            Some("sessions") => write(&w, json!({"type": "session_list", "sessions": []})).await,
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
        Fake { path, attaches, accepted }
    }

    fn client(&self) -> (Client, async_channel::Receiver<Event>) {
        Client::spawn(ClientOptions::new(Endpoint::Socket(self.path.clone()), Identity::new("tester").unwrap())).unwrap()
    }

    async fn next_attach(&mut self) -> Conn {
        tokio::time::timeout(T, self.attaches.recv()).await.expect("no attach").unwrap()
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
