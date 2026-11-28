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
//!   row and incarnation (and a tombstone's token) as the ring last
//!   painted them ([`Walk::painted`]), checked against the bound board,
//!   the newest drawn or waiting under the lease, *before* the lease
//!   releases (the root's capture defers the release to after the key):
//!   folds, revisions (`[` and `]`, an open view's too) and D6b's keys
//!   alike. A block whose incarnation differs now, or gone, does nothing
//!   and says `this block changed · look again`; the release then shows
//!   why. A key just after `j`/`k`, before the ring was painted on the
//!   block reached, does nothing. A key names its target by identity,
//!   never by position, so the just-moved rule never blocks one.
//! - **Targets are found where this desk draws them** ([`Found`]): a
//!   transcript row's block, or a rail section of the rail's layout,
//!   after the user's pins and slot moves and as a tombstone (D6a's
//!   `RailVisibility`, D6b's to answer), on the bound board. What D6b is
//!   asked carries the effective placement ([`BlockDesires::placed`]:
//!   the user's, else the agent's) and the tombstone's token as painted.
//!   A tombstone is walked to; only `x` and `u` act on it.
//! - **Acting.** `z` or `c` folds the block (local, §3.5); `[` and `]`
//!   walk its revisions (§3.6: [`crate::revisions`], the past body drawn
//!   from the re-fold scratch, one at a time; `]` or Esc back to the
//!   latest, and no yanking: an agent op landing meanwhile moves nothing,
//!   a dot says `r6 new`; an open view steps on through its own
//!   incarnation's revisions, history whatever replaced it since). A past
//!   label never wears the live body: until the scratch has it the block
//!   says `loading r2…`, and `r2 unavailable` if it can't be had
//!   ([`PastBody`]). `x` dismisses, `u` shows again, `p` pins, `m`
//!   moves a rail section to the other slot: D6b's desires and stores
//!   (#103), through [`BlockDesires`] (`crate::rail_wishes::Stores`
//!   answers it), where this desk draws what they do: `x` and `u` on any
//!   block (a dismissed rail section is hidden, a transcript block draws
//!   its tombstone, which is walked to and takes only `x` and `u`), `p`
//!   on a transcript block (into the rail, its stub left) and on an
//!   agent's rail section (into the transcript, at its creating call),
//!   and back, `m` on a rail section ([`BlockDesires::offers`]). The
//!   palette's block commands (`x`, `u`, `p`) act on the block walked
//!   when it opened, as painted ([`AppView::wish_at`]). `e` (take) and
//!   `R` (restore) are D8's: nothing yet.
//! - **Undo** (§3.5): Ctrl+Z (Cmd+Z) in the walk undoes the last local
//!   action, one level per block: a fold (a key's or a click's, in the
//!   transcript or the rail), and through [`BlockDesires::undo`] a pin, a
//!   slot move or a dismissal not yet sent, with D6b's token for it (its
//!   block found on the board wherever it is, a dismissed one hidden). It
//!   is the inverse of what the action set, never a toggle: it puts back
//!   what was there before (an expansion the rail's cap folded back for
//!   it too), only while the block still shows what the action set, and
//!   all or nothing: it checks every section it writes, all on the
//!   newest bound board (one waiting under the lease included). An agent
//!   `collapse` since, the rail's cap since, or the expansion it folded
//!   back replaced, made again or collapsed by the agent since, makes it
//!   a no-op that says so ([`STALE`]); another incarnation of the block
//!   itself says `this block changed`. The walk goes to it.
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
use cards_spec::board::{Block, Place, State, Target as Refold};
use gpui_kit::component::h_flex;
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Shown, Tag};

use crate::agent_rail::Toggled;
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
/// revisions ([`revisions::revisions`], newest first), the block's
/// revision and incarnation when the view opened (a later one: `rN new`),
/// the block it is of (by identity, never looked for again), and its
/// row's time, read once as it opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RevView {
    pub rev: Rev,
    pub ix: usize,
    pub latest: u32,
    pub tag: Tag,
    pub id: Ref,
    pub time: Option<String>,
}

/// The ring's last paint of the walked block (§6.2): its incarnation, and
/// the token of the tombstone it was drawn as, if it was (D6b's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Painted {
    pub tag: Tag,
    pub tomb: Option<u64>,
}

