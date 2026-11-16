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

use crate::app::{AppView, EdgeDrag, STRIP, Side, ToggleRail, ToggleRight, count, display, mono, section, tbtn};
use crate::fonts::{self, label};
use crate::model::{ConnState, Model};
use crate::named::RowAction;
use crate::rail::{self, Chip, Item, Rail, State};
use crate::stats::compact;
use crate::tabs::Place;
use crate::theme::{self, Palette};

/// Left inset of the layout before the rail. The mark strip that used to
/// sit here (connection disc over 荒坂) is gone; the disc is in the top bar.
pub(crate) const MARK: f32 = 0.;
/// Context-gauge segments (the folded panel's upright gauge: `STRIP_SEGMENTS`).
const SEGMENTS: usize = 48;
const STRIP_SEGMENTS: usize = 24;
/// The gauge turns `error_color` from here (the TUI's `context_bar_color`).
const HOT: f64 = 0.75;
/// Files listed before "+N more".
const FILES_SHOWN: usize = 40;
/// The top bar's height: the side panels and their edges start under it.
pub(crate) const TOP: f32 = 52.;
/// In a short window ([`crate::turns::Layout::short`]): its one line of
/// 13-px caps and the 18-px SYSTEMS badge still have 11 px above and below.
pub(crate) const TOP_SHORT: f32 = 40.;
/// An edge handle's hit area; its line is 1 px, on the panel's border.
const EDGE: f32 = 6.;
/// The theme picker's narrowest width: two columns of theme names.
const PICKER_MIN: f32 = 340.;
/// The tab strip and the session header (full, and one row): a picker
/// above the composer keeps below them and the top bar. The full one's
/// clearance leaves out the header's 14-px bottom padding (the picker may
/// cover that, not the title row or the state).
const HEADER_CLEAR: f32 = crate::tabstrip::TAB_H + crate::fit::HEADER_FULL - 14.;
const HEADER_CLEAR_SHORT: f32 = crate::tabstrip::TAB_H + crate::fit::HEADER_SHORT + 1.;

/// The top bar's fixed words, measured in the faces they are drawn in.
pub(crate) fn top_words(window: &Window) -> crate::fit::TopWords {
    let width = |text: &str, family: &'static str, weight: FontWeight, size: f32| {
        let text = SharedString::from(text.to_string());
        let run = TextRun {
            len: text.len(),
            font: Font { weight, ..font(family) },
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(window.text_system().shape_line(text, px(size), &[run], None).width).ceil()
    };
    let display = |text, size| width(text, fonts::DISPLAY, FontWeight::SEMIBOLD, size);
    let cell = |size| width("0000000000", fonts::MONO, FontWeight::NORMAL, size) / 10.;
    crate::fit::TopWords {
        word: display(label::WORD, 13.),
        word2: display(label::WORD_2, 13.),
        need: display(&format!("   {}", label::NEED_YOU), 11.),
        digit: display("0", 11.),
        jump: display(label::JUMP, 10.5),
        theme: display(label::THEME, 10.5),
        systems: display(label::SYSTEMS, 11.),
        cell: cell(11.),
        cell_conn: cell(10.5),
        cell_badge: cell(10.),
    }
}

/// The top bar's height this frame.
pub(crate) fn top_h(short: bool) -> f32 {
    if short { TOP_SHORT } else { TOP }
}

/// The TUI's `context_bar_color`: `border_active` under 50 %,
/// `status_streaming` under 75 %, `error_color` above.
fn gauge_color(ratio: Option<f64>, p: &Palette) -> Hsla {
    match ratio.unwrap_or(0.) {
        r if r < 0.5 => p.focus_ch,
        r if r < HOT => p.run_mark_ch,
        _ => p.err_mark_ch,
    }
}

