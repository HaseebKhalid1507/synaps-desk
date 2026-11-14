//! The Usage view: what the telemetry store on the daemon's machine holds,
//! by host (`usage-design/spec.md` sections 3–5). A sticky header (host
//! and range chips, the refresh age, one line per source), then the
//! sections on a 12-column grid. Everything shown is laid out once per
//! load ([`crate::usage::Board`]); the pointer only moves a readout.

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, Open, display, mono};
use crate::chart::{self, Ink, Scale, Spot};
use crate::tabs::Place;
use crate::theme::Palette;
use crate::usage::{self, Board, NONE, Range, SpendBy, Stack};

const WIDE: f32 = 1180.;
const GUTTER: f32 = 24.;
const SECTION: f32 = 40.;
/// Under this pane every block is full width.
const NARROW: f32 = 900.;
const Y_AXIS: f32 = 64.;

/// The charts the pointer can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chart {
    Spend,
    Prompt,
    Hit,
    Ttft,
    Total,
}

pub(crate) struct UsageUi {
    /// The chips: `None` is ALL.
    pub host: Option<String>,
    pub range: Range,
    pub by: SpendBy,
    pub board: Option<Arc<Board>>,
    pub loading: bool,
    pub error: Option<String>,
    /// Bumped per load; an older load's answer is dropped.
    pub generation: u64,
    /// The host chips last seen.
    pub chips: Vec<String>,
    pub fetched: Option<Instant>,
    pub hover: Option<(Chart, usize)>,
    pub scroll: ScrollHandle,
    spots: [Spot; 5],
}

impl UsageUi {
    pub fn new(host: Option<String>, range: Range) -> Self {
        UsageUi {
            host,
            range,
            by: SpendBy::default(),
            board: None,
            loading: false,
            error: None,
            generation: 0,
            chips: Vec::new(),
            fetched: None,
            hover: None,
            scroll: ScrollHandle::new(),
            spots: Default::default(),
        }
    }

    fn spot(&self, c: Chart) -> Spot {
        self.spots[c as usize].clone()
    }
}

fn sans(text: impl Into<SharedString>, size: f32, line: f32, color: Hsla) -> Div {
    div().text_size(px(size)).line_height(px(line)).text_color(color).child(text.into())
}

fn title(t: &'static str, p: &Palette) -> Div {
    display(t, 13., p.header)
}

fn note(t: impl Into<SharedString>, p: &Palette) -> Div {
    sans(t, 13., 20., p.dim).whitespace_normal().mt(px(10.))
}

fn swatch(c: Hsla) -> Div {
    div().size(px(8.)).flex_none().bg(c)
}

fn ink(p: &Palette) -> Ink {
    Ink { series: [p.s1, p.s2, p.s3, p.s4, p.s5, p.s_rest], grid: p.hair, rule: p.dim, hover: p.hair.opacity(0.6) }
}

/// Grid lines at the y ticks, as 0..1 of the plot.
fn tick_fracs(ticks: &[(f64, String)], scale: Scale) -> Vec<f32> {
    ticks.iter().map(|(v, _)| scale.at(*v)).collect()
}

impl AppView {
    fn usage_chip(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        if self.usage.host != host {
            self.usage.host = host;
            self.usage_load(cx);
        }
    }

    fn usage_range(&mut self, r: Range, cx: &mut Context<Self>) {
        if self.usage.range != r {
            self.usage.range = r;
            self.usage_load(cx);
        }
    }

    fn usage_hover(&mut self, c: Chart, ix: Option<usize>, cx: &mut Context<Self>) {
        let h = ix.map(|i| (c, i));
        if self.usage.hover != h && (h.is_some() || self.usage.hover.is_some_and(|x| x.0 == c)) {
            self.usage.hover = h;
            cx.notify();
        }
    }

