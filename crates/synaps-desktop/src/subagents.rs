//! Subagents in the transcript, as the Synaps TUI shows them: each worker
//! under the call that started it (`subagent_start`, `subagent_resume`, or
//! the blocking `subagent`) with its name, model, state, tool count,
//! elapsed time and its latest step or result, kept current while it runs.
//! Only from what reaches an attached client:
//! - the calls and their results (the history, the live `ToolUse` and
//!   `ToolResult`): the label, the task and the model asked for;
//!   `subagent_start`'s and `subagent_resume`'s results name the worker
//!   (`handle_id` `sa_N`, N its id), so a worker never lands under another
//!   call, and a call keeps the worker its result named: ids restart at 1
//!   with a cold-started daemon, so a later call naming the same id takes
//!   the id from then on, never the earlier call's worker;
//! - the live `SubagentStart` / `SubagentUpdate` / `SubagentDone` events
//!   (on the turn's own stream; a start goes to its call, still
//!   unanswered, matched by task (and label), then confirmed by the
//!   result);
//! - `SubagentRows`, 1 Hz while a turn runs and after it while a worker
//!   still does (how a worker that outlives its turn keeps moving);
//! - the completion event in the history (`<event … source="subagent">`)
//!   and `subagent_collect` / `subagent_status` results: how one ended.
//!
//! Nothing is estimated: a worker whose start is only in the history and
//! that no live source has spoken of since is "started", not "running",
//! and so is one that was running when this window last looked (a
//! re-attach). An end read from a `SubagentDone`'s text is a guess until
//! the daemon's own word (a row, the completion event, a collect, the
//! blocking call's result) says how it ended: that may correct it, once.
//! Nothing here ticks: the elapsed time moves when a row comes.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::SharedString;
use serde_json::Value;
use synaps_client::protocol::{AgentEvent, LlmEvent, SessionEvent, StreamEvent};

/// `SubagentStart`'s `task_preview`: the task's first this many chars.
const PREVIEW_CHARS: usize = 80;
/// The daemon's longest label for a worker named by the caller.
const LABEL_CHARS: usize = 32;
/// Reshaped calls kept for [`Subagents::take_reshaped`]; past this, "all".
const RESHAPED_MAX: usize = 64;

/// Shape generations, unique across every tab's [`Subagents`] (a tab
/// switch never meets the number the rows were built at by chance).
static SHAPES: AtomicU64 = AtomicU64::new(1);

/// The tools that start a worker.
pub fn starts_worker(name: &str) -> bool {
    matches!(name, "subagent_start" | "subagent" | "subagent_resume")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// Its start is in the history only (or it was running when this
    /// window last looked): whether it still runs, no live source has said.
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
    /// "started" < live "running" < how it ended.
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
            Value::String(s) => Some((Self::word(s)?, None)),
            Value::Object(o) => o.get("failed").map(|r| (Status::Failed, r.as_str().map(str::to_string))),
            _ => None,
        }
    }

    fn word(s: &str) -> Option<Self> {
        Some(match s {
            "running" => Status::Running,
            "completed" => Status::Done,
            "cancelled" => Status::Cancelled,
            "timed_out" => Status::TimedOut,
            "failed" => Status::Failed,
            _ => return None,
        })
    }
}

/// How a worker ended, from its live `SubagentDone`'s `result_preview` (a
/// guess, see [`Worker::guessed`]): `subagent_start` and `subagent_resume`
/// workers send `ERROR: <reason>` for every end that is not a success, the
/// reason `worker timed out` or `worker was cancelled` for those two; the
/// blocking `subagent` sends its result's head (`[TIMED OUT after Ns — …`
/// when its time ran out), `ERROR: <error>` or `Task panicked or dropped`.
fn done_status(preview: &str) -> (Status, Option<String>) {
    match preview {
        "ERROR: worker timed out" => (Status::TimedOut, None),
        "ERROR: worker was cancelled" => (Status::Cancelled, None),
        "Task panicked or dropped" => (Status::Failed, Some(preview.to_string())),
        p if p.starts_with("[TIMED OUT") => (Status::TimedOut, None),
        p => match p.strip_prefix("ERROR: ") {
            Some(e) => (Status::Failed, Some(e.trim().to_string())),
            None => (Status::Done, None),
        },
    }
}

/// A reason worth a line: not the one the state's word already says (the
/// daemon's `failure_reason` for a timeout or a cancel).
fn reason_shown(status: Option<Status>, reason: Option<&str>) -> Option<&str> {
    reason.filter(|r| {
        !matches!((status, *r), (Some(Status::TimedOut), "worker timed out") | (Some(Status::Cancelled), "worker was cancelled"))
    })
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
    /// Its end was read from a `SubagentDone`'s text ([`done_status`]):
    /// the daemon's own word may still correct it, once.
    guessed: bool,
}

impl Worker {
    fn ended(&self) -> bool {
        self.status.is_some_and(Status::terminal)
    }

    /// Whether a source may still write how it ended (`sure`: the daemon's
    /// own word, not a guess from text): it has not ended, or its end was
    /// a guess and this source is sure.
    fn open(&self, sure: bool) -> bool {
        !self.ended() || (sure && self.guessed)
    }

    /// A weaker source never overrides a stronger one, and an end is
    /// final, unless it was a guess and a sure source says how it ended.
    fn set_status(&mut self, s: Status, reason: Option<String>, sure: bool) {
        if let Some(c) = self.status {
            let correct = c.terminal() && s.terminal() && sure && self.guessed;
            if !correct && (c.terminal() || c.rank() > s.rank()) {
                return;
            }
            if correct {
                self.reason = None;
            }
        }
        self.status = Some(s);
        self.guessed = s.terminal() && !sure;
        if reason.is_some() {
            self.reason = reason;
        }
    }

    /// Running when this window last looked: "started" until a live
    /// source says again. Whether it was running.
    fn demote(&mut self) -> bool {
        let was = self.status.is_some_and(Status::running);
        if was {
            self.status = Some(Status::Started);
        }
        was
    }

