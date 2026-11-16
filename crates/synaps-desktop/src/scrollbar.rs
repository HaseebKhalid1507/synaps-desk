//! Overlay scrollbars for every pane that scrolls: drawn over the pane's
//! edge (never in the layout, so nothing moves and the transcript keeps its
//! column), thin at rest and wide under the pointer or while dragged, in
//! theme roles only (the accent while dragged).
//!
//! A bar shows while the pointer is over its pane and for [`LINGER`] after
//! a scroll, then it is gone: no fade, no motion. The transcript's shows
//! whenever its rows are taller than the pane (the position cue matters on
//! the main reading surface). A pane whose content fits draws nothing and
//! listens to nothing; a hidden bar draws nothing and wakes nothing: one
//! deadline timer per pane after a scroll, never a poll.
//!
//! Drag the thumb; click the track for a page toward the click; Shift+click
//! to jump there (and keep dragging). The bar is placed from its pane's own
//! viewport (the handle's bounds this frame), so it is added as a sibling
//! after the scrolled element and its own box is empty.
//!
//! The transcript's bar reads the list's own scroll model
//! (`scroll_px_offset_for_scrollbar`, `max_offset_for_scrollbar`) and
//! drags through GPUI's own entry (`set_offset_from_scrollbar`), so the
//! thumb, the drag and tail-following agree with the wheel. That model
//! counts a row it has not drawn at its hint: the app hints rows never
//! drawn from their text (`AppView::hint_rows`), and a long reply's first
//! layout, which is empty, is not taken for its height
//! ([`crate::turns::LateRows`]). While the thumb is dragged GPUI holds the
//! content height, so the thumb stays under the pointer as rows are
//! measured; let go, it stays where it was dropped ([`warp`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::ScrollbarHandle;
use gpui_kit::base::input::{EditorScrollbarContext, EditorScrollbarRenderer};
use gpui_kit::*;

/// The thumb's width at rest, and under the pointer or dragged.
pub(crate) const THIN: f32 = 6.;
pub(crate) const WIDE: f32 = 10.;
/// Between the bar and the pane's edge (clear of a panel's resize edge).
const EDGE: f32 = 4.;
/// The track's inset at each end.
const END: f32 = 2.;
/// The strip that takes the pointer while the bar shows: the wide bar, its
/// edge and this much inward. Narrower than the transcript's least gutter
/// (16 px), so it never covers the reading column.
const SLOP: f32 = 2.;
pub(crate) const MIN_THUMB: f32 = 24.;
/// How long a bar stays after a scroll.
pub(crate) const LINGER: Duration = Duration::from_secs(1);

/// What a bar sits on: its thumb's roles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ground {
    Canvas,
    Chrome,
    User,
}

/// A scroll's geometry along the bar's axis (px): the track's length, the
/// viewport's and the content's (padding included), and the thumb's
/// [`warp`] since a drop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geo {
    pub track: f32,
    pub view: f32,
    pub content: f32,
    pub warp: Option<(f32, f32)>,
}

/// The thumb along its track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Thumb {
    pub start: f32,
    pub len: f32,
}

impl Geo {
    /// The scroll range.
    pub(crate) fn max(self) -> f32 {
        (self.content - self.view).max(0.)
    }

    /// Something to scroll (a sub-pixel overhang is not) and room to draw.
    pub(crate) fn overflows(self) -> bool {
        self.max() >= 1. && self.view > 0. && self.track > 0.
    }

    /// The thumb's length: the viewport's share of the track, never under
    /// [`MIN_THUMB`] (nor over the track).
    pub(crate) fn len(self) -> f32 {
        (self.track * self.view / self.content.max(1.)).max(MIN_THUMB).min(self.track)
    }

    /// How far through its range `offset` is.
    pub(crate) fn frac(self, offset: f32) -> f32 {
        let max = self.max();
        if max > 0. { (offset / max).clamp(0., 1.) } else { 0. }
    }

    pub(crate) fn thumb(self, offset: f32) -> Thumb {
        let len = self.len();
        Thumb { start: warp(self.frac(offset), self.warp) * (self.track - len), len }
    }

    /// The thumb's start clamped to the track: where a drag holds it.
    pub(crate) fn start_at(self, start: f32) -> f32 {
        start.clamp(0., (self.track - self.len()).max(0.))
    }

