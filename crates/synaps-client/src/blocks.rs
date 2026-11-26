//! Blocks in a transcript (synaps-cards `docs/blocks.md` v1.3, §1.7, §1.8,
//! §5.1): the board's input, read from the transcript as the history will
//! hold it, and what each row draws of the board. Read-only: nothing here
//! sends (D5).
//!
//! **The input** is one canonical sequence of `cards_spec::board::Event`s
//! (the board docs' adapter guarantee 1), whichever way the rows came:
//! - **the history**, a message at a time as the transcript folds it
//!   ([`HistoryBlocks`]), read as [`cards_spec::fold::events`] reads a
//!   whole history (every call, a card or block call's input whole, the
//!   results of card and block calls, `ResultsEnd` where a message after an
//!   assistant message ends its results, the user's own text), so any split
//!   of a history gives the events of the whole;
//! - **the live turn** ([`live`]) after it, in the order the history will
//!   hold it, not the order its events came: a round's calls once their
//!   input is whole, in model order; then the results it has, in the order
//!   they came; then `ResultsEnd` once every call of the round has its
//!   result (or a later round or message started); the prompt that started
//!   a turn as its user message. A steer isn't read live (D8 reads the edit
//!   grammar; for drawing, the round's `ResultsEnd` already committed what
//!   it would): the history has it once it lands.
//!
//! **The work** ([`Worker`]) is the board's: the history's fold, read on
//! from where it stopped while the history only grows (a new epoch, a
//! history folded again, starts over), then the live overlay and the bound
//! prefix on a clone. It runs off the UI frame (the desk schedules it, the
//! board docs' guarantees 6 and 7): in slices of at most [`SLICE`] events,
//! asking between them whether to go on, so a job a newer one supersedes
//! stops, keeping what it read; and it publishes only a finished board, one
//! [`Drawn`] for one input.
//!
//! **What rows draw** ([`Drawn`], by row id): a block call's fate (a bound
//! or pending call draws no tool row; an unbound one is a plain row, the
//! fail-safe, §1.6), the blocks anchored at a call's row (§1.2: a put's, a
//! move's; a converted card's at its card's row), each with its bound
//! board's incarnation (its glow waits for the bind, §6.1), and a run's
//! title (`collapse` with `run`, §1.3).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cards_spec::block::{self, Receipt};
use cards_spec::board::{Board, Event, Place, State};
use cards_spec::fold::{self, NAMES as CARD_NAMES};
use serde_json::Value;

pub use cards_spec::block::{BlockId, Body, Kind, Ref, Tag};
pub use cards_spec::board::Block;

use crate::transcript::{Item, ItemKind, ToolStatus};

/// Events read between two asks whether to go on (§1.8: a yield every
/// 50,000 events or 30 ms; a slice this small is far under both).
pub const SLICE: usize = 4_096;

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

/// The history's events, read message by message ([`HistoryBlocks::message`]),
/// and each event's row (a call's: its tool row).
#[derive(Clone, Debug)]
pub struct HistoryBlocks {
    epoch: u64,
    generation: Option<block::Generation>,
    /// Its events, a chunk per message that gave any, shared with the jobs
    /// that read them ([`Input`]): taken without a copy.
    chunks: Vec<Chunk>,
    len: usize,
    /// Each tool id's latest call keeps its result (a card's or a block's).
    calls: HashMap<String, bool>,
    /// The last message was an assistant's: the next one ends its results.
    after_assistant: bool,
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
            chunks: Vec::new(),
            len: 0,
            calls: HashMap::new(),
            after_assistant: false,
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

    /// Its events, in order (tests; a job reads the chunks).
    pub fn events(&self) -> Vec<Event> {
        self.chunks.iter().flat_map(|c| c.events.iter().cloned()).collect()
    }

