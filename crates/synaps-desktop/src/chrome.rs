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
/// sit here is gone; the connection disc is in the top bar.
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
    pub(crate) keys: Vec<u64>,
    /// Rows on screen before the last splice: their key and top, in the
    /// list's content (Flip, motion.md §6.4).
    pub(crate) flip_from: Vec<(u64, f32)>,
    /// In the frame of a splice, where each item will be laid out (the
    /// list can't say until it lays out again): [`rail::flip_targets`].
    pub(crate) targets: Option<Vec<Option<(f32, f32)>>>,
    /// The next splice is a section's fold or unfold (FAVOURITES, a
    /// HISTORY bucket): it reveals instead of Flipping ([`Fold`]).
    pub(crate) fold: bool,
    /// That splice's fold, for [`AppView::rail_motion`] to start.
    pub(crate) folded: Option<Fold>,
    /// Each item kind's height as last laid out: a section unfolding
    /// whose rows' kind is on screen nowhere else.
    kind_h: HashMap<u8, f32>,
    /// A fold under way, as last drawn: a splice then measures from where
    /// things were drawn, not laid out (R2-L2, R3-L2).
    pub(crate) riding: Option<Riding>,
}

/// A fold under way, as last drawn ([`RailCache::riding`]): its head, the
/// first item after it, `o` (what follows is drawn `o` from its place),
/// and how far from its place each body item was drawn.
#[derive(Clone, Debug, Default)]
pub(crate) struct Riding {
    pub(crate) start: usize,
    pub(crate) after: usize,
    pub(crate) o: f32,
    pub(crate) body: Vec<f32>,
}

impl Riding {
    /// How far from its place item `ix` was drawn.
    fn dy(&self, ix: usize) -> f32 {
        match ix {
            _ if ix >= self.after => self.o,
            _ if ix > self.start => self.body.get(ix - self.start - 1).copied().unwrap_or(0.),
            _ => 0.,
        }
    }
}

/// A section folding or unfolding in the rail (fluid.md §3.7, polish.md
/// P14): the items after it are drawn `o` px from their place, `o`
/// springing to 0 on `sheet`. Unfolding, its body (the items after
/// `start`, their heights) is revealed down to them, clipped; folding,
/// the body it had (`ghost`: the rail before, and the body's items in it)
/// is drawn under its head, clipped at them as they rise over it: the
/// same reveal, run backwards.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fold {
    pub(crate) start: usize,
    pub(crate) after: usize,
    pub(crate) heights: Vec<f32>,
    /// Unfolding: each body item that was there folded (a favourite
    /// FAVOURITES kept): drawn whole, riding down as the new ones open.
    pub(crate) kept: Vec<bool>,
    pub(crate) from: f32,
    pub(crate) o: f32,
    pub(crate) ghost: Option<std::rc::Rc<Ghost>>,
}

/// A folding section's body as it was ([`Fold`]): drawn under its head as
/// what follows rises over it.
#[derive(Debug)]
pub(crate) struct Ghost {
    pub(crate) rail: Rail,
    pub(crate) body: std::ops::Range<usize>,
    /// Each body item: whether it stays (a folded FAVOURITES keeps the
    /// favourites that run or need you), and its height.
    pub(crate) items: Vec<(bool, f32)>,
}

impl Ghost {
    /// What stays, in all.
    pub(crate) fn kept(&self) -> f32 {
        self.items.iter().filter(|i| i.0).map(|i| i.1).sum()
    }

    /// Each body item's height shown with `o` px of what goes left: what
    /// stays whole, what goes top down (the last goes first, under what
    /// rises; one that stays rises with it once it reaches it). All of it
    /// going: the body clipped at `o`, the unfold's reveal backwards.
    pub(crate) fn shown(&self, o: f32) -> Vec<f32> {
        let mut left = o.max(0.);
        self.items
            .iter()
            .map(|&(kept, h)| {
                if kept {
                    return h;
                }
                let s = left.min(h);
                left -= s;
                s
            })
            .collect()
    }
}

impl Fold {
    /// Unfolding, body item `ix`: its height shown and how far from its
    /// place it is drawn (none: all of it, at its place). The fold run
    /// backwards: what was kept stays whole, and the new ones open top
    /// down, each pushing what is under it down from where it was drawn
    /// folded ([`Ghost::shown`]).
    pub(crate) fn shown(&self, ix: usize) -> Option<(f32, f32)> {
        let n = self.heights.len();
        if ix <= self.start || ix >= self.after || self.from >= 0. || n != self.after - self.start - 1 {
            return None;
        }
        let new = |j: usize| !self.kept.get(j).copied().unwrap_or(false);
        let open = (0..n).filter(|&j| new(j)).map(|j| self.heights[j]).sum::<f32>() + self.o;
        let (mut above, mut hidden) = (0f32, 0f32);
        for j in (0..ix - self.start - 1).filter(|&j| new(j)) {
            let h = self.heights[j];
            hidden += h - (open - above).clamp(0., h);
            above += h;
        }
        let h = self.heights[ix - self.start - 1];
        Some((if new(ix - self.start - 1) { (open - above).clamp(0., h) } else { h }, -hidden))
    }

    /// How far from its place item `ix` is drawn: what follows `o`; a body
    /// item unfolding as [`Fold::shown`]; one a fold keeps, in its ghost.
    fn dy(&self, ix: usize) -> f32 {
        if ix >= self.after {
            return self.o;
        }
        if ix <= self.start {
            return 0.;
        }
        if let Some((_, dy)) = self.shown(ix) {
            return dy;
        }
        let Some(g) = self.ghost.as_ref() else { return 0. };
        let shown = g.shown(self.o);
        // The k-th kept one: what goes above it, as shown.
        let mut k = ix - self.start - 1;
        let mut dy = 0.;
        for (i, s) in g.items.iter().zip(shown) {
            if i.0 {
                if k == 0 {
                    break;
                }
                k -= 1;
            } else {
                dy += s;
            }
        }
        dy
    }
}

