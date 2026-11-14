//! Subagents in the transcript, as the Synaps TUI shows them: each worker
//! under the call that started it (`subagent_start`, or the blocking
//! `subagent`) with its name, model, state, tool count, elapsed time and
//! its latest step or result, kept current while it runs. Only from what
//! reaches an attached client:
//! - the calls and their results (the history, the live `ToolUse` and
//!   `ToolResult`): the label, the task and the model asked for;
//!   `subagent_start`'s result names its worker (`handle_id` `sa_N`, N its
//!   id), so a worker never lands under another call;
//! - the live `SubagentStart` / `SubagentUpdate` / `SubagentDone` events
//!   (on the turn's own stream; a start is matched to its call, still
//!   unanswered, by label and task, then confirmed by the result);
//! - `SubagentRows`, 1 Hz while a turn runs and after it while a worker
//!   still does (how a worker that outlives its turn keeps moving);
//! - the completion event in the history (`<event … source="subagent">`)
//!   and `subagent_collect` / `subagent_status` results: how one ended.
//!
//! Nothing is estimated: a worker whose start is only in the history and
//! that no live source has spoken of since is "started", not "running".
//! Nothing here ticks: the elapsed time moves when a row comes.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::SharedString;
use serde_json::Value;
use synaps_client::protocol::{AgentEvent, LlmEvent, SessionEvent, StreamEvent};

/// `SubagentStart`'s `task_preview`: the task's first this many chars.
const PREVIEW_CHARS: usize = 80;

/// Shape generations, unique across every tab's [`Subagents`] (a tab
/// switch never meets the number the rows were built at by chance).
static SHAPES: AtomicU64 = AtomicU64::new(1);

/// The tools that start a worker.
pub fn starts_worker(name: &str) -> bool {
    matches!(name, "subagent_start" | "subagent")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// Its start is in the history only: whether it still runs, no live
    /// source has said.
    Started,
    Running,
    /// Running, its cancel asked for.
    Cancelling,
    Done,
    Failed,
    Cancelled,
    TimedOut,
}

impl Status {
    pub fn terminal(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled | Status::TimedOut)
    }

    pub fn running(self) -> bool {
        matches!(self, Status::Running | Status::Cancelling)
    }

    /// A weaker source never overrides a stronger one: the history's
    /// "started" < live "running" < how it ended (final).
    fn rank(self) -> u8 {
        match self {
            Status::Started => 0,
            Status::Running | Status::Cancelling => 1,
            _ => 2,
        }
    }

    /// The daemon's word (`SubagentStatus`, the completion event's and
    /// collect's `status`) and a failure's reason.
    fn parse(v: &Value) -> Option<(Self, Option<String>)> {
        match v {
            Value::String(s) => Some((
                match s.as_str() {
                    "running" => Status::Running,
                    "completed" => Status::Done,
                    "cancelled" => Status::Cancelled,
                    "timed_out" => Status::TimedOut,
                    "failed" => Status::Failed,
                    _ => return None,
                },
                None,
            )),
            Value::Object(o) => o.get("failed").map(|r| (Status::Failed, r.as_str().map(str::to_string))),
            _ => None,
        }
    }
}

/// What is known of one worker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worker {
    pub id: Option<u64>,
    pub name: String,
    pub model: String,
    pub status: Option<Status>,
    pub reason: Option<String>,
    /// Its latest progress line (`⚙ bash (tool #3)`, `$ cargo test`).
    pub step: String,
    pub tools: Option<u32>,
    /// Seconds: a running worker's as of its last row (a worker with no
    /// rows, the blocking `subagent`, has none until it ends: nothing here
    /// ticks), a finished one's run.
    pub elapsed: Option<f64>,
    /// Its result's first line.
    pub result: String,
}

impl Worker {
    fn set_status(&mut self, s: Status, reason: Option<String>) {
        if self.status.is_some_and(|c| c.terminal() || c.rank() > s.rank()) {
            return;
        }
        self.status = Some(s);
        if reason.is_some() {
            self.reason = reason;
        }
    }

    /// Facts from another source of the same worker: the stronger status,
    /// and what this one lacks.
    fn merge(&mut self, o: Worker) {
        self.id = self.id.or(o.id);
        if let Some(s) = o.status {
            self.set_status(s, o.reason);
        }
        let fill = |a: &mut String, b: String| {
            if a.is_empty() {
                *a = b
            }
        };
        fill(&mut self.name, o.name);
        fill(&mut self.model, o.model);
        fill(&mut self.step, o.step);
        fill(&mut self.result, o.result);
        self.tools = self.tools.or(o.tools);
        self.elapsed = self.elapsed.or(o.elapsed);
    }
}

/// How a block's state word is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Run,
    Ok,
    Err,
    Warn,
    Dim,
}

/// A worker as its block shows it, formatted once per change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub status: Option<Status>,
    pub tone: Tone,
    pub word: &'static str,
    pub name: SharedString,
    /// `sonnet-5-5 · 6 tools · 41.0s`, what is known of it.
    pub facts: SharedString,
    /// Running: its step; finished: its result, or why it failed.
    pub line: Option<(SharedString, bool)>,
}

