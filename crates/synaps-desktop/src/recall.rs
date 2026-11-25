//! Desk-local memory of agent-card choices, "smart defaults" (synaps-cards
//! `docs/design.md` v1.2: §9.3's D4 row, §10 decision 4, §6.1, §5).
//!
//! When the same question comes again in the same folder, the card starts
//! with the user's last choice highlighted. Nothing more:
//! - **Highlight only.** The remembered option takes the default's place
//!   as the starting highlight of an entered card ([`AppView::recall_start`],
//!   §6.1: Enter takes the highlighted option), and it is marked "last
//!   time" ([`AppView::recalled`]). It never picks, ticks or fills
//!   anything: a form's picks and fields start on the agent's default and
//!   prefills, as they do without it. Enter on it is a user's press, one
//!   `Submit` or `Steer` like any other; nothing here sends, answers or
//!   counts as an answer (§10 decision 4: no auto-answer, no "Always").
//! - **Fixed at first sight.** What a card recalls is read once, the
//!   first time it is drawn or entered, and kept while it is open (bound,
//!   not answered, closed or replaced; behind a tab counts): a choice
//!   remembered meanwhile, the setting turned off or on, other cards come
//!   and gone, a second pane on its session never move a highlight or a
//!   mark under the user. Only Forget clears it.
//! - **The same question** ([`Asked`]): the session's folder as the daemon
//!   reports it (`SessionMeta.cwd`), the question (whitespace collapsed,
//!   case-folded as the spec folds labels), whether it is a multi pick,
//!   and its set of option labels (case-folded, in any order). A card
//!   whose options changed is another question: nothing is recalled. Not
//!   the whole doc's digest: the detail, hints, default, fields and `wait`
//!   change from one ask to the next, and the choice is about the options.
//! - **What is kept** ([`Store`]): per question, a 64-bit key (SHA-256 of
//!   the above, cut) and the ids of the options chosen (their places in
//!   the sorted label set; for a multi pick, the set). Never the folder,
//!   the question, a label, a note, a field's text or a check. Only an
//!   answer's choice: a skip, a card without options, a choice whose
//!   label isn't the doc's, keeps nothing.
//! - **When.** Only once D2a's memo says the answer reached the chat
//!   (answered, or taken in: [`crate::deliver::Memos::take_reached`]),
//!   never on the press: an answer that wasn't sent, or waits on Resend,
//!   is not remembered.
//! - **Where.** `card-choices.json`, next to `settings.json` ([`path`]):
//!   at most [`CAP`] questions, the least recently answered dropped
//!   first. Read once at launch (and when the setting is turned on);
//!   written whole, atomically (a new temporary file, synced, renamed),
//!   off the UI thread and never from a frame; this user's only from its
//!   creation ([`create_private`]: mode 0600, or on Windows a protected
//!   DACL of the user's SID alone). A file that doesn't parse is left for
//!   the next write to replace: the desk starts empty and the log says
//!   why. Two desk processes write whole files: the last write wins.
//! - **The setting** `cards.remember` (Settings, THIS APP: "Remember card
//!   choices", on unless `false` or `"off"`): off, the file is neither
//!   read nor written, a save not begun is dropped, and a card first seen
//!   off marks nothing (one already open keeps what it showed).
//! - **Forget** ("Forget remembered choices", on or off) empties the
//!   memory and removes the file. Accepted at the click, it runs to its
//!   end on a task of its own: after a save already writing, never undone
//!   by an older save, Off or a closed window ([`accept_forget`],
//!   [`remove`]). Settings offers it while anything may be there, and
//!   says so if the removal failed. It is this desk process's: another
//!   one running keeps what it read, and may write it back.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use cards_spec::sanitize::fold_key;
use cards_spec::{CardDoc, CardId, Reply};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::app::AppView;
use crate::deliver::Painted;

/// The settings file's key: on unless `false` or `"off"`.
pub(crate) const SETTING: &str = "cards.remember";

/// The memory's file, next to `settings.json`.
pub(crate) const FILE: &str = "card-choices.json";

/// Questions remembered at most: past it, the least recently answered go.
pub(crate) const CAP: usize = 300;

/// The file's format.
const VERSION: u64 = 1;

