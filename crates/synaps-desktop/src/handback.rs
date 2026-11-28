//! Handing back (synaps-cards `docs/blocks.md` v1.3 §3.2, §3.3, §3.6,
//! §3.7, §5.2; §8.2's D8 row, its second half: D8b): a held draft's edit,
//! or a restore, sent as a user message, and what became of it.
//!
//! - **Sending** takes the card path exactly (§3.7): a `Steer` while the
//!   session is busy, a `Submit` when idle, on the block's own attachment
//!   (`client.on(seq)`), behind the card's gate ([`Model::card_gate`]: the
//!   input gate, no compaction, no gap). The notices that ride typed text
//!   ride it too (§3.5): line 1, line 2, then the desires' lines, built at
//!   the moment it goes ([`crate::desires::Desires::ride`]); one whose
//!   marks aren't written yet goes once they are.
//! - **Delivery** is B4's block memo (`cards_spec::memo::block`), one per
//!   session beside its card memo ([`crate::deliver::Memos`]), fed the
//!   same events, views and clock. Its entry is the edit's body (lines 1
//!   and 2), its binding (id, tag, base) as sent. A `Refused` names no
//!   text: it is the memo's whose oldest sending entry is older
//!   (`oldest_sending`, §3.7 "Two memos, one session").
//! - **What a view shows the memo** ([`DeskEdits`]): the session's genuine
//!   user rows whose body is an edit of the incarnation, in order (the
//!   rows the transcript draws, as the card memo reads the fold the rows
//!   draw), and whether the incarnation is live on the board (bound).
//!   Nothing is copied: each read walks the rows again.
//! - **Application** is the board's (§3.7): each hand-back's outcome, per
//!   key, drawn on its row ([`outcome_words`]). "In the chat" never means
//!   "applied".
//! - **Bounded** (§1.8): the memo keeps 32 entries a session, each at most
//!   [`ENTRY_MAX_BYTES`]; the desk keeps memos for [`SESSIONS_MAX`]
//!   sessions, a session's memo going once it is empty. A hand-back on
//!   another session past that is refused ([`SESSIONS_FULL`]) while every
//!   session kept has an entry that can't go (one not in the chat or
//!   retired); else the memo with none goes.

use std::collections::HashMap;
use std::time::Instant;

use cards_spec::block::message::{self, Edit, Summary, Trailer};
use cards_spec::block::{Ref, Tag};
use cards_spec::board::{Code, EditOutcome, EditResult, Key, keys};
use cards_spec::memo::block::{self as bm, ENTRY_MAX_BYTES, Memo, Refusal};
use cards_spec::memo::{Event as Heard, Resend, State};
use synaps_client::Transcript;
use synaps_client::blocks::Drawn;
use synaps_client::transcript::ItemKind;

/// Sessions whose hand-backs the desk keeps (§1.8).
pub(crate) const SESSIONS_MAX: usize = 16;

/// A hand-back on a 17th session while the 16 kept each have one out.
pub(crate) const SESSIONS_FULL: &str = "hand-backs out in 16 sessions · resend or discard one";

/// A hand-back that went into a compaction's successor (§5.2): its row.
pub(crate) const COMPACTED_AWAY: &str = "delivered · its block was compacted away";

/// An edit of this incarnation is out (§3.7): the block takes no new hold.
pub(crate) const SENDING: &str = "Sending…";

/// Whether `body`'s line 1 starts as an edit of `id`'s does (§3.3),
/// read without allocating (cards-spec's own check is private).
fn names(body: &str, id: &Ref) -> bool {
    let Some(rest) = body.strip_prefix("[block ") else { return false };
    let rest = match id {
        Ref::Block(b) => rest.strip_prefix(b.as_str()),
        Ref::Card(c) => rest.strip_prefix("card:").and_then(|r| r.strip_prefix(c.as_str())),
    };
    rest.is_some_and(|r| r.starts_with(" edited r"))
}

/// Whether user text `text` is an edit message (§3.3): its body, all of
/// it the grammar. A row draws it as a hand-back.
pub(crate) fn is_edit(text: &str) -> bool {
    let body = message::split(text).0;
    body.starts_with("[block ") && message::parse(body).is_some()
}

