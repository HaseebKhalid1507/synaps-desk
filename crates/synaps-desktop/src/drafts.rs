//! Drafts: a block taken, edited and held on this desk (synaps-cards
//! `docs/blocks.md` v1.3 §3.2, §3.4, §5.2, §1.8's drafts row; §8.2's D8
//! row, its first half: D8a). The model; [`crate::draft_view`] draws it
//! and routes its keys and clicks.
//!
//! - **A draft is local.** It records the block's id, the incarnation's
//!   tag, the base revision `B` it was taken at on the **bound** board
//!   (§1.7: never over a pending op), and the user's intents by item id
//!   and path ([`Intents`]), never a whole body. The agent isn't told
//!   (§0.2), and nothing is sent while it is held.
//! - **It draws over `B`** (§3.4 step 1): the block as it was bound at `B`
//!   ([`Draft::base`], B2's block, shared with the board it came from)
//!   with the intents on it ([`Draft::view`]). The agent's changes since
//!   are folded on the board as on every desk, and counted here, never
//!   drawn under the user ([`Draft::agent_changes`]).
//! - **The patch** ([`Draft::parts`]) is the user's changes since `B` by
//!   item id and path, in the agent's vocabulary (§1.4, §3.3's line 2):
//!   `append` the steps the user added (`u-` ids, `todo`, no `stop`),
//!   `order` the steps if the user moved one (or placed an added one),
//!   `set` each label, skip, unskip and stop, a note's text and title, a
//!   hunk's keep or drop. A change taken back is no intent.
//! - **The pre-check** ([`Draft::check`], §3.2) runs B2's own guards
//!   (`State::precheck`, §3.4) on the current bound board: what fails now
//!   is listed, and left out of the patch and of line 1; with nothing
//!   left, nothing is handed back. The message for what is left is
//!   `cards_spec::block::message::build`'s, so line 1 and line 2 are the
//!   grammar's (D8b sends it).
//! - **Stale and orphaned** ([`Draft::fate`]): the incarnation ended while
//!   held (a `replace`, a `remove`, a rollback that took its creation
//!   back) and the draft is stale for good: `the agent rewrote this block
//!   (r1)` or `this block is gone`. A compaction (another conversation
//!   generation) makes it an orphan in place: `compacted · this draft has
//!   no block` (§5.2). Neither is ever handed back: Copy to composer puts
//!   line 1's words in the composer ([`Draft::words`]), or Discard.
//! - **Bounded** (§1.8): [`DRAFTS_MAX`] per session, holds and orphans
//!   together, across compactions: a 9th take is refused, and no draft is
//!   ever dropped for space. A draft's own bytes (its intents) are at most
//!   [`DRAFT_MAX_BYTES`], at every change that can grow them (a label, an
//!   added step, a strike, a stop, a note's text or title, a hunk's keep
//!   or drop) and at the take: a change past that is refused and leaves
//!   the draft as it was. The base block it draws over is charged at B2's
//!   ceiling for a block ([`cards_spec::board::BLOCK_MAX_OWNED`]). Across
//!   sessions, at most [`DESK_DRAFTS_MAX`]. An open field's own copy of
//!   the text it edits is the view's ([`crate::draft_view`]).
//! - **Held in place** (§3.4): an agent's `move` or `collapse` of a held
//!   block waits in drawing until the hold ends. A draft keeps where its
//!   block was drawn when taken ([`Place`]), and the board the desk draws
//!   puts it back there ([`hold_places`]); let go, the next board draws it
//!   where the agent put it.
//! - **A draft follows its session** to a new id, the same conversation
//!   ([`Drafts::follow`]): none is dropped; one of the same incarnation as
//!   a draft already there is kept as [`Fate::Doubled`], in the dock, for
//!   Copy to composer or Discard.

use std::collections::BTreeMap;
use std::sync::Arc;

use cards_spec::block::message::{Edit, Summary, build};
use cards_spec::block::{DiffState, Generation, HunkState, ItemId, Kind, PLAN_STEPS_MAX, Ref, Step, StepState, Tag};
use cards_spec::board::{BLOCK_MAX_OWNED, Block, Code, EditResult, Key, State, keys};
use serde_json::{Map, Value, json};
use synaps_client::blocks::{Body, Drawn};

/// Drafts per session, holds and orphans together (§1.8).
pub(crate) const DRAFTS_MAX: usize = 8;

/// Drafts on this desk, across its sessions: 8 sessions' worth.
pub(crate) const DESK_DRAFTS_MAX: usize = 64;

/// A draft's own bytes at most: its intents and field text (§1.8).
pub(crate) const DRAFT_MAX_BYTES: usize = 8 * 1024;

/// A 9th take (§1.8).
pub(crate) const FULL: &str = "8 drafts open · hand back or discard one";

/// A take past [`DESK_DRAFTS_MAX`].
pub(crate) const DESK_FULL: &str = "64 drafts open on this desk · hand back or discard one";

/// A change past [`DRAFT_MAX_BYTES`].
pub(crate) const DRAFT_FULL: &str = "this draft is full · hand back or discard it";

/// A step that ran can't be struck or given a stop (§3.2).
pub(crate) const RAN: &str = "already ran";

/// A hunk's keep or drop on a diff that isn't a proposal.
pub(crate) const NOT_PROPOSAL: &str = "not a proposal";

/// The pre-check left nothing (§3.2).
pub(crate) const NOTHING_LEFT: &str = "nothing left to hand back";

/// An orphan's header (§5.2).
pub(crate) const ORPHAN: &str = "compacted · this draft has no block";

/// A draft that followed its session onto another draft of its block.
pub(crate) const DOUBLED: &str = "another draft of this block is held · copy or discard this one";

/// `s` cut at `n` characters (a character's boundary), kept at its length
/// (what [`Draft::own_bytes`] counts is what it holds).
pub(crate) fn cut(s: &str, n: usize) -> String {
    // Fail-first (Shady #114 P2-1): this fix is off until the next commit.
    s.chars().take(n).collect()
}

/// The kinds a user edits (§3.2): status, metric and file aren't.
pub(crate) fn editable(kind: Kind) -> bool {
    matches!(kind, Kind::Plan | Kind::Note | Kind::Diff)
}

