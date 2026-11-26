//! What a live turn's log lets go, dropped off the frame ([`retire`]): a
//! round's calls when the round goes (a seal, a user message, a reset, a
//! log replaced), a reset's facts, a log's store when its last owner lets
//! go. Each costs the calls or facts it kept to destroy; whoever lets go
//! (the frame, mostly) hands it over in O(1), and one thread, the
//! disposer, drops it. What the disposer holds is counted until it drops
//! it ([`pending`]): its items and their bytes, heap included, the inputs
//! of a reset's calls too (their rows are gone). A thread's own share of
//! destruction is counted ([`destroyed`]): a hand-over is one element,
//! whatever it holds; what a thread drops in place, every element.
//!
//! Tests gate it ([`gate`]): while a gate stands, what its thread retires
//! is held, not handed over, so an event provably finishes before the
//! garbage it made goes; [`flush`] waits until the disposer dropped all it
//! was handed before.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};

/// Something retired: what it is, the elements its drop destroys, its
/// bytes (heap included).
struct Retired {
    garbage: Box<dyn Any + Send>,
    elements: usize,
    bytes: usize,
}

static ITEMS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

/// The disposer: `None` if its thread could not start (then whoever lets
/// go drops it, counted).
static DISPOSER: LazyLock<Option<Sender<Retired>>> = LazyLock::new(|| {
    let (tx, rx) = channel::<Retired>();
    std::thread::Builder::new()
        .name("synaps-dispose".into())
        .spawn(move || {
            DISPOSING.set(true);
            for Retired { garbage, elements, bytes } in rx {
                drop(garbage);
                count(elements);
                BYTES.fetch_sub(bytes, Ordering::AcqRel);
                ITEMS.fetch_sub(1, Ordering::AcqRel);
            }
        })
        .ok()
        .map(|_| tx)
});

thread_local! {
    /// This thread is the disposer: what it lets go, it drops.
    static DISPOSING: Cell<bool> = const { Cell::new(false) };
    /// Elements this thread destroyed ([`destroyed`]).
    static DESTROYED: Cell<usize> = const { Cell::new(0) };
    /// This thread's gate, if one stands: what it retires meanwhile.
    static GATE: RefCell<Option<Vec<Retired>>> = const { RefCell::new(None) };
}

fn count(n: usize) {
    DESTROYED.set(DESTROYED.get() + n);
}

/// `garbage`, `elements` to destroy and `bytes` (heap included), dropped
/// off this thread: handed to the disposer (counted pending until it is
/// dropped), or held by this thread's gate. This thread's cost: one
/// element. Nothing to destroy (`elements` 0), or on the disposer, it is
/// dropped here.
pub(super) fn retire<T: Send + 'static>(garbage: T, elements: usize, bytes: usize) {
    if elements == 0 || DISPOSING.get() {
        drop(garbage);
        count(elements);
        return;
    }
    count(1);
    let it = Retired { garbage: Box::new(garbage), elements, bytes };
    let it = GATE.with_borrow_mut(|g| match g {
        Some(held) => {
            held.push(it);
            None
        }
        None => Some(it),
    });
    if let Some(it) = it {
        hand_over(it);
    }
}

fn hand_over(it: Retired) {
    let Some(tx) = &*DISPOSER else {
        count(it.elements);
        return;
    };
    let bytes = it.bytes;
    ITEMS.fetch_add(1, Ordering::AcqRel);
    BYTES.fetch_add(bytes, Ordering::AcqRel);
    if let Err(e) = tx.send(it) {
        ITEMS.fetch_sub(1, Ordering::AcqRel);
        BYTES.fetch_sub(bytes, Ordering::AcqRel);
        count(e.0.elements);
    }
}

/// What the disposer holds, process-wide: items handed over and not yet
/// dropped, and their bytes (heap included).
pub fn pending() -> (usize, usize) {
    (ITEMS.load(Ordering::Acquire), BYTES.load(Ordering::Acquire))
}

/// Elements this thread destroyed of what live logs let go (rounds' calls
/// and ids, facts' slots, stores): a hand-over is one (tests: the frame's
/// share of destruction).
pub fn destroyed() -> usize {
    DESTROYED.get()
}

/// Waits until the disposer dropped everything handed to it before (from
/// any thread).
pub fn flush() {
    struct Ack(Option<SyncSender<()>>);
    impl Drop for Ack {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
    let Some(tx) = &*DISPOSER else { return };
    let (ack, done) = sync_channel(1);
    ITEMS.fetch_add(1, Ordering::AcqRel);
    if tx.send(Retired { garbage: Box::new(Ack(Some(ack))), elements: 0, bytes: 0 }).is_ok() {
        let _ = done.recv();
    } else {
        ITEMS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A gate on this thread's garbage (tests): while it stands, what the
/// thread retires is held here, not handed over; dropped, it hands it all
/// to the disposer.
pub struct Gate {
    _thread: PhantomData<*const ()>,
}

/// A gate on this thread's garbage (one at a time).
pub fn gate() -> Gate {
    GATE.with_borrow_mut(|g| {
        assert!(g.is_none(), "one gate a thread");
        *g = Some(Vec::new());
    });
    Gate { _thread: PhantomData }
}

impl Gate {
    /// What it holds: items, the elements their drops destroy, and their
    /// bytes (heap included).
    pub fn held(&self) -> (usize, usize, usize) {
        GATE.with_borrow(|g| {
            let held = g.as_deref().unwrap_or_default();
            (held.len(), held.iter().map(|r| r.elements).sum(), held.iter().map(|r| r.bytes).sum())
        })
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        for it in GATE.with_borrow_mut(Option::take).unwrap_or_default() {
            hand_over(it);
        }
    }
}
