//! Split panes, step 1 (split/architecture.md §7): the window's panes.
//!
//! Every per-view field that was on [`AppView`] lives on its [`Pane`]
//! now: the list over one transcript, its scroll and follow, the held
//! rooms, the composer, the tab strip's state, the card's UI state and
//! the arrival layer. There is exactly one pane, and nothing
//! behaves differently: [`Panes`] holds it as `cur`, a plain field, so
//! `self.panes.cur.list` borrows disjointly from `self.model` and from the
//! pane's other fields as `self.list` did.
//!
//! [`Column`] (the struct called `Pane` before this step) is still the
//! bundle a worker's view parks the session's list in: the same fields,
//! swapped exactly as before.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::*;

#[cfg(doc)]
use crate::app::AppView;
use crate::app::{BodyHold, CardChord, Column, FoldGap, RowCache, Slack, Toggled};
use crate::authorize::{CardExit, CardLook, PromptCard};
use crate::named::ResumeRequest;
use crate::theme::Metrics;
use crate::turns::{self, Disp, Layout, Summaries};

/// The most panes a window splits into (split/architecture.md §2.1).
pub(crate) const PANES_MAX: usize = 4;

/// A pane's id: stable for its life (split/architecture.md §2.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct PaneId(u8);

impl PaneId {
    /// The window's first pane, the only one in step 1.
    pub(crate) const FIRST: PaneId = PaneId(0);

    /// A key used inside the pane (a spring, a hover, a tray chip), salted
    /// so two panes never share one (split/architecture.md §6.3). The first
    /// pane's salt is 0: its keys are today's, unchanged.
    pub(crate) const fn salt(self, key: u64) -> u64 {
        key ^ ((self.0 as u64) << 40)
    }

    /// Pane `i` (tests: a second pane's column before step 3 opens one).
    #[cfg(test)]
    pub(crate) const fn nth(i: u8) -> Self {
        Self::of(i)
    }

    const fn of(i: u8) -> Self {
        assert!((i as usize) < PANES_MAX);
        PaneId(i)
    }

    /// Its slot in per-pane arrays ([`crate::motion::Motion::col`]).
    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One editor group: the list on screen and everything drawn around it.
pub struct Pane {
    pub(crate) id: PaneId,
    /// The live store its session tab shows ([`crate::live`]): every read
    /// of the session's transcript, numbers and workers, and every send,
    /// goes through it (`model.lives[pane.live]`).
    pub(crate) live: crate::live::LiveId,

    // The list over one transcript (architecture.md §1.1, §1.2: "C").
    /// Per transcript item: its strings ([`RowCache`]).
    pub(crate) rows: Vec<RowCache>,
    /// The virtual list's rows ([`turns::build`]): turns, runs, folds.
    pub(crate) disp: Vec<Disp>,
    pub(crate) summaries: Summaries,
    pub(crate) list: ListState,
    /// Rows whose open/closed state the user flipped from its default, and
    /// for a call the state they made it ([`Toggled`]).
    pub(crate) toggled: Toggled,
    /// Runs the user unfolded (by their first item's id).
    pub(crate) open_runs: HashSet<u64>,
    /// The rows an unfold under Full is revealing: tail-follow is held
    /// paused while they do ([`AppView::hold_unfold`]).
    pub(crate) unfold_hold: Vec<u64>,
    /// The transcript's `busy` at the last [`AppView::sync_list`]: when it
    /// falls, the last run may fold.
    pub(crate) synced_busy: bool,
    /// The files ledger's generation the rows' counts are from.
    pub(crate) files_gen: u64,
    /// The subagents' shape generation the rows were built at.
    pub(crate) subs_gen: u64,
    /// The session the rows are of.
    pub(crate) rows_session: Option<String>,
    /// The last call still unfinished: the one a prompt waits on.
    pub(crate) waiting_on: Option<u64>,
    /// What a rise of the list's top edge could not scroll (the list was
    /// nearer its start), and where the list was then ([`AppView::keep_rows`]).
    pub(crate) scroll_carry: Option<(f32, ListOffset)>,
    /// The list's view grew while it was scrolled back: after this
    /// frame's layout it must not have started following the tail.
    pub(crate) hold_back: Rc<Cell<bool>>,
    /// The markdown rows' text views, and whose parse is pending ([`turns::TextRows`]).
    pub(crate) texts: Rc<RefCell<turns::Views>>,
    /// The transcript's own scrolling (fluid.md §3.2).
    pub(crate) scroller: crate::scroll::Scroller,
    /// The list's view (top, bottom) as its last layout left it.
    pub(crate) list_box: Cell<(f32, f32)>,
    /// The list on screen, numbered from the window's count
    /// ([`Panes::next_gen`]): a new one with every [`AppView::list_out`]
    /// (a tab, a worker's view). The held rooms are keyed by it.
    pub(crate) list_gen: u64,
    /// The room a folded card gave up, kept at the list's end as it folded
    /// (the answered row holds still) and after it landed, until rows that
    /// grow in under it use it up, the list leaves its end, or the turn
    /// ends (then on `sheet`, with its run's fold) ([`AppView::tail_slack`]).
    pub(crate) fold_slack: Rc<Cell<Option<Slack>>>,
    /// The runs folding while watched: each one's rows' room under its
    /// summary, each on its own spring ([`FoldGap`], [`AppView::fold_gap_now`];
    /// a second fold never cuts the first: Shady #23 r3 M2).
    pub(crate) fold_gap: RefCell<Vec<FoldGap>>,
    /// A body the user opened or closed (S1) ([`BodyHold`]).
    pub(crate) body_hold: Cell<Option<BodyHold>>,
    /// The user's scroll while an answered card folds: the room it leaves
    /// is told of it as it lands ([`AppView::tail_slack`]).
    pub(crate) touch_kept: Cell<bool>,
    /// The text metrics its list was measured with, from the window's
    /// count ([`Panes::next_gen`]): new when the text size changes every
    /// pane's row heights, or this pane's layout form changes; a cached
    /// tab's list is re-measured when it comes back unless it matches.
    pub(crate) metrics_gen: u64,

