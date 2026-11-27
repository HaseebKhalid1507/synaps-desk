//! What a live turn's log lets go, dropped off the frame: a round's calls
//! when the round goes (a seal, a user message), a reset (its round, its
//! facts and their results, one unit), a log replaced or gone (its rounds
//! with its store), a store when its last reader lets go. Whoever lets go
//! (the frame, mostly) hands it to its log's [`Bin`] in O(1), and the
//! bin's [`Disposer`], one thread, drops it.
//!
//! **Owned.** The app owns its disposer ([`Disposer::start`]) and makes it
//! its thread's ([`Disposer::install`]): a log takes the bin its thread
//! has when it is made, and a log replaced, its predecessor's. The owner
//! dropped (or [`Disposer::shutdown`]) closes the queue: the thread drops
//! what it holds and ends, and the owner waits for it [`JOIN`] at most,
//! then lets it go on alone (detached). What a detached disposer still
//! holds at process exit is memory only (rounds, facts, strings, JSON
//! values: nothing that writes, sends or holds a resource outside the
//! process), so the exit reclaiming it loses nothing. A bin whose disposer
//! is closed, or never started (its thread could not be made: a degraded
//! mode, said once on stderr), drops what it is handed where it is.
//!
//! **Bounded, and never waited on.** A hand-over is admitted while the
//! bin's queue holds fewer than [`MAX_ITEMS`] items and what the bin holds
//! charged stays within [`MAX_BYTES`] with it; into an empty queue (no item
//! queued, so none of its bytes) it is admitted whatever it weighs (a
//! round or a store of over 64 MiB alone). What the queue holds is so
//! within [`MAX_BYTES`] and one item more, the one admitted into it empty.
//! Admission is two atomic updates and a send on an unbounded channel
//! whose length the cap bounds: nothing waits on the disposer.
//!
//! **Overload: dropped where it is let go.** This is the contract, an
//! accepted cost and not a defect (Haseeb's decision, #101 round 7). With
//! the queue saturated ([`MAX_ITEMS`] items queued, or [`MAX_BYTES`]
//! charged with an item queued), a unit handed over is refused, and
//! whoever lets go of it drops it there, as before the disposer (counted:
//! [`Backlog::in_place`]). On the frame that is one unit's whole
//! destruction (a round's calls, a store's facts): work in proportion to
//! the unit, never unbounded memory, and never a wait on the disposer.
//! Never under a store's lock either: what is dropped in place while this
//! thread holds a store's lock ([`locking`]) waits until the last lock it
//! holds is let go, then goes, in the order it was let go.
//!
//! **Charged until dropped.** What it holds is charged, heap included, as
//! asked of the allocator (its blocks' layouts: a JSON input's as it is
//! kept, rebuilt whole, [`super::canonical`] and [`super::value_bytes`],
//! checked against a counting allocator; the allocator's own per-block
//! bookkeeping is not counted, as nowhere in the app), from its hand-over
//! until the disposer has dropped all of it ([`Backlog::bytes`]): the
//! charge goes after the drop, never before, and it is part of what the app
//! retains besides its tabs' counts. Every owner retired on its own is
//! charged with all it keeps, its payload included: a round with its calls'
//! inputs, a reset with its round's and its facts', a store with its
//! facts', a log with its rounds' and its store's. A unit is retired whole
//! (a reset's round, facts and results together; a log's store moved into
//! its unit, never a second handle to it), so no owner of a payload
//! outlives the charge that covers it, the order the queue drops them in
//! aside. A store a job still reads when its log goes is charged from then
//! on ([`Bin::charge`], the charge kept with the store), until its last
//! reader lets go and the disposer drops it. That charge is taken whatever
//! the cap (the store is alive either way): it can pass [`MAX_BYTES`] by
//! those stores, one a job running at most, and nothing new is admitted
//! until it is back under. A job's batch of a store's handles
//! ([`super::COPY`] at most, for as long as it makes them events) is the
//! job's working set, not charged here. The estimates overcount on purpose
//! where an allocation is shared: a retired call's input is charged whether
//! or not a row still keeps it (a history landing keeps its rows), once by
//! each owner retired with it (a round's call and its fact keep one input:
//! a reset charges it twice), and a hash table is charged by hashbrown's
//! layout for its capacity (a kept JSON map, canonical, exactly). They
//! never undercount what the disposer holds.
//!
//! A thread's own share of destruction is counted ([`destroyed`]): a
//! hand-over is one element, whatever it holds; what a thread drops in
//! place, every element.
//!
//! **Tests' controls** are compiled only with the `test-support` feature
//! (off in every build the app ships; this crate's tests and the desk's
//! turn it on, as dev-dependencies): a test holds a disposer's thread
//! ([`Disposer::hold`]: it drops nothing more until a [`Hold::step`] or
//! the hold goes), waits for it ([`Disposer::flush`]), makes one whose
//! thread did not start ([`Disposer::failed`]), and watches this thread's
//! drops in place and hand-overs ([`on_in_place`], [`on_handed`]).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Condvar;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// The most bytes a bin holds charged (its queue's, and the stores jobs
/// still read after their logs went) for a hand-over to be admitted to a
/// queue that holds an item; into an empty one, any is (module docs).
pub const MAX_BYTES: usize = 64 << 20;

