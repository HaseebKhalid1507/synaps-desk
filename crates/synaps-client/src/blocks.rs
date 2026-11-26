//! Blocks in a transcript (synaps-cards `docs/blocks.md` v1.3, §1.7, §1.8,
//! §5.1): the board's input, read from the history and from the live
//! turn's protocol events as the history will hold them, and what each row
//! draws of the board. Read-only: nothing here sends (D5).
//!
//! **The input** is one canonical sequence of `cards_spec::board::Event`s
//! (the board docs' adapter guarantee 1), whichever way it came:
//! - **the history**, a message at a time as the transcript folds it
//!   ([`HistoryBlocks`]), read as [`cards_spec::fold::events`] reads a
//!   whole history: every call, a card or block call's input whole, the
//!   results of card and block calls, `ResultsEnd` where a message after an
//!   assistant message ends its results, the user's own text. A result is
//!   its call's in the message right after the call's (§1.7: the
//!   provider's pairing, which the engine keeps through a failure or a
//!   cancel), so the reader keeps the last assistant message's tool ids
//!   only, never every id of the history. A result naming an older call (a
//!   history that broke the pairing) is left out where `fold::events`
//!   keeps it; its batch has committed, so it binds nothing either way.
//! - **the live turn** ([`LiveLog`], [`live`]) after it: the protocol's
//!   facts as they came, never the rows that show them (a row's status and
//!   result are display: a preview, a streamed delta, a cancel's). A
//!   round's calls once their input is whole (`ToolUse`), in model order;
//!   then the final results (`ToolResult`, never `ToolResultDelta`) of the
//!   round's calls, a call's first only, in the order they came; then
//!   `ResultsEnd` once every call of the round has its result, or once a
//!   later message started; a prompt (`TurnStarted`) as the user message
//!   the history will hold. A result whose
//!   round has ended is left out: the history's results message for that
//!   round is written, and nothing moves back across a commit. A history
//!   that ends with an assistant message (an attach mid-turn) has its
//!   results come live: they are that message's.
//!
//! **One journal.** The history's events are staged in the transcript
//! ([`HistoryBlocks`]) only until the worker of its epoch has taken them,
//! then let go: the worker's journal ([`Worker::journal`]) is the one copy
//! the board reads (B2 reads one contiguous history). A re-fold shares it,
//! copy-on-write: a job that reads on while a re-fold holds it copies it
//! once. A worker started over on a history whose staged events were let
//! go (its store torn down, the transcript kept) has a gap
//! ([`Worker::gap`]): it draws nothing new until the history is folded
//! again (another epoch; an attach does).
//!
//! **The work** ([`Worker::run`]) is the board's: the history's fold, read
//! on from where it stopped while the history only grows (a new epoch
//! starts over), then the live overlay and the bound prefix on a clone. It
//! runs off the UI frame (the desk schedules it, guarantees 6 and 7): its
//! staged events taken and read a slice of at most [`SLICE`] events at a
//! time, `go_on` asked between slices. B2's `read`, `view` and `bound`
//! can't be preempted, and the overlay and its projection run after the
//! last slice without a check. It publishes only a finished board, one
//! [`Drawn`] for one input.
//!
//! **Fates** (§1.6, §1.7): a block call's ops apply if its batch's first
//! result for its id binds it. The worker keeps the open batch's start and
//! one flag, nothing per call: a batch's fates are read from the journal
//! when it commits, and the open batch's at each overlay, in passes of at
//! most [`PASS`] calls, each reading the rest of the batch once through an
//! index borrowing their ids (as B2's replay). A committed call's fate is
//! published once ([`Drawn::committed`]) and kept by its row from then on
//! (a byte of the row the transcript has anyway); the worker lets it go
//! once the rows took it. So refused calls grow nothing but the journal
//! and their rows.
//!
//! **What rows draw** ([`Drawn::rows`], by row id, bounded by the board and
//! the open batch): the open batch's and the live turn's calls' fates (a
//! bound or pending call draws no tool row; an unbound one is a plain
//! row), the blocks anchored at a call's row (§1.2: a put's, a move's; a
//! converted card's at its card's row), each with its bound board's
//! incarnation (its glow waits for the bind, §6.1), and a run's title
//! (`collapse` with `run`, §1.3).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use cards_spec::block::{self, Receipt};
use cards_spec::board::{Board, Event, Place, State};
use cards_spec::fold::{self, NAMES as CARD_NAMES};
use serde_json::Value;