    // The pane's view around its list (architecture.md §1.1-§1.3: "P").
    /// The centre pane's column at the last frame.
    pub(crate) layout: Layout,
    /// The naming row was open at the last layout ([`AppView::keep_rows`]
    /// counts it as that layout had it).
    pub(crate) layout_naming: bool,
    /// The metrics the last frame drew with (a fold's header height:
    /// [`AppView::start_gap`]).
    pub(crate) metrics_last: Cell<Option<Metrics>>,
    /// The transcript's body line this frame (`theme::metrics`), for the rows.
    pub(crate) body_line: f32,
    /// The column on screen (the session its rows are, the worker view)
    /// and its tab's id, and when it last changed; the tab's place then and
    /// whether it was a worker's view; the layer it arrives as: since, and
    /// from where (fluid.md §3.1, §3.6).
    pub(crate) column_key: (Option<String>, u64, Option<u64>),
    pub(crate) column_at: Option<Instant>,
    pub(crate) column_ix: Option<usize>,
    pub(crate) column_worker: bool,
    pub(crate) layer: Option<(Instant, f32, f32)>,
    /// The dock's (stub, notice, chips) and the composer's field's heights
    /// as last laid out: their boxes spring to them (polish.md P13, P14).
    pub(crate) dock_h: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) composer_h: std::rc::Rc<std::cell::Cell<f32>>,
    /// The session and column they were measured in: another's is not a
    /// change (it is drawn at its height at once).
    pub(crate) dock_scope: u64,
    /// The top of what is drawn over the composer (the stub, a notice, the
    /// worker chips) as last laid out; `INFINITY` while none is.
    pub(crate) over_composer: std::rc::Rc<std::cell::Cell<f32>>,
    /// The composer's frame's top as last laid out (the palette ends
    /// above it).
    pub(crate) composer_top: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) composer: Entity<TextareaState>,
    /// The rows the composer's text grows to ([`crate::fit::composer_rows`]).
    pub(crate) composer_rows: usize,
    /// The worker whose name and state the composer's placeholder has
    /// (`None`: the session's placeholder).
    pub(crate) placeholder_of: Option<(String, String, bool)>,
    /// The composer's placeholder was set for a cramped window (its key
    /// hints dropped: the field is short).
    pub(crate) placeholder_cramped: bool,
    /// Refused input to put back in the composer (needs a `Window`: done
    /// at the next frame).
    pub(crate) restore: Option<String>,
    /// A slash command the composer sent (its name, its text): the text
    /// stays in the composer until the daemon has run it.
    pub(crate) cmd_draft: Option<(String, String)>,
    /// That text, run: out of the composer if it still holds it.
    pub(crate) cmd_clear: Option<String>,
    /// The note last set by a command that went out or was answered: shown
    /// quiet (`dim`), not as a warning, while it is the note on screen.
    pub(crate) quiet_note: Option<SharedString>,
    /// Said in the composer's hint until the next send or switch (a draft
    /// came back with a send the daemon did not take).
    pub(crate) draft_note: Option<&'static str>,
    /// Said in the composer's hint until the next key that moves or sends:
    /// why a worker key went nowhere, or why a steer was not sent.
    pub(crate) watch_note: Option<SharedString>,
    pub(crate) composer_of: Option<(u64, Option<String>)>,
    /// HALT armed by a first Esc in the composer ([`ARM`]): a second
    /// halts. Its label is `err` while armed; the timer ends the arm, as
    /// does anything else shown (another tab, session or view).
    pub(crate) halt_armed: Option<Instant>,
    pub(crate) halt_tick: Option<Task<()>>,
    /// Where HALT was armed ([`AppView::halt_place`]).
    pub(crate) halt_where: Option<(Option<u64>, Option<String>, bool)>,
    /// The Esc on its way had an IME composition to end: the composition's,
    /// not HALT's. Read before the kit's `Escape` ends it (the root's
    /// action capture runs first).
    pub(crate) esc_composing: bool,
    /// A secret prompt's answer.
    pub(crate) secret_input: Entity<InputState>,
    /// The tab (and its session) the model and the list hold now.
    pub(crate) shown_tab: Option<(u64, Option<String>)>,
    /// A switch's attach waiting for the gate, and its timer.
    pub(crate) attach_wait: Option<(String, Option<ResumeRequest>, bool)>,
    pub(crate) attach_due: Option<Task<()>>,
    pub(crate) gate: crate::tabs::AttachGate,
    /// The tab strip's first shown tab when they overflow, and the tab it
    /// last kept in view.
    pub(crate) tab_first: usize,
    pub(crate) tab_follow: Option<u64>,
    /// How many tabs the strip had room for at the last frame.
    pub(crate) tab_room: usize,
    pub(crate) tab_labels: crate::tabstrip::Labels,
    /// What the strip's motion remembers ([`crate::tabstrip::TabFx`]).
    pub(crate) tab_fx: crate::tabstrip::TabFx,
    pub(crate) strip_at: std::rc::Rc<std::cell::Cell<Point<Pixels>>>,
    /// The session's list while a worker's view is shown, and the worker
    /// whose rows the list holds (`None`: the session's).
    pub(crate) main_column: Option<Column>,
    /// The worker's view the list holds and its generation
    /// ([`crate::watch::Watch::focus_key`]).
    pub(crate) column_of: Option<(String, u64)>,
    /// The changes to the transcript the list shows that it has not
    /// synced yet (split/architecture.md §3.2): the store's splice is taken
    /// once a frame and handed to every list showing that transcript
    /// ([`AppView::fan_out`]); a parked list keeps its own in its
    /// [`Column`].
    pub(crate) pending: Option<synaps_client::transcript::Splice>,
    /// Worker tabs' lists (rows, folds, heights, scroll) while another
    /// thread of their session is on screen, by view and generation: back
    /// as they were left, while the tab is open.
    pub(crate) worker_columns: Vec<((String, u64), Column)>,
    /// The next sync builds every row again (a worker's view came on).
    pub(crate) full_rebuild: bool,
    /// The authorization card's prompt (see [`PromptCard`]).
    pub(crate) card: Option<PromptCard>,
    /// Its keys were live at the last frame ([`AppView::card_keys_live`]):
    /// when they come back, they wait again ([`Arm`]).
    pub(crate) keys_live: bool,
    /// Enter and Esc, down and up ([`CardChord`]), and whether the last
    /// of them to go down was a new press.
    pub(crate) chord: CardChord,
    pub(crate) chord_press: bool,
    /// A secret card went away (answered elsewhere, re-delivered, another
    /// tab) at this time, or a key reached the window since, within
    /// [`ARM`] of the last: what is still being typed was meant for it,
    /// so `/` does not open find ([`AppView::text_keys_blocked`]).
    pub(crate) secret_left: Option<Instant>,
    /// This frame's [`AppView::card_keys_live`] (the card's row reads it).
    pub(crate) card_keys: Cell<Option<bool>>,
    /// The card was on screen last frame (not behind a worker's view,
    /// Settings or Usage): one coming back waits again ([`Arm`]).
    pub(crate) card_was_up: bool,
    /// The card's row was on screen, buttons whole, at the frame's start.
    pub(crate) card_seen: Cell<bool>,
    /// What the card's last frame showed ([`CardLook`]).
    pub(crate) card_look: std::cell::RefCell<CardLook>,
    /// The answered card collapsing in its row ([`CardExit`]).
    pub(crate) card_exit: Option<CardExit>,
    /// The prompt on the card came while watched, at this instant: its frame
    /// traces, its glyph breathes (motion.md §6.3).
    pub(crate) card_at: Option<Instant>,
    /// Ctrl+J from a worker's view or a pane: reveal this card (its
    /// [`Model::card_key`]) once its row is built; dropped when that card
    /// goes (Shady M5: a later card never takes the keys from the composer).
    pub(crate) jump_card: Option<(u64, u64)>,
    /// Ctrl+J went to this card (its [`Model::card_key`]): its NEEDS YOU
    /// stub leaves at once, not once the card is on screen, so the jump
    /// aims at the view as it will be and the card moves one way. Cleared
    /// once the card is on screen (scrolled away again, the stub returns).
    pub(crate) stub_away: Option<(u64, u64)>,
    /// The card's Allow all box: Tab from the card's keys focuses it (a
    /// click too); Space toggles it then only.
    pub(crate) allow_all_focus: FocusHandle,
    /// A worker tab opened this frame (its id), the tracer to it, and where
    /// the tab strip was painted last.
    pub(crate) tracer_open: Option<u64>,
    pub(crate) tracer: Option<crate::motion::Tracer>,
    /// The tray's armed worker cancel running out: one redraw then
    /// (`AppView::arm_cancel_tick`; Shady #30 L4). The cancels themselves
    /// are the session's ([`crate::live::Live::cancels`]).
    pub(crate) cancel_tick: Option<Task<()>>,
    /// Its composer's subscription (Enter sends, typing clears a note):
    /// it lives and goes with the pane (Shady #30 L4).
    pub(crate) _subs: Vec<Subscription>,
    /// What the window holds of the pane acted on, while another one is
    /// ([`Parked`]).
    pub(crate) parked: Parked,
    /// Its rectangle in the centre, as last laid out (split panes).
    pub(crate) rect: Cell<crate::split::Rect>,
}

