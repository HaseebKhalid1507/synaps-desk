//! The tab strip over the centre pane: one tab per open session, its state
//! as the rail draws it (glyph, word, the needs-you bar), the visible one
//! raised with the "you" bar (its word is the composer foot's: it keeps
//! the glyph and gives the title the room). A worker's tab ([`Kind::Worker`]) says
//! `sleeper · <its session>` with the worker's state mark, its session in
//! its tooltip; right-click opens the worker's menu ([`crate::wtabs`]).
//! A drag drops where it is let go, and tabs that do not fit scroll by the
//! arrows at the ends (never a second row).
//!
//! Motion (fluid.md §3.5, motion.md §6.4, Full only): the visible tab's
//! `raised` fill and its 2 px `you_bar` are one quad pair that springs its
//! x and width to a newly visible tab (`snap`); a tab opening springs its
//! width 0 → its own and one closing back to 0, laid out at full width and
//! clipped, the tabs beside it riding the layout; labels ease to `ink`
//! over `quick` and back over `hover_out`; a NEEDS YOU bar draws left to
//! right over `base`. At rest each tab draws its own fill and bar, as the
//! static app does.

use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, display, mono};
use crate::fonts;
use crate::fonts::TextPx as _;
use crate::glyph::{self, Glyph, Ground};
use crate::rail::State;
use crate::subagents::Liveness;
use crate::tabs::{Kind, Place, Tab};
use crate::theme::Palette;

pub(crate) const TAB_H: f32 = 34.;
/// A tab is never narrower or wider than this; under the minimum they
/// overflow.
pub(crate) const TAB_MIN: f32 = 132.;
pub(crate) const TAB_MAX: f32 = 236.;
/// The `+` (new session) and each overflow arrow.
const PLUS_W: f32 = 38.;
const ARROW_W: f32 = 46.;
/// Tabs this wide show the state's word beside the title (C O M P L E T E D
/// is 79.7 px at 12).
const WORD_MIN: f32 = 220.;

/// Spring keys (the strip's own, [`crate::motion::Motion::spring`]).
pub(crate) const IND_X: u64 = 0x7AB0 << 48;
const IND_W: u64 = 0x7AB1 << 48;
pub(crate) const OPEN_W: u64 = 0x7AB2 << 48;
const LABEL: u64 = 0x7AB3 << 48;
pub(crate) const TOUCH: u64 = 0x7AB4 << 48;
const CLOSE_W: u64 = 0x7AB5 << 48;

/// What the strip's motion remembers between frames.
#[derive(Default)]
pub(crate) struct TabFx {
    /// Every tab's id as of the last frame (none before the first).
    ids: Option<Vec<u64>>,
    /// The tabs drawn last frame, in order: id, width, label.
    drawn: Vec<(u64, f32, SharedString, bool)>,
    /// Tabs closing: the tab drawn before it, its width, its label.
    ghosts: Vec<Ghost>,
    /// The visible tab last frame.
    shown: Option<u64>,
}

struct Ghost {
    id: u64,
    after: Option<u64>,
    width: f32,
    label: SharedString,
    named: bool,
    /// Its width this frame.
    shown: f32,
}

/// A closing tab: its label, clipped to its width as it springs to 0.
fn ghost_tab(g: &Ghost, p: &Palette) -> AnyElement {
    div()
        .flex_none()
        .w(px(g.shown))
        .h_full()
        .overflow_hidden()
        .child(
            h_flex().w(px(g.width)).h_full().pl(px(34.)).pr(px(34.)).items_center().border_r_1().border_color(p.hair).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(g.named, |d| d.font_family(fonts::MONO).text_px(12.))
                    .when(!g.named, |d| d.text_px(13.))
                    .text_color(p.dim_ch)
                    .child(g.label.clone()),
            ),
        )
        .into_any_element()
}

/// A click's place: Ctrl (Cmd on macOS) opens in a new tab.
pub(crate) fn click_place(ev: &ClickEvent, cx: &App) -> Place {
    if ev.modifiers().secondary(cx) { Place::New } else { Place::Current }
}

