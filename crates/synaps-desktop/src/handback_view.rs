//! Handing back, in the window (synaps-cards `docs/blocks.md` v1.3 §3.2,
//! §3.4, §3.6, §3.7; §8.2's D8 row, its second half: D8b). The model is
//! [`crate::handback`]; this sends, and draws what it says.
//!
//! - **Hand back** (Enter on a held block, or its button): the pre-check
//!   on the bound board ([`crate::drafts::Draft::handing`]), the memo's
//!   admission ([`crate::handback::HandBacks::may_hand_back`]: the button
//!   follows it), the notices that ride it, then the card path's send.
//!   The draft then draws over the board marked `SENDING · r4` (or
//!   `QUEUED`, `HELD`, `CHECKING`), takes no key and no new hold, until
//!   its edit is out of flight and a board that has read the history it is
//!   in is drawn ([`AppView::settle_sent_drafts`]): a drawing only.
//! - **Waiting for the notices' marks** (§3.5): a hand-back whose notices
//!   aren't written yet waits ([`EditSend`], on its ticket). No memo entry
//!   owns it then, so its draft does: marked waiting
//!   ([`crate::drafts::Draft::waiting`]), it takes no key and no new hold,
//!   nothing settles it, and an end (a replace, a compaction) leaves it as
//!   any draft, its text recoverable. One edit of an incarnation at a
//!   time counts a wait.
//! - **At the wire** ([`AppView::edit_go`]): the card's gate again (input
//!   authority, no compaction, no dropped events, the same attachment),
//!   and for a new edit the pre-check again on the bound board as it is
//!   then: the same message, or nothing goes and the draft is held again.
//!   The memo's transition is staged ([`crate::handback::Staged`]) and
//!   committed only once the connection took the message: a send that
//!   didn't leave moves no entry, and a refusal is routed by the wire's
//!   order to its own input ([`crate::deliver::Memos::refused`]).
//! - **Restore** (`R` on a past revision in the walk, §3.6): an ordinary
//!   edit of the current incarnation, back to the user paths of `rN` that
//!   differ now ([`crate::drafts::Draft::restore`]), line 1 `back to rN:`,
//!   the same pre-check, guards and gate. An earlier incarnation's is
//!   view-only. One that waits for the bound board is bound to what was
//!   pressed ([`RestoreWanted`]): that walk, that revision, that session,
//!   or nothing and the block says why.
//! - **The block's delivery lines** ([`AppView::edit_delivery`]): every
//!   hand-back of the incarnation this desk keeps while it says something
//!   (out, not sent, not confirmed, covered by another's), each with its
//!   own Retry, Resend (behind the same gate, the notices built afresh)
//!   and Discard, aimed at its own entry. One whose incarnation ended is
//!   in the drafts dock, with Copy and Discard, no Resend
//!   ([`AppView::ended_hand_backs`]).
//! - **A hand-back's row** ([`AppView::render_hand_back`]): line 1's
//!   words, and what the board says it did (`applied 3 of 4 · deploy
//!   already ran`). Two lines, always: what lands later never changes its
//!   height. An outcome the board no longer keeps (§1.8: the latest 32) is
//!   re-folded in the one scratch (`Target::Outcome`) when it is free,
//!   `loading…` until then; what it found is kept for the
//!   [`OUTCOMES_KEPT`] latest rows ([`KeptOutcome`]).
//! - **`compacted away`** (§5.2): a hand-back a compaction carried into
//!   its successor (`Transcript::carried`: the history's own word) whose
//!   block isn't there.
//! - **`changed your edit`** (§3.4 step 6): on a block whose path the
//!   agent overwrote after the user, until the user next focuses it (the
//!   walk's ring on it, or a take). The path, on hover. What was seen is
//!   an overwrite, not a path: kept with the revision it was seen at
//!   ([`Seen`], at most [`SEEN_MAX`]), it shows again once the user wrote
//!   the path since and it isn't the user's now.
//! - **`another edit arrived · r7`** (§5.4): on a held draft, once a
//!   hand-back other than this desk's lands on its incarnation after `B`
//!   ([`crate::drafts::Draft::note_other`]).

use std::sync::Arc;
use std::time::Instant;

use cards_spec::block::message::{self, Edit};
use cards_spec::block::{Author, Path, Ref, Tag};
use cards_spec::board::{Code, EditResult, OUTCOME_MAX_OWNED, Pos, Target};
use cards_spec::memo::State;
use cards_spec::memo::block as bm;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Shown};
use synaps_client::transcript::Item;

use crate::app::AppView;
use crate::desires::{Carry, Ride, Riding};
use crate::drafts::{DRAFT_FULL, DRAFT_MAX_BYTES, Draft};
use crate::fonts::{self, TextPx as _};
use crate::handback::{EditOut, Line1, SENDING, Staging};
use crate::live::LiveId;
use crate::theme::{Metrics, Palette};

/// `changed your edit` overwrites seen, kept at most (§1.8): the oldest goes.
pub(crate) const SEEN_MAX: usize = 256;

/// Re-folded outcomes kept at most (§1.8), the oldest going first.
pub(crate) const OUTCOMES_KEPT: usize = 32;

/// A restore with nothing to put back.
pub(crate) const NOTHING_TO_RESTORE: &str = "nothing to restore · it is as r{} now";

/// A restore of an earlier incarnation (§3.6): view-only.
pub(crate) const VIEW_ONLY: &str = "an earlier block · view only";

/// A restore that waited, its walk or session gone since.
pub(crate) const RESTORE_LEFT: &str = "the walk moved on · nothing was restored";

