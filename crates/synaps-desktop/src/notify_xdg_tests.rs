//! Linux's bus transport (Shady #83 R2-1, R2-2) over a fake bus: the
//! window's side ([`Xdg`] under [`Notes`]), the task ([`run`]) and its
//! state ([`Bus`]) are the production code; only the socket is fake. It
//! hands the task what it would read, through a queue of 64 whose writer
//! waits while it is full (zbus's reader); its writes can stall (a bus
//! that stops reading us: the write stays pending until let go), and a
//! scripted bus and
//! notification server ([`Server`]) answers what the task writes, as
//! `dbus-daemon` routes it: a call to a name nobody owns is the bus
//! daemon's error. Time is the test executor's, moved by hand: `spin` runs
//! what is runnable and never moves the clock.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::TestAppContext;

use super::*;
use crate::notify::{Notes, note, tag_of};
use crate::presence::Key;

/// The fake socket: what the task wrote, and the reader's queue. Its
/// writes stall while `stall` is set (each one started then waits for a
/// `go`); both channels close once the task has dropped every handle on
/// the connection (the in-progress write too).
struct Fake {
    wrote: async_channel::Receiver<Message>,
    feed: async_channel::Sender<zbus::Result<Message>>,
    stall: Arc<AtomicBool>,
    go: async_channel::Sender<()>,
}

/// A dial that connects at once to a fake socket (once), or never.
fn fake(connects: bool) -> (Dial, Fake) {
    let (out_tx, wrote) = async_channel::unbounded::<Message>();
    let (feed, in_rx) = async_channel::bounded::<zbus::Result<Message>>(64);
    let (go, gone_on) = async_channel::unbounded::<()>();
    let stall = Arc::new(AtomicBool::new(false));
    let stalls = stall.clone();
    let slot = Arc::new(Mutex::new(Some((out_tx, in_rx))));
    let dial: Dial = Arc::new(move || {
        let s = slot.lock().unwrap().take();
        let (stalls, gone_on) = (stalls.clone(), gone_on.clone());
        Box::pin(async move {
            if !connects {
                std::future::pending::<()>().await;
            }
            let (out, rx) = s.ok_or_else(|| zbus::Error::Failure("dialled twice".into()))?;
            let send = Box::new(move |m: Message| -> Sending {
                let (out, stalled, go) = (out.clone(), stalls.load(Ordering::SeqCst), gone_on.clone());
                Box::pin(async move {
                    if stalled {
                        go.recv().await.map_err(|_| zbus::Error::Failure("hung up".into()))?;
                    }
                    out.send(m).await.map_err(|_| zbus::Error::Failure("hung up".into()))
                })
            });
            Ok(Wire { send, recv: Box::pin(rx) })
        })
    });
    (dial, Fake { wrote, feed, stall, go })
}

/// Everything runnable run; the clock left where it is.
fn spin(cx: &mut TestAppContext) {
    for _ in 0..100_000 {
        if !cx.executor().tick() {
            return;
        }
    }
    panic!("still busy");
}

fn member(m: &Message) -> String {
    m.header().member().map(|x| x.to_string()).unwrap_or_default()
}

fn to(m: &Message) -> String {
    m.header().destination().map(|x| x.to_string()).unwrap_or_default()
}

/// `Notify`'s arguments: app, replaces, icon, summary, body, actions,
/// hints, timeout.
type NotifyArgs = (String, u32, String, String, String, Vec<String>, HashMap<String, zbus::OwnedVariant>, i32);

/// `Notify`'s body: its summary and body text.
fn notify_text(m: &Message) -> (String, String) {
    let b = m.body();
    let (_, _, _, title, body, _, _, _): NotifyArgs = b.deserialize().expect("Notify's body");
    (title, body)
}

fn close_id(m: &Message) -> u32 {
    m.body().deserialize::<u32>().expect("CloseNotification's id")
}

/// The bus daemon and the notification service, scripted.
struct Server {
    f: Fake,
    /// The service's owner (a unique name), if any.
    owner: Option<String>,
    markup: bool,
    /// `GetCapabilities` answered with an error.
    caps_fail: bool,
    /// The owner's next id (a restarted server counts from 1 again).
    next: u32,
    /// `Notify` calls kept unanswered.
    hold: bool,
    held: Vec<Message>,
    /// Every call the task wrote.
    log: Vec<Message>,
}

