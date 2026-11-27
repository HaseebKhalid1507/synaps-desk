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
//! **The file decides; memory draws what it decided.** Every change is a
//! transaction on the file as it is, under its lock ([`crate::blockfiles`]),
//! never on what this process last read of it: two desk processes share
//! it, and either may have sent, superseded, cancelled or settled since.
//! The transactions run off the UI thread, one at a time, in the order
//! they were made ([`Desires::next_write`], [`Write::run`],
//! [`Desires::landed`]); nothing else here touches the disk but
//! [`Desires::load`]'s one read.
//!
//! - **A wish** ([`Desires::dismiss`], [`Desires::show`]) is checked in
//!   memory first (its caps: refused at once, said, [`Refused`]), then
//!   decided against the file: already so (unchanged); cancelled, when the
//!   desire it undoes is one no notice of the id may have gone for and
//!   what every desk draws is the wish; else a new desire at one more than
//!   every version seen (the board's `highest`, and the highest the file
//!   ever minted in that generation, settled desires' included), which
//!   inherits the old one's sent mark. What the click's board drew decides
//!   a cancel (or that nothing changes) only if that board had seen every
//!   settled history the file knows of; a wish made on an older board is a
//!   notice, whatever it drew. The caps are
//!   checked there too, on the file as the wish leaves it: a new id past
//!   [`PER_CONVERSATION`] or [`PER_DESK`], or the file past [`RESERVE`]
//!   bytes as written or with every version at its widest (so a desire
//!   admitted can always be superseded), is refused. Until it lands the id
//!   draws as it did ([`Desires::pending`]: a pending look), rides
//!   nothing, and a next wish on it is decided against this one. Landed,
//!   it draws; refused or failed (the lock not had in
//!   [`blockfiles::LOCK_WAIT`], the disk), nothing drew, and the id says
//!   why ([`Desires::refused`]). So a restart never loses a wish the user
//!   saw drawn.
//! - **Sent means marked first** ([`Desires::ride`]): a message carrying
//!   notices waits for its transaction, which marks each in the file as
//!   sent if the file still has it so, and only those ride. A notice so
//!   marked stays an obligation through every supersession of its id: an
//!   older copy may still land, so the other wish over it is a new version,
//!   never a cancel. The mark goes only with the desire, at its settling,
//!   where every older copy is stale.
//! - **Riding**: at every send of typed text (a `Submit` when idle, a
//!   `Steer` while a turn runs) or a hand-back, each desire above
//!   `effective_version(id)` that isn't in flight, kept back or pending is
//!   attached, as the message's last lines: at most 8 ids of each kind,
//!   dismissals first, the oldest versions first; the rest ride the next
//!   message. **Never on its own** (an empty body carries nothing) and
//!   **never on a card answer** ([`Carry::Answer`]). A body whose own last
//!   line would read as a notice beside the trailer carries nothing: no
//!   message is ever built whose trailer isn't exactly this desk's. Every
//!   send builds its trailer from the desires at that moment, a Resend
//!   included. Messages leave in the order they were made: one made while
//!   another waits for its mark waits behind it.
//! - **Settling** ([`Desires::settle`], on a settled history's board): a
//!   desire at or below `effective_version(id)` is done and dropped (now,
//!   and from the file at its transaction: whichever desk's it is); if the
//!   shared state differs, the block says why ([`Feedback`], bounded, until
//!   seen or the next wish). A carrier the caller no longer has in flight
//!   (a dropped steer, a refused send, a rollback: [`Desires::lost`] too)
//!   is cleared, and the desire rides the next message at the same version.
//! - **Generations are fenced.** Versions count within a generation: every
//!   transaction touches only the generation it names. One is dropped only
//!   by a process that saw its journal's history move past it (its settle
//!   of another generation after one of it), or at its first settle of the
//!   conversation (the attach: the history it was handed is the daemon's
//!   now), and only that generation's entry, as it is then.
//! - **A restart** ([`Desires::load`]) keeps the desires and their marks,
//!   and loses the carriers; the first settle against the attached history
//!   drops what it shows, and the rest ride again. A copy still queued in
//!   the daemon may land as well: the second of two equal versions is
//!   stale.
//! - **A compaction** ([`Desires::compacted`]) drops the generation's
//!   desires, and hands back the ids this desk drew dismissed: the
//!   successor's soft dismissals ([`crate::presentation`]).
//!
//! **The file**, `block-desires.json` next to `settings.json`: per
//! conversation, per generation (the first 16 hex digits), its desires,
//! ≤ [`ENTRY_MAX`] bytes each, and the generation's marks: the highest
//! version minted there, the highest a settled history had, and whether a
//! discarded notice may still land ([`Book`]). A generation whose desires
//! all went keeps only its marks, the latest [`SETTLED_MAX`] of them,
//! which give way to a desire when the reserve is short. **No cap evicts a
//! desire**, and a file over one (another writer's) is read whole: new ids
//! are refused until it is under again. A desire leaves only when it
//! settles, when the user cancels or discards it, at a compaction, or with
//! its generation. A discard drops desires, never a generation's marks: a
//! notice it dropped may have gone, so no later wish there cancels.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use cards_spec::block::message::{self, NOTICE_IDS_MAX, Notice, NoticeLine, Trailer, VERSION_MAX, Visibility as Seen};
use cards_spec::block::{BlockId, Generation};
use cards_spec::board::{State, Visibility};
use serde_json::{Value, json};

use crate::blockfiles::{self, Stored};

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
pub(crate) const ENTRY_MAX: usize = 64;
/// A conversation's key (its journal id), bytes at most.
pub(crate) const KEY_MAX: usize = 128;
/// Writes waiting at most: past it a wish is refused (the disk is slow).
pub(crate) const QUEUE_MAX: usize = 64;
/// Settled feedback kept per conversation, and in all, at most: the
/// oldest goes first (an explanation, not a wish).
pub(crate) const FEEDBACK_PER_CONVERSATION: usize = PER_CONVERSATION;
pub(crate) const FEEDBACK_MAX: usize = PER_DESK;
/// Conversations whose last settled generation this process keeps.
const SEEN_MAX: usize = 256;
/// Settled marks kept at most ([`Book`]).
const SETTLED_MAX: usize = 64;
/// The file's format.
const FORMAT: u64 = 2;