/// The key an item slides by (Flip): a row's own key, else the head's
/// identity ([`rail::flip_id`]: its count may have changed).
fn flip_key(rail: &Rail, ix: usize) -> Option<u64> {
    rail.items.get(ix).map(|it| row_key(rail, it).unwrap_or_else(|| rail::flip_id(rail, it)))
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
    /// The items travelling farthest (a row moved against the others): drawn
    /// over the rest, clipped to the rail, so the ones it passes never hide
    /// it. Each with its top in the rail's view and its height.
    pub(crate) leads: HashMap<u64, (f32, f32)>,
    /// The rail's view, as of the last layout (the leads' clip).
    pub(crate) view: Size<Pixels>,
    pub(crate) sel: Option<u64>,
    pub(crate) quad: Option<(f32, f32)>,
    pub(crate) moving: bool,
    /// The readout's y, as it follows from the row before.
    pub(crate) readout_dy: f32,
    /// A section folding or unfolding.
    pub(crate) fold: Option<Fold>,
    /// Unfolding, the items after it that ride into view from where they
    /// were but whose place is below it (the list draws only what is in
    /// view where it lays out): each with its top in the rail's view, as
    /// [`AppView::rail_layer`] drew them last.
    pub(crate) tail: Vec<(usize, f32)>,
    /// Folding, the body it had as that layer drew it last: its top in the
    /// rail's view, the height shown, how many items.
    pub(crate) ghost_at: Option<(f32, f32, usize)>,
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
/// The side panels' shown widths (fluid.md §3.8): the rail's, the right
/// panel's.
const PANEL_W: [u64; 2] = [0x7AF0 << 48, (0x7AF0 << 48) | 1];
/// A rail section's fold: the offset of what follows it.
const FOLD_Y: u64 = 0x7AF3 << 48;
/// FAVOURITES' chevron.
const FAVS_TURN: u64 = (0x7AF3 << 48) | 1;

/// A side panel's width on its spring (fluid.md §3.8, polish.md P14).
/// Only what the app does moves it (a fold, a reset, a detent); a window
/// resize and a drag take the new width at once (`follow`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Flight {
    /// A move asked for this frame: from the width shown, on `response`.
    pub(crate) from: Option<f32>,
    pub(crate) response: f32,
    /// A fold or an unfold: the panel's content, laid out at its open
    /// width, slides under the window's side; else (a width change) it is
    /// clipped at the inner edge.
    pub(crate) slide: bool,
    /// The width drawn this frame, and whether it moves: until it is
    /// drawn at its target. Its spring settles under a pixel after that,
    /// in frames that draw the same width; the panel has landed, and the
    /// layout with it (the column re-wraps, the labels and the thumbs
    /// land) in the last frame anything moves (#47: on the spring's own
    /// stop, 6 or 7 frames later at 144 Hz, the panes still).
    pub(crate) shown: f32,
    pub(crate) moving: bool,
    /// The width its content is laid out at: the open one (while it folds,
    /// the one it had).
    pub(crate) open: f32,
    /// Where a width change started (its content is laid out at the wider
    /// of this and the new width, so it is clipped, never short of the box).
    pub(crate) start: f32,
}

/// A folded panel's strip in flight (#35: its glyphs cut in as the rail
/// landed, then lifted in over `quick`, after everything else had stopped):
/// on the panel's inner edge (#39 L1: one edge, its glyphs beside the
/// panel's, not over them), it comes in over the panel's last strip-width
/// of travel, and goes out over its first, as a crossfade of the panel's
/// edge into the strip on the panel's own spring. Linear in the width drawn ([`Motion::spring_px`],
/// whole device pixels), so it eases as the panel does, changes only in a
/// frame the width does (no frame of its own), and is whole as the panel
/// is the strip's width: the last frame that moves. It brings its own
/// opaque ground, the one it lands on (`chrome`), so it may start from 0
/// ([`crate::motion::lift_from`]). Reduced and Off: the panel lands in one
/// frame, and the strip with it.
///
/// [`Motion::spring_px`]: crate::motion::Motion::spring_px
pub(crate) fn strip_strength(shown: f32) -> f32 {
    (1. - (shown - STRIP) / STRIP).clamp(0., 1.)
}

/// Both side panels' flights, and the column's wrap width while they fly.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PanelFx {
    pub(crate) sides: [Flight; 2],
    /// The narrowest centre pane of the flight: the column is laid out
    /// for it until every panel is at rest (it rewraps once, at the start
    /// when it narrows, at the end when it widens; fluid.md §3.8).
    pub(crate) freeze: Option<f32>,
}

impl PanelFx {
    /// Panel `rail` (else the right one) moves from where it is drawn now,
    /// on `response` (`sheet` for a fold or a reset, `snap` to a detent).
    pub(crate) fn fly(&mut self, rail: bool, response: f32, slide: bool) {
        let f = &mut self.sides[usize::from(!rail)];
        // In flight already (a reversal): it turns from where it is, with
        // its velocity (fluid.md §2.1, Shady N2): only the target moves.
        if !f.moving {
            f.from = Some(f.shown);
        }
        f.start = f.shown;
        f.response = response;
        f.slide = slide;
    }