impl Server {
    fn new(f: Fake, owner: &str, markup: bool) -> Self {
        Server { f, owner: Some(owner.into()), markup, caps_fail: false, next: 1, hold: false, held: Vec::new(), log: Vec::new() }
    }

    /// Writes from now on stall (or don't).
    fn stall(&self, on: bool) {
        self.f.stall.store(on, Ordering::SeqCst);
    }

    /// One stalled write let through.
    fn go(&self) {
        self.f.go.try_send(()).unwrap();
    }

    /// The connection dropped by the task: no handle on it left.
    fn dropped(&self) -> bool {
        self.f.feed.is_closed() && self.f.wrote.is_closed()
    }

    fn put(&self, m: Message) {
        self.f.feed.try_send(Ok(m)).expect("room in the reader's queue");
    }

    /// What the task writes answered, until it writes nothing more; those
    /// calls.
    fn serve(&mut self, cx: &mut TestAppContext) -> Vec<Message> {
        let mut all = Vec::new();
        loop {
            spin(cx);
            let calls: Vec<Message> = std::iter::from_fn(|| self.f.wrote.try_recv().ok()).collect();
            if calls.is_empty() {
                break;
            }
            for c in calls {
                if let Some(r) = self.answer(&c) {
                    self.put(r);
                }
                all.push(c);
            }
        }
        self.log.extend(all.iter().cloned());
        all
    }

    fn answer(&mut self, c: &Message) -> Option<Message> {
        let h = c.header();
        let dest = to(c);
        let err = |from: &str, name: &str| Message::error(&h, name).sender(from).build(&"no").unwrap();
        if dest == BUS {
            return Some(match member(c).as_str() {
                "AddMatch" => Message::method_return(&h).sender(BUS).build(&()).unwrap(),
                "GetNameOwner" => match &self.owner {
                    Some(o) => Message::method_return(&h).sender(BUS).build(&o.as_str()).unwrap(),
                    None => err(BUS, "org.freedesktop.DBus.Error.NameHasNoOwner"),
                },
                _ => err(BUS, "org.freedesktop.DBus.Error.ServiceUnknown"),
            });
        }
        // The well-known name reaches whoever owns it now.
        let owner = self.owner.clone();
        let Some(owner) = owner.filter(|o| *o == dest || dest == XDG_NAME) else {
            return Some(err(BUS, "org.freedesktop.DBus.Error.ServiceUnknown"));
        };
        let ok = |body: &dyn Fn(zbus::message::Builder<'static>) -> Message| body(Message::method_return(&h).sender(owner.clone()));
        Some(match member(c).as_str() {
            "GetCapabilities" if self.caps_fail => err(&owner, "org.freedesktop.DBus.Error.Failed"),
            "GetCapabilities" => {
                let caps: Vec<&str> = if self.markup { vec!["body", "body-markup", "actions"] } else { vec!["body", "actions"] };
                ok(&|b| b.build(&caps).unwrap())
            }
            "Notify" if self.hold => {
                self.held.push(c.clone());
                return None;
            }
            "Notify" => {
                let id = self.next;
                self.next += 1;
                ok(&|b| b.build(&id).unwrap())
            }
            "CloseNotification" => ok(&|b| b.build(&()).unwrap()),
            _ => err(&owner, "org.freedesktop.DBus.Error.UnknownMethod"),
        })
    }

    /// A held `Notify` answered (by whoever it was asked of) with `id`.
    fn release(&mut self, ix: usize, id: u32) {
        let c = self.held.remove(ix);
        let from = to(&c);
        self.put(Message::method_return(&c.header()).sender(from).build(&id).unwrap());
    }

    /// A signal from `from` at `path`.
    fn signal<B: serde::Serialize + zbus::DynamicType>(&self, from: &str, path: &str, iface: &str, name: &str, body: &B) {
        self.put(Message::signal(path, iface, name).sender(from).build(body).unwrap());
    }

    fn clicked(&self, id: u32, from: &str) {
        self.signal(from, XDG_PATH, XDG_NAME, "ActionInvoked", &(id, "default"));
    }

    /// The service changes hands: `NameOwnerChanged` from the bus daemon.
    fn owned(&mut self, new: Option<&str>) {
        let old = self.owner.clone().unwrap_or_default();
        self.owner = new.map(str::to_string);
        self.signal(BUS, BUS_PATH, BUS, "NameOwnerChanged", &(XDG_NAME, old.as_str(), new.unwrap_or("")));
    }
}

fn key(n: usize) -> Key {
    Key { session: "s".into(), tool_id: format!("toolu_{n}"), card: cards_spec::CardId::parse(&format!("c{n:06}")).unwrap() }
}

/// A window's notes over the fake bus, and where its clicks come.
fn notes(dial: Dial) -> (Notes, async_channel::Receiver<String>) {
    let (tx, rx) = async_channel::bounded(NOTES_MAX);
    (Notes::new(Box::new(Xdg::new(dial, tx)), None), rx)
}

fn post(cx: &mut TestAppContext, n: &mut Notes, k: usize, question: &str) -> String {
    let key = key(k);
    let note = note(&key, "relay", question);
    let tag = note.tag.clone();
    cx.update(|cx| n.post(key, note, cx));
    tag
}

fn clicks(rx: &async_channel::Receiver<String>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn named<'a>(calls: &'a [Message], m: &str) -> Vec<&'a Message> {
    calls.iter().filter(|c| member(c) == m).collect()
}

