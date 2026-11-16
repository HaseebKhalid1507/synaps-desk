//! The transcript as turns (`transcript-design/spec.md` sections 4 and 5):
//! the virtual list's rows are display rows, not transcript items. A run of
//! thinking and tool items with no agent text between them is one block on
//! the tool surface; a finished run of 4 or more tool calls folds to one
//! summary row. `YOU` and `AGENT` hang in the margin, once per turn.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::TextViewState;
use gpui_kit::base::text::TextView;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::transcript::{Item, ItemKind, ToolStatus};

use crate::app::{AppView, RowCache, display, mono};
use crate::files::{CallDelta, LineKind, Preview};
use crate::fonts::{self, label};
use crate::glyph::{self, Family, Glyph, Ground};
use crate::subagents::{self, Subagents};
use crate::theme::{self, Metrics, Palette};

/// A run folds once it has finished with this many tool calls.
pub(crate) const FOLD_AT: usize = 4;

/// The markdown rows' text views, kept by the app rather than in the
/// element's keyed state (which lives only while the row is drawn), so a
/// row that comes back into view keeps its parse. The kit parses text over
/// 4 KB in the background, and a view made for it lays out empty
/// until that parse lands. That layout is not the row's height: until the
/// view's own parse has landed ([`TextViewState::rendered_text`]) the row is
/// as tall as its text comes to ([`text_lines`]), and the list is told
/// ([`ListState::set_item_measurement_readiness`]), so it keeps its anchor
/// and follow. The parse's commit draws again. Without this, a drag or a
/// scroll into a long reply measured it a few px tall, and the list,
/// believing it had reached its end, followed the tail again.
///
/// Views are kept by item id and checked against their text: ids restart
/// at 1 in every transcript (a tab, a worker), so a kept view whose text is
/// not this row's (nor a prefix of it, streamed on) is replaced, never
/// reused. Every list swap clears them as well ([`TextRows::clear`]).
pub(crate) struct TextRows<V> {
    kept: HashMap<u64, Kept<V>>,
    /// The kept views' text, in bytes ([`KEEP_BYTES`]).
    bytes: usize,
    /// The rows laid out this frame whose parse is pending (indices): no
    /// allocation once warm.
    waiting: Vec<usize>,
    frame: u64,
    /// The characters a line of the column holds this frame ([`cpl`]).
    cpl: f32,
}

/// The transcript's: a view, and its redraw when it changes (a parse lands).
pub(crate) type Views = TextRows<(Entity<TextViewState>, Subscription)>;

struct Kept<V> {
    text: SharedString,
    view: V,
    /// The frame it was last drawn in (the least recent goes first).
    drawn: u64,
    born: Instant,
    /// Its parse has landed (it stays landed: appended text parses on top).
    parsed: bool,
}

/// Views kept, at most: past it the one drawn longest ago goes (it parses
/// again if it comes back). Above the render window's rows: at the smallest
/// text size a markdown row is at least ~37 px (a line and a part gap), so
/// with 600 px of overdraw each side a 2160 px view draws ~86 of them and a
/// 3384 px one ~122. Nothing drawn this frame or last ever goes, whatever
/// the count, so a lower cap thrashes nothing; it only parses more.
const KEEP: usize = 160;

/// The kept views' text, at most, in bytes: past it the ones drawn longest
/// ago go, as past [`KEEP`]. A kept view holds far more than its text: its
/// document, its blocks' layout and the shaped glyphs of every paragraph it
/// last painted (the kit keeps those until the view drops). Measured on a
/// code-heavy transcript, ~190 bytes of memory a character kept (256 KiB
/// kept: +48 MB over keeping none). 32 KiB is ~6 MB: about ten screens of
/// dense text behind the window (rows go whole), for a scroll back and
/// forth that parses nothing again. The window itself may hold more (a
/// 300 KB reply on screen): what it draws stays, as before views were kept;
/// the budget only drops what it no longer draws.
const KEEP_BYTES: usize = 32 * 1024;

/// A parse that never lands (a failed one) stops holding its row after this.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(2);

/// What a kept view does for its row's text this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    /// The text it has.
    Same,
    /// Its text and more (streamed on): it takes the rest.
    Append,
    /// None is kept, or the kept one is another row's (an id reused by
    /// another transcript): a new view.
    Fresh,
}

fn keep(kept: &SharedString, text: &SharedString) -> Keep {
    if kept.as_ptr() == text.as_ptr() && kept.len() == text.len() {
        Keep::Same
    } else if text.starts_with(kept.as_str()) {
        if text.len() == kept.len() { Keep::Same } else { Keep::Append }
    } else {
        Keep::Fresh
    }
}

/// A row waits (holds its estimated height, its layout not taken for its
/// height) while its view's parse has not landed, unless it has no text or
/// has waited [`MAX_WAIT`].
pub(crate) fn waits(parsed: bool, empty: bool, age: Duration) -> bool {
    !parsed && !empty && age < MAX_WAIT
}

impl<V> Default for TextRows<V> {
    fn default() -> Self {
        TextRows { kept: HashMap::new(), bytes: 0, waiting: Vec::new(), frame: 0, cpl: 16. }
    }
}

impl<V> TextRows<V> {
    /// A frame begins, its column `column` px wide (the list's item width).
    pub(crate) fn frame(&mut self, column: f32, m: &Metrics) {
        self.frame += 1;
        self.waiting.clear();
        self.cpl = cpl(column, m);
    }

    /// Another list is shown (a tab, a worker's view, a new transcript):
    /// nothing of this one carries over.
    pub(crate) fn clear(&mut self) {
        self.kept.clear();
        self.bytes = 0;
        self.waiting.clear();
    }

    /// Row `id` is drawn with `text`: what its kept view does with it.
    fn take(&mut self, id: u64, text: &SharedString) -> Keep {
        let frame = self.frame;
        match self.kept.get_mut(&id) {
            Some(k) => {
                let keep = keep(&k.text, text);
                if keep != Keep::Fresh {
                    self.bytes += text.len() - k.text.len();
                    (k.text, k.drawn) = (text.clone(), frame);
                }
                keep
            }
            None => Keep::Fresh,
        }
    }

    fn view(&self, id: u64) -> Option<&V> {
        self.kept.get(&id).map(|k| &k.view)
    }

    /// Keep `view` for row `id` (its `text`), made `now`. Past [`KEEP`] views
    /// or [`KEEP_BYTES`], the views drawn longest ago go, never one drawn
    /// this frame or the last: rows below this one in the window are not
    /// drawn yet this frame, and one dropped would be made again (and over
    /// 4 KB parsed again, pending) every frame.
    fn put(&mut self, id: u64, text: SharedString, view: V, now: Instant) {
        self.bytes += text.len();
        if let Some(old) = self.kept.insert(id, Kept { text, view, drawn: self.frame, born: now, parsed: false }) {
            self.bytes -= old.text.len();
        }
        while self.kept.len() > KEEP || self.bytes > KEEP_BYTES {
            let frame = self.frame;
            let old = self.kept.iter().filter(|(_, k)| k.drawn + 1 < frame).min_by_key(|(_, k)| k.drawn).map(|(&id, _)| id);
            let Some(k) = old.and_then(|id| self.kept.remove(&id)) else { break };
            self.bytes -= k.text.len();
        }
    }

    /// Whether row `id` (drawn at `ix`) waits for its parse, `parsed` telling
    /// whether it has landed (read only until it has).
    fn wait(&mut self, ix: usize, id: u64, now: Instant, parsed: impl FnOnce(&V) -> bool) -> bool {
        let Some(k) = self.kept.get_mut(&id) else { return false };
        if !k.parsed {
            k.parsed = parsed(&k.view);
        }
        let wait = waits(k.parsed, k.text.is_empty(), now.saturating_duration_since(k.born));
        if wait && !self.waiting.contains(&ix) {
            self.waiting.push(ix);
        }
        wait
    }

    /// A row's layout this frame is its height: it is not markdown, or its
    /// parse has landed.
    pub(crate) fn ready(&self, ix: usize) -> bool {
        !self.waiting.contains(&ix)
    }

    /// A row laid out this frame waits for its parse: the layout is
    /// provisional (GPUI neither fills up to the end nor re-arms following).
    pub(crate) fn pending(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// The kept views' rows: item id and text.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> impl Iterator<Item = (u64, &SharedString)> {
        self.kept.iter().map(|(&id, k)| (id, &k.text))
    }
}

/// A transcript list asks `rows` whether a row's layout is its height.
pub(crate) fn readiness(list: &ListState, rows: &Rc<RefCell<Views>>) {
    let rows = rows.clone();
    list.set_item_measurement_readiness(move |ix, _| rows.borrow().ready(ix));
}

/// The lines `text` comes to, wrapped at `cpl` characters a line.
fn text_lines(text: &str, cpl: f32) -> f32 {
    let cpl = cpl.max(1.);
    text.split('\n').map(|l| (l.len() as f32 / cpl).ceil().max(1.)).sum()
}