/// How the strip lays out `n` tabs in `room` px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Strip {
    /// The first tab shown, how many, and each one's width.
    pub first: usize,
    pub shown: usize,
    pub width: f32,
    /// Not all fit: arrows at both ends.
    pub overflow: bool,
}

/// How many tabs fit in `room` px once it overflows (its arrows take room).
pub(crate) fn fits(n: usize, room: f32) -> usize {
    if n as f32 * TAB_MIN <= room {
        return n;
    }
    (((room - 2. * ARROW_W).max(TAB_MIN) / TAB_MIN).floor() as usize).clamp(1, n.max(1))
}

/// The strip for `n` tabs in `room` px. Overflowing, it keeps `first` (the
/// user's scroll), unless `follow` (the visible tab changed, or the room
/// for tabs did): then just enough to bring `active` in.
pub(crate) fn strip(n: usize, active: Option<usize>, room: f32, first: usize, follow: bool) -> Strip {
    if n == 0 {
        return Strip { first: 0, shown: 0, width: 0., overflow: false };
    }
    if n as f32 * TAB_MIN <= room {
        return Strip { first: 0, shown: n, width: (room / n as f32).min(TAB_MAX).floor(), overflow: false };
    }
    let room = (room - 2. * ARROW_W).max(TAB_MIN);
    let k = ((room / TAB_MIN).floor() as usize).clamp(1, n);
    let mut first = first.min(n - k);
    if follow && let Some(a) = active {
        if a < first {
            first = a;
        } else if a >= first + k {
            first = a + 1 - k;
        }
    }
    Strip { first, shown: k, width: (room / k as f32).floor(), overflow: true }
}

/// GPUI's drag value for a tab, and the chip it draws at the pointer.
#[derive(Clone)]
pub(crate) struct TabDrag {
    pub id: u64,
    /// Where it was when the drag began (which side a drop marks).
    pub ix: usize,
    label: SharedString,
    bg: Hsla,
    fg: Hsla,
    edge: Hsla,
}

impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(TAB_H - 6.))
            .max_w(px(TAB_MAX))
            .px(px(12.))
            .flex()
            .items_center()
            .bg(self.bg)
            .border_1()
            .border_color(self.edge)
            .text_px(crate::fonts::size::TITLE)
            .text_color(self.fg)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(self.label.clone())
    }
}

/// A tab's title (and whether it is a session's name, set in mono), built
/// when the lists or the tabs change, never per frame.
#[derive(Default)]
pub(crate) struct Labels {
    key: (u64, u64),
    labels: Vec<(SharedString, bool)>,
    /// The overflow arrows' texts, for the counts they show.
    arrows: ((usize, usize), (SharedString, SharedString)),
}

impl AppView {
    /// What a tab shows: the session's name, else its title, else "New
    /// session" or its id (the rail's rules); not listed: what the tab last
    /// knew.
    fn tab_label(&self, t: &Tab) -> (SharedString, bool) {
        if let (Kind::Worker, Some(w)) = (t.kind, &t.worker) {
            let (session, _) = self.session_tab_label(t);
            return (format!("{} · {session}", w.name).into(), false);
        }
        if t.kind != Kind::Session {
            return (t.kind.label().into(), false);
        }
        self.session_tab_label(t)
    }

    /// What a session tab (or a worker tab, of its session) calls it.
    fn session_tab_label(&self, t: &Tab) -> (SharedString, bool) {
        let meta = t.session.as_deref().and_then(|s| self.model.sessions.iter().find(|m| m.id == s));
        match meta {
            Some(m) => match (m.name.as_deref().filter(|n| !n.is_empty()), m.title.as_deref().filter(|t| !t.is_empty())) {
                (Some(n), _) => (n.to_string().into(), true),
                (None, Some(title)) => (title.to_string().into(), false),
                (None, None) if m.messages == 0 => (crate::app::NEW_SESSION.into(), false),
                (None, None) => (m.id.clone().into(), true),
            },
            None => (t.label.clone().into(), false),
        }
    }

