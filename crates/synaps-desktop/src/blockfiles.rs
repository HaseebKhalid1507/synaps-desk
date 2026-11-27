//! The desk's two block stores on disk (synaps-cards `docs/blocks.md` v1.3,
//! §3.5 and §1.8): `blocks.json` ([`crate::presentation`]) and
//! `block-desires.json` ([`crate::desires`]), next to `settings.json`.
//!
//! **recall's writer** (`recall.rs`, #93): the whole file written to a new
//! temporary file of this process beside it (`<file>.tmp.<pid>.<n>`,
//! [`recall::tmp_of`]), this user's only from its creation
//! ([`recall::create_private`]: mode 0600, a protected DACL on Windows),
//! synced, renamed over the file, the folder synced. A failed write removes
//! its temporary file. What a writer that died left beside the file is
//! swept at a load ([`recall::sweep`]: regular files of exactly that
//! pattern, nothing else, no link followed).
//!
//! **Two desk processes share the files** (two builds, two launches). A
//! write is a change, not a snapshot: under an advisory lock of the file's
//! own (`<file>.lock`, [`crate::theme::lock_settings`]) it reads the file
//! as it is now, applies its change to that ([`update`]), and writes the
//! result. So neither process writes over what the other wrote after it
//! last read, and the caps are checked against the file, not a copy. The
//! sweep runs under the same lock, so it never takes another process's
//! temporary file mid-write. A lock not had in [`LOCK_WAIT`] fails the
//! write (nothing is ever written unlocked): a holder stopped mid-write
//! never freezes this one, and never loses what it wrote.
//!
//! **Off the UI thread.** [`load`] and [`update`] wait for the lock, and
//! write: the stores run them on a background task, one at a time, and
//! take what they return ([`crate::desires::Write`],
//! [`crate::presentation::Write`]). At a launch the UI thread only reads
//! the file once ([`peek`]: no lock, a rename being atomic).
//!
//! **A file that doesn't parse** is copied aside to `<file>.bad` (at a
//! load or a write, under the lock) and read as empty; the next write goes
//! on from there, and the log says so.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::recall;

/// How long a write or a load waits for another process's lock.
pub(crate) const LOCK_WAIT: Duration = Duration::from_millis(250);

/// What a store's file holds, read and written whole.
pub(crate) trait Stored: Default {
    /// The file's text as this store reads it: what parses, and how many
    /// entries were left out (unreadable); `Err` for a file that isn't one.
    fn parse(text: &str) -> Result<(Self, usize), String>;
    /// The file's text.
    fn text(&self) -> String;
}

/// Why a write didn't happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failed {
    /// The change refused itself against the file as it is (a cap).
    Refused(String),
    /// The disk: the lock, the read or the write (nothing written).
    Io(String),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Refused(why) | Failed::Io(why) => f.write_str(why),
        }
    }
}

/// One writer at a time in this process (each also takes the file's lock).
static WRITER: Mutex<()> = Mutex::new(());

/// Every temporary file's number in this process.
static NEXT: AtomicU64 = AtomicU64::new(0);

fn lock_file(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}.lock"))
}

fn name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// The file as it is: missing is empty; one that doesn't parse is set
/// aside (`<file>.bad`) and read as empty, why said.
fn read<S: Stored>(path: &Path, set_aside: bool) -> Result<(S, Option<String>), String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((S::default(), None)),
        Err(e) => return Err(format!("{}: {e}", name(path))),
    };
    let parsed = String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string()).and_then(|t| S::parse(&t));
    match parsed {
        Ok((s, 0)) => Ok((s, None)),
        Ok((s, bad)) => {
            log::warn!(target: "synaps_desktop::blocks", "{}: {bad} unreadable entries left out", path.display());
            Ok((s, None))
        }
        Err(why) if !set_aside => Ok((S::default(), Some(why))),
        Err(why) => {
            let bad = path.with_file_name(format!("{}.bad", name(path)));
            let kept = std::fs::copy(path, &bad).map(|_| ()).map_err(|e| e.to_string());
            log::warn!(target: "synaps_desktop::blocks", "{}: unreadable ({why}), set aside ({kept:?}); read as empty", path.display());
            Ok((S::default(), Some(why)))
        }
    }
}

/// The store at `path` as it is, read without the lock or a write (a
/// launch's first read, on the UI thread: a rename is atomic): missing is
/// empty, and one that doesn't parse reads as empty, why said (set aside
/// by the next [`load`] or [`update`]).
pub(crate) fn peek<S: Stored>(path: &Path) -> (S, Option<String>) {
    read::<S>(path, false).unwrap_or_else(|why| (S::default(), Some(why)))
}

/// The disk's waits and writes made on this thread (tests: none on the UI
/// thread).
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

/// The store at `path`, read at a load (a launch): what crashes left beside
/// it swept first, under the lock. A lock not had in [`LOCK_WAIT`] reads it
/// anyway (a rename is atomic) and leaves the sweep for the next load.
/// Also: why the file was unreadable (read as empty), and the sweep's
/// count of leftovers removed, or its first failure.
pub(crate) fn load<S: Stored>(path: &Path) -> (S, Option<String>, std::io::Result<usize>) {
    counted();
    let _one = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
    let lock = crate::theme::lock_settings(&lock_file(path), LOCK_WAIT);
    let swept = match &lock {
        Ok(_) => recall::sweep(path),
        Err(why) => Err(std::io::Error::other(why.clone())),
    };
    match read::<S>(path, true) {
        Ok((s, why)) => (s, why, swept),
        Err(why) => {
            log::warn!(target: "synaps_desktop::blocks", "{why}; read as empty");
            (S::default(), Some(why), swept)
        }
    }
}

/// Change the store at `path` with `change`, applied to the file as it is
/// now (module docs), and write it: the result, as written. `change` may
/// refuse; then nothing is written.
pub(crate) fn update<S: Stored>(path: &Path, change: impl FnOnce(&mut S) -> Result<(), String>) -> Result<S, Failed> {
    update_then(path, change, || Ok(()))
}

/// [`update`], `between` run once the temporary file is written, before
/// the rename, the locks held (tests: a write caught or failing there).
pub(crate) fn update_then<S: Stored>(
    path: &Path,
    change: impl FnOnce(&mut S) -> Result<(), String>,
    between: impl FnOnce() -> std::io::Result<()>,
) -> Result<S, Failed> {
    use std::io::Write as _;
    counted();
    let _one = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Failed::Io(format!("{}: {e}", dir.display())))?;
    }
    let _lock = crate::theme::lock_settings(&lock_file(path), LOCK_WAIT).map_err(Failed::Io)?;
    let (mut s, _) = read::<S>(path, true).map_err(Failed::Io)?;
    change(&mut s).map_err(Failed::Refused)?;
    let text = s.text();
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
    done.map_err(|e| Failed::Io(format!("{}: {e}", name(path))))?;
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(s)
}
