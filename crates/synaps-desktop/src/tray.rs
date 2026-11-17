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
use crate::glyph::{self, Glyph, Ground};
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
            let (s, next) = t.whole(fx.now());
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
    key: Option<(u64, Option<u64>, bool, u64, u64)>,
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

/// Between chips, and between their lines.
pub(crate) const CHIP_GAP: f32 = 8.;

/// A chip's step, at most, and its name. The demo's steps run 20–36
/// characters (median 31: `reading e2e/checkout/cart.spec.ts` is 33): at
/// 36 they read whole (polish.md §2.5 said 32) while every worker fits;
/// a crowded tray cuts them shorter first ([`STEP_CAPS`]).
pub(crate) const STEP_CHARS: usize = 36;
pub(crate) const NAME_CHARS: usize = 18;
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
            let (s, next) = t.whole(fx.now());
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

/// How the chips flow over the composer this frame ([`chip_pack`]): the
/// slots drawn and the line each is on, the workers not drawn (`+N more`,
/// on the last line) and how many lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Chips {
    pub(crate) slots: Vec<(Slot, usize, f32)>,
    pub(crate) more: usize,
    pub(crate) more_line: usize,
    pub(crate) lines: usize,
}

impl Chips {
    /// The workers' chips drawn (not ghosts).
    pub(crate) fn drawn(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.0, Slot::Row(_))).count()
    }
}

/// The parts' widths of a chip (mono at `m.meta`, every text box a whole
/// pixel): its 1 px frame, padding 12 (the bar and 10) and 10, the
/// glyph, its name, its step (cut at [`STEP_CHARS`]), its clock's slot
/// and the cancel ×'s, 8 apart. Nothing in it changes with a tick. An
/// armed cancel's `Cancel name? ··· C O N F I R M` (CONFIRM `confirm` px,
/// as drawn) takes the step's, the clock's and the ×'s room. No wider than
/// the line (`line` px): the step gives way. With the least it may shrink
/// to in a line's rest: its step cut at [`STEP_LEAST`].
pub(crate) fn chip_w(r: &TrayRow, armed: bool, line: f32, confirm: f32, cap: usize, m: &Metrics) -> (f32, f32) {
    use crate::turns::text_w;
    let cw = m.meta * 0.6;
    let name = r.name.chars().count().min(NAME_CHARS);
    let base = 2. + 12. + 10. + 8. + text_w(name, cw) + 10.;
    if armed {
        // `Cancel ` and `?`, the fuse (3 cells 4 apart, 2 after), CONFIRM,
        // 6 apart.
        let w = (base + 8. + text_w(name + 8, cw) + 6. + 22. + 6. + confirm).ceil();
        return (w, w);
    }
    let fixed = base + if r.clocked() { 8. + text_w(CLOCK_CHARS, cw) } else { 0. } + if r.watchable { 8. + X_W } else { 0. };
    let room = (line - fixed - 8.).max(text_w(8, cw));
    let step = chip_step(r).chars().count().min(cap);
    let w = fixed + 8. + text_w(step, cw).min(room);
    let least = fixed + 8. + text_w(step.min(STEP_LEAST), cw).min(room);
    (w.ceil(), least.ceil())
}

/// The least of a step a chip keeps when it shrinks into a line's rest.
const STEP_LEAST: usize = 14;

/// The step caps tried in turn: whole steps while every worker fits, then
/// shorter ones before a worker goes into `+N more`.
const STEP_CAPS: [usize; 3] = [STEP_CHARS, 24, 16];

/// The step cap the chips are drawn at and the lines they take: the first
/// of [`STEP_CAPS`] at which every chip fits the lines `lines(needed)`
/// leaves (`widths(cap)`, `+N more` `more(n)` wide), else the shortest.
pub(crate) fn fit_caps(
    widths: impl Fn(usize) -> Vec<(f32, f32)>,
    line: f32,
    lines: impl Fn(usize) -> usize,
    more: impl Fn(usize) -> f32,
) -> (usize, usize) {
    let mut pick = (STEP_CHARS, 0);
    for cap in STEP_CAPS {
        let ws = widths(cap);
        let n = lines(chip_lines_needed(&ws, line, CHIP_GAP));
        pick = (cap, n);
        if chip_pack(&ws, line, CHIP_GAP, n, &more).1 == ws.len() {
            break;
        }
    }
    pick
}