    /// How far along its travel the thumb's start `start` is.
    pub(crate) fn frac_at(self, start: f32) -> f32 {
        let travel = self.track - self.len();
        if travel > 0. { (start / travel).clamp(0., 1.) } else { 0. }
    }

    /// The offset that puts the thumb's start at `start`: the drag's
    /// mapping. The thumb at the track's end is the scroll's end exactly.
    pub(crate) fn offset_at(self, start: f32) -> f32 {
        let travel = self.track - self.len();
        if travel <= 0. {
            return 0.;
        }
        unwarp((start / travel).clamp(0., 1.), self.warp) * self.max()
    }

    /// A page (the viewport) from `offset`, toward the end or the start.
    pub(crate) fn page(self, offset: f32, toward_end: bool) -> f32 {
        (if toward_end { offset + self.view } else { offset - self.view }).clamp(0., self.max())
    }

    /// The offset that centres the thumb on `at` (a Shift+click).
    pub(crate) fn jump(self, at: f32) -> f32 {
        self.offset_at(at - self.len() / 2.)
    }
}

/// The list's thumb after a drop. While dragged, GPUI holds the list's
/// height; let go, the rows measured meanwhile correct it, and the thumb
/// would jump to where the corrected height puts it. Instead the travel is
/// remapped, piecewise linear through `(live, shown)`: the drop point stays
/// where it was let go, the ends stay the ends, and the thumb moves on
/// continuously with the content. Exact again at either end.
pub(crate) fn warp(f: f32, anchor: Option<(f32, f32)>) -> f32 {
    match anchor {
        Some((a, b)) if f <= a => f / a * b,
        Some((a, b)) => b + (f - a) / (1. - a) * (1. - b),
        None => f,
    }
}

pub(crate) fn unwarp(d: f32, anchor: Option<(f32, f32)>) -> f32 {
    match anchor {
        Some((a, b)) if d <= b => d / b * a,
        Some((a, b)) => a + (d - b) / (1. - b) * (1. - a),
        None => d,
    }
}

/// The warp through `live` (where the content is) and `shown` (where the
/// thumb was let go); none at the ends or where they agree.
pub(crate) fn anchor(live: f32, shown: f32) -> Option<(f32, f32)> {
    const E: f32 = 1e-3;
    (live > E && live < 1. - E && (live - shown).abs() > 1e-4).then(|| (live, shown.clamp(E, 1. - E)))
}

/// Whether a bar shows: always (the transcript), under the pointer, while
/// dragged, or until its deadline after a scroll.
pub(crate) fn shown(always: bool, over: bool, dragging: bool, until: Option<Instant>, now: Instant) -> bool {
    always || over || dragging || until.is_some_and(|u| now < u)
}

/// A list's content height for the bar: GPUI's `max_offset_for_scrollbar`
/// leaves the list's padding out, while its scroll range (and the "dragged
/// to the end" test that re-arms tail-following) has it. Without it the
/// thumb's end stops `pad` px short of the transcript's end.
pub(crate) fn list_content(view: f32, max_offset: f32, pad: f32) -> f32 {
    if max_offset > 0. { view + max_offset + pad } else { view }
}

fn along(axis: Axis, p: Point<Pixels>) -> f32 {
    f32::from(if axis == Axis::Vertical { p.y } else { p.x })
}

fn length(axis: Axis, s: Size<Pixels>) -> f32 {
    f32::from(if axis == Axis::Vertical { s.height } else { s.width })
}

/// `at` along `axis`, the other coordinate kept from `keep`.
fn place(axis: Axis, at: f32, keep: Point<Pixels>) -> Point<Pixels> {
    if axis == Axis::Vertical { point(keep.x, px(at)) } else { point(px(at), keep.y) }
}

/// The track (its full length, `width` across) along the viewport's right
/// or bottom edge.
pub(crate) fn lane(axis: Axis, vp: Bounds<Pixels>, width: f32) -> Bounds<Pixels> {
    let w = px(width);
    match axis {
        Axis::Vertical => {
            Bounds::new(point(vp.right() - px(EDGE) - w, vp.top() + px(END)), size(w, (vp.size.height - px(2. * END)).max(px(0.))))
        }
        Axis::Horizontal => {
            Bounds::new(point(vp.left() + px(END), vp.bottom() - px(EDGE) - w), size((vp.size.width - px(2. * END)).max(px(0.)), w))
        }
    }
}