/// What the session holds, for the memo (§3.7): its rows' edits, then
/// each this desk's the engine took in that no row shows yet ([`Taken`]),
/// and the board they were drawn from.
pub(crate) struct DeskEdits<'a> {
    t: &'a Transcript,
    drawn: Option<&'a Drawn>,
    taken: &'a [Taken],
}

impl<'a> DeskEdits<'a> {
    pub(crate) fn of(t: &'a Transcript, drawn: Option<&'a Drawn>) -> Self {
        DeskEdits { t, drawn, taken: &[] }
    }

    fn with(mut self, taken: Option<&'a Vec<Taken>>) -> Self {
        self.taken = taken.map_or(&[], Vec::as_slice);
        self
    }
}

/// Whether one of `t`'s rows is user text whose body is `body`.
fn shows(t: &Transcript, body: &str) -> bool {
    t.items().iter().any(|i| i.kind == ItemKind::User && !i.is_note() && message::split(&i.text).0 == body)
}

/// Whether `body` is an edit of incarnation `of` of `id`.
fn edits(body: &str, id: &Ref, of: &Tag) -> bool {
    names(body, id) && message::parse(body).is_some_and(|e| e.id == *id && e.of == *of)
}

impl bm::Edits for DeskEdits<'_> {
    fn each(&self, id: &Ref, of: &Tag, each: &mut dyn FnMut(&str)) {
        for it in self.t.items().iter().filter(|i| i.kind == ItemKind::User && !i.is_note()) {
            let body = message::split(&it.text).0;
            if edits(body, id, of) {
                each(body);
            }
        }
        for k in self.taken.iter().filter(|k| edits(&k.body, id, of)) {
            each(&k.body);
        }
    }

    fn live(&self, id: &Ref, of: &Tag) -> bool {
        let Some(d) = self.drawn else { return false };
        match &d.board {
            Some(st) => st.block(id).is_some_and(|b| b.tag == *of),
            None => crate::lease::find_any(d, crate::blocks::row_id(id))
                .and_then(|(s, _)| s.bound.as_ref().map(|b| b.tag))
                .is_some_and(|t| t == *of),
        }
    }
}

/// An edit to send: its attachment, how, the message whole (body and
/// notices), and its body (what the memo and the echo match).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditOut {
    pub(crate) seq: u64,
    pub(crate) busy: bool,
    pub(crate) text: String,
    pub(crate) body: String,
}

/// Line 1 of an edit, owned: what [`Summary`] borrows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line1 {
    pub(crate) title: Option<String>,
    pub(crate) back_to: Option<u32>,
    pub(crate) pieces: Vec<String>,
}

impl Line1 {
    pub(crate) fn summary(&self) -> Summary<'_> {
        Summary { title: self.title.as_deref(), back_to: self.back_to, pieces: self.pieces.iter().map(String::as_str).collect() }
    }
}

/// A refusal's words (the block's note).
pub(crate) fn refusal(r: &Refusal) -> String {
    match r {
        Refusal::Gone => "the block changed · nothing was sent".into(),
        Refusal::InFlight => SENDING.into(),
        Refusal::Full => bm::FULL.into(),
        Refusal::Edit(e) => e.to_string(),
    }
}

/// A hand-back of this desk's the engine took in (`SteeringDelivered`, or
/// a compaction's restored steer), until a history settles it, as the card
/// memo's (`crate::deliver`'s "Taken in"): the engine drains a steer into
/// its history with no row for it until that history lands, and the slot's
/// clearing comes first. Read on top of the rows, it is in the chat; the
/// first settled view after its turn ended that doesn't show it, it isn't.
#[derive(Debug)]
pub(crate) struct Taken {
    body: String,
    /// An `Idle` or an attach came since it was taken in.
    ended: bool,
}

/// The desk's block memos, by session (module docs).
#[derive(Debug, Default)]
pub(crate) struct HandBacks {
    by: HashMap<String, Memo>,
    /// By session, its hand-backs taken in (≤ `bm::CAP` each).
    taken: HashMap<String, Vec<Taken>>,
    /// Bumped whenever an entry changes: blocks and rows drawn from one
    /// are measured again.
    changes: u64,
}

