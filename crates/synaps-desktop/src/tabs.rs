//! Session tabs without GPUI: which sessions are open, in what order, which
//! one is on screen, and the stack Ctrl+Shift+T reopens from. One tab is one
//! session, and a session is in one tab at most: opening it again focuses
//! its tab. Only the current session tab is attached ([`crate::app`] keeps
//! the others' last transcript in a bounded cache); a background tab's
//! glyph comes from the session poll.
//!
//! Settings and Usage are tabs of their own kind ([`Kind`]): one of each
//! at most, opened at the end of the strip, never saved or reopened. While
//! one is on screen the session tab focused last stays current
//! ([`Tabs::current`]): still attached, its draft kept, and shown again as
//! it was when its tab is.
//!
//! A worker tab ([`Kind::Worker`]) shows one worker (`sa_N`) of a session:
//! it attaches that session, as its session tab would, and its view pages
//! the worker's journal. One tab per worker; it opens right after its
//! session's tab (after the worker tabs already there), follows the session
//! to a new id with it, outlives its session tab, and is never saved (a
//! handle does not survive a park or a daemon restart). The rail lists
//! sessions only. A session tab keeps the worker its view showed in place
//! ([`Tab::watching`]) while another tab is on screen.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::time::{Duration, Instant};

use synaps_client::protocol::SessionMeta;

/// Tabs Ctrl+Shift+T can bring back (closed ones, newest last).
pub const CLOSED_MAX: usize = 10;
/// Session lists (the first, then the 3 s polls) a restored tab waits for
/// its session before it closes: a daemon that has just started restores
/// its pinned sessions one by one, and says nothing about it on the wire.
pub const RESTORE_LISTS: u32 = 6;

/// Where a session opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Place {
    /// In the visible tab, replacing what it shows (a new tab if none).
    #[default]
    Current,
    /// In a new tab right after the visible one.
    New,
}

/// What a tab holds: a session, one of its workers, or one of the two
/// views (no session: nothing attached, cached, saved or listed for it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Session,
    Settings,
    Usage,
    /// A worker of `session` ([`Tab::worker`]).
    Worker,
}

impl Kind {
    /// What a view's tab says.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Session | Kind::Worker => "",
            Kind::Settings => "Settings",
            Kind::Usage => "Usage",
        }
    }

    /// It attaches its session: a session tab, or a worker tab (its
    /// worker's session). The views attach nothing.
    pub fn attaches(self) -> bool {
        matches!(self, Kind::Session | Kind::Worker)
    }
}

/// A worker tab's worker: its handle (`sa_N`), its name, and the key its
/// composer draft is kept under (`session#sa_N`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerRef {
    pub handle: String,
    pub name: String,
    /// The tool id of the call that started it, if that call's own result
    /// had named it when the tab opened: with the name, who it is (a
    /// cold-started daemon numbers its workers from 1 again).
    pub call: Option<String>,
    key: String,
}

impl WorkerRef {
    pub fn new(session: &str, handle: &str, name: &str) -> Self {
        WorkerRef { handle: handle.to_string(), name: name.to_string(), call: None, key: format!("{session}#{handle}") }
    }

    /// Who it is: its call (if known) and name, as
    /// [`crate::wtabs::same_worker`] compares them.
    pub fn who(&self) -> (Option<&str>, &str) {
        (self.call.as_deref(), &self.name)
    }

    /// Its number (`sa_7` → 7).
    pub fn id(&self) -> Option<u64> {
        self.handle.strip_prefix("sa_")?.parse().ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    /// Unique in this window, never reused (element ids, drags).
    pub id: u64,
    pub kind: Kind,
    /// `None`: a create or a resume on its way (the daemon names the
    /// session when it attaches).
    pub session: Option<String>,
    /// What the tab calls itself when the session is not listed: what it
    /// was opened as, then the session's last listed name or title.
    pub label: String,
    /// The last listed journal id and folder: a tab whose session ended
    /// resumes from them.
    pub journal: Option<String>,
    pub cwd: Option<String>,
    /// A worker tab's worker.
    pub worker: Option<WorkerRef>,
    /// A session tab left while its view showed a worker in place: that
    /// worker's handle and name, shown again when the tab is.
    pub watching: Option<(String, String)>,
}

impl Tab {
    fn new(id: u64, kind: Kind, session: Option<String>, label: String) -> Self {
        Tab { id, kind, session, label, journal: None, cwd: None, worker: None, watching: None }
    }

    /// A session tab whose create or resume has not attached yet.
    pub fn pending(&self) -> bool {
        self.kind == Kind::Session && self.session.is_none()
    }

    /// What its composer draft is kept under: its session, or a worker
    /// tab's `session#sa_N`.
    pub fn draft_key(&self) -> Option<&str> {
        match &self.worker {
            Some(w) => Some(&w.key),
            None => self.session.as_deref(),
        }
    }

    /// The tab holds this worker of this session.
    pub fn is_worker(&self, session: &str, handle: &str) -> bool {
        self.session.as_deref() == Some(session) && self.worker.as_ref().is_some_and(|w| w.handle == handle)
    }

    /// Its session is now `to` (a worker tab's draft key with it): the
    /// draft keys that changed.
    fn set_session(&mut self, to: &str) -> Option<(String, String)> {
        let moved = self.worker.as_mut().map(|w| {
            let from = std::mem::take(&mut w.key);
            *w = WorkerRef { call: w.call.take(), ..WorkerRef::new(to, &w.handle, &w.name) };
            (from, w.key.clone())
        });
        self.session = Some(to.to_string());
        moved.filter(|(a, b)| a != b)
    }
}

/// The tabs as the settings file keeps them: session ids in order, each
/// one's journal id (`""`: not known; a session restored after a daemon
/// restart can come back under it) and label (what the tab says until
/// its session is listed again), and the visible one's index. `journals`
/// and `labels` are empty, or one per session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub sessions: Vec<String>,
    pub journals: Vec<String>,
    pub labels: Vec<String>,
    pub active: Option<usize>,
}

impl Saved {
    pub fn journal(&self, i: usize) -> Option<&str> {
        self.journals.get(i).map(String::as_str).filter(|j| !j.is_empty())
    }

    pub fn label(&self, i: usize) -> Option<&str> {
        self.labels.get(i).map(String::as_str).filter(|l| !l.is_empty())
    }
}

#[derive(Debug, Default)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: Option<usize>,
    /// Closed tabs and where they were, newest last.
    closed: Vec<(usize, Tab)>,
    next_id: u64,
    /// Bumped by every change of the tabs, their order, their sessions or
    /// the visible one (the settings file, the rail's marks).
    pub generation: u64,
    /// Restored tabs no session list has shown yet (shown OPENING, not
    /// attached), and the lists seen since the restore. They close after
    /// [`RESTORE_LISTS`] lists, or once the user opens, closes or moves a
    /// tab (where they were means nothing then).
    restoring: Vec<u64>,
    restore_lists: u32,
    /// Restored tabs closed so, since the app last took the count (its
    /// notice), and whether a tab change closed them before their wait was
    /// over (their sessions may yet come back).
    pub dropped: usize,
    pub dropped_early: bool,
    /// Sessions a tab followed to a new id in place (a fallback resume of
    /// an ended session, a daemon restart), oldest first: input given back
    /// to the old id goes to that tab.
    follows: Vec<(String, String)>,
    /// The visible tab went into the one already holding its session
    /// ([`Tabs::attached`]): (its id, that tab's id), for the composer.
    merged: Option<(u64, u64)>,
    /// The session tab shown last (the current one while a view is on
    /// screen).
    last_session: Option<u64>,
    /// Worker tabs' draft keys that changed with their session (a follow,
    /// a merge): (from, to), for [`Drafts::rekey`].
    rekeys: Vec<(String, String)>,
}