fn kv(key: &'static str, value: impl Into<SharedString>, big: bool, color: Hsla, p: &Palette) -> impl IntoElement {
    let value = value.into();
    v_flex().gap(px(5.)).child(display(key, 9.5, p.dim_ch)).child(if big {
        display(value, 17., color).into_any_element()
    } else {
        mono(value, 12.5, color).into_any_element()
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
/// glyph (the embedded fonts are subsets), in the state's theme role
/// (rail recommendation, section 3): at rest, `muted`, lifted.
pub(crate) fn glyph(st: State, p: &Palette) -> Div {
    let at = |inset: f32| div().absolute().top(px(inset)).left(px(inset)).size(px(10. - 2. * inset));
    let g = div().relative().size(px(10.)).flex_none();
    let rest = p.rest_mark;
    match st {
        State::NeedsYou => g.child(at(0.).border_1().border_color(p.warn_mark_ch)).child(at(3.).bg(p.warn_mark_ch)),
        State::Running => g.child(at(1.).bg(p.run_mark_ch)),
        // Two small squares: work going on, not this session's own turn.
        State::Workers => g
            .child(div().absolute().top(px(1.)).left(px(1.)).size(px(4.)).bg(p.run_mark_ch))
            .child(div().absolute().top(px(5.)).left(px(5.)).size(px(4.)).bg(p.run_mark_ch)),
        State::Unread => g.child(at(3.).bg(p.unread_mark_ch)),
        State::Idle => g.child(at(1.).border_1().border_color(rest)),
        State::Parked => g.child(div().absolute().left(px(1.)).bottom(px(1.)).w(px(8.)).h(px(2.)).bg(rest)),
        State::Opening => g.child(at(1.).border_t_1().border_l_1().border_color(p.focus_ch)),
        State::Locked => {
            g.child(at(1.).border_1().border_color(rest)).child(div().absolute().left(px(1.)).top(px(4.)).w(px(8.)).h(px(2.)).bg(rest))
        }
        State::Saved => g,
    }
}

pub(crate) fn word_color(st: State, p: &Palette) -> Hsla {
    match st {
        State::NeedsYou => p.warn_ch,
        State::Running | State::Workers => p.run_ch,
        State::Unread => p.unread_ch,
        State::Opening => p.open_ch,
        _ => p.dim_ch,
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
    let hover = p.dim_ch;
    div()
        .id(id)
        .size(px(20.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(p.edge)
        .hover(move |d| d.border_color(hover))
        .child(mono(glyph, 12., p.dim_ch))
        .tooltip(move |window, cx| {
            let t = Tooltip::new(tip);
            if rail { t.action(&ToggleRail, None) } else { t.action(&ToggleRight, None) }.build(window, cx)
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(rail, cx)))
}

impl AppView {
    /// The connection disc's colour (the top bar and the folded rail).
    fn conn_disc(&self, p: &Palette) -> Hsla {
        match &self.model.conn {
            ConnState::Connected(_) => p.ready_mark_ch,
            _ if self.synthetic => p.ready_mark_ch,
            ConnState::Connecting | ConnState::Reconnecting => p.rest_mark,
            _ => p.err_mark_ch,
        }
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
        let (conn, conn_color): (SharedString, Hsla) = match &self.model.conn {
            _ if self.model.reattaching => ("reattaching".into(), p.dim_ch),
            ConnState::Connected(v) => (format!("connected · {v}").into(), p.dim_ch),
            ConnState::Connecting => ("connecting".into(), p.dim_ch),
            ConnState::Reconnecting => ("reconnecting".into(), p.dim_ch),
            ConnState::Off(e) | ConnState::Incompatible(e) => {
                (crate::app::one_line(e, 60).into(), if self.synthetic { p.dim_ch } else { p.err_ch })
            }
        };
        // As in the mock: subagents running, here the attached session's,
        // as its blocks and its rail row say (they change together).
        let subagents = self.model.active_workers();
        let what = what.filter(|w| !w.is_empty());
        let input = crate::fit::TopIn {
            width: self.width,
            brand: match rail {
                Side::Open(w) => w - 22.,
                _ => 254.,
            },
            need: n,
            label: first.map_or(0, |s| s.label().chars().count()),
            gist: what.as_ref().map_or(0, |w| w.chars().count()),
            more: if n > 1 { (n - 1).ilog10() as usize + 1 } else { 0 },
            jump: jump_to.is_some(),
            conn: conn.chars().count(),
            badge: if subagents > 9 { subagents.ilog10() as usize + 1 } else { 1 },
        };
        let fit = self.top_words.map(|w| crate::fit::fit_top(&w, &input)).unwrap_or(crate::fit::TopFit {
            brand: input.brand,
            word: true,
            word2: true,
            label: 0.,
            gist: 0.,
            conn: 0.,
            theme: true,
            systems: true,
        });
        // Fitted: every part flex_none at its width, nothing overlaps.
        let cut = |d: Div, w: f32| d.flex_none().max_w(px(w)).overflow_hidden().text_ellipsis();
        let on = n > 0;
        let ticker = h_flex()
            .id("attention")
            .flex_none()
            .gap(px(12.))
            .items_center()
            .child(div().size(px(6.)).flex_none().when(on, |d| d.bg(p.warn_mark_ch)).when(!on, |d| d.border_1().border_color(p.rest_mark)))
            .child(display(format!("{n}   {}", label::NEED_YOU), 11., if on { p.warn_ch } else { p.dim_ch }).flex_none())
            .when_some(first.filter(|_| fit.label > 0.), |d, s| d.child(cut(mono(s.label(), 11., p.ink), fit.label)))
            .when_some(what.filter(|_| fit.gist > 0.), |d, w| d.child(cut(mono(w, 11., p.dim_ch), fit.gist)))
            .when(n > 1, |d| d.child(mono(format!("+{}", n - 1), 11., p.dim_ch).flex_none()))
            .when_some(jump_to, |d, id| {
                d.cursor_pointer()
                    .child(
                        h_flex()
                            .flex_none()
                            .ml(px(8.))
                            .gap(px(6.))
                            .child(display(label::JUMP, 10.5, p.prose_ch))
                            .child(mono("→", 11., p.prose_ch)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for fx in this.model.open_row(&RowAction::Attach(id.clone())) {
                            this.effect(fx);
                        }
                        cx.notify();
                    }))
            });
        h_flex()
            .h(px(top_h(self.layout.short)))
            .flex_none()
            .pl(px(22.))
            .pr(px(24.))
            .items_center()
            .border_b_1()
            .border_color(p.hair)
            // Over the open rail: as wide as it (while the bar has room).
            .child(
                h_flex()
                    .w(px(fit.brand))
                    .flex_none()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().size(px(8.)).flex_none().rounded_full().bg(self.conn_disc(p)))
                            .when(fit.word, |d| d.child(display(label::WORD, 13., p.header))),
                    )
                    .when(fit.word2, |d| d.child(display(label::WORD_2, 13., p.dim_ch))),
            )
            .child(div().flex_1().min_w(px(16.)))
            .child(ticker)
            .child(div().flex_1().min_w(px(16.)))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(18.))
                    .items_center()
                    .when(fit.conn > 0., |d| d.child(cut(mono(conn, 10.5, conn_color), fit.conn)))
                    // The theme picker's own control: a swatch of the "you" colour.
                    .child(
                        h_flex()
                            .id("accent-picker")
                            .gap(px(8.))
                            .items_center()
                            .cursor_pointer()
                            .child(div().size(px(10.)).bg(p.you_bar))
                            .when(fit.theme, |d| d.child(display(label::THEME, 10.5, if self.picker_open { p.ink } else { p.dim_ch })))
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
                        h_flex().gap(px(10.)).items_center().when(fit.systems, |d| d.child(display(label::SYSTEMS, 11., p.dim_ch))).child(
                            div()
                                .min_w(px(18.))
                                .h(px(18.))
                                .px(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if subagents > 0 { p.ink } else { p.edge })
                                .child(mono(count(subagents), 10., if subagents > 0 { p.chrome } else { p.dim_ch })),
                        ),
                    ),
            )
    }

    /// The drag handle on a side panel's inner edge (`rail`, else the right
    /// panel's), over the rest of the window: its 1-px line is the panel's
    /// border, `line3` under the pointer, `ink3` while dragged. A strip has
    /// one too (dragged out, it opens), unless the window cannot open it.
    pub(crate) fn render_edge(&self, rail: bool, side: Side, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        if side == Side::Strip && crate::app::toggled(self.width, self.panels, rail).is_none() {
            return None;
        }
        let dragged = self.drag.is_some_and(|g| g.moved && g.rail == rail);
        // Mostly over the panel: the border is the rail's last pixel and
        // the right panel's first.
        let (left, line) = if rail { (MARK + side.width() - 4., 3.) } else { (self.width - side.width() - 2., 2.) };
        let (id, group) = if rail { ("rail-edge", "rail-edge") } else { ("right-edge", "right-edge") };
        Some(
            div()
                .id(id)
                .group(group)
                .absolute()
                .top(px(top_h(self.layout.short)))
                .bottom_0()
                .left(px(left))
                .w(px(EDGE))
                .cursor(CursorStyle::ResizeLeftRight)
                // The wheel still scrolls what is under it.
                .block_mouse_except_scroll()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(line))
                        .w(px(1.))
                        .when(dragged, |d| d.bg(p.dim_ch))
                        .when(!dragged, |d| d.group_hover(group, |s| s.bg(p.edge))),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.grab_edge(rail, ev.position.x.into());
                        cx.stop_propagation();
                    }),
                )
                .on_drag(EdgeDrag, |d, _, _, cx| cx.new(|_| *d))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| this.click_edge(rail, ev.click_count(), cx))),
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
            .border_color(p.hair)
            .cursor_pointer()
            .hover(|d| d.bg(p.raised))
            .child(mono("›", 12., p.dim_ch))
            .child(div().size(px(8.)).rounded_full().bg(self.conn_disc(p)))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(6.)).when(on, |d| d.bg(p.warn_mark_ch)).when(!on, |d| d.border_1().border_color(p.rest_mark)))
                    .child(display(n.to_string(), 13., if on { p.warn_ch } else { p.dim_ch })),
            )
            .child(div().flex_1())
            .child(self.render_strip_foot(p, cx))
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
        let bar = gauge_color(ratio, p);
        let filled = ratio.map(|r| ((r * STRIP_SEGMENTS as f64).round() as usize).min(STRIP_SEGMENTS)).unwrap_or(0);
        let mut gauge = v_flex().w(px(6.)).gap(px(2.));
        for i in (0..STRIP_SEGMENTS).rev() {
            gauge = gauge.child(div().w_full().h(px(4.)).bg(if i < filled { bar } else { p.edge }));
        }
        let pct: SharedString = ratio.map(|r| format!("{:.0}", r * 100.0)).unwrap_or_else(|| "—".into()).into();
        let fits = crate::app::toggled(self.width, self.panels, false).is_some();
        // L2: detached, the last numbers seen are dimmed, as the open panel's
        // are; so are a cached tab's while its attach is on its way.
        let detached = self.model.stats.detached && self.model.active.is_none();
        let dim = detached || self.opening();
        v_flex()
            .id("right-strip")
            .w(px(STRIP))
            .h_full()
            .flex_none()
            .items_center()
            .pt(px(22.))
            .gap(px(14.))
            .border_l_1()
            .border_color(p.hair)
            .cursor_pointer()
            .hover(|d| d.bg(p.raised))
            .child(mono("‹", 12., p.dim_ch))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(14.))
                    .when(dim, |d| d.opacity(0.45))
                    .child(v_flex().items_center().child(display(pct, 13., p.prose_ch)).child(mono("%", 9., p.dim_ch)))
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
            n.error.iter().map(|e| (e, p.err_ch)).chain(n.notice.iter().chain(n.saved_note.iter()).map(|t| (t, p.dim_ch))).collect();
        // The chips count what is held (the attached session as it runs,
        // ahead of the poll) and the unread history rows.
        let (mut needs, mut running, mut unread) = (0, 0, self.rail.rail.history_unread);
        for s in self.model.sessions.iter().filter(|s| s.lifecycle != "ending") {
            match self.model.fact(s) {
                State::NeedsYou => needs += 1,
                State::Running | State::Workers => running += 1,
                State::Unread => unread += 1,
                _ => {}
            }
        }
        // A chip toggles its filter; none selected shows everything.
        let mut chips = h_flex().items_center().gap(px(15.)).px(px(22.)).pt(px(13.)).pb(px(12.)).border_b_1().border_color(p.hair);
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
                    .border_color(if on { p.focus_ch } else { gpui_kit::transparent_black() })
                    .cursor_pointer()
                    .child(display(c.label(), 10., if on { p.ink } else { p.dim_ch }))
                    .child(mono(
                        count.to_string(),
                        10.,
                        match c {
                            _ if count == 0 => p.dim_ch,
                            Chip::NeedsYou => p.warn_ch,
                            Chip::Running => p.run_ch,
                            Chip::Unread => p.unread_ch,
                        },
                    ))
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
            .border_color(p.edge)
            .child(
                div().flex_1().min_w_0().font_family(fonts::MONO).text_size(px(10.5)).child(Input::new(&self.find_input).appearance(false)),
            )
            .child(div().px(px(5.)).border_1().border_color(p.edge).child(mono("/", 9.5, p.dim_ch)));
        let items = list(self.rail_list.clone(), cx.processor(|this, ix, _window, cx| this.render_rail_item(ix, cx))).size_full();
        v_flex()
            .w(px(width))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(p.hair)
            .child(
                h_flex()
                    .px(px(22.))
                    .pt(px(24.))
                    .items_baseline()
                    .child(section(label::SESSIONS, fonts::JP_SESSIONS, p))
                    .child(div().flex_1())
                    .when(self.client.is_some(), |d| {
                        d.child(tbtn("new", label::NEW, p.dim_ch, p.ink).on_click(cx.listener(|this, _, _, cx| {
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
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(items)
                    .child(crate::scrollbar::rows(&self.rail_list, 0., &self.bars.rail).ground(crate::scrollbar::Ground::Chrome)),
            )
            .child(self.render_foot(p, cx))
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
                .when(ix > 0, |d| d.border_t_1().border_color(p.hair))
                .child(display(group.label(), 10., p.dim_ch))
                .child(div().font_family(fonts::JP).text_size(px(9.5)).text_color(p.dim_ch).child(group.jp()))
                .child(div().flex_1())
                .child(mono(count, 10., p.dim_ch))
                .into_any_element(),
            Item::Row(r) => self.render_row2(*r, &p, cx),
            Item::Hist(r) => self.render_hist(*r, &p, cx),
            Item::Bucket { bucket, open, count } => {
                let b = *bucket;
                let color = if *open { p.prose_ch } else { p.dim_ch };
                h_flex()
                    .id(("bucket", b as usize))
                    .w_full()
                    .px(px(22.))
                    .pt(px(7.))
                    .pb(px(6.))
                    .gap(px(10.))
                    .items_baseline()
                    .cursor_pointer()
                    .hover(|d| d.bg(p.raised))
                    .child(div().w(px(10.)).flex_none().child(mono(if *open { "−" } else { "+" }, 11., color)))
                    .child(display(b.label(), 9.5, color))
                    .child(div().flex_1())
                    .child(mono(count, 10., p.dim_ch))
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
                    .child(display(text, 9.5, p.dim_ch))
                    .hover(|d| d.bg(p.raised))
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
                .text_color(p.dim_ch)
                .child(SharedString::from(t))
                .into_any_element(),
            Item::Empty(t) => div().px(px(22.)).py(px(10.)).text_size(px(12.)).text_color(p.dim_ch).child(*t).into_any_element(),
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
        // The marker follows the visible tab (also while its attach is on
        // its way); a session open in a background tab has a faint one.
        let shown = meta.is_some_and(|s| self.model.is_shown(&s.id));
        let in_tab = !shown && meta.is_some_and(|s| self.model.tabs.holds(&s.id));
        // The attached session's cost as the panel shows it (its digests
        // run ahead of the 3 s poll), else the poll's.
        let cost = meta.map(|s| if on { self.model.stats.cost.unwrap_or(s.cost_usd) } else { s.cost_usd }).filter(|c| *c > 0.0);
        let waiting = meta.map(|s| if on { self.model.transcript.pending_prompts().len() } else { s.awaiting_input }).unwrap_or(0);
        let workers = if st == State::Workers { meta.map_or(0, |s| self.model.workers(s)) } else { 0 };
        let title_color = match st {
            State::NeedsYou | State::Running | State::Workers | State::Unread => p.ink,
            _ if shown => p.ink,
            State::Parked | State::Locked => p.dim_ch,
            _ if row.fresh => p.dim_ch,
            _ => p.prose_ch,
        };
        let sub_color = p.dim_ch;
        let bar = if st == State::NeedsYou {
            Some(p.warn_mark_ch)
        } else if shown {
            Some(p.you_bar)
        } else if in_tab {
            Some(p.rest_mark)
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
                .when(st == State::NeedsYou && waiting > 1, |d| d.child(mono(count(waiting), 10., p.warn_ch)))
                .when(st == State::Workers, |d| d.child(mono(count(workers), 10., p.run_ch)))
        });
        div()
            .id(("rail-row", key as usize))
            .relative()
            .w_full()
            .px(px(22.))
            .pt(px(8.))
            .pb(px(9.))
            .cursor_pointer()
            .when(shown, |d| d.bg(p.raised))
            .hover(|d| d.bg(p.raised))
            .when_some(bar, |d, c| d.child(div().absolute().left_0().top_0().bottom_0().w(px(if in_tab { 1. } else { 2. })).bg(c)))
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
                                .child(mono(&row.age, 10., p.dim_ch)),
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
                                .when_some(cost, |d, c| d.child(mono(format!("${c:.2}"), 10., p.cost_ch).flex_none())),
                        ),
                ),
            )
            .when(self.readout == Some(key), |d| d.child(self.render_readout(row, p)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_row(key, *hovered, cx)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.rail_click(key, crate::tabstrip::click_place(ev, cx));
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.is_middle_click() {
                    this.rail_click(key, Place::New);
                    cx.notify();
                }
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
            State::Opening => p.prose_ch,
            _ if row.named => p.prose_ch,
            _ => p.dim_ch,
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
            .hover(|d| d.bg(p.raised))
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
            .child(mono(&row.age, 10., p.dim_ch))
            .when(self.readout == Some(key), |d| d.child(self.render_readout(row, p)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_row(key, *hovered, cx)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.rail_click(key, crate::tabstrip::click_place(ev, cx));
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.is_middle_click() {
                    this.rail_click(key, Place::New);
                    cx.notify();
                }
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
            .bg(p.raised)
            .border_1()
            .border_color(p.edge)
            .child(div().text_size(px(12.5)).text_color(p.ink).pb(px(5.)).child(r.title));
        for (k, value, aside) in r.lines {
            body = body.child(
                h_flex().items_start().child(div().w(px(62.)).flex_none().pt(px(2.)).child(display(k, 9., p.dim_ch))).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .font_family(fonts::MONO)
                        .text_size(px(10.5))
                        .child(div().text_color(p.prose_ch).child(value))
                        .when(!aside.is_empty(), |d| d.child(div().text_color(p.dim_ch).child(aside))),
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
        let hot = ratio.is_some_and(|r| r >= HOT);
        let bar = gauge_color(ratio, p);
        let mut segs = h_flex().gap(px(2.)).h(px(8.)).mt(px(20.));
        for i in 0..SEGMENTS {
            segs = segs.child(div().flex_1().h_full().bg(if i < filled { bar } else { p.edge }));
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
            .border_color(p.hair)
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
                            .child(display(format!("{:.1}", r * 100.0), 46., if hot { p.err_ch } else { p.ink }).line_height(px(46.)))
                            .child(display("%", 16., p.dim_ch).line_height(px(16.)).pb(px(5.)))
                            .into_any_element(),
                        None => display("—", 46., p.dim_ch).into_any_element(),
                    })
                    .child(div().flex_1())
                    .when_some(window, |d, w| {
                        d.child(v_flex().items_end().gap(px(5.)).child(display(label::WINDOW, 9.5, p.dim_ch)).child(mono(
                            compact(w),
                            12.5,
                            p.prose_ch,
                        )))
                    }),
            )
            .child(segs)
            .child(mono(scale, 9.5, if hot { p.err_ch } else { p.dim_ch }).mt(px(9.)));
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
            .border_color(p.hair)
            .child(section(label::TOKENS, fonts::JP_TOKENS, p))
            .child(
                h_flex()
                    .mt(px(20.))
                    .child(div().flex_1().child(kv(label::IN, num(|t| t.input), false, p.ink, p)))
                    .child(div().flex_1().child(kv(label::OUT, num(|t| t.output), false, p.ink, p)))
                    .child(div().flex_1().child(kv(label::CACHE, num(|t| t.cache_read), false, p.ink, p)))
                    .child(div().flex_1().child(kv(label::WRITE, num(|t| t.cache_creation), false, p.ink, p))),
            )
            .child(
                h_flex()
                    .mt(px(22.))
                    .child(div().flex_1().child(kv(label::CACHE_HIT, hit, true, p.ink, p)))
                    .child(div().flex_1().child(kv(label::COST, cost, true, p.cost_ch, p))),
            );
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
                    d.child(h_flex().gap(px(8.)).child(mono(format!("{approx}+{added}"), 10.5, p.ok_ch)).child(mono(
                        format!("−{removed}"),
                        10.5,
                        p.err_ch,
                    )))
                },
            ))
            // Over a worker's view: still the session's thread's, not the
            // worker's on screen.
            .child(
                match self.model.watch.focused() {
                    Some(w) => mono(w.files_note.clone(), 9.5, p.dim_ch),
                    None => mono("as the model asked: tool inputs, not the disk", 9.5, p.dim_ch),
                }
                .mt(px(6.)),
            )
            .child(div().h(px(12.)));
        for f in ledger.files().iter().take(FILES_SHOWN) {
            files = files.child(
                h_flex()
                    .h(px(27.))
                    .gap(px(10.))
                    .items_center()
                    .child(
                        mono(
                            f.kind.letter(),
                            10.5,
                            match f.kind {
                                crate::files::FileKind::Modified => p.edit_ch,
                                crate::files::FileKind::Written => p.write_ch,
                            },
                        )
                        .w(px(10.))
                        .flex_none(),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(mono(f.dir.clone(), 11., p.dim_ch).flex_shrink(1.).min_w(px(14.)).overflow_hidden().text_ellipsis())
                            .child(mono(f.name.clone(), 11., p.ink).flex_none()),
                    )
                    // A write is the lines written, not a delta (`~`).
                    .when(f.written > 0, |d| d.child(mono(format!("~{}", f.written), 10.5, p.dim_ch).flex_none()))
                    .when(f.added > 0 || f.written == 0, |d| {
                        d.child(mono(format!("{}+{}", tilde(f.approx), f.added), 10.5, p.ok_ch).flex_none())
                    })
                    .when(f.removed > 0, |d| d.child(mono(format!("−{}", f.removed), 10.5, p.err_ch).flex_none())),
            );
        }
        let n = ledger.files().len();
        if n > FILES_SHOWN {
            files = files.child(mono(format!("+{} more files", n - FILES_SHOWN), 10.5, p.dim_ch).mt(px(6.)));
        }
        let shell = ledger.shell();
        if n == 0 && shell.is_empty() {
            files = files.child(div().text_size(px(12.)).text_color(p.dim_ch).child("No file edits in this session"));
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
                        .child(display(label::VIA_SHELL, 10., p.prose_ch))
                        .child(mono("~", 11., p.dim_ch)),
                )
                .child(mono("bash commands that succeeded; no counts", 9.5, p.dim_ch).mt(px(6.)))
                .child(div().h(px(8.)));
            for f in shell.iter().take(FILES_SHOWN) {
                files = files.child(
                    h_flex()
                        .h(px(25.))
                        .gap(px(10.))
                        .items_center()
                        .child(mono("$", 10.5, p.dim_ch).w(px(10.)).flex_none())
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(mono(f.dir.clone(), 11., p.dim_ch).flex_shrink(1.).min_w(px(14.)).overflow_hidden().text_ellipsis())
                                .child(mono(f.name.clone(), 11., p.prose_ch).flex_none()),
                        )
                        .child(mono(f.verb, 10., p.dim_ch).flex_none()),
                );
            }
            if shell.len() > FILES_SHOWN {
                files = files.child(mono(format!("+{} more", shell.len() - FILES_SHOWN), 10.5, p.dim_ch).mt(px(6.)));
            }
        }
        // Detached: the last numbers seen, dimmed and labelled, not live. A
        // cached tab whose attach is on its way: the same, as last seen.
        let detached = s.detached && self.model.active.is_none();
        let cached = !detached && self.opening();
        v_flex()
            .id("right")
            .w(px(width))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .track_scroll(&self.bars.right.scroll)
            .border_l_1()
            .border_color(p.hair)
            .when(detached || cached, |d| {
                d.child(
                    h_flex()
                        .px(px(24.))
                        .py(px(10.))
                        .gap(px(10.))
                        .border_b_1()
                        .border_color(p.hair)
                        .child(display(
                            if cached { "C A C H E D" } else { "D E T A C H E D" },
                            10.,
                            if cached { p.dim_ch } else { p.warn_ch },
                        ))
                        .child(mono(if cached { "as last seen · opening…" } else { "last numbers seen" }, 10., p.dim_ch)),
                )
            })
            .child(v_flex().when(detached || cached, |d| d.opacity(0.45)).child(context).child(tokens).child(files))
    }

    /// The theme picker: SYNAPS CONFIG (follow the TUI's `theme =`, the
    /// default), the 20 built-in themes (each with the three colours that
    /// tell them apart: canvas, `prompt_fg`, `claude_label`), then the
    /// accent override (THEME: none; the presets; a custom hex). Over the
    /// open right-hand column when it is wide enough, else above the
    /// composer at the right: never over the header's NAME and HALT. It
    /// scrolls when the window is too short for it. A click applies and
    /// saves; Esc (the root's key handler) or a click outside closes it.
    pub(crate) fn render_picker(&self, p: &Palette, right: Side, cx: &Context<Self>) -> impl IntoElement {
        let scheme = cx.global::<theme::Scheme>();
        let current = scheme.choice.theme.clone();
        let accent = scheme.choice.accent;
        let follows = self.theme_follows_config;
        let swatch = |c: u32| div().size(px(10.)).flex_none().bg(rgb(c));
        let config = h_flex()
            .id("theme-config")
            .mt(px(12.))
            .h(px(26.))
            .px(px(6.))
            .gap(px(8.))
            .items_center()
            .cursor_pointer()
            .when(follows, |d| d.bg(p.raised))
            .hover(|d| d.bg(p.raised))
            .child(div().size(px(12.)).flex_none().border_1().border_color(p.dim_ch))
            .child(display(label::SYNAPS_CONFIG, 10.5, if follows { p.ink } else { p.prose_ch }).flex_none())
            .child(mono(format!("· {}", self.config_theme), 11., p.dim_ch).flex_1().min_w_0().overflow_hidden().text_ellipsis())
            .child(mono(if follows { "✓" } else { " " }, 11., p.focus_ch).w(px(10.)))
            .on_click(cx.listener(|this, _, _, cx| this.follow_config(cx)));
        let mut grid = h_flex().mt(px(2.)).flex_wrap().gap_y(px(2.));
        for (i, &(name, raw)) in crate::palettes::THEMES.iter().enumerate() {
            let on = !follows && name == current;
            grid = grid.child(
                h_flex()
                    .id(("theme", i))
                    .w(relative(0.5))
                    .h(px(26.))
                    .px(px(6.))
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .when(on, |d| d.bg(p.raised))
                    .hover(|d| d.bg(p.raised))
                    .child(
                        h_flex()
                            .flex_none()
                            .border_1()
                            .border_color(p.edge)
                            .child(swatch(raw.message_bg))
                            .child(swatch(raw.prompt_fg))
                            .child(swatch(raw.claude_label)),
                    )
                    .child(mono(name, 11., if on { p.ink } else { p.prose_ch }).flex_1().min_w_0().overflow_hidden().text_ellipsis())
                    .child(mono(if on { "✓" } else { " " }, 11., p.focus_ch).w(px(10.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_theme(name, cx))),
            );
        }
        let mut presets = h_flex().mt(px(10.)).flex_wrap().gap_y(px(2.));
        let off = accent.is_none();
        presets = presets.child(
            h_flex()
                .id("accent-off")
                .w(relative(0.5))
                .h(px(26.))
                .px(px(6.))
                .gap(px(8.))
                .items_center()
                .cursor_pointer()
                .when(off, |d| d.bg(p.raised))
                .hover(|d| d.bg(p.raised))
                .child(div().size(px(12.)).flex_none().border_1().border_color(p.dim_ch))
                .child(display(label::THEME, 10.5, if off { p.ink } else { p.prose_ch }).flex_1())
                .child(mono(if off { "✓" } else { " " }, 11., p.focus_ch).w(px(10.)))
                .on_click(cx.listener(|this, _, _, cx| this.set_accent(None, cx))),
        );
        for (i, &(name, hex)) in theme::PRESETS.iter().enumerate() {
            let on = accent == Some(hex);
            presets = presets.child(
                h_flex()
                    .id(("preset", i))
                    .w(relative(0.5))
                    .h(px(26.))
                    .px(px(6.))
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .when(on, |d| d.bg(p.raised))
                    .hover(|d| d.bg(p.raised))
                    .child(div().size(px(12.)).flex_none().bg(rgb(hex)))
                    .child(display(name, 10.5, if on { p.ink } else { p.prose_ch }).flex_1().min_w_0().overflow_hidden())
                    .child(mono(if on { "✓" } else { " " }, 11., p.focus_ch).w(px(10.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_accent(Some(hex), cx))),
            );
        }
        let lifted = accent.is_some_and(|a| a != scheme.roles.you_bar || a != scheme.roles.focus);
        let (top, bottom) = (top_h(self.layout.short), 140.);
        // Over the open right panel, at least two columns of names wide,
        // when that stays clear of the header's NAME and HALT (they end at
        // the column's right edge); else above the composer, under the
        // header.
        let place = match right {
            Side::Open(w) if w.max(PICKER_MIN) - w + 8. <= self.layout.right => PickerAt::Top(w.max(PICKER_MIN)),
            _ => PickerAt::Bottom(right.width() + 24.),
        };
        let max_h = match place {
            PickerAt::Top(_) => self.height - top - 16.,
            PickerAt::Bottom(_) => self.height - bottom - top - if self.layout.short { HEADER_CLEAR_SHORT } else { HEADER_CLEAR },
        };
        let body = v_flex()
            .flex_none()
            .p(px(18.))
            .child(
                h_flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(display(label::THEME, 11.5, p.header))
                    .child(mono(current.clone(), 10.5, p.dim_ch))
                    .child(div().flex_1())
                    .child(tbtn("picker-close", label::CLOSE, p.dim_ch, p.ink).on_click(cx.listener(|this, _, window, cx| {
                        this.close_picker(window, cx);
                    }))),
            )
            .child(config)
            .child(grid)
            .child(h_flex().mt(px(18.)).items_baseline().gap(px(12.)).child(display(label::ACCENT, 11.5, p.header)).child(mono(
                accent.map(theme::to_hex).unwrap_or_else(|| "the theme's".into()),
                10.5,
                p.dim_ch,
            )))
            .child(mono("replaces the theme's “you” and focus colours; never a state", 10., p.dim_ch).mt(px(6.)).whitespace_normal())
            .child(presets)
            .child(
                h_flex()
                    .mt(px(12.))
                    .gap(px(12.))
                    .items_center()
                    .child(display(label::CUSTOM, 10., p.dim_ch).w(px(62.)))
                    .child(div().flex_1().child(Input::new(&self.hex_input)))
                    .child(
                        tbtn("picker-apply", label::APPLY, p.prose_ch, p.ink)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_custom_accent(cx))),
                    ),
            )
            .when(lifted, |el| {
                el.child(mono("lifted to stay readable: 3:1 for bars, 4.5:1 for text", 10., p.dim_ch).mt(px(10.)).whitespace_normal())
            })
            .when_some(self.picker_note.clone(), |el, n| el.child(mono(n, 10., p.dim_ch).mt(px(10.)).whitespace_normal()));
        v_flex()
            .id("picker")
            .absolute()
            .map(|d| match place {
                PickerAt::Top(w) => d.top(px(top)).right_0().w(px(w)),
                PickerAt::Bottom(r) => d.bottom(px(bottom)).right(px(r)).w(px(PICKER_MIN)),
            })
            .max_h(px(max_h.max(160.)))
            .overflow_y_scroll()
            .track_scroll(&self.bars.picker.scroll)
            .bg(p.chrome)
            .border_1()
            .border_color(p.edge)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                this.picker_dismissed = Some(ev.position);
                this.close_picker(window, cx);
            }))
            .child(body)
    }
}

/// Where the picker opens: over the open right panel (its width), or above
/// the composer this far from the window's right edge.
#[derive(Clone, Copy)]
enum PickerAt {
    Top(f32),
    Bottom(f32),
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
