//! The op log (synaps-cards `docs/blocks.md` v1.3 §3.8, §5.2; §8.2's D7
//! row): the board's mutation stream (wild2, point's WIRE).
//!
//! - **Ctrl+O** (Cmd+O on macOS) opens it over the focused pane, on a new
//!   press, never mid IME composition; Ctrl+O again or Esc closes it, and
//!   the keys go back where they were. Tab flips its two tabs.
//! - **Ops** lists the board's latest 256 lines ([`Drawn::log`], the board
//!   drawn, the live turn's included), newest first: the time (its row's,
//!   found by the line's call, a lookup each: never "now", and never a
//!   walk of the history to find it; `—` where the board kept no id for
//!   its call, past [`synaps_client::blocks::Mark`]'s budget), who, the
//!   op, the id, and how it went (applied, placed here, no change,
//!   exists, skipped and why, a path skipped, unbound, a stale notice).
//!   A pending line says so. It is where an agent's skipped op, which the
//!   agent never learns of, becomes visible. Clicking a line whose block
//!   is live jumps to it: the walk entered on it ([`crate::walk`]).
//! - **Before compaction** is the archive (§5.2): the predecessor's final
//!   blocks, whole (each block's contents as canonical JSON, every kind,
//!   a group's members after it, sanitized for the screen), and its op
//!   log, read-only, as text (≤ [`ARCHIVE_MAX`]: past it, the oldest log
//!   lines and then the last blocks' lines go, and a line says what was
//!   cut). Its text can be copied whole. Never a revision of a new block,
//!   never matched by id: its lines don't jump.
//! - **Whose it is** ([`Archives`]): the session's, and the conversation
//!   its last compaction began. It is captured at the compaction, from the
//!   predecessor's final board: its history folded to its end off the
//!   frame by the adapter's own steps (`crate::board::step`), not the
//!   board last painted (one waiting under the lease, a fold in flight).
//!   Every compaction replaces it, an empty predecessor with an empty
//!   archive. Another session, or another conversation of this one (a
//!   clear, a resume), shows none; switching tabs, splitting panes or
//!   reattaching changes nothing of it. At most [`ARCHIVES_MAX`]
//!   sessions' are held; a desk restart loses them. A capture can be the
//!   predecessor's history's last owner: it lets go of it off the frame,
//!   to the app's disposer, however it ends ([`Predecessor`]).
//! - **Derived and read-only**: nothing here changes the board.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cards_spec::board::{Did, Logged, Outcome, PathWhy, Why};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::Transcript;
use synaps_client::blocks::{Drawn, Input, Ref, Shown};

use crate::app::{AppView, display, mono};
use crate::fonts::{self, TextPx as _, size};
use crate::theme::Palette;

/// The archive's text holds at most this many bytes (§1.8).
pub(crate) const ARCHIVE_MAX: usize = 256 * 1024;

/// Sessions whose archive this desk holds at most.
pub(crate) const ARCHIVES_MAX: usize = 8;

/// A cut line's bytes at most.
const CUT_MAX: usize = 160;

/// The log's key context.
pub(crate) const LOG_CTX: &str = "OpLog";

/// A line's height, and the list's at most.
const ROW_H: f32 = 24.;
const LIST_MAX: f32 = 432.;
const WIDTH: f32 = 720.;
const TOP: f32 = 72.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Ops,
    Archive,
}

/// The op log open: its tab, its focus, where the keys were, its scroll
/// (the archive's, a list drawn a line at a time).
pub(crate) struct OpLog {
    pub tab: Tab,
    pub focus: FocusHandle,
    pub back: Option<FocusHandle>,
    pub scroll: ScrollHandle,
    pub archive_scroll: UniformListScrollHandle,
}

/// One line as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub time: String,
    pub who: &'static str,
    pub did: &'static str,
    pub id: String,
    pub outcome: String,
    pub pending: bool,
    /// The block a click jumps to.
    pub jump: Option<Ref>,
}

pub(crate) fn did(d: Did) -> &'static str {
    match d {
        Did::Put => "put",
        Did::Patch => "patch",
        Did::Replace => "replace",
        Did::Collapse => "collapse",
        Did::Run => "run",
        Did::Merge => "merge",
        Did::Move => "move",
        Did::Remove => "remove",
        Did::Call => "call",
        Did::Edit => "edit",
        Did::Notice => "notice",
    }
}

