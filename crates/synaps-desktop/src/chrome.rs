//! The window's frame around the session: the mark strip, the top bar (what
//! needs you, connection, the accent picker), the sessions rail (filters,
//! the named sessions, the live ones) and the right-hand numbers.

use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::protocol::SessionMeta;

use crate::app::{AppView, display, home_short, mono, section, tbtn};
use crate::fonts::{self, label};
use crate::model::{ConnState, Filter, SAVED_LIMIT};
use crate::named::{Marker, RowAction, relative, short_model};
use crate::stats::compact;
use crate::theme::{self, Palette};

/// Context-gauge segments.
const SEGMENTS: usize = 48;
/// Files listed before "+N more".
const FILES_SHOWN: usize = 40;

fn status(m: Marker, p: &Palette) -> (&'static str, Hsla, bool) {
    match m {
        Marker::Awaiting => (label::NEEDS_YOU, p.hi, true),
        Marker::Busy => (label::RUNNING, p.ink, true),
        Marker::Resuming => (label::RESUMING, p.ink2, true),
        Marker::Pinned => (label::PINNED, p.ink3, false),
        Marker::Idle => (label::IDLE, p.ink4, false),
        Marker::Parked => (label::PARKED, p.ink4, false),
        Marker::Saved => (label::SAVED, p.ink4, false),
    }
}

/// `■ RUNNING` (filled) / `□ IDLE` (hollow).
fn status_chip(m: Marker, p: &Palette) -> impl IntoElement {
    let (text, color, filled) = status(m, p);
    h_flex()
        .gap(px(7.))
        .items_center()
        .child(div().size(px(5.)).flex_none().when(filled, |d| d.bg(color)).when(!filled, |d| d.border_1().border_color(color)))
        .child(display(text, 9.5, color))
}

