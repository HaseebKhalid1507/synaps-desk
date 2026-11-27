//! The agent's sections in the right panel (synaps-cards `docs/blocks.md`
//! v1.3 §4, the D6 row of §8.2; D6a: display only). Pure: what the rail
//! draws, from the board ([`Drawn::rail`]), what this desk shows of it
//! ([`RailVisibility`], D6b's), and the user's local folds ([`Local`]).
//! Drawn in [`crate::agent_rail_view`].
//!
//! - **Two slots** (decision 1): `upper`, after Prompt cache and before
//!   Files, opens with the `AGENT` header and NOW; `end`, after Files,
//!   with a thin `AGENT` divider. A slot draws the user's sections first
//!   (pins and slot moves, in the order the user made them: D6b's, through
//!   [`RailVisibility::placed`]), then the agent's in its placement order.
//!   A group sits in one slot, with its members.
//! - **The cap: [`CAP`] expanded, across both slots** ([`layout`]). In
//!   drawing order (upper, then end): a section the user expanded takes a
//!   place first; one folded (by the user, or the agent's `collapse` that
//!   the user hasn't overridden) draws its header and one summary line and
//!   takes none; the rest expand in order while places remain. Each slot's
//!   leftovers are one `+2 more · plan, tests` row, whose click lists their
//!   headers. Nothing is ever dropped. The desk's own sections never count.
//! - **A fifth expansion** ([`Local::toggle`]): the newest click wins. An
//!   agent-expanded section holding a place gives way by the layout's own
//!   order (the last in drawing order goes to its `+N` row); if all four
//!   places are the user's, the oldest expansion (a counter on this desk,
//!   so no ties) folds back to the agent's default.
//! - **Overflow per kind**, in 20 px lines, at most [`LINES`] a section
//!   ([`lines`]): a plan's window of [`PLAN_ROWS`] steps around the first
//!   not done and `+8 more · 3 done` (the wheel moves the window); a diff's
//!   [`DIFF_LINES`] and `+N lines`; a note's [`NOTE_LINES`] and `more`;
//!   status 2, metric 3, file 1.
//! - **NOW** ([`now_of`]): the first that applies of a permission prompt
//!   waiting, a card the agent waits on, the earliest-started call still
//!   running (`+2 running · 1 queued`), writing, thinking, idle (Ctrl+J's
//!   order, [`crate::waiting`]). Its label stays on a call while it runs.
//! - **The collapse is local and in memory only** (§3.5): it is cheap to
//!   redo, and stale folds after a restart would hide news. The user's
//!   choice wins until the agent's next `collapse` op on that block.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cards_spec::block::{Generation, Slot, StepState};
use cards_spec::board::Pos;
use synaps_client::blocks::{Body, Drawn, Ref, Shown};
use synaps_client::transcript::{Item, ItemKind, ToolStatus};

/// Sections expanded at most, across both slots (§4).
pub(crate) const CAP: usize = 4;
/// A section's body lines (20 px each) at most (§4).
pub(crate) const LINES: usize = 12;
/// A plan's step rows at most; past them, one `+N more` line.
pub(crate) const PLAN_ROWS: usize = 11;
/// Steps shown above the first not done in a plan's window.
pub(crate) const PLAN_ABOVE: usize = 2;
/// A diff's lines at most; past them, one `+N lines` line.
pub(crate) const DIFF_LINES: usize = 10;
/// A note's lines at most; past them, one `more` line (the user's own
/// resize: it opens whole). With it, the note keeps to [`LINES`].
pub(crate) const NOTE_LINES: usize = 11;
/// A metric's series in the rail: its newest this many bars (quads).
pub(crate) const SPARK_MAX: usize = 32;

/// The slots in drawing order.
pub(crate) const SLOTS: [Slot; 2] = [Slot::Upper, Slot::End];

pub(crate) fn slot_ix(s: Slot) -> usize {
    match s {
        Slot::Upper => 0,
        Slot::End => 1,
    }
}

// ---------------------------------------------------------------------------
// The seam (D6b): what this desk shows of the agent's sections.