/// The walk in a pane.
pub(crate) struct Walk {
    pub at: At,
    /// Its place in the walk's order when it came there: a block gone
    /// from under it, `j`/`k` go on from there.
    pub ix: usize,
    /// The session it walks.
    pub session: String,
    /// The ring's last paint ([`Painted`], §6.2): none since it came to
    /// `at`, no key acts on it.
    pub painted: Rc<Cell<Option<Painted>>>,
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

/// What a local action set, for its undo (§3.5): enough to put back what
/// was there, and to know it still holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Undid {
    /// A transcript block's fold: the pane's fold of it before (none: the
    /// agent's), and the one it set (with the agent's `collapse` it was
    /// made against).
    Fold { before: Option<crate::blocks::Fold>, after: crate::blocks::Fold },
    /// A rail section's fold: what it changed of the user's folds (an
    /// expansion the cap folded back for it included), and whether the
    /// section drew open after it.
    RailFold { toggled: Toggled, open: bool },
    /// D6b's: the wish, and D6b's token for what it did.
    Wish(Wish, u64),
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// bound board ([`Found`]): its session, conversation (the transcript's
/// journal, D6b's key) and board, the block, where it paints on this desk
/// (its effective placement: a section's slot as the rail lays it out,
/// the user's pin or slot move included; a transcript block's,
/// [`BlockDesires::placed`]), and the token of the tombstone it was
/// painted as (D6b's: a rail section's soft dismissal, a transcript
/// block's dismissal or soft one), if it was.
pub(crate) struct Aim<'a> {
    #[cfg_attr(not(test), expect(dead_code, reason = "D7's tests' stand-in reads it"))]
    pub session: &'a str,
    pub conv: &'a str,
    pub drawn: &'a Drawn,
    pub block: &'a Shown,
    pub place: Place,
    pub tomb: Option<u64>,
}

/// How D6b answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    /// Not wired (D7's default), or not where this desk draws it
    /// ([`BlockDesires::offers`]): nothing happened.
    Off,
    /// Accepted (D6b draws it), with D6b's token for what it did: its
    /// undo names it ([`BlockDesires::undo`]).
    Done(u64),
    /// Refused, with why (a cap, a write that failed): the block says
    /// `not done · try again`, the log why.
    Refused(String),
}

/// The walk's `x`, `u`, `p` and `m`, the hover's `×`, `rail` and `⇅`, and
/// their undo: D6b's dismiss and show desires and its pins and slot moves
/// (#103), answered by `crate::rail_wishes::Stores`. D7 routes the keys
/// here; the default (tests' `NoDesires`) does nothing, and the status line
/// and the hover don't offer them until [`BlockDesires::wired`]. And the
/// effective placement ([`BlockDesires::placed`]): where a block paints on
/// this desk after the user's pins and slot moves.
pub(crate) trait BlockDesires {
    /// The keys act: their hints and buttons show.
    fn wired(&self) -> bool {
        false
    }
    /// Whether the hover offers `wish` on block `b` (a rail section's
    /// header, `rail`, or a transcript block's): where this desk draws what
    /// it does. Wired, everywhere (the default).
    fn offers(&self, _wish: Wish, _b: &Block, _rail: bool) -> bool {
        self.wired()
    }
    /// Where block `b` of conversation `conv` paints on this desk: the
    /// user's pin or slot move (D6b's presentation), else the agent's
    /// placement (the default). The rail draws the same through D6a's
    /// `RailVisibility`, which D6b answers from the same store.
    fn placed(&self, _conv: &str, b: &Block) -> Place {
        b.place
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
    /// Undo `wish` on `aim`, its token as D6b gave it (§3.5: a pin, a slot
    /// move, a dismissal not yet sent; a delivered one is undone with
    /// Show): D6b refuses one that no longer holds, and one `wish` isn't
    /// offered for now ([`BlockDesires::offers`]: the block moved since).
    fn undo(&mut self, _: &Aim, _: Wish, _token: u64) -> Asked {
        Asked::Off
    }
}

/// D7's default, nothing wired (tests: the app has D6b's stores).
#[cfg(test)]
pub(crate) struct NoDesires;

#[cfg(test)]
impl BlockDesires for NoDesires {}

// ── the walk ──────────────────────────────────────────────────────────

impl AppView {
    /// D6b's side of the seam: its stores (tests: a stand-in, if set).
    pub(crate) fn block_desires(&self) -> &dyn BlockDesires {
        #[cfg(test)]
        if let Some(d) = self.desires.as_deref() {
            return d;
        }
        &self.wishes
    }

    pub(crate) fn block_desires_mut(&mut self) -> &mut dyn BlockDesires {
        #[cfg(test)]
        if let Some(d) = self.desires.as_deref_mut() {
            return d;
        }
        &mut self.wishes
    }

