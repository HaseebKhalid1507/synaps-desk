//! The pointer lease and painted targets in the transcript (synaps-cards
//! `docs/blocks.md` v1.3, §6.2), for the blocks the agent changes.
//!
//! - **The lease** starts when the pointer moves over the list, and lasts
//!   while it moves and [`LEASE`] after its last move, or while a press is
//!   held. A parked pointer releases it (the weaker promise, stated: a
//!   mouse left over the transcript freezes nothing).
//! - **Under it, a board that would move or replace what is under the
//!   pointer waits** (the old geometry stays drawn): the hovered block
//!   removed, moved, merged or replaced (another incarnation is another
//!   block), its own height changing, or a block row in view above it
//!   added, removed or changing height. Over a group, its members are
//!   targets too: one replaced, removed, regrouped or moved within it (its
//!   incarnations and places), and the member under the pointer (its own
//!   painted target: [`Lease::member`]) gone, in another group or changing
//!   height. A change that keeps all that draws at once, in place (a tick:
//!   the same incarnations). The board is one coherent board: all of it waits,
//!   or none (decision: rather than scroll the content above, §6.2's
//!   "position" rule waits too, the calmer of the two; listed in the PR).
//! - **Release**: the lease's end (one timer wake, armed only while a
//!   board waits), the pointer leaving the list, or a key. What waited
//!   draws at once; what it moved or replaced counts as just moved.
//! - **Painted targets**: every action's hit region is painted as its
//!   block row and its incarnation's tag ([`Press`]): a block's fold, a
//!   group member's fold (the member's own row and tag, and the group it
//!   was painted in), a note's `more`. Pointer-down takes the innermost
//!   target, and the press holds the lease; pointer-up acts on that same
//!   target only if the bound board still holds it (the same tag, a member
//!   in the same group), else it does nothing and the block says `this
//!   block changed · look again` until the next input. It never redirects
//!   to whatever holds the name now. A release anywhere else (another
//!   block, blank space, another pane, outside the window), a move with no
//!   button down (the release was lost) or the window losing focus ends
//!   the press with no action ([`AppView::lease_unpress`]).
//! - **Just moved**: a pointer-down on a target an agent op moved (its
//!   painted rect) or replaced (its tag) in the last [`JUST_MOVED`] does
//!   nothing, once, and says `this moved just now`. "An agent op" is the
//!   pane's frame that draws a board published (its number, set as the
//!   board lands: [`Lease::agent_frame`]) and the frames of the slides it
//!   started ([`SlideY`]): each restarts the window, so it runs from the
//!   motion's end. A user's own change in another frame (a fold, `more`,
//!   a scroll, a resize) never starts it, however soon after a board; a
//!   frame that draws both counts as the agent's (the safe side). A
//!   timestamp per hit region, compared when input arrives: no timer, no
//!   frame. Keys never meet it.
//! - **The pill**: blocks that changed above the view, `2 blocks changed
//!   above ↑` at the list's top; a click walks to the first, and a block
//!   painted in view leaves it.
//! - **A move within view slides** (§6.3, Full): a block row an agent op
//!   moved starts where it was painted and settles on its new place
//!   ([`SlideY`]: the desk's row spring, capped at [`motion::SLIDE_CAP`]).
//!   Only while the list is anchored: following its tail, or scrolled
//!   since the last frame, a row's place on screen says nothing about a
//!   move, so it moves at once (decision, listed in the PR). Reduced and
//!   Off: at once.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use synaps_client::blocks::{Body, Drawn, Fate, RowBlocks, Shown, Tag};

use crate::app::AppView;
use crate::live::LiveId;
use crate::motion;
use crate::pane::PaneId;

/// The lease lasts this long after the pointer's last move.
pub(crate) const LEASE: Duration = Duration::from_millis(1500);
/// A target an agent op moved or replaced this recently does nothing, once.
pub(crate) const JUST_MOVED: Duration = Duration::from_millis(600);

pub(crate) const CHANGED: &str = "this block changed · look again";
pub(crate) const MOVED: &str = "this moved just now";

/// What a block's hit region was painted as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub row: u64,
    pub tag: Tag,
}

/// What a press acts on (§6.2: one rule for every action).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    /// The block's local fold.
    Fold,
    /// A note's `more` / `less`.
    More,
    /// A header's hover action (D7, [`crate::walk::Hover`]).
    Hover(crate::walk::Hover),
}