impl View {
    fn of(c: &Call) -> Option<Self> {
        let w = &c.w;
        if w.id.is_none() && w.status.is_none() {
            return None;
        }
        let (tone, word) = match w.status {
            None | Some(Status::Started) => (Tone::Dim, "started"),
            Some(Status::Running) => (Tone::Run, "running"),
            Some(Status::Cancelling) => (Tone::Warn, "cancelling"),
            Some(Status::Done) => (Tone::Ok, "completed"),
            Some(Status::Failed) => (Tone::Err, "failed"),
            Some(Status::Cancelled) => (Tone::Warn, "cancelled"),
            Some(Status::TimedOut) => (Tone::Warn, "timed out"),
        };
        let mut facts: Vec<String> = Vec::new();
        let model = if w.model.is_empty() { &c.model } else { &w.model };
        if !model.is_empty() {
            facts.push(model.rsplit_once('/').map_or(model.as_str(), |(_, m)| m).to_string());
        }
        if let Some(n) = w.tools.filter(|&n| n > 0) {
            facts.push(format!("{n} tool{}", if n == 1 { "" } else { "s" }));
        }
        if let Some(s) = w.elapsed {
            facts.push(fmt_secs(s));
        }
        let line = match w.status {
            Some(Status::Failed) | Some(Status::TimedOut) if w.reason.is_some() => {
                w.reason.as_deref().map(|r| (format!("reason: {}", one_line(r, 200)).into(), true))
            }
            Some(s) if s.terminal() => (!w.result.is_empty()).then(|| (w.result.clone().into(), false)),
            Some(s) if s.running() => (!w.step.is_empty()).then(|| (one_line(&w.step, 200).into(), false)),
            _ => None,
        };
        let name = if w.name.is_empty() { c.label.clone() } else { w.name.clone() };
        Some(View { status: w.status, tone, word, name: name.into(), facts: facts.join(" · ").into(), line })
    }

    /// What its row's height and the run's fold depend on.
    fn shape(v: Option<&View>) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        v.map(|v| (v.status, v.line.is_some())).hash(&mut h);
        h.finish()
    }
}

/// `3.4s`, `59.9s`, `2m 05s`, `1h 02m` (the TUI's `fmt_secs`).
pub fn fmt_secs(secs: f64) -> String {
    if secs < 60.0 {
        return format!("{secs:.1}s");
    }
    let s = secs.round() as u64;
    if s < 3600 { format!("{}m {:02}s", s / 60, s % 60) } else { format!("{}h {:02}m", s / 3600, (s % 3600) / 60) }
}

fn one_line(s: &str, max: usize) -> String {
    crate::app::one_line(s, max)
}

/// A result's first real line (the TUI tray's: not blank, not a heading).
fn first_line(s: &str) -> String {
    let l = s.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("");
    l.strip_prefix("ERROR: ").unwrap_or(l).chars().take(200).collect()
}

/// The worker id in a handle (`sa_7` → 7).
fn handle_id(h: &str) -> Option<u64> {
    h.strip_prefix("sa_")?.parse().ok()
}

#[derive(Clone, Debug)]
struct Call {
    oneshot: bool,
    /// What the daemon calls its worker (`subagent_label`): the agent (a
    /// path's file stem), else the inline name, else "inline".
    label: String,
    /// The task's first [`PREVIEW_CHARS`] chars, as `SubagentStart` has it.
    task: String,
    /// `label · the task's first line`, the row's argument.
    arg: SharedString,
    /// The model it asked for (the worker's own, once a row says, wins).
    model: String,
    /// Its result came: no live start can be its any more.
    answered: bool,
    w: Worker,
    view: Option<View>,
    shape: u64,
}

impl Call {
    fn new(name: &str, input: &Value) -> Self {
        let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let label = match (s("agent"), s("name")) {
            (Some(a), _) if a.contains('/') => std::path::Path::new(a).file_stem().and_then(|f| f.to_str()).unwrap_or("agent").to_string(),
            (Some(a), _) => a.to_string(),
            (None, Some(n)) => n.to_string(),
            (None, None) => "inline".to_string(),
        };
        let task_full = input.get("task").and_then(Value::as_str).unwrap_or("");
        let gist = one_line(task_full, 160);
        let arg = if gist.is_empty() { label.clone() } else { format!("{label} · {gist}") };
        Call {
            oneshot: name == "subagent",
            task: task_full.chars().take(PREVIEW_CHARS).collect(),
            arg: arg.into(),
            model: s("model").unwrap_or("").to_string(),
            w: Worker::default(),
            label,
            answered: false,
            view: None,
            shape: View::shape(None),
        }
    }

    /// Rebuild what its block shows; whether it changed.
    fn refresh(&mut self) -> bool {
        let v = View::of(self);
        if v == self.view {
            return false;
        }
        self.shape = View::shape(v.as_ref());
        self.view = v;
        true
    }
}

/// The workers of the attached session, by the call that started each.
#[derive(Debug, Default)]
pub struct Subagents {
    calls: HashMap<String, Call>,
    /// Calls in the order they came (a live start goes to the oldest that
    /// fits).
    order: Vec<String>,
    /// The call each worker is shown under.
    by_id: HashMap<u64, String>,
    /// Workers no call has claimed yet (a start's result will name them).
    orphans: HashMap<u64, Worker>,
    shapes: u64,
}

impl Subagents {
    /// Bumped when a block changes shape: a line comes or goes, its state
    /// changes (the rows' heights and the runs' folds follow).
    pub fn shape_generation(&self) -> u64 {
        self.shapes
    }

