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
//! a live event is the events it adds, never the turn so far, and no scan
//! or move of the calls before it: a call is placed in its slot (its rank
//! a Fenwick tree's when it came out of order), so an event touches
//! O(log n) elements at most, O(1) amortized in order ([`LiveLog::visits`]).
//! **The frame never waits on a job.** Under the log's lock a reader takes
//! nothing but whether its input holds and what it copied still holds
//! (two cut queries at most), and a batch of handles: at most [`COPY`],
//! stopped before the next handle's read could pass [`HOLD`] elements
//! touched (one handle at least). So one hold touches at most
//! `max(HOLD, P + G)` elements ([`LiveLog::most_held`], [`LiveLog::shape`]):
//! `P` the cut queries, `b(b + 1)/2` tree nodes each for `b = ⌊log₂ C⌋ + 1`
//! and `C` the log's cuts; `G` the most one handle's read touches, its
//! segment found among `S` and its slot's rank-select among the widest
//! segment's `N` slots, `⌊log₂ S⌋ + ⌊log₂ N⌋ + 4`. That is `HOLD` (1,024)
//! while the log has fewer than 2³⁰ cuts **and** fewer than 2³² slots, and
//! 4,290 at most on a 64-bit word, whatever the history; a test's 1,022
//! is an observed maximum under it, not the bound. These bounds count a
//! reader's operations under the lock (the tree nodes, slots and handles
//! it touches), not the mutex's wall-clock hold: allocation, hashing,
//! scheduling and destructors are not in them, and they are the worker's
//! and an input's reads, not every method that takes the lock (a log's
//! copy copies its store). A writer (a fact) touches O(log n) at most.
//! A reader makes the handles events, cuts what no longer holds (a slice
//! at a time, `go_on` asked between) and copies the board after letting
//! go of the lock. A job reads the overlay on from where the last job
//! stopped while what it read holds.
//!
//! **Nothing is destroyed on the frame while the disposer has room**
//! ([`dispose`]): what a log lets go (a round's calls, ids and rows at a
//! seal or a user message; a reset's round, facts and results as one; a
//! log replaced or gone, its rounds with its store; a store when its last
//! job lets go) is handed in O(1) to its bin, the app's own disposer's
//! (one thread, owned, shut down and joined with the app), charged with
//! all it keeps, inputs included, until the disposer has dropped the last
//! of it. Into an empty queue any unit is admitted. With the queue
//! saturated ([`dispose::MAX_ITEMS`] items, or [`dispose::MAX_BYTES`]
//! bytes with an item queued), closed or never started, whoever lets go
//! drops it in place (counted), the contract: one unit's destruction on
//! that event, never unbounded memory, never a wait on the disposer, and
//! never under the store's lock (what is let go under it is dropped once
//! it is released). Everything counted is counted with its heap ([`event_bytes`])
//! and its room as allocated: staged, journal, the live log, the overlay,
//! the card folds (the board's and the overlay's, each its own: read again
//! after whatever can change them, a card call or, with a card in them, a
//! result or a user's message, and whenever a job ends or stops, before
//! its count is published), and the board published ([`Drawn::bytes`]). A
//! call's input is one allocation, its row's: the same input again keeps
//! it ([`LiveLog::call`]). **The worker is the history's** ([`HistoryBlocks::worker`]): what
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
//! [`SLICE`] events at a time, `go_on` asked between slices (and between
//! slices of a cut); a stopped job keeps what it read, and what it cut. B2's `read` of a slice, its `view` and `bound`
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
use cards_spec::board::{Board, Event, Place, State, Visibility};
use cards_spec::fold::{self, NAMES as CARD_NAMES};
use serde_json::Value;

pub use cards_spec::block::{BlockId, Body, Kind, Ref, Tag};
pub use cards_spec::board::{Block, Logged, Record, Tombstone};

use crate::transcript::{Item, ItemKind};

pub mod dispose;

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
                    let input = if kept { canonical(&b["input"]) } else { Value::Null };
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

/// `v` as an input is kept (Shady #101 R8-1): rebuilt whole, so what it
/// holds is what [`value_bytes`] charges, whatever made `v` (a parse, a
/// clone, an insert). Each string at its length; each array at its length;
/// each map built in one pass at the room a clone of it has: under
/// `IndexMap`, made with as many entries' room as its index table holds
/// (`index_layout`), then filled, so it never grows (an insert reserves
/// before it looks a key up: a duplicate key on the wire or a key set
/// again grows a full table, its length the same, and a clone then copies
/// that table and reserves entries to its capacity); under `BTreeMap`,
/// built in bulk from its sorted entries (`FromIterator`: full nodes, the
/// right border topped up), and a clone copies it node for node.
///
/// So the layout is a clone's fixed point: a clone of a canonical value is
/// laid out as it is, and charged alike. Every input a row, a call or an
/// event keeps is canonical, made here at the one place it is first kept
/// (a history's call, its row's, a live call's row's: one each), then
/// shared (an `Arc`) or cloned, never changed (nothing writes a kept
/// input: [`value_bytes`]). It costs about what the clone it replaces did
/// (its maps' keys are hashed again, where an `IndexMap`'s clone copies
/// its table): 1.24× a clone on the demo's inputs, 0.23 µs more each.
pub fn canonical(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.as_str().into()),
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        Value::Object(m) if ordered_maps() => {
            let mut out = serde_json::Map::with_capacity(index_layout(m.len()).1);
            for (k, v) in m {
                out.insert(k.as_str().into(), canonical(v));
            }
            Value::Object(out)
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.as_str().into(), canonical(v))).collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) => v.clone(),
    }
}

/// What JSON value `v` holds on the heap (bytes, as asked of the allocator:
/// the sizes of its blocks' layouts), `v` canonical ([`canonical`]) or a
/// clone of one: exactly. Its strings' and arrays' room is read (their
/// capacities); a map's can't be (`serde_json::Map` has no capacity), so
/// it is `map_bytes` for its length, the room [`canonical`] builds it
/// with, then its keys' and values' own. The allocator's own bookkeeping
/// per block is not counted, as nowhere in the app.
///
/// Every value charged here is canonical or a clone of one, and is never
/// changed once kept: a row's input, a live call's and its fact's are one
/// `Arc`; a history's call event and the journal's events are written
/// once and only read (cloned, as a whole journal is when a reader holds
/// it; truncated; dropped). Another map (parsed, or set again after a
/// clone) can hold more than its length says, and is not charged here.
/// `tests/json_memory.rs` checks it on a counting allocator, both map
/// backends: every shape, canonical and cloned, exactly; maps at every
/// length to 300; and the inputs that defeated the length alone
/// (duplicate keys at each table's capacity, a key set again after a
/// clone), off the wire and through an attach.
pub fn value_bytes(v: &Value) -> usize {
    match v {
        Value::String(s) => s.capacity(),
        Value::Array(a) => a.capacity() * std::mem::size_of::<Value>() + a.iter().map(value_bytes).sum::<usize>(),
        Value::Object(m) => map_bytes(m.len()) + m.iter().map(|(k, v)| k.capacity() + value_bytes(v)).sum::<usize>(),
        _ => 0,
    }
}

/// What a kept input holds (bytes, heap included: [`Item::kept_bytes`],
/// a live call's): its `Arc`'s block (two counts and the value), then
/// what the value holds ([`value_bytes`]: canonical, [`canonical`]).
pub fn kept_bytes(v: &Value) -> usize {
    /// `Arc<Value>`'s block (std's `ArcInner`): its strong and weak counts.
    #[allow(dead_code)]
    struct ArcBlock(usize, usize, Value);
    std::mem::size_of::<ArcBlock>() + value_bytes(v)
}

/// serde_json's maps keep their order (its `preserve_order`, the desk's
/// through GPUI): an `IndexMap` each. If not (the client built alone), a
/// `BTreeMap`. Asked of serde_json itself, once.
fn ordered_maps() -> bool {
    static ORDERED: LazyLock<bool> = LazyLock::new(|| {
        let mut m = serde_json::Map::new();
        m.insert("b".into(), Value::Null);
        m.insert("a".into(), Value::Null);
        m.keys().next().is_some_and(|k| k == "b")
    });
    *ORDERED
}

/// A canonical `IndexMap` of `n` entries ([`canonical`]; a clone of one
/// alike): its index table's buckets, and its entries' room, the table's
/// capacity (indexmap 2.14.2, hashbrown 0.17.1). The buckets are
/// hashbrown's fewest for `n` (`raw.rs` `capacity_to_buckets`, its
/// minimum of 3 for a `usize` index: 4); the capacity, all of them but
/// one under 8, else 7 in 8 (`bucket_mask_to_capacity`). A table made for
/// that capacity has those buckets again, and a clone copies the table,
/// then reserves its entries to its capacity (`inner.rs`
/// `Core::clone_from`, `reserve_entries`).
fn index_layout(n: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let buckets = match n {
        0..4 => 4,
        4..8 => 8,
        8..15 => 16,
        _ => (n * 8 / 7).next_power_of_two(),
    };
    (buckets, if buckets < 8 { buckets - 1 } else { buckets / 8 * 7 })
}

