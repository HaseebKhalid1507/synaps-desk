//! What this desk keeps of how the agent's blocks are presented
//! (synaps-cards `docs/blocks.md` v1.3: §3.5's Pin and Slot, §5.2's soft
//! dismissals, §1.8's `blocks.json` row, §5.4): local, never sent, never
//! another desk's.
//!
//! - **Pin** (`p`, [`Presentation::pin`]) moves a transcript block to the
//!   rail's upper slot, and `p` again brings it back; on a rail section it
//!   brings it into the transcript, at the call that created it, and
//!   there it stays, whatever the agent moves it to since.
//! - **Slot** (`m`, [`Presentation::slot`]) moves a rail section between
//!   `upper` and `end`. On a section the user moved, `m` again takes the
//!   agent's placement; on a pinned one it moves the pin between slots.
//! - **They outrank the agent's `move` and `slot`** ([`Presentation::placed`]):
//!   the user's place draws until `p` or `m` again or "reset placement"
//!   ([`Presentation::reset`]), then the agent's. Where the agent placed
//!   it since, the stub says `the agent moved it`. A group's member moves
//!   with its group: `p` and `m` are off on it.
//! - **Order within a slot** ([`Presentation::order`]): the user's first,
//!   in the order the user placed them, then the agent's in its order.
//! - **Soft dismissals** ([`Presentation::compacted`]): the predecessor's
//!   dismissed ids, carried into a compaction's successor. A block put
//!   again under one draws as its tombstone (`dismissed before compaction
//!   · u show`), never hidden: it may be another block. `u` ends it
//!   ([`Presentation::unsoft`]); `x` sends a real notice of the new
//!   generation ([`crate::desires`]), and ends it too. Pins and slots carry
//!   over by id, as preferences.
//!
//! **The file**, `blocks.json` next to `settings.json`, through
//! [`crate::blockfiles`] (atomic, this user's only, swept; owned by one
//! process, another keeping its presentation in memory only): per
//! conversation (journal id) at most [`ENTRIES_MAX`] entries and
//! [`CONVERSATION_BYTES_MAX`] bytes, the oldest entry going first; at most
//! [`CONVERSATIONS_MAX`] conversations and [`FILE_BYTES_MAX`] bytes, the
//! least recently touched conversation going first. It's presentation: no
//! wish is lost.
//! **Drawn at once, written off the UI thread:** memory is the truth. A
//! change draws now, and a snapshot of memory, whole, is written
//! ([`Presentation::next_write`], run on the process's writer, holding the
//! file's lock until it ran, [`Write::run`], handed back,
//! [`Presentation::landed`]; a window's end hands it the last,
//! [`Presentation::last_write`]), one at a time: what memory
//! removed is gone from the file too. A write that fails is said; memory
//! keeps the change, and the next change's write carries it. (A pin is a
//! preference: one a crash takes before its write lands is placed again; a
//! desire is never drawn before it is written, [`crate::desires`].)

use std::collections::BTreeMap;
use std::path::PathBuf;

use cards_spec::block::{BlockId, Ref, Slot};
use cards_spec::board::{Block, Place, State};
use serde_json::{Value, json};

use crate::blockfiles::{self, NotOwned, Owned, Stored};

/// The file, next to `settings.json`.
pub(crate) const FILE: &str = "blocks.json";
/// Entries per conversation at most.
pub(crate) const ENTRIES_MAX: usize = 64;
/// One conversation in the file, bytes at most.
pub(crate) const CONVERSATION_BYTES_MAX: usize = 16 * 1024;
/// Conversations at most.
pub(crate) const CONVERSATIONS_MAX: usize = 256;
/// The file, bytes at most.
pub(crate) const FILE_BYTES_MAX: usize = 1 << 20;
/// A conversation's key (its journal id), bytes at most.
const KEY_MAX: usize = crate::desires::KEY_MAX;
const FORMAT: u64 = 1;

/// The file next to `settings.json` (`None`: no config folder).
pub(crate) fn path() -> Option<PathBuf> {
    crate::theme::settings_path().map(|p| p.with_file_name(FILE))
}

