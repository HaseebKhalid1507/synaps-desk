//! Answering agent cards: the delivery adapter (synaps-cards
//! `docs/design.md` v1.2, §3.4 and §3.5; the D2a row of §9.3).
//!
//! An answer is a user message, sent as the composer sends one: a `Steer`
//! while the session is busy, a `Submit` when it is idle, always on the
//! card's own attachment (`client.on(seq)` of the [`crate::live::Live`]
//! that holds its session, never `Client::submit`). What happens to it
//! after that is typed text's fate exactly, including where typed text is
//! lost; the card memo (`cards_spec::memo`, S3's pure state machine)
//! follows it and the card says so. This module feeds the memo:
//! - **events** ([`Memos::on`]): the echoes (`TurnStarted`, `Steered`),
//!   `SteeringDelivered`, a refusal, `Dequeued`, `DriverRevoked`, `Idle`, a
//!   reconnect's `Attached`, and the "steering queue full" notice;
//! - **views** ([`Memos::observe`]): the fold the rows draw, and whether
//!   the history is settled ([`synaps_client::Transcript::history_settled`]),
//!   with the digest's `queued_message`; with, on top, each answer the
//!   engine said it took in that the history doesn't show yet (below);
//! - **the clock** ([`Memos::tick`]): no echo in 10 s, no settled view in
//!   10 s.
//!
//! **One memo per session** (§3.5, "keyed by session"): split panes on a
//! session share it as they share its `Live` (§3.6), and a tab switch
//! leaves it to its session, whose next attach (`Attached`) has it check
//! what it had out.
//!
//! **The binding.** A click names the card as it was painted
//! ([`Painted`]): its call's tool id, its id, its doc and mode. It sends
//! only on a settled history, and only if that history's own fold
//! ([`synaps_client::Transcript::history_cards`]) binds that call so, and
//! the fold the rows draw (the history's, the live turn's on top) still
//! does and may answer it ([`Painted::settled`]): the live rows draw a card
//! before the history has it, and the history wins when it lands (Shady
//! #80), so a click never sends against a binding the history hasn't
//! confirmed (Shady #81 P2-1: a live-only card draws, its controls waiting).
//! A memo entry records its binding too, and a new binding of the id drops
//! it (§3.5).
//!
//! **Taken in.** `SteeringDelivered` is the engine's word that the answer
//! is in its conversation: the entry is in the chat. The daemon clears the
//! slot (a `Conversation`) before the round's history comes, so for a
//! while the settled view is the old history, without it, and the memo
//! would read that as a daemon that lost it (Shady #81 P2-2), offering a
//! Resend that would put it in twice. So each answer taken in is read on
//! top of the view the memo follows, the way the live rows carry a
//! submitted one, until a history that is the daemon's word on it after
//! the delivery settles it:
//! - one that shows it, whenever it comes: in the chat, by the history;
//! - the first settled one after the turn that took it in ended, which
//!   doesn't: the turn ended without it (a failed turn's repair, say).
//!   Not confirmed, its cause the turn's end ([`Memos::cause`]), with
//!   Resend. The end is its `Idle`, which comes once the turn's last
//!   history is saved, its digest sent before it (SynapsCLI `189a5a19`,
//!   `agent-engine/src/session/actor.rs`: the end `:4104-4133`, a failure
//!   `:4136-4146` and `:4160-4166`, a compaction `:4389-4391`,
//!   `announce_idle` `:1261-1275`). Settled after it, the history is at
//!   least the turn's last;
//! - a reattach: the daemon may have restarted without saving it, and its
//!   history says so.
//!
//! Before the turn's end no history says it is gone: the slot's clearing
//! comes with the old one (`actor.rs:3840-3844`), and the round's
//! checkpoint, the first history with it (`runtime/helpers.rs:136-158`,
//! then `runtime/stream.rs:1118-1120`), may be a fetch away. Meanwhile
//! the card says "answered", which is what the engine said: it is in the
//! conversation the agent reads, and acts on this turn. "checking…" would
//! read as unknown for the rest of the turn, and the memo's checking
//! state gives up on a settled view in 10 s, a check that isn't this one.
//!
//! **Changes (D4).** A change replaces the card's entry, and with it this
//! desk's only record of the answer it changes. An answer the engine took
//! in (above) is in its conversation, and its history may land after the
//! change is sent: read as an answer this desk didn't know of, it would
//! drop the change's entry while it is on its way (Shady #94 P2-1). So a
//! change expects its predecessors: each answer to the card the engine
//! took in, since the last attach, that the card's answers don't show yet,
//! whatever number of changes back (A, B, then C expects A and B). Each is
//! counted, not just matched: it is one occurrence of its text, one past
//! those the answers held, with the answers expected before it, when it
//! was sent ([`Expected::nth`]); a further one is another's, and the later
//! answer wins as ever. Nothing else is expected: an answer never said to
//! be taken in (not confirmed, not sent) may never come, and expecting it
//! would read another desk's identical answer as this one's old one. The
//! memo matches an entry's answers to those it saw, in order
//! (`cards_spec::memo`): the expected ones follow the history's there, in
//! the order the engine took them in, and the history's place in them is
//! the count's. An expected answer's turn may end without it, like any
//! taken in (above): its next settled view is the word on it. Then no
//! later change expects it, and an answer of its text in the card is
//! another's, which the entry it was expected in counts as its own: that
//! entry has no Resend or Retry, which would put this desk's answer over
//! it. Nor is it the card's answer any more: once a settled history shows
//! that answer, the entry, at rest (not confirmed, or not sent), is
//! retired ([`Memos::retire`]), and the card shows the history's answer,
//! as for any other desk's. A change from it is then a change, sent once;
//! the answer shown, kept, sends nothing (Shady #94 round 3).
//!
//! **Dropped events.** A desk that falls behind the session's broadcast
//! isn't cut off: the daemon drops what it missed, says so ("event stream
//! lagged; N dropped", SynapsCLI `189a5a19`, `daemon/conn.rs:719-728`) and
//! carries on. The round's checkpoint and the turn's last digest may be
//! among what it dropped, and the history held would then settle an `Idle`
//! that survived as the turn's last word without them: "not confirmed",
//! and a Resend of an answer the daemon kept (Shady #81 round 3). So on
//! the notice the view stops being settled until the history asked for
//! again lands ([`synaps_client::Transcript::resyncing`]; the client asks
//! at once, as after a swap), and the engine's slot is unknown
//! ([`synaps_client::Transcript::slot_known`]): its digest and the echoes
//! that moved it may be among what was dropped, and a steer read as lost
//! on a stale slot, a Resend of what the engine holds, goes in twice
//! (Shady #81 round 4: a turn that failed before taking it in keeps it in
//! the slot, `actor.rs:4136-4146`, and the next turn takes it in with the
//! Resend). The slot is known again only from this attachment's own
//! ordered stream, at its place: a digest after the notice, a `Dequeued`
//! (the daemon took the slot: empty), or the snapshot of an attach. No
//! query answers it, and nothing reads it out of band: an attach on a
//! second connection can't be told from another client's (the daemon
//! hands a joiner the first `Attached` it broadcasts), and its `Detach`
//! could then detach that client, and a late snapshot can undo a
//! `Dequeued` heard since (Shady #81 round 5). Until both are back
//! nothing reconciles (the memo, and what was taken in, wait for a
//! settled view whose slot is known; an answer taken in still reads
//! "answered"), and a click, Retry or Resend sends nothing and says why
//! ([`RESYNCING`]). After it, the first settled view is past the gap and
//! decides as above, an `Idle` heard in the gap included: ours in the
//! slot is queued or held, an empty slot after the turn's end is not
//! confirmed, with Resend. Bounded by the memo's own wait for a settled
//! view ([`CHECK_WAIT`]): past it an answer the gap holds says it couldn't
//! be confirmed and that the agent may still have it ([`MAY_HAVE_IT`]),
//! and so does one queued or held before the gap, whose words say the
//! slot has it and which no clock moves (Shady #81 round 6: a held answer
//! whose `Dequeued` was dropped said "it goes in after the agent's next
//! turn" for good; [`Memos::unchecked`]; the memo's state is kept, for the
//! first settled view to settle); an open card's controls say that the
//! chat hasn't caught up ([`UNCHECKED`]),
//! and "Check again" ([`crate::model::Model::check_again`]) stands where
//! Resend would be: it asks for the history again on this connection,
//! which can settle the history, not the slot. No Resend while the slot
//! is unknown; an answer typed in the composer is the user's to send.
//!
//! Two edges no frame the desk can ask for settles, both the safe way (no
//! Resend, so nothing goes twice):
//! - (a) an `Idle` lost in the gap itself leaves an answer taken in
//!   "answered" until the next turn's end or a reattach (its `Idle` was
//!   the turn's end the history is read against);
//! - (c) a session that goes quiet after a gap, no digest or `Dequeued`
//!   since, leaves the slot unknown until its next turn sends one: an
//!   answer it holds says it couldn't be confirmed, and an open card's
//!   controls wait, until then.
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cards_spec::fold::{Answered, Card, Event as Folded, Fold, NAMES};
use cards_spec::memo::{CHECK_WAIT, Cause, Entry, Event as Heard, Memo, Resend, State, Trigger, View};
use cards_spec::{CardDoc, CardId, Envelope, Reply};
use synaps_client::Transcript;
use synaps_client::cards::Ask;
use synaps_client::protocol::{AgentEvent, SessionEvent, StreamEvent};

