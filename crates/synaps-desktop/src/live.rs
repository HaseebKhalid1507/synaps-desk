//! Split panes, step 2 (split/architecture.md §1.4, §3): the per-session
//! store. Everything of one attachment that was on [`Model`]: what is
//! attached and asked for, its transcript, numbers, worker views, input
//! in flight and the prompts answered. [`Model::apply`] routes each
//! session event to the [`Live`] that asked for its `attach_seq`.
//!
//! There is still exactly one pane, and it still switches sessions by
//! replacing the one attachment (`Client::attach`), so there is exactly
//! one `Live`: [`Lives`] holds it as [`LiveId::FIRST`] for the window's
//! life, and a tab switch re-targets it (`Model::want`), as it did the
//! model. Panes name it ([`crate::pane::Pane::live`]); nothing reaches it
//! as "the current one".

use std::collections::{HashSet, VecDeque};

use synaps_client::transcript::Splice;
use synaps_client::{AttachMode, ClientInput, Transcript};

#[cfg(doc)]
use crate::model::Model;
use crate::model::{Active, Effect};
use crate::pane::PaneId;
use crate::rail::State;
use crate::stats::Stats;
use crate::watch::{Candidate, Watch};

/// A live store's id: stable for its life.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct LiveId(u16);

impl LiveId {
    /// The window's first live store, the only one in step 2.
    pub(crate) const FIRST: LiveId = LiveId(0);
}

/// One attachment's store (split/architecture.md §1.4): shared by every
/// pane showing its session (§3.2).
pub struct Live {
    pub active: Option<Active>,
    /// The attachment asked for last: events of any other are stale.
    pub(crate) wanted: Option<u64>,
    /// The attachment this store holds open on the client: the one asked
    /// for last, kept through a switch (`switch_out` and `showing` clear
    /// `wanted`, the client keeps it) until the next attach replaces it or
    /// the last tab's close detaches it ([`Live::release`], Shady #32 H1).
    pub(crate) held: Option<u64>,
    /// The session it asked for (`None`: a create or a resume, whose id
    /// comes with `Attached`).
    pub(crate) wanted_session: Option<String>,
    /// Its state when it was asked for: under a chip that admitted it, its
    /// row stays until the attach lands.
    pub(crate) wanted_fact: Option<State>,
    /// Prompts answered from here that the daemon has not resolved yet, as
    /// `(epoch, prompt id)` ([`Active::epoch`]): prompt ids are per session
    /// and start at 1 (again after a daemon restart), so an id alone would
    /// carry over to the next attachment's prompt.
    pub(crate) answered: HashSet<(u64, u64)>,
    pub transcript: Transcript,
    pub last_error: Option<String>,
    /// The session connection is being re-made (reload).
    pub reattaching: bool,
    /// Sent, not yet confirmed by `TurnStarted` / `Steered`, oldest first:
    /// given back on a refusal (the oldest) or a detach (all) to what sent
    /// it, the composer of a pane or an agent card ([`Target`]).
    pub(crate) in_flight: InFlight,
    /// The attached session's numbers (the composer's foot, the right-hand
    /// panel).
    pub stats: Stats,
    /// The attached session's worker views ([`crate::watch`]): with the
    /// transcript, they go to the tab cache when its tab is left.
    pub watch: Watch,
    /// Worker cancels armed and on their way ([`crate::wtabs`]).
    pub cancels: crate::wtabs::Cancels,
    /// Its transcript's board, worked off the frame ([`crate::board`]).
    pub(crate) board: crate::board::Work,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            active: None,
            wanted: None,
            held: None,
            wanted_session: None,
            wanted_fact: None,
            answered: HashSet::new(),
            transcript: crate::model::transcript(),
            last_error: None,
            reattaching: false,
            in_flight: InFlight::default(),
            stats: Stats::default(),
            watch: Watch::default(),
            cancels: Default::default(),
            board: Default::default(),
        }
    }
}

