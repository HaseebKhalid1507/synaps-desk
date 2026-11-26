//! Files changed in a session, derived from its `edit` / `write` /
//! `multi_edit` tool calls (the full inputs in the history), counted once
//! their result came back as a success: the result's text is classified as
//! the Synaps TUI does ([`synaps_client::transcript::tool_outcome`]), since
//! the daemon leaves `is_error` unset on failed, denied and canceled calls.
//! Also the inline diff a tool row shows, keyed by tool id, from the history
//! or the live `ToolUse` event. No GPUI.
//!
//! Everything here is what the model asked for, not what landed on disk
//! (this client never reads the files): line counts are of the tool inputs
//! (`old_string` → `new_string`), a `write` is "N lines written" (`~N`, not a
//! delta: the old file is unknown), a `multi_edit` regex or line-prefix
//! anchor is shown as the pattern, `{{placeholders}}` are not expanded, and
//! an input an extension rewrote shows as the model sent it.
//!
//! Apart from those, and never added to their counts: the files successful
//! `bash` calls wrote, as far as their command text says ([`crate::shell`]),
//! without line counts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use synaps_client::transcript::{ToolOutcome, tool_result_outcome};

use crate::shell::{self, Touch};

/// Lines a preview keeps (the rest is counted in `more`).
pub const PREVIEW_LINES: usize = 48;
/// Characters a preview line keeps.
const LINE_CHARS: usize = 240;
/// Unchanged lines shown around a change.
const CONTEXT: usize = 2;
/// Above this many `old × new` cells the preview is one block (removed,
/// added): at most ~20 µs per edit, so folding a long history at attach
/// stays cheap. The counts do not depend on it ([`changed_lines`]).
const LCS_CELLS: usize = 20_000;
/// The work ([`changed_lines`]'s `(old + new) × D`) the exact count may take
/// above [`LCS_CELLS`] before it falls back to counting lines as a multiset.
const COUNT_WORK: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
    /// A `multi_edit` file header, or a gap between hunks.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Its leading whitespace, in columns (a tab is 4; at most
    /// [`INDENT_MAX`]): a wrapped line hangs there, never left of its code.
    pub indent: u16,
    /// After that whitespace. Shared, so a frame hands it to the renderer
    /// without a copy.
    pub text: Arc<str>,
}

/// The most of a line's indent a well keeps as a hang.
pub const INDENT_MAX: u16 = 24;

/// `line`'s leading whitespace in columns (a tab is 4, the hang at most
/// [`INDENT_MAX`]), and the rest: what a well hangs a wrapped line at.
pub fn hang(line: &str) -> (u16, &str) {
    let mut cols = 0u16;
    for (at, c) in line.char_indices() {
        let w = match c {
            ' ' => 1,
            '\t' => 4,
            _ => return (cols, &line[at..]),
        };
        if cols + w > INDENT_MAX {
            return (cols, &line[at..]);
        }
        cols += w;
    }
    (cols, "")
}

/// What a tool row draws under its header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// The file (relative to the session's folder when under it), or the
    /// first of several: `a.rs +2 files`.
    pub title: String,
    pub lines: Vec<DiffLine>,
    /// Lines not kept.
    pub more: usize,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// `edit` / `multi_edit`.
    Modified,
    /// `write` (created or replaced whole).
    Written,
}

impl FileKind {
    pub fn letter(self) -> &'static str {
        match self {
            FileKind::Modified => "M",
            FileKind::Written => "W",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    /// As the tool got it, relative to the session's folder when under it.
    pub path: String,
    /// `path`'s folder (`crates/relay/src/`, or empty) and file name,
    /// shared: a frame draws them without a copy.
    pub dir: Arc<str>,
    pub name: Arc<str>,
    /// How it was first touched.
    pub kind: FileKind,
    /// Lines the edits added and removed (not the writes).
    pub added: u32,
    pub removed: u32,
    /// Lines of the last `write` of it (0: never written whole).
    pub written: u32,
    /// `added` / `removed` include an estimate (an edit too large to
    /// count exactly, see [`changed_lines`]): shown with `~`.
    pub approx: bool,
}

/// A file a successful `bash` call wrote, by its command text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellFile {
    /// Relative to the session's folder when under it.
    pub path: String,
    /// As [`FileStat::dir`] and [`FileStat::name`].
    pub dir: Arc<str>,
    pub name: Arc<str>,
    /// The last way a command touched it ([`Touch::verb`]).
    pub verb: &'static str,
}

/// `("crates/relay/src/", "session.rs")`.
fn split_path(path: &str) -> (Arc<str>, Arc<str>) {
    let (dir, name) = match path.rfind(['/', '\\']) {
        Some(i) => path.split_at(i + 1),
        None => ("", path),
    };
    (dir.into(), name.into())
}

/// One file a tool call changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    path: String,
    kind: FileKind,
    added: u32,
    removed: u32,
    approx: bool,
}

/// One file tool call, analysed once.
#[derive(Debug)]
struct Analysed {
    /// When it was first seen: the calls' order.
    seq: u64,
    preview: Arc<Preview>,
    /// Per file.
    changes: Vec<Change>,
    /// A `multi_edit`'s changes per op (its index in `edits`): an op its
    /// `unless_present` skipped is left out once the result says so.
    ops: Vec<(usize, Change)>,
}

/// What one file tool call counted for once it succeeded: exactly what it
/// added to [`Ledger::files`] and [`Ledger::totals`], so a transcript row
/// and the FILES panel never disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallDelta {
    /// Edits: lines added and removed (`approx`: an estimate).
    pub added: u32,
    pub removed: u32,
    pub approx: bool,
    /// Writes: lines written (not a delta).
    pub written: u32,
}

/// An edit preview (blocks.md §1.5) keeps this many calls of a file, at
/// most this many lines in all, a line at most this many characters.
pub const EDIT_CALLS: usize = 12;
pub const EDIT_LINES: usize = 120;
pub const EDIT_LINE_CHARS: usize = 160;

