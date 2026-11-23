//! The tray of workers over the composer (the TUI's subagent tray), and
//! the NEEDS YOU stub over a worker's view. One row per worker, in the grammar of the
//! transcript's worker block (design.md §8.4): its state glyph, name, what
//! it is doing (or ended with), tools and elapsed time, its state word; at
//! most what
//! [`crate::fit::tray_rows`] leaves room for, then "+N more". A finished
//! worker shows how it ended for [`crate::watch::FLASH`], then drops.
//! Nothing moves: no spinner; the elapsed time moves when a row comes, and
//! under motion a running worker's clock ticks 1 Hz in whole seconds from
//! the daemon's last count ([`ticked`]). A click on a row opens that worker's view; the
//! middle button, Ctrl/Cmd+click or a double-click open it in a tab, the
//! right button its menu, and a hovered row shows its card and a × that
//! cancels it ([`crate::wtabs`]). A cancel on its way says `cancelling`.
//!
//! Built from the attached session's workers ([`crate::subagents`]) only
//! when they change: no row is formatted per frame (each frame's rows hold
//! their click handlers, which share the row's strings).

use std::time::Instant;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, WORKER_KEY, column, display, mono};
use crate::fonts::TextPx as _;
use crate::fonts::{self, label};
use crate::glyph::{self, Ground};
use crate::menu::At;
use crate::motion::{self, Tag};
use crate::subagents::{Live, Liveness, Status, fmt_secs};
use crate::theme::{Metrics, Palette};
use crate::watch::Candidate;
use crate::wtabs::Who;

/// One tray row, formatted once per change.
/// `facts` as this frame shows them: a running worker's clock ticked to the
/// frame in whole seconds (motion.md §6.2), asking for the frame its next
/// second needs; Off (and an ended run), as the last row said.
pub(crate) fn ticked(fx: &motion::Motion, facts: &SharedString, tick: &Option<(SharedString, crate::subagents::Tick)>) -> SharedString {
    match tick {
        Some((head, t)) if fx.on() => {
            let (s, next) = t.whole(fx.clock_now());
            fx.tick_clock(next);
            let clock = crate::subagents::fmt_whole(s);
            if head.is_empty() { clock.into() } else { format!("{head} · {clock}").into() }
        }
        _ => facts.clone(),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TrayRow {
    pub(crate) id: u64,
    /// `sa_N`.
    pub(crate) handle: SharedString,
    pub(crate) name: SharedString,
    /// Its call, if that call's own result named it ([`Who`]).
    pub(crate) call: Option<SharedString>,
    /// The tray lists the attached session's workers: never as last seen.
    pub(crate) live: Liveness,
    pub(crate) facts: SharedString,
    /// Running: the facts before its clock, and the clock ([`ticked`]).
    pub(crate) tick: Option<(SharedString, crate::subagents::Tick)>,
    /// Its chip's clock as last reported (`41.0s`; a running one ticks
    /// from [`TrayRow::tick`]), and its step cut to [`STEP_CHARS`].
    pub(crate) clock: Option<SharedString>,
    pub(crate) step: SharedString,
    pub(crate) watchable: bool,
    pub(crate) tip: SharedString,
    /// A re-attach's demoted start ([`glyph::steady`]).
    pub(crate) demoted: bool,
}

impl TrayRow {
    /// It has a clock (running, or as last reported): its chip keeps the
    /// clock's slot.
    pub(crate) fn clocked(&self) -> bool {
        self.tick.is_some() || self.clock.is_some()
    }

    /// `cancelling`: a cancel of it is on its way (its rows have not said
    /// so): its word says cancelling until it has stopped.
    fn of(l: Live, cancelling: bool) -> Self {
        let mut facts = Vec::new();
        if let Some(n) = l.tools.filter(|&n| n > 0) {
            facts.push(format!("{n} tool{}", if n == 1 { "" } else { "s" }));
        }
        let tick = l.tick.map(|t| (SharedString::from(facts.join(" · ")), t));
        let clock = l.elapsed.map(|s| SharedString::from(fmt_secs(s)));
        if let Some(s) = l.elapsed {
            facts.push(fmt_secs(s));
        }
        let step = cut(&crate::turns::tilde(&l.line, true), STEP_CHARS).into();
        let tip = if l.watchable {
            format!("watch {} (sa_{}) · middle-click: in a tab · {WORKER_KEY} walks the workers", l.name, l.id)
        } else {
            format!("{}: the blocking subagent has no thread to watch", l.name)
        };
        TrayRow {
            id: l.id,
            handle: format!("sa_{}", l.id).into(),
            name: l.name.into(),
            call: l.call,
            live: Liveness::of(Some(l.status), true, true).cancelling(cancelling),
            facts: facts.join(" · ").into(),
            tick,
            clock,
            step,
            watchable: l.watchable,
            tip: tip.into(),
            demoted: l.demoted,
        }
    }
}

/// A worker's step in the workers menu, at most (the menu is 360 px at most).
const MENU_STEP: usize = 24;

/// Which chips go first, so `+N more` never folds away a worker that
/// needs a look while a quiet one is drawn (finish gate r2): failed (or
/// timed out), then the watched one, then running (cancelling, started)
/// newest first, then the ended, newest first. The daemon's workers have no
/// waiting state (`Status`): a worker's prompt is its session's (the
/// needs stub), so none ranks above a failure here.
fn need(r: &TrayRow, watched: Option<u64>) -> (u8, std::cmp::Reverse<u64>) {
    let rank = match r.live {
        Liveness::Failed | Liveness::TimedOut => 0,
        _ if watched == Some(r.id) => 1,
        Liveness::Started | Liveness::Running | Liveness::Cancelling | Liveness::LastSeen => 2,
        Liveness::Done | Liveness::Cancelled | Liveness::Gone => 3,
    };
    (rank, std::cmp::Reverse(r.id))
}

/// The tray's rows as last built: rebuilt when a worker changes, the
/// focused view changes, or a finished worker's flash ends.
#[derive(Default)]
pub(crate) struct TrayCache {
    pub(crate) key: Option<(u64, Option<u64>, bool, u64, u64)>,
    pub(crate) rows: Vec<TrayRow>,
    /// When the next finished worker drops.
    until: Option<Instant>,
    /// Whose workers the rows are (a switch is not an arrival).
    session: Option<String>,
    /// Rows that left while watched, fading in their slot (no clicks):
    /// their slot, the row, when it left (motion.md §6.2).
    pub(crate) ghosts: Vec<(usize, TrayRow, Instant)>,
}

/// Tray rows' keys in [`motion::Motion`], apart from item ids.
const TRAY: u64 = 1 << 61;
/// A chip's x in its line ([`motion::Motion::cell_slide`]).
const TRAY_X: u64 = 0x7AF4 << 48;

/// Between chips, and between their lines.
pub(crate) const CHIP_GAP: f32 = 8.;

/// A chip's step, at most (its box cuts it shorter, `…`), and its name.
pub(crate) const STEP_CHARS: usize = 36;
pub(crate) const NAME_CHARS: usize = 14;
/// A chip's least width at the default text (it scales with it): a line
/// of the column holds as many as are this wide, [`CHIP_GAP`] apart
/// ([`grid`]). 260: two a line in the 624-px column of a 1280-px window
/// with both panels open (and the 672-px capped column); one in a column
/// under 528.
pub(crate) const MIN_CHIP: f32 = 260.;
/// A chip's clock slot: as wide as the widest it shows (`59m 59s`), so a
/// tick never reflows the chips.
const CLOCK_CHARS: usize = 7;
/// The cancel ×'s slot, after the clock.
const X_W: f32 = 16.;

/// A chip's clock as this frame shows it: ticking while it runs (motion
/// on), else as last reported.
fn chip_clock(fx: &motion::Motion, r: &TrayRow) -> Option<SharedString> {
    match &r.tick {
        Some((_, t)) if fx.on() => {
            let (s, next) = t.whole(fx.clock_now());
            fx.tick_clock(next);
            Some(crate::subagents::fmt_whole(s).into())
        }
        _ => r.clock.clone(),
    }
}

/// `s` in at most `n` characters, `…` last when cut.
pub(crate) fn cut(s: &str, n: usize) -> String {
    match s.char_indices().nth(n.saturating_sub(1)) {
        Some((at, _)) if s.chars().count() > n => format!("{}…", &s[..at]),
        _ => s.to_string(),
    }
}

/// One place in the chips' flow: a worker's chip or a left one's ghost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Row(usize),
    Ghost(usize),
}

/// How the chips sit over the composer this frame ([`grid_pack`]): the
/// slots drawn and the line each is on, the workers not drawn (`+N more`,
/// on the last line) and how many lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Chips {
    pub(crate) slots: Vec<(Slot, usize, f32)>,
    pub(crate) more: usize,
    pub(crate) more_line: usize,
    pub(crate) lines: usize,
    /// How many chips a line holds ([`grid`]): the grid a chip's cell is
    /// in.
    pub(crate) per: usize,
}