pub use cards_spec::block::{BlockId, Body, Kind, Ref, Tag};
pub use cards_spec::board::Block;

use crate::transcript::{Item, ItemKind};

/// Events read between two asks whether to go on (§1.8: a yield every
/// 50,000 events or 30 ms; a slice this small is far under both).
pub const SLICE: usize = 4_096;

/// Block calls whose fates one pass reads: B2's replay pass
/// ([`cards_spec::board::REPLAY_IDS`]).
pub const PASS: usize = cards_spec::board::REPLAY_IDS;

/// Whether `name` is a block call's (`cards_block`; a collision suffix is
/// another tool).
pub fn is_block_tool(name: &str) -> bool {
    block::NAMES.contains(&name)
}

/// Whether a call's input is kept whole ([`Item::kept_input`]): a card's
/// or a block's (≤ 8 KiB, §5.1), never another tool's.
pub fn keeps_input(name: &str) -> bool {
    CARD_NAMES.contains(&name) || is_block_tool(name)
}

/// A fresh history's epoch: every history folded from its first message is
/// another, so a worker never reads one history's events on another's.
fn next_epoch() -> u64 {
    static EPOCH: AtomicU64 = AtomicU64::new(1);
    EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// What a history's worker and its rows share ([`HistoryBlocks`]): how many
/// of its events the worker holds, and how many of its committed calls'
/// fates the rows took ([`Drawn::committed_to`]).
#[derive(Debug, Default)]
struct Shared {
    taken: AtomicUsize,
    took: AtomicU64,
}

/// The history's events, read message by message ([`HistoryBlocks::message`]),
/// staged until its worker takes them (module docs), and what the next
/// message needs of the last one.
#[derive(Clone, Debug)]
pub struct HistoryBlocks {
    epoch: u64,
    generation: Option<block::Generation>,
    /// Chunks its worker hasn't taken yet: its events from `from` on, a
    /// chunk per message that gave any.
    staged: VecDeque<Chunk>,
    from: usize,
    len: usize,
    shared: Arc<Shared>,
    /// The last message, if an assistant's: its calls' tool ids (an id's
    /// latest call: whether it keeps its result), and how many calls.
    open: Option<(HashMap<String, bool>, usize)>,
    /// A block call read: else there is no board to draw.
    blocks: bool,
}

/// A message's events and their rows.
#[derive(Clone, Debug)]
struct Chunk {
    events: Arc<[Event]>,
    rows: Arc<[Option<u64>]>,
}

impl Default for HistoryBlocks {
    fn default() -> Self {
        HistoryBlocks {
            epoch: next_epoch(),
            generation: None,
            staged: VecDeque::new(),
            from: 0,
            len: 0,
            shared: Arc::default(),
            open: None,
            blocks: false,
        }
    }
}

impl HistoryBlocks {
    /// Its epoch: another for every history folded from its first message.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// A block call in the history.
    pub fn has_blocks(&self) -> bool {
        self.blocks
    }

    /// Events read.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Events staged: read, and not taken by its worker yet.
    pub fn staged(&self) -> usize {
        self.len - self.from
    }

    /// The staged events, in order (tests: every event, if no worker took
    /// any).
    pub fn events(&self) -> Vec<Event> {
        self.staged.iter().flat_map(|c| c.events.iter().cloned()).collect()
    }

    /// The rows took the committed calls' fates up to `to`
    /// ([`Drawn::committed_to`]): its worker lets them go.
    pub fn took(&self, to: u64) {
        self.shared.took.fetch_max(to, Ordering::AcqRel);
    }

    /// Let go of the chunks its worker took.
    pub fn prune(&mut self) {
        let taken = self.shared.taken.load(Ordering::Acquire);
        while let Some(n) = self.staged.front().map(|c| c.events.len()).filter(|n| self.from + n <= taken) {
            self.from += n;
            self.staged.pop_front();
        }
    }

    /// History message `m`, its rows `rows` (the rows it pushed, in order).
    pub fn message(&mut self, m: &Value, rows: &[Item]) {
        if self.generation.is_none() {
            self.generation = Some(block::Generation::of(m));
        }
        let role = m["role"].as_str();
        let open = self.open.take();
        let ends = open.is_some();
        let blocks = m["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        let mut events = Vec::new();
        let mut at = Vec::new();
        let mut push = |e: Event, row: Option<u64>| {
            events.push(e);
            at.push(row);
        };
        match role {
            Some("assistant") => {
                if ends {
                    push(Event::ResultsEnd, None);
                }
                let mut ids = HashMap::new();
                let mut n = 0;
                let mut tools = rows.iter().filter(|i| i.kind == ItemKind::Tool);
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    let Some(id) = b["id"].as_str() else { continue };
                    let name = b["name"].as_str().unwrap_or_default();
                    let kept = keeps_input(name);
                    self.blocks |= is_block_tool(name);
                    ids.insert(id.to_string(), kept);
                    n += 1;
                    let input = if kept { b["input"].clone() } else { Value::Null };
                    let row = tools.by_ref().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == id)).map(|i| i.id);
                    push(Event::Call { tool_id: id.into(), name: name.into(), input }, row);
                }
                self.open = Some((ids, n));
            }
            Some("user") => {
                if let Some((ids, _)) = &open {
                    for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                        if let Some(id) = b["tool_use_id"].as_str().filter(|id| ids.get(*id) == Some(&true))
                            && let Some(text) = crate::cards::result_text(&b["content"])
                        {
                            push(Event::Result { tool_id: id.into(), text }, None);
                        }
                    }
                }
                if ends {
                    push(Event::ResultsEnd, None);
                }
                // Its text, if the host's rule leaves it one string.
                for e in fold::events(std::slice::from_ref(m)).into_iter().filter(|e| matches!(e, Event::User { .. })) {
                    push(e, None);
                }
            }
            _ => {
                if ends {
                    push(Event::ResultsEnd, None);
                }
            }
        }
        if !events.is_empty() {
            self.len += events.len();
            self.staged.push_back(Chunk { events: events.into(), rows: at.into() });
        }
        self.prune();
    }
}