/// A pane's state the window holds for the pane acted on (in `Model` and
/// `AppView`, where steps 1 and 2 left it and the protected tests read
/// it): the others keep theirs here, swapped in with the pane
/// ([`crate::app::AppView::enter_pane`]), so every read and write of it
/// is that pane's.
#[derive(Default)]
pub(crate) struct Parked {
    /// Its tabs (`Model::tabs`).
    pub(crate) tabs: crate::tabs::Tabs,
    /// Its transcript bar's drag and fade (`AppView::bars`).
    pub(crate) bar: crate::scrollbar::BarState,
    /// Its drafts (`AppView::drafts`): two composers on one session never
    /// share one (split/architecture.md §6.5).
    pub(crate) drafts: crate::tabs::Drafts,
    /// Its worker chips: its session's, never another pane's (a chip's
    /// cancel goes to the store it was drawn from).
    pub(crate) tray: crate::tray::TrayCache,
    pub(crate) tray_rows: usize,
    pub(crate) chips: crate::tray::Chips,
    /// Its card's receipt and its HALT's (#31 L2), and its runs' timings
    /// by item id (every transcript numbers from 1).
    pub(crate) sent: Option<crate::authorize::Sent>,
    pub(crate) halted: Option<(String, Instant)>,
    pub(crate) took: RefCell<std::collections::HashMap<u64, std::time::Duration>>,
    pub(crate) ran: RefCell<HashSet<u64>>,
    /// Its worker view (#31: per-pane worker focus): the store's `Watch`
    /// holds the pane acted on's; two panes on one session each keep
    /// their own (a worker in one, the thread in the other).
    pub(crate) view: crate::watch::ViewFocus,
}

