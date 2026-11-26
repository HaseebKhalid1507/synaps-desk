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
//!   later message started; a prompt (`TurnStarted`) and a steer
//!   (`SteeringDelivered`) as the user messages the history will hold, the
//!   steer after its round's `ResultsEnd` (guarantee 3). A result whose
//!   round has ended is left out: the history's results message for that
//!   round is written, and nothing moves back across a commit. A history
//!   that ends with an assistant message (an attach mid-turn) has its
//!   results come live: they are that message's.
//!
//! **One journal.** The history's events are staged in the transcript
//! ([`HistoryBlocks`]) only until the worker of its epoch has taken them,
//! a slice at a time, then let go: the worker's journal
//! ([`Worker::journal`]) is the one copy the board reads (B2 reads one
//! contiguous history). A re-fold shares it, copy-on-write: a job that
//! reads on while a re-fold holds it copies it once. **The live turn** is
//! its log's ([`LiveLog`]): its events written once each, as their facts
//! come (a call's input is its row's, shared, not copied), and a job's
//! input is a cursor into it, not a copy ([`Input`]). The frame's work for
//! a live event is the events it adds, never the turn so far. A job
//! copies the live turn's events after the history in its journal a slice
//! at a time, off the frame, and reads the overlay on from where the last
//! job stopped while what it read holds. Everything counted is counted
//! with its heap ([`event_bytes`]): staged, journal, the live log, the
//! overlay. **The worker is the history's** ([`HistoryBlocks::worker`]): what
//! owns the transcript (a store, the tab cache) owns its fold, and a new
//! history (another epoch) has a new worker, so a store left, a tab
//! evicted or a history replaced lets its fold go with it, and a tab shown
//! again reads on from where it was. A worker that is not its history's
//! (tests) and missed staged events has a gap ([`Worker::gap`]): it draws
//! nothing new.
//!
//! **The work** ([`Worker::run`]) is the board's: the history's fold, read
//! on from where it stopped while the history only grows (a new epoch
//! starts over), then the live overlay (the board at the history's end,
//! the live turn read on it) and the bound prefix. It runs off the UI
//! frame (the desk schedules it, guarantees 6 and 7): its staged events
//! taken and read, and the live turn's copied and read, a slice of at most
//! [`SLICE`] events at a time, `go_on` asked between slices; a stopped job
//! keeps what it read. B2's `read` of a slice, its `view` and `bound`
//! can't be preempted, and the projection runs after the last slice
//! without a check. It publishes only a finished board, one [`Drawn`] for
//! one input.
//!
//! **Fates** (§1.6, §1.7): a block call's ops apply if its batch's first
//! result for its id binds it. The worker keeps the open batch's start and
//! one flag: a batch's fates are read from the journal when it commits,
//! and the open batch's at each overlay, in passes of at most [`PASS`]
//! calls, each reading the rest of the batch once through an index
//! borrowing their ids (as B2's replay). A committed call's fate is
//! published once ([`Drawn::committed`]) and kept by its row from then on
//! (a byte of the row the transcript has anyway); until the rows take
//! them, the worker queues the fates committed since (one entry a block
//! call, counted), and lets them go once taken. The overlay keeps the
//! fates of the live turn's committed batches while it reads on. So
//! refused calls grow the journal, their rows and, until a publication is
//! taken, that queue.
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
use std::sync::{Arc, LazyLock, Mutex};

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
    /// What its worker keeps, roughly (bytes), as of its last job.
    bytes: AtomicUsize,
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
    /// Their bytes, heap included.
    staged_bytes: usize,
    from: usize,
    len: usize,
    shared: Arc<Shared>,
    /// The last message, if an assistant's: its calls' tool ids (an id's
    /// latest call: whether it keeps its result), and how many calls.
    open: Option<(HashMap<String, bool>, usize)>,
    /// A block call read: else there is no board to draw.
    blocks: bool,
    /// Its worker (module docs: one per history, shared by its clones).
    worker: Arc<Mutex<Worker>>,
}

/// A message's events and their rows.
#[derive(Clone, Debug)]
struct Chunk {
    events: Arc<[Event]>,
    rows: Arc<[Option<u64>]>,
    /// Its bytes, heap included ([`event_bytes`]).
    bytes: usize,
}