    /// History message `m`, its rows `rows` (the rows it pushed, in order).
    pub fn message(&mut self, m: &Value, rows: &[Item]) {
        if self.generation.is_none() {
            self.generation = Some(block::Generation::of(m));
        }
        let role = m["role"].as_str();
        let ends = std::mem::replace(&mut self.after_assistant, role == Some("assistant"));
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
                let mut tools = rows.iter().filter(|i| i.kind == ItemKind::Tool);
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    let Some(id) = b["id"].as_str() else { continue };
                    let name = b["name"].as_str().unwrap_or_default();
                    let kept = keeps_input(name);
                    self.blocks |= is_block_tool(name);
                    self.calls.insert(id.to_string(), kept);
                    let input = if kept { b["input"].clone() } else { Value::Null };
                    let row = tools.by_ref().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == id)).map(|i| i.id);
                    push(Event::Call { tool_id: id.into(), name: name.into(), input }, row);
                }
            }
            Some("user") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    if let Some(id) = b["tool_use_id"].as_str().filter(|id| self.calls.get(*id) == Some(&true))
                        && let Some(text) = crate::cards::result_text(&b["content"])
                    {
                        push(Event::Result { tool_id: id.into(), text }, None);
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
            self.chunks.push(Chunk { events: events.into(), rows: at.into() });
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

/// The live turn (`live`: the rows after the history's) as the history
/// will hold it, after `history` (module docs).
pub fn live(history: &HistoryBlocks, live: &[Item]) -> Live {
    let mut out = Live::default();
    let mut after = history.after_assistant;
    // The round being read: its rows, by item order.
    let mut round: Option<u32> = None;
    let mut calls: Vec<&Item> = Vec::new();
    let rows: Vec<&Item> = live.iter().filter(|i| !i.is_note()).collect();
    let flush = |out: &mut Live, calls: &mut Vec<&Item>, after: &mut bool, later: bool| {
        // The assistant message: its calls whose input is whole, in model order.
        if *after {
            push(out, Event::ResultsEnd, None);
        }
        *after = true;
        let mut whole: Vec<&Item> = calls
            .iter()
            .copied()
            .filter(|i| i.tool.as_ref().is_some_and(|t| !t.tool_id.is_empty() && t.status != Some(ToolStatus::Streaming)))
            .collect();
        if whole.is_empty() {
            calls.clear();
            return;
        }
        whole.sort_by_key(|i| i.seq()[0]);
        for it in &whole {
            let t = it.tool.as_ref().expect("a call");
            out.blocks |= is_block_tool(&t.name);
            let input = if keeps_input(&t.name) { it.kept_input().cloned().unwrap_or(Value::Null) } else { Value::Null };
            push(out, Event::Call { tool_id: t.tool_id.clone(), name: t.name.clone(), input }, Some(it.id));
        }
        // Its results message: the results it has (a card's or a block's,
        // its tool id's latest call), in the order they came.
        let mut done: Vec<&Item> =
            whole.iter().copied().filter(|i| i.tool.as_ref().and_then(|t| t.status).is_some_and(ToolStatus::is_terminal)).collect();
        done.sort_by_key(|i| i.seq()[1]);
        let mut latest: HashMap<&str, u64> = HashMap::new();
        for it in &whole {
            latest.insert(it.tool.as_ref().map(|t| t.tool_id.as_str()).unwrap_or_default(), it.id);
        }
        for it in &done {
            let t = it.tool.as_ref().expect("a call");
            if keeps_input(&t.name) && latest.get(t.tool_id.as_str()) == Some(&it.id) {
                push(out, Event::Result { tool_id: t.tool_id.clone(), text: t.result.clone() }, None);
            }
        }
        // Final once every call has its result, or the turn went on.
        if later || done.len() == calls.len() {
            push(out, Event::ResultsEnd, None);
            *after = false;
        }
        calls.clear();
    };
    for (k, it) in rows.iter().enumerate() {
        match it.kind {
            ItemKind::User => {
                if round.take().is_some() {
                    flush(&mut out, &mut calls, &mut after, true);
                }
                if after {
                    push(&mut out, Event::ResultsEnd, None);
                }
                after = false;
                push(&mut out, Event::User { text: it.text.clone() }, Some(it.id));
            }
            ItemKind::Assistant | ItemKind::Thinking | ItemKind::Tool => {
                if round.is_some_and(|r| r != it.round()) {
                    flush(&mut out, &mut calls, &mut after, true);
                }
                round = Some(it.round());
                if it.kind == ItemKind::Tool {
                    calls.push(it);
                }
            }
            _ => {}
        }
        if k + 1 == rows.len() && round.is_some() {
            flush(&mut out, &mut calls, &mut after, false);
        }
    }
    out
}

fn push(out: &mut Live, e: Event, row: Option<u64>) {
    out.events.push(e);
    out.rows.push(row);
}

/// One board job's input: the history's events (its chunks, shared), then
/// the live turn's, read as one history.
#[derive(Clone, Debug)]
pub struct Input {
    pub epoch: u64,
    pub generation: block::Generation,
    chunks: Vec<Chunk>,
    pub history_len: usize,
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
            chunks: history.chunks.clone(),
            history_len: history.len,
            live,
        })
    }

    /// Every event, the history's then the live turn's (tests).
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

