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
    /// The list on screen, numbered: a new one with every [`AppView::list_out`]
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
    /// Bumped when the text metrics change every row's height (a cached
    /// tab's list is re-measured when it comes back).
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
}

/// The window's panes. Step 1: exactly one, `cur`.
pub(crate) struct Panes {
    /// The pane on screen (the focused one: the only one).
    pub(crate) cur: Pane,
}

impl Panes {
    pub(crate) fn one(pane: Pane) -> Self {
        Panes { cur: pane }
    }
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