/// The live turn as the protocol told it (module docs): kept by the
/// transcript from its events since the history's last fold, read by
/// [`live`]. Never a row's status or text.
#[derive(Clone, Debug, Default)]
pub struct LiveLog {
    /// Results of the history's last message's calls (it is an
    /// assistant's): each id's first; a kept call's with its text, in the
    /// order they came.
    head: Vec<(String, String)>,
    head_answered: HashSet<String>,
    steps: Vec<Step>,
    /// A block call among its calls.
    blocks: bool,
}

#[derive(Clone, Debug)]
enum Step {
    /// A prompt or a steer: a user message of its text.
    User { text: String, row: Option<u64> },
    /// A response: an assistant message, once it gave anything.
    Round(Round),
}

#[derive(Clone, Debug, Default)]
struct Round {
    /// Text, thinking or a call came.
    content: bool,
    /// Its calls, in model order (as they started).
    calls: Vec<LiveCall>,
    /// Each tool id's latest call.
    ids: HashMap<String, usize>,
    /// Kept calls' results (the call, its text), in the order they came.
    results: Vec<(usize, String)>,
}

#[derive(Clone, Debug)]
struct LiveCall {
    row: u64,
    tool_id: String,
    name: String,
    /// A card's or block's input, whole ([`keeps_input`]).
    input: Option<Arc<Value>>,
    /// Its input is whole (`ToolUse`).
    whole: bool,
    /// Its final result came.
    answered: bool,
}

impl LiveLog {
    /// A block call among its calls.
    pub fn has_blocks(&self) -> bool {
        self.blocks
    }

    /// Calls in it (tests: what it keeps).
    pub fn calls(&self) -> usize {
        self.steps.iter().map(|s| if let Step::Round(r) = s { r.calls.len() } else { 0 }).sum()
    }