/// R2-2. Ids are the issuing server's: its match rules first, then who
/// owns the service; each call to that owner's unique name; capabilities
/// asked of each owner; a signal or a reply counts only from whom it
/// should. A restart hands the same numbers to another app's
/// notifications: none of them is ours, nothing is closed at the new
/// owner, its clicks route nowhere. An owner change with calls out: the
/// old owner's late id is forgotten too.
#[gpui_kit::test]
fn a_restart_reuses_ids_and_nothing_crosses_over(cx: &mut TestAppContext) {
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, rx) = notes(dial);
    let t1 = post(cx, &mut n, 1, "Ship <b>it</b> & go?");
    let calls = s.serve(cx);
    let order: Vec<String> = calls.iter().map(member).collect();
    assert_eq!(order, ["AddMatch", "AddMatch", "AddMatch", "GetNameOwner", "GetCapabilities", "Notify"], "rules, then the owner");
    let rules: Vec<String> = named(&calls, "AddMatch").iter().map(|m| m.body().deserialize::<String>().unwrap()).collect();
    assert!(rules.iter().any(|r| r.contains("member='NameOwnerChanged'") && r.contains(&format!("arg0='{XDG_NAME}'"))), "{rules:?}");
    assert!(
        rules
            .iter()
            .filter(|r| r.contains("ActionInvoked") || r.contains("NotificationClosed"))
            .all(|r| r.contains(&format!("path='{XDG_PATH}'")))
    );
    let notify = named(&calls, "Notify")[0];
    assert_eq!(to(notify), ":1.10", "to the owner's unique name, never the service's");
    assert_eq!(notify_text(notify).1, "Ship &lt;b&gt;it&lt;/b&gt; &amp; go?", "A reads markup: escaped");
    assert_eq!(to(named(&calls, "GetCapabilities")[0]), ":1.10");
    // A's id 1 is t1's. A click from another sender, or another path: not ours.
    s.clicked(1, ":1.99");
    s.signal(":1.10", "/org/freedesktop/Other", XDG_NAME, "ActionInvoked", &(1u32, "default"));
    spin(cx);
    assert!(clicks(&rx).is_empty(), "wrong sender, wrong path: ignored");
    s.clicked(1, ":1.10");
    spin(cx);
    assert_eq!(clicks(&rx), std::slice::from_ref(&t1), "A's click on A's id");
    // A restarts as B, which counts from 1 again and reads no markup.
    s.owned(None);
    s.owned(Some(":1.11"));
    s.markup = false;
    s.next = 1;
    let calls = s.serve(cx);
    assert_eq!(
        calls.iter().map(|c| (member(c), to(c))).collect::<Vec<_>>(),
        [("GetCapabilities".to_string(), ":1.11".to_string())],
        "B asked anew"
    );
    // Another app's notification on B is id 1: its click and its close are not ours.
    s.clicked(1, ":1.11");
    s.signal(":1.11", XDG_PATH, XDG_NAME, "NotificationClosed", &(1u32, 2u32));
    spin(cx);
    assert!(clicks(&rx).is_empty(), "B's id 1 is not A's");
    // t1's card resolves: nothing goes to B (A's id means nothing there).
    cx.update(|cx| n.retire(&t1, cx));
    let calls = s.serve(cx);
    assert!(named(&calls, "CloseNotification").is_empty(), "no close at the new owner: {calls:?}");
    // A card on B: B's id 1 is now t2's; B reads no markup.
    let t2 = post(cx, &mut n, 2, "x < y?");
    let calls = s.serve(cx);
    let notify = named(&calls, "Notify")[0];
    assert_eq!((to(notify), notify_text(notify).1), (":1.11".to_string(), "x < y?".to_string()), "B's own capabilities");
    s.clicked(1, ":1.10");
    s.clicked(1, ":1.11");
    spin(cx);
    assert_eq!(clicks(&rx), [t2], "only B's click on B's id");
    // A reply from someone else to a call out: ignored. B hands over to C
    // while t3's Notify is out at B; C's capabilities are answered first by
    // an impostor.
    s.hold = true;
    let t3 = post(cx, &mut n, 3, "a & b?");
    s.serve(cx);
    assert_eq!(s.held.len(), 1);
    s.owned(Some(":1.12"));
    spin(cx);
    let caps = std::iter::from_fn(|| s.f.wrote.try_recv().ok()).find(|c| member(c) == "GetCapabilities").expect("C asked");
    assert_eq!(to(&caps), ":1.12");
    s.put(Message::method_return(&caps.header()).sender(":1.99").build(&vec!["body"]).unwrap());
    s.put(Message::method_return(&caps.header()).sender(":1.12").build(&vec!["body", "body-markup"]).unwrap());
    // B's late answer to t3's Notify: an old owner's id, forgotten.
    s.release(0, 2);
    spin(cx);
    s.clicked(2, ":1.11");
    s.clicked(2, ":1.12");
    spin(cx);
    assert!(clicks(&rx).is_empty(), "t3's id was B's, and B is gone");
    cx.update(|cx| n.retire(&t3, cx));
    s.hold = false;
    let t4 = post(cx, &mut n, 4, "a & b?");
    let calls = s.serve(cx);
    assert!(named(&calls, "CloseNotification").is_empty(), "nothing closed at C by B's id: {calls:?}");
    let notify = named(&calls, "Notify");
    assert_eq!(notify.len(), 1, "t3 never told again; t4 told: {calls:?}");
    assert_eq!(
        (to(notify[0]), notify_text(notify[0]).1),
        (":1.12".to_string(), "a &amp; b?".to_string()),
        "C's capabilities, not the impostor's"
    );
    assert_ne!(tag_of(&key(4)), t3);
    // C's capabilities unknown (an error): escaped, the safe reading.
    s.owned(Some(":1.13"));
    s.caps_fail = true;
    s.serve(cx);
    post(cx, &mut n, 5, "1 < 2");
    let calls = s.serve(cx);
    assert_eq!(notify_text(named(&calls, "Notify")[0]).1, "1 &lt; 2");
    // Every close the task ever sent went to the owner that issued its id.
    assert!(s.log.iter().filter(|c| member(c) == "CloseNotification").all(|c| to(c).starts_with(':')));
    let _ = t4;
}

