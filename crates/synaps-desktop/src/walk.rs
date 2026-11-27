//! Block walk (synaps-cards `docs/blocks.md` v1.3 §3.8; §3.5's undo; §3.6;
//! §8.2's D7 row): the keyboard's way through the agent's blocks.
//!
//! - **Entering.** Ctrl+G (Cmd+G on macOS) enters it in the focused pane,
//!   on a new press, never mid IME composition (the IME's keys), over the
//!   session's thread (not a view). It starts on the newest block in view
//!   (none in view: the newest). The pane holds the keys on its own focus
//!   ([`WalkMode::focus`], never the window's, whose keys answer a prompt);
//!   an agent card entered there gives way.
//! - **Walking.** `j`/`k` (or ↓/↑) walk this pane's blocks in reading
//!   order, then the rail's agent sections (the focused pane's session's),
//!   upper slot first, as the rail draws them. The walked one is brought
//!   into view: the list scrolls, the panel scrolls, a folded panel opens
//!   (one too narrow for the window can't: its sections are out of the
//!   walk then).
//! - **Activation** is the entered card's rule: keys act from the first
//!   frame that paints the walk ring ([`Walk::shown`]), and only fresh,
//!   bare presses count ([`crate::ask_keys::Presses::acts`]): a repeat, a
//!   key held from before or a chord does nothing. A key before the ring
//!   was painted ends the walk, and a printable one is typed into the
//!   composer.
//! - **Every key acts on the painted target** (§6.2): the walked block's
//!   row and incarnation as the ring last painted them ([`Walk::painted`]),
//!   checked against the bound board, the newest drawn or waiting under
//!   the lease, *before* the lease releases (the root's capture defers
//!   the release to after the key). A block whose incarnation differs now,
//!   or gone, does nothing and says `this block changed · look again`;
//!   the release then shows why. A key names its target by identity,
//!   never by position, so the just-moved rule never blocks one.
//! - **Acting.** `z` or `c` folds the block (local, §3.5); `[` and `]`
//!   walk its revisions (§3.6: [`crate::revisions`], the past body drawn
//!   from the re-fold scratch, one at a time; `]` or Esc back to the
//!   latest, and no yanking: an agent op landing meanwhile moves nothing,
//!   a dot says `r6 new`). `x` dismisses, `u` shows again, `p` pins, `m`
//!   moves a rail section to the other slot: D6b's desires and stores
//!   (#103), through [`BlockDesires`], whose default does nothing until
//!   D6b wires it. `e` (take) and `R` (restore) are D8's: nothing yet.
//! - **Undo** (§3.5): Ctrl+Z (Cmd+Z) in the walk undoes the last local
//!   action, one level per block: a fold (a key's or a click's, in the
//!   transcript or the rail), and through [`BlockDesires::undo`] a pin, a
//!   slot move or a dismissal not yet sent. The block must still be the
//!   incarnation acted on; the walk goes to it.
//! - **Leaving.** Esc returns to the composer, never to the window, so a
//!   second Esc can't deny a prompt (design §5.1): the composer's Esc with
//!   a prompt up only hands the prompt its keys, after their wait. Any
//!   other printable key leaves it too, typed into the composer. It ends
//!   when the keys go elsewhere (a click on an input, another pane, Ctrl+J
//!   to a card or a prompt), the session or the tab changes, or a view
//!   opens.
//! - **A status line** at the list's foot names the mode's keys, as
//!   point's does: a layer, nothing reflows.
//! - **Hover actions** (§3.8 "Mouse"): hovering a block's header, or a
//!   rail section's, shows `rev · fold` (and with D6b `rail · ⇅ · ×`)
//!   over the header's right end, on its ground: they never reflow it.
//!   Each is a painted target, as the chevron is, and meets the
//!   just-moved rule as a click does.
//! - **Motion:** the ring and the hover actions fade in on the chrome's
//!   `quick` (Reduced: the same, colour; Off: at once); nothing asks a
//!   frame at rest.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cards_spec::block::Ref;
use cards_spec::board::{State, Target as Refold};
use gpui_kit::component::h_flex;
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Shown, Tag};

use crate::app::{AppView, mono};
use crate::fonts::{self, TextPx as _, size};
use crate::lease::CHANGED;
use crate::motion;
use crate::revisions::{self, Rev};
use crate::theme::Palette;
use crate::turns::Kind;

/// The walk's key context.
pub(crate) const WALK_CTX: &str = "BlockWalk";

/// Local undo keeps this many blocks' last actions at most.
pub(crate) const UNDO_MAX: usize = 32;

/// A past revision's motion keys (glows, changes): their own per
/// revision, so stepping never glows (row ids keep 59 bits).
const PAST: u64 = 0x5EED_0000_0000;

/// Ctrl+`letter` (Cmd on macOS), nothing else held.
pub(crate) fn chord(k: &Keystroke, letter: &str, mac: bool) -> bool {
    let m = &k.modifiers;
    let cmd = if mac { m.platform && !m.control } else { m.control && !m.platform };
    k.key.eq_ignore_ascii_case(letter) && cmd && !(m.alt || m.shift || m.function)
}

/// Where the walk is: a block row of this pane's transcript, or a rail
/// section's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum At {
    Block(u64),
    Rail(u64),
}

impl At {
    pub(crate) fn row(self) -> u64 {
        match self {
            At::Block(r) | At::Rail(r) => r,
        }
    }
}

/// A past revision in view (§3.6): which, where it is in the block's
/// revisions ([`revisions::revisions`], newest first), and the block's
/// revision and incarnation when the view opened (a later one: `rN new`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RevView {
    pub rev: Rev,
    pub ix: usize,
    pub latest: u32,
    pub tag: Tag,
}

/// The walk in a pane.
pub(crate) struct Walk {
    pub at: At,
    /// Its place in the walk's order when it came there: a block gone
    /// from under it, `j`/`k` go on from there.
    pub ix: usize,
    /// The session it walks.
    pub session: String,
    /// The ring's last paint: the walked block's incarnation (§6.2).
    pub painted: Rc<Cell<Option<Tag>>>,
    /// Its ring was painted in view since the walk began: keys act.
    pub shown: Rc<Cell<bool>>,
    /// Its ring was painted in view since this frame began.
    pub seen: Rc<Cell<bool>>,
    /// When it came to `at` (the ring's fade).
    pub since: Instant,
    pub rev: Option<RevView>,
    /// Its bringing into view is still to finish (a row not laid out, a
    /// section not painted yet, a panel opening): tried again as the next
    /// frames begin, this many more times.
    pub reveal: u8,
}