impl Chips {
    /// The workers' chips drawn (not ghosts).
    pub(crate) fn drawn(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.0, Slot::Row(_))).count()
    }
}

/// The chips' grid in a column `column` px wide (`lay.column`: while a
/// panel flies, the frozen one): how many chips a line holds, and the
/// width of every chip, the line's share. Set by the column alone: what a
/// chip says (its step, clock, name, hover's ×, an armed cancel, its
/// state) never moves a chip, and the dock changes height only when the
/// workers need another line or one less.
pub(crate) fn grid(column: f32, m: &Metrics) -> (usize, f32) {
    let least = (MIN_CHIP * m.r).round();
    let k = ((column + CHIP_GAP) / (least + CHIP_GAP)).floor().max(1.) as usize;
    (k, ((column - (k - 1) as f32 * CHIP_GAP) / k as f32).floor())
}

/// A chip's step: what it does while it runs, else its state's word.
fn chip_step(r: &TrayRow) -> &str {
    match r.live {
        Liveness::Running | Liveness::Started if !r.step.is_empty() => &r.step,
        _ => glyph::worker(r.live).2,
    }
}

/// `+N more`'s width: room for `+99 more` whatever N is (it rolls inside).
pub(crate) fn more_w(m: &Metrics) -> f32 {
    (2. + 10. + crate::turns::text_w(8, m.meta * 0.6) + 10.).ceil()
}

/// `n` slots (chips and the ghosts of chips that left) in the grid, `k` a
/// line, each `w` wide in `column` px, as many as `lines` lines hold: each
/// one's line, and how many are drawn. All of them, else as many as fit
/// with `+N more` (`more` px) ending the last line. One a line, the last
/// still has one beside it (narrower: [`AppView::pack_chips`]), so one
/// line never shows only `+N more`.
pub(crate) fn grid_pack(n: usize, k: usize, w: f32, column: f32, lines: usize, more: f32) -> (Vec<usize>, usize) {
    if lines == 0 || k == 0 {
        return (Vec::new(), 0);
    }
    let fit = if n <= k * lines {
        n
    } else if k == 1 {
        lines
    } else {
        let beside = ((column - more) / (w + CHIP_GAP)).floor().max(0.) as usize;
        k * (lines - 1) + beside.min(k)
    };
    ((0..fit).map(|i| i / k).collect(), fit)
}

