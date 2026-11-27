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
//!   placements draw first in their slots.
//! - **The rail's actions** (a section header's hover actions, the footer,
//!   a tombstone): `×` dismisses (on a soft one, a real notice of the new
//!   generation, and the tombstone ends), `⇅` moves it to the other slot
//!   (again: the agent's), `show` shows every section this rail hides, and
//!   a tombstone's click ends it (`u`). Each is checked against the bound
//!   board first, as D6a's targets are.
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
//! - **Writes** run off the UI thread, one at a time per file
//!   ([`AppView::pump_wishes`]); what a refusal or a settle says shows in
//!   the rail's footer.

use std::collections::HashMap;
use std::sync::Arc;

use cards_spec::block::message::Visibility as Notice;
use cards_spec::block::{BlockId, Generation, Slot};
use cards_spec::board::{Block, Visibility};
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Ref, Shown};

use crate::agent_rail::{RailVisibility, Seen, Tomb};
use crate::app::AppView;
use crate::desires::{Carry, Desires, Landed, Refused, Ride, Riding};
use crate::live::LiveId;
use crate::presentation::Presentation;

/// A soft dismissal's tombstone (§3.5).
pub(crate) const SOFT: &str = "dismissed before compaction · u show";
/// Carriers remembered as sent, at most: the oldest is forgotten first
/// (its notices then ride again at the same version; a second copy is
/// stale).
const SENT_MAX: usize = 256;

/// This desk's two stores, and what wiring them keeps.
pub(crate) struct Stores {
    pub(crate) desires: Desires,
    pub(crate) pres: Presentation,
    /// Messages waiting for their notices' marks: where each goes.
    waiting: HashMap<u64, Waiting>,
    /// Carriers sent, until a settled history no longer has them in flight.
    sent: Vec<Sent>,
    /// Per session: the conversation its last board was of.
    track: HashMap<String, Track>,
    /// What the rail's footer says last: a refusal, or a settle's why.
    pub(crate) said: Option<String>,
    /// Tests: every message sent to a session, as it went.
    #[cfg(test)]
    pub(crate) outbox: Vec<String>,
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
    /// nothing kept. Tests: a folder of the window's own.
    pub(crate) fn at_launch() -> Self {
        #[cfg(test)]
        {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = crate::theme::test_home::root().join(format!("wishes-{n}"));
            let _ = std::fs::create_dir_all(&dir);
            Self::with(Some(dir.join(crate::desires::FILE)), Some(dir.join(crate::presentation::FILE)))
        }
        #[cfg(not(test))]
        Self::with(None, None)
    }

