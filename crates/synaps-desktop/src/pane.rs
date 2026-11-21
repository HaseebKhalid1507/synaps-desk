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
    /// The labels' move between inline and the gutter ([`crate::turns::HangFx`]).
    pub(crate) hang_fx: crate::turns::HangFx,
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
    /// Its rectangle as the layout's flight lands (split.md §10; at rest,
    /// `rect`): its form, its fit and its foot are the landed pane's, so
    /// none of them change in the middle of a split or a close.
    pub(crate) land: Cell<crate::split::Rect>,
}

/// Why the keys are to follow the focus mark at the next frame
/// ([`crate::app::AppView::apply_keys`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeysTo {
    /// A pane key (Alt+N, F6, a split): to the focused pane, unless
    /// something in it has them already.
    Pane,
    /// A click in the pane: only out of another pane's inputs (whatever
    /// the click focused, or left with nothing, stands).
    Follow,
    /// The mark moved from outside the panes (a rail click, a find, an
    /// Open, a view opened again): also from the root or nowhere (a click
    /// outside left the composer), never from a window input.
    Quiet,
    /// The pane that had the focus closed: also when its inputs, gone,
    /// left the keys nowhere.
    Closed,
}

/// Who has the keys, as the focus mark sees it ([`KeysTo`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeysAt {
    /// The focused pane's composer, secret field, Allow all box or (a
    /// view) its surface.
    Focused,
    /// Another pane's.
    Other,
    /// The window's root (a card's keys).
    Root,
    /// A window-wide input: find, the palette, the naming row.
    Window,
    Nowhere,
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
    /// Its tray's ghost-end timer (Shady #36 L4: each pane's own).
    pub(crate) tray_due: Option<(std::time::Instant, gpui_kit::Task<()>)>,
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
            hang_fx: Default::default(),
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
            land: Cell::new(crate::split::Rect { x: 0., y: 0., w: 0., h: 0. }),
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
    /// The layout's change in flight, on `sheet` (split.md §10).
    pub(crate) flight: Option<crate::split::Flight>,
    /// A divider held down, or dragged (split.md §4).
    pub(crate) grab: Option<crate::split::Grab>,
    /// The window is too narrow (or short) for the split (split.md §5):
    /// the focused pane fills the centre, the tree kept as it is.
    pub(crate) folded: bool,
    /// The last generation handed out ([`Panes::next_gen`]).
    gens: u64,
}

impl Panes {
    pub(crate) fn one(pane: Pane) -> Self {
        let id = pane.id;
        Panes {
            cur: pane,
            rest: Vec::new(),
            focused: id,
            tree: crate::split::Node::Leaf(id),
            flight: None,
            grab: None,
            folded: false,
            gens: 0,
        }
    }

    /// More than one pane open (on screen, or folded).
    pub(crate) fn split(&self) -> bool {
        !self.rest.is_empty()
    }

