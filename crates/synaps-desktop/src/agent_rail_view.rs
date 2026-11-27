//! The agent's rail sections, drawn (synaps-cards `docs/blocks.md` v1.3
//! §4, §6.1, and §6.2's lease and painted targets in the rail; D6a). What
//! they are is [`crate::agent_rail`]'s; this draws it in the right panel,
//! for the focused pane's session (split panes: the panel follows the
//! focused pane, `chrome.rs`).
//!
//! - **The zone.** The upper slot opens with the panel's own section
//!   header, `AGENT`, the accent dot and `N sections · M dismissed`, then
//!   NOW, one fixed 22 px mono line; the end slot with a thin `AGENT`
//!   divider. The zone shows once the board places a section in the rail
//!   (or D6b reports one dismissed): with no agent sections the panel is
//!   the desk's own, as before blocks.
//! - **A section** is the agent's frame: `card_d` with the 2 px accent
//!   spine, square, and a 28 px header with its life glyph, its title, the
//!   `AGENT` chip and `rN`, and the chevron. A block titled "PROMPT CACHE"
//!   still reads as the agent's: the desk's own sections have none of it,
//!   and nothing names them (§4). Its body is its kind's, in 20 px lines.
//! - **The lease in the rail.** Moving over the zone starts it, as over
//!   the transcript ([`crate::lease`], one per window: there is one
//!   panel). While it holds, a board that would change the rail's
//!   structure ([`crate::agent_rail::shape`]: a cap change, a section
//!   added, removed, reordered, replaced or changing height) waits, the
//!   whole board with it (the transcript's rule: one coherent board).
//!   A note's height is its text as laid out by the window's text system
//!   at the rail's face, size and width ([`AppView::rail_note_lines`]),
//!   never an estimate: its cut and `more`, and the structure, follow it.
//!   An agent op's glides already in flight hold where they are while the
//!   lease holds, and go on from there once it ends ([`AppView::rail_reveal`]).
//!   A user's own click applies at once, the fold it causes included,
//!   and the header clicked keeps its screen y: a section folding above
//!   it scrolls the panel instead, as far as the panel can ([`KeepY`]).
//! - **Painted targets**: a header's chevron (its block and incarnation),
//!   a note's `more`, a slot's `+N` row (its leftovers). Down takes the
//!   target and holds the lease; up acts only if the bound board still
//!   holds it, else `this block changed · look again`. A target an agent
//!   op moved in the last 600 ms does nothing, once (`this moved just
//!   now`): its glides' frames are the agent op's until they land, so the
//!   window runs from the motion's end. The rail's frames are counted like
//!   a pane's.
//! - **What it keeps** keeps to what it draws ([`AppView::rail_retire`]):
//!   its painted targets and its glides go with their rows (one leaving
//!   once its glide is over), bounded by the board's caps for the
//!   session's life.
//! - **NOW** draws a call's verb and target and a prompt's scope through
//!   the desk's sanitizer (`Now::text`).
//! - **Motion.** A section's body opens and folds on the glide, the panel
//!   chrome's motion ([`crate::motion::Motion::chrome`]); a section that
//!   arrives glides open from nothing, and one that goes (past the cap, a
//!   list closed, removed) glides shut where it was, drawn from the frame
//!   before; Reduced and Off at once. NOW's
//!   glyph runs on the window's beat only while the turn streams. Nothing
//!   asks for a frame at rest.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use cards_spec::block::{Slot, StepState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::blocks::{Body, Drawn, Ref, Shown, Tag};

use crate::agent_rail::{self as ar, AllShown, Layout, Local, Now, RailVisibility, Section, State};
use crate::app::{AppView, display, mono, section};
use crate::fonts::{self, TextPx as _, label, size};
use crate::glyph::{self, Life};
use crate::lease::{CHANGED, JUST_MOVED, Lease, MOVED, Target};
use crate::live::LiveId;
use crate::motion;
use crate::theme::Palette;

/// A slot's `+N` row's id: its slot with this bit (a block row's is
/// [`crate::blocks::BLOCK_ID`]'s).
pub(crate) const MORE_ROW: u64 = 1 << 58;
/// A section's keys in the chrome's motion, each its own space
/// ([`motion::key`]): a row id keeps 59 bits of its hash, so no bit of it
/// can tell two keys of one row apart. Its arrival's and leaving's glide,
/// its body's open and fold, its chevron's turn.
const ARRIVE: u64 = 0x7B20 << 48;
const BODY: u64 = 0x7B21 << 48;
const TURN: u64 = 0x7B22 << 48;
/// Sessions whose rail state this desk keeps (the least recently drawn
/// goes past them).
const KEPT: usize = 32;
/// A header clicked to expand keeps its screen y this long: the fold it
/// causes glides.
const KEEP_FOR: Duration = Duration::from_millis(900);
/// The rail's sizes (the panel's are fixed px, not the text step's).
const HEAD: f32 = 28.;
const LINE: f32 = 20.;
const NOW_H: f32 = 22.;
const SPINE: f32 = 2.;
const PAD: f32 = 10.;
const GAP: f32 = 8.;
/// A section's margin above it, inside its arrival's glide: [`KeepY`]
/// keeps the section's own top, not its glide's.
const ABOVE: f32 = 8.;
/// A section's left and right padding in the panel (its sections'): 247
/// px of content at the panel's 296 px floor (§4), inside its border.
pub(crate) const INSET: f32 = 24.;
/// The panel's left border (`render_right`'s `border_l_1`): its content is
/// this much narrower than the panel.
const EDGE: f32 = 1.;
/// A header's title keeps this much before its revision is cut.
const TITLE_MIN: f32 = 48.;
/// A revision cut keeps at least this much (`r12…`).
const REV_MIN: f32 = 24.;
/// The `AGENT` chip's padding, each side.
const CHIP_PAD: f32 = 4.;
/// The chevron's box.
const CHEVRON: f32 = 10.;
/// The flexible space before the revision, at least.
const SPACER: f32 = 4.;

/// What a press in the rail acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RailAct {
    /// A section's fold: open folds it, otherwise it expands.
    Fold,
    /// A note's `more` / `less`.
    More,
    /// A slot's `+N` row: its leftovers' headers listed, or not.
    List,
}

/// A press's painted target in the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RailPress {
    pub at: Target,
    pub act: RailAct,
}

