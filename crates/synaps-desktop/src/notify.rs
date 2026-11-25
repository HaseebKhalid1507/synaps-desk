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
//! summary is plain text by the spec; on Linux nothing in it reads as a
//! link either ([`xdg_summary`]: dunst offers to open the URLs it finds
//! in a summary). How a server formats a summary (dunst's `markup =
//! full` styles tags) is its own, outside this guarantee; so is what it
//! does with links in the body, which is the question as the card shows
//! it.
//!
//! **The transports** ([`Transport`]), each one the window's, taken down
//! with it ([`Notes::teardown`], from the view's release and the app's
//! quit):
//! - macOS and Windows: GPUI's own (`show_system_notification`, and
//!   `dismiss_system_notification` when the card resolves or the window
//!   goes: the delivered notification or toast is removed). [`Gpui`]
//!   queues them and hands them to the platform from a task of its own,
//!   one call per turn of the UI thread, never from a frame; what is
//!   still queued when the window goes is never shown. The residual
//!   (Shady #83 R2-8): GPUI keeps the platform's notification state on
//!   the UI thread (a `RefCell` in the platform, not `Send`), so each
//!   call still runs there, outside any frame. On Windows that is the
//!   toast's XML document and `ToastNotifier::Show`/`Hide` (COM calls),
//!   and, the first time, `CreateToastNotifierWithId` and an unpackaged
//!   app's `HKCU\Software\Classes\AppUserModelId\synaps-desktop`
//!   registry write. GPUI offers no other thread for them.
//! - Linux: the XDG notifications service on the session bus, directly
//!   ([`xdg`]): GPUI's Linux backend keeps a thread per notification
//!   waiting for its click and cannot close one. One task per window owns
//!   a bus connection and never stops reading it; ids are the issuing
//!   server's own (its unique bus name), never another's. See [`xdg`].

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

/// A Linux notification's summary: plain text by the spec, and nothing
/// in it a server can take for a link (Shady #89 N3). dunst gathers the
/// URLs of a summary and a body (`<a href>`, `<img src>`, and any text
/// from `http://`, `https://`, `ftp://`, `ftps://`, `news://`, `file://`,
/// `mailto:` or `www.`) and offers to open them. A word joiner (U+2060:
/// no width, no break) after every `:`, `.` and `<` breaks each of those
/// patterns and shows the same text.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn xdg_summary(title: &str) -> String {
    let mut out = String::with_capacity(title.len() + 8);
    for ch in title.chars() {
        out.push(ch);
        if matches!(ch, ':' | '.' | '<') {
            out.push('\u{2060}');
        }
    }
    out
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
    /// The window is going: every notification it posted is taken down,
    /// and nothing still queued is shown after. [`Gone`] is ready once
    /// that is done (at once where it is done here).
    fn retire(&mut self, cx: &mut App) -> Gone;

    /// How many notes wait in the transport's own queue (a test's look).
    #[cfg(test)]
    fn queued(&self) -> usize {
        0
    }
}

/// A transport's teardown, done: ready at once, or (Linux) when its bus
/// task has closed the window's notifications and ended.
#[must_use]
pub(crate) struct Gone(pub(crate) Option<async_channel::Receiver<()>>);

impl Gone {
    pub(crate) async fn wait(self) {
        if let Some(rx) = self.0 {
            // Nothing is ever sent: the task's end closes the channel.
            let _ = rx.recv().await;
        }
    }

    pub(crate) fn done(&self) -> bool {
        self.0.as_ref().is_none_or(async_channel::Receiver::is_closed)
    }
}

/// The teardowns of windows already released that are still closing
/// their notifications (Linux's bus tasks): the app's quit waits for them
/// too, within GPUI's shutdown allowance ([`closing`]).
#[derive(Default)]
struct Closing(Vec<Gone>);

impl Global for Closing {}

/// A released window's teardown, `gone`, kept for the app's quit to wait
/// on while it is not done (the last window's release comes just before
/// the quit, with its bus task still closing).
pub(crate) fn closing(gone: Gone, cx: &mut App) {
    if gone.done() {
        return;
    }
    if !cx.has_global::<Closing>() {
        cx.on_app_quit(|cx| {
            let all = std::mem::take(&mut cx.default_global::<Closing>().0);
            async move {
                for g in all {
                    g.wait().await;
                }
            }
        })
        .detach();
    }
    let c = cx.default_global::<Closing>();
    c.0.retain(|g| !g.done());
    c.0.push(gone);
}

/// GPUI's notifications (macOS, Windows; the test platform): queued, then
/// handed to the platform from a task of their own, one call per turn of
/// the UI thread, never from a frame.
#[cfg(not(all(target_os = "linux", not(test))))]
#[derive(Default)]
pub(crate) struct Gpui {
    q: std::rc::Rc<std::cell::RefCell<Queue>>,
}

/// What [`Gpui`] has to do, and what it did.
#[cfg(not(all(target_os = "linux", not(test))))]
#[derive(Default)]
struct Queue {
    /// Posted, not yet handed to the platform.
    show: VecDeque<Note>,
    /// Handed to the platform: each one dismissed when its card resolves
    /// or the window goes.
    up: Vec<String>,
    /// To dismiss.
    dismiss: VecDeque<String>,
    /// A task is working through it.
    draining: bool,
    /// The window is gone: nothing more is shown.
    gone: bool,
}