/// The broadcast notice of a driver's full queue (SynapsCLI
/// `actor.rs:3292-3314`, "steering queue full (16 messages) — message
/// dropped"): a hint for every entry sending, never a receipt (§3.5).
pub(crate) fn queue_full(notice: &str) -> bool {
    notice.starts_with("steering queue full")
}

/// What the memo hears of session event `event` (`raw`, its wire form), as
/// §3.4 and §3.5 read each, `t` the session's transcript before it: an
/// echo (a driver's: `Steered{delivered: false}` while idle and not
/// compacting), a delivery (`SteeringDelivered`, or the steer a compaction
/// restored into the compacted history), `Dequeued`, `DriverRevoked`,
/// `Idle`, or the full-queue notice.
pub(crate) fn heard<'a>(event: &'a SessionEvent, raw: &'a serde_json::Value, t: &Transcript) -> Option<Heard<'a>> {
    Some(match event {
        SessionEvent::TurnStarted { user_text: Some(text), .. } => Heard::Echo { text, driver: false },
        SessionEvent::Steered { text, delivered } => Heard::Echo { text, driver: !delivered && !t.busy() && !t.compacting() },
        SessionEvent::Stream { event: StreamEvent::Agent(AgentEvent::SteeringDelivered { message }) } => Heard::Delivered { text: message },
        SessionEvent::CompactionApplied { .. } => Heard::Delivered { text: synaps_client::protocol::queued_restored(raw)? },
        SessionEvent::Dequeued { text } => Heard::Dequeued { text },
        SessionEvent::DriverRevoked { undelivered_steering, .. } => Heard::DriverRevoked { undelivered: undelivered_steering },
        SessionEvent::Idle => Heard::Idle,
        SessionEvent::SystemNotice { text } if queue_full(text) => Heard::Notice,
        _ => return None,
    })
}