/// `cards.remember` as the settings file has it: on unless `false` or
/// `"off"`.
pub(crate) fn enabled(v: Option<&Value>) -> bool {
    !(matches!(v, Some(Value::Bool(false))) || v.and_then(Value::as_str) == Some("off"))
}

/// The memory's file: next to `settings.json` (`None`: no config folder).
pub(crate) fn path() -> Option<PathBuf> {
    crate::theme::settings_path().map(|p| p.with_file_name(FILE))
}

/// A card's question as memory knows it: its key, and its option labels
/// (case-folded, sorted), whose places are the ids a choice is kept by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    pub(crate) key: String,
    labels: Vec<String>,
}

impl Asked {
    /// The question `doc` asks in folder `cwd`; `None` for a card with no
    /// options (nothing to recall) or no folder.
    pub(crate) fn of(cwd: &str, doc: &CardDoc) -> Option<Self> {
        if doc.options.is_empty() || cwd.is_empty() {
            return None;
        }
        let question = fold_key(&doc.question.split_whitespace().collect::<Vec<_>>().join(" "));
        let mut labels: Vec<String> = doc.options.iter().map(|o| fold_key(&o.label)).collect();
        labels.sort();
        // A JSON array: each part delimited, whatever it holds.
        let pre = json!(["synaps-desk card choices", VERSION, cwd, question, doc.multi, &labels]).to_string();
        let h = <sha2::Sha256 as sha2::Digest>::digest(pre.as_bytes());
        let key = h[..8].iter().map(|b| format!("{b:02x}")).collect();
        Some(Asked { key, labels })
    }

    /// What `reply` chose, as kept: its options' ids, sorted. `None`:
    /// nothing to keep (a skip, no choice, a label not the doc's).
    pub(crate) fn pick(&self, reply: &Reply) -> Option<Vec<u8>> {
        let (Reply::Answer(a) | Reply::Change(a)) = reply else { return None };
        let mut ids = Vec::new();
        for label in a.chosen() {
            let l = fold_key(label);
            ids.push(u8::try_from(self.labels.iter().position(|x| *x == l)?).ok()?);
        }
        ids.sort_unstable();
        ids.dedup();
        (!ids.is_empty()).then_some(ids)
    }

    /// Kept ids `pick` as `doc`'s options, in its order; none if one isn't
    /// there, or a one-pick card would get several.
    pub(crate) fn options(&self, pick: &[u8], doc: &CardDoc) -> Vec<usize> {
        let mut out = Vec::new();
        for &i in pick {
            let Some(l) = self.labels.get(usize::from(i)) else { return Vec::new() };
            match doc.options.iter().position(|o| fold_key(&o.label) == *l) {
                Some(at) => out.push(at),
                None => return Vec::new(),
            }
        }
        if !doc.multi && out.len() != 1 {
            return Vec::new();
        }
        out.sort_unstable();
        out
    }
}

/// The choices kept, least recently answered first: a key (16 hex
/// digits) and the ids chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Store {
    entries: Vec<(String, Vec<u8>)>,
}

fn is_key(k: &str) -> bool {
    k.len() == 16 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Store {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn get(&self, key: &str) -> Option<&[u8]> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, p)| p.as_slice())
    }

    /// `key` answered `pick`: the most recent now; past [`CAP`], the least
    /// recent goes.
    pub(crate) fn put(&mut self, key: String, pick: Vec<u8>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push((key, pick));
        if self.entries.len() > CAP {
            let over = self.entries.len() - CAP;
            self.entries.drain(..over);
        }
    }

    /// The file's text: `{"version": 1, "choices": [{"key", "pick"}, …]}`,
    /// least recent first.
    pub(crate) fn to_text(&self) -> String {
        let rows: Vec<String> = self.entries.iter().map(|(k, p)| format!("    {}", json!({"key": k, "pick": p}))).collect();
        format!("{{\n  \"version\": {VERSION},\n  \"choices\": [\n{}\n  ]\n}}\n", rows.join(",\n"))
    }

    /// The file's text read back. An entry that isn't a key and ids is
    /// left out (counted); a file of another shape or version is an error.
    pub(crate) fn parse(text: &str) -> Result<(Store, usize), String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        if v.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return Err(format!("not version {VERSION}"));
        }
        let list = v.get("choices").and_then(Value::as_array).ok_or_else(|| "no choices list".to_string())?;
        let (mut store, mut bad) = (Store::default(), 0);
        for e in list {
            let key = e.get("key").and_then(Value::as_str).filter(|k| is_key(k));
            let pick = e.get("pick").and_then(Value::as_array).and_then(|a| {
                a.iter()
                    .map(|n| n.as_u64().filter(|n| *n < cards_spec::OPTIONS_MAX as u64).map(|n| n as u8))
                    .collect::<Option<Vec<u8>>>()
                    .filter(|p| !p.is_empty())
            });
            match (key, pick) {
                (Some(k), Some(mut p)) => {
                    p.sort_unstable();
                    p.dedup();
                    store.put(k.to_string(), p);
                }
                _ => bad += 1,
            }
        }
        Ok((store, bad))
    }
}