pub(crate) fn why(w: Why) -> &'static str {
    match w {
        Why::NotThere => "no live block",
        Why::CardOpen => "the card is open",
        Why::NoBlock => "no block yet",
        Why::Full => "32 live blocks",
        Why::Dismissed => "dismissed",
        Why::Group => "a group",
        Why::NotGroup => "not a group",
        Why::Card => "a card can't join a group",
        Why::InGroup => "already in a group",
        Why::Elsewhere => "not in the group's place",
        Why::GroupFull => "the group has 8",
        Why::TooFew => "fewer than 2 members",
        Why::After => "its after didn't resolve",
        Why::Rail => "a card can't go to the rail",
        Why::Revisions => "replace the block or use a new id",
        Why::GroupRevisions => "its group is at its revisions' cap",
        Why::Paths => "every path skipped",
        Why::Rewritten => "the block was rewritten",
        Why::Gone => "the block is gone",
    }
}

fn path_why(w: PathWhy) -> String {
    match w {
        PathWhy::NotKind => "not a path of its kind".into(),
        PathWhy::NotThere => "not there".into(),
        PathWhy::Removed(r) => format!("removed in r{r}"),
        PathWhy::Ids => "64 item ids · replace the block".into(),
        PathWhy::Full => "the list is full".into(),
        PathWhy::TooBig => "too big".into(),
        PathWhy::FromEdits => "give hunks or from, not both".into(),
    }
}

/// How it went, in words.
pub(crate) fn outcome(o: &Outcome) -> String {
    match o {
        Outcome::Applied => "applied".into(),
        Outcome::Here => "placed here".into(),
        Outcome::NoChange => "no change".into(),
        Outcome::Exists => "exists".into(),
        Outcome::Skipped(w) => format!("skipped · {}", why(*w)),
        Outcome::SkippedPath(p, w) => format!("skipped {p} · {}", path_why(*w)),
        Outcome::Unbound => "unbound".into(),
        Outcome::Stale { version, later } => format!("stale v{version}: v{later} is later"),
        Outcome::Twice => "named twice".into(),
    }
}

/// Log line `l` as drawn, its row's time `time`, and whether its block is
/// live on the board (it jumps).
pub(crate) fn line(l: &Logged, time: Option<&str>, live: bool) -> Line {
    Line {
        time: time.unwrap_or("—").to_string(),
        who: crate::revisions::who(l.author),
        did: did(l.did),
        id: l.id.as_ref().map_or_else(|| "—".to_string(), |r| r.to_string()),
        outcome: outcome(&l.outcome),
        pending: l.pending,
        jump: l.id.clone().filter(|_| live),
    }
}

/// A line's text, its columns as the log draws them.
pub(crate) fn text(l: &Line) -> String {
    let pending = if l.pending { " · pending" } else { "" };
    format!("{:<5}  {:<5}  {:<8}  {:<12}  {}{pending}", l.time, l.who, l.did, l.id, l.outcome)
}

/// A line's time, `HH:MM`: its row's, found by its call (`call`, a
/// lookup: [`Transcript::call_at`]); none without one.
pub(crate) fn time_of(t: &Transcript, call: Option<&str>) -> Option<String> {
    #[cfg(test)]
    VISITS.with(|n| n.set(n.get() + 1));
    let at = t.call_at(call?)?;
    crate::times::Stamp::local(at).map(|s| s.hhmm.to_string())
}

#[cfg(test)]
thread_local! {
    /// The rows the op log looked up for its times (tests: bounded by its
    /// lines, whatever the history).
    pub(crate) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The archive (§5.2, module docs): the predecessor's final blocks and op
/// log, as text lines: each block's head and its contents, a group's
/// members after it, indented; the log newest first. Past the cap, the
/// last line of what was cut says so (`cut`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Archive {
    pub blocks: Vec<String>,
    pub log: Vec<String>,
    pub cut: bool,
}

/// Block `s`'s lines in the archive, after `indent`: its id, kind, title
/// and revision, then its contents, canonical JSON, a line each, every
/// line sanitized for the screen (an agent's string drawn as text).
fn block_lines(s: &Shown, indent: &str, out: &mut Vec<String>) {
    let b = &s.block;
    let mut head = format!("{indent}{} · {} · {} · r{}", b.id, b.body.kind().as_str(), crate::blocks::title_of(s), b.rev);
    if b.replaced.is_some() {
        head.push_str(" · replaced");
    }
    out.push(clean(&head));
    let v = synaps_client::blocks::canonical(&serde_json::to_value(&b.body).unwrap_or_default());
    let json = serde_json::to_string_pretty(&v).unwrap_or_default();
    out.extend(json.lines().map(|l| clean(&format!("{indent}  {l}"))));
}

