//! Favourite sessions: a desktop-only bookmark list, per daemon, in the
//! order the user favourited them. Never a daemon pin: favouriting attaches,
//! pins, parks and keeps warm nothing (it costs no tokens).
//!
//! A favourite is keyed by the session's journal id (`SessionMeta.journal_id`
//! for a held session, `SavedSession.id` for a saved one): it survives a
//! park, a resume and a daemon restart; a live id does not. A held session
//! whose journal id is still empty is keyed by its live id until the journal
//! id appears ([`Favourites::refresh`]). Never by a row's position or label.
//!
//! Each keeps a snapshot (label, folder, model, last update): the saved
//! listing is newest first and capped, so an old favourite may not be in it,
//! and its row must still show and resume (by journal id, in its folder).

use synaps_client::SavedSession;
use synaps_client::protocol::SessionMeta;

/// One favourite, as saved under `favourites.<daemon key>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favourite {
    /// The journal id; a held session's live id while it has none yet
    /// (`provisional`).
    pub id: String,
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
    /// A resume found no journal for it: shown as gone until removed (or
    /// seen again). Never pruned silently.
    pub gone: bool,
}

/// What a favourite is made from: a held session or a saved one.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Live(&'a SessionMeta),
    Saved(&'a SavedSession),
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
            provisional,
            label: String::new(),
            named: false,
            cwd: None,
            model: None,
            updated_at: None,
            added_at: now,
            gone: false,
        };
        f.take(src);
        f
    }

    /// `src` is this favourite.
    pub fn is(&self, src: Source<'_>) -> bool {
        match src {
            Source::Live(s) => s.is(&self.id),
            Source::Saved(v) => v.id == self.id,
        }
    }

    /// The snapshot from `src` (and the journal id, once a held session
    /// has one). `true` if anything changed.
    fn take(&mut self, src: Source<'_>) -> bool {
        let before = self.clone();
        let text = |s: &Option<String>| s.as_deref().filter(|t| !t.is_empty()).map(str::to_string);
        let (name, title, cwd, model, updated) = match src {
            Source::Live(s) => {
                if self.provisional && !s.journal_id.is_empty() {
                    self.id = s.journal_id.clone();
                    self.provisional = false;
                }
                // A held session has no update time of its own: its state's.
                let updated = s.state_since.clone().or_else(|| self.updated_at.clone());
                (text(&s.name), text(&s.title), text(&s.cwd), Some(s.model.clone()).filter(|m| !m.is_empty()), updated)
            }
            Source::Saved(v) => {
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
        self.gone = false;
        *self != before
    }
}

/// The favourites of the daemon on screen, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Favourites {
    pub list: Vec<Favourite>,
    /// Bumped on every change (the save is debounced on it).
    pub generation: u64,
}

impl Favourites {
    pub fn new(list: Vec<Favourite>) -> Self {
        Favourites { list, generation: 0 }
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

    /// Unfavourite the one keyed `id` (a gone row's Remove).
    pub fn remove(&mut self, id: &str) -> bool {
        let n = self.list.len();
        self.list.retain(|f| f.id != id);
        let removed = self.list.len() != n;
        if removed {
            self.generation += 1;
        }
        removed
    }

    /// A resume of `id` found no journal: its row says gone.
    pub fn set_gone(&mut self, id: &str) -> bool {
        match self.list.iter_mut().find(|f| f.id == id && !f.gone) {
            Some(f) => {
                f.gone = true;
                self.generation += 1;
                true
            }
            None => false,
        }
    }

    /// Every favourite held (`sessions`) or listed (`saved`): its snapshot
    /// refreshed, its live id upgraded to its journal id once it has one,
    /// and seen, so not gone. A held one wins over its listing (it is the
    /// newer). `true` if anything changed (the rail rebuilds, the file is
    /// written).
    pub fn refresh<'a>(&mut self, sessions: &[SessionMeta], saved: impl Iterator<Item = &'a SavedSession> + Clone) -> bool {
        let mut changed = false;
        for f in &mut self.list {
            let live = sessions.iter().find(|s| s.lifecycle != "ending" && f.is(Source::Live(s)));
            let src = match live {
                Some(s) => Some(Source::Live(s)),
                None => saved.clone().find(|v| f.is(Source::Saved(v))).map(Source::Saved),
            };
            if let Some(src) = src {
                changed |= f.take(src);
            }
        }
        // An upgrade can make two of one journal (favourited by live id,
        // then again from its saved row): the first stays.
        let mut seen = std::collections::HashSet::new();
        let n = self.list.len();
        self.list.retain(|f| seen.insert(f.id.clone()));
        changed |= self.list.len() != n;
        if changed {
            self.generation += 1;
        }
        changed
    }
}