/// The refusals' words (§1.8).
pub(crate) const FULL_HERE: &str = "64 changes waiting · send a message first";
pub(crate) const FULL_DESK: &str = "512 changes waiting · send in another conversation, or discard";
pub(crate) const BUSY: &str = "still saving earlier changes · try again";
pub(crate) const MOVED: &str = "changed meanwhile · try again";

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
    /// superseded, and may still land: the other wish over it is a new
    /// version, never a cancel (module docs).
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

/// One generation's desires.
type Slot = BTreeMap<BlockId, Desire>;

/// One generation of a conversation in the file: its desires; the
/// highest version this file ever minted there (`top`), kept as desires
/// settle or are discarded, so a wish made on a board older than a settle
/// still mints above every desk's notice of that generation; the highest
/// version a settled history had (`landed`): a wish whose board had seen
/// less was made on a stale board, and isn't decided by what that board
/// drew; and whether the user discarded a notice that may have been sent
/// (`owed`): one may still land, so no wish of the generation cancels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Gen {
    top: u32,
    landed: u32,
    owed: bool,
    desires: Slot,
}

/// A generation whose desires all went: what it keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Mark {
    conv: String,
    generation: String,
    top: u32,
    landed: u32,
    owed: bool,
}

/// Every conversation's desires, per generation: the file's content, and
/// memory's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Book {
    conversations: BTreeMap<String, BTreeMap<String, Gen>>,
    /// The `top` of generations whose desires all settled: the latest
    /// [`SETTLED_MAX`], oldest first. Evictable (a click on a board older
    /// than that settle is all they guard): they give way to a desire.
    settled: VecDeque<Mark>,
}

impl Book {
    fn len(&self) -> usize {
        self.conversations.keys().map(|c| self.conversation_len(c)).sum()
    }

    fn conversation_len(&self, conv: &str) -> usize {
        self.conversations.get(conv).map_or(0, |g| g.values().map(|x| x.desires.len()).sum())
    }

    fn generations(&self, conv: &str) -> Vec<String> {
        self.conversations.get(conv).map(|g| g.keys().cloned().collect()).unwrap_or_default()
    }

    fn slot(&self, conv: &str, generation: &str) -> Option<&Slot> {
        Some(&self.conversations.get(conv)?.get(generation)?.desires)
    }

    fn slot_mut(&mut self, conv: &str, generation: &str) -> Option<&mut Slot> {
        Some(&mut self.conversations.get_mut(conv)?.get_mut(generation)?.desires)
    }

    /// `generation` of `conv`, made if none (its `top` back from the
    /// settled marks).
    fn generation_mut(&mut self, conv: &str, generation: &str) -> &mut Gen {
        let (top, landed, owed) = (self.top(conv, generation), self.landed(conv, generation), self.owed(conv, generation));
        self.settled.retain(|m| !(m.conv == conv && m.generation == generation));
        let here = self.conversations.entry(conv.to_string()).or_default().entry(generation.to_string()).or_default();
        here.top = here.top.max(top);
        here.landed = here.landed.max(landed);
        here.owed |= owed;
        here
    }

    /// A notice of `generation` the user discarded may still land.
    fn owed(&self, conv: &str, generation: &str) -> bool {
        let live = self.conversations.get(conv).and_then(|g| g.get(generation)).is_some_and(|x| x.owed);
        live || self.mark(conv, generation).is_some_and(|m| m.owed)
    }

    /// The user's discard of `gens` of `conv`: their desires go, what they
    /// minted and owe stays.
    fn discard(&mut self, conv: &str, gens: &[String]) {
        for (_, x) in self.conversations.get_mut(conv).into_iter().flatten().filter(|(g, _)| gens.contains(g)) {
            x.owed |= x.desires.values().any(|d| d.sent);
            x.desires.clear();
        }
        self.tidy();
    }

    fn mark(&self, conv: &str, generation: &str) -> Option<&Mark> {
        self.settled.iter().find(|m| m.conv == conv && m.generation == generation)
    }

    /// The highest version minted in `generation` of `conv`.
    fn top(&self, conv: &str, generation: &str) -> u32 {
        let live = self.conversations.get(conv).and_then(|g| g.get(generation)).map_or(0, |x| x.top);
        live.max(self.mark(conv, generation).map_or(0, |m| m.top))
    }

    /// The highest version a settled history of `generation` had.
    fn landed(&self, conv: &str, generation: &str) -> u32 {
        let live = self.conversations.get(conv).and_then(|g| g.get(generation)).map_or(0, |x| x.landed);
        live.max(self.mark(conv, generation).map_or(0, |m| m.landed))
    }

    /// A settled history of `generation` had `highest`.
    fn note_landed(&mut self, conv: &str, generation: &str, highest: u32) {
        if let Some(x) = self.conversations.get_mut(conv).and_then(|g| g.get_mut(generation)) {
            x.landed = x.landed.max(highest);
        } else if let Some(m) = self.settled.iter_mut().find(|m| m.conv == conv && m.generation == generation) {
            m.landed = m.landed.max(highest);
        }
    }

    fn desires_mut(&mut self) -> impl Iterator<Item = &mut Desire> {
        self.conversations.values_mut().flat_map(|g| g.values_mut()).flat_map(|x| x.desires.values_mut())
    }

    /// `gens` of `conv` gone, their settled marks too.
    fn drop_generations(&mut self, conv: &str, gens: &[String]) {
        if let Some(g) = self.conversations.get_mut(conv) {
            g.retain(|k, _| !gens.contains(k));
        }
        self.settled.retain(|m| !(m.conv == conv && gens.contains(&m.generation)));
        self.tidy();
    }

