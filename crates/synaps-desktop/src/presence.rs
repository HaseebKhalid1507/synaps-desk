//! Never miss a waiting card (synaps-cards `docs/design.md` v1.2, §6.1's
//! presence and §7's F10–F12; the D3 row of §9.3): which open sessions
//! have an agent card nobody has answered, and what the window does with
//! that.
//!
//! **The model** ([`Presence`]): every open card of every session this
//! window holds, oldest first, each a **wait** card (the agent ended its
//! turn on it: NEEDS YOU) or an **open** one (it works on: ASKED, §1.3).
//! It is fed only from what D1 and D2a keep: each session's fold as its
//! rows draw it ([`synaps_client::Transcript::cards`]), less the cards
//! this desk has answered and whose answer is on its way or in
//! ([`crate::deliver::Memos::entry`]; one unconfirmed or not sent still
//! needs you). The sessions it reads (§6.1, "attached sessions"):
//! - every attachment (the session each pane shows): **live**, current.
//!   Only these are NEEDS YOU or ASKED: the rail, the tab's state, the top
//!   bar's count, a filled tab dot, and the OS notification.
//! - every other session open in a tab: **as last seen**, the tab cache's
//!   transcript, unverified. Its card is drawn as last seen (a hollow,
//!   dimmed dot; "last seen" in WAITING), never counted, never notified;
//!   opening the tab attaches it again, and the attach says whether the
//!   card still waits (a jump lands only if it does).
//!
//! The limitation, both ways: a background tab's session isn't attached,
//! so what changes there isn't heard until it is shown again. A card
//! asked there after the tab was left has no dot; a card answered,
//! closed, compacted away or gone with a `/new` elsewhere still has its
//! last-seen dot. A session only in the rail is never read: the daemon
//! says nothing of its cards (F11, §8 #1).
//!
//! **What reads it:** the rail's NEEDS YOU and ASKED ([`crate::model::Model::fact`]),
//! the tab dot ([`AppView::tab_dot`]), the palette's WAITING group,
//! Ctrl+Shift+J ([`AppView::jump_waiting`]) and the OS notification.
//! Nothing here moves: no frame is asked for, a dot or a badge is drawn
//! still (0 frames idle at every motion level). It is derived again only
//! when a fold, the memos or the set of sessions moved.
//!
//! **The OS notification** (F12, [`crate::notify`]): one per wait card
//! that came live, after its session's attach ([`Arrivals`]: decided at
//! the event that brought it, in event order), while the window is away
//! or that session isn't the one on screen. Never one an attach or a
//! history replacement brought (it was there before), never an open
//! (ASKED) card, never twice for one binding while its session is open
//! here, whatever its card does meanwhile (an answer on its way, then not
//! sent; a history that drops it and brings it back). Taken down when its
//! card resolves, or its window goes ([`window_gone`]). Its click is
//! checked against what the window holds as it comes ([`AppView::still_open`]):
//! if the card still waits, the window comes up and lands on it, as
//! Ctrl+Shift+J does; a stale one does nothing. `notify.cards` (`false` or `"off"`) in the settings file
//! turns them off.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use cards_spec::CardId;
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::*;
use synaps_client::cards::{CardRow, State};
use synaps_client::{Event, HistoryUpdate, Transcript};

use crate::app::AppView;
use crate::tabs::Kind as TabKind;
use crate::theme::Palette;
use crate::turns::Kind;

/// A card's binding: its session, its call, and the id its result bound
/// it to (as D2a's memo and D2b's entry name it). A card bound again is
/// another binding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub session: String,
    pub tool_id: String,
    pub card: CardId,
}

/// An open card: asked, bound, and nobody's answer yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Open {
    pub session: String,
    /// Its call's tool id: its row.
    pub tool_id: String,
    pub card: CardId,
    /// The agent ended its turn on it (§1.3).
    pub wait: bool,
    /// Its question, one line (sanitized: [`crate::ask::question_line`]).
    pub question: String,
    /// When it was asked (its row's time, milliseconds), if the row says.
    pub at: Option<i64>,
    /// Read from an attachment: current. Else as last seen (a background
    /// tab's cache): unverified.
    pub live: bool,
}

impl Open {
    pub(crate) fn key(&self) -> Key {
        Key { session: self.session.clone(), tool_id: self.tool_id.clone(), card: self.card.clone() }
    }

    fn is(&self, k: &Key) -> bool {
        self.session == k.session && self.tool_id == k.tool_id && self.card == k.card
    }

    /// Oldest first; no time after a time; then by session and call, so
    /// the order never depends on which pane or tab a card was read from.
    fn age(&self, o: &Self) -> std::cmp::Ordering {
        (self.at.unwrap_or(i64::MAX), &self.session, &self.tool_id).cmp(&(o.at.unwrap_or(i64::MAX), &o.session, &o.tool_id))
    }
}

/// A session's open cards, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Count {
    /// Wait cards: NEEDS YOU.
    pub wait: usize,
    /// The rest: ASKED.
    pub asked: usize,
}