/// How this desk draws a block the board has (D6b: §3.5's dismissals,
/// shared and desired, and soft dismissals across a compaction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    /// Drawn as the board has it.
    Shown,
    /// Not drawn (dismissed): it counts in the header's `M dismissed`
    /// only through [`RailVisibility::dismissed`].
    Hidden,
    /// Drawn as a fixed one-line tombstone ([`Tomb`]): it takes no place
    /// under the cap. A group's member too: its line in the group is the
    /// tombstone's, never the member's content.
    Tombstone(Tomb),
}

/// A soft dismissal's tombstone (D6b's, §3.5): what it says (e.g.
/// `dismissed before compaction · u show`), and D6b's own token for the
/// action it offers (the visibility version it was drawn at). D6a keeps
/// the token with its tombstone through the layout to the section drawn
/// ([`Section::tombs`], [`State::Tombstone`]) for D6b's action there, and
/// in the structure (another token is a change under the lease); it never
/// makes one from the text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Tomb {
    pub text: Arc<str>,
    pub token: u64,
}

/// A section's block with its group's members' tombstones ([`layout`]).
type Trimmed = (Shown, Vec<Option<Tomb>>);

/// What this desk shows of the agent's rail sections and where: D6b's
/// dismissals, pins and slot moves (`blocks.md` §3.5) live behind it. D6a
/// reads it and nothing else of them; [`AllShown`] is everything the
/// board places, where it places it. A store for one session's
/// conversation (its board's generation), read on the UI thread as the
/// rail draws: cheap, and never blocking.
pub(crate) trait RailVisibility {
    /// How block `id` draws (top-level or a group's member: a member
    /// hidden leaves its group's list).
    fn seen(&self, _id: &Ref) -> Seen {
        Seen::Shown
    }

    /// The user's placements, pins and slot moves, in the order the user
    /// made them: each draws first in its slot, in this order, before the
    /// agent's sections. One the board doesn't hold as a live top-level
    /// block (in the rail or the transcript) is skipped; a transcript
    /// block listed here is pinned into the rail (its transcript stub is
    /// D6b's to draw).
    fn placed(&self) -> &[(Ref, Slot)] {
        &[]
    }

    /// The board's blocks this desk doesn't draw for a dismissal: the
    /// AGENT header's `M dismissed`.
    fn dismissed(&self) -> usize {
        0
    }

    /// What those dismissed are, as one tag: their ids, incarnations and
    /// visibility versions (what the footer's `show` acts on, checked
    /// whole: never a count).
    fn hidden(&self) -> u64 {
        0
    }
}

/// Everything shown, as the board places it (D6a's tests).
#[cfg(test)]
pub(crate) struct AllShown;

#[cfg(test)]
impl RailVisibility for AllShown {}

// ---------------------------------------------------------------------------
// The user's local state.

/// One user fold: open or not, the agent's `collapse` it overrides (its
/// latest when the user acted: the agent's next one wins again), and its
/// place in this desk's count of the user's folds (oldest lowest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    pub open: bool,
    pub at: Option<Pos>,
    pub n: u64,
}

/// One session's rail state on this desk, in memory only (§3.5): the
/// user's folds, the slots' `+N` lists opened, a plan's window scrolled,
/// the notes opened past their lines. Of one conversation generation: a
/// new one clears it ([`Local::of`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Local {
    generation: Option<Generation>,
    user: HashMap<Ref, Choice>,
    next: u64,
    /// A slot's `+N` row opened: its leftovers' headers listed.
    pub listed: [bool; 2],
    /// A plan's window, moved by the wheel: its first step shown.
    pub plan_top: HashMap<Ref, usize>,
    /// Notes opened past [`NOTE_LINES`].
    pub more: HashSet<Ref>,
}

impl Local {
    /// Kept for the board `d` draws: another generation (a compaction, a
    /// clear, another conversation) starts afresh, and the state of blocks
    /// it no longer holds goes (bounded by the board's 32 live blocks).
    pub(crate) fn of(&mut self, d: &Drawn, live: &HashSet<Ref>) {
        if self.generation != d.generation {
            *self = Local { generation: d.generation, ..Local::default() };
            return;
        }
        self.user.retain(|r, _| live.contains(r));
        self.plan_top.retain(|r, _| live.contains(r));
        self.more.retain(|r| live.contains(r));
    }