/// The most items a bin's queue holds.
pub const MAX_ITEMS: usize = 4_096;

/// How long a disposer's owner waits for its thread at shutdown, at most
/// (then the thread goes on alone, detached).
pub const JOIN: Duration = Duration::from_millis(250);

/// Something retired: what it is, the elements its drop destroys, and its
/// charge (released after the drop).
struct Retired {
    garbage: Box<dyn Any + Send>,
    elements: usize,
    charge: Charge,
}

enum Msg {
    Drop(Retired),
    /// Tests ([`Disposer::flush`]): answered once all before it is dropped
    /// and uncharged.
    #[cfg(any(test, feature = "test-support"))]
    Flush(SyncSender<()>),
}

/// A disposer's state, its bins' and its thread's.
#[derive(Default)]
struct Shared {
    /// The queue; `None` once closed (or never started).
    tx: Mutex<Option<Sender<Msg>>>,
    /// Items queued and the elements they hold.
    items: AtomicUsize,
    elements: AtomicUsize,
    /// Bytes charged: the queue's, and the stores charged where they are.
    bytes: AtomicUsize,
    /// Items dropped where they were let go (refused, or no thread).
    in_place: AtomicUsize,
    /// Items the thread dropped.
    dropped: AtomicUsize,
    /// Tests' holds of the thread ([`Disposer::hold`]).
    #[cfg(any(test, feature = "test-support"))]
    pause: Mutex<Pause>,
    #[cfg(any(test, feature = "test-support"))]
    turn: Condvar,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Pause {
    holds: usize,
    steps: usize,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    /// One more item, of `bytes` more charged: admitted while the queue
    /// holds fewer than [`MAX_ITEMS`] items and the charge stays within
    /// [`MAX_BYTES`] with it, or, the queue empty (the item reserved first:
    /// the queue as it was), whatever it weighs (module docs).
    fn admit(&self, bytes: usize) -> bool {
        let Ok(queued) = self.items.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_ITEMS).then_some(n + 1)) else {
            return false;
        };
        let fits = |b: usize| b.checked_add(bytes).filter(|&t| queued == 0 || t <= MAX_BYTES);
        if bytes > 0 && self.bytes.try_update(Ordering::AcqRel, Ordering::Acquire, fits).is_err() {
            self.items.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }

    /// The thread's turn to drop the next item: at once unless a test holds
    /// it, then one step at a time.
    #[cfg(any(test, feature = "test-support"))]
    fn wait_turn(&self) {
        let mut p = locked(&self.pause);
        while p.holds > 0 && p.steps == 0 {
            p = self.turn.wait(p).unwrap_or_else(PoisonError::into_inner);
        }
        p.steps = p.steps.saturating_sub(1);
    }

    fn backlog(&self) -> Backlog {
        Backlog {
            items: self.items.load(Ordering::Acquire),
            elements: self.elements.load(Ordering::Acquire),
            bytes: self.bytes.load(Ordering::Acquire),
            in_place: self.in_place.load(Ordering::Acquire),
            dropped: self.dropped.load(Ordering::Acquire),
        }
    }
}