/// A canonical JSON map's storage for `n` entries ([`canonical`]; a clone
/// of one alike), its keys' and values' own heap not included (bytes, as
/// allocated). The locked crates' layouts, mirrored below so the compiler
/// sizes them:
/// - **`IndexMap`**: its entries, as many as its table holds
///   ([`index_layout`]), each its hash, key and value (`Bucket`); its
///   table, a `usize` per bucket, then a control byte per bucket and a
///   group's more (`TableLayout::calculate_layout_for`; the group is 16
///   bytes with SSE2, 8 otherwise: 16 is charged).
/// - **`BTreeMap`** (std): its nodes, built in bulk (`bulk_push`): each
///   leaf takes 11 entries, then the next goes up a level as a separator
///   and a new leaf (and a node at each level between) starts; a level's
///   nodes fill alike. So a map of `n` has `n / 12 + 1` leaves, and each
///   level above `(m - 1) / 12 + 1` nodes for the `m` below it, up to one
///   root; topping up the right border moves entries, never a node.
///
/// `tests/json_memory.rs` measures both against a counting allocator:
/// equal for every `n` to 300, canonical and cloned (x86-64's 16-byte
/// group). The layouts are the audited versions', held by `Cargo.lock`
/// and the pinned toolchain; nothing checks them at run time. A change of
/// either needs that evidence again: a drift in a crate's layout, or in
/// std's, fails those tests (CI runs both backends).
fn map_bytes(n: usize) -> usize {
    use std::mem::{MaybeUninit, size_of};
    use std::ptr::NonNull;
    /// indexmap's entry.
    #[allow(dead_code)]
    struct Bucket(usize, String, Value);
    /// std's `LeafNode<String, Value>` and `InternalNode`.
    #[allow(dead_code)]
    struct Leaf {
        parent: Option<NonNull<u8>>,
        parent_idx: MaybeUninit<u16>,
        len: u16,
        keys: [MaybeUninit<String>; 11],
        vals: [MaybeUninit<Value>; 11],
    }
    #[allow(dead_code)]
    struct Internal(Leaf, [MaybeUninit<NonNull<u8>>; 12]);
    const GROUP: usize = 16;
    if n == 0 {
        return 0;
    }
    if !ordered_maps() {
        let mut nodes = n / 12 + 1;
        let mut bytes = nodes * size_of::<Leaf>();
        while nodes > 1 {
            nodes = (nodes - 1) / 12 + 1;
            bytes += nodes * size_of::<Internal>();
        }
        return bytes;
    }
    let (buckets, entries) = index_layout(n);
    entries * size_of::<Bucket>() + (buckets * size_of::<usize>()).next_multiple_of(GROUP) + buckets + GROUP
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
/// made an event's only when a job reads it, off the frame and off the
/// log's lock (a reader takes [`Handle`]s under it, nothing more).
#[derive(Clone, Debug)]
enum Fact {
    /// `size`: its input's bytes, heap included, as its row counts them
    /// ([`Item::kept_bytes`]): counted with what a reset retires.
    Call {
        tool_id: String,
        name: String,
        input: Option<Arc<Value>>,
        row: u64,
        size: usize,
    },
    Result {
        tool_id: String,
        text: String,
    },
    User {
        text: String,
        row: Option<u64>,
    },
    End,
}

/// A fact as the log holds it: shared, so a reader takes one under the
/// log's lock for the cost of a pointer.
type Handle = Arc<Fact>;

static END: LazyLock<Handle> = LazyLock::new(|| Arc::new(Fact::End));

impl Fact {
    /// The board's event and its row.
    fn event(&self) -> (Event, Option<u64>) {
        match self {
            Fact::Call { tool_id, name, input, row, .. } => {
                // A clone of the row's canonical input: laid out as it is.
                let input = input.as_deref().cloned().unwrap_or(Value::Null);
                (Event::Call { tool_id: tool_id.clone(), name: name.clone(), input }, Some(*row))
            }
            Fact::Result { tool_id, text } => (Event::Result { tool_id: tool_id.clone(), text: text.clone() }, None),
            Fact::User { text, row } => (Event::User { text: text.clone() }, *row),
            Fact::End => (Event::ResultsEnd, None),
        }
    }

    /// What it keeps of its own (bytes, heap included, its handle's count
    /// too; a call's input is its row's).
    fn bytes(&self) -> usize {
        2 * std::mem::size_of::<usize>()
            + std::mem::size_of::<Fact>()
            + match self {
                Fact::Call { tool_id, name, .. } => tool_id.capacity() + name.capacity(),
                Fact::Result { tool_id, text } => tool_id.capacity() + text.capacity(),
                Fact::User { text, .. } => text.capacity(),
                Fact::End => 0,
            }
    }

    /// Its input's bytes, heap included (its row's while the row is).
    fn input_bytes(&self) -> usize {
        match self {
            Fact::Call { size, .. } => *size,
            _ => 0,
        }
    }
}

/// Elements read or written under a log's lock ([`Store::touched`]): the
/// frame's work per fact, and how long a reader holds the lock.
#[derive(Debug, Default)]
struct Touched {
    now: std::cell::Cell<usize>,
}

impl Touched {
    fn add(&self, n: usize) {
        self.now.set(self.now.get() + n);
    }
}

/// Which of a segment's slots hold a fact, as a Fenwick tree (one node a
/// slot): a fact's rank among them, and the slot of the `j`th, each in
/// O(log n) nodes touched. A slot added at the end touches its node's
/// children (O(1) amortized, O(log n) at most); the last slot filled, one
/// node.
#[derive(Clone, Debug, Default)]
struct Ranks {
    tree: Vec<u32>,
}

impl Ranks {
    /// Facts in slots `[0, k)`.
    fn prefix(&self, k: usize, t: &Touched) -> usize {
        let (mut i, mut n) = (k, 0);
        while i > 0 {
            n += self.tree[i - 1] as usize;
            i &= i - 1;
            t.add(1);
        }
        n
    }

    /// A slot at the end (`one`: a fact in it).
    fn push(&mut self, one: bool, t: &Touched) {
        let i = self.tree.len() + 1;
        let low = i.isolate_lowest_one();
        let (mut v, mut step) = (u32::from(one), 1);
        while step < low {
            v += self.tree[i - step - 1];
            step <<= 1;
            t.add(1);
        }
        self.tree.push(v);
    }

    /// Slot `k` gets a fact.
    fn add(&mut self, k: usize, t: &Touched) {
        let mut i = k + 1;
        while i <= self.tree.len() {
            self.tree[i - 1] += 1;
            i += i.isolate_lowest_one();
            t.add(1);
        }
    }

    /// The slot of the `j`th fact (from 0).
    fn select(&self, j: usize, t: &Touched) -> usize {
        let n = self.tree.len();
        let (mut at, mut left) = (0, j as u32 + 1);
        let mut step = if n == 0 { 0 } else { 1 << (usize::BITS - 1 - n.leading_zeros()) };
        while step > 0 {
            if at + step <= n && self.tree[at + step - 1] < left {
                at += step;
                left -= self.tree[at - 1];
            }
            step >>= 1;
            t.add(1);
        }
        at
    }
}

/// A run of the live turn's events: slots in model order, a slot empty
/// for a call whose input still streams (the open round's calls, by the
/// round's index: a call whole out of order fills its slot, and nothing
/// moves). Until a fact is past an empty slot (`gaps`) its first `len`
/// slots are its facts, a fact's rank its slot; from then on its
/// [`Ranks`] tell, kept from its first slot on.
#[derive(Clone, Debug, Default)]
struct Seg {
    slots: Vec<Option<Handle>>,
    len: usize,
    ranks: Ranks,
    gaps: bool,
    /// Its facts' bytes ([`Fact::bytes`]), and their inputs' apart
    /// ([`Fact::input_bytes`]).
    bytes: usize,
    inputs: usize,
}

impl Seg {
    fn get(&self, k: usize, t: &Touched) -> &Handle {
        t.add(1);
        let at = if self.gaps { self.ranks.select(k, t) } else { k };
        self.slots[at].as_ref().expect("a fact")
    }

    /// Slot `k`'s fact's rank: the facts before it.
    fn rank(&self, k: usize, t: &Touched) -> usize {
        if self.gaps { self.ranks.prefix(k, t) } else { k }
    }

    /// A slot at the end: fact `f`, or empty.
    fn push(&mut self, f: Option<Handle>, t: &Touched) {
        t.add(1);
        let one = f.is_some();
        self.bytes += f.as_deref().map_or(0, Fact::bytes);
        self.inputs += f.as_deref().map_or(0, Fact::input_bytes);
        self.gaps |= one && self.len < self.slots.len();
        self.ranks.push(one, t);
        self.slots.push(f);
        self.len += usize::from(one);
    }

    /// Empty slot `k` gets fact `f`: its rank.
    fn fill(&mut self, k: usize, f: Handle, t: &Touched) -> usize {
        t.add(1);
        self.bytes += f.bytes();
        self.inputs += f.input_bytes();
        debug_assert!(self.slots[k].is_none());
        self.slots[k] = Some(f);
        self.gaps |= k != self.len;
        self.len += 1;
        self.ranks.add(k, t);
        self.rank(k, t)
    }

    /// Slot `k`'s fact is `f` now: its rank. The one before goes off the
    /// frame ([`dispose`]), charged with its input.
    fn set(&mut self, k: usize, f: Handle, t: &Touched, bin: &dispose::Bin) -> usize {
        t.add(1);
        (self.bytes, self.inputs) = (self.bytes + f.bytes(), self.inputs + f.input_bytes());
        let old = self.slots[k].replace(f).expect("a fact");
        let (own, input) = (old.bytes(), old.input_bytes());
        (self.bytes, self.inputs) = (self.bytes - own, self.inputs - input);
        bin.retire(old, 1, own + input);
        self.rank(k, t)
    }

    /// What its slots and ranks hold besides the facts (bytes, as
    /// allocated).
    fn spare(&self) -> usize {
        self.slots.capacity() * std::mem::size_of::<Option<Handle>>() + self.ranks.tree.capacity() * std::mem::size_of::<u32>()
    }

    /// What it holds, retired (bytes, heap included, as allocated): its
    /// facts with their inputs (their rows' no longer), its slots and
    /// ranks.
    fn charge(&self) -> usize {
        self.bytes + self.inputs + self.spare()
    }

    /// Its allocations, walked one by one (tests: [`LiveLog::retirement`]).
    fn walk(&self) -> usize {
        let facts: usize = self.slots.iter().flatten().map(|f| f.bytes() + f.input_bytes()).sum();
        self.slots.capacity() * std::mem::size_of::<Option<Handle>>() + self.ranks.tree.capacity() * std::mem::size_of::<u32>() + facts
    }

    /// Retired whole ([`dispose`]), charged as it holds.
    fn retire(self, bin: &dispose::Bin) {
        let (n, bytes) = (self.slots.len(), self.charge());
        bin.retire(self, n, bytes);
    }
}

/// A log's cuts ([`Store`]), in order, and the lowest of each run of them
/// a Fenwick tree's node spans: a cut adds its node (O(log n) nodes read
/// at most, O(1) amortized), and the lowest since any cut is read from
/// O(log² n) of them, none let go.
#[derive(Debug, Default)]
struct Cuts {
    at: Vec<usize>,
    low: Vec<usize>,
}

impl Cuts {
    fn len(&self) -> usize {
        self.at.len()
    }

    fn push(&mut self, at: usize, t: &Touched) {
        let i = self.at.len() + 1;
        let span = i.isolate_lowest_one();
        let (mut low, mut step) = (at, 1);
        while step < span {
            low = low.min(self.low[i - step - 1]);
            step <<= 1;
            t.add(1);
        }
        self.at.push(at);
        self.low.push(low);
        t.add(1);
    }

    /// The lowest cut since the first `from`.
    fn since(&self, from: usize, t: &Touched) -> Option<usize> {
        let (mut i, mut low) = (self.at.len(), None::<usize>);
        while i > from {
            let start = i - i.isolate_lowest_one();
            let v = if start >= from {
                let v = self.low[i - 1];
                i = start;
                v
            } else {
                i -= 1;
                self.at[i]
            };
            low = Some(low.map_or(v, |l| l.min(v)));
            t.add(1);
        }
        low
    }
}

/// The live turn's events in order (module docs): those before the open
/// round, then the open round's (its leading `ResultsEnd`, its whole calls
/// in model order, its results, its `ResultsEnd`). The one copy: the
/// transcript adds to it a fact at a time ([`LiveLog`]), and a job reads
/// it through its cursor, a batch of handles at a time, off the frame. A
/// change other than an event added at its end (a reset, a call whole
/// after its round's results, a call's input again) is a cut, at the
/// lowest place it changed: a cursor holds for every cut past its length.
/// Nothing under its lock costs more than a fact's events, or a reader's
/// batch: O(1) amortized, and O(log n) nodes for a call whole out of
/// order or a cut added ([`Store::touched`] counts them). What it lets go
/// (a replaced fact, an empty segment, itself) goes off the frame
/// ([`dispose`]).
#[derive(Debug, Default)]
struct Store {
    /// The events before the open round, in segments (a sealed round's
    /// calls and results are moved in whole: a seal moves no event), and
    /// where each starts; their facts' bytes and inputs' ([`Seg`]), their
    /// slots, and the most slots one has.
    done: Vec<Seg>,
    starts: Vec<usize>,
    done_len: usize,
    done_bytes: usize,
    done_inputs: usize,
    done_slots: usize,
    /// Their slots' and ranks' room ([`Seg::spare`]), kept as they grow.
    done_spare: usize,
    widest: usize,
    /// The last segment takes single events (a sealed round's don't).
    loose: bool,
    lead: bool,
    calls: Seg,
    results: Seg,
    end: bool,
    cuts: Cuts,
    /// Elements read or written under its lock: in all, and the most in
    /// one hold ([`Held`]).
    touched: Touched,
    visits: usize,
    most: usize,
    /// Where what it lets go goes (its log's), and, its log gone while a
    /// job still reads it, its charge there until it goes too.
    bin: dispose::Bin,
    charge: Option<dispose::Charge>,
}

/// A log's store, locked: what it touched counted when it lets go. What
/// is dropped in place meanwhile ([`dispose::locking`], a field after the
/// guard: it goes once the lock is released) is dropped after.
struct Held<'a>(std::sync::MutexGuard<'a, Store>, dispose::Locked);