impl Tabs {
    /// The worker tabs' draft keys changed since the last call ([`Tabs::rekeys`]).
    pub fn take_rekeys(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.rekeys)
    }

    fn set_session(&mut self, ix: usize, to: &str) {
        if let Some(k) = self.tabs[ix].set_session(to) {
            self.rekeys.push(k);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn visible(&self) -> Option<&Tab> {
        self.tabs.get(self.active?)
    }

    /// The visible tab's session (`None` while a view is on screen: no
    /// rail row is the one on screen then).
    pub fn visible_session(&self) -> Option<&str> {
        self.visible()?.session.as_deref()
    }

    /// The view on screen, if one is.
    pub fn view(&self) -> Option<Kind> {
        self.visible().map(|t| t.kind).filter(|k| !k.attaches())
    }

    /// The current tab is a worker tab: its worker.
    pub fn current_worker(&self) -> Option<&WorkerRef> {
        self.current().filter(|t| t.kind == Kind::Worker)?.worker.as_ref()
    }

    /// The session tab the attachment follows: the visible tab, or while a
    /// view is on screen the session tab shown last.
    pub fn current(&self) -> Option<&Tab> {
        self.tabs.get(self.current_index()?)
    }

    fn current_index(&self) -> Option<usize> {
        let a = self.active?;
        if self.tabs[a].kind.attaches() {
            return Some(a);
        }
        let id = self.last_session?;
        self.tabs.iter().position(|t| t.id == id)
    }

    /// The current session tab's session.
    pub fn current_session(&self) -> Option<&str> {
        self.current()?.session.as_deref()
    }

    /// The tab the composer types for (its id and session): the current
    /// session tab, so a view on screen never takes the composer's text.
    pub fn composer_tab(&self) -> Option<(u64, Option<&str>)> {
        self.current().map(|t| (t.id, t.draft_key()))
    }

    /// Open the view `kind` (focused if it is open already) at the end of
    /// the strip. `false`: it was the visible tab already.
    pub fn open_view(&mut self, kind: Kind) -> bool {
        debug_assert!(kind != Kind::Session);
        if let Some(ix) = self.tabs.iter().position(|t| t.kind == kind) {
            return self.focus(ix);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.tabs.push(Tab::new(id, kind, None, kind.label().to_string()));
        self.show(id);
        true
    }

    /// The session tab holding `session`.
    pub fn find(&self, session: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.kind == Kind::Session && t.session.as_deref() == Some(session))
    }

    /// The worker tab holding worker `handle` of `session`.
    pub fn find_worker(&self, session: &str, handle: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.kind == Kind::Worker && t.is_worker(session, handle))
    }

    pub fn is_open(&self, session: &str) -> bool {
        self.find(session).is_some()
    }

    /// A tab attaches `session`: its session tab or a worker tab (what
    /// the tab cache keeps).
    pub fn holds(&self, session: &str) -> bool {
        self.tabs.iter().any(|t| t.kind.attaches() && t.session.as_deref() == Some(session))
    }

    /// A tab open or on the reopen stack has `key`: a session tab's
    /// session, or a worker tab's draft key.
    pub fn knows(&self, key: &str) -> bool {
        let has = |t: &Tab| match t.kind {
            Kind::Worker => t.draft_key() == Some(key),
            _ => t.session.as_deref() == Some(key),
        };
        self.tabs.iter().any(has) || self.closed.iter().any(|(_, t)| has(t))
    }

    /// Session tab `id`'s view shows `worker` (handle, name) in place, or
    /// its session's thread: kept for when it is shown again.
    pub fn set_watching(&mut self, id: u64, worker: Option<(String, String)>) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && t.kind == Kind::Session) {
            t.watching = worker;
        }
    }

    /// The id a tab followed `session` to (along a chain, S → S2 → S3),
    /// if no tab holds `session` any more and one holds that id.
    pub fn followed(&self, session: &str) -> Option<&str> {
        if self.knows(session) {
            return None;
        }
        let mut at = session;
        // Bounded: each step is one entry of `follows`.
        for _ in 0..self.follows.len() {
            let (_, to) = self.follows.iter().rev().find(|(from, _)| from == at)?;
            if self.knows(to) {
                return Some(to.as_str());
            }
            at = to;
        }
        None
    }

    /// The last merge of the visible tab into another ([`Tabs::attached`]).
    pub fn take_merged(&mut self) -> Option<(u64, u64)> {
        self.merged.take()
    }

    /// Tab `ix` follows its session to `to` (same conversation, new id),
    /// and so do the session's worker tabs, and (from a worker tab) its
    /// session tab, unless another session tab holds `to` already.
    fn follow(&mut self, ix: usize, to: &str) {
        let free = self.find(to).is_none();
        let from = self.tabs[ix].session.clone();
        self.set_session(ix, to);
        // A pending tab named (a create, a resume): nothing else follows.
        let Some(from) = from else { return };
        if free && let Some(s) = self.find(&from) {
            self.set_session(s, to);
        }
        self.repoint_workers(&from, to);
        if self.follows.len() == CLOSED_MAX {
            self.follows.remove(0);
        }
        self.follows.push((from, to.to_string()));
    }

    /// `from`'s worker tabs go to `to` with its tab (a follow, a merge);
    /// one whose worker has a tab under `to` already goes into that one.
    fn repoint_workers(&mut self, from: &str, to: &str) {
        let ids: Vec<u64> =
            self.tabs.iter().filter(|t| t.kind == Kind::Worker && t.session.as_deref() == Some(from)).map(|t| t.id).collect();
        for id in ids {
            let Some(ix) = self.tabs.iter().position(|t| t.id == id) else { continue };
            let handle = self.tabs[ix].worker.as_ref().map(|w| w.handle.clone()).unwrap_or_default();
            match self.find_worker(to, &handle) {
                Some(j) => {
                    let (shown, keep) = (self.active == Some(ix), self.tabs[j].id);
                    if self.last_session == Some(id) {
                        self.last_session = Some(keep);
                    }
                    if let (Some(a), Some(b)) = (self.tabs[ix].draft_key(), self.tabs[j].draft_key()) {
                        self.rekeys.push((a.to_string(), b.to_string()));
                    }
                    self.remove(ix);
                    if shown {
                        self.active = self.tabs.iter().position(|t| t.id == keep);
                    }
                }
                None => self.set_session(ix, to),
            }
        }
    }

    /// Tab `id` was restored and its session has not been listed yet.
    pub fn restoring(&self, id: u64) -> bool {
        self.restoring.contains(&id)
    }

    fn changed(&mut self) {
        self.generation += 1;
    }

    /// Take tab `ix` out; the visible one gives way to its right neighbour,
    /// else its left. A view on screen gives way to the session tab behind
    /// it (spec section 1: back as it was), if that is still open.
    fn remove(&mut self, ix: usize) -> Tab {
        let behind = (self.active == Some(ix) && !self.tabs[ix].kind.attaches()).then_some(self.last_session).flatten();
        let t = self.tabs.remove(ix);
        let back = behind.and_then(|id| self.tabs.iter().position(|t| t.id == id));
        self.active = match self.active {
            Some(a) if a == ix && back.is_some() => back,
            Some(a) if a == ix => (!self.tabs.is_empty()).then(|| ix.min(self.tabs.len() - 1)),
            Some(a) if a > ix => Some(a - 1),
            a => a,
        };
        if let Some(v) = self.visible().filter(|v| v.kind.attaches()) {
            self.last_session = Some(v.id);
        }
        t
    }

    fn push_closed(&mut self, ix: usize, t: Tab) {
        self.closed.push((ix, t));
        // Back to the bound (a settle may have gone past it).
        let over = self.closed.len().saturating_sub(CLOSED_MAX);
        self.closed.drain(..over);
    }

    /// The visible tab `i` is given another session: the one it held goes
    /// on the reopen stack (Ctrl+Shift+T undoes a stray click), as a tab
    /// of its own.
    fn replace(&mut self, i: usize, session: Option<String>, label: &str) {
        let old = self.tabs[i].clone();
        self.restoring.retain(|&r| r != old.id);
        if old.session.is_some() {
            self.next_id += 1;
            let id = self.next_id;
            self.push_closed(i, Tab { id, ..old });
        }
        let t = &mut self.tabs[i];
        t.session = session;
        t.label = label.to_string();
        t.journal = None;
        t.cwd = None;
        t.watching = None;
        self.changed();
    }

    /// The restored tabs still waiting for their session close (counted
    /// in [`Tabs::dropped`]), onto the reopen stack: Ctrl+Shift+T brings
    /// one back, its journal with it.
    fn settle_restore(&mut self) {
        if self.restoring.is_empty() {
            return;
        }
        let early = self.restore_lists < RESTORE_LISTS;
        let mut n = 0;
        for id in std::mem::take(&mut self.restoring) {
            if let Some(ix) = self.tabs.iter().position(|t| t.id == id) {
                let t = self.remove(ix);
                self.closed.push((ix, t));
                n += 1;
                self.dropped += 1;
                self.dropped_early |= early;
            }
        }
        // Every one of them can come back, more than the usual bound if
        // need be (the next close trims the stack back to it).
        let over = self.closed.len().saturating_sub(CLOSED_MAX.max(n));
        self.closed.drain(..over);
        self.changed();
    }

    fn tab(&mut self, session: Option<String>, label: String) -> Tab {
        self.next_id += 1;
        Tab::new(self.next_id, Kind::Session, session, label)
    }

    /// Make the tab `id` visible. Leaving the current session tab for
    /// another session tab while it still waits for its session (a create
    /// or a resume) leaves it for good: nothing would come back to it. A
    /// view in between does not: the session tab stays current.
    fn show(&mut self, id: u64) {
        let session = self.tabs.iter().find(|t| t.id == id).is_some_and(|t| t.kind.attaches());
        if session && let Some(i) = self.current_index().filter(|&i| self.tabs[i].pending() && self.tabs[i].id != id) {
            self.tabs.remove(i);
        }
        self.active = self.tabs.iter().position(|t| t.id == id);
        if session {
            self.last_session = Some(id);
        }
        self.changed();
    }

    /// Where a session opens as `place` says: a view on screen is never
    /// replaced (the session opens in a new tab after it).
    fn place(&self, place: Place) -> Option<usize> {
        match place {
            Place::Current => self.active.filter(|&i| self.tabs[i].kind == Kind::Session),
            Place::New => None,
        }
    }

    /// Put `tab` after the visible one (at the end if none) and show it.
    fn insert_after_visible(&mut self, tab: Tab) {
        let id = tab.id;
        let at = self.active.map_or(self.tabs.len(), |a| a + 1);
        self.tabs.insert(at, tab);
        self.show(id);
    }

    /// Show tab `ix`. `false`: there is none, or it is the visible one.
    pub fn focus(&mut self, ix: usize) -> bool {
        match self.tabs.get(ix) {
            Some(t) if self.active != Some(ix) => {
                let id = t.id;
                self.show(id);
                true
            }
            _ => false,
        }
    }

    /// Open `session` (listed as `label`): its own tab if it has one, else
    /// as `place` says.
    pub fn open(&mut self, session: &str, label: &str, place: Place) {
        if let Some(j) = self.find(session) {
            self.focus(j);
            return;
        }
        match self.place(place) {
            Some(i) => self.replace(i, Some(session.to_string()), label),
            None => {
                let t = self.tab(Some(session.to_string()), label.to_string());
                self.insert_after_visible(t);
            }
        }
        self.settle_restore();
    }

    /// Open worker `handle` (called `name`) of `session` (listed as
    /// `label`) in its own tab, focused if it has one, else a new one right
    /// after the session's tab and the worker tabs after it (after the
    /// visible tab if the session has none). `false`: it was the visible
    /// tab already.
    #[cfg(test)]
    pub fn open_worker(&mut self, session: &str, handle: &str, name: &str, label: &str) -> bool {
        self.open_worker_as(session, handle, name, None, label)
    }

    /// [`Tabs::open_worker`], its worker's call known (`call`). A tab that
    /// holds an earlier worker under the handle (a cold-started daemon
    /// reuses `sa_N`) is this one's now.
    pub fn open_worker_as(&mut self, session: &str, handle: &str, name: &str, call: Option<&str>, label: &str) -> bool {
        if let Some(j) = self.find_worker(session, handle) {
            let w = self.tabs[j].worker.as_mut().expect("a worker tab");
            if !crate::wtabs::same_worker(w.who(), (call, name)) {
                *w = WorkerRef { call: call.map(str::to_string), ..WorkerRef::new(session, handle, name) };
                self.changed();
                self.focus(j);
                return true;
            }
            if w.call.is_none() && call.is_some() {
                w.call = call.map(str::to_string);
            }
            return self.focus(j);
        }
        let at = match self.find(session) {
            Some(s) => {
                let mut at = s + 1;
                while self.tabs.get(at).is_some_and(|t| t.kind == Kind::Worker && t.session.as_deref() == Some(session)) {
                    at += 1;
                }
                at
            }
            None => self.active.map_or(self.tabs.len(), |a| a + 1),
        };
        self.next_id += 1;
        let mut t = Tab::new(self.next_id, Kind::Worker, Some(session.to_string()), label.to_string());
        t.worker = Some(WorkerRef { call: call.map(str::to_string), ..WorkerRef::new(session, handle, name) });
        let id = t.id;
        self.tabs.insert(at, t);
        if let Some(a) = self.active.as_mut().filter(|a| **a >= at) {
            *a += 1;
        }
        self.show(id);
        self.settle_restore();
        true
    }

    /// A tab for a session the daemon has not named yet (a create, a
    /// resume), called `label` until it does.
    pub fn open_pending(&mut self, label: &str, place: Place) {
        match self.place(place) {
            Some(i) => self.replace(i, None, label),
            None => {
                let t = self.tab(None, label.to_string());
                self.insert_after_visible(t);
            }
        }
        self.settle_restore();
    }

    /// Close tab `ix` (the session is only detached from, never ended). The
    /// visible one gives way to its right neighbour, else its left.
    pub fn close(&mut self, ix: usize) -> bool {
        if ix >= self.tabs.len() {
            return false;
        }
        let t = self.remove(ix);
        self.restoring.retain(|&r| r != t.id);
        if t.session.is_some() {
            self.push_closed(ix, t);
        }
        self.changed();
        self.settle_restore();
        true
    }

    /// Ctrl+Shift+T: the last closed tab, where it was (a session open
    /// again since is focused instead).
    pub fn reopen(&mut self) -> bool {
        let Some((ix, t)) = self.closed.pop() else { return false };
        let open = match (&t.worker, t.session.as_deref()) {
            (Some(w), Some(s)) => self.find_worker(s, &w.handle),
            (None, Some(s)) => self.find(s),
            _ => None,
        };
        if let Some(j) = open {
            self.focus(j);
            return true;
        }
        let (id, at) = (t.id, ix.min(self.tabs.len()));
        self.tabs.insert(at, t);
        if let Some(a) = self.active.as_mut().filter(|a| **a >= at) {
            *a += 1;
        }
        self.show(id);
        self.settle_restore();
        true
    }

    /// Drag tab `from` to where tab `to` is (the visible one stays visible).
    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return false;
        }
        let shown = self.visible().map(|t| t.id);
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        self.active = shown.and_then(|id| self.tabs.iter().position(|t| t.id == id));
        self.changed();
        self.settle_restore();
        true
    }

    /// Ctrl+Tab (`1`) / Ctrl+Shift+Tab (`-1`): the next or previous tab,
    /// wrapping.
    pub fn step(&mut self, delta: isize) -> bool {
        let n = self.tabs.len() as isize;
        if n < 2 {
            return false;
        }
        let a = self.active.unwrap_or(0) as isize;
        self.focus((a + delta).rem_euclid(n) as usize)
    }

    /// Ctrl+9: the last tab.
    pub fn focus_last(&mut self) -> bool {
        self.tabs.len().checked_sub(1).is_some_and(|i| self.focus(i))
    }

    /// The visible tab's attachment landed on `session`: a create or a
    /// resume named it, or it attached another session than asked for.
    /// Never two tabs for one session: a tab that already holds it stays
    /// and is shown, the visible one goes. No tab yet: one opens.
    pub fn attached(&mut self, session: &str) {
        match self.current_index() {
            Some(i) if self.tabs[i].session.as_deref() == Some(session) => {}
            // A worker tab's session went to a new id: its tab and the
            // session's other tabs follow it (no tab ever merges into a
            // worker tab, or a worker tab into a session tab).
            Some(i) if self.tabs[i].kind == Kind::Worker => {
                let handle = self.tabs[i].worker.as_ref().map(|w| w.handle.clone()).unwrap_or_default();
                if let Some(j) = self.find_worker(session, &handle) {
                    // That worker has its tab under the new id already: that one.
                    self.merged = Some((self.tabs[i].id, self.tabs[j].id));
                    let keep = self.tabs[j].id;
                    let shown = if self.active == Some(i) { Some(keep) } else { self.visible().map(|t| t.id) };
                    let from = self.tabs[i].session.clone();
                    self.tabs.remove(i);
                    self.active = shown.and_then(|id| self.tabs.iter().position(|t| t.id == id));
                    self.last_session = Some(keep);
                    if let Some(from) = from {
                        self.repoint_workers(&from, session);
                    }
                } else {
                    self.follow(i, session);
                }
                self.changed();
            }
            Some(i) => {
                if let Some(j) = self.find(session) {
                    self.merged = Some((self.tabs[i].id, self.tabs[j].id));
                    let shown = self.visible().map(|t| t.id);
                    let keep = self.tabs[j].id;
                    let from = self.tabs.remove(i).session;
                    // A view on screen stays; else the tab holding it is shown.
                    let vis = if self.active == Some(i) { Some(keep) } else { shown };
                    self.active = vis.and_then(|id| self.tabs.iter().position(|t| t.id == id));
                    self.last_session = Some(keep);
                    // Its worker tabs go along (after: they may merge too).
                    if let Some(from) = from {
                        self.repoint_workers(&from, session);
                    }
                } else {
                    self.follow(i, session);
                }
                self.changed();
            }
            // No current tab behind a view on screen (it was closed): a tab
            // holding the session becomes current, else a new one does,
            // before the views (they stay at the end of the strip).
            None if self.view().is_some() => {
                let id = match self.find(session) {
                    Some(j) => self.tabs[j].id,
                    None => {
                        let t = self.tab(Some(session.to_string()), session.to_string());
                        let id = t.id;
                        let at = self.tabs.iter().position(|t| !t.kind.attaches()).unwrap_or(self.tabs.len());
                        self.tabs.insert(at, t);
                        self.active = self.active.map(|a| if a >= at { a + 1 } else { a });
                        id
                    }
                };
                self.last_session = Some(id);
                self.changed();
            }
            None => {
                let t = self.tab(Some(session.to_string()), session.to_string());
                self.insert_after_visible(t);
            }
        }
    }

    /// Keep each listed tab's label, journal and folder (a tab whose
    /// session ends still shows its name and can resume it).
    pub fn listed(&mut self, sessions: &[SessionMeta]) {
        for t in &mut self.tabs {
            let Some(s) = t.session.as_deref().and_then(|id| sessions.iter().find(|s| s.id == id)) else { continue };
            let label = s.name.as_deref().filter(|n| !n.is_empty()).or(s.title.as_deref().filter(|t| !t.is_empty())).unwrap_or(&s.id);
            if t.label != label {
                t.label = label.to_string();
            }
            let journal = Some(if s.journal_id.is_empty() { &s.id } else { &s.journal_id });
            if t.journal.as_ref() != journal {
                t.journal = journal.cloned();
            }
            let cwd = s.cwd.as_ref().filter(|c| !c.is_empty());
            if t.cwd.as_ref() != cwd {
                t.cwd = cwd.cloned();
            }
        }
    }

    /// The tabs to save (a tab still waiting for its session is not; a
    /// restored one still waiting for its list is, as it was).
    pub fn saved(&self) -> Saved {
        let mut sessions = Vec::with_capacity(self.tabs.len());
        let mut journals = Vec::with_capacity(self.tabs.len());
        let mut labels = Vec::with_capacity(self.tabs.len());
        let mut active = None;
        // A worker tab current: its session's tab is the one shown next time.
        let current = self.current_index().and_then(|i| match self.tabs[i].kind {
            Kind::Worker => self.find(self.tabs[i].session.as_deref()?),
            _ => Some(i),
        });
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(s) = t.session.as_ref().filter(|_| t.kind == Kind::Session) {
                if current == Some(i) {
                    active = Some(sessions.len());
                }
                sessions.push(s.clone());
                journals.push(t.journal.clone().unwrap_or_default());
                labels.push(if t.label == *s { String::new() } else { t.label.clone() });
            }
        }
        for v in [&mut journals, &mut labels] {
            if v.iter().all(String::is_empty) {
                v.clear();
            }
        }
        Saved { sessions, journals, labels, active }
    }

    /// The saved tabs, each waiting for a session list to show its session
    /// ([`Tabs::confirm`]); a session saved twice opens once.
    pub fn restore(saved: &Saved) -> Tabs {
        let mut t = Tabs::default();
        let mut active = None;
        for (i, s) in saved.sessions.iter().enumerate() {
            if !t.is_open(s) {
                let mut tab = t.tab(Some(s.clone()), saved.label(i).unwrap_or(s).to_string());
                tab.journal = saved.journal(i).map(str::to_string);
                t.restoring.push(tab.id);
                t.tabs.push(tab);
            }
            if saved.active.is_some_and(|a| i <= a) && !t.tabs.is_empty() {
                active = Some(t.tabs.len() - 1);
            }
        }
        t.active = active.or((!t.tabs.is_empty()).then_some(0));
        t.last_session = t.active.map(|a| t.tabs[a].id);
        t
    }

    /// A session list: each restored tab it shows (by session or journal
    /// id, as the session it is now: a pin restored after a daemon restart
    /// can come back under its journal's id) is confirmed. After
    /// [`RESTORE_LISTS`] lists the rest close. `true`: the visible tab was
    /// confirmed (its attach can go now).
    pub fn confirm(&mut self, sessions: &[SessionMeta]) -> bool {
        if self.restoring.is_empty() {
            return false;
        }
        let vis = self.current().map(|t| t.id);
        let mut shown = false;
        for id in self.restoring.clone() {
            let Some(ix) = self.tabs.iter().position(|t| t.id == id) else { continue };
            let t = &self.tabs[ix];
            let is = |s: &SessionMeta, id: Option<&str>| id.is_some_and(|id| s.is(id));
            let found = sessions.iter().find(|s| s.lifecycle != "ending" && (is(s, t.session.as_deref()) || is(s, t.journal.as_deref())));
            let Some(sid) = found.map(|s| s.id.clone()) else { continue };
            let same = t.session.as_deref() == Some(sid.as_str());
            self.restoring.retain(|&r| r != id);
            if !same {
                if self.is_open(&sid) {
                    // Another tab holds it already: one tab per session.
                    if let Some(from) = self.tabs[ix].session.clone() {
                        self.repoint_workers(&from, &sid);
                    }
                    self.remove(ix);
                } else {
                    self.follow(ix, &sid);
                }
                self.changed();
            }
            shown |= vis == Some(id) && self.current().is_some_and(|t| t.id == id);
        }
        self.restore_lists += 1;
        if self.restore_lists >= RESTORE_LISTS {
            self.settle_restore();
        }
        shown
    }
}