/// The thread: each item dropped, then its charge and count released; a
/// flush answered once all before it went. Ends when its queue closes and
/// is drained.
fn run(s: Arc<Shared>, rx: Receiver<Msg>, done: SyncSender<()>) {
    IN_PLACE.set(1);
    for m in rx {
        match m {
            #[cfg(any(test, feature = "test-support"))]
            Msg::Flush(ack) => {
                let _ = ack.send(());
            }
            Msg::Drop(Retired { garbage, elements, charge }) => {
                #[cfg(any(test, feature = "test-support"))]
                s.wait_turn();
                drop(garbage);
                count(elements);
                drop(charge);
                s.elements.fetch_sub(elements, Ordering::AcqRel);
                s.items.fetch_sub(1, Ordering::AcqRel);
                s.dropped.fetch_add(1, Ordering::AcqRel);
                // Under the lock: a step waiting sees it, or is woken.
                #[cfg(any(test, feature = "test-support"))]
                {
                    let _p = locked(&s.pause);
                    s.turn.notify_all();
                }
            }
        }
    }
    let _ = done.send(());
}

thread_local! {
    /// Above 0, what this thread lets go it drops here: the disposer's
    /// thread, and any thread while it drops something in place (what
    /// that holds goes with it).
    static IN_PLACE: Cell<usize> = const { Cell::new(0) };
    /// Elements this thread destroyed ([`destroyed`]).
    static DESTROYED: Cell<usize> = const { Cell::new(0) };
    /// The bin a log made on this thread takes ([`Disposer::install`]).
    static CURRENT: RefCell<Bin> = RefCell::new(Bin::default());
    /// Store locks this thread holds ([`locking`]), and what it dropped in
    /// place meanwhile, kept until the last goes.
    static LOCKS: Cell<usize> = const { Cell::new(0) };
    static SPILL: RefCell<Vec<Spilt>> = const { RefCell::new(Vec::new()) };
}

fn count(n: usize) {
    DESTROYED.set(DESTROYED.get() + n);
}

/// `garbage` dropped here, its `elements` counted.
fn drop_here<T>(garbage: T, elements: usize) {
    #[cfg(any(test, feature = "test-support"))]
    probe::in_place(elements);
    IN_PLACE.set(IN_PLACE.get() + 1);
    drop(garbage);
    IN_PLACE.set(IN_PLACE.get() - 1);
    count(elements);
}

/// Something dropped in place while a store's lock is held here: dropped,
/// then uncharged, once the last lock goes.
struct Spilt {
    garbage: Box<dyn Any>,
    elements: usize,
    charge: Option<Charge>,
}

/// `garbage` (`elements`, and the charge that covers it, if any) dropped
/// where it is let go: at once, or, this thread holding a store's lock,
/// once it holds none ([`locking`]). Nothing to destroy goes at once.
fn here<T: 'static>(garbage: T, elements: usize, charge: Option<Charge>) {
    if elements > 0 && LOCKS.get() > 0 {
        SPILL.with_borrow_mut(|s| s.push(Spilt { garbage: Box::new(garbage), elements, charge }));
        return;
    }
    drop_here(garbage, elements);
    drop(charge);
}

/// A store's lock held on this thread ([`locking`]).
pub(super) struct Locked(PhantomData<*const ()>);

/// This thread takes a store's lock until the guard goes (the guard goes
/// after the lock's: a field after it): what it drops in place meanwhile
/// is dropped once it holds no store's lock, never under one (module
/// docs).
pub(super) fn locking() -> Locked {
    LOCKS.set(LOCKS.get() + 1);
    Locked(PhantomData)
}

impl Drop for Locked {
    fn drop(&mut self) {
        LOCKS.set(LOCKS.get() - 1);
        if LOCKS.get() > 0 {
            return;
        }
        // Taken whole first: what one drops (a lock taken and let go
        // again) spills into a list of its own.
        for Spilt { garbage, elements, charge } in SPILL.take() {
            drop_here(garbage, elements);
            drop(charge);
        }
    }
}

