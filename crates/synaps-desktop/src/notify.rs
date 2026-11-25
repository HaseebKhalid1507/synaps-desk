//! The OS notifications of wait cards (F12, [`crate::presence`]) as owned
//! resources: each one the window posts is its card's, kept in
//! [`Notes`] until that card resolves (answered, closed, replaced, gone
//! with its session's tab), and then taken down; at most [`NOTES_MAX`]
//! at once, the oldest taken down first.
//!
//! **The text** ([`note`]): the session's name and the question, each
//! one line, sanitized as a card's text is (no controls, no bidi or
//! default-ignorable marks) and bounded. On Linux the body is markup to a
//! server that says so (`body-markup`): there `&`, `<` and `>` are
//! escaped ([`xdg_body`]); macOS and Windows draw text as given. The
//! summary is plain text by the spec, sent as it is: a server that
//! formats it as markup anyway (dunst's `markup = full`) may style a
//! session name's tags, never act on them.
//!
//! **The transports** ([`Transport`]):
//! - macOS and Windows: GPUI's own (`show_system_notification`, and
//!   `dismiss_system_notification` when the card resolves: the delivered
//!   notification or toast is removed). Called from a task of its own,
//!   never from a frame.
//! - Linux: the XDG notifications service on the session bus, directly
//!   ([`Xdg`]): GPUI's Linux backend keeps a thread per notification
//!   waiting for its click and cannot close one. Here one task per
//!   window owns the bus connection: it sends `Notify`, sends
//!   `CloseNotification` by the server's id when the card resolves, and
//!   hears every `ActionInvoked` and `NotificationClosed` on one signal
//!   stream. No thread waits per notification and nothing polls; what it
//!   remembers is bounded ([`Shown`]). The residual: a server that dies
//!   with notifications up never says they closed. They stay remembered
//!   until their card resolves or [`NOTES_MAX`] newer ones push them
//!   out, and their `CloseNotification` goes to the new server, which
//!   ignores an id it doesn't know. A bus lost ends the task; the next
//!   notification connects again.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

use cards_spec::Lines;
use gpui_kit::*;

use crate::presence::Key;

/// At most this many of a window's notifications are up at once.
pub(crate) const NOTES_MAX: usize = 16;

/// A session's name in a notification's summary: at most this long.
const LABEL_MAX: usize = 48;

/// What the OS is told of one card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Note {
    pub tag: String,
    pub title: String,
    pub body: String,
}

/// A card's notification tag: short (Windows keeps 64 characters of one)
/// and the same for the same binding.
pub(crate) fn tag_of(k: &Key) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&k.session, &k.tool_id, k.card.as_str()).hash(&mut h);
    format!("synaps-card-{:016x}", h.finish())
}

/// A session's name as a notification says it: one line, sanitized,
/// bounded; "a session" if nothing is left.
pub(crate) fn label(s: &str) -> String {
    let l = cards_spec::sanitize::cut_chars(crate::ask::shown(s, Lines::Single).trim(), LABEL_MAX);
    if l.trim().is_empty() { "a session".into() } else { l }
}

/// Wait card `k`'s notification: NEEDS YOU, its session's name, its
/// question (one line, bounded: [`crate::ask::question_line`]).
pub(crate) fn note(k: &Key, session: &str, question: &str) -> Note {
    Note { tag: tag_of(k), title: format!("Needs you · {}", label(session)), body: crate::ask::question_line(question) }
}

/// A Linux notification's body: escaped for a server that reads it as
/// markup (`body-markup`), as it is for one that doesn't.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn xdg_body(body: &str, markup: bool) -> String {
    if !markup {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    for ch in body.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Where notifications go.
pub(crate) trait Transport {
    fn show(&mut self, note: Note, cx: &mut App);
    fn close(&mut self, tag: &str, cx: &mut App);
}

/// GPUI's notifications (macOS, Windows; the test platform), from a task
/// of their own: a frame never waits on the OS.
#[cfg(not(all(target_os = "linux", not(test))))]
pub(crate) struct Gpui;

#[cfg(not(all(target_os = "linux", not(test))))]
impl Transport for Gpui {
    fn show(&mut self, n: Note, cx: &mut App) {
        cx.spawn(async move |cx| {
            cx.update(|cx| {
                cx.show_system_notification(SystemNotification {
                    tag: n.tag.into(),
                    title: n.title.into(),
                    body: n.body.into(),
                    actions: Vec::new(),
                })
            })
        })
        .detach();
    }

    fn close(&mut self, tag: &str, cx: &mut App) {
        let tag = tag.to_string();
        cx.spawn(async move |cx| cx.update(|cx| cx.dismiss_system_notification(&tag))).detach();
    }
}

/// The notifications a window posted: each one's card, oldest first.
pub(crate) struct Notes {
    posted: VecDeque<(String, Key)>,
    transport: Box<dyn Transport>,
    /// The clicks a transport of the window's own hears (Linux's), for
    /// [`crate::presence::listen`] to take.
    clicks: Option<async_channel::Receiver<String>>,
}

impl std::fmt::Debug for Notes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notes").field("posted", &self.posted).finish_non_exhaustive()
    }
}

