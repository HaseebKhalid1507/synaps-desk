//! The transcript as turns (`transcript-design/spec.md` sections 4 and 5):
//! the virtual list's rows are display rows, not transcript items. A run of
//! thinking and tool items with no agent text between them is one block on
//! the tool surface; a finished run of 4 or more tool calls folds to one
//! summary row. `YOU` and `AGENT` hang in the margin, once per turn.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use gpui_kit::base::TextViewState;
use gpui_kit::base::text::TextView;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::transcript::{Item, ItemKind, ToolStatus};

use crate::app::{AppView, RowCache, display, mono};
use crate::files::{CallDelta, LineKind, Preview};
use crate::fonts::TextPx as _;
use crate::fonts::{self, label};
use crate::glyph::{self, Family, Glyph, Ground};
use crate::motion::{self, Kind as Mk, Motion, Tag, ease};
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

    /// The frames begun so far ([`TextRows::frame`]).
    #[cfg(test)]
    pub(crate) fn frames(&self) -> u64 {
        self.frame
    }
}

impl Views {
    /// The kept views: item id and the view's entity, sorted by id.
    #[cfg(test)]
    pub(crate) fn entities(&self) -> Vec<(u64, gpui_kit::EntityId)> {
        let mut out: Vec<_> = self.kept.iter().map(|(&id, k)| (id, k.view.0.entity_id())).collect();
        out.sort_unstable();
        out
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
                // Folded (its first lines) under its head line.
                Kind::Event => rows.get(d.item).map_or(1., |r| {
                    let data = r.event.as_ref().is_some_and(|e| e.data.is_some());
                    text_lines(&r.text, cpl).min(crate::events::EVENT_LINES as f32) + 1. + f32::from(u8::from(data))
                }),
                _ => 1.,
            };
            gap_of(d.gap, m) + lines * m.body_line + 2. * m.pad_y
        })
        .sum();
    total / disp.len() as f32
}

pub(crate) fn gap_of(gap: Gap, m: &Metrics) -> f32 {
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
    /// A slash command's answer ([`ItemKind::Output`]): synaps's words,
    /// not the model's (the TUI's system lines), a message of its own.
    Synaps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    User,
    Prose,
    /// A notice or an error: one mono line.
    Note,
    /// An engine event ([`crate::events`]): a message of its own, as the
    /// user's is (the agent's reply under it is labelled).
    Event,
    /// A foldable run's summary: its items are `item..end`.
    Summary {
        end: usize,
        open: bool,
    },
    /// A thinking or tool item in a run.
    Work,
    /// The local date changed since the last dated row: a day line above
    /// its item's row ([`crate::times::Days`]); its `rev` is the day.
    Day,
    /// A prompt's own row after its item (polish.md §2.6): the authorize
    /// card, collapsing once answered, or a one-line receipt
    /// ([`crate::authorize`]).
    Prompt,
    /// An agent card (synaps-cards design §6.1): its `cards_ui` call's row,
    /// a message of the agent's own, never inside a run (a run never folds
    /// a question away). Open, the card; answered, skipped, closed or
    /// replaced, its receipt ([`crate::ask`]).
    Ask,
    /// A user message that answers a card: the user's turn, drawn as one
    /// slim line, not the message's two ([`crate::ask`]).
    Answer,
    /// A block the agent keeps (synaps-cards `docs/blocks.md` §1.2): the
    /// `at`th of those anchored at its item's call, drawn after the run
    /// that call is in (or after its card), never inside it
    /// ([`crate::blocks`]).
    Block {
        at: u16,
    },
}

/// A day line's row id: its item's, with this bit (item ids count up from 1).
pub(crate) const DAY_ID: u64 = 1 << 63;

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

/// A summary's key in the motion's maps (births, reveals): its id is its
/// run's first item's, which an unfolded run also draws as a work row
/// (`DAY_ID` marks day lines; item ids count up from 1).
const SUMMARY_KEY: u64 = 1 << 62;

