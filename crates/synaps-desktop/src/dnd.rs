//! Split panes step 4 (split/architecture.md §4, split.md §2–§3, §10):
//! drag and drop. A tab or a rail row dragged over a pane's body (under
//! its tab strip) drops into the zone under the pointer: one of its four
//! edge bands (a new pane there) or its centre (open here). The zone that
//! a limit would refuse becomes centre, and a drop that would do nothing
//! shows nothing: the preview is always what the drop does.

use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, DragMoveEvent, Hsla, SharedString, Window, div, px};

use crate::app::AppView;
use crate::fonts::TextPx as _;
use crate::named::RowAction;
use crate::pane::{KeysTo, PaneId};
use crate::split::{Rect, Side};
use crate::tabstrip::TabDrag;

/// The share of a pane body's side that is that edge's band (split.md §3).
pub(crate) const BAND: f32 = 0.25;
/// The preview's inset inside the pane (split.md §3).
pub(crate) const INSET: f32 = 4.;
/// Under this width the preview has no label (split.md §3): its spaced
/// caps and their backing need it.
pub(crate) const LABEL_MIN: f32 = 160.;
/// Under this width the label says no refusal's reason.
pub(crate) const NOTE_MIN: f32 = 380.;
/// The label's height (12 px caps, 3 px padding, its line) and its gap
/// to the preview's edge (finish gate #48 F1).
const LABEL_H: f32 = 24.;
const LABEL_GAP: f32 = 14.;
/// The drag chip's reach above and below the pointer: the label is never
/// put there.
const CHIP_REACH: f32 = 40.;

/// Where in a pane's body a drop lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

impl Zone {
    /// The side a new pane goes, for an edge.
    pub(crate) const fn side(self) -> Option<Side> {
        match self {
            Zone::Center => None,
            Zone::Left => Some(Side::Left),
            Zone::Right => Some(Side::Right),
            Zone::Top => Some(Side::Up),
            Zone::Bottom => Some(Side::Down),
        }
    }
}

/// The zone of pane body `r` under the pointer at `(x, y)` (split.md §3,
/// VS Code's): the 25 % band along each side is that edge's, the rest is
/// the centre; in a corner (two bands), the nearer edge.
pub(crate) fn zone(r: Rect, x: f32, y: f32) -> Zone {
    let (l, rt, t, b) = (x - r.x, r.x + r.w - x, y - r.y, r.y + r.h - y);
    let (bw, bh) = (r.w * BAND, r.h * BAND);
    [(l, bw, Zone::Left), (rt, bw, Zone::Right), (t, bh, Zone::Top), (b, bh, Zone::Bottom)]
        .into_iter()
        .filter(|(d, band, _)| *d < *band)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Zone::Center, |(_, _, z)| z)
}

/// The preview's rectangle for `zone` of pane body `r`: the half the new
/// pane takes, or all of it for the centre, inset [`INSET`] px.
pub(crate) fn preview(r: Rect, zone: Zone) -> Rect {
    let (hw, hh) = ((r.w / 2.).round(), (r.h / 2.).round());
    let half = match zone {
        Zone::Center => r,
        Zone::Left => Rect { w: hw, ..r },
        Zone::Right => Rect { x: r.x + r.w - hw, w: hw, ..r },
        Zone::Top => Rect { h: hh, ..r },
        Zone::Bottom => Rect { y: r.y + r.h - hh, h: hh, ..r },
    };
    Rect { x: half.x + INSET, y: half.y + INSET, w: (half.w - 2. * INSET).max(0.), h: (half.h - 2. * INSET).max(0.) }
}

/// What our drag in flight carries (set by its drag's constructor: GPUI's
/// active drag can't be read from the app).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Carry {
    /// A tab of pane `from`: its session (none: a view's or a worker's
    /// tab, which only reorders in its own strip).
    Tab { tab: u64, from: PaneId, session: Option<String> },
    /// A rail row: it opens, never moves anything (split.md §2).
    Row { action: RowAction },
}