/// The attachment changed while a hand-back waited.
pub(crate) const SESSION_CHANGED: &str = "the session changed · nothing was sent";

/// The block moved on while a hand-back waited for its notices' marks:
/// its pre-check says something else now.
pub(crate) const MOVED_ON: &str = "the block changed while its notices were saved · hand back again";

/// The connection didn't take a hand-back.
pub(crate) const STOPPED: &str = "not sent · the connection thread has stopped";

/// An evicted outcome, re-folding (§1.8).
pub(crate) const LOADING: &str = "loading…";

/// An evicted outcome past the re-fold's reach (§1.8's `too far back`).
pub(crate) const TOO_FAR: &str = "too far back to show";

/// What a send is.
#[derive(Debug)]
pub(crate) enum What {
    /// A hand-back of the session's held draft of the incarnation (its
    /// edit read from it again at the wire), or a restore's draft (its
    /// own, boxed: the others are a word).
    New {
        restore: Option<Box<Draft>>,
    },
    Retry,
    Resend,
}

/// An edit on its way to the wire (its notices' marks first, maybe): its
/// attachment (`lid`'s `seq`: the session too), its block row and
/// incarnation, and its body, what the memo matches. The session comes
/// from the gate at the wire; the edit from the draft. ≤
/// [`WAIT_MAX_BYTES`] each, ≤ `desires::QUEUE_MAX` of them.
#[derive(Debug)]
pub(crate) struct EditSend {
    lid: LiveId,
    seq: u64,
    row: u64,
    tag: Tag,
    body: String,
    what: What,
}

impl EditSend {
    /// Its store.
    pub(crate) fn lid(&self) -> LiveId {
        self.lid
    }

    /// Its body, and a restore's draft (tests).
    #[cfg(test)]
    pub(crate) fn parts(&self) -> (&str, Option<&Draft>) {
        match &self.what {
            What::New { restore: Some(d) } => (&self.body, Some(d)),
            _ => (&self.body, None),
        }
    }

    /// What it owns on the heap: its body, a restore's draft.
    fn heap(&self) -> usize {
        self.body.capacity()
            + match &self.what {
                What::New { restore: Some(d) } => std::mem::size_of::<Draft>() + d.bytes(),
                _ => 0,
            }
    }
}

/// One wait at most (bytes): its slot in the map, a body at B4's ceiling,
/// a restore's draft at its own.
pub(crate) const WAIT_MAX_BYTES: usize = std::mem::size_of::<(u64, EditSend)>()
    + 1
    + bm::ENTRY_MAX_BYTES
    + std::mem::size_of::<Draft>()
    + DRAFT_MAX_BYTES
    + cards_spec::board::BLOCK_MAX_OWNED
    + 2 * std::mem::size_of::<usize>();

/// A `changed your edit` seen: the session, the incarnation, the path, and
/// the block's revision then (module docs).
#[derive(Debug)]
pub(crate) struct Seen {
    session: Box<str>,
    tag: Tag,
    path: Box<str>,
    rev: u32,
}

/// A restore pressed while no board was bound (`R`, §3.6): what it was
/// pressed on, which it restores or nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreWanted {
    pub(crate) pane: crate::pane::PaneId,
    pub(crate) session: String,
    pub(crate) row: u64,
    /// The past revision in view: its record's place, and its incarnation.
    pub(crate) at: Pos,
    pub(crate) tag: Tag,
}

/// A re-folded outcome (module docs): of store `live`'s history `epoch`,
/// row `row`, at input revision `rev`.
#[derive(Debug)]
pub(crate) struct KeptOutcome {
    live: LiveId,
    epoch: u64,
    row: u64,
    rev: u64,
    /// What it did, or too far back (`None`).
    said: Option<(EditResult, Box<[Code]>)>,
}

/// One kept outcome at most (bytes).
pub(crate) const KEPT_MAX_BYTES: usize = std::mem::size_of::<KeptOutcome>() + OUTCOME_MAX_OWNED;

/// Whether entry `e` says something on its block (module docs): not in
/// the chat at rest, not covered by this desk's own later one.
fn says(e: &bm::Entry) -> bool {
    !((e.state == State::InChat && e.retired.is_none()) || e.retired == Some(bm::Retired::Here))
}

/// Entry `body`'s key in a selector: its older lines' buttons carry it.
pub(crate) fn entry_key(body: &str) -> u64 {
    crate::blocks::hash_of(body) & 0xffff_ffff
}

impl AppView {
    /// Store `lid`'s board keys are checked against: the newest, drawn or
    /// waiting under the lease ([`AppView::bound_board`]'s, any store).
    fn bound_of(&self, lid: LiveId) -> Option<Arc<Drawn>> {
        let live = &self.model.lives[lid];
        live.board.waiting.clone().or_else(|| live.transcript.drawn().cloned())
    }

    /// Whether a hand-back of incarnation `tag` on store `lid` waits for
    /// its notices' marks (one edit of an incarnation at a time).
    pub(crate) fn edit_waits_on(&self, lid: LiveId, tag: Tag) -> bool {
        self.edit_waits.values().any(|w| w.lid == lid && w.tag == tag)
    }