    pub(crate) fn with(desires: Option<std::path::PathBuf>, pres: Option<std::path::PathBuf>) -> Self {
        Stores {
            desires: Desires::load(desires),
            pres: Presentation::load(pres),
            waiting: HashMap::new(),
            sent: Vec::new(),
            track: HashMap::new(),
            said: None,
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

    /// What this desk draws of board `d` of conversation `conv` (module
    /// docs).
    pub(crate) fn vis(&self, conv: &str, d: &Drawn) -> DeskVis {
        let mut out = DeskVis { placed: self.pres.user_rail(conv), ..DeskVis::default() };
        let Some(g) = d.generation else { return out };
        let shared = &d.visibility;
        let rail = d.rail.iter().flatten().flat_map(|s| std::iter::once(s).chain(&s.members));
        let rows = d.rows.values().flat_map(|r| r.blocks.iter()).flat_map(|s| std::iter::once(s).chain(&s.members));
        for (in_rail, s) in rail.map(|s| (true, s)).chain(rows.map(|s| (false, s))) {
            let Ref::Block(id) = &s.block.id else { continue };
            if out.seen.contains_key(id) {
                continue;
            }
            if self.desires.dismissed(conv, &g, id, shared) {
                out.seen.insert(id.clone(), Seen::Hidden);
                out.dismissed += usize::from(in_rail);
            } else if self.pres.soft(conv, id) {
                let token = u64::from(shared.effective_version(id));
                out.seen.insert(id.clone(), Seen::Tombstone(Tomb { text: SOFT.into(), token }));
            }
        }
        out
    }
}

/// What this desk shows of one board ([`Stores::vis`]).
#[derive(Default)]
pub(crate) struct DeskVis {
    seen: HashMap<BlockId, Seen>,
    placed: Vec<(Ref, Slot)>,
    dismissed: usize,
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
    /// Store `live`'s conversation: its journal (a daemon that names none:
    /// its session).
    pub(crate) fn conversation(&self, live: LiveId) -> String {
        let l = &self.model.lives[live];
        match l.transcript.journal_id() {
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

    /// The launch: both files next to `settings.json`, owned by this
    /// process if no other desk has them (else memory only), and what a
    /// crash left beside them swept.
    pub(crate) fn start_wishes(&mut self, cx: &mut Context<Self>) {
        self.wishes = Stores::with(crate::desires::path(), crate::presentation::path());
        // Another desk's files (memory only here), or one set aside: said.
        self.wishes.said = self.wishes.desires.trouble.clone().or_else(|| self.wishes.pres.trouble.clone());
        self.pump_wishes(cx);
    }

    /// Writes to make, each file's next, off the UI thread.
    pub(crate) fn pump_wishes(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.wishes.desires.next_write() {
            cx.spawn(async move |this, cx| {
                let l = cx.background_spawn(async move { w.run() }).await;
                let _ = this.update(cx, |v, cx| {
                    let landed = v.wishes.desires.landed(l);
                    v.wishes_landed(landed, cx);
                    v.pump_wishes(cx);
                    cx.notify();
                });
            })
            .detach();
        }
        if let Some(w) = self.wishes.pres.next_write() {
            cx.spawn(async move |this, cx| {
                let l = cx.background_spawn(async move { w.run() }).await;
                let _ = this.update(cx, |v, cx| {
                    if let Err(why) = v.wishes.pres.landed(l) {
                        v.wishes.said = Some(why);
                    }
                    v.pump_wishes(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// A write landed: a wish drawn or refused, the messages whose turn
    /// came sent.
    fn wishes_landed(&mut self, landed: Vec<Landed>, cx: &mut Context<Self>) {
        for l in landed {
            match l {
                Landed::Wish { id, done: Err(Refused(why)), .. } => self.wishes.said = Some(format!("{id}: {why}")),
                Landed::Wish { .. } => {}
                Landed::Send { ticket, ride } => {
                    if let Some(w) = self.wishes.waiting.remove(&ticket) {
                        self.send_ride(w.seq, w.busy, ride);
                    }
                }
            }
        }
        cx.notify();
    }

    /// `ride` to the attachment `seq`: a steer while a turn runs there,
    /// else a submit. Not sent: its notices ride the next message.
    fn send_ride(&mut self, seq: u64, busy: bool, ride: Ride) {
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
            Some(l) => self.ride_went(l, &ride.text, ride.carrier, sent),
            None => {
                if let Some(k) = ride.carrier {
                    self.wishes.desires.lost(k);
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

    /// The composer's `text` for store `live` (attachment `seq`), as it
    /// goes `busy` or not, with the notices that ride it: `Ok(Some)` to
    /// send now ([`AppView::ride_went`] after), `Ok(None)` sent once its
    /// marks are written, `Err` kept in the composer, why.
    pub(crate) fn ride_notices(
        &mut self,
        live: LiveId,
        seq: u64,
        busy: bool,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<Option<Ride>, String> {
        let Some((conv, g, shared)) = self.wish_board(live) else { return Ok(Some(Ride { text, carrier: None, notices: Vec::new() })) };
        let how = if busy { Carry::Steer } else { Carry::Submit };
        match self.wishes.desires.ride(&conv, &g, &shared, &text, how) {
            Ok(Riding::Go(ride)) => Ok(Some(ride)),
            Ok(Riding::Wait(ticket)) => {
                self.wishes.waiting.insert(ticket, Waiting { seq, busy });
                self.pump_wishes(cx);
                Ok(None)
            }
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
        cx.notify();
    }

    /// `×` on block `b` of store `live`'s rail.
    pub(crate) fn dismiss_block(&mut self, live: LiveId, b: &Block, cx: &mut Context<Self>) {
        let (Ref::Block(id), Some((conv, g, shared))) = (&b.id, self.wish_board(live)) else { return };
        self.wishes.said = None;
        match self.wishes.desires.dismiss(&conv, &g, id, &shared) {
            Ok(_) => {
                // A soft one: the real notice now, the tombstone ends.
                self.wishes.pres.unsoft(&conv, id);
            }
            Err(Refused(why)) => self.wishes.said = Some(format!("{id}: {why}")),
        }
        self.pump_wishes(cx);
    }

    /// `⇅` on block `b` of store `live`'s rail.
    pub(crate) fn slot_block(&mut self, live: LiveId, b: &Block, cx: &mut Context<Self>) {
        let conv = self.conversation(live);
        self.wishes.said = self.wishes.pres.slot(&conv, b).err();
        self.pump_wishes(cx);
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
    }
}

#[cfg(test)]
#[path = "rail_wishes_tests.rs"]
mod tests;