/// The mean height display rows come to from their text, wrapped at `cpl`
/// characters a line: a hint for rows not drawn yet.
pub(crate) fn mean_height(disp: &[Disp], rows: &[RowCache], m: &Metrics, cpl: f32) -> f32 {
    if disp.is_empty() {
        return 0.;
    }
    let total: f32 = disp
        .iter()
        .map(|d| {
            let lines = match d.kind {
                Kind::User | Kind::Prose | Kind::Note => rows.get(d.item).map_or(1., |r| text_lines(&r.text, cpl)),
                _ => 1.,
            };
            gap_of(d.gap, m) + lines * m.body_line + 2. * m.pad_y
        })
        .sum();
    total / disp.len() as f32
}

fn gap_of(gap: Gap, m: &Metrics) -> f32 {
    match gap {
        Gap::None => 0.,
        Gap::First => m.first_gap,
        Gap::Part => m.part_gap,
        Gap::Turn => m.turn_gap,
    }
}

/// The space above a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gap {
    /// Inside a run: none, the block continues.
    None,
    /// The transcript's first row.
    First,
    /// A new part of the turn.
    Part,
    /// A new turn.
    Turn,
}

/// The speaker label a row carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Who {
    None,
    You,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    User,
    Prose,
    /// A notice or an error: one mono line.
    Note,
    /// A foldable run's summary: its items are `item..end`.
    Summary {
        end: usize,
        open: bool,
    },
    /// A thinking or tool item in a run.
    Work,
}

/// One row of the virtual list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Disp {
    /// The item it shows (a summary: the run's first).
    pub item: usize,
    pub id: u64,
    /// The item's `rev`; a summary's: a hash of its items' ids and revs.
    pub rev: u64,
    pub kind: Kind,
    pub gap: Gap,
    pub who: Who,
}

impl Disp {
    /// Its item `n` places earlier: the transcript dropped its `n` oldest
    /// (a worker's bound), and this row shows one after them.
    pub(crate) fn shift(&mut self, n: usize) {
        self.item -= n;
        if let Kind::Summary { end, .. } = &mut self.kind {
            *end -= n;
        }
    }
}

/// A call that has reported (or never will): not live.
pub(crate) fn finished(s: Option<ToolStatus>) -> bool {
    matches!(s, Some(ToolStatus::Done | ToolStatus::Failed | ToolStatus::Cancelled | ToolStatus::Denied | ToolStatus::Unknown))
}

fn status(i: &Item) -> Option<ToolStatus> {
    i.tool.as_ref().and_then(|t| t.status)
}

/// Where a rebuild after a change at item `k` starts: the turn holding
/// the item before it (a new item can finish the run before it).
pub(crate) fn turn_start(items: &[Item], k: usize) -> usize {
    let k = k.min(items.len());
    items[..k].iter().rposition(|i| i.kind == ItemKind::User).unwrap_or(0)
}

/// An item's `rev` as its row shows it: a call that started a subagent
/// also changes with its block's shape (the block grows a line, the state
/// changes).
fn rev(x: &Item, subs: &Subagents) -> u64 {
    match x.tool.as_ref() {
        Some(t) if subagents::starts_worker(&t.name) => x.rev ^ subs.shape(&t.tool_id),
        _ => x.rev,
    }
}

/// A call still at work: unfinished, or its subagent still running.
fn live_call(x: &Item, subs: &Subagents) -> bool {
    !finished(status(x)) || x.tool.as_ref().is_some_and(|t| subs.running(&t.tool_id))
}

/// The display rows of `items[from..]` (`from`: a turn's start, or 0).
/// `busy`: a turn is running, so a run at the very end is live; `open`:
/// the runs the user unfolded (by their first item's id); `subs`: a run
/// holding a running subagent's call is live too.
pub(crate) fn build(items: &[Item], from: usize, busy: bool, open: &HashSet<u64>, subs: &Subagents) -> Vec<Disp> {
    let mut out = Vec::new();
    let mut agent = from > 0 && items[from].kind != ItemKind::User;
    let mut i = from;
    let gap = |i: usize| if i == 0 { Gap::First } else { Gap::Part };
    while i < items.len() {
        let it = &items[i];
        let row = |kind, gap, who| Disp { item: i, id: it.id, rev: it.rev, kind, gap, who };
        match it.kind {
            ItemKind::User => {
                out.push(row(Kind::User, if i == 0 { Gap::First } else { Gap::Turn }, Who::You));
                agent = false;
                i += 1;
            }
            ItemKind::Assistant => {
                out.push(row(Kind::Prose, gap(i), if agent { Who::None } else { Who::Agent }));
                agent = true;
                i += 1;
            }
            ItemKind::Thinking | ItemKind::Tool => {
                let start = i;
                let mut end = i;
                let (mut tools, mut done) = (0, true);
                let mut h = std::collections::hash_map::DefaultHasher::new();
                while let Some(x) = items.get(end).filter(|x| matches!(x.kind, ItemKind::Thinking | ItemKind::Tool)) {
                    if x.kind == ItemKind::Tool {
                        tools += 1;
                        done &= !live_call(x, subs);
                    }
                    (x.id, rev(x, subs)).hash(&mut h);
                    end += 1;
                }
                let live = !done || (end == items.len() && busy);
                let who = if agent { Who::None } else { Who::Agent };
                agent = true;
                let first = gap(start);
                if !live && tools >= FOLD_AT {
                    let unfolded = open.contains(&it.id);
                    out.push(Disp {
                        item: start,
                        id: it.id,
                        rev: h.finish(),
                        kind: Kind::Summary { end, open: unfolded },
                        gap: first,
                        who,
                    });
                    if unfolded {
                        out.extend((start..end).map(|k| Disp {
                            item: k,
                            id: items[k].id,
                            rev: rev(&items[k], subs),
                            kind: Kind::Work,
                            gap: Gap::None,
                            who: Who::None,
                        }));
                    }
                } else {
                    out.extend((start..end).map(|k| Disp {
                        item: k,
                        id: items[k].id,
                        rev: rev(&items[k], subs),
                        kind: Kind::Work,
                        gap: if k == start { first } else { Gap::None },
                        who: if k == start { who } else { Who::None },
                    }));
                }
                i = end;
            }
            _ => {
                out.push(row(Kind::Note, gap(i), Who::None));
                i += 1;
            }
        }
    }
    out
}

/// `old` replaced by `new`: the range of `old` that differs and how many
/// rows replace it (the common head and tail keep their measured heights).
pub(crate) fn diff(old: &[Disp], new: &[Disp]) -> Option<(std::ops::Range<usize>, usize)> {
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if head == old.len() && head == new.len() {
        return None;
    }
    let tail = old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    Some((head..old.len() - tail, new.len() - head - tail))
}

/// A tool name's colour role (`draw.rs` `tool_accent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hue {
    Bash,
    Read,
    Write,
    Edit,
    Grep,
    Find,
    Ls,
    Sub,
    Ext,
    Gen,
}

impl Hue {
    pub(crate) fn of(name: &str) -> Self {
        match name {
            "bash" => Hue::Bash,
            "read" => Hue::Read,
            "write" => Hue::Write,
            "edit" => Hue::Edit,
            "grep" => Hue::Grep,
            "find" => Hue::Find,
            "ls" => Hue::Ls,
            // Every call that starts a worker wears the worker's hue
            // (design.md §8.3), not only the blocking `subagent`.
            n if subagents::starts_worker(n) => Hue::Sub,
            n if n.starts_with("ext__") => Hue::Ext,
            _ => Hue::Gen,
        }
    }

    pub(crate) fn color(self, p: &Palette) -> Hsla {
        match self {
            Hue::Bash => p.t_bash,
            Hue::Read => p.t_read,
            Hue::Write => p.t_write,
            Hue::Edit => p.t_edit,
            Hue::Grep => p.t_grep,
            Hue::Find => p.t_find,
            Hue::Ls => p.t_ls,
            Hue::Sub => p.t_sub,
            Hue::Ext => p.t_ext,
            Hue::Gen => p.t_gen,
        }
    }
}

/// Line counts as a row or a summary shows them, formatted once: `+3` `−1`
/// for edits, each only when not 0 (no `−0`), an estimate's `~` on the
/// first shown (`~+3 −1`, `~−6`), `~3`
/// for lines written (not a delta). From the ledger's committed counts
/// only, so a failed, denied, canceled or stopped call has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub added: Option<SharedString>,
    pub removed: Option<SharedString>,
    pub written: Option<SharedString>,
}

impl Counts {
    pub(crate) fn of(d: CallDelta) -> Self {
        Counts {
            added: (d.added > 0).then(|| format!("{}+{}", if d.approx { "~" } else { "" }, d.added).into()),
            removed: (d.removed > 0).then(|| format!("{}−{}", if d.approx && d.added == 0 { "~" } else { "" }, d.removed).into()),
            written: (d.written > 0).then(|| format!("~{}", d.written).into()),
        }
    }