/// What became of a draft's block, judged on the bound board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fate {
    /// Its incarnation is live: held.
    Held,
    /// Another incarnation has the id: `the agent rewrote this block (rN)`,
    /// its revision then.
    Rewritten(u32),
    /// No block has the id: `this block is gone`.
    Gone,
    /// Another conversation generation (a compaction): an orphan (§5.2).
    Orphan,
    /// Its session followed to a new id that held a draft of the same
    /// incarnation already, with changes too ([`Drafts::follow`]): this
    /// one is in the dock, for Copy to composer or Discard.
    Doubled,
}

impl Fate {
    /// The header's words for a draft that ended (none: held).
    pub(crate) fn says(self) -> Option<String> {
        match self {
            Fate::Held => None,
            Fate::Rewritten(r) => Some(format!("the agent rewrote this block (r{r})")),
            Fate::Gone => Some("this block is gone".into()),
            Fate::Orphan => Some(ORPHAN.into()),
            Fate::Doubled => Some(DOUBLED.into()),
        }
    }

    /// Whether a draft of this fate draws in its block's row (else in the
    /// dock): held, or stale there.
    pub(crate) fn in_row(f: Option<Self>) -> bool {
        !matches!(f, Some(Fate::Orphan | Fate::Doubled))
    }
}

/// A plan's intents, by item id (§3.4's user paths).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlanIntents {
    /// The steps' order as drawn, the added ones in place.
    pub order: Vec<ItemId>,
    /// Struck (`true`: `skipped`) or unstruck (`false`: `todo`), unlike `B`.
    pub skip: BTreeMap<ItemId, bool>,
    /// A stop before it, unlike `B`.
    pub stop: BTreeMap<ItemId, bool>,
    /// Its label, unlike `B`.
    pub label: BTreeMap<ItemId, String>,
    /// The steps the user added in this draft (minted `u-` ids), with
    /// their labels. Only these are additions: a bound `u-` step, an
    /// earlier edit's, is one of `B`'s like any other.
    pub added: Vec<(ItemId, String)>,
}

impl PlanIntents {
    /// Whether step `id` is one this draft added.
    pub(crate) fn adds(&self, id: &ItemId) -> bool {
        // Fail-first (Shady #114 P2-3): this fix is off until the next commit.
        id.is_user()
    }
}

/// The user's intents (§3.2's editors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Intents {
    Plan(PlanIntents),
    /// A note's text and title, unlike `B`.
    Note {
        text: Option<String>,
        title: Option<String>,
    },
    /// Each hunk's keep (`kept`) or drop (`dropped`), unlike `B`.
    Diff(BTreeMap<ItemId, HunkState>),
}

/// The pre-check's answer (§3.2): what fails now, and the message for
/// what is left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    /// Paths that would fail now, with why: listed, and left out.
    pub fails: Vec<(Key, Code)>,
    /// The edit message for what is left (line 1, line 2), if anything is.
    pub message: Option<String>,
    /// Why nothing can be handed back, if nothing can (none: `message`).
    pub off: Option<String>,
}

impl Checked {
    /// The footer's line for what fails now: `1 change no longer applies:
    /// deploy already ran`.
    pub(crate) fn footer(&self) -> Option<String> {
        let n = self.fails.len();
        if n == 0 {
            return None;
        }
        let why: Vec<String> = self.fails.iter().map(|(k, c)| reason(k, *c)).collect();
        let lead = if n == 1 { "1 change no longer applies".to_string() } else { format!("{n} changes no longer apply") };
        Some(format!("{lead}: {}", why.join(" · ")))
    }
}

/// Why key `k` fails with `c`, in words.
fn reason(k: &Key, c: Code) -> String {
    let item = match k {
        Key::Item(p) | Key::Set(p) => p.item().unwrap_or(p.head()).to_string(),
        Key::Order(p) => p.head().to_string(),
    };
    match c {
        Code::AlreadyRan => format!("{item} already ran"),
        Code::NotThere => format!("{item} is gone"),
        Code::Collided => "an added step collided · add it again".into(),
        Code::Full => "the plan is full".into(),
        Code::NotProposal => "the diff is no longer a proposal".into(),
        Code::TooBig => "too big for the block".into(),
        Code::Applied | Code::Unchanged | Code::Overwritten => format!("{item}: no change"),
    }
}

/// One draft (module docs).
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub id: Ref,
    pub tag: Tag,
    /// `B`: the revision it was taken at.
    pub base: u32,
    pub generation: Generation,
    /// The block as bound at `B`: what it draws over.
    pub base_block: Arc<Block>,
    pub intents: Intents,
    /// What became of its block, for good once it ended.
    pub ended: Option<Fate>,
    /// Bumped by every change (a view's key).
    pub serial: u64,
    /// Its key on this desk, unique ([`Drafts::take`]): a draft doubled by a
    /// follow has its block's id and tag, never its key.
    pub key: u64,
    /// Where its block was drawn when it was taken (§3.4).
    pub place: Option<Place>,
    /// Its footer as last drawn: the board and serial it was judged on, and
    /// what it said ([`crate::draft_view`]: a change remeasures its row).
    pub foot: Option<((u64, u64), u64)>,
}

impl Draft {
    /// A draft of `b` (bound, at revision `b.rev`) of generation `generation`.
    pub(crate) fn of(b: Arc<Block>, generation: Generation) -> Option<Self> {
        let intents = match &b.body {
            Body::Plan(p) => Intents::Plan(PlanIntents { order: p.steps.iter().map(|s| s.id.clone()).collect(), ..Default::default() }),
            Body::Note(_) => Intents::Note { text: None, title: None },
            Body::Diff(_) => Intents::Diff(BTreeMap::new()),
            _ => return None,
        };
        Some(Draft {
            id: b.id.clone(),
            tag: b.tag,
            base: b.rev,
            generation,
            base_block: b,
            intents,
            ended: None,
            serial: 0,
            key: 0,
            place: None,
            foot: None,
        })
    }

    /// Its base body's steps, if it is a plan's.
    fn base_steps(&self) -> &[Step] {
        match &self.base_block.body {
            Body::Plan(p) => &p.steps,
            _ => &[],
        }
    }

