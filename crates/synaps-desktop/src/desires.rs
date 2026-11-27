//! The user's visibility desires (synaps-cards `docs/blocks.md` v1.3:
//! §3.5, §1.8's `block-desires.json` row, §5.1, §5.2; §8.2's D6 row; §9
//! decision 2): dismiss and show, each a versioned wish of this desk that
//! reaches the agent as a notice riding the user's next message.
//!
//! **One desire per id, never a queue of notices.** Per conversation (the
//! transcript's journal id) and generation of its history, for each block
//! id the user changed, the file keeps the latest wish (dismissed or
//! shown), its version, and whether a notice of the id may have been sent
//! ([`Desire`]). In memory only: the message it rides now (its
//! **carrier**, in flight), and whether the composer's chip kept it back.
//!
//! **The protocol is B2's** (`cards_spec::board`, `cards_spec::block::message`):
//! the board's [`Visibility`] holds each id's record, the floor and the
//! highest version seen, and its one helper `effective_version(id)` decides
//! what to attach, what is settled and what a restart still sends, as it
//! decides the fold's admission. Nothing here reads a notice from a
//! history: the fold does.
//!
//! **One writer: memory is the truth.** The process that owns the file
//! ([`crate::blockfiles`]) decides every change in memory, at once, and
//! writes whole snapshots of it, one at a time, in order, on the
//! process's writer ([`Desires::next_write`], [`Write::run`],
//! [`Desires::landed`]); each write holds the file's lock until it ran,
//! and a window's end hands it the last ([`Desires::last_write`]).
//! Another desk process on the machine keeps its desires in memory only
//! (its writes go nowhere), and says so for as long as it runs: what
//! desks share is the history.
//!
//! - **A wish** ([`Desires::dismiss`], [`Desires::show`]): already drawn so,
//!   nothing to do. Over a desire **no notice of which may have gone** (it
//!   and every desire it replaced unsent) whose undoing every board this
//!   desk has (the click's, and its last settled history's) draws, the
//!   desire is cancelled: nothing is sent. Otherwise a new desire, at one
//!   more than every version this desk knows in the generation (the
//!   board's `highest`, every board's it settled or clicked on, every one
//!   it minted, its desires'), keeping the sent mark of the one it
//!   replaces: **a desire that may have been sent is never cancelled**, an
//!   opposite click supersedes it. A new id past [`PER_CONVERSATION`] or
//!   [`PER_DESK`] is refused, as is any change leaving the file past
//!   [`RESERVE`] (as written, or with every version at its widest: a
//!   desire admitted can always be superseded). Until the snapshot with
//!   the wish lands, the id draws as before ([`Desires::pending`]) and
//!   rides nothing; landed, it draws; failed, memory goes back to what
//!   drew and the id says why ([`Desires::refused`]). So a restart never
//!   loses a wish the user saw drawn.
//! - **Sent means written first** ([`Desires::ride`]): a message carrying
//!   notices goes once the snapshot with their sent marks has landed (at
//!   once, if the file already has everything), with those still so; a
//!   failed write sends it plain. Messages leave in the order they were
//!   made.
//! - **Riding**: at every send of typed text (a `Submit` when idle, a
//!   `Steer` while a turn runs) or a hand-back, each desire above
//!   `effective_version(id)` that isn't in flight, kept back or pending is
//!   attached, as the message's last lines: at most 8 ids of each kind,
//!   dismissals first, the oldest versions first; the rest ride the next
//!   message. **Never on its own** (an empty body carries nothing) and
//!   **never on a card answer** ([`Carry::Answer`]). A body whose own last
//!   line would read as a notice beside the trailer carries nothing.
//! - **Settling** ([`Desires::settle`], on a settled history's board): a
//!   desire at or below `effective_version(id)` is done and dropped; if the
//!   shared state differs, the block says why ([`Feedback`], bounded,
//!   until seen or the next wish). A carrier the caller no longer has in
//!   flight (a dropped steer, a refused send, a rollback: [`Desires::lost`]
//!   too) is cleared, and the desire rides the next message at the same
//!   version. Every other generation of the conversation is dropped then
//!   (the history moved on), with its wishes not yet written; a wish on
//!   one of them since is refused.
//! - **A restart** ([`Desires::load`]) keeps the desires and their sent
//!   marks, and loses the carriers; the first settle against the attached
//!   history drops what it shows, and the rest ride again.
//! - **A compaction** ([`Desires::compacted`]) drops the generation's
//!   desires, and hands back the ids this desk drew dismissed: the
//!   successor's soft dismissals ([`crate::presentation`]).
//! - **Discard** ([`Desires::discard`]) drops a conversation's unsent
//!   desires. One that may have been sent stays until it settles: an
//!   obligation (an older copy may still land), never evicted.
//!
//! **Bounds.** Wishes and messages waiting for a write: [`QUEUE_MAX`] in
//! all (and [`QUEUE_BYTES_MAX`] of messages); past it the newest is
//! refused, said, and nothing is dropped. Settles and the rest only change
//! memory, and the next snapshot carries them. What this process knows of
//! a conversation's history ([`Known`]) is kept for [`KNOWN_MAX`]
//! conversations without desires, and for every one with desires.
//!
//! **The file**, `block-desires.json` next to `settings.json`: per
//! conversation, per generation (the first 16 hex digits), its desires,
//! ≤ [`ENTRY_MAX`] bytes each, and nothing else. **No cap evicts a desire**.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use cards_spec::block::message::{self, NOTICE_IDS_MAX, Notice, NoticeLine, Trailer, VERSION_MAX, Visibility as Seen};
use cards_spec::block::{BlockId, Generation};
use cards_spec::board::Visibility;
use serde_json::{Value, json};

use crate::blockfiles::{self, NotOwned, Owned, Stored};

