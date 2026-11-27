//! The desk's two block stores on disk (synaps-cards `docs/blocks.md` v1.3,
//! §3.5 and §1.8): `blocks.json` ([`crate::presentation`]) and
//! `block-desires.json` ([`crate::desires`]), next to `settings.json`.
//!
//! **One writer per file: the desk process that owns it.** At its launch,
//! before any window, the app takes each file's lock (`<file>.lock`,
//! [`own`]) once ([`crate::rail_wishes::Home`]); only that process reads,
//! writes or decides on the file. The lock is an [`Owned`]: the app holds
//! it until it shuts down, its window's store a clone, and every write
//! issued another until it ran, so the lock outlives every write that can
//! still touch the file. Memory is the truth, and each write is a whole
//! snapshot of it. A second desk process on the machine (another build,
//! another launch) doesn't get the lock: it keeps its desires and
//! presentation in memory only, never touches the file, and says so for as
//! long as it runs. Nothing is merged between processes: the history is
//! what desks share (§3.5).
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
//! **Off the UI thread, on the process's writer, always.** [`sweep`] and
//! [`write`] touch the disk: the stores hand them to the [`Writer`], one
//! thread that runs them in order, and take back what they return. Nothing
//! else runs them: a writer whose thread didn't start, or stopped, takes
//! no job (the job never runs), and the desk keeps its rail changes in
//! memory only from then on, and says so, as a second desk does
//! ([`crate::rail_wishes`]). A window closing hands it the last snapshot
//! of each file memory has changed since; the app's quit, a window open or
//! not, waits for it [`JOIN`] at most, as D5's disposer does, then lets go
//! of its locks (a write still running keeps its own until it ends). At a
//! launch the UI thread only takes the lock (never waiting for it) and
//! reads the file once ([`peek`]).
//!
//! **A file that doesn't parse** reads as empty, and the sweep copies it
//! aside to `<file>.bad` before anything is written over it; the log says
//! so.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

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

/// This process's ownership of a file (module docs): its lock, held while
/// any clone of it stands.
#[derive(Clone, Debug)]
pub(crate) struct Owned(#[allow(dead_code, reason = "held: the lock")] Arc<std::fs::File>);

/// Why a file isn't this process's: its store keeps to memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotOwned {
    /// Another process holds its lock: another desk.
    Held,
    /// No lock to take: no folder for it, or its lock couldn't be opened
    /// or taken (why).
    Failed(String),
}

/// The file at `path` owned by this process: its lock, taken now without
/// waiting, held while the returned [`Owned`] (or a clone) stands.
pub(crate) fn own(path: &Path) -> Result<Owned, NotOwned> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| NotOwned::Failed(format!("{}: {e}", dir.display())))?;
    }
    let lock = lock_file(path);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .map_err(|e| NotOwned::Failed(format!("{} not opened ({e})", lock.display())))?;
    match f.try_lock() {
        Ok(()) => Ok(Owned(Arc::new(f))),
        Err(std::fs::TryLockError::WouldBlock) => Err(NotOwned::Held),
        Err(std::fs::TryLockError::Error(e)) => Err(NotOwned::Failed(format!("{} not locked ({e})", lock.display()))),
    }
}

/// How long the app's shutdown waits for the writer's last writes
/// ([`Writer::shutdown`]): D5's disposer's wait.
pub(crate) const JOIN: Duration = Duration::from_secs(2);

type Job = Box<dyn FnOnce() + Send>;

