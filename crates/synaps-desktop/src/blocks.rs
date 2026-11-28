//! Blocks in the transcript, drawn (synaps-cards `docs/blocks.md` v1.3:
//! §1.2 places, §1.5 kinds, §6 look, heights and motion). The agent's
//! blocks as the board publishes them ([`crate::board`]), and what this
//! desk's presentation makes of them (D6b, §3.5): a dismissed block draws
//! as its one-line tombstone, one pinned into the rail as its stub
//! ([`AppView::block_line`]), and an agent's rail section the user put
//! into the transcript draws after its creating call's own blocks
//! ([`Pinned`]).
//!
//! **Rows.** A block call the board binds, or will (pending), draws no row
//! of its own; an unbound one is a plain tool row, the fail-safe (§1.6).
//! The blocks anchored at a call draw after the run it sits in, never
//! inside it, so a run stays one card (§1.2, the `Ask` rule): one list row
//! per block, in its anchor's order ([`Kind::Block`]); a converted card's
//! under its receipt.
//!
//! **Look** (§6.1). The cards-D language: square, read by fill (`card_d`)
//! with no outline, Sans for words, mono for targets and values. A living
//! block has a 2 px spine in the accent on its left edge. The header is
//! 28 px: the block's life glyph (its state, in the call rows' family),
//! the title, then the kind and `rN`, a state word and the chevron. A
//! file's header is its path, `:line` and note (§6.2), its title before
//! them: the file's name and line never give way to the title. A pending
//! change says `pending` (dim) where `rN` is and never glows; a skipped op
//! leaves the dim `op skipped` chip, its reason in its tooltip (an op that
//! didn't apply, never the block's state); a replaced block reads
//! `rN · replaced`. The pointer lease's note on a block (`this block
//! changed · look again`) lies over the header's end, where `rN` and the
//! state are, wrapped to two lines: it reads whole at 320 px and every
//! text size, the title keeps its room, and nothing moves.
//!
//! **Heights** (§6.2) come from structure: a plan's steps are 28 px, a
//! status two lines, a diff's well at most 12 lines of 20 px (it scrolls
//! inside past that), a note at most 6 lines (`more` opens it), a metric
//! 44 px and 16 more with a series, a file its header alone; a folded
//! block, a group folded, one line.
//!
//! **Motion** (§6.3). A value that changes once bound glows once (the
//! afterglow: `Motion::glow`, 480 ms; Reduced a flash; Off nothing). A
//! new block arrives as any new row does. A "doing" step runs its life
//! glyph on the window's beat while the turn streams and is still after
//! it ("doing · turn ended"). Nothing asks for frames at rest.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use cards_spec::block::{DiffState, FileState, HunkState, MetricState, StatusState, StepState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::blocks::{Body, Fate, Shown, is_block_tool};
use synaps_client::transcript::{Item, ItemKind};

use crate::app::AppView;
use crate::fonts::{self, TextPx as _};
use crate::glyph::{self, Life};
use crate::motion;
use crate::theme::{Metrics, Palette};
use crate::turns::{Disp, Gap, Kind, Who};

/// A block row's id: its block id's hash, with this bit (item ids count up
/// from 1; [`crate::turns::DAY_ID`] and the summary key are higher bits).
pub(crate) const BLOCK_ID: u64 = 1 << 59;

/// The afterglows' key space ([`motion::key`]).
pub(crate) const GLOW_KEYS: u64 = 0x7B10 << 48;

/// A diff's well shows this many lines at most; a note this many.
pub(crate) const DIFF_LINES: usize = 12;
pub(crate) const NOTE_LINES: usize = 6;

fn hash(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Block `id`'s row id.
pub(crate) fn row_id(id: &synaps_client::blocks::Ref) -> u64 {
    BLOCK_ID | (hash(id.to_string()) & (BLOCK_ID - 1))
}

/// Item `it` draws no row: a block call the board binds or will (§1.6),
/// or, `board` (the transcript's board draws it), one it hasn't drawn yet.
/// An unbound call is a plain row.
pub(crate) fn silent(it: &Item, board: bool) -> bool {
    if it.kind != ItemKind::Tool || !it.tool.as_ref().is_some_and(|t| is_block_tool(&t.name)) {
        return false;
    }
    match it.blocks.as_deref().and_then(|b| b.call) {
        Some(f) => f != Fate::Unbound,
        None => board || it.blocks.is_some(),
    }
}

/// The agent's rail sections the user put into the transcript (§3.5's
/// `p`), by their creating call's row: they draw after its own blocks
/// ([`row_block`]).
pub(crate) type Pinned = HashMap<u64, Vec<Shown>>;

/// A pinned-in section's content as its row's rev sees it: a change the
/// agent makes to it remeasures its row, as its own row's rev does.
fn pinned_rev(s: &Shown) -> u64 {
    let b = &s.block;
    hash((
        b.tag,
        b.rev,
        b.collapsed,
        b.pending,
        s.bound.as_ref().map(|x| (x.tag, x.rev)),
        s.members.iter().map(pinned_rev).collect::<Vec<_>>(),
    ))
}

/// The rows of the blocks anchored at `items[range]`, in order, after
/// `out`'s last: each row's own, then those `pinned` puts there.
/// `joined`: under a converted card's receipt (no gap).
pub(crate) fn rows(items: &[Item], range: Range<usize>, out: &mut Vec<Disp>, agent: &mut bool, joined: bool, pinned: &Pinned) {
    for k in range {
        let own = items[k].blocks.as_deref().map_or(&[][..], |b| &b.blocks[..]);
        let theirs = pinned.get(&items[k].id).map_or(&[][..], Vec::as_slice);
        for (j, s) in own.iter().chain(theirs).enumerate() {
            let rev = if j < own.len() { items[k].rev } else { pinned_rev(s) };
            out.push(Disp {
                item: k,
                id: row_id(&s.block.id),
                rev: rev ^ (j as u64).rotate_left(40),
                kind: Kind::Block { at: j as u16 },
                gap: if joined && j == 0 { Gap::None } else { Gap::Part },
                who: if *agent { Who::None } else { Who::Agent },
            });
            *agent = true;
        }
    }
}

/// Block `at` of `item`'s row as [`rows`] laid it: its own, then the
/// sections `pinned` puts there.
pub(crate) fn row_block<'a>(item: &'a Item, at: u16, pinned: &'a Pinned) -> Option<&'a Shown> {
    let own = item.blocks.as_deref().map_or(&[][..], |b| &b.blocks[..]);
    let at = at as usize;
    own.get(at).or_else(|| pinned.get(&item.id)?.get(at - own.len()))
}

/// A block's sizes at the text step (§6.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geo {
    pub head: f32,
    pub step: f32,
    pub line: f32,
    pub metric: f32,
    pub series: f32,
    pub spine: f32,
    pub pad: f32,
    pub gap: f32,
}

impl Geo {
    pub(crate) fn of(m: &Metrics) -> Self {
        let px = |v: f32| (v * m.r).round();
        Geo { head: px(28.), step: px(28.), line: px(20.), metric: px(44.), series: px(16.), spine: 2., pad: px(10.), gap: px(8.) }
    }
}

/// What gives way in a narrow column, one order at a time (§6.4): a
/// plan's step notes first, then `rN`, then the state word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    pub notes: bool,
    pub rev: bool,
    pub word: bool,
}