    /// A generation whose desires all went keeps only its settled mark; no
    /// empty conversation kept; at most [`SETTLED_MAX`] marks.
    fn tidy(&mut self) {
        for (conv, gens) in &mut self.conversations {
            for (g, x) in gens.iter().filter(|(_, x)| x.desires.is_empty()) {
                self.settled.retain(|m| !(m.conv == *conv && m.generation == *g));
                self.settled.push_back(Mark { conv: conv.clone(), generation: g.clone(), top: x.top, landed: x.landed, owed: x.owed });
            }
            gens.retain(|_, x| !x.desires.is_empty());
        }
        self.conversations.retain(|_, g| !g.is_empty());
        while self.settled.len() > SETTLED_MAX {
            self.settled.pop_front();
        }
    }

    /// The file's bytes with every version at its widest: what the desires
    /// in it may grow to by supersession alone.
    fn widest_bytes(&self) -> usize {
        let mut wide = self.clone();
        wide.desires_mut().for_each(|d| d.version = VERSION_MAX);
        wide.conversations.values_mut().flat_map(|g| g.values_mut()).for_each(|x| (x.top, x.landed) = (VERSION_MAX, VERSION_MAX));
        wide.settled.iter_mut().for_each(|m| (m.top, m.landed) = (VERSION_MAX, VERSION_MAX));
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
        let version = |x: &Value| x.as_u64().filter(|v| (1..=u64::from(VERSION_MAX)).contains(v)).map(|v| v as u32);
        let (mut book, mut bad) = (Book::default(), 0);
        for (key, gens) in convs {
            let Some(gens) = gens.as_object().filter(|_| key_ok(key)) else {
                bad += 1;
                continue;
            };
            for (g, x) in gens {
                let landed = x["landed"].as_u64().filter(|v| *v <= u64::from(VERSION_MAX)).map(|v| v as u32);
                let owed = x["owed"].as_u64().filter(|b| *b <= 1).map(|b| b == 1);
                let (Some(top), Some(landed), Some(owed), Some(ds), true) =
                    (version(&x["top"]), landed, owed, x["desires"].as_object(), gen_ok(g))
                else {
                    bad += 1;
                    continue;
                };
                let mut desires = Slot::new();
                for (id, d) in ds {
                    let bit = |k: &str| d[k].as_u64().filter(|b| *b <= 1).map(|b| b == 1);
                    match (BlockId::parse(id), bit("d"), version(&d["v"]), bit("s")) {
                        (Some(id), Some(dismissed), Some(v), Some(sent)) => {
                            desires.insert(id, Desire::new(dismissed, v, sent));
                        }
                        _ => bad += 1,
                    }
                }
                let top = desires.values().map(|d| d.version).fold(top, u32::max);
                book.conversations.entry(key.clone()).or_default().insert(g.clone(), Gen { top, landed, owed, desires });
            }
        }
        for m in v["settled"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let landed = m[3].as_u64().filter(|v| *v <= u64::from(VERSION_MAX)).map(|v| v as u32);
            let owed = m[4].as_u64().filter(|b| *b <= 1).map(|b| b == 1);
            match (m[0].as_str().filter(|k| key_ok(k)), m[1].as_str().filter(|g| gen_ok(g)), version(&m[2]), landed, owed) {
                (Some(c), Some(g), Some(top), Some(landed), Some(owed)) => {
                    book.settled.push_back(Mark { conv: c.to_string(), generation: g.to_string(), top, landed, owed });
                }
                _ => bad += 1,
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
                    .map(|(g, x)| {
                        let ds: serde_json::Map<String, Value> = x
                            .desires
                            .iter()
                            .map(|(id, d)| (id.to_string(), json!({"d": u8::from(d.dismissed), "v": d.version, "s": u8::from(d.sent)})))
                            .collect();
                        (g.clone(), json!({"top": x.top, "landed": x.landed, "owed": u8::from(x.owed), "desires": ds}))
                    })
                    .collect();
                (key.clone(), Value::Object(gens))
            })
            .collect();
        let settled: Vec<Value> = self.settled.iter().map(|m| json!([m.conv, m.generation, m.top, m.landed, u8::from(m.owed)])).collect();
        json!({"version": FORMAT, "conversations": convs, "settled": settled}).to_string()
    }
}

/// A wish as the user made it, with what the board drew then: decided
/// against the file ([`decide`]).
#[derive(Clone, Debug)]
struct Wish {
    conv: String,
    generation: String,
    id: BlockId,
    dismiss: bool,
    /// The shared record's state, on the board as drawn.
    shared_dismissed: bool,
    /// The board's highest version seen.
    highest: u32,
    /// The desk drew a desire of the id at the click (its memory).
    had: bool,
}