impl std::ops::Deref for Held<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.0
    }
}

impl std::ops::DerefMut for Held<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.0
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let s = &mut *self.0;
        let n = s.touched.now.replace(0);
        s.visits += n;
        s.most = s.most.max(n);
    }
}

fn hold(store: &Mutex<Store>) -> Held<'_> {
    let locking = dispose::locking();
    Held(store.lock().unwrap_or_else(std::sync::PoisonError::into_inner), locking)
}

impl Store {
    fn in_bin(bin: dispose::Bin) -> Store {
        let mut s = Store::default();
        s.bin = bin;
        s
    }

    fn len(&self) -> usize {
        self.done_len + usize::from(self.lead) + self.calls.len + self.results.len + usize::from(self.end)
    }

    /// Its `k`th event.
    fn get(&self, mut k: usize) -> &Handle {
        let t = &self.touched;
        if k < self.done_len {
            t.add(usize::BITS as usize - self.starts.len().leading_zeros() as usize + 1);
            let seg = self.starts.partition_point(|&s| s <= k) - 1;
            return self.done[seg].get(k - self.starts[seg], t);
        }
        k -= self.done_len;
        if self.lead {
            if k == 0 {
                return &END;
            }
            k -= 1;
        }
        if k < self.calls.len {
            return self.calls.get(k, t);
        }
        k -= self.calls.len;
        if k < self.results.len {
            return self.results.get(k, t);
        }
        &END
    }

    /// The handles of its events `from..to`: at most [`COPY`], and as
    /// many as keep this hold of its lock within [`HOLD`] elements touched
    /// (one at least), each read touching [`Store::get_most`] at most.
    fn batch(&self, from: usize, to: usize) -> Vec<Handle> {
        let most = self.get_most();
        let mut batch = Vec::with_capacity(COPY.min(to.saturating_sub(from)));
        for k in from..to {
            if batch.len() == COPY || (!batch.is_empty() && self.touched.now.get() + most > HOLD) {
                break;
            }
            batch.push(self.get(k).clone());
        }
        batch
    }

    /// How many of its first `len` events still hold for a reader that
    /// saw its first `cuts` cuts.
    fn kept(&self, cuts: usize, len: usize) -> usize {
        self.cuts.since(cuts, &self.touched).map_or(len, |c| c.min(len))
    }

    /// A cut at `at`.
    fn cut(&mut self, at: usize) {
        self.cuts.push(at, &self.touched);
    }

    /// Its facts' bytes ([`Fact::bytes`]).
    fn fact_bytes(&self) -> usize {
        self.done_bytes + self.calls.bytes + self.results.bytes
    }

