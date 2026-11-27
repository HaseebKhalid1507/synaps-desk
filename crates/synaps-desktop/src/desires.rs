//! The user's visibility desires (synaps-cards `docs/blocks.md` v1.3:
//! §3.5, §1.8's `block-desires.json` row, §5.1, §5.2; §8.2's D6 row; §9
//! decision 2): dismiss and show, each a versioned wish of this desk that
//! reaches the agent as a notice riding the user's next message.
//!
//! **One desire per id, never a queue of notices.** For each block id this
//! desk changed, per conversation (the transcript's journal id), it keeps
//! the latest wish (dismissed or shown) and its version, and whether it was
//! ever sent ([`Desire`]). In memory only: the message it rides now (its
//! **carrier**, in flight), and whether the composer's chip kept it back.
//!
//! **The protocol is B2's** (`cards_spec::board`, `cards_spec::block::message`):
//! the board's [`Visibility`] holds each id's record, the floor and the
//! highest version seen, and its one helper `effective_version(id)` decides
//! everything here as it decides the fold's admission: what to mint, what
//! to attach, what is settled, what a restart still sends. Nothing here
//! reads a notice from a history: the fold does.
//!
//! - **The version** ([`Desires::dismiss`], [`Desires::show`]): one more
//!   than the highest seen, in the board's notices (`highest`) and in this
//!   desk's desires of the conversation.
//! - **Accepted means written.** The caps are checked in memory, then
//!   against the file under its lock ([`crate::blockfiles`]), and the wish
//!   is written before the call returns: the caller draws it only then. A
//!   cap or a failed write refuses it, its reason said, and nothing draws
//!   ([`Refused`]). So a restart never loses a wish the user saw accepted.
//!   The write is one file of at most [`RESERVE`] bytes and one sync, at a
//!   click.
//! - **A dismissal not yet sent is cancelled by Show** (and a Show not yet
//!   sent by a dismiss): the desire goes and nothing is sent. Once sent,
//!   even if its carrier is lost (a copy may still land), the other wish is
//!   a new version that supersedes it.
//! - **Riding** ([`Desires::ride`]): at every send of typed text (a
//!   `Submit` when idle, a `Steer` while a turn runs) or a hand-back, each
//!   desire above `effective_version(id)` that isn't in flight or kept back
//!   is attached, as the message's last lines: at most 8 ids of each kind,
//!   dismissals first, the oldest versions first; the rest ride the next
//!   message. **Never on its own** (an empty body carries nothing) and
//!   **never on a card answer** ([`Carry::Answer`]). A body whose own last
//!   line would read as a notice beside the trailer carries nothing: the
//!   desires wait for the next message, so no message is ever built whose
//!   trailer isn't exactly this desk's. Every send builds its trailer from
//!   the desires at that moment, a Resend included.
//! - **Settling** ([`Desires::settle`], on a settled history's board): a
//!   desire at or below `effective_version(id)` is done and dropped; if the
//!   shared state differs, the block says why ([`Feedback`]). A carrier the
//!   caller no longer has in flight (a dropped steer, a refused send, a
//!   rollback: [`Desires::lost`] too) is cleared, and the desire rides the
//!   next message at the same version.
//! - **A restart** ([`Desires::load`]) keeps the desires and loses the
//!   carriers; the first settle against the attached history drops what it
//!   shows, and the rest ride again. A copy still queued in the daemon may
//!   land as well: the second of two equal versions is stale.
//! - **A compaction** ([`Desires::compacted`]) drops the predecessor's
//!   desires, and hands back the ids it drew dismissed: the successor's
//!   soft dismissals ([`crate::presentation`]).
//!
//! **The file**, `block-desires.json` next to `settings.json`: per
//! conversation, its generation (the first 16 hex digits) and its desires,
//! ≤ [`ENTRY_MAX`] bytes each. **No cap evicts a desire:** past
//! [`PER_CONVERSATION`] in a conversation, [`PER_DESK`] in all, or the
//! reserved [`RESERVE`] bytes, a new wish is refused before it draws. A
//! desire leaves only when it settles, when the user cancels or discards
//! it, at a compaction, or when its journal's history is another
//! generation. Two desk processes share it: each write merges its change
//! into the file as it is (a later version wins per id), and memory takes
//! the result.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cards_spec::block::message::{self, NOTICE_IDS_MAX, Notice, NoticeLine, Trailer, VERSION_MAX, Visibility as Seen};
use cards_spec::block::{BlockId, Generation, Ref};
use cards_spec::board::{State, Visibility};
use serde_json::{Value, json};

