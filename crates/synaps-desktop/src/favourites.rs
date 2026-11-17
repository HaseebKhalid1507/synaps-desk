//! Favourite sessions: a desktop-only bookmark list, per daemon, in the
//! order the user favourited them. Never a daemon pin: favouriting attaches,
//! pins, parks and keeps warm nothing (it costs no tokens).
//!
//! A favourite is keyed by the session's journal id (`SessionMeta.journal_id`
//! for a held session, `SavedSession.id` for a saved one): it survives a
//! park, a resume and a daemon restart; a live id does not. A held session
//! whose journal id is still empty is keyed by its live id until the journal
//! id appears. Never by a row's position or label.
//!
//! The journal id moves: a LinkedSuccessor compaction gives the same
//! conversation a new one (the TUI's sessions on the daemon, every pin the
//! daemon restores), and the saved listing folds the old one into it. So a
//! favourite keeps every id it has been seen under (`aliases`) and follows
//! the chain forward: a held session's new journal id (it is still matched
//! by its live id), and the journal a resume of it attached to
//! ([`Favourites::follow`]). One conversation is one favourite.
//!
//! Each keeps a snapshot (label, folder, model, last update): the saved
//! listing is newest first and capped, so an old favourite may not be in it,
//! and its row must still show and resume (by journal id, in its folder).

use serde_json::{Map, Value};
use synaps_client::SavedSession;
use synaps_client::protocol::SessionMeta;

/// The ids a favourite keeps besides its own: a compaction adds one.
const MAX_ALIASES: usize = 16;

/// The keys of an entry this build reads; any other is kept as it was.
const KNOWN: [&str; 10] = ["id", "aliases", "provisional", "label", "named", "cwd", "model", "updated_at", "added_at", "gone"];

/// One favourite, as saved under `favourites.<daemon key>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favourite {
    /// The journal id (the newest seen); a held session's live id while it
    /// has none yet (`provisional`).
    pub id: String,
    /// Every other id it has been seen under, oldest first: the journal
    /// ids a compaction moved it from, and the live ids that held it.
    pub aliases: Vec<String>,
    pub provisional: bool,
    /// What it was last called (its name, else its title).
    pub label: String,
    /// It has a name (the label is its handle).
    pub named: bool,
    /// Its folder on the daemon's machine (the resume's cwd).
    pub cwd: Option<String>,
    pub model: Option<String>,
    /// RFC 3339, as the daemon last said it.
    pub updated_at: Option<String>,
    /// Unix seconds.
    pub added_at: i64,
    /// A resume found no journal for it: shown as gone until removed, or
    /// seen again (held, or in a listing asked for after the verdict: the
    /// listing's number, [`crate::model::Named::asks`]). Never pruned
    /// silently. Saved as a flag (read back: 0, any listing clears it).
    pub gone: Option<u64>,
    /// The entry's keys this build does not know (a newer one's), written
    /// back as they were.
    pub extra: Map<String, Value>,
}

/// What a favourite is made from: a held session or a saved one.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Live(&'a SessionMeta),
    Saved(&'a SavedSession),
}

/// What a sighting changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    None,
    /// Its update time only: kept, written with the next write (or at
    /// quit), never a write by itself.
    Quiet,
    /// Its ids, its gone flag, or what its row shows: the rail rebuilds and
    /// the file is written.
    Shown,
}

impl Favourite {
    /// A new favourite of `src`, added at `now` (Unix seconds).
    pub fn of(src: Source<'_>, now: i64) -> Favourite {
        let (id, provisional) = match src {
            Source::Live(s) if s.journal_id.is_empty() => (s.id.clone(), true),
            Source::Live(s) => (s.journal_id.clone(), false),
            Source::Saved(v) => (v.id.clone(), false),
        };
        let mut f = Favourite {
            id,
            aliases: Vec::new(),
            provisional,
            label: String::new(),
            named: false,
            cwd: None,
            model: None,
            updated_at: None,
            added_at: now,
            gone: None,
            extra: Map::new(),
        };
        f.take(src, 0);
        f
    }

    /// Its id, then its aliases.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    /// `id` (a journal or live id) is one it has been seen under.
    pub fn knows(&self, id: &str) -> bool {
        !id.is_empty() && self.ids().any(|i| i == id)
    }