    /// What it keeps (bytes, heap included; the calls' inputs are their
    /// rows'): its facts, and its slots, ranks and cuts as allocated. O(1):
    /// the sealed segments' room is kept as they grow.
    fn bytes(&self) -> usize {
        std::mem::size_of::<Store>() + self.fact_bytes() + self.done_spare + self.calls.spare() + self.results.spare() + self.rooms()
    }

    /// Its vectors' own room: the segments', the starts' and the cuts'.
    fn rooms(&self) -> usize {
        self.done.capacity() * std::mem::size_of::<Seg>()
            + self.starts.capacity() * std::mem::size_of::<usize>()
            + (self.cuts.at.capacity() + self.cuts.low.capacity()) * std::mem::size_of::<usize>()
    }

    /// What its retirement moves ([`Store`]'s drop), charged (bytes, heap
    /// included, as allocated): every segment with its facts' inputs
    /// (their rows' no longer), slots and ranks, and its vectors' room.
    /// O(1).
    fn moved(&self) -> usize {
        self.done_bytes + self.done_inputs + self.done_spare + self.calls.charge() + self.results.charge() + self.rooms()
    }

    /// The same, its allocations walked one by one (tests).
    fn walk(&self) -> usize {
        self.done.iter().map(Seg::walk).sum::<usize>() + self.calls.walk() + self.results.walk() + self.rooms()
    }

    /// The elements its retirement destroys: segments and slots.
    fn elements(&self) -> usize {
        self.done.len() + self.done_slots + self.calls.slots.len() + self.results.slots.len()
    }

    /// The most one handle's read ([`Store::get`]) touches: its segment
    /// found among the segments, then its slot's rank-select in the widest
    /// segment's slots.
    fn get_most(&self) -> usize {
        let bits = |n: usize| (usize::BITS - n.leading_zeros()) as usize;
        let widest = self.widest.max(self.calls.slots.len()).max(self.results.slots.len());
        bits(self.starts.len()) + 2 + bits(widest)
    }

    /// Event `f` at the end of the events before the open round.
    fn push_done(&mut self, f: Fact) {
        if !self.loose {
            self.starts.push(self.done_len);
            self.done.push(Seg::default());
            self.loose = true;
        }
        let f = Arc::new(f);
        (self.done_bytes, self.done_inputs) = (self.done_bytes + f.bytes(), self.done_inputs + f.input_bytes());
        let last = self.done.last_mut().expect("a loose segment");
        let room = last.spare();
        last.push(Some(f), &self.touched);
        self.done_spare = self.done_spare + last.spare() - room;
        self.widest = self.widest.max(last.slots.len());
        self.done_len += 1;
        self.done_slots += 1;
    }