impl Default for HistoryBlocks {
    fn default() -> Self {
        HistoryBlocks {
            epoch: next_epoch(),
            generation: None,
            staged: VecDeque::new(),
            staged_bytes: 0,
            from: 0,
            len: 0,
            shared: Arc::default(),
            open: None,
            blocks: false,
            worker: Arc::default(),
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

    /// Its worker ([`Worker`]): the board's work on this history.
    pub fn worker(&self) -> &Arc<Mutex<Worker>> {
        &self.worker
    }

    /// What it keeps (bytes, heap included): the staged events and their
    /// rows, the last message's call ids, and its worker's fold as of its
    /// last job ([`Worker::bytes`]).
    pub fn bytes(&self) -> usize {
        let open = self.open.as_ref().map_or(0, |(ids, _)| ids.keys().map(|k| k.capacity() + 48).sum::<usize>());
        self.staged_bytes + self.staged.len() * 64 + open + self.shared.bytes.load(Ordering::Relaxed)
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
            self.staged_bytes -= self.staged.pop_front().map_or(0, |c| c.bytes);
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
            let bytes = events.iter().map(event_bytes).sum::<usize>() + at.len() * std::mem::size_of::<Option<u64>>();
            self.staged_bytes += bytes;
            self.staged.push_back(Chunk { events: events.into(), rows: at.into(), bytes });
        }
        self.prune();
    }
}

/// What JSON value `v` holds on the heap (bytes, roughly: its strings, and
/// its arrays' and maps' slots).
pub fn value_bytes(v: &Value) -> usize {
    match v {
        Value::String(s) => s.capacity(),
        Value::Array(a) => a.capacity() * std::mem::size_of::<Value>() + a.iter().map(value_bytes).sum::<usize>(),
        Value::Object(m) => m.iter().map(|(k, v)| k.capacity() + std::mem::size_of::<(String, Value)>() + 16 + value_bytes(v)).sum(),
        _ => 0,
    }
}

/// What event `e` keeps, heap included (bytes).
pub fn event_bytes(e: &Event) -> usize {
    std::mem::size_of::<Event>()
        + match e {
            Event::Call { tool_id, name, input } => tool_id.capacity() + name.capacity() + value_bytes(input),
            Event::Result { tool_id, text } => tool_id.capacity() + text.capacity(),
            Event::User { text } => text.capacity(),
            Event::ResultsEnd => 0,
        }
}

/// One of the live turn's events as its log keeps it ([`LiveLog`]): a
/// call's input is its row's ([`Item::kept_input`], shared, not copied),
/// made an event's only when a job reads it, off the frame.
#[derive(Clone, Debug)]
enum Fact {
    Call { tool_id: String, name: String, input: Option<Arc<Value>>, row: u64 },
    Result { tool_id: String, text: String },
    User { text: String, row: Option<u64> },
    End,
}

static END: Fact = Fact::End;

impl Fact {
    /// The board's event and its row.
    fn event(&self) -> (Event, Option<u64>) {
        match self {
            Fact::Call { tool_id, name, input, row } => {
                let input = input.as_deref().cloned().unwrap_or(Value::Null);
                (Event::Call { tool_id: tool_id.clone(), name: name.clone(), input }, Some(*row))
            }
            Fact::Result { tool_id, text } => (Event::Result { tool_id: tool_id.clone(), text: text.clone() }, None),
            Fact::User { text, row } => (Event::User { text: text.clone() }, *row),
            Fact::End => (Event::ResultsEnd, None),
        }
    }

    /// What it keeps of its own (bytes, heap included; a call's input is
    /// its row's).
    fn bytes(&self) -> usize {
        std::mem::size_of::<Fact>()
            + match self {
                Fact::Call { tool_id, name, .. } => tool_id.capacity() + name.capacity(),
                Fact::Result { tool_id, text } => tool_id.capacity() + text.capacity(),
                Fact::User { text, .. } => text.capacity(),
                Fact::End => 0,
            }
    }
}

/// The live turn's events in order (module docs): those before the open
/// round, then the open round's (its leading `ResultsEnd`, its whole calls
/// in model order, its results, its `ResultsEnd`). The one copy: the
/// transcript adds to it a fact at a time ([`LiveLog`]), and a job reads
/// it through its cursor, a slice at a time, off the frame. A change other
/// than an event added at its end (a reset, a call whole after its round's
/// results, a call's input again) is a cut, at the lowest place it
/// changed: a cursor holds for every cut past its length.
#[derive(Debug, Default)]
struct Store {
    /// The events before the open round, in segments (a sealed round's
    /// calls and results are moved in whole: a seal moves no event), and
    /// where each starts.
    done: Vec<Vec<Fact>>,
    starts: Vec<usize>,
    done_len: usize,
    /// The last segment takes single events (a sealed round's don't).
    loose: bool,
    lead: bool,
    calls: Vec<Fact>,
    results: Vec<Fact>,
    end: bool,
    cuts: Vec<usize>,
    /// Its facts' bytes ([`Fact::bytes`]).
    bytes: usize,
}

impl Store {
    fn len(&self) -> usize {
        self.done_len + usize::from(self.lead) + self.calls.len() + self.results.len() + usize::from(self.end)
    }

    /// Its `k`th event.
    fn get(&self, mut k: usize) -> &Fact {
        if k < self.done_len {
            let seg = self.starts.partition_point(|&s| s <= k) - 1;
            return &self.done[seg][k - self.starts[seg]];
        }
        k -= self.done_len;
        if self.lead {
            if k == 0 {
                return &END;
            }
            k -= 1;
        }
        if k < self.calls.len() {
            return &self.calls[k];
        }
        k -= self.calls.len();
        self.results.get(k).unwrap_or(&END)
    }

