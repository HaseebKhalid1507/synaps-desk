//! D6b wired (synaps-cards `docs/blocks.md` v1.3 §3.5, §5.2; §8.2's D6
//! row): this desk's dismissals, soft dismissals, pins and slot moves
//! ([`crate::desires`], [`crate::presentation`]) behind D6a's seam
//! ([`RailVisibility`]), and the notices riding the user's messages.
//!
//! - **What the rail draws** ([`Stores::vis`]): a block dismissed (this
//!   desk's desire, else the shared record) is hidden, and the zone counts
//!   it (`N dismissed · show` in its footer); a soft dismissal (carried
//!   across a compaction) is its tombstone, `dismissed before compaction ·
//!   u show`, its token the id's visibility version as drawn; the user's
//!   placements draw first in their slots; an agent's section the user
//!   put into the transcript (`p`) isn't drawn (nor counted) here.
//! - **What the transcript draws** ([`Stores::look`], §3.5): a block
//!   dismissed shrinks to its 24 px tombstone, `◇ tests · dismissed · u
//!   show` (a soft one's, `dismissed before compaction · u show`), its
//!   token the id's desire and effective version as drawn; one pinned into
//!   the rail leaves its stub, `◇ plan · in the rail · p` (`· the agent
//!   moved it`); an agent's rail section the user put there draws as a
//!   block at its creating call ([`AppView::pinned_in`]). A tombstone's
//!   click is its `u`, a stub's its `p`.
//! - **The rail's actions** (a section header's hover actions, the footer,
//!   a tombstone): `×` dismisses (on a soft one, a real notice of the new
//!   generation, and the tombstone ends), `⇅` moves it to the other slot
//!   (again: the agent's), `show` shows every section this rail hides, and
//!   a tombstone's click ends it (`u`). Each is checked against the bound
//!   board first, as D6a's targets are.
//! - **D7's keys** (`x u p m`, the hover's `×` `rail` `⇅`, and their undo)
//!   are D7's seam, answered here ([`Stores`]'s `BlockDesires`) where this
//!   desk draws what they do: `x` and `u` on any block (the rail hides a
//!   dismissed section, the transcript draws its tombstone; `u` on a soft
//!   tombstone ends it, on a dismissed one shows it), `p` on a transcript
//!   block (into the rail's upper slot, its stub left) and on an agent's
//!   rail section (into the transcript), and `p` again back, `m` on a rail
//!   section; the palette's block commands are the same keys on the block
//!   walked when it opened. Ctrl+Z puts back what one
//!   did, all of it or nothing, only while it holds: a dismissal (or a
//!   show) no notice of which may have gone, a pin, a slot move, a soft
//!   tombstone ended; and only where its key is offered now (a block the
//!   agent moved since is refused, [`MOVED_SINCE`]), and not where the
//!   caps would drop what it puts back at once (refused before either
//!   store changes, `presentation::NO_ROOM`). An undo whose desire
//!   is a write away puts its presentation back at once (the desire draws
//!   as before until it is written); that write failing puts both back as
//!   the key left them ([`Stores::undo_landed`]). What that takes is kept
//!   with the write, by its ticket ([`Desires::queued`]), never dropped for
//!   room: at most one per write queued, until it lands (and only its own
//!   landing acts on it) or is retired (a settle of another generation, a
//!   compaction: it goes, nothing put back). A key's token names its undo
//!   ([`UNDO_KEPT`] kept).
//! - **Riding** ([`AppView::ride_notices`]): the composer's `Submit` (idle)
//!   or `Steer` (a turn runs) carries what [`Desires::ride`] attaches; one
//!   whose marks aren't written yet goes once they are ([`Landed::Send`]),
//!   its text out of the composer already. The composer's chip says what
//!   rides (`+ tells the agent: plan dismissed ×`); its click keeps it back.
//! - **Settling** ([`AppView::wishes_on_board`], each board drawn): against
//!   the history's records, the carriers still in flight given (sent, and
//!   the turn not over with its history grown since). A journal that moved
//!   by a compaction carries its dismissed ids into the successor as soft
//!   dismissals.
//! - **Writes** run on the process's writer, off the UI thread, one at a
//!   time per file ([`AppView::pump_wishes`]), whatever changed memory: a
//!   click, a message, a settle or a compaction's carry (each board drawn,
//!   [`AppView::wishes_on_board`]); never anywhere else. A writer that
//!   didn't start, or stopped, takes nothing: the desk keeps its changes
//!   in memory only from then on, and says so ([`Stores::writer_lost`]).
//!   The files' locks are the app's, taken once at its launch ([`Home`]);
//!   a window closing or the app quitting hands the writer each file's
//!   last snapshot ([`AppView::flush_wishes`]), and the app's quit, a
//!   window open or not, waits for them ([`Home::on_quit`]).
//! - **The rail's footer** says what a refusal or a settle said last, and,
//!   for as long as this desk doesn't own its files, that it keeps its
//!   changes in memory only ([`Stores::memory_only`]).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use cards_spec::block::message::Visibility as Notice;
use cards_spec::block::{BlockId, Generation, Slot};
use cards_spec::board::{Block, Place, Visibility};
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Ref, Shown};

use crate::agent_rail::{RailVisibility, Seen, Tomb};
use crate::app::AppView;
use crate::blockfiles::{NotOwned, Owned, Writer};
use crate::desires::{Ask, Carry, Desire, Desires, Landed, Refused, Ride, Riding, same};
use crate::live::LiveId;
use crate::presentation::{Mark, Presentation};
use crate::walk::{Aim, Asked, BlockDesires, Wish};

/// A soft dismissal's tombstone (§3.5).
pub(crate) const SOFT: &str = "dismissed before compaction · u show";
/// A dismissed transcript block's tombstone (§3.5), after its id.
pub(crate) const DISMISSED: &str = "dismissed · u show";
/// A transcript block pinned into the rail: its stub (§3.5), after its id.
pub(crate) const IN_THE_RAIL: &str = "in the rail · p";
/// The stub's word once the agent placed it elsewhere since the pin.
pub(crate) const AGENT_MOVED: &str = "the agent moved it";

