//! The sessions rail without GPUI: each row's state, the groups
//! (FAVOURITES, PINNED, OPEN, HISTORY), the history buckets, find and the
//! count chips, and the hover readout's lines. Built when the lists change,
//! never per frame; a frame reads the result (`chrome.rs`).
//!
//! A favourite (the desktop's own bookmark, [`crate::favourites`]) shows in
//! FAVOURITES only, in the order it was favourited; otherwise where the
//! daemon holds a session decides its group. What it is doing decides its
//! glyph, first match wins (see [`State`]). A row never moves because its
//! state changed: OPEN is ordered by `created_at`.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use synaps_client::SavedSession;
use synaps_client::protocol::SessionMeta;

use crate::favourites::{Favourite, Source};
use crate::fonts::label;
use crate::model::Model;
use crate::named::{ResumeRequest, RowAction, home_short, parse_rfc3339, relative, short_model};

/// What a row is doing. Precedence, first match wins: OPENING › NEEDS YOU ›
/// RUNNING › WORKERS › LOCKED › UNREAD › IDLE › PARKED › SAVED.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// A resume or an attach is on its way (the client's own).
    Opening,
    /// `awaiting_input > 0`: a question, a secret or a tool confirmation.
    NeedsYou,
    /// `busy`: a turn or a compaction.
    Running,
    /// Not busy, background workers (subagents) still running: never
    /// IDLE ([`crate::model::Model::workers`]).
    Workers,
    /// `locked_by`: another process holds the journal.
    Locked,
    /// The poll saw `busy` fall or `messages` rise while it was not on
    /// screen (the client's own; cleared when it is opened).
    Unread,
    /// `lifecycle = live`, at rest.
    Idle,
    /// `lifecycle = parking | parked`: held, unloaded.
    Parked,
    /// On disk only (`SavedSession.live = false`).
    Saved,
}

impl State {
    /// The word line 2 shows: only for a state not at rest.
    pub fn word(self) -> Option<&'static str> {
        match self {
            State::Opening => Some(label::OPENING),
            State::NeedsYou => Some(label::NEEDS_YOU),
            State::Running => Some(label::RUNNING),
            State::Workers => Some(label::WORKERS),
            State::Locked => Some(label::LOCKED),
            State::Unread => Some(label::UNREAD),
            State::Idle | State::Parked | State::Saved => None,
        }
    }

    /// For the readout.
    pub fn name(self) -> &'static str {
        match self {
            State::Opening => "opening",
            State::NeedsYou => "needs you",
            State::Running => "running",
            State::Workers => "workers running",
            State::Locked => "locked",
            State::Unread => "unread",
            State::Idle => "idle",
            State::Parked => "parked",
            State::Saved => "saved",
        }
    }
}

/// A lifecycle the daemon holds unloaded.
pub fn is_parked(s: &SessionMeta) -> bool {
    matches!(s.lifecycle.as_str(), "parking" | "parked")
}

/// The count chips: one at a time filters every group to that state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Chip {
    NeedsYou,
    Running,
    Unread,
}

impl Chip {
    /// RUNNING admits a session whose workers run on after its turn.
    pub fn admits(self, s: State) -> bool {
        matches!(
            (self, s),
            (Chip::NeedsYou, State::NeedsYou) | (Chip::Running, State::Running | State::Workers) | (Chip::Unread, State::Unread)
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Chip::NeedsYou => label::NEEDS_YOU,
            Chip::Running => label::RUNNING,
            Chip::Unread => label::UNREAD,
        }
    }
}

/// HISTORY's buckets, by `updated_at` (rolling: the last 24 hours, 7 days,
/// 30 days), named sessions first whatever their age.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bucket {
    Named,
    Today,
    Week,
    Month,
    Older,
}

impl Bucket {
    pub const ALL: [Bucket; 5] = [Bucket::Named, Bucket::Today, Bucket::Week, Bucket::Month, Bucket::Older];
    /// Open until the user closes them.
    pub const DEFAULT_OPEN: u8 = Bucket::Named.bit() | Bucket::Today.bit();

    pub const fn bit(self) -> u8 {
        1 << self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Bucket::Named => label::NAMED,
            Bucket::Today => label::TODAY,
            Bucket::Week => label::THIS_WEEK,
            Bucket::Month => label::THIS_MONTH,
            Bucket::Older => label::OLDER,
        }
    }

    /// The settings file's name for it.
    pub fn key(self) -> &'static str {
        match self {
            Bucket::Named => "named",
            Bucket::Today => "today",
            Bucket::Week => "week",
            Bucket::Month => "month",
            Bucket::Older => "older",
        }
    }

    /// Oldest age in seconds that still falls in it (`None`: no bound).
    fn max_age(self) -> Option<i64> {
        match self {
            Bucket::Named | Bucket::Older => None,
            Bucket::Today => Some(86_400),
            Bucket::Week => Some(7 * 86_400),
            Bucket::Month => Some(30 * 86_400),
        }
    }

    /// An unnamed saved session's bucket at `now` (no time: OLDER).
    pub fn of_age(updated: Option<i64>, now: i64) -> Bucket {
        let Some(t) = updated else { return Bucket::Older };
        let age = (now - t).max(0);
        [Bucket::Today, Bucket::Week, Bucket::Month].into_iter().find(|b| b.max_age().is_some_and(|m| age < m)).unwrap_or(Bucket::Older)
    }
}

/// Rows an open bucket shows before `+ N MORE`.
pub const BUCKET_ROWS: usize = 8;

/// Which group a header heads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Pinned,
    Open,
    History,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Pinned => label::PINNED,
            Group::Open => label::OPEN_GROUP,
            Group::History => label::HISTORY,
        }
    }

    pub fn jp(self) -> &'static str {
        match self {
            Group::Pinned => crate::fonts::JP_PINNED,
            Group::Open => crate::fonts::JP_OPEN,
            Group::History => crate::fonts::JP_HISTORY,
        }
    }
}

/// Where a saved row's entry is in the model (valid until the lists change,
/// which rebuilds the rail).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedRef {
    Named(usize),
    Recent(usize),
}

/// A saved row's entry in the model.
pub fn saved_entry(m: &Model, r: SavedRef) -> Option<&SavedSession> {
    match r {
        SavedRef::Named(i) => m.named.saved.get(i),
        SavedRef::Recent(i) => m.named.recent.get(i),
    }
}

/// One row's strings, built with the rail.
#[derive(Clone, Debug)]
pub struct Row {
    /// A hash of the session or journal id: its element id, and how the
    /// list knows it is the same row after a rebuild.
    pub key: u64,
    /// Index in [`Model::sessions`] of a held session.
    pub live: Option<usize>,
    pub saved: Option<SavedRef>,
    pub title: Arc<str>,
    /// A named session: the title is its handle, set in mono.
    pub named: bool,
    /// Line 2 (PINNED, OPEN): the folder, or a named session's title.
    pub sub: Arc<str>,
    pub sub_mono: bool,
    pub age: Arc<str>,
    /// Idle and no messages yet: dimmed like a resting row.
    pub fresh: bool,
    /// A saved row's state (a held row's is read per frame).
    pub state: State,
    pub action: RowAction,
    /// A favourite's row: its index in [`Model::favs`].
    pub fav: Option<usize>,
    /// A favourite whose journal a resume did not find: GONE, with Remove.
    pub gone: bool,
    /// The daemon pins it (a favourite's row shows the pin mark: it is not
    /// in PINNED).
    pub pinned: bool,
}

/// One line of the rail's list.
#[derive(Clone, Debug)]
pub enum Item {
    /// FAVOURITES' head: it folds, as a bucket does.
    Favs {
        open: bool,
        count: Arc<str>,
    },
    Group {
        group: Group,
        count: Arc<str>,
    },
    /// A two-line row (PINNED, OPEN): index into [`Rail::rows`].
    Row(usize),
    Bucket {
        bucket: Bucket,
        open: bool,
        count: Arc<str>,
    },
    /// A one-line HISTORY row.
    Hist(usize),
    More {
        bucket: Bucket,
        text: Arc<str>,
    },
    Note(Arc<str>),
    Empty(&'static str),
}

/// What the user has set on the rail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The find text, trimmed and lowercased ("" shows everything).
    pub find: String,
    pub chip: Option<Chip>,
    /// [`Bucket::bit`]s of the open buckets.
    pub open: u8,
    /// Buckets showing all their rows (`+ N MORE` was clicked).
    pub expanded: u8,
    /// FAVOURITES folded to its head.
    pub favs_closed: bool,
}

/// The rail, built.
#[derive(Clone, Debug, Default)]
pub struct Rail {
    pub rows: Vec<Row>,
    pub items: Vec<Item>,
    /// HISTORY rows that are unread (the UNREAD chip adds the held ones per frame).
    pub history_unread: usize,
}