/// The thumb's rectangle in `lane`.
pub(crate) fn thumb_rect(axis: Axis, lane: Bounds<Pixels>, t: Thumb) -> Bounds<Pixels> {
    match axis {
        Axis::Vertical => Bounds::new(point(lane.left(), lane.top() + px(t.start)), size(lane.size.width, px(t.len))),
        Axis::Horizontal => Bounds::new(point(lane.left() + px(t.start), lane.top()), size(px(t.len), lane.size.height)),
    }
}

/// Where the pointer is taken while the bar shows.
pub(crate) fn strip(axis: Axis, vp: Bounds<Pixels>) -> Bounds<Pixels> {
    let across = px(EDGE + WIDE + SLOP);
    match axis {
        Axis::Vertical => Bounds::new(point(vp.right() - across, vp.top()), size(across, vp.size.height)),
        Axis::Horizontal => Bounds::new(point(vp.left(), vp.bottom() - across), size(vp.size.width, across)),
    }
}

/// What scrolls under a bar, in its own terms: offsets are how far it is
/// scrolled (≥ 0), sizes include the content's padding.
pub(crate) trait Scrolled: Clone + 'static {
    fn viewport(&self) -> Bounds<Pixels>;
    fn content(&self) -> Size<Pixels>;
    fn offset(&self) -> Point<Pixels>;
    fn scroll_to(&self, at: Point<Pixels>);
    fn drag(&self, _on: bool) {}
}

fn neg(p: Point<Pixels>) -> Point<Pixels> {
    point(-p.x, -p.y)
}

impl Scrolled for ScrollHandle {
    fn viewport(&self) -> Bounds<Pixels> {
        self.bounds()
    }
    fn content(&self) -> Size<Pixels> {
        ScrollbarHandle::content_size(self)
    }
    fn offset(&self) -> Point<Pixels> {
        neg(ScrollHandle::offset(self))
    }
    fn scroll_to(&self, at: Point<Pixels>) {
        self.set_offset(neg(at));
    }
}

/// The composer's editor ([`editor`]).
impl Scrolled for EditorScrollbarContext {
    fn viewport(&self) -> Bounds<Pixels> {
        self.viewport_bounds()
    }
    fn content(&self) -> Size<Pixels> {
        self.content_size()
    }
    fn offset(&self) -> Point<Pixels> {
        neg(ScrollbarHandle::offset(self))
    }
    fn scroll_to(&self, at: Point<Pixels>) {
        self.set_offset(neg(at));
    }
}

/// A `gpui::list`, with its vertical padding (top + bottom).
#[derive(Clone)]
pub(crate) struct Rows {
    list: ListState,
    pad: f32,
}

impl Scrolled for Rows {
    fn viewport(&self) -> Bounds<Pixels> {
        self.list.viewport_bounds()
    }
    fn content(&self) -> Size<Pixels> {
        let vp = self.list.viewport_bounds().size;
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        size(vp.width, px(list_content(f32::from(vp.height), max, self.pad)))
    }
    fn offset(&self) -> Point<Pixels> {
        neg(self.list.scroll_px_offset_for_scrollbar())
    }
    /// GPUI's own scrollbar entry: a drag away from the end stops
    /// tail-following, one to the end re-arms it (the wheel's rule).
    fn scroll_to(&self, at: Point<Pixels>) {
        self.list.set_offset_from_scrollbar(neg(at));
    }
    fn drag(&self, on: bool) {
        if on {
            self.list.scrollbar_drag_started();
        } else {
            self.list.scrollbar_drag_ended();
        }
    }
}

/// A bar's state across frames.
#[derive(Clone, Default)]
pub(crate) struct BarState(Rc<RefCell<Inner>>);

#[derive(Default)]
struct Inner {
    /// The pointer is over the pane, over the bar's strip.
    over: bool,
    on_bar: bool,
    /// Dragging: where along the thumb it was taken, and where along the
    /// track the pointer holds it (drawn there, not where the content got
    /// to: rows measured meanwhile move that).
    grab: Option<f32>,
    held: f32,
    /// The offset last drawn (`None`: never drawn).
    last: Option<f32>,
    /// Shown until then, after a scroll; its timer is armed.
    until: Option<Instant>,
    timer: bool,
    /// The thumb's [`warp`], and where it was let go (anchored next frame,
    /// on the height GPUI no longer holds).
    warp: Option<(f32, f32)>,
    dropped: Option<f32>,
    /// The bar moved the content since the last frame.
    ours: bool,
}