impl AppView {
    /// The tray's rows, rebuilt if a worker changed; the timer that drops
    /// a finished worker's row at the end of its flash.
    pub(crate) fn sync_tray(&mut self, cx: &mut Context<Self>) {
        let attached = self.model.lives[self.panes.cur.live].active.is_some();
        let now = Instant::now();
        // Arrivals and fades on the motion clock, as every motion reads it
        // (tests: the test's own).
        let fx_now = self.fx.clock();
        // A left row's fade is over (or motion went off): the dock
        // shrinks, in one frame.
        let gone = motion::exit(motion::QUICK);
        let on = self.fx.on();
        self.tray.ghosts.retain(|g| on && fx_now.saturating_duration_since(g.2) < gone);
        // Cancels the rows now say are cancelling (or ended) are done.
        let l = &mut self.model.lives[self.panes.cur.live];
        if let Some(s) = l.active.as_ref().map(|a| a.session_id.as_str()) {
            let subs = &l.stats.subagents;
            let said = |h: &str| {
                let id = h.strip_prefix("sa_").and_then(|n| n.parse().ok());
                id.and_then(|id| subs.status(id)).is_some_and(|st| st == Status::Cancelling || st.terminal())
            };
            l.cancels.settle(s, said, now);
        }
        let key = (
            self.model.lives[self.panes.cur.live].stats.subagents.revision(),
            self.model.lives[self.panes.cur.live].watch.focused().map(|w| w.subagent_id),
            attached,
            self.model.lives[self.panes.cur.live].cancels.generation,
            // A worker tab closed: its view and list go now.
            self.model.tabs.generation,
        );
        if self.tray.key == Some(key) && self.tray.until.is_none_or(|u| fx_now < u) {
            return;
        }
        let live = if attached { self.model.lives[self.panes.cur.live].stats.subagents.live() } else { Vec::new() };
        // A view nobody looks at (on screen, or held by a worker tab) whose
        // worker ended goes (and leaves the walk), as do the views of the
        // worker tabs past the most recently shown few, and their lists.
        // Every pane's on this store (Shady #36 M2): the other panes' views
        // (parked, `Watch::park` keeps them) and their worker tabs' views.
        let live_id = self.panes.cur.live;
        let mut held = self.model.held_views(live_id);
        let session = self.model.lives[live_id].active.as_ref().map(|a| a.session_id.clone());
        for p in self.panes.rest.iter().filter(|p| p.live == live_id) {
            held.extend(
                p.parked
                    .tabs
                    .iter()
                    .filter(|t| t.kind == crate::tabs::Kind::Worker && t.session.is_some() && t.session == session)
                    .filter_map(|t| t.worker.as_ref().map(|w| w.handle.clone())),
            );
        }
        self.model.lives[live_id].watch.hold(held);
        // A finished worker's flash on the motion clock too (tests: the
        // test's own, never a slow runner's wall time).
        self.model.lives[self.panes.cur.live].watch.prune(fx_now, &live);
        self.drop_released_panes();
        let (rows, until) = self.model.lives[self.panes.cur.live].watch.tray(live, fx_now);
        let cancels = &self.model.lives[self.panes.cur.live].cancels;
        let session = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.as_str());
        let pending = |id: u64| session.is_some_and(|s| cancels.pending(s, &format!("sa_{id}")));
        let mut rows: Vec<TrayRow> = rows
            .into_iter()
            .map(|l| {
                let p = pending(l.id);
                TrayRow::of(l, p)
            })
            .collect();
        let watched = self.model.lives[self.panes.cur.live].watch.focused().map(|w| w.subagent_id);
        // The watched one goes up only if it would be folded into `+N
        // more`: a chip clicked where it is drawn stays under the pointer
        // (Shady #16 r4).
        rows.sort_by_key(|r| need(r, None));
        let drawn = self.chips.drawn();
        if watched.is_some_and(|w| rows.iter().position(|r| r.id == w).is_none_or(|i| i >= drawn)) {
            rows.sort_by_key(|r| need(r, watched));
        }
        // Rows that came while watched draw in (staggered); rows that left fade out.
        let session = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone());
        let mut ghosts = std::mem::take(&mut self.tray.ghosts);
        if self.fx.on() && session.is_some() && session == self.tray.session {
            let fresh = rows.iter().filter(|r| !self.tray.rows.iter().any(|o| o.id == r.id));
            for (k, r) in fresh.enumerate() {
                self.fx.col(self.panes.cur.id).born(TRAY | r.id, fx_now + motion::stagger(k));
            }
            for (slot, r) in self.tray.rows.iter().enumerate() {
                if !rows.iter().any(|n| n.id == r.id) {
                    ghosts.push((slot, r.clone(), fx_now));
                }
            }
        } else {
            ghosts.clear();
        }
        self.tray = TrayCache { key: Some(key), rows, until, session, ghosts };
        // An open menu says what its worker can do now (it may have ended
        // under it); a keyboard one whose row has gone closes.
        self.refresh_menu();
        self.close_orphan_menu();
        match until {
            Some(u) if self.tray_due.as_ref().map(|d| d.0) != Some(u) => {
                // The pane's own (Shady #36 L4): another pane's tray never
                // replaces it.
                let id = self.panes.cur.id;
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(u.saturating_duration_since(fx_now)).await;
                    let _ = this.update(cx, |v, cx| {
                        v.in_pane(id, |v| v.tray_due = None);
                        cx.notify();
                    });
                });
                self.tray_due = Some((u, task));
            }
            Some(_) => {}
            None => self.tray_due = None,
        }
    }

    /// Open `who`'s view in place (a tray row's click).
    pub(crate) fn watch_worker(&mut self, who: &Who, cx: &mut Context<Self>) {
        let cwd = self.model.lives[self.panes.cur.live].cwd().map(str::to_string);
        let c = Candidate::of(who.id, &who.name).with_call(who.call.as_deref());
        self.model.lives[self.panes.cur.live].watch.open(&c, cwd.as_deref(), Instant::now());
        self.watch_opened();
        cx.notify();
    }

    /// How many workers' chips are drawn (the rest are "+N more").
    pub(crate) fn tray_drawn(&self) -> usize {
        self.chips.drawn()
    }

    /// The chips' flow, in order: each slot a left row's ghost (it keeps
    /// its place while it fades) or the next worker.
    fn tray_slots(&self) -> Vec<Slot> {
        let n = self.tray.rows.len() + self.tray.ghosts.len();
        let mut real = 0..self.tray.rows.len();
        (0..n)
            .filter_map(|slot| match self.tray.ghosts.iter().position(|g| g.0 == slot) {
                Some(g) => Some(Slot::Ghost(g)),
                None => real.next().map(Slot::Row),
            })
            .collect()
    }

    /// The chips over the composer this frame, in the column (`line` px):
    /// the grid ([`grid`]), the lines its slots need, then as many as
    /// [`crate::fit::chip_lines`] has room for (`lines`); the lines used.
    pub(crate) fn fit_chips(&mut self, line: f32, m: &Metrics, lines: impl Fn(usize) -> usize) -> usize {
        let slots = self.tray_slots();
        let (k, _) = grid(line, m);
        let n = lines(slots.len().div_ceil(k));
        self.pack_chips(slots, line, n, m);
        self.chips.lines
    }

    pub(crate) fn pack_chips(&mut self, slots: Vec<Slot>, column: f32, lines: usize, m: &Metrics) {
        let (k, w) = grid(column, m);
        let (on, fit) = grid_pack(slots.len(), k, w, column, lines, more_w(m));
        let all = fit == slots.len();
        // One a line, with `+N more`: the last line's chip gives it room.
        let last = (column - more_w(m) - CHIP_GAP).floor().max(0.);
        let w_of = |l: usize| if !all && k == 1 && l + 1 == lines { last } else { w };
        let placed: Vec<(Slot, usize, f32)> = slots.into_iter().zip(on).map(|(s, l)| (s, l, w_of(l))).collect();
        let drawn = placed.iter().filter(|s| matches!(s.0, Slot::Row(_))).count();
        let more = self.tray.rows.len().saturating_sub(drawn);
        let used = if all { placed.last().map_or(0, |s| s.1 + 1) } else { lines };
        let more_line = used.saturating_sub(1);
        // Only ghosts cut: no worker is left out, and nothing says so.
        self.chips = Chips { slots: placed, more: if all { 0 } else { more }, more_line, lines: used, per: k };
    }

    /// The workers menu (polish.md §2.5): `+N more`'s workers, each as its
    /// chip would say it (name, state, step); picked, its view in place.
    pub(crate) fn open_workers_menu(&mut self, pt: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.workers_menu_items();
        if items.is_empty() {
            return;
        }
        self.close_menu();
        if let Some(prev) = window.focused(cx).filter(|f| *f != self.focus)
            && self.menu_focus_back.is_none()
        {
            self.menu_focus_back = Some(prev);
        }
        window.focus(&self.focus, cx);
        self.gear_menu = false;
        self.wmenu = Some(crate::menu::Menu::new(items, crate::menu::At::Point(pt), false).born(self.fx.clock()));
        cx.notify();
    }

    /// The workers `+N more` stands for, each as its chip would say it.
    fn workers_menu_items(&self) -> Vec<crate::menu::Item<u64>> {
        self.tray.rows[self.tray_drawn().min(self.tray.rows.len())..]
            .iter()
            .map(|r| {
                let item =
                    crate::menu::Item::new(r.id, format!("{}  {} · {}", r.name, glyph::worker(r.live).2, cut(chip_step(r), MENU_STEP)));
                if r.watchable { item } else { item.off(r.tip.clone()) }
            })
            .collect()
    }

    /// The open workers menu says what `+N more` stands for now (a worker
    /// ended, or got a chip as another left), its selection kept on the
    /// same worker; with none left it closes.
    fn refresh_workers_menu(&mut self) {
        let Some(m) = self.wmenu.as_ref() else { return };
        let items = self.workers_menu_items();
        if items == m.items {
            return;
        }
        if items.is_empty() {
            // Closed here, in the render after `overlay_exits` (Shady #15
            // L1): its exit starts now, at its place, so this frame draws
            // the first of it (`fx.leave` asks for the rest).
            if let Some(m) = self.wmenu.take().filter(|m| m.born.is_some()) {
                self.wmenu_leaving = Some((m, self.fx.now()));
            }
            self.wmenu_was = None;
            return;
        }
        let sel = m.sel.and_then(|i| m.items.get(i)).and_then(|was| items.iter().position(|n| n.act == was.act));
        if let Some(m) = self.wmenu.as_mut() {
            (m.items, m.sel) = (items, sel);
        }
    }

    /// A worker of the workers menu picked: its view in place.
    pub(crate) fn workers_menu_pick(&mut self, id: u64, how: crate::menu::How, _: &mut Window, cx: &mut Context<Self>) {
        if !how.is_press() {
            return;
        }
        self.close_menu();
        if let Some(r) = self.tray.rows.iter().find(|r| r.id == id) {
            let who = Who::new(r.id, r.name.clone(), r.call.clone());
            self.watch_worker(&who, cx);
        }
    }

    /// A key while the workers menu is open: `true` if it took it.
    pub(crate) fn workers_menu_keydown(&mut self, ks: &Keystroke, press: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(m) = self.wmenu.as_mut() else { return false };
        match m.key(&ks.key, ks.modifiers.shift, press) {
            crate::menu::Key::Moved => {}
            crate::menu::Key::Pick(id) => {
                self.menu_picked = true;
                self.workers_menu_pick(id, crate::menu::How::Key { press }, window, cx);
            }
            crate::menu::Key::Close => self.close_menu(),
            crate::menu::Key::Other => {
                self.close_menu();
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// The workers menu, if open: at the root, over everything.
    /// One leaving (fluid.md §3.10) is drawn without a hitbox until gone.
    pub(crate) fn render_workers_menu(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let (m, phase) = match self.wmenu.as_ref() {
            Some(m) => (m, crate::menu::Phase::enter(&self.fx, m.born, -1.)),
            None => {
                let (m, at) = self.wmenu_leaving.as_ref()?;
                (m, crate::menu::Phase::leave(&self.fx, *at, -1.)?)
            }
        };
        let crate::menu::At::Point(pt) = m.at else { return None };
        let body = crate::menu::render("workers-menu", m, p, AppView::workers_menu_pick, |this, _| this.close_menu(), phase, &self.fx, cx);
        Some(deferred(anchored().position(pt).snap_to_window_with_margin(px(8.)).child(body)).with_priority(3))
    }

    /// A menu opened from the keyboard over a tray row whose row is not
    /// drawn any more (its worker dropped, the window got short): it
    /// closes, never left taking keys unseen.
    /// The workers menu follows what `+N more` stands for. `sync_tray`'s
    /// call reads last frame's chips (a guess); the render's, after
    /// `fit_chips`, this frame's (the word).
    pub(crate) fn close_orphan_menu(&mut self) {
        self.refresh_workers_menu();
        let Some(At::Row(id)) = self.menu.as_ref().map(|(m, _)| m.at) else { return };
        if !self.tray.rows[..self.tray_drawn()].iter().any(|r| r.id == id) {
            self.close_menu();
        }
    }

    /// The worker chips over the composer (polish.md §2.5): flowing left
    /// to right in at most [`crate::fit::chip_lines`] lines, the rest in a
    /// `+N more` chip ending the last line (its tooltip names them).
    pub(crate) fn render_tray(&self, p: &Palette, m: &Metrics, cx: &Context<Self>) -> Option<impl IntoElement> {
        let chips = &self.chips;
        if chips.lines == 0 {
            return None;
        }
        let lay = self.panes.cur.layout;
        let cramped = lay.form() == crate::fit::Form::Cramped;
        let watched = self.model.lives[self.panes.cur.live].watch.focused().map(|w| w.subagent_id);
        let row_h = crate::fit::tray_row(m);
        let now = Instant::now();
        let session = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.as_str());
        let fx = &self.fx;
        // The watched chip's bar draws when its view opens here, and lights
        // after the composer's when you steer it (Handoff out).
        let watch_at =
            fx.col(self.panes.cur.id).change(Tag::Watched, 0, watched.unwrap_or(0), None).filter(|_| watched.is_some()).map(|c| c.0);
        let you_drawn = fx.draw(watch_at, std::time::Duration::ZERO, motion::QUICK);
        let steer =
            fx.flash(fx.col(self.panes.cur.id).sent().get().filter(|s| s.1 && watched.is_some()).map(|s| s.0 + 2 * motion::STAGGER));
        let you_color = motion::mix(p.you_mark_t, motion::mix(p.you, p.ink, 0.35), steer);
        let mut lines: Vec<Vec<AnyElement>> = (0..chips.lines).map(|_| Vec::new()).collect();
        // Each line's next chip's cell: a chip whose cell changed (one
        // before it left, polish.md P13) slides there, by cells (a new
        // column's share moves no chip).
        let mut cols = vec![0usize; chips.lines];
        for &(slot, line, w) in &chips.slots {
            let Some(at) = lines.get_mut(line) else { continue };
            let col = cols[line];
            cols[line] += 1;
            match slot {
                Slot::Ghost(g) => {
                    let Some(g) = self.tray.ghosts.get(g) else { continue };
                    let fade = 1.
                        - fx.shot(Some(g.2), std::time::Duration::ZERO, motion::exit(motion::QUICK), &motion::ease::IN, motion::Kind::Fade);
                    at.push(self.tray_ghost(&g.1, fade, w, row_h, p, m));
                    fx.want_frame();
                }
                Slot::Row(i) => {
                    let Some(r) = self.tray.rows.get(i) else { continue };
                    let armed = session
                        .is_some_and(|s| self.model.lives[self.panes.cur.live].cancels.armed(s, r.id, (r.call.as_deref(), &r.name), now));
                    let is_watched = watched == Some(r.id);
                    // Its bar: yours when watched, else the workers' rule,
                    // drawn top down when it came.
                    let bar = if is_watched {
                        glyph::bar_at(you_color, you_drawn)
                    } else {
                        glyph::bar_at(
                            p.sub_rule,
                            fx.draw(fx.col(self.panes.cur.id).birth(TRAY | r.id), std::time::Duration::ZERO, motion::QUICK),
                        )
                    };
                    // A chip whose cell changed slides there from where it
                    // was drawn, x and y on `snap` from one instant (a
                    // diagonal), in px to the end; a new share of the column
                    // moves none. One that came rises 8 px and lifts from
                    // 70 %, in turn (polish.md P13).
                    // In a new grid (the column took another count of chips
                    // a line, whatever changed its width: a layout or panel
                    // flight landed, a divider or panel dragged, the window
                    // resized), a chip is in its cell at once, from the frame
                    // that lands it (design gate #44 F5: after Join one slid
                    // 17 frames on, across another; Shady #44 n15: under a
                    // drag the chips snap too, nothing lags the pointer):
                    // each grid keys its own slides.
                    let grid = (chips.per as u64) << 32;
                    let off = fx.col(self.panes.cur.id).slide(TRAY | grid | r.id, line as f32 * (row_h + CHIP_GAP));
                    let dx = fx.cell_slide(self.panes.cur.id.salt(TRAY_X | grid | r.id), col as u64, col as f32 * (w + CHIP_GAP));
                    let (travel, strength) = fx.arrival(fx.col(self.panes.cur.id).birth(TRAY | r.id));
                    let (bounds, id) = (self.chip_bounds.clone(), r.id);
                    at.push(
                        div()
                            .relative()
                            .top(px(off))
                            .left(px(dx))
                            .flex_none()
                            // On the canvas (the dock's ground: at rest it
                            // shows nothing), so one chip passing another
                            // covers it or is covered, whichever is drawn
                            // later: never text through text (Shady R3-L1).
                            .bg(p.canvas)
                            .child(motion::lift(
                                div().relative().top(px(travel)).child(self.tray_row(r, is_watched, armed, w, row_h, bar, p, m, cx)),
                                strength,
                            ))
                            .child(canvas(move |b, _, _| bounds.borrow_mut().push((id, b)), |_, _, _, _| {}).absolute().inset_0())
                            .into_any_element(),
                    );
                }
            }
        }
        if chips.more > 0 {
            let rest: Vec<String> =
                self.tray.rows.iter().skip(chips.drawn()).map(|r| format!("{}  {}", r.name, glyph::worker(r.live).2)).collect();
            let tip = SharedString::from(rest.join("\n"));
            // As wide as `+99 more`, whatever it says: N rolls inside it.
            let more = h_flex()
                .id("tray-more")
                .debug_selector(|| "tray-more".into())
                .flex_none()
                .w(px(more_w(m)))
                .h(px(row_h))
                // The worker's fill, no frame: its room is padding, in a
                // layer of its own (a chip's, above).
                .px(px(crate::cards::frame_room(self.scale)))
                .bg(p.card_agent)
                .cursor_pointer()
                .hover(|d| d.bg(p.hover_agent))
                // It rolls as workers come and go (polish.md P5).
                .child(h_flex().flex_1().min_w_0().h_full().px(px(10.)).items_center().child(motion::roll_count(
                    self.fx.col(self.panes.cur.id),
                    motion::ROLL_MORE,
                    chips.more,
                    format!("+{} more", chips.more).into(),
                    |t| mono(t, m.meta, p.dim_t),
                )))
                // Its menu open: the menu says it (no tooltip over it).
                .when(self.wmenu.is_none(), |d| d.tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx)))
                .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_workers_menu(ev.position(), window, cx)));
            if let Some(at) = lines.get_mut(chips.more_line.min(chips.lines - 1)) {
                at.push(more.into_any_element());
            }
        }
        let gap = crate::fit::tray_box(lay.form());
        Some(column(
            lay,
            v_flex()
                .mt(px(gap))
                .gap(px(if cramped { 4. } else { CHIP_GAP }))
                .font_family(fonts::MONO)
                .children(lines.into_iter().map(|l| h_flex().h(px(row_h)).gap(px(CHIP_GAP)).items_center().children(l))),
        ))
    }

    /// A worker's chip (polish.md §2.5): square, on the worker's fill
    /// (`card_agent`: a worker, not a tool, Haseeb), no frame, `bar` on its
    /// left edge; its state glyph, name, step
    /// (or its state's word once it ended; an armed cancel's `Cancel …?`
    /// and CONFIRM), its clock (ticking while it runs) and the cancel ×.
    #[allow(clippy::too_many_arguments)]
    fn tray_row(
        &self,
        r: &TrayRow,
        watched: bool,
        armed: bool,
        w: f32,
        h: f32,
        bar: Div,
        p: &Palette,
        m: &Metrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (g, _, _, family) = glyph::worker(r.live);
        let (word_color, mark) = glyph::ink(family, Ground::Run, p);
        let running = matches!(r.live, Liveness::Running | Liveness::Started) && !r.step.is_empty();
        let step_color = if running { p.sub_status_t } else { word_color };
        let who = Who::new(r.id, r.name.clone(), r.call.clone());
        let clock = chip_clock(&self.fx, r);
        let x = r.watchable.then(|| self.cancel_x(&who, &r.handle, armed, watched, h, p, cx)).flatten();
        let (x, armed_el) = if armed { (None, x) } else { (x, None) };
        // Armed and drawn so (a cancel this window may send).
        let armed = armed_el.is_some();
        let cw = m.meta * 0.6;
        let id = r.id;
        let row = h_flex()
            .id(("tray-row", r.id as usize))
            .debug_selector(move || format!("chip {id}"))
            .group("tray-row")
            .h(px(h))
            .w(px(w))
            .relative()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_px(m.meta)
            // The worker's fill, no frame, and its bar (the watched one's
            // is yours); hovered, its hover. The frame's room is padding,
            // in a layer of its own as the frame was laid out apart from
            // the padding in it (Shady #71 P2, P3): nothing in the chip
            // moved when the frame went, at any scale.
            .bg(p.card_agent)
            .px(px(crate::cards::frame_room(self.scale)))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl(px(12.))
                    .pr(px(10.))
                    .gap(px(8.))
                    .items_center()
                    .overflow_hidden()
                    .child(crate::turns::live_glyph(
                        &self.fx,
                        g,
                        mark,
                        p.card_agent,
                        self.fx.col(self.panes.cur.id).change(
                            Tag::TrayGlyph,
                            r.id,
                            crate::turns::glyph_code(glyph::worker(glyph::steady(r.live, r.demoted)).0),
                            None,
                        ),
                        None,
                        false,
                    ))
                    // Armed, `Cancel name?` says the name.
                    .when(!armed, |d| {
                        d.child(div().flex_none().text_color(p.sub_name_t).child(SharedString::from(cut(&r.name, NAME_CHARS))))
                    })
                    .when_some(armed_el, |d, a| d.child(a))
                    .when(!armed, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(step_color)
                                .child(SharedString::from(chip_step(r).to_string())),
                        )
                        // The clock's slot, as wide as it ever reads, and the ×'s
                        // after it (drawn on hover, design.md §5; always on the
                        // watched one), each there whether it shows or not: the
                        // step's room is the same in every state.
                        .child(
                            div()
                                .flex_none()
                                .w(px(crate::turns::text_w(CLOCK_CHARS, cw)))
                                .flex()
                                .justify_end()
                                .text_color(p.sub_time_t)
                                .children(clock.filter(|_| r.clocked())),
                        )
                        .child(div().flex_none().w(px(X_W)).flex().justify_center().children(x))
                    }),
            )
            .child(bar)
            .children(self.render_menu_at_row(r.id, p, cx));
        if !r.watchable {
            let tip = r.tip.clone();
            return row.tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx)).into_any_element();
        }
        let who2 = who.clone();
        // No card over an open menu.
        row.when(self.menu.is_none(), |d| d.tooltip(crate::wtabs::card_tip(cx.weak_entity(), who.clone(), r.tip.clone())))
            .cursor_pointer()
            .hover(|d| d.bg(p.hover_agent))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who, false, ev, window, cx)))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who2, false, ev, window, cx)))
            .into_any_element()
    }

    /// A chip that left, fading (`fade` 1 → 0) in its slot: what it said,
    /// with no id, listener or hitbox (motion.md §8: an exit takes no click).
    /// Its fill, text and marks fade into the canvas it sits on, as colour
    /// mixes (what opacity would paint there; no translucent text,
    /// design.md §5).
    fn tray_ghost(&self, r: &TrayRow, fade: f32, w: f32, h: f32, p: &Palette, m: &Metrics) -> AnyElement {
        let (g, _, _, family) = glyph::worker(r.live);
        let (word_color, mark) = glyph::ink(family, Ground::Run, p);
        let f = |c: Hsla| motion::mix(p.canvas, c, fade);
        h_flex()
            .debug_selector(|| "tray-ghost".into())
            .flex_none()
            .h(px(h))
            .w(px(w))
            .relative()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_px(m.meta)
            .bg(f(p.card_agent))
            .px(px(crate::cards::frame_room(self.scale)))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl(px(12.))
                    .pr(px(10.))
                    .gap(px(8.))
                    .items_center()
                    .overflow_hidden()
                    .child(glyph::draw_on(g, f(mark), f(p.card_agent)))
                    .child(div().flex_none().text_color(f(p.sub_name_t)).child(SharedString::from(cut(&r.name, NAME_CHARS))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_color(f(word_color))
                            .child(SharedString::from(chip_step(r).to_string())),
                    ),
            )
            .child(glyph::bar(f(p.sub_rule)))
            .into_any_element()
    }

    /// The NEEDS YOU stub (design.md §8.17, polish.md §2.6): the authorize
    /// card folded to one line over the chips while its row is not on
    /// screen: scrolled away, or a worker's view is (in place or in its
    /// tab: the card is never shown, nor answered, over a worker's view).
    /// One target, as Ctrl+J: back to the session, the card in view.
    pub(crate) fn render_stub(&self, p: &Palette, m: &Metrics, cx: &Context<Self>) -> Option<impl IntoElement> {
        if !self.stub_shown() {
            return None;
        }
        let lay = self.panes.cur.layout;
        // What waits: the card's gist (made once), as the top bar says it.
        let gist = self.current_card().map(|c| c.gist.clone()).or_else(|| {
            self.model.lives[self.panes.cur.live]
                .transcript
                .pending_prompts()
                .first()
                .map(|q| crate::chrome::prompt_gist(&q.title, &q.prompt).into())
        });
        let (word, mark) = glyph::ink(glyph::Family::Warn, Ground::Card, p);
        let row = h_flex()
            .id("needs-stub")
            .group("needs-stub")
            .debug_selector(|| "needs-stub".into())
            .h(px(m.row))
            .relative()
            .pl(px(m.pad_x - 1.))
            .pr(px(m.pad_x - 1.))
            .items_center()
            .whitespace_nowrap()
            .bg(p.card)
            .border_1()
            .border_color(p.warn_mark)
            .cursor_pointer()
            .hover(|d| d.bg(p.card_hi))
            .child(glyph::lead(Some(glyph::needs(glyph::needs_ping(&self.fx, true), mark, p.card)), m))
            .child(display(label::NEEDS_YOU, m.meta, word).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .ml(px(12.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.prose_k)
                    .when_some(gist.filter(|_| !lay.tight), |d, g| d.child(g)),
            )
            .child(mono("ctrl j", m.meta, p.dim_k).flex_none().ml(px(12.)).debug_selector(|| "needs-stub-key".into()))
            .child(div().flex_none().ml(px(12.)).child(glyph::reveal(glyph::GO, "needs-stub", "needs-stub", true, p.dim_k, p.ink)))
            .tooltip(|window, cx| crate::overlay::tip("A prompt waits in this session: Ctrl+J goes to it.", window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.jump_to_card(window, cx);
                cx.notify();
            }));
        Some(column(lay, row).flex_none().pt(px(crate::fit::STUB_GAP)))
    }

    /// The session waits on a prompt whose card is not on screen: a
    /// worker's view is, or its row is scrolled away.
    pub(crate) fn stub_shown(&self) -> bool {
        self.model.tabs.view().is_none()
            && !self.model.lives[self.panes.cur.live].transcript.pending_prompts().is_empty()
            && (self.model.lives[self.panes.cur.live].watch.focus().is_some() || !self.card_on_screen())
            && (self.panes.cur.stub_away.is_none() || self.panes.cur.stub_away != self.model.lives[self.panes.cur.live].card_key())
    }
}