impl Fit {
    pub(crate) fn of(column: f32) -> Self {
        Fit { notes: column >= 440., rev: column >= 300., word: column >= 250. }
    }
}

/// The block's state, as its header's glyph and word.
pub(crate) fn life_of(b: &Body, streaming: bool) -> (Life, Option<&'static str>) {
    let run = if streaming { Life::Run } else { Life::Seen };
    match b {
        Body::Plan(p) => {
            if p.steps.iter().any(|s| s.state == StepState::Failed) {
                (Life::Failed, None)
            } else if !p.steps.is_empty() && p.steps.iter().all(|s| matches!(s.state, StepState::Done | StepState::Skipped)) {
                (Life::Done, None)
            } else if p.steps.iter().any(|s| s.state == StepState::Doing) {
                (run, None)
            } else {
                (Life::Queued, None)
            }
        }
        Body::Status(s) => match s.state {
            StatusState::Working => (run, Some("working")),
            StatusState::Waiting => (Life::Wait, Some("waiting")),
            StatusState::Blocked => (Life::Failed, Some("blocked")),
            StatusState::Done => (Life::Done, Some("done")),
        },
        Body::Diff(d) => match d.state {
            DiffState::Editing => (run, Some("editing")),
            DiffState::Proposed => (Life::Wait, Some("proposed")),
            DiffState::Written => (Life::Done, Some("written")),
            DiffState::Reverted => (Life::Cancelled, Some("reverted")),
        },
        Body::Note(_) => (Life::Done, None),
        Body::Metric(m) => match m.state {
            MetricState::Ok => (Life::Done, Some("ok")),
            MetricState::Warn => (Life::Wait, Some("warn")),
            MetricState::Bad => (Life::Failed, Some("bad")),
            MetricState::Neutral => (Life::Queued, None),
        },
        Body::File(f) => match f.state {
            FileState::Reading => (run, Some("reading")),
            FileState::Editing => (run, Some("editing")),
            FileState::Written => (Life::Done, Some("written")),
            FileState::Done => (Life::Done, Some("done")),
        },
        Body::Group(_) => (Life::Done, None),
    }
}

/// A state word's colour: work going on in the accent, a good end `ok`,
/// a bad one `err`, the rest `dim` (never the authorize card's warning
/// colours, §6.1).
pub(crate) fn word_color(l: Life, p: &Palette) -> Hsla {
    match l {
        Life::Run | Life::Stream | Life::Wait => p.acc,
        Life::Failed | Life::Denied => p.err,
        Life::Done => p.ok,
        _ => p.dim_t,
    }
}