    fn element(&self, p: &Palette) -> Option<Div> {
        (self.added.is_some() || self.removed.is_some() || self.written.is_some()).then(|| {
            h_flex()
                .flex_none()
                .gap(px(8.))
                .children(self.written.clone().map(|t| div().text_color(p.dim_t).child(t)))
                .children(self.added.clone().map(|t| div().text_color(p.ok).child(t)))
                .children(self.removed.clone().map(|t| div().text_color(p.err).child(t)))
        })
    }
}

/// One tool name in a summary's tally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tally {
    pub name: SharedString,
    pub n: usize,
    pub n_text: SharedString,
    pub hue: Hue,
}

/// What a folded run's summary says: `› 7 tools  bash 3  read 3  grep 1 …`
/// and its worst outcome's glyph, every string formatted once per change
/// of the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunSummary {
    pub tools: usize,
    pub label: SharedString,
    /// Most used first (then first used first).
    pub tally: Vec<Tally>,
    /// Calls by how they ended: done, failed, denied, canceled, stopped at
    /// the output limit.
    pub ok: usize,
    pub failed: usize,
    pub denied: usize,
    pub cancelled: usize,
    pub stopped: usize,
    /// The committed counts of its file tools (the ledger's).
    pub delta: CallDelta,
    pub counts: Counts,
    /// Its worst outcome's glyph ([`glyph::worst`]).
    pub worst: (Glyph, Family),
    /// Nothing when every call is done (the glyph alone); else each count
    /// that is not zero, worst first as [`glyph::worst`] ranks them, so the
    /// glyph sits by its own word: `1 failed`, `1 stopped`, `5 ok`.
    pub outcome: Vec<(SharedString, Family)>,
}

impl RunSummary {
    pub(crate) fn of(items: &[Item], rows: &[RowCache]) -> Self {
        let mut tally: Vec<Tally> = Vec::new();
        let (mut tools, mut ok, mut failed, mut denied, mut cancelled, mut stopped) = (0, 0, 0, 0, 0, 0);
        let mut delta = CallDelta::default();
        for (it, row) in items.iter().zip(rows).filter(|(i, _)| i.kind == ItemKind::Tool) {
            tools += 1;
            match status(it) {
                Some(ToolStatus::Done) => ok += 1,
                Some(ToolStatus::Failed) => failed += 1,
                Some(ToolStatus::Denied) => denied += 1,
                Some(ToolStatus::Cancelled) => cancelled += 1,
                Some(ToolStatus::Unknown) => stopped += 1,
                // Never in a folded run (it is not finished).
                _ => {}
            }
            if let Some(d) = row.delta {
                delta.added += d.added;
                delta.removed += d.removed;
                delta.written += d.written;
                delta.approx |= d.approx;
            }
            match tally.iter_mut().find(|t| t.name == row.name) {
                Some(t) => t.n += 1,
                None => tally.push(Tally { name: row.name.clone(), n: 1, n_text: SharedString::default(), hue: row.hue }),
            }
        }
        // Stable: equal counts keep their first-use order.
        tally.sort_by_key(|a| std::cmp::Reverse(a.n));
        for t in &mut tally {
            t.n_text = t.n.to_string().into();
        }
        let parts = [
            (failed, "failed", Family::Err),
            (denied, "denied", Family::Err),
            (stopped, "stopped", Family::Warn),
            (cancelled, "cancelled", Family::Warn),
            (ok, "ok", Family::Ok),
        ];
        let mut outcome: Vec<(SharedString, Family)> = Vec::new();
        if ok < tools {
            for (n, word, family) in parts.into_iter().filter(|p| p.0 > 0) {
                outcome.push((format!("{n} {word}").into(), family));
            }
        }
        RunSummary {
            tools,
            label: format!("{tools} tools").into(),
            tally,
            ok,
            failed,
            denied,
            cancelled,
            stopped,
            delta,
            counts: Counts::of(delta),
            worst: glyph::worst(failed + denied, stopped, cancelled),
            outcome,
        }
    }
}

/// The run summaries, rebuilt only when a run's items change.
#[derive(Default)]
pub(crate) struct Summaries(HashMap<u64, (u64, Arc<RunSummary>)>);

impl Summaries {
    pub(crate) fn get(&self, id: u64) -> Option<&Arc<RunSummary>> {
        self.0.get(&id).map(|e| &e.1)
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// The oldest items went (a worker's bound): their runs' summaries
    /// too. `first`: the oldest item's id kept.
    pub(crate) fn retain_from(&mut self, first: u64) {
        self.0.retain(|id, _| *id >= first);
    }

    /// The summaries of `rows` that changed. A run whose rows `cache` does
    /// not hold yet (the transcript ran ahead of the last sync) is left out:
    /// the next sync builds it.
    pub(crate) fn update(&mut self, rows: &[Disp], items: &[Item], cache: &[RowCache]) {
        for d in rows {
            if let Kind::Summary { end, .. } = d.kind
                && self.0.get(&d.id).is_none_or(|e| e.0 != d.rev)
                && let (Some(items), Some(cache)) = (items.get(d.item..end), cache.get(d.item..end))
            {
                let s = RunSummary::of(items, cache);
                self.0.insert(d.id, (d.rev, Arc::new(s)));
            }
        }
    }
}

/// How the centre pane is laid out this frame (from its size).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    /// The column (`min(42 rem, pane − 2 gutters)`).
    pub column: f32,
    /// Labels hang in the margin (the pane has the column plus 200 rem/16).
    pub hang: bool,
    /// Under 560 px: the gutter drops to 16, status words and hints go.
    pub tight: bool,
    /// Under [`crate::fit::short_below`] tall (a 792-px window at 16 px,
    /// more with larger text): a 40-px top bar, a one-row header, an 8-line
    /// card well, less padding round the composer. Nothing in the list
    /// changes, so it never remeasures for this.
    pub short: bool,
    /// Under [`crate::fit::cramped_below`] (short too): the card folds its
    /// lead into its title row, the composer puts SEND beside the text.
    pub cramped: bool,
    /// The list's left padding: the column centred, whole pixels.
    pub left: f32,
    /// And its right.
    pub right: f32,
}

impl Layout {
    /// `pane` wide; `height` is the pane's height under the full (52-px)
    /// top bar, so the short bar cannot flip it back. Short and cramped
    /// read the height, the text step and `ex` (the model's state) only:
    /// nothing they change feeds back.
    pub(crate) fn of(pane: f32, height: f32, m: &Metrics, ex: crate::fit::Extras) -> Self {
        let tight = pane < 560.;
        let gutter = if tight { 16. } else { 24. };
        let column = m.column.min(pane - 2. * gutter).max(200.).floor();
        let left = ((pane - column) / 2.).floor().max(0.);
        Layout {
            column,
            hang: pane >= (672. + 200.) * m.r,
            tight,
            short: height < crate::fit::short_below(m),
            cramped: height < crate::fit::cramped_below(m, ex),
            left,
            right: (pane - column - left).max(0.),
        }
    }

    pub(crate) fn form(self) -> crate::fit::Form {
        match (self.short, self.cramped) {
            (_, true) => crate::fit::Form::Cramped,
            (true, false) => crate::fit::Form::Short,
            (false, false) => crate::fit::Form::Full,
        }
    }

    /// The list's left and right padding (the header, card and composer
    /// use the same, so their edges line up with the rows').
    pub(crate) fn padding(self) -> (f32, f32) {
        (self.left, self.right)
    }
}

/// About the characters a line of a `column` px column holds (a row's own
/// side padding off): prose wraps at about two an em. An estimate: it counts
/// bytes, so text that is not ASCII (CJK about one an em) comes out longer.
pub(crate) fn cpl(column: f32, m: &Metrics) -> f32 {
    ((column - 2. * m.pad_x) / (0.5 * m.body)).max(16.)
}

impl AppView {
    /// Row `ix`'s markdown (item `id`) in its kept view ([`TextRows`]), and
    /// the height it holds while the view's parse is pending (0 once landed).
    fn markdown(&self, ix: usize, id: u64, text: &SharedString, m: &Metrics, cx: &mut Context<Self>) -> (TextView, Pixels) {
        let now = Instant::now();
        let keep = self.texts.borrow_mut().take(id, text);
        let kept = self.texts.borrow().view(id).map(|(v, _)| v.clone());
        let view = match (keep, kept) {
            (Keep::Same, Some(v)) => v,
            (Keep::Append, Some(v)) => {
                v.update(cx, |s, cx| s.set_text(text, cx));
                v
            }
            _ => {
                let v = cx.new(|cx| TextViewState::markdown(text, cx));
                let redraw = cx.observe(&v, |_, _, cx| cx.notify());
                self.texts.borrow_mut().put(id, text.clone(), (v.clone(), redraw), now);
                v
            }
        };
        let mut rows = self.texts.borrow_mut();
        let wait = rows.wait(ix, id, now, |(v, _)| !v.read(cx).rendered_text().source().is_empty());
        let h = if wait { text_lines(text, rows.cpl) * m.body_line } else { 0. };
        (TextView::new(&view), px(h))
    }

