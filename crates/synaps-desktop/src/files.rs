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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use synaps_client::transcript::{ToolOutcome, tool_result_outcome};

/// Lines a preview keeps (the rest is counted in `more`).
pub const PREVIEW_LINES: usize = 48;
/// Characters a preview line keeps.
const LINE_CHARS: usize = 240;
/// Unchanged lines shown around a change.
const CONTEXT: usize = 2;
/// Above this many `old × new` cells the diff is one block (removed, added):
/// at most ~20 µs per edit, so folding a long history at attach stays cheap.
const LCS_CELLS: usize = 20_000;

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
    /// Shared, so a frame hands it to the renderer without a copy.
    pub text: Arc<str>,
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
    /// How it was first touched.
    pub kind: FileKind,
    /// Lines the edits added and removed (not the writes).
    pub added: u32,
    pub removed: u32,
    /// Lines of the last `write` of it (0: never written whole).
    pub written: u32,
}

impl FileStat {
    /// `("crates/relay/src/", "session.rs")`.
    pub fn split(&self) -> (&str, &str) {
        match self.path.rfind(['/', '\\']) {
            Some(i) => self.path.split_at(i + 1),
            None => ("", &self.path),
        }
    }
}

/// One file a tool call changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    path: String,
    kind: FileKind,
    added: u32,
    removed: u32,
}

/// One file tool call, analysed once.
#[derive(Debug)]
struct Analysed {
    preview: Arc<Preview>,
    changes: Vec<Change>,
}

#[derive(Debug, Default)]
pub struct Ledger {
    files: Vec<FileStat>,
    index: HashMap<String, usize>,
    /// Tool calls waiting for their result.
    pending: HashSet<String>,
    /// Every file tool call seen, by tool id: kept across a reset of the
    /// same session (a resync, a reconnect), so its history is not diffed
    /// again.
    calls: HashMap<String, Arc<Analysed>>,
    cwd: Option<String>,
    added: u32,
    removed: u32,
}

/// Whether a tool changes files (its row gets a diff).
pub fn is_file_tool(name: &str) -> bool {
    matches!(name, "edit" | "write" | "multi_edit")
}