    /// A prompt (`TurnStarted`) or a delivered steer (`SteeringDelivered`):
    /// a user message of `text`, its row's.
    pub fn user(&mut self, text: &str, row: Option<u64>) {
        self.steps.push(Step::User { text: text.to_string(), row });
    }

    /// A response starts: the next assistant message.
    pub fn response(&mut self) {
        if !matches!(self.steps.last(), Some(Step::Round(r)) if !r.content) {
            self.steps.push(Step::Round(Round::default()));
        }
    }

    /// The response streams again from its start: what it gave goes.
    pub fn reset(&mut self) {
        if let Some(Step::Round(r)) = self.steps.last_mut() {
            *r = Round::default();
        }
    }

    fn round(&mut self) -> &mut Round {
        if !matches!(self.steps.last(), Some(Step::Round(_))) {
            self.steps.push(Step::Round(Round::default()));
        }
        match self.steps.last_mut() {
            Some(Step::Round(r)) => r,
            _ => unreachable!("a round was pushed"),
        }
    }

    /// The response's text or thinking.
    pub fn content(&mut self) {
        self.round().content = true;
    }

    /// Call `tool_id` (row `row`) starts: its input streams.
    pub fn started(&mut self, row: u64, tool_id: &str, name: &str) {
        self.blocks |= is_block_tool(name);
        let r = self.round();
        r.content = true;
        if !r.calls.iter().rev().any(|c| c.row == row) {
            r.ids.insert(tool_id.to_string(), r.calls.len());
            r.calls.push(LiveCall { row, tool_id: tool_id.into(), name: name.into(), input: None, whole: false, answered: false });
        }
    }

    /// Call `tool_id`'s input is whole (`input`: kept, a card's or a
    /// block's). Again for the same row: its input is this one.
    pub fn call(&mut self, row: u64, tool_id: &str, name: &str, input: Option<Arc<Value>>) {
        self.blocks |= is_block_tool(name);
        let r = self.round();
        r.content = true;
        match r.calls.iter().rposition(|c| c.row == row) {
            Some(k) => {
                let c = &mut r.calls[k];
                if c.tool_id != tool_id {
                    c.tool_id = tool_id.to_string();
                    r.ids.insert(tool_id.to_string(), k);
                }
                let c = &mut r.calls[k];
                c.name = name.to_string();
                c.input = input;
                c.whole = true;
            }
            None => {
                r.ids.insert(tool_id.to_string(), r.calls.len());
                r.calls.push(LiveCall { row, tool_id: tool_id.into(), name: name.into(), input, whole: true, answered: false });
            }
        }
    }

    /// Call `tool_id`'s final result. It is the open batch's (the last
    /// message, if an assistant's: a live round, else `history`'s last
    /// message) latest call of that id; one that has its result keeps it,
    /// and a result whose batch has ended is left out (module docs).
    pub fn result(&mut self, history: &HistoryBlocks, tool_id: &str, text: &str) {
        match self.steps.iter_mut().rev().find(|s| !matches!(s, Step::Round(r) if !r.content)) {
            Some(Step::Round(r)) => {
                let Some(&k) = r.ids.get(tool_id) else { return };
                let c = &mut r.calls[k];
                if std::mem::replace(&mut c.answered, true) {
                    return;
                }
                if keeps_input(&c.name) {
                    r.results.push((k, text.to_string()));
                }
            }
            Some(Step::User { .. }) => {}
            None => {
                let Some(&kept) = history.open.as_ref().and_then(|(ids, _)| ids.get(tool_id)) else { return };
                if self.head_answered.insert(tool_id.to_string()) && kept {
                    self.head.push((tool_id.to_string(), text.to_string()));
                }
            }
        }
    }
}

/// The live turn's events and rows ([`live`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub events: Vec<Event>,
    pub rows: Vec<Option<u64>>,
    /// A block call among them.
    pub blocks: bool,
}

