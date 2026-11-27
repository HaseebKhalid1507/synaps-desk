//! The op log (synaps-cards `docs/blocks.md` v1.3 §3.8, §5.2; §8.2's D7
//! row): the board's mutation stream (wild2, point's WIRE).
//!
//! - **Ctrl+O** (Cmd+O on macOS) opens it over the focused pane, on a new
//!   press, never mid IME composition; Ctrl+O again or Esc closes it, and
//!   the keys go back where they were. Tab flips its two tabs.
//! - **Ops** lists the board's latest 256 lines ([`Drawn::log`], the board
//!   drawn, the live turn's included), newest first: the time (its row's,
//!   never "now"), who, the op, the id, and how it went (applied, placed
//!   here, no change, exists, skipped and why, a path skipped, unbound,
//!   a stale notice). A pending line says so. It is where an agent's
//!   skipped op, which the agent never learns of, becomes visible.
//!   Clicking a line whose block is live jumps to it: the walk entered on
//!   it ([`crate::walk`]).
//! - **Before compaction** is the archive (§5.2): the predecessor's final
//!   blocks and op log, read-only, as text (≤ [`ARCHIVE_MAX`]), one per
//!   session; the next compaction replaces it, and a desk restart loses
//!   it. Never a revision of a new block, never matched by id: its lines
//!   don't jump. Its text can be copied whole.
//! - **Derived and read-only**: nothing here changes the board.

use std::collections::HashMap;
use std::sync::Arc;

use cards_spec::board::{Did, Logged, Outcome, PathWhy, Why};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::blocks::{Drawn, Ref};

use crate::app::{AppView, display, mono};
use crate::fonts::{self, TextPx as _, size};
use crate::theme::Palette;

/// The archive's text holds at most this many bytes (§1.8).
pub(crate) const ARCHIVE_MAX: usize = 256 * 1024;

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

/// The op log open: its tab, its focus, where the keys were, its scroll.
pub(crate) struct OpLog {
    pub tab: Tab,
    pub focus: FocusHandle,
    pub back: Option<FocusHandle>,
    pub scroll: ScrollHandle,
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

/// The archive (§5.2, module docs): the predecessor's final blocks and op
/// log, as text lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Archive {
    pub blocks: Vec<String>,
    pub log: Vec<String>,
}

impl Archive {
    /// Board `d`'s, its rows' times read by `time`: its live blocks (the
    /// transcript's in reading order, then the rail's), its log newest
    /// first, the oldest lines dropped past [`ARCHIVE_MAX`].
    pub(crate) fn of(d: &Drawn, time: impl Fn(Option<u64>) -> Option<String>) -> Self {
        let mut blocks: Vec<String> = Vec::new();
        let mut rows: Vec<(&u64, &Arc<synaps_client::blocks::RowBlocks>)> = d.rows.iter().collect();
        rows.sort_by_key(|(k, _)| **k);
        let mut one = |s: &synaps_client::blocks::Shown| {
            let b = &s.block;
            let text = match &b.body {
                synaps_client::blocks::Body::Note(n) => format!(" · {}", n.text.lines().next().unwrap_or("")),
                _ => String::new(),
            };
            blocks.push(
                format!("{} · {} · {} · r{}{text}", b.id, b.body.kind().as_str(), crate::blocks::title_of(s), b.rev).replace('\n', " "),
            );
        };
        for (_, r) in rows {
            r.blocks.iter().for_each(&mut one);
        }
        d.rail.iter().flatten().for_each(&mut one);
        let log: Vec<String> = d.log.iter().rev().map(|(l, row)| text(&line(l, time(*row).as_deref(), false))).collect();
        let mut a = Archive { blocks, log };
        while a.bytes() > ARCHIVE_MAX && a.log.pop().is_some() {}
        while a.bytes() > ARCHIVE_MAX && a.blocks.pop().is_some() {}
        a
    }

    /// What it keeps, heap included (bytes).
    pub(crate) fn bytes(&self) -> usize {
        let v = |l: &Vec<String>| l.capacity() * std::mem::size_of::<String>() + l.iter().map(String::capacity).sum::<usize>();
        std::mem::size_of::<Self>() + v(&self.blocks) + v(&self.log)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.log.is_empty()
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
        self.oplog = Some(OpLog { tab: Tab::Ops, focus, back, scroll: ScrollHandle::new() });
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

    /// The focused pane's session's log lines, newest first.
    pub(crate) fn oplog_lines(&self) -> Vec<Line> {
        let l = &self.model.lives[self.panes.cur.live];
        let Some(d) = l.transcript.drawn() else { return Vec::new() };
        let items: HashMap<u64, &synaps_client::transcript::Item> = l.transcript.items().iter().map(|i| (i.id, i)).collect();
        let time = |row: Option<u64>| {
            row.and_then(|r| items.get(&r)).and_then(|i| i.at.as_deref()).and_then(crate::times::Stamp::local).map(|t| t.hhmm.to_string())
        };
        let live: std::collections::HashSet<Ref> =
            d.rows.values().flat_map(|r| r.blocks.iter()).chain(d.rail.iter().flatten()).map(|s| s.block.id.clone()).collect();
        d.log.iter().rev().map(|(lg, row)| line(lg, time(*row).as_deref(), lg.id.as_ref().is_some_and(|i| live.contains(i)))).collect()
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
            Tab::Archive => match self.model.lives[self.panes.cur.live].archive.as_ref().filter(|a| !a.is_empty()) {
                None => list = list.child(row("Nothing before a compaction yet".into(), p.dim_k, "oplog-archive-empty".into())),
                Some(a) => {
                    list = list.child(row("blocks".into(), p.dim_k, "oplog-archive-head blocks".into()));
                    for (k, b) in a.blocks.iter().enumerate() {
                        list = list.child(row(b.clone(), p.prose_k, format!("oplog-archive-block {k} {b}")));
                    }
                    list = list.child(row("op log".into(), p.dim_k, "oplog-archive-head log".into()));
                    for (k, l) in a.log.iter().enumerate() {
                        list = list.child(row(l.clone(), p.prose_k, format!("oplog-archive-line {k} {l}")));
                    }
                }
            },
        }
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
                    }
                    cx.notify();
                }))
        };
        let archive = self.model.lives[self.panes.cur.live].archive.clone().filter(|a| !a.is_empty());
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

    /// §5.2: the archive is the board's blocks and its log, newest first,
    /// bounded.
    #[test]
    fn the_archive_is_the_final_blocks_and_log_bounded() {
        let (b, events) = Board::of_history(&synaps_client::synthetic::blocks_history());
        let v = b.view(&events);
        let d = Drawn { log: v.log().map(|l| (l.clone(), None)).collect::<Vec<_>>().into(), ..Default::default() };
        let a = Archive::of(&d, |_| None);
        assert_eq!(a.log.len(), v.log().count());
        assert!(a.log[0].contains("call") && a.log[0].contains("unbound"), "newest first: {:?}", a.log[0]);
        assert!(a.text().starts_with("before compaction: blocks\n") && a.text().contains("before compaction: op log"));
        // Past the cap, the oldest lines go.
        let big = Archive { blocks: Vec::new(), log: (0..4000).map(|k| format!("{k:0>100}")).collect() };
        let mut cut = big.clone();
        while cut.bytes() > ARCHIVE_MAX && cut.log.pop().is_some() {}
        assert!(cut.bytes() <= ARCHIVE_MAX && cut.log.len() < big.log.len() && cut.log[0] == big.log[0]);
    }
}