/// R2-1. While `Notify` waits for its reply, the server sends far more
/// signals than the reader's queue holds (other apps' notifications
/// closing): the task keeps reading, so the reader reaches the reply; the
/// click on that id routes, and the next card is told. Then the window
/// goes: what is up is closed and the task ends.
#[gpui_kit::test]
fn a_signal_flood_while_a_call_waits(cx: &mut TestAppContext) {
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, rx) = notes(dial);
    s.hold = true;
    let t1 = post(cx, &mut n, 1, "first?");
    s.serve(cx);
    assert_eq!(s.held.len(), 1, "Notify out, unanswered");
    let call = s.held.remove(0);
    let reply = Message::method_return(&call.header()).sender(":1.10").build(&7u32).unwrap();
    let flood: Vec<Message> = (0..300u32)
        .map(|i| Message::signal(XDG_PATH, XDG_NAME, "NotificationClosed").sender(":1.10").build(&(1000 + i, 2u32)).unwrap())
        .collect();
    let (done, feed) = (Arc::new(AtomicBool::new(false)), s.f.feed.clone());
    let fed = done.clone();
    cx.executor()
        .spawn(async move {
            for m in flood.into_iter().chain([reply]) {
                if feed.send(Ok(m)).await.is_err() {
                    return;
                }
            }
            fed.store(true, Ordering::SeqCst);
        })
        .detach();
    spin(cx);
    assert!(done.load(Ordering::SeqCst), "the reader got through 300 signals to the reply");
    s.clicked(7, ":1.10");
    spin(cx);
    assert_eq!(clicks(&rx), [t1], "the reply was read: id 7 is t1's");
    s.hold = false;
    post(cx, &mut n, 2, "second?");
    let calls = s.serve(cx);
    assert_eq!(named(&calls, "Notify").len(), 1, "the next card told");
    // The window goes: both closed, at their issuer; the task ends.
    let gone = cx.update(|cx| n.teardown(cx));
    let calls = s.serve(cx);
    let mut closed: Vec<(String, u32)> = named(&calls, "CloseNotification").iter().map(|c| (to(c), close_id(c))).collect();
    closed.sort();
    assert_eq!(closed, [(":1.10".to_string(), 1), (":1.10".to_string(), 7)]);
    spin(cx);
    assert!(gone.0.as_ref().is_some_and(|g| g.is_closed()), "the task ended");
    assert!(s.f.feed.is_closed(), "and dropped its connection");
}