impl Default for Notes {
    /// The platform's transport.
    fn default() -> Self {
        #[cfg(all(target_os = "linux", not(test)))]
        {
            let (tx, rx) = async_channel::unbounded();
            Self::new(Box::new(Xdg::new(tx)), Some(rx))
        }
        #[cfg(not(all(target_os = "linux", not(test))))]
        Self::new(Box::new(Gpui), None)
    }
}

impl Notes {
    pub(crate) fn new(transport: Box<dyn Transport>, clicks: Option<async_channel::Receiver<String>>) -> Self {
        Self { posted: VecDeque::new(), transport, clicks }
    }

    pub(crate) fn take_clicks(&mut self) -> Option<async_channel::Receiver<String>> {
        self.clicks.take()
    }

    /// The OS told of card `k` (once: a tag up stays as it is); the
    /// oldest taken down past [`NOTES_MAX`].
    pub(crate) fn post(&mut self, k: Key, n: Note, cx: &mut App) {
        if self.posted.iter().any(|(t, _)| *t == n.tag) {
            return;
        }
        while self.posted.len() >= NOTES_MAX {
            if let Some((t, _)) = self.posted.pop_front() {
                self.transport.close(&t, cx);
            }
        }
        self.posted.push_back((n.tag.clone(), k));
        self.transport.show(n, cx);
    }

    /// The card notification `tag` is of, while it is up.
    pub(crate) fn card(&self, tag: &str) -> Option<&Key> {
        self.posted.iter().find(|(t, _)| t == tag).map(|(_, k)| k)
    }

    /// Notification `tag` taken down (its card resolved, or its click was
    /// stale). Whether it was up.
    pub(crate) fn retire(&mut self, tag: &str, cx: &mut App) -> bool {
        let Some(ix) = self.posted.iter().position(|(t, _)| t == tag) else { return false };
        self.posted.remove(ix);
        self.transport.close(tag, cx);
        true
    }

    /// Every notification whose card is no longer `open` taken down.
    pub(crate) fn reconcile(&mut self, open: impl Fn(&Key) -> bool, cx: &mut App) {
        let gone: Vec<String> = self.posted.iter().filter(|(_, k)| !open(k)).map(|(t, _)| t.clone()).collect();
        for t in gone {
            self.retire(&t, cx);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.posted.len()
    }
}

/// What the XDG service shows of a window's: each tag's server id,
/// oldest first, at most [`NOTES_MAX`] (the bus task's memory; pure, so
/// it is tested without a bus).
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Default)]
pub(crate) struct Shown {
    up: VecDeque<(String, u32)>,
}

#[cfg(any(target_os = "linux", test))]
impl Shown {
    /// The id `tag` is up as (a new `Notify` replaces it).
    pub(crate) fn id(&self, tag: &str) -> Option<u32> {
        self.up.iter().find(|(t, _)| t == tag).map(|(_, i)| *i)
    }

    pub(crate) fn tag(&self, id: u32) -> Option<&str> {
        self.up.iter().find(|(_, i)| *i == id).map(|(t, _)| t.as_str())
    }

    /// `tag` is up as `id`; the ids to close to stay within [`NOTES_MAX`].
    pub(crate) fn shown(&mut self, tag: String, id: u32) -> Vec<u32> {
        self.up.retain(|(t, i)| *t != tag && *i != id);
        self.up.push_back((tag, id));
        let mut out = Vec::new();
        while self.up.len() > NOTES_MAX {
            out.extend(self.up.pop_front().map(|(_, i)| i));
        }
        out
    }

    /// `tag` taken down: its id, to close.
    pub(crate) fn forget(&mut self, tag: &str) -> Option<u32> {
        let ix = self.up.iter().position(|(t, _)| t == tag)?;
        self.up.remove(ix).map(|(_, i)| i)
    }

    /// The server closed `id` (expired, dismissed, clicked, or ours).
    pub(crate) fn closed(&mut self, id: u32) {
        self.up.retain(|(_, i)| *i != id);
    }

    /// Every id up (the window gone: each is closed).
    pub(crate) fn drain(&mut self) -> Vec<u32> {
        self.up.drain(..).map(|(_, i)| i).collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.up.len()
    }
}

/// Linux: the XDG notifications service, through one task per window
/// that owns its bus connection ([`bus`]).
#[cfg(all(target_os = "linux", not(test)))]
pub(crate) struct Xdg {
    tx: Option<async_channel::Sender<Cmd>>,
    clicks: async_channel::Sender<String>,
}

#[cfg(all(target_os = "linux", not(test)))]
enum Cmd {
    Show(Note),
    Close(String),
}

#[cfg(all(target_os = "linux", not(test)))]
impl Xdg {
    fn new(clicks: async_channel::Sender<String>) -> Self {
        Self { tx: None, clicks }
    }