/// A card as a click saw it: what it names, so the click can tell whether
/// that card is still the one the fold holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Painted {
    pub(crate) tool_id: String,
    pub(crate) id: CardId,
    pub(crate) doc: Arc<CardDoc>,
    pub(crate) wait: bool,
}

impl Painted {
    /// A drawn card's, once it is bound.
    pub(crate) fn of(a: &Ask) -> Option<Self> {
        Some(Painted { tool_id: a.tool_id.clone(), id: a.id.clone()?, doc: a.doc.clone(), wait: a.wait })
    }

    /// The card it names in `fold`, while its binding holds there: the
    /// latest call of its tool id, bound to its id, which names that call
    /// (not a later card bound to it), with the same doc and mode.
    pub(crate) fn find<'a>(&self, fold: &'a Fold) -> Option<&'a Card> {
        let c = fold.card(&self.tool_id)?;
        let same = c.id.as_ref() == Some(&self.id)
            && c.wait == self.wait
            && c.doc.as_ref() == Some(&*self.doc)
            && fold.bound(&self.id).is_some_and(|b| b.tool_id == c.tool_id && b.at == c.at);
        same.then_some(c)
    }

    /// The card it names, if a click may answer it in `t` now: on a
    /// settled history whose own fold binds it so (the live turn's alone
    /// isn't confirmed yet), in the fold the rows draw too (no replacement
    /// or close heard since), and answerable there. Else why not.
    pub(crate) fn settled<'a>(&self, t: &'a Transcript) -> Result<&'a Card, &'static str> {
        if !t.history_settled() {
            return Err(UNSETTLED);
        }
        if !self.confirmed(t) {
            // Bound otherwise in the history, or not in it yet.
            return Err(if t.history_cards().card(&self.tool_id).is_some() { CHANGED } else { UNSETTLED });
        }
        self.find(t.cards()).filter(|c| c.answerable()).ok_or(CHANGED)
    }

    /// The history binds it so: not the live turn's alone.
    pub(crate) fn confirmed(&self, t: &Transcript) -> bool {
        self.find(t.history_cards()).is_some()
    }
}

/// Whether `card`'s answers show `e`'s past those it saw when sent (the
/// memo's rule 2).
fn shows(e: &Entry, card: &Card) -> bool {
    let f: Vec<&str> = card.answers.iter().map(|a| a.text.as_str()).collect();
    let k = f.iter().zip(&e.seen).take_while(|(a, b)| **a == b.as_str()).count();
    f[k..].contains(&e.text.as_str())
}

/// An answer ready to go: on attachment `seq`, a `Steer` if `busy`, else
/// a `Submit` (as the composer's, `DESK:sd/app.rs:4640`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outgoing {
    pub(crate) seq: u64,
    pub(crate) busy: bool,
    pub(crate) card: CardId,
    pub(crate) text: String,
}

/// Why a click on a card sends nothing: the card's line says so.
pub(crate) const UNSETTLED: &str = "Checking the chat… try again in a moment";
/// A card the live turn drew that the history doesn't have yet: its
/// controls wait for it.
pub(crate) const CONFIRMING: &str = "Checking the chat… answer in a moment";
pub(crate) const COMPACTING: &str = "Compacting · answer when it's done";
pub(crate) const CHANGED: &str = "This card changed: the chat no longer asks it so";
pub(crate) const OUT: &str = "Its answer is on its way";
/// The daemon dropped events and the history asked for again isn't in
/// (the module's "Dropped events"): a click waits for it.
pub(crate) const RESYNCING: &str = "Checking the chat… some updates were dropped";
/// And it hasn't come in [`CHECK_WAIT`]: an open card's controls.
pub(crate) const UNCHECKED: &str = "Couldn't check the chat: some updates were dropped, and it hasn't caught up";
/// An answer the gap holds, past [`CHECK_WAIT`]: nothing on this
/// connection has said whether the engine holds it, so no Resend.
pub(crate) const MAY_HAVE_IT: &str =
    "Couldn't confirm: some updates were dropped. The agent may still have it; if it doesn't answer, reply again in the chat";

/// The daemon dropped events for `t`'s attachment, and the history asked
/// for again isn't in or no event since has said the engine's slot (the
/// module's "Dropped events"): nothing is checked on what is held.
pub(crate) fn recovering(t: &Transcript) -> bool {
    t.resyncing() || !t.slot_known()
}

/// A view the memo may settle on (§3.5's "settled"): the history is the
/// latest digest's, and the slot is known. One read on a stale slot sends
/// twice a steer the engine holds (Shady #81 round 4).
fn settled(t: &Transcript) -> bool {
    t.history_settled() && t.slot_known()
}

/// A session whose events the daemon dropped, until the history asked for
/// again lands ([`synaps_client::Transcript::resyncing`]): since when (the
/// first view that showed it), whether that is [`CHECK_WAIT`] ago, and
/// whether it ever was (`waited`: what the cards said then stands while
/// "Check again" checks).
#[derive(Clone, Copy, Debug)]
struct Gap {
    since: Instant,
    over: bool,
    waited: bool,
}

/// An answer the engine took in, until a history settles it (the
/// module's "Taken in"): its entry's card, and whether the turn that took
/// it in has ended since (an `Idle`).
#[derive(Debug)]
struct Taken {
    card: CardId,
    ended: bool,
}

/// An answer to a card that an entry for it, a change, replaced, which
/// the engine took in and the card's answers didn't show yet when the
/// change was sent: the change expects it (the module's "Changes").
#[derive(Clone, Debug)]
struct Expected {
    /// Its message, exactly.
    text: String,
    reply: Reply,
    /// It is this occurrence of `text` in the card's answers (from 1): one
    /// past those its entry saw when it was sent, the answers then and
    /// those it expected. Fewer: still to come; more: another's.
    nth: usize,
    /// An `Idle` or an attach came since it was taken in: the next settled
    /// view is the daemon's word on it.
    ended: bool,
    /// That view didn't show it: the turn's end lost it.
    missed: bool,
}