/// The header's title: the block's, else its kind's word (a file's path).
pub(crate) fn title_of(s: &Shown) -> String {
    let b = &s.block;
    if let Some(t) = b.title.as_deref().filter(|t| !t.is_empty()) {
        return crate::ask::shown(t, cards_spec::Lines::Single);
    }
    match &b.body {
        Body::File(f) => crate::turns::tilde(&f.path, false),
        Body::Diff(d) => crate::turns::tilde(&d.file, false),
        body => {
            let k = body.kind().as_str();
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// A value's hash, for its afterglow: of the bound board's block (a pending
/// change never glows; it glows when it binds, §6.1).
pub(crate) fn bound_value<T: Hash>(s: &Shown, f: impl Fn(&Body) -> Option<T>) -> u64 {
    s.bound.as_ref().map_or(0, |b| hash(f(&b.body)))
}

/// A plan step's text in its state.
pub(crate) fn step_word(st: StepState, streaming: bool) -> &'static str {
    match st {
        StepState::Todo => "",
        StepState::Doing if streaming => "doing",
        StepState::Doing => "doing · turn ended",
        StepState::Done => "",
        StepState::Failed => "failed",
        StepState::Skipped => "skipped",
    }
}

pub(crate) fn step_life(st: StepState, streaming: bool) -> Life {
    match st {
        StepState::Todo => Life::Queued,
        StepState::Doing if streaming => Life::Run,
        StepState::Doing => Life::Seen,
        StepState::Done => Life::Done,
        StepState::Failed => Life::Failed,
        StepState::Skipped => Life::Cancelled,
    }
}

/// A note's wrapped lines at `cpl` characters a line (an estimate: the
/// note shows [`NOTE_LINES`] at most, `more` past them).
pub(crate) fn note_lines(text: &str, cpl: f32) -> usize {
    let cpl = cpl.max(8.) as usize;
    text.lines().map(|l| l.chars().count().div_ceil(cpl).max(1)).sum::<usize>().max(1)
}

/// A note's characters a line in a column `column` px wide at metrics
/// `m` (the renderer's estimate, and the lease's: [`crate::lease::shape`]).
pub(crate) fn note_cpl(column: f32, m: &Metrics) -> f32 {
    let g = Geo::of(m);
    (column - g.spine - 2. * g.pad) / (m.label * 0.5)
}

/// Whether a note past [`NOTE_LINES`] (at `cpl`) is cut there (`more`), or
/// open (`open`: `less`); `None`, it fits.
pub(crate) fn note_cut(text: &str, cpl: f32, open: bool) -> Option<bool> {
    (note_lines(text, cpl) > NOTE_LINES).then_some(!open)
}

/// The tooltip of the `op skipped` chip.
pub(crate) const SKIPPED_TIP: &str = "An op the agent sent for this block was skipped: it changed nothing here. Not the block's state.";

/// A metric's value or unit, or a path's name, in mono columns (a wide
/// character takes two).
pub(crate) fn cols(s: &str) -> usize {
    s.chars().map(|c| if (c as u32) < 0x1100 { 1 } else { 2 }).sum()
}

/// One user fold: open or not, and the agent's `collapse` it overrode.
pub(crate) type Fold = (bool, Option<cards_spec::board::Pos>);

/// The user's own folds (§3.5: presentation, local), by block row: open
/// or folded, and the agent's `collapse` it overrode. The agent's next one
/// wins again.
#[derive(Default)]
pub(crate) struct Folds(HashMap<u64, Fold>);

impl Folds {
    /// Whether block `s` (row `id`) draws folded.
    pub(crate) fn folded(&self, id: u64, s: &Shown) -> bool {
        match self.0.get(&id) {
            Some((open, at)) if *at == s.block.folded_at => !open,
            _ => s.block.collapsed,
        }
    }

    /// Only the rows `keep` says.
    pub(crate) fn retain(&mut self, keep: impl Fn(&u64) -> bool) {
        self.0.retain(|r, _| keep(r));
    }

    /// The rows it keeps a fold for (tests).
    #[cfg(test)]
    pub(crate) fn rows(&self) -> Vec<u64> {
        self.0.keys().copied().collect()
    }

    /// Block `s` (row `id`) folded if open, opened if folded: the fold
    /// before (none: the agent's) and the one made (D7's undo).
    pub(crate) fn toggle(&mut self, id: u64, s: &Shown) -> (Option<Fold>, Fold) {
        let folded = self.folded(id, s);
        let after = (folded, s.block.folded_at);
        (self.0.insert(id, after), after)
    }

    /// Row `id`'s fold of the user's, as kept (whether it still holds or
    /// not).
    pub(crate) fn entry(&self, id: u64) -> Option<Fold> {
        self.0.get(&id).copied()
    }

    /// Row `id`'s fold put back as `f` (none: the agent's).
    pub(crate) fn set(&mut self, id: u64, f: Option<Fold>) {
        match f {
            Some(f) => self.0.insert(id, f),
            None => self.0.remove(&id),
        };
    }
}

/// A list's block state as it travels with the list (the tab cache, a
/// worker's view in place): the user's folds and notes opened past their
/// lines, and the history they are of ([`history_of`]).
#[derive(Default)]
pub(crate) struct Kept {
    folds: Folds,
    more: std::collections::HashSet<u64>,
    of: u64,
}

/// The history a board is of, for the block state kept with it: its
/// session and its conversation generation (§1.2), never 0.
pub(crate) fn history_of(session: Option<&str>, drawn: &synaps_client::blocks::Drawn) -> u64 {
    hash((session, drawn.generation)).max(1)
}

impl crate::pane::Pane {
    /// The list's block state goes with it ([`Kept`]); the pointer lease and
    /// the diff wells' scroll were its painting's, and start over.
    pub(crate) fn blocks_out(&mut self) -> Kept {
        self.blocks_reset();
        Kept {
            folds: std::mem::take(&mut self.block_folds),
            more: std::mem::take(&mut self.block_more),
            of: std::mem::take(&mut self.block_of),
        }
    }

    /// A list's block state back with it.
    pub(crate) fn blocks_in(&mut self, k: Kept) {
        self.blocks_reset();
        (self.block_folds, self.block_more, self.block_of) = (k.folds, k.more, k.of);
    }

    /// History `of`'s board ([`history_of`]) draws here, and the block
    /// state was of another's: it starts over.
    pub(crate) fn blocks_over(&mut self, of: u64) {
        drop(self.blocks_out());
        self.block_of = of;
    }

    /// The painting's state: the lease (its frame count and the list's
    /// anchoring go on: [`crate::lease::Lease::clear`]), the diff wells'
    /// scroll.
    pub(crate) fn blocks_reset(&mut self) {
        self.lease.clear();
        self.block_scroll.borrow_mut().clear();
        // The walk was of this list's rows (its focus hands the keys to
        // the composer at the next frame, `AppView::sync_walk`).
        self.walk.walk = None;
        self.walk.undo.clear();
    }
}

impl AppView {
    /// Block row `d` (`at`, its item's `at`th block): its card (module docs).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_block(&self, item: &Item, at: u16, id: u64, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = row_block(item, at, &self.panes.cur.pinned_in) else { return div().into_any_element() };
        let g = Geo::of(m);
        if let Some(line) = self.block_line(s, id, &g, p, m, cx) {
            return line;
        }
        let fit = Fit::of(self.panes.cur.layout.column);
        let folded = self.panes.cur.block_folds.folded(id, s);
        let streaming = self.model.lives[self.panes.cur.live].shown().busy();
        let pane = self.panes.cur.id;
        let tag = s.block.tag;
        // A past revision in the walk's view (D7, §3.6): its header and
        // body, from the re-fold scratch once there; its label in place of
        // `rN`. A group's past members are its lines. Until the scratch has
        // it, or if it can't, its frame says so: never the live block
        // under a past label.
        let past = self.walk_past(id);
        let ready = past.as_ref().and_then(|v| v.ready());
        let wait = past.as_ref().and_then(|v| Some((v.wait()?, v.head())));
        let head = match (&wait, ready) {
            (Some((w, label)), _) => past_wait_head(id, w, label, &g, p, m),
            (None, Some(ps)) => self.block_head(ps, id, p.card_d, folded, false, fit, &g, p, m, cx),
            (None, None) => self.block_head(s, id, p.card_d, folded, streaming, fit, &g, p, m, cx),
        };
        // Its hover actions (D7): over the header's right end, never
        // reflowing it, unless the lease's note is there.
        let hovered = self.panes.cur.lease.head.filter(|h| h.0 == id && past.is_none());
        let says = self.panes.cur.lease.note.is_some_and(|n| n.0 == id);
        let acts = hovered.filter(|_| !says).map(|(_, since)| {
            let offers = self.hover_offers(s, false);
            self.hover_actions(id, since, &offers, p.card_d, p, |act, el| {
                let press =
                    crate::lease::Press { at: crate::lease::Target { row: id, tag }, act: crate::lease::Act::Hover(act), group: None };
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.block_down(pane, press, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                        cx.stop_propagation();
                        this.in_pane(pane, |this| this.hover_up(id, act, window, cx));
                    }),
                )
            })
        });
        let head = head
            .id(("block-head", id))
            .on_hover(cx.listener(move |this, over: &bool, _, cx| this.head_hover(pane, id, *over, cx)))
            .children(acts);
        let body = (!folded && wait.is_none()).then(|| match ready {
            Some(ps) if matches!(ps.block.body, Body::Group(_)) => past_members(ps, id, p, m),
            Some(ps) => self.block_body(ps, id, None, false, fit, &g, p, m, cx),
            None => self.block_body(s, id, None, streaming, fit, &g, p, m, cx),
        });
        let press = crate::lease::Press { at: crate::lease::Target { row: id, tag }, act: crate::lease::Act::Fold, group: None };
        let el = div()
            .id(("block", id))
            .debug_selector(move || format!("block {id}"))
            .relative()
            .w_full()
            .bg(p.card_d)
            .pl(px(g.spine + g.pad))
            .pr(px(g.pad))
            // The spine: the one sign the agent can still change it.
            .child(
                div().absolute().left_0().top_0().bottom_0().w(px(g.spine)).bg(p.acc).debug_selector(move || format!("block-spine {id}")),
            )
            .child(head)
            .children(body)
            .child(crate::lease::probe(&self.panes.cur.lease, &self.fx, id, tag, id))
            // The walk's ring (D7): its keys act once it is painted.
            .children(self.walk_ring(crate::walk::At::Block(id), tag, None, p))
            .on_hover(cx.listener(move |this, over: &bool, _, _| this.block_hover(pane, id, tag, *over)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.block_down(pane, press, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, _: &MouseUpEvent, _, cx| this.block_up(pane, id, press.act, cx)));
        crate::lease::SlideY::new(&self.panes.cur.lease, &self.fx, id, el.into_any_element()).into_any_element()
    }

    /// Block `s` (row `id`) as this desk draws it in one line, if it does
    /// (§3.5, [`crate::rail_wishes::Look`]): dismissed, its tombstone; pinned
    /// into the rail, its stub. A fixed line on the card's ground, no
    /// spine (the agent's changes to it don't draw here), square, mono in
    /// the dim ink. A painted target, as the block's header is: a click is
    /// its key (`u` on a tombstone, `p` on a stub), checked as a hover
    /// action is; the walk's ring paints a tombstone's token with it.
    fn block_line(&self, s: &Shown, id: u64, g: &Geo, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::rail_wishes::Look;
        let (text, tomb, act, kind) = match self.drawn_look(s) {
            Look::Full => return None,
            Look::Tomb { text, token } => (text, Some(token), crate::walk::Hover::Show(token), "block-tomb"),
            Look::Stub { text } => (text, None, crate::walk::Hover::Pin, "block-stub"),
        };
        let pane = self.panes.cur.id;
        let tag = s.block.tag;
        let says = self.panes.cur.lease.note.filter(|n| n.0 == id).map(|n| n.1);
        let t = crate::ask::shown(&text, cards_spec::Lines::Single);
        let press = crate::lease::Press { at: crate::lease::Target { row: id, tag }, act: crate::lease::Act::Hover(act), group: None };
        let el = h_flex()
            .id(("block", id))
            .debug_selector(move || format!("{kind} {id} {t}"))
            .relative()
            .h(px((24. * m.r).round()))
            .w_full()
            .gap(px(g.gap))
            .items_center()
            .whitespace_nowrap()
            .bg(p.card_d)
            .pl(px(g.spine + g.pad))
            .pr(px(g.pad))
            .cursor_pointer()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.dim_t)
                    .child(crate::ask::shown(&text, cards_spec::Lines::Single)),
            )
            .children(says.map(|n| {
                div()
                    .flex_none()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.acc)
                    .debug_selector(move || format!("block-says {id} {n}"))
                    .child(n)
            }))
            .child(crate::lease::probe(&self.panes.cur.lease, &self.fx, id, tag, id))
            .children(self.walk_ring(crate::walk::At::Block(id), tag, tomb, p))
            .on_hover(cx.listener(move |this, over: &bool, _, _| this.block_hover(pane, id, tag, *over)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.block_down(pane, press, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                    cx.stop_propagation();
                    this.in_pane(pane, |this| this.hover_up(id, act, window, cx));
                }),
            );
        Some(crate::lease::SlideY::new(&self.panes.cur.lease, &self.fx, id, el.into_any_element()).into_any_element())
    }

    /// Group member `ms` (row `mid`, of the group painted as `group`) as
    /// this desk draws it dismissed (D6b, §3.5): its tombstone, a line on
    /// the members' ground in its place in the group, nothing of its
    /// content. Its click is its `u`, checked as a member's press is (the
    /// same incarnation, in the group it was painted in) and as a
    /// tombstone's (the one painted): [`AppView::member_tomb_up`]. `None`:
    /// it draws whole.
    #[allow(clippy::too_many_arguments)]
    fn member_tomb(
        &self,
        ms: &Shown,
        mid: u64,
        group: crate::lease::Target,
        g: &Geo,
        p: &Palette,
        m: &Metrics,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let crate::rail_wishes::Look::Tomb { text, token } = self.drawn_look(ms) else { return None };
        let pane = self.panes.cur.id;
        let mtag = ms.block.tag;
        let gid = group.row;
        let act = crate::walk::Hover::Show(token);
        let press = crate::lease::Press {
            at: crate::lease::Target { row: mid, tag: mtag },
            act: crate::lease::Act::Hover(act),
            group: Some(group),
        };
        let says = self.panes.cur.lease.note.filter(|n| n.0 == mid).map(|n| n.1);
        let t = crate::ask::shown(&text, cards_spec::Lines::Single);
        let el = h_flex()
            .id(("block-member-tomb", mid))
            .debug_selector(move || format!("block-member-tomb {gid} {mid} {t}"))
            .relative()
            .h(px((24. * m.r).round()))
            .w_full()
            .gap(px(g.gap))
            .items_center()
            .whitespace_nowrap()
            .bg(p.well_d)
            .pl(px(g.pad))
            .pr(px(g.pad))
            .cursor_pointer()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.dim_t)
                    .child(crate::ask::shown(&text, cards_spec::Lines::Single)),
            )
            .children(says.map(|n| {
                div()
                    .flex_none()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.acc)
                    .debug_selector(move || format!("block-says {mid} {n}"))
                    .child(n)
            }))
            // Its own target, moving with its group (§6.2).
            .child(crate::lease::probe(&self.panes.cur.lease, &self.fx, mid, mtag, gid))
            .on_hover(cx.listener(move |this, over: &bool, _, _| this.member_hover(pane, mid, mtag, *over)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.block_down(pane, press, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                    cx.stop_propagation();
                    this.in_pane(pane, |this| this.member_tomb_up(mid, act, window, cx));
                }),
            );
        Some(el.into_any_element())
    }

    /// The lease's note on block row `id` (§6.2), until the next input:
    /// over its header's end, where `rN` and the state are (they are back
    /// at the next input), on the header's own ground, wrapped to two
    /// lines at most. Nothing moves when it comes or goes, and the title
    /// keeps its room (decision #5 of #101).
    fn block_says(&self, id: u64, g: &Geo, ground: Hsla, p: &Palette, m: &Metrics) -> Option<Div> {
        let n = self.panes.cur.lease.note.filter(|n| n.0 == id).map(|n| n.1)?;
        // Its width: its text's on one line (an estimate) within 60 % of
        // the header; else two lines, within 85 %.
        let head = self.panes.cur.layout.column - g.spine - 2. * g.pad;
        let one = (n.chars().count() as f32 * m.meta * 0.62).ceil();
        let w = if one <= head * 0.6 { one } else { (one * 0.62).ceil().min((head * 0.85).round()) };
        Some(
            div().absolute().top_0().bottom_0().right_0().pl(px(g.gap)).bg(ground).flex().items_center().justify_end().child(
                div()
                    .w(px(w))
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .line_height(px((g.head / 2.).floor()))
                    .text_color(p.acc)
                    .whitespace_normal()
                    .debug_selector(move || format!("block-says {id} {n}"))
                    .child(n),
            ),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn block_head(
        &self,
        s: &Shown,
        id: u64,
        ground: Hsla,
        folded: bool,
        streaming: bool,
        fit: Fit,
        g: &Geo,
        p: &Palette,
        m: &Metrics,
        cx: &mut Context<Self>,
    ) -> Div {
        let b = &s.block;
        let (life, word) = life_of(&b.body, streaming);
        // A file's or a diff's path, `:line` and note (a diff has neither).
        let path = match &b.body {
            Body::File(f) => Some((f.path.as_str(), f.line, f.note.as_deref())),
            Body::Diff(d) => Some((d.file.as_str(), None, None)),
            _ => None,
        };
        // Its title, if it has one, comes before its path (P2-10).
        let title = match path {
            Some(_) => b.title.as_deref().filter(|t| !t.is_empty()).map(|t| crate::ask::shown(t, cards_spec::Lines::Single)),
            None => Some(title_of(s)),
        };
        // A past revision in view has its own motion keys (D7: stepping
        // through revisions never glows).
        let gid = id ^ self.past_salt(id);
        let glow = |field: u64, v: u64, c: Hsla| self.fx.glow(motion::key(GLOW_KEYS, gid ^ field.rotate_left(13)), v, c);
        let title_c = glow(1, bound_value(s, |_| s.bound.as_ref().map(|b| b.title.clone())), p.prose_t);
        let past = self.walk_past(id);
        let rev = if let Some(v) = &past {
            v.head()
        } else if b.pending {
            "pending".to_string()
        } else if b.replaced.is_some() {
            format!("r{} · replaced", b.rev)
        } else {
            format!("r{}", b.rev)
        };
        let kind = match &b.body {
            Body::Diff(d) if d.from.is_some() => "EDIT PREVIEW".to_string(),
            Body::Group(gb) => format!("group · {}", gb.members.len()),
            body => body.kind().as_str().to_string(),
        };
        let word = match (&b.body, word) {
            (Body::Plan(pl), _) => {
                let done = pl.steps.iter().filter(|s| s.state == StepState::Done).count();
                Some(format!("{done}/{}", pl.steps.len()))
            }
            (_, w) => w.map(str::to_string),
        };
        let word_c = glow(2, bound_value(s, |b| life_of(b, true).1), word_color(life, p));
        let says = self.block_says(id, g, ground, p, m);
        h_flex()
            .relative()
            .h(px(g.head))
            .w_full()
            .gap(px(g.gap))
            .items_center()
            .whitespace_nowrap()
            .font_family(fonts::SANS)
            .text_px(m.label)
            .cursor_pointer()
            .debug_selector(move || format!("block-head {id}"))
            .child(div().flex_none().debug_selector(move || format!("block-life {id} {life:?}")).child(glyph::indicator(
                &self.fx,
                life,
                p.card_d,
                p,
                self.fx.col(self.panes.cur.id).change(motion::Tag::Outcome, gid, life.code(), None),
                None,
            )))
            .when_some(title, |d, title| {
                // A file's or a diff's title gives way first: its path is
                // what it is about (§6.2). A diff's keeps its first
                // characters (it says what the change is).
                let floor = match &b.body {
                    Body::Diff(_) => (title.chars().count() as f32 * m.label * 0.55).min((m.label * 1.65).round()),
                    _ => 0.,
                };
                d.child(
                    div()
                        .min_w(px(floor))
                        .flex_shrink(if path.is_some() { 10_000. } else { 1. })
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(title_c)
                        .debug_selector(move || format!("block-title {id}"))
                        .child(title),
                )
            })
            .children(
                path.map(|(path, line, note)| self.path_head(path, line, note, id, b.title.as_deref().is_none_or(str::is_empty), p, m)),
            )
            .when(b.skipped.is_some(), |d| {
                // An op that didn't apply, never the block's state (Shady
                // #101: `skipped` alone read as the plan's).
                d.child(
                    // It gives way before the title (to `op…` at least),
                    // its tooltip whole.
                    div()
                        .id(("block-skipped", id))
                        .min_w(px((m.meta * 1.9).ceil() + 8.))
                        .flex_shrink(8.)
                        .overflow_hidden()
                        .px(px(4.))
                        .bg(p.well_d)
                        .font_family(fonts::MONO)
                        .text_px(m.meta)
                        .text_color(p.dim_t)
                        .debug_selector(move || format!("block-skipped {id}"))
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .debug_selector(move || format!("block-skipped-says {id} op skipped"))
                                .child("op skipped"),
                        )
                        .tooltip(move |window, cx| {
                            crate::overlay::element(
                                move |_, cx| {
                                    let p = crate::theme::palette(cx);
                                    div()
                                        .max_w(px(320.))
                                        .px(px(8.))
                                        .py(px(4.))
                                        .font_family(fonts::SANS)
                                        .text_px(fonts::size::TITLE)
                                        .line_height(px(18.))
                                        .text_color(p.prose_k)
                                        .whitespace_normal()
                                        .debug_selector(move || format!("block-skipped-tip {id}"))
                                        .child(SKIPPED_TIP)
                                },
                                window,
                                cx,
                            )
                        }),
                )
            })
            .child(div().flex_1().min_w(px(8.)))
            .when(fit.rev || past.is_some(), |d| {
                // A past revision's label gives way before the title's
                // floor (its whole in the walk's status line).
                let text = if past.is_some() { rev.clone() } else { format!("{kind} · {rev}") };
                d.child(
                    div()
                        .when(past.is_none(), |d| d.flex_none())
                        .when(past.is_some(), |d| d.min_w_0().flex_shrink(1.).overflow_hidden().text_ellipsis())
                        .font_family(fonts::MONO)
                        .text_px(m.meta)
                        .text_color(if past.is_some() { p.acc } else { p.dim_t })
                        .debug_selector({
                            let t = format!("block-rev {id} {text}");
                            move || t.clone()
                        })
                        .child(text),
                )
            })
            .when(!fit.rev && b.pending, |d| d.child(div().flex_none().text_px(m.meta).text_color(p.dim_t).child("pending")))
            .when_some(word.filter(|_| fit.word), |d, w| {
                let t = format!("block-word {id} {w}");
                d.child(div().flex_none().text_px(m.meta).text_color(word_c).debug_selector(move || t.clone()).child(w))
            })
            .child(
                div()
                    .flex_none()
                    .w(px(10.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(glyph::turn(self.fx.col(self.panes.cur.id).turn(id, !folded), p.dim_t)),
            )
            .children(says)
            .map(|d| {
                let _ = cx;
                d
            })
    }

    /// A file's or a diff's path in its header (§6.2): its directories,
    /// its name and `:line`, a file's note, in one box of the header's row.
    /// The title gives way first; in the box, the directories and the note
    /// (the name never shrinks while they can); the name last, when the box
    /// is narrower than it, in its middle (its start, its extension and
    /// `:line` stay). `untitled`: no title, the path is it.
    #[allow(clippy::too_many_arguments)]
    fn path_head(
        &self,
        path: &str,
        line: Option<u64>,
        note: Option<&str>,
        id: u64,
        untitled: bool,
        p: &Palette,
        m: &Metrics,
    ) -> AnyElement {
        let path = crate::ask::shown(&crate::turns::tilde(path, false), cards_spec::Lines::Single);
        let cut = path.rfind('/').map_or(0, |k| k + 1);
        let (dir, name) = (path[..cut].to_string(), path[cut..].to_string());
        let name = match line {
            Some(l) => format!("{name}:{l}"),
            None => name,
        };
        let note = note.filter(|n| !n.is_empty()).map(|n| crate::ask::shown(n, cards_spec::Lines::Single));
        let gap = Geo::of(m).gap;
        let shown = name.clone();
        h_flex()
            .min_w_0()
            .flex_shrink(1.)
            .overflow_hidden()
            .items_center()
            .debug_selector(move || format!("block-pathbox {id}"))
            .when(!dir.is_empty(), |d| {
                d.child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_family(fonts::MONO)
                        .text_px(m.mono)
                        .text_color(p.dim_t)
                        .debug_selector(move || format!("block-dir {id}"))
                        .child(dir),
                )
            })
            .child(
                // As wide as the name, the box at most.
                div().flex_none().max_w_full().when(untitled, |d| d.debug_selector(move || format!("block-title {id}"))).child(
                    div()
                        .max_w_full()
                        .overflow_hidden()
                        .text_ellipsis_middle()
                        .font_family(fonts::MONO)
                        .text_px(m.mono)
                        .text_color(p.prose_t)
                        .debug_selector(move || format!("block-path {id} {shown}"))
                        .child(name),
                ),
            )
            .when_some(note, |d, n| {
                let t = format!("block-file-note {id} {n}");
                d.child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .ml(px(gap))
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_family(fonts::SANS)
                        .text_px(m.meta)
                        .text_color(p.dim_t)
                        .debug_selector(move || t.clone())
                        .child(n),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn block_body(
        &self,
        s: &Shown,
        id: u64,
        group: Option<crate::lease::Target>,
        streaming: bool,
        fit: Fit,
        g: &Geo,
        p: &Palette,
        m: &Metrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gid = id ^ self.past_salt(id);
        let glow = |field: u64, v: u64, c: Hsla| self.fx.glow(motion::key(GLOW_KEYS, gid ^ field.rotate_left(13)), v, c);
        let pb = px((6. * m.r).round());
        match &s.block.body {
            Body::Plan(pl) => {
                let mut col = v_flex().w_full().pb(pb).debug_selector(move || format!("block-steps {id}"));
                for st in &pl.steps {
                    let sid = hash(st.id.as_str());
                    let label_c = match st.state {
                        StepState::Done | StepState::Skipped => p.dim_t,
                        _ => p.prose_t,
                    };
                    let v = bound_value(s, |b| match b {
                        Body::Plan(bp) => {
                            bp.steps.iter().find(|x| x.id == st.id).map(|x| (x.label.clone(), x.state.as_str(), x.note.clone()))
                        }
                        _ => None,
                    });
                    let label_c = glow(sid, v, label_c);
                    let life = step_life(st.state, streaming);
                    let word = step_word(st.state, streaming);
                    let step_id = st.id.to_string();
                    col = col.child(
                        h_flex()
                            .h(px(g.step))
                            .w_full()
                            .gap(px(g.gap))
                            .items_center()
                            .whitespace_nowrap()
                            .font_family(fonts::SANS)
                            .text_px(m.label)
                            .debug_selector(move || format!("block-step {id} {step_id}"))
                            .child(div().flex_none().child(glyph::indicator(
                                &self.fx,
                                life,
                                p.card_d,
                                p,
                                self.fx.col(self.panes.cur.id).change(
                                    motion::Tag::Outcome,
                                    gid ^ (sid & 0xFFFF_FFFF) << 8,
                                    life.code(),
                                    None,
                                ),
                                None,
                            )))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_shrink(1.)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(label_c)
                                    .when(st.state == StepState::Skipped, |d| d.line_through())
                                    .child(crate::ask::shown(&st.label, cards_spec::Lines::Single)),
                            )
                            .child(div().flex_1().min_w(px(8.)))
                            .when_some(st.note.as_ref().filter(|_| fit.notes), |d, n| {
                                d.child(
                                    div()
                                        .min_w_0()
                                        .flex_shrink(1.)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .font_family(fonts::MONO)
                                        .text_px(m.meta)
                                        .text_color(p.dim_t)
                                        .child(crate::ask::shown(n, cards_spec::Lines::Single)),
                                )
                            })
                            .when(!word.is_empty() && fit.word, |d| {
                                let c = if st.state == StepState::Failed {
                                    p.err
                                } else if st.state == StepState::Doing && streaming {
                                    p.acc
                                } else {
                                    p.dim_t
                                };
                                d.child(div().flex_none().text_px(m.meta).text_color(c).child(word))
                            }),
                    );
                }
                col.into_any_element()
            }
            Body::Status(st) => {
                let text_c = glow(3, bound_value(s, |b| if let Body::Status(x) = b { Some(x.text.clone()) } else { None }), p.prose_t);
                let detail_c = glow(4, bound_value(s, |b| if let Body::Status(x) = b { Some(x.detail.clone()) } else { None }), p.dim_t);
                let line = |d: Div| d.h(px(g.line)).w_full().items_center().whitespace_nowrap().overflow_hidden();
                let meter = st.progress.map(|pr| {
                    let k = if pr.of == 0 { 0. } else { (pr.n as f32 / pr.of as f32).clamp(0., 1.) };
                    let w = (96. * m.r).round();
                    h_flex()
                        .flex_none()
                        .gap(px(6.))
                        .items_center()
                        .child(div().w(px(w)).h(px(3.)).bg(p.well_d).child(div().h_full().w(px((w * k).round())).bg(glow(
                            5,
                            bound_value(s, |b| if let Body::Status(x) = b { x.progress.map(|q| (q.n, q.of)) } else { None }),
                            p.acc,
                        ))))
                        .child(div().font_family(fonts::MONO).text_px(m.meta).text_color(p.dim_t).child(format!("{}/{}", pr.n, pr.of)))
                });
                // Tests read the glow off the drawn text: lit, or its colour.
                let lit = if text_c != p.prose_t { "lit" } else { "rest" };
                v_flex()
                    .w_full()
                    .pb(pb)
                    .debug_selector(move || format!("block-status {id}"))
                    .child(
                        line(h_flex())
                            .font_family(fonts::SANS)
                            .text_px(m.label)
                            .text_color(text_c)
                            .debug_selector(move || format!("block-text {id} {lit}"))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(crate::ask::shown(&st.text, cards_spec::Lines::Single)),
                            ),
                    )
                    .child(
                        line(h_flex())
                            .gap(px(g.gap))
                            .font_family(fonts::SANS)
                            .text_px(m.meta)
                            .text_color(detail_c)
                            .child(
                                div().min_w_0().flex_shrink(1.).overflow_hidden().text_ellipsis().child(
                                    st.detail.as_deref().map(|d| crate::ask::shown(d, cards_spec::Lines::Single)).unwrap_or_default(),
                                ),
                            )
                            .child(div().flex_1())
                            .children(meter),
                    )
                    .into_any_element()
            }
            Body::Diff(d) => self.diff_body(s, d, id, g, p, m, cx),
            Body::Note(n) => {
                let open = self.panes.cur.block_more.contains(&id);
                let cut = note_cut(&n.text, note_cpl(self.panes.cur.layout.column, m), open);
                let text_c = glow(6, bound_value(s, |b| if let Body::Note(x) = b { Some(x.text.clone()) } else { None }), p.prose_t);
                let shown = crate::ask::shown(&n.text, cards_spec::Lines::Multi);
                let body = v_flex()
                    .w_full()
                    .font_family(fonts::SANS)
                    .text_px(m.label)
                    .line_height(px(g.line))
                    .text_color(text_c)
                    .children(crate::detail::lines(&shown).into_iter().map(|l| crate::ask::detail_line(l, p, m)));
                let pane = self.panes.cur.id;
                // `more` is an action like any other (§6.2): its painted
                // target, taken at down, checked at up.
                let press =
                    crate::lease::Press { at: crate::lease::Target { row: id, tag: s.block.tag }, act: crate::lease::Act::More, group };
                v_flex()
                    .w_full()
                    .pb(pb)
                    .debug_selector(move || format!("block-note {id}"))
                    .child(div().w_full().overflow_hidden().when(cut == Some(true), |d| d.h(px(g.line * NOTE_LINES as f32))).child(body))
                    .when_some(cut, |d, cut| {
                        d.child(
                            div()
                                .id(("block-more", id))
                                .h(px(g.line))
                                .font_family(fonts::SANS)
                                .text_px(m.meta)
                                .text_color(p.acc)
                                .cursor_pointer()
                                .debug_selector(move || format!("block-more {id}"))
                                .child(if cut { "more" } else { "less" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.block_down(pane, press, cx);
                                    }),
                                )
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.block_up(pane, id, press.act, cx);
                                    }),
                                ),
                        )
                    })
                    .into_any_element()
            }
            Body::Metric(mt) => {
                let value_c = glow(7, bound_value(s, |b| if let Body::Metric(x) = b { Some(x.value.clone()) } else { None }), p.ink);
                let narrow = !fit.notes;
                // The value and its unit (16 characters each, legal): one
                // run at 22 px while it fits its room, else at the label's
                // size; past that each ellipsizes, the unit first, and the
                // tooltip has it whole (Shady #101 P2-11).
                let (value, unit) = (
                    crate::ask::shown(&mt.value, cards_spec::Lines::Single),
                    mt.unit.as_deref().map(|u| crate::ask::shown(u, cards_spec::Lines::Single)),
                );
                let room = (self.panes.cur.layout.column - g.spine - 2. * g.pad) * if narrow { 1. } else { 0.6 };
                let big = (22. * m.r).round();
                let wide = (cols(&value) + unit.as_deref().map_or(0, cols)) as f32 * big * 0.6;
                let size = if wide <= room { big } else { m.label };
                let whole = match &mt.was {
                    Some(w) => {
                        format!("{value}{} · was {}", unit.clone().unwrap_or_default(), crate::ask::shown(w, cards_spec::Lines::Single))
                    }
                    None => format!("{value}{}", unit.clone().unwrap_or_default()),
                };
                let run = |d: Div| d.min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap();
                let value = h_flex()
                    .id(("block-value", id))
                    .min_w_0()
                    .max_w(px(room))
                    .flex_shrink(1.)
                    .font_family(fonts::MONO)
                    .text_px(size)
                    .text_color(value_c)
                    .debug_selector(move || format!("block-value {id}"))
                    .child(run(div()).flex_shrink(1.).child(value))
                    .children(unit.map(|u| run(div()).flex_shrink(2.).child(u)))
                    .tooltip(move |window, cx| crate::overlay::tip(whole.clone(), window, cx));
                let label = div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .text_color(p.dim_t)
                    .child(match &mt.was {
                        Some(w) => format!("{} · was {w}", crate::ask::shown(&mt.label, cards_spec::Lines::Single)),
                        None => crate::ask::shown(&mt.label, cards_spec::Lines::Single),
                    });
                let top = if narrow {
                    v_flex().h(px(g.metric)).justify_center().child(value).child(label)
                } else {
                    h_flex().h(px(g.metric)).gap(px(12.)).items_center().child(value).child(label)
                };
                let series = (!mt.series.is_empty()).then(|| spark(&mt.series, g.series, p));
                v_flex().w_full().pb(pb).debug_selector(move || format!("block-metric {id}")).child(top).children(series).into_any_element()
            }
            Body::File(f) => {
                let _ = f;
                div().into_any_element()
            }
            Body::Group(_) => {
                let mut col = v_flex().w_full().pb(pb).gap(px(4.)).debug_selector(move || format!("block-members {id}"));
                let pane = self.panes.cur.id;
                let group = crate::lease::Target { row: id, tag: s.block.tag };
                for mshown in &s.members {
                    let mid = row_id(&mshown.block.id);
                    // One this desk draws dismissed: its tombstone, in its
                    // place in the group (Shady #110 P2-1).
                    if let Some(line) = self.member_tomb(mshown, mid, group, g, p, m, cx) {
                        col = col.child(line);
                        continue;
                    }
                    let folded = self.panes.cur.block_folds.folded(mid, mshown);
                    // A member is its own target (§6.2): its fold, never
                    // its group's; its group the one it was painted in.
                    let mtag = mshown.block.tag;
                    let press = crate::lease::Press {
                        at: crate::lease::Target { row: mid, tag: mtag },
                        act: crate::lease::Act::Fold,
                        group: Some(group),
                    };
                    col = col.child(
                        v_flex()
                            .relative()
                            .w_full()
                            .pl(px(g.pad))
                            .bg(p.well_d)
                            .debug_selector(move || format!("block-member {id} {mid}"))
                            .child(self.block_head(mshown, mid, p.well_d, folded, streaming, fit, g, p, m, cx))
                            .children((!folded).then(|| self.block_body(mshown, mid, Some(group), streaming, fit, g, p, m, cx)))
                            .child(crate::lease::probe(&self.panes.cur.lease, &self.fx, mid, mtag, id))
                            // Its own target under the pointer (§6.2).
                            .id(("block-member", mid))
                            .on_hover(cx.listener(move |this, over: &bool, _, _| this.member_hover(pane, mid, mtag, *over)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.block_down(pane, press, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.block_up(pane, mid, press.act, cx);
                                }),
                            ),
                    );
                }
                col.into_any_element()
            }
        }
    }

    /// A diff's lines as a block draws them (in the transcript and the
    /// rail): an edit preview's from its session's edits, built as the
    /// file's last call committed ([`crate::files::Edits`]); a diff's hunks,
    /// each its `@@` line and its lines. And its foot, if it has one.
    pub(crate) fn diff_rows(
        &self,
        live: crate::live::LiveId,
        d: &cards_spec::block::DiffBody,
    ) -> (Vec<(Option<crate::files::LineKind>, SharedString)>, &'static str) {
        let l = &self.model.lives[live];
        let mut lines: Vec<(Option<crate::files::LineKind>, SharedString)> = Vec::new();
        let foot = if d.from.is_some() {
            // Built as the file's last call committed (`files::Edits`):
            // a draw only takes it.
            let preview = l.shown_files().edit_preview(&d.file).unwrap_or_default();
            let (groups, earlier) = (&preview.groups, preview.earlier);
            if earlier > 0 {
                lines.push((None, format!("+{earlier} earlier edit{}", if earlier == 1 { "" } else { "s" }).into()));
            }
            for gr in groups {
                if let Some(n) = gr.written {
                    lines.push((None, format!("{n} lines written").into()));
                }
                for dl in &gr.lines {
                    let indent = " ".repeat(dl.indent as usize);
                    lines.push((Some(dl.kind), format!("{indent}{}", dl.text).into()));
                }
            }
            if groups.is_empty() && l.shown_files().shell().iter().any(|f| crate::files::same_file(&f.path, &d.file)) {
                lines.push((None, "changed by a shell command · no preview".into()));
            } else if groups.is_empty() && l.shown_files().edits_dropped(&d.file) {
                // Its preview went to the ledger's cap (`files::EDIT_FILES`).
                lines.push((None, "edits not kept · too many files changed".into()));
            }
            "as requested · not read from disk"
        } else {
            for h in &d.hunks {
                let word = match h.state {
                    HunkState::Open => "",
                    HunkState::Kept => " · kept",
                    HunkState::Dropped => " · dropped",
                };
                lines.push((None, format!("@@ {}{word}", h.at.as_deref().unwrap_or(h.id.as_str())).into()));
                for t in &h.lines {
                    let kind = match t.chars().next() {
                        Some('+') => crate::files::LineKind::Add,
                        Some('-') => crate::files::LineKind::Del,
                        _ => crate::files::LineKind::Context,
                    };
                    let rest: String = t.chars().skip(1).collect();
                    lines.push((Some(kind), crate::ask::shown(&rest, cards_spec::Lines::Single).into()));
                }
            }
            if d.state == DiffState::Proposed { "PROPOSED · not applied" } else { "" }
        };
        (lines, foot)
    }

    #[allow(clippy::too_many_arguments)]
    fn diff_body(
        &self,
        s: &Shown,
        d: &cards_spec::block::DiffBody,
        id: u64,
        g: &Geo,
        p: &Palette,
        m: &Metrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cw = m.mono * 0.6;
        let (lines, foot) = self.diff_rows(self.panes.cur.live, d);
        let n = lines.len();
        let shown = n.clamp(1, DIFF_LINES);
        let v = bound_value(s, |b| if let Body::Diff(x) = b { Some(x.hunks.iter().map(|h| h.lines.len()).sum::<usize>()) } else { None });
        let fresh = self.fx.glow(motion::key(GLOW_KEYS, id ^ 8u64.rotate_left(13)), v ^ n as u64, p.well_d);
        let mut col = v_flex().w_full();
        for (kind, text) in lines {
            let (sign, fg, sign_fg, bg) = match kind {
                Some(crate::files::LineKind::Add) => ("+", p.ink, p.ok_o, Some(p.add_bg)),
                Some(crate::files::LineKind::Del) => ("−", p.ink, p.err_o, Some(p.del_bg)),
                Some(_) => (" ", p.res, p.dim_o, None),
                None => (" ", p.dim_o, p.dim_o, None),
            };
            col = col.child(
                h_flex()
                    .h(px(g.line))
                    .w_full()
                    .items_center()
                    .whitespace_nowrap()
                    .when_some(bg, |d, bg| d.bg(bg))
                    // Tests read a hunk's header and the notes by their text,
                    // and a line by its text.
                    .when(kind.is_none(), |d| {
                        let t = format!("block-line {id} {text}");
                        d.debug_selector(move || t.clone())
                    })
                    .when(kind.is_some(), |d| {
                        let t = text.clone();
                        d.debug_selector(move || format!("block-text {id} {}", t.trim_start()))
                    })
                    .child(div().w(px(18.)).flex_none().flex().justify_center().text_color(sign_fg).child(sign))
                    .child(div().flex_none().text_color(fg).child(text)),
            );
        }
        let handle = self.panes.cur.block_scroll.borrow_mut().entry(id).or_default().clone();
        let pinned = handle.max_offset().y <= px(0.) || handle.offset().y <= -handle.max_offset().y + px(1.);
        let well = div()
            .id(("block-well", id))
            .w_full()
            .h(px(g.line * shown as f32))
            .overflow_scroll()
            .track_scroll(&handle)
            .bg(fresh)
            .font_family(fonts::MONO)
            .text_px(m.mono)
            .debug_selector(move || format!("block-well {id}"))
            .child(col.min_w(px(text_w_max(cw))));
        // Appends keep the well on its newest line unless the user scrolled it (§6.2).
        if pinned {
            handle.scroll_to_bottom();
        }
        let _ = cx;
        v_flex()
            .w_full()
            .pb(px((6. * m.r).round()))
            .debug_selector(move || format!("block-diff {id}"))
            .child(well)
            .when(!foot.is_empty(), |dv| {
                dv.child(div().h(px(g.line)).flex().items_center().font_family(fonts::SANS).text_px(m.meta).text_color(p.dim_t).child(foot))
            })
            .into_any_element()
    }
}

