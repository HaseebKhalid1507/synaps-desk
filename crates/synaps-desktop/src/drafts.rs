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
//!   ever dropped for space. A draft's own bytes (its intents and field
//!   text) are at most [`DRAFT_MAX_BYTES`]; a change past that is refused.
//!   The base block it draws over is charged at B2's ceiling for a block
//!   ([`cards_spec::board::BLOCK_MAX_OWNED`]). Across sessions, at most
//!   [`DESK_DRAFTS_MAX`].

// The tests-first commit's shape of the module: its types and words, its
// logic not yet written (every function a stand-in).
#![allow(dead_code, unused_imports, unused_variables)]

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
}

impl Fate {
    /// The header's words for a draft that ended (none: held).
    pub(crate) fn says(self) -> Option<String> {
        match self {
            Fate::Held => None,
            Fate::Rewritten(r) => Some(format!("the agent rewrote this block (r{r})")),
            Fate::Gone => Some("this block is gone".into()),
            Fate::Orphan => Some(ORPHAN.into()),
        }
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
    /// The steps the user added (`u-` ids), with their labels.
    pub added: Vec<(ItemId, String)>,
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
}

impl Draft {
    pub(crate) fn of(b: Arc<Block>, generation: Generation) -> Option<Self> {
        None
    }

    pub(crate) fn changed(&self) -> bool {
        false
    }

    pub(crate) fn view(&self) -> Block {
        (*self.base_block).clone()
    }

    pub(crate) fn strike(&mut self, id: &ItemId) -> Result<(), &'static str> {
        Err("not yet")
    }

    pub(crate) fn stop(&mut self, id: &ItemId) -> Result<(), &'static str> {
        Err("not yet")
    }

    pub(crate) fn shift(&mut self, id: &ItemId, by: isize) -> bool {
        false
    }

    pub(crate) fn rename(&mut self, id: &ItemId, label: &str) -> Result<(), &'static str> {
        Err("not yet")
    }

    pub(crate) fn add(&mut self, after: Option<&ItemId>, id: ItemId, label: &str) -> Result<(), &'static str> {
        Err("not yet")
    }

    pub(crate) fn note(&mut self, title: bool, s: &str) -> Result<(), &'static str> {
        Err("not yet")
    }

    pub(crate) fn hunk(&mut self, id: &ItemId, keep: bool) -> Result<(), &'static str> {
        Err("not yet")
    }

    pub(crate) fn own_bytes(&self) -> usize {
        0
    }

    pub(crate) fn bytes(&self) -> usize {
        0
    }

    pub(crate) fn check(&self, board: &State) -> Checked {
        Checked { fails: Vec::new(), message: None, off: None }
    }

    pub(crate) fn words(&self) -> String {
        String::new()
    }

    pub(crate) fn fate(&self, d: &Drawn) -> Fate {
        Fate::Held
    }

    pub(crate) fn agent_changes(&self, st: &State) -> u32 {
        0
    }
}

/// The desk's drafts (module docs).
#[derive(Debug, Default)]
pub(crate) struct Drafts {
    all: Vec<(String, Draft)>,
}

impl Drafts {
    pub(crate) fn of<'a>(&'a self, session: &'a str) -> impl Iterator<Item = &'a Draft> + 'a {
        self.all.iter().filter(move |(s, _)| s == session).map(|(_, d)| d)
    }

    pub(crate) fn count(&self, session: &str) -> usize {
        0
    }

    pub(crate) fn len(&self) -> usize {
        0
    }

    pub(crate) fn get(&self, session: &str, id: &Ref, tag: Tag) -> Option<&Draft> {
        None
    }

    pub(crate) fn get_mut(&mut self, session: &str, id: &Ref, tag: Tag) -> Option<&mut Draft> {
        None
    }

    pub(crate) fn in_row(&self, session: &str, id: &Ref) -> Option<&Draft> {
        None
    }

    pub(crate) fn take(&mut self, session: &str, b: &Arc<Block>, generation: Generation) -> Result<&mut Draft, &'static str> {
        Err("not yet")
    }

    pub(crate) fn discard(&mut self, session: &str, id: &Ref, tag: Tag) -> bool {
        false
    }

    pub(crate) fn judge(&mut self, session: &str, d: &Drawn) -> bool {
        false
    }

    pub(crate) fn gone(&mut self, session: &str) -> bool {
        false
    }

    pub(crate) fn compacted(&mut self, session: &str) -> bool {
        false
    }

    pub(crate) fn holds(&self, session: &str) -> bool {
        false
    }

    pub(crate) fn bytes(&self) -> usize {
        0
    }
}

#[cfg(test)]
#[path = "drafts_tests.rs"]
mod tests;