/// What the entry for `card` whose message is `text` expects.
#[derive(Debug)]
struct Chain {
    card: CardId,
    text: String,
    expects: Vec<Expected>,
}

/// The window's card memos, by session id.
#[derive(Debug, Default)]
pub(crate) struct Memos {
    by: HashMap<String, Memo>,
    /// By session, the entries whose answer the engine took in
    /// (`SteeringDelivered`, or a compaction's restored steer) since the
    /// last attach, until a history settles it (the module's "Taken in").
    taken: HashMap<String, Vec<Taken>>,
    /// By session, the entries whose answer was taken in and the turn's
    /// end didn't save: unconfirmed, they say so ([`Memos::cause`]).
    missed: HashMap<String, Vec<CardId>>,
    /// By session, events the daemon dropped and the history asked for
    /// again not in yet (the module's "Dropped events").
    gaps: HashMap<String, Gap>,
    /// By session, the answers each change expects, until its entry goes
    /// (the module's "Changes").
    chains: HashMap<String, Vec<Chain>>,
    /// Bumped whenever an entry changes, or what a session's cards say of
    /// a gap: cards drawn from it are measured again.
    changes: u64,
    /// Entries that reached the chat (moved to "in the chat": answered, or
    /// taken in) since the desk last took them, for its memory of card
    /// choices ([`crate::recall`]); at most [`REACHED_CAP`].
    reached: Vec<Reached>,
}

/// Answers kept for [`Memos::take_reached`] at most, the oldest dropped.
const REACHED_CAP: usize = 64;

/// The tool id of the close [`Memos::retire`] puts in a copy of a fold:
/// no model's (theirs are `toolu_…` and the like, with no space).
const RETIRE: &str = "synaps-desktop retires";

/// An answer that reached the chat ([`Memos::take_reached`]): its session,
/// card, text and values.
#[derive(Clone, Debug)]
pub(crate) struct Reached {
    pub(crate) session: String,
    pub(crate) card: CardId,
    pub(crate) text: String,
    pub(crate) reply: Reply,
}

impl Memos {
    /// How many times an entry changed: cards drawn from one are measured
    /// again when it moves.
    pub(crate) fn changes(&self) -> u64 {
        self.changes
    }

    /// `session`'s entry for `card`, while it answers it (the same binding).
    pub(crate) fn entry(&self, session: &str, card: &Card) -> Option<&Entry> {
        let id = card.id.as_ref()?;
        self.by.get(session)?.get(id).filter(|e| e.answers(card))
    }

    /// Whether `card`'s answers show `e`, ours, past those it saw when
    /// sent (the memo's rule 2): until they do, the card draws ours (D4:
    /// a change taken in, the history still to come, is not the old
    /// answer).
    pub(crate) fn shown(e: &Entry, card: &Card) -> bool {
        shows(e, card)
    }

    /// Whether any entry waits on the clock (sending: its echo; checking:
    /// a settled view).
    pub(crate) fn timed(&self) -> bool {
        self.by.values().flat_map(Memo::entries).any(|e| matches!(e.state, State::Sending | State::Checking))
            || self.gaps.values().any(|g| !g.over)
    }