/// The board's batching, for each block call's fate: the open batch's
/// calls by tool id (the latest takes it), each with its first result.
#[derive(Clone, Debug, Default)]
struct Fates {
    /// The open batch's block calls: position, and its answer (a result's
    /// position; `None` none yet; taken over: dropped from `ids`).
    open: Vec<(usize, Option<usize>, bool)>,
    ids: HashMap<String, usize>,
    results: bool,
    /// Every block call's fate once its batch committed, by position.
    done: HashMap<usize, Fate>,
}

impl Fates {
    fn read(&mut self, events: &[Event], at: usize) {
        let e = &events[at];
        let commits = match e {
            Event::Call { .. } => self.results,
            Event::Result { .. } => false,
            Event::ResultsEnd | Event::User { .. } => true,
        };
        if commits {
            for (pos, fate) in self.fates(events, false) {
                self.done.insert(pos, fate);
            }
            self.open.clear();
            self.ids.clear();
            self.results = false;
        }
        match e {
            Event::Call { tool_id, name, .. } => {
                // A later call takes the id over: the earlier never binds.
                if let Some(k) = self.ids.remove(tool_id.as_str()) {
                    self.open[k].2 = true;
                }
                if is_block_tool(name) {
                    self.ids.insert(tool_id.clone(), self.open.len());
                    self.open.push((at, None, false));
                }
            }
            Event::Result { tool_id, .. } => {
                self.results = true;
                if let Some(&k) = self.ids.get(tool_id.as_str())
                    && self.open[k].1.is_none()
                {
                    self.open[k].1 = Some(at);
                }
            }
            Event::ResultsEnd | Event::User { .. } => {}
        }
    }

    /// The open batch's calls' fates; `live`: a call with no result yet is
    /// pending, else (its batch committed) unbound for good.
    fn fates(&self, events: &[Event], live: bool) -> Vec<(usize, Fate)> {
        self.open
            .iter()
            .map(|&(pos, answer, taken)| {
                let fate = match (answer, taken) {
                    (_, true) => Fate::Unbound,
                    (None, false) if live => Fate::Pending,
                    (None, false) => Fate::Unbound,
                    (Some(r), false) => match (&events[pos], &events[r]) {
                        (Event::Call { input, .. }, Event::Result { text, .. }) => {
                            let binds = block::parse_call(input).ok().is_some_and(|b| Receipt::parse(text).is_some_and(|r| r.binds(&b)));
                            if binds { Fate::Bound } else { Fate::Unbound }
                        }
                        _ => Fate::Unbound,
                    },
                };
                (pos, fate)
            })
            .collect()
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
}

/// A block as drawn: the overlay's (§1.7), and the bound board's of the
/// same incarnation (`None`: created by a pending op); a group's members.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub block: Arc<Block>,
    pub bound: Option<Arc<Block>>,
    pub members: Vec<Shown>,
}