    fn base_step(&self, id: &ItemId) -> Option<&Step> {
        self.base_steps().iter().find(|s| &s.id == id)
    }

    /// Whether it holds any change.
    pub(crate) fn changed(&self) -> bool {
        match &self.intents {
            Intents::Plan(p) => !(p.skip.is_empty() && p.stop.is_empty() && p.label.is_empty() && p.added.is_empty()) || self.reordered(),
            Intents::Note { text, title } => text.is_some() || title.is_some(),
            Intents::Diff(h) => !h.is_empty(),
        }
    }

    /// Whether the user moved one of `B`'s steps, a bound `u-` one included
    /// (an added step placed anywhere but last is an order too:
    /// [`Draft::placed`]).
    fn reordered(&self) -> bool {
        let Intents::Plan(p) = &self.intents else { return false };
        p.order.iter().filter(|i| !p.adds(i)).ne(self.base_steps().iter().map(|s| &s.id))
    }

    /// Whether line 2 carries an order: the drawn order isn't `B`'s with
    /// the added steps after it, as an append alone puts them.
    fn placed(&self) -> bool {
        let Intents::Plan(p) = &self.intents else { return false };
        let natural = self.base_steps().iter().map(|s| &s.id).chain(p.added.iter().map(|(i, _)| i));
        p.order.iter().ne(natural)
    }

    /// The block as drawn while held: `B` with the intents on it.
    pub(crate) fn view(&self) -> Block {
        let mut b = (*self.base_block).clone();
        match (&mut b.body, &self.intents) {
            (Body::Plan(pl), Intents::Plan(p)) => {
                let mut steps = Vec::with_capacity(p.order.len());
                for id in &p.order {
                    let mut s = match self.base_step(id) {
                        Some(s) => s.clone(),
                        None => match p.added.iter().find(|(i, _)| i == id) {
                            Some((i, l)) => Step { id: i.clone(), label: l.clone(), state: StepState::Todo, note: None, stop: false },
                            None => continue,
                        },
                    };
                    if let Some(&k) = p.skip.get(id) {
                        s.state = if k { StepState::Skipped } else { StepState::Todo };
                    }
                    if let Some(&k) = p.stop.get(id) {
                        s.stop = k;
                    }
                    if let Some(l) = p.label.get(id) {
                        s.label = l.clone();
                    }
                    steps.push(s);
                }
                pl.steps = steps;
            }
            (Body::Note(n), Intents::Note { text, title }) => {
                if let Some(t) = text {
                    n.text = t.clone();
                }
                if let Some(t) = title {
                    b.title = Some(t.clone());
                }
            }
            (Body::Diff(d), Intents::Diff(h)) => {
                for x in &mut d.hunks {
                    if let Some(&s) = h.get(&x.id) {
                        x.state = s;
                    }
                }
            }
            _ => {}
        }
        b
    }

    // ── the editors (§3.2) ─────────────────────────────────────────────