impl Live {
    /// This store's session is `session_id`, attached.
    pub fn is_active(&self, session_id: &str) -> bool {
        self.active.as_ref().is_some_and(|a| a.session_id == session_id)
    }

    /// The last attach asked for this session (attached or on its way).
    pub fn is_wanted(&self, session_id: &str) -> bool {
        self.wanted_session.as_deref() == Some(session_id)
    }

    /// The transcript the list shows: the focused worker's, else the
    /// session's own.
    pub fn shown(&self) -> &Transcript {
        self.watch.focused().map_or(&self.transcript, |w| &w.transcript)
    }

    /// What the shown transcript's rows read: its file tools' diffs and the
    /// workers under its calls.
    pub fn shown_files(&self) -> &crate::files::Ledger {
        self.watch.focused().map_or(&self.stats.files, |w| &w.files)
    }

    pub fn shown_subagents(&self) -> &crate::subagents::Subagents {
        self.watch.focused().map_or(&self.stats.subagents, |w| &w.subagents)
    }

    pub fn shown_subagents_mut(&mut self) -> &mut crate::subagents::Subagents {
        match self.watch.focused_mut() {
            Some(w) => &mut w.subagents,
            None => &mut self.stats.subagents,
        }
    }

    /// The tray's running background workers, as [`Watch::step`] takes
    /// them.
    pub fn watchable(&self) -> Vec<Candidate> {
        self.stats
            .subagents
            .live()
            .into_iter()
            // A worker only started (no live word yet) too: its view says
            // if it is gone.
            .filter(|l| l.watchable && !l.status.terminal())
            .map(|l| Candidate::of(l.id, &l.name))
            .collect()
    }

    /// The session's folder (a worker's diffs are shown relative to it).
    pub fn cwd(&self) -> Option<&str> {
        self.stats.cwd.as_deref()
    }

    /// The session asked for last (attached, or on its way).
    pub fn wanted_session(&self) -> Option<&str> {
        self.wanted_session.as_deref()
    }

    /// The attachment input goes to: the session on screen, attached and
    /// the one asked for last (what [`Live::can_send`] checks). Every send
    /// names its attachment (`client.on(seq)`, split panes §6.4).
    pub fn attachment(&self) -> Option<u64> {
        self.active.as_ref().map(|a| a.attach_seq).filter(|&s| self.wanted == Some(s))
    }

    /// The attachment asked for last (attached or on its way): where a
    /// worker poll or a detach goes.
    pub fn wanted(&self) -> Option<u64> {
        self.wanted
    }