/// A press's painted target: the block row it was painted as (a group's
/// member is its own row), its action, and for a member the group it was
/// painted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Press {
    pub at: Target,
    pub act: Act,
    pub group: Option<Target>,
}

/// A block row's slide ([`SlideY`]), the pane frame it was laid out in
/// last, whether an agent op started it, and the last frame it moved in
/// for one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sliding {
    pub slide: motion::Slide,
    pub at: u64,
    pub agent: bool,
    pub moved: u64,
}

/// A block row's last paint: its rect and tag, and when an agent op last
/// moved or replaced it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Painted {
    pub bounds: Bounds<Pixels>,
    pub tag: Tag,
    pub moved: Option<Instant>,
}

/// One pane's lease and targets.
#[derive(Default)]
pub(crate) struct Lease {
    /// The pointer's last move over the list.
    pub moved: Option<Instant>,
    pub over: bool,
    /// A press taken on a block, held.
    pub pressed: bool,
    pub hover: Option<Target>,
    /// The group member under the pointer (its group is `hover`).
    pub member: Option<Target>,
    /// The block row whose header is under the pointer, and since when:
    /// its hover actions show (D7, [`crate::walk`]).
    pub head: Option<(u64, Instant)>,
    pub down: Option<Press>,
    /// A block's note until the next input.
    pub note: Option<(u64, &'static str)>,
    pub painted: Rc<RefCell<HashMap<u64, Painted>>>,
    /// The pane frame that draws the last board published: it draws an
    /// agent op (module docs).
    pub agent_frame: Rc<Cell<Option<u64>>>,
    /// Block rows changed above the view, oldest first (the pill).
    pub above: Rc<RefCell<Vec<u64>>>,
    /// Block rows' slides ([`SlideY`]), by row.
    pub slides: Rc<RefCell<HashMap<u64, Sliding>>>,
    /// The rail's: rows an agent op's glide moved, by the frame it moved
    /// them in (`AppView::rail_retire`).
    pub glided: Rc<RefCell<HashMap<u64, u64>>>,
    /// The pane's frames drawn.
    pub frame: Rc<Cell<u64>>,
    /// The list neither follows its tail nor scrolled since the last frame
    /// (set as the pane draws): a row's place on screen moves only with
    /// the rows.
    pub anchored: Rc<Cell<bool>>,
    /// The list's scroll offset as the last frame drew.
    pub scrolled: Cell<Option<f32>>,
}

impl Lease {
    pub(crate) fn active(&self, now: Instant) -> bool {
        self.pressed || (self.over && self.moved.is_some_and(|t| now < t + LEASE))
    }

    /// Everything it holds for the blocks it drew (another session, a
    /// detach, its list gone): no press, hover or note, no target, slide
    /// or pill. Its frame count and the list's anchoring go on.
    pub(crate) fn clear(&mut self) {
        (self.moved, self.over, self.pressed, self.hover, self.member, self.down, self.note) = (None, false, false, None, None, None, None);
        self.head = None;
        self.painted.borrow_mut().clear();
        self.agent_frame.set(None);
        self.above.borrow_mut().clear();
        self.slides.borrow_mut().clear();
        self.glided.borrow_mut().clear();
    }

    /// Whether the pane's frame `frame` draws an agent op for row `slid`
    /// (the top-level block row it slides with): the frame that draws a
    /// board published, or one of a slide an agent op started (in the
    /// rail, of a glide that moved it).
    fn agent_in(
        agent: &Cell<Option<u64>>,
        slides: &RefCell<HashMap<u64, Sliding>>,
        glided: &RefCell<HashMap<u64, u64>>,
        frame: u64,
        slid: u64,
    ) -> bool {
        agent.get() == Some(frame)
            || slides.borrow().get(&slid).is_some_and(|s| s.moved == frame)
            || glided.borrow().get(&slid) == Some(&frame)
    }
}

/// What a block's height follows in one pane, beside the board: the
/// pane's column and text (a note's wrap), its `more` and its folds.
pub(crate) struct Geom<'a> {
    pub cpl: f32,
    pub more: &'a std::collections::HashSet<u64>,
    pub folds: &'a crate::blocks::Folds,
}