    /// Why a card of `session` waits while its view recovers from dropped
    /// events ([`recovering`]): checking, or past [`CHECK_WAIT`],
    /// couldn't check.
    pub(crate) fn gap(&self, session: &str) -> &'static str {
        if self.over(session) { UNCHECKED } else { RESYNCING }
    }

    /// What an answer of `session` the gap holds says on its row: checking,
    /// or past [`CHECK_WAIT`] that it couldn't be confirmed, the agent
    /// maybe holding it ([`MAY_HAVE_IT`], with "Check again").
    pub(crate) fn answer_gap(&self, session: &str) -> &'static str {
        if self.over(session) { MAY_HAVE_IT } else { RESYNCING }
    }

    /// `session` has recovered from no gap in [`CHECK_WAIT`].
    pub(crate) fn over(&self, session: &str) -> bool {
        self.gaps.get(session).is_some_and(|g| g.over)
    }

    /// An answer of `session` whose card can't say where it is, on a view
    /// recovering from dropped events (the module's "Dropped events"):
    /// - taken in, its turn ended since, past [`CHECK_WAIT`]: no settled
    ///   view has said whether the turn's end saved it, so the card can't
    ///   say it is in the chat; nor can a Resend be checked;
    /// - queued or held, once the gap's wait has run out, and while "Check
    ///   again" checks: those words say the engine's slot has it, and no
    ///   event since the gap has said so. A cancellation's `Dequeued` may
    ///   be among what was dropped, and no clock moves either state: held
    ///   said "it goes in after the agent's next turn" for good (Shady #81
    ///   round 6). The memo's state stays as it was, and its evidence (a
    ///   held entry's `Idle`): the first settled view after the gap
    ///   settles it as ever.
    ///
    /// It says so ([`Memos::words`]), with no Resend or Retry.
    pub(crate) fn unchecked(&self, session: &str, e: &Entry) -> bool {
        match e.state {
            State::InChat => self.over(session) && self.taken.get(session).is_some_and(|v| v.iter().any(|k| k.card == e.card && k.ended)),
            State::Queued | State::Held => self.gaps.get(session).is_some_and(|g| g.waited),
            _ => false,
        }
    }

    /// Run `f` on `session`'s memo, if it has one (an event for a session
    /// with nothing out costs a lookup); note a change.
    fn with(&mut self, session: &str, f: impl FnOnce(&mut Memo)) {
        let Some(m) = self.by.get_mut(session) else { return };
        let before = m.entries().to_vec();
        f(m);
        if m.entries() != before.as_slice() {
            self.changes += 1;
        }
        let was_in = |e: &Entry| before.iter().any(|b| b.card == e.card && b.text == e.text && b.state == State::InChat);
        for e in m.entries().iter().filter(|e| e.state == State::InChat && !was_in(e)) {
            if self.reached.len() >= REACHED_CAP {
                self.reached.remove(0);
            }
            self.reached.push(Reached { session: session.to_string(), card: e.card.clone(), text: e.text.clone(), reply: e.reply.clone() });
        }
        if m.entries().is_empty() {
            self.by.remove(session);
            self.taken.remove(session);
        }
        self.tidy(session);
    }

    /// Forget what the turn's end missed for each entry of `session` that
    /// moved on (sent again, or gone), and what an entry gone, or replaced,
    /// expected.
    fn tidy(&mut self, session: &str) {
        let m = self.by.get(session);
        if let Some(chains) = self.chains.get_mut(session) {
            chains.retain(|c| !c.expects.is_empty() && m.and_then(|m| m.get(&c.card)).is_some_and(|e| e.text == c.text));
            if chains.is_empty() {
                self.chains.remove(session);
            }
        }
        let Some(missed) = self.missed.get_mut(session) else { return };
        missed.retain(|id| m.and_then(|m| m.get(id)).is_some_and(|e| matches!(e.state, State::InChat | State::Unconfirmed)));
        if missed.is_empty() {
            self.missed.remove(session);
        }
    }

    /// What `session`'s entry `e` expects (the module's "Changes").
    fn chain(&self, session: &str, e: &Entry) -> Option<&Chain> {
        self.chains.get(session)?.iter().find(|c| c.card == e.card && c.text == e.text)
    }

    /// What a new entry for `card` of `session` expects: of this desk's
    /// answers to it the engine took in since the last attach, those the
    /// card's answers don't show yet and no turn's end has settled. The
    /// entry it replaces, so taken in, and what that entry expected, so
    /// on back (the module's "Changes"). The press is on a settled view
    /// ([`Painted::settled`], and no gap: [`recovering`]): an answer whose
    /// turn ended since and that the card doesn't show, the view says, is
    /// one the turn's end lost.
    fn expects(&self, session: &str, card: &Card) -> Vec<Expected> {
        let Some(e) = card.id.as_ref().and_then(|id| self.by.get(session)?.get(id)).filter(|e| e.answers(card)) else {
            return Vec::new();
        };
        let count = |text: &str| card.answers.iter().filter(|a| a.text == text).count();
        let mut out: Vec<Expected> = self
            .chain(session, e)
            .into_iter()
            .flat_map(|c| &c.expects)
            .filter(|x| !x.ended && !x.missed && count(&x.text) < x.nth)
            .cloned()
            .collect();
        let taken = self.taken.get(session).and_then(|v| v.iter().find(|k| k.card == e.card));
        let nth = e.seen.iter().filter(|t| **t == e.text).count() + 1;
        if e.state == State::InChat && taken.is_some_and(|k| !k.ended) && count(&e.text) < nth {
            out.push(Expected { text: e.text.clone(), reply: e.reply.clone(), nth, ended: false, missed: false });
        }
        out
    }

    /// Whether `card`'s answers in `fold` show one of the text of an answer
    /// its entry expected that a turn's end lost: another's, which that
    /// entry's memo counts as the one it expected. No Resend or Retry then:
    /// it would put this desk's answer over a later one (the module's
    /// "Changes").
    pub(crate) fn doubtful(&self, session: &str, card: &CardId, fold: &Fold) -> bool {
        let (Some(e), Some(c)) = (self.by.get(session).and_then(|m| m.get(card)), fold.bound(card)) else { return false };
        self.chain(session, e)
            .is_some_and(|ch| ch.expects.iter().any(|x| x.missed && c.answers.iter().filter(|a| a.text == x.text).count() >= x.nth))
    }

    /// An event of `session`'s attachment. An attach ends what was taken
    /// in: its history is the daemon's word now, and one that lost an
    /// answer (a restart before the save) shows it. An `Idle` ends the
    /// turn that took each in: the next settled history is that turn's.
    pub(crate) fn on(&mut self, session: &str, ev: Heard<'_>, now: Instant) {
        if ev == Heard::Attached {
            self.taken.remove(session);
        }
        // So with what each change expects: the next settled view is the
        // daemon's word on it.
        if matches!(ev, Heard::Attached | Heard::Idle) {
            for x in self.chains.get_mut(session).into_iter().flatten().flat_map(|c| &mut c.expects) {
                x.ended = true;
            }
        }
        self.with(session, |m| m.on(ev, now));
        match ev {
            Heard::Delivered { text } => {
                let Some(m) = self.by.get(session) else { return };
                for e in m.entries().iter().filter(|e| e.state == State::InChat && e.text == text) {
                    let taken = self.taken.entry(session.to_string()).or_default();
                    match taken.iter_mut().find(|k| k.card == e.card) {
                        Some(k) => k.ended = false,
                        None => taken.push(Taken { card: e.card.clone(), ended: false }),
                    }
                }
            }
            Heard::Idle => {
                for k in self.taken.get_mut(session).into_iter().flatten() {
                    k.ended = true;
                }
            }
            _ => {}
        }
    }

    /// `session`'s transcript as it stands: its entries follow the fold
    /// (and, on a settled view, the settle rule).
    pub(crate) fn observe(&mut self, session: &str, t: &Transcript, now: Instant) {
        self.follow_gap(session, t, now);
        self.prune(session, t);
        let fold = self.fold(session, t);
        let view = Self::view(&fold, t);
        self.with(session, |m| m.observe(&view, now));
        self.retire(session, &fold, t, now);
    }

    /// Retire each entry of `session` at rest (not confirmed, or not sent)
    /// that is [`Memos::doubtful`] on `t`'s history, settled: an answer it
    /// expected, which a turn's end lost, shows there, another's, and the
    /// entry counts it as the one it expected (the module's "Changes").
    /// The card's answer is then the history's: its receipt, its change's
    /// starting point and what a change is told from.
    ///
    /// The memo (`cards_spec::memo`, pinned) has no call to drop one entry,
    /// and its rule 1 drops an entry whose card is closed, on any view. So
    /// it reads again the view it has just followed, `fold`, with those
    /// cards closed in it: they go, and each other entry follows the same
    /// view a second time, which moves none.
    fn retire(&mut self, session: &str, fold: &Fold, t: &Transcript, now: Instant) {
        let Some(m) = self.by.get(session).filter(|_| settled(t)) else { return };
        let gone: Vec<CardId> = m
            .entries()
            .iter()
            .filter(|e| matches!(e.state, State::Unconfirmed | State::NotSent) && self.doubtful(session, &e.card, t.history_cards()))
            .map(|e| e.card.clone())
            .collect();
        if gone.is_empty() {
            return;
        }
        let kept: Vec<Entry> = m.entries().iter().filter(|e| !gone.contains(&e.card)).cloned().collect();
        let mut closed = fold.clone();
        for id in &gone {
            log::info!(target: "synaps_desktop::card", "card {id}: another's answer of the one it expected shows; its entry gives way");
            let tool_id = RETIRE.to_string();
            closed.apply(&Folded::Call {
                tool_id: tool_id.clone(),
                name: NAMES[0].to_string(),
                input: serde_json::json!({ "close": id.to_string() }),
            });
            closed.apply(&Folded::Result { tool_id, text: Envelope::closed(id.clone()).to_string() });
        }
        let view = Self::view(&closed, t);
        self.with(session, |m| m.observe(&view, now));
        debug_assert_eq!(
            self.by.get(session).map_or(&[][..], Memo::entries),
            kept.as_slice(),
            "the second reading drops those retired and moves no other entry"
        );
    }

    /// A gap starts with the first view that resyncs, and ends with the
    /// first that doesn't (the history asked for again, or an attach's).
    fn follow_gap(&mut self, session: &str, t: &Transcript, now: Instant) {
        if recovering(t) {
            if !self.gaps.contains_key(session) {
                self.gaps.insert(session.to_string(), Gap { since: now, over: false, waited: false });
                self.changes += 1;
            }
        } else if self.gaps.remove(session).is_some() {
            self.changes += 1;
        }
    }

    /// What a transcript shows the memo: `fold` (its own, with what was
    /// taken in on top: [`Memos::observe`]), and on a settled history
    /// whose slot is known ([`settled`]) the slot's steer and whether a
    /// turn streams.
    fn view<'a>(fold: &'a Fold, t: &'a Transcript) -> View<'a> {
        View { fold, settled: settled(t), queued: t.queued_message(), streaming: t.busy() }
    }

    /// Settle each answer taken in on `t`, the module's "Taken in": the
    /// history shows it, or its turn ended and the history, settled,
    /// doesn't (missed). Forget it too once no entry holds it in the chat.
    fn prune(&mut self, session: &str, t: &Transcript) {
        self.prune_chains(session, t);
        let Some(taken) = self.taken.get_mut(session) else { return };
        let m = self.by.get(session);
        let settled = settled(t);
        let mut missed = Vec::new();
        taken.retain(|k| {
            let Some(e) = m.and_then(|m| m.get(&k.card)).filter(|e| e.state == State::InChat) else { return false };
            if t.history_cards().bound(&k.card).is_some_and(|c| shows(e, c)) {
                return false;
            }
            if k.ended && settled {
                missed.push(k.card.clone());
                return false;
            }
            true
        });
        if taken.is_empty() {
            self.taken.remove(session);
        }
        for id in missed {
            let v = self.missed.entry(session.to_string()).or_default();
            if !v.contains(&id) {
                v.push(id);
            }
        }
    }

    /// Settle what each change of `session` expects on `t`: one the
    /// history shows has come; one whose turn ended, on a settled view
    /// that doesn't, the turn's end lost (the module's "Changes").
    fn prune_chains(&mut self, session: &str, t: &Transcript) {
        let Some(chains) = self.chains.get_mut(session) else { return };
        let settled = settled(t);
        for c in chains.iter_mut() {
            let card = t.history_cards().bound(&c.card);
            let count = |text: &str| card.map_or(0, |c| c.answers.iter().filter(|a| a.text == text).count());
            c.expects.retain_mut(|x| {
                if x.missed {
                    return true;
                }
                if count(&x.text) >= x.nth {
                    return false;
                }
                x.missed = x.ended && settled;
                true
            });
        }
        self.tidy(session);
    }

    /// Why `session`'s entry `e` is unconfirmed or not sent: its own
    /// cause, but for an answer taken in that its turn's end didn't save,
    /// which the memo reads as a history that lost it (a reconnect's
    /// cause): the turn's end, "it didn't reach the chat".
    pub(crate) fn cause(&self, session: &str, e: &Entry) -> Option<Cause> {
        let missed = self.missed.get(session).is_some_and(|v| v.contains(&e.card));
        if missed && e.state == State::Unconfirmed && e.cause == Some(Cause::Lost(Trigger::Attached)) {
            return Some(Cause::Lost(Trigger::Idle));
        }
        e.cause.clone()
    }

    /// What a card in the memo says of its answer, as §3.5's table words
    /// it: the state word on its receipt line, and (unconfirmed, not
    /// sent) the cause line under it. In the chat, it says what it was
    /// (D4): `answered`, `changed` or `skipped`, as the fold's receipt.
    pub(crate) fn words(&self, session: &str, e: &Entry) -> (&'static str, Option<String>) {
        if self.unchecked(session, e) {
            return ("not confirmed", Some(self.answer_gap(session).into()));
        }
        let word = match e.state {
            State::Sending => "sending…",
            State::Queued => "queued · the agent reads it at its next step",
            State::Held => "held · it goes in after the agent's next turn",
            State::Checking => "checking…",
            State::InChat => {
                let word = match e.reply {
                    Reply::Answer(_) => "answered",
                    Reply::Change(_) => "changed",
                    Reply::Skip => "skipped",
                };
                return (word, None);
            }
            State::Unconfirmed => "not confirmed",
            State::NotSent => "not sent",
        };
        let cause = matches!(e.state, State::Unconfirmed | State::NotSent).then(|| self.cause(session, e)).flatten();
        (word, cause.as_ref().map(Cause::line))
    }

    /// The fold `session`'s memo follows: `t`'s, and on top each answer
    /// taken in that it doesn't show (the module's "Taken in").
    fn fold<'a>(&self, session: &str, t: &'a Transcript) -> Cow<'a, Fold> {
        let mut fold = Cow::Borrowed(t.cards());
        let (Some(taken), Some(m)) = (self.taken.get(session), self.by.get(session)) else { return fold };
        for e in taken.iter().filter_map(|k| m.get(&k.card)).filter(|e| e.state == State::InChat) {
            if !fold.bound(&e.card).is_some_and(|c| shows(e, c)) {
                fold.to_mut().apply(&Folded::User { text: e.text.clone() });
            }
        }
        fold
    }

    /// "Check again" on a card of `session`, its gap over: the client asks
    /// for the history again, and the cards say they are checking for
    /// another [`CHECK_WAIT`].
    pub(crate) fn check_again(&mut self, session: &str, now: Instant) {
        if let Some(g) = self.gaps.get_mut(session).filter(|g| g.over) {
            *g = Gap { since: now, over: false, waited: true };
            self.changes += 1;
        }
    }

    /// The clock, for every session: and a gap [`CHECK_WAIT`] long is over.
    pub(crate) fn tick(&mut self, now: Instant) {
        let sessions: Vec<String> = self.by.keys().cloned().collect();
        for s in sessions {
            self.with(&s, |m| m.tick(now));
        }
        for g in self.gaps.values_mut().filter(|g| !g.over && now.saturating_duration_since(g.since) >= CHECK_WAIT) {
            g.over = true;
            g.waited = true;
            self.changes += 1;
        }
    }

    /// A press on `card` of `session` with `reply`: the text to send, or
    /// `None` (it can't be answered, or an answer to it is out).
    ///
    /// A change (D4) replaces this desk's last entry for the card: the new
    /// entry sees the card's answers, then those it expects ([`Memos::expects`],
    /// the module's "Changes"), whose arrival is then this desk's, and older.
    pub(crate) fn press(&mut self, session: &str, card: &Card, reply: Reply, now: Instant) -> Option<String> {
        let expects = self.expects(session, card);
        let seen = if expects.is_empty() {
            Cow::Borrowed(card)
        } else {
            let mut c = card.clone();
            c.answers.extend(expects.iter().map(|x| Answered { text: x.text.clone(), reply: x.reply.clone(), at: usize::MAX }));
            Cow::Owned(c)
        };
        let m = self.by.entry(session.to_string()).or_default();
        let text = m.press(&seen, reply, now);
        let empty = m.entries().is_empty();
        if let (Some(text), Some(id)) = (&text, card.id.as_ref()) {
            self.changes += 1;
            let chains = self.chains.entry(session.to_string()).or_default();
            chains.retain(|c| c.card != *id);
            chains.push(Chain { card: id.clone(), text: text.clone(), expects });
        }
        if empty {
            self.by.remove(session);
        }
        self.tidy(session);
        text
    }

    /// Retry `card`, not sent: the same text.
    pub(crate) fn retry(&mut self, session: &str, card: &CardId, fold: &Fold, now: Instant) -> Option<String> {
        if self.doubtful(session, card, fold) {
            return None;
        }
        let mut out: Option<String> = None;
        self.with(session, |m| out = m.retry(card, fold, now));
        out
    }

    /// A Resend click on `card`, checked on `t`'s view as it is now.
    pub(crate) fn resend(&mut self, session: &str, card: &CardId, t: &Transcript, now: Instant) -> Resend {
        self.prune(session, t);
        if self.doubtful(session, card, t.cards()) {
            return Resend::No;
        }
        let fold = self.fold(session, t);
        let view = Self::view(&fold, t);
        let mut out = Resend::No;
        self.with(session, |m| out = m.resend(card, &view, now));
        out
    }

    /// The answers that reached the chat since the last call, oldest
    /// first: an entry moved to "in the chat" (§3.5's table), by the fold
    /// or `SteeringDelivered`. Never a press, a sending, queued, held or
    /// unconfirmed entry.
    pub(crate) fn take_reached(&mut self) -> Vec<Reached> {
        std::mem::take(&mut self.reached)
    }

    /// `session`'s entry for card `card`'s id, whatever its binding (tests).
    #[cfg(test)]
    pub(crate) fn get_state(&self, session: &str, card: &str) -> Option<State> {
        self.by.get(session)?.get(&CardId::parse(card)?).map(|e| e.state)
    }

    /// Whether Resend shows on `card` (with the window's `input_gate()`).
    pub(crate) fn may_resend(&self, session: &str, card: &CardId, fold: &Fold) -> bool {
        self.by.get(session).is_some_and(|m| m.may_resend(card, fold)) && !self.doubtful(session, card, fold)
    }
}