/// Every open card the window holds, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Presence {
    cards: Vec<Open>,
    /// Bumped when the cards change: what is built from them is built again.
    rev: u64,
}

impl Presence {
    /// The window's open cards now; whether that changed anything.
    pub fn set(&mut self, mut cards: Vec<Open>) -> bool {
        cards.sort_by(Open::age);
        if cards == self.cards {
            return false;
        }
        self.cards = cards;
        self.rev += 1;
        true
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Oldest first.
    pub fn cards(&self) -> &[Open] {
        &self.cards
    }

    fn count(&self, session: &str, live: bool) -> Count {
        let mut n = Count::default();
        for c in self.cards.iter().filter(|c| c.session == session && c.live == live) {
            if c.wait {
                n.wait += 1;
            } else {
                n.asked += 1;
            }
        }
        n
    }

    /// `session`'s live cards: its NEEDS YOU and ASKED.
    pub fn of(&self, session: &str) -> Count {
        self.count(session, true)
    }

    /// `session`'s cards as last seen (a background tab's): unverified.
    pub fn last_seen(&self, session: &str) -> Count {
        self.count(session, false)
    }

    /// Live wait cards, live open ones, then those as last seen (wait
    /// cards first), each oldest first: Ctrl+Shift+J's order and WAITING's.
    pub fn listed(&self) -> impl Iterator<Item = &Open> {
        [(true, true), (true, false), (false, true), (false, false)]
            .into_iter()
            .flat_map(move |(live, wait)| self.cards.iter().filter(move |c| c.live == live && c.wait == wait))
    }

    /// Ctrl+Shift+J's: the first [`Presence::listed`].
    pub fn next(&self) -> Option<&Open> {
        self.listed().next()
    }

    /// Call `tool_id`'s open card in `session`, if one is (the tests').
    #[cfg(test)]
    pub fn get(&self, session: &str, tool_id: &str) -> Option<&Open> {
        self.cards.iter().find(|c| c.session == session && c.tool_id == tool_id)
    }

    /// Whether binding `k` is open here (live, or as last seen).
    pub fn holds(&self, k: &Key) -> bool {
        self.cards.iter().any(|c| c.is(k))
    }
}

/// Whether this desk's answer to `c` is on its way or in (its memo; one
/// unconfirmed or not sent still needs you).
pub(crate) fn answered(memos: &crate::deliver::Memos, session: &str, c: &cards_spec::fold::Card) -> bool {
    memos.entry(session, c).is_some_and(|e| {
        use cards_spec::memo::State as M;
        !matches!(e.state, M::Unconfirmed | M::NotSent)
    })
}

/// A row time to the millisecond (RFC 3339, its fraction cut there).
fn at_ms(s: &str) -> Option<i64> {
    let secs = crate::named::parse_rfc3339(s)?;
    let frac = s.get(19..).and_then(|r| r.strip_prefix('.')).map_or(0, |f| {
        let d: String = f.chars().take_while(char::is_ascii_digit).take(3).collect();
        format!("{d:0<3}").parse::<i64>().unwrap_or(0)
    });
    Some(secs * 1000 + frac)
}

/// `session`'s open cards in `t`, the fold its rows draw: bound and not
/// answered ([`State::Open`]), less those `answered` (this desk's answer
/// on its way or in). One pass over the rows, for their times, and only
/// when a card is open.
pub(crate) fn open_cards(session: &str, t: &Transcript, live: bool, answered: impl Fn(&cards_spec::fold::Card) -> bool) -> Vec<Open> {
    let open: Vec<&cards_spec::fold::Card> =
        t.cards().cards().iter().filter(|c| c.state == State::Open && c.id.is_some() && c.doc.is_some() && !answered(c)).collect();
    if open.is_empty() {
        return Vec::new();
    }
    let mut at: HashMap<&str, Option<i64>> = HashMap::new();
    for i in t.items().iter().rev() {
        if let (Some(CardRow::Ask(_)), Some(tc)) = (i.card.as_deref(), i.tool.as_ref())
            && open.iter().any(|c| c.tool_id == tc.tool_id)
        {
            at.entry(tc.tool_id.as_str()).or_insert_with(|| i.at.as_deref().and_then(at_ms));
        }
    }
    open.into_iter()
        .filter_map(|c| {
            Some(Open {
                session: session.to_string(),
                tool_id: c.tool_id.clone(),
                card: c.id.clone()?,
                wait: c.wait,
                question: crate::ask::question_line(&c.doc.as_ref()?.question),
                at: at.get(c.tool_id.as_str()).copied().flatten(),
                live,
            })
        })
        .collect()
}

/// What an event says of the wait cards it brings to its attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    /// They were there before: an attach's snapshot, or a history
    /// replaced (a resync, `/resume`, a compaction).
    Baseline,
    /// They came now: a live event, or messages appended to the history.
    Live,
}