impl HandBacks {
    pub(crate) fn changes(&self) -> u64 {
        self.changes
    }

    /// Whether any entry waits on the clock (an echo, a settled view).
    pub(crate) fn timed(&self) -> bool {
        self.by.values().flat_map(Memo::entries).any(|e| matches!(e.state, State::Sending | State::Checking))
    }

    /// What they keep (bytes): each entry at its ceiling, and the map's.
    pub(crate) fn bytes(&self) -> usize {
        self.by.capacity() * std::mem::size_of::<(String, Memo)>()
            + self.by.iter().map(|(s, m)| s.capacity() + m.entries().len() * ENTRY_MAX_BYTES).sum::<usize>()
            + self.taken.capacity() * std::mem::size_of::<(String, Vec<Taken>)>()
            + self
                .taken
                .iter()
                .map(|(s, v)| {
                    s.capacity() + v.capacity() * std::mem::size_of::<Taken>() + v.iter().map(|k| k.body.capacity()).sum::<usize>()
                })
                .sum::<usize>()
    }

    pub(crate) fn len(&self) -> usize {
        self.by.len()
    }

    /// `session`'s oldest entry sending: when.
    pub(crate) fn oldest_sending(&self, session: &str) -> Option<Instant> {
        self.by.get(session).and_then(Memo::oldest_sending)
    }

    fn with<R>(&mut self, session: &str, f: impl FnOnce(&mut Memo) -> R) -> Option<R> {
        let m = self.by.get_mut(session)?;
        let before = m.entries().to_vec();
        let r = f(m);
        if m.entries() != before.as_slice() {
            self.changes += 1;
        }
        if m.entries().is_empty() {
            self.by.remove(session);
            self.taken.remove(session);
        }
        Some(r)
    }

