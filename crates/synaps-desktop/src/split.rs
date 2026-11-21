//! Split panes, step 3 (split/split.md §1, §4; split/architecture.md §2.1):
//! the layout tree. VS Code's grid: n-ary and normalised, at most
//! [`PANES_MAX`] panes, at most 3 across and 2 down, each at least
//! [`PANE_MIN_W`] × [`PANE_MIN_H`].

use crate::pane::{PANES_MAX, PaneId};

/// The narrowest a pane gets (split.md §4): the tight layout's floor.
pub(crate) const PANE_MIN_W: f32 = 320.;
/// The shortest a pane gets (split.md §4): its tab strip, a cramped
/// composer and some rows.
pub(crate) const PANE_MIN_H: f32 = 240.;
/// The rule between two panes (split.md §4: a 1 px `hair` rule).
pub(crate) const DIVIDER: f32 = 1.;
/// The most panes side by side, and stacked (split.md §0).
pub(crate) const ACROSS_MAX: usize = 3;
pub(crate) const DOWN_MAX: usize = 2;

/// Side by side (`Row`) or stacked (`Column`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    Row,
    Column,
}

/// Where a new pane goes beside another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Right,
    Down,
}

/// Why a split did not happen (the composer's status line says it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// [`PANES_MAX`] panes already.
    Most,
    /// No room: the pane would go under its minimum, or a 4th column or a
    /// 3rd row.
    Room,
}