/// The window's drag and drop: the drag in flight, the drop under the
/// pointer, and the preview as drawn.
#[derive(Default)]
pub(crate) struct Dnd {
    pub(crate) on: Option<Carry>,
    /// The pane and zone a drop now would land in, and that pane body's
    /// rectangle in the centre (only drops that would work).
    pub(crate) hint: Option<Hint>,
    /// Ctrl (Alt) is held: a session tab's drop copies (split.md §2).
    pub(crate) copy: bool,
    /// The hint the preview was drawn for last frame (a new one glides
    /// from where it was), and the rectangle drawn, where it fades out.
    pub(crate) drawn: Option<((PaneId, Zone), Rect)>,
    /// Why the last drop was the centre (its edge refused), for the
    /// focused composer's status line after the drop's frame.
    pub(crate) note: Option<&'static str>,
    /// The pointer's y in the centre, as of the last move (the label
    /// keeps off it).
    pub(crate) pointer_y: f32,
    /// The preview's strength: from, to, since ([`Dnd::strength`]).
    pub(crate) fade: (f32, f32, Option<std::time::Instant>),
}

impl Dnd {
    /// The preview's strength at `now`, going to `to` over
    /// [`crate::motion::QUICK`] (Off: at once): in on `ease::OUT`, out on
    /// `ease::IN`, motion.md §4.2's exit (finish gate #48 F2: an `OUT` exit
    /// is spent in its first two frames, and a drop's fade-out was not
    /// seen). Running, it asks the next frame.
    pub(crate) fn strength(&mut self, to: f32, now: std::time::Instant, fx: &crate::motion::Motion) -> f32 {
        use crate::motion::{Level, QUICK, ease};
        let (from, end, at) = self.fade;
        let sample = |at: Option<std::time::Instant>| {
            let Some(at) = at else { return end };
            let k = (now.saturating_duration_since(at).as_secs_f32() / QUICK.as_secs_f32()).min(1.);
            let curve = if end > from { ease::OUT } else { ease::IN };
            from + (end - from) * curve.sample(k)
        };
        let a = sample(at);
        if fx.level() == Level::Off {
            self.fade = (to, to, None);
            return to;
        }
        if to != end {
            self.fade = (a, to, Some(now));
            return a;
        }
        if a != end {
            fx.want_frame();
        } else {
            self.fade.2 = None;
        }
        a
    }

    /// The drag ended (dropped, cancelled, or released outside): no hint;
    /// the preview fades where it was.
    pub(crate) fn end(&mut self) {
        self.on = None;
        self.hint = None;
        self.copy = false;
    }
}

/// GPUI's drag value for a rail row, and the chip it draws at the pointer
/// (the tab's: [`crate::tabstrip::TabDrag`]).
#[derive(Clone)]
pub(crate) struct RailDrag {
    pub action: RowAction,
    pub label: SharedString,
    pub bg: Hsla,
    pub fg: Hsla,
    pub edge: Hsla,
}

impl RailDrag {
    /// Its drag's constructor: ours (the panes' drop targets come up for
    /// it), and its chip. It updates the view: pointer input must never be
    /// dispatched from inside an `AppView` update (a double lease, as the
    /// film's was before `3fe2291`: Shady #48 n6).
    pub(crate) fn start(
        me: gpui_kit::WeakEntity<AppView>,
    ) -> impl Fn(&RailDrag, gpui_kit::Point<gpui_kit::Pixels>, &mut Window, &mut gpui_kit::App) -> gpui_kit::Entity<RailDrag> + 'static
    {
        move |d, _, _, cx| {
            let carry = Carry::Row { action: d.action.clone() };
            let _ = me.update(cx, |v, cx| v.drag_started(carry, cx));
            cx.new(|_| d.clone())
        }
    }
}

impl AppView {
    /// A rail row's drag value.
    pub(crate) fn rail_drag(&self, row: &crate::rail::Row, p: &crate::theme::Palette) -> RailDrag {
        RailDrag { action: row.action.clone(), label: SharedString::from(row.title.to_string()), bg: p.raised, fg: p.ink, edge: p.edge }
    }
}

impl Render for RailDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(crate::tabstrip::TAB_H - 6.))
            .max_w(px(crate::tabstrip::TAB_MAX))
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