fn clean(s: &str) -> String {
    crate::ask::shown(s, cards_spec::Lines::Single)
}

impl Archive {
    /// Board `d`'s (module docs), its log's times `times` (one a line, in
    /// its order): its live blocks whole (the transcript's in reading
    /// order, then the rail's), its log newest first, within
    /// [`ARCHIVE_MAX`]: past it, the log's oldest lines, then the blocks'
    /// last lines go, and a line says so. Off the frame.
    pub(crate) fn of(d: &Drawn, times: &[Option<String>]) -> Self {
        let log: Vec<String> =
            d.log.iter().enumerate().rev().map(|(k, (l, _))| text(&line(l, times.get(k).and_then(|t| t.as_deref()), false))).collect();
        let mut rows: Vec<(&u64, &Arc<synaps_client::blocks::RowBlocks>)> = d.rows.iter().collect();
        rows.sort_by_key(|(k, _)| **k);
        let mut blocks = Vec::new();
        for s in rows.iter().flat_map(|(_, r)| r.blocks.iter()).chain(d.rail.iter().flatten()) {
            block_lines(s, "", &mut blocks);
            for m in &s.members {
                block_lines(m, "    ", &mut blocks);
            }
        }
        Self::capped(blocks, log)
    }

    /// `blocks` and `log` within [`ARCHIVE_MAX`] (module docs): the log
    /// first (≤ 256 lines), the blocks in the room left, in order; what
    /// doesn't fit goes from there on, and a line says how much.
    pub(crate) fn capped(blocks: Vec<String>, log: Vec<String>) -> Self {
        let line = std::mem::size_of::<String>();
        let cost = |l: &String| line + l.len();
        let room = ARCHIVE_MAX - std::mem::size_of::<Self>() - 2 * (line + CUT_MAX);
        let mut a = Archive::default();
        let mut used = 0;
        let (mut log_cut, mut block_cut) = (0usize, 0usize);
        for l in log {
            if log_cut == 0 && used + cost(&l) <= room {
                used += cost(&l);
                a.log.push(l);
            } else {
                log_cut += 1;
            }
        }
        for l in blocks {
            if block_cut == 0 && used + cost(&l) <= room {
                used += cost(&l);
                a.blocks.push(l);
            } else {
                block_cut += 1;
            }
        }
        let kib = ARCHIVE_MAX / 1024;
        if block_cut > 0 {
            a.blocks.push(format!("✂ cut here: the archive keeps {kib} KiB · {block_cut} more lines of blocks not kept"));
        }
        if log_cut > 0 {
            a.log.push(format!("✂ cut here: the archive keeps {kib} KiB · {log_cut} older lines not kept"));
        }
        a.cut = block_cut + log_cut > 0;
        for l in a.blocks.iter_mut().chain(a.log.iter_mut()) {
            l.shrink_to_fit();
        }
        a.blocks.shrink_to_fit();
        a.log.shrink_to_fit();
        a
    }