    /// Attached, connected and allowed to drive, and no switch under way:
    /// while another attachment is on its way (`wanted` is not the active
    /// one), input, answers and HALT must not go (they are addressed to
    /// the attachment on screen, [`Live::attachment`]).
    pub fn can_send(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.mode != AttachMode::Observe && self.wanted == Some(a.attach_seq)) && !self.reattaching
    }

    /// Whether this window may send the session on screen input (steer or
    /// cancel its workers), else the reason that applies: the daemon's
    /// rule (`exclusive`: the input owner only; `shared`: every client but
    /// an observer), and no reconnect or switch under way.
    pub fn input_gate(&self) -> Result<(), &'static str> {
        let Some(a) = self.active.as_ref() else { return Err("not attached") };
        if self.reattaching {
            return Err("reconnecting to the session: try again in a moment");
        }
        if self.switching() {
            return Err("switching sessions: try again in a moment");
        }
        if a.mode == AttachMode::Observe {
            return Err("read-only: this window observes the session");
        }
        if !self.can_send() {
            return Err("not connected to the session: try again in a moment");
        }
        if a.input == ClientInput::Exclusive && a.owner != Some(a.client) {
            return Err("read-only here: another client owns this session's input");
        }
        Ok(())
    }

    /// A switch to another attachment is under way.
    pub fn switching(&self) -> bool {
        self.wanted.is_some() && self.wanted != self.active.as_ref().map(|a| a.attach_seq)
    }

    /// The prompt the authorization card shows, as `(epoch, prompt id)`:
    /// the attached session's first pending one.
    pub fn card_key(&self) -> Option<(u64, u64)> {
        Some((self.active.as_ref()?.epoch, self.transcript.pending_prompts().first()?.id))
    }

    /// Prompt `id` of the session on screen may be answered now.
    pub fn may_answer(&self, prompt_id: u64) -> bool {
        self.can_send() && !self.prompt_answered(prompt_id) && self.transcript.pending_prompts().iter().any(|p| p.id == prompt_id)
    }

    /// An answer to prompt `id` of the session on screen went out.
    pub fn prompt_answered(&self, prompt_id: u64) -> bool {
        self.active.as_ref().is_some_and(|a| self.answered.contains(&(a.epoch, prompt_id)))
    }

    /// The answer to prompt `id` went to the daemon.
    pub fn answer_sent(&mut self, prompt_id: u64) {
        if let Some(a) = &self.active {
            self.answered.insert((a.epoch, prompt_id));
        }
    }

    /// `text` went to the daemon. A turn the user sent and then left
    /// running is still news when it ends off screen (the reply is what
    /// they are waiting for); one they watched to its end is in `seen`.
    #[cfg(test)]
    pub fn sent(&mut self, pane: PaneId, text: String) {
        self.in_flight.push_back(pane, text);
    }

    /// The user's `body` went to the daemon as `wire` (the notices that
    /// rode it after it, D6b): its echo is `wire`'s, and what goes back to
    /// the pane is `body`.
    pub(crate) fn sent_as(&mut self, pane: PaneId, body: String, wire: String) {
        self.in_flight.0.push_back(Input { to: Target::Pane(pane), body, wire: Ok(wire) });
    }

    /// The user's `body` waits for its notices' marks (D6b's ticket
    /// `ticket`): out of the composer, not sent yet ([`InFlight::went`]).
    pub(crate) fn waiting(&mut self, pane: PaneId, body: String, ticket: u64) {
        self.in_flight.0.push_back(Input { to: Target::Pane(pane), body, wire: Err(ticket) });
    }

    /// Worker `handle_id`'s view, just polled, tells the attached
    /// session's list what its pages say: one state per worker, whichever
    /// source heard it first ([`crate::subagents::Subagents::heard_page`]).
    /// Only a view of the worker the handle names now (a cold-started
    /// daemon numbers from 1 again: a stale view says nothing of the new
    /// one), and not one gone (offline is not how it ended). Whether the
    /// list changed.
    pub fn heard_view(&mut self, handle_id: &str, word: &str) -> bool {
        let Some(id) = handle_id.strip_prefix("sa_").and_then(|n| n.parse::<u64>().ok()) else { return false };
        let Some(w) = self.watch.worker(handle_id).filter(|w| w.gone.is_none()) else { return false };
        if self.stats.subagents.ident(id).is_some_and(|now| !crate::wtabs::same_worker(w.key(), now)) {
            return false;
        }
        let Some(status) = crate::subagents::Status::word(word) else { return false };
        self.stats.subagents.heard_page(id, status)
    }

    /// Its pane shows nothing now (the last tab closed): the attachment
    /// to detach, once. Not [`Live::wanted`]: the switch out of the tab
    /// closed cleared that, and the attachment is still open.
    pub(crate) fn release(&mut self) -> Option<u64> {
        self.held.take()
    }

    pub(crate) fn current(&self, attach_seq: u64) -> bool {
        self.wanted == Some(attach_seq)
    }

    /// An agent card's answer `text` went to the daemon (synaps-cards
    /// §3.5): its echo or refusal is its card's, never a composer's.
    pub(crate) fn card_sent(&mut self, card: cards_spec::CardId, text: String) {
        self.in_flight.push(Target::Card(card), text);
    }

    /// A refusal: the oldest unconfirmed text was refused (the daemon
    /// handles input in order, and earlier echoes confirmed the rest).
    /// A composer's goes back to it; a card's is the card's: `true` (its
    /// memo hears the refusal, [`crate::deliver`]).
    pub(crate) fn give_back_oldest(&mut self, effects: &mut Vec<Effect>) -> bool {
        match self.in_flight.pop_front() {
            Some(Input { to: Target::Pane(pane), body, .. }) => {
                effects.push(Effect::Restore(pane, body));
                false
            }
            Some(Input { to: Target::Card(_), .. }) => true,
            None => false,
        }
    }

    /// A detach: every composer's text back to it. A card's answer stays
    /// its card's: its memo settles it when the session is back
    /// (`Attached`), or says it couldn't tell (synaps-cards §3.5).
    pub(crate) fn give_back_all(&mut self, effects: &mut Vec<Effect>) {
        // One text per pane, in the order sent: each to its own composer.
        let all = self.in_flight.take();
        let mut panes: Vec<PaneId> = Vec::new();
        for i in &all {
            if let Target::Pane(p) = &i.to
                && !panes.contains(p)
            {
                panes.push(*p);
            }
        }
        for p in panes {
            let texts: Vec<&str> = all.iter().filter(|i| i.to == Target::Pane(p)).map(|i| i.body.as_str()).collect();
            effects.push(Effect::Restore(p, texts.join("\n")));
        }
    }
}