    /// How many of its first `len` events still hold for a reader that
    /// saw its first `cuts` cuts.
    fn kept(&self, cuts: usize, len: usize) -> usize {
        self.cuts.get(cuts..).unwrap_or_default().iter().fold(len, |a, &c| a.min(c))
    }

    fn push(v: &mut Vec<Fact>, bytes: &mut usize, f: Fact) {
        *bytes += f.bytes();
        v.push(f);
    }

    /// Event `f` after the events before the open round.
    fn push_done(&mut self, f: Fact) {
        self.bytes += f.bytes();
        if !self.loose {
            self.starts.push(self.done_len);
            self.done.push(Vec::new());
            self.loose = true;
        }
        self.done.last_mut().expect("a loose segment").push(f);
        self.done_len += 1;
    }

    /// Events `v` after the events before the open round, moved in whole.
    fn append_done(&mut self, v: Vec<Fact>) {
        if v.is_empty() {
            return;
        }
        self.starts.push(self.done_len);
        self.done_len += v.len();
        self.done.push(v);
        self.loose = false;
    }

    /// The open round's start: where its leading `ResultsEnd` is.
    fn open(&self) -> usize {
        self.done_len
    }

    /// Its calls' start.
    fn calls_at(&self) -> usize {
        self.done_len + usize::from(self.lead)
    }
}

/// The live turn as the protocol told it (module docs): kept by the
/// transcript from its events since the history's last fold, the board's
/// live input. Never a row's status or text. Each fact costs the frame the
/// events it adds (a call whole, a result, a user message, a
/// `ResultsEnd`), never the turn so far: the events go into its
/// [`Store`] as they come, and a job reads them there. What it keeps
/// besides is two rounds' calls (the open one's, and the one before it,
/// which a reset of the open one makes the batch again) and the ids of
/// the history's last message's calls that reported.
#[derive(Debug)]
pub struct LiveLog {
    store: Arc<Mutex<Store>>,
    /// Its identity: a worker's read of another log is another's.
    id: u64,
    /// Bumped when its events change.
    version: u64,
    /// The history's last message's calls, if it is an assistant's (read
    /// at its first fact; the history doesn't change under a log).
    head: Option<Option<usize>>,
    head_answered: HashSet<String>,
    head_ended: bool,
    /// After the events before the open round, a batch is open: the next
    /// step starts with a `ResultsEnd`.
    after: bool,
    open: Option<Round>,
    /// The round before the open one (sealed into the events before it).
    sealed: Option<Round>,
    /// What results go to: the history's last message, a user message
    /// (none), the open round, or, the open round reset, the one before.
    target: Target,
    /// What `target` was when the open round started.
    before: Target,
    /// The last step is a response that gave nothing yet.
    empty: bool,
    blocks: bool,
    /// The first user message's generation (§1.2), for a turn with no
    /// history yet.
    first: Option<block::Generation>,
    /// Facts written to its store, and moved within it (tests: the
    /// frame's work).
    work: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Target {
    #[default]
    Head,
    User,
    Open,
    Sealed,
}

#[derive(Clone, Debug, Default)]
struct Round {
    /// Its calls, in model order (as they started).
    calls: Vec<LiveCall>,
    /// Each tool id's latest call, and each row's.
    ids: HashMap<String, usize>,
    rows: HashMap<u64, usize>,
    /// Calls whole and answered.
    done: usize,
    /// Its last whole call.
    last_whole: Option<usize>,
}

impl Round {
    fn complete(&self) -> bool {
        !self.calls.is_empty() && self.done == self.calls.len()
    }

    /// Where whole call `k` is among its whole calls.
    fn place(&self, k: usize) -> usize {
        self.calls[..k].iter().filter(|c| c.whole).count()
    }

    /// What it keeps (bytes, heap included; an input is its row's).
    fn bytes(&self) -> usize {
        self.calls.iter().map(|c| std::mem::size_of::<LiveCall>() + c.tool_id.capacity() + c.name.capacity()).sum::<usize>()
            + self.ids.keys().map(|k| k.capacity() + 48).sum::<usize>()
            + self.rows.len() * 32
    }
}

#[derive(Clone, Debug)]
struct LiveCall {
    tool_id: String,
    name: String,
    /// A card's or block's input, whole ([`keeps_input`]): its row's.
    input: Option<Arc<Value>>,
    /// Its input is whole (`ToolUse`).
    whole: bool,
    /// Its final result came.
    answered: bool,
}

impl Default for LiveLog {
    fn default() -> Self {
        static IDS: AtomicU64 = AtomicU64::new(1);
        LiveLog {
            store: Arc::default(),
            id: IDS.fetch_add(1, Ordering::Relaxed),
            version: 0,
            head: None,
            head_answered: HashSet::new(),
            head_ended: false,
            after: false,
            open: None,
            sealed: None,
            target: Target::Head,
            before: Target::Head,
            empty: false,
            blocks: false,
            first: None,
            work: 0,
        }
    }
}

/// A copy is another log, of the same facts.
impl Clone for LiveLog {
    fn clone(&self) -> Self {
        let s = self.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let store = Store {
            done: s.done.clone(),
            starts: s.starts.clone(),
            done_len: s.done_len,
            loose: s.loose,
            lead: s.lead,
            calls: s.calls.clone(),
            results: s.results.clone(),
            end: s.end,
            cuts: Vec::new(),
            bytes: s.bytes,
        };
        LiveLog {
            store: Arc::new(Mutex::new(store)),
            head_answered: self.head_answered.clone(),
            open: self.open.clone(),
            sealed: self.sealed.clone(),
            first: self.first,
            ..*self.fresh()
        }
    }
}

impl LiveLog {
    /// Its scalars, under a new identity (for [`Clone`]).
    fn fresh(&self) -> Box<LiveLog> {
        Box::new(LiveLog {
            version: self.version,
            head: self.head,
            head_ended: self.head_ended,
            after: self.after,
            target: self.target,
            before: self.before,
            empty: self.empty,
            blocks: self.blocks,
            work: self.work,
            ..LiveLog::default()
        })
    }

