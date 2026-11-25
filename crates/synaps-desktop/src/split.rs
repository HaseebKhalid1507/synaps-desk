//! Split panes, step 3 (split/split.md §1, §4; split/architecture.md §2.1):
//! the layout tree. VS Code's grid: n-ary and normalised, at most
//! [`PANES_MAX`] panes, as many across or down as the room allows (the
//! 2026-11-25 note under split.md §0), each at least [`PANE_MIN_W`] ×
//! [`PANE_MIN_H`].

use crate::pane::{PANES_MAX, PaneId};

/// The narrowest a pane gets (split.md §4): the tight layout's floor.
pub(crate) const PANE_MIN_W: f32 = 320.;
/// The shortest a pane gets (split.md §4): its tab strip, a cramped
/// composer and some rows.
pub(crate) const PANE_MIN_H: f32 = 240.;
/// The rule between two panes (split.md §4: a 1 px `hair` rule).
pub(crate) const DIVIDER: f32 = 1.;
/// Side by side (`Row`) or stacked (`Column`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    Row,
    Column,
}

/// Where a new pane goes beside another (a drop on an edge: any of the
/// four, split.md §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Right,
    Down,
    Left,
    Up,
}

impl Side {
    /// The axis its split runs on, and whether the new pane comes first.
    const fn axis(self) -> (Axis, bool) {
        match self {
            Side::Right => (Axis::Row, false),
            Side::Left => (Axis::Row, true),
            Side::Down => (Axis::Column, false),
            Side::Up => (Axis::Column, true),
        }
    }
}

/// Released within this many px of equal, a divider lands on equal
/// (split.md §4).
pub(crate) const DETENT: f32 = 8.;

/// A divider (split.md §4), as [`Node::dividers`] finds it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Divider {
    pub path: Vec<usize>,
    pub i: usize,
    pub axis: Axis,
    pub rect: Rect,
    pub at: f32,
}

/// A layout change on `sheet` (split.md §10): a split, a close, a
/// double-click's equalise or a release's detent. The tree is drawn from
/// `from` to `to` (the same shape: a split's new pane and a closing pane
/// are in both, a closing one at a share of 0 in `to`) by one spring
/// `travel` px long, the most any edge moves; what lands is the tree
/// itself. A change made mid-flight starts from the tree as drawn, the
/// panes still closing kept closing (Shady #42 M1).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Flight {
    pub from: Node,
    pub to: Node,
    /// The splits' new panes: each laid out at its landed size from its
    /// first frame, its share clipping it (it slides in with the split
    /// line).
    pub grow: Vec<PaneId>,
    /// The panes closes took: each share its ground only, closing.
    pub ghosts: Vec<PaneId>,
    /// Its spring's length; 0 until its first frame measures it.
    pub travel: f32,
    /// Each pane's narrowest box in the flight: its column keeps that wrap
    /// until the flight lands (the panels' `PanelFx.freeze` rule: it
    /// rewraps once, at the start as it narrows, at the end as it widens).
    pub wrap: Vec<(PaneId, f32)>,
    /// The centres `wrap` was set at, the flight's start's and its
    /// landing's, each with its origin in the window (Shady #42 L2: set
    /// once, not the narrowest drawn so far, which crept frame by frame
    /// under a panel's flight; #62 r4 R4-1: the same width at another
    /// origin is another layout); `None`: not yet (the wraps a flight
    /// carries on from are kept where narrower).
    pub wrap_at: Option<(Rect, Rect)>,
    /// A panel yielding on this flight (#51): the layout rides its spring.
    pub ride: Option<Ride>,
}

/// A layout riding a yielding panel's spring (#51): its step is how far
/// the centre has gone from where they start to where they land (each
/// pane's width moving linearly between its ends, [`Node::lerp_px_from`]),
/// and it lands with the panel, in the frame the panel does (#52).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ride {
    /// The centre's width where they start, and where they land.
    pub from: f32,
    pub to: f32,
    /// The centre's left edge in the window where they start: the
    /// starting tree's boxes there are its columns' (Shady #62 r4 R4-1:
    /// read at the landing's edge, a pane 378 wide was 377.8, and its
    /// column went 346 → 345 → 427).
    pub from_x: f32,
}

impl Flight {
    /// The velocity (px/s of its travel) this flight starts at, taking over
    /// `was` (its ends and its travel) moving at `v` (px/s of that travel),
    /// in `area`: each pane edge's velocity carried over as nearly as one
    /// spring can carry it (least squares over the edges of the panes in
    /// both). A reversal (a split's close mid-flight) carries it whole: the
    /// layout turns from where it is with its speed, as a panel does
    /// ([`crate::chrome::PanelFx::fly`]).
    pub(crate) fn carried(&self, was: &(Node, Node, f32), v: f32, area: Rect) -> f32 {
        let (a, b, len) = was;
        if *len < 1. || self.travel < 1. {
            return 0.;
        }
        let old = b.moves_from(a, area);
        let (mut uw, mut ww) = (0f32, 0f32);
        for (id, w) in self.to.moves_from(&self.from, area) {
            ww += w.iter().map(|d| d * d).sum::<f32>();
            if let Some((_, u)) = old.iter().find(|(i, _)| *i == id) {
                uw += u.iter().zip(w).map(|(u, w)| u * w).sum::<f32>();
            }
        }
        if ww < 1. {
            return 0.;
        }
        self.travel * (v / len) * uw / ww
    }

    pub(crate) fn new(from: Node, to: Node, grow: Vec<PaneId>, ghosts: Vec<PaneId>) -> Self {
        Flight { from, to, grow, ghosts, travel: 0., wrap: Vec::new(), wrap_at: None, ride: None }
    }

    /// A frame of its ride `r` (#51), the centre `area`: the panes by how
    /// far the centre has gone from the ride's `from` toward its `to`.
    /// Under 0 while a panel turned back carries on its way first (a
    /// split's close mid-flight: [`crate::pane::Carry::Ride`]), as far as
    /// it goes: an early close's way back is a few px and the panel's
    /// overshoot several times that (Shady #62 M1: held at −1, the divider
    /// stood while the panel moved). Each edge stays between its
    /// neighbours ([`Node::lerp_px_from`]).
    pub(crate) fn ride_frame(&self, r: Ride, area: Rect) -> Node {
        let t = if (r.to - r.from).abs() < 0.5 { 1. } else { ((area.w - r.from) / (r.to - r.from)).min(1.) };
        let at = |w: f32| Rect { w, ..area };
        self.to.lerp_px_from(&self.from, t, at(r.from), at(r.to), area)
    }

    /// Each pane's wrap (its narrowest width on the way), set from the
    /// flight's ends laid out in `area` (the centre at the panels'
    /// narrowest): a pane's width is a product of ratios lerped linearly,
    /// least at an end (Shady #42 L2). Again only when `area`'s width
    /// changes (a panel's flight starts or lands, a resize). A growing or
    /// closing pane keeps its landed or last box, as before.
    pub(crate) fn set_wraps(&mut self, area: Rect) {
        self.set_wraps_at(area, area);
    }

    /// [`Flight::set_wraps`] from its ends each in its own centre: `from`
    /// where the flight starts, `to` where it lands, each at its own
    /// origin in the window (`x`, `y`: the panes are rounded on the
    /// window's pixels, [`Node::rects_in`]). A panel yielding on this
    /// flight (#51) moves on its spring from its start, so the centre and
    /// the ratios move together: a pane's width, the product of a share
    /// and the centre moving the other way, is still least at an end.
    pub(crate) fn set_wraps_at(&mut self, from: Rect, to: Rect) {
        if self.wrap_at == Some((from, to)) {
            return;
        }
        let carried = self.wrap_at.is_none();
        let ends = [self.from.rects(from), self.to.rects(to)];
        for id in self.to.leaves() {
            if self.grow.contains(&id) || self.ghosts.contains(&id) {
                continue;
            }
            let w = ends.iter().filter_map(|rs| rs.iter().find(|(i, _)| *i == id).map(|(_, r)| r.w)).fold(f32::INFINITY, f32::min);
            if !w.is_finite() {
                continue;
            }
            match self.wrap.iter_mut().find(|(i, _)| *i == id) {
                Some((_, old)) => *old = if carried { old.min(w) } else { w },
                None => self.wrap.push((id, w)),
            }
        }
        self.wrap_at = Some((from, to));
    }
}

/// A divider held down (split.md §4): which one, and how far past its
/// rule the pointer went down, along its axis (the divider found again in
/// the tree each move: a resize, a panel or a close moved it; Shady #42
/// L1, M2).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grab {
    pub divider: Divider,
    pub off: f32,
    pub moved: bool,
}

/// Why a split did not happen (the composer's status line says it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// [`PANES_MAX`] panes already.
    Most,
    /// No room: a pane would go under its minimum (the only limit across
    /// or down: the 2026-11-25 note under split.md §0).
    Room,
}

impl Refused {
    pub(crate) const fn note(self) -> &'static str {
        match self {
            Refused::Most => "9 panes is the most",
            Refused::Room => "no room to split this pane",
        }
    }
}