/// What a tab's draft says when it is shown again with input the daemon
/// gave back (refused, or the attachment ended) in the frame it was left.
pub const REFUSED_NOTE: &str = "not sent: given back as you left this tab; check the transcript before sending it again";

/// A tab's composer draft: its text, the caret (a byte offset) and what
/// to tell when it is shown again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
    pub note: Option<&'static str>,
}

/// Each tab's composer draft, by session (memory only): kept when its tab
/// is left, put back when it is shown again, so text typed for one
/// session never goes to another.
#[derive(Debug, Default)]
pub struct Drafts {
    map: HashMap<String, Draft>,
}

impl Drafts {
    /// The composer holds what was typed for the visible tab (`visible`:
    /// its id and session): `owner` is that tab, with that session.
    pub fn owns(owner: Option<&(u64, Option<String>)>, visible: Option<(u64, Option<&str>)>) -> bool {
        owner.map(|(i, s)| (*i, s.as_deref())) == visible
    }

    /// The composer's text is `owner`'s and that is the tab it types for
    /// ([`Tabs::composer_tab`]: the current session tab, never a view on
    /// screen). The one guard `submit()` and the draft swap use.
    pub fn composer_owned(owner: Option<&(u64, Option<String>)>, tabs: &Tabs) -> bool {
        Drafts::owns(owner, tabs.composer_tab())
    }

