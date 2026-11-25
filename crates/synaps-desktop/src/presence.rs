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
//! - every attachment (the session each pane shows), live;
//! - every other session open in a tab, as last seen: the tab cache's
//!   transcript, the one its tab shows again when it is clicked (until the
//!   attach brings it up to date). A card asked after that tab was left is
//!   not known until it is shown again, and a session only in the rail is
//!   never read: the daemon says nothing of its cards (F11, §8 #1).
//!
//! **What reads it:** the rail's NEEDS YOU and ASKED ([`crate::model::Model::fact`]),
//! the tab dot ([`AppView::tab_dot`]), the palette's WAITING group,
//! Ctrl+Shift+J ([`AppView::jump_waiting`]) and the OS notification
//! ([`Told`]). Nothing here moves: no frame is asked for, a dot or a badge
//! is drawn still (0 frames idle at every motion level).
//!
//! **The OS notification** (F12) is GPUI's own (`App::show_system_notification`:
//! XDG notifications on Linux, `UNUserNotificationCenter` on macOS, toasts
//! on Windows): no crate of ours, nothing on the frame path. One per wait
//! card, for a card that came while this window held its session and
//! either the window was not active or its session not the one on screen;
//! never for one an attach brought (it was there before). `notify.cards`
//! (`false` or `"off"`) in the settings file turns them off. A click on one
//! brings the window up and lands on the card, as Ctrl+Shift+J does.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use gpui_kit::*;
use synaps_client::Transcript;
use synaps_client::cards::{CardRow, State};

use crate::app::AppView;
use crate::named::RowAction;
use crate::tabs::Kind as TabKind;
use crate::theme::Palette;
use crate::turns::Kind;

/// An open card: asked, bound, and nobody's answer yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Open {
    pub session: String,
    /// Its call's tool id: its row.
    pub tool_id: String,
    /// The agent ended its turn on it (§1.3).
    pub wait: bool,
    /// Its question, one line (sanitized: [`crate::ask::question_line`]).
    pub question: String,
    /// When it was asked (its row's time, seconds), if the row says.
    pub at: Option<i64>,
    /// Read from an attachment (else as last seen: a background tab's).
    pub live: bool,
    /// The tie-breaks: its source's order, then its place in its fold.
    order: (usize, usize),
}

