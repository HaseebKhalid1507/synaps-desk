//! The window's frame around the session: the mark strip, the top bar (what
//! needs you, connection, the accent picker), the sessions rail (find, the
//! count chips, PINNED / OPEN / HISTORY, the hover readout) and the
//! right-hand numbers.

use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::app::{AppView, EdgeDrag, STRIP, Side, ToggleRail, ToggleRight, count, display, mono, section, tbtn};
use crate::fonts::TextPx as _;
use crate::fonts::{self, label, size};
use crate::glyph::{self, Ground};
use crate::model::{ConnState, Model};
use crate::named::RowAction;
use crate::rail::{self, Chip, Item, Rail, State};
use crate::stats::compact;
use crate::tabs::Place;
use crate::theme::{self, Palette};

/// Left inset of the layout before the rail. The mark strip that used to
/// sit here (connection disc over 荒坂) is gone; the disc is in the top bar.
pub(crate) const MARK: f32 = 0.;
/// Context-gauge segments (the folded panel's upright gauge: `STRIP_SEGMENTS`).
const SEGMENTS: usize = 48;
const STRIP_SEGMENTS: usize = 24;
/// The gauge turns `error_color` from here (the TUI's `context_bar_color`).
const HOT: f64 = 0.75;
/// Files listed before "+N more".
const FILES_SHOWN: usize = 40;
/// The top bar's height: the side panels and their edges start under it.
pub(crate) const TOP: f32 = 52.;
/// In a short window ([`crate::turns::Layout::short`]): its one line of
/// 13-px caps and the 18-px SYSTEMS badge still have 11 px above and below.
pub(crate) const TOP_SHORT: f32 = 40.;
/// An edge handle's hit area; its line is 1 px, on the panel's border.
const EDGE: f32 = 6.;
/// The theme picker's narrowest width: two columns of theme names.
const PICKER_MIN: f32 = 340.;
/// The tab strip (full, and short): a picker above the composer keeps
/// below it and the top bar.
const TABS_CLEAR: f32 = crate::tabstrip::TAB_H;
const TABS_CLEAR_SHORT: f32 = crate::tabstrip::TAB_H + 1.;

/// The find field's placeholder, and its first word for a rail too narrow
/// for all of it.
pub(crate) const FIND: (&str, &str) = ("find · open by name", "find");

/// Whether the whole find placeholder fits a rail `rail` px wide (`cell`:
/// a 12-px mono cell): the rail's insets (22 each side), the gap (8) and
/// the `/` key (5 each side, its border, a cell), less the kit's padding of
/// the field's text.
pub(crate) fn find_fits(rail: f32, cell: f32) -> bool {
    let room = rail - 2. * 22. - 8. - (2. * 5. + 2. + cell) - 2. * crate::app::kit_input_px();
    room >= FIND.0.chars().count() as f32 * cell
}

/// The rail states that say a word on line 2 ([`State::word`]).
pub(crate) const WORDED: [State; 6] = [State::Opening, State::NeedsYou, State::Running, State::Workers, State::Locked, State::Unread];

/// A rail row's sub is dropped when it would show fewer cells than this
/// (six characters and the ellipsis): "Re…" is noise, not information.
const SUB_MIN_CELLS: f32 = 7.;

/// The top bar's fixed words, measured in the faces they are drawn in.
pub(crate) fn top_words(window: &Window) -> crate::fit::TopWords {
    let width = |text: &str, family: &'static str, weight: FontWeight, size: f32| {
        let text = SharedString::from(text.to_string());
        let run = TextRun {
            len: text.len(),
            font: Font { weight, ..font(family) },
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(window.text_system().shape_line(text, px(size), &[run], None).width).ceil()
    };
    let display = |text, size| width(text, fonts::DISPLAY, FontWeight::SEMIBOLD, size);
    let cell = |size| width("0000000000", fonts::MONO, FontWeight::NORMAL, size) / 10.;
    crate::fit::TopWords {
        word: display(label::WORD, 13.),
        word2: display(label::WORD_2, 13.),
        need: display(&format!("   {}", label::NEED_YOU), size::D_S),
        digit: display("0", size::D_S),
        jump: display(label::JUMP, size::D_S),
        theme: display(label::THEME, size::D_S),
        systems: display(label::SYSTEMS, size::D_S),
        cell: cell(size::META),
        cell_conn: cell(size::META),
        cell_badge: cell(size::META),
    }
}

/// The top bar's height this frame.
pub(crate) fn top_h(short: bool) -> f32 {
    if short { TOP_SHORT } else { TOP }
}

/// The TUI's `context_bar_color`: `border_active` under 50 %,
/// `status_streaming` under 75 %, `error_color` above.
fn gauge_color(ratio: Option<f64>, p: &Palette) -> Hsla {
    match ratio.unwrap_or(0.) {
        r if r < 0.5 => p.focus_ch,
        r if r < HOT => p.run_mark_ch,
        _ => p.err_mark_ch,
    }
}