    /// Enter on the held block the pane edits, or its Hand back: `Err`,
    /// why nothing went.
    pub(crate) fn edit_hand_back(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(e) = self.panes.cur.edit.at.as_ref() else { return Ok(()) };
        let (session, id, tag) = (e.session.clone(), e.id.clone(), e.tag);
        let lid = self.panes.cur.live;
        let (seq, _) = self.model.card_gate(lid)?;
        let d = self.block_drafts.get(&session, &id, tag).ok_or(crate::lease::CHANGED)?;
        if d.sent.is_some() {
            return Err(SENDING.into());
        }
        if let Some(f) = d.ended {
            return Err(f.says().unwrap_or_default());
        }
        let st = self.bound_board().and_then(|b| b.board.clone()).ok_or("checking…")?;
        let (edit, l1) = d.handing(&st)?;
        let body = self.admit(lid, &session, &edit, &l1)?;
        self.send_edit(&session, EditSend { lid, seq, row: crate::blocks::row_id(&id), tag, body, what: What::New { restore: None } }, cx)
    }

    /// The memo's admission of `edit` on store `lid`'s `session` now, and
    /// its body.
    fn admit(&self, lid: LiveId, session: &str, edit: &Edit, l1: &Line1) -> Result<String, String> {
        if self.edit_waits_on(lid, edit.of) {
            return Err(SENDING.into());
        }
        let t = &self.model.lives[lid].transcript;
        let drawn = t.drawn().cloned();
        self.model.memos.edits.may_hand_back(session, edit, l1, t, drawn.as_deref())?;
        message::build(edit, &l1.summary()).map_err(|e| e.to_string())
    }

    /// `s`, checked, its notices riding (now, or once their marks are
    /// written).
    fn send_edit(&mut self, session: &str, s: EditSend, cx: &mut Context<Self>) -> Result<(), String> {
        if self.edit_waits_on(s.lid, s.tag) {
            return Err(SENDING.into());
        }
        let t = &self.model.lives[s.lid].transcript;
        let drawn = t.drawn().cloned();
        let d = drawn.as_deref();
        let memo = &self.model.memos.edits;
        match &s.what {
            What::Retry if !memo.may_retry(session, &s.body, t, d) => return Err(crate::lease::CHANGED.into()),
            What::Resend if !memo.may_resend(session, &s.body, t, d) => return Err(crate::lease::CHANGED.into()),
            _ => {}
        }
        let rode = match self.wish_board(s.lid) {
            None => Riding::Go(Ride { text: s.body.clone(), carrier: None, notices: Vec::new() }),
            Some((conv, g, shared)) => self.wishes.desires.ride(&conv, &g, &shared, &s.body, Carry::HandBack).map_err(|r| r.0)?,
        };
        match rode {
            Riding::Go(ride) => self.edit_go(s, ride, cx),
            Riding::Wait(ticket) => {
                // Its draft owns it until the memo does (module docs).
                if let What::New { restore: None } = &s.what
                    && let Some(id) = self.block_drafts.of_tag(session, s.tag).map(|d| d.id.clone())
                {
                    self.block_drafts.wait(session, &id, s.tag, s.body.clone());
                }
                debug_assert!(s.heap() + std::mem::size_of::<(u64, EditSend)>() < WAIT_MAX_BYTES, "a wait within its ceiling");
                self.edit_waits.insert(ticket, s);
                log::info!(
                    target: "synaps_desktop::handback",
                    "a hand-back waits for its notices' marks; {} wait, {} bytes",
                    self.edit_waits.len(),
                    self.edit_waits_bytes()
                );
                self.pump_wishes(cx);
                Ok(())
            }
        }
    }

    /// The notices' marks landed for the edit waiting on `ticket`: it goes
    /// now, `ride` its message, if the wire's checks hold. `false`: not an
    /// edit's ticket.
    pub(crate) fn edit_landed(&mut self, ticket: u64, ride: Ride, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.edit_waits.remove(&ticket) else { return false };
        let (row, lid) = (s.row, s.lid);
        if let Err(why) = self.edit_go(s, ride, cx) {
            let panes: Vec<crate::pane::PaneId> = self.panes.iter().filter(|p| p.live == lid).map(|p| p.id).collect();
            for p in panes {
                self.in_pane(p, |this| this.edit_says(row, why.clone(), cx));
            }
        }
        true
    }

    /// `s` to the wire as `ride` says (its body, the notices then), if the
    /// wire's checks hold (module docs): sent, or nothing moved (its
    /// notices back in the queue, a draft held again, an entry as it was).
    fn edit_go(&mut self, s: EditSend, ride: Ride, cx: &mut Context<Self>) -> Result<(), String> {
        let lid = s.lid;
        match self.edit_wire(&s, &ride.text) {
            Ok((out, session, held)) => {
                log::info!(
                    target: "synaps_desktop::handback",
                    "hand-back on row {}: {} ({} bytes); the desk keeps {} hand-back memos, {} bytes",
                    s.row,
                    if out.busy { "steer" } else { "submit" },
                    out.text.len(),
                    self.model.memos.edits.len(),
                    self.model.memos.edits.bytes()
                );
                self.model.lives[lid].edit_sent(out.text.clone());
                self.ride_went(lid, &out.text, ride.carrier, true);
                if let Some(id) = held {
                    self.block_drafts.sent(&session, &id, s.tag, Some(s.body.clone()));
                }
                #[cfg(test)]
                self.edit_sends.push(out);
                self.after_card(s.row, cx);
                Ok(())
            }
            Err(why) => {
                log::info!(target: "synaps_desktop::handback", "hand-back on row {}: not sent: {why}", s.row);
                self.ride_went(lid, &ride.text, ride.carrier, false);
                if let What::New { restore: None } = &s.what {
                    self.block_drafts.unwait(s.tag, &s.body);
                }
                self.after_card(s.row, cx);
                Err(why)
            }
        }
    }