/// The preview's motion keys (`0x7AF8`: the Q5 family, app.rs): its
/// rectangle's four sides on `snap` (its strength: [`Dnd::fade`]).
const PREVIEW_R: u64 = 0x7AF8 << 48;

/// The drop a pane would take now: the zone it lands in (none: the drop
/// would do nothing, so nothing is shown), the pane body's rectangle in
/// the centre, and why the edge asked for became the centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hint {
    pub(crate) pane: PaneId,
    /// The zone under the pointer, before [`AppView::drop_zone`]: a change
    /// of Ctrl (Alt) asks that again (Shady #48 L2).
    pub(crate) raw: Zone,
    pub(crate) zone: Option<Zone>,
    pub(crate) rect: Rect,
    pub(crate) note: Option<&'static str>,
}

impl AppView {
    /// Each open pane's body (its rectangle under its tab strip) in the
    /// centre: one, the whole centre, when not shown split (one pane, or
    /// folded: split.md §5). The landed tree's: during a split's or a
    /// close's flight (about 300 ms) a drag previews against where the
    /// panes land, not where they are drawn (Shady #48 n5). On the
    /// window's pixels, as the panes are drawn
    /// ([`crate::split::Node::rects_in`]; Shady #62 R3-3).
    pub(crate) fn drop_bodies(&self) -> Vec<(PaneId, Rect)> {
        let (w, h) = self.centre.get();
        let all = Rect { x: 0., y: 0., w, h };
        let rects =
            if self.panes.shown_split() { self.panes.tree.rects_in(all, self.centre_at.get()) } else { vec![(self.panes.focused, all)] };
        let strip = crate::tabstrip::TAB_H;
        rects.into_iter().map(|(id, r)| (id, Rect { y: r.y + strip, h: (r.h - strip).max(0.), ..r })).collect()
    }

    /// What a drop of `carry` (`copy`: Ctrl or Alt held) on `zone` of pane
    /// `pane` does (split.md §2–§3): an edge a limit would refuse (a 10th
    /// pane, a minimum; folded) is the centre,
    /// with the refusal's note; a pane's own last tab to its edge, or a
    /// tab to its own pane's centre, is nothing. A view's or a worker's tab
    /// only reorders in its own strip.
    pub(crate) fn drop_zone(&self, pane: PaneId, zone: Zone, carry: &Carry, copy: bool) -> (Option<Zone>, Option<&'static str>) {
        if !self.split_on() {
            return (None, None);
        }
        let (session, own, tabs) = match carry {
            Carry::Tab { from, session, .. } => {
                let tabs = self
                    .panes
                    .get(*from)
                    .map_or(0, |p| if p.id == self.panes.cur.id { self.model.tabs.len() } else { p.parked.tabs.len() });
                (session.is_some(), *from == pane, tabs)
            }
            Carry::Row { .. } => (true, false, 0),
        };
        if !session {
            return (None, None);
        }
        let (mut zone, mut note) = (zone, None);
        if let Some(side) = zone.side() {
            let refused = if self.panes.folded {
                Some(None)
            } else if own && tabs <= 1 && !copy {
                // Its pane would close as the new one opens: nothing.
                return (None, None);
            } else {
                match self.panes.free_id() {
                    None => Some(Some(crate::split::Refused::Most.note())),
                    Some(new) => self.panes.tree.split(pane, side, new, self.centre.get()).err().map(|r| Some(r.note())),
                }
            };
            if let Some(why) = refused {
                (zone, note) = (Zone::Center, why);
            }
        }
        if zone == Zone::Center && own {
            return (None, note);
        }
        (Some(zone), note)
    }