    /// More than one pane on screen: split and not folded (split.md §5).
    pub(crate) fn shown_split(&self) -> bool {
        self.split() && !self.folded
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
        std::mem::swap(&mut self.tray_due, &mut q.tray_due);
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
    /// pane never answers another's card (split.md §7). The keys follow at
    /// the next frame, as `how` says ([`AppView::apply_keys`]): a click's
    /// own target always keeps them.
    pub(crate) fn focus_pane(&mut self, id: PaneId, how: KeysTo, cx: &mut Context<Self>) {
        if self.panes.focused == id && self.panes.cur.id == id {
            return;
        }
        if !self.mark_pane(id) {
            return;
        }
        self.ask_keys(how);
        cx.notify();
    }

    /// The keys go to the focused pane at the next frame, as `how` says
    /// ([`AppView::apply_keys`]); a pane key's ask outranks a follow's.
    pub(crate) fn ask_keys(&mut self, how: KeysTo) {
        self.keys_to = Some(match (self.keys_to, how) {
            (Some(KeysTo::Pane), _) | (_, KeysTo::Pane) => KeysTo::Pane,
            (Some(KeysTo::Closed), _) | (_, KeysTo::Closed) => KeysTo::Closed,
            (Some(KeysTo::Quiet), _) | (_, KeysTo::Quiet) => KeysTo::Quiet,
            _ => KeysTo::Follow,
        });
    }

    /// The focused pane's composer takes the keys at the next frame
    /// ([`AppView::apply_keys`], as a pane key asks), never now: a
    /// composer focused and left before a frame is drawn never hears its
    /// blur (GPUI sends focus events from what each frame drew), so its
    /// caret would blink, a frame every 500 ms, for the window's life.
    pub(crate) fn focus_composer(&mut self, cx: &mut Context<Self>) {
        self.ask_keys(KeysTo::Pane);
        cx.notify();
    }

    /// The pane whose input `h` is (its composer, secret field or Allow
    /// all box), if one's.
    pub(crate) fn input_pane(&self, h: &FocusHandle, cx: &App) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|p| {
                p.composer.read(cx).focus_handle(cx) == *h || p.secret_input.read(cx).focus_handle(cx) == *h || p.allow_all_focus == *h
            })
            .map(|p| p.id)
    }

    /// Who has the keys now, as the focus mark sees it.
    fn keys_at(&self, window: &Window, cx: &App) -> KeysAt {
        let focused = self.panes.focused;
        // A view's fields (Settings' find, a value being edited) are the
        // pane's that shows that view; the theme picker's colour is the
        // window's (an overlay: Shady #36 r2 n10).
        let view_field = [&self.settings_find, &self.settings_input].iter().any(|f| f.read(cx).focus_handle(cx).is_focused(window));
        for p in self.panes.iter() {
            let tabs = if p.id == self.panes.cur.id { &self.model.tabs } else { &p.parked.tabs };
            let has = p.composer.read(cx).focus_handle(cx).is_focused(window)
                || p.secret_input.read(cx).focus_handle(cx).is_focused(window)
                || p.allow_all_focus.is_focused(window)
                || (view_field && tabs.view() == Some(crate::tabs::Kind::Settings));
            if has {
                return if p.id == focused { KeysAt::Focused } else { KeysAt::Other };
            }
        }
        let shows_view = self.panes.get(focused).is_some_and(|p| {
            let tabs = if p.id == self.panes.cur.id { &self.model.tabs } else { &p.parked.tabs };
            tabs.view().is_some()
        });
        match window.focused(cx) {
            // The root under a view is its surface (its keys' fallbacks).
            Some(f) if f == self.focus => {
                if shows_view {
                    KeysAt::Focused
                } else {
                    KeysAt::Root
                }
            }
            Some(f)
                if [&self.find_input, &self.palette_input, &self.name_input, &self.hex_input]
                    .iter()
                    .any(|i| i.read(cx).focus_handle(cx) == f) =>
            {
                KeysAt::Window
            }
            Some(_) if view_field => KeysAt::Window,
            // Nothing, or an input gone with its pane.
            _ => KeysAt::Nowhere,
        }
    }

    /// At the top of a frame: the keys follow the focus mark, as
    /// [`AppView::ask_keys`] asked (one source of truth, Shady #36 M1),
    /// to the focused pane's composer, or the root if it shows a view.
    /// Whatever the focused pane has focused itself keeps them (a click's
    /// own target: its secret field, its Allow all box; Shady #36 H2), as
    /// does anything outside the panes unless a pane key asked.
    pub(crate) fn apply_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(how) = self.keys_to.take() else { return };
        let at = self.keys_at(window, cx);
        let go = match (how, at) {
            (_, KeysAt::Focused) => false,
            (_, KeysAt::Other) => true,
            (KeysTo::Pane, _) => true,
            (KeysTo::Quiet, KeysAt::Root | KeysAt::Nowhere) => true,
            (KeysTo::Closed, KeysAt::Nowhere) => true,
            _ => false,
        };
        if !go {
            return;
        }
        let focused = self.panes.focused;
        match self.in_pane(focused, |v| v.model.tabs.view().is_none().then(|| v.panes.cur.composer.clone())).flatten() {
            Some(c) => c.update(cx, |s, cx| s.focus(window, cx)),
            None => window.focus(&self.focus, cx),
        }
    }

    /// [`AppView::focus_pane`] from outside the panes (a rail click, a
    /// find, an Open, a view opened again): the pane is the focused one,
    /// its card's keys wait again, and the keys go to it unless a window
    /// input (find, the palette) has them ([`KeysTo::Quiet`]).
    pub(crate) fn focus_quiet(&mut self, id: PaneId) -> bool {
        let moved = self.panes.focused != id;
        if !self.mark_pane(id) {
            return false;
        }
        if moved {
            self.ask_keys(KeysTo::Quiet);
        }
        true
    }

    /// The focus mark on pane `id` (entered): its card's keys wait again.
    fn mark_pane(&mut self, id: PaneId) -> bool {
        if !self.enter_pane(id) {
            return false;
        }
        if std::mem::replace(&mut self.panes.focused, id) != id {
            self.close_menu();
            // Its card's keys wait again, with their repaint (Shady #36 n7).
            self.keys_rearm = true;
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
    pub(crate) fn focus_nth(&mut self, n: usize, cx: &mut Context<Self>) {
        // One pane (or split panes off): not a key (Shady #36 L1).
        if !self.split_on() || !self.panes.split() {
            return;
        }
        if let Some(&id) = self.reading_order().get(n) {
            self.focus_pane(id, KeysTo::Pane, cx);
            self.focus_composer(cx);
        }
    }

    pub(crate) fn focus_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if !self.split_on() {
            return;
        }
        let order = self.reading_order();
        let Some(i) = order.iter().position(|p| *p == self.panes.focused) else { return };
        let n = order.len();
        let j = if back { (i + n - 1) % n } else { (i + 1) % n };
        if j != i {
            self.focus_nth(j, cx);
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
        // Its share grows from the split line on `sheet` (split.md §10).
        self.panes.flight = Some(crate::split::Flight::new(tree.grown_from(new), tree.clone(), Some(new), None));
        self.panes.tree = tree;
        self.panes.rest.push(pane);
        self.focus_pane(new, KeysTo::Pane, cx);
        if let Some((s, label)) = session {
            self.model.tabs.open(&s, &label, crate::tabs::Place::New);
            self.model.rail_gen += 1;
            self.reconcile(None);
        }
        self.focus_composer(cx);
    }

    /// Why a split of the focused pane (`side` as [`Self::split_pane`]'s)
    /// can't happen now, if it can't (the palette greys it).
    pub(crate) fn split_refusal(&self, side: Option<crate::split::Side>) -> Option<&'static str> {
        use crate::split::Refused;
        let Some(new) = self.panes.free_id() else { return Some(Refused::Most.note()) };
        let (at, area) = (self.panes.focused, self.centre.get());
        let r = match side {
            None => self.panes.tree.split_right(at, new, area).map(|_| ()),
            Some(s) => self.panes.tree.split(at, s, new, area).map(|_| ()),
        };
        match r {
            Ok(()) => None,
            Err(Refused::Most) => Some(Refused::Most.note()),
            Err(Refused::Room) => Some("no room at this window size"),
        }
    }

    /// Alt+click on a rail row (split.md §2, A6): Open to the side. In the
    /// pane to the right of the focused one (a new tab there); none, a new
    /// pane on the right; no room for one, the next pane in reading order;
    /// one pane and no room, the focused pane. It never fails and never
    /// asks. With split panes off, a plain click's.
    pub(crate) fn open_to_side(&mut self, action: &crate::named::RowAction, window: &mut Window, cx: &mut Context<Self>) {
        use crate::tabs::Place;
        let run = |v: &mut Self, place: Place| {
            for fx in v.model.open_row_in(action, place) {
                v.effect(fx);
            }
        };
        if !self.split_on() {
            return run(self, Place::Current);
        }
        let me = self.panes.focused;
        let (w, h) = self.centre.get();
        let rects = self.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w, h });
        let right = rects.iter().find(|(id, _)| *id == me).and_then(|(_, f)| {
            rects
                .iter()
                .filter(|(id, r)| *id != me && r.x >= f.x + f.w && r.y < f.y + f.h && r.y + r.h > f.y)
                .min_by(|a, b| (a.1.x, a.1.y).partial_cmp(&(b.1.x, b.1.y)).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(id, _)| *id)
        });
        // No right neighbour: a new pane there if it fits, else the next.
        let target = right.or_else(|| {
            self.split_refusal(Some(crate::split::Side::Right))?;
            let order = self.reading_order();
            let i = order.iter().position(|id| *id == me)?;
            order.get((i + 1) % order.len()).copied().filter(|id| *id != me)
        });
        match target {
            Some(id) => {
                self.focus_pane(id, KeysTo::Pane, cx);
                run(self, Place::New);
            }
            None if self.split_refusal(Some(crate::split::Side::Right)).is_none() => {
                // Its new tab of this pane's session becomes the row's.
                self.split_pane(Some(crate::split::Side::Right), window, cx);
                run(self, Place::Current);
            }
            None => run(self, Place::New),
        }
        self.focus_composer(cx);
        cx.notify();
    }

    /// The palette's Close pane (split.md §8): the focused pane's tabs
    /// close (Ctrl+Shift+T reopens them), and with its last the pane.
    pub(crate) fn close_pane_tabs(&mut self) {
        if !self.panes.split() {
            return;
        }
        let id = self.panes.focused;
        self.in_pane(id, |v| {
            for _ in 0..v.model.tabs.len() {
                if v.model.tabs.is_empty() || !v.panes.ids().contains(&id) {
                    break;
                }
                v.close_tab(v.model.tabs.len() - 1);
            }
        });
    }

    /// The palette's Join all panes (split.md §8): every other pane's
    /// session tabs into the focused pane, in reading order, once each;
    /// the other panes close.
    pub(crate) fn join_panes(&mut self, cx: &mut Context<Self>) {
        if !self.panes.split() {
            return;
        }
        let me = self.panes.focused;
        let others: Vec<PaneId> = self.reading_order().into_iter().filter(|id| *id != me).collect();
        // One flight for all of them (each close's own would start from
        // the tree before it, a jump): every other share to 0 on `sheet`.
        let was = self.panes.tree.clone();
        let mut add = Vec::new();
        for id in &others {
            let tabs = self
                .in_pane(*id, |v| {
                    v.model
                        .tabs
                        .iter()
                        .filter(|t| t.kind == crate::tabs::Kind::Session)
                        .filter_map(|t| Some((t.session.clone()?, t.label.clone())))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            add.extend(tabs);
        }
        let mine: Vec<String> = self.model.tabs.iter().filter_map(|t| t.session.clone()).collect();
        let mut seen = mine;
        let add: Vec<_> = add.into_iter().filter(|(s, _)| (!seen.contains(s)).then(|| seen.push(s.clone())).is_some()).collect();
        for &id in &others {
            self.focus_pane(id, KeysTo::Pane, cx);
            self.close_pane_tabs();
        }
        self.focus_pane(me, KeysTo::Pane, cx);
        // After its own tabs, its visible one still on screen.
        let visible = self.model.tabs.visible().map(|t| t.id);
        if let Some(last) = self.model.tabs.len().checked_sub(1) {
            self.model.tabs.focus(last);
        }
        for (s, label) in &add {
            self.model.tabs.open(s, label, crate::tabs::Place::New);
        }
        if let Some(ix) = visible.and_then(|id| self.model.tabs.iter().position(|t| t.id == id)) {
            self.model.tabs.focus(ix);
        }
        let to = was.solo(me);
        let mut flight = crate::split::Flight::new(was, to, None, None);
        flight.ghosts = others;
        self.panes.flight = Some(flight);
        self.model.rail_gen += 1;
        self.reconcile(None);
        self.focus_composer(cx);
        cx.notify();
    }

    /// Ctrl+Alt+→ / ← (split.md §9): the focused pane's active session tab
    /// to the next / previous pane in reading order, focused there. At the
    /// end of the order a pane with more than one tab splits one off with
    /// it (Ctrl+\'s rules); a pane's only tab moves and its pane closes.
    pub(crate) fn move_tab_pane(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.split_on() {
            return;
        }
        let Some(ix) = self.model.tabs.active() else { return };
        let Some((tab, session, label)) = self
            .model
            .tabs
            .iter()
            .nth(ix)
            .filter(|t| t.kind == crate::tabs::Kind::Session)
            .and_then(|t| Some((t.id, t.session.clone()?, t.label.clone())))
        else {
            return;
        };
        let source = self.panes.focused;
        let order = self.reading_order();
        let Some(i) = order.iter().position(|p| *p == source) else { return };
        let target = if back { i.checked_sub(1).map(|j| order[j]) } else { order.get(i + 1).copied() };
        let close = |v: &mut Self| {
            let ix = v.model.tabs.iter().position(|t| t.id == tab);
            if let Some(ix) = ix {
                v.close_tab(ix);
            }
        };
        match target {
            Some(to) => {
                self.focus_pane(to, KeysTo::Pane, cx);
                self.model.tabs.open(&session, &label, crate::tabs::Place::New);
                self.model.rail_gen += 1;
                self.reconcile(None);
                self.in_pane(source, close);
                self.focus_composer(cx);
            }
            None if !back && self.model.tabs.len() > 1 => {
                let before = self.panes.len();
                self.split_pane(None, window, cx);
                if self.panes.len() > before {
                    self.in_pane(source, close);
                }
            }
            None => {}
        }
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
        // Its share springs to 0 on `sheet`, the panes beside it taking the
        // space (split.md §8, §10).
        let was = std::mem::replace(&mut self.panes.tree, tree);
        self.panes.flight = Some(crate::split::Flight::new(was.clone(), was.zeroed(id), None, Some(id)));
        // Focus moves only off the pane that had it (Shady #36 L3b: a close
        // an event made never takes it from the pane in use), and the keys
        // with it, from the inputs that went (L3a).
        if self.panes.focused == id {
            self.panes.focused = to;
            self.keys_rearm = true;
            self.ask_keys(KeysTo::Closed);
        }
        // What it sent still unconfirmed went to another pane on its store
        // as it left the store (`AppView::repoint`, Shady #36 r2 L-c).
        let next = self.panes.focused;
        self.enter_pane(next);
        if let Some(i) = self.panes.rest.iter().position(|p| p.id == id) {
            // Its worker view, parked as it left, is no longer kept for it.
            let mut gone = self.panes.rest.remove(i);
            let view = std::mem::take(&mut gone.parked.view);
            self.model.lives[gone.live].watch.drop_parked(view);
            drop(gone);
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

    /// Split: the first pane in reading order, not the focused one, whose
    /// visible session has a card waiting (Ctrl+J, split.md §7).
    pub(crate) fn waiting_pane(&self) -> Option<PaneId> {
        if !self.panes.split() {
            return None;
        }
        let me = self.panes.focused;
        self.reading_order().into_iter().filter(|id| *id != me).find(|id| {
            self.panes.get(*id).is_some_and(|p| {
                let tabs = if p.id == self.panes.cur.id { &self.model.tabs } else { &p.parked.tabs };
                tabs.view().is_none() && !self.model.lives[p.live].transcript.pending_prompts().is_empty()
            })
        })
    }

    /// Every pane showing session `s` on its visible tab (the pane map's
    /// lit cells, split.md §6).
    pub(crate) fn panes_showing(&self, s: &str) -> Vec<PaneId> {
        let shows =
            |p: &Pane, tabs: &crate::tabs::Tabs| p.shown_tab.as_ref().and_then(|t| t.1.as_deref()) == Some(s) && tabs.view().is_none();
        let mut out: Vec<PaneId> = shows(&self.panes.cur, &self.model.tabs).then_some(self.panes.cur.id).into_iter().collect();
        out.extend(self.panes.rest.iter().filter(|p| shows(p, &p.parked.tabs)).map(|p| p.id));
        out
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

impl crate::app::AppView {
    /// The bench's scene moves to the synthetic session `name`
    /// ([`crate::model::Model::local_session`]), with every pane on its
    /// store (`SYNAPS_BENCH_PANES`): their tabs follow, as an attach's do.
    pub(crate) fn bench_session(&mut self, name: &str) {
        let live = self.panes.cur.live;
        self.model.local_session(live, name);
        let others: Vec<PaneId> = self.panes.rest.iter().filter(|p| p.live == live).map(|p| p.id).collect();
        for id in others {
            self.in_pane(id, |v| {
                v.model.tabs.attached(name);
                v.reconcile(None);
            });
        }
    }
}

/// [`pane_script`]'s `film3b` scene.
async fn film3b(view: &WeakEntity<crate::app::AppView>, cx: &mut AsyncWindowContext, ex: &BackgroundExecutor, t0: Instant) {
    use crate::split::{Axis, Side};
    let mark = |what: &str| eprintln!("PANES {what} {:.3}", t0.elapsed().as_secs_f64());
    let sleep = |s: f64| ex.timer(std::time::Duration::from_secs_f64(s));
    let act = |cx: &mut AsyncWindowContext, f: &mut dyn FnMut(&mut crate::app::AppView, &mut Window, &mut Context<crate::app::AppView>)| {
        let _ = view.update_in(cx, |v, window, cx| {
            f(v, window, cx);
            cx.notify();
        });
    };
    // A divider on `axis` held, moved `by` px over `secs` (60 moves a
    // second, as a pointer's), let go if `release`.
    async fn drag(
        view: &WeakEntity<crate::app::AppView>,
        cx: &mut AsyncWindowContext,
        ex: &BackgroundExecutor,
        axis: Axis,
        by: f32,
        secs: f64,
        release: bool,
    ) {
        let _ = view.update(cx, |v, _| {
            let (w, h) = v.centre.get();
            let d = v.panes.tree.dividers(crate::split::Rect { x: 0., y: 0., w, h }).into_iter().find(|d| d.axis == axis);
            v.panes.grab = d.map(|divider| crate::split::Grab { divider, from: 0., moved: false });
        });
        let n = (secs * 60.).round().max(1.) as usize;
        for k in 1..=n {
            ex.timer(std::time::Duration::from_secs_f64(1. / 60.)).await;
            let at = by * k as f32 / n as f32;
            let _ = view.update(cx, |v, cx| v.drag_divider(point(px(at), px(at)), cx));
        }
        if release {
            let _ = view.update(cx, |v, cx| v.release_divider(cx));
        }
    }
    mark("split");
    act(cx, &mut |v, w, cx| v.split_pane(None, w, cx));
    sleep(1.6).await;
    mark("split-down");
    act(cx, &mut |v, w, cx| v.split_pane(Some(Side::Down), w, cx));
    sleep(1.6).await;
    mark("drag-left");
    drag(view, cx, ex, Axis::Row, -240., 1.0, true).await;
    sleep(0.8).await;
    // Back to 6 px short of equal, let go: it lands on equal.
    mark("drag-detent");
    let back = view.update(cx, |v, _| {
        let (w, h) = v.centre.get();
        let r = v.panes.tree.rects(crate::split::Rect { x: 0., y: 0., w, h });
        let left = r.iter().map(|(_, r)| r.w).fold(f32::MAX, f32::min);
        ((w - crate::split::DIVIDER) / 2.).floor() - 6. - left
    });
    drag(view, cx, ex, Axis::Row, back.unwrap_or(0.), 0.8, true).await;
    sleep(1.2).await;
    mark("drag-down-to-min");
    drag(view, cx, ex, Axis::Column, 600., 0.8, true).await;
    sleep(0.6).await;
    mark("drag-right-to-min");
    drag(view, cx, ex, Axis::Row, 900., 1.0, true).await;
    sleep(0.4).await;
    mark("min-320x240");
    sleep(1.4).await;
    mark("double-click-equalise");
    act(cx, &mut |v, _, cx| {
        let (w, h) = v.centre.get();
        let d = v.panes.tree.dividers(crate::split::Rect { x: 0., y: 0., w, h });
        if let Some(d) = d.iter().find(|d| d.axis == Axis::Column) {
            v.click_divider(&d.path.clone(), 2, cx);
        }
    });
    sleep(1.2).await;
    act(cx, &mut |v, _, cx| {
        let (w, h) = v.centre.get();
        let d = v.panes.tree.dividers(crate::split::Rect { x: 0., y: 0., w, h });
        if let Some(d) = d.iter().find(|d| d.axis == Axis::Row) {
            v.click_divider(&d.path.clone(), 2, cx);
        }
    });
    sleep(1.6).await;
    for _ in 0..2 {
        mark("close");
        act(cx, &mut |v, _, _| {
            let last = v.reading_order().last().copied();
            if let Some(id) = last {
                v.in_pane(id, |v| {
                    if let Some(ix) = v.model.tabs.active() {
                        v.close_tab(ix);
                    }
                });
            }
        });
        sleep(1.6).await;
    }
    mark("done");
}

/// [`pane_script`]'s `film3b2` scene (split.md §2, §5, §7, §8, §9).
async fn film3b2(view: &WeakEntity<crate::app::AppView>, cx: &mut AsyncWindowContext, ex: &BackgroundExecutor, t0: Instant) {
    let mark = |what: &str| eprintln!("PANES {what} {:.3}", t0.elapsed().as_secs_f64());
    let sleep = |s: f64| ex.timer(std::time::Duration::from_secs_f64(s));
    let act = |cx: &mut AsyncWindowContext, f: &mut dyn FnMut(&mut crate::app::AppView, &mut Window, &mut Context<crate::app::AppView>)| {
        let _ = view.update_in(cx, |v, window, cx| {
            f(v, window, cx);
            cx.notify();
        });
    };
    use crate::palette::AppAct;
    mark("split");
    act(cx, &mut |v, w, cx| v.split_pane(None, w, cx));
    sleep(1.6).await;
    mark("open-idle");
    act(cx, &mut |v, _, _| {
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false },
            crate::tabs::Place::Current,
        )
    });
    sleep(1.8).await;
    mark("ctrl-j");
    act(cx, &mut |v, w, cx| v.jump_to_card(w, cx));
    sleep(1.8).await;
    mark("f6");
    act(cx, &mut |v, _, cx| v.focus_step(false, cx));
    sleep(0.6).await;
    mark("move-prev");
    act(cx, &mut |v, w, cx| v.move_tab_pane(true, w, cx));
    sleep(1.8).await;
    mark("move-next");
    act(cx, &mut |v, w, cx| v.move_tab_pane(false, w, cx));
    sleep(1.8).await;
    mark("open-to-side");
    act(cx, &mut |v, w, cx| v.open_to_side(&crate::named::RowAction::Attach(crate::demo::FLAKY.into()), w, cx));
    sleep(2.0).await;
    mark("equalise");
    act(cx, &mut |v, w, cx| v.app_act(AppAct::Equalise, w, cx));
    sleep(1.6).await;
    mark("join");
    act(cx, &mut |v, w, cx| v.app_act(AppAct::JoinPanes, w, cx));
    sleep(2.0).await;
    mark("done");
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
/// per pane, then the panes close one by one (each collapses). `film3b`
/// (split.md §4, §10): split right and down (each grows), the dividers
/// dragged as a pointer would (1:1, 60 moves a second), to their
/// minimums (the bottom right pane at 320 × 240), let go near equal (the
/// detent), a double-click (equalise), then the two closes. `film3b2`
/// (3b-2): a split, Ctrl+J into the pane that waits, a tab moved back
/// (its pane closes) and on (a pane splits off), Open to the side, the
/// palette's Equalise, then Join all panes. `three[:fold=<w>x<h>]`: three
/// panes across (the panels yield), then the window narrowed to fold.
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
            let film = how.starts_with("film");
            if how == "film3b" {
                sleep(5.0).await;
                film3b(&view, cx, &ex, t0).await;
                return;
            }
            if how == "film3b2" {
                sleep(5.0).await;
                film3b2(&view, cx, &ex, t0).await;
                return;
            }
            // Three across (the panels yield to their strips), then the
            // window to `fold=<w>x<h>` if given (the split folds there).
            if let Some(to) = how.strip_prefix("three") {
                sleep(5.0).await;
                let fold = to
                    .strip_prefix(":fold=")
                    .and_then(|s| s.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)));
                if fold.is_some() {
                    act(cx, &|_, window, _| window.resize(size(px(1280.), px(800.))));
                    sleep(1.0).await;
                }
                act(cx, &|v, w, cx| v.split_pane(None, w, cx));
                sleep(0.8).await;
                act(cx, &|v, _, _| {
                    v.open_tab(
                        crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false },
                        crate::tabs::Place::Current,
                    )
                });
                sleep(0.8).await;
                act(cx, &|v, w, cx| v.split_pane(None, w, cx));
                sleep(1.2).await;
                mark("three");
                if let Some((w, h)) = fold {
                    act(cx, &|_, window, _| window.resize(size(px(w), px(h))));
                    sleep(1.5).await;
                    mark("folded");
                }
                return;
            }
            sleep(if film { 5.0 } else { 1.5 }).await;
            if let Ok(n) = how.parse::<usize>() {
                for (at, side) in shape(n) {
                    act(cx, &|v, w, cx| {
                        if let Some(i) = at {
                            v.focus_nth(i, cx);
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
                act(cx, &|v, _, cx| match what {
                    "f6" => v.focus_step(false, cx),
                    "shift-f6" => v.focus_step(true, cx),
                    _ => v.focus_nth(0, cx),
                });
                sleep(1.2).await;
            }
            mark("drafts");
            for (i, text) in [(1usize, "a draft in the right pane"), (0, "and one in the left")] {
                act(cx, &|v, w, cx| {
                    v.focus_nth(i, cx);
                    v.panes.cur.composer.update(cx, |s, cx| s.set_value(text, w, cx));
                });
                sleep(1.2).await;
            }
            for _ in 0..2 {
                mark("close");
                act(cx, &|v, _, cx| {
                    v.focus_nth(1, cx);
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