    /// The wire's checks and the send (module docs): what went, its
    /// session, and the held draft's id (a new edit's, not a restore's).
    fn edit_wire(&mut self, s: &EditSend, text: &str) -> Result<(EditOut, String, Option<Ref>), String> {
        let (seq, session) = self.model.card_gate(s.lid)?;
        if seq != s.seq {
            return Err(SESSION_CHANGED.into());
        }
        let trailer = message::split(text).1;
        let now = self.model.memos.edits.clock(Instant::now());
        let bound = self.bound_of(s.lid);
        let l = &self.model.lives[s.lid];
        let t = &l.transcript;
        let drawn = t.drawn().cloned();
        let d = drawn.as_deref();
        let memo = &self.model.memos.edits;
        let mut held = None;
        let staged = match &s.what {
            What::New { restore } => {
                let draft = match restore {
                    Some(r) => &**r,
                    None => {
                        let d = self.block_drafts.of_tag(&session, s.tag).ok_or(crate::lease::CHANGED)?;
                        if d.on_wire() || d.sent.as_deref().is_some_and(|b| b != s.body) {
                            return Err(SENDING.into());
                        }
                        held = Some(d.id.clone());
                        d
                    }
                };
                if let Some(f) = draft.ended {
                    return Err(f.says().unwrap_or_default());
                }
                let st = bound.and_then(|b| b.board.clone()).ok_or("checking…")?;
                let (edit, l1) = draft.handing(&st)?;
                if message::build(&edit, &l1.summary()).map_err(|e| e.to_string())? != s.body {
                    return Err(MOVED_ON.into());
                }
                Staging::Send(memo.stage_hand_back(&session, &edit, &l1, &trailer, t, d, now)?)
            }
            What::Retry => Staging::Send(memo.stage_retry(&session, &s.body, &trailer, t, d, now).ok_or(crate::lease::CHANGED)?),
            What::Resend => memo.stage_resend(&session, &s.body, &trailer, t, d, now),
        };
        let staged = match staged {
            Staging::Send(st) => st,
            Staging::Checking(st) => {
                self.model.memos.edits.commit(st);
                return Err(crate::deliver::UNSETTLED.into());
            }
            Staging::No => return Err(crate::lease::CHANGED.into()),
        };
        let busy = self.model.lives[s.lid].transcript.busy();
        let out = EditOut { seq, busy, text: staged.text().to_string(), body: s.body.clone() };
        let sent = match &self.client {
            Some(c) if busy => c.on(seq).steer(out.text.clone()).is_ok(),
            Some(c) => c.on(seq).submit(out.text.clone()).is_ok(),
            None => false,
        };
        if !sent {
            // Nothing left: the memo stays as it was (module docs).
            return Err(STOPPED.into());
        }
        self.model.memos.edits.commit(staged);
        Ok((out, session, held))
    }

    /// Retry, Resend or Discard (`what` none) of this desk's hand-back
    /// `body` of block `id`'s incarnation `tag`, in the pane acted on: that
    /// entry's, whichever is newest.
    pub(crate) fn edit_again(&mut self, id: &Ref, tag: Tag, body: &str, what: Option<What>, cx: &mut Context<Self>) {
        let lid = self.panes.cur.live;
        let row = crate::blocks::row_id(id);
        let Some(session) = self.model.lives[lid].active.as_ref().map(|a| a.session_id.clone()) else { return };
        if self.model.memos.edits.get(&session, body).is_none() {
            return;
        }
        let Some(what) = what else {
            self.model.memos.edits.forget(&session, body);
            log::info!(target: "synaps_desktop::handback", "hand-back of {id}: discarded");
            self.after_card(row, cx);
            return;
        };
        let r = self
            .model
            .card_gate(lid)
            .and_then(|(seq, session)| self.send_edit(&session, EditSend { lid, seq, row, tag, body: body.to_string(), what }, cx));
        if let Err(why) = r {
            self.panes.cur.edit.note = Some((row, why));
        }
        self.after_card(row, cx);
    }

    /// `R` on the walked block's past revision (§3.6): a restore of the
    /// current incarnation to it, sent as an edit. `want`: the revision
    /// the press was on, if it waited; the walk must still show it.
    /// `Err`: why not.
    pub(crate) fn restore(&mut self, row: u64, want: Option<&RestoreWanted>, cx: &mut Context<Self>) -> Result<(), String> {
        let lid = self.panes.cur.live;
        let shown =
            self.panes.cur.walk.walk.as_ref().filter(|w| w.at.row() == row).and_then(|w| w.rev.as_ref()).map(|v| (v.rev.at, v.rev.tag));
        if let Some(w) = want
            && shown != Some((w.at, w.tag))
        {
            return Err(RESTORE_LEFT.into());
        }
        let past = self.walk_past(row).ok_or("walk to a past revision first ([)")?;
        let then = past.ready().cloned().ok_or("loading…")?;
        let (seq, session) = self.model.card_gate(lid)?;
        if want.is_some_and(|w| w.session != session) {
            return Err(RESTORE_LEFT.into());
        }
        let d = self.bound_board().ok_or("checking…")?;
        let st = d.board.clone().ok_or("checking…")?;
        let now = st.block(&then.block.id).cloned().ok_or(VIEW_ONLY)?;
        if now.tag != then.block.tag {
            return Err(VIEW_ONLY.into());
        }
        if self.model.memos.edits.in_flight(&session, &now.id, now.tag).is_some() || self.edit_waits_on(lid, now.tag) {
            return Err(SENDING.into());
        }
        let generation = d.generation.ok_or("checking…")?;
        let draft = Draft::restore(Arc::new(now.clone()), &then.block, generation).ok_or(crate::draft_view::NOT_EDITABLE)?;
        if !draft.changed() {
            return Err(NOTHING_TO_RESTORE.replace("{}", &then.block.rev.to_string()));
        }
        if draft.own_bytes() > DRAFT_MAX_BYTES {
            return Err(DRAFT_FULL.into());
        }
        let (edit, l1) = draft.handing(&st)?;
        let body = self.admit(lid, &session, &edit, &l1)?;
        let s = EditSend { lid, seq, row, tag: now.tag, body, what: What::New { restore: Some(Box::new(draft)) } };
        self.send_edit(&session, s, cx)
    }

