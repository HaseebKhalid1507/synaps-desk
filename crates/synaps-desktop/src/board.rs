//! The board's work, off the UI frame (synaps-cards `docs/blocks.md` v1.3,
//! §8.2's D5 row; `cards_spec::board`'s adapter guarantees 6 and 7).
//!
//! B2's fold, overlay and bound prefix run to their end, so the frame never
//! runs them. Each session store ([`crate::live::Live`]) has one
//! [`Work`], at most one job at a time on its transcript's history's
//! [`Worker`] (the history's: it keeps the fold across jobs, and goes
//! where the transcript goes).
//! - **A store let go of its transcript** (a switch away, a close, a
//!   store reused or dropped) lets go of its work: the job stops, a board
//!   waiting is dropped, and so are re-folds for it ([`AppView::board_out`]).
//! - **Live events are coalesced.** A frame asks for a board only when
//!   the transcript's input revision moved
//!   ([`synaps_client::Transcript::blocks_rev`]), once for everything that
//!   landed since the last frame.
//! - **A job runs in steps** on the background executor, each at most
//!   [`STEP`] (the 30 ms clock) of the history's fold, a slice at a time
//!   ([`synaps_client::blocks::SLICE`]): a long history yields between
//!   them, and the worker keeps what it read.
//! - **A newer input drops the job.** Its stop flag is set and its task
//!   dropped. The step running stops at its next slice, keeping what it
//!   read, and the next job reads on from there. A job of another history
//!   (a clear, a compaction, another session) starts over: the epoch.
//! - **Only a finished board is published**, one board for one input, and
//!   only onto the history it was folded from ([`Drawn::epoch`]). Until it
//!   lands, the rows draw the last one.
//!
//! **Re-folds** ([`Refolds`], guarantee 6): the board at a past position (a
//! revision view, an evicted outcome: D7 and D8 ask for them). One runs at
//! a time, app-wide (§1.8's one scratch): a request while one runs stops
//! it and waits until it has ended. It runs off the frame in steps of
//! [`REFOLD_STEP`] visits, the 30 ms clock checked between them, yielding
//! when it is up; `too far back to show` is its result like any other.
//! - **What it read** ([`Read`]) stamps it: a revision whose batch
//!   committed within the history reads that history only (the worker's
//!   journal, shared), and holds for every longer history of the epoch (it
//!   only grows); any other target reads the whole input, the live turn
//!   included, and holds for that input's revision only.
//! - **An input it no longer holds for** stops it, and runs the request
//!   again on the new one (it stands), or, done, drops what it found:
//!   nothing is reused over another input. Another history (an epoch)
//!   drops it, never finished, and its request with it; so does a request
//!   that changed (another target, the view closed).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cards_spec::board::{Event, Pos, Refold, State, Target, TooFar};
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Input, Worker};

use crate::app::AppView;
use crate::live::LiveId;

/// A step's budget: the 30 ms clock (§1.8).
pub(crate) const STEP: Duration = Duration::from_millis(30);

/// One session store's board work.
#[derive(Default)]
pub(crate) struct Work {
    /// The input revision asked for last.
    asked: Option<u64>,
    /// The running job's: set when a newer input supersedes it.
    stop: Arc<AtomicBool>,
    job: Option<Task<()>>,
    /// Steps run and boards published (tests, the bench).
    pub(crate) steps: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) published: u64,
    /// A finished board that waits for a pane's pointer lease to end
    /// ([`crate::lease`]).
    pub(crate) waiting: Option<Arc<Drawn>>,
    /// A step's budget, if not [`STEP`] (tests: none, a slice a step).
    #[cfg(test)]
    pub(crate) budget: Option<Duration>,
}

impl Work {
    /// Its transcript left (module docs): no job, no board waiting, nothing
    /// asked yet of the next one.
    fn clear(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.job = None;
        self.asked = None;
        self.waiting = None;
    }

    /// Its next frame asks for a board again, its input moved or not (D8:
    /// the bound board kept, or let go, from that job on).
    pub(crate) fn again(&mut self) {
        self.asked = None;
    }

    /// Its transcript's board, revision `rev`, is the one drawn: nothing to
    /// fold (a closing pane's rows).
    pub(crate) fn settled(&mut self, rev: u64) {
        self.asked = Some(rev);
    }

    /// Whether a job runs (tests).
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.job.is_some()
    }
}

// The re-fold slot's callers are D7's revision view and D8's outcomes;
// D5 builds and tests it (guarantee 6), so until then the app build
// doesn't call it.