impl crate::model::Model {
    /// Whether store `lid` may send an agent card's answer now: its
    /// attachment and session, or why not (the card's line).
    fn card_gate(&self, lid: crate::live::LiveId) -> Result<(u64, String), String> {
        let l = &self.lives[lid];
        l.input_gate().map_err(str::to_string)?;
        if l.transcript.compacting() {
            return Err(COMPACTING.into());
        }
        let (Some(seq), Some(a)) = (l.attachment(), l.active.as_ref()) else {
            return Err("not connected to the session: try again in a moment".into());
        };
        // Events dropped: nothing is checked on what is held until the
        // history is in again and an event on this stream has said the
        // slot (the module's "Dropped events").
        if recovering(&l.transcript) {
            return Err(self.memos.gap(&a.session_id).into());
        }
        Ok((seq, a.session_id.clone()))
    }

    /// A click on the card `painted` of store `lid`, answering `reply`: the
    /// answer to send, or why nothing goes. Checked now, on what the store
    /// holds now, not on what was painted: the window may send, no
    /// compaction runs, the history is settled, and the card is still the
    /// one painted and may be answered (§3.5, "The binding").
    pub(crate) fn press_card(
        &mut self,
        lid: crate::live::LiveId,
        painted: &Painted,
        reply: Reply,
        now: Instant,
    ) -> Result<Outgoing, String> {
        let (seq, session) = self.card_gate(lid)?;
        let t = &self.lives[lid].transcript;
        let card = painted.settled(t)?;
        let busy = t.busy();
        let text = self.memos.press(&session, card, reply, now).ok_or(OUT)?;
        Ok(Outgoing { seq, busy, card: painted.id.clone(), text })
    }