/// The process's one writer of the block stores' files (module docs): a
/// thread that runs each write handed to it, in order. Shut down (or
/// dropped): its queue closed, what it holds run, waited for its bound
/// ([`JOIN`]) at most (then it ends alone, detached: each write it still
/// holds keeps its file's lock until it ran). One whose thread didn't
/// start, stopped (a job that panicked) or shut down takes nothing more:
/// [`Writer::run`] says why, and the job never runs, here or anywhere.
///
/// Tests' windows have one that queues ([`Writer::queued`]): what it is
/// handed runs, in order, when [`Writer::drain`] is called (by the
/// window's own executor, deterministic, on the test's thread: GPUI's test
/// scheduler forbids another thread waking its tasks), none while held.
pub(crate) struct Writer {
    tx: Mutex<Option<Sender<Job>>>,
    thread: Mutex<Option<(JoinHandle<()>, Receiver<()>)>>,
    /// How long its shutdown waits for what it holds.
    join: Duration,
    /// Why it never had a thread (none: it had one).
    never: Option<String>,
    #[cfg(test)]
    queued: Option<Mutex<Queued>>,
}

/// Why a [`Writer`] took no job: it never started, stopped or shut down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stopped(pub(crate) String);

/// Tests: a queuing writer's jobs, its holds, and whether it stopped.
#[cfg(test)]
#[derive(Default)]
struct Queued {
    jobs: std::collections::VecDeque<Job>,
    holds: usize,
    stopped: bool,
}

/// Tests: a queuing writer held ([`Writer::hold`]) until this goes.
#[cfg(test)]
pub(crate) struct Hold(Arc<Writer>);

#[cfg(test)]
impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(q) = &self.0.queued {
            locked(q).holds -= 1;
        }
    }
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Writer {
    /// A writer and its thread, its shutdown waiting [`JOIN`] at most.
    pub(crate) fn start() -> Arc<Writer> {
        Self::start_with(JOIN, |f| std::thread::Builder::new().name("synaps-blockfiles".into()).spawn(f))
    }

    /// A writer whose thread `spawn` starts, its shutdown waiting `join`
    /// at most (tests: a spawn that fails, a short bound). Not started: a
    /// writer that takes nothing ([`Writer::never`]).
    pub(crate) fn start_with(
        join: Duration,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>,
    ) -> Arc<Writer> {
        let (tx, rx) = channel::<Job>();
        let (done_tx, done) = sync_channel(1);
        let spawned = spawn(Box::new(move || {
            for job in rx {
                job();
            }
            let _ = done_tx.send(());
        }));
        match spawned {
            Ok(t) => Arc::new(Writer {
                tx: Mutex::new(Some(tx)),
                thread: Mutex::new(Some((t, done))),
                join,
                never: None,
                #[cfg(test)]
                queued: None,
            }),
            Err(e) => {
                log::warn!(target: "synaps_desktop::blocks", "the block stores' writer did not start ({e}): rail changes are kept in memory only");
                Writer::never(format!("its writer didn't start ({e})"))
            }
        }
    }

    /// One with no thread, that takes nothing, and why (`main` before its
    /// launch's: no file; a thread that didn't start).
    pub(crate) fn never(why: String) -> Arc<Writer> {
        Arc::new(Writer {
            tx: Mutex::new(None),
            thread: Mutex::new(None),
            join: JOIN,
            never: Some(why),
            #[cfg(test)]
            queued: None,
        })
    }

    /// Why it has no thread and never will, if so.
    pub(crate) fn never_started(&self) -> Option<&str> {
        self.never.as_deref()
    }

    /// Tests: one that queues (type docs).
    #[cfg(test)]
    pub(crate) fn queued() -> Arc<Writer> {
        Arc::new(Writer { tx: Mutex::new(None), thread: Mutex::new(None), join: JOIN, never: None, queued: Some(Mutex::default()) })
    }

    /// Tests: a queuing writer's jobs run here, in order, until none is
    /// left or it is held. Any other: nothing.
    #[cfg(test)]
    pub(crate) fn drain(&self) {
        let Some(q) = &self.queued else { return };
        loop {
            let job = {
                let mut q = locked(q);
                if q.holds > 0 {
                    return;
                }
                q.jobs.pop_front()
            };
            match job {
                Some(job) => job(),
                None => return,
            }
        }
    }

    /// Tests: a queuing writer stops as a thread whose job panicked does:
    /// what it holds dropped, never run (their receivers close), and it
    /// takes nothing more.
    #[cfg(all(test, unix))]
    pub(crate) fn stop(&self) {
        if let Some(q) = &self.queued {
            let mut q = locked(q);
            q.stopped = true;
            q.jobs.clear();
        }
    }

    /// Tests: a queuing writer runs nothing until the hold goes (then
    /// [`Writer::drain`] runs what it holds).
    #[cfg(test)]
    pub(crate) fn hold(self: &Arc<Self>) -> Hold {
        if let Some(q) = &self.queued {
            locked(q).holds += 1;
        }
        Hold(self.clone())
    }

    /// `job` run after everything handed over before it, on the writer's
    /// thread; what it returns, once it ran (the receiver closed, nothing
    /// sent: the thread stopped before it ran it). A writer that never
    /// started, stopped or shut down takes nothing: why, and `job` dropped,
    /// never run.
    pub(crate) fn run<R: Send + 'static>(&self, job: impl FnOnce() -> R + Send + 'static) -> Result<async_channel::Receiver<R>, Stopped> {
        let (tx, rx) = async_channel::bounded(1);
        let job: Job = Box::new(move || {
            let _ = tx.try_send(job());
        });
        #[cfg(test)]
        if let Some(q) = &self.queued {
            let mut q = locked(q);
            if q.stopped {
                return Err(Stopped("its writer stopped".into()));
            }
            q.jobs.push_back(job);
            return Ok(rx);
        }
        match &*locked(&self.tx) {
            Some(q) => q.send(job).map(|()| rx).map_err(|_| Stopped("its writer stopped".into())),
            None => Err(Stopped(self.never.clone().unwrap_or_else(|| "its writer shut down".into()))),
        }
    }

    /// Its queue closed, and waited for its bound ([`JOIN`]) at most while
    /// its thread runs what it holds: `true` if it ended (joined). Again:
    /// nothing (`true`).
    pub(crate) fn shutdown(&self) -> bool {
        #[cfg(test)]
        if let Some(q) = &self.queued {
            let jobs = std::mem::take(&mut locked(q).jobs);
            jobs.into_iter().for_each(|job| job());
            return true;
        }
        drop(locked(&self.tx).take());
        let Some((thread, done)) = locked(&self.thread).take() else { return true };
        match done.recv_timeout(self.join) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => thread.join().is_ok(),
            Err(RecvTimeoutError::Timeout) => false,
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shutdown();
    }
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