    /// One display row: its gap, its label and the block.
    pub(crate) fn render_disp(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(&d) = self.disp.get(ix) else { return div().into_any_element() };
        let items = self.model.shown().items();
        let (Some(item), Some(row)) = (items.get(d.item), self.rows.get(d.item)) else { return div().into_any_element() };
        let p = theme::palette(cx);
        let m = theme::metrics(cx);
        let lay = self.layout;
        let gap = gap_of(d.gap, &m);
        // Its markdown in its kept view, as tall as its text while the
        // view's parse is pending ([`TextRows`]).
        let (md, min_h) = match d.kind {
            Kind::User | Kind::Prose if d.kind == Kind::User || !self.plain_rows => {
                let (v, h) = self.markdown(ix, d.id, &row.text, &m, cx);
                (Some(v), h)
            }
            _ => (None, px(0.)),
        };
        let block: AnyElement = match d.kind {
            Kind::User => div()
                .w_full()
                .bg(p.user)
                .border_l_2()
                .border_color(p.you_mark)
                .px(px(m.pad_x))
                .py(px(m.pad_y))
                .text_size(px(m.body))
                .line_height(px(m.body_line))
                .text_color(p.ink)
                .min_h(min_h)
                .children(md.map(|v| v.style(cx.global::<theme::Scheme>().user_text.clone())))
                .into_any_element(),
            Kind::Prose => {
                let body = div().w_full().min_h(min_h).text_size(px(m.body)).line_height(px(m.body_line)).text_color(p.prose);
                match md {
                    Some(v) => body.child(v).into_any_element(),
                    None => body.child(row.text.clone()).into_any_element(),
                }
            }
            Kind::Note => {
                let color = if item.kind == ItemKind::Error { p.err } else { p.dim };
                mono(row.text.clone(), m.meta, color).w_full().whitespace_normal().into_any_element()
            }
            Kind::Summary { open, .. } => self.render_summary(d, open, &p, &m, cx),
            Kind::Work => self.render_work(item, row, d.id, &p, &m, lay, cx),
        };
        let who = match d.who {
            Who::None => None,
            Who::You => Some((label::YOU, p.you, m.pad_y)),
            Who::Agent => {
                let row = match d.kind {
                    Kind::Prose => m.body_line,
                    Kind::Summary { .. } => m.summary,
                    _ => m.row,
                };
                Some((label::AGENT, p.agent, ((row - m.body_line) / 2.).max(0.)))
            }
        };
        let mut out = v_flex().w_full().pt(px(gap)).relative();
        if let Some((text, color, top)) = who {
            let l = display(text, m.label, color).line_height(px(m.body_line));
            if lay.hang {
                out = out.child(l.absolute().top(px(gap + top)).right(px(lay.column + m.hang)));
            } else {
                out = out.child(l.mb(px((2. * m.r).round())));
            }
        }
        out.child(block).into_any_element()
    }

    /// A run's summary row (design.md §8.2): the fold, `7 tools`, the tally
    /// (names in their hues), then the trail: counts · its worst outcome's
    /// glyph by the counts by outcome, worst first (`+3 −4 · ✕ 1 failed ·
    /// 7 ok`; tight, the glyph alone: `+3 −4 · ✕`), so no glyph sits
    /// between two numbers. On its lead edge the tally stacked into one
    /// bar: the run's rows' bars, folded.
    fn render_summary(&self, d: Disp, open: bool, p: &Palette, m: &Metrics, cx: &Context<Self>) -> AnyElement {
        let id = d.id;
        let Some(s) = self.summaries.get(id) else { return div().into_any_element() };
        let mut tally = h_flex().flex_1().min_w_0().gap(px((14. * m.r).round())).overflow_hidden().whitespace_nowrap();
        for t in &s.tally {
            tally = tally.child(
                h_flex()
                    .flex_none()
                    .gap(px(m.mono * 0.6))
                    .child(div().text_color(t.hue.color(p)).child(t.name.clone()))
                    .child(div().text_color(p.dim_t).child(t.n_text.clone())),
            );
        }
        let (worst, family) = s.worst;
        let dot = || div().text_color(p.dim_t).child("·");
        let mut trail = h_flex()
            .flex_none()
            .gap(px(8.))
            .items_center()
            .text_size(px(m.meta))
            .when_some(s.counts.element(p), |d, c| d.child(c).child(dot()))
            .child(glyph::draw(worst, glyph::ink(family, Ground::Run, p).1));
        if !self.layout.tight {
            for (i, (text, family)) in s.outcome.iter().enumerate() {
                if i > 0 {
                    trail = trail.child(dot());
                }
                trail = trail.child(div().text_color(glyph::ink(*family, Ground::Run, p).0).child(text.clone()));
            }
        }
        // Open, the bar ends over the 1 px cut under it.
        let h = m.summary as u32 - u32::from(open);
        let segments = glyph::tally_bar(&s.tally, |t| t.n, h)
            .map(|(i, top, h)| div().absolute().left_0().top(px(top as f32)).w(px(2.)).h(px(h as f32)).bg(s.tally[i].hue.color(p)));
        h_flex()
            .id(("run", id))
            .relative()
            .w_full()
            .h(px(m.summary))
            .px(px(m.pad_x))
            .items_center()
            .bg(p.tool)
            .when(open, |d| d.border_b_1().border_color(p.canvas))
            .font_family(fonts::MONO)
            .text_size(px(m.mono))
            .text_color(p.prose_t)
            .cursor_pointer()
            .hover(|d| d.bg(p.tool_out))
            .child(glyph::lead(Some(glyph::draw(if open { Glyph::FoldOpen } else { Glyph::Fold }, p.dim_t)), m))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(m.run_gap))
                    .items_center()
                    .child(div().flex_none().child(s.label.clone()))
                    .child(tally)
                    .child(trail),
            )
            .children(segments)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_run(id);
                cx.notify();
            }))
            .into_any_element()
    }

    /// A thinking or tool row in a run (design.md §8.1): the fold (blank
    /// without a body), name, argument, outcome; its body when open; the
    /// worker it started. On its lead edge a bar in its tool's hue, through
    /// its body (`warn_mark` while a prompt waits on it).
    #[allow(clippy::too_many_arguments)]
    fn render_work(&self, item: &Item, row: &RowCache, id: u64, p: &Palette, m: &Metrics, lay: Layout, cx: &Context<Self>) -> AnyElement {
        let think = item.kind == ItemKind::Thinking;
        let st = status(item);
        let has_body = if think { !row.text.is_empty() } else { row.preview.is_some() || !row.result.is_empty() };
        let open = has_body && self.is_open(item, row);
        let (name, name_color): (SharedString, Hsla) = if think { ("think".into(), p.think) } else { (row.name.clone(), row.hue.color(p)) };
        let arg = if think {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(fonts::SANS)
                .text_size(px(m.think))
                .text_color(p.think)
                .child(row.arg.clone())
        } else {
            div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(p.arg).child(row.arg.clone())
        };
        // The call a prompt waits on (the last one unfinished).
        let waiting = !think && self.waiting_on == Some(item.id) && !self.model.shown().pending_prompts().is_empty();
        let state = glyph::call(st, waiting, self.model.active.is_none());
        let bar = if think {
            p.think
        } else if state.0 == Glyph::Needs {
            p.warn_mark
        } else {
            name_color
        };
        let fold = has_body.then(|| glyph::draw(if open { Glyph::FoldOpen } else { Glyph::Fold }, p.dim_t));
        let head = h_flex()
            .id(("row", id))
            .w_full()
            .min_h(px(m.row))
            .px(px(m.pad_x))
            .items_center()
            .when(has_body, |d| d.cursor_pointer().hover(|d| d.bg(p.tool_out)))
            .child(glyph::lead(fold, m))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(m.run_gap))
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(m.mono * 0.6 * 5.))
                            .max_w(px(m.mono * 0.6 * 16.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(name_color)
                            .child(name),
                    )
                    .child(arg)
                    .when(!think, |d| d.child(outcome(row, state, p, m, lay))),
            )
            .when(has_body, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle(id);
                    cx.notify();
                }))
            });
        let body = open.then(|| {
            if think {
                div()
                    .w_full()
                    .bg(p.tool_out)
                    .pl(px(m.pad_x + m.lead))
                    .pr(px(m.pad_x))
                    .pt(px((8. * m.r).round()))
                    .pb(px((12. * m.r).round()))
                    .font_family(fonts::SANS)
                    .text_size(px(m.think))
                    .line_height(px(m.think_line))
                    .text_color(p.think_o)
                    .child(row.text.clone())
                    .into_any_element()
            } else {
                match &row.preview {
                    Some(pv) => diff_body(pv, p, m).into_any_element(),
                    None => result_body(&row.result, row.result_more, matches!(st, Some(ToolStatus::Failed | ToolStatus::Denied)), p, m)
                        .into_any_element(),
                }
            }
        });
        let worker = item.tool.as_ref().filter(|t| subagents::starts_worker(&t.name)).and_then(|t| {
            let subs = self.model.shown_subagents();
            subs.view(&t.tool_id).map(|v| (v, subs.current(&t.tool_id)))
        });
        v_flex()
            .relative()
            .w_full()
            .bg(p.tool)
            .font_family(fonts::MONO)
            .text_size(px(m.mono))
            .line_height(px(m.mono_line))
            .child(head)
            .children(worker.map(|(v, current)| self.subagent_block(v, current, p, m, lay, cx)))
            .children(body)
            .child(glyph::bar(bar))
            .into_any_element()
    }

    /// The worker a call started, under it (design.md §8.3; the tray row's
    /// grammar): state glyph, name, its step while it runs, its facts
    /// (model · tools · elapsed), its state word and `Go`; once it ended,
    /// its result or why it failed under the row. Its `sub_rule` bar under
    /// the call's name. Nothing moves: it changes when an event or a row
    /// comes. Not attached (a cached tab opening, a detached session): as
    /// last seen. On the session's thread a background worker's block opens
    /// it in a tab (history: [`crate::wtabs`]), `Go` drawn on hover.
    /// `current`: no later call has its `sa_N` (a cold-started daemon
    /// numbers from 1 again); one that has is another worker's handle now,
    /// and the block does nothing.
    fn subagent_block(&self, v: &subagents::View, current: bool, p: &Palette, m: &Metrics, lay: Layout, cx: &Context<Self>) -> AnyElement {
        let live = subagents::Liveness::of(v.status, self.model.active.is_some());
        let (g, _, word, family) = glyph::worker(live);
        let (word_color, mark) = glyph::ink(family, Ground::Run, p);
        let ended = v.status.is_some_and(subagents::Status::terminal);
        let (step, result) = if ended { (None, v.line.clone()) } else { (v.line.clone(), None) };
        let step_color =
            if matches!(live, subagents::Liveness::Running | subagents::Liveness::Cancelling) { p.sub_status_t } else { p.dim_t };
        // A background worker of the session on screen: it opens.
        let open = v.id.filter(|_| v.watchable && current && self.model.watch.focus().is_none());
        let who = open.map(|id| crate::wtabs::Who::new(id, v.name.clone(), v.call.clone()));
        let go = who.clone().map(|who| {
            h_flex()
                .id(("wblock-open", who.id as usize))
                .group("wblock-go")
                .flex_none()
                .items_center()
                .child(glyph::reveal(glyph::GO, "wblock", "wblock-go", false, p.dim_t, p.ink))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if ev.click_count() > 1 {
                        return;
                    }
                    this.swallow_clicks = Some(std::time::Instant::now() + crate::wtabs::MULTI_CLICK);
                    this.open_worker_tab(&who, cx);
                }))
        });
        let trail = h_flex()
            .flex_none()
            .gap(px(8.))
            .items_center()
            .text_size(px(m.meta))
            .when(!v.facts.is_empty(), |d| d.child(div().text_color(p.sub_time_t).child(v.facts.clone())))
            .when(!lay.tight, |d| d.child(div().text_color(word_color).child(word)))
            .children(go);
        let head = h_flex()
            .w_full()
            .min_h(px(m.row))
            .px(px(m.pad_x))
            .items_center()
            .whitespace_nowrap()
            .child(glyph::lead(Some(glyph::draw(g, mark)), m))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(m.run_gap))
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(m.mono * 0.6 * 24.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.sub_name_t)
                            .child(v.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when_some(step, |d, (text, _)| d.text_color(step_color).child(text)),
                    )
                    .child(trail),
            );
        let result = result.map(|(text, err)| {
            div()
                .w_full()
                .pl(px(m.pad_x + m.lead))
                .pr(px(m.pad_x))
                .pb(px((8. * m.r).round()))
                .text_size(px(m.meta))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(if err { p.err } else { p.dim_t })
                .child(text)
        });
        let block = div()
            .w_full()
            .pl(px(m.pad_x + m.lead))
            .child(v_flex().relative().w_full().child(head).children(result).child(glyph::bar(p.sub_rule)));
        let Some(who) = who else {
            // Its handle names a later worker now: nothing of that one's here.
            if let Some(id) = v.id.filter(|_| v.watchable && !current && self.model.watch.focus().is_none()) {
                let tip = SharedString::new_static("an earlier worker: a restarted daemon has given its handle to another one since");
                return block
                    .id(("wblock-old", id as usize))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .into_any_element();
            }
            return block.into_any_element();
        };
        let id = who.id;
        let who2 = who.clone();
        let tip = SharedString::new_static("click: open it in a tab · right-click: more");
        block
            .id(("wblock", id as usize))
            .group("wblock")
            .cursor_pointer()
            .hover(|d| d.bg(p.tool_out))
            .when(self.menu.is_none(), |d| d.tooltip(crate::wtabs::card_tip(cx.weak_entity(), who.clone(), tip)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who, true, ev, window, cx)))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who2, true, ev, window, cx)))
            .into_any_element()
    }
}