    /// Strike or unstrike step `id`: `Err` says why it can't (a step that
    /// ran). An added step struck is taken out of the draft.
    pub(crate) fn strike(&mut self, id: &ItemId) -> Result<(), &'static str> {
        let base = self.base_step(id).map(|s| s.state);
        let before = self.intents.clone();
        let Intents::Plan(p) = &mut self.intents else { return Err("not a plan") };
        match base {
            None if p.added.iter().any(|(i, _)| i == id) => {
                p.added.retain(|(i, _)| i != id);
                p.order.retain(|i| i != id);
            }
            None => return Err("not a step"),
            Some(StepState::Todo | StepState::Skipped) => {
                let was = base == Some(StepState::Skipped);
                let now = !p.skip.get(id).copied().unwrap_or(was);
                if now == was {
                    p.skip.remove(id);
                } else {
                    p.skip.insert(id.clone(), now);
                }
            }
            Some(_) => return Err(RAN),
        }
        // Fail-first (Shady #114 P2-1): this fix is off until the next commit.
        drop(before);
        self.serial += 1;
        Ok(())
    }

    /// Toggle a stop before step `id` (§3.2): only a `todo` or `skipped`
    /// step of `B` takes one; an added step never does.
    pub(crate) fn stop(&mut self, id: &ItemId) -> Result<(), &'static str> {
        let Some(base) = self.base_step(id).map(|s| (s.state, s.stop)) else { return Err("an added step takes no stop") };
        let before = self.intents.clone();
        let Intents::Plan(p) = &mut self.intents else { return Err("not a plan") };
        if !matches!(base.0, StepState::Todo | StepState::Skipped) {
            return Err(RAN);
        }
        let now = !p.stop.get(id).copied().unwrap_or(base.1);
        if now == base.1 {
            p.stop.remove(id);
        } else {
            p.stop.insert(id.clone(), now);
        }
        // Fail-first (Shady #114 P2-1): this fix is off until the next commit.
        drop(before);
        self.serial += 1;
        Ok(())
    }

    /// Move step `id` one place up (`-1`) or down (`1`): text and order
    /// are the user's, a step that ran included (§3.4).
    pub(crate) fn shift(&mut self, id: &ItemId, by: isize) -> bool {
        let Intents::Plan(p) = &mut self.intents else { return false };
        let Some(i) = p.order.iter().position(|x| x == id) else { return false };
        let Some(j) = i.checked_add_signed(by).filter(|j| *j < p.order.len()) else { return false };
        p.order.swap(i, j);
        self.serial += 1;
        true
    }

    /// Step `id`'s label as the user typed it (cut at B1's cap): a label
    /// back to `B`'s is no intent. `Err`: the draft would pass its cap.
    pub(crate) fn rename(&mut self, id: &ItemId, label: &str) -> Result<(), &'static str> {
        let label = cut(label, cards_spec::block::STEP_LABEL_MAX);
        let was = self.base_step(id).map(|s| s.label.clone());
        let before = self.intents.clone();
        let Intents::Plan(p) = &mut self.intents else { return Err("not a plan") };
        if let Some((_, l)) = p.added.iter_mut().find(|(i, _)| i == id) {
            *l = label;
        } else if was.as_deref() == Some(label.as_str()) {
            p.label.remove(id);
        } else if was.is_some() {
            p.label.insert(id.clone(), label);
        } else {
            return Err("not a step");
        }
        self.within(before)
    }

    /// A step added after `after` (first, if none), labelled `label`, its
    /// id `id` (`u-`, minted by the caller from the OS's randomness). `Err`
    /// with why: the plan would pass 20 steps, or the draft its cap.
    pub(crate) fn add(&mut self, after: Option<&ItemId>, id: ItemId, label: &str) -> Result<(), &'static str> {
        let before = self.intents.clone();
        let Intents::Plan(p) = &mut self.intents else { return Err("not a plan") };
        if p.order.len() >= PLAN_STEPS_MAX {
            return Err("the plan is full");
        }
        let at = after.and_then(|a| p.order.iter().position(|x| x == a)).map_or(0, |i| i + 1);
        p.order.insert(at, id.clone());
        p.added.push((id, cut(label, cards_spec::block::STEP_LABEL_MAX)));
        self.within(before)
    }

    /// A note's text or title as typed: back to `B`'s is no intent.
    pub(crate) fn note(&mut self, title: bool, s: &str) -> Result<(), &'static str> {
        let (bt, bx) = (self.base_block.title.clone().unwrap_or_default(), self.base_text());
        let before = self.intents.clone();
        let Intents::Note { text, title: t } = &mut self.intents else { return Err("not a note") };
        if title {
            let s = cut(s, cards_spec::block::TITLE_MAX);
            *t = (s != bt).then_some(s);
        } else {
            let s = cut(s, cards_spec::block::NOTE_TEXT_MAX);
            *text = (s != bx).then_some(s);
        }
        self.within(before)
    }

    /// Its base note's text.
    fn base_text(&self) -> String {
        match &self.base_block.body {
            Body::Note(n) => n.text.clone(),
            _ => String::new(),
        }
    }

    /// Keep (`true`) or drop hunk `id`: only while the diff is a proposal.
    /// The same again takes it back.
    pub(crate) fn hunk(&mut self, id: &ItemId, keep: bool) -> Result<(), &'static str> {
        let Body::Diff(d) = &self.base_block.body else { return Err("not a diff") };
        if d.state != DiffState::Proposed {
            return Err(NOT_PROPOSAL);
        }
        let Some(base) = d.hunks.iter().find(|h| &h.id == id).map(|h| h.state) else { return Err("not a hunk") };
        let before = self.intents.clone();
        let Intents::Diff(h) = &mut self.intents else { return Err("not a diff") };
        let want = if keep { HunkState::Kept } else { HunkState::Dropped };
        let now = h.get(id).copied().unwrap_or(base);
        if now == want || want == base {
            h.remove(id);
        } else {
            h.insert(id.clone(), want);
        }
        // Fail-first (Shady #114 P2-1): this fix is off until the next commit.
        drop(before);
        self.serial += 1;
        Ok(())
    }

    /// The change just made stands if the draft is within its cap; else it
    /// is taken back (`before`), its serial as it was: every editor that
    /// can grow the draft comes through here.
    fn within(&mut self, before: Intents) -> Result<(), &'static str> {
        if self.own_bytes() > DRAFT_MAX_BYTES {
            self.intents = before;
            return Err(DRAFT_FULL);
        }
        self.serial += 1;
        Ok(())
    }

    /// Its own bytes (§1.8's model: inline sizes, strings and vectors by
    /// capacity, a map entry at twice its key and value plus a node's
    /// share): its intents, not the base block.
    pub(crate) fn own_bytes(&self) -> usize {
        fn id(i: &ItemId) -> usize {
            i.as_str().len()
        }
        let entry = |k: usize, v: usize| 2 * (k + v) + 16;
        let inline = std::mem::size_of::<Draft>();
        inline
            + match &self.intents {
                Intents::Plan(p) => {
                    p.order.capacity() * std::mem::size_of::<ItemId>()
                        + p.order.iter().map(id).sum::<usize>()
                        + p.skip.keys().map(|k| entry(std::mem::size_of::<ItemId>(), 1) + id(k)).sum::<usize>()
                        + p.stop.keys().map(|k| entry(std::mem::size_of::<ItemId>(), 1) + id(k)).sum::<usize>()
                        + p.label
                            .iter()
                            .map(|(k, v)| entry(std::mem::size_of::<ItemId>(), std::mem::size_of::<String>()) + id(k) + v.capacity())
                            .sum::<usize>()
                        + p.added.capacity() * std::mem::size_of::<(ItemId, String)>()
                        + p.added.iter().map(|(k, v)| id(k) + v.capacity()).sum::<usize>()
                }
                Intents::Note { text, title } => text.as_ref().map_or(0, String::capacity) + title.as_ref().map_or(0, String::capacity),
                Intents::Diff(h) => {
                    h.keys().map(|k| entry(std::mem::size_of::<ItemId>(), std::mem::size_of::<HunkState>()) + id(k)).sum::<usize>()
                }
            }
    }

    /// What it keeps in all: its own, and the base block at B2's ceiling.
    pub(crate) fn bytes(&self) -> usize {
        self.own_bytes() + BLOCK_MAX_OWNED + 2 * std::mem::size_of::<usize>()
    }

    // ── the patch, line 1, the pre-check ──────────────────────────────

    /// The patch (§3.3's line 2 parts: `append`, `order`, `set`), of every
    /// intent, or of those `keep` says.
    pub(crate) fn parts(&self, keep: &dyn Fn(&Key) -> bool) -> Value {
        let mut set = Map::new();
        let mut append = Vec::new();
        let mut order = None;
        let path = |s: &str| cards_spec::block::Path::parse(s).expect("a user path");
        match &self.intents {
            Intents::Plan(p) => {
                for (i, l) in &p.added {
                    if keep(&Key::Item(path(&format!("steps.{}", i.as_str())))) {
                        append.push(json!({"id": i.as_str(), "label": l, "state": "todo"}));
                    }
                }
                for (i, l) in &p.label {
                    let k = format!("steps.{}.label", i.as_str());
                    if keep(&Key::Set(path(&k))) {
                        set.insert(k, json!(l));
                    }
                }
                for (i, &s) in &p.skip {
                    let k = format!("steps.{}.state", i.as_str());
                    if keep(&Key::Set(path(&k))) {
                        set.insert(k, json!(if s { "skipped" } else { "todo" }));
                    }
                }
                for (i, &s) in &p.stop {
                    let k = format!("steps.{}.stop", i.as_str());
                    if keep(&Key::Set(path(&k))) {
                        set.insert(k, json!(s));
                    }
                }
                if self.placed() && keep(&Key::Order(path("steps"))) {
                    // A step this draft added, left out, isn't named; every
                    // step of `B` is, a bound `u-` one too.
                    let named: Vec<&str> = p
                        .order
                        .iter()
                        .filter(|i| !p.adds(i) || keep(&Key::Item(path(&format!("steps.{}", i.as_str())))))
                        .map(ItemId::as_str)
                        .collect();
                    order = Some(json!({"steps": named}));
                }
            }
            Intents::Note { text, title } => {
                if let Some(t) = text.as_ref().filter(|_| keep(&Key::Set(path("text")))) {
                    set.insert("text".into(), json!(t));
                }
                if let Some(t) = title.as_ref().filter(|_| keep(&Key::Set(path("title")))) {
                    set.insert("title".into(), json!(t));
                }
            }
            Intents::Diff(h) => {
                for (i, s) in h {
                    let k = format!("hunks.{}.state", i.as_str());
                    if keep(&Key::Set(path(&k))) {
                        set.insert(k, json!(s.as_str()));
                    }
                }
            }
        }
        let mut o = Map::new();
        if !append.is_empty() {
            o.insert("append".into(), json!({"steps": append}));
        }
        if let Some(v) = order {
            o.insert("order".into(), v);
        }
        if !set.is_empty() {
            o.insert("set".into(), Value::Object(set));
        }
        Value::Object(o)
    }

    /// Line 1's pieces (§3.3) for the intents `keep` says, in the order
    /// the examples write them: the order, skips, unskips, stops, renames,
    /// added steps; a hunk's keeps then drops; a note's text and title.
    pub(crate) fn pieces(&self, keep: &dyn Fn(&Key) -> bool) -> Vec<String> {
        let path = |s: &str| cards_spec::block::Path::parse(s).expect("a user path");
        let set = |k: String| keep(&Key::Set(path(&k)));
        let list = |ids: Vec<&ItemId>| ids.iter().map(|i| i.as_str()).collect::<Vec<_>>().join(", ");
        let mut out = Vec::new();
        match &self.intents {
            Intents::Plan(p) => {
                if self.reordered() && keep(&Key::Order(path("steps"))) {
                    out.push(format!("order {}", list(p.order.iter().filter(|i| !p.adds(i)).collect())));
                }
                let pick = |m: &BTreeMap<ItemId, bool>, on: bool, field: &str| -> Vec<&ItemId> {
                    p.order.iter().filter(|i| m.get(*i) == Some(&on) && set(format!("steps.{}.{field}", i.as_str()))).collect()
                };
                for (ids, word) in [
                    (pick(&p.skip, true, "state"), "skip"),
                    (pick(&p.skip, false, "state"), "unskip"),
                    (pick(&p.stop, true, "stop"), "stop before"),
                    (pick(&p.stop, false, "stop"), "no stop before"),
                ] {
                    if !ids.is_empty() {
                        out.push(format!("{word} {}", list(ids)));
                    }
                }
                for i in p.order.iter().filter(|i| p.label.contains_key(*i)) {
                    if set(format!("steps.{}.label", i.as_str())) {
                        out.push(format!("rename {} “{}”", i.as_str(), p.label[i]));
                    }
                }
                for (i, l) in &p.added {
                    if keep(&Key::Item(path(&format!("steps.{}", i.as_str())))) {
                        let at = p.order.iter().position(|x| x == i).unwrap_or(0);
                        let after = p.order[..at].iter().rev().find(|x| !p.adds(x)).or(at.checked_sub(1).map(|k| &p.order[k]));
                        out.push(match after {
                            Some(a) => format!("add “{l}” after {}", a.as_str()),
                            None => format!("add “{l}” first"),
                        });
                    }
                }
            }
            Intents::Note { text, title } => {
                if let Some(t) = text.as_ref().filter(|_| set("text".into())) {
                    out.push(format!("text “{t}”"));
                }
                if let Some(t) = title.as_ref().filter(|_| set("title".into())) {
                    out.push(format!("title “{t}”"));
                }
            }
            Intents::Diff(h) => {
                for (st, word) in [(HunkState::Kept, "keep"), (HunkState::Dropped, "drop")] {
                    let ids: Vec<&ItemId> =
                        h.iter().filter(|(i, s)| **s == st && set(format!("hunks.{}.state", i.as_str()))).map(|(i, _)| i).collect();
                    if !ids.is_empty() {
                        out.push(format!("{word} {}", list(ids)));
                    }
                }
            }
        }
        out
    }

    /// The edit of the intents `keep` says, and its message (`Err`: why
    /// there is none, as `Edit::new` and `build` say).
    fn edit_of(&self, keep: &dyn Fn(&Key) -> bool) -> Result<(Edit, String), String> {
        let edit = Edit::new(self.id.clone(), self.base, self.tag, &self.parts(keep)).map_err(|e| e.to_string())?;
        let pieces = self.pieces(keep);
        let summary =
            Summary { title: self.base_block.title.as_deref(), back_to: None, pieces: pieces.iter().map(String::as_str).collect() };
        let text = build(&edit, &summary).map_err(|e| e.to_string())?;
        Ok((edit, text))
    }

    /// The pre-check at hand-back (§3.2) on `board`, the current bound
    /// board: B2's guards on every intent; what fails is listed and left
    /// out; the message for what is left.
    pub(crate) fn check(&self, board: &State) -> Checked {
        let off = |s: &str| Checked { fails: Vec::new(), message: None, off: Some(s.to_string()) };
        if let Some(f) = self.ended {
            return off(&f.says().unwrap_or_default());
        }
        if !self.changed() {
            return off("no changes yet");
        }
        let all = |_: &Key| true;
        let (edit, _) = match self.edit_of(&all) {
            Ok(e) => e,
            Err(e) => return off(&e),
        };
        let Some(o) = board.precheck(&edit) else { return off("not an edit of this block") };
        match o.result {
            EditResult::Applied => {}
            EditResult::Rewritten => return off("the block was rewritten"),
            EditResult::Gone => return off("this block is gone"),
            EditResult::Revisions => return off("replace the block or use a new id"),
        }
        let ks = keys(&edit);
        let fails: Vec<(Key, Code)> =
            ks.iter().zip(&o.codes).filter(|(_, c)| !matches!(c, Code::Applied | Code::Unchanged)).map(|(k, c)| (k.clone(), *c)).collect();
        let applies: Vec<&Key> = ks.iter().zip(&o.codes).filter(|(_, c)| **c == Code::Applied).map(|(k, _)| k).collect();
        if applies.is_empty() {
            return Checked { fails, message: None, off: Some(NOTHING_LEFT.into()) };
        }
        let keep = |k: &Key| applies.contains(&k);
        match self.edit_of(&keep) {
            Ok((_, text)) => Checked { fails, message: Some(text), off: None },
            Err(e) => Checked { fails, message: None, off: Some(e) },
        }
    }

    /// Copy to composer's words (§3.2): line 1's, without its `[block …]`
    /// prefix, of every intent. A stale patch is never sent; its words are
    /// the user's to send as typed text.
    pub(crate) fn words(&self) -> String {
        let all = |_: &Key| true;
        if let Ok((_, text)) = self.edit_of(&all) {
            let line1 = text.split('\n').next().unwrap_or_default();
            if let Some((_, rest)) = line1.split_once("] ") {
                return rest.to_string();
            }
        }
        let name = self.base_block.title.clone().unwrap_or_else(|| self.id.to_string());
        format!("{name} → {}", self.pieces(&all).join(" · "))
    }

    /// What became of its block on `d` (module docs): a draft that ended
    /// stays ended. Judged on the bound board, the board itself when `d`
    /// keeps it (never a pending op); else the block's bound copy, where
    /// `d` draws it. Without the board, a block `d` doesn't draw says
    /// nothing: the live overlay hides one a pending `remove` or `move`
    /// names, and the bound board can't say yet (held, until it does).
    pub(crate) fn fate(&self, d: &Drawn) -> Fate {
        if let Some(f) = self.ended {
            return f;
        }
        if d.generation.is_some_and(|g| g != self.generation) {
            return Fate::Orphan;
        }
        let (tag, rev) = match &d.board {
            Some(st) => match st.block(&self.id) {
                Some(b) => (b.tag, b.rev),
                None => return Fate::Gone,
            },
            None => {
                let Some((s, _)) = crate::lease::find_any(d, crate::blocks::row_id(&self.id)) else {
                    // Fail-first (Shady #114 P2-5): this fix is off until the next commit.
                    return Fate::Gone;
                };
                match s.bound.as_ref() {
                    Some(b) => (b.tag, b.rev),
                    // Pending only: the bound board can't say yet.
                    None => return Fate::Held,
                }
            }
        };
        if tag == self.tag { Fate::Held } else { Fate::Rewritten(rev) }
    }

    /// The agent's changes to its incarnation since `B` (§3.2's `· 2 agent
    /// changes`), as the bound board `st` counts them: its revisions since
    /// `B` less the users' (pending ones count once bound).
    pub(crate) fn agent_changes(&self, st: &State) -> u32 {
        let Some(b) = st.block(&self.id).filter(|b| b.tag == self.tag) else { return 0 };
        let users = st.records_of(self.tag).filter(|r| r.rev > self.base && r.author == cards_spec::block::Author::User).count() as u32;
        b.rev.saturating_sub(self.base).saturating_sub(users)
    }
}

