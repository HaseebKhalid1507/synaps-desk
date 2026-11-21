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
    /// Lists `saved_sessions` but cannot read the sessions dir: `Error`.
    /// With `pages`: every page is a `catalog_page` with `error`.
    saved_error: bool,
    /// `Welcome.features` lists `session_page` and answers `catalog_page`
    /// from [`catalog`].
    pages: bool,
    /// With `pages`: every page reply is a `catalog_page` that does not
    /// decode (`sessions` not a list).
    malformed: bool,
    /// With `pages`: before each page reply, a req-less `Error` that is not
    /// the page's; a `before` of `"bad"` is answered `malformed frame`.
    noise: bool,
}

/// The fake's saved sessions, newest first: five rows, the third named.
fn catalog() -> Vec<Value> {
    (0..5)
        .map(|k| {
            let id = format!("20261110-00000{k}-c{k:03}");
            json!({"id": id, "journal_id": id, "lifecycle": "saved", "model": "m", "messages": k,
                   "name": if k == 2 { json!("kept") } else { Value::Null },
                   "updated_at": format!("2026-11-12T0{}:00:00Z", 9 - k)})
        })
        .collect()
}

/// The fake's `CatalogPage`: rows strictly after `before` in the order,
/// at most `limit`, `next` if any are left; and an undecodable row on
/// every page (skipped by the client, the page kept).
fn catalog_reply(f: &Value) -> Value {
    let key = |v: &Value| (v["updated_at"].as_str().unwrap_or_default().to_string(), v["id"].as_str().unwrap_or_default().to_string());
    let rows: Vec<Value> = catalog()
        .into_iter()
        .filter(|r| !f["named_only"].as_bool().unwrap_or(false) || r["name"].is_string())
        .filter(|r| f["before"].is_null() || key(r) < key(&f["before"]))
        .collect();
    let limit = f["limit"].as_u64().unwrap_or(50) as usize;
    let more = rows.len() > limit;
    let mut page: Vec<Value> = rows.into_iter().take(limit).collect();
    let next = if more { page.last().map(|r| json!({"updated_at": r["updated_at"], "id": r["id"]})) } else { None };
    page.push(json!({"id": 5, "lifecycle": "saved"}));
    json!({"type": "catalog_page", "req": f["req"], "sessions": page, "next": next})
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
    let mut features: Vec<&str> = if opts.saved { vec!["owned_sessions", "saved_sessions"] } else { vec!["owned_sessions"] };
    if opts.pages {
        features.push("session_page");
    }
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
                            Some("saved_sessions") if opts.saved && opts.saved_error => {
                                write(
                                    &w,
                                    json!({"type": "error", "session_id": null,
                                    "message": "saved sessions: Permission denied (os error 13)"}),
                                )
                                .await;
                            }
                            Some("saved_sessions") if opts.saved => {
                                let mut list: Value = serde_json::from_str(fixture("saved_session_list.json").trim()).unwrap();
                                list["truncated"] = json!(f["limit"] == 1);
                                write(&w, list).await;
                            }
                            Some("saved_sessions") => {
                                write(
                                    &w,
                                    json!({"type": "error", "session_id": null,
                                    "message": "malformed frame: unknown variant `saved_sessions`"}),
                                )
                                .await;
                            }
                            Some("catalog_page") if opts.pages && opts.noise && f["before"] == "bad" => {
                                write(&w, json!({"type": "error", "session_id": null, "message": "malformed frame: invalid type: string \"bad\""}))
                                    .await;
                            }
                            Some("catalog_page") if opts.pages && opts.noise => {
                                write(&w, json!({"type": "error", "session_id": null, "message": "config reload: theme file unreadable"}))
                                    .await;
                                write(&w, catalog_reply(&f)).await;
                            }
                            Some("catalog_page") if opts.pages && opts.malformed => {
                                write(&w, json!({"type": "catalog_page", "req": f["req"], "sessions": "not a list"})).await;
                            }
                            Some("catalog_page") if opts.pages && opts.saved_error => {
                                write(&w, json!({"type": "catalog_page", "req": f["req"], "sessions": [], "error": "saved sessions: Permission denied (os error 13)"}))
                                    .await;
                            }
                            Some("catalog_page") if opts.pages => {
                                // A reply to a page nobody asked for comes first: the
                                // client must not pass it on.
                                if f["req"] == 1 {
                                    write(&w, json!({"type": "catalog_page", "req": 999, "sessions": catalog(), "next": null})).await;
                                }
                                write(&w, catalog_reply(&f)).await;
                            }
                            Some("catalog_page") => {
                                write(
                                    &w,
                                    json!({"type": "error", "session_id": null,
                                    "message": "malformed frame: unknown variant `catalog_page`"}),
                                )
                                .await;
                            }
                            Some("rename") => match rename_reply(opts, &f) {
                                Some(reply) => write(&w, reply).await,
                                None => {
                                    write(
                                        &w,
                                        json!({"type": "refused", "reason": {"reason": "unknown_session"},
                                        "message": format!("no live session {}", f["session_id"])}),
                                    )
                                    .await;
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

    let resumed =
        vec![json!({"content": "What is 2+2?", "role": "user"}), json!({"content": [{"text": "4", "type": "text"}], "role": "assistant"})];
    conn.event(10, json!({"ev": "resumed", "id": 7, "old_id": SID, "new_id": "r", "via": null, "clamp_notice": null})).await;
    conn.event(11, json!({"ev": "conversation", "digest": {"messages_len": 2, "messages_hash": messages_hash(&resumed), "cost": 0.1}}))
        .await;
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

/// A storm of daemon reloads (the desktop's motion.md §5.3): each
/// goodbye is a planned handover, so the control connection comes back on
/// the client's short fixed retry every time, not on a doubling backoff
/// (it was 100, 200, 400, 800, 1600 ms: the fifth reload took 1.6 s).
#[tokio::test]
async fn a_reload_storm_reconnects_on_a_fixed_schedule() {
    let mut fake = Fake::start("storm").await;
    let (client, events) = fake.client();
    wait(&events, "connected", |e| matches!(e, Event::Status(Status::Connected { .. })).then_some(())).await;
    let mut control = tokio::time::timeout(T, fake.accepted.recv()).await.unwrap().unwrap().upgrade().unwrap();
    let mut took = Vec::new();
    for g in 2..7 {
        let t0 = tokio::time::Instant::now();
        write(&control, json!({"type": "bye", "reason": {"bye": "reloading", "generation": g, "retry_after_ms": 50}})).await;
        wait(&events, "reconnecting", |e| match e {
            Event::Status(Status::Reconnecting { why }) => {
                assert_eq!(why, synaps_client::RELOADING);
                Some(())
            }
            _ => None,
        })
        .await;
        wait(&events, "connected again", |e| matches!(e, Event::Status(Status::Connected { .. })).then_some(())).await;
        took.push(t0.elapsed());
        control = tokio::time::timeout(T, fake.accepted.recv()).await.unwrap().unwrap().upgrade().unwrap();
    }
    assert!(
        took.iter().all(|d| *d >= synaps_client::RELOAD_RETRY && *d < synaps_client::RELOAD_RETRY * 2),
        "each reload back in about RELOAD_RETRY: {took:?}"
    );
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
    let (sessions, truncated, rejected) = wait(&events, "saved sessions", |e| match e {
        Event::SavedSessions { sessions, truncated, rejected } => Some((sessions.clone(), *truncated, *rejected)),
        _ => None,
    })
    .await;
    assert_eq!((sessions.len(), truncated, rejected), (3, false, 0));
    assert_eq!(sessions[1].name.as_deref(), Some("bebop"));
    client.saved_sessions(false, 1).unwrap();
    assert_eq!(fake.next_control().await["named_only"], false);
    // The limit is clamped to what the daemon takes (`Option<u32>`, ≤ 1000).
    client.saved_sessions(true, u32::MAX).unwrap();
    assert_eq!(fake.next_control().await["limit"], 1000);
    let truncated = wait(&events, "saved sessions", |e| match e {
        Event::SavedSessions { truncated, .. } => Some(*truncated),
        _ => None,
    })
    .await;
    assert!(truncated);
    client.shutdown().unwrap();
}

/// M4: a daemon `Error` answering `saved_sessions` (an unreadable sessions
/// dir) is `Event::SavedSessionsFailed`, not a generic error, and the
/// connection stays usable.
#[tokio::test]
async fn a_saved_sessions_error_is_its_own_event() {
    let mut fake = Fake::start_with("savederr", Opts { saved: true, saved_error: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    connected(&events).await;
    client.saved_sessions(true, 200).unwrap();
    fake.next_control().await;
    let ev = wait(&events, "saved failure", |e| match e {
        Event::SavedSessionsFailed { reason } => Some(Ok(reason.clone())),
        Event::Error { error, .. } => Some(Err(error.clone())),
        _ => None,
    })
    .await;
    assert_eq!(ev, Ok("saved sessions: Permission denied (os error 13)".into()));
    client.list_sessions().unwrap();
    assert_eq!(fake.next_control().await["type"], "sessions");
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

/// `rename`: checked before sending, always compare-and-set, every reply
/// typed: `Renamed`, `Named` (with `current_name`) then a replace that
/// names what it replaces (`expect: "named-one"`), a stale replace refused
/// again, `Taken` (with `taken_by`), and `Refused{UnknownSession}` →
/// `NotLive` (the daemon closes; the control connection comes back).
#[tokio::test]
async fn rename_validates_and_types_every_reply() {
    use synaps_client::{Expect, RenameRefusal, RequestError};
    let mut fake = Fake::start("rename").await;
    let (client, events) = fake.client();
    connected(&events).await;

    assert!(matches!(client.rename(SID2, Some("Not Valid"), Expect::Unnamed), Err(RequestError::InvalidName(_))));
    client.rename(SID2, Some("fresh"), Expect::Unnamed).unwrap();
    assert_eq!(fake.next_control().await, json!({"type": "rename", "session_id": SID2, "name": "fresh", "expect": null}));
    let ev = rename_reply_event(&events).await;
    assert!(matches!(&ev, Event::Renamed { session_id, name: Some(n), .. } if session_id == SID2 && n == "fresh"), "{ev:?}");

    client.rename(SID, Some("other"), Expect::Unnamed).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!((f.reason, f.current_name.as_deref()), (RenameRefusal::Named, Some("named-one")));
    // Replacing a name the user has not seen (renamed meanwhile) is refused.
    client.rename(SID, Some("other"), Expect::Named("kite".into())).unwrap();
    assert_eq!(fake.next_control().await["expect"], "kite");
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!((f.reason, f.current_name.as_deref()), (RenameRefusal::Named, Some("named-one")));
    client.rename(SID, Some("other"), Expect::Named("named-one".into())).unwrap();
    let frame = fake.next_control().await;
    assert_eq!(frame, json!({"type": "rename", "session_id": SID, "name": "other", "expect": "named-one"}));
    assert!(matches!(rename_reply_event(&events).await, Event::Renamed { .. }));

    client.rename(SID2, Some("taken-one"), Expect::Unnamed).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!((f.reason, f.taken_by.as_deref()), (RenameRefusal::Taken, Some("20261028-045932-a1b2")));

    client.rename("20261113-999999-dead", Some("ghost"), Expect::Unnamed).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!(f.reason, RenameRefusal::NotLive);
    // Clearing a name: `name: null`, only if it is still the one seen.
    connected(&events).await;
    client.rename(SID, None, Expect::Named("named-one".into())).unwrap();
    assert_eq!(fake.next_control().await, json!({"type": "rename", "session_id": SID, "name": null, "expect": "named-one"}));
    client.shutdown().unwrap();
}

/// A daemon that predates `Rename` answers `Error{unknown variant}`: that
/// is `RenameRefusal::Unsupported`, and the connection stays usable.
#[tokio::test]
async fn rename_on_an_older_daemon_is_unsupported() {
    let mut fake = Fake::start_with("oldrename", Opts { old: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    connected(&events).await;
    client.rename(SID2, Some("fresh"), synaps_client::Expect::Unnamed).unwrap();
    fake.next_control().await;
    let Event::RenameFailed(f) = rename_reply_event(&events).await else { panic!() };
    assert_eq!(f.reason, synaps_client::RenameRefusal::Unsupported);
    assert!(f.message.contains("unknown variant"));
    client.list_sessions().unwrap();
    assert_eq!(fake.next_control().await["type"], "sessions");
    client.shutdown().unwrap();
}

/// The daemon's real create errors (Synaps `origin/dev`):
/// `daemon/conn.rs`: `format!("create session: {e}")`; `engine/setup.rs`:
/// `"Failed to load session '{}': {}"` as `RuntimeError::Tool` (`"Tool
/// execution failed: {0}"`); `core/session.rs`: `"no session matching '{}'"`.
const NO_SUCH: &str = "create session: Tool execution failed: Failed to load session 'nope': no session matching 'nope'";

/// `resume`: a create with `continue_session` (no feature needed). The
/// daemon's "no session matching" ends it with that reason. A resume
/// whose connection drops after the create is sent again, not looked up
/// (the first may still be loading its journal): the daemon attaches a
/// live target instead of making a second one.
#[tokio::test]
async fn resume_continues_and_is_resent_after_a_drop() {
    let mut fake = Fake::start("resume").await;
    let (client, events) = fake.client();
    let seq = client.resume("nope", CreateOptions::new().cwd(Cwd::Path("/tmp".into()))).unwrap();
    let conn = fake.next_attach().await;
    assert_eq!(conn.attach["attach"], "create");
    assert_eq!(conn.attach["config"]["continue_session"], "nope");
    assert_eq!(conn.attach["config"]["cwd"], "/tmp");
    conn.send(json!({"type": "error", "session_id": null, "message": NO_SUCH})).await;
    let (s, reason) = wait(&events, "detached", |e| match e {
        Event::Detached { attach_seq, reason, .. } => Some((*attach_seq, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!((s, reason.as_str()), (seq, NO_SUCH));

    // A compacted predecessor's id, not in `Welcome.sessions` at all: the
    // retry sends the same create again.
    client.resume("20261001-000000-0ld1", CreateOptions::new().cwd(Cwd::Path("/tmp".into()))).unwrap();
    drop(fake.next_attach().await);
    let again = fake.next_attach().await;
    assert_eq!(again.attach["attach"], "create");
    assert_eq!(again.attach["config"]["continue_session"], "20261001-000000-0ld1");
    assert_eq!(again.attach["config"]["cwd"], "/tmp");
    again.send(attached_frame(4)).await;
    wait(&events, "attached", |e| matches!(e, Event::Attached { .. }).then_some(())).await;
    client.shutdown().unwrap();
}

/// M5: a resume that takes longer than a plain create's 5 s (a long
/// journal loads first) is waited for on the same connection.
#[tokio::test]
async fn a_slow_resume_is_not_given_up_at_five_seconds() {
    let mut fake = Fake::start("slowresume").await;
    let (client, events) = fake.client();
    let seq = client.resume("20261110-202033-e5f6", CreateOptions::new()).unwrap();
    let conn = fake.next_attach().await;
    tokio::time::sleep(Duration::from_millis(6000)).await;
    conn.send(attached_frame(5)).await;
    let s = wait(&events, "attached", |e| match e {
        Event::Attached { attach_seq, reconnect, .. } => Some((*attach_seq, *reconnect)),
        _ => None,
    })
    .await;
    assert_eq!(s, (seq, false));
    assert!(fake.attaches.try_recv().is_err(), "no second attach");
    client.shutdown().unwrap();
}

/// H1: the saved folder goes to the daemon as is. If the daemon says it is
/// gone on its machine (its real `Error`, then `Bye`), the create goes out
/// once more in the fallback (`~`) and `CwdFallback` says so; without a
/// fallback the refusal ends the resume.
#[tokio::test]
async fn a_missing_folder_falls_back_to_home_once() {
    let mut fake = Fake::start("gonecwd").await;
    let (client, events) = fake.client();
    let gone = "/home/u/Projects/gone";
    let seq = client.resume("20261110-202033-e5f6", CreateOptions::new().cwd(Cwd::Path(gone.into())).fallback_cwd(Cwd::Home)).unwrap();
    let first = fake.next_attach().await;
    assert_eq!(first.attach["config"]["cwd"], gone);
    // daemon/conn.rs: `format!("cwd must be an absolute existing directory: {}", cwd.display())`.
    first
        .send(json!({"type": "error", "session_id": null, "message": format!("cwd must be an absolute existing directory: {gone}")}))
        .await;
    first.send(json!({"type": "bye", "reason": null})).await;
    let second = fake.next_attach().await;
    assert_eq!(second.attach["attach"], "create");
    assert_eq!(second.attach["config"]["continue_session"], "20261110-202033-e5f6");
    let home = Cwd::Home.resolve().unwrap();
    assert_eq!(second.attach["config"]["cwd"], json!(home));
    let fb = wait(&events, "fallback", |e| match e {
        Event::CwdFallback { attach_seq, missing, cwd } => Some((*attach_seq, missing.clone(), cwd.clone())),
        _ => None,
    })
    .await;
    assert_eq!(fb, (seq, gone.to_string(), Some(home)));
    second.send(attached_frame(6)).await;
    wait(&events, "attached", |e| matches!(e, Event::Attached { attach_seq, .. } if *attach_seq == seq).then_some(())).await;

    // Once only: a second refusal (or no fallback) ends it.
    let seq = client.resume("x", CreateOptions::new().cwd(Cwd::Path(gone.into()))).unwrap();
    let c = fake.next_attach().await;
    c.send(json!({"type": "error", "session_id": null, "message": format!("cwd must be an absolute existing directory: {gone}")})).await;
    let reason = wait(&events, "detached", |e| match e {
        Event::Detached { attach_seq, reason, .. } if *attach_seq == seq => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert!(reason.starts_with(synaps_client::MISSING_CWD), "{reason}");
    client.shutdown().unwrap();
}

/// A pending prompt reaches the consumer, and `answer` sends the daemon's
/// `Answer { prompt_id, value }`: a confirm's "y" / "n", a cancel as `null`.
#[tokio::test]
async fn prompts_are_answered_on_the_wire() {
    let mut fake = Fake::start("prompt").await;
    let (client, events) = fake.client();
    client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    conn.event(
        30,
        json!({"ev": "prompt", "request": {"id": 7, "kind": "confirm", "title": "Confirm tool call",
        "prompt": "bash: cargo test", "raised_at": "2026-11-13T07:34:27Z"}}),
    )
    .await;
    let (id, title) = wait(&events, "prompt", |e| match e {
        Event::Session { event: synaps_client::SessionEvent::Prompt { request }, .. } => Some((request.id, request.title.clone())),
        _ => None,
    })
    .await;
    assert_eq!((id, title.as_str()), (7, "Confirm tool call"));
    client.answer(7, Some("y".into())).unwrap();
    let cmd = conn.recv_cmd().await;
    assert_eq!(cmd, json!({"cmd": "answer", "prompt_id": 7, "value": "y"}));
    client.answer(8, None).unwrap();
    assert_eq!(conn.recv_cmd().await, json!({"cmd": "answer", "prompt_id": 8, "value": null}));
    conn.event(31, json!({"ev": "prompt_resolved", "prompt_id": 7})).await;
    wait(&events, "resolved", |e| {
        matches!(e, Event::Session { event: synaps_client::SessionEvent::PromptResolved { prompt_id: 7 }, .. }).then_some(())
    })
    .await;
    client.shutdown().unwrap();
}

/// An answer is tied to the connection its prompt came on: once the
/// session has reconnected (a restarted daemon numbers its prompts from 1
/// again), `answer_on` with the old connection is dropped and refused,
/// never sent, whether it was queued during the reconnect or after it.
#[tokio::test]
async fn answers_for_an_earlier_connection_are_never_sent() {
    let mut fake = Fake::start("answer-on").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let conn = fake.next_attach().await;
    conn.send(attached_frame(2)).await;
    let first = wait(&events, "attached", |e| match e {
        Event::Attached { attached, .. } => Some(attached.connection),
        _ => None,
    })
    .await;
    assert_ne!(first, 0);
    // The daemon goes away with prompt #1 pending.
    drop(conn);
    wait(&events, "reattaching", |e| matches!(e, Event::Reattaching { attach_seq, .. } if *attach_seq == seq).then_some(())).await;
    let refused = |e: &Event| match e {
        Event::Error { error: Error::Refused { command: Some(c), reason }, .. } if c == "answer" => Some(reason.clone()),
        _ => None,
    };
    // Queued while reconnecting: refused.
    client.answer_on(first, 1, Some("y".into())).unwrap();
    wait(&events, "refused while reconnecting", refused).await;
    let mut again = fake.next_attach().await;
    again.send(attached_frame(3)).await;
    let second = wait(&events, "re-attached", |e| match e {
        Event::Attached { attached, reconnect: true, .. } => Some(attached.connection),
        _ => None,
    })
    .await;
    assert!(second > first, "{first} then {second}");
    // Queued after the reconnect, aimed at the old connection (a click on
    // the old card the UI had not replaced yet): dropped, refused.
    client.answer_on(first, 1, Some("y".into())).unwrap();
    let why = wait(&events, "refused after the reconnect", refused).await;
    assert!(why.contains("earlier connection"), "{why}");
    // The new connection's own answer goes out, and is the first command
    // the daemon sees: the stale "y" never reached it.
    client.answer_on(second, 1, Some("n".into())).unwrap();
    assert_eq!(again.recv_cmd().await, json!({"cmd": "answer", "prompt_id": 1, "value": "n"}));
    client.shutdown().unwrap();
    assert_eq!(again.recv_cmd().await, json!({"cmd": "detach", "client": 3}));
}

/// Worker watch: a page comes back as `Event::SubagentWatch` for the
/// worker it was asked for (another client's query result never does),
/// the session's "no subagent" is `Gone`, a worker steer goes out in the
/// daemon's shape, and a daemon from before v0.11.0 (`malformed frame:
/// unknown variant`) answers `Unsupported` for the watch and a refusal for
/// the steer, on a connection that stays up.
#[tokio::test]
async fn worker_watch_pages_gone_steer_and_an_older_daemon() {
    use synaps_client::{WatchEvent, WatchReply};
    let mut fake = Fake::start("watch").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    let reply = |events| async move {
        wait(events, "watch reply", |e| match e {
            Event::SubagentWatch { attach_seq, handle_id, since, reply, .. } => {
                Some((*attach_seq, format!("{handle_id}@{since}"), reply.clone()))
            }
            _ => None,
        })
        .await
    };

    client.watch_subagent("sa_3", 0).unwrap();
    let cmd = conn.recv_cmd().await;
    assert_eq!(
        (cmd["cmd"].as_str(), &cmd["query"]),
        (Some("query"), &json!({"query": "subagent_watch", "handle_id": "sa_3", "since": 0, "tool_queue": true}))
    );
    let id = cmd["id"].as_u64().unwrap();
    assert!(id < synaps_client::protocol::RESERVED_QUERY_ID_BASE);
    // Another client's result first (ids are per client): not ours.
    conn.event(30, json!({"ev": "query_result", "id": id ^ 1, "value": {"handle_id": "sa_9", "events": []}})).await;
    conn.event(
        31,
        json!({"ev": "query_result", "id": id, "value": {"handle_id": "sa_3", "subagent_id": 3, "agent_name": "sleeper",
        "status": "running", "steerable": true, "next_seq": 2, "truncated": false, "more": true,
        "events": [{"kind": "text", "text": "hi"}, {"kind": "tool_queued", "tool_id": "t"}], "at_ms": [1, 2], "now_ms": 5}}),
    )
    .await;
    let (s, h, r) = reply(&events).await;
    let WatchReply::Page(page) = r else { panic!("{r:?}") };
    assert_eq!((s, h.as_str(), page.next_seq, page.more, page.events.len()), (seq, "sa_3@0", 2, true, 2));
    assert_eq!(page.events[1], WatchEvent::ToolQueued { tool_id: "t".into() });

    client.watch_subagent("sa_9", 4).unwrap();
    let id = conn.recv_cmd().await["id"].as_u64().unwrap();
    conn.event(32, json!({"ev": "query_result", "id": id, "value": {"error": "no subagent sa_9 in this session"}})).await;
    assert_eq!(reply(&events).await.2, WatchReply::Gone("no subagent sa_9 in this session".into()));

    client.steer_subagent("sa_3", "stop after the next step").unwrap();
    let cmd = conn.recv_cmd().await;
    assert_eq!(cmd, json!({"cmd": "subagent_steer", "handle_id": "sa_3", "text": "stop after the next step"}));

    // An older daemon: neither decodes; the session connection survives.
    client.watch_subagent("sa_3", 2).unwrap();
    let _ = conn.recv_cmd().await;
    conn.send(json!({"type": "error", "session_id": SID,
        "message": "malformed frame: unknown variant `subagent_watch`, expected one of `messages`, `status`"}))
        .await;
    assert_eq!(reply(&events).await, (seq, "sa_3@2".to_string(), WatchReply::Unsupported));
    client.steer_subagent("sa_3", "x").unwrap();
    let _ = conn.recv_cmd().await;
    conn.send(json!({"type": "error", "session_id": SID,
        "message": "malformed frame: unknown variant `subagent_steer`, expected one of `submit`, `steer`"}))
        .await;
    let err = wait(&events, "steer refusal", |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(err, Error::Refused { command: Some("subagent_steer".into()), reason: synaps_client::STEER_UNSUPPORTED.into() });
    client.submit("still here").unwrap();
    assert_eq!(conn.recv_cmd().await["text"], "still here");
    client.shutdown().unwrap();
}

/// Cancelling a worker: `/workers cancel sa_N` as the engine command the
/// daemon (v0.11.1 `actor_cmds.rs`) reads, its `QueryResult` (by id: another
/// client's never counts) as `Event::WorkerCancel`, the daemon's error and a
/// non-owner's refusal as `Err`, and a cancel in flight when the
/// connection drops as `Err` too, never lost.
#[tokio::test]
async fn cancel_worker_wire_shape_and_replies() {
    let mut fake = Fake::start("cancel").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    let reply = |events| async move {
        wait(events, "cancel reply", |e| match e {
            Event::WorkerCancel { attach_seq, handle_id, result } => Some((*attach_seq, handle_id.clone(), result.clone())),
            _ => None,
        })
        .await
    };

    client.cancel_worker("sa_3").unwrap();
    let cmd = conn.recv_cmd().await;
    let id = cmd["id"].as_u64().unwrap();
    assert!(id < synaps_client::protocol::RESERVED_QUERY_ID_BASE);
    assert_eq!(cmd, json!({"cmd": "engine_command", "id": id, "name": "workers", "arg": "cancel sa_3"}));
    // Another client's result (broadcast) first: not ours.
    conn.event(40, json!({"ev": "query_result", "id": id ^ 1, "value": {"kind": "output", "text": "Cancelled sa_9"}})).await;
    let text = "Cancelled sa_3 (they stop at their next step; no completion wake is sent).";
    conn.event(41, json!({"ev": "query_result", "id": id, "value": {"kind": "output", "text": text}})).await;
    assert_eq!(reply(&events).await, (seq, "sa_3".into(), Ok(text.into())));

    // The daemon's own error: a worker that has finished.
    client.cancel_worker("sa_1").unwrap();
    let id = conn.recv_cmd().await["id"].as_u64().unwrap();
    let why = "sa_1 is already completed — nothing to cancel";
    conn.event(42, json!({"ev": "query_result", "id": id, "value": {"kind": "error", "text": why}})).await;
    assert_eq!(reply(&events).await, (seq, "sa_1".into(), Err(why.into())));

    // Not the input owner: the session refuses the engine command.
    client.cancel_worker("sa_4").unwrap();
    let _ = conn.recv_cmd().await;
    conn.event(43, json!({"ev": "refused", "client": 9, "command": "engine_command", "reason": "not you"})).await;
    conn.event(44, json!({"ev": "refused", "client": 2, "command": "engine_command", "reason": "input is owned by client #1"})).await;
    assert_eq!(reply(&events).await, (seq, "sa_4".into(), Err("refused: input is owned by client #1".into())));

    // In flight when the connection drops: answered, as not known.
    client.cancel_worker("sa_5").unwrap();
    let _ = conn.recv_cmd().await;
    drop(conn);
    let (s, h, r) = reply(&events).await;
    assert_eq!((s, h.as_str()), (seq, "sa_5"));
    assert!(r.is_err_and(|e| e.contains("dropped")));
    // Reconnecting: not sent, said so.
    client.cancel_worker("sa_6").unwrap();
    let (_, h, r) = reply(&events).await;
    assert_eq!(h, "sa_6");
    assert!(r.is_err_and(|e| e.contains("not connected")));
    client.shutdown().unwrap();
}

/// One worker only: a handle that is not `sa_N` (`all` would cancel every
/// running worker) is answered `Err` and never sent. A cancel in flight as
/// the session ends, or as it is detached, is answered too.
#[tokio::test]
async fn cancel_worker_sends_one_handle_and_always_answers() {
    let mut fake = Fake::start("cancel-end").await;
    let (client, events) = fake.client();
    let reply = |events| async move {
        wait(events, "cancel reply", |e| match e {
            Event::WorkerCancel { attach_seq, handle_id, result } => Some((*attach_seq, handle_id.clone(), result.clone())),
            _ => None,
        })
        .await
    };
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    for bad in ["all", "sa_", "sa_3 sa_4", "3", "sa_x", " sa_3"] {
        client.cancel_worker(bad).unwrap();
        let (s, h, r) = reply(&events).await;
        assert_eq!((s, h.as_str()), (seq, bad));
        assert!(r.is_err_and(|e| e.contains("not a worker's handle")), "{bad}");
    }
    client.cancel_worker("sa_7").unwrap();
    assert_eq!(conn.recv_cmd().await["arg"], "cancel sa_7", "nothing before it went out");
    conn.event(30, json!({"ev": "ended", "reason": "client_quit"})).await;
    let (s, h, r) = reply(&events).await;
    assert_eq!((s, h.as_str()), (seq, "sa_7"));
    assert!(r.is_err_and(|e| e.contains("session ended")));

    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    client.cancel_worker("sa_8").unwrap();
    assert_eq!(conn.recv_cmd().await["arg"], "cancel sa_8");
    client.detach().unwrap();
    let (s, h, r) = reply(&events).await;
    assert_eq!((s, h.as_str()), (seq, "sa_8"));
    assert!(r.is_err_and(|e| e.contains("detached")));
    client.shutdown().unwrap();
}

/// A slash command (`/compact`, `/context` ...): the engine command the
/// daemon reads, and its `QueryResult` by id as `Event::EngineReply`. The
/// replies are the daemon's own shapes (agent-engine `engine_command_reply`
/// and the commands' texts, identical on v0.11.1 and v0.12 dev): `none` is
/// done (a `/compact`), `output` and `notice` done with their text, `error`
/// and `unhandled` not run, a `/model x` notice naming another model not a
/// switch, a kind this client does not know unknown. A non-owner's refusal
/// goes to the oldest engine command in flight, a worker cancel's or a
/// slash command's; one in flight when the link drops: unknown (it may
/// have run); one sent while not connected: not run.
#[tokio::test]
async fn engine_command_wire_shape_and_replies() {
    use synaps_client::CommandReply::{Done, NotRun, Unknown};
    let mut fake = Fake::start("engine").await;
    let (client, events) = fake.client();
    let seq = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut conn = attached(&mut fake, &events, 2).await;
    let reply = |events| async move {
        wait(events, "engine reply", |e| match e {
            Event::EngineReply { attach_seq, name, reply } => Some((*attach_seq, name.clone(), reply.clone())),
            _ => None,
        })
        .await
    };

    client.engine_command("compact", "").unwrap();
    let cmd = conn.recv_cmd().await;
    let id = cmd["id"].as_u64().unwrap();
    assert!(id < synaps_client::protocol::RESERVED_QUERY_ID_BASE);
    assert_eq!(cmd, json!({"cmd": "engine_command", "id": id, "name": "compact", "arg": ""}));
    conn.event(40, json!({"ev": "query_result", "id": id ^ 1, "value": {"kind": "output", "text": "not ours"}})).await;
    conn.event(41, json!({"ev": "query_result", "id": id, "value": {"kind": "none"}})).await;
    assert_eq!(reply(&events).await, (seq, "compact".into(), Done(String::new())));

    // Each command with its argument, and the daemon's reply as it sends it.
    let model_ok = json!({"kind": "notice", "event": "model_changed", "text": "model → openai/gpt-5.1",
        "model": "openai/gpt-5.1", "clamp": null});
    let model_clamped = json!({"kind": "notice", "event": "model_changed",
        "text": "model → claude-haiku-4-5\nthinking → low (clamped from high: not supported by claude-haiku-4-5)",
        "model": "claude-haiku-4-5", "clamp": {"from": "high", "to": "low"}});
    // `set_model` swallowed the error: the model in use is the old one.
    let model_kept = json!({"kind": "notice", "event": "model_changed", "text": "model → openai/nope-9",
        "model": "claude-opus-4-6", "clamp": null});
    const BUDGET_USAGE: &str = "usage: /budget [status] | /budget time <duration> (positive whole seconds/minutes/hours, e.g. 30m or 4h; maximum 24h) | /budget time off";
    let workers = "Workers (2):\n  sa_1  scout  running  0m\n  sa_2  wren  completed  3m";
    let budget = "Turn budget (configured for future turns, not live usage):\n  time: 1800s\n  provider rounds: off";
    let cases: Vec<(&str, &str, Value, synaps_client::CommandReply)> = vec![
        ("compact", "keep the auth bits", json!({"kind": "none"}), Done(String::new())),
        ("model", "openai/gpt-5.1", model_ok, Done("model → openai/gpt-5.1".into())),
        ("model", "claude-haiku-4-5", model_clamped.clone(), Done(model_clamped["text"].as_str().unwrap().into())),
        ("model", "openai/nope-9", model_kept, NotRun("not switched: the model is still claude-opus-4-6".into())),
        (
            "context",
            "",
            json!({"kind": "output", "text": "Context: 41 messages\nsystem 12 KB"}),
            Done("Context: 41 messages\nsystem 12 KB".into()),
        ),
        ("context", "auto x 2", json!({"kind": "error", "text": "invalid pressure tokens"}), NotRun("invalid pressure tokens".into())),
        (
            "trace",
            "next",
            json!({"kind": "output", "text": "trace armed for the next request (metadata only, then auto-disabled)"}),
            Done("trace armed for the next request (metadata only, then auto-disabled)".into()),
        ),
        ("budget", "", json!({"kind": "output", "text": budget}), Done(budget.into())),
        ("budget", "time 99h", json!({"kind": "error", "text": BUDGET_USAGE}), NotRun(BUDGET_USAGE.into())),
        ("workers", "", json!({"kind": "output", "text": workers}), Done(workers.into())),
        (
            "workers",
            "cancel sa_9",
            json!({"kind": "error", "text": "no worker sa_9 (see /workers)"}),
            NotRun("no worker sa_9 (see /workers)".into()),
        ),
        (
            "confirms",
            "reset",
            json!({"kind": "output", "text": "Nothing to revoke: no extension had \"Allow all\"."}),
            Done("Nothing to revoke: no extension had \"Allow all\".".into()),
        ),
        ("frobnicate", "", json!({"kind": "unhandled"}), NotRun("the daemon does not run /frobnicate".into())),
        (
            "trace",
            "",
            json!({"kind": "something-new", "text": "?"}),
            Unknown("the daemon answered `something-new`, which this app does not read".into()),
        ),
    ];
    for (k, (name, arg, value, want)) in cases.into_iter().enumerate() {
        client.engine_command(name, arg).unwrap();
        let cmd = conn.recv_cmd().await;
        assert_eq!((cmd["name"].as_str(), cmd["arg"].as_str()), (Some(name), Some(arg)));
        let id = cmd["id"].as_u64().unwrap();
        conn.event(50 + k as u64, json!({"ev": "query_result", "id": id, "value": value})).await;
        assert_eq!(reply(&events).await, (seq, name.into(), want), "{name} {arg}");
    }

    // Refusals in order: the cancel sent first takes the first.
    client.cancel_worker("sa_2").unwrap();
    let _ = conn.recv_cmd().await;
    client.engine_command("context", "").unwrap();
    let _ = conn.recv_cmd().await;
    conn.event(80, json!({"ev": "refused", "client": 2, "command": "engine_command", "reason": "observing"})).await;
    let cancel = wait(&events, "cancel reply", |e| match e {
        Event::WorkerCancel { handle_id, result, .. } => Some((handle_id.clone(), result.clone())),
        _ => None,
    })
    .await;
    assert_eq!(cancel, ("sa_2".into(), Err("refused: observing".into())));
    conn.event(81, json!({"ev": "refused", "client": 2, "command": "engine_command", "reason": "observing"})).await;
    assert_eq!(reply(&events).await, (seq, "context".into(), NotRun("refused: observing".into())));

    // In flight when the link drops: unknown, it may have run.
    client.engine_command("trace", "").unwrap();
    let _ = conn.recv_cmd().await;
    drop(conn);
    let (_, name, r) = reply(&events).await;
    assert_eq!(name, "trace");
    assert_eq!(r, Unknown("the link dropped before the daemon answered".into()));
    // Sent while not connected: never reached the daemon, not run.
    client.engine_command("budget", "").unwrap();
    let (_, name, r) = reply(&events).await;
    assert_eq!(name, "budget");
    assert!(matches!(r, NotRun(e) if e.contains("not connected")));
    client.shutdown().unwrap();
}

/// Saved pages (`session_page`, SynapsCLI watch-sessions PR 4): the
/// request is the pinned frame, each reply comes back as
/// `Event::CatalogPage` with its `req`, and a reply's `next` sent back as
/// `before` pages on to the end. A bad row on every page is counted, the
/// page kept; a reply to a `req` nobody asked for is dropped.
#[tokio::test]
async fn catalog_pages_round_trip_their_cursor() {
    let mut fake = Fake::start_with("pages", Opts { pages: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    assert!(connected(&events).await.iter().any(|f| f == "session_page"));
    assert!(client.supports(synaps_client::FEATURE_SESSION_PAGE));
    let mut before = None;
    let mut ids = Vec::new();
    let mut reqs = Vec::new();
    for round in 0.. {
        assert!(round < 5, "paging never ended");
        let req = client.catalog_page(before.clone(), 2, false).unwrap();
        reqs.push(req);
        let sent = fake.next_control().await;
        let mut want = json!({"type": "catalog_page", "req": req, "limit": 2});
        if let Some(c) = &before {
            want["before"] = serde_json::to_value(c).unwrap();
        }
        assert_eq!(sent, want);
        let (got, sessions, next, rejected) = wait(&events, "a page", |e| match e {
            Event::CatalogPage { req, sessions, next, rejected } => Some((*req, sessions.clone(), next.clone(), *rejected)),
            _ => None,
        })
        .await;
        assert_eq!((got, rejected), (req, 1), "the stray req 999 is never passed on");
        assert!(sessions.iter().all(|s| s.is_saved()));
        ids.extend(sessions.iter().map(|s| s.id.clone()));
        match next {
            Some(c) => before = Some(c),
            None => break,
        }
    }
    let all: Vec<String> = catalog().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, all, "every row once, newest first");
    assert_eq!(reqs, [1, 2, 3]);

    let req = client.catalog_page(None, u32::MAX, true).unwrap();
    assert_eq!(fake.next_control().await, json!({"type": "catalog_page", "req": req, "limit": 200, "named_only": true}));
    let named = wait(&events, "the named page", |e| match e {
        Event::CatalogPage { sessions, next: None, .. } => Some(sessions.iter().map(|s| s.label()).collect::<Vec<_>>()),
        _ => None,
    })
    .await;
    assert_eq!(named, ["kept"]);
    client.shutdown().unwrap();
}

/// A page the daemon could not serve (`catalog_page` with `error`) is
/// `Event::CatalogPageFailed` for that `req`.
#[tokio::test]
async fn a_catalog_page_error_fails_its_req() {
    let mut fake = Fake::start_with("pages-err", Opts { pages: true, saved_error: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    connected(&events).await;
    let req = client.catalog_page(None, 50, false).unwrap();
    fake.next_control().await;
    let (got, reason) = wait(&events, "failed", |e| match e {
        Event::CatalogPageFailed { req, reason } => Some((*req, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(got, req);
    assert!(reason.contains("Permission denied"), "{reason}");
    client.shutdown().unwrap();
}

/// Review F2: a `catalog_page` reply that does not decode fails the oldest
/// page waiting (its `req` cannot be trusted), so nothing waits for good.
#[tokio::test]
async fn an_undecodable_page_fails_the_oldest_page() {
    let mut fake = Fake::start_with("pages-bad", Opts { pages: true, malformed: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    connected(&events).await;
    let req = client.catalog_page(None, 50, false).unwrap();
    fake.next_control().await;
    let (got, reason) = wait(&events, "failed", |e| match e {
        Event::CatalogPageFailed { req, reason } => Some((*req, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(got, req);
    assert!(reason.contains("did not decode"), "{reason}");
    client.shutdown().unwrap();
}

/// Review F5: a req-less `Error` is charged to a page only if it is
/// `malformed frame` (a `before` the daemon could not parse) or names
/// `catalog_page`; any other leaves the page waiting for its reply.
#[tokio::test]
async fn a_reqless_error_fails_a_page_only_if_it_is_the_pages() {
    let mut fake = Fake::start_with("pages-noise", Opts { pages: true, noise: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    connected(&events).await;
    let req = client.catalog_page(None, 50, false).unwrap();
    fake.next_control().await;
    let got = wait(&events, "the page or its failure", |e| match e {
        Event::CatalogPage { req, .. } => Some(Ok(*req)),
        Event::CatalogPageFailed { req, reason } => Some(Err((*req, reason.clone()))),
        _ => None,
    })
    .await;
    assert_eq!(got, Ok(req), "another error is not the page's");
    let bad = synaps_client::CatalogCursor(json!("bad"));
    let req = client.catalog_page(Some(bad), 50, false).unwrap();
    fake.next_control().await;
    let (got, reason) = wait(&events, "failed", |e| match e {
        Event::CatalogPageFailed { req, reason } => Some((*req, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(got, req);
    assert!(reason.starts_with("malformed frame"), "{reason}");
    client.shutdown().unwrap();
}

/// Feature gating: without `session_page` no `catalog_page` frame goes
/// out. `catalog_page()` says `Unsupported`; a raw `Command::CatalogPage`
/// fails its `req` and says `Error::Unsupported`.
#[tokio::test]
async fn without_the_feature_no_catalog_page_frame_is_sent() {
    use synaps_client::Command;
    let mut fake = Fake::start_with("no-pages", Opts { saved: true, ..Default::default() }).await;
    let (client, events) = fake.client();
    assert!(!connected(&events).await.iter().any(|f| f == "session_page"));
    assert_eq!(client.catalog_page(None, 50, false), Err(synaps_client::RequestError::Unsupported { feature: "session_page" }));
    client.send(Command::CatalogPage { req: 41, before: None, limit: 50, named_only: false }).unwrap();
    let failed = wait(&events, "failed", |e| match e {
        Event::CatalogPageFailed { req, .. } => Some(*req),
        _ => None,
    })
    .await;
    assert_eq!(failed, 41);
    let err = wait(&events, "unsupported", |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(err, Error::Unsupported { feature: "session_page" });
    client.list_sessions().unwrap();
    assert_eq!(fake.next_control().await["type"], "sessions", "no catalog_page frame may precede it");
    client.shutdown().unwrap();
}

/// Review F6: the daemon aborts a page still loading when its connection
/// attaches, yet zero or one late reply may still arrive before `Attached`.
/// After an `Attach`, every page outstanding on that connection is
/// cancelled and a late reply is dropped. (The client's own pages go on the
/// control connection, which never attaches; this is the connection's rule.)
#[tokio::test]
async fn an_attach_cancels_the_pages_outstanding_on_its_connection() {
    use synaps_client::conn::Conn;
    use synaps_client::protocol::{Attach, ClientFrame, DaemonFrame};
    let dir = std::env::temp_dir().join(format!("synaps-fake-{}-attach-pages", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, w) = stream.into_split();
        let w: Writer = Arc::new(Mutex::new(w));
        let mut r = BufReader::new(r).lines();
        r.next_line().await.unwrap();
        write(&w, welcome(Opts { pages: true, ..Default::default() })).await;
        let mut types = Vec::new();
        for _ in 0..3 {
            let f: Value = serde_json::from_str(&r.next_line().await.unwrap().unwrap()).unwrap();
            types.push(f["type"].as_str().unwrap().to_string());
        }
        // One late reply (req 1), none for req 2, then `Attached`.
        write(&w, json!({"type": "catalog_page", "req": 1, "sessions": catalog(), "next": null})).await;
        write(&w, attached_frame(2)).await;
        write(&w, json!({"type": "pong", "pid": 1, "uptime_s": 1, "sessions": 0})).await;
        let _ = r.next_line().await;
        types
    });
    let mut c = Conn::connect(&path, Identity::new("tester").unwrap().hello(None)).await.unwrap();
    for req in [1, 2] {
        c.send(&ClientFrame::CatalogPage { req, before: None, limit: 50, named_only: false }).await.unwrap();
    }
    assert!(c.cancelled().is_empty());
    c.send(&ClientFrame::Attach(Attach::Existing { session_id: SID.into(), mode: AttachMode::Mirror })).await.unwrap();
    assert_eq!(c.cancelled(), [1, 2], "every page outstanding on the connection");
    let first = tokio::time::timeout(T, c.recv()).await.unwrap().unwrap().unwrap();
    assert!(matches!(first, DaemonFrame::Attached(_)), "the late page is dropped: {first:?}");
    assert!(c.cancelled().is_empty(), "`Attached` ends the window");
    assert!(matches!(tokio::time::timeout(T, c.recv()).await.unwrap().unwrap(), Some(DaemonFrame::Pong { .. })));
    drop(c);
    assert_eq!(server.await.unwrap(), ["catalog_page", "catalog_page", "attach"]);
}

/// `Attached` for session `sid` (the fixture's, with its id swapped).
fn attached_to(client: u64, sid: &str) -> Value {
    let mut a = attached_frame(client);
    a["meta"]["id"] = json!(sid);
    a
}

/// The next `cmd` frame on `conn`, with the session it names.
async fn next_cmd(conn: &mut Conn) -> (String, Value) {
    loop {
        let f = conn.recv().await.expect("eof waiting for a cmd");
        if f["type"] == "cmd" {
            return (f["session_id"].as_str().unwrap_or_default().to_string(), f["cmd"].clone());
        }
    }
}

/// The attachment of the next `Attached`, and its connection.
async fn attached_seq(events: &async_channel::Receiver<Event>) -> (u64, u64) {
    wait(events, "attached", |e| match e {
        Event::Attached { attach_seq, attached, .. } => Some((*attach_seq, attached.connection)),
        _ => None,
    })
    .await
}

/// Split panes step 2 (architecture §3.3): two sessions attached at once,
/// each on its own connection. Every command addressed with `on(seq)`
/// reaches that attachment's daemon connection and session only: a
/// submit, a steer, an abort and an answer for A never reach B, and the
/// other way round. An answer aimed at B's connection but addressed to A
/// is refused and sent nowhere. Detaching A leaves B; A's commands are
/// then `NotAttached` with A's seq, and B never sees them. A plain
/// `attach` still replaces every attachment.
#[tokio::test]
async fn two_attachments_route_every_command_by_seq() {
    let mut fake = Fake::start("multi").await;
    let (client, events) = fake.client();
    let a = client.attach(SID, AttachMode::Mirror).unwrap();
    let mut ca = fake.next_attach().await;
    ca.send(attached_to(2, SID)).await;
    let (sa, conn_a) = attached_seq(&events).await;
    let b = client.attach_more(SID2, AttachMode::Mirror).unwrap();
    assert!(b > a);
    let mut cb = fake.next_attach().await;
    assert!(cb.attach.to_string().contains(SID2), "B's attach is for B's session: {}", cb.attach);
    cb.send(attached_to(5, SID2)).await;
    let (sb, conn_b) = attached_seq(&events).await;
    assert_eq!((sa, sb), (a, b));
    assert_ne!(conn_a, conn_b, "one connection each");

    client.on(b).submit("to B").unwrap();
    client.on(a).submit("to A").unwrap();
    client.on(a).steer("steer A").unwrap();
    client.on(b).abort().unwrap();
    client.on(b).answer_on(conn_b, 7, Some("y".into())).unwrap();
    // B's prompt, answered through A: refused, sent nowhere.
    client.on(a).answer_on(conn_b, 7, Some("y".into())).unwrap();
    let why = wait(&events, "a cross-session answer refused", |e| match e {
        Event::Error { attach_seq, error: Error::Refused { command: Some(c), reason } } if c == "answer" => {
            Some((*attach_seq, reason.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!(why.0, Some(a), "{why:?}");
    client.on(a).answer_on(conn_a, 3, Some("n".into())).unwrap();

    assert_eq!(next_cmd(&mut ca).await, (SID.to_string(), json!({"cmd": "submit", "text": "to A", "attachments": []})));
    assert_eq!(next_cmd(&mut ca).await, (SID.to_string(), json!({"cmd": "steer", "text": "steer A"})));
    assert_eq!(next_cmd(&mut ca).await, (SID.to_string(), json!({"cmd": "answer", "prompt_id": 3, "value": "n"})));
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "submit", "text": "to B", "attachments": []})));
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "cancel"})));
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "answer", "prompt_id": 7, "value": "y"})));

    // A leaves; B stays.
    client.on(a).detach().unwrap();
    assert_eq!(next_cmd(&mut ca).await, (SID.to_string(), json!({"cmd": "detach", "client": 2})));
    let gone = wait(&events, "A detached", |e| match e {
        Event::Detached { attach_seq, reason, .. } => Some((*attach_seq, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(gone, (a, "detached".to_string()));
    client.on(a).submit("A is gone").unwrap();
    let err = wait(&events, "A not attached", |e| match e {
        Event::Error { attach_seq, error: e @ Error::NotAttached { .. } } => Some((*attach_seq, e.clone())),
        _ => None,
    })
    .await;
    assert_eq!(err, (Some(a), Error::NotAttached { command: "submit" }));
    client.on(b).submit("still B").unwrap();
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "submit", "text": "still B", "attachments": []})));

    // A plain attach replaces every attachment (B included).
    let c = client.attach(SID, AttachMode::Mirror).unwrap();
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "detach", "client": 5})));
    let replaced = wait(&events, "B replaced", |e| match e {
        Event::Detached { attach_seq, reason, .. } if *attach_seq == b => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert_eq!(replaced, "replaced by another attach");
    let cc = fake.next_attach().await;
    cc.send(attached_to(9, SID)).await;
    assert_eq!(attached_seq(&events).await.0, c);
    client.shutdown().unwrap();
}

/// One attachment ending by itself (its session ended) leaves the other
/// attached and addressable; commands to the ended one are `NotAttached`
/// with its seq and reach nobody.
#[tokio::test]
async fn one_attachment_ending_leaves_the_other() {
    let mut fake = Fake::start("multi-end").await;
    let (client, events) = fake.client();
    let a = client.attach(SID, AttachMode::Mirror).unwrap();
    let ca = fake.next_attach().await;
    ca.send(attached_to(2, SID)).await;
    attached_seq(&events).await;
    let b = client.attach_more(SID2, AttachMode::Mirror).unwrap();
    let mut cb = fake.next_attach().await;
    cb.send(attached_to(3, SID2)).await;
    attached_seq(&events).await;
    ca.event(30, json!({"ev": "ended", "reason": "client_quit"})).await;
    let ended = wait(&events, "A ended", |e| match e {
        Event::Detached { attach_seq, reason, .. } => Some((*attach_seq, reason.clone())),
        _ => None,
    })
    .await;
    assert_eq!(ended, (a, "session ended".to_string()));
    client.on(a).steer("to the ended one").unwrap();
    let err = wait(&events, "not attached", |e| match e {
        Event::Error { attach_seq, error: e @ Error::NotAttached { .. } } => Some((*attach_seq, e.clone())),
        _ => None,
    })
    .await;
    assert_eq!(err, (Some(a), Error::NotAttached { command: "steer" }));
    client.on(b).steer("to B").unwrap();
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "steer", "text": "to B"})));
    client.shutdown().unwrap();
    assert_eq!(next_cmd(&mut cb).await, (SID2.to_string(), json!({"cmd": "detach", "client": 3})));
}

/// Past `ATTACH_MAX` attachments an `attach_more` is refused (its own
/// `Detached`, no connection made); the ones attached stay.
#[tokio::test]
async fn attach_more_past_the_cap_is_refused() {
    let mut fake = Fake::start("multi-cap").await;
    let (client, events) = fake.client();
    let mut conns = Vec::new();
    let mut seqs = Vec::new();
    for i in 0..synaps_client::ATTACH_MAX as u64 {
        seqs.push(client.attach_more(SID, AttachMode::Mirror).unwrap());
        let c = fake.next_attach().await;
        c.send(attached_to(10 + i, SID)).await;
        attached_seq(&events).await;
        conns.push(c);
    }
    let over = client.attach_more(SID2, AttachMode::Mirror).unwrap();
    let why = wait(&events, "refused", |e| match e {
        Event::Detached { attach_seq, reason, session_id } => Some((*attach_seq, reason.clone(), session_id.clone())),
        _ => None,
    })
    .await;
    assert_eq!(why.0, over);
    assert!(why.1.starts_with("refused"), "{why:?}");
    assert_eq!(why.2, None);
    // The first is still there.
    client.on(seqs[0]).submit("first").unwrap();
    assert_eq!(next_cmd(&mut conns[0]).await, (SID.to_string(), json!({"cmd": "submit", "text": "first", "attachments": []})));
    client.shutdown().unwrap();
}