/// What every row draws of one finished board ([`Worker::run`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    /// The input it was folded from.
    pub epoch: u64,
    pub events: usize,
    pub rows: HashMap<u64, Arc<RowBlocks>>,
    /// Live blocks on the board, groups and members included.
    pub live: usize,
}

/// The board's work for one session, kept across jobs: the history's fold
/// read so far, and the fates of its block calls.
#[derive(Debug)]
pub struct Worker {
    epoch: u64,
    events: Vec<Event>,
    rows: Vec<Option<u64>>,
    /// Chunks of the epoch copied into `events`.
    chunks: usize,
    board: Board,
    fates: Fates,
}

impl Default for Worker {
    fn default() -> Self {
        Worker {
            epoch: 0,
            events: Vec::new(),
            rows: Vec::new(),
            chunks: 0,
            board: Board::new(block::Generation::of(&Value::Null)),
            fates: Fates::default(),
        }
    }
}

impl Worker {
    /// Events of the history folded so far.
    pub fn position(&self) -> usize {
        self.board.position()
    }

    /// The board of `input`, drawn ([`Drawn`]): its history read on from
    /// where the last job stopped (from the start if its epoch is another),
    /// a slice of at most [`SLICE`] events at a time, `go_on` asked after
    /// each; then the overlay and the bound prefix. `None` if `go_on` said
    /// stop: what was read is kept for the next job.
    pub fn run(&mut self, input: &Input, go_on: &mut dyn FnMut() -> bool) -> Option<Drawn> {
        if input.epoch != self.epoch || self.chunks > input.chunks.len() {
            *self = Worker { epoch: input.epoch, board: Board::new(input.generation), ..Worker::default() };
        }
        for c in &input.chunks[self.chunks..] {
            self.events.extend(c.events.iter().cloned());
            self.rows.extend(c.rows.iter().copied());
        }
        self.chunks = input.chunks.len();
        while self.board.position() < self.events.len() {
            let to = (self.board.position() + SLICE).min(self.events.len());
            for at in self.board.position()..to {
                self.fates.read(&self.events, at);
            }
            self.board.read(&self.events[..to]);
            if self.board.position() < self.events.len() && !go_on() {
                return None;
            }
        }
        // The overlay: the board, cloned, with the live turn read on top.
        let history = self.events.len();
        self.events.extend(input.live.events.iter().cloned());
        self.rows.extend(input.live.rows.iter().copied());
        let mut board = self.board.clone();
        let mut fates = self.fates.clone();
        for at in history..self.events.len() {
            fates.read(&self.events, at);
        }
        board.read(&self.events);
        let view = board.view(&self.events);
        let bound = board.bound(&self.events);
        let mut call_fates: Vec<(usize, Fate)> = fates.done.iter().map(|(&p, &f)| (p, f)).collect();
        call_fates.extend(fates.fates(&self.events, true));
        let drawn = self.drawn(input.epoch, &view, &bound, &call_fates);
        self.events.truncate(history);
        self.rows.truncate(history);
        Some(drawn)
    }

    fn drawn(&self, epoch: u64, view: &State, bound: &State, fates: &[(usize, Fate)]) -> Drawn {
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
        Drawn { epoch, events: self.events.len(), rows: rows.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(), live: view.live() }
    }
}

/// The bound board's block, if it is the same incarnation as `b`.
fn same(bound: Option<&Block>, b: &Block) -> Option<Arc<Block>> {
    bound.filter(|x| x.tag == b.tag).map(|x| Arc::new(x.clone()))
}

#[cfg(test)]
mod tests;