/// A chip's step: what it does while it runs, else its state's word.
fn chip_step(r: &TrayRow) -> &str {
    match r.live {
        Liveness::Running | Liveness::Started if !r.step.is_empty() => &r.step,
        _ => glyph::worker(r.live).2,
    }
}

/// `+N more`'s chip width.
pub(crate) fn more_w(n: usize, m: &Metrics) -> f32 {
    (2. + 10. + crate::turns::text_w(format!("+{n} more").chars().count(), m.meta * 0.6) + 10.).ceil()
}

/// The chips of `widths` (each whole, and the least it may shrink to: its
/// step cut shorter) flowed left to right in lines `avail` px wide, `gap`
/// apart: the line each is on and its width, as many as `lines` lines hold.
/// A chip that does not fit whole in what is left of a line takes the rest
/// of it if it can shrink that far, else starts the next line. All of
/// them, or as many as fit with the `+N more` chip (`more(n)` wide) that
/// ends the last line. A chip wider than a line has a line of its own.
pub(crate) fn chip_pack(
    widths: &[(f32, f32)],
    avail: f32,
    gap: f32,
    lines: usize,
    more: impl Fn(usize) -> f32,
) -> (Vec<(usize, f32)>, usize) {
    let flow = |ws: &mut dyn Iterator<Item = (f32, f32)>| {
        let (mut line, mut x, mut out) = (0, 0., Vec::new());
        for (w, least) in ws {
            let mut w = w;
            if x > 0. && x + gap + w > avail {
                if x + gap + least <= avail {
                    w = avail - x - gap;
                } else {
                    line += 1;
                    x = 0.;
                }
            }
            x += if x > 0. { gap + w } else { w };
            out.push((line, w));
        }
        out
    };
    let all = flow(&mut widths.iter().copied());
    if lines == 0 {
        return (Vec::new(), 0);
    }
    if all.last().is_none_or(|l| l.0 < lines) {
        return (all, widths.len());
    }
    // As many as fit with `+N more` after them.
    for k in (0..widths.len()).rev() {
        let m = more(widths.len() - k);
        let placed = flow(&mut widths[..k].iter().copied().chain(std::iter::once((m, m))));
        if placed.last().is_some_and(|l| l.0 < lines) {
            return (placed[..k].to_vec(), k);
        }
    }
    (Vec::new(), 0)
}

/// The lines `widths` flow to, all of them, in `avail` px.
pub(crate) fn chip_lines_needed(widths: &[(f32, f32)], avail: f32, gap: f32) -> usize {
    chip_pack(widths, avail, gap, usize::MAX, |_| 0.).0.last().map_or(0, |l| l.0 + 1)
}

