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

/// Views kept: a few screens of rows at the smallest text size. Past it the
/// one drawn longest ago goes (it parses again if it comes back).
const KEEP: usize = 160;

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
        TextRows { kept: HashMap::new(), waiting: Vec::new(), frame: 0, cpl: 16. }
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
        self.waiting.clear();
    }

    /// Row `id` is drawn with `text`: what its kept view does with it.
    fn take(&mut self, id: u64, text: &SharedString) -> Keep {
        let frame = self.frame;
        match self.kept.get_mut(&id) {
            Some(k) => {
                let keep = keep(&k.text, text);
                if keep != Keep::Fresh {
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

    /// Keep `view` for row `id` (its `text`), made `now`; past [`KEEP`], the
    /// view drawn longest ago (not this frame) goes.
    fn put(&mut self, id: u64, text: SharedString, view: V, now: Instant) {
        if self.kept.len() >= KEEP && !self.kept.contains_key(&id) {
            let old = self.kept.iter().filter(|(_, k)| k.drawn < self.frame).min_by_key(|(_, k)| k.drawn).map(|(&id, _)| id);
            if let Some(old) = old {
                self.kept.remove(&old);
            }
        }
        self.kept.insert(id, Kept { text, view, drawn: self.frame, born: now, parsed: false });
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
            "subagent" => Hue::Sub,
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
/// (`~+3` for an estimate) for edits, `~3` for lines written (not a
/// delta), as the FILES panel shows them. From the ledger's committed
/// counts only, so a failed, denied, canceled or stopped call has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub added: Option<SharedString>,
    pub removed: Option<SharedString>,
    pub written: Option<SharedString>,
}

impl Counts {
    pub(crate) fn of(d: CallDelta) -> Self {
        let edits = d.added + d.removed > 0;
        Counts {
            added: edits.then(|| format!("{}+{}", if d.approx { "~" } else { "" }, d.added).into()),
            removed: edits.then(|| format!("−{}", d.removed).into()),
            written: (d.written > 0).then(|| format!("~{}", d.written).into()),
        }
    }

    fn element(&self, p: &Palette) -> Option<Div> {
        (self.added.is_some() || self.written.is_some()).then(|| {
            h_flex()
                .flex_none()
                .gap(px(6.))
                .children(self.written.clone().map(|t| div().text_color(p.dim_t).child(t)))
                .children(self.added.clone().map(|t| div().text_color(p.ok).child(t)))
                .children(self.removed.clone().map(|t| div().text_color(p.err).child(t)))
        })
    }
}

/// How a summary's outcome part is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Ok,
    Err,
    Warn,
}

/// One tool name in a summary's tally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tally {
    pub name: SharedString,
    pub n: usize,
    pub n_text: SharedString,
    pub hue: Hue,
}

/// What a folded run's summary says: `+ 7 tools  bash 3  read 3  grep 1 …
/// ✓`, every string formatted once per change of the run.
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
    /// `✓` when every call is done; else each count that is not zero
    /// (`5 ok`, `1 cancelled`), and the same without `ok` when tight.
    pub outcome: Vec<(SharedString, Tone)>,
    pub outcome_tight: Vec<(SharedString, Tone)>,
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
            (failed, "failed", "×", Tone::Err),
            (denied, "denied", "×", Tone::Err),
            (cancelled, "cancelled", "", Tone::Warn),
            (stopped, "stopped", "", Tone::Warn),
        ];
        let (outcome, outcome_tight): (Vec<_>, Vec<_>) = if ok == tools {
            (vec![("✓".into(), Tone::Ok)], vec![("✓".into(), Tone::Ok)])
        } else {
            let mut wide: Vec<(SharedString, Tone)> = Vec::new();
            let mut tight: Vec<(SharedString, Tone)> = Vec::new();
            if ok > 0 {
                wide.push((format!("{ok} ok").into(), Tone::Ok));
            }
            for (n, word, glyph, tone) in parts.into_iter().filter(|p| p.0 > 0) {
                wide.push((format!("{n} {word}").into(), tone));
                tight.push((if glyph.is_empty() { format!("{n} {word}") } else { format!("{glyph} {n}") }.into(), tone));
            }
            (wide, tight)
        };
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
            outcome,
            outcome_tight,
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
                    .child(t.n_text.clone()),
            );
        }
        let mut outcome = h_flex().flex_none().gap(px(8.)).text_size(px(m.meta)).children(s.counts.element(p));
        for (i, (text, tone)) in if self.layout.tight { &s.outcome_tight } else { &s.outcome }.iter().enumerate() {
            if i > 0 {
                outcome = outcome.child(div().text_color(p.dim_t).child("·"));
            }
            let color = match tone {
                Tone::Ok => p.ok,
                Tone::Err => p.err,
                Tone::Warn => p.warn,
            };
            outcome = outcome.child(div().text_color(color).child(text.clone()));
        }
        h_flex()
            .id(("run", id))
            .w_full()
            .h(px(m.summary))
            .px(px(m.run_x))
            .gap(px(m.run_gap))
            .items_center()
            .bg(p.tool)
            .when(open, |d| d.border_b_1().border_color(p.canvas))
            .font_family(fonts::MONO)
            .text_size(px(m.mono))
            .text_color(p.prose_t)
            .cursor_pointer()
            .hover(|d| d.bg(p.tool_out))
            .child(div().w(px(m.mono * 0.6)).flex_none().text_color(p.dim_t).child(if open { "−" } else { "+" }))
            .child(div().flex_none().child(s.label.clone()))
            .child(tally)
            .child(outcome)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_run(id);
                cx.notify();
            }))
            .into_any_element()
    }

    /// A thinking or tool row in a run: sign, name, argument, outcome; and
    /// its body when open.
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
        let head = h_flex()
            .id(("row", id))
            .w_full()
            .min_h(px(m.row))
            .px(px(m.run_x))
            .gap(px(m.run_gap))
            .items_center()
            .when(has_body, |d| d.cursor_pointer().hover(|d| d.bg(p.tool_out)))
            .child(div().w(px(m.mono * 0.6)).flex_none().text_color(p.dim_t).child(if open { "−" } else { "+" }))
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
            .when(!think, |d| d.child(self.outcome(item, row, st, p, m, lay)))
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
                    .pl(px(m.run_x + m.mono * 0.6 + m.run_gap))
                    .pr(px(m.run_x))
                    .pt(px((8. * m.r).round()))
                    .pb(px((10. * m.r).round()))
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
            .w_full()
            .bg(p.tool)
            .font_family(fonts::MONO)
            .text_size(px(m.mono))
            .line_height(px(m.mono_line))
            .child(head)
            .children(worker.map(|(v, current)| self.subagent_block(v, current, p, m, lay, cx)))
            .children(body)
            .into_any_element()
    }

    /// The worker a call started, under it (the TUI's subagent card and
    /// tray row): state, name, model · tools · elapsed; then its step while
    /// it runs, its result or why it failed once it ends. Nothing moves:
    /// it changes when an event or a row comes. Not attached (a cached tab
    /// opening, a detached session): as last seen, dimmed. On the
    /// session's thread a background worker's block opens it in a tab
    /// (history: [`crate::wtabs`]), with OPEN ↗ on hover. `current`: no
    /// later call has its `sa_N` (a cold-started daemon numbers from 1
    /// again); one that has is another worker's handle now, and the block
    /// does nothing.
    fn subagent_block(&self, v: &subagents::View, current: bool, p: &Palette, m: &Metrics, lay: Layout, cx: &Context<Self>) -> AnyElement {
        use subagents::Tone as T;
        let seen = self.model.active.is_none();
        let running = v.status.is_some_and(subagents::Status::running);
        let (word, color) = match v.tone {
            _ if seen => (if running { "last seen running" } else { v.word }, p.dim_t),
            T::Run => (v.word, p.run),
            T::Ok => (v.word, p.ok),
            T::Err => (v.word, p.err),
            T::Warn => (v.word, p.warn),
            T::Dim => (v.word, p.dim_t),
        };
        let glyph = match v.tone {
            T::Run if !seen => Some(div().size(px(8.)).m(px(1.)).flex_none().bg(p.run_mark)),
            T::Run | T::Dim => Some(div().size(px(8.)).m(px(1.)).flex_none().border_1().border_color(p.dim_t)),
            _ => None,
        };
        let sign = match v.tone {
            T::Ok => Some("✓"),
            T::Err => Some("×"),
            _ => None,
        };
        let word_too = !lay.tight || (sign.is_none() && glyph.is_none());
        let (name, facts, line) = if seen { (p.dim_t, p.dim_t, p.dim_t) } else { (p.sub_name_t, p.sub_time_t, p.sub_status_t) };
        let head = h_flex()
            .w_full()
            .gap(px(m.run_gap))
            .items_center()
            .whitespace_nowrap()
            .children(glyph)
            .when_some(sign, |d, s| d.child(div().flex_none().text_color(color).child(s)))
            .when(word_too, |d| d.child(div().flex_none().text_color(color).child(word)))
            .child(div().flex_none().max_w(px(m.mono * 0.6 * 24.)).overflow_hidden().text_ellipsis().text_color(name).child(v.name.clone()))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(facts).child(v.facts.clone()));
        // A background worker of the session on screen: it opens.
        let open = v.id.filter(|_| v.watchable && current && self.model.watch.focus().is_none());
        let who = open.map(|id| crate::wtabs::Who::new(id, v.name.clone(), v.call.clone()));
        let head = head.when_some(who.clone(), |d, who| {
            let id = who.id;
            d.child(
                div()
                    .id(("wblock-open", id as usize))
                    .flex_none()
                    .text_size(px(m.meta))
                    .text_color(gpui_kit::transparent_black())
                    .group_hover("wblock", |s| s.text_color(p.dim_t))
                    .hover(|s| s.text_color(p.prose_t))
                    .child("OPEN ↗")
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if ev.click_count() > 1 {
                            return;
                        }
                        this.swallow_clicks = Some(std::time::Instant::now() + crate::wtabs::MULTI_CLICK);
                        this.open_worker_tab(&who, cx);
                    })),
            )
        });
        let rule = if seen { p.dim_t } else { p.sub_rule };
        let block = div().w_full().pl(px(m.run_x + m.mono * 0.6 + m.run_gap)).pr(px(m.run_x)).pb(px((6. * m.r).round())).child(
            v_flex().w_full().border_l_2().border_color(rule).pl(px((10. * m.r).round())).child(head).when_some(
                v.line.clone(),
                |d, (text, err)| {
                    let c = if seen {
                        line
                    } else if err {
                        p.err
                    } else if running {
                        line
                    } else {
                        p.dim_t
                    };
                    d.child(div().w_full().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(c).child(text))
                },
            ),
        );
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

    /// A tool's outcome: `✓`, `× failed`, `× denied`, `cancelled`, a solid
    /// square and `running`, a framed square and `waiting` (the call a
    /// prompt waits on: the last one unfinished). Nothing moves.
    fn outcome(&self, item: &Item, row: &RowCache, st: Option<ToolStatus>, p: &Palette, m: &Metrics, lay: Layout) -> AnyElement {
        let word = |w: &'static str| (!lay.tight).then_some(w);
        // The ledger's committed counts: nothing unless the call succeeded.
        let delta = row.counts.element(p);
        let (glyph, text, color): (Option<Div>, Option<&'static str>, Hsla) = match st {
            Some(ToolStatus::Done) => (None, Some("✓"), p.ok),
            Some(ToolStatus::Failed) => (None, Some(if lay.tight { "×" } else { "× failed" }), p.err),
            Some(ToolStatus::Denied) => (None, Some(if lay.tight { "×" } else { "× denied" }), p.err),
            // These have no glyph, so a tight pane keeps a short word.
            Some(ToolStatus::Cancelled) => (None, Some("cancelled"), p.dim_t),
            // The daemon stopped it at its output limit: not ✓, not ×.
            Some(ToolStatus::Unknown) => (None, Some(if lay.tight { "stopped" } else { "stopped at limit" }), p.dim_t),
            Some(ToolStatus::Queued) => (None, Some("queued"), p.dim_t),
            _ if self.waiting_on == Some(item.id) && !self.model.shown().pending_prompts().is_empty() => {
                let g = div()
                    .relative()
                    .size(px(10.))
                    .flex_none()
                    .child(div().absolute().inset_0().border_1().border_color(p.warn_mark))
                    .child(div().absolute().top(px(3.)).left(px(3.)).size(px(4.)).bg(p.warn_mark));
                (Some(g), word("waiting"), p.warn)
            }
            // Not attached (a cached tab opening, a detached session): what
            // was last seen, not a live run.
            _ if self.model.active.is_none() => {
                (Some(div().size(px(8.)).m(px(1.)).flex_none().border_1().border_color(p.dim_t)), word("last seen running"), p.dim_t)
            }
            _ => (Some(div().size(px(8.)).m(px(1.)).flex_none().bg(p.run_mark)), word("running"), p.run),
        };
        h_flex()
            .flex_none()
            .gap(px(8.))
            .items_center()
            .text_size(px(m.meta))
            .whitespace_nowrap()
            .children(delta)
            .children(glyph)
            .when_some(text, |d, t| d.child(div().text_color(color).child(t)))
            .into_any_element()
    }
}