/// How history stands for the rail.
enum History {
    /// Not connected: nothing to say.
    Off,
    /// The daemon does not list saved sessions.
    Unsupported,
    /// Asked, nothing back yet.
    Loading,
    /// Asked, failed, nothing listed since.
    Failed,
    Listed,
}

pub fn key_of(id: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

fn arc(s: impl Into<Arc<str>>) -> Arc<str> {
    s.into()
}

/// `1,046`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Find matches what a row shows (its title, an untitled row's id, the
/// folder as `~/…`) and the name, title and folder as listed.
fn found(find: &str, fields: [Option<&str>; 4]) -> bool {
    if find.is_empty() {
        return true;
    }
    let cwd = fields[2].filter(|c| !c.is_empty()).map(home_short);
    fields.into_iter().flatten().chain(cwd.as_deref()).any(|f| f.to_lowercase().contains(find))
}

/// The id, when it is what the row shows (nothing else to call it by).
fn shown_id<'a>(id: &'a str, name: Option<&str>, title: Option<&str>) -> Option<&'a str> {
    (named_str(name).is_none() && title.is_none_or(str::is_empty)).then_some(id)
}

fn named_str(n: Option<&str>) -> Option<&str> {
    n.filter(|n| !n.is_empty())
}

fn named(n: &Option<String>) -> Option<&str> {
    named_str(n.as_deref())
}

/// A held session's row.
fn live_row(m: &Model, i: usize, now: i64) -> Row {
    let s = &m.sessions[i];
    let name = named(&s.name);
    let title_text = s.title.as_deref().filter(|t| !t.is_empty());
    let cwd = s.cwd.as_deref().filter(|c| !c.is_empty()).map(home_short);
    let title = match (name, title_text) {
        (Some(n), _) => n.to_string(),
        (None, Some(t)) => t.to_string(),
        (None, None) if s.messages == 0 => "New session".into(),
        (None, None) => s.id.clone(),
    };
    // A named session's line 2 is its title (the handle is line 1).
    let (sub, sub_mono) = match (name, title_text) {
        (Some(_), Some(t)) => (t.to_string(), false),
        _ => match cwd {
            Some(c) => (c, true),
            None => (short_model(&s.model), true),
        },
    };
    let since = s.state_since.as_deref().or(Some(s.created_at.as_str()).filter(|c| !c.is_empty()));
    let fav = m.fav_of(s);
    Row {
        key: key_of(&s.id),
        live: Some(i),
        saved: None,
        title: arc(title),
        named: name.is_some(),
        sub: arc(sub),
        sub_mono,
        age: arc(since.map(|t| relative(t, now)).unwrap_or_default()),
        fresh: s.messages == 0 && name.is_none() && title_text.is_none(),
        state: State::Idle,
        action: RowAction::Attach(s.id.clone()),
        fav,
        gone: false,
        // Only a favourite's row says so: PINNED itself does.
        pinned: s.pinned && fav.is_some(),
    }
}

/// A favourite nothing holds: its listing's entry if the lists have it,
/// else its snapshot (an old one the newest-first listing no longer
/// reaches still shows, and resumes by its journal id in its folder).
fn fav_row(m: &Model, ix: usize, f: &Favourite, entry: Option<SavedRef>, now: i64) -> Row {
    let cwd = f.cwd.as_deref().filter(|c| !c.is_empty());
    let (sub, sub_mono) = match cwd {
        Some(c) => (home_short(c), true),
        None => (f.model.as_deref().map(short_model).unwrap_or_default(), true),
    };
    Row {
        key: key_of(&f.id),
        live: None,
        saved: entry,
        title: arc(f.label.as_str()),
        named: f.named,
        sub: arc(sub),
        sub_mono,
        age: arc(f.updated_at.as_deref().map(|t| relative(t, now)).unwrap_or_default()),
        fresh: false,
        state: m.fav_state(f),
        action: RowAction::Resume(ResumeRequest {
            label: f.label.clone(),
            target: f.id.clone(),
            cwd: cwd.map(str::to_string),
            by_name: false,
        }),
        fav: Some(ix),
        gone: f.is_gone(),
        pinned: false,
    }
}

/// A saved session's row.
fn saved_row(m: &Model, r: SavedRef, v: &SavedSession, now: i64) -> Row {
    let name = named(&v.name);
    let title = name.or(v.title.as_deref().filter(|t| !t.is_empty())).unwrap_or(&v.id);
    Row {
        key: key_of(&v.id),
        live: None,
        saved: Some(r),
        title: arc(title),
        named: name.is_some(),
        sub: arc(""),
        sub_mono: false,
        age: arc(v.updated_at.as_deref().or(v.created_at.as_deref()).map(|t| relative(t, now)).unwrap_or_default()),
        fresh: false,
        state: m.saved_state(v),
        action: RowAction::Resume(ResumeRequest {
            label: name.unwrap_or(title).to_string(),
            target: v.id.clone(),
            cwd: v.cwd.clone().filter(|c| !c.is_empty()),
            by_name: false,
        }),
        fav: None,
        gone: false,
        pinned: false,
    }
}

/// The saved sessions HISTORY lists: the named list and the newest ones,
/// deduped by id, less what the daemon holds (by id, journal id or name:
/// `live` is as of the listing, the session list is newer), newest first.
pub fn history(m: &Model) -> Vec<(SavedRef, &SavedSession, Option<i64>)> {
    use std::collections::HashSet;
    let n = &m.named;
    let held: HashSet<&str> = m.sessions.iter().flat_map(|s| [s.id.as_str(), s.journal_id.as_str()]).filter(|k| !k.is_empty()).collect();
    let held_names: HashSet<&str> = m.sessions.iter().filter_map(|s| named(&s.name)).collect();
    let mut seen = HashSet::new();
    // The newest list first: it is the fresher of the two (the named list
    // is asked again only when needed).
    let mut out: Vec<(SavedRef, &SavedSession, Option<i64>)> = n
        .recent
        .iter()
        .enumerate()
        .map(|(i, v)| (SavedRef::Recent(i), v))
        .chain(n.saved.iter().enumerate().map(|(i, v)| (SavedRef::Named(i), v)))
        .filter(|(_, v)| !v.live && seen.insert(v.id.as_str()))
        .filter(|(_, v)| !held.contains(v.id.as_str()) && named(&v.name).is_none_or(|nm| !held_names.contains(nm)))
        .map(|(r, v)| (r, v, v.updated_at.as_deref().or(v.created_at.as_deref()).and_then(parse_rfc3339)))
        .collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.1.id.cmp(&a.1.id)));
    out
}

/// Build the rail. `now`: Unix seconds (ages, buckets).
pub fn build(m: &Model, view: &View, now: i64) -> Rail {
    let mut rail = Rail::default();
    let filtering = !view.find.is_empty() || view.chip.is_some();

    // FAVOURITES in the order they were favourited, each where the daemon
    // has it now: held (its live row), else listed or remembered.
    let held = |s: &SessionMeta| s.lifecycle != "ending";
    let favs = fav_items(m, view, now, &mut rail);
    // PINNED by name, OPEN by created_at (the model keeps the list in that
    // order); a session that is ending is gone; a favourite is in
    // FAVOURITES only.
    let other = |i: &usize| held(&m.sessions[*i]) && !favs.contains(i);
    let mut pinned: Vec<usize> = (0..m.sessions.len()).filter(|&i| m.sessions[i].pinned).filter(other).collect();
    pinned.sort_by_cached_key(|&i| (m.sessions[i].label().to_lowercase(), m.sessions[i].id.clone()));
    let open: Vec<usize> = (0..m.sessions.len()).filter(|&i| !m.sessions[i].pinned).filter(other).collect();
    for (group, ids) in [(Group::Pinned, pinned), (Group::Open, open)] {
        let shown: Vec<usize> = ids
            .into_iter()
            .filter(|&i| {
                let s = &m.sessions[i];
                view.chip.is_none_or(|c| m.chip_shows(c, s))
                    && found(
                        &view.find,
                        [s.name.as_deref(), s.title.as_deref(), s.cwd.as_deref(), shown_id(&s.id, s.name.as_deref(), s.title.as_deref())],
                    )
            })
            .collect();
        if shown.is_empty() && (group == Group::Pinned || filtering) {
            continue;
        }
        rail.items.push(Item::Group { group, count: arc(shown.len().to_string()) });
        if shown.is_empty() {
            let empty = if m.sessions.is_empty() && !matches!(m.conn, crate::model::ConnState::Connected(_)) {
                "Not connected"
            } else {
                "No open sessions"
            };
            rail.items.push(Item::Empty(empty));
        }
        for i in shown {
            rail.rows.push(live_row(m, i, now));
            rail.items.push(Item::Row(rail.rows.len() - 1));
        }
    }

    let n = &m.named;
    // The last lists stay while the connection is re-made.
    let state = match &m.conn {
        _ if n.saved_supported && n.recent_loaded => History::Listed,
        _ if n.saved_supported && n.saved_failed => History::Failed,
        _ if n.saved_supported => History::Loading,
        crate::model::ConnState::Connected(_) => History::Unsupported,
        _ => History::Off,
    };
    match state {
        History::Off => {}
        History::Unsupported if !filtering => {
            rail.items.push(Item::Group { group: Group::History, count: arc("—") });
            rail.items.push(Item::Note(arc(
                "This daemon does not list saved sessions, so there is no history here. Type a session's exact name in find and press \
                 Enter to resume it.",
            )));
        }
        History::Unsupported => {}
        History::Loading => {
            rail.items.push(Item::Group { group: Group::History, count: arc("…") });
            if filtering {
                rail.items.push(Item::Note(arc("Listing saved sessions…")));
            }
        }
        History::Failed => {
            rail.items.push(Item::Group { group: Group::History, count: arc("—") });
            rail.items.push(Item::Note(arc("Saved sessions could not be listed (why is above). Find asks again when it gets focus.")));
        }
        History::Listed => history_items(m, view, now, &mut rail),
    }
    if filtering && rail.items.is_empty() {
        rail.items.push(Item::Empty("Nothing matches"));
    }
    rail
}