/// Add splice `sp` to the changes `pending` a list has not synced yet:
/// one splice that rebuilds what both would (split/architecture.md §3.2).
/// `sp` was taken after `pending`'s (its `old_len` is `pending`'s
/// `new_len`): from the first index either changed, the old length
/// before both, the new length after both.
pub(crate) fn pend(pending: &mut Option<Splice>, sp: Splice) {
    *pending = Some(match pending.take() {
        None => sp,
        Some(p) => {
            debug_assert_eq!(p.new_len, sp.old_len, "splices taken out of order");
            Splice { start: p.start.min(sp.start), old_len: p.old_len, new_len: sp.new_len }
        }
    });
}

/// What sent a text in flight, and so gets it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A pane's composer.
    Pane(PaneId),
    /// An agent card of the store's session (synaps-cards §3.5): its
    /// answer is never given to a composer.
    Card(cards_spec::CardId),
}

/// One input in flight: what sent it, the user's text (what goes back to
/// it), and what the daemon echoes once it went: the text as sent (the
/// notices that rode it after the body, D6b), or, while it waits for their
/// marks to be written, its ticket (not sent yet, [`InFlight::went`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Input {
    pub(crate) to: Target,
    pub(crate) body: String,
    pub(crate) wire: Result<String, u64>,
}

impl Input {
    /// Whether the daemon's echo `text` is this one's (its text as sent).
    pub(crate) fn echoed(&self, text: &str) -> bool {
        self.wire.as_ref().is_ok_and(|w| w.trim() == text.trim())
    }
}

/// Input sent and not yet confirmed, oldest first, each with what sent it
/// (split/architecture.md §6.5): a refusal or a detach gives it back to
/// that pane's composer, never another's, or to its card. Its echo
/// confirms it by the text as sent ([`Input::echoed`]), never the body
/// alone.
#[derive(Default, Debug)]
pub(crate) struct InFlight(VecDeque<Input>);