/// R2-1. The window goes while a call is out and never answered: that is
/// heard at once (the notification up is closed then, not after the
/// call), nothing queued is shown, and the task waits for the reply at
/// most `CALL_WAIT`, then ends. A `Notify` answered during the teardown is closed at once.
/// A window gone while the bus doesn't even connect: the task ends then.
#[gpui_kit::test]
fn the_window_gone_mid_call(cx: &mut TestAppContext) {
    // Never answered; one shown before it.
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, _rx) = notes(dial);
    post(cx, &mut n, 0, "up?");
    s.serve(cx);
    s.hold = true;
    post(cx, &mut n, 1, "q?");
    s.serve(cx);
    assert_eq!(s.held.len(), 1);
    post(cx, &mut n, 2, "queued behind it?");
    let gone = cx.update(|cx| n.teardown(cx));
    let calls = s.serve(cx);
    assert_eq!(
        calls.iter().map(|c| (member(c), to(c))).collect::<Vec<_>>(),
        [("CloseNotification".to_string(), ":1.10".to_string())],
        "heard mid-call: the one up closed at once, nothing more shown"
    );
    assert!(!gone.0.as_ref().unwrap().is_closed(), "the reply might still come");
    cx.executor().advance_clock(CALL_WAIT);
    let calls = s.serve(cx);
    assert!(calls.is_empty(), "{calls:?}");
    assert!(gone.0.as_ref().unwrap().is_closed(), "ended after CALL_WAIT, unanswered");
    assert!(s.f.feed.is_closed());
    // Answered during the teardown: closed at once, then the end.
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, _rx) = notes(dial);
    s.hold = true;
    post(cx, &mut n, 1, "q?");
    s.serve(cx);
    let gone = cx.update(|cx| n.teardown(cx));
    spin(cx);
    s.release(0, 5);
    let calls = s.serve(cx);
    assert_eq!(named(&calls, "CloseNotification").iter().map(|c| (to(c), close_id(c))).collect::<Vec<_>>(), [(":1.10".to_string(), 5)]);
    spin(cx);
    assert!(gone.0.as_ref().unwrap().is_closed());
    // No bus at all, ever: the window's going ends the task.
    let (dial, _f) = fake(false);
    let (mut n, _rx) = notes(dial);
    post(cx, &mut n, 1, "q?");
    spin(cx);
    let gone = cx.update(|cx| n.teardown(cx));
    spin(cx);
    assert!(gone.0.unwrap().is_closed(), "ended while connecting");
}

/// R2-1. A server that answers nothing about notifications, and a
/// thousand cards posted and resolved: what waits is state, not messages.
/// The window's wanted set never passes `NOTES_MAX`; at most `CALLS_MAX`
/// calls are out; each `CALL_WAIT` frees them for the next.
#[gpui_kit::test]
fn a_silent_server_keeps_everything_bounded(cx: &mut TestAppContext) {
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, _rx) = notes(dial);
    s.hold = true;
    let mut written = 0;
    for k in 0..1000 {
        let tag = post(cx, &mut n, k, "q?");
        if k % 3 == 0 {
            cx.update(|cx| n.retire(&tag, cx));
        }
        if k % 50 == 0 {
            written += named(&s.serve(cx), "Notify").len();
        }
        let wanted = n.queued();
        assert!(wanted <= NOTES_MAX, "{wanted}");
    }
    written += named(&s.serve(cx), "Notify").len();
    assert_eq!(written, CALLS_MAX, "no more Notify out than CALLS_MAX");
    cx.executor().advance_clock(CALL_WAIT);
    let next = named(&s.serve(cx), "Notify").len();
    assert!(next <= CALLS_MAX && next > 0, "the next ones, as many: {next}");
}