    /// One frame's composer work, in this order. Input given back for the
    /// composer's `owner` (refused, or sent before its tab was left and it
    /// is shown again) goes in first, ahead of what it holds; then, if the
    /// visible tab is not the owner, the composer (with `note`, else
    /// [`REFUSED_NOTE`] when input was given back) becomes the owner's
    /// draft and the visible tab's comes out ([`Drafts::switch`]). So
    /// input given back in the frame of a switch stays with its tab, and
    /// says so there.
    /// The composer's new text and caret (`None`: at the end), if it changes.
    pub fn frame(
        &mut self,
        owner: &mut Option<(u64, Option<String>)>,
        visible: Option<(u64, Option<&str>)>,
        given_back: Option<String>,
        note: &mut Option<&'static str>,
        composer: (&str, usize),
        knows: impl Fn(&str) -> bool,
    ) -> Option<(String, Option<usize>)> {
        let owned = Self::owns(owner.as_ref(), visible);
        if owned && given_back.is_none() {
            return None;
        }
        let mut text = given_back.map(|t| match composer.0.trim() {
            "" => t,
            now => format!("{t}\n{now}"),
        });
        let mut cursor = None;
        if !owned {
            let prev = std::mem::replace(owner, visible.map(|(i, s)| (i, s.map(str::to_string))));
            let (held, at, said) = match &text {
                Some(t) => (t.as_str(), t.len(), note.or(Some(REFUSED_NOTE))),
                None => (composer.0, composer.1, *note),
            };
            if let Some(d) = self.switch(prev.as_ref(), visible, held, at, said, &knows) {
                self.retain(&knows);
                *note = d.note;
                if d.text.is_empty() && composer.0.is_empty() {
                    return None;
                }
                (text, cursor) = (Some(d.text), Some(d.cursor));
            }
        }
        text.map(|t| (t, cursor))
    }

    /// The visible tab went from `prev` to `now` (tab id, session) while
    /// the composer held `text` (caret at `cursor`, saying `note`): that is
    /// `prev`'s session's draft now, and `now`'s comes out to be shown.
    /// `None`: the composer keeps what it holds, still the same tab, its
    /// session named by its create or resume, or followed to a new id (no
    /// tab or reopen entry holds the old one, per `knows`: a replaced
    /// session is on the reopen stack, so its draft stays with it). Input
    /// given back to the new id before then (an unsent that came first)
    /// goes in ahead of what the composer holds.
    pub fn switch(
        &mut self,
        prev: Option<&(u64, Option<String>)>,
        now: Option<(u64, Option<&str>)>,
        text: &str,
        cursor: usize,
        note: Option<&'static str>,
        knows: impl Fn(&str) -> bool,
    ) -> Option<Draft> {
        if let (Some((a, s)), Some((b, to))) = (prev, now)
            && a == &b
            && s.as_deref().is_none_or(|s| !knows(s))
        {
            let back = to.and_then(|to| self.map.remove(to))?;
            return Some(if text.trim().is_empty() {
                back
            } else {
                Draft { cursor: back.text.len() + 1 + cursor, text: format!("{}\n{text}", back.text), note: back.note }
            });
        }
        if let Some((_, Some(s))) = prev {
            if text.trim().is_empty() {
                self.map.remove(s);
            } else {
                self.map.insert(s.clone(), Draft { text: text.to_string(), cursor, note });
            }
        }
        Some(now.and_then(|(_, s)| s).and_then(|s| self.map.remove(s)).unwrap_or_default())
    }

    /// Input sent to `session` and given back after its tab was left:
    /// ahead of its draft, which says `note` when shown.
    pub fn give_back(&mut self, session: String, text: String, note: &'static str) {
        let d = self.map.entry(session).or_default();
        if d.text.trim().is_empty() {
            d.cursor = text.len();
            d.text = text;
        } else {
            d.cursor += text.len() + 1;
            d.text = format!("{text}\n{}", d.text);
        }
        d.note = Some(note);
    }

    /// A worker tab's draft key changed with its session ([`Tabs::take_rekeys`]):
    /// its draft goes with it (after one already there).
    pub fn rekey(&mut self, from: &str, to: &str) {
        let Some(d) = self.map.remove(from) else { return };
        match self.map.remove(to) {
            Some(had) if !had.text.trim().is_empty() => {
                let cursor = had.text.len() + 1 + d.cursor;
                self.map.insert(to.to_string(), Draft { text: format!("{}\n{}", had.text, d.text), cursor, note: had.note.or(d.note) });
            }
            _ => {
                self.map.insert(to.to_string(), d);
            }
        }
    }

    /// Drafts of sessions no tab (open, or on the reopen stack) holds go.
    pub fn retain(&mut self, keep: &impl Fn(&str) -> bool) {
        self.map.retain(|s, _| keep(s));
    }
}

/// When a tab switch's attach goes out. Each attach brings the whole
/// history (the protocol has no deltas yet), so a burst of switches
/// attaches once, after the last: [`AttachGate::GAP`] after it, and never
/// while a tab key is held (a held Ctrl+Tab attaches once, when it is let
/// go). A switch by other means (a click) with none just before attaches
/// at once.
#[derive(Debug, Default)]
pub struct AttachGate {
    /// The last switch, or the release of the key that made it.
    last: Option<Instant>,
    /// A tab key is down: its last key-down (the press or a repeat).
    held: Option<Instant>,
}

impl AttachGate {
    pub const GAP: Duration = Duration::from_millis(120);
    /// A key-up this window never saw (the focus moved, or macOS dropped
    /// it while Cmd was held) stops counting after this long without a
    /// repeat: longer than the usual first-repeat delays (X11 660 ms,
    /// Windows up to 1 s).
    pub const STALE: Duration = Duration::from_millis(1100);

    /// A tab key went down at `now` (its switch follows).
    pub fn key_down(&mut self, now: Instant) {
        self.held = Some(now);
    }

    /// The tab key, or the modifier that made it one, went up.
    pub fn key_up(&mut self, now: Instant) {
        if self.held.take().is_some() {
            self.last = Some(now);
        }
    }

    /// A switch at `now`: `true` if its attach waits until
    /// [`AttachGate::due`].
    pub fn switch(&mut self, now: Instant) -> bool {
        let wait = self.held.is_some_and(|k| now.saturating_duration_since(k) < Self::STALE)
            || self.last.is_some_and(|t| now.saturating_duration_since(t) < Self::GAP);
        self.last = Some(now);
        wait
    }

    /// When a waiting attach may go: [`AttachGate::GAP`] after the last
    /// switch, and no tab key held.
    pub fn due(&self) -> Option<Instant> {
        self.last.map(|t| t + Self::GAP).max(self.held.map(|k| k + Self::STALE))
    }

    pub fn ready(&self, now: Instant) -> bool {
        self.due().is_none_or(|d| now >= d)
    }
}

/// A least-recently-used map bounded in entries and in (estimated)
/// bytes: the background tabs' caches. `put` past either drops the oldest
/// entries, preferring ones `keep` does not want.
#[derive(Debug)]
pub struct Lru<K, V> {
    cap: usize,
    budget: usize,
    bytes: usize,
    order: VecDeque<K>,
    map: HashMap<K, (V, usize)>,
}

impl<K: Clone + Eq + Hash, V> Lru<K, V> {
    pub fn new(cap: usize, budget: usize) -> Self {
        Lru { cap, budget, bytes: 0, order: VecDeque::with_capacity(cap + 1), map: HashMap::with_capacity(cap + 1) }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The entries' estimated bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    /// `k`'s entry, left where it is.
    pub fn peek<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.map.get(k).map(|(v, _)| v)
    }

    /// Take `k` out (it is on screen now).
    pub fn take(&mut self, k: &K) -> Option<V> {
        let (v, n) = self.map.remove(k)?;
        self.bytes -= n;
        self.order.retain(|o| o != k);
        Some(v)
    }