/// How a transcript row's block draws on this desk (§3.5, [`Stores::look`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Look {
    /// As the board has it.
    Full,
    /// A fixed one-line tombstone: `◇ tests · dismissed · u show`, or a
    /// soft one's words; its token the id's visibility as drawn (a key or
    /// a click acts only on that one).
    Tomb { text: Arc<str>, token: u64 },
    /// A transcript block pinned into the rail: its one-line stub,
    /// `◇ plan · in the rail · p` (`· the agent moved it`).
    Stub { text: Arc<str> },
}
/// Carriers remembered as sent, at most: the oldest is forgotten first
/// (its notices then ride again at the same version; a second copy is
/// stale).
const SENT_MAX: usize = 256;

/// The footer's word while this desk doesn't own its files.
pub(crate) const MEMORY_ONLY: &str = "this desk keeps rail changes in memory only";

/// The process's hold on the rail's two files ([`crate::blockfiles`]):
/// each one's lock, taken once at the launch (`main`, before any window)
/// and held until the app shuts down ([`Home::shutdown`]), and the one
/// writer. The window's stores hold a clone; every write they issue holds
/// its file's lock until it ran. Its writer didn't start: no lock taken,
/// memory only.
#[derive(Clone)]
pub(crate) struct Home {
    desires: Option<(PathBuf, Result<Owned, NotOwned>)>,
    pres: Option<(PathBuf, Result<Owned, NotOwned>)>,
    pub(crate) writer: Arc<Writer>,
}

impl Home {
    /// The writer, and both files' locks, taken now without waiting.
    pub(crate) fn take(desires: Option<PathBuf>, pres: Option<PathBuf>) -> Home {
        // Tests: a writer that queues, run by the window's executor.
        #[cfg(test)]
        let writer = Writer::queued();
        #[cfg(not(test))]
        let writer = Writer::start();
        Self::with(desires, pres, writer)
    }

    /// [`Home::take`] with `writer` (tests: one that didn't start, a real
    /// one). One that didn't start takes no lock: memory only, and why.
    pub(crate) fn with(desires: Option<PathBuf>, pres: Option<PathBuf>, writer: Arc<Writer>) -> Home {
        let own = |p: Option<PathBuf>| {
            p.map(|p| {
                let o = match writer.never_started() {
                    Some(why) => Err(NotOwned::Failed(why.to_string())),
                    None => crate::blockfiles::own(&p),
                };
                (p, o)
            })
        };
        Home { desires: own(desires), pres: own(pres), writer: writer.clone() }
    }

    /// No file (before the launch's): nothing to write.
    #[cfg(not(test))]
    fn none() -> Home {
        Home { desires: None, pres: None, writer: Writer::never("no settings folder".into()) }
    }

    /// The app's quit, a window open or not (macOS quits from within
    /// `run`, its last window possibly closed long before; Linux and
    /// Windows too): what the writer holds then waited for, its bound
    /// ([`crate::blockfiles::JOIN`]) at most. The wait is the quit's
    /// future: GPUI polls it once it released the windows, so the last
    /// snapshots their release hands over are in the queue by then.
    pub(crate) fn on_quit(&self, cx: &mut App) {
        let writer = self.writer.clone();
        cx.on_app_quit(move |_| {
            let writer = writer.clone();
            async move {
                writer.shutdown();
            }
        })
        .detach();
    }

    /// The app's end: its queue closed, the last writes it holds run,
    /// waited for [`crate::blockfiles::JOIN`] at most; then this hold on
    /// the locks goes (a write still running keeps its own). After
    /// [`Home::on_quit`]'s: nothing more.
    pub(crate) fn shutdown(self) -> bool {
        self.writer.shutdown()
    }
}

/// This desk's two stores, and what wiring them keeps.
pub(crate) struct Stores {
    pub(crate) desires: Desires,
    pub(crate) pres: Presentation,
    /// The files' locks and the writer, the app's.
    pub(crate) home: Home,
    /// Messages waiting for their notices' marks: where each goes.
    waiting: HashMap<u64, Waiting>,
    /// Carriers sent, until a settled history no longer has them in flight.
    sent: Vec<Sent>,
    /// Per session: the conversation its last board was of.
    track: HashMap<String, Track>,
    /// What the rail's footer says last: a refusal, or a settle's why.
    pub(crate) said: Option<String>,
    /// D7's undo of each key's change, by its token, oldest first.
    undos: VecDeque<(u64, Undo)>,
    next_undo: u64,
    /// Undos whose desire's write is out, their presentation put back, by
    /// that write's ticket: at most one per write queued
    /// (≤ `desires::QUEUE_MAX`), each until it lands or is retired.
    undoing: BTreeMap<u64, Undoing>,
    /// Tests: every message sent to a session, as it went.
    #[cfg(test)]
    pub(crate) outbox: Vec<String>,
}

/// An undo refused: what the key did doesn't hold now.
pub(crate) const CHANGED_SINCE: &str = "it changed since";
/// An undo refused: the block is where this desk doesn't draw what it
/// would put back (the agent moved it since), as its key isn't offered
/// there.
pub(crate) const MOVED_SINCE: &str = "it moved where this desk doesn't draw that";
/// An undo refused: its notice may have gone (`u` undoes a dismissal then).
pub(crate) const NOTICE_WENT: &str = "its notice went · u shows it again";

/// Undos kept at most: every pane's walk keeps `crate::walk::UNDO_MAX`.
pub(crate) const UNDO_KEPT: usize = 4 * crate::walk::UNDO_MAX;

