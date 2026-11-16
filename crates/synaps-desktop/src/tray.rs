//! The tray of workers over the composer (the TUI's subagent tray), and a
//! worker's view's breadcrumb. One row per worker, in the grammar of the
//! transcript's worker block (design.md §8.4): its state glyph, name, what
//! it is doing (or ended with), tools and elapsed time, its state word; at
//! most what
//! [`crate::fit::tray_rows`] leaves room for, then "+N more". A finished
//! worker shows how it ended for [`crate::watch::FLASH`], then drops.
//! Nothing moves: no spinner; the elapsed time moves when a row comes (1 Hz
//! while a worker runs). A click on a row opens that worker's view; the
//! middle button, Ctrl/Cmd+click or a double-click open it in a tab, the
//! right button its menu, and a hovered row shows its card and a × that
//! cancels it ([`crate::wtabs`]). A cancel on its way says `cancelling`.
//!
//! Built from the attached session's workers ([`crate::subagents`]) only
//! when they change: no row is formatted per frame (each frame's rows hold
//! their click handlers, which share the row's strings).

use std::time::Instant;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, WORKER_KEY, column, display, mono, tbtn};
use crate::fonts::{self, label};
use crate::glyph::{self, Ground};
use crate::menu::At;
use crate::subagents::{Live, Liveness, Status, fmt_secs};
use crate::theme::{Metrics, Palette};
use crate::watch::Candidate;
use crate::wtabs::Who;

/// A row's state mark (the tray lists the attached session's workers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Its start is all that is known: not running.
    Started,
    Run,
    Cancelling,
    Ok,
    Failed,
    TimedOut,
    Cancelled,
}

impl Mark {
    fn of(s: Status) -> Self {
        match s {
            Status::Cancelling => Mark::Cancelling,
            Status::Done => Mark::Ok,
            Status::Failed => Mark::Failed,
            Status::TimedOut => Mark::TimedOut,
            Status::Cancelled => Mark::Cancelled,
            Status::Running => Mark::Run,
            Status::Started => Mark::Started,
        }
    }

    pub(crate) fn liveness(self) -> Liveness {
        match self {
            Mark::Started => Liveness::Started,
            Mark::Run => Liveness::Running,
            Mark::Cancelling => Liveness::Cancelling,
            Mark::Ok => Liveness::Done,
            Mark::Failed => Liveness::Failed,
            Mark::TimedOut => Liveness::TimedOut,
            Mark::Cancelled => Liveness::Cancelled,
        }
    }
}

/// One tray row, formatted once per change.
#[derive(Clone, Debug)]
pub(crate) struct TrayRow {
    pub(crate) id: u64,
    /// `sa_N`.
    pub(crate) handle: SharedString,
    pub(crate) name: SharedString,
    /// Its call, if that call's own result named it ([`Who`]).
    pub(crate) call: Option<SharedString>,
    pub(crate) mark: Mark,
    pub(crate) line: SharedString,
    pub(crate) facts: SharedString,
    pub(crate) watchable: bool,
    pub(crate) tip: SharedString,
}