    /// A tab's state, first match wins as on the rail. Its session listed:
    /// the rail's state (the attached one from its own transcript). Not
    /// listed: OPENING on its way, else from the transcript if attached
    /// (a create the poll has not caught up with), else gone (no glyph).
    /// A worker tab: NEEDS YOU while its session waits on a prompt (its
    /// card is never shown over the worker), else nothing of the session's.
    pub(crate) fn tab_state(&self, t: &Tab) -> State {
        match (t.kind, t.session.as_deref()) {
            (Kind::Session, _) => self.session_state(t.id, t.session.as_deref()),
            (Kind::Worker, Some(s)) if self.session_state(t.id, Some(s)) == State::NeedsYou => State::NeedsYou,
            _ => State::Idle,
        }
    }

    /// The state of session `s` as tab `id` shows it ([`AppView::tab_state`]).
    fn session_state(&self, id: u64, s: Option<&str>) -> State {
        let Some(s) = s else { return State::Opening };
        // Restored, its session not listed yet (a daemon still starting).
        if self.model.tabs.restoring(id) {
            return State::Opening;
        }
        if let Some(m) = self.model.sessions.iter().find(|m| m.id == s) {
            return self.model.state(m);
        }
        if self.model.is_active(s) {
            let tr = &self.model.transcript;
            if !tr.pending_prompts().is_empty() {
                State::NeedsYou
            } else if tr.busy() {
                State::Running
            } else if self.model.active_workers() > 0 {
                State::Workers
            } else {
                State::Idle
            }
        } else if self.model.is_wanted(s) {
            State::Opening
        } else {
            State::Saved
        }
    }