/// What a key changed, to put back: block `r`'s presentation in `conv`
/// as it was and as the key left it, and its desire (in `generation`) as
/// the key left it, if the key made one.
struct Undo {
    conv: String,
    r: Ref,
    pres: (Mark, Mark),
    desire: Option<(Generation, BlockId, Option<Desire>)>,
}

/// An undo's desire not written yet, its presentation put back already:
/// if that write fails, and the desire is back to what the key left, the
/// presentation goes back too ([`Stores::undo_landed`]). Only kept where
/// that can be: a failed write puts the desire back to what drew before
/// its id's first wish not written, so an undo whose key's own wish isn't
/// written either needs none (both stores go back to before that key).
/// Kept by its write's ticket: no other write's landing touches it.
struct Undoing {
    conv: String,
    generation: Generation,
    id: BlockId,
    r: Ref,
    /// The desire as the key left it.
    left: Option<Desire>,
    /// The presentation: put back to this, from this.
    pres: (Mark, Mark),
}

/// A message waiting for its marks: its attachment, and whether a turn
/// ran there as it was typed.
struct Waiting {
    seq: u64,
    busy: bool,
}

/// A carrier sent on `conv`, its history `history` events long then.
struct Sent {
    carrier: u64,
    conv: String,
    history: usize,
}

/// A session's conversation as its last board had it.
struct Track {
    journal: String,
    compactions: u64,
    generation: Generation,
    settled: Arc<Visibility>,
    busy: bool,
}

impl Stores {
    /// Before the launch reads the files ([`AppView::start_wishes`]):
    /// nothing kept. Tests: a folder of the window's own, new (nothing a
    /// crash left there to sweep: no write until a change asks for one).
    pub(crate) fn at_launch() -> Self {
        #[cfg(test)]
        {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = crate::theme::test_home::root().join(format!("wishes-{n}"));
            let _ = std::fs::create_dir_all(&dir);
            let mut s = Self::open(Home::take(Some(dir.join(crate::desires::FILE)), Some(dir.join(crate::presentation::FILE))));
            s.desires.swept();
            s.pres.swept();
            s
        }
        #[cfg(not(test))]
        Self::open(Home::none())
    }

    /// The stores in `home`'s files, read now.
    pub(crate) fn open(home: Home) -> Self {
        Stores {
            desires: Desires::open(home.desires.clone()),
            pres: Presentation::open(home.pres.clone()),
            home,
            waiting: HashMap::new(),
            sent: Vec::new(),
            track: HashMap::new(),
            said: None,
            undos: VecDeque::new(),
            next_undo: 0,
            undoing: BTreeMap::new(),
            #[cfg(test)]
            outbox: Vec::new(),
        }
    }

    /// Tests: every session's last board forgotten, as a window that
    /// never drew them.
    #[cfg(all(test, unix))]
    pub(crate) fn forget_boards(&mut self) {
        self.track.clear();
    }

    /// The writer took no more (it didn't start, or stopped: `why`): the
    /// files are no longer this desk's to write. Memory only from now, said
    /// for as long as it lasts ([`Stores::memory_only`]); what a write out
    /// carried is kept in memory, and the messages waiting for it go.
    pub(crate) fn writer_lost(&mut self, why: &str) -> Vec<Landed> {
        self.pres.writer_lost(why);
        self.desires.writer_lost(why)
    }

    /// While either file isn't this desk's: that it keeps its changes in
    /// memory only, and why.
    pub(crate) fn memory_only(&self) -> Option<String> {
        let why = self.desires.not_owned().or(self.pres.not_owned())?;
        Some(match why {
            NotOwned::Held => format!("{MEMORY_ONLY} · another desk has its files"),
            NotOwned::Failed(why) => format!("{MEMORY_ONLY} · {why}"),
        })
    }

    /// Tests: a write out on either file.
    #[cfg(test)]
    pub(crate) fn out(&self) -> bool {
        self.desires.out() || self.pres.out()
    }

    /// Whether a store changed with no write out or to come (tests: every
    /// change schedules its write, [`AppView::pump_wishes`]).
    #[cfg(test)]
    pub(crate) fn unscheduled(&self) -> bool {
        self.desires.due() || self.pres.due()
    }

    /// What this desk draws of board `d` of conversation `conv` (module
    /// docs).
    pub(crate) fn vis(&self, conv: &str, d: &Drawn) -> DeskVis {
        let mut out = DeskVis { placed: self.pres.user_rail(conv), ..DeskVis::default() };
        let Some(g) = d.generation else { return out };
        let mut hidden = Vec::new();
        let shared = &d.visibility;
        let rail = d.rail.iter().flatten().flat_map(|s| std::iter::once(s).chain(&s.members));
        let rows = d.rows.values().flat_map(|r| r.blocks.iter()).flat_map(|s| std::iter::once(s).chain(&s.members));
        for (in_rail, s) in rail.map(|s| (true, s)).chain(rows.map(|s| (false, s))) {
            let Ref::Block(id) = &s.block.id else { continue };
            if out.seen.contains_key(id) {
                continue;
            }
            // An agent's section the user put into the transcript: drawn
            // there (its tombstone, if dismissed), not here, and not
            // counted here.
            if in_rail && matches!(self.pres.placed(conv, &s.block).place, Place::Transcript(_)) {
                out.seen.insert(id.clone(), Seen::Hidden);
            } else if self.desires.dismissed(conv, &g, id, shared) {
                out.seen.insert(id.clone(), Seen::Hidden);
                out.dismissed += usize::from(in_rail);
                if in_rail {
                    let wish = self.desires.desire(conv, &g, id).map(|w| (w.version, w.dismissed));
                    hidden.push((id.to_string(), s.block.tag, wish, shared.effective_version(id)));
                }
            } else if self.pres.soft(conv, id) {
                let token = u64::from(shared.effective_version(id));
                out.seen.insert(id.clone(), Seen::Tombstone(Tomb { text: SOFT.into(), token }));
            }
        }
        hidden.sort();
        out.hidden = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            hidden.hash(&mut h);
            h.finish()
        };
        out
    }
}

