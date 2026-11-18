//! Sticky message headers (polish.md P15): while any row of a message is
//! on screen but its `YOU`/`AGENT` label has scrolled above the viewport's
//! top + `pad_y`, the label and its time are drawn pinned there, in the
//! gutter, looking exactly like themselves (C4). The next message's label
//! (or a day line) pushes the pinned one up by exactly the overlap.
//!
//! GPUI's `list` has no sticky rows, so this is a layer over the list
//! ([`Layer`]): after the list's layout, in the same frame, it reads the
//! list's top row (`ListState::logical_scroll_top`) and the rows' bounds,
//! so it follows the scroll 1:1, with no spring. Placement, not motion: the
//! same at Full, Reduced and Off; it asks for no frame and moves no row.
//! It is rebuilt on every app frame (two small trees, no cache).
//! - Hanging (wide panes): the label sits on the empty gutter and has no
//!   hitbox; every click, hover and wheel goes to what is under it.
//! - Inline (F8, narrow panes): the band hides live rows, so it takes the
//!   pointer but not the wheel (nothing hidden under it can be clicked or
//!   hovered; the wheel still scrolls the list), and it hides the beat
//!   layer's spinners and sweeps under it ([`crate::motion::Motion::cover_top`]).
//!
//! What it assumes of the list:
//! - No top padding: GPUI's `bounds_for_item` leaves `padding.top` out while
//!   painted rows include it, so a top padding would shift every pin by it
//!   (the list's `pad_t` is 0, app.rs).
//! - Every labelled row's gap is at least the pin's offset (part 16, turn 40,
//!   first 20, all ·r, against `pad_y` 12·r): when the top row changes, the
//!   next label is already at or below the pin, so nothing pops.
//!
//! Transients it accepts: during Q4's lift the pane is drawn at α k, the
//! fill included, so the original label shows through it by k·(1−k) for
//! `QUICK`; during Q1's arrival a pushing label is drawn `travel` lower than
//! its slot, a gap, never an overlap.

use std::rc::Rc;

use gpui_kit::*;

use crate::motion::Motion;
use crate::turns::{Disp, Kind, Who};

/// A label pinned: the display row that carries it, and its top, in px
/// from the viewport's top (negative: pushed partly out).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stuck {
    pub row: usize,
    pub y: f32,
}

/// Where a message starts: a labelled row, or a day line (a day line
/// between turns ends the message above it).
fn starts(d: &Disp) -> bool {
    d.who != Who::None || d.kind == Kind::Day
}

/// The labels to pin over `disp`, the list's top row being `top`. `at(i)`:
/// row `i`'s label line (a day line's line), in px from the viewport's
/// top, if the list laid that row out; `pin`: where a pinned label sits;
/// `h(i)`: row `i`'s label's height, pinned.
/// - The top message's, once its label would go above `pin` (none while
///   it is at `pin` or below); the next message's label pushes it up.
/// - With the top row a message's first, whose label is still coming up:
///   the message above's, pushed by it, until that label reaches the top
///   (so nothing pops out while part of it shows).
pub(crate) fn place(
    disp: &[Disp],
    top: usize,
    at: impl Fn(usize) -> Option<f32>,
    pin: f32,
    h: impl Fn(usize) -> f32,
) -> [Option<Stuck>; 2] {
    let before = |end: usize| (0..end).rev().find(|&i| starts(&disp[i])).filter(|&i| disp[i].who != Who::None);
    let Some(start) = disp.len().checked_sub(1).and_then(|last| (0..=top.min(last)).rev().find(|&i| starts(&disp[i]))) else {
        return [None, None];
    };
    // Above the top row, its label is above the viewport.
    let natural = if start == top { at(start) } else { Some(f32::NEG_INFINITY) };
    let pushed = |row: usize, by: Option<f32>| {
        let h = h(row);
        let y = by.map_or(pin, |n| pin.min(n - h));
        (y + h > 0.).then_some(Stuck { row, y })
    };
    let next = || disp.iter().enumerate().skip(top + 1).find(|(_, d)| starts(d)).and_then(|(i, _)| at(i));
    let own = natural.filter(|n| *n < pin && disp[start].who != Who::None).and_then(|_| pushed(start, next()));
    let above = natural.filter(|_| start == top).and_then(|n| before(start).and_then(|p| pushed(p, Some(n))));
    [above, own]
}

/// What the layer draws, built after the list's layout from the layer's
/// bounds (the list's): the pinned labels at their places, or nothing.
pub(crate) struct Pinned {
    pub el: AnyElement,
    /// An opaque strip across the column from the view's top (the band), in
    /// window px: the beat layer paints nothing under it.
    pub cover: Option<Bounds<Pixels>>,
}

type Build = Box<dyn Fn(Bounds<Pixels>, &mut Window, &mut App) -> Option<Pinned>>;

/// The layer over the list: an absolute box over it whose content is
/// built in its prepaint, after the list's (it comes after the list among
/// the pane's children), and clipped to it.
pub(crate) struct Layer {
    fx: Rc<Motion>,
    build: Build,
    child: Option<Pinned>,
}

pub(crate) fn layer(fx: Rc<Motion>, build: impl Fn(Bounds<Pixels>, &mut Window, &mut App) -> Option<Pinned> + 'static) -> Layer {
    Layer { fx, build: Box::new(build), child: None }
}