#[cfg(not(all(target_os = "linux", not(test))))]
enum Op {
    Show(Note),
    Dismiss(String),
}

#[cfg(not(all(target_os = "linux", not(test))))]
impl Gpui {
    /// The queue worked through by one task: one platform call per turn,
    /// the window's going checked before each.
    fn drain(&self, cx: &mut App) {
        if std::mem::replace(&mut self.q.borrow_mut().draining, true) {
            return;
        }
        let q = self.q.clone();
        cx.spawn(async move |cx| {
            loop {
                let op = {
                    let mut q = q.borrow_mut();
                    let op = if q.gone {
                        None
                    } else if let Some(t) = q.dismiss.pop_front() {
                        Some(Op::Dismiss(t))
                    } else {
                        q.show.pop_front().map(|n| {
                            q.up.push(n.tag.clone());
                            Op::Show(n)
                        })
                    };
                    if op.is_none() {
                        q.draining = false;
                    }
                    op
                };
                let Some(op) = op else { return };
                cx.update(|cx| match op {
                    Op::Dismiss(t) => cx.dismiss_system_notification(&t),
                    Op::Show(n) => cx.show_system_notification(SystemNotification {
                        tag: n.tag.into(),
                        title: n.title.into(),
                        body: n.body.into(),
                        actions: Vec::new(),
                    }),
                });
                // The next call in a later turn: a frame may come between.
                yield_now().await;
            }
        })
        .detach();
    }
}

#[cfg(not(all(target_os = "linux", not(test))))]
impl Transport for Gpui {
    fn show(&mut self, n: Note, cx: &mut App) {
        self.q.borrow_mut().show.push_back(n);
        self.drain(cx);
    }

    fn close(&mut self, tag: &str, cx: &mut App) {
        let mut q = self.q.borrow_mut();
        if let Some(ix) = q.show.iter().position(|n| n.tag == tag) {
            // Never shown: nothing to take down.
            q.show.remove(ix);
            return;
        }
        let Some(ix) = q.up.iter().position(|t| t == tag) else { return };
        let t = q.up.remove(ix);
        q.dismiss.push_back(t);
        drop(q);
        self.drain(cx);
    }

    /// Every notification handed to the platform dismissed now (the window
    /// is going: there is no later turn of its own); the queue dropped.
    fn retire(&mut self, cx: &mut App) -> Gone {
        let down: Vec<String> = {
            let mut q = self.q.borrow_mut();
            q.gone = true;
            q.show.clear();
            let up = std::mem::take(&mut q.up);
            q.dismiss.drain(..).chain(up).collect()
        };
        for t in down {
            cx.dismiss_system_notification(&t);
        }
        Gone(None)
    }
}

/// One turn given back to the executor.
#[cfg(not(all(target_os = "linux", not(test))))]
async fn yield_now() {
    let mut once = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut once, true) {
            std::task::Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await
}

/// The notifications a window posted: each one's card, oldest first.
pub(crate) struct Notes {
    posted: VecDeque<(String, Key)>,
    transport: Box<dyn Transport>,
    /// The clicks a transport of the window's own hears (Linux's), for
    /// [`crate::presence::listen`] to take.
    clicks: Option<async_channel::Receiver<String>>,
    /// Torn down with its window: nothing is posted again.
    gone: bool,
}

impl std::fmt::Debug for Notes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notes").field("posted", &self.posted).field("gone", &self.gone).finish_non_exhaustive()
    }
}

impl Default for Notes {
    /// The platform's transport.
    fn default() -> Self {
        #[cfg(all(target_os = "linux", not(test)))]
        {
            let (tx, rx) = async_channel::bounded(NOTES_MAX);
            Self::new(Box::new(xdg::Xdg::new(xdg::session(), tx)), Some(rx))
        }
        #[cfg(not(all(target_os = "linux", not(test))))]
        Self::new(Box::new(Gpui::default()), None)
    }
}

impl Notes {
    pub(crate) fn new(transport: Box<dyn Transport>, clicks: Option<async_channel::Receiver<String>>) -> Self {
        Self { posted: VecDeque::new(), transport, clicks, gone: false }
    }

    pub(crate) fn take_clicks(&mut self) -> Option<async_channel::Receiver<String>> {
        self.clicks.take()
    }

    /// The OS told of card `k` (once: a tag up stays as it is); the
    /// oldest taken down past [`NOTES_MAX`]. Nothing once torn down.
    pub(crate) fn post(&mut self, k: Key, n: Note, cx: &mut App) {
        if self.gone || self.posted.iter().any(|(t, _)| *t == n.tag) {
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

    /// The window is going (Shady #83 R2-3): every notification it posted
    /// taken down by its transport, nothing queued shown after, nothing
    /// posted again; the map forgotten.
    pub(crate) fn teardown(&mut self, cx: &mut App) -> Gone {
        if std::mem::replace(&mut self.gone, true) {
            return Gone(None);
        }
        self.posted.clear();
        self.transport.retire(cx)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.posted.len()
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.transport.queued()
    }
}

/// Linux's transport: the XDG notifications service on the session bus.
#[cfg(target_os = "linux")]
#[path = "notify_xdg.rs"]
pub(crate) mod xdg;

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
