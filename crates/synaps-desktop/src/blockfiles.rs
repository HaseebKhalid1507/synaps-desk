//! The desk's two block stores on disk (synaps-cards `docs/blocks.md` v1.3,
//! §3.5 and §1.8): `blocks.json` ([`crate::presentation`]) and
//! `block-desires.json` ([`crate::desires`]), next to `settings.json`.
//!
//! **One writer per file: the desk process that owns it.** At its launch a
//! store takes the file's lock (`<file>.lock`, [`own`]) and holds it for
//! the process's life; only that process reads, writes or decides on the
//! file. Its memory is the truth, and each write is a whole snapshot of it.
//! A second desk process on the machine (another build, another launch)
//! doesn't get the lock: it keeps its desires and presentation in memory
//! only, never touches the file, and says so. Nothing is merged between
//! processes: the history is what desks share (§3.5).
//!
//! **recall's writer** (`recall.rs`, #93): the whole file written to a new
//! temporary file of this process beside it (`<file>.tmp.<pid>.<n>`,
//! [`recall::tmp_of`]), this user's only from its creation
//! ([`recall::create_private`]: mode 0600, a protected DACL on Windows),
//! synced, renamed over the file, the folder synced. A failed write removes
//! its temporary file. What a writer that died left beside the file is
//! swept by the owner's first write ([`sweep`]: regular files of exactly
//! that pattern, nothing else, no link followed).
//!
//! **Off the UI thread.** [`sweep`] and [`write`] touch the disk: the
//! stores run them on a background task, one at a time, in order, and take
//! back what they return. At a launch the UI thread only takes the lock
//! (never waiting for it) and reads the file once ([`peek`]).
//!
//! **A file that doesn't parse** reads as empty, and the sweep copies it
//! aside to `<file>.bad` before anything is written over it; the log says
//! so.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::recall;

/// What a store's file holds, read and written whole.
pub(crate) trait Stored: Default {
    /// The file's text as this store reads it: what parses, and how many
    /// entries were left out (unreadable); `Err` for a file that isn't one.
    fn parse(text: &str) -> Result<(Self, usize), String>;
    /// The file's text.
    fn text(&self) -> String;
}

/// Every temporary file's number in this process.
static NEXT: AtomicU64 = AtomicU64::new(0);

fn lock_file(path: &Path) -> PathBuf {
    path.with_file_name(format!("{}.lock", name(path)))
}

fn name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// The file at `path` owned by this process: its lock, taken now without
/// waiting, held until the returned file is dropped (the store's life).
/// `Err`: why not (another process holds it, or the folder can't be
/// made): the store keeps to memory.
pub(crate) fn own(path: &Path) -> Result<std::fs::File, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    crate::theme::lock_settings(&lock_file(path), std::time::Duration::ZERO)
}

/// The file as it is: missing is empty; one that doesn't parse reads as
/// empty, why said.
fn read<S: Stored>(path: &Path) -> Result<(S, Option<String>), String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((S::default(), None)),
        Err(e) => return Err(format!("{}: {e}", name(path))),
    };
    match String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string()).and_then(|t| S::parse(&t)) {
        Ok((s, 0)) => Ok((s, None)),
        Ok((s, bad)) => {
            log::warn!(target: "synaps_desktop::blocks", "{}: {bad} unreadable entries left out", path.display());
            Ok((s, None))
        }
        Err(why) => Ok((S::default(), Some(why))),
    }
}

/// The store at `path` as it is (the owner's launch read, on the UI
/// thread): missing is empty, and one that doesn't parse reads as empty,
/// why said (set aside by the [`sweep`]).
pub(crate) fn peek<S: Stored>(path: &Path) -> (S, Option<String>) {
    read::<S>(path).unwrap_or_else(|why| (S::default(), Some(why)))
}

/// The disk's writes made on this thread (tests: none on the UI thread).
#[cfg(test)]
pub(crate) fn on_this_thread() -> usize {
    ON_THIS_THREAD.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    static ON_THIS_THREAD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn counted() {
    #[cfg(test)]
    ON_THIS_THREAD.with(|n| n.set(n.get() + 1));
}

/// The owner's first write: what crashes left beside the file removed, and
/// a file that doesn't parse copied aside (`<file>.bad`) before a snapshot
/// replaces it. What went wrong, if anything.
pub(crate) fn sweep<S: Stored>(path: &Path) -> Option<String> {
    counted();
    let swept = recall::sweep(path).err().map(|e| format!("{}: leftovers not swept ({e})", name(path)));
    let unreadable = match read::<S>(path) {
        Ok((_, Some(why))) => {
            let bad = path.with_file_name(format!("{}.bad", name(path)));
            let kept = std::fs::copy(path, &bad).map(|_| ()).map_err(|e| e.to_string());
            log::warn!(target: "synaps_desktop::blocks", "{}: unreadable ({why}), set aside ({kept:?}); read as empty", path.display());
            Some(format!("{}: unreadable ({why}), set aside", name(path)))
        }
        Ok(_) => None,
        Err(why) => Some(why),
    };
    swept.or(unreadable)
}

/// `text`, the whole file, written at `path` (module docs).
pub(crate) fn write(path: &Path, text: &str) -> Result<(), String> {
    write_then(path, text, || Ok(()))
}

/// [`write`], `between` run once the temporary file is written, before the
/// rename (tests: a write caught or failing there).
pub(crate) fn write_then(path: &Path, text: &str, between: impl FnOnce() -> std::io::Result<()>) -> Result<(), String> {
    use std::io::Write as _;
    counted();
    let tmp = recall::tmp_of(path, NEXT.fetch_add(1, Ordering::Relaxed));
    // One of this name already (a link, a leftover) goes first: the new
    // one is created, never opened.
    let _ = std::fs::remove_file(&tmp);
    let done = (|| {
        let mut f = recall::create_private(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        between()?;
        std::fs::rename(&tmp, path)
    })();
    if done.is_err()
        && let Err(e) = std::fs::remove_file(&tmp)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!(target: "synaps_desktop::blocks", "{}: left, not removed ({e})", tmp.display());
    }
    done.map_err(|e| format!("{}: {e}", name(path)))?;
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}