/// FAVOURITES: hidden when there are none (or, filtering, none match);
/// folded to its head when closed (not while filtering, as a bucket), but
/// a favourite that needs you or is running stays under it: it is in no
/// other group, and the rail is where those are seen. Returns the held
/// sessions it has (shown or not): no other group lists them.
fn fav_items(m: &Model, view: &View, now: i64, rail: &mut Rail) -> Vec<usize> {
    let mut held = Vec::new();
    if m.favs.is_empty() {
        return held;
    }
    let filtering = !view.find.is_empty() || view.chip.is_some();
    let n = &m.named;
    // Each shown row, and whether it stays when folded.
    let mut rows = Vec::new();
    for (ix, f) in m.favs.list.iter().enumerate() {
        let live = m.sessions.iter().position(|s| s.lifecycle != "ending" && f.is(Source::Live(s)));
        let row = match live {
            Some(i) => {
                held.push(i);
                let s = &m.sessions[i];
                let shows = view.chip.is_none_or(|c| m.chip_shows(c, s))
                    && found(
                        &view.find,
                        [s.name.as_deref(), s.title.as_deref(), s.cwd.as_deref(), shown_id(&s.id, s.name.as_deref(), s.title.as_deref())],
                    );
                shows.then(|| (live_row(m, i, now), busy(m, s)))
            }
            None => {
                let entry = n
                    .recent
                    .iter()
                    .position(|v| f.is(Source::Saved(v)))
                    .map(SavedRef::Recent)
                    .or_else(|| n.saved.iter().position(|v| f.is(Source::Saved(v))).map(SavedRef::Named));
                let st = m.fav_state(f);
                if st == State::Unread {
                    rail.history_unread += 1;
                }
                let name = f.named.then_some(f.label.as_str());
                let shows = view.chip.is_none_or(|c| c.admits(st))
                    && found(&view.find, [name, Some(f.label.as_str()), f.cwd.as_deref(), (f.label == f.id).then_some(f.id.as_str())]);
                shows.then(|| (fav_row(m, ix, f, entry, now), false))
            }
        };
        rows.extend(row);
    }
    if rows.is_empty() && filtering {
        return held;
    }
    let open = filtering || !view.favs_closed;
    rail.items.push(Item::Favs { open, count: arc(rows.len().to_string()) });
    for (row, busy) in rows {
        if open || busy {
            rail.rows.push(row);
            rail.items.push(Item::Row(rail.rows.len() - 1));
        }
    }
    held
}

/// A held session needs you or runs (a folded FAVOURITES still shows it).
pub fn busy(m: &Model, s: &SessionMeta) -> bool {
    [Chip::NeedsYou, Chip::Running].into_iter().any(|c| m.chip_shows(c, s))
}

fn history_items(m: &Model, view: &View, now: i64, rail: &mut Rail) {
    let n = &m.named;
    let filtering = !view.find.is_empty() || view.chip.is_some();
    // A favourite is in FAVOURITES only.
    let all: Vec<_> = history(m).into_iter().filter(|(_, v, _)| !m.favs.contains(Source::Saved(v))).collect();
    // The oldest time listed: buckets wholly newer than it are complete.
    let horizon = n.recent_truncated.then(|| all.iter().filter(|e| named(&e.1.name).is_none()).filter_map(|e| e.2).min()).flatten();
    let mut buckets: [Vec<(SavedRef, &SavedSession)>; 5] = Default::default();
    for (r, v, t) in &all {
        let b = if named(&v.name).is_some() { Bucket::Named } else { Bucket::of_age(*t, now) };
        buckets[b as usize].push((*r, v));
    }
    let complete = |b: Bucket| match b {
        Bucket::Named => !n.saved_truncated,
        _ if !n.recent_truncated => true,
        _ => b.max_age().is_some_and(|max| horizon.is_some_and(|h| h < now - max)),
    };
    let mut items = Vec::new();
    let mut total = 0;
    for b in Bucket::ALL {
        let rows: Vec<&(SavedRef, &SavedSession)> = buckets[b as usize]
            .iter()
            .filter(|(_, v)| {
                view.chip.is_none_or(|c| m.chip_shows_saved(c, v))
                    && found(
                        &view.find,
                        [v.name.as_deref(), v.title.as_deref(), v.cwd.as_deref(), shown_id(&v.id, v.name.as_deref(), v.title.as_deref())],
                    )
            })
            .collect();
        total += rows.len();
        rail.history_unread += buckets[b as usize].iter().filter(|(_, v)| m.saved_fact(v) == State::Unread).count();
        if rows.is_empty() && (filtering || complete(b)) {
            continue;
        }
        let plus = if complete(b) { "" } else { "+" };
        let open = filtering || view.open & b.bit() != 0;
        items.push(Item::Bucket { bucket: b, open, count: arc(format!("{}{plus}", thousands(rows.len()))) });
        if !open {
            continue;
        }
        let cap = if view.expanded & b.bit() != 0 { usize::MAX } else { BUCKET_ROWS };
        for (r, v) in rows.iter().take(cap) {
            rail.rows.push(saved_row(m, *r, v, now));
            items.push(Item::Hist(rail.rows.len() - 1));
        }
        if rows.len() > cap {
            items.push(Item::More { bucket: b, text: arc(format!("+ {}   {}", thousands(rows.len() - cap), label::MORE)) });
        }
        if b == Bucket::Older && n.recent_truncated && n.deep && !filtering {
            items.push(Item::Note(arc(format!(
                "The newest {} are listed. Anything older needs a daemon search.",
                thousands(synaps_client::SAVED_SESSIONS_MAX as usize)
            ))));
        }
    }
    // A search over a list known to be cut, or still coming, says what it
    // searched: a miss is not "nothing matches".
    let searched = if !filtering {
        None
    } else if m.saved_listing() {
        Some(format!("Listing saved sessions… searched the newest {} so far.", thousands(n.recent.len())))
    } else if n.recent_truncated {
        Some(format!("Searched the newest {} saved sessions. Anything older needs a daemon search.", thousands(n.recent.len())))
    } else {
        None
    };
    if let Some(t) = searched {
        items.push(Item::Note(arc(t)));
    } else if filtering && items.is_empty() {
        return;
    }
    let plus = if n.recent_truncated { "+" } else { "" };
    rail.items.push(Item::Group { group: Group::History, count: arc(format!("{}{plus}", thousands(total))) });
    if items.is_empty() {
        rail.items.push(Item::Empty("No saved sessions"));
    }
    rail.items.extend(items);
}

/// Each item's identity, for a list splice: the same key keeps its place
/// and its measured height.
pub fn item_key(rail: &Rail, item: &Item) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(item).hash(&mut h);
    match item {
        Item::Favs { open, count } => (open, count).hash(&mut h),
        Item::Group { group, count } => (group, count).hash(&mut h),
        Item::Row(i) | Item::Hist(i) => rail.rows[*i].key.hash(&mut h),
        Item::Bucket { bucket, open, count } => (bucket, open, count).hash(&mut h),
        Item::More { bucket, text } => (bucket, text).hash(&mut h),
        Item::Note(t) => t.hash(&mut h),
        Item::Empty(t) => t.hash(&mut h),
    }
    h.finish()
}

/// The splice that turns `old` into `new`: the changed middle between the
/// longest common prefix and suffix, as `(old range, new count)`. `None`:
/// nothing changed.
pub fn splice(old: &[u64], new: &[u64]) -> Option<(std::ops::Range<usize>, usize)> {
    let pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if pre == old.len() && pre == new.len() {
        return None;
    }
    let suf = old[pre..].iter().rev().zip(new[pre..].iter().rev()).take_while(|(a, b)| a == b).count();
    Some((pre..old.len() - suf, new.len() - suf - pre))
}