    /// Facts from another source of the same worker: the stronger status,
    /// and what this one lacks.
    fn merge(&mut self, o: Worker) {
        self.id = self.id.or(o.id);
        if let Some(s) = o.status {
            self.set_status(s, o.reason, !o.guessed);
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
    /// Running (or last seen so): its step; finished: its result, or why
    /// it failed.
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
        let reason = reason_shown(w.status, w.reason.as_deref()).filter(|_| matches!(w.status, Some(Status::Failed | Status::TimedOut)));
        let line = match (w.status, reason) {
            (_, Some(r)) => Some((format!("reason: {}", one_line(r, 200)).into(), true)),
            (Some(s), _) if s.terminal() => (!w.result.is_empty()).then(|| (w.result.clone().into(), false)),
            // Running, or last seen running (a re-attach, until a row says):
            // its step (none for a start the history alone holds).
            (Some(_), _) => (!w.step.is_empty()).then(|| (one_line(&w.step, 200).into(), false)),
            (None, _) => None,
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

/// `3.4s`, `59.9s`, `2m 05s`, `1h 02m` (the TUI's `fmt_secs`), the unit
/// chosen after rounding (59.96 is `1m 00s`, not `60.0s`).
pub fn fmt_secs(secs: f64) -> String {
    if (secs * 10.0).round() < 600.0 {
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

/// The daemon's `subagent_label`: an agent by its name (one given as a
/// path by its file stem), else an inline worker's `name`, else "inline";
/// a stem or a name cut to `[A-Za-z0-9_-]` (whitespace becomes `-`), at
/// most [`LABEL_CHARS`].
fn label(agent: Option<&str>, name: Option<&str>) -> String {
    match agent {
        Some(a) if a.contains('/') => {
            std::path::Path::new(a).file_stem().and_then(|f| f.to_str()).and_then(sanitize_label).unwrap_or_else(|| "agent".into())
        }
        Some(a) => a.to_string(),
        None => name.and_then(sanitize_label).unwrap_or_else(|| "inline".into()),
    }
}

/// The daemon's `sanitize_label`.
fn sanitize_label(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = if c.is_whitespace() { '-' } else { c };
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') || (c == '-' && (out.is_empty() || out.ends_with('-'))) {
            continue;
        }
        out.push(c);
        if out.len() == LABEL_CHARS {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    (!out.is_empty()).then(|| out.to_string())
}

/// What a call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `subagent_start`: a background worker, its result names it.
    Start,
    /// `subagent_resume`: a new worker on an ended one's conversation, its
    /// result names it.
    Resume,
    /// The blocking `subagent`: no handle; its result is how it ended.
    Oneshot,
}

#[derive(Clone, Debug)]
struct Call {
    kind: Kind,
    /// What the daemon calls its worker (`subagent_label`); a resume's, the
    /// ended worker's.
    label: String,
    /// The task's (a resume's instructions') first [`PREVIEW_CHARS`]
    /// chars, as `SubagentStart` has it.
    task: String,
    /// `label · the task's first line`, the row's argument.
    arg: SharedString,
    /// The model it asked for (the worker's own, once a row says, wins).
    model: String,
    /// Its result came: no live start can be its any more.
    answered: bool,
    /// Its worker is the one its own result named, not a match: no other
    /// call ever takes it.
    named: bool,
    w: Worker,
    view: Option<View>,
    shape: u64,
}

impl Call {
    /// `prior`: a resume's ended worker's label, if known.
    fn new(name: &str, input: &Value, prior: Option<String>) -> Self {
        let s = |k: &str| input.get(k).and_then(Value::as_str);
        // The daemon reads a blank agent (spaces, controls) as none.
        let agent = s("agent").filter(|a| !a.chars().all(|c| c.is_whitespace() || c.is_control()));
        let (kind, label, task_full) = match name {
            "subagent_resume" => {
                (Kind::Resume, prior.unwrap_or_else(|| s("handle_id").unwrap_or("resume").to_string()), s("instructions").unwrap_or(""))
            }
            "subagent" => (Kind::Oneshot, label(agent, s("name")), s("task").unwrap_or("")),
            _ => (Kind::Start, label(agent, s("name")), s("task").unwrap_or("")),
        };
        let gist = one_line(task_full, 160);
        let arg = if gist.is_empty() { label.clone() } else { format!("{label} · {gist}") };
        Call {
            kind,
            task: task_full.chars().take(PREVIEW_CHARS).collect(),
            arg: arg.into(),
            model: s("model").map(str::trim).unwrap_or("").to_string(),
            w: Worker::default(),
            label,
            answered: false,
            named: false,
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

/// A completion event's facts.
struct Completion {
    n: u64,
    status: Status,
    reason: Option<String>,
    secs: Option<f64>,
    name: Option<String>,
    preview: String,
}

/// A subagent completion event's body (after its `<event …>` tag): from
/// its Data, else from its text. The daemon cuts Data at 1,000 chars, so a
/// long error leaves it unreadable; the text says `Subagent 'x' (sa_2)
/// finished with status 'failed' after 3.5s. Reason: … Call
/// subagent_collect with handle_id "sa_2" to retrieve the full result.
/// Preview: …` (the preview keeps its newlines: the last Data line is the
/// event's).
fn completion(body: &str) -> Option<Completion> {
    let body = body.split("</event>").next().unwrap_or(body);
    let (prose, data) = body.rsplit_once("\nData: ").unwrap_or((body, ""));
    let preview = prose.split_once(" to retrieve the full result. Preview: ").map(|(_, p)| first_line(p)).unwrap_or_default();
    if let Ok(d) = serde_json::from_str::<Value>(data.trim()) {
        let n = d.get("handle_id").and_then(Value::as_str).and_then(handle_id).or_else(|| d.get("subagent_id").and_then(Value::as_u64))?;
        let (status, _) = d.get("status").and_then(Status::parse)?;
        return Some(Completion {
            n,
            status,
            reason: d.get("error").and_then(Value::as_str).map(str::to_string),
            secs: d.get("duration_secs").and_then(Value::as_f64),
            name: d.get("agent_name").and_then(Value::as_str).map(str::to_string),
            preview,
        });
    }
    let rest = prose.trim_start().strip_prefix("Subagent '")?;
    let (name, rest) = rest.split_once("' (sa_")?;
    let (n, rest) = rest.split_once(") finished with status '")?;
    let (status, rest) = rest.split_once("' after ")?;
    let (secs, rest) = rest.split_once("s.")?;
    let reason =
        rest.strip_prefix(" Reason: ").and_then(|r| r.split_once(" Call subagent_collect with handle_id ")).map(|(r, _)| r.to_string());
    Some(Completion {
        n: n.parse().ok()?,
        status: Status::word(status)?,
        reason,
        secs: secs.parse().ok(),
        name: Some(name.to_string()),
        preview,
    })
}

/// The workers of the attached session, by the call that started each.
#[derive(Debug, Default)]
pub struct Subagents {
    calls: HashMap<String, Call>,
    /// Calls in the order they came (a live start goes to the oldest that
    /// fits; an id to the newest call that has it).
    order: Vec<String>,
    /// The call each worker is shown under.
    by_id: HashMap<u64, String>,
    /// Workers no call has claimed yet (a start's result will name them).
    orphans: HashMap<u64, Worker>,
    shapes: u64,
    /// Calls whose block changed shape since [`Subagents::take_reshaped`].
    reshaped: Vec<String>,
    /// Too many to list, or all may have (a new history).
    reshaped_all: bool,
    /// A live source has spoken of a worker since the attach.
    heard: bool,
    /// Bumped on every change to any worker ([`Subagents::revision`]).
    revision: u64,
}

/// A worker as the tray over the composer lists it ([`Subagents::live`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    pub id: u64,
    pub name: String,
    pub status: Status,
    /// Running: what it is doing (its step, without the daemon's
    /// prefixes); ended: its result's first line, or why it failed.
    pub line: String,
    pub tools: Option<u32>,
    pub elapsed: Option<f64>,
    /// A background worker (`sa_N`, `subagent_start` / `subagent_resume`):
    /// its journal can be watched and it can be steered. The blocking
    /// `subagent`'s cannot: the daemon's registry does not hold it.
    pub watchable: bool,
}

/// A running worker's step as the tray says it (the TUI's `step()`):
/// `⚙ bash (tool #3)` is `bash`, a start or a bare `running` is
/// `starting…`, any thinking is `thinking…`.
pub fn tray_step(step: &str) -> String {
    let st = step.trim();
    if st.is_empty() || st.starts_with("starting") || st == "running" {
        "starting…".into()
    } else if let Some(rest) = st.strip_prefix("\u{2699} ") {
        rest.rsplit_once(" (tool #").map_or(rest, |(tool, _)| tool).to_string()
    } else if st.contains("thinking") {
        "thinking…".into()
    } else {
        one_line(st, 200)
    }
}

impl Subagents {
    /// Bumped when a block changes shape: a line comes or goes, its state
    /// changes (the rows' heights and the runs' folds follow).
    pub fn shape_generation(&self) -> u64 {
        self.shapes
    }

    /// The calls whose block changed shape since the last take; `None`:
    /// any may have (rebuild from the first).
    pub fn take_reshaped(&mut self) -> Option<Vec<String>> {
        let list = std::mem::take(&mut self.reshaped);
        (!std::mem::take(&mut self.reshaped_all)).then_some(list)
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

    /// Background workers running, as the blocks say (and those no call
    /// claims), counted as the daemon counts them (SynapsCLI #410): not
    /// one whose cancel was asked for, and no blocking `subagent`'s (it
    /// never outlives its turn, and the daemon's registry does not hold
    /// it).
    pub fn running_count(&self) -> usize {
        let run = |w: &Worker| w.status == Some(Status::Running);
        self.calls.values().filter(|c| c.kind != Kind::Oneshot && run(&c.w)).count() + self.orphans.values().filter(|w| run(w)).count()
    }

    /// A live source (an event, a row) has spoken of a worker since the
    /// attach: [`Subagents::running_count`] is the daemon's, not only what
    /// the history says.
    pub fn heard(&self) -> bool {
        self.heard
    }

    /// Bumped on every change to any worker (state, step, tools, elapsed,
    /// result), a new history included.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Every worker with an id this window has heard of, oldest first.
    pub fn live(&self) -> Vec<Live> {
        let of = |w: &Worker, label: &str, watchable: bool| {
            let id = w.id?;
            let status = w.status?;
            let reason = reason_shown(w.status, w.reason.as_deref()).filter(|_| matches!(status, Status::Failed | Status::TimedOut));
            let line = match reason {
                Some(r) => format!("reason: {}", one_line(r, 200)),
                None if status.terminal() => w.result.clone(),
                None => tray_step(&w.step),
            };
            let name = if w.name.is_empty() { label.to_string() } else { w.name.clone() };
            Some(Live { id, name, status, line, tools: w.tools, elapsed: w.elapsed, watchable })
        };
        // Each id is the newest call's that has it (`by_id`): ids restart
        // at 1 with a cold-started daemon, and an earlier call's worker
        // under the same id is not this one.
        let mut out: Vec<Live> = self
            .by_id
            .values()
            .filter_map(|t| self.calls.get(t).and_then(|c| of(&c.w, &c.label, c.kind != Kind::Oneshot)))
            .chain(self.orphans.iter().filter(|(n, _)| !self.by_id.contains_key(n)).filter_map(|(_, w)| of(w, "subagent", true)))
            .collect();
        out.sort_by_key(|l| l.id);
        out.dedup_by_key(|l| l.id);
        out
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
            .sum::<usize>()
            + self.orphans.values().map(|w| 128 + w.step.len() + w.result.len()).sum::<usize>()
    }

    /// Start over from a whole history (an attach, a replacement). `same`:
    /// the same session, whose workers' live state stays (a replacement
    /// mid-run repeats the calls; the rows keep coming).
    pub fn reset(&mut self, messages: &[Value], same: bool) {
        if !same {
            *self = Subagents {
                shapes: SHAPES.fetch_add(1, Ordering::Relaxed),
                reshaped_all: true,
                revision: self.revision + 1,
                ..Subagents::default()
            };
        }
        self.revision += 1;
        self.append(messages);
        // Each id goes to the newest call that has it: a live call the
        // history does not hold yet keeps its worker, whatever older call
        // the history named it for.
        for t in &self.order {
            let Some(c) = self.calls.get(t) else { continue };
            if let Some(n) = c.w.id.filter(|_| c.named || !c.answered) {
                self.by_id.insert(n, t.clone());
                self.orphans.remove(&n);
            }
        }
    }

    /// A re-attach of the session these are of: a worker running when this
    /// window last looked is "started" until a live source says again (one
    /// cancelled meanwhile sends no wake, and the rows have stopped), and
    /// nothing has been heard in this attachment yet. Before the history.
    pub fn reattached(&mut self) {
        self.heard = false;
        for w in self.orphans.values_mut() {
            w.demote();
        }
        let ids: Vec<String> = self.calls.iter_mut().filter_map(|(id, c)| c.w.demote().then(|| id.clone())).collect();
        for id in ids {
            self.changed(&id);
        }
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
                AgentEvent::SubagentStart { subagent_id, agent_name, task_preview } => {
                    self.heard = true;
                    self.started(*subagent_id, agent_name, task_preview)
                }
                AgentEvent::SubagentUpdate { subagent_id, status, .. } => {
                    self.heard = true;
                    self.update(*subagent_id, |w| {
                        if w.ended() {
                            return;
                        }
                        // "⚙ <tool> (tool #N)" marks a new tool call.
                        if let Some((_, n)) = status.strip_prefix("\u{2699} ").and_then(|r| r.rsplit_once(" (tool #")) {
                            w.tools = n.trim_end_matches(')').parse().ok().or(w.tools);
                        }
                        w.step = status.clone();
                    })
                }
                AgentEvent::SubagentDone { subagent_id, result_preview, duration_secs, .. } => {
                    self.heard = true;
                    let (s, reason) = done_status(result_preview);
                    self.update(*subagent_id, |w| {
                        if w.open(false) {
                            w.set_status(s, reason, false);
                            w.elapsed = Some(*duration_secs);
                            if s == Status::Done {
                                w.result = first_line(result_preview);
                            }
                        }
                    })
                }
                _ => {}
            },
            SessionEvent::SubagentRows { rows } => {
                self.heard = true;
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
        // A resume runs under the ended worker's label.
        let prior = (name == "subagent_resume")
            .then(|| {
                let n = input.get("handle_id").and_then(Value::as_str).and_then(handle_id)?;
                let c = self.calls.get(self.by_id.get(&n)?)?;
                Some(if c.w.name.is_empty() { c.label.clone() } else { c.w.name.clone() })
            })
            .flatten();
        self.calls.insert(id.to_string(), Call::new(name, input, prior));
        self.order.push(id.to_string());
    }

    fn tool_result(&mut self, id: &str, text: &str, live: bool) {
        let Some(c) = self.calls.get_mut(id) else { return self.read_end(text) };
        c.answered = true;
        if c.kind == Kind::Oneshot {
            // `[subagent:<label>] <output>` / `[subagent:<label> ERROR] <error>`;
            // out of time, `[subagent:<label>] [TIMED OUT after Ns — partial
            // results below]…`. Its own word: how it ended, and its result
            // (twin calls have no handle to undo a swapped match).
            if let Some((head, body)) = text.strip_prefix("[subagent:").and_then(|r| r.split_once("] ")) {
                let (s, reason, result) = if head.ends_with(" ERROR") {
                    (Status::Failed, Some(body.trim().to_string()), String::new())
                } else if body.starts_with("[TIMED OUT") {
                    (Status::TimedOut, None, String::new())
                } else {
                    (Status::Done, None, first_line(body))
                };
                if c.w.open(true) {
                    c.w.set_status(s, reason, true);
                    c.w.result = result;
                }
                if c.w.name.is_empty() {
                    c.w.name = head.trim_end_matches(" ERROR").to_string();
                }
            } else if c.w.status.is_some() && !c.w.ended() {
                // Any other result ends its worker too. A HALT (or a cost
                // cap, a host stop) drops the tool: no `SubagentDone`
                // comes, and no row ever reports a blocking worker. Its
                // result is then `Canceled …`; else the tool's own error.
                let (s, reason) = if text.starts_with("Canceled") || text.contains("\n\n[Canceled") {
                    (Status::Cancelled, None)
                } else {
                    (Status::Failed, Some(first_line(text)))
                };
                c.w.set_status(s, reason, true);
            }
            return self.changed(id);
        }
        // `{"handle_id": "sa_N", "agent_name": …, "status": "running"}`.
        let v = serde_json::from_str::<Value>(text.trim()).ok();
        let Some(n) = v.as_ref().and_then(|v| v.get("handle_id")?.as_str().and_then(handle_id)) else {
            // No worker (denied, refused, failed): one a match gave it is
            // unclaimed again.
            self.unclaim(id);
            return self.changed(id);
        };
        self.bind(id, n, true);
        let c = self.calls.get_mut(id).expect("bound");
        if let Some(name) = v.as_ref().and_then(|v| v.get("agent_name")?.as_str()) {
            c.w.name = name.to_string();
        }
        c.w.set_status(if live { Status::Running } else { Status::Started }, None, true);
        self.changed(id);
    }

    /// How a worker ended, from a call that reads it (`subagent_collect`,
    /// `subagent_status`): final states only (a "running" here is old).
    /// Its result from `output` only: `subagent_status`'s `partial_output`
    /// is the output's last 500 chars, not its start.
    fn read_end(&mut self, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text.trim()) else { return };
        let Some(n) = v.get("handle_id").and_then(Value::as_str).and_then(handle_id) else { return };
        let Some((s, reason)) = v.get("status").and_then(Status::parse).filter(|(s, _)| s.terminal()) else { return };
        let reason = reason.or_else(|| v.get("error").and_then(Value::as_str).map(str::to_string));
        let output = v.get("output").and_then(Value::as_str).map(first_line);
        let secs = v.get("elapsed_secs").and_then(Value::as_f64);
        let tools = v.get("tool_count").and_then(Value::as_u64).map(|t| t as u32);
        self.update(n, |w| {
            if w.open(true) {
                w.set_status(s, reason, true);
                w.elapsed = secs.or(w.elapsed);
                w.tools = tools.or(w.tools);
            }
            w.tools = w.tools.or(tools);
            if let Some(o) = output.filter(|o| !o.is_empty() && w.result.is_empty()) {
                w.result = o;
            }
        });
    }

    /// Completion events in a user message of the history (one per event).
    fn events(&mut self, text: &str) {
        if !text.contains("source=\"subagent\"") {
            return;
        }
        for ev in text.split("<event ").skip(1) {
            let Some((tag, body)) = ev.split_once('>') else { continue };
            if !tag.contains("source=\"subagent\"") {
                continue;
            }
            let Some(c) = completion(body) else { continue };
            self.update(c.n, |w| {
                if w.open(true) {
                    w.set_status(c.status, c.reason, true);
                    w.elapsed = c.secs.or(w.elapsed);
                }
                if w.result.is_empty() {
                    w.result = c.preview;
                }
                if let Some(name) = c.name.filter(|_| w.name.is_empty()) {
                    w.name = name;
                }
            });
        }
    }

    /// A live start: under the call it is (its own, if one already named
    /// it), else the oldest unanswered call with its task (its label too,
    /// if one has it). A start that fits no task is under no call: a label
    /// alone is never a call's own start (`subagent_resume` starts its
    /// worker under the ended one's label).
    fn started(&mut self, n: u64, name: &str, preview: &str) {
        let mut best: Option<(&String, u8)> = None;
        for t in &self.order {
            if let Some(c) = self.calls.get(t).filter(|c| !c.answered && c.w.id.is_none() && c.task == preview) {
                let s = 1 + u8::from(c.label == name);
                // Strictly better: the oldest of equals.
                if best.is_none_or(|(_, b)| s > b) {
                    best = Some((t, s));
                }
            }
        }
        let fit = best.map(|(t, _)| t.clone());
        let prev_runs = self.by_id.get(&n).map(|p| self.calls.get(p).is_some_and(|c| c.w.status.is_some_and(Status::running)));
        match (prev_runs, fit) {
            // A new worker under an id an earlier one had (ids restart at 1
            // with a cold-started daemon): its call is here, and the earlier
            // worker is not running. That call keeps what it showed.
            (Some(false), Some(t)) => {
                self.by_id.remove(&n);
                self.bind(&t, n, false);
            }
            (None, Some(t)) => self.bind(&t, n, false),
            _ => {}
        }
        self.update(n, |w| {
            if w.name.is_empty() {
                w.name = name.to_string();
            }
            w.set_status(Status::Running, None, true);
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
        let cancel = r.get("cancel_requested").and_then(Value::as_bool).unwrap_or(false);
        let status = r.get("status").and_then(Status::parse).map(|(st, reason)| match st {
            Status::Running if cancel => (Status::Cancelling, reason),
            st => (st, reason),
        });
        let elapsed = r.get("elapsed_secs").and_then(Value::as_f64);
        // `finished_elapsed` (a serde `Duration`): how long ago it ended.
        let ago = r
            .get("finished_elapsed")
            .and_then(|d| Some(d.get("secs")?.as_f64()? + d.get("nanos").and_then(Value::as_f64).unwrap_or(0.) / 1e9));
        let (name, model, step) = (s("agent_name"), s("model"), s("step"));
        let tools = r.get("tools").and_then(Value::as_u64).map(|t| t as u32);
        self.update(n, |w| {
            // Once it ended, a row sent before the end (they cross) changes
            // nothing; one that says how it ended may correct a guess.
            let ends = status.as_ref().is_some_and(|(s, _)| s.terminal());
            if w.ended() && !(ends && w.open(true)) {
                return;
            }
            if let Some((st, reason)) = status {
                w.set_status(st, reason, true);
            }
            // A finished worker's run: its age less the time since it ended.
            if let Some(e) = elapsed {
                w.elapsed = Some(match (w.ended(), ago) {
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

    /// Show worker `n` under call `tool_id`; `named`: its own result named
    /// it. What a match had given another call of it comes along (and a
    /// worker a wrong match gave this call is unclaimed again); a worker
    /// another call's own result named stays with that call (`n` was
    /// reused), and `n` is this call's from now on. History order decides
    /// which call an id's events reach.
    fn bind(&mut self, tool_id: &str, n: u64, named: bool) {
        let Some(c) = self.calls.get_mut(tool_id) else { return };
        if self.by_id.get(&n).map(String::as_str) == Some(tool_id) {
            c.named |= named;
            return;
        }
        // Its own result read again (a replacement repeats the history)
        // while a later call has `n`: `n`'s history from here on is this
        // call's, and the later call keeps its worker (the end of the
        // reset gives `n` back to it).
        if c.w.id == Some(n) && self.by_id.contains_key(&n) {
            c.named |= named;
            self.by_id.insert(n, tool_id.to_string());
            return;
        }
        if let Some(old) = c.w.id.filter(|&o| o != n) {
            let lost = std::mem::take(&mut c.w);
            if self.by_id.get(&old).map(String::as_str) == Some(tool_id) {
                self.by_id.remove(&old);
                self.orphans.insert(old, lost);
            }
        }
        let w = match self.by_id.remove(&n) {
            Some(prev) => {
                let w = match self.calls.get_mut(&prev) {
                    Some(p) if !p.named => std::mem::take(&mut p.w),
                    _ => Worker::default(),
                };
                self.changed(&prev);
                w
            }
            None => self.orphans.remove(&n).unwrap_or_default(),
        };
        let c = self.calls.get_mut(tool_id).expect("checked");
        c.w.merge(w);
        c.w.id = Some(n);
        c.named |= named;
        self.by_id.insert(n, tool_id.to_string());
        self.changed(tool_id);
    }

    /// Call `tool_id` started no worker after all: one a match gave it is
    /// unclaimed again.
    fn unclaim(&mut self, tool_id: &str) {
        let Some(c) = self.calls.get_mut(tool_id).filter(|c| !c.named) else { return };
        let Some(g) = c.w.id else { return };
        let w = std::mem::take(&mut c.w);
        if self.by_id.get(&g).map(String::as_str) == Some(tool_id) {
            self.by_id.remove(&g);
            self.orphans.insert(g, w);
        }
    }

    /// Change worker `n`: under its call, else as an orphan.
    fn update(&mut self, n: u64, f: impl FnOnce(&mut Worker)) {
        self.revision += 1;
        match self.by_id.get(&n).cloned() {
            Some(t) if self.calls.contains_key(&t) => {
                f(&mut self.calls.get_mut(&t).expect("present").w);
                self.changed(&t);
            }
            _ => f(self.orphans.entry(n).or_insert_with(|| Worker { id: Some(n), ..Worker::default() })),
        }
    }

    fn changed(&mut self, tool_id: &str) {
        self.revision += 1;
        let Some(c) = self.calls.get_mut(tool_id) else { return };
        let shape = c.shape;
        if c.refresh() && c.shape != shape {
            self.shapes = SHAPES.fetch_add(1, Ordering::Relaxed);
            if self.reshaped.len() >= RESHAPED_MAX {
                self.reshaped.clear();
                self.reshaped_all = true;
            } else if !self.reshaped_all && !self.reshaped.iter().any(|t| t == tool_id) {
                self.reshaped.push(tool_id.to_string());
            }
        }
    }
}

/// A completion event (a history notice) as the TUI says it: `scout
/// completed · 41.0s · <its result's first line>`, or why it failed.
/// `None`: not one.
pub fn completion_note(text: &str) -> Option<String> {
    let ev = text.strip_prefix("<event ")?;
    let (tag, body) = ev.split_once('>')?;
    if tag.contains("type=\"subagent_steer\"") {
        return steer_note(body);
    }
    if !tag.contains("source=\"subagent\"") {
        return None;
    }
    let c = completion(body)?;
    let word = match c.status {
        Status::Done => "completed",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::TimedOut => "timed out",
        _ => return None,
    };
    let mut out = format!("{} {word}", c.name.as_deref().unwrap_or("subagent"));
    if let Some(s) = c.secs {
        out.push_str(&format!(" · {}", fmt_secs(s)));
    }
    let tail = match reason_shown(Some(c.status), c.reason.as_deref()) {
        Some(e) => format!("reason: {}", one_line(e, 160)),
        None => c.preview,
    };
    if !tail.is_empty() {
        out.push_str(&format!(" · {tail}"));
    }
    Some(out)
}

/// The engine's note of a worker steered from its view (`User steered
/// subagent 'sleeper' (sa_1) directly: TEXT`, then its data) as `you
/// steered sleeper (sa_1): TEXT`.
fn steer_note(body: &str) -> Option<String> {
    let body = body.strip_suffix("</event>").unwrap_or(body);
    let body = body.rsplit_once("\nData: ").map_or(body, |(b, _)| b);
    let rest = body.strip_prefix("User steered subagent '")?;
    let (name, rest) = rest.split_once("' (")?;
    let (handle, text) = rest.split_once(") directly: ")?;
    Some(format!("you steered {name} ({handle}): {}", one_line(text, 400)))
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
    /// A completion event as the daemon writes it (`finalize.rs`,
    /// `format_event_for_agent`).
    fn completion_event(n: u64, name: &str, status: &str, secs: f64, error: Option<&str>, preview: &str) -> String {
        let reason = error.map(|e| format!(" Reason: {e}")).unwrap_or_default();
        let mut data =
            json!({"handle_id": format!("sa_{n}"), "subagent_id": n, "agent_name": name, "status": status, "duration_secs": secs});
        if let Some(e) = error {
            data["error"] = json!(e);
        }
        format!(
            "<event id=\"e{n}\" type=\"subagent_completion\" severity=\"high\" source=\"subagent\">Subagent '{name}' (sa_{n}) finished with status '{status}' after {secs:.1}s.{reason} Call subagent_collect with handle_id \"sa_{n}\" to retrieve the full result. Preview: {preview}\nData: {data}</event>"
        )
    }
    fn user(text: &str) -> Value {
        json!({"role": "user", "content": text})
    }
    fn call(id: &str, name: &str, input: Value) -> Value {
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]})
    }
    fn answer(id: &str, text: &str) -> Value {
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]})
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
    /// Labels as the daemon makes them.
    #[test]
    fn cancelled_and_timed_out() {
        let mut s = Subagents::default();
        tool_use(&mut s, "a", "subagent_start", json!({"name": " GIF recorder! ", "system_prompt": "x", "task": "record"}));
        tool_use(&mut s, "b", "subagent_start", json!({"agent": "/home/me/agents/bench.md", "task": "bench it"}));
        tool_use(&mut s, "c", "subagent_start", json!({"agent": "  ", "system_prompt": "x", "task": "nameless"}));
        result(&mut s, "a", &handle(1, "GIF-recorder"));
        result(&mut s, "b", &handle(2, "bench"));
        assert_eq!(s.arg("a").as_deref(), Some("GIF-recorder · record"), "a name cut as the daemon cuts it");
        assert_eq!(s.arg("b").as_deref(), Some("bench · bench it"), "a path's file stem");
        assert_eq!(s.arg("c").as_deref(), Some("inline · nameless"), "a blank agent is none");
        let mut r = row(1, "gif-recorder", json!("running"), 5.0, None, "", 0);
        r["cancel_requested"] = json!(true);
        rows(&mut s, json!([r]));
        assert_eq!(word(&s, "a"), Some("cancelling"));
        assert!(s.running("a"));
        rows(&mut s, json!([row(1, "gif-recorder", json!("cancelled"), 6.0, Some(0.0), "", 0)]));
        assert_eq!((word(&s, "a"), s.view("a").unwrap().tone), (Some("cancelled"), Tone::Warn));
        // A `subagent_start` worker's end that is not a success: `ERROR: <failure_reason>`.
        done(&mut s, 2, "ERROR: worker timed out", 600.0);
        assert_eq!(shown(&s, "b").map(|v| (v.0, v.2, v.3)), Some(("timed out", "10m 00s".into(), None)));
    }

    /// A `subagent_start` worker timed out or cancelled mid-turn says so
    /// (not "failed"); "ERRORS: none" is a success. An end read from the
    /// done event's text gives way, once, to the daemon's own word (a row,
    /// the completion event, a collect); that word is final.
    #[test]
    fn reactive_ends_and_the_daemons_word() {
        let mut s = Subagents::default();
        for (id, n) in [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5)] {
            tool_use(&mut s, id, "subagent_start", json!({"agent": "w", "task": id}));
            result(&mut s, id, &handle(n, "w"));
        }
        done(&mut s, 1, "ERROR: worker timed out", 600.0);
        done(&mut s, 2, "ERROR: worker was cancelled", 5.0);
        done(&mut s, 3, "ERRORS: none\nall good", 2.0);
        done(&mut s, 4, "ERROR: provider overloaded [overloaded]", 3.0);
        done(&mut s, 5, "half way", 1.0);
        let line = |s: &Subagents, id: &str| shown(s, id).map(|v| (v.0, v.3));
        assert_eq!(line(&s, "a"), Some(("timed out", None)));
        assert_eq!((line(&s, "b"), s.view("b").unwrap().tone), (Some(("cancelled", None)), Tone::Warn));
        assert_eq!(line(&s, "c"), Some(("completed", Some("ERRORS: none".into()))));
        assert_eq!(line(&s, "d"), Some(("failed", Some("reason: provider overloaded [overloaded]".into()))));
        // The completion event says d timed out: corrected, and final.
        s.append(&[user(&completion_event(4, "w", "timed_out", 3.0, Some("worker timed out"), ""))]);
        assert_eq!(line(&s, "d"), Some(("timed out", None)), "its own reason is not repeated");
        let collect = json!({"handle_id": "sa_4", "status": {"failed": "late"}, "output": "x"}).to_string();
        s.append(&[call("k", "subagent_collect", json!({"handle_id": "sa_4"})), answer("k", &collect)]);
        assert_eq!(word(&s, "d"), Some("timed out"), "the daemon's word stays");
        // A row corrects a guess too; a stale "running" one does not.
        rows(&mut s, json!([row(5, "w", json!("running"), 0.9, None, "late", 1)]));
        assert_eq!(line(&s, "e"), Some(("completed", Some("half way".into()))));
        rows(&mut s, json!([row(5, "w", json!("cancelled"), 2.0, Some(1.0), "", 1)]));
        assert_eq!(shown(&s, "e").map(|v| (v.0, v.2)), Some(("cancelled", "claude-sonnet-5-5 · 1 tool · 1.0s".into())));
        // And a guess never overrides the daemon's word.
        done(&mut s, 5, "ERROR: boom", 2.0);
        assert_eq!(word(&s, "e"), Some("cancelled"));
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
        // Out of time, from the history alone: timed out, not completed.
        h.append(&[
            call("g", "subagent", json!({"agent": "scout", "task": "slow"})),
            answer("g", "[subagent:scout] [TIMED OUT after 600s \u{2014} partial results below]\n\n$ ls\n"),
        ]);
        assert_eq!(shown(&h, "g").map(|v| (v.0, v.3)), Some(("timed out", None)));
        // Twin blocking calls whose live starts matched the wrong way round:
        // each call's own result says how its worker ended, and with what.
        let mut t = Subagents::default();
        for id in ["p", "q"] {
            tool_use(&mut t, id, "subagent", json!({"agent": "twin", "task": "same"}));
        }
        start(&mut t, 1, "twin", "same");
        start(&mut t, 2, "twin", "same");
        done(&mut t, 2, "second", 1.0);
        done(&mut t, 1, "first", 2.0);
        result(&mut t, "p", "[subagent:twin] second");
        result(&mut t, "q", "[subagent:twin ERROR] boom");
        assert_eq!(shown(&t, "p").map(|v| (v.0, v.3)), Some(("completed", Some("second".into()))));
        assert_eq!(shown(&t, "q").map(|v| (v.0, v.3)), Some(("failed", Some("reason: boom".into()))));
    }

    /// N-M1: a blocking `subagent` cut off by a HALT, a cost cap or a host
    /// stop. The tool is dropped (no `SubagentDone`, and no row reports a
    /// blocking worker); its result is `Canceled …`, partial output first
    /// or not. Its worker is over: cancelled, and nothing counts it. Any
    /// other result ends it too (failed, its first line the reason); a
    /// call that never started one shows nothing, and an end already
    /// heard stays.
    #[test]
    fn a_blocking_subagent_cut_off_is_over() {
        let ids = ["o", "p", "q", "r", "e"];
        let mut s = Subagents::default();
        for (n, id) in ids.into_iter().enumerate() {
            tool_use(&mut s, id, "subagent", json!({"agent": "digger", "task": id}));
            start(&mut s, n as u64 + 1, "digger", id);
        }
        tool_use(&mut s, "never", "subagent", json!({"agent": "digger", "task": "never"}));
        update(&mut s, 1, "\u{2699} bash (tool #1)");
        assert!(s.running("o") && s.heard());
        assert_eq!(s.running_count(), 0, "blocking workers are not background ones");
        done(&mut s, 5, "fine", 1.0);
        result(&mut s, "o", "Canceled by user");
        result(&mut s, "p", "$ ls\n\n[Canceled (session cost cap reached) before the tool finished; the output above is partial.]");
        result(&mut s, "q", "Canceled (session stopped by the host). It had already started and may have partially applied its effects.");
        result(&mut s, "r", "worker thread initialization failed\nmore");
        result(&mut s, "e", "Canceled by user");
        result(&mut s, "never", "Canceled by user");
        let words = ids.map(|id| word(&s, id));
        assert_eq!(words, [Some("cancelled"), Some("cancelled"), Some("cancelled"), Some("failed"), Some("completed")]);
        assert_eq!(shown(&s, "o").unwrap().3, None, "no reason line: the word says it");
        assert_eq!(shown(&s, "r").unwrap().3.as_deref(), Some("reason: worker thread initialization failed"));
        assert_eq!(word(&s, "never"), None, "never started: no block");
        assert!(ids.iter().all(|id| !s.running(id)));
    }

    /// N-L2: a worker whose cancel was asked for runs until its next step,
    /// but the daemon no longer counts it (#410), so neither does this.
    /// Its run stays open until it ends.
    #[test]
    fn a_cancelling_worker_is_not_counted() {
        let mut s = Subagents::default();
        tool_use(&mut s, "t", "subagent_start", json!({"agent": "a", "task": "one"}));
        result(&mut s, "t", &handle(1, "a"));
        rows(&mut s, json!([row(1, "a", json!("running"), 2.0, None, "ls", 1)]));
        assert_eq!(s.running_count(), 1);
        let mut r = row(1, "a", json!("running"), 3.0, None, "ls", 1);
        r["cancel_requested"] = json!(true);
        rows(&mut s, json!([r]));
        assert_eq!((word(&s, "t"), s.running("t"), s.running_count()), (Some("cancelling"), true, 0));
    }

    /// N-L1: a replacement mid-run re-reads an older call's own result
    /// naming `sa_1` while a later blocking call's live worker is `sa_1`
    /// (the ids restarted with the daemon). Neither block takes the
    /// other's worker, and `sa_1`'s events reach the live call again.
    #[test]
    fn a_replace_never_hands_a_live_worker_to_an_older_call() {
        let history = vec![
            call("h1", "subagent_start", json!({"agent": "scout", "task": "map it"})),
            answer("h1", &handle(1, "scout")),
            user(&completion_event(1, "scout", "completed", 4.0, None, "found it")),
            // (the daemon restarted)
            call("o", "subagent", json!({"agent": "digger", "task": "dig"})),
        ];
        let o = |s: &Subagents| shown(s, "o").map(|v| (v.0, v.1, v.3));
        let h1 = |s: &Subagents| shown(s, "h1").map(|v| (v.0, v.1, v.2, v.3));
        let digging = Some(("running", "digger".to_string(), Some("digging".to_string())));
        let ended = Some(("completed", "scout".to_string(), "4.0s".to_string(), Some("found it".to_string())));
        let mut s = Subagents::default();
        s.reset(&history[..3], false);
        tool_use(&mut s, "o", "subagent", json!({"agent": "digger", "task": "dig"}));
        start(&mut s, 1, "digger", "dig");
        update(&mut s, 1, "digging");
        assert_eq!((o(&s), h1(&s)), (digging.clone(), ended.clone()));
        s.reset(&history, true);
        assert_eq!((o(&s), h1(&s)), (digging, ended.clone()), "each keeps its own");
        update(&mut s, 1, "deeper");
        assert_eq!(shown(&s, "o").unwrap().3.as_deref(), Some("deeper"));
        assert_eq!(h1(&s), ended);
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
        assert_eq!((fmt_secs(59.94), fmt_secs(59.96)), ("59.9s".into(), "1m 00s".into()), "rounded before the unit");
        assert_eq!(completion_note("<event id=\"x\" source=\"cli\">hi</event>"), None);
        // A steer from a worker's view, as the engine notes it in the main conversation.
        let steer = "<event id=\"d5\" type=\"subagent_steer\" severity=\"low\" source=\"user\">User steered subagent 'sleeper' (sa_1) directly: Stop after your next step: (a) then STEERED.\nData: {\"agent_name\":\"sleeper\",\"handle_id\":\"sa_1\"}</event>";
        assert_eq!(completion_note(steer).as_deref(), Some("you steered sleeper (sa_1): Stop after your next step: (a) then STEERED."));
        assert_eq!(completion_note("<event id=\"d5\" type=\"subagent_steer\" source=\"user\">something else</event>"), None);
    }

    /// Ids restart at 1 with a cold-started daemon: the history holds an
    /// earlier call whose worker was sa_1 (ended, its completion event
    /// with it) and a later one whose worker is sa_1 again. The live rows
    /// are the later one's; a re-attach replays it all. Neither block ever
    /// takes the other's worker, live or from the history.
    #[test]
    fn a_reused_id_never_moves_a_worker() {
        let history = vec![
            call("h1", "subagent_start", json!({"agent": "scout", "task": "map it"})),
            answer("h1", &handle(1, "scout")),
            user(&completion_event(1, "scout", "completed", 4.0, None, "found it")),
            // (the daemon restarted)
            call("x", "subagent_start", json!({"agent": "scout", "task": "map it again"})),
            answer("x", &handle(1, "scout")),
        ];
        let h1 = |s: &Subagents| shown(s, "h1").map(|v| (v.0, v.2, v.3));
        let ended = Some(("completed", "4.0s".to_string(), Some("found it".to_string())));
        let mut s = Subagents::default();
        s.reset(&history, false);
        assert_eq!((h1(&s), word(&s, "x")), (ended.clone(), Some("started")));
        rows(&mut s, json!([row(1, "scout", json!("running"), 2.0, None, "ls", 1)]));
        assert_eq!(shown(&s, "x").map(|v| (v.0, v.3)), Some(("running", Some("ls".into()))));
        assert_eq!(h1(&s), ended, "h1's facts stay its own");
        assert!(s.running("x") && !s.running("h1"));
        // A re-attach: the same history again, then the rows.
        s.reattached();
        s.reset(&history, true);
        assert_eq!((h1(&s), word(&s, "x")), (ended.clone(), Some("started")));
        rows(&mut s, json!([row(1, "scout", json!("running"), 3.0, None, "ls", 1)]));
        assert_eq!((h1(&s), word(&s, "x")), (ended.clone(), Some("running")));
        // The tray's id 1 is x's (running), never h1's (ended), whatever
        // order the calls sit in (a hash map's: many maps, many orders).
        for _ in 0..16 {
            let mut t = Subagents::default();
            t.reset(&history, false);
            rows(&mut t, json!([row(1, "scout", json!("running"), 3.0, None, "ls", 1)]));
            let live = t.live();
            assert_eq!(live.len(), 1, "one worker per id");
            assert_eq!((live[0].id, live[0].status, live[0].line.as_str()), (1, Status::Running, "ls"));
        }
        rows(&mut s, json!([row(1, "scout", json!("completed"), 9.0, Some(0.0), "ls", 2)]));
        assert_eq!(shown(&s, "x").unwrap().2, "claude-sonnet-5-5 · 2 tools · 9.0s");
        assert_eq!(h1(&s), ended);
        // A start replayed for the worker that ended: nothing moves, nothing runs.
        start(&mut s, 1, "scout", "map it again");
        assert_eq!((word(&s, "x"), s.running_count()), (Some("completed"), 0));

        // Live: the later call's start comes while the history's sa_1 is h1's.
        let mut l = Subagents::default();
        l.reset(&history[..3], false);
        tool_use(&mut l, "x", "subagent_start", json!({"agent": "scout", "task": "map it again"}));
        start(&mut l, 1, "scout", "map it again");
        update(&mut l, 1, "booting");
        result(&mut l, "x", &handle(1, "scout"));
        assert_eq!(shown(&l, "x").map(|v| (v.0, v.3)), Some(("running", Some("booting".into()))));
        assert_eq!(h1(&l), ended);
        // A replacement mid-run repeats the history (h1's result first):
        // the live call keeps its worker and its rows.
        l.reset(&history, true);
        rows(&mut l, json!([row(1, "scout", json!("running"), 4.0, None, "reading", 2)]));
        assert_eq!(shown(&l, "x").map(|v| (v.0, v.3)), Some(("running", Some("reading".into()))));
        assert_eq!(h1(&l), ended);
    }

    /// A re-attach: a worker running when this window last looked is
    /// "started" (its last step still shown) until a live source says
    /// again; one cancelled meanwhile (no wake, the rows stopped) never
    /// comes back, and nothing keeps its run open.
    #[test]
    fn a_reattach_forgets_running_until_a_row_says() {
        let history = vec![
            call("h1", "subagent_start", json!({"agent": "a", "task": "one"})),
            answer("h1", &handle(1, "a")),
            call("h2", "subagent_start", json!({"agent": "b", "task": "two"})),
            answer("h2", &handle(2, "b")),
        ];
        let mut s = Subagents::default();
        s.reset(&history, false);
        assert!(!s.heard(), "the history alone");
        rows(&mut s, json!([row(1, "a", json!("running"), 1.0, None, "ls", 1), row(2, "b", json!("running"), 1.0, None, "cat", 1)]));
        assert_eq!((s.running_count(), s.heard()), (2, true));
        s.reattached();
        s.reset(&history, true);
        assert!(!s.heard());
        assert_eq!(shown(&s, "h1").map(|v| (v.0, v.3)), Some(("started", Some("ls".into()))));
        assert_eq!((s.running("h1"), s.running("h2"), s.running_count()), (false, false, 0));
        rows(&mut s, json!([row(1, "a", json!("running"), 3.0, None, "ls", 1)]));
        assert_eq!((word(&s, "h1"), word(&s, "h2"), s.running_count()), (Some("running"), Some("started"), 1));
    }

    /// A start whose task fits no call is under none, even when its label
    /// does (`subagent_resume` starts under the ended worker's label); a
    /// resume's worker shows under the resume; a call answered without a
    /// handle gives back the worker a match gave it.
    #[test]
    fn starts_fit_by_task_and_resumes_have_blocks() {
        let mut s = Subagents::default();
        tool_use(&mut s, "a", "subagent_start", json!({"agent": "scout", "task": "map it"}));
        result(&mut s, "a", &handle(1, "scout"));
        done(&mut s, 1, "mapped", 3.0);
        // Waiting on its approval: a call with the same label.
        tool_use(&mut s, "b", "subagent_start", json!({"agent": "scout", "task": "something else"}));
        tool_use(&mut s, "r", "subagent_resume", json!({"handle_id": "sa_1", "instructions": "Now map the tests too"}));
        assert_eq!(s.arg("r").as_deref(), Some("scout · Now map the tests too"), "under the ended worker's label");
        start(&mut s, 2, "scout", "Now map the tests too");
        assert_eq!((s.view("b"), word(&s, "r")), (None, Some("running")), "a label alone is not b's start");
        let resumed = json!({"handle_id": "sa_2", "resumed_from": "sa_1", "agent_name": "scout", "status": "running", "context": "full"});
        result(&mut s, "r", &resumed.to_string());
        rows(&mut s, json!([row(2, "scout", json!("running"), 2.0, None, "rg tests", 1)]));
        assert_eq!(shown(&s, "r").map(|v| (v.0, v.3)), Some(("running", Some("rg tests".into()))));
        assert_eq!(word(&s, "a"), Some("completed"));
        result(&mut s, "b", "Tool call blocked by extension: Tool call confirmation denied: no");
        assert_eq!(s.view("b"), None, "denied: no worker");
        // A match, then the call fails without a handle: the worker is unclaimed.
        tool_use(&mut s, "c", "subagent_start", json!({"agent": "x", "task": "t"}));
        start(&mut s, 3, "x", "t");
        assert_eq!(word(&s, "c"), Some("running"));
        result(&mut s, "c", "Tool execution failed: boom");
        assert_eq!(s.view("c"), None);
        assert_eq!(s.running_count(), 2, "r's, and the unclaimed one");
    }

    /// The calls whose block changed shape are listed for the window (it
    /// rebuilds from the earliest one's turn); a new history is "all".
    #[test]
    fn reshaped_calls_are_listed() {
        let mut s = Subagents::default();
        s.reset(&[], false);
        assert_eq!(s.take_reshaped(), None, "a new history: all");
        tool_use(&mut s, "a", "subagent_start", json!({"agent": "a", "task": "one"}));
        tool_use(&mut s, "b", "subagent_start", json!({"agent": "b", "task": "two"}));
        result(&mut s, "b", &handle(2, "b"));
        assert_eq!(s.take_reshaped(), Some(vec!["b".to_string()]));
        let g = s.shape_generation();
        rows(&mut s, json!([row(2, "b", json!("running"), 1.0, None, "ls", 1)]));
        rows(&mut s, json!([row(2, "b", json!("running"), 2.0, None, "ls", 1)]));
        assert_eq!(s.take_reshaped(), Some(vec!["b".to_string()]), "its line came: once");
        assert_ne!(s.shape_generation(), g);
        rows(&mut s, json!([row(2, "b", json!("running"), 3.0, None, "cat", 1)]));
        assert_eq!(s.take_reshaped(), Some(vec![]), "text only");
    }

    /// The completion event: Data is cut at 1,000 chars, so a long error
    /// leaves it unreadable: its text says the same. A preview with lines
    /// of its own; the last Data line is the event's.
    #[test]
    fn completion_events_read_from_data_or_text() {
        let long = "x".repeat(1200);
        let full = completion_event(3, "scout", "failed", 12.5, Some(&long), "");
        let (head, data) = full.split_once("\nData: ").unwrap();
        let cut: String = data.chars().take(1000).collect();
        let truncated = format!("{head}\nData: {cut}</event>");
        let note = completion_note(&truncated).unwrap();
        assert!(note.starts_with("scout failed · 12.5s · reason: xxx"), "{note}");
        let mut s = Subagents::default();
        s.append(&[call("t", "subagent_start", json!({"agent": "scout", "task": "go"})), answer("t", &handle(3, "scout"))]);
        s.append(&[user(&truncated)]);
        assert_eq!((word(&s, "t"), shown(&s, "t").unwrap().2), (Some("failed"), "12.5s".into()));
        let tricky = completion_event(4, "scout", "completed", 1.0, None, "line one\nData: not this\nmore");
        assert_eq!(completion_note(&tricky).as_deref(), Some("scout completed · 1.0s · line one"));
        // `subagent_status`'s partial output is a tail: not a result.
        let status = json!({"handle_id": "sa_3", "status": "completed", "partial_output": "…ail end"}).to_string();
        let mut p = Subagents::default();
        p.append(&[call("t", "subagent_start", json!({"agent": "scout", "task": "go"})), answer("t", &handle(3, "scout"))]);
        p.append(&[call("k", "subagent_status", json!({"handle_id": "sa_3"})), answer("k", &status)]);
        assert_eq!(shown(&p, "t").map(|v| (v.0, v.3)), Some(("completed", None)));
    }
}