    /// Each session's handed-back drafts on the wire that are done with go
    /// (module docs): their edit out of flight, and a board that has read
    /// the history it is in drawn (its row showed at a revision the drawn
    /// board is at or past), whatever outcomes that board still keeps.
    pub(crate) fn settle_sent_drafts(&mut self) {
        for live in self.model.lives.ids() {
            let l = &self.model.lives[live];
            let Some(session) = l.active.as_ref().map(|a| a.session_id.clone()) else { continue };
            if !self.block_drafts.of(&session).any(Draft::on_wire) {
                continue;
            }
            let memo = &self.model.memos.edits;
            let t = &l.transcript;
            let (rev, epoch) = (t.blocks_rev(), t.blocks_epoch());
            let read = t.drawn().filter(|d| d.epoch == epoch).map(|d| d.rev);
            let mut done = |d: &mut Draft| {
                let body = d.sent.as_deref().unwrap_or_default();
                match memo.get(&session, body) {
                    None => true,
                    Some(e) if e.in_flight() => false,
                    Some(e) if e.state == State::InChat && e.retired.is_none() => {
                        if d.shown_at.is_none() && crate::handback::shows(t, body) {
                            d.shown_at = Some(rev);
                        }
                        d.shown_at.is_some_and(|at| read.is_some_and(|r| r >= at))
                    }
                    Some(_) => true,
                }
            };
            let rows = self.block_drafts.settle_sent(&session, &mut done);
            for r in rows {
                self.remeasure_draft(r);
                self.remeasure_block(r);
            }
        }
    }

    /// Block row `row` measured again in every pane that draws it.
    pub(crate) fn remeasure_block(&mut self, row: u64) {
        for pane in self.panes.iter_mut() {
            if let Some(ix) = pane.disp.iter().position(|d| d.id == row && matches!(d.kind, crate::turns::Kind::Block { .. })) {
                pane.list.remeasure_items(ix..ix + 1);
            }
        }
    }

    /// The draft's header while it is sent (module docs): its delivery
    /// word.
    pub(crate) fn sent_head(&self, d: &Draft) -> Option<String> {
        let body = d.sent.as_deref()?;
        let session = self.model.lives[self.panes.cur.live].active.as_ref()?.session_id.as_str();
        let word = match self.model.memos.edits.get(session, body).map(|e| e.state) {
            _ if d.waiting => "WAITING",
            Some(State::Queued) => "QUEUED",
            Some(State::Held) => "HELD",
            Some(State::Checking) => "CHECKING",
            Some(State::InChat) => "IN THE CHAT",
            _ => "SENDING",
        };
        Some(format!("{word} · r{}", d.base))
    }

    /// The words under a sent draft: its delivery's, as the memo says.
    pub(crate) fn sent_words(&self, d: &Draft) -> Option<String> {
        let body = d.sent.as_deref()?;
        let session = self.model.lives[self.panes.cur.live].active.as_ref()?.session_id.as_str();
        Some(match self.model.memos.edits.get(session, body) {
            Some(e) if !d.waiting => crate::handback::words(e).0.to_string(),
            _ => "waiting for the rail's notices to be saved".into(),
        })
    }

    /// Whether the memo admits the hand-back of the draft the pane edits
    /// now (the button's `on`), or why not.
    pub(crate) fn may_hand_back(&self, d: &Draft) -> Result<(), String> {
        let lid = self.panes.cur.live;
        let l = &self.model.lives[lid];
        self.model.card_gate(lid)?;
        let session = l.active.as_ref().map(|a| a.session_id.as_str()).ok_or("not attached")?;
        let st = self.bound_board().and_then(|b| b.board.clone()).ok_or("checking…")?;
        let (edit, l1) = d.handing(&st)?;
        self.admit(lid, session, &edit, &l1).map(drop)
    }

