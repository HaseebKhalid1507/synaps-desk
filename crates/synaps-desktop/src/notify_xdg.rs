//! Linux: the XDG notifications service (`org.freedesktop.Notifications`)
//! on the session bus, spoken to directly with zbus's messages (no proxy,
//! no `call_method`). Shady #83 R2-1 and R2-2.
//!
//! **One task per window, one connection, never blocked on a reply**
//! ([`run`]). zbus's reader puts every message the connection receives
//! into the task's stream and waits while that stream is full; a task
//! that awaits a reply without reading the stream can wait forever. So
//! nothing here awaits a reply. A call is a message written and a serial
//! remembered ([`Bus`]); its reply is just another message read from the
//! stream, matched by serial and by sender. The task reads the stream on
//! every turn: while it connects to nothing yet, while calls are out,
//! while it writes (a write in progress is polled alongside, never
//! dropped half-written), and while it tears down.
//!
//! **Bounded, cancellable lifetimes.** A call not answered within
//! [`CALL_WAIT`] has failed; one not yet written is dropped with it. At
//! most [`CALLS_MAX`] calls are out; what waits beyond that is state
//! (which notifications the window wants), not messages: the window's
//! side ([`Xdg`]) only writes the set of notes it wants (at most
//! [`NOTES_MAX`]) and wakes the task, coalesced. The window gone (its
//! side dropped) is heard on any turn, mid-call too: nothing queued is
//! shown, every notification up is closed, and the task ends once those
//! calls are answered or [`CALL_WAIT`] passes. Connecting is bounded by
//! [`CONNECT_WAIT`] and by the window's going.
//!
//! **Ids belong to the server that issued them** (its unique bus name,
//! the owner epoch). The task adds its match rules, then asks who owns
//! `org.freedesktop.Notifications` (the bus answers in order, so no owner
//! change falls between the two) and follows `NameOwnerChanged` from the
//! bus daemon. Every call to the server goes to that owner's unique name,
//! so it never reaches a successor; its capabilities (`body-markup`, how
//! the body is escaped) are asked of each owner anew. A notification's id
//! is kept with the owner that returned it. A signal counts only from
//! that owner, at the service's path, for an id it issued; a reply only
//! from the name it was asked of. When the owner changes (a restart, a
//! replacement), every id of the old one is forgotten without a call:
//! nothing is closed at, or routed from, its successor, whose counter may
//! well hand the same numbers to another app. A `Notify` in flight across
//! the change is not shown again either (told once): its id, when it
//! comes, is the old owner's, and forgotten. With no owner, the bus is
//! asked to start one (`StartServiceByName`, D-Bus activation) once per
//! new notification; still none, it is not told.
//!
//! **The residuals.** A replaced server that keeps running keeps showing
//! what it showed; its clicks are ignored and its notifications aren't
//! closed (they would have to be closed by an id the new owner doesn't
//! issue). A bus lost (the stream ends, a write fails) ends the task;
//! notifications up then stay with the server, and the next one posted
//! connects again. The process killed outright closes nothing.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant};

use futures_lite::Stream;
use gpui_kit::{App, BackgroundExecutor, Task};
use zbus::Message;
use zbus::message::Type;

use super::{Gone, NOTES_MAX, Note, Transport, xdg_body};

pub(crate) const XDG_NAME: &str = "org.freedesktop.Notifications";
pub(crate) const XDG_PATH: &str = "/org/freedesktop/Notifications";
pub(crate) const BUS: &str = "org.freedesktop.DBus";
pub(crate) const BUS_PATH: &str = "/org/freedesktop/DBus";

/// A call not answered in this long has failed.
pub(crate) const CALL_WAIT: Duration = Duration::from_secs(5);
/// The session bus not reached in this long: no notifications this time.
pub(crate) const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// At most this many calls out at once.
pub(crate) const CALLS_MAX: usize = 8;
/// Closes waiting to be sent at most (each a notification up, or one
/// whose `Notify` was out when its card resolved).
const CLOSES_MAX: usize = NOTES_MAX + CALLS_MAX;
/// Messages taken from the stream in one turn at most (then the window's
/// side, the write and the clock get their turn).
const TURN_MAX: usize = 64;