/// Nobody owns the service: the bus is asked to start one, once per new
/// card; still nobody, that card is not told. One that comes later is
/// told by it.
#[gpui_kit::test]
fn no_service_is_started_once_then_followed(cx: &mut TestAppContext) {
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    s.owner = None;
    let (mut n, _rx) = notes(dial);
    post(cx, &mut n, 1, "q?");
    let calls = s.serve(cx);
    assert_eq!(named(&calls, "StartServiceByName").len(), 1);
    assert!(named(&calls, "Notify").is_empty());
    cx.executor().advance_clock(CALL_WAIT * 3);
    assert!(s.serve(cx).is_empty(), "not tried again for it");
    s.owned(Some(":1.20"));
    post(cx, &mut n, 2, "q2?");
    let calls = s.serve(cx);
    let notify = named(&calls, "Notify");
    assert_eq!(notify.len(), 1, "only the new card: {calls:?}");
    assert_eq!(to(notify[0]), ":1.20");
}

/// A window's notes on a fake bus with one card's notification up (A's
/// id 1), and where its clicks come.
fn one_up(cx: &mut TestAppContext) -> (Server, Notes, async_channel::Receiver<String>) {
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, rx) = notes(dial);
    post(cx, &mut n, 0, "up?");
    s.serve(cx);
    (s, n, rx)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// `Notify`s written: to whom, and the question.
fn told(calls: &[Message]) -> Vec<(String, String)> {
    named(calls, "Notify").iter().map(|m| (to(m), notify_text(m).1)).collect()
}

/// Shady #89 N1. A write the bus never takes (it stopped reading us) has
/// a bound of its own, apart from any reply: past `WRITE_WAIT` the task
/// ends and drops the connection, the stalled write with it, and never
/// writes to it again.
#[gpui_kit::test]
fn a_stalled_write_abandons_its_connection(cx: &mut TestAppContext) {
    let (s, mut n, _rx) = one_up(cx);
    s.stall(true);
    post(cx, &mut n, 1, "stuck?");
    spin(cx);
    cx.executor().advance_clock(WRITE_WAIT - ms(1));
    spin(cx);
    assert!(!s.dropped(), "not yet");
    cx.executor().advance_clock(ms(1));
    spin(cx);
    assert!(s.dropped(), "past WRITE_WAIT: the connection dropped, the stalled write with it");
}

/// N1. The window gone mid-write, the close of what is up waiting behind
/// it: the task ends at the write's bound, and the close is never written
/// on a stream cut mid-message (that notification stays: the residual).
#[gpui_kit::test]
fn the_window_gone_mid_write_ends_at_the_writes_bound(cx: &mut TestAppContext) {
    let (mut s, mut n, _rx) = one_up(cx);
    s.stall(true);
    post(cx, &mut n, 1, "stuck?");
    spin(cx);
    cx.executor().advance_clock(ms(2000));
    let gone = cx.update(|cx| n.teardown(cx));
    spin(cx);
    assert!(!gone.done(), "the write may still finish");
    cx.executor().advance_clock(WRITE_WAIT - ms(2000));
    spin(cx);
    assert!(gone.done(), "ended at the write's bound");
    assert!(s.dropped());
    assert!(s.serve(cx).is_empty(), "nothing written after the cut");
}