/// The live turn (`log`) as the history will hold it, after `history`
/// (module docs).
pub fn live(history: &HistoryBlocks, log: &LiveLog) -> Live {
    let mut out = Live { blocks: log.blocks, ..Live::default() };
    let mut after = history.open.is_some();
    // The history's last message's results, come live.
    if let Some((_, n)) = &history.open {
        for (id, text) in &log.head {
            push(&mut out, Event::Result { tool_id: id.clone(), text: text.clone() }, None);
        }
        if *n > 0 && log.head_answered.len() == *n {
            push(&mut out, Event::ResultsEnd, None);
            after = false;
        }
    }
    for s in &log.steps {
        match s {
            Step::User { text, row } => {
                if std::mem::take(&mut after) {
                    push(&mut out, Event::ResultsEnd, None);
                }
                push(&mut out, Event::User { text: text.clone() }, *row);
            }
            Step::Round(r) if !r.content => {}
            Step::Round(r) => {
                if after {
                    push(&mut out, Event::ResultsEnd, None);
                }
                after = true;
                for c in r.calls.iter().filter(|c| c.whole) {
                    let input = c.input.as_deref().cloned().unwrap_or(Value::Null);
                    push(&mut out, Event::Call { tool_id: c.tool_id.clone(), name: c.name.clone(), input }, Some(c.row));
                }
                for (k, text) in &r.results {
                    push(&mut out, Event::Result { tool_id: r.calls[*k].tool_id.clone(), text: text.clone() }, None);
                }
                // Final once every call has its result.
                if !r.calls.is_empty() && r.calls.iter().all(|c| c.whole && c.answered) {
                    push(&mut out, Event::ResultsEnd, None);
                    after = false;
                }
            }
        }
    }
    out
}

fn push(out: &mut Live, e: Event, row: Option<u64>) {
    out.events.push(e);
    out.rows.push(row);
}

/// One board job's input: the history's staged events (its chunks,
/// shared), then the live turn's, read as one history.
#[derive(Clone, Debug)]
pub struct Input {
    pub epoch: u64,
    pub generation: block::Generation,
    /// The staged chunks: the history's events from `from` on.
    chunks: Vec<Chunk>,
    pub from: usize,
    pub history_len: usize,
    shared: Arc<Shared>,
    pub live: Arc<Live>,
}

impl Input {
    /// `history` and `live` as one job's input; `None` with no block call
    /// in either (no board to draw).
    pub fn of(history: &HistoryBlocks, live: Arc<Live>) -> Option<Self> {
        if !history.blocks && !live.blocks {
            return None;
        }
        // The generation is the history's first message's (§1.2). With no
        // history yet, the live prompt's, as the history will hold it (a
        // string): folded again once the history lands, as any live turn.
        let first = || {
            live.events.iter().find_map(|e| match e {
                Event::User { text } => Some(block::Generation::of(&serde_json::json!({"role": "user", "content": text}))),
                _ => None,
            })
        };
        Some(Input {
            epoch: history.epoch,
            generation: history.generation.or_else(first).unwrap_or_else(|| block::Generation::of(&Value::Null)),
            chunks: history.staged.iter().cloned().collect(),
            from: history.from,
            history_len: history.len,
            shared: history.shared.clone(),
            live,
        })
    }

    /// Its events: the staged history's (all of it, if no worker took any),
    /// then the live turn's (tests).
    pub fn events(&self) -> Vec<Event> {
        self.chunks.iter().flat_map(|c| c.events.iter().cloned()).chain(self.live.events.iter().cloned()).collect()
    }
}

/// A block call's fate (§1.6, §1.7): its ops apply (`Bound`), apply
/// provisionally (`Pending`, its result still to come), or never do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fate {
    Bound,
    Pending,
    Unbound,
}

/// The open batch (§1.7, the board's batching): where it starts, and
/// whether a result was read in it (a call after one starts the next).
/// Nothing per call: a batch's fates are read from the journal ([`fates`]).
#[derive(Clone, Copy, Debug, Default)]
struct Batch {
    start: usize,
    results: bool,
}

impl Batch {
    /// Event `at` read: the batch it commits, if any (`[start, end)`).
    fn read(&mut self, events: &[Event], at: usize) -> Option<(usize, usize)> {
        let (commits, next) = match &events[at] {
            Event::Call { .. } => (self.results, at),
            Event::Result { .. } => (false, self.start),
            Event::ResultsEnd | Event::User { .. } => (true, at + 1),
        };
        let ended = commits.then_some((self.start, at));
        if commits {
            *self = Batch { start: next, results: false };
        }
        self.results |= matches!(events[at], Event::Result { .. });
        ended
    }
}

