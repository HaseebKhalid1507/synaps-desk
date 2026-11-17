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
//! A held session's journal id moves, and not only for the same
//! conversation: a LinkedSuccessor compaction gives it a new one, but the
//! TUI's `/clear` (a new, empty conversation) and `/resume` (another one)
//! swap a different journal into the same live session. So a held session
//! is this favourite only by its journal id (its live id says nothing), and
//! a favourite follows a new journal only on the daemon's own word that it
//! is the same conversation: the conversation's header, on the attached
//! session (its attach, and every swap), names the favourite's journal as
//! its `parent_session` (set by compaction only; one hop per header, so a
//! chain is followed swap by swap; [`Favourites::continued`]), or the
//! attached session was opened by a resume of the favourite's journal that
//! the daemon followed down its compactions to the journal it still holds
//! (`continue_info`, F24; any number of hops; [`Favourites::resolved`]).
//! A compaction of a session not attached here is not seen: its favourite
//! and its new journal are two rows until one of those says so (attached
//! after one compaction; after more, once the session has left the daemon
//! and the favourite's resume reopens it: a live session is handed back by
//! its live id, with no word on how it got to the journal it holds). Never
//! a wrong target, never two favourites the user added folded into one.
//!
//! Each keeps a snapshot (label, folder, model, last update): the saved
//! listing is newest first and capped, so an old favourite may not be in it,
//! and its row must still show and resume (by journal id, in its folder).