/// A message being written.
pub(crate) type Sending = Pin<Box<dyn Future<Output = zbus::Result<()>> + Send>>;

/// A connection, as the task uses it: write one message; read them all.
pub(crate) struct Wire {
    pub send: Box<dyn Fn(Message) -> Sending + Send>,
    pub recv: Pin<Box<dyn Stream<Item = zbus::Result<Message>> + Send>>,
}

/// How a task gets its connection.
pub(crate) type Dial = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = zbus::Result<Wire>> + Send>> + Send + Sync>;

/// The session bus: a connection of the task's own, and its unfiltered
/// stream.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn session() -> Dial {
    Arc::new(|| {
        Box::pin(async {
            let conn = zbus::Connection::session().await?;
            let recv = Box::pin(zbus::MessageStream::from(&conn));
            let send = Box::new(move |m: Message| -> Sending {
                let c = conn.clone();
                Box::pin(async move { c.send(&m).await })
            });
            Ok(Wire { send, recv })
        })
    })
}

/// The window's side: the notes it wants up, for its task to follow.
pub(crate) struct Xdg {
    dial: Dial,
    clicks: async_channel::Sender<String>,
    link: Option<Link>,
}

/// A running task, as the window holds it.
struct Link {
    want: Arc<Mutex<Vec<Note>>>,
    /// The task woken (one wake pending at most); dropped, the window is
    /// gone.
    wake: async_channel::Sender<()>,
    /// Closed when the task ends.
    gone: async_channel::Receiver<()>,
}

impl Xdg {
    pub(crate) fn new(dial: Dial, clicks: async_channel::Sender<String>) -> Self {
        Self { dial, clicks, link: None }
    }

    /// The running task; one started if none runs (the first notification,
    /// or the last task's bus was lost: it starts with nothing wanted but
    /// what comes now, so nothing is told twice).
    fn link(&mut self, cx: &mut App) -> &Link {
        if self.link.as_ref().is_none_or(|l| l.wake.is_closed()) {
            let want = Arc::new(Mutex::new(Vec::new()));
            let (wake, woken) = async_channel::bounded(1);
            let (alive, gone) = async_channel::bounded(1);
            let ex = cx.background_executor().clone();
            ex.spawn(run(self.dial.clone(), want.clone(), woken, self.clicks.clone(), ex.clone(), alive)).detach();
            self.link = Some(Link { want, wake, gone });
        }
        self.link.as_ref().expect("a task runs")
    }
}

impl Transport for Xdg {
    fn show(&mut self, n: Note, cx: &mut App) {
        let l = self.link(cx);
        {
            let mut w = l.want.lock().unwrap_or_else(PoisonError::into_inner);
            w.retain(|o| o.tag != n.tag);
            w.push(n);
            let extra = w.len().saturating_sub(NOTES_MAX);
            w.drain(..extra);
        }
        let _ = l.wake.try_send(());
    }

    fn close(&mut self, tag: &str, _: &mut App) {
        if let Some(l) = &self.link {
            l.want.lock().unwrap_or_else(PoisonError::into_inner).retain(|o| o.tag != tag);
            let _ = l.wake.try_send(());
        }
    }

    /// The task told the window is gone (nothing wanted, its wake
    /// dropped): it closes what is up and ends.
    fn retire(&mut self, _: &mut App) -> Gone {
        Gone(self.link.take().map(|l| {
            l.want.lock().unwrap_or_else(PoisonError::into_inner).clear();
            l.gone
        }))
    }

    #[cfg(test)]
    fn queued(&self) -> usize {
        self.link.as_ref().map_or(0, |l| l.want.lock().unwrap_or_else(PoisonError::into_inner).len())
    }
}