    /// `src` is this favourite.
    pub fn is(&self, src: Source<'_>) -> bool {
        match src {
            Source::Live(s) => self.knows(&s.id) || self.knows(&s.journal_id),
            Source::Saved(v) => self.knows(&v.id),
        }
    }

    /// `o` is the same conversation (they share an id).
    pub fn same(&self, o: &Favourite) -> bool {
        o.ids().any(|i| self.knows(i))
    }

    pub fn is_gone(&self) -> bool {
        self.gone.is_some()
    }

    /// Remember `id` too (the oldest is let go past [`MAX_ALIASES`]).
    fn alias(&mut self, id: &str) {
        if self.knows(id) || id.is_empty() {
            return;
        }
        self.aliases.push(id.to_string());
        if self.aliases.len() > MAX_ALIASES {
            self.aliases.remove(0);
        }
    }

    /// Keyed by `journal` from now on, the old key kept. Never back to an
    /// id it has left (a listing older than the sighting that moved it).
    fn rekey(&mut self, journal: &str) {
        if journal.is_empty() || journal == self.id || self.aliases.iter().any(|a| a == journal) {
            return;
        }
        let old = std::mem::replace(&mut self.id, journal.to_string());
        self.provisional = false;
        self.alias(&old);
    }

    /// The other's ids, kept as aliases (two entries found to be one).
    fn absorb(&mut self, o: &Favourite) {
        for id in o.ids() {
            self.alias(id);
        }
    }

    /// The snapshot from `src` (and the journal id a held session has now),
    /// seen in the listing numbered `listed` (a held sighting: any).
    fn take(&mut self, src: Source<'_>, listed: u64) -> Change {
        let before = self.clone();
        let text = |s: &Option<String>| s.as_deref().filter(|t| !t.is_empty()).map(str::to_string);
        let (name, title, cwd, model, updated) = match src {
            Source::Live(s) => {
                // Matched by its live id, its journal moved on (a
                // compaction): it follows; the live id still matches it.
                self.rekey(&s.journal_id);
                self.alias(&s.id);
                // Held: it is there.
                self.gone = None;
                // A held session has no update time of its own: its state's.
                let updated = s.state_since.clone().or_else(|| self.updated_at.clone());
                (text(&s.name), text(&s.title), text(&s.cwd), Some(s.model.clone()).filter(|m| !m.is_empty()), updated)
            }
            Source::Saved(v) => {
                // Only a listing asked for after the verdict says it is there.
                if self.gone.is_some_and(|at| listed > at) {
                    self.gone = None;
                }
                (text(&v.name), text(&v.title), text(&v.cwd), text(&v.model), v.updated_at.clone().or(v.created_at.clone()))
            }
        };
        self.named = name.is_some();
        if let Some(l) = name.or(title) {
            self.label = l;
        } else if self.label.is_empty() {
            self.label = self.id.clone();
        }
        self.cwd = cwd.or(self.cwd.take());
        self.model = model.or(self.model.take());
        self.updated_at = updated.or(self.updated_at.take());
        if *self == before {
            Change::None
        } else if (Favourite { updated_at: before.updated_at.clone(), ..self.clone() }) == before {
            Change::Quiet
        } else {
            Change::Shown
        }
    }

    /// This instance's entry over `t`, the file's entry for the same
    /// conversation: its fields, their ids and unknown keys kept too.
    fn over(&self, t: &Favourite) -> Favourite {
        let mut f = self.clone();
        f.absorb(t);
        f.extra = t.extra.clone();
        for (k, v) in &self.extra {
            f.extra.entry(k.clone()).or_insert_with(|| v.clone());
        }
        f
    }
}

/// The favourites of the daemon on screen, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Favourites {
    pub list: Vec<Favourite>,
    /// Bumped on every change worth a write (the save is debounced on it).
    pub generation: u64,
    /// An update time moved since the last write (written at quit, or with
    /// the next write).
    pub quiet: bool,
    /// The list as the file had it when this instance last read or wrote
    /// it: what [`merge`] tells this instance's adds and removes by.
    pub base: Vec<Favourite>,
}