/// Where a held block is drawn, as it was when taken (§3.4: the agent's
/// `move` and `collapse` of it wait in drawing until the hold ends): its
/// list (a transcript row's blocks, or a rail slot's sections), its place
/// in it, and the block before it there.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Place {
    /// The `at`th block anchored at transcript row `item`.
    Row { item: u64, at: usize, after: Option<Ref> },
    /// The `at`th section of rail slot `slot` (its creating call's row
    /// `row`: a section the user put into the transcript draws there).
    Rail { slot: usize, at: usize, row: Option<u64>, after: Option<Ref> },
}

impl Place {
    /// Where top-level block `id` is on board `d`.
    pub(crate) fn of(d: &Drawn, id: &Ref) -> Option<Place> {
        for (item, rb) in &d.rows {
            if let Some(at) = rb.blocks.iter().position(|s| &s.block.id == id) {
                let after = at.checked_sub(1).map(|k| rb.blocks[k].block.id.clone());
                return Some(Place::Row { item: *item, at, after });
            }
        }
        for (slot, list) in d.rail.iter().enumerate() {
            if let Some(at) = list.iter().position(|s| &s.block.id == id) {
                let row = d.rail_rows[slot].get(at).copied().flatten();
                let after = at.checked_sub(1).map(|k| list[k].block.id.clone());
                return Some(Place::Rail { slot, at, row, after });
            }
        }
        None
    }
}