    /// A plot with its y labels, x labels and the pointer's day.
    #[allow(clippy::too_many_arguments)]
    fn plot(
        &self,
        c: Chart,
        s: &Arc<Stack>,
        height: f32,
        scale: Scale,
        yfmt: fn(f64) -> String,
        body: AnyElement,
        p: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let ticks = match scale {
            Scale::Linear(lo, hi) if lo > 0. => {
                let step = (hi - lo) / 2.;
                vec![(lo, yfmt(lo)), (lo + step, yfmt(lo + step)), (hi, yfmt(hi))]
            }
            Scale::Linear(_, hi) => usage::y_ticks(hi, yfmt),
            Scale::Log(lo, hi) => usage::log_ticks(lo, hi, yfmt),
        };
        let mut ylab = div().relative().w(px(Y_AXIS)).h(px(height)).flex_none();
        for (v, l) in &ticks {
            let top = height * (1. - scale.at(*v)) - 10.;
            ylab = ylab.child(mono(l.clone(), 12., p.dim).absolute().right(px(8.)).top(px(top.max(-4.))));
        }
        let n = s.days.len().max(1);
        let mut xlab = div().relative().h(px(20.)).ml(px(Y_AXIS)).flex_1();
        for (i, l) in usage::x_ticks(&s.days) {
            xlab = xlab.child(mono(l, 12., p.dim).absolute().top(px(4.)).left(relative(i as f32 / n as f32)));
        }
        let (stack, spot) = (s.clone(), self.usage.spot(c));
        let area = div()
            .id(("plot", c as usize))
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(height))
            .child(body)
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                let ix = chart::day_at(&spot, &stack, ev.position.x);
                this.usage_hover(c, ix, cx);
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.usage_hover(c, None, cx);
                }
            }));
        v_flex().w_full().child(h_flex().w_full().child(ylab).child(area)).child(h_flex().w_full().child(xlab)).into_any_element()
    }

    /// The row above a plot: the hovered day and each series' value, or with
    /// no hover the range totals (`latest`: each series' last known day,
    /// for values that do not add up). It is also the legend.
    fn readout(&self, c: Chart, s: &Stack, fmt: fn(f64) -> String, total_label: &str, latest: bool, p: &Palette) -> AnyElement {
        let day = self.usage.hover.filter(|h| h.0 == c).map(|h| h.1).filter(|&i| i < s.days.len());
        let mut row = h_flex().flex_wrap().gap_x(px(18.)).gap_y(px(4.)).items_center().min_h(px(20.));
        row = row.child(mono(day.map(|i| usage::day_label(&s.days[i])).unwrap_or_else(|| total_label.to_string()), 12.5, p.ink));
        for l in &s.series {
            let v = match day {
                Some(i) => l.values[i].map(fmt).unwrap_or_else(|| NONE.into()),
                None if latest => l.values.iter().rev().flatten().next().copied().map(fmt).unwrap_or_else(|| NONE.into()),
                None => fmt(l.total),
            };
            row = row.child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(swatch(p.series(l.color)))
                    .child(mono(l.key.clone(), 12.5, p.dim))
                    .child(mono(v, 12.5, p.prose)),
            );
        }
        row.into_any_element()
    }

    /// The Usage view, `pane` px wide.
    pub(crate) fn render_usage(&self, pane: f32, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let w = (pane - 48.).clamp(320., WIDE);
        let left = ((pane - w) / 2.).max(24.);
        let narrow = pane < NARROW;
        let col = (w - 11. * GUTTER) / 12.;
        let span = |n: f32| if narrow { w } else { n * col + (n - 1.) * GUTTER };
        let header = self.usage_header(left, w, p, cx);
        let body: AnyElement = match (&self.usage.board, &self.usage.error) {
            (Some(b), _) => self.usage_body(b, w, &span, narrow, p, cx),
            (None, Some(e)) => sans(e.clone(), 14., 22., p.err).pt(px(32.)).whitespace_normal().into_any_element(),
            (None, None) => {
                sans(if self.usage.loading { "Reading the telemetry store…" } else { NONE }, 14., 22., p.dim).pt(px(32.)).into_any_element()
            }
        };
        let scroll = div()
            .id("usage-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.usage.scroll)
            .child(h_flex().w_full().pl(px(left)).child(div().w(px(w)).flex_none().pb(px(64.)).child(body)));
        v_flex().size_full().bg(p.canvas).child(header).child(scroll).into_any_element()
    }

    fn usage_header(&self, left: f32, w: f32, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let chip = |id: (&'static str, usize), label: SharedString, on: bool| {
            div()
                .id(id)
                .pb(px(3.))
                .border_b_1()
                .border_color(if on { p.focus } else { gpui_kit::transparent_black() })
                .cursor_pointer()
                .child(display(label, 12., if on { p.ink } else { p.dim }))
                .hover(|d| d.text_color(p.ink))
        };
        let mut hosts = h_flex().gap(px(16.)).items_baseline();
        hosts = hosts.child(
            chip(("usage-host", 0), "A L L".into(), self.usage.host.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.usage_chip(None, cx))),
        );
        for (i, h) in self.usage.chips.iter().enumerate() {
            let on = self.usage.host.as_deref() == Some(h.as_str());
            let host = h.clone();
            hosts = hosts.child(
                chip(("usage-host", i + 1), usage::tracked(h).into(), on)
                    .on_click(cx.listener(move |this, _, _, cx| this.usage_chip(Some(host.clone()), cx))),
            );
        }
        let mut ranges = h_flex().gap(px(14.)).items_baseline();
        for (i, r) in Range::ALL.into_iter().enumerate() {
            ranges = ranges.child(
                chip(("usage-range", i), r.label().into(), self.usage.range == r)
                    .on_click(cx.listener(move |this, _, _, cx| this.usage_range(r, cx))),
            );
        }
        let age = self.usage.board.as_ref().and_then(|b| b.generated_at).map(|g| usage::age(g, crate::views::now_ms()));
        let refresh = h_flex()
            .id("usage-refresh")
            .gap(px(8.))
            .items_baseline()
            .cursor_pointer()
            .child(display(if self.usage.loading { "R E A D I N G" } else { "R E F R E S H" }, 12., p.prose))
            .when_some(age, |d, a| d.child(mono(a, 12., p.dim)))
            .hover(|d| d.text_color(p.ink))
            .on_click(cx.listener(|this, _, _, cx| this.usage_load(cx)));
        let sources = self.usage.board.as_ref().map(|b| b.sources.clone()).unwrap_or_default();
        let mut line = h_flex().gap(px(8.)).items_center().flex_wrap().min_h(px(20.));
        for (i, s) in sources.iter().enumerate() {
            if i > 0 {
                line = line.child(mono("·", 12., p.dim));
            }
            line = line.child(
                h_flex().gap(px(6.)).items_center().child(swatch(if s.empty { p.hair } else { p.series(s.color) })).child(mono(
                    s.text.clone(),
                    12.,
                    p.dim,
                )),
            );
        }
        if let Some(e) = self.usage.error.as_ref().filter(|_| self.usage.board.is_some()) {
            line = line.child(mono(format!("  ·  {e}"), 12., p.err));
        }
        v_flex().flex_none().border_b_1().border_color(p.hair).child(
            h_flex().w_full().pl(px(left)).child(
                v_flex()
                    .w(px(w))
                    .flex_none()
                    .child(
                        h_flex()
                            .h(px(52.))
                            .gap(px(28.))
                            .items_center()
                            .child(display("U S A G E", 16., p.header))
                            .child(hosts)
                            .child(ranges)
                            .child(div().flex_1())
                            .child(display("A P I - E Q U I V A L E N T   ·   L I S T   P R I C E", 12., p.dim))
                            .child(refresh),
                    )
                    .child(line.pb(px(10.))),
            ),
        )
    }

    fn usage_body(&self, b: &Board, w: f32, span: &dyn Fn(f32) -> f32, narrow: bool, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let ink = ink(p);
        let money = |v: f64| usage::money(v);
        let mut out = v_flex().w(px(w));
        // 4.1 Headline.
        let mut cells = h_flex().flex_wrap().items_start().gap(px(GUTTER)).pt(px(28.));
        for c in &b.cells {
            cells = cells.child(
                v_flex()
                    .w(px(span(2.).floor() - 1.))
                    .gap(px(4.))
                    .child(display(c.label, 12., p.dim))
                    .child(display(c.number.clone(), 28., if c.money { p.cost } else { p.ink }).line_height(px(34.)))
                    .child(sans(c.second.clone(), 13., 20., p.dim).whitespace_normal()),
            );
        }
        out = out.child(cells).child(note(b.notes.headline.clone(), p));
        // 4.2 Spend over time.
        let s = &b.spend[match self.usage.by {
            SpendBy::Host => 0,
            SpendBy::Model => 1,
            SpendBy::Role => 2,
        }];
        let mut switch = h_flex().gap(px(14.)).items_baseline();
        for (i, by) in SpendBy::ALL.into_iter().enumerate() {
            let on = self.usage.by == by;
            switch = switch.child(
                div()
                    .id(("spend-by", i))
                    .pb(px(3.))
                    .border_b_1()
                    .border_color(if on { p.focus } else { gpui_kit::transparent_black() })
                    .cursor_pointer()
                    .child(display(usage::tracked(by.label()), 12., if on { p.ink } else { p.dim }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.by = by;
                        this.usage.hover = None;
                        cx.notify();
                    })),
            );
        }
        let scale = Scale::Linear(0., s.max);
        let yt = usage::y_ticks(s.max, money);
        let hover = self.usage.hover.filter(|h| h.0 == Chart::Spend).map(|h| h.1);
        let bars = chart::bars(s.clone(), ink, tick_fracs(&yt, scale), hover, b.request_ix, self.usage.spot(Chart::Spend));
        let range_label = format!("{} total", b.range.label());
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(h_flex().gap(px(24.)).items_baseline().child(title("S P E N D   O V E R   T I M E", p)).child(switch))
                .child(self.readout(Chart::Spend, s, money, &range_label, false, p))
                .child(self.plot(Chart::Spend, s, 220., scale, money, bars.into_any_element(), p, cx))
                .when(b.request_ix.is_some(), |d| d.child(mono("┊ request-level from here (dotted rule)", 12., p.dim)))
                .child(note(b.notes.spend.clone(), p)),
        );
        // 4.3 Prompt tokens per day (12 columns until the store has output per day).
        let pt = &b.prompt;
        let scale = Scale::Linear(0., pt.max);
        let tok = |v: f64| usage::tokens(v as u64);
        let yt = usage::y_ticks(pt.max, tok);
        let hover = self.usage.hover.filter(|h| h.0 == Chart::Prompt).map(|h| h.1);
        let bars = chart::bars(pt.clone(), ink, tick_fracs(&yt, scale), hover, None, self.usage.spot(Chart::Prompt));
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(title("P R O M P T   T O K E N S   /   D A Y", p))
                .child(self.readout(Chart::Prompt, pt, tok, &range_label, false, p))
                .child(self.plot(Chart::Prompt, pt, 140., scale, tok, bars.into_any_element(), p, cx))
                .child(note(b.notes.tokens.clone(), p)),
        );
        // 4.4 Models and roles.
        let top = b.models.first().map(|m| m.1).unwrap_or(0.);
        let mut models = v_flex().w(px(span(8.))).gap(px(0.));
        models = models
            .child(h_flex().h(px(24.)).gap(px(12.)).child(mono("MODEL", 12., p.dim).flex_1()).child(mono("SPEND", 12., p.dim).w(px(200.))));
        for (i, (name, spend, share)) in b.models.iter().take(12).enumerate() {
            let frac = if top > 0. { (spend / top) as f32 } else { 0. };
            models = models.child(
                h_flex()
                    .h(px(28.))
                    .gap(px(12.))
                    .items_center()
                    .border_t_1()
                    .border_color(p.hair)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(8.))
                            .items_center()
                            .child(swatch(p.series(i.min(5))))
                            .child(mono(name.clone(), 12.5, p.prose).overflow_hidden().text_ellipsis()),
                    )
                    .child(
                        h_flex()
                            .w(px(200.))
                            .gap(px(8.))
                            .items_center()
                            .child(div().w(px(90.)).h(px(6.)).bg(p.hair).child(div().h_full().w(relative(frac)).bg(p.series(i.min(5)))))
                            .child(mono(usage::money(*spend), 12.5, p.cost))
                            .child(mono(format!("{:.0} %", share * 100.), 12., p.dim)),
                    ),
            );
        }
        if b.models.len() > 12 {
            models = models.child(mono(format!("+ {} more", b.models.len() - 12), 12., p.dim).pt(px(6.)));
        }
        let role_total: f64 = b.roles.iter().map(|r| r.1).sum();
        let mut roles = v_flex().w(px(span(4.))).child(h_flex().h(px(24.)).child(mono("MAIN / WORKERS", 12., p.dim)));
        for (i, (name, v)) in b.roles.iter().enumerate() {
            roles = roles.child(
                h_flex()
                    .h(px(28.))
                    .gap(px(10.))
                    .items_center()
                    .border_t_1()
                    .border_color(p.hair)
                    .child(swatch(p.series(i)))
                    .child(mono(name.clone(), 12.5, p.prose).flex_1())
                    .child(mono(usage::money(*v), 12.5, p.cost))
                    .child(mono(if role_total > 0. { format!("{:.0} %", 100. * v / role_total) } else { NONE.into() }, 12., p.dim)),
            );
        }
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(title("M O D E L S   A N D   R O L E S", p))
                .child(h_flex().flex_wrap().gap(px(GUTTER)).items_start().child(models).child(roles))
                .child(note(b.notes.models.clone(), p)),
        );
        // 4.5 Cache.
        let hit = &b.hit;
        let hscale = Scale::Linear(50., 100.);
        let pctf = |v: f64| format!("{v:.0} %");
        let hover = self.usage.hover.filter(|h| h.0 == Chart::Hit).map(|h| h.1);
        let lines = chart::lines(
            hit.clone(),
            hscale,
            ink,
            vec![0., 0.5, 1.],
            vec![false],
            b.legacy_until,
            b.releases.iter().map(|r| r.0).collect(),
            hover,
            self.usage.spot(Chart::Hit),
        );
        let hit_total = b.cells.get(4).map(|c| c.number.clone()).unwrap_or_default();
        let kv = |k: &'static str, v: String| {
            h_flex().h(px(22.)).gap(px(12.)).child(mono(k, 12., p.dim).w(px(110.))).child(mono(v, 12.5, p.prose))
        };
        let side = v_flex()
            .w(px(span(4.)))
            .gap(px(2.))
            .child(display("S A V E D", 12., p.dim))
            .child(kv("gross", usage::money_or(b.saved.0)))
            .child(kv("net", usage::money_or(b.saved.1)))
            .child(display("W R I T E S", 12., p.dim).mt(px(10.)))
            .child(kv("5 min", usage::tokens(b.writes.0)))
            .child(kv("1 hour", usage::tokens(b.writes.1)))
            .child(kv("unknown", usage::tokens(b.writes.2)))
            .child(display("K E E P - W A R M", 12., p.dim).mt(px(10.)))
            .child(kv("pings", usage::count_or(b.keepwarm.pings)))
            .child(kv("cost", usage::money_or(b.keepwarm.cost)))
            .child(kv("saved (gross)", usage::money_or(b.keepwarm.est_saved)));
        let miss_max = b.misses.iter().map(|m| m.1).max().unwrap_or(0).max(1);
        let mut misses = v_flex().w_full().gap(px(4.)).child(display("M I S S   R E A S O N S", 12., p.dim));
        for (reason, n) in &b.misses {
            misses = misses.child(
                h_flex()
                    .h(px(20.))
                    .gap(px(12.))
                    .items_center()
                    .child(mono(reason.clone(), 12.5, p.prose).w(px(240.)).overflow_hidden().text_ellipsis())
                    .child(
                        div().w(px(span(6.).min(360.))).h(px(8.)).child(div().h_full().w(relative(*n as f32 / miss_max as f32)).bg(p.s1)),
                    )
                    .child(mono(usage::thousands(*n), 12.5, p.dim)),
            );
        }
        if b.misses.is_empty() {
            misses = misses.child(mono(NONE, 12.5, p.dim));
        }
        let hit_plot = v_flex()
            .w(px(span(8.)))
            .gap(px(10.))
            .child(self.readout(Chart::Hit, hit, |v| format!("{v:.1} %"), &format!("{} · {hit_total}", b.range.label()), false, p))
            .child(self.plot(Chart::Hit, hit, 140., hscale, pctf, lines.into_any_element(), p, cx))
            .when(!b.releases.is_empty(), |d| {
                d.child(
                    mono(format!("ticks: releases {}", b.releases.iter().map(|r| r.1.as_str()).collect::<Vec<_>>().join(", ")), 12., p.dim)
                        .whitespace_normal(),
                )
            })
            .when(b.legacy_until.is_some(), |d| d.child(mono("dim: legacy log lines (no request rows)", 12., p.dim)));
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(title("C A C H E", p))
                .child(h_flex().flex_wrap().gap(px(GUTTER)).items_start().child(hit_plot).child(side))
                .child(misses.mt(px(16.)))
                .child(note(b.notes.cache.clone(), p)),
        );
        // 4.6 Latency.
        let lat = |c: Chart, s: &Arc<Stack>, label: &'static str| {
            let sc = Scale::log_of(s);
            let hover = self.usage.hover.filter(|h| h.0 == c).map(|h| h.1);
            let (lo, hi) = match sc {
                Scale::Log(lo, hi) => (lo, hi),
                Scale::Linear(lo, hi) => (lo, hi),
            };
            let yt = usage::log_ticks(lo, hi, usage::secs);
            let body =
                chart::lines(s.clone(), sc, ink, tick_fracs(&yt, sc), vec![false, true], None, Vec::new(), hover, self.usage.spot(c));
            v_flex()
                .w(px(span(6.)))
                .gap(px(10.))
                .child(display(label, 12., p.dim))
                .child(self.readout(c, s, usage::secs, "latest day · p95 thin", true, p))
                .child(self.plot(c, s, 140., sc, usage::secs, body.into_any_element(), p, cx))
        };
        let (xr, yr) = b.scatter_range;
        let yt = usage::log_ticks(yr.0, yr.1, usage::secs);
        let scatter = chart::scatter(b.scatter.clone(), p.s2, p.hair, tick_fracs(&yt, Scale::Log(yr.0, yr.1)));
        let mut xs = div().relative().h(px(20.)).ml(px(Y_AXIS)).flex_1();
        for (v, l) in usage::log_ticks(xr.0, xr.1, |v| usage::tokens(v as u64)) {
            xs = xs.child(mono(l, 12., p.dim).absolute().top(px(4.)).left(relative(Scale::Log(xr.0, xr.1).at(v))));
        }
        let mut ys = div().relative().w(px(Y_AXIS)).h(px(180.)).flex_none();
        for (v, l) in &yt {
            ys = ys.child(mono(l.clone(), 12., p.dim).absolute().right(px(8.)).top(px(180. * (1. - Scale::Log(yr.0, yr.1).at(*v)) - 10.)));
        }
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(title("L A T E N C Y", p))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(GUTTER))
                        .items_start()
                        .child(lat(Chart::Ttft, &b.ttft, "T I M E   T O   F I R S T   T O K E N"))
                        .child(lat(Chart::Total, &b.total, "T O T A L")),
                )
                .child(mono(b.split.clone(), 12.5, p.prose).mt(px(6.)).whitespace_normal())
                .child(display("T O T A L   T I M E   B Y   C O N T E X T   S I Z E", 12., p.dim).mt(px(16.)))
                .child(
                    v_flex()
                        .w_full()
                        .child(h_flex().w_full().child(ys).child(div().flex_1().h(px(180.)).child(scatter)))
                        .child(h_flex().w_full().child(xs)),
                )
                .child(mono("prompt tokens →, total time ↑, both log; each square a request", 12., p.dim))
                .child(note(b.notes.latency.clone(), p)),
        );
        // 4.7 Activity: the hour grid.
        let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let mut grid = v_flex().gap(px(2.));
        let mut hours = h_flex().gap(px(2.)).pl(px(40.));
        for h in 0..24 {
            hours = hours.child(div().w(px(16.)).child(if h % 6 == 0 { mono(format!("{h:02}"), 12., p.dim) } else { div() }));
        }
        grid = grid.child(hours);
        for (d, row) in b.heat.iter().enumerate() {
            let mut r = h_flex().gap(px(2.)).items_center().child(mono(days[d], 12., p.dim).w(px(38.)));
            for &n in row {
                let step = if b.heat_max == 0 || n == 0 { 0 } else { ((n as f64 / b.heat_max as f64) * 5.).ceil() as usize };
                r = r.child(div().w(px(16.)).h(px(14.)).bg(if step == 0 { p.hair.opacity(0.5) } else { p.s1.opacity(step as f32 / 5.) }));
            }
            grid = grid.child(r);
        }
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(h_flex().gap(px(16.)).items_baseline().child(title("W H E N", p)).child(mono(
                    format!("requests per hour, New York time · busiest {}", usage::thousands(b.heat_max)),
                    12.,
                    p.dim,
                )))
                .child(grid)
                .child(note(b.notes.activity.clone(), p)),
        );
        // 4.8 Agents.
        let mut agents = v_flex().w_full();
        agents = agents.child(
            h_flex()
                .h(px(24.))
                .gap(px(12.))
                .child(mono("AGENT", 12., p.dim).w(px(140.)))
                .child(mono("RUNS", 12., p.dim).w(px(70.)))
                .child(mono("REQUESTS", 12., p.dim).w(px(90.)))
                .child(mono("SPEND", 12., p.dim).w(px(220.)))
                .child(mono("MODELS", 12., p.dim).flex_1()),
        );
        let amax = b.agents.iter().filter_map(|a| a.spend).fold(0., f64::max);
        for a in b.agents.iter().take(20) {
            let frac = match a.spend {
                Some(s) if amax > 0. => (s / amax) as f32,
                _ => 0.,
            };
            agents = agents.child(
                h_flex()
                    .h(px(28.))
                    .gap(px(12.))
                    .items_center()
                    .border_t_1()
                    .border_color(p.hair)
                    .child(mono(a.name.clone(), 12.5, p.prose).w(px(140.)).overflow_hidden().text_ellipsis())
                    .child(mono(usage::count_or(a.runs), 12.5, p.prose).w(px(70.)))
                    .child(mono(usage::count_or(a.requests), 12.5, p.prose).w(px(90.)))
                    .child(
                        h_flex()
                            .w(px(220.))
                            .gap(px(8.))
                            .items_center()
                            .child(div().w(px(90.)).h(px(6.)).bg(p.hair).child(div().h_full().w(relative(frac)).bg(p.s2)))
                            .child(mono(usage::money_or(a.spend), 12.5, p.cost)),
                    )
                    .child(mono(a.models.join(", "), 12., p.dim).flex_1().min_w_0().overflow_hidden().text_ellipsis()),
            );
        }
        if b.agents.len() > 20 {
            agents = agents.child(mono(format!("+ {} more agents", b.agents.len() - 20), 12., p.dim).pt(px(6.)));
        }
        if b.agents.is_empty() {
            agents = agents.child(mono(NONE, 12.5, p.dim).pt(px(6.)));
        }
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(h_flex().gap(px(16.)).items_baseline().child(title("A G E N T S", p)).child(mono(
                    format!("worker spend {}", usage::money(b.agents_total)),
                    12.,
                    p.dim,
                )))
                .child(agents)
                .child(note(b.notes.agents.clone(), p)),
        );
        // 4.9 Sessions: the top 25 by cost; a live one on this daemon opens in a tab.
        let mut sessions = v_flex().w_full();
        sessions = sessions.child(
            h_flex()
                .h(px(24.))
                .gap(px(12.))
                .child(mono("TITLE", 12., p.dim).flex_1())
                .when(!narrow, |d| d.child(mono("HOST", 12., p.dim).w(px(70.))).child(mono("STARTED", 12., p.dim).w(px(70.))))
                .child(mono("MAIN CONVERSATION", 12., p.dim).w(px(150.)))
                .child(mono("WORKERS", 12., p.dim).w(px(70.)))
                .child(mono("REQUESTS", 12., p.dim).w(px(80.))),
        );
        for (i, s) in b.sessions.iter().enumerate() {
            let live = self.model.sessions.iter().find(|m| m.is(&s.id)).map(|m| m.id.clone());
            let title: SharedString = s.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| s.id.clone()).into();
            sessions = sessions.child(
                h_flex()
                    .id(("usage-session", i))
                    .h(px(28.))
                    .gap(px(12.))
                    .items_center()
                    .border_t_1()
                    .border_color(p.hair)
                    .child(
                        sans(title, 13., 20., if live.is_some() { p.ink } else { p.prose })
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis(),
                    )
                    .when(!narrow, |d| {
                        d.child(mono(s.host.clone().unwrap_or_else(|| NONE.into()), 12.5, p.dim).w(px(70.))).child(
                            mono(s.created.map(|t| usage::day_label(&usage::ny_day(t))).unwrap_or_else(|| NONE.into()), 12.5, p.dim)
                                .w(px(70.)),
                        )
                    })
                    .child(mono(usage::money_or(s.spend), 12.5, p.cost).w(px(150.)))
                    .child(mono(usage::count_or(s.workers.filter(|w| *w > 0)), 12.5, p.prose).w(px(70.)))
                    .child(mono(usage::count_or(s.requests), 12.5, p.prose).w(px(80.)))
                    .when_some(live, |d, id| {
                        d.cursor_pointer().hover(|d| d.bg(p.user)).on_click(cx.listener(move |this, _, _, cx| {
                            let fallback = this.model.tab_fallback(&id);
                            this.open_tab(Open::Attach { session_id: id.clone(), fallback, open: false }, Place::New);
                            cx.notify();
                        }))
                    }),
            );
        }
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(10.))
                .child(h_flex().gap(px(16.)).items_baseline().child(title("S E S S I O N S", p)).child(mono(
                    format!("top {} of {} by cost · a live one opens in a tab", b.sessions.len(), usage::count_or(b.sessions_total)),
                    12.,
                    p.dim,
                )))
                .child(sessions)
                .child(note(b.notes.sessions.clone(), p)),
        );
        // 4.11 Data.
        let mut sources = v_flex().w_full().child(
            h_flex()
                .h(px(24.))
                .gap(px(12.))
                .child(mono("HOST", 12., p.dim).w(px(80.)))
                .child(mono("KIND", 12., p.dim).w(px(120.)))
                .child(mono("SIZE", 12., p.dim).w(px(90.)))
                .child(mono("READ TO", 12., p.dim).w(px(90.)))
                .child(mono("PATH", 12., p.dim).flex_1()),
        );
        for s in &b.ingest {
            let size = s.size.map(usage::bytes).unwrap_or_else(|| NONE.into());
            let read = match (s.offset, s.size) {
                (Some(o), Some(z)) if z > 0 => format!("{:.0} %", 100. * o as f64 / z as f64),
                _ => NONE.into(),
            };
            sources = sources.child(
                h_flex()
                    .h(px(26.))
                    .gap(px(12.))
                    .items_center()
                    .border_t_1()
                    .border_color(p.hair)
                    .child(mono(s.host.clone(), 12.5, p.prose).w(px(80.)))
                    .child(mono(s.kind.clone(), 12.5, p.prose).w(px(120.)))
                    .child(mono(size, 12.5, p.prose).w(px(90.)))
                    .child(mono(read, 12.5, p.prose).w(px(90.)))
                    .child(mono(crate::named::home_short(&s.path), 12., p.dim).flex_1().min_w_0().overflow_hidden().text_ellipsis()),
            );
        }
        out = out.child(
            v_flex()
                .pt(px(SECTION))
                .gap(px(8.))
                .child(title("D A T A", p))
                .child(mono(format!("stops: {}", b.stops), 12.5, p.prose).whitespace_normal())
                .child(mono(b.health_line.clone(), 12.5, p.prose))
                .child(mono(b.ingest_line.clone(), 12.5, p.prose).whitespace_normal())
                .child(sources.mt(px(8.)))
                .child(note("Folders (spend by the folder a session began in) need the store's folders endpoint: not drawn.", p)),
        );
        out.into_any_element()
    }
}