impl TrayRow {
    /// `cancelling`: a cancel of it is on its way (its rows have not said
    /// so): its word says cancelling until it has stopped.
    fn of(mut l: Live, cancelling: bool) -> Self {
        if cancelling && l.status == Status::Running {
            l.status = Status::Cancelling;
        }
        let mut facts = Vec::new();
        if let Some(n) = l.tools.filter(|&n| n > 0) {
            facts.push(format!("{n} tool{}", if n == 1 { "" } else { "s" }));
        }
        if let Some(s) = l.elapsed {
            facts.push(fmt_secs(s));
        }
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
            mark: Mark::of(l.status),
            line: l.line.into(),
            facts: facts.join(" · ").into(),
            watchable: l.watchable,
            tip: tip.into(),
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
}

impl AppView {
    /// The tray's rows, rebuilt if a worker changed; the timer that drops
    /// a finished worker's row at the end of its flash.
    pub(crate) fn sync_tray(&mut self, cx: &mut Context<Self>) {
        let attached = self.model.active.is_some();
        let now = Instant::now();
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
        let rows = rows
            .into_iter()
            .map(|l| {
                let p = pending(l.id);
                TrayRow::of(l, p)
            })
            .collect();
        self.tray = TrayCache { key: Some(key), rows, until };
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

    /// How many tray rows are drawn (the rest are "+N more").
    pub(crate) fn tray_drawn(&self) -> usize {
        let shown = self.tray_rows.min(self.tray.rows.len());
        let cramped = self.layout.form() == crate::fit::Form::Cramped;
        match (self.tray.rows.len() > shown, cramped) {
            (false, _) => shown,
            (true, true) => shown.min(1),
            (true, false) => shown.saturating_sub(1),
        }
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

    /// The tray over the composer: [`AppView::tray_rows`] rows.
    pub(crate) fn render_tray(&self, p: &Palette, m: &Metrics, cx: &Context<Self>) -> Option<impl IntoElement> {
        let shown = self.tray_rows.min(self.tray.rows.len());
        if shown == 0 {
            return None;
        }
        let lay = self.layout;
        let cramped = lay.form() == crate::fit::Form::Cramped;
        let all = self.tray.rows.len();
        // Fewer rows than workers: the last row says how many more (cramped:
        // beside the one row).
        let workers = self.tray_drawn();
        let more = all - workers;
        let watched = self.model.watch.focused().map(|w| w.subagent_id);
        let row_h = crate::fit::tray_row(m);
        let mut list = v_flex();
        let now = Instant::now();
        let session = self.model.active.as_ref().map(|a| a.session_id.as_str());
        for (i, r) in self.tray.rows.iter().take(workers).enumerate() {
            let inline_more = (cramped && more > 0 && i == 0).then(|| mono(format!("+{more}"), m.meta, p.dim_t).flex_none());
            let armed = session.is_some_and(|s| self.model.cancels.armed(s, r.id, (r.call.as_deref(), &r.name), now));
            list = list.child(self.tray_row(r, watched == Some(r.id), armed, row_h, inline_more, p, m, cx));
        }
        if more > 0 && !cramped {
            list = list.child(h_flex().h(px(row_h)).pl(px(m.pad_x + m.lead)).items_center().child(mono(
                format!("+{more} more"),
                m.meta,
                p.dim_t,
            )));
        }
        let (gap, pad) = if cramped { (4., 2.) } else { (8., 4.) };
        // The workers' bar down its lead edge; the watched row's is yours.
        let you = watched.and_then(|w| self.tray.rows.iter().take(workers).position(|r| r.id == w));
        Some(column(
            lay,
            div()
                .mt(px(gap))
                .py(px(pad))
                .relative()
                .bg(p.tool)
                .font_family(fonts::MONO)
                .child(list)
                .child(glyph::bar(p.sub_rule))
                .when_some(you, |d, i| {
                    d.child(div().absolute().left_0().top(px(pad + i as f32 * row_h)).w(px(2.)).h(px(row_h)).bg(p.you_mark_t))
                }),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn tray_row(
        &self,
        r: &TrayRow,
        watched: bool,
        armed: bool,
        h: f32,
        more: Option<Div>,
        p: &Palette,
        m: &Metrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (g, _, word, family) = glyph::worker(r.mark.liveness());
        let (word_color, mark) = glyph::ink(family, Ground::Run, p);
        let line_color = match r.mark {
            Mark::Failed => p.err,
            Mark::Run | Mark::Cancelling => p.sub_status_t,
            _ => p.dim_t,
        };
        let who = Who::new(r.id, r.name.clone(), r.call.clone());
        // Facts · word · cancel ×: an armed cancel takes the facts' room.
        let trail = h_flex()
            .flex_none()
            .gap(px(8.))
            .items_center()
            .when(!r.facts.is_empty() && !armed, |d| d.child(div().text_color(p.sub_time_t).child(r.facts.clone())))
            .when(!self.layout.tight, |d| d.child(div().text_color(word_color).child(word)))
            .children(more)
            .children(r.watchable.then(|| self.cancel_x(&who, &r.handle, armed, p, cx)).flatten());
        let row = h_flex()
            .id(("tray-row", r.id as usize))
            .group("tray-row")
            .h(px(h))
            .w_full()
            .relative()
            .px(px(m.pad_x))
            .items_center()
            .whitespace_nowrap()
            .text_size(px(m.meta))
            // The watched worker (the composer steers it): its bar is the
            // tray's ([`AppView::render_tray`]).
            .when(watched, |d| d.bg(p.tool_out))
            .child(glyph::lead(Some(glyph::draw(g, mark)), m))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(m.run_gap))
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(m.meta * 0.6 * 18.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.sub_name_t)
                            .child(r.name.clone()),
                    )
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(line_color).child(r.line.clone()))
                    .child(trail),
            )
            .children(self.render_menu_at_row(r.id, p, cx));
        if !r.watchable {
            let tip = r.tip.clone();
            return row.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)).into_any_element();
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

    /// A worker's view's breadcrumb: MAIN (back to the session) › its name
    /// and handle · its state · its model, and how to get back. Formatted
    /// when the worker changes ([`crate::watch::Worker`]'s strings).
    pub(crate) fn render_crumb(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let w = self.model.watch.focused()?;
        let lay = self.layout;
        let form = lay.form();
        let (word, color) = match w.state() {
            "running" => ("running", p.run),
            "completed" => ("completed · read-only", p.ok),
            "connecting" => ("connecting", p.dim),
            "failed" => ("failed · read-only", p.err),
            "gone" => ("gone · read-only", p.dim),
            "offline" => ("offline · read-only", p.dim),
            "cancelled" => ("cancelled · read-only", p.warn),
            "timed out" => ("timed out · read-only", p.warn),
            _ => ("read-only", p.dim),
        };
        // The session waits on a prompt: its card is never shown (nor
        // answered) over a worker's view; this says so and takes you there.
        let waits = !self.model.transcript.pending_prompts().is_empty();
        let needs = waits.then(|| {
            h_flex()
                .id("crumb-needs")
                .h_full()
                .flex_none()
                .gap(px(8.))
                .px(px(10.))
                .items_center()
                .border_l_2()
                .border_color(p.warn_mark)
                .cursor_pointer()
                .hover(|d| d.bg(p.raised))
                .child(display(label::NEEDS_YOU, 10.5, p.warn).flex_none())
                .when(!lay.tight, |d| d.child(mono(w.needs.clone(), 12., p.warn).flex_none()))
                .tooltip(|window, cx| {
                    Tooltip::new("a prompt waits in this worker's session: answer it on the session's tab").build(window, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.open_parent_card(cx)))
        });
        let row = h_flex()
            .h(px(crate::fit::crumb_h(form) - 1.))
            .items_center()
            .gap(px(10.))
            .whitespace_nowrap()
            .child(
                tbtn("crumb-main", label::MAIN, p.dim, p.ink)
                    .on_click(cx.listener(|this, _, _, cx| this.leave_worker(false, cx)))
                    .tooltip(|window, cx| Tooltip::new("back to the session · Esc").build(window, cx)),
            )
            .child(mono("›", 12., p.dim).flex_none())
            .child(mono(w.crumb.clone(), 12., p.agent).flex_none().max_w(px(260.)).overflow_hidden().text_ellipsis())
            .child(mono("·", 12., p.dim).flex_none())
            .child(mono(word, 12., color).flex_none())
            .when(!w.facts.is_empty() && !lay.tight, |d| {
                d.child(mono("·", 12., p.dim).flex_none())
                    .child(mono(w.facts.clone(), 12., p.dim).min_w_0().overflow_hidden().text_ellipsis())
            })
            .child(div().flex_1())
            .children(needs)
            // A worker tab shows its one worker: no walk.
            .when(!lay.tight && !w.tabbed && !waits, |d| d.child(mono(WORKER_KEY, 11., p.dim).flex_none()));
        Some(div().flex_none().bg(p.canvas).border_b_1().border_color(p.hair).child(column(lay, row)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Mark, TrayRow};
    use crate::glyph::{self, Glyph};
    use crate::subagents::{Live, Status};

    fn live(status: Status, line: &str) -> Live {
        Live { id: 3, call: None, name: "quick".into(), status, line: line.into(), tools: Some(1), elapsed: Some(40.0), watchable: true }
    }

    /// A row's glyph, word and step.
    fn shown(r: &TrayRow) -> (Glyph, &'static str, &str) {
        let (g, _, word, _) = glyph::worker(r.mark.liveness());
        (g, word, r.line.as_ref())
    }

    /// A cancel on its way, then the rows' cancelling: `cancelling` until it
    /// has stopped; then `cancelled`. The step stays the worker's.
    #[test]
    fn a_cancel_says_cancelling_until_the_rows_say_cancelled() {
        let r = TrayRow::of(live(Status::Running, "bash"), true);
        assert_eq!((r.mark, shown(&r)), (Mark::Cancelling, (Glyph::Cancelling, "cancelling", "bash")));
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
        assert_eq!((r.mark, shown(&r)), (Mark::Started, (Glyph::Idle, "started", "")));
        let block = glyph::worker(crate::subagents::Liveness::of(Some(Status::Started), true));
        assert_eq!(glyph::worker(r.mark.liveness()), block, "the tray row and the block agree");
    }
}