/// A re-fold step's budget in visits ([`Refold::step`]): small, so the
/// 30 ms clock is checked often.
pub(crate) const REFOLD_STEP: usize = 2_048;

/// What a re-fold read (module docs): the input it holds for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    /// History `epoch`'s first `len` events, the target's batch committed
    /// in them: every longer history of the epoch reads the same there.
    Prefix { epoch: u64, len: usize },
    /// The whole input at revision `rev` (the transcript's `blocks_rev`).
    Whole { rev: u64 },
}

impl Read {
    /// What a re-fold of `target` reads of `input` (revision `rev`).
    fn of(target: Target, input: &Input, rev: u64) -> Self {
        match target {
            Target::Revision(Pos { event, .. }) if event < input.history_len && !input.history_open => {
                Read::Prefix { epoch: input.epoch, len: input.history_len }
            }
            _ => Read::Whole { rev },
        }
    }

    /// It holds for the input of history `epoch`, `len` events long, at
    /// revision `rev`.
    fn holds(self, epoch: u64, len: usize, rev: u64) -> bool {
        match self {
            Read::Prefix { epoch: e, len: l } => e == epoch && len >= l,
            Read::Whole { rev: r } => r == rev,
        }
    }
}

/// The re-fold slot (module docs): one running, the next request waiting.
#[derive(Default)]
pub(crate) struct Refolds {
    running: Option<Running>,
    /// The next request, run once the running one has ended.
    next: Option<(LiveId, Target)>,
    /// The last one finished, while its request stands.
    pub(crate) done: Option<Refolded>,
    /// Re-folds started (tests).
    pub(crate) started: u64,
}

struct Running {
    live: LiveId,
    target: Target,
    epoch: u64,
    read: Read,
    stop: Arc<AtomicBool>,
    _task: Task<()>,
}

/// A finished re-fold: store `live`'s board at `target`, of the input it
/// read (`read`). `Ok(None)`: the target never took effect (its call
/// didn't bind, or it is pending).
#[derive(Clone, Debug)]
pub(crate) struct Refolded {
    pub(crate) live: LiveId,
    pub(crate) target: Target,
    pub(crate) read: Read,
    pub(crate) state: Result<Option<Arc<State>>, TooFar>,
    /// Its work, in visits ([`Refold::visited`]): the tests' measure.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) visited: usize,
}

/// What one turn of a re-fold came to, `budget` of the clock spent.
#[derive(Debug)]
pub(crate) enum Chunk {
    Done(Result<Option<Box<State>>, TooFar>),
    Paused,
    Stopped,
}

/// Steps of [`REFOLD_STEP`] visits until it is done, stopped or `budget`
/// is up (one step at least).
pub(crate) fn chunk(rf: &mut Refold, stop: &AtomicBool, budget: Duration) -> Chunk {
    let start = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Chunk::Stopped;
        }
        match rf.step(REFOLD_STEP) {
            // Taken out at once: the re-fold is dropped right after, so two
            // boards are kept only for that moment.
            Ok(true) => return Chunk::Done(Ok(rf.state().cloned().map(Box::new))),
            Err(far) => return Chunk::Done(Err(far)),
            Ok(false) if start.elapsed() >= budget => return Chunk::Paused,
            Ok(false) => {}
        }
    }
}

/// A finished re-fold's board (none: the target never took effect) and
/// its work in visits; `None` if it was stopped.
type Folded = (Result<Option<Box<State>>, TooFar>, usize);

/// A re-fold of `events` to `target`, run to its end or its stop, the
/// executor given a turn each time the clock is up.
async fn refold(
    events: Arc<Vec<Event>>,
    generation: cards_spec::block::Generation,
    target: Target,
    stop: Arc<AtomicBool>,
) -> Option<Folded> {
    let mut rf = match Refold::to(generation, &events, target) {
        Ok(rf) => rf,
        Err(far) => return Some((Err(far), 0)),
    };
    loop {
        match chunk(&mut rf, &stop, STEP) {
            Chunk::Done(out) => return Some((out, rf.visited())),
            Chunk::Stopped => return None,
            Chunk::Paused => yield_now().await,
        }
    }
}

/// One turn given back to the executor (every platform: `futures-lite`
/// is the desk's on Linux only).
async fn yield_now() {
    let mut once = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut once, true) {
            std::task::Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await
}