    /// Events `s` at the end of the events before the open round, moved in
    /// whole (with no event, its empty slots go off the frame).
    fn append_done(&mut self, s: Seg) {
        if s.len == 0 {
            s.retire(&self.bin);
            return;
        }
        self.touched.add(1);
        self.done_spare += s.spare();
        self.starts.push(self.done_len);
        self.done_len += s.len;
        (self.done_bytes, self.done_inputs) = (self.done_bytes + s.bytes, self.done_inputs + s.inputs);
        self.done_slots += s.slots.len();
        self.widest = self.widest.max(s.slots.len());
        self.done.push(s);
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

/// A store's facts go off the frame ([`dispose`]): its last owner lets go
/// of it in O(1), and the disposer destroys all it moves, charged as
/// [`Store::moved`] counts it (a store its log left to a job, by the charge
/// its log took for it then).
impl Drop for Store {
    fn drop(&mut self) {
        let (elements, bytes) = (self.elements(), self.moved());
        let parts = (
            std::mem::take(&mut self.done),
            std::mem::take(&mut self.starts),
            std::mem::take(&mut self.calls),
            std::mem::take(&mut self.results),
            std::mem::take(&mut self.cuts),
        );
        match self.charge.take() {
            Some(held) => self.bin.retire_charged(parts, elements, held),
            None => self.bin.retire(parts, elements, bytes),
        }
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
/// the history's last message's calls that reported. What it lets go (a
/// round; a reset's round, facts and results as one; itself, with its
/// store) goes to its bin ([`dispose`]): no event destroys a round's
/// calls, nor a log replaced its turn's, while the disposer has room.
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
    head_answered: Ids,
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
    /// Where what it lets go goes ([`dispose`]): the bin its thread had
    /// when it was made, or its predecessor's ([`LiveLog::renewed`]).
    bin: dispose::Bin,
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
    /// Its strings' room (its calls' tool ids and names, its ids' keys),
    /// kept as they come.
    strs: usize,
    /// Its calls' inputs' bytes ([`LiveCall::size`]), kept as they come:
    /// what it is charged with besides when it is retired.
    inputs: usize,
    /// Where it goes ([`dispose`]): its log's.
    bin: dispose::Bin,
}

/// A hash table's allocation for `capacity` entries of `(K, V)` (bytes, as
/// allocated: hashbrown's buckets, `capacity` 7/8 of them past 8, each an
/// entry and a control byte, and a group's 16 control bytes more).
fn table<K, V>(capacity: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    let buckets = if capacity < 8 { (capacity + 1).next_power_of_two() } else { (capacity * 8 / 7).next_power_of_two() };
    buckets * (std::mem::size_of::<(K, V)>() + 1) + 16
}

impl Round {
    fn new(bin: dispose::Bin) -> Round {
        let mut r = Round::default();
        r.bin = bin;
        r
    }

    /// What it keeps (bytes, heap included, as allocated; an input is its
    /// row's): its calls' vector and strings, its two tables and their
    /// keys. O(1).
    fn bytes(&self) -> usize {
        self.calls.capacity() * std::mem::size_of::<LiveCall>()
            + self.strs
            + table::<String, usize>(self.ids.capacity())
            + table::<u64, usize>(self.rows.capacity())
    }

    /// What it is charged with retired (bytes): all it keeps, its calls'
    /// inputs too (a row, a fact may keep one as well: counted again,
    /// never under). O(1).
    fn retired(&self) -> usize {
        self.bytes() + self.inputs
    }

    /// The same, its strings and inputs walked one by one (tests).
    fn walk(&self) -> usize {
        let strs: usize = self.calls.iter().map(|c| c.tool_id.capacity() + c.name.capacity()).sum::<usize>()
            + self.ids.keys().map(String::capacity).sum::<usize>();
        let inputs: usize = self.calls.iter().filter_map(|c| c.input.as_deref()).map(kept_bytes).sum();
        self.calls.capacity() * std::mem::size_of::<LiveCall>()
            + strs
            + inputs
            + table::<String, usize>(self.ids.capacity())
            + table::<u64, usize>(self.rows.capacity())
    }

    /// The elements its drop destroys.
    fn elements(&self) -> usize {
        self.calls.len() + self.ids.len() + self.rows.len()
    }

    fn complete(&self) -> bool {
        !self.calls.is_empty() && self.done == self.calls.len()
    }

    /// Call `c` at its end, row `row`'s: its index.
    fn push(&mut self, c: LiveCall, row: u64) -> usize {
        let k = self.calls.len();
        self.strs += c.tool_id.capacity() + c.name.capacity();
        self.inputs += c.size;
        self.id(c.tool_id.clone(), k);
        self.rows.insert(row, k);
        self.calls.push(c);
        k
    }

    /// Tool id `id`'s latest call is `k`.
    fn id(&mut self, id: String, k: usize) {
        let cap = id.capacity();
        if self.ids.insert(id, k).is_none() {
            self.strs += cap;
        }
    }

    /// Call `k`'s tool id and name are these.
    fn rename(&mut self, k: usize, tool_id: &str, name: &str) {
        if self.calls[k].tool_id != tool_id {
            let old = std::mem::replace(&mut self.calls[k].tool_id, tool_id.to_string());
            self.strs = self.strs + self.calls[k].tool_id.capacity() - old.capacity();
            self.id(tool_id.to_string(), k);
        }
        if self.calls[k].name != name {
            let old = std::mem::replace(&mut self.calls[k].name, name.to_string());
            self.strs = self.strs + self.calls[k].name.capacity() - old.capacity();
        }
    }

    /// Call `k`'s input is `input`, of `size` bytes: the one before, given
    /// back (its caller drops it after the store's lock).
    fn reinput(&mut self, k: usize, input: Option<Arc<Value>>, size: usize) -> Option<Arc<Value>> {
        let c = &mut self.calls[k];
        self.inputs = self.inputs + size - c.size;
        c.size = size;
        std::mem::replace(&mut c.input, input)
    }
}

/// A round's calls, ids and rows go off the frame ([`dispose`]): whatever
/// lets go of a round alone (a seal, a user message) does in O(1), the
/// round charged with its calls' inputs (it keeps them whatever else
/// goes). A reset's round goes with its facts, a log's with its log
/// ([`LiveLog`]).
impl Drop for Round {
    fn drop(&mut self) {
        let (elements, bytes) = (self.elements(), self.retired());
        let parts = (std::mem::take(&mut self.calls), std::mem::take(&mut self.ids), std::mem::take(&mut self.rows));
        self.bin.retire(parts, elements, bytes);
    }
}

/// The ids of the history's last message's calls that reported: let go off
/// the frame ([`dispose`]).
#[derive(Clone, Debug, Default)]
struct Ids {
    set: HashSet<String>,
    /// Their room, kept as they come.
    keys: usize,
    bin: dispose::Bin,
}

impl Ids {
    fn new(bin: dispose::Bin) -> Ids {
        Ids { set: HashSet::new(), keys: 0, bin }
    }

    /// `id` reported: `false` if it had.
    fn insert(&mut self, id: &str) -> bool {
        let id = id.to_string();
        let cap = id.capacity();
        let new = self.set.insert(id);
        if new {
            self.keys += cap;
        }
        new
    }

    fn len(&self) -> usize {
        self.set.len()
    }

    /// What it keeps (bytes, heap included, as allocated). O(1).
    fn bytes(&self) -> usize {
        self.keys + table::<String, ()>(self.set.capacity())
    }

    /// The same, walked (tests).
    fn walk(&self) -> usize {
        self.set.iter().map(String::capacity).sum::<usize>() + table::<String, ()>(self.set.capacity())
    }
}

impl Drop for Ids {
    fn drop(&mut self) {
        let (n, bytes) = (self.set.len(), self.bytes());
        self.bin.retire(std::mem::take(&mut self.set), n, bytes);
    }
}

#[derive(Clone, Debug)]
struct LiveCall {
    tool_id: String,
    name: String,
    /// A card's or block's input, whole ([`keeps_input`]): its row's.
    input: Option<Arc<Value>>,
    /// Its input's bytes, heap included ([`kept_bytes`]; none, 0).
    size: usize,
    /// Its input is whole (`ToolUse`).
    whole: bool,
    /// Its final result came.
    answered: bool,
}

/// A log made on a thread takes its bin ([`dispose::current`]).
impl Default for LiveLog {
    fn default() -> Self {
        LiveLog::in_bin(dispose::current())
    }
}

impl LiveLog {
    fn in_bin(bin: dispose::Bin) -> LiveLog {
        static IDS: AtomicU64 = AtomicU64::new(1);
        LiveLog {
            store: Arc::new(Mutex::new(Store::in_bin(bin.clone()))),
            id: IDS.fetch_add(1, Ordering::Relaxed),
            version: 0,
            head: None,
            head_answered: Ids::new(bin.clone()),
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
            bin,
        }
    }

    /// A new log in its bin (one replacing it).
    pub fn renewed(&self) -> LiveLog {
        LiveLog::in_bin(self.bin.clone())
    }
}

/// A copy is another log, of the same facts, in the same bin.
impl Clone for LiveLog {
    fn clone(&self) -> Self {
        let s = hold(&self.store);
        let mut store = Store::in_bin(self.bin.clone());
        (store.done, store.starts, store.calls, store.results) = (s.done.clone(), s.starts.clone(), s.calls.clone(), s.results.clone());
        (store.done_len, store.done_bytes, store.done_inputs, store.done_slots) = (s.done_len, s.done_bytes, s.done_inputs, s.done_slots);
        (store.widest, store.loose, store.lead, store.end) = (s.widest, s.loose, s.lead, s.end);
        // A copy's room is its own (a vector copied has no room to spare).
        store.done_spare = store.done.iter().map(Seg::spare).sum();
        let mut l = self.renewed();
        (l.version, l.head, l.head_ended, l.after, l.target, l.before) =
            (self.version, self.head, self.head_ended, self.after, self.target, self.before);
        (l.empty, l.blocks, l.work, l.first) = (self.empty, self.blocks, self.work, self.first);
        l.store = Arc::new(Mutex::new(store));
        l.head_answered = self.head_answered.clone();
        (l.open, l.sealed) = (self.open.clone(), self.sealed.clone());
        l
    }
}

/// A log goes as one unit ([`dispose`]): its rounds (with their calls'
/// inputs) and the head's ids, with its store when no job reads it (none
/// can start now), moved into the unit (its field left an empty store's:
/// no handle to it outlives the unit), so no owner of a call's input
/// outlives the charge that covers it. A store a job still reads stays the
/// job's, charged from now on ([`dispose::Bin::charge`], what
/// [`Store::moved`] counts) until its last reader lets go of it.
impl Drop for LiveLog {
    fn drop(&mut self) {
        let (open, sealed, head) = (self.open.take(), self.sealed.take(), std::mem::take(&mut self.head_answered));
        let (mut elements, mut bytes) = (head.len(), head.bytes());
        for r in [&open, &sealed].into_iter().flatten() {
            (elements, bytes) = (elements + r.elements(), bytes + r.retired());
        }
        let store = if Arc::strong_count(&self.store) == 1 {
            let s = self.lock();
            (elements, bytes) = (elements + s.elements(), bytes + s.moved());
            drop(s);
            Some(std::mem::take(&mut self.store))
        } else {
            let mut s = self.lock();
            if s.charge.is_none() {
                s.charge = Some(self.bin.charge(s.moved()));
            }
            None
        };
        self.bin.retire((store, open, sealed, head), elements, bytes);
    }
}

/// What a log's drop hands over, with its store (tests).
type LogUnit = (Option<Arc<Mutex<Store>>>, Option<Round>, Option<Round>, Ids);

/// Tests: a log's store's lock, probed ([`LiveLog::lock_probe`]).
#[cfg(any(test, feature = "test-support"))]
pub struct LockProbe(std::sync::Weak<Mutex<Store>>);

#[cfg(any(test, feature = "test-support"))]
impl LockProbe {
    /// Its lock is held now, by this thread or another (the store gone,
    /// or going: no one holds it).
    pub fn held(&self) -> bool {
        self.0.upgrade().is_some_and(|s| matches!(s.try_lock(), Err(std::sync::TryLockError::WouldBlock)))
    }
}

impl LiveLog {
    /// Tests: what its drop hands over now, its store with it (no job
    /// reading it): the charge it takes (O(1), kept as the log grows), and
    /// the bytes of every allocation it moves, walked one by one.
    pub fn retirement(&self) -> (usize, usize) {
        let s = self.lock();
        let unit = std::mem::size_of::<LogUnit>();
        let rounds = || [&self.open, &self.sealed].into_iter().flatten();
        let charge = unit + s.moved() + rounds().map(Round::retired).sum::<usize>() + self.head_answered.bytes();
        let walk = unit + s.walk() + rounds().map(Round::walk).sum::<usize>() + self.head_answered.walk();
        (charge, walk)
    }

    /// Tests: a probe of its store's lock, held or not ([`LockProbe`]).
    #[cfg(any(test, feature = "test-support"))]
    pub fn lock_probe(&self) -> LockProbe {
        LockProbe(Arc::downgrade(&self.store))
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

    /// Elements read or written under its lock so far, by the facts and
    /// by its readers: the slots, rank nodes, cuts and handles each
    /// touched (tests: the frame's work per fact, besides the events it
    /// writes). A slot vector's growth is not counted (doubling: O(1)
    /// amortized a slot). Nothing is destroyed under it but a hand-over
    /// to the disposer ([`dispose::destroyed`] counts a thread's share).
    pub fn visits(&self) -> usize {
        self.lock().visits
    }

    /// The most elements touched in one hold of its lock (tests: how long
    /// a reader, a job's or the frame's, makes the other wait).
    pub fn most_held(&self) -> usize {
        self.lock().most
    }

    /// Its cuts, segments, and the most slots a segment has (tests: what
    /// bounds a hold of its lock, module docs).
    pub fn shape(&self) -> (usize, usize, usize) {
        let s = self.lock();
        (s.cuts.len(), s.starts.len(), s.widest.max(s.calls.slots.len()).max(s.results.slots.len()))
    }

    /// What it keeps (bytes, heap included; the calls' inputs are their
    /// rows').
    pub fn bytes(&self) -> usize {
        let s = self.lock();
        s.bytes() + self.open.as_ref().map_or(0, Round::bytes) + self.sealed.as_ref().map_or(0, Round::bytes) + self.head_answered.bytes()
    }

    /// Its events (tests), [`COPY`] in one hold of its lock at most.
    pub fn events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            let s = self.lock();
            let to = s.len().min(out.len() + COPY);
            for k in out.len()..to {
                out.push(s.get(k).event().0);
            }
            if to == s.len() {
                return out;
            }
        }
    }

    fn lock(&self) -> Held<'_> {
        hold(&self.store)
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
        self.open = Some(Round::new(self.bin.clone()));
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
            s.cut(at);
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
        let mut s = hold(&store);
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
        let mut s = hold(&store);
        let at = s.open();
        let had = s.len() > at;
        (s.lead, s.end) = (false, false);
        let (calls, results) = (std::mem::take(&mut s.calls), std::mem::take(&mut s.results));
        if had {
            s.cut(at);
        }
        drop(s);
        // Its round, its facts and their results go off the frame as one
        // ([`dispose`]), charged with the facts' inputs (their rows went
        // with the response) until the last of them is dropped.
        let open = self.open.take();
        let (re, rb) = open.as_ref().map_or((0, 0), |r| (r.elements(), r.retired()));
        let elements = re + calls.slots.len() + results.slots.len();
        let bytes = rb + calls.charge() + results.charge();
        self.bin.retire((open, calls, results), elements, bytes);
        if had {
            self.changed();
        }
        (self.target, self.empty) = (self.before, true);
    }

    /// The response's text or thinking.
    pub fn content(&mut self, history: &HistoryBlocks) {
        self.init(history);
        let store = self.store.clone();
        let mut s = hold(&store);
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
        let mut s = hold(&store);
        let s = &mut *s;
        let mut moved = self.opened(s) | (was != self.blocks);
        let r = self.open.as_mut().expect("opened");
        if !r.rows.contains_key(&row) {
            r.push(LiveCall { tool_id: tool_id.into(), name: name.into(), input: None, size: 0, whole: false, answered: false }, row);
            // Its slot, in model order: empty until its input is whole.
            s.calls.push(None, &s.touched);
            moved |= self.sync_end(s);
        }
        if moved {
            self.changed();
        }
    }

    /// Call `tool_id`'s input is whole (`input`: kept, a card's or a
    /// block's, as its row keeps it: [`canonical`]). Again for the same
    /// row: its input is this one. Its input as kept, for the row to keep:
    /// the same input again is the one kept before, so the row, the call
    /// and its fact keep one allocation (the row's count is the one).
    #[must_use]
    pub fn call(&mut self, history: &HistoryBlocks, row: u64, tool_id: &str, name: &str, input: Option<Arc<Value>>) -> Option<Arc<Value>> {
        self.init(history);
        let was = self.blocks;
        self.blocks |= is_block_tool(name);
        let mut was_input = None;
        let store = self.store.clone();
        let mut held = hold(&store);
        let s = &mut *held;
        let mut moved = self.opened(s) | (was != self.blocks);
        let r = self.open.as_mut().expect("opened");
        let size = input.as_deref().map_or(0, kept_bytes);
        let fact = |input: &Option<Arc<Value>>| Fact::Call { tool_id: tool_id.into(), name: name.into(), input: input.clone(), row, size };
        let mut kept = input.clone();
        match r.rows.get(&row).copied() {
            Some(k) => {
                let c = &r.calls[k];
                let same = c.whole
                    && c.tool_id == tool_id
                    && c.name == name
                    && match (&c.input, &input) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                        (None, None) => true,
                        _ => false,
                    };
                if same {
                    // The same call again: what it kept stays, the row's too.
                    kept = c.input.clone();
                } else {
                    r.rename(k, tool_id, name);
                    was_input = r.reinput(k, input.clone(), size);
                }
                let c = &mut r.calls[k];
                if !std::mem::replace(&mut c.whole, true) {
                    if c.answered {
                        r.done += 1;
                    }
                    // In model order, in its slot: after its round's whole
                    // calls (its rank their count: nothing read), else
                    // among them (a cut at its rank). Nothing moves.
                    let last = r.last_whole.is_none_or(|l| l < k);
                    let end = s.calls_at() + s.calls.len;
                    if last {
                        r.last_whole = Some(k);
                        if s.results.len > 0 || s.end {
                            s.cut(end);
                        }
                    }
                    let at = s.calls.fill(k, Arc::new(fact(&input)), &s.touched);
                    if !last {
                        s.cut(s.calls_at() + at);
                    }
                    self.work += 1;
                    moved = true;
                } else if !same {
                    // Its input again, another: in its place (a cut).
                    let at = s.calls.set(k, Arc::new(fact(&input)), &s.touched, &s.bin);
                    s.cut(s.calls_at() + at);
                    self.work += 1;
                    moved = true;
                }
            }
            None => {
                let k = r.push(
                    LiveCall { tool_id: tool_id.into(), name: name.into(), input: input.clone(), size, whole: true, answered: false },
                    row,
                );
                r.last_whole = Some(k);
                if s.results.len > 0 || s.end {
                    s.cut(s.calls_at() + s.calls.len);
                }
                s.calls.push(Some(Arc::new(fact(&input))), &s.touched);
                self.work += 1;
                moved = true;
            }
        }
        moved |= self.sync_end(s);
        if moved {
            self.changed();
        }
        // An input replaced goes after the store's lock.
        drop(held);
        drop(was_input);
        kept
    }