/// `w` decided against `book` (the file as it is), and made in it: what it
/// did, or why it was refused (`book` then left to be dropped unwritten).
fn decide(book: &mut Book, w: &Wish) -> Result<Done, String> {
    let cur = book.slot(&w.conv, &w.generation).and_then(|s| s.get(&w.id)).cloned();
    if cur.as_ref().is_some_and(|d| d.dismissed == w.dismiss) {
        return Ok(Done::Unchanged);
    }
    // The click's board had seen less than a settled history, or a notice
    // the user discarded may still land: what the board drew decides
    // nothing, and the wish is a notice.
    let stale = w.highest < book.landed(&w.conv, &w.generation) || book.owed(&w.conv, &w.generation);
    if let Some(d) = &cur
        && !d.sent
        && !stale
        && w.shared_dismissed == w.dismiss
    {
        // No notice of it may have gone, and what every desk draws is the
        // wish: nothing to tell.
        if let Some(s) = book.slot_mut(&w.conv, &w.generation) {
            s.remove(&w.id);
        }
        book.tidy();
        return Ok(Done::Cancelled);
    }
    if cur.is_none() && !w.had && !stale && w.shared_dismissed == w.dismiss {
        // Nothing of it here, nor in the memory the click saw: what every
        // desk draws is the wish (an earlier wish of this desk refused).
        return Ok(Done::Unchanged);
    }
    // Above every notice of the generation this file minted, settled ones
    // included, and every one the board had seen.
    let version = w.highest.max(book.top(&w.conv, &w.generation)).saturating_add(1);
    if version > VERSION_MAX {
        return Err("too many changes in this conversation".into());
    }
    // An older notice may still land: the obligation goes on.
    let sent = cur.as_ref().is_some_and(|d| d.sent);
    let here = book.generation_mut(&w.conv, &w.generation);
    here.top = version;
    here.desires.insert(w.id.clone(), Desire::new(w.dismiss, version, sent));
    if cur.is_none() {
        if book.conversation_len(&w.conv) > PER_CONVERSATION {
            return Err(FULL_HERE.into());
        }
        if book.len() > PER_DESK {
            return Err(FULL_DESK.into());
        }
        // Settled marks give way to a desire; past them, refused.
        while book.widest_bytes() > RESERVE && book.settled.pop_front().is_some() {}
        let n = book.widest_bytes();
        if n > RESERVE {
            return Err(format!("{FULL_DESK} ({n} bytes at the widest, over {RESERVE})"));
        }
    }
    while book.text().len() > RESERVE && book.settled.pop_front().is_some() {}
    let n = book.text().len();
    if n > RESERVE {
        return Err(format!("{FULL_DESK} ({n} bytes, over {RESERVE})"));
    }
    Ok(Done::Wished(version))
}

/// `picks` (id, state, version) marked sent in `book` where it still has
/// them so: those, which alone may ride.
fn mark_sent(book: &mut Book, conv: &str, generation: &str, picks: &[(BlockId, Seen, u32)]) -> Vec<(BlockId, Seen, u32)> {
    let Some(slot) = book.slot_mut(conv, generation) else { return Vec::new() };
    let mut out = Vec::new();
    for (id, seen, version) in picks {
        if let Some(d) = slot.get_mut(id).filter(|d| d.version == *version && d.seen() == *seen) {
            d.sent = true;
            out.push((id.clone(), *seen, *version));
        }
    }
    out
}

/// `generation`'s desires of `conv` settled by `shared` dropped, and the
/// generations `gone` (only those) of `conv` dropped whole.
fn settle_in(book: &mut Book, conv: &str, generation: &str, shared: &Visibility, gone: &[String]) {
    book.drop_generations(conv, gone);
    book.note_landed(conv, generation, shared.highest());
    if let Some(slot) = book.slot_mut(conv, generation) {
        slot.retain(|id, d| shared.effective_version(id) < d.version);
    }
    book.tidy();
}

/// The generations `gens` (only those) of `conv` dropped; or, the user's
/// discard, their desires.
fn drop_in(book: &mut Book, conv: &str, gens: &[String], discard: bool) {
    if discard {
        book.discard(conv, gens);
    } else {
        book.drop_generations(conv, gens);
    }
}

/// A transaction on the file ([`Write::run`]).
#[derive(Clone, Debug)]
enum Work {
    /// The launch's: what crashes left beside the file swept, and the file
    /// read again under its lock.
    Sweep,
    Wish(Wish),
    /// The notices of a message marked sent: those the file still has so.
    Send {
        conv: String,
        generation: String,
        picks: Vec<(BlockId, Seen, u32)>,
    },
    /// What a settled history shows done, and the generations seen gone.
    Settle {
        conv: String,
        generation: String,
        shared: Visibility,
        gone: Vec<String>,
    },
    /// Generations dropped (a compaction, another history), or their
    /// desires (the user's discard).
    Drop {
        conv: String,
        gens: Vec<String>,
        discard: bool,
    },
}

/// What a transaction did.
#[derive(Debug)]
enum Ran {
    Read,
    Wish(Result<Done, Refused>),
    Sent(Result<Vec<(BlockId, Seen, u32)>, String>),
    Dropped(Result<(), String>),
}

/// A write of the file, to run off the UI thread ([`Write::run`]); what it
/// returns goes back to [`Desires::landed`].
#[derive(Debug)]
pub(crate) struct Write {
    ticket: u64,
    path: PathBuf,
    work: Work,
}

/// A [`Write`] as it ran.
#[derive(Debug)]
pub(crate) struct Landing {
    ticket: u64,
    /// The file as the write left it (or found it, refused): memory's now.
    file: Option<Book>,
    ran: Ran,
    /// Why the file read as empty, or what the sweep couldn't remove.
    trouble: Option<String>,
}