impl Pane {
    /// A pane with nothing on it yet: an empty list following its tail.
    pub(crate) fn fresh(
        id: PaneId,
        live: crate::live::LiveId,
        composer: Entity<TextareaState>,
        secret_input: Entity<InputState>,
        allow_all_focus: FocusHandle,
        subs: Vec<Subscription>,
    ) -> Pane {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let texts = Rc::default();
        turns::readiness(&list, &texts);
        Pane {
            id,
            live,
            rows: Vec::new(),
            disp: Vec::new(),
            summaries: Summaries::default(),
            list,
            toggled: Toggled::default(),
            open_runs: HashSet::new(),
            unfold_hold: Vec::new(),
            synced_busy: false,
            files_gen: 0,
            subs_gen: 0,
            rows_session: None,
            waiting_on: None,
            scroll_carry: None,
            hold_back: Rc::new(Cell::new(false)),
            texts,
            scroller: crate::scroll::Scroller::default(),
            list_box: Cell::new((0., 0.)),
            list_gen: 0,
            fold_slack: Rc::new(Cell::new(None)),
            fold_gap: RefCell::new(Vec::new()),
            body_hold: Cell::new(None),
            touch_kept: Cell::new(false),
            metrics_gen: 0,
            layout: Layout::of(800., 800., &Metrics::at(crate::theme::DEFAULT_TEXT), Default::default()),
            layout_naming: false,
            metrics_last: Cell::new(None),
            body_line: 0.,
            column_key: (None, 0, None),
            column_at: None,
            column_ix: None,
            column_worker: false,
            layer: None,
            dock_h: std::rc::Rc::new(std::cell::Cell::new(f32::NAN)),
            composer_h: std::rc::Rc::new(std::cell::Cell::new(f32::NAN)),
            dock_scope: 0,
            over_composer: std::rc::Rc::new(std::cell::Cell::new(f32::INFINITY)),
            composer_top: Default::default(),
            composer,
            composer_rows: crate::fit::COMPOSER_ROWS,
            placeholder_of: None,
            placeholder_cramped: false,
            restore: None,
            cmd_draft: None,
            cmd_clear: None,
            quiet_note: None,
            draft_note: None,
            watch_note: None,
            composer_of: None,
            halt_armed: None,
            halt_tick: None,
            halt_where: None,
            esc_composing: false,
            secret_input,
            shown_tab: None,
            attach_wait: None,
            attach_due: None,
            gate: Default::default(),
            tab_first: 0,
            tab_follow: None,
            tab_room: 0,
            tab_labels: Default::default(),
            tab_fx: Default::default(),
            strip_at: Default::default(),
            main_column: None,
            column_of: None,
            pending: None,
            worker_columns: Vec::new(),
            full_rebuild: false,
            card: None,
            keys_live: false,
            chord: CardChord::default(),
            chord_press: false,
            secret_left: None,
            card_keys: Cell::new(None),
            card_was_up: false,
            card_seen: Cell::new(false),
            card_look: Default::default(),
            card_exit: None,
            card_at: None,
            jump_card: None,
            stub_away: None,
            allow_all_focus,
            tracer_open: None,
            tracer: None,
            cancel_tick: None,
            _subs: subs,
            parked: Default::default(),
            rect: Cell::new(crate::split::Rect { x: 0., y: 0., w: 0., h: 0. }),
        }
    }
}