    /// Retry the card `painted`'s answer, not sent: the same text, by the
    /// same checks.
    pub(crate) fn retry_card(&mut self, lid: crate::live::LiveId, painted: &Painted, now: Instant) -> Result<Outgoing, String> {
        let (seq, session) = self.card_gate(lid)?;
        let t = &self.lives[lid].transcript;
        painted.settled(t)?;
        let busy = t.busy();
        let text = self.memos.retry(&session, &painted.id, t.cards(), now).ok_or(CHANGED)?;
        Ok(Outgoing { seq, busy, card: painted.id.clone(), text })
    }

    /// A Resend click on the card `painted` (§3.5, "Resend"): the settle
    /// rules on the view as it is now, then the gate again. Unsettled, the
    /// card checks and nothing is sent.
    pub(crate) fn resend_card(&mut self, lid: crate::live::LiveId, painted: &Painted, now: Instant) -> Result<Outgoing, String> {
        let (seq, session) = self.card_gate(lid)?;
        let t = &self.lives[lid].transcript;
        // As a click is checked, but an unsettled view checks the entry
        // (the memo's Resend rule) rather than refusing it.
        if painted.find(t.cards()).is_none() || (t.history_settled() && !painted.confirmed(t)) {
            return Err(CHANGED.into());
        }
        let busy = t.busy();
        match self.memos.resend(&session, &painted.id, t, now) {
            Resend::Send(text) => Ok(Outgoing { seq, busy, card: painted.id.clone(), text }),
            Resend::Checking => Err(UNSETTLED.into()),
            Resend::No => Err(CHANGED.into()),
        }
    }