impl IntoElement for Layer {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Layer {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style { position: Position::Absolute, inset: Edges::all(px(0.).into()), ..Style::default() };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child = (self.build)(bounds, window, cx);
        if let Some(Pinned { el: child, .. }) = self.child.as_mut() {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                child.layout_as_root(
                    size(AvailableSpace::Definite(bounds.size.width), AvailableSpace::Definite(bounds.size.height)),
                    window,
                    cx,
                );
                child.prepaint_at(bounds.origin, window, cx);
            });
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(Pinned { el, cover }) = self.child.as_mut() {
            window.with_content_mask(Some(ContentMask { bounds }), |window| el.paint(window, cx));
            // The list has painted and recorded its glyphs' sites; the beat
            // layer paints them after this whole frame, so over the band.
            if let Some(c) = cover.map(|c| c.intersect(&bounds)).filter(|c| !c.is_empty()) {
                self.fx.cover_top(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use super::{Stuck, place};
    use crate::turns::{Disp, Gap, Kind, Who};

    fn row(kind: Kind, who: Who) -> Disp {
        Disp { item: 0, id: 0, rev: 0, kind, gap: Gap::Part, who }
    }

    /// YOU (0), AGENT (1) and its run (2..6), YOU (6), AGENT (7).
    fn turns() -> Vec<Disp> {
        let mut d = vec![row(Kind::User, Who::You), row(Kind::Prose, Who::Agent)];
        d.extend((0..4).map(|_| row(Kind::Work, Who::None)));
        d.extend([row(Kind::User, Who::You), row(Kind::Prose, Who::Agent)]);
        d
    }

    /// The one label pinned, if just one.
    fn one(s: [Option<Stuck>; 2]) -> Option<Stuck> {
        match s {
            [None, one] | [one, None] => one,
            both => panic!("two pinned: {both:?}"),
        }
    }

    #[test]
    fn the_top_messages_label_pins() {
        let d = turns();
        // The top row is in the agent's run: its label (row 1) is above.
        assert_eq!(one(place(&d, 3, |_| None, 12., |_| 46.)), Some(Stuck { row: 1, y: 12. }));
        // Its labelled row on top, the label still at the pin or below:
        // its own is not pinned; YOU's above is, pushed by it.
        assert_eq!(one(place(&d, 1, |i| (i == 1).then_some(12.), 12., |_| 46.)), Some(Stuck { row: 0, y: -34. }));
        assert_eq!(one(place(&d, 1, |i| (i == 1).then_some(80.), 12., |_| 46.)), Some(Stuck { row: 0, y: 12. }));
        // Scrolled past the pin by a pixel: pinned, YOU's still going out.
        assert_eq!(
            place(&d, 1, |i| (i == 1).then_some(11.), 12., |_| 46.),
            [Some(Stuck { row: 0, y: -35. }), Some(Stuck { row: 1, y: 12. })]
        );
        // YOU's gone once AGENT's reaches the top.
        assert_eq!(one(place(&d, 1, |i| (i == 1).then_some(0.), 12., |_| 46.)), Some(Stuck { row: 1, y: 12. }));
        // The first row on top: nothing above it.
        assert_eq!(one(place(&d, 0, |i| (i == 0).then_some(12.), 12., |_| 46.)), None);
    }

    #[test]
    fn the_next_label_pushes_by_the_overlap() {
        let d = turns();
        // The next label (row 6) 100 px down: no overlap.
        assert_eq!(one(place(&d, 4, |i| (i == 6).then_some(100.), 12., |_| 46.)), Some(Stuck { row: 1, y: 12. }));
        // At 50: it overlaps the pinned one (12 + 46) by 8, which goes up 8.
        assert_eq!(one(place(&d, 4, |i| (i == 6).then_some(50.), 12., |_| 46.)), Some(Stuck { row: 1, y: 4. }));
        // At 20: 38 up, partly out of the top.
        assert_eq!(one(place(&d, 5, |i| (i == 6).then_some(20.), 12., |_| 46.)), Some(Stuck { row: 1, y: -26. }));
        // Its row on top, the label at 30: AGENT's last 30 px, touching it.
        assert_eq!(one(place(&d, 6, |i| (i == 6).then_some(30.), 12., |_| 46.)), Some(Stuck { row: 1, y: -16. }));
        // At 5, past the pin: YOU's pinned, AGENT's last 5 px still going.
        let both = [Some(Stuck { row: 1, y: -41. }), Some(Stuck { row: 6, y: 12. })];
        assert_eq!(place(&d, 6, |i| (i == 6).then_some(5.), 12., |_| 46.), both);
        // At the top: out.
        assert_eq!(one(place(&d, 6, |i| (i == 6).then_some(0.), 12., |_| 46.)), Some(Stuck { row: 6, y: 12. }));
    }

    #[test]
    fn a_day_line_ends_the_message_above() {
        let mut d = turns();
        d.insert(6, row(Kind::Day, Who::None));
        // The day line pushes like a label.
        assert_eq!(one(place(&d, 5, |i| (i == 6).then_some(40.), 12., |_| 46.)), Some(Stuck { row: 1, y: -6. }));
        assert_eq!(one(place(&d, 6, |i| (i == 6).then_some(10.), 12., |_| 46.)), Some(Stuck { row: 1, y: -36. }));
        // On top, out of the top: nothing pinned over it.
        assert_eq!(one(place(&d, 6, |_| Some(-10.), 12., |_| 46.)), None);
        // Nor above the YOU under it.
        assert_eq!(one(place(&d, 7, |_| Some(30.), 12., |_| 46.)), None);
        // A transcript that starts with notes: no label to pin.
        let notes = vec![row(Kind::Note, Who::None), row(Kind::Note, Who::None)];
        assert_eq!(one(place(&notes, 1, |_| None, 12., |_| 46.)), None);
        assert_eq!(one(place(&[], 0, |_| None, 12., |_| 46.)), None);
    }
}