/// The window's panes (split panes step 3: up to [`PANES_MAX`], on
/// [`Panes::tree`]).
///
/// The pane acted on is `cur`, a plain field, so `self.panes.cur.list`
/// borrows disjointly from `self.model` and from the pane's other fields.
/// Between events it is the focused pane. While one pane is drawn, or an
/// event, timer or effect addressed to one runs, that pane is swapped
/// into `cur` for its length ([`crate::app::AppView::in_pane`]) and back:
/// code that acts on "the pane" acts on that one, never on another.
pub(crate) struct Panes {
    /// The pane acted on (the focused one, between events).
    pub(crate) cur: Pane,
    /// The others, in no order.
    pub(crate) rest: Vec<Pane>,
    /// The focused pane: where keys, typing and the right panel go.
    pub(crate) focused: PaneId,
    /// The layout ([`crate::split::Node`]).
    pub(crate) tree: crate::split::Node,
    /// The last generation handed out ([`Panes::next_gen`]).
    gens: u64,
}

impl Panes {
    pub(crate) fn one(pane: Pane) -> Self {
        let id = pane.id;
        Panes { cur: pane, rest: Vec::new(), focused: id, tree: crate::split::Node::Leaf(id), gens: 0 }
    }

    /// More than one pane on screen.
    pub(crate) fn split(&self) -> bool {
        !self.rest.is_empty()
    }

    /// How many panes are open.
    pub(crate) fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// A free id for a new pane: the lowest no open pane has (Shady #30 L1:
    /// ids index the motion columns; a closed pane's column is reset when
    /// its id is handed out again).
    pub(crate) fn free_id(&self) -> Option<PaneId> {
        (0..PANES_MAX as u8).map(PaneId::of).find(|id| self.get(*id).is_none())
    }

    /// Every open pane's id, in tree order.
    pub(crate) fn ids(&self) -> Vec<PaneId> {
        self.tree.leaves()
    }

    pub(crate) fn get(&self, id: PaneId) -> Option<&Pane> {
        if self.cur.id == id { Some(&self.cur) } else { self.rest.iter().find(|p| p.id == id) }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Pane> {
        std::iter::once(&self.cur).chain(self.rest.iter())
    }

    /// The panes showing live store `live`.
    pub(crate) fn showing(&self, live: crate::live::LiveId) -> usize {
        self.iter().filter(|p| p.live == live).count()
    }

    /// A window-wide generation (split/architecture.md §6.1 rule 2, §1.1;
    /// Shady #30 M3): a list's number ([`Pane::list_gen`]: a held room
    /// carried into another list can never match there) and the text
    /// metrics' ([`Pane::metrics_gen`]: a tab cached by one pane is
    /// re-measured in another unless they were measured alike). Never
    /// handed out twice.
    pub(crate) fn next_gen(&mut self) -> u64 {
        self.gens += 1;
        self.gens
    }

    /// Every open pane.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Pane> {
        std::iter::once(&mut self.cur).chain(self.rest.iter_mut())
    }

    /// Pane `id`, if it is open (what an effect addressed to a pane finds).
    pub(crate) fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        if self.cur.id == id { Some(&mut self.cur) } else { self.rest.iter_mut().find(|p| p.id == id) }
    }
}

/// Split panes, step 3: the panes as the view acts on them.
impl crate::app::AppView {
    /// Swap pane `id` into [`Panes::cur`], with its tabs (`Model::tabs`),
    /// its drafts and its transcript bar, and show its store
    /// ([`crate::live::Lives::show`]). `false`: no such pane.
    pub(crate) fn enter_pane(&mut self, id: PaneId) -> bool {
        if self.panes.cur.id == id {
            return true;
        }
        let Some(i) = self.panes.rest.iter().position(|p| p.id == id) else { return false };
        let mut q = std::mem::take(&mut self.panes.rest[i].parked);
        // The worker views: the pane leaving takes its own out of its
        // store, then the one coming puts its own in.
        let leaving = self.model.lives[self.panes.cur.live].watch.park();
        let coming = std::mem::replace(&mut q.view, leaving);
        let live = self.panes.rest[i].live;
        self.model.lives[live].watch.unpark(coming);
        std::mem::swap(&mut self.model.tabs, &mut q.tabs);
        std::mem::swap(&mut self.bars.transcript, &mut q.bar);
        std::mem::swap(&mut self.drafts, &mut q.drafts);
        std::mem::swap(&mut self.tray, &mut q.tray);
        std::mem::swap(&mut self.tray_rows, &mut q.tray_rows);
        std::mem::swap(&mut self.chips, &mut q.chips);
        std::mem::swap(&mut self.sent, &mut q.sent);
        std::mem::swap(&mut self.halted, &mut q.halted);
        std::mem::swap(&mut self.took, &mut q.took);
        std::mem::swap(&mut self.ran, &mut q.ran);
        // `q` holds the pane acted on so far: parked with it.
        std::mem::swap(&mut self.panes.cur, &mut self.panes.rest[i]);
        self.panes.rest[i].parked = q;
        self.model.lives.show(self.panes.cur.live);
        true
    }

    /// `f` acting on pane `id` (drawn, or an event, timer or effect
    /// addressed to it), then back to the pane that was acted on, or to
    /// the focused one if `f` moved the focus. `None`: no such pane.
    pub(crate) fn in_pane<R>(&mut self, id: PaneId, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let (back, focused) = (self.panes.cur.id, self.panes.focused);
        if !self.enter_pane(id) {
            return None;
        }
        let r = f(self);
        let to = if self.panes.focused != focused { self.panes.focused } else { back };
        if !self.enter_pane(to) {
            let f = self.panes.focused;
            self.enter_pane(f);
        }
        Some(r)
    }