/// A resume's failure says the journal is not there (deleted, or never
/// on this daemon): not a lock, a load error or a name the resolver missed.
pub fn journal_missing(reason: &str) -> bool {
    let r = reason.to_lowercase();
    ["no session matching", "no sessions to continue", "not found", "no such file", "does not exist", "unknown session"]
        .iter()
        .any(|p| r.contains(p))
}

/// The settings file's form: `favourites.<daemon key>` is a list of these.
pub fn to_json(list: &[Favourite]) -> serde_json::Value {
    serde_json::Value::Array(
        list.iter()
            .map(|f| {
                let mut o = serde_json::json!({
                    "id": f.id,
                    "label": f.label,
                    "cwd": f.cwd,
                    "model": f.model,
                    "updated_at": f.updated_at,
                    "added_at": f.added_at,
                });
                if f.provisional {
                    o["provisional"] = true.into();
                }
                if f.named {
                    o["named"] = true.into();
                }
                if f.gone {
                    o["gone"] = true.into();
                }
                o
            })
            .collect(),
    )
}

/// [`to_json`]'s list read back: an entry without an id is skipped (a
/// hand edit), the rest kept in order, each id once.
pub fn from_json(v: &serde_json::Value) -> Vec<Favourite> {
    let Some(a) = v.as_array() else { return Vec::new() };
    let s = |e: &serde_json::Value, k: &str| e.get(k).and_then(serde_json::Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    let b = |e: &serde_json::Value, k: &str| e.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
    let mut seen = std::collections::HashSet::new();
    a.iter()
        .filter_map(|e| {
            let id = s(e, "id")?;
            seen.insert(id.clone()).then(|| Favourite {
                label: s(e, "label").unwrap_or_else(|| id.clone()),
                id,
                provisional: b(e, "provisional"),
                named: b(e, "named"),
                cwd: s(e, "cwd"),
                model: s(e, "model"),
                updated_at: s(e, "updated_at"),
                added_at: e.get("added_at").and_then(serde_json::Value::as_i64).unwrap_or(0),
                gone: b(e, "gone"),
            })
        })
        .collect()
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
        assert!(f.refresh(std::slice::from_ref(&s), std::iter::empty()));
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
        f.refresh(&[live("L", "J", "x")], std::iter::empty());
        assert_eq!(f.list.len(), 1);
        assert_eq!(f.list[0].added_at, 1, "the first stays");
    }

    /// The snapshot follows what the daemon last said: a held session (its
    /// title, folder, model) or its listing; one in neither keeps it.
    #[test]
    fn the_snapshot_follows_the_session() {
        let mut f = Favourites::default();
        let v = saved("j1", "Old title", "/home/u/a");
        f.toggle(Source::Saved(&v), 5);
        assert_eq!((f.list[0].label.as_str(), f.list[0].cwd.as_deref()), ("Old title", Some("/home/u/a")));
        assert!(!f.refresh(&[], std::iter::once(&v)), "nothing new");
        let v2 = saved("j1", "New title", "/home/u/b");
        assert!(f.refresh(&[], std::iter::once(&v2)));
        assert_eq!((f.list[0].label.as_str(), f.list[0].cwd.as_deref()), ("New title", Some("/home/u/b")));
        let g = f.generation;
        assert!(!f.refresh(&[], std::iter::empty()), "out of the listing: kept as it was");
        assert_eq!((f.generation, f.list[0].label.as_str()), (g, "New title"));
        // Seen again: no longer gone.
        f.set_gone("j1");
        assert!(f.list[0].gone);
        assert!(f.refresh(&[], std::iter::once(&v2)));
        assert!(!f.list[0].gone);
    }

    /// Brief 2: the file's form round-trips (order, the flags), and a hand
    /// edit without an id, or an id twice, is dropped.
    #[test]
    fn the_list_round_trips() {
        let mut f = Favourites::default();
        f.toggle(Source::Saved(&saved("j2", "Two", "/b")), 2);
        f.toggle(Source::Live(&live("L1", "", "One")), 1);
        f.set_gone("j2");
        let back = from_json(&to_json(&f.list));
        assert_eq!(back, f.list);
        let edited = serde_json::json!([{"label": "no id"}, {"id": "a"}, {"id": "a", "label": "twice"}, "junk"]);
        let back = from_json(&edited);
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].id.as_str(), back[0].label.as_str()), ("a", "a"));
        assert!(from_json(&serde_json::json!({"not": "a list"})).is_empty());
    }

    #[test]
    fn a_missing_journal_reads_as_gone() {
        assert!(journal_missing("create session: no session matching 'j-old'"));
        assert!(journal_missing("Failed to load session 'x': No such file or directory (os error 2)"));
        assert!(!journal_missing("'x' is live in another process (pid 4)"));
        assert!(!journal_missing("connection reset"));
    }
}