impl AppView {
    /// The tray's rows, rebuilt if a worker changed; the timer that drops
    /// a finished worker's row at the end of its flash.
    pub(crate) fn sync_tray(&mut self, cx: &mut Context<Self>) {
        let attached = self.model.active.is_some();
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
        if let Some(s) = self.model.active.as_ref().map(|a| a.session_id.as_str()) {
            let subs = &self.model.stats.subagents;
            let said = |h: &str| {
                let id = h.strip_prefix("sa_").and_then(|n| n.parse().ok());
                id.and_then(|id| subs.status(id)).is_some_and(|st| st == Status::Cancelling || st.terminal())
            };
            self.model.cancels.settle(s, said, now);
        }
        let key = (
            self.model.stats.subagents.revision(),
            self.model.watch.focused().map(|w| w.subagent_id),
            attached,
            self.model.cancels.generation,
            // A worker tab closed: its view and list go now.
            self.model.tabs.generation,
        );
        if self.tray.key == Some(key) && self.tray.until.is_none_or(|u| now < u) {
            return;
        }
        let live = if attached { self.model.stats.subagents.live() } else { Vec::new() };
        // A view nobody looks at (on screen, or held by a worker tab) whose
        // worker ended goes (and leaves the walk), as do the views of the
        // worker tabs past the most recently shown few, and their lists.
        let held = self.model.held_views();
        self.model.watch.hold(held);
        self.model.watch.prune(now, &live);
        self.drop_released_panes();
        let (rows, until) = self.model.watch.tray(live, now);
        let cancels = &self.model.cancels;
        let session = self.model.active.as_ref().map(|a| a.session_id.as_str());
        let pending = |id: u64| session.is_some_and(|s| cancels.pending(s, &format!("sa_{id}")));
        let mut rows: Vec<TrayRow> = rows
            .into_iter()
            .map(|l| {
                let p = pending(l.id);
                TrayRow::of(l, p)
            })
            .collect();
        let watched = self.model.watch.focused().map(|w| w.subagent_id);
        rows.sort_by_key(|r| need(r, watched));
        // Rows that came while watched draw in (staggered); rows that left fade out.
        let session = self.model.active.as_ref().map(|a| a.session_id.clone());
        let mut ghosts = std::mem::take(&mut self.tray.ghosts);
        if self.fx.on() && session.is_some() && session == self.tray.session {
            let fresh = rows.iter().filter(|r| !self.tray.rows.iter().any(|o| o.id == r.id));
            for (k, r) in fresh.enumerate() {
                self.fx.born(TRAY | r.id, fx_now + motion::stagger(k));
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
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(u.saturating_duration_since(Instant::now())).await;
                    let _ = this.update(cx, |v, cx| {
                        v.tray_due = None;
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
        let cwd = self.model.cwd().map(str::to_string);
        let c = Candidate::of(who.id, &who.name).with_call(who.call.as_deref());
        self.model.watch.open(&c, cwd.as_deref(), Instant::now());
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

    /// The chips over the composer this frame: their widths, the lines they
    /// need in the column (`avail` px) and, once [`crate::fit::chip_lines`]
    /// has said how many it has room for (`lines`), how they flow
    /// ([`AppView::chips`]).
    /// Fits the chips into the column ([`fit_caps`]); the lines used.
    pub(crate) fn fit_chips(&mut self, line: f32, confirm: f32, m: &Metrics, lines: impl Fn(usize) -> usize) -> usize {
        let slots = self.tray_slots();
        let (cap, n) = fit_caps(|cap| self.chip_widths(line, confirm, cap, m).1, line, lines, |k| more_w(k, m));
        let (_, widths) = self.chip_widths(line, confirm, cap, m);
        self.pack_chips(slots, &widths, line, n, m);
        n
    }

    pub(crate) fn chip_widths(&self, line: f32, confirm: f32, cap: usize, m: &Metrics) -> (Vec<Slot>, Vec<(f32, f32)>) {
        let session = self.model.active.as_ref().map(|a| a.session_id.as_str());
        let now = Instant::now();
        let slots = self.tray_slots();
        let widths = slots
            .iter()
            .map(|s| match *s {
                Slot::Row(i) => {
                    let r = &self.tray.rows[i];
                    let armed = session.is_some_and(|s| self.model.cancels.armed(s, r.id, (r.call.as_deref(), &r.name), now));
                    chip_w(r, armed, line, confirm, cap, m)
                }
                Slot::Ghost(g) => chip_w(&self.tray.ghosts[g].1, false, line, confirm, cap, m),
            })
            .collect();
        (slots, widths)
    }

    pub(crate) fn pack_chips(&mut self, slots: Vec<Slot>, widths: &[(f32, f32)], avail: f32, lines: usize, m: &Metrics) {
        let (on, k) = chip_pack(widths, avail, CHIP_GAP, lines, |n| more_w(n, m));
        let placed: Vec<(Slot, usize, f32)> = slots.into_iter().zip(on).map(|(s, (l, w))| (s, l, w)).collect();
        let drawn = placed.iter().filter(|s| matches!(s.0, Slot::Row(_))).count();
        let more = self.tray.rows.len().saturating_sub(drawn);
        let more_line = placed.last().map_or(0, |s| s.1);
        let used = if k == widths.len() { placed.last().map_or(0, |s| s.1 + 1) } else { lines };
        // Only ghosts cut: no worker is left out, and nothing says so.
        self.chips = Chips { slots: placed, more: if k < widths.len() { more } else { 0 }, more_line, lines: used };
    }

    /// The workers menu (polish.md §2.5): `+N more`'s workers, each as its
    /// chip would say it (name, state, step); picked, its view in place.
    pub(crate) fn open_workers_menu(&mut self, pt: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<crate::menu::Item<u64>> = self.tray.rows[self.tray_drawn().min(self.tray.rows.len())..]
            .iter()
            .map(|r| {
                let item =
                    crate::menu::Item::new(r.id, format!("{}  {} · {}", r.name, glyph::worker(r.live).2, cut(chip_step(r), MENU_STEP)));
                if r.watchable { item } else { item.off(r.tip.clone()) }
            })
            .collect();
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
        self.wmenu = Some(crate::menu::Menu::new(items, crate::menu::At::Point(pt), false));
        cx.notify();
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
    pub(crate) fn render_workers_menu(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let m = self.wmenu.as_ref()?;
        let crate::menu::At::Point(pt) = m.at else { return None };
        let body = crate::menu::render("workers-menu", m, p, AppView::workers_menu_pick, |this, _| this.close_menu(), cx);
        Some(deferred(anchored().position(pt).snap_to_window_with_margin(px(8.)).child(body)).with_priority(3))
    }

    /// A menu opened from the keyboard over a tray row whose row is not
    /// drawn any more (its worker dropped, the window got short): it
    /// closes, never left taking keys unseen.
    pub(crate) fn close_orphan_menu(&mut self) {
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
        let lay = self.layout;
        let cramped = lay.form() == crate::fit::Form::Cramped;
        let watched = self.model.watch.focused().map(|w| w.subagent_id);
        let row_h = crate::fit::tray_row(m);
        let now = Instant::now();
        let session = self.model.active.as_ref().map(|a| a.session_id.as_str());
        let fx = &self.fx;
        // The watched chip's bar draws when its view opens here, and lights
        // after the composer's when you steer it (Handoff out).
        let watch_at = fx.change(Tag::Watched, 0, watched.unwrap_or(0), None).filter(|_| watched.is_some()).map(|c| c.0);
        let you_drawn = fx.draw(watch_at, std::time::Duration::ZERO, motion::QUICK);
        let steer = fx.flash(fx.sent.get().filter(|s| s.1 && watched.is_some()).map(|s| s.0 + 2 * motion::STAGGER));
        let you_color = motion::mix(p.you_mark_t, motion::mix(p.you, p.ink, 0.35), steer);
        let mut lines: Vec<Vec<AnyElement>> = (0..chips.lines).map(|_| Vec::new()).collect();
        for &(slot, line, w) in &chips.slots {
            let Some(at) = lines.get_mut(line) else { continue };
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
                    let armed = session.is_some_and(|s| self.model.cancels.armed(s, r.id, (r.call.as_deref(), &r.name), now));
                    let is_watched = watched == Some(r.id);
                    // Its bar: yours when watched, else the workers' rule,
                    // drawn top down when it came.
                    let bar = if is_watched {
                        glyph::bar_at(you_color, you_drawn)
                    } else {
                        glyph::bar_at(p.sub_rule, fx.draw(fx.birth(TRAY | r.id), std::time::Duration::ZERO, motion::QUICK))
                    };
                    // A chip that changed line slides from its last place.
                    let off = fx.slide(TRAY | r.id, line as f32 * (row_h + CHIP_GAP));
                    at.push(
                        div()
                            .relative()
                            .top(px(off))
                            .flex_none()
                            .child(self.tray_row(r, is_watched, armed, w, row_h, bar, p, m, cx))
                            .into_any_element(),
                    );
                }
            }
        }
        if chips.more > 0 {
            let rest: Vec<String> =
                self.tray.rows.iter().skip(chips.drawn()).map(|r| format!("{}  {}", r.name, glyph::worker(r.live).2)).collect();
            let tip = SharedString::from(rest.join("\n"));
            let more = h_flex()
                .id("tray-more")
                .debug_selector(|| "tray-more".into())
                .flex_none()
                .h(px(row_h))
                .px(px(10.))
                .items_center()
                .border_1()
                .border_color(p.hair_t)
                .cursor_pointer()
                .hover(|d| d.bg(p.tool_out))
                .child(mono(format!("+{} more", chips.more), m.meta, p.dim_t))
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

    /// A worker's chip (polish.md §2.5, made lighter at the finish gate):
    /// no fill, a 1 px `hair_t` frame on the canvas, `bar` on its left edge; its state glyph, name, step
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
        let cw = m.meta * 0.6;
        let id = r.id;
        let row = h_flex()
            .id(("tray-row", r.id as usize))
            .debug_selector(move || format!("chip {id}"))
            .group("tray-row")
            .h(px(h))
            .w(px(w))
            .relative()
            .pl(px(12.))
            .pr(px(10.))
            .gap(px(8.))
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_px(m.meta)
            // No fill: a 1 px frame and its bar on the canvas (the watched
            // one's bar is yours); hovered, the rows' quiet fill.
            .border_1()
            .border_color(p.hair_t)
            .child(crate::turns::live_glyph(
                &self.fx,
                g,
                mark,
                p.canvas,
                self.fx.change(Tag::TrayGlyph, r.id, crate::turns::glyph_code(glyph::worker(glyph::steady(r.live, r.demoted)).0), None),
                None,
                false,
            ))
            .child(div().flex_none().text_color(p.sub_name_t).child(SharedString::from(cut(&r.name, NAME_CHARS))))
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
                // The clock's slot, as wide as it ever reads; the ×'s after
                // it (drawn on hover, design.md §5; always on the watched one).
                .when(r.clocked(), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .w(px(crate::turns::text_w(CLOCK_CHARS, cw)))
                            .flex()
                            .justify_end()
                            .text_color(p.sub_time_t)
                            .children(clock),
                    )
                })
                .when(r.watchable, |d| d.child(div().flex_none().w(px(X_W)).flex().justify_center().children(x)))
            })
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
            .hover(|d| d.bg(p.tool_out))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who, false, ev, window, cx)))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who2, false, ev, window, cx)))
            .into_any_element()
    }

    /// A chip that left, fading (`fade` 1 → 0) in its slot: what it said,
    /// with no id, listener or hitbox (motion.md §8: an exit takes no click).
    /// Its frame, text and marks fade into the canvas it sits on, as colour
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
            .pl(px(12.))
            .pr(px(10.))
            .gap(px(8.))
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_px(m.meta)
            .border_1()
            .border_color(f(p.hair_t))
            .child(glyph::draw(g, f(mark)))
            .child(div().flex_none().text_color(f(p.sub_name_t)).child(SharedString::from(cut(&r.name, NAME_CHARS))))
            .child(div().flex_1().min_w_0().overflow_hidden().text_color(f(word_color)).child(SharedString::from(chip_step(r).to_string())))
            .child(glyph::bar(f(p.sub_rule)))
            .into_any_element()
    }

    /// The NEEDS YOU stub (design.md §8.17): the authorization card folded
    /// to its header line, where the card docks, while a worker's view is
    /// on screen (in place or in its tab) and its session waits on a prompt
    /// (the card is never shown, nor answered, over a worker's view). One
    /// target: it goes back to the session with the card up.
    pub(crate) fn render_stub(&self, p: &Palette, m: &Metrics, cx: &Context<Self>) -> Option<impl IntoElement> {
        if !self.stub_shown() {
            return None;
        }
        let lay = self.layout;
        // What waits: the card's gist (made once), as the top bar says it.
        let gist = self
            .current_card()
            .map(|c| c.gist.clone())
            .or_else(|| self.model.transcript.pending_prompts().first().map(|q| crate::chrome::prompt_gist(&q.title, &q.prompt).into()));
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
            .child(glyph::lead(Some(glyph::draw(Glyph::Needs, mark)), m))
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
            .child(div().flex_none().ml(px(12.)).child(glyph::reveal(glyph::GO, "needs-stub", "needs-stub", true, p.dim_k, p.ink)))
            .tooltip(|window, cx| crate::overlay::tip("A prompt waits in this session: answer it there.", window, cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_parent_card(cx);
                cx.notify();
            }));
        Some(column(lay, row).flex_none().pt(px(crate::fit::STUB_GAP)))
    }

    /// A worker's view is on screen and its session waits on a prompt.
    pub(crate) fn stub_shown(&self) -> bool {
        self.model.tabs.view().is_none() && self.model.watch.focus().is_some() && !self.model.transcript.pending_prompts().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{CHIP_GAP, STEP_CHARS, TrayRow, chip_lines_needed, chip_pack, chip_w, cut, fit_caps, need};
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

    /// Steps read whole while every worker fits; a crowded tray cuts them
    /// shorter before a worker goes into `+N more`.
    #[test]
    fn a_crowded_tray_cuts_steps_before_hiding_workers() {
        let m = Metrics::at(16);
        let rows: Vec<TrayRow> = (0..6).map(|_| TrayRow::of(live(Status::Running, "reading e2e/checkout/cart.spec.ts"), false)).collect();
        let (rows, m) = (&rows, &m);
        let at = |k: usize| move |cap| rows[..k].iter().map(|r| chip_w(r, false, m.column, 60., cap, m)).collect::<Vec<_>>();
        let more = |_| 50.;
        assert_eq!(fit_caps(at(2), m.column, |n| n.min(2), more).0, STEP_CHARS, "two read whole");
        let (cap, n) = fit_caps(at(6), m.column, |n| n.min(2), more);
        assert!(cap < STEP_CHARS && n <= 2, "six in two lines: shorter steps ({cap}, {n})");
        let shown = |cap| chip_pack(&at(6)(cap), m.column, CHIP_GAP, 2, more).1;
        let (cut, whole) = (shown(cap), shown(STEP_CHARS));
        assert!(cut > whole, "more workers shown: {cut} > {whole}");
    }

    /// Chips flow left to right in lines; past the lines they have, the
    /// last line ends with `+N more` (N the chips not drawn); a chip wider
    /// than a line has its own.
    #[test]
    fn chips_flow_in_lines_and_the_rest_is_more() {
        let more = |_| 50.;
        let whole = |ws: &[f32]| ws.iter().map(|&w| (w, w)).collect::<Vec<_>>();
        let lines = |r: (Vec<(usize, f32)>, usize)| (r.0.into_iter().map(|l| l.0).collect::<Vec<_>>(), r.1);
        assert_eq!(lines(chip_pack(&whole(&[100., 100., 100.]), 320., 8., 2, more)), (vec![0, 0, 0], 3), "one line holds all three");
        assert_eq!(
            lines(chip_pack(&whole(&[200., 200., 200.]), 320., 8., 2, more)),
            (vec![0, 1], 2),
            "two lines: the third is +1 more after the second"
        );
        assert_eq!(lines(chip_pack(&whole(&[100., 100., 100., 100.]), 320., 8., 1, more)), (vec![0, 0], 2), "one line: two and +2 more");
        assert_eq!(lines(chip_pack(&whole(&[500., 100.]), 320., 8., 2, more)), (vec![0, 1], 2), "a wide one has its line");
        // A chip that can shrink takes the line's rest; one that cannot
        // starts the next.
        assert_eq!(chip_pack(&[(200., 200.), (200., 100.)], 320., 8., 2, more), (vec![(0, 200.), (0, 112.)], 2));
        assert_eq!(chip_pack(&[(200., 200.), (200., 150.)], 320., 8., 2, more), (vec![(0, 200.), (1, 200.)], 2));
        assert_eq!(chip_lines_needed(&whole(&[200., 200., 200.]), 320., 8.), 3);
        assert_eq!(chip_lines_needed(&[], 320., 8.), 0);
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
        // Its next second (44) at 2.6 s: on the grid's second that crosses
        // it (3 s: every clock's one app frame), at every level.
        for (level, low_power, at) in [(Level::Full, false, 3000), (Level::Reduced, false, 3000), (Level::Full, true, 3000)] {
            fx.begin_frame(level, low_power, t0 + Duration::from_millis(1700));
            assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 43s", "{level:?}");
            assert_eq!(fx.demand(), (false, Some(t0 + Duration::from_millis(at))), "{level:?} {low_power}: only its next second");
        }
        fx.begin_frame(Level::Off, false, t0 + Duration::from_secs(9));
        assert_eq!(super::ticked(&fx, &r.facts, &r.tick).as_ref(), "1 tool · 40.0s");
        assert_eq!(fx.demand(), (false, None), "Off: no clock ticks");
    }
}