/// The file, next to `settings.json`.
pub(crate) const FILE: &str = "block-desires.json";
/// Desires per conversation at most, every generation's (§1.8).
pub(crate) const PER_CONVERSATION: usize = 64;
/// Desires per desk at most (§1.8).
pub(crate) const PER_DESK: usize = 512;
/// The file's reserved budget, bytes (§1.8): no other owner uses it.
pub(crate) const RESERVE: usize = 64 * 1024;
/// One desire in the file, bytes at most (§1.8): a 24-character id, its
/// state, a 9-digit version and its sent mark.
#[cfg_attr(not(test), expect(dead_code, reason = "the bound the tests hold an entry to"))]
pub(crate) const ENTRY_MAX: usize = 64;
/// A conversation's key (its journal id), bytes at most.
pub(crate) const KEY_MAX: usize = 128;
/// Wishes and messages waiting for a write, at most (module docs).
pub(crate) const QUEUE_MAX: usize = 64;
/// Bytes of messages waiting for a write, at most.
pub(crate) const QUEUE_BYTES_MAX: usize = 1 << 20;
/// Settled feedback kept per conversation, and in all, at most: the
/// oldest goes first (an explanation, not a wish).
pub(crate) const FEEDBACK_PER_CONVERSATION: usize = PER_CONVERSATION;
pub(crate) const FEEDBACK_MAX: usize = PER_DESK;
/// Conversations without desires whose history this process knows
/// ([`Known`]), at most.
pub(crate) const KNOWN_MAX: usize = 256;
/// The file's format.
const FORMAT: u64 = 3;

/// The refusals' words (§1.8).
pub(crate) const FULL_HERE: &str = "64 changes waiting · send a message first";
pub(crate) const FULL_DESK: &str = "512 changes waiting · send in another conversation, or discard";
pub(crate) const BUSY: &str = "still saving earlier changes · try again";
pub(crate) const MOVED_ON: &str = "this conversation moved on · try again";
pub(crate) const TOO_MANY: &str = "too many changes in this conversation";

/// The file next to `settings.json` (`None`: no config folder).
pub(crate) fn path() -> Option<PathBuf> {
    crate::theme::settings_path().map(|p| p.with_file_name(FILE))
}

/// A generation as the file keeps it: its first 16 hex digits.
fn generation_key(g: &Generation) -> String {
    let mut s = g.to_string();
    s.truncate(16);
    s
}

fn key_ok(conv: &str) -> bool {
    !conv.is_empty() && conv.len() <= KEY_MAX && !conv.chars().any(char::is_control)
}

/// One id's desire, in one generation of a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Desire {
    pub(crate) dismissed: bool,
    pub(crate) version: u32,
    /// A notice of this id may have been sent, at this version or one it
    /// superseded, and may still land: never cancelled (module docs).
    pub(crate) sent: bool,
    /// The message it rides now (memory only): in flight.
    carrier: Option<u64>,
    /// Kept back from the next message by the chip's `×` (memory only).
    held: bool,
}

impl Desire {
    fn new(dismissed: bool, version: u32, sent: bool) -> Self {
        Desire { dismissed, version, sent, carrier: None, held: false }
    }

    pub(crate) fn seen(&self) -> Seen {
        if self.dismissed { Seen::Dismissed } else { Seen::Shown }
    }

    pub(crate) fn in_flight(&self) -> bool {
        self.carrier.is_some()
    }
}

/// The same wish: both none, or the same state at the same version.
fn same(a: Option<&Desire>, b: Option<&Desire>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a.dismissed, a.version) == (b.dismissed, b.version),
        _ => false,
    }
}

/// One generation's desires.
type Slot = BTreeMap<BlockId, Desire>;

/// Every conversation's desires, per generation: memory's, and the file's
/// content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Book {
    conversations: BTreeMap<String, BTreeMap<String, Slot>>,
}

impl Book {
    fn len(&self) -> usize {
        self.conversations.values().flat_map(BTreeMap::values).map(BTreeMap::len).sum()
    }

    fn conversation_len(&self, conv: &str) -> usize {
        self.conversations.get(conv).map_or(0, |g| g.values().map(BTreeMap::len).sum())
    }

    fn slot(&self, conv: &str, generation: &str) -> Option<&Slot> {
        self.conversations.get(conv)?.get(generation)
    }

    fn slot_mut(&mut self, conv: &str, generation: &str) -> Option<&mut Slot> {
        self.conversations.get_mut(conv)?.get_mut(generation)
    }

    fn get(&self, conv: &str, generation: &str, id: &BlockId) -> Option<&Desire> {
        self.slot(conv, generation)?.get(id)
    }

    /// `id`'s desire in `generation` of `conv` made `d` (`None`: none).
    fn set(&mut self, conv: &str, generation: &str, id: &BlockId, d: Option<Desire>) {
        match d {
            Some(d) => {
                self.conversations.entry(conv.to_string()).or_default().entry(generation.to_string()).or_default().insert(id.clone(), d);
            }
            None => {
                if let Some(s) = self.slot_mut(conv, generation) {
                    s.remove(id);
                }
            }
        }
    }

    fn desires_mut(&mut self) -> impl Iterator<Item = &mut Desire> {
        self.conversations.values_mut().flat_map(|g| g.values_mut()).flat_map(|s| s.values_mut())
    }

    /// No empty generation nor conversation kept.
    fn tidy(&mut self) {
        for gens in self.conversations.values_mut() {
            gens.retain(|_, s| !s.is_empty());
        }
        self.conversations.retain(|_, g| !g.is_empty());
    }

    /// The file's bytes with every version at its widest: what the desires
    /// in it may grow to by supersession alone.
    fn widest_bytes(&self) -> usize {
        let mut wide = self.clone();
        wide.desires_mut().for_each(|d| d.version = VERSION_MAX);
        wide.text().len()
    }
}