fn hash(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// What a block's height follows in pane geometry `g` (§6.2: structure
/// and bounded line counts), as the renderer draws it: a change of it
/// moves what is under it. A note's height is its wrapped text's unless
/// it is cut at [`crate::blocks::NOTE_LINES`] ([`crate::blocks::note_cut`],
/// the renderer's own test): its text then, the safe side (the wrap is the
/// text layout's, not an estimate). A diff's well is its lines' and its
/// foot's; an edit preview's lines are the session's edits of its file.
/// The fold is the pane's ([`crate::blocks::Folds`]).
pub(crate) fn shape(s: &Shown, g: &Geom) -> u64 {
    let b = &s.block;
    let row = crate::blocks::row_id(&b.id);
    let body = match &b.body {
        Body::Plan(p) => p.steps.len() as u64,
        Body::Status(_) | Body::File(_) => 0,
        Body::Diff(d) if d.from.is_some() => hash(&d.file),
        Body::Diff(d) => hash((
            d.hunks.iter().map(|h| h.lines.len() + 1).sum::<usize>().clamp(1, crate::blocks::DIFF_LINES),
            d.state == cards_spec::block::DiffState::Proposed,
        )),
        Body::Note(n) => match crate::blocks::note_cut(&n.text, g.cpl, g.more.contains(&row)) {
            Some(true) => 1,
            cut => hash((cut, &n.text)),
        },
        Body::Metric(m) => u64::from(m.series.is_empty()),
        Body::Group(_) => hash(s.members.iter().map(|m| shape(m, g)).collect::<Vec<_>>()),
    };
    hash((b.body.kind().as_str(), g.folds.folded(row, s), body))
}

/// A row's blocks' shapes, and whether its call draws a row.
fn shapes(r: Option<&Arc<RowBlocks>>, g: &Geom) -> u64 {
    r.map_or(0, |r| hash((matches!(r.call, Some(Fate::Unbound)), r.blocks.iter().map(|s| shape(s, g)).collect::<Vec<_>>())))
}

/// Where block row `row` is on board `d`: its anchor's item id, its place
/// there, and the block.
fn find(d: &Drawn, row: u64) -> Option<(u64, usize, &Shown)> {
    d.rows.iter().find_map(|(item, rb)| {
        rb.blocks.iter().enumerate().find(|(_, s)| crate::blocks::row_id(&s.block.id) == row).map(|(j, s)| (*item, j, s))
    })
}

/// [`find`], or among the rail's sections `pinned` puts into the
/// transcript (after their row's own blocks).
fn find_in<'a>(d: &'a Drawn, pinned: &'a crate::blocks::Pinned, row: u64) -> Option<(u64, usize, &'a Shown)> {
    find(d, row).or_else(|| {
        pinned.iter().find_map(|(item, list)| {
            let j = list.iter().position(|s| crate::blocks::row_id(&s.block.id) == row)?;
            Some((*item, d.rows.get(item).map_or(0, |r| r.blocks.len()) + j, &list[j]))
        })
    })
}

/// Block row `row` on board `d`, a top-level block or a group's member,
/// and for a member its group: a transcript row's, or a rail section's
/// (one the user put into the transcript draws a row there).
pub(crate) fn find_any(d: &Drawn, row: u64) -> Option<(&Shown, Option<&Shown>)> {
    d.rows.values().flat_map(|rb| rb.blocks.iter()).chain(d.rail.iter().flatten()).find_map(|s| {
        if crate::blocks::row_id(&s.block.id) == row {
            return Some((s, None));
        }
        s.members.iter().find(|m| crate::blocks::row_id(&m.block.id) == row).map(|m| (m, Some(s)))
    })
}

/// A group's members as targets: each one's row and incarnation, in its
/// place (a member replaced, removed, added or moved changes it; a tick,
/// the same incarnation, doesn't).
fn members(s: &Shown) -> Vec<(u64, Tag)> {
    s.members.iter().map(|m| (crate::blocks::row_id(&m.block.id), m.block.tag)).collect()
}

/// A block's content as its pill and glow see it.
fn content(s: &Shown) -> u64 {
    hash((s.block.tag, s.block.rev, s.block.collapsed, s.block.pending, s.members.iter().map(content).collect::<Vec<_>>()))
}