/// What one turn of the task brought.
#[derive(Default)]
struct Turn {
    sent: Option<zbus::Result<()>>,
    heard: Vec<zbus::Result<Message>>,
    ended: bool,
    /// `Some(false)`: the window is gone.
    woke: Option<bool>,
    late: bool,
}

/// The bus task (module docs): connects, then turns until the window is
/// gone and its notifications are closed, or the bus is lost.
pub(crate) async fn run(
    dial: Dial,
    want: Arc<Mutex<Vec<Note>>>,
    woken: async_channel::Receiver<()>,
    clicks: async_channel::Sender<String>,
    ex: BackgroundExecutor,
    _alive: async_channel::Sender<()>,
) {
    use futures_lite::future::or;
    let wire = or(
        async { Some(dial().await) },
        or(
            async {
                while woken.recv().await.is_ok() {}
                None
            },
            async {
                ex.timer(CONNECT_WAIT).await;
                None
            },
        ),
    )
    .await;
    let Wire { send, mut recv } = match wire {
        Some(Ok(w)) => w,
        Some(Err(e)) => return log::warn!(target: "synaps_desktop::card", "no session bus for notifications: {e}"),
        None => return log::info!(target: "synaps_desktop::card", "notifications: no session bus yet, or the window went"),
    };
    let wake = |w: &async_channel::Receiver<()>| -> Pin<Box<dyn Future<Output = bool> + Send>> {
        let w = w.clone();
        Box::pin(async move { w.recv().await.is_ok() })
    };
    let mut bus = Bus::new(ex.now());
    let mut woke = Some(wake(&woken));
    let mut fresh = true;
    let mut sending: Option<Sending> = None;
    let mut timer: Option<(Instant, Task<()>)> = None;
    loop {
        let now = ex.now();
        if fresh {
            fresh = false;
            // The window gone since it woke the task: nothing more shown.
            if woken.is_closed() {
                woke = None;
                bus.leave(now);
            } else {
                let w = want.lock().unwrap_or_else(PoisonError::into_inner).clone();
                bus.want(&w, now);
            }
        }
        for tag in bus.clicked.drain(..) {
            let _ = clicks.try_send(tag);
        }
        if sending.is_none()
            && let Some(m) = bus.out.pop_front()
        {
            sending = Some(send(m));
        }
        if sending.is_none() && bus.done() {
            return;
        }
        let due = bus.deadline();
        if timer.as_ref().map(|t| t.0) != due {
            timer = due.map(|d| (d, ex.timer(d.saturating_duration_since(now))));
        }
        let turn = std::future::poll_fn(|cx| {
            let mut t = Turn::default();
            if let Some(s) = sending.as_mut()
                && let Poll::Ready(r) = s.as_mut().poll(cx)
            {
                t.sent = Some(r);
            }
            while t.heard.len() < TURN_MAX {
                match recv.as_mut().poll_next(cx) {
                    Poll::Ready(Some(m)) => t.heard.push(m),
                    Poll::Ready(None) => {
                        t.ended = true;
                        break;
                    }
                    Poll::Pending => break,
                }
            }
            if let Some(w) = woke.as_mut()
                && let Poll::Ready(b) = w.as_mut().poll(cx)
            {
                t.woke = Some(b);
            }
            if let Some((_, task)) = timer.as_mut()
                && Pin::new(task).poll(cx).is_ready()
            {
                t.late = true;
            }
            if t.sent.is_some() || !t.heard.is_empty() || t.ended || t.woke.is_some() || t.late { Poll::Ready(t) } else { Poll::Pending }
        })
        .await;
        let now = ex.now();
        if let Some(r) = turn.sent {
            sending = None;
            if let Err(e) = r {
                return log::warn!(target: "synaps_desktop::card", "notifications: the session bus took no more: {e}");
            }
        }
        for m in turn.heard {
            match m {
                Ok(m) => bus.heard(&m, now),
                Err(e) => log::debug!(target: "synaps_desktop::card", "notifications: {e}"),
            }
        }
        if turn.ended {
            return log::info!(target: "synaps_desktop::card", "session bus gone: notifications end");
        }
        match turn.woke {
            Some(true) => {
                fresh = true;
                woke = Some(wake(&woken));
            }
            Some(false) => {
                woke = None;
                bus.leave(now);
            }
            None => {}
        }
        if turn.late {
            timer = None;
            bus.late(now);
        }
    }
}