    /// The walk's order in the pane acted on (module docs): its block rows
    /// in reading order, then the rail's sections as drawn, upper first
    /// (a tombstone too: `u` shows it again), if the rail is this pane's
    /// and can be drawn.
    pub(crate) fn walk_order(&self) -> Vec<At> {
        let mut out: Vec<At> =
            self.panes.cur.disp.iter().filter(|d| matches!(d.kind, Kind::Block { .. })).map(|d| At::Block(d.id)).collect();
        if self.walk_rail_reachable()
            && let Some((_, lay)) = self.rail_layout()
        {
            use crate::agent_rail::State as S;
            out.extend(
                lay.sections()
                    .filter(|s| matches!(s.state, S::Open | S::Summary | S::Listed | S::Tombstone(_)))
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
        match w.rev.as_ref().map(|rv| rv.rev.at) {
            Some(at) => {
                self.panes.cur.walk.refolding = true;
                self.refold(self.panes.cur.live, Refold::Revision(at), cx);
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

    /// The board keys are checked against (§6.2): the newest, drawn or
    /// waiting under the lease.
    pub(crate) fn bound_board(&self) -> Option<std::sync::Arc<Drawn>> {
        let live = &self.model.lives[self.panes.cur.live];
        live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned())
    }

    /// Walk target `at` on board `d` as this desk draws it (module docs):
    /// a transcript row's top-level block, its placement D6b's to say
    /// ([`BlockDesires::placed`]); a rail section as the rail lays `d` out
    /// (the user's pins and slot moves, tombstones), in its slot then.
    pub(crate) fn find_on(&self, d: &Drawn, at: At) -> Option<Found> {
        match at {
            At::Block(row) => {
                let s = crate::lease::find_any(d, row).filter(|(_, g)| g.is_none())?.0.clone();
                self.model.lives[self.panes.cur.live].active.as_ref()?;
                let conv = self.conversation(self.panes.cur.live);
                // Its transcript tombstone's token, as the board has it.
                let tomb = match self.look_on(d, &s) {
                    crate::rail_wishes::Look::Tomb { token, .. } => Some(token),
                    _ => None,
                };
                Some(Found { place: self.block_desires().placed(&conv, &s.block), s, tomb })
            }
            At::Rail(row) => {
                let lay = self.rail_of(self.panes.cur.live, d);
                let sec = lay.sections().find(|x| crate::blocks::row_id(&x.s.block.id) == row)?;
                let tomb = match &sec.state {
                    crate::agent_rail::State::Tombstone(t) => Some(t.token),
                    _ => None,
                };
                Some(Found { s: sec.s.clone(), place: Place::Rail(sec.slot), tomb })
            }
        }
    }

    /// The walked block's painted target, if the bound board still holds
    /// it as painted (its incarnation, its tombstone's token): the board,
    /// and the block as found there. `None`: not painted since it came
    /// there, or changed.
    fn walk_target(&self) -> Option<(std::sync::Arc<Drawn>, Found)> {
        let w = self.panes.cur.walk.walk.as_ref()?;
        self.target_at(w.at, w.painted.get()?)
    }

    /// Target `at` as `painted`, if the bound board still holds it so.
    pub(crate) fn target_at(&self, at: At, painted: Painted) -> Option<(std::sync::Arc<Drawn>, Found)> {
        let board = self.bound_board()?;
        let f = self.find_on(&board, at)?;
        (f.s.bound.as_ref().is_some_and(|b| b.tag == painted.tag) && f.tomb == painted.tomb).then_some((board, f))
    }

    /// The walked block in the pane acted on, as its ring last painted it
    /// (the palette's commands act on it, [`AppView::wish_at`]).
    pub(crate) fn walked(&self) -> Option<(At, Painted)> {
        let w = self.panes.cur.walk.walk.as_ref()?;
        Some((w.at, w.painted.get()?))
    }

    /// The walked block, if its ring was painted on it since it came there
    /// (else no key acts on it).
    fn walk_painted_at(&self) -> Option<At> {
        self.panes.cur.walk.walk.as_ref().filter(|w| w.painted.get().is_some()).map(|w| w.at)
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

    /// `z`/`c`: the walked block's local fold (§3.5), on its painted target
    /// (a tombstone has none).
    fn walk_fold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Its ring not painted yet since it came there: nothing to act on.
        let Some(at) = self.walk_painted_at() else { return };
        let Some((_, f)) = self.walk_target() else {
            self.walk_says(at, CHANGED, window, cx);
            return;
        };
        // A tombstone or a stub draws no body to fold.
        let line = matches!(at, At::Block(_)) && self.drawn_look(&f.s) != crate::rail_wishes::Look::Full;
        if f.tomb.is_none() && !line {
            self.fold_now(at, &f.s, cx);
        }
    }

    /// `e`: the walked block taken (D8, §3.2), on its painted target: a
    /// transcript block drawn whole ([`AppView::take_block`] checks the rest). A
    /// rail section is edited in the transcript (`p` puts it there).
    fn walk_take(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.walk_painted_at() else { return };
        let Some((_, f)) = self.walk_target() else {
            self.walk_says(at, CHANGED, window, cx);
            return;
        };
        match at {
            At::Block(row) if f.tomb.is_none() && self.drawn_look(&f.s) == crate::rail_wishes::Look::Full => {
                self.take_block(row, f.s.block.tag, crate::draft_view::Part::Take, window, cx);
            }
            At::Block(_) => {}
            At::Rail(_) => self.walk_says(at, RAIL_TAKE, window, cx),
        }
    }

    /// Block `s` (at `at`) folded or unfolded, locally: in this pane's
    /// transcript, or in the rail; its undo kept, what it set and what it
    /// was before (§3.5). `false`: not drawn here now.
    pub(crate) fn fold_now(&mut self, at: At, s: &Shown, cx: &mut Context<Self>) -> bool {
        let act = match at {
            At::Block(row) => {
                let Some((ix, drawn)) = self.block_shown(row) else { return false };
                let (before, after) = self.panes.cur.block_folds.toggle(row, &drawn);
                self.panes.cur.list.remeasure_items(ix..ix + 1);
                Undid::Fold { before, after }
            }
            At::Rail(_) => {
                let Some(session) = self.rail_sid().map(str::to_string) else { return false };
                let Some((d, lay)) = self.rail_layout() else { return false };
                let Some(toggled) = self.agent_rail.local_mut(&session).toggle(&lay, &s.block.id) else { return false };
                let open = self.rail_open(&d, &s.block.id);
                Undid::RailFold { toggled, open }
            }
        };
        self.panes.cur.walk.push(Undo { at, tag: s.block.tag, act });
        cx.notify();
        true
    }

    /// Whether section `id` draws open in the rail of board `d` (the
    /// focused pane's session's), as laid out now.
    pub(crate) fn rail_open(&self, d: &Drawn, id: &Ref) -> bool {
        self.rail_of(self.panes.cur.live, d).section(id).is_some_and(|x| x.state == crate::agent_rail::State::Open)
    }

    /// `x`, `u`, `p`, `m` (and the hover's `×`, `rail`, `⇅`): D6b's, on the
    /// walked block's painted target ([`BlockDesires`]); on a tombstone,
    /// `x` and `u` only.
    fn walk_wish(&mut self, wish: Wish, window: &mut Window, cx: &mut Context<Self>) {
        let Some((at, painted)) = self.walked() else { return };
        self.wish_at(at, painted, wish, window, cx);
    }

    /// `wish` on target `at` as `painted` (the walk's key, the palette's
    /// command on the block walked when it opened): only if the bound
    /// board still holds it so, else nothing and the note; on a
    /// tombstone, `x` and `u` only. Its undo kept.
    pub(crate) fn wish_at(&mut self, at: At, painted: Painted, wish: Wish, window: &mut Window, cx: &mut Context<Self>) {
        let Some((board, f)) = self.target_at(at, painted) else {
            self.walk_says(at, CHANGED, window, cx);
            return;
        };
        if f.tomb.is_some() && !matches!(wish, Wish::Dismiss | Wish::Show) {
            return;
        }
        if let Some(token) = self.wish_now(at, wish, &board, &f, window, cx) {
            self.panes.cur.walk.push(Undo { at, tag: f.s.block.tag, act: Undid::Wish(wish, token) });
        }
    }

    /// `wish` on block `f` (at `at`) of board `board`, asked of D6b; its
    /// token if it took it.
    pub(crate) fn wish_now(
        &mut self,
        at: At,
        wish: Wish,
        board: &Drawn,
        f: &Found,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let session = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone())?;
        let conv = self.conversation(self.panes.cur.live);
        let aim = Aim { session: &session, conv: &conv, drawn: board, block: &f.s, place: f.place, tomb: f.tomb };
        let d = self.block_desires_mut();
        let asked = match wish {
            Wish::Dismiss => d.dismiss(&aim),
            Wish::Show => d.show(&aim),
            Wish::Pin => d.pin(&aim),
            Wish::Slot => d.slot(&aim),
        };
        log::info!(target: "synaps_desktop::walk", "walk: {wish:?} {}: {asked:?}", f.s.block.id);
        match asked {
            Asked::Done(token) => {
                // D6b's stores: what changed written (a dismissal draws
                // once it is).
                self.pump_wishes(cx);
                cx.notify();
                Some(token)
            }
            Asked::Off => None,
            Asked::Refused(why) => {
                log::info!(target: "synaps_desktop::walk", "walk: {wish:?} {} refused: {why}", f.s.block.id);
                self.walk_says(at, REFUSED, window, cx);
                None
            }
        }
    }

    /// Ctrl+Z (Cmd+Z) in the walk: the last local action undone, one level
    /// per block (§3.5): what it set put back as it was, if the block is
    /// still the incarnation acted on (else `this block changed`) and
    /// still shows what it set (else nothing, and [`STALE`]); the walk
    /// goes to it.
    fn walk_undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(u) = self.panes.cur.walk.undo.pop() else { return };
        let board = self.bound_board();
        // A wish's block wherever the board has it: a dismissed section is
        // hidden, a pin taken out of the rail is gone from it.
        let found = board
            .as_deref()
            .and_then(|d| self.find_on(d, u.at).or_else(|| matches!(u.act, Undid::Wish(..)).then(|| on_board(d, u.at.row())).flatten()))
            .filter(|f| f.s.bound.as_ref().is_some_and(|b| b.tag == u.tag));
        let (Some(board), Some(f)) = (board, found) else {
            self.walk_says(u.at, CHANGED, window, cx);
            return;
        };
        // The walk goes to it.
        let order = self.walk_order();
        if let Some(k) = order.iter().position(|a| *a == u.at) {
            let now = self.fx.now();
            let had_rev = self.panes.cur.walk.walk.as_ref().is_some_and(|w| w.at != u.at && w.rev.is_some());
            if let Some(w) = self.panes.cur.walk.walk.as_mut().filter(|w| w.at != u.at) {
                (w.at, w.ix, w.since, w.rev) = (u.at, k, now, None);
                w.painted.set(None);
            }
            if had_rev {
                self.walk_scratch_drop();
            }
        }
        let undone = match &u.act {
            Undid::Fold { before, after } => match (u.at, self.block_shown(u.at.row())) {
                // Still what the fold set: no agent `collapse` since.
                (At::Block(row), Some((ix, s)))
                    if self.panes.cur.block_folds.entry(row) == Some(*after) && s.block.folded_at == after.1 =>
                {
                    self.panes.cur.block_folds.set(row, *before);
                    self.panes.cur.list.remeasure_items(ix..ix + 1);
                    true
                }
                _ => false,
            },
            Undid::RailFold { toggled, open } => {
                // Both sections it writes checked on one rail: the newest
                // bound board's (the one `f` was found on, a board waiting
                // under the lease included), never the one drawn while a
                // newer one waits (Shady #105 R3).
                let lay = self.rail_of(self.panes.cur.live, &board);
                let laid_open = lay.section(&toggled.id).is_some_and(|x| x.state == crate::agent_rail::State::Open);
                match self.rail_sid().map(str::to_string) {
                    // Still what it set: its choice as made (no agent
                    // `collapse` since), the expansion it folded back
                    // still folded back, the same incarnation, its own
                    // choice holding (`Local::undo`), and it draws as it
                    // did then (no change of the cap's since).
                    Some(session) if f.s.block.folded_at == toggled.after.at && laid_open == *open => {
                        self.agent_rail.local_mut(&session).undo(toggled, &lay)
                    }
                    _ => false,
                }
            }
            Undid::Wish(wish, token) => {
                let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return };
                let conv = self.conversation(self.panes.cur.live);
                let aim = Aim { session: &session, conv: &conv, drawn: &board, block: &f.s, place: f.place, tomb: f.tomb };
                match self.block_desires_mut().undo(&aim, *wish, *token) {
                    Asked::Refused(why) => {
                        log::info!(target: "synaps_desktop::walk", "walk: undo {wish:?} {} refused: {why}", f.s.block.id);
                        self.walk_says(u.at, REFUSED, window, cx);
                        true
                    }
                    _ => {
                        self.pump_wishes(cx);
                        true
                    }
                }
            }
        };
        if undone {
            log::info!(target: "synaps_desktop::walk", "walk: undid {:?} on {:?}", u.act, u.at);
        } else {
            log::info!(target: "synaps_desktop::walk", "walk: {:?} on {:?} no longer holds: not undone", u.act, u.at);
            self.walk_says(u.at, STALE, window, cx);
        }
        cx.notify();
        self.walk_reveal(false, cx);
    }

    /// `[` (`back`) and `]` on the walked block's painted target (§3.6,
    /// §6.2): checked as every key is, before the lease releases; a
    /// tombstone has none.
    fn walk_rev_key(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.walk_painted_at() else { return };
        let Some((board, f)) = self.walk_target() else {
            self.walk_says(at, CHANGED, window, cx);
            return;
        };
        if f.tomb.is_none() {
            self.walk_rev(at, &board, &f.s, back, cx);
        }
    }

    /// `[` (`back`) and `]` on block `s` (at `at`) of the bound board `d`,
    /// its target checked: its revisions, newest first; `]` from the
    /// newest past one, the latest (the view closes). An open view steps
    /// through its own incarnation's (no yanking: the board's records of
    /// it are history, whatever replaced it since).
    fn walk_rev(&mut self, at: At, d: &Drawn, s: &Shown, back: bool, cx: &mut Context<Self>) {
        if s.block.pending {
            return;
        }
        let cur = self.panes.cur.walk.walk.as_ref().filter(|w| w.at == at).and_then(|w| w.rev.clone());
        // Of the incarnation the view opened on, else the current one; the
        // one in view found by identity (a revision landed since is
        // newer, never in its place).
        let tag = cur.as_ref().map_or(s.block.tag, |v| v.tag);
        let latest = cur.as_ref().map_or(s.block.rev, |v| v.latest);
        let revs = revisions::revisions(d, &s.block.id, tag);
        if revs.is_empty() {
            return;
        }
        let ix = cur
            .as_ref()
            .and_then(|v| revs.iter().position(|r| r.tag == v.rev.tag && r.rev == v.rev.rev && r.earlier == v.rev.earlier))
            .unwrap_or(0);
        let next = if back { (ix + 1).min(revs.len() - 1) } else { ix.saturating_sub(1) };
        // Forward to the view's latest (or past it): the latest, the view
        // closed.
        let past = next > 0 && !(revs[next].tag == tag && !revs[next].earlier && revs[next].rev >= latest);
        if next == ix && cur.is_some() == past {
            return;
        }
        let view = past.then(|| {
            let time = revs[next].call.as_deref().and_then(|c| self.call_time(c));
            RevView { rev: revs[next].clone(), ix: next, latest, tag, id: s.block.id.clone(), time }
        });
        let target = view.as_ref().map(|v| Refold::Revision(v.rev.at));
        if let Some(w) = self.panes.cur.walk.walk.as_mut() {
            w.rev = view;
        }
        match target {
            Some(target) => {
                self.panes.cur.walk.refolding = true;
                self.refold(self.panes.cur.live, target, cx);
            }
            None => {
                self.walk_scratch_drop();
            }
        }
        cx.notify();
    }

    /// Call `tool_id`'s row's time in the pane acted on, `HH:MM` (a lookup
    /// by its call: [`synaps_client::Transcript::call_at`]).
    pub(crate) fn call_time(&self, tool_id: &str) -> Option<String> {
        crate::oplog::time_of(&self.model.lives[self.panes.cur.live].transcript, Some(tool_id))
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
        // `R` (Shift+R) restores a past revision (§3.8, D8b).
        let restore = m.shift && key == "r";
        let ours =
            undo || restore || matches!(key, "j" | "k" | "down" | "up" | "z" | "c" | "x" | "u" | "p" | "m" | "[" | "]" | "e" | "escape");
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
        if !ours || (m.shift && key != "escape" && !restore) {
            // Another printable key: typed into the composer, as a card's.
            if let Some(t) = text {
                self.walk_leave(true, window, cx);
                let c = self.panes.cur.composer.clone();
                c.update(cx, |s, cx| EntityInputHandler::replace_text_in_range(s, None, &t, window, cx));
            }
            return;
        }
        // Only a new, bare press acts (Ctrl+Z: a new press).
        let fresh = if undo || restore { self.ask_presses.last } else { self.ask_presses.acts };
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
            "[" => self.walk_rev_key(true, window, cx),
            "]" => self.walk_rev_key(false, window, cx),
            "x" => self.walk_wish(Wish::Dismiss, window, cx),
            "u" => self.walk_wish(Wish::Show, window, cx),
            "p" => self.walk_wish(Wish::Pin, window, cx),
            "m" => self.walk_wish(Wish::Slot, window, cx),
            "r" if restore => self.walk_restore(cx),
            "e" => self.walk_take(window, cx),
            _ => {}
        }
    }