/// Read the file at `path`: what it keeps, why it was not used, if it
/// wasn't (missing is not a reason: empty), and whether something is
/// there (an unreadable file too).
fn read(path: &Path) -> (Store, Option<String>, bool) {
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Store::default(), None, false),
        Err(e) => Err(e.to_string()),
    };
    match text.and_then(|t| Store::parse(&t)) {
        Ok((store, 0)) => (store, None, true),
        Ok((store, bad)) => {
            log::warn!(target: "synaps_desktop::card", "card choices {}: {bad} unreadable entries left out", path.display());
            (store, None, true)
        }
        Err(why) => {
            log::warn!(target: "synaps_desktop::card", "card choices {}: unreadable ({why}); starting empty", path.display());
            (Store::default(), Some(why), true)
        }
    }
}

/// Every snapshot's and every Forget's number, across the process: a
/// later one is newer.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// Per file, the newest snapshot that reached it (written, or the file
/// removed). Held across each write, so one write at a time: an older
/// snapshot that comes after a newer one does nothing.
static WRITTEN: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Mutex::default);

/// Per file, the newest Forget accepted: a save numbered before it never
/// writes, whenever it runs. Held only for a look, never across a write:
/// the UI thread raises it at the click, never waiting on the disk.
static FORGOTTEN: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Mutex::default);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A Forget of `path` accepted (on the UI thread): its number. From now
/// on no save numbered before it writes the file, one waiting or one
/// cancelled too late; its removal ([`remove`]) runs after a save already
/// writing.
pub(crate) fn accept_forget(path: &Path) -> u64 {
    let generation = next_generation();
    let mut f = lock(&FORGOTTEN);
    let at = f.entry(path.to_path_buf()).or_default();
    *at = (*at).max(generation);
    generation
}

/// Write snapshot `generation` to `path` (true), unless memory was turned
/// off before it began (`stop`), a Forget came after it, or a newer
/// snapshot is there (false). Atomic: a new temporary file of this
/// process beside it, this user's only ([`create_private`]), synced,
/// renamed over it (and on Unix the folder synced); the temporary file
/// never outlives a failure. One write at a time. Off the UI thread
/// ([`AppView::save_recall`]).
pub(crate) fn save(path: &Path, generation: u64, text: &str, stop: &AtomicBool) -> std::io::Result<bool> {
    save_then(path, generation, text, stop, || {})
}