impl Favourites {
    /// The list as read from the file.
    pub fn new(list: Vec<Favourite>) -> Self {
        Favourites { base: list.clone(), list, generation: 0, quiet: false }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The favourite `src` is, if it is one.
    pub fn find(&self, src: Source<'_>) -> Option<usize> {
        self.list.iter().position(|f| f.is(src))
    }

    pub fn contains(&self, src: Source<'_>) -> bool {
        self.find(src).is_some()
    }

    /// The favourite seen under `id` (any of its ids), if any.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.list.iter().position(|f| f.knows(id))
    }

    /// Favourite `src` (at the end), or unfavourite it. `true`: it is one now.
    pub fn toggle(&mut self, src: Source<'_>, now: i64) -> bool {
        self.generation += 1;
        match self.find(src) {
            Some(i) => {
                self.list.remove(i);
                false
            }
            None => {
                self.list.push(Favourite::of(src, now));
                true
            }
        }
    }

    /// Unfavourite the one seen under `id` (a row's star, a gone row's
    /// Remove).
    pub fn remove(&mut self, id: &str) -> bool {
        let n = self.list.len();
        self.list.retain(|f| !f.knows(id));
        let removed = self.list.len() != n;
        if removed {
            self.generation += 1;
        }
        removed
    }

    /// A resume of `id` found no journal, after `asks` listings were asked
    /// for: its row says gone until it is seen again.
    pub fn set_gone(&mut self, id: &str, asks: u64) -> bool {
        match self.list.iter_mut().find(|f| f.knows(id) && f.gone.is_none()) {
            Some(f) => {
                f.gone = Some(asks);
                self.generation += 1;
                true
            }
            None => false,
        }
    }

    /// A resume of `target` attached `meta`: the daemon followed the
    /// compaction chain to its newest journal. The favourite follows.
    pub fn follow(&mut self, target: &str, meta: &SessionMeta) -> bool {
        let Some(f) = self.list.iter_mut().find(|f| f.knows(target)) else { return false };
        let change = f.take(Source::Live(meta), 0);
        self.settle(change)
    }

    /// Every favourite held (`sessions`) or listed (`saved`, the listing
    /// numbered `listed`): its snapshot refreshed, its key moved to the
    /// journal id it has now, and seen, so not gone. A held one wins over
    /// its listing (it is the newer). `true` if its row or its ids changed
    /// (the rail rebuilds, the file is written); an update time alone is
    /// kept quietly.
    pub fn refresh<'a>(&mut self, sessions: &[SessionMeta], saved: impl Iterator<Item = &'a SavedSession> + Clone, listed: u64) -> bool {
        let mut change = Change::None;
        for f in &mut self.list {
            let live = sessions.iter().find(|s| s.lifecycle != "ending" && f.is(Source::Live(s)));
            let src = match live {
                Some(s) => Some(Source::Live(s)),
                None => saved.clone().find(|v| f.is(Source::Saved(v))).map(Source::Saved),
            };
            if let Some(src) = src {
                change = change.max(f.take(src, listed));
            }
        }
        self.settle(change)
    }

    /// After sightings: two found to be one conversation (favourited by its
    /// live id and again from its saved row, or before and after a
    /// compaction) are one, the first kept. `true` if a write is due.
    fn settle(&mut self, change: Change) -> bool {
        let merged = dedupe(&mut self.list);
        self.quiet |= change == Change::Quiet;
        let shown = merged || change == Change::Shown;
        if shown {
            self.generation += 1;
        }
        shown
    }

    /// The file now has `file`, written from `sent` (this list as it was
    /// when the write began): the list takes another instance's adds and
    /// removes, keeping its own changes made since.
    pub fn written(&mut self, sent: &[Favourite], file: Vec<Favourite>) -> bool {
        let now = merge(&file, sent, &self.list);
        self.base = file;
        let changed = now != self.list;
        self.list = now;
        changed
    }
}

/// Entries of one conversation folded into the first. `true` if any were.
fn dedupe(list: &mut Vec<Favourite>) -> bool {
    let n = list.len();
    let mut out: Vec<Favourite> = Vec::with_capacity(n);
    for f in list.drain(..) {
        match out.iter_mut().find(|g| g.same(&f)) {
            Some(g) => g.absorb(&f),
            None => out.push(f),
        }
    }
    *list = out;
    list.len() != n
}