/// Who owns the service, as far as the task knows.
#[derive(Debug)]
enum Owner {
    /// Not known: `GetNameOwner` to send (`false`) or sent.
    Ask(bool),
    None,
    Some(Epoch),
}

/// One owner of the service: its unique name, and what it said it does.
#[derive(Debug)]
struct Epoch {
    name: String,
    caps: Caps,
}

#[derive(Debug)]
enum Caps {
    /// Not known: `GetCapabilities` to send (`false`) or sent.
    Ask(bool),
    /// Whether it reads a body as markup.
    Markup(bool),
}

/// A call out: its serial, whom it went to, until when its reply counts.
#[derive(Debug)]
struct Call {
    serial: NonZeroU32,
    to: String,
    until: Instant,
    what: What,
}

#[derive(Debug)]
enum What {
    Rule,
    Owner,
    Start,
    Caps(String),
    Notify { tag: String, owner: String },
    Close,
}

/// A note's notification: queued, its `Notify` out, up (by the id its
/// issuer gave it), or done (told once: shown and closed since, failed,
/// or its issuer gone).
#[derive(Debug)]
enum State {
    Queued,
    Sending,
    Up { owner: String, id: u32 },
    Done,
}

#[derive(Debug)]
struct Tagged {
    note: Note,
    wanted: bool,
    state: State,
}

/// The task's state, without I/O: what it hears in, what it writes out.
#[derive(Debug)]
pub(crate) struct Bus {
    owner: Owner,
    calls: Vec<Call>,
    /// Built and waiting to be written, oldest first.
    pub(crate) out: VecDeque<Message>,
    tags: Vec<Tagged>,
    /// Ids to close, each at the owner that issued it.
    closes: VecDeque<(String, u32)>,
    /// Tags clicked, for the window.
    pub(crate) clicked: Vec<String>,
    /// `StartServiceByName` tried for what is queued.
    started: bool,
    /// The window is gone.
    leaving: bool,
}

impl Bus {
    /// The match rules, then who owns the service.
    fn new(now: Instant) -> Self {
        let mut b = Bus {
            owner: Owner::Ask(false),
            calls: Vec::new(),
            out: VecDeque::new(),
            tags: Vec::new(),
            closes: VecDeque::new(),
            clicked: Vec::new(),
            started: false,
            leaving: false,
        };
        let svc = format!("type='signal',sender='{XDG_NAME}',path='{XDG_PATH}',interface='{XDG_NAME}'");
        for rule in [
            format!("{svc},member='ActionInvoked'"),
            format!("{svc},member='NotificationClosed'"),
            format!("type='signal',sender='{BUS}',path='{BUS_PATH}',interface='{BUS}',member='NameOwnerChanged',arg0='{XDG_NAME}'"),
        ] {
            b.call(BUS, BUS_PATH, BUS, "AddMatch", &rule, What::Rule, now);
        }
        b.pump(now);
        b
    }

    /// The window wants `notes` up: new ones queued, those it no longer
    /// wants closed (or dropped, never sent).
    fn want(&mut self, notes: &[Note], now: Instant) {
        let mut kept = Vec::with_capacity(self.tags.len());
        for mut t in self.tags.drain(..) {
            t.wanted = notes.iter().any(|n| n.tag == t.note.tag);
            match t.state {
                _ if t.wanted => kept.push(t),
                State::Up { owner, id } => self.closes.push_back((owner, id)),
                // Closed when its id comes.
                State::Sending => kept.push(t),
                State::Queued | State::Done => {}
            }
        }
        self.tags = kept;
        for n in notes {
            if !self.tags.iter().any(|t| t.note.tag == n.tag) {
                self.tags.push(Tagged { note: n.clone(), wanted: true, state: State::Queued });
                self.started = false;
            }
        }
        self.pump(now);
    }