/// The rail's state on this window: each session's local state, the
/// rail's lease and painted targets (one panel, so one), and the header
/// that keeps its screen y.
#[derive(Default)]
pub(crate) struct RailState {
    local: HashMap<String, (u64, Local)>,
    used: u64,
    pub(crate) lease: Lease,
    pub(crate) down: Option<RailPress>,
    /// The session the pointer's press and note are of.
    pub(crate) of: Option<String>,
    /// The session the painted targets are of: another's are dropped as
    /// the rail draws it ([`AppView::rail_frame`]).
    painted_of: RefCell<Option<String>>,
    /// The header clicked to expand: its row, its screen y then, until when.
    pub(crate) keep: Rc<Cell<Option<(u64, f32, Instant)>>>,
    /// Its section's layout this frame ([`Mark`]).
    pub(crate) mark: Rc<Cell<Option<LayoutId>>>,
    /// The sections the rail drew last frame and this one: one not drawn
    /// last frame arrives ([`RailState::arrives`]).
    seen: RefCell<Seen>,
    /// The panel's content width inside its insets, as last drawn (a
    /// note's wrap, a header's budget, for the lease too).
    pub(crate) inner: Cell<f32>,
    /// The rows the rail draws this frame, either slot: its sections, those
    /// leaving and its `+N` rows. Its records keep to them
    /// ([`AppView::rail_retire`]).
    drew_rows: RefCell<HashSet<u64>>,
    /// Its glides ([`AppView::rail_reveal`]).
    glides: RefCell<Glides>,
    /// The rail frame that draws the last board published (an agent op's).
    landed: Cell<Option<u64>>,
    /// The window's text system, as the window last drew: a note's lines
    /// as laid out ([`AppView::rail_note_lines`]).
    pub(crate) text: Option<Rc<WindowTextSystem>>,
    /// Tests: a visibility in place of D6a's [`AllShown`] (D6b's seam).
    #[cfg(test)]
    pub(crate) vis: Option<Rc<dyn RailVisibility>>,
}

/// The sections drawn, by rail frame ([`RailState::drew`]).
#[derive(Default)]
struct Seen {
    of: Option<String>,
    frame: u64,
    last: HashMap<u64, Drew>,
    this: HashMap<u64, Drew>,
}

/// A section as drawn in a frame: its slot, its place among the slot's
/// drawn sections, listed under the slot's `+N` or not, and itself (one
/// leaving is drawn from it as it glides shut).
#[derive(Clone)]
struct Drew {
    k: usize,
    at: usize,
    listed: bool,
    sec: Section,
}

/// The rail's glides (its sections' arrivals, leavings and bodies: reveals
/// in the chrome's motion), by key: each one's target as last drawn, and
/// those an agent op started that still move ([`Flight`]).
#[derive(Default)]
struct Glides {
    to: HashMap<u64, bool>,
    agent: HashMap<u64, Flight>,
    /// An agent op's glides moved as the last rail frame ended.
    flight: bool,
    /// One was held this frame (the rail's lease).
    held: bool,
}

/// An agent op's glide in flight: the fraction it showed last (NaN: not
/// stepped yet), its height's cell, and since when it is held.
struct Flight {
    k: f32,
    sink: Rc<Cell<f32>>,
    held: Option<Instant>,
}

/// How a section's frame comes: there, arriving or leaving.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Come {
    Stay,
    Arrive,
    Leave,
}

static EMPTY: LazyLock<Local> = LazyLock::new(Local::default);
static ALL_SHOWN: AllShown = AllShown;

impl RailState {
    /// Session `s`'s local state (none kept: the empty one).
    pub(crate) fn local(&self, s: &str) -> &Local {
        self.local.get(s).map_or(&EMPTY, |(_, l)| l)
    }

    /// Session `s`'s local state to change, kept from now on.
    pub(crate) fn local_mut(&mut self, s: &str) -> &mut Local {
        self.used += 1;
        if !self.local.contains_key(s)
            && self.local.len() >= KEPT
            && let Some(old) = self.local.iter().min_by_key(|(_, (u, _))| *u).map(|(k, _)| k.clone())
        {
            self.local.remove(&old);
        }
        let used = self.used;
        let e = self.local.entry(s.to_string()).or_default();
        e.0 = used;
        &mut e.1
    }

    /// Rail frame `frame` of session `s`: the sections drawn the frame
    /// before are last frame's; a gap (the rail not drawn, another
    /// session) forgets them.
    fn begin(&self, s: &str, frame: u64) {
        let mut seen = self.seen.borrow_mut();
        if seen.of.as_deref() != Some(s) {
            *seen = Seen { of: Some(s.to_string()), frame, ..Seen::default() };
        } else if seen.frame != frame {
            let gap = seen.frame + 1 != frame;
            seen.last = std::mem::take(&mut seen.this);
            seen.frame = frame;
            if gap {
                seen.last.clear();
            }
        }
    }

    /// Section `d` drawn in rail frame `frame` of session `s`: how it comes
    /// (§6.3). It arrives (glides open) only into a rail drawn the frame
    /// before for the same session, with sections: a session opened, the
    /// panel unfolded or a history attached draws what it holds at once
    /// (history never animates).
    fn drew(&self, s: &str, frame: u64, d: Drew) -> Come {
        self.begin(s, frame);
        let mut seen = self.seen.borrow_mut();
        let row = crate::blocks::row_id(&d.sec.s.block.id);
        let come = if !seen.last.is_empty() && !seen.last.contains_key(&row) { Come::Arrive } else { Come::Stay };
        seen.this.insert(row, d);
        come
    }

    /// Slot `k`'s sections drawn the frame before that this frame's rail
    /// does not draw (`now`: the rows it draws, either slot): leaving, in
    /// the order they were drawn.
    fn leaving(&self, s: &str, frame: u64, k: usize, now: &HashSet<u64>) -> Vec<Drew> {
        self.begin(s, frame);
        let seen = self.seen.borrow();
        let mut gone: Vec<Drew> = seen.last.iter().filter(|(r, d)| d.k == k && !now.contains(r)).map(|(_, d)| d.clone()).collect();
        gone.sort_by_key(|d| d.at);
        gone
    }

    /// Section `d`, leaving, still drawn this frame (it glides shut).
    fn still(&self, d: Drew) {
        let row = crate::blocks::row_id(&d.sec.s.block.id);
        self.seen.borrow_mut().this.insert(row, d);
    }
}

