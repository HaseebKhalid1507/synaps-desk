//! The main window (the approved "hybrid" layout): a top bar with what
//! needs you, the sessions rail, the session in the middle (its tabs, the
//! transcript as turns ([`crate::turns`]), authorization card, composer)
//! and its numbers on the right. Every colour is a Synaps TUI theme role
//! ([`crate::theme`]); every number comes from the daemon ([`crate::stats`]).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::transcript::{Item, ItemKind, ToolStatus};
use synaps_client::{AttachMode, Client, CreateOptions, Cwd, Event, RequestError, Transcript};

use crate::bench::BenchState;
use crate::files::{self, Preview};
use crate::fonts::TextPx as _;
use crate::fonts::{self, label};
use crate::glyph::{self, Ground};
use crate::model::{CONFIRM_NO, CONFIRM_YES, ConnState, Effect, Model, RenameRequest};
use crate::named::{ResumeRequest, RowAction, home_short};
use crate::rail::{Bucket, Chip};
use crate::stats::Stats;
use crate::subagents::Liveness;
use crate::tabs::{Drafts, Lru, Place, Saved};
use crate::theme::{self, Metrics, Palette};
use crate::turns::{self, Disp, Kind, Layout, Summaries};
use crate::watch::{Moved, Step, Watch};

/// Per-row strings derived from a transcript item, rebuilt only when the
/// item's `rev` changes (so a frame never re-allocates unchanged rows).
pub(crate) struct RowCache {
    id: u64,
    rev: u64,
    pub(crate) text: SharedString,
    /// Tool rows: the tool's name (lower case, as sent) and its colour
    /// role, the one-line argument, the result's first lines and the diff
    /// of a file tool.
    pub(crate) name: SharedString,
    pub(crate) hue: turns::Hue,
    pub(crate) arg: SharedString,
    pub(crate) result: SharedString,
    pub(crate) result_more: usize,
    pub(crate) preview: Option<Arc<Preview>>,
    /// A file tool's committed counts (the ledger's: `None` until it
    /// succeeded), and as shown.
    pub(crate) delta: Option<files::CallDelta>,
    pub(crate) counts: turns::Counts,
    /// The row's card parts (polish.md §2.1): its verb, a path target split
    /// in two, the meta facts and the well's header and numbered lines.
    pub(crate) card: turns::Card,
    /// The item's time ([`Item::at`], which is not in `rev`), and read in
    /// the local zone: parsed once here, never per frame or per sync.
    at: Option<Arc<str>>,
    pub(crate) time: Option<crate::times::Stamp>,
    /// The same instant on this machine's clock, read once against the
    /// wall clock: a running call's clock counts from it on the frame's
    /// instant ([`crate::motion::Motion::now`]).
    pub(crate) started: Option<Instant>,
    /// An event row's words ([`crate::events::EventView`]).
    pub(crate) event: Option<Arc<crate::events::EventView>>,
}

impl RowCache {
    /// The item's time as read now: kept if it is the one read before.
    fn retime(&mut self, item: &Item) {
        if self.at != item.at {
            self.at = item.at.clone();
            self.time = item.at.as_deref().and_then(crate::times::Stamp::local);
            self.started = item.at.as_deref().and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok()).map(|t| {
                let ago = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).to_std().unwrap_or_default();
                let now = Instant::now();
                now.checked_sub(ago).unwrap_or(now)
            });
        }
    }
}

/// Result lines an open tool row shows.
pub(crate) const RESULT_LINES: usize = 12;
/// Background tabs whose transcript and rows are kept (switching back to
/// one shows it at once, then its attach brings it up to date), at most
/// this many and this many bytes (estimated, [`TabCache::bytes`]).
pub(crate) const TAB_CACHE: usize = 8;
pub(crate) const TAB_CACHE_BYTES: usize = 64 << 20;
/// A cached tab's bytes per transcript item beyond its strings: the
/// `--bench-tabs` live (trimmed) RSS per cached tab, on the synthetic
/// fixture and on real saved sessions of 0.75, 3.2 and 24 MB, less the
/// strings counted, came to 1.1–2.1 KiB per item.
const ITEM_OVERHEAD: usize = 1536;
/// A splice longer than this reuses the rows of unchanged items.
const REUSE_ROWS: usize = 32;
/// What a tab opened by a create is called until the daemon names it.
pub(crate) const NEW_SESSION: &str = "New session";

/// A background tab's session as it was last on screen: its transcript and
/// numbers, and everything the list built from them (rows, folds, measured
/// heights and scroll position).
pub(crate) struct TabCache {
    transcript: Transcript,
    pub(crate) stats: Stats,
    rows: Vec<RowCache>,
    disp: Vec<Disp>,
    summaries: Summaries,
    list: ListState,
    toggled: HashSet<u64>,
    open_runs: HashSet<u64>,
    synced_busy: bool,
    files_gen: u64,
    waiting_on: Option<u64>,
    /// The text metrics its heights were measured with ([`AppView::metrics_gen`]).
    metrics_gen: u64,
    /// Its worker views (the one focused comes back focused).
    pub(crate) watch: Watch,
}

/// The list's state over one transcript: its rows, folds, measured heights
/// and scroll position. The session's waits here while a worker's view is
/// shown ([`AppView::sync_pane`]), so neither ever moves the other.
pub(crate) struct Pane {
    rows: Vec<RowCache>,
    disp: Vec<Disp>,
    summaries: Summaries,
    list: ListState,
    toggled: HashSet<u64>,
    open_runs: HashSet<u64>,
    synced_busy: bool,
    files_gen: u64,
    subs_gen: u64,
    waiting_on: Option<u64>,
    rows_session: Option<String>,
    scroll_carry: Option<(f32, ListOffset)>,
    metrics_gen: u64,
}

impl TabCache {
    /// What it keeps, roughly (bytes): the transcript's items (texts, tool
    /// inputs and results), the rows' strings, the display rows, the files
    /// ledger, and per item what the strings do not show (the list's
    /// heights, maps, allocator slack: [`ITEM_OVERHEAD`]). Measured once,
    /// when the tab is left.
    fn bytes(&self) -> usize {
        let item = |i: &Item| {
            std::mem::size_of::<Item>()
                + i.text.capacity()
                + i.tool
                    .as_ref()
                    .map_or(0, |t| t.tool_id.capacity() + t.name.capacity() + t.input.capacity() + t.result.capacity() + t.tail.capacity())
        };
        let row = |r: &RowCache| std::mem::size_of::<RowCache>() + r.text.len() + r.arg.len() + r.result.len() + r.card.bytes();
        self.transcript.items().iter().map(item).sum::<usize>()
            + self.transcript.len() * ITEM_OVERHEAD
            + self.rows.iter().map(row).sum::<usize>()
            + self.disp.len() * (std::mem::size_of::<Disp>() + 32)
            + self.stats.files.bytes()
            + self.stats.subagents.bytes()
            + self.watch.bytes()
    }
}

/// What a tab says when it is shown again with a send given back.
const UNSENT_NOTE: &str = "not confirmed before you left this tab: back here; check the transcript before sending it again";

/// How a tab's session is opened.
pub(crate) enum Open {
    /// A live session (`fallback`: its journal, if it ended meanwhile;
    /// `open`: an Open by name).
    Attach { session_id: String, fallback: Option<ResumeRequest>, open: bool },
    /// A saved one.
    Resume { req: ResumeRequest, open: bool },
    /// A new one.
    Create,
}

/// The events a synthetic daemon sends for an attach (`--offline`, the
/// tab bench): `(session id, attach_seq)`; none: never answered.
pub(crate) type AttachSource = Box<dyn Fn(&str, u64) -> Vec<Event>>;
/// The authorization card ignores clicks this long after it appears (the
/// TUI's `CONFIRM_ARM_DELAY` is 400 ms): it pushes the list up, so a click
/// meant for a row can land on it. Its keys wait longer ([`Arm`]).
pub(crate) const ARM: Duration = Duration::from_millis(500);
/// How far toward the ink Handoff in's light is (motion.md §10 amendment
/// 17: the old scan beam's, the column's brightest light); the settle
/// pass is the same light (polish.md P3).
pub(crate) const HANDOFF_LIT: f32 = 0.6;
/// How long a rename asked for another session waits for its attach.
pub(crate) const RENAME_WAIT: Duration = Duration::from_secs(10);
/// The chord that answers a card yes ([`card_key`]), as its button shows it.
pub(crate) const YES_KEY: &str = if cfg!(target_os = "macos") { "⌘ENTER" } else { "CTRL+ENTER" };
/// A folded side panel's strip.
pub(crate) const STRIP: f32 = 32.;
/// The dock's and the composer field's heights on their springs. Q5's
/// motion keys share `0x7AF*`: `0x7AF0` the panels' widths and
/// `0x7AF3` a rail section's fold and FAVOURITES' chevron (chrome.rs),
/// `0x7AF1` / `0x7AF2` these, `0x7AF4` a chip's cell (tray.rs).
const DOCK_H: u64 = 0x7AF1 << 48;
const COMPOSER_H: u64 = 0x7AF2 << 48;

/// `el`, whose laid-out height goes to `sink`: when it changes under
/// [`crate::motion::Level::Full`], one more frame, so its box springs to
/// it (the dock, the composer's field).
fn measured(el: Div, sink: &std::rc::Rc<std::cell::Cell<f32>>, full: bool) -> Div {
    let sink = sink.clone();
    el.relative().child(
        canvas(
            move |b, window, _| {
                let h = f32::from(b.size.height);
                let was = sink.replace(h);
                if full && was.is_finite() && was != h {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0(),
    )
}

/// What `measured` measured, in a box `shown` tall on a spring, anchored
/// at its bottom (it grows upward; the line at its foot stays put) and
/// clipped to the box's top and bottom. `edge`: drawn over its top while
/// it cuts into what leads it (a sheet coming up leads with its edge).
fn sprung(el: Div, shown: f32, name: &'static str, edge: Option<AnyElement>) -> AnyElement {
    crate::motion::clip_y(
        div()
            .debug_selector(move || name.into())
            .relative()
            .w_full()
            .h(px(shown))
            .child(div().absolute().left_0().right_0().bottom_0().child(el))
            .children(edge)
            .into_any_element(),
    )
}
/// Narrower than this, the open right panel is not drawn (the session wins).
const CENTRE_MIN: f32 = 480.;

actions!(
    synaps_desktop,
    [
        ToggleRail,
        ToggleRight,
        TextBigger,
        TextSmaller,
        TextReset,
        NewTab,
        CloseTab,
        ReopenTab,
        NextTab,
        PrevTab,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        LastTab,
        OpenSettings,
        OpenUsage,
        WorkerUp,
        WorkerDown,
        WorkerTab,
        RenameSession,
        ToggleFavourite,
        CardTab,
        MenuTab,
        MenuTabBack,
        OpenGoTo,
        OpenCommands
    ]
);

/// A prompt as [`AppView::card_first`] knows it: its session, its id and
/// its `raised_at`.
pub(crate) type PromptAt = (String, u64, Option<String>);

/// The key context while the card's keys are live and it offers Allow all:
/// Tab and Shift+Tab go to its box and back ([`AppView::card_tab`]), ahead
/// of the kit's focus cycling.
const ALLOW_ALL_CTX: &str = "AllowAll";

/// The key context while a worker's, a session's or the workers menu is
/// open: Tab and Shift+Tab step it ([`AppView::menu_tab`]), ahead of the
/// kit's focus cycling (a binding, which runs before the menus' key
/// listener).
const MENU_CTX: &str = "Menu";

/// Ctrl+B folds the sessions rail, Ctrl+Alt+B the right panel; Ctrl+= /
/// Ctrl+- / Ctrl+0 step the centre pane's text size; the tab keys
/// ([`tab_keys`]). Cmd on macOS. The composer and inputs bind none of them.
pub fn bind_keys(cx: &mut App) {
    let mac = cfg!(target_os = "macos");
    let (rail, right) = panel_keys(mac);
    let [bigger, plus, shift_eq, smaller, reset] = text_keys(mac);
    let mut keys = vec![
        KeyBinding::new(rail, ToggleRail, None),
        KeyBinding::new(right, ToggleRight, None),
        KeyBinding::new(bigger, TextBigger, None),
        KeyBinding::new(plus, TextBigger, None),
        KeyBinding::new(shift_eq, TextBigger, None),
        KeyBinding::new(smaller, TextSmaller, None),
        KeyBinding::new(reset, TextReset, None),
    ];
    let (settings, usage, usage_alt) = crate::views::view_keys(mac, cfg!(target_os = "linux"));
    keys.push(KeyBinding::new(settings, OpenSettings, None));
    keys.push(KeyBinding::new(usage, OpenUsage, None));
    if let Some(k) = usage_alt {
        keys.push(KeyBinding::new(k, OpenUsage, None));
    }
    for &(k, t) in tab_keys(mac) {
        keys.push(match t {
            TabKey::New => KeyBinding::new(k, NewTab, None),
            TabKey::Close => KeyBinding::new(k, CloseTab, None),
            TabKey::Reopen => KeyBinding::new(k, ReopenTab, None),
            TabKey::Next => KeyBinding::new(k, NextTab, None),
            TabKey::Prev => KeyBinding::new(k, PrevTab, None),
            TabKey::Nth(0) => KeyBinding::new(k, Tab1, None),
            TabKey::Nth(1) => KeyBinding::new(k, Tab2, None),
            TabKey::Nth(2) => KeyBinding::new(k, Tab3, None),
            TabKey::Nth(3) => KeyBinding::new(k, Tab4, None),
            TabKey::Nth(4) => KeyBinding::new(k, Tab5, None),
            TabKey::Nth(5) => KeyBinding::new(k, Tab6, None),
            TabKey::Nth(6) => KeyBinding::new(k, Tab7, None),
            TabKey::Nth(_) => KeyBinding::new(k, Tab8, None),
            TabKey::Last => KeyBinding::new(k, LastTab, None),
        });
    }
    for &(k, step) in &worker_keys(mac) {
        keys.push(match step {
            Step::Up => KeyBinding::new(k, WorkerUp, None),
            Step::Down => KeyBinding::new(k, WorkerDown, None),
        });
    }
    for &k in crate::wtabs::tab_keys(mac) {
        keys.push(KeyBinding::new(k, WorkerTab, None));
    }
    keys.push(KeyBinding::new("tab", CardTab, Some(ALLOW_ALL_CTX)));
    keys.push(KeyBinding::new("shift-tab", CardTab, Some(ALLOW_ALL_CTX)));
    keys.push(KeyBinding::new("tab", MenuTab, Some(MENU_CTX)));
    keys.push(KeyBinding::new("shift-tab", MenuTabBack, Some(MENU_CTX)));
    keys.push(KeyBinding::new(RENAME_KEY, RenameSession, None));
    keys.push(KeyBinding::new(fav_key(mac), ToggleFavourite, None));
    let (goto, commands) = palette_keys(mac);
    keys.push(KeyBinding::new(goto, OpenGoTo, None));
    keys.push(KeyBinding::new(commands, OpenCommands, None));
    cx.bind_keys(keys);
}

/// The palette's keys (polish.md P16), as VS Code's: Ctrl+P goes to,
/// Ctrl+K runs a command; Cmd on macOS. Bound at the root with no context,
/// so the composer's input (which binds neither) never swallows them.
pub(crate) const fn palette_keys(mac: bool) -> (&'static str, &'static str) {
    if mac { ("cmd-p", "cmd-k") } else { ("ctrl-p", "ctrl-k") }
}

/// Favourites the session on screen, or unfavourites it (as a browser's
/// bookmark key): Ctrl+D, Cmd+D on macOS. No input, app, view or card key
/// is it (`tab_keys_parse_and_clash_with_nothing`).
pub(crate) const fn fav_key(mac: bool) -> &'static str {
    if mac { "cmd-d" } else { "ctrl-d" }
}

/// [`fav_key`] as the session menu shows it.
const FAV_KEY: &str = if cfg!(target_os = "macos") { "⌘D" } else { "Ctrl+D" };

/// Names the session on screen (opens or closes its naming row), as the
/// session menu's Rename… and a double-click on its tab's title.
pub(crate) const RENAME_KEY: &str = "f2";

/// The worker keys ([`crate::watch`]): Ctrl+↑/↓ and Alt+↑/↓ as in the TUI
/// (Option+↑/↓ first on macOS, where Ctrl+↑/↓ are Mission Control's). No
/// input binds them: in the composer they switch the view, the caret
/// stays where it was.
pub(crate) const fn worker_keys(mac: bool) -> [(&'static str, Step); 4] {
    if mac {
        [("alt-up", Step::Up), ("alt-down", Step::Down), ("ctrl-up", Step::Up), ("ctrl-down", Step::Down)]
    } else {
        [("ctrl-up", Step::Up), ("ctrl-down", Step::Down), ("alt-up", Step::Up), ("alt-down", Step::Down)]
    }
}

const COMPOSER_PLACEHOLDER: &str = "Message, or steer a running turn  (Enter sends, Shift+Enter: new line)";

/// The worker key as the hints spell it.
pub(crate) const WORKER_KEY: &str = if cfg!(target_os = "macos") { "⌥↑/↓" } else { "Ctrl+↑/↓" };

/// What a tab key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabKey {
    /// A new session in a new tab.
    New,
    /// Close the visible tab (detach; the session keeps running).
    Close,
    /// The last closed tab.
    Reopen,
    Next,
    Prev,
    /// Tab 1–8 (0-based).
    Nth(usize),
    Last,
}

/// The tab keys, as browsers and VS Code have them: Ctrl+T / Ctrl+W /
/// Ctrl+Shift+T, Ctrl+Tab and Ctrl+Shift+Tab, Ctrl+PageDown / PageUp,
/// Ctrl+1–8 and Ctrl+9 for the last. On macOS Cmd for each but Ctrl+Tab
/// (Cmd+Tab is the app switcher), plus Cmd+Shift+] / [ (spelled both ways:
/// AppKit may report the shifted key as `}` / `{`). None is bound by the
/// inputs (they take Tab, PageUp/PageDown and Ctrl/Cmd+[ / ] unshifted).
pub(crate) fn tab_keys(mac: bool) -> &'static [(&'static str, TabKey)] {
    use TabKey::*;
    const LINUX: [(&str, TabKey); 16] = [
        ("ctrl-t", New),
        ("ctrl-w", Close),
        ("ctrl-shift-t", Reopen),
        ("ctrl-tab", Next),
        ("ctrl-shift-tab", Prev),
        ("ctrl-pagedown", Next),
        ("ctrl-pageup", Prev),
        ("ctrl-1", Nth(0)),
        ("ctrl-2", Nth(1)),
        ("ctrl-3", Nth(2)),
        ("ctrl-4", Nth(3)),
        ("ctrl-5", Nth(4)),
        ("ctrl-6", Nth(5)),
        ("ctrl-7", Nth(6)),
        ("ctrl-8", Nth(7)),
        ("ctrl-9", Last),
    ];
    const MAC: [(&str, TabKey); 20] = [
        ("cmd-t", New),
        ("cmd-w", Close),
        ("cmd-shift-t", Reopen),
        ("ctrl-tab", Next),
        ("ctrl-shift-tab", Prev),
        ("cmd-shift-]", Next),
        ("cmd-shift-[", Prev),
        ("cmd-}", Next),
        ("cmd-{", Prev),
        ("cmd-pagedown", Next),
        ("cmd-pageup", Prev),
        ("cmd-1", Nth(0)),
        ("cmd-2", Nth(1)),
        ("cmd-3", Nth(2)),
        ("cmd-4", Nth(3)),
        ("cmd-5", Nth(4)),
        ("cmd-6", Nth(5)),
        ("cmd-7", Nth(6)),
        ("cmd-8", Nth(7)),
        ("cmd-9", Last),
    ];
    if mac { &MAC } else { &LINUX }
}

/// `key` (a keystroke's key, as GPUI names it) is one a tab key is made of.
fn is_tab_key(key: &str) -> bool {
    tab_keys(cfg!(target_os = "macos")).iter().any(|(k, _)| k.rsplit('-').next().is_some_and(|last| last.eq_ignore_ascii_case(key)))
}

/// The text size keys: `=`, `+` (its own key on DE and Nordic layouts)
/// and Shift+`=` (US `+`, as some backends report it).
const fn text_keys(mac: bool) -> [&'static str; 5] {
    if mac { ["cmd-=", "cmd-+", "cmd-shift-=", "cmd--", "cmd-0"] } else { ["ctrl-=", "ctrl-+", "ctrl-shift-=", "ctrl--", "ctrl-0"] }
}

/// The fold keys (`KeyBinding::new` does not resolve `secondary-`, and
/// panics on a keystroke it cannot parse: spelled per OS).
pub(crate) const fn panel_keys(mac: bool) -> (&'static str, &'static str) {
    if mac { ("cmd-b", "cmd-alt-b") } else { ("ctrl-b", "ctrl-alt-b") }
}

/// The rail's and the right panel's widths in the 1280-px layout: the auto
/// widths never go under them, and a dragged width wider than them gives
/// way down to them, not further, when the window narrows. The rail's: its
/// three chips on one line at 12 px (measured: 255 px of 268 with one-digit
/// counts; a two-digit count adds a 7.2-px cell), line 2 room for its sub.
pub(crate) const FLOORS: (f32, f32) = (312., 296.);
/// The auto rail's narrowest (PR #7's floor): beside a strip, on a window
/// too narrow for its floor and the centre's [`CENTRE_MIN`], a rail the
/// user never dragged gives way down to this (its chips wrap). A dragged
/// one stops at its floor.
const RAIL_YIELD: f32 = 276.;
/// A drag past this share of a panel's narrowest width folds it.
const SNAP: f32 = 0.6;

/// The rail's and the right panel's auto widths at a window width: wider
/// on wide screens, never under [`FLOORS`].
pub(crate) fn side_widths(window: f32) -> (f32, f32) {
    ((window * 0.135).round().clamp(FLOORS.0, 340.), (window * 0.14).round().clamp(FLOORS.1, 360.))
}

/// A panel's drag limits (px).
fn limits(rail: bool) -> (f32, f32) {
    let (lo, hi) = if rail { theme::RAIL_WIDTHS } else { theme::RIGHT_WIDTHS };
    (lo.into(), hi.into())
}

/// The widths the panels ask for: the dragged one, else the auto one.
fn wanted(window: f32, panels: theme::Panels) -> (f32, f32) {
    let (rail, right) = side_widths(window);
    let hand = |w: Option<u16>, rail: bool| w.map(|w| f32::from(w).clamp(limits(rail).0, limits(rail).1));
    (hand(panels.rail_width, true).unwrap_or(rail), hand(panels.right_width, false).unwrap_or(right))
}

/// A drag of a panel's edge: where the pointer went down (`x`), the panels
/// then and the panel's width then (a strip's, if folded).
#[derive(Clone, Copy, Debug)]
pub(crate) struct EdgeGrab {
    pub(crate) rail: bool,
    x: f32,
    start: theme::Panels,
    width: f32,
    pub(crate) moved: bool,
    /// The width the pointer asks for (unclamped): a release near the fold
    /// threshold folds ([`detent`]).
    want: f32,
}

/// GPUI's drag value for an edge handle, and the (empty) view it draws at
/// the pointer.
#[derive(Clone, Copy)]
pub(crate) struct EdgeDrag;

impl Render for EdgeDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// How a side panel is drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Side {
    Open(f32),
    /// Folded, or open but the window is too narrow for it: never hidden,
    /// its strip is how it opens again.
    Strip,
}

impl Side {
    pub(crate) fn width(self) -> f32 {
        match self {
            Side::Open(w) => w,
            Side::Strip => STRIP,
        }
    }
}

/// Both side panels at a window width (`mark`: the left mark strip). The
/// rail opens from 760 px; the right panel only if the centre keeps
/// [`CENTRE_MIN`] with both at their floors (a panel dragged narrower than
/// its floor: that width). Panels that ask for more than the room left
/// give way in proportion, down to their floors; a strip comes before the
/// centre is squeezed. With auto widths this is the PR #7 layout exactly.
pub(crate) fn sides(window: f32, panels: theme::Panels, mark: f32) -> (Side, Side) {
    let (want_l, want_r) = wanted(window, panels);
    let (floor_l, floor_r) = (want_l.min(FLOORS.0), want_r.min(FLOORS.1));
    // What the two side panels may take without squeezing the centre.
    let room = window - mark - CENTRE_MIN;
    let rail_open = !panels.rail_collapsed && window >= 760.;
    let right_open = !panels.right_collapsed && room - if rail_open { floor_l } else { STRIP } >= floor_r;
    match (rail_open, right_open) {
        (true, true) => {
            let (mut l, mut r) = (want_l, want_r);
            let over = l + r - room;
            if over > 0. {
                // `right_open` leaves the floors room: the slack covers it.
                let (sl, sr) = (l - floor_l, r - floor_r);
                let k = over / (sl + sr);
                (l, r) = ((l - sl * k).floor(), (r - sr * k).floor());
            }
            (Side::Open(l), Side::Open(r))
        }
        (true, false) => {
            let floor = if panels.rail_width.is_none() { RAIL_YIELD } else { floor_l };
            (Side::Open(want_l.min((room - STRIP).max(floor)).floor()), Side::Strip)
        }
        (false, true) => (Side::Strip, Side::Open(want_r.min(room - STRIP).floor())),
        (false, false) => (Side::Strip, Side::Strip),
    }
}

/// The panels while a panel's edge is dragged (`rail`, else the right
/// panel's) to where the panel would be `width` px wide; `start`: the
/// panels when the drag began (every move is worked out from them, so a
/// drag can fold and unfold as it goes). Under [`SNAP`] of the narrowest
/// width the panel folds, keeping the width it had; dragged out of a strip
/// it opens as [`toggled`] would. The width is clamped to the panel's
/// limits and to the room the other panel leaves the centre, never less
/// than the panel already had.
pub(crate) fn dragged(window: f32, start: theme::Panels, rail: bool, width: f32) -> theme::Panels {
    let (lo, hi) = limits(rail);
    let pick = |(l, r): (Side, Side)| if rail { (l, r) } else { (r, l) };
    let (was, _) = pick(sides(window, start, crate::chrome::MARK));
    let was_open = matches!(was, Side::Open(_));
    if width < lo * SNAP {
        return if was_open { toggled(window, start, rail).unwrap_or(start) } else { start };
    }
    let Some(mut p) = (if was_open { Some(start) } else { toggled(window, start, rail) }) else { return start };
    let (_, other) = pick(sides(window, p, crate::chrome::MARK));
    let room = window - crate::chrome::MARK - CENTRE_MIN - other.width();
    let w = width.round().clamp(lo, hi.min(room.max(was.width())).max(lo).floor());
    // The other panel keeps what it shows: one that had given way to a
    // narrow window is held there, so this edge follows the pointer.
    let (want_l, want_r) = wanted(window, p);
    let pin = |o: Side, want: f32| match o {
        Side::Open(ow) if ow < want => Some(ow as u16),
        _ => None,
    };
    if rail {
        p.rail_width = Some(w as u16);
        p.right_width = pin(other, want_r).or(p.right_width);
    } else {
        p.right_width = Some(w as u16);
        p.rail_width = pin(other, want_l).or(p.rail_width);
    }
    p
}

/// A drag let go this close to a detent springs to it (fluid.md §3.8).
pub(crate) const DETENT: f32 = 24.;

/// The panels a drag of panel `rail` (else the right one) settles to when
/// let go with the pointer asking for `want` px, if it is within
/// [`DETENT`] of a detent: the fold threshold (it folds, keeping the width
/// it had when the drag began, `start`: the drag clamped it to its limit
/// on the way), the auto width (it is auto again) or the floor
/// ([`FLOORS`], 312 for the rail). None: it stays where it was let go.
pub(crate) fn detent(window: f32, start: theme::Panels, panels: theme::Panels, rail: bool, want: f32) -> Option<theme::Panels> {
    let pick = |(l, r): (Side, Side)| if rail { l } else { r };
    let Side::Open(w) = pick(sides(window, panels, crate::chrome::MARK)) else { return None };
    let fold = limits(rail).0 * SNAP;
    if want >= fold && want < fold + DETENT {
        // Dragged out of its strip and back: the strip it was.
        return match pick(sides(window, start, crate::chrome::MARK)) {
            Side::Open(_) => toggled(window, start, rail),
            Side::Strip => Some(start),
        };
    }
    let auto = if rail { side_widths(window).0 } else { side_widths(window).1 };
    let floor = if rail { FLOORS.0 } else { FLOORS.1 };
    let set = |width: Option<u16>| {
        let mut p = panels;
        if rail {
            p.rail_width = width;
        } else {
            p.right_width = width;
        }
        p
    };
    let to = [(auto, None), (floor, Some(floor as u16))]
        .into_iter()
        .filter(|(d, _)| (w - d).abs() <= DETENT)
        .min_by(|a, b| (w - a.0).abs().total_cmp(&(w - b.0).abs()))?;
    let p = set(to.1);
    (p != panels).then_some(p)
}

/// The panels after the user folds or unfolds one (`rail`, else the right
/// panel) as it is shown at this width: an open one folds; a strip opens,
/// folding the other if only that makes room. `None`: the window is too
/// narrow to open it at all. What is saved is what is shown, so a panel
/// that the window could not fit is saved folded, not "open" and unseen.
pub(crate) fn toggled(window: f32, panels: theme::Panels, rail: bool) -> Option<theme::Panels> {
    let open = |p: theme::Panels, rail: bool| {
        let (r, rt) = sides(window, p, crate::chrome::MARK);
        matches!(if rail { r } else { rt }, Side::Open(_))
    };
    let set = |p: &mut theme::Panels, rail: bool, folded: bool| {
        if rail {
            p.rail_collapsed = folded;
        } else {
            p.right_collapsed = folded;
        }
    };
    let mut p = panels;
    if open(p, rail) {
        set(&mut p, rail, true);
    } else {
        set(&mut p, rail, false);
        if !open(p, rail) {
            set(&mut p, !rail, true);
        }
        if !open(p, rail) {
            return None;
        }
    }
    Some(theme::Panels { rail_collapsed: !open(p, true), right_collapsed: !open(p, false), ..panels })
}

/// Tells a key's press from its auto-repeats (the fold keys, the tab
/// keys, the card's Enter and Esc). Wayland, Windows and macOS repeat a
/// held key without a key-up between; X11 (GPUI does not ask for
/// detectable auto-repeat) sends a key-up right before each repeat, at the
/// same instant. GPUI drops that key-up when both come in one batch; when
/// they straddle two, the key-up comes through and the repeat follows it
/// at once. No hand lets go of a key and presses it again in
/// [`FoldKeys::PAIR`].
#[derive(Debug, Default)]
pub(crate) struct FoldKeys {
    pressed: Option<Instant>,
    released: Option<Instant>,
}

impl FoldKeys {
    const PAIR: Duration = Duration::from_millis(30);
    /// A key-up this window never saw (the window lost focus while the key
    /// was down) stops counting after this long without a repeat: longer
    /// than the usual first-repeat delays (X11 660 ms, Windows up to 1 s).
    const STALE: Duration = Duration::from_millis(1100);

    /// A fold key went down at `now`: `true` for a press, `false` for a
    /// repeat. `held`: the key-down says it is one (`KeyDownEvent::is_held`:
    /// AppKit's `isARepeat`, Windows' repeat bit, Wayland's repeats; X11
    /// never says, and the key-up rule tells there).
    pub(crate) fn press(&mut self, now: Instant, held: bool) -> bool {
        self.press_on(now, held, cfg!(target_os = "macos"))
    }

    /// [`FoldKeys::press`] on macOS (`mac`) or not.
    pub(crate) fn press_on(&mut self, now: Instant, held: bool, mac: bool) -> bool {
        let repeat = held
            || match (self.pressed, self.released) {
                (None, _) => false,
                (Some(_), Some(up)) => now.saturating_duration_since(up) < Self::PAIR,
                // Not on macOS: AppKit drops the key-up of B when it is let
                // go while Cmd is still held, so a quick unfold (or tapping B
                // with Cmd held) would read as a repeat and do nothing. Its
                // repeats say `held` instead.
                (Some(down), None) => !mac && now.saturating_duration_since(down) < Self::STALE,
            };
        self.pressed = Some(now);
        self.released = None;
        !repeat
    }

    /// The `B` key went up.
    pub(crate) fn release(&mut self, now: Instant) {
        if self.pressed.is_some() {
            self.released = Some(now);
        }
    }
}

/// The card's answer keys, Enter and Esc, and Space (a worker menu's pick
/// key, with Enter), as every key-down and key-up in the window passes
/// (the root's capture), whoever takes it: whether a key-down is a new
/// press ([`FoldKeys`]). Only a new press picks a menu item, or answers a
/// card ([`Arm::key`]): a key still down from before the card came, or from
/// before its keys armed, never does, until it has been let go and
/// pressed again. The modifiers do not count: an Enter held, then Ctrl,
/// is still the Enter held. On X11 a held key's first repeat comes after
/// the repeat delay (660 ms by default, 600 on KDE), past [`ARM`], and
/// says nothing of being one.
#[derive(Debug, Default)]
pub(crate) struct CardChord {
    enter: FoldKeys,
    escape: FoldKeys,
    space: FoldKeys,
}

impl CardChord {
    fn of(&mut self, key: &str) -> Option<&mut FoldKeys> {
        match key {
            "enter" => Some(&mut self.enter),
            "escape" => Some(&mut self.escape),
            "space" => Some(&mut self.space),
            _ => None,
        }
    }

    /// `key` went down at `now` (`held`: the platform says it is a repeat):
    /// true for a new press of Enter, Esc or Space.
    pub(crate) fn down(&mut self, key: &str, now: Instant, held: bool) -> bool {
        self.of(key).is_some_and(|k| k.press(now, held))
    }

    pub(crate) fn up(&mut self, key: &str, now: Instant) {
        if let Some(k) = self.of(key) {
            k.release(now);
        }
    }
}

/// A key whose auto-repeats must not act: a held Ctrl+T (Cmd+T) is one new
/// tab, not a burst of daemon sessions; a held Ctrl+W closes one tab. GPUI
/// says whether a key-down is a repeat (`is_held`) only to key listeners,
/// which run after the actions, so the action leaves the decision to the
/// key-down that carries it ([`AppView::once_key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnceKey {
    /// Ctrl+B (`true`, the rail) / Ctrl+Alt+B.
    Fold(bool),
    /// New, Close, Reopen.
    Tab(TabKey),
}

impl OnceKey {
    /// Its key's letter: the key-down that follows the action is its own.
    fn letter(self) -> &'static str {
        match self {
            OnceKey::Fold(_) => "b",
            OnceKey::Tab(TabKey::Close) => "w",
            OnceKey::Tab(_) => "t",
        }
    }
}

/// A fold key the view did not get (the focus is outside it): an app-wide
/// listener, which GPUI runs when nothing on the focus path handled the
/// action. It folds, then gives the view the focus back once the dispatch
/// is over, so the key-up (and the keys after it) reach the view again.
/// A press here is always a press: the key-up of the one before it went
/// where the focus was, not to the view.
fn fold_fallback<A: Action>(rail: bool, view: WeakEntity<AppView>, window: AnyWindowHandle, focus: FocusHandle, cx: &mut App) {
    cx.on_action(move |_: &A, cx| {
        // The view had it: its key-down decides ([`OnceKey`]).
        if view.read_with(cx, |v, _| v.once.is_some()).unwrap_or(false) {
            cx.propagate();
            return;
        }
        let _ = view.update(cx, |v, cx| {
            v.fold_keys = FoldKeys::default();
            v.toggle_panel(rail, cx);
        });
        let focus = focus.clone();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| window.focus(&focus, cx));
        });
    });
}

/// The tab keys' actions on the window's root (the composer, find and
/// the other inputs bind none of them, so they act wherever the focus is).
fn tab_actions(d: Stateful<Div>, cx: &mut Context<AppView>) -> Stateful<Div> {
    macro_rules! on {
        ($d:expr, $($a:ident => $k:expr),* $(,)?) => {
            $d$(.on_action(cx.listener(|this, _: &$a, _, cx| this.tab_key($k, cx))))*
        };
    }
    on!(d,
        NewTab => TabKey::New, CloseTab => TabKey::Close, ReopenTab => TabKey::Reopen,
        NextTab => TabKey::Next, PrevTab => TabKey::Prev,
        Tab1 => TabKey::Nth(0), Tab2 => TabKey::Nth(1), Tab3 => TabKey::Nth(2), Tab4 => TabKey::Nth(3),
        Tab5 => TabKey::Nth(4), Tab6 => TabKey::Nth(5), Tab7 => TabKey::Nth(6), Tab8 => TabKey::Nth(7),
        LastTab => TabKey::Last,
    )
}

/// A tab key the view did not get (the focus left the tree with an
/// input): as [`fold_fallback`], it acts and hands the view its focus back.
fn tab_fallback<A: Action>(key: TabKey, view: WeakEntity<AppView>, window: AnyWindowHandle, focus: FocusHandle, cx: &mut App) {
    cx.on_action(move |_: &A, cx| {
        if view.read_with(cx, |v, _| v.once.is_some()).unwrap_or(false) {
            cx.propagate();
            return;
        }
        let _ = view.update(cx, |v, cx| {
            v.tab_keys = FoldKeys::default();
            v.tab_act(key, cx);
        });
        let focus = focus.clone();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| window.focus(&focus, cx));
        });
    });
}

/// A fold is saved this long after the last one, off the UI thread: a
/// burst of folds is one write.
pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(400);
/// The pointer rests this long on a rail row before its readout opens.
const READOUT_DELAY: Duration = Duration::from_millis(400);

/// `inner` in the centre pane's column ([`Layout::column`], centred).
/// `inner` on the centre column, at the list's own (whole-pixel) padding.
pub(crate) fn column(lay: Layout, inner: impl IntoElement) -> Div {
    h_flex().w_full().pl(px(lay.left)).child(div().flex_none().w(px(lay.column)).min_w_0().child(inner))
}

/// What is known of a session, in the order the session header once said
/// it ([`AppView::session_meta`]): its tab's tooltip; model · effort in the
/// composer foot, `detached` its hint, the folder the right pane's FILES.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionMeta {
    pub(crate) title: Option<SharedString>,
    pub(crate) name: Option<SharedString>,
    pub(crate) model: Option<SharedString>,
    pub(crate) effort: Option<SharedString>,
    pub(crate) cwd: Option<SharedString>,
    pub(crate) detached: bool,
}

impl SessionMeta {
    /// The whole line (design.md §8.5): title, name, model, effort, folder
    /// and `detached`, each if known, joined with `·`; `label` (the tab's)
    /// leads when the title is not known.
    pub(crate) fn tooltip(&self, label: &str) -> String {
        let detached = self.detached.then_some("detached");
        let title = self.title.as_deref().unwrap_or(label);
        let name = self.name.as_deref().filter(|n| *n != title);
        let parts = [name, self.model.as_deref(), self.effort.as_deref(), self.cwd.as_deref(), detached];
        std::iter::once(title).chain(parts.into_iter().flatten()).collect::<Vec<_>>().join(" · ")
    }
}

/// What a session's menu does (a right-click on its tab or rail row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAct {
    Rename,
    Close,
    /// Favourite or unfavourite it (desktop only: never the daemon's pin).
    Favourite,
}

/// Where a session's menu was opened: its tab, or its rail row (its key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAt {
    Tab(u64),
    Row(u64),
}

/// The session a menu is for, and its tab if it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionTarget {
    pub(crate) session: String,
    pub(crate) tab: Option<u64>,
    pub(crate) row: Option<u64>,
}

/// What a session's menu is for: a held session (its live id), and
/// whether it is a favourite (`None`: it cannot be one yet, not listed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MenuFor {
    pub(crate) tab: bool,
    pub(crate) live: bool,
    pub(crate) fav: Option<bool>,
}

/// The session menu's items: Rename… (F2, a held session's), Favourite /
/// Unfavourite (Ctrl+D) and Close tab (the tab key).
pub(crate) fn session_items(f: MenuFor) -> Vec<crate::menu::Item<SessionAct>> {
    use crate::menu::Item;
    let fav = Item::new(SessionAct::Favourite, if f.fav == Some(true) { "Unfavourite" } else { "Favourite" }).key(FAV_KEY);
    let fav = if f.fav.is_some() { fav } else { fav.off("not listed by the daemon yet") };
    let rename = Item::new(SessionAct::Rename, "Rename…").key("F2");
    let rename = if f.live { rename } else { rename.off("open it to name it") };
    let close = Item::new(SessionAct::Close, "Close tab").key(CLOSE_KEY);
    vec![rename, fav, if f.tab { close } else { close.off("it is not open in a tab") }]
}

/// The close-tab key as the session menu shows it.
const CLOSE_KEY: &str = if cfg!(target_os = "macos") { "⌘W" } else { "Ctrl+W" };

/// The composer foot's parts (design.md §8.11), as the room allows: what
/// drops first when the row is short of width. The glyph, MAIN, HALT and
/// the primary button never drop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Foot {
    pub(crate) hint: bool,
    pub(crate) effort: bool,
    /// Model (main view) or facts (worker view).
    pub(crate) model: bool,
    /// The state word and its count (also gone in a tight pane).
    pub(crate) word: bool,
    /// The worker's name and handle (worker view).
    pub(crate) name: bool,
}

/// The widths a [`Foot`] is fitted from (px), each part's own; 0: none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FootIn {
    /// The row's inner width (the composer's padding off).
    pub(crate) room: f32,
    /// The lead: the glyph column (main), or MAIN › and the glyph (worker).
    pub(crate) lead: f32,
    pub(crate) word: f32,
    pub(crate) name: f32,
    pub(crate) model: f32,
    pub(crate) effort: f32,
    pub(crate) hint: f32,
    /// HALT (0: not shown) and the primary button.
    pub(crate) halt: f32,
    pub(crate) primary: f32,
    pub(crate) tight: bool,
}

/// Gaps (design.md §8.11): 8 inside a part, 12 between name and word, 16
/// between parts, 24 before each button.
const FOOT_PART: f32 = 16.;
const FOOT_BUTTON: f32 = 24.;
/// The least the hint keeps before it goes (it ellipsizes down to this).
const FOOT_HINT_MIN: f32 = 60.;

/// The foot fitted to its row (design.md §8.11), measured like
/// [`crate::fit::fit_top`]: while the parts are wider than the row they
/// drop in this order: the hint (counted at its least), the effort, the
/// model or facts, the state word and its count (also gone in a tight
/// pane), the name and handle. The glyph, MAIN, HALT and the primary never
/// drop. The effort is said after the model (`model · effort`): it goes
/// with it.
pub(crate) fn fit_foot(i: &FootIn) -> Foot {
    let mut f = Foot { hint: i.hint > 0., effort: i.effort > 0., model: i.model > 0., word: i.word > 0. && !i.tight, name: i.name > 0. };
    let width = |f: &Foot| {
        let mut w = i.lead + FOOT_BUTTON + i.primary;
        if i.halt > 0. {
            w += i.halt + FOOT_BUTTON;
        }
        if f.name {
            w += i.name;
        }
        if f.word {
            w += i.word + if f.name { 12. } else { 0. };
        }
        if f.model {
            w += FOOT_PART + i.model + if f.effort { i.effort } else { 0. };
        }
        if f.hint {
            w += FOOT_PART + i.hint.min(FOOT_HINT_MIN);
        }
        w
    };
    type Drop = fn(&mut Foot);
    let order: [Drop; 5] = [|f| f.hint = false, |f| f.effort = false, |f| f.model = false, |f| f.word = false, |f| f.name = false];
    for drop in order {
        if width(&f) <= i.room {
            break;
        }
        drop(&mut f);
    }
    f.effort &= f.model;
    f
}

/// The hold-back after a layout ([`AppView::keep_rows`]): whether the list,
/// grown to its end while scrolled back, is paused short of it now. It was
/// not following when the hold was armed, so following now is GPUI's
/// re-arm: paused, whatever is pending. Otherwise the hold is spent on the
/// first layout with no row's parse pending: a provisional one (`pending`)
/// neither fills up to the end nor re-arms, and spent there, the next
/// would follow. `pending` also counts rows GPUI did not wait for (above
/// the view), so it can re-arm with a row pending.
pub(crate) fn held_back(hold: &Cell<bool>, pending: bool, following: bool) -> bool {
    let pause = hold.get() && following;
    if pause || !pending {
        hold.set(false);
    }
    pause
}

/// The transcript list's bottom padding: its end sits this far over the
/// composer (the scroller's end is past it).
fn list_pad_b(m: &Metrics) -> f32 {
    (28. * m.r).round()
}

/// Whether list `l` (of `n` rows) has rows to hint ([`AppView::hint_rows`]):
/// more rows than fit, one with no height, the last one drawn.
fn wants_hint(l: &ListState, n: usize) -> bool {
    n > 0
        && l.item_count() == n
        && l.max_offset_for_scrollbar().y > px(0.)
        && l.is_scrolled_to_end().is_none()
        && l.bounds_for_item(n - 1).is_some()
}

/// How far down the transcript's list starts: the top bar and the naming
/// row when it is open (the tab strip is the same in every form; nothing
/// else sits over the list: design.md §8.14).
pub(crate) fn above_list(lay: Layout, naming: bool) -> f32 {
    let naming = if naming { crate::fit::naming_h(lay.form()) } else { 0. };
    crate::chrome::top_h(lay.short) + naming
}

/// When the card's keys may act ([`ARM`] after each of these): its buttons
/// from when it was shown; its keys also from when they last went live
/// (the card came into view, the window or the keys came to it) and from
/// the last Ctrl+Enter or Esc that came too early or was not a new press
/// ([`CardChord`]). That is the TUI's rule: every key that reaches an
/// unarmed card starts the wait again. A key held from before never
/// answers, however long after the card its first repeat comes.
///
/// The buttons do not wait again when the card comes back into view (from
/// Settings, an overlay, another window): a click is aimed, and the shift
/// of the layout the wait guards against happened when the card came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Arm {
    shown: Instant,
    keys: Instant,
}

impl Arm {
    pub(crate) fn new(now: Instant) -> Self {
        Arm { shown: now, keys: now }
    }

    /// The buttons may act.
    pub(crate) fn armed(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.shown) >= ARM
    }

    /// When the card was shown (its answer's wait counts from it).
    pub(crate) fn shown_at(&self) -> Instant {
        self.shown
    }

    /// The keys may act.
    pub(crate) fn keys_armed(&self, now: Instant) -> bool {
        self.armed(now) && now.saturating_duration_since(self.keys) >= ARM
    }

    /// The keys went live: they wait from `now`.
    pub(crate) fn restart(&mut self, now: Instant) {
        self.keys = now;
    }

    /// A Ctrl+Enter or Esc at `now` (`press`: a new press, not a repeat
    /// or a key still down, [`CardChord`]): true if it may answer; if not,
    /// the keys wait from `now`.
    pub(crate) fn key(&mut self, now: Instant, press: bool) -> bool {
        let ok = press && self.keys_armed(now);
        if !ok {
            self.keys = now;
        }
        ok
    }
}

pub(crate) use crate::authorize::{CardExit, CardLook, PromptCard};

/// A well's hidden height: what is still below its view (`overflow`) and
/// what of it is under the window's bottom.
pub(crate) fn hidden(overflow: f32, bottom: f32, view: f32) -> f32 {
    overflow.max(0.) + (bottom - view).max(0.)
}

/// What a key does to the authorization card while the window has the
/// focus (nothing typed in does): Ctrl+Enter (Cmd+Enter on macOS) answers
/// yes, Esc no, Page Down / Page Up / Down / Up scroll its well. A bare
/// Enter does nothing: the window has the focus after a launch, a click
/// outside a field or Settings, and an Enter meant for the composer must
/// not run a tool. None of them is an app binding, and only the window's
/// own focus sees them: the composer, find and every field keep theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardKey {
    Yes,
    No,
    Scroll { down: bool, page: bool },
}

pub(crate) fn card_key(ks: &Keystroke) -> Option<CardKey> {
    card_key_on(ks, cfg!(target_os = "macos"))
}

/// [`card_key`] on macOS (`mac`) or not: Cmd there, Ctrl elsewhere (Super
/// is the desktop's on Linux).
pub(crate) fn card_key_on(ks: &Keystroke, mac: bool) -> Option<CardKey> {
    let m = &ks.modifiers;
    let bare = !(m.shift || m.alt || m.control || m.platform || m.function);
    let primary = if mac { m.platform && !m.control } else { m.control && !m.platform };
    match ks.key.as_str() {
        "enter" if primary && !(m.shift || m.alt || m.function) => Some(CardKey::Yes),
        "escape" if bare => Some(CardKey::No),
        k @ ("pagedown" | "pageup" | "down" | "up") if bare => {
            Some(CardKey::Scroll { down: matches!(k, "pagedown" | "down"), page: k.starts_with("page") })
        }
        _ => None,
    }
}

/// AUTHORIZE may act: armed, and the whole body was on screen (it fits, or
/// was scrolled to its end). `below`: the well's hidden height, `None`
/// before its first layout.
pub(crate) fn authorize_ready(armed: bool, below: Option<f32>, read: bool) -> bool {
    armed && below.is_some_and(|b| b <= 1. || read)
}

/// What a secret card sends: the typed answer to `submit` (never an empty
/// one: Enter in an empty field, a second Enter after the first moved the
/// focus there, a held Enter after the answer went out, SUBMIT with
/// nothing typed: none of them sends), or nothing, to cancel. `None`: send
/// nothing at all.
pub(crate) fn secret_answer(submit: bool, typed: &str) -> Option<Option<String>> {
    match submit {
        true if typed.is_empty() => None,
        true => Some(Some(typed.to_string())),
        false => Some(None),
    }
}

/// One frame's composer work ([`Drafts::frame`]) and the notes with it
/// (the draft's, a worker key's or a refused steer's): a worker note goes
/// only when the composer becomes another tab's. Input given back into
/// the composer's own tab (a refused steer, its view gone) keeps the note
/// that says where Enter now sends it.
fn frame_composer(
    drafts: &mut Drafts,
    owner: &mut Option<(u64, Option<String>)>,
    tabs: &crate::tabs::Tabs,
    given_back: Option<String>,
    (draft_note, watch_note): (&mut Option<&'static str>, &mut Option<SharedString>),
    composer: (&str, usize),
) -> Option<(String, Option<usize>)> {
    if !Drafts::composer_owned(owner.as_ref(), tabs) {
        *watch_note = None;
    }
    drafts.frame(owner, tabs.composer_tab(), given_back, draft_note, composer, |s| tabs.knows(s))
}

/// A call's input with more than one field (a `bash` with its `timeout`)
/// as its row says it: its `command`, not its JSON (one field is shown
/// bare already).
/// A call's input as JSON, when it is an object (not a cut preview).
pub(crate) fn input_of(input: &str) -> Option<serde_json::Value> {
    input.starts_with('{').then(|| serde_json::from_str::<serde_json::Value>(input).ok()).flatten()
}

fn command_of(input: Option<&serde_json::Value>) -> Option<String> {
    match input?.get("command")? {
        serde_json::Value::String(c) => Some(c.clone()),
        _ => None,
    }
}

/// How many of the oldest rows a change from the start dropped: where the
/// first item left sits among the rows built (0: none dropped, or another
/// transcript).
pub(crate) fn front_trimmed(rows: &[RowCache], items: &[Item]) -> usize {
    items.first().and_then(|first| rows.iter().position(|r| r.id == first.id)).unwrap_or(0)
}

/// A key reached the window at `now`, the last secret card having gone
/// (or the last key come) at `left`: the typing meant for it goes on if
/// that was less than [`ARM`] ago (then from `now`).
pub(crate) fn still_typing(left: Option<Instant>, now: Instant) -> Option<Instant> {
    left.filter(|&t| now.saturating_duration_since(t) < ARM).map(|_| now)
}

/// Lines of the well (`line` px each) still below its view.
pub(crate) fn lines_below(below: f32, line: f32) -> usize {
    (below / line).ceil() as usize
}

/// What the picker says after a save.
fn saved_note(saved: Option<(std::io::Result<()>, std::path::PathBuf)>, overridden: &str) -> SharedString {
    match saved {
        Some((Ok(()), path)) => format!("saved to {}{overridden}", path.display()).into(),
        Some((Err(e), _)) => format!("not saved: {e}").into(),
        None => "not saved: no config folder".into(),
    }
}

/// What the well shows of a prompt: all of it, as sent (blank lines and
/// indentation kept; long lines wrap, nothing is cut).
pub(crate) fn well_body(prompt: &str) -> SharedString {
    SharedString::from(prompt)
}

/// A favourites write shared by the window and its background job: the job
/// writes holding the lock and leaves what it wrote here, so the window
/// takes it ([`crate::favourites::Favourites::written`]) whether or not the
/// task that started it is still there.
#[derive(Default)]
struct FavsJob {
    /// The window closed: a job not begun yet writes nothing.
    closed: bool,
    /// The generation written, the list sent, the file's list after.
    done: Option<(u64, Vec<crate::favourites::Favourite>, Vec<crate::favourites::Favourite>)>,
}

impl FavsJob {
    /// `sent` merged into the file over `base` ([`theme::save_favourites`]),
    /// unless the window closed first.
    fn write(
        slot: &std::sync::Mutex<FavsJob>,
        path: &std::path::Path,
        key: &str,
        base: &[crate::favourites::Favourite],
        sent: Vec<crate::favourites::Favourite>,
        generation: u64,
    ) -> std::io::Result<()> {
        let mut job = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if job.closed {
            return Ok(());
        }
        let file = theme::save_favourites(path, key, base, &sent)?;
        job.done = Some((generation, sent, file));
        Ok(())
    }
}

/// Each worker chip's bounds (its worker's id) as last laid out.
pub(crate) type ChipBounds = Vec<(u64, Bounds<Pixels>)>;

pub struct AppView {
    pub client: Option<Client>,
    /// Connection, sessions, attachment and transcript. Events are applied
    /// to it as they arrive (the pump), never queued for `render`.
    pub model: Model,
    /// Per transcript item: its strings ([`RowCache`]).
    pub(crate) rows: Vec<RowCache>,
    /// The virtual list's rows ([`turns::build`]): turns, runs, folds.
    pub(crate) disp: Vec<Disp>,
    pub(crate) summaries: Summaries,
    pub list: ListState,
    /// Rows whose open/closed state the user flipped from its default.
    pub(crate) toggled: HashSet<u64>,
    /// Runs the user unfolded (by their first item's id).
    open_runs: HashSet<u64>,
    /// The rows an unfold under Full is revealing: tail-follow is held
    /// paused while they do ([`Self::hold_unfold`]).
    pub(crate) unfold_hold: Vec<u64>,
    /// The transcript's `busy` at the last [`Self::sync_list`]: when it
    /// falls, the last run may fold.
    synced_busy: bool,
    /// The files ledger's generation the rows' counts are from.
    files_gen: u64,
    /// The subagents' shape generation the rows were built at.
    subs_gen: u64,
    /// The session the rows are of.
    rows_session: Option<String>,
    /// The last call still unfinished: the one a prompt waits on.
    pub(crate) waiting_on: Option<u64>,
    /// The centre pane's column at the last frame.
    pub(crate) layout: Layout,
    /// What a rise of the list's top edge could not scroll (the list was
    /// nearer its start), and where the list was then ([`AppView::keep_rows`]).
    scroll_carry: Option<(f32, ListOffset)>,
    /// The list's view grew while it was scrolled back: after this
    /// frame's layout it must not have started following the tail.
    hold_back: Rc<Cell<bool>>,
    /// The markdown rows' text views, and whose parse is pending ([`turns::TextRows`]).
    pub(crate) texts: Rc<RefCell<turns::Views>>,
    /// Every pane's scrollbar ([`crate::scrollbar`]).
    pub(crate) bars: crate::scrollbar::Bars,
    pub(crate) composer: Entity<TextareaState>,
    /// The rows the composer's text grows to ([`crate::fit::composer_rows`]).
    pub(crate) composer_rows: usize,
    /// The top bar's words, measured at the first frame.
    pub(crate) top_words: Option<crate::fit::TopWords>,
    /// The find field shows its placeholder's first word only (a rail too
    /// narrow for all of it: [`crate::chrome::find_fits`]).
    find_short: bool,
    /// The rail's find (`/`; Enter opens by exact name) and "Name this
    /// session…" (the naming row's) inputs.
    pub(crate) find_input: Entity<InputState>,
    name_input: Entity<InputState>,
    /// The accent picker's custom colour and a secret prompt's answer.
    pub(crate) hex_input: Entity<InputState>,
    pub(crate) secret_input: Entity<InputState>,
    /// The find text, trimmed and lowercased.
    pub(crate) find: String,
    /// The count chip filtering the rail.
    pub(crate) chip: Option<Chip>,
    /// HISTORY buckets showing all their rows (`+ N MORE`; not saved).
    pub(crate) expanded: u8,
    /// The rail's virtual list (only the visible rows are built).
    pub(crate) rail_list: ListState,
    pub(crate) rail_width: f32,
    /// The rail row under the pointer, the one whose readout is open, and
    /// the 400 ms wait in between.
    hover: Option<u64>,
    pub(crate) readout: Option<u64>,
    readout_wait: Option<Task<()>>,
    /// When the readout came (its enter; none: it followed another, fluid.md
    /// §3.10), the one leaving and since when, the row it follows from (its
    /// top in the rail's content), and the last one left: its row, when.
    pub(crate) readout_at: Option<Instant>,
    pub(crate) readout_leaving: Option<(u64, Instant)>,
    pub(crate) readout_from: Option<f32>,
    readout_left: Option<(u64, Instant)>,
    /// The rail's motion this frame ([`AppView::rail_motion`]).
    pub(crate) rail_fx: crate::chrome::RailFx,
    /// A favourited row's star: when it was favourited, and when its row
    /// landed (the hop's start, polish.md P6).
    pub(crate) star_land: std::cell::RefCell<HashMap<u64, (Instant, Option<Instant>)>>,
    /// The bench's life scene (`SYNAPS_BENCH_LIFE`): its local session is
    /// this window's to drive, as a user's own session is (its turn's end
    /// hands the floor back: Handoff in and the settle pass, polish.md P3).
    pub(crate) owner_here: bool,
    /// Tests: a count with a plural in the top bar, rolling as the rarer
    /// counts do (the word swap's UI test: no count left rolling has one).
    #[cfg(test)]
    pub(crate) roll_probe: Option<usize>,
    /// The side panels folded to strips and the open HISTORY buckets
    /// (saved with the accent).
    pub panels: theme::Panels,
    /// Panel keys changed since the last save began, and keys handed to a
    /// save that has not finished: both written at once on a clean quit.
    panels_dirty: theme::PanelKeys,
    panels_writing: theme::PanelKeys,
    /// The delayed save ([`SAVE_DELAY`]); a newer fold replaces it.
    panels_save: Option<Task<()>>,
    /// A drag of a panel's edge, from the mouse-down on its handle.
    pub(crate) drag: Option<EdgeGrab>,
    /// A drag let go near a detent, its panel springing there: the
    /// transcript's thumb holds until it lands, as through the drag (the
    /// gate's S6).
    springback: bool,
    /// The side panels' widths on their springs, and the column's wrap
    /// width while they move (fluid.md §3.8).
    pub(crate) panel_fx: crate::chrome::PanelFx,
    fold_keys: FoldKeys,
    /// The window's width at the last frame (what a fold acts on).
    pub(crate) width: f32,
    /// Its height (the picker fits under it).
    pub(crate) height: f32,
    pub picker_open: bool,
    pub(crate) picker_note: Option<SharedString>,
    /// Where the mouse-down that closed the picker from outside was: the
    /// click it starts must not reopen it on its trigger, however long it
    /// is held.
    pub(crate) picker_dismissed: Option<Point<Pixels>>,
    /// Where the launch accent and theme came from (a flag or env wins
    /// over the picker's file).
    pub accent_source: theme::Source,
    pub theme_source: theme::Source,
    /// No theme is picked: the Synaps config's is shown (the picker's
    /// SYNAPS CONFIG entry), read again from the profile's config.
    pub theme_follows_config: bool,
    pub theme_profile: Option<String>,
    /// What following the config shows now (read when the picker opens).
    pub(crate) config_theme: SharedString,
    /// The authorization card's prompt (see [`PromptCard`]).
    pub(crate) card: Option<PromptCard>,
    /// Its keys were live at the last frame ([`AppView::card_keys_live`]):
    /// when they come back, they wait again ([`Arm`]).
    keys_live: bool,
    /// Enter and Esc, down and up ([`CardChord`]), and whether the last
    /// of them to go down was a new press.
    chord: CardChord,
    chord_press: bool,
    /// A secret card went away (answered elsewhere, re-delivered, another
    /// tab) at this time, or a key reached the window since, within
    /// [`ARM`] of the last: what is still being typed was meant for it,
    /// so `/` does not open find ([`AppView::text_keys_blocked`]).
    secret_left: Option<Instant>,
    /// The naming row was open at the last layout ([`AppView::keep_rows`]
    /// counts it as that layout had it).
    layout_naming: bool,
    /// The running dot's step, and the 2 Hz timer while a turn runs.
    pub(crate) rail: crate::chrome::RailCache,
    pub(crate) focus: FocusHandle,
    /// Refused input to put back in the composer (needs a `Window`: done
    /// at the next frame).
    restore: Option<String>,
    /// A slash command the composer sent (its name, its text): the text
    /// stays in the composer until the daemon has run it.
    cmd_draft: Option<(String, String)>,
    /// That text, run: out of the composer if it still holds it.
    cmd_clear: Option<String>,
    /// The note last set by a command that went out or was answered: shown
    /// quiet (`dim`), not as a warning, while it is the note on screen.
    pub(crate) quiet_note: Option<SharedString>,
    /// Each tab's composer draft by session (kept when its tab is left,
    /// put back when it is shown again), and the tab whose draft the
    /// composer holds.
    drafts: Drafts,
    composer_of: Option<(u64, Option<String>)>,
    /// Said in the composer's hint until the next send or switch (a draft
    /// came back with a send the daemon did not take).
    draft_note: Option<&'static str>,
    /// Bumped when the text metrics change every row's height (a cached
    /// tab's list is re-measured when it comes back).
    metrics_gen: u64,
    pub bench: Option<BenchState>,
    /// Diagnostics: render rows as plain text instead of markdown.
    pub plain_rows: bool,
    /// The synthetic transport feeds this window (`--offline`); it answers
    /// `saved_sessions` itself, on this channel.
    pub(crate) synthetic: bool,
    pub(crate) synthetic_tx: Option<async_channel::Sender<Event>>,
    synthetic_seq: u64,
    /// `--session ID`: open it once the first session list lands.
    pub start_session: Option<String>,
    /// `--offline`: the session opened at start when no tab is restored.
    default_session: Option<String>,
    /// The tabs to reopen once the first session list lands (gone
    /// sessions drop, with a notice).
    pub restore_tabs: Option<Saved>,
    /// Where the tabs are saved in the settings file (per daemon); `None`:
    /// not saved (bench).
    pub tabs_key: Option<String>,
    /// The tabs generation handed to a save, the one written (or meant to
    /// stay unwritten), and the delayed save.
    tabs_saved: u64,
    tabs_written: u64,
    tabs_save: Option<Task<()>>,
    /// The same for the favourites (saved under `tabs_key` too), and the
    /// rail generation their snapshots were last brought up to.
    favs_saved: u64,
    favs_written: u64,
    favs_save: Option<Task<()>>,
    /// The favourites write on its way, shared with its background job.
    favs_job: Arc<std::sync::Mutex<FavsJob>>,
    /// A favourites job is between taking the list and its result being
    /// taken (the window's own: never a wait on the job's lock).
    favs_writing: bool,
    favs_synced: Option<u64>,
    /// The session lists the restored tabs have been checked against.
    lists_seen: u64,
    /// The background tabs' sessions as last seen ([`TAB_CACHE`]).
    pub(crate) tab_cache: Lru<String, TabCache>,
    /// The tab (and its session) the model and the list hold now.
    pub(crate) shown_tab: Option<(u64, Option<String>)>,
    /// A switch's attach waiting for the gate, and its timer.
    attach_wait: Option<(String, Option<ResumeRequest>, bool)>,
    attach_due: Option<Task<()>>,
    gate: crate::tabs::AttachGate,
    /// Tells Ctrl+T / Ctrl+W / Ctrl+Shift+T presses from their repeats.
    tab_keys: FoldKeys,
    /// A once-per-press key whose action came and whose key-down has not
    /// yet ([`OnceKey`]).
    once: Option<OnceKey>,
    /// The tab strip's first shown tab when they overflow, and the tab it
    /// last kept in view.
    pub(crate) tab_first: usize,
    pub(crate) tab_follow: Option<u64>,
    /// How many tabs the strip had room for at the last frame.
    pub(crate) tab_room: usize,
    pub(crate) tab_labels: crate::tabstrip::Labels,
    /// What the strip's motion remembers ([`crate::tabstrip::TabFx`]).
    pub(crate) tab_fx: crate::tabstrip::TabFx,
    /// The synthetic daemon's attach replies.
    pub(crate) attach_source: Option<AttachSource>,
    /// Settings and Usage ([`crate::views`]): their state, the dash channel
    /// they read, this app's own new settings, the gear's menu (and where
    /// the mouse-down that closed it from outside was), and the view on
    /// screen at the last frame.
    pub(crate) settings: crate::settings_view::SettingsUi,
    pub(crate) usage: crate::usage_view::UsageUi,
    pub(crate) dash: crate::views::DashUi,
    pub(crate) app_settings: crate::settings_view::AppSettings,
    pub(crate) settings_find: Entity<InputState>,
    pub(crate) settings_input: Entity<InputState>,
    pub(crate) gear_menu: bool,
    /// Overlays as they leave (fluid.md §3.10): what was open last frame,
    /// and what closed and when, drawn without a hitbox until gone; the
    /// gear's menu: when it opened, and when it closed.
    menu_was: Option<crate::menu::Menu<crate::wtabs::Act>>,
    pub(crate) menu_leaving: Option<(crate::menu::Menu<crate::wtabs::Act>, Instant)>,
    smenu_was: Option<crate::menu::Menu<SessionAct>>,
    smenu_leaving: Option<(crate::menu::Menu<SessionAct>, Instant)>,
    pub(crate) wmenu_was: Option<crate::menu::Menu<u64>>,
    pub(crate) wmenu_leaving: Option<(crate::menu::Menu<u64>, Instant)>,
    pub(crate) gear_at: Option<Instant>,
    pub(crate) gear_leaving: Option<Instant>,
    gear_was: bool,
    pub(crate) gear_dismissed: Option<Point<Pixels>>,
    shown_view: Option<crate::tabs::Kind>,
    shown_views: usize,
    /// The panes Settings and Usage draw in (cached: [`crate::views::ViewPane`]),
    /// their width, what they read of the rest of the app at the last frame
    /// ([`AppView::view_inputs`] and the theme), and whether the window is
    /// in front (the dash poll waits while it is not).
    pub(crate) settings_pane: Entity<crate::views::ViewPane>,
    pub(crate) usage_pane: Entity<crate::views::ViewPane>,
    pub(crate) view_width: f32,
    /// Where each segmented control's segments were laid out last frame
    /// (their bar springs between them).
    pub(crate) seg_places: std::rc::Rc<RefCell<HashMap<u64, (f32, f32)>>>,
    /// The link's signatures (motion.md §5.1, §5.3), and the top bar's
    /// connection cell's tear offset this frame.
    pub(crate) link: crate::motion::Link,
    pub(crate) conn_dy: f32,
    /// The connected words' width (chars, mono), held through a reload.
    pub(crate) conn_held: std::cell::Cell<usize>,
    /// What the composer's bar and the Scan's line were last drawn in (tests).
    #[cfg(test)]
    pub(crate) composer_bar: std::cell::Cell<Option<Hsla>>,
    #[cfg(test)]
    pub(crate) scan_line: Option<Hsla>,
    /// A worker tab opened this frame (its id), the tracer to it, and where
    /// the tab strip was painted last.
    pub(crate) tracer_open: Option<u64>,
    pub(crate) tracer: Option<crate::motion::Tracer>,
    pub(crate) strip_at: std::rc::Rc<std::cell::Cell<Point<Pixels>>>,
    /// The composer's frame's top as last laid out (the palette ends
    /// above it).
    pub(crate) composer_top: std::rc::Rc<std::cell::Cell<f32>>,
    /// The top of what is drawn over the composer (the stub, a notice, the
    /// worker chips) as last laid out; `INFINITY` while none is.
    pub(crate) over_composer: std::rc::Rc<std::cell::Cell<f32>>,
    /// The dock's (stub, notice, chips) and the composer's field's heights
    /// as last laid out: their boxes spring to them (polish.md P13, P14).
    pub(crate) dock_h: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) composer_h: std::rc::Rc<std::cell::Cell<f32>>,
    /// The session and column they were measured in: another's is not a
    /// change (it is drawn at its height at once).
    dock_scope: u64,
    /// Each worker chip's bounds as last laid out (the tracer starts from
    /// the source chip's bar).
    pub(crate) chip_bounds: Rc<RefCell<ChipBounds>>,
    view_seen: Option<(u64, theme::Choice)>,
    pub(crate) window_active: bool,
    /// The session's list while a worker's view is shown, and the worker
    /// whose rows the list holds (`None`: the session's).
    main_pane: Option<Pane>,
    /// The worker's view the list holds and its generation
    /// ([`crate::watch::Watch::focus_key`]).
    pane_of: Option<(String, u64)>,
    /// Worker tabs' lists (rows, folds, heights, scroll) while another
    /// thread of their session is on screen, by view and generation: back
    /// as they were left, while the tab is open.
    worker_panes: Vec<((String, u64), Pane)>,
    /// The next sync builds every row again (a worker's view came on).
    full_rebuild: bool,
    /// The watch poll's timer and when it fires ([`AppView::arm_watch_poll`]).
    watch_poll: Option<(Instant, Task<()>)>,
    /// Frames drawn for the window are shown (GPUI's visibility: mapped and
    /// not wholly covered, not minimized). Nothing polls while it is not.
    visible: bool,
    /// The window's motion (motion.md): the frame's clock, what animates,
    /// the one timer.
    pub(crate) fx: Rc<crate::motion::Motion>,
    /// The transcript's own scrolling (fluid.md §3.2).
    pub(crate) scroller: crate::scroll::Scroller,
    /// The column on screen (the session its rows are, the worker view)
    /// and its tab's id, and when it last changed; the tab's place then and
    /// whether it was a worker's view; the layer it arrives as: since, and
    /// from where (fluid.md §3.1, §3.6).
    column_key: (Option<String>, u64, Option<u64>),
    column_at: Option<Instant>,
    column_ix: Option<usize>,
    column_worker: bool,
    /// The transcript's body line this frame (`theme::metrics`), for the rows.
    body_line: f32,
    pub(crate) layer: Option<(Instant, f32, f32)>,
    /// The prompt on the card came while watched, at this instant: its frame
    /// traces, its glyph breathes (motion.md §6.3).
    pub(crate) card_at: Option<Instant>,
    /// The answered card collapsing in its row ([`CardExit`]).
    pub(crate) card_exit: Option<CardExit>,
    /// Answered prompts' receipts, by session ([`crate::authorize::Receipt`]).
    pub(crate) receipts: HashMap<String, Vec<crate::authorize::Receipt>>,
    /// The answer this window sent to the card, until its prompt resolves.
    pub(crate) sent: Option<crate::authorize::Sent>,
    /// This frame's [`AppView::card_keys_live`] (the card's row reads it).
    pub(crate) card_keys: Cell<Option<bool>>,
    /// Ctrl+J from a worker's view or a pane: reveal this card (its
    /// [`Model::card_key`]) once its row is built; dropped when that card
    /// goes (Shady M5: a later card never takes the keys from the composer).
    pub(crate) jump_card: Option<(u64, u64)>,
    /// Ctrl+J went to this card (its [`Model::card_key`]): its NEEDS YOU
    /// stub leaves at once, not once the card is on screen, so the jump
    /// aims at the view as it will be and the card moves one way. Cleared
    /// once the card is on screen (scrolled away again, the stub returns).
    pub(crate) stub_away: Option<(u64, u64)>,
    /// The session this window last sent HALT to, and when: a prompt that
    /// resolves after it was cancelled here, not answered elsewhere.
    pub(crate) halted: Option<(String, Instant)>,
    /// The card's Allow all box: Tab from the card's keys focuses it (a
    /// click too); Space toggles it then only.
    pub(crate) allow_all_focus: FocusHandle,
    /// When each recent prompt was first shown here, by session, id and
    /// `raised_at` (Shady N4: a tab's round trip builds its card anew, and
    /// "how long you took" counts from the first showing).
    pub(crate) card_first: Vec<(PromptAt, Instant)>,
    /// The list's view (top, bottom) as its last layout left it.
    pub(crate) list_box: Cell<(f32, f32)>,
    /// The card's row was on screen, buttons whole, at the frame's start.
    pub(crate) card_seen: Cell<bool>,
    /// Presses the live card took (tests: a leaving card takes none).
    #[cfg(test)]
    pub(crate) card_presses: u32,
    /// What the card's last frame showed ([`CardLook`]).
    pub(crate) card_look: std::cell::RefCell<CardLook>,
    /// The card was on screen last frame (not behind a worker's view,
    /// Settings or Usage): one coming back waits again ([`Arm`]).
    card_was_up: bool,
    /// The tray's rows as last built, and the timer that drops a finished
    /// worker's ([`crate::watch::FLASH`]).
    pub(crate) tray: crate::tray::TrayCache,
    pub(crate) tray_due: Option<(Instant, Task<()>)>,
    /// Lines of worker chips this frame ([`crate::fit::chip_lines`]), and
    /// how the chips flow in them.
    pub(crate) tray_rows: usize,
    pub(crate) chips: crate::tray::Chips,
    /// Said in the composer's hint until the next key that moves or sends:
    /// why a worker key went nowhere, or why a steer was not sent.
    pub(crate) watch_note: Option<SharedString>,
    /// The worker whose name and state the composer's placeholder has
    /// (`None`: the session's placeholder).
    placeholder_of: Option<(String, String, bool)>,
    /// A worker's context menu and the worker it is for ([`crate::wtabs`]),
    /// and the timer that ends an armed cancel's wait.
    pub(crate) menu: Option<(crate::menu::Menu<crate::wtabs::Act>, crate::wtabs::Target)>,
    /// What had the focus when the menu opened (it takes the keys while
    /// open): given back once it closes, if nothing else took it.
    pub(crate) menu_focus_back: Option<FocusHandle>,
    /// The palette, open (polish.md P16), its query field, what had the
    /// focus when it opened (given back when it closes), the row its quad
    /// was last drawn on, its list's scroll, and the lists it was built
    /// from ([`AppView::refresh_palette`]).
    pub(crate) palette: Option<crate::palette::Palette>,
    pub(crate) palette_input: Entity<InputState>,
    pub(crate) palette_back: Option<FocusHandle>,
    pub(crate) palette_sel: Cell<Option<usize>>,
    pub(crate) palette_scroll: ScrollHandle,
    pub(crate) palette_gen: u64,
    /// The palette's list height and quad as last drawn, and their targets.
    #[cfg(test)]
    pub(crate) palette_drawn: Cell<(f32, f32, f32, f32)>,
    /// The palette box's top, its tallest bottom, its left and width as last drawn.
    #[cfg(test)]
    pub(crate) palette_box: Cell<(f32, f32, f32, f32)>,
    pub(crate) cancel_tick: Option<Task<()>>,
    /// A session's menu (a right-click on its tab or rail row): Rename…,
    /// Close tab.
    pub(crate) smenu: Option<(crate::menu::Menu<SessionAct>, SessionTarget)>,
    /// The workers menu (`+N more`'s): its items' acts are worker ids.
    pub(crate) wmenu: Option<crate::menu::Menu<u64>>,
    /// A rename asked for a session not on screen yet (and when): its
    /// naming row opens once it is attached, if that is soon and nothing
    /// else was shown meanwhile ([`RENAME_WAIT`]); else it is forgotten.
    pub(crate) rename_pending: Option<(String, Instant)>,
    /// HALT armed by a first Esc in the composer ([`ARM`]): a second
    /// halts. Its label is `err` while armed; the timer ends the arm, as
    /// does anything else shown (another tab, session or view).
    pub(crate) halt_armed: Option<Instant>,
    halt_tick: Option<Task<()>>,
    /// Where HALT was armed ([`AppView::halt_place`]).
    halt_where: Option<(Option<u64>, Option<String>, bool)>,
    /// The Esc on its way had an IME composition to end: the composition's,
    /// not HALT's. Read before the kit's `Escape` ends it (the root's
    /// action capture runs first).
    esc_composing: bool,
    /// The composer's placeholder was set for a cramped window (its key
    /// hints dropped: the field is short).
    placeholder_cramped: bool,
    /// An item was picked from the keyboard: that key's repeats, once the
    /// menu is gone, are not the next thing's ([`AppView::menu_keydown`]).
    /// The root's capture listener stops them, but GPUI runs a focused
    /// input's key bindings before any key listener: its Enter has already
    /// emitted `PressEnter` by then. That event is a deferred effect, so it
    /// reaches the subscriptions after the listener has set this flag for
    /// a repeat (cleared it for a new press): each one checks it
    /// ([`AppView::enter_is_the_picks`]).
    pub(crate) menu_picked: bool,
    /// A click opened a worker's tab: until then, a mouse-down that is the
    /// rest of its double-click goes nowhere ([`crate::wtabs::MULTI_CLICK`]).
    pub(crate) swallow_clicks: Option<Instant>,
    /// Display caps' widths ([`AppView::display_w`]), measured once.
    pub(crate) widths: RefCell<std::collections::HashMap<(&'static str, u32), f32>>,
    /// What reached [`AppView::submit`], and whether a worker's view was
    /// on screen (a steer): the UI tests' record of what would have gone.
    #[cfg(test)]
    pub(crate) submits: Vec<(String, bool)>,
    /// HALTs sent ([`AppView::halt`]): the UI tests' record.
    #[cfg(test)]
    pub(crate) halts: usize,

    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

/// A text run in the display face: caps, tracked, semibold.
impl AppView {
    /// Control `key` answers the pointer (fluid.md §3.9): its hover eases
    /// in and out ([`crate::motion::Motion::set_hover`]) and a press shows
    /// in the next frame ([`crate::motion::Motion::press_down`]). Draw it
    /// with [`crate::motion::Motion::fill`]. One `on_hover` per element:
    /// an element with its own calls `set_hover` from it instead.
    pub(crate) fn touch<E: StatefulInteractiveElement>(el: E, key: u64, cx: &Context<Self>) -> E {
        let el = el.on_hover(cx.listener(move |this, on: &bool, _, cx| {
            this.fx.set_hover(key, *on);
            cx.notify();
        }));
        Self::pressable(el, key, cx)
    }

    /// A control's hover fill (fluid.md §3.9): on `ground`, toward `hover`
    /// as the pointer comes (over `hover_in`) and back as it goes (over
    /// `hover_out`); held, `press`. Where `.hover(|d| d.bg(hover))` was.
    pub(crate) fn lit(&self, el: Stateful<Div>, key: u64, roles: (Hsla, Hsla, Hsla), cx: &Context<Self>) -> Stateful<Div> {
        self.lit_rest(el, key, roles, None, cx)
    }

    /// [`AppView::lit`] for a control with a fill of its own at rest (a
    /// selected picker row's `raised`): its hover starts from that, not
    /// from the ground.
    pub(crate) fn lit_rest(
        &self,
        el: Stateful<Div>,
        key: u64,
        (ground, hover, press): (Hsla, Hsla, Hsla),
        rest: Option<Hsla>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let (fill, _) = self.fx.fill(key, ground, rest, hover, press);
        Self::touch(el.when_some(fill, |d, c| d.bg(c)), key, cx)
    }

    /// [`AppView::touch`]'s press half alone.
    pub(crate) fn pressable<E: InteractiveElement>(el: E, key: u64, cx: &Context<Self>) -> E {
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.fx.press_down(key);
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.fx.press_up() {
                    cx.notify();
                }
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.fx.press_up() {
                    cx.notify();
                }
            }),
        )
    }
}

/// A control's key for [`AppView::touch`]: its name and index, hashed
/// into the hover space.
pub(crate) fn tkey(name: &str, n: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (name, n).hash(&mut h);
    (0x7AD0 << 48) | (h.finish() & 0xFFFF_FFFF_FFFF)
}

pub(crate) fn display(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font_family(fonts::DISPLAY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_px(size)
        .text_color(color)
        .whitespace_nowrap()
        .child(text.into())
}

/// A count as text, drawn every frame: no allocation under 10.
pub(crate) fn count(n: usize) -> SharedString {
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    DIGITS.get(n).map_or_else(|| n.to_string().into(), |&d| d.into())
}

pub(crate) fn mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(fonts::MONO).text_px(size).text_color(color).whitespace_nowrap().child(text.into())
}

/// A section header: display caps plus the Japanese sub-label.
pub(crate) fn section(title: &'static str, jp: &'static str, p: &Palette) -> Div {
    h_flex()
        .items_baseline()
        .gap(px(10.))
        .child(display(title, fonts::size::D_S, p.header))
        .child(div().font_family(fonts::JP).text_px(fonts::size::JP).text_color(p.dim_ch).child(jp))
}

impl AppView {
    pub fn new(
        client: Option<(Client, async_channel::Receiver<Event>)>,
        synthetic: Option<Vec<Event>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(1, 6).submit_on_enter(true).placeholder(COMPOSER_PLACEHOLDER);
            s.set_scrollbar_renderer(Some(crate::scrollbar::editor()));
            s
        });
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::chrome::FIND.0));
        let palette_input = cx.new(|cx| InputState::new(window, cx).placeholder(crate::palette::PLACEHOLDER));
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("name: a-z, 0-9, '-'"));
        let hex_input = cx.new(|cx| InputState::new(window, cx).placeholder("#RRGGBB"));
        let secret_input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("answer (hidden)"));
        let settings_find = cx.new(|cx| InputState::new(window, cx).placeholder("find a setting"));
        let settings_input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { shift: false, .. } if !this.enter_is_the_picks() => {
                    let text = input.read(cx).value().trim().to_string();
                    if !text.is_empty() && this.submit(text) {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                }
                InputEvent::PressEnter { shift: false, .. } => {
                    log::debug!(target: "synaps_desktop::send", "enter: a held repeat of a menu pick's key, nothing sent");
                }
                // Typing: what a worker key or a refused steer said is old.
                InputEvent::Change if this.watch_note.is_some() => {
                    this.watch_note = None;
                    cx.notify();
                }
                _ => {}
            }),
            // The palette filters as it is typed.
            cx.subscribe_in(&palette_input, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::Change = ev
                    && let Some(p) = this.palette.as_mut()
                {
                    p.set_query(&input.read(cx).value());
                    this.palette_scroll.set_offset(Point::default());
                    cx.notify();
                }
            }),
            // Find filters as it is typed; Enter opens by exact name. The
            // typed name stays until the outcome is known: cleared on
            // `Attached` (`named.opened`, in `render`), kept on an error.
            cx.subscribe_in(&find_input, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| match ev {
                InputEvent::Change => {
                    let find = input.read(cx).value().trim().to_lowercase();
                    if find != this.find {
                        if this.find.is_empty() {
                            for fx in this.model.want_deep() {
                                this.effect(fx);
                            }
                        }
                        this.find = find;
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { secondary, .. } if !this.enter_is_the_picks() => {
                    let typed = input.read(cx).value().to_string();
                    this.find_enter(typed.trim(), if *secondary { Place::New } else { Place::Current });
                    cx.notify();
                }
                // The user is looking: list what find searches, fresh.
                InputEvent::Focus => {
                    for fx in this.model.looking() {
                        this.effect(fx);
                    }
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe_in(&name_input, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev
                    && !this.enter_is_the_picks()
                {
                    let typed = input.read(cx).value().to_string();
                    let req = this.model.rename_request(&typed);
                    this.rename(req);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&hex_input, window, |this: &mut Self, _input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev
                    && !this.enter_is_the_picks()
                {
                    this.apply_custom_accent(cx);
                }
            }),
            // Enter (or Ctrl/Cmd+Enter) submits what was typed, never
            // nothing; typing lights SUBMIT.
            cx.subscribe_in(&secret_input, window, |this: &mut Self, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } if !this.enter_is_the_picks() => this.answer_secret(true, window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
            cx.subscribe_in(&settings_find, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::Change = ev {
                    let text = input.read(cx).value().to_string();
                    this.settings_find(text, cx);
                }
            }),
            cx.subscribe_in(&settings_input, window, |this: &mut Self, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } if !this.enter_is_the_picks() => {
                    this.settings_commit(cx);
                    window.focus(&this.focus, cx);
                }
                // Clicked away: the edit is left as it was.
                InputEvent::Blur => {
                    let was = this.settings.editing.take();
                    if was.is_some() {
                        this.views_changed(cx);
                    }
                }
                _ => {}
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // M1: a focused input that leaves the tree (OPEN BY NAME in a folded
        // rail, a closed name bar, an answered secret prompt) leaves keys to
        // the window's root, above this view: the focus comes back here once
        // a frame has drawn without it. Until then the fold keys reach an
        // app-wide fallback, which also hands the focus back.
        subs.push(cx.on_focus_lost(window, |this, window, cx| window.focus(&this.focus, cx)));
        // The window went: a card's keys are not live. It came back: they
        // wait again ([`Arm`]), whether or not a frame was drawn meanwhile
        // (a hidden Wayland surface or an occluded macOS window draws none).
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.restart_card_keys(cx);
            } else {
                this.keys_live = false;
            }
            cx.notify();
        }));
        fold_fallback::<ToggleRail>(true, cx.weak_entity(), window.window_handle(), focus.clone(), cx);
        fold_fallback::<ToggleRight>(false, cx.weak_entity(), window.window_handle(), focus.clone(), cx);
        {
            let (v, w) = (cx.weak_entity(), window.window_handle());
            tab_fallback::<NewTab>(TabKey::New, v.clone(), w, focus.clone(), cx);
            tab_fallback::<CloseTab>(TabKey::Close, v.clone(), w, focus.clone(), cx);
            tab_fallback::<ReopenTab>(TabKey::Reopen, v.clone(), w, focus.clone(), cx);
            tab_fallback::<NextTab>(TabKey::Next, v.clone(), w, focus.clone(), cx);
            tab_fallback::<PrevTab>(TabKey::Prev, v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab1>(TabKey::Nth(0), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab2>(TabKey::Nth(1), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab3>(TabKey::Nth(2), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab4>(TabKey::Nth(3), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab5>(TabKey::Nth(4), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab6>(TabKey::Nth(5), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab7>(TabKey::Nth(6), v.clone(), w, focus.clone(), cx);
            tab_fallback::<Tab8>(TabKey::Nth(7), v.clone(), w, focus.clone(), cx);
            tab_fallback::<LastTab>(TabKey::Last, v, w, focus.clone(), cx);
        }
        // M3: a fold still waiting to be saved is written when the window
        // closes (the last one quits on Linux and Windows) or the app quits.
        subs.push(cx.on_release(|this, _| {
            this.flush_panels();
            this.flush_tabs();
            this.flush_favs();
            this.flush_app();
        }));
        subs.push(cx.on_app_quit(|this, _| {
            this.flush_panels();
            this.flush_tabs();
            this.flush_favs();
            this.flush_app();
            async {}
        }));
        // The dash poll waits while the window is in the background.
        subs.push(cx.observe_window_activation(window, |this, window, cx| this.window_activated(window.is_window_active(), cx)));
        // A worker's view polls only while the window shows.
        subs.push(cx.observe_window_visibility(window, |this, v, _, cx| this.visibility_changed(v.is_visible(), cx)));
        crate::views::view_fallbacks(cx.weak_entity(), cx);
        let me = cx.weak_entity();
        let settings_pane = cx.new(|_| crate::views::ViewPane::new(me.clone(), crate::tabs::Kind::Settings));
        let usage_pane = cx.new(|_| crate::views::ViewPane::new(me, crate::tabs::Kind::Usage));
        let inputs = Inputs {
            composer,
            find_input,
            palette_input,
            name_input,
            hex_input,
            secret_input,
            settings_find,
            settings_input,
            settings_pane,
            usage_pane,
            focus,
            // Not a tab stop (Shady M1 of round 4): the kit's own Tab order
            // would reach it from the composer. Only the card's Tab and a
            // click focus it.
            allow_all_focus: cx.focus_handle(),
            visible: window.is_visible(),
        };
        match (client, synthetic) {
            (Some((c, rx)), _) => {
                let mut v = Self::blank(inputs, Some(c), ConnState::Connecting, subs);
                v.pump(rx, cx);
                v
            }
            (None, Some(events)) => {
                let mut v = Self::blank(inputs, None, ConnState::Off("offline · synthetic transport".into()), subs);
                v.synthetic = true;
                v.attach_source = Some(Box::new(crate::demo::attach));
                v.default_session = Some(crate::demo::SID.to_string());
                let (tx, rx) = async_channel::unbounded();
                for e in events {
                    let _ = tx.try_send(e);
                }
                v.synthetic_tx = Some(tx);
                for fx in v.model.offer_saved() {
                    v.effect(fx);
                }
                v.pump(rx, cx);
                v
            }
            (None, None) => Self::blank(inputs, None, ConnState::Off("offline".into()), subs),
        }
    }

    fn blank(i: Inputs, client: Option<Client>, conn: ConnState, subs: Vec<Subscription>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let texts = Rc::default();
        turns::readiness(&list, &texts);
        Self {
            client,
            model: Model::new(conn),
            rows: Vec::new(),
            disp: Vec::new(),
            summaries: Summaries::default(),
            list,
            toggled: HashSet::new(),
            open_runs: HashSet::new(),
            unfold_hold: Vec::new(),
            synced_busy: false,
            files_gen: 0,
            subs_gen: 0,
            rows_session: None,
            waiting_on: None,
            layout: Layout::of(800., 800., &Metrics::at(theme::DEFAULT_TEXT), Default::default()),
            scroll_carry: None,
            hold_back: Rc::new(Cell::new(false)),
            texts,
            bars: Default::default(),
            composer: i.composer,
            composer_rows: crate::fit::COMPOSER_ROWS,
            top_words: None,
            find_short: false,
            find_input: i.find_input,
            name_input: i.name_input,
            hex_input: i.hex_input,
            secret_input: i.secret_input,
            find: String::new(),
            chip: None,
            expanded: 0,
            // Every row measured (they are cheap, and new ones only come by a
            // splice, [`crate::chrome::RailCache`]): the bar's thumb reaches
            // the rail's end.
            rail_list: ListState::new(0, ListAlignment::Top, px(300.)).measure_all(),
            rail_width: 0.,
            hover: None,
            readout: None,
            readout_wait: None,
            readout_at: None,
            readout_leaving: None,
            readout_from: None,
            readout_left: None,
            rail_fx: Default::default(),
            star_land: Default::default(),
            owner_here: false,
            #[cfg(test)]
            roll_probe: None,
            panels: theme::Panels::default(),
            panels_dirty: Default::default(),
            panels_writing: Default::default(),
            panels_save: None,
            drag: None,
            springback: false,
            panel_fx: crate::chrome::PanelFx::default(),
            fold_keys: FoldKeys::default(),
            width: 0.,
            height: 0.,
            picker_open: false,
            picker_note: None,
            picker_dismissed: None,
            accent_source: theme::Source::Default,
            theme_source: theme::Source::Default,
            theme_follows_config: true,
            theme_profile: None,
            config_theme: SharedString::default(),
            card: None,
            keys_live: false,
            chord: CardChord::default(),
            chord_press: false,
            secret_left: None,
            layout_naming: false,
            rail: Default::default(),
            focus: i.focus,
            restore: None,
            cmd_draft: None,
            cmd_clear: None,
            quiet_note: None,
            drafts: Default::default(),
            composer_of: None,
            draft_note: None,
            metrics_gen: 0,
            bench: None,
            plain_rows: false,
            synthetic: false,
            synthetic_tx: None,
            synthetic_seq: 0,
            start_session: None,
            default_session: None,
            restore_tabs: None,
            tabs_key: None,
            tabs_saved: 0,
            favs_saved: 0,
            favs_written: 0,
            favs_save: None,
            favs_job: Arc::default(),
            favs_writing: false,
            favs_synced: None,
            tabs_written: 0,
            tabs_save: None,
            lists_seen: 0,
            tab_cache: Lru::new(TAB_CACHE, TAB_CACHE_BYTES),
            shown_tab: None,
            attach_wait: None,
            attach_due: None,
            gate: Default::default(),
            tab_keys: FoldKeys::default(),
            once: None,
            tab_first: 0,
            tab_follow: None,
            tab_room: 0,
            tab_labels: Default::default(),
            tab_fx: Default::default(),
            attach_source: None,
            settings: Default::default(),
            usage: crate::usage_view::UsageUi::new(None, Default::default()),
            dash: Default::default(),
            app_settings: Default::default(),
            settings_find: i.settings_find,
            settings_input: i.settings_input,
            gear_menu: false,
            menu_was: None,
            menu_leaving: None,
            smenu_was: None,
            smenu_leaving: None,
            wmenu_was: None,
            wmenu_leaving: None,
            gear_at: None,
            gear_leaving: None,
            gear_was: false,
            gear_dismissed: None,
            shown_view: None,
            shown_views: 0,
            settings_pane: i.settings_pane,
            usage_pane: i.usage_pane,
            view_width: 0.,
            seg_places: Default::default(),
            link: Default::default(),
            conn_dy: 0.,
            conn_held: Default::default(),
            #[cfg(test)]
            composer_bar: Default::default(),
            #[cfg(test)]
            scan_line: None,
            tracer_open: None,
            tracer: None,
            strip_at: Default::default(),
            composer_top: Default::default(),
            over_composer: std::rc::Rc::new(std::cell::Cell::new(f32::INFINITY)),
            dock_h: std::rc::Rc::new(std::cell::Cell::new(f32::NAN)),
            composer_h: std::rc::Rc::new(std::cell::Cell::new(f32::NAN)),
            dock_scope: 0,
            chip_bounds: Default::default(),
            view_seen: None,
            window_active: true,
            main_pane: None,
            pane_of: None,
            worker_panes: Vec::new(),
            full_rebuild: false,
            watch_poll: None,
            visible: i.visible,
            fx: Rc::default(),
            scroller: crate::scroll::Scroller::default(),
            column_key: (None, 0, None),
            column_at: None,
            column_ix: None,
            column_worker: false,
            body_line: 0.,
            layer: None,
            card_at: None,
            card_exit: None,
            receipts: HashMap::new(),
            sent: None,
            card_keys: Cell::new(None),
            jump_card: None,
            stub_away: None,
            halted: None,
            allow_all_focus: i.allow_all_focus,
            card_first: Vec::new(),
            list_box: Cell::new((0., 0.)),
            card_seen: Cell::new(false),
            #[cfg(test)]
            card_presses: 0,
            card_look: Default::default(),
            card_was_up: false,
            tray: crate::tray::TrayCache::default(),
            tray_due: None,
            tray_rows: 0,
            chips: Default::default(),
            watch_note: None,
            placeholder_of: None,
            menu: None,
            menu_focus_back: None,
            palette: None,
            palette_input: i.palette_input,
            palette_back: None,
            palette_sel: Cell::new(None),
            palette_scroll: ScrollHandle::new(),
            palette_gen: 0,
            #[cfg(test)]
            palette_drawn: Cell::new((0., 0., 0., 0.)),
            #[cfg(test)]
            palette_box: Cell::new((0., 0., 0., 0.)),
            cancel_tick: None,
            smenu: None,
            wmenu: None,
            rename_pending: None,
            halt_armed: None,
            halt_tick: None,
            halt_where: None,
            esc_composing: false,
            placeholder_cramped: false,
            menu_picked: false,
            swallow_clicks: None,
            widths: RefCell::default(),
            #[cfg(test)]
            submits: Vec::new(),
            #[cfg(test)]
            halts: 0,

            _tasks: Vec::new(),
            _subs: subs,
        }
    }

    /// Drain an event channel ([`crate::model::pump`]) and apply each batch
    /// to the model at once, whether or not the window renders (minimized,
    /// hidden, occluded): the queue can never grow behind a window that
    /// gets no frames. Only the row rebuild and remeasure wait for the next
    /// frame (`sync_list`), and the transcript coalesces those into one
    /// splice.
    pub fn pump(&mut self, rx: async_channel::Receiver<Event>, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            crate::model::pump(rx, |batch| {
                let now = Instant::now();
                this.update(cx, |this, cx| {
                    if let Some(b) = this.bench.as_mut() {
                        b.on_received(&batch, now);
                    }
                    this.apply_batch(batch, cx);
                })
                .is_ok()
            })
            .await;
        });
        self._tasks.push(task);
    }

    /// One batch of events, as the pump hands it over: into the model, then
    /// the tabs, the visible tab's view and the polls brought to it.
    pub(crate) fn apply_batch(&mut self, batch: Vec<Event>, cx: &mut Context<Self>) {
        self.model.view_dirty = false;
        for ev in batch {
            // A worker's menu, the workers menu, or a cancel armed
            // from the tray, means nothing across a reconnect (a
            // restarted daemon numbers its workers anew).
            if matches!(ev, Event::Attached { .. } | Event::Reattaching { .. }) {
                if self.menu.is_some() || self.wmenu.is_some() {
                    self.close_menu();
                    self.model.view_dirty = true;
                } else if self.model.cancels.disarm() {
                    self.cancel_tick = None;
                    self.model.view_dirty = true;
                }
            }
            // The compaction a `/compact` asked for has its notice in the
            // transcript now: the note that asked is done.
            if let Event::Session { attach_seq, event, .. } = &ev
                && matches!(
                    event,
                    synaps_client::protocol::SessionEvent::CompactionApplied { .. }
                        | synaps_client::protocol::SessionEvent::CompactionFailed { .. }
                        | synaps_client::protocol::SessionEvent::CompactionCancelled
                )
                && self.model.active.as_ref().is_some_and(|a| a.attach_seq == *attach_seq)
                && self.watch_note.as_ref().is_some_and(|n| n.starts_with("/compact:"))
            {
                self.watch_note = None;
                self.model.view_dirty = true;
            }
            for fx in self.model.apply(ev) {
                self.effect(fx);
            }
        }
        self.start_tabs();
        // An attach may have named or merged the visible tab.
        self.reconcile(None);
        // The worker tab on screen: its handle may name another
        // worker now.
        self.disown_reused();
        // A Welcome: the daemon's place and dash are looked for.
        self.dash_on_welcome(cx);
        // A worker's page came: its next poll, drawn or not.
        self.arm_watch_poll(cx);
        // An idle daemon's poll changes nothing: no frame, unless
        // the rail's ages have a new minute to show.
        if self.model.view_dirty || self.rail.minute() != crate::chrome::now_secs() / 60 {
            cx.notify();
        }
    }

    pub(crate) fn effect(&mut self, fx: Effect) {
        match fx {
            Effect::Restore(text) => {
                self.restore = Some(match self.restore.take() {
                    Some(prev) => format!("{prev}\n{text}"),
                    None => text,
                })
            }
            Effect::Unsent { session, text } => self.unsent(session, text),
            Effect::ListSessions => {
                if let Some(c) = &self.client {
                    let _ = c.list_sessions();
                }
            }
            Effect::Resync => {
                if let Some(c) = &self.client {
                    let _ = c.resync();
                }
            }
            Effect::ListSaved { named_only, limit } => {
                let sent = match (&self.client, &self.synthetic_tx) {
                    (Some(c), _) => c.saved_sessions(named_only, limit).map_err(|e| e.to_string()),
                    // The synthetic transport answers as a daemon would.
                    (None, Some(tx)) => tx.try_send(crate::demo::saved(named_only, limit)).map_err(|e| e.to_string()),
                    (None, None) => Err("offline".into()),
                };
                if let Err(e) = sent {
                    for fx in self.model.saved_failed(&e) {
                        self.effect(fx);
                    }
                }
            }
            Effect::Attach { session_id, fallback, open, place } => self.open_tab(Open::Attach { session_id, fallback, open }, place),
            Effect::Resume { req, open, place } => self.open_tab(Open::Resume { req, open }, place),
            Effect::WorkerNote(note) => self.watch_note = Some(note.into()),
            Effect::Command { name, reply } => {
                self.watch_note = Some(crate::palette::reply_note(&name, &reply).into());
                self.quiet_note = self.watch_note.clone().filter(|_| matches!(reply, synaps_client::CommandReply::Done(_)));
                if let Some((_, text)) = self.cmd_draft.take_if(|d| d.0 == name)
                    && matches!(reply, synaps_client::CommandReply::Done(_))
                {
                    self.cmd_clear = Some(text);
                }
            }
        }
    }

    /// Input sent to `session` before its tab was left, given back: into
    /// that tab's draft (said when it is shown again), or the composer if
    /// it is shown again already.
    fn unsent(&mut self, session: String, text: String) {
        // Its tab followed the session to a new id: that tab's.
        let session = self.model.tabs.followed(&session).map_or(session, str::to_string);
        if self.composer_of.as_ref().is_some_and(|(_, s)| s.as_deref() == Some(session.as_str())) {
            self.effect(Effect::Restore(text));
            self.draft_note = Some(UNSENT_NOTE);
            self.model.view_dirty = true;
        } else if self.model.tabs.knows(&session) {
            self.drafts.give_back(session, text, UNSENT_NOTE);
        } else {
            self.model.named.notice = Some(format!("Not sent (its tab is closed): {}", one_line(&text, 120)));
            self.model.view_dirty = true;
        }
    }

    /// Input given back for the composer's tab goes in, then, if the
    /// visible tab changed, the composer's text and caret go to the tab
    /// left and the shown tab's come back ([`crate::tabs::Drafts::frame`]):
    /// in that order, so input given back in the frame of a switch stays
    /// with its tab. Nothing is read or allocated on a frame with neither.
    fn sync_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Over a worker's view the composer steers it (compared, not
        // allocated, each frame).
        let worker = self.model.watch.focused().map(|w| (w.handle_id.as_str(), w.state(), w.tabbed));
        let cramped = self.layout.cramped;
        if worker != self.placeholder_of.as_ref().map(|(h, s, t)| (h.as_str(), s.as_str(), *t)) || cramped != self.placeholder_cramped {
            let text = match self.model.watch.focused() {
                Some(w) if w.read_only() && w.tabbed => format!("{} is {}: nothing to steer", w.name, w.state()),
                Some(w) if w.read_only() => format!("{} is {}: nothing to steer  (Esc: back to the session)", w.name, w.state()),
                Some(w) => format!("Steer {}: it reads this at its next step  (Enter sends)", w.name),
                None => COMPOSER_PLACEHOLDER.to_string(),
            };
            // Cramped, the field is short: its key hints go, so it is not
            // cut off against the glyph.
            let text: SharedString = if cramped { text.split("  (").next().unwrap_or_default().to_string().into() } else { text.into() };
            self.placeholder_of = worker.map(|(h, s, t)| (h.to_string(), s.to_string(), t));
            self.placeholder_cramped = cramped;
            self.composer.update(cx, |c, cx| c.set_placeholder(text, window, cx));
        }
        // Worker tabs that followed their session: their drafts with them.
        for (from, to) in self.model.tabs.take_rekeys() {
            self.drafts.rekey(&from, &to);
        }
        // A refused steer's text, once the thread it is for is on screen
        // and the composer is that tab's (never in the frame of a switch:
        // it would go to the tab left).
        if self.composer_owned() {
            if let Some((text, note)) = self.model.watch.take_unsent() {
                self.effect(Effect::Restore(text));
                self.watch_note = Some(note.into());
            }
        } else if self.model.watch.has_unsent() {
            cx.notify();
        }
        // The visible tab went into the one holding its session: the
        // composer's text is that tab's now (the follow rule keeps it).
        if let Some((from, into)) = self.model.tabs.take_merged()
            && let Some(o) = self.composer_of.as_mut().filter(|o| o.0 == from)
        {
            o.0 = into;
        }
        // A slash command the daemon ran: its text leaves the composer,
        // if the composer still holds it (the tab's, unedited).
        if let Some(text) = self.cmd_clear.take()
            && self.composer_owned()
            && self.composer.read(cx).value().trim() == text
        {
            self.composer.update(cx, |c, cx| c.set_value("", window, cx));
        }
        // A view on screen leaves the session tab behind it current: the
        // composer stays that tab's.
        if self.restore.is_none() && self.composer_owned() {
            return;
        }
        // A tab left while its slash command was out: the answer goes to
        // an attachment no longer on screen (never shown here), so the
        // command's text leaves with it, as a sent message's does: the tab's
        // draft is not left holding a command already run.
        if !self.composer_owned()
            && let Some((_, sent)) = self.cmd_draft.take()
            && self.composer.read(cx).value().trim() == sent
        {
            self.composer.update(cx, |c, cx| c.set_value("", window, cx));
        }
        let c = self.composer.read(cx);
        let (text, cursor) = (c.value(), c.cursor());
        let out = frame_composer(
            &mut self.drafts,
            &mut self.composer_of,
            &self.model.tabs,
            self.restore.take(),
            (&mut self.draft_note, &mut self.watch_note),
            (text.as_ref(), cursor),
        );
        if let Some((text, cursor)) = out {
            self.composer.update(cx, |c, cx| {
                c.set_value(text, window, cx);
                if let Some(at) = cursor {
                    c.set_selected_range(at..at, cx);
                }
            });
        }
    }

    /// Resume a saved session: its folder as is, `~` if the daemon says it
    /// is gone there.
    fn resume(&mut self, req: ResumeRequest, open: bool) {
        let Some(c) = &self.client else {
            if self.synthetic {
                // Never answered: the row stays OPENING.
                self.synthetic_seq += 1;
                self.model.resuming(req, self.synthetic_seq, open);
            } else {
                self.model.named.error = Some("offline".into());
            }
            return;
        };
        match c.resume(req.target.clone(), req.options()) {
            Ok(seq) => {
                self.model.resuming(req, seq, open);
            }
            Err(e) => self.model.named.error = Some(e.to_string()),
        }
    }

    /// The synthetic daemon (`--offline`, the tab bench): an attach it
    /// answers as a daemon would, on the channel, or never (OPENING).
    pub(crate) fn synthetic_attach(&mut self, session_id: &str, fallback: Option<ResumeRequest>, open: bool) {
        self.synthetic_seq += 1;
        let seq = self.synthetic_seq;
        self.model.attaching(seq, fallback, open);
        self.model.asked_for(session_id);
        if let (Some(src), Some(tx)) = (&self.attach_source, &self.synthetic_tx) {
            for e in src(session_id, seq) {
                let _ = tx.try_send(e);
            }
        }
    }

    /// A rail row was clicked (looked up by its key now: the rail may have
    /// been rebuilt since the frame that drew it): in the visible tab, or
    /// (Ctrl / Cmd, the middle button) a new one.
    pub(crate) fn rail_click(&mut self, key: u64, place: Place) {
        let Some(action) = self.rail.rail.rows.iter().find(|r| r.key == key).map(|r| r.action.clone()) else { return };
        for fx in self.model.open_row_in(&action, place) {
            self.effect(fx);
        }
    }

    /// Overlays that closed since the last frame leave (fluid.md §3.10):
    /// 90 ms `in` and 2 px back toward their anchor, without a hitbox. One
    /// the keyboard opened goes at once, as it came.
    fn overlay_exits(&mut self) {
        let now = self.fx.now();
        if self.menu.is_none() {
            // A row's menu has no exit drawn (it is anchored to the row).
            if let Some(m) = self.menu_was.take().filter(|m| m.born.is_some() && matches!(m.at, crate::menu::At::Point(_))) {
                self.menu_leaving = Some((m, now));
            }
        } else {
            self.menu_leaving = None;
            // Kept once per menu opened or refreshed in place (its exit shows
            // what it showed last), not cloned every frame.
            if self.menu_was.as_ref().map(|m| (m.born, &m.items)) != self.menu.as_ref().map(|m| (m.0.born, &m.0.items)) {
                self.menu_was = self.menu.as_ref().map(|m| m.0.clone());
            }
        }
        if self.menu_leaving.as_ref().is_some_and(|(_, at)| self.fx.leave(*at).is_none()) {
            self.menu_leaving = None;
        }
        if self.smenu.is_none() {
            if let Some(m) = self.smenu_was.take().filter(|m| m.born.is_some()) {
                self.smenu_leaving = Some((m, now));
            }
        } else {
            self.smenu_leaving = None;
            if self.smenu_was.as_ref().map(|m| m.born) != self.smenu.as_ref().map(|m| m.0.born) {
                self.smenu_was = self.smenu.as_ref().map(|m| m.0.clone());
            }
        }
        if self.smenu_leaving.as_ref().is_some_and(|(_, at)| self.fx.leave(*at).is_none()) {
            self.smenu_leaving = None;
        }
        // The workers menu's items follow the tray while it is open: its
        // exit shows the last of them.
        if self.wmenu.is_none() {
            if let Some(m) = self.wmenu_was.take().filter(|m| m.born.is_some()) {
                self.wmenu_leaving = Some((m, now));
            }
        } else {
            self.wmenu_leaving = None;
            if self.wmenu_was.as_ref().map(|m| (m.born, &m.items)) != self.wmenu.as_ref().map(|m| (m.born, &m.items)) {
                self.wmenu_was = self.wmenu.clone();
            }
        }
        if self.wmenu_leaving.as_ref().is_some_and(|(_, at)| self.fx.leave(*at).is_none()) {
            self.wmenu_leaving = None;
        }
        if self.gear_menu != self.gear_was {
            self.gear_was = self.gear_menu;
            if self.gear_menu {
                self.gear_at = Some(now);
                self.gear_leaving = None;
            } else {
                self.gear_leaving = Some(now);
            }
        }
        if self.gear_leaving.is_some_and(|at| self.fx.leave(at).is_none()) {
            self.gear_leaving = None;
        }
        if self.readout_leaving.is_some_and(|(_, at)| self.fx.leave(at).is_none()) {
            self.readout_leaving = None;
        }
    }

    /// The rail as the model, find, the chip and the buckets have it now.
    fn refresh_rail(&mut self) {
        // The favourites' snapshots follow the lists (and are saved when
        // they change): only when the lists did.
        if self.favs_synced != Some(self.model.rail_gen) {
            self.model.sync_favs();
            self.favs_synced = Some(self.model.rail_gen);
        }
        let view = crate::rail::View {
            find: std::mem::take(&mut self.find),
            chip: self.chip,
            open: self.panels.history_open,
            expanded: self.expanded,
            favs_closed: self.panels.favs_closed,
        };
        let hold = self.fx.held(crate::chrome::RAIL_TOUCH);
        self.rail.refresh(&self.model, &view, &self.rail_list, hold);
        self.find = view.find;
    }

    /// Enter in find (see [`Model::find_enter`]), over the rows the rail
    /// shows for what is typed now; Ctrl+Enter opens in a new tab.
    fn find_enter(&mut self, typed: &str, place: Place) {
        self.refresh_rail();
        let rail = &self.rail.rail;
        let visible: Vec<RowAction> = rail
            .items
            .iter()
            .filter_map(|i| match i {
                crate::rail::Item::Row(r) | crate::rail::Item::Hist(r) => Some(rail.rows[*r].action.clone()),
                _ => None,
            })
            .collect();
        for fx in self.model.find_enter_in(typed, &visible, place) {
            self.effect(fx);
        }
    }

    /// The pointer entered or left a rail row: its readout opens after
    /// [`READOUT_DELAY`] at rest, and closes when it leaves.
    pub(crate) fn hover_row(&mut self, key: u64, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            // A drag of the rail's edge passing over it is not a rest.
            if self.hover == Some(key) || self.drag.is_some_and(|g| g.moved) {
                return;
            }
            self.hover = Some(key);
            // Another row's readout shown, or gone less than 300 ms ago: this
            // one shows now and follows from it, without entering again
            // (fluid.md §3.10, motion.md §6.4).
            let now = self.fx.now();
            let from = self
                .readout
                .take()
                .or(self.readout_left.filter(|l| now.saturating_duration_since(l.1) < crate::motion::REPLACE).map(|l| l.0));
            if let Some(old) = from.filter(|&k| k != key) {
                let rail = &self.rail.rail;
                let y = rail
                    .items
                    .iter()
                    .position(|it| crate::chrome::row_key(rail, it) == Some(old))
                    .and_then(|ix| crate::chrome::item_y(&self.rail_list, ix))
                    .map(|y| y.0);
                self.readout_from = y.or(self.fx.spring_at(crate::chrome::READOUT_Y));
                self.readout = Some(key);
                self.readout_at = None;
                self.readout_leaving = None;
                self.readout_left = None;
                self.readout_wait = None;
                cx.notify();
                return;
            }
            self.readout_wait = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(READOUT_DELAY).await;
                let _ = this.update(cx, |v, cx| {
                    if v.hover == Some(key) {
                        v.readout = Some(key);
                        v.readout_at = Some(v.fx.clock());
                        cx.notify();
                    }
                });
            }));
        } else if self.hover == Some(key) {
            self.hover = None;
            self.readout_wait = None;
            if self.readout.take().is_some() {
                let now = self.fx.now();
                self.readout_leaving = Some((key, now));
                self.readout_left = Some((key, now));
                cx.notify();
            }
        }
    }

    /// A HISTORY bucket opened or closed (remembered, as the panels are);
    /// OLDER opening lists up to the wire's cap.
    pub(crate) fn toggle_bucket(&mut self, b: Bucket, cx: &mut Context<Self>) {
        let mut p = self.panels;
        p.history_open ^= b.bit();
        let open = p.history_open & b.bit() != 0;
        self.rail.fold = true;
        if !open {
            self.expanded &= !b.bit();
        }
        self.set_panels(p, cx);
        if open && b == Bucket::Older {
            for fx in self.model.want_deep() {
                self.effect(fx);
            }
        }
        cx.notify();
    }

    /// "Name this session…": send what the model checked.
    fn rename(&mut self, req: Option<RenameRequest>) {
        let Some(RenameRequest { session_id, name, expect }) = req else { return };
        let sent = match &self.client {
            Some(c) => c.rename(session_id, Some(&name), expect),
            None => Err(RequestError::NotConnected),
        };
        if let Err(e) = sent {
            self.model.rename_not_sent(e.to_string());
        }
    }

    /// A new session, in a new tab (`+`, NEW, Ctrl+T).
    pub(crate) fn create(&mut self) {
        self.open_tab(Open::Create, Place::New);
    }

    fn create_now(&mut self) {
        // Tools run in the user's home, not wherever the launcher started us.
        if let Some(seq) = self.client.as_ref().and_then(|c| c.create(CreateOptions::new().cwd(Cwd::Home)).ok()) {
            self.model.want(seq);
        }
    }

    // ── tabs ───────────────────────────────────────────────────────────

    /// Open a session in a tab: its own if it has one (focused), else the
    /// visible one or a new one after it (`place`).
    pub(crate) fn open_tab(&mut self, how: Open, place: Place) {
        match how {
            Open::Attach { session_id, fallback, open } => {
                let label = self.model.sessions.iter().find(|s| s.id == session_id).map_or_else(|| session_id.clone(), |s| s.label());
                self.model.tabs.open(&session_id, &label, place);
                self.model.rail_gen += 1;
                self.reconcile(Some((fallback, open)));
            }
            Open::Resume { req, open } => {
                self.model.tabs.open_pending(&req.label, place);
                self.model.rail_gen += 1;
                self.reconcile(None);
                self.resume(req, open);
            }
            Open::Create => {
                if self.client.is_none() {
                    return;
                }
                self.model.tabs.open_pending(NEW_SESSION, place);
                self.model.rail_gen += 1;
                self.reconcile(None);
                self.create_now();
            }
        }
    }

    /// Bring the model and the list to the visible tab. The model already
    /// holding its session (attached or on its way: a create or a resume
    /// named the tab, an attach merged it into the tab that had it) is
    /// left as is. Another session: the one shown goes to the tab cache,
    /// the visible one's cached state (or nothing) comes in, and its
    /// attach goes out. `hint`: the open asked for an attach (with its
    /// fallback) even if nothing changed (a session that detached).
    pub(crate) fn reconcile(&mut self, hint: Option<(Option<ResumeRequest>, bool)>) {
        let dropped = std::mem::take(&mut self.model.tabs.dropped);
        self.restore_notice(dropped);
        // A view on screen leaves the session tab behind it current.
        let vis = self.model.tabs.current().map(|t| (t.id, t.session.clone()));
        // Another tab on screen (not the same one named or re-attached),
        // or the one on screen opened again (a rail row, an open by name):
        // its view starts from its hand-off, never from an event batch.
        let shown = hint.is_some() || self.shown_tab.as_ref().map(|v| v.0) != vis.as_ref().map(|v| v.0);
        // A session tab left keeps the worker its view showed in place.
        if let Some((id, _)) = self.shown_tab
            && vis.as_ref().map(|v| v.0) != Some(id)
        {
            let inplace = self.model.watch.focused().filter(|w| !w.tabbed).map(|w| (w.handle_id.clone(), w.name.clone()));
            self.model.tabs.set_watching(id, inplace);
            self.close_menu();
        }
        if let Some((_, Some(s))) = &vis
            && self.model.wanted_session() == Some(s.as_str())
        {
            // What a cache held of it is older than what attached.
            let _ = self.tab_cache.take(s);
            self.shown_tab = vis;
            // Another tab of the same session (its worker's, or back to
            // it): the same attachment, another view.
            self.tab_watch(shown);
            if hint.as_ref().is_some_and(|h| h.1) {
                // An Open by name of the session on screen: done.
                self.model.named.opened = true;
                self.model.named.opening = None;
            }
            return;
        }
        if vis == self.shown_tab && hint.is_none() {
            return;
        }
        if vis != self.shown_tab {
            self.stash();
            self.load(vis.as_ref().and_then(|v| v.1.clone()));
        }
        self.shown_tab = vis.clone();
        self.tab_watch(shown);
        match vis {
            Some((_, Some(s))) => self.attach_tab(s, hint),
            // A create or a resume: the caller sends it.
            Some((_, None)) => {}
            None => self.detach_all(),
        }
    }

    /// The current tab's worker view. A worker tab: its worker, checked
    /// on every reconcile (an attach elsewhere resets the view, a handle
    /// may name another worker now). A session tab, only when it is
    /// `shown` (come on screen, or opened again): the worker it showed in
    /// place when it was left, else its thread. Between, its view is the
    /// user's: a session poll (one every 3 s, changed or not) neither
    /// drops a worker shown in place nor brings back one Esc'd away.
    fn tab_watch(&mut self, shown: bool) {
        let Some(t) = self.model.tabs.current() else { return };
        let want = match (&t.worker, t.kind) {
            (Some(w), crate::tabs::Kind::Worker) => Some((w.handle.clone(), w.name.clone(), w.call.clone(), true)),
            _ if !shown => return,
            _ => {
                let id = t.id;
                let handoff = self.model.tabs.take_watching(id);
                log::debug!(target: "synaps_desktop::watch", "tab {id} shown: {}", handoff.as_ref().map_or("its thread", |w| w.0.as_str()));
                handoff.map(|(h, n)| (h, n, None, false))
            }
        };
        match want {
            Some((handle, name, call, tab)) => {
                let Some(id) = handle.strip_prefix("sa_").and_then(|n| n.parse().ok()) else { return };
                let c = crate::watch::Candidate::of(id, &name).with_call(call.as_deref());
                // A worker tab whose handle names another worker now: its
                // view is gone, never that worker's.
                let reused = tab && self.tab_worker_reused();
                if self.model.watch.focus() == Some(handle.as_str())
                    && self.model.watch.in_tab() == tab
                    && (reused || self.model.watch.focused_is(&c))
                {
                    if reused {
                        self.disown_reused();
                    }
                    return;
                }
                let cwd = self.model.cwd().map(str::to_string);
                if tab {
                    self.model.watch.open_tab(&c, cwd.as_deref(), Instant::now());
                } else {
                    self.model.watch.open(&c, cwd.as_deref(), Instant::now());
                }
                self.watch_opened();
                if reused {
                    self.disown_reused();
                }
            }
            None if self.model.watch.focus().is_some() => self.model.watch.focus_main(),
            None => {}
        }
    }

    /// The session on screen goes to the tab cache (if it is one): the
    /// model's transcript and numbers, the rows, folds and the list with
    /// its heights and scroll position. A fresh list takes its place.
    fn stash(&mut self) {
        // Its worker views go with it; the session's own list comes back
        // first (a worker's view is built again when the tab is shown).
        let mut watch = std::mem::take(&mut self.model.watch);
        watch.suspend();
        // The rows first: the transcript may have run ahead of the last frame.
        self.sync_list();
        self.list_out();
        let key = self.model.transcript.session_id().map(str::to_string);
        let (transcript, stats) = self.model.switch_out();
        self.card = None;
        self.attach_wait = None;
        self.attach_due = None;
        let fresh = ListState::new(0, ListAlignment::Top, px(600.));
        fresh.set_follow_mode(FollowMode::Tail);
        turns::readiness(&fresh, &self.texts);
        // What a rise left owed is the old list's.
        self.scroll_carry = None;
        let cache = TabCache {
            transcript,
            stats,
            rows: std::mem::take(&mut self.rows),
            disp: std::mem::take(&mut self.disp),
            summaries: std::mem::take(&mut self.summaries),
            list: std::mem::replace(&mut self.list, fresh),
            toggled: std::mem::take(&mut self.toggled),
            open_runs: std::mem::take(&mut self.open_runs),
            synced_busy: std::mem::take(&mut self.synced_busy),
            files_gen: std::mem::take(&mut self.files_gen),
            waiting_on: self.waiting_on.take(),
            metrics_gen: self.metrics_gen,
            watch,
        };
        self.rows_session = None;
        if let Some(k) = key {
            let tabs = &self.model.tabs;
            let bytes = cache.bytes();
            self.tab_cache.put(k, cache, bytes, |k| tabs.holds(k));
        }
    }

    /// The visible tab's session as last seen, from the cache, or nothing
    /// until its attach lands.
    fn load(&mut self, session: Option<String>) {
        self.list_out();
        self.disarm_halt();
        match session.as_ref().and_then(|s| self.tab_cache.take(s)) {
            Some(c) => {
                self.model.switch_in(c.transcript, c.stats);
                self.rows = c.rows;
                self.disp = c.disp;
                self.summaries = c.summaries;
                self.list = c.list;
                self.scroll_carry = None;
                self.toggled = c.toggled;
                self.open_runs = c.open_runs;
                self.synced_busy = c.synced_busy;
                self.files_gen = c.files_gen;
                self.waiting_on = c.waiting_on;
                if c.metrics_gen != self.metrics_gen {
                    // The text size changed while it was in the background.
                    self.list.remeasure();
                }
                // Polled again once its attach lands (`Watch::on_reconnect`).
                self.model.watch = c.watch;
            }
            None => {
                self.model.switch_in(crate::model::transcript(), Stats::default());
                self.model.watch = Watch::default();
            }
        }
        self.rows_session = session;
    }

    /// Attach the visible tab's session, or wait (shown OPENING) as the
    /// gate says: a burst of switches, or a held tab key, attaches once,
    /// after the last ([`crate::tabs::AttachGate`]).
    fn attach_tab(&mut self, session: String, hint: Option<(Option<ResumeRequest>, bool)>) {
        let (fallback, open) = hint.unwrap_or_else(|| (self.model.tab_fallback(&session), false));
        self.attach_wait = None;
        let restoring = self.model.tabs.current().is_some_and(|t| self.model.tabs.restoring(t.id));
        if restoring || (self.client.is_some() && self.gate.switch(Instant::now())) {
            self.model.showing(&session);
            // A restored tab attaches once a list has its session (`start_tabs`).
            if !restoring {
                self.attach_wait = Some((session, fallback, open));
            }
            return;
        }
        self.attach_now(session, fallback, open);
    }

    fn attach_now(&mut self, session: String, fallback: Option<ResumeRequest>, open: bool) {
        if let Some(seq) = self.client.as_ref().and_then(|c| c.attach(session.clone(), AttachMode::Mirror).ok()) {
            self.model.attaching(seq, fallback, open);
            self.model.asked_for(&session);
        } else if self.synthetic_tx.is_some() {
            self.synthetic_attach(&session, fallback, open);
        } else {
            // No daemon: shown, never attached.
            self.model.showing(&session);
        }
    }

    /// The waiting attach's timer, (re)set for when the gate is due.
    fn arm_attach(&mut self, cx: &mut Context<Self>) {
        if self.attach_wait.is_none() {
            self.attach_due = None;
            return;
        }
        let at = self.gate.due();
        self.attach_due = Some(cx.spawn(async move |this, cx| {
            if let Some(left) = at.map(|t| t.saturating_duration_since(Instant::now())).filter(|d| !d.is_zero()) {
                cx.background_executor().timer(left).await;
            }
            let _ = this.update(cx, |v, cx| v.attach_waited(cx));
        }));
    }

    /// The deferred attach, if the gate is ready (else its timer again) and
    /// its tab is still the visible one.
    fn attach_waited(&mut self, cx: &mut Context<Self>) {
        self.attach_due = None;
        if !self.gate.ready(Instant::now()) {
            return self.arm_attach(cx);
        }
        if let Some((s, fallback, open)) = self.attach_wait.take()
            && self.model.tabs.current_session() == Some(s.as_str())
            && self.model.active.is_none()
        {
            self.attach_now(s, fallback, open);
            cx.notify();
        }
    }

    /// A tab key, or Ctrl / Cmd, went up: a waiting attach is due
    /// [`crate::tabs::AttachGate::GAP`] from now.
    fn tab_key_up(&mut self, cx: &mut Context<Self>) {
        self.gate.key_up(Instant::now());
        if self.attach_wait.is_some() {
            self.arm_attach(cx);
        }
    }

    /// The last tab closed: nothing attached (the session keeps running).
    fn detach_all(&mut self) {
        self.attach_wait = None;
        if let Some(c) = &self.client {
            let _ = c.detach();
        }
    }

    /// Restore the saved tabs once the first session list has landed, then
    /// open `--session` (or `--offline`'s session when nothing was
    /// restored). A restored tab waits, OPENING, for a list that has its
    /// session ([`crate::tabs::Tabs::confirm`]); one that never comes closes,
    /// said once, and the file keeps it until the user changes the tabs.
    fn start_tabs(&mut self) {
        if !self.model.sessions_listed {
            return;
        }
        if let Some(saved) = self.restore_tabs.take()
            && self.model.tabs.is_empty()
        {
            self.model.tabs = crate::tabs::Tabs::restore(&saved);
            self.tabs_saved = self.model.tabs.generation;
            self.tabs_written = self.tabs_saved;
            self.model.rail_gen += 1;
            self.model.view_dirty = true;
            self.reconcile(None);
        }
        if self.model.lists != self.lists_seen {
            self.lists_seen = self.model.lists;
            let quiet = self.model.tabs.generation == self.tabs_saved;
            let sessions = std::mem::take(&mut self.model.sessions);
            let shown = self.model.tabs.confirm(&sessions);
            self.model.sessions = sessions;
            let dropped = std::mem::take(&mut self.model.tabs.dropped);
            if quiet && dropped > 0 {
                // Not saved without them: the file changes when the user
                // changes the tabs (a daemon slower than the lists keeps them).
                self.tabs_saved = self.model.tabs.generation;
                self.tabs_written = self.tabs_saved;
            }
            let vis = self.model.tabs.current().map(|t| (t.id, t.session.clone()));
            if vis != self.shown_tab {
                // A tab followed its session to a new id, or closed.
                self.model.rail_gen += 1;
                self.model.view_dirty = true;
                self.reconcile(None);
            } else if shown && let Some((_, Some(s))) = vis {
                self.model.rail_gen += 1;
                self.model.view_dirty = true;
                self.attach_tab(s, None);
            }
            self.restore_notice(dropped);
        }
        let start = self.start_session.take().or_else(|| self.default_session.take().filter(|_| self.model.tabs.is_empty()));
        self.default_session = None;
        if let Some(id) = start {
            let place = if self.model.tabs.is_empty() { Place::Current } else { Place::New };
            let fallback = self.model.tab_fallback(&id);
            self.open_tab(Open::Attach { session_id: id, fallback, open: false }, place);
        }
    }

    /// Restored tabs closed (their sessions never came back): said once,
    /// and drawn.
    fn restore_notice(&mut self, dropped: usize) {
        if dropped == 0 {
            return;
        }
        let early = std::mem::take(&mut self.model.tabs.dropped_early);
        self.model.named.notice = Some(match (dropped, early) {
            (1, false) => "1 saved tab closed: its session is no longer open (Ctrl+Shift+T reopens it).".into(),
            (n, false) => format!("{n} saved tabs closed: their sessions are no longer open (Ctrl+Shift+T reopens them)."),
            (1, true) => "1 saved tab closed before its session came back (Ctrl+Shift+T reopens it).".into(),
            (n, true) => format!("{n} saved tabs closed before their sessions came back (Ctrl+Shift+T reopens them)."),
        });
        self.model.rail_gen += 1;
        self.model.view_dirty = true;
    }

    /// Show tab `ix` (a click on it).
    pub(crate) fn focus_tab(&mut self, ix: usize) {
        if self.model.tabs.focus(ix) {
            self.model.rail_gen += 1;
            self.reconcile(None);
        }
    }

    /// Close tab `ix` (×, the middle button, Ctrl+W): only a detach; the
    /// session keeps running. Its state stays in the cache for Ctrl+Shift+T.
    pub(crate) fn close_tab(&mut self, ix: usize) {
        if self.model.tabs.close(ix) {
            self.model.rail_gen += 1;
            self.reconcile(None);
        }
    }

    /// Drop tab `from` where tab `to` is.
    pub(crate) fn move_tab(&mut self, from: u64, to: u64) {
        let ix = |id| self.model.tabs.iter().position(|t| t.id == id);
        if let (Some(a), Some(b)) = (ix(from), ix(to))
            && self.model.tabs.move_tab(a, b)
        {
            self.model.rail_gen += 1;
        }
    }

    /// A tab key's action. Ctrl+T, Ctrl+W and Ctrl+Shift+T act on a press,
    /// never on its auto-repeats (a held Ctrl+T is not a burst of new
    /// sessions): their key-down decides ([`OnceKey`]).
    pub(crate) fn tab_key(&mut self, key: TabKey, cx: &mut Context<Self>) {
        self.gate.key_down(Instant::now());
        if matches!(key, TabKey::New | TabKey::Close | TabKey::Reopen) {
            self.once = Some(OnceKey::Tab(key));
            cx.propagate();
            return;
        }
        self.tab_act(key, cx);
    }

    /// A once-per-press key's key-down, after its action: a press acts, a
    /// repeat does not.
    fn once_key(&mut self, k: OnceKey, held: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        match k {
            OnceKey::Fold(rail) => {
                if self.fold_keys.press(now, held) {
                    self.toggle_panel(rail, cx);
                }
            }
            OnceKey::Tab(key) => {
                if self.tab_keys.press(now, held) {
                    self.tab_act(key, cx);
                }
            }
        }
    }

    /// What a tab key does.
    pub(crate) fn tab_act(&mut self, key: TabKey, cx: &mut Context<Self>) {
        let tabs = &mut self.model.tabs;
        let changed = match key {
            TabKey::New => {
                self.create();
                cx.notify();
                return;
            }
            TabKey::Close => match tabs.active() {
                Some(ix) => tabs.close(ix),
                None => false,
            },
            TabKey::Reopen => tabs.reopen(),
            TabKey::Next => tabs.step(1),
            TabKey::Prev => tabs.step(-1),
            TabKey::Nth(n) => tabs.focus(n),
            TabKey::Last => tabs.focus_last(),
        };
        if changed {
            self.model.rail_gen += 1;
            self.reconcile(None);
            cx.notify();
        }
    }

    /// The tabs changed since the last save: saved [`SAVE_DELAY`] later, off
    /// the UI thread (a burst of changes is one write).
    fn schedule_tabs_save(&mut self, cx: &mut Context<Self>) {
        if self.tabs_key.is_none() || self.bench.is_some() || self.model.tabs.generation == self.tabs_saved {
            return;
        }
        self.tabs_saved = self.model.tabs.generation;
        self.tabs_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let job =
                this.update(cx, |v, _| (theme::settings_path(), v.tabs_key.clone(), v.model.tabs.saved(), v.model.tabs.generation)).ok();
            let Some((Some(path), Some(key), saved, generation)) = job else { return };
            let done = cx.background_spawn(async move { theme::save_tabs(&path, &key, &saved) }).await;
            let _ = this.update(cx, |v, cx| match done {
                Ok(()) => v.tabs_written = v.tabs_written.max(generation),
                Err(e) => {
                    // Its own line: a switch or an attach does not wipe it.
                    v.model.named.notice = Some(format!("Tabs not saved: {e}"));
                    cx.notify();
                }
            });
        }));
    }

    /// The favourites changed since the last save (the set, or what a row
    /// shows; an update time alone waits for quit): saved [`SAVE_DELAY`]
    /// later, off the UI thread, under the tabs' key (never in a bench), as
    /// a merge into what the file has now ([`crate::favourites::merge`]):
    /// another window's adds and removes are kept, and taken.
    ///
    /// A write once begun is never dropped: a change while it runs waits for
    /// it (the same task writes again), and what it wrote is taken
    /// ([`crate::favourites::Favourites::written`]) even if the window closes meanwhile
    /// ([`FavsJob`]), so the next merge never reads a stale base (a removed
    /// favourite coming back as another instance's).
    fn schedule_favs_save(&mut self, cx: &mut Context<Self>) {
        if self.tabs_key.is_none() || self.bench.is_some() || self.model.favs.generation == self.favs_saved {
            return;
        }
        self.favs_saved = self.model.favs.generation;
        self.carry_favs();
        if self.favs_writing {
            return;
        }
        self.favs_save = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SAVE_DELAY).await;
                let job = this
                    .update(cx, |v, _| {
                        let f = &v.model.favs;
                        v.favs_writing = true;
                        (theme::settings_path(), v.tabs_key.clone(), f.base.clone(), f.list.clone(), f.generation, v.favs_job.clone())
                    })
                    .ok();
                let Some((Some(path), Some(key), base, sent, generation, slot)) = job else { return };
                let done = cx.background_spawn(async move { FavsJob::write(&slot, &path, &key, &base, sent, generation) }).await;
                let again = this.update(cx, |v, cx| {
                    let taken = v.take_favs_written();
                    if let Err(e) = done {
                        v.model.named.notice = Some(format!("Favourites not saved: {e}"));
                        cx.notify();
                        return false;
                    }
                    v.favs_written = v.favs_written.max(generation);
                    v.model.favs.quiet = false;
                    if taken {
                        v.model.rail_gen += 1;
                        cx.notify();
                    }
                    // Changed while it wrote: write again.
                    v.model.favs.generation != generation
                });
                if !again.unwrap_or(false) {
                    return;
                }
            }
        }));
    }

    /// The write that finished, taken into the list (once). `true`: the
    /// list changed (another instance's adds or removes).
    fn take_favs_written(&mut self) -> bool {
        self.favs_writing = false;
        let done = self.favs_job.lock().unwrap_or_else(std::sync::PoisonError::into_inner).done.take();
        let changed = done.is_some_and(|(_, sent, file)| self.model.favs.written(&sent, file));
        if changed {
            self.carry_favs();
        }
        changed
    }

    /// The favourites in memory, for a write that sets a corrupt settings
    /// file aside to carry ([`theme::carry_favourites`]).
    fn carry_favs(&self) {
        if let (Some(path), Some(key), None) = (theme::settings_path(), self.tabs_key.as_deref(), self.bench.as_ref()) {
            theme::carry_favourites(&path, key, &self.model.favs.list);
        }
    }

    /// Favourites changed since the last write (an update time too),
    /// written now, merged as [`AppView::schedule_favs_save`] does.
    ///
    /// A background write still running is waited for and taken first (it
    /// holds [`FavsJob`]'s lock while it writes, never longer than the
    /// settings lock's bounded wait and the write); one not begun never
    /// will.
    fn flush_favs(&mut self) {
        self.favs_save = None;
        let slot = self.favs_job.clone();
        let mut job = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        job.closed = true;
        if let Some((generation, sent, file)) = job.done.take() {
            self.model.favs.written(&sent, file);
            self.favs_written = self.favs_written.max(generation);
        }
        let f = &self.model.favs;
        if self.tabs_key.is_none() || self.bench.is_some() || (f.generation == self.favs_written && !f.quiet) {
            return;
        }
        if let (Some(path), Some(key)) = (theme::settings_path(), self.tabs_key.as_deref()) {
            let sent = f.list.clone();
            match theme::save_favourites(&path, key, &f.base, &sent) {
                Ok(file) => {
                    self.favs_written = self.model.favs.generation;
                    self.model.favs.quiet = false;
                    self.model.favs.written(&sent, file);
                }
                Err(e) => eprintln!("synaps-desktop: favourites not saved: {e}"),
            }
        }
    }

    /// Tabs changed since the last write, written now (the window closes,
    /// the app quits), whether or not a frame scheduled the save.
    fn flush_tabs(&mut self) {
        self.tabs_save = None;
        if self.tabs_key.is_none() || self.bench.is_some() || self.model.tabs.generation == self.tabs_written {
            return;
        }
        if let (Some(path), Some(key)) = (theme::settings_path(), self.tabs_key.as_deref()) {
            match theme::save_tabs(&path, key, &self.model.tabs.saved()) {
                Ok(()) => self.tabs_written = self.model.tabs.generation,
                Err(e) => eprintln!("synaps-desktop: tabs not saved: {e}"),
            }
        }
    }

    /// The composer holds the text of the tab it types for (the current
    /// session tab, never a view on screen): the one guard of `submit()`
    /// and `sync_composer()`.
    fn composer_owned(&self) -> bool {
        Drafts::composer_owned(self.composer_of.as_ref(), &self.model.tabs)
    }

    /// Send the composer's text. `false` keeps it in the composer.
    fn submit(&mut self, text: String) -> bool {
        #[cfg(test)]
        self.submits.push((text.clone(), self.model.watch.focus().is_some()));
        let Some(c) = &self.client else {
            self.model.last_error = Some("offline".into());
            return false;
        };
        // The composer's text is the visible tab's only once the frame after
        // a switch has swapped the drafts: never sent to another session.
        if !self.composer_owned() {
            self.model.last_error = Some("switching tabs: not sent".into());
            self.model.view_dirty = true;
            return false;
        }
        if !self.model.can_send() {
            self.model.last_error = Some(if self.model.switching() {
                "switching sessions: not sent".into()
            } else if self.model.active.is_some() {
                "this session is read-only here (observing or reconnecting)".into()
            } else {
                "not attached to a session".into()
            });
            return false;
        }
        // `//text`: `/text`, a message (the TUI's escape), never a command.
        let escaped = text.trim_start().strip_prefix('/').filter(|r| r.starts_with('/')).map(str::to_string);
        let text = match escaped {
            Some(t) => t,
            None => {
                // `/compact`, `/workers cancel sa_3` ...: the whole text one
                // valid invocation of an engine command. The engine runs it
                // (the palette's Commands take this path too; while a turn
                // runs, neither does, as the TUI); its text stays until the
                // daemon has.
                if let Some((name, arg)) = crate::palette::invocation(&text) {
                    if self.run_command(name, arg) {
                        self.cmd_draft = Some((name.to_string(), text));
                    }
                    return false;
                }
                // A command's own words its grammar does not take (a bare
                // `/model`, `/budget time 30`): its usage, the text kept,
                // nothing sent. Anything else is a message, as it always was.
                if let Some(name) = crate::palette::near_miss(&text) {
                    self.watch_note = Some(crate::palette::usage(name).into());
                    self.model.view_dirty = true;
                    return false;
                }
                text
            }
        };
        // In a worker's view, Enter steers that worker (never the session).
        if let Some(target) = self.model.watch.steer_target() {
            let handle = match target {
                Ok(h) => h,
                Err(why) => {
                    self.watch_note = Some(why.into());
                    self.model.view_dirty = true;
                    return false;
                }
            };
            if c.steer_subagent(handle.clone(), text.clone()).is_err() {
                self.model.last_error = Some("the connection thread has stopped".into());
                return false;
            }
            log::debug!(target: "synaps_desktop::send", "steer {handle} ({} chars)", text.chars().count());
            self.model.watch.steer_sent(&handle, &text);
            self.watch_note = None;
            // Handoff out: the composer's bar, then the watched row's.
            self.fx.sent.set(Some((self.fx.clock(), true)));
            return true;
        }
        let busy = self.model.transcript.busy();
        let sent = if busy { c.steer(text.clone()) } else { c.submit(text.clone()) };
        if sent.is_err() {
            self.model.last_error = Some("the connection thread has stopped".into());
            return false;
        }
        log::debug!(target: "synaps_desktop::send", "{} ({} chars)", if busy { "steer" } else { "submit" }, text.chars().count());
        self.model.sent(text);
        self.draft_note = None;
        self.watch_note = None;
        // Handoff out: the composer's bar, then the new block's (a steer
        // lands in no block: the composer's alone).
        self.fx.sent.set(Some((self.fx.clock(), busy)));
        true
    }

    /// Ctrl/Alt+↑ / ↓: walk the workers' views ([`crate::watch`]). Not
    /// under Settings or Usage. At either end the composer's hint says so.
    fn worker_key(&mut self, step: Step, cx: &mut Context<Self>) {
        if self.model.tabs.view().is_some() {
            return;
        }
        // A worker tab shows its one worker: its session's tab walks them.
        if self.model.tabs.current_worker().is_some() {
            self.watch_note = Some(format!("this tab shows one worker: its session's tab walks them ({WORKER_KEY})").into());
            cx.notify();
            return;
        }
        let running = if self.model.active.is_some() { self.model.watchable() } else { Vec::new() };
        let cwd = self.model.cwd().map(str::to_string);
        let moved = self.model.watch.step(step, &running, cwd.as_deref(), Instant::now());
        log::debug!(target: "synaps_desktop::watch", "key {step:?}: {moved:?}");
        self.watch_note = match moved {
            Moved::Worker(_) => {
                self.watch_opened();
                None
            }
            Moved::Main => None,
            Moved::Oldest => Some(format!("the oldest worker: {WORKER_KEY} or Esc goes back").into()),
            Moved::No if self.model.watch.candidates(&running).is_empty() => Some("no workers to watch".into()),
            Moved::No => Some(format!("on the session's thread: {WORKER_KEY} watches its workers").into()),
        };
        cx.notify();
    }

    /// A worker's view came on: without a daemon it says nothing comes.
    pub(crate) fn watch_opened(&mut self) {
        self.watch_note = None;
        if self.client.is_none() {
            self.model.watch.no_daemon("offline: there is no daemon to read this worker's thread from");
        }
    }

    /// Back to the session's thread (`esc`: by Esc, which guards the next).
    pub(crate) fn leave_worker(&mut self, esc: bool, cx: &mut Context<Self>) {
        // A worker tab's MAIN: its session's tab (opened if closed).
        if let Some(t) = self.model.tabs.current().filter(|t| t.kind == crate::tabs::Kind::Worker) {
            let Some(session) = t.session.clone() else { return };
            match self.model.tabs.find(&session) {
                Some(ix) => self.focus_tab(ix),
                None => {
                    let fallback = self.model.tab_fallback(&session);
                    self.open_tab(Open::Attach { session_id: session, fallback, open: false }, Place::New);
                }
            }
            cx.notify();
            return;
        }
        if esc {
            self.model.watch.leave_on_esc(Instant::now());
        } else {
            self.model.watch.focus_main();
        }
        self.watch_note = None;
        cx.notify();
    }

    /// The watch poll's timer, (re)set for when the next page is due: by a
    /// frame, a reply or the poll itself, without drawing one. Only while
    /// the window is visible and no Settings or Usage is on screen: a
    /// window that shows nothing (minimized, covered) asks for nothing
    /// after the poll on its way.
    fn arm_watch_poll(&mut self, cx: &mut Context<Self>) {
        let due = self.model.watch.poll_due().filter(|_| self.visible && self.model.tabs.view().is_none());
        let Some(due) = due else {
            self.watch_poll = None;
            return;
        };
        if self.watch_poll.as_ref().is_some_and(|(at, _)| *at == due) {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let left = due.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                cx.background_executor().timer(left).await;
            }
            let _ = this.update(cx, |v, cx| v.watch_poll_now(cx));
        });
        self.watch_poll = Some((due, task));
    }

    fn watch_poll_now(&mut self, cx: &mut Context<Self>) {
        self.watch_poll = None;
        let now = Instant::now();
        if self.client.is_none() {
            self.model.watch.no_daemon("offline: there is no daemon to read this worker's thread from");
            cx.notify();
        } else if let Some((handle, since)) = self.model.watch.take_poll(now) {
            log::debug!(target: "synaps_desktop::watch", "watch {handle} since {since}");
            if !self.client.as_ref().is_some_and(|c| c.watch_subagent(handle, since).is_ok()) {
                self.model.watch.poll_failed(now);
            }
        }
        // The timeout's (or the retry's) timer; the reply arms the next.
        self.arm_watch_poll(cx);
    }

    /// The window was shown or hidden: polls stop while it shows nothing.
    fn visibility_changed(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        self.arm_watch_poll(cx);
        // Hidden, the heartbeat stops; shown again, the next frame re-arms it.
        self.fx.set_visible(visible);
        if visible {
            cx.notify();
        }
    }

    /// The visible tab shows its session as last seen (the tab cache, or
    /// nothing yet) while its attach is on its way: nothing on screen is
    /// live.
    pub(crate) fn opening(&self) -> bool {
        self.model.active.is_none() && self.model.tabs.current_session().is_some() && !self.model.stats.detached
    }

    /// A daemon is connected and this attachment may drive it (the
    /// synthetic transport never can), or the bench's life scene says its
    /// local session is this window's ([`Self::owner_here`]).
    pub(crate) fn drives(&self) -> bool {
        (self.client.is_some() || self.owner_here) && self.model.can_send()
    }

    /// The list's top edge moved by `shift` (the bars above it changed
    /// form), and its view `grew` at the bottom (the card and composer
    /// gave up room) or not. Following, nothing to do: the tail stays at
    /// the bottom. Scrolled back, the list scrolls by `shift` too, so the
    /// rows stay where they are on screen. Near the start a rise is
    /// followed only as far as the first row; the fall that undoes it
    /// gives back what was left over (unless the list was scrolled
    /// meanwhile), so a round trip ends where it began. A view grown to the
    /// transcript's end would set the list following the tail (GPUI does
    /// at the end): `hold_back` stops it 2 px short instead, after the
    /// layout, still scrolled back.
    fn keep_rows(&mut self, shift: f32, grew: bool) {
        // Between short and cramped the bars stay: what is owed stays owed.
        let carry = if shift != 0. { self.scroll_carry.take() } else { None };
        if self.list.is_following_tail() {
            return;
        }
        self.hold_back.set(grew);
        let (mut by, mut left) = (shift, 0.);
        if shift > 0.
            && let Some((owed, at)) = carry
            && at == self.list.logical_scroll_top()
        {
            by -= owed;
        } else if shift < 0. {
            let offset = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
            left = (-shift - offset).max(0.);
        }
        if by != 0. {
            self.list.scroll_by(px(by));
        }
        if left > 0. {
            self.scroll_carry = Some((left, self.list.logical_scroll_top()));
        }
    }

    /// The scroller's frame (fluid.md §3.2): before the list's layout, the
    /// list moved by what the wheel, the finger or the coast did since the
    /// last frame. `pad_b`: the list's bottom padding (its end is past it;
    /// its top padding is 0, [`Self::render`]).
    fn scroll_frame(&mut self, pad_b: f32) {
        if !self.scroller.active() {
            return;
        }
        if self.list.is_scrollbar_dragging() || self.fx.level() != crate::motion::Level::Full {
            return self.scroller.cancel();
        }
        let room = crate::scroll::Room {
            offset: -f32::from(self.list.scroll_px_offset_for_scrollbar().y),
            max: f32::from(self.list.max_offset_for_scrollbar().y) + pad_b,
            end_known: self.list.is_scrolled_to_end().is_some(),
        };
        let step = self.scroller.frame(self.fx.now(), room);
        if step.follow {
            self.list.scroll_to_end();
        } else if step.by != 0. {
            self.list.scroll_by(px(step.by));
        }
        if self.scroller.active() {
            self.fx.want_frame();
        }
    }

    /// The column changed (launch, a session, a tab, a worker's view): it
    /// arrives as one layer, at its final layout, lifting 70 → 100 % and
    /// travelling [`crate::motion::SHIFT`] → 0 on `glide` (fluid.md §3.6),
    /// from its first frame on screen:
    /// - another tab: from that tab's side (the side it is on as the change
    ///   happens; a closed tab's, from where it was);
    /// - a worker's view: from below (out of the tray and blocks); back to
    ///   the session from it: from above;
    /// - another session in the same tab (the rail, ↑↓): from the side its
    ///   rail row is on, relative to the one before (fluid.md §3.6);
    /// - otherwise (launch): from below.
    ///
    /// Keyed by what is drawn: the session whose rows are on screen (a tab's
    /// kept rows show before its attach lands; the attach changes nothing),
    /// the worker view, the tab's id (closing, reopening or dragging another
    /// tab moves nothing). A change within 150 ms of the last is instant: a
    /// held ↓ never stacks them, and a switch straight back shows no slide.
    /// Settings and Usage draw in their own pane: no layer. Full only.
    fn column_layer(&mut self) {
        let tabs = &self.model.tabs;
        let key = (self.rows_session.clone(), self.motion_column(), tabs.visible().map(|t| t.id));
        if key == self.column_key {
            return;
        }
        let (worker, view) = (self.model.watch.focus().is_some(), tabs.view().is_some());
        // Nothing to draw yet (an attach in flight, nothing kept for it):
        // the column it lands on is the change.
        if !worker && !view && self.disp.is_empty() && self.model.active.is_none() {
            return;
        }
        let now = self.fx.now();
        let rapid = self.column_at.is_some_and(|t| now.saturating_duration_since(t) < crate::motion::RAPID);
        // A session's row in the rail (its place, top to bottom).
        let rail = &self.rail.rail;
        let row_of = |s: &Option<String>| {
            let s = s.as_deref()?;
            rail.items.iter().position(|it| match it {
                crate::rail::Item::Row(r) => rail.rows[*r].live.and_then(|i| self.model.sessions.get(i)).is_some_and(|m| m.id == s),
                _ => false,
            })
        };
        let ix = tabs.active();
        let (dx, dy) = match (self.column_key.2, key.2) {
            (Some(a), Some(b)) if a != b && !worker => {
                let from = tabs.iter().position(|t| t.id == a).or(self.column_ix);
                let left = matches!((ix, from), (Some(n), Some(o)) if n < o);
                (if left { -1. } else { 1. }, 0.)
            }
            _ if self.column_worker && !worker => (0., -1.),
            (a, b) if a == b && !worker && self.column_key.0.is_some() && self.column_key.0 != key.0 => {
                match (row_of(&self.column_key.0), row_of(&key.0)) {
                    (Some(o), Some(n)) if n < o => (0., -1.),
                    _ => (0., 1.),
                }
            }
            _ => (0., 1.),
        };
        (self.column_key, self.column_ix, self.column_worker, self.column_at) = (key, ix, worker, Some(now));
        self.layer = (self.fx.fluid() && !rapid && !view).then_some((now, dx, dy));
    }

    /// The column's layer this frame: its offset and strength, or none.
    fn layer_now(&mut self) -> Option<(f32, f32, f32)> {
        let (at, dx, dy) = self.layer?;
        let t = self.fx.now().saturating_duration_since(at).as_secs_f32();
        let d = crate::motion::Spring::from_rest(crate::motion::SHIFT, crate::motion::GLIDE_RESPONSE, t);
        let q = (t / crate::motion::QUICK.as_secs_f32()).min(1.);
        let strength = crate::motion::LIFT + (1. - crate::motion::LIFT) * crate::motion::ease::OUT.sample(q);
        if d < 0.5 && q >= 1. || !self.fx.fluid() {
            self.layer = None;
            return None;
        }
        self.fx.want_frame();
        Some((dx * d, dy * d, strength))
    }

    /// Row `ix` as it arrives or grows (fluid.md §3.3, §3.4): a row that
    /// arrived while watched reveals its height, its content rising
    /// [`crate::motion::ARRIVE`] and lifting from 70 %; the streaming row's
    /// new lines glide in. History, Reduced and Off: the row as is.
    fn arrive(&self, ix: usize, row: turns::RowParts) -> AnyElement {
        let Some(d) = self.disp.get(ix) else { return row.whole() };
        let streaming = self.synced_busy && ix + 1 == self.disp.len() && d.kind == Kind::Prose;
        let key = d.motion_key();
        match self.fx.grow(key, ix, self.fx.birth(key), streaming, self.body_line) {
            // Your own message was readable a frame ago: it doesn't lift.
            Some(mut g) => {
                if d.kind == Kind::User {
                    g.strength = 1.;
                }
                crate::motion::grow_block(row.body.into_any_element(), row.label, row.head, g)
            }
            None => row.whole(),
        }
    }

    /// Rows the list has no height for (a load, a switch, a new width: not
    /// drawn yet) count 0 px to GPUI, so the thumb would fill most of the
    /// track and shrink as history is read. They get a hint, the mean height
    /// their text comes to ([`turns::mean_height`]; GPUI hints all alike);
    /// each row's height replaces it once drawn. The last row must have its height: rows
    /// streaming in below a scrolled-back view keep 0 px until drawn (a
    /// guess there could put the end where it is not, and a drag to it
    /// would follow). Once hinted every row has a height, and this is a few
    /// lookups a frame.
    fn hint_rows(&self, m: &Metrics) {
        let (l, n) = (&self.list, self.disp.len());
        if !wants_hint(l, n) {
            return;
        }
        // The rows not drawn are above the view (rows below it are drawn
        // or keep 0 px): their mean, not the drawn tail's.
        let top = l.logical_scroll_top().item_ix.min(n);
        let above = if top > 0 { &self.disp[..top] } else { &self.disp[..] };
        let h = turns::mean_height(above, &self.rows, m, turns::cpl(self.layout.column, m));
        if h > 0. {
            // A builder, on a clone: `ListState` is shared, so the hint lands
            // on the list itself.
            let _ = l.clone().with_uniform_item_height(px(h));
        }
    }

    /// The card, if it shows the attached session's first pending prompt.
    pub(crate) fn current_card(&self) -> Option<&PromptCard> {
        self.card.as_ref().filter(|c| self.model.card_key() == Some(c.key))
    }

    /// The prompt the card shows, if it is still the attached session's and
    /// armed: what a click or Enter on the card answers.
    fn card_target(&self) -> Option<u64> {
        let c = self.card.as_ref().filter(|c| c.armed())?;
        (self.model.card_key() == Some(c.key)).then_some(c.key.1)
    }

    /// Answer the card's prompt (`Client::answer_on` the connection it came
    /// on: dropped if the session reconnected meanwhile), if it is the one shown
    /// (`key`) and the model allows it (attached, no switch under way, not
    /// answered yet). A confirm takes `y` / `n` (the daemon's own spelling);
    /// a secret its text, or nothing to cancel.
    pub(crate) fn answer(&mut self, key: (u64, u64), value: Option<String>) -> bool {
        let Some(c) = &self.client else {
            self.model.last_error = Some("offline: there is no daemon to answer".into());
            return false;
        };
        if self.card.as_ref().map(|c| c.key) != Some(key) || self.card_target() != Some(key.1) || !self.model.may_answer(key.1) {
            return false;
        }
        let connection = self.model.active.as_ref().map_or(0, |a| a.connection);
        // `SYNAPS_DESKTOP_LOG=info`: what went, never a secret's text.
        use crate::authorize::Answer;
        let (what, answer) = match &value {
            Some(v) if v == CONFIRM_YES => ("yes", Answer::Allowed),
            Some(v) if v == synaps_client::protocol::CONFIRM_ANSWER_ALLOW_ALL => ("allow all", Answer::AllowedAll),
            Some(v) if v == CONFIRM_NO => ("no", Answer::Denied),
            Some(_) => ("a secret", Answer::Provided),
            None => ("cancel", Answer::Cancelled),
        };
        let took = self.card.as_ref().map_or(Duration::ZERO, |c| c.shown.elapsed());
        match c.answer_on(connection, key.1, value) {
            Ok(()) => {
                log::info!(target: "synaps_desktop::card", "card {key:?}: answered {what}");
                self.model.answer_sent(key.1);
                self.sent = Some(crate::authorize::Sent { key, answer, at: crate::authorize::hhmm_now(), took });
                true
            }
            Err(_) => {
                self.model.last_error = Some("the connection thread has stopped".into());
                false
            }
        }
    }

    /// What the composer says of its text instead of its hint: a worker
    /// key's or a refused steer's note, else the draft's.
    fn composer_note(&self) -> Option<SharedString> {
        self.watch_note.clone().or_else(|| self.draft_note.map(SharedString::from))
    }

    /// A note's colour: a command that went out or was answered, quiet
    /// (`dim_u`); anything else (refused, not run, a warning) `warn`.
    pub(crate) fn note_color(&self, note: &SharedString, p: &Palette) -> Hsla {
        if self.quiet_note.as_ref() == Some(note) { p.dim_u } else { p.warn }
    }

    /// What the centre column draws besides the bars, the card and the
    /// composer ([`crate::fit::Extras`]).
    pub(crate) fn extras(&self) -> crate::fit::Extras {
        let n = &self.model.named;
        crate::fit::Extras {
            naming: n.naming && self.model.active.is_some(),
            naming_notes: u8::from(n.rename_error.is_some()) + u8::from(self.replace_offer().is_some()),
            error: self.model.last_error.is_some(),
            note: self.watch_note.is_some() || self.draft_note.is_some(),
            stub: self.stub_shown(),
            tray: self.tray_rows as u8,
        }
    }

    /// Motion's session scope ([`crate::motion::Motion::rescope`]): the
    /// session on screen and its attach (a reconnect is history).
    fn motion_session(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.model.active.as_ref().map(|a| (&a.session_id, a.epoch)).hash(&mut h);
        h.finish()
    }

    /// Motion's column scope: the session's thread or a worker's view (in
    /// place or a tab), each with its own item ids.
    fn motion_column(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.model.watch.focus(), self.model.watch.in_tab()).hash(&mut h);
        h.finish()
    }

    /// The session tab is on screen and its card shows the first prompt.
    /// Not over a worker's view: the card is the session's, and Esc there
    /// goes back to it.
    fn card_up(&self) -> bool {
        self.model.tabs.view().is_none() && self.model.watch.focus().is_none() && self.current_card().is_some()
    }

    /// The card's keys act: it is on screen in the active window with its
    /// buttons whole (a tiled window can be shorter than its minimum), no
    /// overlay is open (the theme picker, the gear's menu, a worker's, a
    /// session's or the workers menu: Tab steps them; the palette: Tab
    /// flips its mode), and nothing
    /// typed in has the focus (the window's own), or a secret prompt's
    /// field does (`Some(true)`: Enter there submits). The composer, find
    /// and the other fields keep Enter and Esc.
    fn card_keys_live(&self, window: &Window, cx: &App) -> Option<bool> {
        let shown = self.card_on_screen();
        let menu = self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some() || self.palette.is_some();
        if !self.card_up() || !shown || self.picker_open || self.gear_menu || menu || !window.is_window_active() {
            None
        } else if self.secret_input.read(cx).focus_handle(cx).is_focused(window) {
            Some(true)
        } else {
            (self.focus.is_focused(window) || self.allow_all_focus.is_focused(window)).then_some(false)
        }
    }

    /// Tab or Shift+Tab with the card's keys: to its Allow all box, or
    /// from it back to the card.
    fn card_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to = if self.allow_all_focus.is_focused(window) { &self.focus } else { &self.allow_all_focus };
        window.focus(to, cx);
        cx.notify();
    }

    /// Tab (or Shift+Tab, `back`) in an open menu: its selection steps.
    fn menu_tab(&mut self, back: bool, cx: &mut Context<Self>) {
        if let Some((m, _)) = self.menu.as_mut() {
            m.key("tab", back, true);
        } else if let Some((m, _)) = self.smenu.as_mut() {
            m.key("tab", back, true);
        } else if let Some(m) = self.wmenu.as_mut() {
            m.key("tab", back, true);
        }
        cx.notify();
    }

    /// L3: keys that reach the window may be the rest of a secret: while a
    /// secret card is up, and after one went until the typing stops
    /// ([`ARM`] without a key here), `/` does not open find (a field that
    /// would show the rest in plain text, and send it as a session name on
    /// Enter). The only key here that takes text; Ctrl/Cmd keys are not
    /// typed in a secret.
    fn text_keys_blocked(&mut self, now: Instant) -> bool {
        let up = self.card_up() && self.current_card().is_some_and(|c| c.secret);
        self.secret_left = still_typing(self.secret_left, now);
        up || self.secret_left.is_some()
    }

    /// Repaint the card when its arm runs out (its buttons, and the keys
    /// they show, light up).
    pub(crate) fn arm_repaint(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ARM).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
    }

    /// The card's keys went live: they wait [`ARM`] from now.
    fn restart_card_keys(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.card.as_mut() {
            log::info!(target: "synaps_desktop::card", "card {:?}: keys live, waiting", c.key);
            c.arm.restart(Instant::now());
            c._arm = Some(Self::arm_repaint(cx));
        }
    }

    /// A Ctrl+Enter or Esc reached the card (its keys are live): true if
    /// it may answer ([`Arm::key`]: a new press, armed). Keys that were not
    /// live at the last frame went live with it, so it waits too.
    fn card_key_armed(&mut self, cx: &mut Context<Self>) -> bool {
        let press = self.chord_press;
        let fresh = !std::mem::replace(&mut self.keys_live, true);
        let Some(c) = self.card.as_mut() else { return false };
        let now = Instant::now();
        if fresh {
            c.arm.restart(now);
        }
        let ok = c.arm.key(now, press);
        log::info!(target: "synaps_desktop::card", "card {:?}: key (new press: {press}) {}", c.key, if ok { "acts" } else { "waits" });
        if !ok {
            c._arm = Some(Self::arm_repaint(cx));
            cx.notify();
        }
        ok
    }

    /// The card's prompt, if this window may answer it now (as its
    /// buttons: driving, not yet answered, armed).
    pub(crate) fn card_answerable(&self) -> Option<(&PromptCard, bool)> {
        let c = self.current_card()?;
        let pr = self.model.transcript.pending_prompts().first()?;
        (self.drives() && self.model.may_answer(pr.id) && c.armed()).then_some((c, pr.is_secret()))
    }

    /// Ctrl/Cmd+Enter with the card's keys live and armed. A confirm:
    /// AUTHORIZE, once the whole body has been on screen (as the button);
    /// until then nothing. A secret: its field takes the focus (Enter there
    /// submits what was typed).
    fn card_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((c, secret)) = self.card_answerable() else { return };
        let key = c.key;
        if secret {
            self.secret_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.authorize(key);
        }
        cx.notify();
    }

    /// Page Down / Page Up / Down / Up with the card's keys live: the well
    /// a page (a line) further on or back, the keyboard's way to its end.
    fn card_scroll(&mut self, down: bool, page: bool, cx: &mut Context<Self>) {
        let Some(c) = self.current_card() else { return };
        let s = &c.scroll;
        let line = theme::metrics(cx).code_line;
        let view = f32::from(s.bounds().size.height);
        let by = if page { (view - 2. * line).max(line) } else { line };
        let (at, max) = (s.offset(), s.max_offset());
        let y = (f32::from(at.y) + if down { -by } else { by }).clamp(-f32::from(max.y), 0.);
        s.set_offset(point(at.x, px(y)));
        cx.notify();
    }

    /// Esc with the card's keys live: DENY, or a secret's CANCEL.
    fn card_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The second of two quick Escs that left a worker's view.
        if self.model.watch.esc_guarded(Instant::now()) {
            return;
        }
        let Some((c, secret)) = self.card_answerable() else { return };
        let key = c.key;
        if secret {
            self.answer_secret(false, window, cx);
        } else {
            self.answer(key, Some(CONFIRM_NO.into()));
            cx.notify();
        }
    }

    /// AUTHORIZE: only once the whole body has been on screen.
    pub(crate) fn authorize(&mut self, key: (u64, u64)) {
        let ready = self.card.as_ref().is_some_and(|c| c.key == key && authorize_ready(c.armed(), c.below(self.list_view()), c.read));
        if ready {
            // Allow all, checked (and still offered: this client owns the input).
            let all = self.allow_all_offered() && self.card.as_ref().is_some_and(|c| c.allow_all);
            let value = if all { synaps_client::protocol::CONFIRM_ANSWER_ALLOW_ALL } else { CONFIRM_YES };
            self.answer(key, Some(value.into()));
        }
    }

    /// A secret prompt's answer (`submit`) or cancel, for the prompt the
    /// card shows ([`secret_answer`]: never an empty one); the field is
    /// cleared only when it went out.
    pub(crate) fn answer_secret(&mut self, submit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.card.as_ref().map(|c| c.key) else { return };
        let Some(value) = secret_answer(submit, &self.secret_input.read(cx).value()) else { return };
        if self.answer(key, value) {
            self.secret_input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    /// The card's prompt resolved: its receipt (this window's answer, or
    /// answered elsewhere) and, with its row on screen under Full, its
    /// collapse ([`CardExit`]).
    fn resolve_card(&mut self, fx_now: Instant, cx: &App) {
        let Some(c) = self.card.take() else { return };
        let Some(sid) = self.model.active.as_ref().map(|a| a.session_id.clone()) else { return };
        let items = self.model.transcript.items();
        let Some(last) = items.last().map(|i| i.id) else { return };
        let (after, own) = c.after.get().unwrap_or_else(|| match turns::waiting_call(items, Some(&c.pr)).filter(|_| !c.secret) {
            Some(call) => (call, false),
            None => (last, true),
        });
        // The call it belongs to, by its tool-use id (Shady H2): the one it
        // sits under, or for its own line the call it waited in (else the
        // last call before it in its turn, never an earlier turn's). None:
        // no receipt (nothing to hang it on), but it still collapses.
        let ix = items.iter().rposition(|i| i.id == after);
        let call = ix.and_then(|ix| {
            let gated = turns::waiting_call(items, Some(&c.pr)).and_then(|id| items.iter().rposition(|i| i.id == id));
            let at = if own { gated.filter(|&g| g <= ix).unwrap_or(ix) } else { ix };
            let turn = items[..=at].iter().rposition(|i| i.kind == ItemKind::User).unwrap_or(0);
            items[turn..=at].iter().rev().find_map(|i| i.tool.as_ref()).map(|t| SharedString::from(t.tool_id.clone()))
        });
        let mine = self.sent.take().filter(|s| s.key == c.key);
        let (answer, at, took) = match mine {
            Some(s) => (s.answer, s.at, Some(s.took)),
            None => (self.resolved_by(&c, &sid), crate::authorize::hhmm_now(), None),
        };
        log::info!(target: "synaps_desktop::card", "card {:?}: resolved ({answer:?})", c.key);
        let line = call.is_some();
        if let Some(call) = call {
            let receipts = self.receipts.entry(sid.clone()).or_default();
            receipts.push(crate::authorize::Receipt { call, own, row: c.row, secret: c.secret, answer, at, took, landed: fx_now });
            if receipts.len() > crate::authorize::RECEIPTS {
                receipts.remove(0);
            }
        }
        let (top, bottom) = self.list_view();
        let on_screen = self.model.watch.focus().is_none()
            && self.model.tabs.view().is_none()
            && self
                .disp
                .iter()
                .rposition(|d| d.kind == Kind::Prompt && d.id == c.row)
                .and_then(|ix| self.list.bounds_for_item(ix))
                .is_some_and(|b| f32::from(b.top()) < bottom && f32::from(b.bottom()) > top);
        if on_screen && self.fx.fluid() && !c.height.get().is_nan() {
            let to = if own && line { crate::authorize::receipt_line(&theme::metrics(cx)) } else { 0. };
            let look = self.card_look.borrow().clone();
            self.card_exit = Some(CardExit { card: c, look, at: fx_now, session: sid, after, to, sink: Rc::new(Cell::new(f32::NAN)) });
        }
    }

    /// What resolved a prompt this window did not answer (Shady M4): a HALT
    /// from here since it was shown, or its turn already over when it
    /// resolved (a cancel, a revoke, a reload's drain), is `cancelled` (the
    /// tool saw no answer: blocked); another client owning the input,
    /// `answered elsewhere`; else `resolved`, saying nothing it does not
    /// know. Whether the turn ran is read as the resolution came (Shady N3:
    /// a window that drew nothing meanwhile sees the turn over by now).
    fn resolved_by(&self, c: &PromptCard, sid: &str) -> crate::authorize::Answer {
        use crate::authorize::Answer;
        let halted = self.halted.as_ref().is_some_and(|(s, at)| s == sid && *at >= c.shown);
        let ran = self.model.transcript.resolved_busy(c.key.1).unwrap_or_else(|| self.model.transcript.busy());
        if halted || !ran || self.model.reload {
            return Answer::Cancelled;
        }
        match self.model.active.as_ref() {
            Some(a) if a.owner.is_some() && a.owner != Some(a.client) => Answer::Elsewhere,
            _ => Answer::Resolved,
        }
    }

    /// Keep the card's state with the prompt it shows ([`PromptCard::sync`]).
    /// Another card, or none, clears what the secret field holds (its undo
    /// history too): it was meant for the card that went. A secret card
    /// that went leaves `/` dead while its typing goes on
    /// ([`AppView::text_keys_blocked`]). A new card takes the keys if the
    /// secret field has them (L5: what is typed next is not for this
    /// prompt) or nothing does (rare: the focus-lost fallback has usually
    /// given them to the window already, which is why a bare Enter must not
    /// answer): never from the composer or another field. Then keys that
    /// came back (the card came into view, an overlay closed, the focus or
    /// the window came to it) wait again.
    fn sync_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.list_view();
        let now = Instant::now();
        let was = self.card.as_ref().map(|c| (c.key, c.secret));
        // The collapse lands; a switch, a reconnect or motion turned down
        // ends it at once.
        let fx_now = self.fx.now();
        if let Some(e) = &self.card_exit {
            let here = self.model.active.as_ref().is_some_and(|a| a.session_id == e.session && a.epoch == e.card.key.0);
            // Another prompt in its place ends it at once, as the dock's did.
            let other = self.model.card_key().is_some_and(|k| k != e.card.key);
            if e.shown(fx_now).1 || !here || other || !self.fx.fluid() || self.model.watch.focus().is_some() {
                self.card_exit = None;
            }
        }
        // A refused (or lost) answer: forgotten, the card asks again.
        if self.sent.as_ref().is_some_and(|s| self.model.card_key() == Some(s.key) && !self.model.prompt_answered(s.key.1)) {
            self.sent = None;
        }
        // Its prompt resolved in its attachment (answered here or elsewhere):
        // its receipt, and (on screen, Full) its row collapses.
        let resolved = self
            .card
            .as_ref()
            .zip(self.model.active.as_ref())
            .is_some_and(|(c, a)| c.key.0 == a.epoch && !self.model.transcript.pending_prompts().iter().any(|p| p.id == c.key.1));
        if resolved {
            self.resolve_card(fx_now, cx);
        }
        let new = PromptCard::sync(&mut self.card, &self.model, now, view);
        if was.map(|w| w.0) != self.card.as_ref().map(|c| c.key) {
            // The box's focus was the card's that went (the next card's
            // box is its own).
            if self.allow_all_focus.is_focused(window) {
                window.focus(&self.focus, cx);
            }
            if was.is_some_and(|w| w.1) {
                self.secret_left = Some(now);
            }
            self.secret_input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if new
            && let Some(c) = self.card.as_mut()
            && let Some(a) = self.model.active.as_ref()
        {
            let k = (a.session_id.clone(), c.key.1, c.pr.raised_at.clone());
            match self.card_first.iter().find(|f| f.0 == k) {
                Some(f) => c.shown = f.1,
                None => {
                    if self.card_first.len() == 16 {
                        self.card_first.remove(0);
                    }
                    self.card_first.push((k, c.shown));
                }
            }
        }
        if let Some(c) = self.card.as_mut() {
            c.box_focus = self.allow_all_focus.is_focused(window);
        }
        if new && let Some(c) = &self.card {
            log::info!(target: "synaps_desktop::card", "card {:?}: shown", c.key);
            if window.focused(cx).is_none() || self.secret_input.read(cx).focus_handle(cx).is_focused(window) {
                window.focus(&self.focus, cx);
            }
            let task = Self::arm_repaint(cx);
            if let Some(c) = self.card.as_mut() {
                c._arm = Some(task);
            }
        }
        // Back on screen from behind a worker's view (or Settings, Usage):
        // its buttons and keys wait again, as a new card's do. Nothing
        // that was not drawn counts as seen.
        let up = self.card_up();
        if up && !self.card_was_up && !new {
            if let Some(c) = self.card.as_mut() {
                log::info!(target: "synaps_desktop::card", "card {:?}: back on screen, waiting", c.key);
                c.arm = Arm::new(now);
                c._arm = Some(Self::arm_repaint(cx));
            }
            self.keys_live = false;
        }
        self.card_was_up = up;
        let live = self.card_keys_live(window, cx).is_some();
        if live && !self.keys_live {
            self.restart_card_keys(cx);
        }
        self.keys_live = live;
    }

    /// Open the picker with the focus on this view, so Esc reaches the
    /// root's key listener even if the focused input has since gone
    /// (a card answered, a name bar closed): GPUI dispatches a vanished
    /// focus at the window's root, above this view.
    pub(crate) fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = true;
        self.picker_note = None;
        self.config_theme = self.config_choice().0.into();
        // It opens at its top, as it did before it kept a scroll handle.
        self.bars.picker.scroll.set_offset(point(px(0.), px(0.)));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Close the picker; focus in its hex field moves back to this view.
    pub(crate) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = false;
        if self.hex_input.read(cx).focus_handle(cx).is_focused(window) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// The accent override (`None`: the theme's own `prompt_fg` and
    /// `border_active`), repainted at once and saved.
    pub(crate) fn set_accent(&mut self, hex: Option<u32>, cx: &mut Context<Self>) {
        theme::update(cx, |c| c.accent = hex);
        let overridden = match self.accent_source {
            theme::Source::Flag => " (the next launch's --accent wins over it)".to_string(),
            theme::Source::Env => format!(" (${} wins over it at the next launch)", theme::ENV_ACCENT),
            _ => String::new(),
        };
        self.picker_note = Some(saved_note(theme::settings_path().map(|path| (theme::save_accent(&path, hex), path)), &overridden));
        cx.notify();
    }

    /// A built-in theme (or the `<base>/themes/<name>` file over it),
    /// repainted at once and saved.
    pub(crate) fn set_theme(&mut self, name: &'static str, cx: &mut Context<Self>) {
        let base = synaps_client::discovery::base_dir();
        let Some(raw) = theme::lookup(name, Some(&base)) else { return };
        self.show_theme(name.to_string(), raw, Some(name), cx);
    }

    /// SYNAPS CONFIG: no picked theme any more (`"theme": null`); the
    /// theme the TUI's config names, read now, as at launch.
    pub(crate) fn follow_config(&mut self, cx: &mut Context<Self>) {
        let (name, raw) = self.config_choice();
        self.config_theme = name.clone().into();
        self.show_theme(name, raw, None, cx);
    }

    /// What following the Synaps config shows: `$SYNAPS_THEME`, else the
    /// profile's `theme` file or `theme =` line, else nord.
    pub(crate) fn config_choice(&self) -> (String, crate::palettes::Raw) {
        let env = std::env::var(theme::ENV_THEME).ok();
        let base = synaps_client::discovery::base_dir();
        let (name, raw, _, _) = theme::follow_config(env.as_deref(), &base, self.theme_profile.as_deref());
        (name, raw)
    }

    /// Repaint with a theme and save the choice (`saved`: `None` follows
    /// the config).
    fn show_theme(&mut self, name: String, raw: crate::palettes::Raw, saved: Option<&str>, cx: &mut Context<Self>) {
        theme::update(cx, |c| {
            c.theme = name;
            c.raw = raw;
        });
        self.theme_follows_config = saved.is_none();
        let overridden = match self.theme_source {
            theme::Source::Flag => " (the next launch's --theme wins over it)".to_string(),
            theme::Source::Env if saved.is_some() => format!(" (${} wins over it at the next launch)", theme::ENV_THEME),
            _ => String::new(),
        };
        self.picker_note = Some(saved_note(theme::settings_path().map(|path| (theme::save_theme(&path, saved), path)), &overridden));
        cx.notify();
    }

    /// Ctrl+= / Ctrl+- (`Some(±1)`), Ctrl+0 (`None`): the centre pane's
    /// text size. One repaint, one re-measure of the list; saved with the
    /// panel states, [`SAVE_DELAY`] later.
    fn step_text(&mut self, dir: Option<i8>, cx: &mut Context<Self>) {
        let now = self.panels.text_size;
        let Some(next) = (match dir {
            Some(d) => Metrics::bump(now, d),
            None => Some(theme::DEFAULT_TEXT),
        })
        .filter(|&n| n != now) else {
            return;
        };
        self.show_text_size(next, cx);
        let mut p = self.panels;
        p.text_size = next;
        self.set_panels(p, cx);
    }

    /// The centre pane's text at step `next` (one of [`theme::TEXT_STEPS`]):
    /// the metrics, and one re-measure of the list.
    pub(crate) fn show_text_size(&mut self, next: u8, cx: &mut App) {
        theme::update(cx, |c| c.text = next);
        self.list.remeasure();
        self.metrics_gen += 1;
    }

    pub(crate) fn apply_custom_accent(&mut self, cx: &mut Context<Self>) {
        let typed = self.hex_input.read(cx).value().to_string();
        match theme::parse_hex(&typed) {
            Some(hex) => self.set_accent(Some(hex), cx),
            None => {
                self.picker_note = Some("not a colour: use #RRGGBB".into());
                cx.notify();
            }
        }
    }

    /// Fold or unfold a side panel (`rail`: the sessions rail, else the
    /// right panel) as [`toggled`] says: its width flies on `sheet`
    /// (fluid.md §3.8, [`crate::chrome::PanelFx`]); saved [`SAVE_DELAY`]
    /// later. The column keeps its wrap until it lands.
    pub(crate) fn toggle_panel(&mut self, rail: bool, cx: &mut Context<Self>) {
        // A fold mid-drag commits the drag where it is first; the rest of
        // GPUI's drag then moves nothing (`drag_edge` needs `self.drag`).
        self.end_drag(cx);
        self.drag = None;
        let Some(p) = toggled(self.width, self.panels, rail) else { return };
        self.fly_to(p, crate::motion::SHEET_RESPONSE);
        self.set_panels(p, cx);
    }

    /// The panels go to `p` on their springs (fluid.md §3.8): each one whose
    /// width changes flies there from where it is drawn, on `response`; one
    /// that folds or unfolds slides (`sheet`).
    pub(crate) fn fly_to(&mut self, p: theme::Panels, response: f32) {
        let (a, b) = (sides(self.width, self.panels, crate::chrome::MARK), sides(self.width, p, crate::chrome::MARK));
        for (rail, was, now) in [(true, a.0, b.0), (false, a.1, b.1)] {
            if was != now {
                let slide = matches!(was, Side::Strip) != matches!(now, Side::Strip);
                self.panel_fx.fly(rail, if slide { crate::motion::SHEET_RESPONSE } else { response }, slide);
            }
        }
    }

    /// Every row's height changes with the text size: re-measured now (a
    /// cached tab's when it comes back).
    pub(crate) fn text_metrics_changed(&mut self) {
        self.list.remeasure();
        self.metrics_gen += 1;
    }

    pub(crate) fn set_panels_pub(&mut self, p: theme::Panels, cx: &mut Context<Self>) {
        self.set_panels(p, cx);
    }

    /// New panel states, saved [`SAVE_DELAY`] after the last change.
    fn set_panels(&mut self, p: theme::Panels, cx: &mut Context<Self>) {
        let before = std::mem::replace(&mut self.panels, p);
        self.panels_changed(before, cx);
        cx.notify();
    }

    /// The panels changed from `before`: the keys that differ are saved
    /// [`SAVE_DELAY`] later.
    fn panels_changed(&mut self, before: theme::Panels, cx: &mut Context<Self>) {
        self.panels_dirty = self.panels_dirty.or(theme::PanelKeys::changed(before, self.panels));
        if self.bench.is_none() {
            self.panels_save = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SAVE_DELAY).await;
                let _ = this.update(cx, |v, cx| v.save_panels(cx));
            }));
        }
    }

    /// The mouse went down on a panel's edge handle (`rail`, else the right
    /// panel's) at `x`.
    pub(crate) fn grab_edge(&mut self, rail: bool, x: f32) {
        let (l, r) = sides(self.width, self.panels, crate::chrome::MARK);
        let width = if rail { l.width() } else { r.width() };
        self.drag = Some(EdgeGrab { rail, x, start: self.panels, width, moved: false, want: width });
    }

    /// The pointer is at `x` during a drag of an edge (anywhere: GPUI
    /// reports the moves of its drag outside the handle too).
    fn drag_edge(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(g) = self.drag.as_mut() else { return };
        g.moved = true;
        let width = if g.rail { g.width + (x - g.x) } else { g.width - (x - g.x) };
        g.want = width;
        let p = dragged(self.width, g.start, g.rail, width);
        if p != self.panels {
            self.panels = p;
            cx.notify();
        }
    }

    /// The drag let go: what it left is saved, once.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.drag.take_if(|g| g.moved) {
            // Let go near a detent: the panel springs there (`snap`).
            if let Some(p) = detent(self.width, g.start, self.panels, g.rail, g.want) {
                self.fly_to(p, crate::motion::SNAP_RESPONSE);
                self.springback = true;
                self.panels = p;
                cx.notify();
            }
            self.panels_changed(g.start, cx);
        }
    }

    /// A click on an edge handle: a double-click resets the panel to its
    /// auto width; a click on a strip's edge opens it, as the strip does.
    pub(crate) fn click_edge(&mut self, rail: bool, count: usize, cx: &mut Context<Self>) {
        self.drag = None;
        let (l, r) = sides(self.width, self.panels, crate::chrome::MARK);
        if count >= 2 {
            let mut p = self.panels;
            if rail {
                p.rail_width = None;
            } else {
                p.right_width = None;
            }
            self.fly_to(p, crate::motion::SHEET_RESPONSE);
            self.set_panels(p, cx);
        } else if (if rail { l } else { r }) == Side::Strip {
            self.toggle_panel(rail, cx);
        }
    }

    /// Ctrl+B / Ctrl+Alt+B (Cmd on macOS): a press folds or unfolds, its
    /// key repeats do nothing; its key-down decides ([`OnceKey`]).
    fn fold_key(&mut self, rail: bool, cx: &mut Context<Self>) {
        self.once = Some(OnceKey::Fold(rail));
        cx.propagate();
    }

    /// Write the changed panel keys on the background executor (a read, a
    /// write, two fsyncs: never on the UI thread).
    fn save_panels(&mut self, cx: &mut Context<Self>) {
        // Never a width in the middle of a drag: its end saves again.
        if self.drag.as_ref().is_some_and(|g| g.moved) {
            return;
        }
        let keys = std::mem::take(&mut self.panels_dirty);
        let Some(path) = theme::settings_path().filter(|_| !keys.is_empty()) else { return };
        self.panels_writing = self.panels_writing.or(keys);
        let p = self.panels;
        cx.spawn(async move |this, cx| {
            let saved = cx.background_spawn(async move { theme::save_panels(&path, p, keys) }).await;
            let _ = this.update(cx, |v, cx| {
                v.panels_writing = theme::PanelKeys::default();
                if let Err(e) = saved {
                    v.panels_dirty = v.panels_dirty.or(keys);
                    v.model.last_error = Some(format!("panel layout not saved: {e}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A save still waiting (or still being written), written now, here:
    /// the window is closing or the app quitting.
    fn flush_panels(&mut self) {
        self.panels_save = None;
        let keys = std::mem::take(&mut self.panels_dirty).or(std::mem::take(&mut self.panels_writing));
        if self.bench.is_none()
            && !keys.is_empty()
            && let Some(path) = theme::settings_path()
            && let Err(e) = theme::save_panels(&path, self.panels, keys)
        {
            eprintln!("synaps-desktop: panel layout not saved: {e}");
        }
    }

    /// Push transcript changes into the virtual list: the item strings of
    /// the changed tail are rebuilt, then the display rows from the turn
    /// holding it ([`turns::build`]); only rows that differ are spliced
    /// (and so re-measured). A turn that ends can fold its last run.
    pub fn sync_list(&mut self) {
        let pane_before = self.pane_of.clone();
        let session_before = self.rows_session.clone();
        self.sync_pane();
        let busy = self.model.shown().busy();
        let mut splice = self.model.shown_mut().take_splice();
        // A splice that adds items (a delta to the last one adds no row).
        let live_splice = splice.as_ref().is_some_and(|sp| sp.start > 0 && sp.new_len > sp.old_len && sp.new_len <= sp.old_len + 16)
            && !self.full_rebuild;
        if std::mem::take(&mut self.full_rebuild) {
            let n = self.model.shown().len();
            splice = Some(synaps_client::transcript::Splice { start: 0, old_len: self.rows.len(), new_len: n });
        }
        let items = self.model.shown().items();
        let from = match &splice {
            Some(sp) => {
                // The oldest rows dropped (a worker's bound): the list drops
                // theirs and keeps every other row, its height and the
                // scroll on it; the rest syncs as any change does.
                let trimmed = if sp.start == 0 { front_trimmed(&self.rows, items) } else { 0 };
                if trimmed > 0 {
                    let k = self.disp.partition_point(|d| d.item < trimmed);
                    self.list.splice(0..k, 0);
                    self.disp.drain(..k);
                    self.disp.iter_mut().for_each(|d| d.shift(trimmed));
                    self.rows.drain(..trimmed);
                    // The cut runs' summaries and folds go with them.
                    if let Some(first) = items.first().map(|i| i.id) {
                        self.summaries.retain_from(first);
                        self.toggled.retain(|id| *id >= first);
                        self.open_runs.retain(|id| *id >= first);
                    }
                }
                if trimmed > 0 || items.len() - sp.start > REUSE_ROWS {
                    // A long tail rebuilt (an attach, a history replace):
                    // rows whose item kept its id and rev keep their strings,
                    // so a cached tab's re-attach rebuilds only what changed.
                    let mut old: HashMap<(u64, u64), RowCache> =
                        self.rows.drain(sp.start.min(self.rows.len())..).map(|r| ((r.id, r.rev), r)).collect();
                    for item in &items[sp.start..] {
                        let mut r = old
                            .remove(&(item.id, item.rev))
                            .unwrap_or_else(|| row_cache(item, self.model.shown_files(), self.model.shown_subagents()));
                        r.retime(item);
                        self.rows.push(r);
                    }
                } else {
                    self.rows.truncate(sp.start);
                    for item in &items[sp.start..] {
                        self.rows.push(row_cache(item, self.model.shown_files(), self.model.shown_subagents()));
                    }
                }
                debug_assert!(self.rows.iter().zip(items).all(|(r, i)| r.id == i.id && r.rev == i.rev && r.at == i.at));
                if sp.start == 0 {
                    // Another session: what was unfolded there means nothing here.
                    let session = self.model.active.as_ref().map(|a| a.session_id.clone());
                    if session != self.rows_session {
                        self.rows_session = session;
                        self.summaries.clear();
                        self.toggled.clear();
                        self.open_runs.clear();
                    }
                }
                sp.start
            }
            None if busy != self.synced_busy => items.len(),
            // A prompt's row came, went or changed (it rebuilds from its turn).
            None if crate::authorize::built(&self.disp) != self.prompt_rows() => items.len(),
            None => {
                self.sync_counts();
                return self.sync_subagents();
            }
        };
        self.waiting_on = turns::waiting_call(items, self.model.shown().pending_prompts().first());
        let start = turns::turn_start(items, from);
        self.synced_busy = busy;
        // Rows that arrive while watched (a tail splice of a few items, not
        // an attach, a history load or another session's list) draw in:
        // a burst staggered 30 ms each (motion.md §6.1).
        let live = self.fx.on() && live_splice && self.pane_of == pane_before && self.rows_session == session_before;
        let before: HashSet<u64> =
            if live { self.disp.iter().skip_while(|d| d.item < start).map(|d| d.id).collect() } else { HashSet::new() };
        self.rebuild_from(start);
        if live {
            // The motion clock's time (in tests, the executor's), as every
            // reveal reads it.
            let now = self.fx.clock();
            let fresh = self.disp.iter().skip_while(|d| d.item < start).filter(|d| !before.contains(&d.id));
            for (k, d) in fresh.enumerate() {
                self.fx.born(d.motion_key(), now + crate::motion::stagger(k));
            }
        }
        self.sync_counts();
        self.sync_subagents();
    }

    /// The list holds the rows of the transcript on screen: when a worker's
    /// view comes on, the session's list (rows, folds, heights, scroll)
    /// waits in [`AppView::main_pane`] and a fresh one is built for the
    /// worker; when it goes, the worker's is dropped and the session's
    /// comes back exactly as it was left.
    fn sync_pane(&mut self) {
        let want = self.model.watch.focus_key();
        if want == self.pane_of.as_ref().map(|(h, g)| (h.as_str(), *g)) {
            return;
        }
        let want = want.map(|(h, g)| (h.to_string(), g));
        let left = self.take_pane();
        match self.pane_of.take() {
            None => self.main_pane = Some(left),
            // A worker tab's: kept for when it is shown again.
            Some(k) if self.model.holds_view(&k.0) => {
                self.worker_panes.retain(|(w, _)| *w != k);
                self.worker_panes.push((k, left));
            }
            Some(_) => {}
        }
        self.drop_released_panes();
        match (&want, self.main_pane.take()) {
            (None, Some(main)) => {
                let stale = main.metrics_gen != self.metrics_gen;
                self.put_pane(main);
                if stale {
                    self.list.remeasure();
                }
            }
            // Never: the session's list is always kept. Built again.
            (None, None) => self.full_rebuild = true,
            (Some(k), main) => {
                self.main_pane = main;
                match self.worker_panes.iter().position(|(w, _)| w == k) {
                    Some(i) => {
                        let (_, pane) = self.worker_panes.swap_remove(i);
                        let stale = pane.metrics_gen != self.metrics_gen;
                        self.put_pane(pane);
                        if stale {
                            self.list.remeasure();
                        }
                    }
                    None => {
                        self.rows_session = self.model.active.as_ref().map(|a| a.session_id.clone());
                        self.full_rebuild = true;
                    }
                }
            }
        }
        self.pane_of = want;
    }

    /// Worker tabs' lists stay only while their tab is open and their view
    /// is the same one ([`crate::watch::Watch::hold`]).
    pub(crate) fn drop_released_panes(&mut self) {
        let (model, panes) = (&self.model, &mut self.worker_panes);
        panes.retain(|((h, g), _)| model.holds_view(h) && model.watch.generation_of(h) == Some(*g));
    }

    /// The kept text views ([`turns::TextRows`]), and how many of them are
    /// for no row of the list on screen (an item id whose text is not
    /// theirs, nor theirs streamed on).
    #[cfg(test)]
    pub(crate) fn text_views(&self) -> (usize, usize) {
        let texts = self.texts.borrow();
        let stale = texts.kept().filter(|&(id, t)| !self.rows.iter().any(|r| r.id == id && r.text.starts_with(t.as_str()))).count();
        (texts.kept().count(), stale)
    }

    /// The list on screen gives way to another (a tab, a worker's view, a
    /// new transcript): a drag on its bar ends on it, its hold-back is
    /// dropped (it was this list's), and the text views kept for its rows
    /// go (the next list numbers its items from 1 too).
    fn list_out(&mut self) {
        // Its scroller's moves are relative: none carries to the next list.
        self.scroller.cancel();
        self.bars.transcript.release();
        self.list.scrollbar_drag_ended();
        self.hold_back.set(false);
        // An unfold's hold was this list's rows.
        self.unfold_hold.clear();
        self.texts.borrow_mut().clear();
    }

    /// The list's state, handed out; a fresh, empty list in its place.
    fn take_pane(&mut self) -> Pane {
        self.list_out();
        let fresh = ListState::new(0, ListAlignment::Top, px(600.));
        fresh.set_follow_mode(FollowMode::Tail);
        turns::readiness(&fresh, &self.texts);
        Pane {
            rows: std::mem::take(&mut self.rows),
            disp: std::mem::take(&mut self.disp),
            summaries: std::mem::take(&mut self.summaries),
            list: std::mem::replace(&mut self.list, fresh),
            toggled: std::mem::take(&mut self.toggled),
            open_runs: std::mem::take(&mut self.open_runs),
            synced_busy: std::mem::take(&mut self.synced_busy),
            files_gen: std::mem::take(&mut self.files_gen),
            subs_gen: std::mem::take(&mut self.subs_gen),
            waiting_on: self.waiting_on.take(),
            rows_session: self.rows_session.take(),
            scroll_carry: self.scroll_carry.take(),
            metrics_gen: self.metrics_gen,
        }
    }

    fn put_pane(&mut self, p: Pane) {
        self.list_out();
        self.rows = p.rows;
        self.disp = p.disp;
        self.summaries = p.summaries;
        self.list = p.list;
        self.toggled = p.toggled;
        self.open_runs = p.open_runs;
        self.synced_busy = p.synced_busy;
        self.files_gen = p.files_gen;
        self.subs_gen = p.subs_gen;
        self.waiting_on = p.waiting_on;
        self.rows_session = p.rows_session;
        self.scroll_carry = p.scroll_carry;
    }

    /// A subagent's block changed shape (a line came, its state changed)
    /// with no item changing (its events and rows are not the
    /// transcript's): the rows from the turn of the earliest call whose
    /// block did (any: the first worker call's), so its height and its
    /// run's fold follow. Its text alone changing needs nothing: the row
    /// reads it when drawn.
    fn sync_subagents(&mut self) {
        let subs = self.model.shown_subagents_mut();
        if subs.shape_generation() == self.subs_gen {
            return;
        }
        self.subs_gen = subs.shape_generation();
        let changed = subs.take_reshaped().filter(|c| !c.is_empty());
        let items = self.model.shown().items();
        let k = items.iter().position(|i| {
            i.tool.as_ref().is_some_and(|t| match &changed {
                Some(c) => c.contains(&t.tool_id),
                None => crate::subagents::starts_worker(&t.name),
            })
        });
        if let Some(k) = k {
            self.rebuild_from(turns::turn_start(items, k));
        }
    }

    /// The files ledger committed a call since the rows were built (its
    /// result comes with the history, not with the item's status): the
    /// file tools' counts, and the summaries that sum them, follow it.
    fn sync_counts(&mut self) {
        let ledger = self.model.shown_files();
        if ledger.generation() == self.files_gen {
            return;
        }
        self.files_gen = ledger.generation();
        let items = self.model.shown().items();
        let mut changed = false;
        for (row, item) in self.rows.iter_mut().zip(items) {
            let Some(t) = item.tool.as_ref().filter(|t| files::is_file_tool(&t.name)) else { continue };
            let d = ledger.delta(&t.tool_id);
            if d != row.delta {
                row.delta = d;
                row.counts = turns::Counts::of(d.unwrap_or_default());
                changed = true;
            }
        }
        if changed {
            self.summaries.clear();
            self.summaries.update(&self.disp, items, &self.rows);
        }
    }

    /// Start the list over on a new transcript (the bench's scenarios): no
    /// row, height or fold of the old one survives.
    pub(crate) fn reset_transcript(&mut self) {
        self.model.transcript = crate::model::transcript();
        self.rows.clear();
        self.disp.clear();
        self.summaries.clear();
        self.toggled.clear();
        self.open_runs.clear();
        self.list_out();
        self.list.reset(0);
    }

    /// Display rows from item `start` (a turn's start) on.
    fn rebuild_from(&mut self, start: usize) {
        // The prompts' rows (the authorize card, its receipt lines): one
        // that changed before `start` rebuilds from its own turn.
        let prompts = self.prompt_rows();
        let start = {
            let items = self.model.shown().items();
            let old = crate::authorize::built(&self.disp);
            let changed = old.iter().filter(|r| !prompts.contains(r)).chain(prompts.iter().filter(|r| !old.contains(r)));
            changed.map(|r| turns::turn_start(items, r.after + 1)).fold(start, usize::min)
        };
        let shown = self.model.shown();
        let items = shown.items();
        let rows = &self.rows;
        let day = |i: usize| rows.get(i).and_then(|r| r.time.as_ref()).map(crate::times::Stamp::day);
        let new = turns::build(items, &day, start, shown.busy(), &self.open_runs, self.model.shown_subagents());
        let new = crate::authorize::with_prompts(new, &prompts, start);
        self.summaries.update(&new, items, &self.rows);
        let at = self.disp.partition_point(|d| d.item < start);
        if let Some((range, count)) = turns::diff(&self.disp[at..], &new) {
            self.list.splice(at + range.start..at + range.end, count);
            self.disp.splice(at + range.start..at + range.end, new[range.start..range.start + count].iter().copied());
        }
        // A full build per sync: bounded, so long sessions stay usable in dev builds.
        debug_assert!(
            items.len() > 2_000
                || self.disp
                    == crate::authorize::with_prompts(
                        turns::build(items, &day, 0, shown.busy(), &self.open_runs, self.model.shown_subagents()),
                        &prompts,
                        0
                    )
        );
    }

    /// Open by default: a file tool's diff when it is short (unless the
    /// call failed, was denied or canceled: nothing changed), and a failed
    /// call's result.
    pub(crate) fn is_open(&self, item: &Item, row: &RowCache) -> bool {
        let status = item.tool.as_ref().and_then(|t| t.status);
        let failed = matches!(status, Some(ToolStatus::Failed | ToolStatus::Denied));
        let default = item.kind == ItemKind::Tool
            && (failed
                || (!matches!(status, Some(ToolStatus::Cancelled))
                    && row.preview.as_ref().is_some_and(|p| p.lines.len() <= 24 && !p.lines.is_empty())));
        default ^ self.toggled.contains(&item.id)
    }

    pub(crate) fn toggle(&mut self, id: u64) {
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
        // A call's row, or a slash command's answer (a note).
        if let Some(ix) = self.disp.iter().position(|d| d.id == id && matches!(d.kind, Kind::Work | Kind::Note | Kind::Event)) {
            self.list.remeasure_items(ix..ix + 1);
        }
    }

    /// Fold or unfold a finished run (by its first item's id); the user's
    /// choice sticks.
    pub(crate) fn toggle_run(&mut self, id: u64) {
        // Events may have landed since the last frame: the rows catch up
        // first, so the rebuild below never runs ahead of them.
        self.sync_list();
        if !self.open_runs.remove(&id) {
            self.open_runs.insert(id);
        }
        // The clicked header stays put (motion.md §6.1): the list keeps its
        // top row and offset, tail-follow paused (it resumes at the end).
        self.scroller.cancel();
        if self.list.is_following_tail() {
            let top = self.list.logical_scroll_top();
            self.list.pause_following_tail();
            self.list.scroll_to(top);
        }
        let before: HashSet<u64> = self.disp.iter().filter(|d| d.kind == Kind::Work).map(|d| d.id).collect();
        let items = self.model.shown().items();
        if let Some(start) = items.iter().position(|i| i.id == id).map(|ix| turns::turn_start(items, ix + 1)) {
            self.rebuild_from(start);
        }
        // The cascade: the rows the unfold brought draw their bars from the
        // header down; a fold draws the tally's segments.
        let now = self.fx.clock();
        self.fx.fold.set(Some((id, now)));
        let fresh: Vec<u64> = self.disp.iter().filter(|d| d.kind == Kind::Work && !before.contains(&d.id)).map(|d| d.id).collect();
        for (k, id) in fresh.iter().enumerate() {
            self.fx.born(*id, now + crate::motion::stagger(k));
        }
        // They reveal from 0 px (Full), so the list is still at its end the
        // frame after: GPUI's tail-follow would resume there and carry the
        // header up as they grow. Paused until they have (fluid.md §3.3).
        // A second unfold during a reveal adds its rows: the first run's
        // still grow (Shady #16 r4).
        if self.fx.fluid() {
            // A fold took its rows away: their ids leave the hold with
            // them (Shady #17 L3), not when their reveal would have ended.
            let shown: HashSet<u64> = self.disp.iter().filter(|d| d.kind == Kind::Work).map(|d| d.id).collect();
            self.unfold_hold.retain(|id| shown.contains(id));
            self.unfold_hold.extend(fresh);
        } else {
            self.unfold_hold.clear();
        }
    }

    /// Every finished run folded, or unfolded (the palette's commands): at
    /// once, no cascade (a bulk change is not a reveal). The list keeps its
    /// top row, as one run's fold does; following the tail, it follows on.
    pub(crate) fn fold_all_runs(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sync_list();
        let runs: Vec<u64> = self.disp.iter().filter(|d| matches!(d.kind, Kind::Summary { .. })).map(|d| d.id).collect();
        let before = self.open_runs.len();
        if open {
            self.open_runs.extend(runs);
        } else {
            self.open_runs.clear();
        }
        if self.open_runs.len() == before && (open || before == 0) {
            return;
        }
        self.scroller.cancel();
        // The row at the top before (and how far into it), found again
        // after: itself, or the run's summary that folded it away (at its
        // top). One splice from the first changed row to the last would
        // put the list's top at the first run that changed.
        let following = self.list.is_following_tail();
        let top = self.list.logical_scroll_top();
        let was = self.disp.get(top.item_ix).copied();
        self.unfold_hold.clear();
        self.rebuild_from(0);
        if !following && let Some(was) = was {
            let summary = |d: &Disp| matches!(d.kind, Kind::Summary { .. });
            let same = self.disp.iter().position(|d| d.id == was.id && summary(d) == summary(&was));
            let into =
                || self.disp.iter().position(|d| matches!(d.kind, Kind::Summary { end, .. } if d.item <= was.item && was.item < end));
            let to = match same {
                Some(ix) => ListOffset { item_ix: ix, offset_in_item: top.offset_in_item },
                None => {
                    ListOffset { item_ix: into().unwrap_or(top.item_ix.min(self.disp.len().saturating_sub(1))), offset_in_item: px(0.) }
                }
            };
            self.list.scroll_to(to);
        }
        cx.notify();
    }

    /// An unfold's rows revealing ([`Self::unfold_hold`]): tail-follow
    /// stays paused, the clicked header where it was. Before the list's
    /// layout, each frame.
    fn hold_unfold(&mut self) {
        // Rows that have finished leave first: a hold with none left
        // pauses nothing (Shady #16 r4).
        let fx = &self.fx;
        self.unfold_hold.retain(|&id| fx.revealing(id));
        if !self.unfold_hold.is_empty() && self.list.is_following_tail() {
            self.list.pause_following_tail();
        }
    }

    /// What the daemon said of the session on screen, each part if known:
    /// detached, model, effort, folder.
    fn session_src(&self) -> (bool, Option<&str>, Option<&str>, Option<&str>) {
        let view = self.model.stats.view.as_ref();
        let model = view.map(|v| v.model.as_str()).filter(|m| !m.is_empty()).or(self.model.active_meta().map(|s| s.model.as_str()));
        let effort = view.map(|v| v.effort.as_str()).filter(|e| !e.is_empty());
        let cwd = self.model.stats.cwd.as_deref().or(self.model.active_meta().and_then(|s| s.cwd.as_deref()));
        (self.model.active.is_none() && self.model.stats.detached, model, effort, cwd)
    }

    /// The session on screen's line ([`SessionMeta`]), set. Built when it
    /// is shown (a tooltip on hover).
    pub(crate) fn session_meta(&self) -> SessionMeta {
        let (detached, model, effort, cwd) = self.session_src();
        SessionMeta {
            title: self.model.transcript.title().map(|t| t.to_string().into()),
            name: self.model.active_name().map(|n| n.to_string().into()),
            model: model.map(|m| crate::named::short_model(m).into()),
            effort: effort.map(|e| e.to_string().into()),
            cwd: cwd.map(|c| home_short(c).into()),
            detached,
        }
    }

    /// Tab `id`'s session line, for its tooltip: the session on screen's
    /// in full, another's as its list entry says it (no effort: only an
    /// attached session's view says it).
    pub(crate) fn tab_meta(&self, id: u64) -> Option<SessionMeta> {
        let t = self.model.tabs.iter().find(|t| t.id == id)?;
        let session = t.session.as_deref()?;
        if self.model.is_active(session) {
            return Some(self.session_meta());
        }
        let m = self.model.sessions.iter().find(|m| m.id == session)?;
        Some(SessionMeta {
            title: m.title.clone().filter(|t| !t.is_empty()).map(Into::into),
            name: m.name.clone().filter(|n| !n.is_empty()).map(Into::into),
            model: (!m.model.is_empty()).then(|| crate::named::short_model(&m.model).into()),
            effort: None,
            cwd: m.cwd.as_deref().filter(|c| !c.is_empty()).map(|c| home_short(c).into()),
            detached: false,
        })
    }

    /// The session's folder for the right pane's FILES head (design.md
    /// §8.18): shortened from the left to `chars`, so its leaf stays
    /// (`…/work/relay-gateway`), and the whole of it for the tooltip.
    pub(crate) fn session_cwd(&self, chars: usize) -> Option<(SharedString, SharedString)> {
        let (_, _, _, cwd) = self.session_src();
        let whole = home_short(cwd?);
        Some((left_cut(&whole, chars).into(), whole.into()))
    }

    /// F2, the session menu's Rename…, a double-click on its tab's title:
    /// the naming row of the session on screen opens (or closes).
    pub(crate) fn rename_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_screen = self.model.tabs.view().is_none()
            && self.model.tabs.current().and_then(|t| t.session.as_deref()).is_some_and(|s| self.model.is_active(s));
        if !on_screen {
            return;
        }
        self.model.toggle_naming();
        if self.model.named.naming {
            let current = self.model.active_name().unwrap_or_default().to_string();
            self.name_input.update(cx, |s, cx| {
                s.set_value(current, window, cx);
                s.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// A rename asked for from another session's tab or row: once it is on
    /// screen and attached, its naming row opens.
    /// Anything else shown first (another tab or session, a view), or an
    /// attach that did not land within [`RENAME_WAIT`], and the intent is
    /// gone: a naming row never opens later, under someone's typing.
    fn rename_when_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((s, at)) = self.rename_pending.as_ref() else { return };
        let shown = self.model.tabs.view().is_none() && self.model.tabs.current().and_then(|t| t.session.as_deref()) == Some(s.as_str());
        if !shown || at.elapsed() > RENAME_WAIT {
            self.rename_pending = None;
            return;
        }
        if !self.model.is_active(s) {
            return;
        }
        self.rename_pending = None;
        if !self.model.named.naming {
            self.rename_session(window, cx);
        }
    }

    /// The live session rail row `key` holds.
    fn rail_session(&self, key: u64) -> Option<String> {
        let row = self.rail.rail.rows.iter().find(|r| r.key == key)?;
        self.model.sessions.get(row.live?).map(|m| m.id.clone())
    }

    /// Favourite a rail row's session, or unfavourite it, by what the row
    /// is: held, a favourite (its own row) or listed
    /// ([`Model::toggle_fav_row`]). Looked up by the row's id in the lists
    /// as they are now, never by an index the rail was built with (a poll
    /// may have replaced the lists since). Desktop only: nothing is sent.
    pub(crate) fn toggle_fav_row(&mut self, key: u64) {
        let Some(row) = self.rail.rail.rows.iter().find(|r| r.key == key) else { return };
        let (action, fav) = (row.action.clone(), row.fav.is_some());
        if self.model.toggle_fav_row(&action, fav, crate::chrome::now_secs()) {
            self.favs_changed();
        }
    }

    /// Favourite a held session (its tab, the key), or unfavourite it.
    pub(crate) fn toggle_fav_session(&mut self, session_id: &str) {
        let m = &mut self.model;
        let Some(s) = m.sessions.iter().find(|s| s.id == session_id) else { return };
        m.favs.toggle(crate::favourites::Source::Live(s), crate::chrome::now_secs());
        self.favs_changed();
    }

    /// A gone favourite's Remove.
    pub(crate) fn remove_fav(&mut self, id: &str) {
        if self.model.favs.remove(id) {
            self.favs_changed();
        }
    }

    fn favs_changed(&mut self) {
        self.model.rail_gen += 1;
        self.model.view_dirty = true;
    }

    /// A text field has the keyboard (the composer, find, a name, a colour,
    /// a secret, Settings' fields): keys are its text, not the app's.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.composer.read(cx).focus_handle(cx).is_focused(window)
            || [&self.find_input, &self.name_input, &self.hex_input, &self.secret_input, &self.settings_find, &self.settings_input]
                .iter()
                .any(|i| i.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The favourite key: the session tab on screen. Not while typing (a
    /// TUI hand's Ctrl+D there is no toggle of the favourites).
    pub(crate) fn fav_key_pressed(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.typing(window, cx) {
            return;
        }
        let Some(id) = self.model.tabs.current().filter(|t| t.kind == crate::tabs::Kind::Session).and_then(|t| t.session.clone()) else {
            return;
        };
        self.toggle_fav_session(&id);
        cx.notify();
    }

    /// FAVOURITES folded or opened (saved, as the buckets are).
    pub(crate) fn toggle_favs_group(&mut self, cx: &mut Context<Self>) {
        let mut p = self.panels;
        p.favs_closed = !p.favs_closed;
        self.rail.fold = true;
        self.set_panels(p, cx);
        cx.notify();
    }

    /// A session's menu at `pt` (a right-click on its tab or rail row).
    pub(crate) fn open_session_menu(&mut self, at: SessionAt, pt: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (session, tab, row) = match at {
            SessionAt::Tab(id) => {
                let Some(t) = self.model.tabs.iter().find(|t| t.id == id && t.kind == crate::tabs::Kind::Session) else { return };
                let Some(s) = t.session.clone() else { return };
                (s, Some(id), None)
            }
            SessionAt::Row(key) => {
                // A saved row's menu has no live session (Favourite only).
                let s = self.rail_session(key).unwrap_or_default();
                let tab =
                    (!s.is_empty()).then(|| self.model.tabs.find(&s).and_then(|ix| self.model.tabs.iter().nth(ix)).map(|t| t.id)).flatten();
                (s, tab, Some(key))
            }
        };
        let fav = match row {
            Some(key) => self.rail.rail.rows.iter().find(|r| r.key == key).map(|r| r.fav.is_some()),
            None => self.model.sessions.iter().find(|m| m.id == session).map(|m| self.model.fav_of(m).is_some()),
        };
        let for_ = MenuFor { tab: tab.is_some(), live: !session.is_empty(), fav };
        self.close_menu();
        // The row's readout is not drawn under its menu.
        self.readout = None;
        self.readout_wait = None;
        if let Some(prev) = window.focused(cx).filter(|f| *f != self.focus)
            && self.menu_focus_back.is_none()
        {
            self.menu_focus_back = Some(prev);
        }
        window.focus(&self.focus, cx);
        self.gear_menu = false;
        let menu = crate::menu::Menu::new(session_items(for_), crate::menu::At::Point(pt), false).born(self.fx.clock());
        self.smenu = Some((menu, SessionTarget { session, tab, row }));
        cx.notify();
    }

    /// An item of the session menu picked, `how`.
    pub(crate) fn session_menu_pick(&mut self, act: SessionAct, how: crate::menu::How, window: &mut Window, cx: &mut Context<Self>) {
        if !how.is_press() {
            return;
        }
        let Some((_, t)) = self.smenu.take() else { return };
        self.close_menu();
        match act {
            SessionAct::Rename => {
                let ix = t.tab.and_then(|id| self.model.tabs.iter().position(|x| x.id == id));
                match (ix, t.row) {
                    (Some(ix), _) => self.focus_tab(ix),
                    (None, Some(key)) => self.rail_click(key, crate::tabs::Place::Current),
                    (None, None) => {}
                }
                self.rename_pending = Some((t.session, Instant::now()));
                self.rename_when_shown(window, cx);
            }
            SessionAct::Close => {
                if let Some(ix) = t.tab.and_then(|id| self.model.tabs.iter().position(|x| x.id == id)) {
                    self.close_tab(ix);
                }
            }
            SessionAct::Favourite => match t.row {
                Some(key) => self.toggle_fav_row(key),
                None => self.toggle_fav_session(&t.session),
            },
        }
        cx.notify();
    }

    /// A key while the session menu is open: `true` if it took it.
    fn session_menu_keydown(&mut self, ks: &Keystroke, press: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((m, _)) = self.smenu.as_mut() else { return false };
        match m.key(&ks.key, ks.modifiers.shift, press) {
            crate::menu::Key::Moved => {}
            crate::menu::Key::Pick(a) => {
                self.menu_picked = true;
                self.session_menu_pick(a, crate::menu::How::Key { press }, window, cx);
            }
            crate::menu::Key::Close => self.close_menu(),
            crate::menu::Key::Other => {
                self.close_menu();
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// The session menu, if open: at the root, over everything.
    fn render_session_menu(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let (m, phase) = match self.smenu.as_ref() {
            Some((m, _)) => (m, crate::menu::Phase::enter(&self.fx, m.born, -1.)),
            None => {
                let (m, at) = self.smenu_leaving.as_ref()?;
                (m, crate::menu::Phase::leave(&self.fx, *at, -1.)?)
            }
        };
        let crate::menu::At::Point(pt) = m.at else { return None };
        let body = crate::menu::render("session-menu", m, p, AppView::session_menu_pick, |this, _| this.close_menu(), phase, &self.fx, cx);
        Some(deferred(anchored().position(pt).snap_to_window_with_margin(px(8.)).child(body)).with_priority(3))
    }

    /// The worker on screen as the composer foot says it: its state as its
    /// tab, its tray row and its menu say it of this worker (its call and
    /// name: another worker under its handle now, after a cold-started
    /// daemon numbered its workers from 1 again, is GONE), and its facts:
    /// its tray row's (tools · elapsed) when that row is this worker's,
    /// else its view's own.
    pub(crate) fn watched_state(&self, w: &crate::watch::Worker) -> (Liveness, SharedString) {
        let session = self.model.active.as_ref().map(|a| a.session_id.as_str()).or(self.model.tabs.current_session());
        let live = session
            .and_then(|s| self.worker_state(s, &w.handle_id, Some(w.key())))
            // Its session in no list here: what its own pages said.
            .unwrap_or_else(|| Liveness::of(crate::subagents::Status::word(&w.status), true, w.gone.is_none()));
        let row = self.tray.rows.iter().find(|r| {
            r.id == w.subagent_id && !r.facts.is_empty() && crate::wtabs::same_worker((r.call.as_deref(), r.name.as_ref()), w.key())
        });
        (live, row.map_or_else(|| w.facts.clone(), |r| crate::tray::ticked(&self.fx, &r.facts, &r.tick)))
    }

    /// HALT is drawn: this window drives the session's running turn (in a
    /// worker's view too).
    pub(crate) fn halt_shown(&self) -> bool {
        self.model.transcript.busy() && self.drives()
    }

    /// What is on screen, as far as HALT's arm goes: the visible tab, the
    /// attached session, and whether a worker's view is up.
    fn halt_place(&self) -> (Option<u64>, Option<&str>, bool) {
        (
            self.model.tabs.visible().map(|t| t.id),
            self.model.active.as_ref().map(|a| a.session_id.as_str()),
            self.model.watch.focus().is_some(),
        )
    }

    /// HALT is armed for what is on screen now.
    fn halt_here(&self) -> bool {
        self.halt_where.as_ref().map(|(t, s, w)| (*t, s.as_deref(), *w)) == Some(self.halt_place())
    }

    /// The arm ends: another tab, session or view is on screen (a second
    /// Esc there never halts the turn the first was for), or it was used.
    pub(crate) fn disarm_halt(&mut self) {
        self.halt_armed = None;
        self.halt_tick = None;
        self.halt_where = None;
    }

    /// HALT: the session's turn stops (the app's one abort).
    pub(crate) fn halt(&mut self) {
        self.disarm_halt();
        if let Some(c) = &self.client
            && self.model.can_send()
        {
            let _ = c.abort();
            self.halted = self.model.active.as_ref().map(|a| (a.session_id.clone(), Instant::now()));
            #[cfg(test)]
            {
                self.halts += 1;
            }
        }
    }

    /// An Esc the composer did not use (no card up, the session's thread
    /// on screen, a new press): the first arms HALT for [`ARM`], its label
    /// `err`; a second within it halts. Whether the Esc was HALT's.
    fn halt_escape(&mut self, now: Instant, cx: &mut Context<Self>) -> bool {
        // The second of two quick Escs that left a worker's view is not the
        // session's.
        if !self.halt_shown() || self.model.watch.esc_guarded(now) {
            self.disarm_halt();
            return false;
        }
        if self.halt_armed.is_some_and(|at| now.saturating_duration_since(at) < ARM) && self.halt_here() {
            self.halt();
        } else {
            self.halt_armed = Some(now);
            self.halt_where = Some({
                let (t, s, w) = self.halt_place();
                (t, s.map(str::to_string), w)
            });
            self.halt_tick = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ARM).await;
                let _ = this.update(cx, |v, cx| {
                    v.halt_tick = None;
                    v.halt_where = None;
                    if v.halt_armed.take().is_some() {
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
        true
    }

    /// Display caps `text` at `size`, as wide as it is drawn: measured once
    /// per size in the embedded face.
    pub(crate) fn display_w(&self, text: &'static str, size: f32, window: &Window) -> f32 {
        let key = (text, size.to_bits());
        if let Some(w) = self.widths.borrow().get(&key) {
            return *w;
        }
        let run = TextRun {
            len: text.len(),
            font: Font { weight: FontWeight::SEMIBOLD, ..font(fonts::DISPLAY) },
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let w = f32::from(window.text_system().shape_line(text.into(), px(size), &[run], None).width).ceil();
        self.widths.borrow_mut().insert(key, w);
        w
    }

    /// The naming row's REPLACE offer: only for the session it was made
    /// for (a switch drops it).
    fn replace_offer(&self) -> Option<&crate::model::Replace> {
        self.model.named.replace.as_ref().filter(|r| self.model.is_active(&r.session_id))
    }

    /// The naming row, under the tab strip (F2, the session menu's Rename…,
    /// a double-click on the session's tab): NAME, its field, SAVE and
    /// CANCEL, then its own lines,
    /// one each (the error, the REPLACE offer; the rest in a tooltip), as
    /// [`crate::fit::Extras`] counts them. Cramped, they are in the field's
    /// row: the error shortened, REPLACE.
    fn render_naming(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let pending = self.model.named.rename.is_some();
        let form = self.layout.form();
        let cramped = form == crate::fit::Form::Cramped;
        let offer = self.replace_offer().map(|r| format!("Replace '{}' with '{}'?", r.current, r.asked));
        let replace = || {
            tbtn("name-replace", label::REPLACE, p.warn, p.ink).flex_none().on_click(cx.listener(|this, _, _, cx| {
                let req = this.model.replace_request();
                this.rename(req);
                cx.notify();
            }))
        };
        let note = |id: &'static str, text: SharedString, color: Hsla| {
            let tip = text.clone();
            mono(text, 12., color)
                .id(id)
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx))
        };
        let error = self.model.named.rename_error.clone();
        let mut row = v_flex().py(px(crate::fit::naming_pad(form))).gap(px(crate::fit::NAMING_GAP)).child(
            h_flex()
                .gap(px(14.))
                .items_center()
                .child(display(label::NAME, 13., p.dim))
                .child(div().flex_1().min_w(px(140.)).max_w(px(320.)).child(Input::new(&self.name_input)))
                .child(tbtn("name-save", if pending { "S A V I N G" } else { label::SAVE }, p.prose, p.ink).on_click(cx.listener(
                    |this, _, _, cx| {
                        let typed = this.name_input.read(cx).value().to_string();
                        let req = this.model.rename_request(&typed);
                        this.rename(req);
                        cx.notify();
                    },
                )))
                .child(tbtn("name-cancel", label::CANCEL, p.dim, p.ink).on_click(cx.listener(|this, _, _, cx| {
                    this.model.named.naming = false;
                    cx.notify();
                })))
                .when(cramped, |d| {
                    let text = match (&error, &offer) {
                        (Some(e), Some(o)) => Some(format!("{e} {o}")),
                        (Some(e), None) => Some(e.to_string()),
                        (None, Some(o)) => Some(o.clone()),
                        (None, None) => None,
                    };
                    // Shares the row with the field (half each, the field at most 320).
                    d.children(text.map(|t| note("name-note", t.into(), p.err).flex_1())).children(offer.is_some().then(replace))
                }),
        );
        if !cramped {
            let line = px(crate::fit::NAMING_NOTE);
            if let Some(e) = error {
                row = row.child(note("name-error", e.into(), p.err).h(line).line_height(line));
            }
            if let Some(o) = offer {
                row = row.child(
                    h_flex().h(line).gap(px(12.)).items_center().child(note("name-offer", o.into(), p.prose).text_px(13.)).child(replace()),
                );
            }
        }
        div().bg(p.canvas).border_b_1().border_color(p.hair).child(column(self.layout, row))
    }

    /// The notice line under the card: the last error and, cramped, the
    /// composer's note (a worker's, else the draft's). With a card up it is
    /// one line, the rest in its tooltip ([`crate::fit::Extras`] counts it
    /// so).
    fn render_notice(&self, p: &Palette, m: &Metrics) -> Option<impl IntoElement> {
        let lay = self.layout;
        let note = self.composer_note().filter(|_| lay.form() == crate::fit::Form::Cramped);
        let (text, color): (SharedString, Hsla) = match (self.model.last_error.clone(), note) {
            (Some(e), Some(n)) => (format!("{e} · {n}").into(), p.err),
            (Some(e), None) => (e.into(), p.err),
            (None, Some(n)) => (n.clone(), self.note_color(&n, p)),
            (None, None) => return None,
        };
        let line = mono(text.clone(), m.meta, color).pt(px(crate::fit::NOTICE_TOP)).line_height(px(crate::fit::notice_line(m)));
        let line = if self.current_card().is_some() || self.card_exit.is_some() {
            line.id("notice")
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .tooltip(move |window, cx| crate::overlay::tip(text.clone(), window, cx))
                .into_any_element()
        } else {
            line.whitespace_normal().into_any_element()
        };
        Some(column(lay, line))
    }

    /// The composer (design.md §8.11): on the user surface, the user's bar
    /// on its left edge. Its last row is the foot: the state of what is on
    /// screen (the session's glyph and word, or over a worker's view MAIN ›
    /// the worker's), model · effort (or the worker's facts), an exceptional
    /// hint, HALT while this window drives a running turn, then SEND (STEER
    /// while a turn runs) filled in `prompt_fg`; what does not fit drops in
    /// [`fit_foot`]'s order. Sized by [`crate::fit::Composer`]; cramped, no
    /// foot: the glyph, HALT and the primary sit beside the text (a note
    /// about the draft keeps its row). `to_card`: it has the focus while a
    /// card waits; its hint says how to answer from the keyboard.
    /// How much the dock has still to grow on its spring: the list's view
    /// ends this much higher once it lands (a reveal aims there).
    pub(crate) fn dock_growth(&self) -> f32 {
        let natural = self.dock_h.get();
        match self.fx.spring_at(DOCK_H) {
            Some(shown) if natural.is_finite() => (natural - shown).max(0.),
            _ => 0.,
        }
    }

    /// The dock over the composer: the NEEDS YOU stub, a notice and the
    /// worker chips (polish.md P13). Under Full its height springs on
    /// `sheet` to what they take (a stub arrives, the chips wrap or unwrap,
    /// the last one leaves), growing upward and clipped; else it takes it
    /// at once. `stub`: it leads with the NEEDS YOU stub, whose frame's
    /// top is drawn at the box's top while the box cuts into it.
    fn dock(&self, parts: Vec<AnyElement>, stub: Option<Hsla>) -> Option<AnyElement> {
        let full = self.fx.level() == crate::motion::Level::Full;
        if parts.is_empty() && self.dock_h.get().is_finite() {
            self.dock_h.set(0.);
        }
        let natural = self.dock_h.get();
        let shown = (full && natural.is_finite()).then(|| self.fx.spring(DOCK_H, None, natural, crate::motion::SHEET_RESPONSE).0);
        if parts.is_empty() && shown.is_none_or(|h| h < 0.5) {
            return None;
        }
        let content = measured(v_flex().w_full().debug_selector(|| "dock".into()).children(parts), &self.dock_h, full);
        Some(match shown {
            Some(h) => {
                // The stub's row starts `STUB_GAP` under the content's top.
                let lay = self.layout;
                let edge = stub.filter(|_| natural - h > crate::fit::STUB_GAP && h >= 1.).map(|c| {
                    div()
                        .debug_selector(|| "dock-edge".into())
                        .absolute()
                        .top_0()
                        .left(px(lay.left))
                        .w(px(lay.column))
                        .h(px(1.))
                        .bg(c)
                        .into_any_element()
                });
                sprung(content, h, "dock-box", edge)
            }
            None => content.into_any_element(),
        })
    }

    fn render_composer(&self, p: &Palette, m: &Metrics, to_card: bool, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let lay = self.layout;
        let dims = crate::fit::Composer::of(m, lay.form());
        let busy = self.model.transcript.busy();
        // Over a worker's view the composer steers it, or takes nothing
        // once it has finished.
        let worker = self.model.watch.focused();
        // Its state and facts, read once (one source for the whole row).
        let watched = worker.map(|w| self.watched_state(w));
        let drives = self.drives();
        // Read-only by its view (it finished, or is gone) or by its state
        // (it ended first by the session's word; another worker under its
        // handle now).
        let read_only = worker.zip(watched.as_ref()).is_some_and(|(w, (live, _))| w.read_only() || live.ended());
        let can_send = drives && worker.is_none_or(|w| !read_only && w.steerable);
        let (detached, model, effort, _) = self.session_src();
        // Only what is out of the ordinary: the routine two are SEND's and
        // STEER's tooltips.
        let hint: Option<(SharedString, Hsla)> = if let Some(n) = self.composer_note() {
            Some((n.clone(), self.note_color(&n, p)))
        } else if self.client.is_none() && !self.synthetic {
            Some(("offline: no daemon to send to".into(), p.dim_u))
        } else if self.client.is_none() {
            Some(("offline: synthetic transport, nothing is sent".into(), p.dim_u))
        } else if self.model.switching() {
            Some(("switching sessions…".into(), p.dim_u))
        } else if !drives && self.opening() {
            Some(("opening…".into(), p.dim_u))
        } else if detached {
            Some(("detached".into(), p.warn))
        } else if !drives {
            Some(("not attached".into(), p.dim_u))
        } else if let Some(w) = worker {
            match watched.as_ref().map(|(live, _)| *live) {
                // Its state says it ended before its view does.
                Some(live) if read_only && !w.read_only() => {
                    let back = if w.tabbed { "" } else { " · Esc returns to the session" };
                    Some((format!("{} {}: read-only{back}", w.name, glyph::worker(live).2).into(), p.dim_u))
                }
                _ => Some((w.hint.clone(), p.dim_u)),
            }
        } else if to_card {
            // The composer keeps its keys (Ctrl/Cmd+Enter sends from here);
            // the card's buttons show none until it has them.
            Some((
                if cfg!(target_os = "macos") {
                    "⌘Enter sends this · Esc, then ⌘Enter answers the card".into()
                } else {
                    "Ctrl+Enter sends this · Esc, then Ctrl+Enter answers the card".into()
                },
                p.dim_u,
            ))
        } else {
            None
        };
        let primary = match worker {
            Some(_) if read_only => label::READ_ONLY,
            Some(_) => label::STEER_WORKER,
            None if busy => label::STEER,
            None => label::SEND,
        };
        let halt = self.halt_shown();
        let cell = 0.6 * m.meta;
        let display_w = |t: &'static str, size: f32| self.display_w(t, size, window);
        // The state part: the session's (main view), or the watched worker's
        // (READ ONLY is the primary's: said once).
        // What motion keys the glyph by ([`glyph::steady`]).
        let mut steady = None;
        let (g, word, family, tally) = match watched.as_ref() {
            None => {
                let st = self.model.tabs.current().map_or(crate::rail::State::Saved, |t| self.tab_state(t));
                let (g, word, family) = glyph::foot(st);
                let tally = match st {
                    crate::rail::State::Workers => Some(self.model.active_workers()),
                    crate::rail::State::NeedsYou => Some(self.model.transcript.pending_prompts().len()).filter(|n| *n > 1),
                    _ => None,
                };
                // READY only where this window can send.
                let word = word.filter(|_| drives || st != crate::rail::State::Idle);
                (g, word, family, tally)
            }
            Some((live, _)) => {
                let (g, word, _, family) = glyph::worker(*live);
                let demoted = worker.is_some_and(|w| self.model.stats.subagents.demoted(w.subagent_id));
                steady = Some(glyph::worker(glyph::steady(*live, demoted)).0);
                (Some(g), Some(word), family, None)
            }
        };
        let (word_color, mark) = glyph::ink(family, Ground::User, p);
        let turn_runs = watched.is_none() && g == Some(glyph::Glyph::Run);
        // Model · effort, or the worker's facts.
        let (model, effort): (Option<SharedString>, Option<SharedString>) = match watched {
            Some((_, f)) => ((!f.is_empty()).then_some(f), None),
            None => (model.map(|m| crate::named::short_model(m).into()), effort.map(|e| SharedString::from(e.to_string()))),
        };
        // GPUI lays a text box out a whole pixel wide.
        let mono_w = |t: &str| (t.chars().count() as f32 * cell).ceil();
        let main_w = display_w(label::MAIN, m.label);
        let primary_w = 32. + display_w(primary, m.label);
        let halt_w = if halt { display_w(label::HALT, m.label) } else { 0. };
        // The word is the column's size (design.md §1.2 `d.s`: 12·r).
        let word_w = word.map_or(0., |w| display_w(w, m.meta) + tally.map_or(0., |n| 8. + mono_w(&count(n))));
        let name_w = worker.map_or(0., |w| (mono_w(&w.name).min(24. * cell)) + 8. + mono_w(&w.handle_id));
        let lead_w = if worker.is_some() { main_w + 8. + cell + 8. + m.lead } else { m.lead };
        let fit = fit_foot(&FootIn {
            room: lay.column - 2. * m.pad_x,
            lead: lead_w,
            word: word_w,
            name: name_w,
            model: if lay.tight { 0. } else { model.as_deref().map_or(0., mono_w) },
            effort: if lay.tight { 0. } else { effort.as_deref().map_or(0., |e| 8. + cell + 8. + mono_w(e)) },
            hint: hint.as_ref().map_or(0., |(h, _)| mono_w(h)),
            halt: halt_w,
            primary: primary_w,
            tight: lay.tight,
        });
        let send = fbtn("send", primary, can_send, Fill::Btn, p, m)
            .when(worker.is_none(), |d| {
                d.tooltip(move |window, cx| {
                    crate::overlay::tip(if busy { "steer: applied at the next turn boundary" } else { "send: starts a turn" }, window, cx)
                })
            })
            .on_click(cx.listener(|this, _, window, cx| {
                let text = this.composer.read(cx).value().trim().to_string();
                if !text.is_empty() && this.submit(text) {
                    this.composer.update(cx, |s, cx| s.set_value("", window, cx));
                }
                cx.notify();
            }));
        let halt_el = halt.then(|| {
            let armed = self.halt_armed.is_some();
            let tip = if worker.is_some() { "Halt the session's turn" } else { "Halt this turn (Esc, Esc)" };
            display(label::HALT, m.label, if armed { p.err } else { p.dim_u })
                .id("halt")
                .debug_selector(|| "halt".into())
                .flex_none()
                .cursor_pointer()
                .hover(|d| d.text_color(p.err))
                .tooltip(move |window, cx| crate::overlay::tip(tip, window, cx))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.halt();
                    cx.notify();
                }))
        });
        let main_btn = || {
            display(label::MAIN, m.label, p.dim_u)
                .id("foot-main")
                .debug_selector(|| "foot-main".into())
                .flex_none()
                .cursor_pointer()
                .hover(|d| d.text_color(p.ink))
                .tooltip(|window, cx| crate::overlay::tip("back to the session · Esc", window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.leave_worker(false, cx)))
        };
        // The glyph cross-fades and scans; NEEDS YOU breathes; a turn that
        // ended here (busy → READY) hands the floor back: Handoff in
        // (motion.md §5.2, §6.4). Keyed by whose foot it is: a switch is not a change.
        let fx = &self.fx;
        // The scope is the session's ([`AppView::motion_session`]).
        let foot_key = worker.map_or(0, |w| (1 << 62) | w.subagent_id);
        let foot_ch = steady.or(g).and_then(|g| fx.change(crate::motion::Tag::Foot, foot_key, crate::turns::glyph_code(g), None));
        let busy_glyph = |c: u64| {
            [glyph::Glyph::Run, glyph::Glyph::Workers, glyph::Glyph::Needs, glyph::Glyph::Cancelling]
                .iter()
                .any(|g| crate::turns::glyph_code(*g) == c)
        };
        let handoff_in =
            foot_ch.filter(|(_, prev)| worker.is_none() && drives && word == Some(label::READY) && busy_glyph(*prev)).map(|(at, _)| at);
        let breath_at = foot_ch.filter(|_| g == Some(glyph::Glyph::Needs)).map(|c| c.0);
        let mut foot_glyph = g.map(|g| {
            div()
                .flex()
                .debug_selector(|| "foot-glyph".into())
                .child(crate::turns::live_glyph(fx, g, mark, p.user, foot_ch, breath_at, true))
                .into_any_element()
        });
        let mut glyph_el = || foot_glyph.take();
        let lit = crate::motion::mix(p.you, p.ink, 0.35);
        let out = fx.flash(fx.sent.get().map(|(at, _)| at));
        let (bar_flash, segment) = match crate::motion::Level::current(cx) {
            crate::motion::Level::Full => (out, fx.draw(handoff_in, Duration::ZERO, crate::motion::SIG)),
            _ => (out.max(fx.flash(handoff_in)), 1.),
        };
        let bar = crate::motion::mix(p.you_mark, lit, bar_flash);
        #[cfg(test)]
        self.composer_bar.set(Some(bar));
        // Handoff in's light (motion.md §10 amendment 17): lit as the beam
        // is, on at once, decaying into the bar over the last `glow` of its
        // travel: it lands as the bar, never switched off.
        let segment_color = crate::motion::mix(bar, crate::motion::mix(p.you, p.ink, HANDOFF_LIT), fx.handoff_light(handoff_in));
        // The composer's top edge (polish.md P2, P3; F10): the shade sweep
        // while this session's turn runs (the TUI's header sweep), then, as
        // the turn ends, one last pass in Handoff's light from the same
        // corner over the same `sig`: one L-shaped light, down the bar and
        // across the top. Full only; Reduced and Off leave the edge unlit.
        let top_edge = if turn_runs {
            Some(crate::motion::sweep(fx, p.run_mark, p.edge))
        } else if segment < 1. && crate::motion::Level::current(cx) == crate::motion::Level::Full {
            Some(crate::motion::pass(fx, segment, crate::motion::mix(p.you, p.ink, HANDOFF_LIT), p.edge))
        } else {
            None
        };
        // A worker tab's glyph says whose; the primary may shorten.
        // Its text at `inset` (design.md §8.11): the kit pads a multi-line
        // field's text by its own `input_px` whatever its appearance, so the
        // field reaches that far past the composer's padding on each side.
        let field = div().mx(px(-kit_input_px())).child(
            Textarea::new(&self.composer)
                .appearance(false)
                .disabled(!drives)
                .text_px(m.body)
                .line_height(px(m.body_line))
                .text_color(p.ink),
        );
        // A line more or less: its box springs there on `flick`, growing
        // upward (the caret's line stays put; fluid.md §3.8).
        let full = self.fx.level() == crate::motion::Level::Full;
        let natural = self.composer_h.get();
        let field = measured(field, &self.composer_h, full);
        let field = match (full && natural.is_finite()).then(|| self.fx.spring(COMPOSER_H, None, natural, crate::motion::FLICK_RESPONSE).0)
        {
            Some(h) => sprung(field, h, "composer-box", None),
            None => field.into_any_element(),
        };
        // Cramped, the draft's note is on the notice line.
        let inline = dims.row_gap.is_none();
        let inner = if inline {
            // Under a 120 px field the row sheds MAIN, then STEER WORKER
            // says STEER (the glyph still names whose).
            let room = lay.column - 2. * m.pad_x;
            let rest = |main: bool, primary: f32| {
                16. + if main { main_w + 8. } else { 0. } + 10. + if halt { 24. + halt_w } else { 0. } + 24. + primary
            };
            let main = worker.is_some() && room - rest(true, primary_w) >= 120.;
            let short = primary == label::STEER_WORKER && room - rest(main, primary_w) < 120.;
            let send = if short {
                fbtn("send", label::STEER, can_send, Fill::Btn, p, m).on_click(cx.listener(|this, _, window, cx| {
                    let text = this.composer.read(cx).value().trim().to_string();
                    if !text.is_empty() && this.submit(text) {
                        this.composer.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                    cx.notify();
                }))
            } else {
                send
            };
            h_flex()
                .items_center()
                .child(div().flex_1().min_w_0().child(field))
                .child(
                    // 16 clear of the field: the glyph never reads as the
                    // end of its text.
                    h_flex()
                        .flex_none()
                        .ml(px(16.))
                        .gap(px(8.))
                        .items_center()
                        .when(main, |d| d.child(main_btn()))
                        .child(div().flex().flex_none().size(px(10.)).children(glyph_el())),
                )
                .children(halt_el.map(|h| h.ml(px(24.))))
                .child(send.ml(px(24.)))
        } else {
            let lead = match worker {
                None => glyph::lead(glyph_el(), m).into_any_element(),
                Some(_) => h_flex()
                    .flex_none()
                    .items_center()
                    .child(main_btn())
                    .child(mono("›", m.meta, p.dim_u).mx(px(8.)).flex_none())
                    .child(glyph::lead(glyph_el(), m))
                    .into_any_element(),
            };
            let name = worker.filter(|_| fit.name).map(|w| {
                h_flex()
                    .flex_none()
                    .gap(px(8.))
                    .child(mono(w.name.clone(), m.meta, p.ink).max_w(px(24. * cell)).overflow_hidden().text_ellipsis())
                    .child(mono(w.handle_id.clone(), m.meta, p.dim_u))
            });
            let word = word.filter(|_| fit.word).map(|w| {
                h_flex()
                    .flex_none()
                    .gap(px(8.))
                    .items_center()
                    .debug_selector(|| "foot-word".into())
                    .when(name.is_some(), |d| d.ml(px(12.)))
                    .child(display(w, m.meta, word_color))
                    .when_some(tally, |d, n| d.child(mono(count(n), m.meta, word_color)))
            });
            let meta = model.filter(|_| fit.model).map(|t| {
                h_flex()
                    .flex_shrink(1.)
                    .min_w_0()
                    .ml(px(16.))
                    .gap(px(8.))
                    .overflow_hidden()
                    .child(mono(t, m.meta, p.dim_u).min_w_0().overflow_hidden().text_ellipsis())
                    .when_some(effort.filter(|_| fit.effort), |d, e| {
                        d.child(mono("·", m.meta, p.dim_u).flex_none()).child(mono(e, m.meta, p.dim_u).flex_none())
                    })
            });
            // Cut short, its tooltip has it whole.
            let hint = hint.filter(|_| fit.hint).map(|(h, c)| {
                mono(h.clone(), m.meta, c)
                    .id("composer-note")
                    .debug_selector(|| "composer-note".into())
                    .ml(px(16.))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .tooltip(move |window, cx| crate::overlay::tip(h.clone(), window, cx))
            });
            let filler = hint.is_none().then(|| div().flex_1());
            v_flex().child(field).child(
                h_flex()
                    .mt(px(dims.row_gap.unwrap_or_default()))
                    .h(px(dims.button))
                    .items_center()
                    .whitespace_nowrap()
                    .child(lead)
                    .children(name)
                    .children(word)
                    .children(meta)
                    .children(hint)
                    .children(filler)
                    .children(halt_el.map(|h| h.ml(px(24.))))
                    .child(send.ml(px(24.))),
            )
        };
        column(
            lay,
            v_flex()
                // The frame sits inside the inset (design.md §1.1).
                .px(px(m.pad_x - 1.))
                .pt(px(dims.pad_top))
                .pb(px(dims.pad_bottom))
                .bg(p.user)
                .relative()
                .border_1()
                .border_color(p.edge)
                .child({
                    let at = self.composer_top.clone();
                    canvas(move |b, _, _| at.set(f32::from(b.origin.y)), |_, _, _, _| {}).absolute().inset_0()
                })
                // The user's bar over the left border.
                .child(
                    div().debug_selector(|| "composer-bar".into()).absolute().left(px(-1.)).top(px(-1.)).bottom(px(-1.)).w(px(2.)).bg(bar),
                )
                .children(top_edge.map(|e| div().absolute().left(px(-1.)).right(px(-1.)).top(px(-1.)).h(px(1.)).child(e)))
                // Handoff in: a 24 px light travels down it.
                .when(segment < 1., |d| {
                    d.child(
                        div().absolute().left(px(-1.)).top(px(-1.)).bottom(px(23.)).w(px(2.)).child(
                            div()
                                .debug_selector(|| "handoff-in".into())
                                .absolute()
                                .left_0()
                                .top(relative(segment))
                                .w(px(2.))
                                .h(px(24.))
                                .bg(segment_color),
                        ),
                    )
                })
                .text_px(m.body)
                .line_height(px(m.body_line))
                .text_color(p.ink)
                // The kit sizes its own text: the step is set on it.
                .child(inner),
        )
        .flex_none()
        .pt(px(dims.top))
        .pb(px(dims.bottom))
    }
}

/// The kit's horizontal padding of a medium input's text, drawn whatever
/// the input's appearance (read from the kit: it follows an upgrade).
pub(crate) fn kit_input_px() -> f32 {
    gpui_kit::component::Size::Medium.input_px().into()
}

/// A text button: tracked caps, `color`, `hover` on hover.
pub(crate) fn tbtn(id: &'static str, text: &'static str, color: Hsla, hover: Hsla) -> Stateful<Div> {
    display(text, fonts::size::D_M, color).id(id).cursor_pointer().hover(move |d| d.text_color(hover))
}

/// A filled button's fill: SEND and STEER's, or the warning's
/// (AUTHORIZE, SUBMIT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fill {
    Btn,
    Warn,
}

impl Fill {
    /// At rest and hovered (a step toward the ink).
    fn roles(self, p: &Palette) -> (Hsla, Hsla) {
        match self {
            Fill::Btn => (p.btn_fill, p.btn_hi),
            Fill::Warn => (p.warn_fill, p.warn_hi),
        }
    }

    /// Pressed (fluid.md §3.9): state, drawn in the frame the pointer goes
    /// down, at every motion level.
    fn press(self, p: &Palette) -> Hsla {
        match self {
            Fill::Btn => p.btn_press,
            Fill::Warn => p.warn_press,
        }
    }
}

/// A filled button (AUTHORIZE, SEND, design.md §8.12): its label is the
/// chrome colour on its fill (lifted until that reads at 4.5:1), the fill
/// a step up while hovered. One that cannot act has no fill: a 1 px
/// `edge` outline (inside its box: nothing moves) and a `dim` label.
pub(crate) fn fbtn(id: &'static str, text: &'static str, enabled: bool, fill: Fill, p: &Palette, m: &Metrics) -> Stateful<Div> {
    fbtn_key(id, text, None, enabled, fill, p, m)
}

/// [`fbtn`] with the key that presses it, after the label.
pub(crate) fn fbtn_key(
    id: &'static str,
    text: &'static str,
    key: Option<&'static str>,
    enabled: bool,
    fill: Fill,
    p: &Palette,
    m: &Metrics,
) -> Stateful<Div> {
    let ((bg, hi), down) = (fill.roles(p), fill.press(p));
    let fg = if enabled { p.chrome } else { p.dim };
    div()
        .id(id)
        .relative()
        .h(px(crate::fit::button_h(m)))
        .px(px(16.))
        .flex()
        .flex_none()
        .gap(px(10.))
        .items_center()
        .when(enabled, |d| d.bg(bg).cursor_pointer().hover(move |d| d.bg(hi)).active(move |d| d.bg(down)))
        .when(!enabled, |d| d.child(div().absolute().inset_0().border_1().border_color(p.edge)))
        .child(display(text, m.label, fg))
        .when_some(key, |d, k| d.child(mono(k, m.meta, fg)))
}

/// [`fbtn_key`], disabled, its fill charging: `charge` of 10 cells lit in
/// `fill`, left to right (motion.md §6.3). No charge: [`fbtn_key`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn fbtn_charge(
    id: &'static str,
    text: &'static str,
    key: Option<&'static str>,
    enabled: bool,
    charge: Option<u32>,
    fill: Fill,
    p: &Palette,
    m: &Metrics,
) -> Stateful<Div> {
    let Some(n) = charge.filter(|_| !enabled) else { return fbtn_key(id, text, key, enabled, fill, p, m) };
    // Drawn as a disabled button is (its `edge` outline, a `dim` label),
    // the charge a meter along its foot inside the outline: 10 cells of
    // the fill, 3 px, lit left to right. Under the label it would take the
    // disabled label's contrast (motion.md §10 amendment 2).
    let lit = fill.roles(p).0;
    let cells = h_flex()
        .absolute()
        .left(px(1.))
        .right(px(1.))
        .bottom(px(1.))
        .h(px(3.))
        .gap(px(1.))
        .children((0..10).map(|i| div().flex_1().h_full().when(i < n, |d| d.bg(lit))));
    div()
        .id(id)
        .relative()
        .h(px(crate::fit::button_h(m)))
        .px(px(16.))
        .flex()
        .flex_none()
        .gap(px(10.))
        .items_center()
        .child(div().absolute().inset_0().border_1().border_color(p.edge))
        .child(cells)
        .child(display(text, m.label, p.dim))
}

pub(crate) fn row_cache(item: &Item, ledger: &files::Ledger, subs: &crate::subagents::Subagents) -> RowCache {
    let mut r = RowCache {
        id: item.id,
        rev: item.rev,
        text: SharedString::default(),
        name: SharedString::default(),
        hue: turns::Hue::Gen,
        arg: SharedString::default(),
        result: SharedString::default(),
        result_more: 0,
        preview: None,
        delta: None,
        counts: turns::Counts::default(),
        card: turns::Card::default(),
        at: None,
        time: None,
        started: None,
        event: None,
    };
    r.retime(item);
    match item.kind {
        ItemKind::Tool => {
            let Some(t) = item.tool.as_ref() else { return r };
            r.name = t.name.clone().into();
            r.hue = turns::Hue::of(&t.name);
            if files::is_file_tool(&t.name) {
                r.preview = ledger.preview(&t.tool_id);
                r.delta = ledger.delta(&t.tool_id);
                r.counts = turns::Counts::of(r.delta.unwrap_or_default());
            }
            let input = input_of(&t.input);
            r.arg = match (&r.preview, subs.arg(&t.tool_id)) {
                (Some(pv), _) if !pv.title.is_empty() => pv.title.clone().into(),
                // A subagent's call: its label and its task's first line.
                (_, Some(arg)) => arg,
                _ => match command_of(input.as_ref()) {
                    Some(c) => one_line(&c, 200).into(),
                    None => one_line(&t.input, 200).into(),
                },
            };
            let lines: Vec<&str> = t.result.lines().collect();
            r.result_more = lines.len().saturating_sub(RESULT_LINES);
            r.result = lines.iter().take(RESULT_LINES).copied().collect::<Vec<_>>().join("\n").into();
            r.card = turns::Card::of(t, r.preview.as_deref(), &r.arg, input.as_ref());
        }
        ItemKind::Thinking => {
            r.text = item.text.clone().into();
            r.arg = one_line(&item.text, 200).into();
            r.card.verb = "think".into();
        }
        ItemKind::Event => {
            r.text = item.text.clone().into();
            r.event = item.event.as_deref().map(|e| Arc::new(crate::events::EventView::of(e)));
        }
        // A subagent's completion event: as the TUI says it.
        ItemKind::Notice => r.text = crate::subagents::completion_note(&item.text).unwrap_or_else(|| item.text.clone()).into(),
        _ => r.text = item.text.clone().into(),
    }
    r
}

/// `s` in at most `max` characters, cut from the left (`…` first) at a
/// `/` or `\\` if one is in reach: a path keeps its leaf (Windows' too).
pub(crate) fn left_cut(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max.max(2) {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - (max.max(2) - 1)).collect();
    let tail = match tail.find(['/', '\\']) {
        Some(at) if at > 0 && tail[at..].len() > 1 => tail[at..].to_string(),
        _ => tail,
    };
    format!("…{tail}")
}

pub(crate) fn one_line(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let more = s.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let mut out: String = line.chars().take(max).collect();
    if more || line.chars().count() > max {
        out.push_str(" …");
    }
    out
}

struct Inputs {
    composer: Entity<TextareaState>,
    find_input: Entity<InputState>,
    palette_input: Entity<InputState>,
    name_input: Entity<InputState>,
    hex_input: Entity<InputState>,
    pub(crate) secret_input: Entity<InputState>,
    settings_find: Entity<InputState>,
    settings_input: Entity<InputState>,
    settings_pane: Entity<crate::views::ViewPane>,
    usage_pane: Entity<crate::views::ViewPane>,
    focus: FocusHandle,
    allow_all_focus: FocusHandle,
    visible: bool,
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::mark("first_render");
        // Every frame drawn (`SYNAPS_DESKTOP_LOG=debug`): what an idle
        // window costs is countable.
        log::debug!(target: "synaps_desktop::frame", "frame");
        // The frame's clock and level: every animated element samples them.
        let low_power = self.fx.low_power(cx);
        self.fx.set_visible(self.visible);
        self.fx.begin_frame(crate::motion::Level::current(cx), low_power, self.fx.tick());
        self.fx.set_scale(window.scale_factor());
        self.fx.rescope(self.motion_session(), self.motion_column());
        // Another session, or a worker's view in or out: its dock and draft
        // are drawn at their heights (a switch replays nothing).
        let scope = self.motion_session() ^ self.motion_column().rotate_left(17);
        if std::mem::replace(&mut self.dock_scope, scope) != scope {
            self.dock_h.set(f32::NAN);
            self.composer_h.set(f32::NAN);
        }
        // A worker tab opened by the pointer: the tracer from it (§6.2),
        // from the source chip's bar (its left edge, at its centre) when the
        // click, or the menu picked from, was on a chip.
        if let Some(tab) = self.tracer_open.take()
            && self.fx.level() == crate::motion::Level::Full
            && !window.last_input_was_keyboard()
        {
            let mouse = window.mouse_position();
            let src = match self.menu_was.as_ref().map(|m| m.at) {
                Some(crate::menu::At::Point(pt)) => pt,
                _ => mouse,
            };
            let chip = self.chip_bounds.borrow().iter().find(|(_, b)| b.contains(&src)).map(|(_, b)| point(b.left(), b.center().y));
            self.tracer = Some(crate::motion::Tracer { tab, from: chip.unwrap_or(mouse), at: self.fx.now(), to: None });
        }
        self.chip_bounds.borrow_mut().clear();
        if self.menu.is_none()
            && self.smenu.is_none()
            && self.wmenu.is_none()
            && let Some(h) = self.menu_focus_back.take()
            && self.focus.is_focused(window)
        {
            window.focus(&h, cx);
        }
        if self.halt_armed.is_some() && !self.halt_here() {
            self.disarm_halt();
        }
        self.sync_composer(window, cx);
        self.refresh_palette(window, cx);
        if std::mem::take(&mut self.model.named.opened) {
            self.find_input.update(cx, |s, cx| s.set_value("", window, cx));
            self.find.clear();
        }
        // The card first: its row in the list is its state's.
        self.read_list_view();
        self.sync_card(window, cx);
        self.sync_list();
        self.read_card_on_screen();
        if self.stub_away.is_some() && (self.card_on_screen() || self.stub_away != self.model.card_key()) {
            self.stub_away = None;
        }
        self.card_keys.set(self.card_keys_live(window, cx));
        // Ctrl+J from a worker's view: the card once its row is built (that
        // card only: another, or none, drops it).
        if self.jump_card.is_some() && self.jump_card != self.model.card_key() {
            self.jump_card = None;
        }
        if self.jump_card.is_some_and(|k| self.current_card().is_some_and(|c| c.key == k))
            && self.model.watch.focus().is_none()
            && self.model.tabs.view().is_none()
            && self.card_ix().is_some()
        {
            self.jump_card = None;
            self.reveal_card(window, cx);
        }
        // Seen every frame while attached, shown or not: a card that comes
        // back (from a worker's view, Settings) is not new; another
        // session's, or one a reconnect brings again, is history (the
        // scope, [`AppView::motion_session`]). Keyed by the prompt's id, not
        // the attach: the same prompt is the same card.
        let prompt_key = self.model.transcript.pending_prompts().first().map_or(0, |pr| pr.id ^ (1 << 63));
        self.card_at = self
            .model
            .active
            .as_ref()
            .and_then(|_| self.fx.change(crate::motion::Tag::Card, 0, prompt_key, None))
            .filter(|_| prompt_key != 0)
            .map(|(at, _)| at);
        self.sync_tray(cx);
        self.rename_when_shown(window, cx);
        // A reorder waits while a rail row is held (motion.md §6.4): a row
        // never moves away from a click. Its release draws.
        self.refresh_rail();
        self.rail_motion();
        self.overlay_exits();
        if let Some(b) = self.bench.as_mut() {
            b.on_render(&self.list, window);
        }
        // Settings hidden: what it showed is forgotten, so a value changed
        // meanwhile (Ctrl+=, the top bar's THEME) is history when it opens
        // again, not a save to glow for.
        if self.model.tabs.view() != Some(crate::tabs::Kind::Settings) {
            self.settings.saved.get_mut().clear();
        }
        // What moves in Settings or Usage (a segment's bar, a chart's
        // reveal) draws their pane again.
        if self.fx.pane_wants()
            && let Some(k) = self.model.tabs.view()
        {
            let pane = self.pane(k).clone();
            cx.defer(move |cx| pane.update(cx, |_, cx| cx.notify()));
        }
        // The link (motion.md §5.1, §5.3; never offline): up, it scans;
        // dropped, it tears and the palette drains, held while down; back,
        // the palette restores. Settings and Usage draw with it.
        if !self.synthetic {
            let up = matches!(self.model.conn, ConnState::Connected(_));
            self.link.see(up, self.model.reload.then_some(self.model.reload_seq), &self.fx);
            // A reload past its grace is a loss: from here the rail and the
            // words say so (re-gate 3).
            if self.link.lost && self.model.reload {
                self.model.reload = false;
                self.model.rail_gen += 1;
            }
            if self.link.lost {
                self.fx.hold_still(self.link.lost_at());
            }
        }
        let drain = self.link.drain(&self.fx);
        if drain > 0. || self.link.lost {
            log::debug!(target: "synaps_desktop::link", "drain {drain:.3} lost {} pending {:?}", self.link.lost, self.fx.pending());
        }
        let scheme = cx.global::<theme::Scheme>();
        if scheme.drain.replace(drain) != drain
            && let Some(k) = self.model.tabs.view()
        {
            let pane = self.pane(k).clone();
            cx.defer(move |cx| pane.update(cx, |_, cx| cx.notify()));
        }
        let tear = self.link.tear(&self.fx);
        self.conn_dy = match tear {
            Some(0) => 3.,
            Some(_) => -2.,
            None => 0.,
        };
        // A theme change cross-fades over `slow` `inout` (motion.md §6.8),
        // from the app's frame; Settings and Usage draw with it. Sampled as
        // `Spatial` (a colour, but Reduced wants it at once: a level change
        // mid-fade ends it); its last frame syncs the kit and text views.
        if let Some((at, _)) = cx.global::<theme::Scheme>().fading() {
            let at = at.unwrap_or(self.fx.now());
            let t = self.fx.shot(Some(at), Duration::ZERO, crate::motion::SLOW, &crate::motion::ease::INOUT, crate::motion::Kind::Spatial);
            if cx.global::<theme::Scheme>().fade(at, t) {
                cx.defer(|cx| {
                    theme::settle_fade(cx);
                    cx.refresh_windows();
                });
            } else if t < 1. {
                cx.defer(theme::blend_text);
            }
            if let Some(k) = self.model.tabs.view() {
                let pane = self.pane(k).clone();
                cx.defer(move |cx| pane.update(cx, |_, cx| cx.notify()));
            }
        }
        let p = theme::palette(cx);
        let m = theme::metrics(cx);
        self.fx.set_ink(p.ink);
        self.body_line = m.body_line;
        self.scroll_frame(list_pad_b(&m));
        self.column_layer();
        // Settings or Usage came on screen (or one closed): after this
        // frame, never inside it, what it shows is asked for if stale.
        let view = self.model.tabs.view();
        let open_views = self.model.tabs.iter().filter(|t| t.kind != crate::tabs::Kind::Session).count();
        if (view, open_views) != (self.shown_view, self.shown_views) {
            (self.shown_view, self.shown_views) = (view, open_views);
            let me = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = me.update(cx, |v, cx| v.view_shown(cx));
            });
        }
        // What the view on screen reads of the rest of the app changed (the
        // daemon, the session, the panels, the theme): its cached pane draws
        // again, next frame.
        if let Some(k) = view {
            let key = self.view_inputs(k);
            let scheme = &cx.global::<theme::Scheme>().choice;
            if self.view_seen.as_ref().is_none_or(|(seen, c)| *seen != key || c != scheme) {
                let first = self.view_seen.is_none();
                self.view_seen = Some((key, scheme.clone()));
                if !first {
                    let pane = self.pane(k).clone();
                    cx.defer(move |cx| pane.update(cx, |_, cx| cx.notify()));
                }
            }
        }
        // A drag released outside the window has no drop: it ends here.
        if self.drag.is_some_and(|g| g.moved) && !cx.has_active_drag() {
            self.end_drag(cx);
        }
        if let Some((rail, w)) = self.bench.as_ref().and_then(BenchState::drag_width) {
            self.panels = dragged(self.width, self.panels, rail, w);
        }
        // Wide windows widen the side panels; narrow ones fold the open
        // numbers, then the open rail, to strips before the session gives.
        let width: f32 = window.viewport_size().width.into();
        let height: f32 = window.viewport_size().height.into();
        // L5: the card's read gate and AUTHORIZE are drawn from the last
        // frame's bounds: after a resize, one more frame draws them from
        // this one's.
        if height != self.height && self.card.is_some() {
            window.request_animation_frame();
        }
        // A window resize is the window manager's (fluid.md §3.8, `follow`):
        // the dock and the draft take the heights it gives them at once, as
        // after a switch.
        if (width, height) != (self.width, self.height) {
            self.dock_h.set(f32::NAN);
            self.composer_h.set(f32::NAN);
        }
        self.width = width;
        self.height = height;
        let (rail, right) = sides(self.width, self.panels, crate::chrome::MARK);
        if let Side::Open(w) = rail {
            self.rail_width = w;
        }
        // The centre pane's column. The list spans the pane (the wheel
        // scrolls it anywhere, the labels hang in its padding) with the
        // column as its item width: while the column is capped, a resize or
        // a panel drag changes only the padding, and the list keeps every
        // measured row (it re-measures when its item width changes).
        // The panels as drawn, on their springs (fluid.md §3.8); while one
        // moves, the column keeps the wrap of the flight's narrowest pane.
        let (rail_w, right_w) = self.panel_frame(rail, right);
        // In device pixels, one division, as the layout rounds the pane's
        // box (R2-M1): at 125/150/175 % three widths each divided apart
        // differ from it in the last bit, frame to frame, and the list
        // would forget its heights as at a fractional width.
        let s = window.scale_factor();
        let dp = |v: f32| (v * s).round();
        let pane_of = |less: &[f32]| ((dp(self.width) - less.iter().map(|&v| dp(v)).sum::<f32>()) / s).max(0.);
        let pane = pane_of(&[crate::chrome::MARK, rail_w, right_w]);
        let pane_to = pane_of(&[crate::chrome::MARK, rail.width(), right.width()]);
        self.panel_fx.freeze = self.panel_fx.moving().then(|| self.panel_fx.freeze.unwrap_or(pane).min(pane).min(pane_to));
        let wrap = self.panel_fx.freeze.unwrap_or(pane);
        // A view is not a session: no right panel (it describes one); the
        // pane takes its width. The session's column keeps its layout.
        let view_pane = pane_of(&[crate::chrome::MARK, rail_w]);
        // Its height under the full top bar, whatever bar is drawn.
        if self.top_words.is_none() {
            let words = crate::chrome::top_words(window);
            log::debug!(target: "synaps_desktop::words", "top bar {words:?}");
            self.top_words = Some(words);
            // The rail rows' state words (line 2's room for the rest).
            for st in crate::chrome::WORDED {
                if let Some(w) = st.word() {
                    self.display_w(w, fonts::size::D_S, window);
                }
            }
        }
        // The find field's placeholder whole where it fits, else its first
        // word: never cut mid-word.
        if let (Side::Open(w), Some(t)) = (rail, self.top_words.as_ref()) {
            let short = !crate::chrome::find_fits(w, t.cell);
            if short != self.find_short {
                self.find_short = short;
                let text = if short { crate::chrome::FIND.1 } else { crate::chrome::FIND.0 };
                self.find_input.update(cx, |s, cx| s.set_placeholder(text, window, cx));
            }
        }
        // The form from what decides it (not the stub or the tray).
        let ex = crate::fit::Extras { stub: false, tray: 0, ..self.extras() };
        let lay = Layout::of_in(pane, wrap, self.height - crate::chrome::TOP, &m, ex);
        if (lay.hang, lay.tight) != (self.layout.hang, self.layout.tight) {
            self.list.remeasure();
            self.metrics_gen += 1;
        }
        // A new column re-wraps the rows: this frame's scrollbar would read
        // the rows not yet measured at their estimates (it is held as it
        // was, `Bar::still`). One more frame draws it from their heights,
        // at every level (a fold at Off draws one frame, and its bar would
        // keep the estimate until the next change).
        let rewrap = lay.column != self.layout.column;
        if rewrap {
            window.request_animation_frame();
        }
        // Short or not: the list's top edge moves by what the bars above it
        // gain or lose, and the rows keep their place on screen; after a
        // remeasure too (a maximize crosses the hang width and the short
        // height at once: the two scrolls compose). The naming row counts
        // as each layout had it: it can open or close in the frame its room
        // flips the form. Under Settings or Usage too: the session's list
        // is back where it was when its tab is.
        if lay.form() != self.layout.form() {
            let shift = above_list(lay, ex.naming) - above_list(self.layout, self.layout_naming);
            self.keep_rows(shift, (lay.form() as u8) > (self.layout.form() as u8));
        }
        self.layout = lay;
        self.layout_naming = ex.naming;
        // The tray takes what is left over the card's least well and a
        // one-line composer; the composer grows only into room the card's
        // smallest form and the tray leave.
        // The card is in the log (polish.md §2.6): the dock holds none.
        let card_shown = false;
        let ex = crate::fit::Extras { stub: self.stub_shown(), ..ex };
        // The worker chips: the lines they need in the column, then how
        // many the room leaves. A chip fading out keeps its slot until its
        // fade is over.
        // The 3-line rule counts the chips fading out too: 5 workers going
        // to 4 keep their lines until the fifth's fade is over, so no
        // `+1 more` blinks in for it (Shady #16 r4).
        let (form, height, workers) = (lay.form(), self.height, self.tray.rows.len() + self.tray.ghosts.len());
        self.tray_rows = if view.is_none() {
            self.fit_chips(lay.column, &m, |needed| crate::fit::chip_lines(&m, form, height, card_shown, ex, workers, needed))
        } else {
            self.pack_chips(Vec::new(), lay.column, 0, &m);
            0
        };
        // A keyboard menu over a row no longer drawn closes; a workers menu
        // left empty leaves. Either way the next frame gives focus back.
        let menus = (self.menu.is_some(), self.wmenu.is_some());
        self.close_orphan_menu();
        if menus != (self.menu.is_some(), self.wmenu.is_some()) {
            window.request_animation_frame();
        }
        let ex = crate::fit::Extras { tray: self.tray_rows as u8, ..ex };
        let rows = crate::fit::composer_rows(&m, lay.form(), self.height, card_shown, ex);
        if rows != self.composer_rows {
            self.composer_rows = rows;
            self.composer.update(cx, |s, cx| s.set_auto_grow(1, rows, cx));
        }
        let tabs = self.render_tabs(if view.is_some() { view_pane } else { pane }, &p, cx);
        self.schedule_tabs_save(cx);
        self.schedule_favs_save(cx);
        // A worker's view on screen asks for its next page; under Settings
        // or Usage, nothing. Whether its list is scrolled back (its oldest
        // rows stay while they may be read).
        if self.model.watch.focus().is_some() {
            self.model.watch.scrolled_back = !self.list.is_following_tail();
        }
        self.arm_watch_poll(cx);
        if self.attach_wait.is_some() && self.attach_due.is_none() {
            self.arm_attach(cx);
        }
        let (pad_l, pad_r) = lay.padding();
        // Nothing over the list but the tab strip: the first row's own gap
        // (20·r) is the transcript's top edge (design.md §8.14).
        // No top padding: the sticky labels read rows' bounds, which leave
        // it out (`crate::sticky`).
        let (pad_t, pad_b) = (0., list_pad_b(&m));
        let list = list(
            self.list.clone(),
            cx.processor(|this, ix, _window, cx| {
                let row = this.render_disp(ix, cx);
                this.arrive(ix, row)
            }),
        )
        .size_full()
        .pl(px(pad_l))
        .pr(px(pad_r))
        .pt(px(pad_t))
        .pb(px(pad_b));
        self.texts.borrow_mut().frame(lay.column, &m);
        self.hint_rows(&m);
        self.hold_unfold();
        // After the list's layout: rows it has no height for get their hint
        // in the next frame's [`AppView::hint_rows`], so one is drawn (once:
        // hinted, every row has a height). Else, a load's last frame left the
        // thumb its unhinted length until something else drew.
        let hint_frame = self.list.is_scrolled_to_end().is_none().then(|| {
            let (l, n) = (self.list.clone(), self.disp.len());
            canvas(
                move |_, window, _| {
                    if wants_hint(&l, n) {
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0()
        });
        // After the list's layout: arrivals it laid out in its overdraw (off
        // screen, never painted) were drawn 0 px tall, and GPUI keeps that
        // height. They come back at their own, measured again next frame
        // (Shady M1): the scrollbar, the end and a wheel to it stay true.
        let unpainted = self.fx.fluid().then(|| {
            let (l, fx) = (self.list.clone(), self.fx.clone());
            canvas(
                move |_, window, _| {
                    let rows = fx.unpainted();
                    for ix in &rows {
                        l.remeasure_items(*ix..*ix + 1);
                    }
                    if !rows.is_empty() {
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0()
        });
        // Over the list's right padding, after its layout (and the hold-back's).
        let layer = self.layer_now();
        let glide = (self.fx.level() == crate::motion::Level::Full).then(|| (self.scroller.clone(), self.fx.clone()));
        // Held while an edge is dragged and as it springs to a detent after,
        // and in the frame a new column re-wraps the rows (on their
        // estimates): the next frame draws it from their heights (the
        // gate's S3 and S6, and the landing's frame).
        self.springback &= self.panel_fx.moving();
        let transcript_bar = crate::scrollbar::rows_gliding(&self.list, pad_t + pad_b, &self.bars.transcript, glide)
            .always()
            .still(self.drag.is_some() || self.springback || rewrap);
        // After the list's layout ([`AppView::keep_rows`]): a view grown to
        // the end of a scrolled-back list set it following; it stops 2 px
        // short of the end instead, still scrolled back. Built only on the
        // frames that need it; a layout with a reply's parse pending may be
        // provisional (GPUI re-arms following only on a real one), so the
        // hold lasts until GPUI re-arms or a layout has nothing pending
        // ([`held_back`]).
        let hold_back = self.hold_back.get().then(|| {
            let (held, hold, texts) = (self.list.clone(), self.hold_back.clone(), self.texts.clone());
            canvas(
                move |_, window, _| {
                    if held_back(&hold, texts.borrow().pending(), held.is_following_tail()) {
                        held.pause_following_tail();
                        held.scroll_by(px(-2.));
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0()
        });
        // A view draws in its own cached pane: this frame (a session
        // streaming behind it) reuses its last paint.
        let shown_view = view.filter(|k| *k != crate::tabs::Kind::Session).map(|k| {
            self.view_width = view_pane;
            AnyView::from(self.pane(k).clone()).cached(StyleRefinement::default().size_full())
        });
        let session = shown_view.is_none();
        // As elements now (they borrow nothing after this).
        let stub = session.then(|| self.render_stub(&p, &m, cx).map(IntoElement::into_any_element)).flatten();
        let lead = stub.is_some().then_some(p.warn_mark);
        let notice = session.then(|| self.render_notice(&p, &m).map(IntoElement::into_any_element)).flatten();
        let tray = session.then(|| self.render_tray(&p, &m, cx).map(IntoElement::into_any_element)).flatten();
        let dock = session.then(|| self.dock([stub, notice, tray].into_iter().flatten().collect(), lead)).flatten();
        let over = dock.is_some().then(|| self.over_composer.clone());
        if over.is_none() {
            self.over_composer.set(f32::INFINITY);
        }
        let main = if let Some(v) = shown_view {
            v_flex().flex_1().h_full().min_w_0().bg(p.canvas).child(tabs).child(div().flex_1().min_h_0().child(v))
        } else {
            v_flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .bg(p.canvas)
                .child(tabs)
                .when(self.model.named.naming && self.model.active.is_some(), |d| d.child(self.render_naming(&p, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .when_some(layer, |d, (x, y, k)| crate::motion::lift(d.left(px(x)).top(px(y)), k))
                        .child(list)
                        // Right after the list's layout, reading it as it
                        // paints: the top message's label pinned (P15).
                        .child(crate::sticky::layer(
                            self.fx.clone(),
                            cx.processor(|this, b: Bounds<Pixels>, _, cx| this.sticky_label(b, cx)),
                        ))
                        .children(crate::motion::probe(&self.fx, "sticky"))
                        .child(crate::scroll::owner(self.scroller.clone(), self.fx.clone()))
                        .children(unpainted)
                        .children(hint_frame)
                        .children(hold_back)
                        .child(transcript_bar),
                )
                // The top of what is drawn over the composer (the stub, a
                // notice, the chips), as laid out: the palette ends above it.
                .when_some(over, |d, at| d.child(canvas(move |b, _, _| at.set(f32::from(b.origin.y)), |_, _, _, _| {}).w_full().h(px(0.))))
                // The card off screen (scrolled away, or a worker's view):
                // NEEDS YOU, folded, over the chips.
                .children(dock)
                .children(crate::motion::probe(&self.fx, "composer"))
                .child(self.render_composer(
                    &p,
                    &m,
                    self.card_up() && self.composer.read(cx).focus_handle(cx).is_focused(window),
                    window,
                    cx,
                ))
        };
        let card_tab = self.card_keys.get() == Some(false) && self.allow_all_offered();
        h_flex()
            .id("root")
            .track_focus(&self.focus)
            .when(card_tab, |d| d.key_context(ALLOW_ALL_CTX))
            .on_action(cx.listener(|this, _: &CardTab, window, cx| this.card_tab(window, cx)))
            .when(self.menu.is_some() || self.smenu.is_some() || self.wmenu.is_some(), |d| d.key_context(MENU_CTX))
            .on_action(cx.listener(|this, _: &MenuTab, _, cx| this.menu_tab(false, cx)))
            .on_action(cx.listener(|this, _: &MenuTabBack, _, cx| this.menu_tab(true, cx)))
            .on_action(cx.listener(|this, _: &OpenGoTo, window, cx| this.open_palette(crate::palette::Mode::GoTo, window, cx)))
            .on_action(cx.listener(|this, _: &OpenCommands, window, cx| this.open_palette(crate::palette::Mode::Commands, window, cx)))
            .map(|d| crate::palette::capture_keys(d, cx))
            // A click opened a worker's tab: the rest of its double-click
            // is not the tab's (it would fold a row there, say).
            .capture_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, _, cx| match this.swallow_clicks {
                Some(until) if ev.click_count > 1 && Instant::now() < until => cx.stop_propagation(),
                Some(_) => this.swallow_clicks = None,
                None => {}
            }))
            // The kit's `Escape` (the key bindings run before any key
            // listener) ends an IME composition: whether there was one is
            // read here first, for HALT's Esc below.
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::Escape, window, cx| {
                this.esc_composing = this.composer.read(cx).focus_handle(cx).is_focused(window)
                    && this.composer.update(cx, |s, cx| gpui_kit::EntityInputHandler::marked_text_range(s, window, cx)).is_some();
            }))
            // Esc closes the picker, wherever the focus is; in find it clears
            // it and leaves it.
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                // Every Enter, Esc and Space, whoever takes it ([`CardChord`]).
                this.chord_press = this.chord.down(&ev.keystroke.key, Instant::now(), ev.is_held);
                // Ctrl+J: to the card, wherever the focus is (polish.md §2.6).
                let m = &ev.keystroke.modifiers;
                if ev.keystroke.key == "j"
                    && m.control
                    && !(m.alt || m.shift || m.platform || m.function)
                    && !this.model.transcript.pending_prompts().is_empty()
                {
                    cx.stop_propagation();
                    if this.palette.is_some() {
                        this.close_palette(window, cx);
                    }
                    this.jump_to_card(window, cx);
                    return;
                }
                // A menu item picked from the keyboard: that key's repeats
                // go nowhere (a held Enter on Steer… would send the draft).
                // A focused input's Enter binding has run already: its
                // `PressEnter` checks the flag ([`AppView::enter_is_the_picks`]).
                if std::mem::take(&mut this.menu_picked) && matches!(ev.keystroke.key.as_str(), "enter" | "space") && !this.chord_press {
                    this.menu_picked = true;
                    cx.stop_propagation();
                    return;
                }
                // The letter as the key or as what it types: a binding matches
                // either (a "Dvorak – QWERTY ⌘" layout on macOS).
                let k_is = |k: OnceKey| {
                    let ks = &ev.keystroke;
                    ks.key.eq_ignore_ascii_case(k.letter()) || ks.key_char.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(k.letter()))
                };
                if let Some(k) = this.once.take()
                    && k_is(k)
                {
                    cx.stop_propagation();
                    return this.once_key(k, ev.is_held, cx);
                }
                // A worker's context menu takes the keys while it is open.
                if this.menu.is_some() && this.menu_keydown(&ev.keystroke, this.chord_press, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if this.smenu.is_some() && this.session_menu_keydown(&ev.keystroke, this.chord_press, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if this.wmenu.is_some() && this.workers_menu_keydown(&ev.keystroke, this.chord_press, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                // The worker menu from the keyboard: from the session's
                // thread or a worker's (the composer, the view), never over
                // the accent picker or the find field.
                if crate::wtabs::is_menu_key(&ev.keystroke)
                    && this.model.tabs.view().is_none()
                    && !this.picker_open
                    && (this.focus.is_focused(window) || this.composer.read(cx).focus_handle(cx).is_focused(window))
                {
                    cx.stop_propagation();
                    this.menu_key(window, cx);
                    return;
                }
                if this.picker_open && ev.keystroke.key == "escape" {
                    this.close_picker(window, cx);
                    cx.stop_propagation();
                } else if ev.keystroke.key == "escape"
                    && (this.gear_menu || this.settings.editing.is_some() || this.settings.dropdown.is_some())
                {
                    // The gear's menu, a settings field (left as it was) or a dropdown.
                    this.gear_menu = false;
                    this.settings.dropdown = None;
                    if this.settings.editing.take().is_some() {
                        window.focus(&this.focus, cx);
                    }
                    cx.stop_propagation();
                    this.views_changed(cx);
                } else if ev.keystroke.key == "escape" && this.find_input.read(cx).focus_handle(cx).is_focused(window) {
                    this.find_input.update(cx, |s, cx| s.set_value("", window, cx));
                    this.find.clear();
                    window.focus(&this.focus, cx);
                    cx.stop_propagation();
                    cx.notify();
                } else if ev.keystroke.key == "escape"
                    && this.model.watch.focus().is_some()
                    && !this.model.watch.in_tab()
                    && this.model.tabs.view().is_none()
                {
                    // A worker's view: back to the session (a held Esc once).
                    cx.stop_propagation();
                    log::debug!(target: "synaps_desktop::watch", "esc (new press: {})", this.chord_press);
                    if this.chord_press {
                        this.leave_worker(true, cx);
                    }
                } else if ev.keystroke.key == "escape" && this.card_up() {
                    if this.card_keys_live(window, cx) == Some(true) {
                        // A secret prompt's field: CANCEL, once armed.
                        cx.stop_propagation();
                        if this.card_key_armed(cx) {
                            this.card_escape(window, cx);
                        }
                    } else if this.composer.read(cx).focus_handle(cx).is_focused(window) {
                        // Out of the composer: the card's keys act, after
                        // their wait (a second Esc right after is not a DENY).
                        cx.stop_propagation();
                        window.focus(&this.focus, cx);
                        this.restart_card_keys(cx);
                        cx.notify();
                    }
                } else if ev.keystroke.key == "escape"
                    && this.chord_press
                    && !{
                        let m = &ev.keystroke.modifiers;
                        m.control || m.alt || m.shift || m.platform
                    }
                    && this.model.watch.focus().is_none()
                    && this.model.tabs.view().is_none()
                    && this.composer.read(cx).focus_handle(cx).is_focused(window)
                    // An IME composition's Esc is the IME's (seen before
                    // the kit's `Escape` ended it).
                    && !std::mem::take(&mut this.esc_composing)
                    && this.halt_escape(Instant::now(), cx)
                {
                    // Esc, Esc in the composer (no card up, the session's
                    // thread, no IME composition): HALT.
                    cx.stop_propagation();
                }
            }))
            // `/` (nothing else focused) goes to find, but not under a
            // secret ([`AppView::text_keys_blocked`]). With a card's keys
            // live, Ctrl/Cmd+Enter answers yes and Esc no, once armed.
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let m = &ev.keystroke.modifiers;
                let blocked = this.focus.is_focused(window) && this.text_keys_blocked(Instant::now());
                let card = card_key(&ev.keystroke).filter(|_| this.card_keys_live(window, cx) == Some(false));
                let bare = !(m.control || m.alt || m.shift || m.platform || m.function);
                if ev.keystroke.key == "space"
                    && bare
                    && this.allow_all_focus.is_focused(window)
                    && this.card_keys_live(window, cx) == Some(false)
                {
                    // Space toggles Allow all only with its box focused: a new
                    // press, once the keys are armed (the toggle re-arms them).
                    // Too early or held, it waits again, like Ctrl+Enter and
                    // Esc ([`Arm`], Shady L3).
                    cx.stop_propagation();
                    if this.allow_all_offered() && this.card_key_armed(cx) {
                        this.toggle_allow_all(cx);
                    }
                } else if let Some(c) = card {
                    cx.stop_propagation();
                    match c {
                        CardKey::Scroll { down, page } => this.card_scroll(down, page, cx),
                        // Too early, or held: it waits again ([`Arm`]).
                        _ if !this.card_key_armed(cx) => {}
                        CardKey::Yes => this.card_enter(window, cx),
                        CardKey::No => this.card_escape(window, cx),
                    }
                } else if ev.keystroke.key == "/" && !(m.control || m.alt || m.platform) && this.focus.is_focused(window) && !blocked {
                    cx.stop_propagation();
                    let input = this.find_input.clone();
                    if !matches!(sides(this.width, this.panels, crate::chrome::MARK).0, Side::Open(_)) {
                        // Folded: the input is not drawn yet, and a focus on
                        // something not drawn is lost. Focus it once the
                        // frame that unfolds the rail has been drawn.
                        this.toggle_panel(true, cx);
                        window.on_next_frame(move |window, _| {
                            window.on_next_frame(move |window, cx| input.update(cx, |s, cx| s.focus(window, cx)));
                        });
                    } else {
                        input.update(cx, |s, cx| s.focus(window, cx));
                    }
                }
            }))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<EdgeDrag>, window, cx| {
                if ev.event.pressed_button == Some(MouseButton::Left) {
                    this.drag_edge(ev.event.position.x.into(), cx);
                } else {
                    // The release went elsewhere (a workspace switch, a broken
                    // grab, Alt+Tab): GPUI never ends the drag, so end it here.
                    cx.stop_active_drag(window);
                    this.end_drag(cx);
                }
            }))
            .on_drop(cx.listener(|this, _: &EdgeDrag, _, cx| this.end_drag(cx)))
            .on_action(cx.listener(|this, _: &ToggleRail, _, cx| this.fold_key(true, cx)))
            .on_action(cx.listener(|this, _: &ToggleRight, _, cx| this.fold_key(false, cx)))
            .on_action(cx.listener(|this, _: &TextBigger, _, cx| this.step_text(Some(1), cx)))
            .on_action(cx.listener(|this, _: &TextSmaller, _, cx| this.step_text(Some(-1), cx)))
            .on_action(cx.listener(|this, _: &TextReset, _, cx| this.step_text(None, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_view(crate::tabs::Kind::Settings, cx)))
            .on_action(cx.listener(|this, _: &OpenUsage, _, cx| this.open_view(crate::tabs::Kind::Usage, cx)))
            .on_action(cx.listener(|this, _: &WorkerUp, _, cx| this.worker_key(Step::Up, cx)))
            .on_action(cx.listener(|this, _: &WorkerDown, _, cx| this.worker_key(Step::Down, cx)))
            .on_action(cx.listener(|this, _: &WorkerTab, _, cx| this.worker_tab_key(cx)))
            .on_action(cx.listener(|this, _: &RenameSession, window, cx| this.rename_session(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleFavourite, window, cx| this.fav_key_pressed(window, cx)))
            .map(|d| tab_actions(d, cx))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
                let k = &ev.keystroke.key;
                this.chord.up(k, Instant::now());
                if k.eq_ignore_ascii_case("b") {
                    this.fold_keys.release(Instant::now());
                } else if k.eq_ignore_ascii_case("t") || k.eq_ignore_ascii_case("w") {
                    this.tab_keys.release(Instant::now());
                }
                if is_tab_key(k) {
                    this.tab_key_up(cx);
                }
            }))
            // Ctrl / Cmd let go ends a held tab key too (macOS drops the
            // key-up of a key let go while Cmd is down).
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _, cx| {
                if !(ev.modifiers.control || ev.modifiers.platform) {
                    this.tab_key_up(cx);
                }
            }))
            .relative()
            .size_full()
            .bg(p.chrome)
            .text_color(p.ink)
            .font_family(fonts::SANS)
            .text_px(13.)
            .child(
                v_flex().flex_1().h_full().min_w_0().child(self.render_top(&p, rail, cx)).child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_side(true, rail, &p, cx))
                        .child(main)
                        .when(view.is_none(), |d| d.child(self.render_side(false, right, &p, cx))),
                ),
            )
            .children(self.render_edge(true, rail, rail_w, &p, cx))
            .children(view.is_none().then(|| self.render_edge(false, right, right_w, &p, cx)).flatten())
            .when(self.picker_open, |d| {
                d.child(deferred(self.render_picker(&p, if view.is_some() { Side::Open(0.) } else { right }, cx)).with_priority(1))
                    .child(deferred(self.bars.picker.bar(crate::scrollbar::Ground::Chrome)).with_priority(1))
            })
            .when(self.gear_menu || self.gear_leaving.is_some(), |d| {
                d.child(deferred(self.render_gear_menu(rail == Side::Strip, &p, cx)).with_priority(2))
            })
            .children(self.render_menu_at_point(&p, cx))
            .children(self.render_session_menu(&p, cx))
            .children(self.render_workers_menu(&p, cx))
            .children(self.render_palette(
                crate::chrome::MARK + rail_w,
                if view.is_some() { view_pane } else { pane },
                // Over a view there is no composer: the window's end.
                if view.is_some() { self.height } else { self.composer_top.get().min(self.over_composer.get()) },
                &p,
                cx,
            ))
            .children(self.render_link(tear, &p, &theme::live_palette(cx)))
            .children(self.render_tracer(&p))
            // Painted last, after every row was rendered: what the frame's
            // motion asked for (a frame, the one timer, or nothing; with a
            // beat layer, it arms the timer after painting the beat).
            .child({
                let (fx, me) = (self.fx.clone(), cx.entity_id());
                canvas(
                    |_, _, _| {},
                    move |_, _, window, cx| {
                        #[cfg(test)]
                        fx.painted("end-frame");
                        fx.end_frame(window, cx, me)
                    },
                )
                .absolute()
                .size_0()
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use std::time::{Duration, Instant};

    use super::{
        ARM, Arm, CENTRE_MIN, CardChord, CardKey, FavsJob, FoldKeys, Foot, FootIn, RENAME_KEY, SessionMeta, Side, TabKey, YES_KEY,
        above_list, authorize_ready, card_key, card_key_on, command_of, dragged, fav_key, fit_foot, held_back, hidden, input_of, left_cut,
        lines_below, one_line, palette_keys, panel_keys, secret_answer, side_widths, sides, still_typing, tab_keys, text_keys, toggled,
        well_body, worker_keys,
    };
    use crate::theme::{Metrics, Panels};
    use crate::turns::Layout;
    use crate::watch::Step;

    /// R5: a favourites write leaves what it wrote for the window to take,
    /// whatever became of the task that started it; once the window has
    /// closed (its flush wrote, holding the lock), a job not begun writes
    /// nothing over it.
    #[test]
    fn a_favourites_write_is_taken_or_never_begun() {
        use crate::favourites::{Favourite, Source};
        let dir = std::env::temp_dir().join(format!("synaps-desktop-favsjob-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let v: synaps_client::SavedSession = serde_json::from_value(serde_json::json!({"id": "x", "title": "X"})).unwrap();
        let sent = vec![Favourite::of(Source::Saved(&v), 1)];
        let slot = std::sync::Mutex::new(FavsJob::default());
        FavsJob::write(&slot, &path, "k", &[], sent.clone(), 7).unwrap();
        let (generation, s, file) = slot.lock().unwrap().done.take().expect("left to take");
        assert_eq!((generation, &s, &file), (7, &sent, &sent));
        std::fs::write(&path, "{}").unwrap();
        slot.lock().unwrap().closed = true;
        FavsJob::write(&slot, &path, "k", &[], sent, 8).unwrap();
        assert!(slot.lock().unwrap().done.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}", "closed: not written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Short is the pane's height alone, under the full top bar: 648 px,
    /// a 700-px window (1280×720 is full). It changes nothing the list's
    /// rows are laid out by (the column, the hanging labels, tight), so the
    /// list never remeasures for it. Nothing sits between the tab strip and
    /// the list (design.md §8.14): the list's top edge moves only with the
    /// top bar and the naming row.
    #[test]
    fn a_short_pane_changes_nothing_in_the_list() {
        let m = Metrics::at(16);
        let pane_h = |window: f32| window - crate::chrome::TOP;
        for (window_h, short) in [(1080., false), (864., false), (720., false), (700., false), (699., true), (600., true)] {
            assert_eq!(Layout::of(1000., pane_h(window_h), &m, Default::default()).short, short, "{window_h}");
        }
        assert_eq!(crate::fit::short_below(&m), 648.);
        for w in [500., 900., 1600.] {
            let (tall, short) = (Layout::of(w, 1000., &m, Default::default()), Layout::of(w, 500., &m, Default::default()));
            assert_eq!(
                (tall.column, tall.hang, tall.tight, tall.left, tall.right),
                (short.column, short.hang, short.tight, short.left, short.right)
            );
            assert!(!tall.short && short.short);
        }
        // A panel dragged across 560 moves nothing up or down.
        let (tight, wide) = (Layout::of(532., 1000., &m, Default::default()), Layout::of(600., 1000., &m, Default::default()));
        assert!(tight.tight && !tight.short);
        assert_eq!(above_list(tight, false), above_list(wide, false));
        // Full to short: the list's top edge rises by the top bar's 12 px
        // (what a scrolled-back list scrolls by); the naming row adds its own.
        let (full, short) = (Layout::of(1000., 1000., &m, Default::default()), Layout::of(1000., 600., &m, Default::default()));
        assert_eq!(above_list(short, false) - above_list(full, false), -12.);
        assert_eq!(above_list(full, true) - above_list(full, false), crate::fit::naming_h(full.form()));
        let cramped = Layout::of(1000., 340., &m, Default::default());
        assert!(cramped.cramped && cramped.short);
        assert_eq!(above_list(cramped, false), above_list(short, false));
    }

    /// The tab's tooltip is the line the session header had: title, name,
    /// model, effort, folder and `detached`, each if known.
    #[test]
    fn the_tab_tooltip_says_the_session_line() {
        let meta = SessionMeta {
            title: Some("Refresh race".into()),
            name: Some("relay-refresh".into()),
            model: Some("opus-4-1".into()),
            effort: Some("high".into()),
            cwd: Some("~/work/relay".into()),
            detached: false,
        };
        assert_eq!(meta.tooltip("relay-refresh"), "Refresh race · relay-refresh · opus-4-1 · high · ~/work/relay");
        let bare = SessionMeta { detached: true, ..SessionMeta::default() };
        assert_eq!(bare.tooltip("t"), "t · detached");
        // A name the tab already shows as its title is not said twice.
        let named = SessionMeta { name: Some("t".into()), ..SessionMeta::default() };
        assert_eq!(named.tooltip("t"), "t");
    }

    /// The folder keeps its leaf: cut from the left at a `/`.
    #[test]
    fn the_folder_is_cut_from_the_left() {
        assert_eq!(left_cut("~/work/relay-gateway", 40), "~/work/relay-gateway");
        assert_eq!(left_cut("/home/you/work/relay-gateway", 22), "…/work/relay-gateway");
        assert_eq!(left_cut("/home/you/work/relay-gateway", 14), "…relay-gateway");
        assert!(left_cut("/a/very/long/path/to/somewhere", 10).chars().count() <= 10);
        // Windows' separator (`home_short` gives `~\\work\\…` there).
        assert_eq!(left_cut("C:\\Users\\you\\work\\relay-gateway", 22), "…\\work\\relay-gateway");
        assert_eq!(left_cut("~\\work\\relay-gateway", 15), "…\\relay-gateway");
    }

    /// The foot's parts at full width: every part that has something to
    /// say; then, as the row narrows, they drop in order (hint, effort,
    /// model, word, name), never the lead, HALT or the primary; a tight
    /// pane has no word.
    #[test]
    fn the_foot_drops_its_parts_in_order() {
        let all = FootIn {
            room: 2000.,
            lead: 20.,
            word: 60.,
            name: 80.,
            model: 72.,
            effort: 50.,
            hint: 200.,
            halt: 40.,
            primary: 80.,
            tight: false,
        };
        let every = Foot { hint: true, effort: true, model: true, word: true, name: true };
        assert_eq!(fit_foot(&all), every);
        // Every foot a narrowing row shows, in order: exactly one part
        // fewer each time, in the drop order.
        let mut seen = vec![every];
        for room in (100..2000).rev() {
            let f = fit_foot(&FootIn { room: room as f32, ..all });
            if seen.last() != Some(&f) {
                seen.push(f);
            }
        }
        let no_hint = Foot { hint: false, ..every };
        let no_effort = Foot { effort: false, ..no_hint };
        let no_model = Foot { model: false, ..no_effort };
        let no_word = Foot { word: false, ..no_model };
        assert_eq!(seen, [every, no_hint, no_effort, no_model, no_word, Foot::default()]);
        assert_eq!(fit_foot(&FootIn { room: 100., ..all }), Foot::default(), "the lead, HALT and the primary stay");
        assert!(!fit_foot(&FootIn { tight: true, ..all }).word, "a tight pane drops the word");
        // Nothing to say: nothing shown.
        let quiet = FootIn { hint: 0., effort: 0., name: 0., ..all };
        assert_eq!(fit_foot(&quiet), Foot { model: true, word: true, ..Foot::default() });
    }

    /// N-L1: `submit()` and `sync_composer()` guard with `composer_owned()`
    /// (tested against views on screen in `tabs.rs`), first thing, and no
    /// other composer check is left in this file. It reads this file: a
    /// call site that drifts back to the visible tab fails here without a
    /// GPUI window.
    #[test]
    fn composer_guard_is_the_one_helper() {
        // A Windows checkout's CRLF read as the LF it is in the repository.
        let src = include_str!("app.rs").replace("\r\n", "\n");
        let body = |name: &str| {
            let at = src.find(&format!("\n    fn {name}(")).unwrap_or_else(|| panic!("fn {name}"));
            let rest = &src[at + 1..];
            &rest[..rest.find("\n    }\n").unwrap_or_else(|| panic!("end of fn {name}"))]
        };
        let submit = body("submit");
        let guard = submit.find("if !self.composer_owned() {").expect("submit() guards with composer_owned()");
        let send = submit.find(".submit(text").expect("submit() sends");
        assert!(guard < send, "the guard comes before the send");
        assert!(body("sync_composer").contains("&& self.composer_owned() {"), "the draft swap uses the same guard");
        assert_eq!(body("composer_owned").matches("Drafts::composer_owned(").count(), 1);
        // Nothing else asks for the composer's tab on its own.
        assert_eq!(src.matches(["Drafts", "::owns("].concat().as_str()).count(), 0);
    }

    /// The fold keys parse on every OS (a bad one panics at start-up).
    #[test]
    fn panel_keys_parse() {
        for mac in [false, true] {
            let (a, b) = panel_keys(mac);
            for k in [a, b].into_iter().chain(text_keys(mac)) {
                assert!(gpui_kit::Keystroke::parse(k).is_ok(), "{k}");
            }
        }
    }

    /// A held tab key ends with its own key-up (or Ctrl / Cmd let go),
    /// not another key's.
    #[test]
    fn tab_key_ups_are_known() {
        for k in ["tab", "pagedown", "pageup", "1", "9", "t", "w"] {
            assert!(super::is_tab_key(k), "{k}");
        }
        for k in ["b", "a", "enter", "escape", "shift", "control"] {
            assert!(!super::is_tab_key(k), "{k}");
        }
    }

    /// The tab keys parse on every OS, each keystroke means one thing, and
    /// none is a key the inputs (composer, find, names) or an IME use: the
    /// list is gpui-base 0.9.1's input bindings (`input/base/state.rs`), a
    /// hand copy: refresh it when the kit's pin moves.
    /// The session menu: Rename… (a held session's), Favourite or
    /// Unfavourite as it is (off while the daemon has not listed it), Close
    /// tab (a tab's).
    #[test]
    fn the_session_menu_favourites() {
        use super::{MenuFor, SessionAct, session_items};
        let labels = |f: MenuFor| -> Vec<(String, bool)> {
            session_items(f).into_iter().map(|i| (i.label.to_string(), i.why_not.is_none())).collect()
        };
        let held = labels(MenuFor { tab: true, live: true, fav: Some(false) });
        assert_eq!(held, [("Rename…".into(), true), ("Favourite".into(), true), ("Close tab".into(), true)]);
        let saved = labels(MenuFor { tab: false, live: false, fav: Some(true) });
        assert_eq!(saved, [("Rename…".into(), false), ("Unfavourite".into(), true), ("Close tab".into(), false)]);
        assert!(!labels(MenuFor { tab: true, live: true, fav: None })[1].1);
        assert_eq!(session_items(MenuFor { tab: true, live: true, fav: Some(true) })[1].act, SessionAct::Favourite);
    }

    #[test]
    fn tab_keys_parse_and_clash_with_nothing() {
        const INPUT: &[&str] = &[
            "alt-backspace",
            "alt-delete",
            "alt-left",
            "alt-right",
            "alt-shift-left",
            "alt-shift-right",
            "backspace",
            "cmd-.",
            "cmd-[",
            "cmd-]",
            "cmd-a",
            "cmd-alt-down",
            "cmd-alt-up",
            "cmd-backspace",
            "cmd-c",
            "cmd-delete",
            "cmd-down",
            "cmd-f",
            "cmd-left",
            "cmd-right",
            "cmd-shift-down",
            "cmd-shift-f",
            "cmd-shift-up",
            "cmd-shift-z",
            "cmd-up",
            "cmd-v",
            "cmd-x",
            "cmd-z",
            "ctrl-.",
            "ctrl-[",
            "ctrl-]",
            "ctrl-a",
            "ctrl-alt-down",
            "ctrl-alt-up",
            "ctrl-backspace",
            "ctrl-c",
            "ctrl-cmd-space",
            "ctrl-delete",
            "ctrl-e",
            "ctrl-end",
            "ctrl-f",
            "ctrl-h",
            "ctrl-home",
            "ctrl-left",
            "ctrl-right",
            "ctrl-shift-a",
            "ctrl-shift-e",
            "ctrl-shift-end",
            "ctrl-shift-home",
            "ctrl-shift-left",
            "ctrl-shift-right",
            "ctrl-v",
            "ctrl-x",
            "ctrl-y",
            "ctrl-z",
            "delete",
            "down",
            "end",
            "escape",
            "home",
            "left",
            "pagedown",
            "pageup",
            "right",
            "shift-alt-down",
            "shift-alt-left",
            "shift-alt-right",
            "shift-alt-up",
            "shift-backspace",
            "shift-cmd-left",
            "shift-cmd-right",
            "shift-delete",
            "shift-down",
            "shift-end",
            "shift-home",
            "shift-left",
            "shift-right",
            "shift-tab",
            "shift-up",
            "tab",
            "up",
            // The find box and composer: Enter opens / sends, Ctrl+Enter a new tab.
            "enter",
            "ctrl-enter",
            "cmd-enter",
            "shift-enter",
        ];
        let parse = |k: &'static str| gpui_kit::Keystroke::parse(k).unwrap_or_else(|e| panic!("{k}: {e:?}"));
        let input: Vec<_> = INPUT.iter().map(|k| parse(k)).collect();
        // The worker keys: Ctrl/Alt+↑/↓ (Option first on macOS), none an
        // input's (the composer keeps ↑/↓, Shift+↑/↓, Ctrl+Alt+↑/↓ and
        // Cmd+↑/↓), an app key, a view key or a card key.
        for mac in [false, true] {
            let (a, b) = panel_keys(mac);
            let (s, u, alt) = crate::views::view_keys(mac, !mac);
            let mut taken: Vec<_> = [a, b, RENAME_KEY, fav_key(mac)]
                .into_iter()
                .chain(text_keys(mac))
                .chain(tab_keys(mac).iter().map(|(k, _)| *k))
                .map(parse)
                .collect();
            taken.extend([Some(s), Some(u), alt].into_iter().flatten().map(parse));
            let keys = worker_keys(mac);
            assert_eq!(keys[0], (if mac { "alt-up" } else { "ctrl-up" }, Step::Up));
            for (i, (k, _)) in keys.iter().enumerate() {
                let k = parse(k);
                assert!(!input.contains(&k) && !taken.contains(&k), "{k:?}");
                assert!(!keys[..i].iter().any(|(o, _)| parse(o) == k), "{k:?} bound twice");
                assert_eq!(card_key_on(&k, mac), None, "{k:?}");
                assert!(k.modifiers.control != k.modifiers.alt && !k.modifiers.shift && !k.modifiers.platform, "{k:?}");
                assert!(k.key == "up" || k.key == "down");
            }
            assert_eq!(keys.iter().filter(|(_, s)| *s == Step::Up).count(), 2);
        }
        for mac in [false, true] {
            let (a, b) = panel_keys(mac);
            // Ctrl+D / Cmd+D, the favourite key, and the palette's
            // Ctrl+P / Ctrl+K (Cmd on macOS) with them; Ctrl+J (the card) is
            // read off the key itself, so no binding is it either.
            let (goto, commands) = palette_keys(mac);
            let mut app: Vec<_> =
                [a, b, fav_key(mac), goto, commands].into_iter().chain(text_keys(mac)).map(|k| (parse(k), None)).collect();
            assert!(!app.iter().any(|(k, _)| k.key == "j" && k.modifiers.control), "Ctrl+J is the card's");
            app.extend(tab_keys(mac).iter().map(|(k, what)| (parse(k), Some(*what))));
            for (i, (k, what)) in app.iter().enumerate() {
                assert!(!input.contains(k), "{k:?} is an input key");
                assert!(!app[..i].iter().any(|(o, _)| o == k), "{k:?} bound twice");
                // Never a bare key or Shift+key: those are typing.
                assert!(k.modifiers.control || k.modifiers.platform, "{k:?} has no Ctrl/Cmd");
                // Cmd+Tab is macOS's app switcher.
                assert!(!(mac && k.modifiers.platform && k.key == "tab"), "{k:?}");
                if what.is_some() && mac {
                    assert!(k.key == "tab" || k.modifiers.platform, "{k:?}: Cmd on macOS");
                }
            }
            // F2 names the session (design.md §8.14): a function key, so
            // not typing; no input, app, view or card key is it.
            let f2 = parse(RENAME_KEY);
            assert_eq!((f2.key.as_str(), f2.modifiers), ("f2", Default::default()));
            assert!(!input.contains(&f2) && !app.iter().any(|(k, _)| *k == f2), "F2 is taken");
            assert_eq!(card_key_on(&f2, mac), None);
            let nth: Vec<usize> = tab_keys(mac).iter().filter_map(|(_, w)| if let TabKey::Nth(n) = w { Some(*n) } else { None }).collect();
            assert_eq!(nth, (0..8).collect::<Vec<_>>());
            for want in [TabKey::New, TabKey::Close, TabKey::Reopen, TabKey::Next, TabKey::Prev, TabKey::Last] {
                assert!(tab_keys(mac).iter().any(|(_, w)| *w == want), "{want:?}");
            }
            // The card's keys: no app binding is one of them (they act only
            // with the window's own focus, so the inputs keep theirs).
            let (s, u, alt) = crate::views::view_keys(mac, !mac);
            let views: Vec<_> = [Some(s), Some(u), alt].into_iter().flatten().map(parse).collect();
            for (k, _) in &app {
                assert_eq!(card_key_on(k, mac), None, "{k:?}");
            }
            for k in &views {
                assert_eq!(card_key_on(k, mac), None, "{k:?}");
            }
        }
        for mac in [false, true] {
            let card = |k: &'static str| card_key_on(&parse_any(k), mac);
            // H3: a bare Enter never answers (the window holds the focus
            // after a launch, a click, Settings); Ctrl+Enter does, Cmd+Enter
            // on macOS. L4: Super+Enter is the desktop's on Linux.
            let (yes, other) = if mac { ("cmd-enter", "ctrl-enter") } else { ("ctrl-enter", "cmd-enter") };
            assert_eq!(card(yes), Some(CardKey::Yes), "{mac}");
            assert_eq!((card("enter"), card(other), card("super-enter")), (None, None, if mac { Some(CardKey::Yes) } else { None }));
            assert_eq!(card("escape"), Some(CardKey::No));
            assert_eq!(card("pagedown"), Some(CardKey::Scroll { down: true, page: true }));
            assert_eq!(card("up"), Some(CardKey::Scroll { down: false, page: false }));
            // Shift+Enter is a new line elsewhere; Shift+Esc, Ctrl+Esc and
            // Shift+Down are not the card's; no letter is (no y / n).
            for k in [
                "shift-enter",
                "alt-enter",
                "ctrl-cmd-enter",
                "shift-ctrl-enter",
                "shift-cmd-enter",
                "alt-ctrl-enter",
                "shift-escape",
                "ctrl-escape",
                "shift-down",
                "ctrl-pagedown",
                "y",
                "n",
                "a",
                "space",
            ] {
                assert_eq!(card(k), None, "{k} {mac}");
            }
        }
        assert_eq!(card_key(&parse_any(if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" })), Some(CardKey::Yes));
        assert_eq!(YES_KEY, if cfg!(target_os = "macos") { "⌘ENTER" } else { "CTRL+ENTER" });
    }

    fn parse_any(k: &'static str) -> gpui_kit::Keystroke {
        gpui_kit::Keystroke::parse(k).unwrap_or_else(|e| panic!("{k}: {e:?}"))
    }

    /// Wide windows widen the panels; folded ones are strips at any width;
    /// an open panel the window cannot fit is a strip too, never hidden.
    #[test]
    fn side_panels_follow_the_window() {
        let open = Panels::default();
        assert_eq!(sides(1280., open, 30.), (Side::Open(312.), Side::Open(296.)));
        assert_eq!(sides(1920., open, 30.), (Side::Open(312.), Side::Open(296.)));
        assert_eq!(sides(2560., open, 30.), (Side::Open(340.), Side::Open(358.)));
        assert_eq!(sides(1000., open, 30.), (Side::Open(312.), Side::Strip));
        assert_eq!(sides(700., open, 30.), (Side::Strip, Side::Strip));
        let folded = Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() };
        assert_eq!(sides(700., folded, 30.), (Side::Strip, Side::Strip));
        // A folded rail leaves room for the open numbers.
        assert_eq!(
            sides(1000., Panels { rail_collapsed: true, right_collapsed: false, ..Panels::default() }, 30.),
            (Side::Strip, Side::Open(296.))
        );
    }

    /// L1: a click on a strip opens its panel, folding the other one if
    /// only that makes room; a window too narrow for it changes nothing.
    /// What is saved is what is shown.
    #[test]
    fn a_strip_opens_its_panel_or_says_it_cannot() {
        let open = Panels::default();
        let rail_only = Panels { rail_collapsed: false, right_collapsed: true, ..Panels::default() };
        let right_only = Panels { rail_collapsed: true, right_collapsed: false, ..Panels::default() };
        let both = Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() };
        // Wide: each folds and opens on its own.
        assert_eq!(toggled(1920., open, true), Some(right_only));
        assert_eq!(toggled(1920., right_only, true), Some(open));
        assert_eq!(toggled(1920., open, false), Some(rail_only));
        // 1000 px, both "open": the right one shows as a strip. Its click
        // opens it and folds the rail (was: the panel vanished).
        assert_eq!(toggled(1000., open, false), Some(right_only));
        assert_eq!(sides(1000., right_only, 30.), (Side::Strip, Side::Open(296.)));
        // Opening the rail again folds the right panel that no longer fits.
        assert_eq!(toggled(1000., right_only, true), Some(rail_only));
        // 700 px: neither fits; a click on either strip changes nothing.
        assert_eq!(toggled(700., both, true), None);
        assert_eq!(toggled(700., both, false), None);
        assert_eq!(toggled(700., open, true), None);
        // Shown as strips anyway, so folding one is a no-op that saves it folded.
        assert_eq!(sides(700., open, 30.), sides(700., both, 30.));
    }

    /// The PR #7 rule, before panels could be dragged, with the rail's
    /// 312 floor: beside a strip it gives way to the centre's minimum, down
    /// to PR #7's 276.
    fn pr7_sides(window: f32, panels: Panels) -> (Side, Side) {
        let (rail_w, right_w) = side_widths(window);
        let rail = if !panels.rail_collapsed && window >= 760. { Side::Open(rail_w) } else { Side::Strip };
        let right =
            if !panels.right_collapsed && window - 30. - rail.width() - right_w >= CENTRE_MIN { Side::Open(right_w) } else { Side::Strip };
        let rail = match (rail, right) {
            (Side::Open(w), Side::Strip) => Side::Open(w.min((window - 30. - CENTRE_MIN - super::STRIP).max(276.)).floor()),
            (r, _) => r,
        };
        (rail, right)
    }

    /// The auto rail on a narrow window: it gives way so the centre keeps
    /// [`CENTRE_MIN`], down to 276 (then the centre gives); one the user
    /// dragged keeps its floor.
    #[test]
    fn the_auto_rail_gives_way_to_the_centre() {
        let m = crate::chrome::MARK;
        let centre = |window: f32, (l, r): (Side, Side)| window - m - l.width() - r.width();
        let open = Panels::default();
        assert_eq!(sides(1280., open, m), (Side::Open(312.), Side::Open(296.)));
        assert_eq!(centre(1280., sides(1280., open, m)), 672.);
        assert_eq!(sides(1000., open, m), (Side::Open(312.), Side::Strip));
        assert_eq!(centre(1000., sides(1000., open, m)), 656.);
        assert_eq!(sides(800., open, m), (Side::Open(288.), Side::Strip));
        assert_eq!(centre(800., sides(800., open, m)), CENTRE_MIN);
        assert_eq!(sides(770., open, m), (Side::Open(276.), Side::Strip), "no narrower than 276");
        // A width the user saved: its floor, as before.
        assert_eq!(sides(800., hand(Some(312), None), m), (Side::Open(312.), Side::Strip));
        assert_eq!(sides(800., hand(Some(400), None), m), (Side::Open(312.), Side::Strip));
    }

    fn centre(window: f32, (l, r): (Side, Side)) -> f32 {
        window - 30. - l.width() - r.width()
    }

    fn hand(rail: Option<u16>, right: Option<u16>) -> Panels {
        Panels { rail_width: rail, right_width: right, ..Panels::default() }
    }

    /// Auto widths lay out as before at every width; dragged widths stay in
    /// their limits, and never squeeze the centre under its minimum except
    /// where the auto rail already does (a narrow window, the rail at its
    /// floor).
    #[test]
    fn the_fit_rules_hold_at_every_width() {
        let folds = [(false, false), (true, false), (false, true), (true, true)];
        // Every 7 px, the breakpoints, and fractional widths (125 %/150 %
        // scaling at a tiled size): panels stay on whole pixels (L4).
        let edges = [760., 838., 1082., 2044., 1092.8, 1366.4, 1707.2, 2044.5];
        for window in (640..3400).step_by(7).map(|w| w as f32).chain(edges) {
            for (rc, gc) in folds {
                let auto = Panels { rail_collapsed: rc, right_collapsed: gc, ..Panels::default() };
                assert_eq!(sides(window, auto, 30.), pr7_sides(window, auto), "{window} {auto:?}");
                for (l, r) in [(Some(220), Some(240)), (Some(480), Some(520)), (Some(400), None), (None, Some(500)), (Some(300), Some(260))]
                {
                    let p = Panels { rail_collapsed: rc, right_collapsed: gc, ..hand(l, r) };
                    let shown = sides(window, p, 30.);
                    let c = centre(window, shown);
                    if let Side::Open(w) = shown.0 {
                        assert!((220. ..=480.).contains(&w), "{window} {p:?} {shown:?}");
                        assert!(c >= CENTRE_MIN || w <= super::FLOORS.0, "{window} {p:?} {shown:?}");
                        assert_eq!(w, w.round());
                    }
                    if let Side::Open(w) = shown.1 {
                        assert!((240. ..=520.).contains(&w), "{window} {p:?} {shown:?}");
                        assert!(c >= CENTRE_MIN, "{window} {p:?} {shown:?}");
                        assert_eq!(w, w.round());
                    }
                    assert!(!rc || shown.0 == Side::Strip, "folded stays folded");
                    assert!(!gc || shown.1 == Side::Strip, "folded stays folded");
                }
            }
        }
        // A width set by hand beats the wide window's auto-widening.
        assert_eq!(sides(2560., hand(Some(250), Some(260)), 30.), (Side::Open(250.), Side::Open(260.)));
        // Too narrow for both as dragged: they give way in proportion...
        assert_eq!(sides(1130., hand(Some(480), Some(520)), 30.), (Side::Open(317.), Side::Open(302.)));
        // ...then the right one is a strip and the rail stops at the centre's minimum...
        assert_eq!(sides(1000., hand(Some(480), Some(520)), 30.), (Side::Open(458.), Side::Strip));
        // ...then the PR #7 narrow layout.
        assert_eq!(sides(800., hand(Some(480), Some(520)), 30.), (Side::Open(312.), Side::Strip));
        assert_eq!(sides(700., hand(Some(480), Some(520)), 30.), (Side::Strip, Side::Strip));
    }

    /// A drag sets the width within the limits and the centre's minimum,
    /// and leaves the folds alone.
    #[test]
    fn a_drag_clamps_its_width() {
        let open = Panels::default();
        let rail = |window: f32, w: f32| dragged(window, open, true, w);
        // The layout's left inset (0 since the mark strip went); `centre`
        // below assumes the old 30 px, so these sides use the real one.
        let m = crate::chrome::MARK;
        let centre = |window: f32, (l, r): (Side, Side)| window - m - l.width() - r.width();
        assert_eq!(rail(1920., 400.), hand(Some(400), None));
        assert_eq!(sides(1920., rail(1920., 400.), m), (Side::Open(400.), Side::Open(296.)));
        assert_eq!(rail(1920., 1000.).rail_width, Some(480));
        assert_eq!(rail(1920., 200.).rail_width, Some(220));
        assert_eq!(rail(1920., 300.4).rail_width, Some(300), "whole pixels");
        // 1250 px: the centre keeps 480 px.
        assert_eq!(rail(1250., 480.).rail_width, Some(474));
        assert_eq!(centre(1250., sides(1250., rail(1250., 480.), m)), CENTRE_MIN);
        let right = dragged(1250., open, false, 520.);
        assert_eq!(right, hand(None, Some(458)));
        assert_eq!(sides(1250., right, m), (Side::Open(312.), Side::Open(458.)));
        assert_eq!(dragged(1920., open, false, 900.).right_width, Some(520));
        // 770 px, rail only: as wide as the narrow layout already shows it
        // (the auto rail gave way to 276), no wider.
        assert_eq!(rail(770., 400.).rail_width, Some(276));
        assert_eq!(rail(770., 240.).rail_width, Some(240));
        // Panels that had given way: the other one is held where it is, so
        // the dragged edge follows the pointer.
        let both = hand(Some(480), Some(520));
        let p = dragged(1150., both, true, 250.);
        assert_eq!(p, hand(Some(250), Some(331)));
        assert_eq!(sides(1150., p, m), (Side::Open(250.), Side::Open(331.)));
    }

    /// Dragged well past the narrowest width a panel folds, keeping the
    /// width it had for when it opens again; dragged back out (the same
    /// drag, or a new one from the strip) it opens and follows the pointer.
    #[test]
    fn a_drag_snaps_to_the_strip_and_back() {
        let set = hand(Some(400), None);
        let folded = dragged(1920., set, true, 131.);
        assert_eq!(folded, Panels { rail_collapsed: true, ..set });
        assert_eq!(sides(1920., folded, 30.).0, Side::Strip);
        assert_eq!(dragged(1920., set, true, 133.).rail_width, Some(220), "under the limit, over the snap: the limit");
        // Unfolding (a click, Ctrl+B) restores the dragged width.
        assert_eq!(sides(1920., toggled(1920., folded, true).unwrap(), 30.).0, Side::Open(400.));
        // From the strip: nothing until the snap distance, then open.
        assert_eq!(dragged(1920., folded, true, 60.), folded);
        assert_eq!(dragged(1920., folded, true, 300.), hand(Some(300), None));
        let right = dragged(1920., Panels::default(), false, 143.);
        assert!(right.right_collapsed && right.right_width.is_none());
        assert_eq!(dragged(1920., right, false, 144.), hand(None, Some(240)));
        // A window too narrow to open it: the strip stays.
        let both = Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() };
        assert_eq!(dragged(700., both, true, 300.), both);
        // 1000 px: the right one opens out of its strip by folding the rail, as a click does.
        let rail_only = Panels { right_collapsed: true, ..Panels::default() };
        assert_eq!(dragged(1000., rail_only, false, 300.), Panels { rail_collapsed: true, right_width: Some(300), ..Panels::default() });
    }

    /// A double-click on an edge forgets its width: the auto width again,
    /// wide windows widening it.
    #[test]
    fn a_reset_is_the_auto_width() {
        let p = dragged(2560., Panels::default(), true, 250.);
        assert_eq!(sides(2560., p, 30.).0, Side::Open(250.));
        let reset = Panels { rail_width: None, ..p };
        assert_eq!(reset, Panels::default());
        assert_eq!(sides(2560., reset, 30.).0, Side::Open(340.));
    }

    /// M3: a held fold key toggles once. Wayland/Windows repeat with no
    /// key-up; X11 sends a key-up right before each repeat.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn fold_key_repeats_are_ignored() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut k = FoldKeys::default();
        assert!(k.press(ms(0), false), "a press");
        // Held (no key-up): the repeats after the delay, then every 33 ms.
        assert!(!k.press(ms(500), false));
        assert!(!k.press(ms(533), false));
        k.release(ms(600));
        assert!(k.press(ms(700), false), "let go, pressed again");
        // X11: a key-up and its repeat at the same instant.
        k.release(ms(1200));
        assert!(!k.press(ms(1200), false));
        k.release(ms(1233));
        assert!(!k.press(ms(1234), false));
        // A real release, then a quick second press (a double tap).
        k.release(ms(1300));
        assert!(k.press(ms(1380), false));
        // A key-up this window never saw: a press long after is a press.
        assert!(!k.press(ms(2000), false), "within the first-repeat delay");
        assert!(k.press(ms(3200), false));
    }

    /// R1 (PR #7 re-review): AppKit drops B's key-up while Cmd is held, so on
    /// macOS a second press without a key-up is a press: fold, then unfold
    /// at once, must both work. X11's paired key-up still marks a repeat.
    #[test]
    #[cfg(target_os = "macos")]
    fn fold_key_without_a_key_up_is_a_press_on_macos() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut k = FoldKeys::default();
        assert!(k.press(ms(0), false), "fold");
        assert!(k.press(ms(300), false), "unfold, Cmd still held, no key-up seen");
        k.release(ms(400));
        assert!(!k.press(ms(400), false), "a key-up and a press at the same instant");
    }

    /// M1: a key-down that says it is a repeat (`is_held`: AppKit's
    /// `isARepeat`, Windows' repeat bit, Wayland) never acts, with no
    /// key-up ever seen (macOS drops it while Cmd is held): a held Cmd+T
    /// is one tab, a held Cmd+W closes one. A press after it acts again.
    #[test]
    fn a_held_key_down_never_acts() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut k = FoldKeys::default();
        assert!(k.press(ms(0), false), "the press");
        for n in 0..60 {
            assert!(!k.press(ms(250 + n * 30), true), "repeat {n}");
        }
        // Let go (the key-up lost), pressed again: a press, at once.
        assert!(k.press(ms(2100), false) || cfg!(not(target_os = "macos")), "macOS: a press without a key-up");
        k.release(ms(2200));
        assert!(k.press(ms(2400), false), "after a key-up, everywhere");
        assert!(!k.press(ms(2430), true));
    }

    /// H3: the card shows a long prompt whole, and AUTHORIZE waits until
    /// all of it has been on screen.
    #[test]
    fn a_long_prompt_is_shown_whole_and_must_be_read() {
        let mut lines: Vec<String> = (1..=30).map(|i| format!("line {i}")).collect();
        lines[13] = "  rm -rf ~/work   # line 14, indented".into();
        lines[20] = String::new();
        let prompt = lines.join("\n");
        let body = well_body(&prompt);
        assert_eq!(body.lines().count(), 30, "every line, the blank one too");
        assert_eq!(&*body, prompt.as_str(), "verbatim");
        // 30 lines in a 12-line well: 18 below at first.
        let below = 18. * 21.;
        assert_eq!(lines_below(below, 21.), 18);
        assert!(!authorize_ready(true, Some(below), false), "not read");
        assert!(!authorize_ready(true, None, false), "not laid out yet");
        assert!(!authorize_ready(false, Some(0.), true), "not armed");
        assert!(authorize_ready(true, Some(below), true), "scrolled to the end once");
        assert!(authorize_ready(true, Some(0.), false), "fits: nothing to scroll");
    }

    /// L2: the read gate counts what is under the window's bottom.
    #[test]
    fn a_well_under_the_window_bottom_is_not_read() {
        assert_eq!(hidden(0., 380., 420.), 0.);
        assert_eq!(hidden(0., 440., 420.), 20.);
        assert_eq!(hidden(42., 440., 420.), 62.);
        assert_eq!(hidden(-0.4, 300., 420.), 0.);
        assert!(!authorize_ready(true, Some(hidden(0., 440., 420.)), false));
    }

    /// A key event in [`answers`]: a key-down X11 says nothing of (`D`),
    /// one the platform marks held (`H`: Wayland, macOS, Windows), a
    /// key-up (`U`), a key-down the composer took (`C`: the card did not
    /// see it, [`CardChord`] did).
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum K {
        D,
        H,
        U,
        C,
    }

    /// The card's keys, as the window sees them: `(ms, event)` for one
    /// key (Enter or Esc), a card shown at `card` ms (its keys live from
    /// then, and again at each of `live_at`). Returns the times a key
    /// answered; the next card comes 40 ms after an answer.
    fn answers(card: u64, live_at: &[u64], keys: &[(u64, K)]) -> Vec<u64> {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let (mut arm, mut next): (Option<Arm>, Option<u64>) = (None, Some(card));
        let mut chord = FoldKeys::default();
        let mut out = Vec::new();
        let mut live = live_at.iter().peekable();
        let mut keys = keys.iter().peekable();
        for ms in 0..6000 {
            if next == Some(ms) {
                arm = Some(Arm::new(at(ms)));
                next = None;
            }
            if live.next_if_eq(&&ms).is_some()
                && let Some(a) = arm.as_mut()
            {
                a.restart(at(ms));
            }
            while let Some(&&(t, k)) = keys.peek() {
                if t != ms {
                    break;
                }
                keys.next();
                if k == K::U {
                    chord.release(at(ms));
                    continue;
                }
                let press = chord.press_on(at(ms), k == K::H, false);
                if k != K::C
                    && let Some(a) = arm.as_mut()
                    && a.key(at(ms), press)
                {
                    out.push(ms);
                    arm = None;
                    next = Some(ms + 40);
                }
            }
        }
        out
    }

    /// A tap: down at `t`, up 80 ms later.
    fn tap(t: u64) -> [(u64, K); 2] {
        [(t, K::D), (t + 80, K::U)]
    }

    /// A key pressed at `from` and held to `to`, its first repeat `delay`
    /// ms after the press and then every 40 ms, as `how` repeats it: X11
    /// with the pair's key-up dropped by GPUI (`D`), X11 with it kept
    /// (`U`: a key-up, the repeat 2 ms later), Wayland/macOS/Windows (`H`).
    fn held(from: u64, delay: u64, to: u64, how: K) -> Vec<(u64, K)> {
        let mut v = vec![(from, K::D)];
        let mut t = from + delay;
        while t < to {
            match how {
                K::U => v.extend([(t, K::U), (t + 2, K::D)]),
                K::H => v.push((t, K::H)),
                _ => v.push((t, K::D)),
            }
            t += 40;
        }
        v.push((to, K::U));
        v
    }

    /// H2 + M1, and the re-review's H1: a held Ctrl+Enter or Esc never
    /// answers a card that came up under it, however long the first repeat
    /// takes (X11 660 ms by default, KDE 600, GNOME 500, a tuned 250 or
    /// Windows' 1 s), on X11 (no "held" either way) or Wayland/macOS/
    /// Windows (repeats marked held); a press after the release, once the
    /// wait is over, does. The wait starts again when the keys go live.
    #[test]
    fn a_held_key_never_answers_the_card() {
        let arm = ARM.as_millis() as u64;
        for how in [K::D, K::U, K::H] {
            for delay in [250, 500, 600, 660, 1000] {
                // The press answers the first card (up at 0, armed); the next
                // comes 40 ms later, under the key held to 2000.
                let mut keys = held(1000, delay, 2000, how);
                assert_eq!(answers(0, &[], &keys), vec![1000], "{how:?} {delay}: the press answers, its repeats nothing");
                // The card comes 5-200 ms after the press, before the first
                // repeat: nothing answers it (it was `[600]` at 600 ms).
                for card in [1005, 1050, 1100, 1150, 1200] {
                    assert_eq!(answers(card, &[], &keys), Vec::<u64>::new(), "{how:?} {delay}: card at {card}");
                }
                // Let go and pressed again, once the last repeat's wait is
                // over: that answers.
                keys.extend(tap(2000 + arm + 1));
                assert_eq!(answers(1100, &[], &keys), vec![2000 + arm + 1], "{how:?} {delay}: a new press");
            }
        }
        // Held from before the card came, its first repeat 660 ms on.
        assert_eq!(answers(300, &[], &held(0, 660, 2000, K::D)), Vec::<u64>::new());
        // Esc held in the composer: its press gave the card the keys (live
        // at 1000), its repeats from 1660 do not deny.
        let mut esc = held(1000, 660, 2500, K::D);
        esc[0].1 = K::C;
        assert_eq!(answers(100, &[1000], &esc), Vec::<u64>::new());
        // A press right after a release is too soon (the last repeat
        // started the wait); a press after the wait answers.
        let keys = held(0, 600, 2000, K::D);
        let last = keys.iter().rev().find(|k| k.1 == K::D).unwrap().0;
        let mut k = keys.clone();
        k.extend(tap(last + 200));
        assert_eq!(answers(900, &[], &k), Vec::<u64>::new(), "too soon after the last repeat");
        let mut k = keys.clone();
        k.extend(tap(last + arm + 1));
        assert_eq!(answers(900, &[], &k), vec![last + arm + 1], "a new press after the wait");
        // A tap before the card is armed restarts the wait.
        let taps = [tap(300), tap(700), tap(1300)].concat();
        assert_eq!(answers(100, &[], &taps), vec![1300], "300 restarts: 700 is early too");
        assert_eq!(answers(100, &[], &tap(650)), vec![650]);
        // Keys that come live (the card came into view, Esc out of the
        // composer, the picker closed, the window came back) wait again.
        assert_eq!(answers(100, &[3000], &[tap(3200), tap(3800)].concat()), vec![3800]);
        // A double Esc: the first moved the keys to the card (live at 3000),
        // the second (3100) is not a DENY, nor the next one too soon after it.
        assert_eq!(answers(100, &[3000], &[tap(3100), tap(3300), tap(3900)].concat()), vec![3900]);
        // A press the platform marks held never answers, armed or not.
        assert_eq!(answers(100, &[], &[(2000, K::H)]), Vec::<u64>::new());
        // macOS drops Enter's key-up while Cmd is down: a press without one
        // is still a press there (its repeats say held).
        let mut c = CardChord::default();
        let t0 = Instant::now();
        assert!(c.enter.press_on(t0, false, true));
        assert!(c.enter.press_on(t0 + Duration::from_millis(300), false, true));
        assert!(!c.enter.press_on(t0 + Duration::from_millis(330), true, true));
        // Enter and Esc are tracked apart; other keys are not tracked.
        assert!(c.down("escape", t0, false) && !c.down("a", t0, false));
    }

    /// The buttons count from when the card was shown, not from when its
    /// keys came: a click moves the keys to the card and still lands.
    #[test]
    fn the_buttons_keep_their_own_arm() {
        let t0 = Instant::now();
        let mut a = Arm::new(t0);
        let later = t0 + ARM + Duration::from_millis(1000);
        a.restart(later);
        assert!(a.armed(later) && !a.keys_armed(later));
        assert!(a.keys_armed(later + ARM));
        assert!(!Arm::new(t0).armed(t0 + ARM - Duration::from_millis(1)));
    }

    /// H1: a secret card never sends an empty answer: Enter in the empty
    /// field (twice: the first Ctrl+Enter only moved the focus there), a
    /// held Enter after the answer went out (the field is cleared), SUBMIT
    /// with nothing typed. CANCEL sends nothing, to cancel.
    #[test]
    fn a_secret_card_never_sends_an_empty_answer() {
        assert_eq!(secret_answer(true, ""), None);
        assert_eq!(secret_answer(true, "hunter2"), Some(Some("hunter2".into())));
        assert_eq!(secret_answer(false, ""), Some(None));
        assert_eq!(secret_answer(false, "half"), Some(None));
        // A held Enter in the field: the first press sends, the field is
        // cleared, every repeat after it finds it empty.
        let mut field = String::from("s3cret");
        let mut sent = Vec::new();
        for _ in 0..30 {
            if let Some(v) = secret_answer(true, &field) {
                sent.push(v);
                field.clear();
            }
        }
        assert_eq!(sent, vec![Some("s3cret".to_string())]);
        // Two Enters in an empty field: nothing.
        assert_eq!([secret_answer(true, ""), secret_answer(true, "")], [None, None]);
    }

    /// L3 (re-review): after a secret card went, keys still coming to the
    /// window are its typing: `/` stays dead until a pause of [`ARM`].
    #[test]
    fn typing_meant_for_a_secret_card_keeps_find_shut() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // The card went at 0; keys at 120 ms intervals keep it going.
        let mut left = Some(t0);
        for n in 1..20 {
            left = still_typing(left, ms(n * 120));
            assert_eq!(left, Some(ms(n * 120)), "key {n}");
        }
        // A pause of ARM: the next key is the user's own.
        assert_eq!(still_typing(left, ms(19 * 120) + ARM), None);
        assert_eq!(still_typing(None, ms(5000)), None);
    }

    #[test]
    fn one_line_arguments() {
        assert_eq!(one_line("  \nfirst\nsecond", 80), "first …");
        assert_eq!(one_line("abcdef", 3), "abc …");
        let command = |s: &str| command_of(input_of(s).as_ref());
        assert_eq!(command(r#"{"command":"sleep 16 && echo step 1","timeout":30}"#).as_deref(), Some("sleep 16 && echo step 1"));
        assert_eq!(command(r#"{"path":"a.rs","limit":3}"#), None);
        assert_eq!(command(r#"{"command":"cut off at the prev"#), None, "a cut preview stays as it is");
        assert_eq!(command("ls"), None);
    }

    /// N-M1: a steer to scout, Esc out, scout finishes and its view goes,
    /// then the daemon refuses the steer. Its text comes back into the
    /// session's composer (the same tab) with the note that says it was a
    /// steer to scout, refused, and that Enter now sends it to the session.
    /// Only a switch to another tab's composer takes a worker note away.
    #[test]
    fn a_refused_steer_back_in_the_session_composer_says_so() {
        use gpui_kit::SharedString;
        use serde_json::json;
        use synaps_client::WatchReply;

        use super::frame_composer;
        use crate::subagents::{Live, Status};
        use crate::tabs::{Drafts, Place, Tabs};
        use crate::watch::{Candidate, Watch};

        let t0 = Instant::now();
        let mut w = Watch::default();
        w.open(&Candidate::of(3, "scout"), None, t0);
        let _ = w.take_poll(t0);
        let page = json!({"handle_id": "sa_3", "agent_name": "scout", "status": "running", "steerable": true, "next_seq": 1, "events": []});
        w.on_reply("sa_3", 0, WatchReply::Page(Box::new(serde_json::from_value(page).unwrap())), t0);
        assert_eq!(w.steer_target(), Some(Ok("sa_3".into())));
        w.steer_sent("sa_3", "look in crates/relay too");
        w.leave_on_esc(t0);
        let done = Live {
            id: 3,
            call: None,
            name: "scout".into(),
            status: Status::Done,
            line: String::new(),
            tools: None,
            elapsed: None,
            tick: None,
            watchable: true,
            demoted: false,
        };
        w.prune(t0, &[done]);
        assert!(w.worker("sa_3").is_none(), "its view has gone");
        assert!(w.steer_refused("scout (sa_3) is not running"));
        let (text, note) = w.take_unsent().expect("the session's thread is on screen");
        // `sync_composer`: the text goes back into the composer it came from.
        let mut tabs = Tabs::default();
        tabs.open("s", "s", Place::Current);
        let (mut drafts, mut owner) = (Drafts::default(), tabs.composer_tab().map(|(id, s)| (id, s.map(str::to_string))));
        let (mut draft_note, mut watch_note) = (None, Some(SharedString::from(note)));
        let out = frame_composer(&mut drafts, &mut owner, &tabs, Some(text), (&mut draft_note, &mut watch_note), ("", 0));
        assert_eq!(out, Some(("look in crates/relay too".to_string(), None)));
        assert_eq!(
            watch_note.as_deref(),
            Some("your steer to scout (sa_3) was refused (scout (sa_3) is not running): Enter now sends it to the session"),
            "the note shows with it"
        );
        // Another tab's composer: the note is not its.
        tabs.open("t", "t", Place::New);
        let _ = frame_composer(&mut drafts, &mut owner, &tabs, None, (&mut draft_note, &mut watch_note), ("x", 1));
        assert_eq!(watch_note, None);
    }

    /// The window crosses into a more compact form (full to short, short to
    /// cramped) while the list is scrolled back near its end: the card and
    /// composer give up room and the list's view grows at the bottom, where
    /// a long reply not drawn since the list was shown comes into view. Its
    /// parse is pending, so that layout is provisional and GPUI does not
    /// re-arm following on it. The hold waits; the next layout (the parse
    /// landed) reaches the end, GPUI re-arms, and the hold pauses it short
    /// of the end: still scrolled back. Spent on the provisional layout (as
    /// before), the next would have followed.
    #[test]
    fn a_grown_view_holds_back_past_a_reply_still_parsing() {
        use std::cell::Cell;
        let hold = Cell::new(true);
        // Frame F: provisional (the reply's parse pending), not following.
        assert!(!held_back(&hold, true, false));
        assert!(hold.get(), "not spent on a provisional layout");
        // Frame F+1: the parse landed; at the end, GPUI re-armed following.
        assert!(held_back(&hold, false, true), "paused short of the end");
        assert!(!hold.get());
        // Spent: later layouts leave following alone (a wheel to the end follows).
        assert!(!held_back(&hold, false, true));
        // The old rule (spent on the first layout, whatever it was) let F+1 follow.
        let old = |hold: &Cell<bool>, following: bool| hold.replace(false) && following;
        let hold = Cell::new(true);
        assert!(!old(&hold, false));
        assert!(!old(&hold, true), "F+1 followed");
        // No reply pending: spent at once, as before.
        let hold = Cell::new(true);
        assert!(held_back(&hold, false, true));
        let hold = Cell::new(true);
        assert!(!held_back(&hold, false, false) && !hold.get(), "not at the end: nothing to hold");
        // A row pending above the view (GPUI waits only for those it fills
        // the view with): GPUI re-armed all the same. Paused, and spent.
        let hold = Cell::new(true);
        assert!(held_back(&hold, true, true), "paused, a row pending or not");
        assert!(!hold.get() && !held_back(&hold, true, true));
        // The rule before held only once nothing was pending: it followed.
        let before = |hold: &Cell<bool>, pending: bool, following: bool| !pending && hold.replace(false) && following;
        assert!(!before(&Cell::new(true), true, true), "followed");
    }
}