    /// The user's fold of `s`, if it still holds (no `collapse` op on it
    /// since).
    pub(crate) fn choice(&self, s: &Shown) -> Option<Choice> {
        self.user.get(&s.block.id).copied().filter(|c| c.at == s.block.folded_at)
    }

    /// The user's click on section `id`'s chevron, as laid out in `l`:
    /// open folds it, anything else expands it (§4). An expansion past the
    /// cap with all four places the user's folds the oldest one back to
    /// the agent's default; an agent-expanded one gives way by the layout.
    pub(crate) fn toggle(&mut self, l: &Layout, id: &Ref) {
        let Some(sec) = l.section(id) else { return };
        self.next += 1;
        let n = self.next;
        if sec.state == State::Open {
            self.user.insert(id.clone(), Choice { open: false, at: sec.s.block.folded_at, n });
            return;
        }
        self.user.insert(id.clone(), Choice { open: true, at: sec.s.block.folded_at, n });
        let mut theirs: Vec<(u64, Ref)> = l
            .sections()
            .filter(|x| &x.s.block.id != id)
            .filter_map(|x| self.choice(&x.s).filter(|c| c.open).map(|c| (c.n, x.s.block.id.clone())))
            .collect();
        if theirs.len() >= CAP {
            theirs.sort();
            self.user.remove(&theirs[0].1);
        }
    }

    /// The user's expansions held, oldest first (tests).
    #[cfg(test)]
    pub(crate) fn expanded(&self) -> Vec<Ref> {
        let mut v: Vec<(u64, Ref)> = self.user.iter().filter(|(_, c)| c.open).map(|(r, c)| (c.n, r.clone())).collect();
        v.sort();
        v.into_iter().map(|(_, r)| r).collect()
    }
}

// ---------------------------------------------------------------------------
// The layout.

/// How a section draws under the cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Expanded: its header and its body (a place taken).
    Open,
    /// Folded: its header and one summary line.
    Summary,
    /// A leftover: in its slot's `+N` row, not drawn.
    Over,
    /// A leftover its slot's `+N` lists: its header alone.
    Listed,
    /// A tombstone ([`Seen::Tombstone`]): one line.
    Tombstone(Tomb),
}

/// One section in its slot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Section {
    pub s: Shown,
    pub slot: Slot,
    /// Placed by the user (a pin or a slot move).
    pub user: bool,
    pub state: State,
    /// An expansion the user made holds its place.
    pub expanded_by_user: bool,
    /// A group's members' tombstones, in `s.members`' order (none: drawn
    /// as itself). A hidden member is in neither.
    pub tombs: Vec<Option<Tomb>>,
}

/// A slot's leftovers' row: `+2 more · plan, tests`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct More {
    pub ids: Vec<Ref>,
    pub open: bool,
}

/// What the rail draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Layout {
    /// Each slot's sections in drawing order (leftovers included).
    pub slots: [Vec<Section>; 2],
    /// Each slot's `+N` row, if it has leftovers.
    pub more: [Option<More>; 2],
    /// The AGENT header's `N sections` (drawn or in a `+N` row).
    pub sections: usize,
    /// Its `M dismissed`, and what they are ([`RailVisibility::hidden`]).
    pub dismissed: usize,
    pub hidden: u64,
}

impl Layout {
    pub(crate) fn sections(&self) -> impl Iterator<Item = &Section> {
        self.slots.iter().flatten()
    }

    pub(crate) fn section(&self, id: &Ref) -> Option<&Section> {
        self.sections().find(|s| &s.s.block.id == id)
    }

    /// The zone shows: a section placed, or one dismissed (its count).
    pub(crate) fn any(&self) -> bool {
        self.sections > 0 || self.dismissed > 0 || self.slots.iter().any(|s| !s.is_empty())
    }