/// Where the user put a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserPlace {
    Rail(Slot),
    /// In the transcript, at the call that created it (`p` on a rail
    /// section).
    Transcript,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// A pin (`p`) or a slot move (`m`): where the user put it, and where
    /// the agent had it then (none: carried across a compaction).
    Placed { r: Ref, to: UserPlace, pinned: bool, agent: Option<Place> },
    /// A dismissal carried across a compaction.
    Soft(BlockId),
}

impl Entry {
    /// Whether it is `r`'s.
    fn of(&self, r: &Ref) -> bool {
        match self {
            Entry::Placed { r: x, .. } => x == r,
            Entry::Soft(id) => matches!(r, Ref::Block(b) if b == id),
        }
    }
}

/// What the user set for one block in one conversation ([`Presentation::mark`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mark(Vec<Kept>);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Kept {
    entry: Entry,
    /// The conversation's counter when the user made it: its order, and
    /// the oldest goes first.
    seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Conversation {
    /// When it last changed (ms since the epoch, rising): the least
    /// recently touched goes first.
    touched: u64,
    seq: u64,
    entries: Vec<Kept>,
}

impl Conversation {
    fn placed(&self, r: &Ref) -> Option<&Kept> {
        self.entries.iter().find(|k| matches!(&k.entry, Entry::Placed { r: x, .. } if x == r))
    }

    fn json(&self) -> Value {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|k| match &k.entry {
                Entry::Placed { r, to, pinned, agent } => json!({
                    "ref": r.to_string(),
                    "to": user_place_str(*to),
                    "pin": u8::from(*pinned),
                    "agent": agent.map(place_str),
                    "seq": k.seq,
                }),
                Entry::Soft(id) => json!({"soft": id.to_string(), "seq": k.seq}),
            })
            .collect();
        json!({"touched": self.touched, "seq": self.seq, "entries": entries})
    }

    fn bytes(&self) -> usize {
        self.json().to_string().len()
    }
}

fn user_place_str(p: UserPlace) -> &'static str {
    match p {
        UserPlace::Rail(s) => s.as_str(),
        UserPlace::Transcript => "transcript",
    }
}

fn place_str(p: Place) -> String {
    match p {
        Place::Rail(s) => s.as_str().to_string(),
        Place::Transcript(at) => format!("t{at}"),
    }
}

fn slot_of(s: &str) -> Option<Slot> {
    match s {
        "upper" => Some(Slot::Upper),
        "end" => Some(Slot::End),
        _ => None,
    }
}

fn place_of(s: &str) -> Option<Place> {
    slot_of(s).map(Place::Rail).or_else(|| s.strip_prefix('t')?.parse().ok().map(Place::Transcript))
}

fn entry_of(v: &Value) -> Option<Kept> {
    let seq = v["seq"].as_u64()?;
    if let Some(id) = v["soft"].as_str() {
        return Some(Kept { entry: Entry::Soft(BlockId::parse(id)?), seq });
    }
    let r = Ref::parse(v["ref"].as_str()?)?;
    let to = match v["to"].as_str()? {
        "transcript" => UserPlace::Transcript,
        s => UserPlace::Rail(slot_of(s)?),
    };
    let pinned = v["pin"].as_u64().filter(|b| *b <= 1)? == 1;
    let agent = match &v["agent"] {
        Value::Null => None,
        a => Some(place_of(a.as_str()?)?),
    };
    Some(Kept { entry: Entry::Placed { r, to, pinned, agent }, seq })
}

/// Every conversation's presentation: the file's content, and memory's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Book {
    conversations: BTreeMap<String, Conversation>,
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
            let (Some(touched), Some(seq), Some(list), true) =
                (c["touched"].as_u64(), c["seq"].as_u64(), c["entries"].as_array(), key_ok(key))
            else {
                bad += 1;
                continue;
            };
            let mut conv = Conversation { touched, seq, entries: Vec::new() };
            for e in list {
                match entry_of(e) {
                    Some(k) => conv.entries.push(k),
                    None => bad += 1,
                }
            }
            book.conversations.insert(key.clone(), conv);
        }
        Ok((book, bad))
    }

    fn text(&self) -> String {
        let convs: serde_json::Map<String, Value> = self.conversations.iter().map(|(k, c)| (k.clone(), c.json())).collect();
        json!({"version": FORMAT, "conversations": convs}).to_string()
    }
}