#[cfg(test)]
mod tests {
    use super::{TrayRow, cut, grid, grid_pack, need};
    use crate::glyph::{self, Glyph};
    use crate::subagents::{Live, Liveness, Status};
    use crate::theme::Metrics;

    fn live(status: Status, line: &str) -> Live {
        Live {
            id: 3,
            call: None,
            name: "quick".into(),
            status,
            line: line.into(),
            tools: Some(1),
            elapsed: Some(40.0),
            tick: None,
            watchable: true,
            demoted: false,
        }
    }

    /// Chips go by need, not age: failed first, then the watched one, the
    /// running newest first, the ended last (newest first).
    #[test]
    fn chips_go_by_need() {
        let at = |id: u64, s: Status| TrayRow::of(Live { id, ..live(s, "x") }, false);
        let mut rows = [
            at(1, Status::Running),
            at(2, Status::Done),
            at(3, Status::Running),
            at(4, Status::Failed),
            at(5, Status::Running),
            at(6, Status::TimedOut),
            at(7, Status::Cancelled),
            at(8, Status::Cancelling),
        ];
        rows.sort_by_key(|r| need(r, Some(1)));
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [6, 4, 1, 8, 5, 3, 7, 2]);
        rows.sort_by_key(|r| need(r, None));
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [6, 4, 8, 5, 3, 1, 7, 2]);
        rows.sort_by_key(|r| need(r, Some(4)));
        assert_eq!(rows[0].id, 6, "a failed one watched is still a failure");
    }

    /// The grid: as many chips a line as are [`MIN_CHIP`] wide, each the
    /// line's share; two in the 1280-px window's column with both panels
    /// open (624) and in the capped column (672), one under 528.
    #[test]
    fn chips_are_the_columns_share() {
        let m = Metrics::at(16);
        assert_eq!(grid(624., &m), (2, 308.));
        assert_eq!(grid(672., &m), (2, 332.));
        assert_eq!(grid(536., &m), (2, 264.));
        assert_eq!(grid(527., &m), (1, 527.));
        assert_eq!(grid(1200., &m), (4, 294.));
        let (k, w) = grid(Metrics::at(20).column, &Metrics::at(20));
        assert_eq!((k, w), (2, 416.), "the least scales with the text");
    }

    /// Chips fill lines of `k`; past the lines they have, the last line
    /// ends with `+N more` after as many as fit beside it.
    #[test]
    fn chips_flow_in_lines_and_the_rest_is_more() {
        assert_eq!(grid_pack(3, 2, 308., 624., 2, 80.), (vec![0, 0, 1], 3));
        assert_eq!(grid_pack(4, 2, 308., 624., 2, 80.), (vec![0, 0, 1, 1], 4));
        assert_eq!(grid_pack(5, 2, 308., 624., 2, 80.), (vec![0, 0, 1], 3), "two lines: one beside +2 more");
        assert_eq!(grid_pack(5, 1, 527., 527., 2, 80.), (vec![0, 1], 2), "one a line: the last's one beside +3 more");
        assert_eq!(grid_pack(3, 1, 400., 400., 1, 80.), (vec![0], 1), "one line of one: a chip and +2 more (R2-L1)");
        assert_eq!(grid_pack(5, 2, 308., 624., 0, 80.), (vec![], 0));
        assert_eq!(cut("reading crates/relay/src/upstream.rs", 12), "reading cra…");
        assert_eq!(cut("short", 12), "short");
    }

    /// A row's glyph, word and step.
    fn shown(r: &TrayRow) -> (Glyph, &'static str, &str) {
        let (g, _, word, _) = glyph::worker(r.live);
        (g, word, r.step.as_ref())
    }

    /// A cancel on its way, then the rows' cancelling: `cancelling` until it
    /// has stopped; then `cancelled`. The step stays the worker's.
    #[test]
    fn a_cancel_says_cancelling_until_the_rows_say_cancelled() {
        let r = TrayRow::of(live(Status::Running, "bash"), true);
        assert_eq!((r.live, shown(&r)), (Liveness::Cancelling, (Glyph::Cancelling, "cancelling", "bash")));
        let r = TrayRow::of(live(Status::Cancelling, "bash"), false);
        assert_eq!(shown(&r), (Glyph::Cancelling, "cancelling", "bash"));
        let r = TrayRow::of(live(Status::Cancelled, ""), true);
        assert_eq!(shown(&r), (Glyph::Cancelled, "cancelled", ""));
        let r = TrayRow::of(live(Status::Done, "ok"), true);
        assert_eq!(shown(&r), (Glyph::Ok, "completed", "ok"), "ended: a cancel too late changes nothing");
        assert_eq!(shown(&TrayRow::of(live(Status::Running, "bash"), false)), (Glyph::Run, "running", "bash"));
    }

    /// A start the history alone holds is not running (design.md §8.4): the
    /// rest glyph and `started`, as the worker block says.
    #[test]
    fn a_started_worker_is_not_drawn_running() {
        let r = TrayRow::of(live(Status::Started, ""), false);
        assert_eq!((r.live, shown(&r)), (Liveness::Started, (Glyph::Idle, "started", "")));
        let block = glyph::worker(Liveness::of(Some(Status::Started), true, true));
        assert_eq!(glyph::worker(r.live), block, "the tray row and the block agree");
    }

    /// The tray's clock ticks on the frame from the daemon's last count,
    /// asking for its next second only, on the window's one second;
    /// Reduced and low power tick too (clocks are information), on the
    /// same second (Shady N5); Off holds the row's text and asks nothing.
    #[test]
    fn a_running_clock_ticks_on_the_frame_and_off_holds_it() {
        use crate::motion::{Level, Motion};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let fx = Motion::new(t0);
        let l = Live { tick: Some(crate::subagents::Tick::new(41.4, t0)), ..live(Status::Running, "bash") };
        let r = TrayRow::of(l, false);
        assert_eq!(r.facts.as_ref(), "1 tool · 40.0s", "the row's own text, as the daemon said");
        // A frame at 1.7 s reads the clock at its grid second (1 s: 42.4,
        // `42s`); its next second (43) at 1.6 s is on the grid's second
        // that crosses it (2 s: every clock's one app frame), at every level.
        for (level, low_power, at) in [(Level::Full, false, 2000), (Level::Reduced, false, 2000), (Level::Full, true, 2000)] {
            fx.begin_frame(level, low_power, t0 + Duration::from_millis(1700));
            assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 42s", "{level:?}");
            assert_eq!(fx.demand(), (false, Some(t0 + Duration::from_millis(at))), "{level:?} {low_power}: only its next second");
        }
        fx.begin_frame(Level::Off, false, t0 + Duration::from_secs(9));
        assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 40.0s");
        assert_eq!(fx.demand(), (false, None), "Off: no clock ticks");
    }

    /// Shady #17 M2: a clock whose second falls just after the grid's
    /// (2 ms) and a frame late by a refresh (5 ms): the frame asks for the
    /// very next grid second, and that one shows the next whole second.
    /// Read at the frame's own instant, it showed 43 at once, asked for
    /// 3 s, and 2 s was never drawn (the clock jumped two).
    #[test]
    fn a_late_frame_never_skips_a_clocks_second() {
        use crate::motion::{Level, Motion};
        use std::time::{Duration, Instant};
        let base = Instant::now();
        let t0 = base + Duration::from_secs(5);
        let ms = Duration::from_millis;
        let fx = Motion::new(t0);
        let l = Live { tick: Some(crate::subagents::Tick::new(41.998, t0)), ..live(Status::Running, "bash") };
        let r = TrayRow::of(l, false);
        fx.begin_frame(Level::Full, false, t0 + ms(1005));
        assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 42s");
        assert_eq!(fx.demand(), (false, Some(t0 + ms(2000))), "the next grid second, not the one after");
        fx.begin_frame(Level::Full, false, t0 + ms(2005));
        assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 43s", "one second on");
        assert_eq!(fx.demand(), (false, Some(t0 + ms(3000))));
        // The transcript's own clocks (a running call's) read the same second.
        fx.begin_frame(Level::Full, false, t0 + ms(3005));
        assert_eq!(crate::turns::clock(&fx, Some(base + ms(2))).as_deref(), Some("7s"), "as of 3 s: 7.998");
        assert_eq!(fx.demand(), (false, Some(t0 + ms(4000))));
    }

    /// Shady #17 R2-L1 (motion.md §5.3): the link lost at 2.4 s, every
    /// clock holds its count as of then, frame after frame, and asks for no
    /// second; back, they count from the time again.
    #[test]
    fn a_lost_link_holds_the_clocks() {
        use crate::motion::{Level, Motion};
        use std::time::{Duration, Instant};
        let t0 = Instant::now() + Duration::from_secs(5);
        let ms = Duration::from_millis;
        let fx = Motion::new(t0);
        let l = Live { tick: Some(crate::subagents::Tick::new(40., t0)), ..live(Status::Running, "bash") };
        let r = TrayRow::of(l, false);
        for at in [2500, 3000, 9000, 60_000] {
            fx.begin_frame(Level::Full, false, t0 + ms(at));
            fx.hold_still(Some(t0 + ms(2400)));
            assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 42s", "held at {at} ms");
            assert_eq!(super::chip_clock(&fx, &r).as_deref(), Some("42s"), "the chip, held at {at} ms");
            assert_eq!(crate::turns::clock(&fx, Some(t0)).as_deref(), Some("2s"), "a call's, held at {at} ms");
            assert_eq!(fx.demand(), (false, None), "no clock asks for a second at {at} ms");
        }
        fx.begin_frame(Level::Full, false, t0 + ms(61_000));
        assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 1m 41s", "back: the time again");
        assert_eq!(fx.demand(), (false, Some(t0 + ms(62_000))));
    }
}