impl AppView {
    /// Whether board `new` would move or replace what is under the pointer
    /// in the pane acted on (module docs), its board drawn `old`.
    fn disturbs(&self, old: Option<&Drawn>, new: &Drawn) -> bool {
        let lease = &self.panes.cur.lease;
        let Some(hover) = lease.hover.filter(|_| lease.active(self.fx.now())) else { return false };
        let Some(old) = old else { return false };
        // The rail's sections the user put into the transcript draw rows
        // too (D6b): after their creating call's own.
        let live = self.panes.cur.live;
        let (op, np) = (self.pinned_in(live, old), self.pinned_in(live, new));
        // A hover the board drawn doesn't hold is stale (its block went
        // with a board drawn since): nothing of it is under the pointer.
        let Some((oi, oj, os)) = find_in(old, &op, hover.row) else { return false };
        let Some((ni, nj, ns)) = find_in(new, &np, hover.row) else { return true };
        let Some(m) = self.panes.cur.metrics_last.get() else { return true };
        let p = &self.panes.cur;
        let g = Geom { cpl: crate::blocks::note_cpl(p.layout.column, &m), more: &p.block_more, folds: &p.block_folds };
        // What this desk draws of a block (D6b): its tombstone or its stub
        // is one line, whatever its body.
        let look = |d: &Drawn, s: &Shown| hash(self.look_on(d, s));
        // A target as painted last (the hover's tag is the one the pointer
        // came in over: a replacement drawn since is what is under it now).
        let painted = lease.painted.borrow();
        let shown = |t: Target| painted.get(&t.row).map_or(t.tag, |p| p.tag);
        if (ni, nj) != (oi, oj) || ns.block.tag != shown(hover) || shape(ns, &g) != shape(os, &g) || look(new, ns) != look(old, os) {
            return true;
        }
        // Its members are targets too (Shady #101 R2-3): the same ones, the
        // same incarnations in the same places.
        if members(ns) != members(os) {
            return true;
        }
        // The member under the pointer, as painted: in this group, its
        // incarnation and its height. One the board drawn doesn't hold in
        // this group can't be proved safe: the board waits.
        if let Some(mt) = lease.member {
            let at = |s: &Shown| s.members.iter().position(|x| crate::blocks::row_id(&x.block.id) == mt.row);
            let (Some(a), Some(b)) = (at(os), at(ns)) else { return true };
            let (om, nm) = (&os.members[a], &ns.members[b]);
            if a != b || om.block.tag != shown(mt) || nm.block.tag != shown(mt) || shape(om, &g) != shape(nm, &g) {
                return true;
            }
        }
        // The rows in view above it.
        let disp = &self.panes.cur.disp;
        let Some(h) = disp.iter().position(|d| d.id == hover.row) else { return false };
        let top = self.panes.cur.list.logical_scroll_top().item_ix.min(h);
        let items = self.model.lives[self.panes.cur.live].shown().items();
        let mut seen = Vec::new();
        for d in &disp[top..=h] {
            let Some(id) = items.get(d.item).map(|i| i.id) else { continue };
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            // A row's blocks as drawn: their shapes and looks, its own then
            // the sections put there, the first `n`.
            let drawn = |d: &Drawn, pinned: &crate::blocks::Pinned, n: usize| {
                let own = d.rows.get(&id).map_or(&[][..], |r| &r.blocks[..]);
                let theirs = pinned.get(&id).map_or(&[][..], Vec::as_slice);
                hash(own.iter().chain(theirs).take(n).map(|s| (shape(s, &g), look(d, s))).collect::<Vec<_>>())
            };
            if id == oi {
                // Its own anchor: the blocks before it there.
                if drawn(old, &op, oj) != drawn(new, &np, oj) {
                    return true;
                }
                continue;
            }
            if shapes(old.rows.get(&id), &g) != shapes(new.rows.get(&id), &g) || drawn(old, &op, usize::MAX) != drawn(new, &np, usize::MAX)
            {
                return true;
            }
        }
        false
    }