/// The fates of the block calls of one batch, `events[start..end]` (§1.7):
/// its first result for a call's id binds it, or not; a later call of its
/// id (any tool's) takes the id over, and it never binds; with no result,
/// pending while the batch is open (`open`), else unbound for good. In
/// passes of at most [`PASS`] calls, each reading the rest of the batch
/// once, its index borrowing their ids.
fn fates(events: &[Event], start: usize, end: usize, open: bool, mut each: impl FnMut(usize, Fate)) {
    let mut next = Some(start);
    while let Some(from) = next.take() {
        let mut calls: Vec<(usize, Option<usize>, bool)> = Vec::new();
        let mut ids: HashMap<&str, usize> = HashMap::new();
        for (at, e) in events.iter().enumerate().take(end).skip(from) {
            match e {
                Event::Call { tool_id, name, .. } => {
                    if let Some(k) = ids.remove(tool_id.as_str()) {
                        calls[k].2 = true;
                    }
                    if is_block_tool(name) {
                        if calls.len() < PASS {
                            ids.insert(tool_id, calls.len());
                            calls.push((at, None, false));
                        } else if next.is_none() {
                            next = Some(at);
                        }
                    }
                }
                Event::Result { tool_id, .. } => {
                    if let Some(&k) = ids.get(tool_id.as_str())
                        && calls[k].1.is_none()
                    {
                        calls[k].1 = Some(at);
                    }
                }
                Event::ResultsEnd | Event::User { .. } => {}
            }
        }
        for (pos, answer, taken) in calls {
            let fate = match (answer, taken) {
                (_, true) => Fate::Unbound,
                (None, false) if open => Fate::Pending,
                (None, false) => Fate::Unbound,
                (Some(r), false) => match (&events[pos], &events[r]) {
                    (Event::Call { input, .. }, Event::Result { text, .. }) => {
                        let binds = block::parse_call(input).ok().is_some_and(|b| Receipt::parse(text).is_some_and(|r| r.binds(&b)));
                        if binds { Fate::Bound } else { Fate::Unbound }
                    }
                    _ => Fate::Unbound,
                },
            };
            each(pos, fate);
        }
    }
}

/// What one row draws of the board.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowBlocks {
    /// A block call's: its fate. A bound or pending call draws no tool row.
    pub call: Option<Fate>,
    /// The blocks anchored here, in their list's order.
    pub blocks: Vec<Shown>,
    /// The run this call sits in is folded under this title (`collapse`
    /// with `run`, §1.3).
    pub run: Option<Arc<str>>,
}

impl RowBlocks {
    /// A call whose ops apply, or will: no tool row of its own.
    pub fn silent(&self) -> bool {
        matches!(self.call, Some(Fate::Bound | Fate::Pending))
    }

    /// A committed call's row with nothing else drawn on it: one per
    /// fate, shared by every such row.
    pub fn committed(fate: Fate) -> Arc<RowBlocks> {
        static ROWS: LazyLock<[Arc<RowBlocks>; 2]> =
            LazyLock::new(|| [Fate::Bound, Fate::Unbound].map(|f| Arc::new(RowBlocks { call: Some(f), ..RowBlocks::default() })));
        ROWS[usize::from(fate != Fate::Bound)].clone()
    }

    /// What row `id` draws: `drawn`'s, and its call's committed fate
    /// ([`Drawn::committed`]).
    pub fn of(drawn: Option<&Drawn>, id: u64, fate: Option<Fate>) -> Option<Arc<RowBlocks>> {
        match (drawn.and_then(|d| d.rows.get(&id)), fate) {
            (Some(r), Some(f)) if r.call.is_none() => Some(Arc::new(RowBlocks { call: Some(f), ..(**r).clone() })),
            (Some(r), _) => Some(r.clone()),
            (None, Some(f)) => Some(Self::committed(f)),
            (None, None) => None,
        }
    }
}

