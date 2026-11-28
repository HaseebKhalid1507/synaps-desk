//! #102's counting allocator (`card_memory.rs`'s first), shared by the
//! tests that read the heap: each block is charged to an account, the
//! account of the thread that allocated it ([`mine`]; [`charging`] moves
//! a thread to another), and taken off that account wherever it is freed
//! ([`held`], [`held_by`]); and [`here`], what this thread allocated less
//! what it freed, whoever's. Each test binary uses what it reads.
//!
//! Only a thread that measures pays for it: one with an account of its own
//! ([`mine`], [`charging`]). Nothing is counted for any other thread, and
//! on a 64-bit target its blocks of alignment 8 or less are the system's
//! own, with no room before them ([`bare`]), so a whole test binary on this
//! allocator runs at about the system allocator's speed. A thread's [`here`]
//! counts from its first [`mine`]; account 0 holds nothing.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

pub struct Counting;

/// The accounts: the bytes each holds. 0: the threads that took none,
/// never counted (no thread contends for it).
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

/// Whether blocks are split by their address ([`bare`]): 64-bit targets,
/// whose system blocks are 16-aligned (checked, [`opened`]).
const SPLIT: bool = cfg!(target_pointer_width = "64");

/// A block allocated on a thread that doesn't measure, of alignment 8 or
/// less, is bare: `l` asked of the system at 16, 16 bytes at least (its
/// fast path), and nothing before it. One with its account before it
/// starts 8 bytes past the system's 16-aligned block, so a block is bare
/// exactly when its address is a multiple of 16 ([`is_bare`]). None: too
/// big.
fn bare(l: Layout) -> Option<Layout> {
    Layout::from_size_align(l.size().max(16), 16).ok()
}

fn is_bare(p: *mut u8, l: Layout) -> bool {
    SPLIT && l.align() <= 8 && (p as usize).is_multiple_of(16)
}

/// `l`'s block: bare on a thread that doesn't measure ([`bare`]), else with
/// its account before it. `get` is the system's allocation.
unsafe fn open(l: Layout, get: impl FnOnce(Layout) -> *mut u8) -> *mut u8 {
    if SPLIT && l.align() <= 8 && ACCOUNT.get() == 0 {
        return bare(l).map_or(std::ptr::null_mut(), get);
    }
    let Some((o, room)) = outer(l) else { return std::ptr::null_mut() };
    unsafe { opened(get(o), l, room) }
}

/// `l`'s block at `base` (null: none), charged to this thread's account.
unsafe fn opened(base: *mut u8, l: Layout, room: usize) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    // A block with its account must never read as bare: the system's
    // blocks are 16-aligned on every 64-bit target this builds for. If one
    // isn't, stop rather than free a block with the wrong layout later.
    if SPLIT && l.align() <= 8 && !(base as usize).is_multiple_of(16) {
        std::process::abort();
    }
    let account = ACCOUNT.get();
    unsafe { (base as *mut usize).write(account) };
    if account != 0 {
        HELD[account].fetch_add(l.size() as isize, Ordering::Relaxed);
        HERE.set(HERE.get() + l.size() as isize);
    }
    unsafe { base.add(room) }
}

/// `n` bytes allocated (less: freed) on this thread, for [`here`]: counted
/// only while it measures.
fn here_moves(n: isize) {
    if ACCOUNT.get() != 0 {
        HERE.set(HERE.get() + n);
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { open(l, |o| System.alloc(o)) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { open(l, |o| System.alloc_zeroed(o)) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if is_bare(p, l) {
            here_moves(-(l.size() as isize));
            let b = bare(l).expect("a bare block's layout");
            return unsafe { System.dealloc(p, b) };
        }
        let (o, room) = outer(l).expect("a block allocated");
        let base = unsafe { p.sub(room) };
        let account = unsafe { (base as *const usize).read() };
        if account != 0 {
            HELD[account].fetch_sub(l.size() as isize, Ordering::Relaxed);
        }
        here_moves(-(l.size() as isize));
        unsafe { System.dealloc(base, o) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        // A bare block stays bare, at 16 (the system keeps its alignment).
        if is_bare(p, l) {
            let (Some(b), Some(grown)) = (bare(l), Layout::from_size_align(size, l.align()).ok().and_then(bare)) else {
                return std::ptr::null_mut();
            };
            let q = unsafe { System.realloc(p, b, grown.size()) };
            if !q.is_null() {
                here_moves(size as isize - l.size() as isize);
            }
            return q;
        }
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
        // Moved, it must still never read as bare ([`opened`]).
        if SPLIT && l.align() <= 8 && !(q as usize).is_multiple_of(16) {
            std::process::abort();
        }
        // The block's account is the one it was allocated on, moved with it.
        if account != 0 {
            HELD[account].fetch_add(size as isize - l.size() as isize, Ordering::Relaxed);
        }
        here_moves(size as isize - l.size() as isize);
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

/// The bytes this thread allocated less those it freed, whoever's, while
/// it measured (since its first [`mine`]).
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