    /// Block `s`'s delivery lines (row `id`): every hand-back of its
    /// incarnation this desk keeps while it says something, oldest first,
    /// each with its own buttons (module docs). The newest one's buttons
    /// are the block's (`edit-button {row} Retry`); an older one's carry its
    /// key ([`entry_key`]).
    pub(crate) fn edit_delivery(&self, s: &Shown, id: u64, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lid = self.panes.cur.live;
        let l = &self.model.lives[lid];
        let session = l.active.as_ref()?.session_id.clone();
        let all: Vec<&bm::Entry> = self.model.memos.edits.of_incarnation(&session, &s.block.id, s.block.tag).collect();
        let newest = all.last().map(|e| e.text.clone());
        let lines: Vec<&bm::Entry> = all.into_iter().filter(|e| says(e)).collect();
        if lines.is_empty() {
            return None;
        }
        let drawn = l.transcript.drawn().cloned();
        let gate = self.model.card_gate(lid).is_ok() && !self.edit_waits_on(lid, s.block.tag);
        let pane = self.panes.cur.id;
        let mut col = v_flex().w_full().gap(px(2.)).pb(px((6. * m.r).round()));
        for e in lines {
            let (word, cause) = crate::handback::words(e);
            let text = match cause {
                Some(c) => format!("{word} · {c}"),
                None => word.to_string(),
            };
            let retry = gate && self.model.memos.edits.may_retry(&session, &e.text, &l.transcript, drawn.as_deref());
            let resend = gate && self.model.memos.edits.may_resend(&session, &e.text, &l.transcript, drawn.as_deref());
            let discard = !e.in_flight();
            let key = (newest.as_deref() != Some(e.text.as_str())).then(|| entry_key(&e.text));
            let (bid, tag, body) = (s.block.id.clone(), s.block.tag, e.text.clone());
            let button = |label: &'static str, what: fn() -> Option<What>| {
                let (bid, body) = (bid.clone(), body.clone());
                let sel = match key {
                    Some(k) => format!("edit-button {id} {label} {k}"),
                    None => format!("edit-button {id} {label}"),
                };
                div()
                    .id((label, id ^ key.unwrap_or(0)))
                    .flex_none()
                    .px(px(8.))
                    .cursor_pointer()
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .text_color(p.prose_t)
                    .bg(p.well_d)
                    .debug_selector(move || sel.clone())
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.in_pane(pane, |this| this.edit_again(&bid, tag, &body, what(), cx));
                    }))
            };
            let tt = text.clone();
            col = col.child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .items_center()
                    .whitespace_nowrap()
                    .debug_selector(move || format!("edit-delivery {id} {tt}"))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_family(fonts::SANS)
                            .text_px(m.meta)
                            .text_color(if e.in_flight() { p.dim_t } else { p.warn })
                            .child(text),
                    )
                    .when(retry, |d| d.child(button("Retry", || Some(What::Retry))))
                    .when(resend, |d| d.child(button("Resend", || Some(What::Resend))))
                    .when(discard, |d| d.child(button("Discard", || None))),
            );
        }
        Some(col.into_any_element())
    }

    /// The pane's session's hand-backs whose incarnation isn't live on its
    /// board (replaced, removed, compacted away) while they say something
    /// (module docs): what the drafts dock lists, each with Copy (its line
    /// 1's words) and Discard, never a Resend (§3.7: it would apply
    /// nothing). Their body and words.
    pub(crate) fn ended_hand_backs(&self) -> Vec<(String, String)> {
        let lid = self.panes.cur.live;
        let l = &self.model.lives[lid];
        let Some(session) = l.active.as_ref().map(|a| a.session_id.as_str()) else { return Vec::new() };
        let drawn = l.transcript.drawn().cloned();
        let edits = self.model.memos.edits.edits(session, &l.transcript, drawn.as_deref());
        self.model
            .memos
            .edits
            .entries(session)
            .filter(|e| says(e) && !bm::Edits::live(&edits, &e.id, &e.of))
            .map(|e| {
                let (word, cause) = crate::handback::words(e);
                let w = match cause {
                    Some(c) => format!("{word} · {c}"),
                    None => word.to_string(),
                };
                (e.text.clone(), format!("hand-back of {} r{} · {w}", e.id, e.base))
            })
            .collect()
    }

    /// The dock's Copy on hand-back `body`: its line 1's words into the
    /// composer, its entry discarded.
    pub(crate) fn hand_back_copy(&mut self, body: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return };
        let line1 = body.split('\n').next().unwrap_or_default();
        let words = line1.split_once("] ").map_or(line1, |(_, w)| w).to_string();
        let c = self.panes.cur.composer.clone();
        c.update(cx, |s, cx| {
            let had = s.value().to_string();
            let text = if had.trim().is_empty() { words.clone() } else { format!("{}\n{words}", had.trim_end()) };
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        self.model.memos.edits.forget(&session, body);
        log::info!(target: "synaps_desktop::handback", "an ended block's hand-back: copied to the composer");
        cx.notify();
    }

    /// The dock's Discard on hand-back `body`.
    pub(crate) fn hand_back_discard(&mut self, body: &str, cx: &mut Context<Self>) {
        let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return };
        self.model.memos.edits.forget(&session, body);
        log::info!(target: "synaps_desktop::handback", "an ended block's hand-back: discarded");
        cx.notify();
    }

    /// The outcome of hand-back row `row` of store `live` (module docs):
    /// the board's, or a re-fold's kept; `Err` why there is none yet.
    fn outcome_of(&self, live: LiveId, row: u64) -> Result<Option<(EditResult, Vec<Code>)>, &'static str> {
        let l = &self.model.lives[live];
        let t = &l.transcript;
        let Some(d) = t.drawn() else { return Ok(None) };
        if let Some(o) = d.outcomes.iter().find(|o| o.row == Some(row)) {
            return Ok(Some((o.outcome.result, o.outcome.codes.clone())));
        }
        let epoch = t.blocks_epoch();
        if let Some(k) = self.outcomes_kept.iter().rev().find(|k| k.live == live && k.epoch == epoch && k.row == row) {
            return match &k.said {
                Some((r, codes)) => Ok(Some((*r, codes.to_vec()))),
                None => Err(TOO_FAR),
            };
        }
        // Its place in the history, from what the board still marks: an
        // outcome it let go of is re-folded from there.
        match evicted_at(d, row) {
            Some(at) => {
                if d.epoch == epoch {
                    self.outcome_wanted.set(self.outcome_wanted.get().or(Some((live, row, at))));
                }
                Err(LOADING)
            }
            // Nothing marks it: a board that has read the whole history
            // let it go past every mark it keeps.
            None if d.epoch == epoch && d.rev == t.blocks_rev() && t.in_history(row) => Err(TOO_FAR),
            None => Ok(None),
        }
    }

    /// What hand-back row `item` (its user message) says the board did,
    /// in the pane acted on.
    pub(crate) fn hand_back_said(&self, item: &Item) -> Option<String> {
        let edit = message::parse(message::split(&item.text).0);
        let live = self.panes.cur.live;
        // Carried into a compaction's successor, its block not there
        // (§5.2): the history says so, on any desk that reads it.
        let carried = self.model.lives[live].transcript.carried(item.id);
        match self.outcome_of(live, item.id) {
            Ok(Some((r, codes))) => Some(crate::handback::words_of(r, &codes, edit.as_ref(), carried)),
            Ok(None) if carried => Some(crate::handback::COMPACTED_AWAY.into()),
            Ok(None) => None,
            Err(why) => Some(why.to_string()),
        }
    }

    /// A hand-back's row (module docs): line 1's words, then what the
    /// board says it did. `item` is its user message.
    pub(crate) fn render_hand_back(&self, item: &Item, id: u64, p: &Palette, m: &Metrics) -> AnyElement {
        let body = message::split(&item.text).0;
        let line1 = body.split('\n').next().unwrap_or_default();
        let (head, words) = match line1.split_once("] ") {
            Some((h, w)) => (h.trim_start_matches("[block "), w),
            None => ("", line1),
        };
        let words = crate::ask::shown(words, cards_spec::Lines::Single);
        let said = self.hand_back_said(item);
        let (h, w, s) = (head.to_string(), words.to_string(), said.clone().unwrap_or_default());
        v_flex()
            .relative()
            .w_full()
            .bg(p.user)
            .px(px(m.pad_x))
            .py(px(m.pad_y))
            .font_family(fonts::SANS)
            .text_px(m.body)
            .line_height(px(m.body_line))
            .whitespace_nowrap()
            .debug_selector(move || format!("hand-back {id} {h} · {w} · {s}"))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .child(div().flex_none().text_color(p.dim_u).child(crate::glyph::draw(crate::glyph::Glyph::Elbow, p.dim_u)))
                    .child(div().flex_none().font_family(fonts::MONO).text_px(m.meta).text_color(p.dim_u).child(head.to_string()))
                    .child(div().min_w_0().flex_1().overflow_hidden().text_ellipsis().text_color(p.ink).child(words)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(m.body_line))
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.dim_u)
                    .debug_selector(move || format!("hand-back-outcome {id}"))
                    .children(said),
            )
            .child(crate::glyph::bar_at(p.you_mark, 1.))
            .into_any_element()
    }

    /// As a pane's frame begins: an evicted outcome a row drew asks for its
    /// re-fold while the one scratch is free (no walk's revision wants it,
    /// none runs); one that finished is kept, and the scratch let go.
    pub(crate) fn sync_outcomes(&mut self, cx: &mut Context<Self>) {
        for live in self.model.lives.ids() {
            self.prune_outcomes(live);
        }
        if let Some((live, row, at, epoch)) = self.outcome_asked {
            let target = Target::Outcome(at);
            match self.refolds.done.as_ref().filter(|d| d.live == live && d.target == target) {
                Some(done) => {
                    let said = match &done.state {
                        Ok(Some(st)) => st.outcomes().find(|o| o.at == at).map(|o| (o.result, o.codes.clone().into_boxed_slice())),
                        _ => None,
                    };
                    let rev = self.model.lives.get(live).map_or(0, |l| l.transcript.blocks_rev());
                    debug_assert!(said.as_ref().is_none_or(|(_, c)| std::mem::size_of_val(&**c) <= KEPT_MAX_BYTES), "within B2's ceiling");
                    if self.outcomes_kept.len() >= OUTCOMES_KEPT {
                        self.outcomes_kept.remove(0);
                    }
                    self.outcomes_kept.push(KeptOutcome { live, epoch, row, rev, said });
                    log::debug!(
                        target: "synaps_desktop::handback",
                        "an evicted outcome re-folded; {} kept, {} bytes",
                        self.outcomes_kept.len(),
                        self.outcomes_kept_bytes()
                    );
                    self.outcome_asked = None;
                    self.refold_cancel();
                    cx.notify();
                }
                // Another took the scratch (a walk's revision): asked again
                // once it is free.
                None if !self.refolds.wants(live, target) => self.outcome_asked = None,
                None => {}
            }
        }
        let Some((live, row, at)) = self.outcome_wanted.take() else { return };
        let walking = self.panes.iter().any(|p| p.walk.refolding || p.walk.walk.as_ref().is_some_and(|w| w.rev.is_some()));
        if self.outcome_asked.is_some() || walking || self.refolds.busy() {
            return;
        }
        let Some(epoch) = self.model.lives.get(live).map(|l| l.transcript.blocks_epoch()) else { return };
        self.outcome_asked = Some((live, row, at, epoch));
        self.refold(live, Target::Outcome(at), cx);
    }

    /// What the re-folded outcomes keep (bytes): each one's slot and codes.
    pub(crate) fn outcomes_kept_bytes(&self) -> usize {
        self.outcomes_kept.capacity() * std::mem::size_of::<KeptOutcome>()
            + self.outcomes_kept.iter().map(|k| k.said.as_ref().map_or(0, |(_, c)| std::mem::size_of_val(&**c))).sum::<usize>()
    }

    /// Store `live`'s re-folded outcomes of another history, or of an input
    /// that moved while no turn runs, go (an agent's later write marks an
    /// outcome overwritten: it is re-folded when it draws again).
    pub(crate) fn prune_outcomes(&mut self, live: LiveId) {
        let Some(l) = self.model.lives.get(live) else {
            self.outcomes_kept.retain(|k| k.live != live);
            return;
        };
        let (epoch, rev, busy) = (l.transcript.blocks_epoch(), l.transcript.blocks_rev(), l.transcript.busy());
        self.outcomes_kept.retain(|k| k.live != live || (k.epoch == epoch && (busy || k.rev == rev)));
    }

    /// The `changed your edit` chip of block `s`, if it shows (module
    /// docs): its path, for the hover.
    pub(crate) fn changed_your_edit(&self, s: &Shown) -> Option<Path> {
        let path = s.block.changed_your_edit.clone()?;
        let l = &self.model.lives[self.panes.cur.live];
        let session = l.active.as_ref()?.session_id.as_str();
        let seen = self.changed_seen.iter().rev().find(|x| &*x.session == session && x.tag == s.block.tag && &*x.path == path.as_str());
        match seen {
            None => Some(path),
            Some(x) => overwritten_again(l.transcript.drawn().map(|d| &**d), s, &path, x.rev).then_some(path),
        }
    }

    /// The user focused block row `row` (the walk's ring on it, or a
    /// take): its `changed your edit`, seen at its revision now.
    pub(crate) fn saw_changed(&mut self, row: u64) {
        let Some(session) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return };
        let Some((_, s)) = self.block_drawn(row) else { return };
        let Some(path) = self.changed_your_edit(&s) else { return };
        self.see(&session, s.block.tag, path.as_str(), s.block.rev);
        log::debug!(target: "synaps_desktop::handback", "changed your edit on {}: seen; {} kept, {} bytes", s.block.id, self.changed_seen.len(), self.changed_seen_bytes());
        self.remeasure_block(row);
    }

    /// The overwrite of `path` on `session`'s incarnation `tag`, seen at
    /// revision `rev`: kept in place of the one before, the oldest going
    /// past [`SEEN_MAX`].
    pub(crate) fn see(&mut self, session: &str, tag: Tag, path: &str, rev: u32) {
        self.changed_seen.retain(|x| !(*x.session == *session && x.tag == tag && *x.path == *path));
        if self.changed_seen.len() >= SEEN_MAX {
            self.changed_seen.remove(0);
        }
        self.changed_seen.push(Seen { session: session.into(), tag, path: path.into(), rev });
    }

    /// What the seen overwrites own on the heap, counted on its own
    /// (tests): each one's session and path.
    #[cfg(test)]
    pub(crate) fn changed_seen_heap(&self) -> usize {
        self.changed_seen.iter().map(|x| std::mem::size_of_val(&*x.session) + std::mem::size_of_val(&*x.path)).sum()
    }

    /// What the hand-backs waiting for their notices' marks hold: each
    /// one's slot in the map, its body, a restore's draft (≤
    /// [`WAIT_MAX_BYTES`] each).
    pub(crate) fn edit_waits_bytes(&self) -> usize {
        self.edit_waits.capacity() * (std::mem::size_of::<(u64, EditSend)>() + 1)
            + self.edit_waits.values().map(EditSend::heap).sum::<usize>()
    }

    /// What the `changed your edit` overwrites seen keep (bytes): each
    /// one's slot, its session and its path.
    pub(crate) fn changed_seen_bytes(&self) -> usize {
        self.changed_seen.capacity() * std::mem::size_of::<Seen>()
            + self.changed_seen.iter().map(|x| x.session.len() + x.path.len()).sum::<usize>()
    }
}