/// [`save`], `between` run once the temporary file is written, before the
/// rename, the write's lock held (tests: a save caught mid-write).
pub(crate) fn save_then(path: &Path, generation: u64, text: &str, stop: &AtomicBool, between: impl FnOnce()) -> std::io::Result<bool> {
    use std::io::Write as _;
    let mut written = lock(&WRITTEN);
    let forgotten = lock(&FORGOTTEN).get(path).is_some_and(|f| *f > generation);
    if forgotten || stop.load(Ordering::SeqCst) || written.get(path).is_some_and(|g| *g >= generation) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.{generation}.tmp", std::process::id()));
    // One left by a crash goes first: the new one is created, never opened.
    let _ = std::fs::remove_file(&tmp);
    let done = (|| {
        let mut f = create_private(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        between();
        std::fs::rename(&tmp, path)
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done?;
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    written.insert(path.to_path_buf(), generation);
    Ok(true)
}

/// Remove the file at `path`: Forget `generation`'s removal. After a save
/// already writing (the lock), never cancelled (its task is its own). A
/// snapshot newer than the Forget already there is left: it holds only
/// what was chosen after it.
pub(crate) fn remove(path: &Path, generation: u64) -> std::io::Result<()> {
    let mut written = lock(&WRITTEN);
    if written.get(path).is_some_and(|g| *g >= generation) {
        return Ok(());
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    written.insert(path.to_path_buf(), generation);
    Ok(())
}

/// A new file at `path`, this user's only, created (never an existing
/// file opened, nor a link followed): on Unix mode 0600; on Windows a
/// protected DACL with one entry, the current user's SID, full access
/// (nothing inherited from the folder), set as it is created
/// (`recall_win.rs`). No such file, no write.
pub(crate) fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        win::create_private(path)
    }
    #[cfg(not(windows))]
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        o.open(path)
    }
}

#[cfg(windows)]
#[path = "recall_win.rs"]
pub(crate) mod win;

/// A drawn card, as what it recalled is kept: its session, call and id
/// (its binding).
pub(crate) type Sight = (String, String, CardId);

/// Cards seen kept before the first sweep of those no longer open
/// ([`Recall::at`]).
const SWEEP: usize = 64;

/// The window's memory of card choices.
pub(crate) struct Recall {
    on: bool,
    /// The file (`None`: none, kept for this run only).
    path: Option<PathBuf>,
    store: Store,
    /// What each card recalled at first sight: kept while it is open,
    /// whatever else comes (the setting turned, other cards seen, a choice
    /// remembered). Swept of cards closed, answered or gone
    /// ([`AppView::recall_open`]) once it holds [`Recall::sweep_at`].
    seen: RefCell<HashMap<Sight, Vec<usize>>>,
    sweep_at: Cell<usize>,
    /// Why the file was not used when read, if it wasn't (the log says so).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) said: Option<String>,
    /// Reads and writes of the file started: off, neither.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) reads: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) writes: u32,
    /// The save on its way: a newer snapshot drops it, as turning it off
    /// does. Never a Forget's removal: that runs on its own.
    save: Option<Task<()>>,
    /// Set when memory is turned off: a save not begun by then writes
    /// nothing (one already writing finishes: a write can't be recalled).
    stop: Arc<AtomicBool>,
    /// The file may be there: it was when read (whatever it held), or a
    /// save started since; not once a Forget removed it.
    on_disk: bool,
    /// The newest save's number, and the newest Forget's.
    saved: u64,
    forgot: u64,
    /// Why the last Forget could not remove the file, until one does.
    pub(crate) failed: Option<String>,
}

impl Recall {
    /// On or off, its file: read now if on.
    pub(crate) fn new(on: bool, path: Option<PathBuf>) -> Self {
        let mut r = Recall {
            on,
            path,
            store: Store::default(),
            seen: RefCell::default(),
            sweep_at: Cell::new(SWEEP),
            said: None,
            reads: 0,
            writes: 0,
            save: None,
            stop: Arc::default(),
            on_disk: false,
            saved: 0,
            forgot: 0,
            failed: None,
        };
        if on {
            r.load();
        }
        r
    }

    fn load(&mut self) {
        let Some(path) = self.path.as_deref() else { return };
        self.reads += 1;
        let there;
        (self.store, self.said, there) = read(path);
        self.on_disk |= there;
    }

    pub(crate) fn on(&self) -> bool {
        self.on
    }

    /// Questions remembered (tests).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.store.len()
    }

    /// Settings offers Forget: off (the file unread, unknown), something
    /// remembered, the file there (an unreadable one too), or the last
    /// Forget failed.
    pub(crate) fn offers_forget(&self) -> bool {
        !self.on || !self.store.is_empty() || self.on_disk || self.failed.is_some()
    }

    /// Turned on (the file read) or off (what was read let go, a save not
    /// begun stopped; never a Forget). What open cards recalled stays:
    /// one first seen off recalls nothing, on again too.
    pub(crate) fn set_on(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        self.on = on;
        self.store = Store::default();
        self.said = None;
        if on {
            self.load();
        } else {
            self.save = None;
            self.stop.store(true, Ordering::SeqCst);
            self.stop = Arc::default();
        }
    }

    /// The options of card `card` (folder `cwd`, `doc`) that were chosen
    /// last time, in its order: read at its first sight (nothing, off),
    /// then kept while it is open. `open` says if a card kept is still
    /// open, for the sweep.
    pub(crate) fn at(&self, card: &Sight, cwd: Option<&str>, doc: &CardDoc, open: impl Fn(&Sight) -> bool) -> Vec<usize> {
        if let Some(v) = self.seen.borrow().get(card) {
            return v.clone();
        }
        let v = cwd
            .filter(|_| self.on)
            .and_then(|cwd| Asked::of(cwd, doc))
            .and_then(|a| self.store.get(&a.key).map(|p| a.options(p, doc)))
            .unwrap_or_default();
        let mut seen = self.seen.borrow_mut();
        if seen.len() >= self.sweep_at.get() {
            seen.retain(|k, _| open(k));
            self.sweep_at.set((2 * seen.len()).max(SWEEP));
        }
        seen.insert(card.clone(), v.clone());
        v
    }

    /// Cards whose recall is kept (tests).
    #[cfg(test)]
    pub(crate) fn seen_len(&self) -> usize {
        self.seen.borrow().len()
    }

    /// An answer `reply` to `doc`, in folder `cwd`, reached the chat:
    /// its choice is remembered (true), if it has one to keep.
    pub(crate) fn remember(&mut self, cwd: &str, doc: &CardDoc, reply: &Reply) -> bool {
        if !self.on {
            return false;
        }
        let Some(asked) = Asked::of(cwd, doc) else { return false };
        let Some(pick) = asked.pick(reply) else { return false };
        self.store.put(asked.key, pick);
        true
    }

    /// Forget every choice: open cards recall nothing more (and keep
    /// that), a save on its way is dropped, and no older one writes
    /// after it. Its removal's path and number (`None`: no file).
    pub(crate) fn forget(&mut self) -> Option<(PathBuf, u64)> {
        self.store = Store::default();
        for v in self.seen.borrow_mut().values_mut() {
            v.clear();
        }
        self.save = None;
        let path = self.path.clone()?;
        let generation = accept_forget(&path);
        self.forgot = generation;
        Some((path, generation))
    }

    /// Forget `generation`'s removal is done: the file gone (unless a save
    /// since put one back), or why not. An older Forget's word is late.
    pub(crate) fn removed(&mut self, generation: u64, done: Result<(), String>) {
        if generation < self.forgot {
            return;
        }
        match done {
            Ok(()) => {
                self.on_disk &= self.saved > generation;
                self.failed = None;
            }
            Err(why) => self.failed = Some(why),
        }
    }

    /// The file's next snapshot: its path, number and text, and what stops
    /// it.
    fn snapshot(&mut self) -> Option<(PathBuf, u64, String, Arc<AtomicBool>)> {
        let path = self.path.clone()?;
        let generation = next_generation();
        self.saved = generation;
        self.on_disk = true;
        Some((path, generation, self.store.to_text(), self.stop.clone()))
    }
}