impl Disp {
    /// Its key in the motion's maps: its id, a summary's marked (else an
    /// unfold's first row, born, would reveal the clicked header too).
    pub(crate) fn motion_key(&self) -> u64 {
        if matches!(self.kind, Kind::Summary { .. }) { self.id | SUMMARY_KEY } else { self.id }
    }

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

/// The call a prompt waits on: an unfinished call of the last turn, the
/// first (Shady M1: the engine runs a response's calls that change things
/// in one lane, in order, so only its head can be waiting on a confirm).
/// With `pr`: the first whose tool is the one its scope names (and for a
/// guard's shape, whose input holds the command's first line), else the
/// first.
pub(crate) fn waiting_call(items: &[Item], pr: Option<&synaps_client::protocol::PromptRequest>) -> Option<u64> {
    let turn = items.iter().rposition(|i| i.kind == ItemKind::User).map_or(0, |u| u + 1);
    let open: Vec<&Item> =
        items[turn..].iter().filter(|i| i.kind == ItemKind::Tool && !finished(i.tool.as_ref().and_then(|t| t.status))).collect();
    let tool = pr.and_then(|p| p.confirm_scope.as_ref()).and_then(|s| s.tool.as_deref()).filter(|t| !t.is_empty());
    let line = pr.and_then(crate::authorize::guard_shape).map(|g| g.command.lines().next().unwrap_or_default().trim().to_string());
    let named = |i: &&Item| {
        i.tool
            .as_ref()
            .is_some_and(|t| tool.is_none_or(|n| t.name == n) && line.as_deref().is_none_or(|l| l.is_empty() || t.input.contains(l)))
    };
    let pick = if tool.is_some() || line.is_some() { open.iter().find(|i| named(i)) } else { None };
    pick.or(open.first()).map(|i| i.id)
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
/// holding a running subagent's call is live too; `day`: item `i`'s date
/// (its row strings', `RowCache::time`), for the day lines.
#[cfg(test)]
pub(crate) fn build(
    items: &[Item],
    day: &dyn Fn(usize) -> Option<NaiveDate>,
    from: usize,
    busy: bool,
    open: &HashSet<u64>,
    subs: &Subagents,
) -> Vec<Disp> {
    build_with(items, day, from, busy, open, subs, false)
}

/// [`build`], `board`: the transcript's board draws its block calls, so a
/// block call it hasn't drawn yet draws no row either ([`crate::blocks::silent`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_with(
    items: &[Item],
    day: &dyn Fn(usize) -> Option<NaiveDate>,
    from: usize,
    busy: bool,
    open: &HashSet<u64>,
    subs: &Subagents,
    board: bool,
) -> Vec<Disp> {
    let mut out = Vec::new();
    let mut agent = from > 0
        && items[from].kind != ItemKind::User
        && items[..from].iter().rev().find(|x| !x.is_note() || x.kind == ItemKind::Output).is_none_or(|x| x.kind != ItemKind::Output);
    let mut i = from;
    let gap = |i: usize| if i == 0 { Gap::First } else { Gap::Part };
    let day = |i: usize| if items[i].is_note() { None } else { day(i) };
    let mut days = crate::times::Days::after((0..from.min(items.len())).rev().find_map(day));
    while i < items.len() {
        let it = &items[i];
        if let Some(day) = days.next(day(i)) {
            // Its gap is the row's under it, which then sits a part below.
            out.push(Disp {
                item: i,
                id: it.id | DAY_ID,
                rev: crate::times::day_number(day),
                kind: Kind::Day,
                gap: Gap::Part,
                who: Who::None,
            });
        }
        let row = |kind, gap, who| Disp { item: i, id: it.id, rev: it.rev, kind, gap, who };
        match it.kind {
            ItemKind::User if crate::ask::answers(it) => {
                out.push(row(Kind::Answer, if i == 0 { Gap::First } else { Gap::Turn }, Who::You));
                agent = false;
                i += 1;
            }
            ItemKind::Tool if crate::ask::asks(it) => {
                out.push(row(Kind::Ask, gap(i), if agent { Who::None } else { Who::Agent }));
                agent = true;
                // A converted card's block, under its receipt (§1.5).
                crate::blocks::rows(items, i..i + 1, &mut out, &mut agent, true);
                i += 1;
            }
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
                while let Some(x) = items.get(end).filter(|x| matches!(x.kind, ItemKind::Thinking | ItemKind::Tool) && !crate::ask::asks(x))
                {
                    // A run that crosses midnight is two: the day line between.
                    if end > start && days.starts_day(day(end)) {
                        break;
                    }
                    days.next(day(end));
                    if x.kind == ItemKind::Tool && !crate::blocks::silent(x, board) {
                        tools += 1;
                        done &= !live_call(x, subs);
                    }
                    (x.id, rev(x, subs)).hash(&mut h);
                    end += 1;
                }
                // A block call the board draws has no row: its blocks draw
                // after the run (§1.2), and the run is the rest.
                let shows = |k: usize| !crate::blocks::silent(&items[k], board);
                let Some(first_row) = (start..end).find(|&k| shows(k)) else {
                    crate::blocks::rows(items, start..end, &mut out, &mut agent, false);
                    i = end;
                    continue;
                };
                let live = !done || (end == items.len() && busy);
                let who = if agent { Who::None } else { Who::Agent };
                agent = true;
                let first = gap(start);
                // The agent titled it (`collapse` with `run`, §1.3): it folds
                // under that title once finished, however short.
                let titled = (start..end).any(|k| items[k].blocks.as_ref().is_some_and(|b| b.run.is_some()));
                if !live && (tools >= FOLD_AT || titled && tools > 0) {
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
                        out.extend((start..end).filter(|&k| shows(k)).map(|k| Disp {
                            item: k,
                            id: items[k].id,
                            rev: rev(&items[k], subs),
                            kind: Kind::Work,
                            gap: Gap::None,
                            who: Who::None,
                        }));
                    }
                } else {
                    out.extend((start..end).filter(|&k| shows(k)).map(|k| Disp {
                        item: k,
                        id: items[k].id,
                        rev: rev(&items[k], subs),
                        kind: Kind::Work,
                        gap: if k == first_row { first } else { Gap::None },
                        who: if k == first_row { who } else { Who::None },
                    }));
                }
                crate::blocks::rows(items, start..end, &mut out, &mut agent, false);
                i = end;
            }
            // An event: a message the agent answers, as a user's is (a
            // turn's gap above it, the agent's next words labelled); never
            // a user row.
            ItemKind::Event => {
                out.push(row(Kind::Event, if i == 0 { Gap::First } else { Gap::Turn }, Who::None));
                agent = false;
                i += 1;
            }
            // A command's answer: its own label; the agent's next words
            // are labelled again.
            ItemKind::Output => {
                out.push(row(Kind::Note, gap(i), Who::Synaps));
                agent = false;
                i += 1;
            }
            _ => {
                out.push(row(Kind::Note, gap(i), Who::None));
                i += 1;
            }
        }
    }
    for k in 1..out.len() {
        if out[k - 1].kind == Kind::Day {
            out[k - 1].gap = std::mem::replace(&mut out[k].gap, Gap::Part);
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

/// A labelled row's label: its text, colour, and its top in the gutter
/// under the row's gap (level with the row's first line).
pub(crate) fn who_of(d: &Disp, p: &Palette, m: &Metrics) -> Option<(&'static str, Hsla, f32)> {
    match d.who {
        Who::None => None,
        Who::You => Some((label::YOU, p.you, m.pad_y)),
        Who::Agent => {
            let row = match d.kind {
                Kind::Prose => m.body_line,
                Kind::Summary { .. } => m.summary,
                // Level with the card's head line, under its slot's top.
                Kind::Ask => return Some((label::AGENT, p.agent, crate::ask::label_top(m))),
                // Level with a block's header.
                Kind::Block { .. } => crate::blocks::Geo::of(m).head,
                _ => m.row,
            };
            Some((label::AGENT, p.agent, ((row - m.body_line) / 2.).max(0.)))
        }
        // Level with the well's head line (its frame, padding, one mono
        // line: [`AppView::render_output`]).
        Who::Synaps => {
            let head = 2. + 2. * (4. * m.r).round() + m.mono_line;
            Some((label::SYNAPS, p.dim, ((head - m.body_line) / 2.).max(0.)))
        }
    }
}

/// The inline label line's room: the line and the rule's gap under it.
fn inline_line(m: &Metrics) -> f32 {
    m.body_line.max(m.mono_line) + (2. * m.r).round()
}

/// The time fades out in place and in at its end (else it glides with
/// its label). On film (1920, rail closing) the glide crossed the row under the
/// label line, the whole column in about 130 ms: too fast to read, and over
/// the text.
const TIME_GLIDES: bool = false;

/// Where a label's sideways move is done ([`moving_label`]): its corner is
/// clear of its row's card (by 3 px or more at every text step) before the
/// card's top, rising as the line's room closes, reaches its capitals.
const LEAD_BY: f32 = 0.4;

/// The label's sideways share at `k` ([`moving_label`]): a cubic from 0
/// (leaving at the line's rate, `k`'s own) to 1 at [`LEAD_BY`] (landing
/// flat), 1 after.
fn lead(k: f32) -> f32 {
    if k >= LEAD_BY {
        return 1.;
    }
    let t = k / LEAD_BY;
    LEAD_BY * (t * t * t - 2. * t * t + t) + 3. * t * t - 2. * t * t * t
}

/// A label (`lw` px wide) and its time on their way between the line over
/// their row (`k` 0: the label at its left, the time at its right end) and
/// the gutter (`k` 1: both right-aligned `m.hang` left of the column, the
/// time under the label, the label `top` down), in a slot at the row's gap,
/// the column wide, on whole pixels; the time by its right edge (no width
/// measured). The label's sideways move leads and its drop follows (back,
/// the same curve rises first and then moves over): on a straight line its
/// corner crossed its row's card as the line's room closed under it (the
/// finish gate of #33, S1). y on `k²`; x all the way by `k` = [`LEAD_BY`],
/// on a cubic that leaves at the line's rate and lands flat: a lead that
/// starts steeper (an ease-out) nearly cancels the column's own move on a
/// panel's slow tail, and the label shimmers a pixel back and forth there.
#[allow(clippy::too_many_arguments)]
fn moving_label(label: Div, lw: f32, time: Option<&crate::times::Stamp>, top: f32, k: f32, column: f32, p: &Palette, m: &Metrics) -> Div {
    let lerp = |a: f32, b: f32| (a + (b - a) * k).round();
    let (kx, ky) = (lead(k), k * k);
    // By the right edge, `r` px left of the column's.
    let at = |r: f32, y: f32, el: Div| div().absolute().right(px(r)).top(px(y)).whitespace_nowrap().child(el);
    // The time's two places: the line's right end, centred on it; under
    // the label in the gutter.
    let (t0, t1) = (((m.body_line - m.mono_line) / 2.).max(0.), top + m.body_line);
    let text = |t: &crate::times::Stamp, a: f32| mono(t.hhmm.clone(), m.meta, motion::mix(p.canvas, p.dim, a)).line_height(px(m.mono_line));
    let time = time.map(|t| {
        if TIME_GLIDES {
            at(lerp(0., column + m.hang), lerp(t0, t1), text(t, 1.)).into_any_element()
        } else {
            // Out over the first 40 % of the way, in over the last (its
            // ink into the canvas: nothing is drawn with opacity); gone, not
            // drawn (over a card, the canvas's ink would show).
            let (out, into) = (1. - (k / 0.4).min(1.), ((k - 0.6) / 0.4).max(0.));
            div()
                .when(out > 0., |d| d.child(at(0., t0, text(t, out))))
                .when(into > 0., |d| d.child(at(column + m.hang, t1, text(t, into))))
                .into_any_element()
        }
    });
    let label = div().absolute().left(px((-(m.hang + lw) * kx).round())).top(px((top * ky).round())).whitespace_nowrap().child(label);
    div().absolute().left_0().w(px(column)).child(label).children(time)
}

/// A labelled row's time as text: local `HH:MM` in `m.meta` mono, `dim`.
fn time_text(t: &crate::times::Stamp, p: &Palette, m: &Metrics) -> Div {
    mono(t.hhmm.clone(), m.meta, p.dim).line_height(px(m.mono_line))
}

/// A labelled row's time (`crate::times`): [`time_text`], the date and
/// seconds in its tooltip, made when that opens.
/// Static text: it never animates (a history row's least of all).
pub(crate) fn time_slot(t: &crate::times::Stamp, id: u64, p: &Palette, m: &Metrics) -> Stateful<Div> {
    let at = t.clone();
    time_text(t, p, m)
        .id(("time", id))
        .debug_selector(move || format!("time {id}"))
        .tooltip(move |window, cx| crate::overlay::tip(at.readout(), window, cx))
}

/// A day line (`crate::times::Days`): the date in `m.meta` mono, `dim`,
/// between two `hair` rules, across the column.
fn day_line(d: Disp, p: &Palette, m: &Metrics) -> AnyElement {
    let Some(day) = crate::times::day_of(d.rev) else { return div().into_any_element() };
    let (id, rule) = (d.id & !DAY_ID, || div().flex_1().h(px(1.)).bg(p.hair));
    h_flex()
        .w_full()
        .gap(px(m.run_gap))
        .items_center()
        .debug_selector(move || format!("day {id}"))
        .child(rule())
        .child(mono(crate::times::day_label(day), m.meta, p.dim).line_height(px(m.mono_line)))
        .child(rule())
        .into_any_element()
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

/// A tool's verb (polish.md §2.1a): lowercase mono machine words, the same
/// in the row, its tooltip and the folded tally. `multi_edit` is an edit,
/// a worker's start a `spawn`, another `subagent_*` its suffix, an MCP
/// tool (`ext__server__tool`) its own name with its `[server]` in the meta
/// (the TUI's `format_tool_name`). The full name is the tooltip's when the
/// verb is not it.
pub(crate) fn verb_of(name: &str) -> (SharedString, Option<SharedString>) {
    let mcp = name.strip_prefix("ext__").and_then(|r| r.split_once("__")).filter(|(s, t)| !s.is_empty() && !t.is_empty());
    if let Some((server, tool)) = mcp {
        return (tool.to_string().into(), Some(format!("[{server}]").into()));
    }
    match name {
        "multi_edit" => ("edit".into(), None),
        "subagent_start" | "subagent" => ("spawn".into(), None),
        n if n.starts_with("subagent_") => (n["subagent_".len()..].to_string().into(), None),
        n => (n.to_string().into(), None),
    }
}

/// The verb column holds 8 characters and a space: a longer verb is cut
/// to 8 and `…` (its full name in the tooltip).
pub(crate) const VERB_CHARS: usize = 9;

fn verb_cut(v: &str) -> Option<SharedString> {
    (v.chars().count() > VERB_CHARS - 1).then(|| format!("{}…", v.chars().take(VERB_CHARS - 1).collect::<String>()).into())
}

/// `s` with the home folder said as `~` (R3: the TUI does), wherever a
/// path to it starts ([`crate::named::home_len`]: a Windows path however
/// it is spelled). `quotes`: a quote starts a path too (JSON); not in a
/// shell command, where `"~/x"` is not the home.
pub(crate) fn tilde(s: &str, quotes: bool) -> String {
    match crate::named::home() {
        Some(h) => tilde_in(s, h, quotes),
        None => s.to_string(),
    }
}

fn tilde_in(s: &str, home: &str, quotes: bool) -> String {
    let b = s.as_bytes();
    let (mut out, mut from, mut i) = (String::with_capacity(s.len()), 0, 0);
    while i < b.len() {
        let starts = i == 0 || matches!(b[i - 1], b' ' | b'=' | b'(' | b':' | b'\t') || (quotes && matches!(b[i - 1], b'"' | b'\''));
        if starts
            && s.is_char_boundary(i)
            && let Some(n) = crate::named::home_len(&s[i..], home)
        {
            out.push_str(&s[from..i]);
            out.push('~');
            i += n;
            from = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[from..]);
    out
}

/// A tool row's parts as its card shows them (polish.md §2.1, §2.3),
/// formatted once per change of the row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Card {
    /// The verb as drawn (cut to the column), and the tool's own name for
    /// the tooltip when the verb is not it.
    pub verb: SharedString,
    pub full: Option<SharedString>,
    /// The verb whole (the folded tally's), and what the tally counts it
    /// under: the verb, an MCP tool's full name (servers apart).
    pub word: SharedString,
    pub key: SharedString,
    /// An MCP tool's `[server]`, in the meta.
    pub server: Option<SharedString>,
    /// The target, `~` for the home folder.
    pub target: SharedString,
    /// A path target: its folder (`arg`, with its last `/`) and its file
    /// (`prose_t`: the file is what one scans for).
    pub path: Option<(SharedString, SharedString)>,
    /// A finished read's `214 lines`, a grep's or find's `11 hits` (R4), as
    /// the whole result says: `90+ lines`, `40+ hits` when it can only
    /// say at least, `no hits` for a miss.
    pub facts: Option<SharedString>,
    /// The well's header: its path, and on the right `replaced 3 → 6
    /// lines` (an edit) or `lines 41–88` (a read; `of 664` when the file
    /// goes on).
    pub head: Option<(SharedString, Option<SharedString>)>,
    /// A read's lines with the file's own numbers (R6): number, indent
    /// ([`crate::files::hang`]), text.
    pub numbered: Option<Arc<[(SharedString, u16, SharedString)]>>,
    /// A read well's end: the lines read it does not show (`… 32 more
    /// lines`; `… more lines` when how many is not known).
    pub more: Option<SharedString>,
}

/// A read result's line, `N\t…` as the engine numbers it (`read.rs`).
fn numbered_line(l: &str) -> Option<(&str, &str)> {
    let (n, text) = l.split_once('\t')?;
    let n = n.trim_start();
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some((n, text))
}

/// The engine's edit result: `Edited p — replaced 3 line(s) with 6 line(s)`.
fn replaced(result: &str) -> Option<SharedString> {
    let rest = &result[result.rfind("replaced ")? + "replaced ".len()..];
    let (a, rest) = rest.split_once(" line(s) with ")?;
    let b = rest.split_once(" line(s)")?.0;
    let num = |s: &str| s.trim().parse::<u64>().ok();
    Some(format!("replaced {} → {} lines", num(a)?, num(b)?).into())
}

fn plural(n: usize, one: &str, many: &str) -> SharedString {
    format!("{n} {}", if n == 1 { one } else { many }).into()
}

/// What a read returned, from its whole result (the engine's `read.rs`:
/// `N\tline` from `offset + 1`, `limit` lines (500 unless given), then
/// `... (M more lines)` when the file goes on): its first and last line's
/// numbers, and the file's lines past them. `last` is `None` when the
/// result was cut ([`synaps_client::transcript::ToolCall::tail`]) and its
/// end says neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    first: u64,
    last: Option<u64>,
    /// The last number the preview holds.
    seen: u64,
    rest: Option<u64>,
}

/// The engine's `... (M more lines)`.
fn more_lines(l: &str) -> Option<u64> {
    l.strip_prefix("... (")?.strip_suffix(" more lines)")?.parse().ok()
}

fn span(t: &synaps_client::transcript::ToolCall, limit: Option<u64>) -> Option<Span> {
    let num = |l: &str| numbered_line(l).and_then(|(n, _)| n.parse::<u64>().ok());
    let mut nums = t.result.lines().filter_map(num);
    let first = nums.next()?;
    let seen = nums.next_back().unwrap_or(first);
    let end = if t.tail.is_empty() { t.result.as_str() } else { t.tail.as_str() };
    let last = if t.tail.is_empty() { Some(seen) } else { t.tail.lines().rev().find_map(num) };
    let rest = end.lines().rev().find_map(more_lines);
    // The file goes on past it: it read all it was asked to.
    let last = last.or_else(|| rest.map(|_| first + limit.unwrap_or(500) - 1));
    Some(Span { first, last, seen, rest })
}

/// Where a grep line's path can end (`grep.rs`: `path:N:text` a match,
/// `path-N-text` around one; `N:text` / `N-text` for one file): before
/// each `SNS` (S `:` or `-`, N digits), or at 0 before a leading `NS`;
/// with its S, in order.
fn grep_cuts(l: &str) -> Vec<(usize, u8)> {
    let b = l.as_bytes();
    let digits = |from: usize| b.get(from..).map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count());
    let mut out = Vec::new();
    let n = digits(0);
    if n > 0 && matches!(b.get(n), Some(b':' | b'-')) {
        out.push((0, b[n]));
    }
    for i in 1..b.len() {
        let n = digits(i + 1);
        if matches!(b[i], b':' | b'-') && n > 0 && b.get(i + 1 + n) == Some(&b[i]) {
            out.push((i, b[i]));
        }
    }
    out
}

/// A grep's matches with `context` set, in its text before the first
/// blank line. A group's lines (`--` between groups) are one file's and
/// their numbers differ, so a line's path ends at the last cut it shares
/// with a line next to it: numbers in a path (`2026-11-17.md`) or in a
/// text say nothing. A line alone is a match (context comes with one);
/// one that shares no cut goes by its first.
fn grep_matches(body: &str) -> usize {
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    lines
        .split(|l| *l == "--")
        .map(|g| {
            let cuts: Vec<Vec<(usize, u8)>> = g.iter().map(|l| grep_cuts(l)).collect();
            let shared = |j: usize, k: usize| {
                cuts[j]
                    .iter()
                    .rev()
                    .find(|(at, _)| cuts[k].iter().any(|(a, _)| a == at) && g[k].as_bytes()[..*at] == g[j].as_bytes()[..*at])
                    .copied()
            };
            (0..g.len())
                .filter(|&j| {
                    let near = [j.checked_sub(1), (j + 1 < g.len()).then_some(j + 1)];
                    match near.into_iter().flatten().filter_map(|k| shared(j, k)).max_by_key(|(at, _)| *at) {
                        Some((_, sep)) => sep == b':',
                        None => g.len() == 1 || cuts[j].first().is_some_and(|(_, sep)| *sep == b':'),
                    }
                })
                .count()
        })
        .sum()
}

/// A grep's or find's hits (R4), from the engine's text (`search/mod.rs`):
/// `no hits` for its `No matches found.` / `No files found.`; else its
/// lines up to the first blank one (its notes come after one), matches
/// only when `context` is set; `N+` when the result or the search was cut
/// (the preview's slice, a truncation, a timeout, errors).
fn hits(t: &synaps_client::transcript::ToolCall, context: bool) -> SharedString {
    let r = t.result.as_str();
    if r.starts_with("No matches found.") || r.starts_with("No files found.") {
        return "no hits".into();
    }
    let body = r.split("\n\n").next().unwrap_or_default();
    let notes =
        |s: &str| s.split("\n\n").skip(1).any(|n| n.starts_with("... (") || n.contains("while searching, results may be incomplete"));
    let cut = !t.tail.is_empty() || notes(r) || t.tail.lines().any(|l| l.starts_with("... (") || l.contains("results may be incomplete"));
    let n = if context { grep_matches(body) } else { body.lines().filter(|l| !l.trim().is_empty()).count() };
    match (n, cut) {
        (n, true) => format!("{n}+ hits").into(),
        (n, false) => plural(n, "hit", "hits"),
    }
}

impl Card {
    /// Its strings' bytes (a tab cache's budget).
    pub(crate) fn bytes(&self) -> usize {
        let n = |s: &SharedString| s.len();
        n(&self.verb)
            + n(&self.word)
            + n(&self.key)
            + n(&self.target)
            + self.full.as_ref().map_or(0, n)
            + self.server.as_ref().map_or(0, n)
            + self.facts.as_ref().map_or(0, n)
            + self.more.as_ref().map_or(0, n)
            + self.path.as_ref().map_or(0, |(a, b)| n(a) + n(b))
            + self.head.as_ref().map_or(0, |(a, b)| n(a) + b.as_ref().map_or(0, n))
            + self.numbered.as_ref().map_or(0, |l| l.iter().map(|(a, _, b)| n(a) + n(b) + 2 * std::mem::size_of::<SharedString>()).sum())
    }

    /// `arg`: the row's one-line argument ([`crate::app::row_cache`]);
    /// `input`: the call's input parsed.
    pub(crate) fn of(
        t: &synaps_client::transcript::ToolCall,
        preview: Option<&Preview>,
        arg: &str,
        input: Option<&serde_json::Value>,
    ) -> Self {
        let (word, server) = verb_of(&t.name);
        let cut = verb_cut(&word);
        let full = (cut.is_some() || word.as_ref() != t.name).then(|| SharedString::from(t.name.clone()));
        let key = if server.is_some() { SharedString::from(t.name.clone()) } else { word.clone() };
        let field = |k: &str| input.and_then(|v| v.get(k)).and_then(|v| v.as_str());
        let int = |k: &str| input.and_then(|v| v.get(k)).and_then(serde_json::Value::as_u64);
        let file = matches!(t.name.as_str(), "read" | "write" | "edit" | "multi_edit" | "ls");
        let target = match (t.name.as_str(), field("path")) {
            ("grep" | "find", Some(path)) => match field("pattern") {
                Some(pat) => format!("{pat}  {}", tilde(path, false)),
                None => tilde(arg, true),
            },
            ("grep" | "find", None) => field("pattern").map_or_else(|| tilde(arg, true), str::to_string),
            // A path and more (a read's offset, a write's content): the
            // path; the rest says in the meta or the well's header.
            (_, Some(path)) if file && preview.is_none_or(|pv| pv.title.is_empty()) => tilde(path, false),
            ("bash", _) => tilde(arg, false),
            // A card this desk can't draw (synaps-cards design §6.3): what
            // it asks, so it can be answered in text: on one line,
            // sanitized and cut as a card's text is (its input was never
            // validated). With nothing of it left, its input, sanitized.
            ("cards_ui" | "cards:ui", _) => match field("question").map(crate::ask::question_line).filter(|q| !q.is_empty()) {
                Some(q) => format!("«{q}»"),
                None => crate::ask::shown(&tilde(arg, true), cards_spec::Lines::Single),
            },
            _ => tilde(arg, true),
        };
        let is_path = file && !target.contains(" +") && !target.contains('\n') && !target.starts_with('{');
        let path = is_path
            .then(|| target.rfind(['/', '\\']).filter(|at| at + 1 < target.len()))
            .flatten()
            .map(|at| (SharedString::from(target[..=at].to_string()), SharedString::from(target[at + 1..].to_string())));
        let done = t.status == Some(ToolStatus::Done);
        let read = (t.name == "read").then(|| span(t, int("limit").filter(|l| *l > 0))).flatten();
        let numbered: Option<Vec<(SharedString, u16, SharedString)>> = read.map(|_| {
            t.result
                .lines()
                .filter_map(numbered_line)
                .take(crate::app::RESULT_LINES)
                .map(|(a, b)| {
                    let (indent, rest) = crate::files::hang(b);
                    (SharedString::from(a.to_string()), indent, SharedString::from(rest.to_string()))
                })
                .collect()
        });
        let facts = match t.name.as_str() {
            "read" if done => read.map(|s| match s.last {
                Some(last) => plural((last + 1).saturating_sub(s.first) as usize, "line", "lines"),
                None => format!("{}+ lines", s.seen + 1 - s.first).into(),
            }),
            "grep" if done => Some(hits(t, int("context").is_some_and(|c| c > 0))),
            "find" if done => Some(hits(t, false)),
            _ => None,
        };
        let head = match (preview, read) {
            (Some(pv), _) if !pv.title.is_empty() => Some((tilde(&pv.title, false).into(), replaced(&t.result))),
            (_, Some(s)) => {
                let range = match s.last {
                    Some(last) if last != s.first => format!("lines {}–{last}", s.first),
                    Some(_) => format!("line {}", s.first),
                    None => format!("lines {}–{}+", s.first, s.seen),
                };
                let of = s.rest.zip(s.last).map(|(rest, last)| format!(" of {}", last + rest)).unwrap_or_default();
                Some((target.clone().into(), Some(format!("{range}{of}").into())))
            }
            _ => None,
        };
        let more = read.and_then(|s| {
            let shown = numbered.as_ref().and_then(|n| n.last()).and_then(|(n, _, _)| n.parse::<u64>().ok())?;
            match s.last {
                Some(last) if last > shown => Some(SharedString::from(format!("… {} more lines", last - shown))),
                Some(_) => None,
                None => Some("… more lines".into()),
            }
        });
        Card {
            verb: cut.unwrap_or_else(|| word.clone()),
            full,
            word,
            key,
            server,
            target: target.into(),
            path,
            facts,
            head,
            numbered: numbered.map(Into::into),
            more,
        }
    }
}

/// One tool name in a summary's tally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Its verb whole, and what it counts under ([`Card::key`]).
    pub name: SharedString,
    pub key: SharedString,
    pub n: usize,
    pub n_text: SharedString,
    pub hue: Hue,
    /// The characters of its name (mono: its width, without layout).
    pub name_chars: usize,
}

/// A text box of `chars` mono characters (`cw` px each) as GPUI lays it
/// out: its advances, rounded up to a whole pixel (`TextLayout` rounds
/// every text element's width up, and nothing gives the pixel back).
pub(crate) fn text_w(chars: usize, cw: f32) -> f32 {
    (chars as f32 * cw).ceil()
}

/// How many of `tally`'s tools a folded run's summary names in `avail` px
/// of mono text (`cw` px a character): each `name n` (two text boxes, a
/// character apart), `gap` apart. All of them, or as many as fit with the
/// `+N` that ends the row (N: the calls of the tools not named, so the
/// counts shown add up to the summary's `n tools`); none if not even one
/// does.
pub(crate) fn tally_fit(tally: &[Tally], avail: f32, cw: f32, gap: f32) -> usize {
    let w = |t: &Tally| text_w(t.name_chars, cw) + cw + text_w(t.n_text.len(), cw);
    let all = tally.iter().map(w).sum::<f32>() + gap * tally.len().saturating_sub(1) as f32;
    if all <= avail {
        return tally.len();
    }
    let mut rest = tally.iter().map(|t| t.n).sum::<usize>();
    let (mut used, mut k) = (0., 0);
    for t in tally {
        rest -= t.n;
        let item = w(t) + if k > 0 { gap } else { 0. };
        let tail = gap + text_w(1 + digits(rest), cw);
        if used + item + tail > avail {
            break;
        }
        used += item;
        k += 1;
    }
    k
}

fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |l| l as usize + 1)
}

/// `+N`, the end of a tally cut short: static for the counts a fold
/// usually hides (no string a frame), formatted past them.
fn plus(n: usize) -> SharedString {
    const PLUS: [&str; 25] = [
        "+0", "+1", "+2", "+3", "+4", "+5", "+6", "+7", "+8", "+9", "+10", "+11", "+12", "+13", "+14", "+15", "+16", "+17", "+18", "+19",
        "+20", "+21", "+22", "+23", "+24",
    ];
    PLUS.get(n).map_or_else(|| format!("+{n}").into(), |&p| p.into())
}

/// What a folded run's summary says: `› 7 tools  bash 3  read 3  grep 1 …`
/// and its last outcome's glyph, every string formatted once per change
/// of the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunSummary {
    pub tools: usize,
    /// The agent's title for it (`collapse` with `run`, blocks.md §1.3):
    /// drawn where `N tool calls` is.
    pub title: Option<SharedString>,
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
    /// Its last call's outcome's glyph (how the run ended: a failure fixed
    /// later reads ✓, its `1 failed` in the meta).
    pub last: (Glyph, Family),
    /// The whole tally, a tool a line (`bash 4`, `read 3`, …): the tooltip
    /// of one cut short, as narrow as its longest name.
    pub tip: SharedString,
    /// Each count of what did not end ok, worst first (failed or denied >
    /// stopped at the limit > cancelled): `1 failed`, `1 stopped`. Ok is
    /// silent: nothing when every call is done.
    pub outcome: Vec<(SharedString, Family)>,
}