/// An edit's diff on the result surface: added and removed lines washed
/// toward `tool_result_ok` / `error_color` (16 % / 18 %).
fn diff_body(pv: &Preview, p: &Palette, m: &Metrics) -> impl IntoElement {
    let indent = m.run_x + m.mono * 0.6 + m.run_gap;
    let mut col = v_flex().w_full().bg(p.tool_out).py(px((8. * m.r).round()));
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
                .pr(px(m.run_x))
                .items_start()
                .when_some(bg, |d, bg| d.bg(bg))
                .child(div().w(px(m.mono * 1.2)).flex_none().text_color(sign_fg).child(sign))
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
    let indent = m.run_x + m.mono * 0.6 + m.run_gap;
    v_flex()
        .w_full()
        .bg(p.tool_out)
        .pl(px(indent))
        .pr(px(m.run_x))
        .py(px((8. * m.r).round()))
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
        Counts, Disp, Gap, Hue, KEEP, Keep, Kind, Layout, MAX_WAIT, Metrics, RowCache, RunSummary, Summaries, TextRows, Tone, Who, build,
        diff, text_lines, turn_start,
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

    fn outcome(s: &RunSummary) -> Vec<(&str, Tone)> {
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
        assert_eq!(s.label.as_ref(), "5 tools");
        assert_eq!(outcome(&s), [("4 ok", Tone::Ok), ("1 failed", Tone::Err)]);
    }

    /// `✓` only when every call finished normally; HALT mid-run cancels the
    /// rest, and a denied call says denied.
    #[test]
    fn a_summary_ticks_only_when_every_call_is_done() {
        let t = transcript(&turn(5, "a"));
        let s = RunSummary::of(&t.items()[1..6], &rows(&t, &Ledger::default())[1..6]);
        assert_eq!(outcome(&s), [("✓", Tone::Ok)]);

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
        assert_eq!(outcome(&s), [("2 ok", Tone::Ok), ("3 cancelled", Tone::Warn)]);
        assert_eq!(s.outcome_tight.iter().map(|o| o.0.as_ref()).collect::<Vec<_>>(), ["3 cancelled"]);

        // A denied call, and one stopped at the output limit.
        let mut m = turn(4, "a");
        m[2] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a0",
            "content": "Tool call blocked by extension: Tool call confirmation denied: no"}]});
        m[4] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1",
            "content": "line\n[output truncated at 30000 chars]"}]});
        let t = transcript(&m);
        let s = RunSummary::of(&t.items()[1..5], &self::rows(&t, &Ledger::default())[1..5]);
        assert_eq!((s.ok, s.failed, s.denied, s.stopped), (2, 0, 1, 1));
        assert_eq!(outcome(&s), [("2 ok", Tone::Ok), ("1 denied", Tone::Err), ("1 stopped", Tone::Warn)]);
        assert_eq!(s.outcome_tight.iter().map(|o| o.0.as_ref()).collect::<Vec<_>>(), ["× 1", "1 stopped"]);
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
        assert_eq!(outcome(&s), [("4 ok", Tone::Ok), ("1 failed", Tone::Err)]);
        // An estimate keeps its `~`.
        assert_eq!(Counts::of(CallDelta { added: 2, removed: 5, approx: true, written: 0 }).added.as_deref(), Some("~+2"));
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

    #[test]
    fn text_lines_wrap_each_line_and_count_blank_ones() {
        assert_eq!(text_lines("", 80.), 1.);
        assert_eq!(text_lines(&"x".repeat(80), 80.), 1.);
        assert_eq!(text_lines(&"x".repeat(81), 80.), 2.);
        assert_eq!(text_lines("a\n\nb", 80.), 3.);
    }
}
