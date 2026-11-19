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

use synaps_client::{AttachMode, ClientInput, Transcript};

#[cfg(doc)]
use crate::model::Model;
use crate::model::{Active, Effect};
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
    /// given back to the user on a refusal (the oldest) or a detach (all).
    pub(crate) in_flight: VecDeque<String>,
    /// The attached session's numbers (the composer's foot, the right-hand
    /// panel).
    pub stats: Stats,
    /// The attached session's worker views ([`crate::watch`]): with the
    /// transcript, they go to the tab cache when its tab is left.
    pub watch: Watch,
    /// Worker cancels armed and on their way ([`crate::wtabs`]).
    pub cancels: crate::wtabs::Cancels,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            active: None,
            wanted: None,
            wanted_session: None,
            wanted_fact: None,
            answered: HashSet::new(),
            transcript: crate::model::transcript(),
            last_error: None,
            reattaching: false,
            in_flight: VecDeque::new(),
            stats: Stats::default(),
            watch: Watch::default(),
            cancels: Default::default(),
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

    pub fn shown_mut(&mut self) -> &mut Transcript {
        match self.watch.focused_mut() {
            Some(w) => &mut w.transcript,
            None => &mut self.transcript,
        }
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
    pub fn sent(&mut self, text: String) {
        self.in_flight.push_back(text);
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

    pub(crate) fn current(&self, attach_seq: u64) -> bool {
        self.wanted == Some(attach_seq)
    }

    /// A refusal: the oldest unconfirmed text was refused (the daemon
    /// handles input in order, and earlier echoes confirmed the rest).
    pub(crate) fn give_back_oldest(&mut self, effects: &mut Vec<Effect>) {
        if let Some(t) = self.in_flight.pop_front() {
            effects.push(Effect::Restore(t));
        }
    }

    pub(crate) fn give_back_all(&mut self, effects: &mut Vec<Effect>) {
        if !self.in_flight.is_empty() {
            effects.push(Effect::Restore(Vec::from(std::mem::take(&mut self.in_flight)).join("\n")));
        }
    }
}

/// The window's live stores (split/architecture.md §3.3). Step 2: exactly
/// one, [`LiveId::FIRST`], for the window's life.
pub struct Lives {
    slots: Vec<(LiveId, Live)>,
}

impl Default for Lives {
    fn default() -> Self {
        Lives { slots: vec![(LiveId::FIRST, Live::default())] }
    }
}

impl Lives {
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

    /// The one store, for what no attachment names (an error of the
    /// control connection, a seq-less refusal). Step 2 has exactly one;
    /// step 3 must decide where such events go, so this says so loudly.
    pub(crate) fn sole(&self) -> LiveId {
        debug_assert_eq!(self.slots.len(), 1, "an event that names no attachment, with more than one live store");
        self.slots[0].0
    }
}

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