    /// The window is gone: nothing more is shown; what is up is closed.
    fn leave(&mut self, now: Instant) {
        self.leaving = true;
        let mut kept = Vec::new();
        for mut t in self.tags.drain(..) {
            t.wanted = false;
            match t.state {
                State::Up { owner, id } => self.closes.push_back((owner, id)),
                State::Sending => kept.push(t),
                State::Queued | State::Done => {}
            }
        }
        self.tags = kept;
        self.pump(now);
    }

    /// Done: the window gone and nothing left out or to send.
    fn done(&self) -> bool {
        self.leaving && self.calls.is_empty() && self.out.is_empty() && self.closes.is_empty()
    }

    /// When the next call out fails, unanswered.
    fn deadline(&self) -> Option<Instant> {
        self.calls.iter().map(|c| c.until).min()
    }

    /// The calls past [`CALL_WAIT`] failed; one not yet written dropped.
    fn late(&mut self, now: Instant) {
        let (late, out): (Vec<Call>, Vec<Call>) = self.calls.drain(..).partition(|c| c.until <= now);
        self.calls = out;
        for c in late {
            self.out.retain(|m| m.primary_header().serial_num() != c.serial);
            log::warn!(target: "synaps_desktop::card", "notifications: {:?} to {} unanswered", c.what, c.to);
            self.answered(c, Err("unanswered".into()), now);
        }
        self.pump(now);
    }

    /// One message from the bus.
    fn heard(&mut self, m: &Message, now: Instant) {
        let h = m.header();
        match m.message_type() {
            Type::MethodReturn | Type::Error => {
                let Some(serial) = h.reply_serial() else { return };
                let Some(ix) = self.calls.iter().position(|c| c.serial == serial) else { return };
                // A reply only from whom it was asked; an error also from
                // the bus daemon (nobody there to deliver it to).
                let from = h.sender().map(|s| s.as_str());
                if from != Some(self.calls[ix].to.as_str()) && !(m.message_type() == Type::Error && from == Some(BUS)) {
                    return;
                }
                let c = self.calls.remove(ix);
                let r = if m.message_type() == Type::MethodReturn {
                    Ok(m)
                } else {
                    Err(h.error_name().map_or_else(|| "an error".to_string(), |e| e.to_string()))
                };
                self.answered(c, r, now);
            }
            Type::Signal => self.signal(m, now),
            Type::MethodCall => {}
        }
        self.pump(now);
    }

    fn signal(&mut self, m: &Message, now: Instant) {
        let h = m.header();
        let (Some(from), Some(path), Some(iface), Some(member)) = (h.sender(), h.path(), h.interface(), h.member()) else { return };
        let (from, path) = (from.as_str(), path.as_str());
        match (iface.as_str(), member.as_str()) {
            (BUS, "NameOwnerChanged") if from == BUS && path == BUS_PATH => {
                if let Ok((name, _old, new)) = m.body().deserialize::<(String, String, String)>()
                    && name == XDG_NAME
                {
                    self.owned_by((!new.is_empty()).then_some(new), now);
                }
            }
            (XDG_NAME, "ActionInvoked") if path == XDG_PATH => {
                if let Ok((id, _action)) = m.body().deserialize::<(u32, String)>()
                    && let Some(t) = self.up(from, id)
                {
                    let tag = t.note.tag.clone();
                    self.clicked.push(tag);
                }
            }
            (XDG_NAME, "NotificationClosed") if path == XDG_PATH => {
                if let Ok((id, _why)) = m.body().deserialize::<(u32, u32)>()
                    && let Some(t) = self.up(from, id)
                {
                    t.state = State::Done;
                }
            }
            _ => {}
        }
    }