use crate::blockfiles::{self, Stored};

/// The file, next to `settings.json`.
pub(crate) const FILE: &str = "block-desires.json";
/// Desires per conversation at most (§1.8).
pub(crate) const PER_CONVERSATION: usize = 64;
/// Desires per desk at most (§1.8).
pub(crate) const PER_DESK: usize = 512;
/// The file's reserved budget, bytes (§1.8): no other owner uses it.
pub(crate) const RESERVE: usize = 64 * 1024;
/// One desire in the file, bytes at most (§1.8): a 24-character id, its
/// state, a 9-digit version and its sent flag.
pub(crate) const ENTRY_MAX: usize = 64;
/// A conversation's key (its journal id), bytes at most.
pub(crate) const KEY_MAX: usize = 128;
/// The file's format.
const FORMAT: u64 = 1;

/// The refusals' words (§1.8).
pub(crate) const FULL_HERE: &str = "64 changes waiting · send a message first";
pub(crate) const FULL_DESK: &str = "512 changes waiting · send in another conversation, or discard";

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

/// One id's desire: this desk's latest wish for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Desire {
    pub(crate) dismissed: bool,
    pub(crate) version: u32,
    /// Sent at least once (kept in the file): a copy may still land, so the
    /// other wish over it is a new version, never a cancel.
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Conversation {
    /// The generation its versions count in ([`generation_key`]).
    generation: String,
    desires: BTreeMap<BlockId, Desire>,
}

/// Every conversation's desires: the file's content, and memory's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Book {
    conversations: BTreeMap<String, Conversation>,
}

impl Book {
    fn len(&self) -> usize {
        self.conversations.values().map(|c| c.desires.len()).sum()
    }
}

impl Stored for Book {
    fn parse(text: &str) -> Result<(Self, usize), String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if v["version"].as_u64() != Some(FORMAT) {
            return Err(format!("not format {FORMAT}"));
        }
        let convs = v["conversations"].as_object().ok_or("no conversations")?;
        let (mut book, mut bad) = (Book::default(), 0);
        for (key, c) in convs {
            let generation = c["generation"].as_str().filter(|g| g.len() == 16 && g.bytes().all(|b| b.is_ascii_hexdigit()));
            let (Some(generation), Some(ds), true) = (generation, c["desires"].as_object(), key_ok(key)) else {
                bad += 1;
                continue;
            };
            let mut conv = Conversation { generation: generation.to_string(), desires: BTreeMap::new() };
            for (id, d) in ds {
                let bit = |k: &str| d[k].as_u64().filter(|b| *b <= 1).map(|b| b == 1);
                let version = d["v"].as_u64().filter(|v| (1..=u64::from(VERSION_MAX)).contains(v));
                match (BlockId::parse(id), bit("d"), version, bit("s")) {
                    (Some(id), Some(dismissed), Some(version), Some(sent)) => {
                        conv.desires.insert(id, Desire::new(dismissed, version as u32, sent));
                    }
                    _ => bad += 1,
                }
            }
            if !conv.desires.is_empty() {
                book.conversations.insert(key.clone(), conv);
            }
        }
        Ok((book, bad))
    }

    fn text(&self) -> String {
        let convs: serde_json::Map<String, Value> = self
            .conversations
            .iter()
            .map(|(key, c)| {
                let ds: serde_json::Map<String, Value> = c
                    .desires
                    .iter()
                    .map(|(id, d)| (id.to_string(), json!({"d": u8::from(d.dismissed), "v": d.version, "s": u8::from(d.sent)})))
                    .collect();
                (key.clone(), json!({"generation": c.generation, "desires": ds}))
            })
            .collect();
        json!({"version": FORMAT, "conversations": convs}).to_string()
    }
}