    /// `out` went to the daemon from store `lid`: its echo or refusal is
    /// its card's.
    pub(crate) fn card_sent(&mut self, lid: crate::live::LiveId, out: Outgoing) {
        self.lives[lid].card_sent(out.card, out.text);
    }

    /// `out` could not be handed to the connection (its thread stopped):
    /// not sent.
    pub(crate) fn card_unsent(&mut self, lid: crate::live::LiveId, reason: &str) {
        self.card_refused(lid, reason);
    }

    /// A card's answer from store `lid` was refused, or never went: not
    /// sent. Then the view as it is, which the memo follows again: an
    /// answer it leaves at rest that a lost one's copy shows over is
    /// retired at once ([`Memos::retire`]), not at the next event.
    pub(crate) fn card_refused(&mut self, lid: crate::live::LiveId, reason: &str) {
        let l = &self.lives[lid];
        if let Some(s) = l.active.as_ref().map(|a| a.session_id.as_str()) {
            let now = Instant::now();
            self.memos.on(s, Heard::Refused { reason }, now);
            self.memos.observe(s, &l.transcript, now);
        }
    }

    /// "Check again" on a card of store `lid`, dropped events not
    /// recovered from in [`CHECK_WAIT`]: the client asks for the history
    /// again, a `Messages` query on the card's own attachment
    /// ([`synaps_client::Client`]'s `Resync`), and nothing else. It never
    /// attaches or detaches (a `Detach` can move input or revoke a
    /// driver), so it can settle the history, never the slot.
    pub(crate) fn check_again(&mut self, lid: crate::live::LiveId, now: Instant) -> Option<crate::model::Effect> {
        let l = &self.lives[lid];
        let (Some(seq), Some(a)) = (l.attachment(), l.active.as_ref()) else { return None };
        if !recovering(&l.transcript) {
            return None;
        }
        self.memos.check_again(&a.session_id, now);
        Some(crate::model::Effect::Resync(seq))
    }

    /// The card memos' clock: then each attached session's view, so an
    /// entry the clock moved to checking settles on it at once.
    pub(crate) fn tick_cards(&mut self, now: Instant) {
        self.memos.tick(now);
        for id in self.lives.ids() {
            let l = &self.lives[id];
            if let Some(a) = l.active.as_ref() {
                self.memos.observe(&a.session_id, &l.transcript, now);
            }
        }
    }
}

#[cfg(test)]
#[path = "deliver_tests.rs"]
mod tests;