impl RunSummary {
    pub(crate) fn of(items: &[Item], rows: &[RowCache]) -> Self {
        let mut tally: Vec<Tally> = Vec::new();
        let (mut tools, mut ok, mut failed, mut denied, mut cancelled, mut stopped) = (0, 0, 0, 0, 0, 0);
        let mut delta = CallDelta::default();
        let mut last = (Glyph::Ok, Family::Ok);
        let title = items.iter().find_map(|i| i.blocks.as_ref()?.run.as_deref().map(SharedString::new));
        for (it, row) in items.iter().zip(rows).filter(|(i, _)| i.kind == ItemKind::Tool && !crate::blocks::silent(i, true)) {
            tools += 1;
            let (g, _, family) = glyph::call(status(it), false, false);
            last = (g, family);
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
            // By verb (polish.md §2.1a): `multi_edit` counts as an edit;
            // an MCP tool by its full name (two servers' `search` apart).
            let (name, key) = if row.card.key.is_empty() { (&row.name, &row.name) } else { (&row.card.word, &row.card.key) };
            match tally.iter_mut().find(|t| &t.key == key) {
                Some(t) => t.n += 1,
                None => tally.push(Tally {
                    name: name.clone(),
                    key: key.clone(),
                    n: 1,
                    n_text: SharedString::default(),
                    hue: row.hue,
                    name_chars: 0,
                }),
            }
        }
        // Stable: equal counts keep their first-use order.
        tally.sort_by_key(|a| std::cmp::Reverse(a.n));
        for t in &mut tally {
            t.n_text = t.n.to_string().into();
            t.name_chars = t.name.chars().count();
        }
        // The tools' own names, never cut (an MCP tool's full name).
        let tip = tally.iter().map(|t| format!("{} {}", t.key, t.n)).collect::<Vec<_>>().join("\n").into();
        let parts = [
            (failed, "failed", Family::Err),
            (denied, "denied", Family::Err),
            (stopped, "stopped", Family::Warn),
            (cancelled, "cancelled", Family::Warn),
        ];
        // Ok is silent (C10, polish.md §2.2): only what did not end ok.
        let outcome: Vec<(SharedString, Family)> =
            parts.into_iter().filter(|p| p.0 > 0).map(|(n, word, family)| (format!("{n} {word}").into(), family)).collect();
        RunSummary {
            tools,
            title,
            label: format!("{tools} calls").into(),
            tally,
            ok,
            failed,
            denied,
            cancelled,
            stopped,
            delta,
            counts: Counts::of(delta),
            last,
            tip,
            outcome,
        }
    }

    /// Its trail's width at `meta` px (mono), as [`AppView::render_summary`]
    /// lays it out: the counts, then (not `tight`) the words, `·` between,
    /// every part 8 px apart, each text a whole pixel wide. Its last
    /// outcome's glyph is in the gutter.
    pub(crate) fn trail_w(&self, meta: f32, tight: bool) -> f32 {
        let cw = 0.6 * meta;
        let chars = |t: &SharedString| text_w(t.chars().count(), cw);
        let (mut w, mut parts) = (0., 0usize);
        let counts = [&self.counts.written, &self.counts.added, &self.counts.removed];
        let shown = counts.iter().filter(|c| c.is_some()).count();
        if shown > 0 {
            // The counts, 8 apart: one part.
            w += counts.iter().filter_map(|c| c.as_ref()).map(chars).sum::<f32>() + 8. * (shown - 1) as f32;
            parts += 1;
        }
        if !tight && !self.outcome.is_empty() {
            for (i, (text, _)) in self.outcome.iter().enumerate() {
                if i > 0 || shown > 0 {
                    w += text_w(1, cw);
                    parts += 1;
                }
                w += chars(text);
                parts += 1;
            }
        }
        w + 8. * parts.saturating_sub(1) as f32
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

    /// Every run summarised: its id and summary.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u64, &RunSummary)> {
        self.0.iter().map(|(&id, e)| (id, &*e.1))
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
    /// The hang as drawn: 1 hanging, 0 inline; between, the labels on their
    /// way with a side panel's flight ([`HangFx`]). At rest, `hang`'s.
    pub hang_k: f32,
    /// Under 560 px: the gutter drops to 16, status words and hints go.
    pub tight: bool,
    /// Under [`crate::fit::short_below`] tall (a 700-px window at 16 px,
    /// more with larger text): a 40-px top bar, an 8-line card well, less
    /// padding round the composer. Nothing in the list
    /// changes, so it never remeasures for this.
    pub short: bool,
    /// Under [`crate::fit::cramped_below`] (short too): the card folds its
    /// lead into its title row, the composer puts SEND beside the text.
    pub cramped: bool,
    /// The list's left padding: the column centred, whole pixels.
    pub left: f32,
    /// And its right.
    pub right: f32,
    /// How far the column's box is drawn from the pane's left edge: 0 but
    /// in a widening flight's reveal ([`Reveal`]), where the box is the
    /// pane as it lands (`left` and `right` are that pane's), moved so the
    /// column stays where the reveal puts it in the window.
    pub shift: f32,
}

impl Layout {
    /// `pane` wide; `height` is the pane's height under the full (52-px)
    /// top bar, so the short bar cannot flip it back. Short and cramped
    /// read the height, the text step and `ex` (the model's state) only:
    /// nothing they change feeds back.
    pub(crate) fn of(pane: f32, height: f32, m: &Metrics, ex: crate::fit::Extras) -> Self {
        Self::of_in(pane, pane, height, m, ex)
    }

    /// [`Layout::of`] with the column laid out for a `wrap` px pane (its
    /// width, the labels' hang, the tight gutter) and centred in `pane`:
    /// in a flight, `wrap` is the pane as the flight lands, so the rows
    /// wrap once, as it starts, and only the padding moves (fluid.md §3.8;
    /// widening, the column is placed by its [`Reveal`]).
    pub(crate) fn of_in(pane: f32, wrap: f32, height: f32, m: &Metrics, ex: crate::fit::Extras) -> Self {
        let tight = wrap < 560.;
        let gutter = if tight { 16. } else { 24. };
        let column = m.column.min(wrap - 2. * gutter).max(200.).floor();
        let left = ((pane - column) / 2.).floor().max(0.);
        let hang = hangs(wrap, m);
        Layout {
            column,
            hang,
            hang_k: if hang { 1. } else { 0. },
            tight,
            short: height < crate::fit::short_below(m),
            cramped: height < crate::fit::cramped_below(m, ex),
            left,
            right: (pane - column - left).max(0.),
            shift: 0.,
        }
    }

    pub(crate) fn form(self) -> crate::fit::Form {
        match (self.short, self.cramped) {
            (_, true) => crate::fit::Form::Cramped,
            (true, false) => crate::fit::Form::Short,
            (false, false) => crate::fit::Form::Full,
        }
    }

    /// The list's left and right padding (the card, the tray and the
    /// composer use the same, so their edges line up with the rows').
    pub(crate) fn padding(self) -> (f32, f32) {
        (self.left, self.right)
    }
}

/// A `pane` px pane hangs its labels: it has the column and 200 rem/16.
pub(crate) fn hangs(pane: f32, m: &Metrics) -> bool {
    pane >= (672. + 200.) * m.r
}

/// The labels' move between their line over the row and the gutter, with
/// the side panels (Haseeb, 2026-11-21: a cut a frame after the flight read
/// as choppy). The hang is the flight's end's, decided as it starts (its
/// `pane_to`); what is drawn, `k` (0 inline, 1 hanging), follows the centre
/// pane's own progress from where it was to there, so the labels move on the
/// panel spring's timing, each frame's step one with the pane's, and a
/// reversal turns from where they are. The rows wrap as they did (the
/// column is the flight's narrowest, fluid.md §3.8): only the label line's
/// room closes or opens, and the label and its time travel. No flight (a
/// window resize, a drag, Reduced, Off): a cut. The rows off screen are
/// measured again once, when it lands (either side: one turned back to
/// where it began has rows laid out on the way too).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HangFx {
    /// What is drawn this frame.
    pub(crate) k: f32,
    /// The hang the rows were last all measured at.
    measured: bool,
    /// Rows were laid out between the two since (a flight turned back to
    /// its start side lands measured again too: Shady #33 L1).
    between: bool,
    /// Its flight: `k` as it began, the pane then, the pane it flies to,
    /// and the hang there.
    from: Option<(f32, f32, f32, bool)>,
    started: bool,
}

impl HangFx {
    /// This frame's: `lay` with its hang the flight's end's (`pane_to`) and
    /// `hang_k` as drawn; and whether the rows must be measured again (it
    /// landed on the other side, or back where it began from on the way).
    pub(crate) fn frame(&mut self, lay: Layout, pane: f32, pane_to: f32, flying: bool, m: &Metrics) -> (Layout, bool) {
        let hang = hangs(pane_to, m);
        let to = if hang { 1. } else { 0. };
        if !self.started {
            (self.k, self.started) = (to, true);
        }
        if flying && self.k != to {
            if !matches!(self.from, Some((_, _, p, h)) if p == pane_to && h == hang) {
                self.from = Some((self.k, pane, pane_to, hang));
            }
            let (k0, p0, p1, _) = self.from.unwrap_or((self.k, pane, pane_to, hang));
            let f = if (p1 - p0).abs() < 0.5 { 1. } else { ((pane - p0) / (p1 - p0)).clamp(0., 1.) };
            self.k = k0 + (to - k0) * f;
        } else {
            (self.k, self.from) = (to, None);
        }
        let at_rest = self.k == 0. || self.k == 1.;
        self.between |= !at_rest;
        let landed = at_rest && ((self.k == 1.) != self.measured || self.between);
        if landed {
            (self.measured, self.between) = (self.k == 1., false);
        }
        (Layout { hang, hang_k: self.k, ..lay }, landed)
    }
}

/// A widening flight's column (Haseeb, 2026-11-26: a rail closing kept
/// the old, narrow column for the whole flight, then jumped). From the
/// flight's first frame the column is the one it lands with, laid out in
/// the pane as it lands, and the moving edge reveals it: its far edge
/// (from the edge that moves) stays where it was, and the column goes on
/// to its landed place with the pane's width. A column that takes all the
/// width it gains (under the cap, the same gutters) is where it lands from
/// the first frame and does not move at all; one at the cap glides as the
/// centred column did. One whose far edge held would put its near edge in
/// view (a gain under its gutter) holds its near edge instead, and glides.
/// Each an affine map of the pane's width: on the panel's spring, with no
/// curve of its own. Everything in the window, on the window's x.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reveal {
    /// The pane in the frame before the flight: its left edge and width.
    pub from: (f32, f32),
    /// Its column then: its left edge and width.
    pub col: (f32, f32),
    /// The pane as it lands (its left edge and width), and the column's
    /// left edge in it there (the landed layout's `left`).
    pub to: (f32, f32),
    pub left: f32,
    /// The landed column's width.
    pub column: f32,
    /// The column's left edge as last drawn ([`Reveal::step`]).
    pub drawn: Option<f32>,
}

impl Reveal {
    /// [`Reveal::column_x`] for the frame drawn now, never back from the
    /// one before towards where it started: a pane's width, from edges on
    /// half pixels, can step back half a pixel in a flight's tail, and the
    /// column would twitch a pixel back and forth.
    pub(crate) fn step(&mut self, w: f32) -> f32 {
        let (x, landed) = (self.column_x(w), self.to.0 + self.left);
        let x = match self.drawn {
            Some(d) if d > landed => x.clamp(landed, d),
            Some(d) if d < landed => x.clamp(d, landed),
            Some(_) => landed,
            None => x,
        };
        self.drawn = Some(x);
        x
    }

    /// The column's left edge in the window, the pane `w` wide this frame.
    pub(crate) fn column_x(&self, w: f32) -> f32 {
        let ((x0, w0), (x1, w1)) = (self.from, self.to);
        let landed = x1 + self.left;
        let grow = w1 - w0;
        if grow < 0.5 {
            return landed;
        }
        // What the column gains, taken on each side as the pane's edges
        // move: all on the left when only the left edge moves (the rail),
        // none when only the right one does (the right panel). Both edges
        // moving the same way (a pane in a split, its divider riding the
        // rail), all on the side that leads: the far edge holds still in
        // the first frame, never stepping back to make room.
        let held = self.col.0 - (self.column - self.col.1) * ((x0 - x1) / grow).clamp(0., 1.);
        // Held, the far edge puts the near one where the pane shows it (a
        // gain under its gutter, near the cap): it would hop. The near edge
        // holds instead, and the column glides on with the pane.
        let start = if held > x0 { self.col.0 } else { held };
        // Within a pixel of it (the floors of the centring): where it
        // lands, never a pixel's step in the flight.
        if (start - landed).abs() <= 1. {
            return landed;
        }
        let k = ((w - w0) / grow).clamp(0., 1.);
        start + (landed - start) * k
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
        let keep = self.panes.cur.texts.borrow_mut().take(id, text);
        let kept = self.panes.cur.texts.borrow().view(id).map(|(v, _)| v.clone());
        let view = match (keep, kept) {
            (Keep::Same, Some(v)) => v,
            (Keep::Append, Some(v)) => {
                // Text streamed in: its words start at 70 % and reach full
                // colour (fluid.md §3.4; `motion::text_motion`, the one
                // policy). Not in low power (the agent's motion stops).
                let motion = if self.fx.fluid() { motion::text_motion() } else { Default::default() };
                v.update(cx, |s, cx| {
                    s.set_motion(motion);
                    s.set_text(text, cx)
                });
                v
            }
            _ => {
                // A view made with its text (history, a new row) has no
                // motion policy: what it starts with is not streamed, and
                // history never animates.
                let v = cx.new(|cx| TextViewState::markdown(text, cx));
                let redraw = cx.observe(&v, |_, _, cx| cx.notify());
                self.panes.cur.texts.borrow_mut().put(id, text.clone(), (v.clone(), redraw), now);
                v
            }
        };
        let mut rows = self.panes.cur.texts.borrow_mut();
        let wait = rows.wait(ix, id, now, |(v, _)| !v.read(cx).rendered_text().source().is_empty());
        let h = if wait { text_lines(text, rows.cpl) * m.body_line } else { 0. };
        (TextView::new(&view), px(h))
    }

    /// A folding run's rows (N1), as they were drawn: the blocks inside its
    /// card, which the room under its header draws round them.
    pub(crate) fn fold_rows(&self, rows: &[Disp], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let items = self.model.lives[self.panes.cur.live].shown().items();
        let (p, m, lay) = (theme::palette(cx), theme::metrics(cx), self.panes.cur.layout);
        rows.iter()
            .filter_map(|d| {
                let (item, row) = (items.get(d.item)?, self.panes.cur.rows.get(d.item)?);
                let block = self.render_work(item, row, d.id, &p, &m, lay, cx);
                Some(div().w_full().pt(px(gap_of(d.gap, &m))).child(block).into_any_element())
            })
            .collect()
    }

