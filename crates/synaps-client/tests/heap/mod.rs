//! #102's counting allocator (`card_memory.rs`'s first), shared by the
//! tests that read the heap: each block is charged to an account, the
//! account of the thread that allocated it ([`mine`]; [`charging`] moves
//! a thread to another), and taken off that account wherever it is freed
//! ([`held`], [`held_by`]); and [`here`], what this thread allocated less
//! what it freed, whoever's. Each test binary uses what it reads.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

pub struct Counting;

/// The accounts: the bytes each holds. 0: the threads that took none.
static HELD: [AtomicIsize; 64] = [const { AtomicIsize::new(0) }; 64];
static NEXT: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    static ACCOUNT: Cell<usize> = const { Cell::new(0) };
    static HERE: Cell<isize> = const { Cell::new(0) };
}

/// A block of `l` and its account before it: the block's alignment (a
/// word at least) of room, the account at its start. None: too big.
pub fn outer(l: Layout) -> Option<(Layout, usize)> {
    let room = l.align().max(std::mem::size_of::<usize>());
    Some((Layout::from_size_align(l.size().checked_add(room)?, room).ok()?, room))
}

/// `l`'s block at `base` (null: none), charged to this thread's account.
unsafe fn opened(base: *mut u8, l: Layout, room: usize) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    let account = ACCOUNT.get();
    unsafe { (base as *mut usize).write(account) };
    HELD[account].fetch_add(l.size() as isize, Ordering::Relaxed);
    HERE.set(HERE.get() + l.size() as isize);
    unsafe { base.add(room) }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let Some((o, room)) = outer(l) else { return std::ptr::null_mut() };
        unsafe { opened(System.alloc(o), l, room) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let Some((o, room)) = outer(l) else { return std::ptr::null_mut() };
        unsafe { opened(System.alloc_zeroed(o), l, room) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let (o, room) = outer(l).expect("a block allocated");
        let base = unsafe { p.sub(room) };
        let account = unsafe { (base as *const usize).read() };
        HELD[account].fetch_sub(l.size() as isize, Ordering::Relaxed);
        HERE.set(HERE.get() - l.size() as isize);
        unsafe { System.dealloc(base, o) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let (o, room) = outer(l).expect("a block allocated");
        // The block it grows to, room added, must be a layout too: if not,
        // none, and `p` is left as it was.
        let Some((grown, _)) = Layout::from_size_align(size, l.align()).ok().and_then(outer) else {
            return std::ptr::null_mut();
        };
        let base = unsafe { p.sub(room) };
        let account = unsafe { (base as *const usize).read() };
        let q = unsafe { System.realloc(base, o, grown.size()) };
        if q.is_null() {
            return q;
        }
        // The block's account is the one it was allocated on, moved with it.
        HELD[account].fetch_add(size as isize - l.size() as isize, Ordering::Relaxed);
        HERE.set(HERE.get() + size as isize - l.size() as isize);
        unsafe { q.add(room) }
    }
}

#[global_allocator]
pub static COUNTING: Counting = Counting;

/// This thread's own account, from now on (a test's first line).
pub fn mine() -> usize {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    assert!(n < HELD.len(), "more tests than accounts");
    ACCOUNT.set(n);
    n
}

/// The bytes this thread's account holds: what it allocated (since
/// [`mine`]) and is not freed, wherever it is freed.
pub fn held() -> isize {
    HELD[ACCOUNT.get()].load(Ordering::Relaxed)
}

/// The bytes this thread allocated less those it freed, whoever's.
pub fn here() -> isize {
    HERE.get()
}

/// The bytes account `a` holds.
pub fn held_by(a: usize) -> isize {
    HELD[a].load(Ordering::Relaxed)
}

/// From now on this thread's blocks are charged to account `a` (one
/// [`mine`] gave): the account it charged before.
pub fn charging(a: usize) -> usize {
    ACCOUNT.replace(a)
}