    /// The strip, `pane` px wide. Keeps the visible tab in view when it
    /// changes (an arrow click scrolls without changing it).
    pub(crate) fn render_tabs(&mut self, pane: f32, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let key = (self.model.rail_gen, self.model.tabs.generation);
        if self.panes.cur.tab_labels.key != key || self.panes.cur.tab_labels.labels.len() != self.model.tabs.len() {
            let labels = self.model.tabs.iter().map(|t| self.tab_label(t)).collect();
            self.panes.cur.tab_labels = Labels { key, labels, arrows: std::mem::take(&mut self.panes.cur.tab_labels.arrows) };
        }
        let tabs = &self.model.tabs;
        let n = tabs.len();
        let active = tabs.active();
        let visible = tabs.visible().map(|t| t.id);
        let new = self.client.is_some();
        let room = pane - if new { PLUS_W } else { 0. };
        // A resize or a panel drag that changes how many tabs fit brings
        // the visible one back in; arrow scrolls change neither.
        let fit = fits(n, room);
        let follow = visible != self.panes.cur.tab_follow || fit != self.panes.cur.tab_room;
        let lay = strip(n, active, room, self.panes.cur.tab_first, follow);
        self.panes.cur.tab_first = lay.first;
        self.panes.cur.tab_follow = visible;
        self.panes.cur.tab_room = fit;
        // Opens and closes (fluid.md §3.5): a tab not there last frame
        // springs its width from 0; one gone leaves a ghost springing to 0.
        let snap = crate::motion::SNAP_RESPONSE;
        let ids: Vec<u64> = tabs.iter().map(|t| t.id).collect();
        let full = self.fx.level() == crate::motion::Level::Full;
        let fresh: Vec<u64> = match &self.panes.cur.tab_fx.ids {
            Some(last) if full => ids.iter().filter(|id| !last.contains(id)).copied().collect(),
            _ => Vec::new(),
        };
        if full && self.panes.cur.tab_fx.ids.is_some() {
            for (k, (id, w, label, named)) in self.panes.cur.tab_fx.drawn.iter().enumerate() {
                if !ids.contains(id) && !self.panes.cur.tab_fx.ghosts.iter().any(|g| g.id == *id) {
                    let after = k.checked_sub(1).map(|j| self.panes.cur.tab_fx.drawn[j].0);
                    self.fx.spring_jump(self.panes.cur.id.salt(crate::motion::key(CLOSE_W, *id)), *w, 0., snap);
                    self.panes.cur.tab_fx.ghosts.push(Ghost { id: *id, after, width: *w, label: label.clone(), named: *named, shown: *w });
                }
            }
        } else {
            self.panes.cur.tab_fx.ghosts.clear();
        }
        self.panes.cur.tab_fx.ids = Some(ids);
        let fx = &self.fx;
        self.panes.cur.tab_fx.ghosts.retain_mut(|g| {
            let (w, moving) = fx.spring(self.panes.cur.id.salt(crate::motion::key(CLOSE_W, g.id)), Some(g.width), 0., snap);
            g.shown = w;
            moving && w >= 0.5
        });
        // The slots, in order: (a tab's index and width, opening) or a ghost.
        enum Slot {
            Tab(usize, f32, bool),
            Ghost(usize),
        }
        let ghosts = &self.panes.cur.tab_fx.ghosts;
        let mut slots = Vec::with_capacity(lay.shown + ghosts.len());
        let mut placed = vec![false; ghosts.len()];
        let mut ghosts_after = |after: Option<u64>, slots: &mut Vec<Slot>| {
            for (k, g) in ghosts.iter().enumerate() {
                if g.after == after && !placed[k] {
                    placed[k] = true;
                    slots.push(Slot::Ghost(k));
                }
            }
        };
        ghosts_after(None, &mut slots);
        for (ix, t) in tabs.iter().enumerate().skip(lay.first).take(lay.shown) {
            // A new tab's width springs from 0; after that (settled) a new
            // width is taken at once (a resize, a neighbour opening: fluid
            // §3.8 `follow`), as for every tab from launch.
            let key = self.panes.cur.id.salt(crate::motion::key(OPEN_W, t.id));
            let (w, opening) = if fresh.contains(&t.id) {
                fx.spring(key, Some(0.), lay.width, snap)
            } else if fx.spring_at(key).is_some() {
                fx.spring_held(key, lay.width, snap)
            } else {
                (lay.width, false)
            };
            slots.push(Slot::Tab(ix, w, opening));
            ghosts_after(Some(t.id), &mut slots);
        }
        slots.extend(placed.iter().enumerate().filter(|(_, p)| !**p).map(|(k, _)| Slot::Ghost(k)));
        // The indicator: a newly visible tab's fill and bar spring there
        // from where they were drawn (`snap`); at rest they follow the
        // layout at once (a resize, an arrow scroll). Its target is the
        // tab's final slot (every tab at its width, no ghosts), not where
        // the tab is this frame: a close's neighbour slides under a fill
        // that stays put, never chased out and back (finish gate 1).
        let mut x = if lay.overflow { ARROW_W } else { 0. };
        let mut goal_x = x;
        let (mut target, mut goal) = (None, None);
        for slot in &slots {
            match *slot {
                Slot::Tab(ix, w, _) => {
                    if active == Some(ix) {
                        (target, goal) = (Some((x, w)), Some((goal_x, lay.width)));
                    }
                    x += w;
                    goal_x += lay.width;
                }
                Slot::Ghost(k) => x += ghosts[k].shown,
            }
        }
        // The indicator is the selected tab's (its attach may still be on
        // its way: the strip shows it selected at once).
        let selected = active.and_then(|i| tabs.iter().nth(i)).map(|t| t.id);
        let moved = full && selected.is_some() && self.panes.cur.tab_fx.shown.is_some() && selected != self.panes.cur.tab_fx.shown;
        let mut quads = None;
        if let (Some((x, w)), Some((tx, tw))) = (goal, target) {
            let (ind_x, ind_w) = (self.panes.cur.id.salt(IND_X), self.panes.cur.id.salt(IND_W));
            if moved && let (Some(x0), Some(w0)) = (fx.spring_at(ind_x), fx.spring_at(ind_w)) {
                fx.spring_jump(ind_x, x0, x, snap);
                fx.spring_jump(ind_w, w0, w, snap);
            }
            let ((qx, mx), (qw, mw)) = (fx.spring_held(ind_x, x, snap), fx.spring_held(ind_w, w, snap));
            // Drawn as the quad while it moves, or while its tab is still
            // on its way to the slot under it.
            if mx || mw || (full && ((tx - x).abs() >= 0.5 || (tw - w).abs() >= 0.5)) {
                quads = Some((qx, qw));
            }
        }
        // The tracer's target: the new tab's top-left corner (§6.2); once
        // it lands, the tab's bar draws left to right over `quick` (the
        // undrawn part covered in the tab's own fill).
        let mut cover = None;
        if let Some(tr) = self.panes.cur.tracer.as_mut() {
            let mut x = if lay.overflow { ARROW_W } else { 0. };
            let mut at = None;
            for slot in &slots {
                match *slot {
                    Slot::Tab(ix, w, _) => {
                        if tabs.iter().nth(ix).is_some_and(|t| t.id == tr.tab) {
                            at = Some((x, w));
                        }
                        x += w;
                    }
                    Slot::Ghost(k) => x += ghosts[k].shown,
                }
            }
            let origin = self.panes.cur.strip_at.get();
            tr.to = at.map(|(x, _)| point(origin.x + px(x), origin.y));
            let k = fx.draw(Some(tr.at), crate::motion::BASE, crate::motion::QUICK);
            if at.is_none() || k >= 1. {
                self.panes.cur.tracer = None;
            } else if let Some((x, w)) = at {
                cover = Some(div().absolute().top_0().left(px(x + w * k)).w(px(w * (1. - k))).h(px(2.)).bg(p.raised));
            }
        }
        let mut row = h_flex().h_full().flex_none();
        let mut drawn = Vec::with_capacity(lay.shown);
        for slot in slots {
            let (ix, w, opening) = match slot {
                Slot::Ghost(k) => {
                    row = row.child(ghost_tab(&ghosts[k], p));
                    continue;
                }
                Slot::Tab(ix, w, opening) => (ix, w, opening),
            };
            let Some(t) = tabs.iter().nth(ix) else { continue };
            let st = self.tab_state(t);
            let (label, named) = self.panes.cur.tab_labels.labels.get(ix).cloned().unwrap_or_default();
            // A worker of a session not attached here: as last seen (as
            // nothing, if this window holds nothing of its session).
            let worker = match (&t.worker, t.session.as_deref()) {
                (Some(w), Some(s)) if t.kind == Kind::Worker => self.worker_state(s, &w.handle, Some(w.who())),
                _ => None,
            };
            drawn.push((t.id, lay.width, label.clone(), named));
            let on = active == Some(ix);
            // While the indicator moves, its quads are the fill and bar.
            let lit = !(on && quads.is_some());
            let tab = self.render_tab(t.id, ix, (on, lit), t.kind, (st, worker), label, named, lay.width, p, cx);
            row = row.child(if opening {
                div().flex_none().w(px(w)).h_full().overflow_hidden().child(tab).into_any_element()
            } else {
                tab.into_any_element()
            });
        }
        let needs =
            selected.and_then(|id| self.model.tabs.iter().find(|t| t.id == id)).is_some_and(|t| self.tab_state(t) == State::NeedsYou);
        let quads = quads.map(|(qx, qw)| {
            div()
                .absolute()
                .top_0()
                .left(px(qx))
                .w(px(qw))
                .h_full()
                .bg(p.raised)
                .when(!needs, |d| d.child(div().absolute().top_0().left_0().right(px(1.)).h(px(2.)).bg(p.you_bar)))
        });
        self.panes.cur.tab_fx.drawn = drawn;
        self.panes.cur.tab_fx.shown = selected;
        let (before, after) = (lay.first, n - lay.first - lay.shown);
        if lay.overflow && self.panes.cur.tab_labels.arrows.0 != (before, after) {
            self.panes.cur.tab_labels.arrows = ((before, after), (format!("‹ {before}").into(), format!("{after} ›").into()));
        }
        let (left, right) = self.panes.cur.tab_labels.arrows.1.clone();
        let arrow = |id: &'static str, text: SharedString, on: bool| {
            h_flex()
                .id(id)
                .w(px(ARROW_W))
                .h_full()
                .flex_none()
                .justify_center()
                .items_center()
                .child(mono(text, crate::fonts::size::META, if on { p.prose_ch } else { p.dim_ch }))
                .when(on, |d| d.cursor_pointer().hover(|d| d.bg(p.raised)))
        };
        h_flex()
            .id("tab-strip")
            .relative()
            .h(px(TAB_H))
            .flex_none()
            .w_full()
            .overflow_hidden()
            .bg(p.chrome)
            .child({
                let at = self.panes.cur.strip_at.clone();
                canvas(move |b, _, _| at.set(b.origin), |_, _, _, _| {}).absolute().inset_0()
            })
            .children(quads)
            .border_b_1()
            .border_color(p.hair)
            .when(lay.overflow, |d| {
                d.child(arrow("tabs-left", left, before > 0).border_r_1().border_color(p.hair).on_click(cx.listener(|this, _, _, cx| {
                    this.panes.cur.tab_first = this.panes.cur.tab_first.saturating_sub(1);
                    cx.notify();
                })))
            })
            .child(row)
            .children(cover)
            .when(lay.overflow, |d| {
                d.child(arrow("tabs-right", right, after > 0).on_click(cx.listener(|this, _, _, cx| {
                    this.panes.cur.tab_first += 1;
                    cx.notify();
                })))
            })
            .child(div().flex_1())
            .when(new, |d| {
                d.child(
                    h_flex()
                        .id("tab-new")
                        .w(px(PLUS_W))
                        .h_full()
                        .flex_none()
                        .justify_center()
                        .items_center()
                        .border_l_1()
                        .border_color(p.hair)
                        .cursor_pointer()
                        .child(mono("+", 15., p.dim_ch).hover(|d| d.text_color(p.ink)))
                        .hover(|d| d.bg(p.raised))
                        .tooltip(|window, cx| crate::overlay::tip(new_tip(), window, cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.create();
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        id: u64,
        ix: usize,
        (on, lit): (bool, bool),
        kind: Kind,
        (st, worker): (State, Option<Liveness>),
        label: SharedString,
        named: bool,
        width: f32,
        p: &Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = !kind.attaches();
        let title_of = |on: bool| match st {
            _ if view || kind == Kind::Worker => {
                if on {
                    p.ink
                } else {
                    p.prose_ch
                }
            }
            State::NeedsYou | State::Running | State::Workers | State::Unread => p.ink,
            _ if on => p.ink,
            State::Parked | State::Locked | State::Saved => p.dim_ch,
            _ => p.prose_ch,
        };
        let fx = &self.fx;
        // The label eases to `ink` over `quick` as its tab is shown, and
        // back over `hover_out` (fluid.md §3.5).
        let k = fx.ease_to(
            self.panes.cur.id.salt(crate::motion::key(LABEL, id)),
            if on { 1. } else { 0. },
            crate::motion::QUICK,
            crate::motion::HOVER_OUT,
        );
        let title_color = crate::motion::mix(title_of(false), title_of(true), k);
        // NEEDS YOU first: a worker tab's session's prompt too. Its bar
        // draws left to right over `base` (motion.md §6.3).
        let needs = st == State::NeedsYou;
        let bar = if needs {
            let since = fx.change(crate::motion::Tag::TabNeeds, id, 1, None).map(|c| c.0);
            Some((p.warn_mark_ch, fx.draw(since, std::time::Duration::ZERO, crate::motion::BASE)))
        } else {
            fx.change(crate::motion::Tag::TabNeeds, id, 0, None);
            (on && lit).then_some((p.you_bar, 1.))
        };
        // Hover and press (fluid.md §3.9): the fill on over `hover_in`, off
        // over `hover_out`; held, `press_ch` and the content 1 px down.
        let touch = self.panes.cur.id.salt(crate::motion::key(TOUCH, id));
        let (fill, dy) = fx.fill(touch, p.chrome, (on && lit).then_some(p.raised), p.raised, p.press_ch);
        // The selected tab while the indicator's quads are up (it slides in
        // under them on a close, re-gate 2): no fill of its own, so it never
        // paints over them.
        let fill = fill.filter(|_| lit || !on);
        // A worker tab: its worker's glyph and word (§4.2), whatever its
        // session waits on (the bar says that; the stub over its view too).
        // One this window holds nothing of: the rest glyph and no word.
        let (g, word, family) = match (kind, worker) {
            (_, Some(w)) => {
                let (g, word, _, family) = glyph::worker(w);
                (Some(g), Some(word), family)
            }
            (Kind::Worker, None) => (Some(Glyph::Idle), None, glyph::Family::Rest),
            _ => {
                let (g, family) = glyph::session(st);
                (g, st.word(), family)
            }
        };
        // The current tab's word is the composer foot's (design.md §8.14):
        // its glyph stays, its title gets the room.
        let word = word.filter(|_| width >= WORD_MIN && !view && !on);
        let (word_color, mark_color) = glyph::ink(family, Ground::Chrome, p);
        let drag = TabDrag { id, ix, label: label.clone(), bg: p.raised, fg: p.ink, edge: p.edge };
        let mark = p.focus_ch;
        let name = match (kind, worker) {
            _ if view => "view",
            (_, Some(w)) => glyph::worker(w).2,
            (Kind::Worker, None) => "not known in this window",
            _ => st.name(),
        };
        let waits = if needs && worker.is_some() { " · its session needs you" } else { "" };
        let (tip, me) = (label.clone(), cx.weak_entity());
        let menu_open = self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some();
        let close = h_flex()
            .id(("tab-x", id as usize))
            .debug_selector(|| format!("tab {id} x"))
            .group("tab-x")
            .w(px(18.))
            .h_full()
            .flex_none()
            .justify_center()
            .items_center()
            .child(glyph::reveal(Glyph::Close, "tab", "tab-x", on, p.dim_ch, p.ink))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let ix = this.model.tabs.iter().position(|t| t.id == id);
                if let Some(ix) = ix {
                    this.close_tab(ix);
                }
                cx.notify();
            }));
        h_flex()
            .id(("tab", id as usize))
            .debug_selector(|| format!("tab {id}"))
            .group("tab")
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .pl(px(16.))
            .pr(px(8.))
            .gap(px(8.))
            .items_center()
            .border_r_1()
            .border_color(p.hair)
            .cursor_pointer()
            .when_some(fill, |d, c| d.bg(c))
            .when_some(bar, |d, (c, f)| d.child(div().absolute().top_0().left_0().w(px(((width - 1.) * f).max(0.))).h(px(2.)).bg(c)))
            .child(
                div().flex().flex_none().relative().top(px(dy)).child(match kind {
                    Kind::Settings => crate::views::gear(if on { p.you_bar } else { p.dim_ch }, None),
                    Kind::Usage => crate::views::bars_glyph(if on { p.you_bar } else { p.dim_ch }),
                    Kind::Session | Kind::Worker => div()
                        .flex()
                        .flex_none()
                        .size(px(10.))
                        .debug_selector(|| format!("tab {id} glyph"))
                        .children(g.map(|g| glyph::spinning(fx, g, mark_color, fill.unwrap_or(p.chrome)))),
                }),
            )
            .child(
                div()
                    .debug_selector(|| format!("tab {id} title"))
                    .relative()
                    .top(px(dy))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(named, |d| d.font_family(fonts::MONO).text_px(12.))
                    .when(!named, |d| d.text_px(13.))
                    .text_color(title_color)
                    .child(label),
            )
            .when_some(word, |d, w| d.child(display(w, 12., word_color).flex_none().relative().top(px(dy))))
            .child(close.relative().top(px(dy)))
            // Formatted on hover only.
            // Not under an open menu (a right-click's pointer rests on the
            // tab it opened from).
            .when(!menu_open, |d| {
                d.tooltip(move |window, cx| {
                    // A session's: the line the session header had (design.md §8.5).
                    let meta = me.upgrade().filter(|_| kind == Kind::Session).and_then(|v| v.read(cx).tab_meta(id));
                    let text = match (kind, meta) {
                        (Kind::Worker, _) => match tip.split_once(" · ") {
                            Some((w, s)) => format!("worker {w} · {name} · of the session {s}{waits} · closing this tab leaves it running"),
                            None => format!("{tip} · {name}"),
                        },
                        (_, Some(meta)) => format!("{} · {name}", meta.tooltip(&tip)),
                        _ => format!("{tip} · {name}"),
                    };
                    crate::overlay::tip(text, window, cx)
                })
            })
            .map(|d| AppView::touch(d, touch, cx))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                let ix = this.model.tabs.iter().position(|t| t.id == id);
                if let Some(ix) = ix {
                    this.focus_tab(ix);
                }
                // A double-click on a session's tab names it (as F2).
                if kind == Kind::Session && ev.click_count() == 2 {
                    this.rename_session(window, cx);
                }
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                let ix = this.model.tabs.iter().position(|t| t.id == id);
                if ev.is_middle_click()
                    && let Some(ix) = ix
                {
                    this.close_tab(ix);
                    cx.notify();
                } else if ev.is_right_click() && kind == Kind::Worker {
                    this.worker_tab_menu(id, ev.position(), window, cx);
                } else if ev.is_right_click() && kind == Kind::Session {
                    this.open_session_menu(crate::app::SessionAt::Tab(id), ev.position(), window, cx);
                }
            }))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<TabDrag>(move |s, d, _, _| match d.ix.cmp(&ix) {
                std::cmp::Ordering::Less => s.border_r_2().border_color(mark),
                std::cmp::Ordering::Greater => s.border_l_2().border_color(mark),
                std::cmp::Ordering::Equal => s,
            })
            .on_drop(cx.listener(move |this, d: &TabDrag, _, cx| {
                this.move_tab(d.id, id);
                cx.notify();
            }))
    }
}