    /// One display row: its gap, its label and the block.
    pub(crate) fn render_disp(&self, ix: usize, cx: &mut Context<Self>) -> RowParts {
        let Some(&d) = self.panes.cur.disp.get(ix) else { return RowParts::empty() };
        let items = self.model.lives[self.panes.cur.live].shown().items();
        let (Some(item), Some(row)) = (items.get(d.item), self.panes.cur.rows.get(d.item)) else { return RowParts::empty() };
        let p = theme::palette(cx);
        let m = theme::metrics(cx);
        let lay = self.panes.cur.layout;
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
            Kind::User => {
                // The you bar over its left edge (design.md §8.11): the
                // text at `inset`, the bar inside it; drawn top down when
                // the block arrives, and the target of a send's Handoff
                // (motion.md §5.2).
                let fx = &self.fx;
                let born = fx.col(self.panes.cur.id).birth(item.id);
                let lit = match (born, fx.col(self.panes.cur.id).sent().get()) {
                    (Some(b), Some((at, false))) if b.saturating_duration_since(at) < Duration::from_secs(2) => {
                        fx.flash(Some(at + 2 * motion::STAGGER))
                    }
                    _ => 0.,
                };
                let bar = motion::mix(p.you_mark, motion::mix(p.you, p.ink, 0.35), lit);
                let id = d.id;
                div()
                    .debug_selector(move || format!("user-block {id}"))
                    .relative()
                    .w_full()
                    .bg(p.user)
                    .px(px(m.pad_x))
                    .py(px(m.pad_y))
                    .text_px(m.body)
                    .line_height(px(m.body_line))
                    .text_color(p.ink)
                    .min_h(min_h)
                    .children(md.map(|v| v.style(cx.global::<theme::Scheme>().user_text.clone())))
                    .child(glyph::bar_at(bar, fx.draw(born, Duration::ZERO, motion::BASE)))
                    .into_any_element()
            }
            Kind::Prose => {
                let body = div().w_full().min_h(min_h).text_px(m.body).line_height(px(m.body_line)).text_color(p.prose);
                match md {
                    Some(v) => body.child(v).into_any_element(),
                    None => body.child(row.text.clone()).into_any_element(),
                }
            }
            Kind::Note if item.kind == ItemKind::Output => self.render_output(item, row, &p, &m, cx),
            Kind::Event => match row.event.as_deref() {
                Some(v) => self.render_event(item.id, v, row.time.as_ref(), &p, &m, cx),
                None => mono(row.text.clone(), m.meta, p.dim).w_full().whitespace_normal().into_any_element(),
            },
            Kind::Note => {
                let color = if item.kind == ItemKind::Error { p.err } else { p.dim };
                mono(row.text.clone(), m.meta, color).w_full().whitespace_normal().into_any_element()
            }
            Kind::Summary { open, .. } => self.render_summary(d, open, &p, &m, cx),
            Kind::Work => self.render_work(item, row, d.id, &p, &m, lay, cx),
            Kind::Day => day_line(d, &p, &m),
            Kind::Prompt => self.render_authorize(d, &p, &m, cx),
            Kind::Ask => self.render_ask(item, d.id, &p, &m, cx),
            Kind::Answer => self.render_answer(item, d.id, &p, &m),
            Kind::Block { at } => self.render_block(item, at, d.id, &p, &m, cx),
        };
        // A run is one card (cards4/proposal.md §3): each of its rows draws
        // its slice, the first and last its edges. The slice (fill,
        // caps) is drawn outside the row's reveal clip and lift
        // (Shady #23 H2): a row that reveals grows the card, whose edges
        // are whole on every frame.
        let card: Option<CardFrame> = match crate::cards::edges(&self.panes.cur.disp, ix) {
            _ if !crate::cards::in_card(&d) => None,
            (top, bottom) => {
                // Folding (N1): its card goes on in the room under it.
                let bottom =
                    bottom && !self.panes.cur.fold_gap.borrow().iter().any(|g| g.is(d.id) && matches!(d.kind, Kind::Summary { .. }));
                let (pal, geo, scale) = (p, crate::cards::Geo::of(&m), self.scale);
                let id = d.id;
                let click = matches!(d.kind, Kind::Summary { .. }).then(|| {
                    cx.listener(move |this, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        this.toggle_run(id);
                        cx.notify();
                    })
                });
                Some(Box::new(move |inner: AnyElement| {
                    let slab = crate::cards::slab(top, bottom, inner, &pal, geo, scale).debug_selector(move || format!("card-slice {id}"));
                    match click {
                        // Its card's edge and inset unfold it too.
                        Some(f) => slab.id(("run-card", id)).cursor_pointer().on_click(f).into_any_element(),
                        None => slab.into_any_element(),
                    }
                }))
            }
        };
        let who = who_of(&d, &p, &m);
        let mut out = v_flex().w_full().pt(px(gap)).relative();
        let mut hang = None;
        if let Some((text, color, top)) = who {
            let id = d.id;
            let l = display(text, m.label, color).line_height(px(m.body_line)).debug_selector(move || format!("label {id}"));
            // Its time: under it in the margin, or at the line's end.
            let time = row.time.as_ref().map(|t| time_slot(t, d.id, &p, &m));
            if lay.hang_k >= 1. {
                // The label and its time hang in the gutter, outside the
                // row's reveal clip (`RowParts::label`).
                let slot = v_flex().items_end().child(l).children(time);
                hang = Some((slot.absolute().right(px(lay.column + m.hang)).debug_selector(move || format!("hang-label-{id}")), gap + top));
            } else if lay.hang_k > 0. {
                // On its way ([`HangFx`]): the line's room closes as `k`
                // goes to 1, and the label and its time travel between
                // their two places outside the clip, as hanging.
                let k = lay.hang_k;
                // On whole pixels (the layout's), as the labelled rows'
                // rooms add up: rounded alike they would step together, the
                // rows under n of them n px in a frame. The `n`th's is what
                // rounding n + 1 of them adds to n.
                let n = self.panes.cur.disp[..ix].iter().filter(|d| d.who != Who::None).count() as f32;
                let c = (1. - k) * inline_line(&m);
                out = out.child(div().flex_none().h(px(((n + 1.) * c).round() - (n * c).round())));
                let lw = self.widths.borrow().get(&(text, m.label.to_bits())).copied().unwrap_or(0.);
                hang = Some((
                    moving_label(l, lw, row.time.as_ref(), top, k, lay.column, &p, &m).debug_selector(move || format!("hang-move-{id}")),
                    gap,
                ));
            } else {
                let line = h_flex().w_full().justify_between().items_center().child(l).children(time);
                out = out.child(line.mb(px((2. * m.r).round())));
            }
        }
        // The write head (motion.md §6.1): where the agent is writing, the
        // streaming row's bottom-left in the gutter; lit while deltas come,
        // decaying once they stop. Touches no text.
        // Not on a day line over it (they share the item).
        let head = (!matches!(d.kind, Kind::Day | Kind::Prompt | Kind::Ask)
            && d.item + 1 == items.len()
            && matches!(item.kind, ItemKind::Assistant | ItemKind::Thinking))
        .then(|| {
            self.fx.col(self.panes.cur.id).change(
                Tag::Stream,
                item.id,
                item.text.len() as u64,
                self.fx.col(self.panes.cur.id).birth(item.id),
            )
        })
        .flatten()
        .map(|(at, _)| self.fx.decay(Some(at), motion::HEAD_HOLD, motion::GLOW))
        .filter(|k| *k > 0.)
        .map(|k| {
            let mark = if item.kind == ItemKind::Thinking { p.think } else { p.run_mark };
            div()
                .absolute()
                // Clear of a gutter glyph's column (it ends at −8·r).
                .left(px((-6. * m.r).round()))
                .bottom_0()
                .w(px(2.))
                .h(px((26. * m.r).round()))
                .bg(motion::mix(p.canvas, mark, k))
                .debug_selector(|| "write-head".into())
        });
        match card {
            Some(frame) => RowParts { body: out, label: hang, head, card: Some((block, frame)) },
            None => RowParts { body: out.child(block), label: hang, head, card: None },
        }
    }

    /// The top message's label pinned over the list (polish.md P15,
    /// [`crate::sticky`]; the one above it too while it is pushed out),
    /// built after the list's layout; `b` is the layer's box, the list's.
    /// Hanging, a label and its time sit in the gutter at `pad_y`, as their
    /// slot draws them, on the empty canvas; the canvas under them, from the
    /// top, hides the original going out. Inline (F8), the label line sits
    /// at the column's top on a `canvas` band one line tall, a 1 px `hair`
    /// rule under it; the band takes the pointer (not the wheel) and hides
    /// the beat under it. Nothing in it asks for a frame. On the labels' way
    /// between the two ([`HangFx`]), the pinned label and its time travel
    /// with the rows'; the band is the layout's (the flight's end's): going
    /// to the gutter it closes and its rule fades as they leave it, coming
    /// back it is there from the start (Q5: pinned on its band while the
    /// layout is inline) and they travel into it.
    pub(crate) fn sticky_label(&self, b: Bounds<Pixels>, cx: &App) -> Option<crate::sticky::Pinned> {
        let (p, m, lay) = (theme::palette(cx), theme::metrics(cx), self.panes.cur.layout);
        let k = lay.hang_k;
        // Placed as hanging, on their way to the gutter; inline, as inline.
        // A flight back inline that has not moved yet (its first frame, the
        // pane where it was) is drawn as it was: in the gutter, at its `pad_y`
        // (the finish gate of #33, M1: placed at the inline pin it hopped up
        // a frame).
        let kh = if lay.hang || k >= 1. { k } else { 0. };
        let time = |i: usize| self.panes.cur.disp.get(i).and_then(|d| self.panes.cur.rows.get(d.item)).and_then(|r| r.time.as_ref());
        // Where the labels pin at `kh` (0 inline, 1 hanging), and how tall.
        let h = |kh: f32, i: usize| m.body_line + (1. - kh) + if time(i).is_some() { kh * m.mono_line } else { 0. };
        let place = |kh: f32| {
            let at = |i: usize| {
                let (d, row) = (self.panes.cur.disp.get(i)?, self.panes.cur.list.bounds_for_item(i)?);
                let top = kh * who_of(d, &p, &m).map_or(0., |w| w.2);
                Some(f32::from(row.top() - b.top()) + gap_of(d.gap, &m) + top)
            };
            crate::sticky::place(&self.panes.cur.disp, self.panes.cur.list.logical_scroll_top().item_ix, at, kh * m.pad_y, |i| h(kh, i))
        };
        let stuck = place(kh);
        // On the way, the label (and its time) is where it pins at the `k`
        // drawn, either way: from the gutter's place to the line's, the
        // band the layout's (the finish gate of #33, M1).
        let glide = if k > 0. && k < 1. { place(k) } else { stuck };
        let bottom = stuck.iter().flatten().map(|s| s.y + h(kh, s.row)).reduce(f32::max)?;
        // The slot's right edge, as the row's (`right(column + hang)`).
        let gutter = px((lay.left - m.hang).max(0.));
        let root = div()
            .relative()
            .w(b.size.width)
            .h(b.size.height)
            .when(k > 0., |d| d.child(div().absolute().left_0().top_0().w(gutter).h(px(bottom)).bg(p.canvas)));
        // Inline, the band (and its rule) across the column, from the top;
        // on the way, as tall as it still is.
        let band_h = ((1. - kh) * m.body_line).round();
        let band_bottom = if k > 0. { stuck.iter().flatten().map(|s| s.y + band_h + 1.).reduce(f32::max).unwrap_or(0.) } else { bottom };
        let cover = (k < 1. && band_h > 0.).then(|| {
            Bounds::from_corners(
                point(b.left() + px(lay.left), b.top()),
                point(b.left() + px(lay.left + lay.column), b.top() + px(band_bottom)),
            )
        });
        let el = root
            .children(stuck.into_iter().flatten().filter_map(|s| {
                let d = self.panes.cur.disp[s.row];
                let (text, color, _) = who_of(&d, &p, &m)?;
                let id = d.id;
                let label = display(text, m.label, color).line_height(px(m.body_line)).debug_selector(move || format!("sticky-label {id}"));
                let time = time(s.row).map(|t| time_text(t, &p, &m));
                if k >= 1. {
                    let slot = v_flex().absolute().right(b.size.width - gutter).top(px(s.y)).items_end().child(label).children(time);
                    return Some(slot.debug_selector(move || format!("sticky {id}")).into_any_element());
                }
                // It hides live rows: nothing under it is clicked or hovered.
                let band = || div().absolute().left(px(lay.left)).w(px(lay.column)).block_mouse_except_scroll();
                if k > 0. {
                    let stamp = self.panes.cur.rows.get(d.item).and_then(|r| r.time.as_ref());
                    let lw = self.widths.borrow().get(&(text, m.label.to_bits())).copied().unwrap_or(0.);
                    let y = glide.iter().flatten().find(|g| g.row == s.row).map_or(s.y, |g| g.y);
                    let slot = div()
                        .absolute()
                        .left(px(lay.left))
                        .top(px(y))
                        .w(px(lay.column))
                        .child(moving_label(label, lw, stamp, 0., k, lay.column, &p, &m));
                    let rule = band().top(px(s.y + band_h)).h(px(1.)).bg(motion::mix(p.canvas, p.hair, 1. - kh));
                    let band = band().top(px(s.y)).h(px(band_h)).bg(p.canvas).debug_selector(move || format!("sticky {id}"));
                    return Some(div().child(band).child(rule).child(slot).into_any_element());
                }
                let line = h_flex().size_full().justify_between().items_center().child(label).children(time);
                let rule = band().top(px(s.y + m.body_line)).h(px(1.)).bg(p.hair).debug_selector(move || format!("sticky-rule {id}"));
                let band = band().top(px(s.y)).h(px(m.body_line)).bg(p.canvas).child(line).debug_selector(move || format!("sticky {id}"));
                Some(div().child(band).child(rule).into_any_element())
            }))
            .into_any_element();
        Some(crate::sticky::Pinned { el, cover })
    }

    /// What the sticky band hides at the list's top when the labels are
    /// inline (F8: one label line and its rule); hanging, nothing.
    pub(crate) fn sticky_inset(&self, cx: &App) -> f32 {
        // The band's own `kh`: coming back inline it is whole once it moves.
        let lay = self.panes.cur.layout;
        let kh = if lay.hang || lay.hang_k >= 1. { lay.hang_k } else { 0. };
        (1. - kh) * (theme::metrics(cx).body_line + 1.)
    }
}

/// A transcript row as drawn ([`AppView::render_disp`]): its body, and what
/// hangs in the gutter, left of the row's box: the `YOU`/`AGENT` label
/// (positioned, and its top in the row) and the write head (on the row's
/// bottom). A row that reveals or grows clips its body and not these
/// ([`motion::grow_block`]).
pub(crate) struct RowParts {
    pub(crate) body: Div,
    pub(crate) label: Option<(Div, f32)>,
    pub(crate) head: Option<Div>,
    /// A card row's block and its card's slice around it (cards4/proposal.md
    /// §3): a reveal clips and lifts the block inside the slice, never the
    /// slice (Shady #23 H2).
    pub(crate) card: Option<(AnyElement, CardFrame)>,
}

/// A card row's slice of its card, drawn round what the row shows.
pub(crate) type CardFrame = Box<dyn FnOnce(AnyElement) -> AnyElement>;

impl RowParts {
    fn empty() -> Self {
        RowParts { body: div(), label: None, head: None, card: None }
    }

    /// At rest: one element, as the row always was.
    pub(crate) fn whole(self) -> AnyElement {
        self.body
            .children(self.card.map(|(block, frame)| frame(block)))
            .children(self.label.map(|(l, top)| l.top(px(top))))
            .children(self.head)
            .into_any_element()
    }
}

/// Where a tool row's parts sit (polish.md §2.1, with F1 adopted): bare on
/// the canvas, the verb on the prose column. With the labels hanging, the
/// row's box reaches `out` into the gutter for its status glyph, so the
/// hover fill takes the glyph in; in a narrow pane the glyph has the row's
/// lead column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geo {
    /// How far the row's box reaches left of the column.
    pub out: f32,
    /// The box's own padding, left (before the status glyph) and right.
    pub pad: f32,
    pub right: f32,
    /// The verb column: 9 mono characters.
    pub verb_w: f32,
    /// Between the verb, the target, the meta and the fold.
    pub gap: f32,
    /// The verb's x in the column (a well's left edge) and the target's.
    pub verb: f32,
    pub target: f32,
    /// The fold chevron's column, at the right end.
    pub fold: f32,
}

impl Geo {
    pub(crate) fn of(lay: Layout, m: &Metrics) -> Self {
        let gap = (8. * m.r).round();
        let verb_w = text_w(VERB_CHARS, m.mono * 0.6);
        // Hanging: the glyph's 12·r column starts where the AGENT label
        // and its time end (column − 20·r, C4), centred at column − 14·r;
        // the fill stops there too (it never paints under the label).
        // On their way between the two (`hang_k`), between them.
        let k = lay.hang_k;
        let (out, pad, verb) = ((k * m.hang).round(), (k * (m.hang - m.lead)).round(), ((1. - k) * m.lead).round());
        Geo { out, pad, right: gap, verb_w, gap, verb, target: verb + verb_w + gap, fold: (12. * m.r).round() }
    }
}

/// A row's target (cards4/proposal.md §2): mono, the scanned part (a
/// path's file, a command) in `prose_t`, a path's folder in `dim_t`,
/// ellipsised. `quiet` (cancelled, denied): all of it `dim_t`. A path's
/// folder gives way first; its file's name then middle-ellipsizes (its
/// start and extension stay), never cut. Its flex basis is the name's own
/// width, so the row's facts give way before the name ([`target_line`]).
fn target_el(card: &Card, arg: &SharedString, quiet: bool, id: u64, p: &Palette, m: &Metrics) -> Div {
    let text = if card.target.is_empty() { arg.clone() } else { card.target.clone() };
    let lit = if quiet { p.dim_t } else { p.prose_t };
    match &card.path {
        Some((dir, base)) => {
            // The name's width, a little over (the facts wrap away early
            // rather than squeeze it).
            let name_w = (crate::blocks::cols(base) as f32 * m.mono * 0.62).ceil() + 2.;
            let t = format!("tool-file {id} {base}");
            h_flex()
                .flex_grow(1.)
                .flex_shrink(1.)
                .flex_basis(px(name_w))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .debug_selector(move || format!("tool-target {id}"))
                .child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(p.dim_t)
                        .debug_selector(move || format!("tool-dir {id}"))
                        .child(dir.clone()),
                )
                // As wide as the name, the target at most: the folder
                // shrinks alone while it can.
                .child(
                    div()
                        .flex_none()
                        .max_w_full()
                        .overflow_hidden()
                        .text_ellipsis_middle()
                        .text_color(lit)
                        .debug_selector(move || t)
                        .child(base.clone()),
                )
        }
        None => div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(lit).child(text),
    }
}

/// A path row's target and its facts on one line that wraps: the facts
/// come after the target only while its file's name fits whole beside
/// them; else they take a second line, out of sight, and the name has the
/// row (a 320 px pane: `Edit token_store.rs`, not `token_store.r +3 −1`).
fn target_line(target: Div, facts: Div, line: f32, gap: f32, id: u64) -> Div {
    h_flex()
        .flex_1()
        .min_w_0()
        .h(px(line))
        .flex_wrap()
        .gap_x(px(gap))
        .items_center()
        .overflow_hidden()
        .debug_selector(move || format!("tool-line {id}"))
        .child(target)
        .child(facts)
}

/// A running call's clock (the TUI's `running 1.2s`): whole seconds since
/// it started, as of the frame's grid second, ticking on the second while motion
/// is on.
pub(crate) fn clock(fx: &Motion, started: Option<Instant>) -> Option<SharedString> {
    // As of the frame's grid second (Shady #17 M2), as every clock.
    let at = fx.clock_now();
    let ms = at.saturating_duration_since(started?).as_millis() as u64;
    if fx.on() {
        fx.tick_clock(at + Duration::from_millis(1000 - ms % 1000));
    }
    Some(subagents::fmt_whole(ms / 1000).into())
}