impl Inner {
    /// What is drawn: shown, and wide.
    fn look(&self, always: bool, now: Instant) -> (bool, bool) {
        let dragging = self.grab.is_some();
        let s = shown(always, self.over, dragging, self.until, now);
        (s, s && (self.on_bar || dragging))
    }
}

/// A scroll handle and its bar's state, for the panes whose scroll the app
/// did not keep a handle of.
#[derive(Default)]
pub(crate) struct Pane {
    pub scroll: ScrollHandle,
    pub bar: BarState,
}

impl Pane {
    pub(crate) fn bar(&self, ground: Ground) -> Bar<ScrollHandle> {
        Bar::new(self.scroll.clone(), &self.bar).ground(ground)
    }
}

/// Every pane's bar, kept by the app.
#[derive(Default)]
pub(crate) struct Bars {
    /// The transcript (one at a time: the session's, a worker's, a cached
    /// tab's list).
    pub transcript: BarState,
    pub rail: BarState,
    pub right: Pane,
    pub picker: Pane,
    pub settings: BarState,
    /// The session's system prompt in Settings.
    pub prompt: Pane,
    pub usage: BarState,
    pub card: BarState,
}

/// A bar over `scroll`.
pub(crate) struct Bar<S: Scrolled> {
    scroll: S,
    state: BarState,
    axis: Axis,
    ground: Ground,
    always: bool,
}

impl<S: Scrolled> Bar<S> {
    pub(crate) fn new(scroll: S, state: &BarState) -> Self {
        Bar { scroll, state: state.clone(), axis: Axis::Vertical, ground: Ground::Canvas, always: false }
    }

    pub(crate) fn ground(mut self, ground: Ground) -> Self {
        self.ground = ground;
        self
    }

    /// Shown whenever there is something to scroll.
    pub(crate) fn always(mut self) -> Self {
        self.always = true;
        self
    }
}

/// A bar over a `gpui::list` padded `pad` px (top and bottom together).
pub(crate) fn rows(list: &ListState, pad: f32, state: &BarState) -> Bar<Rows> {
    Bar::new(Rows { list: list.clone(), pad }, state)
}

/// The composer's bar, in place of the kit's (its editor calls this each
/// frame it lays out).
pub(crate) fn editor() -> EditorScrollbarRenderer {
    let state = BarState::default();
    Rc::new(move |ctx: EditorScrollbarContext, _: &mut Window, _: &mut App| Bar::new(ctx, &state).ground(Ground::User).into_any_element())
}

/// What a frame's prepaint hands its paint.
pub(crate) struct Frame<S: Scrolled> {
    bar: Bar<S>,
    geo: Geo,
    offset: Point<Pixels>,
    thumb: Thumb,
    lane: Bounds<Pixels>,
    vp: Bounds<Pixels>,
    pane: Hitbox,
    strip: Option<Hitbox>,
    thumb_hit: Option<Hitbox>,
    look: (bool, bool),
    view: EntityId,
}

impl<S: Scrolled> IntoElement for Bar<S> {
    type Element = Canvas<Option<Frame<S>>>;

    fn into_element(self) -> Self::Element {
        canvas(
            move |_, window, cx| self.prepaint(window, cx),
            |_, frame, window, cx| {
                if let Some(f) = frame {
                    f.paint(window, cx);
                }
            },
        )
        .absolute()
        .size_0()
    }
}