/// A past revision's header while its body isn't drawn (D7, §3.6): what
/// the frame says (`loading r2…`, `r2 unavailable · why`) where the title
/// is, and its label where `rN` is. Nothing of the live block.
fn past_wait_head(id: u64, wait: &str, label: &str, g: &Geo, p: &Palette, m: &Metrics) -> Div {
    let (w, l) = (wait.to_string(), label.to_string());
    h_flex()
        .relative()
        .h(px(g.head))
        .w_full()
        .gap(px(g.gap))
        .items_center()
        .whitespace_nowrap()
        .font_family(fonts::SANS)
        .text_px(m.label)
        .debug_selector(move || format!("block-head {id}"))
        .child(
            div()
                .min_w_0()
                .flex_shrink(1.)
                .overflow_hidden()
                .text_ellipsis()
                .text_color(p.dim_t)
                .debug_selector(move || format!("block-past-wait {id} {w}"))
                .child(wait.to_string()),
        )
        .child(div().flex_1())
        .child(
            div()
                .min_w_0()
                .flex_shrink(1.)
                .overflow_hidden()
                .text_ellipsis()
                .font_family(fonts::MONO)
                .text_px(m.meta)
                .text_color(p.acc)
                .debug_selector(move || format!("block-rev {id} {l}"))
                .child(label.to_string()),
        )
}