impl AppView {
    /// A folded run's summary row (cards4/proposal.md §3): the run's first
    /// row, tool-row shaped. Its last outcome's life glyph, "20 tool calls"
    /// (Sans 600 `prose_t`), the tally in Sans `dim` ("Run 6 · Read 4 ·
    /// Search 3", what fits, then "+N more"), the counts, the words of what
    /// did not end ok in their colour ("1 failed"), the chevron. Opened, a
    /// 1 px `line` rule under it parts it from the rows.
    fn render_summary(&self, d: Disp, open: bool, p: &Palette, m: &Metrics, cx: &Context<Self>) -> AnyElement {
        let id = d.id;
        let Some(s) = self.panes.cur.summaries.get(id) else { return div().into_any_element() };
        let lay = self.panes.cur.layout;
        let g = crate::cards::Geo::of(m);
        let tight = lay.tight;
        // The tally names what fits (Sans 12, measured without layout at a
        // mono cell's width: a little room to spare), then `+N more`.
        let cw = m.meta * 0.6;
        let calls = format!("{} tool call{}", s.tools, if s.tools == 1 { "" } else { "s" });
        // The agent's title, at most half the column (it ellipsizes).
        let label_w = match &s.title {
            Some(t) => text_w(t.chars().count(), m.label * 0.55).min(lay.column / 2.),
            None => text_w(calls.chars().count(), m.label * 0.6),
        };
        // The card's edges and inset, the row's padding, the glyph, the
        // label, the trail and the chevron, the four gaps between them.
        let trail = s.trail_w(m.meta, tight);
        let avail = lay.column - 2. * (g.edge + g.inset) - 2. * g.pad - g.glyph - label_w - trail - g.chevron - 4. * g.gap;
        // `+N more` and the dot before it.
        let more_w = text_w(5, cw) + 3. + 12.;
        let names: Vec<SharedString> = s.tally.iter().map(|t| crate::cards::verb(&t.name)).collect();
        let fit_tally: Vec<Tally> = s.tally.iter().zip(&names).map(|(t, n)| Tally { name_chars: n.chars().count(), ..t.clone() }).collect();
        let all = tally_fit(&fit_tally, avail - 1., cw, 15.) == fit_tally.len();
        let named = if all { fit_tally.len() } else { tally_fit(&fit_tally, avail - 1. - more_w, cw, 15.) }.min(if tight { 3 } else { 5 });
        let mut tally = h_flex()
            .id(("tally", id))
            .debug_selector(|| format!("tally {id}"))
            .flex_1()
            .min_w_0()
            .gap(px(6.))
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(fonts::SANS)
            .text_px(m.meta);
        for (i, (t, name)) in s.tally[..named].iter().zip(&names).enumerate() {
            if i > 0 {
                tally = tally.child(crate::cards::dot(p));
            }
            tally = tally.child(
                h_flex()
                    .when(i + 1 == named, |d| d.debug_selector(|| format!("tally {id} last")))
                    .flex_none()
                    .gap(px(4.))
                    .child(div().text_color(p.dim_t).child(name.clone()))
                    .child(div().text_color(p.prose_t).child(t.n_text.clone())),
            );
        }
        let tip = (named < s.tally.len()).then(|| {
            let tip = s.tip.clone();
            move |window: &mut Window, cx: &mut App| crate::overlay::tip(tip.clone(), window, cx)
        });
        let (tally_tip, row_tip) = if named > 0 { (tip, None) } else { (None, tip) };
        if let Some(tip) = tally_tip {
            let rest: usize = s.tally[named..].iter().map(|t| t.n).sum();
            tally = tally
                .child(crate::cards::dot(p))
                .child(
                    div()
                        .debug_selector(|| format!("tally {id} more"))
                        .flex_none()
                        .text_color(p.dim_t)
                        .child(format!("{} more", plus(rest))),
                )
                .tooltip(tip);
        }
        let (last, _) = s.last;
        let life = match last {
            Glyph::Failed => glyph::Life::Failed,
            Glyph::Cancelled => glyph::Life::Cancelled,
            Glyph::Limit => glyph::Life::Limit,
            _ => glyph::Life::Done,
        };
        let fx = &self.fx;
        let mut parts: Vec<AnyElement> = Vec::new();
        if let Some(c) = s.counts.element(p) {
            parts.push(c.into_any_element());
        }
        if !tight {
            for (text, family) in &s.outcome {
                let color = if *family == Family::Err { p.err } else { p.dim_t };
                parts.push(div().font_weight(FontWeight::SEMIBOLD).text_color(color).child(text.clone()).into_any_element());
            }
        }
        let row = h_flex()
            .id(("run", id))
            .debug_selector(move || format!("run-summary {id}"))
            .h(px(g.row))
            .px(px(g.pad))
            .gap(px(g.gap))
            .items_center()
            .font_family(fonts::MONO)
            .text_px(m.mono)
            .cursor_pointer()
            .hover(|d| d.bg(p.hover))
            .child(glyph::life(life, glyph::life_ink(life, p.card_d, p)))
            .child(match &s.title {
                // The agent's title (blocks.md §1.3), the count in the tally's tip.
                Some(t) => div()
                    .flex_shrink(1.)
                    .max_w(px(label_w))
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(fonts::SANS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_px(m.label)
                    .text_color(p.prose_t)
                    .debug_selector(move || format!("run-title {id}"))
                    .child(t.clone())
                    .into_any_element(),
                // `N tool calls` rolls while the run grows (polish.md P5).
                None => {
                    motion::roll_count(self.fx.col(self.panes.cur.id), motion::key(motion::ROLL_CALLS, id), s.tools, calls.into(), |t| {
                        div()
                            .flex_none()
                            .font_family(fonts::SANS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_px(m.label)
                            .text_color(p.prose_t)
                            .child(t)
                    })
                }
            })
            .child(tally)
            .child(crate::cards::facts(parts, p, m))
            .child(
                div()
                    .flex_none()
                    .w(px(g.chevron))
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(glyph::turn(fx.col(self.panes.cur.id).turn(id | RUN_KEY, open), p.dim_t)),
            )
            .when_some(row_tip, |d, tip| d.tooltip(tip));
        let _ = cx;
        if open {
            return crate::cards::opened(row, id, p, g).into_any_element();
        }
        row.into_any_element()
    }

    /// A thinking or tool row in a run (cards4/proposal.md §3, §4): one
    /// quiet 34 px row in its run's card. Its life glyph, the verb in Sans,
    /// the target in mono (the scanned part `prose_t`, a path's folder
    /// `dim_t`), the facts right in Sans 12 `dim` (`+add −del`, counts, the
    /// duration or the live clock in `acc`, the receipt), the state word in
    /// its colour, the chevron when it has a body. Hover fills it with
    /// `hover` (colour only). A failed row rests on a 7.5 % `err` wash, a
    /// waiting one on 6 % of the accent. Opened, its body is a `well_d`
    /// under the verb. A call that started a worker is its spawn row.
    #[allow(clippy::too_many_arguments)]
    fn render_work(&self, item: &Item, row: &RowCache, id: u64, p: &Palette, m: &Metrics, lay: Layout, cx: &Context<Self>) -> AnyElement {
        let think = item.kind == ItemKind::Thinking;
        // Drawn running once, it never steps back to Queued.
        let st = match status(item) {
            Some(ToolStatus::Queued) if self.ran.borrow().contains(&id) => Some(ToolStatus::Running),
            s => s,
        };
        let geo = Geo::of(lay, m);
        let g = crate::cards::Geo::of(m);
        let call_id = item.tool.as_ref().map(|t| t.tool_id.as_str());
        let starts = item.tool.as_ref().filter(|t| subagents::starts_worker(&t.name));
        let worker = starts.and_then(|t| {
            let subs = self.model.lives[self.panes.cur.live].shown_subagents();
            subs.view(&t.tool_id).map(|v| (v, subs.current(&t.tool_id)))
        });
        if let Some((v, current)) = worker {
            return self.render_spawn(row, v, current, id, geo, p, m, lay, cx);
        }
        if starts.is_some() {
            // No worker yet: when it comes, its spawn row's bar draws.
            self.fx.col(self.panes.cur.id).change(Tag::Block, id, 0, self.fx.col(self.panes.cur.id).birth(id));
        }
        let has_body = if think { !row.text.is_empty() } else { row.preview.is_some() || !row.result.is_empty() };
        let open = has_body && self.is_open(item, row);
        let target = if think {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(fonts::SANS)
                .text_px(m.think)
                .text_color(p.think)
                .child(row.arg.clone())
        } else {
            target_el(&row.card, &row.arg, matches!(st, Some(ToolStatus::Cancelled | ToolStatus::Denied)), id, p, m)
        };
        // The call a prompt waits on ([`waiting_call`]).
        let waiting = !think
            && self.panes.cur.waiting_on == Some(item.id)
            && !self.model.lives[self.panes.cur.live].shown().pending_prompts().is_empty();
        let fx = &self.fx;
        let shown = self.model.lives[self.panes.cur.live].shown();
        let running_think = think && shown.busy() && shown.items().last().map(|i| i.id) == Some(id);
        let state = if think {
            if running_think { (Glyph::Run, None, Family::Run) } else { (Glyph::Ok, None, Family::Ok) }
        } else {
            glyph::call(st, waiting, self.model.lives[self.panes.cur.live].active.is_none())
        };
        let life = if think {
            if running_think { glyph::Life::Run } else { glyph::Life::Done }
        } else {
            glyph::life_of_call(st, waiting, self.model.lives[self.panes.cur.live].active.is_none())
        };
        // Its output being written: the lattice's streaming pose.
        let life = if life == glyph::Life::Run && !think && !row.result.is_empty() { glyph::Life::Stream } else { life };
        if life.live() {
            self.ran.borrow_mut().insert(id);
        }
        let change = fx.col(self.panes.cur.id).change(Tag::Outcome, id, life.code(), None);
        let wait =
            fx.col(self.panes.cur.id).change(Tag::Waiting, id, u64::from(waiting), None).filter(|w| waiting && w.1 == 0).map(|w| w.0);
        // The row's wash (§1): the state colour mixed into the card, one
        // quad behind the row. A failure flushes at 18 % and calms to 7.5 %
        // (§8 #6); a wait at 16 % and holds at 6 % (#7). Off: at rest.
        let wash = match life {
            glyph::Life::Failed => {
                let k = fx.decay(change.filter(|c| glyph::Life::was_live(c.1)).map(|c| c.0), Duration::ZERO, motion::GLOW);
                Some(motion::mix(p.wash_fail, p.wash_fail_pk, k))
            }
            glyph::Life::Wait => {
                let k = fx.decay(wait, Duration::ZERO, motion::SIG);
                Some(motion::mix(p.wash_wait, p.wash_wait_pk, k))
            }
            _ => None,
        };
        let ground = wash.unwrap_or(p.card_d);
        let status_glyph = glyph::indicator(fx, life, ground, p, change, wait);
        // Thinking done says nothing: only its running mark.
        let status_glyph = (!think || running_think).then_some(status_glyph);
        let running = life.live() && !think;
        let receipt = self.row_receipt(call_id).filter(|_| !think);
        // A gated call runs from its answer (the wait is the receipt's).
        let start = crate::authorize::run_start(row.started, receipt);
        let ran = running.then(|| clock(fx, start)).flatten();
        // Finished while watched: how long it took (§3; history has none).
        if let (Some((at, prev)), false, Some(s)) = (change, running, start)
            && glyph::Life::was_live(prev)
        {
            self.took.borrow_mut().entry(id).or_insert_with(|| at.saturating_duration_since(s));
        }
        let took = (!running && !think).then(|| self.took.borrow().get(&id).copied()).flatten();
        // The state word (§4): a failed command its exit code.
        let exit = (st == Some(ToolStatus::Failed)).then(|| crate::cards::Exit::of(&row.name, &row.result)).flatten();
        let word: Option<SharedString> = match exit {
            Some(e) => Some(e.word()),
            None => crate::authorize::status_beside(
                state.1.filter(|w| *w != "stopped at output limit"),
                receipt.map(|r| r.answer_for(st, self.call_applied(&r.call))),
            )
            .map(SharedString::new_static),
        }
        .filter(|_| !think && !(lay.tight && receipt.is_some()));
        let limit = st == Some(ToolStatus::Unknown);
        let word_color = match life {
            glyph::Life::Failed | glyph::Life::Denied => p.err,
            glyph::Life::Wait => p.acc,
            _ => p.dim_t,
        };
        let mut parts: Vec<AnyElement> = Vec::new();
        if let Some(c) = row.counts.element(p) {
            parts.push(c.into_any_element());
        }
        // A read's range or an edit's `replaced 9 → 4 lines` (the well's
        // header before D), else the row's own facts.
        if let Some(f) = row.card.head.as_ref().and_then(|h| h.1.clone()).or_else(|| row.card.facts.clone()) {
            parts.push(div().text_color(p.dim_t).child(f).into_any_element());
        }
        if limit {
            parts.push(div().text_color(p.dim_t).child("stopped at output limit").into_any_element());
        }
        if let Some(c) = ran {
            parts.push(div().text_color(p.acc).child(c).into_any_element());
        } else if let Some(t) = took {
            // The live clock rolls over into the final duration (§8 #5).
            let from = change
                .filter(|c| glyph::Life::was_live(c.1))
                .zip(start)
                .map(|((at, _), s)| (at, subagents::fmt_whole(at.saturating_duration_since(s).as_secs())));
            parts.push(clock_roll(fx, from, crate::cards::took(t), p, m));
        }
        if let Some(s) = row.card.server.clone() {
            parts.push(div().text_color(p.dim_t).child(s).into_any_element());
        }
        let after = !parts.is_empty();
        let receipt_el = receipt.and_then(|_| self.receipt_el(call_id, st, after, running, p));
        // A path's facts may give way to its name ([`target_line`]); its
        // receipt never does.
        let path = !think && row.card.path.is_some() && after;
        let (facts, receipt_el) = if path {
            (crate::cards::facts(parts, p, m), receipt_el)
        } else {
            (crate::cards::facts(parts, p, m).children(receipt_el), None)
        };
        let facts = facts.debug_selector(move || format!("tool-facts {id}"));
        // "Exit N" slides in from +6 as the cross writes on (§8 #6).
        let word_el = word.map(|w| {
            let since = change.filter(|c| glyph::Life::was_live(c.1) || c.1 == glyph::Life::Queued.code()).map(|c| c.0);
            let k = fx.shot(since, Duration::ZERO, motion::QUICK, &ease::OUT, Mk::Fade);
            let x = (1. - fx.shot(since, Duration::ZERO, motion::FLICK, &ease::OUT, Mk::Spatial)) * 6.;
            div()
                .debug_selector(move || format!("tool-word {id}"))
                .flex_none()
                .relative()
                .left(px(x))
                .font_family(fonts::SANS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_px(m.meta)
                .whitespace_nowrap()
                .text_color(motion::mix(ground, word_color, k))
                .child(w)
        });
        let fold = has_body.then(|| glyph::turn(fx.col(self.panes.cur.id).turn(id, open), p.dim_t));
        let verb_text = if think {
            SharedString::new_static("Think")
        } else if matches!(item.card.as_deref(), Some(synaps_client::cards::CardRow::Close(_))) {
            SharedString::new_static("Close")
        } else {
            crate::cards::verb(&row.name)
        };
        // The verb column (§3): 56 px, wider for a longer word; the verb is
        // never cut, the target gives way (the finish gate of #23, item 11).
        let verb = div()
            .flex_none()
            .min_w(px(g.verb))
            .whitespace_nowrap()
            .font_family(fonts::SANS)
            .font_weight(FontWeight::SEMIBOLD)
            .text_px(m.label)
            .text_color(p.prose_t)
            .id(("verb", id))
            .when_some(row.card.full.clone(), |d, full| d.tooltip(move |window, cx| crate::overlay::tip(full.clone(), window, cx)))
            .child(verb_text);
        let head = h_flex()
            .id(("row", id))
            .debug_selector(move || format!("tool-row {id}"))
            .relative()
            .h(px(g.row))
            .px(px(g.pad))
            .gap(px(g.gap))
            .items_center()
            .when_some(wash, |d, w| d.bg(w))
            .when(has_body && wash.is_none(), |d| d.cursor_pointer().hover(|d| d.bg(p.hover)))
            .when(has_body && wash.is_some(), |d| d.cursor_pointer())
            .child(div().flex_none().size(px(g.glyph)).children(status_glyph).debug_selector(move || format!("tool-status {id}")))
            .child(verb)
            .map(|d| if path { d.child(target_line(target, facts, m.mono_line, g.gap, id)) } else { d.child(target).child(facts) })
            .children(receipt_el.map(|r| div().flex_none().font_family(fonts::SANS).text_px(m.meta).whitespace_nowrap().child(r)))
            .children(word_el)
            .child(div().flex_none().w(px(g.chevron)).flex().items_center().justify_end().children(fold))
            .when(has_body, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle(id);
                    cx.notify();
                }))
            });
        // The body opens and closes under the row on `glide` (§8 #6, #12),
        // the header still; a body opened only while its output streamed
        // folds as the call succeeds (@ +180, #5).
        let finished_now = change.is_some_and(|c| c.0 >= fx.now() && glyph::Life::was_live(c.1));
        let delay = if !open && finished_now { Duration::from_millis(180) } else { Duration::ZERO };
        let (reveal, sink) =
            if has_body { fx.col(self.panes.cur.id).reveal(id, open, delay, life.code()) } else { (None, Default::default()) };
        // Output streaming in (§8 #4): each new line grows and lifts.
        let streamed = if (open || reveal.is_some()) && !think && row.preview.is_none() && row.card.numbered.is_none() {
            fx.col(self.panes.cur.id).streamed(id, row.result.lines().count())
        } else {
            Vec::new()
        };
        let body = (open || reveal.is_some()).then(|| {
            if think {
                // Prose, not code: no well, under the target.
                div()
                    .ml(px(g.indent + g.verb + g.gap))
                    .pb(px((8. * m.r).round()))
                    .pr(px(g.pad))
                    .font_family(fonts::SANS)
                    .text_px(m.think)
                    .line_height(px(m.think_line))
                    .text_color(p.think)
                    .child(row.text.clone())
                    .into_any_element()
            } else {
                let failed = matches!(st, Some(ToolStatus::Failed | ToolStatus::Denied));
                well(row, failed, id, &streamed, p, m).into_any_element()
            }
        });
        let body = body.map(|b| revealed(b, reveal, sink).debug_selector(move || format!("reveal {id}")));
        v_flex()
            .relative()
            .w_full()
            .font_family(fonts::MONO)
            .text_px(m.mono)
            .line_height(px(m.mono_line))
            .child(head)
            .children(body)
            .into_any_element()
    }

    /// A worker's spawn row (polish.md §2.4): one row on its run's card
    /// (`card_d`, as a tool's row: Haseeb; the worker's own fill is its
    /// chip's), the worker's state
    /// in the gutter (not the call's), `spawn`, its name and task, its facts
    /// (the clock ticks while it runs here: [`crate::tray::ticked`]) and
    /// OPEN ↗; under it while it runs its step (↳, `sub_status_t`), once
    /// it ended its result (a failure's reason in `err`), at the target
    /// column with the `sub_rule` bar. Not attached (a cached tab opening,
    /// a detached session): as last seen. On the session's thread a
    /// background worker's row opens it (ctrl, middle or double click: in a
    /// tab; right click: its menu). `current`: no later call has its `sa_N`
    /// (a cold-started daemon numbers from 1 again); one that has is
    /// another worker's handle now, and the row opens nothing.
    #[allow(clippy::too_many_arguments)]
    fn render_spawn(
        &self,
        row: &RowCache,
        v: &subagents::View,
        current: bool,
        key: u64,
        geo: Geo,
        p: &Palette,
        m: &Metrics,
        lay: Layout,
        cx: &Context<Self>,
    ) -> AnyElement {
        let fx = &self.fx;
        let session = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.as_str());
        let pending = session.zip(v.id).is_some_and(|(s, id)| self.model.lives[self.panes.cur.live].cancels.pending_id(s, id));
        let live = subagents::Liveness::of(v.status, session.is_some(), true).cancelling(pending);
        let (g, _, word, family) = glyph::worker(live);
        let (word_color, mark) = glyph::ink(family, Ground::Run, p);
        let ended = v.status.is_some_and(subagents::Status::terminal);
        let running = matches!(live, subagents::Liveness::Running | subagents::Liveness::Cancelling);
        let (step, result) = if ended { (None, v.line.clone()) } else { (v.line.clone(), None) };
        // A background worker of the session on screen: it opens.
        let open = v.id.filter(|_| v.watchable && current && self.model.lives[self.panes.cur.live].watch.focus().is_none());
        let who = open.map(|id| crate::wtabs::Who::new(id, v.name.clone(), v.call.clone()));
        let _ = (g, mark);
        let d = crate::cards::Geo::of(m);
        let life = glyph::life_of_worker(live);
        let steady = glyph::life_of_worker(glyph::steady(live, v.demoted));
        let status =
            glyph::indicator(fx, life, p.card_d, p, fx.col(self.panes.cur.id).change(Tag::BlockGlyph, key, steady.code(), None), None);
        // Its task: the call's argument after the worker's label.
        let task = row.arg.split_once(" · ").map_or_else(|| row.arg.clone(), |(_, t)| SharedString::from(tilde(t, true)));
        let target = h_flex()
            .flex_1()
            .min_w_0()
            .gap(px(geo.gap))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(
                div()
                    .flex_none()
                    .max_w(px(m.mono * 0.6 * 24.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(fonts::SANS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_px(m.label)
                    .text_color(p.prose_t)
                    .child(v.name.clone()),
            )
            // The task quiet, the name in its hue (noir; nord's name and
            // `arg` were one blue).
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(p.dim_t).child(task));
        // The state's word only when it is not going well (C10).
        let warn = matches!(family, Family::Err | Family::Warn);
        let tick = if running { &v.tick_bare } else { &None };
        let meta = h_flex()
            .flex_none()
            .ml(px(geo.gap))
            .gap(px(8.))
            .items_center()
            .text_px(m.meta)
            .whitespace_nowrap()
            .when(!v.facts_bare.is_empty(), |d| d.child(spawn_facts(fx, &v.facts_bare, tick, p)))
            .when(warn && !lay.tight, |d| d.child(div().text_color(word_color).child(word)));
        let button = who.clone().map(|who| {
            let who2 = who.clone();
            h_flex()
                .id(("wblock-open", who.id as usize))
                .debug_selector(move || format!("spawn-open {key}"))
                .flex_none()
                .ml(px(geo.gap))
                .h(px((22. * m.r).round()))
                .px(px(8.))
                .gap(px(6.))
                .items_center()
                .group("wopen")
                .border_1()
                .border_color(p.line)
                .bg(p.card_d)
                .hover(|d| d.bg(p.hover))
                // Hovered, it lights: its outline to `frame`, its word and
                // arrow to `prose_t` (both floored on the hovered row).
                .child(div().font_family(fonts::SANS).font_weight(FontWeight::SEMIBOLD).text_px(m.meta).text_color(p.prose_t).child("Open"))
                .child(svg().path(Glyph::Open.sprite().unwrap_or_default()).flex_none().size(px(Glyph::Open.size())).text_color(p.prose_t))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.worker_click(&who, true, ev, window, cx);
                }))
                .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.worker_click(&who2, true, ev, window, cx);
                }))
        });
        let head = h_flex()
            .h(px(d.row))
            .px(px(d.pad))
            .gap(px(d.gap))
            .items_center()
            .debug_selector(move || format!("spawn-row {key}"))
            .child(div().flex_none().size(px(d.glyph)).child(status))
            .child(
                div()
                    .flex_none()
                    .w(px(d.verb))
                    .font_family(fonts::SANS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_px(m.label)
                    .text_color(p.prose_t)
                    .child("Spawn"),
            )
            .child(target)
            .child(meta.font_family(fonts::SANS))
            .children(button)
            .child(div().flex_none().w(px(d.chevron)));
        // The worker came under the call while watched (its call was drawn
        // without it: `render_work`): its bar draws.
        let came = fx.col(self.panes.cur.id).change(Tag::Block, key, 1, fx.col(self.panes.cur.id).birth(key)).map(|(at, _)| at);
        // Under the target (§3): no bar in a card (C5).
        let _ = came;
        let line = |child: AnyElement| div().relative().ml(px(d.indent + d.verb + d.gap)).mr(px(d.pad)).mt(px(-6.)).mb(px(8.)).child(child);
        let under = if let Some((text, err)) = result {
            Some(line(
                div()
                    .font_family(fonts::SANS)
                    .text_px(m.think)
                    .line_height(px(m.think_line))
                    .whitespace_normal()
                    .text_color(if err { p.err } else { p.prose_t })
                    .child(text)
                    .into_any_element(),
            ))
        } else {
            step.map(|(text, _)| {
                line(
                    h_flex()
                        .h(px((18. * m.r).round()))
                        .gap(px(8.))
                        .items_center()
                        .font_family(fonts::SANS)
                        .text_px(m.meta)
                        .whitespace_nowrap()
                        // A changed step swaps in place: under a live fan-out a
                        // roll is an app frame at 30 Hz for every change
                        // (polish.md P5, gate 4).
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(p.dim_t).child(text))
                        .into_any_element(),
                )
            })
        };
        // The whole of it is the hover's: its box reaches into the gutter.
        let block = v_flex().relative().font_family(fonts::MONO).text_px(m.mono).line_height(px(m.mono_line)).child(head).children(under);
        let Some(who) = who else {
            // Its handle names a later worker now: nothing of that one's here.
            if let Some(id) = v.id.filter(|_| v.watchable && !current && self.model.lives[self.panes.cur.live].watch.focus().is_none()) {
                let tip = SharedString::new_static("an earlier worker: a restarted daemon has given its handle to another one since");
                return block
                    .id(("wblock-old", id as usize))
                    .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx))
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
            .hover(|d| d.bg(p.hover))
            .when(self.menu.is_none(), |d| d.tooltip(crate::wtabs::card_tip(cx.weak_entity(), who.clone(), tip)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who, true, ev, window, cx)))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| this.worker_click(&who2, true, ev, window, cx)))
            .into_any_element()
    }
}