impl AppView {
    /// The options of the open card `painted`, in this pane, that the user
    /// chose last time this question came in this folder, in the doc's
    /// order: marked "last time", never picked.
    pub(crate) fn recalled(&self, painted: &Painted) -> Vec<usize> {
        let l = &self.model.lives[self.panes.cur.live];
        let Some(a) = l.active.as_ref() else { return Vec::new() };
        let card = (a.session_id.clone(), painted.tool_id.clone(), painted.id.clone());
        self.recall.at(&card, l.cwd(), &painted.doc, |c| self.recall_open(c))
    }

    /// Whether card `card` may still be open: open in a store that shows
    /// its session (bound to its call, not answered, closed or replaced),
    /// or its session behind a tab of a pane (unknown there: kept). Gone
    /// otherwise.
    pub(crate) fn recall_open(&self, (session, tool, id): &Sight) -> bool {
        let lives = &self.model.lives;
        let mut any = false;
        for l in lives.ids().into_iter().map(|i| &lives[i]).filter(|l| l.active.as_ref().is_some_and(|a| a.session_id == *session)) {
            any = true;
            let f = l.transcript.cards();
            let open = f.card(tool).is_some_and(|c| {
                c.state == cards_spec::fold::State::Open
                    && c.id.as_ref() == Some(id)
                    && f.bound(id).is_some_and(|b| b.tool_id == c.tool_id && b.at == c.at)
            });
            if open {
                return true;
            }
        }
        let behind = |t: &crate::tabs::Tabs| t.iter().any(|t| t.session.as_deref() == Some(session.as_str()));
        !any && (behind(&self.model.tabs) || self.panes.rest.iter().any(|p| behind(&p.parked.tabs)))
    }