impl Write {
    /// The transaction, on whatever thread runs it: the file's lock (waited
    /// for up to [`blockfiles::LOCK_WAIT`]), the file as it is now, the
    /// change decided against it, the sync and the rename.
    pub(crate) fn run(self) -> Landing {
        let path = &self.path;
        let mut out = Landing { ticket: self.ticket, file: None, ran: Ran::Read, trouble: None };
        match self.work {
            Work::Sweep => {
                let (book, why, swept) = blockfiles::load::<Book>(path);
                out.file = Some(book);
                out.trouble = why.or(swept.err().map(|e| e.to_string()));
            }
            // Nothing to mark: it only waits its turn.
            Work::Send { picks, .. } if picks.is_empty() => out.ran = Ran::Sent(Ok(Vec::new())),
            Work::Send { conv, generation, picks } => {
                let mut rode = Vec::new();
                let done = blockfiles::update::<Book>(path, |b| {
                    rode = mark_sent(b, &conv, &generation, &picks);
                    Ok(())
                });
                out.ran = Ran::Sent(done.as_ref().map(|_| rode).map_err(ToString::to_string));
                out.file = done.ok();
            }
            Work::Wish(w) => {
                let (mut did, mut found) = (None, None);
                let done = blockfiles::update::<Book>(path, |b| {
                    let was = b.clone();
                    let r = decide(b, &w);
                    if r.is_err() {
                        found = Some(was);
                    }
                    did = r.as_ref().ok().copied();
                    r.map(|_| ())
                });
                out.ran = Ran::Wish(match &done {
                    Ok(_) => Ok(did.expect("decided")),
                    Err(e) => Err(Refused(e.to_string())),
                });
                out.file = done.ok().or(found);
            }
            Work::Settle { conv, generation, shared, gone } => {
                let done = blockfiles::update::<Book>(path, |b| {
                    settle_in(b, &conv, &generation, &shared, &gone);
                    Ok(())
                });
                out.ran = Ran::Dropped(done.as_ref().map(|_| ()).map_err(ToString::to_string));
                out.file = done.ok();
            }
            Work::Drop { conv, gens, discard } => {
                let done = blockfiles::update::<Book>(path, |b| {
                    drop_in(b, &conv, &gens, discard);
                    Ok(())
                });
                out.ran = Ran::Dropped(done.as_ref().map(|_| ()).map_err(ToString::to_string));
                out.file = done.ok();
            }
        }
        out
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
    HandBack,
    /// A card answer: all grammar, never a notice (§3.5).
    Answer,
}

/// What a dismiss or a show asked, at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ask {
    /// This desk already draws it so (or a wish not landed will).
    Unchanged,
    /// Its transaction queued: it draws when that lands.
    Saving,
}

/// What a wish did, decided against the file and drawn ([`Landed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Done {
    /// The file had it so already.
    Unchanged,
    /// A desire no notice went for, cancelled: nothing will be sent.
    Cancelled,
    /// A new desire at this version: it rides the next message.
    Wished(u32),
}

/// A wish refused before it drew: why, in the user's words.
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

/// A message asked of [`Desires::ride`]: to go now, or once its notices
/// are marked sent ([`Landed::Send`] with this ticket).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Riding {
    Go(Ride),
    Wait(u64),
}

/// A transaction landed ([`Desires::landed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Landed {
    /// A wish: what it did, drawn now; or why it was refused, nothing drawn.
    Wish { conv: String, id: BlockId, done: Result<Done, Refused> },
    /// A message to send now, in this order: [`Riding::Wait`]'s.
    Send { ticket: u64, ride: Ride },
}

/// A transaction queued, the first out once [`Desires::next_write`] gave it.
#[derive(Clone, Debug)]
struct Queued {
    ticket: u64,
    work: Work,
}

/// The user's latest wish for an id, its transaction not landed: what a
/// next wish is decided against. `wish`: the desire it likely makes
/// (`None`: a cancel; its version is the file's to mint).
#[derive(Clone, Debug)]
struct Asked {
    ticket: u64,
    generation: String,
    wish: Option<Desire>,
}

/// A message waiting for its notices' mark.
#[derive(Clone, Debug)]
struct Sending {
    body: String,
    carrier: Option<u64>,
    conv: String,
    generation: String,
}

/// This desk's desires, every conversation's (module docs).
pub(crate) struct Desires {
    path: Option<PathBuf>,
    /// What draws: the file as it last landed, what memory dropped since
    /// (settled, discarded) applied.
    book: Book,
    next_carrier: u64,
    /// Settled feedback per (conversation, generation, id), with its order:
    /// bounded ([`FEEDBACK_PER_CONVERSATION`], [`FEEDBACK_MAX`]).
    feedback: BTreeMap<(String, String, BlockId), (u64, Feedback)>,
    /// Why a wish's transaction refused it, until the next wish on the id:
    /// bounded as the feedback.
    refusals: BTreeMap<(String, BlockId), (u64, String)>,
    next_note: u64,
    /// Wishes not yet landed, per id: the latest.
    asked: BTreeMap<(String, BlockId), Asked>,
    /// Messages waiting for their mark, by ticket.
    sending: BTreeMap<u64, Sending>,
    /// Per conversation, the generation of the last history this process
    /// settled ([`SEEN_MAX`] at most).
    seen: BTreeMap<String, String>,
    /// Transactions not yet landed, in order; the first is out when `out`.
    queue: VecDeque<Queued>,
    out: bool,
    next_ticket: u64,
    /// Why the file read as empty at the load, or what its sweep couldn't
    /// remove (Settings says it).
    pub(crate) trouble: Option<String>,
}

impl Desires {
    /// The desires in the file at `path`, read now, without its lock (a
    /// rename is atomic); the sweep of what crashes left beside it is the
    /// first write. `None`: no config folder, and every wish is refused: it
    /// couldn't be kept.
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let (book, trouble) = match &path {
            Some(p) => blockfiles::peek::<Book>(p),
            None => (Book::default(), None),
        };
        let mut d = Desires {
            path,
            book,
            next_carrier: 0,
            feedback: BTreeMap::new(),
            refusals: BTreeMap::new(),
            next_note: 0,
            asked: BTreeMap::new(),
            sending: BTreeMap::new(),
            seen: BTreeMap::new(),
            queue: VecDeque::new(),
            out: false,
            next_ticket: 0,
            trouble,
        };
        if d.path.is_some() {
            d.queue_write(Work::Sweep);
        }
        d
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn queue_write(&mut self, work: Work) -> u64 {
        // A settle or a drop not yet out takes the next of its conversation:
        // effective versions only grow, so the later settle drops a superset.
        let waiting = self.queue.len() > usize::from(self.out);
        if let (Some(back), true) = (self.queue.back_mut(), waiting) {
            match (&mut back.work, &work) {
                (Work::Settle { conv: c, generation: g, shared: s, gone: gone_then }, Work::Settle { conv, generation, shared, gone })
                    if c == conv && g == generation =>
                {
                    s.clone_from(shared);
                    gone_then.extend(gone.iter().filter(|x| !gone_then.contains(x)).cloned().collect::<Vec<_>>());
                    return back.ticket;
                }
                (Work::Drop { conv: c, gens: then, discard: was }, Work::Drop { conv, gens, discard }) if c == conv && was == discard => {
                    then.extend(gens.iter().filter(|x| !then.contains(x)).cloned().collect::<Vec<_>>());
                    return back.ticket;
                }
                _ => {}
            }
        }
        self.next_ticket += 1;
        self.queue.push_back(Queued { ticket: self.next_ticket, work });
        self.next_ticket
    }