/// A call's trail (design.md §8.1): its counts, its outcome's glyph and
/// its word (none for ok, none in a tight pane; the glyph never drops).
/// Nothing moves.
fn outcome(row: &RowCache, (g, word, family): (Glyph, Option<&'static str>, Family), p: &Palette, m: &Metrics, lay: Layout) -> AnyElement {
    let (text, mark) = glyph::ink(family, Ground::Run, p);
    h_flex()
        .flex_none()
        .gap(px(8.))
        .items_center()
        .text_size(px(m.meta))
        .whitespace_nowrap()
        // The ledger's committed counts: nothing unless the call succeeded.
        .children(row.counts.element(p))
        .child(glyph::draw(g, mark))
        .when_some(word.filter(|_| !lay.tight), |d, w| d.child(div().text_color(text).child(w)))
        .into_any_element()
}

/// An edit's diff on the result surface: its signs at the row's name (36),
/// its text one inset in (52); added and removed lines washed 1.10:1 off it
/// toward `tool_result_ok` / `error_color`.
fn diff_body(pv: &Preview, p: &Palette, m: &Metrics) -> impl IntoElement {
    let indent = m.pad_x + m.lead;
    let mut col = v_flex().w_full().bg(p.tool_out).pt(px((8. * m.r).round())).pb(px((12. * m.r).round()));
    for l in &pv.lines {
        let (sign, fg, sign_fg, bg) = match l.kind {
            LineKind::Add => ("+", p.ink, p.ok_o, Some(p.add_bg)),
            LineKind::Del => ("−", p.ink, p.err_o, Some(p.del_bg)),
            LineKind::Context => (" ", p.dim_o, p.dim_o, None),
            LineKind::Note => (" ", p.dim_o, p.dim_o, None),
        };
        col = col.child(
            h_flex()
                .w_full()
                .pl(px(indent))
                .pr(px(m.pad_x))
                .items_start()
                .when_some(bg, |d, bg| d.bg(bg))
                .child(div().w(px(m.pad_x)).flex_none().text_color(sign_fg).child(sign))
                // `SharedString` from the line's `Arc<str>`: no copy per frame.
                .child(div().flex_1().min_w_0().text_color(fg).whitespace_normal().child(SharedString::from(&l.text))),
        );
    }
    if pv.more > 0 {
        col = col.child(div().pl(px(indent)).text_color(p.dim_o).child(format!("… {} more lines", pv.more)));
    }
    col
}

fn result_body(result: &SharedString, more: usize, failed: bool, p: &Palette, m: &Metrics) -> impl IntoElement {
    v_flex()
        .w_full()
        .bg(p.tool_out)
        .pl(px(m.pad_x + m.lead))
        .pr(px(m.pad_x))
        .pt(px((8. * m.r).round()))
        .pb(px((12. * m.r).round()))
        .text_color(if failed { p.err_o } else { p.res })
        .whitespace_normal()
        .child(result.clone())
        .when(more > 0, |d| d.child(div().text_color(p.dim_o).child(format!("… {more} more lines"))))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    use gpui_kit::SharedString;
    use serde_json::json;
    use synaps_client::transcript::ItemKind;

    use super::{
        Counts, Disp, Gap, Hue, KEEP, KEEP_BYTES, Keep, Kind, Layout, MAX_WAIT, Metrics, RowCache, RunSummary, Summaries, TextRows, Who,
        build, diff, text_lines, turn_start,
    };
    use crate::files::{CallDelta, Ledger};
    use crate::subagents::Subagents;
    use synaps_client::Transcript;

    fn transcript(msgs: &[serde_json::Value]) -> Transcript {
        let mut t = Transcript::new();
        t.apply_history(msgs);
        t
    }

    fn tool(id: &str, name: &str) -> (serde_json::Value, serde_json::Value) {
        (
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": {"command": "x"}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]}),
        )
    }

    fn turn(n_tools: usize, prefix: &str) -> Vec<serde_json::Value> {
        let mut m = vec![json!({"role": "user", "content": format!("go {prefix}")})];
        for k in 0..n_tools {
            let (a, b) = tool(&format!("{prefix}{k}"), if k % 2 == 0 { "bash" } else { "read" });
            m.push(a);
            m.push(b);
        }
        m.push(json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}));
        m
    }

    fn kinds(rows: &[Disp]) -> Vec<&'static str> {
        rows.iter()
            .map(|d| match d.kind {
                Kind::User => "user",
                Kind::Prose => "prose",
                Kind::Note => "note",
                Kind::Summary { open: false, .. } => "fold",
                Kind::Summary { open: true, .. } => "unfold",
                Kind::Work => "work",
            })
            .collect()
    }

    #[test]
    fn a_finished_run_of_four_folds_three_does_not() {
        let mut m = turn(4, "a");
        m.extend(turn(3, "b"));
        let t = transcript(&m);
        let rows = build(t.items(), 0, false, &HashSet::new(), &Subagents::default());
        assert_eq!(kinds(&rows), ["user", "fold", "prose", "user", "work", "work", "work", "prose"]);
        assert_eq!(rows.iter().map(|d| d.gap).collect::<Vec<_>>()[..5], [Gap::First, Gap::Part, Gap::Part, Gap::Turn, Gap::Part]);
        assert_eq!(rows[5].gap, Gap::None, "a run is one block");
        // AGENT once per turn, at its first agent part.
        assert_eq!(
            rows.iter().map(|d| d.who).collect::<Vec<_>>(),
            [Who::You, Who::Agent, Who::None, Who::You, Who::Agent, Who::None, Who::None, Who::None]
        );
        // Unfolded by the user: the summary stays, the rows follow it.
        let open: HashSet<u64> = [rows[1].id].into();
        let rows = build(t.items(), 0, false, &open, &Subagents::default());
        assert_eq!(kinds(&rows)[..7], ["user", "unfold", "work", "work", "work", "work", "prose"]);
    }

    /// A run holding the call of a subagent still running never folds (its
    /// call is answered at once, the worker runs on); it folds once the
    /// worker ends, even after the turn. The call's row changes with its
    /// block's shape, and only that row.
    #[test]
    fn a_run_with_a_running_subagent_never_folds() {
        let mut m = turn(4, "a");
        let n = m.len();
        m[3] = json!({"role": "assistant", "content": [{"type": "tool_use", "id": "a1", "name": "subagent_start", "input": {"agent": "scout", "task": "look"}}]});
        m[4] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1", "content": json!({"handle_id": "sa_3", "agent_name": "scout", "status": "running"}).to_string()}]});
        assert_eq!(m.len(), n);
        let t = transcript(&m);
        let mut subs = Subagents::default();
        subs.reset(&m, false);
        let rows = |subs: &Subagents| build(t.items(), 0, false, &HashSet::new(), subs);
        assert_eq!(kinds(&rows(&subs))[1], "fold", "started, in the history only: nothing says it runs");
        let row = |st: serde_json::Value| {
            let v = json!({"ev": "subagent_rows", "rows": [{"subagent_id": 3, "agent_name": "scout", "status": st, "elapsed_secs": 5.0, "step": "ls", "tools": 1}]});
            synaps_client::protocol::decode_event(&v).unwrap()
        };
        subs.on_session(&row(json!("running")));
        let live = rows(&subs);
        assert_eq!(kinds(&live), ["user", "work", "work", "work", "work", "prose"], "running: never folded");
        let still = live[2];
        subs.on_session(&row(json!("running")));
        assert_eq!(rows(&subs)[2], still, "a row with only a new elapsed: same rev");
        subs.on_session(&row(json!("completed")));
        assert_eq!(kinds(&rows(&subs))[1], "fold", "it ended: the run folds");
    }

    #[test]
    fn the_live_run_never_folds() {
        let mut m = turn(5, "a");
        m.pop();
        let t = transcript(&m);
        assert_eq!(kinds(&build(t.items(), 0, true, &HashSet::new(), &Subagents::default()))[1], "work", "busy: the last run is live");
        assert_eq!(kinds(&build(t.items(), 0, false, &HashSet::new(), &Subagents::default()))[1], "fold", "the turn ended");
    }

    #[test]
    fn a_rebuild_from_the_turn_matches_a_full_one() {
        let mut m = turn(4, "a");
        m.extend(turn(2, "b"));
        let t = transcript(&m);
        let all = build(t.items(), 0, false, &HashSet::new(), &Subagents::default());
        let k = t.items().iter().rposition(|i| i.kind == ItemKind::User).unwrap();
        assert_eq!(turn_start(t.items(), k + 1), k);
        assert_eq!(turn_start(t.items(), k), 0, "a new turn can finish the run before it");
        let from = all.partition_point(|d| d.item < k);
        assert_eq!(&all[from..], &build(t.items(), k, false, &HashSet::new(), &Subagents::default())[..]);
    }

    /// The transcript dropped its oldest rows (a worker's bound): the rows
    /// after the cut, shifted, are the new build's but for the run the cut
    /// went through, so the list keeps every other row, its measured
    /// height and the scroll on it; nothing is replaced wholesale.
    #[test]
    fn a_front_trim_keeps_the_rows_after_the_cut() {
        let ev = |v: serde_json::Value| serde_json::from_value::<synaps_client::protocol::WatchEvent>(v).unwrap();
        let mut t = crate::model::transcript();
        t.set_busy(true);
        for k in 0..6 {
            t.apply_watch(&ev(json!({"kind": "steered", "text": format!("step {k}")})));
            for j in 0..4 {
                let id = format!("t{k}{j}");
                t.apply_watch(&ev(json!({"kind": "tool_use", "tool_id": id, "tool_name": "bash", "input": {"command": "ls"}})));
                t.apply_watch(&ev(json!({"kind": "tool_result", "tool_id": id, "result": "ok"})));
            }
            t.apply_watch(&ev(json!({"kind": "text", "text": format!("done {k}")})));
        }
        t.set_busy(false);
        let open = HashSet::new();
        let old = build(t.items(), 0, false, &open, &Subagents::default());
        let mut sums = Summaries::default();
        sums.update(&old, t.items(), &rows(&t, &Ledger::default()));
        let runs: Vec<u64> = old.iter().filter(|d| matches!(d.kind, Kind::Summary { .. })).map(|d| d.id).collect();
        assert_eq!(t.trim_live_front(t.len() - 8, usize::MAX), 8, "the first turn and two rows of the second");
        // The runs cut go with their summaries (N-L4).
        let first = t.items()[0].id;
        sums.retain_from(first);
        assert!(runs.iter().any(|id| *id < first) && runs.iter().any(|id| *id >= first), "{runs:?} at {first}");
        assert!(runs.iter().all(|id| sums.get(*id).is_some() == (*id >= first)));
        let k = old.partition_point(|d| d.item < 8);
        let mut kept = old[k..].to_vec();
        kept.iter_mut().for_each(|d| d.shift(8));
        let new = build(t.items(), 0, false, &open, &Subagents::default());
        let (range, count) = diff(&kept, &new).expect("the cut run changed");
        // The cut run's three calls left come in; nothing kept is spliced.
        assert_eq!((range, count), (0..0, 3), "{kept:?} -> {new:?}");
        assert_eq!(kept[..], new[3..]);
    }

    #[test]
    fn diffs_keep_the_common_head_and_tail() {
        let mut m = turn(4, "a");
        m.extend(turn(2, "b"));
        let t = transcript(&m);
        let a = build(t.items(), 0, false, &HashSet::new(), &Subagents::default());
        assert_eq!(diff(&a, &a), None);
        let b = build(t.items(), 0, false, &[a[1].id].into(), &Subagents::default());
        assert_eq!(diff(&a, &b), Some((1..2, 5)), "unfolding: the summary (its sign) and the run's rows");
        assert_eq!(diff(&b, &a), Some((1..6, 1)));
        assert_eq!(diff(&a[..3], &a), Some((3..3, a.len() - 3)));
    }

    fn rows(t: &Transcript, ledger: &Ledger) -> Vec<RowCache> {
        t.items().iter().map(|i| crate::app::row_cache(i, ledger, &Subagents::default())).collect()
    }

    use crate::glyph::{Family, Glyph};

    fn outcome(s: &RunSummary) -> Vec<(&str, Family)> {
        s.outcome.iter().map(|(t, tone)| (t.as_ref(), *tone)).collect()
    }

    #[test]
    fn summaries_count_tools_failures_and_names() {
        let mut m = turn(5, "a");
        m[4] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1", "content": "boom", "is_error": true}]});
        let t = transcript(&m);
        let rows = rows(&t, &Ledger::default());
        let s = RunSummary::of(&t.items()[1..6], &rows[1..6]);
        assert_eq!((s.tools, s.ok, s.failed), (5, 4, 1));
        assert_eq!(
            s.tally.iter().map(|t| (t.name.as_ref(), t.n, t.n_text.as_ref())).collect::<Vec<_>>(),
            [("bash", 3, "3"), ("read", 2, "2")]
        );
        assert_eq!(s.tally[0].hue, Hue::Bash);
        // A call that starts a worker wears the worker's hue.
        assert_eq!([Hue::of("subagent_start"), Hue::of("subagent_resume"), Hue::of("subagent")], [Hue::Sub; 3]);
        assert_eq!(Hue::of("subagent_collect"), Hue::Gen);
        assert_eq!(s.label.as_ref(), "5 tools");
        assert_eq!(outcome(&s), [("1 failed", Family::Err), ("4 ok", Family::Ok)], "worst first: by its glyph");
        assert_eq!(s.worst, (Glyph::Failed, Family::Err));
    }

    /// The `Ok` glyph alone only when every call finished normally; else
    /// its worst outcome's glyph and the counts. HALT mid-run cancels the
    /// rest, and a denied call says denied.
    #[test]
    fn a_summary_ticks_only_when_every_call_is_done() {
        let t = transcript(&turn(5, "a"));
        let s = RunSummary::of(&t.items()[1..6], &rows(&t, &Ledger::default())[1..6]);
        assert_eq!((outcome(&s), s.worst), (vec![], (Glyph::Ok, Family::Ok)));

        // HALT after two calls: the other three end canceled.
        let mut m = turn(5, "a");
        m.truncate(1 + 2 * 2);
        for k in 2..5 {
            let (call, _) = tool(&format!("a{k}"), "bash");
            m.push(call);
            m.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": format!("a{k}"), "content": "Canceled by user"}]}));
        }
        let t = transcript(&m);
        let rows = build(t.items(), 0, false, &HashSet::new(), &Subagents::default());
        assert_eq!(kinds(&rows), ["user", "fold"], "a halted run folds");
        let Kind::Summary { end, .. } = rows[1].kind else { unreachable!() };
        let s = RunSummary::of(&t.items()[1..end], &self::rows(&t, &Ledger::default())[1..end]);
        assert_eq!((s.ok, s.cancelled), (2, 3));
        assert_eq!(outcome(&s), [("3 cancelled", Family::Warn), ("2 ok", Family::Ok)]);
        assert_eq!(s.worst, (Glyph::Cancelled, Family::Warn));

        // A denied call, and one stopped at the output limit.
        let mut m = turn(4, "a");
        m[2] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a0",
            "content": "Tool call blocked by extension: Tool call confirmation denied: no"}]});
        m[4] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1",
            "content": "line\n[output truncated at 30000 chars]"}]});
        let t = transcript(&m);
        let s = RunSummary::of(&t.items()[1..5], &self::rows(&t, &Ledger::default())[1..5]);
        assert_eq!((s.ok, s.failed, s.denied, s.stopped), (2, 0, 1, 1));
        assert_eq!(outcome(&s), [("1 denied", Family::Err), ("1 stopped", Family::Warn), ("2 ok", Family::Ok)]);
        assert_eq!(s.worst, (Glyph::Failed, Family::Err), "denied is the worst: failed > limit > cancelled");
    }

    /// The transcript's counts are the FILES panel's: in the live shot a
    /// write showed `+3 −0` and the rows summed to +6 −1 while the panel
    /// said +3 −1 and `~3`. Now a write is `~3`, a failed edit counts
    /// nothing, and the summary equals the ledger.
    #[test]
    fn counts_come_from_the_ledger() {
        let call = |id: &str, name: &str, input: serde_json::Value, result: serde_json::Value| {
            [
                json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}),
                json!({"role": "user", "content": [result]}),
            ]
        };
        let ok = |id: &str| json!({"type": "tool_result", "tool_use_id": id, "content": "ok"});
        let mut m = vec![json!({"role": "user", "content": "go"})];
        m.extend(call("r", "read", json!({"path": "/w/a.rs"}), ok("r")));
        m.extend(call("e", "edit", json!({"path": "/w/a.rs", "old_string": "x", "new_string": "a\nb\nc"}), ok("e")));
        m.extend(call("w", "write", json!({"path": "/w/summary.md", "content": "1\n2\n3"}), ok("w")));
        m.extend(call(
            "f",
            "edit",
            json!({"path": "/w/b.rs", "old_string": "p\nq", "new_string": "r\ns"}),
            json!({"type": "tool_result", "tool_use_id": "f", "content": "old_string not found", "is_error": true}),
        ));
        m.extend(call("b", "bash", json!({"command": "ls"}), ok("b")));
        m.push(json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}));
        let t = transcript(&m);
        let mut ledger = Ledger::default();
        ledger.reset(&m, Some("/w"), false);
        let rows = rows(&t, &ledger);
        let by = |id: &str| &rows[t.items().iter().position(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == id)).unwrap()];
        assert_eq!(by("e").counts, Counts { added: Some("+3".into()), removed: Some("−1".into()), written: None });
        assert_eq!(by("w").counts, Counts { added: None, removed: None, written: Some("~3".into()) });
        assert_eq!(by("f").counts, Counts::default(), "a failed edit counts nothing");
        assert_eq!(by("f").preview.as_ref().map(|p| (p.added, p.removed)), Some((2, 2)), "its diff still shows");
        let s = RunSummary::of(&t.items()[1..6], &rows[1..6]);
        assert_eq!(s.delta, CallDelta { added: 3, removed: 1, approx: false, written: 3 });
        assert_eq!((s.delta.added, s.delta.removed), ledger.totals(), "the summary is the panel's");
        assert_eq!(ledger.files().iter().map(|f| f.written).sum::<u32>(), s.delta.written);
        assert_eq!(s.counts, Counts { added: Some("+3".into()), removed: Some("−1".into()), written: Some("~3".into()) });
        assert_eq!(outcome(&s), [("1 failed", Family::Err), ("4 ok", Family::Ok)]);
        // An estimate keeps its `~`, on the first count shown.
        assert_eq!(
            Counts::of(CallDelta { added: 2, removed: 5, approx: true, written: 0 }),
            Counts { added: Some("~+2".into()), removed: Some("−5".into()), written: None }
        );
        assert_eq!(
            Counts::of(CallDelta { added: 0, removed: 6, approx: true, written: 0 }),
            Counts { added: None, removed: Some("~−6".into()), written: None },
            "a lower bound with nothing added is still an estimate"
        );
        // No `−0` (audit E3) and no `+0`: a count shows only when it is not 0.
        assert_eq!(
            Counts::of(CallDelta { added: 4, removed: 0, approx: false, written: 0 }),
            Counts { added: Some("+4".into()), removed: None, written: None }
        );
        assert_eq!(
            Counts::of(CallDelta { added: 0, removed: 6, approx: false, written: 0 }),
            Counts { added: None, removed: Some("−6".into()), written: None }
        );
    }

    /// M1: events land between frames, so a click can rebuild against rows
    /// cached for a shorter transcript. The summary it can't build yet is
    /// skipped (the next sync builds it), never a panic.
    #[test]
    fn a_summary_ahead_of_the_cached_rows_is_skipped() {
        let first = turn(2, "a");
        let mut all = first.clone();
        all.extend(turn(5, "b"));
        let stale = rows(&transcript(&first), &Ledger::default());
        let t = transcript(&all);
        let new = build(t.items(), 0, false, &HashSet::new(), &Subagents::default());
        let Some(run) = new.iter().find(|d| matches!(d.kind, Kind::Summary { end, .. } if end > stale.len())) else {
            panic!("the new turn's run folds past the cached rows")
        };
        let mut sums = Summaries::default();
        sums.update(&new, t.items(), &stale);
        assert!(sums.get(run.id).is_none());
        sums.update(&new, t.items(), &rows(&t, &Ledger::default()));
        assert_eq!(sums.get(run.id).map(|s| s.tools), Some(5));
    }

    #[test]
    fn the_column_is_capped_and_labels_hang_when_there_is_room() {
        let m = Metrics::at(16);
        let wide = Layout::of(1988., 900., &m, Default::default());
        assert_eq!((wide.column, wide.hang, wide.tight), (672., true, false));
        assert_eq!(Layout::of(942., 900., &m, Default::default()).column, 672.);
        assert!(Layout::of(942., 900., &m, Default::default()).hang);
        assert_eq!(
            (Layout::of(678., 900., &m, Default::default()).column, Layout::of(678., 900., &m, Default::default()).hang),
            (630., false)
        );
        assert_eq!((Layout::of(1988., 900., &Metrics::at(18), Default::default()).column), 756.);
        let tight = Layout::of(500., 900., &m, Default::default());
        assert_eq!((tight.column, tight.tight, tight.short), (468., true, false));
        // Dragging a side panel while the column is capped changes only the
        // padding: the list's item width (what its cache is keyed on) holds.
        for pane in [900., 1100., 1318., 1988.] {
            let l = Layout::of(pane, 900., &m, Default::default());
            let (a, b) = l.padding();
            assert_eq!(pane - a - b, 672., "{pane}");
            assert!((a - b).abs() <= 1.);
        }
    }

    /// A row's layout is its height once its view's own parse has landed,
    /// however many frames that takes (a drag or a stream draws many
    /// meanwhile); landed, it stays landed. A parse that never lands stops
    /// holding the row after [`MAX_WAIT`]; a row without text never waits.
    #[test]
    fn a_rows_layout_waits_for_its_parse_to_land() {
        let m = Metrics::at(16);
        let t0 = Instant::now();
        let mut rows = TextRows::<u32>::default();
        let text = SharedString::from("x".repeat(9000));
        rows.frame(672., &m);
        assert_eq!(rows.take(42, &text), Keep::Fresh);
        rows.put(42, text.clone(), 1, t0);
        assert!(rows.wait(3, 42, t0, |_| false), "made this frame, parse pending");
        assert!(!rows.ready(3) && rows.ready(4) && rows.pending());
        for f in 1..6 {
            // Frames the drag draws while the parse is still running.
            rows.frame(672., &m);
            assert_eq!(rows.take(42, &text), Keep::Same);
            assert!(rows.wait(3, 42, t0 + Duration::from_millis(f), |_| false), "frame {f}");
            assert!(!rows.ready(3));
        }
        rows.frame(672., &m);
        assert!(!rows.wait(5, 42, t0, |_| true), "landed (a splice moved it: the same row)");
        assert!(rows.ready(5) && !rows.pending());
        rows.frame(672., &m);
        assert!(!rows.wait(5, 42, t0, |_| panic!("read once landed")));
        // A parse that never lands, and an empty row.
        rows.put(7, "y".repeat(9000).into(), 2, t0);
        assert!(rows.wait(1, 7, t0 + MAX_WAIT - Duration::from_millis(1), |_| false));
        rows.frame(672., &m);
        assert!(!rows.wait(1, 7, t0 + MAX_WAIT, |_| false));
        rows.put(8, SharedString::default(), 3, t0);
        assert!(!rows.wait(2, 8, t0, |_| false));
    }

    /// Item ids restart at 1 in every transcript: a tab's row 7 and a
    /// worker's row 7 are different rows. A kept view is reused only for
    /// its own text (or its text streamed on), and a list swap clears them.
    #[test]
    fn rows_of_two_lists_with_the_same_ids_never_share_a_view() {
        let m = Metrics::at(16);
        let t0 = Instant::now();
        let mut rows = TextRows::<&str>::default();
        let (a, b) = (SharedString::from("the tab's reply"), SharedString::from("the worker's reply"));
        rows.frame(672., &m);
        assert_eq!(rows.take(7, &a), Keep::Fresh);
        rows.put(7, a.clone(), "tab", t0);
        assert!(!rows.wait(0, 7, t0, |_| true));
        // The worker's list, same id, without a clear: not the tab's view.
        rows.frame(672., &m);
        assert_eq!(rows.take(7, &b), Keep::Fresh);
        rows.put(7, b.clone(), "worker", t0);
        assert_eq!(rows.view(7), Some(&"worker"));
        assert!(rows.wait(0, 7, t0, |_| false), "its own parse, not the tab's");
        // Its own text again (another allocation), and streamed on.
        rows.frame(672., &m);
        assert_eq!(rows.take(7, &SharedString::from(b.to_string())), Keep::Same);
        assert_eq!(rows.take(7, &SharedString::from(format!("{b}, and more"))), Keep::Append);
        // A swap: nothing carries over, nothing pending.
        rows.wait(0, 7, t0, |_| false);
        rows.clear();
        assert!(!rows.pending() && rows.view(7).is_none());
        assert_eq!(rows.take(7, &b), Keep::Fresh);
    }

    /// Past [`KEEP`] views, the one drawn longest ago goes, never one drawn
    /// this frame.
    #[test]
    fn the_view_drawn_longest_ago_goes_first() {
        let m = Metrics::at(16);
        let t0 = Instant::now();
        let mut rows = TextRows::<u64>::default();
        for id in 0..KEEP as u64 {
            rows.frame(672., &m);
            rows.put(id, SharedString::from(id.to_string()), id, t0);
        }
        rows.frame(672., &m);
        assert_eq!(rows.take(0, &SharedString::from("0")), Keep::Same, "0 drawn again: 1 is the oldest now");
        rows.put(1000, "new".into(), 1000, t0);
        assert!(rows.view(0).is_some() && rows.view(1).is_none() && rows.view(1000).is_some());
        assert_eq!(rows.kept.len(), KEEP);
    }

    fn kept_bytes<V>(rows: &TextRows<V>) -> usize {
        rows.kept.values().map(|k| k.text.len()).sum()
    }

    /// Past [`KEEP_BYTES`] of text, the views drawn longest ago go, however
    /// few they are; the count follows streamed text and replaced views.
    #[test]
    fn the_byte_budget_drops_the_views_drawn_longest_ago() {
        let m = Metrics::at(16);
        let t0 = Instant::now();
        let quarter = |c: char| SharedString::from(c.to_string().repeat(KEEP_BYTES / 4));
        let mut rows = TextRows::<char>::default();
        // Scrolled through six long replies, one a frame.
        for (id, c) in (0..).zip("abcdef".chars()) {
            rows.frame(672., &m);
            assert_eq!(rows.take(id, &quarter(c)), Keep::Fresh);
            rows.put(id, quarter(c), c, t0);
            assert_eq!(rows.bytes, kept_bytes(&rows));
        }
        assert!(rows.view(0).is_none() && rows.view(1).is_none(), "a and b went");
        assert!((2..6).all(|id| rows.view(id).is_some()));
        assert_eq!(rows.bytes, KEEP_BYTES);
        // Streamed on, replaced by another row's text, cleared.
        rows.frame(672., &m);
        assert_eq!(rows.take(5, &SharedString::from(format!("{}, and more", quarter('f')))), Keep::Append);
        assert_eq!(rows.bytes, kept_bytes(&rows));
        assert_eq!(rows.take(4, &"another".into()), Keep::Fresh);
        rows.put(4, "another".into(), 'g', t0);
        assert_eq!(rows.bytes, kept_bytes(&rows));
        rows.clear();
        assert_eq!(rows.bytes, 0);
    }

    /// A window that draws more text than [`KEEP_BYTES`] keeps every view it
    /// draws. A row made this frame above the others (drawn before them)
    /// spares the ones drawn last frame and not yet this one: with only this
    /// frame's spared, they would go and be made again (a parse each) every
    /// frame. Rows off the window go.
    #[test]
    fn a_window_over_the_byte_budget_never_drops_its_own_views() {
        let m = Metrics::at(16);
        let t0 = Instant::now();
        let half = |c: char| SharedString::from(c.to_string().repeat(KEEP_BYTES / 2));
        let mut rows = TextRows::<char>::default();
        rows.frame(672., &m);
        for (id, c) in (1..).zip("xyz".chars()) {
            assert_eq!(rows.take(id, &half(c)), Keep::Fresh);
            rows.put(id, half(c), c, t0);
        }
        assert_eq!(rows.kept.len(), 3, "1.5 budgets, all on screen");
        // Each frame a new row above them, made before they are drawn.
        for new in 10..14 {
            rows.frame(672., &m);
            let text = SharedString::from(new.to_string());
            assert_eq!(rows.take(new, &text), Keep::Fresh);
            rows.put(new, text, 'n', t0);
            for (id, c) in (1..).zip("xyz".chars()) {
                assert_eq!(rows.take(id, &half(c)), Keep::Same, "{c} kept, frame after frame");
            }
        }
        assert!(rows.view(10).is_none() && rows.view(11).is_none(), "off the window, over the budget: gone");
        // Scrolled on: x leaves the window, and goes after a frame's grace.
        rows.frame(672., &m);
        for (id, c) in (2..).zip("yz".chars()) {
            assert_eq!(rows.take(id, &half(c)), Keep::Same);
        }
        rows.frame(672., &m);
        rows.put(20, "v".into(), 'v', t0);
        assert!(rows.view(1).is_none(), "x went");
        assert!(rows.view(2).is_some() && rows.view(3).is_some(), "drawn last frame: kept");
        assert_eq!(rows.bytes, kept_bytes(&rows));
    }

    #[test]
    fn text_lines_wrap_each_line_and_count_blank_ones() {
        assert_eq!(text_lines("", 80.), 1.);
        assert_eq!(text_lines(&"x".repeat(80), 80.), 1.);
        assert_eq!(text_lines(&"x".repeat(81), 80.), 2.);
        assert_eq!(text_lines("a\n\nb", 80.), 3.);
    }
}