/// Elements this thread destroyed of what live logs let go (rounds' calls
/// and ids, facts' slots, stores): a hand-over is one (tests: the frame's
/// share of destruction).
pub fn destroyed() -> usize {
    DESTROYED.get()
}

/// The bin a log made on this thread now takes: the installed disposer's,
/// or none (what it lets go is dropped in place).
pub fn current() -> Bin {
    CURRENT.with_borrow(Bin::clone)
}

/// Where a log's garbage goes: its disposer's queue, or (none, or closed)
/// in place.
#[derive(Clone, Default)]
pub struct Bin(Option<Arc<Shared>>);

impl fmt::Debug for Bin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(s) => write!(f, "Bin({:?})", s.backlog()),
            None => f.write_str("Bin(in place)"),
        }
    }
}

impl Bin {
    /// `garbage`, `elements` to destroy and `bytes` (heap included, as
    /// allocated; its box's added here), dropped off this thread: handed
    /// over and charged until dropped, if admitted. This thread's cost:
    /// one element. Nothing to destroy, on the disposer, or refused, it is
    /// dropped here.
    pub(super) fn retire<T: Send + 'static>(&self, garbage: T, elements: usize, bytes: usize) {
        self.hand(garbage, elements, bytes + std::mem::size_of::<T>(), None);
    }

    /// `garbage`, charged by `held` already (and its box besides), dropped
    /// off this thread as [`Bin::retire`] would.
    pub(super) fn retire_charged<T: Send + 'static>(&self, garbage: T, elements: usize, held: Charge) {
        self.hand(garbage, elements, std::mem::size_of::<T>(), Some(held));
    }

    fn hand<T: Send + 'static>(&self, garbage: T, elements: usize, bytes: usize, held: Option<Charge>) {
        let shared = self.0.as_ref().filter(|_| elements > 0 && IN_PLACE.get() == 0);
        let Some(s) = shared else {
            return here(garbage, elements, held);
        };
        // Refused (saturated): dropped where it is let go, the contract
        // (module docs), after any store's lock this thread holds.
        if !s.admit(bytes) {
            s.in_place.fetch_add(1, Ordering::AcqRel);
            return here(garbage, elements, held);
        }
        // What it admitted is released with the charge (one held of nothing
        // had no bin to release from).
        let mut charge = held.unwrap_or_else(|| Charge { shared: None, bytes: 0 });
        charge.shared.get_or_insert_with(|| s.clone());
        charge.bytes += bytes;
        s.elements.fetch_add(elements, Ordering::AcqRel);
        let it = Retired { garbage: Box::new(garbage), elements, charge };
        let refused = {
            let tx = locked(&s.tx);
            match &*tx {
                Some(tx) => tx.send(Msg::Drop(it)).err().map(|e| e.0),
                None => Some(Msg::Drop(it)),
            }
        };
        match refused {
            None => {
                count(1);
                #[cfg(any(test, feature = "test-support"))]
                probe::handed();
            }
            // Closed since, or its thread gone: dropped here.
            Some(Msg::Drop(it)) => {
                s.items.fetch_sub(1, Ordering::AcqRel);
                s.elements.fetch_sub(it.elements, Ordering::AcqRel);
                s.in_place.fetch_add(1, Ordering::AcqRel);
                here(it.garbage, it.elements, Some(it.charge));
            }
            #[cfg(any(test, feature = "test-support"))]
            Some(Msg::Flush(_)) => unreachable!("a drop was sent"),
        }
    }

    /// `bytes` charged here until the charge drops (a store a job still
    /// reads, its log gone): whatever the cap, as what it charges is alive
    /// either way (module docs).
    pub(super) fn charge(&self, bytes: usize) -> Charge {
        let shared = self.0.clone().filter(|_| bytes > 0);
        if let Some(s) = &shared {
            s.bytes.fetch_add(bytes, Ordering::AcqRel);
        }
        Charge { shared, bytes }
    }

    /// What it holds (tests and the app's count: part of what it retains).
    pub fn backlog(&self) -> Backlog {
        self.0.as_ref().map(|s| s.backlog()).unwrap_or_default()
    }
}