fn hash(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// A `+N` row's painted tag: its leftovers.
pub(crate) fn more_tag(m: &ar::More) -> Tag {
    let ids: Vec<String> = m.ids.iter().map(|r| r.to_string()).collect();
    let (a, b) = (hash(&ids), hash((&ids, 1u8)));
    Tag::parse(&format!("{a:016x}{b:016x}")).unwrap_or_else(|| Tag::parse(&"0".repeat(32)).expect("a tag"))
}

impl AppView {
    /// What this desk shows of store `live`'s agent sections (D6b's seam:
    /// its dismissals, pins and slot moves; D6a shows everything where the
    /// board places it).
    pub(crate) fn rail_visibility(&self, _live: LiveId) -> &dyn RailVisibility {
        #[cfg(test)]
        if let Some(v) = self.agent_rail.vis.as_deref() {
            return v;
        }
        &ALL_SHOWN
    }

    /// Tests: what the rail retains of its renderer state: its painted
    /// targets and the chrome's reveals.
    #[cfg(test)]
    pub(crate) fn rail_retained(&self) -> (usize, usize) {
        (self.agent_rail.lease.painted.borrow().len(), self.fx.chrome().reveal_count())
    }

    /// The agent zone's footer (D6b's seam: `1 dismissed · show`). D6a has
    /// none.
    pub(crate) fn rail_footer(&self, _live: LiveId, _p: &Palette, _cx: &Context<Self>) -> Option<AnyElement> {
        None
    }

    /// The session the panel describes: the focused pane's.
    pub(crate) fn rail_sid(&self) -> Option<&str> {
        self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.as_str())
    }

    /// The rail of board `d` for store `live`, with the panel's local state.
    pub(crate) fn rail_of(&self, live: LiveId, d: &Drawn) -> Layout {
        let local = self.model.lives[live].active.as_ref().map_or(&*EMPTY, |a| self.agent_rail.local(&a.session_id));
        ar::layout(d, self.rail_visibility(live), local)
    }

    /// The board the panel's rail draws (the focused pane's session's) and
    /// what it draws of it.
    pub(crate) fn rail_layout(&self) -> Option<(Arc<Drawn>, Layout)> {
        let live = self.panes.cur.live;
        let d = self.model.lives[live].transcript.drawn()?.clone();
        let l = self.rail_of(live, &d);
        Some((d, l))
    }

    /// The strip's `◆N`: the agent sections the folded panel holds.
    pub(crate) fn rail_count(&self) -> usize {
        self.rail_layout().map_or(0, |(_, l)| l.sections)
    }

    /// A section's content width in the rail (a header's, a note's text's):
    /// the panel's content as last drawn, less the spine and the padding.
    pub(crate) fn rail_text_w(&self) -> f32 {
        let inner = self.agent_rail.inner.get();
        let inner = if inner > 0. { inner } else { crate::app::FLOORS.1 - EDGE - 2. * INSET };
        (inner - SPINE - 2. * PAD).max(1.)
    }

    /// A note's lines as laid out in the rail: its text as drawn, at its
    /// face, size and width, wrapped by the window's text system as its
    /// text element wraps it (the same call, cached for the frame: the
    /// layout reuses it). The renderer's cut and `more`, and the lease's
    /// shape, are this count. No window drawn yet: the estimate.
    pub(crate) fn rail_note_lines(&self, text: &str) -> usize {
        let shown = crate::ask::shown(text, cards_spec::Lines::Multi);
        let w = self.rail_text_w();
        let Some(ts) = self.agent_rail.text.as_ref() else { return crate::blocks::note_lines(&shown, w / (size::TITLE * 0.5)) };
        let run = TextRun {
            len: shown.len(),
            font: font(fonts::SANS),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let est = crate::blocks::note_lines(&shown, w / (size::TITLE * 0.5));
        match ts.shape_text(shown.into(), px(size::TITLE), &[run], Some(px(w)), None) {
            Ok(lines) => lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum::<usize>().max(1),
            Err(_) => est,
        }
    }

    /// `text`'s width in the mono face at `size`, as drawn (no window drawn
    /// yet: its 0.6 em columns).
    fn rail_mono_w(&self, text: &str, size: f32) -> f32 {
        let Some(ts) = self.agent_rail.text.as_ref() else { return text.chars().count() as f32 * size * 0.6 };
        let run = TextRun {
            len: text.len(),
            font: font(fonts::MONO),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(ts.shape_line(text.to_string().into(), px(size), &[run], None).width).ceil()
    }

    /// A diff's lines and foot as the rail draws them, for store `live`.
    fn rail_diff(&self, live: LiveId, s: &Shown) -> Option<(usize, bool)> {
        let Body::Diff(d) = &s.block.body else { return None };
        let (lines, foot) = self.diff_rows(live, d);
        Some((lines.len(), !foot.is_empty()))
    }

    /// The rail's structure for store `live` on board `d` (the lease's).
    fn rail_shape(&self, live: LiveId, d: &Drawn) -> u64 {
        let l = self.rail_of(live, d);
        let local = self.model.lives[live].active.as_ref().map_or(&*EMPTY, |a| self.agent_rail.local(&a.session_id));
        ar::shape(&l, local, &|t| self.rail_note_lines(t), &|s| self.rail_diff(live, s))
    }

    /// Whether board `new` for store `live` would change the rail under the
    /// rail's lease (module docs), its board drawn `old`.
    pub(crate) fn rail_disturbs(&self, live: LiveId, old: Option<&Drawn>, new: &Drawn) -> bool {
        let r = &self.agent_rail;
        if !r.lease.active(self.fx.now()) || self.panes.cur.live != live {
            return false;
        }
        if r.of.as_deref() != self.rail_sid() {
            return false;
        }
        let Some(old) = old else { return false };
        self.rail_shape(live, old) != self.rail_shape(live, new)
    }

    /// Board `d` drawn for store `live`: its session's local state keeps
    /// what it still holds, and the rail's next frame draws an agent op.
    pub(crate) fn rail_landed(&mut self, live: LiveId, d: &Drawn) {
        let Some(s) = self.model.lives[live].active.as_ref().map(|a| a.session_id.clone()) else { return };
        let refs: HashSet<Ref> = d
            .rail
            .iter()
            .flatten()
            .chain(d.rows.values().flat_map(|r| r.blocks.iter()))
            .flat_map(|s| std::iter::once(s).chain(&s.members))
            .map(|s| s.block.id.clone())
            .collect();
        self.agent_rail.local_mut(&s).of(d, &refs);
        if self.panes.cur.live == live {
            let l = &self.agent_rail.lease;
            l.agent_frame.set(Some(l.frame.get() + 1));
            self.agent_rail.landed.set(Some(l.frame.get() + 1));
        }
    }

    /// NOW for the panel's session (§4).
    pub(crate) fn rail_now(&self) -> Now {
        let live = self.panes.cur.live;
        let l = &self.model.lives[live];
        let t = &l.transcript;
        let items = t.items();
        // The live turn: from its user row on.
        let from = if t.busy() {
            items.iter().rposition(|i| i.kind == synaps_client::transcript::ItemKind::User).unwrap_or(0)
        } else {
            items.len()
        };
        let turn = &items[from..];
        let verb = |c: &synaps_client::transcript::ToolCall| -> (String, String) {
            let input = crate::app::input_of(&c.input);
            let arg = crate::app::command_of(input.as_ref()).unwrap_or_else(|| c.input.clone());
            let card = crate::turns::Card::of(c, None, &crate::app::one_line(&arg, 200), input.as_ref());
            (card.word.to_string(), card.target.to_string())
        };
        let prompt = t.pending_prompts().first().map(|p| {
            // What it waits to allow: the call it gates (the turn's last
            // unfinished), else the scope it would latch.
            let open = |c: &&synaps_client::transcript::ToolCall| {
                matches!(
                    c.status,
                    Some(
                        synaps_client::transcript::ToolStatus::Running
                            | synaps_client::transcript::ToolStatus::Queued
                            | synaps_client::transcript::ToolStatus::Streaming
                    )
                )
            };
            match turn.iter().rev().filter_map(|i| i.tool.as_ref()).find(open) {
                _ if p.is_secret() => "a secret".to_string(),
                Some(c) => format!("allow {}", verb(c).0),
                None => match p.confirm_scope.as_ref() {
                    Some(s) => format!("allow {}", s.tool.as_deref().unwrap_or("it")),
                    None => "answer".to_string(),
                },
            }
        });
        let card = self.open_cards(live).iter().any(|(painted, _)| painted.wait);
        ar::now_of(prompt, card, t.busy(), turn, &verb)
    }

    /// The rail's frame: counted as it draws (its targets' just-moved).
    /// Another session's: its painted targets are not this one's.
    pub(crate) fn rail_frame(&self) {
        let f = &self.agent_rail.lease.frame;
        f.set(f.get() + 1);
        self.agent_rail.drew_rows.borrow_mut().clear();
        let s = self.rail_sid();
        let mut of = self.agent_rail.painted_of.borrow_mut();
        if of.as_deref() != s {
            self.agent_rail.lease.painted.borrow_mut().clear();
            *of = s.map(str::to_string);
        }
    }

    /// Key `key`'s glide toward `open` (the chrome's reveal), and where it
    /// came from: a target changed in the frame that draws a board
    /// published is an agent op's, in any other the user's (their fold, a
    /// `+N` list, a resize: never held, never marked). One of an agent op's
    /// still moving holds where it was drawn while the rail's lease holds
    /// (§6.2: the geometry under the pointer stays, the glide in flight
    /// too), and goes on from there once it ends.
    fn rail_reveal(&self, key: u64, open: bool) -> (Option<f32>, Rc<Cell<f32>>) {
        let r = &self.agent_rail;
        let (frame, now) = (r.lease.frame.get(), self.fx.now());
        let chrome = self.fx.chrome();
        let mut g = r.glides.borrow_mut();
        let g = &mut *g;
        if g.to.insert(key, open) != Some(open) {
            if r.landed.get() == Some(frame) {
                g.agent.insert(key, Flight { k: f32::NAN, sink: Rc::default(), held: None });
            } else {
                g.agent.remove(&key);
            }
        }
        if let Some(e) = g.agent.get_mut(&key).filter(|e| !e.k.is_nan()) {
            if r.lease.active(now) {
                e.held.get_or_insert(now);
                g.held = true;
                chrome.hold_reveal(key);
                return (Some(e.k), e.sink.clone());
            }
            if let Some(since) = e.held.take() {
                chrome.resume_reveal(key, now.saturating_duration_since(since));
            }
        }
        let (shown, sink) = chrome.reveal(key, open, Duration::ZERO, 0);
        match shown {
            Some(k) => {
                if let Some(e) = g.agent.get_mut(&key) {
                    (e.k, e.sink) = (k, sink.clone());
                }
            }
            None => {
                g.agent.remove(&key);
            }
        }
        (shown, sink)
    }

    /// The rail's frame drawn (both slots). What it keeps of its renderer
    /// state keeps to what it drew: its painted targets and its glides in
    /// the chrome's motion go with their rows (a section leaving keeps its
    /// own until its glide is over; none of another session's, a new
    /// generation's gone blocks or a rail no longer drawn), so they are
    /// bounded by the board's caps for the session's life. While an agent
    /// op's glides move, and in the frame they land in, the rail's frames
    /// draw an agent op: what they move is just moved, from the motion's
    /// end. A glide held asks one frame, at the lease's end.
    pub(crate) fn rail_retire(&self) {
        let r = &self.agent_rail;
        let drew = r.drew_rows.borrow();
        r.lease.painted.borrow_mut().retain(|row, _| drew.contains(row));
        let keys: HashSet<u64> = drew.iter().flat_map(|row| [motion::key(ARRIVE, *row), motion::key(BODY, *row)]).collect();
        let mut g = r.glides.borrow_mut();
        let gone: Vec<u64> = g.to.keys().filter(|k| !keys.contains(k)).copied().collect();
        for k in &gone {
            g.to.remove(k);
            g.agent.remove(k);
        }
        self.fx.chrome().forget_reveals(&gone);
        let flight = !g.agent.is_empty();
        if flight || g.flight {
            r.lease.agent_frame.set(Some(r.lease.frame.get()));
        }
        g.flight = flight;
        if std::mem::take(&mut g.held)
            && !r.lease.pressed
            && let Some(t) = r.lease.moved
        {
            self.fx.want_at(t + crate::lease::LEASE);
        }
    }

    /// An agent op's glides in flight in the rail (moving, or held).
    pub(crate) fn rail_gliding(&self) -> bool {
        self.agent_rail.glides.borrow().flight
    }

    /// Slot `slot`'s zone in the panel `width` px wide, if the rail has one:
    /// the upper slot's header, NOW, its sections and `+N`; the end slot's
    /// divider and its own. `faded`: the panel shows last numbers seen.
    pub(crate) fn render_agent(&self, slot: Slot, width: f32, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let (_, l) = self.rail_layout()?;
        if !l.any() {
            return None;
        }
        let k = ar::slot_ix(slot);
        if slot == Slot::End && l.slots[k].is_empty() {
            return None;
        }
        let session = self.rail_sid()?.to_string();
        let live = self.panes.cur.live;
        let local = self.agent_rail.local(&session);
        let inner = (width - EDGE - 2. * INSET).max(0.);
        self.agent_rail.inner.set(inner);
        let streaming = self.model.lives[live].transcript.busy();
        let frame = self.agent_rail.lease.frame.get();
        let mut zone = v_flex()
            .id(("agent-zone", k))
            .debug_selector(move || format!("agent-zone {k}"))
            .w_full()
            .px(px(INSET))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.rail_move(e.pressed_button.is_some(), cx)))
            .on_hover(cx.listener(|this, over: &bool, _, cx| this.rail_over(*over, cx)));
        zone = match slot {
            Slot::Upper => {
                let count = if l.dismissed > 0 {
                    format!("{} · {} dismissed", plural(l.sections, "section"), l.dismissed)
                } else {
                    plural(l.sections, "section")
                };
                let now = self.rail_now();
                zone.pt(px(24.))
                    .pb(px(26.))
                    .border_b_1()
                    .border_color(p.hair)
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(8.))
                            .child(section(label::AGENT, p))
                            .child(div().size(px(6.)).flex_none().bg(p.acc))
                            .child(div().flex_1())
                            .child(mono(count.clone(), size::META, p.dim_ch).debug_selector(move || format!("agent-count {count}"))),
                    )
                    .child(self.now_line(&now, streaming, p))
            }
            Slot::End => zone.pt(px(18.)).pb(px(26.)).border_t_1().border_color(p.hair).child(
                h_flex()
                    .items_center()
                    .gap(px(8.))
                    .debug_selector(|| "agent-divider".into())
                    .child(div().h(px(1.)).w(px(12.)).bg(p.hair))
                    .child(display(label::AGENT, size::D_S, p.dim_ch))
                    .child(div().h(px(1.)).flex_1().bg(p.hair)),
            ),
        };
        // This frame's sections, in order: the slot's own, then those
        // listed under its `+N`; and those drawn last frame that it no
        // longer draws (pushed past the cap, a list closed, removed),
        // each where it was, gliding shut.
        let now_rows: HashSet<u64> =
            l.sections().filter(|s| s.state != State::Over).map(|s| crate::blocks::row_id(&s.s.block.id)).collect();
        let gone = self.agent_rail.leaving(&session, frame, k, &now_rows);
        let here = l.slots[k].iter().filter(|s| !matches!(s.state, State::Over | State::Listed)).map(|s| (false, s));
        let listed = l.slots[k].iter().filter(|s| s.state == State::Listed).map(|s| (true, s));
        let mut draw: Vec<(Drew, Come)> = Vec::new();
        for (at, (listed, sec)) in here.chain(listed).enumerate() {
            let d = Drew { k, at, listed, sec: sec.clone() };
            let come = self.agent_rail.drew(&session, frame, d.clone());
            draw.push((d, come));
        }
        for d in gone {
            let row = crate::blocks::row_id(&d.sec.s.block.id);
            if self.rail_reveal(motion::key(ARRIVE, row), false).0.is_none() {
                continue;
            }
            // After the last still drawn that came before it.
            let was = |x: &Drew| self.agent_rail.seen.borrow().last.get(&crate::blocks::row_id(&x.sec.s.block.id)).map(|w| w.at);
            let at = draw
                .iter()
                .rposition(|(x, _)| x.listed == d.listed && was(x).is_some_and(|w| w < d.at))
                .map_or_else(|| draw.iter().position(|(x, _)| x.listed == d.listed).unwrap_or(draw.len()), |i| i + 1);
            self.agent_rail.still(d.clone());
            draw.insert(at, (d, Come::Leave));
        }
        {
            let mut drew = self.agent_rail.drew_rows.borrow_mut();
            drew.extend(draw.iter().map(|(d, _)| crate::blocks::row_id(&d.sec.s.block.id)));
            if l.more[k].is_some() {
                drew.insert(MORE_ROW | k as u64);
            }
        }
        for (d, come) in draw.iter().filter(|(d, _)| !d.listed) {
            zone = zone.child(self.rail_section(&d.sec, local, live, streaming, *come, p, cx));
        }
        if let Some(m) = &l.more[k] {
            zone = zone.child(self.more_row(k, m, p, cx));
        }
        for (d, come) in draw.iter().filter(|(d, _)| d.listed) {
            zone = zone.child(self.rail_section(&d.sec, local, live, streaming, *come, p, cx));
        }
        if slot == Slot::Upper {
            zone = zone.children(self.rail_footer(live, p, cx));
        }
        Some(zone.into_any_element())
    }

    /// NOW (§4): one fixed 22 px mono line, its glyph running on the beat
    /// only while the turn streams.
    fn now_line(&self, now: &Now, streaming: bool, p: &Palette) -> Div {
        let life = match now {
            Now::Prompt(_) | Now::Card => Life::Wait,
            Now::Running { .. } | Now::Thinking if streaming => Life::Run,
            Now::Writing if streaming => Life::Stream,
            Now::Idle => Life::Done,
            _ => Life::Seen,
        };
        let text = now.text();
        let ink = match now {
            Now::Prompt(_) | Now::Card => p.acc,
            Now::Idle => p.dim_ch,
            _ => p.prose_ch,
        };
        h_flex()
            .mt(px(14.))
            .h(px(NOW_H))
            .gap(px(8.))
            .items_center()
            .whitespace_nowrap()
            .debug_selector({
                let t = format!("agent-now {text}");
                move || t.clone()
            })
            .child(display(label::NOW, size::D_S, p.dim_ch).flex_none())
            .child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("agent-now-life {life:?}"))
                    .child(glyph::indicator(&self.fx, life, p.chrome, p, None, None)),
            )
            .child(mono(text, size::META, ink).min_w_0().overflow_hidden().text_ellipsis())
    }

    /// A slot's `+N` row: its leftovers, a painted target.
    fn more_row(&self, k: usize, m: &ar::More, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let row = MORE_ROW | k as u64;
        let tag = more_tag(m);
        let press = RailPress { at: Target { row, tag }, act: RailAct::List };
        let text = ar::more_text(m);
        let says = self.agent_rail.lease.note.filter(|n| n.0 == row).map(|n| n.1);
        div()
            .id(("agent-more", k))
            .relative()
            .mt(px(8.))
            .h(px(LINE))
            .w_full()
            .flex()
            .items_center()
            .gap(px(GAP))
            .cursor_pointer()
            .debug_selector({
                let t = format!("agent-more {k} {text}");
                move || t.clone()
            })
            .child(mono(text, size::META, p.dim_ch).flex_1().min_w_0().overflow_hidden().text_ellipsis())
            .children(says.map(|n| mono(n, size::META, p.acc).flex_none().debug_selector(move || format!("agent-says {row} {n}"))))
            .child(div().flex_none().child(glyph::turn(if m.open { 1. } else { 0. }, p.dim_ch)))
            .child(crate::lease::probe(&self.agent_rail.lease, &self.fx, row, tag, row))
            .on_hover(cx.listener(move |this, over: &bool, _, _| this.rail_hover(row, tag, *over)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.rail_down(press, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, _: &MouseUpEvent, _, cx| this.rail_up(row, RailAct::List, cx)))
            .into_any_element()
    }

    /// One section (module docs).
    #[allow(clippy::too_many_arguments)]
    fn rail_section(
        &self,
        sec: &Section,
        local: &Local,
        live: LiveId,
        streaming: bool,
        come: Come,
        p: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let s = &sec.s;
        let row = crate::blocks::row_id(&s.block.id);
        let tag = s.block.tag;
        if let State::Tombstone(t) = &sec.state {
            let t = crate::ask::shown(&t.text, cards_spec::Lines::Single);
            return h_flex()
                .mt(px(8.))
                .h(px(24.))
                .w_full()
                .items_center()
                .debug_selector(move || format!("agent-tomb {row}"))
                .child(mono(t, size::META, p.dim_ch).min_w_0().overflow_hidden().text_ellipsis())
                .into_any_element();
        }
        let open = sec.state == State::Open;
        let head = self.rail_head(sec, open, streaming, p, cx);
        // The body opens and folds on the glide; one listed or folded has
        // its summary line, or nothing.
        let (shown, sink) = self.rail_reveal(motion::key(BODY, row), open);
        let body = (open || shown.is_some()).then(|| {
            let b = self.rail_body(s, &sec.tombs, local, live, streaming, p, cx);
            crate::turns::revealed(b, shown, sink).debug_selector(move || format!("agent-body {row}"))
        });
        let summary = (sec.state == State::Summary).then(|| {
            let t = ar::summary(s);
            h_flex()
                .h(px(LINE))
                .w_full()
                .items_center()
                .debug_selector({
                    let t = format!("agent-summary {row} {t}");
                    move || t.clone()
                })
                .child(mono(t, size::META, p.dim_t).min_w_0().overflow_hidden().text_ellipsis())
        });
        let el = div()
            .id(("agent-section", row))
            .debug_selector(move || format!("agent-section {row}"))
            .relative()
            .mt(px(ABOVE))
            .w_full()
            .bg(p.card_d)
            .pl(px(SPINE + PAD))
            .pr(px(PAD))
            .pb(px(if open || summary.is_some() { 6. } else { 0. }))
            // The agent's frame: the spine (§4: a block titled "PROMPT
            // CACHE" still reads as the agent's).
            .child(div().absolute().left_0().top_0().bottom_0().w(px(SPINE)).bg(p.acc).debug_selector(move || format!("agent-spine {row}")))
            .child(head)
            .children(body)
            .children(summary)
            .child(crate::lease::probe(&self.agent_rail.lease, &self.fx, row, tag, row))
            .on_hover(cx.listener(move |this, over: &bool, _, _| this.rail_hover(row, tag, *over)));
        // Arriving: from nothing, on the glide (the chrome's motion: Off
        // and Reduced, at once); shut first in its first frame, then open.
        // Leaving: to nothing, the same way.
        if come == Come::Arrive {
            let _ = self.rail_reveal(motion::key(ARRIVE, row), false);
        }
        let (grow, gsink) = self.rail_reveal(motion::key(ARRIVE, row), come != Come::Leave);
        let el = crate::turns::revealed(el.into_any_element(), grow, gsink)
            .debug_selector(move || format!("agent-glide {row}"))
            .into_any_element();
        let keep = self.agent_rail.keep.get().filter(|k| k.0 == row);
        if keep.is_some() { Mark { child: el, at: self.agent_rail.mark.clone() }.into_any_element() } else { el }
    }

    /// A section's 28 px header: its life glyph, its title, the `AGENT`
    /// chip, `rN` (or the lease's note), the chevron. Its fold's target.
    fn rail_head(&self, sec: &Section, open: bool, streaming: bool, p: &Palette, cx: &Context<Self>) -> Stateful<Div> {
        let s = &sec.s;
        let b = &s.block;
        let row = crate::blocks::row_id(&b.id);
        let tag = b.tag;
        let (life, _) = crate::blocks::life_of(&b.body, streaming);
        let glow = |field: u64, v: u64, c: Hsla| self.fx.glow(motion::key(crate::blocks::GLOW_KEYS, row ^ field.rotate_left(13)), v, c);
        let title_c = glow(1, crate::blocks::bound_value(s, |_| s.bound.as_ref().map(|b| b.title.clone())), p.prose_t);
        let title = crate::blocks::title_of(s);
        let rev = if b.pending {
            "pending".to_string()
        } else if b.replaced.is_some() {
            format!("r{} · replaced", b.rev)
        } else {
            format!("r{}", b.rev)
        };
        let says = self.agent_rail.lease.note.filter(|n| n.0 == row).map(|n| n.1);
        let press = RailPress { at: Target { row, tag }, act: RailAct::Fold };
        let turn = self.fx.chrome().turn(motion::key(TURN, row), open);
        // The header's fixed chrome (its glyph, the AGENT chip, the
        // chevron, the gaps) holds its width: the title gives way first,
        // down to TITLE_MIN, then the revision is cut (whole in its
        // tooltip).
        let chip = self.rail_mono_w("AGENT", size::META) + 2. * CHIP_PAD;
        let fixed = glyph::LIFE + chip + SPACER + CHEVRON + 5. * GAP;
        let rev_max = (self.rail_text_w() - fixed - TITLE_MIN).max(REV_MIN);
        h_flex()
            .id(("agent-head", row))
            .h(px(HEAD))
            .w_full()
            .gap(px(GAP))
            .items_center()
            .whitespace_nowrap()
            .font_family(fonts::SANS)
            .text_px(size::TITLE)
            .cursor_pointer()
            .debug_selector(move || format!("agent-head {row}"))
            .child(div().flex_none().child(glyph::indicator(&self.fx, life, p.card_d, p, None, None)))
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(title_c)
                    .debug_selector({
                        let t = format!("agent-title {row} {title}");
                        move || t.clone()
                    })
                    .child(title),
            )
            // The agent's chip: an agent section always carries it (§4).
            .child(
                div()
                    .flex_none()
                    .px(px(CHIP_PAD))
                    .bg(p.well_d)
                    .font_family(fonts::MONO)
                    .text_px(size::META)
                    .text_color(p.dim_t)
                    .debug_selector(move || format!("agent-chip {row}"))
                    .child("AGENT"),
            )
            .child(div().flex_1().min_w(px(SPACER)))
            .child(match says {
                Some(n) => mono(n, size::META, p.acc)
                    .flex_shrink(1.)
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .debug_selector(move || format!("agent-says {row} {n}"))
                    .into_any_element(),
                None => {
                    let whole = rev.clone();
                    mono(rev.clone(), size::META, p.dim_t)
                        .id(("agent-rev", row))
                        .flex_none()
                        .max_w(px(rev_max))
                        .overflow_hidden()
                        .text_ellipsis()
                        .debug_selector({
                            let t = format!("agent-rev {row} {rev}");
                            move || t.clone()
                        })
                        .tooltip(move |window, cx| crate::overlay::tip(whole.clone(), window, cx))
                        .into_any_element()
                }
            })
            .child(
                div()
                    .flex_none()
                    .w(px(CHEVRON))
                    .flex()
                    .items_center()
                    .justify_end()
                    .debug_selector(move || format!("agent-chevron {row}"))
                    .child(glyph::turn(turn, p.dim_t)),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.rail_down(press, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, _: &MouseUpEvent, _, cx| this.rail_up(row, RailAct::Fold, cx)))
    }

    /// A section's body in its kind's layout, in 20 px lines, at most
    /// [`ar::LINES`] (§4's overflow per kind).
    #[allow(clippy::too_many_arguments)]
    fn rail_body(
        &self,
        s: &Shown,
        tombs: &[Option<ar::Tomb>],
        local: &Local,
        live: LiveId,
        streaming: bool,
        p: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let b = &s.block;
        let row = crate::blocks::row_id(&b.id);
        let glow = |field: u64, v: u64, c: Hsla| self.fx.glow(motion::key(crate::blocks::GLOW_KEYS, row ^ field.rotate_left(13)), v, c);
        let line = || h_flex().h(px(LINE)).w_full().items_center().gap(px(6.)).whitespace_nowrap();
        let text = |t: String, c: Hsla| mono(t, size::META, c).min_w_0().overflow_hidden().text_ellipsis();
        let mut col = v_flex().w_full().font_family(fonts::MONO).text_px(size::META);
        match &b.body {
            Body::Plan(pl) => {
                let id = b.id.clone();
                let (start, n, more) = ar::plan_window(&pl.steps, local.plan_top.get(&b.id).copied());
                for (j, st) in pl.steps.iter().enumerate().skip(start).take(n) {
                    let word = crate::blocks::step_word(st.state, streaming);
                    let ink = match st.state {
                        StepState::Done | StepState::Skipped => p.dim_t,
                        StepState::Failed => p.err,
                        _ => p.prose_t,
                    };
                    col = col.child(
                        line()
                            .debug_selector({
                                let t = format!("agent-step {row} {j}");
                                move || t.clone()
                            })
                            .child(div().flex_none().child(glyph::indicator(
                                &self.fx,
                                crate::blocks::step_life(st.state, streaming),
                                p.card_d,
                                p,
                                None,
                                None,
                            )))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .font_family(fonts::SANS)
                                    .text_color(ink)
                                    .child(st.label.clone()),
                            )
                            .when(!word.is_empty(), |d| {
                                d.child(
                                    mono(word, size::META, crate::blocks::word_color(crate::blocks::step_life(st.state, streaming), p))
                                        .flex_none(),
                                )
                            }),
                    );
                }
                if let Some((hidden, done)) = more {
                    let t = format!("+{hidden} more · {done} done");
                    col = col.child(
                        line()
                            .debug_selector({
                                let t = format!("agent-plan-more {row} {t}");
                                move || t.clone()
                            })
                            .child(text(t, p.dim_t)),
                    );
                }
                // The window scrolls inside on the wheel.
                let total = pl.steps.len();
                return col
                    .id(("agent-plan", row))
                    .on_scroll(cx.listener(move |this, e: &ScrollEvent, _, cx| {
                        let dy = f32::from(e.delta.pixel_delta(px(LINE)).y);
                        if this.rail_wheel(&id, total, dy) {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .into_any_element();
            }
            Body::Status(st) => {
                let text_c = glow(
                    3,
                    crate::blocks::bound_value(s, |b| if let Body::Status(x) = b { Some(x.text.clone()) } else { None }),
                    p.prose_t,
                );
                col = col.child(line().child(text(crate::ask::shown(&st.text, cards_spec::Lines::Single), text_c)));
                let detail = st.detail.as_deref().map(|d| crate::ask::shown(d, cards_spec::Lines::Single)).unwrap_or_default();
                col = col.child(line().child(text(detail, p.dim_t).flex_1()).children(st.progress.map(|q| {
                    let k = if q.of > 0 { (q.n as f32 / q.of as f32).clamp(0., 1.) } else { 0. };
                    h_flex()
                        .flex_none()
                        .gap(px(6.))
                        .items_center()
                        .child(div().w(px(40.)).h(px(4.)).bg(p.well_d).child(div().h_full().w(px((40. * k).round())).bg(p.acc)))
                        .child(mono(format!("{}/{}", q.n, q.of), size::META, p.dim_t))
                })));
            }
            Body::Metric(mt) => {
                let value_c =
                    glow(7, crate::blocks::bound_value(s, |b| if let Body::Metric(x) = b { Some(x.value.clone()) } else { None }), p.ink);
                let unit = mt.unit.as_deref().map(|u| format!(" {u}")).unwrap_or_default();
                col = col.child(line().child(text(format!("{}{unit}", mt.value), value_c).font_weight(FontWeight::SEMIBOLD)));
                let was = mt.was.as_deref().map(|w| format!(" · was {w}")).unwrap_or_default();
                col = col.child(line().child(text(format!("{}{was}", mt.label), p.dim_t)));
                if !mt.series.is_empty() {
                    let from = mt.series.len().saturating_sub(ar::SPARK_MAX);
                    col = col.child(line().debug_selector(move || format!("agent-spark {row}")).child(crate::blocks::spark(
                        &mt.series[from..],
                        14.,
                        p,
                    )));
                }
            }
            Body::File(f) => {
                let path = crate::turns::tilde(&f.path, false);
                let at = f.line.map(|l| format!(":{l}")).unwrap_or_default();
                let note = f.note.as_deref().map(|n| format!(" · {n}")).unwrap_or_default();
                col = col.child(line().child(text(format!("{path}{at}{note}"), p.prose_t)));
            }
            Body::Diff(d) => {
                let (lines, foot) = self.diff_rows(live, d);
                let (shown, over, _) = ar::diff_shown(lines.len(), !foot.is_empty());
                for (kind, t) in lines.iter().take(shown) {
                    let (sign, fg, bg) = match kind {
                        Some(crate::files::LineKind::Add) => ("+", p.ink, Some(p.add_bg)),
                        Some(crate::files::LineKind::Del) => ("−", p.ink, Some(p.del_bg)),
                        Some(_) => (" ", p.res, None),
                        None => (" ", p.dim_o, None),
                    };
                    col = col.child(
                        line()
                            .when_some(bg, |d, bg| d.bg(bg))
                            .child(mono(sign, size::META, p.dim_o).flex_none().w(px(10.)))
                            .child(text(t.to_string(), fg)),
                    );
                }
                if over {
                    let t = format!("+{} lines", lines.len() - shown);
                    col = col.child(
                        line()
                            .debug_selector({
                                let t = format!("agent-diff-more {row} {t}");
                                move || t.clone()
                            })
                            .child(text(t, p.dim_t)),
                    );
                }
                if !foot.is_empty() {
                    col = col.child(line().child(text(foot.to_string(), p.dim_t)));
                }
            }
            Body::Note(n) => {
                // Its lines as laid out: cut past them at a fixed height,
                // with `more` (§4), whatever its alphabet.
                let laid = self.rail_note_lines(&n.text);
                let (_, cut) = ar::note_shown(laid, local.more.contains(&b.id));
                let tall = laid > ar::NOTE_LINES;
                let text_c =
                    glow(6, crate::blocks::bound_value(s, |b| if let Body::Note(x) = b { Some(x.text.clone()) } else { None }), p.prose_t);
                col = col.child(
                    div()
                        .w_full()
                        .font_family(fonts::SANS)
                        .text_px(size::TITLE)
                        .font_weight(FontWeight::NORMAL)
                        .line_height(px(LINE))
                        .text_color(text_c)
                        .whitespace_normal()
                        .when(cut, |d| d.h(px(LINE * ar::NOTE_LINES as f32)).overflow_hidden())
                        .debug_selector(move || format!("agent-note {row}"))
                        .child(crate::ask::shown(&n.text, cards_spec::Lines::Multi)),
                );
                if tall {
                    let tag = b.tag;
                    let press = RailPress { at: Target { row, tag }, act: RailAct::More };
                    let word = if cut { "more" } else { "less" };
                    col = col.child(
                        line()
                            .id(("agent-note-more", row))
                            .cursor_pointer()
                            .debug_selector(move || format!("agent-note-more {row} {word}"))
                            .child(mono(word, size::META, p.acc))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.rail_down(press, cx)))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| this.rail_up(row, RailAct::More, cx)),
                            ),
                    );
                }
            }
            Body::Group(_) => {
                for (j, m) in s.members.iter().enumerate() {
                    let mrow = crate::blocks::row_id(&m.block.id);
                    // A member that is a tombstone (D6b's soft dismissal)
                    // draws as its tombstone: nothing of its content.
                    if let Some(Some(tomb)) = tombs.get(j) {
                        let t = crate::ask::shown(&tomb.text, cards_spec::Lines::Single);
                        col = col.child(
                            line()
                                .debug_selector({
                                    let s = format!("agent-member-tomb {row} {mrow} {t}");
                                    move || s.clone()
                                })
                                .child(text(t, p.dim_ch)),
                        );
                        continue;
                    }
                    let (life, _) = crate::blocks::life_of(&m.block.body, streaming);
                    col = col.child(
                        line()
                            .debug_selector(move || format!("agent-member {row} {mrow}"))
                            .child(div().flex_none().child(glyph::indicator(&self.fx, life, p.card_d, p, None, None)))
                            .child(
                                div()
                                    .flex_shrink(1.)
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .font_family(fonts::SANS)
                                    .text_color(p.prose_t)
                                    .child(crate::blocks::title_of(m)),
                            )
                            .child(text(ar::summary(m), p.dim_t).flex_1()),
                    );
                }
            }
        }
        col.into_any_element()
    }

    // -- Input --------------------------------------------------------------

    /// The pointer moved over the agent zone: the rail's lease (no frame).
    /// With no button down while a press is held, its release was lost.
    pub(crate) fn rail_move(&mut self, buttons: bool, cx: &mut Context<Self>) {
        let now = self.fx.now();
        let s = self.rail_sid().map(str::to_string);
        let r = &mut self.agent_rail;
        (r.lease.moved, r.lease.over) = (Some(now), true);
        if r.of != s {
            // Another session's press and note: none held.
            if r.of.is_some() {
                (r.lease.hover, r.down, r.lease.note, r.lease.pressed) = (None, None, None, false);
            }
            r.of = s;
        }
        if !buttons && (self.agent_rail.lease.pressed || self.panes.iter().any(|p| p.lease.pressed)) {
            self.lease_unpress(cx);
        }
    }

    /// The pointer over the agent zone or not.
    pub(crate) fn rail_over(&mut self, over: bool, cx: &mut Context<Self>) {
        self.agent_rail.lease.over = over;
        if !over {
            self.agent_rail.lease.hover = None;
            self.lease_release(cx);
        }
    }

    /// The pointer over rail target `row` (painted as `tag`) or off it.
    pub(crate) fn rail_hover(&mut self, row: u64, tag: Tag, over: bool) {
        let l = &mut self.agent_rail.lease;
        if over {
            l.hover = Some(Target { row, tag });
        } else if l.hover.is_some_and(|h| h.row == row) {
            l.hover = None;
        }
    }

    /// Pointer-down on rail target `press`: taken, the lease held; or,
    /// just moved by an agent op, nothing (once).
    pub(crate) fn rail_down(&mut self, press: RailPress, cx: &mut Context<Self>) {
        let now = self.fx.now();
        let row = press.at.row;
        let l = &mut self.agent_rail.lease;
        l.note = None;
        let moved = l.painted.borrow().get(&row).and_then(|p| p.moved).is_some_and(|t| now.saturating_duration_since(t) < JUST_MOVED);
        if moved {
            if let Some(p) = l.painted.borrow_mut().get_mut(&row) {
                p.moved = None;
            }
            (self.agent_rail.down, l.pressed) = (None, false);
            l.note = Some((row, MOVED));
        } else {
            self.agent_rail.down = Some(press);
            (l.pressed, l.moved) = (true, Some(now));
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Pointer-up on rail target `row`'s `act`: the press's target, if it
    /// is this one and the bound board still holds it, acts (local); else
    /// nothing, and the note.
    pub(crate) fn rail_up(&mut self, row: u64, act: RailAct, cx: &mut Context<Self>) {
        self.agent_rail.lease.pressed = false;
        let Some(down) = self.agent_rail.down.take().filter(|d| d.at.row == row && d.act == act) else { return };
        let live = self.panes.cur.live;
        let Some(session) = self.rail_sid().map(str::to_string) else { return };
        // The bound board now: the newest, drawn or waiting.
        let l = &self.model.lives[live];
        let newest = l.board.waiting.clone().or_else(|| l.transcript.drawn().cloned());
        let bound = newest.as_deref().map(|d| self.rail_of(live, d));
        let held = bound.as_ref().is_some_and(|b| match act {
            RailAct::List => {
                let k = (row & !MORE_ROW) as usize;
                b.more.get(k).and_then(Option::as_ref).is_some_and(|m| more_tag(m) == down.at.tag)
            }
            RailAct::Fold | RailAct::More => b
                .sections()
                .any(|s| crate::blocks::row_id(&s.s.block.id) == row && s.s.bound.as_ref().is_some_and(|x| x.tag == down.at.tag)),
        });
        // Drawn now: the layout acted on.
        let drawn = self.rail_layout().map(|(_, l)| l);
        match drawn.filter(|_| held) {
            Some(lay) => match act {
                RailAct::Fold => {
                    let Some(sec) = lay.sections().find(|s| crate::blocks::row_id(&s.s.block.id) == row) else { return };
                    let id = sec.s.block.id.clone();
                    if sec.state != State::Open {
                        // It expands: its header keeps its screen y.
                        let y = self.agent_rail.lease.painted.borrow().get(&row).map(|p| f32::from(p.bounds.top()));
                        if let Some(y) = y {
                            self.agent_rail.keep.set(Some((row, y, self.fx.now() + KEEP_FOR)));
                        }
                    }
                    self.agent_rail.local_mut(&session).toggle(&lay, &id);
                }
                RailAct::More => {
                    if let Some(sec) = lay.sections().find(|s| crate::blocks::row_id(&s.s.block.id) == row) {
                        let id = sec.s.block.id.clone();
                        let more = &mut self.agent_rail.local_mut(&session).more;
                        if !more.remove(&id) {
                            more.insert(id);
                        }
                    }
                }
                RailAct::List => {
                    let k = (row & !MORE_ROW) as usize;
                    let listed = &mut self.agent_rail.local_mut(&session).listed[k];
                    *listed = !*listed;
                }
            },
            None => self.agent_rail.lease.note = Some((row, CHANGED)),
        }
        if self.model.lives.ids().into_iter().any(|l| self.model.lives[l].board.waiting.is_some()) {
            self.arm_lease_wake(cx);
        }
        cx.notify();
    }

    /// The wheel over plan `id`'s window (`total` steps, `dy` px): it
    /// moves a step a line, if it can (else the panel scrolls).
    fn rail_wheel(&mut self, id: &Ref, total: usize, dy: f32) -> bool {
        let Some(session) = self.rail_sid().map(str::to_string) else { return false };
        if total <= ar::LINES {
            return false;
        }
        let Some((_, lay)) = self.rail_layout() else { return false };
        let Some(sec) = lay.section(id) else { return false };
        let Body::Plan(pl) = &sec.s.block.body else { return false };
        let local = self.agent_rail.local(&session);
        let (start, _, _) = ar::plan_window(&pl.steps, local.plan_top.get(id).copied());
        let steps = (-dy / LINE).round() as i64;
        let max = (total - ar::PLAN_ROWS) as i64;
        let to = (start as i64 + steps).clamp(0, max) as usize;
        if steps == 0 || to == start {
            return false;
        }
        self.agent_rail.local_mut(&session).plan_top.insert(id.clone(), to);
        true
    }
}

/// `n` and its noun, plural past one: `3 sections`.
fn plural(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// A section whose layout [`KeepY`] reads this frame: it notes its layout
/// id as it is laid out.
pub(crate) struct Mark {
    child: AnyElement,
    at: Rc<Cell<Option<LayoutId>>>,
}

impl IntoElement for Mark {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Mark {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let id = self.child.request_layout(window, cx);
        self.at.set(Some(id));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// The panel's scroll, around it: while a header clicked to expand keeps
/// its screen y (§4), each frame's offset puts its section where it was
/// (its layout read before the panel's own prepaint applies the offset,
/// which GPUI then clamps to what the panel can scroll: as far as it can).
pub(crate) struct KeepY {
    child: AnyElement,
    keep: Rc<Cell<Option<(u64, f32, Instant)>>>,
    mark: Rc<Cell<Option<LayoutId>>>,
    scroll: ScrollHandle,
    now: Instant,
}

impl KeepY {
    pub(crate) fn new(child: AnyElement, r: &RailState, scroll: &ScrollHandle, now: Instant) -> Self {
        KeepY { child, keep: r.keep.clone(), mark: r.mark.clone(), scroll: scroll.clone(), now }
    }
}

impl IntoElement for KeepY {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for KeepY {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.mark.set(None);
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        match self.keep.get() {
            Some((_, y, until)) if self.now < until => {
                if let Some(id) = self.mark.get() {
                    // Its top as laid out, the panel unscrolled (the
                    // section's, below its margin).
                    let top = f32::from(window.layout_bounds(id).top()) + ABOVE;
                    let off = self.scroll.offset();
                    let want = px((y - top).min(0.));
                    if want != off.y {
                        self.scroll.set_offset(point(off.x, want));
                    }
                }
            }
            Some(_) => self.keep.set(None),
            None => {}
        }
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