    /// The places taken (tests).
    #[cfg(test)]
    pub(crate) fn open(&self) -> Vec<String> {
        self.sections().filter(|s| s.state == State::Open).map(|s| s.s.block.id.to_string()).collect()
    }
}

/// A block the board holds as a live top-level block, wherever it is
/// (the rail's slots, the transcript's rows): a user placement's.
fn top_level<'a>(d: &'a Drawn, id: &Ref) -> Option<&'a Shown> {
    d.rail.iter().flatten().chain(d.rows.values().flat_map(|r| r.blocks.iter())).find(|s| &s.block.id == id)
}

/// What the rail draws of board `d` (§4): the slots' sections in drawing
/// order, their states under the cap, the `+N` rows.
pub(crate) fn layout(d: &Drawn, vis: &dyn RailVisibility, local: &Local) -> Layout {
    let placed = vis.placed();
    let mut out = Layout { dismissed: vis.dismissed(), hidden: vis.hidden(), ..Layout::default() };
    // A member hidden leaves its group's list; one a tombstone keeps its
    // place there, as its tombstone.
    let trim = |s: &Shown| {
        let mut members = Vec::new();
        let mut tombs = Vec::new();
        for m in &s.members {
            match vis.seen(&m.block.id) {
                Seen::Hidden => continue,
                Seen::Tombstone(t) => tombs.push(Some(t)),
                Seen::Shown => tombs.push(None),
            }
            members.push(m.clone());
        }
        (Shown { members, ..s.clone() }, tombs)
    };
    for slot in SLOTS {
        let k = slot_ix(slot);
        let mut list: Vec<(Trimmed, bool)> = Vec::new();
        for (id, _) in placed.iter().filter(|(_, s)| *s == slot) {
            if let Some(s) = top_level(d, id).filter(|_| !list.iter().any(|((x, _), _)| &x.block.id == id)) {
                list.push((trim(s), true));
            }
        }
        for s in &d.rail[k] {
            if !placed.iter().any(|(id, _)| id == &s.block.id) {
                list.push((trim(s), false));
            }
        }
        for ((s, tombs), user) in list {
            let state = match vis.seen(&s.block.id) {
                Seen::Hidden => continue,
                Seen::Tombstone(t) => State::Tombstone(t),
                Seen::Shown => State::Over,
            };
            out.slots[k].push(Section { s, slot, user, state, expanded_by_user: false, tombs });
        }
    }
    // The cap, in drawing order: the user's expansions first (their
    // newest [`CAP`], should more be held), then the folded, then the
    // rest while places remain.
    let mut mine: Vec<(u64, usize, usize)> = Vec::new();
    for (k, slot) in out.slots.iter().enumerate() {
        for (j, sec) in slot.iter().enumerate() {
            if sec.state == State::Over
                && let Some(c) = local.choice(&sec.s).filter(|c| c.open)
            {
                mine.push((c.n, k, j));
            }
        }
    }
    mine.sort_by_key(|m| std::cmp::Reverse(m.0));
    mine.truncate(CAP);
    let mut places = CAP;
    for &(_, k, j) in &mine {
        out.slots[k][j].state = State::Open;
        out.slots[k][j].expanded_by_user = true;
        places -= 1;
    }
    for sec in out.slots.iter_mut().flatten().filter(|s| s.state == State::Over) {
        // The user's fold; else, and for an expansion past the newest
        // four, the agent's.
        let folded = match local.choice(&sec.s) {
            Some(c) if !c.open => true,
            _ => sec.s.block.collapsed,
        };
        if folded {
            sec.state = State::Summary;
        } else if places > 0 {
            sec.state = State::Open;
            places -= 1;
        }
    }
    for k in 0..2 {
        let ids: Vec<Ref> = out.slots[k].iter().filter(|s| s.state == State::Over).map(|s| s.s.block.id.clone()).collect();
        if !ids.is_empty() {
            if local.listed[k] {
                for s in out.slots[k].iter_mut().filter(|s| s.state == State::Over) {
                    s.state = State::Listed;
                }
            }
            out.more[k] = Some(More { ids, open: local.listed[k] });
        }
    }
    out.sections = out.sections().filter(|s| !matches!(s.state, State::Tombstone(_))).count();
    out
}