/// Bytes charged to a bin until it drops ([`Bin::charge`]).
pub(super) struct Charge {
    shared: Option<Arc<Shared>>,
    bytes: usize,
}

impl fmt::Debug for Charge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Charge({})", self.bytes)
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        if let Some(s) = &self.shared {
            s.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

/// What a bin holds: items queued and their elements, bytes charged (the
/// queue's and the stores charged where they are), and, so far, items
/// dropped in place (refused, closed or never started) and by its thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backlog {
    pub items: usize,
    pub elements: usize,
    pub bytes: usize,
    pub in_place: usize,
    pub dropped: usize,
}

/// The disposer (module docs): its thread, and its bin. Dropped, it shuts
/// down ([`Disposer::shutdown`]).
pub struct Disposer {
    bin: Bin,
    thread: Option<(JoinHandle<()>, Receiver<()>)>,
}

impl fmt::Debug for Disposer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Disposer({:?}, running: {})", self.bin, self.thread.is_some())
    }
}

impl Disposer {
    /// A disposer and its thread. If the thread can't start, its bin drops
    /// everything in place, counted (said once on stderr).
    pub fn start() -> Disposer {
        let s = Arc::new(Shared::default());
        let (tx, rx) = channel();
        let (done_tx, done) = sync_channel(1);
        let thread = {
            let s = s.clone();
            std::thread::Builder::new().name("synaps-dispose".into()).spawn(move || run(s, rx, done_tx))
        };
        match thread {
            Ok(t) => {
                *locked(&s.tx) = Some(tx);
                Disposer { bin: Bin(Some(s)), thread: Some((t, done)) }
            }
            Err(e) => {
                eprintln!("synaps-desktop: the disposer's thread did not start ({e}): what live turns let go is dropped in place");
                Disposer { bin: Bin(Some(s)), thread: None }
            }
        }
    }

    /// A disposer whose thread did not start (tests: the degraded mode).
    #[cfg(any(test, feature = "test-support"))]
    pub fn failed() -> Disposer {
        Disposer { bin: Bin(Some(Arc::default())), thread: None }
    }

    /// Its bin: what a log made with it hands over.
    pub fn bin(&self) -> Bin {
        self.bin.clone()
    }

    /// What it holds ([`Backlog`]).
    pub fn backlog(&self) -> Backlog {
        self.bin.backlog()
    }

    /// Its bin, this thread's while the guard stands: the logs made here
    /// meanwhile hand over to it ([`current`]).
    pub fn install(&self) -> Installed {
        let was = CURRENT.with_borrow_mut(|c| std::mem::replace(c, self.bin.clone()));
        Installed { was: Some(was), _thread: PhantomData }
    }

    /// Closes its queue, and waits [`JOIN`] at most for its thread to drop
    /// what it holds and end: `true` if it did (joined), `false` if it is
    /// still at it (detached: it ends once done, module docs). What its
    /// bin is handed from now on is dropped in place.
    pub fn shutdown(mut self) -> bool {
        self.close()
    }

    fn close(&mut self) -> bool {
        if let Some(s) = &self.bin.0 {
            drop(locked(&s.tx).take());
        }
        let Some((thread, done)) = self.thread.take() else { return true };
        match done.recv_timeout(JOIN) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => thread.join().is_ok(),
            Err(RecvTimeoutError::Timeout) => false,
        }
    }

    /// Tests: its thread drops nothing more until a step or the hold goes
    /// (holds stack).
    #[cfg(any(test, feature = "test-support"))]
    pub fn hold(&self) -> Hold {
        let s = self.bin.0.clone().expect("a disposer's bin");
        locked(&s.pause).holds += 1;
        Hold(s)
    }

    /// Tests: waits until its thread dropped and uncharged all it was
    /// handed before (from any thread). A hold standing, it waits for the
    /// hold to go: drop holds first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn flush(&self) {
        let Some(s) = &self.bin.0 else { return };
        let (ack, done) = sync_channel(1);
        let sent = locked(&s.tx).as_ref().is_some_and(|tx| tx.send(Msg::Flush(ack)).is_ok());
        if sent {
            let _ = done.recv();
        }
    }
}