impl Book {
    /// The caps (module docs): the oldest entries of a conversation past
    /// its own, then the least recently touched conversations past the
    /// file's.
    fn fit(&mut self) {
        for c in self.conversations.values_mut() {
            while c.entries.len() > ENTRIES_MAX || (!c.entries.is_empty() && c.bytes() > CONVERSATION_BYTES_MAX) {
                let oldest = c.entries.iter().enumerate().min_by_key(|(_, k)| k.seq).map(|(i, _)| i).expect("not empty");
                c.entries.remove(oldest);
            }
        }
        self.conversations.retain(|_, c| !c.entries.is_empty());
        while self.conversations.len() > CONVERSATIONS_MAX || (!self.conversations.is_empty() && self.text().len() > FILE_BYTES_MAX) {
            let lru = self.conversations.iter().min_by_key(|(k, c)| (c.touched, (*k).clone())).map(|(k, _)| k.clone()).expect("not empty");
            self.conversations.remove(&lru);
        }
    }
}

fn key_ok(conv: &str) -> bool {
    !conv.is_empty() && conv.len() <= KEY_MAX && !conv.chars().any(char::is_control)
}

/// Where a block draws on this desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) place: Place,
    /// The user put it there: the hover note says `moved by you`.
    pub(crate) by_user: bool,
    /// The agent placed it elsewhere since: the stub adds `the agent moved
    /// it`.
    pub(crate) agent_moved: bool,
}

/// A write of `blocks.json`, to run off the UI thread ([`Write::run`]);
/// what it returns goes back to [`Presentation::landed`].
#[derive(Debug)]
pub(crate) struct Write {
    ticket: u64,
    /// Memory's change count the snapshot has.
    seq: u64,
    path: PathBuf,
    /// The file's lock, held until the write ran ([`blockfiles::Owned`]).
    owner: Owned,
    /// `None`: the sweep (the first write); else memory, whole.
    text: Option<String>,
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
        let done = match &self.text {
            None => Ok(blockfiles::sweep::<Book>(&self.path)),
            Some(text) => blockfiles::write(&self.path, text).map(|()| None),
        };
        drop(self.owner);
        Landing { ticket: self.ticket, seq: self.seq, sweep: self.text.is_none(), done }
    }
}

/// This desk's presentation, every conversation's (module docs).
pub(crate) struct Presentation {
    path: Option<PathBuf>,
    /// The file's lock, the app's ([`blockfiles::Owned`]), or why not:
    /// memory only.
    owner: Result<Owned, NotOwned>,
    book: Book,
    /// Memory's changes, counted; the count the file has; the last that
    /// failed (not tried again until memory changes).
    seq: u64,
    written: u64,
    failed_at: Option<u64>,
    /// The write out, by ticket.
    out: Option<u64>,
    next_ticket: u64,
    sweep: bool,
    /// Why the last write failed, until one lands.
    pub(crate) failed: Option<String>,
    /// Why the file read as empty at the load, what its sweep said, or that
    /// it is another process's.
    pub(crate) trouble: Option<String>,
}