    /// A finished board for store `live`, drawn now, or kept waiting under
    /// a pane's lease ([`crate::board`] lands it here).
    pub(crate) fn board_ready(&mut self, live: LiveId, drawn: Arc<Drawn>, cx: &mut Context<Self>) {
        let old = self.model.lives[live].transcript.drawn().cloned();
        let panes: Vec<PaneId> = self.panes.iter().filter(|p| p.live == live).map(|p| p.id).collect();
        // The rail's lease too (D6a, §4): the panel draws the focused
        // pane's session's rail from the same board.
        let held = panes.iter().any(|&p| self.in_pane(p, |this| this.disturbs(old.as_deref(), &drawn)).unwrap_or(false))
            || self.rail_disturbs(live, old.as_deref(), &drawn);
        if held {
            self.model.lives[live].board.waiting = Some(drawn);
            self.arm_lease_wake(cx);
            return;
        }
        self.model.lives[live].board.waiting = None;
        for p in panes {
            self.in_pane(p, |this| {
                // Changed above the view: the pill.
                let top = this.panes.cur.list.logical_scroll_top().item_ix;
                let items = this.model.lives[live].transcript.items();
                // A section put into the transcript is one of its rows (D6b).
                let op = old.as_deref().map(|o| this.pinned_in(live, o)).unwrap_or_default();
                let np = this.pinned_in(live, &drawn);
                let mut above = this.panes.cur.lease.above.borrow_mut();
                for d in this.panes.cur.disp.iter().take(top) {
                    let crate::turns::Kind::Block { .. } = d.kind else { continue };
                    let Some(item) = items.get(d.item) else { continue };
                    let was = old.as_deref().and_then(|o| find_in(o, &op, d.id)).map(|(_, _, s)| content(s));
                    let now_is = find_in(&drawn, &np, d.id).filter(|(at, ..)| *at == item.id).map(|(_, _, s)| content(s));
                    if now_is.is_some() && was != now_is && !above.contains(&d.id) {
                        above.push(d.id);
                    }
                }
                drop(above);
                // Its next frame draws it: an agent op's.
                let l = &this.panes.cur.lease;
                l.agent_frame.set(Some(l.frame.get() + 1));
            });
        }
        self.model.lives[live].transcript.set_drawn(Some(drawn.clone()));
        self.model.lives[live].board.published += 1;
        self.rail_landed(live, &drawn);
        // What it settled or carried, written (D6b: not only a click's).
        self.pump_wishes(cx);
        // The panes' caches keep only the blocks this board draws (§1.8:
        // renderer caches are bounded, per pane).
        let rows: std::collections::HashSet<u64> = drawn
            .rows
            .values()
            .flat_map(|r| r.blocks.iter().flat_map(|s| std::iter::once(s).chain(&s.members)))
            .map(|s| crate::blocks::row_id(&s.block.id))
            .collect();
        for p in self.panes.iter_mut().filter(|p| p.live == live) {
            p.lease.painted.borrow_mut().retain(|r, _| rows.contains(r));
            p.lease.above.borrow_mut().retain(|r| rows.contains(r));
            p.lease.slides.borrow_mut().retain(|r, _| rows.contains(r));
            p.block_scroll.borrow_mut().retain(|r, _| rows.contains(r));
            p.block_more.retain(|r| rows.contains(r));
            p.block_folds.retain(|r| rows.contains(r));
        }
        cx.notify();
    }

    /// The lease's end, the pointer gone or a key: a board that waited
    /// draws, unless a lease still holds it.
    pub(crate) fn lease_release(&mut self, cx: &mut Context<Self>) {
        // The rail's glides held under its lease go on.
        if self.rail_gliding() {
            cx.notify();
        }
        for live in self.model.lives.ids() {
            if let Some(d) = self.model.lives[live].board.waiting.take()
                && d.epoch == self.model.lives[live].transcript.blocks_epoch()
            {
                self.board_ready(live, d, cx);
            }
        }
    }