fn kv(key: &'static str, value: impl Into<SharedString>, big: bool, color: Hsla, p: &Palette) -> impl IntoElement {
    let value = value.into();
    v_flex().gap(px(5.)).child(display(key, size::D_S, p.dim_ch)).child(if big {
        display(value, 17., color).into_any_element()
    } else {
        mono(value, 13., color).into_any_element()
    })
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The rail, built when the lists or the user's view of them change (or a
/// minute passes, for the ages), never per frame. The list is spliced only
/// where items changed, so the rest keep their place and measured height.
#[derive(Default)]
pub(crate) struct RailCache {
    generation: Option<u64>,
    minute: i64,
    view: rail::View,
    states: u64,
    pub(crate) rail: Rail,
    keys: Vec<u64>,
    /// Rows on screen before the last splice: their key and top, in the
    /// list's content (Flip, motion.md §6.4).
    pub(crate) flip_from: Vec<(u64, f32)>,
    /// In the frame of a splice, where each item will be laid out (the
    /// list can't say until it lays out again): [`rail::flip_targets`].
    pub(crate) targets: Option<Vec<Option<(f32, f32)>>>,
}

/// A row's key, if the item is a row.
pub(crate) fn row_key(rail: &Rail, item: &Item) -> Option<u64> {
    match item {
        Item::Row(i) | Item::Hist(i) => rail.rows.get(*i).map(|r| r.key),
        _ => None,
    }
}

/// Item `ix`'s top and height in `list`'s content (not moved by a scroll),
/// and the content's scroll offset, as of the last layout.
pub(crate) fn item_y(list: &ListState, ix: usize) -> Option<(f32, f32, f32)> {
    let vp = list.viewport_bounds();
    let b = list.bounds_for_item(ix)?;
    let scroll = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    Some((f32::from(b.top() - vp.top()) + scroll, f32::from(b.size.height), scroll))
}

/// What the rail's motion knows this frame (fluid.md §3.7): each row's
/// paint offset (Flip), the selection quad (its top in the list's view and
/// height, while it moves), the readout's y offset as it follows.
#[derive(Default)]
pub(crate) struct RailFx {
    pub(crate) flips: Vec<u64>,
    pub(crate) offsets: HashMap<u64, f32>,
    pub(crate) sel: Option<u64>,
    pub(crate) quad: Option<(f32, f32)>,
    pub(crate) moving: bool,
    /// The readout's y, as it follows from the row before.
    pub(crate) readout_dy: f32,
}

/// Spring keys (the rail's own).
const FLIP: u64 = 0x7AC0 << 48;
const SEL_Y: u64 = 0x7AC1 << 48;
const SEL_H: u64 = 0x7AC2 << 48;
pub(crate) const RAIL_TOUCH: u64 = 0x7AC3 << 48;
pub(crate) const READOUT_Y: u64 = 0x7AC4 << 48;
/// Flip's cap (fluid.md §2.3): a row that moved farther starts this far
/// from its new place, on its true side.
pub(crate) const FLIP_CAP: f32 = 160.;
/// The numbers' and meters' keys (motion.md §6.6).
const GLOW: u64 = 0x7AC5 << 48;
const METER: u64 = 0x7AC6 << 48;

/// A shown value's hash, for its afterglow ([`crate::motion::Motion::glow`]).
fn vhash(v: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// The context gauge's colour class (`gauge_color`'s bands).
fn gauge_class(ratio: Option<f64>) -> u64 {
    match ratio.unwrap_or(0.) {
        r if r < 0.5 => 0,
        r if r < HOT => 1,
        _ => 2,
    }
}

impl RailCache {
    /// The minute the ages were last written in.
    pub(crate) fn minute(&self) -> i64 {
        self.minute
    }

    /// `hold`: a row is held down; a reorder waits for its release (the
    /// rows' content does not: [`rail::keep_order`]).
    pub(crate) fn refresh(&mut self, m: &Model, view: &rail::View, list: &ListState, hold: bool) {
        let now = now_secs();
        // With a chip on, which rows show follows their states (the attached
        // session's run ahead of the poll), and so does which favourites a
        // folded FAVOURITES shows (those that need you or run): a hash, no
        // allocation.
        let folded = view.favs_closed && !m.favs.is_empty();
        let states = if view.chip.is_some() || folded {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for s in &m.sessions {
                if let Some(chip) = view.chip {
                    m.chip_shows(chip, s).hash(&mut h);
                }
                if folded {
                    (m.fav_of(s).is_some() && rail::busy(m, s)).hash(&mut h);
                }
            }
            h.finish()
        } else {
            0
        };
        if self.generation == Some(m.rail_gen) && self.minute == now / 60 && self.view == *view && self.states == states {
            return;
        }
        let rail = rail::build(m, view, now);
        let keys: Vec<u64> = rail.items.iter().map(|i| rail::item_key(&rail, i)).collect();
        let mut flip_from = Vec::new();
        let mut targets = None;
        if let Some((range, count)) = rail::splice(&self.keys, &keys) {
            if hold && !self.keys.is_empty() {
                // Held: the old order, today's content; built again every
                // frame until the release (no generation kept).
                self.rail = rail::keep_order(&self.rail, &rail);
                (self.generation, self.view, self.states) = (None, view.clone(), states);
                return;
            }
            // Flip: where the rows on screen were, before the splice, and
            // where every item will be after it, from the old layout.
            let old: Vec<(u64, u8, Option<(f32, f32)>)> = (0..self.rail.items.len())
                .map(|ix| (self.keys[ix], rail::item_kind(&self.rail.items[ix]), item_y(list, ix).map(|(y, h, _)| (y, h))))
                .collect();
            let new: Vec<(u64, u8)> = keys.iter().zip(&rail.items).map(|(&k, it)| (k, rail::item_kind(it))).collect();
            targets = Some(rail::flip_targets(&old, &new, range.clone(), count));
            let top = list.logical_scroll_top().item_ix;
            let scroll = -f32::from(list.scroll_px_offset_for_scrollbar().y);
            for ix in top..self.rail.items.len() {
                let Some((y, _)) = old[ix].2 else { break };
                if y - scroll > f32::from(list.viewport_bounds().size.height) {
                    break;
                }
                if let Some(k) = row_key(&self.rail, &self.rail.items[ix]) {
                    flip_from.push((k, y));
                }
            }
            let start = range.start;
            list.splice(range, count);
            // The rail measures every row, so its bar reaches its end; GPUI
            // measures new ones up front only once told they need it.
            list.remeasure_items(start..start + count);
        }
        *self = RailCache { generation: Some(m.rail_gen), minute: now / 60, view: view.clone(), states, rail, keys, flip_from, targets };
    }
}

/// What a top-bar ticker (and the NEEDS YOU stub) says about a prompt:
/// the tool and what it will run or asks, from the daemon's confirm body
/// (`Extension: …`, `Tool: …`, `Input that will run:`, `Message from …:`),
/// else the body's first line that is not the closing question. A message
/// that introduces a command (`… a command in ~/w:` then `$ cargo test`)
/// says the command; a detail that names the tool itself is not prefixed
/// with it again; a line that only introduces drops its colon.
pub(crate) fn prompt_gist(title: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let tool = lines.iter().find_map(|l| l.strip_prefix("Tool: ")).filter(|t| !t.is_empty());
    let first_after = |at: usize| (at + 1..lines.len()).find(|&i| !lines[i].is_empty());
    let after = |head: fn(&str) -> bool| lines.iter().position(|l| head(l)).and_then(first_after);
    let at =
        after(|l| l == "Input that will run:").or_else(|| after(|l| l.starts_with("Message from ") && l.ends_with(':'))).or_else(|| {
            lines
                .iter()
                .position(|l| !l.is_empty() && *l != "Allow this tool call?" && !l.starts_with("Extension: ") && !l.starts_with("Tool: "))
        });
    let command = at.filter(|&i| lines[i].ends_with(':')).and_then(first_after).and_then(|i| lines[i].strip_prefix("$ "));
    let text = match (tool, at.map(|i| lines[i]), command) {
        (Some(t), _, Some(c)) => format!("{t}: {c}"),
        (None, _, Some(c)) => format!("$ {c}"),
        (Some(t), Some(d), None) if d.split_whitespace().next() == Some(t) => d.trim_end_matches(':').to_string(),
        (Some(t), Some(d), None) => format!("{t}: {}", d.trim_end_matches(':')),
        (Some(t), None, None) => t.to_string(),
        (None, Some(d), None) => d.trim_end_matches(':').to_string(),
        (None, None, None) => title.to_string(),
    };
    crate::app::one_line(&text, 90)
}

fn fold(id: &'static str, glyph: &'static str, tip: &'static str, rail: bool, p: &Palette, cx: &Context<AppView>) -> Stateful<Div> {
    let hover = p.dim_ch;
    div()
        .id(id)
        .size(px(20.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(p.edge)
        .hover(move |d| d.border_color(hover))
        .child(mono(glyph, 12., p.dim_ch))
        .tooltip(move |window, cx| {
            let key: &dyn Action = if rail { &ToggleRail } else { &ToggleRight };
            crate::overlay::tip_key(tip, key, window, cx)
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(rail, cx)))
}

impl AppView {
    /// The connection disc's colour (the top bar and the folded rail).
    fn conn_disc(&self, p: &Palette) -> Hsla {
        match &self.model.conn {
            ConnState::Connected(_) => p.ready_mark_ch,
            _ if self.synthetic => p.ready_mark_ch,
            ConnState::Connecting | ConnState::Reconnecting => p.rest_mark,
            _ => p.err_mark_ch,
        }
    }

    /// The link's signatures over the whole window, never in its way
    /// (motion.md §5.1, §5.3): the tear's three lines (2, 1 and 3 px in
    /// `err_mark_ch`, at heights hashed from the drop's time and the
    /// frame), or the Scan's 1 px `run_mark_ch` line with its 48 px trail
    /// of four bands of `run` (alpha .10, .07, .04, .02) above it.
    pub(crate) fn render_link(&mut self, tear: Option<usize>, p: &Palette) -> Option<impl IntoElement + use<>> {
        let h = self.height.max(1.);
        let mut out = div().absolute().top_0().left_0().size_full();
        if let (Some(frame), Some(at)) = (tear, self.link.glitch) {
            let seed = {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (at, frame).hash(&mut h);
                h.finish()
            };
            for (i, thick) in [2., 1., 3.].into_iter().enumerate() {
                let y = ((seed >> (i * 16)) & 0xFFFF) as f32 / 65535. * (h - 3.);
                out = out.child(div().absolute().left_0().right_0().top(px(y.floor())).h(px(thick)).bg(p.err_mark_ch));
            }
            return Some(deferred(out).with_priority(5));
        }
        let t = self.link.scan(&self.fx)?;
        let y = (t * (h + 48.)).floor();
        for (i, a) in [0.10, 0.07, 0.04, 0.02].into_iter().enumerate() {
            let top = y - 12. * (i as f32 + 1.);
            out = out.child(div().absolute().left_0().right_0().top(px(top)).h(px(12.)).bg(Hsla { a, ..p.run }));
        }
        out = out.child(div().absolute().left_0().right_0().top(px(y)).h(px(1.)).bg(p.run_mark_ch));
        Some(deferred(out).with_priority(5))
    }

    /// The tracer's line on its way (motion.md §6.2): its head `base` `out`
    /// along the straight line from where the tab was opened to its top
    /// edge, 24 px of it behind, 2 px in `you_bar`, over everything.
    pub(crate) fn render_tracer(&self, p: &Palette) -> Option<impl IntoElement + use<>> {
        let tr = self.tracer?;
        let to = tr.to?;
        let t = self.fx.shot(
            Some(tr.at),
            std::time::Duration::ZERO,
            crate::motion::BASE,
            &crate::motion::ease::OUT,
            crate::motion::Kind::Spatial,
        );
        if t >= 1. {
            return None;
        }
        let (from, color) = (tr.from, p.you_bar);
        let d = to - from;
        let len = f32::from(d.x).hypot(f32::from(d.y)).max(1.);
        let tail = (t - crate::motion::TRACER_LEN / len).max(0.);
        Some(
            deferred(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let mut path = PathBuilder::stroke(px(2.));
                        path.move_to(from + d * tail);
                        path.line_to(from + d * t);
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    },
                )
                .absolute()
                .size_0(),
            )
            .with_priority(5),
        )
    }

    pub(crate) fn render_top(&self, p: &Palette, rail: Side, cx: &Context<Self>) -> impl IntoElement {
        let n = self.model.attention().count();
        let first = self.model.ticker();
        let jump_to = self.model.jump_target().map(str::to_string);
        let what: Option<SharedString> = first.map(|s| {
            if self.model.is_active(&s.id)
                && let Some(pr) = self.model.transcript.pending_prompts().first()
            {
                // Built once per prompt, with the card (`sync_card` ran first).
                self.current_card().map(|c| c.gist.clone()).unwrap_or_else(|| prompt_gist(&pr.title, &pr.prompt).into())
            } else {
                s.title.clone().filter(|t| !t.is_empty() && s.name.is_some()).unwrap_or_default().into()
            }
        });
        // On its way, the word's trailing dots step `·` → `··` → `···` at
        // 2 Hz on the beat (motion.md §6.7: text, not a spinner); held at
        // `···` without the heartbeat. Mono, padded: the word never moves.
        let dots = |word: &str| -> SharedString {
            let n = self.fx.beat(4).map_or(3, |b| (b / 4 % 3) as usize + 1);
            format!("{word} {:<3}", "·".repeat(n)).into()
        };
        let (conn, conn_color): (SharedString, Hsla) = match &self.model.conn {
            _ if self.model.reattaching => (dots("reattaching"), p.dim_ch),
            ConnState::Connected(v) => (format!("connected · {v}").into(), p.dim_ch),
            ConnState::Connecting => (dots("connecting"), p.dim_ch),
            ConnState::Reconnecting => (dots("reconnecting"), p.dim_ch),
            ConnState::Off(e) | ConnState::Incompatible(e) => {
                (crate::app::one_line(e, 60).into(), if self.synthetic { p.dim_ch } else { p.err_ch })
            }
        };
        // As in the mock: subagents running, here the attached session's,
        // as its blocks and its rail row say (they change together).
        let subagents = self.model.active_workers();
        let what = what.filter(|w| !w.is_empty());
        let input = crate::fit::TopIn {
            width: self.width,
            brand: match rail {
                Side::Open(w) => w - 22.,
                _ => 254.,
            },
            need: n,
            label: first.map_or(0, |s| s.label().chars().count()),
            gist: what.as_ref().map_or(0, |w| w.chars().count()),
            more: if n > 1 { (n - 1).ilog10() as usize + 1 } else { 0 },
            jump: jump_to.is_some(),
            conn: conn.chars().count(),
            badge: if subagents > 9 { subagents.ilog10() as usize + 1 } else { 1 },
        };
        let fit = self.top_words.map(|w| crate::fit::fit_top(&w, &input)).unwrap_or(crate::fit::TopFit {
            brand: input.brand,
            word: true,
            word2: true,
            label: 0.,
            gist: 0.,
            conn: 0.,
            theme: true,
            systems: true,
        });
        // Fitted: every part flex_none at its width, nothing overlaps.
        let cut = |d: Div, w: f32| d.flex_none().max_w(px(w)).overflow_hidden().text_ellipsis();
        let on = n > 0;
        let ticker = h_flex()
            .id("attention")
            .flex_none()
            .gap(px(12.))
            .items_center()
            // The state grammar (design.md §4.1): NEEDS YOU's glyph, or at rest the idle one.
            .child(if on { glyph::draw(glyph::Glyph::Needs, p.warn_mark_ch) } else { glyph::draw(glyph::Glyph::Idle, p.rest_mark) })
            // The count afterglows when it changes (motion.md §6.6).
            .child(
                display(
                    format!("{n}   {}", label::NEED_YOU),
                    size::D_S,
                    self.fx.glow(GLOW | 0xFFFF, n as u64, if on { p.warn_ch } else { p.dim_ch }),
                )
                .flex_none(),
            )
            .when_some(first.filter(|_| fit.label > 0.), |d, s| d.child(cut(mono(s.label(), size::META, p.ink), fit.label)))
            .when_some(what.filter(|_| fit.gist > 0.), |d, w| d.child(cut(mono(w, size::META, p.dim_ch), fit.gist)))
            .when(n > 1, |d| d.child(mono(format!("+{}", n - 1), size::META, p.dim_ch).flex_none()))
            .when_some(jump_to, |d, id| {
                d.cursor_pointer()
                    .child(h_flex().flex_none().ml(px(8.)).gap(px(6.)).child(display(label::JUMP, size::D_S, p.prose_ch)).child(mono(
                        "→",
                        size::META,
                        p.prose_ch,
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for fx in this.model.open_row(&RowAction::Attach(id.clone())) {
                            this.effect(fx);
                        }
                        cx.notify();
                    }))
            });
        h_flex()
            .h(px(top_h(self.layout.short)))
            .flex_none()
            .pl(px(22.))
            .pr(px(24.))
            .items_center()
            .border_b_1()
            .border_color(p.hair)
            // Over the open rail: as wide as it (while the bar has room).
            .child(
                h_flex()
                    .w(px(fit.brand))
                    .flex_none()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().size(px(8.)).flex_none().rounded_full().bg(self.conn_disc(p)))
                            .when(fit.word, |d| d.child(display(label::WORD, size::D_M, p.header))),
                    )
                    .when(fit.word2, |d| d.child(display(label::WORD_2, size::D_M, p.dim_ch))),
            )
            .child(div().flex_1().min_w(px(16.)))
            .child(ticker)
            .child(div().flex_1().min_w(px(16.)))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(18.))
                    .items_center()
                    .when(fit.conn > 0., |d| d.child(cut(mono(conn, size::META, conn_color), fit.conn).relative().top(px(self.conn_dy))))
                    // The theme picker's own control: a swatch of the "you" colour.
                    .child(
                        h_flex()
                            .id("accent-picker")
                            .gap(px(8.))
                            .items_center()
                            .cursor_pointer()
                            .child(div().size(px(10.)).bg(p.you_bar))
                            .when(fit.theme, |d| d.child(display(label::THEME, size::D_S, if self.picker_open { p.ink } else { p.dim_ch })))
                            .hover(|d| d.text_color(p.ink))
                            .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                                // The click whose mouse-down closed it from outside does not reopen it.
                                let dismissed = this.picker_dismissed.take();
                                if let ClickEvent::Mouse(m) = ev
                                    && dismissed == Some(m.down.position)
                                {
                                    return;
                                }
                                if this.picker_open {
                                    this.close_picker(window, cx);
                                } else {
                                    this.open_picker(window, cx);
                                }
                            })),
                    )
                    // As in the mock: the attached session's running subagents.
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .when(fit.systems, |d| d.child(display(label::SYSTEMS, size::D_S, p.dim_ch)))
                            .child(
                                div()
                                    .min_w(px(18.))
                                    .h(px(18.))
                                    .px(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(if subagents > 0 { p.ink } else { p.edge })
                                    .child(mono(count(subagents), size::META, if subagents > 0 { p.chrome } else { p.dim_ch })),
                            ),
                    ),
            )
    }

    /// The gauge's bar colour: crossing a band (`HOT`), it cross-fades from
    /// the band before over `slow` (motion.md §6.6).
    fn gauge_bar(&self, key: u64, ratio: Option<f64>, p: &Palette) -> Hsla {
        let bar = gauge_color(ratio, p);
        let Some((prev, since)) = self.fx.changed(key, gauge_class(ratio)) else { return bar };
        let was = [p.focus_ch, p.run_mark_ch, p.err_mark_ch][prev.min(2) as usize];
        let t = self.fx.shot(
            Some(since),
            std::time::Duration::ZERO,
            crate::motion::SLOW,
            &crate::motion::ease::INOUT,
            crate::motion::Kind::Fade,
        );
        crate::motion::mix(was, bar, t)
    }

    /// The rail's motion this frame (fluid.md §3.7, motion.md §6.4), from
    /// the list's last layout, before the list lays out again (its rows
    /// can't ask it then):
    /// - Flip: a row on screen before a splice (a reorder, a session
    ///   starting or ending above it) starts where it was, at most
    ///   [`FLIP_CAP`] away, and springs to its new place (`snap`);
    /// - the selection: the shown row's `raised` fill and 2 px bar are one
    ///   quad that springs from the row shown before (`snap`), from the
    ///   view's edge if that row is off screen;
    /// - the readout: replacing another within 300 ms, it springs its y from
    ///   that row's (`snap`).
    ///
    /// Full only; at rest every row draws its own fill and bar.
    pub(crate) fn rail_motion(&mut self) {
        let snap = crate::motion::SNAP_RESPONSE;
        let readout_from = self.readout_from.take();
        let list = &self.rail_list;
        let rail = &self.rail.rail;
        let fx = &self.fx;
        let ix_of = |key: u64| rail.items.iter().position(|it| row_key(rail, it) == Some(key));
        let view_h = f32::from(list.viewport_bounds().size.height);
        // Where an item is: in a splice's frame, where it will be laid out
        // (never the list's bounds, wrong until it lays out again).
        let targets = self.rail.targets.take();
        let scroll = -f32::from(list.scroll_px_offset_for_scrollbar().y);
        let item_y = |list: &ListState, ix: usize| match &targets {
            Some(t) => t.get(ix).copied().flatten().map(|(y, h)| (y, h, scroll)),
            None => item_y(list, ix),
        };
        let mut out = std::mem::take(&mut self.rail_fx);
        out.offsets.clear();
        for (key, from) in std::mem::take(&mut self.rail.flip_from) {
            if let Some((y, _, _)) = ix_of(key).and_then(|ix| item_y(list, ix))
                && (from - y).abs() >= 0.5
            {
                fx.spring_jump(crate::motion::key(FLIP, key), y + (from - y).clamp(-FLIP_CAP, FLIP_CAP), y, snap);
                if !out.flips.contains(&key) {
                    out.flips.push(key);
                }
            }
        }
        out.flips.retain(|&key| {
            let Some((y, _, _)) = ix_of(key).and_then(|ix| item_y(list, ix)) else { return false };
            let (v, moving) = fx.spring_held(crate::motion::key(FLIP, key), y, snap);
            if moving {
                out.offsets.insert(key, v - y);
            }
            moving
        });
        // The row shown: the visible tab's session.
        let sel = rail.items.iter().enumerate().find_map(|(ix, it)| match it {
            Item::Row(r) => {
                let row = &rail.rows[*r];
                row.live.and_then(|i| self.model.sessions.get(i)).filter(|s| self.model.is_shown(&s.id)).map(|_| (ix, row.key))
            }
            _ => None,
        });
        out.quad = None;
        out.moving = false;
        match sel.and_then(|(ix, key)| item_y(list, ix).map(|y| (key, y))) {
            Some((key, (y, h, scroll))) => {
                if out.sel.is_some_and(|k| k != key)
                    && let (Some(y0), Some(h0)) = (fx.spring_at(SEL_Y), fx.spring_at(SEL_H))
                {
                    // From where it was drawn; off screen, from that edge.
                    let y0 = (y0 - scroll).clamp(-h0, view_h) + scroll;
                    fx.spring_jump(SEL_Y, y0, y, snap);
                    fx.spring_jump(SEL_H, h0, h, snap);
                }
                let ((qy, my), (qh, mh)) = (fx.spring_held(SEL_Y, y, snap), fx.spring_held(SEL_H, h, snap));
                out.moving = my || mh;
                if out.moving {
                    out.quad = Some((qy - scroll, qh));
                }
                out.sel = Some(key);
            }
            None => out.sel = sel.map(|s| s.1),
        }
        out.readout_dy = 0.;
        let shows = self.readout.or(self.readout_leaving.map(|l| l.0));
        if let Some((y, _, _)) = shows.and_then(ix_of).and_then(|ix| item_y(list, ix)) {
            if let Some(from) = readout_from {
                fx.spring_jump(READOUT_Y, from, y, snap);
            }
            out.readout_dy = fx.spring_held(READOUT_Y, y, snap).0 - y;
        }
        self.rail_fx = out;
    }

    /// The drag handle on a side panel's inner edge (`rail`, else the right
    /// panel's), over the rest of the window: its 1-px line is the panel's
    /// border, `line3` under the pointer, `ink3` while dragged. A strip has
    /// one too (dragged out, it opens), unless the window cannot open it.
    pub(crate) fn render_edge(&self, rail: bool, side: Side, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        if side == Side::Strip && crate::app::toggled(self.width, self.panels, rail).is_none() {
            return None;
        }
        let dragged = self.drag.is_some_and(|g| g.moved && g.rail == rail);
        // Mostly over the panel: the border is the rail's last pixel and
        // the right panel's first.
        let (left, line) = if rail { (MARK + side.width() - 4., 3.) } else { (self.width - side.width() - 2., 2.) };
        let (id, group) = if rail { ("rail-edge", "rail-edge") } else { ("right-edge", "right-edge") };
        Some(
            div()
                .id(id)
                .group(group)
                .absolute()
                .top(px(top_h(self.layout.short)))
                .bottom_0()
                .left(px(left))
                .w(px(EDGE))
                .cursor(CursorStyle::ResizeLeftRight)
                // The wheel still scrolls what is under it.
                .block_mouse_except_scroll()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(line))
                        .w(px(1.))
                        .when(dragged, |d| d.bg(p.dim_ch))
                        .when(!dragged, |d| d.group_hover(group, |s| s.bg(p.edge))),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.grab_edge(rail, ev.position.x.into());
                        cx.stop_propagation();
                    }),
                )
                .on_drag(EdgeDrag, |d, _, _, cx| cx.new(|_| *d))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| this.click_edge(rail, ev.click_count(), cx))),
        )
    }

    /// The folded rail: the connection disc and how many sessions need
    /// you. A click (or Ctrl+B) opens it again.
    pub(crate) fn render_rail_strip(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let n = self.model.attention().count();
        let on = n > 0;
        // L1: a window too narrow for the rail keeps its strip and says so.
        let fits = crate::app::toggled(self.width, self.panels, true).is_some();
        v_flex()
            .id("rail-strip")
            .w(px(STRIP))
            .h_full()
            .flex_none()
            .items_center()
            .pt(px(22.))
            .gap(px(18.))
            .border_r_1()
            .border_color(p.hair)
            .cursor_pointer()
            .map(|d| self.lit(d, crate::app::tkey("rail-strip", 0), (p.chrome, p.raised, p.press_ch), cx))
            .child(mono("›", 12., p.dim_ch))
            .child(div().size(px(8.)).rounded_full().bg(self.conn_disc(p)))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(6.)).when(on, |d| d.bg(p.warn_mark_ch)).when(!on, |d| d.border_1().border_color(p.rest_mark)))
                    .child(display(n.to_string(), 13., if on { p.warn_ch } else { p.dim_ch })),
            )
            .child(div().flex_1())
            .child(self.render_strip_foot(p, cx))
            .tooltip(move |window, cx| {
                let tip = match (on, fits) {
                    (_, false) => format!("{n} need you · widen the window to show sessions"),
                    (true, true) => format!("{n} need you · show sessions"),
                    (false, true) => "Show sessions".to_string(),
                };
                crate::overlay::tip_key(tip, &ToggleRail, window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(true, cx)))
    }

    /// The folded right panel: the context gauge, upright, and its
    /// percentage. A click (or Ctrl+Alt+B) opens it again.
    pub(crate) fn render_right_strip(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        // L2: detached, the last numbers seen are in the dim roles, as the
        // open panel's are; so are a cached tab's while its attach is on
        // its way.
        let detached = self.model.stats.detached && self.model.active.is_none();
        let dim = detached || self.opening();
        let faded = p.faded();
        let q = if dim { &faded } else { p };
        let ratio = self.model.stats.context_ratio();
        let bar = self.gauge_bar(METER | 8, ratio, q);
        let filled = ratio.map(|r| ((r * STRIP_SEGMENTS as f64).round() as usize).min(STRIP_SEGMENTS)).unwrap_or(0);
        let mut gauge = v_flex().w(px(6.)).gap(px(2.));
        // Bottom to top as it fills (motion.md §6.6).
        for i in (0..STRIP_SEGMENTS).rev() {
            gauge = gauge.child(div().w_full().h(px(4.)).bg(self.fx.segment(METER | 9, filled, i, p.edge, bar)));
        }
        let pct: SharedString = ratio.map(|r| format!("{:.0}", r * 100.0)).unwrap_or_else(|| "—".into()).into();
        let fits = crate::app::toggled(self.width, self.panels, false).is_some();
        v_flex()
            .id("right-strip")
            .w(px(STRIP))
            .h_full()
            .flex_none()
            .items_center()
            .pt(px(22.))
            .gap(px(14.))
            .border_l_1()
            .border_color(p.hair)
            .cursor_pointer()
            .map(|d| self.lit(d, crate::app::tkey("right-strip", 0), (p.chrome, p.raised, p.press_ch), cx))
            .child(mono("‹", 12., p.dim_ch))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(14.))
                    .child(v_flex().items_center().child(display(pct, 13., q.prose_ch)).child(mono("%", size::META, q.dim_ch)))
                    .child(gauge),
            )
            .tooltip(move |window, cx| {
                let tip = match (detached, fits) {
                    (_, false) => "Widen the window to show context, tokens and files",
                    (true, true) => "Detached: last numbers seen · show context, tokens and files",
                    (false, true) => "Show context, tokens and files",
                };
                crate::overlay::tip_key(tip, &ToggleRight, window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(false, cx)))
    }

    pub(crate) fn render_rail(&self, width: f32, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let n = &self.model.named;
        let notes: Vec<(&String, Hsla)> =
            n.error.iter().map(|e| (e, p.err_ch)).chain(n.notice.iter().chain(n.saved_note.iter()).map(|t| (t, p.dim_ch))).collect();
        // The chips count what is held (the attached session as it runs,
        // ahead of the poll) and the unread history rows.
        let (mut needs, mut running, mut unread) = (0, 0, self.rail.rail.history_unread);
        for s in self.model.sessions.iter().filter(|s| s.lifecycle != "ending") {
            match self.model.fact(s) {
                State::NeedsYou => needs += 1,
                State::Running | State::Workers => running += 1,
                State::Unread => unread += 1,
                _ => {}
            }
        }
        // A chip toggles its filter; none selected shows everything. At
        // 12 px, each count two cells wide whatever it is ([`chip_count`]),
        // the three take 265.6 px: the default rail (268 inside) holds them
        // on one line ([`crate::app::FLOORS`]) and a count's change never
        // moves them; a rail narrower than that wraps the last.
        let mut chips = h_flex()
            .when(width < crate::app::FLOORS.0, |d| d.flex_wrap())
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(4.))
            .px(px(22.))
            .py(px(12.))
            .border_b_1()
            .border_color(p.hair);
        for (c, count) in [(Chip::NeedsYou, needs), (Chip::Running, running), (Chip::Unread, unread)] {
            let on = self.chip == Some(c);
            let tip = match c {
                Chip::NeedsYou => "Show only sessions that need you",
                Chip::Running => "Show only running sessions",
                Chip::Unread => "Show only sessions with news since you looked",
            };
            chips = chips.child(
                h_flex()
                    .id(("chip", c as usize))
                    .relative()
                    .items_baseline()
                    .gap(px(4.))
                    .pb(px(4.))
                    .cursor_pointer()
                    // Selected: the 2-px `you_bar` (design.md §5), under it.
                    .when(on, |d| d.child(div().absolute().left_0().right_0().bottom_0().h(px(2.)).bg(p.you_bar)))
                    .child(display(c.label(), size::D_S, if on { p.ink } else { p.dim_ch }))
                    .child(mono(
                        chip_count(count),
                        size::META,
                        match c {
                            _ if count == 0 => p.dim_ch,
                            Chip::NeedsYou => p.warn_ch,
                            Chip::Running => p.run_ch,
                            Chip::Unread => p.unread_ch,
                        },
                    ))
                    .tooltip(move |window, cx| crate::overlay::tip(tip, window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.chip = if this.chip == Some(c) { None } else { Some(c) };
                        cx.notify();
                    })),
            );
        }
        let find = h_flex()
            .mx(px(22.))
            .mt(px(16.))
            .h(px(27.))
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(p.edge)
            .child(
                div().flex_1().min_w_0().font_family(fonts::MONO).text_px(size::META).child(Input::new(&self.find_input).appearance(false)),
            )
            .child(div().px(px(5.)).border_1().border_color(p.edge).child(mono("/", size::META, p.dim_ch)));
        let items = list(self.rail_list.clone(), cx.processor(|this, ix, _window, cx| this.render_rail_item(ix, cx))).size_full();
        v_flex()
            .w(px(width))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(p.hair)
            .child(
                h_flex()
                    .px(px(22.))
                    .pt(px(24.))
                    .items_baseline()
                    .child(section(label::SESSIONS, fonts::JP_SESSIONS, p))
                    .child(div().flex_1())
                    .when(self.client.is_some(), |d| {
                        d.child(tbtn("new", label::NEW, p.dim_ch, p.ink).on_click(cx.listener(|this, _, _, cx| {
                            this.create();
                            cx.notify();
                        })))
                    })
                    .child(fold("rail-fold", "‹", "Hide sessions", true, p, cx).ml(px(16.))),
            )
            .child(find)
            .when(!notes.is_empty(), |d| {
                d.child(v_flex().px(px(22.)).pt(px(8.)).gap(px(4.)).children(
                    notes.into_iter().map(|(t, c)| div().font_family(fonts::MONO).text_px(size::META).text_color(c).child(t.clone())),
                ))
            })
            .child(chips)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .children(self.rail_fx.quad.map(|(y, h)| {
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(y))
                            .h(px(h))
                            .bg(p.raised)
                            .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(p.you_bar))
                    }))
                    .child(items)
                    .child(crate::scrollbar::rows(&self.rail_list, 0., &self.bars.rail).ground(crate::scrollbar::Ground::Chrome)),
            )
            .child(self.render_foot(p, cx))
    }

    /// One item of the rail's list (only the visible ones are built).
    pub(crate) fn render_rail_item(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let p = theme::palette(cx);
        let rail = &self.rail.rail;
        let Some(item) = rail.items.get(ix) else { return div().into_any_element() };
        match item {
            // A group's head (the first: no rule above it) that folds, as a
            // bucket does: its chevron in the left inset, so the label
            // lines up with the other groups'.
            Item::Favs { open, count } => {
                let chevron = if *open { glyph::Glyph::FoldOpen } else { glyph::Glyph::Fold };
                h_flex()
                    .id("favs-head")
                    .debug_selector(|| "favs-head".into())
                    .relative()
                    .w_full()
                    .px(px(22.))
                    .pt(px(17.))
                    .pb(px(7.))
                    .items_baseline()
                    .gap(px(9.))
                    .cursor_pointer()
                    .map(|d| self.lit(d, crate::motion::key(RAIL_TOUCH, crate::app::tkey("favs-head", 0)), (p.chrome, p.raised, p.press_ch), cx))
                    .child(div().absolute().left(px(7.)).top(px(19.)).child(glyph::draw(chevron, p.rest_mark)))
                    .child(display(label::FAVOURITES, size::D_S, p.dim_ch))
                    .child(div().font_family(fonts::JP).text_px(size::JP).text_color(p.dim_ch).child(fonts::JP_FAVOURITES))
                    .child(div().flex_1())
                    .child(mono(count, size::META, p.dim_ch))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_favs_group(cx)))
                    .into_any_element()
            }
            Item::Group { group, count } => h_flex()
                .w_full()
                .px(px(22.))
                .pt(px(17.))
                .pb(px(7.))
                .items_baseline()
                .gap(px(9.))
                .when(ix > 0, |d| d.border_t_1().border_color(p.hair))
                .child(display(group.label(), size::D_S, p.dim_ch))
                .child(div().font_family(fonts::JP).text_px(size::JP).text_color(p.dim_ch).child(group.jp()))
                .child(div().flex_1())
                .child(mono(count, size::META, p.dim_ch))
                .into_any_element(),
            Item::Row(r) => self.render_row2(*r, &p, cx),
            Item::Hist(r) => self.render_hist(*r, &p, cx),
            Item::Bucket { bucket, open, count } => {
                let b = *bucket;
                let color = if *open { p.prose_ch } else { p.dim_ch };
                h_flex()
                    .id(("bucket", b as usize))
                    .w_full()
                    .px(px(22.))
                    .pt(px(7.))
                    .pb(px(6.))
                    .gap(px(10.))
                    .items_baseline()
                    .cursor_pointer()
                    .map(|d| self.lit(d, crate::motion::key(RAIL_TOUCH, crate::app::tkey("bucket", b as usize)), (p.chrome, p.raised, p.press_ch), cx))
                    .child(div().w(px(10.)).flex_none().child(mono(if *open { "−" } else { "+" }, size::META, color)))
                    .child(display(b.label(), size::D_S, color))
                    .child(div().flex_1())
                    .child(mono(count, size::META, p.dim_ch))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_bucket(b, cx)))
                    .into_any_element()
            }
            Item::More { bucket, text } => {
                let b = *bucket;
                div()
                    .id(("more", b as usize))
                    .w_full()
                    .pl(px(42.))
                    .pr(px(22.))
                    .pt(px(5.))
                    .pb(px(8.))
                    .cursor_pointer()
                    .child(display(text, size::D_S, p.dim_ch))
                    .map(|d| self.lit(d, crate::motion::key(RAIL_TOUCH, crate::app::tkey("more", b as usize)), (p.chrome, p.raised, p.press_ch), cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expanded |= b.bit();
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Item::Note(t) => div()
                .px(px(22.))
                .pt(px(2.))
                .pb(px(10.))
                .font_family(fonts::MONO)
                .text_px(size::META)
                .line_height(px(18.))
                .text_color(p.dim_ch)
                .child(SharedString::from(t))
                .into_any_element(),
            Item::Empty(t) => div().px(px(22.)).py(px(10.)).text_px(size::TITLE).text_color(p.dim_ch).child(*t).into_any_element(),
        }
    }

    /// A PINNED or OPEN row: bar, glyph, title, age; then the state word
    /// (if not at rest), the folder or a named session's title, the cost.
    /// Its state is read per frame (the attached session runs ahead of the
    /// poll), its strings come from the cache; it never changes shape.
    fn render_row2(&self, r: usize, p: &Palette, cx: &Context<Self>) -> AnyElement {
        // No readout under an open menu (a right-click's pointer rests on
        // the row it opened from).
        let menu_open = self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some();
        let row = &self.rail.rail.rows[r];
        let meta = row.live.and_then(|i| self.model.sessions.get(i));
        let st = meta.map(|s| self.model.state(s)).unwrap_or(row.state);
        let on = meta.is_some_and(|s| self.model.is_active(&s.id));
        // The marker follows the visible tab (also while its attach is on
        // its way); a session open in a background tab has a faint one.
        let shown = meta.is_some_and(|s| self.model.is_shown(&s.id));
        let in_tab = !shown && meta.is_some_and(|s| self.model.tabs.holds(&s.id));
        // The attached session's cost as the panel shows it (its digests
        // run ahead of the 3 s poll), else the poll's.
        let cost = meta.map(|s| if on { self.model.stats.cost.unwrap_or(s.cost_usd) } else { s.cost_usd }).filter(|c| *c > 0.0);
        let waiting = meta.map(|s| if on { self.model.transcript.pending_prompts().len() } else { s.awaiting_input }).unwrap_or(0);
        let workers = if st == State::Workers { meta.map_or(0, |s| self.model.workers(s)) } else { 0 };
        let gone = row.gone && meta.is_none();
        let title_color = match st {
            _ if gone => p.dim_ch,
            State::NeedsYou | State::Running | State::Workers | State::Unread => p.ink,
            _ if shown => p.ink,
            State::Parked | State::Locked => p.dim_ch,
            _ if row.fresh => p.dim_ch,
            _ => p.prose_ch,
        };
        let sub_color = p.dim_ch;
        let key = row.key;
        let fx = &self.fx;
        // While the selection quad moves, it is this row's fill and bar.
        let lit = !(self.rail_fx.moving && self.rail_fx.sel == Some(key));
        // NEEDS YOU's bar draws top to bottom over `base` (motion.md §6.3).
        let needs = st == State::NeedsYou;
        let since = fx.change(crate::motion::Tag::RowNeeds, key, needs as u64, None).filter(|_| needs).map(|c| c.0);
        let bar = if needs {
            Some((p.warn_mark_ch, fx.draw(since, std::time::Duration::ZERO, crate::motion::BASE)))
        } else if shown && lit {
            Some((p.you_bar, 1.))
        } else if in_tab {
            Some((p.rest_mark, 1.))
        } else {
            None
        };
        let touch = crate::motion::key(RAIL_TOUCH, key);
        let (fill, dy) = fx.fill(touch, p.chrome, (shown && lit).then_some(p.raised), p.raised, p.press_ch);
        let flip = self.rail_fx.offsets.get(&key).copied().unwrap_or(0.);
        // Line 2's room for the sub, past the word (and its count) and the
        // cost: under SUB_MIN_CELLS cells it is dropped.
        let cell = self.top_words.as_ref().map_or(7.2, |t| t.cell);
        let cells = |t: &str| t.chars().count() as f32 * cell;
        let count_w = match st {
            State::NeedsYou if waiting > 1 => 5. + cells(&count(waiting)),
            State::Workers => 5. + cells(&count(workers)),
            _ => 0.,
        };
        // A favourite whose journal is gone: GONE (rest), and Remove.
        let st_word = if gone { Some(label::GONE) } else { st.word() };
        let word_w = st_word.map_or(0., |w| self.widths.borrow().get(&(w, size::D_S.to_bits())).copied().unwrap_or(0.) + count_w + 8.);
        let cost_w = cost.map_or(0., |c| 8. + cells(&format!("${c:.2}"))) + if gone { 8. + 11. * cell } else { 0. };
        // The row's insets, its glyph and gap.
        let room = self.rail_width - 2. * 22. - 10. - 10. - word_w - cost_w;
        let sub = room >= SUB_MIN_CELLS * cell;
        let word_color = if gone { p.dim_ch } else { glyph::session_word(st, Ground::Chrome, p) };
        let word = st_word.map(|w| {
            h_flex()
                .flex_none()
                .items_baseline()
                .gap(px(5.))
                .child(display(w, size::D_S, word_color))
                .when(st == State::NeedsYou && waiting > 1, |d| d.child(mono(count(waiting), size::META, p.warn_ch)))
                .when(st == State::Workers, |d| d.child(mono(count(workers), size::META, p.run_ch)))
        });
        let lead = if gone {
            div().flex().flex_none().size(px(10.)).child(glyph::draw(glyph::Glyph::Gone, p.rest_mark))
        } else {
            glyph::session_mark(st, Ground::Chrome, p)
        };
        let remove = gone.then(|| {
            let id = row.fav.and_then(|i| self.model.favs.list.get(i)).map(|f| f.id.clone()).unwrap_or_default();
            display(label::REMOVE, size::D_S, p.dim_ch)
                .id(("rail-remove", key as usize))
                .debug_selector(|| "fav-remove".into())
                .flex_none()
                .cursor_pointer()
                .hover(|d| d.text_color(p.ink))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.remove_fav(&id);
                    cx.notify();
                }))
        });
        let el = div()
            .id(("rail-row", key as usize))
            .group("rail-row")
            .relative()
            .top(px(flip))
            .w_full()
            .px(px(22.))
            .py(px(8.))
            .cursor_pointer()
            .when_some(fill, |d, c| d.bg(c))
            .when_some(bar, |d, (c, f)| d.child(div().absolute().left_0().top_0().h(relative(f)).w(px(if in_tab { 1. } else { 2. })).bg(c)))
            .child(
                h_flex().relative().top(px(dy)).gap(px(10.)).items_start().child(lead.mt(px(5.))).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(
                            h_flex()
                                .h(px(18.))
                                .gap(px(10.))
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .when(row.named, |d| d.font_family(fonts::MONO).text_px(size::META))
                                        .when(!row.named, |d| d.text_px(size::TITLE))
                                        .text_color(title_color)
                                        .child(SharedString::from(&row.title)),
                                )
                                // The daemon pins it too (it is not in PINNED).
                                .when(row.pinned, |d| {
                                    d.child(
                                        div()
                                            .flex()
                                            .flex_none()
                                            .debug_selector(|| "pin-mark".into())
                                            .child(glyph::draw(glyph::Glyph::Pinned, p.rest_mark)),
                                    )
                                })
                                .child(mono(&row.age, size::META, p.dim_ch))
                                .child(self.star(key, row.fav.is_some(), 18., p, cx)),
                        )
                        .child(
                            h_flex()
                                .h(px(17.))
                                .gap(px(8.))
                                .items_center()
                                .min_w_0()
                                .children(word)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .when(row.sub_mono, |d| d.font_family(fonts::MONO).text_px(size::META))
                                        .when(!row.sub_mono, |d| d.text_px(size::TITLE))
                                        .text_color(sub_color)
                                        .when(sub, |d| d.child(SharedString::from(&row.sub))),
                                )
                                .when_some(cost, |d, c| d.child(mono(format!("${c:.2}"), size::META, p.cost_ch).flex_none()))
                                .children(remove),
                        ),
                ),
            )
            .children(self.readout_of(key, menu_open).map(|phase| self.render_readout(row, phase, p)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.fx.set_hover(touch, *hovered);
                this.hover_row(key, *hovered, cx);
                cx.notify();
            }))
            .map(|d| AppView::pressable(d, touch, cx))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.rail_click(key, crate::tabstrip::click_place(ev, cx));
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if ev.is_middle_click() {
                    this.rail_click(key, Place::New);
                    cx.notify();
                } else if ev.is_right_click() {
                    this.open_session_menu(crate::app::SessionAt::Row(key), ev.position(), window, cx);
                }
            }));
        el.into_any_element()
    }

    /// A HISTORY row: one fixed-height line, title and age, a mark only if
    /// it is unread (or being opened).
    fn render_hist(&self, r: usize, p: &Palette, cx: &Context<Self>) -> AnyElement {
        // No readout under an open menu (a right-click's pointer rests on
        // the row it opened from).
        let menu_open = self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some();
        let row = &self.rail.rail.rows[r];
        let key = row.key;
        let title_color = match row.state {
            State::Unread => p.ink,
            State::Opening => p.prose_ch,
            _ if row.named => p.prose_ch,
            _ => p.dim_ch,
        };
        let touch = crate::motion::key(RAIL_TOUCH, key);
        let (fill, dy) = self.fx.fill(touch, p.chrome, None, p.raised, p.press_ch);
        let flip = self.rail_fx.offsets.get(&key).copied().unwrap_or(0.);
        h_flex()
            .id(("rail-hist", key as usize))
            .group("rail-row")
            .relative()
            .top(px(flip))
            .w_full()
            .h(px(25.))
            .px(px(22.))
            .gap(px(10.))
            .items_center()
            .cursor_pointer()
            .when_some(fill, |d, c| d.bg(c))
            .child(div().flex().flex_none().relative().top(px(dy)).child(glyph::session_mark(row.state, Ground::Chrome, p)))
            .child(
                div()
                    .relative()
                    .top(px(dy))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(row.named, |d| d.font_family(fonts::MONO).text_px(size::META))
                    .when(!row.named, |d| d.text_px(size::TITLE))
                    .text_color(title_color)
                    .child(SharedString::from(&row.title)),
            )
            .child(mono(&row.age, size::META, p.dim_ch).relative().top(px(dy)))
            .child(self.star(key, false, 25., p, cx))
            .children(self.readout_of(key, menu_open).map(|phase| self.render_readout(row, phase, p)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.fx.set_hover(touch, *hovered);
                this.hover_row(key, *hovered, cx);
                cx.notify();
            }))
            .map(|d| AppView::pressable(d, touch, cx))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.rail_click(key, crate::tabstrip::click_place(ev, cx));
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if ev.is_middle_click() {
                    this.rail_click(key, Place::New);
                    cx.notify();
                } else if ev.is_right_click() {
                    this.open_session_menu(crate::app::SessionAt::Row(key), ev.position(), window, cx);
                }
            }))
            .into_any_element()
    }

    /// A row's favourite star, at its line 1's end (design.md §5: a
    /// reveal action keeps its slot always, so nothing moves): a favourite's
    /// filled, always drawn (accent through `focus_ch`: it is yours); any
    /// other row's outline, drawn while the row is hovered. A click
    /// (favourite or not) is the star's only: the row does not open.
    fn star(&self, key: u64, fav: bool, h: f32, p: &Palette, cx: &Context<Self>) -> Stateful<Div> {
        let mark = if fav {
            div().flex().flex_none().size(px(10.)).child(glyph::draw(glyph::Glyph::Starred, p.focus_ch))
        } else {
            glyph::reveal(glyph::Glyph::Star, "rail-row", "rail-star", false, p.dim_ch, p.ink)
        };
        let tip = if fav { "Unfavourite" } else { "Favourite (this desktop only: the daemon is not told)" };
        h_flex()
            .id(("rail-star", key as usize))
            .debug_selector(move || format!("star {key}"))
            .group("rail-star")
            .flex_none()
            // A wider target than the glyph, laid out as 10 px.
            .w(px(18.))
            .mx(px(-4.))
            .h(px(h))
            .justify_center()
            .items_center()
            .cursor_pointer()
            .child(mark)
            .tooltip(move |window, cx| crate::overlay::tip(tip, window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_fav_row(key);
                cx.notify();
            }))
    }

    /// The hover readout, beside the row over the session pane: drawn on
    /// top of everything (not clipped by the rail's scroll), only after the
    /// pointer has rested 400 ms.
    /// The readout of row `key` this frame, if it shows (or is leaving):
    /// its strength, its x offset (its rise from the row's side) and y
    /// (following from the row before, or sinking back as it leaves).
    fn readout_of(&self, key: u64, menu_open: bool) -> Option<(f32, f32, f32)> {
        if menu_open {
            return None;
        }
        if self.readout == Some(key) {
            let (a, rise) = self.fx.enter(self.readout_at);
            return Some((a, -rise, self.rail_fx.readout_dy));
        }
        let (k, at) = self.readout_leaving?;
        if k != key {
            return None;
        }
        let (a, sink) = self.fx.leave(at)?;
        Some((a, -sink, self.rail_fx.readout_dy))
    }

    fn render_readout(&self, row: &rail::Row, (strength, dx, dy): (f32, f32, f32), p: &Palette) -> AnyElement {
        let now = now_secs();
        let fav = row.fav.and_then(|i| self.model.favs.list.get(i));
        let r = match (row.live.and_then(|i| self.model.sessions.get(i)), row.saved.and_then(|s| rail::saved_entry(&self.model, s))) {
            (Some(s), _) => rail::readout_live(&self.model, s, now),
            (None, Some(v)) if !row.gone => rail::readout_saved(&self.model, v, now),
            _ => match fav {
                Some(f) => rail::readout_fav(&self.model, f, now),
                None => return div().into_any_element(),
            },
        };
        // The overlay anatomy (design.md §8.13): the title, then the facts.
        let facts = r.lines.into_iter().map(|(k, value, aside)| crate::overlay::Fact::new(k, value).aside(aside));
        let body = crate::overlay::frame(v_flex(), p)
            .w(px(296.))
            .p(px(16.))
            .gap(px(8.))
            .child(div().text_px(size::TITLE).line_height(px(18.)).text_color(p.ink).child(r.title))
            .child(crate::overlay::facts(facts, p));
        div()
            .absolute()
            .top_0()
            .left_0()
            .child(
                deferred(
                    anchored().offset(point(px(self.rail_width + 12. + dx), px(2. + dy))).child(crate::motion::appear(body, strength)),
                )
                .with_priority(2),
            )
            .into_any_element()
    }

    pub(crate) fn render_right(&self, width: f32, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let s = &self.model.stats;
        // Detached: the last numbers seen, in the dim roles and labelled,
        // not live. A cached tab whose attach is on its way: the same, as
        // last seen.
        let detached = s.detached && self.model.active.is_none();
        let cached = !detached && self.opening();
        let (frame, faded) = (p, p.faded());
        let p = if detached || cached { &faded } else { p };
        let window = s.view.as_ref().map(|v| v.context_window).filter(|&w| w > 0);
        let ratio = s.context_ratio();
        let filled = ratio.map(|r| ((r * SEGMENTS as f64).round() as usize).min(SEGMENTS)).unwrap_or(0);
        let hot = ratio.is_some_and(|r| r >= HOT);
        let fx = &self.fx;
        let bar = self.gauge_bar(METER, ratio, p);
        let mut segs = h_flex().gap(px(2.)).h(px(8.)).mt(px(20.));
        for i in 0..SEGMENTS {
            segs = segs.child(div().flex_1().h_full().bg(fx.segment(METER | 2, filled, i, p.edge, bar)));
        }
        // Changed numbers afterglow (motion.md §6.6).
        let glow = |n: u64, v: &str, c: Hsla| fx.glow(GLOW | n, vhash(v), c);
        let scale: SharedString = match (s.context, window) {
            (Some(c), Some(w)) => format!("{} of {} tokens", compact(c), compact(w)).into(),
            (None, Some(_)) => "no model call seen yet".into(),
            _ => "the daemon has not sent a window size".into(),
        };
        let context = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .border_b_1()
            .border_color(p.hair)
            .child(h_flex().items_center().child(section(label::CONTEXT, fonts::JP_CONTEXT, p)).child(div().flex_1()).child(fold(
                "right-fold",
                "›",
                "Hide context, tokens and files",
                false,
                p,
                cx,
            )))
            .child(
                h_flex()
                    .mt(px(20.))
                    .items_end()
                    .child(match ratio {
                        // Bottom-aligned with the `%` lifted onto the
                        // number's baseline (GPUI flex has no text baseline).
                        Some(r) => h_flex()
                            .items_end()
                            .gap(px(3.))
                            .child({
                                let v = format!("{:.1}", r * 100.0);
                                let c = glow(1, &v, if hot { p.err_ch } else { p.ink });
                                display(v, 46., c).line_height(px(46.))
                            })
                            .child(display("%", 16., p.dim_ch).line_height(px(16.)).pb(px(5.)))
                            .into_any_element(),
                        None => display("—", 46., p.dim_ch).into_any_element(),
                    })
                    .child(div().flex_1())
                    .when_some(window, |d, w| {
                        let v = compact(w);
                        let c = glow(2, &v, p.prose_ch);
                        d.child(v_flex().items_end().gap(px(5.)).child(display(label::WINDOW, size::D_S, p.dim_ch)).child(mono(v, 13., c)))
                    }),
            )
            .child(segs)
            .child(mono(scale, size::META, if hot { p.err_ch } else { p.dim_ch }).mt(px(9.)));
        let t = s.tokens;
        let dash = || SharedString::from("—");
        let num = |f: fn(&crate::stats::Tokens) -> u64| t.as_ref().map(|t| SharedString::from(compact(f(t)))).unwrap_or_else(dash);
        let hit = t.and_then(|t| t.cache_hit()).map(|h| SharedString::from(format!("{:.1}%", h * 100.0))).unwrap_or_else(dash);
        let cost = s.cost.map(|c| SharedString::from(format!("${c:.2}"))).unwrap_or_else(dash);
        let tokens = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .border_b_1()
            .border_color(p.hair)
            .child(section(label::TOKENS, fonts::JP_TOKENS, p))
            .child(
                h_flex()
                    .mt(px(20.))
                    .child(div().flex_1().child({
                        let v = num(|t| t.input);
                        kv(label::IN, v.clone(), false, glow(3, &v, p.ink), p)
                    }))
                    .child(div().flex_1().child({
                        let v = num(|t| t.output);
                        kv(label::OUT, v.clone(), false, glow(4, &v, p.ink), p)
                    }))
                    .child(div().flex_1().child({
                        let v = num(|t| t.cache_read);
                        kv(label::CACHE, v.clone(), false, glow(5, &v, p.ink), p)
                    }))
                    .child(div().flex_1().child({
                        let v = num(|t| t.cache_creation);
                        kv(label::WRITE, v.clone(), false, glow(6, &v, p.ink), p)
                    })),
            )
            .child(
                h_flex()
                    .mt(px(22.))
                    .child(div().flex_1().child(kv(label::CACHE_HIT, hit.clone(), true, glow(7, &hit, p.ink), p)))
                    .child(div().flex_1().child(kv(label::COST, cost.clone(), true, glow(8, &cost, p.cost_ch), p))),
            );
        let ledger = &s.files;
        let (added, removed) = ledger.totals();
        // An estimate is never shown as an exact count (`files::changed_lines`).
        let tilde = |approx: bool| if approx { "~" } else { "" };
        let approx = tilde(ledger.approx());
        let mut files = v_flex()
            .px(px(24.))
            .pt(px(24.))
            .pb(px(26.))
            .child(h_flex().items_end().child(section(label::FILES, fonts::JP_FILES, p)).child(div().flex_1()).when(
                added + removed > 0,
                |d| {
                    d.child(h_flex().gap(px(8.)).child(mono(format!("{approx}+{added}"), size::META, p.ok_ch)).child(mono(
                        format!("−{removed}"),
                        size::META,
                        p.err_ch,
                    )))
                },
            ))
            // The session's folder (design.md §8.18): the paths under it are
            // relative to it. Cut from the left, so its leaf stays.
            .when_some(self.session_cwd(((width - 48.) / (12. * 0.6)).floor() as usize), |d, (cwd, whole)| {
                d.child(
                    mono(cwd, 12., p.prose_ch)
                        .id("files-cwd")
                        .mt(px(8.))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .tooltip(move |window, cx| crate::overlay::tip(whole.clone(), window, cx)),
                )
            })
            // Over a worker's view: still the session's thread's, not the
            // worker's on screen.
            .child(
                match self.model.watch.focused() {
                    Some(w) => mono(w.files_note.clone(), 12., p.dim_ch),
                    None => mono("as the model asked: tool inputs, not the disk", 12., p.dim_ch),
                }
                // At 12 px it is wider than a narrow pane: it wraps.
                .whitespace_normal()
                .mt(px(6.)),
            )
            .child(div().h(px(12.)));
        for f in ledger.files().iter().take(FILES_SHOWN) {
            files = files.child(
                h_flex()
                    .h(px(27.))
                    .gap(px(10.))
                    .items_center()
                    .child(
                        mono(
                            f.kind.letter(),
                            size::META,
                            match f.kind {
                                crate::files::FileKind::Modified => p.edit_ch,
                                crate::files::FileKind::Written => p.write_ch,
                            },
                        )
                        .w(px(10.))
                        .flex_none(),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                mono(f.dir.clone(), size::META, p.dim_ch).flex_shrink(1.).min_w(px(14.)).overflow_hidden().text_ellipsis(),
                            )
                            .child(mono(f.name.clone(), size::META, p.ink).flex_none()),
                    )
                    // A write is the lines written, not a delta (`~`).
                    .when(f.written > 0, |d| d.child(mono(format!("~{}", f.written), size::META, p.dim_ch).flex_none()))
                    .when(f.added > 0 || f.written == 0, |d| {
                        d.child(mono(format!("{}+{}", tilde(f.approx), f.added), size::META, p.ok_ch).flex_none())
                    })
                    .when(f.removed > 0, |d| d.child(mono(format!("−{}", f.removed), size::META, p.err_ch).flex_none())),
            );
        }
        let n = ledger.files().len();
        if n > FILES_SHOWN {
            files = files.child(mono(format!("+{} more files", n - FILES_SHOWN), size::META, p.dim_ch).mt(px(6.)));
        }
        let shell = ledger.shell();
        if n == 0 && shell.is_empty() {
            files = files.child(div().text_px(size::TITLE).text_color(p.dim_ch).child("No file edits in this session"));
        }
        if !shell.is_empty() {
            // Read from `bash` command lines: approximate, no line counts,
            // not in the totals above.
            files = files
                .child(
                    h_flex()
                        .mt(px(if n == 0 { 0. } else { 18. }))
                        .gap(px(10.))
                        .items_baseline()
                        .child(display(label::VIA_SHELL, size::D_S, p.prose_ch))
                        .child(mono("~", size::META, p.dim_ch)),
                )
                // The FILES note's size (design.md §8.18); it wraps.
                .child(mono("bash commands that succeeded; no counts", 12., p.dim_ch).whitespace_normal().mt(px(6.)))
                .child(div().h(px(8.)));
            for f in shell.iter().take(FILES_SHOWN) {
                files = files.child(
                    h_flex()
                        .h(px(25.))
                        .gap(px(10.))
                        .items_center()
                        .child(mono("$", size::META, p.dim_ch).w(px(10.)).flex_none())
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(
                                    mono(f.dir.clone(), size::META, p.dim_ch)
                                        .flex_shrink(1.)
                                        .min_w(px(14.))
                                        .overflow_hidden()
                                        .text_ellipsis(),
                                )
                                .child(mono(f.name.clone(), size::META, p.prose_ch).flex_none()),
                        )
                        .child(mono(f.verb, size::META, p.dim_ch).flex_none()),
                );
            }
            if shell.len() > FILES_SHOWN {
                files = files.child(mono(format!("+{} more", shell.len() - FILES_SHOWN), size::META, p.dim_ch).mt(px(6.)));
            }
        }
        v_flex()
            .id("right")
            .w(px(width))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .track_scroll(&self.bars.right.scroll)
            .border_l_1()
            .border_color(frame.hair)
            .when(detached || cached, |d| {
                let p = frame;
                d.child(
                    h_flex()
                        .px(px(24.))
                        .py(px(10.))
                        .gap(px(10.))
                        .border_b_1()
                        .border_color(p.hair)
                        .child(display(
                            if cached { "C A C H E D" } else { "D E T A C H E D" },
                            size::D_S,
                            if cached { p.dim_ch } else { p.warn_ch },
                        ))
                        .child(mono(if cached { "as last seen · opening…" } else { "last numbers seen" }, size::META, p.dim_ch)),
                )
            })
            .child(context)
            .child(tokens)
            .child(files)
    }

    /// The theme picker: SYNAPS CONFIG (follow the TUI's `theme =`, the
    /// default), the 20 built-in themes (each with the three colours that
    /// tell them apart: canvas, `prompt_fg`, `claude_label`), then the
    /// accent override (THEME: none; the presets; a custom hex). Over the
    /// open right-hand column when it is wide enough, else above the
    /// composer at the right: never over the tab strip's `+`. It
    /// scrolls when the window is too short for it. A click applies and
    /// saves; Esc (the root's key handler) or a click outside closes it.
    pub(crate) fn render_picker(&self, p: &Palette, right: Side, cx: &Context<Self>) -> impl IntoElement {
        let scheme = cx.global::<theme::Scheme>();
        let current = scheme.choice.theme.clone();
        let accent = scheme.choice.accent;
        let follows = self.theme_follows_config;
        let swatch = |c: u32| div().size(px(10.)).flex_none().bg(rgb(c));
        let config = h_flex()
            .id("theme-config")
            .mt(px(12.))
            .h(px(26.))
            .px(px(6.))
            .gap(px(8.))
            .items_center()
            .cursor_pointer()
            .map(|d| self.lit_rest(d, crate::app::tkey("theme-config", 0), (p.chrome, p.raised, p.press_ch), follows.then_some(p.raised), cx))
            .child(div().size(px(12.)).flex_none().border_1().border_color(p.dim_ch))
            .child(display(label::SYNAPS_CONFIG, size::D_S, if follows { p.ink } else { p.prose_ch }).flex_none())
            .child(mono(format!("· {}", self.config_theme), size::META, p.dim_ch).flex_1().min_w_0().overflow_hidden().text_ellipsis())
            .child(mono(if follows { "✓" } else { " " }, size::META, p.focus_ch).w(px(10.)))
            .on_click(cx.listener(|this, _, _, cx| this.follow_config(cx)));
        let mut grid = h_flex().mt(px(2.)).flex_wrap().gap_y(px(2.));
        for (i, &(name, raw)) in crate::palettes::THEMES.iter().enumerate() {
            let on = !follows && name == current;
            grid = grid.child(
                h_flex()
                    .id(("theme", i))
                    .w(relative(0.5))
                    .h(px(26.))
                    .px(px(6.))
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .map(|d| self.lit_rest(d, crate::app::tkey("theme", i), (p.chrome, p.raised, p.press_ch), on.then_some(p.raised), cx))
                    .child(
                        h_flex()
                            .flex_none()
                            .border_1()
                            .border_color(p.edge)
                            .child(swatch(raw.message_bg))
                            .child(swatch(raw.prompt_fg))
                            .child(swatch(raw.claude_label)),
                    )
                    .child(mono(name, size::META, if on { p.ink } else { p.prose_ch }).flex_1().min_w_0().overflow_hidden().text_ellipsis())
                    .child(mono(if on { "✓" } else { " " }, size::META, p.focus_ch).w(px(10.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_theme(name, cx))),
            );
        }
        let mut presets = h_flex().mt(px(10.)).flex_wrap().gap_y(px(2.));
        let off = accent.is_none();
        presets = presets.child(
            h_flex()
                .id("accent-off")
                .w(relative(0.5))
                .h(px(26.))
                .px(px(6.))
                .gap(px(8.))
                .items_center()
                .cursor_pointer()
                .map(|d| self.lit_rest(d, crate::app::tkey("accent-off", 0), (p.chrome, p.raised, p.press_ch), off.then_some(p.raised), cx))
                .child(div().size(px(12.)).flex_none().border_1().border_color(p.dim_ch))
                .child(display(label::THEME, size::D_S, if off { p.ink } else { p.prose_ch }).flex_1())
                .child(mono(if off { "✓" } else { " " }, size::META, p.focus_ch).w(px(10.)))
                .on_click(cx.listener(|this, _, _, cx| this.set_accent(None, cx))),
        );
        for (i, &(name, hex)) in theme::PRESETS.iter().enumerate() {
            let on = accent == Some(hex);
            presets = presets.child(
                h_flex()
                    .id(("preset", i))
                    .w(relative(0.5))
                    .h(px(26.))
                    .px(px(6.))
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .map(|d| self.lit_rest(d, crate::app::tkey("preset", i), (p.chrome, p.raised, p.press_ch), on.then_some(p.raised), cx))
                    .child(div().size(px(12.)).flex_none().bg(rgb(hex)))
                    .child(display(name, size::D_S, if on { p.ink } else { p.prose_ch }).flex_1().min_w_0().overflow_hidden())
                    .child(mono(if on { "✓" } else { " " }, size::META, p.focus_ch).w(px(10.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_accent(Some(hex), cx))),
            );
        }
        let lifted = accent.is_some_and(|a| a != scheme.roles.you_bar || a != scheme.roles.focus);
        let (top, bottom) = (top_h(self.layout.short), 140.);
        // Over the open right panel, at least two columns of names wide,
        // when that stays clear of the tab strip's `+` (it ends at the
        // column's right edge); else above the composer, under the tab
        // strip.
        let place = match right {
            Side::Open(w) if w.max(PICKER_MIN) - w + 8. <= self.layout.right => PickerAt::Top(w.max(PICKER_MIN)),
            _ => PickerAt::Bottom(right.width() + 24.),
        };
        let max_h = match place {
            PickerAt::Top(_) => self.height - top - 16.,
            PickerAt::Bottom(_) => self.height - bottom - top - if self.layout.short { TABS_CLEAR_SHORT } else { TABS_CLEAR },
        };
        let body = v_flex()
            .flex_none()
            .p(px(18.))
            .child(
                h_flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(display(label::THEME, size::D_S, p.header))
                    .child(mono(current.clone(), size::META, p.dim_ch))
                    .child(div().flex_1())
                    .child(tbtn("picker-close", label::CLOSE, p.dim_ch, p.ink).on_click(cx.listener(|this, _, window, cx| {
                        this.close_picker(window, cx);
                    }))),
            )
            .child(config)
            .child(grid)
            .child(h_flex().mt(px(18.)).items_baseline().gap(px(12.)).child(display(label::ACCENT, size::D_S, p.header)).child(mono(
                accent.map(theme::to_hex).unwrap_or_else(|| "the theme's".into()),
                size::META,
                p.dim_ch,
            )))
            .child(mono("replaces the theme's “you” and focus colours; never a state", size::META, p.dim_ch).mt(px(6.)).whitespace_normal())
            .child(presets)
            .child(
                h_flex()
                    .mt(px(12.))
                    .gap(px(12.))
                    .items_center()
                    .child(display(label::CUSTOM, size::D_S, p.dim_ch).w(px(62.)))
                    .child(div().flex_1().child(Input::new(&self.hex_input)))
                    .child(
                        tbtn("picker-apply", label::APPLY, p.prose_ch, p.ink)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_custom_accent(cx))),
                    ),
            )
            .when(lifted, |el| {
                el.child(
                    mono("lifted to stay readable: 3:1 for bars, 4.5:1 for text", size::META, p.dim_ch).mt(px(10.)).whitespace_normal(),
                )
            })
            .when_some(self.picker_note.clone(), |el, n| el.child(mono(n, size::META, p.dim_ch).mt(px(10.)).whitespace_normal()));
        v_flex()
            .id("picker")
            .absolute()
            .map(|d| match place {
                PickerAt::Top(w) => d.top(px(top)).right_0().w(px(w)),
                PickerAt::Bottom(r) => d.bottom(px(bottom)).right(px(r)).w(px(PICKER_MIN)),
            })
            .max_h(px(max_h.max(160.)))
            .overflow_y_scroll()
            .track_scroll(&self.bars.picker.scroll)
            .bg(p.chrome)
            .border_1()
            .border_color(p.edge)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                this.picker_dismissed = Some(ev.position);
                this.close_picker(window, cx);
            }))
            .child(body)
    }
}