impl Presentation {
    /// The presentation in the file at `path`, owned by this process (its
    /// lock taken, without waiting) and read now; its sweep is the first
    /// write. Another process's, or `None` (no config folder): memory only.
    #[cfg(test)]
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        Self::open(path.map(|p| {
            let o = blockfiles::own(&p);
            (p, o)
        }))
    }

    /// [`Presentation::load`], `file`'s lock the app's ([`blockfiles::own`]).
    pub(crate) fn open(file: Option<(PathBuf, Result<Owned, NotOwned>)>) -> Self {
        let (path, owner) = match file {
            Some((p, o)) => (Some(p), o),
            None => (None, Err(NotOwned::Failed("no settings folder".into()))),
        };
        let (book, trouble) = match (&path, &owner) {
            (Some(p), Ok(_)) => blockfiles::peek::<Book>(p),
            (Some(_), Err(NotOwned::Held)) => {
                (Book::default(), Some(format!("{FILE} is another desk's: placements here are kept in memory only")))
            }
            (Some(_), Err(NotOwned::Failed(why))) => {
                (Book::default(), Some(format!("{FILE}: {why}: placements here are kept in memory only")))
            }
            (None, _) => (Book::default(), None),
        };
        Presentation {
            path,
            sweep: owner.is_ok(),
            owner,
            book,
            seq: 0,
            written: 0,
            failed_at: None,
            out: None,
            next_ticket: 0,
            failed: None,
            trouble,
        }
    }

    /// The next write to run off the UI thread, once the one out landed:
    /// the sweep first, then memory whole whenever it changed since the
    /// file. Memory only: none.
    pub(crate) fn next_write(&mut self) -> Option<Write> {
        let (path, owner) = self.file()?;
        if self.out.is_some() {
            return None;
        }
        let text = if std::mem::take(&mut self.sweep) {
            None
        } else if self.seq > self.written && self.failed_at != Some(self.seq) {
            Some(self.book.text())
        } else {
            return None;
        };
        self.next_ticket += 1;
        self.out = Some(self.next_ticket);
        Some(Write { ticket: self.next_ticket, seq: self.seq, path, owner, text })
    }

    /// The file and its lock, owned.
    fn file(&self) -> Option<(PathBuf, Owned)> {
        Some((self.path.clone()?, self.owner.as_ref().ok()?.clone()))
    }

    /// Tests: its file.
    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
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

    /// The window's last write, or the app's: memory whole if it changed
    /// since the file has, whatever write is out (the writer runs this one
    /// after it); no one hears how it went.
    pub(crate) fn last_write(&mut self) -> Option<Write> {
        let (path, owner) = self.file()?;
        if self.seq <= self.written {
            return None;
        }
        self.next_ticket += 1;
        Some(Write { ticket: self.next_ticket, seq: self.seq, path, owner, text: Some(self.book.text()) })
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
        self.owner.is_ok() && self.out.is_none() && self.seq > self.written && self.failed_at != Some(self.seq)
    }

    /// Whether a write is out or to make.
    #[cfg_attr(not(test), expect(dead_code, reason = "tests"))]
    pub(crate) fn writing(&self) -> bool {
        self.owner.is_ok() && (self.out.is_some() || self.sweep || (self.seq > self.written && self.failed_at != Some(self.seq)))
    }

    /// Tests: nothing to sweep (a folder of its own, new).
    #[cfg(test)]
    pub(crate) fn swept(&mut self) {
        self.sweep = false;
    }

    /// The writer took no more (`why`: it didn't start, or stopped): the
    /// file isn't this desk's to write from now (memory only, and why);
    /// the write out, if any, is gone, and memory keeps it all.
    pub(crate) fn writer_lost(&mut self, why: &str) {
        if self.owner.is_ok() {
            self.owner = Err(NotOwned::Failed(why.to_string()));
        }
        self.sweep = false;
        self.out = None;
    }

    /// A write as it ran. Failed: said, and memory keeps it all.
    pub(crate) fn landed(&mut self, l: Landing) -> Result<(), String> {
        if self.out != Some(l.ticket) {
            return Ok(());
        }
        self.out = None;
        match l.done {
            Ok(said) if l.sweep => {
                self.trouble = said.or(self.trouble.take());
                Ok(())
            }
            Ok(_) => {
                self.written = l.seq;
                self.failed = None;
                Ok(())
            }
            Err(e) => {
                let why = format!("blocks.json not written: {e}");
                log::warn!(target: "synaps_desktop::blocks", "{why}");
                self.failed_at = Some(l.seq);
                self.failed = Some(why.clone());
                Err(why)
            }
        }
    }

    /// Where `b` draws in `conv` (module docs).
    pub(crate) fn placed(&self, conv: &str, b: &Block) -> Placed {
        let agent_only = Placed { place: b.place, by_user: false, agent_moved: false };
        let Some(Kept { entry: Entry::Placed { to, agent, .. }, .. }) = self.book.conversations.get(conv).and_then(|c| c.placed(&b.id))
        else {
            return agent_only;
        };
        let place = match to {
            UserPlace::Rail(s) => Place::Rail(*s),
            // Where the user put it, whatever the agent's `move` since.
            UserPlace::Transcript => Place::Transcript(b.created.event),
        };
        Placed { place, by_user: true, agent_moved: agent.is_some_and(|a| a != b.place) }
    }

    /// `slot`'s sections in `conv`, in drawing order: the user's first, in
    /// the order the user placed them, then the agent's in its order. A
    /// group's members draw in it, not here.
    #[cfg_attr(not(test), expect(dead_code, reason = "the transcript's pins, with D7"))]
    pub(crate) fn order(&self, conv: &str, state: &State, slot: Slot) -> Vec<Ref> {
        let c = self.book.conversations.get(conv);
        let mut user: Vec<(u64, Ref)> = c
            .map(|c| {
                c.entries
                    .iter()
                    .filter_map(|k| match &k.entry {
                        Entry::Placed { r, to: UserPlace::Rail(s), .. } if *s == slot => Some((k.seq, r.clone())),
                        _ => None,
                    })
                    .filter(|(_, r)| state.block(r).is_some_and(|b| b.group.is_none()))
                    .collect()
            })
            .unwrap_or_default();
        user.sort();
        let moved = |r: &Ref| c.is_some_and(|c| c.placed(r).is_some());
        let agent = state.list(Place::Rail(slot)).iter().filter(|r| !moved(r)).cloned();
        user.into_iter().map(|(_, r)| r).chain(agent).collect()
    }

    /// The user's rail placements in `conv`, pins and slot moves, in the
    /// order the user made them (D6a's `RailVisibility::placed`).
    pub(crate) fn user_rail(&self, conv: &str) -> Vec<(Ref, Slot)> {
        let Some(c) = self.book.conversations.get(conv) else { return Vec::new() };
        let mut v: Vec<(u64, Ref, Slot)> = c
            .entries
            .iter()
            .filter_map(|k| match &k.entry {
                Entry::Placed { r, to: UserPlace::Rail(s), .. } => Some((k.seq, r.clone(), *s)),
                _ => None,
            })
            .collect();
        v.sort_by_key(|(seq, ..)| *seq);
        v.into_iter().map(|(_, r, s)| (r, s)).collect()
    }

    /// What the user set for `r` in `conv`: its pin or slot move and its
    /// soft dismissal, each as kept (an undo's before and after, D7).
    pub(crate) fn mark(&self, conv: &str, r: &Ref) -> Mark {
        Mark(self.book.conversations.get(conv).map(|c| c.entries.iter().filter(|k| k.entry.of(r)).cloned().collect()).unwrap_or_default())
    }

    /// `r`'s entries in `conv` put back to `was` (D7's undo: only while
    /// they are still what the action left, [`Presentation::mark`]).
    pub(crate) fn put_back(&mut self, conv: &str, r: &Ref, was: &Mark) {
        let c = self.book.conversations.entry(conv.to_string()).or_default();
        c.entries.retain(|k| !k.entry.of(r));
        c.entries.extend(was.0.iter().cloned());
        self.touch(conv);
    }

    /// `p` on `b` (module docs): what it does now.
    pub(crate) fn pin(&mut self, conv: &str, b: &Block) -> Result<Placed, String> {
        Self::movable(conv, b)?;
        let on_rail = matches!(self.placed(conv, b).place, Place::Rail(_));
        let c = self.book.conversations.entry(conv.to_string()).or_default();
        let had = c.placed(&b.id).cloned();
        c.entries.retain(|k| !matches!(&k.entry, Entry::Placed { r, .. } if *r == b.id));
        match had {
            Some(Kept { entry: Entry::Placed { pinned: true, .. }, .. }) => {}
            _ => {
                let to = if on_rail { UserPlace::Transcript } else { UserPlace::Rail(Slot::Upper) };
                c.seq += 1;
                let seq = c.seq;
                c.entries.push(Kept { entry: Entry::Placed { r: b.id.clone(), to, pinned: true, agent: Some(b.place) }, seq });
            }
        }
        self.touch(conv);
        Ok(self.placed(conv, b))
    }

    /// `m` on `b`, a rail section (module docs): what it does now.
    pub(crate) fn slot(&mut self, conv: &str, b: &Block) -> Result<Placed, String> {
        Self::movable(conv, b)?;
        let Place::Rail(now) = self.placed(conv, b).place else { return Err("not in the rail · p brings it there".into()) };
        let other = match now {
            Slot::Upper => Slot::End,
            Slot::End => Slot::Upper,
        };
        let c = self.book.conversations.entry(conv.to_string()).or_default();
        let had = c.placed(&b.id).cloned();
        c.entries.retain(|k| !matches!(&k.entry, Entry::Placed { r, .. } if *r == b.id));
        c.seq += 1;
        let seq = c.seq;
        match had {
            // A pin moves between the slots, still a pin.
            Some(Kept { entry: Entry::Placed { pinned: true, agent, .. }, .. }) => {
                c.entries.push(Kept { entry: Entry::Placed { r: b.id.clone(), to: UserPlace::Rail(other), pinned: true, agent }, seq });
            }
            // `m` again: the agent's placement.
            Some(_) => {}
            None => {
                c.entries.push(Kept {
                    entry: Entry::Placed { r: b.id.clone(), to: UserPlace::Rail(other), pinned: false, agent: Some(b.place) },
                    seq,
                });
            }
        }
        self.touch(conv);
        Ok(self.placed(conv, b))
    }

    fn movable(conv: &str, b: &Block) -> Result<(), String> {
        if !key_ok(conv) {
            return Err("this conversation has no journal yet".into());
        }
        if b.group.is_some() {
            return Err("in a group · move the group".into());
        }
        Ok(())
    }

    /// "Reset placement": the agent's placement for `r`. False: it had it.
    #[cfg_attr(not(test), expect(dead_code, reason = "D7's reset placement"))]
    pub(crate) fn reset(&mut self, conv: &str, r: &Ref) -> bool {
        let Some(c) = self.book.conversations.get_mut(conv) else { return false };
        let before = c.entries.len();
        c.entries.retain(|k| !matches!(&k.entry, Entry::Placed { r: x, .. } if x == r));
        let changed = c.entries.len() != before;
        if changed {
            self.touch(conv);
        }
        changed
    }

    /// Whether `id` is dismissed softly in `conv` (dismissed before a
    /// compaction).
    pub(crate) fn soft(&self, conv: &str, id: &BlockId) -> bool {
        self.book.conversations.get(conv).is_some_and(|c| c.entries.iter().any(|k| k.entry == Entry::Soft(id.clone())))
    }

    /// `u` (or `x`) on a soft tombstone: it ends. False: there was none.
    pub(crate) fn unsoft(&mut self, conv: &str, id: &BlockId) -> bool {
        let Some(c) = self.book.conversations.get_mut(conv) else { return false };
        let before = c.entries.len();
        c.entries.retain(|k| k.entry != Entry::Soft(id.clone()));
        let changed = c.entries.len() != before;
        if changed {
            self.touch(conv);
        }
        changed
    }

    /// `from` was compacted into `to`: its pins and slots carried over by
    /// id (the agent's placement then unknown), and `dismissed` (the ids
    /// it drew dismissed, [`crate::desires::Desires::compacted`]) and its
    /// own soft dismissals made `to`'s. `from` stays (as the file's caps
    /// allow): another desk may still show it.
    pub(crate) fn compacted(&mut self, from: &str, to: &str, dismissed: &[BlockId]) {
        if !key_ok(to) || from == to {
            return;
        }
        let carried: Vec<Entry> = self
            .book
            .conversations
            .get(from)
            .map(|c| {
                let mut e: Vec<&Kept> = c.entries.iter().collect();
                e.sort_by_key(|k| k.seq);
                e.into_iter()
                    .map(|k| match &k.entry {
                        Entry::Placed { r, to, pinned, .. } => Entry::Placed { r: r.clone(), to: *to, pinned: *pinned, agent: None },
                        soft => soft.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let c = self.book.conversations.entry(to.to_string()).or_default();
        let soft = dismissed.iter().cloned().map(Entry::Soft);
        for e in carried.into_iter().chain(soft) {
            let same = |k: &Kept| match (&k.entry, &e) {
                (Entry::Placed { r: a, .. }, Entry::Placed { r: b, .. }) => a == b,
                (a, b) => a == b,
            };
            if c.entries.iter().any(same) {
                continue;
            }
            c.seq += 1;
            let seq = c.seq;
            c.entries.push(Kept { entry: e, seq });
        }
        self.touch(to);
    }

    /// `conv` changed: its touch time, the caps, and one more change to
    /// write.
    fn touch(&mut self, conv: &str) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let newest = self.book.conversations.values().map(|c| c.touched).max().unwrap_or(0);
        if let Some(c) = self.book.conversations.get_mut(conv) {
            c.touched = now.max(newest + 1);
        }
        self.book.fit();
        self.seq += 1;
    }
}

#[cfg(test)]
#[path = "presentation_tests.rs"]
mod tests;