impl Stored for Book {
    fn parse(text: &str) -> Result<(Self, usize), String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if v["version"].as_u64() != Some(FORMAT) {
            return Err(format!("not format {FORMAT}"));
        }
        let convs = v["conversations"].as_object().ok_or("no conversations")?;
        let gen_ok = |g: &str| g.len() == 16 && g.bytes().all(|b| b.is_ascii_hexdigit());
        let (mut book, mut bad) = (Book::default(), 0);
        for (key, gens) in convs {
            let Some(gens) = gens.as_object().filter(|_| key_ok(key)) else {
                bad += 1;
                continue;
            };
            for (g, ds) in gens {
                let Some(ds) = ds.as_object().filter(|_| gen_ok(g)) else {
                    bad += 1;
                    continue;
                };
                for (id, d) in ds {
                    let bit = |k: &str| d[k].as_u64().filter(|b| *b <= 1).map(|b| b == 1);
                    let version = d["v"].as_u64().filter(|v| (1..=u64::from(VERSION_MAX)).contains(v)).map(|v| v as u32);
                    match (BlockId::parse(id), bit("d"), version, bit("s")) {
                        (Some(id), Some(dismissed), Some(v), Some(sent)) => book.set(key, g, &id, Some(Desire::new(dismissed, v, sent))),
                        _ => bad += 1,
                    }
                }
            }
        }
        book.tidy();
        Ok((book, bad))
    }

    fn text(&self) -> String {
        let convs: serde_json::Map<String, Value> = self
            .conversations
            .iter()
            .map(|(key, gens)| {
                let gens: serde_json::Map<String, Value> = gens
                    .iter()
                    .map(|(g, s)| {
                        let ds: serde_json::Map<String, Value> = s
                            .iter()
                            .map(|(id, d)| (id.to_string(), json!({"d": u8::from(d.dismissed), "v": d.version, "s": u8::from(d.sent)})))
                            .collect();
                        (g.clone(), Value::Object(ds))
                    })
                    .collect();
                (key.clone(), Value::Object(gens))
            })
            .collect();
        json!({"version": FORMAT, "conversations": convs}).to_string()
    }
}

/// What this process knows of a conversation's history: its generation
/// (the last settled, else the first clicked), the highest version it has
/// seen there (on any board it was given) or minted, and its last settled
/// history's board.
#[derive(Clone, Debug)]
struct Known {
    generation: String,
    high: u32,
    settled: Option<Visibility>,
    touched: u64,
}

/// A write of the file, to run off the UI thread ([`Write::run`]); what it
/// returns goes back to [`Desires::landed`].
#[derive(Debug)]
pub(crate) struct Write {
    ticket: u64,
    /// Memory's change count the snapshot has.
    seq: u64,
    /// The file (`None`: memory only, nothing written), and its lock, held
    /// until the write ran ([`blockfiles::Owned`]).
    path: Option<PathBuf>,
    owner: Option<Owned>,
    work: Work,
}

#[derive(Debug)]
enum Work {
    /// The owner's first: what crashes left beside the file swept, an
    /// unreadable one set aside.
    Sweep,
    /// Memory, whole: the file's text.
    Snapshot(String),
}

/// A [`Write`] as it ran.
#[derive(Debug)]
pub(crate) struct Landing {
    ticket: u64,
    seq: u64,
    sweep: bool,
    /// Written (and what the sweep said, if anything), or why not.
    done: Result<Option<String>, String>,
}

impl Write {
    /// The write, on whatever thread runs it ([`crate::blockfiles`]).
    pub(crate) fn run(self) -> Landing {
        let sweep = matches!(self.work, Work::Sweep);
        let done = match (&self.path, self.work) {
            (None, _) => Ok(None),
            (Some(p), Work::Sweep) => Ok(blockfiles::sweep::<Book>(p)),
            (Some(p), Work::Snapshot(text)) => blockfiles::write(p, &text).map(|()| None),
        };
        drop(self.owner);
        Landing { ticket: self.ticket, seq: self.seq, sweep, done }
    }
}

/// How a message goes to the agent ([`Desires::ride`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Carry {
    /// Typed text, idle.
    Submit,
    /// Typed text while a turn runs.
    Steer,
    /// A block edit handed back, or restored (D8), or its Resend.
    #[cfg_attr(not(test), expect(dead_code, reason = "D8's hand-back carries notices"))]
    HandBack,
    /// A card answer: all grammar, never a notice (§3.5).
    Answer,
}

/// What a dismiss or a show asked, at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ask {
    /// This desk already draws it so.
    Unchanged,
    /// Decided; it draws when its write lands ([`Landed::Wish`]).
    Saving,
}

/// What a wish did, written and drawn ([`Landed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Done {
    /// A desire no notice went for, cancelled: nothing will be sent.
    Cancelled,
    /// A new desire at this version: it rides the next message.
    Wished(u32),
}

/// A wish or a message refused: why, in the user's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refused(pub(crate) String);

/// Why a settled block draws other than this desk wished (§3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feedback {
    /// Another desk's notice, or a typed one, landed first.
    ChangedByLater,
    /// The id has no record, and the floor made the notice stale.
    NotApplied,
}

impl Feedback {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Feedback::ChangedByLater => "changed by a later message",
            Feedback::NotApplied => "not applied · dismiss again",
        }
    }
}

/// A message as it goes: its text (the body, then the trailer if any), the
/// carrier its notices ride (none: nothing rides), and what rides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ride {
    pub(crate) text: String,
    pub(crate) carrier: Option<u64>,
    pub(crate) notices: Vec<(BlockId, Seen, u32)>,
}

/// A message asked of [`Desires::ride`]: to go now, or once what it
/// marked is written ([`Landed::Send`] with this ticket).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Riding {
    Go(Ride),
    Wait(u64),
}

/// A write landed ([`Desires::landed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Landed {
    /// A wish: what it did, drawn now; or why it wasn't kept, what drew
    /// before drawn again.
    Wish { conv: String, id: BlockId, done: Result<Done, Refused> },
    /// A message to send now, in this order: [`Riding::Wait`]'s.
    Send { ticket: u64, ride: Ride },
}

/// The user's wishes on an id not written yet: what drew before the first
/// (drawn until they land), and memory's now, from the change `at`.
#[derive(Clone, Debug)]
struct Asked {
    at: u64,
    before: Option<Desire>,
    after: Option<Desire>,
}

/// A message waiting for its marks to be written, or for one ahead of it.
#[derive(Clone, Debug)]
struct Sending {
    ticket: u64,
    /// Memory's change count with its marks (0: it marked nothing).
    at: u64,
    body: String,
    carrier: Option<u64>,
    conv: String,
    generation: String,
    picks: Vec<(BlockId, Seen, u32)>,
}

/// An asked wish's key: conversation, generation, id.
type Key = (String, String, BlockId);