/// The events a re-fold of `input` reads in full, its history's journal
/// then the live turn's (tests: the reference).
#[cfg(test)]
pub(crate) fn events(input: &Input) -> Vec<Event> {
    let journal = input.worker.lock().unwrap_or_else(PoisonError::into_inner).journal(input).expect("no gap");
    journal.iter().cloned().chain(input.live_events().expect("the input holds")).collect()
}

/// What one step came to.
#[derive(Debug)]
pub(crate) enum Step {
    Done(Arc<Drawn>),
    /// Its time was up: the next step reads on.
    Paused,
    /// A newer input superseded it.
    Stopped,
}

/// One step of `input`'s job: the history's fold read on for at most
/// `budget` (a slice at least), then, once it is all read, the overlay and
/// the bound prefix.
pub(crate) fn step(worker: &Mutex<Worker>, input: &Input, stop: &AtomicBool, budget: Duration) -> Step {
    let mut w = worker.lock().unwrap_or_else(PoisonError::into_inner);
    if stop.load(Ordering::Relaxed) {
        return Step::Stopped;
    }
    let start = Instant::now();
    let mut paused = false;
    let out = w.run(input, &mut || {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        paused = start.elapsed() >= budget;
        !paused
    });
    match out {
        Some(d) => Step::Done(Arc::new(d)),
        None if paused => Step::Paused,
        None => Step::Stopped,
    }
}

