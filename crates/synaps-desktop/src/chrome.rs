//! The window's frame around the session: the mark strip, the top bar (what
//! needs you, connection, the accent picker), the sessions rail (find, the
//! count chips, PINNED / OPEN / HISTORY, the hover readout) and the
//! right-hand numbers.

use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use std::hash::{Hash, Hasher};

use gpui_kit::component::tooltip::Tooltip;

use crate::app::{AppView, STRIP, Side, ToggleRail, ToggleRight, display, mono, section, tbtn};
use crate::fonts::{self, label};
use crate::model::{ConnState, Model};
use crate::named::RowAction;
use crate::rail::{self, Chip, Item, Rail, State};
use crate::stats::compact;
use crate::theme::{self, Palette};

/// The mark strip at the window's left edge.
pub(crate) const MARK: f32 = 30.;
/// Context-gauge segments (the folded panel's upright gauge: `STRIP_SEGMENTS`).
const SEGMENTS: usize = 48;
const STRIP_SEGMENTS: usize = 24;
/// Files listed before "+N more".
const FILES_SHOWN: usize = 40;

fn kv(key: &'static str, value: impl Into<SharedString>, big: bool, p: &Palette) -> impl IntoElement {
    let value = value.into();
    v_flex().gap(px(5.)).child(display(key, 9.5, p.ink4)).child(if big {
        display(value, 17., p.ink).into_any_element()
    } else {
        mono(value, 12.5, p.ink).into_any_element()
    })
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The rail, built when the lists or the user's view of them change (or a
/// minute passes, for the ages), never per frame. The list is spliced only
/// where items changed, so the rest keep their place and measured height.
#[derive(Default)]
pub(crate) struct RailCache {
    generation: Option<u64>,
    minute: i64,
    view: rail::View,
    states: u64,
    pub(crate) rail: Rail,
    keys: Vec<u64>,
}

impl RailCache {
    /// The minute the ages were last written in.
    pub(crate) fn minute(&self) -> i64 {
        self.minute
    }

    pub(crate) fn refresh(&mut self, m: &Model, view: &rail::View, list: &ListState) {
        let now = now_secs();
        // With a chip on, which rows show follows their states (the attached
        // session's run ahead of the poll): a hash, no allocation.
        let states = if let Some(chip) = view.chip {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for s in &m.sessions {
                m.chip_shows(chip, s).hash(&mut h);
            }
            h.finish()
        } else {
            0
        };
        if self.generation == Some(m.rail_gen) && self.minute == now / 60 && self.view == *view && self.states == states {
            return;
        }
        let rail = rail::build(m, view, now);
        let keys: Vec<u64> = rail.items.iter().map(|i| rail::item_key(&rail, i)).collect();
        if let Some((range, count)) = rail::splice(&self.keys, &keys) {
            list.splice(range, count);
        }
        *self = RailCache { generation: Some(m.rail_gen), minute: now / 60, view: view.clone(), states, rail, keys };
    }
}

/// A state's 10 px glyph: one or two axis-aligned rectangles, never a font
/// glyph (the embedded fonts are subsets). The accent only for NEEDS YOU.
fn glyph(st: State, p: &Palette) -> Div {
    let at = |inset: f32| div().absolute().top(px(inset)).left(px(inset)).size(px(10. - 2. * inset));
    let g = div().relative().size(px(10.)).flex_none();
    match st {
        State::NeedsYou => g.child(at(0.).border_1().border_color(p.accent)).child(at(3.).bg(p.accent)),
        State::Running => g.child(at(1.).bg(p.ink)),
        State::Unread => g.child(at(3.).bg(p.ink)),
        State::Idle => g.child(at(1.).border_1().border_color(p.ink3)),
        State::Parked => g.child(div().absolute().left(px(1.)).bottom(px(1.)).w(px(8.)).h(px(2.)).bg(p.ink3)),
        State::Opening => g.child(at(1.).border_t_1().border_l_1().border_color(p.ink2)),
        State::Locked => {
            g.child(at(1.).border_1().border_color(p.ink3)).child(div().absolute().left(px(1.)).top(px(4.)).w(px(8.)).h(px(2.)).bg(p.ink3))
        }
        State::Saved => g,
    }
}

fn word_color(st: State, p: &Palette) -> Hsla {
    match st {
        State::NeedsYou => p.hi,
        State::Running => p.ink,
        State::Locked => p.ink3,
        _ => p.ink2,
    }
}

/// What a top-bar ticker says about a prompt: the tool and the first line
/// of what it will run or asks, from the daemon's confirm body
/// (`Extension: …`, `Tool: …`, `Input that will run:`, `Message from …:`),
/// else the body's first line that is not the closing question.
pub(crate) fn prompt_gist(title: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let tool = lines.iter().find_map(|l| l.strip_prefix("Tool: ")).filter(|t| !t.is_empty());
    let after = |head: fn(&str) -> bool| lines.iter().skip_while(|l| !head(l)).skip(1).find(|l| !l.is_empty()).copied();
    let detail =
        after(|l| l == "Input that will run:").or_else(|| after(|l| l.starts_with("Message from ") && l.ends_with(':'))).or_else(|| {
            lines
                .iter()
                .copied()
                .find(|l| !l.is_empty() && *l != "Allow this tool call?" && !l.starts_with("Extension: ") && !l.starts_with("Tool: "))
        });
    let text = match (tool, detail) {
        (Some(t), Some(d)) => format!("{t}: {d}"),
        (Some(t), None) => t.to_string(),
        (None, Some(d)) => d.to_string(),
        (None, None) => title.to_string(),
    };
    crate::app::one_line(&text, 90)
}

fn fold(id: &'static str, glyph: &'static str, tip: &'static str, rail: bool, p: &Palette, cx: &Context<AppView>) -> Stateful<Div> {
    let hover = p.ink4;
    div()
        .id(id)
        .size(px(20.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(p.line2)
        .hover(move |d| d.border_color(hover))
        .child(mono(glyph, 12., p.ink3))
        .tooltip(move |window, cx| {
            let t = Tooltip::new(tip);
            if rail { t.action(&ToggleRail, None) } else { t.action(&ToggleRight, None) }.build(window, cx)
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(rail, cx)))
}

impl AppView {
    /// The connection disc's colour (the mark strip and the folded rail).
    fn conn_disc(&self, p: &Palette) -> Hsla {
        match &self.model.conn {
            ConnState::Connected(_) => p.accent,
            _ if self.synthetic => p.accent,
            ConnState::Connecting | ConnState::Reconnecting => p.ink4,
            _ => p.line3,
        }
    }

    pub(crate) fn render_mark(&self, p: &Palette) -> impl IntoElement {
        let disc = self.conn_disc(p);
        v_flex()
            .w(px(MARK))
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

    pub(crate) fn render_top(&self, p: &Palette, rail: Side, cx: &Context<Self>) -> impl IntoElement {
        let n = self.model.attention().count();
        let first = self.model.ticker();
        let jump_to = self.model.jump_target().map(str::to_string);
        let what: Option<SharedString> = first.map(|s| {
            if self.model.is_active(&s.id)
                && let Some(pr) = self.model.transcript.pending_prompts().first()
            {
                // Built once per prompt, with the card (`sync_card` ran first).
                self.current_card().map(|c| c.gist.clone()).unwrap_or_else(|| prompt_gist(&pr.title, &pr.prompt).into())
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
            // Over the open rail: as wide as it.
            .child(
                h_flex()
                    .w(px(match rail {
                        Side::Open(w) => w - 22.,
                        _ => 254.,
                    }))
                    .flex_none()
                    .gap(px(14.))
                    .child(display(label::WORD, 13., p.ink))
                    .child(display(label::WORD_2, 13., p.ink4)),
            )
            .child(div().flex_1())
            .child(ticker)
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(px(18.))
                    .items_center()
                    .child(mono(conn, 10.5, conn_color).max_w(px(260.)).overflow_hidden().text_ellipsis())
                    // The accent picker's own control: a swatch of the accent.
                    .child(
                        h_flex()
                            .id("accent-picker")
                            .gap(px(8.))
                            .items_center()
                            .cursor_pointer()
                            .child(div().size(px(10.)).bg(p.accent))
                            .child(display(label::ACCENT, 10.5, if self.picker_open { p.ink } else { p.ink4 }))
                            .hover(|d| d.text_color(p.ink))
                            .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                                // The click whose mouse-down closed it from outside does not reopen it.
                                let dismissed = this.picker_dismissed.take();
                                if let ClickEvent::Mouse(m) = ev
                                    && dismissed == Some(m.down.position)
                                {
                                    return;
                                }
                                if this.picker_open {
                                    this.close_picker(window, cx);
                                } else {
                                    this.open_picker(window, cx);
                                }
                            })),
                    )
                    // As in the mock: the attached session's running subagents.
                    .child(
                        h_flex().gap(px(10.)).items_center().child(display(label::SYSTEMS, 11., p.ink3)).child(
                            div()
                                .min_w(px(18.))
                                .h(px(18.))
                                .px(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if subagents > 0 { p.ink } else { p.line2 })
                                .child(mono(subagents.to_string(), 10., if subagents > 0 { p.bg } else { p.ink3 })),
                        ),
                    ),
            )
    }

    /// The folded rail: the connection disc and how many sessions need
    /// you. A click (or Ctrl+B) opens it again.
    pub(crate) fn render_rail_strip(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let n = self.model.attention().count();
        let on = n > 0;
        // L1: a window too narrow for the rail keeps its strip and says so.
        let fits = crate::app::toggled(self.width, self.panels, true).is_some();
        v_flex()
            .id("rail-strip")
            .w(px(STRIP))
            .h_full()
            .flex_none()
            .items_center()
            .pt(px(22.))
            .gap(px(18.))
            .border_r_1()
            .border_color(p.line)
            .cursor_pointer()
            .hover(|d| d.bg(p.bg1))
            .child(mono("›", 12., p.ink3))
            .child(div().size(px(8.)).rounded_full().bg(self.conn_disc(p)))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(6.)).when(on, |d| d.bg(p.accent)).when(!on, |d| d.border_1().border_color(p.ink4)))
                    .child(display(n.to_string(), 13., if on { p.hi } else { p.ink4 })),
            )
            .tooltip(move |window, cx| {
                let tip = match (on, fits) {
                    (_, false) => format!("{n} need you · widen the window to show sessions"),
                    (true, true) => format!("{n} need you · show sessions"),
                    (false, true) => "Show sessions".to_string(),
                };
                Tooltip::new(tip).action(&ToggleRail, None).build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(true, cx)))
    }

    /// The folded right panel: the context gauge, upright, and its
    /// percentage. A click (or Ctrl+Alt+B) opens it again.
    pub(crate) fn render_right_strip(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let ratio = self.model.stats.context_ratio();
        let hot = ratio.is_some_and(|r| r >= 0.85);
        let filled = ratio.map(|r| ((r * STRIP_SEGMENTS as f64).round() as usize).min(STRIP_SEGMENTS)).unwrap_or(0);
        let mut gauge = v_flex().w(px(6.)).gap(px(2.));
        for i in (0..STRIP_SEGMENTS).rev() {
            gauge = gauge.child(div().w_full().h(px(4.)).bg(if i < filled { if hot { p.hi } else { p.accent } } else { p.seg }));
        }
        let pct: SharedString = ratio.map(|r| format!("{:.0}", r * 100.0)).unwrap_or_else(|| "—".into()).into();
        let fits = crate::app::toggled(self.width, self.panels, false).is_some();
        // L2: detached, the last numbers seen are dimmed, as the open panel's are.
        let detached = self.model.stats.detached && self.model.active.is_none();
        v_flex()
            .id("right-strip")
            .w(px(STRIP))
            .h_full()
            .flex_none()
            .items_center()
            .pt(px(22.))
            .gap(px(14.))
            .border_l_1()
            .border_color(p.line)
            .cursor_pointer()
            .hover(|d| d.bg(p.bg1))
            .child(mono("‹", 12., p.ink3))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(14.))
                    .when(detached, |d| d.opacity(0.45))
                    .child(v_flex().items_center().child(display(pct, 13., if hot { p.hi } else { p.ink2 })).child(mono("%", 9., p.ink4)))
                    .child(gauge),
            )
            .tooltip(move |window, cx| {
                let tip = match (detached, fits) {
                    (_, false) => "Widen the window to show context, tokens and files",
                    (true, true) => "Detached: last numbers seen · show context, tokens and files",
                    (false, true) => "Show context, tokens and files",
                };
                Tooltip::new(tip).action(&ToggleRight, None).build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(false, cx)))
    }

    pub(crate) fn render_rail(&self, width: f32, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let n = &self.model.named;
        let notes: Vec<(&String, Hsla)> =
            n.error.iter().map(|e| (e, p.hi)).chain(n.notice.iter().chain(n.saved_note.iter()).map(|t| (t, p.ink4))).collect();
        // The chips count what is held (the attached session as it runs,
        // ahead of the poll) and the unread history rows.
        let (mut needs, mut running, mut unread) = (0, 0, self.rail.rail.history_unread);
        for s in self.model.sessions.iter().filter(|s| s.lifecycle != "ending") {
            match self.model.fact(s) {
                State::NeedsYou => needs += 1,
                State::Running => running += 1,
                State::Unread => unread += 1,
                _ => {}
            }
        }
        // A chip toggles its filter; none selected shows everything.
        let mut chips = h_flex().items_center().gap(px(15.)).px(px(22.)).pt(px(13.)).pb(px(12.)).border_b_1().border_color(p.line);
        for (c, count) in [(Chip::NeedsYou, needs), (Chip::Running, running), (Chip::Unread, unread)] {
            let on = self.chip == Some(c);
            let tip = match c {
                Chip::NeedsYou => "Show only sessions that need you",
                Chip::Running => "Show only running sessions",
                Chip::Unread => "Show only sessions with news since you looked",
            };
            chips = chips.child(
                h_flex()
                    .id(("chip", c as usize))
                    .items_baseline()
                    .gap(px(5.))
                    .pb(px(3.))
                    .border_b_1()
                    .border_color(if on { p.ink } else { gpui_kit::transparent_black() })
                    .cursor_pointer()
                    .child(display(c.label(), 10., if on { p.ink } else { p.ink3 }))
                    .child(mono(count.to_string(), 10., if c == Chip::NeedsYou && count > 0 { p.hi } else { p.ink2 }))
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.chip = if this.chip == Some(c) { None } else { Some(c) };
                        cx.notify();
                    })),
            );
        }
        let find = h_flex()
            .mx(px(22.))
            .mt(px(16.))
            .h(px(27.))
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(p.line2)
            .child(
                div().flex_1().min_w_0().font_family(fonts::MONO).text_size(px(10.5)).child(Input::new(&self.find_input).appearance(false)),
            )
            .child(div().px(px(5.)).border_1().border_color(p.line2).child(mono("/", 9.5, p.ink4)));
        let items = list(self.rail_list.clone(), cx.processor(|this, ix, _window, cx| this.render_rail_item(ix, cx))).size_full();
        v_flex()
            .w(px(width))
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
                    })
                    .child(fold("rail-fold", "‹", "Hide sessions", true, p, cx).ml(px(16.))),
            )
            .child(find)
            .when(!notes.is_empty(), |d| {
                d.child(
                    v_flex()
                        .px(px(22.))
                        .pt(px(8.))
                        .gap(px(4.))
                        .children(notes.into_iter().map(|(t, c)| div().text_size(px(11.)).text_color(c).child(t.clone()))),
                )
            })
            .child(chips)
            .child(div().flex_1().min_h_0().child(items))
    }

    /// One item of the rail's list (only the visible ones are built).
    pub(crate) fn render_rail_item(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let p = theme::palette(cx);
        let rail = &self.rail.rail;
        let Some(item) = rail.items.get(ix) else { return div().into_any_element() };
        match item {
            Item::Group { group, count } => h_flex()
                .w_full()
                .px(px(22.))
                .pt(px(17.))
                .pb(px(7.))
                .items_baseline()
                .gap(px(9.))
                .when(ix > 0, |d| d.border_t_1().border_color(p.line))
                .child(display(group.label(), 10., p.ink3))
                .child(div().font_family(fonts::JP).text_size(px(9.5)).text_color(p.ink4).child(group.jp()))
                .child(div().flex_1())
                .child(mono(count, 10., p.ink4))
                .into_any_element(),
            Item::Row(r) => self.render_row2(*r, &p, cx),
            Item::Hist(r) => self.render_hist(*r, &p, cx),
            Item::Bucket { bucket, open, count } => {
                let b = *bucket;
                let color = if *open { p.ink3 } else { p.ink4 };
                h_flex()
                    .id(("bucket", b as usize))
                    .w_full()
                    .px(px(22.))
                    .pt(px(7.))
                    .pb(px(6.))
                    .gap(px(10.))
                    .items_baseline()
                    .cursor_pointer()
                    .hover(|d| d.bg(p.bg1))
                    .child(div().w(px(10.)).flex_none().child(mono(if *open { "−" } else { "+" }, 11., color)))
                    .child(display(b.label(), 9.5, color))
                    .child(div().flex_1())
                    .child(mono(count, 10., p.ink4))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_bucket(b, cx)))
                    .into_any_element()
            }
            Item::More { bucket, text } => {
                let b = *bucket;
                div()
                    .id(("more", b as usize))
                    .w_full()
                    .pl(px(42.))
                    .pr(px(22.))
                    .pt(px(5.))
                    .pb(px(8.))
                    .cursor_pointer()
                    .child(display(text, 9.5, p.ink4))
                    .hover(|d| d.bg(p.bg1))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expanded |= b.bit();
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Item::Note(t) => div()
                .px(px(22.))
                .pt(px(2.))
                .pb(px(10.))
                .font_family(fonts::MONO)
                .text_size(px(10.))
                .line_height(px(15.))
                .text_color(p.ink4)
                .child(SharedString::from(t))
                .into_any_element(),
            Item::Empty(t) => div().px(px(22.)).py(px(10.)).text_size(px(12.)).text_color(p.ink4).child(*t).into_any_element(),
        }
    }

    /// A PINNED or OPEN row: bar, glyph, title, age; then the state word
    /// (if not at rest), the folder or a named session's title, the cost.
    /// Its state is read per frame (the attached session runs ahead of the
    /// poll), its strings come from the cache; it never changes shape.
    fn render_row2(&self, r: usize, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let row = &self.rail.rail.rows[r];
        let meta = row.live.and_then(|i| self.model.sessions.get(i));
        let st = meta.map(|s| self.model.state(s)).unwrap_or(row.state);
        let on = meta.is_some_and(|s| self.model.is_active(&s.id));
        // The attached session's cost as the panel shows it (its digests
        // run ahead of the 3 s poll), else the poll's.
        let cost = meta.map(|s| if on { self.model.stats.cost.unwrap_or(s.cost_usd) } else { s.cost_usd }).filter(|c| *c > 0.0);
        let waiting = meta.map(|s| if on { self.model.transcript.pending_prompts().len() } else { s.awaiting_input }).unwrap_or(0);
        let title_color = match st {
            State::NeedsYou | State::Running | State::Unread => p.ink,
            _ if on => p.ink,
            State::Parked | State::Locked => p.ink3,
            _ if row.fresh => p.ink3,
            _ => p.ink2,
        };
        let sub_color = if matches!(st, State::Parked | State::Locked) { p.ink4 } else { p.ink3 };
        let bar = if st == State::NeedsYou {
            Some(p.accent)
        } else if on {
            Some(p.ink)
        } else {
            None
        };
        let key = row.key;
        let word = st.word().map(|w| {
            h_flex()
                .flex_none()
                .items_baseline()
                .gap(px(5.))
                .child(display(w, 9.5, word_color(st, p)))
                .when(st == State::NeedsYou && waiting > 1, |d| d.child(mono(waiting.to_string(), 10., p.hi)))
        });
        div()
            .id(("rail-row", key as usize))
            .relative()
            .w_full()
            .px(px(22.))
            .pt(px(8.))
            .pb(px(9.))
            .cursor_pointer()
            .when(on, |d| d.bg(p.bg2))
            .hover(|d| d.bg(p.bg2))
            .when_some(bar, |d, c| d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(c)))
            .child(
                h_flex().gap(px(10.)).items_start().child(glyph(st, p).mt(px(5.))).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(
                            h_flex()
                                .h(px(18.))
                                .gap(px(10.))
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .when(row.named, |d| d.font_family(fonts::MONO).text_size(px(12.)))
                                        .when(!row.named, |d| d.text_size(px(12.5)))
                                        .text_color(title_color)
                                        .child(SharedString::from(&row.title)),
                                )
                                .child(mono(&row.age, 10., p.ink4)),
                        )
                        .child(
                            h_flex()
                                .h(px(17.))
                                .gap(px(8.))
                                .items_center()
                                .min_w_0()
                                .children(word)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .when(row.sub_mono, |d| d.font_family(fonts::MONO).text_size(px(10.5)))
                                        .when(!row.sub_mono, |d| d.text_size(px(11.)))
                                        .text_color(sub_color)
                                        .child(SharedString::from(&row.sub)),
                                )
                                .when_some(cost, |d, c| d.child(mono(format!("${c:.2}"), 10., p.ink3).flex_none())),
                        ),
                ),
            )
            .when(self.readout == Some(key), |d| d.child(self.render_readout(row, p)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_row(key, *hovered, cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.rail_click(key);
                cx.notify();
            }))
            .into_any_element()
    }

    /// A HISTORY row: one fixed-height line, title and age, a mark only if
    /// it is unread (or being opened).
    fn render_hist(&self, r: usize, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let row = &self.rail.rail.rows[r];
        let key = row.key;
        let title_color = match row.state {
            State::Unread => p.ink,
            State::Opening => p.ink2,
            _ if row.named => p.ink2,
            _ => p.ink3,
        };
        h_flex()
            .id(("rail-hist", key as usize))
            .relative()
            .w_full()
            .h(px(25.))
            .px(px(22.))
            .gap(px(10.))
            .items_center()
            .cursor_pointer()
            .hover(|d| d.bg(p.bg2))
            .child(glyph(row.state, p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(row.named, |d| d.font_family(fonts::MONO).text_size(px(11.5)))
                    .when(!row.named, |d| d.text_size(px(12.)))
                    .text_color(title_color)
                    .child(SharedString::from(&row.title)),
            )
            .child(mono(&row.age, 10., p.ink4))
            .when(self.readout == Some(key), |d| d.child(self.render_readout(row, p)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_row(key, *hovered, cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.rail_click(key);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The hover readout, beside the row over the session pane: drawn on
    /// top of everything (not clipped by the rail's scroll), only after the
    /// pointer has rested 400 ms.
    fn render_readout(&self, row: &rail::Row, p: &Palette) -> AnyElement {
        let now = now_secs();
        let r = match (row.live.and_then(|i| self.model.sessions.get(i)), row.saved.and_then(|s| rail::saved_entry(&self.model, s))) {
            (Some(s), _) => rail::readout_live(&self.model, s, now),
            (None, Some(v)) => rail::readout_saved(&self.model, v, now),
            _ => return div().into_any_element(),
        };
        let mut body = v_flex()
            .w(px(296.))
            .px(px(16.))
            .py(px(13.))
            .gap(px(4.))
            .bg(p.bg1)
            .border_1()
            .border_color(p.line3)
            .child(div().text_size(px(12.5)).text_color(p.ink).pb(px(5.)).child(r.title));
        for (k, value, aside) in r.lines {
            body = body.child(
                h_flex().items_start().child(div().w(px(62.)).flex_none().pt(px(2.)).child(display(k, 9., p.ink4))).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .font_family(fonts::MONO)
                        .text_size(px(10.5))
                        .child(div().text_color(p.ink2).child(value))
                        .when(!aside.is_empty(), |d| d.child(div().text_color(p.ink4).child(aside))),
                ),
            );
        }
        div()
            .absolute()
            .top_0()
            .left_0()
            .child(deferred(anchored().offset(point(px(self.rail_width + 12.), px(2.))).child(body)).with_priority(2))
            .into_any_element()
    }

    pub(crate) fn render_right(&self, width: f32, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
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
            .child(h_flex().items_center().child(section(label::CONTEXT, fonts::JP_CONTEXT, p)).child(div().flex_1()).child(fold(
                "right-fold",
                "›",
                "Hide context, tokens and files",
                false,
                p,
                cx,
            )))
            .child(
                h_flex()
                    .mt(px(20.))
                    .items_end()
                    .child(match ratio {
                        // Bottom-aligned with the `%` lifted onto the
                        // number's baseline (GPUI flex has no text baseline).
                        Some(r) => h_flex()
                            .items_end()
                            .gap(px(3.))
                            .child(display(format!("{:.1}", r * 100.0), 46., if hot { p.hi } else { p.ink }).line_height(px(46.)))
                            .child(display("%", 16., p.ink3).line_height(px(16.)).pb(px(5.)))
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
                    .child(div().flex_1().child(kv(label::CACHE, num(|t| t.cache_read), false, p)))
                    .child(div().flex_1().child(kv(label::WRITE, num(|t| t.cache_creation), false, p))),
            )
            .child(h_flex().mt(px(22.)).child(div().flex_1().child(kv(label::CACHE_HIT, hit, true, p))).child(div().flex_1().child(kv(
                label::COST,
                cost,
                true,
                p,
            ))));
        let ledger = &s.files;
        let (added, removed) = ledger.totals();
        // An estimate is never shown as an exact count (`files::changed_lines`).
        let tilde = |approx: bool| if approx { "~" } else { "" };
        let approx = tilde(ledger.approx());
        let mut files = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .child(h_flex().items_end().child(section(label::FILES, fonts::JP_FILES, p)).child(div().flex_1()).when(
                added + removed > 0,
                |d| {
                    d.child(h_flex().gap(px(8.)).child(mono(format!("{approx}+{added}"), 10.5, p.ink3)).child(mono(
                        format!("−{removed}"),
                        10.5,
                        p.hi,
                    )))
                },
            ))
            .child(mono("as the model asked: tool inputs, not the disk", 9.5, p.ink4).mt(px(6.)))
            .child(div().h(px(12.)));
        for f in ledger.files().iter().take(FILES_SHOWN) {
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
                            .child(mono(f.dir.clone(), 11., p.ink4).flex_shrink(1.).min_w(px(14.)).overflow_hidden().text_ellipsis())
                            .child(mono(f.name.clone(), 11., p.ink).flex_none()),
                    )
                    // A write is the lines written, not a delta (`~`).
                    .when(f.written > 0, |d| d.child(mono(format!("~{}", f.written), 10.5, p.ink3).flex_none()))
                    .when(f.added > 0 || f.written == 0, |d| {
                        d.child(mono(format!("{}+{}", tilde(f.approx), f.added), 10.5, p.ink2).flex_none())
                    })
                    .when(f.removed > 0, |d| d.child(mono(format!("−{}", f.removed), 10.5, p.hi).flex_none())),
            );
        }
        let n = ledger.files().len();
        if n > FILES_SHOWN {
            files = files.child(mono(format!("+{} more files", n - FILES_SHOWN), 10.5, p.ink4).mt(px(6.)));
        }
        let shell = ledger.shell();
        if n == 0 && shell.is_empty() {
            files = files.child(div().text_size(px(12.)).text_color(p.ink4).child("No file edits in this session"));
        }
        if !shell.is_empty() {
            // Read from `bash` command lines: approximate, no line counts,
            // not in the totals above.
            files = files
                .child(
                    h_flex()
                        .mt(px(if n == 0 { 0. } else { 18. }))
                        .gap(px(10.))
                        .items_baseline()
                        .child(display(label::VIA_SHELL, 10., p.ink2))
                        .child(mono("~", 11., p.ink3)),
                )
                .child(mono("bash commands that succeeded; no counts", 9.5, p.ink4).mt(px(6.)))
                .child(div().h(px(8.)));
            for f in shell.iter().take(FILES_SHOWN) {
                files = files.child(
                    h_flex()
                        .h(px(25.))
                        .gap(px(10.))
                        .items_center()
                        .child(mono("$", 10.5, p.ink4).w(px(10.)).flex_none())
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(mono(f.dir.clone(), 11., p.ink4).flex_shrink(1.).min_w(px(14.)).overflow_hidden().text_ellipsis())
                                .child(mono(f.name.clone(), 11., p.ink2).flex_none()),
                        )
                        .child(mono(f.verb, 10., p.ink3).flex_none()),
                );
            }
            if shell.len() > FILES_SHOWN {
                files = files.child(mono(format!("+{} more", shell.len() - FILES_SHOWN), 10.5, p.ink4).mt(px(6.)));
            }
        }
        // Detached: the last numbers seen, dimmed and labelled, not live.
        let detached = s.detached && self.model.active.is_none();
        v_flex()
            .id("right")
            .w(px(width))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(p.line)
            .when(detached, |d| {
                d.child(
                    h_flex()
                        .px(px(24.))
                        .py(px(10.))
                        .gap(px(10.))
                        .border_b_1()
                        .border_color(p.line)
                        .child(display("D E T A C H E D", 10., p.hi))
                        .child(mono("last numbers seen", 10., p.ink4)),
                )
            })
            .child(v_flex().when(detached, |d| d.opacity(0.45)).child(context).child(tokens).child(files))
    }

    /// The accent picker: over the open right-hand column (clear of the
    /// header's NAME and HALT), else above the composer at the right, under
    /// its ACCENT button either way.
    /// Esc (the root's key handler) or a click outside closes it.
    pub(crate) fn render_picker(&self, p: &Palette, right: Side, cx: &Context<Self>) -> impl IntoElement {
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
            .map(|d| match right {
                Side::Open(w) => d.top(px(52.)).right_0().w(px(w)),
                Side::Strip => d.bottom(px(140.)).right(px(right.width() + 24.)).w(px(296.)),
            })
            .p(px(20.))
            .bg(p.bg1)
            .border_1()
            .border_color(p.line3)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                this.picker_dismissed = Some(ev.position);
                this.close_picker(window, cx);
            }))
            .child(
                h_flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(display(label::ACCENT, 11.5, p.ink))
                    .child(mono(theme::to_hex(current), 10.5, p.ink3))
                    .child(div().flex_1())
                    .child(tbtn("picker-close", label::CLOSE, p.ink3, p.ink).on_click(cx.listener(|this, _, window, cx| {
                        this.close_picker(window, cx);
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
                el.child(mono("too dark to see: lifted to 3:1 (fills, lines), 4.5:1 (text)", 10., p.ink3).mt(px(12.)).whitespace_normal())
            })
            .when_some(self.picker_note.clone(), |el, n| el.child(mono(n, 10., p.ink4).mt(px(10.)).whitespace_normal()))
    }
}

#[cfg(test)]
mod tests {
    use super::prompt_gist;

    #[test]
    fn the_ticker_names_the_tool_and_what_it_runs() {
        let body = "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/w:\n$ cargo test\n\nAllow this tool call?";
        assert_eq!(prompt_gist("Confirm tool call", body), "bash: bash wants to run a command in ~/w:");
        let rewritten = "Extension: a, b\nTool: edit\nInput rewritten by: b\nInput that will run:\n{\"path\": \"/w/x\"}\n\nMessage from a, b:\nok?\n\nAllow this tool call?";
        assert_eq!(prompt_gist("Confirm tool call", rewritten), "edit: {\"path\": \"/w/x\"}");
        assert_eq!(prompt_gist("Confirm tool activation", "Activate shell_start?"), "Activate shell_start?");
        assert_eq!(prompt_gist("API key", ""), "API key");
    }
}