use serde_json::{Map, Value};
use synaps_client::SavedSession;
use synaps_client::protocol::{ContinueInfo, SessionMeta};

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
    /// Every other id it has been known by, oldest first: the journal ids
    /// a compaction moved it from (and the live id it had while
    /// provisional).
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

    /// `src` is this favourite: by its journal id (a held session's live
    /// id stays when `/clear` or `/resume` swaps another conversation in);
    /// by its live id only while this favourite has no journal id yet.
    pub fn is(&self, src: Source<'_>) -> bool {
        match src {
            Source::Live(s) if self.provisional => s.id == self.id,
            Source::Live(s) => self.knows(&s.journal_id),
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

    /// Keyed by `journal` from now on, the old key kept.
    fn rekey(&mut self, journal: &str) {
        if journal.is_empty() || self.knows(journal) {
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
                // Provisional (keyed by the live id): its journal id once
                // it has one. Nothing else moves it.
                if self.provisional && !s.journal_id.is_empty() {
                    self.rekey(&s.journal_id);
                    self.provisional = false;
                }
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

    /// The attached session's conversation is `header` (its attach's
    /// snapshot, or a `Conversation` after a swap): if it names, as its
    /// `parent_session`, a favourite's journal, it is that favourite's
    /// conversation compacted (the daemon sets it on a compaction's
    /// successor only; `/clear` leaves it empty, `/resume` names the
    /// resumed one's own), and the favourite follows it there. Its journal
    /// now, not an older id it was known by (a chain is linear: a successor
    /// of an older one is a branch). `true`: a favourite moved (written).
    pub fn continued(&mut self, header: &Value) -> bool {
        let id = header.get("id").and_then(Value::as_str).unwrap_or("");
        let parent = header.get("parent_session").and_then(Value::as_str).unwrap_or("");
        self.follow(parent, id)
    }

    /// The attached session was opened by a resume of `info.query` that the
    /// daemon followed down its compactions to `info.session_id` (its
    /// `continue_info`, set at create), and it still holds that journal
    /// (`holds`, its header's id: a session `/clear`ed or `/resume`d since,
    /// or compacted again, says nothing here): the favourite whose journal
    /// was asked for follows it there, however many compactions it was.
    /// `true`: a favourite moved (written).
    pub fn resolved(&mut self, info: &ContinueInfo, holds: &str) -> bool {
        info.compacted() && info.session_id == holds && self.follow(&info.query, holds)
    }

    /// The favourite whose journal is `from` is `to` now, the daemon says.
    /// Not if another favourite already holds `to`: two the user added
    /// stay two.
    fn follow(&mut self, from: &str, to: &str) -> bool {
        if from.is_empty() || to.is_empty() || from == to || self.position(to).is_some() {
            return false;
        }
        let Some(f) = self.list.iter_mut().find(|f| f.id == from) else { return false };
        f.rekey(to);
        self.generation += 1;
        true
    }

    /// Every favourite held (`sessions`) or listed (`lists`: each saved
    /// listing with the number it was asked under, [`crate::model::Named`]'s
    /// own per list): its snapshot refreshed,
    /// a provisional one keyed by its journal id once it has one, and seen,
    /// so not gone. A held one wins over its listings (it is the newer).
    /// `true` if its row or its ids changed (the rail rebuilds, the file is
    /// written); an update time alone is kept quietly.
    pub fn refresh(&mut self, sessions: &[SessionMeta], lists: &[(&[SavedSession], u64)]) -> bool {
        let mut change = Change::None;
        for f in &mut self.list {
            let live = sessions.iter().find(|s| s.lifecycle != "ending" && f.is(Source::Live(s)));
            let src = match live {
                Some(s) => Some((Source::Live(s), 0)),
                // The newest listing that has it (each says only what it
                // knew when it was asked).
                None => lists
                    .iter()
                    .filter_map(|(l, n)| l.iter().find(|v| f.is(Source::Saved(v))).map(|v| (Source::Saved(v), *n)))
                    .max_by_key(|(_, n)| *n),
            };
            if let Some((src, listed)) = src {
                change = change.max(f.take(src, listed));
            }
        }
        self.settle(change)
    }

    /// After sightings: a provisional favourite now keyed by a journal id
    /// another already has (favourited by its live id, and again from its
    /// saved row) is that one, the first kept. Nothing else is folded.
    /// `true` if a write is due.
    fn settle(&mut self, change: Change) -> bool {
        let merged = dedupe_by_id(&mut self.list);
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

/// Entries with one journal id folded into the first. `true` if any were.
fn dedupe_by_id(list: &mut Vec<Favourite>) -> bool {
    let n = list.len();
    let mut out: Vec<Favourite> = Vec::with_capacity(n);
    for f in list.drain(..) {
        match out.iter_mut().find(|g| g.id == f.id) {
            Some(g) => g.absorb(&f),
            None => out.push(f),
        }
    }
    *list = out;
    list.len() != n
}

/// Entries of one conversation (they share an id: one instance followed a
/// compaction the other's entry predates) folded into the first, as files
/// are read and merged. `true` if any were.
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

    /// A saved listing, asked for as number `n`.
    fn listed(v: &[SavedSession], n: u64) -> [(&[SavedSession], u64); 1] {
        [(v, n)]
    }

    /// The header of the attached session's conversation, as the daemon
    /// sends it (`SessionHeader`: `parent_session` set by compaction only).
    fn header(id: &str, parent: Option<&str>) -> Value {
        serde_json::json!({"id": id, "title": "t", "parent_session": parent})
    }

    fn ids(f: &Favourites) -> Vec<&str> {
        f.list.iter().map(|f| f.id.as_str()).collect()
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
        assert!(f.refresh(std::slice::from_ref(&s), &[]));
        assert_eq!((f.list[0].id.as_str(), f.list[0].provisional), ("j-relay", false));
        // Parked, then resumed under a new live id: the same favourite.
        let again = live("20261117-120000-bbbb", "j-relay", "Relay");
        assert!(f.contains(Source::Live(&again)));
        // Saved: the same journal.
        assert!(f.contains(Source::Saved(&saved("j-relay", "Relay", "/home/u/relay"))));
        // Its old live id now holding another journal is not it.
        assert!(!f.contains(Source::Live(&live("20261117-100000-aaaa", "j-other", "x"))));
        // Favourited by its live id, and again from its saved row: one (the
        // same journal id).
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("L", "", "x")), 1);
        f.list.push(Favourite::of(Source::Saved(&saved("J", "x", "/")), 2));
        f.refresh(&[live("L", "J", "x")], &[]);
        assert_eq!(f.list.len(), 1);
        assert_eq!(f.list[0].added_at, 1, "the first stays");
    }

    /// R1: a compaction seen here (the attached session's `Conversation`
    /// after `CompactionApplied`, its header naming the old journal as its
    /// parent: live-checked on 0.11.1) is followed, hop by hop; the
    /// favourite is one, held, parked or listed under the newest id.
    #[test]
    fn a_compaction_is_followed() {
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("J1", "J1", "Relay")), 1);
        let g = f.generation;
        // The poll may land before the header: not matched for that poll
        // (its journal is not one the favourite holds yet), never moved.
        assert!(!f.refresh(&[live("J1", "J2", "Relay")], &[]));
        assert_eq!(ids(&f), ["J1"]);
        assert!(f.continued(&header("J2", Some("J1"))), "the daemon says J2 continues J1");
        assert_eq!((f.list[0].id.as_str(), &f.list[0].aliases[..]), ("J2", &["J1".to_string()][..]));
        assert_eq!(f.generation, g + 1, "written");
        assert!(f.contains(Source::Live(&live("J1", "J2", "Relay"))));
        assert!(!f.continued(&header("J2", Some("J1"))), "every later turn's header: once");
        // Two: one hop each.
        assert!(f.continued(&header("J3", Some("J2"))));
        assert_eq!(f.list[0].id, "J3");
        // Parked: the same ids.
        let parked = SessionMeta { lifecycle: "parked".into(), ..live("J1", "J3", "Relay") };
        assert!(!f.refresh(std::slice::from_ref(&parked), &[]));
        // Evicted: listed under its chain head (the daemon folds J1, J2 into it).
        let head = [saved("J3", "Relay", "/home/u/relay")];
        assert!(f.contains(Source::Saved(&head[0])));
        f.refresh(&[], &listed(&head, 1));
        assert_eq!((f.list.len(), f.list[0].id.as_str()), (1, "J3"));
        // Resumed: the actor is rebuilt as the head.
        assert!(f.contains(Source::Live(&live("J3", "J3", "Relay"))));
        // A listing from before the compaction never moves it back.
        f.refresh(&[], &listed(&[saved("J2", "Relay", "/")], 2));
        assert_eq!(f.list[0].id, "J3");
    }

    /// R1: `/clear` (a new, empty conversation: its header has no parent)
    /// in the live session that holds a favourite: not followed. The
    /// favourite stays on its conversation (its saved row, its resume), and
    /// the cleared one, under the same live id, is not it.
    #[test]
    fn a_clear_is_not_followed() {
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("J1", "J1", "Relay")), 1);
        let g = f.generation;
        assert!(!f.continued(&header("Y", None)));
        let cleared = live("J1", "Y", "");
        assert!(!f.contains(Source::Live(&cleared)), "the live id says nothing");
        assert!(!f.refresh(std::slice::from_ref(&cleared), &[]));
        assert_eq!((ids(&f), f.list[0].aliases.len(), f.generation), (vec!["J1"], 0, g));
        assert!(f.contains(Source::Saved(&saved("J1", "Relay", "/"))), "its saved row is it");
        // `/resume J1` back in that session: it is held again.
        assert!(f.contains(Source::Live(&live("J1", "J1", "Relay"))));
        // After a compaction it followed: `/clear` keeps it on the successor.
        assert!(f.continued(&header("J2", Some("J1"))));
        assert!(!f.continued(&header("Y2", None)));
        assert!(!f.contains(Source::Live(&live("J1", "Y2", ""))));
        assert_eq!(ids(&f), ["J2"]);
    }

    /// R1: `/resume` of another conversation in the live session holding a
    /// favourite: not followed; if it is another favourite, the two stay
    /// two (each matched by its own journal).
    #[test]
    fn a_resume_onto_another_favourite_merges_nothing() {
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("A", "A", "Alpha")), 1);
        f.toggle(Source::Saved(&saved("B", "Bravo", "/")), 2);
        // `/resume B` in A's session: B's own header (no parent).
        assert!(!f.continued(&header("B", None)));
        let swapped = live("A", "B", "Bravo");
        assert!(f.refresh(std::slice::from_ref(&swapped), &[]), "B is held now (its row shows it)");
        assert_eq!(ids(&f), ["A", "B"]);
        assert_eq!(f.find(Source::Live(&swapped)), Some(1), "held: B's favourite, not A's");
        assert!(f.contains(Source::Saved(&saved("A", "Alpha", "/"))), "A's saved row is still A's");
        // B resumed is itself a compaction's successor (of B0, which A's
        // favourite never held): not A's.
        assert!(!f.continued(&header("B", Some("B0"))));
        // A successor of A that B's favourite already holds: the user's two
        // favourites stay two (no merge, no move).
        let mut f = Favourites::default();
        f.toggle(Source::Saved(&saved("A", "Alpha", "/")), 1);
        f.toggle(Source::Saved(&saved("A2", "Alpha, compacted", "/")), 2);
        assert!(!f.continued(&header("A2", Some("A"))));
        assert_eq!((ids(&f), f.list[0].aliases.len()), (vec!["A", "A2"], 0));
    }

    /// R1: a compaction of a held session not attached here (the TUI's) is
    /// not seen: the favourite and the new journal are two rows (it stays
    /// on its old id, which resumes into the chain). Attached, the header
    /// says so: one. A resume of the old id the daemon took down the chain
    /// is followed only as far as the header proves (one hop).
    #[test]
    fn an_unseen_compaction_is_two_rows_until_attached() {
        let mut f = Favourites::default();
        f.toggle(Source::Live(&live("J1", "J1", "Relay")), 1);
        let compacted = live("J1", "J2", "↳ Relay");
        assert!(!f.contains(Source::Live(&compacted)), "two rows: the favourite, and J2 in OPEN");
        assert!(!f.refresh(std::slice::from_ref(&compacted), &[]));
        assert_eq!(ids(&f), ["J1"]);
        // Attached (its row, or the favourite's resume): reconciled.
        assert!(f.continued(&header("J2", Some("J1"))));
        assert!(f.contains(Source::Live(&compacted)));
        // Two compactions unseen: the header names only the last hop: not
        // proven, not moved (two rows).
        let mut f = Favourites::default();
        f.toggle(Source::Saved(&saved("J1", "Relay", "/")), 1);
        assert!(!f.continued(&header("J3", Some("J2"))));
        assert_eq!(ids(&f), ["J1"]);
    }

    fn info(query: &str, opened: &str, via: Option<&str>) -> ContinueInfo {
        serde_json::from_value(serde_json::json!({"session_id": opened, "resolved_via": via, "query": query})).unwrap()
    }

    /// N2: two compactions unseen, then the favourite's resume, which the
    /// daemon followed down the chain (`continue_info`: J1 resolved via
    /// compaction to J3) to a session that holds J3: followed, however many
    /// hops. Not when the session holds another journal now (cleared since),
    /// nor for a name, a chain or the journal itself, nor from an older id
    /// the favourite was known by, nor onto another favourite's journal.
    #[test]
    fn a_resume_down_the_chain_is_followed() {
        let one = || {
            let mut f = Favourites::default();
            f.toggle(Source::Saved(&saved("J1", "Relay", "/")), 1);
            f
        };
        let mut f = one();
        let g = f.generation;
        assert!(!f.resolved(&info("J1", "J3", Some("compacted")), "Y"), "the session holds another journal");
        assert!(!f.resolved(&info("J1", "J3", Some("name")), "J3"));
        assert!(!f.resolved(&info("J1", "J1", None), "J1"));
        assert!(!f.resolved(&info("J0", "J3", Some("compacted")), "J3"), "not this favourite's");
        assert_eq!((ids(&f), f.generation), (vec!["J1"], g));
        assert!(f.resolved(&info("J1", "J3", Some("compacted")), "J3"));
        assert_eq!((ids(&f), &f.list[0].aliases[..], f.generation), (vec!["J3"], &["J1".to_string()][..], g + 1));
        assert!(f.contains(Source::Live(&live("J1", "J3", "Relay"))), "one row: held");
        // From an alias (J1 after it moved to J3): the favourite's journal
        // is J3; a resolve of J1 elsewhere moves nothing.
        assert!(!f.resolved(&info("J1", "J5", Some("compacted")), "J5"));
        assert_eq!(ids(&f), ["J3"]);
        // Onto another favourite's journal: both stay.
        let mut f = one();
        f.toggle(Source::Saved(&saved("J3", "Relay, later", "/")), 2);
        assert!(!f.resolved(&info("J1", "J3", Some("compacted")), "J3"));
        assert_eq!(ids(&f), ["J1", "J3"]);
        // The header's link is the favourite's journal only, too.
        let mut f = one();
        assert!(f.continued(&header("J2", Some("J1"))));
        assert!(!f.continued(&header("J9", Some("J1"))), "a successor of an older id: a branch");
        assert_eq!(ids(&f), ["J2"]);
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
        assert!(!f.refresh(std::slice::from_ref(&s), &[]));
        assert_eq!((f.generation, f.quiet), (g, true), "kept, not written");
        assert_eq!(f.list[0].updated_at.as_deref(), Some("2026-11-16T10:05:00Z"));
        s.title = Some("Relay, renamed".into());
        assert!(f.refresh(std::slice::from_ref(&s), &[]));
        assert_eq!(f.generation, g + 1);
    }

    /// The snapshot follows what the daemon last said: a held session (its
    /// title, folder, model) or its listing; one in neither keeps it.
    #[test]
    fn the_snapshot_follows_the_session() {
        let mut f = Favourites::default();
        let v = [saved("j1", "Old title", "/home/u/a")];
        f.toggle(Source::Saved(&v[0]), 5);
        assert_eq!((f.list[0].label.as_str(), f.list[0].cwd.as_deref()), ("Old title", Some("/home/u/a")));
        assert!(!f.refresh(&[], &listed(&v, 1)), "nothing new");
        let v2 = [saved("j1", "New title", "/home/u/b")];
        assert!(f.refresh(&[], &listed(&v2, 1)));
        assert_eq!((f.list[0].label.as_str(), f.list[0].cwd.as_deref()), ("New title", Some("/home/u/b")));
        let g = f.generation;
        assert!(!f.refresh(&[], &listed(&[], 1)), "out of the listing: kept as it was");
        assert_eq!((f.generation, f.list[0].label.as_str()), (g, "New title"));
    }

    /// M2: gone holds against a listing asked for before the verdict (the
    /// one cached, or one already on its way); a listing asked for after it,
    /// or the session held, says it is there.
    #[test]
    fn gone_holds_until_it_is_seen_again() {
        let mut f = Favourites::default();
        let v = [saved("j1", "Title", "/home/u/a")];
        f.toggle(Source::Saved(&v[0]), 5);
        // Two listings asked for so far; the resume failed.
        assert!(f.set_gone("j1", 2));
        assert!(!f.set_gone("j1", 2), "once");
        assert!(!f.refresh(&[], &listed(&v, 2)), "the cached listing (ask 2): still gone");
        assert!(f.list[0].is_gone());
        assert!(f.refresh(&[], &listed(&v, 3)), "asked for after: there");
        assert!(!f.list[0].is_gone());
        // R3: each list by its own number: a stale one that still has it
        // (the named list, asked before the verdict) says nothing, whatever
        // the other (newer, without it) says.
        f.set_gone("j1", 3);
        assert!(!f.refresh(&[], &[(&[], 5), (&v, 2)]), "named list from ask 2: still gone");
        assert!(f.list[0].is_gone());
        assert!(f.refresh(&[], &[(&v, 2), (&v, 4)]), "the newer list that has it says it is there");
        assert!(!f.list[0].is_gone());
        f.set_gone("j1", 4);
        assert!(f.refresh(&[live("L", "j1", "Title")], &[]), "held: there");
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
        assert!(f.continued(&header("J4", Some("J3"))));
        assert_eq!(f.list[2].aliases, ["J3"]);
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

    /// R5: a write's result taken even when the list changed while it ran:
    /// a favourite removed meanwhile stays removed at the next write (with
    /// the base the write left, it is this instance's remove, not another
    /// instance's add).
    #[test]
    fn a_remove_during_a_write_stays_removed() {
        let fav = |id: &str, at: i64| Favourite::of(Source::Saved(&saved(id, id, "/")), at);
        let mut f = Favourites::new(vec![fav("a", 1)]);
        f.list.push(fav("x", 2));
        let sent = f.list.clone();
        let file = merge(&[fav("a", 1)], &f.base, &sent);
        // x unfavourited while the write ran.
        f.remove("x");
        f.written(&sent, file.clone());
        assert_eq!(merge(&file, &f.base, &f.list).iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["a"]);
        // Its result dropped (the bug): x comes back as another's.
        let stale = Favourites::new(vec![fav("a", 1)]);
        assert_eq!(merge(&file, &stale.base, &f.list).len(), 2);
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