    /// The block under call `tool_id`, if it started a worker this knows.
    pub fn view(&self, tool_id: &str) -> Option<&View> {
        self.calls.get(tool_id)?.view.as_ref()
    }

    /// The row's argument for a call that starts a worker.
    pub fn arg(&self, tool_id: &str) -> Option<SharedString> {
        self.calls.get(tool_id).map(|c| c.arg.clone())
    }

    /// Its worker is running: the run holding the call never folds.
    pub fn running(&self, tool_id: &str) -> bool {
        self.calls.get(tool_id).is_some_and(|c| c.w.status.is_some_and(Status::running))
    }

    /// What the call's row height and its run's fold depend on.
    pub fn shape(&self, tool_id: &str) -> u64 {
        self.calls.get(tool_id).map_or(0, |c| c.shape)
    }

    /// What it keeps, roughly (bytes).
    pub fn bytes(&self) -> usize {
        self.calls
            .values()
            .map(|c| 256 + c.label.len() + c.task.len() + c.arg.len() + c.model.len() + c.w.step.len() + c.w.result.len())
            .sum()
    }

    /// Start over from a whole history (an attach, a replacement). `same`:
    /// the same session, whose workers' live state stays (a replacement
    /// mid-run repeats the calls; the rows keep coming).
    pub fn reset(&mut self, messages: &[Value], same: bool) {
        if !same {
            *self = Subagents { shapes: SHAPES.fetch_add(1, Ordering::Relaxed), ..Subagents::default() };
        }
        self.append(messages);
    }