impl AppView {
    /// Ask for store `live`'s board if its input moved since the last ask:
    /// the job before is dropped, and a new one runs in steps off the frame
    /// (module docs). With no block call there is no board.
    pub(crate) fn sync_board(&mut self, live: LiveId, cx: &mut Context<Self>) {
        let l = &mut self.model.lives[live];
        let rev = l.transcript.blocks_rev();
        if l.board.asked == Some(rev) {
            return;
        }
        l.board.asked = Some(rev);
        l.board.stop.store(true, Ordering::Relaxed);
        l.board.job = None;
        // A re-fold of an input that moved on: stopped and run again, or,
        // of another history, dropped (module docs).
        let (epoch, len) = (l.transcript.blocks_epoch(), l.transcript.blocks_len());
        let r = &mut self.refolds;
        if let Some(j) = r.running.as_ref().filter(|j| j.live == live && !j.read.holds(epoch, len, rev)) {
            j.stop.store(true, Ordering::Relaxed);
            if j.epoch != epoch {
                r.next = r.next.filter(|n| n.0 != live);
            } else if r.next.is_none() {
                r.next = Some((live, j.target));
            }
        }
        if r.done.as_ref().is_some_and(|d| d.live == live && !d.read.holds(epoch, len, rev)) {
            r.done = None;
        }
        let l = &mut self.model.lives[live];
        let Some(input) = l.transcript.blocks_input() else {
            l.board.waiting = None;
            if l.transcript.drawn().is_some() {
                l.transcript.set_drawn(None);
            }
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        l.board.stop = stop.clone();
        let (worker, steps, input) = (input.worker.clone(), l.board.steps.clone(), Arc::new(input));
        #[cfg(test)]
        let budget = l.board.budget.unwrap_or(STEP);
        #[cfg(not(test))]
        let budget = STEP;
        l.board.job = Some(cx.spawn(async move |this, cx| {
            loop {
                let (w, s, i, n) = (worker.clone(), stop.clone(), input.clone(), steps.clone());
                let out = cx
                    .background_spawn(async move {
                        n.fetch_add(1, Ordering::Relaxed);
                        step(&w, &i, &s, budget)
                    })
                    .await;
                match out {
                    Step::Done(d) => {
                        let _ = this.update(cx, |v, cx| v.board_landed(live, rev, d, cx));
                        return;
                    }
                    Step::Paused => {}
                    Step::Stopped => return,
                }
            }
        }));
    }

    /// Store `live`'s board at `target` (module docs): a re-fold, run when
    /// the slot is free; the one running for another request is stopped.
    /// Asked again while it runs or once done, nothing new.
    pub(crate) fn refold(&mut self, live: LiveId, target: Target, cx: &mut Context<Self>) {
        let t = &self.model.lives[live].transcript;
        let now = (t.blocks_epoch(), t.blocks_len(), t.blocks_rev());
        let holds = |read: Read| read.holds(now.0, now.1, now.2);
        let r = &mut self.refolds;
        if r.done.as_ref().is_some_and(|d| (d.live, d.target) == (live, target) && holds(d.read)) {
            return;
        }
        r.done = None;
        match &r.running {
            Some(j) if (j.live, j.target) == (live, target) && holds(j.read) && !j.stop.load(Ordering::Relaxed) => {
                r.next = None;
            }
            Some(j) => {
                j.stop.store(true, Ordering::Relaxed);
                r.next = Some((live, target));
            }
            None => self.refold_start(live, target, cx),
        }
    }

    /// The re-fold's request withdrawn (its view closed): stopped, and
    /// nothing kept.
    pub(crate) fn refold_cancel(&mut self) {
        let r = &mut self.refolds;
        r.running.as_ref().inspect(|j| j.stop.store(true, Ordering::Relaxed));
        r.next = None;
        r.done = None;
    }

    fn refold_start(&mut self, live: LiveId, target: Target, cx: &mut Context<Self>) {
        let t = &self.model.lives[live].transcript;
        let rev = t.blocks_rev();
        let Some(input) = t.blocks_input() else {
            self.refolds.done = Some(Refolded { live, target, read: Read::Whole { rev }, state: Ok(None), visited: 0 });
            cx.notify();
            return;
        };
        let (epoch, generation, read) = (input.epoch, input.generation, Read::of(target, &input, rev));
        let worker = input.worker.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        self.refolds.started += 1;
        let task = cx.spawn(async move |this, cx| {
            let out = cx
                .background_spawn(async move {
                    // The history: the worker's journal, shared, not copied
                    // (a gap: nothing to read). The live turn after it: a
                    // copy, the re-fold's alone, while it runs.
                    let journal = worker.lock().unwrap_or_else(PoisonError::into_inner).journal(&input)?;
                    let events = match read {
                        // Its live turn read from its log (`None`: changed
                        // since, a newer input stands: nothing to read).
                        Read::Whole { .. } if input.live_len() > 0 => {
                            Arc::new(journal.iter().cloned().chain(input.live_events()?).collect())
                        }
                        _ => journal,
                    };
                    refold(events, generation, target, s).await
                })
                .await;
            let _ = this.update(cx, |v, cx| v.refold_ended(live, target, read, out, cx));
        });
        self.refolds.running = Some(Running { live, target, epoch, read, stop, _task: task });
    }

    /// A re-fold ended: kept if it finished, and its request stands and its
    /// input still holds (module docs); then the request that waited runs.
    fn refold_ended(&mut self, live: LiveId, target: Target, read: Read, out: Option<Folded>, cx: &mut Context<Self>) {
        let r = &mut self.refolds;
        let stopped = r.running.take().is_none_or(|j| j.stop.load(Ordering::Relaxed));
        let holds = self.model.lives.get(live).is_some_and(|l| {
            let t = &l.transcript;
            read.holds(t.blocks_epoch(), t.blocks_len(), t.blocks_rev())
        });
        if let (Some((state, visited)), false, true) = (out, stopped, holds) {
            r.done = Some(Refolded { live, target, read, state: state.map(|s| s.map(|s| Arc::new(*s))), visited });
            cx.notify();
        }
        if let Some((live, target)) = r.next.take() {
            self.refold(live, target, cx);
        }
    }

    /// A finished board for store `live`: drawn if its history is still the
    /// store's (the same epoch), else dropped.
    fn board_landed(&mut self, live: LiveId, rev: u64, drawn: Arc<Drawn>, cx: &mut Context<Self>) {
        let Some(l) = self.model.lives.get_mut(live) else { return };
        if l.board.asked == Some(rev) {
            l.board.job = None;
        }
        if l.transcript.blocks_epoch() != drawn.epoch {
            return;
        }
        // A pane's block state is of the history it was made on: another's
        // (another session, or a cleared or compacted one) starts over, as
        // row ids are of block names, which histories reuse. The same one
        // folded again (a resync, a tab shown again) keeps it.
        let of = crate::blocks::history_of(l.transcript.session_id(), &drawn);
        for p in self.panes.iter_mut().filter(|p| p.live == live && p.block_of != of) {
            p.blocks_over(of);
        }
        self.board_ready(live, drawn, cx);
    }

    /// Store `live` let go of its transcript (module docs): its job, the
    /// board waiting and its re-folds go.
    pub(crate) fn board_out(&mut self, live: LiveId) {
        if let Some(l) = self.model.lives.get_mut(live) {
            l.board.clear();
        }
        let r = &mut self.refolds;
        if let Some(j) = r.running.as_ref().filter(|j| j.live == live) {
            j.stop.store(true, Ordering::Relaxed);
        }
        r.next = r.next.filter(|n| n.0 != live);
        r.done = r.done.take().filter(|d| d.live != live);
    }
}

#[cfg(test)]
mod tests {
    use super::{Chunk, Duration, Input, REFOLD_STEP, Step, Worker, chunk, step};
    use cards_spec::board::{Board, Pos, REFOLD_MAX, Refold, Target, TooFar};
    use serde_json::Value;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use synaps_client::blocks::{HistoryBlocks, LiveLog};
    use synaps_client::synthetic::{blocks_history, history};