/// A slash command's answer shown folded to this many lines, when that
/// hides at least two (the `… more` line is a line itself: a 9-line
/// answer shows whole).
pub(crate) const OUTPUT_LINES: usize = 8;

/// Whether an answer of `n` lines is shown folded.
pub(crate) fn output_folds(n: usize) -> bool {
    n >= OUTPUT_LINES + 2
}

impl AppView {
    /// A slash command's answer ([`ItemKind::Output`], this client's row):
    /// whole, as the TUI prints it into its scrollback, in the house well
    /// (`well` in a 1 px `hair_w` frame, mono `res`), its command over it;
    /// past [`OUTPUT_LINES`] lines folded, a click on its head or its
    /// `… more` line opens and folds it.
    fn render_output(&self, item: &Item, row: &RowCache, p: &Palette, m: &Metrics, cx: &Context<Self>) -> AnyElement {
        let inset = (12. * m.r).round();
        let pad = (6. * m.r).round();
        let id = item.id;
        let (name, body) = row.text.split_once('\n').unwrap_or((&row.text, ""));
        let lines: Vec<&str> = body.lines().collect();
        let long = output_folds(lines.len());
        let open = !long || self.panes.cur.toggled.contains(&id);
        let shown = if open { lines.len() } else { OUTPUT_LINES };
        let toggle = |d: Stateful<Div>, cx: &Context<Self>| {
            d.cursor_pointer().hover(|d| d.bg(p.tool_out)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.toggle(id);
                cx.notify();
            }))
        };
        let count = format!(
            "{} line{}{}",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" },
            if long { if open { " · fold" } else { " · show all" } } else { "" }
        );
        let head = h_flex()
            .id(("output-head", id as usize))
            .px(px(inset))
            .py(px((4. * m.r).round()))
            .gap(px(8.))
            .border_b_1()
            .border_color(p.hair_w)
            .text_px(m.meta)
            .text_color(p.dim_o)
            .whitespace_nowrap()
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(SharedString::from(name.to_string())))
            .child(div().flex_none().child(count));
        let head = if long { toggle(head, cx) } else { head };
        let mut col = v_flex().px(px(inset)).py(px(pad)).text_color(p.res);
        for l in &lines[..shown] {
            // A line's own indent kept (the daemon aligns its columns).
            col = col.child(div().w_full().min_h(px(m.mono_line)).whitespace_normal().child(SharedString::from(l.to_string())));
        }
        let more = (long && !open).then(|| {
            toggle(div().id(("output-more", id as usize)), cx)
                .px(px(inset))
                .pb(px(pad))
                .text_color(p.dim_o)
                .child(format!("… {} more lines", lines.len() - shown))
        });
        v_flex()
            .debug_selector(move || format!("output {id}"))
            .w_full()
            .bg(p.well)
            .border_1()
            .border_color(p.hair_w)
            .font_family(fonts::MONO)
            .text_px(m.mono)
            .line_height(px(m.mono_line))
            .child(head)
            .child(col)
            .children(more)
            .into_any_element()
    }
}

/// An opened call's body (cards4/proposal.md §3): under its row on
/// `well_d`, square, no frame, indented to the verb column (36 px from
/// the row's edge), to the card's inner edge. An edit's lines full width on
/// their washes, the sign hanging in an 18 px column; a read's with the
/// file's own numbers right-aligned in a 30 px column; any other output as
/// it came (a failure's in `err_o`). The cut says "N more lines" in Sans 12
/// `dim`. The path and the range are the row's (its target and facts).
/// A body as it opens or closes ([`Motion::reveal`]): clipped to `shown`
/// of its height (none: all of it), measured into `sink`.
pub(crate) fn revealed(body: AnyElement, shown: Option<f32>, sink: std::rc::Rc<std::cell::Cell<f32>>) -> Div {
    let h = sink.get();
    div()
        .w_full()
        .flex()
        .flex_col()
        // Whole pixels: a fractional height would put every row under it
        // off the pixel grid (and the following list's end off its bottom).
        // All of it (a resize's first frame holding the height it had):
        // its own measured height, as laid out unclipped.
        .when_some(shown, |d, k| {
            d.overflow_hidden().h(px(if h.is_nan() {
                0.
            } else if k == 1. {
                h
            } else {
                (h * k).round()
            }))
        })
        .child(div().w_full().flex_shrink_0().child(body))
        .on_children_prepainted(move |b, _, _| {
            if let Some(b) = b.first() {
                sink.set(f32::from(b.size.height));
            }
        })
}

fn well(row: &RowCache, failed: bool, id: u64, streamed: &[(usize, f32, f32, Rc<Cell<f32>>)], p: &Palette, m: &Metrics) -> Div {
    let g = crate::cards::Geo::of(m);
    let inset = (10. * m.r).round();
    let cw = m.mono * 0.6;
    let col = v_flex()
        .debug_selector(move || format!("well {id}"))
        .ml(px(g.indent))
        .mr(px(g.pad))
        .mt(px(1.))
        .mb(px((8. * m.r).round()))
        .overflow_hidden()
        .bg(p.well_d)
        .text_color(p.res)
        .text_px(m.mono)
        .line_height(px((19. * m.r).round()));
    let pad = (6. * m.r).round();
    let more_el = |t: SharedString| div().px(px(inset)).pt(px(2.)).font_family(fonts::SANS).text_px(m.meta).text_color(p.dim_o).child(t);
    let more = |n: usize| more_el(format!("{n} more lines").into());
    match (&row.preview, &row.card.numbered) {
        (Some(pv), _) => {
            let mut lines = v_flex().py(px(pad));
            for l in &pv.lines {
                let (sign, fg, sign_fg, bg) = match l.kind {
                    LineKind::Add => ("+", p.ink, p.ok_o, Some(p.add_bg)),
                    LineKind::Del => ("−", p.ink, p.err_o, Some(p.del_bg)),
                    LineKind::Context => (" ", p.res, p.dim_o, None),
                    LineKind::Note => (" ", p.dim_o, p.dim_o, None),
                };
                lines = lines.child(
                    h_flex()
                        .w_full()
                        .pr(px(inset))
                        .items_start()
                        .when_some(bg, |d, bg| d.bg(bg))
                        .child(div().w(px(18.)).flex_none().flex().justify_center().text_color(sign_fg).child(sign))
                        .child(div().w(px(text_w(l.indent as usize, cw))).flex_none())
                        // `SharedString` from the line's `Arc<str>`: no copy per frame.
                        .child(div().flex_1().min_w_0().text_color(fg).whitespace_normal().child(SharedString::from(&l.text))),
                );
            }
            col.child(lines.when(pv.more > 0, |d| d.child(more(pv.more))))
        }
        (None, Some(numbered)) if !failed => {
            let mut lines = v_flex().py(px(pad));
            for (n, indent, text) in numbered.iter() {
                lines = lines.child(
                    h_flex()
                        .w_full()
                        .pr(px(inset))
                        .items_start()
                        .child(div().w(px(30.)).flex_none().flex().justify_end().text_color(p.dim_o).child(n.clone()))
                        .child(div().w(px(text_w(2 + *indent as usize, cw))).flex_none())
                        .child(div().flex_1().min_w_0().whitespace_normal().child(text.clone())),
                );
            }
            let cut = row.card.more.clone().map(|t| SharedString::from(t.trim_start_matches("… ").to_string()));
            col.child(lines.children(cut.map(more_el)))
        }
        _ if !streamed.is_empty() => {
            // Streaming (§8 #4): line by line, a new one's clip growing
            // 0 → its line on `flick` and its text lifting from 70 %; text
            // never moves inside its block. A line that wraps grows to all
            // of it, as laid out a frame ago (one line's height until then):
            // it never pops to its wraps when it retires (#26 L6).
            let line = (19. * m.r).round();
            let lines = row.result.lines().enumerate().map(|(i, l)| {
                let text =
                    div().w_full().flex_shrink_0().child(SharedString::from(if l.is_empty() { " ".to_string() } else { l.to_string() }));
                match streamed.iter().find(|s| s.0 == i) {
                    Some((_, clip, k, sink)) => {
                        let (h, sink) = (sink.get(), sink.clone());
                        let h = if h.is_nan() { line } else { h };
                        div()
                            .debug_selector(move || format!("stream-line {id} {i}"))
                            .w_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .h(px((h * clip).round()))
                            .child(motion::lift(text, *k))
                            .on_children_prepainted(move |b, _, _| {
                                if let Some(b) = b.first() {
                                    sink.set(f32::from(b.size.height));
                                }
                            })
                            .into_any_element()
                    }
                    None => text.into_any_element(),
                }
            });
            col.child(
                v_flex()
                    .px(px(inset))
                    .py(px(pad))
                    .whitespace_normal()
                    .text_color(if failed { p.err_o } else { p.res })
                    .children(lines)
                    .when(row.result_more > 0, |d| d.child(more(row.result_more))),
            )
        }
        _ => col.child(
            v_flex()
                .px(px(inset))
                .py(px(pad))
                .whitespace_normal()
                .text_color(if failed { p.err_o } else { p.res })
                .child(row.result.clone())
                .when(row.result_more > 0, |d| d.child(more(row.result_more))),
        ),
    }
}

/// A spawn row's facts (polish.md §2.4): running, its clock ticks; its
/// tool count changes in place (P5: under a live fan-out a roll is an app
/// frame at 30 Hz for every change, gate 4); ended, as reported.
fn spawn_facts(fx: &std::rc::Rc<Motion>, facts: &SharedString, tick: &Option<(SharedString, subagents::Tick)>, p: &Palette) -> AnyElement {
    div().text_color(p.dim_t).child(crate::tray::ticked(fx, facts, tick)).into_any_element()
}

/// A summary's chevron is keyed apart from its first row's own.
const RUN_KEY: u64 = 1 << 63;

/// A glyph's index in the drawing table: what [`Motion::change`] remembers.
pub(crate) fn glyph_code(g: Glyph) -> u64 {
    Glyph::ALL.iter().position(|x| *x == g).unwrap_or(0) as u64
}

/// A state glyph as it is now (motion.md §6, §7): `Run`, `Workers` and
/// `Opening` spin on the beat (polish.md P1); `Needs` breathes from `breath` (the prompt's arrival)
/// three times, then holds; a glyph that changed (`change`: when, and from
/// what) writes on, draws its quads or (`fade`) cross-fades. Off: [`glyph::draw`].
pub(crate) fn live_glyph(
    fx: &std::rc::Rc<Motion>,
    g: Glyph,
    mark: Hsla,
    ground: Hsla,
    change: Option<(Instant, u64)>,
    breath: Option<Instant>,
    fade: bool,
) -> AnyElement {
    match g {
        // The braille family spins (the beat layer paints it over the
        // cached frame).
        Glyph::Run | Glyph::Workers | Glyph::Opening => glyph::spinning(fx, g, mark, ground),
        Glyph::Needs => {
            let step = breath.map(|t| fx.beats_since(t)).filter(|s| *s < motion::BREATH_BEATS * motion::BREATHS);
            if let (Some(t), Some(_), true) = (breath, step, fx.layered()) {
                return glyph::on_beat(motion::Beat::Breath(t), mark, ground, fx);
            }
            // Only a breath in progress asks for the beat: a held one is still.
            glyph::breath(step.and_then(|s| fx.beat(1).map(|_| s)), mark, ground)
        }
        _ => match change.filter(|c| (c.1 as usize) < Glyph::ALL.len()) {
            Some((at, prev)) => {
                let from = Glyph::ALL[prev as usize];
                if fade {
                    let p = fx.shot(Some(at), Duration::ZERO, motion::QUICK, &ease::INOUT, Mk::Fade);
                    return glyph::change(Some((from, mark)), g, mark, ground, p, true);
                }
                let (old, p) = fx.write_on(Some(at));
                glyph::change_over(Some((from, fx.beat_at(at), mark)), g, mark, ground, old, p)
            }
            None => glyph::draw(g, mark),
        },
    }
}