/// Board `d` with each of `holds`' blocks (`(id, place)`) put back where it
/// was when its draft was taken (§3.4), a group's member taken out of its
/// group, none drawn twice; `None`: each is in its place, or gone (its
/// draft is in the dock). What the board says of each block is the
/// board's: only where it draws changes.
pub(crate) fn hold_places(d: &Drawn, holds: &[(Ref, Place)]) -> Option<Drawn> {
    // Fail-first (Shady #114 P2-6): this fix is off until the next commit.
    if !holds.is_empty() {
        return None;
    }
    let mut out: Option<Drawn> = None;
    for (id, place) in holds {
        let cur = out.as_ref().unwrap_or(d);
        let now = Place::of(cur, id);
        let after_there = |x: &Drawn, after: &Option<Ref>| match (place, after) {
            (_, None) => false,
            (Place::Row { item, .. }, Some(a)) => x.rows.get(item).is_some_and(|r| r.blocks.iter().any(|s| &s.block.id == a)),
            (Place::Rail { slot, .. }, Some(a)) => x.rail[*slot].iter().any(|s| &s.block.id == a),
        };
        let after = match place {
            Place::Row { after, .. } | Place::Rail { after, .. } => after,
        };
        let in_place = match (place, now.as_ref()) {
            (Place::Row { item, .. }, Some(Place::Row { item: i, after: a, .. })) => item == i && (a == after || !after_there(cur, after)),
            (Place::Rail { slot, .. }, Some(Place::Rail { slot: s, after: a, .. })) => {
                slot == s && (a == after || !after_there(cur, after))
            }
            _ => false,
        };
        // In its place, or gone (its draft is in the dock): as it is.
        if in_place || (now.is_none() && !member(cur, id)) {
            continue;
        }
        let x = out.get_or_insert_with(|| d.clone());
        let Some(s) = take_out(x, id) else { continue };
        let there = after_there(x, after);
        match place {
            Place::Row { item, at, after } => {
                let rb = std::sync::Arc::make_mut(x.rows.entry(*item).or_default());
                let k = match after.as_ref().filter(|_| there) {
                    Some(a) => rb.blocks.iter().position(|s| &s.block.id == a).map_or(0, |k| k + 1),
                    None if after.is_none() => 0,
                    None => (*at).min(rb.blocks.len()),
                };
                rb.blocks.insert(k, s);
            }
            Place::Rail { slot, at, row, after } => {
                let list = &mut x.rail[*slot];
                let k = match after.as_ref().filter(|_| there) {
                    Some(a) => list.iter().position(|s| &s.block.id == a).map_or(0, |k| k + 1),
                    None if after.is_none() => 0,
                    None => (*at).min(list.len()),
                };
                list.insert(k, s);
                let rows = &mut x.rail_rows[*slot];
                rows.insert(k.min(rows.len()), *row);
            }
        }
    }
    out
}

