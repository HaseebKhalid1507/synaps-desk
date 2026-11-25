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
//!   first time it is drawn or entered, and kept while it is open: a
//!   choice remembered meanwhile never moves a highlight or a mark under
//!   the user.
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
//!   written whole, atomically (a temporary file, synced, renamed), off
//!   the UI thread and never from a frame. A file that doesn't parse is
//!   left for the next write to replace: the desk starts empty and the
//!   log says why. Two desk processes write whole files: the last write
//!   wins.
//! - **The setting** `cards.remember` (Settings, THIS APP: "Remember card
//!   choices", on unless `false` or `"off"`): off, the file is neither
//!   read nor written, and nothing is marked. "Forget remembered choices"
//!   empties it and removes the file, on or off.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use cards_spec::sanitize::fold_key;
use cards_spec::{CardDoc, Reply};
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

/// Open cards whose recall is fixed, at most (then it starts over).
const SEEN_CAP: usize = 1024;

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

/// Read the file at `path`: what it keeps, and why it was not used, if it
/// wasn't (missing is not a reason: empty).
fn read(path: &Path) -> (Store, Option<String>) {
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Store::default(), None),
        Err(e) => Err(e.to_string()),
    };
    match text.and_then(|t| Store::parse(&t)) {
        Ok((store, 0)) => (store, None),
        Ok((store, bad)) => {
            log::warn!(target: "synaps_desktop::card", "card choices {}: {bad} unreadable entries left out", path.display());
            (store, None)
        }
        Err(why) => {
            log::warn!(target: "synaps_desktop::card", "card choices {}: unreadable ({why}); starting empty", path.display());
            (Store::default(), Some(why))
        }
    }
}

/// Every write's number, across the process: a later one is a newer
/// snapshot.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Per file, the newest snapshot written: an older write that comes after
/// it does nothing.
static WRITTEN: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Mutex::default);

/// Write snapshot `generation` to `path`, or remove the file (`None`).
/// Atomic: a temporary file of this process beside it, synced, renamed
/// over it (and on Unix the folder synced); the temporary file never
/// outlives a failure. One write at a time, and never an older snapshot
/// over a newer one. Off the UI thread ([`AppView::save_recall`]).
pub(crate) fn write(path: &Path, generation: u64, text: Option<&str>) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    if written.get(path).is_some_and(|g| *g >= generation) {
        return Ok(());
    }
    match text {
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        },
        Some(text) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension(format!("json.{}.{generation}.tmp", std::process::id()));
            let done = (|| {
                let mut o = std::fs::OpenOptions::new();
                o.write(true).create(true).truncate(true);
                #[cfg(unix)]
                std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
                let mut f = o.open(&tmp)?;
                f.write_all(text.as_bytes())?;
                f.sync_all()?;
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
        }
    }
    written.insert(path.to_path_buf(), generation);
    Ok(())
}

/// The window's memory of card choices.
pub(crate) struct Recall {
    on: bool,
    /// The file (`None`: none, kept for this run only).
    path: Option<PathBuf>,
    store: Store,
    /// What each open card recalled at first sight, by session and call:
    /// fixed while it is open.
    seen: RefCell<HashMap<(String, String), Vec<usize>>>,
    /// Why the file was not used when read, if it wasn't (the log says so).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) said: Option<String>,
    /// Reads and writes of the file started: off, neither.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) reads: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) writes: u32,
    /// The write on its way: a newer snapshot, or turning it off, drops it.
    save: Option<Task<()>>,
}

impl Recall {
    /// On or off, its file: read now if on.
    pub(crate) fn new(on: bool, path: Option<PathBuf>) -> Self {
        let mut r = Recall { on, path, store: Store::default(), seen: RefCell::default(), said: None, reads: 0, writes: 0, save: None };
        if on {
            r.load();
        }
        r
    }

    fn load(&mut self) {
        let Some(path) = self.path.as_deref() else { return };
        self.reads += 1;
        (self.store, self.said) = read(path);
    }

    pub(crate) fn on(&self) -> bool {
        self.on
    }

    pub(crate) fn len(&self) -> usize {
        self.store.len()
    }

    /// Turned on (the file read) or off (what was read let go, a write on
    /// its way dropped). Open cards recall again; an entered card's
    /// highlight stays where it is.
    pub(crate) fn set_on(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        self.on = on;
        self.seen.borrow_mut().clear();
        self.store = Store::default();
        self.said = None;
        if on {
            self.load();
        } else {
            self.save = None;
        }
    }

    /// The options of card `tool` of `session` (folder `cwd`, `doc`) that
    /// were chosen last time, in its order: read at its first sight, then
    /// kept while it is open.
    pub(crate) fn at(&self, session: &str, tool: &str, cwd: Option<&str>, doc: &CardDoc) -> Vec<usize> {
        let Some(cwd) = cwd.filter(|_| self.on) else { return Vec::new() };
        let k = (session.to_string(), tool.to_string());
        if let Some(v) = self.seen.borrow().get(&k) {
            return v.clone();
        }
        let v = Asked::of(cwd, doc).and_then(|a| self.store.get(&a.key).map(|p| a.options(p, doc))).unwrap_or_default();
        let mut seen = self.seen.borrow_mut();
        if seen.len() >= SEEN_CAP {
            seen.clear();
        }
        seen.insert(k, v.clone());
        v
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

    /// Forget every choice: open cards recall nothing more.
    pub(crate) fn forget(&mut self) {
        self.store = Store::default();
        self.seen.borrow_mut().clear();
    }

    /// The file's next snapshot: its path, number and text (`None`: the
    /// file goes, nothing being kept).
    fn snapshot(&self) -> Option<(PathBuf, u64, Option<String>)> {
        let path = self.path.clone()?;
        let text = (!self.store.is_empty()).then(|| self.store.to_text());
        Some((path, GENERATION.fetch_add(1, Ordering::SeqCst) + 1, text))
    }
}

impl AppView {
    /// The options of the open card `painted`, in this pane, that the user
    /// chose last time this question came in this folder, in the doc's
    /// order: marked "last time", never picked.
    pub(crate) fn recalled(&self, painted: &Painted) -> Vec<usize> {
        let l = &self.model.lives[self.panes.cur.live];
        let Some(a) = l.active.as_ref() else { return Vec::new() };
        self.recall.at(&a.session_id, &painted.tool_id, l.cwd(), &painted.doc)
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

    /// The memory's file, written as it stands off the UI thread (removed
    /// when it keeps nothing).
    pub(crate) fn save_recall(&mut self, cx: &mut Context<Self>) {
        let Some((path, generation, text)) = self.recall.snapshot() else { return };
        self.recall.writes += 1;
        self.recall.save = Some(cx.spawn(async move |_, cx| {
            let shown = path.display().to_string();
            let done = cx.background_spawn(async move { write(&path, generation, text.as_deref()) }).await;
            if let Err(e) = done {
                log::warn!(target: "synaps_desktop::card", "card choices {shown}: not saved ({e})");
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
    /// file goes (on or off: the user asked).
    pub(crate) fn forget_card_choices(&mut self, cx: &mut Context<Self>) {
        self.recall.forget();
        self.recall.save = None;
        log::info!(target: "synaps_desktop::card", "card choices: forgotten");
        self.save_recall(cx);
        cx.notify();
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