impl Refused {
    pub(crate) const fn note(self) -> &'static str {
        match self {
            Refused::Most => "4 panes is the most",
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
    fn extent(&self) -> (usize, usize) {
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
        let axis = match side {
            Side::Right => Axis::Row,
            Side::Down => Axis::Column,
        };
        match self {
            Node::Leaf(id) if *id == at => Some(Node::Split { axis, kids: vec![Node::Leaf(at), Node::Leaf(new)], ratios: vec![0.5, 0.5] }),
            Node::Leaf(_) => None,
            Node::Split { axis: a, kids, ratios } => {
                // Beside a leaf of this split, on its axis: one more kid.
                if *a == axis
                    && let Some(i) = kids.iter().position(|k| *k == Node::Leaf(at))
                {
                    let (mut kids, mut ratios) = (kids.clone(), ratios.clone());
                    let half = ratios[i] / 2.;
                    ratios[i] = half;
                    kids.insert(i + 1, Node::Leaf(new));
                    ratios.insert(i + 1, half);
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
    /// of `area` (the caps and the minimums of every pane).
    pub(crate) fn split(&self, at: PaneId, side: Side, new: PaneId, area: (f32, f32)) -> Result<Node, Refused> {
        if self.len() >= PANES_MAX {
            return Err(Refused::Most);
        }
        let next = self.with_split(at, side, new).ok_or(Refused::Room)?;
        let (across, down) = next.extent();
        if across > ACROSS_MAX || down > DOWN_MAX {
            return Err(Refused::Room);
        }
        let rects = next.rects(Rect { x: 0., y: 0., w: area.0, h: area.1 });
        if rects.iter().any(|(_, r)| r.w + 0.5 < PANE_MIN_W || r.h + 0.5 < PANE_MIN_H) {
            return Err(Refused::Room);
        }
        Ok(next)
    }

    /// Ctrl+\ (split.md §2): right, else down if the pane is too narrow
    /// for two; why not if neither fits.
    pub(crate) fn split_right(&self, at: PaneId, new: PaneId, area: (f32, f32)) -> Result<(Node, Side), Refused> {
        match self.split(at, Side::Right, new, area) {
            Ok(n) => Ok((n, Side::Right)),
            Err(Refused::Most) => Err(Refused::Most),
            Err(Refused::Room) => self.split(at, Side::Down, new, area).map(|n| (n, Side::Down)),
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
    /// out, every edge on a whole pixel.
    pub(crate) fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.place(area, &mut out);
        out
    }

    fn place(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split { axis, kids, ratios } => {
                let len = match axis {
                    Axis::Row => area.w,
                    Axis::Column => area.h,
                };
                let room = (len - DIVIDER * (kids.len() - 1) as f32).max(0.);
                let mut at = 0f32;
                let mut acc = 0f32;
                for (i, (kid, r)) in kids.iter().zip(ratios).enumerate() {
                    acc += r;
                    let end = if i + 1 == kids.len() { room } else { (room * acc).round() };
                    let size = (end - at).max(0.);
                    let off = at + DIVIDER * i as f32;
                    let sub = match axis {
                        Axis::Row => Rect { x: area.x + off, y: area.y, w: size, h: area.h },
                        Axis::Column => Rect { x: area.x, y: area.y + off, w: area.w, h: size },
                    };
                    kid.place(sub, out);
                    at = end;
                }
            }
        }
    }

    /// The panes in reading order (split.md §1): top to bottom, then left
    /// to right, by each one's top-left corner. Alt+N and F6 use it.
    pub(crate) fn reading_order(&self, area: Rect) -> Vec<PaneId> {
        let mut r = self.rects(area);
        r.sort_by(|a, b| a.1.y.total_cmp(&b.1.y).then(a.1.x.total_cmp(&b.1.x)));
        r.into_iter().map(|(id, _)| id).collect()
    }

    /// Where focus goes when `id` closes (split.md §8): the pane before it
    /// in its split (left or up), else the one after it.
    pub(crate) fn neighbour(&self, id: PaneId) -> Option<PaneId> {
        match self {
            Node::Leaf(_) => None,
            Node::Split { kids, .. } => {
                if let Some(i) = kids.iter().position(|k| *k == Node::Leaf(id)) {
                    let k = if i > 0 { &kids[i - 1] } else { kids.get(i + 1)? };
                    // The nearest pane of that kid: its last if before, its first if after.
                    let leaves = k.leaves();
                    return if i > 0 { leaves.last().copied() } else { leaves.first().copied() };
                }
                kids.iter().find_map(|k| k.neighbour(id))
            }
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

    /// The caps (split.md §0): 4 panes at most, 3 across, 2 down, and no
    /// pane under 320 × 240.
    #[test]
    fn the_caps_and_the_minimums_refuse() {
        let one = Node::Leaf(p(0));
        let two = one.split(p(0), Side::Down, p(1), A).unwrap();
        assert_eq!(two.split(p(1), Side::Down, p(2), A), Err(Refused::Room), "a 3rd row");
        let three = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), (2000., 1000.)).unwrap();
        let three = three.split(p(0), Side::Right, p(2), (2000., 1000.)).unwrap();
        assert_eq!(three.split(p(2), Side::Right, p(3), (4000., 1000.)), Err(Refused::Room), "a 4th column");
        let four = three.split(p(0), Side::Down, p(3), (2000., 1000.)).unwrap();
        assert_eq!(four.len(), 4);
        assert_eq!(four.split(p(1), Side::Down, PaneId::nth(3), (2000., 1000.)), Err(Refused::Most));
        // 640 px wide: two of 319.5 would be under 320.
        assert_eq!(one.split(p(0), Side::Right, p(1), (640., 1000.)), Err(Refused::Room));
        assert!(one.split(p(0), Side::Right, p(1), (641., 1000.)).is_ok());
        assert_eq!(one.split(p(0), Side::Down, p(1), (1000., 480.)), Err(Refused::Room));
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
        assert_eq!(t.neighbour(p(1)), Some(p(2)), "the first of its split: the one after");
        assert_eq!(t.neighbour(p(0)), Some(p(1)));
        let t2 = t.remove(p(2)).unwrap();
        assert_eq!(t2, Node::Split { axis: Axis::Row, kids: vec![Node::Leaf(p(0)), Node::Leaf(p(1))], ratios: vec![0.5, 0.5] });
        assert_eq!(t2.remove(p(0)).unwrap(), Node::Leaf(p(1)));
        assert_eq!(Node::Leaf(p(0)).remove(p(0)), None, "the last pane");
    }

    /// Reading order: top to bottom, then left to right (split.md §1).
    #[test]
    fn reading_order_is_rows_then_columns() {
        let t = Node::Leaf(p(0)).split(p(0), Side::Right, p(1), A).unwrap();
        let t = t.split(p(0), Side::Down, p(2), A).unwrap();
        let t = t.split(p(1), Side::Down, p(3), A).unwrap();
        assert_eq!(t.reading_order(area()), vec![p(0), p(1), p(2), p(3)]);
    }
}