    /// A session event (a `Refused` is [`crate::deliver::Memos::on`]'s to
    /// route). An attach ends what was taken in (its history is the
    /// daemon's word now); an `Idle`, the turn that took each in.
    pub(crate) fn on(&mut self, session: &str, ev: Heard<'_>, now: Instant) {
        match ev {
            Heard::Attached => {
                self.taken.remove(session);
            }
            Heard::Idle => self.taken.get_mut(session).into_iter().flatten().for_each(|k| k.ended = true),
            _ => {}
        }
        self.with(session, |m| m.on(ev, now));
        if let Heard::Delivered { text } = ev
            && let Some(e) = self.by.get(session).and_then(|m| m.get(text)).filter(|e| e.state == State::InChat)
        {
            let body = e.text.clone();
            let v = self.taken.entry(session.to_string()).or_default();
            if let Some(k) = v.iter_mut().find(|k| k.body == body) {
                k.ended = false;
            } else if v.len() < bm::CAP {
                v.push(Taken { body, ended: false });
            }
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) {
        let ids: Vec<String> = self.by.keys().cloned().collect();
        for s in ids {
            self.with(&s, |m| m.tick(now));
        }
    }

    /// `session`'s transcript as it stands (its board `drawn`): each entry
    /// follows it.
    pub(crate) fn observe(&mut self, session: &str, t: &Transcript, drawn: Option<&Drawn>, now: Instant) {
        if !self.by.contains_key(session) {
            return;
        }
        let settled = crate::deliver::settled(t);
        // What a settled view settles: one its rows show is there; one
        // whose turn ended that they don't, the turn's end lost.
        if settled && let Some(v) = self.taken.get_mut(session) {
            v.retain(|k| !k.ended && !shows(t, &k.body));
            if v.is_empty() {
                self.taken.remove(session);
            }
        }
        let Self { by, taken, changes } = self;
        let edits = DeskEdits::of(t, drawn).with(taken.get(session));
        let view = bm::View { edits: &edits, settled, queued: t.queued_message(), streaming: t.busy() };
        let Some(m) = by.get_mut(session) else { return };
        let before = m.entries().to_vec();
        m.observe(&view, now);
        if m.entries() != before.as_slice() {
            *changes += 1;
        }
    }

    /// `session`'s view, for a send: its rows and what was taken in.
    pub(crate) fn edits<'a>(&'a self, session: &str, t: &'a Transcript, drawn: Option<&'a Drawn>) -> DeskEdits<'a> {
        DeskEdits::of(t, drawn).with(self.taken.get(session))
    }

    /// Room for a memo of `session`: it has one, there are fewer than
    /// [`SESSIONS_MAX`], or one with nothing that can't go goes.
    fn room(&mut self, session: &str) -> Result<(), String> {
        if self.by.contains_key(session) || self.by.len() < SESSIONS_MAX {
            return Ok(());
        }
        let spare = self.by.iter().find(|(_, m)| !m.entries().iter().any(bm::Entry::recoverable)).map(|(s, _)| s.clone());
        match spare {
            Some(s) => {
                self.by.remove(&s);
                Ok(())
            }
            None => Err(SESSIONS_FULL.into()),
        }
    }

    /// Whether `edit` may be handed back on `session` now (`t` and `drawn`
    /// its view): exactly what [`HandBacks::hand_back`] admits.
    pub(crate) fn may_hand_back(
        &self,
        session: &str,
        edit: &Edit,
        l1: &Line1,
        t: &Transcript,
        drawn: Option<&Drawn>,
    ) -> Result<(), String> {
        if !self.by.contains_key(session)
            && self.by.len() >= SESSIONS_MAX
            && self.by.values().all(|m| m.entries().iter().any(bm::Entry::recoverable))
        {
            return Err(SESSIONS_FULL.into());
        }
        let empty = Memo::default();
        let edits = self.edits(session, t, drawn);
        self.by.get(session).unwrap_or(&empty).may_hand_back(edit, &l1.summary(), &edits).map_err(|r| refusal(&r))
    }

    /// A hand-back of `edit` on `session`, `trailer` riding it: the
    /// message to send.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn hand_back(
        &mut self,
        session: &str,
        edit: &Edit,
        l1: &Line1,
        trailer: &Trailer,
        t: &Transcript,
        drawn: Option<&Drawn>,
        now: Instant,
    ) -> Result<String, String> {
        self.room(session)?;
        let Self { by, taken, changes } = self;
        let edits = DeskEdits::of(t, drawn).with(taken.get(session));
        let m = by.entry(session.to_string()).or_default();
        let r = m.hand_back(edit, &l1.summary(), trailer, &edits, now).map_err(|r| refusal(&r));
        if m.entries().is_empty() {
            by.remove(session);
        }
        if r.is_ok() {
            *changes += 1;
        }
        r
    }

    pub(crate) fn may_retry(&self, session: &str, body: &str, t: &Transcript, drawn: Option<&Drawn>) -> bool {
        self.by.get(session).is_some_and(|m| m.may_retry(body, &self.edits(session, t, drawn)))
    }

    pub(crate) fn may_resend(&self, session: &str, body: &str, t: &Transcript, drawn: Option<&Drawn>) -> bool {
        self.by.get(session).is_some_and(|m| m.may_resend(body, &self.edits(session, t, drawn)))
    }

    /// Retry hand-back `body` (not sent), `trailer` riding it.
    pub(crate) fn retry(
        &mut self,
        session: &str,
        body: &str,
        trailer: &Trailer,
        t: &Transcript,
        drawn: Option<&Drawn>,
        now: Instant,
    ) -> Option<String> {
        let Self { by, taken, changes } = self;
        let edits = DeskEdits::of(t, drawn).with(taken.get(session));
        let r = by.get_mut(session)?.retry(body, trailer, &edits, now);
        *changes += 1;
        r
    }

    /// A Resend of hand-back `body` (not confirmed) against the view as it
    /// is, `trailer` riding it.
    pub(crate) fn resend(
        &mut self,
        session: &str,
        body: &str,
        trailer: &Trailer,
        t: &Transcript,
        drawn: Option<&Drawn>,
        now: Instant,
    ) -> Resend {
        let Self { by, taken, changes } = self;
        let edits = DeskEdits::of(t, drawn).with(taken.get(session));
        let view = bm::View { edits: &edits, settled: crate::deliver::settled(t), queued: t.queued_message(), streaming: t.busy() };
        let Some(m) = by.get_mut(session) else { return Resend::No };
        *changes += 1;
        m.resend(body, trailer, &view, now)
    }

    /// Discard (§3.7, `Memo::forget`): the entry goes, nothing else.
    pub(crate) fn forget(&mut self, session: &str, body: &str) -> bool {
        self.with(session, |m| m.forget(body).is_some()).unwrap_or(false)
    }

    /// This desk's latest entry of incarnation `of` of `id` on `session`.
    pub(crate) fn latest(&self, session: &str, id: &Ref, of: Tag) -> Option<&bm::Entry> {
        self.by.get(session)?.entries().iter().rev().find(|e| e.id == *id && e.of == of)
    }

    /// The edit of the incarnation in flight, if any (§3.7).
    pub(crate) fn in_flight(&self, session: &str, id: &Ref, of: Tag) -> Option<&bm::Entry> {
        self.by.get(session)?.in_flight(id, &of)
    }

    /// The entry of hand-back `body` on `session`.
    pub(crate) fn get(&self, session: &str, body: &str) -> Option<&bm::Entry> {
        self.by.get(session)?.get(body)
    }
}