    /// One wake, at the lease's end, while a board waits (§6.2 "Frames"):
    /// the panes' leases and the rail's.
    pub(crate) fn arm_lease_wake(&mut self, cx: &mut Context<Self>) {
        let now = self.fx.now();
        let rail = &self.agent_rail.lease;
        let leases = self.panes.iter().map(|p| &p.lease).chain(std::iter::once(rail));
        let due = leases.filter(|l| !l.pressed).filter_map(|l| l.moved).map(|t| t + LEASE).max();
        let Some(due) = due.filter(|d| *d > now) else {
            if !self.panes.iter().any(|p| p.lease.pressed) && !self.agent_rail.lease.pressed {
                self.lease_release(cx);
            }
            return;
        };
        let wait = due.saturating_duration_since(now);
        self.lease_wake = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                v.lease_wake = None;
                if v.model.lives.ids().into_iter().any(|l| v.model.lives[l].board.waiting.is_some()) {
                    v.arm_lease_wake(cx);
                }
            });
        }));
    }

    /// The pointer moved over pane `pane`'s list: the lease (no frame).
    /// With no button down while a press is held, its release was lost
    /// (outside the window, a grab taken): the press ends.
    pub(crate) fn lease_move(&mut self, pane: PaneId, buttons: bool, cx: &mut Context<Self>) {
        let now = self.fx.now();
        self.in_pane(pane, |this| {
            this.panes.cur.lease.moved = Some(now);
            this.panes.cur.lease.over = true;
        });
        if !buttons && self.panes.iter().any(|p| p.lease.pressed) {
            self.lease_unpress(cx);
        }
    }

    /// The pointer over pane `pane`'s list or not.
    pub(crate) fn lease_over(&mut self, pane: PaneId, over: bool, cx: &mut Context<Self>) {
        self.in_pane(pane, |this| {
            this.panes.cur.lease.over = over;
            if !over {
                (this.panes.cur.lease.hover, this.panes.cur.lease.member) = (None, None);
            }
        });
        if !over {
            self.lease_release(cx);
        }
    }

    /// A key: the note goes, and what waited draws.
    pub(crate) fn lease_key(&mut self, cx: &mut Context<Self>) {
        for p in self.panes.iter_mut() {
            p.lease.note = None;
            p.lease.moved = None;
        }
        (self.agent_rail.lease.note, self.agent_rail.lease.moved) = (None, None);
        self.lease_release(cx);
    }

    /// A press ends with no action (§6.2 "Down and up": a release that is
    /// not on its target, a lost one, the window's focus gone): no pane
    /// holds one, and a board that waited on it has its wake, or draws.
    pub(crate) fn lease_unpress(&mut self, cx: &mut Context<Self>) {
        let mut ended = false;
        for p in self.panes.iter_mut() {
            ended |= std::mem::take(&mut p.lease.pressed) | p.lease.down.take().is_some();
        }
        ended |= std::mem::take(&mut self.agent_rail.lease.pressed) | self.agent_rail.down.take().is_some();
        if !ended {
            return;
        }
        if self.model.lives.ids().into_iter().any(|l| self.model.lives[l].board.waiting.is_some()) {
            self.arm_lease_wake(cx);
        }
        cx.notify();
    }

    /// The pointer over block row `row` (painted as `tag`) or off it.
    pub(crate) fn block_hover(&mut self, pane: PaneId, row: u64, tag: Tag, over: bool) {
        self.in_pane(pane, |this| {
            let l = &mut this.panes.cur.lease;
            if over {
                l.hover = Some(Target { row, tag });
            } else if l.hover.is_some_and(|h| h.row == row) {
                (l.hover, l.member) = (None, None);
            }
        });
    }

    /// The pointer over group member `row` (painted as `tag`) or off it;
    /// its group is the hovered block.
    pub(crate) fn member_hover(&mut self, pane: PaneId, row: u64, tag: Tag, over: bool) {
        self.in_pane(pane, |this| {
            let l = &mut this.panes.cur.lease;
            if over {
                l.member = Some(Target { row, tag });
            } else if l.member.is_some_and(|m| m.row == row) {
                l.member = None;
            }
        });
    }

    /// Pointer-down on press target `press`: the target taken and the
    /// lease held, or, just moved, nothing (once).
    pub(crate) fn block_down(&mut self, pane: PaneId, press: Press, cx: &mut Context<Self>) {
        let now = self.fx.now();
        let row = press.at.row;
        self.in_pane(pane, |this| {
            let l = &mut this.panes.cur.lease;
            l.note = None;
            let moved = l.painted.borrow().get(&row).and_then(|p| p.moved).is_some_and(|t| now.saturating_duration_since(t) < JUST_MOVED);
            if moved {
                if let Some(p) = l.painted.borrow_mut().get_mut(&row) {
                    p.moved = None;
                }
                (l.down, l.pressed) = (None, false);
                l.note = Some((row, MOVED));
                return;
            }
            l.down = Some(press);
            l.pressed = true;
            l.moved = Some(now);
        });
        cx.notify();
    }

    /// Pointer-up on block row `row`'s `act`: the press's target, if it is
    /// this one and the bound board still holds it (a member in the same
    /// group), acts (§3.5: local); else nothing, and the note.
    pub(crate) fn block_up(&mut self, pane: PaneId, row: u64, act: Act, cx: &mut Context<Self>) {
        self.in_pane(pane, |this| {
            let l = &mut this.panes.cur.lease;
            l.pressed = false;
            let Some(down) = l.down.take().filter(|d| d.at.row == row && d.act == act) else { return };
            // The bound board now: the newest, drawn or waiting.
            let live = &this.model.lives[this.panes.cur.live];
            let board = live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned());
            let found = board.as_deref().and_then(|d| find_any(d, row)).map(|(s, g)| {
                let tag = |s: &Shown| s.bound.as_ref().map(|b| b.tag);
                let group = g.map(|g| (crate::blocks::row_id(&g.block.id), tag(g)));
                (tag(s), group)
            });
            let held = found.is_some_and(|(tag, group)| tag == Some(down.at.tag) && group == down.group.map(|g| (g.row, Some(g.tag))));
            // Drawn now in this pane: its list row (a member's is its group's).
            let shown = this.block_drawn(row);
            match shown {
                Some((ix, s)) if held => {
                    match act {
                        Act::Fold => {
                            let (before, after) = this.panes.cur.block_folds.toggle(row, &s);
                            // A top-level block's fold: its undo (D7), what
                            // it set and what was before.
                            if this.block_shown(row).is_some() {
                                let act = crate::walk::Undid::Fold { before, after };
                                let u = crate::walk::Undo { at: crate::walk::At::Block(row), tag: s.block.tag, act };
                                this.panes.cur.walk.push(u);
                            }
                        }
                        Act::More => {
                            if !this.panes.cur.block_more.remove(&row) {
                                this.panes.cur.block_more.insert(row);
                            }
                        }
                        // Its own up ([`AppView::hover_up`]).
                        Act::Hover(_) => {}
                    }
                    this.panes.cur.list.remeasure_items(ix..ix + 1);
                }
                _ => this.panes.cur.lease.note = Some((row, CHANGED)),
            }
        });
        // A board that waited on the press: its wake, or now.
        if self.model.lives.ids().into_iter().any(|l| self.model.lives[l].board.waiting.is_some()) {
            self.arm_lease_wake(cx);
        }
        cx.notify();
    }

    /// The pill's click: the first block changed above, walked to.
    pub(crate) fn walk_above(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        self.in_pane(pane, |this| {
            let first = this.panes.cur.lease.above.borrow().first().copied();
            if let Some(ix) = first.and_then(|r| this.panes.cur.disp.iter().position(|d| d.id == r)) {
                // The user's jump: it leaves the end (a list following its
                // tail would snap back at its next layout).
                this.panes.cur.list.pause_following_tail();
                this.panes.cur.scroller.touch();
                this.panes.cur.list.scroll_to_reveal_item(ix);
            }
        });
        cx.notify();
    }
}