/// `<1m`, `2m`, `3h`, `2d` until `ts` (`now` if it has passed).
pub fn until(ts: &str, now: i64) -> Option<String> {
    let d = (parse_rfc3339(ts)? - now).max(0);
    Some(match d {
        // Whole minutes: the readout is drawn on pointer motion, so seconds
        // would sit frozen while the pointer rests.
        0..60 => "<1m".to_string(),
        60..3600 => format!("{}m", d / 60),
        3600..86_400 => format!("{}h", d / 3600),
        _ => format!("{}d", d / 86_400),
    })
}

/// The hover readout: the full title, then `(key, value, aside)` lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Readout {
    pub title: String,
    pub lines: Vec<(&'static str, String, String)>,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A held session's readout.
pub fn readout_live(m: &Model, s: &SessionMeta, now: i64) -> Readout {
    let st = m.state(s);
    let mut lines = Vec::new();
    let age = s.state_since.as_deref().map(|t| relative(t, now)).unwrap_or_default();
    let (value, aside) = match st {
        State::Opening => ("opening".to_string(), "attach sent, waiting for the daemon".to_string()),
        State::NeedsYou => {
            let n = if m.is_active(&s.id) { m.transcript.pending_prompts().len() } else { s.awaiting_input };
            let turn = if age.is_empty() { String::new() } else { format!(" · turn began {age} ago") };
            ("needs you".into(), format!("{}{turn}", plural(n.max(1), "prompt", "prompts")))
        }
        State::Workers => {
            let n = m.workers(s);
            let idle = if age.is_empty() { String::new() } else { format!(" · idle {age}") };
            (plural(n, "worker running", "workers running"), format!("its turn ended, its subagents run on{idle}"))
        }
        State::Unread => ("unread".into(), "its turn ended while you were elsewhere".into()),
        State::Locked => (format!("locked {age}").trim_end().to_string(), if is_parked(s) { "parked".into() } else { String::new() }),
        st => (format!("{} {age}", st.name()).trim_end().to_string(), String::new()),
    };
    lines.push((label::R_STATE, value, aside));
    let held = if s.pinned {
        ("pinned".to_string(), "kept loaded, never parked".to_string())
    } else if is_parked(s) {
        let evict = s.evict_due.as_deref().and_then(|t| until(t, now)).map(|u| format!("evicted to history in {u}")).unwrap_or_default();
        ("unloaded".into(), evict)
    } else if s.clients > 0 {
        ("loaded".into(), "a client is attached, so it does not park".into())
    } else {
        ("loaded".into(), s.park_due.as_deref().and_then(|t| until(t, now)).map(|u| format!("parks in {u}")).unwrap_or_default())
    };
    lines.push((label::R_HELD, held.0, held.1));
    if named(&s.name).is_some()
        && let Some(t) = s.title.as_deref().filter(|t| !t.is_empty())
    {
        lines.push((label::R_TITLE, t.to_string(), String::new()));
    }
    if !s.model.is_empty() {
        lines.push((label::R_MODEL, short_model(&s.model), String::new()));
    }
    if let Some(c) = s.cwd.as_deref().filter(|c| !c.is_empty()) {
        lines.push((label::R_CWD, home_short(c), String::new()));
    }
    let cost = if m.is_active(&s.id) { m.stats.cost.unwrap_or(s.cost_usd) } else { s.cost_usd };
    lines.push((label::R_SIZE, format!("{} · ${cost:.2}", plural(s.messages, "msg", "msg")), String::new()));
    let names: Vec<String> = s.roster.iter().map(|c| if c.name.is_empty() { c.kind.clone() } else { c.name.clone() }).collect();
    let clients = s.clients.max(s.roster.len());
    lines.push((label::R_CLIENTS, clients.to_string(), if names.is_empty() { String::new() } else { names.join(", ") }));
    if let Some(l) = &s.locked_by {
        lines.push((label::R_LOCK, l.clone(), "close it there first".into()));
    }
    lines.push((label::R_ID, s.id.clone(), String::new()));
    Readout { title: named(&s.name).or(s.title.as_deref().filter(|t| !t.is_empty())).unwrap_or(&s.id).to_string(), lines }
}

/// A favourite nothing holds, as remembered (the listing does not reach
/// it, or it is gone).
pub fn readout_fav(m: &Model, f: &Favourite, now: i64) -> Readout {
    let mut lines = Vec::new();
    let (value, aside) = match m.fav_state(f) {
        _ if f.is_gone() => ("gone", "a resume found no journal for it; Remove takes it off this list"),
        State::Opening => ("opening", "resume sent, waiting for the daemon"),
        State::Unread => ("unread", "saved · its last turn ended while you were elsewhere"),
        _ => ("saved", "a click resumes it in its folder"),
    };
    lines.push((label::R_STATE, value.to_string(), aside.to_string()));
    if let Some(u) = f.updated_at.as_deref() {
        lines.push((label::R_UPDATED, format!("{} ago", relative(u, now)).replace("now ago", "now"), "as last listed".into()));
    }
    if let Some(model) = f.model.as_deref().filter(|m| !m.is_empty()) {
        lines.push((label::R_MODEL, short_model(model), String::new()));
    }
    if let Some(c) = f.cwd.as_deref().filter(|c| !c.is_empty()) {
        lines.push((label::R_CWD, home_short(c), String::new()));
    }
    lines.push((label::R_ID, f.id.clone(), String::new()));
    Readout { title: f.label.clone(), lines }
}

/// A saved session's readout.
pub fn readout_saved(m: &Model, v: &SavedSession, now: i64) -> Readout {
    let mut lines = Vec::new();
    let (value, aside) = match m.saved_state(v) {
        State::Opening => ("opening", "resume sent, waiting for the daemon"),
        State::Unread => ("unread", "saved · its last turn ended while you were elsewhere"),
        _ => ("saved", "a click resumes it in its folder"),
    };
    lines.push((label::R_STATE, value.to_string(), aside.to_string()));
    if named(&v.name).is_some()
        && let Some(t) = v.title.as_deref().filter(|t| !t.is_empty())
    {
        lines.push((label::R_TITLE, t.to_string(), String::new()));
    }
    if let Some(u) = v.updated_at.as_deref() {
        lines.push((label::R_UPDATED, format!("{} ago", relative(u, now)).replace("now ago", "now"), String::new()));
    }
    if let Some(model) = v.model.as_deref().filter(|m| !m.is_empty()) {
        lines.push((label::R_MODEL, short_model(model), String::new()));
    }
    if let Some(c) = v.cwd.as_deref().filter(|c| !c.is_empty()) {
        lines.push((label::R_CWD, home_short(c), String::new()));
    }
    let size = v.message_count.map(|n| plural(n as usize, "msg", "msg")).unwrap_or_else(|| "size not listed".into());
    lines.push((label::R_SIZE, size, "cost not listed".into()));
    lines.push((label::R_ID, v.id.clone(), String::new()));
    let title = named(&v.name).or(v.title.as_deref().filter(|t| !t.is_empty())).unwrap_or(&v.id);
    Readout { title: title.to_string(), lines }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use synaps_client::{Event, Status};

    use super::*;
    use crate::model::ConnState;

    const NOW: i64 = 1_794_600_000; // 2026-11-13T22:40:00Z

    fn at(secs_ago: i64) -> String {
        crate::demo::rfc3339(NOW - secs_ago)
    }

    fn metas(v: Value) -> Vec<SessionMeta> {
        serde_json::from_value(v).unwrap()
    }

    fn saved(v: Value) -> Vec<SavedSession> {
        serde_json::from_value(v).unwrap()
    }

    fn connected(features: &[&str]) -> Event {
        Event::Status(Status::Connected {
            daemon_version: "0.0.0".into(),
            profile: None,
            pid: 1,
            features: features.iter().map(|f| f.to_string()).collect(),
        })
    }

    /// A model with the saved lists landed as the fetch asks for them:
    /// the newest, then (only if it was cut) the named list.
    fn model(sessions: Value, named: Value, recent: Value, truncated: bool) -> Model {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        m.apply(Event::Sessions(metas(sessions)));
        if truncated {
            m.apply(Event::SavedSessions { sessions: saved(recent), truncated: true, rejected: 0 });
            m.apply(Event::SavedSessions { sessions: saved(named), truncated: false, rejected: 0 });
        } else {
            let mut all = saved(named);
            all.extend(saved(recent));
            m.apply(Event::SavedSessions { sessions: all, truncated: false, rejected: 0 });
        }
        m
    }

    fn shape(rail: &Rail) -> Vec<String> {
        rail.items
            .iter()
            .map(|i| match i {
                Item::Favs { open, count } => format!("[Favs{} {count}]", if *open { "" } else { "+" }),
                Item::Group { group, count } => format!("[{group:?} {count}]"),
                Item::Row(r) => format!("row {}", rail.rows[*r].title),
                Item::Bucket { bucket, open, count } => format!("{} {bucket:?} {count}", if *open { "-" } else { "+" }),
                Item::Hist(r) => format!("  {}", rail.rows[*r].title),
                Item::More { text, .. } => format!("  {text}"),
                Item::Note(t) => format!("note {}", &t[..t.len().min(20)]),
                Item::Empty(t) => format!("empty {t}"),
            })
            .collect()
    }

    fn view(open: u8) -> View {
        View { open, ..View::default() }
    }

    /// The favourites' scene: held (one pinned, one with no journal id
    /// yet), one listed in TODAY, one too old for the capped listing (its
    /// snapshot only), favourited in that order: p2, t3, deep, o1.
    fn fav_scene() -> Model {
        use crate::favourites::{Favourite, Source};
        let sessions = json!([
            {"id": "o1", "lifecycle": "live", "created_at": at(600), "title": "Older open", "cwd": "/home/u/o1"},
            {"id": "o2", "lifecycle": "live", "created_at": at(60), "title": "Newest open"},
            {"id": "p2", "lifecycle": "live", "created_at": at(5), "name": "zulu", "pinned": true, "journal_id": "j-p2"},
            {"id": "p1", "lifecycle": "parked", "created_at": at(9000), "name": "alpha", "pinned": true},
        ]);
        let mut recent = Vec::new();
        for i in 0..10 {
            recent.push(json!({"id": format!("t{i}"), "title": format!("today {i}"), "cwd": "/home/u/t", "updated_at": at(3600 + i), "live": false}));
        }
        // The daemon cut the newest list: j-deep is older than its last.
        let mut m = model(sessions, json!([]), Value::Array(recent), true);
        let now = 1;
        m.favs.toggle(Source::Live(&m.sessions.iter().find(|s| s.id == "p2").unwrap().clone()), now);
        let t3 = m.named.recent.iter().find(|v| v.id == "t3").unwrap().clone();
        m.favs.toggle(Source::Saved(&t3), now);
        let deep: SavedSession = serde_json::from_value(
            json!({"id": "j-deep", "title": "Deep history", "cwd": "/home/u/deep", "model": "anthropic/claude-opus-4-1", "updated_at": at(400 * 86_400)}),
        )
        .unwrap();
        m.favs.list.push(Favourite::of(Source::Saved(&deep), now));
        m.favs.toggle(Source::Live(&m.sessions.iter().find(|s| s.id == "o1").unwrap().clone()), now);
        assert!(m.favs.list[3].provisional, "o1 has no journal id yet");
        m.sync_favs();
        m
    }

    fn keys_once(rail: &Rail) {
        let mut seen = std::collections::HashSet::new();
        for i in &rail.items {
            if let Item::Row(r) | Item::Hist(r) = i {
                assert!(seen.insert(rail.rows[*r].key), "{} is listed twice", rail.rows[*r].title);
            }
        }
    }

    /// Brief 4: FAVOURITES leads the rail in the order the user favourited,
    /// each favourite there only (not in PINNED, OPEN or HISTORY), with its
    /// real state; the pinned one keeps the pin mark; the one the capped
    /// listing no longer reaches renders from its snapshot and resumes by
    /// its journal id in its folder.
    #[test]
    fn favourites_lead_the_rail_and_show_only_there() {
        let m = fav_scene();
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        let s = shape(&rail);
        assert_eq!(
            s[..9],
            [
                "[Favs 4]",
                "row zulu",
                "row today 3",
                "row Deep history",
                "row Older open",
                "[Pinned 1]",
                "row alpha",
                "[Open 1]",
                "row Newest open"
            ]
        );
        assert_eq!(s[9], "[History 9+]");
        assert!(!s.iter().any(|r| r == "  today 3"), "{s:?}");
        keys_once(&rail);
        let row = |t: &str| rail.rows.iter().find(|r| &*r.title == t).unwrap();
        assert!(row("zulu").pinned && row("zulu").live.is_some(), "held: the live row, with the pin mark");
        assert_eq!(row("zulu").fav, Some(0));
        assert!(!row("alpha").pinned, "PINNED needs no mark");
        let deep = row("Deep history");
        assert_eq!((deep.live, deep.saved, deep.fav, deep.gone), (None, None, Some(2), false));
        assert_eq!((&*deep.sub, deep.state), ("/home/u/deep", State::Saved));
        assert_eq!(deep.age.as_ref(), relative(&at(400 * 86_400), NOW));
        let RowAction::Resume(req) = &deep.action else { panic!("{:?}", deep.action) };
        assert_eq!((req.target.as_str(), req.cwd.as_deref(), req.by_name), ("j-deep", Some("/home/u/deep"), false));
        assert!(matches!(row("today 3").saved, Some(SavedRef::Recent(_))), "listed: its entry (the readout)");
        // No favourites: no group.
        let mut m = m;
        m.favs = Default::default();
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        assert_eq!(shape(&rail)[..3], ["[Pinned 2]", "row alpha", "row zulu"]);
        assert!(shape(&rail).contains(&"  today 3".to_string()));
    }

    /// Brief 8: a favourite's row never moves for a state change: running,
    /// needs you, unread, parked, or resumed from history under a new live
    /// id; the live id is upgraded to the journal id once it comes.
    #[test]
    fn favourites_keep_their_order_across_state_changes() {
        let mut m = fav_scene();
        let order = |m: &Model| -> Vec<String> {
            let rail = build(m, &view(Bucket::DEFAULT_OPEN), NOW);
            keys_once(&rail);
            let end = rail.items.iter().position(|i| matches!(i, Item::Group { .. })).unwrap();
            rail.items[1..end].iter().map(|i| if let Item::Row(r) = i { rail.rows[*r].title.to_string() } else { panic!() }).collect()
        };
        let want = ["zulu", "today 3", "Deep history", "Older open"];
        assert_eq!(order(&m), want);
        let base = json!([
            {"id": "o1", "lifecycle": "live", "created_at": at(600), "title": "Older open", "busy": true, "journal_id": "j-o1"},
            {"id": "o2", "lifecycle": "live", "created_at": at(60), "title": "Newest open"},
            {"id": "p2", "lifecycle": "parked", "created_at": at(5), "name": "zulu", "pinned": true, "journal_id": "j-p2", "awaiting_input": 1},
            {"id": "p1", "lifecycle": "parked", "created_at": at(9000), "name": "alpha", "pinned": true},
            {"id": "r3", "lifecycle": "live", "created_at": at(1), "title": "today 3", "journal_id": "t3", "busy": true},
        ]);
        m.apply(Event::Sessions(metas(base)));
        assert!(m.sync_favs(), "o1's journal id came; t3 is held");
        assert_eq!((m.favs.list[3].id.as_str(), m.favs.list[3].provisional), ("j-o1", false));
        assert_eq!(order(&m), want, "running, needs you, resumed: in place");
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        let t3 = rail.rows.iter().find(|r| &*r.title == "today 3").unwrap();
        assert_eq!((t3.live.map(|i| m.sessions[i].id.as_str()), t3.fav), (Some("r3"), Some(1)), "held now: its live row");
        // Back to saved (ended, listed again): the same place.
        m.apply(Event::Sessions(metas(json!([{"id": "o2", "lifecycle": "live", "created_at": at(60), "title": "Newest open"}]))));
        m.sync_favs();
        assert_eq!(order(&m), want);
    }

    /// Brief 4: find and the chips filter FAVOURITES as any group (it hides
    /// when nothing in it matches), it folds to its head (not while
    /// filtering), and an unread favourite nothing holds counts in UNREAD.
    #[test]
    fn favourites_find_fold_and_chips() {
        let mut m = fav_scene();
        let v = |find: &str, chip: Option<Chip>, closed: bool| View {
            find: find.into(),
            chip,
            open: Bucket::DEFAULT_OPEN,
            expanded: 0,
            favs_closed: closed,
        };
        let s = shape(&build(&m, &v("deep", None, false), NOW));
        assert_eq!(s[..2], ["[Favs 1]", "row Deep history"], "{s:?}");
        let s = shape(&build(&m, &v("/home/u/o1", None, true), NOW));
        assert_eq!(s[..2], ["[Favs 1]", "row Older open"], "find opens a folded group: {s:?}");
        let s = shape(&build(&m, &v("newest", None, false), NOW));
        assert!(!s.iter().any(|r| r.starts_with("[Favs")), "nothing matches: hidden: {s:?}");
        let s = shape(&build(&m, &v("", None, true), NOW));
        assert_eq!(s[..2], ["[Favs+ 4]", "[Pinned 1]"], "folded: its head and count");
        // The chips: UNREAD admits the favourite nothing holds; RUNNING the held one.
        m.mark_unread_for_test("t3");
        let rail = build(&m, &v("", Some(Chip::Unread), false), NOW);
        assert_eq!(shape(&rail)[..2], ["[Favs 1]", "row today 3"]);
        assert_eq!(rail.history_unread, 1, "counted in the chip");
        m.apply(Event::Sessions(metas(json!([
            {"id": "o1", "lifecycle": "live", "created_at": at(600), "title": "Older open", "busy": true, "cwd": "/home/u/o1"},
        ]))));
        let s = shape(&build(&m, &v("", Some(Chip::Running), false), NOW));
        assert_eq!(s[..2], ["[Favs 1]", "row Older open"], "{s:?}");
        // M4: folded, a favourite that needs you or is running stays under
        // the head (it is in no other group).
        m.apply(Event::Sessions(metas(json!([
            {"id": "o1", "lifecycle": "live", "created_at": at(600), "title": "Older open", "busy": true, "cwd": "/home/u/o1"},
            {"id": "o2", "lifecycle": "live", "created_at": at(60), "title": "Newest open"},
            {"id": "p2", "lifecycle": "live", "created_at": at(5), "name": "zulu", "pinned": true, "journal_id": "j-p2", "awaiting_input": 1},
        ]))));
        let s = shape(&build(&m, &v("", None, true), NOW));
        assert_eq!(s[..4], ["[Favs+ 4]", "row zulu", "row Older open", "[Open 1]"], "{s:?}");
    }

    /// Brief 6: a favourite whose resume finds no journal shows GONE (with
    /// Remove); it is never dropped by itself, and a refusal for another
    /// reason (a lock) is not gone. Seen again, it is not gone.
    #[test]
    fn a_favourite_whose_journal_is_missing_is_gone_not_pruned() {
        let mut m = fav_scene();
        let deep =
            || ResumeRequest { label: "Deep history".into(), target: "j-deep".into(), cwd: Some("/home/u/deep".into()), by_name: false };
        m.resuming(deep(), 40, false);
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        assert_eq!(rail.rows.iter().find(|r| &*r.title == "Deep history").unwrap().state, State::Opening);
        m.apply(Event::Detached { attach_seq: 40, session_id: None, reason: crate::named::LOCKED_AXEL.into() });
        assert!(!m.favs.list[2].is_gone(), "locked is not gone");
        // H1: the daemon down (its socket gone, a tunnel down): the client's
        // own ENOENT is not gone, and the real reason is what is said.
        m.resuming(deep(), 39, false);
        let down = "attach gave up: connect /home/u/.synaps-cli/run/daemon.sock: No such file or directory (os error 2)";
        m.apply(Event::Detached { attach_seq: 39, session_id: None, reason: down.into() });
        assert!(!m.favs.list[2].is_gone(), "a stopped daemon is not gone");
        assert!(m.named.error.as_deref().unwrap().contains("No such file or directory"), "{:?}", m.named.error);
        m.resuming(deep(), 41, false);
        let missing = "create session: Tool execution failed: Failed to load session 'j-deep': no session matching 'j-deep'";
        m.apply(Event::Detached { attach_seq: 41, session_id: None, reason: missing.into() });
        assert!(m.favs.list[2].is_gone());
        assert_eq!(
            m.named.error.as_deref(),
            Some("'Deep history' is gone: the daemon has no journal for it. It stays in FAVOURITES until you remove it.")
        );
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        let row = rail.rows.iter().find(|r| &*r.title == "Deep history").expect("never pruned");
        assert!(row.gone);
        assert_eq!(readout_fav(&m, &m.favs.list[2], NOW).lines[0].1, "gone");
        assert_eq!(m.favs.list.len(), 4);
        // Removed by the user: gone from the list.
        assert!(m.favs.remove("j-deep"));
        assert_eq!(shape(&build(&m, &view(Bucket::DEFAULT_OPEN), NOW))[0], "[Favs 3]");
    }

    /// M2: a favourite the cached listing still has (asked for before the
    /// verdict) stays GONE through the app's refresh step (`sync_favs`, then
    /// the rail, as `AppView::refresh_rail` runs them); the verdict asks for
    /// a fresh listing, and only that one can say it is there.
    #[test]
    fn gone_survives_the_cached_listing() {
        let mut m = fav_scene();
        let t3 = ResumeRequest { label: "today 3".into(), target: "t3".into(), cwd: Some("/home/u/t".into()), by_name: false };
        m.resuming(t3, 50, false);
        let missing = "create session: Tool execution failed: Failed to load session 't3': no session matching 't3'";
        let fx = m.apply(Event::Detached { attach_seq: 50, session_id: None, reason: missing.into() });
        assert!(
            fx.iter().any(|f| matches!(f, crate::model::Effect::ListSaved { named_only: false, .. })),
            "a fresh listing asked for: {fx:?}"
        );
        let gone = |m: &mut Model| {
            m.sync_favs();
            let rail = build(m, &view(Bucket::DEFAULT_OPEN), NOW);
            keys_once(&rail);
            rail.rows.iter().find(|r| &*r.title == "today 3").expect("never pruned").gone
        };
        assert!(gone(&mut m), "the cached listing has it: still gone");
        assert!(gone(&mut m), "and on the next rebuild");
        // The fresh listing lands without it: gone. With it (the journal is
        // there after all): not.
        let rest: Vec<Value> = m
            .named
            .recent
            .iter()
            .filter(|v| v.id != "t3")
            .map(|v| json!({"id": v.id, "title": v.title, "updated_at": v.updated_at, "live": false}))
            .collect();
        m.apply(Event::SavedSessions { sessions: saved(Value::Array(rest)), truncated: true, rejected: 0 });
        assert!(gone(&mut m));
        m.refresh_saved_for_test();
        m.apply(Event::SavedSessions {
            sessions: saved(json!([{"id": "t3", "title": "today 3", "updated_at": at(3603), "live": false}])),
            truncated: true,
            rejected: 0,
        });
        assert!(!gone(&mut m), "a listing asked for after the verdict has it");
    }

    /// H2: one conversation is one row through its compactions: held (its
    /// journal id moving under one live id), once and twice, open and
    /// parked; evicted to history under its chain head; and resumed from an
    /// old id the daemon followed forward.
    #[test]
    fn a_compacted_favourite_is_one_row() {
        fn once(m: &mut Model, why: &str) {
            m.sync_favs();
            let rail = build(m, &view(Bucket::DEFAULT_OPEN), NOW);
            keys_once(&rail);
            let s = shape(&rail);
            assert_eq!(s.iter().filter(|r| r.ends_with("Long talk")).count(), 1, "{why}: {s:?}");
            assert_eq!(s[..2], ["[Favs 1]", "row Long talk"], "{why}");
        }
        let s = |journal: &str, life: &str| json!({"id": "c1", "lifecycle": life, "created_at": at(50), "title": "Long talk", "journal_id": journal});
        let mut m = model(json!([s("c1", "live")]), json!([]), json!([]), false);
        m.favs.toggle(Source::Live(&m.sessions[0].clone()), 1);
        once(&mut m, "favourited");
        for (journal, life, why) in [("c2", "live", "one compaction, open"), ("c3", "live", "two, open"), ("c3", "parked", "two, parked")] {
            m.apply(Event::Sessions(metas(json!([s(journal, life)]))));
            once(&mut m, why);
        }
        assert_eq!((m.favs.list[0].id.as_str(), &m.favs.list[0].aliases[..]), ("c3", &["c1".to_string(), "c2".to_string()][..]));
        // Evicted: listed under its chain head (c1 and c2 fold into it).
        m.apply(Event::Sessions(vec![]));
        m.refresh_saved_for_test();
        m.apply(Event::SavedSessions {
            sessions: saved(json!([{"id": "c3", "title": "Long talk", "updated_at": at(10), "live": false}])),
            truncated: false,
            rejected: 0,
        });
        once(&mut m, "evicted");
        // Favourited at c1 and never seen since (the listing has only the
        // head): a resume of c1 the daemon follows to c3.
        let mut m = model(json!([]), json!([]), json!([{"id": "c3", "title": "Long talk", "updated_at": at(10), "live": false}]), false);
        let old: SavedSession = serde_json::from_value(json!({"id": "c1", "title": "Long talk", "updated_at": at(900)})).unwrap();
        m.favs.toggle(Source::Saved(&old), 1);
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        assert_eq!(shape(&rail).iter().filter(|r| r.ends_with("Long talk")).count(), 2, "nothing ties them yet");
        m.resuming(ResumeRequest { label: "Long talk".into(), target: "c1".into(), cwd: None, by_name: false }, 9, false);
        let frame = json!({"type": "attached", "client": 2, "meta": {"id": "c3", "journal_id": "c3", "title": "Long talk"}, "conversation": {"api_messages": []}, "input_owner": 2});
        let synaps_client::protocol::DaemonFrame::Attached(a) = synaps_client::protocol::decode_frame(&frame.to_string()).unwrap() else {
            panic!()
        };
        m.apply(Event::Attached { attach_seq: 9, attached: a, mode: synaps_client::AttachMode::Mirror, reconnect: false });
        assert_eq!(m.favs.list[0].id, "c3");
        once(&mut m, "resumed from c1, before the session list says so");
        m.apply(Event::Sessions(metas(
            json!([{"id": "c3", "lifecycle": "live", "created_at": at(1), "title": "Long talk", "journal_id": "c3"}]),
        )));
        once(&mut m, "resumed, held");
    }

    /// Precedence: OPENING › NEEDS YOU › RUNNING › WORKERS › LOCKED › UNREAD
    /// › IDLE › PARKED; each state not at rest has its word, those at rest
    /// none.
    #[test]
    fn states_follow_their_precedence() {
        let mut m = Model::new(ConnState::Connecting);
        let all = json!([
            {"id": "a", "lifecycle": "live", "busy": true, "awaiting_input": 1, "locked_by": "pid 1 (tui)"},
            {"id": "b", "lifecycle": "live", "busy": true, "locked_by": "pid 1 (tui)"},
            {"id": "c", "lifecycle": "parked", "locked_by": "pid 1 (tui)"},
            {"id": "d", "lifecycle": "live"},
            {"id": "e", "lifecycle": "parking"},
            {"id": "f", "lifecycle": "parked"},
            {"id": "g", "lifecycle": "live", "subagents_running": 1, "locked_by": "pid 1 (tui)"},
            {"id": "h", "lifecycle": "live", "busy": true, "subagents_running": 3},
        ]);
        m.apply(Event::Sessions(metas(all)));
        let got: Vec<State> = m.sessions.iter().map(|s| m.state(s)).collect();
        // The model keeps the list newest first by created_at (all equal here: by id, descending).
        let ids: Vec<&str> = m.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["h", "g", "f", "e", "d", "c", "b", "a"]);
        assert_eq!(
            got,
            [State::Running, State::Workers, State::Parked, State::Parked, State::Idle, State::Locked, State::Running, State::NeedsYou]
        );
        m.want(3);
        m.asked_for("a");
        assert_eq!(m.state(&m.sessions[7]), State::Opening, "an attach on its way beats everything");
        for s in [State::Opening, State::NeedsYou, State::Running, State::Workers, State::Locked, State::Unread] {
            assert!(s.word().is_some(), "{s:?}");
        }
        for s in [State::Idle, State::Parked, State::Saved] {
            assert!(s.word().is_none(), "{s:?}");
        }
    }

    /// Groups: PINNED by name (hidden when empty), OPEN newest first by
    /// `created_at` whatever their state, `ending` gone; HISTORY in
    /// buckets, NAMED and TODAY open with 8 rows then `+ N MORE`, closed
    /// buckets build no rows, held sessions are not history.
    #[test]
    fn groups_and_buckets() {
        let sessions = json!([
            {"id": "o1", "lifecycle": "live", "created_at": at(600), "title": "Older open", "busy": true},
            {"id": "o2", "lifecycle": "live", "created_at": at(60), "title": "Newest open"},
            {"id": "p2", "lifecycle": "live", "created_at": at(5), "name": "zulu", "pinned": true},
            {"id": "p1", "lifecycle": "parked", "created_at": at(9000), "name": "alpha", "pinned": true},
            {"id": "x", "lifecycle": "ending", "created_at": at(1)},
            {"id": "h", "lifecycle": "live", "created_at": at(3), "journal_id": "j-held", "name": "held"},
        ]);
        let named = json!([
            {"id": "n1", "name": "kite", "updated_at": at(400 * 86_400), "live": false},
            {"id": "n2", "name": "bebop", "updated_at": at(100), "live": false},
            {"id": "j-held", "name": "held", "updated_at": at(10), "live": false},
            {"id": "n3", "name": "axel", "live": true},
        ]);
        let mut recent = vec![json!({"id": "n2", "name": "bebop", "updated_at": at(100), "live": false})];
        for i in 0..10 {
            recent.push(json!({"id": format!("t{i}"), "title": format!("today {i}"), "updated_at": at(3600 + i), "live": false}));
        }
        recent.push(json!({"id": "w", "title": "this week", "updated_at": at(3 * 86_400), "live": false}));
        recent.push(json!({"id": "mo", "title": "this month", "updated_at": at(20 * 86_400), "live": false}));
        recent.push(json!({"id": "old", "title": "older", "updated_at": at(90 * 86_400), "live": false}));
        let m = model(sessions, named, Value::Array(recent), false);
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        let mut want: Vec<String> =
            ["[Pinned 2]", "row alpha", "row zulu", "[Open 3]", "row held", "row Newest open", "row Older open", "[History 15]"]
                .map(String::from)
                .to_vec();
        want.extend(["- Named 2", "  bebop", "  kite", "- Today 10"].map(String::from));
        want.extend((0..8).map(|i| format!("  today {i}")));
        want.extend(["  + 2   M O R E", "+ Week 1", "+ Month 1", "+ Older 1"].map(String::from));
        assert_eq!(shape(&rail), want);
        assert!(rail.rows[0].named && !rail.rows[3].named && rail.rows[5].named);
        // `+ N MORE` shows the rest; every bucket open.
        let all = View { open: 0x1f, expanded: Bucket::Today.bit(), ..View::default() };
        let rail = build(&m, &all, NOW);
        assert_eq!(rail.items.iter().filter(|i| matches!(i, Item::Hist(_))).count(), 15);
        // A state change does not move a row.
        let mut m2 = m;
        let mut moved = m2.sessions.clone();
        for s in &mut moved {
            s.busy = !s.busy;
        }
        let before = shape(&build(&m2, &view(0), NOW));
        m2.apply(Event::Sessions(moved));
        assert_eq!(shape(&build(&m2, &view(0), NOW)), before);
    }

    /// A truncated recent list: buckets newer than its oldest entry are
    /// exact, the rest say `+`; deep, OLDER says where the listing ends.
    #[test]
    fn a_truncated_history_says_so() {
        let recent = json!([
            {"id": "t", "title": "today", "updated_at": at(60), "live": false},
            {"id": "w", "title": "week", "updated_at": at(2 * 86_400), "live": false},
        ]);
        let mut m = model(json!([]), json!([]), recent, true);
        let rail = build(&m, &view(0), NOW);
        assert_eq!(
            shape(&rail),
            ["[Open 0]", "empty No open sessions", "[History 2+]", "+ Today 1", "+ Week 1+", "+ Month 0+", "+ Older 0+"]
        );
        let fx = m.want_deep();
        assert_eq!(fx, [crate::model::Effect::ListSaved { named_only: false, limit: 1000 }]);
        assert!(m.want_deep().is_empty(), "once");
        m.apply(Event::SavedSessions { sessions: Vec::new(), truncated: true, rejected: 0 });
        let rail = build(&m, &view(Bucket::Older.bit()), NOW);
        assert!(matches!(rail.items.last(), Some(Item::Note(t)) if t.contains("newest 1,000")), "{:?}", shape(&rail));
    }

    /// Find filters every group by name, title or folder; the chips by
    /// state (forcing the buckets open); nothing found says so.
    #[test]
    fn find_and_chips() {
        let sessions = json!([
            {"id": "a", "lifecycle": "live", "title": "Rail states", "cwd": "/w/synaps", "awaiting_input": 1, "created_at": at(5)},
            {"id": "b", "lifecycle": "live", "title": "Bisect", "cwd": "/w/axon", "busy": true, "created_at": at(4)},
        ]);
        let recent = json!([{"id": "s1", "title": "Synaps release", "updated_at": at(60), "live": false},
                            {"id": "s2", "title": "Other", "updated_at": at(60), "live": false}]);
        let m = model(sessions, json!([]), recent, false);
        let v = View { find: "synaps".into(), ..View::default() };
        assert_eq!(shape(&build(&m, &v, NOW)), ["[Open 1]", "row Rail states", "[History 1]", "- Today 1", "  Synaps release"]);
        let v = View { chip: Some(Chip::Running), ..View::default() };
        assert_eq!(shape(&build(&m, &v, NOW)), ["[Open 1]", "row Bisect"]);
        let v = View { chip: Some(Chip::Unread), ..View::default() };
        assert_eq!(shape(&build(&m, &v, NOW)), ["empty Nothing matches"]);
    }

    /// M3: a search over a list known to be cut, or still coming, says
    /// what it searched; "Nothing matches" only when the search was whole.
    #[test]
    fn a_search_says_what_it_searched() {
        let recent = json!([{"id": "t", "title": "today", "updated_at": at(60), "live": false}]);
        let v = View { find: "zzz".into(), ..View::default() };
        let mut m = model(json!([]), json!([]), recent.clone(), true);
        let notes = |m: &Model| -> Vec<String> {
            build(m, &v, NOW).items.iter().filter_map(|i| if let Item::Note(t) = i { Some(t.to_string()) } else { None }).collect()
        };
        assert_eq!(shape(&build(&m, &v, NOW))[0], "[History 0+]");
        assert_eq!(notes(&m), ["Searched the newest 1 saved sessions. Anything older needs a daemon search."]);
        m.want_deep();
        assert_eq!(notes(&m), ["Listing saved sessions… searched the newest 1 so far."]);
        m.apply(Event::SavedSessions { sessions: saved(recent.clone()), truncated: false, rejected: 0 });
        assert_eq!(shape(&build(&m, &v, NOW)), ["empty Nothing matches"], "the whole list: a miss is a miss");
        let mut m = Model::new(ConnState::Connecting);
        m.apply(connected(&["saved_sessions"]));
        assert_eq!(shape(&build(&m, &v, NOW)), ["[History …]", "note Listing saved sessio"]);
    }

    /// L6: find matches what a row shows: an untitled row's id, the folder
    /// as `~/…`; not an id the row does not show.
    #[test]
    fn find_matches_what_rows_show() {
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap();
        let sessions = json!([
            {"id": "20261113-101010-abcd", "lifecycle": "live", "messages": 3, "cwd": format!("{home}/work/synaps"), "created_at": at(5)},
            {"id": "20261113-101010-ffff", "lifecycle": "live", "title": "Titled", "created_at": at(4)},
        ]);
        let m = model(sessions, json!([]), json!([]), false);
        let rows = |find: &str| shape(&build(&m, &View { find: find.into(), ..View::default() }, NOW));
        assert_eq!(rows("abcd"), ["[Open 1]", "row 20261113-101010-abcd"]);
        assert_eq!(rows("~/work"), ["[Open 1]", "row 20261113-101010-abcd"]);
        assert_eq!(rows("ffff"), ["empty Nothing matches"]);
    }

    /// Without `saved_sessions` HISTORY is a header and a note that says
    /// why and what still works; not connected, nothing.
    #[test]
    fn without_saved_sessions_history_says_why() {
        let mut m = Model::new(ConnState::Connecting);
        assert_eq!(shape(&build(&m, &View::default(), NOW)), ["[Open 0]", "empty Not connected"]);
        m.apply(connected(&["owned_sessions"]));
        m.apply(Event::Sessions(metas(json!([{"id": "a", "lifecycle": "live", "name": "kite"}]))));
        let rail = build(&m, &View::default(), NOW);
        assert_eq!(shape(&rail), ["[Open 1]", "row kite", "[History —]", "note This daemon does not"]);
    }

    /// H1: a saved row carries its folder into `CreateOptions` as is (no
    /// local check), with `~` as the fallback; no folder: `~`.
    #[test]
    fn a_saved_rows_folder_reaches_the_create() {
        use synaps_client::Cwd;
        let named = json!([{"id": "j-axel", "name": "axel", "cwd": "/home/u/Projects/axel", "updated_at": at(60), "live": false}]);
        let m = model(json!([]), named, json!([]), false);
        let rail = build(&m, &view(Bucket::DEFAULT_OPEN), NOW);
        let RowAction::Resume(req) = &rail.rows[0].action else { panic!("{:?}", rail.rows[0].action) };
        let o = req.options();
        assert_eq!(o.cwd, Cwd::Path("/home/u/Projects/axel".into()));
        assert_eq!(o.fallback_cwd, Some(Cwd::Home));
        assert_eq!(o.clone().resume(&req.target).config().continue_session, Some(Some("j-axel".into())));
        let req = ResumeRequest { label: "x".into(), target: "x".into(), cwd: None, by_name: true };
        assert_eq!((req.options().cwd, req.options().fallback_cwd), (Cwd::Home, None));
    }

    #[test]
    fn splices_are_minimal() {
        assert_eq!(splice(&[1, 2, 3], &[1, 2, 3]), None);
        assert_eq!(splice(&[1, 2, 3], &[1, 9, 3]), Some((1..2, 1)));
        assert_eq!(splice(&[1, 2, 3], &[1, 2, 3, 4]), Some((3..3, 1)));
        assert_eq!(splice(&[1, 2, 3], &[0, 1, 2, 3]), Some((0..0, 1)));
        assert_eq!(splice(&[1, 2, 2, 3], &[1, 2, 3]), Some((2..3, 0)));
        assert_eq!(splice(&[], &[1]), Some((0..0, 1)));
        assert_eq!(splice(&[1], &[]), Some((0..1, 0)));
    }

    #[test]
    fn buckets_and_times() {
        assert_eq!(Bucket::of_age(Some(NOW - 60), NOW), Bucket::Today);
        assert_eq!(Bucket::of_age(Some(NOW - 86_400), NOW), Bucket::Week);
        assert_eq!(Bucket::of_age(Some(NOW - 7 * 86_400), NOW), Bucket::Month);
        assert_eq!(Bucket::of_age(Some(NOW - 30 * 86_400), NOW), Bucket::Older);
        assert_eq!(Bucket::of_age(None, NOW), Bucket::Older);
        assert_eq!(Bucket::of_age(Some(NOW + 999), NOW), Bucket::Today, "clock skew");
        assert_eq!(until(&at(-45), NOW).as_deref(), Some("<1m"));
        assert_eq!(until(&at(-150), NOW).as_deref(), Some("2m"));
        assert_eq!(until(&at(-3 * 3600), NOW).as_deref(), Some("3h"));
        assert_eq!(until(&at(10), NOW).as_deref(), Some("<1m"));
        assert_eq!(until("x", NOW), None);
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    /// The readout: state and since when, how the daemon holds it (park or
    /// eviction time, from `park_due` / `evict_due`), clients by name, the
    /// lock holder; a saved session says what it lacks.
    #[test]
    fn readouts_say_what_the_daemon_sent() {
        let mut m = Model::new(ConnState::Connecting);
        m.apply(Event::Sessions(metas(json!([
            {"id": "p", "lifecycle": "parked", "state_since": at(38 * 60), "evict_due": at(-22 * 60), "messages": 44, "cost_usd": 0.88,
             "model": "anthropic/claude-sonnet-4-5", "cwd": "/srv/x"},
            {"id": "l", "lifecycle": "live", "clients": 1, "locked_by": "pid 48213 (tui)", "messages": 1, "name": "axel", "title": "Forum",
             "roster": [{"id": 3, "name": "tui", "kind": "tui", "mode": "mirror", "attached_at_ms": 0}]},
            {"id": "w", "lifecycle": "live", "state_since": at(4 * 60), "subagents_running": 2},
        ]))));
        let w = m.sessions.iter().find(|s| s.id == "w").unwrap();
        assert_eq!(
            readout_live(&m, w, NOW).lines[0],
            (label::R_STATE, "2 workers running".into(), "its turn ended, its subagents run on · idle 4m".into())
        );
        let p = m.sessions.iter().find(|s| s.id == "p").unwrap();
        let r = readout_live(&m, p, NOW);
        assert_eq!(r.title, "p");
        assert_eq!(r.lines[0], (label::R_STATE, "parked 38m".into(), String::new()));
        assert_eq!(r.lines[1], (label::R_HELD, "unloaded".into(), "evicted to history in 22m".into()));
        assert!(r.lines.contains(&(label::R_SIZE, "44 msg · $0.88".into(), String::new())));
        assert!(r.lines.contains(&(label::R_MODEL, "sonnet-4-5".into(), String::new())));
        let l = m.sessions.iter().find(|s| s.id == "l").unwrap();
        let r = readout_live(&m, l, NOW);
        assert_eq!((r.title.as_str(), r.lines[0].1.as_str()), ("axel", "locked"));
        assert!(r.lines.contains(&(label::R_TITLE, "Forum".into(), String::new())));
        assert!(r.lines.contains(&(label::R_CLIENTS, "1".into(), "tui".into())));
        assert!(r.lines.contains(&(label::R_LOCK, "pid 48213 (tui)".into(), "close it there first".into())));
        let v: SavedSession =
            serde_json::from_value(json!({"id": "j", "title": "T", "updated_at": at(3 * 86_400), "live": false})).unwrap();
        let r = readout_saved(&m, &v, NOW);
        assert_eq!(r.lines[0].1, "saved");
        assert!(r.lines.contains(&(label::R_UPDATED, "3d ago".into(), String::new())));
        assert!(r.lines.contains(&(label::R_SIZE, "size not listed".into(), "cost not listed".into())));
    }
}