impl InFlight {
    /// The texts, oldest first (tests).
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|i| &i.body)
    }

    #[cfg(test)]
    pub(crate) fn push_back(&mut self, pane: PaneId, text: String) {
        self.push(Target::Pane(pane), text);
    }

    pub(crate) fn push(&mut self, target: Target, text: String) {
        self.0.push_back(Input { to: target, body: text.clone(), wire: Ok(text) });
    }

    /// The oldest sent (a refusal's: the daemon handles input in order;
    /// one still waiting for its marks wasn't sent).
    pub(crate) fn pop_front(&mut self) -> Option<Input> {
        let i = self.0.iter().position(|i| i.wire.is_ok())?;
        self.0.remove(i)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The daemon echoed `text`: the oldest send of it is confirmed.
    pub(crate) fn confirm(&mut self, text: &str) {
        if let Some(i) = self.0.iter().position(|i| i.echoed(text)) {
            self.0.remove(i);
        }
    }

    /// The input waiting on `ticket` went as `wire` (its echo's text now),
    /// or wasn't sent (`None`): no longer in flight, what sent it and its
    /// text, to give back.
    pub(crate) fn went(&mut self, ticket: u64, wire: Option<String>) -> Option<(Target, String)> {
        let i = self.0.iter().position(|i| i.wire == Err(ticket))?;
        match wire {
            Some(w) => {
                self.0[i].wire = Ok(w);
                None
            }
            None => self.0.remove(i).map(|i| (i.to, i.body)),
        }
    }

    /// Pane `from` left the store (closed, or on another session now):
    /// what it sent is pane `to`'s (Shady #36 L5, r2 L-c).
    pub(crate) fn rehome(&mut self, from: PaneId, to: PaneId) {
        for i in self.0.iter_mut().filter(|i| i.to == Target::Pane(from)) {
            i.to = Target::Pane(to);
        }
    }

    /// Everything, oldest first; none left.
    pub(crate) fn take(&mut self) -> Vec<Input> {
        std::mem::take(&mut self.0).into()
    }
}

/// The window's live stores (split/architecture.md §3.3). Step 2: exactly
/// one, [`LiveId::FIRST`], for the window's life.
pub struct Lives {
    slots: Vec<(LiveId, Live)>,
    /// The store the visible tab is shown from (the focused pane's): the
    /// one whose attach the tab strip follows (`Tabs::attached`). Step 2:
    /// [`LiveId::FIRST`], always; step 3's focus moves it.
    shown: LiveId,
    /// The last id handed out: ids are never reused (#31 L4), so an id
    /// kept past its store's release can never name another store.
    last: u16,
}

impl Default for Lives {
    fn default() -> Self {
        Lives { slots: vec![(LiveId::FIRST, Live::default())], shown: LiveId::FIRST, last: LiveId::FIRST.0 }
    }
}

impl Lives {
    /// The store the visible tab is shown from ([`Lives::shown`]'s field).
    pub(crate) fn shown(&self) -> LiveId {
        self.shown
    }

    /// The pane acted on shows `id` now (split panes step 3: the store
    /// whose attach its tab strip follows is that pane's).
    pub(crate) fn show(&mut self, id: LiveId) {
        self.shown = id;
    }

    /// A store no pane shows any more goes (its attachment detached by the
    /// caller). The first store stays for the window's life.
    pub(crate) fn remove(&mut self, id: LiveId) {
        if id != LiveId::FIRST {
            self.slots.retain(|(i, _)| *i != id);
        }
    }

    /// The store whose session is `session_id` (attached, or asked for
    /// last), other than `but`: a pane switching to it shares it.
    pub(crate) fn of_session(&self, session_id: &str, but: LiveId) -> Option<LiveId> {
        self.slots.iter().find(|(i, l)| *i != but && (l.is_active(session_id) || l.is_wanted(session_id))).map(|(i, _)| *i)
    }

    /// The store whose attachment is `attach_seq` (the one it asked for
    /// last): where that attachment's events go.
    pub(crate) fn wanting(&self, attach_seq: u64) -> Option<LiveId> {
        self.slots.iter().find(|(_, l)| l.wanted == Some(attach_seq)).map(|(id, _)| *id)
    }

