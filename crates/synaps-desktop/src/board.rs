//! The board's work, off the UI frame (synaps-cards `docs/blocks.md` v1.3,
//! §8.2's D5 row; `cards_spec::board`'s adapter guarantees 6 and 7).
//!
//! B2's fold, overlay and bound prefix run to their end, so the frame never
//! runs them. Each session store ([`crate::live::Live`]) has one
//! [`Work`]: a [`Worker`] that keeps the history's fold across jobs, and
//! at most one job at a time.
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use gpui_kit::*;
use synaps_client::blocks::{Drawn, Input, Worker};

use crate::app::AppView;
use crate::live::LiveId;

/// A step's budget: the 30 ms clock (§1.8).
pub(crate) const STEP: Duration = Duration::from_millis(30);

/// One session store's board work.
#[derive(Default)]
pub(crate) struct Work {
    worker: Arc<Mutex<Worker>>,
    /// The input revision asked for last.
    asked: Option<u64>,
    /// The running job's: set when a newer input supersedes it.
    stop: Arc<AtomicBool>,
    job: Option<Task<()>>,
    /// Steps run and boards published (tests, the bench).
    pub(crate) steps: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) published: u64,
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
        let Some(input) = l.transcript.blocks_input() else {
            if l.transcript.drawn().is_some() {
                l.transcript.set_drawn(None);
            }
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        l.board.stop = stop.clone();
        let (worker, steps, input) = (l.board.worker.clone(), l.board.steps.clone(), Arc::new(input));
        l.board.job = Some(cx.spawn(async move |this, cx| {
            loop {
                let (w, s, i, n) = (worker.clone(), stop.clone(), input.clone(), steps.clone());
                let out = cx
                    .background_spawn(async move {
                        n.fetch_add(1, Ordering::Relaxed);
                        step(&w, &i, &s, STEP)
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
        l.transcript.set_drawn(Some(drawn));
        l.board.published += 1;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Input, Step, Worker, step};
    use serde_json::Value;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use synaps_client::blocks::{HistoryBlocks, Live};
    use synaps_client::synthetic::{blocks_history, history};

    fn input(h: &[Value]) -> Input {
        let mut b = HistoryBlocks::default();
        for m in h {
            b.message(m, &[]);
        }
        Input::of(&b, Arc::new(Live::default())).unwrap()
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
}