/// This desk's desires, every conversation's (module docs).
pub(crate) struct Desires {
    path: Option<PathBuf>,
    /// The file's lock, the app's ([`blockfiles::Owned`]), or why not:
    /// memory only.
    owner: Result<Owned, NotOwned>,
    /// The truth: every change decided, written or not.
    book: Book,
    known: BTreeMap<String, Known>,
    next_carrier: u64,
    /// Settled feedback per (conversation, generation, id), with its order:
    /// bounded ([`FEEDBACK_PER_CONVERSATION`], [`FEEDBACK_MAX`]).
    feedback: BTreeMap<(String, String, BlockId), (u64, Feedback)>,
    /// Why a wish wasn't kept, until the next wish on the id: bounded as
    /// the feedback.
    refusals: BTreeMap<(String, BlockId), (u64, String)>,
    next_note: u64,
    /// Wishes not written yet, per id.
    asked: BTreeMap<Key, Asked>,
    /// Messages waiting, in order.
    sending: VecDeque<Sending>,
    next_send: u64,
    /// Memory's changes to the file's content, counted.
    seq: u64,
    /// The count the file has (the last snapshot landed).
    written: u64,
    /// The last snapshot that failed: not tried again until memory changes,
    /// a message waits for it, or a settle.
    failed_at: Option<u64>,
    /// The write out, by ticket.
    out: Option<u64>,
    next_ticket: u64,
    /// The sweep still to run (the owner's first write).
    sweep: bool,
    /// Why the file read as empty at the load, what its sweep said, or that
    /// it is another process's (Settings says it).
    pub(crate) trouble: Option<String>,
}