/// The layout tree (architecture §2.1): a leaf is a pane; a split's kids
/// share its length by `ratios` (Σ = 1).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    Leaf(PaneId),
    Split { axis: Axis, kids: Vec<Node>, ratios: Vec<f32> },
}

/// A rectangle in the centre's own pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Node {
    /// Every pane, in tree order.
    pub(crate) fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.walk(&mut |id| out.push(id));
        out
    }

    fn walk(&self, f: &mut impl FnMut(PaneId)) {
        match self {
            Node::Leaf(id) => f(*id),
            Node::Split { kids, .. } => kids.iter().for_each(|k| k.walk(f)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.leaves().len()
    }

    /// The most panes this tree puts side by side, and stacked.
    pub(crate) fn extent(&self) -> (usize, usize) {
        match self {
            Node::Leaf(_) => (1, 1),
            Node::Split { axis, kids, .. } => {
                let ex: Vec<(usize, usize)> = kids.iter().map(Node::extent).collect();
                match axis {
                    Axis::Row => (ex.iter().map(|e| e.0).sum(), ex.iter().map(|e| e.1).max().unwrap_or(1)),
                    Axis::Column => (ex.iter().map(|e| e.0).max().unwrap_or(1), ex.iter().map(|e| e.1).sum()),
                }
            }
        }
    }

    /// Re-establish the invariants (architecture §2.1): no split with one
    /// kid, none with a kid split on its own axis (flattened, ratios
    /// multiplied in), ratios summing to 1.
    pub(crate) fn normalise(self) -> Node {
        match self {
            Node::Leaf(_) => self,
            Node::Split { axis, kids, ratios } => {
                let mut k2 = Vec::new();
                let mut r2 = Vec::new();
                for (kid, r) in kids.into_iter().zip(ratios) {
                    match kid.normalise() {
                        Node::Split { axis: a, kids: kk, ratios: rr } if a == axis => {
                            for (k, q) in kk.into_iter().zip(rr) {
                                k2.push(k);
                                r2.push(q * r);
                            }
                        }
                        k => {
                            k2.push(k);
                            r2.push(r);
                        }
                    }
                }
                if k2.len() == 1 {
                    return k2.pop().unwrap_or(Node::Leaf(PaneId::FIRST));
                }
                let sum: f32 = r2.iter().sum();
                let r2 = if sum > 0. { r2.iter().map(|r| r / sum).collect() } else { vec![1. / k2.len() as f32; k2.len()] };
                Node::Split { axis, kids: k2, ratios: r2 }
            }
        }
    }

    /// The tree with `new` beside `at` on `side`, `at`'s share halved
    /// between them; `None` if `at` is not in it.
    fn with_split(&self, at: PaneId, side: Side, new: PaneId) -> Option<Node> {
        let (axis, first) = side.axis();
        match self {
            Node::Leaf(id) if *id == at => {
                let kids = if first { vec![Node::Leaf(new), Node::Leaf(at)] } else { vec![Node::Leaf(at), Node::Leaf(new)] };
                Some(Node::Split { axis, kids, ratios: vec![0.5, 0.5] })
            }
            Node::Leaf(_) => None,
            Node::Split { axis: a, kids, ratios } => {
                // Beside a leaf of this split, on its axis: one more kid.
                if *a == axis
                    && let Some(i) = kids.iter().position(|k| *k == Node::Leaf(at))
                {
                    let (mut kids, mut ratios) = (kids.clone(), ratios.clone());
                    let half = ratios[i] / 2.;
                    ratios[i] = half;
                    let j = if first { i } else { i + 1 };
                    kids.insert(j, Node::Leaf(new));
                    ratios.insert(j, half);
                    return Some(Node::Split { axis: *a, kids, ratios });
                }
                let (i, kid) = kids.iter().enumerate().find_map(|(i, k)| k.with_split(at, side, new).map(|k| (i, k)))?;
                let mut kids = kids.clone();
                kids[i] = kid;
                Some(Node::Split { axis: *a, kids, ratios: ratios.clone() }.normalise())
            }
        }
    }

    /// Split `at` (split.md §2): `new` on `side`, or why not in a centre
    /// of `area`: [`PANES_MAX`] in all, and the minimums of every pane
    /// (room is the only limit across or down: the 2026-11-25 note under
    /// split.md §0). `at`'s share is halved between them; where that puts
    /// a pane under its minimum, the split `new` joins is evened out
    /// instead ([`Node::equalise`]), if that fits.
    pub(crate) fn split(&self, at: PaneId, side: Side, new: PaneId, area: (f32, f32)) -> Result<Node, Refused> {
        if self.len() >= PANES_MAX {
            return Err(Refused::Most);
        }
        let area = Rect { x: 0., y: 0., w: area.0, h: area.1 };
        let fits = |t: &Node| t.rects(area).iter().all(|(_, r)| r.w + 0.5 >= PANE_MIN_W && r.h + 0.5 >= PANE_MIN_H);
        let next = self.with_split(at, side, new).ok_or(Refused::Room)?;
        if fits(&next) {
            return Ok(next);
        }
        let mut even = next;
        let path = even.path_of(new).ok_or(Refused::Room)?;
        even.equalise(&path[..path.len().saturating_sub(1)], area);
        if fits(&even) { Ok(even) } else { Err(Refused::Room) }
    }

    /// Ctrl+\ (split.md §2): right, else down if the pane is too narrow
    /// for two; why not if neither fits.
    pub(crate) fn split_right(&self, at: PaneId, new: PaneId, area: (f32, f32)) -> Result<(Node, Side), Refused> {
        match self.split(at, Side::Right, new, area) {
            Ok(n) => Ok((n, Side::Right)),
            Err(Refused::Most) => Err(Refused::Most),
            Err(_) => self.split(at, Side::Down, new, area).map(|n| (n, Side::Down)),
        }
    }

    /// The tree without `id` (its share to the kids beside it), or `None`
    /// if it was the last pane.
    pub(crate) fn remove(&self, id: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(l) => (*l != id).then(|| self.clone()),
            Node::Split { axis, kids, ratios } => {
                let mut k2 = Vec::new();
                let mut r2 = Vec::new();
                for (k, r) in kids.iter().zip(ratios) {
                    if let Some(k) = k.remove(id) {
                        k2.push(k);
                        r2.push(*r);
                    }
                }
                if k2.is_empty() {
                    return None;
                }
                Some(Node::Split { axis: *axis, kids: k2, ratios: r2 }.normalise())
            }
        }
    }

    /// Each pane's rectangle in `area`, the dividers between kids taken
    /// out, every edge on a whole pixel of `area`'s own coordinates.
    pub(crate) fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.place(area, &mut out);
        out
    }

    /// [`Node::rects`] in `area` of the centre, the centre's top-left at
    /// `origin` in the window: every edge on a whole pixel of the window
    /// (where it is drawn), each rectangle in the centre's own pixels. A
    /// panel's edge moves the centre by device pixels (half a pixel at 2×);
    /// rounded from the centre's own edge, a divider could step back half
    /// a pixel as the panel stepped on (Shady #62 M2: rail 476.5 → 477, the
    /// first pane 495 → 494). From the window's, each edge is the rounding
    /// of where it is going, one way.
    pub(crate) fn rects_in(&self, area: Rect, origin: (f32, f32)) -> Vec<(PaneId, Rect)> {
        let shifted = Rect { x: area.x + origin.0, y: area.y + origin.1, ..area };
        self.rects(shifted).into_iter().map(|(id, r)| (id, Rect { x: r.x - origin.0, y: r.y - origin.1, ..r })).collect()
    }

    /// [`Node::dividers`] as [`Node::rects_in`] lays them out.
    pub(crate) fn dividers_in(&self, area: Rect, origin: (f32, f32)) -> Vec<Divider> {
        let shifted = Rect { x: area.x + origin.0, y: area.y + origin.1, ..area };
        let mut out = self.dividers(shifted);
        for d in &mut out {
            d.rect.x -= origin.0;
            d.rect.y -= origin.1;
        }
        out
    }

    fn place(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split { kids, .. } => {
                let mut subs = Vec::new();
                self.kid_rects(area, &mut subs);
                for (kid, sub) in kids.iter().zip(subs) {
                    kid.place(sub, out);
                }
            }
        }
    }

    /// The smallest this subtree gets along `axis`, on a whole pixel: its
    /// panes' minimums and the rules between them, each kid of a split on
    /// `axis` at its share (a drag scales a kid's own split, so the kid
    /// with the least room for its share sets it; Shady #42 M3).
    pub(crate) fn min_along(&self, axis: Axis) -> f32 {
        match self {
            Node::Leaf(_) => match axis {
                Axis::Row => PANE_MIN_W,
                Axis::Column => PANE_MIN_H,
            },
            Node::Split { axis: a, kids, ratios } if *a == axis => {
                let room = kids.iter().zip(ratios).filter(|(_, r)| **r > 0.).map(|(k, r)| k.min_along(axis) / r).fold(0f32, f32::max);
                (room - 1e-3).ceil() + DIVIDER * (kids.len() - 1) as f32
            }
            Node::Split { kids, .. } => kids.iter().map(|k| k.min_along(axis)).fold(0f32, f32::max),
        }
    }

    /// The smallest centre this tree fits in with every pane at least
    /// `w` × `h` (its most panes across and down, the rules between).
    pub(crate) fn min_size(&self, w: f32, h: f32) -> (f32, f32) {
        let (across, down) = self.extent();
        (across as f32 * w + (across - 1) as f32 * DIVIDER, down as f32 * h + (down - 1) as f32 * DIVIDER)
    }

    /// The same tree with leaf `from` named `to` (a closing pane's share
    /// once its id is handed out again: Shady #42 r2-L1).
    pub(crate) fn renamed(&self, from: PaneId, to: PaneId) -> Node {
        match self {
            Node::Leaf(id) => Node::Leaf(if *id == from { to } else { *id }),
            Node::Split { axis, kids, ratios } => {
                Node::Split { axis: *axis, kids: kids.iter().map(|k| k.renamed(from, to)).collect(), ratios: ratios.clone() }
            }
        }
    }

    /// This tree's shape with each split's ratios where `rects` puts its
    /// panes (their extent along its axis), `ghosts` (closing) at 0: a
    /// flight's end with its closing panes in it (Shady #42 r2-L2).
    pub(crate) fn fitted(&self, rects: &[(PaneId, Rect)], ghosts: &[PaneId]) -> Node {
        match self {
            Node::Leaf(_) => self.clone(),
            Node::Split { axis, kids, ratios } => {
                let ext: Vec<f32> = kids.iter().map(|k| k.span_of(rects, ghosts, *axis)).collect();
                let sum: f32 = ext.iter().sum();
                let ratios = if sum > 0. { ext.iter().map(|e| e / sum).collect() } else { ratios.clone() };
                Node::Split { axis: *axis, kids: kids.iter().map(|k| k.fitted(rects, ghosts)).collect(), ratios }
            }
        }
    }

    /// The span of this node's panes (`ghosts` aside) in `rects` along
    /// `axis`; 0 with none.
    fn span_of(&self, rects: &[(PaneId, Rect)], ghosts: &[PaneId], axis: Axis) -> f32 {
        let (lo, hi) = self
            .leaves()
            .into_iter()
            .filter(|id| !ghosts.contains(id))
            .filter_map(|id| rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r))
            .map(|r| match axis {
                Axis::Row => (r.x, r.x + r.w),
                Axis::Column => (r.y, r.y + r.h),
            })
            .fold((f32::MAX, f32::MIN), |(a, b), (l, h)| (a.min(l), b.max(h)));
        (hi - lo).max(0.)
    }

    /// The same panes in the same splits (ratios aside): a flight can go
    /// from one to the other.
    pub(crate) fn same_shape(&self, other: &Node) -> bool {
        match (self, other) {
            (Node::Leaf(a), Node::Leaf(b)) => a == b,
            (Node::Split { axis, kids, .. }, Node::Split { axis: a2, kids: k2, .. }) => {
                axis == a2 && kids.len() == k2.len() && kids.iter().zip(k2).all(|(a, b)| a.same_shape(b))
            }
            _ => false,
        }
    }

    /// The tree with `new` beside `at` on `side` (as a split makes it),
    /// with no checks: a flight's ends.
    pub(crate) fn split_as(&self, at: PaneId, side: Side, new: PaneId) -> Option<Node> {
        self.with_split(at, side, new)
    }

    /// Every divider (split.md §4): its split's path from the root (kid
    /// indices), the kid it follows, the split's axis and rectangle, and
    /// where it is (px from the split's start along its axis).
    pub(crate) fn dividers(&self, area: Rect) -> Vec<Divider> {
        let mut out = Vec::new();
        self.find_dividers(area, &mut Vec::new(), &mut out);
        out
    }

    fn find_dividers(&self, area: Rect, path: &mut Vec<usize>, out: &mut Vec<Divider>) {
        let Node::Split { axis, kids, .. } = self else { return };
        let mut rects = Vec::new();
        self.kid_rects(area, &mut rects);
        for (i, r) in rects.iter().enumerate() {
            if i + 1 < rects.len() {
                let at = match axis {
                    Axis::Row => r.x + r.w - area.x,
                    Axis::Column => r.y + r.h - area.y,
                };
                out.push(Divider { path: path.clone(), i, axis: *axis, rect: area, at });
            }
            path.push(i);
            kids[i].find_dividers(*r, path, out);
            path.pop();
        }
    }

    /// Each kid's rectangle in `area` (as [`Node::rects`] lays them): each
    /// edge between kids on a whole pixel of `area`'s coordinates (its own
    /// start may not be on one: [`Node::rects_in`]), as a flight's step
    /// rounds them ([`Node::lerp_px_from`]).
    fn kid_rects(&self, area: Rect, out: &mut Vec<Rect>) {
        let Node::Split { axis, kids, ratios } = self else { return };
        let (start, len) = match axis {
            Axis::Row => (area.x, area.w),
            Axis::Column => (area.y, area.h),
        };
        let room = (len - DIVIDER * (kids.len() - 1) as f32).max(0.);
        let (mut at, mut acc) = (0f32, 0f32);
        for (i, r) in ratios.iter().enumerate() {
            acc += r;
            let base = start + DIVIDER * i as f32;
            let end = if i + 1 == kids.len() { room } else { ((base + room * acc).round() - base).clamp(at, room) };
            let off = at + DIVIDER * i as f32;
            out.push(match axis {
                Axis::Row => Rect { x: area.x + off, y: area.y, w: (end - at).max(0.), h: area.h },
                Axis::Column => Rect { x: area.x, y: area.y + off, w: area.w, h: (end - at).max(0.) },
            });
            at = end;
        }
    }

    /// Its shape into `h`: the axes, each kid, each ratio's bits, each leaf
    /// (a cheap key for [`crate::app::AppView`]'s layout generation).
    pub(crate) fn hash_shape(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        match self {
            Node::Leaf(id) => (0u8, id).hash(h),
            Node::Split { axis, kids, ratios } => {
                (1u8, *axis == Axis::Row, kids.len()).hash(h);
                for (k, r) in kids.iter().zip(ratios) {
                    r.to_bits().hash(h);
                    k.hash_shape(h);
                }
            }
        }
    }

    /// This tree as the settings file keeps it (split/architecture.md §5):
    /// each pane by its index in `order`.
    pub(crate) fn to_saved(&self, order: &[PaneId]) -> crate::theme::LayoutNode {
        match self {
            Node::Leaf(id) => crate::theme::LayoutNode::Pane(order.iter().position(|o| o == id).unwrap_or(0)),
            Node::Split { axis, kids, ratios } => crate::theme::LayoutNode::Split {
                axis: *axis,
                ratios: ratios.clone(),
                kids: kids.iter().map(|k| k.to_saved(order)).collect(),
            },
        }
    }

    /// A saved tree with its pane `i` as `ids[i]` (a whole one:
    /// [`crate::theme::load_layout`]), normalised.
    pub(crate) fn from_saved(n: &crate::theme::LayoutNode, ids: &[PaneId]) -> Node {
        fn build(n: &crate::theme::LayoutNode, ids: &[PaneId]) -> Node {
            match n {
                crate::theme::LayoutNode::Pane(i) => Node::Leaf(ids.get(*i).copied().unwrap_or(PaneId::FIRST)),
                crate::theme::LayoutNode::Split { axis, ratios, kids } => {
                    Node::Split { axis: *axis, kids: kids.iter().map(|k| build(k, ids)).collect(), ratios: ratios.clone() }
                }
            }
        }
        build(n, ids).normalise()
    }

    /// Its ratios re-clamped to `area` (a restored layout in this window,
    /// architecture §5): a pane its share puts under its minimum gets it
    /// (and a pixel for the rounding, when there is room), the others
    /// keep their proportions of what is left; a split whose minimums
    /// need more than its room is left as it is (the window folds).
    pub(crate) fn clamped(&self, area: Rect) -> Node {
        let Node::Split { axis, kids, ratios } = self else { return self.clone() };
        let len = match axis {
            Axis::Row => area.w,
            Axis::Column => area.h,
        };
        let room = (len - DIVIDER * (kids.len() - 1) as f32).max(1.);
        let mins: Vec<f32> = kids.iter().map(|k| k.min_along(*axis)).collect();
        let mut out = self.clone();
        if mins.iter().sum::<f32>() <= room + 1e-3 {
            let pad = if mins.iter().map(|m| m + 1.).sum::<f32>() <= room { 1. } else { 0. };
            let mut held = vec![false; kids.len()];
            let shares = loop {
                let fixed: f32 = mins.iter().zip(&held).filter(|(_, h)| **h).map(|(m, _)| m + pad).sum();
                let free: f32 = ratios.iter().zip(&held).filter(|(_, h)| !**h).map(|(r, _)| r).sum();
                let rest = room - fixed;
                let share = |i: usize| if free > 0. { rest * ratios[i] / free } else { 0. };
                let mut raised = false;
                for i in 0..kids.len() {
                    if !held[i] && share(i) < mins[i] + pad {
                        held[i] = true;
                        raised = true;
                    }
                }
                if !raised || held.iter().all(|h| *h) {
                    break (0..kids.len()).map(|i| if held[i] { mins[i] + pad } else { share(i) } / room).collect::<Vec<f32>>();
                }
            };
            let sum: f32 = shares.iter().sum();
            if let Node::Split { ratios, .. } = &mut out {
                *ratios = shares.iter().map(|s| s / sum).collect();
            }
        }
        let mut subs = Vec::new();
        out.kid_rects(area, &mut subs);
        if let Node::Split { kids, .. } = &mut out {
            for (k, sub) in kids.iter_mut().zip(subs) {
                *k = k.clamped(sub);
            }
        }
        out
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Node::Split { kids, .. } => kids.get_mut(*i)?.at_mut(rest),
                Node::Leaf(_) => None,
            },
        }
    }

    /// Divider `d` dragged to `at` px from its split's start (split.md
    /// §4, A10): 1:1, the two kids beside it keeping their minimums (it
    /// stops there; it never pushes a third).
    pub(crate) fn drag(&mut self, d: &Divider, at: f32) {
        let len = match d.axis {
            Axis::Row => d.rect.w,
            Axis::Column => d.rect.h,
        };
        let Some(Node::Split { kids, ratios, axis }) = self.at_mut(&d.path) else { return };
        let i = d.i;
        if i + 1 >= kids.len() {
            return;
        }
        let room = (len - DIVIDER * (kids.len() - 1) as f32).max(1.);
        let Some((lo, hi)) = edge_bounds(kids, ratios, *axis, i, room) else { return };
        let a: f32 = ratios[..i].iter().sum();
        let b = a + ratios[i] + ratios[i + 1];
        let c = (at - DIVIDER * i as f32).clamp(lo, hi) / room;
        ratios[i] = c - a;
        ratios[i + 1] = b - c;
    }

    /// A drag let go (split.md §4): within [`DETENT`] px of the two kids
    /// beside it being equal, they are.
    pub(crate) fn detent(&mut self, d: &Divider) {
        let len = match d.axis {
            Axis::Row => d.rect.w,
            Axis::Column => d.rect.h,
        };
        let Some(Node::Split { kids, ratios, axis }) = self.at_mut(&d.path) else { return };
        // A divider gone (its split lost a kid): nothing (Shady #42 M2).
        if d.i + 1 >= ratios.len() {
            return;
        }
        let room = (len - DIVIDER * (kids.len() - 1) as f32).max(1.);
        let (r0, r1) = (ratios[d.i], ratios[d.i + 1]);
        if ((r0 - r1).abs() * room / 2.) <= DETENT {
            // No nearer equal than the drag's own stops (Shady #42 M3-b: a
            // kid stopped at a nested minimum stays there).
            let Some((lo, hi)) = edge_bounds(kids, ratios, *axis, d.i, room) else { return };
            let a: f32 = ratios[..d.i].iter().sum();
            let c = (room * (a + (r0 + r1) / 2.)).clamp(lo, hi) / room;
            (ratios[d.i], ratios[d.i + 1]) = (c - a, a + r0 + r1 - c);
        }
    }

    /// A double-click on a divider (split.md §4): its split's kids equal,
    /// as far as each keeps its minimum in `area` (Shady #42 M3-b: a kid
    /// whose own split needs more than an equal share keeps that, the
    /// others share the rest equally); none can be, the split stays.
    pub(crate) fn equalise(&mut self, path: &[usize], area: Rect) {
        let Some(rect) = self.dividers(area).into_iter().find(|d| d.path == path).map(|d| d.rect) else { return };
        let Some(Node::Split { kids, ratios, axis }) = self.at_mut(path) else { return };
        let len = match axis {
            Axis::Row => rect.w,
            Axis::Column => rect.h,
        };
        let room = (len - DIVIDER * (kids.len() - 1) as f32).max(1.);
        if let Some(even) = even_shares(kids, *axis, room) {
            *ratios = even;
        }
    }

    /// The palette's Equalise pane sizes: every split's kids equal as far
    /// as each keeps its minimum in `area` ([`Node::equalise`]), the
    /// outermost split first (Shady #44 M1: 1/n each folded a nested split
    /// at 1280, a pane under its minimum).
    pub(crate) fn equalise_all(&mut self, area: Rect) {
        let mut paths: Vec<Vec<usize>> = self.dividers(area).into_iter().map(|d| d.path).collect();
        paths.sort();
        paths.dedup();
        paths.sort_by_key(Vec::len);
        // Each from the dividers as laid out now: a nested split's box
        // follows its parent's new share.
        for path in paths {
            self.equalise(&path, area);
        }
    }

    /// The kid indices from the root down to leaf `id`.
    fn path_of(&self, id: PaneId) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        self.path_to(id, &mut path).then(|| path.iter().map(|(_, i)| *i).collect())
    }

    /// Where a split's flight starts (split.md §10): `id` (the new pane)
    /// at a share of 0, its share back with pane `at`, the one it split
    /// off, when that is beside it (a split left or up puts it before
    /// `at`), else with the pane before it in its split, so it grows from
    /// the split line.
    pub(crate) fn grown_beside(&self, id: PaneId, at: Option<PaneId>) -> Node {
        let mut n = self.clone();
        if let Some(path) = self.path_of(id)
            && let Some((&i, parent)) = path.split_last()
            && let Some(Node::Split { kids, ratios, .. }) = n.at_mut(parent)
        {
            let beside = at.and_then(|a| [i.wrapping_sub(1), i + 1].into_iter().find(|&j| kids.get(j) == Some(&Node::Leaf(a))));
            let j = beside.unwrap_or(if i > 0 { i - 1 } else { i + 1 });
            if j < ratios.len() {
                ratios[j] += ratios[i];
                ratios[i] = 0.;
            }
        }
        n
    }

    /// Where a close's flight lands (split.md §8): `id` (the pane closing)
    /// at a share of 0, the kids beside it in its split scaled up to take
    /// it, as [`Node::remove`] shares it out. Its split's other kids all
    /// closing too (at 0): that split goes to 0 in its own, and so on up.
    pub(crate) fn zeroed(&self, id: PaneId) -> Node {
        let mut n = self.clone();
        let Some(path) = self.path_of(id) else { return n };
        for k in (0..path.len()).rev() {
            let i = path[k];
            if let Some(Node::Split { ratios, .. }) = n.at_mut(&path[..k]) {
                let rest = 1. - ratios[i];
                if rest > 1e-6 {
                    for (j, r) in ratios.iter_mut().enumerate() {
                        *r = if j == i { 0. } else { *r / rest };
                    }
                    break;
                }
            }
        }
        n
    }

    /// Each split's ratios `t` of the way from `from`'s to this tree's
    /// (`t` 1: this tree itself). Not the same shape: this tree. Under 0
    /// (a flight turning with the velocity of the one it took over: back
    /// past `from` the way that one went), no share under 0.
    pub(crate) fn lerp_from(&self, from: &Node, t: f32) -> Node {
        if t >= 1. {
            return self.clone();
        }
        match (self, from) {
            (Node::Leaf(a), Node::Leaf(b)) if a == b => self.clone(),
            (Node::Split { axis, kids, ratios }, Node::Split { axis: a2, kids: k2, ratios: r2 })
                if axis == a2 && kids.len() == k2.len() =>
            {
                let kids = kids.iter().zip(k2).map(|(k, f)| k.lerp_from(f, t)).collect();
                let mut ratios: Vec<f32> = ratios.iter().zip(r2).map(|(to, fr)| fr + (to - fr) * t).collect();
                if t < 0. {
                    ratios.iter_mut().for_each(|r| *r = r.max(0.));
                    let sum: f32 = ratios.iter().sum();
                    ratios.iter_mut().for_each(|r| *r /= sum.max(f32::EPSILON));
                }
                Node::Split { axis: *axis, kids, ratios }
            }
            _ => self.clone(),
        }
    }

    /// The layout's step at `t` with the centre moving under it (#51):
    /// `from` laid out in `a` and this tree in `b`, every edge between
    /// panes at its place there lerped in pixels and on a whole one, as
    /// shares of the room each split is drawn in (`now`, `t` of the way
    /// from `a` to `b`). Each pane's edges move linearly between their
    /// ends, as the panel's does: a pane's width never goes past either.
    pub(crate) fn lerp_px_from(&self, from: &Node, t: f32, a: Rect, b: Rect, now: Rect) -> Node {
        if t >= 1. {
            return self.clone();
        }
        match (self, from) {
            (Node::Leaf(x), Node::Leaf(y)) if x == y => self.clone(),
            (Node::Split { axis, kids, .. }, Node::Split { axis: a2, kids: k2, .. }) if axis == a2 && kids.len() == k2.len() => {
                let (mut fa, mut tb) = (Vec::new(), Vec::new());
                from.kid_rects(a, &mut fa);
                self.kid_rects(b, &mut tb);
                // A kid's far edge, from the split's start.
                let row = *axis == Axis::Row;
                let end = |r: &Rect| if row { r.x + r.w } else { r.y + r.h };
                let (start, len) = if row { (now.x, now.w) } else { (now.y, now.h) };
                let room = (len - DIVIDER * (kids.len() - 1) as f32).max(0.);
                let mut at = 0f32;
                let mut ratios = Vec::with_capacity(kids.len());
                for i in 0..kids.len() {
                    let e = if i + 1 == kids.len() {
                        room
                    } else {
                        let (f, to) = (end(&fa[i]), end(&tb[i]));
                        ((f + (to - f) * t).round() - start - DIVIDER * i as f32).clamp(at, room)
                    };
                    ratios.push(if room > 0. { (e - at) / room } else { 1. / kids.len() as f32 });
                    at = e;
                }
                let step = Node::Split { axis: *axis, kids: kids.clone(), ratios: ratios.clone() };
                let mut nk = Vec::new();
                step.kid_rects(now, &mut nk);
                let kids = kids.iter().enumerate().map(|(i, k)| k.lerp_px_from(&k2[i], t, fa[i], tb[i], nk[i])).collect();
                Node::Split { axis: *axis, kids, ratios }
            }
            _ => self.clone(),
        }
    }

    /// How far a flight from `from` to this tree moves anything in `area`
    /// (the most any pane's edge travels, px): its spring's length.
    pub(crate) fn travel_from(&self, from: &Node, area: Rect) -> f32 {
        self.moves_from(from, area).iter().flat_map(|(_, d)| d.iter()).fold(0f32, |m, d| m.max(d.abs()))
    }

    /// How far each pane in both trees moves its edges in `area` (px: left,
    /// top, right, bottom), going from `from` to this tree.
    pub(crate) fn moves_from(&self, from: &Node, area: Rect) -> Vec<(PaneId, [f32; 4])> {
        let (a, b) = (from.rects(area), self.rects(area));
        b.iter()
            .filter_map(|(id, r)| {
                let (_, f) = a.iter().find(|(i, _)| i == id)?;
                Some((*id, [r.x - f.x, r.y - f.y, r.x + r.w - f.x - f.w, r.y + r.h - f.y - f.h]))
            })
            .collect()
    }

    /// The panes in reading order (split.md §1): top to bottom, then left
    /// to right, by each one's top-left corner. Alt+N and F6 use it.
    pub(crate) fn reading_order(&self, area: Rect) -> Vec<PaneId> {
        let mut r = self.rects(area);
        r.sort_by(|a, b| a.1.y.total_cmp(&b.1.y).then(a.1.x.total_cmp(&b.1.x)));
        r.into_iter().map(|(id, _)| id).collect()
    }

    /// Where focus goes when `id` closes (split.md §8): its left/up
    /// neighbour (the pane before it in the nearest split where it is not
    /// first); the top-left pane's is the one after it.
    pub(crate) fn neighbour(&self, id: PaneId) -> Option<PaneId> {
        let mut path = Vec::new();
        if !self.path_to(id, &mut path) {
            return None;
        }
        // The nearest split above it where it is not first: the pane just
        // before it there (left or up).
        for (node, i) in path.iter().rev() {
            if let Node::Split { kids, .. } = node
                && *i > 0
            {
                return kids[*i - 1].leaves().last().copied();
            }
        }
        // The top-left pane: the one after it.
        match path.last()? {
            (Node::Split { kids, .. }, i) => kids.get(i + 1)?.leaves().first().copied(),
            _ => None,
        }
    }

    /// The splits from the root down to `id`, each with the kid taken.
    fn path_to<'a>(&'a self, id: PaneId, out: &mut Vec<(&'a Node, usize)>) -> bool {
        match self {
            Node::Leaf(l) => *l == id,
            Node::Split { kids, .. } => {
                for (i, k) in kids.iter().enumerate() {
                    out.push((self, i));
                    if k.path_to(id, out) {
                        return true;
                    }
                    out.pop();
                }
                false
            }
        }
    }
}