impl<S: Scrolled> Bar<S> {
    fn prepaint(self, window: &mut Window, cx: &mut App) -> Option<Frame<S>> {
        let mut st = self.state.0.borrow_mut();
        let vp = self.scroll.viewport();
        let axis = self.axis;
        let lane0 = lane(axis, vp, WIDE);
        let mut geo =
            Geo { track: length(axis, lane0.size), view: length(axis, vp.size), content: length(axis, self.scroll.content()), warp: None };
        let offset = self.scroll.offset();
        let at = along(axis, offset);
        if !geo.overflows() {
            // Nothing to scroll (any more): no hitbox, no listener.
            if st.grab.take().is_some() {
                self.scroll.drag(false);
            }
            (st.over, st.on_bar, st.last, st.warp, st.dropped) = (false, false, Some(at), None, None);
            return None;
        }
        // The warp: anchored where the thumb was let go; gone at the ends
        // and after a jump the bar did not make (another list, a key).
        if let Some(d) = st.dropped.take() {
            st.warp = anchor(geo.frac(at), d);
        }
        let jumped = !st.ours && st.last.is_some_and(|l| (l - at).abs() > 2. * geo.view);
        if at <= 0.5 || at >= geo.max() - 0.5 || jumped {
            st.warp = None;
        }
        st.ours = false;
        geo.warp = st.warp;
        let now = Instant::now();
        let view = window.current_view();
        if st.last.is_some_and(|l| (l - at).abs() > 0.5) && !self.always {
            st.until = Some(now + LINGER);
            if !st.timer {
                st.timer = true;
                linger(self.state.clone(), view, window, cx);
            }
        }
        st.last = Some(at);
        let look = st.look(self.always, now);
        // Under the pointer first, the pane over them (it blocks nothing):
        // the strip hides what is behind it from the pointer but not from
        // the wheel; the thumb takes the cursor.
        let mut thumb = geo.thumb(at);
        if st.grab.is_some() {
            thumb.start = st.held;
        }
        let (strip_hit, thumb_hit) = if look.0 {
            let s = window.insert_hitbox(strip(axis, vp), HitboxBehavior::BlockMouseExceptScroll);
            let t = window.insert_hitbox(thumb_rect(axis, lane0, thumb), HitboxBehavior::Normal);
            (Some(s), Some(t))
        } else {
            (None, None)
        };
        let pane = window.insert_hitbox(vp, HitboxBehavior::Normal);
        let drawn = lane(axis, vp, if look.1 { WIDE } else { THIN });
        drop(st);
        Some(Frame { bar: self, geo, offset, thumb, lane: drawn, vp, pane, strip: strip_hit, thumb_hit, look, view })
    }
}

impl<S: Scrolled> Frame<S> {
    fn paint(self, window: &mut Window, cx: &mut App) {
        let Frame { bar, geo, offset, thumb, lane, vp, pane, strip, thumb_hit, look, view } = self;
        let Bar { scroll, state, axis, ground, always } = bar;
        let dragging = state.0.borrow().grab.is_some();
        if look.0 {
            let p = &cx.global::<crate::theme::Scheme>().p;
            let (rest, hot) = match ground {
                Ground::Canvas => (p.dim, p.focus),
                Ground::Chrome => (p.dim_ch, p.focus_ch),
                Ground::User => (p.dim_u, p.focus),
            };
            let color = if dragging {
                hot
            } else if look.1 {
                rest
            } else {
                rest.opacity(0.55)
            };
            let track = look.1.then_some(p.hair);
            let rect = thumb_rect(axis, lane, thumb);
            window.with_content_mask(Some(ContentMask { bounds: vp }), |window| {
                if let Some(t) = track {
                    window.paint_quad(fill(lane, t));
                }
                window.paint_quad(fill(rect, color));
            });
            if dragging {
                window.set_window_cursor_style(CursorStyle::PointingHand);
            } else if let Some(t) = &thumb_hit {
                window.set_cursor_style(CursorStyle::PointingHand, t);
            }
        }
        let origin = along(axis, lane.origin);
        // The pointer: a drag follows it; otherwise only a change in what
        // is drawn (shown, wide) draws again.
        window.on_mouse_event({
            let (scroll, state, strip) = (scroll.clone(), state.clone(), strip.clone());
            move |e: &MouseMoveEvent, phase, window, cx| {
                let mut s = state.0.borrow_mut();
                if let Some(grab) = s.grab {
                    if !phase.capture() {
                        return;
                    }
                    cx.stop_propagation();
                    if e.pressed_button != Some(MouseButton::Left) {
                        // Let go outside the window.
                        s.grab = None;
                        s.dropped = Some(geo.frac_at(s.held));
                        drop(s);
                        scroll.drag(false);
                    } else {
                        s.held = geo.start_at(along(axis, e.position) - origin - grab);
                        s.ours = true;
                        let to = geo.offset_at(s.held);
                        drop(s);
                        scroll.scroll_to(place(axis, to, scroll.offset()));
                    }
                    cx.notify(view);
                    return;
                }
                if !phase.bubble() {
                    return;
                }
                let (over, on_bar) = (pane.is_hovered(window), strip.as_ref().is_some_and(|h| h.is_hovered(window)));
                if (over, on_bar) != (s.over, s.on_bar) {
                    let now = Instant::now();
                    let before = s.look(always, now);
                    (s.over, s.on_bar) = (over, on_bar);
                    if s.look(always, now) != before {
                        cx.notify(view);
                    }
                }
            }
        });
        if state.0.borrow().over {
            window.on_mouse_event({
                let state = state.clone();
                move |_: &MouseExitEvent, phase, _, cx| {
                    let mut s = state.0.borrow_mut();
                    if phase.bubble() && s.grab.is_none() {
                        let now = Instant::now();
                        let before = s.look(always, now);
                        (s.over, s.on_bar) = (false, false);
                        if s.look(always, now) != before {
                            cx.notify(view);
                        }
                    }
                }
            });
        }
        if dragging {
            window.on_mouse_event({
                let (scroll, state) = (scroll.clone(), state.clone());
                move |e: &MouseUpEvent, phase, _, cx| {
                    let mut s = state.0.borrow_mut();
                    if phase.capture()
                        && e.button == MouseButton::Left
                        && let Some(grab) = s.grab.take()
                    {
                        // Where the thumb is let go (the pointer may be ahead of the last move).
                        s.dropped = Some(geo.frac_at(geo.start_at(along(axis, e.position) - origin - grab)));
                        drop(s);
                        scroll.drag(false);
                        cx.notify(view);
                    }
                }
            });
        }
        // The track: the thumb to drag, a page toward a click, Shift+click
        // straight there (and on dragging from there).
        if let Some(strip) = strip.filter(|_| look.0) {
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if !phase.bubble() || e.button != MouseButton::Left || !strip.is_hovered(window) {
                    return;
                }
                cx.stop_propagation();
                window.prevent_default();
                let at = along(axis, e.position) - origin;
                let now = along(axis, offset);
                state.0.borrow_mut().ours = true;
                let grab = if (thumb.start..=thumb.start + thumb.len).contains(&at) {
                    Some(at - thumb.start)
                } else if e.modifiers.shift {
                    scroll.scroll_to(place(axis, geo.jump(at), offset));
                    Some(geo.len() / 2.)
                } else {
                    scroll.scroll_to(place(axis, geo.page(now, at > thumb.start), offset));
                    None
                };
                if let Some(g) = grab {
                    let mut s = state.0.borrow_mut();
                    (s.grab, s.held) = (Some(g), geo.start_at(at - g));
                    drop(s);
                    scroll.drag(true);
                }
                cx.notify(view);
            });
        }
    }
}