impl Stores {
    /// How block `s`, drawn in conversation `conv`'s transcript from board
    /// `d`, draws on this desk (§3.5): dismissed (this desk's desire, else
    /// the shared record), its tombstone, `◇ tests · dismissed · u show`,
    /// its token its desire and its effective version as drawn; softly
    /// dismissed (across a compaction), the rail's words; pinned into the
    /// rail, its stub; else the block.
    pub(crate) fn look(&self, conv: &str, d: &Drawn, s: &Shown) -> Look {
        let Ref::Block(id) = &s.block.id else { return Look::Full };
        let Some(g) = d.generation else { return Look::Full };
        let shared = &d.visibility;
        if self.desires.dismissed(conv, &g, id, shared) {
            let wish = self.desires.desire(conv, &g, id).map(|w| (w.version, w.dismissed));
            let token = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (wish, shared.effective_version(id)).hash(&mut h);
                h.finish()
            };
            return Look::Tomb { text: format!("◇ {id} · {DISMISSED}").into(), token };
        }
        if self.pres.soft(conv, id) {
            return Look::Tomb { text: format!("◇ {id} · {SOFT}").into(), token: u64::from(shared.effective_version(id)) };
        }
        let placed = self.pres.placed(conv, &s.block);
        if placed.by_user && matches!(placed.place, Place::Rail(_)) {
            let moved = if placed.agent_moved { format!(" · {AGENT_MOVED}") } else { String::new() };
            return Look::Stub { text: format!("◇ {id} · {IN_THE_RAIL}{moved}").into() };
        }
        Look::Full
    }

    /// `aim`'s change made by `f` (it says what happened), kept to undo:
    /// its presentation before and after, and its desire after, if `f`
    /// made one (its generation, its id).
    fn keep(&mut self, aim: &Aim, desire: Option<(Generation, BlockId)>, f: impl FnOnce(&mut Self) -> Result<bool, String>) -> Asked {
        let r = &aim.block.block.id;
        let before = self.pres.mark(aim.conv, r);
        match f(self) {
            Ok(wished) => {
                let after = self.pres.mark(aim.conv, r);
                let desire = desire.filter(|_| wished).map(|(g, id)| {
                    let d = self.desires.wish(aim.conv, &g, &id).cloned();
                    (g, id, d)
                });
                self.next_undo += 1;
                self.undos.push_back((self.next_undo, Undo { conv: aim.conv.to_string(), r: r.clone(), pres: (before, after), desire }));
                if self.undos.len() > UNDO_KEPT {
                    self.undos.pop_front();
                }
                Asked::Done(self.next_undo)
            }
            Err(why) => {
                self.said = Some(format!("{r}: {why}"));
                Asked::Refused(why)
            }
        }
    }

    /// `x` or `u` on `aim`: the desire (on a soft tombstone, `x` a real
    /// notice of the new generation, and the tombstone ends; `u` ends it).
    fn want(&mut self, aim: &Aim, dismiss: bool) -> Asked {
        let wish = if dismiss { Wish::Dismiss } else { Wish::Show };
        let (Ref::Block(id), Some(g)) = (&aim.block.block.id, aim.drawn.generation) else { return Asked::Off };
        if !self.offers(wish, &aim.block.block, matches!(aim.place, Place::Rail(_))) {
            return Asked::Off;
        }
        self.said = None;
        let shared = aim.drawn.visibility.clone();
        // A soft tombstone's `u` ends it; a dismissed one's (the
        // transcript's) is a Show.
        if !dismiss && aim.tomb.is_some() && self.pres.soft(aim.conv, id) && !self.desires.dismissed(aim.conv, &g, id, &shared) {
            return self.keep(aim, None, |s| Ok(s.pres.unsoft(aim.conv, id)));
        }
        self.keep(aim, Some((g, id.clone())), |s| {
            let asked = if dismiss { s.desires.dismiss(aim.conv, &g, id, &shared) } else { s.desires.show(aim.conv, &g, id, &shared) };
            match asked {
                Ok(a) => {
                    if dismiss {
                        s.pres.unsoft(aim.conv, id);
                    }
                    Ok(a == Ask::Saving)
                }
                Err(Refused(why)) => Err(why),
            }
        })
    }
}

/// D7's keys on D6b's stores (module docs).
impl BlockDesires for Stores {
    fn wired(&self) -> bool {
        true
    }

    fn offers(&self, wish: Wish, _b: &Block, rail: bool) -> bool {
        match wish {
            // The rail hides a dismissal (its footer counts it), the
            // transcript draws its tombstone.
            Wish::Dismiss | Wish::Show => true,
            // A transcript block into the rail (its stub stays), an
            // agent's section into the transcript, and back.
            Wish::Pin => true,
            Wish::Slot => rail,
        }
    }

    fn placed(&self, conv: &str, b: &Block) -> Place {
        self.pres.placed(conv, b).place
    }

    fn dismiss(&mut self, aim: &Aim) -> Asked {
        self.want(aim, true)
    }

    fn show(&mut self, aim: &Aim) -> Asked {
        self.want(aim, false)
    }

    fn pin(&mut self, aim: &Aim) -> Asked {
        if !self.offers(Wish::Pin, &aim.block.block, false) {
            return Asked::Off;
        }
        self.said = None;
        self.keep(aim, None, |s| s.pres.pin(aim.conv, &aim.block.block).map(|_| false))
    }

    fn slot(&mut self, aim: &Aim) -> Asked {
        if !matches!(aim.place, Place::Rail(_)) {
            return Asked::Off;
        }
        self.said = None;
        self.keep(aim, None, |s| s.pres.slot(aim.conv, &aim.block.block).map(|_| false))
    }