/// N1. The teardown's own bound: the write in progress finishes after the
/// window went, and its reply never comes. The task ends `LEAVE_WAIT`
/// after the window went, not `CALL_WAIT` after that write.
#[gpui_kit::test]
fn a_teardown_ends_by_leave_wait(cx: &mut TestAppContext) {
    let (mut s, mut n, _rx) = one_up(cx);
    s.stall(true);
    s.hold = true;
    post(cx, &mut n, 1, "slow?");
    spin(cx);
    cx.executor().advance_clock(ms(1000));
    let gone = cx.update(|cx| n.teardown(cx));
    spin(cx);
    cx.executor().advance_clock(ms(3000));
    s.stall(false);
    s.go();
    let calls = s.serve(cx);
    assert_eq!(told(&calls).len(), 1, "the write in progress went on: {calls:?}");
    assert_eq!(named(&calls, "CloseNotification").iter().map(|c| close_id(c)).collect::<Vec<_>>(), [1], "t0 closed");
    cx.executor().advance_clock(LEAVE_WAIT - ms(3000) - ms(1));
    spin(cx);
    assert!(!gone.done(), "t1's id might still come");
    cx.executor().advance_clock(ms(1));
    spin(cx);
    assert!(gone.done(), "LEAVE_WAIT after the window went: ended, t1's reply unheard");
    assert!(s.dropped());
}

/// N1. A write that is slow but finishes counts its reply's `CALL_WAIT`
/// from then: its id still counts.
#[gpui_kit::test]
fn a_slow_write_counts_its_reply_from_the_write(cx: &mut TestAppContext) {
    let (mut s, mut n, rx) = one_up(cx);
    s.stall(true);
    s.hold = true;
    let t1 = post(cx, &mut n, 1, "slow?");
    s.serve(cx);
    cx.executor().advance_clock(WRITE_WAIT - ms(1));
    s.stall(false);
    s.go();
    s.serve(cx);
    assert_eq!(s.held.len(), 1, "written at last");
    cx.executor().advance_clock(CALL_WAIT - ms(1));
    s.release(0, 9);
    s.clicked(9, ":1.10");
    spin(cx);
    assert_eq!(clicks(&rx), [t1], "its reply counted from its write: id 9 is t1's");
    assert!(!s.dropped());
}

/// Shady #89 N2. The window goes with three `Notify`s built behind a
/// stalled write: they are taken back, whole; only the one being written
/// goes on, and it is closed when its id comes.
#[gpui_kit::test]
fn the_window_gone_takes_back_what_is_built(cx: &mut TestAppContext) {
    let (mut s, mut n, _rx) = one_up(cx);
    s.stall(true);
    for k in 1..=3 {
        post(cx, &mut n, k, &format!("q{k}?"));
    }
    spin(cx);
    let gone = cx.update(|cx| n.teardown(cx));
    spin(cx);
    s.stall(false);
    s.go();
    let calls = s.serve(cx);
    assert_eq!(told(&calls), [(":1.10".to_string(), "q1?".to_string())], "nothing written after the window went but that write: {calls:?}");
    let mut closed: Vec<u32> = named(&calls, "CloseNotification").iter().map(|c| close_id(c)).collect();
    closed.sort();
    assert_eq!(closed, [1, 2], "t0, and t1 when its id came");
    spin(cx);
    assert!(gone.done());
}

/// N2. A card resolves while its `Notify` is built, behind a stalled
/// write: never written, nothing to close.
#[gpui_kit::test]
fn a_resolved_card_takes_back_its_notify(cx: &mut TestAppContext) {
    let (mut s, mut n, _rx) = one_up(cx);
    s.stall(true);
    post(cx, &mut n, 1, "q1?");
    let t2 = post(cx, &mut n, 2, "q2?");
    spin(cx);
    cx.update(|cx| n.retire(&t2, cx));
    spin(cx);
    s.stall(false);
    s.go();
    let calls = s.serve(cx);
    assert_eq!(told(&calls), [(":1.10".to_string(), "q1?".to_string())], "t2's taken back: {calls:?}");
    assert!(named(&calls, "CloseNotification").is_empty(), "{calls:?}");
}

/// N2 (Shady #89's epoch caveat). The owner changes while a `Notify` is
/// built for it, behind a stalled write: never written to the old owner,
/// told to the new one.
#[gpui_kit::test]
fn a_new_owner_gets_what_was_only_built_for_the_old(cx: &mut TestAppContext) {
    let (mut s, mut n, _rx) = one_up(cx);
    s.stall(true);
    post(cx, &mut n, 1, "q1?");
    post(cx, &mut n, 2, "q2?");
    spin(cx);
    s.owned(Some(":1.11"));
    spin(cx);
    s.stall(false);
    s.go();
    let calls = s.serve(cx);
    assert_eq!(
        told(&calls),
        [(":1.10".to_string(), "q1?".to_string()), (":1.11".to_string(), "q2?".to_string())],
        "q1 was being written to A; q2, only built, goes to B: {calls:?}"
    );
}

