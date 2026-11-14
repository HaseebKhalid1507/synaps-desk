//! The transcript as turns (`transcript-design/spec.md` sections 4 and 5):
//! the virtual list's rows are display rows, not transcript items. A run of
//! thinking and tool items with no agent text between them is one block on
//! the tool surface; a finished run of 4 or more tool calls folds to one
//! summary row. `YOU` and `AGENT` hang in the margin, once per turn.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use gpui_kit::base::text::TextView;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::transcript::{Item, ItemKind, ToolStatus};

use crate::app::{AppView, RowCache, display, mono};
use crate::files::{LineKind, Preview};
use crate::fonts::{self, label};
use crate::theme::{self, Metrics, Palette};

/// A run folds once it has finished with this many tool calls.
pub(crate) const FOLD_AT: usize = 4;

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

/// The display rows of `items[from..]` (`from`: a turn's start, or 0).
/// `busy`: a turn is running, so a run at the very end is live; `open`:
/// the runs the user unfolded (by their first item's id).
pub(crate) fn build(items: &[Item], from: usize, busy: bool, open: &HashSet<u64>) -> Vec<Disp> {
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
                        done &= finished(status(x));
                    }
                    (x.id, x.rev).hash(&mut h);
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
                            rev: items[k].rev,
                            kind: Kind::Work,
                            gap: Gap::None,
                            who: Who::None,
                        }));
                    }
                } else {
                    out.extend((start..end).map(|k| Disp {
                        item: k,
                        id: items[k].id,
                        rev: items[k].rev,
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

/// What a folded run's summary says: `+ 7 tools  bash 3  read 3  grep 1 … ✓`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunSummary {
    pub tools: usize,
    /// Most used first (then first used first).
    pub tally: Vec<(SharedString, usize, Hue)>,
    pub failed: usize,
    pub added: u32,
    pub removed: u32,
}

impl RunSummary {
    pub(crate) fn of(items: &[Item], rows: &[RowCache]) -> Self {
        let mut tally: Vec<(SharedString, usize, Hue)> = Vec::new();
        let mut s = RunSummary { tools: 0, tally: Vec::new(), failed: 0, added: 0, removed: 0 };
        for (it, row) in items.iter().zip(rows).filter(|(i, _)| i.kind == ItemKind::Tool) {
            s.tools += 1;
            if matches!(status(it), Some(ToolStatus::Failed | ToolStatus::Denied)) {
                s.failed += 1;
            }
            if let Some(pv) = &row.preview {
                s.added += pv.added;
                s.removed += pv.removed;
            }
            match tally.iter_mut().find(|t| t.0 == row.name) {
                Some(t) => t.1 += 1,
                None => tally.push((row.name.clone(), 1, row.hue)),
            }
        }
        // Stable: equal counts keep their first-use order.
        tally.sort_by_key(|a| std::cmp::Reverse(a.1));
        s.tally = tally;
        s
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

    pub(crate) fn update(&mut self, rows: &[Disp], items: &[Item], cache: &[RowCache]) {
        for d in rows {
            if let Kind::Summary { end, .. } = d.kind
                && self.0.get(&d.id).is_none_or(|e| e.0 != d.rev)
            {
                let s = RunSummary::of(&items[d.item..end], &cache[d.item..end]);
                self.0.insert(d.id, (d.rev, Arc::new(s)));
            }
        }
    }
}

/// How the centre pane is laid out this frame (from its width).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    /// The column (`min(42 rem, pane − 2 gutters)`).
    pub column: f32,
    /// Labels hang in the margin (the pane has the column plus 200 rem/16).
    pub hang: bool,
    /// Under 560 px: the gutter drops to 16, status words and hints go.
    pub tight: bool,
}

impl Layout {
    pub(crate) fn of(pane: f32, m: &Metrics) -> Self {
        let tight = pane < 560.;
        let gutter = if tight { 16. } else { 24. };
        let column = m.column.min(pane - 2. * gutter).max(200.).floor();
        Layout { column, hang: pane >= (672. + 200.) * m.r, tight }
    }

    /// The list's left and right padding: the column centred, whole pixels.
    pub(crate) fn padding(self, pane: f32) -> (f32, f32) {
        let left = ((pane - self.column) / 2.).floor().max(0.);
        (left, (pane - self.column - left).max(0.))
    }
}

impl AppView {
    /// One display row: its gap, its label and the block.
    pub(crate) fn render_disp(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let Some(&d) = self.disp.get(ix) else { return div().into_any_element() };
        let items = self.model.transcript.items();
        let (Some(item), Some(row)) = (items.get(d.item), self.rows.get(d.item)) else { return div().into_any_element() };
        let p = theme::palette(cx);
        let m = theme::metrics(cx);
        let lay = self.layout;
        let gap = match d.gap {
            Gap::None => 0.,
            Gap::First => m.first_gap,
            Gap::Part => m.part_gap,
            Gap::Turn => m.turn_gap,
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
                .child(TextView::markdown(("md", d.id), row.text.clone()).style(cx.global::<theme::Scheme>().user_text.clone()))
                .into_any_element(),
            Kind::Prose => {
                let body = div().w_full().text_size(px(m.body)).line_height(px(m.body_line)).text_color(p.prose);
                if self.plain_rows {
                    body.child(row.text.clone()).into_any_element()
                } else {
                    body.child(TextView::markdown(("md", d.id), row.text.clone())).into_any_element()
                }
            }
            Kind::Note => {
                let color = if item.kind == ItemKind::Error { p.err } else { p.dim };
                mono(row.text.clone(), m.meta, color).w_full().whitespace_normal().into_any_element()
            }
            Kind::Summary { end, open } => self.render_summary(d, end, open, &p, &m, cx),
            Kind::Work => self.render_work(item, row, d.id, &p, &m, lay, cx),
        };
        let who = match d.who {
            Who::None => None,
            Who::You => Some((label::YOU, p.you, m.pad_y)),
            Who::Agent => Some((label::AGENT, p.agent, if d.kind == Kind::Prose { 0. } else { ((m.row - m.body_line) / 2.).max(0.) })),
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

    fn render_summary(&self, d: Disp, end: usize, open: bool, p: &Palette, m: &Metrics, cx: &Context<Self>) -> AnyElement {
        let id = d.id;
        let Some(s) = self.summaries.get(id) else { return div().into_any_element() };
        let _ = end;
        let mut tally = h_flex().flex_1().min_w_0().gap(px((14. * m.r).round())).overflow_hidden().whitespace_nowrap();
        for (name, n, hue) in &s.tally {
            tally = tally.child(
                h_flex().flex_none().gap(px(m.mono * 0.6)).child(div().text_color(hue.color(p)).child(name.clone())).child(n.to_string()),
            );
        }
        let outcome = h_flex()
            .flex_none()
            .gap(px(8.))
            .text_size(px(m.meta))
            .when(s.added + s.removed > 0, |d| {
                d.child(div().text_color(p.ok).child(format!("+{}", s.added)))
                    .child(div().text_color(p.err).child(format!("−{}", s.removed)))
            })
            .map(|d| {
                if s.failed > 0 {
                    d.child(div().text_color(p.err).child(if self.layout.tight {
                        format!("× {}", s.failed)
                    } else {
                        format!("× {} failed", s.failed)
                    }))
                } else {
                    d.child(div().text_color(p.ok).child("✓"))
                }
            });
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
            .child(div().flex_none().child(format!("{} tools", s.tools)))
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
        v_flex()
            .w_full()
            .bg(p.tool)
            .font_family(fonts::MONO)
            .text_size(px(m.mono))
            .line_height(px(m.mono_line))
            .child(head)
            .children(body)
            .into_any_element()
    }

    /// A tool's outcome: `✓`, `× failed`, `× denied`, `cancelled`, a solid
    /// square and `running`, a framed square and `waiting` (the call a
    /// prompt waits on: the last one unfinished). Nothing moves.
    fn outcome(&self, item: &Item, row: &RowCache, st: Option<ToolStatus>, p: &Palette, m: &Metrics, lay: Layout) -> AnyElement {
        let word = |w: &'static str| (!lay.tight).then_some(w);
        let delta = row.preview.as_ref().filter(|pv| pv.added + pv.removed > 0 && st == Some(ToolStatus::Done)).map(|pv| {
            h_flex()
                .gap(px(6.))
                .child(div().text_color(p.ok).child(format!("+{}", pv.added)))
                .child(div().text_color(p.err).child(format!("−{}", pv.removed)))
        });
        let (glyph, text, color): (Option<Div>, Option<&'static str>, Hsla) = match st {
            Some(ToolStatus::Done) => (None, Some("✓"), p.ok),
            Some(ToolStatus::Failed) => (None, Some(if lay.tight { "×" } else { "× failed" }), p.err),
            Some(ToolStatus::Denied) => (None, Some(if lay.tight { "×" } else { "× denied" }), p.err),
            Some(ToolStatus::Cancelled) => (None, word("cancelled"), p.dim_t),
            // The daemon stopped it at its output limit: not ✓, not ×.
            Some(ToolStatus::Unknown) => (None, word("stopped at limit"), p.dim_t),
            Some(ToolStatus::Queued) => (None, word("queued"), p.dim_t),
            _ if self.waiting_on == Some(item.id) && !self.model.transcript.pending_prompts().is_empty() => {
                let g = div()
                    .relative()
                    .size(px(10.))
                    .flex_none()
                    .child(div().absolute().inset_0().border_1().border_color(p.warn_mark))
                    .child(div().absolute().top(px(3.)).left(px(3.)).size(px(4.)).bg(p.warn_mark));
                (Some(g), word("waiting"), p.warn)
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

    use serde_json::json;
    use synaps_client::transcript::ItemKind;

    use super::{Disp, Gap, Hue, Kind, Layout, Metrics, RowCache, RunSummary, Who, build, diff, turn_start};
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
        let rows = build(t.items(), 0, false, &HashSet::new());
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
        let rows = build(t.items(), 0, false, &open);
        assert_eq!(kinds(&rows)[..7], ["user", "unfold", "work", "work", "work", "work", "prose"]);
    }

    #[test]
    fn the_live_run_never_folds() {
        let mut m = turn(5, "a");
        m.pop();
        let t = transcript(&m);
        assert_eq!(kinds(&build(t.items(), 0, true, &HashSet::new()))[1], "work", "busy: the last run is live");
        assert_eq!(kinds(&build(t.items(), 0, false, &HashSet::new()))[1], "fold", "the turn ended");
    }

    #[test]
    fn a_rebuild_from_the_turn_matches_a_full_one() {
        let mut m = turn(4, "a");
        m.extend(turn(2, "b"));
        let t = transcript(&m);
        let all = build(t.items(), 0, false, &HashSet::new());
        let k = t.items().iter().rposition(|i| i.kind == ItemKind::User).unwrap();
        assert_eq!(turn_start(t.items(), k + 1), k);
        assert_eq!(turn_start(t.items(), k), 0, "a new turn can finish the run before it");
        let from = all.partition_point(|d| d.item < k);
        assert_eq!(&all[from..], &build(t.items(), k, false, &HashSet::new())[..]);
    }

    #[test]
    fn diffs_keep_the_common_head_and_tail() {
        let mut m = turn(4, "a");
        m.extend(turn(2, "b"));
        let t = transcript(&m);
        let a = build(t.items(), 0, false, &HashSet::new());
        assert_eq!(diff(&a, &a), None);
        let b = build(t.items(), 0, false, &[a[1].id].into());
        assert_eq!(diff(&a, &b), Some((1..2, 5)), "unfolding: the summary (its sign) and the run's rows");
        assert_eq!(diff(&b, &a), Some((1..6, 1)));
        assert_eq!(diff(&a[..3], &a), Some((3..3, a.len() - 3)));
    }

    #[test]
    fn summaries_count_tools_failures_and_names() {
        let mut m = turn(5, "a");
        m[4] = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1", "content": "boom", "is_error": true}]});
        let t = transcript(&m);
        let rows: Vec<RowCache> = t.items().iter().map(|i| crate::app::row_cache(i, &crate::files::Ledger::default())).collect();
        let s = RunSummary::of(&t.items()[1..6], &rows[1..6]);
        assert_eq!(s.tools, 5);
        assert_eq!(s.failed, 1);
        assert_eq!(s.tally.iter().map(|t| (t.0.as_ref(), t.1)).collect::<Vec<_>>(), [("bash", 3), ("read", 2)]);
        assert_eq!(s.tally[0].2, Hue::Bash);
    }

    #[test]
    fn the_column_is_capped_and_labels_hang_when_there_is_room() {
        let m = Metrics::at(16);
        let wide = Layout::of(1988., &m);
        assert_eq!((wide.column, wide.hang, wide.tight), (672., true, false));
        assert_eq!(Layout::of(942., &m).column, 672.);
        assert!(Layout::of(942., &m).hang);
        assert_eq!((Layout::of(678., &m).column, Layout::of(678., &m).hang), (630., false));
        assert_eq!((Layout::of(1988., &Metrics::at(18)).column), 756.);
        let tight = Layout::of(500., &m);
        assert_eq!((tight.column, tight.tight), (468., true));
        // Dragging a side panel while the column is capped changes only the
        // padding: the list's item width (what its cache is keyed on) holds.
        for pane in [900., 1100., 1318., 1988.] {
            let l = Layout::of(pane, &m);
            let (a, b) = l.padding(pane);
            assert_eq!(pane - a - b, 672., "{pane}");
            assert!((a - b).abs() <= 1.);
        }
    }
}