/// What a local action was, for its undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Undid {
    Fold,
    Wish(Wish),
}

/// D6b's actions ([`BlockDesires`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wish {
    Dismiss,
    Show,
    Pin,
    Slot,
}

/// A block's last local action (§3.5's undo): its row and incarnation then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Undo {
    pub at: At,
    pub tag: Tag,
    pub act: Undid,
}

/// A pane's walk: the focus that holds its keys (made at the pane's first
/// frame, always drawn), the walk, and the local undo.
#[derive(Default)]
pub(crate) struct WalkMode {
    pub focus: Option<FocusHandle>,
    pub walk: Option<Walk>,
    /// The last local action per block, oldest first, ≤ [`UNDO_MAX`].
    pub undo: Vec<Undo>,
    /// A revision view asked for a re-fold: dropped once the walk is gone,
    /// however it went (§1.8: the scratch goes with its view).
    pub refolding: bool,
}

impl WalkMode {
    /// Block `u.at`'s last local action is `u`: one level per block.
    pub(crate) fn push(&mut self, u: Undo) {
        self.undo.retain(|x| x.at.row() != u.at.row());
        self.undo.push(u);
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
    }

    /// Whether its focus has the keys.
    pub(crate) fn holds(&self, window: &Window) -> bool {
        self.focus.as_ref().is_some_and(|f| f.is_focused(window))
    }
}

// ── the seam for D6b ──────────────────────────────────────────────────

/// The block a shared or stored key acts on, as the walk found it on the
/// bound board: its session and board, the block, and for a rail section
/// its slot.
#[cfg_attr(not(test), expect(dead_code, reason = "D6b's side reads it (#103); D7's default doesn't"))]
pub(crate) struct Aim<'a> {
    pub session: &'a str,
    pub drawn: &'a Drawn,
    pub block: &'a Shown,
    pub rail: Option<cards_spec::block::Slot>,
}

/// How D6b answered.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code, reason = "D6b's side answers them (#103); D7's default is Off"))]
pub(crate) enum Asked {
    /// Not wired (D7's default): nothing happened.
    Off,
    /// Accepted (D6b draws it).
    Done,
    /// Refused, with why (a cap, a write that failed): the block says
    /// `not done · try again`, the log why.
    Refused(String),
}

/// The walk's `x`, `u`, `p` and `m`, the hover's `×`, `rail` and `⇅`, and
/// their undo: D6b's dismiss and show desires and its pins and slot moves
/// (#103). D7 routes the keys here; the default does nothing, and the
/// status line and the hover don't offer them until [`BlockDesires::wired`].
pub(crate) trait BlockDesires {
    /// The keys act: their hints and buttons show.
    fn wired(&self) -> bool {
        false
    }
    fn dismiss(&mut self, _: &Aim) -> Asked {
        Asked::Off
    }
    fn show(&mut self, _: &Aim) -> Asked {
        Asked::Off
    }
    fn pin(&mut self, _: &Aim) -> Asked {
        Asked::Off
    }
    fn slot(&mut self, _: &Aim) -> Asked {
        Asked::Off
    }
    /// Undo `wish` on `aim` (§3.5: a pin, a slot move, a dismissal not yet
    /// sent; a delivered one is undone with Show).
    fn undo(&mut self, _: &Aim, _: Wish) -> Asked {
        Asked::Off
    }
}

/// D7's: nothing wired.
pub(crate) struct NoDesires;

impl BlockDesires for NoDesires {}

// ── the walk ──────────────────────────────────────────────────────────

impl AppView {
    /// The walk's order in the pane acted on (module docs): its block rows
    /// in reading order, then the rail's sections as drawn, upper first,
    /// if the rail is this pane's and can be drawn.
    pub(crate) fn walk_order(&self) -> Vec<At> {
        let mut out: Vec<At> =
            self.panes.cur.disp.iter().filter(|d| matches!(d.kind, Kind::Block { .. })).map(|d| At::Block(d.id)).collect();
        if self.walk_rail_reachable()
            && let Some((_, lay)) = self.rail_layout()
        {
            use crate::agent_rail::State as S;
            out.extend(
                lay.sections()
                    .filter(|s| matches!(s.state, S::Open | S::Summary | S::Listed))
                    .map(|s| At::Rail(crate::blocks::row_id(&s.s.block.id))),
            );
        }
        out
    }

    /// The rail's sections are in the walk: the pane acted on is the
    /// focused one (the panel describes its session), no view is up, and
    /// the panel is open or can open.
    fn walk_rail_reachable(&self) -> bool {
        if self.panes.cur.id != self.panes.focused || self.model.tabs.view().is_some() {
            return false;
        }
        let open = |panels| matches!(crate::app::sides(self.width, panels, crate::chrome::MARK).1, crate::app::Side::Open(_));
        open(self.panels) || open(crate::theme::Panels { right_collapsed: false, ..self.panels })
    }