/// Block row `row`, sliding from where it was painted when an agent op
/// moved it within view (module docs). The offset applies to its paint
/// and its hit regions alike: what is drawn is what is clicked.
pub(crate) struct SlideY {
    child: AnyElement,
    row: u64,
    agent: Rc<Cell<Option<u64>>>,
    slides: Rc<RefCell<HashMap<u64, Sliding>>>,
    anchored: Rc<Cell<bool>>,
    frame: Rc<Cell<u64>>,
    fx: Rc<motion::Motion>,
    off: f32,
}

impl SlideY {
    pub(crate) fn new(l: &Lease, fx: &Rc<motion::Motion>, row: u64, child: AnyElement) -> Self {
        SlideY {
            child,
            row,
            agent: l.agent_frame.clone(),
            slides: l.slides.clone(),
            anchored: l.anchored.clone(),
            frame: l.frame.clone(),
            fx: fx.clone(),
            off: 0.,
        }
    }

    /// Its offset this frame, its layout top at `top`. A slide starts only
    /// in the frame that draws an agent op, and goes on while its row's
    /// place holds: a user's change (a fold, a scroll, a resize) moves the
    /// row at once, even mid-slide.
    fn offset(&mut self, top: f32) -> f32 {
        let now = self.fx.now();
        let frame = self.frame.get();
        let agent = self.agent.get() == Some(frame) && self.anchored.get() && self.fx.level() == motion::Level::Full;
        let mut slides = self.slides.borrow_mut();
        let e =
            slides.entry(self.row).or_insert_with(|| Sliding { slide: motion::Slide::rest(top, now), at: frame, agent: false, moved: 0 });
        // Laid out in the pane's last frame: from there; else it comes
        // from out of view (`appear`, not a slide).
        let last = e.at + 1 == frame || e.at == frame;
        e.at = frame;
        if last && agent {
            e.slide.retarget(top, now);
            e.agent = true;
        } else if !(last && e.agent && e.slide.target() == top) {
            (e.slide, e.agent) = (motion::Slide::rest(top, now), false);
        }
        if e.slide.settled(now) {
            // Its last move lands now: the window runs from here.
            if std::mem::take(&mut e.agent) {
                e.moved = frame;
            }
            e.slide = motion::Slide::rest(top, now);
            return 0.;
        }
        if e.agent {
            e.moved = frame;
        }
        self.fx.want_frame();
        e.slide.state(now).0
    }
}