/// Where hand-back row `row`'s message is in board `d`'s input, from the
/// marks it still keeps (its op log's lines, its revision records): what
/// an evicted outcome is re-folded at.
fn evicted_at(d: &Drawn, row: u64) -> Option<Pos> {
    d.log
        .iter()
        .find(|(l, m)| m.row == Some(row) && l.author == Author::User && l.at.op.is_none())
        .map(|(l, _)| l.at)
        .or_else(|| d.records.iter().find(|(r, m)| m.row == Some(row) && r.author == Author::User && r.at.op.is_none()).map(|(r, _)| r.at))
}

/// Whether `path` of block `s`, seen overwritten at revision `at`, was
/// overwritten again since (module docs): it isn't the user's now, and a
/// user revision of the incarnation after `at` may have written it (it
/// names it, or more paths than it names); or the board's records no
/// longer reach back to `at`, and what came between can't be told: it
/// says so rather than stay silent.
fn overwritten_again(d: Option<&Drawn>, s: &Shown, path: &Path, at: u32) -> bool {
    if s.block.user_wrote(path).is_some() || s.block.rev <= at {
        return false;
    }
    let Some(d) = d else { return false };
    let recs = || d.records.iter().map(|(r, _)| r).filter(|r| r.tag == s.block.tag);
    let named = |r: &cards_spec::board::Record| {
        let n = r.paths().count();
        r.paths().any(|p| p == path.as_str()) || usize::from(r.changed) > n
    };
    if recs().any(|r| r.rev > at && r.author == Author::User && named(r)) {
        return true;
    }
    !recs().any(|r| r.rev <= at + 1)
}