    /// A block call among its calls.
    pub fn has_blocks(&self) -> bool {
        self.blocks
    }

    /// Bumped when its events change (a desk's revision follows it).
    pub fn version(&self) -> (u64, u64) {
        (self.id, self.version)
    }

    /// Calls it keeps besides its events (tests: two rounds' at most).
    pub fn calls(&self) -> usize {
        self.open.as_ref().map_or(0, |r| r.calls.len()) + self.sealed.as_ref().map_or(0, |r| r.calls.len())
    }

    /// Events it holds.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Facts written to its store, and moved within it, so far (tests: the
    /// frame's work is the events each fact adds).
    pub fn work(&self) -> usize {
        self.work
    }

    /// What it keeps (bytes, heap included; the calls' inputs are their
    /// rows').
    pub fn bytes(&self) -> usize {
        let s = self.lock();
        std::mem::size_of::<Store>()
            + s.bytes
            + (s.done.iter().map(Vec::capacity).sum::<usize>() + s.calls.capacity() + s.results.capacity()
                - s.done_len
                - s.calls.len()
                - s.results.len())
                * std::mem::size_of::<Fact>()
            + (s.done.capacity() * std::mem::size_of::<Vec<Fact>>())
            + (s.starts.capacity() + s.cuts.capacity()) * std::mem::size_of::<usize>()
            + self.open.as_ref().map_or(0, Round::bytes)
            + self.sealed.as_ref().map_or(0, Round::bytes)
            + self.head_answered.iter().map(|k| k.capacity() + 48).sum::<usize>()
    }