    /// A drag of ours moved over pane `pane`'s body (`bounds`, the
    /// window's px): the zone under the pointer, the drop it would be.
    /// Outside that body, a hint of this pane's goes. The button let go
    /// elsewhere (a release outside the window: GPUI never ends the drag),
    /// it ends here, and nothing drops.
    #[allow(clippy::too_many_arguments)]
    fn drag_over(
        &mut self,
        pane: PaneId,
        carry: Carry,
        bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
        ev: &gpui_kit::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.pressed_button != Some(gpui_kit::MouseButton::Left) {
            cx.stop_active_drag(window);
            self.dnd.end();
            cx.notify();
            return;
        }
        if self.dnd.on.is_none() {
            self.dnd.on = Some(carry.clone());
        }
        let copy = ev.modifiers.control || ev.modifiers.alt;
        let (x0, y0) = self.centre_at.get();
        let r = Rect {
            x: f32::from(bounds.origin.x) - x0,
            y: f32::from(bounds.origin.y) - y0,
            w: f32::from(bounds.size.width),
            h: f32::from(bounds.size.height),
        };
        let (x, y) = (f32::from(ev.position.x) - x0, f32::from(ev.position.y) - y0);
        let inside = x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if inside {
            self.dnd.pointer_y = y;
        }
        let hint = if inside {
            let raw = zone(r, x, y);
            let (zone, note) = self.drop_zone(pane, raw, &carry, copy);
            Some(Hint { pane, raw, zone, rect: r, note })
        } else if self.dnd.hint.is_some_and(|h| h.pane == pane) {
            None
        } else {
            return;
        };
        if hint != self.dnd.hint || copy != self.dnd.copy {
            self.dnd.hint = hint;
            self.dnd.copy = copy;
            cx.notify();
        }
    }