impl Ledger {
    /// Start over from a whole history (attach, replacement). `same`: the
    /// same session as before, whose analysed calls are reused.
    pub fn reset(&mut self, messages: &[Value], cwd: Option<&str>, same: bool) {
        let calls = if same { std::mem::take(&mut self.calls) } else { HashMap::new() };
        *self =
            Ledger { cwd: cwd.map(|c| c.trim_end_matches(['/', '\\']).to_string()).filter(|c| !c.is_empty()), calls, ..Ledger::default() };
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
                        if self.pending.remove(id)
                            && tool_result_outcome(b) == ToolOutcome::Ok
                            && let Some(call) = self.calls.get(id).cloned()
                        {
                            for c in &call.changes {
                                self.commit(c);
                            }
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
        if !is_file_tool(name) || id.is_empty() {
            return;
        }
        if !self.calls.contains_key(id) {
            let (mut preview, changes) = analyse(name, input);
            let changes: Vec<Change> = changes.into_iter().map(|c| Change { path: self.relative(&c.path), ..c }).collect();
            preview.title = match changes.as_slice() {
                [] => String::new(),
                [one] => one.path.clone(),
                [first, rest @ ..] => format!("{} +{} files", first.path, rest.len()),
            };
            self.calls.insert(id.to_string(), Arc::new(Analysed { preview: Arc::new(preview), changes }));
        }
        self.pending.insert(id.to_string());
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

    fn commit(&mut self, c: &Change) {
        let i = match self.index.get(&c.path) {
            Some(&i) => i,
            None => {
                self.index.insert(c.path.clone(), self.files.len());
                self.files.push(FileStat { path: c.path.clone(), kind: c.kind, added: 0, removed: 0, written: 0 });
                self.files.len() - 1
            }
        };
        let f = &mut self.files[i];
        match c.kind {
            FileKind::Written => f.written = c.added,
            FileKind::Modified => {
                f.added += c.added;
                f.removed += c.removed;
                self.added += c.added;
                self.removed += c.removed;
            }
        }
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

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn count_lines(s: &str) -> u32 {
    s.lines().count() as u32
}

/// The preview and per-file changes of one tool input.
fn analyse(name: &str, input: &Value) -> (Preview, Vec<Change>) {
    let mut p = Preview::default();
    let mut changes = Vec::new();
    match name {
        "edit" => {
            let path = str_of(input, "path");
            let (a, r) = diff_into(&mut p, str_of(input, "old_string"), str_of(input, "new_string"));
            changes.push(Change { path: path.into(), kind: FileKind::Modified, added: a, removed: r });
        }
        "write" => {
            let content = str_of(input, "content");
            let n = count_lines(content);
            for l in content.lines() {
                push(&mut p, LineKind::Add, l);
            }
            p.added += n;
            changes.push(Change { path: str_of(input, "path").into(), kind: FileKind::Written, added: n, removed: 0 });
        }
        "multi_edit" => {
            let mut last_path = "";
            for e in input.get("edits").and_then(Value::as_array).into_iter().flatten() {
                let path = str_of(e, "path");
                if path != last_path {
                    push(&mut p, LineKind::Note, path);
                    last_path = path;
                }
                let text = str_of(e, "text");
                let (a, r) = match (str_of(e, "op"), str_of(e, "anchor_mode")) {
                    ("replace", "" | "exact") => diff_into(&mut p, str_of(e, "anchor"), text),
                    // A pattern, not the text it replaces: show it as such.
                    ("replace", mode) => {
                        push(&mut p, LineKind::Note, &format!("{mode}: {}", str_of(e, "anchor")));
                        diff_into(&mut p, "", text)
                    }
                    ("append_jsonl", _) => {
                        push(&mut p, LineKind::Add, "{…}");
                        p.added += 1;
                        (1, 0)
                    }
                    _ => diff_into(&mut p, "", text),
                };
                match changes.iter_mut().find(|c: &&mut Change| c.path == path) {
                    Some(c) => {
                        c.added += a;
                        c.removed += r;
                    }
                    None => changes.push(Change { path: path.into(), kind: FileKind::Modified, added: a, removed: r }),
                }
            }
        }
        _ => {}
    }
    (p, changes)
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
    p.lines.push(DiffLine { kind, text: t.into() });
}

/// Append the line diff `old` → `new` to `p`; returns (added, removed).
fn diff_into(p: &mut Preview, old: &str, new: &str) -> (u32, u32) {
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
    let ops = middle(am, bm);
    let (mut added, mut removed) = (0, 0);
    for (kind, l) in ops {
        match kind {
            LineKind::Add => added += 1,
            LineKind::Del => removed += 1,
            _ => {}
        }
        push(p, kind, l);
    }
    for l in &a[a.len() - suf..][..suf.min(CONTEXT)] {
        push(p, LineKind::Context, l);
    }
    if suf > CONTEXT {
        push(p, LineKind::Note, "…");
    }
    p.added += added;
    p.removed += removed;
    (added, removed)
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

    #[test]
    fn edit_diff_counts_and_context() {
        let old = "a\nb\nc\nd\nold1\nold2\ne\nf\ng\nh";
        let new = "a\nb\nc\nd\nnew1\ne\nf\ng\nh";
        let mut p = Preview::default();
        assert_eq!(diff_into(&mut p, old, new), (1, 2));
        let kinds: Vec<LineKind> = p.lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(kinds, vec![Note, Context, Context, Del, Del, Add, Context, Context, Note]);
        assert_eq!(&*p.lines[1].text, "c");
        assert_eq!(&*p.lines[5].text, "new1");
        // An LCS keeps a moved-around common line as context.
        let mut p = Preview::default();
        assert_eq!(diff_into(&mut p, "x\nkeep\ny", "keep\nz"), (1, 2));
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
        assert_eq!(l.files(), &[FileStat { path: "src/a.rs".into(), kind: FileKind::Modified, added: 2, removed: 1, written: 0 }]);
        assert_eq!(l.totals(), (2, 1));
        assert!(l.preview("t2").is_some(), "a failed write still shows what it tried");
        assert!(l.preview("t3").is_none());
        assert_eq!(l.preview("t1").unwrap().title, "src/a.rs");
        // t4's result arrives in a later append.
        l.append(&[result("t4", OK)]);
        assert_eq!(l.files()[0].added, 3);
        assert_eq!(l.files()[0].removed, 2);
        assert_eq!(l.files()[0].split(), ("src/", "a.rs"));
        // The live event, then its history copy: counted once.
        l.tool_use("write", "t5", &json!({"path": "/elsewhere/x.txt", "content": "1\n2\n3"}));
        assert!(l.preview("t5").is_some(), "the live event shows its diff before the history has it");
        assert_eq!(l.files().len(), 1, "but counts only once it succeeded");
        l.append(&[tool_use("t5", "write", json!({"path": "/elsewhere/x.txt", "content": "1\n2\n3"})), result("t5", OK)]);
        assert_eq!(l.files()[1], FileStat { path: "/elsewhere/x.txt".into(), kind: FileKind::Written, added: 0, removed: 0, written: 3 });
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
        assert_eq!(diff_into(&mut p, &old, &new), (3000, 3000));
    }

    #[test]
    fn junk_inputs_do_not_panic() {
        let mut l = Ledger::default();
        for input in [Value::Null, json!("str"), json!({"path": 3}), json!({"edits": "no"}), json!({"edits": [1, null]})] {
            for name in ["edit", "write", "multi_edit"] {
                l.tool_use(name, "j", &input);
            }
        }
        l.append(&[json!({"role": "user"}), json!({"content": [{"type": "tool_result"}]}), json!(null)]);
    }
}