    /// Its events (tests).
    pub fn events(&self) -> Vec<Event> {
        let s = self.lock();
        (0..s.len()).map(|k| s.get(k).event().0).collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Read at its first fact: the history's last message.
    fn init(&mut self, history: &HistoryBlocks) {
        if self.head.is_none() {
            let n = history.open.as_ref().map(|(_, n)| *n);
            self.head = Some(n);
            self.after = n.is_some();
        }
    }

    fn changed(&mut self) {
        self.version += 1;
    }

    /// The open round goes before what comes next: its events are the
    /// ones before the open round now (the same events: no cut).
    fn seal(&mut self, s: &mut Store) {
        let Some(r) = self.open.take() else { return };
        // Its parts moved in whole: no event moves (the frame's work is the
        // events added, never a round's).
        if std::mem::take(&mut s.lead) {
            s.push_done(Fact::End);
        }
        let (calls, results) = (std::mem::take(&mut s.calls), std::mem::take(&mut s.results));
        s.append_done(calls);
        s.append_done(results);
        if std::mem::take(&mut s.end) {
            s.push_done(Fact::End);
        }
        self.after = !r.complete();
        self.sealed = Some(r);
    }

    /// The last step gets something: a new round, unless it is the open
    /// one (an empty response, a user message or nothing before it).
    fn opened(&mut self, s: &mut Store) -> bool {
        if self.target == Target::Open && !self.empty {
            return false;
        }
        let before = if self.open.is_some() { Target::Sealed } else { self.target };
        if self.open.is_some() {
            self.seal(s);
        } else if before != Target::Sealed {
            self.sealed = None;
        }
        self.open = Some(Round::default());
        (self.target, self.before, self.empty) = (Target::Open, before, false);
        if self.after {
            s.lead = true;
            self.work += 1;
            return true;
        }
        false
    }

    /// The open round's `ResultsEnd`: there once every call has its
    /// result, taken back (a cut) when a call comes after.
    fn sync_end(&mut self, s: &mut Store) -> bool {
        let want = self.open.as_ref().is_some_and(Round::complete);
        if s.end == want {
            return false;
        }
        if want {
            s.end = true;
            self.work += 1;
        } else {
            let at = s.len() - 1;
            s.end = false;
            s.cuts.push(at);
        }
        true
    }

    /// A prompt (`TurnStarted`) or a delivered steer (`SteeringDelivered`):
    /// a user message of `text`, its row's.
    pub fn user(&mut self, history: &HistoryBlocks, text: &str, row: Option<u64>) {
        self.init(history);
        if self.first.is_none() {
            self.first = Some(block::Generation::of(&serde_json::json!({"role": "user", "content": text})));
        }
        let store = self.store.clone();
        let mut s = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = &mut *s;
        self.seal(s);
        if std::mem::take(&mut self.after) {
            s.push_done(Fact::End);
        }
        s.push_done(Fact::User { text: text.to_string(), row });
        self.work += 2;
        (self.target, self.sealed, self.empty) = (Target::User, None, false);
        self.changed();
    }

    /// A response starts: the next assistant message.
    pub fn response(&mut self) {
        self.empty = true;
    }

    /// The response streams again from its start: what it gave goes.
    pub fn reset(&mut self) {
        if self.empty || self.target != Target::Open {
            return;
        }
        let store = self.store.clone();
        let mut s = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let at = s.open();
        let gone: usize = s.calls.iter().chain(&s.results).map(Fact::bytes).sum();
        let had = s.len() > at;
        (s.lead, s.end) = (false, false);
        s.calls.clear();
        s.results.clear();
        s.bytes -= gone;
        if had {
            s.cuts.push(at);
            self.changed();
        }
        self.open = None;
        (self.target, self.empty) = (self.before, true);
    }

    /// The response's text or thinking.
    pub fn content(&mut self, history: &HistoryBlocks) {
        self.init(history);
        let store = self.store.clone();
        let mut s = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.opened(&mut s) {
            self.changed();
        }
    }

    /// Call `tool_id` (row `row`) starts: its input streams.
    pub fn started(&mut self, history: &HistoryBlocks, row: u64, tool_id: &str, name: &str) {
        self.init(history);
        let was = self.blocks;
        self.blocks |= is_block_tool(name);
        let store = self.store.clone();
        let mut s = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut moved = self.opened(&mut s) | (was != self.blocks);
        let r = self.open.as_mut().expect("opened");
        if !r.rows.contains_key(&row) {
            let k = r.calls.len();
            r.ids.insert(tool_id.to_string(), k);
            r.rows.insert(row, k);
            r.calls.push(LiveCall { tool_id: tool_id.into(), name: name.into(), input: None, whole: false, answered: false });
            moved |= self.sync_end(&mut s);
        }
        if moved {
            self.changed();
        }
    }

    /// Call `tool_id`'s input is whole (`input`: kept, a card's or a
    /// block's). Again for the same row: its input is this one.
    pub fn call(&mut self, history: &HistoryBlocks, row: u64, tool_id: &str, name: &str, input: Option<Arc<Value>>) {
        self.init(history);
        let was = self.blocks;
        self.blocks |= is_block_tool(name);
        let store = self.store.clone();
        let mut s = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = &mut *s;
        let mut moved = self.opened(s) | (was != self.blocks);
        let r = self.open.as_mut().expect("opened");
        let fact = |input: &Option<Arc<Value>>| Fact::Call { tool_id: tool_id.into(), name: name.into(), input: input.clone(), row };
        match r.rows.get(&row).copied() {
            Some(k) => {
                let c = &mut r.calls[k];
                let same = c.whole
                    && c.tool_id == tool_id
                    && c.name == name
                    && match (&c.input, &input) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                        (None, None) => true,
                        _ => false,
                    };
                if c.tool_id != tool_id {
                    c.tool_id = tool_id.to_string();
                    r.ids.insert(tool_id.to_string(), k);
                }
                let c = &mut r.calls[k];
                c.name = name.to_string();
                c.input = input.clone();
                if !std::mem::replace(&mut c.whole, true) {
                    if c.answered {
                        r.done += 1;
                    }
                    // In model order: after its round's whole calls, else
                    // among them (a cut).
                    let at = r.place(k);
                    if r.last_whole.is_none_or(|l| l < k) {
                        r.last_whole = Some(k);
                        if !s.results.is_empty() || s.end {
                            s.cuts.push(s.calls_at() + s.calls.len());
                        }
                        Store::push(&mut s.calls, &mut s.bytes, fact(&input));
                    } else {
                        s.cuts.push(s.calls_at() + at);
                        let f = fact(&input);
                        s.bytes += f.bytes();
                        s.calls.insert(at, f);
                    }
                    self.work += 1;
                    moved = true;
                } else if !same {
                    // Its input again, another: in its place (a cut).
                    let at = r.place(k);
                    s.cuts.push(s.calls_at() + at);
                    let f = fact(&input);
                    s.bytes = s.bytes + f.bytes() - s.calls[at].bytes();
                    s.calls[at] = f;
                    self.work += 1;
                    moved = true;
                }
            }
            None => {
                let k = r.calls.len();
                r.ids.insert(tool_id.to_string(), k);
                r.rows.insert(row, k);
                r.calls.push(LiveCall { tool_id: tool_id.into(), name: name.into(), input: input.clone(), whole: true, answered: false });
                r.last_whole = Some(k);
                if !s.results.is_empty() || s.end {
                    s.cuts.push(s.calls_at() + s.calls.len());
                }
                Store::push(&mut s.calls, &mut s.bytes, fact(&input));
                self.work += 1;
                moved = true;
            }
        }
        moved |= self.sync_end(s);
        if moved {
            self.changed();
        }
    }