/// The clock rolling over into the call's final duration (§8 #5): the old
/// whole seconds up 8 px and out (90 ms, `in`), the duration up from 8 px
/// (`snap` from +60, so they never overlap), in a one-line clip. `from`:
/// when it finished while watched, and the clock it showed then; none: the
/// duration at rest.
fn clock_roll(fx: &std::rc::Rc<Motion>, from: Option<(Instant, String)>, took: SharedString, p: &Palette, m: &Metrics) -> AnyElement {
    let (old, new) = fx.handover(from.as_ref().map(|f| f.0));
    if new >= 1. {
        return div().text_color(p.dim_t).child(took).into_any_element();
    }
    let line = px(m.mono_line);
    let travel = 8.;
    div()
        .relative()
        .h(line)
        .overflow_hidden()
        .child(div().invisible().child(took.clone()))
        .children(
            from.filter(|_| old < 1.).map(|(_, text)| {
                div().absolute().right_0().top(px(-travel * old)).text_color(motion::mix(p.acc, p.card_d, old)).child(text)
            }),
        )
        .child(div().absolute().right_0().top(px(travel * (1. - new))).text_color(motion::mix(p.card_d, p.dim_t, new)).child(took))
        .into_any_element()
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
        Counts, DAY_ID, Disp, Gap, Hue, KEEP, KEEP_BYTES, Keep, Kind, Layout, MAX_WAIT, Metrics, Reveal, RowCache, RunSummary, Summaries,
        Tally, TextRows, Who, diff, digits, plus, tally_fit, text_lines, text_w, turn_start,
    };
    use crate::files::{CallDelta, Ledger};
    use crate::subagents::Subagents;
    use crate::times::{Stamp, utc_in, zones};
    use chrono::FixedOffset;
    use synaps_client::Transcript;
    use synaps_client::protocol::SessionEvent;
    use synaps_client::transcript::Item;

    /// [`super::build`], each row's date read in `tz` (as `RowCache::time`
    /// reads it in the local zone).
    fn build_in(tz: &FixedOffset, items: &[Item], from: usize, busy: bool, open: &HashSet<u64>, subs: &Subagents) -> Vec<Disp> {
        let day = |i: usize| items[i].at.as_deref().and_then(|a| Stamp::in_zone(a, tz)).map(|t| t.day());
        super::build(items, &day, from, busy, open, subs)
    }

    /// [`build_in`] UTC.
    fn build(items: &[Item], from: usize, busy: bool, open: &HashSet<u64>, subs: &Subagents) -> Vec<Disp> {
        build_in(&FixedOffset::east_opt(0).unwrap(), items, from, busy, open, subs)
    }

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
                Kind::Event => "event",
                Kind::Summary { open: false, .. } => "fold",
                Kind::Summary { open: true, .. } => "unfold",
                Kind::Work => "work",
                Kind::Day => "day",
                Kind::Prompt => "prompt",
                Kind::Ask => "ask",
                Kind::Answer => "answer",
                Kind::Block { .. } => "block",
            })
            .collect()
    }

    /// Shady #20 N4: a command's answer is synaps's words, not the
    /// agent's: its own label, and the agent's next words labelled again.
    #[test]
    fn a_command_answer_is_labelled_its_own() {
        use synaps_client::protocol::{LlmEvent, StreamEvent};
        let mut t = transcript(&[
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": [{"type": "text", "text": "one"}]}),
        ]);
        t.output("budget", "Budget: none set");
        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: "two".into() }) });
        let rows = build(t.items(), 0, false, &HashSet::new(), &Subagents::default());
        assert_eq!(kinds(&rows), ["user", "prose", "note", "prose"]);
        assert_eq!(rows.iter().map(|d| d.who).collect::<Vec<_>>(), [Who::You, Who::Agent, Who::Synaps, Who::Agent]);
        // Rebuilt from the row after it: the same.
        let from = build(t.items(), 3, false, &HashSet::new(), &Subagents::default());
        assert_eq!(from.iter().map(|d| d.who).collect::<Vec<_>>(), [Who::Agent]);
    }

    /// Shady #22 H1: an event that comes while the agent's reply streams
    /// sits under the reply until the engine delivers it: one AGENT
    /// response, never two around the event; the same rows when its
    /// history lands. Delivered between rounds, the next round's words
    /// are labelled again, under it, as its history has them.
    #[test]
    fn an_event_mid_reply_never_splits_the_agents_words() {
        let ev = |v: serde_json::Value| synaps_client::protocol::decode_event(&v).unwrap();
        let text = |t: &str| ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": t}}));
        let start = || ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "response_start"}}));
        let form = "<event id=\"ev-1\" type=\"message\" severity=\"high\" source=\"cli\">deploy finished</event>";
        let frame = json!({"ev": "external", "event": {"id": "ev-1", "timestamp": "2031-02-03T04:05:06Z",
            "source": {"source_type": "cli", "name": "cli"}, "sender": {"id": "u", "name": "ci", "sender_type": "user"},
            "content": {"text": "deploy finished", "content_type": "message", "severity": "High"}, "expects_response": false}});
        let who = |t: &Transcript| {
            let rows = build(t.items(), 0, true, &HashSet::new(), &Subagents::default());
            (kinds(&rows), rows.iter().map(|d| d.who).collect::<Vec<_>>())
        };
        let mut t = Transcript::new();
        t.apply(&ev(json!({"ev": "turn_started", "trigger": "user", "user_text": "go"})));
        t.apply(&start());
        t.apply(&text("one "));
        t.apply(&ev(frame.clone()));
        t.apply(&text("two"));
        let live = who(&t);
        assert_eq!(live, (vec!["user", "prose", "event"], vec![Who::You, Who::Agent, Who::None]));
        t.apply_history(&[
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": [{"type": "text", "text": "one two"}]}),
            json!({"role": "user", "content": form}),
        ]);
        assert_eq!(who(&t), live, "the history the same");
        // Then a tool round, the event delivered after its result.
        let mut t = Transcript::new();
        t.apply(&ev(json!({"ev": "turn_started", "trigger": "user", "user_text": "go"})));
        t.apply(&start());
        t.apply(&text("checking "));
        t.apply(&ev(frame));
        t.apply(&text("now"));
        t.apply(&ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use_start", "tool_name": "bash", "tool_id": "t1"}})));
        t.apply(&ev(json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": "t1", "result": "ok"}})));
        t.apply(&ev(json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": form}})));
        t.apply(&start());
        t.apply(&text("done"));
        assert_eq!(
            who(&t),
            (vec!["user", "prose", "work", "event", "prose"], vec![Who::You, Who::Agent, Who::None, Who::None, Who::Agent])
        );
    }

    /// Shady #20 N6: folded only when that hides two lines or more (the
    /// `… more` line is a line itself).
    #[test]
    fn an_answer_folds_only_where_it_saves_lines() {
        assert!(!super::output_folds(8));
        assert!(!super::output_folds(9), "9 lines: whole");
        assert!(super::output_folds(10), "8 shown, 2 more");
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

    /// A compaction summary as the daemon writes it into the history
    /// (SynapsCLI `compaction_context_messages`).
    const SUMMARY: &str = "The conversation history before this point was compacted into the following summary:\n\n\
        <context-summary>\nwhat came before\n</context-summary>\n\n\
        Continue from where we left off. The summary and the system prompt contain all the context you need.";

    fn dated(m: serde_json::Value, at: String) -> serde_json::Value {
        let mut m = m;
        m["_synaps_at"] = json!(at);
        m
    }

    /// Day lines (`crate::times::Days`): one where the local date changes
    /// between dated rows; none before the first dated row (it sets the
    /// day), none for a legacy row. A session resumed the next day and the
    /// first row after a compaction dated later get theirs (its summary and
    /// acknowledgement are no rows: #14). The line takes the gap of
    /// the row under it, which sits a part below; a rebuild from any turn
    /// agrees with a full build. In zones west and east of UTC: a local day
    /// spans two UTC dates (no line), and a local midnight can fall inside
    /// one (a line).
    #[test]
    fn a_day_line_where_the_local_date_changes() {
        use crate::times::day_number;
        for tz in zones() {
            let d1 = |h, mi| utc_in(&tz, 2026, 11, 13, h, mi, 0);
            let d2 = |h, mi| utc_in(&tz, 2026, 11, 14, h, mi, 0);
            let d3 = |h, mi| utc_in(&tz, 2026, 11, 20, h, mi, 0);
            let text = |t: &str| json!({"role": "assistant", "content": [{"type": "text", "text": t}]});
            let m = vec![
                json!({"role": "user", "content": "from before message times"}),
                text("legacy"),
                dated(json!({"role": "user", "content": "first dated"}), d1(0, 30)),
                dated(text("late"), d1(23, 30)),
                dated(json!({"role": "user", "content": "resumed the next morning"}), d2(0, 30)),
                dated(text("morning"), d2(0, 31)),
                // The daemon's compaction (`compaction_context_messages`):
                // the summary in a user message, and an acknowledgement.
                dated(json!({"role": "user", "content": SUMMARY}), d3(8, 0)),
                dated(json!({"role": "assistant", "content": "I've loaded the conversation summary. Ready to continue."}), d3(8, 0)),
                dated(json!({"role": "user", "content": "after the compaction"}), d3(8, 1)),
                dated(text("same day"), d3(8, 2)),
            ];
            let utc = |at: &str| at[..10].to_string();
            assert_ne!(utc(&d1(0, 30)), utc(&d1(23, 30)), "{tz}: one local day, two UTC dates");
            assert_eq!(utc(&d1(23, 30)), utc(&d2(0, 30)), "{tz}: a local midnight inside one UTC date");
            let t = transcript(&m);
            assert!(t.items()[..2].iter().all(|i| i.at.is_none()), "legacy rows have no time");
            let rows = build_in(&tz, t.items(), 0, false, &HashSet::new(), &Subagents::default());
            // The summary and its acknowledgement are no rows (#14, the
            // daemon's rule: `synaps_client::transcript::is_compaction_summary`).
            assert_eq!(kinds(&rows), ["user", "prose", "user", "prose", "day", "user", "prose", "day", "user", "prose"], "{tz}");
            assert_eq!(t.items()[6].text, "after the compaction", "{tz}");
            let (day, under) = (rows[4], rows[5]);
            assert_eq!((day.item, day.id, day.who), (under.item, under.id | DAY_ID, Who::None));
            assert_eq!((day.gap, under.gap), (Gap::Turn, Gap::Part), "the line takes the turn's gap");
            assert_eq!(day.rev, day_number(chrono::NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()), "{tz}");
            assert_eq!(
                (rows[7].item, rows[7].gap, rows[8].gap),
                (rows[8].item, Gap::Turn, Gap::Part),
                "after the compaction, its turn's gap"
            );
            for k in 0..t.len() {
                let from = rows.partition_point(|d| d.item < turn_start(t.items(), k));
                let part = build_in(&tz, t.items(), turn_start(t.items(), k), false, &HashSet::new(), &Subagents::default());
                assert_eq!(&rows[from..], &part[..], "rebuilt from item {k}'s turn");
            }
            // Nothing dated: no line, whatever the history.
            let legacy: Vec<serde_json::Value> = m
                .into_iter()
                .map(|mut m| {
                    m.as_object_mut().unwrap().remove("_synaps_at");
                    m
                })
                .collect();
            let t = transcript(&legacy);
            assert!(build_in(&tz, t.items(), 0, false, &HashSet::new(), &Subagents::default()).iter().all(|d| d.kind != Kind::Day));
        }
    }

    /// Only messages move the day: a client note (dated by the event it
    /// came in, placed after its round's messages, which the daemon dates
    /// at the round's end) draws no line back and forth.
    #[test]
    fn a_note_never_moves_the_day() {
        for tz in zones() {
            let (d1, d2) = (|h, mi, s| utc_in(&tz, 2026, 11, 13, h, mi, s), |h, mi, s| utc_in(&tz, 2026, 11, 14, h, mi, s));
            let mut t = transcript(&[
                dated(json!({"role": "user", "content": "go"}), d1(23, 50, 0)),
                dated(json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}), d2(0, 0, 2)),
            ]);
            t.apply_at(&SessionEvent::SystemNotice { text: "a note from the round".into() }, &d1(23, 59, 58));
            let next = SessionEvent::TurnStarted { turn_baseline: 2, trigger: "user".into(), user_text: Some("more".into()) };
            t.apply_at(&next, &d2(0, 1, 0));
            assert!(t.items()[2].is_note() && t.items()[2].at.is_some());
            let rows = build_in(&tz, t.items(), 0, true, &HashSet::new(), &Subagents::default());
            assert_eq!(kinds(&rows), ["user", "day", "prose", "note", "user"], "{tz}");
        }
    }

    /// A run that crosses midnight is two, the day line between: its rows
    /// keep their order and the turn its one AGENT.
    #[test]
    fn a_run_across_midnight_splits_at_the_day_line() {
        for tz in zones() {
            let at = |d, h, mi| utc_in(&tz, 2026, 11, d, h, mi, 0);
            let mut m = vec![dated(json!({"role": "user", "content": "go"}), at(13, 23, 50))];
            for (id, at) in [("a", at(13, 23, 59)), ("b", at(14, 0, 1))] {
                let (call, result) = tool(id, "bash");
                m.push(dated(call, at.clone()));
                m.push(dated(result, at));
            }
            m.push(dated(json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}), at(14, 0, 2)));
            let t = transcript(&m);
            let rows = build_in(&tz, t.items(), 0, false, &HashSet::new(), &Subagents::default());
            assert_eq!(kinds(&rows), ["user", "work", "day", "work", "prose"], "{tz}");
            assert_eq!(rows.iter().map(|d| d.who).collect::<Vec<_>>(), [Who::You, Who::Agent, Who::None, Who::None, Who::None]);
            assert_eq!(rows.iter().map(|d| d.item).collect::<Vec<_>>(), [0, 1, 2, 2, 3]);
        }
    }

    /// The TUI's live rows: a row keeps the time it started streaming at
    /// when its history lands, though the daemon dates the message when its
    /// round ends (here 20 s later): across a minute, the AGENT label keeps
    /// its `HH:MM`; across midnight, no day line comes late. A row first
    /// seen in history (an attach, a resume) has the history's time.
    #[test]
    fn a_live_time_stays_when_the_history_lands() {
        let text = |t: &str| {
            serde_json::from_value::<SessionEvent>(json!({"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": t}})).unwrap()
        };
        // (user, the agent's first delta, the agent's message time), as (day, h, m, s); its `HH:MM`.
        let cases =
            [((13, 14, 5, 0), (13, 14, 5, 50), (13, 14, 6, 10), "14:05"), ((13, 23, 59, 30), (13, 23, 59, 50), (14, 0, 0, 10), "23:59")];
        for tz in zones() {
            let at = |(d, h, mi, s): (u32, u32, u32, u32)| utc_in(&tz, 2026, 11, d, h, mi, s);
            let shown = |t: &Transcript| {
                let it = &t.items()[1];
                (it.kind, it.id, it.at.clone(), it.at.as_deref().and_then(|a| Stamp::in_zone(a, &tz)).map(|s| s.hhmm.to_string()))
            };
            for (user, live, history, hhmm) in cases {
                let mut t = Transcript::new();
                let go = SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some("go".into()) };
                t.apply_at(&go, &at(user).replace('Z', ".5Z"));
                t.apply_at(&text("done"), &at(live).replace('Z', ".25Z"));
                let before = shown(&t);
                assert_eq!((before.0, before.3.as_deref()), (ItemKind::Assistant, Some(hhmm)), "{tz}");
                let rows = build_in(&tz, t.items(), 0, true, &HashSet::new(), &Subagents::default());
                let msgs = [
                    dated(json!({"role": "user", "content": "go"}), at(user)),
                    dated(json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}), at(history)),
                ];
                t.apply_history(&msgs);
                assert_eq!(shown(&t), before, "{tz} {hhmm}: the same row, the same time");
                let after = build_in(&tz, t.items(), 0, true, &HashSet::new(), &Subagents::default());
                assert_eq!(kinds(&after), kinds(&rows));
                assert!(after.iter().all(|d| d.kind != Kind::Day), "{tz} {hhmm}: no late day line");
                // Never seen live: the history's time, and its day.
                let fresh = transcript(&msgs);
                assert_eq!(fresh.items()[1].at.as_deref(), Some(at(history).as_str()));
                let lines = build_in(&tz, fresh.items(), 0, false, &HashSet::new(), &Subagents::default());
                assert_eq!(lines.iter().any(|d| d.kind == Kind::Day), history.0 != user.0, "{tz} {hhmm}");
            }
        }
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

    /// Q1 cards (polish.md §2.1a, R1, R3, R4, R6): verbs, `~`, a path
    /// split in two, read lines and hits, the well's header and numbers.
    #[test]
    fn a_card_says_its_verb_target_and_facts() {
        use super::{Card, replaced, tilde_in, verb_of};
        use synaps_client::transcript::{ToolCall, ToolStatus};
        let v = |n: &str| verb_of(n).0.to_string();
        assert_eq!(
            [v("read"), v("multi_edit"), v("subagent_start"), v("subagent_collect"), v("ext__github__search_issues")],
            ["read", "edit", "spawn", "collect", "search_issues"]
        );
        assert_eq!(verb_of("ext__github__search_issues").1.as_deref(), Some("[github]"));
        assert_eq!(tilde_in("/home/you/work/a.rs", "/home/you", true), "~/work/a.rs");
        assert_eq!(tilde_in("cd /home/you && ls /home/yourself", "/home/you", false), "cd ~ && ls /home/yourself");
        // Quoted, a shell does not say the home as `~`; JSON does.
        assert_eq!(tilde_in("cat \"/home/you/a\"", "/home/you", false), "cat \"/home/you/a\"");
        assert_eq!(tilde_in(r#"{"path":"/home/you/a"}"#, "/home/you", true), r#"{"path":"~/a"}"#);
        // Windows: any case, either separator, escaped in JSON.
        let win = "C:\\Users\\you";
        assert_eq!(tilde_in("type c:\\users\\you\\a.rs", win, false), "type ~\\a.rs");
        assert_eq!(tilde_in("C:/Users/you/a.rs", win, true), "~/a.rs");
        assert_eq!(tilde_in(r#"{"p":"C:\\Users\\you\\a.rs"}"#, win, true), r#"{"p":"~\\a.rs"}"#);
        assert_eq!(replaced("Edited a.rs — replaced 3 line(s) with 6 line(s)").as_deref(), Some("replaced 3 → 6 lines"));
        assert_eq!(replaced("Edited replaced/a.rs — replaced 1 line(s) with 2 line(s)").as_deref(), Some("replaced 1 → 2 lines"));
        let call = |name: &str, input: &str, result: &str| ToolCall {
            tool_id: "t".into(),
            name: name.into(),
            input: input.into(),
            result: result.into(),
            tail: String::new(),
            status: Some(ToolStatus::Done),
            started: 0,
        };
        let card = |t: &ToolCall, arg: &str| Card::of(t, None, arg, crate::app::input_of(&t.input).as_ref());
        // A read of 2 lines, the file going on for 9 more.
        let read = card(&call("read", "src/a.rs", "41\tfn a() {\n42\t}\n\n... (9 more lines)"), "src/a.rs");
        assert_eq!(read.path, Some(("src/".into(), "a.rs".into())));
        assert_eq!(read.facts.as_deref(), Some("2 lines"));
        assert_eq!(read.head, Some(("src/a.rs".into(), Some("lines 41–42 of 51".into()))));
        assert_eq!(read.numbered.as_deref().map(|n| n.len()), Some(2));
        assert_eq!(read.more, None);
        // Offset and limit: the path, not the input's JSON.
        let input = r#"{"limit":50,"offset":40,"path":"src/a.rs"}"#;
        let body: String = (41..=90).map(|n| format!("{n}\tx\n")).collect();
        let read = card(&call("read", input, &body), input);
        assert_eq!((read.target.as_ref(), read.facts.as_deref()), ("src/a.rs", Some("50 lines")));
        assert_eq!(read.head.as_ref().and_then(|h| h.1.as_deref()), Some("lines 41–90"));
        assert_eq!(read.more.as_deref(), Some("… 38 more lines"), "12 shown of 50");
        // Cut to the preview: the count from the whole result's end, not
        // the preview's slice.
        let mut cut = call("read", "src/big.rs", &format!("{}…", (1..=90).map(|n| format!("{n}\tx\n")).collect::<String>()));
        cut.tail = "499\tx\n500\tx\n\n... (164 more lines)".into();
        let read = card(&cut, "src/big.rs");
        assert_eq!(read.facts.as_deref(), Some("500 lines"));
        assert_eq!(read.head.as_ref().and_then(|h| h.1.as_deref()), Some("lines 1–500 of 664"));
        assert_eq!(read.more.as_deref(), Some("… 488 more lines"));
        // Its end holds no whole line: it read all it was asked (500).
        cut.tail = "\n... (164 more lines)".into();
        assert_eq!(card(&cut, "src/big.rs").facts.as_deref(), Some("500 lines"));
        // Nor that: at least what the preview holds.
        cut.tail = "[Output virtualised: 30000 bytes, 600 lines]".into();
        let read = card(&cut, "src/big.rs");
        assert_eq!(read.facts.as_deref(), Some("90+ lines"));
        assert_eq!(read.head.as_ref().and_then(|h| h.1.as_deref()), Some("lines 1–90+"));
        assert_eq!(read.more.as_deref(), Some("… more lines"));
        // Grep: matches only; a miss is none, its notes nothing; cut, at least.
        let grep_in = r#"{"path":"src","pattern":"Stale"}"#;
        let grep = card(&call("grep", grep_in, "a.rs:1: x\nb.rs:2: y"), "");
        assert_eq!((grep.target.as_ref(), grep.path.is_none(), grep.facts.as_deref()), ("Stale  src", true, Some("2 hits")));
        let hits = |input: &str, result: &str| card(&call("grep", input, result), "").facts.map(|f| f.to_string());
        assert_eq!(hits(grep_in, "No matches found."), Some("no hits".into()));
        let ignored =
            "No matches found.\n\nNot searched: files excluded by .gitignore or .ignore, and node_modules and target directories.";
        assert_eq!(hits(grep_in, ignored), Some("no hits".into()));
        assert_eq!(
            hits(grep_in, "a.rs:1:x\n\n[1 error while searching, results may be incomplete. First: b: denied]"),
            Some("1+ hits".into())
        );
        assert_eq!(hits(grep_in, "a.rs:1:x\nb.rs:1:y\n\n... (output truncated, more than 100 total bytes)"), Some("2+ hits".into()));
        assert_eq!(hits(grep_in, "a.rs:1:x\n\nNot searched: files excluded by .gitignore"), Some("1 hit".into()));
        let ctx = r#"{"path":"src","pattern":"x","context":1}"#;
        let shown = "src/a.rs-1-at 12:30:05\nsrc/a.rs:2:x on 2026-11-17\nsrc/a.rs-3-after\n--\nsrc/c.rs-9-before\nsrc/c.rs:10:x y\n";
        assert_eq!(hits(ctx, shown), Some("2 hits".into()), "the text's own numbers say nothing");
        assert_eq!(hits(ctx, "src/c.rs-9-before\nsrc/c.rs:10:x y\n"), Some("1 hit".into()));
        assert_eq!(hits(ctx, "1-before\n2:x\n3-after"), Some("1 hit".into()), "one file: no path");
        // Numbers in the path: dated notes, a numbered migration, a path
        // that starts with its date, Windows.
        let notes = "notes/2026-11-17.md-2-y\nnotes/2026-11-17.md:3:todo\nnotes/2026-11-17.md-4-z\n--\nnotes/2026-11-18.md:12:todo\nnotes/2026-11-18.md:13:todo 2026-11-18\nnotes/2026-11-18.md-14-z\n";
        assert_eq!(hits(ctx, notes), Some("3 hits".into()), "a dated path's -11- is not a context line's");
        assert_eq!(hits(ctx, "notes/2026-11-17.md:3:todo\n"), Some("1 hit".into()), "alone: a match");
        assert_eq!(hits(ctx, "notes/2026-11-17.md-2-y\nnotes/2026-11-17.md:3:todo\n"), Some("1 hit".into()));
        assert_eq!(hits(ctx, "migrations/20240101-1-init.sql:4:CREATE\nmigrations/20240101-1-init.sql-5-x\n"), Some("1 hit".into()));
        assert_eq!(hits(ctx, "2026-11-17.md-2-y\n2026-11-17.md:3:x\n2026-11-17.md-4-3:z\n"), Some("1 hit".into()));
        let win = "C:\\Users\\you\\notes\\2026-11-17.md-2-y\nC:\\Users\\you\\notes\\2026-11-17.md:3:todo\nC:\\Users\\you\\notes\\2026-11-17.md-4-z\n";
        assert_eq!(hits(ctx, win), Some("1 hit".into()), "Windows");
        let mut big = call("grep", grep_in, "a.rs:1:x\nb.rs:2:…");
        big.tail = "z.rs:900:x".into();
        assert_eq!(card(&big, "").facts.as_deref(), Some("2+ hits"));
        let find = |result: &str| card(&call("find", r#"{"pattern":"*.rs"}"#, result), "").facts.map(|f| f.to_string());
        assert_eq!(find("No files found."), Some("no hits".into()));
        assert_eq!(find("a.rs\nb.rs"), Some("2 hits".into()));
        // MCP: the tool's own name, cut to the column; its server in the
        // meta; counted apart from another server's.
        let mcp = card(&call("ext__byteray__read_pseudocode", "", ""), "");
        assert_eq!(
            (mcp.verb.as_ref(), mcp.full.as_deref(), mcp.server.as_deref(), mcp.word.as_ref(), mcp.key.as_ref()),
            ("read_pse…", Some("ext__byteray__read_pseudocode"), Some("[byteray]"), "read_pseudocode", "ext__byteray__read_pseudocode")
        );
        let long = card(&call("subagent_status", "", ""), "");
        assert_eq!((long.verb.as_ref(), long.full.as_deref()), ("status", Some("subagent_status")));
        let odd = card(&call("notebook_edit", "", ""), "");
        assert_eq!((odd.verb.as_ref(), odd.full.as_deref(), odd.key.as_ref()), ("notebook…", Some("notebook_edit"), "notebook_edit"));
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
        assert_eq!(s.label.as_ref(), "5 calls");
        assert_eq!(outcome(&s), [("1 failed", Family::Err)], "ok is silent");
        assert_eq!(s.last, (Glyph::Ok, Family::Ok), "a1 failed, a4 (the last) did not");
        assert_eq!(s.tip.as_ref(), "bash 3\nread 2");
        assert_eq!(s.trail_w(10., false), 48.);
        assert_eq!(s.tally.iter().map(|t| t.name_chars).collect::<Vec<_>>(), [4, 4]);
        // The trail at 10 px (6 a character): `1 failed` 48; tight,
        // nothing (the glyph is in the gutter).
        assert_eq!(s.trail_w(10., true), 0.);
    }

    /// A tally names what fits and ends with `+N`, N the calls of the
    /// tools it does not name: the counts on the row add up to its
    /// `n tools`. Mono, so the widths are arithmetic: 6 px a character.
    #[test]
    fn a_tally_cut_short_names_what_fits_and_counts_the_rest() {
        let tal = |name: &str, n: usize| Tally {
            name: name.to_string().into(),
            key: name.to_string().into(),
            n,
            n_text: n.to_string().into(),
            hue: Hue::of(name),
            name_chars: name.len(),
        };
        let t = [tal("bash", 4), tal("read", 3), tal("grep", 2), tal("edit", 2), tal("find", 1), tal("ls", 1), tal("glob", 1)];
        // `name n` is (chars + 1)·6: 36 each, `ls 1` 24; 14 apart.
        let all = 6. * 36. + 24. + 6. * 14.;
        assert_eq!(tally_fit(&t, all, 6., 14.), 7, "it all fits: no +N");
        // A pixel short: `glob 1` gives way to `+1` (14 + 2·6 = 26 px).
        assert_eq!(tally_fit(&t, all - 1., 6., 14.), 6);
        // `bash 4` and `+10` (14 + 3·6) need 68 px; with `read 3` (50)
        // the tail is `+7` (26): 112.
        assert_eq!(tally_fit(&t, 111., 6., 14.), 1);
        assert_eq!(tally_fit(&t, 112., 6., 14.), 2);
        assert_eq!(tally_fit(&t, 67., 6., 14.), 0, "not even one: the label alone, the tally in the tooltip");
        assert_eq!(tally_fit(&[], 0., 6., 14.), 0);
        let named = tally_fit(&t, all - 1., 6., 14.);
        let shown = t[..named].iter().map(|t| t.n).sum::<usize>();
        let rest = t[named..].iter().map(|t| t.n).sum::<usize>();
        assert_eq!((shown, rest, shown + rest), (13, 1, 14));
        assert_eq!((plus(1).as_ref(), plus(10).as_ref(), plus(30).as_ref()), ("+1", "+10", "+30"));
        assert_eq!((digits(0), digits(9), digits(10), digits(123)), (1, 1, 2, 3));
        // At 7.2 px a character (mono at 12 px) every text box is a whole
        // pixel wide, as GPUI lays it out: `bash` 28.8 → 29, `4` 7.2 → 8.
        assert_eq!((text_w(4, 7.2), text_w(1, 7.2), text_w(5, 7.2), text_w(0, 7.2)), (29., 8., 36., 0.));
        // `bash 4`, `read 3`: 2 · (29 + 7.2 + 8) + 14 = 102.4 (the advances
        // alone, 2 · 43.2 + 14 = 100.4, would name both in 101 px).
        let two = &t[..2];
        assert_eq!(tally_fit(two, 102.5, 7.2, 14.), 2);
        assert_eq!(tally_fit(two, 101., 7.2, 14.), 1, "both do not fit: `bash 4` and `+3`");
    }

    /// The glyph says how the run ended (its last call); the words say
    /// what did not end ok, worst first. HALT mid-run cancels the rest, and
    /// a denied call says denied.
    #[test]
    fn a_summary_ticks_only_when_every_call_is_done() {
        let t = transcript(&turn(5, "a"));
        let s = RunSummary::of(&t.items()[1..6], &rows(&t, &Ledger::default())[1..6]);
        assert_eq!((outcome(&s), s.last), (vec![], (Glyph::Ok, Family::Ok)));

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
        assert_eq!(outcome(&s), [("3 cancelled", Family::Warn)]);
        assert_eq!(s.last, (Glyph::Cancelled, Family::Warn));

        // A denied call, and one stopped at the output limit.
        let mut m = turn(4, "a");
        m[2] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a0",
            "content": "Tool call blocked by extension: Tool call confirmation denied: no"}]});
        m[4] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1",
            "content": "line\n[output truncated at 30000 chars]"}]});
        let t = transcript(&m);
        let s = RunSummary::of(&t.items()[1..5], &self::rows(&t, &Ledger::default())[1..5]);
        assert_eq!((s.ok, s.failed, s.denied, s.stopped), (2, 0, 1, 1));
        assert_eq!(outcome(&s), [("1 denied", Family::Err), ("1 stopped", Family::Warn)], "worst first");
        assert_eq!(s.last, (Glyph::Ok, Family::Ok), "it ended ok: the denial and the limit are in the words");
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
        assert_eq!(outcome(&s), [("1 failed", Family::Err)]);
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

    /// A widening flight's column ([`Reveal`]): from its first frame where
    /// its far edge was, at the landed width; to where it lands, with the
    /// pane. Under the cap with the same gutters it does not move at all.
    #[test]
    fn a_reveal_keeps_the_far_edge_and_lands_centred() {
        let m = Metrics::at(16);
        let lay = |pane: f32| Layout::of(pane, 900., &m, Default::default());
        let reveal = |from: (f32, f32), to: (f32, f32)| {
            let (l0, l1) = (lay(from.1), lay(to.1));
            Reveal { from, col: (from.0 + l0.left, l0.column), to, left: l1.left, column: l1.column, drawn: None }
        };
        // The rail closing (448 to a 48 px strip): the pane 652 to 1052,
        // the column 604 to its cap, 672. Its right edge holds, then it
        // glides to its centre, slower than a centred column would (half
        // the pane's step); the rail's edge goes 400 px.
        let r = reveal((448., 652.), (48., 1052.));
        let right = |w: f32| r.column_x(w) + r.column;
        assert_eq!(right(652.), 448. + 24. + 604., "the far edge where it was");
        assert_eq!(r.column_x(1052.), 48. + 190., "lands centred");
        let xs: Vec<f32> = (0..=10).map(|i| r.column_x(652. + 40. * i as f32)).collect();
        assert!(xs.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] <= 20.), "a glide, never a step: {xs:?}");
        // The right panel closing (its left edge holds): the near edge holds.
        let r = reveal((48., 652.), (48., 1052.));
        assert_eq!(r.column_x(652.), 48. + 24., "its left edge where it was");
        assert_eq!(r.column_x(1052.), 48. + 190.);
        // A split's left pane as the rail closes (its divider rides the
        // rail: both edges go left, the left one twice as far): the far
        // edge holds in the first frame, never stepping back, then glides
        // with the pane to its centre.
        let r = reveal((312., 648.), (32., 788.));
        let right = |w: f32| r.column_x(w) + r.column;
        assert_eq!(right(648.), 312. + 24. + 600., "the far edge where it was");
        assert_eq!(r.column_x(788.), 32. + 58., "lands centred");
        let xs: Vec<f32> = (0..=14).map(|i| r.column_x(648. + 10. * i as f32)).collect();
        assert!(xs.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] <= 20.), "a glide, never a step: {xs:?}");
        // Near the cap (660 to 672: 12 px gained, under the 24 px gutter):
        // the far edge held would hop the near one 12 px in view; the near
        // edge holds, and the column glides with the pane to its centre.
        let r = reveal((448., 708.), (48., 1108.));
        assert_eq!(r.column_x(708.), 448. + 24., "the near edge where it was");
        assert_eq!(r.column_x(1108.), 48. + 218., "lands centred");
        let xs: Vec<f32> = (0..=10).map(|i| r.column_x(708. + 40. * i as f32)).collect();
        assert!(xs.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] <= 21.), "a glide, never a step: {xs:?}");
        // Under the cap both ends, the same gutters (580 to 700): where it
        // lands, in every frame.
        let (l0, l1) = (lay(580.), lay(700.));
        assert!(l1.column < m.column && (l0.left, l1.left) == (24., 24.), "under the cap: {l0:?} {l1:?}");
        let r = reveal((168., 580.), (48., 700.));
        for w in [580., 600., 650., 699., 700.] {
            assert_eq!(r.column_x(w), 48. + 24., "still at {w}");
        }
        // Nothing to gain (a flight back to where it was): where it lands.
        let r = Reveal { from: (48., 652.), col: (72., 604.), to: (48., 652.), left: 24., column: 604., drawn: None };
        assert_eq!(r.column_x(652.), 72.);
        // A width that steps back half a pixel in the tail: the column
        // holds, never back towards where it started.
        let mut r = reveal((312., 648.), (32., 788.));
        let xs: Vec<f32> = [648., 700., 780., 786., 785.5, 787., 786.5, 788.].iter().map(|&w| r.step(w)).collect();
        assert!(xs.windows(2).all(|w| w[1] <= w[0]), "never back: {xs:?}");
        assert_eq!(xs[xs.len() - 1], 32. + 58., "lands");
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