    /// Split panes are on (`panes.split` in the settings file, or
    /// `SYNAPS_DESK_SPLIT=1`): off, Ctrl+\ does nothing and the window is
    /// main's.
    pub(crate) fn split_on(&self) -> bool {
        self.app_settings.split
    }

    /// Focus pane `id` (a mouse-down in it, keyboard focus entering it,
    /// Alt+N, F6, a split): keys, typing and the right panel go to it. Its
    /// card's keys wait [`crate::app::ARM`] again, so a key meant for one
    /// pane never answers another's card (split.md §7).
    pub(crate) fn focus_pane(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.focused == id && self.panes.cur.id == id {
            return;
        }
        if !self.focus_quiet(id) {
            return;
        }
        self.restart_card_keys(cx);
        // Typing follows (one composer is lit): out of another pane's
        // composer into this one's; the root keeps the keys it has.
        let elsewhere = self.panes.rest.iter().any(|p| {
            p.composer.read(cx).focus_handle(cx).is_focused(window) || p.secret_input.read(cx).focus_handle(cx).is_focused(window)
        });
        if elsewhere {
            self.panes.cur.composer.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// [`AppView::focus_pane`] without the keyboard (a rail click, an
    /// event): the pane is the focused one, its card's keys wait again.
    pub(crate) fn focus_quiet(&mut self, id: PaneId) -> bool {
        if !self.enter_pane(id) {
            return false;
        }
        if std::mem::replace(&mut self.panes.focused, id) != id {
            self.close_menu();
            if let Some(c) = self.panes.cur.card.as_mut() {
                c.arm.restart(std::time::Instant::now());
            }
            self.model.rail_gen += 1;
            self.model.view_dirty = true;
        }
        true
    }

    /// A rail click, a find or an Open of session `s` (split.md §2): on
    /// screen in another pane, that pane is focused; a background tab of
    /// another pane, it is shown there and focused. Whether it was, so
    /// nothing opens (a click never makes a second copy).
    pub(crate) fn focus_session(&mut self, s: &str) -> bool {
        if !self.panes.split() {
            return false;
        }
        if let Some(id) = self.pane_showing(s) {
            return id != self.panes.cur.id && self.focus_quiet(id);
        }
        let other = self
            .panes
            .rest
            .iter()
            .find(|p| p.parked.tabs.iter().any(|t| t.session.as_deref() == Some(s) && t.kind == crate::tabs::Kind::Session));
        let Some(id) = other.map(|p| p.id) else { return false };
        self.focus_quiet(id);
        let ix = self.model.tabs.iter().position(|t| t.session.as_deref() == Some(s) && t.kind == crate::tabs::Kind::Session);
        if let Some(ix) = ix {
            self.focus_tab(ix);
        }
        true
    }

    /// The panes in reading order (split.md §1).
    pub(crate) fn reading_order(&self) -> Vec<PaneId> {
        let (w, h) = self.centre.get();
        self.panes.tree.reading_order(crate::split::Rect { x: 0., y: 0., w, h })
    }

    /// Alt+N: pane `n` (0-based) in reading order; F6 / Shift+F6: the
    /// next or previous one, round.
    pub(crate) fn focus_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&id) = self.reading_order().get(n) {
            self.focus_pane(id, window, cx);
            self.panes.cur.composer.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    pub(crate) fn focus_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.reading_order();
        let Some(i) = order.iter().position(|p| *p == self.panes.focused) else { return };
        let n = order.len();
        let j = if back { (i + n - 1) % n } else { (i + 1) % n };
        if j != i {
            self.focus_nth(j, window, cx);
        }
    }

    /// Ctrl+\ (`side` `None`: right, else down if too narrow) and
    /// Ctrl+Shift+\ (`Some(Down)`), split.md §2: a new pane beside the
    /// focused one with a new tab of its current session tab (the same
    /// store: one attachment), focused, an empty composer, following the
    /// tail. Refused with the reason on the composer's status line.
    pub(crate) fn split_pane(&mut self, side: Option<crate::split::Side>, window: &mut Window, cx: &mut Context<Self>) {
        use crate::split::Refused;
        if !self.split_on() {
            return;
        }
        let at = self.panes.focused;
        let new = self.panes.free_id();
        let area = self.centre.get();
        let next = match new {
            None => Err(Refused::Most),
            Some(new) => match side {
                None => self.panes.tree.split_right(at, new, area).map(|(t, _)| t),
                Some(s) => self.panes.tree.split(at, s, new, area),
            },
        };
        let (tree, new) = match (next, new) {
            (Ok(t), Some(n)) => (t, n),
            (Err(r), _) => {
                self.split_refused(r.note(), cx);
                return;
            }
            (Ok(_), None) => {
                self.split_refused(Refused::Most.note(), cx);
                return;
            }
        };
        // The focused pane's current session tab (a view's too: the
        // session tab behind it), on its store; none: an empty pane.
        let session = self.model.tabs.current().and_then(|t| Some((t.session.clone()?, t.label.clone())));
        let live = match &session {
            Some((s, _)) if self.model.lives[self.panes.cur.live].is_active(s) || self.model.lives[self.panes.cur.live].is_wanted(s) => {
                self.panes.cur.live
            }
            _ => self.fresh_live(),
        };
        let (composer, secret, allow_all, subs) = Self::pane_inputs(new, window, cx);
        let mut pane = Pane::fresh(new, live, composer, secret, allow_all, subs);
        // A reused id's motion was its last pane's (Shady #30 L1).
        self.fx.col(new).reset();
        pane.parked.tabs = crate::tabs::Tabs::numbered_from(self.panes.next_gen() << 32);
        pane.list_gen = self.panes.next_gen();
        pane.metrics_gen = self.panes.cur.metrics_gen;
        pane.full_rebuild = true;
        self.panes.tree = tree;
        self.panes.rest.push(pane);
        self.focus_pane(new, window, cx);
        if let Some((s, label)) = session {
            self.model.tabs.open(&s, &label, crate::tabs::Place::New);
            self.model.rail_gen += 1;
            self.reconcile(None);
        }
        self.panes.cur.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// A split refused: the reason on the focused composer's status line
    /// for 2.4 s (split.md §2), no toast.
    fn split_refused(&mut self, why: &'static str, cx: &mut Context<Self>) {
        let id = self.panes.cur.id;
        self.panes.cur.watch_note = Some(why.into());
        self.panes.cur.halt_tick = None;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(2400)).await;
            let _ = this.update(cx, |v, cx| {
                v.in_pane(id, |v| {
                    if v.panes.cur.watch_note.as_deref() == Some(why) {
                        v.panes.cur.watch_note = None;
                    }
                });
                cx.notify();
            });
        });
        self.split_note = Some(task);
        cx.notify();
    }

    /// A store no pane shows (reset), else a new one.
    pub(crate) fn fresh_live(&mut self) -> crate::live::LiveId {
        match self.model.lives.ids().into_iter().find(|l| self.panes.showing(*l) == 0) {
            Some(l) => {
                self.model.lives[l] = crate::live::Live::default();
                l
            }
            None => self.model.lives.add(),
        }
    }

    /// Store `live` is shown by no pane now: what it holds open is
    /// detached, and it goes.
    pub(crate) fn release_live(&mut self, live: crate::live::LiveId) {
        if self.panes.showing(live) > 0 {
            return;
        }
        if let (Some(c), Some(seq)) = (&self.client, self.model.lives[live].release()) {
            let _ = c.on(seq).detach();
        }
        if live == crate::live::LiveId::FIRST {
            self.model.lives[live] = crate::live::Live::default();
        } else {
            self.model.lives.remove(live);
        }
    }

    /// The pane acted on closes (its last tab closed; split.md §8): its
    /// share goes to the panes beside it, focus to its left/up neighbour,
    /// and its store goes if no other pane shows it. The last pane never
    /// closes: it is today's empty pane.
    pub(crate) fn close_pane(&mut self) {
        let id = self.panes.cur.id;
        let (Some(to), Some(tree)) = (self.panes.tree.neighbour(id), self.panes.tree.remove(id)) else { return };
        let live = self.panes.cur.live;
        self.panes.tree = tree;
        self.panes.focused = to;
        self.enter_pane(to);
        if let Some(i) = self.panes.rest.iter().position(|p| p.id == id) {
            drop(self.panes.rest.remove(i));
        }
        self.fx.col(id).reset();
        self.release_live(live);
        self.model.rail_gen += 1;
        self.model.view_dirty = true;
    }

    /// The pane showing store `live` an event of it goes to: the focused
    /// one if it does, else the first that does.
    pub(crate) fn pane_of_live(&self, live: crate::live::LiveId) -> Option<PaneId> {
        if self.panes.get(self.panes.focused).is_some_and(|p| p.live == live) {
            return Some(self.panes.focused);
        }
        self.panes.iter().find(|p| p.live == live).map(|p| p.id)
    }

    /// The pane showing session `s` on its visible tab, if any.
    pub(crate) fn pane_showing(&self, s: &str) -> Option<PaneId> {
        let shows =
            |p: &Pane, tabs: &crate::tabs::Tabs| p.shown_tab.as_ref().and_then(|t| t.1.as_deref()) == Some(s) && tabs.view().is_none();
        if shows(&self.panes.cur, &self.model.tabs) {
            return Some(self.panes.cur.id);
        }
        self.panes.rest.iter().find(|p| shows(p, &p.parked.tabs)).map(|p| p.id)
    }
}