/// Whether block `id` is a group's member on board `d`.
fn member(d: &Drawn, id: &Ref) -> bool {
    d.rows.values().flat_map(|r| r.blocks.iter()).chain(d.rail.iter().flatten()).any(|g| g.members.iter().any(|m| &m.block.id == id))
}

/// Block `id` taken out of board `d`, wherever it draws: a transcript
/// row's, a rail slot's (its creating call's row with it), or a group's
/// member.
fn take_out(d: &mut Drawn, id: &Ref) -> Option<synaps_client::blocks::Shown> {
    let row_of = |rb: &synaps_client::blocks::RowBlocks| {
        rb.blocks.iter().position(|s| &s.block.id == id || s.members.iter().any(|m| &m.block.id == id))
    };
    if let Some(item) = d.rows.iter().find_map(|(item, rb)| row_of(rb).map(|_| *item)) {
        let rb = std::sync::Arc::make_mut(d.rows.get_mut(&item)?);
        let j = row_of(rb)?;
        if &rb.blocks[j].block.id == id {
            return Some(rb.blocks.remove(j));
        }
        let g = &mut rb.blocks[j];
        let m = g.members.iter().position(|m| &m.block.id == id)?;
        return Some(g.members.remove(m));
    }
    for slot in 0..d.rail.len() {
        if let Some(j) = d.rail[slot].iter().position(|s| &s.block.id == id) {
            if j < d.rail_rows[slot].len() {
                d.rail_rows[slot].remove(j);
            }
            return Some(d.rail[slot].remove(j));
        }
        for g in &mut d.rail[slot] {
            if let Some(m) = g.members.iter().position(|m| &m.block.id == id) {
                return Some(g.members.remove(m));
            }
        }
    }
    None
}

/// The desk's drafts (module docs): each session's, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Drafts {
    all: Vec<(String, Draft)>,
    /// The last key given ([`Draft::key`]).
    keys: u64,
}