impl Open {
    fn age(&self) -> (i64, (usize, usize)) {
        (self.at.unwrap_or(i64::MAX), self.order)
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
        cards.sort_by_key(Open::age);
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

    pub fn of(&self, session: &str) -> Count {
        let mut n = Count::default();
        for c in self.cards.iter().filter(|c| c.session == session) {
            if c.wait {
                n.wait += 1;
            } else {
                n.asked += 1;
            }
        }
        n
    }

    /// Ctrl+Shift+J's: the oldest wait card, else the oldest open one.
    pub fn next(&self) -> Option<&Open> {
        self.cards.iter().find(|c| c.wait).or_else(|| self.cards.first())
    }

    /// The palette's WAITING group: wait cards, then the rest, each oldest
    /// first.
    pub fn listed(&self) -> impl Iterator<Item = &Open> {
        self.cards.iter().filter(|c| c.wait).chain(self.cards.iter().filter(|c| !c.wait))
    }
}

/// `session`'s open cards in `t`, the fold its rows draw: bound and not
/// answered ([`State::Open`]), less those `answered` (this desk's answer
/// on its way or in). `order`: the source's place among the window's.
pub(crate) fn open_cards(
    session: &str,
    t: &Transcript,
    live: bool,
    order: usize,
    answered: impl Fn(&cards_spec::fold::Card) -> bool,
) -> Vec<Open> {
    let mut out = Vec::new();
    for c in t.cards().cards().iter().filter(|c| c.state == State::Open && c.id.is_some() && !answered(c)) {
        let Some(doc) = c.doc.as_ref() else { continue };
        let row = t
            .items()
            .iter()
            .rev()
            .find(|i| matches!(i.card.as_deref(), Some(CardRow::Ask(_))) && i.tool.as_ref().is_some_and(|tc| tc.tool_id == c.tool_id));
        out.push(Open {
            session: session.to_string(),
            tool_id: c.tool_id.clone(),
            wait: c.wait,
            question: crate::ask::question_line(&doc.question),
            at: row.and_then(|r| r.at.as_deref()).and_then(crate::named::parse_rfc3339),
            live,
            order: (order, c.at),
        });
    }
    out
}

/// What the OS has been told (F12): each wait card once.
#[derive(Debug, Default)]
pub(crate) struct Told {
    /// Every wait card seen so far, told or not: never told again.
    seen: HashSet<(String, String)>,
    /// The sessions attached at the last look: a wait card new in one of
    /// them came while this window held it.
    held: HashSet<String>,
}

impl Told {
    /// The wait cards to tell the OS of now, of `cards` (the window's):
    /// new since the last look, in a session attached then and now (one an
    /// attach brought was there before), its session not the one on screen
    /// in an active window (`shown`), notifications `on`. Every wait card
    /// is seen from now on, told or not.
    pub fn fresh(&mut self, cards: &[Open], attached: &[String], shown: Option<&str>, active: bool, on: bool) -> Vec<Open> {
        let mut out = Vec::new();
        for c in cards.iter().filter(|c| c.wait && c.live) {
            if !self.seen.insert((c.session.clone(), c.tool_id.clone())) {
                continue;
            }
            let came = self.held.contains(&c.session);
            let watched = active && shown == Some(c.session.as_str());
            if on && came && !watched {
                out.push(c.clone());
            }
        }
        // Only what is still open: a card gone and back (a rollback) is new.
        self.seen.retain(|(s, t)| cards.iter().any(|c| c.wait && &c.session == s && &c.tool_id == t));
        self.held = attached.iter().cloned().collect();
        out
    }
}

/// A jump's landing: the card to bring into view once its session is on
/// screen and its row is built ([`AppView::land_card`]).
#[derive(Clone, Debug)]
pub(crate) struct Land {
    pub session: String,
    pub tool_id: String,
    pub at: Instant,
}

/// A landing that hasn't happened in this long (the attach failed, the
/// card went) is dropped: it never jumps the view later.
const LAND_WAIT: Duration = Duration::from_secs(10);

/// The window's lookout for open cards: the landing on its way, what the
/// OS was told, and each notification's card by its tag. (D2b's
/// `crate::waiting` is Ctrl+J's order; [`Presence::next`] and
/// [`Presence::listed`] are that order over every open session.)
#[derive(Debug, Default)]
pub(crate) struct Lookout {
    pub land: Option<Land>,
    pub told: Told,
    tags: HashMap<String, (String, String)>,
    /// The last card a jump landed on (its session and tool id): where
    /// keyboard entry takes over ([`AppView::enter_card`]).
    pub landed: Option<(String, String)>,
}

/// A notification's tag: short (Windows keeps 64 characters of one) and
/// the same for the same card.
fn tag_of(session: &str, tool_id: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (session, tool_id).hash(&mut h);
    format!("synaps-card-{:016x}", h.finish())
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

impl AppView {
    /// The presence as the window holds it now: the model's, then the
    /// notifications for wait cards new since the last look. After every
    /// batch of events, and as a frame begins (tabs closed, a pane's
    /// switch); cheap when nothing moved (the folds' cards, no rows read).
    pub(crate) fn sync_presence(&mut self, cx: &mut Context<Self>) {
        let m = &self.model;
        let answered = |s: &str, c: &cards_spec::fold::Card| {
            m.memos.entry(s, c).is_some_and(|e| {
                use cards_spec::memo::State as M;
                !matches!(e.state, M::Unconfirmed | M::NotSent)
            })
        };
        let mut cards = Vec::new();
        let mut attached: Vec<String> = Vec::new();
        for (i, id) in m.lives.ids().into_iter().enumerate() {
            let l = &m.lives[id];
            let Some(s) = l.active.as_ref().map(|a| a.session_id.clone()) else { continue };
            if attached.contains(&s) {
                continue;
            }
            cards.extend(open_cards(&s, &l.transcript, true, i, |c| answered(&s, c)));
            attached.push(s);
        }
        // The other sessions open in a tab (any pane's), as last seen.
        let tabs = m.tabs.iter().chain(self.panes.rest.iter().flat_map(|p| p.parked.tabs.iter()));
        let mut cached: Vec<&str> = Vec::new();
        for s in tabs.filter(|t| t.kind == TabKind::Session).filter_map(|t| t.session.as_deref()) {
            if attached.iter().any(|a| a == s) || cached.contains(&s) {
                continue;
            }
            cached.push(s);
            if let Some(c) = self.tab_cache.peek(s) {
                cards.extend(open_cards(s, &c.transcript, false, 1000 + cached.len(), |c| answered(s, c)));
            }
        }
        if self.model.presence.set(cards) {
            self.model.view_dirty = true;
        }
        let shown = self.shown_session();
        let on = !self.app_settings.quiet;
        let fresh = self.lookout.told.fresh(self.model.presence.cards(), &attached, shown.as_deref(), self.window_active, on);
        for c in fresh {
            self.notify_card(&c, cx);
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

    /// The OS is told of wait card `c` (F12).
    fn notify_card(&mut self, c: &Open, cx: &mut Context<Self>) {
        let tag = tag_of(&c.session, &c.tool_id);
        self.lookout.tags.insert(tag.clone(), (c.session.clone(), c.tool_id.clone()));
        log::info!(target: "synaps_desktop::card", "notify {tag}: {} waits on {}", self.who(&c.session), c.tool_id);
        cx.show_system_notification(SystemNotification {
            tag: tag.into(),
            title: format!("Needs you · {}", self.who(&c.session)).into(),
            body: c.question.clone().into(),
            actions: Vec::new(),
        });
    }

    /// A click on a notification of this window's (any other tag is
    /// another's, or gone): the window up, and the card, as Ctrl+Shift+J.
    pub(crate) fn notification_clicked(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((session, tool_id)) = self.lookout.tags.get(tag).cloned() else { return };
        window.activate_window();
        self.go_to_card(session, tool_id, window, cx);
    }

    /// Ctrl+Shift+J: the oldest waiting card in any open session (else the
    /// oldest open one). Whether there was one.
    pub(crate) fn jump_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(c) = self.model.presence.next().map(|c| (c.session.clone(), c.tool_id.clone())) else { return false };
        self.go_to_card(c.0, c.1, window, cx);
        true
    }

    /// Card `tool_id` of `session` on screen: its pane focused (another
    /// pane's), its tab shown (a background one, out of a view or a
    /// worker's), or it opens as the rail's click opens it; then its row
    /// in view once built ([`AppView::land_card`]).
    pub(crate) fn go_to_card(&mut self, session: String, tool_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
        }
        if !self.focus_session(&session) {
            match self.model.tabs.find(&session) {
                Some(ix) => self.focus_tab(ix),
                None => {
                    for fx in self.model.open_row(&RowAction::Attach(session.clone())) {
                        self.effect(fx);
                    }
                }
            }
        }
        let l = &self.model.lives[self.panes.cur.live];
        if l.watch.focus().is_some() && l.is_active(&session) {
            self.leave_worker(false, cx);
        }
        self.lookout.land = Some(Land { session, tool_id, at: Instant::now() });
        cx.notify();
    }

    /// A jump's landing, in the pane being drawn ([`AppView::pane_sync`]):
    /// once the focused pane shows its session and the card's row is built,
    /// the row comes into view (as Ctrl+J brings the authorize card's), and
    /// keyboard entry takes over ([`AppView::enter_card`]). Dropped when the
    /// card is no longer open there, or after [`LAND_WAIT`].
    pub(crate) fn land_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.lookout.land.clone() else { return };
        if t.at.elapsed() > LAND_WAIT {
            self.lookout.land = None;
            return;
        }
        let l = &self.model.lives[self.panes.cur.live];
        if self.panes.cur.id != self.panes.focused
            || !l.is_active(&t.session)
            || self.model.tabs.view().is_some()
            || l.watch.focus().is_some()
        {
            return;
        }
        let items = l.transcript.items();
        let ix = self.panes.cur.disp.iter().rposition(|d| {
            d.kind == Kind::Ask && items.get(d.item).and_then(|i| i.tool.as_ref()).is_some_and(|tc| tc.tool_id == t.tool_id)
        });
        let Some(ix) = ix else {
            // Its history says it is no longer open here: nothing to land on.
            if l.transcript.history_settled() && !l.transcript.cards().card(&t.tool_id).is_some_and(|c| c.state == State::Open) {
                self.lookout.land = None;
            }
            return;
        };
        self.lookout.land = None;
        // The user's jump: it leaves the end.
        self.panes.cur.scroller.touch();
        self.panes.cur.list.scroll_to_reveal_item(ix);
        self.enter_card(&t.session, &t.tool_id, window, cx);
        cx.notify();
    }

    /// **The D2b seam.** A jump (Ctrl+Shift+J, the palette's WAITING, a
    /// notification, the tab dot) landed on card `tool_id`, its row in
    /// view. Keyboard entry into a card (its digits, Enter, Esc) is D2b's:
    /// when it lands, this hands the card to its Ctrl+J entry, so the next
    /// key answers it (F10's second key). Until then a jump lands as Ctrl+J
    /// lands the authorize card's row, and the keys stay where they were.
    pub(crate) fn enter_card(&mut self, session: &str, tool_id: &str, _window: &mut Window, _cx: &mut Context<Self>) {
        self.lookout.landed = Some((session.to_string(), tool_id.to_string()));
    }

    /// A session tab's dot (§6.1): a still 6 px square in the needs-you
    /// mark while one of its cards waits, the accent's while one is open;
    /// none else. A click goes to that card (F10's first click).
    pub(crate) fn tab_dot(&self, tab: u64, session: Option<&str>, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let s = session?;
        let n = self.model.presence.of(s);
        if n.wait + n.asked == 0 {
            return None;
        }
        let (color, what) = if n.wait > 0 { (p.warn_mark_ch, "waits on you") } else { (p.focus_ch, "is open") };
        let card = self.model.presence.cards().iter().find(|c| c.session == s && c.wait == (n.wait > 0)).map(|c| c.tool_id.clone());
        let tip = format!("{} · a card {what} · click to go to it", plural(n.wait + n.asked));
        let session = s.to_string();
        Some(
            div()
                .id(("tab-dot", tab as usize))
                .debug_selector(move || format!("tab {tab} dot"))
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(div().size(px(6.)).bg(color))
                .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(tool_id) = card.clone() {
                        this.go_to_card(session.clone(), tool_id, window, cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// The palette's WAITING rows: each open card, wait cards first, oldest
    /// first; its session and how long it has waited as its meta.
    pub(crate) fn waiting_entries(&self, now: i64) -> Vec<crate::palette::Entry> {
        use crate::palette::{Act, Entry, Group, Lead};
        self.model
            .presence
            .listed()
            .enumerate()
            .map(|(i, c)| {
                let mut e = Entry::new(Group::Waiting, Act::Card(c.session.clone(), c.tool_id.clone()), c.question.clone());
                let mut meta = vec![self.who(&c.session), if c.wait { "needs you".into() } else { "asked".into() }];
                meta.extend(c.at.map(|t| crate::named::relative(&crate::demo::rfc3339(t), now)).filter(|a| !a.is_empty()));
                e.meta = meta.join(" · ").into();
                e.lead = Lead::Session(if c.wait { crate::rail::State::NeedsYou } else { crate::rail::State::Asked });
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
                v.notify_card(&c, cx);
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

/// The notifications' clicks come to this window (one per process: the
/// last registration is the one the platform calls).
pub(crate) fn listen(window: &Window, cx: &mut Context<AppView>) {
    let handle = window.window_handle();
    let me = cx.weak_entity();
    cx.on_system_notification_response(move |r, cx| {
        let Some(view) = me.upgrade() else { return };
        let _ = handle.update(cx, |_, window, cx| view.update(cx, |v, cx| v.notification_clicked(&r.tag, window, cx)));
    });
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