/// The splits that make `n` panes (split.md §4): 2 side by side, 3
/// across, 4 as 2×2. Each: the pane to focus first (reading order), and
/// the side (`None`: right, or down if narrow).
pub(crate) fn shape(n: usize) -> Vec<(Option<usize>, Option<crate::split::Side>)> {
    use crate::split::Side;
    match n {
        0 | 1 => vec![],
        2 => vec![(None, None)],
        3 => vec![(None, None), (None, None)],
        _ => vec![(None, None), (None, Some(Side::Down)), (Some(0), Some(Side::Down))],
    }
}

/// `SYNAPS_DESK_PANES=<n>|film` (benches and evidence, with split panes
/// on): once the first session is attached, `n` panes on it (2: side by
/// side; 3: three across; 4: 2×2), or the film's scene, each step marked
/// on stderr (`PANES <what> <s>`): split right, the right pane opens
/// another session, split down, focus by F6, Shift+F6 and Alt+1, a draft
/// per pane, then the panes close one by one (each collapses).
pub(crate) fn pane_script(view: &Entity<crate::app::AppView>, how: String, window: &mut Window, cx: &mut App) {
    use crate::app::AppView;
    use crate::split::Side;
    let view = view.downgrade();
    window
        .spawn(cx, async move |cx| {
            let ex = cx.background_executor().clone();
            let t0 = Instant::now();
            let mark = |what: &str| eprintln!("PANES {what} {:.3}", t0.elapsed().as_secs_f64());
            let act = |cx: &mut AsyncWindowContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
                let _ = view.update_in(cx, |v, window, cx| {
                    f(v, window, cx);
                    cx.notify();
                });
            };
            let sleep = |s: f64| ex.timer(std::time::Duration::from_secs_f64(s));
            // The film's first split once its recording has begun
            // (`scripts/capture-linux.sh` starts 3.5 s after the window).
            let film = how == "film";
            sleep(if film { 5.0 } else { 1.5 }).await;
            if let Ok(n) = how.parse::<usize>() {
                for (at, side) in shape(n) {
                    act(cx, &|v, w, cx| {
                        if let Some(i) = at {
                            v.focus_nth(i, w, cx);
                        }
                        v.split_pane(side, w, cx);
                    });
                    sleep(0.3).await;
                }
                mark("panes");
                return;
            }
            mark("split");
            act(cx, &|v, w, cx| v.split_pane(None, w, cx));
            sleep(2.0).await;
            mark("open");
            act(cx, &|v, _, _| {
                // The demo's idle session (its history drawn), else another.
                let idle = v.model.sessions.iter().any(|s| s.id == crate::demo::IDLE_CPU).then(|| crate::demo::IDLE_CPU.to_string());
                let other = idle
                    .or_else(|| v.model.sessions.iter().map(|s| s.id.clone()).find(|s| v.model.tabs.current_session() != Some(s.as_str())));
                if let Some(s) = other {
                    v.open_tab(crate::app::Open::Attach { session_id: s, fallback: None, open: false }, crate::tabs::Place::Current);
                }
            });
            sleep(2.0).await;
            mark("split-down");
            act(cx, &|v, w, cx| v.split_pane(Some(Side::Down), w, cx));
            sleep(2.0).await;
            for what in ["f6", "f6", "shift-f6", "alt-1"] {
                mark(what);
                act(cx, &|v, w, cx| match what {
                    "f6" => v.focus_step(false, w, cx),
                    "shift-f6" => v.focus_step(true, w, cx),
                    _ => v.focus_nth(0, w, cx),
                });
                sleep(1.2).await;
            }
            mark("drafts");
            for (i, text) in [(1usize, "a draft in the right pane"), (0, "and one in the left")] {
                act(cx, &|v, w, cx| {
                    v.focus_nth(i, w, cx);
                    v.panes.cur.composer.update(cx, |s, cx| s.set_value(text, w, cx));
                });
                sleep(1.2).await;
            }
            for _ in 0..2 {
                mark("close");
                act(cx, &|v, w, cx| {
                    v.focus_nth(1, w, cx);
                    if let Some(ix) = v.model.tabs.active() {
                        v.close_tab(ix);
                    }
                });
                sleep(2.0).await;
            }
            mark("done");
        })
        .detach();
}