impl Look {
    /// What `ev` brings, and to which attachment.
    pub(crate) fn of(ev: &Event) -> Option<(Look, u64)> {
        match ev {
            Event::Attached { attach_seq, .. } => Some((Look::Baseline, *attach_seq)),
            Event::Session { attach_seq, .. } => Some((Look::Live, *attach_seq)),
            Event::History { attach_seq, update: HistoryUpdate::Append { .. }, .. } => Some((Look::Live, *attach_seq)),
            Event::History { attach_seq, .. } => Some((Look::Baseline, *attach_seq)),
            _ => None,
        }
    }
}

/// Bindings remembered once they are no longer open, at most (the oldest
/// forgotten first). One still open in its session's fold is never
/// forgotten while its session is held; a session this window no longer
/// holds is forgotten whole.
pub(crate) const SEEN_MAX: usize = 4096;

/// F12's provenance, at the event boundary ([`crate::model::Model::apply`]):
/// every bound wait card a session's events bring is seen once per
/// binding. One the attach's snapshot or a replaced history brought is
/// the baseline (there before: never told); one that comes after, live,
/// in event order, is an arrival (the window decides, after the batch,
/// whether to tell the OS). Seen stays seen while the session is open in
/// this window: a card answered and then not sent, or dropped by a
/// history and brought back, is never new again.
///
/// The bound (Shady #83 R2-6): a binding open in its session's fold at
/// that session's last look is kept, however many there are (they are
/// the fold's own cards, already in memory). Only bindings no longer open
/// are bounded, [`SEEN_MAX`] of them, the oldest to stop being open
/// forgotten first; so an open card is never news again. The one
/// exception, by that bound: a binding that left its fold, stayed out
/// while [`SEEN_MAX`] others left theirs, and then comes back is told
/// again.
#[derive(Debug)]
pub(crate) struct Arrivals {
    seen: HashSet<Key>,
    /// Each held session's bindings open at its last look: never forgotten.
    open: HashMap<String, HashSet<Key>>,
    /// Seen bindings no longer open, in the order they stopped being: the
    /// oldest forgotten past `cap`.
    left: VecDeque<Key>,
    cap: usize,
    came: Vec<Key>,
    /// Each session's fold revision at its last live look: unchanged,
    /// nothing is read.
    looked: HashMap<String, u64>,
}

impl Default for Arrivals {
    fn default() -> Self {
        Self::with_cap(SEEN_MAX)
    }
}

impl Arrivals {
    pub(crate) fn with_cap(cap: usize) -> Self {
        Self { seen: HashSet::new(), open: HashMap::new(), left: VecDeque::new(), cap, came: Vec::new(), looked: HashMap::new() }
    }

    /// `session`'s fold `t`, as event `how` left it.
    pub(crate) fn look(&mut self, how: Look, session: &str, t: &Transcript) {
        let rev = t.cards_rev();
        match self.looked.get_mut(session) {
            Some(r) if how == Look::Live && *r == rev => return,
            Some(r) => *r = rev,
            None => {
                self.looked.insert(session.to_string(), rev);
            }
        }
        let mut now: HashSet<Key> = HashSet::new();
        for c in t.cards().cards().iter().filter(|c| c.wait && c.state == State::Open) {
            let Some(card) = c.id.clone() else { continue };
            let k = Key { session: session.to_string(), tool_id: c.tool_id.clone(), card };
            if self.seen.insert(k.clone()) && how == Look::Live {
                self.came.push(k.clone());
            }
            now.insert(k);
        }
        let before = self.open.insert(session.to_string(), now).unwrap_or_default();
        let now = &self.open[session];
        // Back in the fold: no longer bounded.
        if self.left.iter().any(|k| k.session == session && now.contains(k)) {
            self.left.retain(|k| !now.contains(k));
        }
        // Out of it: bounded from now on.
        self.left.extend(before.into_iter().filter(|k| !now.contains(k)));
        while self.left.len() > self.cap {
            if let Some(k) = self.left.pop_front() {
                self.seen.remove(&k);
            }
        }
    }

    /// The arrivals since the last take, in event order.
    pub(crate) fn take(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.came)
    }

    /// Only the sessions this window still `held`: the rest forgotten (an
    /// attach brings them back as the baseline).
    pub(crate) fn keep(&mut self, held: impl Fn(&str) -> bool) {
        self.seen.retain(|k| held(&k.session));
        self.open.retain(|s, _| held(s));
        self.left.retain(|k| held(&k.session));
        self.came.retain(|k| held(&k.session));
        self.looked.retain(|s, _| held(s));
    }

    #[cfg(test)]
    pub(crate) fn seen(&self) -> usize {
        self.seen.len()
    }
}

/// A jump's landing: the binding to bring into view once its session is
/// on screen and its row is built ([`AppView::land_card`]).
#[derive(Clone, Debug)]
pub(crate) struct Land {
    pub key: Key,
    pub at: Instant,
}