    /// `R` on the walked block (§3.6): its past revision in view restored,
    /// as an edit; then the latest again. Else the block says why.
    fn walk_restore(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.walk_painted_at() else { return };
        let At::Block(row) = at else {
            self.edit_says(at.row(), crate::handback_view::VIEW_ONLY, cx);
            return;
        };
        self.restore_now(row, cx);
    }

    /// The restore of row `row`'s past revision in view (`R`): sent, the
    /// walk back on the latest; or why not. Without the bound board kept
    /// (no draft holds one), it waits for it: the next job keeps it.
    pub(crate) fn restore_now(&mut self, row: u64, cx: &mut Context<Self>) {
        if !self.bound_board().is_some_and(|b| b.board.is_some()) {
            self.restore_wanted = Some((self.panes.cur.id, row));
            self.sync_drafts(cx);
            return;
        }
        match self.restore(row, cx) {
            Ok(()) => {
                if let Some(w) = self.panes.cur.walk.walk.as_mut() {
                    w.rev = None;
                }
                self.walk_scratch_drop();
                self.remeasure_block(row);
                cx.notify();
            }
            Err(why) => self.edit_says(row, why, cx),
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

    /// The walk ring on block or section row `row` (painted as `tag`, and
    /// as the tombstone of token `tomb` if it draws as one), if the walk of
    /// the pane acted on is there: a 1 px accent outline over the block
    /// (its spine stays), fading in; painted in view, its fade begun (as
    /// an entered card's growth: a frame of the motion clock), its keys act
    /// ([`Walk::shown`]).
    pub(crate) fn walk_ring(&self, at: At, tag: Tag, tomb: Option<u64>, p: &Palette) -> Option<AnyElement> {
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
                                painted.set(Some(Painted { tag, tomb }));
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
        let text = match &w.rev {
            Some(_) => {
                let label = self.walk_past(w.at.row()).map(|pv| pv.label).unwrap_or_default();
                format!("{label} · [ older · ] newer · esc latest")
            }
            None => status_keys(self.block_desires().wired(), cfg!(target_os = "macos")),
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
    /// walk shows one (module docs, §3.6): its label, a later revision
    /// landed since, and its body from the re-fold scratch, or why there
    /// is none yet, or none at all: never the live block's.
    pub(crate) fn walk_past(&self, row: u64) -> Option<Past> {
        let w = self.panes.cur.walk.walk.as_ref().filter(|w| w.at.row() == row)?;
        let v = w.rev.as_ref()?;
        let live = self.panes.cur.live;
        let l = &self.model.lives[live];
        let label = revisions::label(&v.rev, v.latest, v.time.as_deref());
        let now = l.transcript.drawn().and_then(|d| self.find_on(d, w.at)).map(|f| (f.s.block.tag, f.s.block.rev));
        // No yanking: a later revision landed meanwhile says so.
        let newer = now.filter(|(tag, rev)| *tag != v.tag || *rev > v.latest).map(|(_, rev)| rev);
        let done = self.refolds.done.as_ref().filter(|d| d.live == live && d.target == Refold::Revision(v.rev.at));
        let body = match done.map(|d| &d.state) {
            Some(Ok(Some(st))) => past_shown(st, &v.id, v.rev.tag).map_or(PastBody::Unavailable("not on its board"), PastBody::Ready),
            Some(Ok(None)) => PastBody::Unavailable("not shown"),
            Some(Err(_)) => PastBody::Unavailable("too far back to show"),
            None => PastBody::Loading,
        };
        Some(Past { label, newer, rev: v.rev.rev, body })
    }

    /// The motion keys' salt for row `row` in the pane acted on: a past
    /// revision's own (0: the block's).
    pub(crate) fn past_salt(&self, row: u64) -> u64 {
        let Some(w) = self.panes.cur.walk.walk.as_ref().filter(|w| w.at.row() == row) else { return 0 };
        w.rev.as_ref().map_or(0, |v| past_salt(row, &v.rev))
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
    /// past revision, D6b's where it offers them (`rail`, `⇅` a rail
    /// section's, `×`: [`BlockDesires::offers`]), `fold`.
    pub(crate) fn hover_offers(&self, s: &Shown, rail: bool) -> Vec<(Hover, &'static str)> {
        let d = self.block_desires();
        let mut v = Vec::new();
        if !s.block.pending && (s.block.rev > 1 || s.block.replaced.is_some()) {
            v.push((Hover::Rev, "rev"));
        }
        if d.offers(Wish::Pin, &s.block, rail) {
            // An agent's section goes into the transcript; anything else
            // into the rail, or back out of it.
            let into = rail && matches!(s.block.place, Place::Rail(_));
            v.push((Hover::Pin, if into { "transcript" } else { "rail" }));
        }
        if rail && d.offers(Wish::Slot, &s.block, rail) {
            v.push((Hover::Slot, "⇅"));
        }
        if !rail && !s.block.pending && crate::drafts::editable(s.block.kind) {
            v.push((Hover::Edit, "edit"));
        }
        v.push((Hover::Fold, "fold"));
        if d.offers(Wish::Dismiss, &s.block, rail) {
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
                self.fold_now(at, s, cx);
            }
            Hover::Edit => {
                if let At::Block(row) = at {
                    self.take_block(row, s.block.tag, crate::draft_view::Part::Take, window, cx);
                }
            }
            Hover::Rev => {
                if self.walk_to(at, window, cx)
                    && let Some(board) = self.bound_board()
                    && let Some(f) = self.find_on(&board, at)
                {
                    self.walk_rev(at, &board, &f.s, true, cx);
                }
            }
            Hover::Pin | Hover::Slot | Hover::Dismiss | Hover::Show(_) => {
                let wish = match act {
                    Hover::Pin => Wish::Pin,
                    Hover::Slot => Wish::Slot,
                    Hover::Show(_) => Wish::Show,
                    _ => Wish::Dismiss,
                };
                let Some(board) = self.bound_board() else { return };
                let Some(f) = self.find_on(&board, at) else { return };
                // A tombstone's click acts on the one it was painted as.
                if let Hover::Show(token) = act
                    && f.tomb != Some(token)
                {
                    self.walk_says(at, CHANGED, window, cx);
                    return;
                }
                if let Some(token) = self.wish_now(at, wish, &board, &f, window, cx) {
                    self.panes.cur.walk.push(Undo { at, tag: f.s.block.tag, act: Undid::Wish(wish, token) });
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

    /// Pointer-up on group member `row`'s tombstone, its `u` (`act`): the
    /// press's target acts only if it is this one, and the bound board
    /// still holds it as painted: the same incarnation, in the group it
    /// was painted in ([`AppView::block_up`]'s check), the same tombstone
    /// ([`AppView::hover_up`]'s). Else nothing, and the note. (A member
    /// isn't walked: no undo of it is kept.)
    pub(crate) fn member_tomb_up(&mut self, row: u64, act: Hover, window: &mut Window, cx: &mut Context<Self>) {
        let l = &mut self.panes.cur.lease;
        l.pressed = false;
        let Some(down) = l.down.take().filter(|d| d.at.row == row && d.act == crate::lease::Act::Hover(act)) else { return };
        let Hover::Show(token) = act else { return };
        let board = self.bound_board();
        let found = board.as_deref().and_then(|d| {
            let (s, group) = crate::lease::find_any(d, row)?;
            let tag = |s: &Shown| s.bound.as_ref().map(|b| b.tag);
            let in_it =
                group.zip(down.group).is_some_and(|(g, was)| (crate::blocks::row_id(&g.block.id), tag(g)) == (was.row, Some(was.tag)));
            let tomb = matches!(self.look_on(d, s), crate::rail_wishes::Look::Tomb { token: t, .. } if t == token);
            (in_it && tomb && tag(s) == Some(down.at.tag)).then(|| s.clone())
        });
        match (board, found) {
            (Some(board), Some(s)) if self.block_drawn(row).is_some() => {
                let conv = self.conversation(self.panes.cur.live);
                let f = Found { place: self.block_desires().placed(&conv, &s.block), s, tomb: Some(token) };
                let _ = self.wish_now(At::Block(row), Wish::Show, &board, &f, window, cx);
            }
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

/// `e` on a rail section (D8a): it is edited in the transcript.
pub(crate) const RAIL_TAKE: &str = "edit it in the transcript · p puts it there";

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
    /// A transcript tombstone's click (`u`), its token as painted.
    Show(u64),
    /// Take it (D8, §3.2): an editable block's `e`.
    Edit,
}

/// A past revision as drawn (§3.6): its label, a later revision landed
/// meanwhile (`rN new`), its revision, and its body.
pub(crate) struct Past {
    pub label: String,
    pub newer: Option<u32>,
    pub rev: u32,
    pub body: PastBody,
}

/// A past revision's body (§3.6): never the live block's under its label.
#[derive(Clone, Debug)]
pub(crate) enum PastBody {
    /// Its block then, from the re-fold scratch.
    Ready(Shown),
    /// The scratch is folding it: `loading r2…`.
    Loading,
    /// It can't be had, and why: `r2 unavailable · too far back to show`.
    Unavailable(&'static str),
}

impl Past {
    /// Its label as the header draws it: `rN new` after it.
    pub(crate) fn head(&self) -> String {
        match self.newer {
            Some(n) => format!("{} · r{n} new", self.label),
            None => self.label.clone(),
        }
    }

    /// Its block then, once the scratch has it.
    pub(crate) fn ready(&self) -> Option<&Shown> {
        match &self.body {
            PastBody::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// What its frame says in place of a body not drawn: `loading r2…`, or
    /// `r2 unavailable · why`.
    pub(crate) fn wait(&self) -> Option<String> {
        match self.body {
            PastBody::Ready(_) => None,
            PastBody::Loading => Some(format!("loading r{}…", self.rev)),
            PastBody::Unavailable(why) => Some(format!("r{} unavailable · {why}", self.rev)),
        }
    }
}

/// A local undo that no longer holds (§3.5): the block shows something
/// else now than what the action set.
pub(crate) const STALE: &str = "changed since · not undone";

fn past_salt(row: u64, r: &Rev) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (row, r.tag, r.rev, PAST).hash(&mut h);
    (h.finish() | 1) & (crate::blocks::BLOCK_ID - 1)
}

/// A walk target as this desk draws it on a board ([`AppView::find_on`]):
/// the block, where it paints (its effective placement), and the token of
/// the tombstone it draws as, if it does.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub s: Shown,
    pub place: Place,
    pub tomb: Option<u64>,
}

/// Top-level block row `row` wherever board `d` has it (a rail slot, a
/// transcript row), drawn or not: the agent's placement, no tombstone.
fn on_board(d: &Drawn, row: u64) -> Option<Found> {
    let s =
        d.rail.iter().flatten().chain(d.rows.values().flat_map(|r| r.blocks.iter())).find(|s| crate::blocks::row_id(&s.block.id) == row)?;
    Some(Found { s: s.clone(), place: s.block.place, tomb: None })
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
        let fold = Undid::Fold { before: None, after: (false, None) };
        let mut w = WalkMode::default();
        w.push(Undo { at: At::Block(1), tag, act: fold.clone() });
        w.push(Undo { at: At::Block(2), tag, act: fold.clone() });
        w.push(Undo { at: At::Block(1), tag, act: Undid::Wish(Wish::Pin, 7) });
        assert_eq!(
            w.undo.iter().map(|u| (u.at, u.act.clone())).collect::<Vec<_>>(),
            [(At::Block(2), fold.clone()), (At::Block(1), Undid::Wish(Wish::Pin, 7))]
        );
        for r in 0..100 {
            w.push(Undo { at: At::Rail(r), tag, act: fold.clone() });
        }
        assert_eq!(w.undo.len(), UNDO_MAX);
        assert_eq!(w.undo.last().map(|u| u.at), Some(At::Rail(99)));
    }

    #[test]
    fn the_status_line_names_d6bs_keys_only_once_wired() {
        assert_eq!(status_keys(false, false), "j k walk · z fold · [ ] revisions · Ctrl+Z undo · esc composer");
        assert!(status_keys(true, true).contains("x dismiss · u show · p pin · m slot") && status_keys(true, true).contains("⌘Z"));
    }

    /// D7's default answers nothing, and places a block where the agent
    /// placed it.
    #[test]
    fn no_desires_does_nothing_and_places_as_the_agent_did() {
        let d = NoDesires;
        assert!(!d.wired());
        let (b, events) = cards_spec::board::Board::of_history(&synaps_client::synthetic::rail_history());
        let v = b.view(&events);
        for id in ["plan", "rot"] {
            let blk = v.block(&cards_spec::block::Ref::parse(id).expect("an id")).expect("on the board");
            assert_eq!(d.placed("s", blk), blk.place, "{id}: the agent's placement");
        }
    }
}