impl Desires {
    /// The desires in the file at `path`, its lock taken here, now
    /// ([`Desires::open`]; tests: a launch).
    #[cfg(test)]
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        Self::open(path.map(|p| {
            let o = blockfiles::own(&p);
            (p, o)
        }))
    }

    /// The desires in the file at `path`, owned by this process (`file`'s
    /// lock, the app's) and read now; its sweep is the first write.
    /// Another process's: memory only, the file untouched. `None`: no
    /// config folder, and every wish is refused: it couldn't be kept.
    pub(crate) fn open(file: Option<(PathBuf, Result<Owned, NotOwned>)>) -> Self {
        let (path, owner) = match file {
            Some((p, o)) => (Some(p), o),
            None => (None, Err(NotOwned::Failed("no settings folder".into()))),
        };
        let (book, trouble) = match (&path, &owner) {
            (Some(p), Ok(_)) => blockfiles::peek::<Book>(p),
            (Some(_), Err(NotOwned::Held)) => {
                (Book::default(), Some(format!("{FILE} is another desk's: changes here are kept in memory only")))
            }
            (Some(_), Err(NotOwned::Failed(why))) => {
                (Book::default(), Some(format!("{FILE}: {why}: changes here are kept in memory only")))
            }
            (None, _) => (Book::default(), None),
        };
        Desires {
            path,
            sweep: owner.is_ok(),
            owner,
            book,
            known: BTreeMap::new(),
            next_carrier: 0,
            feedback: BTreeMap::new(),
            refusals: BTreeMap::new(),
            next_note: 0,
            asked: BTreeMap::new(),
            sending: VecDeque::new(),
            next_send: 0,
            seq: 0,
            written: 0,
            failed_at: None,
            out: None,
            next_ticket: 0,
            trouble,
        }
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "tests"))]
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether this process owns the file (else memory only).
    pub(crate) fn owns_file(&self) -> bool {
        self.owner.is_ok()
    }

    /// Why the file isn't this process's (memory only), if it isn't.
    pub(crate) fn not_owned(&self) -> Option<&NotOwned> {
        self.owner.as_ref().err()
    }

    /// The next write to run off the UI thread, once the one out landed: the
    /// sweep first, then memory whole whenever it changed since the file.
    pub(crate) fn next_write(&mut self) -> Option<Write> {
        if self.out.is_some() {
            return None;
        }
        let (path, owner) = self.file();
        let work = if std::mem::take(&mut self.sweep) {
            Work::Sweep
        } else if self.seq > self.written && self.failed_at != Some(self.seq) {
            Work::Snapshot(if path.is_some() { self.book.text() } else { String::new() })
        } else {
            return None;
        };
        self.next_ticket += 1;
        self.out = Some(self.next_ticket);
        Some(Write { ticket: self.next_ticket, seq: self.seq, path, owner, work })
    }

    /// The file and its lock, owned; else neither (memory only).
    fn file(&self) -> (Option<PathBuf>, Option<Owned>) {
        match &self.owner {
            Ok(o) => (self.path.clone(), Some(o.clone())),
            Err(_) => (None, None),
        }
    }

    /// The window's last write, or the app's (it closes, it quits): memory
    /// whole if it changed since the file has, whatever write is out (the
    /// writer runs this one after it). What it does is no one's to hear.
    pub(crate) fn last_write(&mut self) -> Option<Write> {
        let (Some(path), Some(owner)) = self.file() else { return None };
        if self.seq <= self.written {
            return None;
        }
        self.next_ticket += 1;
        Some(Write {
            ticket: self.next_ticket,
            seq: self.seq,
            path: Some(path),
            owner: Some(owner),
            work: Work::Snapshot(self.book.text()),
        })
    }

    /// Tests: a write out (handed to the writer, not landed).
    #[cfg(test)]
    pub(crate) fn out(&self) -> bool {
        self.out.is_some()
    }

    /// Tests: memory changed since the file, and no write out to carry it
    /// (a change no write was asked for yet).
    #[cfg(test)]
    pub(crate) fn due(&self) -> bool {
        self.out.is_none() && self.seq > self.written && self.failed_at != Some(self.seq)
    }

    /// Whether a write is out or to make.
    #[cfg_attr(not(test), expect(dead_code, reason = "tests"))]
    pub(crate) fn writing(&self) -> bool {
        self.out.is_some() || self.sweep || (self.seq > self.written && self.failed_at != Some(self.seq))
    }

    /// Tests: nothing to sweep (a folder of its own, new).
    #[cfg(test)]
    pub(crate) fn swept(&mut self) {
        self.sweep = false;
    }

    /// The writer took no more (`why`: it didn't start, or stopped): the
    /// file isn't this desk's to write from now (memory only, and why), and
    /// memory is all there is: what a write out carried, and what changed
    /// since, lands as kept, and the messages waiting for it go.
    pub(crate) fn writer_lost(&mut self, why: &str) -> Vec<Landed> {
        if self.owner.is_ok() {
            self.owner = Err(NotOwned::Failed(why.to_string()));
        }
        self.sweep = false;
        let ticket = match self.out {
            Some(t) => t,
            None => {
                self.next_ticket += 1;
                self.next_ticket
            }
        };
        self.out = Some(ticket);
        self.landed(Landing { ticket, seq: self.seq, sweep: false, done: Ok(None) })
    }

    /// A write as it ran ([`Write::run`]): the wishes it carried draw (or,
    /// failed, are refused, what drew before drawn again), and the messages
    /// whose turn came go.
    pub(crate) fn landed(&mut self, l: Landing) -> Vec<Landed> {
        if self.out != Some(l.ticket) {
            return Vec::new();
        }
        self.out = None;
        let mut out = Vec::new();
        match l.done {
            Ok(said) if l.sweep => self.trouble = said.or(self.trouble.take()),
            Ok(_) => {
                self.written = l.seq;
                for (k, a) in self.take_asked(l.seq) {
                    out.push(Landed::Wish { conv: k.0, id: k.2, done: Ok(a.after.map_or(Done::Cancelled, |d| Done::Wished(d.version))) });
                }
            }
            Err(e) => {
                log::warn!(target: "synaps_desktop::blocks", "{FILE} not written: {e}");
                self.failed_at = Some(l.seq);
                let why = format!("not saved · {e}");
                for ((conv, g, id), a) in self.take_asked(l.seq) {
                    if same(self.book.get(&conv, &g, &id), a.after.as_ref()) {
                        // What drew, again (a revert that no longer fits
                        // keeps memory's).
                        let _ = self.commit(|b| b.set(&conv, &g, &id, a.before.clone()));
                    }
                    self.note_refusal((conv.clone(), id.clone()), why.clone());
                    out.push(Landed::Wish { conv, id, done: Err(Refused(why.clone())) });
                }
            }
        }
        out.extend(self.go());
        out
    }

    /// The wishes a snapshot of change `seq` carried, taken.
    fn take_asked(&mut self, seq: u64) -> Vec<(Key, Asked)> {
        let keys: Vec<Key> = self.asked.iter().filter(|(_, a)| a.at <= seq).map(|(k, _)| k.clone()).collect();
        keys.into_iter().map(|k| (k.clone(), self.asked.remove(&k).expect("listed"))).collect()
    }

    /// The messages whose turn came, in order: each once what it marked is
    /// written, with the notices memory still has so; plain once its write
    /// failed.
    fn go(&mut self) -> Vec<Landed> {
        let mut out = Vec::new();
        while let Some(s) = self.sending.front() {
            let written = s.at <= self.written;
            if !written && self.failed_at.is_none_or(|f| s.at > f) {
                break;
            }
            let s = self.sending.pop_front().expect("the first");
            let book = &self.book;
            let still = |(id, seen, v): &&(BlockId, Seen, u32)| {
                book.get(&s.conv, &s.generation, id).is_some_and(|d| d.version == *v && d.seen() == *seen && d.sent)
            };
            let rode: Vec<_> = if written { s.picks.iter().filter(still).cloned().collect() } else { Vec::new() };
            let ride = self.build(&s.body, s.carrier, &rode);
            // What doesn't ride isn't in flight.
            if let Some(slot) = self.book.slot_mut(&s.conv, &s.generation) {
                for (id, ..) in s.picks.iter().filter(|(id, ..)| !ride.notices.iter().any(|(i, ..)| i == id)) {
                    if let Some(d) = slot.get_mut(id).filter(|d| d.carrier == s.carrier) {
                        d.carrier = None;
                    }
                }
            }
            out.push(Landed::Send { ticket: s.ticket, ride });
        }
        out
    }

    /// `body` with the notices `rode` on `carrier`: its trailer, or the
    /// body alone.
    fn build(&self, body: &str, carrier: Option<u64>, rode: &[(BlockId, Seen, u32)]) -> Ride {
        let plain = Ride { text: body.to_string(), carrier: None, notices: Vec::new() };
        let Some(carrier) = carrier.filter(|_| !rode.is_empty()) else { return plain };
        let line = |kind: Seen| {
            let notices: Vec<Notice> =
                rode.iter().filter(|(_, k, _)| *k == kind).map(|(id, _, version)| Notice { id: id.clone(), version: *version }).collect();
            (!notices.is_empty()).then(|| NoticeLine::new(kind, notices).expect("≤ 8 ids, each once, versions in range"))
        };
        let lines: Vec<NoticeLine> = [line(Seen::Dismissed), line(Seen::Shown)].into_iter().flatten().collect();
        let trailer = Trailer::new(lines).expect("one line of each kind, an id in one");
        let text = format!("{body}{}", trailer.text());
        if message::split(&text) != (body, trailer) {
            return plain;
        }
        Ride { text, carrier: Some(carrier), notices: rode.to_vec() }
    }

    /// `change` made to memory if what it leaves fits: the reserve's one
    /// check, every change's. A change that doesn't grow the file (as
    /// written, and with every version at its widest) always fits; one
    /// that does is refused past [`RESERVE`]. A change to the file's text
    /// is one more to write.
    fn commit(&mut self, change: impl FnOnce(&mut Book)) -> Result<(), Refused> {
        let mut next = self.book.clone();
        change(&mut next);
        next.tidy();
        let (was, now) = (self.book.text(), next.text());
        if now.len() > was.len() && now.len() > RESERVE {
            return Err(Refused(format!("{FULL_DESK} ({} bytes, over {RESERVE})", now.len())));
        }
        let wide = next.widest_bytes();
        if wide > RESERVE && wide > self.book.widest_bytes() {
            return Err(Refused(format!("{FULL_DESK} ({wide} bytes at the widest, over {RESERVE})")));
        }
        if now != was {
            self.seq += 1;
        }
        self.book = next;
        Ok(())
    }

    fn key(conv: &str, generation: &Generation, id: &BlockId) -> Key {
        (conv.to_string(), generation_key(generation), id.clone())
    }

    /// `id`'s desire in `conv` (of `generation`'s history), as drawn: a
    /// wish not written yet draws what was before it.
    pub(crate) fn desire(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<&Desire> {
        match self.asked.get(&Self::key(conv, generation, id)) {
            Some(a) => a.before.as_ref(),
            None => self.book.get(conv, &generation_key(generation), id),
        }
    }

    /// `id`'s last wish in `conv` (of `generation`'s history), written or
    /// not: memory's.
    pub(crate) fn wish(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<&Desire> {
        self.book.get(conv, &generation_key(generation), id)
    }

    /// Whether `id` has a wish not written yet: it draws as before
    /// meanwhile (a pending look) and rides nothing.
    pub(crate) fn pending(&self, conv: &str, generation: &Generation, id: &BlockId) -> bool {
        self.asked.contains_key(&Self::key(conv, generation, id))
    }

    /// Why `id`'s last wish wasn't kept, until its next wish.
    #[cfg_attr(not(test), expect(dead_code, reason = "D7's block walk says it on the block"))]
    pub(crate) fn refused(&self, conv: &str, id: &BlockId) -> Option<&str> {
        self.refusals.get(&(conv.to_string(), id.clone())).map(|(_, why)| why.as_str())
    }

    /// Whether this desk draws `id` dismissed: its desire, else the shared
    /// record (`shared`: the board as drawn).
    pub(crate) fn dismissed(&self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> bool {
        self.desire(conv, generation, id).map_or_else(|| shared.dismissed(id), |d| d.dismissed)
    }

    /// Why `id` draws other than this desk wished: until it is seen
    /// ([`Desires::feedback_seen`]), the id's next wish, or its generation
    /// or conversation goes.
    #[cfg_attr(not(test), expect(dead_code, reason = "D7's block walk says it on the block"))]
    pub(crate) fn feedback(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<Feedback> {
        self.feedback.get(&Self::key(conv, generation, id)).map(|(_, f)| *f)
    }

    /// `id`'s feedback was shown: it goes.
    #[cfg_attr(not(test), expect(dead_code, reason = "D7's block walk says it on the block"))]
    pub(crate) fn feedback_seen(&mut self, conv: &str, generation: &Generation, id: &BlockId) {
        self.feedback.remove(&Self::key(conv, generation, id));
    }

    /// The side tables' sizes: feedback and refusals kept (tests; Settings).
    #[cfg_attr(not(test), expect(dead_code, reason = "tests"))]
    pub(crate) fn notes(&self) -> (usize, usize) {
        (self.feedback.len(), self.refusals.len())
    }

    /// Wishes and messages waiting for a write (≤ [`QUEUE_MAX`]).
    pub(crate) fn waiting_actions(&self) -> usize {
        self.asked.len() + self.sending.len()
    }

    fn note_feedback(&mut self, conv: &str, generation: &str, id: BlockId, f: Feedback) {
        self.next_note += 1;
        self.feedback.insert((conv.to_string(), generation.to_string(), id), (self.next_note, f));
        let oldest = |m: &BTreeMap<Key, (u64, Feedback)>, here: bool| {
            m.iter().filter(|((c, ..), _)| !here || c == conv).min_by_key(|(_, (n, _))| *n).map(|(k, _)| k.clone())
        };
        while self.feedback.keys().filter(|(c, ..)| c == conv).count() > FEEDBACK_PER_CONVERSATION {
            let k = oldest(&self.feedback, true).expect("over the cap");
            self.feedback.remove(&k);
        }
        while self.feedback.len() > FEEDBACK_MAX {
            let k = oldest(&self.feedback, false).expect("over the cap");
            self.feedback.remove(&k);
        }
    }

    fn note_refusal(&mut self, key: (String, BlockId), why: String) {
        self.next_note += 1;
        self.refusals.insert(key, (self.next_note, why));
        while self.refusals.len() > FEEDBACK_MAX {
            let k = self.refusals.iter().min_by_key(|(_, (n, _))| *n).map(|(k, _)| k.clone()).expect("over the cap");
            self.refusals.remove(&k);
        }
    }

    /// Every desire of every conversation, by conversation (every
    /// generation's): the `discard` list.
    #[cfg_attr(not(test), expect(dead_code, reason = "tests"))]
    pub(crate) fn waiting(&self) -> Vec<(String, usize)> {
        self.book.conversations.keys().map(|k| (k.clone(), self.book.conversation_len(k))).collect()
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "tests"))]
    pub(crate) fn len(&self) -> usize {
        self.book.len()
    }

    /// `highest`, seen on a board of `generation` of `conv` (or minted
    /// there).
    fn saw(&mut self, conv: &str, generation: &str, highest: u32) {
        self.next_note += 1;
        match self.known.get_mut(conv) {
            Some(k) if k.generation == generation => (k.high, k.touched) = (k.high.max(highest), self.next_note),
            // Not the settled history's generation: nothing of it is kept.
            Some(k) if k.settled.is_some() => {}
            _ => {
                let k = Known { generation: generation.to_string(), high: highest, settled: None, touched: self.next_note };
                self.known.insert(conv.to_string(), k);
                self.bound_known(conv);
            }
        }
    }

    /// `conv`'s settled history, at `generation` (another: the history
    /// moved on).
    fn know(&mut self, conv: &str, generation: &str, settled: &Visibility) {
        self.next_note += 1;
        let high = self.known.get(conv).filter(|k| k.generation == generation).map_or(0, |k| k.high);
        let k = Known {
            generation: generation.to_string(),
            high: high.max(settled.highest()),
            settled: Some(settled.clone()),
            touched: self.next_note,
        };
        self.known.insert(conv.to_string(), k);
        self.bound_known(conv);
    }

    /// What's known of a conversation without desires goes past
    /// [`KNOWN_MAX`], the least recently used first: nothing of this desk's
    /// may still land there, and a later board has the rest.
    fn bound_known(&mut self, conv: &str) {
        while self.known.len() > KNOWN_MAX {
            let book = &self.book;
            let Some(gone) = self
                .known
                .iter()
                .filter(|(c, _)| *c != conv && book.conversation_len(c) == 0)
                .min_by_key(|(_, k)| k.touched)
                .map(|(c, _)| c.clone())
            else {
                break;
            };
            self.known.remove(&gone);
        }
    }

    /// One more than every version this desk knows in `generation` of
    /// `conv`: `shared`'s highest, what it settled, saw and minted there,
    /// its desires'.
    fn mint(&self, conv: &str, generation: &str, shared: &Visibility) -> u32 {
        let own = self.book.slot(conv, generation).into_iter().flat_map(BTreeMap::values).map(|d| d.version).max().unwrap_or(0);
        let known = self.known.get(conv).filter(|k| k.generation == generation).map_or(0, |k| k.high);
        shared.highest().max(own).max(known).saturating_add(1)
    }

    /// Dismiss `id` in `conv` (§3.5). `shared`: the board as drawn, the
    /// live turn included. Refused at once, or decided: it draws when its
    /// write lands (module docs).
    pub(crate) fn dismiss(&mut self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> Result<Ask, Refused> {
        self.want(conv, generation, id, true, shared)
    }

    /// Show `id` again in `conv` (§3.5): as [`Desires::dismiss`].
    pub(crate) fn show(&mut self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> Result<Ask, Refused> {
        self.want(conv, generation, id, false, shared)
    }

    fn want(&mut self, conv: &str, generation: &Generation, id: &BlockId, dismiss: bool, shared: &Visibility) -> Result<Ask, Refused> {
        if self.path.is_none() {
            return Err(Refused("no config folder: the change couldn't be kept".into()));
        }
        if !key_ok(conv) {
            return Err(Refused("this conversation has no journal yet".into()));
        }
        let key = Self::key(conv, generation, id);
        let g = key.1.clone();
        if self.known.get(conv).is_some_and(|k| k.settled.is_some() && k.generation != g) {
            return Err(Refused(MOVED_ON.into()));
        }
        self.feedback.remove(&key);
        self.refusals.remove(&(conv.to_string(), id.clone()));
        self.saw(conv, &g, shared.highest());
        // Memory's: the last wish, written or not.
        let cur = self.book.get(conv, &g, id).cloned();
        if cur.as_ref().map_or_else(|| shared.dismissed(id), |d| d.dismissed) == dismiss {
            return Ok(Ask::Unchanged);
        }
        if !self.asked.contains_key(&key) && self.waiting_actions() >= QUEUE_MAX {
            return Err(Refused(BUSY.into()));
        }
        let settled = self.known.get(conv).and_then(|k| k.settled.as_ref());
        let agrees = |v: &Visibility| v.dismissed(id) == dismiss;
        let after = match &cur {
            // No notice of it may have gone, and every board this desk has
            // draws the wish: nothing to tell.
            Some(d) if !d.sent && agrees(shared) && settled.is_none_or(agrees) => None,
            _ => {
                let version = self.mint(conv, &g, shared);
                if version > VERSION_MAX {
                    return Err(Refused(TOO_MANY.into()));
                }
                Some(Desire::new(dismiss, version, cur.as_ref().is_some_and(|d| d.sent)))
            }
        };
        if cur.is_none() {
            if self.book.conversation_len(conv) >= PER_CONVERSATION {
                return Err(Refused(FULL_HERE.into()));
            }
            if self.book.len() >= PER_DESK {
                return Err(Refused(FULL_DESK.into()));
            }
        }
        let made = after.clone();
        self.commit(|b| b.set(conv, &g, id, made))?;
        if let Some(d) = &after {
            self.saw(conv, &g, d.version);
        }
        let before = self.asked.remove(&key).map_or(cur, |a| a.before);
        self.asked.insert(key, Asked { at: self.seq, before, after });
        Ok(Ask::Saving)
    }

    /// What the next message of `conv` carries, as the composer's chip
    /// shows it: each desire above `effective_version(id)`, not in flight,
    /// not kept back, no wish of its id pending; at most 8 of each kind,
    /// the oldest versions first.
    pub(crate) fn riding(&self, conv: &str, generation: &Generation, shared: &Visibility) -> Vec<(BlockId, Seen, u32)> {
        let Some(c) = self.book.slot(conv, &generation_key(generation)) else { return Vec::new() };
        let mut all: Vec<(BlockId, Seen, u32)> = c
            .iter()
            .filter(|(id, d)| !d.held && d.carrier.is_none() && d.version > shared.effective_version(id))
            .filter(|(id, _)| !self.pending(conv, generation, id))
            .map(|(id, d)| (id.clone(), d.seen(), d.version))
            .collect();
        all.sort_by_key(|(id, _, v)| (*v, id.clone()));
        let mut out = Vec::new();
        for kind in [Seen::Dismissed, Seen::Shown] {
            out.extend(all.iter().filter(|(_, s, _)| *s == kind).take(NOTICE_IDS_MAX).cloned());
        }
        out
    }

    /// The chip's `×`: `id`'s notice kept back from the next message (it
    /// rides the one after). False: nothing of `id` was riding.
    pub(crate) fn hold(&mut self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> bool {
        let riding = self.riding(conv, generation, shared).iter().any(|(i, ..)| i == id);
        match self.book.slot_mut(conv, &generation_key(generation)).and_then(|c| c.get_mut(id)) {
            Some(d) if riding => {
                d.held = true;
                true
            }
            _ => false,
        }
    }

    /// `body`, as it goes `how` on `conv`, with the notices that ride it
    /// (module docs). `Go`: now. `Wait`: when [`Landed::Send`] of the
    /// ticket lands, with the notices still so then; in flight on its
    /// carrier until then. Refused, nothing changed, when it would wait
    /// past the bounds ([`QUEUE_MAX`], [`QUEUE_BYTES_MAX`]): the composer
    /// keeps the text. `shared`: the board as drawn.
    pub(crate) fn ride(
        &mut self,
        conv: &str,
        generation: &Generation,
        shared: &Visibility,
        body: &str,
        how: Carry,
    ) -> Result<Riding, Refused> {
        let g = generation_key(generation);
        self.saw(conv, &g, shared.highest());
        let typed = how != Carry::Answer && !body.trim().is_empty();
        let mut picked = if typed { self.riding(conv, generation, shared) } else { Vec::new() };
        if !picked.is_empty() && self.build(body, Some(0), &picked).notices.is_empty() {
            // The body's own last line reads as a notice beside these: they
            // wait for the next message.
            picked.clear();
        }
        let marked = picked.iter().all(|(id, ..)| self.book.get(conv, &g, id).is_some_and(|d| d.sent));
        let wait = !self.sending.is_empty() || (!picked.is_empty() && (!marked || self.written < self.seq));
        let bytes: usize = self.sending.iter().map(|s| s.body.len()).sum();
        if wait && (self.waiting_actions() >= QUEUE_MAX || bytes + body.len() > QUEUE_BYTES_MAX) {
            return Err(Refused(BUSY.into()));
        }
        if typed && let Some(c) = self.book.slot_mut(conv, &g) {
            // What the chip kept back was kept from this message: the next
            // carries it.
            c.values_mut().for_each(|d| d.held = false);
        }
        let carrier = (!picked.is_empty()).then(|| {
            self.next_carrier += 1;
            self.next_carrier
        });
        if let Some(c) = self.book.slot_mut(conv, &g) {
            for (id, ..) in &picked {
                if let Some(d) = c.get_mut(id) {
                    d.carrier = carrier;
                }
            }
        }
        if !wait {
            return Ok(Riding::Go(self.build(body, carrier, &picked)));
        }
        let marks = picked.clone();
        self.commit(|b| {
            for (id, ..) in &marks {
                if let Some(d) = b.slot_mut(conv, &g).and_then(|s| s.get_mut(id)) {
                    d.sent = true;
                }
            }
        })
        .expect("a sent mark doesn't grow the file");
        self.next_send += 1;
        let at = if picked.is_empty() { 0 } else { self.seq };
        if at > self.written && self.failed_at == Some(self.seq) {
            // The file failed at this very state: a message waiting for it
            // tries it again (and goes plain if it fails again).
            self.failed_at = None;
        }
        self.sending.push_back(Sending {
            ticket: self.next_send,
            at,
            body: body.to_string(),
            carrier,
            conv: conv.into(),
            generation: g,
            picks: picked,
        });
        Ok(Riding::Wait(self.next_send))
    }

    /// `carrier` won't land (a refused send): what it carried rides the
    /// next message at the same version.
    pub(crate) fn lost(&mut self, carrier: u64) {
        for d in self.book.desires_mut() {
            if d.carrier == Some(carrier) {
                d.carrier = None;
            }
        }
    }

    /// `conv`'s desires against a settled history's board (`shared`), the
    /// carriers still in flight (`in_flight`: sent, and not yet shown by a
    /// settled history nor lost) given: what's done is dropped, with why it
    /// draws otherwise if it does; a carrier no longer in flight is
    /// cleared, its desires re-armed. Every other generation of `conv` is
    /// dropped, its wishes not written retired (never reported). A write
    /// that failed is tried again.
    pub(crate) fn settle(
        &mut self,
        conv: &str,
        generation: &Generation,
        shared: &Visibility,
        in_flight: &[u64],
    ) -> Vec<(BlockId, Feedback)> {
        let g = generation_key(generation);
        self.know(conv, &g, shared);
        self.asked.retain(|(c, ag, _), _| c != conv || *ag == g);
        self.feedback.retain(|(c, fg, _), _| c != conv || *fg == g);
        let waiting: BTreeSet<u64> = self.sending.iter().filter_map(|s| s.carrier).collect();
        let mut out = Vec::new();
        let _ = self.commit(|b| {
            if let Some(gens) = b.conversations.get_mut(conv) {
                gens.retain(|k, _| *k == g);
            }
            let Some(slot) = b.slot_mut(conv, &g) else { return };
            slot.retain(|id, d| {
                if shared.effective_version(id) >= d.version {
                    let why = match shared.record(id) {
                        Some(r) if r.dismissed != d.dismissed => Some(Feedback::ChangedByLater),
                        None if d.dismissed => Some(Feedback::NotApplied),
                        _ => None,
                    };
                    out.extend(why.map(|f| (id.clone(), f)));
                    return false;
                }
                if d.carrier.is_some_and(|k| !in_flight.contains(&k) && !waiting.contains(&k)) {
                    d.carrier = None;
                }
                true
            });
        });
        for (id, f) in &out {
            self.note_feedback(conv, &g, id.clone(), *f);
        }
        // A write that failed is tried again (the disk may be back).
        self.failed_at = None;
        out
    }

    /// `conv` was compacted (`generation`, `shared`: its history's records
    /// at the end): its
    /// generation's desires dropped, and the ids this desk drew dismissed
    /// returned (the successor's soft dismissals): every id the board's
    /// visibility records ([`Visibility::records`]) or a desire names,
    /// drawn dismissed. An id drawn dismissed has one or the other, so
    /// none is missed.
    pub(crate) fn compacted(&mut self, conv: &str, generation: &Generation, shared: &Visibility) -> Vec<BlockId> {
        let g = generation_key(generation);
        let mut ids: BTreeSet<BlockId> = shared.records().filter(|(_, r)| r.dismissed).map(|(id, _)| id.clone()).collect();
        ids.extend(self.book.slot(conv, &g).into_iter().flat_map(BTreeMap::keys).cloned());
        ids.extend(self.asked.keys().filter(|(c, ag, _)| c == conv && *ag == g).map(|(.., id)| id.clone()));
        let out: Vec<BlockId> = ids.into_iter().filter(|id| self.dismissed(conv, generation, id, shared)).collect();
        self.asked.retain(|(c, ag, _), _| !(c == conv && *ag == g));
        self.feedback.retain(|(c, ..), _| c != conv);
        self.refusals.retain(|(c, _), _| c != conv);
        let _ = self.commit(|b| {
            if let Some(gens) = b.conversations.get_mut(conv) {
                gens.remove(&g);
            }
        });
        out
    }

    /// The user's discard: `conv`'s unsent desires dropped, every
    /// generation's (a desire that may have been sent stays until it
    /// settles); how many.
    #[cfg_attr(not(test), expect(dead_code, reason = "the full desk's discard, with D7"))]
    pub(crate) fn discard(&mut self, conv: &str) -> usize {
        let before = self.book.conversation_len(conv);
        self.feedback.retain(|(c, ..), _| c != conv);
        self.refusals.retain(|(c, _), _| c != conv);
        let _ = self.commit(|b| {
            for s in b.conversations.get_mut(conv).into_iter().flat_map(BTreeMap::values_mut) {
                s.retain(|_, d| d.sent);
            }
        });
        // A wish whose desire went is retired.
        let book = &self.book;
        self.asked.retain(|(c, g, id), a| c != conv || same(book.get(c, g, id), a.after.as_ref()));
        before - self.book.conversation_len(conv)
    }
}

#[cfg(test)]
#[path = "desires_tests.rs"]
pub(crate) mod tests;