impl Drafts {
    /// Session `session`'s drafts, oldest first.
    pub(crate) fn of<'a>(&'a self, session: &'a str) -> impl Iterator<Item = &'a Draft> + 'a {
        self.all.iter().filter(move |(s, _)| s == session).map(|(_, d)| d)
    }

    /// How many drafts session `session` has (holds and orphans).
    pub(crate) fn count(&self, session: &str) -> usize {
        self.of(session).count()
    }

    pub(crate) fn len(&self) -> usize {
        self.all.len()
    }

    /// Session `session`'s draft of block `id`'s incarnation `tag` (not
    /// one doubled by a follow: that one's key names it). Tests'.
    #[cfg(test)]
    pub(crate) fn get(&self, session: &str, id: &Ref, tag: Tag) -> Option<&Draft> {
        self.all.iter().find(|(s, d)| s == session && &d.id == id && d.tag == tag && d.ended != Some(Fate::Doubled)).map(|(_, d)| d)
    }

    /// The draft of key `key` ([`Draft::key`]), and its session.
    pub(crate) fn by_key(&self, key: u64) -> Option<(&str, &Draft)> {
        self.all.iter().find(|(_, d)| d.key == key).map(|(s, d)| (s.as_str(), d))
    }

    pub(crate) fn by_key_mut(&mut self, key: u64) -> Option<&mut Draft> {
        self.all.iter_mut().find(|(_, d)| d.key == key).map(|(_, d)| d)
    }

    /// Session `session`'s drafts, to change their view state.
    pub(crate) fn of_mut<'a>(&'a mut self, session: &'a str) -> impl Iterator<Item = &'a mut Draft> + 'a {
        self.all.iter_mut().filter(move |(s, _)| s == session).map(|(_, d)| d)
    }

    /// Session `session`'s draft drawn in block `id`'s row: its latest
    /// that draws in a row ([`Fate::in_row`]: a stale one stays in its row).
    pub(crate) fn in_row(&self, session: &str, id: &Ref) -> Option<&Draft> {
        self.all.iter().rev().find(|(s, d)| s == session && &d.id == id && Fate::in_row(d.ended)).map(|(_, d)| d)
    }

    /// Where session `session`'s drafts drawn in a row keep their blocks
    /// ([`hold_places`]), one a block.
    pub(crate) fn places(&self, session: &str) -> Vec<(Ref, Place)> {
        let mut out: Vec<(Ref, Place)> = Vec::new();
        for (_, d) in self.all.iter().rev().filter(|(s, d)| s == session && Fate::in_row(d.ended)) {
            if let Some(p) = d.place.as_ref()
                && !out.iter().any(|(id, _)| id == &d.id)
            {
                out.push((d.id.clone(), p.clone()));
            }
        }
        out
    }

    /// Session `session`'s drafts of block `id` drawn in a row keep it at
    /// `place` now (`None`: where the agent puts it).
    pub(crate) fn re_place(&mut self, session: &str, id: &Ref, place: Option<Place>) {
        for (_, d) in self.all.iter_mut().filter(|(s, d)| s == session && &d.id == id && Fate::in_row(d.ended)) {
            d.place.clone_from(&place);
        }
    }

    /// Take `b` (bound at its revision), of generation `generation`, for
    /// session `session` (§3.2): its draft if it has one; else a new one,
    /// unless the caps refuse it (why).
    pub(crate) fn take(&mut self, session: &str, b: &Arc<Block>, generation: Generation) -> Result<&mut Draft, &'static str> {
        if let Some(i) = self.all.iter().position(|(s, d)| s == session && d.id == b.id && d.tag == b.tag && d.ended.is_none()) {
            return Ok(&mut self.all[i].1);
        }
        if self.count(session) >= DRAFTS_MAX {
            return Err(FULL);
        }
        if self.all.len() >= DESK_DRAFTS_MAX {
            return Err(DESK_FULL);
        }
        let mut d = Draft::of(b.clone(), generation).ok_or("this block isn't editable")?;
        if d.own_bytes() > DRAFT_MAX_BYTES {
            return Err(DRAFT_FULL);
        }
        self.keys += 1;
        d.key = self.keys;
        self.all.push((session.to_string(), d));
        Ok(&mut self.all.last_mut().expect("pushed").1)
    }

    /// Discard session `session`'s draft of `id`'s incarnation `tag` (not
    /// one doubled by a follow). Tests'.
    #[cfg(test)]
    pub(crate) fn discard(&mut self, session: &str, id: &Ref, tag: Tag) -> bool {
        match self.get(session, id, tag).map(|d| d.key) {
            Some(k) => self.discard_key(k),
            None => false,
        }
    }

    /// Discard the draft of key `key`.
    pub(crate) fn discard_key(&mut self, key: u64) -> bool {
        let n = self.all.len();
        self.all.retain(|(_, d)| d.key != key);
        self.all.len() < n
    }

    /// Session `session`'s drafts judged on its bound board `d`: a block
    /// that ended makes its draft stale for good, a compaction an orphan,
    /// in place (none is dropped). Whether any changed.
    pub(crate) fn judge(&mut self, session: &str, d: &Drawn) -> bool {
        let mut moved = false;
        for (_, x) in self.all.iter_mut().filter(|(s, _)| s == session) {
            if x.ended.is_none() {
                let f = x.fate(d);
                if f != Fate::Held {
                    log::info!(target: "synaps_desktop::drafts", "draft of {} ended: {f:?}", x.id);
                    x.ended = Some(f);
                    x.serial += 1;
                    moved = true;
                }
            }
        }
        moved
    }

    /// Session `session`'s board is gone (no block call is left): its
    /// held drafts' blocks are gone. Whether any were.
    pub(crate) fn gone(&mut self, session: &str) -> bool {
        self.end(session, Fate::Gone)
    }

    /// Session `session` was compacted (§5.2): its held drafts are orphans,
    /// in place. Whether any were.
    pub(crate) fn compacted(&mut self, session: &str) -> bool {
        self.end(session, Fate::Orphan)
    }

    fn end(&mut self, session: &str, f: Fate) -> bool {
        let mut any = false;
        for (_, x) in self.all.iter_mut().filter(|(s, x)| s == session && x.ended.is_none()) {
            log::info!(target: "synaps_desktop::drafts", "draft of {} ended: {f:?}", x.id);
            x.ended = Some(f);
            x.serial += 1;
            any = true;
        }
        any
    }

    /// Session `from` followed to a new id `to`, the same conversation
    /// ([`crate::tabs::Tabs::take_followed`]): its drafts are `to`'s, in
    /// their places, the open ones still open. One of the same incarnation
    /// as a draft `to` holds already: if either has no change, that one
    /// goes (nothing of the user's is lost); else the one that came is
    /// [`Fate::Doubled`], in the dock. None else is dropped: past
    /// [`DRAFTS_MAX`] there, a take is refused until some go. The keys
    /// let go of (drafts with no change).
    pub(crate) fn follow(&mut self, from: &str, to: &str) -> Vec<u64> {
        let mut gone = Vec::new();
        // Fail-first (Shady #114 P2-4): this fix is off until the next commit.
        if from == to || !from.is_empty() {
            return gone;
        }
        let came: Vec<usize> = (0..self.all.len()).filter(|&i| self.all[i].0 == from).collect();
        for i in came {
            let (id, tag) = (self.all[i].1.id.clone(), self.all[i].1.tag);
            let twin = self.all.iter().position(|(s, d)| s == to && d.id == id && d.tag == tag && d.ended != Some(Fate::Doubled));
            if let Some(j) = twin {
                let (a, b) = (&self.all[i].1, &self.all[j].1);
                if !a.changed() {
                    gone.push(a.key);
                } else if !b.changed() {
                    gone.push(b.key);
                } else {
                    let a = &mut self.all[i].1;
                    a.ended = Some(Fate::Doubled);
                    a.serial += 1;
                }
            }
            self.all[i].0 = to.to_string();
        }
        self.all.retain(|(_, d)| !gone.contains(&d.key));
        log::info!(target: "synaps_desktop::drafts", "drafts followed {from} to {to}; {} let go (no change)", gone.len());
        gone
    }

    /// Whether session `session` holds a draft whose block is live (its
    /// board's jobs keep the bound board for its pre-check).
    pub(crate) fn holds(&self, session: &str) -> bool {
        self.of(session).any(|d| d.ended.is_none())
    }

    /// What they keep (bytes): each draft's, and the list's own.
    pub(crate) fn bytes(&self) -> usize {
        self.all.capacity() * std::mem::size_of::<(String, Draft)>()
            + self.all.iter().map(|(s, d)| s.capacity() + d.bytes() - std::mem::size_of::<Draft>()).sum::<usize>()
    }
}

#[cfg(test)]
#[path = "drafts_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "drafts_edge_tests.rs"]
mod edge_tests;