/// A group's past revision (D7, §3.6): its members then, a line each (their
/// own past is theirs to walk), read-only.
fn past_members(ps: &Shown, id: u64, p: &Palette, m: &Metrics) -> AnyElement {
    let pb = px((6. * m.r).round());
    v_flex()
        .w_full()
        .pb(pb)
        .gap(px(4.))
        .debug_selector(move || format!("block-past-members {id}"))
        .children(ps.members.iter().map(|ms| {
            div()
                .h(px((20. * m.r).round()))
                .flex()
                .items_center()
                .font_family(fonts::SANS)
                .text_px(m.label)
                .text_color(p.prose_t)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(format!("{} · r{}", title_of(ms), ms.block.rev))
        }))
        .into_any_element()
}

fn text_w_max(cw: f32) -> f32 {
    cw * 40.
}

/// A metric's series (≤ 32): square bars, `series` px tall, the newest
/// right, in the accent.
pub(crate) fn spark(series: &[cards_spec::block::Num], h: f32, p: &Palette) -> Div {
    let (lo, hi) = series.iter().map(|n| n.get()).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(x), b.max(x)));
    let span = (hi - lo).max(f64::EPSILON);
    h_flex().h(px(h)).items_end().gap(px(2.)).children(series.iter().map(|n| {
        let k = if hi > lo { ((n.get() - lo) / span) as f32 } else { 1. };
        div().w(px(4.)).h(px((2. + (h - 2.) * k).round())).bg(motion::mix(p.card_d, p.acc, 0.7))
    }))
}