/// How far kid `i`'s far edge goes in a split (px from its start, the
/// rules left out, `room` long): the kids either side of it keeping their
/// minimums, on whole pixels (as `kid_rects` rounds the edges: a pane at
/// its minimum is 320, not 319; Shady #42 M3, n1). `None`: they cannot
/// both.
fn edge_bounds(kids: &[Node], ratios: &[f32], axis: Axis, i: usize, room: f32) -> Option<(f32, f32)> {
    let a: f32 = ratios[..i].iter().sum();
    let b = a + ratios[i] + ratios[i + 1];
    // The pair's outer edges as laid out (the last kid's ends at `room`).
    let start = (room * a).round();
    let end = if i + 2 == ratios.len() { room } else { (room * b).round() };
    let (lo, hi) = ((start + kids[i].min_along(axis) - 1e-3).ceil(), (end - kids[i + 1].min_along(axis) + 1e-3).floor());
    (lo <= hi).then_some((lo, hi))
}

/// A split's kids' shares of `room` px, equal as far as each keeps its
/// minimum: a kid an equal share puts under it keeps its minimum, the
/// others share what is left equally (Shady #42 M3-b). `None`: their
/// minimums need more than `room`.
fn even_shares(kids: &[Node], axis: Axis, room: f32) -> Option<Vec<f32>> {
    let mins: Vec<f32> = kids.iter().map(|k| k.min_along(axis)).collect();
    if mins.iter().sum::<f32>() > room + 1e-3 {
        return None;
    }
    let mut held = vec![false; kids.len()];
    loop {
        let rest = room - mins.iter().zip(&held).filter(|(_, h)| **h).map(|(m, _)| m).sum::<f32>();
        let free = held.iter().filter(|h| !**h).count();
        if free == 0 {
            return None;
        }
        let each = rest / free as f32;
        let mut raised = false;
        for (h, m) in held.iter_mut().zip(&mins) {
            if !*h && *m > each {
                *h = true;
                raised = true;
            }
        }
        if !raised {
            return Some(mins.iter().zip(&held).map(|(m, h)| if *h { *m } else { each } / room).collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: (f32, f32) = (1600., 1000.);
    fn p(i: u8) -> PaneId {
        PaneId::nth(i)
    }
    fn area() -> Rect {
        Rect { x: 0., y: 0., w: A.0, h: A.1 }
    }

    /// Ctrl+\ beside a pane of a row adds a kid to the row (3 columns are
    /// one split of three, not nested); its share is halved.
    #[test]
    fn a_split_on_the_same_axis_flattens() {
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let t = t.split(p(1), Side::Right, p(2), A).unwrap();
        assert_eq!(
            t,
            Node::Split {
                axis: Axis::Row,
                kids: vec![Node::Leaf(p(0)), Node::Leaf(p(1)), Node::Leaf(p(2))],
                ratios: vec![0.5, 0.25, 0.25]
            }
        );
        let r = t.rects(area());
        assert_eq!(r.iter().map(|(_, r)| r.w).sum::<f32>() + 2. * DIVIDER, A.0, "the dividers and panes fill the row");
        assert!(r.iter().all(|(_, r)| r.w.fract() == 0. && r.x.fract() == 0.), "whole pixels: {r:?}");
    }

    /// The caps (split.md §0, the 2026-11-25 note): [`PANES_MAX`] panes at
    /// most, as many across or down as fit, and no pane under 320 × 240.
    #[test]
    fn the_caps_and_the_minimums_refuse() {
        let one = Node::Leaf(p(0));
        let two = one.split(p(0), Side::Down, p(1), A).unwrap();
        assert!(two.split(p(1), Side::Down, p(2), A).is_ok(), "a 3rd row: room for it");
        let three = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), (2000., 1000.)).unwrap();
        let three = three.split(p(0), Side::Right, p(2), (2000., 1000.)).unwrap();
        assert!(three.split(p(2), Side::Right, p(3), (4000., 1000.)).is_ok(), "a 4th column: room for it");
        let four = three.split(p(0), Side::Down, p(3), (2000., 1000.)).unwrap();
        assert_eq!(four.len(), 4);
        let mut nine = four;
        for i in 4..9 {
            nine = nine.split(p(i - 1), Side::Right, p(i), (8000., 2000.)).unwrap();
        }
        assert_eq!(nine.split(p(1), Side::Down, PaneId::ghost(0), (8000., 2000.)), Err(Refused::Most));
        // 640 px wide: two of 319.5 would be under 320.
        assert_eq!(one.split(p(0), Side::Right, p(1), (640., 1000.)), Err(Refused::Room));
        assert!(one.split(p(0), Side::Right, p(1), (641., 1000.)).is_ok());
        assert_eq!(one.split(p(0), Side::Down, p(1), (1000., 480.)), Err(Refused::Room));
    }

    /// Room is the only limit (split.md §0, the 2026-11-25 note): no cap
    /// across or down, only each pane's 320 × 240 and [`PANES_MAX`] in all.
    /// A pane halved under its minimum evens out the split it joins
    /// instead, when that fits.
    #[test]
    fn room_is_the_only_limit() {
        // 1920 with both panels at their strips.
        let a = (1856., 1040.);
        let area = Rect { x: 0., y: 0., w: a.0, h: a.1 };
        let mut t = Node::Leaf(p(0));
        for i in 1..5 {
            t = t.split(p(i - 1), Side::Right, p(i), a).unwrap_or_else(|e| panic!("pane {i} across: {e:?}"));
        }
        let r = t.rects(area);
        assert_eq!(r.len(), 5);
        assert!(r.iter().all(|(_, b)| b.y == 0. && b.h == a.1 && b.w >= PANE_MIN_W), "five across, each at least 320: {r:?}");
        // A sixth across would be 308: refused, and Ctrl+\ splits down.
        assert_eq!(t.split(p(4), Side::Right, p(5), a), Err(Refused::Room));
        let (t, side) = t.split_right(p(4), p(5), a).expect("down");
        assert_eq!(side, Side::Down);
        // Four down in a column where they fit (4 × 240 + 3 ≤ 1040).
        let t = t.split(p(5), Side::Down, p(6), a).expect("a third row");
        let t = t.split(p(6), Side::Down, p(7), a).expect("a fourth row, the column evened");
        assert!(t.rects(area).iter().all(|(_, b)| b.w >= PANE_MIN_W && b.h >= PANE_MIN_H), "{:?}", t.rects(area));
        assert_eq!(t.split(p(7), Side::Down, p(8), a), Err(Refused::Room), "a fifth row: 5 × 240 + 4 > 1040");
        let t = t.split(p(0), Side::Down, p(8), a).expect("the ninth");
        assert_eq!(t.len(), PANES_MAX);
        assert_eq!(PANES_MAX, 9);
        // The tenth: the total's cap, whatever the room.
        assert_eq!(t.split(p(1), Side::Down, PaneId::ghost(0), (8000., 4000.)), Err(Refused::Most));
        assert_eq!(Refused::Most.note(), "9 panes is the most");
    }

    /// Ctrl+\ on a pane too narrow for two splits it down (split.md §2).
    #[test]
    fn split_right_falls_back_to_down() {
        let (t, side) = Node::Leaf(p(0)).split_right(p(0), p(1), (600., 1000.)).unwrap();
        assert_eq!(side, Side::Down);
        assert!(matches!(t, Node::Split { axis: Axis::Column, .. }));
        assert_eq!(Node::Leaf(p(0)).split_right(p(0), p(1), (600., 400.)), Err(Refused::Room));
    }

    /// A close gives its share to the kids beside it, collapses a split
    /// left with one kid, and focus goes left/up (split.md §8).
    #[test]
    fn remove_normalises_and_the_neighbour_is_left_or_up() {
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let t = t.split(p(1), Side::Down, p(2), A).unwrap();
        assert_eq!(t.neighbour(p(2)), Some(p(1)), "up");
        assert_eq!(t.neighbour(p(1)), Some(p(0)), "first in its column: the pane to its left");
        assert_eq!(t.neighbour(p(0)), Some(p(1)), "the top-left pane: the one after it");
        let t2 = t.remove(p(2)).unwrap();
        assert_eq!(t2, Node::Split { axis: Axis::Row, kids: vec![Node::Leaf(p(0)), Node::Leaf(p(1))], ratios: vec![0.5, 0.5] });
        assert_eq!(t2.remove(p(0)).unwrap(), Node::Leaf(p(1)));
        assert_eq!(Node::Leaf(p(0)).remove(p(0)), None, "the last pane");
    }

    /// A divider follows the pointer 1:1, stops at the minimums of the two
    /// panes beside it, lands on equal within 8 px, and a double-click
    /// equalises its split (split.md §4, A10).
    #[test]
    fn a_divider_drags_stops_at_the_minimum_and_lands_on_equal() {
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let mut t = t.split(p(1), Side::Right, p(2), A).unwrap();
        let d = t.dividers(area());
        assert_eq!(d.len(), 2);
        let first = d[0].clone();
        let r = t.rects(area());
        assert_eq!(first.at, r[0].1.w, "right after the first pane");
        t.drag(&first, 600.);
        let r = t.rects(area());
        assert_eq!(r[0].1.w, 600., "1:1");
        assert_eq!(r[1].1.x, 601.);
        // Past the second pane's minimum: it stops there.
        t.drag(&first, 1200.);
        let r = t.rects(area());
        assert_eq!(r[1].1.w, PANE_MIN_W, "the next pane at its minimum: {r:?}");
        assert!(r[2].1.w >= PANE_MIN_W, "never pushes a third");
        t.drag(&first, 10.);
        assert_eq!(t.rects(area())[0].1.w, PANE_MIN_W);
        // Let go 6 px from equal with the pane beside it: equal.
        let d = t.dividers(area())[0].clone();
        let pair = t.rects(area())[0].1.w + t.rects(area())[1].1.w;
        t.drag(&d, (pair / 2.).round() + 6.);
        t.detent(&d);
        let r = t.rects(area());
        assert!((r[0].1.w - r[1].1.w).abs() <= 1., "on equal: {r:?}");
        t.drag(&d, 200. + PANE_MIN_W);
        t.equalise(&d.path, area());
        let r = t.rects(area());
        assert!(r.windows(2).all(|w| (w[0].1.w - w[1].1.w).abs() <= 1.), "equalised: {r:?}");
        // A nested split's divider: stacked, its own path.
        let t2 = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap().split(p(1), Side::Down, p(2), A).unwrap();
        let d2 = t2.dividers(area());
        assert_eq!(d2.len(), 2);
        assert_eq!((d2[1].path.clone(), d2[1].axis), (vec![1], Axis::Column));
    }

    /// A split's flight starts with the new pane at 0 beside the pane it
    /// split off (the rest where they were) and lands on the tree; a
    /// close's lands with the pane at 0 and its share with the kids beside
    /// it, as `remove` gives it them; each step is on whole pixels (split.md
    /// §10).
    #[test]
    fn a_flight_grows_from_the_split_line_and_a_close_lands_as_remove() {
        let two = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let three = two.split(p(1), Side::Down, p(2), A).unwrap();
        let from = three.grown_beside(p(2), None);
        let (r0, r1) = (two.rects(area()), from.rects(area()));
        let at = |rs: &[(PaneId, Rect)], id| rs.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!(at(&r1, p(0)), at(&r0, p(0)), "the left pane holds still");
        assert_eq!(at(&r1, p(2)).h, 0., "the new pane from 0");
        assert_eq!(at(&r1, p(1)).h, at(&r0, p(1)).h - DIVIDER, "the pane it split off keeps its share, less the rule");
        assert_eq!(at(&r1, p(2)).y, at(&r1, p(1)).y + at(&r1, p(1)).h + DIVIDER, "at the split line");
        assert_eq!(three.lerp_from(&from, 0.).rects(area()), r1);
        assert_eq!(three.lerp_from(&from, 1.), three, "lands on the tree");
        for k in 0..=20 {
            let r = three.lerp_from(&from, k as f32 / 20.).rects(area());
            assert!(r.iter().all(|(_, r)| [r.x, r.y, r.w, r.h].iter().all(|v| v.fract() == 0.)), "whole pixels at {k}: {r:?}");
        }
        assert_eq!(three.travel_from(&from, area()), at(&three.rects(area()), p(2)).h, "its travel: the new pane's height");
        // A close: the pane at 0, its share to the one beside it.
        let to = three.zeroed(p(2));
        let gone = three.remove(p(2)).unwrap();
        let (rt, rg) = (to.rects(area()), gone.rects(area()));
        assert_eq!(at(&rt, p(2)).h, 0.);
        assert_eq!(at(&rt, p(0)), at(&rg, p(0)));
        assert_eq!(at(&rt, p(1)).h + DIVIDER, at(&rg, p(1)).h, "all but the rule the landed tree drops");
        // Three across, the middle one closing: both sides take it, as remove shares it.
        let across = two.split(p(1), Side::Right, p(2), A).unwrap();
        let rt = across.zeroed(p(1)).rects(area());
        let rg = across.remove(p(1)).unwrap().rects(area());
        assert!((at(&rt, p(0)).w - at(&rg, p(0)).w).abs() <= 1. && (at(&rt, p(2)).w - at(&rg, p(2)).w).abs() <= 1., "{rt:?} / {rg:?}");
    }

    /// Shady #42 M3: `[A A′ / B | C]`, A′ dragged to its minimum, then
    /// the root's divider dragged as far as it goes: every pane keeps 320
    /// (a kid's minimum is its own split's at its shares, not its count's);
    /// and the stacked twin keeps 240.
    #[test]
    fn a_nested_split_keeps_every_panes_minimum() {
        let area = Rect { x: 0., y: 0., w: 1920., h: 1080. };
        let a = (1920., 1080.);
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(2), a).unwrap();
        let t = t.split(p(0), Side::Down, p(1), a).unwrap();
        let mut t = t.split(p(0), Side::Right, p(3), a).unwrap();
        let ds = t.dividers(area);
        let inner = ds.iter().find(|d| d.path.len() == 2).expect("A | A′").clone();
        t.drag(&inner, 2000.);
        let at = |t: &Node, id| t.rects(area).into_iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!(at(&t, p(3)).w, PANE_MIN_W, "A′ at its minimum");
        let root = t.dividers(area).into_iter().find(|d| d.path.is_empty()).expect("the root's");
        t.drag(&root, 0.);
        let r = t.rects(area);
        assert!(r.iter().all(|(_, r)| r.w >= PANE_MIN_W), "every pane keeps 320: {r:?}");
        assert_eq!(at(&t, p(3)).w, PANE_MIN_W, "A′ stops it: {r:?}");
        // Stacked: `[A / A′ | B] / C`, laid out by hand (as when three
        // down was past the old cap): the minimum holds all the same.
        let t = Node::Leaf(p(0)).split(p(0), Side::Down, p(2), a).unwrap();
        let t = t.split(p(0), Side::Right, p(1), a).unwrap();
        let mut t = t.split_as(p(0), Side::Down, p(3)).unwrap();
        let inner = t.dividers(area).into_iter().find(|d| d.path.len() == 2).expect("A / A′");
        t.drag(&inner, 2000.);
        assert_eq!(at(&t, p(3)).h, PANE_MIN_H, "A′ at its minimum");
        let root = t.dividers(area).into_iter().find(|d| d.path.is_empty()).expect("the root's");
        t.drag(&root, 0.);
        let r = t.rects(area);
        assert!(r.iter().all(|(_, r)| r.h >= PANE_MIN_H), "every pane keeps 240: {r:?}");
    }

    /// Shady #42 M3-b: `[A A′ / B | C]`, C and A′ at their minimums: a
    /// double-click on the root's divider, or a detent near equal, keeps
    /// every pane at 320 or more (equal as far as A′ lets it); a split
    /// with room equalises as before.
    #[test]
    fn equalise_and_the_detent_keep_the_nested_minimums() {
        let area = Rect { x: 0., y: 0., w: 1600., h: 1000. };
        let a = (1600., 1000.);
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(2), a).unwrap();
        let t = t.split(p(0), Side::Down, p(1), a).unwrap();
        let mut t = t.split(p(0), Side::Right, p(3), a).unwrap();
        let root = |t: &Node| t.dividers(area).into_iter().find(|d| d.path.is_empty()).expect("the root's");
        let inner = |t: &Node| t.dividers(area).into_iter().find(|d| d.path.len() == 2).expect("A | A′");
        let at = |t: &Node, id| t.rects(area).into_iter().find(|(i, _)| *i == id).unwrap().1;
        t.drag(&root(&t), 2000.);
        assert_eq!(at(&t, p(2)).w, PANE_MIN_W, "C at its minimum");
        t.drag(&inner(&t), 2000.);
        assert_eq!(at(&t, p(3)).w, PANE_MIN_W, "A′ at its minimum");
        let mut eq = t.clone();
        eq.equalise(&[], area);
        let r = eq.rects(area);
        assert!(r.iter().all(|(_, r)| r.w >= PANE_MIN_W), "a double-click keeps every pane at 320: {r:?}");
        // Room for some of it: C keeps 320, the left side as near equal
        // as A′ lets it.
        let mut t = t.clone();
        t.drag(&inner(&t), 900.);
        let mut eq = t.clone();
        eq.equalise(&[], area);
        let r = eq.rects(area);
        assert!(r.iter().all(|(_, r)| r.w >= PANE_MIN_W), "every pane keeps 320: {r:?}");
        assert!(at(&eq, p(2)).w > at(&t, p(2)).w, "C took what the left side could give: {r:?}");
        // The detent: the root's divider stopped by A′'s minimum 7.5 px
        // from equal, let go: it stays at the stop (equal put A′ at 317).
        let fresh = Node::Leaf(p(0)).split(p(0), Side::Right, p(2), a).unwrap().split(p(0), Side::Down, p(1), a).unwrap();
        let mut t = fresh.split(p(0), Side::Right, p(3), a).unwrap();
        t.drag(&root(&t), 1000.);
        t.drag(&inner(&t), 602.);
        assert_eq!(at(&t, p(3)).w, 397.);
        t.drag(&root(&t), 0.);
        assert_eq!(at(&t, p(3)).w, PANE_MIN_W, "the drag stops at A′'s minimum: {:?}", t.rects(area));
        t.detent(&root(&t));
        let r = t.rects(area);
        assert!(r.iter().all(|(_, r)| r.w >= PANE_MIN_W), "the detent keeps A′ at 320: {r:?}");
        // With room, both as before: equal.
        let mut two = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), a).unwrap();
        two.drag(&two.dividers(area)[0].clone(), 300.);
        two.equalise(&[], area);
        let r = two.rects(area);
        assert!((r[0].1.w - r[1].1.w).abs() <= 1., "equal: {r:?}");
    }

    /// Shady #42 M2: a divider whose split lost a kid (a pane closed under
    /// the drag) lets go of nothing: no panic, the tree as it was.
    #[test]
    fn a_detent_on_a_divider_gone_does_nothing() {
        let three = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap().split(p(1), Side::Right, p(2), A).unwrap();
        let d = three.dividers(area()).into_iter().find(|d| d.i == 1).expect("the second divider");
        let mut two = three.remove(p(2)).unwrap();
        let was = two.clone();
        two.detent(&d);
        two.drag(&d, 300.);
        assert_eq!(two, was);
    }

    /// Shady #42 M1: a flight's end with a pane still closing in it (at 0)
    /// lays the others out as the tree without it does, to the rule; a
    /// pane whose split's others are all closing takes its split to 0.
    #[test]
    fn a_closing_pane_at_0_lays_out_as_the_tree_without_it() {
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap().split(p(1), Side::Down, p(2), A).unwrap();
        let to = t.zeroed(p(2));
        let gone = t.remove(p(2)).unwrap();
        let at = |rs: &[(PaneId, Rect)], id| rs.iter().find(|(i, _)| *i == id).unwrap().1;
        let (rt, rg) = (to.rects(area()), gone.rects(area()));
        assert!((at(&rt, p(1)).h - at(&rg, p(1)).h).abs() <= DIVIDER);
        // Then p(1) closes too: its column (p(2) at 0 in it) goes to 0.
        let to2 = to.zeroed(p(1));
        let r = to2.rects(area());
        assert_eq!((at(&r, p(1)).w, at(&r, p(2)).w), (0., 0.), "{r:?}");
        assert!((at(&r, p(0)).w - (A.0 - DIVIDER)).abs() <= 0.5, "{r:?}");
        assert!(to2.same_shape(&t) && !to2.same_shape(&gone));
        // A split beside a closing pane, on both ends of a flight: the same shape.
        let (a, b) = (to.split_as(p(0), Side::Down, p(3)).unwrap(), t.split_as(p(0), Side::Down, p(3)).unwrap());
        assert!(a.same_shape(&b));
    }

    /// Reading order: top to bottom, then left to right (split.md §1).
    #[test]
    fn reading_order_is_rows_then_columns() {
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let t = t.split(p(0), Side::Down, p(2), A).unwrap();
        let t = t.split(p(1), Side::Down, p(3), A).unwrap();
        assert_eq!(t.reading_order(area()), vec![p(0), p(1), p(2), p(3)]);
    }

    /// Step 4's drops on a left or a top edge: the new pane before the one
    /// it splits (beside it in its split on that axis, else the two in a
    /// new split), its share from that pane, and grown from 0 there.
    /// split/architecture.md §5: a layout saved in a wide window, restored
    /// in a narrower one, keeps every pane at its minimum (the others in
    /// proportion); one whose minimums cannot fit is left to the fold; a
    /// tree round-trips through its saved form.
    #[test]
    fn a_restored_layout_is_reclamped_to_the_window() {
        let t = Node::Split {
            axis: Axis::Row,
            kids: vec![
                Node::Leaf(p(0)),
                Node::Split { axis: Axis::Column, kids: vec![Node::Leaf(p(1)), Node::Leaf(p(2))], ratios: vec![0.1, 0.9] },
                Node::Leaf(p(3)),
            ],
            ratios: vec![0.15, 0.6, 0.25],
        };
        let order = [p(0), p(1), p(2), p(3)];
        assert_eq!(Node::from_saved(&t.to_saved(&order), &order), t, "round trip");
        let ids = [p(3), p(2), p(1), p(0)];
        assert_eq!(Node::from_saved(&t.to_saved(&order), &ids).leaves(), ids, "named as given");
        for (w, h) in [(1100., 700.), (1600., 1000.), (2400., 1200.)] {
            let area = Rect { x: 0., y: 0., w, h };
            let c = t.clamped(area);
            assert!(c.same_shape(&t));
            let r = c.rects(area);
            assert!(r.iter().all(|(_, r)| r.w >= PANE_MIN_W && r.h >= PANE_MIN_H), "{w}×{h}: {r:?}");
            // The rest in proportion: pane 1 (0.6 of the row) stays wider than pane 3 (0.25).
            let at = |id: PaneId| r.iter().find(|(i, _)| *i == id).unwrap().1;
            assert!(at(p(1)).w > at(p(3)).w, "{w}: {r:?}");
            if w >= 2400. {
                assert_eq!(c.clamped(area), c, "already whole: unchanged");
            }
        }
        // No room for three across: left to the fold.
        let area = Rect { x: 0., y: 0., w: 900., h: 700. };
        if let (Node::Split { ratios: a, .. }, Node::Split { ratios: b, .. }) = (&t.clamped(area), &t) {
            assert_eq!(a, b, "left as it was");
        }
    }

    #[test]
    fn a_split_left_or_up_puts_the_new_pane_first() {
        let two = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let three = two.split(p(1), Side::Left, p(2), A).unwrap();
        assert_eq!(
            three,
            Node::Split {
                axis: Axis::Row,
                kids: vec![Node::Leaf(p(0)), Node::Leaf(p(2)), Node::Leaf(p(1))],
                ratios: vec![0.5, 0.25, 0.25]
            }
        );
        let grown = three.grown_beside(p(2), Some(p(1)));
        assert_eq!(
            grown,
            Node::Split { axis: Axis::Row, kids: vec![Node::Leaf(p(0)), Node::Leaf(p(2)), Node::Leaf(p(1))], ratios: vec![0.5, 0., 0.5] },
            "from the pane it split"
        );
        assert_eq!(
            three.grown_beside(p(2), None),
            Node::Split { axis: Axis::Row, kids: vec![Node::Leaf(p(0)), Node::Leaf(p(2)), Node::Leaf(p(1))], ratios: vec![0.75, 0., 0.25] }
        );
        let up = two.split(p(0), Side::Up, p(2), A).unwrap();
        let col = Node::Split { axis: Axis::Column, kids: vec![Node::Leaf(p(2)), Node::Leaf(p(0))], ratios: vec![0.5, 0.5] };
        assert_eq!(up, Node::Split { axis: Axis::Row, kids: vec![col, Node::Leaf(p(1))], ratios: vec![0.5, 0.5] });
        assert_eq!(up.reading_order(area()), vec![p(2), p(1), p(0)]);
        assert_eq!(Node::Leaf(p(0)).split(p(0), Side::Left, p(1), (640., 1000.)), Err(Refused::Room), "the minimums hold on the left too");
    }

    /// Shady #62 M2, as a property: a side panel moving one way moves
    /// every divider one way, at any panel width, scale, window and split.
    /// The panels on the device grid (the rail's spring, the right panel
    /// fixed, or yielding tied to it: `chrome::Tie`), the centre in device
    /// pixels and laid out from the rail's edge (`AppView::render`: the
    /// centre at `MARK + rail`, [`Node::rects_in`]). Rounded from the
    /// centre's own edge (`rects`), 1500 px at 2× with 480/240 panels steps
    /// back half a pixel: rail 476.5 → 477, the first pane 495 → 494.
    #[test]
    fn a_moving_panel_moves_every_divider_one_way() {
        use crate::{app::STRIP, chrome::MARK};
        let trees = [vec![0.5, 0.5], vec![0.3, 0.7], vec![0.62, 0.38], vec![1. / 3., 1. / 3., 1. / 3.]];
        let mut fails = Vec::new();
        let mut cases = 0;
        for s in [1., 1.25, 1.5, 1.75, 2.] {
            let snap = |v: f32| (v * s).round() / s;
            let dp = |v: f32| (v * s).round();
            for win in [1280., 1500., 1700., 1920., 2560.] {
                for rail in (220..=480).step_by(20).chain([221, 333, 479, 481]) {
                    for right in (240..=520).step_by(40).chain([241, 519]) {
                        for tied in [false, true] {
                            for ratios in &trees {
                                let n = ratios.len();
                                let kids = (0..n).map(|i| Node::Leaf(p(i as u8))).collect();
                                let tree = Node::Split { axis: Axis::Row, kids, ratios: ratios.clone() };
                                let (rail_to, right_from, right_to) =
                                    (snap(rail as f32), snap(right as f32), if tied { STRIP } else { snap(right as f32) });
                                // An unfold, frame by frame (any one-way path:
                                // the spring's ease, sampled at 144 Hz).
                                let steps = 90;
                                let mut xs: Vec<Vec<f32>> = Vec::new();
                                for k in 0..=steps {
                                    let e = 1. - (1. - k as f32 / steps as f32).powi(3);
                                    let r = snap(STRIP + (rail_to - STRIP) * e);
                                    let q = if tied {
                                        snap(right_from + (right_to - right_from) * (r - STRIP) / (rail_to - STRIP))
                                    } else {
                                        right_from
                                    };
                                    let centre = ((dp(win) - dp(MARK) - dp(r) - dp(q)) / s).max(0.);
                                    let origin = (MARK + r, 52.);
                                    let rects = tree.rects_in(Rect { x: 0., y: 0., w: centre, h: 900. }, origin);
                                    xs.push(rects.iter().take(n - 1).map(|(_, b)| origin.0 + b.x + b.w).collect());
                                }
                                cases += 1;
                                for d in 0..n - 1 {
                                    let path: Vec<f32> = xs.iter().map(|x| x[d]).collect();
                                    let dir = (path[path.len() - 1] - path[0]).signum();
                                    if let Some(w) = path.windows(2).find(|w| (w[1] - w[0]) * dir < -1e-3)
                                        && fails.len() < 12
                                    {
                                        fails.push(format!(
                                            "{s}× {win} px, rail {rail} right {right} (tied {tied}) {ratios:?}: divider {d} {} -> {}",
                                            w[0], w[1]
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(cases > 10_000, "{cases} cases");
        assert!(fails.is_empty(), "{fails:#?}");
    }

    /// Shady #62 M1: a split's close rides its panel turned back, and the
    /// panel first goes on past where the close came (its velocity), so
    /// the ride's progress goes under 0, as far as it overshoots: an early
    /// close's way back is a few px and the overshoot several times that.
    /// Held only by each edge's bounds ([`Node::lerp_px_from`]): the new
    /// pane `d` px wide as drawn when the centre was `from`, closing at
    /// `from - back`, the centre going on to `from + 400`, with the new
    /// pane on the right or the left. Every pane at least 0 wide, in its
    /// room, in order; the closing pane where it was drawn at `from` and
    /// moving with the centre (it was held while the progress stood at -1).
    /// Through the ride's frame as the app draws it ([`Flight::ride_frame`],
    /// where the progress was clamped to −1: Shady #62 r3).
    #[test]
    fn a_ride_past_its_start_keeps_every_pane_in_its_room() {
        let h = 1000.;
        let at = |w: f32| Rect { x: 0., y: 0., w, h };
        let mut fails = Vec::new();
        for left in [false, true] {
            let kids = if left { vec![Node::Leaf(p(1)), Node::Leaf(p(0))] } else { vec![Node::Leaf(p(0)), Node::Leaf(p(1))] };
            let shares = |new: f32| if left { vec![new, 1. - new] } else { vec![1. - new, new] };
            let to = Node::Split { axis: Axis::Row, kids: kids.clone(), ratios: shares(0.) };
            for (d, back) in [(1., 1.), (3., 3.), (9., 9.), (9., 2.), (40., 10.), (200., 50.)] {
                let from = 930.;
                let drawn = Node::Split { axis: Axis::Row, kids: kids.clone(), ratios: shares(d / (from - DIVIDER)) };
                let flight = Flight::new(drawn, to.clone(), Vec::new(), vec![p(1)]);
                let ride = Ride { from, to: from - back, from_x: 0. };
                let mut last: Option<f32> = None;
                for n in 0..=57 {
                    let w = from + 7. * n as f32;
                    let t = (w - from) / (-back);
                    let step = flight.ride_frame(ride, at(w));
                    let rects = step.rects(at(w));
                    let what = format!("{} pane {d} px, back {back}, centre {w} (t {t})", if left { "a left" } else { "a right" });
                    let new = rects.iter().find(|(id, _)| *id == p(1)).map(|(_, r)| r.w).expect("the closing pane");
                    if rects.iter().any(|(_, r)| r.w < -0.01 || r.x < -0.01 || r.x + r.w > w + 0.01) {
                        fails.push(format!("{what}: out of its room: {rects:?}"));
                    }
                    if rects.windows(2).any(|k| k[1].1.x + 0.01 < k[0].1.x + k[0].1.w) {
                        fails.push(format!("{what}: out of order: {rects:?}"));
                    }
                    if n == 0 && (new - d).abs() > 1. {
                        fails.push(format!("{what}: not where it was drawn: {new}"));
                    }
                    if let Some(l) = last
                        && new + 0.01 < l
                    {
                        fails.push(format!("{what}: the closing pane turned before its panel: {l} -> {new}"));
                    }
                    if n == 8 && last.is_some_and(|l| (new - l).abs() < 0.01) && new + 1. < w - DIVIDER {
                        fails.push(format!("{what}: held while the centre moved: {new}"));
                    }
                    last = Some(new);
                }
            }
        }
        assert!(fails.is_empty(), "{fails:#?}");
    }
}