    /// To the bus task; one started if none runs (the first notification,
    /// or the bus was lost).
    fn send(&mut self, cmd: Cmd, cx: &mut App) {
        if self.tx.as_ref().is_none_or(async_channel::Sender::is_closed) {
            if matches!(cmd, Cmd::Close(_)) {
                return;
            }
            let (tx, rx) = async_channel::unbounded();
            cx.background_spawn(bus(rx, self.clicks.clone())).detach();
            self.tx = Some(tx);
        }
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(cmd);
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
impl Transport for Xdg {
    fn show(&mut self, n: Note, cx: &mut App) {
        self.send(Cmd::Show(n), cx);
    }

    fn close(&mut self, tag: &str, cx: &mut App) {
        self.send(Cmd::Close(tag.to_string()), cx);
    }
}

#[cfg(all(target_os = "linux", not(test)))]
const XDG_NAME: &str = "org.freedesktop.Notifications";
#[cfg(all(target_os = "linux", not(test)))]
const XDG_PATH: &str = "/org/freedesktop/Notifications";

/// The bus task: `Notify` and `CloseNotification` as commands come, and
/// the service's signals as they come, on one connection. Ends when the
/// window's side is dropped (every notification still up is closed) or
/// the bus is lost.
#[cfg(all(target_os = "linux", not(test)))]
async fn bus(rx: async_channel::Receiver<Cmd>, clicks: async_channel::Sender<String>) {
    use futures_lite::StreamExt as _;
    use std::collections::HashMap;
    enum Next {
        Cmd(Option<Cmd>),
        Sig(Option<zbus::Result<zbus::Message>>),
    }
    let conn = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(e) => return log::warn!(target: "synaps_desktop::card", "no session bus for notifications: {e}"),
    };
    let rules = async {
        let proxy = zbus::fdo::DBusProxy::new(&conn).await?;
        for member in ["ActionInvoked", "NotificationClosed"] {
            let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(XDG_NAME).member(member).build()?;
            proxy.add_match_rule(rule).await?;
        }
        zbus::Result::Ok(())
    };
    let mut stream = zbus::MessageStream::from(&conn);
    if let Err(e) = rules.await {
        return log::warn!(target: "synaps_desktop::card", "notification signals: {e}");
    }
    let markup = match conn.call_method(Some(XDG_NAME), XDG_PATH, Some(XDG_NAME), "GetCapabilities", &()).await {
        Ok(m) => m.body().deserialize::<Vec<String>>().is_ok_and(|c| c.iter().any(|c| c == "body-markup")),
        Err(e) => return log::warn!(target: "synaps_desktop::card", "no notification service: {e}"),
    };
    let mut shown = Shown::default();
    let close = async |id: u32| {
        if let Err(e) = conn.call_method(Some(XDG_NAME), XDG_PATH, Some(XDG_NAME), "CloseNotification", &(id,)).await {
            log::debug!(target: "synaps_desktop::card", "close notification {id}: {e}");
        }
    };
    loop {
        let next = futures_lite::future::or(async { Next::Cmd(rx.recv().await.ok()) }, async { Next::Sig(stream.next().await) }).await;
        match next {
            Next::Cmd(None) => {
                for id in shown.drain() {
                    close(id).await;
                }
                return;
            }
            Next::Cmd(Some(Cmd::Show(n))) => {
                let hints: HashMap<&str, zbus::Variant> = HashMap::new();
                let body = (
                    "Synaps",
                    shown.id(&n.tag).unwrap_or(0),
                    "",
                    n.title.as_str(),
                    xdg_body(&n.body, markup),
                    vec!["default", "Open"],
                    hints,
                    -1i32,
                );
                match conn.call_method(Some(XDG_NAME), XDG_PATH, Some(XDG_NAME), "Notify", &body).await {
                    Ok(m) => match m.body().deserialize::<u32>() {
                        Ok(id) => {
                            for old in shown.shown(n.tag, id) {
                                close(old).await;
                            }
                        }
                        Err(e) => log::warn!(target: "synaps_desktop::card", "notify: {e}"),
                    },
                    Err(e) => log::warn!(target: "synaps_desktop::card", "notify: {e}"),
                }
            }
            Next::Cmd(Some(Cmd::Close(tag))) => {
                if let Some(id) = shown.forget(&tag) {
                    close(id).await;
                }
            }
            Next::Sig(None) => return log::info!(target: "synaps_desktop::card", "session bus gone: notifications end"),
            Next::Sig(Some(Err(_))) => {}
            Next::Sig(Some(Ok(msg))) => {
                let h = msg.header();
                if h.message_type() != zbus::message::Type::Signal || h.interface().is_none_or(|i| i.as_str() != XDG_NAME) {
                    continue;
                }
                match h.member().map(|m| m.as_str()) {
                    Some("ActionInvoked") => {
                        if let Ok((id, _action)) = msg.body().deserialize::<(u32, String)>()
                            && let Some(tag) = shown.tag(id)
                        {
                            let _ = clicks.try_send(tag.to_string());
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((id, _why)) = msg.body().deserialize::<(u32, u32)>() {
                            shown.closed(id);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