/// A landing that hasn't happened in this long (the attach failed, the
/// card went) is dropped: it never jumps the view later.
pub(crate) const LAND_WAIT: Duration = Duration::from_secs(10);

/// What a presence was derived from: each source's (live, fold
/// revision), the memos' changes, and the sessions held.
type Derived = (Vec<(bool, u64)>, u64, Vec<String>);

/// The window's lookout for open cards: the landing on its way, the
/// notifications up, and what the presence was last derived from. (D2b's
/// `crate::waiting` is Ctrl+J's order; [`Presence::listed`] is this
/// window's over every open session.)
#[derive(Debug, Default)]
pub(crate) struct Lookout {
    pub land: Option<Land>,
    pub notes: crate::notify::Notes,
    /// The last card a jump landed on ([`AppView::card_landed`] enters
    /// it, if it is still open there).
    pub landed: Option<Key>,
    /// A card landed on while the window was not active (a notification's
    /// click raised it): entered as it comes up ([`AppView::landed_up`]).
    pub entering: Option<Land>,
    /// Each source's (live, fold revision), the memos' changes, and the
    /// sessions held: unchanged, the presence is as it was.
    derived: Option<Derived>,
    /// How many derivations ran (a test's count).
    pub derivations: u64,
}

/// `notify.cards` in the settings file: `false` or `"off"` turns the OS
/// notifications off.
pub(crate) fn quiet(v: Option<&serde_json::Value>) -> bool {
    matches!(v, Some(serde_json::Value::Bool(false))) || v.and_then(serde_json::Value::as_str) == Some("off")
}

/// Ctrl+Shift+J (Ctrl on macOS too, as Ctrl+J).
pub(crate) fn is_jump(k: &Keystroke) -> bool {
    let m = &k.modifiers;
    k.key.eq_ignore_ascii_case("j") && m.control && m.shift && !(m.alt || m.platform || m.function)
}

/// NEEDS YOU's and ASKED's dots at least this far apart (OKLab ΔE).
pub(crate) const DOT_APART: f64 = 0.1;

fn rgb(h: Hsla) -> u32 {
    let c = Rgba::from(h);
    let q = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (q(c.r) << 16) | (q(c.g) << 8) | q(c.b)
}