/// One change to the file, merged into it as it is ([`apply`]).
#[derive(Clone, Debug)]
enum Change {
    /// The user's latest wish for `id`, at `version` or, over another
    /// process's wish of the other kind at it or later, one above that: the
    /// newest click wins. One of the same kind, at it or later, stands.
    Wish { conv: String, generation: String, id: BlockId, dismissed: bool, version: u32 },
    /// The desire at `version` was sent: unless the file has another.
    Sent { conv: String, id: BlockId, version: u32 },
    /// A desire settled or cancelled: unless the file has a later version.
    Drop { conv: String, id: BlockId, version: u32 },
    /// A conversation's desires, all (a compaction, a discard, another
    /// generation).
    DropConversation { conv: String },
}

/// `changes` merged into `book`, the caps checked if `adding` (a new id):
/// refused, `book` is left to be dropped unwritten.
fn apply(book: &mut Book, changes: &[Change], adding: Option<&str>) -> Result<(), String> {
    for c in changes {
        match c {
            Change::Wish { conv, generation, id, dismissed, version } => {
                let here = book.conversations.entry(conv.clone()).or_default();
                if here.generation != *generation {
                    // Another generation's versions count nowhere now.
                    here.desires.clear();
                    here.generation.clone_from(generation);
                }
                match here.desires.get_mut(id) {
                    Some(d) if d.version >= *version && d.dismissed == *dismissed => {}
                    Some(d) if d.version >= *version => {
                        if d.version >= VERSION_MAX {
                            return Err("too many changes in this conversation".into());
                        }
                        *d = Desire::new(*dismissed, d.version + 1, false);
                    }
                    _ => {
                        here.desires.insert(id.clone(), Desire::new(*dismissed, *version, false));
                    }
                }
            }
            Change::Sent { conv, id, version } => {
                if let Some(d) = book.conversations.get_mut(conv).and_then(|c| c.desires.get_mut(id)).filter(|d| d.version == *version) {
                    d.sent = true;
                }
            }
            Change::Drop { conv, id, version } => {
                if let Some(here) = book.conversations.get_mut(conv) {
                    if here.desires.get(id).is_some_and(|d| d.version <= *version) {
                        here.desires.remove(id);
                    }
                    if here.desires.is_empty() {
                        book.conversations.remove(conv);
                    }
                }
            }
            Change::DropConversation { conv } => {
                book.conversations.remove(conv);
            }
        }
    }
    if let Some(conv) = adding {
        if book.conversations.get(conv).is_some_and(|c| c.desires.len() > PER_CONVERSATION) {
            return Err(FULL_HERE.into());
        }
        if book.len() > PER_DESK {
            return Err(FULL_DESK.into());
        }
        let n = book.text().len();
        if n > RESERVE {
            return Err(format!("{FULL_DESK} ({n} bytes, over {RESERVE})"));
        }
    }
    Ok(())
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

/// What a dismiss or a show did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Done {
    /// This desk already draws it so.
    Unchanged,
    /// A wish not yet sent, cancelled: nothing will be sent.
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

/// This desk's desires, every conversation's (module docs).
pub(crate) struct Desires {
    path: Option<PathBuf>,
    book: Book,
    next_carrier: u64,
    feedback: BTreeMap<(String, BlockId), Feedback>,
    /// Why the file read as empty at the load, or what its sweep couldn't
    /// remove (Settings says it).
    pub(crate) trouble: Option<String>,
}

impl Desires {
    /// The desires in the file at `path`, what crashes left beside it swept
    /// (`None`: no config folder, and every wish is refused: it couldn't be
    /// kept).
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let (book, trouble) = match &path {
            Some(p) => {
                let (book, why, swept) = blockfiles::load::<Book>(p);
                (book, why.or(swept.err().map(|e| e.to_string())))
            }
            None => (Book::default(), None),
        };
        Desires { path, book, next_carrier: 0, feedback: BTreeMap::new(), trouble }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn conversation(&self, conv: &str, generation: &Generation) -> Option<&Conversation> {
        self.book.conversations.get(conv).filter(|c| c.generation == generation_key(generation))
    }

    fn conversation_mut(&mut self, conv: &str, generation: &Generation) -> Option<&mut Conversation> {
        let g = generation_key(generation);
        self.book.conversations.get_mut(conv).filter(|c| c.generation == g)
    }

    /// `id`'s desire in `conv` (of `generation`'s history).
    pub(crate) fn desire(&self, conv: &str, generation: &Generation, id: &BlockId) -> Option<&Desire> {
        self.conversation(conv, generation)?.desires.get(id)
    }

    /// Whether this desk draws `id` dismissed: its desire, else the shared
    /// record (`shared`: the board as drawn).
    pub(crate) fn dismissed(&self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> bool {
        self.desire(conv, generation, id).map_or_else(|| shared.dismissed(id), |d| d.dismissed)
    }

    /// Why `id` draws other than this desk wished, until its next wish.
    pub(crate) fn feedback(&self, conv: &str, id: &BlockId) -> Option<Feedback> {
        self.feedback.get(&(conv.to_string(), id.clone())).copied()
    }

    /// Every desire of every conversation, by conversation: the `discard`
    /// list.
    pub(crate) fn waiting(&self) -> Vec<(String, usize)> {
        self.book.conversations.iter().map(|(k, c)| (k.clone(), c.desires.len())).collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.book.len()
    }

    /// Dismiss `id` in `conv` (§3.5). `shared`: the board as drawn, the
    /// live turn included. Accepted, it is written (module docs).
    pub(crate) fn dismiss(&mut self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> Result<Done, Refused> {
        self.want(conv, generation, id, true, shared)
    }

    /// Show `id` again in `conv` (§3.5): as [`Desires::dismiss`].
    pub(crate) fn show(&mut self, conv: &str, generation: &Generation, id: &BlockId, shared: &Visibility) -> Result<Done, Refused> {
        self.want(conv, generation, id, false, shared)
    }

    fn want(&mut self, conv: &str, generation: &Generation, id: &BlockId, dismiss: bool, shared: &Visibility) -> Result<Done, Refused> {
        let Some(path) = self.path.clone() else { return Err(Refused("no config folder: the change couldn't be kept".into())) };
        if !key_ok(conv) {
            return Err(Refused("this conversation has no journal yet".into()));
        }
        self.feedback.remove(&(conv.to_string(), id.clone()));
        let cur = self.desire(conv, generation, id).cloned();
        if cur.as_ref().map_or_else(|| shared.dismissed(id), |d| d.dismissed) == dismiss {
            return Ok(Done::Unchanged);
        }
        if let Some(d) = &cur
            && !d.sent
            && shared.dismissed(id) == dismiss
        {
            // Never sent, and what every desk draws is the wish: nothing
            // to tell.
            self.write(&path, vec![Change::Drop { conv: conv.into(), id: id.clone(), version: d.version }], None)?;
            return Ok(Done::Cancelled);
        }
        let mine = self.conversation(conv, generation).and_then(|c| c.desires.values().map(|d| d.version).max()).unwrap_or(0);
        let version = shared.highest().max(mine).saturating_add(1);
        if version > VERSION_MAX {
            return Err(Refused("too many changes in this conversation".into()));
        }
        if cur.is_none() {
            // The caps, in memory first: refused before the disk.
            if self.conversation(conv, generation).is_some_and(|c| c.desires.len() >= PER_CONVERSATION) {
                return Err(Refused(FULL_HERE.into()));
            }
            let stale =
                self.book.conversations.get(conv).filter(|c| c.generation != generation_key(generation)).map_or(0, |c| c.desires.len());
            if self.len() - stale >= PER_DESK {
                return Err(Refused(FULL_DESK.into()));
            }
        }
        let wish = Change::Wish { conv: conv.into(), generation: generation_key(generation), id: id.clone(), dismissed: dismiss, version };
        self.write(&path, vec![wish], cur.is_none().then_some(conv))?;
        // Over another process's later wish, re-minted above it.
        Ok(Done::Wished(self.desire(conv, generation, id).map_or(version, |d| d.version)))
    }

    /// `changes` merged into the file and written; memory takes the result
    /// (another process's desires too), keeping carriers and holds of what
    /// didn't change.
    fn write(&mut self, path: &Path, changes: Vec<Change>, adding: Option<&str>) -> Result<(), Refused> {
        let merged = blockfiles::update::<Book>(path, |b| apply(b, &changes, adding)).map_err(|f| Refused(f.to_string()))?;
        self.adopt(merged);
        Ok(())
    }

    fn adopt(&mut self, mut merged: Book) {
        for (key, c) in &mut merged.conversations {
            let Some(was) = self.book.conversations.get(key).filter(|w| w.generation == c.generation) else { continue };
            for (id, d) in &mut c.desires {
                if let Some(w) = was.desires.get(id).filter(|w| w.version == d.version && w.dismissed == d.dismissed) {
                    d.carrier = w.carrier;
                    d.held = w.held;
                }
            }
        }
        self.book = merged;
    }

    /// What the next message of `conv` carries, as the composer's chip
    /// shows it: each desire above `effective_version(id)`, not in flight,
    /// not kept back; at most 8 of each kind, the oldest versions first.
    pub(crate) fn riding(&self, conv: &str, generation: &Generation, shared: &Visibility) -> Vec<(BlockId, Seen, u32)> {
        let Some(c) = self.conversation(conv, generation) else { return Vec::new() };
        let mut all: Vec<(BlockId, Seen, u32)> = c
            .desires
            .iter()
            .filter(|(id, d)| !d.held && d.carrier.is_none() && d.version > shared.effective_version(id))
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
        match self.conversation_mut(conv, generation).and_then(|c| c.desires.get_mut(id)) {
            Some(d) if riding => {
                d.held = true;
                true
            }
            _ => false,
        }
    }

    /// `body`, as it goes `how` on `conv`, with the notices that ride it
    /// (module docs): marked sent (written first: refused, nothing rides)
    /// and in flight on the carrier returned. `shared`: the board as drawn.
    pub(crate) fn ride(&mut self, conv: &str, generation: &Generation, shared: &Visibility, body: &str, how: Carry) -> Ride {
        let plain = Ride { text: body.to_string(), carrier: None, notices: Vec::new() };
        if how == Carry::Answer || body.trim().is_empty() {
            return plain;
        }
        let picked = self.riding(conv, generation, shared);
        // What the chip kept back was kept from this message: the next
        // carries it.
        if let Some(c) = self.conversation_mut(conv, generation) {
            c.desires.values_mut().for_each(|d| d.held = false);
        }
        if picked.is_empty() {
            return plain;
        }
        let line = |kind: Seen| {
            let notices: Vec<Notice> =
                picked.iter().filter(|(_, s, _)| *s == kind).map(|(id, _, version)| Notice { id: id.clone(), version: *version }).collect();
            (!notices.is_empty()).then(|| NoticeLine::new(kind, notices).expect("≤ 8 ids, each once, versions in range"))
        };
        let lines: Vec<NoticeLine> = [line(Seen::Dismissed), line(Seen::Shown)].into_iter().flatten().collect();
        let trailer = Trailer::new(lines).expect("one line of each kind, an id in one");
        let text = format!("{body}{}", trailer.text());
        if message::split(&text) != (body, trailer) {
            // The body's own last line reads as a notice beside these: they
            // wait for the next message.
            return plain;
        }
        let Some(path) = self.path.clone() else { return plain };
        let unsent: Vec<Change> = picked
            .iter()
            .filter(|(id, ..)| self.desire(conv, generation, id).is_some_and(|d| !d.sent))
            .map(|(id, _, version)| Change::Sent { conv: conv.into(), id: id.clone(), version: *version })
            .collect();
        if !unsent.is_empty()
            && let Err(Refused(why)) = self.write(&path, unsent, None)
        {
            log::warn!(target: "synaps_desktop::blocks", "notices kept for a later message: {why}");
            return plain;
        }
        // Another process's later wish may have come in with the write: then
        // this trailer is no longer this desk's latest, and nothing rides.
        let still = |me: &Self| {
            picked.iter().all(|(id, s, version)| me.desire(conv, generation, id).is_some_and(|d| d.version == *version && d.seen() == *s))
        };
        if !still(self) {
            return plain;
        }
        self.next_carrier += 1;
        let carrier = self.next_carrier;
        if let Some(c) = self.conversation_mut(conv, generation) {
            for (id, ..) in &picked {
                if let Some(d) = c.desires.get_mut(id) {
                    d.carrier = Some(carrier);
                }
            }
        }
        Ride { text, carrier: Some(carrier), notices: picked }
    }

    /// `carrier` won't land (a refused send): what it carried rides the
    /// next message at the same version.
    pub(crate) fn lost(&mut self, carrier: u64) {
        for c in self.book.conversations.values_mut() {
            for d in c.desires.values_mut().filter(|d| d.carrier == Some(carrier)) {
                d.carrier = None;
            }
        }
    }

    /// `conv`'s desires against a settled history's board (`shared`), the
    /// carriers still in flight (`in_flight`: sent, and not yet shown by a
    /// settled history nor lost) given: what's done is dropped (and written),
    /// with why it draws otherwise if it does; a carrier no longer in flight
    /// is cleared, its desires re-armed. Another generation's desires under
    /// this journal are dropped.
    pub(crate) fn settle(
        &mut self,
        conv: &str,
        generation: &Generation,
        shared: &Visibility,
        in_flight: &[u64],
    ) -> Vec<(BlockId, Feedback)> {
        let g = generation_key(generation);
        let Some(c) = self.book.conversations.get_mut(conv) else { return Vec::new() };
        let mut changes = Vec::new();
        let mut out = Vec::new();
        if c.generation != g {
            changes.push(Change::DropConversation { conv: conv.into() });
            self.book.conversations.remove(conv);
        } else {
            let mut done = Vec::new();
            for (id, d) in c.desires.iter_mut() {
                if shared.effective_version(id) >= d.version {
                    done.push(id.clone());
                    changes.push(Change::Drop { conv: conv.into(), id: id.clone(), version: d.version });
                    let why = match shared.record(id) {
                        Some(r) if r.dismissed != d.dismissed => Some(Feedback::ChangedByLater),
                        None if d.dismissed => Some(Feedback::NotApplied),
                        _ => None,
                    };
                    out.extend(why.map(|f| (id.clone(), f)));
                } else if d.carrier.is_some_and(|k| !in_flight.contains(&k)) {
                    d.carrier = None;
                }
            }
            for id in &done {
                c.desires.remove(id);
            }
            if c.desires.is_empty() {
                self.book.conversations.remove(conv);
            }
        }
        for (id, f) in &out {
            self.feedback.insert((conv.to_string(), id.clone()), *f);
        }
        if !changes.is_empty()
            && let Some(path) = self.path.clone()
        {
            // Memory dropped them already: a write that fails leaves them in
            // the file, and the next load's settle drops them again.
            match blockfiles::update::<Book>(&path, |b| apply(b, &changes, None)) {
                Ok(merged) => self.adopt(merged),
                Err(e) => log::warn!(target: "synaps_desktop::blocks", "settled desires not written: {e}"),
            }
        }
        out
    }

    /// `conv` was compacted (`state`: its board at the end): its desires
    /// dropped, and the ids this desk drew dismissed returned (the
    /// successor's soft dismissals): every id the board still names (live,
    /// a tombstone, the op log) or a desire names, drawn dismissed.
    pub(crate) fn compacted(&mut self, conv: &str, state: &State) -> Vec<BlockId> {
        let generation = state.generation();
        let shared = state.visibility();
        let mut ids: BTreeSet<BlockId> = BTreeSet::new();
        let block = |r: &Ref| match r {
            Ref::Block(id) => Some(id.clone()),
            Ref::Card(_) => None,
        };
        ids.extend(state.blocks().filter_map(|b| block(&b.id)));
        ids.extend(state.tombstones().filter_map(|t| block(&t.id)));
        ids.extend(state.log().filter_map(|l| l.id.as_ref().and_then(block)));
        if let Some(c) = self.conversation(conv, &generation) {
            ids.extend(c.desires.keys().cloned());
        }
        let out: Vec<BlockId> = ids.into_iter().filter(|id| self.dismissed(conv, &generation, id, shared)).collect();
        self.discard(conv);
        out
    }

    /// Every desire of `conv` dropped (a compaction, the user's discard):
    /// how many. Written; a failed write leaves them in the file (said).
    pub(crate) fn discard(&mut self, conv: &str) -> usize {
        let Some(c) = self.book.conversations.remove(conv) else { return 0 };
        self.feedback.retain(|(k, _), _| k != conv);
        if let Some(path) = self.path.clone() {
            let changes = [Change::DropConversation { conv: conv.into() }];
            match blockfiles::update::<Book>(&path, |b| apply(b, &changes, None)) {
                Ok(merged) => self.adopt(merged),
                Err(e) => log::warn!(target: "synaps_desktop::blocks", "discarded desires not written: {e}"),
            }
        }
        c.desires.len()
    }
}

#[cfg(test)]
#[path = "desires_tests.rs"]
pub(crate) mod tests;