    /// Call `tool_id`'s final result. It is the open batch's (the last
    /// message, if an assistant's: a live round, else `history`'s last
    /// message) latest call of that id; one that has its result keeps it,
    /// and a result whose batch has ended is left out (module docs).
    pub fn result(&mut self, history: &HistoryBlocks, tool_id: &str, text: &str) {
        self.init(history);
        let store = self.store.clone();
        let mut s = hold(&store);
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
                        s.results.push(Some(Arc::new(fact())), &s.touched);
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
                if !self.head_answered.insert(tool_id) {
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
    /// The staged chunks: the history's events from `from` on, and their
    /// bytes, heap included.
    chunks: Vec<Chunk>,
    staged: usize,
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
            s.touched.add(1);
            (s.cuts.len(), s.len())
        };
        // The generation is the history's first message's (§1.2). With no
        // history yet, the live prompt's, as the history will hold it (a
        // string): folded again once the history lands, as any live turn.
        Some(Input {
            epoch: history.epoch,
            generation: history.generation.or(log.first).unwrap_or_else(|| block::Generation::of(&Value::Null)),
            chunks: history.staged.iter().cloned().collect(),
            staged: history.staged_bytes,
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

    /// One handle of an input let go off this thread (D7's archive
    /// capture of a compaction's predecessor: Shady #105 R2-2), whichever
    /// way its holder ends: handed to this thread's bin
    /// ([`dispose::current`]) in O(1), and dropped by its disposer, with
    /// it, if it was their last owner, the history's worker (its journal,
    /// its board) and the staged chunks. Charged meanwhile with what they
    /// keep (the worker's as of its last job: an overcount while another
    /// owner keeps them, never under). Refused or with no disposer, it is
    /// dropped here, as everything a bin is handed (module docs of
    /// [`dispose`]). This thread's cost: one element.
    pub fn retire(input: Arc<Input>) {
        let elements = input.history_len + 1;
        let bytes = input.shared.bytes.load(Ordering::Relaxed) + input.staged + input.chunks.capacity() * std::mem::size_of::<Chunk>();
        dispose::current().retire(input, elements, bytes);
    }

    /// A block call in its live turn.
    pub fn live_blocks(&self) -> bool {
        self.live.blocks
    }

    /// Its live turn's events, copied (a re-fold of the whole input's);
    /// `None` if its log changed them since (a newer input stands). Read
    /// as a job reads them: a batch of handles under the log's lock at a
    /// time ([`Input::handles`]), made events after it.
    pub fn live_events(&self) -> Option<Vec<Event>> {
        Some(self.live_facts()?.iter().map(|f| f.event().0).collect())
    }

    /// Its live turn's facts, a batch at a time; `None` if its log changed
    /// them since.
    fn live_facts(&self) -> Option<Vec<Handle>> {
        let mut all = Vec::with_capacity(self.live.len);
        while all.len() < self.live.len {
            all.extend(self.handles(self.live.cuts, all.len())?);
        }
        Some(all)
    }

    /// Under its log's lock, nothing but this: whether its live turn still
    /// holds (one cut query), and a batch of the handles of its events from
    /// `from` on ([`Store::batch`]: a reader makes the frame wait no
    /// longer). `None` if a cut since its `cuts` changed its first `len`.
    fn handles(&self, cuts: usize, from: usize) -> Option<Vec<Handle>> {
        let s = hold(&self.live.store);
        if s.kept(cuts, self.live.len) < self.live.len {
            return None;
        }
        Some(s.batch(from, self.live.len))
    }

    /// Its live turn's events' rows (tests).
    #[cfg(test)]
    fn live_rows(&self) -> Vec<Option<u64>> {
        self.live_facts().unwrap_or_default().iter().map(|f| f.event().1).collect()
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

/// An [`Arc`]'s counts, besides what it holds (bytes).
const ARC: usize = 2 * std::mem::size_of::<usize>();

impl RowBlocks {
    /// What it keeps of its own, its [`Arc`]'s included (bytes): its
    /// shown blocks' entries and its run's title, not the blocks (the
    /// board's copies, counted with it: [`Drawn::bytes`]).
    pub fn own_bytes(&self) -> usize {
        ARC + std::mem::size_of::<RowBlocks>() + Shown::list_bytes(&self.blocks) + self.run.as_ref().map_or(0, |r| ARC + r.len())
    }

    /// One of the rows every committed call shares ([`RowBlocks::committed`]):
    /// nobody's to count.
    pub fn is_shared(r: &Arc<RowBlocks>) -> bool {
        [Fate::Bound, Fate::Unbound].iter().any(|&f| Arc::ptr_eq(r, &Self::committed(f)))
    }

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

impl Shown {
    /// What a list of them keeps of its own (bytes): its entries and its
    /// groups' member lists, not the blocks (the board's copies, counted
    /// with it: [`Drawn::bytes`]).
    pub fn list_bytes(list: &Vec<Shown>) -> usize {
        list.capacity() * std::mem::size_of::<Shown>() + list.iter().map(|s| Shown::list_bytes(&s.members)).sum::<usize>()
    }
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
    /// The rail's two slots (§4), `upper` then `end`: each slot's blocks
    /// that aren't group members, in its list's order (the agent's
    /// placement order), a group with its members.
    pub rail: [Vec<Shown>; 2],
    /// The history's block calls that committed since the rows last took
    /// them, their rows and fates: each row keeps its fate once taken
    /// ([`HistoryBlocks::took`] with `committed_to`, how many so far).
    pub committed: Arc<[(u64, Fate)]>,
    pub committed_to: u64,
    /// Live blocks on the board, groups and members included.
    pub live: usize,
    /// Its visibility records (§3.5): as drawn, the live turn's included
    /// (what a click is made on), and the history's alone (settled: what a
    /// desire settles against). D6b's.
    pub visibility: Arc<Visibility>,
    pub settled: Arc<Visibility>,
    /// The board's op log (§3.8: ≤ 256 lines, oldest first), each line
    /// with where its event is ([`Mark`]: its time is its row's), and the
    /// revision records and tombstones it still holds (§1.8), records
    /// marked too: what D7's op log and revision walk read (§3.6). Copies
    /// of the board's, counted at B2's count of them.
    pub log: Arc<[(Logged, Mark)]>,
    pub records: Arc<[(Record, Mark)]>,
    pub tombstones: Arc<[Tombstone]>,
    /// What it keeps, heap included (bytes): its rows and their blocks
    /// (copies of the boards' blocks, counted at B2's count of those: an
    /// upper bound), its committed fates, its op log, records and
    /// tombstones, its visibility maps ([`vis_owned`]: `settled`, which its
    /// worker holds too, counted in both; `visibility` once more unless it
    /// is `settled`'s). A row's
    /// [`Arc`] of it is its.
    pub bytes: usize,
}

/// A visibility map in its own [`Arc`], owned bytes (§1.8's model): the
/// allocation (its counts and the map's inline part) and what the map
/// owns, `heap` (B2's count of its records: [`State::owned_by_owner`]'s).
fn vis_owned(heap: usize) -> usize {
    ARC + std::mem::size_of::<Visibility>() + heap
}

/// What a state's visibility map owns past its inline part
/// ([`State::owned_by_owner`]'s fourth owner).
fn vis_heap(s: &State) -> usize {
    s.owned_by_owner()[3]
}
/// Where a log line or a record's event is: its row, and the call that
/// made it (its `tool_id`), by which the row's time is found
/// ([`crate::Transcript::call_at`]: a lookup, never a walk of the rows).
/// Only a call has a row.
///
/// **Bounded** (Shady #105 R2-1): a board keeps a call's id once, shared
/// by every line and record of that call (one allocation, whatever its
/// ops), an id of at most [`CALL_ID_MAX`] bytes, and [`CALL_IDS_MAX`]
/// bytes of ids in all, its Arcs' counts included: the newest lines'
/// first, then the newest records'. Past either, a mark keeps its row
/// and no id: its time is unknown, and drawn as such (`—`), never
/// guessed. A lookup by it hashes [`CALL_ID_MAX`] bytes at most.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    pub row: Option<u64>,
    pub call: Option<Arc<str>>,
}

/// A call id a board's marks keep, at most (bytes; [`Mark`]): a model's
/// tool ids are some 30.
pub const CALL_ID_MAX: usize = 128;

/// The call ids a board's marks keep in all, at most (bytes, their Arcs'
/// counts included; [`Mark`]): room for every line and record of a board
/// at B2's caps (256 and 1,024) with a model's ids.
pub const CALL_IDS_MAX: usize = 64 * 1024;

/// What a [`Worker`] keeps, counted: the journal's events (and a row
/// each) and the events it has room for, those of them the live turn's,
/// the committed calls' fates the rows haven't taken yet, the board's
/// owned bytes (B2's working set, its card fold apart), the card folds'
/// (the board's and the overlay's, each its own), and all it keeps in
/// bytes, heap included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retained {
    pub journal: usize,
    pub capacity: usize,
    pub rows: usize,
    pub live: usize,
    pub committed: usize,
    pub board_bytes: usize,
    pub card_bytes: usize,
    pub bytes: usize,
}

/// Handles a reader takes from a log in one hold of its lock, at most
/// (the frame waits at most this long to add a fact).
pub const COPY: usize = 256;

/// Elements a reader's batch keeps a hold of a log's lock within: it stops
/// before the next handle's read could pass it (module docs: its cut
/// queries before it can, by `O(log² cuts)`).
pub const HOLD: usize = 4 * COPY;

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
    /// The history's visibility records, as of the history read last, and
    /// what their map owns ([`vis_heap`]).
    settled: Arc<Visibility>,
    settled_heap: usize,
    /// The board's card fold's bytes ([`Board::card_fold_bytes`], an
    /// estimate that reads every card), as of its position then: read
    /// again when the history is read on, not per slice.
    cards: (usize, usize),
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
    /// The live turn's events copied that still hold (the journal's from
    /// `hist` on past these are cut, a slice at a time).
    valid: usize,
    batch: Batch,
    fates: Vec<(usize, Fate)>,
    /// Its card fold's bytes (a copy of the board's, counted as copied,
    /// until it reads what can change it, `cards_read`: a card call, or with
    /// a card in it a result or a user's message; then read again when the
    /// run ends or stops).
    cards: usize,
    cards_read: bool,
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
            settled: Arc::default(),
            settled_heap: 0,
            cards: (0, 0),
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

    /// What it keeps (bytes, heap included): its journal and rows as
    /// allocated, the committed fates not taken yet (their queue as
    /// allocated), its board and the overlay's, each with its card fold,
    /// and the history's visibility map ([`vis_owned`]; the last
    /// publication shares it, and counts it too).
    pub fn bytes(&self) -> usize {
        self.bytes
            + vis_owned(self.settled_heap)
            + (self.journal.capacity() - self.journal.len()) * std::mem::size_of::<Event>()
            + self.rows.capacity() * std::mem::size_of::<Option<u64>>()
            + self.committed.capacity() * std::mem::size_of::<(u64, Fate)>()
            + self.board.owned_bytes()
            + self.cards.1
            + self.over.as_ref().map_or(0, |o| o.board.owned_bytes() + o.cards + o.fates.capacity() * std::mem::size_of::<(usize, Fate)>())
    }

    /// What it keeps ([`Retained`]).
    pub fn retained(&self) -> Retained {
        Retained {
            journal: self.journal.len(),
            capacity: self.journal.capacity(),
            rows: self.rows.len(),
            live: self.journal.len() - self.hist,
            committed: self.committed.len(),
            board_bytes: self.board.owned_bytes(),
            card_bytes: self.cards.1 + self.over.as_ref().map_or(0, |o| o.cards),
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
        while !self.rewind(self.hist + n, &mut || true) {}
    }

    /// The journal cut to `to` events (the history's at least), a slice of
    /// them at a time, `go_on` asked between; `false` if it said stop (what
    /// is cut stays cut: the next run cuts on). Shared (a re-fold reads
    /// it), the events kept are copied, not those cut. A journal with room
    /// for more than twice its events past a slice gives the room back.
    fn rewind(&mut self, to: usize, go_on: &mut dyn FnMut() -> bool) -> bool {
        while self.journal.len() > to {
            let from = self.journal.len().saturating_sub(SLICE).max(to);
            self.bytes -= self.journal[from..].iter().map(event_bytes).sum::<usize>();
            match Arc::get_mut(&mut self.journal) {
                Some(journal) => journal.truncate(from),
                None => self.journal = Arc::new(self.journal[..from].to_vec()),
            }
            self.rows.truncate(from);
            if from > to && !go_on() {
                return false;
            }
        }
        let (len, room) = (self.journal.len(), self.journal.capacity());
        if room > 2 * len + SLICE {
            if let Some(journal) = Arc::get_mut(&mut self.journal) {
                journal.shrink_to(len + len / 2);
            }
            self.rows.shrink_to(len + len / 2);
        }
        true
    }

    /// A job stopped ([`Worker::run`]): what it read is kept, and counted
    /// as of where it is, the card folds read again if they read on.
    fn paused(&mut self, input: &Input) -> Option<Drawn> {
        self.count_cards();
        if let Some(o) = self.over.as_mut()
            && std::mem::take(&mut o.cards_read)
        {
            o.cards = o.board.card_fold_bytes();
        }
        input.shared.bytes.store(self.bytes(), Ordering::Relaxed);
        None
    }

    /// The board's card fold's bytes, read again if it read on since.
    fn count_cards(&mut self) {
        let at = self.board.position();
        if self.cards.0 != at {
            self.cards = (at, self.board.card_fold_bytes());
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
            self.over = None;
            if !self.rewind(self.hist, go_on) {
                return self.paused(input);
            }
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
                return self.paused(input);
            }
        }
        self.count_cards();
        // The overlay: another log's, or on another history, starts over.
        let history = input.history_len;
        let (log, want) = (input.live.log, input.live.len);
        if !self.over.as_ref().is_some_and(|o| o.log == log && o.history == history) {
            // The history's records (once a history: its view's replay
            // changes no record).
            let settled = self.board.view(&self.journal[..history]);
            if *settled.visibility() != *self.settled {
                self.settled = Arc::new(settled.visibility().clone());
                self.settled_heap = vis_heap(&settled);
            }
            // Its fold a copy of the board's, counted as copied (a copy
            // has no room to spare).
            let board = self.board.clone();
            let cards = board.card_fold_bytes();
            self.over = Some(Over {
                log,
                cuts: 0,
                history,
                board,
                read: 0,
                valid: 0,
                batch: self.batch,
                fates: Vec::new(),
                cards,
                cards_read: false,
            });
        }
        loop {
            // Under the log's lock, nothing but this (module docs): whether
            // the input still holds, what of the events copied still holds
            // (two cut queries), and a batch of the next ones' handles.
            let batch = {
                let s = hold(&input.live.store);
                if s.kept(input.live.cuts, want) < want {
                    None
                } else {
                    let o = self.over.as_mut().expect("the overlay");
                    o.valid = s.kept(o.cuts, o.valid).min(want);
                    o.cuts = s.cuts.len();
                    Some(s.batch(o.valid, want))
                }
            };
            // Off the lock: what no longer holds cut (a slice at a time),
            // the board copied, the inputs too.
            let Some(batch) = batch else { return self.paused(input) };
            let o = self.over.as_mut().expect("the overlay");
            let valid = o.valid;
            if valid < o.read {
                // What it read changed: it reads again from the history's end.
                (o.board, o.read, o.batch, o.cards_read) = (self.board.clone(), 0, self.batch, false);
                o.cards = o.board.card_fold_bytes();
                o.fates.clear();
            }
            if !self.rewind(self.hist + valid, go_on) {
                return self.paused(input);
            }
            let journal = Arc::make_mut(&mut self.journal);
            let o = self.over.as_mut().expect("the overlay");
            for f in &batch {
                let (e, row) = f.event();
                // What can change B2's card fold: a card call, and with a
                // card in it, a result (binding one) or a user's message
                // (answering one).
                o.cards_read |= match &e {
                    Event::Call { name, .. } => CARD_NAMES.contains(&name.as_str()),
                    Event::Result { .. } | Event::User { .. } => !o.board.cards().cards().is_empty(),
                    Event::ResultsEnd => false,
                };
                self.bytes += event_bytes(&e);
                journal.push(e);
                self.rows.push(row);
            }
            o.valid += batch.len();
            drop(batch);
            // Read on over what is copied, a slice at most.
            let have = o.valid;
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
                return self.paused(input);
            }
        }
        let o = self.over.as_mut().expect("the overlay");
        if std::mem::take(&mut o.cards_read) {
            o.cards = o.board.card_fold_bytes();
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
        let rail =
            [block::Slot::Upper, block::Slot::End].map(|slot| view.list(Place::Rail(slot)).iter().filter_map(shown).collect::<Vec<_>>());
        // Its blocks are copies of the boards' (each block once at most):
        // B2's count of their blocks bounds them, heap included (its
        // own: a block's item ids and last writers are B2's to read).
        let (mut shown_n, mut bound_n) = (0, 0);
        let tops = rows.values().flat_map(|r| &r.blocks).chain(rail.iter().flatten());
        for s in tops.flat_map(|s| std::iter::once(s).chain(&s.members)) {
            shown_n += 1;
            bound_n += usize::from(s.bound.is_some());
        }
        let blocks = if shown_n > 0 { view.owned_by_owner()[0] } else { 0 }
            + if bound_n > 0 { bound.owned_by_owner()[0] } else { 0 }
            + (shown_n + bound_n) * ARC;
        let rows: HashMap<u64, Arc<RowBlocks>> = rows.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
        let committed: Arc<[(u64, Fate)]> = self.committed.iter().copied().collect();
        // The live view's records: the history's map when they are its
        // (one allocation, shared), else a map of their own.
        let visibility = if *view.visibility() == *self.settled { None } else { Some(Arc::new(view.visibility().clone())) };
        // The op log, records and tombstones (D7): copies, each record and
        // line with its event's mark (its row, its call's id: [`Mark`],
        // one allocation a call, within the budget, the newest lines'
        // first, then the newest records'); B2's count of its owners
        // bounds them (their deques' slots and heap), plus a mark each,
        // the ids kept, and their Arcs.
        let lines: Vec<&Logged> = view.log().collect();
        let recs: Vec<&Record> = view.records().collect();
        let mut ids: HashMap<usize, Option<Arc<str>>> = HashMap::new();
        let mut room = CALL_IDS_MAX;
        for pos in lines.iter().rev().map(|l| l.at.event).chain(recs.iter().rev().map(|r| r.at.event)) {
            ids.entry(pos).or_insert_with(|| match self.journal.get(pos) {
                Some(Event::Call { tool_id, .. }) if row(pos).is_some() && tool_id.len() <= CALL_ID_MAX && tool_id.len() + ARC <= room => {
                    room -= tool_id.len() + ARC;
                    Some(Arc::from(tool_id.as_str()))
                }
                _ => None,
            });
        }
        let mark = |pos: usize| Mark { row: row(pos), call: ids.get(&pos).cloned().flatten() };
        let log: Arc<[(Logged, Mark)]> = lines.iter().map(|l| ((*l).clone(), mark(l.at.event))).collect();
        let records: Arc<[(Record, Mark)]> = recs.iter().map(|r| ((*r).clone(), mark(r.at.event))).collect();
        let tombstones: Arc<[Tombstone]> = view.tombstones().cloned().collect();
        let [_, tombs, recs, _, _, _, logs] = view.owned_by_owner();
        let calls = CALL_IDS_MAX - room;
        let kept = tombs + recs + logs + (log.len() + records.len()) * std::mem::size_of::<Mark>() + calls + 3 * ARC;
        let bytes = std::mem::size_of::<Drawn>()
            + vis_owned(self.settled_heap)
            + visibility.as_ref().map_or(0, |_| vis_owned(vis_heap(view)))
            + rows.capacity() * (std::mem::size_of::<(u64, Arc<RowBlocks>)>() + 1)
            + rows.values().map(|r| r.own_bytes()).sum::<usize>()
            + rail.iter().map(Shown::list_bytes).sum::<usize>()
            + blocks
            + ARC
            + committed.len() * std::mem::size_of::<(u64, Fate)>()
            + kept;
        Drawn {
            epoch: input.epoch,
            generation: Some(input.generation),
            history: input.history_len,
            events: self.journal.len(),
            rows,
            rail,
            committed,
            committed_to: self.committed_from + self.committed.len() as u64,
            live: view.live(),
            visibility: visibility.unwrap_or_else(|| self.settled.clone()),
            settled: self.settled.clone(),
            log,
            records,
            tombstones,
            bytes,
        }
    }
}

/// The bound board's block, if it is the same incarnation as `b`.
fn same(bound: Option<&Block>, b: &Block) -> Option<Arc<Block>> {
    bound.filter(|x| x.tag == b.tag).map(|x| Arc::new(x.clone()))
}

#[cfg(test)]
mod tests;