/// Tests: the file at `path` owned once it's free, within 10 s. (A lock
/// just let go may still be held a moment: a process another test spawns
/// holds a copy of it until it execs.)
#[cfg(test)]
pub(crate) fn own_once_free(path: &Path) -> Result<Owned, NotOwned> {
    let t = std::time::Instant::now();
    loop {
        match own(path) {
            Err(NotOwned::Held) if t.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(5)),
            o => return o,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer's thread runs what it is handed in order, off the
    /// caller's thread; its shutdown runs what it holds and joins; handed
    /// anything after, it takes nothing and the job never runs (Shady #103
    /// R4-P2-2: never in place).
    #[test]
    fn the_writer_runs_in_order_and_its_shutdown_drains_it() {
        let w = Writer::start();
        let here = std::thread::current().id();
        let (go, wait) = channel::<()>();
        let first = w
            .run(move || {
                let _ = wait.recv_timeout(Duration::from_secs(5));
                std::thread::current().id()
            })
            .expect("taken");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let after: Vec<_> = (0..8)
            .map(|k| {
                let seen = seen.clone();
                w.run(move || locked(&seen).push(k)).expect("taken")
            })
            .collect();
        assert!(locked(&seen).is_empty(), "behind the first");
        go.send(()).unwrap();
        assert_ne!(first.recv_blocking().unwrap(), here, "off the caller's thread");
        assert!(w.shutdown(), "drained and joined within JOIN");
        assert!(after.iter().all(|r| r.recv_blocking().is_ok()));
        assert_eq!(*locked(&seen), (0..8).collect::<Vec<_>>(), "in order");
        let ran = Arc::new(Mutex::new(false));
        let late = {
            let ran = ran.clone();
            w.run(move || *locked(&ran) = true)
        };
        assert_eq!(late.err(), Some(Stopped("its writer shut down".into())), "shut down: taken by no one");
        assert!(!*locked(&ran), "never run in place");
        assert!(w.shutdown(), "again: nothing");
    }

    /// Shady #103 R4-P2-2: a writer whose thread didn't start takes no job
    /// (it never runs, on the caller's thread or any), says why, and its
    /// shutdown has nothing to wait for.
    #[test]
    fn a_writer_whose_thread_didnt_start_takes_nothing() {
        let w = Writer::start_with(JOIN, |_| Err(std::io::Error::other("no threads left")));
        assert_eq!(w.never_started(), Some("its writer didn't start (no threads left)"));
        let ran = Arc::new(Mutex::new(false));
        let job = {
            let ran = ran.clone();
            w.run(move || *locked(&ran) = true)
        };
        assert_eq!(job.err(), Some(Stopped("its writer didn't start (no threads left)".into())));
        assert!(!*locked(&ran), "never run");
        let t = std::time::Instant::now();
        assert!(w.shutdown());
        assert!(t.elapsed() < Duration::from_millis(500), "nothing waited for");
    }

    /// Shady #103 R4-P2-2/-3: a shutdown whose writer is stuck waits its
    /// bound, not longer, and returns; the job it still holds keeps its
    /// file's lock until it ran, and then runs.
    #[test]
    fn a_shutdown_that_waits_in_vain_returns_and_the_job_keeps_its_lock() {
        let d = std::env::temp_dir().join(format!("synaps-desktop-blockfiles-vain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("x.json");
        let owned = own_once_free(&path).expect("free");
        let w = Writer::start_with(Duration::from_millis(100), |f| std::thread::Builder::new().spawn(f));
        let (go, wait) = channel::<()>();
        let job = {
            let owned = owned.clone();
            let path = path.clone();
            w.run(move || {
                let _ = wait.recv_timeout(Duration::from_secs(10));
                let done = write(&path, "{}");
                drop(owned);
                done
            })
            .expect("taken")
        };
        drop(owned);
        let t = std::time::Instant::now();
        assert!(!w.shutdown(), "stuck: not joined");
        let waited = t.elapsed();
        assert!(waited >= Duration::from_millis(100) && waited < Duration::from_secs(2), "its bound: {waited:?}");
        assert_eq!(own(&path).err(), Some(NotOwned::Held), "the job still holds the lock");
        assert!(!path.exists(), "not run yet");
        go.send(()).unwrap();
        assert_eq!(job.recv_blocking().unwrap(), Ok(()), "it ran, detached");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        let free = own_once_free(&path);
        assert!(free.is_ok(), "then free ({:?})", free.err());
    }

    /// A write's ownership outlives its store's: the lock stays taken while
    /// the writer still holds a job carrying it (Shady #103 R3-P2-3). (A
    /// shutdown that waits in vain: the test after the next.)
    #[test]
    fn a_job_holding_the_lock_keeps_it_until_it_ran() {
        let d = std::env::temp_dir().join(format!("synaps-desktop-blockfiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("x.json");
        let owned = own_once_free(&path).expect("free");
        let w = Writer::start();
        let (go, wait) = channel::<()>();
        let job = {
            let owned = owned.clone();
            let path = path.clone();
            w.run(move || {
                let _ = wait.recv_timeout(Duration::from_secs(5));
                let done = write(&path, "{}");
                drop(owned);
                done
            })
            .expect("taken")
        };
        drop(owned);
        assert_eq!(own(&path).err(), Some(NotOwned::Held), "the job out holds it");
        go.send(()).unwrap();
        assert_eq!(job.recv_blocking().unwrap(), Ok(()));
        assert!(w.shutdown());
        let free = own_once_free(&path);
        assert!(free.is_ok(), "the job ran: free ({:?})", free.err());
    }
}
