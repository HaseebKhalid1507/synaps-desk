//! The tray of workers over the composer (the TUI's subagent tray), and a
//! worker's view's breadcrumb. One row per worker: its state, name, what
//! it is doing (or ended with), tools and elapsed time; at most what
//! [`crate::fit::tray_rows`] leaves room for, then "+N more". A finished
//! worker shows how it ended for [`crate::watch::FLASH`], then drops.
//! Nothing moves: no spinner; the elapsed time moves when a row comes (1 Hz
//! while a worker runs). A click on a row opens that worker's view.
//!
//! Built from the attached session's workers ([`crate::subagents`]) only
//! when they change: no row is formatted per frame.

use std::time::Instant;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, WORKER_KEY, column, display, mono, tbtn};
use crate::fonts::{self, label};
use crate::subagents::{Live, Status, fmt_secs};
use crate::theme::{Metrics, Palette};
use crate::watch::Candidate;

/// A row's state mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
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
            Status::Running | Status::Started => Mark::Run,
        }
    }
}

/// One tray row, formatted once per change.
#[derive(Clone, Debug)]
pub(crate) struct TrayRow {
    pub(crate) id: u64,
    pub(crate) name: SharedString,
    pub(crate) mark: Mark,
    pub(crate) line: SharedString,
    pub(crate) facts: SharedString,
    pub(crate) watchable: bool,
    pub(crate) tip: SharedString,
}

impl TrayRow {
    fn of(l: Live) -> Self {
        let mut facts = Vec::new();
        if let Some(n) = l.tools.filter(|&n| n > 0) {
            facts.push(format!("{n} tool{}", if n == 1 { "" } else { "s" }));
        }
        if let Some(s) = l.elapsed {
            facts.push(fmt_secs(s));
        }
        let tip = if l.watchable {
            format!("watch {} (sa_{}) · {WORKER_KEY} walks the workers", l.name, l.id)
        } else {
            format!("{}: the blocking subagent has no thread to watch", l.name)
        };
        TrayRow {
            id: l.id,
            name: l.name.into(),
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
    key: Option<(u64, Option<u64>, bool)>,
    pub(crate) rows: Vec<TrayRow>,
    /// When the next finished worker drops.
    until: Option<Instant>,
}

impl AppView {
    /// The tray's rows, rebuilt if a worker changed; the timer that drops
    /// a finished worker's row at the end of its flash.
    pub(crate) fn sync_tray(&mut self, cx: &mut Context<Self>) {
        let attached = self.model.active.is_some();
        let key = (self.model.stats.subagents.revision(), self.model.watch.focused().map(|w| w.subagent_id), attached);
        let now = Instant::now();
        if self.tray.key == Some(key) && self.tray.until.is_none_or(|u| now < u) {
            return;
        }
        let live = if attached { self.model.stats.subagents.live() } else { Vec::new() };
        // A view nobody looks at whose worker ended goes (and leaves the walk).
        self.model.watch.prune(now, &live);
        let (rows, until) = self.model.watch.tray(live, now);
        self.tray = TrayCache { key: Some(key), rows: rows.into_iter().map(TrayRow::of).collect(), until };
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

    /// Open worker `id`'s view (a tray row's click).
    pub(crate) fn watch_worker(&mut self, id: u64, name: &str, cx: &mut Context<Self>) {
        let cwd = self.model.cwd().map(str::to_string);
        self.model.watch.open(&Candidate::of(id, name), cwd.as_deref(), Instant::now());
        self.watch_opened();
        cx.notify();
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
        let (workers, more) = match (all > shown, cramped) {
            (false, _) => (shown, 0),
            (true, true) => (1, all - 1),
            (true, false) => (shown - 1, all - (shown - 1)),
        };
        let watched = self.model.watch.focused().map(|w| w.subagent_id);
        let row_h = crate::fit::tray_row(m);
        let mut list = v_flex();
        for (i, r) in self.tray.rows.iter().take(workers).enumerate() {
            let inline_more = (cramped && more > 0 && i == 0).then(|| mono(format!("+{more}"), m.meta, p.dim_t).flex_none());
            list = list.child(self.tray_row(r, watched == Some(r.id), row_h, inline_more, p, m, cx));
        }
        if more > 0 && !cramped {
            list = list.child(h_flex().h(px(row_h)).pl(px(m.run_x + 18.)).items_center().child(mono(
                format!("+{more} more"),
                m.meta,
                p.dim_t,
            )));
        }
        let (gap, pad) = if cramped { (4., 2.) } else { (8., 4.) };
        Some(column(
            lay,
            div().mt(px(gap)).py(px(pad)).bg(p.tool).border_t_1().border_color(p.sub_rule).font_family(fonts::MONO).child(list),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn tray_row(&self, r: &TrayRow, watched: bool, h: f32, more: Option<Div>, p: &Palette, m: &Metrics, cx: &Context<Self>) -> AnyElement {
        let (sign, color) = match r.mark {
            Mark::Run => (None, p.run),
            Mark::Cancelling => (Some("◌"), p.warn),
            Mark::Ok => (Some("✓"), p.ok),
            Mark::Failed => (Some("×"), p.err),
            Mark::TimedOut => (Some("!"), p.warn),
            Mark::Cancelled => (Some("–"), p.dim_t),
        };
        let ended = !matches!(r.mark, Mark::Run | Mark::Cancelling);
        let glyph = match sign {
            None => div().size(px(8.)).m(px(1.)).flex_none().bg(p.run_mark),
            Some(s) => div().w(px(10.)).flex_none().text_color(color).child(s),
        };
        let line_color = match r.mark {
            Mark::Failed => p.err,
            _ if ended => p.dim_t,
            _ => p.sub_status_t,
        };
        let row = h_flex()
            .id(("tray-row", r.id as usize))
            .h(px(h))
            .w_full()
            .relative()
            .px(px(m.run_x))
            .gap(px(m.run_gap))
            .items_center()
            .whitespace_nowrap()
            .text_size(px(m.meta))
            // The watched worker: the composer steers it.
            .when(watched, |d| d.bg(p.tool_out).child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(p.focus)))
            .child(glyph)
            .child(
                div()
                    .flex_none()
                    .max_w(px(m.meta * 0.6 * 18.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(if ended { p.dim_t } else { p.sub_name_t })
                    .child(r.name.clone()),
            )
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(line_color).child(r.line.clone()))
            .when(!r.facts.is_empty(), |d| d.child(div().flex_none().text_color(p.sub_time_t).child(r.facts.clone())))
            .children(more);
        let tip = r.tip.clone();
        let row = row.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
        if !r.watchable || watched {
            return row.into_any_element();
        }
        let (id, name) = (r.id, r.name.clone());
        row.cursor_pointer()
            .hover(|d| d.bg(p.tool_out))
            .on_click(cx.listener(move |this, _, _, cx| this.watch_worker(id, &name, cx)))
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
        // The session waits on a prompt (its card is hidden here).
        let waits = !self.model.transcript.pending_prompts().is_empty();
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
            .when(waits, |d| d.child(display(label::NEEDS_YOU, 10.5, p.warn).flex_none()))
            .when(!lay.tight, |d| d.child(mono(WORKER_KEY, 11., p.dim).flex_none()));
        Some(div().flex_none().bg(p.canvas).border_b_1().border_color(p.hair).child(column(lay, row)))
    }
}