fn kv(key: &'static str, value: impl Into<SharedString>, big: bool, p: &Palette) -> impl IntoElement {
    let value = value.into();
    v_flex().gap(px(5.)).child(display(key, 9.5, p.ink4)).child(if big {
        display(value, 17., p.ink).into_any_element()
    } else {
        mono(value, 12.5, p.ink).into_any_element()
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// One session in the rail.
struct Card {
    key: (&'static str, usize),
    title: SharedString,
    sub: SharedString,
    age: SharedString,
    marker: Marker,
    cost: Option<f64>,
    active: bool,
}

impl AppView {
    pub(crate) fn render_mark(&self, p: &Palette) -> impl IntoElement {
        let disc = match &self.model.conn {
            ConnState::Connected(_) => p.accent,
            _ if self.synthetic => p.accent,
            ConnState::Connecting | ConnState::Reconnecting => p.ink4,
            _ => p.line3,
        };
        v_flex()
            .w(px(30.))
            .h_full()
            .flex_none()
            .items_center()
            .pt(px(20.))
            .pb(px(24.))
            .border_r_1()
            .border_color(p.line)
            .child(div().size(px(12.)).rounded_full().bg(disc))
            .child(div().flex_1())
            .child(
                v_flex()
                    .items_center()
                    .gap(px(8.))
                    .font_family(fonts::JP)
                    .text_size(px(15.))
                    .text_color(p.ink3)
                    .child(fonts::JP_MARK_1)
                    .child(fonts::JP_MARK_2),
            )
    }

    pub(crate) fn render_top(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let waiting: Vec<&SessionMeta> = self.model.attention().collect();
        let n = waiting.len();
        // The one to jump to: a waiting session other than this one, else this one.
        let first = waiting.iter().find(|s| !self.model.is_active(&s.id)).or(waiting.first()).copied();
        let jump_to = first.filter(|s| !self.model.is_active(&s.id)).map(|s| s.id.clone());
        let what: Option<SharedString> = first.map(|s| {
            if self.model.is_active(&s.id)
                && let Some(pr) = self.model.transcript.pending_prompts().first()
            {
                crate::app::one_line(&format!("{} {}", pr.title, pr.prompt.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")), 90)
                    .into()
            } else {
                s.title.clone().filter(|t| !t.is_empty() && s.name.is_some()).unwrap_or_default().into()
            }
        });
        let on = n > 0;
        let ticker = h_flex()
            .id("attention")
            .gap(px(12.))
            .items_center()
            .child(div().size(px(6.)).flex_none().when(on, |d| d.bg(p.accent)).when(!on, |d| d.border_1().border_color(p.ink4)))
            .child(display(format!("{n}   {}", label::NEED_YOU), 11., if on { p.hi } else { p.ink4 }))
            .when_some(first, |d, s| d.child(mono(s.label(), 11., p.ink)))
            .when_some(what.filter(|w| !w.is_empty()), |d, w| {
                d.child(mono(w, 11., p.ink3).max_w(px(330.)).overflow_hidden().text_ellipsis())
            })
            .when(n > 1, |d| d.child(mono(format!("+{}", n - 1), 11., p.ink3)))
            .when_some(jump_to, |d, id| {
                d.cursor_pointer()
                    .child(h_flex().ml(px(8.)).gap(px(6.)).child(display(label::JUMP, 10.5, p.ink2)).child(mono("→", 11., p.ink2)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for fx in this.model.open_row(&RowAction::Attach(id.clone())) {
                            this.effect(fx);
                        }
                        cx.notify();
                    }))
            });
        let (conn, conn_color): (SharedString, Hsla) = match &self.model.conn {
            _ if self.model.reattaching => ("reattaching".into(), p.ink3),
            ConnState::Connected(v) => (format!("connected · {v}").into(), p.ink4),
            ConnState::Connecting => ("connecting".into(), p.ink3),
            ConnState::Reconnecting => ("reconnecting".into(), p.ink3),
            ConnState::Off(e) | ConnState::Incompatible(e) => {
                (crate::app::one_line(e, 60).into(), if self.synthetic { p.ink4 } else { p.hi })
            }
        };
        // As in the mock: subagents running, here the attached session's
        // (`SubagentRows`, the daemon's own list).
        let subagents =
            self.model.transcript.subagents().iter().filter(|r| r.get("status").and_then(|s| s.as_str()) == Some("running")).count();
        h_flex()
            .h(px(52.))
            .flex_none()
            .pl(px(22.))
            .pr(px(24.))
            .items_center()
            .border_b_1()
            .border_color(p.line)
            .child(h_flex().w(px(254.)).flex_none().gap(px(14.)).child(display(label::WORD, 13., p.ink)).child(display(
                label::WORD_2,
                13.,
                p.ink4,
            )))
            .child(div().flex_1())
            .child(ticker)
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(px(18.))
                    .items_center()
                    .child(mono(conn, 10.5, conn_color).max_w(px(260.)).overflow_hidden().text_ellipsis())
                    .child(
                        h_flex()
                            .id("systems")
                            .gap(px(10.))
                            .items_center()
                            .cursor_pointer()
                            .child(display(label::SYSTEMS, 11., if self.picker_open { p.ink } else { p.ink3 }))
                            .child(
                                div()
                                    .min_w(px(18.))
                                    .h(px(18.))
                                    .px(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(if subagents > 0 { p.ink } else { p.line2 })
                                    .child(mono(subagents.to_string(), 10., if subagents > 0 { p.bg } else { p.ink3 })),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picker_open = !this.picker_open;
                                this.picker_note = None;
                                cx.notify();
                            })),
                    ),
            )
    }

    fn card(
        &self,
        c: Card,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        p: &Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let needs = c.marker == Marker::Awaiting;
        div()
            .id(c.key)
            .relative()
            .px(px(22.))
            .pt(px(14.))
            .pb(px(13.))
            .border_b_1()
            .border_color(p.line)
            .cursor_pointer()
            .when(c.active, |d| d.bg(p.bg2))
            .hover(|d| d.bg(p.bg2))
            .when(c.active || needs, |d| {
                d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(if needs { p.accent } else { p.ink }))
            })
            .child(
                h_flex()
                    .gap(px(12.))
                    .items_baseline()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .text_color(p.ink)
                            .child(c.title),
                    )
                    .child(mono(c.age, 10., p.ink4)),
            )
            .when(!c.sub.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(3.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(p.ink3)
                        .child(c.sub),
                )
            })
            .child(
                h_flex()
                    .mt(px(9.))
                    .items_center()
                    .child(status_chip(c.marker, p))
                    .child(div().flex_1())
                    .when_some(c.cost.filter(|c| *c > 0.0), |d, cost| d.child(mono(format!("${cost:.2}"), 10., p.ink3))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
                cx.notify();
            }))
    }

    pub(crate) fn render_rail(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let filters =
            [(Filter::All, label::ALL), (Filter::Live, label::LIVE), (Filter::Needs, label::NEEDS_YOU), (Filter::Idle, label::IDLE)];
        let mut tabs = h_flex().gap(px(18.)).px(px(22.)).pt(px(18.)).pb(px(14.)).border_b_1().border_color(p.line);
        for (i, (f, text)) in filters.into_iter().enumerate() {
            let on = self.filter == f;
            tabs = tabs.child(
                display(text, 10., if on { p.ink } else { p.ink4 })
                    .id(("filter", i))
                    .pb(px(3.))
                    .border_b_1()
                    .border_color(if on { p.accent } else { gpui_kit::transparent_black() })
                    .cursor_pointer()
                    .hover(|d| d.text_color(p.ink2))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        cx.notify();
                    })),
            );
        }
        let now = now_secs();
        let mut list = v_flex().id("sessions").flex_1().min_h_0().overflow_y_scroll().child(self.render_named(p, now, cx));
        let mut shown = 0;
        for (i, s) in self.model.unnamed_sessions().enumerate() {
            let marker = self.model.marker(s);
            if !self.filter.admits(marker) {
                continue;
            }
            shown += 1;
            let id = s.id.clone();
            let card = Card {
                key: ("session", i),
                title: s.label().into(),
                sub: s.cwd.as_deref().filter(|c| !c.is_empty()).map(home_short).unwrap_or_else(|| short_model(&s.model)).into(),
                age: s.state_since.as_deref().or(Some(s.created_at.as_str())).map(|t| relative(t, now)).unwrap_or_default().into(),
                marker,
                cost: Some(s.cost_usd),
                active: self.model.is_active(&s.id),
            };
            list = list.child(self.card(card, move |this, _| this.attach(id.clone()), p, cx));
        }
        if shown == 0 {
            let empty = if self.model.sessions.is_empty() { "No live sessions" } else { "No unnamed sessions in this filter" };
            list = list.child(div().px(px(22.)).py(px(14.)).text_size(px(12.)).text_color(p.ink4).child(empty));
        }
        v_flex()
            .w(px(276.))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .px(px(22.))
                    .pt(px(24.))
                    .items_baseline()
                    .child(section(label::SESSIONS, fonts::JP_SESSIONS, p))
                    .child(div().flex_1())
                    .when(self.client.is_some(), |d| {
                        d.child(tbtn("new", label::NEW, p.ink3, p.ink).on_click(cx.listener(|this, _, _, cx| {
                            this.create();
                            cx.notify();
                        })))
                    }),
            )
            .child(tabs)
            .child(list)
    }

    /// The named sessions (live and saved), folded into the rail.
    fn render_named(&self, p: &Palette, now: i64, cx: &Context<Self>) -> impl IntoElement {
        let rows = self.model.named_rows();
        let open = !self.named_collapsed;
        let mut section = v_flex().child(
            h_flex()
                .px(px(22.))
                .pt(px(14.))
                .pb(px(8.))
                .items_center()
                .child(
                    h_flex()
                        .id("named-toggle")
                        .flex_1()
                        .gap(px(10.))
                        .cursor_pointer()
                        .child(mono(if open { "−" } else { "+" }, 11., p.ink4))
                        .child(display(label::NAMED, 10., p.ink3))
                        .child(mono(rows.len().to_string(), 10., p.ink4))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.named_collapsed = !this.named_collapsed;
                            cx.notify();
                        })),
                )
                .child(tbtn("open-by-name", label::OPEN, if self.open_shown { p.ink } else { p.ink4 }, p.ink).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.open_shown = !this.open_shown;
                        this.named_collapsed = false;
                        if this.open_shown {
                            this.model.open_shown();
                            this.open_input.update(cx, |s, cx| s.focus(window, cx));
                        }
                        cx.notify();
                    },
                ))),
        );
        if self.open_shown {
            section = section.child(div().px(px(22.)).pb(px(8.)).child(Input::new(&self.open_input)));
        }
        let n = &self.model.named;
        if let Some(e) = &n.error {
            section = section.child(div().px(px(22.)).pb(px(6.)).text_size(px(11.5)).text_color(p.hi).child(e.clone()));
        }
        let notes = [
            n.notice.clone(),
            n.saved_note.clone(),
            n.saved_truncated.then(|| format!("Showing the newest {SAVED_LIMIT} saved sessions.")),
        ];
        for note in notes.into_iter().flatten() {
            section = section.child(div().px(px(22.)).pb(px(6.)).text_size(px(11.)).text_color(p.ink4).child(note));
        }
        if !open {
            return section.child(div().border_b_1().border_color(p.line));
        }
        let mut shown = 0;
        for (i, row) in rows.iter().enumerate() {
            let meta = match &row.action {
                RowAction::Attach(id) => self.model.sessions.iter().find(|s| &s.id == id),
                RowAction::Resume(_) => None,
            };
            // A live row's state from the model (the attached one runs ahead
            // of the poll); "resuming" stays as the model set it.
            let marker = match meta {
                Some(m) if row.marker != Marker::Resuming => self.model.marker(m),
                _ => row.marker,
            };
            if !self.filter.admits(marker) {
                continue;
            }
            shown += 1;
            let sub = meta
                .and_then(|m| m.title.clone().filter(|t| !t.is_empty()))
                .or_else(|| meta.and_then(|m| m.cwd.as_deref()).map(home_short))
                .unwrap_or_else(|| row.model.clone());
            let card = Card {
                key: ("named", i),
                title: row.name.clone().into(),
                sub: sub.into(),
                age: row.activity.as_deref().map(|t| relative(t, now)).unwrap_or_default().into(),
                marker,
                cost: meta.map(|m| m.cost_usd),
                active: meta.is_some_and(|m| self.model.is_active(&m.id)),
            };
            let action = row.action.clone();
            section = section.child(self.card(
                card,
                move |this, _| {
                    for fx in this.model.open_row(&action) {
                        this.effect(fx);
                    }
                },
                p,
                cx,
            ));
        }
        if shown == 0 {
            let empty = if rows.is_empty() { "No named sessions" } else { "None in this filter" };
            section = section.child(div().px(px(22.)).pb(px(12.)).text_size(px(12.)).text_color(p.ink4).child(empty));
        }
        section.child(div().h(px(10.)).border_b_1().border_color(p.line))
    }

    pub(crate) fn render_right(&self, p: &Palette) -> impl IntoElement {
        let s = &self.model.stats;
        let window = s.view.as_ref().map(|v| v.context_window).filter(|&w| w > 0);
        let ratio = s.context_ratio();
        let filled = ratio.map(|r| ((r * SEGMENTS as f64).round() as usize).min(SEGMENTS)).unwrap_or(0);
        let hot = ratio.is_some_and(|r| r >= 0.85);
        let mut segs = h_flex().gap(px(2.)).h(px(8.)).mt(px(20.));
        for i in 0..SEGMENTS {
            segs = segs.child(div().flex_1().h_full().bg(if i < filled { if hot { p.hi } else { p.accent } } else { p.seg }));
        }
        let scale: SharedString = match (s.context, window) {
            (Some(c), Some(w)) => format!("{} of {} tokens", compact(c), compact(w)).into(),
            (None, Some(_)) => "no model call seen yet".into(),
            _ => "the daemon has not sent a window size".into(),
        };
        let context = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .border_b_1()
            .border_color(p.line)
            .child(section(label::CONTEXT, fonts::JP_CONTEXT, p))
            .child(
                h_flex()
                    .mt(px(20.))
                    .items_end()
                    .child(match ratio {
                        Some(r) => h_flex()
                            .items_baseline()
                            .gap(px(3.))
                            .child(display(format!("{:.1}", r * 100.0), 46., if hot { p.hi } else { p.ink }))
                            .child(display("%", 16., p.ink3))
                            .into_any_element(),
                        None => display("—", 46., p.ink4).into_any_element(),
                    })
                    .child(div().flex_1())
                    .when_some(window, |d, w| {
                        d.child(v_flex().items_end().gap(px(5.)).child(display(label::WINDOW, 9.5, p.ink4)).child(mono(
                            compact(w),
                            12.5,
                            p.ink2,
                        )))
                    }),
            )
            .child(segs)
            .child(mono(scale, 9.5, if hot { p.hi } else { p.ink4 }).mt(px(9.)));
        let t = s.tokens;
        let dash = || SharedString::from("—");
        let num = |f: fn(&crate::stats::Tokens) -> u64| t.as_ref().map(|t| SharedString::from(compact(f(t)))).unwrap_or_else(dash);
        let hit = t.and_then(|t| t.cache_hit()).map(|h| SharedString::from(format!("{:.1}%", h * 100.0))).unwrap_or_else(dash);
        let cost = s.cost.map(|c| SharedString::from(format!("${c:.2}"))).unwrap_or_else(dash);
        let tokens = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .border_b_1()
            .border_color(p.line)
            .child(section(label::TOKENS, fonts::JP_TOKENS, p))
            .child(
                h_flex()
                    .mt(px(20.))
                    .child(div().flex_1().child(kv(label::IN, num(|t| t.input), false, p)))
                    .child(div().flex_1().child(kv(label::OUT, num(|t| t.output), false, p)))
                    .child(div().flex_1().child(kv(label::CACHE, num(|t| t.cache_read), false, p))),
            )
            .child(h_flex().mt(px(22.)).child(div().flex_1().child(kv(label::CACHE_HIT, hit, true, p))).child(div().flex_1().child(kv(
                label::COST,
                cost,
                true,
                p,
            ))));
        let ledger = &s.files;
        let (added, removed) = ledger.totals();
        let mut files = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .child(h_flex().items_baseline().child(section(label::FILES, fonts::JP_FILES, p)).child(div().flex_1()).when(
                added + removed > 0,
                |d| {
                    d.child(h_flex().gap(px(8.)).child(mono(format!("+{added}"), 10.5, p.ink3)).child(mono(
                        format!("−{removed}"),
                        10.5,
                        p.hi,
                    )))
                },
            ))
            .child(div().h(px(14.)));
        for f in ledger.files().iter().take(FILES_SHOWN) {
            let (dir, name) = f.split();
            files = files.child(
                h_flex()
                    .h(px(27.))
                    .gap(px(10.))
                    .items_center()
                    .child(mono(f.kind.letter(), 10.5, p.ink4).w(px(10.)).flex_none())
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(mono(dir.to_string(), 11., p.ink4).flex_shrink(1.).min_w(px(14.)).overflow_hidden().text_ellipsis())
                            .child(mono(name.to_string(), 11., p.ink).flex_none()),
                    )
                    .child(mono(format!("+{}", f.added), 10.5, p.ink2).flex_none())
                    .when(f.removed > 0, |d| d.child(mono(format!("−{}", f.removed), 10.5, p.hi).flex_none())),
            );
        }
        let n = ledger.files().len();
        if n > FILES_SHOWN {
            files = files.child(mono(format!("+{} more files", n - FILES_SHOWN), 10.5, p.ink4).mt(px(6.)));
        }
        if n == 0 {
            files = files.child(div().text_size(px(12.)).text_color(p.ink4).child("No file edits in this session"));
        }
        v_flex()
            .id("right")
            .w(px(296.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(p.line)
            .child(context)
            .child(tokens)
            .child(files)
    }

    pub(crate) fn render_picker(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let accent = cx.global::<theme::Accent>();
        let current = accent.hex;
        let d = accent.derived;
        let mut presets = v_flex().mt(px(16.));
        for (i, &(name, hex)) in theme::PRESETS.iter().enumerate() {
            let on = hex == current;
            presets = presets.child(
                h_flex()
                    .id(("preset", i))
                    .h(px(30.))
                    .px(px(6.))
                    .gap(px(12.))
                    .items_center()
                    .cursor_pointer()
                    .when(on, |d| d.bg(p.bg3))
                    .hover(|d| d.bg(p.bg2))
                    .child(div().size(px(14.)).flex_none().bg(rgb(hex)))
                    .child(display(name, 10.5, if on { p.ink } else { p.ink2 }))
                    .child(div().flex_1())
                    .child(mono(theme::to_hex(hex), 10.5, p.ink4))
                    .child(mono(if on { "✓" } else { " " }, 11., p.hi).w(px(10.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_accent(hex, cx))),
            );
        }
        let swatch = |c: u32, name: &'static str| {
            v_flex()
                .gap(px(4.))
                .items_center()
                .child(div().size(px(22.)).bg(rgb(c)).border_1().border_color(p.line3))
                .child(mono(name, 9., p.ink4))
        };
        v_flex()
            .id("picker")
            .absolute()
            .top(px(52.))
            .right(px(24.))
            .w(px(330.))
            .p(px(20.))
            .bg(p.bg1)
            .border_1()
            .border_color(p.line3)
            .occlude()
            .child(
                h_flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(display(label::ACCENT, 11.5, p.ink))
                    .child(mono(theme::to_hex(current), 10.5, p.ink3))
                    .child(div().flex_1())
                    .child(tbtn("picker-close", label::CLOSE, p.ink3, p.ink).on_click(cx.listener(|this, _, _, cx| {
                        this.picker_open = false;
                        cx.notify();
                    }))),
            )
            .child(presets)
            .child(
                h_flex()
                    .mt(px(16.))
                    .gap(px(12.))
                    .items_center()
                    .child(display(label::CUSTOM, 10., p.ink4).w(px(62.)))
                    .child(div().flex_1().child(Input::new(&self.hex_input)))
                    .child(
                        tbtn("picker-apply", label::APPLY, p.ink2, p.ink)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_custom_accent(cx))),
                    ),
            )
            .child(
                h_flex()
                    .mt(px(18.))
                    .gap(px(12.))
                    .child(swatch(d.accent, "fill"))
                    .child(swatch(d.hi, "text"))
                    .child(swatch(d.mid, "line"))
                    .child(swatch(d.bg, "card"))
                    .child(swatch(d.soft, "soft")),
            )
            .when(d.lifted, |el| {
                el.child(mono("too dark to see: lifted to 3:1 (fills), 4.5:1 (text)", 10., p.ink3).mt(px(12.)).whitespace_normal())
            })
            .when_some(self.picker_note.clone(), |el, n| el.child(mono(n, 10., p.ink4).mt(px(10.)).whitespace_normal()))
    }
}