/// A block as drawn: the overlay's (§1.7), and the bound board's of the
/// same incarnation (`None`: created by a pending op); a group's members.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub block: Arc<Block>,
    pub bound: Option<Arc<Block>>,
    pub members: Vec<Shown>,
}

/// One finished board ([`Worker::run`]): what the rows draw of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    /// The input it was folded from: its history, the history's events,
    /// and all its events (the live turn's after them).
    pub epoch: u64,
    pub history: usize,
    pub events: usize,
    /// What rows draw of it, by row id (module docs): the live turn's and
    /// the open batch's calls' fates, blocks, runs' titles.
    pub rows: HashMap<u64, Arc<RowBlocks>>,
    /// The history's block calls that committed since the rows last took
    /// them, their rows and fates: each row keeps its fate once taken
    /// ([`HistoryBlocks::took`] with `committed_to`, how many so far).
    pub committed: Arc<[(u64, Fate)]>,
    pub committed_to: u64,
    /// Live blocks on the board, groups and members included.
    pub live: usize,
}

/// What a [`Worker`] keeps, counted: the journal's events (and a row
/// each), the committed calls' fates the rows haven't taken yet, and the
/// board's owned bytes (B2's working set, its card fold apart).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retained {
    pub journal: usize,
    pub rows: usize,
    pub committed: usize,
    pub board_bytes: usize,
}

/// The board's work for one session, kept across jobs (module docs).
#[derive(Debug)]
pub struct Worker {
    epoch: u64,
    /// The history's events, the one copy, and each event's row (a
    /// call's: its tool row).
    journal: Arc<Vec<Event>>,
    rows: Vec<Option<u64>>,
    board: Board,
    batch: Batch,
    /// Committed calls' rows and fates the rows haven't taken, and how
    /// many before them they took.
    committed: VecDeque<(u64, Fate)>,
    committed_from: u64,
    gap: bool,
}

impl Default for Worker {
    fn default() -> Self {
        Worker {
            epoch: 0,
            journal: Arc::default(),
            rows: Vec::new(),
            board: Board::new(block::Generation::of(&Value::Null)),
            batch: Batch::default(),
            committed: VecDeque::new(),
            committed_from: 0,
            gap: false,
        }
    }
}

impl Worker {
    /// Events of the history folded so far.
    pub fn position(&self) -> usize {
        self.board.position()
    }

    /// Its last input's history had events it let go (module docs).
    pub fn gap(&self) -> bool {
        self.gap
    }

    /// What it keeps ([`Retained`]).
    pub fn retained(&self) -> Retained {
        Retained {
            journal: self.journal.len(),
            rows: self.rows.len(),
            committed: self.committed.len(),
            board_bytes: self.board.owned_bytes(),
        }
    }

    /// Its epoch for `input`, started over on another; `false`: `input`'s
    /// history isn't one it can read on (a gap, or one it read past).
    fn on(&mut self, input: &Input) -> bool {
        if input.epoch != self.epoch {
            *self = Worker { epoch: input.epoch, board: Board::new(input.generation), ..Worker::default() };
        }
        self.gap = self.journal.len() < input.from;
        // The rows took these: let them go.
        let took = input.shared.took.load(Ordering::Acquire);
        while self.committed_from < took && self.committed.pop_front().is_some() {
            self.committed_from += 1;
        }
        !self.gap && self.journal.len() <= input.history_len
    }

    /// The journal up to `to`, taken from `input`'s staged chunks.
    fn take(&mut self, input: &Input, to: usize) {
        let have = self.journal.len();
        if have >= to {
            return;
        }
        let journal = Arc::make_mut(&mut self.journal);
        let mut at = input.from;
        for c in &input.chunks {
            let end = at + c.events.len();
            let (a, b) = (have.max(at), to.min(end));
            if a < b {
                journal.extend(c.events[a - at..b - at].iter().cloned());
                self.rows.extend(c.rows[a - at..b - at].iter().copied());
            }
            at = end;
            if at >= to {
                break;
            }
        }
        input.shared.taken.fetch_max(journal.len(), Ordering::AcqRel);
    }