    /// Ctrl+G (Cmd+G): the walk entered in the focused pane (module docs);
    /// `false`: nothing to walk here.
    pub(crate) fn walk_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.model.tabs.view().is_some() || self.panes.cur.walk.walk.is_some() {
            return self.panes.cur.walk.walk.is_some();
        }
        let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return false };
        let order = self.walk_order();
        if order.is_empty() {
            return false;
        }
        // The newest block in view: the last block row laid out in it.
        let (top, bottom) = self.list_view();
        let in_view = |row: u64| {
            let ix = self.panes.cur.disp.iter().position(|d| d.id == row)?;
            let b = self.panes.cur.list.bounds_for_item(ix)?;
            (f32::from(b.bottom()) > top && f32::from(b.top()) < bottom).then_some(())
        };
        let blocks: Vec<usize> = order.iter().enumerate().filter(|(_, a)| matches!(a, At::Block(_))).map(|(k, _)| k).collect();
        let start = blocks.iter().rev().copied().find(|&k| in_view(order[k].row()).is_some()).or(blocks.last().copied()).unwrap_or(0);
        if self.panes.cur.ask.entry.is_some() {
            self.leave_ask(false, window, cx);
        }
        let focus = self.panes.cur.walk.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        log::info!(target: "synaps_desktop::walk", "walk: entered on {:?}", order[start]);
        self.panes.cur.walk.walk = Some(Walk {
            at: order[start],
            ix: start,
            session,
            painted: Rc::default(),
            shown: Rc::default(),
            seen: Rc::default(),
            since: self.fx.now(),
            rev: None,
            reveal: 0,
        });
        window.focus(&focus, cx);
        self.walk_reveal(false, cx);
        cx.notify();
        true
    }

    /// The walk ends in the pane acted on; `refocus`: the keys go to its
    /// composer (never to the window, whose Esc denies a prompt).
    pub(crate) fn walk_leave(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(w) = self.panes.cur.walk.walk.take() {
            log::info!(target: "synaps_desktop::walk", "walk: left {:?}", w.at);
            if w.rev.is_some() {
                self.walk_scratch_drop();
            }
        }
        if refocus {
            let c = self.panes.cur.composer.clone();
            c.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// The revision view's re-fold dropped, with what it found (§1.8).
    fn walk_scratch_drop(&mut self) {
        self.panes.cur.walk.refolding = false;
        self.refold_cancel();
    }

    /// As a pane's frame begins: a walk whose keys went elsewhere ends
    /// there; one whose session or tab went, or under a view, ends to the
    /// composer; a revision in view has its re-fold asked for.
    pub(crate) fn sync_walk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Its focus is drawn from the pane's first frame: a key right after
        // Ctrl+G, before the walk's first frame, reaches the walk (and,
        // the ring not painted, goes to the composer), never nowhere.
        if self.panes.cur.walk.focus.is_none() {
            self.panes.cur.walk.focus = Some(cx.focus_handle());
        }
        let holds = self.panes.cur.walk.holds(window);
        let Some(w) = self.panes.cur.walk.walk.as_ref() else {
            // Its focus with no walk (its list went): back to the composer.
            if holds {
                self.walk_leave(true, window, cx);
            }
            if self.panes.cur.walk.refolding {
                self.walk_scratch_drop();
            }
            return;
        };
        w.seen.set(false);
        let here = self.model.tabs.view().is_none()
            && self.model.lives[self.panes.cur.live].active.as_ref().is_some_and(|a| a.session_id == w.session);
        if !holds || !here {
            self.walk_leave(holds && !here, window, cx);
            return;
        }
        // Its bringing into view, finished as the rows are laid out.
        if w.reveal > 0 {
            if let Some(w) = self.panes.cur.walk.walk.as_mut() {
                w.reveal -= 1;
            }
            self.walk_reveal(true, cx);
        }
        let Some(w) = self.panes.cur.walk.walk.as_ref() else { return };
        match w.rev {
            Some(rv) => {
                self.panes.cur.walk.refolding = true;
                self.refold(self.panes.cur.live, Refold::Revision(rv.rev.at), cx);
            }
            None if self.panes.cur.walk.refolding => self.walk_scratch_drop(),
            None => {}
        }
    }

    /// The walked block brought into view: its list row, or its rail
    /// section (the panel opened if folded, scrolled to it). What can't be
    /// placed yet (a row not laid out, which GPUI's reveal only anchors at
    /// the list's edge, under the sticky band; a section not painted; a
    /// panel opening) is placed as the next frames begin (`retry`: one of
    /// those, [`REVEAL_TRIES`] at most).
    fn walk_reveal(&mut self, retry: bool, cx: &mut Context<Self>) {
        let Some(at) = self.panes.cur.walk.walk.as_ref().map(|w| w.at) else { return };
        let placed = match at {
            At::Block(row) => {
                let Some(ix) = self.panes.cur.disp.iter().position(|d| d.id == row) else { return };
                // Clear of the sticky band and the status line, with room
                // to see it is the one.
                let (top, bottom) = self.list_view();
                let top = top + self.sticky_inset(cx) + REVEAL_PAD;
                let bottom = bottom - self.dock_growth() - STATUS_H - REVEAL_PAD;
                match self.panes.cur.list.bounds_for_item(ix) {
                    Some(b) => {
                        let (t, bt) = (f32::from(b.top()), f32::from(b.bottom()));
                        let by = if t < top || bt - t > bottom - top {
                            t - top
                        } else if bt > bottom {
                            bt - bottom
                        } else {
                            0.
                        };
                        if by.abs() > 0.5 {
                            self.panes.cur.list.pause_following_tail();
                            self.panes.cur.scroller.touch();
                            self.panes.cur.list.scroll_by(px(by));
                        }
                        true
                    }
                    None => {
                        self.panes.cur.list.pause_following_tail();
                        self.panes.cur.scroller.touch();
                        self.panes.cur.list.scroll_to_reveal_item(ix);
                        false
                    }
                }
            }
            At::Rail(row) => 'rail: {
                if !matches!(crate::app::sides(self.width, self.panels, crate::chrome::MARK).1, crate::app::Side::Open(_)) {
                    self.toggle_panel(false, cx);
                    break 'rail false;
                }
                // Scrolled to it, as last painted (one not painted yet: as
                // the next frames begin).
                let at = self.agent_rail.lease.painted.borrow().get(&row).map(|p| p.bounds);
                let scroll = &self.bars.right.scroll;
                let view = scroll.bounds();
                if let Some(b) = at
                    && view.size.height > px(0.)
                {
                    let (t, bt, vt, vb) = (b.top(), b.bottom(), view.top(), view.bottom());
                    let by = if t < vt || bt - t > vb - vt {
                        t - vt
                    } else if bt > vb {
                        bt - vb
                    } else {
                        px(0.)
                    };
                    if by.abs() > px(0.5) {
                        let o = scroll.offset();
                        scroll.set_offset(point(o.x, o.y - by));
                    }
                    true
                } else {
                    false
                }
            }
        };
        if let Some(w) = self.panes.cur.walk.walk.as_mut() {
            if placed {
                w.reveal = 0;
            } else if !retry {
                w.reveal = REVEAL_TRIES;
            }
        }
        cx.notify();
    }

    /// `j`/`k`: the next or the previous in the walk's order.
    fn walk_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let order = self.walk_order();
        let Some(w) = self.panes.cur.walk.walk.as_ref() else { return };
        if order.is_empty() {
            return;
        }
        let from = order.iter().position(|a| *a == w.at);
        let next = match from {
            Some(k) => k as isize + by,
            // Gone from under it: on from where it was.
            None if by > 0 => w.ix as isize,
            None => w.ix as isize - 1,
        }
        .clamp(0, order.len() as isize - 1) as usize;
        if from == Some(next) {
            return;
        }
        let had_rev = w.rev.is_some();
        let now = self.fx.now();
        if let Some(w) = self.panes.cur.walk.walk.as_mut() {
            (w.at, w.ix, w.since, w.rev) = (order[next], next, now, None);
            w.painted.set(None);
        }
        if had_rev {
            self.walk_scratch_drop();
        }
        self.walk_reveal(false, cx);
    }

    /// The walked block's painted target, if the bound board (the newest,
    /// drawn or waiting under the lease) still holds it: its board, the
    /// block, and for a rail section its slot. `None`: not painted yet, or
    /// changed.
    fn walk_target(&self) -> Option<(std::sync::Arc<Drawn>, Shown, Option<cards_spec::block::Slot>)> {
        let w = self.panes.cur.walk.walk.as_ref()?;
        let tag = w.painted.get()?;
        let live = &self.model.lives[self.panes.cur.live];
        let board = live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned())?;
        let bound = |s: &Shown| s.bound.as_ref().is_some_and(|b| b.tag == tag);
        let found = match w.at {
            At::Block(row) => crate::lease::find_any(&board, row).filter(|(_, g)| g.is_none()).map(|(s, _)| (s.clone(), None)),
            At::Rail(row) => [cards_spec::block::Slot::Upper, cards_spec::block::Slot::End].into_iter().find_map(|slot| {
                board.rail[crate::agent_rail::slot_ix(slot)]
                    .iter()
                    .find(|s| crate::blocks::row_id(&s.block.id) == row)
                    .map(|s| (s.clone(), Some(slot)))
            }),
        }
        .filter(|(s, _)| bound(s))?;
        Some((board, found.0, found.1))
    }

    /// The note on the walked block, after the key's release (§6.2: the
    /// release then shows why).
    fn walk_says(&mut self, at: At, note: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.panes.cur.id;
        cx.defer_in(window, move |this, _, cx| {
            match at {
                At::Block(row) => {
                    this.in_pane(pane, |this| this.panes.cur.lease.note = Some((row, note)));
                }
                At::Rail(row) => this.agent_rail.lease.note = Some((row, note)),
            }
            cx.notify();
        });
    }

    /// `z`/`c`: the walked block's local fold (§3.5), on its painted target.
    fn walk_fold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Its ring not painted yet since it came there: nothing to act on.
        let Some(at) = self.panes.cur.walk.walk.as_ref().filter(|w| w.painted.get().is_some()).map(|w| w.at) else { return };
        let Some((_, s, _)) = self.walk_target() else {
            self.walk_says(at, CHANGED, window, cx);
            return;
        };
        if self.fold_now(at, &s, cx) {
            self.panes.cur.walk.push(Undo { at, tag: s.block.tag, act: Undid::Fold });
        }
    }

    /// Block `s` (at `at`) folded or unfolded, locally: in this pane's
    /// transcript, or in the rail. `false`: not drawn here now.
    pub(crate) fn fold_now(&mut self, at: At, s: &Shown, cx: &mut Context<Self>) -> bool {
        match at {
            At::Block(row) => {
                let Some((ix, drawn)) = self.block_shown(row) else { return false };
                self.panes.cur.block_folds.toggle(row, &drawn);
                self.panes.cur.list.remeasure_items(ix..ix + 1);
            }
            At::Rail(_) => {
                let Some(session) = self.rail_sid().map(str::to_string) else { return false };
                let Some((_, lay)) = self.rail_layout() else { return false };
                if lay.section(&s.block.id).is_none() {
                    return false;
                }
                self.agent_rail.local_mut(&session).toggle(&lay, &s.block.id);
            }
        }
        cx.notify();
        true
    }

    /// `x`, `u`, `p`, `m` (and the hover's `×`, `rail`, `⇅`): D6b's, on the
    /// walked block's painted target ([`BlockDesires`]).
    fn walk_wish(&mut self, wish: Wish, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.panes.cur.walk.walk.as_ref().filter(|w| w.painted.get().is_some()).map(|w| w.at) else { return };
        let Some((board, s, slot)) = self.walk_target() else {
            self.walk_says(at, CHANGED, window, cx);
            return;
        };
        if self.wish_now(at, wish, &board, &s, slot, window, cx) {
            self.panes.cur.walk.push(Undo { at, tag: s.block.tag, act: Undid::Wish(wish) });
        }
    }

    /// `wish` on block `s` (at `at`) of board `board`, asked of D6b;
    /// `true`: it took it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn wish_now(
        &mut self,
        at: At,
        wish: Wish,
        board: &Drawn,
        s: &Shown,
        slot: Option<cards_spec::block::Slot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return false };
        let aim = Aim { session: &session, drawn: board, block: s, rail: slot };
        let asked = match wish {
            Wish::Dismiss => self.desires.dismiss(&aim),
            Wish::Show => self.desires.show(&aim),
            Wish::Pin => self.desires.pin(&aim),
            Wish::Slot => self.desires.slot(&aim),
        };
        log::info!(target: "synaps_desktop::walk", "walk: {wish:?} {}: {asked:?}", s.block.id);
        match asked {
            Asked::Done => {
                cx.notify();
                true
            }
            Asked::Off => false,
            Asked::Refused(why) => {
                log::info!(target: "synaps_desktop::walk", "walk: {wish:?} {} refused: {why}", s.block.id);
                self.walk_says(at, REFUSED, window, cx);
                false
            }
        }
    }

    /// Ctrl+Z (Cmd+Z) in the walk: the last local action undone, one level
    /// per block (§3.5), if its block is still the incarnation acted on;
    /// the walk goes to it.
    fn walk_undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(u) = self.panes.cur.walk.undo.pop() else { return };
        let live = &self.model.lives[self.panes.cur.live];
        let board = live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned());
        let found = board.as_deref().and_then(|d| match u.at {
            At::Block(row) => crate::lease::find_any(d, row).filter(|(_, g)| g.is_none()).map(|(s, _)| (s.clone(), None)),
            At::Rail(row) => [cards_spec::block::Slot::Upper, cards_spec::block::Slot::End].into_iter().find_map(|slot| {
                d.rail[crate::agent_rail::slot_ix(slot)]
                    .iter()
                    .find(|s| crate::blocks::row_id(&s.block.id) == row)
                    .map(|s| (s.clone(), Some(slot)))
            }),
        });
        let Some((s, slot)) = found.filter(|(s, _)| s.bound.as_ref().is_some_and(|b| b.tag == u.tag)) else {
            self.walk_says(u.at, CHANGED, window, cx);
            return;
        };
        // The walk goes to it.
        let order = self.walk_order();
        if let Some(k) = order.iter().position(|a| *a == u.at) {
            let now = self.fx.now();
            if let Some(w) = self.panes.cur.walk.walk.as_mut().filter(|w| w.at != u.at) {
                (w.at, w.ix, w.since, w.rev) = (u.at, k, now, None);
                w.painted.set(None);
            }
        }
        match u.act {
            Undid::Fold => {
                self.fold_now(u.at, &s, cx);
            }
            Undid::Wish(wish) => {
                let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return };
                let Some(board) = board else { return };
                let aim = Aim { session: &session, drawn: &board, block: &s, rail: slot };
                if let Asked::Refused(why) = self.desires.undo(&aim, wish) {
                    log::info!(target: "synaps_desktop::walk", "walk: undo {wish:?} {} refused: {why}", s.block.id);
                    self.walk_says(u.at, REFUSED, window, cx);
                }
            }
        }
        log::info!(target: "synaps_desktop::walk", "walk: undid {:?} on {:?}", u.act, u.at);
        self.walk_reveal(false, cx);
    }

    /// `[` (`back`) and `]`: the walked block's revisions (§3.6), newest
    /// first; `]` from the newest past one, the latest (the view closes).
    fn walk_rev(&mut self, back: bool, cx: &mut Context<Self>) {
        let Some(w) = self.panes.cur.walk.walk.as_ref() else { return };
        let (at, cur) = (w.at, w.rev);
        let Some(d) = self.model.lives[self.panes.cur.live].transcript.drawn().cloned() else { return };
        let s = match at {
            At::Block(row) => crate::lease::find_any(&d, row).filter(|(_, g)| g.is_none()).map(|(s, _)| s.clone()),
            At::Rail(row) => d.rail.iter().flatten().find(|s| crate::blocks::row_id(&s.block.id) == row).cloned(),
        };
        let Some(s) = s.filter(|s| !s.block.pending) else { return };
        // Of the incarnation the view opened on (no yanking), else the
        // current one; the one in view found by identity (a revision
        // landed since is newer, never in its place).
        let tag = cur.map_or(s.block.tag, |v| v.tag);
        let latest = cur.map_or(s.block.rev, |v| v.latest);
        let revs = revisions::revisions(&d, &s.block.id, tag);
        let ix =
            cur.and_then(|v| revs.iter().position(|r| r.tag == v.rev.tag && r.rev == v.rev.rev && r.earlier == v.rev.earlier)).unwrap_or(0);
        let next = if back { (ix + 1).min(revs.len().saturating_sub(1)) } else { ix.saturating_sub(1) };
        // Forward to the view's latest (or past it): the latest, the view
        // closed.
        let past = next > 0 && !(revs[next].tag == tag && !revs[next].earlier && revs[next].rev >= latest);
        if next == ix && cur.is_some() == past {
            return;
        }
        let view = past.then(|| RevView { rev: revs[next], ix: next, latest, tag });
        if let Some(w) = self.panes.cur.walk.walk.as_mut() {
            w.rev = view;
        }
        match view {
            Some(v) => {
                self.panes.cur.walk.refolding = true;
                self.refold(self.panes.cur.live, Refold::Revision(v.rev.at), cx);
            }
            None => {
                self.walk_scratch_drop();
            }
        }
        cx.notify();
    }

    /// A key down on the pane's walk focus: see the module.
    pub(crate) fn walk_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let mac = cfg!(target_os = "macos");
        let undo = chord(ks, "z", mac);
        // Any other chord is the app's.
        if (m.control || m.alt || m.platform || m.function) && !undo {
            return;
        }
        let Some(shown) = self.panes.cur.walk.walk.as_ref().map(|w| w.shown.get()) else { return };
        let key = ks.key.as_str();
        let text = ks.key_char.clone().filter(|c| !c.is_empty() && !c.chars().any(char::is_control));
        let ours = undo || matches!(key, "j" | "k" | "down" | "up" | "z" | "c" | "x" | "u" | "p" | "m" | "[" | "]" | "e" | "escape");
        cx.stop_propagation();
        if !shown {
            // Not painted yet: Esc and a printable key leave, the key
            // typed into the composer; nothing acts.
            if text.is_some() || key == "escape" {
                self.walk_leave(true, window, cx);
                if let Some(t) = text {
                    let c = self.panes.cur.composer.clone();
                    c.update(cx, |s, cx| EntityInputHandler::replace_text_in_range(s, None, &t, window, cx));
                }
            }
            return;
        }
        if !ours || (m.shift && key != "escape") {
            // Another printable key: typed into the composer, as a card's.
            if let Some(t) = text {
                self.walk_leave(true, window, cx);
                let c = self.panes.cur.composer.clone();
                c.update(cx, |s, cx| EntityInputHandler::replace_text_in_range(s, None, &t, window, cx));
            }
            return;
        }
        // Only a new, bare press acts (Ctrl+Z: a new press).
        let fresh = if undo { self.ask_presses.last } else { self.ask_presses.acts };
        if !fresh {
            log::debug!(target: "synaps_desktop::walk", "walk key {key}: not a new, bare press");
            return;
        }
        if undo {
            self.walk_undo(window, cx);
            return;
        }
        match key {
            "escape" => {
                // A past revision first goes back to the latest.
                if self.panes.cur.walk.walk.as_ref().is_some_and(|w| w.rev.is_some()) {
                    if let Some(w) = self.panes.cur.walk.walk.as_mut() {
                        w.rev = None;
                    }
                    self.walk_scratch_drop();
                    cx.notify();
                } else {
                    self.walk_leave(true, window, cx);
                }
            }
            "j" | "down" => self.walk_step(1, cx),
            "k" | "up" => self.walk_step(-1, cx),
            "z" | "c" => self.walk_fold(window, cx),
            "[" => self.walk_rev(true, cx),
            "]" => self.walk_rev(false, cx),
            "x" => self.walk_wish(Wish::Dismiss, window, cx),
            "u" => self.walk_wish(Wish::Show, window, cx),
            "p" => self.walk_wish(Wish::Pin, window, cx),
            "m" => self.walk_wish(Wish::Slot, window, cx),
            // `e` takes the block: D8's.
            _ => {}
        }
    }

    /// The pane's walk hooks, on its element: the focus that holds the
    /// walk's keys (zero-sized, always drawn once made: a focus lost hands
    /// the keys to the window's, which answer a prompt).
    pub(crate) fn walk_hooks(&self, d: Div, cx: &mut Context<Self>) -> Div {
        let pane = self.panes.cur.id;
        let focus = self.panes.cur.walk.focus.clone().map(|f| {
            div().absolute().size_0().track_focus(&f).key_context(WALK_CTX).on_key_down(cx.listener(
                move |this, ev: &KeyDownEvent, window, cx| {
                    this.in_pane(pane, |this| this.walk_key(ev, window, cx));
                },
            ))
        });
        d.children(focus)
    }

    // ── drawn ─────────────────────────────────────────────────────────

    /// The walk ring on block or section row `row` (painted as `tag`), if
    /// the walk of the pane acted on is there: a 1 px accent outline over
    /// the block (its spine stays), fading in; painted in view, its fade
    /// begun (as an entered card's growth: a frame of the motion clock),
    /// its keys act ([`Walk::shown`]).
    pub(crate) fn walk_ring(&self, at: At, tag: Tag, p: &Palette) -> Option<AnyElement> {
        let w = self.panes.cur.walk.walk.as_ref().filter(|w| w.at == at)?;
        let t = self.fx.shot(Some(w.since), Duration::ZERO, motion::QUICK, &motion::ease::OUT, motion::Kind::Fade);
        // Its fade begun: a frame of the motion clock since it came (or no
        // fade: Off).
        let begun = t >= 1. || self.fx.now() > w.since;
        let (painted, shown, seen) = (w.painted.clone(), w.shown.clone(), w.seen.clone());
        let row = at.row();
        Some(
            div()
                .absolute()
                .inset_0()
                .border_1()
                // Fading in from its card's ground (both grounds are
                // `card_d`), never an opacity (design.md §5).
                .border_color(motion::mix(p.card_d, p.acc, t))
                .debug_selector(move || format!("walk-ring {row}"))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, (), window, _| {
                            // Painted means seen: in view, and its fade begun
                            // (a frame of the motion clock since it came).
                            let in_view = bounds.intersect(&window.content_mask().bounds);
                            if begun && in_view.size.width > px(0.) && in_view.size.height > px(0.) {
                                painted.set(Some(tag));
                                shown.set(true);
                                seen.set(true);
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element(),
        )
    }

    /// The walk's status line (module docs), at the list's foot in the
    /// pane walking: its keys, or the revision in view.
    pub(crate) fn walk_status(&self, p: &Palette) -> Option<AnyElement> {
        let w = self.panes.cur.walk.walk.as_ref()?;
        let text = match w.rev {
            Some(_) => {
                let label = self.walk_past(w.at.row()).map(|pv| pv.label).unwrap_or_default();
                format!("{label} · [ older · ] newer · esc latest")
            }
            None => status_keys(self.desires.wired(), cfg!(target_os = "macos")),
        };
        let t = text.clone();
        Some(
            h_flex()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(STATUS_H))
                .px(px(12.))
                .gap(px(10.))
                .items_center()
                .whitespace_nowrap()
                .bg(p.chrome)
                .border_t_1()
                .border_color(p.frame)
                .debug_selector(move || format!("walk-status {t}"))
                .child(crate::app::display("WALK", size::D_S, p.acc).flex_none())
                .child(mono(text, size::META, p.dim_k).min_w_0().overflow_hidden().text_ellipsis())
                .into_any_element(),
        )
    }

    /// The past revision drawn for row `row` in the pane acted on, if its
    /// walk shows one (module docs, §3.6).
    pub(crate) fn walk_past(&self, row: u64) -> Option<Past> {
        let w = self.panes.cur.walk.walk.as_ref().filter(|w| w.at.row() == row)?;
        let v = w.rev?;
        let live = self.panes.cur.live;
        let l = &self.model.lives[live];
        let time = v
            .rev
            .row
            .and_then(|r| l.transcript.items().iter().find(|i| i.id == r))
            .and_then(|i| i.at.as_deref())
            .and_then(crate::times::Stamp::local);
        let mut label = revisions::label(&v.rev, v.latest, time.as_ref().map(|t| t.hhmm.as_ref()));
        let now = l.transcript.drawn().and_then(|d| match w.at {
            At::Block(r) => crate::lease::find_any(d, r).map(|(s, _)| (s.block.tag, s.block.rev)),
            At::Rail(r) => d.rail.iter().flatten().find(|s| crate::blocks::row_id(&s.block.id) == r).map(|s| (s.block.tag, s.block.rev)),
        });
        // No yanking: a later revision landed meanwhile says so.
        let newer = now.filter(|(tag, rev)| *tag != v.tag || *rev > v.latest).map(|(_, rev)| rev);
        let done = self.refolds.done.as_ref().filter(|d| d.live == live && d.target == Refold::Revision(v.rev.at));
        let shown = match done.map(|d| &d.state) {
            Some(Ok(Some(st))) => past_shown(st, &row_ref(l.transcript.drawn().map(|d| &**d), w.at)?, v.rev.tag),
            Some(Ok(None)) => {
                label.push_str(" · not shown");
                None
            }
            Some(Err(_)) => {
                label = format!("r{} · too far back to show", v.rev.rev);
                None
            }
            None => None,
        };
        Some(Past { label, newer, shown })
    }

    /// The motion keys' salt for row `row` in the pane acted on: a past
    /// revision's own (0: the block's).
    pub(crate) fn past_salt(&self, row: u64) -> u64 {
        let Some(w) = self.panes.cur.walk.walk.as_ref().filter(|w| w.at.row() == row) else { return 0 };
        w.rev.map_or(0, |v| past_salt(row, &v.rev))
    }

    /// The pointer over block row `row`'s header in pane `pane`, or off it:
    /// its hover actions show (a frame: the user's own).
    pub(crate) fn head_hover(&mut self, pane: crate::pane::PaneId, row: u64, over: bool, cx: &mut Context<Self>) {
        let now = self.fx.now();
        self.in_pane(pane, |this| {
            let l = &mut this.panes.cur.lease;
            if over {
                l.head = Some((row, now));
            } else if l.head.is_some_and(|h| h.0 == row) {
                l.head = None;
            }
        });
        cx.notify();
    }

    /// The hover actions over a header's right end (module docs), for row
    /// `row` hovered since `since`, on `ground`: `rev` (a block with
    /// revisions), D6b's when wired, `fold`. Each a painted target, `press`
    /// making the press for its act.
    pub(crate) fn hover_actions(
        &self,
        row: u64,
        since: Instant,
        acts: &[(Hover, &'static str)],
        ground: Hsla,
        p: &Palette,
        mut on: impl FnMut(Hover, Stateful<Div>) -> Stateful<Div>,
    ) -> Div {
        let t = self.fx.shot(Some(since), Duration::ZERO, motion::QUICK, &motion::ease::OUT, motion::Kind::Fade);
        let ink = motion::mix(ground, p.dim_t, t);
        let hot = motion::mix(ground, p.prose_t, t);
        let mut row_el = h_flex()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .pl(px(8.))
            .gap(px(8.))
            .items_center()
            .bg(ground)
            .font_family(fonts::MONO)
            .text_px(size::META)
            .whitespace_nowrap()
            .debug_selector(move || format!("hover-acts {row}"));
        for (k, (act, label)) in acts.iter().enumerate() {
            if k > 0 {
                row_el = row_el.child(div().text_color(ink).child("·"));
            }
            let (act, label) = (*act, *label);
            let el = div()
                .id(SharedString::from(format!("hover-{label}-{row}")))
                .cursor_pointer()
                .text_color(ink)
                .hover(move |s| s.text_color(hot))
                .debug_selector(move || format!("hover-act {row} {label}"))
                .child(label);
            row_el = row_el.child(on(act, el));
        }
        row_el
    }

    /// The hover actions block `s` offers (module docs): `rev` if it has a
    /// past revision, D6b's when wired (`rail` a transcript block, `⇅` a
    /// rail section, `×`), `fold`.
    pub(crate) fn hover_offers(&self, s: &Shown, rail: bool) -> Vec<(Hover, &'static str)> {
        let mut v = Vec::new();
        if !s.block.pending && (s.block.rev > 1 || s.block.replaced.is_some()) {
            v.push((Hover::Rev, "rev"));
        }
        if self.desires.wired() {
            v.push((Hover::Pin, "rail"));
            if rail {
                v.push((Hover::Slot, "⇅"));
            }
        }
        v.push((Hover::Fold, "fold"));
        if self.desires.wired() {
            v.push((Hover::Dismiss, "×"));
        }
        v
    }

    /// A hover action acted (its press held to its up, on its painted
    /// target, as [`AppView::block_up`] checked it): `rev` enters the walk
    /// on the block, its previous revision in view; D6b's ask D6b.
    pub(crate) fn hover_act(&mut self, at: At, act: Hover, s: &Shown, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Hover::Fold => {
                if self.fold_now(at, s, cx) {
                    self.panes.cur.walk.push(Undo { at, tag: s.block.tag, act: Undid::Fold });
                }
            }
            Hover::Rev => {
                if self.walk_to(at, window, cx) {
                    self.walk_rev(true, cx);
                }
            }
            Hover::Pin | Hover::Slot | Hover::Dismiss => {
                let wish = match act {
                    Hover::Pin => Wish::Pin,
                    Hover::Slot => Wish::Slot,
                    _ => Wish::Dismiss,
                };
                let live = &self.model.lives[self.panes.cur.live];
                let Some(board) = live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned()) else { return };
                let slot = match at {
                    At::Rail(_) => board
                        .rail
                        .iter()
                        .position(|l| l.iter().any(|x| x.block.id == s.block.id))
                        .map(|k| if k == 0 { cards_spec::block::Slot::Upper } else { cards_spec::block::Slot::End }),
                    At::Block(_) => None,
                };
                if self.wish_now(at, wish, &board, s, slot, window, cx) {
                    self.panes.cur.walk.push(Undo { at, tag: s.block.tag, act: Undid::Wish(wish) });
                }
            }
        }
    }

    /// Pointer-up on block row `row`'s hover action `act` in the pane acted
    /// on: as [`AppView::block_up`], the press's target acts only if it is
    /// this one and the bound board still holds it; else nothing, and the
    /// note.
    pub(crate) fn hover_up(&mut self, row: u64, act: Hover, window: &mut Window, cx: &mut Context<Self>) {
        let l = &mut self.panes.cur.lease;
        l.pressed = false;
        let Some(down) = l.down.take().filter(|d| d.at.row == row && d.act == crate::lease::Act::Hover(act)) else { return };
        let live = &self.model.lives[self.panes.cur.live];
        let board = live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned());
        let held = board
            .as_deref()
            .and_then(|d| crate::lease::find_any(d, row))
            .is_some_and(|(s, g)| g.is_none() && s.bound.as_ref().is_some_and(|b| b.tag == down.at.tag));
        match self.block_shown(row) {
            Some((_, s)) if held => self.hover_act(At::Block(row), act, &s, window, cx),
            _ => self.panes.cur.lease.note = Some((row, CHANGED)),
        }
        if self.model.lives.ids().into_iter().any(|l| self.model.lives[l].board.waiting.is_some()) {
            self.arm_lease_wake(cx);
        }
        cx.notify();
    }

    /// Pointer-up on rail section `row`'s hover action `act`: as
    /// [`AppView::rail_up`].
    pub(crate) fn rail_hover_up(&mut self, row: u64, act: Hover, window: &mut Window, cx: &mut Context<Self>) {
        use crate::agent_rail_view::RailAct;
        self.agent_rail.lease.pressed = false;
        let Some(down) = self.agent_rail.down.take().filter(|d| d.at.row == row && d.act == RailAct::Hover(act)) else { return };
        let live = self.panes.cur.live;
        let l = &self.model.lives[live];
        let newest = l.board.waiting.clone().or_else(|| l.transcript.drawn().cloned());
        let held = newest.as_deref().map(|d| self.rail_of(live, d)).is_some_and(|b| {
            b.sections().any(|s| crate::blocks::row_id(&s.s.block.id) == row && s.s.bound.as_ref().is_some_and(|x| x.tag == down.at.tag))
        });
        let drawn =
            self.rail_layout().and_then(|(_, l)| l.sections().find(|s| crate::blocks::row_id(&s.s.block.id) == row).map(|s| s.s.clone()));
        match drawn.filter(|_| held) {
            Some(s) => self.hover_act(At::Rail(row), act, &s, window, cx),
            None => self.agent_rail.lease.note = Some((row, CHANGED)),
        }
        if self.model.lives.ids().into_iter().any(|l| self.model.lives[l].board.waiting.is_some()) {
            self.arm_lease_wake(cx);
        }
        cx.notify();
    }

    /// The pointer over rail section `row`'s header, or off it.
    pub(crate) fn rail_head_hover(&mut self, row: u64, over: bool, cx: &mut Context<Self>) {
        let now = self.fx.now();
        let l = &mut self.agent_rail.lease;
        if over {
            l.head = Some((row, now));
        } else if l.head.is_some_and(|h| h.0 == row) {
            l.head = None;
        }
        cx.notify();
    }

    /// The walk entered on `at` in the pane acted on (a hover's `rev`, the
    /// op log's jump): `false` if it isn't in the walk here.
    pub(crate) fn walk_to(&mut self, at: At, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let order = self.walk_order();
        let Some(k) = order.iter().position(|a| *a == at) else { return false };
        if self.panes.cur.walk.walk.is_none() && !self.walk_enter(window, cx) {
            return false;
        }
        let now = self.fx.now();
        let had_rev = self.panes.cur.walk.walk.as_ref().is_some_and(|w| w.rev.is_some() && w.at != at);
        if let Some(w) = self.panes.cur.walk.walk.as_mut().filter(|w| w.at != at) {
            (w.at, w.ix, w.since, w.rev) = (at, k, now, None);
            w.painted.set(None);
        }
        if had_rev {
            self.walk_scratch_drop();
        }
        if let Some(f) = self.panes.cur.walk.focus.clone() {
            window.focus(&f, cx);
        }
        self.walk_reveal(false, cx);
        true
    }
}