    /// The next transaction to run off the UI thread, once the one out
    /// landed: one at a time, in the order they were made. `None`: nothing
    /// to write, or one is out.
    pub(crate) fn next_write(&mut self) -> Option<Write> {
        let path = self.path.clone()?;
        if self.out {
            return None;
        }
        let q = self.queue.front()?;
        self.out = true;
        Some(Write { ticket: q.ticket, path, work: q.work.clone() })
    }

    /// Whether a transaction is queued or out.
    pub(crate) fn writing(&self) -> bool {
        !self.queue.is_empty()
    }

    /// A transaction as it ran ([`Write::run`]): memory takes the file as
    /// it left it (another process's desires too), what memory dropped
    /// since still dropped; a wish draws now, or is refused (said, nothing
    /// drawn); a message goes, with the notices marked.
    pub(crate) fn landed(&mut self, l: Landing) -> Vec<Landed> {
        if !self.out || self.queue.front().is_none_or(|q| q.ticket != l.ticket) {
            return Vec::new();
        }
        let q = self.queue.pop_front().expect("the one out");
        self.out = false;
        if let Some(file) = l.file {
            self.adopt(file);
        }
        match (q.work, l.ran) {
            (Work::Sweep, _) => {
                self.trouble = l.trouble.or(self.trouble.take());
                Vec::new()
            }
            (Work::Wish(w), Ran::Wish(done)) => {
                let key = (w.conv.clone(), w.id.clone());
                if self.asked.get(&key).is_some_and(|a| a.ticket == q.ticket) {
                    self.asked.remove(&key);
                }
                let done = done.and_then(|d| self.drawn(&w, d));
                if let Err(Refused(why)) = &done {
                    self.note_refusal(key, why.clone());
                }
                vec![Landed::Wish { conv: w.conv, id: w.id, done }]
            }
            (Work::Send { picks, .. }, Ran::Sent(rode)) => {
                let s = self.sending.remove(&q.ticket).expect("a message waits for it");
                let rode = rode.unwrap_or_else(|e| {
                    log::warn!(target: "synaps_desktop::blocks", "notices kept for a later message: {e}");
                    Vec::new()
                });
                let ride = self.build(&s, &rode);
                // What doesn't ride (the file had it otherwise, the write
                // failed, the trailer didn't fit) isn't in flight.
                let gone: Vec<&BlockId> =
                    picks.iter().map(|(id, ..)| id).filter(|id| !ride.notices.iter().any(|(i, ..)| i == *id)).collect();
                if let Some(slot) = self.book.slot_mut(&s.conv, &s.generation) {
                    for id in gone {
                        if let Some(d) = slot.get_mut(id).filter(|d| d.carrier == s.carrier) {
                            d.carrier = None;
                        }
                    }
                }
                vec![Landed::Send { ticket: q.ticket, ride }]
            }
            (_, Ran::Dropped(Err(e))) => {
                // Memory dropped them already: the file keeps them, and a
                // later settle drops them again.
                log::warn!(target: "synaps_desktop::blocks", "dropped desires not written: {e}");
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The message `s` with the notices that were marked: its trailer, or
    /// the body alone.
    fn build(&self, s: &Sending, rode: &[(BlockId, Seen, u32)]) -> Ride {
        let plain = Ride { text: s.body.clone(), carrier: None, notices: Vec::new() };
        let Some(carrier) = s.carrier.filter(|_| !rode.is_empty()) else { return plain };
        let line = |kind: Seen| {
            let notices: Vec<Notice> =
                rode.iter().filter(|(_, k, _)| *k == kind).map(|(id, _, version)| Notice { id: id.clone(), version: *version }).collect();
            (!notices.is_empty()).then(|| NoticeLine::new(kind, notices).expect("≤ 8 ids, each once, versions in range"))
        };
        let lines: Vec<NoticeLine> = [line(Seen::Dismissed), line(Seen::Shown)].into_iter().flatten().collect();
        let trailer = Trailer::new(lines).expect("one line of each kind, an id in one");
        let text = format!("{}{}", s.body, trailer.text());
        if message::split(&text) != (s.body.as_str(), trailer) {
            return plain;
        }
        Ride { text, carrier: Some(carrier), notices: rode.to_vec() }
    }

    /// What a landed wish draws: what the file decided, or why not.
    fn drawn(&self, w: &Wish, done: Done) -> Result<Done, Refused> {
        let now = self.book.slot(&w.conv, &w.generation).and_then(|s| s.get(&w.id));
        let ok = match done {
            Done::Wished(v) => now.is_some_and(|d| d.dismissed == w.dismiss && d.version == v),
            Done::Cancelled => now.is_none(),
            Done::Unchanged => now.map_or(w.shared_dismissed, |d| d.dismissed) == w.dismiss,
        };
        if ok { Ok(done) } else { Err(Refused(MOVED.into())) }
    }

    /// `file` (the file as a transaction left it) is memory's now: what
    /// memory dropped and hasn't written yet stays dropped, and a desire
    /// that didn't change keeps its carrier and hold.
    fn adopt(&mut self, mut file: Book) {
        for q in &self.queue {
            match &q.work {
                Work::Settle { conv, generation, shared, gone } => settle_in(&mut file, conv, generation, shared, gone),
                Work::Drop { conv, gens, discard } => drop_in(&mut file, conv, gens, *discard),
                _ => {}
            }
        }
        for (key, gens) in &mut file.conversations {
            for (g, x) in gens {
                for (id, d) in &mut x.desires {
                    if let Some(w) =
                        self.book.slot(key, g).and_then(|s| s.get(id)).filter(|w| w.version == d.version && w.dismissed == d.dismissed)
                    {
                        (d.carrier, d.held) = (w.carrier, w.held);
                    }
                }
            }
        }
        self.book = file;
    }

    fn slot(&self, conv: &str, generation: &Generation) -> Option<&Slot> {
        self.book.slot(conv, &generation_key(generation))
    }

    /// `id`'s desire in `conv` (of `generation`'s history), as drawn.
    pub(crate) fn desire(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<&Desire> {
        self.slot(conv, generation)?.get(id)
    }

    /// `id`'s wish not yet landed, if any: `Some(None)` a cancel.
    fn asked(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<Option<&Desire>> {
        let a = self.asked.get(&(conv.to_string(), id.clone()))?;
        (a.generation == generation_key(generation)).then_some(a.wish.as_ref())
    }

    /// Whether `id` has a wish whose transaction hasn't landed: it draws as
    /// before meanwhile (a pending look) and rides nothing.
    pub(crate) fn pending(&self, conv: &str, generation: &Generation, id: &BlockId) -> bool {
        self.asked(conv, generation, id).is_some()
    }

    /// Why `id`'s last wish wasn't kept, until its next wish.
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
    pub(crate) fn feedback(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<Feedback> {
        self.feedback.get(&(conv.to_string(), generation_key(generation), id.clone())).map(|(_, f)| *f)
    }

    /// `id`'s feedback was shown: it goes.
    pub(crate) fn feedback_seen(&mut self, conv: &str, generation: &Generation, id: &BlockId) {
        self.feedback.remove(&(conv.to_string(), generation_key(generation), id.clone()));
    }

    /// The side tables' sizes: feedback and refusals kept (tests; Settings).
    pub(crate) fn notes(&self) -> (usize, usize) {
        (self.feedback.len(), self.refusals.len())
    }

    fn note_feedback(&mut self, conv: &str, generation: &str, id: BlockId, f: Feedback) {
        self.next_note += 1;
        self.feedback.insert((conv.to_string(), generation.to_string(), id), (self.next_note, f));
        let oldest = |m: &BTreeMap<(String, String, BlockId), (u64, Feedback)>, here: bool| {
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
    pub(crate) fn waiting(&self) -> Vec<(String, usize)> {
        self.book.conversations.keys().map(|k| (k.clone(), self.book.conversation_len(k))).collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.book.len()
    }

    /// Dismiss `id` in `conv` (§3.5). `shared`: the board as drawn, the
    /// live turn included. Refused at once, or its transaction queued: it
    /// draws when that lands (module docs).
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
        let g = generation_key(generation);
        self.feedback.remove(&(conv.to_string(), g.clone(), id.clone()));
        self.refusals.remove(&(conv.to_string(), id.clone()));
        // What the user asked last: a wish not landed yet, else what draws.
        // A preflight: the file decides.
        let cur = match self.asked(conv, generation, id) {
            Some(a) => a.cloned(),
            None => self.desire(conv, generation, id).cloned(),
        };
        if cur.as_ref().map_or_else(|| shared.dismissed(id), |d| d.dismissed) == dismiss {
            return Ok(Ask::Unchanged);
        }
        if self.queue.len() >= QUEUE_MAX {
            return Err(Refused(BUSY.into()));
        }
        let cancel = cur.as_ref().is_some_and(|d| !d.sent && shared.dismissed(id) == dismiss);
        if cur.is_none() {
            // The caps, in memory: refused at once, before the disk. Wishes
            // not landed count as the desires they'll be.
            let new = |(c, i): &(String, BlockId), a: &Asked| {
                a.wish.is_some() && self.book.slot(c, &a.generation).is_none_or(|s| !s.contains_key(i))
            };
            let more_here = self.asked.iter().filter(|(k, a)| k.0 == conv && new(k, a)).count();
            if self.book.conversation_len(conv) + more_here >= PER_CONVERSATION {
                return Err(Refused(FULL_HERE.into()));
            }
            let more = self.asked.iter().filter(|(k, a)| new(k, a)).count();
            if self.len() + more >= PER_DESK {
                return Err(Refused(FULL_DESK.into()));
            }
        }
        let wish = Wish {
            conv: conv.into(),
            generation: g.clone(),
            id: id.clone(),
            dismiss,
            shared_dismissed: shared.dismissed(id),
            highest: shared.highest(),
            had: self.desire(conv, generation, id).is_some(),
        };
        let ticket = self.queue_write(Work::Wish(wish));
        let likely = (!cancel).then(|| Desire::new(dismiss, 0, cur.as_ref().is_some_and(|d| d.sent)));
        self.asked.insert((conv.to_string(), id.clone()), Asked { ticket, generation: g, wish: likely });
        Ok(Ask::Saving)
    }

    /// What the next message of `conv` carries, as the composer's chip
    /// shows it: each desire above `effective_version(id)`, not in flight,
    /// not kept back, no wish of its id pending; at most 8 of each kind,
    /// the oldest versions first.
    pub(crate) fn riding(&self, conv: &str, generation: &Generation, shared: &Visibility) -> Vec<(BlockId, Seen, u32)> {
        let Some(c) = self.slot(conv, generation) else { return Vec::new() };
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
    /// (module docs). `Go`: nothing rides, nor waits ahead of it. `Wait`:
    /// it goes when [`Landed::Send`] of the ticket lands, with the notices
    /// the file still had so, marked sent; in flight on its carrier until
    /// then. `shared`: the board as drawn.
    pub(crate) fn ride(&mut self, conv: &str, generation: &Generation, shared: &Visibility, body: &str, how: Carry) -> Riding {
        let g = generation_key(generation);
        let mut picked = Vec::new();
        if how != Carry::Answer && !body.trim().is_empty() {
            picked = self.riding(conv, generation, shared);
            // What the chip kept back was kept from this message: the next
            // carries it.
            if let Some(c) = self.book.slot_mut(conv, &g) {
                c.values_mut().for_each(|d| d.held = false);
            }
        }
        if !picked.is_empty() {
            let probe = Sending { body: body.to_string(), carrier: Some(0), conv: conv.into(), generation: g.clone() };
            if self.build(&probe, &picked).notices.is_empty() {
                // The body's own last line reads as a notice beside these:
                // they wait for the next message.
                picked.clear();
            }
        }
        if picked.is_empty() && self.sending.is_empty() {
            return Riding::Go(Ride { text: body.to_string(), carrier: None, notices: Vec::new() });
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
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        self.queue.push_back(Queued { ticket, work: Work::Send { conv: conv.into(), generation: g.clone(), picks: picked } });
        self.sending.insert(ticket, Sending { body: body.to_string(), carrier, conv: conv.into(), generation: g });
        Riding::Wait(ticket)
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
    /// settled history nor lost) given: what's done is dropped now (its
    /// transaction queued), with why it draws otherwise if it does; a
    /// carrier no longer in flight is cleared, its desires re-armed. The
    /// generation this process settled before, if another, is dropped; at
    /// its first settle of `conv`, every other generation it knows of.
    pub(crate) fn settle(
        &mut self,
        conv: &str,
        generation: &Generation,
        shared: &Visibility,
        in_flight: &[u64],
    ) -> Vec<(BlockId, Feedback)> {
        let g = generation_key(generation);
        let gone: Vec<String> = match self.seen.insert(conv.to_string(), g.clone()) {
            None => self.book.generations(conv).into_iter().filter(|k| *k != g).collect(),
            Some(before) if before != g => vec![before],
            Some(_) => Vec::new(),
        };
        while self.seen.len() > SEEN_MAX {
            let k = self.seen.keys().find(|k| *k != conv).cloned().expect("over the cap");
            self.seen.remove(&k);
        }
        self.feedback.retain(|(c, fg, _), _| c != conv || !gone.contains(fg));
        let waiting: BTreeSet<u64> = self.sending.values().filter_map(|s| s.carrier).collect();
        let mut out = Vec::new();
        let known = self.book.generations(conv);
        let mut dropped = gone.iter().any(|x| known.contains(x));
        if let Some(slot) = self.book.slot_mut(conv, &g) {
            slot.retain(|id, d| {
                if shared.effective_version(id) >= d.version {
                    let why = match shared.record(id) {
                        Some(r) if r.dismissed != d.dismissed => Some(Feedback::ChangedByLater),
                        None if d.dismissed => Some(Feedback::NotApplied),
                        _ => None,
                    };
                    out.extend(why.map(|f| (id.clone(), f)));
                    dropped = true;
                    return false;
                }
                if d.carrier.is_some_and(|k| !in_flight.contains(&k) && !waiting.contains(&k)) {
                    d.carrier = None;
                }
                true
            });
        }
        self.book.drop_generations(conv, &gone);
        for (id, f) in &out {
            self.note_feedback(conv, &g, id.clone(), *f);
        }
        if dropped && self.path.is_some() {
            self.queue_write(Work::Settle { conv: conv.into(), generation: g, shared: shared.clone(), gone });
        }
        out
    }

    /// `conv` was compacted (`state`: its board at the end): its
    /// generation's desires dropped, and the ids this desk drew dismissed
    /// returned (the successor's soft dismissals): every id the board's
    /// visibility records ([`Visibility::records`]) or a desire names,
    /// drawn dismissed. An id drawn dismissed has one or the other, so
    /// none is missed.
    pub(crate) fn compacted(&mut self, conv: &str, state: &State) -> Vec<BlockId> {
        let generation = state.generation();
        let shared = state.visibility();
        let mut ids: BTreeSet<BlockId> = shared.records().filter(|(_, r)| r.dismissed).map(|(id, _)| id.clone()).collect();
        if let Some(c) = self.slot(conv, &generation) {
            ids.extend(c.keys().cloned());
        }
        let out: Vec<BlockId> = ids.into_iter().filter(|id| self.dismissed(conv, &generation, id, shared)).collect();
        // The file's too, whichever process's: the generation is over.
        self.drop_generations(conv, vec![generation_key(&generation)], true);
        out
    }

    /// Every desire of `conv` the user sees listed dropped (the user's
    /// discard: every generation this process knows of it): how many.
    pub(crate) fn discard(&mut self, conv: &str) -> usize {
        let gens = self.book.generations(conv);
        let n = self.book.conversation_len(conv);
        self.feedback.retain(|(c, ..), _| c != conv);
        self.refusals.retain(|(c, _), _| c != conv);
        self.book.discard(conv, &gens);
        if n > 0 && self.path.is_some() {
            self.queue_write(Work::Drop { conv: conv.into(), gens, discard: true });
        }
        n
    }

    /// `gens` of `conv` dropped now, its transaction queued (only those
    /// generations, as the file has them then); `conv`'s notes go.
    fn drop_generations(&mut self, conv: &str, gens: Vec<String>, always: bool) {
        self.feedback.retain(|(c, ..), _| c != conv);
        self.refusals.retain(|(c, _), _| c != conv);
        let had = self.book.generations(conv).iter().any(|x| gens.contains(x));
        drop_in(&mut self.book, conv, &gens, false);
        if (had || always) && self.path.is_some() {
            self.queue_write(Work::Drop { conv: conv.into(), gens, discard: false });
        }
    }
}

#[cfg(test)]
#[path = "desires_tests.rs"]
pub(crate) mod tests;