/// A file's successful file tool calls, for its edit preview: the newest
/// [`EDIT_CALLS`] in call order, how many earlier ones there were, and the
/// preview they make. Kept as each call commits ([`Ledger::edits`]): no
/// preview reads the history, and a draw only takes the finished one.
#[derive(Clone, Debug, Default)]
pub struct Edits {
    pub recent: VecDeque<Edit>,
    pub earlier: usize,
    pub preview: Arc<EditPreview>,
}

/// One call's part of an edit preview: its diff preview (the call's, shared)
/// and, a `write`, the lines it wrote.
#[derive(Clone, Debug)]
pub struct Edit {
    seq: u64,
    pub preview: Arc<Preview>,
    pub written: Option<u32>,
}

/// An edit preview as drawn (§1.5): a group per call in call order, and
/// how many earlier calls folded away.
#[derive(Clone, Debug, Default)]
pub struct EditPreview {
    pub groups: Vec<EditGroup>,
    pub earlier: usize,
}

/// One call's group of an edit preview: its lines as requested.
#[derive(Clone, Debug)]
pub struct EditGroup {
    pub lines: Vec<DiffLine>,
    /// A `write`: "N lines written", no old side.
    pub written: Option<u32>,
}

#[cfg(test)]
thread_local! {
    /// Edit previews built ([`Edits::preview`]; tests: when they are).
    pub static EDIT_PREVIEWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Edits {
    /// The preview of its calls: the newest of them, at most
    /// [`EDIT_LINES`] lines in all, the oldest kept ones folding first.
    fn project(&self) -> EditPreview {
        #[cfg(test)]
        EDIT_PREVIEWS.with(|n| n.set(n.get() + 1));
        let mut earlier = self.earlier;
        let mut kept: Vec<&Edit> = self.recent.iter().collect();
        while kept.len() > 1 && kept.iter().map(|e| e.preview.lines.len()).sum::<usize>() > EDIT_LINES {
            kept.remove(0);
            earlier += 1;
        }
        let groups = kept
            .iter()
            .map(|e| {
                let lines = e
                    .preview
                    .lines
                    .iter()
                    .take(EDIT_LINES)
                    .map(|l| match l.text.chars().count() > EDIT_LINE_CHARS {
                        true => DiffLine { text: l.text.chars().take(EDIT_LINE_CHARS).collect::<String>().into(), ..l.clone() },
                        false => l.clone(),
                    })
                    .collect();
                EditGroup { lines, written: e.written }
            })
            .collect();
        EditPreview { groups, earlier }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Ledger {
    /// This session's ledger: another for every other session (a cache
    /// keyed by `(id, generation)` is never another session's).
    id: u64,
    files: Vec<FileStat>,
    index: HashMap<String, usize>,
    /// Tool calls waiting for their result.
    pending: HashSet<String>,
    /// Every file tool call seen, by tool id: kept across a reset of the
    /// same session (a resync, a reconnect), so its history is not diffed
    /// again.
    calls: HashMap<String, Arc<Analysed>>,
    /// Every `bash` call seen, by tool id (kept as `calls` is): the files
    /// it writes, `None` when it writes none (or cannot be read), so its
    /// command is lexed once, not again at every reset.
    shell_calls: HashMap<String, Option<Arc<Vec<Touch>>>>,
    shell: Vec<ShellFile>,
    shell_index: HashMap<String, usize>,
    /// Each file tool call that succeeded: what it counted for.
    committed: HashMap<String, CallDelta>,
    /// Bumped whenever `committed` changes (a reset included).
    generation: u64,
    /// Calls seen so far: the next call's `seq`.
    seq: u64,
    /// Each file's edits ([`Edits`]), by path.
    edits: HashMap<String, Edits>,
    cwd: Option<String>,
    added: u32,
    removed: u32,
    approx: bool,
}

/// Whether a tool changes files (its row gets a diff).
pub fn is_file_tool(name: &str) -> bool {
    matches!(name, "edit" | "write" | "multi_edit")
}

impl Ledger {
    /// Start over from a whole history (attach, replacement). `same`: the
    /// same session as before, whose analysed calls are reused.
    pub fn reset(&mut self, messages: &[Value], cwd: Option<&str>, same: bool) {
        static IDS: AtomicU64 = AtomicU64::new(1);
        let (calls, shell_calls) =
            if same { (std::mem::take(&mut self.calls), std::mem::take(&mut self.shell_calls)) } else { Default::default() };
        *self = Ledger {
            id: if same && self.id != 0 { self.id } else { IDS.fetch_add(1, Ordering::Relaxed) },
            cwd: cwd.map(|c| c.trim_end_matches(['/', '\\']).to_string()).filter(|c| !c.is_empty()),
            calls,
            shell_calls,
            generation: self.generation + 1,
            seq: self.seq,
            ..Ledger::default()
        };
        self.append(messages);
    }

    /// Fold history messages after the ones seen so far.
    pub fn append(&mut self, messages: &[Value]) {
        for m in messages {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else { continue };
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") => {
                        let (Some(id), Some(name)) = (b.get("id").and_then(Value::as_str), b.get("name").and_then(Value::as_str)) else {
                            continue;
                        };
                        let input = b.get("input").unwrap_or(&Value::Null);
                        self.tool_use(name, id, input);
                    }
                    Some("tool_result") => {
                        let Some(id) = b.get("tool_use_id").and_then(Value::as_str) else { continue };
                        if !self.pending.remove(id) || tool_result_outcome(b) != ToolOutcome::Ok {
                            continue;
                        }
                        if let Some(Some(touches)) = self.shell_calls.get(id).cloned() {
                            for t in touches.iter() {
                                self.commit_shell(t);
                            }
                        }
                        if let Some(call) = self.calls.get(id).cloned() {
                            let skipped =
                                if call.ops.is_empty() { Vec::new() } else { skipped_ops(b.get("content").unwrap_or(&Value::Null)) };
                            let mut delta = CallDelta::default();
                            let changes: Vec<&Change> = if skipped.is_empty() {
                                call.changes.iter().collect()
                            } else {
                                call.ops.iter().filter(|(i, _)| !skipped.contains(i)).map(|(_, c)| c).collect()
                            };
                            for c in &changes {
                                self.commit(c, &mut delta);
                            }
                            let written = changes.iter().all(|c| c.kind == FileKind::Written).then_some(call.preview.added);
                            let mut paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
                            paths.dedup();
                            for p in paths {
                                self.record(p, Edit { seq: call.seq, preview: call.preview.clone(), written });
                            }
                            self.committed.insert(id.to_string(), delta);
                            self.generation += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// A tool call: its preview now, its counts once it succeeds. Also for
    /// the live `ToolUse` event (the history repeats it; same id, no double count).
    pub fn tool_use(&mut self, name: &str, id: &str, input: &Value) {
        if name == "bash" && !id.is_empty() {
            if !self.shell_calls.contains_key(id) {
                let touches = shell::writes(str_of(input, "command")).filter(|t| !t.is_empty());
                let touches = touches.map(|t| Arc::new(t.into_iter().map(|t| Touch { path: self.relative(&t.path), ..t }).collect()));
                self.shell_calls.insert(id.to_string(), touches);
            }
            if matches!(self.shell_calls.get(id), Some(Some(_))) {
                self.pending.insert(id.to_string());
            }
            return;
        }
        if !is_file_tool(name) || id.is_empty() {
            return;
        }
        if !self.calls.contains_key(id) {
            let (mut preview, changes, ops) = analyse(name, input);
            let changes: Vec<Change> = changes.into_iter().map(|c| Change { path: self.relative(&c.path), ..c }).collect();
            let ops: Vec<(usize, Change)> = ops.into_iter().map(|(i, c)| (i, Change { path: self.relative(&c.path), ..c })).collect();
            preview.title = match changes.as_slice() {
                [] => String::new(),
                [one] => one.path.clone(),
                [first, rest @ ..] => format!("{} +{} files", first.path, rest.len()),
            };
            self.seq += 1;
            self.calls.insert(id.to_string(), Arc::new(Analysed { seq: self.seq, preview: Arc::new(preview), changes, ops }));
        }
        self.pending.insert(id.to_string());
    }

    /// A call that succeeded on `path`: its file's edits, the newest
    /// [`EDIT_CALLS`] in call order kept.
    fn record(&mut self, path: &str, e: Edit) {
        let f = self.edits.entry(path.to_string()).or_default();
        if f.recent.len() == EDIT_CALLS && f.recent.front().is_some_and(|o| o.seq > e.seq) {
            f.earlier += 1;
        } else {
            let at = f.recent.partition_point(|o| o.seq < e.seq);
            f.recent.insert(at, e);
            if f.recent.len() > EDIT_CALLS {
                f.recent.pop_front();
                f.earlier += 1;
            }
        }
        f.preview = Arc::new(f.project());
    }

    /// File `file`'s edits ([`Edits`]): its path as the ledger keeps it
    /// (relative to the session's folder), or one naming the same file
    /// (one the other's tail at a `/`).
    pub fn edits(&self, file: &str) -> Option<&Edits> {
        let rel = self.relative(file);
        self.edits.get(&rel).or_else(|| self.edits.iter().find(|(p, _)| same_file(p, &rel)).map(|(_, e)| e))
    }

    /// File `file`'s edit preview ([`Edits::preview`]), as it was built
    /// when its last call committed.
    pub fn edit_preview(&self, file: &str) -> Option<Arc<EditPreview>> {
        self.edits(file).map(|e| e.preview.clone())
    }

    /// Its identity (another for every other session's ledger, kept by a
    /// reset of the same session) and generation.
    #[cfg(test)]
    pub fn version(&self) -> (u64, u64) {
        (self.id, self.generation)
    }

    fn relative(&self, path: &str) -> String {
        match &self.cwd {
            Some(cwd) => match path.strip_prefix(cwd.as_str()).and_then(|r| r.strip_prefix(['/', '\\'])) {
                Some(rest) if !rest.is_empty() => rest.to_string(),
                _ => path.to_string(),
            },
            None => path.to_string(),
        }
    }

    fn commit(&mut self, c: &Change, delta: &mut CallDelta) {
        let i = match self.index.get(&c.path) {
            Some(&i) => i,
            None => {
                self.index.insert(c.path.clone(), self.files.len());
                let (dir, name) = split_path(&c.path);
                self.files.push(FileStat {
                    path: c.path.clone(),
                    dir,
                    name,
                    kind: c.kind,
                    added: 0,
                    removed: 0,
                    written: 0,
                    approx: false,
                });
                self.files.len() - 1
            }
        };
        let f = &mut self.files[i];
        match c.kind {
            FileKind::Written => {
                f.written = c.added;
                delta.written += c.added;
            }
            FileKind::Modified => {
                delta.added += c.added;
                delta.removed += c.removed;
                delta.approx |= c.approx;
                f.added += c.added;
                f.removed += c.removed;
                f.approx |= c.approx;
                self.added += c.added;
                self.removed += c.removed;
                self.approx |= c.approx;
            }
        }
    }

    fn commit_shell(&mut self, t: &Touch) {
        match self.shell_index.get(&t.path) {
            Some(&i) => self.shell[i].verb = t.verb,
            None => {
                self.shell_index.insert(t.path.clone(), self.shell.len());
                let (dir, name) = split_path(&t.path);
                self.shell.push(ShellFile { path: t.path.clone(), dir, name, verb: t.verb });
            }
        }
    }

    /// Files successful `bash` calls wrote, first touched first: apart
    /// from [`Ledger::files`], and never in [`Ledger::totals`].
    pub fn shell(&self) -> &[ShellFile] {
        &self.shell
    }

    /// The totals include an estimate ([`FileStat::approx`]).
    pub fn approx(&self) -> bool {
        self.approx
    }

    /// What a file tool call counted for; `None` until it succeeded (and
    /// never for a failed, denied, canceled or stopped one).
    pub fn delta(&self, tool_id: &str) -> Option<CallDelta> {
        self.committed.get(tool_id).copied()
    }

    /// Changes whenever a [`Ledger::delta`] may have.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What it keeps, roughly (bytes): the analysed calls' diff previews
    /// (the rows share them) and paths, the files and the call ids.
    pub fn bytes(&self) -> usize {
        let preview = |p: &Preview| {
            std::mem::size_of::<Preview>()
                + p.title.capacity()
                + p.lines.iter().map(|l| std::mem::size_of::<DiffLine>() + l.text.len() + 16).sum::<usize>()
        };
        let call = |(k, a): (&String, &Arc<Analysed>)| {
            k.capacity()
                + 64
                + preview(&a.preview)
                + a.changes.iter().chain(a.ops.iter().map(|(_, c)| c)).map(|c| c.path.capacity() + 48).sum::<usize>()
        };
        let file = |path: &String, dir: &Arc<str>, name: &Arc<str>| path.capacity() + dir.len() + name.len() + 96;
        self.calls.iter().map(call).sum::<usize>()
            + self.files.iter().map(|f| file(&f.path, &f.dir, &f.name)).sum::<usize>()
            + self.shell.iter().map(|f| file(&f.path, &f.dir, &f.name)).sum::<usize>()
            + (self.shell_calls.len() + self.committed.len() + self.pending.len()) * 64
            + self
                .edits
                .iter()
                .map(|(p, e)| {
                    let shown = e.preview.groups.iter().map(|g| g.lines.iter().map(|l| l.text.len() + 32).sum::<usize>() + 48);
                    p.capacity() + 64 + e.recent.len() * std::mem::size_of::<Edit>() + shown.sum::<usize>()
                })
                .sum::<usize>()
    }

    pub fn preview(&self, tool_id: &str) -> Option<Arc<Preview>> {
        self.calls.get(tool_id).map(|c| c.preview.clone())
    }

    /// Changed files, first touched first.
    pub fn files(&self) -> &[FileStat] {
        &self.files
    }

    /// Lines the edits added and removed (writes are not deltas).
    pub fn totals(&self) -> (u32, u32) {
        (self.added, self.removed)
    }
}

/// Two names of one file: equal, or one the other's tail at a `/`.
pub fn same_file(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches("./"), b.trim_start_matches("./"));
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn count_lines(s: &str) -> u32 {
    s.lines().count() as u32
}

/// The ops a `multi_edit` result says its `unless_present` skipped
/// (`[i] <path> <op> skipped: unless_present found at L<n>`, agent-engine
/// `tools/multi_edit/mod.rs`).
fn skipped_ops(content: &Value) -> Vec<usize> {
    let texts: Vec<&str> = match content {
        Value::String(s) => vec![s],
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect(),
        _ => Vec::new(),
    };
    texts
        .iter()
        .flat_map(|t| t.lines())
        .filter(|l| l.contains(" skipped: unless_present found at L"))
        .filter_map(|l| l.strip_prefix('[')?.split_once(']')?.0.parse().ok())
        .collect()
}

/// The preview, per-file changes and (`multi_edit`) per-op changes of one
/// tool input.
fn analyse(name: &str, input: &Value) -> (Preview, Vec<Change>, Vec<(usize, Change)>) {
    let mut p = Preview::default();
    let mut changes = Vec::new();
    let mut ops = Vec::new();
    match name {
        "edit" => {
            let path = str_of(input, "path");
            let (a, r, approx) = diff_into(&mut p, str_of(input, "old_string"), str_of(input, "new_string"));
            changes.push(Change { path: path.into(), kind: FileKind::Modified, added: a, removed: r, approx });
        }
        "write" => {
            let content = str_of(input, "content");
            let n = count_lines(content);
            for l in content.lines() {
                push(&mut p, LineKind::Add, l);
            }
            p.added += n;
            changes.push(Change { path: str_of(input, "path").into(), kind: FileKind::Written, added: n, removed: 0, approx: false });
        }
        "multi_edit" => {
            let mut last_path = "";
            for (i, e) in input.get("edits").and_then(Value::as_array).into_iter().flatten().enumerate() {
                let path = str_of(e, "path");
                if path != last_path {
                    push(&mut p, LineKind::Note, path);
                    last_path = path;
                }
                let text = str_of(e, "text");
                let (a, r, approx) = match (str_of(e, "op"), str_of(e, "anchor_mode")) {
                    ("replace", "" | "exact") => diff_into(&mut p, str_of(e, "anchor"), text),
                    // A pattern, not the text it replaces: show it as such.
                    ("replace", mode) => {
                        push(&mut p, LineKind::Note, &format!("{mode}: {}", str_of(e, "anchor")));
                        diff_into(&mut p, "", text)
                    }
                    ("append_jsonl", _) => {
                        push(&mut p, LineKind::Add, "{…}");
                        p.added += 1;
                        (1, 0, false)
                    }
                    _ => diff_into(&mut p, "", text),
                };
                match changes.iter_mut().find(|c: &&mut Change| c.path == path) {
                    Some(c) => {
                        c.added += a;
                        c.removed += r;
                        c.approx |= approx;
                    }
                    None => changes.push(Change { path: path.into(), kind: FileKind::Modified, added: a, removed: r, approx }),
                }
                ops.push((i, Change { path: path.into(), kind: FileKind::Modified, added: a, removed: r, approx }));
            }
        }
        _ => {}
    }
    (p, changes, ops)
}

fn push(p: &mut Preview, kind: LineKind, text: &str) {
    if p.lines.len() >= PREVIEW_LINES {
        p.more += 1;
        return;
    }
    let mut t: String = text.chars().take(LINE_CHARS).collect();
    if t.len() < text.len() {
        t.push('…');
    }
    let (indent, rest) = hang(&t);
    p.lines.push(DiffLine { kind, indent, text: rest.into() });
}

/// Append the line diff `old` → `new` to `p`; returns (added, removed,
/// whether those are an estimate).
fn diff_into(p: &mut Preview, old: &str, new: &str) -> (u32, u32, bool) {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if pre > CONTEXT {
        push(p, LineKind::Note, "…");
    }
    for l in &a[pre.saturating_sub(CONTEXT)..pre] {
        push(p, LineKind::Context, l);
    }
    let aligned = am.len() * bm.len() <= LCS_CELLS;
    if !aligned && !am.is_empty() && !bm.is_empty() {
        push(p, LineKind::Note, "too long to align: the old lines, then the new");
    }
    let ops = middle(am, bm);
    let (mut added, mut removed, mut approx) = (0, 0, false);
    for (kind, l) in ops {
        match kind {
            LineKind::Add => added += 1,
            LineKind::Del => removed += 1,
            _ => {}
        }
        push(p, kind, l);
    }
    if !aligned {
        (added, removed, approx) = changed_lines(am, bm);
    }
    for l in &a[a.len() - suf..][..suf.min(CONTEXT)] {
        push(p, LineKind::Context, l);
    }
    if suf > CONTEXT {
        push(p, LineKind::Note, "…");
    }
    p.added += added;
    p.removed += removed;
    (added, removed, approx)
}

/// Lines added and removed by the shortest edit `a` → `b`: Myers'
/// O((N+M)·D) greedy search, within [`COUNT_WORK`]; past it, the lines of
/// each that the other lacks, counted as multisets: a lower bound when
/// lines also move, so flagged as an estimate (the last value) unless the
/// two share no line at all (then every line was replaced: exact).
fn changed_lines(a: &[&str], b: &[&str]) -> (u32, u32, bool) {
    let (n, m) = (a.len(), b.len());
    let max_d = (COUNT_WORK / (n + m).max(1)).min(n + m);
    // `v[off + k]`: the furthest `x` on diagonal `k = x - y`.
    let off = max_d as isize + 1;
    let mut v = vec![0isize; 2 * max_d + 3];
    for d in 0..=max_d as isize {
        for k in (-d..=d).step_by(2) {
            let i = (off + k) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut y = x - k;
            while (x as usize) < n && (y as usize) < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x as usize >= n && y as usize >= m {
                // added - removed = m - n, added + removed = d.
                let d = d as usize;
                return (((d + m - n) / 2) as u32, ((d + n - m) / 2) as u32, false);
            }
        }
    }
    let mut count: HashMap<&str, i64> = HashMap::new();
    for l in a {
        *count.entry(l).or_default() += 1;
    }
    for l in b {
        *count.entry(l).or_default() -= 1;
    }
    let (mut added, mut removed) = (0, 0);
    for c in count.into_values() {
        if c > 0 {
            removed += c as u32;
        } else {
            added += (-c) as u32;
        }
    }
    (added, removed, removed as usize != n)
}

/// The changed middle as an LCS line diff (one removed-then-added block
/// when it is too big for the table).
fn middle<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(LineKind, &'a str)> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    if a.is_empty() || b.is_empty() || a.len() * b.len() > LCS_CELLS {
        out.extend(a.iter().map(|l| (LineKind::Del, *l)));
        out.extend(b.iter().map(|l| (LineKind::Add, *l)));
        return out;
    }
    let (n, m) = (a.len(), b.len());
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * (m + 1) + j] =
                if a[i] == b[j] { t[(i + 1) * (m + 1) + j + 1] + 1 } else { t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((LineKind::Context, a[i]));
            i += 1;
            j += 1;
        } else if t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1] {
            out.push((LineKind::Del, a[i]));
            i += 1;
        } else {
            out.push((LineKind::Add, b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| (LineKind::Del, *l)));
    out.extend(b[j..].iter().map(|l| (LineKind::Add, *l)));
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]})
    }

    /// A result as the daemon sends it: text only, never `is_error` on
    /// these paths (agent-engine `runtime/stream.rs`).
    fn result(id: &str, text: &str) -> Value {
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]})
    }

    const OK: &str = "Edited the file";
    const DENIED: &str = "Tool call blocked by extension: Tool call confirmation denied: guard wants to check this edit";
    const FAILED: &str = "Tool execution failed: old_string not found in '/w/r/src/a.rs'. Make sure it matches exactly, including whitespace and indentation.";

    /// A wrapped code line hangs at its own indent: the indent in columns
    /// (a tab is 4), at most INDENT_MAX, and the rest.
    #[test]
    fn a_line_hangs_at_its_own_indent() {
        assert_eq!(hang("        // The tray's flash"), (8, "// The tray's flash"));
        assert_eq!(hang("\t\tx"), (8, "x"));
        assert_eq!(hang("x  y"), (0, "x  y"));
        assert_eq!(hang("   "), (3, ""));
        assert_eq!(hang(&format!("{}deep", " ".repeat(40))).0, INDENT_MAX);
        let mut p = Preview::default();
        push(&mut p, LineKind::Add, "    let x = 1;");
        assert_eq!((p.lines[0].indent, &*p.lines[0].text), (4, "let x = 1;"));
    }

    #[test]
    fn edit_diff_counts_and_context() {
        let old = "a\nb\nc\nd\nold1\nold2\ne\nf\ng\nh";
        let new = "a\nb\nc\nd\nnew1\ne\nf\ng\nh";
        let mut p = Preview::default();
        assert_eq!(diff_into(&mut p, old, new), (1, 2, false));
        let kinds: Vec<LineKind> = p.lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(kinds, vec![Note, Context, Context, Del, Del, Add, Context, Context, Note]);
        assert_eq!(&*p.lines[1].text, "c");
        assert_eq!(&*p.lines[5].text, "new1");
        // An LCS keeps a moved-around common line as context.
        let mut p = Preview::default();
        assert_eq!(diff_into(&mut p, "x\nkeep\ny", "keep\nz"), (1, 2, false));
        assert!(p.lines.iter().any(|l| l.kind == Context && &*l.text == "keep"));
    }

    #[test]
    fn only_successful_calls_count_and_paths_are_relative() {
        let msgs = vec![
            tool_use("t1", "edit", json!({"path": "/w/r/src/a.rs", "old_string": "x\ny", "new_string": "x\nz\nw"})),
            result("t1", OK),
            tool_use("t2", "write", json!({"path": "/w/r/new.sh", "content": "#!/bin/sh\necho hi\n"})),
            result("t2", FAILED),
            tool_use("t3", "bash", json!({"command": "ls"})),
            result("t3", OK),
            tool_use("t4", "edit", json!({"path": "/w/r/src/a.rs", "old_string": "q", "new_string": "r"})),
        ];
        let mut l = Ledger::default();
        l.reset(&msgs, Some("/w/r/"), false);
        assert_eq!(
            l.files(),
            &[FileStat {
                path: "src/a.rs".into(),
                dir: "src/".into(),
                name: "a.rs".into(),
                kind: FileKind::Modified,
                added: 2,
                removed: 1,
                written: 0,
                approx: false
            }]
        );
        assert_eq!(l.totals(), (2, 1));
        assert!(l.preview("t2").is_some(), "a failed write still shows what it tried");
        assert!(l.preview("t3").is_none());
        assert_eq!(l.preview("t1").unwrap().title, "src/a.rs");
        // t4's result arrives in a later append.
        l.append(&[result("t4", OK)]);
        assert_eq!(l.files()[0].added, 3);
        assert_eq!(l.files()[0].removed, 2);
        assert_eq!((&*l.files()[0].dir, &*l.files()[0].name), ("src/", "a.rs"));
        // The live event, then its history copy: counted once.
        l.tool_use("write", "t5", &json!({"path": "/elsewhere/x.txt", "content": "1\n2\n3"}));
        assert!(l.preview("t5").is_some(), "the live event shows its diff before the history has it");
        assert_eq!(l.files().len(), 1, "but counts only once it succeeded");
        l.append(&[tool_use("t5", "write", json!({"path": "/elsewhere/x.txt", "content": "1\n2\n3"})), result("t5", OK)]);
        assert_eq!(
            l.files()[1],
            FileStat {
                path: "/elsewhere/x.txt".into(),
                dir: "/elsewhere/".into(),
                name: "x.txt".into(),
                kind: FileKind::Written,
                added: 0,
                removed: 0,
                written: 3,
                approx: false
            }
        );
        // A write is not a delta: not in the totals.
        assert_eq!(l.totals(), (3, 2));
        // A replacement starts over.
        l.reset(&[], None, true);
        assert!(l.files().is_empty());
        assert_eq!(l.totals(), (0, 0));
    }

    /// The daemon's own failure texts (no `is_error`): a denied, failed or
    /// canceled edit changes nothing.
    #[test]
    fn denied_failed_and_canceled_edits_change_nothing() {
        let edit = |id: &str| tool_use(id, "edit", json!({"path": "/w/a.rs", "old_string": "a", "new_string": "b\nc"}));
        let mut l = Ledger::default();
        l.reset(
            &[
                edit("d"),
                result("d", DENIED),
                edit("f"),
                result("f", FAILED),
                edit("c"),
                result("c", "Canceled by user"),
                edit("p"),
                result("p", "[partial]\n\n[Canceled by user before the tool finished; the output above is partial.]"),
                edit("g"),
                result("g", "Tool call denied: tool is not activated for this session: edit"),
            ],
            None,
            false,
        );
        assert!(l.files().is_empty(), "{:?}", l.files());
        assert_eq!(l.totals(), (0, 0));
        assert!(l.preview("d").is_some(), "the row still shows what was asked");
        // The live path: the `ToolUse` event, then the history with the denial.
        l.tool_use("edit", "live", &json!({"path": "/w/b.rs", "old_string": "x", "new_string": "y"}));
        l.append(&[edit("live"), result("live", DENIED)]);
        assert!(l.files().is_empty());
        l.append(&[edit("ok"), result("ok", OK)]);
        assert_eq!(l.files().len(), 1);
    }

    #[test]
    fn a_resync_of_the_same_session_reuses_the_analysis() {
        let msgs = [tool_use("t", "edit", json!({"path": "a", "old_string": "1", "new_string": "2"})), result("t", OK)];
        let mut l = Ledger::default();
        l.reset(&msgs, None, false);
        let first = l.preview("t").unwrap();
        l.reset(&msgs, None, true);
        assert!(Arc::ptr_eq(&first, &l.preview("t").unwrap()));
        assert_eq!(l.totals(), (1, 1));
        l.reset(&msgs, None, false);
        assert!(!Arc::ptr_eq(&first, &l.preview("t").unwrap()), "another session starts afresh");
    }

    #[test]
    fn multi_edit_groups_by_file() {
        let input = json!({"edits": [
            {"path": "a.md", "op": "replace", "anchor": "one", "text": "uno"},
            {"path": "a.md", "op": "append", "text": "tail\nmore"},
            {"path": "b.jsonl", "op": "append_jsonl", "value": {"k": 1}},
            {"path": "c.rs", "op": "replace", "anchor_mode": "line_regex", "anchor": "^fn (old)", "text": "new"},
        ]});
        let mut l = Ledger::default();
        l.append(&[tool_use("m", "multi_edit", input), result("m", OK)]);
        let f = l.files();
        assert_eq!((f[0].path.as_str(), f[0].added, f[0].removed), ("a.md", 3, 1));
        assert_eq!((f[1].path.as_str(), f[1].added, f[1].removed), ("b.jsonl", 1, 0));
        // A regex anchor is a pattern, not a removed line.
        assert_eq!((f[2].path.as_str(), f[2].added, f[2].removed), ("c.rs", 1, 0));
        let p = l.preview("m").unwrap();
        let notes: Vec<&str> = p.lines.iter().filter(|l| l.kind == LineKind::Note).map(|l| &*l.text).collect();
        assert_eq!(notes, ["a.md", "b.jsonl", "c.rs", "line_regex: ^fn (old)"]);
        assert!(!p.lines.iter().any(|l| l.kind == LineKind::Del && l.text.contains("fn")));
        assert_eq!(p.title, "a.md +2 files");
        // The same call with `[1]` skipped by its `unless_present`, as the
        // result says it (agent-engine `tools/multi_edit/mod.rs`).
        let skipped = "multi_edit: 3 file(s) written, 3 edit(s) applied, 1 skipped (line numbers are as of each edit)\n\
            [0] a.md replace L1 -3 +3 B\n\
            [1] a.md append skipped: unless_present found at L4\n\
            [2] b.jsonl append_jsonl L1 +8 B\n\
            [3] c.rs replace L1 -6 +3 B";
        let mut l = Ledger::default();
        let input = json!({"edits": [
            {"path": "a.md", "op": "replace", "anchor": "one", "text": "uno"},
            {"path": "a.md", "op": "append", "text": "tail\nmore", "unless_present": "tail"},
            {"path": "b.jsonl", "op": "append_jsonl", "value": {"k": 1}},
            {"path": "c.rs", "op": "replace", "anchor_mode": "line_regex", "anchor": "^fn (old)", "text": "new"},
        ]});
        l.append(&[tool_use("m", "multi_edit", input), result("m", skipped)]);
        let f = l.files();
        assert_eq!((f[0].path.as_str(), f[0].added, f[0].removed), ("a.md", 1, 1));
        assert_eq!(l.totals(), (3, 1));
    }

    #[test]
    fn previews_are_bounded() {
        let big = (0..5000).map(|i| format!("line {i} {}", "x".repeat(400))).collect::<Vec<_>>().join("\n");
        let mut l = Ledger::default();
        l.tool_use("write", "w", &json!({"path": "big", "content": big}));
        let p = l.preview("w").unwrap();
        assert_eq!(p.lines.len(), PREVIEW_LINES);
        assert_eq!(p.more, 5000 - PREVIEW_LINES);
        assert_eq!(p.added, 5000);
        assert!(p.lines.iter().all(|l| l.text.chars().count() <= LINE_CHARS + 1));
        // A huge replace is one block, not a 25M-cell table.
        let old = (0..3000).map(|i| format!("o{i}")).collect::<Vec<_>>().join("\n");
        let new = (0..3000).map(|i| format!("n{i}")).collect::<Vec<_>>().join("\n");
        let mut p = Preview::default();
        // No line in common: the multiset count is exact.
        assert_eq!(diff_into(&mut p, &old, &new), (3000, 3000, false));
    }

    /// A file's edits for its preview (blocks.md §1.5, Shady #101 P2-3):
    /// kept as each call commits, the newest [`EDIT_CALLS`] in call order
    /// (results out of order in one message included) and a count of the
    /// earlier ones, whatever the history's length; a failed call is none;
    /// a `multi_edit` counts for each file it changed; a name of the same
    /// file (absolute, `./`) finds them; another session's ledger is
    /// another identity.
    #[test]
    fn a_files_edits_keep_the_newest_calls_in_call_order() {
        let call = |id: &str, name: &str, input: Value| json!({"type": "tool_use", "id": id, "name": name, "input": input});
        let res = |id: &str, text: &str| json!({"type": "tool_result", "tool_use_id": id, "content": text});
        let mut l = Ledger::default();
        l.reset(&[], Some("/w"), false);
        let n = 500;
        for k in 0..n {
            let id = format!("e{k}");
            let input = json!({"path": "/w/src/a.rs", "old_string": format!("a{k}"), "new_string": format!("b{k}")});
            l.append(&[tool_use(&id, "edit", input), result(&id, if k % 7 == 3 { FAILED } else { OK })]);
        }
        // Two calls in one message, their results the other way round; and
        // a multi_edit of two files.
        let x = call("x", "edit", json!({"path": "src/a.rs", "old_string": "p", "new_string": "q"}));
        let y = call("y", "write", json!({"path": "src/a.rs", "content": "1\n2\n"}));
        let m = call(
            "m",
            "multi_edit",
            json!({"edits": [
            {"path": "src/a.rs", "op": "replace", "anchor": "r", "text": "s"},
            {"path": "src/b.rs", "op": "replace", "anchor": "t", "text": "u"}]}),
        );
        l.append(&[
            json!({"role": "assistant", "content": [x, y, m]}),
            json!({"role": "user", "content": [res("y", OK), res("x", OK), res("m", OK)]}),
        ]);
        let ok = (0..n).filter(|k| k % 7 != 3).count() + 3;
        let e = l.edits("src/a.rs").expect("a.rs");
        assert_eq!(e.recent.len(), EDIT_CALLS);
        assert_eq!(e.earlier + e.recent.len(), ok);
        let seqs: Vec<u64> = e.recent.iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "call order: {seqs:?}");
        let last: Vec<Option<u32>> = e.recent.iter().rev().take(3).map(|e| e.written).collect();
        assert_eq!(last, [None, Some(2), None], "m, y (a write), x: in call order");
        assert!(Arc::ptr_eq(&e.recent.back().unwrap().preview, &l.preview("m").unwrap()), "the call's preview, shared");
        let p = l.edit_preview("src/a.rs").unwrap();
        assert!(p.groups.len() <= EDIT_CALLS && p.groups.iter().map(|g| g.lines.len()).sum::<usize>() <= EDIT_LINES);
        assert_eq!(p.groups.len() + p.earlier, ok, "every call kept or counted");
        assert_eq!(p.groups.last().map(|g| g.written), Some(None), "the newest last");
        assert_eq!(l.edits("/w/src/a.rs").map(|e| e.earlier), Some(e.earlier), "an absolute name");
        assert_eq!(l.edits("./src/b.rs").map(|e| e.recent.len()), Some(1), "the multi_edit's other file");
        assert!(l.edits("src/c.rs").is_none());
        let mut other = Ledger::default();
        other.reset(&[], Some("/w"), false);
        assert_ne!(other.version().0, l.version().0, "another session's ledger");
        let v = l.version();
        l.reset(&[], Some("/w"), true);
        assert_eq!(l.version().0, v.0, "the same session's, reset");
        assert!(l.version().1 > v.1);
    }

    /// N1: an edit past the preview's alignment cap still counts what changed,
    /// not every line as removed and added.
    #[test]
    fn counts_do_not_depend_on_the_preview_cap() {
        let old: Vec<String> = (0..160).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[1] = "changed near the top".into();
        new[158] = "changed near the end".into();
        new.insert(80, "one more in the middle".into());
        let (old, new) = (old.join("\n"), new.join("\n"));
        let mut p = Preview::default();
        assert_eq!(diff_into(&mut p, &old, &new), (3, 2, false));
        assert_eq!((p.added, p.removed), (3, 2));
        assert!(p.lines.iter().any(|l| l.kind == LineKind::Note && l.text.starts_with("too long to align")));
        let mut l = Ledger::default();
        l.append(&[tool_use("e", "edit", json!({"path": "a.rs", "old_string": old, "new_string": new})), result("e", OK)]);
        assert_eq!(l.totals(), (3, 2));
        // The exact count agrees with the aligned (LCS) one where both run.
        let mut seed = 0x9e37_79b9u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..300 {
            let (n, m) = (next() as usize % 12, next() as usize % 12);
            let a: Vec<String> = (0..n).map(|_| (next() % 4).to_string()).collect();
            let b: Vec<String> = (0..m).map(|_| (next() % 4).to_string()).collect();
            let (a, b): (Vec<&str>, Vec<&str>) = (a.iter().map(String::as_str).collect(), b.iter().map(String::as_str).collect());
            let ops = middle(&a, &b);
            let lcs =
                (ops.iter().filter(|o| o.0 == LineKind::Add).count() as u32, ops.iter().filter(|o| o.0 == LineKind::Del).count() as u32);
            assert_eq!(changed_lines(&a, &b), (lcs.0, lcs.1, false), "{a:?} -> {b:?}");
        }
    }

    /// P6-2: past the work budget the multiset count is a lower bound (a
    /// reordered block counts as almost nothing): flagged, never exact.
    #[test]
    fn an_estimated_count_is_flagged() {
        let old: Vec<String> = (0..1500).map(|i| format!("row {i}")).collect();
        let new: Vec<String> = old.iter().rev().cloned().collect();
        let (old, new) = (old.join("\n"), new.join("\n"));
        let mut p = Preview::default();
        let (a, r, approx) = diff_into(&mut p, &old, &new);
        assert!(approx, "+{a} -{r} is an estimate");
        let mut l = Ledger::default();
        l.append(&[
            tool_use("big", "edit", json!({"path": "t.md", "old_string": old, "new_string": new})),
            result("big", OK),
            tool_use("small", "edit", json!({"path": "u.md", "old_string": "a", "new_string": "b"})),
            result("small", OK),
        ]);
        assert!(l.files()[0].approx && !l.files()[1].approx);
        assert!(l.approx(), "the totals include it");
        // The same rows, reordered inside the budget, are exact.
        let old: Vec<String> = (0..60).map(|i| format!("row {i}")).collect();
        let new: Vec<String> = old.iter().rev().cloned().collect();
        let mut p = Preview::default();
        assert!(!diff_into(&mut p, &old.join("\n"), &new.join("\n")).2);
    }

    /// `bash` writes: their own list, only after a successful run, never in
    /// the edit counts.
    #[test]
    fn shell_writes_are_listed_apart() {
        let bash = |id: &str, cmd: &str| tool_use(id, "bash", json!({"command": cmd}));
        let msgs = vec![
            bash("b1", "sed -i 's/a/b/' src/a.rs && rm -f /w/r/old.txt"),
            result("b1", "Tool execution failed: Command failed (exit 1):\nsed: can't read src/a.rs"),
            bash("b2", "sed -i 's/a/b/' src/a.rs && rm -f /w/r/old.txt"),
            result("b2", ""),
            bash("b3", "cargo test > /tmp/t.log 2>&1"),
            result("b3", OK),
            bash("b4", "echo $(ls) > \"$OUT\"; ls"),
            result("b4", OK),
            bash("b5", "mv src/a.rs src/b.rs"),
            result("b5", "Canceled by user"),
            // M2: stopped at the daemon's output limit (`tools/bash.rs`
            // `output_limit_notice`): a success result, exit status unknown.
            bash("b7", "cargo build -vv 2>&1; sed -i 's/x/y/' Cargo.toml && rm old.rs"),
            result(
                "b7",
                "Compiling…\n\n[output truncated at 30000 bytes: the command was stopped at this limit before it finished, so its exit status is unknown. Redirect large output to a file (e.g. cmd > out.log 2>&1) and read only the parts you need.]",
            ),
            // The same as default history carries it: virtualised (head,
            // gap, tail, note), the notice ending the tail.
            bash("b8", "yes | head -c 400000; touch done.flag"),
            result(
                "b8",
                "y\n[... lines 2-131070 omitted ...]\ny\n\n[output truncated at 262144 bytes: the command was stopped at this limit before it finished, so its exit status is unknown. Redirect large output to a file (e.g. cmd > out.log 2>&1) and read only the parts you need.]\n\n[Output virtualised: 262356 bytes, 131074 lines; lines 2-131070 are omitted above. Full output: /s/toolu_8.txt (read it with offset=1 limit=131069, or grep it).]",
            ),
        ];
        let mut l = Ledger::default();
        l.reset(&msgs, Some("/w/r"), false);
        let got: Vec<(&str, &str)> = l.shell().iter().map(|f| (f.path.as_str(), f.verb)).collect();
        assert_eq!(got, [("src/a.rs", "sed -i"), ("old.txt", "rm"), ("/tmp/t.log", ">")]);
        // L5: a command that writes nothing (or cannot be read) is lexed
        // once and remembered as such, never waited on.
        assert!(matches!(l.shell_calls.get("b3"), Some(Some(_))));
        assert!(matches!(l.shell_calls.get("b4"), Some(None)));
        assert!(!l.pending.contains("b4"));
        assert!(l.files().is_empty());
        assert_eq!(l.totals(), (0, 0));
        assert!(l.preview("b2").is_none(), "no diff for a shell call");
        // The live event, then its history copy: listed once, by its last verb.
        l.tool_use("bash", "b6", &json!({"command": "rm src/a.rs"}));
        l.append(&[bash("b6", "rm src/a.rs"), result("b6", OK)]);
        assert_eq!(l.shell().len(), 3);
        assert_eq!((l.shell()[0].path.as_str(), l.shell()[0].verb), ("src/a.rs", "rm"));
        assert_eq!((&*l.shell()[0].dir, &*l.shell()[0].name), ("src/", "a.rs"));
    }

    #[test]
    fn junk_inputs_do_not_panic() {
        let mut l = Ledger::default();
        for input in [Value::Null, json!("str"), json!({"path": 3}), json!({"edits": "no"}), json!({"edits": [1, null]})] {
            for name in ["edit", "write", "multi_edit", "bash"] {
                l.tool_use(name, "j", &input);
            }
        }
        l.append(&[json!({"role": "user"}), json!({"content": [{"type": "tool_result"}]}), json!(null)]);
    }
}