/// Shady #89 N3. The summary on the bus holds no link a server would offer.
#[gpui_kit::test]
fn the_summary_on_the_bus_holds_no_link(cx: &mut TestAppContext) {
    let (dial, f) = fake(true);
    let mut s = Server::new(f, ":1.10", true);
    let (mut n, _rx) = notes(dial);
    cx.update(|cx| n.post(key(0), note(&key(0), "https://x.io www.y.org", "q?"), cx));
    let calls = s.serve(cx);
    assert_eq!(notify_text(named(&calls, "Notify")[0]).0, "Needs you · https:\u{2060}//x.\u{2060}io www.\u{2060}y.\u{2060}org");
}

/// The production dial ([`session`]) on a real, private session bus,
/// against a real notification server (`notifyd.py`, in the evidence:
/// GDBus, ids from 1 per process, a click on its first id): the task's
/// calls, the bus daemon's routing and the servers' own headers. Server A
/// (markup) is told t1 and clicks it; A dies and B (plain text) takes the
/// name, its ids from 1 again: nothing of t1's reaches B; B is told t2 as
/// plain text, and B's click on its id 1 is t2's; the window gone, B
/// closes t2. Not run by default: `d3-evidence/notify/realbus/run.sh`
/// runs it inside `dbus-run-session` on bella.
#[gpui_kit::test]
#[ignore = "a private session bus and D3N_NOTIFYD=<notifyd.py>: d3-evidence/notify/realbus/run.sh"]
fn on_a_real_session_bus(cx: &mut TestAppContext) {
    // Real I/O: zbus's own thread wakes the task (GPUI's leave for it).
    cx.executor().allow_parking();
    let script = std::env::var("D3N_NOTIFYD").expect("D3N_NOTIFYD: the server's script");
    let dir = std::env::temp_dir().join(format!("d3n-realbus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let serve = |name: &str, args: &[&str]| {
        let log = std::fs::File::create(dir.join(format!("{name}.log"))).unwrap();
        std::process::Command::new("python3").arg(&script).arg(name).args(args).stdout(log).spawn().expect("python3")
    };
    let read = |name: &str| std::fs::read_to_string(dir.join(format!("{name}.log"))).unwrap_or_default();
    let until = |cx: &mut TestAppContext, what: &str, f: &mut dyn FnMut() -> bool| {
        for _ in 0..400 {
            spin(cx);
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("not within 10 s: {what}");
    };
    let mut a = serve("A", &[]);
    until(cx, "A owns the name", &mut || read("A").contains("owns"));
    let (mut n, rx) = notes(session());
    let mut heard = Vec::new();
    let t1 = post(cx, &mut n, 1, "Ship <b>it</b>?");
    until(cx, "A's click", &mut || {
        heard.extend(clicks(&rx));
        !heard.is_empty()
    });
    assert_eq!(heard, std::slice::from_ref(&t1), "A's click on its id 1 is t1's");
    assert!(read("A").contains("body='Ship &lt;b&gt;it&lt;/b&gt;?'"), "A reads markup: escaped\n{}", read("A"));
    a.kill().unwrap();
    a.wait().unwrap();
    let mut b = serve("B", &["--plain"]);
    until(cx, "B asked anew", &mut || read("B").contains("GetCapabilities"));
    cx.update(|cx| n.retire(&t1, cx));
    for _ in 0..20 {
        spin(cx);
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!read("B").contains("CloseNotification"), "t1's id never sent to B\n{}", read("B"));
    let t2 = post(cx, &mut n, 2, "x < y?");
    heard.clear();
    until(cx, "B's click", &mut || {
        heard.extend(clicks(&rx));
        !heard.is_empty()
    });
    assert_eq!(heard, [t2], "B's click on its id 1 is t2's, not t1's");
    assert!(read("B").contains("body='x < y?'"), "B reads plain text\n{}", read("B"));
    let gone = cx.update(|cx| n.teardown(cx));
    until(cx, "the task's end", &mut || gone.done());
    assert!(read("B").contains("CloseNotification from"), "t2 closed at B as the window went\n{}", read("B"));
    b.kill().unwrap();
    let _ = b.wait();
    eprintln!("--- A\n{}--- B\n{}", read("A"), read("B"));
    let _ = std::fs::remove_dir_all(&dir);
}