/// Where the picker opens: over the open right panel (its width), or above
/// the composer this far from the window's right edge.
#[derive(Clone, Copy)]
enum PickerAt {
    Top(f32),
    Bottom(f32),
}

/// A chip's count: two mono cells whatever it is (a no-break space after
/// one digit), so a count crossing 10 moves nothing; 100 and more is
/// `99+`, the one count a cell wider.
fn chip_count(n: usize) -> String {
    if n > 99 { "99+".into() } else { format!("{n:\u{a0}<2}") }
}

#[cfg(test)]
mod tests {
    use super::{FIND, RailCache, chip_count, find_fits, prompt_gist};
    use crate::model::{ConnState, Model};
    use crate::rail::{self, Item};
    use gpui_kit::{ListAlignment, ListState, px};
    use serde_json::{Value, json};
    use synaps_client::Event;
    use synaps_client::protocol::SessionEvent;

    /// R6: a folded FAVOURITES shows a favourite that runs (M4). The
    /// attached session's run starts by an event that changes no list
    /// (`rail_gen` stays): the cache rebuilds for it all the same, and again
    /// when it stops.
    #[test]
    fn a_folded_favourites_rebuilds_when_a_favourite_starts_running() {
        let mut m = Model::new(ConnState::Connecting);
        let meta = json!({"id": "s", "journal_id": "j", "lifecycle": "live", "title": "Relay", "created_at": "2026-11-01T00:00:00Z"});
        m.apply(Event::Sessions(serde_json::from_value(json!([meta.clone()])).unwrap()));
        m.favs.toggle(crate::favourites::Source::Live(&m.sessions[0].clone()), 1);
        let frame = json!({"type": "attached", "client": 2, "meta": meta, "conversation": {"api_messages": []}, "input_owner": 2});
        let synaps_client::protocol::DaemonFrame::Attached(attached) = synaps_client::protocol::decode_frame(&frame.to_string()).unwrap()
        else {
            panic!()
        };
        m.want(1);
        m.apply(Event::Attached { attach_seq: 1, attached, mode: synaps_client::AttachMode::Mirror, reconnect: false });
        m.sync_favs();
        let view = rail::View { favs_closed: true, ..Default::default() };
        let list = ListState::new(0, ListAlignment::Top, px(300.));
        let shows = |c: &RailCache| c.rail.items.iter().any(|i| matches!(i, Item::Row(r) if &*c.rail.rows[*r].title == "Relay"));
        let mut cache = RailCache::default();
        cache.refresh(&m, &view, &list, false);
        assert!(!shows(&cache), "folded, at rest: under the head's count only");
        let event = |event: SessionEvent| Event::session(1, "s", 0, event, std::sync::Arc::new(Value::Null));
        let before = m.rail_gen;
        m.apply(event(SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: None }));
        assert_eq!(m.rail_gen, before, "a run starting is no list change");
        cache.refresh(&m, &view, &list, false);
        assert!(shows(&cache), "running: it shows under the folded head");
        m.apply(event(SessionEvent::Idle));
        cache.refresh(&m, &view, &list, false);
        assert!(!shows(&cache), "done: folded again");
    }

    /// Re-review N5: every count is two cells (`99+` past 99), so the chips
    /// hold one line at the default rail whatever the counts. Their labels
    /// at `d.s` 12 measured 194.4 px together (bella, 255 px with one-digit
    /// counts at the old 12/5 gaps).
    #[test]
    fn the_chips_hold_one_line_at_the_default_rail() {
        assert_eq!([0, 7, 10, 99, 100, 4321].map(chip_count), ["0\u{a0}", "7\u{a0}", "10", "99", "99+", "99+"]);
        let (labels, cell, inside) = (194.4, 7.2, crate::app::FLOORS.0 - 2. * 22.);
        let widest_two_cells = labels + 3. * (4. + 2. * cell) + 2. * 8.;
        assert!(widest_two_cells <= inside, "{widest_two_cells} of {inside}");
    }

    /// Finish gate F2: the find field's placeholder is whole or its first
    /// word, never cut mid-word: whole at the default rail, the word alone
    /// at the 220-px limit (it read "find · open by nar").
    #[test]
    fn the_find_placeholder_is_never_cut() {
        let cell = 7.2;
        assert!(find_fits(crate::app::FLOORS.0, cell), "the default rail: all of it");
        assert!(!find_fits(220., cell), "220 px: {}", FIND.1);
        let edge = (220..480).find(|&w| find_fits(w as f32, cell)).expect("a width that fits it");
        assert!((220..240).contains(&edge), "whole from {edge} px");
    }

    #[test]
    fn the_ticker_names_the_tool_and_what_it_runs() {
        // The command the message introduces, not the introduction.
        let body = "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/w:\n$ cargo test -p relay\n\nAllow this tool call?";
        assert_eq!(prompt_gist("Confirm tool call", body), "bash: cargo test -p relay");
        let crlf = body.replace('\n', "\r\n");
        assert_eq!(prompt_gist("Confirm tool call", &crlf), "bash: cargo test -p relay", "a CRLF body reads the same");
        // No command: the message, once named, with no dangling colon.
        let bare = "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/w:\n\nAllow this tool call?";
        assert_eq!(prompt_gist("Confirm tool call", bare), "bash wants to run a command in ~/w");
        let other = "Extension: guard\nTool: edit\n\nMessage from guard:\nwrites outside the folder\n\nAllow this tool call?";
        assert_eq!(prompt_gist("Confirm tool call", other), "edit: writes outside the folder");
        let rewritten = "Extension: a, b\nTool: edit\nInput rewritten by: b\nInput that will run:\n{\"path\": \"/w/x\"}\n\nMessage from a, b:\nok?\n\nAllow this tool call?";
        assert_eq!(prompt_gist("Confirm tool call", rewritten), "edit: {\"path\": \"/w/x\"}");
        assert_eq!(prompt_gist("Confirm tool activation", "Activate shell_start?"), "Activate shell_start?");
        assert_eq!(prompt_gist("API key", ""), "API key");
    }
}