/// A D6b action refused (its reason in the log).
pub(crate) const REFUSED: &str = "not done · try again";

/// The status line's height.
pub(crate) const STATUS_H: f32 = 24.;

/// A walked block brought into view clears the list's edges by this.
const REVEAL_PAD: f32 = 12.;

/// How many frames a walked block's bringing into view is tried again.
const REVEAL_TRIES: u8 = 4;

/// The walk's keys, as the status line names them.
pub(crate) fn status_keys(wired: bool, mac: bool) -> String {
    let undo = if mac { "⌘Z undo" } else { "Ctrl+Z undo" };
    let mut s = String::from("j k walk · z fold · [ ] revisions");
    if wired {
        s.push_str(" · x dismiss · u show · p pin · m slot");
    }
    s.push_str(" · ");
    s.push_str(undo);
    s.push_str(" · esc composer");
    s
}

/// A hover action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hover {
    Rev,
    Pin,
    Slot,
    Fold,
    Dismiss,
}

/// A past revision as drawn (§3.6): its label, a later revision landed
/// meanwhile (`rN new`), and its block from the re-fold scratch once there.
pub(crate) struct Past {
    pub label: String,
    pub newer: Option<u32>,
    pub shown: Option<Shown>,
}

fn past_salt(row: u64, r: &Rev) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (row, r.tag, r.rev, PAST).hash(&mut h);
    (h.finish() | 1) & (crate::blocks::BLOCK_ID - 1)
}