    /// Call `tool_id`'s final result. It is the open batch's (the last
    /// message, if an assistant's: a live round, else `history`'s last
    /// message) latest call of that id; one that has its result keeps it,
    /// and a result whose batch has ended is left out (module docs).
    pub fn result(&mut self, history: &HistoryBlocks, tool_id: &str, text: &str) {
        self.init(history);
        let store = self.store.clone();
        let mut s = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = &mut *s;
        let fact = || Fact::Result { tool_id: tool_id.to_string(), text: text.to_string() };
        match self.target {
            Target::Open | Target::Sealed => {
                let open = self.target == Target::Open;
                let Some(r) = (if open { self.open.as_mut() } else { self.sealed.as_mut() }) else { return };
                let Some(&k) = r.ids.get(tool_id) else { return };
                let c = &mut r.calls[k];
                if std::mem::replace(&mut c.answered, true) {
                    return;
                }
                if c.whole {
                    r.done += 1;
                }
                let kept = keeps_input(&c.name);
                let complete = r.complete();
                let mut moved = false;
                if kept {
                    // At the events' end: the open round's results, or, the
                    // open round reset, the one before it, last.
                    if open {
                        Store::push(&mut s.results, &mut s.bytes, fact());
                    } else {
                        s.push_done(fact());
                    }
                    self.work += 1;
                    moved = true;
                }
                if open {
                    moved |= self.sync_end(s);
                } else if complete {
                    s.push_done(Fact::End);
                    self.after = false;
                    self.work += 1;
                    moved = true;
                }
                if moved {
                    self.changed();
                }
            }
            Target::User => {}
            Target::Head => {
                let Some(&kept) = history.open.as_ref().and_then(|(ids, _)| ids.get(tool_id)) else { return };
                if !self.head_answered.insert(tool_id.to_string()) {
                    return;
                }
                let mut moved = false;
                if kept {
                    s.push_done(fact());
                    self.work += 1;
                    moved = true;
                }
                if let Some(Some(n)) = self.head
                    && n > 0
                    && self.head_answered.len() == n
                    && !std::mem::replace(&mut self.head_ended, true)
                {
                    s.push_done(Fact::End);
                    self.after = false;
                    self.work += 1;
                    moved = true;
                }
                if moved {
                    self.changed();
                }
            }
        }
    }
}

/// A job's read of the live turn ([`Input`]): its log's events as of its
/// cuts, the first `len`. Nothing copied: a job reads them from the log.
#[derive(Clone, Debug)]
struct Cursor {
    store: Arc<Mutex<Store>>,
    log: u64,
    cuts: usize,
    len: usize,
    blocks: bool,
}

/// One board job's input: the history's staged events (its chunks,
/// shared), then the live turn's (a cursor into its log), read as one
/// history.
#[derive(Clone, Debug)]
pub struct Input {
    pub epoch: u64,
    pub generation: block::Generation,
    /// The staged chunks: the history's events from `from` on.
    chunks: Vec<Chunk>,
    pub from: usize,
    pub history_len: usize,
    /// The history ends with an assistant message's calls: their batch is
    /// open, and the live turn's results can still commit it.
    pub history_open: bool,
    shared: Arc<Shared>,
    live: Cursor,
    /// The history's worker ([`HistoryBlocks::worker`]).
    pub worker: Arc<Mutex<Worker>>,
}

impl Input {
    /// `history` and the live turn `log` as one job's input; `None` with no
    /// block call in either (no board to draw). Its live turn is a cursor
    /// into `log`: nothing is copied.
    pub fn of(history: &HistoryBlocks, log: &LiveLog) -> Option<Self> {
        if !history.blocks && !log.blocks {
            return None;
        }
        let (cuts, len) = {
            let s = log.lock();
            (s.cuts.len(), s.len())
        };
        // The generation is the history's first message's (§1.2). With no
        // history yet, the live prompt's, as the history will hold it (a
        // string): folded again once the history lands, as any live turn.
        Some(Input {
            epoch: history.epoch,
            generation: history.generation.or(log.first).unwrap_or_else(|| block::Generation::of(&Value::Null)),
            chunks: history.staged.iter().cloned().collect(),
            from: history.from,
            history_len: history.len,
            history_open: history.open.as_ref().is_some_and(|(_, n)| *n > 0),
            shared: history.shared.clone(),
            live: Cursor { store: log.store.clone(), log: log.id, cuts, len, blocks: log.blocks },
            worker: history.worker.clone(),
        })
    }

    /// Its live turn's events.
    pub fn live_len(&self) -> usize {
        self.live.len
    }