/// The pane's deadline: one timer after a scroll, moved on (not re-armed)
/// by more scrolling; when it is past, the bar's view draws once more.
fn linger(state: BarState, view: EntityId, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            loop {
                let wait = state.0.borrow().until.map(|u| u.saturating_duration_since(Instant::now()));
                match wait {
                    Some(w) if !w.is_zero() => cx.background_executor().timer(w).await,
                    _ => break,
                }
            }
            {
                let mut s = state.0.borrow_mut();
                (s.timer, s.until) = (false, None);
            }
            cx.update(|_, cx| cx.notify(view)).ok();
        })
        .detach();
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's `test` attribute would shadow the built-in.
    use super::{EDGE, END, Geo, LINGER, MIN_THUMB, THIN, Thumb, WIDE, anchor, lane, list_content, shown, strip, thumb_rect, unwarp, warp};
    use gpui_kit::{Axis, Bounds, point, px, size};
    use std::time::{Duration, Instant};

    const G: Geo = Geo { track: 500., view: 500., content: 2000., warp: None };

    #[test]
    fn the_thumb_is_the_viewports_share_of_the_track_never_under_the_minimum() {
        assert_eq!(G.len(), 125.);
        assert!(G.overflows());
        let long = Geo { content: 1_000_000., ..G };
        assert_eq!(long.len(), MIN_THUMB);
        // A track shorter than the minimum: the whole track.
        assert_eq!(Geo { track: 20., view: 20., content: 1000., warp: None }.len(), 20.);
        // Content that fits (or overhangs by under a pixel) draws no bar.
        assert!(!Geo { content: 500., ..G }.overflows());
        assert!(!Geo { content: 500.6, ..G }.overflows());
        assert!(!Geo { content: 300., ..G }.overflows());
    }

    #[test]
    fn the_thumb_travels_the_track_as_the_offset_runs_the_range() {
        assert_eq!(G.max(), 1500.);
        assert_eq!(G.thumb(0.), Thumb { start: 0., len: 125. });
        assert_eq!(G.thumb(1500.), Thumb { start: 375., len: 125. });
        assert_eq!(G.thumb(750.).start, 187.5);
        // Out of range (a list's live offset past a held height) stays on the track.
        assert_eq!(G.thumb(9000.).start, 375.);
        assert_eq!(G.thumb(-5.).start, 0.);
        // With the minimum, the travel is what the minimum leaves.
        let long = Geo { content: 1_000_000., ..G };
        assert_eq!(long.thumb(long.max()).start, 500. - MIN_THUMB);
    }

    #[test]
    fn a_track_click_pages_toward_it_and_stops_at_the_ends() {
        assert_eq!(G.page(0., true), 500.);
        assert_eq!(G.page(700., false), 200.);
        assert_eq!(G.page(1200., true), 1500.);
        assert_eq!(G.page(200., false), 0.);
    }

    #[test]
    fn a_drag_maps_the_thumb_back_to_its_offset_and_shift_click_centres_it() {
        for o in [0., 1., 333., 750., 1499., 1500.] {
            let t = G.thumb(o);
            assert!((G.offset_at(t.start) - o).abs() < 1e-3, "{o}");
        }
        // Past either end of the track: the ends of the scroll.
        assert_eq!(G.offset_at(-40.), 0.);
        assert_eq!(G.offset_at(10_000.), 1500.);
        // Shift+click at 250 (the middle): the thumb centred there.
        let o = G.jump(250.);
        let t = G.thumb(o);
        assert!((t.start + t.len / 2. - 250.).abs() < 1e-3);
        assert_eq!(G.jump(5.), 0.);
        assert_eq!(G.jump(495.), 1500.);
        // A drag on a track the thumb fills moves nothing.
        assert_eq!(Geo { track: 20., view: 20., content: 1000., warp: None }.offset_at(10.), 0.);
    }

    /// GPUI's `ListState::set_offset_from_scrollbar` (gpui-pre 0.3.12,
    /// list.rs): its range is the rows' height plus the list's padding less
    /// the viewport, and a drag ending within 1 px of that end re-arms
    /// tail-following; anywhere else stops it. The transcript list's
    /// `max_offset_for_scrollbar` leaves the padding out, so the bar adds it.
    #[test]
    fn dragging_the_transcripts_thumb_to_the_end_follows_and_away_does_not() {
        let (rows, pad, view) = (40_000_f32, 8. + 28., 800.);
        let scroll_max = rows + pad - view;
        let follows = |offset: f32| offset >= scroll_max - 1.;
        let g = Geo { track: view - 2. * END, view, content: list_content(view, rows - view, pad), warp: None };
        assert_eq!(g.max(), scroll_max);
        let end = g.track - g.len();
        assert!(follows(g.offset_at(end)), "the thumb at its end is the end");
        assert!(follows(g.offset_at(end + 30.)), "past it too");
        assert!(!follows(g.offset_at(end - 1.)), "a pixel up stops following");
        assert!(!follows(g.offset_at(0.)));
        // Without the padding the thumb's end is 36 px short: never re-armed.
        let bare = Geo { content: view + (rows - view), ..g };
        assert!(!follows(bare.offset_at(end + 30.)));
        // A list that fits draws no bar.
        assert!(!Geo { content: list_content(view, 0., pad), ..g }.overflows());
    }

    #[test]
    fn a_bar_shows_under_the_pointer_while_dragged_and_for_a_second_after_a_scroll() {
        let t0 = Instant::now();
        let until = Some(t0 + LINGER);
        assert!(!shown(false, false, false, None, t0), "at rest: hidden");
        assert!(shown(false, false, false, until, t0 + Duration::from_millis(999)));
        assert!(!shown(false, false, false, until, t0 + LINGER), "gone at its deadline");
        assert!(shown(false, true, false, until, t0 + Duration::from_secs(5)), "the pointer keeps it");
        assert!(shown(false, false, true, None, t0), "a drag keeps it");
        assert!(shown(true, false, false, None, t0), "the transcript's always shows");
        assert_eq!(LINGER, Duration::from_secs(1));
    }

    #[test]
    fn the_bar_lies_over_the_panes_edge_and_widens_inward() {
        let vp = Bounds::new(point(px(100.), px(50.)), size(px(600.), px(400.)));
        let thin = lane(Axis::Vertical, vp, THIN);
        let wide = lane(Axis::Vertical, vp, WIDE);
        assert_eq!(thin.right(), px(700. - EDGE));
        assert_eq!(wide.right(), thin.right());
        assert_eq!(wide.size.width, px(WIDE));
        assert_eq!((thin.top(), thin.bottom()), (px(50. + END), px(450. - END)));
        // The strip that takes the pointer is inside the pane.
        let s = strip(Axis::Vertical, vp);
        assert_eq!(s.right(), vp.right());
        assert!(s.size.width <= px(16.), "narrower than the transcript's least gutter");
        let r = thumb_rect(Axis::Vertical, wide, Thumb { start: 10., len: 40. });
        assert_eq!((r.top(), r.size.height, r.left()), (wide.top() + px(10.), px(40.), wide.left()));
    }

    #[test]
    fn a_horizontal_overflow_runs_its_bar_along_the_bottom() {
        let vp = Bounds::new(point(px(20.), px(30.)), size(px(300.), px(120.)));
        let l = lane(Axis::Horizontal, vp, THIN);
        assert_eq!((l.bottom(), l.size.height), (px(150. - EDGE), px(THIN)));
        assert_eq!((l.left(), l.size.width), (px(20. + END), px(300. - 2. * END)));
        // 300 px of a 900-px line: a third of the track, at the far end when scrolled out.
        let g = Geo { track: f32::from(l.size.width), view: 300., content: 900., warp: None };
        let t = g.thumb(g.max());
        assert!((t.len - g.track / 3.).abs() < 1e-3);
        let r = thumb_rect(Axis::Horizontal, l, t);
        assert!((f32::from(r.right()) - f32::from(l.right())).abs() < 1e-3);
        assert_eq!((r.top(), r.size.height), (l.top(), px(THIN)));
        let s = strip(Axis::Horizontal, vp);
        assert_eq!((s.bottom(), s.size.width), (vp.bottom(), vp.size.width));
    }

    /// While dragged the thumb is where the pointer holds it, clamped to
    /// the track, and its drop point is that share of the travel.
    #[test]
    fn a_dragged_thumb_stays_under_the_pointer() {
        let travel = G.track - G.len();
        assert_eq!(G.start_at(-40.), 0.);
        assert_eq!(G.start_at(travel + 40.), travel);
        assert_eq!(G.start_at(100.), 100.);
        assert_eq!(G.frac_at(travel / 4.), 0.25);
        assert_eq!(G.frac_at(G.start_at(1e6)), 1.);
        assert_eq!(G.offset_at(G.start_at(1e6)), G.max(), "held past the end is the end: it follows");
        let fits = Geo { content: 400., ..G };
        assert_eq!((fits.start_at(50.), fits.frac_at(50.)), (0., 0.));
    }

    /// Let go at 40 % of the travel on a height GPUI held; the rows
    /// measured meanwhile put the content at 52 %. The thumb stays at 40 %,
    /// the ends stay the ends, and it moves on continuously from there.
    #[test]
    fn a_dropped_thumb_stays_where_it_was_let_go() {
        let a = anchor(0.52, 0.40);
        assert_eq!(a, Some((0.52, 0.40)));
        assert!((warp(0.52, a) - 0.40).abs() < 1e-6);
        assert_eq!((warp(0., a), warp(1., a)), (0., 1.));
        let mut prev = -1.;
        for i in 0..=100 {
            let f = i as f32 / 100.;
            let d = warp(f, a);
            assert!(d >= prev, "monotone at {f}");
            assert!((unwarp(d, a) - f).abs() < 1e-5, "round trip at {f}");
            prev = d;
        }
        // A drag from the warped thumb maps through it: the thumb shown at
        // 40 % of the travel is the content at 52 %.
        let g = Geo { warp: a, ..G };
        let travel = g.track - g.len();
        assert!((g.offset_at(0.40 * travel) - 0.52 * g.max()).abs() < 1e-2);
        assert!((g.thumb(0.52 * g.max()).start - 0.40 * travel).abs() < 1e-2);
        assert_eq!(g.thumb(g.max()).start, travel);
        assert_eq!(g.offset_at(travel), g.max(), "the end still follows");
        // No warp at the ends, nor where the two agree.
        assert_eq!(anchor(0., 0.3), None);
        assert_eq!(anchor(1., 0.7), None);
        assert_eq!(anchor(0.5, 0.5), None);
    }
}