/// Row `at`'s block id on board `d`.
fn row_ref(d: Option<&Drawn>, at: At) -> Option<Ref> {
    let d = d?;
    match at {
        At::Block(r) => crate::lease::find_any(d, r).map(|(s, _)| s.block.id.clone()),
        At::Rail(r) => d.rail.iter().flatten().find(|s| crate::blocks::row_id(&s.block.id) == r).map(|s| s.block.id.clone()),
    }
}

/// Block `id` of incarnation `tag` on past board `st`, as a row draws it
/// (bound: it was; a group with its members).
fn past_shown(st: &State, id: &Ref, tag: Tag) -> Option<Shown> {
    let b = st.block(id).filter(|b| b.tag == tag)?;
    let arc = std::sync::Arc::new(b.clone());
    let members = b
        .members()
        .iter()
        .filter_map(|m| {
            let mb = st.block(&Ref::Block(m.clone()))?;
            let a = std::sync::Arc::new(mb.clone());
            Some(Shown { block: a.clone(), bound: Some(a), members: Vec::new() })
        })
        .collect();
    Some(Shown { block: arc.clone(), bound: Some(arc), members })
}

#[cfg(test)]
mod tests {
    use gpui_kit::Keystroke;
    use synaps_client::blocks::Tag;

    use super::{At, BlockDesires as _, NoDesires, UNDO_MAX, Undid, Undo, WalkMode, Wish, chord, status_keys};