    /// What it keeps, heap included (bytes: tests, the cap's measure).
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        let v = |l: &Vec<String>| l.capacity() * std::mem::size_of::<String>() + l.iter().map(String::capacity).sum::<usize>();
        std::mem::size_of::<Self>() + v(&self.blocks) + v(&self.log)
    }

    /// Its lines as the tab lists them: a head, the blocks', a head, the
    /// log's.
    pub(crate) fn rows(&self) -> usize {
        2 + self.blocks.len() + self.log.len()
    }

    /// Its text whole, for the clipboard.
    pub(crate) fn text(&self) -> String {
        let mut s = String::from("before compaction: blocks\n");
        for l in &self.blocks {
            s.push_str(l);
            s.push('\n');
        }
        s.push_str("\nbefore compaction: op log\n");
        for l in &self.log {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

/// One session's archive ([`Archives`]): the conversation its compaction
/// began (shown only while the session holds it), and the archive once
/// captured.
pub(crate) struct Held {
    pub session: String,
    pub journal: String,
    pub archive: Option<Arc<Archive>>,
    capture: u64,
    _task: Option<Task<()>>,
}

/// The archives this desk holds (module docs): one per session, of its
/// last compaction, at most [`ARCHIVES_MAX`] (the least recently captured
/// goes first).
#[derive(Default)]
pub(crate) struct Archives {
    held: Vec<Held>,
    next: u64,
}

impl Archives {
    /// Session `session`'s archive, if it holds one of conversation
    /// `journal` (another conversation's is not shown).
    pub(crate) fn of(&self, session: &str, journal: &str) -> Option<&Held> {
        self.held.iter().find(|h| h.session == session && h.journal == journal)
    }

    /// A compaction of `session` began conversation `journal`: its archive
    /// replaced by the one this capture makes (the one before is gone).
    fn begin(&mut self, session: &str, journal: &str) -> u64 {
        self.next += 1;
        self.held.retain(|h| h.session != session);
        if self.held.len() >= ARCHIVES_MAX {
            self.held.remove(0);
        }
        self.held.push(Held { session: session.into(), journal: journal.into(), archive: None, capture: self.next, _task: None });
        self.next
    }

    fn task(&mut self, session: &str, capture: u64, task: Task<()>) {
        if let Some(h) = self.held.iter_mut().find(|h| h.session == session && h.capture == capture) {
            h._task = Some(task);
        }
    }

    /// Capture `capture` of `session` made `a`: held if it is still the
    /// one asked for.
    fn landed(&mut self, session: &str, capture: u64, a: Archive) -> bool {
        let Some(h) = self.held.iter_mut().find(|h| h.session == session && h.capture == capture) else { return false };
        h.archive = Some(Arc::new(a));
        h._task = None;
        true
    }
}

/// A capture asked for at a compaction (module docs), as the boundary
/// left it: the predecessor's input, its board last bound and those lines'
/// times (its rows are the predecessor's until its history is replaced),
/// its history's epoch.
pub(crate) struct Capture {
    session: String,
    capture: u64,
    input: Predecessor,
    bound: Option<Arc<Drawn>>,
    times: HashMap<Arc<str>, String>,
    epoch: u64,
}

/// A capture's hold of the predecessor's input (Shady #105 R2-2): once
/// the successor's history replaced the transcript's, it can be the last
/// owner of the predecessor's history (its worker's journal and board, its
/// staged chunks), whose drop is the history's size. So it is never
/// dropped on the frame: its handle goes to the app's disposer in O(1)
/// ([`Input::retire`]), whichever way the capture ends: done, superseded
/// by another compaction of its session, evicted, or its task dropped
/// before it ran. A step running off the frame holds its own handle, and
/// lets it go there.
pub(crate) struct Predecessor(Option<Arc<Input>>);

impl Drop for Predecessor {
    fn drop(&mut self) {
        if let Some(i) = self.0.take() {
            Input::retire(i);
        }
    }
}

/// Archive `a`'s row `k` as the tab lists it ([`Archive::rows`]).
fn archive_row(a: &Archive, k: usize, dim: Hsla, ink: Hsla) -> AnyElement {
    let nb = a.blocks.len();
    let (text, ink, sel) = if k == 0 {
        ("blocks".to_string(), dim, "oplog-archive-head blocks".to_string())
    } else if k <= nb {
        let b = &a.blocks[k - 1];
        (b.clone(), ink, format!("oplog-archive-block {} {b}", k - 1))
    } else if k == nb + 1 {
        ("op log".to_string(), dim, "oplog-archive-head log".to_string())
    } else {
        let l = &a.log[k - nb - 2];
        (l.clone(), ink, format!("oplog-archive-line {} {l}", k - nb - 2))
    };
    div()
        .h(px(ROW_H))
        .px(px(16.))
        .flex()
        .items_center()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .font_family(fonts::MONO)
        .text_px(size::META)
        .text_color(ink)
        .debug_selector(move || sel.clone())
        .child(text)
        .into_any_element()
}

impl AppView {
    /// Ctrl+O (Cmd+O): the op log opened over the focused pane, or closed.
    pub(crate) fn oplog_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.oplog.is_some() {
            self.oplog_close(window, cx);
            return;
        }
        if self.palette.is_some() {
            self.close_palette(window, cx);
        }
        let focus = cx.focus_handle();
        let back = window.focused(cx);
        window.focus(&focus, cx);
        self.oplog =
            Some(OpLog { tab: Tab::Ops, focus, back, scroll: ScrollHandle::new(), archive_scroll: UniformListScrollHandle::new() });
        log::info!(target: "synaps_desktop::oplog", "op log: opened");
        cx.notify();
    }

    /// Esc, Ctrl+O, a click outside or a jump: it closes, and the keys go
    /// back where they were (the window's root never: the composer then).
    pub(crate) fn oplog_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = self.oplog.take() else { return };
        match o.back.filter(|b| *b != self.focus) {
            Some(b) => window.focus(&b, cx),
            None => {
                let c = self.panes.cur.composer.clone();
                c.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        cx.notify();
    }

    /// The log's lines (newest first) for the focused pane's session, the
    /// tab flipped by Tab.
    fn oplog_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        // Only a new press (a held Tab flips once).
        if !self.ask_presses.last {
            return;
        }
        match ev.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.oplog_close(window, cx);
            }
            // Shift+Tab too: two tabs, either way is the other.
            "tab" => {
                cx.stop_propagation();
                if let Some(o) = self.oplog.as_mut() {
                    o.tab = if o.tab == Tab::Ops { Tab::Archive } else { Tab::Ops };
                    o.scroll.set_offset(Point::default());
                    o.archive_scroll = UniformListScrollHandle::new();
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// A line's click: its block, walked to in the focused pane.
    fn oplog_jump(&mut self, id: &Ref, window: &mut Window, cx: &mut Context<Self>) {
        self.oplog_close(window, cx);
        let row = crate::blocks::row_id(id);
        let at = if self.panes.cur.disp.iter().any(|d| d.id == row) { crate::walk::At::Block(row) } else { crate::walk::At::Rail(row) };
        if !self.walk_to(at, window, cx) {
            log::info!(target: "synaps_desktop::oplog", "op log: {id} is not in the walk here");
        }
    }

    /// The focused pane's session's log lines, newest first: at most 256,
    /// each one's time looked up by its call (never an index of the
    /// history: module docs).
    pub(crate) fn oplog_lines(&self) -> Vec<Line> {
        let l = &self.model.lives[self.panes.cur.live];
        let Some(d) = l.transcript.drawn() else { return Vec::new() };
        let live: std::collections::HashSet<&Ref> =
            d.rows.values().flat_map(|r| r.blocks.iter()).chain(d.rail.iter().flatten()).map(|s| &s.block.id).collect();
        d.log
            .iter()
            .rev()
            .map(|(lg, m)| line(lg, time_of(&l.transcript, m.call.as_deref()).as_deref(), lg.id.as_ref().is_some_and(|i| live.contains(i))))
            .collect()
    }

    /// The focused pane's session's archive, if it holds one of the
    /// conversation the session holds now.
    pub(crate) fn archive_shown(&self) -> Option<&Held> {
        let l = &self.model.lives[self.panes.cur.live];
        let session = l.active.as_ref().map(|a| a.session_id.as_str())?;
        self.archives.of(session, l.transcript.journal())
    }

    /// A compaction of store `live`'s session began conversation `journal`
    /// (module docs): its archive is replaced by a capture of the
    /// predecessor as this boundary leaves it (its input, its board bound
    /// now and those lines' times: a lookup each), run once the batch is
    /// applied ([`AppView::archive_run`]).
    pub(crate) fn archive_boundary(&mut self, live: crate::live::LiveId, journal: &str) {
        let l = &self.model.lives[live];
        let Some(session) = l.active.as_ref().map(|a| a.session_id.clone()) else { return };
        let t = &l.transcript;
        let input = Predecessor(t.blocks_input().map(Arc::new));
        let bound = l.board.waiting.clone().or_else(|| t.drawn().cloned()).filter(|d| d.epoch == t.blocks_epoch());
        let mut times = HashMap::new();
        for (_, m) in bound.iter().flat_map(|d| d.log.iter()) {
            if let Some(c) = m.call.as_ref()
                && let Some(at) = time_of(t, Some(c))
            {
                times.insert(c.clone(), at);
            }
        }
        let epoch = t.blocks_epoch();
        let capture = self.archives.begin(&session, journal);
        log::info!(target: "synaps_desktop::oplog", "archive: {session} compacted into {journal}: capture {capture}");
        self.captures.push(Capture { session, capture, input, bound, times, epoch });
    }

    /// The captures asked for in the batch, run (module docs): the
    /// predecessor's history folded to its end off the frame, a step at a
    /// time as a board job's; its lines' times; its archive written off
    /// the frame, and held if it is still the one asked for. A live turn
    /// changed under the fold (its log reset) leaves the board bound at
    /// the boundary. The predecessor's input is let go off the frame
    /// ([`Predecessor`]) once folded, or with the task if it never ends.
    pub(crate) fn archive_run(&mut self, cx: &mut Context<Self>) {
        for c in std::mem::take(&mut self.captures) {
            let Capture { session, capture, input, bound, times, epoch } = c;
            let s = session.clone();
            let task = cx.spawn(async move |this, cx| {
                // Only `input` holds it here: each step its own handle,
                // off the frame.
                let drawn = match input.0.as_ref() {
                    None => None,
                    Some(input) => {
                        let stop = Arc::new(AtomicBool::new(false));
                        loop {
                            let (i, s) = (input.clone(), stop.clone());
                            match cx.background_spawn(async move { crate::board::step(&i.worker, &i, &s, crate::board::STEP) }).await {
                                crate::board::Step::Done(d) => break Some(d),
                                crate::board::Step::Paused => {}
                                crate::board::Step::Stopped => break bound,
                            }
                        }
                    }
                };
                drop(input);
                let Ok(times) = this.update(cx, |v, _| v.archive_times(drawn.as_deref(), &times, epoch)) else { return };
                let a = cx.background_spawn(async move { drawn.map_or_else(Archive::default, |d| Archive::of(&d, &times)) }).await;
                let _ = this.update(cx, |v, cx| {
                    if v.archives.landed(&s, capture, a) {
                        log::info!(target: "synaps_desktop::oplog", "archive: {s}'s capture {capture} held");
                        cx.notify();
                    }
                });
            });
            self.archives.task(&session, capture, task);
        }
    }

    /// The times of board `d`'s log lines (a capture's): the boundary's,
    /// else looked up in the predecessor's rows while a store still holds
    /// its history (`epoch`); a lookup a line.
    fn archive_times(&self, d: Option<&Drawn>, snap: &HashMap<Arc<str>, String>, epoch: u64) -> Vec<Option<String>> {
        let Some(d) = d else { return Vec::new() };
        let t = self.model.lives.ids().into_iter().map(|l| &self.model.lives[l].transcript).find(|t| t.blocks_epoch() == epoch);
        d.log
            .iter()
            .map(|(_, m)| {
                let c = m.call.as_deref()?;
                snap.get(c).cloned().or_else(|| t.and_then(|t| time_of(t, Some(c))))
            })
            .collect()
    }

    /// The op log over the pane at `left` (`pane` wide), its foot above
    /// `bottom`.
    pub(crate) fn render_oplog(&self, left: f32, pane: f32, bottom: f32, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let o = self.oplog.as_ref()?;
        let w = WIDTH.min(pane - 48.).max(320.);
        let x = left + ((pane - w) / 2.).max(0.);
        let max = LIST_MAX.min((bottom - TOP - 120.).max(ROW_H * 4.));
        let row = |text: String, ink: Hsla, sel: String| {
            div()
                .h(px(ROW_H))
                .px(px(16.))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .font_family(fonts::MONO)
                .text_px(size::META)
                .text_color(ink)
                .debug_selector(move || sel.clone())
                .child(text)
        };
        let mut list = div().id("oplog-list").overflow_y_scroll().track_scroll(&o.scroll).max_h(px(max)).w_full();
        let held = self.archive_shown();
        let archive = held.and_then(|h| h.archive.clone());
        let mut lines_el = None;
        match o.tab {
            Tab::Ops => {
                let lines = self.oplog_lines();
                if lines.is_empty() {
                    list = list.child(row("No ops yet".into(), p.dim_k, "oplog-empty".into()));
                }
                for (k, l) in lines.into_iter().enumerate() {
                    let t = text(&l);
                    let ink = if l.pending || l.jump.is_none() { p.dim_k } else { p.prose_k };
                    let sel = format!("oplog-line {k} {t}");
                    let el = row(t, ink, sel).id(("oplog-line", k));
                    list = list.child(match l.jump {
                        Some(id) => el
                            .cursor_pointer()
                            .hover(|s| s.bg(p.card_hi))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.oplog_jump(&id, window, cx);
                            }))
                            .into_any_element(),
                        None => el.into_any_element(),
                    });
                }
            }
            Tab::Archive => match (held, archive.clone()) {
                (None, _) => list = list.child(row("Nothing before a compaction yet".into(), p.dim_k, "oplog-archive-empty".into())),
                (Some(_), None) => list = list.child(row("Capturing what was before…".into(), p.dim_k, "oplog-archive-capturing".into())),
                // Drawn a line at a time as it scrolls: ≤ 256 KiB of lines.
                (Some(_), Some(a)) => {
                    let n = a.rows();
                    let (dim, ink) = (p.dim_k, p.prose_k);
                    lines_el = Some(
                        uniform_list("oplog-archive", n, move |range, _, _| {
                            range.map(|k| archive_row(&a, k, dim, ink)).collect::<Vec<_>>()
                        })
                        .track_scroll(&o.archive_scroll)
                        .w_full()
                        .h(px(max.min(n as f32 * ROW_H)))
                        .into_any_element(),
                    );
                }
            },
        }
        let list = lines_el.unwrap_or_else(|| list.into_any_element());
        let seg = |tab: Tab, text: &'static str| {
            let on = o.tab == tab;
            div()
                .id(text)
                .relative()
                .h_full()
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(on, |d| d.bg(p.card_hi).child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(p.you_bar)))
                .child(display(text, size::D_S, if on { p.ink } else { p.dim_k }))
                .debug_selector(move || format!("oplog-tab {text}"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(o) = this.oplog.as_mut() {
                        o.tab = tab;
                        o.scroll.set_offset(Point::default());
                        o.archive_scroll = UniformListScrollHandle::new();
                    }
                    cx.notify();
                }))
        };
        let copy = (o.tab == Tab::Archive).then_some(archive).flatten().map(|a| {
            div()
                .id("oplog-copy")
                .cursor_pointer()
                .debug_selector(|| "oplog-copy".into())
                .child(mono("copy", size::META, p.dim_k))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(a.text()));
                }))
        });
        let head = h_flex()
            .px(px(16.))
            .pt(px(12.))
            .pb(px(8.))
            .gap(px(12.))
            .items_center()
            .child(
                h_flex()
                    .h(px(28.))
                    .border_1()
                    .border_color(p.frame)
                    .child(seg(Tab::Ops, "OPS"))
                    .child(div().w(px(1.)).h_full().bg(p.frame))
                    .child(seg(Tab::Archive, "BEFORE COMPACTION")),
            )
            .child(div().flex_1())
            .children(copy)
            .child(mono("Tab · Esc", size::META, p.dim_k));
        let pane_id = self.panes.cur.id;
        let body = crate::overlay::frame(v_flex(), p)
            .id("oplog")
            .debug_selector(|| "oplog".into())
            .w(px(w))
            .font_family(fonts::SANS)
            .text_px(size::TITLE)
            .occlude()
            .track_focus(&o.focus)
            .key_context(LOG_CTX)
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                this.in_pane(pane_id, |this| this.oplog_key(ev, window, cx));
            }))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.oplog_close(window, cx)))
            .child(head)
            .child(div().h(px(1.)).bg(p.frame))
            .child(list)
            .child(div().h(px(8.)));
        Some(deferred(anchored().position(point(px(x), px(TOP))).child(body)).with_priority(4).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use cards_spec::board::Board;
    use synaps_client::blocks::Drawn;

    use std::sync::Arc;

    use synaps_client::blocks::{Mark, Shown};

    use super::{ARCHIVE_MAX, Archive, Line, line, text};

    /// §3.8: every line names who, the op, the id and how it went; a
    /// skipped op says why (the agent never learns of it); an unbound
    /// call names no id.
    #[test]
    fn the_logs_lines_say_who_what_and_how_it_went() {
        let (b, events) = Board::of_history(&synaps_client::synthetic::blocks_history());
        let v = b.view(&events);
        let lines: Vec<Line> = v.log().map(|l| line(l, None, true)).collect();
        assert!(lines.len() >= 10, "{lines:#?}");
        assert!(lines.iter().any(|l| l.did == "put" && l.id == "plan" && l.outcome == "applied" && l.who == "agent"));
        assert!(lines.iter().any(|l| l.did == "replace" && l.id == "now" && l.outcome == "applied"));
        assert!(lines.iter().any(|l| l.did == "patch" && l.outcome.starts_with("skipped steps.nope")), "the skipped path: {lines:#?}");
        let unbound = lines.iter().find(|l| l.outcome == "unbound").expect("the refused call");
        assert_eq!((unbound.did, unbound.id.as_str(), unbound.jump.as_ref()), ("call", "—", None));
        assert!(lines.iter().all(|l| l.time == "—"), "no time recorded: none, never now");
        assert_eq!(text(&lines[0]).split_whitespace().collect::<Vec<_>>(), ["—", "agent", "put", "now", "applied"]);
    }

    /// The blocks demo's board as a drawn board carries it: its live
    /// top-level blocks (a group with its members), its log.
    fn demo() -> Drawn {
        let (b, events) = Board::of_history(&synaps_client::synthetic::blocks_history());
        let v = b.view(&events);
        let shown = |r: &cards_spec::block::Ref| {
            let blk = v.block(r)?;
            let members = blk
                .members()
                .iter()
                .filter_map(|m| v.block(&cards_spec::block::Ref::Block(m.clone())))
                .map(|m| Shown { block: Arc::new(m.clone()), bound: None, members: Vec::new() })
                .collect();
            Some(Shown { block: Arc::new(blk.clone()), bound: None, members })
        };
        let tops: Vec<Shown> = v.lists().flat_map(|(_, refs)| refs.iter().filter_map(shown).collect::<Vec<_>>()).collect();
        Drawn {
            rail: [tops, Vec::new()],
            log: v.log().map(|l| (l.clone(), Mark::default())).collect::<Vec<_>>().into(),
            ..Default::default()
        }
    }

    /// §5.2: the archive is the predecessor's final blocks whole (every
    /// kind's contents, a group's members after it, multiline text) and its
    /// log newest first; nothing cut, nothing said cut.
    #[test]
    fn the_archive_keeps_every_blocks_contents_whole() {
        let d = demo();
        let a = Archive::of(&d, &[]);
        let all = a.text();
        assert!(!a.cut && !all.contains("✂"), "nothing cut");
        for want in [
            // A plan's steps and their states.
            "Trace issuance and verification paths",
            "Regression test: 32 concurrent refreshes",
            "\"state\": \"done\"",
            // A status's text and detail.
            "Running the regression test",
            "32 concurrent refreshes",
            // A diff's hunks.
            "self.relay.refresh(&self.token).await",
            // A file's path and note.
            "TokenStore::rotate",
            // A metric's value and series.
            "\"value\": \"1.0\"",
            // A note's text past its first line.
            "The retry band-aid hid it",
            // A group's members, after it.
            "why · group",
        ] {
            assert!(all.contains(want), "{want:?} kept: {all}");
        }
        let g = a.blocks.iter().position(|l| l.starts_with("why · group")).expect("the group");
        assert!(a.blocks[g..].iter().any(|l| l.starts_with("    cause · note")), "its members after it, indented");
        assert_eq!(a.log.len(), d.log.len(), "every line");
        assert!(a.log[0].contains("call") && a.log[0].contains("unbound"), "newest first: {:?}", a.log[0]);
        assert!(a.text().starts_with("before compaction: blocks\n") && a.text().contains("before compaction: op log"));
        assert!(a.bytes() <= ARCHIVE_MAX);
    }

    /// §1.8: past 256 KiB the archive is cut, and says so where it is cut;
    /// the log comes first, the blocks' last lines go.
    #[test]
    fn the_archive_is_cut_at_its_cap_and_says_so() {
        let d = demo();
        let one = Archive::of(&d, &[]);
        let many: Vec<Shown> = (0..400).flat_map(|_| d.rail[0].iter().cloned()).collect();
        let big = Drawn { rail: [many, Vec::new()], ..d.clone() };
        let a = Archive::of(&big, &[]);
        assert!(a.bytes() <= ARCHIVE_MAX, "{} bytes", a.bytes());
        assert!(a.cut, "cut");
        let last = a.blocks.last().expect("lines");
        assert!(last.starts_with("✂ cut here: the archive keeps 256 KiB") && last.contains("more lines of blocks not kept"), "{last}");
        assert_eq!(a.log, one.log, "the log whole");
        assert_eq!(a.blocks[..one.blocks.len()], one.blocks[..], "the blocks in order, whole up to the cut");
    }
}