// ---------------------------------------------------------------------------
// Overflow per kind (20 px lines, at most [`LINES`]).

/// A plan's window: the first step shown, how many, and the `+N more ·
/// M done` line's counts (the steps not shown, the done among them), if
/// it has one. Around the first step not done, [`PLAN_ABOVE`] above it,
/// unless the wheel moved it (`top`).
pub(crate) fn plan_window(steps: &[cards_spec::block::Step], top: Option<usize>) -> (usize, usize, Option<(usize, usize)>) {
    let n = steps.len();
    if n <= LINES {
        return (0, n, None);
    }
    let first = steps.iter().position(|s| !matches!(s.state, StepState::Done | StepState::Skipped)).unwrap_or(n - 1);
    let start = top.unwrap_or(first.saturating_sub(PLAN_ABOVE)).min(n - PLAN_ROWS);
    let done = steps.iter().enumerate().filter(|(i, s)| (*i < start || *i >= start + PLAN_ROWS) && s.state == StepState::Done).count();
    (start, PLAN_ROWS, Some((n - PLAN_ROWS, done)))
}

/// A diff's lines and whether it has a foot (`PROPOSED · not applied`, an
/// edit preview's `as requested · not read from disk`): each hunk's `@@`
/// line and its lines, as the transcript draws them
/// ([`AppView::diff_rows`](crate::app::AppView::diff_rows)); an edit
/// preview's are its session's edits (`drawn`, the renderer's).
pub(crate) fn diff_lines(d: &cards_spec::block::DiffBody, drawn: Option<(usize, bool)>) -> (usize, bool) {
    drawn.unwrap_or_else(|| (d.hunks.iter().map(|h| h.lines.len() + 1).sum::<usize>(), d.state == cards_spec::block::DiffState::Proposed))
}

/// A diff's rail lines: at most [`DIFF_LINES`] of its own, `+N lines`
/// past them, and its foot.
pub(crate) fn diff_shown(n: usize, foot: bool) -> (usize, bool, usize) {
    let shown = n.clamp(1, DIFF_LINES);
    (shown, n > DIFF_LINES, shown + usize::from(n > DIFF_LINES) + usize::from(foot))
}

/// A note's text's lines as laid out in the rail (the window's text
/// system, at the rail's face, size and width: what the renderer draws).
pub(crate) type NoteLines<'a> = &'a dyn Fn(&str) -> usize;

/// A note of `n` lines as laid out: the lines it shows, and whether it is
/// cut at [`NOTE_LINES`] (`more`), unless opened.
pub(crate) fn note_shown(n: usize, open: bool) -> (usize, bool) {
    if n > NOTE_LINES && !open { (NOTE_LINES, true) } else { (n, false) }
}

/// The body lines a section draws in its state (20 px each): what its
/// height follows, the renderer's and the lease's (§6.2: structure and
/// bounded line counts). `diff`: a diff's lines and foot as drawn (an edit
/// preview's come from its session's edits); `note`: a note's lines as
/// laid out.
pub(crate) fn lines(s: &Shown, state: &State, local: &Local, note: NoteLines, diff: Option<(usize, bool)>) -> usize {
    match state {
        State::Open => {}
        State::Summary => return 1,
        State::Over | State::Listed | State::Tombstone(_) => return 0,
    }
    match &s.block.body {
        Body::Plan(p) => {
            let (_, rows, more) = plan_window(&p.steps, local.plan_top.get(&s.block.id).copied());
            rows + usize::from(more.is_some())
        }
        Body::Status(_) => 2,
        Body::Metric(m) => 2 + usize::from(!m.series.is_empty()),
        Body::File(_) => 1,
        Body::Diff(d) => {
            let (n, foot) = diff_lines(d, diff);
            diff_shown(n, foot).2
        }
        Body::Note(n) => {
            let (k, cut) = note_shown(note(&n.text), local.more.contains(&s.block.id));
            k + usize::from(cut)
        }
        Body::Group(_) => s.members.len().max(1),
    }
}