fn apart(a: Hsla, b: Hsla) -> f64 {
    let (p, q) = (crate::theme::oklab(rgb(a)), crate::theme::oklab(rgb(b)));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

/// The tab dots: NEEDS YOU's in the needs-you mark; ASKED's in the
/// accent's, or where an accent is (near) the needs-you colour (a custom
/// one, or phosphor's own) the first of the chrome's ink, prose and dim
/// text that stands apart from it (the words differ already; the dots
/// must too).
pub(crate) fn dot_colors(p: &Palette) -> (Hsla, Hsla) {
    let wait = p.warn_mark_ch;
    let tries = [p.focus_ch, p.ink, p.prose_ch, p.dim_ch];
    let asked = tries
        .into_iter()
        .find(|c| apart(*c, wait) >= DOT_APART)
        .unwrap_or_else(|| tries.into_iter().max_by(|a, b| apart(*a, wait).total_cmp(&apart(*b, wait))).unwrap_or(p.ink));
    (wait, asked)
}

impl AppView {
    /// The presence as the window holds it now, then what the OS is told:
    /// the arrivals' notifications, and those of cards no longer open
    /// taken down. After every batch of events, and as a frame begins
    /// (tabs closed, a pane's switch). Nothing is read again while no
    /// fold, memo or held session moved.
    pub(crate) fn sync_presence(&mut self, cx: &mut Context<Self>) {
        let m = &self.model;
        let mut live: Vec<(String, crate::live::LiveId)> = Vec::new();
        for id in m.lives.ids() {
            if let Some(a) = m.lives[id].active.as_ref()
                && !live.iter().any(|(s, _)| *s == a.session_id)
            {
                live.push((a.session_id.clone(), id));
            }
        }
        // The other sessions open in a tab (any pane's), as last seen.
        let tabs = m.tabs.iter().chain(self.panes.rest.iter().flat_map(|p| p.parked.tabs.iter()));
        let mut held: Vec<&str> = live.iter().map(|(s, _)| s.as_str()).collect();
        let mut cached: Vec<&str> = Vec::new();
        for s in tabs.filter(|t| t.kind == TabKind::Session).filter_map(|t| t.session.as_deref()) {
            if !held.contains(&s) {
                held.push(s);
                cached.push(s);
            }
        }
        let mut key: Vec<(bool, u64)> = live.iter().map(|(_, id)| (true, m.lives[*id].transcript.cards_rev())).collect();
        key.extend(cached.iter().filter_map(|s| self.tab_cache.peek(*s)).map(|c| (false, c.transcript.cards_rev())));
        let now = Some((key, m.memos.changes(), held.iter().map(|s| s.to_string()).collect()));
        if now != self.lookout.derived {
            self.lookout.derivations += 1;
            let mut cards = Vec::new();
            for (s, id) in &live {
                cards.extend(open_cards(s, &m.lives[*id].transcript, true, |c| answered(&m.memos, s, c)));
            }
            for s in &cached {
                if let Some(c) = self.tab_cache.peek(*s) {
                    cards.extend(open_cards(s, &c.transcript, false, |c| answered(&m.memos, s, c)));
                }
            }
            let held: HashSet<String> = held.iter().map(|s| s.to_string()).collect();
            self.lookout.derived = now;
            self.model.arrivals.keep(|s| held.contains(s));
            if self.model.presence.set(cards) {
                self.model.view_dirty = true;
            }
            let p = &self.model.presence;
            self.lookout.notes.reconcile(|k| p.holds(k), cx);
        }
        // What came live since: told, unless nobody's away from it.
        let came = self.model.arrivals.take();
        if came.is_empty() {
            return;
        }
        let shown = self.shown_session();
        for k in came {
            let Some(c) = self.model.presence.cards().iter().find(|c| c.live && c.is(&k)).cloned() else { continue };
            let watched = self.window_active && shown.as_deref() == Some(c.session.as_str());
            if !self.app_settings.quiet && !watched {
                self.post_card(&c, cx);
            }
        }
    }

    /// The session on screen in the focused pane (not a view, not a
    /// worker's): the one a card in it is seen in.
    fn shown_session(&self) -> Option<String> {
        let me = self.panes.focused;
        let (live, tabs) = if self.panes.cur.id == me {
            (self.panes.cur.live, &self.model.tabs)
        } else {
            let p = self.panes.get(me)?;
            (p.live, &p.parked.tabs)
        };
        let l = &self.model.lives[live];
        if tabs.view().is_some() || l.watch.focus().is_some() {
            return None;
        }
        l.active.as_ref().map(|a| a.session_id.clone())
    }

    /// What a card's session is called where it isn't in front: its listed
    /// name or title, else its attachment's title, else its tab's.
    fn who(&self, s: &str) -> String {
        let listed = self.model.sessions.iter().any(|m| m.id == s);
        match self.model.lives.on(s).and_then(|l| l.transcript.title()).filter(|t| !listed && !t.is_empty()) {
            Some(t) => t.to_string(),
            None => self.session_label(s),
        }
    }

    /// The OS told of wait card `c` (F12), its text through the
    /// notification's boundary ([`crate::notify::note`]).
    pub(crate) fn post_card(&mut self, c: &Open, cx: &mut Context<Self>) {
        let n = crate::notify::note(&c.key(), &self.who(&c.session), &c.question);
        log::info!(target: "synaps_desktop::card", "notify {}: {} waits on {}", n.tag, n.title, c.tool_id);
        self.lookout.notes.post(c.key(), n, cx);
    }

    /// A click on notification `tag`: whether it is this window's. Its
    /// card is checked as it is now, not as the presence last said
    /// ([`AppView::still_open`]: a press or a closed tab since need no
    /// frame to count). Still waiting, the window shows it and comes up,
    /// and lands on it as Ctrl+Shift+J does; else (answered, on its way,
    /// closed, its tab closed) it is taken down and nothing moves: no
    /// window raised, no pane, tab or scroll changed.
    pub(crate) fn notification_clicked(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(k) = self.lookout.notes.card(tag).cloned() else { return false };
        if !self.still_open(&k) {
            log::info!(target: "synaps_desktop::card", "notification {tag}: its card no longer waits");
            self.lookout.notes.retire(tag, cx);
            return true;
        }
        // The jump checks that same binding; raised only once it has
        // somewhere to go.
        if self.go_to_card(k, window, cx) {
            window.activate_window();
        }
        true
    }

    /// Ctrl+Shift+J: the first waiting card ([`Presence::listed`]'s order)
    /// that still waits now ([`AppView::still_open`]), the presence first
    /// derived again from what the window holds now. Whether there was one.
    pub(crate) fn jump_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.sync_presence(cx);
        let Some(key) = self.model.presence.listed().map(Open::key).find(|k| self.still_open(k)) else { return false };
        self.go_to_card(key, window, cx)
    }

    /// Whether binding `k` waits now, read where it lives, never from the
    /// presence (derived after a batch or as a frame begins: a press, an
    /// answer sent or a tab closed since isn't in it yet): its session
    /// still in a tab here (any pane's); its card open in that session's
    /// fold (its attachment's, else its tab's cache, as last seen), bound
    /// as it was; and no answer of this desk's on its way or in.
    pub(crate) fn still_open(&self, k: &Key) -> bool {
        let m = &self.model;
        let held = m
            .tabs
            .iter()
            .chain(self.panes.rest.iter().flat_map(|p| p.parked.tabs.iter()))
            .any(|t| t.kind == TabKind::Session && t.session.as_deref() == Some(k.session.as_str()));
        let open = |t: &Transcript| {
            t.cards().card(&k.tool_id).is_some_and(|c| {
                c.state == State::Open && c.id.as_ref() == Some(&k.card) && c.doc.is_some() && !answered(&m.memos, &k.session, c)
            })
        };
        held && match m.lives.on(&k.session) {
            Some(l) => open(&l.transcript),
            None => self.tab_cache.peek(k.session.as_str()).is_some_and(|c| open(&c.transcript)),
        }
    }

    /// Binding `key`, an open card here, on screen: its pane focused
    /// (another pane's), its tab shown (a background one, out of a view or
    /// a worker's); then its row in view once built and the card confirmed
    /// ([`AppView::land_card`]). `key` is the binding the jump showed (the
    /// WAITING row's, the dot's, as painted; Ctrl+Shift+J's, checked as
    /// it presses), never looked up again: its call bound since to
    /// another card is another card, never this jump's. Checked first,
    /// before anything moves: exactly that binding must still wait now
    /// ([`AppView::still_open`]); else nothing moves. Never opens or
    /// attaches a session the window no longer holds. Whether it went.
    pub(crate) fn go_to_card(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let session = key.session.clone();
        if !self.still_open(&key) {
            log::info!(target: "synaps_desktop::card", "card {}: no longer waits: no jump", key.card.as_str());
            return false;
        }
        let here = self.model.lives[self.panes.cur.live].is_active(&session) && self.panes.cur.id == self.panes.focused;
        if !here && !self.focus_session(&session) {
            match self.model.tabs.find(&session) {
                Some(ix) => self.focus_tab(ix),
                None => return false,
            }
        }
        if self.palette.is_some() {
            self.close_palette(window, cx);
        }
        let l = &self.model.lives[self.panes.cur.live];
        if l.watch.focus().is_some() && l.is_active(&session) {
            self.leave_worker(false, cx);
        }
        self.lookout.land = Some(Land { key, at: Instant::now() });
        cx.notify();
        true
    }

    /// A jump's landing, in the pane being drawn ([`AppView::pane_sync`]):
    /// once the focused pane shows its session, the exact binding is
    /// checked (open in its fold, bound as it was, no answer of this
    /// desk's on its way); then its row comes into view (as Ctrl+J brings
    /// the authorize card's) and keyboard entry takes over
    /// ([`AppView::card_landed`]). Dropped when its settled history says
    /// the card no longer waits (answered, closed, replaced, compacted
    /// away), or after [`LAND_WAIT`].
    pub(crate) fn land_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.lookout.land.clone() else { return };
        if t.at.elapsed() > LAND_WAIT {
            self.lookout.land = None;
            return;
        }
        let l = &self.model.lives[self.panes.cur.live];
        if self.panes.cur.id != self.panes.focused
            || !l.is_active(&t.key.session)
            || self.model.tabs.view().is_some()
            || l.watch.focus().is_some()
        {
            return;
        }
        let open = l.transcript.cards().card(&t.key.tool_id).is_some_and(|c| {
            c.state == State::Open && c.id.as_ref() == Some(&t.key.card) && !answered(&self.model.memos, &t.key.session, c)
        });
        if !open {
            if l.transcript.history_settled() {
                log::info!(target: "synaps_desktop::card", "card {}: no longer waits: not landed", t.key.card.as_str());
                self.lookout.land = None;
            }
            return;
        }
        let items = l.transcript.items();
        let ix = self.panes.cur.disp.iter().rposition(|d| {
            d.kind == Kind::Ask && items.get(d.item).and_then(|i| i.tool.as_ref()).is_some_and(|tc| tc.tool_id == t.key.tool_id)
        });
        let Some(ix) = ix else { return };
        self.lookout.land = None;
        // The user's jump: it leaves the end.
        self.panes.cur.scroller.touch();
        self.panes.cur.list.scroll_to_reveal_item(ix);
        self.card_landed(t.key, window, cx);
        cx.notify();
    }

    /// **The D2b seam** (Shady #83 P2-7). A jump (Ctrl+Shift+J, the
    /// palette's WAITING, a notification, the tab dot, the top bar's JUMP)
    /// landed on binding `k`, checked open, its row in view: that card is
    /// entered ([`AppView::enter_card`]: the binding checked again, its
    /// card mode with its pane's batch, its keys live from its first
    /// paint), so the next key answers it. Never a prompt (§5.1). Once this
    /// frame is drawn (a landing is read as a pane's frame begins): the
    /// entry focuses and asks for the frame that paints it, as a key's
    /// would. Recorded either way; a binding no longer open there is not
    /// entered, and the keys stay where they were.
    ///
    /// The window not active (a notification's click asked for it to come
    /// up): entered once it is ([`AppView::landed_up`]). Its activation is
    /// on its way on every platform (`gpui-pre-linux 0.3.12`
    /// `x11/window.rs:1530`, a request to the window manager;
    /// `wayland/window.rs:1867`, a token the compositor may refuse), and an
    /// entry in a window not active is left at its next frame
    /// ([`AppView::sync_ask`]).
    pub(crate) fn card_landed(&mut self, k: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.lookout.landed = Some(k.clone());
        if !window.is_window_active() {
            self.lookout.entering = Some(Land { key: k, at: Instant::now() });
            return;
        }
        self.lookout.entering = None;
        self.enter_landed(k, window, cx);
    }

    /// The window came up: the card a landing left for it is entered, as
    /// [`AppView::card_landed`] would have (the binding checked again
    /// there). One older than [`LAND_WAIT`] is dropped: the window came up
    /// for something else.
    pub(crate) fn landed_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !window.is_window_active() {
            return;
        }
        let Some(t) = self.lookout.entering.take() else { return };
        if t.at.elapsed() <= LAND_WAIT {
            self.enter_landed(t.key, window, cx);
        }
    }

    /// Binding `k` entered once this frame is drawn ([`AppView::enter_card`]).
    fn enter_landed(&mut self, k: Key, window: &mut Window, cx: &mut Context<Self>) {
        let binding = crate::ask_keys::Binding { tool_id: k.tool_id, card: k.card };
        cx.defer_in(window, move |this, window, cx| {
            this.enter_card(&k.session, &binding, window, cx);
        });
    }

    /// Every text field of the window, one list: each pane's composer and
    /// secret field, the overlays' (find, palette, rename, colour, the
    /// settings' find and value) and every card's note, line and long
    /// fields (D2a's drafts, any pane's). What a global chord asks
    /// ([`AppView::composing_now`]).
    pub(crate) fn text_fields(&self) -> (Vec<&Entity<InputState>>, Vec<&Entity<TextareaState>>) {
        let mut lines =
            vec![&self.find_input, &self.palette_input, &self.name_input, &self.hex_input, &self.settings_find, &self.settings_input];
        let mut areas = Vec::new();
        for p in self.panes.iter() {
            areas.push(&p.composer);
            lines.push(&p.secret_input);
        }
        for d in self.card_drafts.values() {
            lines.extend(d.note.iter().chain(d.lines.values()));
            areas.extend(d.longs.values());
        }
        (lines, areas)
    }

    /// Whether the text field with the keys is composing (an IME's marked
    /// text), whichever of [`AppView::text_fields`] it is. The window's one
    /// rule for "an IME composition owns every key" (§6.1): Ctrl+Shift+J
    /// and Ctrl+J leave the keys to the IME then, and an entered agent
    /// card's keys in its fields act on nothing.
    pub(crate) fn composing_now(&self, window: &mut Window, cx: &mut App) -> bool {
        fn marked<T: EntityInputHandler + Focusable>(e: &Entity<T>, window: &mut Window, cx: &mut App) -> bool {
            e.read(cx).focus_handle(cx).is_focused(window) && e.update(cx, |s, cx| s.marked_text_range(window, cx)).is_some()
        }
        let (lines, areas) = self.text_fields();
        lines.into_iter().any(|e| marked(e, window, cx)) || areas.into_iter().any(|e| marked(e, window, cx))
    }

    /// A session tab's dot (§6.1), still, 6 px: filled in the needs-you
    /// mark while one of its live cards waits, in ASKED's colour while one
    /// is open ([`dot_colors`]); hollow and dimmed for a card as last seen
    /// (a background tab's: unverified); none else. A click goes to that
    /// card (F10's first click); a last-seen one is checked as its tab
    /// attaches.
    pub(crate) fn tab_dot(&self, tab: u64, session: Option<&str>, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let s = session?;
        let (n, seen) = (self.model.presence.of(s), self.model.presence.last_seen(s));
        let (wait, asked) = dot_colors(p);
        let (dot, sel, tip) = if n.wait + n.asked > 0 {
            let what = if n.wait > 0 { "waits on you" } else { "is open" };
            let fill = if n.wait > 0 { wait } else { asked };
            (div().size(px(6.)).bg(fill), "dot", format!("{} · a card {what} · click to go to it", plural(n.wait + n.asked)))
        } else if seen.wait + seen.asked > 0 {
            let what = if seen.wait > 0 { "waited on you" } else { "was open" };
            (
                div().size(px(6.)).border_1().border_color(p.dim_ch),
                "last-seen",
                format!("Last seen: {} · a card {what} · click to check", plural(seen.wait + seen.asked)),
            )
        } else {
            return None;
        };
        // The binding it shows, as painted: the one a click goes to.
        let card = self.model.presence.listed().find(|c| c.session == s).map(Open::key);
        Some(
            div()
                .id(("tab-dot", tab as usize))
                .debug_selector(move || format!("tab {tab} {sel}"))
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(dot)
                .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(k) = card.clone() {
                        this.go_to_card(k, window, cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// The palette's WAITING rows: each open card in [`Presence::listed`]'s
    /// order; its session, NEEDS YOU or ASKED (or "last seen") and how
    /// long it has waited as its meta.
    pub(crate) fn waiting_entries(&self, now: i64) -> Vec<crate::palette::Entry> {
        use crate::palette::{Act, Entry, Group, Lead};
        self.model
            .presence
            .listed()
            .enumerate()
            .map(|(i, c)| {
                let mut e = Entry::new(Group::Waiting, Act::Card(c.key()), c.question.clone());
                let what = match (c.live, c.wait) {
                    (true, true) => "needs you",
                    (true, false) => "asked",
                    (false, true) => "last seen · waited on you",
                    (false, false) => "last seen · asked",
                };
                let mut meta = vec![crate::notify::label(&self.who(&c.session)), what.into()];
                meta.extend(c.at.map(|t| crate::named::relative(&crate::demo::rfc3339(t / 1000), now)).filter(|a| !a.is_empty()));
                e.meta = meta.join(" · ").into();
                e.lead = match (c.live, c.wait) {
                    (true, true) => Lead::Session(crate::rail::State::NeedsYou),
                    (true, false) => Lead::Session(crate::rail::State::Asked),
                    _ => Lead::None,
                };
                e.rank = i as u32;
                e.key = (i == 0).then(|| if cfg!(target_os = "macos") { "⌃⇧J" } else { "Ctrl+Shift+J" }.into());
                e
            })
            .collect()
    }
}

/// `SYNAPS_DESK_PANES=d3`, `d3:palette` or `d3:notify` (D3's stills,
/// `--offline` with `SYNAPS_DEMO_CARDS=1`): the three card sessions opened
/// in tabs one after the other, then the first tab shown again, so each
/// is a background tab as last seen (its dot; the rail's NEEDS YOU and
/// ASKED); then the palette's GO TO (WAITING first), or the OS told of the
/// oldest wait card as a card that came would be.
pub(crate) async fn scene(view: &WeakEntity<AppView>, cx: &mut AsyncWindowContext, ex: &BackgroundExecutor, how: &str) {
    let act = |cx: &mut AsyncWindowContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
        let _ = view.update_in(cx, |v, window, cx| {
            f(v, window, cx);
            cx.notify();
        });
    };
    let sleep = |s: f64| ex.timer(Duration::from_secs_f64(s));
    sleep(1.0).await;
    for s in [crate::demo::ASKS, crate::demo::ASK_FORM, crate::demo::ASK_OPEN] {
        act(cx, &|v, _, _| {
            v.open_tab(crate::app::Open::Attach { session_id: s.into(), fallback: None, open: false }, crate::tabs::Place::New)
        });
        sleep(1.5).await;
    }
    act(cx, &|v, _, _| v.focus_tab(0));
    sleep(1.5).await;
    match how {
        "d3:palette" => act(cx, &|v, window, cx| v.open_palette(crate::palette::Mode::GoTo, window, cx)),
        "d3:notify" => act(cx, &|v, _, cx| {
            if let Some(c) = v.model.presence.next().cloned() {
                v.post_card(&c, cx);
            }
        }),
        _ => {}
    }
    eprintln!("D3 scene {how} done");
}

/// "1 card", "3 cards".
fn plural(n: usize) -> String {
    if n == 1 { "1 card".into() } else { format!("{n} cards") }
}

/// The windows a notification's click may be for: GPUI calls one
/// callback per process (the last registered), so it asks each window in
/// turn whose tag it is.
#[derive(Default)]
struct Clickers(Vec<(AnyWindowHandle, WeakEntity<AppView>)>);

impl Global for Clickers {}

/// The notifications' clicks come to `view`'s window: GPUI's (macOS,
/// Windows), through the one process-wide callback; its own transport's
/// (Linux), from its channel.
pub(crate) fn listen(view: &mut AppView, window: &Window, cx: &mut Context<AppView>) {
    let first = !cx.has_global::<Clickers>();
    let me = cx.weak_entity();
    cx.default_global::<Clickers>().0.push((window.window_handle(), me));
    if first {
        cx.on_system_notification_response(|r, cx| {
            let windows = cx.global::<Clickers>().0.clone();
            for (handle, view) in windows {
                let Some(view) = view.upgrade() else { continue };
                let mine = handle.update(cx, |_, window, cx| view.update(cx, |v, cx| v.notification_clicked(&r.tag, window, cx)));
                if mine.unwrap_or(false) {
                    break;
                }
            }
            cx.global_mut::<Clickers>().0.retain(|(_, v)| v.upgrade().is_some());
        });
    }
    if let Some(rx) = view.lookout.notes.take_clicks() {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(tag) = rx.recv().await {
                if this.update_in(cx, |v, window, cx| v.notification_clicked(&tag, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

/// `view`'s window is going (released, or the app quitting): the
/// notifications it posted taken down with it, nothing it queued shown
/// after (Shady #83 R2-3), and its place among the click routes dropped.
pub(crate) fn window_gone(view: &mut AppView, cx: &mut App) -> crate::notify::Gone {
    if cx.has_global::<Clickers>() {
        cx.global_mut::<Clickers>().0.retain(|(_, v)| v.upgrade().is_some());
    }
    view.lookout.notes.teardown(cx)
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