/// What to write over `theirs` (the file's list now), given `base` (the
/// list as this instance last read or wrote it) and `ours` (its list now):
/// this instance's adds and removes since `base`, and its snapshot of each
/// both have; every add and remove another instance made kept. Two
/// instances on one daemon do not erase each other's favourites.
pub fn merge(theirs: &[Favourite], base: &[Favourite], ours: &[Favourite]) -> Vec<Favourite> {
    let mut out: Vec<Favourite> = Vec::new();
    for t in theirs {
        match ours.iter().find(|o| o.same(t)) {
            Some(o) => out.push(o.over(t)),
            // Unfavourited here.
            None if base.iter().any(|b| b.same(t)) => {}
            // Another instance's (or one this one never saw).
            None => out.push(t.clone()),
        }
    }
    for o in ours {
        // Favourited here (again, if `added_at` differs); in `base` as it
        // is and not in the file: unfavourited by another instance.
        let there = base.iter().any(|b| b.same(o) && b.added_at == o.added_at);
        if !there && !out.iter().any(|f| f.same(o)) {
            out.push(o.clone());
        }
    }
    dedupe(&mut out);
    out
}

/// A resume of `target` failed because the daemon has no journal for it:
/// its create refused with the resolver's own words for that id, as SynapsCLI
/// says them (`daemon/conn.rs`: `create session: {e}`, `e` ending
/// `core/session.rs`'s `no session matching '{id}'`). Nothing else: a
/// connect, attach, timeout, lock, busy or auth failure (the client's own
/// text included) is not gone.
pub fn journal_missing(reason: &str, target: &str) -> bool {
    !target.is_empty()
        && reason.strip_prefix("create session: ").is_some_and(|why| why.contains(&format!("no session matching '{target}'")))
}