    /// Insert `k` (`bytes` of it) as the newest; past the cap or the
    /// budget the oldest entries not `keep` go first, then the oldest of
    /// all (one bigger than the whole budget is not kept).
    pub fn put(&mut self, k: K, v: V, bytes: usize, keep: impl Fn(&K) -> bool) {
        self.take(&k);
        self.order.push_back(k.clone());
        self.map.insert(k, (v, bytes));
        self.bytes += bytes;
        while self.map.len() > self.cap || (self.bytes > self.budget && !self.map.is_empty()) {
            let at = self.order.iter().position(|o| !keep(o)).unwrap_or(0);
            let Some(old) = self.order.remove(at) else { break };
            if let Some((_, n)) = self.map.remove(&old) {
                self.bytes -= n;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(t: &Tabs) -> Vec<&str> {
        t.iter().map(|t| t.session.as_deref().unwrap_or("…")).collect()
    }

    fn vis(t: &Tabs) -> Option<&str> {
        t.visible().map(|t| t.session.as_deref().unwrap_or("…"))
    }

    /// No two session tabs ever hold one session, nor two worker tabs one
    /// worker, whatever the operations.
    fn unique(t: &Tabs) {
        let open: Vec<&str> = t.iter().filter(|t| t.kind == Kind::Session).filter_map(|t| t.session.as_deref()).collect();
        let mut dedup = open.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), open.len(), "two tabs for one session: {open:?}");
        let workers: Vec<&str> = t.iter().filter_map(|t| t.draft_key().filter(|_| t.kind == Kind::Worker)).collect();
        let mut dedup = workers.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), workers.len(), "two tabs for one worker: {workers:?}");
        assert!(t.iter().all(|x| x.kind != Kind::Worker || (x.session.is_some() && x.worker.is_some())));
        assert_eq!(t.active.is_some(), !t.is_empty(), "a visible tab iff there are tabs");
        assert!(t.active.is_none_or(|a| a < t.len()));
        assert!(t.iter().all(|x| !x.pending() || t.current().is_some_and(|c| c.id == x.id)), "a pending tab in the background");
        assert!(t.iter().filter(|x| x.kind == Kind::Settings).count() <= 1 && t.iter().filter(|x| x.kind == Kind::Usage).count() <= 1);
    }

    #[test]
    fn open_replaces_the_current_tab_or_adds_one_after_it() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        assert_eq!((ids(&t), vis(&t)), (vec!["a"], Some("a")), "no tab: Current opens one");
        t.open("b", "b", Place::Current);
        assert_eq!((ids(&t), vis(&t)), (vec!["b"], Some("b")), "a plain click replaces");
        t.open("c", "c", Place::New);
        t.focus(0);
        t.open("d", "d", Place::New);
        assert_eq!((ids(&t), vis(&t)), (vec!["b", "d", "c"], Some("d")), "a new tab goes right after the visible one");
        unique(&t);
    }

    #[test]
    fn opening_an_open_session_focuses_its_tab() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c"] {
            t.open(s, s, Place::New);
        }
        let g = t.generation;
        t.open("a", "a", Place::Current);
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "b", "c"], Some("a")), "not replaced: focused");
        t.open("c", "c", Place::New);
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "b", "c"], Some("c")), "not a second tab");
        assert!(t.generation > g);
        let g = t.generation;
        t.open("c", "c", Place::New);
        assert_eq!(t.generation, g, "the visible one again: nothing changes");
        unique(&t);
    }

    #[test]
    fn close_and_reopen() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c", "d"] {
            t.open(s, s, Place::New);
        }
        t.focus(1);
        assert!(t.close(1));
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "c", "d"], Some("c")), "the right neighbour");
        t.close(2);
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "c"], Some("c")), "a background tab: the visible one stays");
        t.focus(1);
        t.close(1);
        assert_eq!(vis(&t), Some("a"), "the last tab: its left neighbour");
        assert!(t.reopen());
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "c"], Some("c")), "back where it was, shown");
        assert!(t.reopen());
        assert_eq!(ids(&t), vec!["a", "c", "d"]);
        assert!(t.reopen());
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "b", "c", "d"], Some("b")));
        assert!(!t.reopen(), "the stack is empty");
        // Closing the last tab leaves none.
        while !t.is_empty() {
            t.close(0);
        }
        assert_eq!(vis(&t), None);
        unique(&t);
    }

    #[test]
    fn reopening_a_session_open_again_focuses_it() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        t.open("b", "b", Place::New);
        t.close(0);
        t.open("a", "a", Place::New);
        assert!(t.reopen());
        assert_eq!((ids(&t), vis(&t)), (vec!["b", "a"], Some("a")));
        unique(&t);
    }

    #[test]
    fn the_reopen_stack_is_bounded() {
        let mut t = Tabs::default();
        for i in 0..CLOSED_MAX + 5 {
            t.open(&i.to_string(), "", Place::New);
        }
        while !t.is_empty() {
            t.close(0);
        }
        let mut back = 0;
        while t.reopen() {
            back += 1;
        }
        assert_eq!(back, CLOSED_MAX);
        assert_eq!(t.len(), CLOSED_MAX);
        unique(&t);
    }

    #[test]
    fn a_pending_tab_is_named_on_attach_and_never_doubles_a_session() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        t.open_pending("new session", Place::New);
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "…"], Some("…")));
        t.attached("x");
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "x"], Some("x")), "the create named it");
        // A resume in a new tab that lands on a session already open: that tab stays, shown.
        t.open_pending("a", Place::New);
        t.attached("a");
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "x"], Some("a")));
        // In the current tab: the tab holding it wins over the replaced one.
        t.focus(1);
        t.open_pending("resume", Place::Current);
        t.attached("a");
        assert_eq!((ids(&t), vis(&t)), (vec!["a"], Some("a")));
        // Another session than asked for (the resolver's pick): the tab follows.
        t.attached("b");
        assert_eq!((ids(&t), vis(&t)), (vec!["b"], Some("b")));
        unique(&t);
    }

    #[test]
    fn leaving_a_pending_tab_drops_it() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        t.open_pending("new session", Place::New);
        t.focus(0);
        assert_eq!((ids(&t), vis(&t)), (vec!["a"], Some("a")));
        t.open_pending("new session", Place::New);
        t.open("b", "b", Place::New);
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "b"], Some("b")));
        assert_eq!(t.saved(), Saved { sessions: vec!["a".into(), "b".into()], journals: vec![], labels: vec![], active: Some(1) });
        unique(&t);
    }

    #[test]
    fn switching_wraps_and_numbers() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c"] {
            t.open(s, s, Place::New);
        }
        assert!(t.step(1));
        assert_eq!(vis(&t), Some("a"), "wraps forward");
        assert!(t.step(-1));
        assert_eq!(vis(&t), Some("c"), "wraps back");
        assert!(t.focus(1));
        assert_eq!(vis(&t), Some("b"));
        assert!(!t.focus(1), "already visible");
        assert!(!t.focus(7), "no 8th tab");
        assert!(t.focus_last());
        assert_eq!(vis(&t), Some("c"));
        let mut one = Tabs::default();
        one.open("a", "a", Place::New);
        assert!(!one.step(1) && !one.focus_last());
    }

    #[test]
    fn drag_reorders_and_the_visible_tab_stays_visible() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c", "d"] {
            t.open(s, s, Place::New);
        }
        t.focus(1);
        assert!(t.move_tab(1, 3));
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "c", "d", "b"], Some("b")));
        assert!(t.move_tab(0, 2));
        assert_eq!((ids(&t), vis(&t)), (vec!["c", "d", "a", "b"], Some("b")));
        assert!(t.move_tab(3, 0));
        assert_eq!((ids(&t), vis(&t)), (vec!["b", "c", "d", "a"], Some("b")));
        assert!(!t.move_tab(1, 1) && !t.move_tab(0, 9));
        unique(&t);
    }

    /// What the strip shows: a session tab its session, a worker tab
    /// `session/sa_N`.
    fn strip(t: &Tabs) -> Vec<String> {
        t.iter()
            .map(|t| match (&t.worker, t.session.as_deref()) {
                (Some(w), Some(s)) => format!("{s}/{}", w.handle),
                (_, s) => s.unwrap_or("…").to_string(),
            })
            .collect()
    }

    fn shown(t: &Tabs) -> Option<String> {
        let ix = t.active()?;
        strip(t).get(ix).cloned()
    }

    #[test]
    fn a_worker_tab_opens_after_its_session_and_is_focused_if_open() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c"] {
            t.open(s, s, Place::New);
        }
        t.focus(1);
        assert!(t.open_worker("b", "sa_2", "sleeper", "b"));
        assert_eq!((strip(&t), shown(&t).as_deref()), (vec!["a".into(), "b".into(), "b/sa_2".into(), "c".into()], Some("b/sa_2")));
        // It attaches its session: it is the current tab, its draft its own.
        assert_eq!(t.current_session(), Some("b"));
        assert_eq!(t.composer_tab().map(|c| c.1), Some(Some("b#sa_2")));
        assert_eq!(t.view(), None, "not a view");
        assert_eq!(t.current_worker().map(|w| (w.handle.as_str(), w.name.as_str(), w.id())), Some(("sa_2", "sleeper", Some(2))));
        // The next one of the same session after it; from the session's tab.
        t.focus(1);
        assert!(t.open_worker("b", "sa_3", "scout", "b"));
        assert_eq!(strip(&t)[1..4], ["b".to_string(), "b/sa_2".into(), "b/sa_3".into()]);
        // Open already: focused, not a second tab; the visible one: nothing.
        let g = t.generation;
        assert!(t.open_worker("b", "sa_2", "sleeper", "b"));
        assert_eq!((t.len(), shown(&t).as_deref()), (5, Some("b/sa_2")));
        assert!(!t.open_worker("b", "sa_2", "sleeper", "b"));
        assert!(t.generation > g);
        // The rail's session lookups see session tabs only.
        assert_eq!((t.find("b"), t.is_open("b"), t.holds("b")), (Some(1), true, true));
        assert_eq!(t.find_worker("b", "sa_3"), Some(3));
        // A session with no tab: after the visible one.
        t.focus(0);
        t.open_worker("z", "sa_1", "w", "z");
        assert_eq!(strip(&t)[..2], ["a".to_string(), "z/sa_1".into()]);
        assert!(!t.is_open("z") && t.holds("z"));
        unique(&t);
    }

    #[test]
    fn a_worker_tab_closes_reopens_and_outlives_its_session_tab() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        t.open("b", "b", Place::New);
        t.open_worker("a", "sa_1", "sleeper", "a");
        assert_eq!(strip(&t), ["a", "a/sa_1", "b"]);
        // Closing the session's tab leaves its worker's tab, still able to
        // attach it (the cache keeps the session for it).
        t.close(0);
        assert_eq!((strip(&t), shown(&t).as_deref()), (vec!["a/sa_1".to_string(), "b".into()], Some("a/sa_1")));
        assert_eq!((t.current_session(), t.holds("a"), t.is_open("a")), (Some("a"), true, false));
        // Its breadcrumb (or a rail click) opens the session again: a tab
        // of its own, never the worker's.
        t.open("a", "a", Place::Current);
        assert_eq!((strip(&t), shown(&t).as_deref()), (vec!["a/sa_1".to_string(), "a".into(), "b".into()], Some("a")));
        // Closed: never the worker (a detach of nothing); Ctrl+Shift+T brings it back.
        t.close(0);
        assert_eq!(strip(&t), ["a", "b"]);
        assert!(t.knows("a#sa_1"), "its draft is kept while it can come back");
        assert!(t.reopen());
        assert_eq!((strip(&t), shown(&t).as_deref()), (vec!["a/sa_1".to_string(), "a".into(), "b".into()], Some("a/sa_1")));
        // Reopened while open again: focused, not doubled.
        t.close(0);
        t.focus(0);
        t.open_worker("a", "sa_1", "sleeper", "a");
        t.focus(0);
        assert!(t.reopen());
        assert_eq!((t.len(), shown(&t).as_deref()), (3, Some("a/sa_1")));
        unique(&t);
    }

    #[test]
    fn worker_tabs_follow_their_session_and_are_never_saved() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        t.open_worker("a", "sa_1", "sleeper", "a");
        t.open("b", "b", Place::New);
        // A view on screen: the worker tab behind it stays current.
        t.focus(1);
        t.open_view(Kind::Settings);
        assert_eq!((t.current_session(), t.current_worker().map(|w| w.handle.as_str())), (Some("a"), Some("sa_1")));
        t.focus(1);
        // Its session comes back under a new id (a fallback resume, a
        // daemon restart): the worker tab, the session's tab and its draft key follow.
        t.attached("a2");
        assert_eq!(strip(&t), ["a2", "a2/sa_1", "b", "…"]);
        assert_eq!(t.composer_tab().map(|c| c.1), Some(Some("a2#sa_1")));
        assert_eq!(t.followed("a"), Some("a2"));
        // From the session's tab too.
        t.focus(0);
        t.attached("a3");
        assert_eq!(strip(&t)[..2], ["a3".to_string(), "a3/sa_1".into()]);
        // A session tab merging into one that holds the new id takes its
        // worker tabs along.
        t.focus(2);
        t.open_worker("b", "sa_4", "w", "b");
        t.focus(2);
        t.attached("a3");
        assert_eq!(strip(&t), ["a3", "a3/sa_1", "a3/sa_4", "…"]);
        unique(&t);
        // Saved: session tabs only; a worker tab current saves its session's.
        t.focus(1);
        let s = t.saved();
        assert_eq!((s.sessions, s.active), (vec!["a3".to_string()], Some(0)));
        // A worker tab whose session has no tab: nothing is active.
        t.close(0);
        assert_eq!(t.saved(), Saved::default());
        assert!(Tabs::restore(&t.saved()).is_empty(), "not restored on launch");
    }

    /// L9: a background worker tab whose session follows a new id keeps
    /// its draft (its key changes with it); one that merges into the tab
    /// already holding its worker under the new id brings its text along.
    #[test]
    fn a_background_worker_tabs_draft_follows_its_session() {
        let mut t = Tabs::default();
        let mut d = Drafts::default();
        t.open("a", "a", Place::New);
        t.open_worker("a", "sa_1", "sleeper", "a");
        t.open_worker("a", "sa_2", "scout", "a");
        d.give_back("a#sa_2".into(), "to scout".into(), "n");
        // sa_1's tab is shown; sa_2's is in the background with a draft.
        t.focus(1);
        t.attached("a2");
        assert_eq!(strip(&t), ["a2", "a2/sa_1", "a2/sa_2"]);
        for (from, to) in t.take_rekeys() {
            d.rekey(&from, &to);
        }
        assert!(t.take_rekeys().is_empty(), "once");
        d.retain(&|k| t.knows(k));
        assert_eq!(d.switch(None, Some((9, Some("a2#sa_2"))), "", 0, None, |_| true).map(|d| d.text).as_deref(), Some("to scout"));
        // Into a tab that holds the worker already: the text goes along, after its own.
        let mut d = Drafts::default();
        d.give_back("b#sa_4".into(), "mine".into(), "n");
        d.give_back("a2#sa_4".into(), "theirs".into(), "n");
        d.rekey("b#sa_4", "a2#sa_4");
        assert_eq!(d.switch(None, Some((9, Some("a2#sa_4"))), "", 0, None, |_| true).map(|d| d.text).as_deref(), Some("theirs\nmine"));
    }

    /// H2: a tab that holds an earlier worker under a handle (a
    /// cold-started daemon reuses `sa_N`) is the new one's once that one
    /// opens: never two tabs for one handle, and the new worker's call
    /// says who it is.
    #[test]
    fn a_reused_handle_takes_its_tab_over() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        assert!(t.open_worker_as("a", "sa_2", "scout", Some("t_old"), "a"));
        t.focus(0);
        assert!(t.open_worker_as("a", "sa_2", "scout", Some("t_old"), "a"), "the same worker: focused");
        assert!(!t.open_worker_as("a", "sa_2", "scout", None, "a"), "already shown");
        t.focus(0);
        assert!(t.open_worker_as("a", "sa_2", "builder", Some("t_new"), "a"));
        assert_eq!(strip(&t), ["a", "a/sa_2"]);
        assert_eq!(t.current_worker().map(|w| w.who()), Some((Some("t_new"), "builder")));
        unique(&t);
    }

    #[test]
    fn a_session_tab_keeps_the_worker_it_showed_in_place() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        let id = t.current().unwrap().id;
        t.set_watching(id, Some(("sa_2".into(), "sleeper".into())));
        t.open_worker("a", "sa_3", "scout", "a");
        let w = t.current().unwrap().id;
        t.set_watching(w, Some(("sa_9".into(), "x".into())));
        assert_eq!(t.current().unwrap().watching, None, "a worker tab shows its own worker only");
        t.focus(0);
        assert_eq!(t.current().unwrap().watching, Some(("sa_2".into(), "sleeper".into())));
        // Replaced by another session: its thread.
        t.open("b", "b", Place::Current);
        assert_eq!(t.current().unwrap().watching, None);
    }

    fn meta(id: &str, journal: &str) -> SessionMeta {
        serde_json::from_value(serde_json::json!({"id": id, "journal_id": journal, "lifecycle": "live"})).unwrap()
    }

    #[test]
    fn saved_tabs_round_trip_and_drop_gone_sessions() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c", "d"] {
            t.open(s, s, Place::New);
        }
        t.listed(&[meta("b", "jb")]);
        t.focus(2);
        assert_eq!(t.saved().labels, Vec::<String>::new(), "labels that are only the ids are not saved");
        let saved = t.saved();
        let four = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        assert_eq!(
            saved,
            Saved { sessions: four, journals: vec!["".into(), "jb".into(), "".into(), "".into()], labels: vec![], active: Some(2) }
        );
        let mut r = Tabs::restore(&saved);
        assert_eq!((ids(&r), vis(&r)), (vec!["a", "b", "c", "d"], Some("c")));
        assert!(r.iter().all(|t| r.restoring(t.id)), "all wait for a list");
        assert!(r.confirm(&[meta("a", ""), meta("b", "jb"), meta("c", ""), meta("d", "")]), "the visible one is confirmed");
        assert_eq!((ids(&r), r.dropped, r.saved()), (vec!["a", "b", "c", "d"], 0, saved.clone()));
        // The visible one is gone: once the lists give up on it, the nearest kept one.
        let mut r = Tabs::restore(&saved);
        for _ in 0..RESTORE_LISTS {
            r.confirm(&[meta("b", "jb"), meta("d", "")]);
        }
        assert_eq!((ids(&r), vis(&r), r.dropped), (vec!["b", "d"], Some("d"), 2));
        // A hand-edited file naming a session twice opens it once, and that is no drop.
        let twice = Saved { sessions: vec!["a".into(), "a".into()], journals: vec![], labels: vec![], active: Some(1) };
        let mut r = Tabs::restore(&twice);
        r.confirm(&[meta("a", "")]);
        assert_eq!((ids(&r), vis(&r), r.dropped), (vec!["a"], Some("a"), 0));
        unique(&r);
    }

    /// M2: after a daemon's cold start the first lists lack the pinned
    /// sessions it is still restoring, and a compacted one comes back
    /// under its journal's id. The tabs wait (OPENING), follow it, and
    /// only close, with the notice, once several lists never had them.
    #[test]
    fn restored_tabs_survive_a_daemon_cold_start() {
        let saved = Saved {
            sessions: vec!["s-a".into(), "s-b".into(), "s-gone".into()],
            journals: vec!["".into(), "j-b".into(), "".into()],
            labels: vec!["Tide pools".into(), "".into(), "".into()],
            active: Some(1),
        };
        let mut r = Tabs::restore(&saved);
        assert_eq!(r.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), vec!["Tide pools", "s-b", "s-gone"], "named as last seen");
        let g = r.generation;
        // The daemon is up but has restored nothing yet.
        assert!(!r.confirm(&[]));
        assert!(!r.confirm(&[meta("other", "")]));
        assert_eq!((ids(&r), r.dropped, r.generation), (vec!["s-a", "s-b", "s-gone"], 0, g), "nothing dropped, nothing to save");
        // A comes back; B comes back as its journal (a new session id).
        assert!(!r.confirm(&[meta("s-a", "")]), "the visible one (B) still waits");
        assert!(r.confirm(&[meta("s-a", ""), meta("j-b", "j-b")]), "B is confirmed: its attach can go");
        assert_eq!((ids(&r), vis(&r)), (vec!["s-a", "j-b", "s-gone"], Some("j-b")));
        assert!(!r.restoring(r.visible().unwrap().id));
        // The third never shows: it closes after RESTORE_LISTS lists, not before.
        for _ in 5..RESTORE_LISTS {
            r.confirm(&[meta("s-a", ""), meta("j-b", "j-b")]);
            assert_eq!(r.dropped, 0);
        }
        r.confirm(&[meta("s-a", ""), meta("j-b", "j-b")]);
        assert_eq!((ids(&r), vis(&r), r.dropped, r.dropped_early), (vec!["s-a", "j-b"], Some("j-b"), 1, false));
        assert!(r.knows("s-gone"), "on the reopen stack");
        // A restored session that comes back as one another tab holds: one tab.
        let both =
            Saved { sessions: vec!["old".into(), "new".into()], journals: vec!["new".into(), "".into()], labels: vec![], active: Some(0) };
        let mut r = Tabs::restore(&both);
        r.confirm(&[meta("new", "new")]);
        assert_eq!((ids(&r), r.dropped), (vec!["new"], 0));
        unique(&r);
    }

    #[test]
    fn a_user_change_closes_the_tabs_still_restoring() {
        let saved = Saved { sessions: vec!["a".into(), "b".into(), "c".into()], journals: vec![], labels: vec![], active: Some(0) };
        let mut r = Tabs::restore(&saved);
        r.confirm(&[meta("a", "")]);
        assert!(r.focus(1), "looking at one is no change");
        assert_eq!((r.len(), r.dropped), (3, 0));
        r.open("x", "x", Place::New);
        assert_eq!((ids(&r), vis(&r), r.dropped, r.dropped_early), (vec!["a", "x"], Some("x"), 2, true));
        unique(&r);
        // R-L2: they may be a moment away: Ctrl+Shift+T brings each back,
        // waiting for its session no more (an attach, resumed from its journal if it ended).
        assert!(r.knows("b") && r.knows("c"));
        assert!(r.reopen());
        assert_eq!((ids(&r), vis(&r)), (vec!["a", "x", "c"], Some("c")));
        assert!(!r.restoring(r.visible().unwrap().id));
        assert!(r.reopen());
        assert_eq!(ids(&r), vec!["a", "b", "x", "c"]);
        unique(&r);
    }

    /// L3: a plain click replaces the visible tab's session; Ctrl+Shift+T
    /// brings the replaced one back (as a tab of its own).
    #[test]
    fn a_replaced_session_can_be_reopened() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::New);
        t.open("b", "b", Place::Current);
        assert_eq!(ids(&t), vec!["b"]);
        assert!(t.reopen());
        assert_eq!((ids(&t), vis(&t)), (vec!["a", "b"], Some("a")));
        unique(&t);
    }

    /// M3: a held Ctrl+Tab (30 Hz repeats for 2 s) attaches once, when it is
    /// let go, on X11 (a key-up right before each repeat) and elsewhere (no
    /// key-up until the end); a lost key-up still attaches, late; a lone
    /// click attaches at once, a burst of clicks once more after the last.
    #[test]
    fn the_attach_gate_waits_for_the_last_switch() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        // What the app does: a switch that does not wait attaches at once;
        // one that does attaches when its timer finds the gate ready.
        let run = |events: &[(u64, &str)], end: u64| {
            let mut g = AttachGate::default();
            let (mut attaches, mut waiting) = (Vec::new(), false);
            let mut ms = 0;
            let mut ev = events.iter().peekable();
            while ms <= end {
                while let Some(&&(t, what)) = ev.peek().filter(|(t, _)| *t <= ms) {
                    match what {
                        "down" => {
                            g.key_down(at(t));
                            waiting = g.switch(at(t));
                            if !waiting {
                                attaches.push(t);
                            }
                        }
                        "up" => g.key_up(at(t)),
                        _ => {
                            waiting = g.switch(at(t));
                            if !waiting {
                                attaches.push(t);
                            }
                        }
                    }
                    ev.next();
                }
                if waiting && g.ready(at(ms)) {
                    waiting = false;
                    attaches.push(ms);
                }
                ms += 1;
            }
            attaches
        };
        let mut wayland = vec![(0, "down")];
        let mut x11 = vec![(0, "down")];
        for k in 0..45 {
            let t = 600 + k * 33;
            wayland.push((t, "down"));
            x11.extend([(t, "up"), (t, "down")]);
        }
        wayland.push((2100, "up"));
        x11.push((2100, "up"));
        assert_eq!(run(&wayland, 4000), vec![2100 + 120], "held (is_held repeats): one attach, after the key-up");
        assert_eq!(run(&x11, 4000), vec![2100 + 120], "held on X11: one attach");
        let mut lost = wayland.clone();
        lost.pop();
        assert_eq!(run(&lost, 4000), vec![600 + 44 * 33 + 1100], "a lost key-up: once, STALE after the last repeat");
        assert_eq!(run(&[(0, "down"), (90, "up")], 1000), vec![210], "a tap: GAP after the release");
        assert_eq!(run(&[(500, "click")], 1000), vec![500], "a click: at once");
        assert_eq!(run(&[(0, "click"), (50, "click"), (100, "click")], 1000), vec![0, 220], "a burst of clicks: once more, after the last");
    }

    /// M4: each tab keeps its own draft and caret; a send given back after
    /// its tab was left goes into that tab's draft, ahead of what was typed
    /// there, never into the one on screen.
    #[test]
    fn each_tab_keeps_its_draft() {
        let mut d = Drafts::default();
        let all = |_: &str| true;
        let (a, b) = ((1, Some("A".to_string())), (2, Some("B".to_string())));
        fn v(t: &(u64, Option<String>)) -> Option<(u64, Option<&str>)> {
            Some((t.0, t.1.as_deref()))
        }
        assert_eq!(d.switch(None, v(&a), "", 0, None, all), Some(Draft::default()));
        // A steer typed for A, caret mid-text; Ctrl+Tab to B: B's composer is empty.
        assert_eq!(d.switch(Some(&a), v(&b), "steer A this way", 5, None, all), Some(Draft::default()));
        // Something for B, then back to A: A's text and caret come back.
        let back = d.switch(Some(&b), v(&a), "for B", 5, None, all).unwrap();
        assert_eq!((back.text.as_str(), back.cursor, back.note), ("steer A this way", 5, None));
        // A's send refused after its tab was left (B shown): into A's draft.
        let shown_b = d.switch(Some(&a), v(&b), "more for A", 10, None, all).unwrap();
        assert_eq!(shown_b.text, "for B", "B's own, untouched");
        d.give_back("A".into(), "the refused one".into(), "not sent");
        let a_again = d.switch(Some(&b), v(&a), "", 0, None, all).unwrap();
        assert_eq!(a_again.text, "the refused one\nmore for A");
        assert_eq!((a_again.cursor, a_again.note), ("the refused one\n".len() + 10, Some("not sent")));
        // A pending tab named by its create keeps the composer as it is.
        let p = (3, None);
        d.switch(Some(&a), v(&p), "", 0, None, all);
        assert_eq!(d.switch(Some(&p), Some((3, Some("C"))), "typed", 5, None, all), None);
        // Closed and gone from the reopen stack: dropped.
        d.give_back("B".into(), "x".into(), "not sent");
        d.retain(&|s| s != "B");
        assert_eq!(d.switch(None, v(&b), "", 0, None, all), Some(Draft::default()));
    }

    /// R-L1: input given back in the frame of a tab switch (A's refusal
    /// lands, then Ctrl+Tab to B, then the frame) goes into A's draft with
    /// its note, never into B's composer; and the composer does not own B
    /// until that frame, so Enter before it sends nothing.
    #[test]
    fn input_given_back_in_the_frame_of_a_switch_stays_with_its_tab() {
        let mut d = Drafts::default();
        let all = |_: &str| true;
        d.switch(None, Some((2, Some("B"))), "", 0, None, all);
        let mut owner = Some((1, Some("A".to_string())));
        d.map.insert("B".into(), Draft { text: "for B".into(), cursor: 2, note: None });
        // Before the frame: the composer still holds A's text, B is visible.
        assert!(!Drafts::owns(owner.as_ref(), Some((2, Some("B")))), "Enter now: refused, nothing sent");
        assert!(Drafts::owns(owner.as_ref(), Some((1, Some("A")))));
        let mut note = Some("not sent");
        let out = d.frame(&mut owner, Some((2, Some("B"))), Some("steer A".into()), &mut note, ("typed for A", 3), all);
        assert_eq!(out, Some(("for B".into(), Some(2))), "B's own draft, untouched");
        assert_eq!((owner.clone(), note), (Some((2, Some("B".to_string()))), None));
        assert!(Drafts::owns(owner.as_ref(), Some((2, Some("B")))));
        let back = d.switch(owner.as_ref(), Some((1, Some("A"))), "", 0, None, all).unwrap();
        assert_eq!((back.text.as_str(), back.cursor, back.note), ("steer A\ntyped for A", "steer A\ntyped for A".len(), Some("not sent")));
        // Given back with no switch: into the composer, ahead of what it holds.
        let mut owner = Some((1, Some("A".to_string())));
        let mut note = None;
        assert_eq!(
            d.frame(&mut owner, Some((1, Some("A"))), Some("refused".into()), &mut note, ("  now ", 6), all),
            Some(("refused\nnow".into(), None))
        );
        assert_eq!(d.frame(&mut owner, Some((1, Some("A"))), None, &mut note, ("x", 1), all), None, "nothing to do: nothing allocated");
    }

    /// R-L3: a tab whose session comes back under a new id (a fallback
    /// resume, a daemon restart) keeps its draft, and input given back to
    /// the old id goes to it; a replaced session's draft stays with it on
    /// the reopen stack.
    #[test]
    fn a_tab_followed_to_a_new_id_keeps_its_draft() {
        let mut t = Tabs::default();
        t.open("S", "s", Place::Current);
        let tab = t.visible().unwrap().id;
        let mut d = Drafts::default();
        let mut owner = Some((tab, Some("S".to_string())));
        t.attached("S2");
        assert_eq!((t.visible_session(), t.followed("S"), t.followed("S2")), (Some("S2"), Some("S2"), None));
        let mut note = Some("kept");
        let vis = t.visible().map(|x| (x.id, x.session.as_deref()));
        assert_eq!(d.frame(&mut owner, vis, None, &mut note, ("half a steer", 4), |s| t.knows(s)), None, "the composer keeps it");
        assert_eq!((owner.clone(), note), (Some((tab, Some("S2".to_string()))), Some("kept")));
        // A list after a daemon restart: the restored tab follows its journal.
        let mut r = Tabs::restore(&Saved { sessions: vec!["old".into()], journals: vec!["j-old".into()], labels: vec![], active: Some(0) });
        assert!(r.confirm(&[meta("j-old", "j-old")]));
        assert_eq!((r.visible_session(), r.followed("old")), (Some("j-old"), Some("j-old")));
        // Replaced by a click on another session: S2 goes to the reopen stack, its draft with it.
        t.open("T", "t", Place::Current);
        assert!(t.knows("S2") && t.followed("S2").is_none());
        let vis = t.visible().map(|x| (x.id, x.session.as_deref()));
        assert_eq!(d.frame(&mut owner, vis, None, &mut note, ("half a steer", 4), |s| t.knows(s)), Some((String::new(), Some(0))));
        assert_eq!(d.map.get("S2").map(|x| (x.text.as_str(), x.note)), Some(("half a steer", Some("kept"))));
    }

    /// L5: input the daemon gave back in the frame of a switch says so
    /// when its tab is shown again, even with no note of its own.
    #[test]
    fn input_given_back_as_a_tab_is_left_says_so() {
        let mut d = Drafts::default();
        let all = |_: &str| true;
        let mut owner = Some((1, Some("A".to_string())));
        let mut note = None;
        d.frame(&mut owner, Some((2, Some("B"))), Some("refused".into()), &mut note, ("", 0), all);
        assert_eq!(d.map.get("A").map(|x| (x.text.as_str(), x.note)), Some(("refused", Some(REFUSED_NOTE))));
        // A switch with nothing given back files no note.
        let mut note = None;
        d.frame(&mut owner, Some((1, Some("A"))), None, &mut note, ("typed for B", 3), all);
        assert_eq!(d.map.get("B").map(|x| x.note), Some(None));
    }

    /// Nit 7: input given back to the id a tab just followed to (before
    /// the frame caught up) goes in ahead of the composer, not into a
    /// draft no one shows. A tab that goes into the one already holding
    /// its session brings its composer text along, after that tab's draft.
    /// Nit 8: a chain of follows.
    #[test]
    fn a_follow_or_a_merge_loses_no_input() {
        let mut t = Tabs::default();
        t.open("S", "s", Place::Current);
        let tab = t.visible().unwrap().id;
        let mut d = Drafts::default();
        let mut owner = Some((tab, Some("S".to_string())));
        t.attached("S2");
        // An unsent for S lands before the frame: S2's (the app maps it).
        assert_eq!(t.followed("S"), Some("S2"));
        d.give_back("S2".into(), "steer".into(), "unsent");
        let mut note = None;
        let vis = t.visible().map(|x| (x.id, x.session.as_deref()));
        let out = d.frame(&mut owner, vis, None, &mut note, ("half", 4), |s| t.knows(s));
        assert_eq!((out, note), (Some(("steer\nhalf".into(), Some(10))), Some("unsent")));
        assert!(d.map.is_empty());
        // S2 → S3: an unsent for S still finds the tab.
        t.attached("S3");
        assert_eq!((t.followed("S"), t.followed("S2")), (Some("S3"), Some("S3")));
        // A pending tab whose resume lands on a session another tab holds.
        let mut m = Tabs::default();
        m.open("a", "a", Place::New);
        let a = m.visible().unwrap().id;
        d.map.insert("a".into(), Draft { text: "a's draft".into(), cursor: 0, note: None });
        m.open_pending("resume", Place::New);
        let pending = m.visible().unwrap().id;
        let mut owner = Some((pending, None));
        m.attached("a");
        assert_eq!(m.take_merged(), Some((pending, a)));
        assert_eq!(m.take_merged(), None);
        // What the app does with it: the composer is a's.
        owner.as_mut().unwrap().0 = a;
        let vis = m.visible().map(|x| (x.id, x.session.as_deref()));
        let out = d.frame(&mut owner, vis, None, &mut None, ("typed while it opened", 5), |s| m.knows(s));
        assert_eq!(out, Some(("a's draft\ntyped while it opened".into(), Some(15))));
    }

    /// Nit 10: an early settle puts every restored tab on the reopen
    /// stack, more than its usual bound; the next close trims it back.
    #[test]
    fn every_dropped_restore_can_come_back() {
        let n = CLOSED_MAX + 3;
        let sessions: Vec<String> = (0..n).map(|i| format!("s{i}")).collect();
        let mut r = Tabs::restore(&Saved { sessions: sessions.clone(), journals: vec![], labels: vec![], active: Some(0) });
        r.open("new", "new", Place::New);
        assert_eq!((r.dropped, r.dropped_early), (n, true));
        assert!(sessions.iter().all(|s| r.knows(s)), "all {n} on the reopen stack");
        r.open("other", "other", Place::New);
        r.close(0);
        let mut back = 0;
        while r.reopen() {
            back += 1;
        }
        assert_eq!(back, CLOSED_MAX);
    }

    #[test]
    fn the_cache_is_bounded_in_bytes_too() {
        let mut c: Lru<&str, ()> = Lru::new(8, 100);
        c.put("a", (), 40, |_| true);
        c.put("b", (), 40, |_| true);
        assert_eq!((c.len(), c.bytes()), (2, 80));
        c.put("c", (), 40, |_| true);
        assert_eq!((c.len(), c.bytes(), c.contains(&"a")), (2, 80, false), "the oldest went for the budget");
        c.put("d", (), 40, |k| *k == "b" || *k == "d");
        assert!(c.contains(&"b") && c.contains(&"d") && !c.contains(&"c"), "one no tab holds goes first");
        c.put("huge", (), 500, |_| true);
        assert_eq!((c.len(), c.bytes()), (0, 0), "bigger than the budget: not kept");
        assert_eq!(c.take(&"b"), None);
    }

    #[test]
    fn the_cache_keeps_the_newest_and_prefers_dropping_closed_tabs() {
        let mut c: Lru<&str, u32> = Lru::new(3, usize::MAX);
        for (i, k) in ["a", "b", "c", "d"].into_iter().enumerate() {
            c.put(k, i as u32, 1, |_| true);
        }
        assert_eq!(c.len(), 3);
        assert!(!c.contains(&"a"), "the oldest went");
        // Prefer dropping one no tab holds.
        c.put("e", 9, 1, |k| *k != "c");
        assert!(c.contains(&"b") && c.contains(&"d") && c.contains(&"e") && !c.contains(&"c"));
        assert_eq!(c.take(&"b"), Some(1));
        assert_eq!(c.len(), 2);
        c.put("d", 5, 1, |_| true);
        assert_eq!((c.len(), c.take(&"d")), (2, Some(5)), "re-put replaces, no growth");
    }

    /// Any sequence of operations keeps the invariants.
    #[test]
    fn invariants_hold_under_any_sequence() {
        let mut t = Tabs::default();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..5000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let s = ((x >> 8) % 6).to_string();
            let i = ((x >> 16) % 7) as usize;
            let place = if x & 1 == 0 { Place::Current } else { Place::New };
            let before = format!(
                "{:?} active {:?} last {:?}",
                t.iter().map(|x| (x.id, x.kind, x.session.clone())).collect::<Vec<_>>(),
                t.active,
                t.last_session
            );
            let op = (x >> 24) % 12;
            match (x >> 24) % 12 {
                0 | 1 => t.open(&s, &s, place),
                2 => t.open_pending("p", place),
                3 => {
                    t.close(i);
                }
                4 => {
                    t.reopen();
                }
                5 => {
                    t.move_tab(i, (x >> 32) as usize % 7);
                }
                6 => {
                    t.step(if x & 2 == 0 { 1 } else { -1 });
                }
                7 => {
                    t.focus(i);
                }
                // M1: the views, and closing whatever is on screen (a view
                // over a pending tab must give it back, never strand it).
                9 => {
                    t.open_view(if x & 4 == 0 { Kind::Settings } else { Kind::Usage });
                }
                8 => {
                    let h = format!("sa_{}", (x >> 40) % 3);
                    t.open_worker(&s, &h, "w", &s);
                    assert!(t.current_worker().is_some_and(|w| w.handle == h));
                }
                10 => {
                    if let Some(a) = t.active() {
                        let behind = t.view().and(t.current().map(|c| c.id));
                        let before = format!(
                            "{:?} active {:?} last {:?}",
                            t.iter().map(|x| (x.id, x.kind)).collect::<Vec<_>>(),
                            t.active,
                            t.last_session
                        );
                        t.close(a);
                        if let Some(id) = behind {
                            assert_eq!(t.visible().map(|v| v.id), Some(id), "a closed view gives way to the session behind it: {before}");
                        }
                    }
                }
                _ => {
                    if t.visible().is_some() {
                        t.attached(&s)
                    }
                }
            }
            let ok = t.last_session.is_none_or(|id| t.iter().find(|x| x.id == id).is_none_or(|x| x.kind.attaches()));
            assert!(
                ok,
                "op {op} s {s} i {i}: {before} -> {:?} active {:?} last {:?}",
                t.iter().map(|x| (x.id, x.kind, x.session.clone())).collect::<Vec<_>>(),
                t.active,
                t.last_session
            );
            unique(&t);
        }
    }

    #[test]
    fn views_are_singletons_at_the_end_and_keep_the_session_current() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open("b", "b", Place::New);
        t.focus(0);
        assert!(t.open_view(Kind::Settings));
        assert_eq!((t.len(), t.view()), (3, Some(Kind::Settings)));
        assert_eq!(t.visible_session(), None, "no rail row is the one on screen");
        assert_eq!(t.current_session(), Some("a"), "the session tab shown last stays current");
        assert_eq!(t.iter().last().map(|x| x.kind), Some(Kind::Settings), "it opens at the end");
        t.open_view(Kind::Usage);
        t.focus(1);
        assert_eq!(t.current_session(), Some("b"));
        assert!(t.open_view(Kind::Settings), "focused, not opened again");
        assert!(!t.open_view(Kind::Settings), "already on screen");
        assert_eq!((t.len(), t.current_session()), (4, Some("b")));
        unique(&t);
    }

    #[test]
    fn a_session_opened_from_a_view_gets_its_own_tab_and_the_view_stays() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open_view(Kind::Usage);
        t.open("b", "b", Place::Current);
        assert_eq!((ids(&t), vis(&t), t.current_session()), (vec!["a", "…", "b"], Some("b"), Some("b")));
        t.open_view(Kind::Usage);
        t.open("a", "a", Place::Current);
        assert_eq!((vis(&t), t.len()), (Some("a"), 3), "an open session's tab is focused");
        t.open_view(Kind::Usage);
        t.open_pending("New session", Place::Current);
        assert_eq!((t.len(), t.view()), (4, None), "a create from a view opens a tab too");
        unique(&t);
    }

    /// The merge with PR #14: `submit()` and `sync_composer()` guard with
    /// [`Drafts::composer_owned`] (the current session tab; `app.rs`'s
    /// `composer_guard_is_the_one_helper` pins the call sites). A view on
    /// screen (the visible tab) must neither take the composer's text nor
    /// make a send refused as "switching tabs".
    #[test]
    fn a_view_on_screen_leaves_the_composer_with_the_session_tab() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        let a = t.visible().unwrap().id;
        let mut owner = Some((a, Some("a".to_string())));
        for kind in [Kind::Settings, Kind::Usage] {
            t.open_view(kind);
            assert_ne!(t.visible().map(|x| x.id), Some(a));
            assert!(!Drafts::owns(owner.as_ref(), t.visible().map(|x| (x.id, x.session.as_deref()))), "the visible tab is the view");
            assert!(Drafts::composer_owned(owner.as_ref(), &t), "{kind:?} on screen: the composer is still a's");
            let mut d = Drafts::default();
            let mut note = None;
            assert_eq!(d.frame(&mut owner, t.composer_tab(), None, &mut note, ("half typed", 4), |s| t.knows(s)), None, "no swap");
            assert!(d.map.is_empty() && owner == Some((a, Some("a".to_string()))));
        }
        // Back on a: still a's.
        t.focus(0);
        assert!(Drafts::composer_owned(owner.as_ref(), &t));
    }

    /// Nit 1: the tab an attach opens behind a view, with no current tab,
    /// goes before the views (they stay at the end of the strip) and the
    /// view on screen stays.
    #[test]
    fn an_attach_behind_a_view_opens_its_tab_before_the_views() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open_view(Kind::Settings);
        t.open_view(Kind::Usage);
        t.focus(0);
        t.close(0);
        let on = t.visible().map(|v| v.id);
        assert_eq!((t.view(), t.current().map(|c| c.id)), (Some(Kind::Settings), None));
        t.attached("b");
        let kinds: Vec<Kind> = t.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, [Kind::Session, Kind::Settings, Kind::Usage]);
        assert_eq!((t.visible().map(|v| v.id), t.current_session()), (on, Some("b")));
        unique(&t);
    }

    /// Found by the property test once views joined it: the current tab
    /// behind a view closed, then an attach lands on a session another tab
    /// holds. That tab becomes current; never a second tab for it.
    #[test]
    fn an_attach_behind_a_view_with_no_current_tab_never_duplicates() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open_view(Kind::Settings);
        t.open_pending("New session", Place::New);
        let ix = t.active().unwrap();
        t.close(ix);
        assert_eq!((t.view(), t.current().map(|c| c.id)), (Some(Kind::Settings), None));
        t.attached("a");
        assert_eq!((ids(&t).len(), t.current_session(), t.view()), (2, Some("a"), Some(Kind::Settings)));
        unique(&t);
    }

    #[test]
    fn views_are_not_saved_or_reopened() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open("b", "b", Place::New);
        t.open("c", "c", Place::New);
        t.focus(0);
        t.open_view(Kind::Settings);
        let s = t.saved();
        assert_eq!(s.active, Some(0), "the current session is the active one");
        let ix = t.active().unwrap();
        assert!(t.close(ix));
        assert_eq!(vis(&t), Some("a"), "M1: back to the session behind it, not its neighbour c");
        assert!(!t.reopen(), "nothing to bring back");
        unique(&t);
    }

    /// M1: a pending create behind a view, not the rightmost tab, is the
    /// one shown again when the view closes (never stranded in the
    /// background with its reply dropped).
    #[test]
    fn closing_a_view_over_a_pending_tab_gives_it_back() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open("b", "b", Place::New);
        t.focus(0);
        t.open_pending("New session", Place::New);
        assert_eq!(ids(&t), vec!["a", "…", "b"]);
        assert!(t.current().is_some_and(Tab::pending));
        t.open_view(Kind::Usage);
        let ix = t.active().unwrap();
        t.close(ix);
        assert!(t.visible().is_some_and(Tab::pending), "the pending tab is shown again: {:?}", ids(&t));
        unique(&t);
    }

    #[test]
    fn a_pending_tab_behind_a_view_is_named_there_or_dropped_when_left() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open_pending("New session", Place::New);
        t.open_view(Kind::Settings);
        assert!(t.current().is_some_and(Tab::pending));
        t.attached("c");
        assert_eq!((ids(&t), t.view()), (vec!["a", "c", "…"], Some(Kind::Settings)), "named; the view stays on screen");
        // An attach that lands on a session another tab holds: the pending one goes.
        t.focus(0);
        t.open_pending("New session", Place::New);
        t.open_view(Kind::Settings);
        t.attached("c");
        assert_eq!((ids(&t), t.view(), t.current_session()), (vec!["a", "c", "…"], Some(Kind::Settings), Some("c")));
        // Left for another session tab: a pending one is gone for good.
        t.open_pending("New session", Place::New);
        t.open_view(Kind::Settings);
        t.focus(0);
        assert_eq!(ids(&t), vec!["a", "c", "…"]);
        unique(&t);
    }

    #[test]
    fn closing_the_current_session_behind_a_view_leaves_none() {
        let mut t = Tabs::default();
        t.open("a", "a", Place::Current);
        t.open_view(Kind::Usage);
        t.close(0);
        assert_eq!((t.current_session(), t.view()), (None, Some(Kind::Usage)));
        // Closing a session tab moves the current one to the tab shown.
        t.open("x", "x", Place::Current);
        t.open("y", "y", Place::New);
        let y = t.active().unwrap();
        t.close(y);
        t.open_view(Kind::Usage);
        assert_eq!(t.current_session(), Some("x"));
        unique(&t);
    }
}