    /// Where an entered card's highlight starts, if memory says: the first
    /// option recalled. Before the agent's default (§6.1); the caller falls
    /// back to it.
    pub(crate) fn recall_start(&self, painted: &Painted) -> Option<usize> {
        self.recalled(painted).first().copied()
    }

    /// Answers that reached the chat since the last look
    /// ([`crate::deliver::Memos::take_reached`]): each one's choice is
    /// remembered, if it still answers its card, then written off the UI
    /// thread. Off, nothing.
    pub(crate) fn recall_answers(&mut self, cx: &mut Context<Self>) {
        let reached = self.model.memos.take_reached();
        if reached.is_empty() || !self.recall.on() {
            return;
        }
        let mut changed = false;
        for r in reached {
            let lives = &self.model.lives;
            let Some(l) =
                lives.ids().into_iter().map(|id| &lives[id]).find(|l| l.active.as_ref().is_some_and(|a| a.session_id == r.session))
            else {
                continue;
            };
            let Some(card) = l.transcript.cards().bound(&r.card) else { continue };
            let ours =
                self.model.memos.entry(&r.session, card).is_some_and(|e| e.state == cards_spec::memo::State::InChat && e.text == r.text);
            let (true, Some(doc), Some(cwd)) = (ours, card.doc.as_ref(), l.cwd()) else { continue };
            if self.recall.remember(cwd, doc, &r.reply) {
                log::info!(target: "synaps_desktop::card", "card {}: its choice remembered", r.card);
                changed = true;
            }
        }
        if changed {
            self.save_recall(cx);
        }
    }

    /// The memory's file, written as it stands off the UI thread.
    pub(crate) fn save_recall(&mut self, cx: &mut Context<Self>) {
        let Some((path, generation, text, stop)) = self.recall.snapshot() else { return };
        self.recall.writes += 1;
        self.recall.save = Some(cx.background_spawn(async move {
            if let Err(e) = save(&path, generation, &text, &stop) {
                log::warn!(target: "synaps_desktop::card", "card choices {}: not saved ({e})", path.display());
            }
        }));
    }

    /// Settings' "Remember card choices": on or off, saved with THIS APP's
    /// keys.
    pub(crate) fn set_recall(&mut self, on: bool, cx: &mut Context<Self>) {
        self.recall.set_on(on);
        cx.notify();
    }

    /// Settings' "Forget remembered choices": the memory empties, and its
    /// file goes (on or off: the user asked). Accepted now: the removal
    /// runs to its end on a task of its own, after a save already writing,
    /// whatever comes next (Off, the window closed); Settings hears how it
    /// went.
    pub(crate) fn forget_card_choices(&mut self, cx: &mut Context<Self>) {
        let Some((path, generation)) = self.recall.forget() else {
            log::info!(target: "synaps_desktop::card", "card choices: forgotten");
            cx.notify();
            return;
        };
        let removal = cx.background_spawn(async move {
            let done = remove(&path, generation);
            match &done {
                Ok(()) => log::info!(target: "synaps_desktop::card", "card choices: forgotten, {} removed", path.display()),
                Err(e) => log::warn!(target: "synaps_desktop::card", "card choices {}: forgotten, not removed ({e})", path.display()),
            }
            done.map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let done = removal.await;
            let _ = this.update(cx, |v, cx| {
                v.recall.removed(generation, done);
                v.views_changed(cx);
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