/// The settings file's form: `favourites.<daemon key>` is a list of these.
pub fn to_json(list: &[Favourite]) -> Value {
    Value::Array(
        list.iter()
            .map(|f| {
                let mut o = f.extra.clone();
                o.insert("id".into(), f.id.clone().into());
                if !f.aliases.is_empty() {
                    o.insert("aliases".into(), f.aliases.clone().into());
                }
                o.insert("label".into(), f.label.clone().into());
                o.insert("cwd".into(), f.cwd.clone().into());
                o.insert("model".into(), f.model.clone().into());
                o.insert("updated_at".into(), f.updated_at.clone().into());
                o.insert("added_at".into(), f.added_at.into());
                for (k, on) in [("provisional", f.provisional), ("named", f.named), ("gone", f.gone.is_some())] {
                    if on {
                        o.insert(k.into(), true.into());
                    }
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// [`to_json`]'s list read back: an entry without an id is skipped (a
/// hand edit), the rest kept in order, one conversation once; keys this
/// build does not know are kept. Not a list: none.
pub fn from_json(v: &Value) -> Vec<Favourite> {
    let Some(a) = v.as_array() else { return Vec::new() };
    let s = |e: &Map<String, Value>, k: &str| e.get(k).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    let b = |e: &Map<String, Value>, k: &str| e.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut list: Vec<Favourite> = a
        .iter()
        .filter_map(|e| {
            let e = e.as_object()?;
            let id = s(e, "id")?;
            let mut f = Favourite {
                label: s(e, "label").unwrap_or_else(|| id.clone()),
                id,
                aliases: Vec::new(),
                provisional: b(e, "provisional"),
                named: b(e, "named"),
                cwd: s(e, "cwd"),
                model: s(e, "model"),
                updated_at: s(e, "updated_at"),
                added_at: e.get("added_at").and_then(Value::as_i64).unwrap_or(0),
                gone: b(e, "gone").then_some(0),
                extra: e.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
            };
            for a in e.get("aliases").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                f.alias(a);
            }
            Some(f)
        })
        .collect();
    dedupe(&mut list);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(id: &str, journal: &str, title: &str) -> SessionMeta {
        serde_json::from_value(serde_json::json!({
            "id": id, "journal_id": journal, "title": title, "lifecycle": "live", "created_at": "2026-11-01T00:00:00Z",
            "model": "anthropic/claude-opus-4-1", "cwd": "/home/u/relay", "state_since": "2026-11-16T10:00:00Z"
        }))
        .unwrap()
    }

    fn saved(id: &str, title: &str, cwd: &str) -> SavedSession {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "cwd": cwd, "updated_at": "2026-10-01T00:00:00Z", "model": "m", "live": false
        }))
        .unwrap()
    }

    fn none<'a>() -> std::iter::Empty<&'a SavedSession> {
        std::iter::empty()
    }

    /// Brief 1: a held session with no journal id yet is keyed by its live
    /// id, and by its journal id once it has one (the key survives a park
    /// and a resume; the live id does not).
    #[test]
    fn a_live_id_is_upgraded_to_the_journal_id() {
        let mut f = Favourites::default();
        let s = live("20261117-100000-aaaa", "", "Relay");
        assert!(f.toggle(Source::Live(&s), 100));
        assert_eq!((f.list[0].id.as_str(), f.list[0].provisional), ("20261117-100000-aaaa", true));
        let s = live("20261117-100000-aaaa", "j-relay", "Relay");
        assert!(f.refresh(std::slice::from_ref(&s), none(), 0));
        assert_eq!((f.list[0].id.as_str(), f.list[0].provisional), ("j-relay", false));
        // Parked, then resumed under a new live id: the same favourite.
        let again = live("20261117-120000-bbbb", "j-relay", "Relay");
        assert!(f.contains(Source::Live(&again)));
        // Saved: the same journal.
        assert!(f.contains(Source::Saved(&saved("j-relay", "Relay", "/home/u/relay"))));
        // Favourited by its live id, and again from its saved row: one.
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("L", "", "x")), 1);
        f.list.push(Favourite::of(Source::Saved(&saved("J", "x", "/")), 2));
        f.refresh(&[live("L", "J", "x")], none(), 0);
        assert_eq!(f.list.len(), 1);
        assert_eq!(f.list[0].added_at, 1, "the first stays");
    }

    /// H2: a LinkedSuccessor compaction moves the journal id of a session
    /// that stays held (its live id does not move); the favourite follows,
    /// once and twice, and is one favourite held, parked or listed under the
    /// newest id. Never back to an id it left (a stale listing).
    #[test]
    fn a_favourite_follows_its_compactions() {
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("J1", "J1", "Relay")), 1);
        assert_eq!(f.list[0].id, "J1");
        // One compaction, held: J1's actor writes J2.
        let once = live("J1", "J2", "Relay");
        assert!(f.refresh(std::slice::from_ref(&once), none(), 0), "a new key: written");
        assert_eq!((f.list[0].id.as_str(), &f.list[0].aliases[..]), ("J2", &["J1".to_string()][..]));
        assert!(f.contains(Source::Live(&once)));
        // Two: still held by J1's actor.
        let twice = live("J1", "J3", "Relay");
        assert!(f.contains(Source::Live(&twice)), "matched by its live id while its journal moves");
        assert!(f.refresh(std::slice::from_ref(&twice), none(), 0));
        assert_eq!(f.list[0].id, "J3");
        // Parked: the same actor, the same ids.
        let parked = SessionMeta { lifecycle: "parked".into(), ..twice.clone() };
        assert!(!f.refresh(std::slice::from_ref(&parked), none(), 0));
        // Evicted: listed under its chain head (the daemon folds J1, J2 into it).
        let head = saved("J3", "Relay", "/home/u/relay");
        assert!(f.contains(Source::Saved(&head)));
        f.refresh(&[], std::iter::once(&head), 1);
        assert_eq!((f.list.len(), f.list[0].id.as_str()), (1, "J3"));
        // Resumed: the actor is rebuilt as the head.
        assert!(f.contains(Source::Live(&live("J3", "J3", "Relay"))));
        // A listing from before the compaction never moves it back.
        f.refresh(&[], std::iter::once(&saved("J2", "Relay", "/")), 2);
        assert_eq!(f.list[0].id, "J3");
        // Favourited before and after (an old J1, and the head from
        // history): one favourite once a sighting ties them.
        let mut f = Favourites::default();
        f.toggle(Source::Saved(&saved("J1", "Relay", "/")), 1);
        f.toggle(Source::Saved(&saved("J3", "Relay", "/")), 2);
        assert_eq!(f.list.len(), 2, "nothing ties them yet");
        assert!(f.refresh(&[live("J1", "J3", "Relay")], none(), 0));
        assert_eq!((f.list.len(), f.list[0].added_at, f.list[0].id.as_str()), (1, 1, "J3"));
    }

    /// H2: a resume of an old id the daemon followed to the chain's newest
    /// journal (the favourite not held meanwhile): the favourite follows the
    /// attach.
    #[test]
    fn a_resume_that_lands_on_a_newer_journal_is_followed() {
        let mut f = Favourites::new(vec![Favourite::of(Source::Saved(&saved("J1", "Relay", "/home/u/relay")), 1)]);
        assert!(!f.follow("J9", &live("J3", "J3", "Relay")), "not a favourite's resume");
        assert!(f.follow("J1", &live("J3", "J3", "Relay")));
        assert_eq!((f.list[0].id.as_str(), &f.list[0].aliases[..]), ("J3", &["J1".to_string()][..]));
        assert!(f.contains(Source::Saved(&saved("J3", "Relay", "/"))));
        assert_eq!(f.position("J1"), Some(0));
    }

    /// M1: what a held session's every turn changes (its state's time) is
    /// no reason to write; its label, folder, model or ids are.
    #[test]
    fn a_turn_is_not_a_write() {
        let mut f = Favourites::default();
        let mut s = live("L", "J", "Relay");
        f.toggle(Source::Live(&s), 1);
        let g = f.generation;
        s.state_since = Some("2026-11-16T10:05:00Z".into());
        assert!(!f.refresh(std::slice::from_ref(&s), none(), 0));
        assert_eq!((f.generation, f.quiet), (g, true), "kept, not written");
        assert_eq!(f.list[0].updated_at.as_deref(), Some("2026-11-16T10:05:00Z"));
        s.title = Some("Relay, renamed".into());
        assert!(f.refresh(std::slice::from_ref(&s), none(), 0));
        assert_eq!(f.generation, g + 1);
    }

    /// The snapshot follows what the daemon last said: a held session (its
    /// title, folder, model) or its listing; one in neither keeps it.
    #[test]
    fn the_snapshot_follows_the_session() {
        let mut f = Favourites::default();
        let v = saved("j1", "Old title", "/home/u/a");
        f.toggle(Source::Saved(&v), 5);
        assert_eq!((f.list[0].label.as_str(), f.list[0].cwd.as_deref()), ("Old title", Some("/home/u/a")));
        assert!(!f.refresh(&[], std::iter::once(&v), 1), "nothing new");
        let v2 = saved("j1", "New title", "/home/u/b");
        assert!(f.refresh(&[], std::iter::once(&v2), 1));
        assert_eq!((f.list[0].label.as_str(), f.list[0].cwd.as_deref()), ("New title", Some("/home/u/b")));
        let g = f.generation;
        assert!(!f.refresh(&[], none(), 1), "out of the listing: kept as it was");
        assert_eq!((f.generation, f.list[0].label.as_str()), (g, "New title"));
    }

    /// M2: gone holds against a listing asked for before the verdict (the
    /// one cached, or one already on its way); a listing asked for after it,
    /// or the session held, says it is there.
    #[test]
    fn gone_holds_until_it_is_seen_again() {
        let mut f = Favourites::default();
        let v = saved("j1", "Title", "/home/u/a");
        f.toggle(Source::Saved(&v), 5);
        // Two listings asked for so far; the resume failed.
        assert!(f.set_gone("j1", 2));
        assert!(!f.set_gone("j1", 2), "once");
        assert!(!f.refresh(&[], std::iter::once(&v), 2), "the cached listing (ask 2): still gone");
        assert!(f.list[0].is_gone());
        assert!(f.refresh(&[], std::iter::once(&v), 3), "asked for after: there");
        assert!(!f.list[0].is_gone());
        f.set_gone("j1", 3);
        assert!(f.refresh(&[live("L", "j1", "Title")], none(), 0), "held: there");
        assert!(!f.list[0].is_gone());
    }

    /// Brief 2 (and L3): the file's form round-trips (order, the flags, the
    /// ids), keys a newer build added to an entry are kept, and a hand edit
    /// without an id, or an id twice, is dropped.
    #[test]
    fn the_list_round_trips() {
        let mut f = Favourites::default();
        f.toggle(Source::Saved(&saved("j2", "Two", "/b")), 2);
        f.toggle(Source::Live(&live("L1", "", "One")), 1);
        f.toggle(Source::Live(&live("L3", "J3", "Three")), 3);
        f.set_gone("j2", 0);
        assert_eq!(f.list[2].aliases, ["L3"]);
        let back = from_json(&to_json(&f.list));
        assert_eq!(back, f.list);
        let edited = serde_json::json!([{"label": "no id"}, {"id": "a"}, {"id": "a", "label": "twice"}, "junk"]);
        let back = from_json(&edited);
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].id.as_str(), back[0].label.as_str()), ("a", "a"));
        assert!(from_json(&serde_json::json!({"not": "a list"})).is_empty());
        // A newer build's keys: kept through a read and a write.
        let newer = serde_json::json!([{"id": "a", "label": "A", "colour": "teal", "tags": ["x"]}]);
        let back = to_json(&from_json(&newer));
        assert_eq!((&back[0]["colour"], &back[0]["tags"][0]), (&serde_json::json!("teal"), &serde_json::json!("x")));
    }

    /// M1: two instances on one daemon, each started with the same file:
    /// each one's adds and removes survive the other's write.
    #[test]
    fn two_instances_merge_not_overwrite() {
        let fav = |id: &str, at: i64| Favourite::of(Source::Saved(&saved(id, id, "/")), at);
        let start = vec![fav("a", 1), fav("b", 2)];
        // A adds x; B removes a and adds y.
        let a = vec![fav("a", 1), fav("b", 2), fav("x", 3)];
        let b = vec![fav("b", 2), fav("y", 4)];
        let file = merge(&start, &start, &a);
        assert_eq!(file.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["a", "b", "x"]);
        let file = merge(&file, &start, &b);
        assert_eq!(file.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["b", "x", "y"], "A's x kept, B's remove of a kept");
        // A writes again (a snapshot change): B's work stays; A's list takes it.
        let mut fa = Favourites::new(start.clone());
        fa.list = a.clone();
        fa.list[1].label = "B renamed".into();
        let sent = fa.list.clone();
        let file = merge(&file, &fa.base, &sent);
        assert_eq!(file.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["b", "x", "y"]);
        assert_eq!(file[0].label, "B renamed");
        assert!(fa.written(&sent, file.clone()));
        assert_eq!(fa.list, file);
        // Unfavourited and favourited again here, while another instance
        // dropped it: favourited (a new add).
        let again = merge(&[], &[fav("z", 1)], &[fav("z", 9)]);
        assert_eq!(again.len(), 1);
        // A newer build's keys on another instance's entry: kept.
        let mut theirs = fav("b", 2);
        theirs.extra.insert("colour".into(), "teal".into());
        let out = merge(&[theirs], &start, &start);
        assert_eq!(out[0].extra.get("colour"), Some(&Value::from("teal")));
    }

    /// H1: only the daemon's own refusal for the id resumed is gone; the
    /// client's connect failure (a stopped daemon, a tunnel down: ENOENT on
    /// the socket), a timeout, a lock, busy or auth is not.
    #[test]
    fn a_missing_journal_reads_as_gone() {
        let real = "create session: Tool execution failed: Failed to load session 'j-old': no session matching 'j-old'";
        assert!(journal_missing(real, "j-old"));
        assert!(!journal_missing(real, "j-other"), "another id's refusal");
        assert!(!journal_missing(real, ""));
        for not in [
            "attach gave up: connect /home/u/.synaps-cli/run/daemon.sock: No such file or directory (os error 2)",
            "reconnect gave up: connect /tmp/tunnel.sock: No such file or directory (os error 2)",
            "attach timed out after 10s",
            "'j-old' is live in another process (pid 4)",
            "daemon busy: another create is running",
            "this daemon is served by uid 1001: Permission denied (os error 13)",
            "create session: Failed to load session 'j-old': No such file or directory (os error 2)",
            "Failed to load session 'j-old': no session matching 'j-old'",
            "unknown session j-old",
            "connection reset",
        ] {
            assert!(!journal_missing(not, "j-old"), "{not}");
        }
    }
}