    /// The store attached to `session_id`, if any.
    pub(crate) fn on(&self, session_id: &str) -> Option<&Live> {
        self.iter().find(|l| l.is_active(session_id))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Live> {
        self.slots.iter().map(|(_, l)| l)
    }

    /// Store `id`, if it still exists (a job that outlived its pane).
    /// Store `id`, if it is still there.
    pub(crate) fn get(&self, id: LiveId) -> Option<&Live> {
        self.slots.iter().find(|(i, _)| *i == id).map(|(_, l)| l)
    }

    pub(crate) fn get_mut(&mut self, id: LiveId) -> Option<&mut Live> {
        self.slots.iter_mut().find(|(i, _)| *i == id).map(|(_, l)| l)
    }

    /// Every store's id, oldest first.
    pub(crate) fn ids(&self) -> Vec<LiveId> {
        self.slots.iter().map(|(id, _)| *id).collect()
    }

    /// A second store, as step 3's multi-live activation will make one
    /// (tests: two sessions attached at once; `--bench-tabs`' `mem_lives`).
    pub(crate) fn add(&mut self) -> LiveId {
        self.last += 1;
        let id = LiveId(self.last);
        self.slots.push((id, Live::default()));
        id
    }
}

/// A store that exists: a pane's (a pane's store is released only after
/// its last pane goes; only panes keep a `LiveId`). Ids are never reused,
/// so a slip panics here rather than reach another session's store.
impl std::ops::Index<LiveId> for Lives {
    type Output = Live;
    fn index(&self, id: LiveId) -> &Live {
        &self.slots.iter().find(|(i, _)| *i == id).expect("a live store that exists").1
    }
}

impl std::ops::IndexMut<LiveId> for Lives {
    fn index_mut(&mut self, id: LiveId) -> &mut Live {
        &mut self.slots.iter_mut().find(|(i, _)| *i == id).expect("a live store that exists").1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transcript's splice contract (`Transcript::take_splice`): the
    /// first changed index (clamped to what was clean), the old length and
    /// the new one.
    struct Doc {
        items: Vec<u32>,
        dirty_from: Option<usize>,
        clean_len: usize,
    }

    impl Doc {
        fn touch(&mut self, ix: usize) {
            self.dirty_from = Some(self.dirty_from.map_or(ix, |d| d.min(ix)));
        }
        fn take_splice(&mut self) -> Option<Splice> {
            let start = self.dirty_from.take()?;
            let s = Splice { start: start.min(self.clean_len), old_len: self.clean_len, new_len: self.items.len() };
            self.clean_len = self.items.len();
            Some(s)
        }
    }

    fn apply(list: &mut Vec<u32>, items: &[u32], sp: &Splice) {
        assert_eq!(list.len(), sp.old_len, "a list is at its splice's old length");
        list.truncate(sp.start);
        list.extend_from_slice(&items[sp.start..sp.new_len]);
    }

    /// The merged splice rebuilds a list exactly as taking each one would
    /// (split/architecture.md §7 step 2): a list that syncs every frame
    /// and one parked for many frames (its splices merged with `pend`)
    /// end up the same as the transcript, through appends, edits in the
    /// middle, a trimmed front and a whole replace.
    #[test]
    fn a_merged_splice_rebuilds_what_each_would() {
        let mut seed = 0x5eed_u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for _round in 0..200 {
            let mut doc = Doc { items: Vec::new(), dirty_from: None, clean_len: 0 };
            let (mut every, mut parked, mut pending) = (Vec::new(), Vec::new(), None);
            for step in 0..60u32 {
                match rnd(6) {
                    0 | 1 => {
                        doc.items.push(step);
                        doc.touch(doc.items.len() - 1);
                    }
                    2 if !doc.items.is_empty() => {
                        let i = rnd(doc.items.len() as u64) as usize;
                        doc.items[i] = 1000 + step;
                        doc.touch(i);
                    }
                    3 if !doc.items.is_empty() => {
                        let k = rnd(doc.items.len() as u64) as usize;
                        doc.items.truncate(k);
                        doc.touch(k);
                    }
                    4 => {
                        doc.items = (0..rnd(8) as u32).map(|i| 5000 + step * 10 + i).collect();
                        doc.touch(0);
                    }
                    _ => {}
                }
                if rnd(3) == 0
                    && let Some(sp) = doc.take_splice()
                {
                    apply(&mut every, &doc.items, &sp);
                    pend(&mut pending, sp);
                }
                // The parked list comes back: as `sync_list`, the store's
                // splice is fanned out first, then its pending applied.
                if rnd(7) == 0 {
                    if let Some(sp) = doc.take_splice() {
                        apply(&mut every, &doc.items, &sp);
                        pend(&mut pending, sp);
                    }
                    if let Some(sp) = pending.take() {
                        apply(&mut parked, &doc.items, &sp);
                    }
                }
            }
            if let Some(sp) = doc.take_splice() {
                apply(&mut every, &doc.items, &sp);
                pend(&mut pending, sp);
            }
            if let Some(sp) = pending.take() {
                apply(&mut parked, &doc.items, &sp);
            }
            assert_eq!(every, doc.items);
            assert_eq!(parked, doc.items);
        }
    }

    /// #31 L4: a released store's id is never handed out again.
    #[test]
    fn a_released_stores_id_is_never_reused() {
        let mut l = Lives::default();
        let a = l.add();
        l.remove(a);
        let b = l.add();
        assert_ne!(a, b, "a new id");
        assert_eq!(l.ids(), [LiveId::FIRST, b]);
    }

    /// Input from two panes on one session: a refusal gives the oldest
    /// back to the pane that sent it; a detach gives each pane its own
    /// texts, in order, never the other's.
    #[test]
    fn input_in_flight_goes_back_to_the_pane_that_sent_it() {
        let (a, b) = (PaneId::FIRST, PaneId::nth(1));
        let mut l = Live::default();
        l.sent(b, "from b".into());
        l.sent(a, "one from a".into());
        l.sent(a, "two from a".into());
        l.sent(b, "again b".into());
        let mut fx = Vec::new();
        l.give_back_oldest(&mut fx);
        assert_eq!(fx, vec![Effect::Restore(b, "from b".into())]);
        l.in_flight.confirm("two from a");
        let mut fx = Vec::new();
        l.give_back_all(&mut fx);
        assert_eq!(fx, vec![Effect::Restore(a, "one from a".into()), Effect::Restore(b, "again b".into())]);
        assert!(l.in_flight.is_empty());
    }

    /// D6b (Shady #103 R3-P2-1): an input is confirmed by its text as
    /// sent (its notices' trailer too), never its body alone; one waiting
    /// for its marks isn't sent, so a refusal is the oldest sent's, and one
    /// that couldn't go is given back.
    #[test]
    fn input_is_confirmed_by_its_text_as_sent() {
        let a = PaneId::FIRST;
        let mut l = Live::default();
        l.waiting(a, "later".into(), 7);
        l.sent_as(a, "now".into(), "now\n[block plan v1 dismissed]".into());
        l.in_flight.confirm("now");
        assert_eq!(l.in_flight.iter().count(), 2, "the body alone isn't its echo");
        let mut fx = Vec::new();
        l.give_back_oldest(&mut fx);
        assert_eq!(fx, vec![Effect::Restore(a, "now".into())], "refused: the oldest sent, never one waiting");
        assert_eq!(l.in_flight.went(7, Some("later\n[block x v2 shown]".into())), None, "it went");
        l.in_flight.confirm("later");
        assert_eq!(l.in_flight.iter().count(), 1);
        l.in_flight.confirm("later\n[block x v2 shown]");
        assert!(l.in_flight.is_empty(), "its echo, as sent");
        l.waiting(a, "lost".into(), 8);
        assert_eq!(l.in_flight.went(8, None), Some((Target::Pane(a), "lost".into())), "not sent: back to its pane");
        assert!(l.in_flight.is_empty());
    }
}