    /// `input`'s history, the journal shared (a re-fold's: module docs);
    /// `None` on a gap.
    pub fn journal(&mut self, input: &Input) -> Option<Arc<Vec<Event>>> {
        if !self.on(input) {
            return None;
        }
        self.take(input, input.history_len);
        Some(self.journal.clone())
    }

    /// The board of `input`, drawn ([`Drawn`]): its history read on from
    /// where the last job stopped (from the start if its epoch is another),
    /// a slice of at most [`SLICE`] events at a time, `go_on` asked after
    /// each; then the overlay and the bound prefix. `None` if `go_on` said
    /// stop (what was read is kept for the next job), or on a gap.
    pub fn run(&mut self, input: &Input, go_on: &mut dyn FnMut() -> bool) -> Option<Drawn> {
        if !self.on(input) {
            return None;
        }
        while self.board.position() < input.history_len {
            let at = self.board.position();
            let to = (at + SLICE).min(input.history_len);
            self.take(input, to);
            for at in at..to {
                if let Some((start, end)) = self.batch.read(&self.journal, at) {
                    let (rows, committed) = (&self.rows, &mut self.committed);
                    fates(&self.journal, start, end, false, |pos, fate| {
                        if let Some(r) = rows[pos] {
                            committed.push_back((r, fate));
                        }
                    });
                }
            }
            self.board.read(&self.journal[..to]);
            if to < input.history_len && !go_on() {
                return None;
            }
        }
        // The overlay: the board, cloned, with the live turn read on top.
        let history = input.history_len;
        Arc::make_mut(&mut self.journal).extend(input.live.events.iter().cloned());
        self.rows.extend(input.live.rows.iter().copied());
        let events = &self.journal[..];
        let mut board = self.board.clone();
        board.read(events);
        let view = board.view(events);
        let bound = board.bound(events);
        // The fates of the live turn's batches and the open one's.
        let mut calls = Vec::new();
        let mut batch = self.batch;
        for at in history..events.len() {
            if let Some((start, end)) = batch.read(events, at) {
                fates(events, start, end, false, |p, f| calls.push((p, f)));
            }
        }
        fates(events, batch.start, events.len(), true, |p, f| calls.push((p, f)));
        let drawn = self.drawn(input, &view, &bound, &calls);
        Arc::make_mut(&mut self.journal).truncate(history);
        self.rows.truncate(history);
        Some(drawn)
    }

    fn drawn(&self, input: &Input, view: &State, bound: &State, fates: &[(usize, Fate)]) -> Drawn {
        let mut rows: HashMap<u64, RowBlocks> = HashMap::new();
        let row = |pos: usize| self.rows.get(pos).copied().flatten();
        for &(pos, fate) in fates {
            if let Some(r) = row(pos) {
                rows.entry(r).or_default().call = Some(fate);
            }
        }
        let shown = |r: &Ref| -> Option<Shown> {
            let b = view.block(r)?;
            let members = b
                .members()
                .iter()
                .filter_map(|m| {
                    let m = Ref::Block(m.clone());
                    let b = view.block(&m)?;
                    Some(Shown { block: Arc::new(b.clone()), bound: same(bound.block(&m), b), members: Vec::new() })
                })
                .collect();
            Some(Shown { block: Arc::new(b.clone()), bound: same(bound.block(r), b), members })
        };
        for (place, refs) in view.lists() {
            let Place::Transcript(pos) = place else { continue };
            let Some(r) = row(pos) else { continue };
            rows.entry(r).or_default().blocks.extend(refs.iter().filter_map(shown));
        }
        for run in view.runs() {
            if let Some(r) = row(run.at.event) {
                rows.entry(r).or_default().run = Some(Arc::from(&*run.summary));
            }
        }
        Drawn {
            epoch: input.epoch,
            history: input.history_len,
            events: self.journal.len(),
            rows: rows.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
            committed: self.committed.iter().copied().collect(),
            committed_to: self.committed_from + self.committed.len() as u64,
            live: view.live(),
        }
    }
}

/// The bound board's block, if it is the same incarnation as `b`.
fn same(bound: Option<&Block>, b: &Block) -> Option<Arc<Block>> {
    bound.filter(|x| x.tag == b.tag).map(|x| Arc::new(x.clone()))
}

#[cfg(test)]
mod tests;
