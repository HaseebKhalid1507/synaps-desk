//! Session tabs without GPUI: which sessions are open, in what order, which
//! one is on screen, and the stack Ctrl+Shift+T reopens from. One tab is one
//! session, and a session is in one tab at most: opening it again focuses
//! its tab. Only the visible tab is attached ([`crate::app`] keeps the
//! others' last transcript in a bounded cache); a background tab's glyph
//! comes from the session poll.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use synaps_client::protocol::SessionMeta;

/// Tabs Ctrl+Shift+T can bring back (closed ones, newest last).
pub const CLOSED_MAX: usize = 10;

/// Where a session opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Place {
    /// In the visible tab, replacing what it shows (a new tab if none).
    #[default]
    Current,
    /// In a new tab right after the visible one.
    New,
}

/// What a tab holds. Only sessions for now; Settings and Usage are to open
/// as views in the centre pane, each in a tab of its own kind (no session:
/// nothing attached, cached, saved or listed for it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Session,
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
}

impl Tab {
    /// A session tab whose create or resume has not attached yet.
    pub fn pending(&self) -> bool {
        self.kind == Kind::Session && self.session.is_none()
    }
}

/// The tabs as the settings file keeps them: session ids in order and the
/// visible one's index among them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub sessions: Vec<String>,
    pub active: Option<usize>,
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
}

impl Tabs {
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

    pub fn visible_session(&self) -> Option<&str> {
        self.visible()?.session.as_deref()
    }