fn new_tip() -> &'static str {
    if cfg!(target_os = "macos") { "New session in a new tab (Cmd+T)" } else { "New session in a new tab (Ctrl+T)" }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use super::{Strip, TAB_MAX, TAB_MIN, fits, strip};

    #[test]
    fn tabs_share_the_room_then_overflow_keeping_the_visible_one_in_view() {
        assert_eq!(strip(0, None, 900., 0, true).shown, 0);
        // Few tabs: each as wide as allowed.
        assert_eq!(strip(2, Some(0), 1000., 0, true), Strip { first: 0, shown: 2, width: TAB_MAX, overflow: false });
        // They narrow to share the room, down to the minimum.
        let s = strip(6, Some(5), 900., 0, true);
        assert_eq!((s.shown, s.width, s.overflow), (6, 150., false));
        // Past it: arrows, as many as fit, the visible one brought in.
        let s = strip(12, Some(11), 900., 0, true);
        assert!(s.overflow && s.shown == 6 && s.width >= TAB_MIN, "{s:?}");
        assert_eq!(s.first + s.shown, 12, "the last (visible) one is the rightmost shown");
        // The user's scroll stays while the visible tab does not change...
        assert_eq!(strip(12, Some(11), 900., 2, false).first, 2);
        // ...and is clamped to what can be shown.
        assert_eq!(strip(12, Some(0), 900., 40, false).first, 6);
        // A visible tab left of the window comes in from the left.
        assert_eq!(strip(12, Some(1), 900., 5, true).first, 1);
        // Never less than one tab, however narrow.
        let s = strip(5, Some(3), 100., 0, true);
        assert_eq!((s.shown, s.first), (1, 3));
        // `fits` is what `strip` shows.
        for (n, room) in [(0, 900.), (2, 1000.), (6, 900.), (12, 900.), (12, 1100.), (5, 100.), (13, 920.)] {
            assert_eq!(fits(n, room), strip(n, Some(0), room, 0, true).shown, "{n} in {room}");
        }
        // The room shrinking (7 fit, then 6): following again keeps the
        // visible tab in view; without it, it would scroll out.
        let wide = strip(13, Some(10), 1030., 0, true);
        assert_eq!((wide.first, wide.shown), (4, 7));
        assert_eq!(strip(13, Some(10), 920., wide.first, false).first, 4, "kept: 10 is out of 4..10");
        let narrow = strip(13, Some(10), 920., wide.first, true);
        assert!(narrow.first <= 10 && 10 < narrow.first + narrow.shown, "{narrow:?}");
    }
}