    fn input(h: &[Value]) -> Input {
        let mut b = HistoryBlocks::default();
        for m in h {
            b.message(m, &[]);
        }
        Input::of(&b, &LiveLog::default()).unwrap()
    }

    /// A job runs in steps of its budget, the worker reading on from where
    /// the last stopped; a stopped one keeps what it read; and the board
    /// it finishes is the one a single run gives.
    #[test]
    fn a_job_runs_in_steps_and_a_stopped_one_keeps_its_place() {
        let long: Vec<Value> = blocks_history().into_iter().chain(history(20_000, 9)).collect();
        let i = input(&long);
        let whole = Worker::default().run(&i, &mut || true).unwrap();
        let w = Mutex::new(Worker::default());
        let go = AtomicBool::new(false);
        let mut steps = 0;
        let done = loop {
            steps += 1;
            match step(&w, &i, &go, Duration::ZERO) {
                Step::Done(d) => break d,
                Step::Paused => assert!(w.lock().unwrap().position() > 0),
                Step::Stopped => panic!("not stopped"),
            }
        };
        assert!(steps > 2, "{steps}");
        assert_eq!(*done, whole);
        // Stopped: nothing published, its place kept.
        let w = Mutex::new(Worker::default());
        assert!(matches!(step(&w, &i, &go, Duration::ZERO), Step::Paused));
        let at = w.lock().unwrap().position();
        let stop = AtomicBool::new(true);
        assert!(matches!(step(&w, &i, &stop, Duration::MAX), Step::Stopped));
        assert_eq!(w.lock().unwrap().position(), at);
        assert!(matches!(step(&w, &i, &go, Duration::MAX), Step::Done(_)));
    }

    /// Guarantee 6: a re-fold runs in steps of [`REFOLD_STEP`] visits, the
    /// clock checked between them: with none left, one step and a pause;
    /// stopped, it ends without a board; run on, it finishes with the
    /// board a single run gives. Past [`REFOLD_MAX`] events it is too far
    /// back, as a result like any other.
    #[test]
    fn a_refold_runs_in_small_steps_under_the_clock() {
        let long: Vec<Value> = blocks_history().into_iter().chain(history(4_000, 9)).collect();
        let i = input(&long);
        let events = i.events();
        let at = Target::Revision(Pos::event(events.len() - 1));
        let mut whole = Refold::to(i.generation, &events, at).unwrap();
        while !whole.step(usize::MAX).unwrap() {}
        let mut rf = Refold::to(i.generation, &events, at).unwrap();
        let go = AtomicBool::new(false);
        let mut chunks = 0;
        let state = loop {
            let before = rf.visited();
            match chunk(&mut rf, &go, Duration::ZERO) {
                Chunk::Paused => assert_eq!(rf.visited() - before, REFOLD_STEP, "one step a chunk"),
                Chunk::Done(s) => break s,
                Chunk::Stopped => panic!("not stopped"),
            }
            chunks += 1;
        };
        assert!(chunks >= events.len() / REFOLD_STEP, "{chunks} chunks for {} events", events.len());
        assert_eq!(state.unwrap().as_deref(), whole.state());
        assert_eq!(whole.state(), Some(&Board::fold(i.generation, &events).bound(&events)));
        let mut rf = Refold::to(i.generation, &events, at).unwrap();
        assert!(matches!(chunk(&mut rf, &AtomicBool::new(true), Duration::MAX), Chunk::Stopped));
        assert_eq!(rf.visited(), 0, "stopped before a step");
        let many = vec![cards_spec::board::Event::User { text: String::new() }; REFOLD_MAX + 10];
        let mut rf = Refold::to(i.generation, &many, Target::Outcome(Pos::event(0)));
        assert!(rf.is_err() || matches!(chunk(rf.as_mut().unwrap(), &go, Duration::MAX), Chunk::Done(Err(TooFar))));
    }
}