    /// All of it put back, or nothing: only where its key is offered now
    /// (the block as bound, where it draws), while its presentation and
    /// its desire are what the key left, and no notice of that desire may
    /// have gone (a delivered dismissal is undone with Show).
    fn undo(&mut self, aim: &Aim, wish: Wish, token: u64) -> Asked {
        self.retire();
        let Some(k) = self.undos.iter().position(|(t, _)| *t == token) else {
            return Asked::Refused("too long ago to undo".into());
        };
        let u = &self.undos[k].1;
        // The inverse is the same key's (`x` and `u`, `p`, `m`): refused
        // where that key is, the scope it was offered in.
        let offered = self.offers(wish, &aim.block.block, matches!(aim.place, Place::Rail(_)));
        let mut why = (!offered).then_some(MOVED_SINCE);
        if self.pres.mark(&u.conv, &u.r) != u.pres.1 {
            why = why.or(Some(CHANGED_SINCE));
        }
        if let Some((g, id, after)) = &u.desire {
            match (self.desires.wish(&u.conv, g, id), after) {
                (None, None) => {}
                (Some(now), Some(then)) if (now.dismissed, now.version) == (then.dismissed, then.version) => {
                    if now.sent || now.in_flight() {
                        why = why.or(Some(NOTICE_WENT));
                    }
                }
                _ => why = why.or(Some(CHANGED_SINCE)),
            }
        }
        if let Some(why) = why {
            self.said = Some(format!("{}: {why}", u.r));
            return Asked::Refused(why.into());
        }
        // The presentation's half, fitted to the caps first: none of it if
        // it wouldn't keep what it puts back.
        let restored = match self.pres.restored(&u.conv, &u.r, &u.pres.0) {
            Ok(b) => b,
            Err(why) => {
                self.said = Some(format!("{}: {why}", u.r));
                return Asked::Refused(why);
            }
        };
        if let Some((g, id, left)) = &u.desire {
            let shared = &aim.drawn.visibility;
            // The opposite wish: over one no notice went for, a cancel.
            let undone = match wish {
                Wish::Dismiss => self.desires.show(&u.conv, g, id, shared),
                _ => self.desires.dismiss(&u.conv, g, id, shared),
            };
            match undone {
                Err(Refused(why)) => {
                    self.said = Some(format!("{id}: {why}"));
                    return Asked::Refused(why);
                }
                // A write away, and the presentation with it: what its
                // failure needs kept (type docs), its write's newest, until
                // that write lands or is retired. One per write queued,
                // never dropped for room.
                Ok(Ask::Saving) if u.pres.0 != u.pres.1 => {
                    if let Some(ticket) = self.desires.queued(&u.conv, g, id)
                        && same(self.desires.desire(&u.conv, g, id), left.as_ref())
                    {
                        let (conv, id, r, left, pres) = (u.conv.clone(), id.clone(), u.r.clone(), left.clone(), u.pres.clone());
                        self.undoing.insert(ticket, Undoing { conv, generation: *g, id, r, left, pres });
                    }
                }
                Ok(_) => {}
            }
        }
        self.undos.remove(k);
        self.pres.install(restored);
        Asked::Done(token)
    }
}

impl Stores {
    /// Desire write `ticket` landed (`kept`) or failed: an undo it carried
    /// stands, or, the desire back to what its key left, its presentation
    /// goes back too (unless it changed since): all of it, or nothing.
    /// Only that write's own undo: never another's of the id.
    pub(crate) fn undo_landed(&mut self, ticket: u64, kept: bool) {
        let Some(u) = self.undoing.remove(&ticket) else { return };
        if !kept
            && same(self.desires.wish(&u.conv, &u.generation, &u.id), u.left.as_ref())
            && self.pres.mark(&u.conv, &u.r) == u.pres.0
            && let Err(why) = self.pres.put_back(&u.conv, &u.r, &u.pres.1)
        {
            log::warn!(target: "synaps_desktop::blocks", "{}: not put back as its key left it: {why}", u.r);
        }
    }

    /// The undos whose write is no longer queued, retired (a settle of
    /// another generation, a compaction, a discard took it): nothing put
    /// back, as its write never landed.
    fn retire(&mut self) {
        let desires = &self.desires;
        self.undoing.retain(|t, u| desires.queued(&u.conv, &u.generation, &u.id) == Some(*t));
    }

    /// Tests: the undos kept for their writes.
    #[cfg(test)]
    pub(crate) fn undoing(&self) -> usize {
        self.undoing.len()
    }
}

/// What this desk shows of one board ([`Stores::vis`]).
#[derive(Default)]
pub(crate) struct DeskVis {
    seen: HashMap<BlockId, Seen>,
    placed: Vec<(Ref, Slot)>,
    dismissed: usize,
    /// The rail's hidden blocks, as one tag ([`RailVisibility::hidden`]).
    hidden: u64,
}

impl RailVisibility for DeskVis {
    fn seen(&self, id: &Ref) -> Seen {
        match id {
            Ref::Block(b) => self.seen.get(b).cloned().unwrap_or(Seen::Shown),
            _ => Seen::Shown,
        }
    }

    fn placed(&self) -> &[(Ref, Slot)] {
        &self.placed
    }

    fn dismissed(&self) -> usize {
        self.dismissed
    }

    fn hidden(&self) -> u64 {
        self.hidden
    }
}

/// The chip's words for what rides: `plan, tests dismissed · notes shown`.
pub(crate) fn tells(riding: &[(BlockId, Notice, u32)]) -> Option<String> {
    let kind = |k: Notice, word: &str| {
        let ids: Vec<String> = riding.iter().filter(|(_, s, _)| *s == k).map(|(id, ..)| id.to_string()).collect();
        (!ids.is_empty()).then(|| format!("{} {word}", ids.join(", ")))
    };
    let parts: Vec<String> = [kind(Notice::Dismissed, "dismissed"), kind(Notice::Shown, "shown")].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| format!("+ tells the agent: {} ×", parts.join(" · ")))
}