    /// Fold history messages after the ones seen so far (the live events
    /// came first: the same calls, nothing counted twice).
    pub fn append(&mut self, messages: &[Value]) {
        for m in messages {
            let user = m.get("role").and_then(Value::as_str) == Some("user");
            match m.get("content") {
                Some(Value::String(s)) if user => self.events(s),
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("tool_use") => {
                                if let (Some(id), Some(name)) = (b.get("id").and_then(Value::as_str), b.get("name").and_then(Value::as_str))
                                {
                                    self.tool_use(name, id, b.get("input").unwrap_or(&Value::Null));
                                }
                            }
                            Some("tool_result") => {
                                if let Some(id) = b.get("tool_use_id").and_then(Value::as_str) {
                                    self.tool_result(id, &text_of(b.get("content").unwrap_or(&Value::Null)), false);
                                }
                            }
                            Some("text") if user => self.events(b.get("text").and_then(Value::as_str).unwrap_or("")),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// A live event.
    pub fn on_session(&mut self, ev: &SessionEvent) {
        match ev {
            SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ToolUse { tool_name, tool_id, input }) } => {
                self.tool_use(tool_name, tool_id, input)
            }
            SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id, result }) } => {
                self.tool_result(tool_id, result, true)
            }
            SessionEvent::Stream { event: StreamEvent::Agent(a) } => match a {
                AgentEvent::SubagentStart { subagent_id, agent_name, task_preview } => self.started(*subagent_id, agent_name, task_preview),
                AgentEvent::SubagentUpdate { subagent_id, status, .. } => self.update(*subagent_id, |w| {
                    // "⚙ <tool> (tool #N)" marks a new tool call.
                    if let Some((_, n)) = status.strip_prefix("\u{2699} ").and_then(|r| r.rsplit_once(" (tool #")) {
                        w.tools = n.trim_end_matches(')').parse().ok().or(w.tools);
                    }
                    w.step = status.clone();
                }),
                AgentEvent::SubagentDone { subagent_id, result_preview, duration_secs, .. } => self.update(*subagent_id, |w| {
                    let (s, reason) = if result_preview.starts_with("[TIMED OUT") {
                        (Status::TimedOut, None)
                    } else if let Some(e) = result_preview.strip_prefix("ERROR") {
                        (Status::Failed, Some(e.trim_start_matches(':').trim().to_string()))
                    } else if result_preview == "Task panicked or dropped" {
                        (Status::Failed, Some(result_preview.clone()))
                    } else {
                        (Status::Done, None)
                    };
                    let was_final = w.status.is_some_and(Status::terminal);
                    w.set_status(s, reason);
                    if !was_final {
                        w.elapsed = Some(*duration_secs);
                        if s == Status::Done {
                            w.result = first_line(result_preview);
                        }
                    }
                }),
                _ => {}
            },
            SessionEvent::SubagentRows { rows } => {
                for r in rows {
                    self.row(r);
                }
            }
            _ => {}
        }
    }

    fn tool_use(&mut self, name: &str, id: &str, input: &Value) {
        if !starts_worker(name) || id.is_empty() || self.calls.contains_key(id) {
            return;
        }
        self.calls.insert(id.to_string(), Call::new(name, input));
        self.order.push(id.to_string());
    }

    fn tool_result(&mut self, id: &str, text: &str, live: bool) {
        if let Some(c) = self.calls.get_mut(id) {
            c.answered = true;
            if c.oneshot {
                // `[subagent:<label>] <output>` / `[subagent:<label> ERROR] <error>`.
                let Some(rest) = text.strip_prefix("[subagent:") else { return };
                let Some((head, body)) = rest.split_once("] ") else { return };
                if head.ends_with(" ERROR") {
                    c.w.set_status(Status::Failed, Some(body.trim().to_string()));
                } else {
                    c.w.set_status(Status::Done, None);
                    if c.w.result.is_empty() {
                        c.w.result = first_line(body);
                    }
                }
                if c.w.name.is_empty() {
                    c.w.name = head.trim_end_matches(" ERROR").to_string();
                }
                self.changed(id);
                return;
            }
            // `{"handle_id": "sa_N", "agent_name": …, "status": "running"}`.
            let Ok(v) = serde_json::from_str::<Value>(text.trim()) else { return self.changed(id) };
            let Some(n) = v.get("handle_id").and_then(Value::as_str).and_then(handle_id) else { return self.changed(id) };
            self.bind(id, n);
            let c = self.calls.get_mut(id).expect("bound");
            if let Some(name) = v.get("agent_name").and_then(Value::as_str) {
                c.w.name = name.to_string();
            }
            c.w.set_status(if live { Status::Running } else { Status::Started }, None);
            self.changed(id);
            return;
        }
        // How a worker ended, from a call that reads it (`subagent_collect`,
        // `subagent_status`): final states only (a "running" here is old).
        let Ok(v) = serde_json::from_str::<Value>(text.trim()) else { return };
        let Some(n) = v.get("handle_id").and_then(Value::as_str).and_then(handle_id) else { return };
        let Some((s, reason)) = v.get("status").and_then(Status::parse).filter(|(s, _)| s.terminal()) else { return };
        let reason = reason.or_else(|| v.get("error").and_then(Value::as_str).map(str::to_string));
        let output = ["output", "partial_output"].iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(first_line);
        let secs = v.get("elapsed_secs").and_then(Value::as_f64);
        let tools = v.get("tool_count").and_then(Value::as_u64).map(|t| t as u32);
        self.update(n, |w| {
            let was_final = w.status.is_some_and(Status::terminal);
            w.set_status(s, reason);
            if !was_final {
                w.elapsed = secs.or(w.elapsed);
            }
            w.tools = tools.or(w.tools);
            if let Some(o) = output.filter(|o| !o.is_empty() && w.result.is_empty()) {
                w.result = o;
            }
        });
    }

    /// Completion events in a user message of the history:
    /// `<event … source="subagent">Subagent 'x' (sa_2) finished … Preview: …
    /// Data: {"handle_id": …, "status": …, "duration_secs": …}</event>`.
    fn events(&mut self, text: &str) {
        if !text.contains("source=\"subagent\"") {
            return;
        }
        for ev in text.split("<event ").skip(1) {
            let Some((tag, body)) = ev.split_once('>') else { continue };
            if !tag.contains("source=\"subagent\"") {
                continue;
            }
            let body = body.split("</event>").next().unwrap_or(body);
            let (prose, data) = body.split_once("\nData: ").unwrap_or((body, ""));
            let Ok(d) = serde_json::from_str::<Value>(data.trim()) else { continue };
            let Some(n) =
                d.get("handle_id").and_then(Value::as_str).and_then(handle_id).or_else(|| d.get("subagent_id").and_then(Value::as_u64))
            else {
                continue;
            };
            let Some((s, _)) = d.get("status").and_then(Status::parse) else { continue };
            let reason = d.get("error").and_then(Value::as_str).map(str::to_string);
            let preview = prose.split_once(" Preview: ").map(|(_, p)| first_line(p)).unwrap_or_default();
            let secs = d.get("duration_secs").and_then(Value::as_f64);
            let name = d.get("agent_name").and_then(Value::as_str).map(str::to_string);
            self.update(n, |w| {
                let was_final = w.status.is_some_and(Status::terminal);
                w.set_status(s, reason);
                if !was_final {
                    w.elapsed = secs.or(w.elapsed);
                }
                if w.result.is_empty() {
                    w.result = preview;
                }
                if let Some(name) = name.filter(|_| w.name.is_empty()) {
                    w.name = name;
                }
            });
        }
    }

    /// A live start: under the call it is (its own, if one already named
    /// it), else the oldest unanswered call with its label and task.
    fn started(&mut self, n: u64, name: &str, preview: &str) {
        // An id seen before under a call that has finished (ids restart
        // after a daemon reload): a new worker.
        if let Some(t) = self.by_id.get(&n).cloned()
            && self.calls.get(&t).is_none_or(|c| c.w.status.is_some_and(Status::terminal))
        {
            self.by_id.remove(&n);
            if let Some(c) = self.calls.get_mut(&t) {
                c.w.id = None;
            }
        }
        if !self.by_id.contains_key(&n) {
            let score = |c: &Call| u8::from(c.task == preview) * 2 + u8::from(c.label == name);
            let mut best: Option<(&String, u8)> = None;
            for t in &self.order {
                if let Some(c) = self.calls.get(t).filter(|c| !c.answered && c.w.id.is_none()) {
                    let s = score(c);
                    // Strictly better: the oldest of equals.
                    if s > 0 && best.is_none_or(|(_, b)| s > b) {
                        best = Some((t, s));
                    }
                }
            }
            if let Some(t) = best.map(|(t, _)| t.clone()) {
                self.bind(&t, n);
            }
        }
        self.update(n, |w| {
            if w.name.is_empty() {
                w.name = name.to_string();
            }
            w.set_status(Status::Running, None);
        });
    }

    /// A `SubagentDisplayRow`.
    fn row(&mut self, r: &Value) {
        let Some(n) =
            r.get("subagent_id").and_then(Value::as_u64).or_else(|| r.get("handle_id").and_then(Value::as_str).and_then(handle_id))
        else {
            return;
        };
        let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let status = r.get("status").and_then(Status::parse);
        let cancel = r.get("cancel_requested").and_then(Value::as_bool).unwrap_or(false);
        let elapsed = r.get("elapsed_secs").and_then(Value::as_f64);
        // `finished_elapsed` (a serde `Duration`): how long ago it ended.
        let ago = r
            .get("finished_elapsed")
            .and_then(|d| Some(d.get("secs")?.as_f64()? + d.get("nanos").and_then(Value::as_f64).unwrap_or(0.) / 1e9));
        let (name, model, step) = (s("agent_name"), s("model"), s("step"));
        let tools = r.get("tools").and_then(Value::as_u64).map(|t| t as u32);
        self.update(n, |w| {
            let was_final = w.status.is_some_and(Status::terminal);
            if let Some((st, reason)) = status {
                let st = if st == Status::Running && cancel { Status::Cancelling } else { st };
                w.set_status(st, reason);
            }
            let ended = w.status.is_some_and(Status::terminal);
            // A finished worker's run: its age less the time since it ended.
            if !was_final && let Some(e) = elapsed {
                w.elapsed = Some(match (ended, ago) {
                    (true, Some(a)) => (e - a).max(0.),
                    _ => e,
                });
            }
            if !name.is_empty() {
                w.name = name;
            }
            if !model.is_empty() {
                w.model = model;
            }
            if !step.is_empty() {
                w.step = step;
            }
            w.tools = tools.or(w.tools);
        });
    }

    /// Show worker `n` under call `tool_id` (its result named it): what
    /// another call, or no call, had of it comes along; a worker that call
    /// was given by a wrong match is unclaimed again.
    fn bind(&mut self, tool_id: &str, n: u64) {
        if !self.calls.contains_key(tool_id) || self.by_id.get(&n).map(String::as_str) == Some(tool_id) {
            return;
        }
        let w = match self.by_id.remove(&n) {
            Some(prev) => {
                let w = self.calls.get_mut(&prev).map(|c| std::mem::take(&mut c.w)).unwrap_or_default();
                self.changed(&prev);
                w
            }
            None => self.orphans.remove(&n).unwrap_or_default(),
        };
        let c = self.calls.get_mut(tool_id).expect("checked");
        if let Some(old) = c.w.id.filter(|&o| o != n) {
            let lost = std::mem::take(&mut c.w);
            self.by_id.remove(&old);
            self.orphans.insert(old, lost);
        }
        let c = self.calls.get_mut(tool_id).expect("checked");
        c.w.merge(w);
        c.w.id = Some(n);
        self.by_id.insert(n, tool_id.to_string());
        self.changed(tool_id);
    }

    /// Change worker `n`: under its call, else as an orphan.
    fn update(&mut self, n: u64, f: impl FnOnce(&mut Worker)) {
        match self.by_id.get(&n).cloned() {
            Some(t) if self.calls.contains_key(&t) => {
                f(&mut self.calls.get_mut(&t).expect("present").w);
                self.changed(&t);
            }
            _ => f(self.orphans.entry(n).or_insert_with(|| Worker { id: Some(n), ..Worker::default() })),
        }
    }

    fn changed(&mut self, tool_id: &str) {
        let Some(c) = self.calls.get_mut(tool_id) else { return };
        let shape = c.shape;
        if c.refresh() && c.shape != shape {
            self.shapes = SHAPES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A completion event (a history notice) as the TUI says it: `scout
/// completed · 41.0s · <its result's first line>`, or why it failed.
/// `None`: not one.
pub fn completion_note(text: &str) -> Option<String> {
    let ev = text.strip_prefix("<event ")?;
    let (tag, body) = ev.split_once('>')?;
    if !tag.contains("source=\"subagent\"") {
        return None;
    }
    let body = body.split("</event>").next().unwrap_or(body);
    let (prose, data) = body.split_once("\nData: ")?;
    let d: Value = serde_json::from_str(data.trim()).ok()?;
    let (status, _) = d.get("status").and_then(Status::parse)?;
    let name = d.get("agent_name").and_then(Value::as_str).unwrap_or("subagent");
    let word = match status {
        Status::Done => "completed",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::TimedOut => "timed out",
        _ => return None,
    };
    let mut out = format!("{name} {word}");
    if let Some(s) = d.get("duration_secs").and_then(Value::as_f64) {
        out.push_str(&format!(" · {}", fmt_secs(s)));
    }
    let tail = match d.get("error").and_then(Value::as_str) {
        Some(e) => format!("reason: {}", one_line(e, 160)),
        None => prose.split_once(" Preview: ").map(|(_, p)| first_line(p)).unwrap_or_default(),
    };
    if !tail.is_empty() {
        out.push_str(&format!(" · {tail}"));
    }
    Some(out)
}

/// A tool result's text (a string, or text blocks).
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use synaps_client::protocol::decode_event;

    use super::{Status, Subagents, Tone, completion_note, fmt_secs};

    fn ev(s: &mut Subagents, v: Value) {
        s.on_session(&decode_event(&v).unwrap());
    }
    fn tool_use(s: &mut Subagents, id: &str, name: &str, input: Value) {
        ev(s, json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input}}));
    }
    fn result(s: &mut Subagents, id: &str, text: &str) {
        ev(s, json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": id, "result": text}}));
    }
    fn start(s: &mut Subagents, n: u64, name: &str, task: &str) {
        ev(
            s,
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "subagent_start", "subagent_id": n, "agent_name": name, "task_preview": task}}),
        );
    }
    fn update(s: &mut Subagents, n: u64, status: &str) {
        ev(
            s,
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "subagent_update", "subagent_id": n, "agent_name": "", "status": status}}),
        );
    }
    fn done(s: &mut Subagents, n: u64, preview: &str, secs: f64) {
        ev(
            s,
            json!({"ev": "stream", "event": {"kind": "agent", "agent": "subagent_done", "subagent_id": n, "agent_name": "", "result_preview": preview, "duration_secs": secs}}),
        );
    }
    fn rows(s: &mut Subagents, rows: Value) {
        ev(s, json!({"ev": "subagent_rows", "rows": rows}));
    }
    fn row(n: u64, name: &str, status: Value, elapsed: f64, ago: Option<f64>, step: &str, tools: u32) -> Value {
        json!({"subagent_id": n, "agent_name": name, "status": status, "cancel_requested": false, "elapsed_secs": elapsed,
            "finished_elapsed": ago.map(|a| json!({"secs": a as u64, "nanos": 0})), "step": step, "tools": tools,
            "handle_id": format!("sa_{n}"), "model": "anthropic/claude-sonnet-5-5"})
    }
    fn handle(n: u64, name: &str) -> String {
        json!({"handle_id": format!("sa_{n}"), "agent_name": name, "status": "running"}).to_string()
    }
    /// `(state word, name, facts, line)` under call `id`.
    fn shown(s: &Subagents, id: &str) -> Option<(&'static str, String, String, Option<String>)> {
        s.view(id).map(|v| (v.word, v.name.to_string(), v.facts.to_string(), v.line.as_ref().map(|l| l.0.to_string())))
    }
    fn word(s: &Subagents, id: &str) -> Option<&'static str> {
        s.view(id).map(|v| v.word)
    }

    /// Two workers started side by side, their starts out of order, each
    /// under its own call; running (steps, tool counts, elapsed from the
    /// rows), then one completes and one fails. The run they are in stays
    /// live while either runs.
    #[test]
    fn workers_show_under_their_calls_from_start_to_end() {
        let mut s = Subagents::default();
        tool_use(&mut s, "t1", "subagent_start", json!({"agent": "scout", "task": "Find where the relay refreshes tokens"}));
        tool_use(
            &mut s,
            "t2",
            "subagent_start",
            json!({"agent": "reviewer", "task": "Review the diff", "model": "anthropic/claude-opus-4-1"}),
        );
        assert_eq!(s.arg("t1").as_deref(), Some("scout · Find where the relay refreshes tokens"));
        assert_eq!((s.view("t1"), s.view("t2")), (None, None), "no worker yet: no block");
        start(&mut s, 8, "reviewer", "Review the diff");
        start(&mut s, 7, "scout", "Find where the relay refreshes tokens");
        assert_eq!(shown(&s, "t1"), Some(("running", "scout".into(), String::new(), None)));
        assert_eq!(shown(&s, "t2"), Some(("running", "reviewer".into(), "claude-opus-4-1".into(), None)), "the model it asked for");
        result(&mut s, "t1", &handle(7, "scout"));
        result(&mut s, "t2", &handle(8, "reviewer"));
        assert!(s.running("t1") && s.running("t2"));
        update(&mut s, 7, "\u{2699} bash (tool #3)");
        update(&mut s, 7, "$ rg refresh crates/relay");
        rows(&mut s, json!([row(7, "scout", json!("running"), 41.0, None, "$ rg refresh crates/relay", 3)]));
        assert_eq!(
            shown(&s, "t1"),
            Some(("running", "scout".into(), "claude-sonnet-5-5 · 3 tools · 41.0s".into(), Some("$ rg refresh crates/relay".into())))
        );
        let shape = s.shape("t1");
        rows(&mut s, json!([row(7, "scout", json!("running"), 42.0, None, "$ rg refresh crates/relay", 3)]));
        assert_eq!(s.shape("t1"), shape, "elapsed alone: same shape (no re-layout)");
        done(&mut s, 7, "## Findings\n\nRefresh happens in TokenStore::rotate.\nMore.", 43.25);
        assert_eq!(
            shown(&s, "t1"),
            Some((
                "completed",
                "scout".into(),
                "claude-sonnet-5-5 · 3 tools · 43.2s".into(),
                Some("Refresh happens in TokenStore::rotate.".into())
            ))
        );
        assert_ne!(s.shape("t1"), shape);
        assert!(!s.running("t1") && s.running("t2"));
        // A stale row (it was sent before the end) changes nothing.
        rows(&mut s, json!([row(7, "scout", json!("running"), 44.0, None, "x", 3)]));
        assert_eq!(word(&s, "t1"), Some("completed"));
        assert_eq!(shown(&s, "t1").unwrap().2, "claude-sonnet-5-5 · 3 tools · 43.2s");
        rows(&mut s, json!([row(8, "reviewer", json!({"failed": "provider overloaded"}), 70.0, Some(10.0), "", 5)]));
        let v = s.view("t2").unwrap();
        assert_eq!((v.word, v.tone, v.facts.as_ref()), ("failed", Tone::Err, "claude-sonnet-5-5 · 5 tools · 1m 00s"));
        assert_eq!(v.line, Some(("reason: provider overloaded".into(), true)));
    }

    /// Cancelled and timed out; a cancel asked for shows while it runs.
    #[test]
    fn cancelled_and_timed_out() {
        let mut s = Subagents::default();
        tool_use(&mut s, "a", "subagent_start", json!({"name": "gif-recorder", "system_prompt": "x", "task": "record"}));
        tool_use(&mut s, "b", "subagent_start", json!({"agent": "/home/me/agents/bench.md", "task": "bench it"}));
        result(&mut s, "a", &handle(1, "gif-recorder"));
        result(&mut s, "b", &handle(2, "bench"));
        assert_eq!(s.arg("b").as_deref(), Some("bench · bench it"), "a path's file stem");
        let mut r = row(1, "gif-recorder", json!("running"), 5.0, None, "", 0);
        r["cancel_requested"] = json!(true);
        rows(&mut s, json!([r]));
        assert_eq!(word(&s, "a"), Some("cancelling"));
        assert!(s.running("a"));
        rows(&mut s, json!([row(1, "gif-recorder", json!("cancelled"), 6.0, Some(0.0), "", 0)]));
        assert_eq!((word(&s, "a"), s.view("a").unwrap().tone), (Some("cancelled"), Tone::Warn));
        done(&mut s, 2, "[TIMED OUT after 600s — partial results below]\n\nhalf", 600.0);
        assert_eq!(shown(&s, "b").map(|v| (v.0, v.2)), Some(("timed out", "10m 00s".into())));
    }

    /// A completion before its start, a start before its call's result,
    /// and a late completion after the turn (rows only): nothing goes
    /// backwards and nothing lands under another call.
    #[test]
    fn out_of_order_and_late_completion() {
        let mut s = Subagents::default();
        done(&mut s, 3, "early bird", 2.0);
        tool_use(&mut s, "c", "subagent_start", json!({"agent": "scout", "task": "quick"}));
        result(&mut s, "c", &handle(3, "scout"));
        assert_eq!(shown(&s, "c").map(|v| (v.0, v.3)), Some(("completed", Some("early bird".into()))), "the orphan's end came along");
        start(&mut s, 3, "scout", "quick");
        assert_eq!(word(&s, "c"), Some("completed"), "a late start does not revive it");
        // The turn ended; the worker runs on, its rows come at 1 Hz, then its last.
        tool_use(&mut s, "d", "subagent_start", json!({"agent": "slow", "task": "take your time"}));
        start(&mut s, 4, "slow", "take your time");
        result(&mut s, "d", &handle(4, "slow"));
        rows(&mut s, json!([row(4, "slow", json!("running"), 120.0, None, "reading", 2)]));
        rows(&mut s, json!([row(4, "slow", json!("completed"), 190.0, Some(5.0), "reading", 4)]));
        assert_eq!(shown(&s, "d").map(|v| (v.0, v.2)), Some(("completed", "claude-sonnet-5-5 · 4 tools · 3m 05s".into())));
        assert_eq!(word(&s, "c"), Some("completed"));
    }

    /// The history alone (an attach): a start is "started" until a live
    /// source speaks; a completion event or a collect says how it ended. A
    /// replacement mid-run keeps the live state; another session's history
    /// starts over.
    #[test]
    fn history_and_replace_mid_run() {
        let history = |extra: Vec<Value>| {
            let mut m = vec![
                json!({"role": "user", "content": "go"}),
                json!({"role": "assistant", "content": [
                    {"type": "tool_use", "id": "h1", "name": "subagent_start", "input": {"agent": "scout", "task": "map it"}},
                    {"type": "tool_use", "id": "h2", "name": "subagent_start", "input": {"agent": "tester", "task": "test it"}}]}),
                json!({"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "h1", "content": handle(11, "scout")},
                    {"type": "tool_result", "tool_use_id": "h2", "content": [{"type": "text", "text": handle(12, "tester")}]}]}),
            ];
            m.extend(extra);
            m
        };
        let mut s = Subagents::default();
        s.reset(&history(vec![]), false);
        assert_eq!((word(&s, "h1"), word(&s, "h2")), (Some("started"), Some("started")));
        assert!(!s.running("h1"), "nothing live said so: the run may fold");
        rows(&mut s, json!([row(11, "scout", json!("running"), 9.0, None, "ls", 1)]));
        assert_eq!(word(&s, "h1"), Some("running"));
        // A resync mid-run: the same history again, and the worker's end in it for h2.
        let event = "<event id=\"e1\" type=\"subagent_completion\" severity=\"high\" source=\"subagent\">Subagent 'tester' (sa_12) finished with status 'failed' after 3.5s. Reason: boom Call subagent_collect with handle_id \"sa_12\" to retrieve the full result. Preview: \nData: {\"agent_name\":\"tester\",\"duration_secs\":3.5,\"error\":\"boom\",\"handle_id\":\"sa_12\",\"status\":\"failed\",\"subagent_id\":12}</event>";
        s.reset(&history(vec![json!({"role": "user", "content": event})]), true);
        assert_eq!(shown(&s, "h1").map(|v| (v.0, v.3)), Some(("running", Some("ls".into()))), "kept: not back to started");
        assert_eq!(shown(&s, "h2").map(|v| (v.0, v.2, v.3)), Some(("failed", "3.5s".into(), Some("reason: boom".into()))));
        assert_eq!(completion_note(event).as_deref(), Some("tester failed · 3.5s · reason: boom"));
        // A collect's result says how h1's worker ended.
        let collect = json!({"handle_id": "sa_11", "status": "completed", "output": "# Map\n\nThree modules.", "model": "m"}).to_string();
        s.append(&[
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "k", "name": "subagent_collect", "input": {"handle_id": "sa_11"}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "k", "content": collect}]}),
        ]);
        assert_eq!(shown(&s, "h1").map(|v| (v.0, v.3)), Some(("completed", Some("Three modules.".into()))));
        assert_eq!(s.view("k"), None, "the collect call starts no worker");
        // Another session: nothing of this one's live state.
        s.reset(&history(vec![]), false);
        assert_eq!(word(&s, "h1"), Some("started"));
    }

    /// Two calls alike (same agent, same task): the live starts go to the
    /// oldest first, and when the results say otherwise each worker moves
    /// to its own call with what was seen of it. A start for another task
    /// never takes a call it does not fit.
    #[test]
    fn a_worker_never_stays_under_the_wrong_call() {
        let mut s = Subagents::default();
        for id in ["x", "y"] {
            tool_use(&mut s, id, "subagent_start", json!({"agent": "twin", "task": "same"}));
        }
        tool_use(&mut s, "z", "subagent_start", json!({"agent": "other", "task": "different"}));
        start(&mut s, 5, "twin", "same");
        start(&mut s, 6, "twin", "same");
        update(&mut s, 6, "step of six");
        assert_eq!(shown(&s, "x").unwrap().3, None);
        assert_eq!(shown(&s, "y").unwrap().3.as_deref(), Some("step of six"));
        assert_eq!(s.view("z"), None);
        // The results: x started 6, y started 5.
        result(&mut s, "x", &handle(6, "twin"));
        result(&mut s, "y", &handle(5, "twin"));
        assert_eq!(shown(&s, "x").unwrap().3.as_deref(), Some("step of six"), "six's step moved with it");
        assert_eq!(shown(&s, "y").unwrap().3, None);
        update(&mut s, 5, "step of five");
        assert_eq!(shown(&s, "y").unwrap().3.as_deref(), Some("step of five"));
        assert_eq!(shown(&s, "x").unwrap().3.as_deref(), Some("step of six"));
        // A start nobody's task fits: under no call.
        start(&mut s, 9, "stranger", "unknown");
        assert_eq!(s.view("z"), None);
        result(&mut s, "z", &handle(10, "other"));
        assert_eq!(word(&s, "z"), Some("running"));
        assert!(s.view("x").is_some_and(|v| v.name == "twin"));
    }

    /// The blocking `subagent`: no handle, no rows; its start fits it by
    /// task, its end comes as an event, its result says how it ended.
    #[test]
    fn the_blocking_subagent() {
        let mut s = Subagents::default();
        tool_use(&mut s, "o", "subagent", json!({"agent": "tag-lister", "task": "List the tags"}));
        start(&mut s, 21, "tag-lister", "List the tags");
        assert!(s.running("o"));
        update(&mut s, 21, "\u{2699} bash (tool #1)");
        done(&mut s, 21, "No tags here.", 3.4);
        result(&mut s, "o", "[subagent:tag-lister] No tags here.");
        assert_eq!(shown(&s, "o").map(|v| (v.0, v.2, v.3)), Some(("completed", "1 tool · 3.4s".into(), Some("No tags here.".into()))));
        let mut h = Subagents::default();
        h.reset(
            &[
                json!({"role": "assistant", "content": [{"type": "tool_use", "id": "f", "name": "subagent", "input": {"task": "x"}}]}),
                json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "f", "content": "[subagent:inline ERROR] model refused"}]}),
            ],
            false,
        );
        assert_eq!(shown(&h, "f").map(|v| (v.0, v.1, v.3)), Some(("failed", "inline".into(), Some("reason: model refused".into()))));
        assert_eq!(h.view("f").unwrap().status, Some(Status::Failed));
    }

    /// Ids restart after a daemon reload: a start under an id a finished
    /// worker had is a new worker, under its own call.
    #[test]
    fn a_reused_id_is_a_new_worker() {
        let mut s = Subagents::default();
        tool_use(&mut s, "old", "subagent_start", json!({"agent": "a", "task": "one"}));
        result(&mut s, "old", &handle(1, "a"));
        done(&mut s, 1, "fine", 1.0);
        tool_use(&mut s, "new", "subagent_start", json!({"agent": "a", "task": "two"}));
        start(&mut s, 1, "a", "two");
        update(&mut s, 1, "busy");
        assert_eq!((word(&s, "old"), word(&s, "new")), (Some("completed"), Some("running")));
        assert_eq!(shown(&s, "new").unwrap().3.as_deref(), Some("busy"));
    }

    #[test]
    fn secs_read_as_the_tui_writes_them() {
        assert_eq!((fmt_secs(3.44), fmt_secs(125.0), fmt_secs(3720.0)), ("3.4s".into(), "2m 05s".into(), "1h 02m".into()));
        assert_eq!(completion_note("<event id=\"x\" source=\"cli\">hi</event>"), None);
    }
}