/// A section's one-line summary, folded (§4): a plan `3/5 done`, a metric
/// `1.0 · was 2.0`, a status its text; a diff its file and counts, a note
/// its first line, a file its path, a group its count.
pub(crate) fn summary(s: &Shown) -> String {
    match &s.block.body {
        Body::Plan(p) => {
            let done = p.steps.iter().filter(|s| s.state == StepState::Done).count();
            format!("{done}/{} done", p.steps.len())
        }
        Body::Metric(m) => {
            let unit = m.unit.as_deref().map(|u| format!(" {u}")).unwrap_or_default();
            match &m.was {
                Some(w) => format!("{}{unit} · was {w}", m.value),
                None => format!("{}{unit}", m.value),
            }
        }
        Body::Status(st) => crate::ask::shown(&st.text, cards_spec::Lines::Single),
        Body::Diff(d) => {
            let (add, del) = d.hunks.iter().flat_map(|h| &h.lines).fold((0, 0), |(a, r), l| match l.as_bytes().first() {
                Some(b'+') => (a + 1, r),
                Some(b'-') => (a, r + 1),
                _ => (a, r),
            });
            let file = crate::turns::tilde(&d.file, false);
            if d.from.is_some() { format!("{file} · edit preview") } else { format!("{file} · +{add} −{del}") }
        }
        Body::Note(n) => crate::ask::shown(n.text.lines().find(|l| !l.trim().is_empty()).unwrap_or(""), cards_spec::Lines::Single),
        Body::File(f) => match f.line {
            Some(l) => format!("{}:{l}", crate::turns::tilde(&f.path, false)),
            None => crate::turns::tilde(&f.path, false),
        },
        Body::Group(_) => match s.members.len() {
            1 => "1 block".to_string(),
            n => format!("{n} blocks"),
        },
    }
}

/// The `+N` row's text: `+2 more · plan, tests`.
pub(crate) fn more_text(m: &More) -> String {
    let names: Vec<String> = m.ids.iter().map(|r| r.to_string()).collect();
    format!("+{} more · {}", m.ids.len(), names.join(", "))
}

fn hash(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// The rail's structure as drawn (§6.2, §4): each slot's sections, their
/// incarnations, states and heights, tombstones whole (a section's own
/// and a group's members': text and token), the `+N` rows. Under the rail's lease, a board that changes
/// it waits: a cap change, a section added, removed or reordered, one
/// replaced or changing height. One that keeps it (a tick, a label) draws
/// in place. A note open and whole is as tall as its text lays out: its
/// text then (D5's rule, `lease::shape`: the safe side, never an estimate
/// of its wrap); cut, it is [`NOTE_LINES`] tall whatever its text.
pub(crate) fn shape(l: &Layout, local: &Local, note: NoteLines, diff: &dyn Fn(&Shown) -> Option<(usize, bool)>) -> u64 {
    let mut parts = Vec::new();
    for (k, slot) in l.slots.iter().enumerate() {
        for sec in slot {
            let st = match &sec.state {
                State::Open => 0u8,
                State::Summary => 1,
                State::Over => 2,
                State::Listed => 3,
                State::Tombstone(_) => 4,
            };
            // A tombstone whole, its own or a member's: its text and D6b's
            // token (another token is another structure, Shady #104 R2-3).
            let tomb = match &sec.state {
                State::Tombstone(t) => Some(t),
                _ => None,
            };
            let members: Vec<u64> =
                sec.s.members.iter().zip(&sec.tombs).map(|(m, t)| hash((m.block.id.to_string(), m.block.tag, t))).collect();
            let text = match &sec.s.block.body {
                Body::Note(n) if sec.state == State::Open && !note_shown(note(&n.text), local.more.contains(&sec.s.block.id)).1 => {
                    Some(&n.text)
                }
                _ => None,
            };
            parts.push(hash((
                k,
                sec.s.block.id.to_string(),
                sec.s.block.tag,
                st,
                lines(&sec.s, &sec.state, local, note, diff(&sec.s)),
                tomb,
                members,
                text,
            )));
        }
        parts.push(hash(l.more[k].as_ref().map(|m| (m.ids.iter().map(|r| r.to_string()).collect::<Vec<_>>(), m.open))));
    }
    // The footer (D6b's `N dismissed · show`): there or not, a line of the
    // upper slot's.
    parts.push(hash(l.dismissed > 0));
    hash(parts)
}

// ---------------------------------------------------------------------------
// NOW.

/// What NOW says (§4), the first that applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Now {
    /// A permission prompt waits: `waiting for you · allow bash`.
    Prompt(String),
    /// An open card the agent waits on.
    Card,
    /// The earliest-started call still running: its verb and target, the
    /// other calls running and those queued.
    Running {
        verb: String,
        target: String,
        more: usize,
        queued: usize,
    },
    Writing,
    Thinking,
    Idle,
}