    /// The notification `owner` issued as `id`, while up.
    fn up(&mut self, owner: &str, id: u32) -> Option<&mut Tagged> {
        self.tags.iter_mut().find(|t| matches!(&t.state, State::Up { owner: o, id: i } if o == owner && *i == id))
    }

    /// The service's owner is now `new` (`None`: nobody). Every id of the
    /// one before is forgotten, with no call: none of them means anything
    /// to `new`. `new`'s capabilities are asked anew.
    fn owned_by(&mut self, new: Option<String>, now: Instant) {
        let same = match (&self.owner, &new) {
            (Owner::Some(e), Some(n)) => e.name == *n,
            (Owner::None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        log::info!(target: "synaps_desktop::card", "notification service owner: {}", new.as_deref().unwrap_or("none"));
        for t in &mut self.tags {
            if matches!(t.state, State::Up { .. }) {
                t.state = State::Done;
            }
        }
        self.closes.clear();
        self.owner = match new {
            Some(name) => {
                self.started = false;
                Owner::Some(Epoch { name, caps: Caps::Ask(false) })
            }
            None => Owner::None,
        };
        self.pump(now);
    }

    /// Call `c`'s reply (`Err`: an error, or none in time).
    fn answered(&mut self, c: Call, r: Result<&Message, String>, now: Instant) {
        match c.what {
            What::Rule => {
                if let Err(e) = r {
                    log::warn!(target: "synaps_desktop::card", "notification signals: {e}");
                }
            }
            What::Owner => match r {
                Ok(m) => self.owned_by(m.body().deserialize::<String>().ok(), now),
                Err(_) => self.owned_by(None, now),
            },
            What::Start => {
                if let Err(e) = r {
                    log::info!(target: "synaps_desktop::card", "no notification service to start: {e}");
                }
                if matches!(self.owner, Owner::None) {
                    self.owner = Owner::Ask(false);
                }
            }
            What::Caps(name) => {
                if let Owner::Some(e) = &mut self.owner
                    && e.name == name
                {
                    // Not said: escaped, as for markup (literal entities at
                    // worst, never markup read).
                    let markup =
                        r.map_or(true, |m| m.body().deserialize::<Vec<String>>().map_or(true, |c| c.iter().any(|c| c == "body-markup")));
                    e.caps = Caps::Markup(markup);
                }
            }
            What::Notify { tag, owner } => {
                let id = r.as_ref().ok().and_then(|m| m.body().deserialize::<u32>().ok());
                if let Err(e) = &r {
                    log::warn!(target: "synaps_desktop::card", "notify {tag}: {e}");
                }
                let current = matches!(&self.owner, Owner::Some(e) if e.name == owner);
                let ix = self.tags.iter().position(|t| t.note.tag == tag && matches!(t.state, State::Sending));
                let wanted = ix.is_some_and(|ix| self.tags[ix].wanted) && !self.leaving;
                match (id, ix) {
                    (Some(id), Some(ix)) if current && wanted => self.tags[ix].state = State::Up { owner, id },
                    (id, ix) => {
                        // Not wanted now: closed at its issuer, if that is
                        // still the owner; an old owner's id is forgotten.
                        if let Some(id) = id
                            && current
                            && !wanted
                        {
                            self.closes.push_back((owner, id));
                        }
                        if let Some(ix) = ix {
                            if self.tags[ix].wanted && !self.leaving {
                                self.tags[ix].state = State::Done;
                            } else {
                                self.tags.remove(ix);
                            }
                        }
                    }
                }
            }
            What::Close => {}
        }
    }

    /// As many calls out as [`CALLS_MAX`] allows: closes first, then who
    /// owns the service and what it does, then the queued notes (or, with
    /// no owner, one try at starting one).
    fn pump(&mut self, now: Instant) {
        let extra = self.closes.len().saturating_sub(CLOSES_MAX);
        if extra > 0 {
            log::warn!(target: "synaps_desktop::card", "notifications: {extra} closes dropped");
            self.closes.drain(..extra);
        }
        while self.calls.len() < CALLS_MAX {
            if let Some((owner, id)) = self.closes.pop_front() {
                self.call(&owner, XDG_PATH, XDG_NAME, "CloseNotification", &id, What::Close, now);
                continue;
            }
            if self.leaving {
                return;
            }
            let queued = self.tags.iter().position(|t| matches!(t.state, State::Queued));
            match &mut self.owner {
                Owner::Ask(sent @ false) => {
                    *sent = true;
                    self.call(BUS, BUS_PATH, BUS, "GetNameOwner", &XDG_NAME, What::Owner, now);
                }
                Owner::Some(Epoch { name, caps: Caps::Ask(sent @ false) }) => {
                    *sent = true;
                    let name = name.clone();
                    self.call(&name, XDG_PATH, XDG_NAME, "GetCapabilities", &(), What::Caps(name.clone()), now);
                }
                Owner::Some(Epoch { name, caps: Caps::Markup(markup) }) => {
                    let Some(ix) = queued else { return };
                    let (name, markup) = (name.clone(), *markup);
                    self.tags[ix].state = State::Sending;
                    let n = self.tags[ix].note.clone();
                    let hints: HashMap<&str, zbus::Variant> = HashMap::new();
                    let body = ("Synaps", 0u32, "", n.title.as_str(), xdg_body(&n.body, markup), vec!["default", "Open"], hints, -1i32);
                    self.call(&name, XDG_PATH, XDG_NAME, "Notify", &body, What::Notify { tag: n.tag, owner: name.clone() }, now);
                }
                Owner::None => {
                    if queued.is_none() || self.calls.iter().any(|c| matches!(c.what, What::Start)) {
                        return;
                    }
                    if self.started {
                        // Asked to start one, and still nobody: not told.
                        for t in &mut self.tags {
                            if matches!(t.state, State::Queued) {
                                log::info!(target: "synaps_desktop::card", "notify {}: no notification service", t.note.tag);
                                t.state = State::Done;
                            }
                        }
                        return;
                    }
                    self.started = true;
                    self.call(BUS, BUS_PATH, BUS, "StartServiceByName", &(XDG_NAME, 0u32), What::Start, now);
                }
                Owner::Ask(true) | Owner::Some(Epoch { caps: Caps::Ask(true), .. }) => return,
            }
        }
    }

    /// A call written out: remembered by serial until its reply, or
    /// [`CALL_WAIT`].
    #[allow(clippy::too_many_arguments)]
    fn call<B>(&mut self, to: &str, path: &str, iface: &str, member: &str, body: &B, what: What, now: Instant)
    where
        B: serde::Serialize + zbus::DynamicType,
    {
        match Message::method_call(path, member).destination(to).interface(iface).build(body) {
            Ok(m) => {
                self.calls.push(Call { serial: m.primary_header().serial_num(), to: to.to_string(), until: now + CALL_WAIT, what });
                self.out.push_back(m);
            }
            // Not built (a name the bus gave that zbus refuses): that note
            // is not told, that owner's body escaped; nothing else retried.
            Err(e) => {
                log::warn!(target: "synaps_desktop::card", "notifications: {member}: {e}");
                match what {
                    What::Notify { tag, .. } => {
                        if let Some(ix) = self.tags.iter().position(|t| t.note.tag == tag && matches!(t.state, State::Sending)) {
                            self.tags[ix].state = State::Done;
                        }
                    }
                    What::Caps(name) => {
                        if let Owner::Some(e) = &mut self.owner
                            && e.name == name
                        {
                            e.caps = Caps::Markup(true);
                        }
                    }
                    What::Rule | What::Owner | What::Start | What::Close => {}
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "notify_xdg_tests.rs"]
mod tests;
