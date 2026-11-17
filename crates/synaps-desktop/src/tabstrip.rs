//! The tab strip over the centre pane: one tab per open session, its state
//! as the rail draws it (glyph, word, the needs-you bar), the visible one
//! raised with the "you" bar (its word is the composer foot's: it keeps
//! the glyph and gives the title the room). A worker's tab ([`Kind::Worker`]) says
//! `sleeper · <its session>` with the worker's state mark, its session in
//! its tooltip; right-click opens the worker's menu ([`crate::wtabs`]).
//! Nothing moves on its own: no animation, a
//! drag drops where it is let go, and tabs that do not fit scroll by the
//! arrows at the ends (never a second row).

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
        if self.tab_labels.key != key || self.tab_labels.labels.len() != self.model.tabs.len() {
            let labels = self.model.tabs.iter().map(|t| self.tab_label(t)).collect();
            self.tab_labels = Labels { key, labels, arrows: std::mem::take(&mut self.tab_labels.arrows) };
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
        let follow = visible != self.tab_follow || fit != self.tab_room;
        let lay = strip(n, active, room, self.tab_first, follow);
        self.tab_first = lay.first;
        self.tab_follow = visible;
        self.tab_room = fit;
        let mut row = h_flex().h_full().flex_none();
        for (ix, t) in tabs.iter().enumerate().skip(lay.first).take(lay.shown) {
            let st = self.tab_state(t);
            let (label, named) = self.tab_labels.labels.get(ix).cloned().unwrap_or_default();
            // A worker of a session not attached here: as last seen (as
            // nothing, if this window holds nothing of its session).
            let worker = match (&t.worker, t.session.as_deref()) {
                (Some(w), Some(s)) if t.kind == Kind::Worker => self.worker_state(s, &w.handle, Some(w.who())),
                _ => None,
            };
            row = row.child(self.render_tab(t.id, ix, active == Some(ix), t.kind, (st, worker), label, named, lay.width, p, cx));
        }
        let (before, after) = (lay.first, n - lay.first - lay.shown);
        if lay.overflow && self.tab_labels.arrows.0 != (before, after) {
            self.tab_labels.arrows = ((before, after), (format!("‹ {before}").into(), format!("{after} ›").into()));
        }
        let (left, right) = self.tab_labels.arrows.1.clone();
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
            .h(px(TAB_H))
            .flex_none()
            .w_full()
            .overflow_hidden()
            .bg(p.chrome)
            .border_b_1()
            .border_color(p.hair)
            .when(lay.overflow, |d| {
                d.child(arrow("tabs-left", left, before > 0).border_r_1().border_color(p.hair).on_click(cx.listener(|this, _, _, cx| {
                    this.tab_first = this.tab_first.saturating_sub(1);
                    cx.notify();
                })))
            })
            .child(row)
            .when(lay.overflow, |d| {
                d.child(arrow("tabs-right", right, after > 0).on_click(cx.listener(|this, _, _, cx| {
                    this.tab_first += 1;
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
        on: bool,
        kind: Kind,
        (st, worker): (State, Option<Liveness>),
        label: SharedString,
        named: bool,
        width: f32,
        p: &Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = !kind.attaches();
        let title_color = match st {
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
        // NEEDS YOU first: a worker tab's session's prompt too.
        let needs = st == State::NeedsYou;
        let bar = if needs {
            Some(p.warn_mark_ch)
        } else if on {
            Some(p.you_bar)
        } else {
            None
        };
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
            .when(on, |d| d.bg(p.raised))
            .when(!on, |d| d.hover(|d| d.bg(p.raised)))
            .when_some(bar, |d, c| d.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(c)))
            .child(match kind {
                Kind::Settings => crate::views::gear(if on { p.you_bar } else { p.dim_ch }, None),
                Kind::Usage => crate::views::bars_glyph(if on { p.you_bar } else { p.dim_ch }),
                Kind::Session | Kind::Worker => div()
                    .flex()
                    .flex_none()
                    .size(px(10.))
                    .debug_selector(|| format!("tab {id} glyph"))
                    .children(g.map(|g| glyph::draw(g, mark_color))),
            })
            .child(
                div()
                    .debug_selector(|| format!("tab {id} title"))
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
            .when_some(word, |d, w| d.child(display(w, 12., word_color).flex_none()))
            .child(close)
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