impl AppView {
    /// The block row `id` and its block as drawn in this pane now, and its
    /// list index.
    pub(crate) fn block_shown(&self, id: u64) -> Option<(usize, Shown)> {
        let ix = self.panes.cur.disp.iter().position(|d| d.id == id && matches!(d.kind, Kind::Block { .. }))?;
        let d = self.panes.cur.disp[ix];
        let Kind::Block { at } = d.kind else { return None };
        let items = self.model.lives[self.panes.cur.live].shown().items();
        let s = row_block(items.get(d.item)?, at, &self.panes.cur.pinned_in)?.clone();
        Some((ix, s))
    }

    /// Block row `id` as drawn in this pane now, a top-level block or a
    /// group's member, and the list index of its row (a member's: its
    /// group's).
    pub(crate) fn block_drawn(&self, id: u64) -> Option<(usize, Shown)> {
        if let Some(found) = self.block_shown(id) {
            return Some(found);
        }
        let items = self.model.lives[self.panes.cur.live].shown().items();
        self.panes.cur.disp.iter().enumerate().find_map(|(ix, d)| {
            let Kind::Block { at } = d.kind else { return None };
            let s = row_block(items.get(d.item)?, at, &self.panes.cur.pinned_in)?;
            s.members.iter().find(|m| row_id(&m.block.id) == id).map(|m| (ix, m.clone()))
        })
    }
}