    /// A block call in its live turn.
    pub fn live_blocks(&self) -> bool {
        self.live.blocks
    }

    /// Its live turn's events, copied (a re-fold of the whole input's);
    /// `None` if its log changed them since (a newer input stands).
    pub fn live_events(&self) -> Option<Vec<Event>> {
        let s = self.live.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (s.kept(self.live.cuts, self.live.len) == self.live.len).then(|| (0..self.live.len).map(|k| s.get(k).event().0).collect())
    }

    /// Its live turn's events' rows (tests).
    #[cfg(test)]
    fn live_rows(&self) -> Vec<Option<u64>> {
        let s = self.live.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (0..self.live.len).map(|k| s.get(k).event().1).collect()
    }

    /// Its events: the staged history's (all of it, if no worker took
    /// any), then the live turn's (tests).
    pub fn events(&self) -> Vec<Event> {
        let live = self.live_events().unwrap_or_default();
        self.chunks.iter().flat_map(|c| c.events.iter().cloned()).chain(live).collect()
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
    /// Its conversation generation (§1.2): the same history, folded again
    /// (a resync), has the same; a cleared or compacted one another.
    pub generation: Option<block::Generation>,
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
/// each), those of them the live turn's, the committed calls' fates the
/// rows haven't taken yet, the board's owned bytes (B2's working set, its
/// card fold apart), and all it keeps in bytes, heap included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retained {
    pub journal: usize,
    pub rows: usize,
    pub live: usize,
    pub committed: usize,
    pub board_bytes: usize,
    pub bytes: usize,
}

/// Live events copied from a log while holding it (the frame waits at
/// most this long to add one).
const COPY: usize = 256;

/// The board's work for one session, kept across jobs (module docs).
#[derive(Debug)]
pub struct Worker {
    epoch: u64,
    /// The history's events, the one copy, and each event's row (a
    /// call's: its tool row); then, from `hist` on, the live turn's, as a
    /// job read them from its log ([`Over`]).
    journal: Arc<Vec<Event>>,
    rows: Vec<Option<u64>>,
    hist: usize,
    /// The journal's bytes, heap included ([`event_bytes`]), kept as
    /// events come and go.
    bytes: usize,
    board: Board,
    batch: Batch,
    /// Committed calls' rows and fates the rows haven't taken, and how
    /// many before them they took.
    committed: VecDeque<(u64, Fate)>,
    committed_from: u64,
    gap: bool,
    over: Option<Over>,
}

/// The live overlay as far as it is read (module docs): of log `log`'s
/// events (as of its first `cuts` cuts), on the history's first `history`.
#[derive(Debug)]
struct Over {
    log: u64,
    cuts: usize,
    history: usize,
    /// The board at the history's end, read on over the live turn's first
    /// `read` events; the batching there, and the fates of the batches
    /// they committed.
    board: Board,
    read: usize,
    batch: Batch,
    fates: Vec<(usize, Fate)>,
}

impl Default for Worker {
    fn default() -> Self {
        Worker {
            epoch: 0,
            journal: Arc::default(),
            rows: Vec::new(),
            hist: 0,
            bytes: 0,
            board: Board::new(block::Generation::of(&Value::Null)),
            batch: Batch::default(),
            committed: VecDeque::new(),
            committed_from: 0,
            gap: false,
            over: None,
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

    /// What it keeps (bytes, heap included): its journal and rows, the
    /// committed fates not taken yet, its board and the overlay's.
    pub fn bytes(&self) -> usize {
        self.bytes
            + self.rows.capacity() * std::mem::size_of::<Option<u64>>()
            + self.committed.len() * std::mem::size_of::<(u64, Fate)>()
            + self.board.owned_bytes()
            + self.over.as_ref().map_or(0, |o| o.board.owned_bytes() + o.fates.capacity() * std::mem::size_of::<(usize, Fate)>())
    }

    /// What it keeps ([`Retained`]).
    pub fn retained(&self) -> Retained {
        Retained {
            journal: self.journal.len(),
            rows: self.rows.len(),
            live: self.journal.len() - self.hist,
            committed: self.committed.len(),
            board_bytes: self.board.owned_bytes(),
            bytes: self.bytes(),
        }
    }

    /// The live turn's events it read, as far as the overlay read them
    /// (tests).
    pub fn live_read(&self) -> usize {
        self.over.as_ref().map_or(0, |o| o.read)
    }

    /// Its epoch for `input`, started over on another; `false`: `input`'s
    /// history isn't one it can read on (a gap, or one it read past).
    fn on(&mut self, input: &Input) -> bool {
        if input.epoch != self.epoch {
            *self = Worker { epoch: input.epoch, board: Board::new(input.generation), ..Worker::default() };
        }
        self.gap = self.hist < input.from;
        // The rows took these: let them go.
        let took = input.shared.took.load(Ordering::Acquire);
        while self.committed_from < took && self.committed.pop_front().is_some() {
            self.committed_from += 1;
        }
        !self.gap && self.hist <= input.history_len
    }

    /// The live turn's events in the journal cut to `n`.
    fn live_to(&mut self, n: usize) {
        let to = self.hist + n;
        if self.journal.len() > to {
            let journal = Arc::make_mut(&mut self.journal);
            self.bytes -= journal[to..].iter().map(event_bytes).sum::<usize>();
            journal.truncate(to);
            self.rows.truncate(to);
        }
    }

    /// The journal up to `to`, taken from `input`'s staged chunks (no live
    /// events in it).
    fn take(&mut self, input: &Input, to: usize) {
        let have = self.hist;
        if have >= to {
            return;
        }
        self.live_to(0);
        self.over = None;
        let journal = Arc::make_mut(&mut self.journal);
        let mut at = input.from;
        for c in &input.chunks {
            let end = at + c.events.len();
            let (a, b) = (have.max(at), to.min(end));
            if a < b {
                let events = &c.events[a - at..b - at];
                self.bytes += events.iter().map(event_bytes).sum::<usize>();
                journal.extend(events.iter().cloned());
                self.rows.extend(c.rows[a - at..b - at].iter().copied());
            }
            at = end;
            if at >= to {
                break;
            }
        }
        self.hist = journal.len();
        input.shared.taken.fetch_max(self.hist, Ordering::AcqRel);
        input.shared.bytes.store(self.bytes(), Ordering::Relaxed);
    }

    /// `input`'s history, the journal shared (a re-fold's: module docs);
    /// `None` on a gap. The live turn's events go from it (the next job
    /// reads them again).
    pub fn journal(&mut self, input: &Input) -> Option<Arc<Vec<Event>>> {
        if !self.on(input) {
            return None;
        }
        self.live_to(0);
        self.over = None;
        self.take(input, input.history_len);
        Some(self.journal.clone())
    }

    /// The board of `input`, drawn ([`Drawn`]): its history read on from
    /// where the last job stopped (from the start if its epoch is another),
    /// a slice of at most [`SLICE`] events at a time, `go_on` asked after
    /// each; then the live turn's events, read from its log a slice at a
    /// time too, onto the overlay (the board at the history's end, read on
    /// from where the last job's stopped while what it read still holds);
    /// then the overlay's view and the bound prefix. `None` if `go_on` said
    /// stop (what was read is kept for the next job), on a gap, or if the
    /// live turn's log changed what `input` holds (a newer input stands).
    pub fn run(&mut self, input: &Input, go_on: &mut dyn FnMut() -> bool) -> Option<Drawn> {
        if !self.on(input) {
            return None;
        }
        if self.board.position() < input.history_len {
            self.live_to(0);
            self.over = None;
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
        // The overlay: another log's, or on another history, starts over.
        let history = input.history_len;
        let (log, want) = (input.live.log, input.live.len);
        if !self.over.as_ref().is_some_and(|o| o.log == log && o.history == history) {
            self.live_to(0);
            let board = self.board.clone();
            self.over = Some(Over { log, cuts: 0, history, board, read: 0, batch: self.batch, fates: Vec::new() });
        }
        loop {
            // A slice of the live turn's events from its log.
            {
                let s = input.live.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if s.kept(input.live.cuts, want) < want {
                    return None;
                }
                let o = self.over.as_mut().expect("the overlay");
                let have = self.journal.len() - self.hist;
                let kept = s.kept(o.cuts, have).min(want);
                o.cuts = s.cuts.len();
                if kept < o.read {
                    // What it read changed: it reads again from the history's end.
                    (o.board, o.read, o.batch) = (self.board.clone(), 0, self.batch);
                    o.fates.clear();
                }
                self.live_to(kept);
                let journal = Arc::make_mut(&mut self.journal);
                for k in kept..want.min(kept + COPY) {
                    let (e, row) = s.get(k).event();
                    self.bytes += event_bytes(&e);
                    journal.push(e);
                    self.rows.push(row);
                }
            }
            // Read on over what is copied, a slice at most.
            let o = self.over.as_mut().expect("the overlay");
            let have = self.journal.len() - self.hist;
            let to = have.min(o.read + SLICE);
            let events = &self.journal[..];
            for at in history + o.read..history + to {
                if let Some((start, end)) = o.batch.read(events, at) {
                    fates(events, start, end, false, |p, f| o.fates.push((p, f)));
                }
            }
            o.board.read(&events[..history + to]);
            o.read = to;
            if to == want {
                break;
            }
            if !go_on() {
                input.shared.bytes.store(self.bytes(), Ordering::Relaxed);
                return None;
            }
        }
        let o = self.over.as_ref().expect("the overlay");
        let events = &self.journal[..];
        let view = o.board.view(events);
        let bound = o.board.bound(events);
        // The fates of the live turn's batches and the open one's.
        let mut calls = o.fates.clone();
        fates(events, o.batch.start, events.len(), true, |p, f| calls.push((p, f)));
        let drawn = self.drawn(input, &view, &bound, &calls);
        input.shared.bytes.store(self.bytes(), Ordering::Relaxed);
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
            generation: Some(input.generation),
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