/// The delivery word for entry `e` (§3.7: the cards' states), and its
/// cause under it.
pub(crate) fn words(e: &bm::Entry) -> (&'static str, Option<String>) {
    if let Some(r) = e.retired {
        return (
            match r {
                bm::Retired::Here => "covered by your later hand-back",
                bm::Retired::Elsewhere => "covered by a later edit in the chat",
            },
            None,
        );
    }
    let word = match e.state {
        State::Sending => "sending…",
        State::Queued => "queued · the agent reads it at its next step",
        State::Held => "held · it goes in after the agent's next turn",
        State::Checking => "checking…",
        State::InChat => "in the chat",
        State::Unconfirmed => "hand-back not confirmed",
        State::NotSent => "hand-back not sent",
    };
    (word, e.cause.as_ref().map(cards_spec::memo::Cause::line))
}

/// Why key `k` didn't apply (`c`), for a row.
fn why(k: &Key, c: Code) -> Option<String> {
    let item = match k {
        Key::Item(p) | Key::Set(p) => p.item().unwrap_or(p.head()).to_string(),
        Key::Order(p) => p.head().to_string(),
    };
    Some(match c {
        Code::Applied | Code::Unchanged | Code::Overwritten => return None,
        Code::AlreadyRan => format!("{item} already ran"),
        Code::NotThere => format!("{item} is gone"),
        Code::Collided => "an added step collided · add it again".into(),
        Code::Full => "the plan was full".into(),
        Code::NotProposal => "the diff was no longer a proposal".into(),
        Code::TooBig => "too big for the block".into(),
    })
}

/// What a hand-back's row says it did (§3.7): `applied 3 of 4 · deploy
/// already ran`, `· 1 overwritten by the agent` after; or why it applied
/// nothing. `compacted`: its block's generation was compacted away
/// (§5.2: the queued message moved to the successor, where its id names
/// no block).
pub(crate) fn outcome_words(o: &EditOutcome, edit: Option<&Edit>, compacted: bool) -> String {
    match o.result {
        EditResult::Rewritten => return "not applied: the block was rewritten".into(),
        EditResult::Gone if compacted => return COMPACTED_AWAY.into(),
        EditResult::Gone => return "not applied: the block is gone".into(),
        EditResult::Revisions => return "not applied: replace the block or use a new id".into(),
        EditResult::Applied => {}
    }
    let n = o.codes.len();
    let applied = o.codes.iter().filter(|c| matches!(c, Code::Applied | Code::Unchanged | Code::Overwritten)).count();
    let over = o.codes.iter().filter(|c| **c == Code::Overwritten).count();
    let mut out = if applied == n { "applied".to_string() } else { format!("applied {applied} of {n}") };
    if let Some(e) = edit {
        let mut said: Vec<String> = Vec::new();
        for (k, c) in keys(e).iter().zip(&o.codes) {
            if let Some(w) = why(k, *c).filter(|w| !said.contains(w)) {
                out.push_str(" · ");
                out.push_str(&w);
                said.push(w);
            }
        }
    }
    if over > 0 {
        out.push_str(&format!(" · {over} overwritten by the agent"));
    }
    out
}

#[cfg(test)]
#[path = "handback_tests.rs"]
mod tests;