impl AppView {
    /// How block `s` draws in the pane acted on's transcript, on board `d`
    /// ([`Stores::look`]; D7's stand-in draws every block).
    pub(crate) fn look_on(&self, d: &Drawn, s: &Shown) -> Look {
        #[cfg(test)]
        if self.desires.is_some() {
            return Look::Full;
        }
        self.wishes.look(&self.conversation(self.panes.cur.live), d, s)
    }

    /// How block `s` draws in the pane acted on's transcript now: on the
    /// board its rows draw.
    pub(crate) fn drawn_look(&self, s: &Shown) -> Look {
        match self.model.lives[self.panes.cur.live].transcript.drawn() {
            Some(d) => self.look_on(d, s),
            None => Look::Full,
        }
    }

    /// The agent's rail sections the user put into store `live`'s
    /// transcript (§3.5's `p`), on board `d`, by their creating call's row
    /// ([`crate::blocks::Pinned`]).
    pub(crate) fn pinned_in(&self, live: LiveId, d: &Drawn) -> crate::blocks::Pinned {
        let mut out = crate::blocks::Pinned::new();
        let conv = self.conversation(live);
        let desk = self.block_desires();
        for (list, rows) in d.rail.iter().zip(&d.rail_rows) {
            for (s, row) in list.iter().zip(rows) {
                if let Some(row) = row
                    && matches!(desk.placed(&conv, &s.block), Place::Transcript(_))
                {
                    out.entry(*row).or_default().push(s.clone());
                }
            }
        }
        out
    }

    /// The pane acted on's sections put into its transcript, as its board
    /// has them now (its session's own transcript; a worker's view has
    /// none): what changed since the list was built takes them, and the
    /// first item whose row has them, then or now, is where it's built
    /// again from.
    pub(crate) fn sync_pinned(&mut self) -> Option<usize> {
        let live = self.panes.cur.live;
        let l = &self.model.lives[live];
        let board = l.shown().has_blocks() && l.watch.focused().is_none();
        let now = match l.transcript.drawn().filter(|_| board).cloned() {
            Some(d) => self.pinned_in(live, &d),
            None => crate::blocks::Pinned::new(),
        };
        if now == self.panes.cur.pinned_in {
            return None;
        }
        let rows: std::collections::HashSet<u64> = now.keys().chain(self.panes.cur.pinned_in.keys()).copied().collect();
        let items = self.model.lives[live].shown().items();
        let first = items.iter().position(|it| rows.contains(&it.id)).unwrap_or(items.len());
        self.panes.cur.pinned_in = now;
        Some(first)
    }

    /// The pane acted on's block rows this desk draws in one line (a
    /// tombstone, a stub), against the board its rows draw: a row whose
    /// look changed since its last measure is measured again (its height
    /// is its look's).
    pub(crate) fn sync_looks(&mut self) {
        let d = self.model.lives[self.panes.cur.live].transcript.drawn().cloned();
        let items = self.model.lives[self.panes.cur.live].shown().items();
        let mut now = HashMap::new();
        let mut changed = Vec::new();
        for (ix, row) in self.panes.cur.disp.iter().enumerate() {
            let crate::turns::Kind::Block { at } = row.kind else { continue };
            let look = match (&d, items.get(row.item).and_then(|it| crate::blocks::row_block(it, at, &self.panes.cur.pinned_in))) {
                (Some(d), Some(s)) => self.look_on(d, s),
                _ => Look::Full,
            };
            if self.panes.cur.looks.get(&row.id).unwrap_or(&Look::Full) != &look {
                changed.push(ix);
            }
            if look != Look::Full {
                now.insert(row.id, look);
            }
        }
        self.panes.cur.looks = now;
        for ix in changed {
            self.panes.cur.list.remeasure_items(ix..ix + 1);
        }
    }

    /// Store `live`'s conversation: its journal (a daemon that names none:
    /// its session).
    pub(crate) fn conversation(&self, live: LiveId) -> String {
        let l = &self.model.lives[live];
        match l.transcript.journal() {
            "" => l.active.as_ref().map(|a| a.session_id.clone()).unwrap_or_default(),
            j => j.to_string(),
        }
    }

    /// Store `live`'s conversation, and its board as drawn (its
    /// generation and records: what a click is made on).
    fn wish_board(&self, live: LiveId) -> Option<(String, Generation, Arc<Visibility>)> {
        let d = self.model.lives[live].transcript.drawn()?;
        Some((self.conversation(live), d.generation?, d.visibility.clone()))
    }

    /// The launch: both files next to `settings.json` (`home`'s: owned by
    /// this process if no other desk has them, else memory only), and what
    /// a crash left beside them swept.
    pub(crate) fn start_wishes(&mut self, home: Home, cx: &mut Context<Self>) {
        self.wishes = Stores::open(home);
        // A file set aside: said (memory only is said for as long as it
        // lasts, [`Stores::memory_only`]).
        if self.wishes.desires.owns_file() {
            self.wishes.said = self.wishes.desires.trouble.clone();
        }
        if self.wishes.pres.owns_file() && self.wishes.said.is_none() {
            self.wishes.said = self.wishes.pres.trouble.clone();
        }
        self.pump_wishes(cx);
        // The rail draws what it has now (memory only, said).
        cx.notify();
    }

    /// The window's end, or the app's: each file's last snapshot (what
    /// memory changed since the file has), handed to the writer after what
    /// it holds; the app's quit waits for it ([`Home::on_quit`]). Memory
    /// only: nothing (no file to write).
    pub(crate) fn flush_wishes(&mut self) {
        let writer = self.wishes.home.writer.clone();
        if let Some(w) = self.wishes.desires.last_write() {
            drop(writer.run(move || drop(w.run())));
        }
        if let Some(w) = self.wishes.pres.last_write() {
            drop(writer.run(move || drop(w.run())));
        }
    }