    /// Esc while a drag of ours is up (split.md §3): it ends, nothing
    /// drops, and the focused pane's composer has the keys again (the
    /// press on the tab took them: Shady #48 n3).
    pub(crate) fn cancel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.dnd.on.is_none() {
            return false;
        }
        cx.stop_active_drag(window);
        self.dnd.end();
        self.focus_composer(cx);
        cx.notify();
        true
    }

    /// Ctrl (Alt) pressed or let go mid-drag: the drop under the pointer
    /// asked again (a copy may split where a move does nothing), so the
    /// preview stays what the drop does (Shady #48 L2).
    pub(crate) fn drag_modifiers(&mut self, m: gpui_kit::Modifiers, cx: &mut Context<Self>) {
        let (Some(carry), Some(h)) = (self.dnd.on.clone(), self.dnd.hint) else { return };
        let copy = m.control || m.alt;
        let (zone, note) = self.drop_zone(h.pane, h.raw, &carry, copy);
        let hint = Some(Hint { zone, note, ..h });
        if hint != self.dnd.hint || copy != self.dnd.copy {
            self.dnd.hint = hint;
            self.dnd.copy = copy;
            cx.notify();
        }
    }

    /// The button let go with a drag of ours up (the root's mouse-up, in
    /// the capture phase: before any target's, and with no hover test).
    /// The drop is asked again where it was let go, with the modifiers
    /// held then (Shady #48 L2–L4):
    /// - a key typed mid-drag leaves GPUI hovering nothing until the
    ///   pointer moves (`Window::last_input_was_keyboard`), so a target's
    ///   own `on_drop` never hears a release that did not move;
    /// - a key mid-drag (F6, Ctrl+\, Ctrl+W) may have changed the panes
    ///   since the last move: the hint's pane may be gone.
    ///
    /// Over a pane's tab strip, the strip's own drop (before a tab) takes
    /// it when GPUI hovers; when it does not, the drop goes to that pane's
    /// centre. Anywhere else, nothing drops.
    pub(crate) fn release_drag(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(carry) = self.dnd.on.clone() else { return };
        let (x0, y0) = self.centre_at.get();
        let (x, y) = (f32::from(at.x) - x0, f32::from(at.y) - y0);
        let inside = |r: &Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        let strip = crate::tabstrip::TAB_H;
        let bodies = self.drop_bodies();
        let at = if let Some(&(pane, r)) = bodies.iter().find(|(_, r)| inside(r)) {
            (pane, zone(r, x, y))
        } else if let Some(&(pane, _)) = bodies.iter().find(|(_, r)| inside(&Rect { y: r.y - strip, h: strip, ..*r }))
            && window.last_input_was_keyboard()
        {
            (pane, Zone::Center)
        } else {
            return;
        };
        let copy = window.modifiers().control || window.modifiers().alt;
        let (zone, note) = self.drop_zone(at.0, at.1, &carry, copy);
        self.dnd.end();
        match zone {
            Some(zone) => self.drop_into(at.0, zone, carry, copy, window, cx),
            // A drop that does nothing: the keys back to the composer.
            None => self.focus_composer(cx),
        }
        // Said once the drop's frame has given the composer its tab (a new
        // tab's composer drops the old one's note).
        self.dnd.note = note;
        cx.notify();
    }

    /// `carry` dropped on `zone` of pane `pane` (split.md §2): a tab moves
    /// (`copy`: a copy stays behind) into that pane, or a new one on that
    /// edge; a rail row opens there, never moving anything. The drop's
    /// pane is focused, with the keys.
    pub(crate) fn drop_into(&mut self, pane: PaneId, zone: Zone, carry: Carry, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        use crate::tabs::Place;
        // A pane gone since (Shady #48 L4): nothing drops, nothing moves.
        if self.panes.get(pane).is_none() {
            return;
        }
        match (carry, zone.side()) {
            (Carry::Row { action }, side) => {
                let place = match side {
                    None => {
                        self.focus_pane(pane, KeysTo::Pane, cx);
                        Place::New
                    }
                    Some(side) => {
                        if self.split_at(pane, Some(side), None, window, cx).is_none() {
                            return;
                        }
                        // Its rows wait for the split to land (F2).
                        self.panes.cur.hold_rows = self.panes.flight.is_some();
                        Place::Current
                    }
                };
                // Here, whatever other pane shows it (a drag copies:
                // split.md §0), unlike a click.
                match action {
                    RowAction::Attach(s) => {
                        let fallback = self.model.journal_of(&s);
                        self.open_tab(crate::app::Open::Attach { session_id: s, fallback, open: false }, place);
                    }
                    RowAction::Resume(req) => self.open_tab(crate::app::Open::Resume { req, open: false }, place),
                }
                self.focus_composer(cx);
            }
            (Carry::Tab { tab, from, session: Some(_) }, None) => self.move_tab_to(tab, from, pane, None, copy, cx),
            (Carry::Tab { tab, from, session: Some(s) }, Some(side)) => {
                let Some(label) = self.dragged_label(from, tab) else { return };
                if self.split_at(pane, Some(side), Some((s.clone(), label)), window, cx).is_none() {
                    return;
                }
                if !copy {
                    // Its draft with it (Shady #44 M2), out of its pane.
                    let draft = self.in_pane(from, |v| v.take_draft(tab, &s, cx)).flatten();
                    if let Some((k, d)) = draft {
                        self.give_draft(&k, d);
                    }
                    self.take_tab(from, tab);
                }
                self.focus_composer(cx);
            }
            (Carry::Tab { session: None, .. }, _) => {}
        }
    }

    /// Tab `tab`'s label in pane `from`.
    fn dragged_label(&mut self, from: PaneId, tab: u64) -> Option<String> {
        self.in_pane(from, |v| v.model.tabs.iter().find(|t| t.id == tab).map(|t| t.label.clone())).flatten()
    }

    /// Tab `tab` out of pane `from` with no reopen entry (it is on screen
    /// in another pane: Shady #44 M3); with its last tab the pane closes.
    fn take_tab(&mut self, from: PaneId, tab: u64) {
        self.in_pane(from, |v| {
            let ix = v.model.tabs.iter().position(|t| t.id == tab);
            if let Some(ix) = ix
                && v.model.tabs.take(ix).is_some()
            {
                v.model.rail_gen += 1;
                v.reconcile(None);
            }
        });
    }

    /// Session tab `tab` of pane `from` into pane `to` (split.md §2: a
    /// drop on its centre or its tab strip), before tab `before` there if
    /// given, else after its visible one; focused there. As Ctrl+Alt+→
    /// moves one (3b-2's path): its draft comes with it, and its store
    /// (what it sent unconfirmed, its worker views) is shared with the
    /// pane it goes to before it leaves the pane it was in. `copy`: it
    /// stays there too.
    pub(crate) fn move_tab_to(&mut self, tab: u64, from: PaneId, to: PaneId, before: Option<u64>, copy: bool, cx: &mut Context<Self>) {
        use crate::tabs::{Kind, Place};
        // Into a pane that is gone, nothing moves (Shady #48 L4: the
        // tab would be opened, and taken, in its own pane).
        if from == to || self.panes.get(to).is_none() {
            return;
        }
        let Some((session, label)) = self
            .in_pane(from, |v| {
                v.model
                    .tabs
                    .iter()
                    .find(|t| t.id == tab && t.kind == Kind::Session)
                    .and_then(|t| Some((t.session.clone()?, t.label.clone())))
            })
            .flatten()
        else {
            return;
        };
        self.focus_pane(to, KeysTo::Pane, cx);
        if self.panes.cur.id != to {
            return;
        }
        let draft = if copy { None } else { self.in_pane(from, |v| v.take_draft(tab, &session, cx)).flatten() };
        self.model.tabs.open(&session, &label, Place::New);
        if let Some(b) = before.and_then(|b| self.model.tabs.iter().position(|t| t.id == b))
            && let Some(ix) = self.model.tabs.find(&session)
        {
            let to = if ix < b { b - 1 } else { b };
            self.model.tabs.move_tab(ix, to);
        }
        if let Some((k, d)) = draft {
            self.give_draft(&k, d);
        }
        self.model.rail_gen += 1;
        self.reconcile(None);
        if !copy {
            self.take_tab(from, tab);
        }
        self.focus_composer(cx);
        cx.notify();
    }

    /// The pane whose strip has tab `onto`.
    fn pane_of_tab(&self, onto: u64) -> Option<PaneId> {
        let has = |p: &crate::pane::Pane| {
            if p.id == self.panes.cur.id {
                self.model.tabs.iter().any(|t| t.id == onto)
            } else {
                p.parked.tabs.iter().any(|t| t.id == onto)
            }
        };
        self.panes.iter().find(|p| has(p)).map(|p| p.id)
    }

    /// A tab dropped on tab `onto` (its strip's mark: split.md §2): in its
    /// own strip, today's move; from another pane's, a move (or a copy)
    /// into this one, before it.
    pub(crate) fn drop_on_tab(&mut self, d: &crate::tabstrip::TabDrag, onto: u64, window: &mut Window, cx: &mut Context<Self>) {
        let copy = window.modifiers().control || window.modifiers().alt;
        let to = self.pane_of_tab(onto);
        self.dnd.end();
        match to {
            Some(to) if to != d.from && self.split_on() && d.session.is_some() => self.move_tab_to(d.id, d.from, to, Some(onto), copy, cx),
            Some(to) => {
                self.in_pane(to, |v| v.move_tab(d.id, onto));
            }
            None => {}
        }
        cx.notify();
    }

    /// A rail row dropped on a tab: it opens in that tab's pane (the
    /// centre's drop).
    pub(crate) fn drop_row_on_tab(&mut self, d: &RailDrag, onto: u64, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.pane_of_tab(onto);
        self.dnd.end();
        if let Some(to) = to.filter(|_| self.split_on()) {
            self.drop_into(to, Zone::Center, Carry::Row { action: d.action.clone() }, false, window, cx);
        }
        cx.notify();
    }

    /// A drag's start (its constructor): what it carries, the readout
    /// hover off (split/architecture.md §4.4).
    pub(crate) fn drag_started(&mut self, carry: Carry, cx: &mut Context<Self>) {
        // Split off, a tab's drag only reorders its strip (Shady #48 n1).
        if !self.split_on() {
            return;
        }
        if matches!(carry, Carry::Row { .. })
            && let Some(k) = self.hover
        {
            self.hover_row(k, false, cx);
        }
        self.dnd.on = Some(carry);
        self.dnd.hint = None;
        cx.notify();
    }

    /// The drop layer, over the centre (in the window's px): while a drag
    /// of ours is up, one target over each pane's body; the preview of the
    /// drop under the pointer, gliding between zones on `snap` and fading
    /// in and out over [`crate::motion::QUICK`] (split.md §10).
    pub(crate) fn render_drop_layer(&mut self, p: &crate::theme::Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // A drag that ended anywhere (a drop outside every target, the
        // window left) leaves nothing.
        if self.dnd.on.is_some() && !cx.has_active_drag() {
            self.dnd.end();
        }
        let (x0, y0) = self.centre_at.get();
        let mut out = Vec::new();
        if self.dnd.on.is_some() {
            for (id, r) in self.drop_bodies() {
                out.push(
                    div()
                        .id(("drop-target", id.index()))
                        .debug_selector(move || format!("drop-target {}", id.index()))
                        .absolute()
                        .left(px(x0 + r.x))
                        .top(px(y0 + r.y))
                        .w(px(r.w))
                        .h(px(r.h))
                        .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<TabDrag>, window, cx| {
                            let d = ev.drag(cx);
                            let carry = Carry::Tab { tab: d.id, from: d.from, session: d.session.clone() };
                            this.drag_over(id, carry, ev.bounds, &ev.event, window, cx);
                        }))
                        .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<RailDrag>, window, cx| {
                            let carry = Carry::Row { action: ev.drag(cx).action.clone() };
                            this.drag_over(id, carry, ev.bounds, &ev.event, window, cx);
                        }))
                        .into_any_element(),
                );
            }
            // The drop itself: the window's mouse-up, in the capture phase
            // and with no hover test ([`AppView::release_drag`]).
            let me = cx.weak_entity();
            out.push(
                gpui_kit::canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        window.on_mouse_event(move |ev: &gpui_kit::MouseUpEvent, phase, window, cx| {
                            if phase == gpui_kit::DispatchPhase::Capture && ev.button == gpui_kit::MouseButton::Left {
                                let _ = me.update(cx, |v, cx| v.release_drag(ev.position, window, cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_0()
                .into_any_element(),
            );
        }
        out.extend(self.render_preview(p, x0, y0));
        out
    }

    /// The drop preview (split.md §3): inset 4 px, the accent at 16 % with
    /// a 1 px line at 70 %, square, NEW PANE or OPEN HERE when it is over
    /// 120 px wide. A new zone glides from where it was drawn on `snap`
    /// (Full; Reduced and Off: there at once); it fades in and out over
    /// 120 ms (Reduced: at most that; Off: at once). At rest it asks no
    /// frame.
    fn render_preview(&mut self, p: &crate::theme::Palette, x0: f32, y0: f32) -> Option<AnyElement> {
        use crate::motion::SNAP_RESPONSE;
        if self.dnd.on.is_none() && self.dnd.drawn.is_none() {
            return None;
        }
        let shown = self.dnd.hint.and_then(|h| h.zone.map(|z| ((h.pane, z), preview(h.rect, z))));
        let a = self.dnd.strength(if shown.is_some() { 1. } else { 0. }, self.fx.now(), &self.fx);
        let (key, target) = shown.or(self.dnd.drawn)?;
        if shown.is_none() && a <= 0. {
            self.dnd.drawn = None;
            return None;
        }
        let moved = self.dnd.drawn.is_some_and(|(k, _)| k != key);
        let ends = [target.x, target.y, target.w, target.h];
        let mut v = ends;
        for (i, end) in ends.into_iter().enumerate() {
            let k = PREVIEW_R | i as u64;
            if moved && let Some(was) = self.fx.spring_at(k) {
                self.fx.spring_jump(k, was, end, SNAP_RESPONSE);
            }
            v[i] = self.fx.spring_px(k, end, SNAP_RESPONSE).0;
        }
        self.dnd.drawn = Some((key, target));
        let label = if key.1 == Zone::Center { "O P E N   H E R E" } else { "N E W   P A N E" };
        // Why an edge asked for is the centre, on the preview as it shows
        // (finish gate #48 S1): the composer says it again after the drop.
        let note = self.dnd.hint.and_then(|h| h.note).filter(|_| shown.is_some() && v[2] > NOTE_MIN);
        // A see-through quad over the panes (split.md §3: the accent at
        // 16 %, its line at 70 %), at `a` of that as it fades.
        let see = |k: f32| Hsla { a: p.acc.a * k * a, ..p.acc };
        let faded = |c: Hsla| Hsla { a: c.a * a, ..c };
        // Its label on its own square backing (finish gate #48 F1: legible
        // over code and prose), near the top edge, or near the bottom when
        // the pointer (and the chip with it) is there.
        let top = LABEL_GAP;
        let py = self.dnd.pointer_y - v[1];
        let clear = |y: f32| py < y - CHIP_REACH || py > y + LABEL_H + CHIP_REACH;
        let ly = if clear(top) { top } else { (v[3] - LABEL_GAP - LABEL_H).max(top) };
        let chip = h_flex()
            .h(px(LABEL_H))
            .px(px(8.))
            .gap(px(8.))
            .items_center()
            .bg(faded(p.canvas))
            .border_1()
            .border_color(see(0.70))
            .child(crate::app::display(label, crate::fonts::size::D_S, faded(p.you)))
            .when_some(note, |d, n| {
                d.child(crate::app::display("·", crate::fonts::size::D_S, faded(p.dim))).child(
                    crate::app::display(n.to_uppercase(), crate::fonts::size::D_S, faded(p.dim))
                        .debug_selector(|| "drop-label-note".into()),
                )
            });
        Some(
            div()
                .debug_selector(|| "drop-preview".into())
                .absolute()
                .left(px(x0 + v[0]))
                .top(px(y0 + v[1]))
                .w(px(v[2]))
                .h(px(v[3]))
                .overflow_hidden()
                .bg(see(0.16))
                .border_1()
                .border_color(see(0.70))
                .when(v[2] > LABEL_MIN, |d| {
                    d.child(
                        h_flex()
                            .debug_selector(|| "drop-label".into())
                            .absolute()
                            .left_0()
                            .w_full()
                            .top(px(ly))
                            .justify_center()
                            .child(chip),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect { x: 100., y: 50., w: 400., h: 200. };

    /// split.md §3: the 25 % bands, the centre, the corners to the nearer
    /// edge, the band's own edge.
    #[test]
    fn zone_table() {
        let at = |fx: f32, fy: f32| zone(R, R.x + fx * R.w, R.y + fy * R.h);
        let rows = [
            ((0.5, 0.5), Zone::Center),
            ((0.26, 0.5), Zone::Center),
            ((0.74, 0.74), Zone::Center),
            ((0.24, 0.5), Zone::Left),
            ((0.01, 0.5), Zone::Left),
            ((0.76, 0.5), Zone::Right),
            ((0.99, 0.5), Zone::Right),
            ((0.5, 0.24), Zone::Top),
            ((0.5, 0.01), Zone::Top),
            ((0.5, 0.76), Zone::Bottom),
            ((0.5, 0.99), Zone::Bottom),
            // Corners: px to the edge decide (the band is 100 px across,
            // 50 px down).
            ((0.05, 0.2), Zone::Left),    // 20 px from the left, 40 from the top
            ((0.2, 0.05), Zone::Top),     // 80 px from the left, 10 from the top
            ((0.95, 0.9), Zone::Right),   // 20 px from the right, 20 from the bottom: the first
            ((0.9, 0.98), Zone::Bottom),  // 40 px from the right, 4 from the bottom
            ((0.02, 0.98), Zone::Bottom), // 8 px from the left, 4 from the bottom
        ];
        for ((fx, fy), want) in rows {
            assert_eq!(at(fx, fy), want, "at ({fx}, {fy})");
        }
        // Exactly on a band's edge: the centre (the band is under 25 %).
        assert_eq!(zone(R, R.x + 100., R.y + 100.), Zone::Center);
        assert_eq!(zone(R, R.x + 99.9, R.y + 100.), Zone::Left);
    }

    #[test]
    fn previews_are_the_half_the_new_pane_takes_inset() {
        assert_eq!(preview(R, Zone::Center), Rect { x: 104., y: 54., w: 392., h: 192. });
        assert_eq!(preview(R, Zone::Left), Rect { x: 104., y: 54., w: 192., h: 192. });
        assert_eq!(preview(R, Zone::Right), Rect { x: 304., y: 54., w: 192., h: 192. });
        assert_eq!(preview(R, Zone::Top), Rect { x: 104., y: 54., w: 392., h: 92. });
        assert_eq!(preview(R, Zone::Bottom), Rect { x: 104., y: 154., w: 392., h: 92. });
        assert_eq!(Zone::Top.side(), Some(Side::Up));
        assert_eq!(Zone::Center.side(), None);
    }
}