    pub(crate) fn moving(&self) -> bool {
        self.sides.iter().any(|f| f.moving)
    }
}

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
        let mut folded = None;
        let fold = std::mem::take(&mut self.fold);
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
            let old: Vec<rail::Placed> = (0..self.rail.items.len())
                .map(|ix| (self.keys[ix], rail::item_kind(&self.rail.items[ix]), item_y(list, ix).map(|(y, h, _)| (y, h))))
                .collect();
            let new: Vec<(u64, u8)> = keys.iter().zip(&rail.items).map(|(&k, it)| (k, rail::item_kind(it))).collect();
            let at = rail::flip_targets(&old, &new, range.clone(), count);
            let after = range.start + count;
            for o in &old {
                if let Some((_, h)) = o.2 {
                    self.kind_h.insert(o.1, h);
                }
            }
            if fold {
                // A section's fold: what follows rides it (no Flip); its
                // body, unfolding, is revealed.
                let h_of = |ix: usize| {
                    at.get(ix).copied().flatten().map(|t| t.1).or_else(|| new.get(ix).and_then(|n| self.kind_h.get(&n.1).copied()))
                };
                let heights: Option<Vec<f32>> = (range.start + 1..after).map(h_of).collect();
                // What follows starts where it was: the old range's end, less
                // the new one's.
                let end_of = |o: Option<(f32, f32)>| o.map(|(y, h)| y + h);
                let old_end = old
                    .get(range.end)
                    .and_then(|o| o.2)
                    .map(|g| g.0)
                    .or_else(|| range.end.checked_sub(1).and_then(|i| end_of(old.get(i).and_then(|o| o.2))));
                let new_end = old
                    .get(range.start)
                    .and_then(|o| o.2)
                    .map(|g| g.0)
                    .zip((range.start..after).map(h_of).collect::<Option<Vec<f32>>>())
                    .map(|(y, hs)| y + hs.iter().sum::<f32>());
                let from = old_end.zip(new_end).map(|(o, n)| o - n);
                let from = if count > range.len() { from.or(heights.as_ref().map(|h| -h.iter().sum::<f32>())) } else { from };
                // A reversal mid-fold (a double click on the head): from
                // where what follows was drawn, its `o` with it (R2-L2).
                let from = from.map(|f| f + self.riding.as_ref().filter(|r| r.start == range.start).map_or(0., |r| r.o));
                // Folding (to its head, or to the favourites it keeps): the
                // body it had goes under it, what stays in it whole.
                let body = range.start + 1..range.end;
                let stays: Vec<u64> = keys.get(range.start + 1..after).map(<[u64]>::to_vec).unwrap_or_default();
                let items: Option<Vec<(bool, f32)>> = body
                    .clone()
                    .map(|ix| {
                        let o = old.get(ix)?;
                        o.2.map(|g| g.1).or_else(|| self.kind_h.get(&o.1).copied()).map(|h| (stays.contains(&o.0), h))
                    })
                    .collect();
                let ghost = items
                    .filter(|it| range.len() > count && count >= 1 && it.iter().filter(|i| i.0).count() == stays.len())
                    .map(|items| std::rc::Rc::new(Ghost { rail: self.rail.clone(), body, items }));
                // Unfolding: what was there folded (the favourites it kept).
                let was = self.keys.get(range.clone()).unwrap_or_default();
                let kept = (range.start + 1..after).map(|ix| keys.get(ix).is_some_and(|k| was.contains(k))).collect();
                folded = from.filter(|f| f.abs() >= 0.5).map(|from| Fold {
                    start: range.start,
                    after,
                    heights: if from < 0. { heights.unwrap_or_default() } else { Vec::new() },
                    kept: if from < 0. { kept } else { Vec::new() },
                    from,
                    o: from,
                    ghost: ghost.filter(|_| from > 0.),
                });
            }
            targets = Some(at);
            let top = list.logical_scroll_top().item_ix;
            let scroll = -f32::from(list.scroll_px_offset_for_scrollbar().y);
            for (ix, o) in old.iter().enumerate().skip(top) {
                let Some((y, _)) = o.2 else { break };
                if y - scroll > f32::from(list.viewport_bounds().size.height) {
                    break;
                }
                if let Some(k) = flip_key(&self.rail, ix).filter(|_| !fold) {
                    // Riding a fold, or in its body: where it was drawn
                    // (R2-L2, R3-L2).
                    let o = self.riding.as_ref().map_or(0., |r| r.dy(ix));
                    flip_from.push((k, y + o));
                }
            }
            let start = range.start;
            list.splice(range, count);
            // The rail measures every row, so its bar reaches its end; GPUI
            // measures new ones up front only once told they need it.
            list.remeasure_items(start..start + count);
        }
        *self = RailCache {
            generation: Some(m.rail_gen),
            minute: now / 60,
            view: view.clone(),
            states,
            rail,
            keys,
            flip_from,
            targets,
            fold: false,
            folded,
            kind_h: std::mem::take(&mut self.kind_h),
            riding: self.riding.take(),
        };
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
            // A reload within its grace is not a disconnect (re-gate 3).
            _ if self.synthetic || self.model.reload => p.ready_mark_ch,
            ConnState::Connecting | ConnState::Reconnecting => p.rest_mark,
            _ => p.err_mark_ch,
        }
    }

    /// The link's signatures over the whole window, never in its way
    /// (motion.md §5.1, §5.3): the tear's three lines (2, 1 and 3 px in
    /// `err_mark_ch`, at heights hashed from the drop's time and the
    /// frame), or the Scan's 1 px `run_mark_ch` line with its 48 px trail
    /// of four bands of `run` (alpha .10, .07, .04, .02) above it.
    /// The Scan is drawn in `live` (not drained): the link is back, in
    /// its live colour from the first frame, while the palette restores.
    pub(crate) fn render_link(&mut self, tear: Option<usize>, p: &Palette, live: &Palette) -> Option<impl IntoElement + use<>> {
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
            out = out.child(div().absolute().left_0().right_0().top(px(top)).h(px(12.)).bg(Hsla { a, ..live.run }));
        }
        #[cfg(test)]
        {
            self.scan_line = Some(live.run_mark_ch);
        }
        out = out.child(div().absolute().left_0().right_0().top(px(y)).h(px(1.)).bg(live.run_mark_ch));
        Some(deferred(out).with_priority(5))
    }

    /// The tracer's line on its way (motion.md §6.2): its head `base` `out`
    /// along the straight line from where the tab was opened to its top
    /// edge, 24 px of it behind, 2 px in `you_bar`, over everything.
    pub(crate) fn render_tracer(&self, p: &Palette) -> Option<impl IntoElement + use<>> {
        let tr = self.panes.cur.tracer?;
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
            // The prompt of the store that holds that session (Shady #36 L6:
            // never this pane's for another pane's session).
            let store = self.model.lives.ids().into_iter().find(|l| self.model.lives[*l].is_active(&s.id));
            if let Some(l) = store
                && let Some(pr) = self.model.lives[l].transcript.pending_prompts().first()
            {
                // Built once per prompt, with the card (`sync_card` ran first).
                let card = self.current_card().filter(|_| l == self.panes.cur.live).map(|c| c.gist.clone());
                card.unwrap_or_else(|| prompt_gist(&pr.title, &pr.prompt).into())
            } else {
                s.title.clone().filter(|t| !t.is_empty() && s.name.is_some()).unwrap_or_default().into()
            }
        });
        // On its way, the word's trailing dots step `·` → `··` → `···` at
        // 2 Hz on the beat (motion.md §6.7: text, not a spinner); held at
        // `···` without the heartbeat. Mono, padded: the word never moves.
        let dots = |word: &str| -> SharedString {
            let n = self.fx.words_beat(4).map_or(3, |b| (b / 4 % 3) as usize + 1);
            format!("{word} {:<3}", "·".repeat(n)).into()
        };
        let (conn, conn_color): (SharedString, Hsla) = match &self.model.conn {
            // A daemon reload within its grace (re-gate 3): a planned
            // handover, not a disconnect. The words say so, a session
            // attached or not.
            _ if self.model.reload => (dots("reloading"), p.dim_ch),
            _ if self.model.lives[self.panes.cur.live].reattaching => (dots("reattaching"), p.dim_ch),
            ConnState::Connected(v) => {
                let words: SharedString = format!("connected · {v}").into();
                self.conn_held.set(words.chars().count());
                (words, p.dim_ch)
            }
            ConnState::Connecting => (dots("connecting"), p.dim_ch),
            ConnState::Reconnecting => (dots("reconnecting"), p.dim_ch),
            ConnState::Off(e) | ConnState::Incompatible(e) => {
                (crate::app::one_line(e, 60).into(), if self.synthetic { p.dim_ch } else { p.err_ch })
            }
        };
        // Down after it was up (a loss, a reload), or up again with the
        // session still re-attaching: the words keep the connected words'
        // width (mono), padded or cut with an ellipsis, so NEEDS YOU,
        // centred between, never moves (re-gate r3, Shady #15 M1). Cut, the
        // whole of them is the cell's tooltip.
        let held = self.conn_held.get();
        let hold = held > 0 && (self.model.lives[self.panes.cur.live].reattaching || !matches!(self.model.conn, ConnState::Connected(_)));
        let whole: Option<SharedString> = (hold && conn.chars().count() > held).then(|| match &self.model.conn {
            ConnState::Off(e) | ConnState::Incompatible(e) => e.clone().into(),
            _ => conn.clone(),
        });
        let conn: SharedString = if hold { hold_width(&conn, held).into() } else { conn };
        // As in the mock: subagents running, here the attached session's,
        // as its blocks and its rail row say (they change together).
        let subagents = self.model.active_workers(self.panes.cur.live);
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
            .debug_selector(|| "attention".into())
            .flex_none()
            .gap(px(12.))
            .items_center()
            // The state grammar (design.md §4.1): NEEDS YOU's glyph, or at rest the idle one.
            .child(if on {
                glyph::needs(glyph::needs_ping(&self.fx, true), p.warn_mark_ch, p.chrome)
            } else {
                let _ = glyph::needs_ping(&self.fx, false);
                glyph::draw(glyph::Glyph::Idle, p.rest_mark)
            })
            // The count rolls (polish.md P5) and afterglows (motion.md
            // §6.6) when it changes; the words stay.
            // One text at rest, as before; rolling, only its changed cells.
            .child({
                let c = self.fx.glow(GLOW | 0xFFFF, n as u64, if on { p.warn_ch } else { p.dim_ch });
                let text = format!("{n}   {}", label::NEED_YOU).into();
                crate::motion::roll_count(self.fx.chrome(), crate::motion::ROLL_NEEDS, n, text, |t| display(t, size::D_S, c).flex_none())
            })
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
        #[cfg(test)]
        let ticker = ticker.when_some(self.roll_probe, |d, n| {
            let text = format!("{n} tool{}", if n == 1 { "" } else { "s" }).into();
            d.child(crate::motion::roll_count(self.fx.chrome(), crate::motion::ROLL_PROBE, n, text, |t| {
                mono(t, size::META, p.dim_ch).flex_none()
            }))
        });
        self.probe(h_flex(), "top-bar")
            .debug_selector(|| "top-bar".into())
            .h(px(top_h(self.top_short)))
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
                    .when(fit.conn > 0., |d| {
                        // Down after it was up the cell keeps the connected
                        // words' width: NEEDS YOU, centred between, never moves.
                        let cell = cut(mono(conn, size::META, conn_color), fit.conn).relative().top(px(self.conn_dy));
                        let cell = if hold { cell.w(px(fit.conn)) } else { cell };
                        d.child(
                            cell.id("conn-words")
                                .when_some(whole, |c, t| c.tooltip(move |window, cx| crate::overlay::tip(t.clone(), window, cx))),
                        )
                    })
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
        let ix_of = |key: u64| (0..rail.items.len()).find(|&ix| flip_key(rail, ix) == Some(key));
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
        out.leads.clear();
        // A section folding: what follows it rides `o` to 0 on `sheet`.
        // Any other splice (a session starting, a reorder, find) ends a
        // fold under way: its items are not where they were.
        if let Some(f) = self.rail.folded.take() {
            fx.spring_jump(FOLD_Y, f.from, 0., crate::motion::SHEET_RESPONSE);
            out.fold = Some(f);
        } else if targets.is_some() {
            out.fold = None;
        }
        if let Some(f) = out.fold.as_mut() {
            let (o, moving) = fx.spring_held(FOLD_Y, 0., crate::motion::SHEET_RESPONSE);
            f.o = o;
            if !moving {
                out.fold = None;
            }
        }
        if out.fold.is_none() {
            out.tail.clear();
            out.ghost_at = None;
        }
        let riding = out.fold.as_ref().map(|f| Riding {
            start: f.start,
            after: f.after,
            o: f.o,
            body: (f.start + 1..f.after).map(|ix| f.dy(ix)).collect(),
        });
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
        // Items moving farther than the rest pass over them.
        let far = out.offsets.values().fold(0f32, |m, d| m.max(d.abs()));
        let leads: Vec<u64> = out.offsets.iter().filter(|(_, d)| d.abs() >= 0.75 * far).map(|(&k, _)| k).collect();
        out.view = list.viewport_bounds().size;
        if far >= 1. && leads.len() < out.offsets.len() {
            for key in leads {
                if let Some((y, h, scroll)) = ix_of(key).and_then(|ix| item_y(list, ix)) {
                    out.leads.insert(key, (y - scroll, h));
                }
            }
        }
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
        self.rail.riding = riding;
    }

    /// The drag handle on a side panel's inner edge (`rail`, else the right
    /// panel's), over the rest of the window: its 1-px line is the panel's
    /// border, `line3` under the pointer, `ink3` while dragged. A strip has
    /// one too (dragged out, it opens), unless the window cannot open it.
    /// `shown`: the panel's width as drawn (on its spring, fluid.md §3.8).
    /// None while the panel flies: a drag starts from a panel at rest (one
    /// caught in flight would start from where it is going, not where it
    /// is drawn).
    pub(crate) fn render_edge(
        &self,
        rail: bool,
        side: Side,
        shown: f32,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if side == Side::Strip && crate::app::toggled(self.width, self.panels, rail).is_none() {
            return None;
        }
        if self.panel_fx.sides[usize::from(!rail)].moving {
            return None;
        }
        let dragged = self.drag.is_some_and(|g| g.moved && g.rail == rail);
        // Mostly over the panel: the border is the rail's last pixel and
        // the right panel's first.
        let (left, line) = if rail { (MARK + shown - 4., 3.) } else { (self.width - shown - 2., 2.) };
        let (id, group) = if rail { ("rail-edge", "rail-edge") } else { ("right-edge", "right-edge") };
        Some(
            div()
                .id(id)
                .debug_selector(move || id.into())
                .group(group)
                .absolute()
                .top(px(top_h(self.top_short)))
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

    /// The side panels this frame (fluid.md §3.8): their widths as drawn,
    /// on their springs. A move the app asked for ([`PanelFx::fly`]) starts
    /// from where the panel is drawn; anything else (a window resize, the
    /// auto width giving way) is taken at once, and a drag follows the
    /// pointer. Reduced and Off: at once.
    ///
    /// On whole device pixels ([`crate::motion::Motion::spring_px`]): the
    /// centre pane, and so the transcript list's item width (the column,
    /// its padding whole pixels too), is the same in every frame of a
    /// flight. GPUI forgets every measured row height when the item width
    /// changes at all (`list.rs` prepaint), and its scrollbar with them.
    pub(crate) fn panel_frame(&mut self, rail: Side, right: Side) -> (f32, f32) {
        let dragged = self.drag.filter(|g| g.moved).map(|g| usize::from(!g.rail));
        for (i, side) in [rail, right].into_iter().enumerate() {
            let f = &mut self.panel_fx.sides[i];
            let to = side.width();
            if f.response == 0. {
                f.response = crate::motion::SHEET_RESPONSE;
            }
            if let Some(from) = f.from.take() {
                self.fx.spring_jump(PANEL_W[i], from, to, f.response);
            }
            // The dragged edge follows the pointer; the other panel, if it
            // flies, flies on.
            if dragged == Some(i) {
                self.fx.spring_jump(PANEL_W[i], to, to, f.response);
            }
            let asked = self.fx.asks();
            let (w, moving) = self.fx.spring_px(PANEL_W[i], to, f.response);
            // Drawn at its target: landed, its spring at rest (no frame
            // and no timer for the settling it would draw nothing of).
            let landed = moving && w == self.fx.snap(to);
            if landed {
                self.fx.spring_jump(PANEL_W[i], to, to, f.response);
                self.fx.ask_back(asked);
            }
            (f.shown, f.moving) = (w, moving && !landed);
            if let Side::Open(ow) = side {
                f.open = ow;
            }
        }
        (self.panel_fx.sides[0].shown, self.panel_fx.sides[1].shown)
    }

    /// A side panel (`rail`, else the right one) as drawn this frame. In
    /// flight it is its open content at full width in a box as wide as the
    /// spring says, clipped, never squeezed: a fold or an unfold slides it
    /// under the window's side, a width change clips it at the inner edge.
    /// Its strip comes and goes with it ([`strip_strength`]): whole on the
    /// last frame that moves, unchanged once it has landed (#35).
    pub(crate) fn render_side(&self, rail: bool, side: Side, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let f = self.panel_fx.sides[usize::from(!rail)];
        let content = |w: f32| -> AnyElement {
            if rail {
                self.render_rail(w, p, cx).into_any_element()
            } else {
                h_flex()
                    .h_full()
                    .child(self.render_right(w, p, cx))
                    .child(self.bars.right.bar(crate::scrollbar::Ground::Chrome))
                    .into_any_element()
            }
        };
        let strip = || -> AnyElement {
            if rail { self.render_rail_strip(p, cx).into_any_element() } else { self.render_right_strip(p, cx).into_any_element() }
        };
        let id = if rail { "rail-flight" } else { "right-flight" };
        if !f.moving {
            // Under the flight's id, as in flight: a press in the fold's
            // tail (#39 n2), or on a row in the unfold's (Shady #41 L1),
            // let go after the landing is still a click.
            return match side {
                Side::Open(w) => div().id(id).h_full().flex_none().flex().child(content(w)).into_any_element(),
                Side::Strip => div().id(id).h_full().flex_none().child(strip()).into_any_element(),
            };
        }
        let cw = match side {
            Side::Open(w) if !f.slide => w.max(f.start),
            Side::Open(w) => w,
            Side::Strip => f.open,
        };
        // Its content's left in the box: a rail sliding keeps its inner
        // edge on the box's; a right panel's slides with its own left edge.
        let x = match (rail, f.slide) {
            (true, true) | (false, false) => f.shown - cw,
            _ => 0.,
        };
        div()
            .id(id)
            .debug_selector(move || format!("{}-flight", if rail { "rail" } else { "right" }))
            .relative()
            .w(px(f.shown))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(p.chrome)
            .child(div().absolute().top_0().bottom_0().left(px(x)).w(px(cw)).flex().child(content(cw)))
            // The rail's border on its moving edge, over what it clips.
            .when(rail, |d| d.child(div().absolute().top_0().bottom_0().right_0().w(px(1.)).bg(p.hair)))
            // Its strip on the panel's inner edge, the one edge there is
            // (#39 L1), on the ground it lands on. A click on it is its own,
            // never what it covers (n2).
            .map(|d| match strip_strength(f.shown) {
                k if k > 0. => d.child(crate::motion::lift_from(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .when(rail, |d| d.right_0())
                        .when(!rail, |d| d.left_0())
                        .w(px(STRIP))
                        .bg(p.chrome)
                        .occlude()
                        .child(strip()),
                    k,
                    0.,
                )),
                _ => d,
            })
            .into_any_element()
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
        let detached = self.model.lives[self.panes.cur.live].stats.detached && self.model.lives[self.panes.cur.live].active.is_none();
        let dim = detached || self.opening();
        let faded = p.faded();
        let q = if dim { &faded } else { p };
        let ratio = self.model.lives[self.panes.cur.live].stats.context_ratio();
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
                    // Its count rolls (polish.md P5).
                    .child({
                        let color = match c {
                            _ if count == 0 => p.dim_ch,
                            Chip::NeedsYou => p.warn_ch,
                            Chip::Running => p.run_ch,
                            Chip::Unread => p.unread_ch,
                        };
                        let key = crate::motion::key(crate::motion::ROLL_RAIL, c as u64);
                        crate::motion::roll_count(self.fx.chrome(), key, count, chip_count(count).into(), move |t| {
                            mono(t, size::META, color)
                        })
                    })
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
        // A section folding holds the rail's length until it lands (shorter
        // at once, a rail scrolled to its end would jump down): what goes
        // is drawn ([`AppView::rail_layer`]) and its room kept.
        let fold_o = self.rail_fx.fold.as_ref().map_or(0., |f| f.o);
        let items = list(self.rail_list.clone(), cx.processor(|this, ix, _window, cx| this.render_rail_item(ix, cx)))
            .size_full()
            .pb(px(fold_o.max(0.)));
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
                    .child(section(label::SESSIONS, p))
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
                    // Right after the list's layout, in its frame: what a
                    // section folding or unfolding has that the list doesn't
                    // draw.
                    .when(self.rail_fx.fold.is_some(), |d| {
                        d.child(crate::sticky::layer(
                            self.fx.clone(),
                            cx.processor(|this, b: Bounds<Pixels>, _, cx| this.rail_layer(b, cx)),
                        ))
                    })
                    // Its length as drawn: the fold's offset in it.
                    .child(crate::scrollbar::rows(&self.rail_list, fold_o, &self.bars.rail).ground(crate::scrollbar::Ground::Chrome)),
            )
            .child(self.render_foot(p, cx))
    }

    /// Over the rail's list, laid out after it in the same frame (a layer,
    /// as the transcript's pinned labels are: [`crate::sticky`]): what a
    /// section folding or unfolding shows that the list doesn't draw, from
    /// where the list has just laid its items out.
    /// - Unfolding: the items after it whose place is below the view (the
    ///   list draws only those whose place is in it) ride into it from where
    ///   they were, `o` above their place.
    /// - Folding: the body it had, under its head, down to what follows as
    ///   it rises over it (clipped there): the unfold's reveal, backwards.
    ///
    /// Decorations: they take no pointer (the wheel still scrolls the rail).
    pub(crate) fn rail_layer(&mut self, _: Bounds<Pixels>, cx: &mut Context<Self>) -> Option<crate::sticky::Pinned> {
        let f = self.rail_fx.fold.clone()?;
        let vp = self.rail_list.viewport_bounds();
        let view_h = f32::from(vp.size.height);
        let list = self.rail_list.clone();
        let place = |ix: usize| list.bounds_for_item(ix).map(|b| (f32::from(b.top() - vp.top()), f32::from(b.size.height)));
        let mut tail = Vec::new();
        let mut ghost_at = None;
        // Each piece, and the band it covers in the view.
        let mut drawn: Vec<(AnyElement, f32, f32)> = Vec::new();
        if f.o < 0. {
            // Its body too: what it kept is drawn above its place.
            for ix in f.start + 1..self.rail.rail.items.len() {
                let Some((top, _)) = place(ix) else { break };
                let dy = f.dy(ix);
                if top + dy >= view_h {
                    break;
                }
                // (One of its body that shows nothing yet draws nothing.)
                if top >= view_h && f.shown(ix).is_none_or(|s| s.0 > 0.) {
                    tail.push((ix, top + dy));
                }
            }
            if let Some(&(_, y)) = tail.first() {
                let rows = v_flex().w_full().children(tail.iter().map(|&(ix, _)| {
                    let el = self.render_rail_item_at(ix, cx);
                    match f.shown(ix) {
                        Some((s, _)) => div().w_full().flex_none().h(px(s)).child(crate::motion::clip_h(el, s)).into_any_element(),
                        None => el,
                    }
                }));
                drawn.push((rows.into_any_element(), y, view_h));
            }
        } else if let Some(g) = f.ghost.as_ref() {
            // Its head's foot, down to what follows (laid out under what
            // stays, drawn `o` lower).
            let at = place(f.start).map(|(t, h)| t + h).or_else(|| place(f.after).map(|(t, _)| t - g.kept()));
            let h = g.kept() + f.o;
            if let Some(y) = at.filter(|&y| y < view_h && y + h > 0.) {
                // What goes takes only what it shows (laid out at it, drawn
                // down to it): what stays rises with what follows.
                let shown = g.shown(f.o);
                let rows = v_flex().w_full().children(g.body.clone().zip(shown).zip(&g.items).map(|((ix, s), &(kept, _))| {
                    let el = self.render_rail_item_in(&g.rail, ix, cx);
                    if kept { el } else { div().w_full().flex_none().h(px(s)).child(crate::motion::clip_h(el, s)).into_any_element() }
                }));
                drawn.push((crate::motion::clip_h(rows.into_any_element(), h), y, y + h));
                ghost_at = Some((y, h, g.body.len()));
            }
        }
        self.rail_fx.tail = tail;
        self.rail_fx.ghost_at = ghost_at;
        if drawn.is_empty() {
            return None;
        }
        let el = div().size_full().relative().children(drawn.into_iter().flat_map(|(el, y, end)| {
            [
                div().absolute().left_0().right_0().top(px(y)).child(el).into_any_element(),
                div().absolute().left_0().right_0().top(px(y)).h(px(end - y)).block_mouse_except_scroll().into_any_element(),
            ]
        }));
        Some(crate::sticky::Pinned { el: el.into_any_element(), cover: None })
    }

    /// One item of the rail's list (only the visible ones are built).
    /// A rail item, a head sliding with the rows (Flip) when it moved.
    pub(crate) fn render_rail_item(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let el = self.render_rail_item_at(ix, cx);
        let rail = &self.rail.rail;
        let head = rail.items.get(ix).is_some_and(|it| row_key(rail, it).is_none());
        let key = flip_key(rail, ix);
        let el = match key.and_then(|k| self.rail_fx.offsets.get(&k)).filter(|_| head) {
            // On its own ground, as a sliding row is: items that cross don't mix.
            Some(&dy) => div().w_full().relative().top(px(dy)).bg(theme::palette(cx).chrome).child(el).into_any_element(),
            None => el,
        };
        // A section folding: what follows it rides the fold; its body
        // shows down to it (clipped, never squeezed).
        let el = match self.rail_fx.fold.as_ref() {
            Some(f) if ix >= f.after => div().w_full().relative().top(px(f.o)).child(el).into_any_element(),
            // What a fold keeps is drawn in the body going (its ghost)
            // until it lands: here, its place held (its height as it was).
            Some(Fold { ghost: Some(g), start, .. }) if ix > *start => {
                let h = g.items.iter().filter(|i| i.0).nth(ix - start - 1).map_or(0., |i| i.1);
                div().w_full().h(px(h)).into_any_element()
            }
            Some(f) => match f.shown(ix) {
                Some((h, 0.)) => crate::motion::clip_h(el, h),
                Some((h, dy)) => div().w_full().relative().top(px(dy)).child(crate::motion::clip_h(el, h)).into_any_element(),
                None => el,
            },
            None => el,
        };
        // A lead: its place kept in the list, the item drawn over the rest,
        // clipped to the rail's view.
        match key.and_then(|k| self.rail_fx.leads.get(&k)) {
            Some(&(top, h)) => {
                let vp = self.rail_fx.view;
                let over = div()
                    .relative()
                    .top(px(-top))
                    .w(vp.width)
                    .h(vp.height)
                    .overflow_hidden()
                    .child(div().absolute().left_0().top(px(top)).w(vp.width).child(el));
                div().w_full().h(px(h)).child(deferred(over).with_priority(0)).into_any_element()
            }
            None => el,
        }
    }

    fn render_rail_item_at(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        self.render_rail_item_in(&self.rail.rail, ix, cx)
    }

    /// Item `ix` of `rail` (the rail now, or the one before a section
    /// folded: its body, drawn as it goes).
    fn render_rail_item_in(&self, rail: &Rail, ix: usize, cx: &Context<Self>) -> AnyElement {
        let p = theme::palette(cx);
        let Some(item) = rail.items.get(ix) else { return div().into_any_element() };
        match item {
            // A group's head (the first: no rule above it) that folds, as a
            // bucket does: its chevron in the left inset, so the label
            // lines up with the other groups'.
            Item::Favs { open, count } => {
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
                    .map(|d| {
                        self.lit(d, crate::motion::key(RAIL_TOUCH, crate::app::tkey("favs-head", 0)), (p.chrome, p.raised, p.press_ch), cx)
                    })
                    // It turns as the section folds (fluid.md §3.7).
                    .child(
                        div().absolute().left(px(7.)).top(px(19.)).child(glyph::turn(self.fx.chrome().turn(FAVS_TURN, *open), p.rest_mark)),
                    )
                    .child(display(label::FAVOURITES, size::D_S, p.dim_ch))
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
                .child(div().flex_1())
                .child(mono(count, size::META, p.dim_ch))
                .into_any_element(),
            Item::Row(r) => self.render_row2(&rail.rows[*r], &p, cx),
            Item::Hist(r) => self.render_hist(&rail.rows[*r], &p, cx),
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
                    .map(|d| {
                        self.lit(
                            d,
                            crate::motion::key(RAIL_TOUCH, crate::app::tkey("bucket", b as usize)),
                            (p.chrome, p.raised, p.press_ch),
                            cx,
                        )
                    })
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
                    .map(|d| {
                        self.lit(
                            d,
                            crate::motion::key(RAIL_TOUCH, crate::app::tkey("more", b as usize)),
                            (p.chrome, p.raised, p.press_ch),
                            cx,
                        )
                    })
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
    fn render_row2(&self, row: &rail::Row, p: &Palette, cx: &Context<Self>) -> AnyElement {
        // No readout under an open menu (a right-click's pointer rests on
        // the row it opened from).
        let menu_open = self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some();
        let meta = row.live.and_then(|i| self.model.sessions.get(i));
        let st = meta.map(|s| self.model.state(s)).unwrap_or(row.state);
        let on = meta.is_some_and(|s| self.model.is_active(&s.id));
        // The marker follows the visible tab (also while its attach is on
        // its way); a session open in a background tab has a faint one.
        let shown = meta.is_some_and(|s| self.model.is_shown(&s.id));
        let in_tab = !shown && meta.is_some_and(|s| self.model.tabs.holds(&s.id));
        // The attached session's cost as the panel shows it (its digests
        // run ahead of the 3 s poll), else the poll's.
        let cost = meta
            .map(|s| if on { self.model.lives[self.panes.cur.live].stats.cost.unwrap_or(s.cost_usd) } else { s.cost_usd })
            .filter(|c| *c > 0.0);
        let waiting = meta
            .map(|s| if on { self.model.lives[self.panes.cur.live].transcript.pending_prompts().len() } else { s.awaiting_input })
            .unwrap_or(0);
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
        let since = self.fx.chrome().change(crate::motion::Tag::RowNeeds, key, needs as u64, None).filter(|_| needs).map(|c| c.0);
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
        // Sliding, a row is on its own ground: rows that cross don't mix.
        let fill = fill.or((flip != 0.).then_some(p.chrome));
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
            // A running, workers or opening session spins (polish.md P1).
            glyph::session_live(st, Ground::Chrome, fill.unwrap_or(p.chrome), p, &self.fx)
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
            .debug_selector(move || format!("rail-row {key}"))
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
                                // Where it is on screen (split.md §6, A13):
                                // the pane map, its panes lit.
                                .children(meta.filter(|_| self.panes.split()).and_then(|s| {
                                    let on = self.panes_showing(&s.id);
                                    (!on.is_empty()).then(|| {
                                        self.pane_map(13., 9., move |id| on.contains(&id), false, p, cx)
                                            .debug_selector(|| "rail-pane-map".into())
                                    })
                                }))
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
            // Dragged onto a pane (split panes step 4): it opens there. A
            // click still opens here (GPUI drops the click once a drag
            // starts, past 2 px).
            .when(self.split_on(), |d| d.on_drag(self.rail_drag(row, p), crate::dnd::RailDrag::start(cx.weak_entity())))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                // Alt+click: Open to the side (split.md §2).
                match this.rail.rail.rows.iter().find(|r| r.key == key).map(|r| r.action.clone()) {
                    Some(action) if ev.modifiers().alt && this.split_on() => this.open_to_side(&action, window, cx),
                    _ => this.rail_click(key, crate::tabstrip::click_place(ev, cx)),
                }
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
    fn render_hist(&self, row: &rail::Row, p: &Palette, cx: &Context<Self>) -> AnyElement {
        // No readout under an open menu (a right-click's pointer rests on
        // the row it opened from).
        let menu_open = self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some();
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
        let fill = fill.or((flip != 0.).then_some(p.chrome));
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
            .child(div().flex().flex_none().relative().top(px(dy)).child(glyph::session_live(
                row.state,
                Ground::Chrome,
                fill.unwrap_or(p.chrome),
                p,
                &self.fx,
            )))
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
            // Dragged onto a pane (split panes step 4): it opens there. A
            // click still opens here (GPUI drops the click once a drag
            // starts, past 2 px).
            .when(self.split_on(), |d| d.on_drag(self.rail_drag(row, p), crate::dnd::RailDrag::start(cx.weak_entity())))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                // Alt+click: Open to the side (split.md §2).
                match this.rail.rail.rows.iter().find(|r| r.key == key).map(|r| r.action.clone()) {
                    Some(action) if ev.modifiers().alt && this.split_on() => this.open_to_side(&action, window, cx),
                    _ => this.rail_click(key, crate::tabstrip::click_place(ev, cx)),
                }
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
    /// Row `key`, favourited `at`: when it landed (its Flip offset gone, a
    /// frame after the favouriting at the earliest), which is when its star
    /// hops; `None` while it moves (a frame asked for). Moving a second
    /// on, it hops anyway; first seen a second after `at` (drawn again
    /// after a switch), it hopped long ago.
    fn star_landed(&self, key: u64, at: std::time::Instant) -> Option<std::time::Instant> {
        let now = self.fx.now();
        let moving = self.rail_fx.offsets.get(&key).is_some_and(|d| *d != 0.);
        let late = now.saturating_duration_since(at) >= std::time::Duration::from_secs(1);
        let mut land = self.star_land.borrow_mut();
        land.retain(|_, (t, _)| now.saturating_duration_since(*t) < std::time::Duration::from_secs(10));
        let e = land.entry(key).or_insert((at, late.then_some(at)));
        if e.0 != at {
            *e = (at, late.then_some(at));
        }
        if e.1.is_none() {
            if (moving || now <= at) && !late {
                self.fx.want_frame();
                return None;
            }
            e.1 = Some(now);
        }
        e.1
    }

    fn star(&self, key: u64, fav: bool, h: f32, p: &Palette, cx: &Context<Self>) -> Stateful<Div> {
        // Favourited while drawn (a click, Ctrl+D): the star hops on an
        // impulse, critically damped, and four sparks leave it (polish.md
        // P6). Unfavourited: the fill clears at once. Reduced, Off: the fill.
        // It hops once its row has landed (finish gate #17): favouriting
        // moves the row into FAVOURITES, and the acknowledgement lands on a
        // still row. On device pixels, hop and sparks (Shady #17).
        let fx = &self.fx;
        let since = self.fx.chrome().change(crate::motion::Tag::Star, key, u64::from(fav), None).filter(|c| fav && c.1 == 0).map(|c| c.0);
        let since = match since {
            Some(at) => self.star_landed(key, at),
            None => {
                self.star_land.borrow_mut().remove(&key);
                None
            }
        };
        let mark = if fav {
            let sparks = fx.sparks(since).map(|x| {
                let d = 8. * x * std::f32::consts::FRAC_1_SQRT_2;
                let c = if x < 0.55 { p.you_bar } else { crate::motion::mix(p.you_bar, p.chrome, (x - 0.55) / 0.45) };
                [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)]
                    .map(|(sx, sy)| div().absolute().left(px(fx.snap(4. + sx * d))).top(px(fx.snap(4. + sy * d))).size(px(2.)).bg(c))
            });
            div()
                .flex()
                .flex_none()
                .size(px(10.))
                .relative()
                .top(px(fx.snap(fx.hop(since))))
                .child(glyph::draw(glyph::Glyph::Starred, p.focus_ch))
                .children(sparks.into_iter().flatten())
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
                    anchored()
                        .offset(point(px(self.panel_fx.sides[0].shown + 12. + dx), px(2. + dy)))
                        .child(crate::motion::appear(body, strength)),
                )
                .with_priority(2),
            )
            .into_any_element()
    }

    pub(crate) fn render_right(&self, width: f32, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let s = &self.model.lives[self.panes.cur.live].stats;
        // Detached: the last numbers seen, in the dim roles and labelled,
        // not live. A cached tab whose attach is on its way: the same, as
        // last seen.
        let detached = s.detached && self.model.lives[self.panes.cur.live].active.is_none();
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
            .child(h_flex().items_center().child(section(label::CONTEXT, p)).child(div().flex_1()).child(fold(
                "right-fold",
                "›",
                "Hide context, tokens and files",
                false,
                p,
                cx,
            )))
            // Whose numbers (split.md §6, A12): the session behind them (the
            // focused pane's store, on a worker or Settings tab too), its
            // title, and the cells of the panes that show it (Shady #44 L6).
            .when(self.panes.split(), |d| {
                let (title, lit) = self.numbers_session();
                d.child(
                    h_flex()
                        .mt(px(6.))
                        .gap(px(8.))
                        .items_center()
                        .debug_selector(|| "right-pane-map".into())
                        .child(self.pane_map(13., 9., move |id| lit.contains(&id), false, p, cx))
                        .child(
                            mono(title, 12., p.prose)
                                .debug_selector(|| "right-pane-title".into())
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis(),
                        ),
                )
            })
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
            .child(section(label::TOKENS, p))
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
            .child(h_flex().items_end().child(section(label::FILES, p)).child(div().flex_1()).when(added + removed > 0, |d| {
                d.child(h_flex().gap(px(8.)).child(mono(format!("{approx}+{added}"), size::META, p.ok_ch)).child(mono(
                    format!("−{removed}"),
                    size::META,
                    p.err_ch,
                )))
            }))
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
                match self.model.lives[self.panes.cur.live].watch.focused() {
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
            .map(|d| {
                self.lit_rest(d, crate::app::tkey("theme-config", 0), (p.chrome, p.raised, p.press_ch), follows.then_some(p.raised), cx)
            })
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
        let (top, bottom) = (top_h(self.top_short), 140.);
        // Over the open right panel, at least two columns of names wide,
        // when that stays clear of the tab strip's `+` (it ends at the
        // column's right edge); else above the composer, under the tab
        // strip.
        // The panel as drawn (it may be flying).
        let w = self.panel_fx.sides[1].shown;
        let place = match right {
            Side::Open(_) if w.max(PICKER_MIN) - w + 8. <= self.panes.cur.layout.right => PickerAt::Top(w.max(PICKER_MIN)),
            _ => PickerAt::Bottom(w + 24.),
        };
        let max_h = match place {
            PickerAt::Top(_) => self.height - top - 16.,
            PickerAt::Bottom(_) => self.height - bottom - top - if self.panes.cur.layout.short { TABS_CLEAR_SHORT } else { TABS_CLEAR },
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

/// `words` at exactly `w` chars (mono): padded, or cut with an ellipsis.
fn hold_width(words: &str, w: usize) -> String {
    if words.chars().count() <= w {
        format!("{words:<w$}")
    } else {
        let cut: String = words.chars().take(w.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end()).chars().chain(std::iter::repeat(' ')).take(w).collect()
    }
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
        m.want(crate::live::LiveId::FIRST, 1);
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