/// Tests read the pane's fields as they read the view's before
/// (`v.list`, `v.fold_slack`, `v.layer`...): the protected tests stay
/// byte-identical (split/architecture.md §6.6). Production code never
/// sees this: it names `panes.cur`.
#[cfg(test)]
impl std::ops::Deref for crate::app::AppView {
    type Target = Pane;
    fn deref(&self) -> &Pane {
        &self.panes.cur
    }
}

#[cfg(test)]
impl std::ops::DerefMut for crate::app::AppView {
    fn deref_mut(&mut self) -> &mut Pane {
        &mut self.panes.cur
    }
}

#[cfg(test)]
mod tests {
    use super::{PANES_MAX, PaneId};

    /// The first pane's keys are today's: its salt is 0 (split panes
    /// step 1 changes no spring, hover or chip key). Every other pane's
    /// differ from it and from each other's.
    #[test]
    fn the_first_panes_salt_is_zero() {
        let keys = [0x7AF1_u64 << 48, 0x7AF2 << 48, (0x7AF4 << 48) | 12, (0x7AB4 << 48) | 0xFFFF_FFFF];
        for k in keys {
            assert_eq!(PaneId::FIRST.salt(k), k);
            let salted: Vec<u64> = (0..PANES_MAX as u8).map(|i| PaneId::nth(i).salt(k)).collect();
            for i in 0..salted.len() {
                for j in i + 1..salted.len() {
                    assert_ne!(salted[i], salted[j], "panes {i} and {j} share {k:#x}");
                }
            }
        }
    }
}