impl Drop for Disposer {
    fn drop(&mut self) {
        self.close();
    }
}

/// A disposer's bin installed on this thread ([`Disposer::install`]);
/// dropped, the one before is back.
pub struct Installed {
    was: Option<Bin>,
    _thread: PhantomData<*const ()>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        if let Some(was) = self.was.take() {
            CURRENT.with_borrow_mut(|c| *c = was);
        }
    }
}

/// A test's hold of a disposer's thread ([`Disposer::hold`]).
#[cfg(any(test, feature = "test-support"))]
pub struct Hold(Arc<Shared>);

#[cfg(any(test, feature = "test-support"))]
impl Hold {
    /// Lets the thread drop the next `n` items (those queued, at most), and
    /// waits until it did (10 s at most: then it panics).
    pub fn step(&self, n: usize) {
        let s = &self.0;
        let mut p = locked(&s.pause);
        let n = n.min(s.items.load(Ordering::Acquire));
        let target = s.dropped.load(Ordering::Acquire) + n;
        p.steps += n;
        s.turn.notify_all();
        let (p, wait) = s
            .turn
            .wait_timeout_while(p, Duration::from_secs(10), |_| s.dropped.load(Ordering::Acquire) < target)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!wait.timed_out(), "the disposer did not drop {n} items in 10 s");
        drop(p);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for Hold {
    fn drop(&mut self) {
        let mut p = locked(&self.0.pause);
        p.holds -= 1;
        if p.holds == 0 {
            p.steps = 0;
        }
        self.0.turn.notify_all();
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use probe::{Probe, on_handed, on_in_place};

/// Tests: this thread's drops in place and hand-overs, watched.
#[cfg(any(test, feature = "test-support"))]
mod probe {
    use std::cell::RefCell;

    #[derive(Default)]
    struct Probes {
        in_place: Option<Box<dyn FnMut(usize)>>,
        handed: Option<Box<dyn FnMut()>>,
    }

    thread_local! {
        static PROBES: RefCell<Probes> = RefCell::new(Probes::default());
    }

    /// A probe of this thread's; dropped, it watches no more.
    pub struct Probe(fn(&mut Probes));

    impl Drop for Probe {
        fn drop(&mut self) {
            PROBES.with_borrow_mut(self.0);
        }
    }

    /// Tests: `f` is told the elements of each drop this thread makes in
    /// place (a refusal, a closed or missing disposer: what it holds with
    /// it), as it makes it, until the probe goes.
    pub fn on_in_place(f: impl FnMut(usize) + 'static) -> Probe {
        PROBES.with_borrow_mut(|p| p.in_place = Some(Box::new(f)));
        Probe(|p| p.in_place = None)
    }

    /// Tests: `f` runs on this thread right after each hand-over it makes
    /// (queued: the disposer may drop it from now on), until the probe
    /// goes.
    pub fn on_handed(f: impl FnMut() + 'static) -> Probe {
        PROBES.with_borrow_mut(|p| p.handed = Some(Box::new(f)));
        Probe(|p| p.handed = None)
    }

    // Each taken out while it runs: one dropping or handing over in turn
    // is not told of its own.
    pub(super) fn in_place(elements: usize) {
        if let Some(mut f) = PROBES.with_borrow_mut(|p| p.in_place.take()) {
            f(elements);
            PROBES.with_borrow_mut(|p| {
                p.in_place.get_or_insert(f);
            });
        }
    }

    pub(super) fn handed() {
        if let Some(mut f) = PROBES.with_borrow_mut(|p| p.handed.take()) {
            f();
            PROBES.with_borrow_mut(|p| {
                p.handed.get_or_insert(f);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A charge of nothing (a store whose log left it to a job, all its
    /// room let go), then its store handed over: what the hand-over admits
    /// (its box) is released once it is dropped, as a charge of something
    /// is.
    #[test]
    fn a_held_charge_releases_what_its_hand_over_admits() {
        let d = Disposer::start();
        let bin = d.bin();
        for held in [0, 4_096] {
            let charge = bin.charge(held);
            assert_eq!(bin.backlog().bytes, held);
            bin.retire_charged(vec![7u8; 64], 1, charge);
            d.flush();
            assert_eq!((bin.backlog().items, bin.backlog().bytes), (0, 0), "held {held}");
        }
        assert_eq!(bin.backlog().dropped, 2);
    }

    /// An item past the byte cap is admitted into an empty queue (module
    /// docs), and refused while one is queued; with it queued the queue
    /// takes no more bytes, and empty again it takes the next.
    #[test]
    fn an_item_past_the_byte_cap_is_admitted_into_an_empty_queue_only() {
        let d = Disposer::start();
        let bin = d.bin();
        let big = Arc::new(());
        let hold = d.hold();
        bin.retire(big.clone(), 1, MAX_BYTES + 1);
        assert_eq!((bin.backlog().items, bin.backlog().in_place), (1, 0), "admitted into an empty queue");
        bin.retire(big.clone(), 1, MAX_BYTES + 1);
        bin.retire(big.clone(), 1, 1_024);
        assert_eq!((bin.backlog().items, bin.backlog().in_place, Arc::strong_count(&big)), (1, 2, 2), "refused while one is queued");
        drop(hold);
        d.flush();
        bin.retire(big.clone(), 1, MAX_BYTES + 1);
        d.flush();
        assert_eq!((Arc::strong_count(&big), bin.backlog().dropped, bin.backlog().bytes), (1, 2, 0));
    }

    /// Hand-overs from many threads at once (a barrier between them), the
    /// thread held: one item past the byte cap each and a run of 1 MiB
    /// ones. One item at most is admitted past the cap, into the queue
    /// empty; what is queued stays within `MAX_BYTES` and that one item,
    /// and every hand-over is queued or refused (counted). The hold gone,
    /// all is dropped and uncharged.
    #[test]
    fn hand_overs_racing_keep_the_queue_within_its_bound() {
        const THREADS: usize = 8;
        let d = Disposer::start();
        let bin = d.bin();
        let hold = d.hold();
        let gate = Arc::new(std::sync::Barrier::new(THREADS));
        let big = MAX_BYTES + (1 << 20);
        let racers: Vec<_> = (0..THREADS)
            .map(|k| {
                let (bin, gate) = (bin.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    for j in 0..40 {
                        let bytes = if j == k { big } else { 1 << 20 };
                        bin.retire(vec![0u8; 8], 1, bytes);
                    }
                })
            })
            .collect();
        for r in racers {
            r.join().expect("a racer");
        }
        let b = bin.backlog();
        let one = big + std::mem::size_of::<Vec<u8>>();
        assert!(b.bytes <= MAX_BYTES + one, "{b:?}");
        assert_eq!(b.items + b.in_place, THREADS * 40, "every hand-over queued or refused: {b:?}");
        assert!(b.items >= 1 && b.in_place >= 1, "{b:?}");
        drop(hold);
        d.flush();
        assert_eq!((bin.backlog().items, bin.backlog().elements, bin.backlog().bytes), (0, 0, 0));
    }

    /// What is dropped in place under a store's lock ([`locking`]) is
    /// dropped once the last such lock goes, uncharged after; nothing to
    /// destroy goes at once.
    #[test]
    fn what_is_dropped_under_a_stores_lock_waits_for_the_lock() {
        let d = Disposer::start();
        let bin = d.bin();
        let hold = d.hold();
        bin.retire(Arc::new(()), 1, MAX_BYTES);
        let (a, b) = (Arc::new(()), Arc::new(()));
        {
            let _outer = locking();
            {
                let _inner = locking();
                bin.retire(a.clone(), 1, 1_024);
                Bin::default().retire(b.clone(), 1, 0);
            }
            assert_eq!((Arc::strong_count(&a), Arc::strong_count(&b)), (2, 2), "an outer lock still held");
        }
        assert_eq!((Arc::strong_count(&a), Arc::strong_count(&b), bin.backlog().in_place), (1, 1, 1));
        drop(hold);
        d.flush();
    }
}