impl IntoElement for SlideY {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for SlideY {
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
        (self.child.request_layout(window, cx), ())
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
        self.off = self.offset(f32::from(bounds.top()));
        if self.off != 0. {
            window.request_animation_frame();
        }
        window.with_element_offset(point(px(0.), px(self.off)), |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // Its bounds were offset as it was prepainted.
        self.child.paint(window, cx);
    }
}

/// A block's probe (a child of the block, or of a group's member, which is
/// positioned): each paint notes its rect and tag; a change in a frame
/// that draws an agent op (for row `slid`, the top-level block row it
/// slides with) marks it just moved; painted in view, it leaves the pill.
pub(crate) fn probe(l: &Lease, fx: &Rc<crate::motion::Motion>, row: u64, tag: Tag, slid: u64) -> impl IntoElement + use<> {
    let (painted, agent, slides, glided, frame, above, fx) =
        (l.painted.clone(), l.agent_frame.clone(), l.slides.clone(), l.glided.clone(), l.frame.clone(), l.above.clone(), fx.clone());
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let now = fx.now();
            let agent = Lease::agent_in(&agent, &slides, &glided, frame.get(), slid);
            let mut map = painted.borrow_mut();
            match map.get_mut(&row) {
                Some(p) if p.bounds != bounds || p.tag != tag => {
                    if agent {
                        p.moved = Some(now);
                    }
                    (p.bounds, p.tag) = (bounds, tag);
                }
                Some(_) => {}
                None => {
                    map.insert(row, Painted { bounds, tag, moved: agent.then_some(now) });
                }
            }
            let seen = bounds.intersect(&window.content_mask().bounds);
            let mut above = above.borrow_mut();
            if seen.size.height > px(0.) && above.contains(&row) {
                above.retain(|r| *r != row);
                // The pill drawn this frame counted it: the next one doesn't.
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .inset_0()
}

impl AppView {
    /// The pill (§6.2): `N blocks changed above ↑` at the list's top, a
    /// layer over it; a click walks to the first.
    pub(crate) fn blocks_pill(&self, p: &crate::theme::Palette, m: &crate::theme::Metrics, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::fonts::{self, TextPx as _};
        use gpui_kit::component::h_flex;
        let n = self.panes.cur.lease.above.borrow().len();
        if n == 0 {
            return None;
        }
        let pane = self.panes.cur.id;
        let text = format!("{n} block{} changed above ↑", if n == 1 { "" } else { "s" });
        Some(
            div()
                .absolute()
                .top(px((8. * m.r).round()))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    h_flex()
                        .id(("blocks-above", pane.index()))
                        .debug_selector(move || format!("blocks-above {}", pane.index()))
                        .h(px((26. * m.r).round()))
                        .px(px((10. * m.r).round()))
                        .items_center()
                        .bg(p.card_d)
                        .font_family(fonts::SANS)
                        .text_px(m.meta)
                        .text_color(p.acc)
                        .cursor_pointer()
                        .hover(|d| d.bg(p.hover))
                        .child(text)
                        .on_click(cx.listener(move |this, _, _, cx| this.walk_above(pane, cx))),
                )
                .into_any_element(),
        )
    }
}