    fn ks(s: &'static str) -> Keystroke {
        Keystroke::parse(s).expect("a keystroke")
    }

    /// Ctrl+G and Ctrl+O (Cmd on macOS): only the chord itself.
    #[test]
    fn the_walk_and_op_log_chords_are_ctrl_or_cmd_alone() {
        assert!(chord(&ks("ctrl-g"), "g", false) && chord(&ks("cmd-g"), "g", true));
        assert!(!chord(&ks("cmd-g"), "g", false) && !chord(&ks("ctrl-g"), "g", true));
        assert!(!chord(&ks("ctrl-shift-g"), "g", false) && !chord(&ks("ctrl-alt-o"), "o", false));
        assert!(!chord(&ks("g"), "g", false) && !chord(&ks("ctrl-j"), "g", false));
        assert!(chord(&ks("ctrl-o"), "o", false) && chord(&ks("ctrl-z"), "z", false));
    }

    /// §3.5: one level per block, the newest last, bounded.
    #[test]
    fn undo_keeps_one_level_per_block() {
        let tag = Tag::parse("00000000000000000000000000000001").expect("a tag");
        let mut w = WalkMode::default();
        w.push(Undo { at: At::Block(1), tag, act: Undid::Fold });
        w.push(Undo { at: At::Block(2), tag, act: Undid::Fold });
        w.push(Undo { at: At::Block(1), tag, act: Undid::Wish(Wish::Pin) });
        assert_eq!(
            w.undo.iter().map(|u| (u.at, u.act)).collect::<Vec<_>>(),
            [(At::Block(2), Undid::Fold), (At::Block(1), Undid::Wish(Wish::Pin))]
        );
        for r in 0..100 {
            w.push(Undo { at: At::Rail(r), tag, act: Undid::Fold });
        }
        assert_eq!(w.undo.len(), UNDO_MAX);
        assert_eq!(w.undo.last().map(|u| u.at), Some(At::Rail(99)));
    }

    #[test]
    fn the_status_line_names_d6bs_keys_only_once_wired() {
        assert_eq!(status_keys(false, false), "j k walk · z fold · [ ] revisions · Ctrl+Z undo · esc composer");
        assert!(status_keys(true, true).contains("x dismiss · u show · p pin · m slot") && status_keys(true, true).contains("⌘Z"));
    }

    /// D7's default answers nothing.
    #[test]
    fn no_desires_does_nothing() {
        let d = NoDesires;
        assert!(!d.wired());
    }
}