    /// `job` handed to the writer; what it returns, once it ran. Not taken
    /// (the writer didn't start or stopped): why; it never runs. (Tests:
    /// the writer queues, and this window's executor runs what it holds.)
    fn hand<R: Send + 'static>(
        &self,
        job: impl FnOnce() -> R + Send + 'static,
        cx: &mut Context<Self>,
    ) -> Result<async_channel::Receiver<R>, String> {
        let writer = self.wishes.home.writer.clone();
        let done = writer.run(job).map_err(|s| s.0);
        #[cfg(test)]
        cx.background_spawn(async move { writer.drain() }).detach();
        #[cfg(not(test))]
        let _ = cx;
        done
    }

    /// Writes to make, each file's next, on the writer (off the UI thread),
    /// never here. The writer took none, or stopped before it ran one:
    /// memory only from then ([`Stores::writer_lost`]).
    pub(crate) fn pump_wishes(&mut self, cx: &mut Context<Self>) {
        // Memory only now: what memory changed since is kept there, and
        // the messages it held go; again until nothing is left to write.
        while let Some(why) = self.pump_once(cx) {
            let landed = self.wishes.writer_lost(&why);
            self.wishes_landed(landed, cx);
        }
    }

    /// Each file's next write handed over; why not, if the writer took
    /// none.
    fn pump_once(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let mut lost = None;
        if let Some(w) = self.wishes.desires.next_write() {
            match self.hand(move || w.run(), cx) {
                Ok(done) => {
                    cx.spawn(async move |this, cx| {
                        let landed = done.recv().await;
                        let _ = this.update(cx, |v, cx| {
                            match landed {
                                Ok(l) => {
                                    let landed = v.wishes.desires.landed(l);
                                    v.wishes_landed(landed, cx);
                                }
                                // Dropped unrun: the writer stopped.
                                Err(_) => {
                                    let landed = v.wishes.writer_lost("its writer stopped");
                                    v.wishes_landed(landed, cx);
                                }
                            }
                            v.pump_wishes(cx);
                            cx.notify();
                        });
                    })
                    .detach();
                }
                Err(why) => lost = Some(why),
            }
        }
        if let Some(w) = self.wishes.pres.next_write() {
            match self.hand(move || w.run(), cx) {
                Ok(done) => {
                    cx.spawn(async move |this, cx| {
                        let landed = done.recv().await;
                        let _ = this.update(cx, |v, cx| {
                            match landed {
                                Ok(l) => {
                                    if let Err(why) = v.wishes.pres.landed(l) {
                                        v.wishes.said = Some(why);
                                    }
                                }
                                Err(_) => {
                                    let landed = v.wishes.writer_lost("its writer stopped");
                                    v.wishes_landed(landed, cx);
                                }
                            }
                            v.pump_wishes(cx);
                            cx.notify();
                        });
                    })
                    .detach();
                }
                Err(why) => lost = lost.or(Some(why)),
            }
        }
        lost
    }

    /// A write landed: a wish drawn or refused, the messages whose turn
    /// came sent.
    fn wishes_landed(&mut self, landed: Vec<Landed>, cx: &mut Context<Self>) {
        for l in landed {
            match l {
                Landed::Wish { id, ticket, done, .. } => {
                    self.wishes.undo_landed(ticket, done.is_ok());
                    if let Err(Refused(why)) = done {
                        self.wishes.said = Some(format!("{id}: {why}"));
                    }
                }
                Landed::Send { ticket, ride } => {
                    if let Some(w) = self.wishes.waiting.remove(&ticket) {
                        self.send_ride(ticket, w.seq, w.busy, ride);
                    }
                }
            }
        }
        cx.notify();
    }

    /// `ride` (waiting on `ticket`) to the attachment `seq`: a steer while
    /// a turn runs there, else a submit. Sent, its echo is its text as
    /// sent; not sent, its notices ride the next message and its body goes
    /// back to the pane's composer.
    fn send_ride(&mut self, ticket: u64, seq: u64, busy: bool, ride: Ride) {
        let live = self.model.lives.ids().into_iter().find(|l| self.model.lives[*l].attachment() == Some(seq));
        let busy = live.map_or(busy, |l| self.model.lives[l].transcript.busy());
        let sent = match &self.client {
            Some(c) if live.is_some() => {
                if busy {
                    c.on(seq).steer(ride.text.clone()).is_ok()
                } else {
                    c.on(seq).submit(ride.text.clone()).is_ok()
                }
            }
            _ => false,
        };
        match live {
            Some(l) => {
                if let Some((crate::live::Target::Pane(p), body)) =
                    self.model.lives[l].in_flight.went(ticket, sent.then(|| ride.text.clone()))
                {
                    self.effect(crate::model::Effect::Restore(p, body));
                }
                self.ride_went(l, &ride.text, ride.carrier, sent)
            }
            None => {
                if let Some(k) = ride.carrier {
                    self.wishes.desires.lost(k);
                }
                // Its attachment left (a tab switch): not sent; its text
                // back to its pane, as a refusal there gives it.
                if let Some(e) = self.model.left_went(seq, ticket) {
                    self.effect(e);
                }
            }
        }
        if let (false, Some(l)) = (sent, live) {
            self.model.lives[l].last_error = Some("the connection thread has stopped".into());
        }
    }

    /// Message `text` of store `live`, its notices on `carrier`, went to
    /// the daemon (`sent`), or not: its notices ride the next one.
    pub(crate) fn ride_went(&mut self, live: LiveId, text: &str, carrier: Option<u64>, sent: bool) {
        #[cfg(test)]
        if sent {
            self.wishes.outbox.push(text.to_string());
        }
        let _ = text;
        let Some(carrier) = carrier else { return };
        if sent {
            let history = self.model.lives[live].transcript.drawn().map_or(0, |d| d.history);
            self.wishes.sent.push(Sent { carrier, conv: self.conversation(live), history });
            if self.wishes.sent.len() > SENT_MAX {
                self.wishes.sent.remove(0);
            }
        } else {
            self.wishes.desires.lost(carrier);
        }
    }

    /// Pane `pane`'s `text` for store `live` (attachment `seq`), as it
    /// goes `busy` or not, with the notices that ride it: `Go` to send now
    /// ([`AppView::ride_went`] after), `Wait` sent once its marks are
    /// written, `Err` kept in the composer, why.
    ///
    /// Waiting, it is in flight (on its ticket, [`crate::live::InFlight::went`])
    /// before anything can send it: the writer may take no write now (memory
    /// only from then), and then it goes before this returns (Shady #103
    /// R5-P2).
    pub(crate) fn ride_notices(
        &mut self,
        live: LiveId,
        pane: crate::pane::PaneId,
        seq: u64,
        busy: bool,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<Riding, String> {
        let Some((conv, g, shared)) = self.wish_board(live) else {
            return Ok(Riding::Go(Ride { text, carrier: None, notices: Vec::new() }));
        };
        let how = if busy { Carry::Steer } else { Carry::Submit };
        match self.wishes.desires.ride(&conv, &g, &shared, &text, how) {
            Ok(Riding::Wait(ticket)) => {
                self.model.lives[live].waiting(pane, text, ticket);
                self.wishes.waiting.insert(ticket, Waiting { seq, busy });
                self.pump_wishes(cx);
                Ok(Riding::Wait(ticket))
            }
            Ok(go) => Ok(go),
            Err(Refused(why)) => Err(why),
        }
    }

    /// The composer's chip for the focused pane's session: what its next
    /// message tells the agent.
    pub(crate) fn tells_chip(&self) -> Option<String> {
        let (conv, g, shared) = self.wish_board(self.panes.cur.live)?;
        tells(&self.wishes.desires.riding(&conv, &g, &shared))
    }

    /// The chip's click: what it says kept back from the next message.
    pub(crate) fn keep_back(&mut self, cx: &mut Context<Self>) {
        let Some((conv, g, shared)) = self.wish_board(self.panes.cur.live) else { return };
        for (id, ..) in self.wishes.desires.riding(&conv, &g, &shared) {
            self.wishes.desires.hold(&conv, &g, &id, &shared);
        }
        self.pump_wishes(cx);
        cx.notify();
    }

    /// A soft tombstone's `u`: it ends.
    pub(crate) fn unsoft_block(&mut self, live: LiveId, id: &Ref, cx: &mut Context<Self>) {
        let Ref::Block(id) = id else { return };
        let conv = self.conversation(live);
        self.wishes.pres.unsoft(&conv, id);
        self.pump_wishes(cx);
    }

    /// The footer's `show`: every section of store `live`'s rail this desk
    /// hides, shown.
    pub(crate) fn show_dismissed(&mut self, live: LiveId, d: &Drawn, cx: &mut Context<Self>) {
        let Some((conv, g, shared)) = self.wish_board(live) else { return };
        let hidden: Vec<BlockId> = d
            .rail
            .iter()
            .flatten()
            .flat_map(|s: &Shown| std::iter::once(s).chain(&s.members))
            .filter_map(|s| match &s.block.id {
                Ref::Block(id) if self.wishes.desires.dismissed(&conv, &g, id, &shared) => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.wishes.said = None;
        for id in hidden {
            if let Err(Refused(why)) = self.wishes.desires.show(&conv, &g, &id, &shared) {
                self.wishes.said = Some(format!("{id}: {why}"));
            }
        }
        self.pump_wishes(cx);
    }

    /// Board `d` drawn for store `live`: a compaction's dismissals carried
    /// into the successor, and the desires settled against its history.
    pub(crate) fn wishes_on_board(&mut self, live: LiveId, d: &Drawn) {
        let l = &self.model.lives[live];
        let (Some(session), Some(g)) = (l.active.as_ref().map(|a| a.session_id.clone()), d.generation) else { return };
        let (compactions, busy) = (l.transcript.compactions(), l.transcript.busy());
        let conv = self.conversation(live);
        let was = self.wishes.track.remove(&session);
        if let Some(t) = &was
            && t.journal != conv
            && compactions > t.compactions
        {
            let ids = self.wishes.desires.compacted(&t.journal, &t.generation, &t.settled);
            self.wishes.pres.compacted(&t.journal, &conv, &ids);
            self.wishes.retire();
        }
        let changed = was
            .as_ref()
            .is_none_or(|t| t.journal != conv || t.generation != g || !Arc::ptr_eq(&t.settled, &d.settled) || (t.busy && !busy));
        // Only attached sessions are tracked.
        let attached: Vec<String> =
            self.model.lives.ids().into_iter().filter_map(|l| self.model.lives[l].active.as_ref().map(|a| a.session_id.clone())).collect();
        self.wishes.track.retain(|s, _| attached.contains(s));
        self.wishes.track.insert(session, Track { journal: conv.clone(), compactions, generation: g, settled: d.settled.clone(), busy });
        if !changed {
            return;
        }
        // In flight: sent, and its turn not over with the history grown.
        let flying = |s: &Sent| s.conv == conv && (busy || d.history <= s.history);
        let in_flight: Vec<u64> = self.wishes.sent.iter().filter(|s| flying(s)).map(|s| s.carrier).collect();
        self.wishes.sent.retain(|s| s.conv != conv || in_flight.contains(&s.carrier));
        for (id, f) in self.wishes.desires.settle(&conv, &g, &d.settled, &in_flight) {
            self.wishes.said = Some(format!("{id}: {}", f.text()));
        }
        self.wishes.retire();
    }
}

#[cfg(test)]
#[path = "rail_wishes_tests.rs"]
mod tests;