    /// The tab holding `session`.
    pub fn find(&self, session: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.session.as_deref() == Some(session))
    }

    pub fn is_open(&self, session: &str) -> bool {
        self.find(session).is_some()
    }

    fn changed(&mut self) {
        self.generation += 1;
    }

    fn tab(&mut self, session: Option<String>, label: String) -> Tab {
        self.next_id += 1;
        Tab { id: self.next_id, kind: Kind::Session, label, session, journal: None, cwd: None }
    }

    /// Make the tab `id` visible. A visible tab still waiting for its
    /// session (a create or a resume) is left for good: nothing would come
    /// back to it.
    fn show(&mut self, id: u64) {
        if let Some(i) = self.active.filter(|&i| self.tabs[i].pending() && self.tabs[i].id != id) {
            self.tabs.remove(i);
        }
        self.active = self.tabs.iter().position(|t| t.id == id);
        self.changed();
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
        match (place, self.active) {
            (Place::Current, Some(i)) => {
                let t = &mut self.tabs[i];
                t.session = Some(session.to_string());
                t.label = label.to_string();
                t.journal = None;
                t.cwd = None;
                self.changed();
            }
            _ => {
                let t = self.tab(Some(session.to_string()), label.to_string());
                self.insert_after_visible(t);
            }
        }
    }

    /// A tab for a session the daemon has not named yet (a create, a
    /// resume), called `label` until it does.
    pub fn open_pending(&mut self, label: &str, place: Place) {
        match (place, self.active) {
            (Place::Current, Some(i)) => {
                let t = &mut self.tabs[i];
                t.session = None;
                t.label = label.to_string();
                t.journal = None;
                t.cwd = None;
                self.changed();
            }
            _ => {
                let t = self.tab(None, label.to_string());
                self.insert_after_visible(t);
            }
        }
    }

    /// Close tab `ix` (the session is only detached from, never ended). The
    /// visible one gives way to its right neighbour, else its left.
    pub fn close(&mut self, ix: usize) -> bool {
        if ix >= self.tabs.len() {
            return false;
        }
        let t = self.tabs.remove(ix);
        if t.session.is_some() {
            self.closed.push((ix, t));
            if self.closed.len() > CLOSED_MAX {
                self.closed.remove(0);
            }
        }
        self.active = match self.active {
            Some(a) if a == ix => (!self.tabs.is_empty()).then(|| ix.min(self.tabs.len() - 1)),
            Some(a) if a > ix => Some(a - 1),
            a => a,
        };
        self.changed();
        true
    }

    /// Ctrl+Shift+T: the last closed tab, where it was (a session open
    /// again since is focused instead).
    pub fn reopen(&mut self) -> bool {
        let Some((ix, t)) = self.closed.pop() else { return false };
        if let Some(j) = t.session.as_deref().and_then(|s| self.find(s)) {
            self.focus(j);
            return true;
        }
        let (id, at) = (t.id, ix.min(self.tabs.len()));
        self.tabs.insert(at, t);
        if let Some(a) = self.active.as_mut().filter(|a| **a >= at) {
            *a += 1;
        }
        self.show(id);
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
        match self.active {
            Some(i) if self.tabs[i].session.as_deref() == Some(session) => {}
            Some(i) => {
                if let Some(j) = self.find(session) {
                    self.tabs.remove(i);
                    self.active = Some(if j > i { j - 1 } else { j });
                } else {
                    self.tabs[i].session = Some(session.to_string());
                }
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

    /// The tabs to save (a tab still waiting for its session is not).
    pub fn saved(&self) -> Saved {
        let mut sessions = Vec::with_capacity(self.tabs.len());
        let mut active = None;
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(s) = &t.session {
                if self.active == Some(i) {
                    active = Some(sessions.len());
                }
                sessions.push(s.clone());
            }
        }
        Saved { sessions, active }
    }

    /// The saved tabs whose sessions `exists` (a session open twice opens
    /// once), and how many were dropped. The saved visible one is visible
    /// again, else the nearest kept tab before it.
    pub fn restore(saved: &Saved, exists: impl Fn(&str) -> bool) -> (Tabs, usize) {
        let mut t = Tabs::default();
        let mut dropped = 0;
        let mut active = None;
        for (i, s) in saved.sessions.iter().enumerate() {
            if !exists(s) || t.is_open(s) {
                dropped += 1;
                continue;
            }
            let tab = t.tab(Some(s.clone()), s.clone());
            t.tabs.push(tab);
            if saved.active.is_some_and(|a| i <= a) {
                active = Some(t.tabs.len() - 1);
            }
        }
        t.active = active.or((!t.tabs.is_empty()).then_some(0));
        (t, dropped)
    }
}

/// A bounded least-recently-used map: the background tabs' caches. `put`
/// past `cap` drops the oldest entry, preferring one `keep` does not want.
#[derive(Debug)]
pub struct Lru<K, V> {
    cap: usize,
    order: VecDeque<K>,
    map: HashMap<K, V>,
}

impl<K: Clone + Eq + Hash, V> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Lru { cap, order: VecDeque::with_capacity(cap + 1), map: HashMap::with_capacity(cap + 1) }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    /// Take `k` out (it is on screen now).
    pub fn take(&mut self, k: &K) -> Option<V> {
        let v = self.map.remove(k)?;
        self.order.retain(|o| o != k);
        Some(v)
    }

    /// Insert `k` as the newest; past the cap the oldest entry not `keep`
    /// goes (the oldest of all if every one is kept).
    pub fn put(&mut self, k: K, v: V, keep: impl Fn(&K) -> bool) {
        self.take(&k);
        self.order.push_back(k.clone());
        self.map.insert(k, v);
        while self.map.len() > self.cap {
            let at = self.order.iter().position(|o| !keep(o)).unwrap_or(0);
            if let Some(old) = self.order.remove(at) {
                self.map.remove(&old);
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

    /// No two tabs ever hold one session, whatever the operations.
    fn unique(t: &Tabs) {
        let open: Vec<&str> = t.iter().filter_map(|t| t.session.as_deref()).collect();
        let mut dedup = open.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), open.len(), "two tabs for one session: {open:?}");
        assert_eq!(t.active.is_some(), !t.is_empty(), "a visible tab iff there are tabs");
        assert!(t.active.is_none_or(|a| a < t.len()));
        assert!(t.iter().enumerate().all(|(i, x)| !x.pending() || t.active == Some(i)), "a pending tab in the background");
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
        assert_eq!(t.saved(), Saved { sessions: vec!["a".into(), "b".into()], active: Some(1) });
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

    #[test]
    fn saved_tabs_round_trip_and_drop_gone_sessions() {
        let mut t = Tabs::default();
        for s in ["a", "b", "c", "d"] {
            t.open(s, s, Place::New);
        }
        t.focus(2);
        let saved = t.saved();
        assert_eq!(saved, Saved { sessions: vec!["a".into(), "b".into(), "c".into(), "d".into()], active: Some(2) });
        let (r, dropped) = Tabs::restore(&saved, |_| true);
        assert_eq!((ids(&r), vis(&r), dropped), (vec!["a", "b", "c", "d"], Some("c"), 0));
        // The visible one is gone: the nearest kept one before it.
        let (r, dropped) = Tabs::restore(&saved, |s| s != "c" && s != "a");
        assert_eq!((ids(&r), vis(&r), dropped), (vec!["b", "d"], Some("b"), 2));
        let (r, dropped) = Tabs::restore(&saved, |_| false);
        assert_eq!((r.len(), vis(&r), dropped), (0, None, 4));
        // A hand-edited file naming a session twice opens it once.
        let twice = Saved { sessions: vec!["a".into(), "a".into()], active: Some(1) };
        let (r, dropped) = Tabs::restore(&twice, |_| true);
        assert_eq!((ids(&r), vis(&r), dropped), (vec!["a"], Some("a"), 1));
        unique(&r);
    }

    #[test]
    fn the_cache_keeps_the_newest_and_prefers_dropping_closed_tabs() {
        let mut c: Lru<&str, u32> = Lru::new(3);
        for (i, k) in ["a", "b", "c", "d"].into_iter().enumerate() {
            c.put(k, i as u32, |_| true);
        }
        assert_eq!(c.len(), 3);
        assert!(!c.contains(&"a"), "the oldest went");
        // Prefer dropping one no tab holds.
        c.put("e", 9, |k| *k != "c");
        assert!(c.contains(&"b") && c.contains(&"d") && c.contains(&"e") && !c.contains(&"c"));
        assert_eq!(c.take(&"b"), Some(1));
        assert_eq!(c.len(), 2);
        c.put("d", 5, |_| true);
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
            match (x >> 24) % 9 {
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
                _ => {
                    if t.visible().is_some() {
                        t.attached(&s)
                    }
                }
            }
            unique(&t);
        }
    }
}
