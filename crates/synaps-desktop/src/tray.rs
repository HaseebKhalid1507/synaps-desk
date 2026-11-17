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
        let step = cut(&l.line, STEP_CHARS).into();
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

/// A chip's step, at most, and its name. polish.md §2.5 says 32: at 24 two
/// chips share the 672 px column (a 10-character name, a cut step and a
/// clock come to ~325 px).
pub(crate) const STEP_CHARS: usize = 24;
const NAME_CHARS: usize = 18;

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
fn cut(s: &str, n: usize) -> String {
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
/// glyph, its name, its step and its clock (`clock` chars), 8 apart; the
/// cancel × shows over the clock on hover. An armed cancel's `Cancel
/// name? ··· C O N F I R M` takes the step's and the clock's room.
pub(crate) fn chip_w(r: &TrayRow, armed: bool, clock: usize, m: &Metrics) -> f32 {
    use crate::turns::text_w;
    let cw = m.meta * 0.6;
    let name = r.name.chars().count().min(NAME_CHARS);
    let mut w = 2. + 12. + 10. + 8. + text_w(name, cw) + 10.;
    if armed {
        // `Cancel ` and `?`, the fuse (3 cells 4 apart), CONFIRM (13 caps).
        w += 8. + text_w(name + 8, cw) + 6. + 20. + 8. + 13. * m.meta * 0.62;
    } else {
        w += 8. + text_w(chip_step(r).chars().count(), cw);
        if clock > 0 {
            w += 8. + text_w(clock.max(2), cw);
        } else if r.watchable {
            // The × still has its place on hover.
            w += 8. + 16.;
        }
    }
    w.ceil()
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

/// The chips of `widths` flowed left to right in lines `avail` px wide,
/// `gap` apart: the line each is on, as many as `lines` lines hold. All of
/// them, or as many as fit with the `+N more` chip (`more(n)` wide) that
/// ends the last line. A chip wider than a line has a line of its own.
pub(crate) fn chip_pack(widths: &[f32], avail: f32, gap: f32, lines: usize, more: impl Fn(usize) -> f32) -> (Vec<usize>, usize) {
    let flow = |ws: &mut dyn Iterator<Item = f32>| {
        let (mut line, mut x, mut out) = (0, 0., Vec::new());
        for w in ws {
            if x > 0. && x + gap + w > avail {
                line += 1;
                x = 0.;
            }
            x += if x > 0. { gap + w } else { w };
            out.push((line, x));
        }
        out
    };
    let all = flow(&mut widths.iter().copied());
    if lines == 0 {
        return (Vec::new(), 0);
    }
    if all.last().is_none_or(|l| l.0 < lines) {
        return (all.into_iter().map(|l| l.0).collect(), widths.len());
    }
    // As many as fit with `+N more` after them.
    for k in (0..widths.len()).rev() {
        let placed = flow(&mut widths[..k].iter().copied().chain(std::iter::once(more(widths.len() - k))));
        if placed.last().is_some_and(|l| l.0 < lines) {
            return (placed[..k].iter().map(|l| l.0).collect(), k);
        }
    }
    (Vec::new(), 0)
}

/// The lines `widths` flow to, all of them, in `avail` px.
pub(crate) fn chip_lines_needed(widths: &[f32], avail: f32, gap: f32) -> usize {
    chip_pack(widths, avail, gap, usize::MAX, |_| 0.).0.last().map_or(0, |l| l + 1)
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
        let rows: Vec<TrayRow> = rows
            .into_iter()
            .map(|l| {
                let p = pending(l.id);
                TrayRow::of(l, p)
            })
            .collect();
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
    pub(crate) fn chip_widths(&self, m: &Metrics) -> (Vec<Slot>, Vec<f32>) {
        let session = self.model.active.as_ref().map(|a| a.session_id.as_str());
        let now = Instant::now();
        let slots = self.tray_slots();
        let widths = slots
            .iter()
            .map(|s| match *s {
                Slot::Row(i) => {
                    let r = &self.tray.rows[i];
                    let armed = session.is_some_and(|s| self.model.cancels.armed(s, r.id, (r.call.as_deref(), &r.name), now));
                    chip_w(r, armed, chip_clock(&self.fx, r).map_or(0, |c| c.chars().count()), m)
                }
                Slot::Ghost(g) => {
                    let r = &self.tray.ghosts[g].1;
                    chip_w(r, false, r.clock.as_ref().map_or(0, |c| c.chars().count()), m)
                }
            })
            .collect();
        (slots, widths)
    }

    pub(crate) fn pack_chips(&mut self, slots: Vec<Slot>, widths: &[f32], avail: f32, lines: usize, m: &Metrics) {
        let (on, k) = chip_pack(widths, avail, CHIP_GAP, lines, |n| more_w(n, m));
        let placed: Vec<(Slot, usize, f32)> = slots.into_iter().zip(on).zip(widths).map(|((s, l), w)| (s, l, *w)).collect();
        let drawn = placed.iter().filter(|s| matches!(s.0, Slot::Row(_))).count();
        let more = self.tray.rows.len().saturating_sub(drawn);
        let more_line = placed.last().map_or(0, |s| s.1);
        let used = if k == widths.len() { placed.last().map_or(0, |s| s.1 + 1) } else { lines };
        self.chips = Chips { slots: placed, more: if k < widths.len() { more.max(1) } else { 0 }, more_line, lines: used };
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
                .bg(p.tool)
                .border_1()
                .border_color(p.hair)
                .child(mono(format!("+{} more", chips.more), m.meta, p.dim_t))
                .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx));
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

    /// A worker's chip (polish.md §2.5): on the `tool` ground in a 1 px
    /// `hair` frame, `bar` on its left edge; its state glyph, name, step
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
        // The × at the chip's end, over the clock: drawn on hover (a reveal,
        // design.md §5; always on the watched chip), on the hover's ground.
        let x = x.map(|x| {
            div()
                .absolute()
                .right(px(4.))
                .top_0()
                .bottom_0()
                .flex()
                .items_center()
                .when(watched, |d| d.bg(p.tool_out))
                .group_hover("tray-row", |s| s.bg(p.tool_out))
                .child(x)
        });
        let row =
            h_flex()
                .id(("tray-row", r.id as usize))
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
                .bg(if watched { p.tool_out } else { p.tool })
                .border_1()
                .border_color(p.hair)
                .child(crate::turns::live_glyph(
                    &self.fx,
                    g,
                    mark,
                    p.tool,
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
                    .children(clock.map(|c| div().flex_none().text_color(p.sub_time_t).child(c)))
                })
                .children(x)
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
            .when(!watched, |d| d.hover(|d| d.bg(p.tool_out)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who, false, ev, window, cx)))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who2, false, ev, window, cx)))
            .into_any_element()
    }

    /// A chip that left, fading (`fade` 1 → 0) in its slot: what it said,
    /// with no id, listener or hitbox (motion.md §8: an exit takes no click).
    /// Faded as a colour mix toward the chip's opaque ground (what opacity
    /// would paint there; no translucent text, design.md §5).
    fn tray_ghost(&self, r: &TrayRow, fade: f32, w: f32, h: f32, p: &Palette, m: &Metrics) -> AnyElement {
        let (g, _, _, family) = glyph::worker(r.live);
        let (word_color, mark) = glyph::ink(family, Ground::Run, p);
        let f = |c: Hsla| motion::mix(p.tool, c, fade);
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
            .bg(p.tool)
            .border_1()
            .border_color(f(p.hair))
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
    use super::{TrayRow, chip_lines_needed, chip_pack, cut};
    use crate::glyph::{self, Glyph};
    use crate::subagents::{Live, Liveness, Status};

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

    /// Chips flow left to right in lines; past the lines they have, the
    /// last line ends with `+N more` (N the chips not drawn); a chip wider
    /// than a line has its own.
    #[test]
    fn chips_flow_in_lines_and_the_rest_is_more() {
        let more = |_| 50.;
        assert_eq!(chip_pack(&[100., 100., 100.], 320., 8., 2, more), (vec![0, 0, 0], 3), "one line holds all three");
        assert_eq!(chip_pack(&[200., 200., 200.], 320., 8., 2, more), (vec![0, 1], 2), "two lines: the third is +1 more after the second");
        assert_eq!(chip_pack(&[100., 100., 100., 100.], 320., 8., 1, more), (vec![0, 0], 2), "one line: two and +2 more");
        assert_eq!(chip_pack(&[500., 100.], 320., 8., 2, more), (vec![0, 1], 2), "a wide one has its line");
        assert_eq!(chip_lines_needed(&[200., 200., 200.], 320., 8.), 3);
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