impl Now {
    /// Its line. What came from the agent or a tool (a call's verb and
    /// target, what a prompt asks to allow) goes through the desk's
    /// sanitizer ([`crate::ask::shown`], one line: no control, bidi or
    /// invisible character, no line break); its own words and counts are
    /// put around it after.
    pub(crate) fn text(&self) -> String {
        let clean = |s: &str| crate::ask::shown(s, cards_spec::Lines::Single);
        match self {
            Now::Prompt(what) => format!("waiting for you · {}", clean(what)),
            Now::Card => "waiting for your answer".to_string(),
            Now::Running { verb, target, more, queued } => {
                let (verb, target) = (clean(verb), clean(target));
                let mut s = if target.is_empty() { verb } else { format!("{verb} {target}") };
                let tail: Vec<String> = [(*more, "running"), (*queued, "queued")]
                    .iter()
                    .filter(|(n, _)| *n > 0)
                    .map(|(n, w)| if *w == "running" { format!("+{n} {w}") } else { format!("{n} {w}") })
                    .collect();
                if !tail.is_empty() {
                    s.push_str("  ");
                    s.push_str(&tail.join(" · "));
                }
                s
            }
            Now::Writing => "writing".to_string(),
            Now::Thinking => "thinking".to_string(),
            Now::Idle => "idle".to_string(),
        }
    }
}

/// A call's verb and target as its tool row says them.
pub(crate) type Call<'a> = &'a dyn Fn(&synaps_client::transcript::ToolCall) -> (String, String);

/// NOW for a session (§4): `prompt`, what a permission prompt waiting asks
/// to allow; `card`, a card the agent waits on is open; `busy`, a turn
/// runs; `items`, its rows (the live turn's calls and what streams); `call`,
/// a call's verb and target.
pub(crate) fn now_of(prompt: Option<String>, card: bool, busy: bool, items: &[Item], call: Call) -> Now {
    if let Some(p) = prompt {
        return Now::Prompt(p);
    }
    if card {
        return Now::Card;
    }
    if !busy {
        return Now::Idle;
    }
    let tools = || items.iter().filter_map(|i| i.tool.as_ref());
    // Running: heard start (`ToolStarted`; a call's input whole is not yet
    // a start). None heard (a daemon that doesn't say, a replay): the rows
    // still running, in row order.
    let mut running: Vec<_> = tools().filter(|t| t.status == Some(ToolStatus::Running) && t.started > 0).collect();
    if running.is_empty() {
        running = tools().filter(|t| t.status == Some(ToolStatus::Running)).collect();
    }
    // The earliest-started still running: its label stays on it while it
    // runs, whatever starts or ends beside it.
    if let Some(first) = running.iter().min_by_key(|t| (t.started == 0, t.started)) {
        let queued = tools().filter(|t| t.status == Some(ToolStatus::Queued)).count();
        let (verb, target) = call(first);
        return Now::Running { verb, target, more: running.len() - 1, queued };
    }
    match items.iter().rev().find(|i| i.streaming).map(|i| (i.kind, i.tool.as_ref().and_then(|t| t.status))) {
        Some((ItemKind::Assistant, _)) | Some((ItemKind::Tool, Some(ToolStatus::Streaming))) => Now::Writing,
        _ => Now::Thinking,
    }
}

#[cfg(test)]
mod tests;
